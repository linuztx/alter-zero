//! The inline `AskUserQuestion` modal: the state a pending
//! [`AskRequest`] puts the composer into, and its key map. See `docs/ask.md`.
//!
//! The permission prompt's sibling ([`super::permission`]): while one is open
//! it **owns every key** (routed at the top of [`on_key`](App::on_key)) and
//! `ui::render_ask` replaces the whole live region with it — the turn is
//! blocked on the answers, so there is nothing to animate above it. The
//! composer draft is stashed on open and restored on close, and the ask's own
//! text entry (the free-text "Other" row, the notes field) reuses
//! [`App::input`] exactly like Tab's amend field does.

use std::collections::BTreeSet;

use super::*;

use crate::ask::{AskAnswer, AskDecision, AskQuestion, AskRequest};

/// What one row of a question page *is* — the shared vocabulary between the
/// key map (what Enter activates) and the renderer (what the row shows).
/// Ordered as displayed: the options, the auto-added free-text "Other" row,
/// a multi-select question's own confirm row, then `Chat about this`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskRow {
    /// The i-th [`crate::ask::AskOption`].
    Option(usize),
    /// The free-text "Type something." row every question gets.
    Other,
    /// A multi-select question's unnumbered `Submit` row — confirms this
    /// question's selection and advances.
    Confirm,
    /// `Chat about this` — resolves the whole call: the user wants to talk.
    Chat,
}

/// The rows of one question's page, in display order.
#[must_use]
pub fn ask_rows(question: &AskQuestion) -> Vec<AskRow> {
    let mut rows: Vec<AskRow> = (0..question.options.len()).map(AskRow::Option).collect();
    rows.push(AskRow::Other);
    if question.multi_select {
        rows.push(AskRow::Confirm);
    }
    rows.push(AskRow::Chat);
    rows
}

/// The 1-based digit that jump-activates a row, `None` for the unnumbered
/// multi-select `Submit` row. Options count 1…n, the Other row n+1, `Chat
/// about this` n+2 — the reference transcript's numbering.
#[must_use]
pub fn ask_row_number(question: &AskQuestion, row: AskRow) -> Option<usize> {
    let n = question.options.len();
    match row {
        AskRow::Option(i) => Some(i + 1),
        AskRow::Other => Some(n + 1),
        AskRow::Confirm => None,
        AskRow::Chat => Some(n + 2),
    }
}

/// One question's answer under construction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AskAnswerState {
    /// The chosen option indices — at most one entry for a single-select
    /// question, any number for a multi-select one.
    pub selected: BTreeSet<usize>,
    /// The free-text "Other" entry as last accepted (kept across re-edits so
    /// re-opening the field shows it).
    pub other: String,
    /// Whether the Other text is part of the answer (typed and accepted).
    pub other_chosen: bool,
    /// The notes typed via `n` (preview questions only) — kept verbatim; a
    /// whitespace-only note is dropped at submission.
    pub notes: String,
}

impl AskAnswerState {
    /// Is this question answered — any option picked, or an accepted Other?
    #[must_use]
    pub fn answered(&self) -> bool {
        !self.selected.is_empty() || self.other_chosen
    }
}

/// What [`App::input`] is editing while the modal is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskInput {
    /// Nothing — the option list owns the keys.
    Select,
    /// The free-text "Other" row's entry field.
    Other,
    /// The notes field (`n` on a preview question).
    Notes,
}

/// A pending `AskUserQuestion` call, shown where the composer was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskPrompt {
    /// What is being asked, as the backend built it.
    pub request: AskRequest,
    /// The current page: `0..questions.len()` is that question's tab, and —
    /// with more than one question — `questions.len()` is the Submit page.
    pub tab: usize,
    /// The highlighted row on the current page (an index into
    /// [`ask_rows`], or 0/1 on the Submit page: `Submit answers` / `Cancel`).
    pub row: usize,
    /// Per-question answers under construction (parallel to
    /// `request.questions`).
    pub answers: Vec<AskAnswerState>,
    /// Which field [`App::input`] is editing, if any.
    pub input_mode: AskInput,
    /// The composer draft this prompt displaced — restored verbatim when it
    /// closes, the permission prompt's stash.
    saved_input: String,
    saved_cursor: usize,
    saved_shell_mode: bool,
}

