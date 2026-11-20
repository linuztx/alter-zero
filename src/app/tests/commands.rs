//! The slash-command palette and its effects (`/clear`, `/copy`, `/init` —
//! `docs/init.md`).

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
    assert_eq!(command_query("/cl"), Some("cl"));
    assert_eq!(command_query("/clear"), Some("clear"));
}

#[test]
fn command_query_is_none_for_non_command_input() {
    assert_eq!(command_query(""), None);
    assert_eq!(command_query("hello"), None);
    assert_eq!(command_query("ask /clear"), None); // not at the start
    assert_eq!(command_query("/clear me"), None); // past a space → args, not a token
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
    // 53 columns a standard 80-column terminal leaves past the description
    // column (the `→` marker's two and the name column's 25 come first), so
    // the default view shows each command on one row (narrower terminals
    // wrap instead of clipping — `ui::command_menu_lines`).
    for cmd in COMMANDS {
        assert!(
            !cmd.description.to_lowercase().contains("alter-zero"),
            "/{} mentions the product name: {:?}",
            cmd.name,
            cmd.description
        );
        assert!(
            cmd.description.len() <= 53,
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
    // The hooks browser is also where a hook's trust changes
    // (docs/config-review.md), so it says so in the palette.
    assert_eq!(
        desc("hooks"),
        "Browse hooks and trust or stop trusting each one"
    );
    // `/trust` is gone: hooks and project MCP servers are asked about at
    // startup instead, and managed in `/hooks` and `/mcp`
    // (docs/config-review.md).
    assert!(
        COMMANDS.iter().all(|c| c.name != "trust"),
        "/trust is no longer a command"
    );
    // `/login` has three ways in, and a description naming only a key sends
    // someone with a local server looking for one (docs/local-models.md).
    assert_eq!(
        desc("login"),
        "Sign in, add an API key, or connect a local model"
    );
}

#[test]
fn matching_commands_filters_by_name_prefix_case_insensitively() {
    assert_eq!(
        matching_commands(COMMANDS, "").len(),
        COMMANDS.len(),
        "an empty query lists everything"
    );
    let hits = matching_commands(COMMANDS, "CO");
    assert!(hits.iter().any(|c| c.name == "copy"));
    assert!(
        hits.iter().all(|c| c.name.starts_with("co")),
        "every match shares the prefix"
    );
}

#[test]
fn typing_a_slash_opens_the_command_palette_at_the_top() {
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('/')));
    let menu = app.command_menu.as_ref().expect("the palette is open");
    assert_eq!(menu.selected, 0);
    assert_eq!(
        app.highlighted_command().map(|c| c.name.into_owned()),
        Some("clear".to_string()),
        "the list opens on /clear"
    );
}

