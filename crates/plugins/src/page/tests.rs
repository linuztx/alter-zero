//! The `/plugins` page's state machine (`docs/plugins-menu.md`).

use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{
    AddRow, ComponentSummary, InstalledState, KeyOutcome, MarketplaceRow, MarketplaceSuggestion,
    PLUGINS_JOB_DELAY, PluginAction, PluginContents, PluginDone, PluginId, PluginOp, PluginRow,
    PluginsConfirm, PluginsPage, PluginsScreen, PluginsSnapshot, PluginsTab, plugin_actions,
};

fn id(key: &str) -> PluginId {
    let (name, marketplace) = key.split_once('@').expect("name@marketplace");
    PluginId::new(name, marketplace).expect("a valid id")
}

fn installed(enabled: bool) -> InstalledState {
    InstalledState {
        version: Some("1.0.0".to_string()),
        enabled,
        ..InstalledState::default()
    }
}

fn plugin(key: &str, state: Option<bool>) -> PluginRow {
    PluginRow {
        id: id(key),
        description: Some(format!("The {key} plugin.")),
        version: Some("1.0.0".to_string()),
        keywords: vec!["tools".to_string()],
        source: "./plugins/x".to_string(),
        contents: PluginContents::Known(ComponentSummary::default()),
        installed: state.map(installed),
        ..PluginRow::default()
    }
}

fn market(name: &str, git: bool) -> MarketplaceRow {
    MarketplaceRow {
        name: name.to_string(),
        source: format!("https://example.com/{name}.git"),
        git,
        plugins: 2,
        installed: 1,
        ..MarketplaceRow::default()
    }
}

fn suggestions() -> Vec<MarketplaceSuggestion> {
    [
        (
            "anthropics/claude-plugins-official",
            "claude-plugins-official",
        ),
        ("openai/plugins", "openai-curated"),
    ]
    .into_iter()
    .map(|(source, name)| MarketplaceSuggestion {
        source: source.to_string(),
        name: name.to_string(),
        description: format!("The {name} catalog"),
    })
    .collect()
}

fn catalog() -> PluginsSnapshot {
    PluginsSnapshot {
        marketplaces: vec![market("team", true), market("local", false)],
        plugins: vec![
            plugin("hookify@team", Some(true)),
            plugin("lint@local", Some(false)),
            plugin("deploy@team", None),
            plugin("zeta@team", None),
        ],
        suggestions: suggestions(),
        error: None,
        session_enabled: true,
    }
}

