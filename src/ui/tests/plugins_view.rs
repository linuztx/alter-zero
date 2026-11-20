//! The inline `/plugins` page's rendering (`docs/plugins.md` *The
//! `/plugins` page*).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::app::{PLUGINS_JOB_DELAY, PluginDone, PluginsTab};
use crate::plugins::{
    CatalogMarketplace, CatalogPlugin, ComponentSummary, InstalledState, PluginCatalog,
    PluginFormat, PluginId,
};
use crate::ui::theme::{
    PLUGINS_TRUST_NOTE, ask_chip_current_bg, ask_warning_color, model_active_color,
    toast_error_color,
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
            in_place: false,
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
        has(&texts, "2 installed · 1 on in this directory"),
        "{texts:#?}"
    );
    assert!(
        has(
            &texts,
            " All (4)    Installed (2)    team (3)    local (1)    + Add"
        ),
        "{texts:#?}"
    );
    assert!(texts.iter().any(|t| t == "  ❯"), "{texts:#?}");
    let hookify = texts
        .iter()
        .find(|t| t.contains("→ hookify"))
        .expect("the highlighted row");
    assert!(hookify.contains("✔ enabled"), "{hookify}");
    // Where every marketplace is listed, a row names its own, in a column.
    assert!(
        hookify.contains(" team    What hookify@team does"),
        "{hookify}"
    );
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
    assert!(has(&texts, "←/→ tabs"), "{texts:#?}");
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

/// A catalog shaped like the official ones: two marketplaces with long
/// names, a few long plugin names among short ones, and descriptions of
/// every length.
fn official_like() -> PluginCatalog {
    let described = |key: &str, text: &str| {
        let mut plugin = plugin(key, None);
        plugin.description = Some(text.to_string());
        plugin
    };
    PluginCatalog {
        marketplaces: vec![
            market("claude-plugins-official", true),
            market("openai-curated", true),
        ],
        plugins: vec![
            described(
                "42crunch-api-security-testing@claude-plugins-official",
                "Automate API security directly in Claude Code with 42Crunch - automatically \
                 audit OpenAPI specs, detect vulnerabilities aligned with OWASP API Security \
                 risks (including BOLA/BFLA), and apply AI-powered fixes.",
            ),
            described(
                "activecampaign@claude-plugins-official",
                "Marketing automation, CRM and email campaigns from ActiveCampaign.",
            ),
            described(
                "context7@claude-plugins-official",
                "Upstash Context7 MCP server for up-to-date documentation lookup.",
            ),
            described(
                "github@openai-curated",
                "Inspect repositories, triage pull requests and issues, debug CI.",
            ),
            described("sentry@openai-curated", "Errors."),
        ],
        error: None,
    }
}

/// The row of `texts` that names `needle`.
fn row_of<'a>(texts: &'a [String], needle: &str) -> &'a str {
    texts
        .iter()
        .find(|t| t.contains(needle))
        .unwrap_or_else(|| panic!("a row naming {needle}: {texts:#?}"))
}

#[test]
fn a_long_name_never_crowds_the_descriptions_off_the_rows() {
    // On the official catalogs a 29-column name sized the column for every
    // row: at 60 columns no row had room left for its description — the
    // marketplace was cut to `clau…` — and at 80 an All row read
    // `claude-plugins-official …`.
    let mut app = open_with(official_like());
    for tab in ["All", "claude-plugins-official"] {
        for width in [60_u16, 80, 120] {
            let texts = page_at(&app, width, TALL);
            let context = format!("the {tab} tab at {width} columns: {texts:#?}");
            assert!(
                row_of(&texts, "→ 42crunch").contains("Automate API"),
                "{context}"
            );
            assert!(
                row_of(&texts, "activecampaign").contains("Marketing automation"),
                "{context}"
            );
            assert!(
                row_of(&texts, "context7").contains("Upstash Context7"),
                "{context}"
            );
            // The highlighted row's name, cut in its column, is whole under
            // the list.
            assert!(
                has(
                    &texts,
                    "42crunch-api-security-testing@claude-plugins-official"
                ),
                "{context}"
            );
        }
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
    }
    let texts = page_at(&open_with(official_like()), 60, TALL);
    let (name, _) = row_of(&texts, "→ 42crunch")
        .split_once("+ available")
        .expect("the status");
    assert!(
        name.trim_end().ends_with('…'),
        "a name past its column gives way: {texts:#?}"
    );
}

