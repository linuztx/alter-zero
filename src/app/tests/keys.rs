//! Key dispatch over the conversation view: typing, Enter/Esc, Ctrl+C,
//! the newline keys, and cursor movement (`docs/textarea.md`,
//! `docs/shift-enter.md`).

use super::*;

#[test]
fn enter_with_text_submits_and_clears_input() {
    let mut app = App::new();
    app.input = TextArea::from_text("hello");
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Submit("hello".to_string()));
    assert_eq!(app.input.text(), "");
}

#[test]
fn enter_with_blank_input_does_nothing() {
    let mut app = App::new();
    app.input = TextArea::from_text("   ");
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::None);
    // whitespace-only input is left untouched
    assert_eq!(app.input.text(), "   ");
}

#[test]
fn enter_while_streaming_does_not_submit() {
    // A turn is in flight: Enter never produces Submit — it queues the
    // message (codex's queued_user_messages) and consumes the composer.
    let mut app = App::new();
    app.input = TextArea::from_text("hello");
    app.begin_stream();
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::None);
    assert_eq!(
        app.input.text(),
        "",
        "the composer is consumed into the queue"
    );
    assert_eq!(app.queued.front(), Some(&batch(&["hello"])));
}

#[test]
fn alt_enter_inserts_a_newline_instead_of_submitting() {
    let mut app = App::new();
    app.input = TextArea::from_text("line one");
    let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
    assert_eq!(app.on_key(alt_enter), Action::None);
    assert_eq!(
        app.input.text(),
        "line one\n",
        "Alt+Enter appends a newline"
    );
}

#[test]
fn shift_enter_inserts_a_newline_too() {
    // Terminals with enhanced keyboard support report Shift+Enter; treat it as
    // a newline like Alt+Enter so the box grows on demand.
    let mut app = App::new();
    app.input = TextArea::from_text("a");
    let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
    assert_eq!(app.on_key(shift_enter), Action::None);
    assert_eq!(app.input.text(), "a\n");
}

#[test]
fn ctrl_j_inserts_a_newline_too() {
    // Ctrl+J is the *universal* newline key: in raw mode the byte 0x0A parses
    // to Char('j')+CONTROL on every terminal (no keyboard enhancement needed),
    // so it's the reliable fallback when a terminal can't report Shift+Enter
    // (codex binds Ctrl+J the same way). See docs/shift-enter.md.
    let mut app = App::new();
    app.input = TextArea::from_text("a");
    let ctrl_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
    assert_eq!(app.on_key(ctrl_j), Action::None);
    assert_eq!(app.input.text(), "a\n");
}

#[test]
fn ctrl_j_grows_input_while_streaming_without_submitting() {
    // Editing (incl. newlines) is allowed mid-stream; only sending is blocked.
    let mut app = App::new();
    app.input = TextArea::from_text("draft");
    app.begin_stream();
    let ctrl_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
    assert_eq!(app.on_key(ctrl_j), Action::None);
    assert_eq!(app.input.text(), "draft\n");
}

#[test]
fn plain_enter_submits_a_multi_line_message_intact() {
    // After Alt+Enter newlines, a plain Enter submits the whole thing.
    let mut app = App::new();
    app.input = TextArea::from_text("first\nsecond");
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Submit("first\nsecond".to_string()));
    assert_eq!(app.input.text(), "");
}

#[test]
fn alt_enter_grows_input_while_streaming_without_submitting() {
    // Editing (incl. newlines) is allowed mid-stream; only sending is blocked.
    let mut app = App::new();
    app.input = TextArea::from_text("draft");
    app.begin_stream();
    let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
    assert_eq!(app.on_key(alt_enter), Action::None);
    assert_eq!(app.input.text(), "draft\n");
}

#[test]
fn esc_quits() {
    let mut app = App::new();
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Quit);
}

#[test]
fn ctrl_c_quits() {
    let mut app = App::new();
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.on_key(ctrl_c), Action::Quit);
}

// --- Ctrl+C clears a non-empty input before it quits (codex-style) ---

#[test]
fn ctrl_c_with_text_in_the_input_clears_it_instead_of_quitting() {
    let mut app = App::new();
    app.input = TextArea::from_text("a long draft the user no longer wants");
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.on_key(ctrl_c), Action::None, "first Ctrl+C only clears");
    assert!(app.input.is_empty(), "the draft is gone");
    assert_eq!(
        app.on_key(ctrl_c),
        Action::Quit,
        "the next Ctrl+C (empty input) quits as before"
    );
}

#[test]
fn ctrl_c_clearing_a_command_token_also_closes_the_palette() {
    let mut app = App::new();
    type_str(&mut app, "/qu");
    assert!(app.command_menu.is_some(), "the palette opened");
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.on_key(ctrl_c), Action::None);
    assert!(app.input.is_empty());
    assert!(
        app.command_menu.is_none(),
        "an emptied input is no longer a /token — the palette closes"
    );
}

#[test]
fn ctrl_c_clearing_an_at_token_also_closes_the_file_picker() {
    let mut app = App::new();
    type_str(&mut app, "@src");
    assert!(app.file_search.is_some(), "the @ picker opened");
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.on_key(ctrl_c), Action::None);
    assert!(app.input.is_empty());
    assert!(
        app.file_search.is_none(),
        "an emptied input is no longer an @token — the picker closes"
    );
}

#[test]
fn ctrl_c_still_quits_from_the_tool_view_even_with_a_draft() {
    // The overlay never shows the input box, so there is nothing to clear
    // there — Ctrl+C keeps meaning quit (smoke Phase 7 relies on it).
    let mut app = App::new();
    app.input = TextArea::from_text("draft");
    app.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert_eq!(app.view, View::ToolOutput);
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.on_key(ctrl_c), Action::Quit);
}

#[test]
fn up_steps_back_through_older_messages_and_clamps_at_the_oldest() {
    let mut app = App::new();
    submit(&mut app, "first");
    submit(&mut app, "second");
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.input.text(), "second", "newest first");
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.input.text(), "first", "then older");
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.input.text(), "first", "the oldest entry clamps");
}

#[test]
fn down_steps_forward_and_clears_past_the_newest() {
    let mut app = App::new();
    submit(&mut app, "first");
    submit(&mut app, "second");
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Up)); // browsing at "first"
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.input.text(), "second", "down steps newer");
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.input.text(),
        "",
        "past the newest the composer clears (codex's exit-browsing)"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.input.text(), "", "a further down is a no-op");
}

#[test]
fn up_does_not_clobber_a_typed_draft() {
    let mut app = App::new();
    submit(&mut app, "sent earlier");
    type_str(&mut app, "a fresh draft");
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.input.text(),
        "a fresh draft",
        "a typed draft is never replaced — the arrow moves the cursor"
    );
}

