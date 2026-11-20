//! The `/plugins` page's pure state and keys (`docs/plugins.md`).

use super::*;

use crate::plugins::catalog::{
    CatalogMarketplace, CatalogPlugin, Components, PluginCatalog, PluginDetail, PluginStatus,
};
use crate::plugins::id::PluginId;
use crate::plugins::manifest::ManifestFormat;

fn marketplace(name: &str, source: &str) -> CatalogMarketplace {
    CatalogMarketplace {
        skipped: Vec::new(),
        name: name.to_string(),
        title: name.to_string(),
        source: source.to_string(),
        web_url: None,
        is_default: true,
        is_local: false,
        fetched: true,
        updated: Some(1_000),
        revision: Some("d7d32797e7e0".to_string()),
        error: None,
    }
}

fn plugin(name: &str, marketplace: usize, status: PluginStatus) -> CatalogPlugin {
    CatalogPlugin {
        installed_version: None,
        name: name.to_string(),
        display_name: None,
        marketplace,
        description: format!("What {name} does"),
        category: None,
        author: None,
        version: Some("1.0.0".to_string()),
        status,
        update_available: false,
    }
}

/// Two marketplaces; two plugins installed from the first, one from the
/// second (with an update), the rest available.
fn catalog() -> PluginCatalog {
    let mut linear = plugin("linear", 1, PluginStatus::Enabled);
    linear.update_available = true;
    PluginCatalog {
        marketplaces: vec![
            marketplace(
                "claude-plugins-official",
                "anthropics/claude-plugins-official",
            ),
            marketplace("openai-curated", "openai/plugins"),
        ],
        plugins: vec![
            plugin("code-review", 0, PluginStatus::Enabled),
            plugin("feature-dev", 0, PluginStatus::Disabled),
            linear,
            plugin("agent-sdk-dev", 0, PluginStatus::Available),
            plugin("github", 0, PluginStatus::Available),
            plugin("notion", 1, PluginStatus::Available),
            plugin("vercel", 1, PluginStatus::Unsupported),
        ],
    }
}

fn id(name: &str, marketplace: &str) -> PluginId {
    PluginId::new(name, marketplace).expect("a valid id")
}

fn detail(name: &str, status: PluginStatus) -> PluginDetail {
    PluginDetail {
        installed_version: None,
        id: id(name, "claude-plugins-official"),
        display_name: None,
        description: format!("What {name} does, at length"),
        version: Some("1.0.0".to_string()),
        author: Some("Anthropic".to_string()),
        category: Some("development".to_string()),
        homepage: None,
        license: None,
        source: format!("plugins/{name}"),
        source_url: None,
        format: ManifestFormat::Claude,
        status,
        update_available: false,
        blocked: None,
        components: Some(Components::default()),
        warnings: Vec::new(),
    }
}

/// An open page with [`catalog`] landed.
fn browsing() -> (App, u64) {
    let mut app = App::new();
    let round = app.open_plugins_menu();
    app.apply_plugin_event(round, PluginEvent::Catalog(catalog()));
    (app, round)
}

fn menu(app: &App) -> &PluginsMenu {
    app.plugins_menu.as_ref().expect("the page is open")
}

/// The names of the plugin rows the current tab lists.
fn row_names(app: &App) -> Vec<String> {
    let menu = menu(app);
    let catalog = menu.catalog.as_ref().expect("a catalog");
    menu.rows()
        .into_iter()
        .map(|row| match row {
            BrowseRow::Plugin(i) => catalog.plugins[i].name.clone(),
            BrowseRow::Marketplace(i) => catalog.marketplaces[i].name.clone(),
            BrowseRow::AddMarketplace => "+add".to_string(),
            BrowseRow::Restore(source) => format!("+restore {source}"),
        })
        .collect()
}

fn highlighted_name(app: &App) -> String {
    let menu = menu(app);
    let catalog = menu.catalog.as_ref().expect("a catalog");
    match menu.highlighted().expect("a row") {
        BrowseRow::Plugin(i) => catalog.plugins[i].name.clone(),
        BrowseRow::Marketplace(i) => catalog.marketplaces[i].name.clone(),
        BrowseRow::AddMarketplace => "+add".to_string(),
        BrowseRow::Restore(source) => format!("+restore {source}"),
    }
}

/// Move the highlight down onto the row named `name`.
fn select(app: &mut App, name: &str) {
    for _ in 0..40 {
        if highlighted_name(app) == name {
            return;
        }
        app.on_key(key(KeyCode::Down));
    }
    panic!("{name} is not listed: {:?}", row_names(app));
}

/// [`browsing`] on the Plugin page of `name`, its detail landed.
fn on_plugin_page(name: &str, status: PluginStatus) -> (App, u64) {
    let (mut app, round) = browsing();
    select(&mut app, name);
    app.on_key(key(KeyCode::Enter));
    app.apply_plugin_event(
        round,
        PluginEvent::Detail {
            id: id(name, "claude-plugins-official"),
            result: Ok(Box::new(detail(name, status))),
        },
    );
    (app, round)
}

#[test]
fn the_palette_lists_plugins_right_after_skills() {
    let names: Vec<String> = COMMANDS.iter().map(|c| c.name.to_string()).collect();
    let skills = names.iter().position(|n| n == "skills").expect("/skills");
    assert_eq!(names[skills + 1], "plugins");
    assert_eq!(
        names.last().map(String::as_str),
        Some("quit"),
        "/quit stays last"
    );
    let row = COMMANDS.iter().find(|c| c.name == "plugins").unwrap();
    assert_eq!(row.description, "Browse, install and manage plugins");
}