#[test]
fn the_marketplace_is_a_column_of_its_own_where_the_rows_have_room() {
    let app = open_with(official_like());
    let wide = page_at(&app, 120, TALL);
    let at = |row: &str, needle: &str| cols_before(row_of(&wide, row), needle);
    assert_eq!(
        at("context7", "claude-plugins-official"),
        at("github", "openai-curated"),
        "{wide:#?}"
    );
    assert_eq!(
        at("context7", "Upstash"),
        at("github", "Inspect"),
        "{wide:#?}"
    );
    assert_eq!(
        at("context7", "Upstash"),
        at("sentry", "Errors."),
        "{wide:#?}"
    );
    // Narrower, the descriptions keep the room: the marketplace is the
    // line under the list's to name, for the highlighted row.
    for width in [60_u16, 80, 100] {
        let texts = page_at(&app, width, TALL);
        for row in texts.iter().filter(|t| t.contains("+ available")) {
            assert!(
                !row.contains("claude-plugins") && !row.contains("openai-curated"),
                "{width} columns: {row}"
            );
        }
    }
}

#[test]
fn moving_through_the_list_never_resizes_the_page() {
    // On the official catalogs the highlighted plugin's description, wrapped
    // whole under the list, set the page's height: ↑/↓ moved its bottom
    // rule from row 30 to 26 to 22 and back, every growth scrolling the
    // conversation above it.
    let mut catalog = official_like();
    catalog.plugins[4].description = None;
    let mut app = open_with(catalog);
    for width in [60_u16, 80, 120] {
        press(&mut app, KeyCode::Home);
        let mut heights = Vec::new();
        for _ in 0..5 {
            heights.push(page_at(&app, width, TALL).len());
            press(&mut app, KeyCode::Down);
        }
        assert!(
            heights.windows(2).all(|pair| pair[0] == pair[1]),
            "{width} columns: {heights:?}"
        );
    }
    // The highlighted row's description reads under the list, cut to its
    // row; one with none says so, rather than giving the row back.
    press(&mut app, KeyCode::Home);
    let texts = page(&app);
    assert!(
        has(&texts, "Automate API security directly in Claude Code"),
        "{texts:#?}"
    );
    press(&mut app, KeyCode::End);
    assert!(has(&page(&app), "No description"), "{:#?}", page(&app));

    // The add tab's typed source and its suggestions alike.
    press(&mut app, KeyCode::Left);
    assert_eq!(app.plugins_menu.as_ref().unwrap().tab, PluginsTab::Add);
    for c in "anthropics".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let typed = page(&app);
    press(&mut app, KeyCode::Down);
    let suggestion = page(&app);
    assert_eq!(typed.len(), suggestion.len(), "{typed:#?}\n{suggestion:#?}");
}

#[test]
fn a_list_from_one_marketplace_does_not_name_it_on_every_row() {
    // With one marketplace added, every row on *All* and *Installed* is
    // its own: a column repeating its name says nothing.
    let mut one = catalog();
    one.marketplaces.retain(|market| market.name == "team");
    one.plugins.retain(|plugin| plugin.id.marketplace == "team");
    let mut app = open_with(one);
    press(&mut app, KeyCode::Left);
    assert_eq!(app.plugins_menu.as_ref().unwrap().tab, PluginsTab::All);
    let texts = page(&app);
    let deploy = row_of(&texts, "deploy");
    assert!(!deploy.contains(" team "), "{deploy}");
    assert!(deploy.contains("Ships builds to staging"), "{deploy}");
    // Two marketplaces' plugins name their own.
    let mut app = plugins_app();
    press(&mut app, KeyCode::Left);
    let deploy = row_of(&page(&app), "deploy").to_string();
    assert!(deploy.contains(" team "), "{deploy}");
}

