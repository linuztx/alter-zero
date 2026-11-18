//! Frame scheduling — coalesce many redraw requests into rate-limited draws.
//!
//! One floor, [`MIN_FRAME_INTERVAL`], paces the frames a stream, an animation
//! or a worker's result asks for. The frame that shows a key press is not
//! held to it, nor to any floor of its own: it is drawn when it is asked for
//! (`docs/input-latency.md`).
//!
//! Ported from openai/codex's `FrameRequester`/`FrameScheduler`. The pure parts
//! ([`FrameRateLimiter`], [`soonest`]) are unit-tested with injected `Instant`s;
//! the async [`run_scheduler`] task that glues them to real time is covered by
//! `scripts/smoke.sh`, like the rest of the I/O boundary, and by tests that
//! run it on a runtime with no timer — which is how "a due frame never waits
//! on one" is held (`docs/input-latency.md`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::sleep_until;

/// The shortest gap between two emitted frames — 120 fps. A flood of redraw
/// requests can never produce frames closer together than this.
pub const MIN_FRAME_INTERVAL: Duration = Duration::from_nanos(8_333_334);

/// Rate-limits emitted frames to at most one per [`MIN_FRAME_INTERVAL`] —
/// all but the one a key press asks for ([`FrameRateLimiter::clamp`]).
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

    /// The earliest `request` may be emitted. A stream's, an animation's or a
    /// worker's waits for its slot ([`clamp_deadline`](Self::clamp_deadline)),
    /// so their requests coalesce into one draw a slot. A **key press**'s is
    /// due when it was asked for: a key is not one of many events to
    /// coalesce — a person types a few dozen a second — and typed just after
    /// a stream's frame it waited the rest of that slot to be shown, four
    /// milliseconds on average and eight at worst, where on an idle composer
    /// it is shown at once. Nor does it keep a shorter floor of its own: any
    /// wait at all goes through the runtime's timer, which counts in whole
    /// milliseconds, so a key that owed a fraction of one paid one to two
    /// (`docs/input-latency.md`). What bounds the paints is the loop, which
    /// takes every key already waiting before it draws; and since the frame
    /// a key draws counts as the last one ([`mark_emitted`](Self::mark_emitted)),
    /// the stream's next waits a whole slot past it.
    #[must_use]
    pub fn clamp(&self, request: FrameRequest) -> Instant {
        if request.input {
            request.at
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
    /// How many frames have been asked for through this handle and its
    /// clones — every request but the banner's own
    /// ([`schedule_banner_frame_in`](Self::schedule_banner_frame_in)).
    asked: Arc<AtomicU64>,
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
        self.asked.fetch_add(1, Ordering::Relaxed);
        // The scheduler outlives every requester, so a send only fails during
        // shutdown — nothing left to draw, so dropping the request is correct.
        let _ = self.tx.send(FrameRequest {
            at: Instant::now(),
            input: false,
        });
    }

    /// Request the redraw that shows the user's own input — a **key press**,
    /// a paste — at once, whatever the frame floor says
    /// ([`FrameRateLimiter::clamp`]). A stream's frames are paced by that
    /// floor because nothing is lost by showing a token eight milliseconds
    /// late; a key typed while they run is what the user is looking at.
    ///
    /// Counted in [`asked`](Self::asked) like every request but the
    /// banner's: a frame nothing counted asked for is drawn as the banner's
    /// alone, which would take the key and never show it.
    pub fn schedule_input_frame(&self) {
        self.asked.fetch_add(1, Ordering::Relaxed);
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
        self.asked.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(FrameRequest {
            at: Instant::now() + after,
            input: false,
        });
    }

    /// Request a redraw no sooner than `after` from now **on the banner's
    /// account alone**: the frame its mascot's next pose is due at
    /// (`docs/mascot-animation.md` *A frame of its own*). A deadline like
    /// any other to the scheduler, but not counted in [`asked`](Self::asked)
    /// — which is how the loop knows a frame nothing else asked for, and
    /// repaints the banner's cells without rendering the rest of the
    /// screen to find that it has not changed.
    pub fn schedule_banner_frame_in(&self, after: Duration) {
        let _ = self.tx.send(FrameRequest {
            at: Instant::now() + after,
            input: false,
        });
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
pub fn channel() -> (FrameRequester, UnboundedReceiver<FrameRequest>) {
    let (tx, rx) = unbounded_channel();
    let asked = Arc::default();
    (FrameRequester { tx, asked }, rx)
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
    fn a_keys_frame_skips_the_frame_floor() {
        let mut limiter = FrameRateLimiter::new();
        let base = Instant::now();
        limiter.mark_emitted(base);
        // A key typed right behind a stream's frame is due when it is typed:
        // a floor of its own, however short, is a wait on the runtime's
        // millisecond timer — one to two milliseconds for a key that asked
        // for a tenth of one (`docs/input-latency.md`).
        let typed = base + Duration::from_micros(100);
        let key = FrameRequest {
            at: typed,
            input: true,
        };
        assert_eq!(limiter.clamp(key), typed, "a key's frame is not held");
        // Everything else still waits for its slot…
        let chunk = FrameRequest {
            at: typed,
            input: false,
        };
        assert_eq!(limiter.clamp(chunk), base + MIN_FRAME_INTERVAL);
        // …and the frame a key draws counts as the last one, so typing adds
        // at most its own rate to the frame rate.
        limiter.mark_emitted(typed);
        assert_eq!(limiter.clamp(chunk), typed + MIN_FRAME_INTERVAL);
    }

    #[test]
    fn a_keys_frame_right_behind_a_streams_needs_no_timer() {
        // A reply is streaming, its frames a floor apart, and a key is typed
        // right behind one of them. Its frame is drawn at once — on a runtime
        // with no timer, which any wait at all would have needed.
        timerless().block_on(async {
            let (requester, req_rx) = channel();
            let (draw_tx, mut draw_rx) = unbounded_channel();
            let scheduler = tokio::spawn(run_scheduler(req_rx, draw_tx));
            requester.schedule_frame();
            assert_eq!(draw_rx.recv().await, Some(()));
            requester.schedule_input_frame();
            assert_eq!(
                draw_rx.recv().await,
                Some(()),
                "a key's frame behind a stream's is drawn at once, with no timer"
            );
            drop(requester);
            scheduler
                .await
                .expect("the scheduler ends when its requesters are gone");
        });
    }

    #[test]
    fn keys_handled_together_are_one_frame() {
        // With no floor of their own, what keeps a run of keys from being a
        // paint each is that the loop takes every key already waiting before
        // it draws (`tui::event_loop`): their requests reach the scheduler
        // together, and are one frame.
        timerless().block_on(async {
            let (requester, req_rx) = channel();
            let (draw_tx, mut draw_rx) = unbounded_channel();
            requester.schedule_input_frame();
            requester.schedule_input_frame();
            requester.schedule_input_frame();
            let scheduler = tokio::spawn(run_scheduler(req_rx, draw_tx));
            assert_eq!(draw_rx.recv().await, Some(()));
            drop(requester);
            scheduler
                .await
                .expect("the scheduler ends when its requesters are gone");
            assert!(
                draw_rx.try_recv().is_err(),
                "three keys handled together are one frame, not three"
            );
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
    fn a_keys_frame_is_never_the_banners_alone() {
        // A key's frame skips the frame floor, and it must still count as
        // asked for: the loop draws a frame nothing counted asked for as the
        // banner's alone — the mascot's cells and nothing else — so an
        // uncounted key would be taken and never shown
        // (`docs/mascot-animation.md` *A frame of its own*,
        // `docs/input-latency.md`).
        let (frame, mut rx) = channel();
        let mut ledger = FrameLedger::default();
        assert!(!ledger.banner_alone(frame.asked()), "the first frame");
        frame.schedule_input_frame();
        assert!(
            !ledger.banner_alone(frame.asked()),
            "the frame a key asked for is a whole one"
        );
        assert!(rx.try_recv().is_ok(), "and it reaches the scheduler");
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