#[test]
fn editing_a_recalled_message_returns_arrows_to_cursor_movement() {
    let mut app = App::new();
    submit(&mut app, "first");
    submit(&mut app, "second");
    app.on_key(key(KeyCode::Up)); // recall "second"
    app.on_key(key(KeyCode::Char('X'))); // edit it
    assert_eq!(app.input.text(), "secondX");
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.input.text(),
        "secondX",
        "an edited recall is a draft — up no longer browses"
    );
}

#[test]
fn submitting_restarts_browsing_at_the_newest() {
    let mut app = App::new();
    submit(&mut app, "alpha");
    submit(&mut app, "beta");
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Up)); // browsing at "alpha"
    assert_eq!(app.input.text(), "alpha");
    app.on_key(key(KeyCode::Enter)); // resubmit it
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.input.text(),
        "alpha",
        "after a submit, up starts from the newest entry again"
    );
}

#[test]
fn ctrl_c_cleared_draft_is_not_persisted() {
    // The cleared draft recalls this session (see the test above) but must
    // not reach disk — it goes through record_ephemeral.
    let mut app = App::new();
    app.input = TextArea::from_text("abandoned");
    app.on_key(ctrl('c'));
    assert!(
        app.take_unpersisted_inputs().is_empty(),
        "a Ctrl+C-cleared draft is recorded ephemerally, never persisted"
    );
}

#[test]
fn a_sent_message_duplicating_a_cleared_draft_is_still_persisted() {
    // A Ctrl+C-cleared draft is recorded ephemerally (not persisted). If the
    // user then sends the SAME text, the persist dedup is against
    // last_persisted (None here), NOT the ephemeral entries tail — so the
    // genuine submission still reaches disk. (The in-memory entries collapse
    // the visual duplicate; persistence does not.)
    let mut app = App::new();
    app.input = TextArea::from_text("deploy prod");
    app.on_key(ctrl('c')); // ephemeral clear — nothing to persist
    assert!(app.take_unpersisted_inputs().is_empty());
    submit(&mut app, "deploy prod"); // the same text, genuinely sent
    assert_eq!(
        app.take_unpersisted_inputs(),
        ["deploy prod"],
        "a sent message is persisted even when it duplicates a cleared ephemeral draft"
    );
}

// ===== `?` shortcuts band (codex's footer shortcut overlay — docs/shortcuts.md) =====

#[test]
fn question_mark_with_an_empty_composer_toggles_the_shortcuts_band() {
    let mut app = App::new();
    assert!(!app.shortcuts_open);
    assert_eq!(app.on_key(key(KeyCode::Char('?'))), Action::None);
    assert!(app.shortcuts_open, "first ? opens the band");
    assert!(app.input.is_empty(), "the ? was consumed, not typed");
    assert_eq!(app.on_key(key(KeyCode::Char('?'))), Action::None);
    assert!(!app.shortcuts_open, "second ? closes it");
    assert!(app.input.is_empty());
}

#[test]
fn shift_question_mark_also_toggles_the_band() {
    // Terminals differ in whether Shift+/ reports SHIFT — codex binds both.
    let mut app = App::new();
    let shift_q = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);
    app.on_key(shift_q);
    assert!(app.shortcuts_open);
}

#[test]
fn question_mark_types_into_a_non_empty_draft() {
    let mut app = App::new();
    type_str(&mut app, "what is this");
    app.on_key(key(KeyCode::Char('?')));
    assert_eq!(app.input.text(), "what is this?");
    assert!(!app.shortcuts_open, "with a draft, ? is just a character");
}

#[test]
fn any_other_key_closes_the_band_but_still_acts() {
    // codex's reset_mode_after_activity: the overlay is display-only, never
    // modal — the key that dismisses it still does its normal job.
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('?')));
    assert!(app.shortcuts_open);
    app.on_key(key(KeyCode::Char('h')));
    assert!(!app.shortcuts_open, "typing closes the band");
    assert_eq!(app.input.text(), "h", "and the character still lands");
}

#[test]
fn esc_only_dismisses_the_shortcuts_band_when_idle() {
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('?')));
    assert_eq!(
        app.on_key(key(KeyCode::Esc)),
        Action::None,
        "Esc dismisses the band instead of quitting"
    );
    assert!(!app.shortcuts_open);
    assert_eq!(
        app.on_key(key(KeyCode::Esc)),
        Action::Quit,
        "the next Esc (band closed) quits as before"
    );
}

// --- cursor editing (the codex-style textarea, dispatched from on_key) ---

#[test]
fn left_and_right_arrows_move_the_input_cursor() {
    let mut app = App::new();
    type_str(&mut app, "hi");
    assert_eq!(app.input.cursor(), 2, "typing leaves the cursor at the end");
    app.on_key(key(KeyCode::Left));
    assert_eq!(app.input.cursor(), 1);
    app.on_key(key(KeyCode::Right));
    assert_eq!(app.input.cursor(), 2);
}

#[test]
fn typing_inserts_at_the_cursor_not_just_the_end() {
    let mut app = App::new();
    type_str(&mut app, "ac");
    app.on_key(key(KeyCode::Left)); // cursor between a and c
    app.on_key(key(KeyCode::Char('b')));
    assert_eq!(app.input.text(), "abc", "the char lands at the cursor");
}

#[test]
fn backspace_deletes_before_the_cursor_mid_text() {
    let mut app = App::new();
    type_str(&mut app, "axbc");
    app.on_key(key(KeyCode::Home));
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Right)); // cursor just after the 'x'
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(
        app.input.text(),
        "abc",
        "backspace removes the char before it"
    );
}

#[test]
fn delete_key_removes_the_character_at_the_cursor() {
    let mut app = App::new();
    type_str(&mut app, "abc");
    app.on_key(key(KeyCode::Home)); // cursor at the start
    app.on_key(key(KeyCode::Delete));
    assert_eq!(
        app.input.text(),
        "bc",
        "delete removes the char at the cursor"
    );
}

#[test]
fn home_and_end_move_to_the_input_line_bounds() {
    let mut app = App::new();
    type_str(&mut app, "hello");
    app.on_key(key(KeyCode::Home));
    assert_eq!(app.input.cursor(), 0);
    app.on_key(key(KeyCode::End));
    assert_eq!(app.input.cursor(), 5);
}

#[test]
fn up_and_down_move_the_cursor_when_no_palette_is_open() {
    // Two logical lines; the cursor starts at the end (on the 2nd line). With
    // no palette open and a cold wrap cache, ↑/↓ navigate logical lines.
    let mut app = App::new();
    app.input = TextArea::from_text("abc\nde");
    assert!(app.command_menu.is_none());
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.input.cursor(), 2, "up to column 2 of the first line");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.input.cursor(), 6, "down to column 2 of the second line");
}

#[test]
fn down_drives_the_palette_not_the_cursor_when_it_is_open() {
    // With the palette open, ↓ moves the highlight (not the text cursor).
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('/'))); // opens the palette, lists all commands
    let before = app.input.cursor();
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.command_menu.as_ref().unwrap().selected,
        1,
        "palette moved"
    );
    assert_eq!(app.input.cursor(), before, "the text cursor stayed put");
}

