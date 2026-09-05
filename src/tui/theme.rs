//! The `/theme` picker at the boundary: applying a selection
//! (`docs/theme.md`).
//!
//! The pure side ([`alter_zero::app::App::theme`]) already holds the new
//! theme when the picker's Enter lands here; what has to *happen* — the
//! `theme.json` write, the palette switch, the repaint in the new colours,
//! the confirming toast — lives at the boundary, the
//! `tui::mascot::Session::select_mascot` pattern.

use std::io;

use alter_zero::app::{Theme, ToastKind};
use alter_zero::ui;

use super::{Session, config};

impl Session<'_> {
    /// Enter/Space in the `/theme` picker: persist the choice, switch the
    /// palette the renderers read, and repaint everything in it.
    ///
    /// The colours of every committed row are baked into scrollback, so the
    /// one way to recolour the conversation is the standard purge rebuild
    /// (`repaint_active_view`), which re-renders the whole history — the
    /// banner, every cell, the in-flight partial — under the palette
    /// [`ui::activate_theme`] has just made ambient; the transcript and
    /// context caches key on that same theme and rebuild on their next read
    /// (`docs/theme.md`). The toast is raised first so the rebuilt frame
    /// already carries the confirmation, and the palette switches before
    /// the rebuild so the rebuild is what paints it.
    pub(crate) fn select_theme(&mut self, theme: Theme) -> io::Result<()> {
        config::save_theme(config::theme_json_path().as_deref(), theme);
        ui::activate_theme(theme);
        self.toast(format!("Theme: {}", theme.name()), ToastKind::Info);
        self.repaint_active_view()
    }
}
