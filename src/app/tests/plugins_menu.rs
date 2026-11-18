//! The `/plugins` page's state machine (`docs/plugins.md`).

use super::*;
use crate::app::{
    MarketplaceAction, PluginAction, PluginDone, PluginOp, PluginsConfirm, PluginsPage, PluginsTab,
    marketplace_actions, plugin_actions,
};
use crate::plugins::{
    CatalogMarketplace, CatalogPlugin, ComponentSummary, InstalledState, PluginCatalog, PluginId,
};

fn plugin(key: &str, installed: Option<bool>) -> CatalogPlugin {
    CatalogPlugin {
        id: PluginId::parse(key).unwrap(),
        display_name: None,
        description: Some(format!("The {key} plugin.")),
        version: Some("1.0.0".to_string()),
        author: None,
        category: None,
        homepage: None,
        keywords: vec!["tools".to_string()],
        source: "./plugins/x".to_string(),
        unavailable: None,
        format: None,
        components: Some(ComponentSummary::default()),
        installed: installed.map(|enabled| InstalledState {
            version: Some("1.0.0".to_string()),
            enabled,
            update: false,
            error: None,
        }),
        notes: Vec::new(),
    }
}

fn market(name: &str, git: bool) -> CatalogMarketplace {
    CatalogMarketplace {
        name: name.to_string(),
        source: format!("https://example.com/{name}.git"),
        git,
        description: None,
        owner: None,
        revision: None,
        updated_at: None,
        root: std::path::PathBuf::from("/m").join(name),
        plugins: 2,
        installed: 1,
        error: None,
        notes: Vec::new(),
    }
}

fn catalog() -> PluginCatalog {
    PluginCatalog {
        marketplaces: vec![market("team", true), market("local", false)],
        plugins: vec![
            plugin("hookify@team", Some(true)),
            plugin("lint@local", Some(false)),
            plugin("deploy@team", None),
            plugin("zeta@team", None),
        ],
        error: None,
    }
}

/// An app with the page open and its first read answered.
fn plugins_app() -> App {
    let mut app = App::new();
    let round = app.open_plugins_menu(true);
    assert!(app.finish_plugin_job(round, Some(catalog()), &Ok(PluginDone::default())));
    app
}

fn menu(app: &App) -> &crate::app::PluginsMenu {
    app.plugins_menu.as_ref().expect("the page is open")
}

fn rows(app: &App) -> Vec<String> {
    menu(app).plugin_rows().iter().map(|p| p.id.key()).collect()
}

/// The op the action asks for, finished with `catalog`.
fn finish(
    app: &mut App,
    action: Action,
    catalog: PluginCatalog,
    outcome: Result<PluginDone, String>,
) -> PluginOp {
    let Action::PluginOp { op, round } = action else {
        panic!("{action:?}");
    };
    assert!(app.finish_plugin_job(round, Some(catalog), &outcome));
    op
}

#[test]
fn the_palette_lists_plugins_and_it_opens_the_page_through_the_loop() {
    assert!(COMMANDS.iter().any(|c| c.name == "plugins"));
    let mut app = App::new();
    type_str(&mut app, "/plugins");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenPluginsMenu);
}

#[test]
fn the_page_opens_reading_and_picks_the_tab_with_something_on_it() {
    let mut app = App::new();
    let round = app.open_plugins_menu(true);
    assert!(menu(&app).catalog.is_none());
    assert_eq!(app.plugins_job_round(), Some(round));
    assert!(app.finish_plugin_job(round, Some(catalog()), &Ok(PluginDone::default())));
    assert_eq!(menu(&app).tab, PluginsTab::Installed);
    assert_eq!(rows(&app), ["hookify@team", "lint@local"]);
    assert!(menu(&app).job.is_none());

    // Nothing installed: what can be.
    let mut app = App::new();
    let round = app.open_plugins_menu(true);
    let mut fresh = catalog();
    fresh.plugins.iter_mut().for_each(|p| p.installed = None);
    app.finish_plugin_job(round, Some(fresh), &Ok(PluginDone::default()));
    assert_eq!(menu(&app).tab, PluginsTab::Discover);

    // Nothing at all: where a marketplace is added.
    let mut app = App::new();
    let round = app.open_plugins_menu(true);
    app.finish_plugin_job(
        round,
        Some(PluginCatalog::default()),
        &Ok(PluginDone::default()),
    );
    assert_eq!(menu(&app).tab, PluginsTab::Marketplaces);
    assert_eq!(
        menu(&app).marketplace_rows(),
        [crate::app::MarketplaceRow::Add]
    );
}