#[test]
fn ctrl_modified_characters_are_not_typed_into_the_input() {
    // A Ctrl+<char> combo is never text — even one bound to an editing
    // action (Ctrl+A moves to the line start now; it must not insert 'a').
    let mut app = App::new();
    app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert_eq!(
        app.input.text(),
        "",
        "control combos don't insert a character"
    );
}

#[test]
fn set_system_prompt_stores_the_backends_prompt_for_the_debug_view() {
    let mut app = App::new();
    assert!(app.system_prompt.is_none());
    app.set_system_prompt(Some("be nice".to_string()));
    assert_eq!(app.system_prompt.as_deref(), Some("be nice"));
    app.set_system_prompt(None);
    assert!(app.system_prompt.is_none());
}

#[test]
fn set_agent_system_prompt_stores_the_subagent_prompt_for_the_agent_debug_view() {
    // The boundary injects the prompt a launched subagent actually gets (the
    // main prompt + the subagent note) beside the main one, so the agent
    // session view's Ctrl+D shows the real thing (docs/agent-tool.md).
    let mut app = App::new();
    assert!(app.agent_system_prompt.is_none());
    app.set_agent_system_prompt(Some("be nice\n\nsubagent note".to_string()));
    assert_eq!(
        app.agent_system_prompt.as_deref(),
        Some("be nice\n\nsubagent note")
    );
    app.set_agent_system_prompt(None);
    assert!(app.agent_system_prompt.is_none());
}

#[test]
fn esc_in_the_conversation_still_quits() {
    let mut app = App::new();
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Quit);
}

#[test]
fn ctrl_c_quits_from_the_tool_view_too() {
    let mut app = App::new();
    app.on_key(ctrl('o'));
    assert_eq!(app.on_key(ctrl('c')), Action::Quit);
}

#[test]
fn enter_mid_turn_appends_to_one_batch_in_order() {
    // Consecutive Enters share a single turn-batch, oldest first — they
    // flush together as one next turn.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("first");
    app.on_key(key(KeyCode::Enter));
    app.input = TextArea::from_text("second");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.queued.len(), 1, "both Enters land in one batch");
    assert_eq!(
        app.queued[0],
        batch(&["first", "second"]),
        "FIFO, oldest first"
    );
}

#[test]
fn enter_after_tab_appends_to_the_follow_up_batch() {
    // Once Tab opens a new batch, a plain Enter joins *that* batch (the one
    // now being accumulated), not the first.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("a");
    app.on_key(key(KeyCode::Enter)); // batch 1 = [a]
    app.input = TextArea::from_text("b");
    app.on_key(key(KeyCode::Tab)); // batch 2 = [b]
    app.input = TextArea::from_text("c");
    app.on_key(key(KeyCode::Enter)); // batch 2 = [b, c]
    assert_eq!(app.queued.len(), 2);
    assert_eq!(app.queued[0], batch(&["a"]));
    assert_eq!(app.queued[1], batch(&["b", "c"]));
}

#[test]
fn alt_up_does_not_clobber_a_draft() {
    // With text already in the composer, Alt+Up must not yank a queued message
    // over the draft (it falls through to cursor movement instead).
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("queued");
    app.on_key(key(KeyCode::Enter));
    app.input = TextArea::from_text("a draft");
    assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
    assert_eq!(app.input.text(), "a draft", "the draft is untouched");
    assert_eq!(app.queued.len(), 1, "and the queue is untouched");
}

#[test]
fn arrow_keys_move_the_palette_selection_wrapping_at_the_ends() {
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('/'))); // all commands listed
    let n = COMMANDS.len();
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.command_menu.as_ref().unwrap().selected,
        n - 1,
        "Up from the first command wraps to the last"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.command_menu.as_ref().unwrap().selected,
        0,
        "Down from the last command wraps back to the first"
    );
    for _ in 0..n {
        app.on_key(key(KeyCode::Down));
    }
    assert_eq!(
        app.command_menu.as_ref().unwrap().selected,
        0,
        "a full lap of Downs comes back around"
    );
}

#[test]
fn enter_runs_the_clear_command_emptying_history() {
    let mut app = App::new();
    app.record_user_message("old message");
    type_str(&mut app, "/clear");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Clear);
    assert!(app.history.is_empty(), "clear emptied the conversation");
}

#[test]
fn enter_runs_help_posting_a_notice_that_lists_commands() {
    let mut app = App::new();
    type_str(&mut app, "/help");
    match app.on_key(key(KeyCode::Enter)) {
        Action::Notice(text) => {
            assert!(text.contains("Available commands"), "{text:?}");
            assert!(
                text.contains("/clear"),
                "the notice lists commands: {text:?}"
            );
        }
        other => panic!("expected a Notice, got {other:?}"),
    }
    assert!(app.input.is_empty());
    assert!(app.command_menu.is_none());
}

#[test]
fn enter_runs_the_quit_command() {
    // `/quit` exits the app, like codex's `/quit` ("exit Codex").
    let mut app = App::new();
    type_str(&mut app, "/quit");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Quit);
    assert!(app.input.is_empty(), "the command was consumed");
    assert!(app.command_menu.is_none());
}

#[test]
fn tab_runs_the_highlighted_command_like_enter() {
    let mut app = App::new();
    app.record_user_message("old message");
    type_str(&mut app, "/clear");
    assert_eq!(app.on_key(key(KeyCode::Tab)), Action::Clear);
    assert!(app.history.is_empty(), "Tab ran the command, like Enter");
}

#[test]
fn last_assistant_text_is_none_without_an_assistant_message() {
    let mut app = App::new();
    app.record_user_message("only a user message");
    assert!(app.last_assistant_text().is_none());
}

#[test]
fn enter_with_no_matching_command_does_not_submit() {
    let mut app = App::new();
    type_str(&mut app, "/zzz");
    assert!(app.command_menu.is_some(), "palette is open but empty");
    assert!(matching_commands("zzz").is_empty());
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::None,
        "a no-match query is not submitted as a message"
    );
    assert_eq!(app.input.text(), "/zzz", "the input is left intact");
}

#[test]
fn enter_still_submits_a_normal_message_when_no_palette_is_open() {
    let mut app = App::new();
    app.input = TextArea::from_text("hello");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("hello".to_string())
    );
}

#[test]
fn search_is_case_insensitive() {
    let history = history_of(&["Build Release", "other"]);
    assert_eq!(history.search("release"), vec![0]);
    assert_eq!(history.search("BUILD"), vec![0]);
}

#[test]
fn search_skips_older_duplicates_of_the_same_text() {
    let history = history_of(&["dup", "other dup", "dup"]);
    // "dup" was recorded at 0 and again at 2 (not adjacent, so both kept);
    // the search keeps only the newest occurrence — codex's seen_texts.
    assert_eq!(history.search("dup"), vec![2, 1]);
}

