//! The colour-theme catalog, the `/theme` picker, and its persistence
//! format (`docs/theme.md`).

use super::*;

/// An app with the `/theme` picker open.
fn theme_app() -> App {
    let mut app = App::new();
    app.open_theme_picker();
    app
}

/// The names of the rows the picker currently lists.
fn names(app: &App) -> Vec<&'static str> {
    app.theme_rows().iter().map(|r| r.theme.name()).collect()
}

// ===== the catalog =====

#[test]
fn the_catalog_holds_the_eleven_themes_mocha_first() {
    let names: Vec<&str> = Theme::ALL.iter().map(|t| t.name()).collect();
    assert_eq!(
        names,
        [
            "mocha",
            "macchiato",
            "frappe",
            "latte",
            "onedark",
            "dracula",
            "nord",
            "gruvbox",
            "solarized",
            "monokai",
            "ansi",
        ]
    );
    assert_eq!(
        Theme::default(),
        Theme::Mocha,
        "Catppuccin Mocha — the code blocks' theme since the syntect port — is the default"
    );
}

#[test]
fn every_theme_has_a_distinct_lowercase_name_a_title_and_a_description() {
    let mut seen = std::collections::HashSet::new();
    for theme in Theme::ALL {
        let name = theme.name();
        assert!(!name.is_empty());
        assert_eq!(name, name.to_lowercase(), "{name}: names are lowercase");
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric()),
            "{name}: a name is one bare word (the search and the file read it)"
        );
        assert!(seen.insert(name), "{name}: duplicate name");
        assert!(!theme.title().is_empty(), "{name}: missing title");
        assert!(
            !theme.description().is_empty(),
            "{name}: missing description"
        );
        assert!(
            theme.description().starts_with(theme.title()),
            "{name}: the description opens with the title, so the row's bare \
             name is explained right under the preview: {:?}",
            theme.description()
        );
    }
}

#[test]
fn from_name_round_trips_case_insensitively() {
    for theme in Theme::ALL {
        assert_eq!(Theme::from_name(theme.name()), Some(theme));
        assert_eq!(Theme::from_name(&theme.name().to_uppercase()), Some(theme));
    }
    assert_eq!(Theme::from_name("no-such-theme"), None);
    assert_eq!(Theme::from_name(""), None);
}

#[test]
fn the_light_theme_and_the_terminal_theme_say_so() {
    // The two themes that behave differently from the rest announce it in
    // their descriptions, since the picker's description row is where a
    // user learns why a theme looks wrong on their terminal.
    assert!(Theme::Latte.is_light(), "latte is the light flavour");
    assert!(
        Theme::ALL.iter().filter(|t| t.is_light()).count() == 1,
        "latte is the only light theme in the catalog"
    );
    assert!(
        Theme::Latte.description().to_lowercase().contains("light"),
        "{:?}",
        Theme::Latte.description()
    );
    assert!(
        Theme::Ansi
            .description()
            .to_lowercase()
            .contains("terminal"),
        "{:?}",
        Theme::Ansi.description()
    );
}

// ===== the persistence format (docs/theme.md) =====

#[test]
fn the_theme_file_round_trips() {
    for theme in Theme::ALL {
        let json = theme_file_json(theme);
        assert_eq!(parse_theme_file(&json), Some(theme), "{json}");
    }
    assert_eq!(
        theme_file_json(Theme::Dracula),
        "{\n  \"theme\": \"dracula\"\n}\n"
    );
}

#[test]
fn a_missing_or_corrupt_theme_file_reads_as_none() {
    assert_eq!(parse_theme_file(""), None);
    assert_eq!(parse_theme_file("not json"), None);
    assert_eq!(parse_theme_file("{}"), None);
    assert_eq!(parse_theme_file(r#"{"theme": "unknown"}"#), None);
    assert_eq!(
        parse_theme_file(r#"{"spinner": "mocha"}"#),
        None,
        "the spinner file's key is not this file's"
    );
}

// ===== the /theme command =====

#[test]
fn slash_theme_opens_the_picker() {
    let mut app = App::new();
    type_chars(&mut app, "/theme");
    assert!(app.command_menu.is_some());
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::OpenThemePicker);
    assert!(app.theme_picker.is_some(), "the pure open happened");
    assert!(app.input.is_empty(), "the /theme token was consumed");
    assert!(app.command_menu.is_none(), "the palette closed");
}

#[test]
fn the_palette_lists_theme_ahead_of_mascot() {
    let names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
    let theme = names.iter().position(|n| *n == "theme").expect("theme");
    assert_eq!(
        names.get(theme + 1),
        Some(&"mascot"),
        "the three look-and-feel pickers sit together, the theme first: {names:?}"
    );
}

#[test]
fn the_open_lands_on_the_active_theme() {
    let mut app = App::new();
    app.set_theme(Theme::Nord);
    app.open_theme_picker();
    assert_eq!(
        app.highlighted_theme(),
        Some(Theme::Nord),
        "the picker opens with the current theme highlighted"
    );
}

#[test]
fn opening_abandons_the_bands_that_share_the_composer() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_theme_picker();
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none());
    assert!(app.file_search.is_none());
    assert!(app.theme_picker.is_some());
}

#[test]
fn the_open_picker_needs_no_animation_frames() {
    // The page is still — nothing on it ticks — so, unlike `/spinner`, the
    // picker never re-arms the 32 ms chain.
    let mut app = App::new();
    app.open_theme_picker();
    assert!(!app.wants_animation_frames());
}