fn tab(name: &str) -> PluginsTab {
    PluginsTab::Marketplace(name.to_string())
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn type_str(page: &mut PluginsPage, text: &str) {
    for c in text.chars() {
        page.on_key(key(KeyCode::Char(c)));
    }
}

/// The page open over `catalog()`.
fn plugins_app() -> PluginsPage {
    PluginsPage::open(catalog(), 0)
}

fn menu(page: &PluginsPage) -> &PluginsPage {
    page
}

fn rows(page: &PluginsPage) -> Vec<String> {
    page.plugin_rows()
        .iter()
        .map(|p| p.id.to_string())
        .collect()
}

/// The op the action asks for, finished with `catalog`.
fn finish(
    app: &mut PluginsPage,
    action: KeyOutcome,
    catalog: PluginsSnapshot,
    outcome: Result<PluginDone, String>,
) -> PluginOp {
    let KeyOutcome::Op { op, round } = action else {
        panic!("{action:?}");
    };
    assert!(app.finish_job(round, Some(catalog), &outcome));
    op
}

#[test]
fn the_tabs_are_all_installed_each_marketplace_and_add() {
    // codex's `/plugins`: every plugin, the installed ones, a tab per
    // marketplace in the snapshot's order, and the tab that adds one.
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
    let app = PluginsPage::open(PluginsSnapshot::default(), 0);
    assert_eq!(
        menu(&app).tabs(),
        [PluginsTab::All, PluginsTab::Installed, PluginsTab::Add]
    );
}

#[test]
fn the_page_opens_on_its_snapshot_on_the_tab_with_something_on_it() {
    // The boundary reads the catalogs as the page opens, so the first frame
    // already has its rows — no read runs behind them, no placeholder.
    let app = plugins_app();
    assert_eq!(menu(&app).tab, PluginsTab::Installed);
    assert_eq!(rows(&app), ["hookify@team", "lint@local"]);
    assert!(menu(&app).job.is_none());
    assert_eq!(app.job_round(), None);

    // Nothing installed: everything there is.
    let mut fresh = catalog();
    fresh.plugins.iter_mut().for_each(|p| p.installed = None);
    let app = PluginsPage::open(fresh, 0);
    assert_eq!(menu(&app).tab, PluginsTab::All);

    // Nothing at all: where a marketplace is added.
    let app = PluginsPage::open(PluginsSnapshot::default(), 0);
    assert_eq!(menu(&app).tab, PluginsTab::Add);
}

#[test]
fn tabs_move_with_the_arrows_and_tab_and_the_search_survives_between_plugin_tabs() {
    let mut app = plugins_app();
    type_str(&mut app, "o");
    app.on_key(key(KeyCode::Right));
    assert_eq!(menu(&app).tab, tab("team"));
    assert_eq!(menu(&app).query, "o", "the search rides along");
    assert_eq!(rows(&app), ["hookify@team", "deploy@team", "zeta@team"]);
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Tab));
    assert_eq!(menu(&app).tab, tab("local"));
    assert_eq!(rows(&app), ["lint@local"]);
    type_str(&mut app, "lint");
    // `+ Add`'s `❯` line is a source field: entering it clears the search.
    app.on_key(key(KeyCode::Tab));
    assert_eq!(menu(&app).tab, PluginsTab::Add);
    assert_eq!(menu(&app).query, "");
    // …and so does leaving it.
    type_str(&mut app, "acme/x");
    app.on_key(key(KeyCode::Right));
    assert_eq!(menu(&app).tab, PluginsTab::All, "wrapping");
    assert_eq!(menu(&app).query, "");
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
    let mut fresh = catalog();
    fresh.plugins[2].description = Some("Ships builds to staging".to_string());
    fresh.plugins[3].keywords = vec!["kubernetes".to_string()];
    let mut app = PluginsPage::open(fresh, 0);
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
    assert_eq!(app.on_key(key(KeyCode::Esc)), KeyOutcome::Close);
}

#[test]
fn space_turns_an_installed_plugin_on_or_off_at_once() {
    let mut app = plugins_app();
    let action = app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(
        action,
        KeyOutcome::Op {
            op: PluginOp::SetEnabled {
                id: id("hookify@team"),
                enabled: false
            },
            round: app.job_round().unwrap()
        }
    );
    // The row answers before the write does.
    assert!(!menu(&app).plugin_rows()[0].is_enabled());
    assert_eq!(menu(&app).job.as_ref().unwrap().label, "Disabling hookify…");
    // A second op waits for it — refused in the status row, never a toast
    // over the page.
    assert_eq!(app.on_key(key(KeyCode::Down)), KeyOutcome::None);
    assert_eq!(app.on_key(key(KeyCode::Char(' '))), KeyOutcome::None);
    assert_eq!(
        menu(&app).error.as_deref(),
        Some("Disabling hookify — wait for it, or Esc to cancel")
    );
    assert!(menu(&app).plugin_rows()[1].installed.is_some());
    assert!(!menu(&app).plugin_rows()[1].is_enabled(), "lint stays off");
}

#[test]
fn a_failure_is_said_on_the_page_until_the_next_key() {
    // What went right shows on the page itself — the row, the page's state
    // line, the new marketplace's tab — so only a failure is said, and never
    // in a toast row over the page, which would move it down a row and back.
    let mut app = plugins_app();
    let action = app.on_key(key(KeyCode::Char(' ')));
    let KeyOutcome::Op { round, .. } = action else {
        panic!("{action:?}");
    };
    app.finish_job(round, Some(catalog()), &Err("disk full".to_string()));
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
    assert_eq!(app.on_key(key(KeyCode::Char(' '))), KeyOutcome::None);
    assert!(menu(&app).job.is_none());
}

