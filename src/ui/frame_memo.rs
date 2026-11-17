//! One frame's build of the strip's live cells, shared by everything in the
//! frame that sizes them and the one thing that paints them.
//!
//! A frame asks how tall the strip's preview is many times before it draws
//! it once: the flow check, the modal-rebuild check, the region's height, the
//! layout, the strip's content rows and the cursor seat each derive it, and
//! each derivation **built the cells** to count them — a running command's
//! tail, a `write` streaming into its cell, a batch's queue. Eight builds a
//! frame, measured (`docs/input-latency.md`): a `write`'s preview cost
//! 5–15 ms a frame and a command with 60 KB of output held every keystroke
//! 13–46 ms, the loop at half a core, to paint cells that had not changed
//! between the builds.
//!
//! [`frame`] opens a scope in which the first build is kept and the rest are
//! answered from it. The scope is the whole of the soundness argument: the
//! boundary opens one around a draw, and a draw does not change what the
//! cells are built from. Outside a scope nothing is kept — every unit test,
//! and every reading the loop takes between draws, right after a state
//! change, builds afresh — so there is no key to keep in step with the
//! state and no entry to invalidate. A debug build checks the argument
//! rather than trusting it: each answer from the memo is compared with a
//! fresh build.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ratatui::text::Line;

/// The rows one build made, shared rather than copied: most readers want
/// their count.
pub(super) type Cells = Option<Rc<Vec<Line<'static>>>>;

thread_local! {
    /// Whether a frame is open on this thread, and what it has built: the
    /// live cells at each width asked for (one, in practice).
    static FRAME: RefCell<Option<Vec<(u16, Cells)>>> = const { RefCell::new(None) };
    /// How many times the live cells have been **built** on this thread —
    /// the memo's own test hook: what it exists for is a cost, and a cost is
    /// only pinned by counting it (`ui::footer`'s `queued_builds`).
    static BUILDS: Cell<u64> = const { Cell::new(0) };
}

/// How many times the strip's live cells have been built on this thread.
#[cfg(test)]
pub(super) fn live_cell_builds() -> u64 {
    BUILDS.with(Cell::get)
}

/// Run `draw` as **one frame**: every reading of the strip's live cells
/// inside it shares one build. Call it around a draw, never around anything
/// that changes the conversation — see the module docs. A frame opened
/// inside another is part of it.
pub fn frame<T>(draw: impl FnOnce() -> T) -> T {
    let opened = FRAME.with(|frame| {
        let mut frame = frame.borrow_mut();
        if frame.is_some() {
            false
        } else {
            *frame = Some(Vec::new());
            true
        }
    });
    // Closed on the way out — a panic's way included: a frame left open
    // would answer every later reading from this one's state.
    struct Close(bool);
    impl Drop for Close {
        fn drop(&mut self) {
            if self.0 {
                FRAME.with(|frame| *frame.borrow_mut() = None);
            }
        }
    }
    let _close = Close(opened);
    draw()
}

/// The strip's live cells at `width`: built by `build` — and, in a frame,
/// kept, so the frame's next reading at that width is answered from the
/// first build's rows.
pub(super) fn live_cells(width: u16, build: impl FnOnce() -> Option<Vec<Line<'static>>>) -> Cells {
    // `None`: no frame is open. `Some(None)`: one is, with nothing built at
    // this width yet. Read out before `build` runs, so no borrow is held
    // across it.
    let kept = FRAME.with(|frame| {
        frame.borrow().as_ref().map(|built| {
            built
                .iter()
                .find(|(at, _)| *at == width)
                .map(|(_, cells)| cells.clone())
        })
    });
    match kept {
        Some(Some(cells)) => {
            // The argument the memo rests on — a draw changes nothing the
            // cells are built from — checked wherever checks are on.
            debug_assert!(
                cells.as_deref() == build().as_ref(),
                "the strip's live cells changed inside a frame"
            );
            cells
        }
        opened => {
            BUILDS.with(|builds| builds.set(builds.get().saturating_add(1)));
            let cells = build().map(Rc::new);
            if opened.is_some() {
                FRAME.with(|frame| {
                    if let Some(built) = frame.borrow_mut().as_mut() {
                        built.push((width, cells.clone()));
                    }
                });
            }
            cells
        }
    }
}