#[test]
fn a_row_never_ends_on_a_stub() {
    // `clau…` and `claude-plugins-official …` say nothing: a description
    // with less room than a word or two is left out whole.
    let app = open_with(official_like());
    for width in 30..=130_u16 {
        let texts = page_at(&app, width, TALL);
        for row in texts.iter().filter(|t| t.contains("+ available")) {
            let after = row
                .split_once("+ available")
                .map_or("", |(_, after)| after)
                .trim_start();
            if let Some(cut) = after.strip_suffix('…') {
                let last = cut.rsplit("  ").next().unwrap_or(cut);
                assert!(
                    crate::ui::wrap::cols(last) >= 7,
                    "{width} columns: a stub {last:?} in {row:?}"
                );
            }
        }
    }
}

#[test]
fn the_current_tab_is_a_lit_chip() {
    let app = plugins_app();
    let lines = crate::ui::plugins_view::plugins_view_lines(&app, 80, TALL);
    let strip = lines
        .iter()
        .find(|line| plain(line).contains("+ Add"))
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
fn a_marketplace_tab_lists_its_plugins_under_what_it_is() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Right);
    let mut texts = page(&app);
    assert!(has(&texts, "The team catalog"), "{texts:#?}");
    assert!(has(&texts, "3 plugins · 1 installed"), "{texts:#?}");
    assert!(
        has(&texts, "https://example.com/team.git · 1a2b3c4"),
        "{texts:#?}"
    );
    let deploy = texts.iter().find(|t| t.contains("deploy")).expect("deploy");
    assert!(deploy.contains("+ available"), "{deploy}");
    // Its own tab does not name it on every row.
    assert!(deploy.contains("  Ships builds to staging"), "{deploy}");
    assert!(!deploy.contains("team ·"), "{deploy}");
    let npm = texts.iter().find(|t| t.contains("npm-thing")).expect("npm");
    assert!(npm.contains("– unavailable"), "{npm}");
    assert!(has(&texts, "(1/3)"), "{texts:#?}");
    assert!(
        has(&texts, "Ctrl+U update"),
        "a git marketplace: {texts:#?}"
    );
    assert!(has(&texts, "Ctrl+R remove"), "{texts:#?}");
    assert!(!has(&texts, "updated"), "no clock yet: {texts:#?}");
    app.set_plugins_clock(std::time::Duration::ZERO, 1_000 + 7_200);
    texts = page(&app);
    assert!(
        has(&texts, "3 plugins · 1 installed · updated 2h ago"),
        "{texts:#?}"
    );
    // A directory has nothing to update.
    press(&mut app, KeyCode::Right);
    let texts = page(&app);
    assert!(!has(&texts, "Ctrl+U update"), "{texts:#?}");
    assert!(has(&texts, "Ctrl+R remove"), "{texts:#?}");
}

#[test]
fn a_marketplace_says_in_its_header_why_it_could_not_be_read_or_what_it_skipped() {
    let mut catalog = catalog();
    catalog.marketplaces[1].error = Some("no marketplace.json".to_string());
    catalog.marketplaces[0].notes = vec![
        "entry 4 skipped: `evil`: `ref` `--x` is not a branch or tag name".to_string(),
        "entry 5 skipped: `bad name`: invalid plugin name".to_string(),
    ];
    let mut app = open_with(catalog);
    press(&mut app, KeyCode::Right);
    let texts = page(&app);
    assert!(has(&texts, "2 entries skipped — `evil`"), "{texts:#?}");
    assert!(
        has(&texts, "is not a branch or tag name"),
        "the reason, whole: {texts:#?}"
    );
    press(&mut app, KeyCode::Right);
    let lines = crate::ui::plugins_view::plugins_view_lines(&app, 80, TALL);
    let error = lines
        .iter()
        .find(|line| plain(line).contains("✘ no marketplace.json"))
        .expect("the failure in the header");
    assert!(
        error
            .spans
            .iter()
            .any(|span| span.style.fg == Some(toast_error_color()))
    );
}