#[test]
fn search_with_an_empty_query_matches_everything() {
    let history = history_of(&["one", "two"]);
    assert_eq!(history.search(""), vec![1, 0]);
}

#[test]
fn ctrl_s_steps_back_newer_and_clamps_at_the_newest_match() {
    let mut app = searchable_app(&["git status", "cargo build", "git push"]);
    app.on_key(ctrl('r'));
    type_query(&mut app, "git");
    app.on_key(ctrl('r'));
    assert_eq!(app.input.text(), "git status");
    app.on_key(ctrl('s'));
    assert_eq!(app.input.text(), "git push");
    app.on_key(ctrl('s'));
    assert_eq!(app.input.text(), "git push", "newest end clamps too");
}

#[test]
fn up_and_down_step_the_search_instead_of_browsing_or_moving() {
    let mut app = searchable_app(&["git status", "cargo build", "git push"]);
    app.on_key(ctrl('r'));
    type_query(&mut app, "git");
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.input.text(), "git status", "↑ steps older, like Ctrl+R");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.input.text(), "git push", "↓ steps newer, like Ctrl+S");
    assert!(
        app.history_search.is_some(),
        "arrows never close the search"
    );
}

#[test]
fn enter_accepts_the_match_into_the_composer_without_submitting() {
    let mut app = searchable_app(&["git status", "git push"]);
    app.on_key(ctrl('r'));
    type_query(&mut app, "status");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.history_search.is_none(), "accepting closes the search");
    assert_eq!(app.input.text(), "git status", "the match stays as a draft");
    assert_eq!(app.input.cursor(), "git status".len());
    assert!(app.queued.is_empty());
}

#[test]
fn enter_without_a_match_is_swallowed_and_the_search_stays_open() {
    let mut app = searchable_app(&["git status"]);
    app.on_key(ctrl('r'));
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.history_search.is_some(), "Idle: Enter does nothing");
    type_query(&mut app, "zzz");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.history_search.is_some(), "NoMatch: Enter does nothing");
}

#[test]
fn accepting_seats_arrow_browsing_at_the_match() {
    let mut app = searchable_app(&["alpha one", "beta two", "alpha three"]);
    app.on_key(ctrl('r'));
    type_query(&mut app, "two");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.input.text(), "beta two");
    // ↑ continues *older* from the accepted entry, codex-style.
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.input.text(), "alpha one");
}

#[test]
fn esc_cancels_and_restores_the_draft_text_and_cursor() {
    let mut app = searchable_app(&["git status"]);
    app.input = TextArea::from_text("hello");
    app.input.move_left();
    app.input.move_left();
    let cursor = app.input.cursor();
    app.on_key(ctrl('r'));
    type_query(&mut app, "git");
    assert_eq!(app.input.text(), "git status");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(app.history_search.is_none());
    assert_eq!(app.input.text(), "hello");
    assert_eq!(app.input.cursor(), cursor, "the exact cursor is restored");
}

#[test]
fn ctrl_c_cancels_the_search_instead_of_clearing_or_quitting() {
    let mut app = searchable_app(&["git status"]);
    app.input = TextArea::from_text("a draft");
    app.on_key(ctrl('r'));
    type_query(&mut app, "git");
    assert_eq!(app.on_key(ctrl('c')), Action::None);
    assert!(app.history_search.is_none());
    assert_eq!(app.input.text(), "a draft", "the restored draft survives");
}

#[test]
fn an_edit_that_creates_a_leading_bang_absorbs_it() {
    // codex syncs the mode from the text after every edit, so inserting a
    // `!` in front of an existing draft enters shell mode too.
    let mut app = App::new();
    type_query(&mut app, "ls");
    app.on_key(key(KeyCode::Home));
    type_query(&mut app, "!");
    assert!(app.shell_mode);
    assert_eq!(app.input.text(), "ls");
}

#[test]
fn absorbing_a_typed_bang_keeps_the_cursor_where_it_was() {
    // Only the bang leaves the text — the cursor must not teleport to the
    // end: type "ls", Home, "!" (cursor now before the "l"), then "x"
    // continues typing there, not after the "s".
    let mut app = App::new();
    type_query(&mut app, "ls");
    app.on_key(key(KeyCode::Home));
    type_query(&mut app, "!");
    assert_eq!(app.input.cursor(), 0, "the cursor stays before the command");
    type_query(&mut app, "x");
    assert_eq!(app.input.text(), "xls");
}

#[test]
fn absorbing_a_bang_exposed_by_backspace_keeps_the_cursor_at_the_front() {
    // Draft "x!ls" with the cursor after the "x": Backspace exposes the
    // leading bang, which is absorbed — the cursor stays at the front of
    // the remaining command rather than jumping past "ls".
    let mut app = App::new();
    type_query(&mut app, "x!ls");
    for _ in 0..3 {
        app.on_key(key(KeyCode::Left));
    }
    app.on_key(key(KeyCode::Backspace));
    assert!(app.shell_mode);
    assert_eq!(app.input.text(), "ls");
    assert_eq!(app.input.cursor(), 0);
}

#[test]
fn ctrl_c_in_shell_mode_records_the_prefixed_draft_and_exits_the_mode() {
    let mut app = App::new();
    type_query(&mut app, "!ls");
    assert_eq!(app.on_key(ctrl('c')), Action::None);
    assert!(!app.shell_mode);
    assert_eq!(app.input.text(), "");
    app.on_key(key(KeyCode::Up));
    assert!(app.shell_mode, "↑ recalls the cleared draft back into mode");
    assert_eq!(app.input.text(), "ls");
}

#[test]
fn idle_enter_in_shell_mode_runs_the_command() {
    let mut app = App::new();
    type_query(&mut app, "!echo hello");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::RunShell("echo hello".to_string())
    );
    assert_eq!(app.input.text(), "", "the composer is consumed");
    assert!(!app.shell_mode, "running exits the mode");
}

#[test]
fn enter_accepts_the_highlighted_file_replacing_the_token() {
    let mut app = App::new();
    type_str(&mut app, "see @ma");
    app.set_file_matches("ma", vec![fm("src/main.rs")]);
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(app.input.text(), "see src/main.rs ");
    assert!(app.file_search.is_none());
}

#[test]
fn tab_accepts_the_highlighted_file_too() {
    let mut app = App::new();
    type_str(&mut app, "@m");
    app.set_file_matches("m", vec![fm("a.rs"), fm("b.rs")]);
    app.on_key(key(KeyCode::Down)); // pick the second match
    assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
    assert_eq!(app.input.text(), "b.rs ");
}

