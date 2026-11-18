//! The inline `/plugins` page's rendering (`docs/plugins.md` *The
//! `/plugins` page*).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::app::{PluginDone, PluginsTab};
use crate::plugins::{
    CatalogMarketplace, CatalogPlugin, ComponentSummary, InstalledState, PluginCatalog,
    PluginFormat, PluginId,
};
use crate::ui::theme::{
    PLUGINS_ADD_HINT, PLUGINS_LIST_HINT, PLUGINS_MARKETS_HINT, PLUGINS_TRUST_NOTE,
    ask_chip_current_bg, ask_warning_color, model_active_color, toast_error_color,
};

const TALL: u16 = 200;

fn plugin(key: &str, installed: Option<bool>) -> CatalogPlugin {
    CatalogPlugin {
        id: PluginId::parse(key).unwrap(),
        display_name: None,
        description: Some(format!("What {key} does")),
        version: Some("1.0.0".to_string()),
        author: None,
        category: None,
        homepage: None,
        keywords: Vec::new(),
        source: "./plugins/x".to_string(),
        unavailable: None,
        format: Some(PluginFormat::ClaudeCode),
        components: Some(ComponentSummary {
            skills: vec!["writing-rules".to_string()],
            commands: vec!["hookify".to_string(), "list".to_string()],
            hooks: vec!["PreToolUse".to_string(), "Stop (2)".to_string()],
            hook_count: 3,
            unused: vec!["LSP servers".to_string()],
            ..ComponentSummary::default()
        }),
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
        source: if git {
            format!("https://example.com/{name}.git")
        } else {
            format!("/srv/{name}")
        },
        git,
        description: Some(format!("The {name} catalog")),
        owner: Some("Team".to_string()),
        revision: Some("1a2b3c4d5e6f7a8b".to_string()),
        updated_at: Some(1_000),
        root: std::path::PathBuf::from("/m").join(name),
        plugins: 3,
        installed: 1,
        error: None,
        notes: Vec::new(),
    }
}

fn catalog() -> PluginCatalog {
    let mut deploy = plugin("deploy@team", None);
    deploy.description = Some("Ships builds to staging".to_string());
    let mut npm = plugin("npm-thing@team", None);
    npm.unavailable = Some("npm sources are not supported".to_string());
    PluginCatalog {
        marketplaces: vec![market("team", true), market("local", false)],
        plugins: vec![
            plugin("hookify@team", Some(true)),
            plugin("lint@local", Some(false)),
            deploy,
            npm,
        ],
        error: None,
    }
}

/// The page open over `catalog`, its first read answered.
fn open_with(catalog: PluginCatalog) -> App {
    let mut app = App::new();
    let round = app.open_plugins_menu(true);
    assert!(app.finish_plugin_job(round, Some(catalog), &Ok(PluginDone::default())));
    app
}

fn plugins_app() -> App {
    open_with(catalog())
}

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