#[test]
fn running_plugins_from_the_palette_asks_the_loop_to_open_the_page() {
    let mut app = App::new();
    type_str(&mut app, "/plugins");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenPluginsMenu);
}

#[test]
fn opening_hands_out_a_fresh_round_on_browse_all_with_nothing_loaded() {
    let mut app = App::new();
    let first = app.open_plugins_menu();
    let page = menu(&app);
    assert_eq!(page.page, PluginsPage::Browse);
    assert_eq!(page.tab, PluginsTab::All);
    assert_eq!(page.catalog, None);
    assert_eq!(page.round, first);
    app.close_plugins_menu();
    let second = app.open_plugins_menu();
    assert!(
        second > first,
        "a reopened page is never named by an old round"
    );
}

#[test]
fn an_answer_for_another_round_or_a_closed_page_is_dropped() {
    let mut app = App::new();
    let old = app.open_plugins_menu();
    app.close_plugins_menu();
    app.apply_plugin_event(old, PluginEvent::Catalog(catalog()));
    assert!(app.plugins_menu.is_none(), "a closed page stays closed");
    let round = app.open_plugins_menu();
    app.apply_plugin_event(old, PluginEvent::Catalog(catalog()));
    assert_eq!(
        menu(&app).catalog,
        None,
        "the old round's answer is dropped"
    );
    app.apply_plugin_event(round, PluginEvent::Catalog(catalog()));
    assert!(menu(&app).catalog.is_some());
}

#[test]
fn closing_drops_the_catalog_with_the_page() {
    let (mut app, _) = browsing();
    assert_eq!(
        app.on_key(key(KeyCode::Esc)),
        Action::ClosePluginsMenu,
        "Esc on Browse with no query closes"
    );
    assert!(app.plugins_menu.is_none());
    let round = app.open_plugins_menu();
    assert_eq!(menu(&app).catalog, None, "nothing was kept for the reopen");
    assert_eq!(menu(&app).round, round);
}

#[test]
fn the_tabs_are_all_installed_each_marketplace_and_marketplaces() {
    let (app, _) = browsing();
    assert_eq!(
        menu(&app).tabs(),
        vec![
            PluginsTab::All,
            PluginsTab::Installed,
            PluginsTab::Marketplace("claude-plugins-official".to_string()),
            PluginsTab::Marketplace("openai-curated".to_string()),
            PluginsTab::Marketplaces,
        ]
    );
}

#[test]
fn left_right_and_tab_step_through_the_tabs_wrapping() {
    let (mut app, _) = browsing();
    app.on_key(key(KeyCode::Right));
    assert_eq!(menu(&app).tab, PluginsTab::Installed);
    app.on_key(key(KeyCode::Tab));
    assert_eq!(
        menu(&app).tab,
        PluginsTab::Marketplace("claude-plugins-official".to_string())
    );
    app.on_key(key(KeyCode::BackTab));
    app.on_key(key(KeyCode::Left));
    assert_eq!(menu(&app).tab, PluginsTab::All);
    app.on_key(key(KeyCode::Left));
    assert_eq!(menu(&app).tab, PluginsTab::Marketplaces, "wraps");
}

#[test]
fn the_installed_tab_lists_installed_plugins_first_by_name() {
    let (mut app, _) = browsing();
    assert_eq!(
        row_names(&app)[..3],
        ["code-review", "feature-dev", "linear"]
    );
    app.on_key(key(KeyCode::Right));
    assert_eq!(row_names(&app), ["code-review", "feature-dev", "linear"]);
}

#[test]
fn the_marketplaces_tab_lists_marketplaces_then_add_then_each_removed_default() {
    let (mut app, round) = browsing();
    app.apply_plugin_event(
        round,
        PluginEvent::RemovedDefaults(vec!["acme/plugins".to_string()]),
    );
    app.on_key(key(KeyCode::Left));
    assert_eq!(
        row_names(&app),
        [
            "claude-plugins-official",
            "openai-curated",
            "+add",
            "+restore acme/plugins"
        ]
    );
    type_str(&mut app, "openai");
    assert_eq!(
        row_names(&app),
        ["openai-curated", "+add", "+restore acme/plugins"],
        "the search filters the marketplaces by name or source"
    );
}

#[test]
fn typing_filters_the_tab_and_returns_the_highlight_to_the_first_row() {
    let (mut app, _) = browsing();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    type_str(&mut app, "no");
    assert_eq!(menu(&app).query, "no");
    assert_eq!(row_names(&app), ["notion"]);
    assert_eq!(highlighted_name(&app), "notion");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(menu(&app).query, "n");
}

#[test]
fn clearing_the_search_keeps_the_plugin_that_was_highlighted() {
    let (mut app, _) = browsing();
    type_str(&mut app, "notion");
    assert_eq!(highlighted_name(&app), "notion");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).query, "");
    assert_eq!(highlighted_name(&app), "notion", "Esc keeps it");
    type_str(&mut app, "gi");
    assert_eq!(
        highlighted_name(&app),
        "github",
        "typing goes to the best match"
    );
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(
        highlighted_name(&app),
        "github",
        "still typing: the best match"
    );
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(menu(&app).query, "");
    assert_eq!(
        highlighted_name(&app),
        "github",
        "Backspace to empty keeps it"
    );
}

