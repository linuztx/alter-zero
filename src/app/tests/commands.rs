//! The slash-command palette and its effects (`/help`, `/clear`, `/copy`,
//! `/init` — `docs/init.md`).

use super::*;

#[test]
fn diff_command_is_available_for_review() {
    let app = App::new();
    let commands = app.commands();
    let matches = matching_commands(&commands, "diff");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].name, "diff");
}

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
        assert!(
            seen.insert(c.name.as_ref()),
            "duplicate command /{}",
            c.name
        );
    }
}

#[test]
fn palette_descriptions_are_concise_and_product_name_free() {
    // The palette's descriptions never carry the product name — "Exit
    // alter-zero" said nothing "Exit the app" doesn't — and stay inside the
    // 55 columns a standard 80-column terminal leaves past the description
    // column, so the default view shows each command on one row (narrower
    // terminals wrap instead of clipping — `ui::command_menu_lines`).
    for cmd in COMMANDS {
        assert!(
            !cmd.description.to_lowercase().contains("alter-zero"),
            "/{} mentions the product name: {:?}",
            cmd.name,
            cmd.description
        );
        assert!(
            cmd.description.len() <= 55,
            "/{}'s description outgrows the 80-column room: {:?}",
            cmd.name,
            cmd.description
        );
    }
    let desc = |name: &str| {
        COMMANDS
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("/{name} is registered"))
            .description
            .as_ref()
    };
    assert_eq!(desc("quit"), "Exit the app");
    assert_eq!(desc("trust"), "Review and approve this project's config");
}

#[test]
fn matching_commands_filters_by_name_prefix_case_insensitively() {
    assert_eq!(
        matching_commands(COMMANDS, "").len(),
        COMMANDS.len(),
        "an empty query lists everything"
    );
    let hits = matching_commands(COMMANDS, "HE");
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
fn the_palette_lists_init_with_a_concise_description() {
    // Codex's description carried the product name ("…instructions for
    // alter-zero"); the palette keeps its descriptions concise and
    // product-name-free.
    let init = COMMANDS
        .iter()
        .position(|c| c.name == "init")
        .expect("/init is registered");
    let cmd = &COMMANDS[init];
    assert_eq!(cmd.description, "Create an AGENTS.md contributor guide");
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

// ===== the per-tier speed commands (docs/fast-mode.md) =====

fn fast_tier() -> ServiceTier {
    ServiceTier::new("priority", "Fast", "1.5x speed, increased usage")
}

fn ultrafast_tier() -> ServiceTier {
    ServiceTier::new("ultrafast", "Ultrafast", "The fastest available responses.")
}

fn two_tiers() -> Option<SpeedState> {
    SpeedState::new(vec![fast_tier(), ultrafast_tier()], None)
}

/// The palette rows that are speed tiers, by name.
fn tier_rows(app: &App) -> Vec<String> {
    app.commands()
        .iter()
        .filter(|c| matches!(c.effect, CommandEffect::ServiceTier(_)))
        .map(|c| c.name.to_string())
        .collect()
}

#[test]
fn the_registry_has_no_static_fast_command() {
    // Fast mode is a per-tier command derived from the model's listing
    // (codex's `SlashCommandItem::ServiceTier`): a model that lists no tier
    // gets no row, so the palette never offers a speed it cannot switch to.
    assert!(
        COMMANDS.iter().all(|c| c.name != "fast"),
        "the static registry carries no /fast"
    );
    assert!(
        !COMMANDS
            .iter()
            .any(|c| matches!(c.effect, CommandEffect::ServiceTier(_))),
        "no tier is hardcoded"
    );
    let app = App::new();
    assert_eq!(
        app.commands().len(),
        COMMANDS.len(),
        "no tier listed, no tier row"
    );
    assert!(tier_rows(&app).is_empty());
    assert!(matching_commands(&app.commands(), "fast").is_empty());
}

#[test]
fn every_listed_tier_becomes_a_command_right_after_model() {
    // Codex inserts its tier commands directly after /model, one per tier
    // the record lists, in the record's order; the row's description is
    // the backend's own cost statement for the tier.
    let mut app = App::new();
    app.set_speed(two_tiers());
    let commands = app.commands();
    let names: Vec<&str> = commands.iter().map(|c| c.name.as_ref()).collect();
    let model = names.iter().position(|n| *n == "model").expect("/model");
    assert_eq!(&names[model + 1..model + 3], ["fast", "ultrafast"]);
    assert_eq!(names.len(), COMMANDS.len() + 2);
    let fast = &commands[model + 1];
    assert_eq!(fast.description, "1.5x speed, increased usage");
    assert_eq!(fast.effect, CommandEffect::ServiceTier(fast_tier()));
    let ultrafast = &commands[model + 2];
    assert_eq!(ultrafast.description, "The fastest available responses.");
    assert_eq!(
        ultrafast.effect,
        CommandEffect::ServiceTier(ultrafast_tier())
    );
    // The filter sees them like any other row.
    let ultra: Vec<&SlashCommand> = matching_commands(&commands, "ULTRA");
    assert_eq!(ultra.len(), 1);
    assert_eq!(ultra[0].name, "ultrafast");
    // A switch to a model listing none takes the rows away again.
    app.set_speed(None);
    assert!(tier_rows(&app).is_empty());
    assert!(matching_commands(&app.commands(), "ultra").is_empty());
}

#[test]
fn a_tier_row_degrades_gracefully_on_a_record_that_says_less() {
    // A tier the record did not describe gets a generic description rather
    // than an empty column; a tier whose name cannot be a palette token
    // gets no row; a tier named after a built-in command never shadows it
    // (the built-in is the door to everything else).
    let bare = ServiceTier::new("priority", "Fast", "");
    let unusable = ServiceTier::new("x", "!!!", "");
    let clash = ServiceTier::new("y", "Model", "a tier that would shadow /model");
    let mut app = App::new();
    app.set_speed(SpeedState::new(vec![bare.clone(), unusable, clash], None));
    assert_eq!(tier_rows(&app), ["fast"]);
    let commands = app.commands();
    let fast = commands.iter().find(|c| c.name == "fast").unwrap();
    assert_eq!(fast.description, "Toggle fast mode");
    assert_eq!(fast.effect, CommandEffect::ServiceTier(bare));
    assert_eq!(
        commands.iter().filter(|c| c.name == "model").count(),
        1,
        "/model is listed once, and it is the built-in"
    );
    assert_eq!(
        commands.iter().find(|c| c.name == "model").unwrap().effect,
        CommandEffect::Model
    );
}

#[test]
fn set_speed_seeds_and_clears_the_state() {
    let mut app = App::new();
    assert!(app.speed.is_none(), "unknown/unsupported by default");
    app.set_speed(two_tiers());
    let state = app.speed.as_ref().expect("seeded");
    assert_eq!(state.tiers, vec![fast_tier(), ultrafast_tier()]);
    assert_eq!(
        state.tier, None,
        "standard until a tier command says otherwise"
    );
    app.set_speed(None);
    assert!(
        app.speed.is_none(),
        "a switch to a model listing no tier clears it"
    );
}

#[test]
fn a_tier_command_toggles_its_tier_and_hands_the_loop_the_selection() {
    // Codex's `toggle_service_tier_from_ui`: the command's tier when it is
    // not the selection, standard when it is — and another tier's command
    // switches straight to that tier, no standard step between.
    let mut app = App::new();
    app.set_speed(two_tiers());
    type_chars(&mut app, "/ultrafast");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::SetSpeed(Some(ultrafast_tier())),
        "standard steps straight to ultrafast"
    );
    assert_eq!(
        app.speed.as_ref().unwrap().tier.as_deref(),
        Some("ultrafast")
    );
    assert_eq!(app.input.text(), "", "the command is consumed");
    assert!(app.command_menu.is_none(), "and the palette closed");
    type_chars(&mut app, "/fast");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::SetSpeed(Some(fast_tier())),
        "ultrafast → fast directly"
    );
    type_chars(&mut app, "/fast");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::SetSpeed(None),
        "the selected tier's own command is the way back to standard"
    );
    assert_eq!(app.speed.as_ref().unwrap().tier, None);
}

