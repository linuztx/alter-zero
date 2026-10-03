//! The `AskUserQuestion` idle clock's boundary half (`docs/ask.md`): the
//! instants the pure [`AskTimer`] is read with, and what its readings do.
//!
//! A question waits on the user, and the user may be away. Every key press or
//! paste starts the wait over ([`Session::note_user_activity`]); the draw tick
//! reads the clock before the paint ([`Session::tick_ask_clock`]) and, once
//! the wait runs out, resolves every waiting question unanswered
//! ([`App::expire_asks`]) and posts the decisions on the gate, so the parked
//! tool thread wakes and the model is told the user is away and to keep
//! working. Every rule of the clock itself lives in [`AskTimer`] and is
//! unit-tested there; only `Instant::now()` and the frame booking are here.
//!
//! [`AskTimer`]: alter_zero::ask::AskTimer
//! [`App::expire_asks`]: alter_zero::app::App::expire_asks

use std::time::{Duration, Instant};

use alter_zero::ask::AskClock;

use super::Session;

/// How often the open modal's countdown moves: once a second, the rate its
/// `m:ss` changes at — the `/login` device page's cadence.
const ASK_TICK_INTERVAL: Duration = Duration::from_secs(1);

/// The furthest ahead a frame is booked under an overlay, where the countdown
/// is not on screen and only the expiry matters: an hourly wake-up costs
/// nothing, and the frame scheduler never adds a decades-long wait to `now`.
const ASK_OVERLAY_TICK_MAX: Duration = Duration::from_secs(60 * 60);

impl Session<'_> {
    /// A key press or a paste: the user is here, so a waiting question's wait
    /// starts over — wherever the key lands, an overlay over the modal
    /// included.
    pub(crate) fn note_user_activity(&mut self) {
        self.ask_timer.touch(Instant::now());
    }

    /// The draw tick's half, run before the paint. Past the wait, every
    /// waiting question resolves unanswered and its decision goes up on the
    /// gate — this very frame paints the modal closed. Before it, the open
    /// modal gets its countdown and a frame stays booked: a second away while
    /// the countdown is on screen, else no later than the expiry (capped)
    /// under an overlay — where the status chain stops re-arming, and an idle
    /// overlay is exactly where a user leaves a question waiting.
    pub(crate) fn tick_ask_clock(&mut self) {
        let clock = self.ask_timer.tick(
            self.app.has_pending_asks(),
            self.app.ask().map(|prompt| prompt.request.id.as_str()),
            self.app.settings().ask_timeout(),
            Instant::now(),
        );
        match clock {
            AskClock::Idle => self.app.set_ask_remaining(None),
            AskClock::Running(left) => {
                self.app.set_ask_remaining(Some(left));
                let next = if self.app.view.is_overlay() {
                    left.min(ASK_OVERLAY_TICK_MAX)
                } else {
                    left.min(ASK_TICK_INTERVAL)
                };
                self.frame.schedule_frame_in(next);
            }
            AskClock::Expired(after) => {
                for (id, decision) in self.app.expire_asks(after) {
                    self.ask.resolve(&id, decision);
                }
            }
        }
    }
}