#[test]
fn ctrl_u_clears_the_search_and_the_add_field() {
    let ctrl_u = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
    let (mut app, _) = browsing();
    type_str(&mut app, "linear");
    assert_eq!(app.on_key(ctrl_u), Action::None);
    assert_eq!(menu(&app).query, "");
    assert_eq!(highlighted_name(&app), "linear");
    app.on_key(key(KeyCode::Left));
    select(&mut app, "+add");
    app.on_key(key(KeyCode::Enter));
    type_str(&mut app, "acme/plugins");
    app.on_key(ctrl_u);
    assert_eq!(menu(&app).input, "");
    assert_eq!(menu(&app).page, PluginsPage::AddMarketplace);
}

#[test]
fn a_failure_whose_reason_names_the_target_does_not_name_it_twice() {
    let (mut app, round) = browsing();
    app.on_key(key(KeyCode::Left));
    select(&mut app, "+add");
    app.on_key(key(KeyCode::Enter));
    let path = "/home/someone/work/a-folder-with-a-very-long-name/marketplaces/team";
    type_str(&mut app, path);
    app.on_key(key(KeyCode::Enter));
    app.apply_plugin_event(
        round,
        PluginEvent::JobFinished {
            result: Err(format!("{path} does not exist")),
        },
    );
    assert_eq!(
        menu(&app).notice.as_ref().unwrap().text,
        format!("Couldn't add the marketplace — {path} does not exist")
    );
}

#[test]
fn space_is_typed_into_a_search_already_begun() {
    let (mut app, _) = browsing();
    type_str(&mut app, "code review");
    assert_eq!(menu(&app).query, "code review");
    assert_eq!(menu(&app).page, PluginsPage::Browse);
    assert_eq!(
        menu(&app).catalog.as_ref().unwrap().plugins[0].status,
        PluginStatus::Enabled,
        "typing a phrase changes no plugin"
    );
}

#[test]
fn no_phrase_typed_into_the_list_starts_an_operation() {
    // Every printable key, in phrases led by a letter, a digit and a mark,
    // spaces and digits among them, from an installed row and an available
    // one: nothing but a search happens until Enter.
    let printable: Vec<char> = (' '..='~').collect();
    for start in ["code-review", "github"] {
        for &lead in &['c', '1', '-'] {
            for &c in &printable {
                let (mut app, _) = browsing();
                select(&mut app, start);
                let before = menu(&app).catalog.clone();
                for typed in [lead, c, ' ', '1', c, ' ', '3', c] {
                    let action = app.on_key(key(KeyCode::Char(typed)));
                    assert_eq!(
                        action,
                        Action::None,
                        "{start}: {lead}{c} then {typed:?} acted"
                    );
                }
                assert_eq!(menu(&app).page, PluginsPage::Browse, "{lead}{c}");
                assert!(menu(&app).job.is_none());
                assert_eq!(menu(&app).catalog, before, "{lead}{c}");
            }
        }
    }
}

#[test]
fn a_digit_typed_on_a_plugin_page_only_moves_the_highlight() {
    // A leading Space opens an available plugin's page; what is typed after
    // it on that page runs nothing without Enter.
    let (mut app, round) = browsing();
    select(&mut app, "github");
    app.on_key(key(KeyCode::Char(' ')));
    app.apply_plugin_event(
        round,
        PluginEvent::Detail {
            id: id("github", "claude-plugins-official"),
            result: Ok(Box::new(detail("github", PluginStatus::Available))),
        },
    );
    for c in (' '..='~').collect::<Vec<_>>() {
        assert_eq!(app.on_key(key(KeyCode::Char(c))), Action::None, "{c:?}");
        assert!(menu(&app).job.is_none(), "{c:?}");
        if menu(&app).page != PluginsPage::Plugin {
            break;
        }
    }
}

#[test]
fn up_and_down_wrap_and_home_end_jump() {
    let (mut app, _) = browsing();
    app.on_key(key(KeyCode::Up));
    assert_eq!(highlighted_name(&app), "vercel", "↑ from the top wraps");
    app.on_key(key(KeyCode::Down));
    assert_eq!(highlighted_name(&app), "code-review");
    app.on_key(key(KeyCode::End));
    assert_eq!(highlighted_name(&app), "vercel");
    app.on_key(key(KeyCode::Home));
    assert_eq!(highlighted_name(&app), "code-review");
}

#[test]
fn each_tab_remembers_its_own_highlight_while_the_page_is_open() {
    let (mut app, _) = browsing();
    select(&mut app, "github");
    app.on_key(key(KeyCode::Right));
    select(&mut app, "linear");
    app.on_key(key(KeyCode::Left));
    assert_eq!(highlighted_name(&app), "github");
    app.on_key(key(KeyCode::Right));
    assert_eq!(highlighted_name(&app), "linear");
}

#[test]
fn a_new_catalog_keeps_the_tab_the_query_and_the_highlighted_plugin_by_id() {
    let (mut app, round) = browsing();
    type_str(&mut app, "i");
    select(&mut app, "github");
    let mut fresh = catalog();
    // A plugin that sorts ahead of it arrives, and every index moves.
    fresh
        .plugins
        .insert(0, plugin("aardvark-ai", 1, PluginStatus::Available));
    app.apply_plugin_event(round, PluginEvent::Catalog(fresh));
    assert_eq!(menu(&app).query, "i");
    assert_eq!(menu(&app).tab, PluginsTab::All);
    assert_eq!(highlighted_name(&app), "github");
}