#[test]
fn toggling_a_plugin_never_moves_the_page() {
    // The flicker: a job line and a gap that came for one frame, then a toast
    // row over the page that pushed it down a row, then took it back up. A
    // slow job is said in the hint's own row now, and what went right not at
    // all — the row shows it.
    let mut app = plugins_app();
    let before = page(&app);
    let hint_row = before
        .iter()
        .position(|t| t.contains("←/→ tabs"))
        .expect("the hint");
    press(&mut app, KeyCode::Char(' '));
    app.set_plugins_clock(std::time::Duration::ZERO, 0);
    let running = page(&app);
    assert_eq!(running.len(), before.len(), "{running:#?}");
    assert!(
        !has(&running, "Disabling"),
        "a quick job says nothing: {running:#?}"
    );
    app.set_plugins_clock(PLUGINS_JOB_DELAY, 0);
    let slow = page(&app);
    assert_eq!(slow.len(), before.len(), "{slow:#?}");
    assert!(
        slow[hint_row].contains("Disabling hookify… (0s · esc to cancel)"),
        "{slow:#?}"
    );
    let round = app.plugins_job_round().unwrap();
    app.finish_plugin_job(
        round,
        Some(catalog()),
        &Ok(PluginDone {
            message: "Disabled hookify in this directory".to_string(),
            marketplace: None,
        }),
    );
    let done = page(&app);
    assert_eq!(done.len(), before.len(), "{done:#?}");
    assert!(done[hint_row].contains("←/→ tabs"), "{done:#?}");
    assert!(!has(&done, "Disabled hookify"), "{done:#?}");
    // On a plugin's own page, the same.
    let mut app = plugins_app();
    press(&mut app, KeyCode::Enter);
    let before = page(&app);
    press(&mut app, KeyCode::Enter);
    app.set_plugins_clock(PLUGINS_JOB_DELAY, 0);
    assert_eq!(page(&app).len(), before.len());
    let round = app.plugins_job_round().unwrap();
    app.finish_plugin_job(
        round,
        Some(catalog()),
        &Ok(PluginDone {
            message: "Disabled hookify in this directory".to_string(),
            marketplace: None,
        }),
    );
    let done = page(&app);
    assert_eq!(done.len(), before.len());
    assert!(!has(&done, "Disabled hookify"), "{done:#?}");
    assert!(has(&done, "Esc to go back"), "{done:#?}");
}

#[test]
fn a_failure_is_said_in_red_on_the_page() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Char(' '));
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
    assert_frame_hugs_content("a failure", &lines);
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
    assert!(
        text.contains(" team    ▲ update · What hookify@team does"),
        "{text}"
    );
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
    press(&mut app, KeyCode::Left);
    let texts = page(&app);
    assert!(has(&texts, "lint@local"), "{texts:#?}");
    assert!(has(&texts, "lint@team"), "{texts:#?}");
    // A name only one row has stays short.
    assert!(texts.iter().any(|t| t.contains("  deploy  ")), "{texts:#?}");
}

#[test]
fn the_first_read_waits_a_moment_then_shows_its_clock_where_the_rows_will_be() {
    let mut app = App::new();
    app.open_plugins_menu(true);
    app.set_plugins_clock(std::time::Duration::ZERO, 0);
    assert!(!has(&page(&app), "Reading"), "{:#?}", page(&app));
    app.set_plugins_clock(std::time::Duration::from_secs(3), 0);
    let texts = page(&app);
    let reading: Vec<&String> = texts
        .iter()
        .filter(|t| t.contains("Reading the marketplaces…"))
        .collect();
    assert_eq!(reading.len(), 1, "once: {texts:#?}");
    assert!(reading[0].contains("(3s · esc to cancel)"), "{texts:#?}");
    assert!(
        has(&texts, " All    Installed    + Add"),
        "no counts yet: {texts:#?}"
    );
}

