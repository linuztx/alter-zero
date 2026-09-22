//! The `/export` page and the export file name (`docs/export.md`).

use super::*;

/// An app with a conversation and the `/export` page open.
fn export_app() -> App {
    let mut app = App::new();
    app.record_user_message("hello");
    app.open_export_picker();
    app
}

/// The highlighted row's index.
fn selected(app: &App) -> usize {
    app.export_picker
        .as_ref()
        .expect("the page is open")
        .selected
}

// ===== the /export command =====

#[test]
fn the_palette_lists_export_right_after_copy() {
    // `/export` is `/copy`'s sibling — the whole conversation where `/copy`
    // is the last reply — so it lists beside it.
    let copy = COMMANDS
        .iter()
        .position(|c| c.name == "copy")
        .expect("/copy is registered");
    let cmd = &COMMANDS[copy + 1];
    assert_eq!(cmd.name, "export");
    assert_eq!(
        cmd.description,
        "Copy or save the conversation as plain text"
    );
    assert_eq!(cmd.effect, CommandEffect::Export);
}

#[test]
fn slash_export_opens_the_page_on_a_conversation() {
    let mut app = App::new();
    app.record_user_message("hello");
    type_chars(&mut app, "/export");
    assert!(app.command_menu.is_some());
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::OpenExportPicker);
    assert!(app.export_picker.is_some(), "the pure open happened");
    assert!(app.input.is_empty(), "the /export token was consumed");
    assert!(app.command_menu.is_none(), "the palette closed");
    assert_eq!(selected(&app), 0, "opens on the first row");
    assert_eq!(
        app.highlighted_export_target(),
        Some(ExportTarget::Clipboard),
        "the first row is the clipboard"
    );
}

#[test]
fn slash_export_on_an_empty_conversation_is_a_toast() {
    // Nothing recorded means nothing to export: the `/compact` rule — a
    // soft rejection the user needn't keep, never an empty file.
    let mut app = App::new();
    type_chars(&mut app, "/export");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Toast(EXPORT_EMPTY_NOTICE.to_string())
    );
    assert!(app.export_picker.is_none(), "no page over nothing");
    assert!(app.input.is_empty(), "the /export token was consumed");
    assert_eq!(EXPORT_EMPTY_NOTICE, "Nothing to export");
}

#[test]
fn slash_export_works_mid_turn() {
    // Like every picker it only replaces the composer — the running turn
    // streams on its own thread and keeps its strip above the page; the
    // export then carries the live tail exactly as Ctrl+O shows it.
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_stream();
    app.push_chunk("streaming…");
    type_chars(&mut app, "/export");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenExportPicker);
    assert!(app.export_picker.is_some());
    assert!(app.is_streaming(), "the turn was not touched");
}

#[test]
fn inside_an_agent_view_the_viewed_transcript_is_what_counts() {
    // The main history can be empty while a subagent's session view is up —
    // the export is of the conversation on screen, so the page opens on the
    // agent's own transcript (the `/copy` rule, `docs/copy.md`).
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(
        false,
        &[crate::stream::AgentSpec {
            id: "a1".into(),
            description: "Fetch Warsaw".into(),
            agent_type: "general-purpose".into(),
            prompt: "warsaw?".into(),
            background: false,
            call_id: None,
            arguments: None,
        }],
    );
    app.open_agent_view("a1");
    assert!(app.history.is_empty(), "the lead recorded nothing yet");
    assert!(
        app.export_available(),
        "the viewed agent's transcript has its prompt"
    );
    type_chars(&mut app, "/export");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenExportPicker);
    assert!(app.export_picker.is_some());
}

#[test]
fn opening_abandons_the_bands_that_share_the_composer() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_export_picker();
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none());
    assert!(app.file_search.is_none());
    assert!(app.export_picker.is_some());
}

// ===== the key grammar (the /donate family: navigation, no text entry) =====

#[test]
fn up_and_down_wrap_at_the_ends() {
    let mut app = export_app();
    assert_eq!(selected(&app), 0);
    assert_eq!(app.on_key(key(KeyCode::Down)), Action::None);
    assert_eq!(selected(&app), 1, "↓ reaches the file row");
    assert_eq!(
        app.highlighted_export_target(),
        Some(ExportTarget::File),
        "the second row is the file"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(selected(&app), 0, "↓ past the last wraps to the first");
    app.on_key(key(KeyCode::Up));
    assert_eq!(selected(&app), 1, "↑ from the first wraps to the last");
}

#[test]
fn home_and_end_jump() {
    let mut app = export_app();
    app.on_key(key(KeyCode::End));
    assert_eq!(selected(&app), 1);
    app.on_key(key(KeyCode::Home));
    assert_eq!(selected(&app), 0);
}

#[test]
fn enter_picks_the_highlighted_target_and_closes_the_page() {
    // The choice is the whole point of the page, so unlike `/donate` (where
    // a second address is one key away) it closes on the pick.
    let mut app = export_app();
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Export(ExportTarget::Clipboard)
    );
    assert!(app.export_picker.is_none(), "the pick closes the page");

    let mut app = export_app();
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Export(ExportTarget::File)
    );
    assert!(app.export_picker.is_none());
}

