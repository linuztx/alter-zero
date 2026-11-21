//! The `/plugins` page's state machine (`docs/plugins.md`).

use super::*;
use crate::app::{
    AddRow, PluginAction, PluginDone, PluginOp, PluginsConfirm, PluginsPage, PluginsTab,
    plugin_actions,
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
        unread: None,
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

fn tab(name: &str) -> PluginsTab {
    PluginsTab::Marketplace(name.to_string())
}

/// An app with the page open on `catalog()`.
fn plugins_app() -> App {
    let mut app = App::new();
    app.open_plugins_menu(true, catalog());
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
fn the_tabs_are_all_installed_each_marketplace_and_add() {
    // codex's `/plugins`: every plugin, the installed ones, a tab per
    // marketplace in the catalog's order, and the tab that adds one.
    let app = plugins_app();
    assert_eq!(
        menu(&app).tabs(),
        [
            PluginsTab::All,
            PluginsTab::Installed,
            tab("team"),
            tab("local"),
            PluginsTab::Add
        ]
    );
    // Nothing added: the fixed tabs alone.
    let mut app = App::new();
    app.open_plugins_menu(true, PluginCatalog::default());
    assert_eq!(
        menu(&app).tabs(),
        [PluginsTab::All, PluginsTab::Installed, PluginsTab::Add]
    );
}

#[test]
fn the_page_opens_on_its_catalog_on_the_tab_with_something_on_it() {
    // The boundary reads the catalog as the page opens, so the first frame
    // has its rows: no job runs, and nothing waits to push them in.
    let mut app = App::new();
    app.open_plugins_menu(true, catalog());
    assert_eq!(menu(&app).catalog, catalog());
    assert!(menu(&app).job.is_none());
    assert_eq!(app.plugins_job_round(), None);
    assert_eq!(menu(&app).tab, PluginsTab::Installed);
    assert_eq!(rows(&app), ["hookify@team", "lint@local"]);

    // Nothing installed: everything there is.
    let mut app = App::new();
    let mut fresh = catalog();
    fresh.plugins.iter_mut().for_each(|p| p.installed = None);
    app.open_plugins_menu(true, fresh);
    assert_eq!(menu(&app).tab, PluginsTab::All);

    // Nothing at all: where a marketplace is added.
    let mut app = App::new();
    app.open_plugins_menu(true, PluginCatalog::default());
    assert_eq!(menu(&app).tab, PluginsTab::Add);
}

#[test]
fn a_closed_page_keeps_nothing_of_its_catalog() {
    // Nothing about plugins stays in memory while the page is closed: the
    // next open shows what the boundary reads then, and only that.
    let mut app = plugins_app();
    app.close_plugins_menu();
    assert!(app.plugins_menu.is_none());
    app.open_plugins_menu(true, PluginCatalog::default());
    assert_eq!(menu(&app).catalog, PluginCatalog::default());
    assert_eq!(menu(&app).tab, PluginsTab::Add);
}

#[test]
fn a_late_answer_is_dropped() {
    let mut app = plugins_app();
    let Action::PluginOp { round: first, .. } = app.on_key(key(KeyCode::Char(' '))) else {
        panic!("Space asks to turn the plugin off");
    };
    app.close_plugins_menu();
    app.open_plugins_menu(true, catalog());
    assert!(!app.finish_plugin_job(first, Some(catalog()), &Ok(PluginDone::default())));
    let Action::PluginOp { round: second, .. } = app.on_key(key(KeyCode::Char(' '))) else {
        panic!("Space asks to turn the plugin off");
    };
    assert_ne!(first, second);
    assert!(app.finish_plugin_job(second, Some(catalog()), &Ok(PluginDone::default())));
    assert!(!app.finish_plugin_job(second, Some(catalog()), &Ok(PluginDone::default())));
    app.close_plugins_menu();
    assert!(!app.finish_plugin_job(second, Some(catalog()), &Ok(PluginDone::default())));
}

#[test]
fn the_page_asks_for_a_plugin_s_contents_as_it_shows_it() {
    // A plugin the catalog listed from its manifest alone is read when the
    // page shows what it adds: highlighted in the list, or its own page.
    let unread = |key: &str| {
        let mut p = plugin(key, None);
        p.components = None;
        p.unread = Some(Box::new(crate::plugins::UnreadPlugin {
            root: std::path::PathBuf::from("/m/team/plugins").join(key),
            entry: crate::plugins::MarketplaceEntry {
                name: key.split('@').next().unwrap().to_string(),
                source: crate::plugins::PluginSource::Local {
                    path: "./x".to_string(),
                },
                display_name: None,
                description: None,
                version: None,
                author: None,
                category: None,
                homepage: None,
                keywords: Vec::new(),
                manifest_fields: None,
            },
        }));
        p
    };
    let mut fresh = catalog();
    fresh.plugins[2] = unread("deploy@team");
    fresh.plugins[3] = unread("zeta@team");
    let mut app = App::new();
    app.open_plugins_menu(true, fresh);
    // The Installed tab's rows are read already.
    assert_eq!(app.plugin_to_read(), None);
    app.on_key(key(KeyCode::Left));
    assert_eq!(menu(&app).tab, PluginsTab::All);
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(rows(&app)[menu(&app).selected], "deploy@team");
    let asked = app.plugin_to_read().cloned().expect("the highlighted row");
    assert_eq!(asked.id.key(), "deploy@team");
    // The boundary reads it and hands it back; the row keeps what was read.
    let mut read = asked;
    read.unread = None;
    read.components = Some(ComponentSummary {
        skills: vec!["deploy:ship".to_string()],
        ..ComponentSummary::default()
    });
    app.supply_plugin_contents(read.clone());
    assert_eq!(app.plugin_to_read(), None);
    assert_eq!(menu(&app).plugin(&read.id), Some(&read));
    // The next row down, on its own page.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(&menu(&app).page, PluginsPage::Plugin(id) if id.key() == "zeta@team"));
    assert_eq!(
        app.plugin_to_read().map(|p| p.id.key()).as_deref(),
        Some("zeta@team")
    );
    // A plugin no longer in the catalog is not put back.
    let mut stray = plugin("stray@team", None);
    stray.unread = None;
    app.supply_plugin_contents(stray.clone());
    assert_eq!(menu(&app).plugin(&stray.id), None);
    // Nothing is asked on the add tab, or with the page closed.
    app.close_plugins_menu();
    assert_eq!(app.plugin_to_read(), None);
}

#[test]
fn tabs_move_with_the_arrows_and_tab_and_clear_the_search() {
    let mut app = plugins_app();
    type_str(&mut app, "hook");
    app.on_key(key(KeyCode::Right));
    assert_eq!(menu(&app).tab, tab("team"));
    assert_eq!(menu(&app).query, "");
    assert_eq!(rows(&app), ["hookify@team", "deploy@team", "zeta@team"]);
    app.on_key(key(KeyCode::Tab));
    assert_eq!(menu(&app).tab, tab("local"));
    assert_eq!(rows(&app), ["lint@local"]);
    app.on_key(key(KeyCode::Tab));
    assert_eq!(menu(&app).tab, PluginsTab::Add);
    app.on_key(key(KeyCode::Right));
    assert_eq!(menu(&app).tab, PluginsTab::All, "wrapping");
    assert_eq!(
        rows(&app),
        ["hookify@team", "lint@local", "deploy@team", "zeta@team"]
    );
    app.on_key(key(KeyCode::BackTab));
    assert_eq!(menu(&app).tab, PluginsTab::Add);
    app.on_key(key(KeyCode::Left));
    assert_eq!(menu(&app).tab, tab("local"));
}

#[test]
fn the_search_matches_names_marketplaces_descriptions_and_keywords() {
    let mut app = App::new();
    let mut fresh = catalog();
    fresh.plugins[2].description = Some("Ships builds to staging".to_string());
    fresh.plugins[3].keywords = vec!["kubernetes".to_string()];
    app.open_plugins_menu(true, fresh);
    app.on_key(key(KeyCode::Left));
    assert_eq!(menu(&app).tab, PluginsTab::All);
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
fn a_failure_is_said_on_the_page_until_the_next_key() {
    // What went right shows on the page itself — the row, the page's state
    // line, the new marketplace's tab — so only a failure is said, and never
    // in a toast row over the page, which would move it down a row and back.
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
    app.on_key(key(KeyCode::Left));
    assert_eq!(menu(&app).tab, PluginsTab::All);
    // deploy is the third row of All.
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

#[test]
fn a_marketplace_tab_updates_with_ctrl_u_and_removes_with_ctrl_r() {
    // codex's keys, on the marketplace's own tab.
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Right));
    assert_eq!(menu(&app).tab, tab("team"));
    let action = app.on_key(ctrl('u'));
    assert!(
        matches!(action, Action::PluginOp { op: PluginOp::UpdateMarketplace(ref name), .. } if name == "team"),
        "{action:?}"
    );

    let mut app = plugins_app();
    app.on_key(key(KeyCode::Right));
    assert_eq!(app.on_key(ctrl('r')), Action::None);
    assert_eq!(
        menu(&app).page,
        PluginsPage::Confirm(PluginsConfirm::RemoveMarketplace {
            name: "team".to_string(),
            plugins: vec!["hookify".to_string()],
        })
    );
    assert_eq!(menu(&app).selected, 0, "Remove is highlighted");
    // Esc, or Cancel, comes back to the tab.
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).tab, tab("team"));
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('2')));
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).tab, tab("team"));
    app.on_key(ctrl('r'));
    let action = app.on_key(key(KeyCode::Enter));
    let mut after = catalog();
    after.marketplaces.remove(0);
    after.plugins.retain(|p| p.id.marketplace != "team");
    let op = finish(&mut app, action, after, Ok(PluginDone::default()));
    assert_eq!(op, PluginOp::RemoveMarketplace("team".to_string()));
    // Its tab is gone: the page lands on All.
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).tab, PluginsTab::All);
}

