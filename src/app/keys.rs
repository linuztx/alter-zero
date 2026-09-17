//! Key dispatch: [`App::on_key`](super::App::on_key) routes by
//! [`View`](super::View), and the conversation view's handler owns the
//! composer's key map.
//!
//! The keys themselves are documented with their features: editing and cursor
//! motion in `docs/textarea.md`, the newline keys in `docs/shift-enter.md`, Esc
//! in `docs/interrupt.md` and `docs/backtrack.md`, and the `?` band in
//! `docs/shortcuts.md`.

use super::*;

impl App {
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if self.view == View::DiffReview {
            return self.on_key_diff_review(key);
        }
        // A pending `AskUserQuestion` modal owns every key, ahead of even the
        // permission prompt (the two queue behind each other, so at most one
        // is ever open) — the main turn's thread is blocked on the answers.
        // See `docs/ask.md`.
        if self.view == View::Conversation && self.ask.is_some() {
            return self.on_key_ask(key);
        }
        // A pending tool-permission request is **modal**: it owns every key,
        // ahead of even the Ctrl+R search and the inline pickers, because a
        // tool thread is blocked on the answer. See `docs/permissions.md`.
        if self.view == View::Conversation && self.permission.is_some() {
            return self.on_key_permission(key);
        }
        // An open Ctrl+R search owns *every* key (codex consumes them all in
        // handle_history_search_key) — including the global Ctrl+C/Ctrl+O
        // below, which it redefines: Ctrl+C cancels the search instead of
        // clearing the draft or quitting, and Ctrl+O cancels before the
        // overlay opens so no search state leaks into it.
        if self.view == View::Conversation && self.history_search.is_some() {
            return self.on_key_search(key);
        }
        // The inline `/model` picker likewise owns every key while it's open —
        // including the global Ctrl+C/Ctrl+O below, which it redefines (Ctrl+C
        // closes the picker instead of clearing the draft or quitting). It only
        // opens from the conversation view and blocks a turn from starting, so
        // this is the whole of its key handling. See `docs/llm.md`.
        if self.view == View::Conversation && self.model_picker.is_some() {
            return self.on_key_model_picker(key);
        }
        // The inline `/login` onboarding flow owns every key while open too, the
        // same way the `/model` picker does. See `docs/llm.md`.
        if self.view == View::Conversation && self.key_onboarding.is_some() {
            return self.on_key_key_onboarding(key);
        }
        // …and so does the inline `/settings` menu. See `docs/settings.md`.
        if self.view == View::Conversation && self.settings_picker.is_some() {
            return self.on_key_settings(key);
        }
        // …and the inline `/mascot` picker, its twin. See `docs/mascot.md`.
        if self.view == View::Conversation && self.mascot_picker.is_some() {
            return self.on_key_mascot_picker(key);
        }
        // …and the inline `/spinner` picker, the `/mascot` picker's twin. See
        // `docs/spinner.md`.
        if self.view == View::Conversation && self.spinner_picker.is_some() {
            return self.on_key_spinner_picker(key);
        }
        // …and the inline `/theme` picker, the `/spinner` picker's twin. See
        // `docs/theme.md`.
        if self.view == View::Conversation && self.theme_picker.is_some() {
            return self.on_key_theme_picker(key);
        }
        // …and the read-only `/donate` page, the `/hooks` menu's sibling.
        // See `docs/donate.md`.
        if self.view == View::Conversation && self.donate_picker.is_some() {
            return self.on_key_donate_picker(key);
        }
        // …and the inline `/skills` menu. See `docs/skills.md`.
        if self.view == View::Conversation && self.mcp_menu.is_some() {
            return self.on_key_mcp(key);
        }
        if self.view == View::Conversation && self.skills_menu.is_some() {
            return self.on_key_skills(key);
        }
        // …and the read-only `/hooks` menu. See `docs/hooks-menu.md`.
        if self.view == View::Conversation && self.hooks_menu.is_some() {
            return self.on_key_hooks(key);
        }
        // …and the `/trust` review menu. See `docs/project-config.md`.
        if self.view == View::Conversation && self.trust_menu.is_some() {
            return self.on_key_trust(key);
        }
        // The ↓ background manager band owns every key while open, the same
        // way the pickers do. See `docs/background.md`.
        if self.view == View::Conversation && self.background_view.is_some() {
            return self.on_key_background(key);
        }
        // A lit footer shell indicator (↓ pressed, the band not open yet)
        // claims a handful of keys first — Enter opens the band, Esc/↑/Ctrl+C
        // dismiss the highlight — and lets every other key through after
        // clearing it. It sits above the Ctrl+C global so a lit indicator
        // absorbs that press instead of quitting. See `docs/background.md`.
        if self.view == View::Conversation
            && self.background_focus
            && let Some(action) = self.on_key_background_focus(key)
        {
            return action;
        }
        // The ↓ roster selection (the `❯` on the footer's agent list) claims
        // its navigation keys the same way — ↑/↓/Enter/x/Esc — and lets every
        // other key through after clearing itself. See `docs/agent-tool.md`.
        if self.view == View::Conversation
            && self.agent_selection.is_some()
            && let Some(action) = self.on_key_agent_selection(key)
        {
            return action;
        }
        // Ctrl+C: in the conversation, a first press with text in the input
        // clears the draft instead of quitting (codex's composer-clear step —
        // see docs/design.md); otherwise it quits, from either screen. The
        // overlay never shows the input box, so there is nothing to clear there.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            // The /resume picker closes on Ctrl+C — codex's from-a-session
            // picker "exit" leaves the picker, never the app (only its
            // startup picker quits). See docs/resume.md.
            if self.view == View::ResumePicker {
                self.close_resume_picker();
                return Action::CloseResumePicker;
            }
            if self.view == View::Conversation && !self.input.is_empty() {
                // Record the cleared draft so ↑ can bring it back (codex's
                // clear_for_ctrl_c does the same). A shell-mode draft re-gains
                // its `!` so the recall re-enters the mode. Recorded
                // *ephemerally*: a cleared, abandoned draft recalls this session
                // but is not persisted (codex keeps cleared drafts in
                // local_history only — docs/history-persistence.md).
                let mut text = self.take_input();
                if self.shell_mode {
                    self.shell_mode = false;
                    text = format!("!{text}");
                }
                self.input_history.record_ephemeral(&text);
                self.command_menu = None; // an emptied input can't be a /token
                self.file_search = None; // …nor an @token, so close the picker too
                self.skill_picker = None; // …nor a $mention
                return Action::None;
            }
            return Action::Quit;
        }
        // Ctrl+O / Ctrl+D toggle the two full-screen views — from either
        // screen, even mid-stream, so the conversation keeps updating
        // underneath them. The two modals route *above* this and reach the
        // same arm from inside themselves, which is why these are the only
        // keys that get through a blocked turn
        // ([`on_key_overlay_toggle`](Self::on_key_overlay_toggle)).
        if let Some(action) = self.on_key_overlay_toggle(key) {
            return action;
        }
        match self.view {
            View::Conversation => self.on_key_conversation(key),
            View::ToolOutput => self.on_key_tool_view(key),
            View::ResumePicker => self.on_key_resume_picker(key),
            View::ContextDebug => self.on_key_context_debug(key),
            View::DiffReview => self.on_key_diff_review(key),
        }
    }

    /// Keys while the inline conversation is showing.
    ///
    /// When the slash-command palette is open it intercepts the navigation/select
    /// keys (↑/↓ move, Tab/Enter run, Esc dismisses); typing still edits the input
    /// (which filters the palette). With no palette open every key behaves as it
    /// always has.
    fn on_key_conversation(&mut self, key: KeyEvent) -> Action {
        // Any non-Esc key disarms a primed backtrack — codex resets its
        // priming on any other keypress, no timeout (docs/backtrack.md). The
        // key still does its normal job below.
        if self.backtrack.primed && key.code != KeyCode::Esc {
            self.backtrack.primed = false;
        }
        // The `?` shortcuts band (docs/shortcuts.md): `?` from an *empty*
        // composer toggles it (SHIFT allowed — terminals differ in reporting
        // Shift+/; with a draft `?` falls through and types). Any other key
        // closes an open band first and then acts normally (codex's
        // reset-after-activity) — except Esc, which only dismisses, since our
        // idle Esc would otherwise quit (the palette's Esc rule).
        let shortcuts_toggle = key.code == KeyCode::Char('?')
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && self.input.is_empty()
            // In shell mode `?` is a shell character (e.g. a glob), not the band.
            && !self.shell_mode;
        if shortcuts_toggle {
            self.shortcuts_open = !self.shortcuts_open;
            return Action::None;
        }
        if self.shortcuts_open {
            self.shortcuts_open = false;
            if key.code == KeyCode::Esc {
                return Action::None;
            }
        }
        let menu_open = self.command_menu.is_some();
        // The `@` file picker (docs/file-search.md): when its band is open it
        // intercepts the same navigation/select keys as the palette (they are
        // mutually exclusive — a bare `/token` has no whitespace, so an `@` in
        // it isn't at a token boundary).
        let file_open = self.file_search.is_some();
        // The `$` skill picker (docs/skill-mentions.md): the same keys again,
        // gated on the band actually *showing* (picker open and the cursor
        // still in a usable mention — the rows derive from the live token, so
        // a bare cursor move out of it must release ↑/↓ too). Exclusive with
        // both bands above by construction: a token starts with exactly one
        // sigil.
        let skill_open = self.skill_band_active();
        match key.code {
            // Esc dismisses the palette when it's open (codex's "popup wins"
            // rule — even mid-turn); else it interrupts an in-flight turn
            // (codex-style, see docs/interrupt.md); else it quits as before.
            KeyCode::Esc if menu_open => {
                self.command_menu = None;
                Action::None
            }
            // Esc likewise dismisses the file picker (sticky within the token —
            // see refresh_file_search), before the interrupt/quit fallbacks.
            KeyCode::Esc if file_open => {
                self.file_search = None;
                Action::None
            }
            // …and the skill picker (sticky within the mention — see
            // refresh_skill_picker), the same way.
            KeyCode::Esc if skill_open => {
                self.skill_picker = None;
                Action::None
            }
            // Esc on an empty shell-mode composer exits the mode (codex's
            // bash-mode escape) — before the interrupt/quit fallbacks, like the
            // palette dismissal. With a draft, Esc keeps its normal meaning.
            KeyCode::Esc if self.shell_mode && self.input.is_empty() => {
                self.shell_mode = false;
                Action::None
            }
            // Esc in an agent session view returns to the main conversation
            // (with an empty composer — a typed draft keeps Esc a no-op, the
            // codex composer rule). Before the interrupt arm: leaving the
            // view never interrupts the main turn. See docs/agent-tool.md.
            KeyCode::Esc if self.agent_view.is_some() && self.input.is_empty() => {
                self.close_agent_view();
                Action::LeaveAgentView
            }
            KeyCode::Esc if self.turn_active() && self.agent_view.is_none() => Action::Interrupt,
            // Esc-Esc backtrack (docs/backtrack.md): a primed second Esc opens
            // the transcript overlay previewing the newest user message; the
            // first Esc primes when the composer is empty and a previous user
            // message exists. Only with *nothing* to backtrack to does idle
            // Esc keep its historical meaning — quit.
            KeyCode::Esc if self.backtrack.primed && self.input.is_empty() => {
                self.open_backtrack_preview();
                Action::ToggleToolView
            }
            KeyCode::Esc if self.input.is_empty() && self.has_backtrack_target() => {
                self.backtrack.primed = true;
                Action::None
            }
            // Esc with a typed draft is a no-op, like codex (its composer
            // only acts on Esc when empty): never a quit that throws typed
            // work away — Ctrl+C is the composer-clear, Ctrl+C/`/quit` the
            // exits. Quit below needs an *empty* composer with no target.
            KeyCode::Esc if !self.input.is_empty() => Action::None,
            KeyCode::Esc => Action::Quit,
            // Shift+Tab cycles the permission mode (docs/permissions.md) —
            // Claude Code's key for it, freeing Ctrl+A for the terminal's
            // line-start. Legacy terminals report it as BackTab (`ESC[Z`), the
            // kitty protocol can report Tab+SHIFT — both bind (the Shift+Enter
            // pattern), and the Tab+SHIFT arm sits before every plain-Tab arm
            // so a shifted Tab never queues or completes.
            KeyCode::BackTab => self.toggle_permission_mode(),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.toggle_permission_mode()
            }
            KeyCode::Tab if menu_open => self.run_selected_command(),
            // Tab/Enter accept the highlighted file when the picker is open and
            // a match is selected (codex's accept) — replacing the `@token` with
            // the path. With no matches they fall through to the normal Tab/Enter
            // (queue / submit), so an unmatched `@query` is still sendable text.
            KeyCode::Tab if file_open && self.highlighted_file().is_some() => {
                self.accept_file_selection()
            }
            // Tab/Enter likewise accept the highlighted skill (codex inserts
            // on both) — with no match they fall through, so an unmatched
            // `$query` is still sendable text.
            KeyCode::Tab if skill_open && self.highlighted_skill_match().is_some() => {
                self.accept_skill_selection()
            }
            // Tab inside an agent session view queues a follow-up turn for
            // **that agent** — the same intent one level down, over that
            // agent's own queue (docs/queue.md). It has to be checked before
            // the main-session arm below, and the main arm has to exclude the
            // view, or Tab reaches `is_streaming()`/`queued` — the *lead's*
            // stream and the *lead's* backlog — and a message typed into a
            // subagent runs as a follow-up turn of the main conversation.
            KeyCode::Tab
                if self.agent_view.is_some()
                    && self.viewed_agent_running()
                    && !self.input.text().trim().is_empty() =>
            {
                self.queue_agent_draft();
                Action::None
            }
            // Tab while a turn streams queues the draft as a *new* follow-up
            // batch — a separate turn that runs after the batches already queued,
            // instead of merging into the current one like Enter (codex's
            // Tab-to-queue). Empty/idle Tab falls through to a no-op — and so
            // does Tab in an agent view whose agent has settled, which the
            // arm above deliberately lets through rather than queueing here.
            KeyCode::Tab
                if self.agent_view.is_none()
                    && self.is_streaming()
                    && !self.input.text().trim().is_empty() =>
            {
                self.queue_draft(/*new_batch*/ true);
                Action::None
            }
            KeyCode::Enter if menu_open => {
                if self.highlighted_command().is_some() {
                    self.run_selected_command()
                } else {
                    // A query that matches nothing isn't a message — swallow Enter
                    // rather than submitting the literal "/typo".
                    Action::None
                }
            }
            KeyCode::Enter if file_open && self.highlighted_file().is_some() => {
                self.accept_file_selection()
            }
            KeyCode::Enter if skill_open && self.highlighted_skill_match().is_some() => {
                self.accept_skill_selection()
            }
            // Alt+Enter and Shift+Enter insert a newline at the cursor so the input
            // box grows on demand; a plain Enter submits. Shift+Enter only reaches
            // us when keyboard enhancement is on (pushed by `term::init`); Ctrl+J
            // below is the universal fallback. See docs/shift-enter.md.
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.input.insert_newline();
                Action::None
            }
            KeyCode::Enter => {
                if self.shell_mode && self.input.text().trim().is_empty() {
                    // A bare `!` runs nothing — post the help notice and stay
                    // in the mode (codex's empty-bang help).
                    return Action::Notice(SHELL_EMPTY_NOTICE.to_string());
                }
                if self.input.text().trim().is_empty() {
                    Action::None
                } else if let Some(id) = self.agent_view.clone() {
                    // Inside an agent session view the draft goes to *that
                    // agent*. How it lands is the registry's call, not ours —
                    // queued into a running loop (shown above the box until
                    // that loop reads it) or a chat continuation when idle —
                    // so the boundary records it once it knows which
                    // (docs/agent-tool.md, docs/queue.md).
                    let text = self.take_input();
                    self.file_search = None;
                    self.skill_picker = None;
                    self.input_history.record(&text);
                    Action::AgentChat { id, text }
                } else if self.turn_steerable() {
                    // A model turn is in flight — hand the draft to *that
                    // turn* (codex's steering): it reaches the model at the
                    // next round boundary, right after the round's tool
                    // results, instead of waiting for the turn to finish.
                    // Tab is the other intent — a separate follow-up turn
                    // (see the Tab arm above) — and a `!` command or an
                    // attachment falls back to that queue too (steer_draft
                    // routes both). See docs/queue.md.
                    match self.steer_draft() {
                        Some(text) => Action::Steer(text),
                        None => Action::None,
                    }
                } else if self.is_streaming() {
                    // A `!` shell turn: nothing is reading a conversation, so
                    // the draft queues as a follow-up turn exactly as it
                    // always did (docs/shell-command.md).
                    self.queue_draft(/*new_batch*/ false);
                    Action::None
                } else if self.shell_mode {
                    // Shell mode: run the draft locally (docs/shell-command.md).
                    // Record the full `!command` for ↑ recall (codex records the
                    // whole text — recall re-absorbs the bang).
                    let raw = self.take_input();
                    self.shell_mode = false;
                    self.file_search = None;
                    self.skill_picker = None;
                    self.input_history.record(&format!("!{raw}"));
                    Action::RunShell(raw.trim().to_string())
                } else {
                    // Stage any Ctrl+V-attached images for the boundary to
                    // deliver alongside the text, *before* take_input clears the
                    // composer (the placeholder text stays; the pairs travel
                    // the side channel — docs/image-paste.md).
                    self.submission_images = std::mem::take(&mut self.images);
                    let text = self.take_input();
                    self.file_search = None;
                    self.skill_picker = None;
                    self.input_history.record(&text);
                    Action::Submit(text)
                }
            }
            // Ctrl+J is the *universal* newline key: in raw mode every terminal
            // delivers it as Char('j')+CONTROL (no keyboard enhancement needed), so
            // it inserts a newline like Alt/Shift+Enter even where the terminal
            // can't report a modified Enter (codex binds Ctrl+J the same way; see
            // docs/shift-enter.md).
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.insert_newline();
                Action::None
            }
            // Ctrl+V (and Ctrl+Alt+V — the WSL-friendly alias codex also binds)
            // pastes an image from the system clipboard. The decision is pure; the
            // loop performs the clipboard I/O (`crate::clipboard`) and calls
            // `attach_image` on success. See docs/image-paste.md.
            KeyCode::Char(c)
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && c.eq_ignore_ascii_case(&'v') =>
            {
                Action::PasteImage
            }
            // Editing and cursor movement, dispatched to the textarea. Backspace /
            // Delete / typing also re-derive the slash-command palette.
            // Backspace on an empty shell-mode composer deletes the absorbed
            // `!` — i.e. exits the mode (the natural inverse of typing it).
            KeyCode::Backspace if self.shell_mode && self.input.is_empty() => {
                self.shell_mode = false;
                Action::None
            }
            // Alt+Backspace (readline) / Ctrl+Backspace (the kitty spelling)
            // kill the previous readline word; Alt+Delete / Ctrl+Delete the
            // next — the plain arms below keep their one-grapheme meaning.
            KeyCode::Backspace
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let span = self.input.prev_word_boundary()..self.input.cursor();
                self.kill_and_refresh(span)
            }
            KeyCode::Delete
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let span = self.input.cursor()..self.input.next_word_boundary();
                self.kill_and_refresh(span)
            }
            KeyCode::Backspace => self.backspace_and_refresh(),
            KeyCode::Delete => {
                let had_query = command_query(self.input.text()).is_some();
                let had_token = self.in_at_token();
                let had_mention = self.in_skill_mention();
                if !self.delete_placeholder(/*backward*/ false) {
                    self.input.delete_forward();
                }
                self.refresh_command_menu(had_query);
                self.sync_shell_mode();
                self.refresh_file_search(had_token);
                self.refresh_skill_picker(had_mention);
                Action::None
            }
            // Word motion on Ctrl/Alt-modified arrows (docs/textarea.md) —
            // before the plain arms, which would otherwise swallow them.
            KeyCode::Left
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.input.move_word_left();
                Action::None
            }
            KeyCode::Right
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.input.move_word_right();
                Action::None
            }
            KeyCode::Left => {
                self.input.move_left();
                Action::None
            }
            KeyCode::Right => {
                self.input.move_right();
                Action::None
            }
            // In an agent session view Alt+Up edits **that agent's** pending
            // messages and nothing else — its last follow-up turn, then the
            // message its loop has not read (via the boundary, which is the
            // only side that knows). The main session's backlog belongs to a
            // conversation the user is not looking at, so this never falls
            // through to the arms below.
            KeyCode::Up
                if key.modifiers.contains(KeyModifiers::ALT)
                    && self.input.is_empty()
                    && self.agent_view.is_some() =>
            {
                self.recall_agent_pending()
            }
            // Alt+Up pulls the *last* queued batch back into an *empty* composer
            // as one newline-joined draft (its own messages oldest first) to
            // edit, extend, or drop — codex's edit_queued_message pops the most
            // recent entry, leaving the earlier batches queued. Guarded on an
            // empty composer so it never clobbers a draft (the composer is empty
            // in the normal flow — Enter/Tab emptied it on queue).
            KeyCode::Up
                if key.modifiers.contains(KeyModifiers::ALT)
                    && self.input.is_empty()
                    && !self.queued.is_empty() =>
            {
                self.recall_last_queued();
                Action::None
            }
            // With no follow-up left to edit, Alt+Up reaches the messages
            // handed to the turn already running — but whether one can still
            // be taken back is the boundary's shared queue to answer, not
            // ours: it may have been read a moment ago (docs/queue.md).
            KeyCode::Up
                if key.modifiers.contains(KeyModifiers::ALT)
                    && self.input.is_empty()
                    && !self.steered.is_empty() =>
            {
                Action::ReclaimSteered
            }
            // ↑/↓ (and their terminal twins Ctrl+P/Ctrl+N): the open band's
            // selection first (codex's "popups win"), then shell-style history
            // recall — only from an empty composer or an unedited recall
            // (docs/input-history.md) — then cursor movement. Only the real ↓
            // walks the footer (the shell indicator / agent roster) — Ctrl+N
            // stays an editing key.
            KeyCode::Up => self.nav_up(),
            KeyCode::Down => self.nav_down(/*footer_walk*/ true),
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => self.nav_up(),
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.nav_down(/*footer_walk*/ false)
            }
            // Ctrl+B moves the running command (a model `bash` call or a `!`
            // shell turn) to the background — the loop raises the registry
            // request the runner's poll loop consumes. See `docs/background.md`.
            // With nothing backgroundable running it is the terminal's
            // cursor-left instead (readline's Ctrl+B) — the session meaning
            // wins while it applies, the editing one the rest of the time.
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.can_move_to_background() {
                    Action::MoveToBackground
                } else {
                    self.input.move_left();
                    Action::None
                }
            }
            // The readline cursor keys (docs/textarea.md): Ctrl+A/Ctrl+E jump
            // to the logical line's ends (Home/End), Ctrl+F steps right
            // (Ctrl+B above steps left when idle). The permission-mode cycle
            // that used to sit on Ctrl+A moved to Shift+Tab.
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.move_home();
                Action::None
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.move_end();
                Action::None
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.move_right();
                Action::None
            }
            // Word motion: Alt+B/Alt+F (readline) and Ctrl/Alt+←/→ (the
            // editor spelling) — by readline words, `docs/textarea.md`.
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.input.move_word_left();
                Action::None
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.input.move_word_right();
                Action::None
            }
            // The kill keys (docs/textarea.md): Ctrl+W rubs out the previous
            // whitespace-delimited word (the shell's), Alt+Backspace /
            // Ctrl+Backspace and Alt+D / Ctrl+Delete kill by readline words,
            // Ctrl+U/Ctrl+K kill to the logical line's start/end (Ctrl+K at
            // the line end takes the newline — Emacs' join). All of them
            // widen over a pasted placeholder rather than cutting it in half
            // (`kill_span`), and re-derive the bands like Backspace.
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let span = self.input.prev_unix_word_boundary()..self.input.cursor();
                self.kill_and_refresh(span)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let span = self.input.cursor_line_start()..self.input.cursor();
                self.kill_and_refresh(span)
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let span = self.input.cursor()..self.input.cursor_kill_end();
                self.kill_and_refresh(span)
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::ALT) => {
                let span = self.input.cursor()..self.input.next_word_boundary();
                self.kill_and_refresh(span)
            }
            // Ctrl+H is Backspace (the terminal's oldest alias) — with the
            // kitty protocol the key arrives as Char('h')+CONTROL where a
            // legacy terminal already sends 0x08. Shell-mode parity included:
            // on an empty shell composer it exits the mode like Backspace.
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.shell_mode && self.input.is_empty() {
                    self.shell_mode = false;
                    Action::None
                } else {
                    self.backspace_and_refresh()
                }
            }
            // Ctrl+T cycles the thinking mode (docs/reasoning.md) — moved off
            // Shift+Tab, which cycles the permission mode now. Like /model,
            // cycling never touches a running turn — the mode rides the
            // *next* request.
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cycle_thinking()
            }
            KeyCode::Home => {
                self.input.move_home();
                Action::None
            }
            KeyCode::End => {
                self.input.move_end();
                Action::None
            }
            // Ctrl+R opens the reverse history search (docs/history-search.md);
            // once open, every key routes to on_key_search instead, where
            // Ctrl+R steps to older matches.
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.begin_history_search();
                Action::None
            }
            // Plain (and Shift-modified) characters insert at the cursor; ALT/CONTROL
            // combos are not text, so they are ignored here. An insert that
            // leaves the text starting with `!` is absorbed into shell mode
            // (sync_shell_mode — codex's bash-mode sync after every edit).
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let had_query = command_query(self.input.text()).is_some();
                let had_token = self.in_at_token();
                let had_mention = self.in_skill_mention();
                self.input.insert_char(c);
                self.refresh_command_menu(had_query);
                self.sync_shell_mode();
                self.refresh_file_search(had_token);
                self.refresh_skill_picker(had_mention);
                Action::None
            }
            _ => Action::None,
        }
    }

    /// ↑ / Ctrl+P: the open band's selection first (codex's "popups win"),
    /// then shell-style history recall (docs/input-history.md), then the
    /// textarea cursor.
    fn nav_up(&mut self) -> Action {
        if self.command_menu.is_some() {
            self.move_command_selection(-1);
            return Action::None;
        }
        if self.file_search.is_some() {
            self.move_file_selection(-1);
            return Action::None;
        }
        if self.skill_band_active() {
            self.move_skill_selection(-1);
            return Action::None;
        }
        if self.should_browse_history()
            && let Some(text) = self.input_history.up()
        {
            self.recall_input(&text);
            return Action::None;
        }
        self.input.move_up();
        Action::None
    }

    /// ↓ / Ctrl+N — [`nav_up`](Self::nav_up)'s mirror. `footer_walk` is the
    /// real ↓'s own affordance: from an idle composer it steps onto the
    /// footer's shell indicator while one runs (`docs/background.md`), else
    /// opens the agent roster's selection (`docs/agent-tool.md`) — on the
    /// `● main` row, or straight back onto the **last picked** agent when the
    /// user has been in the roster before. Ctrl+N skips the walk: it is an
    /// editing key.
    fn nav_down(&mut self, footer_walk: bool) -> Action {
        if self.command_menu.is_some() {
            self.move_command_selection(1);
            return Action::None;
        }
        if self.file_search.is_some() {
            self.move_file_selection(1);
            return Action::None;
        }
        if self.skill_band_active() {
            self.move_skill_selection(1);
            return Action::None;
        }
        if self.should_browse_history()
            && let Some(text) = self.input_history.down()
        {
            self.recall_input(&text);
            return Action::None;
        }
        if footer_walk {
            if self.background_focusable() {
                self.background_focus = true;
                return Action::None;
            }
            if self.agent_selectable() {
                self.agent_selection = Some(self.agent_selection_start());
                return Action::None;
            }
        }
        self.input.move_down();
        Action::None
    }

    /// Backspace (and its Ctrl+H alias): a large-paste placeholder goes whole
    /// (docs/paste.md), otherwise one grapheme — then re-derive the bands and
    /// the shell mode, exactly what every deleting key owes.
    fn backspace_and_refresh(&mut self) -> Action {
        let had_query = command_query(self.input.text()).is_some();
        let had_token = self.in_at_token();
        let had_mention = self.in_skill_mention();
        if !self.delete_placeholder(/*backward*/ true) {
            self.input.delete_backward();
        }
        self.refresh_command_menu(had_query);
        self.sync_shell_mode();
        self.refresh_file_search(had_token);
        self.refresh_skill_picker(had_mention);
        Action::None
    }

    /// A kill key's deletion: splice the byte `span` out placeholder-atomically
    /// ([`kill_span`](Self::kill_span)), then re-derive the bands and the
    /// shell mode like Backspace. An empty span is a quiet no-op.
    fn kill_and_refresh(&mut self, span: std::ops::Range<usize>) -> Action {
        if span.start >= span.end {
            return Action::None;
        }
        let had_query = command_query(self.input.text()).is_some();
        let had_token = self.in_at_token();
        let had_mention = self.in_skill_mention();
        self.kill_span(span);
        self.refresh_command_menu(had_query);
        self.sync_shell_mode();
        self.refresh_file_search(had_token);
        self.refresh_skill_picker(had_mention);
        Action::None
    }
}