/// The page on a terminal `height` rows tall, as trimmed plain rows.
fn page_at(app: &App, width: u16, height: u16) -> Vec<String> {
    crate::ui::plugins_view::plugins_view_lines(app, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

fn page(app: &App) -> Vec<String> {
    page_at(app, 80, TALL)
}

fn has(texts: &[String], needle: &str) -> bool {
    texts.iter().any(|t| t.contains(needle))
}

#[test]
fn the_list_shows_the_tabs_the_rows_and_the_highlighted_plugins_facts() {
    let texts = page(&plugins_app());
    assert!(has(&texts, "Plugins"), "{texts:#?}");
    assert!(
        has(&texts, "2 installed · 4 plugins in 2 marketplaces"),
        "{texts:#?}"
    );
    assert!(
        has(&texts, " Installed (2)    Discover (4)    Marketplaces (2)"),
        "{texts:#?}"
    );
    assert!(texts.iter().any(|t| t == "  ❯"), "{texts:#?}");
    let hookify = texts
        .iter()
        .find(|t| t.contains("→ hookify"))
        .expect("the highlighted row");
    assert!(hookify.contains("✔ enabled"), "{hookify}");
    assert!(hookify.contains("What hookify@team does"), "{hookify}");
    let lint = texts.iter().find(|t| t.contains("lint")).expect("lint");
    assert!(lint.contains("◯ disabled"), "{lint}");
    assert!(has(&texts, "(1/2)"), "{texts:#?}");
    assert!(
        has(
            &texts,
            "hookify@team · v1.0.0 · 1 skill · 2 commands · 3 hooks"
        ),
        "the facts under the list: {texts:#?}"
    );
    assert!(has(&texts, PLUGINS_LIST_HINT), "{texts:#?}");
    assert_frame_hugs_content(
        "the list",
        &crate::ui::plugins_view::plugins_view_lines(&plugins_app(), 80, TALL),
    );
}

#[test]
fn the_rows_line_up_their_status_column() {
    let texts = page(&plugins_app());
    let at = |needle: &str| {
        let row = texts.iter().find(|t| t.contains(needle)).expect(needle);
        cols_before(row, needle)
    };
    assert_eq!(at("✔ enabled"), at("◯ disabled"));
}

/// The display columns of `row` before `needle`.
fn cols_before(row: &str, needle: &str) -> usize {
    crate::ui::wrap::cols(&row[..row.find(needle).expect(needle)])
}

#[test]
fn the_current_tab_is_a_lit_chip() {
    let app = plugins_app();
    let lines = crate::ui::plugins_view::plugins_view_lines(&app, 80, TALL);
    let strip = lines
        .iter()
        .find(|line| plain(line).contains("Discover"))
        .expect("the strip");
    let lit: Vec<&str> = strip
        .spans
        .iter()
        .filter(|span| span.style.bg == Some(ask_chip_current_bg()))
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(lit, [" Installed (2) "]);
}

#[test]
fn discover_says_what_can_and_cannot_be_installed() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Right);
    let texts = page(&app);
    let deploy = texts.iter().find(|t| t.contains("deploy")).expect("deploy");
    assert!(deploy.contains("+ available"), "{deploy}");
    assert!(deploy.contains("Ships builds to staging"), "{deploy}");
    let npm = texts.iter().find(|t| t.contains("npm-thing")).expect("npm");
    assert!(npm.contains("– unavailable"), "{npm}");
    assert!(has(&texts, "(1/4)"), "{texts:#?}");
}

#[test]
fn an_update_opens_the_description_on_an_amber_mark() {
    let mut catalog = catalog();
    catalog.plugins[0].installed.as_mut().unwrap().update = true;
    let app = open_with(catalog);
    let lines = crate::ui::plugins_view::plugins_view_lines(&app, 80, TALL);
    let row = lines
        .iter()
        .find(|line| plain(line).contains("→ hookify"))
        .expect("the row");
    let text = plain(row);
    assert!(text.contains("✔ enabled"), "{text}");
    assert!(text.contains("▲ update · What hookify@team does"), "{text}");
    let mark = row
        .spans
        .iter()
        .find(|span| span.content.contains('▲'))
        .expect("the mark");
    assert_eq!(mark.style.fg, Some(ask_warning_color()));
}

#[test]
fn the_check_is_green_and_only_the_check() {
    let lines = crate::ui::plugins_view::plugins_view_lines(&plugins_app(), 80, TALL);
    let row = lines
        .iter()
        .find(|line| plain(line).contains("→ hookify"))
        .expect("the row");
    let check = row
        .spans
        .iter()
        .find(|span| span.content == "✔")
        .expect("the check");
    assert_eq!(check.style.fg, Some(model_active_color()));
    let word = row
        .spans
        .iter()
        .find(|span| span.content == "enabled")
        .expect("the word");
    assert_ne!(word.style.fg, Some(model_active_color()));
}

#[test]
fn two_plugins_of_one_name_name_their_marketplaces() {
    let mut catalog = catalog();
    catalog.plugins.push(plugin("lint@team", None));
    let mut app = open_with(catalog);
    press(&mut app, KeyCode::Right);
    let texts = page(&app);
    assert!(has(&texts, "lint@local"), "{texts:#?}");
    assert!(has(&texts, "lint@team"), "{texts:#?}");
    // A name only one row has stays short.
    assert!(texts.iter().any(|t| t.contains("  deploy  ")), "{texts:#?}");
}

