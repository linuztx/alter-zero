//! The full-screen overlays' [`App`](super::App) state: the Ctrl+O transcript
//! pager and the Ctrl+D context-debug view (open/close, scroll, key handling),
//! plus the one shared key arm that opens them —
//! [`on_key_overlay_toggle`](super::App::on_key_overlay_toggle), which the
//! dispatch *and* the two inline modals call, so a prompt blocking the turn
//! still cannot lock the two views that show what it is asking about
//! (`docs/permissions.md`).
//!
//! What each shows is `docs/tool-view-performance.md` and `docs/context.md`; the
//! rule that nothing commits to scrollback while an overlay is up is CLAUDE.md
//! invariant 4.

use super::*;

/// How many lines PageUp/PageDown move the tool-output view.
pub(super) const TOOL_VIEW_PAGE: usize = 10;

/// Where a ←/→ step from a window whose top row is `top` lands, on a page
/// whose entries start on `rows` (ascending): ← on the last start above the
/// top — the entry being read, or the one before it when the window already
/// starts there — and the page's top with none; → on the first start below
/// it, and past the end with none, where the settle pins the scroll and
/// re-engages tail-follow. The one rule both pagers step by — Ctrl+D's
/// (`docs/context.md`) and Ctrl+O's (`docs/transcript-navigation.md`).
fn step_target(jump: EntryJump, rows: &[usize], top: usize) -> usize {
    match jump {
        EntryJump::Previous => rows
            .iter()
            .rev()
            .find(|&&row| row < top)
            .map_or(0, |&row| row),
        EntryJump::Next => rows
            .iter()
            .find(|&&row| row > top)
            .map_or(usize::MAX, |&row| row),
    }
}

/// The move a pager key makes — ←/→ and the scroll set, shared by the
/// Ctrl+O transcript and both Ctrl+D pages — or `None` for any other key.
fn pager_move(code: KeyCode) -> Option<PagerMove> {
    Some(match code {
        KeyCode::Left => PagerMove::Step(EntryJump::Previous),
        KeyCode::Right => PagerMove::Step(EntryJump::Next),
        KeyCode::Up => PagerMove::Back(1),
        KeyCode::Down => PagerMove::On(1),
        KeyCode::PageUp => PagerMove::Back(TOOL_VIEW_PAGE),
        KeyCode::PageDown => PagerMove::On(TOOL_VIEW_PAGE),
        KeyCode::Home => PagerMove::Top,
        KeyCode::End => PagerMove::Bottom,
        _ => return None,
    })
}

/// Whether a pager key's move waits for the draw rather than moving the
/// window at once: a step always does — where the neighbouring entry starts
/// is a fact about the page as built — and so does any move pressed behind
/// moves already waiting, so the keys a frame receives land in the order
/// they came (`docs/transcript-navigation.md`).
fn waits(mv: PagerMove, waiting: &[PagerMove]) -> bool {
    matches!(mv, PagerMove::Step(_)) || !waiting.is_empty()
}

/// Move a pager window — its scroll offset and tail-follow flag — by one
/// key's move, on a page whose entries start on `rows` (ascending) with
/// scroll ceiling `max`. Only a step reads those two; a scroll that need not
/// wait for the draw ([`waits`]) moves without them.
fn move_window(mv: PagerMove, scroll: &mut usize, follow: &mut bool, rows: &[usize], max: usize) {
    match mv {
        // The window's top row is the page's current end while it
        // tail-follows, so a page that grew since its last draw is stepped
        // from its new bottom. A step anywhere unpins the tail.
        PagerMove::Step(jump) => {
            let top = if *follow { max } else { (*scroll).min(max) };
            *scroll = step_target(jump, rows, top);
            *follow = false;
        }
        PagerMove::Back(by) => {
            *follow = false; // reading back — stop tailing the bottom
            *scroll = scroll.saturating_sub(by);
        }
        // `follow` untouched: reaching the bottom re-engages it in the settle.
        PagerMove::On(by) => *scroll = scroll.saturating_add(by),
        PagerMove::Top => {
            *follow = false;
            *scroll = 0;
        }
        // Past any end — the settle pins it to the bottom and re-engages
        // tail-follow (codex's jump_bottom).
        PagerMove::Bottom => *scroll = usize::MAX,
    }
}

