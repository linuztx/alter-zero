//! ↑/↓ input recall and the Ctrl+R reverse search over it
//! (`docs/input-history.md`, `docs/history-search.md`).

use super::*;

#[test]
fn up_recalls_the_last_submitted_message() {
    let mut app = App::new();
    submit(&mut app, "first message");
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.input.text(), "first message");
    assert_eq!(
        app.input.cursor(),
        "first message".len(),
        "recall puts the cursor at the end (codex's placement)"
    );
}

#[test]
fn down_when_not_browsing_does_not_recall() {
    let mut app = App::new();
    submit(&mut app, "sent");
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.input.text(),
        "",
        "down from an empty composer never recalls — only up enters history"
    );
}

#[test]
fn recall_resumes_only_from_the_text_edges() {
    let mut app = App::new();
    submit(&mut app, "first");
    submit(&mut app, "second");
    app.on_key(key(KeyCode::Up)); // recall "second", cursor at the end
    app.on_key(key(KeyCode::Left)); // cursor now inside the text
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.input.text(),
        "second",
        "an interior cursor means cursor movement, not history"
    );
    app.on_key(key(KeyCode::End)); // back to an edge
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.input.text(), "first", "an edge cursor browses again");
}

#[test]
fn adjacent_duplicate_submissions_collapse_in_history() {
    let mut app = App::new();
    submit(&mut app, "same");
    submit(&mut app, "same");
    assert_eq!(
        app.input_history.entries,
        ["same"],
        "an entry identical to the newest is not re-recorded (codex)"
    );
}

#[test]
fn blank_texts_are_never_recorded() {
    let mut history = InputHistory::default();
    history.record("");
    assert_eq!(history.up(), None, "nothing to recall");
}

#[test]
fn seed_populates_entries_without_queuing_them() {
    let mut app = App::new();
    app.seed_input_history(vec!["old-a".to_string(), "old-b".to_string()]);
    assert!(
        app.take_unpersisted_inputs().is_empty(),
        "seeded entries are already on disk — never re-queued"
    );
    // Both recall and search see the seeded entries.
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.input.text(),
        "old-b",
        "↑ recalls the newest seeded entry"
    );
    assert_eq!(
        app.input_history.search("old"),
        vec![1, 0],
        "Ctrl+R search spans the seeded (cross-session) entries"
    );
}

#[test]
fn seed_collapses_adjacent_duplicates_like_record() {
    // A messy or concurrently-written file can carry adjacent duplicates;
    // seeding replays them through record's collapse, so the buffer looks
    // exactly as a fresh session would build it.
    let mut app = App::new();
    app.seed_input_history(vec!["a".to_string(), "a".to_string(), "b".to_string()]);
    assert_eq!(
        app.input_history.entries,
        ["a", "b"],
        "seed collapses adjacent duplicates like record"
    );
}

#[test]
fn a_seeded_duplicate_of_the_first_submission_is_not_re_persisted() {
    // Seed the last entry, then submit the same text: record collapses it
    // (adjacent duplicate) and queues nothing, so it isn't written twice.
    let mut app = App::new();
    app.seed_input_history(vec!["repeat".to_string()]);
    submit(&mut app, "repeat");
    assert!(
        app.take_unpersisted_inputs().is_empty(),
        "a submission identical to the newest seeded entry is not re-persisted"
    );
}

#[test]
fn ctrl_c_cleared_draft_is_recallable_with_up() {
    // codex's clear_for_ctrl_c records the cleared draft so ↑ undoes it.
    let mut app = App::new();
    app.input = TextArea::from_text("draft the user cleared");
    app.on_key(ctrl('c'));
    assert!(app.input.is_empty());
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.input.text(), "draft the user cleared");
}