#[test]
fn a_tier_command_typed_on_a_model_listing_none_matches_nothing() {
    // No row, no run: the query matches no command, Enter is swallowed like
    // any other miss, and `/fast` is never sent to the model as a message.
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/repo");
    type_chars(&mut app, "/fast");
    assert!(app.command_menu.is_some(), "the palette is open");
    assert!(app.highlighted_command().is_none());
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(app.input.text(), "/fast", "the draft stays");
    assert!(app.speed.is_none());
}

#[test]
fn a_tier_command_works_mid_turn_for_the_next_turn() {
    // Like /model and Ctrl+T, the switch never touches the running turn —
    // the tier simply rides the next request.
    let mut app = App::new();
    app.set_speed(two_tiers());
    app.begin_stream();
    type_chars(&mut app, "/fast");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::SetSpeed(Some(fast_tier()))
    );
    assert!(app.turn_active(), "the turn keeps running underneath");
}

#[test]
fn slash_help_lists_the_tier_commands_where_the_palette_shows_them() {
    // /help enumerates what the palette offers this session — the tier rows
    // included, right after /model, each with its own description.
    let mut app = App::new();
    app.set_speed(two_tiers());
    type_chars(&mut app, "/help");
    let Action::Notice(text) = app.on_key(key(KeyCode::Enter)) else {
        panic!("/help commits a notice when idle");
    };
    let lines: Vec<&str> = text.lines().collect();
    let model = lines
        .iter()
        .position(|l| l.starts_with("/model — "))
        .expect("/model listed");
    assert_eq!(lines[model + 1], "/fast — 1.5x speed, increased usage");
    assert_eq!(
        lines[model + 2],
        "/ultrafast — The fastest available responses."
    );
    // …and a model listing none lists none.
    let mut plain = App::new();
    type_chars(&mut plain, "/help");
    let Action::Notice(text) = plain.on_key(key(KeyCode::Enter)) else {
        panic!("/help commits a notice when idle");
    };
    assert!(!text.contains("/fast"), "{text}");
}