#[test]
fn help_is_no_longer_a_command() {
    // The palette is the command list: every command, what it does, and a
    // `(n/total)` counter saying how many there are. `/help` only printed
    // the same list into scrollback, so it is gone — the registry carries no
    // row for it, and typing it matches nothing, so Enter is swallowed like
    // any other miss rather than sent to the model as a message.
    assert!(
        COMMANDS.iter().all(|c| c.name != "help"),
        "/help is no longer a command"
    );
    let mut app = App::new();
    type_chars(&mut app, "/help");
    assert!(app.command_menu.is_some(), "the palette is open");
    assert!(app.highlighted_command().is_none(), "and matches nothing");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(app.input.text(), "/help", "the draft stays");
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
    app.input = TextArea::from_text("/clear");
    app.on_key(ctrl('c'));
    app.on_key(ctrl('r'));
    type_query(&mut app, "clear");
    assert_eq!(app.input.text(), "/clear");
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

// ===== plugin commands (`/plugin:command`, docs/plugins.md) =====

fn plugin_row(name: &str, description: &str, hint: Option<&str>) -> PluginCommandRow {
    PluginCommandRow {
        name: name.to_string(),
        description: description.to_string(),
        argument_hint: hint.map(str::to_string),
        takes_arguments: hint.is_some(),
    }
}

/// An app with two plugin commands: one that takes arguments, one that
/// doesn't (the boundary's `set_plugin_commands` injection).
fn plugin_app() -> App {
    let mut app = App::new();
    app.set_plugin_commands(vec![
        plugin_row("code-review:commit", "Commit the work", None),
        plugin_row("code-review:review", "Review a PR", Some("[pr]")),
    ]);
    app
}

#[test]
fn no_plugin_commands_leave_the_palette_exactly_the_built_ins() {
    let app = App::new();
    let names: Vec<String> = app.commands().iter().map(|c| c.name.to_string()).collect();
    let builtins: Vec<String> = COMMANDS.iter().map(|c| c.name.to_string()).collect();
    assert_eq!(names, builtins);
}

#[test]
fn plugin_commands_list_after_every_built_in_quit_included() {
    let app = plugin_app();
    let commands = app.commands();
    let names: Vec<&str> = commands.iter().map(|c| c.name.as_ref()).collect();
    let n = names.len();
    assert_eq!(
        names[n - 3..],
        ["quit", "code-review:commit", "code-review:review"]
    );
    assert_eq!(n, COMMANDS.len() + 2);
    assert_eq!(commands[n - 2].description, "Commit the work");
    assert_eq!(commands[n - 1].description, "Review a PR [pr]");
    assert_eq!(
        commands[n - 1].effect,
        CommandEffect::PluginPrompt {
            takes_arguments: true
        }
    );
}

#[test]
fn a_hint_written_as_prose_is_bracketed_so_it_reads_as_a_hint() {
    // `argument-hint: Optional specific behavior to address` ran straight on
    // from the description; one already in `[…]` or `<…>` is left as written.
    let mut app = App::new();
    app.set_plugin_commands(vec![
        plugin_row("p:prose", "Create hooks", Some("Optional behavior")),
        plugin_row("p:square", "Review a PR", Some("[pr-number]")),
        plugin_row("p:angle", "Open a file", Some("<path>")),
        plugin_row("p:alone", "", Some("Optional behavior")),
        plugin_row("p:blank", "Do it", Some("  ")),
    ]);
    let commands = app.commands();
    let description = |name: &str| -> String {
        commands
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.description.to_string())
            .unwrap_or_default()
    };
    assert_eq!(description("p:prose"), "Create hooks [Optional behavior]");
    assert_eq!(description("p:square"), "Review a PR [pr-number]");
    assert_eq!(description("p:angle"), "Open a file <path>");
    assert_eq!(description("p:alone"), "[Optional behavior]");
    assert_eq!(description("p:blank"), "Do it");
}

#[test]
fn a_plugin_command_never_shadows_a_built_in() {
    let mut app = App::new();
    app.set_plugin_commands(vec![plugin_row("model", "Not the picker", None)]);
    let commands = app.commands();
    let models: Vec<&SlashCommand> = commands.iter().filter(|c| c.name == "model").collect();
    assert_eq!(models[0].effect, CommandEffect::Model);
}