#[test]
fn recalling_a_slash_token_reopens_the_palette() {
    let mut app = App::new();
    type_str(&mut app, "/he");
    assert!(app.command_menu.is_some());
    app.on_key(ctrl('c')); // clear (and record) the token, palette closes
    assert!(app.command_menu.is_none());
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.input.text(), "/he");
    assert!(
        app.command_menu.is_some(),
        "a recalled /token re-derives the palette, like typing it"
    );
}

#[test]
fn clear_command_keeps_the_recall_history() {
    let mut app = App::new();
    submit(&mut app, "kept across clear");
    type_str(&mut app, "/clear");
    app.on_key(key(KeyCode::Enter)); // run /clear (wipes the conversation)
    assert!(app.history.is_empty());
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.input.text(),
        "kept across clear",
        "/clear wipes the conversation, not the composer's recall"
    );
}

#[test]
fn up_recall_closes_the_band_and_still_recalls() {
    let mut app = App::new();
    submit(&mut app, "recall me");
    app.on_key(key(KeyCode::Char('?')));
    app.on_key(key(KeyCode::Up));
    assert!(!app.shortcuts_open);
    assert_eq!(app.input.text(), "recall me");
}

#[test]
fn tab_queued_message_is_recorded_for_up_recall() {
    // Like Enter, a Tab-queued message is recorded so ↑ brings it back.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("tabbed");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.input.text(), "");
    app.on_key(key(KeyCode::Up)); // empty composer → history recall
    assert_eq!(app.input.text(), "tabbed");
}

#[test]
fn a_queued_message_is_recorded_for_up_recall() {
    // Queueing records into input_history (like a submit), so ↑ brings it back.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("queued line");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.input.text(), "");
    app.on_key(key(KeyCode::Up)); // empty composer → history recall
    assert_eq!(app.input.text(), "queued line");
}

#[test]
fn slash_init_does_not_enter_the_up_arrow_recall_history() {
    // Palette commands never record into the ↑ recall history — the
    // composer held "/init", not the canned prompt, and recalling a
    // 40-line prompt the user never typed would be noise.
    let mut app = App::new();
    type_str(&mut app, "/init");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Up));
    assert!(app.input.is_empty(), "nothing to recall");
}

#[test]
fn search_with_no_match_or_no_entries_is_empty() {
    assert_eq!(history_of(&["alpha"]).search("zzz"), Vec::<usize>::new());
    assert_eq!(InputHistory::default().search("a"), Vec::<usize>::new());
}

#[test]
fn ctrl_r_opens_an_idle_search_without_previewing() {
    let mut app = searchable_app(&["git status"]);
    assert_eq!(app.on_key(ctrl('r')), Action::None);
    let search = app.history_search.as_ref().expect("search open");
    assert_eq!(search.query, "");
    assert_eq!(search.state, SearchState::Idle);
    assert_eq!(app.input.text(), "", "no preview until a query is typed");
}

#[test]
fn ctrl_r_steps_older_and_clamps_at_the_oldest_match() {
    let mut app = searchable_app(&["git status", "cargo build", "git push"]);
    app.on_key(ctrl('r'));
    type_query(&mut app, "git");
    app.on_key(ctrl('r'));
    assert_eq!(app.input.text(), "git status");
    // At the boundary the match is kept (codex's AtBoundary — no flicker).
    app.on_key(ctrl('r'));
    assert_eq!(app.input.text(), "git status");
    let search = app.history_search.as_ref().expect("search open");
    assert_eq!(search.state, SearchState::Match { selected: 1 });
}

#[test]
fn backspace_pops_the_query_and_restarts_from_the_newest() {
    let mut app = searchable_app(&["git status", "git push"]);
    app.on_key(ctrl('r'));
    type_query(&mut app, "gitz");
    let search = app.history_search.as_ref().expect("search open");
    assert_eq!(search.state, SearchState::NoMatch);
    app.on_key(key(KeyCode::Backspace));
    let search = app.history_search.as_ref().expect("search open");
    assert_eq!(search.query, "git");
    assert_eq!(search.state, SearchState::Match { selected: 0 });
    assert_eq!(app.input.text(), "git push");
}