#[test]
fn a_late_answer_is_dropped() {
    let mut app = App::new();
    let first = app.open_plugins_menu(true);
    app.close_plugins_menu();
    let second = app.open_plugins_menu(true);
    assert!(!app.finish_plugin_job(first, Some(catalog()), &Ok(PluginDone::default())));
    assert!(menu(&app).catalog.is_none());
    assert!(app.finish_plugin_job(second, Some(catalog()), &Ok(PluginDone::default())));
    app.close_plugins_menu();
    assert!(!app.finish_plugin_job(second, Some(catalog()), &Ok(PluginDone::default())));
}

#[test]
fn tabs_move_with_the_arrows_and_tab_and_clear_the_search() {
    let mut app = plugins_app();
    type_str(&mut app, "hook");
    app.on_key(key(KeyCode::Right));
    assert_eq!(menu(&app).tab, PluginsTab::Discover);
    assert_eq!(menu(&app).query, "");
    assert_eq!(
        rows(&app),
        ["hookify@team", "lint@local", "deploy@team", "zeta@team"]
    );
    app.on_key(key(KeyCode::Tab));
    assert_eq!(menu(&app).tab, PluginsTab::Marketplaces);
    app.on_key(key(KeyCode::Tab));
    assert_eq!(menu(&app).tab, PluginsTab::Installed, "wrapping");
    app.on_key(key(KeyCode::BackTab));
    assert_eq!(menu(&app).tab, PluginsTab::Marketplaces);
    app.on_key(key(KeyCode::Left));
    assert_eq!(menu(&app).tab, PluginsTab::Discover);
}

#[test]
fn the_search_matches_names_marketplaces_descriptions_and_keywords() {
    let mut app = App::new();
    let round = app.open_plugins_menu(true);
    let mut fresh = catalog();
    fresh.plugins[2].description = Some("Ships builds to staging".to_string());
    fresh.plugins[3].keywords = vec!["kubernetes".to_string()];
    app.finish_plugin_job(round, Some(fresh), &Ok(PluginDone::default()));
    app.on_key(key(KeyCode::Right));
    type_str(&mut app, "LOCAL");
    assert_eq!(rows(&app), ["lint@local"]);
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).query, "", "Esc clears the search first");
    type_str(&mut app, "staging");
    assert_eq!(rows(&app), ["deploy@team"]);
    // Space is the toggle key, never part of a search.
    app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(menu(&app).query, "staging");
    for _ in 0.."staging".len() {
        app.on_key(key(KeyCode::Backspace));
    }
    type_str(&mut app, "kubernetes");
    assert_eq!(rows(&app), ["zeta@team"]);
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::ClosePluginsMenu);
    assert!(app.plugins_menu.is_none());
}

#[test]
fn space_turns_an_installed_plugin_on_or_off_at_once() {
    let mut app = plugins_app();
    let action = app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(
        action,
        Action::PluginOp {
            op: PluginOp::SetEnabled {
                id: PluginId::parse("hookify@team").unwrap(),
                enabled: false
            },
            round: app.plugins_job_round().unwrap()
        }
    );
    // The row answers before the write does.
    assert!(!menu(&app).plugin_rows()[0].is_enabled());
    assert_eq!(menu(&app).job.as_ref().unwrap().label, "Disabling hookify…");
    // A second op waits for it.
    assert_eq!(app.on_key(key(KeyCode::Down)), Action::None);
    assert!(
        matches!(app.on_key(key(KeyCode::Char(' '))), Action::Toast(text) if text.contains("Disabling hookify"))
    );
}

#[test]
fn space_on_a_plugin_that_is_not_installed_does_nothing() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.on_key(key(KeyCode::Char(' '))), Action::None);
    assert!(menu(&app).job.is_none());
}

#[test]
fn esc_stops_a_running_job_before_anything_else() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CancelPluginJob);
    assert!(app.plugins_menu.is_some());
    let round = app.plugins_job_round().unwrap();
    app.finish_plugin_job(round, Some(catalog()), &Err("cancelled".to_string()));
    assert_eq!(menu(&app).error.as_deref(), Some("cancelled"));
    assert_eq!(app.on_key(ctrl('c')), Action::ClosePluginsMenu);
}

