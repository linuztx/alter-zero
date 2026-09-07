//! The inline `/settings` menu's state and key map (`docs/settings.md`).

use super::*;

use crate::permission::PermissionMode;
use crate::settings::{SettingAvailability, SettingKey};

/// An app with the settings menu open and a permission gate present (the
/// ordinary session shape).
fn settings_app() -> App {
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Manual));
    app.open_settings();
    app
}

fn space() -> KeyEvent {
    key(KeyCode::Char(' '))
}

fn type_query(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

/// The labels of the rows the menu currently lists.
fn labels(app: &App) -> Vec<&'static str> {
    app.setting_rows().iter().map(|r| r.label).collect()
}

#[test]
fn the_menu_opens_listing_every_setting_from_the_top() {
    let app = settings_app();
    assert!(app.settings_picker.is_some());
    assert_eq!(app.setting_rows().len(), SettingKey::ALL.len());
    assert_eq!(
        app.highlighted_setting().map(|r| r.key),
        Some(SettingKey::HideThinking),
        "opens on the first row"
    );
}

#[test]
fn opening_abandons_the_bands_that_share_the_composer() {
    // The menu replaces the composer, so the palette / `?` band / `@` picker
    // that hang off it go — the `/model` picker's rule.
    let mut app = App::new();
    app.shortcuts_open = true;
    type_chars(&mut app, "/set");
    assert!(app.command_menu.is_some());
    app.open_settings();
    assert!(app.command_menu.is_none());
    assert!(!app.shortcuts_open);
    assert!(app.file_search.is_none());
}

#[test]
fn the_rows_show_the_live_values() {
    let mut app = settings_app();
    let rows = app.setting_rows();
    let value = |key: SettingKey| {
        rows.iter()
            .find(|r| r.key == key)
            .map(|r| r.value.clone())
            .unwrap()
    };
    assert_eq!(value(SettingKey::HideThinking), "false");
    assert_eq!(value(SettingKey::ErrorRetry), "3");
    assert_eq!(value(SettingKey::PermissionMode), "manual");

    // A change anywhere in the session is reflected without the menu storing
    // anything of its own.
    app.set_permission_mode(Some(PermissionMode::Master));
    assert_eq!(
        app.setting_rows()
            .iter()
            .find(|r| r.key == SettingKey::PermissionMode)
            .unwrap()
            .value,
        "master"
    );
}

#[test]
fn up_and_down_move_the_selection_wrapping_at_the_ends() {
    let mut app = settings_app();
    assert_eq!(app.on_key(key(KeyCode::Up)), Action::None);
    assert_eq!(
        app.highlighted_setting().map(|r| r.key),
        SettingKey::ALL.last().copied(),
        "Up from the first row wraps to the last"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.highlighted_setting().map(|r| r.key),
        Some(SettingKey::HideThinking),
        "Down from the last row wraps back to the first"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.highlighted_setting().map(|r| r.key),
        Some(SettingKey::ShowImages)
    );
    app.on_key(key(KeyCode::End));
    assert_eq!(
        app.highlighted_setting().map(|r| r.key),
        SettingKey::ALL.last().copied()
    );
    app.on_key(key(KeyCode::Home));
    assert_eq!(
        app.highlighted_setting().map(|r| r.key),
        Some(SettingKey::HideThinking)
    );
}

#[test]
fn arrows_on_an_empty_filtered_list_do_nothing() {
    let mut app = settings_app();
    for c in "zzzz".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    assert!(app.setting_rows().is_empty(), "nothing matches zzzz");
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Down));
    assert!(app.highlighted_setting().is_none());
    assert_eq!(app.settings_picker.as_ref().unwrap().selected, 0);
}

#[test]
fn enter_and_space_both_cycle_the_highlighted_setting() {
    for press in [key(KeyCode::Enter), space()] {
        let mut app = settings_app();
        assert_eq!(
            app.on_key(press),
            Action::SettingChanged(SettingKey::HideThinking)
        );
        assert!(app.settings().hide_thinking, "the value moved");
        assert!(
            app.settings_picker.is_some(),
            "the menu stays open so several knobs can be set in one visit"
        );
    }
}

#[test]
fn the_permission_row_routes_through_the_shift_tab_path() {
    // One state, two doors: cycling here must produce the same action
    // Shift+Tab does, so the gate, the persisted project entry and the
    // queued-request sweep all still happen.
    let mut app = settings_app();
    type_query(&mut app, "permission");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::SetPermissionMode(PermissionMode::Edit)
    );
    assert_eq!(app.permission_mode(), Some(PermissionMode::Edit));
}

