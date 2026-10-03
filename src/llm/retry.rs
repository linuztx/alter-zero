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
//!
//! Two kinds of failure, two policies. A host that **answers badly** — a
//! `503`, a reset mid-exchange, a stall — gets the bounded retry: a few
//! attempts a short backoff apart, then the error. A host that **cannot be
//! reached at all** ([`LlmError::Unreachable`]: the machine is offline, or the
//! host is) gets no budget and no deadline: the driver announces the outage
//! ([`StreamEvent::Offline`]), waits a few seconds ([`offline_backoff`]), and
//! tries again, for as long as it takes — Esc is the way out, and the attempt
//! that gets through is the one that streams. See `docs/offline.md`.

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
    /// No connection could be made to `host`: announce the outage with the
    /// running count of failed attempts (`attempt`, 1-based), wait `wait`,
    /// then try again — **never counted** against the bounded budget
    /// (`docs/offline.md`).
    AwaitConnection {
        attempt: u32,
        wait: Duration,
        host: String,
    },
}

/// Where a stream's attempts stand, as [`next_step`] reads them: how many of
/// the bounded retries have been spent, and how many connection attempts in a
/// row have failed since the host last answered. The two are kept apart
/// because they answer different questions — a host that cannot be reached
/// spends none of the budget meant for one that answers badly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Attempts {
    /// Bounded retries spent so far (`retrying {n}/{max}`'s `n`).
    pub retried: u32,
    /// Connection attempts that failed in a row — reset once the host answers
    /// at all, even with an error.
    pub unreachable: u32,
}

/// Is this failure worth retrying? Transport errors (the connection/send
/// failures the user is chasing) and the transient server statuses (`408`
/// request-timeout, `429` rate-limit, and the `5xx` family) are; a client error
/// (`4xx` auth/not-found/bad-request), a decode failure, and a cancellation are
/// not — retrying those just fails the same way.
#[must_use]
pub fn is_retryable(err: &LlmError) -> bool {
    match err {
        LlmError::Http(_) | LlmError::Unreachable { .. } => true,
        LlmError::Api { status, .. } => matches!(status, 408 | 429 | 500 | 502 | 503 | 504),
        LlmError::Decode(_) | LlmError::Cancelled => false,
    }
}

/// The first retry's backoff; each subsequent retry doubles it.
const BACKOFF_BASE: Duration = Duration::from_millis(500);
/// The ceiling on a single backoff, so a high retry count can't wait forever.
const BACKOFF_CAP: Duration = Duration::from_secs(8);

/// The backoff before the `retry_number`-th retry (1-based): exponential from
/// `BACKOFF_BASE`, doubling each retry, capped at `BACKOFF_CAP` — so the
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

/// The first wait for a lost connection; each failed attempt after it doubles
/// the wait up to [`OFFLINE_BACKOFF_CAP`].
const OFFLINE_BACKOFF_BASE: Duration = Duration::from_secs(1);
/// The longest wait between two connection attempts while offline. Short on
/// purpose: a connect that fails costs nothing while the network is gone, and
/// the turn should pick up within seconds of its return.
pub const OFFLINE_BACKOFF_CAP: Duration = Duration::from_secs(5);

/// The wait before the `attempt`-th connection attempt (1-based) while the
/// host cannot be reached: 1 s, 2 s, 4 s, then [`OFFLINE_BACKOFF_CAP`] for
/// every attempt after — for as long as the outage lasts.
#[must_use]
pub fn offline_backoff(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(16);
    OFFLINE_BACKOFF_BASE
        .checked_mul(1u32 << shift)
        .unwrap_or(OFFLINE_BACKOFF_CAP)
        .min(OFFLINE_BACKOFF_CAP)
}

