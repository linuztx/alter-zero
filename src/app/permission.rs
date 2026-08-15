//! The inline tool-permission prompt: the modal state a pending
//! [`PermissionRequest`] puts the composer into, and its key map.
//! See `docs/permissions.md`.

use super::*;

use crate::permission::OPTION_COUNT;

/// A tool call waiting on the user, shown where the composer was.
///
/// While one is open it **owns every key** (routed at the top of
/// [`on_key`](App::on_key)) and [`crate::ui::render_live`] replaces the whole
/// live region with it — the turn is blocked on the answer, so there is
/// nothing to animate above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPrompt {
    /// What is being asked, as the backend built it.
    pub request: PermissionRequest,
    /// Which option row is highlighted (0-based, `< OPTION_COUNT`).
    pub selected: usize,
    /// Whether Tab's **amend** field is showing in place of the option rows:
    /// the composer is back (empty), and Enter rejects with what was typed.
    pub amend: bool,
    /// The composer draft this prompt displaced — restored verbatim when it
    /// closes, so a request landing mid-sentence costs nothing.
    saved_input: String,
    saved_cursor: usize,
    saved_shell_mode: bool,
}

impl App {
    /// The open prompt, if a tool is waiting on the user.
    #[must_use]
    pub const fn permission(&self) -> Option<&PermissionPrompt> {
        self.permission.as_ref()
    }

    /// The session's permission posture — `None` while permissions are
    /// disabled (no gate, nothing asks, no footer segment).
    #[must_use]
    pub const fn permission_mode(&self) -> Option<PermissionMode> {
        self.permission_mode
    }

    /// Inject or update the permission mode (the startup seed, a Ctrl+A
    /// toggle's echo, an option-2 "allow all edits" — the boundary keeps this
    /// field and the gate's rules in step).
    pub const fn set_permission_mode(&mut self, mode: Option<PermissionMode>) {
        self.permission_mode = mode;
    }

    /// Ctrl+A — step the mode cycle (manual → edit → auto → master) and hand
    /// the loop the new mode ([`Action::SetPermissionMode`]: mirror it onto
    /// the gate, persist it for this project, sweep newly covered requests,
    /// toast). With permissions disabled there is no mode to cycle — the
    /// toast says so (the `cycle_thinking` pattern).
    pub(super) fn toggle_permission_mode(&mut self) -> Action {
        match self.permission_mode {
            Some(mode) => {
                let next = mode.cycled();
                self.permission_mode = Some(next);
                Action::SetPermissionMode(next)
            }
            None => Action::Toast("Tool permissions are disabled".to_string()),
        }
    }

    /// Requests that arrived while another was open, oldest first — each opens
    /// as the one before it resolves (their tool threads simply stay blocked).
    #[must_use]
    pub fn pending_permissions(&self) -> &VecDeque<PermissionRequest> {
        &self.pending_permissions
    }

    /// Raise a permission prompt for `request` (the boundary's handler for
    /// [`crate::stream::StreamEvent::Permission`]). The composer's draft and
    /// `!` shell mode are stashed and the field cleared, so Tab's amend
    /// field starts empty and closing can hand the draft straight back. A
    /// request arriving while any modal is open — another permission prompt
    /// or the ask modal (`docs/ask.md`) — **queues** instead of replacing it.
    pub fn open_permission(&mut self, request: PermissionRequest) {
        if self.permission.is_some() || self.ask.is_some() {
            self.pending_permissions.push_back(request);
            return;
        }
        // The prompt takes the composer over, so the bands that hang off it go
        // (the `/model` picker's rule).
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.history_search = None;
        self.backtrack = Backtrack::default();
        let saved_input = self.input.text().to_string();
        let saved_cursor = self.input.cursor();
        let saved_shell_mode = self.shell_mode;
        self.input.clear();
        self.shell_mode = false;
        self.permission = Some(PermissionPrompt {
            request,
            selected: 0,
            amend: false,
            saved_input,
            saved_cursor,
            saved_shell_mode,
        });
    }

    /// Close the open prompt, restoring the stashed draft. The resolve paths
    /// follow with [`open_next_pending`](Self::open_next_pending) — which
    /// re-stashes the same draft for the next queued modal, so a run of
    /// requests costs the user nothing; the discard paths don't (everything
    /// queued is abandoned with the prompt).
    fn close_permission(&mut self) {
        let Some(prompt) = self.permission.take() else {
            return;
        };
        self.input
            .set_text_with_cursor(&prompt.saved_input, prompt.saved_cursor);
        self.shell_mode = prompt.saved_shell_mode;
    }