#[test]
fn the_marketplaces_tab_lists_counts_sources_and_the_add_row() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Left);
    assert_eq!(
        app.plugins_menu.as_ref().unwrap().tab,
        PluginsTab::Marketplaces
    );
    let mut texts = page(&app);
    let team = texts.iter().find(|t| t.contains("→ team")).expect("team");
    assert!(
        team.contains("3 plugins · 1 installed · https://example.com/team.git"),
        "{team}"
    );
    assert!(has(&texts, "+ Add a marketplace"), "{texts:#?}");
    assert!(has(&texts, PLUGINS_MARKETS_HINT), "{texts:#?}");
    // Under the list: its description, then its source and revision — and
    // its age, once the clock is injected.
    assert!(has(&texts, "The team catalog"), "{texts:#?}");
    assert!(
        has(&texts, "https://example.com/team.git · 1a2b3c4"),
        "{texts:#?}"
    );
    assert!(!has(&texts, "updated"), "no clock yet: {texts:#?}");
    app.set_plugins_clock(0, 1_000 + 7_200);
    texts = page(&app);
    assert!(has(&texts, "· updated 2h ago"), "{texts:#?}");
}

#[test]
fn a_marketplace_that_cannot_be_read_says_why_on_its_row() {
    let mut catalog = catalog();
    catalog.marketplaces[1].error = Some("no marketplace.json".to_string());
    let mut app = open_with(catalog);
    press(&mut app, KeyCode::Left);
    let texts = page(&app);
    let local = texts.iter().find(|t| t.contains("local")).expect("local");
    assert!(local.contains("✘ no marketplace.json"), "{local}");
}

#[test]
fn the_first_read_shows_its_clock_where_the_rows_will_be() {
    let mut app = App::new();
    app.open_plugins_menu(true);
    app.set_plugins_clock(3, 0);
    let texts = page(&app);
    let reading: Vec<&String> = texts
        .iter()
        .filter(|t| t.contains("Reading the marketplaces…"))
        .collect();
    assert_eq!(reading.len(), 1, "once: {texts:#?}");
    assert!(reading[0].contains("(3s · esc to cancel)"), "{texts:#?}");
    assert!(has(&texts, " Installed  "), "no counts yet: {texts:#?}");
}

#[test]
fn an_empty_tab_names_where_to_go() {
    let app = open_with(PluginCatalog::default());
    // No catalog entry at all: the Marketplaces tab, whose add row is the
    // way in.
    assert!(has(&page(&app), "+ Add a marketplace"));
    let mut app = open_with(PluginCatalog {
        marketplaces: vec![market("team", true)],
        plugins: vec![plugin("deploy@team", None)],
        error: None,
    });
    press(&mut app, KeyCode::Left);
    let texts = page(&app);
    assert!(has(&texts, "No plugins installed yet"), "{texts:#?}");
}

#[test]
fn a_running_job_and_a_failure_show_over_the_hint() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Char(' '));
    app.set_plugins_clock(2, 0);
    let texts = page(&app);
    let job = texts
        .iter()
        .position(|t| t.contains("Disabling hookify… (2s · esc to cancel)"))
        .expect("the job line");
    let hint = texts
        .iter()
        .position(|t| t.contains(PLUGINS_LIST_HINT))
        .expect("the hint");
    assert_eq!(hint, job + 2, "a gap between them: {texts:#?}");

    let round = app.plugins_job_round().unwrap();
    app.finish_plugin_job(
        round,
        Some(catalog()),
        &Err("could not save plugins.json".to_string()),
    );
    let lines = crate::ui::plugins_view::plugins_view_lines(&app, 80, TALL);
    let error = lines
        .iter()
        .find(|line| plain(line).contains("could not save plugins.json"))
        .expect("the failure");
    assert!(
        error
            .spans
            .iter()
            .any(|span| span.style.fg == Some(toast_error_color()))
    );
}

#[test]
fn the_header_says_when_plugins_are_off_or_the_registry_is_broken() {
    let mut app = App::new();
    let round = app.open_plugins_menu(false);
    let broken = PluginCatalog {
        error: Some("plugins.json will not parse (EOF) — fix or remove it".to_string()),
        ..PluginCatalog::default()
    };
    app.finish_plugin_job(round, Some(broken), &Ok(PluginDone::default()));
    let texts = page(&app);
    assert!(
        has(&texts, "Plugins are off for this session"),
        "{texts:#?}"
    );
    assert!(has(&texts, "plugins.json will not parse"), "{texts:#?}");
}

