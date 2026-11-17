//! Frame scheduling — coalesce many redraw requests into rate-limited draws.
//!
//! Ported from openai/codex's `FrameRequester`/`FrameScheduler`. The pure parts
//! ([`FrameRateLimiter`], [`soonest`]) are unit-tested with injected `Instant`s;
//! the async [`run_scheduler`] task that glues them to real time is covered by
//! `scripts/smoke.sh`, like the rest of the I/O boundary.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// How many frames have been asked for through this handle and its
    /// clones — every request but the banner's own
    /// ([`schedule_banner_frame_in`](Self::schedule_banner_frame_in)).
    asked: Arc<AtomicU64>,
}

impl FrameRequester {
    /// Request a redraw as soon as the frame floor allows (immediately when idle).
    pub fn schedule_frame(&self) {
        self.asked.fetch_add(1, Ordering::Relaxed);
        // The scheduler outlives every requester, so a send only fails during
        // shutdown — nothing left to draw, so dropping the request is correct.
        let _ = self.tx.send(Instant::now());
    }

    /// Request a redraw no sooner than `after` from now — a *relaxed* request
    /// (a detected input burst's characters, the status line's next animation
    /// tick) that doesn't demand an immediate frame. The scheduler still keeps
    /// the **soonest** pending deadline, so this never postpones a frame someone
    /// else already asked for — it only avoids piling immediate demands on the
    /// rate limiter (see [`crate::paste`] and `run_scheduler`).
    pub fn schedule_frame_in(&self, after: Duration) {
        self.asked.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(Instant::now() + after);
    }

    /// Request a redraw no sooner than `after` from now **on the banner's
    /// account alone**: the frame its mascot's next pose is due at
    /// (`docs/mascot-animation.md` *A frame of its own*). A deadline like
    /// any other to the scheduler, but not counted in [`asked`](Self::asked)
    /// — which is how the loop knows a frame nothing else asked for, and
    /// repaints the banner's cells without rendering the rest of the
    /// screen to find that it has not changed.
    pub fn schedule_banner_frame_in(&self, after: Duration) {
        let _ = self.tx.send(Instant::now() + after);
    }

    /// How many frames have been asked for so far by anything but
    /// [`schedule_banner_frame_in`](Self::schedule_banner_frame_in), through
    /// this handle or any clone of it.
    #[must_use]
    pub fn asked(&self) -> u64 {
        self.asked.load(Ordering::Relaxed)
    }
}

/// Tells the frames **only the banner asked for** from every other
/// (`docs/mascot-animation.md` *A frame of its own*).
///
/// The scheduler folds every pending request into one draw tick, so a tick
/// does not say who wanted it. The count of requests does
/// ([`FrameRequester::asked`], which leaves the banner's own out): read as
/// each frame begins, a count that has not moved since the frame before
/// began means nothing asked in between but the banner — nothing else on
/// screen can have changed, and whatever else re-arms itself would have
/// asked while that frame was drawn. The first frame of all is never the
/// banner's alone.
#[derive(Debug, Default)]
pub struct FrameLedger {
    /// [`FrameRequester::asked`] as the last frame began.
    seen: Option<u64>,
}

impl FrameLedger {
    /// Whether the frame beginning now, with `asked` frames asked for so
    /// far, is one only the banner asked for — and note the count for the
    /// frame after it.
    pub fn banner_alone(&mut self, asked: u64) -> bool {
        self.seen.replace(asked) == Some(asked)
    }
}

/// Create a [`FrameRequester`] and the receiver its [`run_scheduler`] consumes.
#[must_use]
pub fn channel() -> (FrameRequester, UnboundedReceiver<Instant>) {
    let (tx, rx) = unbounded_channel();
    let asked = Arc::default();
    (FrameRequester { tx, asked }, rx)
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

    #[test]
    fn a_frame_asked_for_on_the_banners_account_is_not_counted() {
        // What tells a frame nothing but the banner's mascot asked for from
        // every other (`docs/mascot-animation.md` *A frame of its own*):
        // each request is counted but the banner's own — which is a
        // deadline for the scheduler all the same.
        let (frame, mut rx) = channel();
        assert_eq!(frame.asked(), 0);
        frame.schedule_frame();
        frame.schedule_frame_in(Duration::from_millis(5));
        assert_eq!(frame.asked(), 2);
        frame.schedule_banner_frame_in(Duration::from_millis(5));
        assert_eq!(frame.asked(), 2, "the banner's own request");
        // A clone asks on the same count: a worker's request is one too.
        frame.clone().schedule_frame();
        assert_eq!(frame.asked(), 3);
        let mut deadlines = 0;
        while rx.try_recv().is_ok() {
            deadlines += 1;
        }
        assert_eq!(deadlines, 4, "every request reaches the scheduler");
    }

    #[test]
    fn a_frame_is_the_banners_alone_when_nothing_else_asked_since_the_last_one() {
        let mut ledger = FrameLedger::default();
        // The first frame of all is nobody's alone, whatever the count.
        assert!(!ledger.banner_alone(0));
        // Nothing has asked since it began: the frame now due can only be
        // one the banner asked for.
        assert!(ledger.banner_alone(0));
        assert!(ledger.banner_alone(0), "and the one after it");
        // Anything asking makes the next frame a whole one — once.
        assert!(!ledger.banner_alone(1));
        assert!(ledger.banner_alone(1));
        assert!(!ledger.banner_alone(4));
        assert!(ledger.banner_alone(4));
    }
}