    /// Drop the open prompt and every queued one (Esc, `/clear`, a quit),
    /// restoring the composer draft. Their ids are recorded for the boundary to
    /// **release on the gate** ([`take_abandoned_permissions`]): a tool thread
    /// whose turn is being cancelled reaps itself on its cancel token, but a
    /// *background agent's* never would — nothing cancels it — so the release
    /// is what keeps an abandoned request from parking that thread forever.
    ///
    /// [`take_abandoned_permissions`]: App::take_abandoned_permissions
    pub(super) fn discard_permissions(&mut self) {
        let queued = std::mem::take(&mut self.pending_permissions);
        self.abandoned_permissions
            .extend(queued.into_iter().map(|r| r.id));
        if let Some(prompt) = self.permission.as_ref() {
            self.abandoned_permissions.push(prompt.request.id.clone());
            // Close only — the queue was just abandoned, and a pending ask is
            // its caller's business (`clear_conversation` discards it too; an
            // Esc cancel interrupts the whole turn it belongs to).
            self.close_permission();
        }
    }

    /// Release every open/queued request a **standing approval now covers**,
    /// returning their ids for the loop to approve on the gate.
    ///
    /// Parallel agents raise their requests before any of them is answered —
    /// each thread checked the rules before the first prompt was even shown —
    /// so option 2 has to reach the ones already waiting. Without this, three
    /// agents running the same command still ask three times *after* you said
    /// not to ask again. `covered` is the gate's own
    /// [`PermissionRules::allows`](crate::permission::PermissionRules::allows),
    /// passed in because the rules live at the boundary.
    ///
    /// The queue is swept first, so closing the open prompt lands on a request
    /// that genuinely still asks; the composer draft rides through untouched.
    pub fn drain_covered_permissions(
        &mut self,
        covered: &dyn Fn(&PermissionRequest) -> bool,
    ) -> Vec<String> {
        let mut ids = Vec::new();
        let queued = std::mem::take(&mut self.pending_permissions);
        for request in queued {
            if covered(&request) {
                ids.push(request.id);
            } else {
                self.pending_permissions.push_back(request);
            }
        }
        while let Some(prompt) = self.permission.as_ref() {
            if !covered(&prompt.request) {
                break;
            }
            ids.push(prompt.request.id.clone());
            self.close_permission();
            // Open the next queued modal. A pending *ask* ends the loop (the
            // permission slot is empty then) — every covered permission left
            // in the queue was already drained above, so nothing re-asks.
            self.open_next_pending();
        }
        ids
    }

    /// Drain the ids of requests dropped without an answer, for the loop to
    /// release on the [`crate::permission::PermissionGate`]. Empty on every
    /// iteration that abandoned nothing.
    pub fn take_abandoned_permissions(&mut self) -> Vec<String> {
        std::mem::take(&mut self.abandoned_permissions)
    }

    /// Resolve the open prompt with `decision`: close it (restoring the
    /// draft), open the next queued modal, and hand the loop the request to
    /// post on the gate.
    fn resolve_permission(&mut self, decision: PermissionDecision) -> Action {
        let Some(prompt) = self.permission.as_ref() else {
            return Action::None;
        };
        let request = prompt.request.clone();
        self.close_permission();
        self.open_next_pending();
        Action::ResolvePermission { request, decision }
    }

    /// The decision the currently highlighted option row stands for.
    fn selected_decision(&self) -> PermissionDecision {
        match self.permission.as_ref().map_or(0, |p| p.selected) {
            0 => PermissionDecision::Approve,
            1 => PermissionDecision::ApproveAlways,
            _ => PermissionDecision::Deny(None),
        }
    }