/// Settle a pager window for a draw whose scroll ceiling is `max`: at (or
/// past) the bottom it sticks there, tail-following; otherwise it is capped
/// to `max`.
fn settle_window(scroll: &mut usize, follow: &mut bool, max: usize) {
    if *scroll >= max {
        *follow = true; // at (or past) the bottom — stick to it
    }
    *scroll = if *follow { max } else { (*scroll).min(max) };
}

/// Replay the moves `waiting` for the draw, oldest first, against a page
/// whose entries start on `rows` (ascending) with scroll ceiling `max`: each
/// lands where it would have had a draw come between them, since the window
/// is settled after every one.
fn replay_moves(
    waiting: Vec<PagerMove>,
    scroll: &mut usize,
    follow: &mut bool,
    rows: &[usize],
    max: usize,
) {
    for mv in waiting {
        move_window(mv, scroll, follow, rows, max);
        settle_window(scroll, follow, max);
    }
}

impl App {
    /// Keys while the full-screen tool-output view is showing: it is a read-only
    /// scroller (codex's transcript pager), so typing is ignored; Esc and `q`
    /// (like Ctrl+O) return to the chat, Home/End jump to the transcript's
    /// edges, and ←/→ step between its entries outside a backtrack preview
    /// (`docs/transcript-navigation.md`).
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
            // Outside a preview, ←/→ step between the transcript's entries —
            // Ctrl+D's step (`docs/transcript-navigation.md`) — and the rest
            // of the pager keys scroll. A step knows only the direction, so
            // it waits for the draw, which has the transcript as built
            // (`resolve_tool_moves`); a scroll pressed behind it waits too.
            code => {
                if let Some(mv) = pager_move(code) {
                    self.move_tool_view(mv);
                }
                Action::None
            }
        }
    }

    /// A Ctrl+O pager key's move: queued for the draw when it must wait
    /// ([`waits`]), else the window moved at once.
    fn move_tool_view(&mut self, mv: PagerMove) {
        if waits(mv, &self.tool_moves) {
            self.tool_moves.push(mv);
        } else {
            move_window(mv, &mut self.tool_scroll, &mut self.tool_follow, &[], 0);
        }
    }

    /// A Ctrl+D key's move of the page showing —
    /// [`move_tool_view`](Self::move_tool_view)'s twin.
    fn move_debug_page(&mut self, mv: PagerMove) {
        if waits(mv, &self.debug_moves) {
            self.debug_moves.push(mv);
        } else {
            let (scroll, follow) = self.debug_page_scroll();
            move_window(mv, scroll, follow, &[], 0);
        }
    }

    /// The two read-only full-screen views, as one shared key arm: Ctrl+O
    /// opens the transcript pager, Ctrl+D the raw-context view. `Some` when
    /// the key was one of them (the view is already flipped, and the returned
    /// [`Action`] tells the loop to sync the alternate screen), `None` when it
    /// was not.
    ///
    /// [`on_key`](Self::on_key) runs it for the conversation and the overlays;
    /// the **modals** — the tool-permission prompt and the `AskUserQuestion`
    /// question — run it themselves, ahead of swallowing the rest of their
    /// keys. Both are questions *about* the conversation, so locking the two
    /// views that show it is exactly backwards: reading the transcript, or the
    /// context the model was actually sent, is how you decide (see
    /// `docs/permissions.md`, `docs/ask.md`). Neither view can act on the
    /// conversation — they only scroll — so the blocked tool thread keeps
    /// waiting, undisturbed, and the modal is still there on the way back.
    ///
    /// Both keys work mid-stream, so the conversation keeps updating
    /// underneath them. Neither stacks on the other's view, nor on the
    /// `/resume` picker: the full-screen views share the alternate screen. In
    /// an agent session view they show the *viewed agent's* transcript and
    /// context (`docs/agent-tool.md`).
    pub(super) fn on_key_overlay_toggle(&mut self, key: KeyEvent) -> Option<Action> {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return None;
        }
        match key.code {
            KeyCode::Char('o') => {
                if matches!(self.view, View::ResumePicker | View::ContextDebug) {
                    return Some(Action::None);
                }
                self.toggle_tool_view();
                Some(Action::ToggleToolView)
            }
            KeyCode::Char('d') => {
                if matches!(self.view, View::ResumePicker | View::ToolOutput) {
                    return Some(Action::None);
                }
                self.toggle_context_debug();
                Some(Action::ToggleContextDebug)
            }
            _ => None,
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
            View::ToolOutput | View::ResumePicker | View::ContextDebug | View::DiffReview => {
                View::Conversation
            }
        };
        self.tool_scroll = 0;
        self.tool_follow = self.view == View::ToolOutput;
        // A move pressed before a close never lands on the next open.
        self.tool_moves.clear();
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
            // Tab flips between the view's two pages — the model's context
            // window and the classifier's task context (`docs/permissions.md`).
            // Reachable over an open permission prompt like the view itself:
            // the modal's key routing only runs in the conversation view, so
            // the prompt's Tab (its amend field) and this one never contend.
            KeyCode::Tab | KeyCode::BackTab => {
                self.debug_page = self.debug_page.flipped();
                // The moves waiting were pressed on the page just left behind.
                self.debug_moves.clear();
                Action::None
            }
            // ←/→ step between the page's entries (`docs/context.md`) and the
            // rest of the pager keys scroll it, the transcript pager's moves:
            // a step knows only the direction, so it waits for the draw,
            // which has the built page (`resolve_debug_moves`), and a scroll
            // pressed behind it waits too.
            code => {
                if let Some(mv) = pager_move(code) {
                    self.move_debug_page(mv);
                }
                Action::None
            }
        }
    }

    /// The scroll offset and tail-follow flag **of the page currently
    /// showing** — so one set of pager arms drives whichever is up, and each
    /// page keeps its own place when you flip to compare them and back.
    fn debug_page_scroll(&mut self) -> (&mut usize, &mut bool) {
        match self.debug_page {
            DebugPage::Context => (&mut self.debug_scroll, &mut self.debug_follow),
            DebugPage::Classifier => (&mut self.classifier_scroll, &mut self.classifier_follow),
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
            View::ContextDebug | View::ToolOutput | View::ResumePicker | View::DiffReview => {
                View::Conversation
            }
        };
        self.debug_scroll = 0;
        self.debug_follow = self.view == View::ContextDebug;
        // Both pages open pinned to the bottom; the page itself persists
        // across opens, so Ctrl+D comes back to whichever you were reading.
        self.classifier_scroll = 0;
        self.classifier_follow = self.view == View::ContextDebug;
        // A move pressed before a close never lands on the next open.
        self.debug_moves.clear();
    }

    /// Whether the draw tick should **re-arm** the next animation frame — the
    /// 32 ms clock chain that keeps the status line's shimmer sweeping and its
    /// timer advancing when no event arrives (codex's status-widget cadence).
    ///
    /// Everything it animates lives in the **inline** live region: the
    /// spinner, the elapsed counter, the pulsing tool bullet, a background
    /// shell's ticking runtime, the agent roster's counters and linger sweep
    /// — and the `/spinner` picker's live page, whose every row and preview
    /// turn at this cadence with no turn running (`docs/spinner.md`), and the
    /// `/plugins` page while a fetch or a job runs on it (its spinner and
    /// elapsed time) — never while it is still (`docs/plugins.md`).
    /// Under an alternate-screen overlay none of that is on screen, and the
    /// overlay's own content changes only when an event lands — every event
    /// source schedules its own frame, so nothing goes stale. Re-arming there
    /// repainted an unchanged full screen ~31 times a second, and a terminal
    /// drops the user's mouse selection the moment the cells under it are
    /// rewritten: that is what made text in Ctrl+O / Ctrl+D uncopyable until
    /// the turn finished. The chain re-seeds on the first draw after the
    /// return. See `docs/overlay-repaint.md`.
    #[must_use]
    pub fn wants_animation_frames(&self) -> bool {
        !self.view.is_overlay()
            && (self.turn_active()
                || self.background_view.is_some()
                || self.spinner_picker.is_some()
                || self.plugins_menu.as_ref().is_some_and(PluginsMenu::is_busy)
                || !self.agents().is_empty())
    }

    /// Settle the tool-view scroll for a draw given the largest offset the current
    /// screen allows. While following, it stays pinned to the bottom (`max`);
    /// otherwise it's capped to `max`, and reaching the bottom re-engages
    /// following so new content keeps scrolling into view.
    pub fn settle_tool_scroll(&mut self, max: usize) {
        settle_window(&mut self.tool_scroll, &mut self.tool_follow, max);
    }

    /// Resolve the Ctrl+O pager's waiting moves ([`tool_moves`](Self::tool_moves)),
    /// oldest first, against the rows the transcript's entries start on
    /// (`rows`, ascending) and its scroll ceiling (`max`) — the draw's call,
    /// made with the transcript as built and before [`settle_tool_scroll`]
    /// (`docs/transcript-navigation.md`). Each press is a move of its own,
    /// landing where it would have had a draw come between them. A step
    /// follows Ctrl+D's rule ([`resolve_debug_moves`](Self::resolve_debug_moves)):
    /// ← opens the start of the entry being read, or the one before it when
    /// the window already starts there; → the next one; past either end, the
    /// page's edge.
    ///
    /// [`settle_tool_scroll`]: Self::settle_tool_scroll
    pub fn resolve_tool_moves(&mut self, rows: &[usize], max: usize) {
        let waiting = std::mem::take(&mut self.tool_moves);
        replay_moves(
            waiting,
            &mut self.tool_scroll,
            &mut self.tool_follow,
            rows,
            max,
        );
    }

    /// Settle the context-debug scroll for a draw —
    /// [`settle_tool_scroll`](Self::settle_tool_scroll) for the Ctrl+D view.
    pub fn settle_debug_scroll(&mut self, max: usize) {
        settle_window(&mut self.debug_scroll, &mut self.debug_follow, max);
    }

    /// Resolve the waiting moves ([`debug_moves`](Self::debug_moves)) on the
    /// Ctrl+D page showing, oldest first, against the rows its entries start
    /// on (`rows`, ascending) and the page's scroll ceiling (`max`) — the
    /// draw's call, made with the page as built and before its
    /// `settle_*_scroll` (`docs/context.md`). A context window is a few dozen
    /// messages over thousands of rows; a step opens the previous or next
    /// one at the top.
    ///
    /// The window's top row is the scroll offset — `max` while the page
    /// tail-follows, since a page that grew since its last draw is still
    /// read from its bottom. ← lands on the last entry starting above that
    /// row (the start of the entry being read, or the one before it when
    /// the window already starts there), the top with none; → on the first
    /// starting below it, past the end with none — where the settle pins it
    /// and re-engages tail-follow, as it does for a header the last page
    /// shares. A step anywhere else unpins the tail. Each press is a move of
    /// its own, the window settled between them as a draw would have.
    pub fn resolve_debug_moves(&mut self, rows: &[usize], max: usize) {
        let waiting = std::mem::take(&mut self.debug_moves);
        let (scroll, follow) = self.debug_page_scroll();
        replay_moves(waiting, scroll, follow, rows, max);
    }

    /// Settle the **classifier page's** scroll for a draw, exactly like
    /// [`settle_debug_scroll`](Self::settle_debug_scroll) does the context
    /// page's (`docs/permissions.md`).
    pub fn settle_classifier_scroll(&mut self, max: usize) {
        settle_window(
            &mut self.classifier_scroll,
            &mut self.classifier_follow,
            max,
        );
    }
}
