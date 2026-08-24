//! The Esc-Esc backtrack: prime → transcript preview → rewind + prefill.
//! See `docs/backtrack.md`.

use super::*;

/// The transient toast shown when a `/resume` restored the working directory to
/// the session's checkpoint (`docs/checkpoint.md`) — feedback that the code, not
/// just the transcript, was rewound.
pub const CHECKPOINT_RESTORED_NOTICE: &str = "Restored files to this session's checkpoint";

/// The transient toast shown when an Esc-Esc backtrack reset the working
/// directory to the rewound-to message's checkpoint (`docs/checkpoint.md`).
pub const CHECKPOINT_REWOUND_NOTICE: &str = "Reset files to the checkpoint for that message";

/// The Esc-Esc backtrack gesture's state — codex's `BacktrackState`
/// (`tui/src/app_backtrack.rs`): Esc from an idle, empty composer *primes* the
/// gesture, a second Esc previews previous user messages highlighted in the
/// transcript overlay, Esc/← / → step the highlight, and Enter rewinds the
/// conversation to the highlighted message and puts its text back in the
/// composer to edit. See `docs/backtrack.md`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Backtrack {
    /// The first Esc armed the gesture (codex's `primed` — no timeout); any
    /// non-Esc key disarms it. While armed the footer slot shows the
    /// `esc again to edit previous message` hint.
    pub primed: bool,
    /// `Some(i)` while the overlay preview is active: the highlighted user
    /// message, as an index into the conversation's user messages (oldest =
    /// 0 — codex's `nth_user_message`). `None` when not previewing.
    pub selected: Option<usize>,
    /// Set when the preview opens or the highlight moves; the next overlay
    /// draw scrolls the highlight into view and consumes it
    /// ([`App::take_backtrack_scroll`] — codex's `scroll_chunk_into_view`),
    /// so it never fights the user's own scrolling afterwards.
    pub scroll_pending: bool,
}

