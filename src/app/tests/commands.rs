//! The slash-command palette and its effects (`/help`, `/clear`, `/copy`,
//! `/init` — `docs/init.md`).

use super::*;

#[test]
fn a_slash_closes_the_band_and_opens_the_palette() {
    // The band and the palette never show together.
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('?')));
    app.on_key(key(KeyCode::Char('/')));
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_some());
    assert_eq!(app.input.text(), "/");
}

#[test]
fn command_query_detects_a_bare_slash_token() {
    assert_eq!(command_query("/"), Some(""));
    assert_eq!(command_query("/he"), Some("he"));
    assert_eq!(command_query("/help"), Some("help"));
}

#[test]
fn command_query_is_none_for_non_command_input() {
    assert_eq!(command_query(""), None);
    assert_eq!(command_query("hello"), None);
    assert_eq!(command_query("ask /help"), None); // not at the start
    assert_eq!(command_query("/help me"), None); // past a space → args, not a token
    assert_eq!(command_query("/a\nb"), None); // a newline ends the token too
}

#[test]
fn the_command_registry_is_non_empty_with_unique_lowercase_names() {
    assert!(!COMMANDS.is_empty());
    let mut seen = std::collections::HashSet::new();
    for c in COMMANDS {
        assert!(!c.name.is_empty());
        assert!(
            !c.name.starts_with('/'),
            "names are stored without the slash"
        );
        assert_eq!(c.name, c.name.to_lowercase(), "names are lowercase");
        assert!(seen.insert(c.name), "duplicate command /{}", c.name);
    }
}

#[test]
fn matching_commands_filters_by_name_prefix_case_insensitively() {
    assert_eq!(
        matching_commands("").len(),
        COMMANDS.len(),
        "an empty query lists everything"
    );
    let hits = matching_commands("HE");
    assert!(hits.iter().any(|c| c.name == "help"));
    assert!(
        hits.iter().all(|c| c.name.starts_with("he")),
        "every match shares the prefix"
    );
}

#[test]
fn typing_a_slash_opens_the_command_palette_at_the_top() {
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('/')));
    let menu = app.command_menu.as_ref().expect("the palette is open");
    assert_eq!(menu.selected, 0);
}

#[test]
fn backspacing_the_slash_closes_the_palette() {
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('/')));
    assert!(app.command_menu.is_some());
    app.on_key(key(KeyCode::Backspace));
    assert!(app.command_menu.is_none(), "removing the slash closes it");
}

#[test]
fn esc_closes_the_palette_instead_of_quitting() {
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('/')));
    assert_eq!(
        app.on_key(key(KeyCode::Esc)),
        Action::None,
        "esc dismisses the palette, it does not quit"
    );
    assert!(app.command_menu.is_none());
}

#[test]
fn leaving_and_re_entering_command_mode_reopens_the_palette() {
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('/')));
    app.on_key(key(KeyCode::Esc)); // dismissed
    app.on_key(key(KeyCode::Backspace)); // delete '/', input now empty
    assert!(app.command_menu.is_none());
    app.on_key(key(KeyCode::Char('/'))); // re-enter command mode
    assert!(
        app.command_menu.is_some(),
        "re-entering reopens the palette"
    );
}

// --- /init (docs/init.md) ---

#[test]
fn the_palette_lists_init_with_codexs_description() {
    let init = COMMANDS
        .iter()
        .position(|c| c.name == "init")
        .expect("/init is registered");
    let cmd = &COMMANDS[init];
    assert_eq!(
        cmd.description,
        "create an AGENTS.md file with instructions for alter-zero"
    );
    assert_eq!(cmd.effect, CommandEffect::Init);
    // Codex's palette adjacency: Init lists immediately before Compact
    // (its enum order) — the documented order, pinned like MENU_MAX_ROWS.
    let compact = COMMANDS
        .iter()
        .position(|c| c.name == "compact")
        .expect("/compact is registered");
    assert_eq!(init + 1, compact, "/init lists right before /compact");
}

#[test]
fn slash_init_submits_the_canned_prompt_when_idle() {
    // Codex's /init is submit_user_message(INIT_PROMPT): the whole canned
    // prompt goes out as a regular user turn — echoed as the ❯ message,
    // recorded, checkpointed — and the model's tool loop does the work.
    // Trailing whitespace is trimmed: the file's final newline would wrap
    // into an empty last line that message_lines pads into a stray
    // full-width dark row under the ❯ cell (codex trims the same way at
    // render time, its display_lines' trim_end_matches).
    let mut app = App::new();
    type_str(&mut app, "/init");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit(INIT_PROMPT.trim_end().to_string())
    );
    assert!(app.input.is_empty());
    assert!(app.command_menu.is_none());
}

#[test]
fn opening_the_search_closes_the_palette_and_canceling_keeps_it_closed() {
    let mut app = searchable_app(&["git status"]);
    app.input = TextArea::new();
    type_query(&mut app, "/he"); // typing a bare /token opens the palette
    assert!(app.command_menu.is_some());
    app.on_key(ctrl('r'));
    assert!(app.command_menu.is_none(), "search owns the keys");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.input.text(), "/he");
    assert!(app.command_menu.is_none(), "cancel restores the draft only");
}

#[test]
fn a_previewed_slash_token_does_not_open_the_palette_but_accepting_does() {
    let mut app = App::new();
    // A bare /token can enter the history via Ctrl+C's composer-clear.
    app.input = TextArea::from_text("/help");
    app.on_key(ctrl('c'));
    app.on_key(ctrl('r'));
    type_query(&mut app, "help");
    assert_eq!(app.input.text(), "/help");
    assert!(app.command_menu.is_none(), "previews never pop the palette");
    app.on_key(key(KeyCode::Enter));
    assert!(
        app.command_menu.is_some(),
        "accepting re-derives the palette, like ↑-recall"
    );
}

#[test]
fn the_palette_never_opens_in_shell_mode() {
    let mut app = App::new();
    type_query(&mut app, "!/he");
    assert!(app.shell_mode);
    assert_eq!(app.input.text(), "/he");
    assert!(
        app.command_menu.is_none(),
        "a /token inside a shell command is literal"
    );
}

#[test]
fn slash_resume_mid_turn_is_rejected_with_a_toast() {
    // Codex blocks /resume while a task runs (it swaps the whole
    // conversation) — the picker never opens over an active turn, and the
    // rejection is a transient toast, not a scrollback bullet.
    let mut app = App::new();
    app.begin_stream();
    type_chars(&mut app, "/resume");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Toast(RESUME_BUSY_NOTICE.to_string()),
    );
    assert_eq!(app.view, View::Conversation);
    assert!(app.resume_picker.is_none());
}

#[test]
fn opening_the_picker_abandons_the_palette_and_shortcuts() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_model_picker("m1");
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none());
}
