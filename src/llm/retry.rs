//! Retry policy for the streaming backend.
//!
//! The real backend talks to the network, so a request can fail with a
//! transient transport error (`request failed: error sending request for url…`)
//! or a retryable server status (429, 5xx). This module holds the **pure**
//! decision logic — which failures are worth retrying ([`is_retryable`]), how
//! long to wait ([`retry_backoff`]), and what to do after one attempt
//! ([`next_step`]) — plus a small generic driver ([`run_stream`]) that sequences
//! the attempts and announces each retry on the event channel. Only the actual
//! sleep ([`sleep_cancellable`]) touches the clock; everything else is unit-tested
//! with fakes and no network. See `docs/llm.md`.

use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use super::LlmError;
use crate::stream::{CancelToken, StreamEvent};

/// How many times a failed request is retried before the error is surfaced —
/// the user's "retry 3x". Attempts run as `1` initial + up to [`MAX_RETRIES`]
/// retries, the retries shown live as `retrying 1/3 … 3/3`.
pub const MAX_RETRIES: u32 = 3;

/// One streaming attempt's outcome, as the retry driver sees it (the success
/// payload is dropped — the driver only needs the disposition).
pub enum AttemptResult {
    /// The stream completed normally.
    Ok,
    /// The request was cancelled (Esc / quit) — a silent stop, never retried.
    Cancelled,
    /// The attempt failed with this error.
    Failed(LlmError),
}

/// What the driver should do after one attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum RetryStep {
    /// Stop looping and act on the outcome (send `StreamDone` / `Error` / nothing).
    Proceed,
    /// Announce this retry `number`, wait `wait`, then attempt again.
    Retry { number: u32, wait: Duration },
}

/// Is this failure worth retrying? Transport errors (the connection/send
/// failures the user is chasing) and the transient server statuses (`408`
/// request-timeout, `429` rate-limit, and the `5xx` family) are; a client error
/// (`4xx` auth/not-found/bad-request), a decode failure, and a cancellation are
/// not — retrying those just fails the same way.
#[must_use]
pub fn is_retryable(err: &LlmError) -> bool {
    match err {
        LlmError::Http(_) => true,
        LlmError::Api { status, .. } => matches!(status, 408 | 429 | 500 | 502 | 503 | 504),
        LlmError::Decode(_) | LlmError::Cancelled => false,
    }
}

/// The first retry's backoff; each subsequent retry doubles it.
const BACKOFF_BASE: Duration = Duration::from_millis(500);
/// The ceiling on a single backoff, so a high retry count can't wait forever.
const BACKOFF_CAP: Duration = Duration::from_secs(8);

/// The backoff before the `retry_number`-th retry (1-based): exponential from
/// [`BACKOFF_BASE`], doubling each retry, capped at [`BACKOFF_CAP`] — so the
/// three retries wait 500 ms, 1 s, then 2 s.
#[must_use]
pub fn retry_backoff(retry_number: u32) -> Duration {
    // 1-based: the 1st retry waits BACKOFF_BASE, each later one doubles. Clamp
    // the shift so a large count can't overflow the multiply (it caps anyway).
    let shift = retry_number.saturating_sub(1).min(16);
    BACKOFF_BASE
        .checked_mul(1u32 << shift)
        .unwrap_or(BACKOFF_CAP)
        .min(BACKOFF_CAP)
}

/// Decide what to do after an attempt: retry only a **retryable** failure that
/// has **not yet emitted any content** (`emitted == false`) and still has
/// budget (`attempt < max`) — retrying after bytes have streamed would duplicate
/// them, so once anything is emitted the error is surfaced instead.
#[must_use]
pub fn next_step(result: &AttemptResult, attempt: u32, emitted: bool, max: u32) -> RetryStep {
    if let AttemptResult::Failed(err) = result
        && !emitted
        && attempt < max
        && is_retryable(err)
    {
        let number = attempt + 1;
        return RetryStep::Retry {
            number,
            wait: retry_backoff(number),
        };
    }
    RetryStep::Proceed
}

