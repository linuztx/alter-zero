//! The inline tool-permission prompt: the modal state a pending
//! [`PermissionRequest`] puts the composer into, and its key map.
//! See `docs/permissions.md`.

use super::*;

/// How many options every prompt offers (Yes / remember / No).
const OPTION_COUNT: usize = 3;

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

    /// Requests that arrived while another was open, oldest first — each opens
    /// as the one before it resolves (their tool threads simply stay blocked).
    #[must_use]
    pub fn pending_permissions(&self) -> &VecDeque<PermissionRequest> {
        &self.pending_permissions
    }

    /// Raise a permission prompt for `request` (the boundary's handler for
    /// [`crate::stream::StreamEvent::Permission`]). The composer's draft and
    /// `!` shell mode are stashed and the field cleared, so Tab's amend
    /// field starts empty and closing can hand the draft straight back. A request arriving while one is
    /// already open **queues** instead of replacing it.
    pub fn open_permission(&mut self, request: PermissionRequest) {
        if self.permission.is_some() {
            self.pending_permissions.push_back(request);
            return;
        }
        // The prompt takes the composer over, so the bands that hang off it go
        // (the `/model` picker's rule).
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
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

    /// Close the open prompt, restoring the stashed draft — then open the next
    /// queued request, if any (which re-stashes the same draft, so a run of
    /// requests costs the user nothing).
    fn close_permission(&mut self) {
        let Some(prompt) = self.permission.take() else {
            return;
        };
        self.input
            .set_text_with_cursor(&prompt.saved_input, prompt.saved_cursor);
        self.shell_mode = prompt.saved_shell_mode;
        if let Some(next) = self.pending_permissions.pop_front() {
            self.open_permission(next);
        }
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
            self.close_permission(); // …which opens the next queued one, if any
        }
        ids
    }

    /// Drain the ids of requests dropped without an answer, for the loop to
    /// release on the [`crate::permission::PermissionGate`]. Empty on every
    /// iteration that abandoned nothing.
    pub fn take_abandoned_permissions(&mut self) -> Vec<String> {
        std::mem::take(&mut self.abandoned_permissions)
    }

    /// Resolve the open prompt with `decision`: close it (restoring the draft)
    /// and hand the loop the request to post on the gate.
    fn resolve_permission(&mut self, decision: PermissionDecision) -> Action {
        let Some(prompt) = self.permission.as_ref() else {
            return Action::None;
        };
        let request = prompt.request.clone();
        self.close_permission();
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
    /// Options: ↑/↓ move (clamped), Enter takes the highlighted one, and
    /// `1`/`2`/`3` (plus `a` for the remember row — `shift+tab` already cycles
    /// the thinking mode) take one directly. Esc aborts the turn, Tab opens the
    /// amend field, and Ctrl+E asks a `bash` prompt for an explanation.
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
                    prompt.selected = prompt.selected.saturating_sub(1);
                }
                Action::None
            }
            KeyCode::Down => {
                if let Some(prompt) = self.permission.as_mut() {
                    prompt.selected = (prompt.selected + 1).min(OPTION_COUNT - 1);
                }
                Action::None
            }
            KeyCode::Enter => {
                let decision = self.selected_decision();
                self.resolve_permission(decision)
            }
            KeyCode::Char('1') => self.resolve_permission(PermissionDecision::Approve),
            KeyCode::Char('2' | 'a' | 'A') => {
                self.resolve_permission(PermissionDecision::ApproveAlways)
            }
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
    /// ordinary Esc-interrupt path (`docs/interrupt.md`).
    fn cancel_permission(&mut self) -> Action {
        self.discard_permissions();
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
    fn edit_amend(&mut self, key: KeyEvent) {
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
            KeyCode::Backspace => self.input.delete_backward(),
            KeyCode::Delete => self.input.delete_forward(),
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