#[test]
fn a_marketplace_tab_whose_marketplace_vanished_falls_back_to_all() {
    let (mut app, round) = browsing();
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Right));
    assert_eq!(
        menu(&app).tab,
        PluginsTab::Marketplace("openai-curated".to_string())
    );
    let mut fresh = catalog();
    fresh.marketplaces.truncate(1);
    fresh.plugins.retain(|p| p.marketplace == 0);
    app.apply_plugin_event(round, PluginEvent::Catalog(fresh));
    assert_eq!(menu(&app).tab, PluginsTab::All);
}

#[test]
fn space_flips_an_installed_row_at_once_and_a_failure_flips_it_back() {
    let (mut app, round) = browsing();
    let action = app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(
        action,
        Action::PluginOp(PluginOp::SetEnabled {
            id: id("code-review", "claude-plugins-official"),
            enabled: false,
        })
    );
    let status = |app: &App| menu(app).catalog.as_ref().unwrap().plugins[0].status;
    assert_eq!(status(&app), PluginStatus::Disabled, "optimistic");
    assert!(menu(&app).job.is_none(), "a toggle is not a job");
    app.apply_plugin_event(
        round,
        PluginEvent::EnabledSet {
            id: id("code-review", "claude-plugins-official"),
            enabled: false,
            result: Err("plugins.json is not writable".to_string()),
        },
    );
    assert_eq!(status(&app), PluginStatus::Enabled, "rolled back");
    let notice = menu(&app).notice.clone().expect("a notice");
    assert!(!notice.ok);
    assert_eq!(
        notice.text,
        "Couldn't disable code-review — plugins.json is not writable"
    );
}

#[test]
fn a_toggle_that_lands_states_its_result() {
    let (mut app, round) = browsing();
    select(&mut app, "feature-dev");
    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Action::PluginOp(PluginOp::SetEnabled {
            id: id("feature-dev", "claude-plugins-official"),
            enabled: true,
        })
    );
    app.apply_plugin_event(
        round,
        PluginEvent::EnabledSet {
            id: id("feature-dev", "claude-plugins-official"),
            enabled: true,
            result: Ok(JobDone {
                text: "Enabled feature-dev — it loads from your next message".to_string(),
                detail: None,
                goto: None,
            }),
        },
    );
    let notice = menu(&app).notice.clone().expect("a notice");
    assert!(notice.ok);
    assert_eq!(
        menu(&app).catalog.as_ref().unwrap().plugins[1].status,
        PluginStatus::Enabled
    );
}

#[test]
fn space_on_a_plugin_not_installed_opens_it_like_enter() {
    let (mut app, _) = browsing();
    select(&mut app, "github");
    let action = app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(
        action,
        Action::PluginOp(PluginOp::LoadDetail {
            id: id("github", "claude-plugins-official")
        })
    );
    assert_eq!(menu(&app).page, PluginsPage::Plugin);
}

#[test]
fn enter_on_a_plugin_opens_its_page_and_asks_for_its_detail() {
    let (mut app, round) = browsing();
    select(&mut app, "github");
    let action = app.on_key(key(KeyCode::Enter));
    let github = id("github", "claude-plugins-official");
    assert_eq!(
        action,
        Action::PluginOp(PluginOp::LoadDetail { id: github.clone() })
    );
    let page = menu(&app);
    assert_eq!(page.page, PluginsPage::Plugin);
    assert_eq!(page.subject, Some(github.clone()));
    assert_eq!(page.detail, None, "loading");
    assert_eq!(page.plugin_actions(), vec![PluginAction::Back]);
    // A detail for another plugin is not this page's.
    app.apply_plugin_event(
        round,
        PluginEvent::Detail {
            id: id("notion", "openai-curated"),
            result: Ok(Box::new(detail("notion", PluginStatus::Available))),
        },
    );
    assert_eq!(menu(&app).detail, None);
    app.apply_plugin_event(
        round,
        PluginEvent::Detail {
            id: github,
            result: Ok(Box::new(detail("github", PluginStatus::Available))),
        },
    );
    assert_eq!(
        menu(&app).plugin_actions(),
        vec![PluginAction::Install, PluginAction::Back]
    );
}

#[test]
fn the_plugin_page_offers_what_the_status_allows() {
    let actions = |status, update: bool, blocked: Option<&str>| {
        let (mut app, round) = on_plugin_page("code-review", PluginStatus::Enabled);
        let mut d = detail("code-review", status);
        d.update_available = update;
        d.version = Some("1.1.0".to_string());
        d.blocked = blocked.map(str::to_string);
        app.apply_plugin_event(
            round,
            PluginEvent::Detail {
                id: id("code-review", "claude-plugins-official"),
                result: Ok(Box::new(d)),
            },
        );
        menu(&app).plugin_actions()
    };
    use PluginAction::*;
    assert_eq!(
        actions(PluginStatus::Available, false, None),
        [Install, Back]
    );
    assert_eq!(
        actions(
            PluginStatus::Available,
            false,
            Some("npm sources are not supported")
        ),
        [Back]
    );
    assert_eq!(
        actions(PluginStatus::Enabled, false, None),
        [Disable, CheckForUpdates, Uninstall, Back]
    );
    assert_eq!(
        actions(PluginStatus::Disabled, true, None),
        [Enable, UpdateTo("1.1.0".to_string()), Uninstall, Back]
    );
}