#[test]
fn up_down_move_the_file_selection_wrapping_at_the_ends() {
    let mut app = App::new();
    type_str(&mut app, "@x");
    app.set_file_matches("x", vec![fm("x1"), fm("x2")]);
    assert_eq!(app.file_search.as_ref().unwrap().selected, 0);
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.file_search.as_ref().unwrap().selected, 1);
    app.on_key(key(KeyCode::Down)); // wrap past the last match
    assert_eq!(app.file_search.as_ref().unwrap().selected, 0);
    app.on_key(key(KeyCode::Up)); // wrap back past the first
    assert_eq!(app.file_search.as_ref().unwrap().selected, 1);
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.file_search.as_ref().unwrap().selected, 0);
}

#[test]
fn submitting_an_unmatched_at_query_closes_the_picker() {
    let mut app = App::new();
    type_str(&mut app, "hi @zzz"); // no matches → nothing highlighted
    assert!(app.file_search.is_some());
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Submit("hi @zzz".to_string()));
    assert!(app.file_search.is_none());
}

#[test]
fn any_other_key_unprimes() {
    // Codex resets priming on any non-Esc key — no timeout, no sticky arm.
    let mut app = App::new();
    exchange(&mut app, "hello", "hi");
    app.on_key(key(KeyCode::Esc));
    assert!(app.backtrack.primed);
    app.on_key(key(KeyCode::Char('x')));
    assert!(!app.backtrack.primed, "typing disarms");
    assert_eq!(app.input.text(), "x", "and the key still does its job");
}

#[test]
fn second_esc_opens_the_transcript_preview_on_the_last_user_message() {
    let mut app = App::new();
    exchange(&mut app, "first", "a");
    exchange(&mut app, "second", "b");
    app.on_key(key(KeyCode::Esc)); // prime
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::ToggleToolView);
    assert_eq!(app.view, View::ToolOutput, "the transcript overlay opens");
    assert_eq!(
        app.backtrack.selected,
        Some(1),
        "the newest user message is highlighted first"
    );
    assert!(
        !app.tool_follow,
        "previewing pins to the highlight, not the tail"
    );
}

#[test]
fn esc_and_left_step_the_preview_older_saturating() {
    let mut app = App::new();
    exchange(&mut app, "one", "a");
    exchange(&mut app, "two", "b");
    exchange(&mut app, "three", "c");
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.backtrack.selected, Some(2));
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(app.backtrack.selected, Some(1), "Esc steps older");
    app.on_key(key(KeyCode::Left));
    assert_eq!(app.backtrack.selected, Some(0), "← steps older too");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(
        app.backtrack.selected,
        Some(0),
        "stepping stops at the oldest (codex saturates)"
    );
}

#[test]
fn right_steps_the_preview_newer_clamped() {
    let mut app = App::new();
    exchange(&mut app, "one", "a");
    exchange(&mut app, "two", "b");
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc)); // step older → 0
    app.on_key(key(KeyCode::Right));
    assert_eq!(app.backtrack.selected, Some(1), "→ steps newer");
    app.on_key(key(KeyCode::Right));
    assert_eq!(app.backtrack.selected, Some(1), "…clamped at the newest");
}

#[test]
fn enter_confirms_truncating_history_and_prefilling_the_composer() {
    let mut app = App::new();
    exchange(&mut app, "first", "a");
    exchange(&mut app, "second", "b");
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc)); // preview on "second"
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::ConfirmBacktrack);
    assert_eq!(app.view, View::Conversation, "back to the inline view");
    assert_eq!(app.input.text(), "second", "the message is back to edit");
    assert_eq!(app.input.cursor(), "second".len(), "cursor at the end");
    // The selected message and everything after it are gone; the first
    // exchange (user + assistant + summary) survives untouched.
    assert_eq!(app.history.len(), 3);
    assert_eq!(message_at(&app, 0).text, "first");
    assert_eq!(
        app.backtrack,
        Backtrack::default(),
        "the gesture state fully resets"
    );
}

#[test]
fn enter_discards_a_drafted_attachment_the_rewind_clobbers() {
    // A draft with its own attachment can be open when the preview is
    // begun from the Ctrl+O view; confirming clobbers the draft, so its
    // pair must not linger as a ghost image on the rewound message.
    let mut app = App::new();
    exchange(&mut app, "hello", "hi");
    app.attach_image(PathBuf::from("/tmp/draft.png"));
    app.on_key(ctrl('o')); // the overlay opens over the non-empty draft
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None); // begin preview
    app.on_key(key(KeyCode::Enter)); // rewind to "hello"
    assert_eq!(app.input.text(), "hello");
    assert!(app.images.is_empty(), "no unanchored pairs survive");
    assert_eq!(
        app.take_discarded_images(),
        vec![PathBuf::from("/tmp/draft.png")],
        "the clobbered draft's temp file is queued for deletion"
    );
}

#[test]
fn confirming_the_oldest_message_empties_the_history() {
    let mut app = App::new();
    exchange(&mut app, "first", "a");
    exchange(&mut app, "second", "b");
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc)); // step to "first"
    app.on_key(key(KeyCode::Enter));
    assert!(
        app.history.is_empty(),
        "everything from `first` on is dropped"
    );
    assert_eq!(app.input.text(), "first");
}

#[test]
fn q_cancels_the_preview_without_truncating() {
    let mut app = App::new();
    exchange(&mut app, "first", "a");
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.on_key(key(KeyCode::Char('q'))), Action::ToggleToolView);
    assert_eq!(app.view, View::Conversation);
    assert_eq!(app.history.len(), 3, "nothing was dropped");
    assert!(app.input.is_empty(), "nothing was prefilled");
    assert_eq!(app.backtrack, Backtrack::default());
}

#[test]
fn esc_in_the_overlay_begins_the_preview_when_idle_with_a_target() {
    // Codex's Ctrl+T → Esc path: Esc inside an already-open transcript
    // view starts backtracking in place instead of closing the view.
    let mut app = App::new();
    exchange(&mut app, "hello", "hi");
    app.on_key(ctrl('o'));
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(app.view, View::ToolOutput, "the overlay stays up");
    assert_eq!(app.backtrack.selected, Some(0));
}

#[test]
fn esc_in_the_overlay_still_closes_it_with_no_target() {
    let mut app = App::new();
    app.on_key(ctrl('o'));
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::ToggleToolView);
    assert_eq!(app.view, View::Conversation);
}