/// For every built-in and every prefix of its name, plugin commands whose
/// segments start with that prefix: each built-in that matches the query
/// lists ahead of every plugin row, so Enter can never run a plugin's
/// template where the user typed the start of a built-in.
#[test]
fn no_plugin_command_lists_ahead_of_a_built_in_for_any_prefix_of_its_name() {
    for builtin in COMMANDS {
        let name = builtin.name.as_ref();
        for end in 1..=name.len() {
            let prefix = &name[..end];
            let mut app = App::new();
            app.set_plugin_commands(vec![
                plugin_row(&format!("dup:{name}"), "d", None),
                plugin_row(&format!("dup:{prefix}"), "d", None),
                plugin_row(&format!("{prefix}:x"), "d", None),
                plugin_row(&format!("{name}:{name}"), "d", Some("[a]")),
            ]);
            let commands = app.commands();
            let matches = matching_commands(&commands, prefix);
            let first_plugin = matches
                .iter()
                .position(|c| matches!(c.effect, CommandEffect::PluginPrompt { .. }))
                .expect("the plugin rows match");
            let last_builtin = matches
                .iter()
                .rposition(|c| !matches!(c.effect, CommandEffect::PluginPrompt { .. }))
                .expect("the built-in matches its own prefix");
            assert!(
                last_builtin < first_plugin,
                "/{prefix}: {:?}",
                matches.iter().map(|c| c.name.as_ref()).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn a_plugin_quit_command_never_takes_enter_from_quit() {
    for typed in ["/q", "/qu", "/quit"] {
        let mut app = App::new();
        app.set_plugin_commands(vec![plugin_row("dup:quit", "Not quitting", None)]);
        type_str(&mut app, typed);
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Quit, "{typed}");
    }
}

#[test]
fn a_plugin_command_is_found_by_any_segment_of_its_name() {
    let app = plugin_app();
    let commands = app.commands();
    let names = |query: &str| -> Vec<String> {
        matching_commands(&commands, query)
            .iter()
            .map(|c| c.name.to_string())
            .collect()
    };
    assert_eq!(names("rev"), ["code-review:review"]);
    assert_eq!(names("code"), ["code-review:commit", "code-review:review"]);
    assert_eq!(
        names("co"),
        [
            "copy",
            "compact",
            "context",
            "code-review:commit",
            "code-review:review"
        ]
    );
    // A built-in still matches by prefix alone.
    assert_eq!(names("lear"), Vec::<String>::new());
}

#[test]
fn running_a_plugin_command_that_takes_arguments_leaves_it_in_the_composer() {
    let mut app = plugin_app();
    type_str(&mut app, "/rev");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(app.input.text(), "/code-review:review ");
    assert_eq!(app.input.cursor(), app.input.text().len());
    assert!(app.command_menu.is_none());
}

#[test]
fn running_a_plugin_command_without_arguments_runs_it() {
    let mut app = plugin_app();
    type_str(&mut app, "/commit");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::RunPluginCommand {
            name: "code-review:commit".to_string(),
            args: String::new(),
        }
    );
    assert_eq!(app.input.text(), "");
}

#[test]
fn enter_on_a_typed_invocation_runs_the_command_with_its_arguments() {
    let mut app = plugin_app();
    type_str(&mut app, "/code-review:review 123 fast");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::RunPluginCommand {
            name: "code-review:review".to_string(),
            args: "123 fast".to_string(),
        }
    );
    assert_eq!(app.input.text(), "");
    assert_eq!(
        app.input_history.up().as_deref(),
        Some("/code-review:review 123 fast"),
        "recalled as typed"
    );
}

#[test]
fn a_bare_last_segment_reaches_the_command() {
    let mut app = plugin_app();
    type_str(&mut app, "/review 9");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::RunPluginCommand {
            name: "code-review:review".to_string(),
            args: "9".to_string(),
        }
    );
}

#[test]
fn a_built_in_word_is_never_routed_to_a_plugin_command() {
    let mut app = App::new();
    app.set_plugin_commands(vec![plugin_row("p:model", "d", Some("[x]"))]);
    type_str(&mut app, "/model gpt");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("/model gpt".to_string())
    );
}

#[test]
fn an_unknown_slash_word_is_still_an_ordinary_message() {
    let mut app = plugin_app();
    type_str(&mut app, "/usr/bin/env is where it lives");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("/usr/bin/env is where it lives".to_string())
    );
}

#[test]
fn a_plugin_command_mid_turn_is_refused_and_the_draft_kept() {
    let mut app = plugin_app();
    app.record_user_message("hello");
    app.begin_stream();
    type_str(&mut app, "/code-review:review 123");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Toast("/code-review:review is disabled while a task is in progress".to_string())
    );
    assert_eq!(app.input.text(), "/code-review:review 123");
    assert!(app.steered.is_empty(), "not handed to the running turn");
}