#[test]
fn an_empty_tab_names_where_to_go() {
    let mut app = open_with(PluginCatalog {
        marketplaces: vec![market("team", true)],
        plugins: vec![plugin("deploy@team", None)],
        error: None,
    });
    press(&mut app, KeyCode::Right);
    assert_eq!(
        app.plugins_menu.as_ref().unwrap().tab,
        PluginsTab::Installed
    );
    let texts = page(&app);
    assert!(has(&texts, "No plugins installed yet"), "{texts:#?}");
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
        "←/→ tabs",
    );
    for height in 1..=12 {
        assert_caret_on_search_line(&app, &page_at(&app, 80, height), 80, "");
    }
    // The tabs outlast the title: a page with room for the compact list and
    // the strip, but not the whole header, keeps the strip.
    let compact: Vec<usize> = (1..=30)
        .filter(|h| {
            let texts = page_at(&app, 80, *h);
            has(&texts, "+ Add") && !has(&texts, "1 on in this directory")
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
        "←/→ tabs",
    );
}

#[test]
fn the_hint_keeps_its_clauses_whole_on_a_narrow_terminal() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Right);
    for width in [40_u16, 52, 64, 80, 120] {
        let texts = page_at(&app, width, TALL);
        let hint = texts
            .iter()
            .rev()
            .nth(1)
            .expect("the hint, over the closing rule");
        assert!(!hint.ends_with('…'), "{width} columns: {hint:?}");
        assert!(hint.contains("Esc to close"), "{width} columns: {hint:?}");
        assert!(
            crate::ui::wrap::cols(hint) <= usize::from(width),
            "{width}: {hint:?}"
        );
    }
}

