//! The `/trust` review menu: the project config layer's approval surface
//! (`docs/project-config.md`).
//!
//! The seventh composer-replacing inline picker, the [`super::hooks_menu`]'s
//! sibling — no text entry, the cursor parked in the corner, every key owned
//! while open. It renders the boundary-injected [`TrustReview`] (the
//! `open_hooks_menu` seam: the pure command returns the intent, the loop
//! digests the project layer it actually loaded) — the project root, every
//! hook command and MCP server target **verbatim**, and the option rows —
//! and answers Enter with [`Action::ApplyTrust`], which the loop turns into
//! the `trust.json` write and the live (de)activation.

use super::*;

use crate::trust::TrustReview;

/// The open `/trust` menu (`None` on [`App`] when closed): the review
/// snapshot and the highlighted option row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustMenu {
    /// The project layer digested for review — injected whole at open.
    pub review: TrustReview,
    /// The highlighted option row (an index into [`TrustReview::options`];
    /// meaningless when there are none).
    pub selected: usize,
}

impl App {
    /// Open the `/trust` menu over the boundary-supplied review. Abandons
    /// the bands that share the composer, like every picker.
    pub fn open_trust_menu(&mut self, review: TrustReview) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.trust_menu = Some(TrustMenu {
            review,
            selected: 0,
        });
    }

    /// Dismiss the menu (Esc, or Ctrl+C): the composer returns. No view
    /// change — it was never an overlay.
    pub fn close_trust_menu(&mut self) {
        self.trust_menu = None;
    }

    /// Keys while the `/trust` menu is open. Owns **every** key (routed at
    /// the top of [`on_key`]): ↑/↓ move over the option rows wrapping at the
    /// ends, Home/End jump, digits jump-apply, Enter applies the highlighted
    /// option (closing the menu — the loop records/activates), Esc and
    /// Ctrl+C close.
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_trust(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the menu (never quits — the picker family's rule).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_trust_menu();
            return Action::CloseTrustMenu;
        }
        let Some(menu) = self.trust_menu.as_mut() else {
            return Action::None;
        };
        let options = menu.review.options();
        let apply = |index: usize| options.get(index).copied();
        match key.code {
            KeyCode::Up => menu.selected = wrap_step(menu.selected, options.len(), -1),
            KeyCode::Down => menu.selected = wrap_step(menu.selected, options.len(), 1),
            KeyCode::Home => menu.selected = 0,
            KeyCode::End => menu.selected = options.len().saturating_sub(1),
            KeyCode::Enter => {
                if let Some(action) = apply(menu.selected) {
                    self.close_trust_menu();
                    return Action::ApplyTrust(action);
                }
            }
            // Digits jump-apply their absolute row (the ask modal's rule). A
            // digit past the rows names nothing and is ignored.
            KeyCode::Char(c @ '1'..='9')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(action) = apply((c as usize) - ('1' as usize)) {
                    self.close_trust_menu();
                    return Action::ApplyTrust(action);
                }
            }
            KeyCode::Esc => {
                self.close_trust_menu();
                return Action::CloseTrustMenu;
            }
            _ => {}
        }
        Action::None
    }
}