    /// Keys while a permission prompt is open — it owns all of them.
    ///
    /// Options: ↑/↓ move (wrapping at the ends), Enter takes the highlighted
    /// one, and `1`/`2`/`3` take one directly. **Ctrl+A** — the permission-mode toggle
    /// — selects the remember row on a `write`/`edit` prompt (choosing it *is*
    /// the switch to edit mode), and on a `bash` prompt just flips the mode,
    /// the prompt staying open (the mode never covers commands). Esc aborts
    /// the turn, Tab opens the amend field, and Ctrl+E asks a `bash` prompt
    /// for an explanation.
    ///
    /// In the amend field the composer is live: every editing key goes to it,
    /// Enter rejects with the typed feedback, and Esc backs out to the options.
    pub(super) fn on_key_permission(&mut self, key: KeyEvent) -> Action {
        if self.permission.as_ref().is_some_and(|p| p.amend) {
            return self.on_key_permission_amend(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Ctrl+E — only a command prompt offers it (its hint row says so).
        if ctrl
            && key.code == KeyCode::Char('e')
            && self
                .permission
                .as_ref()
                .is_some_and(|p| p.request.kind == PermissionKind::Bash)
        {
            return self.resolve_permission(PermissionDecision::Explain);
        }
        // Ctrl+A — the mode toggle, reachable inside the prompt too. On a
        // file prompt it IS option 2 (allow all edits = edit mode); on a
        // command or MCP prompt it only flips the posture — the call still
        // asks (their option 2 is a named rule, not the mode), and the
        // loop's sweep releases any queued file requests the new mode covers.
        if ctrl && key.code == KeyCode::Char('a') {
            if self.permission.as_ref().is_some_and(|p| {
                matches!(p.request.kind, PermissionKind::Write | PermissionKind::Edit)
            }) {
                return self.resolve_permission(PermissionDecision::ApproveAlways);
            }
            return self.toggle_permission_mode();
        }
        // Ctrl+C is Esc here: the prompt owns the key, so it neither clears a
        // draft (there is none — it is stashed) nor quits mid-decision.
        if ctrl && key.code == KeyCode::Char('c') {
            return self.cancel_permission();
        }
        if ctrl {
            return Action::None;
        }
        match key.code {
            KeyCode::Up => {
                if let Some(prompt) = self.permission.as_mut() {
                    prompt.selected = wrap_step(prompt.selected, OPTION_COUNT, -1);
                }
                Action::None
            }
            KeyCode::Down => {
                if let Some(prompt) = self.permission.as_mut() {
                    prompt.selected = wrap_step(prompt.selected, OPTION_COUNT, 1);
                }
                Action::None
            }
            KeyCode::Enter => {
                let decision = self.selected_decision();
                self.resolve_permission(decision)
            }
            KeyCode::Char('1') => self.resolve_permission(PermissionDecision::Approve),
            KeyCode::Char('2') => self.resolve_permission(PermissionDecision::ApproveAlways),
            KeyCode::Char('3') => self.resolve_permission(PermissionDecision::Deny(None)),
            KeyCode::Tab => {
                if let Some(prompt) = self.permission.as_mut() {
                    prompt.amend = true;
                }
                Action::None
            }
            KeyCode::Esc => self.cancel_permission(),
            _ => Action::None,
        }
    }

    /// Esc / Ctrl+C on the prompt — **cancel**, not just "no": the call is
    /// abandoned (the loop releases it on the gate) *and* the turn stops, the
    /// ordinary Esc-interrupt path (`docs/interrupt.md`). A queued ask
    /// belongs to the turn being torn down, so it is abandoned with the
    /// prompts (`docs/ask.md`).
    fn cancel_permission(&mut self) -> Action {
        self.discard_permissions();
        self.discard_asks();
        Action::Interrupt
    }

    /// Keys in Tab's amend field: the composer edits normally, Enter rejects
    /// with the feedback, Esc drops it and returns to the options.
    fn on_key_permission_amend(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.input.clear();
                if let Some(prompt) = self.permission.as_mut() {
                    prompt.amend = false;
                }
                Action::None
            }
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                let feedback = self.input.text().trim().to_string();
                // Read, don't `take_input`: the stashed draft's paste
                // placeholders are still live and must not be spliced here.
                self.input.clear();
                let feedback = (!feedback.is_empty()).then_some(feedback);
                self.resolve_permission(PermissionDecision::Deny(feedback))
            }
            _ => {
                self.edit_amend(key);
                Action::None
            }
        }
    }

    /// The amend field's editing keys — the composer's set minus everything
    /// that only makes sense for a *message* (the palette, the `@` picker, the
    /// ↑/↓ history recall, shell mode): free-text feedback, nothing more.
    /// Shared with the ask modal's Other/notes entries (`docs/ask.md`).
    pub(super) fn edit_amend(&mut self, key: KeyEvent) {
        let ctrl_or_alt = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            // Shift+Enter / Ctrl+J insert a newline, as in the composer
            // (docs/shift-enter.md).
            KeyCode::Enter => self.input.insert_newline(),
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.insert_newline();
            }
            KeyCode::Char(c) if !ctrl_or_alt => self.input.insert_char(c),
            // A Backspace/Delete on a `[Pasted Content N chars]` placeholder
            // removes it whole, like the composer (`docs/paste.md`) — the ask
            // modal's entry fields take pastes, and one keystroke must not
            // leave a mangled placeholder backed by a live pair.
            KeyCode::Backspace => {
                if !self.delete_placeholder(/*backward*/ true) {
                    self.input.delete_backward();
                }
            }
            KeyCode::Delete => {
                if !self.delete_placeholder(/*backward*/ false) {
                    self.input.delete_forward();
                }
            }
            KeyCode::Left => self.input.move_left(),
            KeyCode::Right => self.input.move_right(),
            KeyCode::Up => self.input.move_up(),
            KeyCode::Down => self.input.move_down(),
            KeyCode::Home => self.input.move_home(),
            KeyCode::End => self.input.move_end(),
            _ => {}
        }
    }
}
