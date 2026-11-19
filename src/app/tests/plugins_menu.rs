//! The `/plugins` page's state machine (`docs/plugins.md`).

use super::*;
use crate::app::{
    MarketplaceAction, MarketplaceRow, PluginAction, PluginDone, PluginOp, PluginsConfirm,
    PluginsPage, PluginsTab, marketplace_actions, plugin_actions,
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
            in_place: false,
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
fn the_tabs_are_installed_discover_and_marketplaces() {
    // Three tabs that never change: a tab per marketplace made `All` and
    // the one marketplace's tab the same list, and filled the strip with
    // marketplace names.
    assert_eq!(
        PluginsTab::ALL,
        [
            PluginsTab::Installed,
            PluginsTab::Discover,
            PluginsTab::Marketplaces
        ]
    );
    let mut app = App::new();
    app.open_plugins_menu(true);
    assert_eq!(menu(&app).tabs(), PluginsTab::ALL);
    assert_eq!(menu(&plugins_app()).tabs(), PluginsTab::ALL);
}

#[test]
fn the_page_opens_on_the_tab_with_something_on_it() {
    let mut app = App::new();
    let round = app.open_plugins_menu(true);
    assert!(menu(&app).catalog.is_none());
    assert_eq!(app.plugins_job_round(), Some(round));
    assert!(app.finish_plugin_job(round, Some(catalog()), &Ok(PluginDone::default())));
    assert_eq!(menu(&app).tab, PluginsTab::Installed);
    assert_eq!(rows(&app), ["hookify@team", "lint@local"]);
    assert!(menu(&app).job.is_none());

    // Nothing installed: everything there is.
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
}

#[test]
fn a_reopened_page_shows_the_catalog_it_kept_at_once() {
    // The read runs again behind the rows, so the page opens at its own
    // height instead of on a placeholder that the rows then push away.
    let mut app = plugins_app();
    app.close_plugins_menu();
    let round = app.open_plugins_menu(true);
    assert_eq!(menu(&app).catalog, Some(catalog()));
    assert_eq!(menu(&app).tab, PluginsTab::Installed);
    assert_eq!(app.plugins_job_round(), Some(round));
    // A catalog the boundary read with no page open is kept too — an older
    // round never over a newer one.
    let mut app = App::new();
    app.keep_plugin_catalog(0, catalog());
    let mut older = catalog();
    older.plugins.clear();
    app.keep_plugin_catalog(0, older.clone());
    app.open_plugins_menu(true);
    assert_eq!(menu(&app).catalog, Some(older), "the same round replaces");
    let mut app = App::new();
    app.keep_plugin_catalog(5, catalog());
    app.keep_plugin_catalog(3, PluginCatalog::default());
    app.open_plugins_menu(true);
    assert_eq!(
        menu(&app).catalog,
        Some(catalog()),
        "an older round does not"
    );
}

#[test]
fn a_late_answer_is_dropped() {
    let mut app = App::new();
    let first = app.open_plugins_menu(true);
    app.close_plugins_menu();
    let second = app.open_plugins_menu(true);
    assert!(!app.finish_plugin_job(first, Some(catalog()), &Ok(PluginDone::default())));
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
    assert!(rows(&app).is_empty(), "no plugin rows there");
    app.on_key(key(KeyCode::Right));
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
    assert_eq!(menu(&app).tab, PluginsTab::Discover);
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
    // `@{marketplace}` lists one marketplace's plugins — what its page's
    // Browse searches for.
    type_str(&mut app, "@team");
    assert_eq!(rows(&app), ["hookify@team", "deploy@team", "zeta@team"]);
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
fn a_failure_is_said_on_the_page_until_the_next_key() {
    // What went right shows on the page itself — the row, the page's state
    // line — so only a failure is said, never a toast row over the page.
    let mut app = plugins_app();
    let action = app.on_key(key(KeyCode::Char(' ')));
    let Action::PluginOp { round, .. } = action else {
        panic!("{action:?}");
    };
    app.finish_plugin_job(round, Some(catalog()), &Err("disk full".to_string()));
    assert_eq!(menu(&app).error.as_deref(), Some("disk full"));
    app.on_key(key(KeyCode::Up));
    assert_eq!(menu(&app).error, None);
}

#[test]
fn space_on_a_plugin_that_is_not_installed_does_nothing() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(rows(&app)[menu(&app).selected], "deploy@team");
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
    // Loaded in place, it is always current: nothing to update.
    let mut in_place = plugin("a@m", Some(true));
    in_place.installed.as_mut().unwrap().in_place = true;
    assert!(!plugin_actions(&in_place).contains(&PluginAction::Update));
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
        in_place: false,
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
    assert_eq!(rows(&app)[menu(&app).selected], "deploy@team");
}

#[test]
fn back_from_a_plugins_page_lands_on_its_row_wherever_the_list_moved_it() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Right));
    assert_eq!(menu(&app).tab, PluginsTab::Discover);
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
        in_place: false,
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
fn uninstalling_asks_first_with_uninstall_highlighted() {
    // The question is the safeguard: once asked, Enter answers it — no
    // arrow key to reach the action the user just chose.
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Enter));
    let hookify = PluginId::parse("hookify@team").unwrap();
    assert_eq!(app.on_key(key(KeyCode::Char('3'))), Action::None);
    assert_eq!(
        menu(&app).page,
        PluginsPage::Confirm(PluginsConfirm::Uninstall(hookify.clone()))
    );
    assert_eq!(menu(&app).selected, 0, "Uninstall is highlighted");
    // Esc, or Cancel, goes back.
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).page, PluginsPage::Plugin(hookify.clone()));
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Char('2')));
    assert_eq!(menu(&app).page, PluginsPage::Plugin(hookify.clone()));
    app.on_key(key(KeyCode::Char('3')));
    let action = app.on_key(key(KeyCode::Enter));
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