#[test]
fn esc_stops_a_running_job_before_anything_else() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Char(' ')));
    let round = app.job_round().unwrap();
    assert_eq!(app.on_key(key(KeyCode::Esc)), KeyOutcome::CancelJob);
    assert_eq!(app.job_round(), None, "the page let go of it");
    // Its answer, when it comes, is no longer the page's.
    assert!(!app.finish_job(round, Some(catalog()), &Err("cancelled".to_string())));
    assert_eq!(menu(&app).error, None);
    assert_eq!(app.on_key(ctrl('c')), KeyOutcome::Close);
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
    let deploy = id("deploy@team");
    // Its contents are read, so its page asks for nothing.
    assert_eq!(app.on_key(key(KeyCode::Enter)), KeyOutcome::None);
    assert_eq!(menu(&app).screen, PluginsScreen::Plugin(deploy.clone()));
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(menu(&app).job.as_ref().unwrap().label, "Installing deploy…");
    let mut after = catalog();
    after.plugins[2].installed = Some(installed(true));
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
    assert_eq!(menu(&app).screen, PluginsScreen::Plugin(deploy));
    // Its actions are an installed plugin's now; Esc goes back to its row.
    assert_eq!(app.on_key(key(KeyCode::Char('9'))), KeyOutcome::None);
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).screen, PluginsScreen::List);
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
    deploy.installed = Some(installed(true));
    after.plugins.insert(0, deploy);
    finish(&mut app, action, after, Ok(PluginDone::default()));
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).screen, PluginsScreen::List);
    assert_eq!(rows(&app)[menu(&app).selected], "deploy@team");
}

#[test]
fn updating_an_installed_plugin_says_so() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Enter));
    let action = app.on_key(key(KeyCode::Char('2')));
    assert!(matches!(
        action,
        KeyOutcome::Op {
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
    let hookify = id("hookify@team");
    assert_eq!(app.on_key(key(KeyCode::Char('3'))), KeyOutcome::None);
    assert_eq!(
        menu(&app).screen,
        PluginsScreen::Confirm(PluginsConfirm::Uninstall(hookify.clone()))
    );
    assert_eq!(menu(&app).selected, 0, "Uninstall is highlighted");
    // Esc, or Cancel, goes back.
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).screen, PluginsScreen::Plugin(hookify.clone()));
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Char('2')));
    assert_eq!(menu(&app).screen, PluginsScreen::Plugin(hookify.clone()));
    app.on_key(key(KeyCode::Char('3')));
    let action = app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(action, KeyOutcome::Op { op: PluginOp::Uninstall(ref id), .. } if *id == hookify)
    );
    assert_eq!(menu(&app).screen, PluginsScreen::Plugin(hookify));
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
    assert_eq!(menu(&app).screen, PluginsScreen::List);
}

#[test]
fn a_marketplace_tab_updates_with_ctrl_u_and_removes_with_ctrl_r() {
    // codex's keys, on the marketplace's own tab.
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Right));
    assert_eq!(menu(&app).tab, tab("team"));
    let action = app.on_key(ctrl('u'));
    assert!(
        matches!(action, KeyOutcome::Op { op: PluginOp::UpdateMarketplace(ref name), .. } if name == "team"),
        "{action:?}"
    );

    let mut app = plugins_app();
    app.on_key(key(KeyCode::Right));
    assert_eq!(app.on_key(ctrl('r')), KeyOutcome::None);
    assert_eq!(
        menu(&app).screen,
        PluginsScreen::Confirm(PluginsConfirm::RemoveMarketplace {
            name: "team".to_string(),
            plugins: vec!["hookify".to_string()],
        })
    );
    assert_eq!(menu(&app).selected, 0, "Remove is highlighted");
    // Esc, or Cancel, comes back to the tab.
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).screen, PluginsScreen::List);
    assert_eq!(menu(&app).tab, tab("team"));
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Char('2')));
    assert_eq!(menu(&app).screen, PluginsScreen::List);
    assert_eq!(menu(&app).tab, tab("team"));
    app.on_key(ctrl('r'));
    let action = app.on_key(key(KeyCode::Enter));
    let mut after = catalog();
    after.marketplaces.remove(0);
    after.plugins.retain(|p| p.id.marketplace != "team");
    let op = finish(&mut app, action, after, Ok(PluginDone::default()));
    assert_eq!(op, PluginOp::RemoveMarketplace("team".to_string()));
    // Its tab is gone: the page lands on All.
    assert_eq!(menu(&app).screen, PluginsScreen::List);
    assert_eq!(menu(&app).tab, PluginsTab::All);
}

