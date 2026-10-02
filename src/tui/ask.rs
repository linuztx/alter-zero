//! The `AskUserQuestion` idle clock — the boundary half of the question
//! timeout (`docs/ask.md`).
//!
//! A question waits on the user, and the user may be away. While any question
//! is pending — the open modal, or one queued behind another modal — a clock
//! runs, and **every key press or paste starts it over**: a user who is
//! reading, choosing or typing never runs it out. When it does run out, every
//! waiting question resolves unanswered ([`App::expire_asks`]) and the
//! decisions go up on the gate, so the parked tool thread wakes and the model
//! is told the user is away and to keep working.
//!
//! The clock lives here because time does: the pure side sees only the
//! decisions and the remaining time injected per draw for the countdown
//! ([`App::set_ask_remaining`]), the toast deadline's pattern.
//!
//! [`App::expire_asks`]: alter_zero::app::App::expire_asks
//! [`App::set_ask_remaining`]: alter_zero::app::App::set_ask_remaining

use std::time::{Duration, Instant};

use super::Session;

/// How often the open modal's countdown moves: once a second, the rate its
/// `m:ss` changes at — the `/login` device page's cadence.
const ASK_TICK_INTERVAL: Duration = Duration::from_secs(1);

/// The furthest ahead a frame is booked under an overlay, where the countdown
/// is not on screen and only the deadline matters: an hourly wake-up costs
/// nothing, and the frame scheduler never adds a decades-long wait to `now`.
const ASK_OVERLAY_TICK_MAX: Duration = Duration::from_secs(60 * 60);

/// A running idle clock: when it runs out, and the wait it was armed with —
/// what the resolved cell and the model are told (`within 10m`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct AskClock {
    deadline: Instant,
    wait: Duration,
}

impl Session<'_> {
    /// Start the wait over: a question just arrived, or the **Ask timeout**
    /// row changed. With the row at `never`, or nothing waiting, no clock
    /// runs — nor for a wait so long its deadline is past what an `Instant`
    /// can hold (`ALTER_ZERO_ASK_TIMEOUT_SECS` takes any number), which is
    /// never in every sense that matters and must not panic the loop.
    pub(crate) fn restart_ask_clock(&mut self) {
        self.ask_clock = self
            .app
            .settings()
            .ask_timeout()
            .filter(|_| self.app.has_pending_asks())
            .and_then(|wait| {
                Some(AskClock {
                    deadline: Instant::now().checked_add(wait)?,
                    wait,
                })
            });
    }

    /// A key press or a paste: the user is here, so a waiting question's wait
    /// starts over — wherever the key lands, an overlay over the modal
    /// included.
    pub(crate) fn note_user_activity(&mut self) {
        if self.ask_clock.is_some() {
            self.restart_ask_clock();
        }
    }

    /// Keep the clock in step with the questions at the loop bottom: armed
    /// while one waits — so a question opened from the queue inherits the
    /// running wait — and dropped once none does (answered, declined,
    /// cleared).
    pub(crate) fn sync_ask_clock(&mut self) {
        if !self.app.has_pending_asks() {
            self.ask_clock = None;
        } else if self.ask_clock.is_none() {
            self.restart_ask_clock();
        }
    }

    /// The draw tick's half, run before the paint. Past the deadline every
    /// waiting question resolves unanswered and its decision goes up on the
    /// gate — this very frame paints the modal closed. Before it, the open
    /// modal gets its countdown and a frame stays pending: a second away
    /// while the countdown is on screen, else at the deadline itself (under
    /// an overlay nothing of it is visible, and an idle overlay is exactly
    /// where a user leaves a question waiting).
    pub(crate) fn tick_ask_clock(&mut self) {
        let Some(clock) = self.ask_clock else {
            self.app.set_ask_remaining(None);
            return;
        };
        let now = Instant::now();
        if now >= clock.deadline {
            self.ask_clock = None;
            for (id, decision) in self.app.expire_asks(clock.wait) {
                self.ask.resolve(&id, decision);
            }
            return;
        }
        let left = clock.deadline - now;
        self.app.set_ask_remaining(Some(left));
        let next = if self.app.view.is_overlay() {
            left.min(ASK_OVERLAY_TICK_MAX)
        } else {
            left.min(ASK_TICK_INTERVAL)
        };
        self.frame.schedule_frame_in(next);
    }
}