impl AskPrompt {
    /// Whether this call has its own Submit page — only with several
    /// questions; a lone question resolves straight from its page.
    #[must_use]
    pub fn has_submit_tab(&self) -> bool {
        self.request.questions.len() > 1
    }

    /// How many tabs the chip strip shows (the questions + the Submit page).
    #[must_use]
    pub fn tab_count(&self) -> usize {
        self.request.questions.len() + usize::from(self.has_submit_tab())
    }

    /// Is the current page the Submit page?
    #[must_use]
    pub fn on_submit_tab(&self) -> bool {
        self.has_submit_tab() && self.tab == self.request.questions.len()
    }

    /// The question the current page shows, `None` on the Submit page.
    #[must_use]
    pub fn current_question(&self) -> Option<&AskQuestion> {
        self.request.questions.get(self.tab)
    }

    /// Whether [`App::input`] is live (the Other entry or the notes field) —
    /// the hardware cursor shows again while it is.
    #[must_use]
    pub fn editing(&self) -> bool {
        self.input_mode != AskInput::Select
    }

    /// The current page's row count.
    fn row_count(&self) -> usize {
        match self.current_question() {
            Some(question) => ask_rows(question).len(),
            None => 2, // the Submit page: `Submit answers` / `Cancel`
        }
    }
}

impl App {
    /// The open ask modal, if an `AskUserQuestion` call is waiting on the user.
    #[must_use]
    pub const fn ask(&self) -> Option<&AskPrompt> {
        self.ask.as_ref()
    }

    /// Raise the question modal for `request` (the boundary's handler for
    /// [`crate::stream::StreamEvent::AskUser`]). The composer's draft and `!`
    /// shell mode are stashed and the field cleared — the Other/notes entries
    /// start empty and closing hands the draft straight back. A request
    /// arriving while any modal is open (this one or a permission prompt)
    /// **queues** instead of replacing it.
    pub fn open_ask(&mut self, request: AskRequest) {
        if self.permission.is_some() || self.ask.is_some() {
            self.pending_asks.push_back(request);
            return;
        }
        // The modal takes the composer over, so the bands that hang off it go
        // (the permission prompt's rule).
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
        let answers = vec![AskAnswerState::default(); request.questions.len()];
        self.ask = Some(AskPrompt {
            request,
            tab: 0,
            row: 0,
            answers,
            input_mode: AskInput::Select,
            saved_input,
            saved_cursor,
            saved_shell_mode,
        });
    }

    /// Close the open modal, restoring the stashed draft. The resolve paths
    /// follow with [`open_next_pending`](Self::open_next_pending) so a queued
    /// request opens next; the discard paths don't (everything queued is
    /// abandoned with it).
    fn close_ask(&mut self) {
        let Some(prompt) = self.ask.take() else {
            return;
        };
        self.input
            .set_text_with_cursor(&prompt.saved_input, prompt.saved_cursor);
        self.shell_mode = prompt.saved_shell_mode;
    }

    /// Open the next queued modal, if any — the shared tail of every resolve:
    /// the main turn's questions first (it is what the user is working with),
    /// then the permission requests other threads queued behind it.
    pub(super) fn open_next_pending(&mut self) {
        if self.permission.is_some() || self.ask.is_some() {
            return;
        }
        if let Some(request) = self.pending_asks.pop_front() {
            self.open_ask(request);
        } else if let Some(request) = self.pending_permissions.pop_front() {
            self.open_permission(request);
        }
    }