#[test]
fn a_directory_marketplace_has_nothing_to_update() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Right));
    assert_eq!(menu(&app).tab, tab("local"));
    assert!(
        matches!(app.on_key(ctrl('u')), Action::Toast(text) if text.contains("always current"))
    );
    assert!(menu(&app).job.is_none());
    // Neither key means anything on a tab that is no marketplace's.
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Right));
    assert_eq!(menu(&app).tab, PluginsTab::All);
    assert_eq!(app.on_key(ctrl('r')), Action::None);
    assert_eq!(menu(&app).page, PluginsPage::List);
}

/// The add tab of a fresh page.
fn add_tab() -> App {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    assert_eq!(menu(&app).tab, PluginsTab::Add);
    app
}

fn add_rows(app: &App) -> Vec<String> {
    menu(app)
        .add_rows()
        .iter()
        .map(|row| match row {
            AddRow::Typed(source) => format!("typed:{source}"),
            AddRow::Suggestion(suggestion) => suggestion.source.to_string(),
            AddRow::Other => "other".to_string(),
        })
        .collect()
}

#[test]
fn the_add_tab_offers_the_official_marketplaces_and_any_other() {
    // Claude Code's and codex's own catalogs are one Enter away — nobody
    // types `owner/repo` to get them — and the row under them says any
    // other source can be added too.
    let mut app = add_tab();
    assert_eq!(
        add_rows(&app),
        [
            "anthropics/claude-plugins-official",
            "openai/plugins",
            "other"
        ]
    );
    let action = app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(action, Action::PluginOp { op: PluginOp::AddMarketplace(ref source), .. }
            if source == "anthropics/claude-plugins-official"),
        "{action:?}"
    );
    let mut app = add_tab();
    app.on_key(key(KeyCode::Down));
    let action = app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(action, Action::PluginOp { op: PluginOp::AddMarketplace(ref source), .. }
            if source == "openai/plugins"),
        "{action:?}"
    );
}