#[test]
fn tab_moves_the_toolbar_focus_and_arrows_toggle_the_sort() {
    let mut app = picker_app(&[("a", "old"), ("b", "new")]);
    {
        // "a" was modified most recently but created earlier; "b" the
        // reverse — so the two sort keys order them differently.
        let picker = app.resume_picker.as_mut().unwrap();
        picker.sessions[0].updated_secs = 10; // a: touched just now
        picker.sessions[0].created_secs = 900; // …but started earlier
        picker.sessions[1].updated_secs = 500;
        picker.sessions[1].created_secs = 100; // b: the newer session
    }
    let updated_order: Vec<&str> = app
        .resume_picker
        .as_ref()
        .unwrap()
        .matches()
        .iter()
        .map(|s| s.preview.as_str())
        .collect();
    assert_eq!(updated_order, vec!["old", "new"], "Updated: mtime order");
    app.on_key(key(KeyCode::Tab)); // focus: Filter → Sort
    assert_eq!(
        app.resume_picker.as_ref().unwrap().focus,
        ResumeControl::Sort
    );
    app.on_key(key(KeyCode::Right)); // Sort: Updated → Created
    let picker = app.resume_picker.as_ref().unwrap();
    assert_eq!(picker.sort, ResumeSort::Created);
    let created_order: Vec<&str> = picker
        .matches()
        .iter()
        .map(|s| s.preview.as_str())
        .collect();
    assert_eq!(created_order, vec!["new", "old"], "Created: start order");
    assert_eq!(picker.filter, ResumeFilter::Cwd, "filter untouched");
    app.on_key(key(KeyCode::BackTab)); // two controls: prev == next
    assert_eq!(
        app.resume_picker.as_ref().unwrap().focus,
        ResumeControl::Filter
    );
}

#[test]
fn picker_up_down_moves_wrap_at_both_ends() {
    let mut app = picker_app(&[("a", "one"), ("b", "two"), ("c", "three")]);
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.resume_picker.as_ref().unwrap().selected,
        2,
        "Up from the first row wraps to the last"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.resume_picker.as_ref().unwrap().selected,
        0,
        "Down from the last row wraps back to the first"
    );
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.resume_picker.as_ref().unwrap().selected, 2);
}

#[test]
fn picker_page_home_and_end_jump() {
    let sessions: Vec<(String, String)> = (0..25)
        .map(|i| (format!("s{i}"), format!("message {i}")))
        .collect();
    let refs: Vec<(&str, &str)> = sessions
        .iter()
        .map(|(p, v)| (p.as_str(), v.as_str()))
        .collect();
    let mut app = picker_app(&refs);
    app.on_key(key(KeyCode::PageDown));
    assert_eq!(app.resume_picker.as_ref().unwrap().selected, 10);
    app.on_key(key(KeyCode::End));
    assert_eq!(app.resume_picker.as_ref().unwrap().selected, 24);
    app.on_key(key(KeyCode::Home));
    assert_eq!(app.resume_picker.as_ref().unwrap().selected, 0);
    app.on_key(key(KeyCode::PageUp));
    assert_eq!(app.resume_picker.as_ref().unwrap().selected, 0, "clamped");
}

#[test]
fn enter_resumes_the_selected_filtered_row() {
    let mut app = picker_app(&[("a", "wrap bug"), ("b", "shell fun"), ("c", "wrap fix")]);
    type_chars(&mut app, "wrap");
    app.on_key(key(KeyCode::Down)); // second match = "wrap fix" at path c
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::ResumeSession(PathBuf::from("c")),
    );
}

#[test]
fn enter_on_an_empty_picker_does_nothing() {
    let mut app = picker_app(&[]);
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(app.view, View::ResumePicker, "the picker stays up");
}

#[test]
fn ctrl_c_closes_the_picker_not_the_app() {
    // Codex's from-a-session picker: Ctrl+C leaves the picker, never the
    // app (the startup picker's quit has no equivalent here).
    let mut app = picker_app(&[("a", "hello")]);
    assert_eq!(app.on_key(ctrl('c')), Action::CloseResumePicker);
    assert_eq!(app.view, View::Conversation);
}

#[test]
fn enter_does_nothing_when_no_provider_is_configured() {
    let mut app = App::new();
    app.open_model_picker("m");
    app.set_models_needs_login();
    // Enter has nothing to select — it must not emit a SelectModel action.
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.model_picker.is_some(), "picker stays open");
}

#[test]
fn arrows_move_the_selection_wrapping_at_the_ends() {
    let mut app = model_app(&sample_models());
    app.model_picker.as_mut().unwrap().selected = 0;
    app.on_key(key(KeyCode::Up)); // top → bottom
    assert_eq!(app.model_picker.as_ref().unwrap().selected, 2);
    app.on_key(key(KeyCode::Down)); // bottom → top
    assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.model_picker.as_ref().unwrap().selected, 1);
    app.on_key(key(KeyCode::End));
    assert_eq!(app.model_picker.as_ref().unwrap().selected, 2);
    app.on_key(key(KeyCode::Home));
    assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
}

#[test]
fn enter_on_an_empty_list_keeps_the_picker_open() {
    let mut app = App::new();
    app.open_model_picker("m");
    app.set_models(vec![]);
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.model_picker.is_some());
}

#[test]
fn esc_clears_the_query_first_then_closes() {
    let mut app = model_app(&sample_models());
    type_chars(&mut app, "kimi");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(app.model_picker.as_ref().unwrap().query.is_empty());
    assert!(
        app.model_picker.is_some(),
        "first Esc only clears the query"
    );
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseModelPicker);
    assert!(app.model_picker.is_none());
}

#[test]
fn enter_advances_to_the_key_step_for_the_highlighted_provider() {
    let mut app = login_app();
    app.key_onboarding.as_mut().unwrap().selected = 1; // openrouter
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Key);
    assert_eq!(onboarding.chosen_provider().unwrap().id, "openrouter");
}

#[test]
fn enter_pins_the_index_from_the_filtered_matches() {
    let mut app = login_app();
    // Filter to a single match whose *unfiltered* index is 2 (together).
    type_chars(&mut app, "toget");
    assert_eq!(app.key_onboarding.as_ref().unwrap().matches().len(), 1);
    app.on_key(key(KeyCode::Enter));
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.chosen, Some(2));
    assert_eq!(onboarding.chosen_provider().unwrap().id, "together");
}

#[test]
fn arrows_move_the_provider_selection_wrapping_at_the_ends() {
    let mut app = login_app();
    app.on_key(key(KeyCode::Up)); // top → bottom
    assert_eq!(app.key_onboarding.as_ref().unwrap().selected, 2);
    app.on_key(key(KeyCode::Down)); // bottom → top
    assert_eq!(app.key_onboarding.as_ref().unwrap().selected, 0);
    app.on_key(key(KeyCode::End));
    assert_eq!(app.key_onboarding.as_ref().unwrap().selected, 2);
    app.on_key(key(KeyCode::Home));
    assert_eq!(app.key_onboarding.as_ref().unwrap().selected, 0);
}

#[test]
fn enter_on_the_key_step_saves_and_closes() {
    let mut app = key_app("openrouter");
    type_chars(&mut app, "sk-secret");
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Action::SaveApiKey {
            provider: "openrouter".into(),
            env_var: "OPENROUTER_API_KEY".into(),
            key: "sk-secret".into(),
        }
    );
    assert!(app.key_onboarding.is_none(), "saving closes the flow");
}

#[test]
fn empty_key_enter_is_a_noop() {
    let mut app = key_app("openrouter");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.key_onboarding.is_some(), "still waiting for a key");
}