#[test]
fn a_directory_marketplace_has_nothing_to_update() {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Right));
    assert_eq!(menu(&app).tab, tab("local"));
    assert_eq!(app.on_key(ctrl('u')), KeyOutcome::None);
    assert!(
        menu(&app)
            .error
            .as_deref()
            .is_some_and(|text| text.contains("always current"))
    );
    assert!(menu(&app).job.is_none());
    // Neither key means anything on a tab that is no marketplace's.
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Right));
    assert_eq!(menu(&app).tab, PluginsTab::All);
    assert_eq!(app.on_key(ctrl('r')), KeyOutcome::None);
    assert_eq!(menu(&app).screen, PluginsScreen::List);
}

/// The add tab of a fresh page.
fn add_tab() -> PluginsPage {
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    assert_eq!(menu(&app).tab, PluginsTab::Add);
    app
}

fn add_rows(app: &PluginsPage) -> Vec<String> {
    menu(app)
        .add_rows()
        .iter()
        .map(|row| match row {
            AddRow::Typed(source) => format!("typed:{source}"),
            AddRow::Suggestion(index) => menu(app).snapshot.suggestions[*index].source.clone(),
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
        matches!(action, KeyOutcome::Op { op: PluginOp::AddMarketplace(ref source), .. }
            if source == "anthropics/claude-plugins-official"),
        "{action:?}"
    );
    let mut app = add_tab();
    app.on_key(key(KeyCode::Down));
    let action = app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(action, KeyOutcome::Op { op: PluginOp::AddMarketplace(ref source), .. }
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
    let mut app = PluginsPage::open(catalog, 0);
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    assert_eq!(menu(&app).tab, PluginsTab::Add);
    assert_eq!(app.on_key(key(KeyCode::Enter)), KeyOutcome::None);
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
    app.paste("  acme/tools\n");
    assert_eq!(menu(&app).query, "acme/tools");
    let action = app.on_key(key(KeyCode::Enter));
    let KeyOutcome::Op { op, round } = action else {
        panic!("{action:?}");
    };
    assert_eq!(op, PluginOp::AddMarketplace("acme/tools".to_string()));
    app.finish_job(round, None, &Err("not a marketplace".to_string()));
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
    assert_eq!(menu(&app).screen, PluginsScreen::List);
    assert_eq!(menu(&app).tab, tab("team"));
    assert_eq!(menu(&app).query, "");
}

#[test]
fn the_other_row_opens_a_field_for_any_source() {
    let mut app = add_tab();
    app.on_key(key(KeyCode::End));
    assert_eq!(app.on_key(key(KeyCode::Enter)), KeyOutcome::None);
    assert_eq!(menu(&app).screen, PluginsScreen::Add);
    assert_eq!(menu(&app).input.text(), "");
    // Space types here: a directory's path may hold one.
    type_str(&mut app, "/srv/my plugins");
    assert_eq!(menu(&app).input.text(), "/srv/my plugins");
    // Esc goes back to the row it came from.
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).screen, PluginsScreen::List);
    assert_eq!(menu(&app).tab, PluginsTab::Add);
    assert_eq!(menu(&app).add_rows()[menu(&app).selected], AddRow::Other);

    // Pasted, then added — a failure says why and keeps what was typed.
    let mut app = add_tab();
    app.on_key(key(KeyCode::End));
    app.on_key(key(KeyCode::Enter));
    app.paste("acme/tools\n");
    assert_eq!(menu(&app).input.text(), "acme/tools");
    let action = app.on_key(key(KeyCode::Enter));
    let KeyOutcome::Op { op, round } = action else {
        panic!("{action:?}");
    };
    assert_eq!(op, PluginOp::AddMarketplace("acme/tools".to_string()));
    app.finish_job(round, None, &Err("not a marketplace".to_string()));
    assert_eq!(menu(&app).screen, PluginsScreen::Add);
    assert_eq!(menu(&app).input.text(), "acme/tools");
    assert_eq!(menu(&app).error.as_deref(), Some("not a marketplace"));
    // Editing clears it; Ctrl+U empties the field; Enter on an empty one
    // asks for nothing.
    app.on_key(key(KeyCode::Backspace));
    assert!(menu(&app).error.is_none());
    app.on_key(ctrl('u'));
    assert_eq!(menu(&app).input.text(), "");
    assert_eq!(app.on_key(key(KeyCode::Enter)), KeyOutcome::None);
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
    assert_eq!(menu(&app).screen, PluginsScreen::List);
    assert_eq!(menu(&app).tab, tab("team"));
    assert_eq!(menu(&app).input.text(), "");
}

