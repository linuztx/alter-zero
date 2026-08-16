//! The `/mascot` picker at the boundary: applying a selection
//! (`docs/mascot.md`).
//!
//! The pure side ([`alter_zero::app::App::mascot`]) already holds the new
//! mascot when the picker's Enter lands here; what has to *happen* — the
//! `mascot.json` write, the banner redraw, the confirming toast — lives at
//! the boundary, the `tui::settings::Session::apply_setting` pattern.

use std::io;

use alter_zero::app::{Mascot, ToastKind};

use super::{Session, config};

impl Session<'_> {
    /// Enter/Space in the `/mascot` picker: persist the choice and redraw
    /// the banner with it.
    ///
    /// The banner lives at the **top of scrollback** (chrome, outside
    /// `history` — `docs/header.md`), so the one way to redraw it is the
    /// standard purge rebuild, which re-emits it via `ui::banner_tail` with
    /// the mascot `App` now holds — the switch is visible at once, exactly
    /// like a resize's repaint. The toast is raised first so the rebuilt
    /// frame already carries the confirmation.
    pub(crate) fn select_mascot(&mut self, mascot: Mascot) -> io::Result<()> {
        config::save_mascot(config::mascot_json_path().as_deref(), mascot);
        self.toast(format!("Mascot: {}", mascot.name()), ToastKind::Info);
        self.repaint_active_view()
    }
}