    /// Drop the open modal and every queued request (`/clear`, a permission
    /// prompt's Esc cancel), restoring the composer draft. The ids are
    /// recorded for the boundary to **release on the gate**
    /// ([`take_abandoned_asks`](Self::take_abandoned_asks)) as declines, so
    /// the blocked tool thread never parks forever.
    pub(super) fn discard_asks(&mut self) {
        let queued = std::mem::take(&mut self.pending_asks);
        self.abandoned_asks.extend(queued.into_iter().map(|r| r.id));
        if let Some(prompt) = self.ask.as_ref() {
            self.abandoned_asks.push(prompt.request.id.clone());
            self.close_ask();
        }
    }

    /// Drain the ids of requests dropped without an answer, for the loop to
    /// resolve on the [`crate::ask::AskGate`] as declines. Empty on every
    /// iteration that abandoned nothing.
    pub fn take_abandoned_asks(&mut self) -> Vec<String> {
        std::mem::take(&mut self.abandoned_asks)
    }

    /// Resolve the open modal with `decision`: close it (restoring the draft),
    /// open the next queued modal, and hand the loop the id to post on the
    /// gate.
    fn resolve_ask(&mut self, decision: AskDecision) -> Action {
        let Some(prompt) = self.ask.as_ref() else {
            return Action::None;
        };
        let id = prompt.request.id.clone();
        self.close_ask();
        self.open_next_pending();
        Action::ResolveAsk { id, decision }
    }

    /// Build the submission from the answered questions — `None` (and a jump
    /// to the first unanswered page) when nothing is answered yet: submitting
    /// nothing is meaningless, so the modal walks the user to the question
    /// instead.
    fn try_submit_ask(&mut self) -> Action {
        let Some(prompt) = self.ask.as_ref() else {
            return Action::None;
        };
        let answers: Vec<AskAnswer> = prompt
            .request
            .questions
            .iter()
            .zip(&prompt.answers)
            .filter(|(_, state)| state.answered())
            .map(|(question, state)| build_answer(question, state))
            .collect();
        if answers.is_empty() {
            // Nothing to submit — walk to the first unanswered question.
            if let Some(prompt) = self.ask.as_mut() {
                let first = prompt
                    .answers
                    .iter()
                    .position(|state| !state.answered())
                    .unwrap_or(0);
                prompt.tab = first;
                prompt.row = 0;
            }
            return Action::None;
        }
        self.resolve_ask(AskDecision::Submitted(answers))
    }

    /// Advance past the current question — the shared tail of a single-select
    /// pick and a multi-select confirm: with more pages, move to the next tab;
    /// a lone question submits right away (there is no Submit page to visit).
    fn advance_ask(&mut self) -> Action {
        let Some(prompt) = self.ask.as_mut() else {
            return Action::None;
        };
        if prompt.has_submit_tab() {
            prompt.tab = (prompt.tab + 1).min(prompt.tab_count() - 1);
            prompt.row = 0;
            Action::None
        } else {
            self.try_submit_ask()
        }
    }

    /// Activate the highlighted row (Enter, or a digit jump).
    fn activate_ask_row(&mut self, row: AskRow) -> Action {
        let Some(prompt) = self.ask.as_mut() else {
            return Action::None;
        };
        let tab = prompt.tab;
        let Some(question) = prompt.request.questions.get(tab) else {
            return Action::None;
        };
        let multi = question.multi_select;
        match row {
            AskRow::Option(i) => {
                let state = &mut prompt.answers[tab];
                if multi {
                    // Toggle — the checkbox flips, the page stays.
                    if !state.selected.remove(&i) {
                        state.selected.insert(i);
                    }
                    Action::None
                } else {
                    state.selected = BTreeSet::from([i]);
                    state.other_chosen = false;
                    self.advance_ask()
                }
            }
            AskRow::Other => {
                // Open the free-text entry, seeded with what was typed before.
                let draft = prompt.answers[tab].other.clone();
                prompt.input_mode = AskInput::Other;
                self.input.set_text_with_cursor(&draft, draft.len());
                Action::None
            }
            AskRow::Confirm => self.advance_ask(),
            AskRow::Chat => self.resolve_ask(AskDecision::Chat),
        }
    }