#[test]
fn a_plugins_page_asks_for_its_contents_once_and_takes_the_answer() {
    // A plugin not yet installed says what it holds only when its page asks
    // (the boundary reads the marketplace's file listing) — and the answer
    // is no job: the page keeps working, under round 0.
    let mut fresh = catalog();
    fresh.plugins[2].contents = PluginContents::NotRead;
    let mut app = PluginsPage::open(fresh, 0);
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Down));
    let deploy = id("deploy@team");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        KeyOutcome::Op {
            op: PluginOp::Inspect(deploy.clone()),
            round: 0
        }
    );
    assert_eq!(app.job_round(), None);
    assert_eq!(menu(&app).inspecting, Some(deploy.clone()));
    assert_eq!(app.pending_inspect(), None, "already asked");
    let preview = ComponentSummary {
        skills: vec!["deploy:ship".to_string()],
        has_hooks: true,
        preview: true,
        ..ComponentSummary::default()
    };
    assert!(app.supply_contents(&deploy, PluginContents::Known(preview.clone())));
    assert_eq!(menu(&app).inspecting, None);
    assert_eq!(
        menu(&app).plugin(&deploy).unwrap().contents,
        PluginContents::Known(preview.clone())
    );
    // A snapshot read since (when the page opened) keeps what was learnt.
    let mut again = catalog();
    again.plugins[2].contents = PluginContents::NotRead;
    app.update_snapshot(again);
    assert_eq!(
        menu(&app).plugin(&deploy).unwrap().contents,
        PluginContents::Known(preview)
    );
    // Back, and in again: nothing more to ask.
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.on_key(key(KeyCode::Enter)), KeyOutcome::None);
    // A plugin the snapshot no longer lists takes no answer.
    let gone = id("gone@team");
    assert!(!app.supply_contents(&gone, PluginContents::Unknown));
}

#[test]
fn an_install_reads_the_plugins_contents_again() {
    // What a preview said is the marketplace's listing; installed, the
    // plugin's own files say — so its page has the boundary read them.
    let mut app = plugins_app();
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    let action = app.on_key(key(KeyCode::Enter));
    let KeyOutcome::Op { round, .. } = action else {
        panic!("{action:?}");
    };
    assert!(app.finish_job(round, None, &Ok(PluginDone::default())));
    let deploy = id("deploy@team");
    assert_eq!(
        menu(&app).plugin(&deploy).unwrap().contents,
        PluginContents::NotRead
    );
    assert_eq!(app.pending_inspect(), Some(deploy));
}

#[test]
fn a_job_is_said_only_once_it_has_run_long_enough() {
    let mut app = plugins_app();
    app.set_clock(1_000, Duration::from_secs(50));
    app.on_key(key(KeyCode::Char(' ')));
    assert!(app.job_round().is_some(), "a job wants frames");
    // Its clock starts at the first draw after it began.
    app.set_clock(1_000, Duration::from_secs(60));
    assert!(!menu(&app).job_shown());
    let just_short = Duration::from_secs(60) + PLUGINS_JOB_DELAY - Duration::from_millis(1);
    app.set_clock(1_000, just_short);
    assert!(!menu(&app).job_shown(), "a quick toggle says nothing");
    app.set_clock(1_000, Duration::from_secs(64));
    assert!(menu(&app).job_shown());
    assert_eq!(menu(&app).job_elapsed(), Duration::from_secs(4));
    assert_eq!(menu(&app).now, 1_000);
    // The boundary's word on it rides along — for this job only.
    let round = app.job_round().unwrap();
    assert!(!app.report_step(round + 1, "nope"));
    assert!(app.report_step(round, "writing plugins.json"));
    assert_eq!(
        menu(&app).job.as_ref().unwrap().step.as_deref(),
        Some("writing plugins.json")
    );
    app.finish_job(round, None, &Ok(PluginDone::default()));
    assert!(app.job_round().is_none(), "and none once it is done");
}