#[test]
fn a_plugin_command_from_the_palette_mid_turn_is_refused() {
    let mut app = plugin_app();
    app.record_user_message("hello");
    app.begin_stream();
    type_str(&mut app, "/commit");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Toast("/code-review:commit is disabled while a task is in progress".to_string())
    );
}

#[test]
fn a_plugin_command_typed_in_shell_mode_is_a_shell_command() {
    let mut app = plugin_app();
    type_str(&mut app, "!/review 1");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::RunShell("/review 1".to_string())
    );
}

#[test]
fn esc_undo_of_a_plugin_command_turn_restores_the_invocation() {
    let mut app = plugin_app();
    let rendered = "Review PR 123 with the rules.".to_string();
    app.remember_plugin_prompt(&rendered, "/code-review:review 123");
    app.record_user_message(&rendered); // mirror start_turn's recording
    app.begin_stream();
    assert_eq!(app.interrupt_turn(0), Some(InterruptedTurn::Undone));
    assert_eq!(app.input.text(), "/code-review:review 123");
    assert!(app.command_menu.is_none());
    assert!(app.history.is_empty());
}

#[test]
fn esc_undo_of_a_no_argument_plugin_command_keeps_the_palette_closed() {
    let mut app = plugin_app();
    app.remember_plugin_prompt("Commit now.", "/code-review:commit");
    app.record_user_message("Commit now.");
    app.begin_stream();
    assert_eq!(app.interrupt_turn(0), Some(InterruptedTurn::Undone));
    assert_eq!(app.input.text(), "/code-review:commit");
    assert!(app.command_menu.is_none());
}

#[test]
fn only_the_last_plugin_prompt_is_mapped_back() {
    let mut app = plugin_app();
    app.remember_plugin_prompt("first", "/code-review:commit");
    app.remember_plugin_prompt("second", "/code-review:review 2");
    app.record_user_message("first");
    app.begin_stream();
    assert_eq!(app.interrupt_turn(0), Some(InterruptedTurn::Undone));
    assert_eq!(app.input.text(), "first");
}

#[test]
fn images_attached_to_a_plugin_invocation_travel_with_its_turn() {
    let mut app = plugin_app();
    type_str(&mut app, "/code-review:review look at ");
    app.attach_image(std::path::PathBuf::from("/tmp/shot.png"));
    assert!(matches!(
        app.on_key(key(KeyCode::Enter)),
        Action::RunPluginCommand { .. }
    ));
    assert!(app.take_discarded_images().is_empty(), "nothing deleted");
    assert_eq!(
        app.take_submission_images(),
        [(
            "[Image #1]".to_string(),
            std::path::PathBuf::from("/tmp/shot.png")
        )]
    );
}

#[test]
fn a_plugin_command_picked_in_an_agent_view_is_refused() {
    let notice = Action::Toast("Plugin commands run in the main session".to_string());
    let mut app = plugin_app();
    app.open_agent_view("a1");
    type_str(&mut app, "/commit");
    assert_eq!(app.on_key(key(KeyCode::Enter)), notice);
    let mut app = plugin_app();
    app.open_agent_view("a1");
    type_str(&mut app, "/rev");
    assert_eq!(app.on_key(key(KeyCode::Enter)), notice);
    assert_eq!(app.input.text(), "", "no invocation left to send the agent");
}

#[test]
fn a_typed_plugin_invocation_in_an_agent_view_is_refused_and_kept() {
    let mut app = plugin_app();
    app.open_agent_view("a1");
    type_str(&mut app, "/code-review:review 12");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Toast("Plugin commands run in the main session".to_string())
    );
    assert_eq!(app.input.text(), "/code-review:review 12");
}

#[test]
fn ordinary_text_in_an_agent_view_still_goes_to_the_agent() {
    let mut app = plugin_app();
    app.open_agent_view("a1");
    type_str(&mut app, "/usr/bin/env please");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::AgentChat {
            id: "a1".to_string(),
            text: "/usr/bin/env please".to_string()
        }
    );
}