#[test]
fn a_suggestion_already_added_opens_its_tab() {
    let mut catalog = catalog();
    let mut official = market("claude-plugins-official", true);
    official.source = "https://github.com/anthropics/claude-plugins-official.git".to_string();
    catalog.marketplaces.push(official);
    let mut app = App::new();
    app.open_plugins_menu(true, catalog);
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    assert_eq!(menu(&app).tab, PluginsTab::Add);
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(menu(&app).tab, tab("claude-plugins-official"));
}

#[test]
fn a_typed_source_is_added_and_lands_on_its_tab() {
    let mut app = add_tab();
    // Space types here: a directory's path may hold one.
    type_str(&mut app, "/srv/my plugins");
    assert_eq!(menu(&app).query, "/srv/my plugins");
    assert_eq!(add_rows(&app), ["typed:/srv/my plugins"]);
    // A source that names a suggestion's owner shows the suggestion too.
    let mut app = add_tab();
    type_str(&mut app, "openai");
    assert_eq!(add_rows(&app), ["typed:openai", "openai/plugins"]);
    // Pasted, then added; a failure says why and keeps what was typed.
    let mut app = add_tab();
    app.on_paste("  acme/tools\n");
    assert_eq!(menu(&app).query, "acme/tools");
    let action = app.on_key(key(KeyCode::Enter));
    let Action::PluginOp { op, round } = action else {
        panic!("{action:?}");
    };
    assert_eq!(op, PluginOp::AddMarketplace("acme/tools".to_string()));
    app.finish_plugin_job(round, None, &Err("not a marketplace".to_string()));
    assert_eq!(menu(&app).tab, PluginsTab::Add);
    assert_eq!(menu(&app).query, "acme/tools");
    assert_eq!(menu(&app).error.as_deref(), Some("not a marketplace"));
    // Editing clears it; Ctrl+U empties the field; a success lands on the
    // new marketplace's tab.
    app.on_key(key(KeyCode::Backspace));
    assert!(menu(&app).error.is_none());
    app.on_key(ctrl('u'));
    assert_eq!(menu(&app).query, "");
    type_str(&mut app, "acme/team");
    let action = app.on_key(key(KeyCode::Enter));
    finish(
        &mut app,
        action,
        catalog(),
        Ok(PluginDone {
            message: "Added team — 3 plugins".into(),
            marketplace: Some("team".to_string()),
        }),
    );
    assert_eq!(menu(&app).page, PluginsPage::List);
    assert_eq!(menu(&app).tab, tab("team"));
    assert_eq!(menu(&app).query, "");
}

#[test]
fn the_other_row_opens_a_field_for_any_source() {
    let mut app = add_tab();
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
    assert_eq!(menu(&app).tab, PluginsTab::Add);
    assert_eq!(menu(&app).add_rows()[menu(&app).selected], AddRow::Other);

    // Pasted, then added — a failure says why and keeps what was typed.
    let mut app = add_tab();
    app.on_key(key(KeyCode::End));
    app.on_key(key(KeyCode::Enter));
    app.on_paste("acme/tools\n");
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
    // A success lands on the new marketplace's tab, the field emptied.
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
    assert_eq!(menu(&app).tab, tab("team"));
    assert_eq!(menu(&app).input.text(), "");
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