#[test]
fn a_no_match_query_restores_the_draft_and_keeps_the_search_open() {
    let mut app = searchable_app(&["git status"]);
    app.input = TextArea::from_text("a draft");
    app.on_key(ctrl('r'));
    type_query(&mut app, "zzz");
    let search = app.history_search.as_ref().expect("search stays open");
    assert_eq!(search.state, SearchState::NoMatch);
    assert_eq!(app.input.text(), "a draft");
}

#[test]
fn searching_with_no_history_shows_no_match() {
    let mut app = App::new();
    app.on_key(ctrl('r'));
    type_query(&mut app, "a");
    let search = app.history_search.as_ref().expect("search open");
    assert_eq!(search.state, SearchState::NoMatch);
}

#[test]
fn highlight_ranges_are_empty_unless_a_match_is_previewed() {
    let mut app = searchable_app(&["git status"]);
    assert!(app.search_highlight_ranges().is_empty(), "no search open");
    app.on_key(ctrl('r'));
    assert!(app.search_highlight_ranges().is_empty(), "Idle");
    type_query(&mut app, "zzz");
    assert!(app.search_highlight_ranges().is_empty(), "NoMatch");
    type_query(&mut app, ""); // unchanged
    app.on_key(ctrl('u'));
    type_query(&mut app, "git");
    app.on_key(key(KeyCode::Enter));
    assert!(
        app.search_highlight_ranges().is_empty(),
        "accepted = plain draft"
    );
}

#[test]
fn recalling_a_bang_entry_restores_shell_mode() {
    let mut app = App::new();
    type_query(&mut app, "!echo hello");
    app.on_key(key(KeyCode::Enter));
    // ↑ recalls the recorded "!echo hello" — absorbed back into the mode
    // (codex records the full text and re-absorbs it on recall).
    app.on_key(key(KeyCode::Up));
    assert!(app.shell_mode);
    assert_eq!(app.input.text(), "echo hello");
}

#[test]
fn recalling_a_plain_entry_clears_shell_mode() {
    let mut app = App::new();
    submit(&mut app, "hello there");
    type_query(&mut app, "!");
    app.on_key(key(KeyCode::Up));
    assert!(
        !app.shell_mode,
        "a recalled plain message replaces the mode"
    );
    assert_eq!(app.input.text(), "hello there");
}

// ===== audited-defect regressions (2026-07 review) =====

#[test]
fn history_browsing_steps_past_a_recalled_shell_entry() {
    // Recalling "!ls" absorbs the bang into shell_mode (composer "ls"),
    // but the recorded entry is "!ls" — the unedited-recall comparison
    // must account for the absorbed bang or browsing strands on the
    // shell entry (docs/input-history.md: unedited recalls keep browsing).
    let mut app = App::new();
    submit(&mut app, "hello");
    type_query(&mut app, "!ls");
    app.on_key(key(KeyCode::Enter)); // records "!ls"
    app.on_key(key(KeyCode::Up)); // recall "!ls" → shell mode, text "ls"
    assert!(app.shell_mode);
    assert_eq!(app.input.text(), "ls");
    app.on_key(key(KeyCode::Up)); // must step OLDER, not move the cursor
    assert_eq!(app.input.text(), "hello");
    assert!(!app.shell_mode, "the older plain entry leaves the mode");
    app.on_key(key(KeyCode::Down)); // and ↓ steps back to the shell entry
    assert!(app.shell_mode);
    assert_eq!(app.input.text(), "ls");
}

#[test]
fn down_past_a_recalled_shell_entry_clears_the_composer() {
    let mut app = App::new();
    type_query(&mut app, "!ls");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Up)); // recall the newest ("!ls")
    assert!(app.shell_mode);
    app.on_key(key(KeyCode::Down)); // ↓ past the newest clears + exits
    assert_eq!(app.input.text(), "");
    assert!(!app.shell_mode);
}