#[test]
fn install_starts_a_job_in_the_same_key_press() {
    let (mut app, _) = on_plugin_page("github", PluginStatus::Available);
    app.set_plugins_clock(Duration::from_secs(7));
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Action::PluginOp(PluginOp::Install {
            id: id("github", "claude-plugins-official")
        })
    );
    let job = menu(&app).job.clone().expect("the job runs");
    assert_eq!(job.kind, JobKind::Install);
    assert_eq!(job.target, "github");
    assert_eq!(job.since, Duration::from_secs(7));
    assert!(app.wants_animation_frames(), "a running job animates");
}

#[test]
fn every_key_but_esc_and_ctrl_c_is_swallowed_while_a_job_runs() {
    let (mut app, round) = on_plugin_page("github", PluginStatus::Available);
    app.on_key(key(KeyCode::Enter));
    for code in [
        KeyCode::Down,
        KeyCode::Enter,
        KeyCode::Char('2'),
        KeyCode::Left,
    ] {
        assert_eq!(app.on_key(key(code)), Action::None);
    }
    assert_eq!(menu(&app).page, PluginsPage::Plugin);
    assert!(menu(&app).job.is_some());
    app.apply_plugin_event(
        round,
        PluginEvent::JobProgress {
            step: Some("cloning github.com/owner/repo".to_string()),
            percent: Some(62),
        },
    );
    let job = menu(&app).job.clone().unwrap();
    assert_eq!(job.step.as_deref(), Some("cloning github.com/owner/repo"));
    assert_eq!(job.percent, Some(62));
}

#[test]
fn esc_goes_back_exactly_one_level_in_every_state() {
    // A running job: Esc cancels it, and the page stays.
    let (mut app, _) = on_plugin_page("github", PluginStatus::Available);
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.on_key(key(KeyCode::Esc)),
        Action::PluginOp(PluginOp::Cancel)
    );
    assert!(menu(&app).job.is_none());
    assert_eq!(menu(&app).page, PluginsPage::Plugin);
    // An armed confirmation: Esc disarms it.
    let (mut app, _) = on_plugin_page("code-review", PluginStatus::Enabled);
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Enter));
    assert!(menu(&app).armed());
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(!menu(&app).armed());
    assert_eq!(menu(&app).page, PluginsPage::Plugin);
    // A page: back to Browse, on the row it was opened from.
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(menu(&app).page, PluginsPage::Browse);
    assert_eq!(highlighted_name(&app), "code-review");
    // A typed query: cleared.
    type_str(&mut app, "lin");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(menu(&app).query, "");
    // Browse: closed.
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::ClosePluginsMenu);
    assert!(app.plugins_menu.is_none());
}

#[test]
fn backspace_on_a_detail_page_goes_back_like_esc() {
    let (mut app, _) = on_plugin_page("code-review", PluginStatus::Enabled);
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(menu(&app).page, PluginsPage::Browse);
}

#[test]
fn uninstall_asks_once_in_place_and_any_other_key_disarms_it() {
    let (mut app, _) = on_plugin_page("code-review", PluginStatus::Enabled);
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(menu(&app).armed());
    app.on_key(key(KeyCode::Up));
    assert!(!menu(&app).armed(), "moving disarms");
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::PluginOp(PluginOp::Uninstall {
            id: id("code-review", "claude-plugins-official")
        })
    );
    assert_eq!(menu(&app).job.as_ref().unwrap().kind, JobKind::Uninstall);
}

#[test]
fn a_digit_moves_to_its_action_and_enter_runs_it() {
    let (mut app, _) = on_plugin_page("code-review", PluginStatus::Enabled);
    assert_eq!(app.on_key(key(KeyCode::Char('2'))), Action::None);
    assert_eq!(menu(&app).action(), 1);
    assert!(menu(&app).job.is_none(), "a digit runs nothing");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::PluginOp(PluginOp::Update {
            id: id("code-review", "claude-plugins-official")
        })
    );
    let (mut app, _) = on_plugin_page("code-review", PluginStatus::Enabled);
    assert_eq!(app.on_key(key(KeyCode::Char('9'))), Action::None);
    assert_eq!(
        menu(&app).action(),
        0,
        "a digit past the list moves nothing"
    );
    app.on_key(key(KeyCode::Char('4')));
    assert_eq!(
        menu(&app).page,
        PluginsPage::Plugin,
        "4 only highlights Back"
    );
    app.on_key(key(KeyCode::Enter));
    assert_eq!(menu(&app).page, PluginsPage::Browse, "Enter on 4. Back");
}

#[test]
fn disable_on_the_plugin_page_flips_the_status_and_the_row() {
    let (mut app, _) = on_plugin_page("code-review", PluginStatus::Enabled);
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::PluginOp(PluginOp::SetEnabled {
            id: id("code-review", "claude-plugins-official"),
            enabled: false,
        })
    );
    let page = menu(&app);
    let Some(Ok(d)) = &page.detail else {
        panic!("a detail")
    };
    assert_eq!(d.status, PluginStatus::Disabled);
    assert_eq!(page.plugin_actions()[0], PluginAction::Enable);
    assert_eq!(
        page.catalog.as_ref().unwrap().plugins[0].status,
        PluginStatus::Disabled
    );
}

