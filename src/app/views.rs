//! The full-screen overlays' [`App`](super::App) state: the Ctrl+O transcript
//! pager and the Ctrl+D context-debug view (open/close, scroll, key handling).
//!
//! What each shows is `docs/tool-view-performance.md` and `docs/context.md`; the
//! rule that nothing commits to scrollback while an overlay is up is CLAUDE.md
//! invariant 4.

use super::*;

/// How many lines PageUp/PageDown move the tool-output view.
pub(super) const TOOL_VIEW_PAGE: usize = 10;

impl App {
    /// Keys while the full-screen tool-output view is showing: it is a read-only
    /// scroller (codex's transcript pager), so typing is ignored; Esc and `q`
    /// (like Ctrl+O) return to the chat, Home/End jump to the transcript's edges.
    pub(super) fn on_key_tool_view(&mut self, key: KeyEvent) -> Action {
        match key.code {
            // Backtrack preview keys (docs/backtrack.md): Esc/← step the
            // highlight to the next-older user message, → back toward the
            // newest, Enter confirms the rewind (truncate + prefill — the
            // view flips back inside, so the ToggleToolView lands on the
            // loop's normal return-from-overlay path). The scroll keys below
            // keep working; q (or Ctrl+O) still closes, cancelling.
            KeyCode::Esc | KeyCode::Left if self.backtrack.selected.is_some() => {
                self.step_backtrack(-1);
                Action::None
            }
            KeyCode::Right if self.backtrack.selected.is_some() => {
                self.step_backtrack(1);
                Action::None
            }
            KeyCode::Enter if self.backtrack.selected.is_some() => {
                self.confirm_backtrack();
                // A dedicated action (not ToggleToolView) so the loop resets
                // the code to this point's checkpoint before the shared
                // return-from-overlay repaint (docs/checkpoint.md).
                Action::ConfirmBacktrack
            }
            // Esc in a plain Ctrl+O view *begins* the preview in place when
            // idle with a target — codex's Ctrl+T → Esc path; without one
            // (or mid-turn) it keeps closing the overlay below. The guard is
            // the shared [`App::overlay_esc_backtracks`], which also decides
            // the closing hint row's wording (`esc to edit prev` vs
            // `q/esc/ctrl+o to quit`), so the hint and this arm agree by
            // construction (docs/backtrack.md).
            KeyCode::Esc if self.overlay_esc_backtracks() => {
                self.begin_backtrack_preview();
                Action::None
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.toggle_tool_view(); // close the overlay, back to chat
                Action::ToggleToolView
            }
            KeyCode::Up => {
                self.tool_follow = false; // reading back — stop tailing the bottom
                self.tool_scroll = self.tool_scroll.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                // Don't touch `tool_follow`: reaching the bottom re-engages it in
                // `settle_tool_scroll`.
                self.tool_scroll = self.tool_scroll.saturating_add(1);
                Action::None
            }
            KeyCode::PageUp => {
                self.tool_follow = false;
                self.tool_scroll = self.tool_scroll.saturating_sub(TOOL_VIEW_PAGE);
                Action::None
            }
            KeyCode::PageDown => {
                self.tool_scroll = self.tool_scroll.saturating_add(TOOL_VIEW_PAGE);
                Action::None
            }
            KeyCode::Home => {
                self.tool_follow = false;
                self.tool_scroll = 0;
                Action::None
            }
            KeyCode::End => {
                // Past any end — `settle_tool_scroll` pins it to the bottom
                // and re-engages tail-follow (codex's jump_bottom).
                self.tool_scroll = usize::MAX;
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Flip between the conversation and the tool-output view. Entering the view
    /// pins it to the bottom (tail-follow) so it opens on the latest content.
    /// Ctrl+O is "activity" like any other key, so an open `?` shortcuts band
    /// closes rather than re-showing after the round-trip (docs/shortcuts.md).
    pub(super) fn toggle_tool_view(&mut self) {
        self.shortcuts_open = false;
        // Any toggle abandons an in-flight backtrack gesture — Ctrl+O/q close
        // a preview without truncating, and Ctrl+O over a primed composer is
        // "any other key" activity (codex resets its BacktrackState on
        // overlay close the same way; docs/backtrack.md).
        self.backtrack = Backtrack::default();
        self.view = match self.view {
            View::Conversation => View::ToolOutput,
            // The Ctrl+O guard in on_key keeps the picker and the Ctrl+D view
            // out of here; the arm is only exhaustiveness.
            View::ToolOutput | View::ResumePicker | View::ContextDebug => View::Conversation,
        };
        self.tool_scroll = 0;
        self.tool_follow = self.view == View::ToolOutput;
    }

    /// Keys while the Ctrl+D context-debug view is showing: the transcript
    /// pager's scroll set, with q/Esc (or Ctrl+D itself, handled globally)
    /// closing it. It has no backtrack — the view shows the derived context,
    /// not the editable conversation. See `docs/context.md`.
    pub(super) fn on_key_context_debug(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.toggle_context_debug();
                Action::ToggleContextDebug
            }
            KeyCode::Up => {
                self.debug_follow = false;
                self.debug_scroll = self.debug_scroll.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                self.debug_scroll = self.debug_scroll.saturating_add(1);
                Action::None
            }
            KeyCode::PageUp => {
                self.debug_follow = false;
                self.debug_scroll = self.debug_scroll.saturating_sub(TOOL_VIEW_PAGE);
                Action::None
            }
            KeyCode::PageDown => {
                self.debug_scroll = self.debug_scroll.saturating_add(TOOL_VIEW_PAGE);
                Action::None
            }
            KeyCode::Home => {
                self.debug_follow = false;
                self.debug_scroll = 0;
                Action::None
            }
            KeyCode::End => {
                // Past any end — `settle_debug_scroll` pins it to the bottom
                // and re-engages tail-follow, like the pager's End.
                self.debug_scroll = usize::MAX;
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Flip between the conversation and the Ctrl+D context-debug view —
    /// [`toggle_tool_view`](Self::toggle_tool_view)'s sibling, with the same
    /// activity rules (the `?` band closes, a backtrack gesture is abandoned)
    /// and the same open-at-the-bottom tail-follow.
    pub(super) fn toggle_context_debug(&mut self) {
        self.shortcuts_open = false;
        self.backtrack = Backtrack::default();
        self.view = match self.view {
            View::Conversation => View::ContextDebug,
            View::ContextDebug | View::ToolOutput | View::ResumePicker => View::Conversation,
        };
        self.debug_scroll = 0;
        self.debug_follow = self.view == View::ContextDebug;
    }

    /// Settle the tool-view scroll for a draw given the largest offset the current
    /// screen allows. While following, it stays pinned to the bottom (`max`);
    /// otherwise it's capped to `max`, and reaching the bottom re-engages
    /// following so new content keeps scrolling into view.
    pub fn settle_tool_scroll(&mut self, max: usize) {
        if self.tool_scroll >= max {
            self.tool_follow = true; // at (or past) the bottom — stick to it
        }
        self.tool_scroll = if self.tool_follow {
            max
        } else {
            self.tool_scroll.min(max)
        };
    }

    /// Settle the context-debug scroll for a draw —
    /// [`settle_tool_scroll`](Self::settle_tool_scroll) for the Ctrl+D view.
    pub fn settle_debug_scroll(&mut self, max: usize) {
        if self.debug_scroll >= max {
            self.debug_follow = true;
        }
        self.debug_scroll = if self.debug_follow {
            max
        } else {
            self.debug_scroll.min(max)
        };
    }
}