#[test]
fn a_short_terminal_sheds_rows_down_to_the_search_line() {
    let app = plugins_app();
    assert_sheds_to_its_search_line(
        |height| page_at(&app, 80, height),
        "",
        "→ hookify",
        "Type to search",
    );
    for height in 1..=12 {
        assert_caret_on_search_line(&app, &page_at(&app, 80, height), 80, "");
    }
    // The tabs outlast the title: a page with room for the compact list and
    // the strip, but not the whole header, keeps the strip.
    let compact: Vec<usize> = (1..=30)
        .filter(|h| {
            let texts = page_at(&app, 80, *h);
            has(&texts, "Discover") && !has(&texts, "4 plugins in")
        })
        .map(usize::from)
        .collect();
    assert!(!compact.is_empty(), "a rung keeps only the tab strip");

    let mut app = plugins_app();
    for c in "zzz".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    assert_sheds_to_its_search_line(
        |height| page_at(&app, 80, height),
        "zzz",
        "No matching plugins",
        "Type to search",
    );
}

#[test]
fn the_caret_follows_the_search() {
    let mut app = plugins_app();
    for c in "hoo".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let texts = page(&app);
    assert_caret_on_search_line(&app, &texts, 80, "hoo");
    assert!(cursor_visible(&app));
}

fn open_plugin_page(app: &mut App) {
    press(app, KeyCode::Enter);
    assert!(matches!(
        app.plugins_menu.as_ref().unwrap().page,
        crate::app::PluginsPage::Plugin(_)
    ));
}

#[test]
fn a_plugins_page_shows_its_facts_over_its_actions() {
    let mut app = plugins_app();
    open_plugin_page(&mut app);
    let texts = page(&app);
    assert!(has(&texts, "hookify"), "{texts:#?}");
    assert!(has(&texts, "hookify@team · v1.0.0"), "{texts:#?}");
    assert!(has(&texts, "What hookify@team does"), "{texts:#?}");
    let status = texts
        .iter()
        .find(|t| t.contains("Status:"))
        .expect("status");
    assert!(status.contains("✔ enabled in this directory"), "{status}");
    assert!(has(&texts, "Claude Code plugin"), "{texts:#?}");
    let skills = texts
        .iter()
        .find(|t| t.contains("Skills:"))
        .expect("skills");
    assert!(skills.contains("writing-rules"), "{skills}");
    assert!(has(&texts, "hookify, list"), "{texts:#?}");
    assert!(has(&texts, "PreToolUse, Stop (2)"), "{texts:#?}");
    assert!(has(&texts, "LSP servers"), "{texts:#?}");
    assert!(has(&texts, "❯ 1. Disable in this directory"), "{texts:#?}");
    assert!(has(&texts, "2. Update"), "{texts:#?}");
    assert!(has(&texts, "3. Uninstall"), "{texts:#?}");
    assert!(has(&texts, "4. Back"), "{texts:#?}");
    assert!(!cursor_visible(&app), "a page of actions hides the cursor");
    assert_frame_hugs_content(
        "a plugin's page",
        &crate::ui::plugins_view::plugins_view_lines(&app, 80, TALL),
    );
}

#[test]
fn a_plugin_that_brings_hooks_says_they_wait_for_review() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    open_plugin_page(&mut app);
    let texts = page(&app);
    assert!(has(&texts, "+ not installed"), "{texts:#?}");
    assert!(has(&texts, "❯ 1. Install"), "{texts:#?}");
    let note = PLUGINS_TRUST_NOTE.split(',').next().unwrap();
    assert!(has(&texts, note), "{texts:#?}");
}

#[test]
fn a_plugin_that_cannot_be_installed_says_why() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::End);
    open_plugin_page(&mut app);
    let texts = page(&app);
    assert!(has(&texts, "– cannot be installed"), "{texts:#?}");
    assert!(has(&texts, "Npm sources are not supported"), "{texts:#?}");
    assert!(has(&texts, "❯ 1. Back"), "{texts:#?}");
}

