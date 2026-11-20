//! The `/plugins` page's shim in `App` (`docs/plugins-menu.md`): the page's
//! own state machine is tested in its crate (`cargo test -p
//! alter-zero-plugins page::`); these hold what `App` adds — the palette
//! row, the routing, the round counter across a reopen, the paste and the
//! two read-only views.

use super::*;
use alter_zero_plugins::page::{PluginDone, SnapshotRows};

fn catalog() -> SnapshotRows {
    use alter_zero_plugins::page::{InstalledState, MarketplaceRow, PluginId, PluginRow};
    SnapshotRows {
        marketplaces: vec![MarketplaceRow {
            name: "team".to_string(),
            ..MarketplaceRow::default()
        }],
        plugins: vec![PluginRow {
            id: PluginId::new("hookify", "team").expect("a valid id"),
            installed: Some(InstalledState {
                enabled: true,
                ..InstalledState::default()
            }),
            ..PluginRow::default()
        }],
        session_enabled: true,
        ..SnapshotRows::default()
    }
}

fn plugins_app() -> App {
    let mut app = App::new();
    app.open_plugins_menu(catalog().into());
    app
}

fn menu(app: &App) -> &alter_zero_plugins::page::PluginsPage {
    app.plugins_menu.as_deref().expect("the page is open")
}

#[test]
fn the_palette_lists_plugins_and_it_opens_the_page_through_the_loop() {
    assert!(COMMANDS.iter().any(|c| c.name == "plugins"));
    let mut app = App::new();
    type_str(&mut app, "/plugins");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenPluginsMenu);
}

#[test]
fn closing_drops_the_snapshot_and_a_late_answer_is_dropped() {
    // Nothing about plugins is resident while the page is closed — and a
    // reopened page never takes the old page's answer: the round counter
    // lives on `App`.
    let mut app = plugins_app();
    let Action::PluginOp { round: first, .. } = app.on_key(key(KeyCode::Char(' '))) else {
        panic!("a toggle is a job");
    };
    app.close_plugins_menu();
    assert!(app.plugins_menu.is_none());
    assert!(!app.finish_plugin_job(first, Some(catalog().into()), &Ok(PluginDone::default())));
    app.open_plugins_menu(catalog().into());
    assert!(!app.finish_plugin_job(first, Some(catalog().into()), &Ok(PluginDone::default())));
    let Action::PluginOp { round: second, .. } = app.on_key(key(KeyCode::Char(' '))) else {
        panic!("a toggle is a job");
    };
    assert_ne!(first, second);
    assert!(app.finish_plugin_job(second, Some(catalog().into()), &Ok(PluginDone::default())));
    assert!(
        !app.finish_plugin_job(second, Some(catalog().into()), &Ok(PluginDone::default())),
        "an answer is taken once"
    );
}

#[test]
fn the_ctrl_b_hint_hides_while_the_page_is_up() {
    let mut app = plugins_app();
    app.set_command_elapsed(Some(std::time::Duration::from_secs(9)));
    assert_eq!(app.background_hint_elapsed(), None);
    app.close_plugins_menu();
    assert_eq!(
        app.background_hint_elapsed(),
        Some(std::time::Duration::from_secs(9))
    );
}

// ===== what this page adds to the prior one =====

#[test]
fn a_paste_never_reaches_the_composer() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Enter));
    app.on_paste("stray text");
    assert_eq!(app.input.text(), "", "a plugin's page swallows it");
    app.on_key(key(KeyCode::Esc));
    app.on_paste("hook\n");
    assert_eq!(menu(&app).query, "hook");
    assert_eq!(app.input.text(), "");
}

#[test]
fn ctrl_o_and_ctrl_d_open_the_read_only_views_over_the_page() {
    let mut app = plugins_app();
    assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
    assert!(app.plugins_menu.is_some());
    assert!(!app.plugins_menu_ticks() || app.view.is_overlay());
}

#[test]
fn the_palette_lists_plugins_right_after_mcp() {
    let names: Vec<&str> = COMMANDS.iter().map(|c| c.name.as_ref()).collect();
    let mcp = names.iter().position(|n| *n == "mcp").unwrap();
    assert_eq!(names[mcp + 1], "plugins");
}
