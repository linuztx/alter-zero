//! Retry policy for the streaming backend.
//!
//! The real backend talks to the network, so a request can fail with a
//! transient transport error (`request failed: error sending request for url…`)
//! or a retryable server status (429, 5xx) — or never leave the machine at
//! all, because the network is not there. This module holds the **pure**
//! decision logic — which failures are worth retrying ([`is_retryable`]),
//! which are a lost connection to wait out ([`is_offline`]), how long to wait
//! ([`retry_backoff`], [`offline_backoff`]), and what to do after one attempt
//! ([`next_step`]) — plus a small generic driver ([`run_stream`]) that
//! sequences the attempts and announces each retry and each offline wait on
//! the event channel. Only the actual sleep ([`sleep_cancellable`]) touches
//! the clock; everything else is unit-tested with fakes and no network. See
//! `docs/llm.md` and `docs/offline.md`.

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
    /// The network is not there: announce `wait`, sleep it, then attempt
    /// again — the `check`-th offline failure in a row. Spends no retry
    /// budget and has no ceiling (`docs/offline.md`).
    AwaitNetwork { check: u32, wait: Duration },
}

/// Is this failure worth retrying? Transport errors (the connection/send
/// failures the user is chasing) and the transient server statuses (`408`
/// request-timeout, `429` rate-limit, and the `5xx` family) are; a client error
/// (`4xx` auth/not-found/bad-request), a decode failure, and a cancellation are
/// not — retrying those just fails the same way.
#[must_use]
pub fn is_retryable(err: &LlmError) -> bool {
    match err {
        LlmError::Http(_) | LlmError::Offline(_) => true,
        LlmError::Api { status, .. } => matches!(status, 408 | 429 | 500 | 502 | 503 | 504),
        LlmError::Decode(_) | LlmError::Cancelled => false,
    }
}

/// Is this failure a **lost connection** — a request that never left the
/// machine because the network is not there ([`LlmError::Offline`])? The
/// driver waits those out instead of counting them against the budget. See
/// `docs/offline.md`.
#[must_use]
pub const fn is_offline(err: &LlmError) -> bool {
    matches!(err, LlmError::Offline(_))
}

/// The wait before the first re-send while offline; each later one doubles.
const OFFLINE_BACKOFF_BASE: Duration = Duration::from_secs(1);
/// The longest gap between two attempts while offline — short, because an
/// attempt against a dead network costs nothing (it fails before its body is
/// sent) and this is the most the turn lags behind a network that came back.
const OFFLINE_BACKOFF_CAP: Duration = Duration::from_secs(5);