/// The Marketplaces tab of a fresh page.
fn marketplaces_tab(catalog: PluginCatalog) -> App {
    let mut app = App::new();
    let round = app.open_plugins_menu(true);
    app.finish_plugin_job(round, Some(catalog), &Ok(PluginDone::default()));
    app.on_key(key(KeyCode::Left));
    assert_eq!(menu(&app).tab, PluginsTab::Marketplaces);
    app
}

fn market_rows(app: &App) -> Vec<String> {
    menu(app)
        .marketplace_rows()
        .iter()
        .map(|row| match row {
            MarketplaceRow::Market(name) => name.clone(),
            MarketplaceRow::Suggestion(suggestion) => format!("+{}", suggestion.name),
            MarketplaceRow::Add => "add".to_string(),
        })
        .collect()
}

#[test]
fn the_marketplaces_tab_lists_the_added_the_official_and_the_way_to_add_any() {
    let app = marketplaces_tab(catalog());
    assert_eq!(
        market_rows(&app),
        [
            "team",
            "local",
            "+claude-plugins-official",
            "+openai-curated",
            "add"
        ]
    );
    // An official one added is a marketplace like any other.
    let mut with_official = catalog();
    with_official
        .marketplaces
        .push(market("claude-plugins-official", true));
    let app = marketplaces_tab(with_official);
    assert_eq!(
        market_rows(&app),
        [
            "team",
            "local",
            "claude-plugins-official",
            "+openai-curated",
            "add"
        ]
    );
    // The search narrows the marketplaces, never the way to add one.
    let mut app = marketplaces_tab(catalog());
    type_str(&mut app, "loc");
    assert_eq!(market_rows(&app), ["local", "add"]);
    // Before the first read there is nothing to list.
    let mut app = App::new();
    app.open_plugins_menu(true);
    app.on_key(key(KeyCode::Left));
    assert!(market_rows(&app).is_empty());
}

#[test]
fn enter_on_an_official_marketplace_adds_it_and_lists_its_plugins() {
    // Claude Code's and Codex's own catalogs are one Enter away.
    let mut app = marketplaces_tab(catalog());
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    let action = app.on_key(key(KeyCode::Enter));
    let mut after = catalog();
    after
        .marketplaces
        .push(market("claude-plugins-official", true));
    after
        .plugins
        .push(plugin("context7@claude-plugins-official", None));
    let op = finish(
        &mut app,
        action,
        after,
        Ok(PluginDone {
            message: String::new(),
            marketplace: Some("claude-plugins-official".to_string()),
        }),
    );
    assert_eq!(
        op,
        PluginOp::AddMarketplace("anthropics/claude-plugins-official".to_string())
    );
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).tab, PluginsTab::Discover);
    assert_eq!(menu(&app).query, "@claude-plugins-official");
    assert_eq!(rows(&app), ["context7@claude-plugins-official"]);
}