#[test]
fn history_generation_bumps_on_every_non_append_mutation() {
    // The Ctrl+O transcript cache freezes rendered history items and only
    // appends — sound *only if* every non-append history mutation is
    // observable. Appends keep the generation; anything that clears,
    // replaces, truncates, or pops history must bump it (a pop + re-push
    // can land on the same length, so lengths alone can't be trusted).
    let mut app = App::new();
    let start = app.history_generation();
    app.record_user_message("one");
    app.record_user_message("two");
    assert_eq!(app.history_generation(), start, "appends never bump");

    // The Esc-Esc backtrack rewind truncates history.
    app.on_key(key(KeyCode::Esc)); // arm
    app.on_key(key(KeyCode::Esc)); // preview at the newest ("two")
    app.on_key(key(KeyCode::Enter)); // rewind: history = [one]
    let after_rewind = app.history_generation();
    assert_ne!(after_rewind, start, "a backtrack truncation bumps");

    // The interrupt-undo pops the just-submitted user message.
    app.record_user_message("new");
    app.begin_stream();
    assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
    let after_undo = app.history_generation();
    assert_ne!(after_undo, after_rewind, "an interrupt-undo pop bumps");

    // /resume replaces the whole conversation (load_session clears first).
    app.load_session(vec![HistoryItem::Message(Message {
        role: Role::User,
        text: "loaded".to_string(),
        timestamp: String::new(),
        images: Vec::new(),
    })]);
    let after_load = app.history_generation();
    assert_ne!(after_load, after_undo, "a session load bumps");

    // /clear wipes it.
    app.clear_conversation();
    assert_ne!(app.history_generation(), after_load, "/clear bumps");
}

#[test]
fn the_newline_keys_keep_their_meaning_under_the_highlight() {
    // Shift/Alt+Enter and Ctrl+J are the newline keys (docs/shift-enter.md):
    // a lit indicator must not swallow them — only a *plain* Enter opens
    // the band.
    for newline in [
        KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
    ] {
        let mut app = app_with_shells(&["a"]);
        app.on_key(key(KeyCode::Down));
        app.on_key(newline);
        assert!(
            app.background_view.is_none(),
            "{newline:?} is a newline key, not the band's opener"
        );
        assert_eq!(app.input.text(), "\n", "{newline:?} inserted a newline");
        assert!(
            !app.background_focused(),
            "{newline:?} still dismisses the highlight"
        );
    }
}

#[test]
fn esc_up_and_ctrl_c_clear_the_shell_highlight_without_quitting() {
    let mut app = app_with_shells(&["a"]);
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.on_key(key(KeyCode::Esc)),
        Action::None,
        "Esc dismisses the highlight instead of quitting"
    );
    assert!(!app.background_focused());
    assert!(!app.backtrack.primed, "…and never arms the backtrack");
    // ↑ steps back out of the footer the way ↓ stepped into it.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Up));
    assert!(!app.background_focused());
    // Ctrl+C clears the highlight first, like it clears a draft.
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.on_key(ctrl('c')), Action::None);
    assert!(!app.background_focused());
}

#[test]
fn manager_list_keys_select_view_stop_and_close() {
    let mut app = app_with_shells(&["a", "b", "c"]);
    app.open_background_view();
    // ↓/↑ move, wrapping at the ends.
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.background_view,
        Some(BackgroundView::List { selected: 2 }),
        "Up from the top wraps to the last row"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.background_view,
        Some(BackgroundView::List { selected: 0 }),
        "Down from the bottom wraps back to the first"
    );
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.background_view,
        Some(BackgroundView::List { selected: 2 })
    );
    // x stops the highlighted shell (the view stays).
    let action = app.on_key(key(KeyCode::Char('x')));
    assert_eq!(action, Action::KillBackground("bash_3".into()));
    assert!(app.background_view.is_some());
    // Enter opens the details of the highlighted shell.
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.background_view,
        Some(BackgroundView::Details {
            id: "bash_2".into()
        })
    );
    // ← goes back to the list, seated on that shell.
    app.on_key(key(KeyCode::Left));
    assert_eq!(
        app.background_view,
        Some(BackgroundView::List { selected: 1 })
    );
    // Esc closes the band.
    app.on_key(key(KeyCode::Esc));
    assert!(app.background_view.is_none());
}

#[test]
fn manager_details_keys_close_and_stop() {
    let mut app = app_with_shells(&["a"]);
    app.open_background_view();
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(
        app.background_view,
        Some(BackgroundView::Details { .. })
    ));
    // x stops this shell.
    assert_eq!(
        app.on_key(key(KeyCode::Char('x'))),
        Action::KillBackground("bash_1".into())
    );
    // Space closes outright (Esc and Enter do too — spec).
    app.on_key(key(KeyCode::Char(' ')));
    assert!(app.background_view.is_none());
    app.open_background_view();
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));
    assert!(app.background_view.is_none(), "Enter closes the details");
    // Ctrl+C closes from either page.
    app.open_background_view();
    app.on_key(ctrl('c'));
    assert!(app.background_view.is_none());
}

#[test]
fn ctrl_t_without_support_raises_an_info_toast() {
    // A model with no reasoning (or the dummy backend): Ctrl+T explains
    // instead of dying silently. The *loop* presents the toast (arming its
    // expiry), so this is an Action, not a direct show_toast.
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/repo");
    assert_eq!(
        app.on_key(ctrl('t')),
        Action::Toast("dummy_model_name does not support thinking".into())
    );
    assert!(app.thinking.is_none());
}

// ===== terminal editing shortcuts (docs/textarea.md) =====

#[test]
fn ctrl_a_and_ctrl_e_jump_to_the_line_ends() {
    // Ctrl+A is line-start now — the permission-mode cycle moved to
    // Shift+Tab (docs/permissions.md).
    let mut app = App::new();
    type_chars(&mut app, "hello");
    assert_eq!(app.on_key(ctrl('a')), Action::None);
    assert_eq!(app.input.cursor(), 0, "ctrl+a = home");
    assert_eq!(app.on_key(ctrl('e')), Action::None);
    assert_eq!(app.input.cursor(), 5, "ctrl+e = end");
}

#[test]
fn shift_tab_cycles_the_permission_mode() {
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Manual));
    assert_eq!(
        app.on_key(backtab()),
        Action::SetPermissionMode(PermissionMode::Edit)
    );
    // The kitty protocol reports Shift+Tab as Tab+SHIFT (docs/shift-enter.md).
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT)),
        Action::SetPermissionMode(PermissionMode::Auto)
    );
}

#[test]
fn shift_tab_with_permissions_disabled_explains() {
    let mut app = App::new();
    assert_eq!(
        app.on_key(backtab()),
        Action::Toast("Tool permissions are disabled".into())
    );
}