#[test]
fn a_detail_pages_hint_keeps_the_way_back_on_a_narrow_terminal() {
    // At 40 columns a plugin's page cut its hint to `… · Enter to select ·
    // E…`: the one key that leaves the page was the one it lost.
    let mut app = plugins_app();
    open_plugin_page(&mut app);
    let mut confirm = plugins_app();
    open_plugin_page(&mut confirm);
    press(&mut confirm, KeyCode::Char('3'));
    assert!(matches!(
        confirm.plugins_menu.as_ref().unwrap().page,
        crate::app::PluginsPage::Confirm(_)
    ));
    for (what, app) in [("a plugin's page", &app), ("the confirmation", &confirm)] {
        for width in [24_u16, 30, 40, 52, 80] {
            let texts = page_at(app, width, TALL);
            let hint = texts
                .iter()
                .rev()
                .nth(1)
                .expect("the hint, over the closing rule");
            assert!(!hint.ends_with('…'), "{what} at {width}: {hint:?}");
            assert!(
                hint.contains("Esc to go back"),
                "{what} at {width}: {hint:?}"
            );
            assert!(
                crate::ui::wrap::cols(hint) <= usize::from(width),
                "{what} at {width}: {hint:?}"
            );
        }
    }
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

#[test]
fn the_tab_strip_scrolls_to_keep_the_current_tab_in_view() {
    let mut catalog = catalog();
    for i in 0..8 {
        catalog
            .marketplaces
            .push(market(&format!("market-{i}"), true));
    }
    let mut app = open_with(catalog);
    // The strip is two rows over the search line, past its gap.
    let strip = |app: &App| {
        let texts = page_at(app, 60, TALL);
        let search = texts
            .iter()
            .position(|t| t.starts_with("  ❯"))
            .expect("the search line");
        texts[search - 2].clone()
    };
    let first = strip(&app);
    assert!(first.contains(" Installed (2) "), "{first}");
    assert!(first.ends_with('›'), "more to the right: {first}");
    assert!(crate::ui::wrap::cols(&first) <= 60, "{first}");
    for _ in 0..9 {
        press(&mut app, KeyCode::Right);
    }
    let last = strip(&app);
    assert!(last.contains(" market-7 (0) "), "{last}");
    assert!(
        last.trim_start().starts_with('‹'),
        "more to the left: {last}"
    );
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
    assert_eq!(texts[1], "  hookify", "{texts:#?}");
    // Its state leads the line under its name: on a long description a
    // `Status:` row sat five rows further down.
    assert_eq!(
        texts[2], "  ✔ enabled in this directory · hookify@team · v1.0.0",
        "{texts:#?}"
    );
    assert!(!has(&texts, "Status:"), "{texts:#?}");
    assert!(has(&texts, "What hookify@team does"), "{texts:#?}");
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
    assert!(has(&texts, "4. Back to plugins"), "{texts:#?}");
    assert!(!cursor_visible(&app), "a page of actions hides the cursor");
    assert_frame_hugs_content(
        "a plugin's page",
        &crate::ui::plugins_view::plugins_view_lines(&app, 80, TALL),
    );
}

#[test]
fn a_plugin_loaded_in_place_says_where_from_and_offers_no_update() {
    let mut catalog = catalog();
    let state = catalog.plugins[0].installed.as_mut().unwrap();
    state.in_place = true;
    catalog.plugins[0].source = "/srv/team/plugins/hookify".to_string();
    let mut app = open_with(catalog);
    open_plugin_page(&mut app);
    let texts = page(&app);
    let loads = texts
        .iter()
        .find(|t| t.contains("Loads from:"))
        .expect("where it loads from");
    assert!(loads.contains("/srv/team/plugins/hookify"), "{loads}");
    assert!(!has(&texts, "Update"), "{texts:#?}");
    assert!(has(&texts, "2. Uninstall"), "{texts:#?}");
}

#[test]
fn a_plugin_that_was_not_loaded_says_why() {
    let mut catalog = catalog();
    catalog.plugins[0].installed.as_mut().unwrap().error = Some(
        "not loaded — hookify@other is on under the same name, `hookify`; turn one off".to_string(),
    );
    let mut app = open_with(catalog);
    let texts = page(&app);
    let row = texts.iter().find(|t| t.contains("→ hookify")).expect("row");
    assert!(row.contains("✘ not loaded"), "{row}");
    open_plugin_page(&mut app);
    let texts = page(&app);
    assert!(
        texts[2].starts_with("  ✘ installed, but not loaded · "),
        "{texts:#?}"
    );
    assert!(
        has(&texts, "hookify@other is on under the same name"),
        "{texts:#?}"
    );
}

#[test]
fn a_plugin_that_brings_hooks_says_they_wait_for_review() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Down);
    open_plugin_page(&mut app);
    let texts = page(&app);
    assert!(has(&texts, "+ not installed"), "{texts:#?}");
    assert!(has(&texts, "❯ 1. Install"), "{texts:#?}");
    assert!(
        has(&texts, "Plugins can run code on your computer"),
        "{texts:#?}"
    );
    let note: String = texts.join(" ");
    let note: String = note.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(note.contains(PLUGINS_TRUST_NOTE), "{texts:#?}");
}

#[test]
fn a_plugin_whose_contents_are_not_read_yet_warns_all_the_same() {
    // A plugin in a repository of its own is read once it is installed: its
    // page said `read once it is installed` and nothing about what it may
    // run — the one case the user knows least about.
    let mut catalog = catalog();
    catalog.plugins[2].components = None;
    let mut app = open_with(catalog);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Down);
    open_plugin_page(&mut app);
    let texts = page(&app);
    assert!(has(&texts, "read once it is installed"), "{texts:#?}");
    assert!(
        has(&texts, "Plugins can run code on your computer"),
        "{texts:#?}"
    );
    // One that brings only instructions runs nothing of its own.
    let mut catalog = catalog_with_skills_only_deploy();
    catalog.plugins[2].installed = None;
    let mut app = open_with(catalog);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Down);
    open_plugin_page(&mut app);
    let texts = page(&app);
    assert!(
        !has(&texts, "Plugins can run code on your computer"),
        "{texts:#?}"
    );
}