#[test]
fn a_finished_job_states_its_result_on_the_page_it_ran_on() {
    let (mut app, round) = on_plugin_page("github", PluginStatus::Available);
    app.on_key(key(KeyCode::Enter));
    app.apply_plugin_event(
        round,
        PluginEvent::JobFinished {
            result: Ok(JobDone {
                text: "Installed github — 1 skill is ready for your next message.".to_string(),
                detail: Some("1 MCP server waits for your review at the next launch.".to_string()),
                goto: None,
            }),
        },
    );
    let page = menu(&app);
    assert!(page.job.is_none());
    assert_eq!(page.page, PluginsPage::Plugin);
    let notice = page.notice.clone().expect("the result");
    assert!(notice.ok);
    assert!(notice.text.starts_with("Installed github"));
    assert!(notice.detail.is_some());
    // The next key that moves clears it.
    app.on_key(key(KeyCode::Down));
    assert_eq!(menu(&app).notice, None);
}

#[test]
fn a_failed_job_says_why_in_the_page_s_own_words() {
    let (mut app, round) = on_plugin_page("github", PluginStatus::Available);
    app.on_key(key(KeyCode::Enter));
    app.apply_plugin_event(
        round,
        PluginEvent::JobFinished {
            result: Err("remote: Repository not found.".to_string()),
        },
    );
    let notice = menu(&app).notice.clone().expect("the failure");
    assert!(!notice.ok);
    assert_eq!(
        notice.text,
        "Couldn't install github — remote: Repository not found."
    );
    assert_eq!(menu(&app).page, PluginsPage::Plugin, "the page stays");
}

#[test]
fn an_uninstall_that_lands_returns_to_browse_with_its_notice() {
    let (mut app, round) = on_plugin_page("code-review", PluginStatus::Enabled);
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));
    app.apply_plugin_event(
        round,
        PluginEvent::JobFinished {
            result: Ok(JobDone {
                text: "Uninstalled code-review".to_string(),
                detail: None,
                goto: Some(JobGoto::Browse),
            }),
        },
    );
    assert_eq!(menu(&app).page, PluginsPage::Browse);
    assert_eq!(
        menu(&app).notice.as_ref().unwrap().text,
        "Uninstalled code-review"
    );
}

#[test]
fn a_job_finished_after_it_was_cancelled_is_dropped() {
    let (mut app, round) = on_plugin_page("github", PluginStatus::Available);
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Esc));
    app.apply_plugin_event(
        round,
        PluginEvent::JobFinished {
            result: Err("cancelled".to_string()),
        },
    );
    let notice = menu(&app).notice.clone().expect("the cancel is said");
    assert!(notice.cancelled);
    assert_eq!(notice.text, "Cancelled — install github");
}

#[test]
fn esc_on_a_running_job_says_cancelled_and_a_late_finish_says_so() {
    let (mut app, round) = on_plugin_page("github", PluginStatus::Available);
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Esc));
    assert!(menu(&app).notice.as_ref().unwrap().cancelled);
    app.apply_plugin_event(round, PluginEvent::CancelledJobEnded { changed: false });
    assert_eq!(menu(&app).notice.as_ref().unwrap().detail, None);
    app.apply_plugin_event(round, PluginEvent::CancelledJobEnded { changed: true });
    assert_eq!(
        menu(&app).notice.as_ref().unwrap().detail.as_deref(),
        Some("It had already finished — the page shows it as it is now.")
    );
}

#[test]
fn a_plugin_from_a_removed_marketplace_offers_no_update() {
    let (mut app, round) = browsing();
    let mut fresh = catalog();
    fresh.marketplaces[0].fetched = false;
    fresh.marketplaces[0].error = Some("removed".to_string());
    app.apply_plugin_event(round, PluginEvent::Catalog(fresh));
    select(&mut app, "code-review");
    app.on_key(key(KeyCode::Enter));
    app.apply_plugin_event(
        round,
        PluginEvent::Detail {
            id: id("code-review", "claude-plugins-official"),
            result: Ok(Box::new(detail("code-review", PluginStatus::Enabled))),
        },
    );
    assert_eq!(
        menu(&app).plugin_actions(),
        vec![
            PluginAction::Disable,
            PluginAction::Uninstall,
            PluginAction::Back
        ]
    );
}

#[test]
fn the_review_count_is_the_subject_s_alone() {
    let (mut app, _) = on_plugin_page("code-review", PluginStatus::Enabled);
    app.set_plugin_review(&id("code-review", "claude-plugins-official"), 2, 1, false);
    assert_eq!(menu(&app).review_waiting(), Some((2, 1, false)));
    app.set_plugin_review(&id("github", "claude-plugins-official"), 4, 0, false);
    assert_eq!(menu(&app).review_waiting(), None);
}

#[test]
fn ctrl_c_closes_from_any_page_even_while_a_job_runs() {
    let (mut app, _) = on_plugin_page("github", PluginStatus::Available);
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.on_key(ctrl('c')), Action::ClosePluginsMenu);
    assert!(app.plugins_menu.is_none());
}

#[test]
fn ctrl_r_fetches_every_marketplace_again_unless_one_is_out() {
    let (mut app, round) = browsing();
    assert_eq!(
        app.on_key(ctrl('r')),
        Action::PluginOp(PluginOp::Refresh { force: true })
    );
    app.apply_plugin_event(
        round,
        PluginEvent::FetchStarted {
            marketplace: "openai-curated".to_string(),
        },
    );
    assert_eq!(app.on_key(ctrl('r')), Action::None);
}

