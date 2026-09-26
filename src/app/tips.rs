//! The spinner tip's walk (`docs/tips.md`): which catalog tip a turn's status
//! line shows, when it draws one, and what keeps the row — and the walk —
//! still.
//!
//! The catalog and the slot arithmetic are the pure [`crate::tips`]; the row
//! itself is `ui::tips`. What lives here is the state between them: the
//! **cursor** the boundary seeds from `tips.json` and persists back, and the
//! draw [`App::set_status_times`] runs on every frame's clock.

use super::*;
use crate::tips::{tip, tip_slot};

/// The tip a turn's status line is showing (`docs/tips.md`): which catalog
/// entry, and the rotation slot it was drawn for. A later slot draws the next
/// tip; every frame of the same slot keeps this one — hidden or not — so a
/// checklist that comes and goes, or **Show tips** toggled off and on, brings
/// back the same tip rather than spending another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShownTip {
    /// Its index into [`crate::tips::TIPS`].
    pub index: usize,
    /// The slot it was drawn for ([`crate::tips::tip_slot`]).
    pub slot: u64,
}

impl App {
    /// Seed the tip walk at `next` — the catalog index the next tip shown
    /// opens on, as `tips.json` recorded it (called once at the I/O boundary,
    /// the [`set_session_info`](App::set_session_info) pattern). Until this
    /// runs no turn shows a tip at all: the unit-test default, so every test
    /// that holds a turn past the delay keeps the strip it always had.
    pub fn seed_tips(&mut self, next: usize) {
        self.tip_cursor = Some(next);
    }

    /// The walk's position — the index the next tip drawn will be, one past
    /// the last one shown — for the boundary to persist whenever it moves.
    /// `None` until [`seed_tips`](App::seed_tips).
    #[must_use]
    pub const fn tip_cursor(&self) -> Option<usize> {
        self.tip_cursor
    }

    /// The tip hanging off the status line right now, if any: the one the
    /// turn last drew, while nothing hides it. `None` idle, before the
    /// delay, and under every rule `tip_slot_open` names — checked here as
    /// well as at the draw, so a row hidden between two frames (Show tips
    /// turned off, a task created) goes at once.
    #[must_use]
    pub fn tip(&self) -> Option<&'static str> {
        let shown = self.status.as_ref()?.tip?;
        self.tip_slot_open().then(|| tip(shown.index))
    }

    /// Whether a tip may hang off the status line at all — every condition
    /// but the clock: a seeded walk, **Show tips** on, a status line of the
    /// main session's own to hang from (not a `!` shell turn, which has none,
    /// nor an agent session view, whose line is the subagent's), no task
    /// checklist, which takes the slot (`docs/task-tools.md`) — and the
    /// strip actually on screen: an inline modal replaces the whole live
    /// region and an overlay covers it, and a tip drawn under either would be
    /// spent without being seen.
    fn tip_slot_open(&self) -> bool {
        self.tip_cursor.is_some()
            && self.settings.tips
            && self.status.as_ref().is_some_and(|status| !status.shell)
            && self.viewed_agent().is_none()
            && self.tasks.is_empty()
            && !self.modal_open()
            && !self.view.is_overlay()
    }

    /// Draw the turn's tip for `elapsed`, run by
    /// [`set_status_times`](App::set_status_times) on every frame's clock:
    /// the first frame a slot is on screen takes the cursor's tip and moves
    /// the cursor on; every later frame of that slot keeps it. So the cursor
    /// moves only when a tip is actually seen — a turn that ends inside the
    /// delay, or spent under the checklist, leaves the next turn the tip it
    /// would have shown.
    pub(super) fn draw_tip(&mut self, elapsed: Duration) {
        let Some(slot) = tip_slot(elapsed) else {
            return;
        };
        if !self.tip_slot_open() {
            return;
        }
        let (Some(cursor), Some(status)) = (self.tip_cursor, self.status.as_mut()) else {
            return;
        };
        if status.tip.is_some_and(|shown| shown.slot == slot) {
            return;
        }
        let index = cursor % crate::tips::TIPS.len();
        status.tip = Some(ShownTip { index, slot });
        self.tip_cursor = Some((index + 1) % crate::tips::TIPS.len());
    }
}