#[test]
fn the_hidden_cursor_seats_on_the_highlighted_action() {
    let mut app = plugins_app();
    open_plugin_page(&mut app);
    press(&mut app, KeyCode::Down);
    let width = 80;
    let height = crate::ui::plugins_menu_height(&app, width, TALL).expect("open");
    let area = Rect::new(0, 0, width, height);
    let texts = page(&app);
    let row = texts
        .iter()
        .position(|t| t.contains("❯ 2. Update"))
        .expect("the action");
    assert_eq!(
        cursor_position(area, &app),
        (2, u16::try_from(row).unwrap())
    );
}

#[test]
fn a_marketplaces_page_shows_where_it_comes_from() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Enter);
    app.set_plugins_clock(0, 1_000 + 120);
    let texts = page(&app);
    assert!(
        has(&texts, "3 plugins · 1 installed · by Team"),
        "{texts:#?}"
    );
    assert!(has(&texts, "The team catalog"), "{texts:#?}");
    let source = texts
        .iter()
        .find(|t| t.contains("Source:"))
        .expect("source");
    assert!(source.contains("https://example.com/team.git"), "{source}");
    let revision = texts.iter().find(|t| t.contains("Revision:")).expect("rev");
    assert!(revision.contains("1a2b3c4d5e6f"), "{revision}");
    let updated = texts.iter().find(|t| t.contains("Updated:")).expect("age");
    assert!(updated.contains("2m ago"), "{updated}");
    assert!(has(&texts, "Location:"), "{texts:#?}");
    assert!(has(&texts, "❯ 1. Browse its plugins"), "{texts:#?}");
    assert!(has(&texts, "2. Update"), "{texts:#?}");
    assert!(has(&texts, "3. Remove"), "{texts:#?}");
    assert_frame_hugs_content(
        "a marketplace's page",
        &crate::ui::plugins_view::plugins_view_lines(&app, 80, TALL),
    );
}

#[test]
fn removing_a_marketplace_names_what_it_uninstalls() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('3'));
    let texts = page(&app);
    assert!(has(&texts, "Remove the team marketplace?"), "{texts:#?}");
    assert!(
        has(
            &texts,
            "It uninstalls the plugins installed from it: hookify."
        ),
        "{texts:#?}"
    );
    assert!(has(&texts, "  1. Remove"), "{texts:#?}");
    assert!(has(&texts, "❯ 2. Cancel"), "the safe answer: {texts:#?}");
    assert_frame_hugs_content(
        "the question",
        &crate::ui::plugins_view::plugins_view_lines(&app, 80, TALL),
    );
}

#[test]
fn the_add_page_seats_the_caret_in_its_field() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::End);
    press(&mut app, KeyCode::Enter);
    let texts = page(&app);
    assert!(has(&texts, "Add a marketplace"), "{texts:#?}");
    assert!(
        texts.iter().any(|t| t == "  ❯ owner/repo"),
        "the placeholder: {texts:#?}"
    );
    assert!(has(&texts, PLUGINS_ADD_HINT), "{texts:#?}");
    assert!(cursor_visible(&app));
    for c in "acme/tools".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let texts = page(&app);
    let row = texts
        .iter()
        .position(|t| t == "  ❯ acme/tools")
        .expect("the field");
    let width = 80;
    let height = crate::ui::plugins_menu_height(&app, width, TALL).expect("open");
    let area = Rect::new(0, 0, width, height);
    assert_eq!(
        cursor_position(area, &app),
        (4 + 10, u16::try_from(row).unwrap())
    );
    assert_frame_hugs_content(
        "the add page",
        &crate::ui::plugins_view::plugins_view_lines(&app, 80, TALL),
    );
}

#[test]
fn the_live_region_shows_the_page_in_place_of_the_composer() {
    let app = plugins_app();
    let width = 80;
    let height = crate::ui::plugins_menu_height(&app, width, 40).expect("open");
    let mut buf = buffer(width, height);
    render_live(buf.area, &mut buf, &app);
    let text: Vec<String> = (0..height).map(|y| row(&buf, y, width)).collect();
    assert!(text.iter().any(|t| t.contains("→ hookify")), "{text:#?}");
    assert!(text.iter().any(|t| t.contains("Plugins")), "{text:#?}");
}