/// Decide what to do after an attempt. Only a failure that has **not yet
/// emitted any content** (`emitted == false`) is ever re-sent — restarting a
/// request after bytes have streamed would duplicate them, so once anything
/// is emitted the error is surfaced instead — and with retries turned off
/// (`max == 0`) nothing is. Then: a host that could not be reached is
/// **waited for** ([`RetryStep::AwaitConnection`], the count running on
/// `attempts.unreachable` and no budget spent), and any other retryable
/// failure takes the bounded retry while `attempts.retried < max`.
#[must_use]
pub fn next_step(result: &AttemptResult, attempts: Attempts, emitted: bool, max: u32) -> RetryStep {
    let AttemptResult::Failed(err) = result else {
        return RetryStep::Proceed;
    };
    if emitted || max == 0 {
        return RetryStep::Proceed;
    }
    if let LlmError::Unreachable { host, .. } = err {
        let attempt = attempts.unreachable + 1;
        return RetryStep::AwaitConnection {
            attempt,
            wait: offline_backoff(attempt),
            host: host.clone(),
        };
    }
    if attempts.retried < max && is_retryable(err) {
        let number = attempts.retried + 1;
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
    attempt: impl FnMut() -> (AttemptResult, bool),
    sleep: impl Fn(Duration, &CancelToken),
) {
    // The terminal event depends only on the disposition [`run_attempts`]
    // returns; a cancel is a silent stop (the loop's interrupt path commits the
    // notice, mirroring the dummy).
    match run_attempts(tx, cancel, max, attempt, sleep) {
        AttemptResult::Ok => {
            let _ = tx.send(StreamEvent::StreamDone);
        }
        AttemptResult::Cancelled => {}
        AttemptResult::Failed(err) => {
            let _ = tx.send(StreamEvent::Error(err.to_string()));
        }
    }
}

/// Drive the retry sequence and **return** the final disposition instead of
/// sending a terminal event — announcing each [`StreamEvent::Retrying`] and
/// each [`StreamEvent::Offline`] on the way. This is the reusable core of
/// [`run_stream`]; the agentic tool loop ([`crate::llm::agent::run_agent`])
/// uses it directly, because *it* decides when the turn is really done (a
/// successful round that requested tool calls is not the end). Generic over
/// `attempt`/`sleep` for the same fake-driven tests. A cancel — mid-attempt,
/// during a backoff, or while waiting for the connection — returns
/// [`AttemptResult::Cancelled`].
pub fn run_attempts(
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
    max: u32,
    mut attempt: impl FnMut() -> (AttemptResult, bool),
    sleep: impl Fn(Duration, &CancelToken),
) -> AttemptResult {
    let mut attempts = Attempts::default();
    // Cumulative: once any attempt streams a byte we never retry (retrying
    // would duplicate the content). So the attempt that emits is always the
    // last one — succeed or give up.
    let mut emitted = false;
    loop {
        let (result, this_emitted) = attempt();
        emitted |= this_emitted;
        let wait = match next_step(&result, attempts, emitted, max) {
            RetryStep::Proceed => return result,
            RetryStep::Retry { number, wait } => {
                let _ = tx.send(StreamEvent::Retrying {
                    attempt: number,
                    max,
                });
                attempts.retried = number;
                // The host answered (badly): a later outage is a new one.
                attempts.unreachable = 0;
                wait
            }
            RetryStep::AwaitConnection {
                attempt: number,
                wait,
                host,
            } => {
                let _ = tx.send(StreamEvent::Offline {
                    host,
                    attempts: number,
                });
                attempts.unreachable = number;
                wait
            }
        };
        sleep(wait, cancel);
        // A cancel during the wait reaps us here — a silent stop, exactly as
        // a cancel mid-attempt would.
        if cancel.is_cancelled() {
            return AttemptResult::Cancelled;
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

    /// `retried` bounded retries spent, no connection attempt failed.
    const fn retried(n: u32) -> Attempts {
        Attempts {
            retried: n,
            unreachable: 0,
        }
    }

    fn unreachable(host: &str) -> AttemptResult {
        AttemptResult::Failed(LlmError::Unreachable {
            host: host.into(),
            message: "tcp connect error: Network is unreachable".into(),
        })
    }

    #[test]
    fn a_lost_connection_is_retryable() {
        assert!(is_retryable(&LlmError::Unreachable {
            host: "api.venice.ai".into(),
            message: "refused".into(),
        }));
    }

    #[test]
    fn the_offline_backoff_doubles_from_a_second_to_a_low_cap() {
        // Short on purpose: when the network returns, the turn should pick up
        // within seconds, and a failed connect costs nothing while it is gone.
        assert_eq!(offline_backoff(1), Duration::from_secs(1));
        assert_eq!(offline_backoff(2), Duration::from_secs(2));
        assert_eq!(offline_backoff(3), Duration::from_secs(4));
        assert_eq!(offline_backoff(4), OFFLINE_BACKOFF_CAP);
        assert_eq!(offline_backoff(100), OFFLINE_BACKOFF_CAP);
        assert!(OFFLINE_BACKOFF_CAP <= Duration::from_secs(5));
    }

    #[test]
    fn next_step_waits_for_the_connection_instead_of_spending_a_retry() {
        let step = next_step(
            &unreachable("api.venice.ai"),
            retried(0),
            false,
            MAX_RETRIES,
        );
        assert_eq!(
            step,
            RetryStep::AwaitConnection {
                attempt: 1,
                wait: offline_backoff(1),
                host: "api.venice.ai".into(),
            }
        );
    }

    #[test]
    fn the_wait_for_a_connection_is_unbounded() {
        // Every retry spent and a hundred failed connects later, the answer is
        // still to wait — the budget is for a host that answers badly, not
        // for one that cannot be reached (`docs/offline.md`).
        let attempts = Attempts {
            retried: MAX_RETRIES,
            unreachable: 100,
        };
        let step = next_step(&unreachable("api.venice.ai"), attempts, false, MAX_RETRIES);
        assert_eq!(
            step,
            RetryStep::AwaitConnection {
                attempt: 101,
                wait: OFFLINE_BACKOFF_CAP,
                host: "api.venice.ai".into(),
            }
        );
    }

    #[test]
    fn retries_turned_off_surface_a_lost_connection_at_once() {
        // `Error retry` 0 means never retry — a lost connection included.
        assert_eq!(
            next_step(&unreachable("api.venice.ai"), retried(0), false, 0),
            RetryStep::Proceed
        );
    }

    #[test]
    fn a_connection_lost_after_content_streamed_is_surfaced() {
        // Only a request that emitted nothing can be re-sent; a drop after
        // bytes streamed would duplicate them (the bounded rule's twin).
        assert_eq!(
            next_step(&unreachable("api.venice.ai"), retried(0), true, MAX_RETRIES),
            RetryStep::Proceed
        );
    }

    #[test]
    fn next_step_retries_a_content_free_transport_failure() {
        let step = next_step(
            &AttemptResult::Failed(LlmError::Http("boom".into())),
            retried(0),
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
            retried(0),
            true,
            MAX_RETRIES,
        );
        assert_eq!(step, RetryStep::Proceed);
    }

    #[test]
    fn next_step_stops_when_the_budget_is_exhausted() {
        let step = next_step(
            &AttemptResult::Failed(LlmError::Http("boom".into())),
            retried(MAX_RETRIES),
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
            retried(0),
            false,
            MAX_RETRIES,
        );
        assert_eq!(step, RetryStep::Proceed);
    }

    #[test]
    fn next_step_proceeds_on_success_and_cancel() {
        assert_eq!(
            next_step(&AttemptResult::Ok, retried(0), false, MAX_RETRIES),
            RetryStep::Proceed
        );
        assert_eq!(
            next_step(&AttemptResult::Cancelled, retried(0), false, MAX_RETRIES),
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
    fn a_lost_connection_waits_without_spending_the_budget_and_recovers() {
        // Offline for longer than the whole bounded budget would allow, then
        // the network comes back: the request goes through, every wait
        // announced with its running count and the host it waits for.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let mut calls = 0;
        let outages = MAX_RETRIES as usize + 2;
        run_stream(
            &tx,
            &cancel,
            MAX_RETRIES,
            || {
                calls += 1;
                if calls <= outages {
                    (unreachable("api.venice.ai"), false)
                } else {
                    let _ = tx.send(StreamEvent::Chunk("back".into()));
                    (AttemptResult::Ok, true)
                }
            },
            no_sleep,
        );
        assert_eq!(
            calls,
            outages + 1,
            "one attempt per outage, then the one that got through"
        );
        let mut expected: Vec<StreamEvent> = (1..=outages)
            .map(|n| StreamEvent::Offline {
                host: "api.venice.ai".into(),
                attempts: n as u32,
            })
            .collect();
        expected.push(StreamEvent::Chunk("back".into()));
        expected.push(StreamEvent::StreamDone);
        assert_eq!(drain(&mut rx), expected);
    }

    #[test]
    fn a_host_that_answers_badly_after_an_outage_takes_the_bounded_retry() {
        // Offline, then reachable but failing (503): the bounded retry opens
        // on its first number — the outage spent none of it — and the outage
        // count starts over once the host has answered.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let mut calls = 0;
        run_stream(
            &tx,
            &cancel,
            MAX_RETRIES,
            || {
                calls += 1;
                match calls {
                    1 => (unreachable("api.venice.ai"), false),
                    2 => (
                        AttemptResult::Failed(LlmError::Api {
                            status: 503,
                            body: String::new(),
                        }),
                        false,
                    ),
                    3 => (unreachable("api.venice.ai"), false),
                    _ => {
                        let _ = tx.send(StreamEvent::Chunk("ok".into()));
                        (AttemptResult::Ok, true)
                    }
                }
            },
            no_sleep,
        );
        assert_eq!(
            drain(&mut rx),
            vec![
                StreamEvent::Offline {
                    host: "api.venice.ai".into(),
                    attempts: 1
                },
                StreamEvent::Retrying {
                    attempt: 1,
                    max: MAX_RETRIES
                },
                StreamEvent::Offline {
                    host: "api.venice.ai".into(),
                    attempts: 1
                },
                StreamEvent::Chunk("ok".into()),
                StreamEvent::StreamDone,
            ]
        );
    }

    #[test]
    fn a_cancel_while_waiting_for_the_connection_stops_silently() {
        // Esc while offline: no further attempt, no Error — the loop's
        // interrupt path owns the outcome (the submission is handed back).
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let mut calls = 0;
        run_stream(
            &tx,
            &cancel,
            MAX_RETRIES,
            || {
                calls += 1;
                (unreachable("api.venice.ai"), false)
            },
            |_, c| c.cancel(),
        );
        assert_eq!(calls, 1);
        assert_eq!(
            drain(&mut rx),
            vec![StreamEvent::Offline {
                host: "api.venice.ai".into(),
                attempts: 1
            }]
        );
    }

    #[test]
    fn with_retries_off_a_lost_connection_is_the_surfaced_error() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let mut calls = 0;
        run_stream(
            &tx,
            &cancel,
            0,
            || {
                calls += 1;
                (unreachable("api.venice.ai"), false)
            },
            no_sleep,
        );
        assert_eq!(calls, 1, "never retried");
        match drain(&mut rx).as_slice() {
            [StreamEvent::Error(message)] => {
                assert!(
                    message.starts_with("could not reach api.venice.ai"),
                    "{message}"
                );
            }
            other => panic!("expected the surfaced error alone, got {other:?}"),
        }
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