impl App {
    /// The history indices of the conversation's user messages ([`Role::User`]
    /// prompts, oldest first) — the backtrack gesture's target list (codex's
    /// `user_positions_iter`). A `!` shell header is user-*typed* but not a
    /// user *prompt*, so it is never a target (codex only walks user cells).
    fn user_message_positions(&self) -> Vec<usize> {
        self.history
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, HistoryItem::Message(m) if m.role == Role::User))
            .map(|(index, _)| index)
            .collect()
    }

    /// Is there a previous user message for Esc-Esc to edit? Gates priming
    /// (codex's `has_backtrack_target`) — with no target, idle Esc keeps its
    /// historical meaning here: quit. See `docs/backtrack.md`.
    #[must_use]
    pub fn has_backtrack_target(&self) -> bool {
        self.history
            .iter()
            .any(|item| matches!(item, HistoryItem::Message(m) if m.role == Role::User))
    }

    /// Would Esc in the Ctrl+O transcript overlay **begin** the backtrack
    /// preview instead of closing the overlay? True exactly when idle (no
    /// running turn), with no inline modal open, outside an agent session
    /// view, and with a previous user message to edit — codex's Ctrl+T → Esc
    /// path. The one predicate the overlay's Esc key arm
    /// (`on_key_tool_view`) and its closing hint row (`ui::render_tool_view` —
    /// `q/ctrl+o to quit   esc to edit prev` vs `q/esc/ctrl+o to quit`) share,
    /// so what the hint promises and what the key does can never drift. See
    /// `docs/backtrack.md`.
    ///
    /// [`modal_open`](App::modal_open) is what makes the overlays safe to open
    /// from *inside* a tool-permission prompt or an `AskUserQuestion` question
    /// (`App::on_key_overlay_toggle`). A **background agent** can raise either
    /// with no turn running, so "idle" alone would let Esc arm a rewind that
    /// truncates history and prefills the very composer the modal has stashed
    /// — while a tool thread is still blocked on the gate. Nothing is
    /// rewindable while something is blocked on the user.
    #[must_use]
    pub fn overlay_esc_backtracks(&self) -> bool {
        !self.turn_active()
            && !self.modal_open()
            && self.agent_view.is_none()
            && self.has_backtrack_target()
    }

    /// Start previewing with the newest user message highlighted, requesting
    /// a scroll to bring it into view. No-op with no target (the key arms
    /// guard, but a stale call must not underflow).
    pub(super) fn begin_backtrack_preview(&mut self) {
        let count = self.user_message_positions().len();
        if count == 0 {
            return;
        }
        self.backtrack.selected = Some(count - 1);
        self.backtrack.scroll_pending = true;
        self.tool_follow = false; // pin to the highlight, not the tail
    }

    /// The primed second Esc: open the transcript overlay as a backtrack
    /// preview (codex's `open_backtrack_preview`). The caller returns
    /// [`Action::ToggleToolView`] so the loop enters the overlay screen.
    pub(super) fn open_backtrack_preview(&mut self) {
        self.view = View::ToolOutput;
        self.tool_scroll = 0;
        self.begin_backtrack_preview();
    }

    /// Step the preview highlight (`-1` older, `+1` newer), clamped at both
    /// ends (codex saturates the same way). A move requests a scroll-into-view.
    pub(super) fn step_backtrack(&mut self, delta: isize) {
        let count = self.user_message_positions().len();
        if let Some(selected) = self.backtrack.selected
            && count > 0
        {
            let stepped = selected.saturating_add_signed(delta).min(count - 1);
            if stepped != selected {
                self.backtrack.selected = Some(stepped);
                self.backtrack.scroll_pending = true;
            }
        }
    }

    /// Confirm the preview (Enter): drop the highlighted user message and
    /// everything after it from the history, reset the gesture, return to the
    /// conversation view, and put the message's text back in the composer to
    /// edit — codex's rollback + `set_composer_text`, except there is no
    /// backend session to fork: one prompt per turn means truncating
    /// [`history`] *is* the whole rewind. The loop's return-from-overlay
    /// repaint rebuilds the inline view from the truncated history. The
    /// message's image attachments come back with it (the interrupt-undo
    /// dance): any pairs backing the clobbered draft are discarded, the
    /// restored placeholders are re-keyed to the message's recorded paths,
    /// and the *later* dropped user messages' attachments — now referenced by
    /// nothing — are queued for temp-file deletion. See `docs/backtrack.md`.
    ///
    /// [`history`]: App::history
    pub(super) fn confirm_backtrack(&mut self) {
        let positions = self.user_message_positions();
        let Some(&position) = self.backtrack.selected.and_then(|i| positions.get(i)) else {
            return;
        };
        let HistoryItem::Message(message) = &self.history[position] else {
            return;
        };
        let text = message.text.clone();
        let images = message.images.clone();
        // Attachments of user messages *after* the rewound one leave the
        // conversation entirely — orphaned temp files, queued for deletion.
        for item in &self.history[position + 1..] {
            if let HistoryItem::Message(m) = item
                && m.role == Role::User
            {
                self.discarded_images.extend(m.images.iter().cloned());
            }
        }
        self.history.truncate(position);
        self.history_generation += 1;
        // The checklist rewinds with the conversation: the last task record
        // before the cut holds the state it had there (docs/task-tools.md);
        // the boundary syncs the shared registry after the rewind.
        self.reset_tasks_from_history();
        // The rewound history's tail can be an older batch-sibling user
        // message — fence it off from the interrupt-undo like a resumed tail.
        self.undo_floor = self.history.len();
        // The gauge re-seats on the rewound conversation (docs/compact.md).
        self.refresh_context_used();
        self.backtrack = Backtrack::default();
        self.view = View::Conversation;
        self.recall_input(&text);
        // recall_input replaced the draft: discard any pairs that backed it,
        // then re-key the rewound message's attachments to the placeholders
        // now back in the composer (paths were recorded in occurrence order).
        let stale = std::mem::take(&mut self.images);
        self.discarded_images
            .extend(stale.into_iter().map(|(_, path)| path));
        self.images = crate::paste::image_placeholder_occurrences(&text)
            .into_iter()
            .zip(images)
            .collect();
    }

    /// Take the pending scroll-into-view request, if any. The overlay draw
    /// calls this once per frame and, when it returns true, applies
    /// `ui::backtrack_scroll`'s decision via [`apply_backtrack_scroll`] —
    /// consumed so it runs once per selection change, like codex's
    /// `scroll_chunk_into_view`, never fighting manual scrolling.
    ///
    /// [`apply_backtrack_scroll`]: App::apply_backtrack_scroll
    #[must_use]
    pub fn take_backtrack_scroll(&mut self) -> bool {
        std::mem::take(&mut self.backtrack.scroll_pending)
    }

    /// Seat the overlay scroll on the previewed highlight, releasing the
    /// tail-follow pin: previewing the last message parks the scroll at max,
    /// which re-engages following ([`settle_tool_scroll`]) — left set, it
    /// would yank the next step-older's scroll straight back to the bottom.
    ///
    /// [`settle_tool_scroll`]: App::settle_tool_scroll
    pub fn apply_backtrack_scroll(&mut self, scroll: usize) {
        self.tool_scroll = scroll;
        self.tool_follow = false;
    }
}