#[test]
fn cycling_an_unavailable_setting_explains_itself_instead() {
    // The host can't run checkpoints — the row says `(unavailable)` and the
    // press raises a toast rather than silently doing nothing.
    let mut app = settings_app();
    // The directory had turned checkpoints on; this host can't serve them.
    app.settings_mut().checkpoints = true;
    app.set_setting_availability(SettingAvailability {
        checkpoints: false,
        hooks: true,
        skills: true,
        images: true,
        telemetry: true,
    });
    type_query(&mut app, "checkpoint");
    let row = app.highlighted_setting().unwrap();
    assert!(!row.available);
    assert!(row.value.contains("unavailable"), "{}", row.value);
    match app.on_key(key(KeyCode::Enter)) {
        Action::Toast(text) => assert!(text.contains("Checkpoints"), "{text}"),
        other => panic!("expected an explanatory toast, got {other:?}"),
    }
    assert!(app.settings().checkpoints, "the preference is untouched");
}

#[test]
fn typing_filters_on_the_label_and_the_description() {
    let mut app = settings_app();
    type_query(&mut app, "retry");
    assert_eq!(labels(&app), vec!["Error retry"]);
    assert_eq!(
        app.highlighted_setting().map(|r| r.key),
        Some(SettingKey::ErrorRetry),
        "the highlight re-seats on the narrowed list"
    );

    // The description is matched too, so a user who knows the concept but not
    // our label still finds the row.
    app.on_key(key(KeyCode::Esc));
    type_query(&mut app, "agents.md");
    assert_eq!(labels(&app), vec!["Project docs"]);
}

#[test]
fn the_search_is_case_insensitive_and_backspace_pops_it() {
    let mut app = settings_app();
    type_query(&mut app, "TOOLS");
    let shouted = labels(&app);
    assert_eq!(shouted, vec!["Tools"]);
    for _ in 0..5 {
        app.on_key(key(KeyCode::Backspace));
    }
    assert_eq!(
        labels(&app).len(),
        SettingKey::ALL.len(),
        "back to everything"
    );
    type_query(&mut app, "tools");
    assert_eq!(labels(&app), shouted, "case makes no difference");
}

#[test]
fn space_cycles_rather_than_typing_even_mid_search() {
    // Space is the cycle key everywhere in this menu — which is exactly why a
    // setting label can never need one to be found.
    let mut app = settings_app();
    type_query(&mut app, "tools");
    assert_eq!(
        app.on_key(space()),
        Action::SettingChanged(SettingKey::Tools)
    );
    assert!(!app.settings().tools);
    assert_eq!(labels(&app), vec!["Tools"], "the query survived");
}

#[test]
fn a_query_matching_nothing_lists_nothing_and_enter_is_inert() {
    let mut app = settings_app();
    type_query(&mut app, "zzz");
    assert!(app.setting_rows().is_empty());
    assert!(app.highlighted_setting().is_none());
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
}

#[test]
fn esc_clears_a_query_first_then_closes() {
    let mut app = settings_app();
    type_query(&mut app, "tools");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(
        app.settings_picker.is_some(),
        "the first Esc cleared the query"
    );
    assert_eq!(labels(&app).len(), SettingKey::ALL.len());
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseSettings);
    assert!(app.settings_picker.is_none());
}

#[test]
fn ctrl_c_closes_the_menu_and_never_quits() {
    let mut app = settings_app();
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.on_key(ctrl_c), Action::CloseSettings);
    assert!(app.settings_picker.is_none());
}

#[test]
fn the_menu_owns_every_key_while_open() {
    // Ctrl+O, Ctrl+D, Shift+Tab and Ctrl+T all do something drastic in the
    // composer; none of them may fire out from under an open menu (the
    // `/model` rule).
    let mut app = settings_app();
    for k in [
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
    ] {
        assert_eq!(app.on_key(k), Action::None, "{k:?} leaked out of the menu");
        assert_eq!(app.view, View::Conversation);
        assert!(app.settings_picker.is_some());
    }
}

#[test]
fn the_slash_command_opens_it() {
    let mut app = App::new();
    type_chars(&mut app, "/settings");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenSettings);
}

#[test]
fn the_settings_command_works_mid_turn() {
    // Like `/model` and `/login` it only replaces the composer — the running
    // turn streams on its own thread, untouched.
    let mut app = App::new();
    app.begin_stream();
    type_chars(&mut app, "/settings");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenSettings);
}

#[test]
fn auto_compaction_obeys_the_setting() {
    // The one knob whose whole effect is a pure gate: with it off the loop's
    // idle check never fires, however full the window is.
    let mut app = App::new();
    app.record_user_message("a long-running conversation");
    app.set_context_window(Some(1_000));
    app.begin_stream();
    app.apply_usage(&crate::stream::TokenUsage {
        input: 950,
        ..crate::stream::TokenUsage::default()
    });
    app.finish_stream();
    app.end_turn(1);
    assert!(app.should_auto_compact(), "the gauge is past the threshold");
    app.settings_mut().auto_compact = false;
    assert!(!app.should_auto_compact());
}
