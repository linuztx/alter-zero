//! The `/spinner` picker at the boundary: applying a selection
//! (`docs/spinner.md`).
//!
//! The pure side ([`alter_zero::app::App::spinner`]) already holds the new
//! style when the picker's Enter lands here; what has to *happen* — the
//! `spinner.json` write and the confirming toast — lives at the boundary,
//! the `tui::mascot::Session::select_mascot` pattern.

use alter_zero::app::{Spinner, ToastKind};

use super::{Session, config};

impl Session<'_> {
    /// Enter/Space in the `/spinner` picker: persist the choice and confirm
    /// it with a toast.
    ///
    /// Unlike a `/mascot` switch this needs **no purge rebuild**: the status
    /// line is live-region-only — its per-frame spinner and shimmer colours
    /// never reach scrollback (`docs/status-indicator.md`) — so nothing
    /// committed has to be redrawn. The next frame's status line, a running
    /// turn's or the next turn's, already wears the new style; `after_key`
    /// schedules that frame.
    pub(crate) fn select_spinner(&mut self, spinner: Spinner) {
        config::save_spinner(config::spinner_json_path().as_deref(), spinner);
        self.toast(format!("Spinner: {}", spinner.name()), ToastKind::Info);
    }
}