#[test]
fn a_failed_toggle_with_no_fresh_snapshot_moves_its_row_back() {
    let mut app = plugins_app();
    let KeyOutcome::Op { round, .. } = app.on_key(key(KeyCode::Char(' '))) else {
        panic!("a toggle is a job");
    };
    assert!(!menu(&app).plugin_rows()[0].is_enabled());
    app.finish_job(round, None, &Err("plugins.json is read-only".to_string()));
    assert!(menu(&app).plugin_rows()[0].is_enabled(), "back on");
    assert_eq!(
        menu(&app).error.as_deref(),
        Some("plugins.json is read-only")
    );
}

#[test]
fn an_update_leads_a_plugins_actions_when_there_is_one() {
    // The primary action first: the one the page was opened for.
    let mut stale = plugin("a@m", Some(true));
    stale.installed.as_mut().unwrap().update = true;
    assert_eq!(
        plugin_actions(&stale),
        [
            PluginAction::Update,
            PluginAction::Disable,
            PluginAction::Uninstall,
            PluginAction::Back
        ]
    );
    assert_eq!(PluginAction::Back.label(), "Back to plugins");
}

#[test]
fn a_preview_names_hooks_and_servers_without_counts() {
    let preview = ComponentSummary {
        skills: vec!["a:x".into(), "a:y".into()],
        has_hooks: true,
        has_servers: true,
        preview: true,
        ..ComponentSummary::default()
    };
    assert_eq!(preview.line(), "2 skills · hooks · MCP servers");
    assert!(preview.runs_code());
    let exact = ComponentSummary {
        commands: vec!["/a:go".into()],
        hooks: vec!["Stop (2)".into()],
        hook_count: 2,
        servers: vec!["db".into()],
        ..ComponentSummary::default()
    };
    assert_eq!(exact.line(), "1 command · 2 hooks · 1 MCP server");
    assert_eq!(ComponentSummary::default().line(), "nothing this app loads");
    assert!(!ComponentSummary::default().runs_code());
}

// ===== the add page's own line editor =====

#[test]
fn the_field_edits_at_its_caret() {
    let mut field = super::LineField::default();
    for c in "acme/tols".chars() {
        field.insert_char(c);
    }
    field.move_left();
    field.move_left();
    field.insert_char('o');
    assert_eq!(field.text(), "acme/tools");
    field.move_home();
    field.delete_forward();
    field.move_end();
    field.delete_backward();
    assert_eq!(field.text(), "cme/tool");
    field.move_home();
    field.insert_str("~/");
    assert_eq!(field.text(), "~/cme/tool");
    assert_eq!(field.caret(), 2);
    field.clear();
    assert!(field.is_empty());
    // Backspace at the start and Delete at the end do nothing.
    field.delete_backward();
    field.delete_forward();
    assert!(field.is_empty());
}

#[test]
fn the_field_wraps_by_columns_and_keeps_its_caret_a_cell() {
    let mut field = super::LineField::default();
    field.insert_str("abcdef");
    // A caret at the end of a full row starts the next one.
    assert_eq!(
        field.rows(3),
        (vec!["abc".into(), "def".into(), String::new()], (2, 0))
    );
    field.move_home();
    field.move_right();
    assert_eq!(field.rows(4).1, (0, 1));
    // A wide glyph never splits across rows, and counts two columns.
    let mut wide = super::LineField::default();
    wide.insert_str("a漢字");
    assert_eq!(wide.rows(4).0, ["a漢", "字"]);
    assert_eq!(wide.rows(4).1, (1, 2));
}

#[test]
fn a_reopened_page_continues_its_rounds() {
    let mut page = PluginsPage::open(catalog(), 41);
    let KeyOutcome::Op { round, .. } = page.on_key(key(KeyCode::Char(' '))) else {
        panic!("a toggle is a job");
    };
    assert_eq!(round, 42);
    assert_eq!(page.last_round(), 42);
}