/// Drive one streamed turn with retries. `attempt` performs a single streaming
/// attempt — it sends the `Chunk`/`Thinking*` events itself and returns its
/// outcome plus whether it emitted any content — and `sleep` is an interruptible
/// wait. On a retryable, content-free failure the driver announces a
/// [`StreamEvent::Retrying`], waits, and calls `attempt` again, up to `max`
/// times; then it sends the terminal `StreamDone` (success) / `Error` (give-up)
/// or nothing (cancelled). Generic over `attempt`/`sleep` so it is unit-testable
/// with fakes and no network.
pub fn run_stream(
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
    max: u32,
    mut attempt: impl FnMut() -> (AttemptResult, bool),
    sleep: impl Fn(Duration, &CancelToken),
) {
    let mut attempt_no = 0u32;
    // Cumulative: once any attempt streams a byte we never retry (retrying
    // would duplicate the content). So the attempt that emits is always the
    // last one — succeed or give up.
    let mut emitted = false;
    loop {
        let (result, this_emitted) = attempt();
        emitted |= this_emitted;
        match next_step(&result, attempt_no, emitted, max) {
            RetryStep::Proceed => {
                match result {
                    AttemptResult::Ok => {
                        let _ = tx.send(StreamEvent::StreamDone);
                    }
                    // A cancel is a silent stop — the loop's interrupt path
                    // commits the notice, not the backend (mirrors the dummy).
                    AttemptResult::Cancelled => {}
                    AttemptResult::Failed(err) => {
                        let _ = tx.send(StreamEvent::Error(err.to_string()));
                    }
                }
                return;
            }
            RetryStep::Retry { number, wait } => {
                let _ = tx.send(StreamEvent::Retrying {
                    attempt: number,
                    max,
                });
                attempt_no = number;
                sleep(wait, cancel);
                // A cancel during the backoff reaps us here — stop silently,
                // exactly as a cancel mid-attempt would.
                if cancel.is_cancelled() {
                    return;
                }
            }
        }
    }
}