    /// Keys while the ask modal is open — it owns all of them.
    ///
    /// Pages: ←/→ (and Tab/Shift+Tab) move between the question tabs and the
    /// Submit page; ↑/↓ move rows; Enter activates the highlighted row;
    /// digits jump-activate the numbered ones; `n` opens the notes field on a
    /// preview question; Esc **declines** the whole call (the turn continues —
    /// the model is told the user declined). In the Other/notes entry the
    /// composer is live: Enter accepts, Esc backs out keeping the text.
    pub(super) fn on_key_ask(&mut self, key: KeyEvent) -> Action {
        if self.ask.as_ref().is_some_and(AskPrompt::editing) {
            return self.on_key_ask_input(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Ctrl+C is Esc here: the modal owns the key, so it neither clears a
        // draft (it is stashed) nor quits mid-question.
        if ctrl && key.code == KeyCode::Char('c') {
            return self.resolve_ask(AskDecision::Declined);
        }
        if ctrl {
            return Action::None;
        }
        let Some(prompt) = self.ask.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => {
                prompt.row = prompt.row.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                prompt.row = (prompt.row + 1).min(prompt.row_count() - 1);
                Action::None
            }
            KeyCode::Left | KeyCode::BackTab => {
                prompt.tab = prompt.tab.saturating_sub(1);
                prompt.row = 0;
                Action::None
            }
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                prompt.tab = prompt.tab.saturating_sub(1);
                prompt.row = 0;
                Action::None
            }
            KeyCode::Right => {
                prompt.tab = (prompt.tab + 1).min(prompt.tab_count() - 1);
                prompt.row = 0;
                Action::None
            }
            // Tab wraps past the last page back to the first, so it alone can
            // cycle the whole strip.
            KeyCode::Tab => {
                prompt.tab = (prompt.tab + 1) % prompt.tab_count();
                prompt.row = 0;
                Action::None
            }
            KeyCode::Enter => {
                if prompt.on_submit_tab() {
                    return match prompt.row {
                        0 => self.try_submit_ask(),
                        _ => self.resolve_ask(AskDecision::Declined),
                    };
                }
                let Some(question) = prompt.current_question() else {
                    return Action::None;
                };
                let rows = ask_rows(question);
                match rows.get(prompt.row) {
                    Some(row) => self.activate_ask_row(*row),
                    None => Action::None,
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let digit = (c as u8 - b'0') as usize;
                if prompt.on_submit_tab() {
                    return match digit {
                        1 => self.try_submit_ask(),
                        2 => self.resolve_ask(AskDecision::Declined),
                        _ => Action::None,
                    };
                }
                let Some(question) = prompt.current_question() else {
                    return Action::None;
                };
                let rows = ask_rows(question);
                match rows
                    .iter()
                    .find(|row| ask_row_number(question, **row) == Some(digit))
                {
                    Some(row) => {
                        let row = *row;
                        // Seat the highlight on the activated row first, so a
                        // toggle leaves the `❯` where the digit pointed.
                        if let Some(at) = rows.iter().position(|r| *r == row) {
                            prompt.row = at;
                        }
                        self.activate_ask_row(row)
                    }
                    None => Action::None,
                }
            }
            // `n` opens the notes field — preview questions only, where the
            // hint offers it (`docs/ask.md`).
            KeyCode::Char('n') => {
                let tab = prompt.tab;
                if prompt
                    .current_question()
                    .is_some_and(AskQuestion::has_previews)
                {
                    let draft = prompt.answers[tab].notes.clone();
                    prompt.input_mode = AskInput::Notes;
                    self.input.set_text_with_cursor(&draft, draft.len());
                }
                Action::None
            }
            KeyCode::Esc => self.resolve_ask(AskDecision::Declined),
            _ => Action::None,
        }
    }

    /// A bracketed paste while the modal is open (`docs/ask.md`): the live
    /// entry field (the Other row, the notes line) takes it exactly like the
    /// composer — an over-threshold paste collapses to its
    /// `[Pasted Content N chars]` placeholder, expanded back to the real text
    /// when the entry closes. On the option pages a paste is not typing, so
    /// it is swallowed rather than smuggled into the hidden composer.
    pub fn paste_into_ask(&mut self, pasted: &str) {
        if self.ask.as_ref().is_some_and(AskPrompt::editing) {
            self.insert_paste_at_cursor(pasted);
        }
    }

    /// Take the entry field's text on exit, splicing every large-paste
    /// placeholder back to its real content and **consuming** exactly the
    /// pairs that matched — the stashed composer draft's pairs must survive
    /// the modal for the draft's own eventual send (`docs/paste.md`).
    fn take_entry_text(&mut self) -> String {
        let text = self.input.take();
        crate::paste::expand_pastes_consuming(&text, &mut self.pasted)
    }

    /// Keys in the Other/notes entry: the composer edits normally
    /// (Shift+Enter / Ctrl+J insert a newline, a large paste collapses to its
    /// placeholder), Enter accepts, Esc backs out — both keep the typed text
    /// on the state, expanded, so re-opening the field shows it again.
    fn on_key_ask_input(&mut self, key: KeyEvent) -> Action {
        let Some(prompt) = self.ask.as_mut() else {
            return Action::None;
        };
        let tab = prompt.tab;
        let mode = prompt.input_mode;
        match key.code {
            KeyCode::Esc => {
                let text = self.take_entry_text();
                if let Some(prompt) = self.ask.as_mut() {
                    match mode {
                        // Esc keeps the draft (the reference behaviour: the
                        // typed note stays visible) without choosing it.
                        AskInput::Other => prompt.answers[tab].other = text,
                        AskInput::Notes => prompt.answers[tab].notes = text,
                        AskInput::Select => {}
                    }
                    prompt.input_mode = AskInput::Select;
                }
                Action::None
            }
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                let text = self.take_entry_text();
                let Some(prompt) = self.ask.as_mut() else {
                    return Action::None;
                };
                prompt.input_mode = AskInput::Select;
                match mode {
                    AskInput::Other => {
                        let multi = prompt.current_question().is_some_and(|q| q.multi_select);
                        let state = &mut prompt.answers[tab];
                        state.other = text;
                        state.other_chosen = !state.other.trim().is_empty();
                        if state.other_chosen && !multi {
                            // A single-select answer: the custom text replaces
                            // any picked option and the page advances.
                            state.selected.clear();
                            return self.advance_ask();
                        }
                        Action::None
                    }
                    AskInput::Notes => {
                        prompt.answers[tab].notes = text;
                        Action::None
                    }
                    AskInput::Select => Action::None,
                }
            }
            _ => {
                self.edit_amend(key);
                Action::None
            }
        }
    }
}

/// One question's finished [`AskAnswer`]: the chosen labels in option order
/// (the accepted Other text last), the trimmed notes, and — for a preview
/// question — the first chosen option's preview (the annotation the schema
/// describes).
fn build_answer(question: &AskQuestion, state: &AskAnswerState) -> AskAnswer {
    let mut labels: Vec<String> = state
        .selected
        .iter()
        .filter_map(|&i| question.options.get(i).map(|o| o.label.clone()))
        .collect();
    if state.other_chosen {
        labels.push(state.other.trim().to_string());
    }
    let preview = state
        .selected
        .iter()
        .find_map(|&i| question.options.get(i).and_then(|o| o.preview.clone()));
    AskAnswer {
        question: question.question.clone(),
        labels,
        notes: Some(state.notes.trim().to_string()).filter(|n| !n.is_empty()),
        preview,
    }
}
