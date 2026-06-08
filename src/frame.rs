//! Frame scheduling — coalesce many redraw requests into rate-limited draws.
//!
//! Ported from openai/codex's `FrameRequester`/`FrameScheduler`. The pure parts
//! ([`FrameRateLimiter`], [`soonest`]) are unit-tested with injected `Instant`s;
//! the async [`run_scheduler`] task that glues them to real time is covered by
//! `scripts/smoke.sh`, like the rest of the I/O boundary.

use std::time::{Duration, Instant};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::sleep_until;

/// The shortest gap between two emitted frames — 120 fps. A flood of redraw
/// requests can never produce frames closer together than this.
pub const MIN_FRAME_INTERVAL: Duration = Duration::from_nanos(8_333_334);

/// The scheduler's idle sleep when nothing is pending — effectively "never" until
/// the next request wakes it (mirrors codex's `ONE_YEAR` placeholder).
const IDLE_SLEEP: Duration = Duration::from_secs(60 * 60 * 24 * 365);

/// Rate-limits emitted frames to at most one per [`MIN_FRAME_INTERVAL`].
///
/// Pure: it makes decisions from injected `Instant`s and remembers only when the
/// last frame was emitted, so it is testable with no clock or runtime.
#[derive(Debug, Default)]
pub struct FrameRateLimiter {
    /// When the most recent frame was emitted; `None` until the first one.
    last_emitted_at: Option<Instant>,
}

impl FrameRateLimiter {
    /// A fresh limiter that has emitted nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The earliest a frame requested at `requested` may actually be emitted:
    /// never sooner than [`MIN_FRAME_INTERVAL`] after the previous frame. With no
    /// previous frame the request stands as-is (an idle keystroke draws at once).
    #[must_use]
    pub fn clamp_deadline(&self, requested: Instant) -> Instant {
        match self.last_emitted_at {
            Some(last) => requested.max(last + MIN_FRAME_INTERVAL),
            None => requested,
        }
    }

    /// Record that a frame was emitted for deadline `at`, so the next request is
    /// clamped to at least [`MIN_FRAME_INTERVAL`] later.
    pub fn mark_emitted(&mut self, at: Instant) {
        self.last_emitted_at = Some(at);
    }
}

/// Fold a newly-requested (already clamped) `deadline` into the `pending` one,
/// keeping the **soonest** — so many requests between two frames coalesce into a
/// single draw at the earliest of their deadlines.
#[must_use]
pub fn soonest(pending: Option<Instant>, deadline: Instant) -> Instant {
    match pending {
        Some(p) => p.min(deadline),
        None => deadline,
    }
}

/// A cloneable handle for asking the [`run_scheduler`] task to draw a frame. The
/// loop holds one and calls [`schedule_frame`] after every state change; the
/// scheduler coalesces a burst of these into a single rate-limited draw.
///
/// [`schedule_frame`]: FrameRequester::schedule_frame
#[derive(Debug, Clone)]
pub struct FrameRequester {
    tx: UnboundedSender<Instant>,
}

impl FrameRequester {
    /// Request a redraw as soon as the frame floor allows (immediately when idle).
    pub fn schedule_frame(&self) {
        // The scheduler outlives every requester, so a send only fails during
        // shutdown — nothing left to draw, so dropping the request is correct.
        let _ = self.tx.send(Instant::now());
    }

    /// Request a redraw no sooner than `after` from now — used to defer a paint
    /// to the end of a detected input burst (see [`crate::paste`]).
    pub fn schedule_frame_in(&self, after: Duration) {
        let _ = self.tx.send(Instant::now() + after);
    }
}

/// Create a [`FrameRequester`] and the receiver its [`run_scheduler`] consumes.
#[must_use]
pub fn channel() -> (FrameRequester, UnboundedReceiver<Instant>) {
    let (tx, rx) = unbounded_channel();
    (FrameRequester { tx }, rx)
}

/// The scheduler task: receive requested draw deadlines, coalesce them and
/// rate-limit to [`MIN_FRAME_INTERVAL`], and emit exactly one tick on `draw_tx`
/// per due frame. Returns when every [`FrameRequester`] has been dropped (or the
/// draw consumer hung up), so the runtime can shut down cleanly.
///
/// The timing is real, so this is exercised by `scripts/smoke.sh`; the pure
/// decisions it leans on ([`FrameRateLimiter::clamp_deadline`], [`soonest`]) are
/// unit-tested above.
pub async fn run_scheduler(mut req_rx: UnboundedReceiver<Instant>, draw_tx: UnboundedSender<()>) {
    let mut limiter = FrameRateLimiter::new();
    let mut pending: Option<Instant> = None;
    loop {
        let target = pending.unwrap_or_else(|| Instant::now() + IDLE_SLEEP);
        let sleep = sleep_until(tokio::time::Instant::from_std(target));
        tokio::pin!(sleep);
        tokio::select! {
            requested = req_rx.recv() => {
                match requested {
                    // Fold the (rate-limit-clamped) request into the pending
                    // deadline, keeping the soonest, and re-loop to re-arm the sleep.
                    Some(requested) => {
                        let clamped = limiter.clamp_deadline(requested);
                        pending = Some(soonest(pending, clamped));
                    }
                    None => break, // all requesters dropped → nothing more to draw
                }
            }
            // Only fires while a frame is pending (idle, the year-long sleep never
            // wins). Emit one tick, record it for the next clamp, and go idle.
            () = &mut sleep, if pending.is_some() => {
                limiter.mark_emitted(target);
                pending = None;
                if draw_tx.send(()).is_err() {
                    break; // the loop hung up → stop scheduling
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_with_no_prior_frame_keeps_the_request() {
        let limiter = FrameRateLimiter::new();
        let t = Instant::now();
        assert_eq!(limiter.clamp_deadline(t), t, "idle: draw when asked");
    }

    #[test]
    fn clamp_pushes_a_too_soon_request_to_the_frame_floor() {
        let mut limiter = FrameRateLimiter::new();
        let base = Instant::now();
        limiter.mark_emitted(base);
        // Requested only 2ms later, but the floor is 8.33ms past the last frame.
        let requested = base + Duration::from_millis(2);
        assert_eq!(
            limiter.clamp_deadline(requested),
            base + MIN_FRAME_INTERVAL,
            "a request inside the frame interval is delayed to the floor"
        );
    }

    #[test]
    fn clamp_leaves_a_late_enough_request_alone() {
        let mut limiter = FrameRateLimiter::new();
        let base = Instant::now();
        limiter.mark_emitted(base);
        // Requested well past the interval — no need to delay it.
        let requested = base + Duration::from_millis(20);
        assert_eq!(limiter.clamp_deadline(requested), requested);
    }

    #[test]
    fn soonest_takes_the_pending_deadline_when_it_is_earlier() {
        let base = Instant::now();
        let earlier = base;
        let later = base + Duration::from_millis(5);
        assert_eq!(soonest(Some(earlier), later), earlier);
    }

    #[test]
    fn soonest_takes_the_new_deadline_when_it_is_earlier() {
        let base = Instant::now();
        let later = base + Duration::from_millis(5);
        assert_eq!(soonest(Some(later), base), base);
    }

    #[test]
    fn soonest_with_nothing_pending_is_the_new_deadline() {
        let t = Instant::now();
        assert_eq!(soonest(None, t), t);
    }
}