/// Sleep up to `dur`, in short slices, returning early the moment `cancel` trips
/// — so an Esc/quit during a retry backoff is reaped promptly (mirrors the
/// dummy's `nap`). Boundary code: the module's only real clock use.
pub fn sleep_cancellable(dur: Duration, cancel: &CancelToken) {
    const SLICE: Duration = Duration::from_millis(50);
    let mut left = dur;
    while left > Duration::ZERO {
        if cancel.is_cancelled() {
            return;
        }
        let slice = SLICE.min(left);
        std::thread::sleep(slice);
        left = left.saturating_sub(slice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn transport_errors_are_retryable() {
        assert!(is_retryable(&LlmError::Http(
            "error sending request".into()
        )));
    }

    #[test]
    fn transient_server_statuses_are_retryable() {
        for status in [408, 429, 500, 502, 503, 504] {
            assert!(
                is_retryable(&LlmError::Api {
                    status,
                    body: String::new()
                }),
                "HTTP {status} should retry"
            );
        }
    }

    #[test]
    fn client_errors_and_non_transport_failures_are_not_retryable() {
        for status in [400, 401, 403, 404, 422] {
            assert!(
                !is_retryable(&LlmError::Api {
                    status,
                    body: String::new()
                }),
                "HTTP {status} should not retry"
            );
        }
        assert!(!is_retryable(&LlmError::Decode("bad shape".into())));
        assert!(!is_retryable(&LlmError::Cancelled));
    }

    #[test]
    fn backoff_grows_exponentially_and_caps() {
        assert_eq!(retry_backoff(1), Duration::from_millis(500));
        assert_eq!(retry_backoff(2), Duration::from_secs(1));
        assert_eq!(retry_backoff(3), Duration::from_secs(2));
        // Far-out retries never exceed the cap (and never overflow).
        assert!(retry_backoff(100) <= Duration::from_secs(8));
    }

    #[test]
    fn next_step_retries_a_content_free_transport_failure() {
        let step = next_step(
            &AttemptResult::Failed(LlmError::Http("boom".into())),
            0,
            false,
            MAX_RETRIES,
        );
        assert_eq!(
            step,
            RetryStep::Retry {
                number: 1,
                wait: retry_backoff(1)
            }
        );
    }

    #[test]
    fn next_step_stops_once_content_has_streamed() {
        // A failure after any bytes streamed can't be retried (it would
        // duplicate), so we surface it.
        let step = next_step(
            &AttemptResult::Failed(LlmError::Http("mid-stream drop".into())),
            0,
            true,
            MAX_RETRIES,
        );
        assert_eq!(step, RetryStep::Proceed);
    }

    #[test]
    fn next_step_stops_when_the_budget_is_exhausted() {
        let step = next_step(
            &AttemptResult::Failed(LlmError::Http("boom".into())),
            MAX_RETRIES,
            false,
            MAX_RETRIES,
        );
        assert_eq!(step, RetryStep::Proceed);
    }

    #[test]
    fn next_step_does_not_retry_a_non_retryable_error() {
        let step = next_step(
            &AttemptResult::Failed(LlmError::Api {
                status: 401,
                body: String::new(),
            }),
            0,
            false,
            MAX_RETRIES,
        );
        assert_eq!(step, RetryStep::Proceed);
    }

    #[test]
    fn next_step_proceeds_on_success_and_cancel() {
        assert_eq!(
            next_step(&AttemptResult::Ok, 0, false, MAX_RETRIES),
            RetryStep::Proceed
        );
        assert_eq!(
            next_step(&AttemptResult::Cancelled, 0, false, MAX_RETRIES),
            RetryStep::Proceed
        );
    }

    /// A no-op sleep so the driver tests don't wait.
    fn no_sleep(_: Duration, _: &CancelToken) {}

    /// Collect every event the driver sent.
    fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<StreamEvent>) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    #[test]
    fn a_clean_attempt_sends_stream_done_and_never_retries() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let mut calls = 0;
        run_stream(
            &tx,
            &cancel,
            MAX_RETRIES,
            || {
                calls += 1;
                let _ = tx.send(StreamEvent::Chunk("hi".into()));
                (AttemptResult::Ok, true)
            },
            no_sleep,
        );
        assert_eq!(calls, 1, "one attempt, no retry");
        assert_eq!(
            drain(&mut rx),
            vec![StreamEvent::Chunk("hi".into()), StreamEvent::StreamDone]
        );
    }

    #[test]
    fn a_content_free_failure_retries_then_succeeds() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let mut calls = 0;
        run_stream(
            &tx,
            &cancel,
            MAX_RETRIES,
            || {
                calls += 1;
                if calls == 1 {
                    // First attempt: connection failed before any byte.
                    (
                        AttemptResult::Failed(LlmError::Http("send failed".into())),
                        false,
                    )
                } else {
                    let _ = tx.send(StreamEvent::Chunk("recovered".into()));
                    (AttemptResult::Ok, true)
                }
            },
            no_sleep,
        );
        assert_eq!(calls, 2, "retried once, then succeeded");
        assert_eq!(
            drain(&mut rx),
            vec![
                StreamEvent::Retrying {
                    attempt: 1,
                    max: MAX_RETRIES
                },
                StreamEvent::Chunk("recovered".into()),
                StreamEvent::StreamDone,
            ]
        );
    }

    #[test]
    fn exhausting_the_retries_surfaces_the_error() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let mut calls = 0;
        run_stream(
            &tx,
            &cancel,
            MAX_RETRIES,
            || {
                calls += 1;
                (
                    AttemptResult::Failed(LlmError::Http("still down".into())),
                    false,
                )
            },
            no_sleep,
        );
        assert_eq!(
            calls,
            1 + MAX_RETRIES as usize,
            "initial + MAX_RETRIES tries"
        );
        let events = drain(&mut rx);
        assert_eq!(
            events[..3],
            [
                StreamEvent::Retrying {
                    attempt: 1,
                    max: MAX_RETRIES
                },
                StreamEvent::Retrying {
                    attempt: 2,
                    max: MAX_RETRIES
                },
                StreamEvent::Retrying {
                    attempt: 3,
                    max: MAX_RETRIES
                },
            ]
        );
        assert!(
            matches!(events.last(), Some(StreamEvent::Error(_))),
            "ends with the surfaced error"
        );
    }

    #[test]
    fn a_cancel_during_backoff_stops_silently() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let mut calls = 0;
        run_stream(
            &tx,
            &cancel,
            MAX_RETRIES,
            || {
                calls += 1;
                (AttemptResult::Failed(LlmError::Http("down".into())), false)
            },
            |_, c| c.cancel(), // the "sleep" trips the cancel, as an Esc would
        );
        assert_eq!(calls, 1, "no further attempts after the cancel");
        // Retrying was announced before the wait; then the cancel stops it with
        // no Error (a cancel is silent, like the dummy).
        assert_eq!(
            drain(&mut rx),
            vec![StreamEvent::Retrying {
                attempt: 1,
                max: MAX_RETRIES
            }]
        );
    }

    #[test]
    fn a_non_retryable_failure_surfaces_immediately() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let mut calls = 0;
        run_stream(
            &tx,
            &cancel,
            MAX_RETRIES,
            || {
                calls += 1;
                (
                    AttemptResult::Failed(LlmError::Api {
                        status: 401,
                        body: "bad key".into(),
                    }),
                    false,
                )
            },
            no_sleep,
        );
        assert_eq!(calls, 1, "an auth error is not retried");
        assert!(matches!(drain(&mut rx).as_slice(), [StreamEvent::Error(_)]));
    }
}