#[test]
fn fetches_run_report_progress_and_finish_or_fail() {
    let (mut app, round) = browsing();
    app.set_plugins_clock(Duration::from_secs(3));
    let name = "openai-curated".to_string();
    app.apply_plugin_event(
        round,
        PluginEvent::FetchStarted {
            marketplace: name.clone(),
        },
    );
    assert!(app.wants_animation_frames(), "a fetch animates");
    app.apply_plugin_event(
        round,
        PluginEvent::FetchProgress {
            marketplace: name.clone(),
            phase: Some("Receiving objects".to_string()),
            percent: Some(45),
        },
    );
    assert_eq!(
        menu(&app).fetch_for(&name).unwrap().state,
        FetchState::Running {
            phase: Some("Receiving objects".to_string()),
            percent: Some(45),
            since: Duration::from_secs(3),
        }
    );
    app.apply_plugin_event(
        round,
        PluginEvent::FetchFinished {
            marketplace: name.clone(),
            result: Err("Could not resolve host: github.com".to_string()),
        },
    );
    assert_eq!(
        menu(&app).fetch_for(&name).unwrap().state,
        FetchState::Failed("Could not resolve host: github.com".to_string())
    );
    assert!(
        !app.wants_animation_frames(),
        "a still page asks for no frames"
    );
    app.apply_plugin_event(
        round,
        PluginEvent::FetchStarted {
            marketplace: name.clone(),
        },
    );
    app.apply_plugin_event(
        round,
        PluginEvent::FetchFinished {
            marketplace: name.clone(),
            result: Ok(()),
        },
    );
    assert_eq!(menu(&app).fetch_for(&name), None);
}

#[test]
fn a_still_page_asks_for_no_animation_frames() {
    let (app, _) = browsing();
    assert!(!app.wants_animation_frames());
}

#[test]
fn a_catalog_that_cannot_be_read_says_why_until_one_can() {
    let mut app = App::new();
    let round = app.open_plugins_menu();
    app.apply_plugin_event(
        round,
        PluginEvent::CatalogFailed("plugins.json: expected value at line 1".to_string()),
    );
    assert_eq!(
        menu(&app).load_error(),
        Some("plugins.json: expected value at line 1")
    );
    app.apply_plugin_event(round, PluginEvent::Catalog(catalog()));
    assert_eq!(menu(&app).load_error(), None);
}

#[test]
fn the_marketplace_page_offers_fetch_update_or_read_again() {
    let (mut app, round) = browsing();
    let mut fresh = catalog();
    fresh.marketplaces[1].fetched = false;
    fresh.marketplaces.push(CatalogMarketplace {
        skipped: Vec::new(),
        is_local: true,
        is_default: false,
        ..marketplace("team", "~/work/team-plugins")
    });
    app.apply_plugin_event(round, PluginEvent::Catalog(fresh));
    app.on_key(key(KeyCode::Left));
    use MarketplaceAction::*;
    let open = |app: &mut App, name: &str| {
        app.on_key(key(KeyCode::Home));
        select(app, name);
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert_eq!(menu(app).page, PluginsPage::Marketplace);
        menu(app).marketplace_actions()
    };
    assert_eq!(
        open(&mut app, "claude-plugins-official"),
        [BrowsePlugins, UpdateNow, Remove, Back]
    );
    app.on_key(key(KeyCode::Esc));
    assert_eq!(open(&mut app, "openai-curated"), [FetchNow, Remove, Back]);
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::PluginOp(PluginOp::FetchMarketplace {
            marketplace: "openai-curated".to_string()
        })
    );
    assert!(
        matches!(
            menu(&app).fetch_for("openai-curated").unwrap().state,
            FetchState::Running { .. }
        ),
        "the fetch shows at once"
    );
    app.on_key(key(KeyCode::Esc));
    assert_eq!(
        open(&mut app, "team"),
        [BrowsePlugins, ReadAgain, Remove, Back]
    );
}

#[test]
fn browse_its_plugins_lands_on_the_marketplace_s_tab() {
    let (mut app, _) = browsing();
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(menu(&app).page, PluginsPage::Browse);
    assert_eq!(
        menu(&app).tab,
        PluginsTab::Marketplace("claude-plugins-official".to_string())
    );
}

#[test]
fn removing_a_marketplace_asks_once_then_runs_and_returns_to_its_tab() {
    let (mut app, round) = browsing();
    app.on_key(key(KeyCode::Left));
    select(&mut app, "openai-curated");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.on_key(key(KeyCode::Char('3'))), Action::None);
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(menu(&app).armed());
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::PluginOp(PluginOp::RemoveMarketplace {
            name: "openai-curated".to_string()
        })
    );
    app.apply_plugin_event(
        round,
        PluginEvent::JobFinished {
            result: Ok(JobDone {
                text: "Removed openai-curated".to_string(),
                detail: None,
                goto: Some(JobGoto::Browse),
            }),
        },
    );
    assert_eq!(menu(&app).page, PluginsPage::Browse);
    assert_eq!(menu(&app).tab, PluginsTab::Marketplaces);
}

