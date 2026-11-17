//! Frame scheduling — coalesce many redraw requests into rate-limited draws.
//!
//! Two floors: [`MIN_FRAME_INTERVAL`] paces the frames a stream, an animation
//! or a worker's result asks for, and [`MIN_INPUT_FRAME_INTERVAL`] — much
//! shorter — the frame that shows a key press, which is not made to wait out
//! the first (`docs/input-latency.md`).
//!
//! Ported from openai/codex's `FrameRequester`/`FrameScheduler`. The pure parts
//! ([`FrameRateLimiter`], [`soonest`]) are unit-tested with injected `Instant`s;
//! the async [`run_scheduler`] task that glues them to real time is covered by
//! `scripts/smoke.sh`, like the rest of the I/O boundary, and by tests that
//! run it on a runtime with no timer — which is how "a due frame never waits
//! on one" is held (`docs/input-latency.md`).

use std::time::{Duration, Instant};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::sleep_until;

/// The shortest gap between two emitted frames — 120 fps. A flood of redraw
/// requests can never produce frames closer together than this.
pub const MIN_FRAME_INTERVAL: Duration = Duration::from_nanos(8_333_334);

/// The shortest gap between the last frame and one a **key press** asks for
/// ([`FrameRequester::schedule_input_frame`]). A key is not paced by the
/// floor that paces a stream's frames: typed just after one of those, it
/// waited the rest of that floor — four milliseconds on average, eight at
/// worst — to be shown, where on an idle composer it is shown at once
/// (`docs/input-latency.md`). It keeps a floor of its own so that a run of
/// keys no burst detector caught is still a bounded number of paints.
pub const MIN_INPUT_FRAME_INTERVAL: Duration = Duration::from_millis(2);

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

    /// [`clamp_deadline`](Self::clamp_deadline) for a frame a key press asked
    /// for: never sooner than [`MIN_INPUT_FRAME_INTERVAL`] after the previous
    /// frame — its own short floor, not the stream's.
    #[must_use]
    pub fn clamp_input(&self, requested: Instant) -> Instant {
        match self.last_emitted_at {
            Some(last) => requested.max(last + MIN_INPUT_FRAME_INTERVAL),
            None => requested,
        }
    }

    /// The earliest `request` may be emitted — by the floor of its kind.
    #[must_use]
    pub fn clamp(&self, request: FrameRequest) -> Instant {
        if request.input {
            self.clamp_input(request.at)
        } else {
            self.clamp_deadline(request.at)
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
    tx: UnboundedSender<FrameRequest>,
}

/// One request for a frame, as a [`FrameRequester`] sends it to
/// [`run_scheduler`]: when it is wanted, and whether a key press wants it.
#[derive(Debug, Clone, Copy)]
pub struct FrameRequest {
    at: Instant,
    input: bool,
}

impl FrameRequester {
    /// Request a redraw as soon as the frame floor allows (immediately when idle).
    pub fn schedule_frame(&self) {
        // The scheduler outlives every requester, so a send only fails during
        // shutdown — nothing left to draw, so dropping the request is correct.
        let _ = self.tx.send(FrameRequest {
            at: Instant::now(),
            input: false,
        });
    }

    /// Request the redraw that shows a **key press** — as soon as
    /// [`MIN_INPUT_FRAME_INTERVAL`] allows, whatever the frame floor says. A
    /// stream's frames are paced by that floor because nothing is lost by
    /// showing a token eight milliseconds late; a key typed while they run
    /// is what the user is looking at.
    pub fn schedule_input_frame(&self) {
        let _ = self.tx.send(FrameRequest {
            at: Instant::now(),
            input: true,
        });
    }

    /// Request a redraw no sooner than `after` from now — a *relaxed* request
    /// (a detected input burst's characters, the status line's next animation
    /// tick) that doesn't demand an immediate frame. The scheduler still keeps
    /// the **soonest** pending deadline, so this never postpones a frame someone
    /// else already asked for — it only avoids piling immediate demands on the
    /// rate limiter (see [`crate::paste`] and `run_scheduler`).
    pub fn schedule_frame_in(&self, after: Duration) {
        let _ = self.tx.send(FrameRequest {
            at: Instant::now() + after,
            input: false,
        });
    }
}

/// Create a [`FrameRequester`] and the receiver its [`run_scheduler`] consumes.
#[must_use]
pub fn channel() -> (FrameRequester, UnboundedReceiver<FrameRequest>) {
    let (tx, rx) = unbounded_channel();
    (FrameRequester { tx }, rx)
}

/// The scheduler task: receive requested draw deadlines, coalesce them and
/// rate-limit to [`MIN_FRAME_INTERVAL`], and emit exactly one tick on `draw_tx`
/// per due frame. Returns when every [`FrameRequester`] has been dropped (or the
/// draw consumer hung up), so the runtime can shut down cleanly.
///
/// A frame that is **already due** is emitted at once, with no timer: the
/// runtime's timer wheel counts in whole milliseconds, so a frame sent through
/// it lands one to two milliseconds after it was asked for — which every
/// keystroke on an idle composer paid before its paint
/// (`docs/input-latency.md`). Only a frame still inside the floor, or one
/// asked for a beat out, waits on the timer.
///
/// Everything asked for before the task ran folds into the one pending frame
/// first, so the requests one turn of the loop makes (a key's handler, then
/// its bookkeeping) are one frame rather than one at once and a second a
/// floor later.
///
/// The timing is real, so this is exercised by `scripts/smoke.sh`; the pure
/// decisions it leans on ([`FrameRateLimiter::clamp_deadline`], [`soonest`]) are
/// unit-tested above, and the tests below drive the task itself on a runtime
/// with no timer.
pub async fn run_scheduler(
    mut req_rx: UnboundedReceiver<FrameRequest>,
    draw_tx: UnboundedSender<()>,
) {
    let mut limiter = FrameRateLimiter::new();
    let mut pending: Option<Instant> = None;
    loop {
        while let Ok(request) = req_rx.try_recv() {
            pending = Some(soonest(pending, limiter.clamp(request)));
        }
        let Some(deadline) = pending else {
            // Idle: nothing to draw until someone asks.
            match req_rx.recv().await {
                Some(request) => pending = Some(limiter.clamp(request)),
                None => break, // all requesters dropped → nothing more to draw
            }
            continue;
        };
        let now = Instant::now();
        if deadline <= now {
            // Recorded as emitted *now*, not at its deadline: a frame the
            // loop was too busy to ask for on time must still hold the next
            // one a whole floor away.
            limiter.mark_emitted(now);
            pending = None;
            if draw_tx.send(()).is_err() {
                break; // the loop hung up → stop scheduling
            }
            continue;
        }
        let sleep = sleep_until(tokio::time::Instant::from_std(deadline));
        tokio::pin!(sleep);
        tokio::select! {
            request = req_rx.recv() => {
                match request {
                    // Fold the (rate-limit-clamped) request into the pending
                    // deadline, keeping the soonest, and re-loop to re-arm.
                    Some(request) => {
                        pending = Some(soonest(pending, limiter.clamp(request)));
                    }
                    None => break,
                }
            }
            () = &mut sleep => {
                limiter.mark_emitted(deadline);
                pending = None;
                if draw_tx.send(()).is_err() {
                    break;
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

    /// A runtime with **no time driver**: any path that arms a timer panics
    /// on it, which ends the scheduler and closes its draw channel — so a
    /// tick that arrives here was emitted without one.
    fn timerless() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime")
    }

    #[test]
    fn a_due_frame_is_emitted_without_a_timer() {
        // An idle keystroke's frame is due the moment it is asked for. Sent
        // through `sleep_until` it waited on the runtime's timer wheel, whose
        // millisecond granularity put one to two milliseconds between every
        // key press and the paint that shows it (`docs/input-latency.md`).
        timerless().block_on(async {
            let (requester, req_rx) = channel();
            let (draw_tx, mut draw_rx) = unbounded_channel();
            let scheduler = tokio::spawn(run_scheduler(req_rx, draw_tx));
            requester.schedule_frame();
            assert_eq!(
                draw_rx.recv().await,
                Some(()),
                "a frame that is already due is drawn at once, with no timer"
            );
            drop(requester);
            scheduler
                .await
                .expect("the scheduler ends when its requesters are gone");
        });
    }

    #[test]
    fn requests_made_together_are_one_frame() {
        // One turn of the loop often asks more than once (a key's handler,
        // then its bookkeeping). Emitting the first at once must not leave
        // the rest to draw a second, identical frame a beat later.
        timerless().block_on(async {
            let (requester, req_rx) = channel();
            let (draw_tx, mut draw_rx) = unbounded_channel();
            requester.schedule_frame();
            requester.schedule_frame();
            requester.schedule_frame();
            let scheduler = tokio::spawn(run_scheduler(req_rx, draw_tx));
            assert_eq!(draw_rx.recv().await, Some(()));
            drop(requester);
            scheduler
                .await
                .expect("the scheduler ends when its requesters are gone");
            assert!(
                draw_rx.try_recv().is_err(),
                "three requests made together are one frame, not three"
            );
        });
    }

    #[test]
    fn a_keys_frame_waits_only_its_own_floor() {
        let mut limiter = FrameRateLimiter::new();
        let base = Instant::now();
        assert_eq!(limiter.clamp_input(base), base, "idle: draw when asked");
        limiter.mark_emitted(base);
        // A key typed a millisecond after a stream's frame: shown two
        // milliseconds after that frame, not eight.
        let typed = base + Duration::from_millis(1);
        assert_eq!(limiter.clamp_input(typed), base + MIN_INPUT_FRAME_INTERVAL);
        assert_eq!(limiter.clamp_deadline(typed), base + MIN_FRAME_INTERVAL);
        // …and one typed later than its floor, when it is typed.
        let later = base + Duration::from_millis(5);
        assert_eq!(limiter.clamp_input(later), later);
    }

    #[test]
    fn a_keys_frame_does_not_wait_out_a_streams_floor() {
        // A reply is streaming, its frames a floor apart, and a key is typed
        // right behind one of them. Its frame is due once its own short
        // floor has passed — on a runtime with no timer, which the wait for
        // the stream's floor would have needed.
        timerless().block_on(async {
            let (requester, req_rx) = channel();
            let (draw_tx, mut draw_rx) = unbounded_channel();
            let scheduler = tokio::spawn(run_scheduler(req_rx, draw_tx));
            requester.schedule_frame();
            assert_eq!(draw_rx.recv().await, Some(()));
            std::thread::sleep(MIN_INPUT_FRAME_INTERVAL);
            requester.schedule_input_frame();
            assert_eq!(
                draw_rx.recv().await,
                Some(()),
                "a key's frame behind a stream's is drawn without the stream's floor"
            );
            drop(requester);
            scheduler
                .await
                .expect("the scheduler ends when its requesters are gone");
        });
    }

    #[test]
    fn a_keys_frame_right_behind_another_waits_out_its_own_floor() {
        // The short floor is still a floor: keys no burst detector caught
        // are not a paint each.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("a runtime");
        runtime.block_on(async {
            let (requester, req_rx) = channel();
            let (draw_tx, mut draw_rx) = unbounded_channel();
            let scheduler = tokio::spawn(run_scheduler(req_rx, draw_tx));
            let asked = Instant::now();
            requester.schedule_input_frame();
            assert_eq!(draw_rx.recv().await, Some(()));
            requester.schedule_input_frame();
            assert_eq!(draw_rx.recv().await, Some(()));
            assert!(
                asked.elapsed() >= MIN_INPUT_FRAME_INTERVAL,
                "the second frame came {:?} after the first was asked for",
                asked.elapsed()
            );
            drop(requester);
            scheduler
                .await
                .expect("the scheduler ends when its requesters are gone");
        });
    }

    #[test]
    fn a_frame_right_behind_another_still_waits_out_the_floor() {
        // The floor is what coalesces a stream's requests: emitting due
        // frames at once must not let two land closer than it.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("a runtime");
        runtime.block_on(async {
            let (requester, req_rx) = channel();
            let (draw_tx, mut draw_rx) = unbounded_channel();
            let scheduler = tokio::spawn(run_scheduler(req_rx, draw_tx));
            let asked = Instant::now();
            requester.schedule_frame();
            assert_eq!(draw_rx.recv().await, Some(()));
            requester.schedule_frame();
            assert_eq!(draw_rx.recv().await, Some(()));
            assert!(
                asked.elapsed() >= MIN_FRAME_INTERVAL,
                "the second frame came {:?} after the first was asked for",
                asked.elapsed()
            );
            drop(requester);
            scheduler
                .await
                .expect("the scheduler ends when its requesters are gone");
        });
    }
}