#[test]
fn the_add_row_opens_a_field_for_any_source() {
    let mut app = marketplaces_tab(catalog());
    app.on_key(key(KeyCode::End));
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(menu(&app).page, PluginsPage::Add);
    assert_eq!(menu(&app).input.text(), "");
    // Space types here: a directory's path may hold one.
    type_str(&mut app, "/srv/my plugins");
    assert_eq!(menu(&app).input.text(), "/srv/my plugins");
    // Esc goes back to the row it came from.
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(
        menu(&app).marketplace_rows()[menu(&app).selected],
        MarketplaceRow::Add
    );

    // What was searched for comes along; pasted, then added — a failure
    // says why and keeps what was typed.
    let mut app = marketplaces_tab(catalog());
    type_str(&mut app, "acme");
    assert_eq!(market_rows(&app), ["add"]);
    app.on_key(key(KeyCode::Enter));
    assert_eq!(menu(&app).input.text(), "acme");
    app.on_paste("/tools\n");
    assert_eq!(menu(&app).input.text(), "acme/tools");
    let action = app.on_key(key(KeyCode::Enter));
    let Action::PluginOp { op, round } = action else {
        panic!("{action:?}");
    };
    assert_eq!(op, PluginOp::AddMarketplace("acme/tools".to_string()));
    app.finish_plugin_job(round, None, &Err("not a marketplace".to_string()));
    assert_eq!(menu(&app).page, PluginsPage::Add);
    assert_eq!(menu(&app).input.text(), "acme/tools");
    assert_eq!(menu(&app).error.as_deref(), Some("not a marketplace"));
    // Editing clears it; Ctrl+U empties the field; Enter on an empty one
    // asks for nothing.
    app.on_key(key(KeyCode::Backspace));
    assert!(menu(&app).error.is_none());
    app.on_key(ctrl('u'));
    assert_eq!(menu(&app).input.text(), "");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    // A success lists the new marketplace's plugins.
    type_str(&mut app, "acme/team");
    let action = app.on_key(key(KeyCode::Enter));
    finish(
        &mut app,
        action,
        catalog(),
        Ok(PluginDone {
            message: String::new(),
            marketplace: Some("team".to_string()),
        }),
    );
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).tab, PluginsTab::Discover);
    assert_eq!(menu(&app).query, "@team");
}

#[test]
fn a_marketplace_page_offers_what_it_affords() {
    let git = market("team", true);
    assert_eq!(
        marketplace_actions(&git),
        [
            MarketplaceAction::Browse,
            MarketplaceAction::Update,
            MarketplaceAction::Remove,
            MarketplaceAction::Back
        ]
    );
    // A directory is read in place: always current.
    assert_eq!(
        marketplace_actions(&market("local", false)),
        [
            MarketplaceAction::Browse,
            MarketplaceAction::Remove,
            MarketplaceAction::Back
        ]
    );
    // One listing nothing has nothing to browse.
    let mut empty = market("team", true);
    empty.plugins = 0;
    assert!(!marketplace_actions(&empty).contains(&MarketplaceAction::Browse));
}

#[test]
fn a_marketplace_page_browses_updates_and_removes_it() {
    let mut app = marketplaces_tab(catalog());
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        menu(&app).page,
        PluginsPage::Marketplace("team".to_string())
    );
    // Browse: its plugins, on Discover.
    app.on_key(key(KeyCode::Enter));
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).tab, PluginsTab::Discover);
    assert_eq!(menu(&app).query, "@team");
    assert_eq!(rows(&app), ["hookify@team", "deploy@team", "zeta@team"]);

    let mut app = marketplaces_tab(catalog());
    app.on_key(key(KeyCode::Enter));
    let action = app.on_key(key(KeyCode::Char('2')));
    assert!(
        matches!(action, Action::PluginOp { op: PluginOp::UpdateMarketplace(ref name), .. } if name == "team"),
        "{action:?}"
    );
    assert_eq!(menu(&app).job.as_ref().unwrap().label, "Updating team…");

    // Remove asks first, naming what it uninstalls, Remove highlighted.
    let mut app = marketplaces_tab(catalog());
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('3')));
    assert_eq!(
        menu(&app).page,
        PluginsPage::Confirm(PluginsConfirm::RemoveMarketplace {
            name: "team".to_string(),
            plugins: vec!["hookify".to_string()],
        })
    );
    assert_eq!(menu(&app).selected, 0, "Remove is highlighted");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(
        menu(&app).page,
        PluginsPage::Marketplace("team".to_string())
    );
    app.on_key(key(KeyCode::Char('3')));
    let action = app.on_key(key(KeyCode::Enter));
    let mut after = catalog();
    after.marketplaces.remove(0);
    after.plugins.retain(|p| p.id.marketplace != "team");
    let op = finish(&mut app, action, after, Ok(PluginDone::default()));
    assert_eq!(op, PluginOp::RemoveMarketplace("team".to_string()));
    // Its page is gone: back on the Marketplaces tab.
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).tab, PluginsTab::Marketplaces);
    // Esc from a marketplace's page comes back to its row.
    let mut app = marketplaces_tab(catalog());
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        menu(&app).page,
        PluginsPage::Marketplace("local".to_string())
    );
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).selected, 1);
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