#[test]
fn a_digit_jumps_to_that_row_and_picks_it() {
    let mut app = export_app();
    assert_eq!(
        app.on_key(key(KeyCode::Char('2'))),
        Action::Export(ExportTarget::File)
    );
    assert!(
        app.export_picker.is_none(),
        "the digit's pick closes the page"
    );

    let mut app = export_app();
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.on_key(key(KeyCode::Char('1'))),
        Action::Export(ExportTarget::Clipboard)
    );
    assert!(app.export_picker.is_none());

    // A digit past the rows names nothing and is ignored.
    let mut app = export_app();
    assert_eq!(app.on_key(key(KeyCode::Char('3'))), Action::None);
    assert_eq!(app.on_key(key(KeyCode::Char('9'))), Action::None);
    assert!(
        app.export_picker.is_some(),
        "an ignored digit keeps the page"
    );
    assert_eq!(selected(&app), 0);
}

#[test]
fn esc_and_ctrl_c_close_the_page_without_quitting() {
    let mut app = export_app();
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseExportPicker);
    assert!(app.export_picker.is_none());

    let mut app = export_app();
    assert_eq!(app.on_key(ctrl('c')), Action::CloseExportPicker);
    assert!(
        app.export_picker.is_none(),
        "Ctrl+C closes the page, never the app"
    );
}

#[test]
fn the_page_owns_every_other_key() {
    // No text entry: a printable key never reaches the composer draft
    // underneath, and the global Ctrl+O never opens the overlay over it.
    let mut app = export_app();
    for code in [
        KeyCode::Char('x'),
        KeyCode::Char('c'),
        KeyCode::Char(' '),
        KeyCode::Backspace,
        KeyCode::Tab,
        KeyCode::Left,
        KeyCode::Right,
    ] {
        assert_eq!(app.on_key(key(code)), Action::None, "{code:?}");
        assert!(app.export_picker.is_some(), "{code:?} closed the page");
    }
    assert_eq!(app.on_key(ctrl('o')), Action::None);
    assert_eq!(app.view, View::Conversation, "Ctrl+O is owned while open");
    assert!(app.input.is_empty(), "nothing leaked into the composer");
    assert_eq!(selected(&app), 0);
}

#[test]
fn an_open_page_suppresses_the_ctrl_b_hint_clock_and_asks_no_animation() {
    // The running cell it keeps visible above itself must not advertise a
    // Ctrl+B the page would swallow (docs/background.md) — and the page is
    // still, so it never re-arms the animation chain on its own.
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_stream();
    app.start_tool("Bash", "sleep 100", None);
    app.set_command_elapsed(Some(Duration::from_secs(5)));
    assert!(app.background_hint_elapsed().is_some());
    app.open_export_picker();
    assert_eq!(
        app.background_hint_elapsed(),
        None,
        "the page swallows Ctrl+B"
    );

    let app = export_app();
    assert!(
        !app.wants_animation_frames(),
        "a still page with no turn running asks for no frames"
    );
}

// ===== the file name =====

#[test]
fn the_export_file_name_is_conversation_date_time_txt() {
    // `conversation-YYYY-MM-DD-HHMMSS.txt`: the date as ISO, the time as one
    // run of digits, every field zero-padded so the names sort as they were
    // written.
    assert_eq!(
        export_file_name((2026, 9, 22), (9, 7, 20), 0),
        "conversation-2026-09-22-090720.txt"
    );
    assert_eq!(
        export_file_name((2026, 12, 3), (23, 59, 5), 0),
        "conversation-2026-12-03-235905.txt"
    );
}

#[test]
fn a_taken_name_gets_a_numbered_suffix() {
    // Two exports inside one second must not overwrite each other: the
    // boundary steps `dup` past every name already on disk, and the second
    // file says it is the second.
    assert_eq!(
        export_file_name((2026, 9, 22), (9, 7, 20), 1),
        "conversation-2026-09-22-090720-2.txt"
    );
    assert_eq!(
        export_file_name((2026, 9, 22), (9, 7, 20), 2),
        "conversation-2026-09-22-090720-3.txt"
    );
}