#[test]
fn a_plugin_page_offers_what_its_state_affords() {
    let installed = plugin("a@m", Some(true));
    assert_eq!(
        plugin_actions(&installed),
        [
            PluginAction::Disable,
            PluginAction::Update,
            PluginAction::Uninstall,
            PluginAction::Back
        ]
    );
    let off = plugin("a@m", Some(false));
    assert_eq!(plugin_actions(&off)[0], PluginAction::Enable);
    let available = plugin("a@m", None);
    assert_eq!(
        plugin_actions(&available),
        [PluginAction::Install, PluginAction::Back]
    );
    let mut unavailable = plugin("a@m", None);
    unavailable.unavailable = Some("npm sources are not supported".to_string());
    assert_eq!(plugin_actions(&unavailable), [PluginAction::Back]);
    let mut orphan = plugin("a@m", Some(true));
    orphan.unavailable = Some("its marketplace no longer lists it".to_string());
    assert!(!plugin_actions(&orphan).contains(&PluginAction::Update));
    assert_eq!(
        marketplace_actions(&market("m", true)),
        [
            MarketplaceAction::Browse,
            MarketplaceAction::Update,
            MarketplaceAction::Remove,
            MarketplaceAction::Back
        ]
    );
    assert!(!marketplace_actions(&market("m", false)).contains(&MarketplaceAction::Update));
}

#[test]
fn installing_from_a_plugins_page_stays_on_it() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    let deploy = PluginId::parse("deploy@team").unwrap();
    assert_eq!(menu(&app).page, PluginsPage::Plugin(deploy.clone()));
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(menu(&app).job.as_ref().unwrap().label, "Installing deploy…");
    let mut after = catalog();
    after.plugins[2].installed = Some(InstalledState {
        version: Some("1.0.0".to_string()),
        enabled: true,
        update: false,
        error: None,
    });
    let op = finish(
        &mut app,
        action,
        after,
        Ok(PluginDone {
            message: "Installed".into(),
            marketplace: None,
        }),
    );
    assert_eq!(op, PluginOp::Install(deploy.clone()));
    assert_eq!(menu(&app).page, PluginsPage::Plugin(deploy));
    // Its actions are an installed plugin's now; Esc goes back to its row.
    assert_eq!(app.on_key(key(KeyCode::Char('9'))), Action::None);
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).selected, 2);
}

#[test]
fn back_from_a_plugins_page_lands_on_its_row_wherever_the_list_moved_it() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Right));
    // deploy is the third row of Discover.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    let action = app.on_key(key(KeyCode::Enter));
    // Installed, it sorts with the installed ones — first.
    let mut after = catalog();
    let mut deploy = after.plugins.remove(2);
    deploy.installed = Some(InstalledState {
        version: Some("1.0.0".to_string()),
        enabled: true,
        update: false,
        error: None,
    });
    after.plugins.insert(0, deploy);
    finish(&mut app, action, after, Ok(PluginDone::default()));
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(rows(&app)[menu(&app).selected], "deploy@team");
}

#[test]
fn updating_an_installed_plugin_says_so() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Enter));
    let action = app.on_key(key(KeyCode::Char('2')));
    assert!(matches!(
        action,
        Action::PluginOp {
            op: PluginOp::Install(_),
            ..
        }
    ));
    assert_eq!(menu(&app).job.as_ref().unwrap().label, "Updating hookify…");
}

#[test]
fn uninstalling_asks_first() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Enter));
    let hookify = PluginId::parse("hookify@team").unwrap();
    assert_eq!(app.on_key(key(KeyCode::Char('3'))), Action::None);
    assert_eq!(
        menu(&app).page,
        PluginsPage::Confirm(PluginsConfirm::Uninstall(hookify.clone()))
    );
    assert_eq!(menu(&app).selected, 1, "the safe answer is highlighted");
    // Enter on the safe answer goes back.
    app.on_key(key(KeyCode::Enter));
    assert_eq!(menu(&app).page, PluginsPage::Plugin(hookify.clone()));
    app.on_key(key(KeyCode::Char('3')));
    let action = app.on_key(key(KeyCode::Char('1')));
    assert!(
        matches!(action, Action::PluginOp { op: PluginOp::Uninstall(ref id), .. } if *id == hookify)
    );
    assert_eq!(menu(&app).page, PluginsPage::Plugin(hookify));
}