/// The wait before re-sending after the `check`-th offline failure in a row
/// (1-based): 1 s, 2 s, 4 s, then every 5 s for as long as it takes.
#[must_use]
pub fn offline_backoff(check: u32) -> Duration {
    let shift = check.saturating_sub(1).min(16);
    OFFLINE_BACKOFF_BASE
        .checked_mul(1u32 << shift)
        .unwrap_or(OFFLINE_BACKOFF_CAP)
        .min(OFFLINE_BACKOFF_CAP)
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

/// Decide what to do after an attempt: retry only a **retryable** failure that
/// has **not yet emitted any content** (`emitted == false`) and still has
/// budget (`attempt < max`) — retrying after bytes have streamed would duplicate
/// them, so once anything is emitted the error is surfaced instead.
///
/// A content-free **offline** failure ([`is_offline`]) is the exception to
/// the budget: it is waited out ([`RetryStep::AwaitNetwork`]) however many
/// times it happens — `checks` is how many already have in a row — because
/// nothing left the machine and the network coming back is the only fix. A
/// budget of `0` still means *never retry*, the wait included.
#[must_use]
pub fn next_step(
    result: &AttemptResult,
    attempt: u32,
    emitted: bool,
    max: u32,
    checks: u32,
) -> RetryStep {
    if let AttemptResult::Failed(err) = result
        && !emitted
    {
        if is_offline(err) && max > 0 {
            let check = checks.saturating_add(1);
            return RetryStep::AwaitNetwork {
                check,
                wait: offline_backoff(check),
            };
        }
        if attempt < max && is_retryable(err) {
            let number = attempt + 1;
            return RetryStep::Retry {
                number,
                wait: retry_backoff(number),
            };
        }
    }
    RetryStep::Proceed
}

/// Drive one streamed turn with retries. `attempt` performs a single streaming
/// attempt — it sends the `Chunk`/`Thinking*` events itself and returns its
/// outcome plus whether it emitted any content — and `sleep` is an interruptible
/// wait. On a retryable, content-free failure the driver announces a
/// [`StreamEvent::Retrying`], waits, and calls `attempt` again, up to `max`
/// times — or, when the network is simply gone, announces a
/// [`StreamEvent::Offline`] wait and keeps re-sending for as long as it takes
/// (`docs/offline.md`); then it sends the terminal `StreamDone` (success) /
/// `Error` (give-up) or nothing (cancelled). Generic over `attempt`/`sleep` so
/// it is unit-testable with fakes and no network.
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
/// each [`StreamEvent::Offline`] wait on the way. This is the reusable core of
/// [`run_stream`]; the agentic tool loop ([`crate::llm::agent::run_agent`])
/// uses it directly, because *it* decides when the turn is really done (a
/// successful round that requested tool calls is not the end). Generic over
/// `attempt`/`sleep` for the same fake-driven tests. A cancel — mid-attempt,
/// during a backoff or during an offline wait — returns
/// [`AttemptResult::Cancelled`].
pub fn run_attempts(
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
    max: u32,
    mut attempt: impl FnMut() -> (AttemptResult, bool),
    sleep: impl Fn(Duration, &CancelToken),
) -> AttemptResult {
    let mut attempt_no = 0u32;
    // Cumulative: once any attempt streams a byte we never retry (retrying
    // would duplicate the content). So the attempt that emits is always the
    // last one — succeed or give up.
    let mut emitted = false;
    // Offline failures in a row — the wait's cadence, never its budget.
    let mut checks = 0u32;
    loop {
        let (result, this_emitted) = attempt();
        emitted |= this_emitted;
        match next_step(&result, attempt_no, emitted, max, checks) {
            RetryStep::Proceed => return result,
            RetryStep::AwaitNetwork { check, wait } => {
                let _ = tx.send(StreamEvent::Offline { wait });
                checks = check;
                sleep(wait, cancel);
                // Esc while waiting for the network is the same silent stop.
                if cancel.is_cancelled() {
                    return AttemptResult::Cancelled;
                }
            }
            RetryStep::Retry { number, wait } => {
                // Whatever failed this time, it was not the network being
                // gone, so a later offline spell starts its cadence over.
                checks = 0;
                let _ = tx.send(StreamEvent::Retrying {
                    attempt: number,
                    max,
                });
                attempt_no = number;
                sleep(wait, cancel);
                // A cancel during the backoff reaps us here — a silent stop,
                // exactly as a cancel mid-attempt would.
                if cancel.is_cancelled() {
                    return AttemptResult::Cancelled;
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
    fn an_offline_failure_is_retryable() {
        assert!(is_retryable(&LlmError::Offline("dns error".into())));
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
            0,
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
            0,
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
            0,
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
            0,
        );
        assert_eq!(step, RetryStep::Proceed);
    }

    #[test]
    fn next_step_proceeds_on_success_and_cancel() {
        assert_eq!(
            next_step(&AttemptResult::Ok, 0, false, MAX_RETRIES, 0),
            RetryStep::Proceed
        );
        assert_eq!(
            next_step(&AttemptResult::Cancelled, 0, false, MAX_RETRIES, 0),
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

    // --- waiting out a lost connection (docs/offline.md) -------------------

    fn offline() -> AttemptResult {
        AttemptResult::Failed(LlmError::Offline("dns error".into()))
    }

    #[test]
    fn only_an_offline_failure_is_offline() {
        assert!(is_offline(&LlmError::Offline("dns error".into())));
        assert!(!is_offline(&LlmError::Http("connection refused".into())));
        assert!(!is_offline(&LlmError::Api {
            status: 503,
            body: String::new()
        }));
        assert!(!is_offline(&LlmError::Cancelled));
    }

    #[test]
    fn the_offline_cadence_grows_then_holds_at_five_seconds() {
        assert_eq!(offline_backoff(1), Duration::from_secs(1));
        assert_eq!(offline_backoff(2), Duration::from_secs(2));
        assert_eq!(offline_backoff(3), Duration::from_secs(4));
        assert_eq!(offline_backoff(4), Duration::from_secs(5));
        // However long the network stays away, never longer between tries
        // (and never an overflow).
        assert_eq!(offline_backoff(10_000), Duration::from_secs(5));
    }

    #[test]
    fn an_offline_failure_is_waited_out_without_spending_the_budget() {
        // The budget is already spent, and still the step is to wait: a lost
        // connection is not a server misbehaving.
        assert_eq!(
            next_step(&offline(), MAX_RETRIES, false, MAX_RETRIES, 0),
            RetryStep::AwaitNetwork {
                check: 1,
                wait: offline_backoff(1)
            }
        );
    }

    #[test]
    fn the_offline_wait_has_no_ceiling() {
        assert_eq!(
            next_step(&offline(), 0, false, MAX_RETRIES, 1_000),
            RetryStep::AwaitNetwork {
                check: 1_001,
                wait: offline_backoff(1_001)
            }
        );
    }

    #[test]
    fn a_budget_of_zero_turns_the_offline_wait_off_too() {
        // `0` means never retry — the one knob for surfacing every failure.
        assert_eq!(next_step(&offline(), 0, false, 0, 0), RetryStep::Proceed);
    }

    #[test]
    fn an_offline_failure_after_content_streamed_is_surfaced() {
        assert_eq!(
            next_step(&offline(), 0, true, MAX_RETRIES, 0),
            RetryStep::Proceed
        );
    }

    #[test]
    fn the_driver_waits_out_a_lost_connection_until_it_returns() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let slept = std::cell::RefCell::new(Vec::new());
        let mut calls = 0;
        run_stream(
            &tx,
            &cancel,
            MAX_RETRIES,
            || {
                calls += 1;
                if calls <= 2 {
                    (offline(), false)
                } else {
                    let _ = tx.send(StreamEvent::Chunk("back".into()));
                    (AttemptResult::Ok, true)
                }
            },
            |wait, _| slept.borrow_mut().push(wait),
        );
        assert_eq!(calls, 3);
        assert_eq!(
            drain(&mut rx),
            vec![
                StreamEvent::Offline {
                    wait: offline_backoff(1)
                },
                StreamEvent::Offline {
                    wait: offline_backoff(2)
                },
                StreamEvent::Chunk("back".into()),
                StreamEvent::StreamDone,
            ],
            "each wait announced before it is slept, and no retry counter"
        );
        assert_eq!(
            slept.into_inner(),
            vec![offline_backoff(1), offline_backoff(2)],
            "the driver sleeps exactly what it announced"
        );
    }

    #[test]
    fn the_budget_is_whole_after_an_offline_spell() {
        // Five failed checks, then a server that keeps failing: the retry
        // budget it meets is the full three, counted from one.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let mut calls = 0;
        run_stream(
            &tx,
            &cancel,
            MAX_RETRIES,
            || {
                calls += 1;
                if calls <= 5 {
                    (offline(), false)
                } else {
                    (
                        AttemptResult::Failed(LlmError::Api {
                            status: 503,
                            body: String::new(),
                        }),
                        false,
                    )
                }
            },
            no_sleep,
        );
        assert_eq!(calls, 5 + 1 + MAX_RETRIES as usize);
        let events = drain(&mut rx);
        let offline_events = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::Offline { .. }))
            .count();
        assert_eq!(offline_events, 5);
        let retries: Vec<u32> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Retrying { attempt, .. } => Some(*attempt),
                _ => None,
            })
            .collect();
        assert_eq!(retries, vec![1, 2, 3]);
        assert!(matches!(events.last(), Some(StreamEvent::Error(_))));
    }

    #[test]
    fn a_new_offline_spell_starts_the_cadence_over() {
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
                    1 | 2 | 4 => (offline(), false),
                    3 => (
                        AttemptResult::Failed(LlmError::Http("connection reset".into())),
                        false,
                    ),
                    _ => (AttemptResult::Ok, true),
                }
            },
            no_sleep,
        );
        assert_eq!(
            drain(&mut rx),
            vec![
                StreamEvent::Offline {
                    wait: offline_backoff(1)
                },
                StreamEvent::Offline {
                    wait: offline_backoff(2)
                },
                StreamEvent::Retrying {
                    attempt: 1,
                    max: MAX_RETRIES
                },
                StreamEvent::Offline {
                    wait: offline_backoff(1)
                },
                StreamEvent::StreamDone,
            ]
        );
    }

    #[test]
    fn a_cancel_during_the_offline_wait_stops_silently() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let mut calls = 0;
        let result = run_attempts(
            &tx,
            &cancel,
            MAX_RETRIES,
            || {
                calls += 1;
                (offline(), false)
            },
            |_, c| c.cancel(),
        );
        assert!(matches!(result, AttemptResult::Cancelled));
        assert_eq!(calls, 1, "nothing is sent after the cancel");
        assert_eq!(
            drain(&mut rx),
            vec![StreamEvent::Offline {
                wait: offline_backoff(1)
            }]
        );
    }
}