/// [`catalog`] with `deploy` bringing one skill and nothing that runs.
fn catalog_with_skills_only_deploy() -> PluginCatalog {
    let mut catalog = catalog();
    catalog.plugins[2].components = Some(ComponentSummary {
        skills: vec!["deploy:ship".to_string()],
        ..ComponentSummary::default()
    });
    catalog
}

#[test]
fn a_plugins_line_under_its_name_breaks_between_its_clauses() {
    // At 60 columns `… · by` ended one row and `42Crunch` opened the next.
    let mut catalog = official_like();
    catalog.plugins[0].author = Some("42Crunch".to_string());
    let mut app = open_with(catalog);
    open_plugin_page(&mut app);
    for width in [40_u16, 60, 80, 120] {
        let texts = page_at(&app, width, TALL);
        assert!(has(&texts, "by 42Crunch"), "{width}: {texts:#?}");
        for row in &texts {
            assert!(
                !row.ends_with(" by") && !row.ends_with('·') && !row.starts_with("  · "),
                "{width} columns: {row:?} in {texts:#?}"
            );
        }
    }
}

#[test]
fn a_plugins_facts_line_up_on_a_column_sized_to_their_labels() {
    // The `/mcp` page's label column, sized for its `Config location:`,
    // left every value of a plugin's page four columns further right than
    // its own labels needed — a wrapped homepage one row longer.
    let mut app = plugins_app();
    open_plugin_page(&mut app);
    let texts = page(&app);
    // The widest label here is `Not used here:`.
    let column = 2 + crate::ui::wrap::cols("Not used here:") + 2;
    assert_eq!(
        cols_before(row_of(&texts, "Format:"), "Claude Code plugin"),
        column,
        "{texts:#?}"
    );
    assert_eq!(
        cols_before(row_of(&texts, "Source:"), "./plugins/x"),
        column,
        "{texts:#?}"
    );
}

#[test]
fn a_source_with_a_repository_links_its_address() {
    let mut catalog = official_like();
    catalog.plugins[0].source =
        "https://github.com/42Crunch-AI/claude-plugins.git · plugins/api-security-testing @ v1.5.5"
            .to_string();
    let mut app = open_with(catalog);
    open_plugin_page(&mut app);
    let lines = crate::ui::plugins_view::plugins_view_lines(&app, 60, TALL);
    let targets: Vec<String> = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .filter_map(|span| crate::links::style_link(&span.style))
        .map(|target| target.to_string())
        .collect();
    assert!(
        targets
            .iter()
            .any(|target| target == "https://github.com/42Crunch-AI/claude-plugins.git"),
        "the repository is a link, every wrapped piece of it: {targets:#?}"
    );
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
    assert!(has(&texts, "❯ 1. Back to plugins"), "{texts:#?}");
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
fn removing_a_marketplace_names_what_it_uninstalls() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Right);
    app.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    let texts = page(&app);
    assert!(has(&texts, "Remove the team marketplace?"), "{texts:#?}");
    assert!(
        has(
            &texts,
            "It uninstalls the plugins installed from it: hookify."
        ),
        "{texts:#?}"
    );
    // The answer the user just chose is the one Enter gives.
    assert!(has(&texts, "❯ 1. Remove"), "{texts:#?}");
    assert!(has(&texts, "  2. Cancel"), "{texts:#?}");
    assert_frame_hugs_content(
        "the question",
        &crate::ui::plugins_view::plugins_view_lines(&app, 80, TALL),
    );
}