#[test]
fn ctrl_b_is_cursor_left_when_nothing_is_running() {
    let mut app = App::new();
    type_chars(&mut app, "hi");
    assert_eq!(app.on_key(ctrl('b')), Action::None);
    assert_eq!(app.input.cursor(), 1, "idle ctrl+b = ←");
    assert_eq!(app.on_key(ctrl('f')), Action::None);
    assert_eq!(app.input.cursor(), 2, "ctrl+f = →");
}

#[test]
fn ctrl_b_still_backgrounds_a_running_command_over_a_draft() {
    // With a backgroundable command running the session meaning wins even
    // mid-draft — the cursor never steals the key from the background move.
    let mut app = App::new();
    type_chars(&mut app, "draft");
    app.begin_stream();
    app.start_tool("Bash", "ping x.com");
    assert_eq!(app.on_key(ctrl('b')), Action::MoveToBackground);
    assert_eq!(app.input.cursor(), 5, "the cursor stayed put");
}

#[test]
fn word_motion_binds_alt_b_f_and_ctrl_arrows() {
    let mut app = App::new();
    type_chars(&mut app, "foo bar");
    app.on_key(alt(KeyCode::Char('b')));
    assert_eq!(app.input.cursor(), 4, "alt+b: start of \"bar\"");
    app.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
    assert_eq!(app.input.cursor(), 0, "ctrl+←: start of \"foo\"");
    app.on_key(alt(KeyCode::Char('f')));
    assert_eq!(app.input.cursor(), 3, "alt+f: end of \"foo\"");
    app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
    assert_eq!(app.input.cursor(), 7, "ctrl+→: end of \"bar\"");
}

#[test]
fn ctrl_w_kills_the_previous_unix_word() {
    let mut app = App::new();
    type_chars(&mut app, "run src/main.rs ");
    app.on_key(ctrl('w'));
    assert_eq!(app.input.text(), "run ", "the whole path went as one word");
    assert_eq!(app.input.cursor(), 4);
}

#[test]
fn alt_backspace_kills_the_previous_word_stopping_at_punctuation() {
    let mut app = App::new();
    type_chars(&mut app, "src/main.rs");
    app.on_key(alt(KeyCode::Backspace));
    assert_eq!(app.input.text(), "src/main.");
    // Ctrl+Backspace (kitty protocol) is its alias.
    app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
    assert_eq!(app.input.text(), "src/");
}

#[test]
fn alt_d_kills_the_next_word() {
    let mut app = App::new();
    type_chars(&mut app, "foo bar");
    app.input.move_home();
    app.on_key(alt(KeyCode::Char('d')));
    assert_eq!(app.input.text(), " bar", "to the end of \"foo\"");
    assert_eq!(app.input.cursor(), 0);
    // Ctrl+Delete is its alias.
    app.on_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::CONTROL));
    assert_eq!(app.input.text(), "", "\" bar\" went too");
}

#[test]
fn ctrl_u_kills_to_the_line_start_and_ctrl_k_to_the_end() {
    let mut app = App::new();
    type_chars(&mut app, "hello world");
    for _ in 0..6 {
        app.input.move_left();
    }
    app.on_key(ctrl('u'));
    assert_eq!(app.input.text(), " world");
    assert_eq!(app.input.cursor(), 0);
    app.on_key(ctrl('k'));
    assert_eq!(app.input.text(), "", "ctrl+k took the rest of the line");
}

#[test]
fn ctrl_k_at_a_line_end_joins_the_lines() {
    let mut app = App::new();
    type_chars(&mut app, "ab");
    app.on_key(ctrl('j')); // newline
    type_chars(&mut app, "cd");
    app.input.move_up(); // cold cache: logical-line motion → end of "ab"
    assert_eq!(app.input.cursor(), 2);
    app.on_key(ctrl('k'));
    assert_eq!(app.input.text(), "abcd", "the '\\n' itself was killed");
}

#[test]
fn a_kill_re_derives_the_command_palette() {
    // Killing the draft back to empty must close the palette like Backspace
    // does — a stale band over an empty composer taught the old bug class.
    let mut app = App::new();
    type_chars(&mut app, "/he");
    assert!(app.command_menu.is_some());
    app.on_key(ctrl('w'));
    assert_eq!(app.input.text(), "");
    assert!(app.command_menu.is_none(), "the palette re-derived");
}

#[test]
fn a_kill_exits_shell_mode_with_the_text() {
    // `!` lives in shell_mode, not the text — an emptied composer stays in
    // the mode (Backspace parity: only Backspace/Esc on empty exit it).
    let mut app = App::new();
    type_chars(&mut app, "!ping x");
    assert!(app.shell_mode);
    app.on_key(ctrl('u'));
    assert_eq!(app.input.text(), "");
    assert!(
        app.shell_mode,
        "the mode itself survives, like Backspace-to-empty"
    );
}

#[test]
fn ctrl_h_is_backspace() {
    let mut app = App::new();
    type_chars(&mut app, "ab");
    app.on_key(ctrl('h'));
    assert_eq!(app.input.text(), "a");
}

#[test]
fn ctrl_p_and_ctrl_n_step_the_input_history() {
    let mut app = App::new();
    submit(&mut app, "one");
    submit(&mut app, "two");
    app.on_key(ctrl('p'));
    assert_eq!(app.input.text(), "two");
    app.on_key(ctrl('p'));
    assert_eq!(app.input.text(), "one");
    app.on_key(ctrl('n'));
    assert_eq!(app.input.text(), "two");
}

#[test]
fn ctrl_p_and_ctrl_n_move_the_cursor_in_an_edited_draft() {
    let mut app = App::new();
    type_chars(&mut app, "ab");
    app.on_key(ctrl('j'));
    type_chars(&mut app, "cd");
    app.on_key(ctrl('p'));
    assert_eq!(app.input.cursor(), 2, "cursor-up like ↑");
    app.on_key(ctrl('n'));
    assert_eq!(app.input.cursor(), 5, "cursor-down like ↓");
}

#[test]
fn ctrl_p_and_ctrl_n_navigate_the_palette() {
    let mut app = App::new();
    type_chars(&mut app, "/");
    let start = app.command_menu.as_ref().expect("palette open").selected;
    app.on_key(ctrl('n'));
    assert_eq!(
        app.command_menu.as_ref().map(|m| m.selected),
        Some(start + 1)
    );
    app.on_key(ctrl('p'));
    assert_eq!(app.command_menu.as_ref().map(|m| m.selected), Some(start));
}

#[test]
fn ctrl_n_never_lights_the_background_indicator() {
    // ↓'s footer walk (the shell indicator, the agent roster) is the arrow
    // key's own affordance — ctrl+n stays an editing key.
    let mut app = App::new();
    app.bg_started("bash_1", "sleep 99", None, true, None);
    assert!(app.background_focusable());
    assert_eq!(app.on_key(ctrl('n')), Action::None);
    assert!(!app.background_focus, "no indicator walk on ctrl+n");
}