#[test]
fn the_open_picker_hides_the_ctrl_b_hint_like_its_siblings() {
    let mut app = App::new();
    app.set_command_elapsed(Some(Duration::from_secs(5)));
    assert!(app.background_hint_elapsed().is_some());
    app.open_theme_picker();
    assert_eq!(
        app.background_hint_elapsed(),
        None,
        "the picker swallows Ctrl+B"
    );
    app.close_theme_picker();
    assert!(
        app.background_hint_elapsed().is_some(),
        "closed: the hint comes back"
    );
}

// ===== the picker's key grammar (the /settings family) =====

#[test]
fn up_and_down_wrap_at_the_ends() {
    let mut app = theme_app();
    assert_eq!(
        app.highlighted_theme(),
        Some(Theme::Mocha),
        "opens on the active theme"
    );
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.highlighted_theme(),
        Some(Theme::Ansi),
        "↑ from the first row wraps to the last"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.highlighted_theme(),
        Some(Theme::Mocha),
        "↓ from the last row wraps to the first"
    );
    app.on_key(key(KeyCode::End));
    assert_eq!(
        app.highlighted_theme(),
        Some(Theme::Ansi),
        "End clamps at the bottom"
    );
    app.on_key(key(KeyCode::PageUp));
    assert_eq!(
        app.highlighted_theme(),
        Some(Theme::Mocha),
        "PageUp clamps at the top"
    );
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Home));
    assert_eq!(app.highlighted_theme(), Some(Theme::Mocha), "Home");
}

#[test]
fn typing_filters_the_rows_and_resets_the_selection() {
    let mut app = theme_app();
    app.on_key(key(KeyCode::Down));
    type_chars(&mut app, "drac");
    assert_eq!(names(&app), ["dracula"]);
    assert_eq!(app.highlighted_theme(), Some(Theme::Dracula));
    for _ in 0..4 {
        app.on_key(key(KeyCode::Backspace));
    }
    assert_eq!(names(&app).len(), Theme::ALL.len(), "Backspace widens");
}

#[test]
fn the_search_matches_titles_and_descriptions_too() {
    let mut app = theme_app();
    type_chars(&mut app, "catppuccin");
    assert_eq!(
        names(&app),
        ["mocha", "macchiato", "frappe", "latte"],
        "the four flavours are found by the family name in their titles"
    );
    for _ in 0..10 {
        app.on_key(key(KeyCode::Backspace));
    }
    // (One word: a space would *select*, the family's Enter/Space grammar.)
    type_chars(&mut app, "follows");
    assert_eq!(
        names(&app),
        ["ansi"],
        "the terminal theme is found by its description"
    );
}

#[test]
fn enter_selects_the_highlighted_theme_and_closes() {
    let mut app = theme_app();
    app.on_key(key(KeyCode::Down)); // mocha (the open seat) → macchiato
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::SelectTheme(Theme::Macchiato));
    assert_eq!(
        app.theme(),
        Theme::Macchiato,
        "the pure state already moved"
    );
    assert!(app.theme_picker.is_none(), "the picker closed");
}

#[test]
fn space_selects_like_enter() {
    let mut app = theme_app();
    app.on_key(key(KeyCode::End));
    let action = app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(action, Action::SelectTheme(Theme::Ansi));
    assert!(app.theme_picker.is_none());
}

#[test]
fn enter_on_an_empty_filter_is_a_no_op() {
    let mut app = theme_app();
    type_chars(&mut app, "zzz");
    assert!(names(&app).is_empty());
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.theme_picker.is_some(), "nothing selected, stays open");
    assert_eq!(app.theme(), Theme::Mocha, "the theme is untouched");
}

#[test]
fn esc_clears_the_query_first_then_closes() {
    let mut app = theme_app();
    type_chars(&mut app, "no");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    let picker = app.theme_picker.as_ref().expect("still open");
    assert!(picker.query.is_empty(), "Esc cleared the query");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseThemePicker);
    assert!(app.theme_picker.is_none());
}

#[test]
fn ctrl_c_closes_without_quitting() {
    let mut app = theme_app();
    assert_eq!(app.on_key(ctrl('c')), Action::CloseThemePicker);
    assert!(app.theme_picker.is_none());
}

#[test]
fn the_picker_owns_every_key_while_open() {
    let mut app = theme_app();
    app.on_key(key(KeyCode::Char('?')));
    assert!(!app.shortcuts_open, "`?` types, never opens the band");
    let picker = app.theme_picker.as_ref().expect("open");
    assert_eq!(picker.query, "?");
}

#[test]
fn typing_reseats_the_highlight_on_the_first_match() {
    let mut app = theme_app();
    app.on_key(key(KeyCode::End));
    type_chars(&mut app, "nord");
    assert_eq!(names(&app), ["nord"]);
    assert_eq!(app.highlighted_theme(), Some(Theme::Nord));
}

#[test]
fn the_active_row_is_marked() {
    let mut app = App::new();
    app.set_theme(Theme::Gruvbox);
    app.open_theme_picker();
    let active: Vec<&'static str> = app
        .theme_rows()
        .iter()
        .filter(|r| r.active)
        .map(|r| r.theme.name())
        .collect();
    assert_eq!(active, ["gruvbox"], "exactly the session's theme is marked");
}

#[test]
fn the_picker_works_mid_turn_without_touching_the_turn() {
    // Like /mascot and /spinner it only replaces the composer: opening,
    // browsing and choosing leave the streaming turn exactly as it was.
    let mut app = App::new();
    app.record_user_message("hi");
    app.begin_stream();
    app.push_chunk("streaming");
    type_chars(&mut app, "/theme");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenThemePicker);
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::SelectTheme(Theme::Macchiato)
    );
    assert!(app.is_streaming(), "the turn is untouched");
    assert!(app.turn_active());
}