#[test]
fn the_add_tab_offers_the_official_marketplaces_under_its_field() {
    let mut catalog = catalog();
    let mut official = market("openai-curated", true);
    official.source = "https://github.com/openai/plugins.git".to_string();
    catalog.marketplaces.push(official);
    let mut app = open_with(catalog);
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Left);
    assert_eq!(app.plugins_menu.as_ref().unwrap().tab, PluginsTab::Add);
    let texts = page(&app);
    assert!(
        has(
            &texts,
            "Add a marketplace: owner/repo, a git URL, or a directory"
        ),
        "what the field takes: {texts:#?}"
    );
    let claude = texts
        .iter()
        .find(|t| t.contains("anthropics/claude-plugins-official"))
        .expect("Claude Code's catalog");
    assert!(claude.contains("→ "), "highlighted first: {claude}");
    assert!(claude.contains("Claude Code"), "{claude}");
    let codex = texts
        .iter()
        .find(|t| t.contains("openai/plugins"))
        .expect("codex's catalog");
    assert!(codex.contains("✔ added"), "it is added already: {codex}");
    // Under them, the row that adds any other source, saying what it takes.
    let other = texts
        .iter()
        .find(|t| t.contains("Add a marketplace…"))
        .expect("the row for any other source");
    assert!(other.ends_with("owner/repo, git URL or path"), "{other}");
    assert_eq!(
        cols_before(claude, "anthropics"),
        cols_before(other, "Add a marketplace…"),
        "{texts:#?}"
    );
    assert!(has(&texts, "Enter to add"), "{texts:#?}");
    assert!(cursor_visible(&app));
    assert_caret_on_search_line(&app, &texts, 80, "");
    assert_frame_hugs_content(
        "the add tab",
        &crate::ui::plugins_view::plugins_view_lines(&app, 80, TALL),
    );
    for c in "acme/tools".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let texts = page(&app);
    assert_caret_on_search_line(&app, &texts, 80, "acme/tools");
    let typed = texts
        .iter()
        .find(|t| t.contains("→ acme/tools"))
        .expect("the typed source, highlighted");
    assert!(typed.contains("Press Enter to add it"), "{typed}");
    // The typed source is that row now.
    assert!(!has(&texts, "Add a marketplace…"), "{texts:#?}");
}

#[test]
fn the_other_row_opens_a_field_for_any_source() {
    let mut app = plugins_app();
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Left);
    assert_eq!(app.plugins_menu.as_ref().unwrap().tab, PluginsTab::Add);
    press(&mut app, KeyCode::End);
    press(&mut app, KeyCode::Enter);
    let texts = page(&app);
    assert_eq!(texts[1], "  Add a marketplace", "{texts:#?}");
    assert!(
        has(
            &texts,
            "A git repository or a directory holding a plugin marketplace"
        ),
        "{texts:#?}"
    );
    // An empty field shows what goes in it, the caret at its start.
    let field = texts
        .iter()
        .position(|t| t.starts_with("  ❯"))
        .expect("the field");
    assert_eq!(texts[field], "  ❯ owner/repo", "{texts:#?}");
    assert!(
        has(
            &texts,
            "Use owner/repo, a git URL, or a path to a directory."
        ),
        "{texts:#?}"
    );
    assert!(has(&texts, "Enter to add · Esc to go back"), "{texts:#?}");
    assert!(cursor_visible(&app), "a field shows its caret");
    let width = 80;
    let height = crate::ui::plugins_menu_height(&app, width, TALL).expect("open");
    let area = Rect::new(0, 0, width, height);
    assert_eq!(
        cursor_position(area, &app),
        (4, u16::try_from(field).unwrap())
    );
    assert_frame_hugs_content(
        "the add page",
        &crate::ui::plugins_view::plugins_view_lines(&app, 80, TALL),
    );
    // Typed into, the caret follows the text.
    for c in "acme/tools".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let texts = page(&app);
    assert_eq!(texts[field], "  ❯ acme/tools", "{texts:#?}");
    assert_eq!(
        cursor_position(area, &app),
        (14, u16::try_from(field).unwrap())
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