#[test]
fn a_page_whose_plugin_is_gone_comes_back_to_the_list() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('3')));
    let action = app.on_key(key(KeyCode::Char('1')));
    let mut after = catalog();
    after.plugins.remove(0);
    finish(&mut app, action, after, Ok(PluginDone::default()));
    assert_eq!(menu(&app).page, PluginsPage::List);
}

#[test]
fn a_marketplace_page_browses_updates_and_removes() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Left));
    assert_eq!(menu(&app).tab, PluginsTab::Marketplaces);
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        menu(&app).page,
        PluginsPage::Marketplace("team".to_string())
    );
    // Browse: the list, searched for it.
    app.on_key(key(KeyCode::Enter));
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).tab, PluginsTab::Discover);
    assert_eq!(rows(&app), ["hookify@team", "deploy@team", "zeta@team"]);

    let mut app = plugins_app();
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Enter));
    let action = app.on_key(key(KeyCode::Char('2')));
    assert!(
        matches!(action, Action::PluginOp { op: PluginOp::UpdateMarketplace(ref name), .. } if name == "team")
    );

    let mut app = plugins_app();
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('3')));
    assert_eq!(
        menu(&app).page,
        PluginsPage::Confirm(PluginsConfirm::RemoveMarketplace {
            name: "team".to_string(),
            plugins: vec!["hookify".to_string()],
        })
    );
    let action = app.on_key(key(KeyCode::Char('1')));
    let mut after = catalog();
    after.marketplaces.remove(0);
    let op = finish(&mut app, action, after, Ok(PluginDone::default()));
    assert_eq!(op, PluginOp::RemoveMarketplace("team".to_string()));
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).tab, PluginsTab::Marketplaces);
}

#[test]
fn a_local_marketplace_has_nothing_to_update() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        menu(&app).page,
        PluginsPage::Marketplace("local".to_string())
    );
    // Browse, Remove, Back.
    assert_eq!(app.on_key(key(KeyCode::Char('4'))), Action::None);
    app.on_key(key(KeyCode::Char('3')));
    assert_eq!(menu(&app).page, PluginsPage::List);
}

#[test]
fn a_marketplace_is_added_from_its_field() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::End));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(menu(&app).page, PluginsPage::Add);
    // Enter on an empty field does nothing.
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    app.on_paste("  anthropics/claude-plugins-official\n");
    assert_eq!(
        menu(&app).input.text(),
        "anthropics/claude-plugins-official"
    );
    let action = app.on_key(key(KeyCode::Enter));
    let Action::PluginOp { op, round } = action else {
        panic!();
    };
    assert_eq!(
        op,
        PluginOp::AddMarketplace("anthropics/claude-plugins-official".to_string())
    );
    // A failure stays on the page, under the field.
    app.finish_plugin_job(round, None, &Err("not a marketplace".to_string()));
    assert_eq!(menu(&app).page, PluginsPage::Add);
    assert_eq!(menu(&app).error.as_deref(), Some("not a marketplace"));
    // Editing clears it; a success lands on its plugins.
    app.on_key(key(KeyCode::Backspace));
    assert!(menu(&app).error.is_none());
    type_str(&mut app, "l");
    let action = app.on_key(key(KeyCode::Enter));
    finish(
        &mut app,
        action,
        catalog(),
        Ok(PluginDone {
            message: "Added".into(),
            marketplace: Some("team".to_string()),
        }),
    );
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).tab, PluginsTab::Discover);
    assert_eq!(menu(&app).query, "team");
}

#[test]
fn esc_on_the_field_goes_back_to_the_marketplaces() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::End));
    app.on_key(key(KeyCode::Enter));
    type_str(&mut app, "x");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).tab, PluginsTab::Marketplaces);
}

#[test]
fn ctrl_r_reads_the_marketplaces_again() {
    let mut app = plugins_app();
    let action = app.on_key(ctrl('r'));
    assert!(matches!(
        action,
        Action::PluginOp {
            op: PluginOp::Refresh,
            ..
        }
    ));
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