#[test]
fn the_add_page_takes_a_source_and_lands_on_the_new_tab() {
    let (mut app, round) = browsing();
    app.on_key(key(KeyCode::Left));
    select(&mut app, "+add");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(menu(&app).page, PluginsPage::AddMarketplace);
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::None,
        "blank: ignored"
    );
    assert!(menu(&app).job.is_none());
    type_str(&mut app, "acme/agent-plugins");
    app.on_key(key(KeyCode::Char(' ')));
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(menu(&app).input, "acme/agent-plugins");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::PluginOp(PluginOp::AddMarketplace {
            source: "acme/agent-plugins".to_string()
        })
    );
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::None,
        "one at a time"
    );
    let mut fresh = catalog();
    fresh
        .marketplaces
        .push(marketplace("acme", "acme/agent-plugins"));
    app.apply_plugin_event(round, PluginEvent::Catalog(fresh));
    app.apply_plugin_event(
        round,
        PluginEvent::JobFinished {
            result: Ok(JobDone {
                text: "Added acme — 4 plugins. Pick one and press Enter.".to_string(),
                detail: None,
                goto: Some(JobGoto::MarketplaceTab("acme".to_string())),
            }),
        },
    );
    assert_eq!(menu(&app).page, PluginsPage::Browse);
    assert_eq!(menu(&app).tab, PluginsTab::Marketplace("acme".to_string()));
    assert!(menu(&app).notice.as_ref().unwrap().ok);
}

#[test]
fn a_failed_add_stays_on_the_add_page_with_the_field_intact() {
    let (mut app, round) = browsing();
    app.on_key(key(KeyCode::Left));
    select(&mut app, "+add");
    app.on_key(key(KeyCode::Enter));
    type_str(&mut app, "acme/nope");
    app.on_key(key(KeyCode::Enter));
    app.apply_plugin_event(
        round,
        PluginEvent::JobFinished {
            result: Err("remote: Repository not found.".to_string()),
        },
    );
    let page = menu(&app);
    assert_eq!(page.page, PluginsPage::AddMarketplace);
    assert_eq!(page.input, "acme/nope");
    assert_eq!(
        page.notice.as_ref().unwrap().text,
        "Couldn't add acme/nope — remote: Repository not found."
    );
    // Esc from the Add page: back to Browse on the Marketplaces tab.
    app.on_key(key(KeyCode::Esc));
    assert_eq!(menu(&app).page, PluginsPage::Browse);
    assert_eq!(menu(&app).tab, PluginsTab::Marketplaces);
}

#[test]
fn a_restore_row_restores_the_default_and_fetches_it() {
    let (mut app, round) = browsing();
    app.apply_plugin_event(
        round,
        PluginEvent::RemovedDefaults(vec!["openai/plugins".to_string()]),
    );
    app.on_key(key(KeyCode::Left));
    select(&mut app, "+restore openai/plugins");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::PluginOp(PluginOp::RestoreDefault {
            source: "openai/plugins".to_string()
        })
    );
    assert_eq!(
        menu(&app).job.as_ref().unwrap().kind,
        JobKind::RestoreMarketplace
    );
}

#[test]
fn a_paste_lands_in_the_field_the_page_has_and_never_in_the_composer() {
    let (mut app, _) = browsing();
    app.input.insert_str("draft");
    app.paste_into_plugins("  linear\n");
    assert_eq!(menu(&app).query, "linear", "Browse: into the search");
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Left));
    select(&mut app, "+add");
    app.on_key(key(KeyCode::Enter));
    app.paste_into_plugins("  https://gitlab.com/acme/plugins.git\r\n#main \n");
    assert_eq!(
        menu(&app).input,
        "https://gitlab.com/acme/plugins.git#main",
        "Add: newlines and surrounding whitespace stripped"
    );
    app.on_key(key(KeyCode::Esc));
    select(&mut app, "claude-plugins-official");
    app.on_key(key(KeyCode::Enter));
    app.paste_into_plugins("anything");
    assert_eq!(menu(&app).page, PluginsPage::Marketplace);
    assert_eq!(app.input.text(), "draft", "the composer draft is untouched");
}

#[test]
fn the_background_hint_is_hidden_while_the_page_is_open() {
    let (mut app, _) = browsing();
    app.set_command_elapsed(Some(Duration::from_secs(30)));
    assert_eq!(app.background_hint_elapsed(), None);
    app.close_plugins_menu();
    assert_eq!(app.background_hint_elapsed(), Some(Duration::from_secs(30)));
}

#[test]
fn a_fresh_app_holds_nothing_for_the_page() {
    let app = App::new();
    assert!(app.plugins_menu.is_none());
    assert!(!app.wants_animation_frames());
}

#[test]
fn a_removed_marketplace_and_the_folder_one_offer_only_their_plugins() {
    let (mut app, round) = browsing();
    let mut fresh = catalog();
    fresh.marketplaces.push(CatalogMarketplace {
        skipped: Vec::new(),
        fetched: false,
        updated: None,
        error: Some("removed — its installed plugins stay".to_string()),
        is_default: false,
        ..marketplace("old-market", "acme/old")
    });
    fresh.marketplaces.push(CatalogMarketplace {
        skipped: Vec::new(),
        is_local: true,
        is_default: false,
        updated: None,
        ..marketplace("local", "folders on this machine")
    });
    app.apply_plugin_event(round, PluginEvent::Catalog(fresh));
    app.on_key(key(KeyCode::Left));
    for name in ["old-market", "local"] {
        app.on_key(key(KeyCode::Home));
        select(&mut app, name);
        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            menu(&app).marketplace_actions(),
            [MarketplaceAction::BrowsePlugins, MarketplaceAction::Back],
            "{name}"
        );
        app.on_key(key(KeyCode::Esc));
    }
}
