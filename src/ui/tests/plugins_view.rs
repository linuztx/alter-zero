//! The `/plugins` page's rendering (`docs/plugins.md`).

use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::app::{JobDone, PluginEvent, PluginsPage};
use crate::plugins::catalog::{
    CatalogMarketplace, CatalogPlugin, Components, PluginCatalog, PluginDetail, PluginStatus,
};
use crate::plugins::id::PluginId;
use crate::plugins::manifest::ManifestFormat;
use crate::ui::plugins_view::plugins_view_lines;
use crate::ui::theme::{footer_focus_bg, model_active_color, tool_fail_color};
use crate::ui::wrap::cols;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(super) fn press(app: &mut App, code: KeyCode) {
    app.on_key(key(code));
}

pub(super) fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

/// When the injected wall clock reads.
const NOW: i64 = 1_763_650_000;

fn market(name: &str, source: &str, plugins_updated_ago: i64) -> CatalogMarketplace {
    CatalogMarketplace {
        skipped: Vec::new(),
        name: name.to_string(),
        title: name.to_string(),
        source: source.to_string(),
        web_url: Some(format!("https://github.com/{source}")),
        is_default: true,
        is_local: false,
        fetched: true,
        updated: Some(u64::try_from(NOW - plugins_updated_ago).unwrap()),
        revision: Some("d7d32797e7e02708d0bb00ffe8b8f2397dc1aea6".to_string()),
        error: None,
    }
}

fn plug(name: &str, marketplace: usize, status: PluginStatus, description: &str) -> CatalogPlugin {
    CatalogPlugin {
        installed_version: None,
        name: name.to_string(),
        display_name: None,
        marketplace,
        description: description.to_string(),
        category: Some("development".to_string()),
        author: Some("Anthropic".to_string()),
        version: Some("1.0.0".to_string()),
        status,
        update_available: false,
    }
}

/// A 1,200-character description, the longest a real marketplace ships.
fn long_description() -> String {
    let sentence = "Builds, tests and ships Xcode projects from the agent, reading build logs \
                    and simulator output so failures are explained in plain words. ";
    let mut text = String::new();
    while text.len() < 1_200 {
        text.push_str(sentence);
    }
    text.truncate(1_200);
    text
}

/// A catalog shaped like the real one: two marketplaces, a dozen plugins in
/// the first and a handful in the second, names and descriptions of every
/// length — one with a display name, one in CJK, one with an emoji — some
/// installed, disabled, updatable or impossible to install.
fn catalog() -> PluginCatalog {
    use PluginStatus::*;
    let mut plugins = vec![
        plug(
            "code-review",
            0,
            Enabled,
            "Automated code review for pull requests using multiple specialized agents with confidence-based scoring to filter false positives.",
        ),
        plug(
            "feature-dev",
            0,
            Disabled,
            "Guided feature development with codebase understanding and architecture focus",
        ),
        plug(
            "agent-sdk-dev",
            0,
            Available,
            "Claude Agent SDK Development Plugin",
        ),
        plug("github", 0, Available, "GitHub's official MCP server"),
        plug("xcode", 0, Available, &long_description()),
        plug(
            "commit-commands",
            0,
            Available,
            "Streamline your git workflow with simple commands for committing, pushing, and creating pull requests",
        ),
        plug(
            "security-guidance",
            0,
            Available,
            "Security reminder hook that warns about potential security issues when editing files",
        ),
        plug(
            "pr-review-toolkit",
            0,
            Available,
            "Comprehensive PR review agents specializing in comments, tests, error handling, type design, code quality, and code simplification",
        ),
        plug(
            "frontend-design",
            0,
            Available,
            "Create distinctive, production-grade frontend interfaces",
        ),
        plug(
            "ralph-wiggum-iterative-development-loop",
            0,
            Available,
            "An iterative loop",
        ),
        plug(
            "翻訳ツール",
            0,
            Available,
            "日本語の文書を英語に翻訳するプラグイン",
        ),
        plug(
            "emoji-tools",
            0,
            Unsupported,
            "Insert 🎉 and ✨ into commit messages — an npm source",
        ),
        plug("linear", 1, Enabled, "Find and manage Linear issues"),
        plug("notion", 1, Available, "Search and edit Notion pages"),
        plug("vercel", 1, Available, "Deploy and manage Vercel projects"),
        plug("figma", 1, Available, "Read Figma files"),
        plug("sentry", 1, Available, "Investigate Sentry issues"),
    ];
    plugins[12].update_available = true;
    plugins[3].display_name = Some("GitHub".to_string());
    plugins[13].category = None;
    plugins[13].author = None;
    PluginCatalog {
        marketplaces: vec![
            market(
                "claude-plugins-official",
                "anthropics/claude-plugins-official",
                7_200,
            ),
            market("openai-curated", "openai/plugins", 3_600),
        ],
        plugins,
    }
}

fn id(name: &str, marketplace: &str) -> PluginId {
    PluginId::new(name, marketplace).unwrap()
}

/// A detail with every field.
pub(super) fn full_detail() -> PluginDetail {
    PluginDetail {
        installed_version: None,
        id: id("code-review", "claude-plugins-official"),
        display_name: None,
        description: "Automated code review for pull requests using multiple specialized agents with confidence-based scoring to filter false positives.".to_string(),
        version: Some("1.0.0".to_string()),
        author: Some("Anthropic".to_string()),
        category: Some("development".to_string()),
        homepage: Some("https://github.com/anthropics/claude-plugins-official/tree/main/plugins/code-review".to_string()),
        license: Some("Apache-2.0".to_string()),
        source: "plugins/code-review".to_string(),
        source_url: Some("https://github.com/anthropics/claude-plugins-official/tree/main/plugins/code-review".to_string()),
        format: ManifestFormat::Claude,
        status: PluginStatus::Enabled,
        update_available: false,
        blocked: None,
        components: Some(Components {
            skills: vec!["code-review:lint".to_string(), "code-review:triage".to_string()],
            commands: vec!["/code-review:code-review".to_string()],
            agents: vec![
                "code-review:reviewer".to_string(),
                "code-review:tester".to_string(),
                "code-review:security".to_string(),
                "code-review:style".to_string(),
                "code-review:docs".to_string(),
            ],
            hooks: vec!["PreToolUse · Bash".to_string()],
            mcp_servers: vec!["github (http)".to_string()],
            unsupported: vec!["1 app".to_string(), "2 LSP servers".to_string()],
        }),
        warnings: vec!["hooks/extra.json: an `if` condition was dropped".to_string()],
    }
}

/// A plugin whose files live in another repository: nothing known of its
/// components until it is installed.
fn remote_detail() -> PluginDetail {
    PluginDetail {
        installed_version: None,
        id: id("github", "claude-plugins-official"),
        display_name: Some("GitHub".to_string()),
        status: PluginStatus::Available,
        components: None,
        source: "github.com/github/github-mcp-server · plugins/x @ v1.2 (9fceb02)".to_string(),
        homepage: None,
        license: None,
        warnings: Vec::new(),
        ..full_detail()
    }
}

/// One that can't be installed here.
fn blocked_detail() -> PluginDetail {
    PluginDetail {
        installed_version: None,
        id: id("emoji-tools", "claude-plugins-official"),
        status: PluginStatus::Unsupported,
        blocked: Some("npm sources are not supported".to_string()),
        components: None,
        ..full_detail()
    }
}

/// The page open with the catalog landed, the clocks set.
pub(super) fn browsing() -> (App, u64) {
    let mut app = App::new();
    let round = app.open_plugins_menu();
    app.set_plugins_now(NOW);
    app.set_plugins_clock(Duration::from_secs(5));
    app.apply_plugin_event(round, PluginEvent::Catalog(catalog()));
    app.apply_plugin_event(
        round,
        PluginEvent::RemovedDefaults(vec!["acme/plugins".to_string()]),
    );
    app.set_plugins_home(Some("~/.alter-zero".to_string()));
    (app, round)
}

/// The Plugin page of the highlighted row with `detail` landed.
pub(super) fn on_plugin(detail: Option<Result<PluginDetail, String>>) -> App {
    let (mut app, round) = browsing();
    press(&mut app, KeyCode::Enter);
    if let Some(result) = detail {
        let subject = app.plugins_menu.as_ref().unwrap().subject.clone().unwrap();
        app.apply_plugin_event(
            round,
            PluginEvent::Detail {
                id: subject,
                result: result.map(|d| {
                    Box::new(PluginDetail {
                        id: id("code-review", "claude-plugins-official"),
                        ..d
                    })
                }),
            },
        );
    }
    app
}

/// Every state worth looking at, named.
pub(super) fn states() -> Vec<(&'static str, App)> {
    let mut states = Vec::new();
    let (app, _) = browsing();
    states.push(("browse: All", app));
    let (mut app, _) = browsing();
    press(&mut app, KeyCode::Right);
    states.push(("browse: Installed", app));
    let (mut app, _) = browsing();
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    for _ in 0..4 {
        press(&mut app, KeyCode::Down);
    }
    states.push(("browse: a marketplace's tab, the long description", app));
    let (mut app, _) = browsing();
    press(&mut app, KeyCode::Left);
    states.push(("browse: Marketplaces", app));
    let (mut app, _) = browsing();
    type_text(&mut app, "zzz");
    states.push(("browse: no match", app));
    let mut app = App::new();
    app.open_plugins_menu();
    states.push(("browse: loading", app));
    let (mut app, round) = browsing();
    let mut fresh = catalog();
    fresh.plugins.retain(|p| p.marketplace == 0);
    fresh.marketplaces[1].fetched = false;
    fresh.marketplaces[1].updated = None;
    app.apply_plugin_event(round, PluginEvent::Catalog(fresh));
    app.apply_plugin_event(
        round,
        PluginEvent::FetchProgress {
            marketplace: "openai-curated".to_string(),
            phase: Some("Receiving objects".to_string()),
            percent: Some(45),
        },
    );
    app.set_plugins_clock(Duration::from_secs(7));
    for _ in 0..3 {
        press(&mut app, KeyCode::Right);
    }
    states.push(("browse: a tab being fetched", app));
    let (mut app, round) = browsing();
    app.apply_plugin_event(
        round,
        PluginEvent::Catalog(PluginCatalog {
            marketplaces: vec![market("openai-curated", "openai/plugins", 0)],
            plugins: Vec::new(),
        }),
    );
    app.apply_plugin_event(
        round,
        PluginEvent::FetchFinished {
            marketplace: "openai-curated".to_string(),
            result: Err("fatal: unable to access 'https://github.com/openai/plugins/': Could not resolve host: github.com".to_string()),
        },
    );
    states.push(("browse: a failed fetch", app));
    let (mut app, round) = browsing();
    app.apply_plugin_event(
        round,
        PluginEvent::EnabledSet {
            id: id("code-review", "claude-plugins-official"),
            enabled: false,
            result: Err("plugins.json is not writable".to_string()),
        },
    );
    states.push(("browse: a failure notice", app));
    states.push(("plugin: every field", on_plugin(Some(Ok(full_detail())))));
    states.push((
        "plugin: components unknown",
        on_plugin(Some(Ok(remote_detail()))),
    ));
    states.push(("plugin: blocked", on_plugin(Some(Ok(blocked_detail())))));
    states.push(("plugin: loading", on_plugin(None)));
    states.push((
        "plugin: failed",
        on_plugin(Some(Err("plugin.json: expected `,` at line 3".to_string()))),
    ));
    let mut app = on_plugin(Some(Ok(full_detail())));
    press(&mut app, KeyCode::Char('3'));
    press(&mut app, KeyCode::Enter);
    states.push(("plugin: uninstall armed", app));
    let mut app = on_plugin(Some(Ok(PluginDetail {
        installed_version: None,
        status: PluginStatus::Available,
        ..full_detail()
    })));
    let round = app.plugins_menu.as_ref().unwrap().round;
    press(&mut app, KeyCode::Enter);
    app.apply_plugin_event(
        round,
        PluginEvent::JobProgress {
            step: Some("cloning github.com/owner/repo".to_string()),
            percent: Some(62),
        },
    );
    app.set_plugins_clock(Duration::from_secs(8));
    states.push(("plugin: installing", app));
    let mut app = on_plugin(Some(Ok(full_detail())));
    let round = app.plugins_menu.as_ref().unwrap().round;
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Enter);
    app.apply_plugin_event(
        round,
        PluginEvent::JobFinished {
            result: Ok(JobDone {
                text: "Installed code-review — 2 skills, 1 command and 5 agents are ready for your next message.".to_string(),
                detail: Some("1 hook and 1 MCP server wait for your review at the next launch.".to_string()),
                goto: None,
            }),
        },
    );
    states.push(("plugin: a result", app));
    let (mut app, _) = browsing();
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Enter);
    states.push(("marketplace", app));
    states.push((
        "plugin: no description",
        on_plugin(Some(Ok(PluginDetail {
            description: String::new(),
            ..full_detail()
        }))),
    ));
    states.push(("plugin: an update waits", outdated_plugin()));
    states.push(("plugin: hooks and servers wait", waiting_plugin()));
    states.push(("plugin: its marketplace removed", orphaned_plugin()));
    let mut app = on_plugin(Some(Ok(PluginDetail {
        status: PluginStatus::Available,
        ..full_detail()
    })));
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Esc);
    states.push(("plugin: an install cancelled", app));
    states.push((
        "browse: Marketplaces, failing and removed",
        failing_markets(),
    ));
    let mut app = failing_markets();
    press(&mut app, KeyCode::Enter);
    states.push(("marketplace: entries skipped", app));
    let (mut app, _) = browsing();
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "acme/agent-plugins");
    states.push(("add", app));
    states
}

/// The page on `code-review`, enabled, at v1.0.0 with v2.0.0 offered.
fn outdated_plugin() -> App {
    on_plugin(Some(Ok(PluginDetail {
        installed_version: Some("1.0.0".to_string()),
        version: Some("2.0.0".to_string()),
        update_available: true,
        ..full_detail()
    })))
}

/// The page on `code-review` with two of its hooks and one server waiting.
fn waiting_plugin() -> App {
    let mut app = on_plugin(Some(Ok(full_detail())));
    app.set_plugin_review(&id("code-review", "claude-plugins-official"), 2, 1, false);
    app
}

/// The page on `code-review`, installed from a marketplace since removed.
fn orphaned_plugin() -> App {
    let (mut app, round) = browsing();
    let mut fresh = catalog();
    fresh.marketplaces[0].fetched = false;
    fresh.marketplaces[0].updated = None;
    fresh.marketplaces[0].error = Some("removed — its installed plugins stay".to_string());
    app.apply_plugin_event(round, PluginEvent::Catalog(fresh));
    press(&mut app, KeyCode::Enter);
    app.apply_plugin_event(
        round,
        PluginEvent::Detail {
            id: id("code-review", "claude-plugins-official"),
            result: Ok(Box::new(full_detail())),
        },
    );
    app
}

/// The Marketplaces tab with a marketplace whose manifest could not be
/// read, one removed, and one that skipped two entries, highlighted.
fn failing_markets() -> App {
    let (mut app, round) = browsing();
    let mut fresh = catalog();
    fresh.marketplaces[0].skipped = vec![
        "日本語 — a plugin name is ASCII letters, digits, ., _ and - only".to_string(),
        "broken-entry — its source has no path and no url, so there is nothing to fetch it from"
            .to_string(),
    ];
    fresh.marketplaces.push(CatalogMarketplace {
        error: Some("marketplace.json: expected `,` or `}` at line 12 column 5".to_string()),
        is_default: false,
        ..market(
            "team-tools",
            "/home/someone/work/team/plugins/marketplace-of-the-team",
            60,
        )
    });
    fresh.marketplaces.push(CatalogMarketplace {
        fetched: false,
        updated: None,
        error: Some("removed — its installed plugins stay".to_string()),
        is_default: false,
        ..market(
            "old-market",
            "github.com/acme/an-old-marketplace-with-a-long-name",
            0,
        )
    });
    app.apply_plugin_event(round, PluginEvent::Catalog(fresh));
    press(&mut app, KeyCode::Left);
    app
}

/// Every row's plain text.
pub(super) fn texts(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

/// The invariants every page keeps at every size: it fits the height, no
/// row is wider than the terminal, no two blank rows stack, and no blank
/// row hugs a rule it keeps — every page that opens and closes on its rules
/// hugs them whole.
#[track_caller]
fn assert_page_keeps_its_shape(what: &str, lines: &[Line<'_>], width: u16, height: u16) {
    let shown = texts(lines);
    assert!(
        lines.len() <= usize::from(height),
        "{what} {width}x{height}: {} rows overflow: {shown:#?}",
        lines.len()
    );
    for line in lines {
        let wide: usize = line.spans.iter().map(|s| cols(&s.content)).sum();
        assert!(
            wide <= usize::from(width),
            "{what} {width}x{height}: a row {wide} columns wide: {:?}",
            plain(line)
        );
    }
    assert!(
        shown
            .windows(2)
            .all(|pair| !(pair[0].is_empty() && pair[1].is_empty())),
        "{what} {width}x{height}: two blank rows stack: {shown:#?}"
    );
    let rule = |t: &String| t.starts_with('─');
    if shown.first().is_some_and(rule) {
        assert!(
            !shown.get(1).is_some_and(String::is_empty),
            "{what} {width}x{height}: a blank row hugs the top rule: {shown:#?}"
        );
    }
    if shown.last().is_some_and(rule) {
        assert!(
            !shown.iter().rev().nth(1).is_some_and(String::is_empty),
            "{what} {width}x{height}: a blank row hugs the bottom rule: {shown:#?}"
        );
    }
    if shown.len() >= 3 && shown.first().is_some_and(rule) && shown.last().is_some_and(rule) {
        assert_frame_hugs_content(&format!("{what} {width}x{height}"), lines);
    }
}

#[test]
fn prints_every_page_at_every_size_and_each_keeps_its_shape() {
    for (what, app) in states() {
        for width in [60_u16, 80, 96, 120, 200] {
            for height in [6_u16, 10, 16, 24, 40] {
                let lines = plugins_view_lines(&app, width, height);
                println!("── {what} · {width}x{height} ──");
                for text in texts(&lines) {
                    println!("{text}");
                }
                assert_page_keeps_its_shape(what, &lines, width, height);
            }
        }
    }
}

#[test]
fn every_page_keeps_its_shape_at_every_height_from_three_up() {
    for (what, app) in states() {
        for width in [40_u16, 60, 96] {
            for height in 3_u16..=48 {
                let lines = plugins_view_lines(&app, width, height);
                assert_page_keeps_its_shape(what, &lines, width, height);
            }
        }
    }
}

#[test]
fn a_tall_terminal_gets_the_whole_page_hugging_its_rules() {
    for (what, app) in states() {
        let lines = plugins_view_lines(&app, 96, 200);
        assert_frame_hugs_content(what, &lines);
    }
}

/// The column a row's description starts in: the first non-blank column
/// after the name, found by its text.
fn column_of(text: &str, needle: &str) -> usize {
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} in {text:?}"));
    cols(&text[..at])
}

#[test]
fn the_whole_browse_page_reads_like_the_spec() {
    let (app, _) = browsing();
    let shown = texts(&plugins_view_lines(&app, 96, 200));
    assert_eq!(shown[1], "  Plugins");
    assert_eq!(
        shown[2],
        "  2 marketplaces · 17 plugins · 3 installed · updated 2h ago"
    );
    assert_eq!(shown[3], "");
    assert!(shown[4].starts_with("   All    Installed 3    claude-plugins-official 12"));
    assert_eq!(shown[5], "  ❯");
    assert_eq!(shown[6], "");
    assert!(shown[7].starts_with("  → ✔ code-review"), "{:?}", shown[7]);
    assert!(shown[8].starts_with("    ◯ feature-dev"));
    assert!(shown[9].starts_with("    ✔ linear ↑"), "{:?}", shown[9]);
    assert!(shown[10].starts_with("      agent-sdk-dev"));
    assert_eq!(shown[15], "    (1/17)");
    assert!(shown[17].starts_with("    Automated code review"));
    assert!(
        shown
            .iter()
            .any(|t| t == "    development · Anthropic · v1.0.0")
    );
    assert_eq!(
        shown[shown.len() - 2],
        "  Enter for details · Space to disable · ←/→ tabs · Type to search · Esc to close"
    );
    assert!(wears_the_title_dress(&plugins_view_lines(&app, 96, 200)[1]));
}

#[test]
fn the_columns_line_up_down_the_list_wide_glyphs_and_all() {
    let (mut app, _) = browsing();
    for _ in 0..4 {
        press(&mut app, KeyCode::End);
    }
    // Scroll to show the CJK and emoji rows.
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Up);
    let shown = texts(&plugins_view_lines(&app, 120, 200));
    let rows: Vec<&String> = shown
        .iter()
        .filter(|t| t.contains("claude-plugins-official") && !t.contains("All"))
        .collect();
    assert!(rows.len() >= 3, "{shown:#?}");
    let market_cols: Vec<usize> = rows
        .iter()
        .map(|t| column_of(t, "claude-plugins-official"))
        .collect();
    assert!(
        market_cols.iter().all(|c| *c == market_cols[0]),
        "the marketplace column holds still: {market_cols:?} {rows:#?}"
    );
    assert!(
        rows.iter().all(|t| cols(t) == 120),
        "flush right: {rows:#?}"
    );
    let cjk = shown
        .iter()
        .find(|t| t.contains("翻訳ツール"))
        .expect("the CJK row");
    let plain_row = shown.iter().find(|t| t.contains("xcode")).expect("a row");
    assert_eq!(
        column_of(cjk, "日本語"),
        column_of(plain_row, "Builds, tests"),
        "the description column lines up past a wide name"
    );
}

#[test]
fn a_narrow_terminal_gives_up_the_marketplace_then_the_description_never_the_name() {
    let (app, _) = browsing();
    let at = |width| texts(&plugins_view_lines(&app, width, 200));
    let wide = at(96);
    assert!(
        wide[7].ends_with("claude-plugins-official"),
        "{:?}",
        wide[7]
    );
    let medium = at(80);
    assert!(!medium[7].contains("claude-plugins-official"));
    assert!(medium[7].contains("Automated code review"));
    let narrow = at(50);
    assert!(
        narrow.iter().any(|t| t.trim() == "→ ✔ code-review"),
        "{narrow:#?}"
    );
}

#[test]
fn the_active_tab_is_a_chip_and_a_strip_too_wide_keeps_it_whole() {
    let (mut app, _) = browsing();
    press(&mut app, KeyCode::Right);
    let lines = plugins_view_lines(&app, 96, 200);
    let chip = lines[4]
        .spans
        .iter()
        .find(|s| s.content.contains("Installed 3"))
        .expect("the tab");
    assert_eq!(chip.style.bg, Some(footer_focus_bg()));
    // Narrower: the active tab stays whole between cut edges.
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    let strip = texts(&plugins_view_lines(&app, 64, 200))[4].clone();
    assert!(strip.contains("openai-curated 5"), "{strip:?}");
    assert!(strip.contains('‹'), "{strip:?}");
    // Under sixty columns: the active tab alone, and where it is.
    let strip = texts(&plugins_view_lines(&app, 50, 200))[4].clone();
    assert_eq!(strip, "  ‹  openai-curated 5  › (4/5)");
}

#[test]
fn a_tab_being_fetched_wears_the_spinner_and_one_that_failed_a_red_cross() {
    let (mut app, round) = browsing();
    app.apply_plugin_event(
        round,
        PluginEvent::FetchStarted {
            marketplace: "openai-curated".to_string(),
        },
    );
    let strip = texts(&plugins_view_lines(&app, 120, 200))[4].clone();
    assert!(!strip.contains("openai-curated 5"), "{strip:?}");
    let subtitle = texts(&plugins_view_lines(&app, 120, 200))[2].clone();
    assert!(subtitle.ends_with("refreshing…"), "{subtitle:?}");
    app.apply_plugin_event(
        round,
        PluginEvent::FetchFinished {
            marketplace: "openai-curated".to_string(),
            result: Err("no network".to_string()),
        },
    );
    let lines = plugins_view_lines(&app, 120, 200);
    let cross = lines[4]
        .spans
        .iter()
        .find(|s| s.content == "✗")
        .expect("the cross");
    assert_eq!(cross.style.fg, Some(tool_fail_color()));
    assert!(texts(&lines).last().is_some());
    let hint = texts(&lines)[lines.len() - 2].clone();
    assert!(hint.ends_with("Ctrl+R to refresh"), "{hint:?}");
}

#[test]
fn an_empty_tab_says_why_in_the_list_s_place() {
    let (mut app, _) = browsing();
    type_text(&mut app, "zzz");
    let shown = texts(&plugins_view_lines(&app, 96, 200));
    assert!(
        shown.contains(&"  No plugins match \"zzz\"".to_string()),
        "{shown:#?}"
    );
    assert!(shown[shown.len() - 2].ends_with("Esc to clear"));
    let mut app = App::new();
    app.open_plugins_menu();
    let shown = texts(&plugins_view_lines(&app, 96, 200));
    assert!(
        shown.contains(&"  Loading plugins…".to_string()),
        "{shown:#?}"
    );
    let (mut app, round) = browsing();
    let mut nothing = catalog();
    for plugin in &mut nothing.plugins {
        plugin.status = PluginStatus::Available;
    }
    app.apply_plugin_event(round, PluginEvent::Catalog(nothing));
    press(&mut app, KeyCode::Right);
    let shown = texts(&plugins_view_lines(&app, 96, 200));
    assert!(
        shown.iter().any(|t| t.contains("Nothing installed yet")),
        "{shown:#?}"
    );
}

#[test]
fn a_fetch_in_the_list_s_place_shows_git_s_phase_its_percent_and_the_time() {
    let states = states();
    let (_, app) = states
        .iter()
        .find(|(what, _)| *what == "browse: a tab being fetched")
        .unwrap();
    let shown = texts(&plugins_view_lines(app, 96, 200));
    let row = shown
        .iter()
        .find(|t| t.contains("Fetching openai-curated"))
        .unwrap_or_else(|| panic!("{shown:#?}"));
    assert!(
        row.ends_with("Fetching openai-curated · receiving objects 45% · 2s"),
        "{row:?}"
    );
    let (_, app) = states
        .iter()
        .find(|(what, _)| *what == "browse: a failed fetch")
        .unwrap();
    let shown = texts(&plugins_view_lines(app, 60, 200));
    assert!(
        shown
            .iter()
            .any(|t| t.starts_with("  ✗ Couldn't fetch openai-curated — fatal")),
        "{shown:#?}"
    );
    assert!(
        shown
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("Could not resolve host: github.com"),
        "never cut: {shown:#?}"
    );
    assert!(shown.contains(&"  Ctrl+R to try again".to_string()));
}

#[test]
fn the_marketplaces_tab_lists_sources_and_facts_then_the_add_and_restore_rows() {
    let (mut app, _) = browsing();
    press(&mut app, KeyCode::Left);
    let shown = texts(&plugins_view_lines(&app, 96, 200));
    let first = shown
        .iter()
        .find(|t| t.starts_with("  → claude-plugins-official"))
        .unwrap_or_else(|| panic!("{shown:#?}"));
    assert!(first.contains("anthropics/claude-plugins-official"));
    assert!(first.ends_with("12 plugins · updated 2h ago"), "{first:?}");
    assert!(shown.iter().any(|t| t.contains("+ Add a marketplace")));
    assert!(shown.iter().any(|t| t.contains("+ Restore acme/plugins")));
    assert_eq!(
        shown[shown.len() - 2],
        "  Enter to manage · ←/→ tabs · Esc to close"
    );
}

#[test]
fn the_plugin_page_states_status_facts_and_actions() {
    let app = on_plugin(Some(Ok(full_detail())));
    let lines = plugins_view_lines(&app, 96, 200);
    let shown = texts(&lines);
    assert_eq!(shown[1], "  Plugins › code-review");
    assert_eq!(
        shown[2],
        "  ✔ installed · enabled · v1.0.0 · claude-plugins-official"
    );
    for row in [
        "  Author      Anthropic",
        "  Category    development",
        "  License     Apache-2.0",
        "  Source      plugins/code-review",
        "  Skills      2 · code-review:lint, code-review:triage",
        "  Commands    1 · /code-review:code-review",
        "  Hooks       1 · PreToolUse · Bash",
        "  MCP         1 · github (http)",
        "  Not used    1 app · 2 LSP servers",
        "  ❯ 1. Disable            Stop loading it; it stays installed",
        "    2. Check for updates",
        "    3. Uninstall",
        "    4. Back",
        "  ↑/↓ or 1–9 to choose · Enter to confirm · Esc to go back",
    ] {
        assert!(shown.contains(&row.to_string()), "{row:?} in {shown:#?}");
    }
    assert!(shown.iter().any(|t| t.starts_with("  ⚠ hooks/extra.json")));
    // The source is a link to its web page.
    let source = lines
        .iter()
        .find(|l| plain(l).contains("plugins/code-review") && plain(l).contains("Source"))
        .unwrap();
    assert!(
        source
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::UNDERLINED)),
        "{source:?}"
    );
    let mark = lines[2].spans.iter().find(|s| s.content == "✔").unwrap();
    assert_eq!(mark.style.fg, Some(model_active_color()));
}

#[test]
fn a_plugin_not_installed_says_what_will_ask_for_review_and_one_elsewhere_says_later() {
    let app = on_plugin(Some(Ok(PluginDetail {
        installed_version: None,
        status: PluginStatus::Available,
        ..full_detail()
    })));
    let shown = texts(&plugins_view_lines(&app, 120, 200));
    assert_eq!(
        shown[2],
        "  not installed · v1.0.0 · claude-plugins-official"
    );
    assert!(
        shown.contains(
            &"  Hooks       1 · PreToolUse · Bash — asks for your review at the next launch"
                .to_string()
        ),
        "{shown:#?}"
    );
    assert!(shown.iter().any(|t| t.starts_with("  ❯ 1. Install")));
    let app = on_plugin(Some(Ok(remote_detail())));
    let shown = texts(&plugins_view_lines(&app, 120, 200));
    assert!(
        shown.contains(
            &"  Contains    shown after install — its files are in another repository".to_string()
        ),
        "{shown:#?}"
    );
}

#[test]
fn a_blocked_plugin_offers_only_back_under_why() {
    let app = on_plugin(Some(Ok(blocked_detail())));
    let shown = texts(&plugins_view_lines(&app, 96, 200));
    assert!(shown.contains(&"  Can't install  npm sources are not supported".to_string()));
    assert!(shown.contains(&"  ⊘ can't be installed here · claude-plugins-official".to_string()));
    assert!(shown.contains(&"  ❯ 1. Back".to_string()));
    assert!(!shown.iter().any(|t| t.contains("Install ")));
}

#[test]
fn a_running_job_replaces_the_actions_and_esc_is_the_hint() {
    let states = states();
    let (_, app) = states
        .iter()
        .find(|(what, _)| *what == "plugin: installing")
        .unwrap();
    let shown = texts(&plugins_view_lines(app, 120, 200));
    let row = shown
        .iter()
        .find(|t| t.contains("Installing code-review"))
        .unwrap_or_else(|| panic!("{shown:#?}"));
    assert!(
        row.ends_with("Installing code-review · cloning github.com/owner/repo · 62% · 3s"),
        "{row:?}"
    );
    assert!(!shown.iter().any(|t| t.contains("1. Install")));
    assert_eq!(shown[shown.len() - 2], "  Esc to cancel");
}

#[test]
fn a_result_is_stated_on_the_page_above_the_actions() {
    let states = states();
    let (_, app) = states
        .iter()
        .find(|(what, _)| *what == "plugin: a result")
        .unwrap();
    let shown = texts(&plugins_view_lines(app, 120, 200));
    let at = shown
        .iter()
        .position(|t| t.starts_with("  ✔ Installed code-review — 2 skills"))
        .unwrap_or_else(|| panic!("{shown:#?}"));
    assert_eq!(
        shown[at + 1],
        "    1 hook and 1 MCP server wait for your review at the next launch."
    );
    assert!(shown[at + 3].contains("1. Disable"));
}

#[test]
fn uninstall_armed_asks_in_amber_in_place() {
    let states = states();
    let (_, app) = states
        .iter()
        .find(|(what, _)| *what == "plugin: uninstall armed")
        .unwrap();
    let shown = texts(&plugins_view_lines(app, 96, 200));
    assert!(shown.contains(&"  ❯ 3. Uninstall — press Enter again to remove it".to_string()));
}

#[test]
fn the_marketplace_page_shows_its_source_revision_and_where_it_is_kept() {
    let states = states();
    let (_, app) = states
        .iter()
        .find(|(what, _)| *what == "marketplace")
        .unwrap();
    let shown = texts(&plugins_view_lines(app, 96, 200));
    assert_eq!(shown[1], "  Plugins › claude-plugins-official");
    assert_eq!(
        shown[2],
        "  12 plugins · 2 installed · updated 2h ago · default marketplace"
    );
    for row in [
        "  Source      anthropics/claude-plugins-official",
        "  Revision    d7d3279",
        "  Kept at     ~/.alter-zero/plugins/marketplaces/claude-plugins-official",
        "  ❯ 1. Browse its plugins  Open its tab of the catalog",
        "    2. Update now",
        "    3. Remove",
        "    4. Back",
    ] {
        assert!(shown.contains(&row.to_string()), "{row:?} in {shown:#?}");
    }
}

#[test]
fn the_add_page_has_its_field_and_examples() {
    let states = states();
    let (_, app) = states.iter().find(|(what, _)| *what == "add").unwrap();
    let shown = texts(&plugins_view_lines(app, 96, 200));
    assert_eq!(shown[1], "  Plugins › Add a marketplace");
    assert!(shown.contains(&"  ❯ acme/agent-plugins".to_string()));
    assert!(shown.contains(&"  Examples    acme/agent-plugins".to_string()));
    assert!(shown.contains(&"              ~/work/my-plugins".to_string()));
    assert_eq!(shown[shown.len() - 2], "  Enter to add · Esc to go back");
}

#[test]
fn a_short_terminal_keeps_the_browse_search_line_and_the_highlighted_row() {
    let (app, _) = browsing();
    assert_sheds_to_its_search_line(
        |height| texts(&plugins_view_lines(&app, 96, height)),
        "",
        "code-review",
        "Esc to close",
    );
}

#[test]
fn a_short_terminal_keeps_the_highlighted_action_on_a_detail_page() {
    let app = on_plugin(Some(Ok(full_detail())));
    for height in 1..=40_u16 {
        let shown = texts(&plugins_view_lines(&app, 96, height));
        assert!(shown.len() <= usize::from(height));
        assert!(
            shown.iter().any(|t| t.contains("1. Disable")),
            "{height} rows: {shown:#?}"
        );
    }
}

#[test]
fn the_page_is_never_flow_eligible() {
    let (app, _) = browsing();
    assert!(crate::ui::view_flow(&app, 96, 10, 1_000).is_none());
}

#[test]
fn rendering_paints_the_built_page() {
    let (app, _) = browsing();
    let height = plugins_menu_height(&app, 96, 40).unwrap();
    let mut buf = buffer(96, height);
    render_plugins_menu(buf.area, &mut buf, &app);
    let lines = plugins_view_lines(&app, 96, height);
    for y in 0..height {
        // A wide glyph's shadow cell reads back as a space of its own, so
        // the two are compared word for word.
        let words = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(
            words(&row(&buf, y, 96)),
            words(&plain(&lines[usize::from(y)]))
        );
    }
    assert!(app.plugins_menu.as_ref().unwrap().page == PluginsPage::Browse);
}

#[test]
fn a_hint_too_wide_drops_the_keys_the_page_already_shows_before_it_cuts() {
    let (app, _) = browsing();
    let hint = |width| {
        let shown = texts(&plugins_view_lines(&app, width, 200));
        shown[shown.len() - 2].clone()
    };
    assert_eq!(
        hint(96),
        "  Enter for details · Space to disable · ←/→ tabs · Type to search · Esc to close"
    );
    assert_eq!(
        hint(80),
        "  Enter for details · Space to disable · ←/→ tabs · Esc to close"
    );
    assert_eq!(
        hint(60),
        "  Enter for details · Space to disable · Esc to close"
    );
}

/// The longest real name, 53 columns.
const LONGEST_NAME: &str = "oracle-ai-data-platform-workbench-engineer-agent-suit";

/// [`browsing`] with a plugin of [`LONGEST_NAME`] in the catalog, highlighted.
fn browsing_a_long_name() -> App {
    let (mut app, round) = browsing();
    let mut fresh = catalog();
    let mut long = plug(
        LONGEST_NAME,
        0,
        PluginStatus::Available,
        "Oracle's AI data platform",
    );
    long.category = Some("database".to_string());
    long.author = Some("Oracle".to_string());
    long.version = Some("1.0".to_string());
    fresh.plugins.push(long);
    app.apply_plugin_event(round, PluginEvent::Catalog(fresh));
    type_text(&mut app, "oracle");
    app
}

#[test]
fn a_long_name_is_cut_at_twenty_two_columns_and_named_whole_under_the_description() {
    let app = browsing_a_long_name();
    assert_eq!(LONGEST_NAME.len(), 53);
    let shown = texts(&plugins_view_lines(&app, 120, 200));
    let row = shown
        .iter()
        .find(|t| t.starts_with("  →   oracle"))
        .unwrap_or_else(|| panic!("{shown:#?}"));
    let name = row["  →   ".len()..].split("  ").next().unwrap();
    assert_eq!(cols(name), 22, "{row:?}");
    assert!(name.ends_with('…'), "{row:?}");
    assert!(
        shown.contains(&format!("    {LONGEST_NAME} · database · Oracle · v1.0")),
        "{shown:#?}"
    );
    // A name that fits is not repeated.
    let (app, _) = browsing();
    let shown = texts(&plugins_view_lines(&app, 120, 200));
    assert!(shown.contains(&"    development · Anthropic · v1.0.0".to_string()));
}

#[test]
fn a_sixteen_row_terminal_shows_more_than_three_plugin_rows() {
    let (app, _) = browsing();
    for width in [80_u16, 96] {
        let shown = texts(&plugins_view_lines(&app, width, 16));
        let rows = shown
            .iter()
            .filter(|t| {
                t.starts_with("  → ")
                    || t.starts_with("    ✔")
                    || t.starts_with("    ◯")
                    || t.starts_with("    ↑")
                    || t.starts_with("      ")
            })
            .count();
        assert!(rows > 3, "{width}x16: {shown:#?}");
        assert!(shown.iter().any(|t| t.trim() == "(1/17)"), "{shown:#?}");
    }
}

#[test]
fn a_sixteen_row_plugin_page_keeps_one_contains_row() {
    let app = on_plugin(Some(Ok(full_detail())));
    let shown = texts(&plugins_view_lines(&app, 80, 16));
    assert!(
        shown.contains(
            &"  Contains    2 skills · 1 command · 5 agents · 1 hook · 1 MCP server".to_string()
        ),
        "{shown:#?}"
    );
    assert!(shown.iter().any(|t| t.contains("1. Disable")));
}

#[test]
fn the_page_s_spinner_is_one_cell_whatever_the_session_s_style() {
    let (mut app, round) = browsing();
    let strip = |app: &App| {
        let lines = plugins_view_lines(app, 120, 200);
        lines[4]
            .spans
            .iter()
            .map(|s| cols(&s.content))
            .sum::<usize>()
    };
    let fetched = strip(&app);
    app.apply_plugin_event(
        round,
        PluginEvent::FetchStarted {
            marketplace: "openai-curated".to_string(),
        },
    );
    // "5" gives way to a one-cell spinner: the strip keeps its width.
    assert_eq!(strip(&app), fetched);
    let shown = texts(&plugins_view_lines(&app, 120, 200));
    assert!(
        crate::ui::theme::SPINNER_DOTS_FRAMES
            .iter()
            .any(|frame| shown[4].contains(&format!("openai-curated {frame}    "))),
        "{:?}",
        shown[4]
    );
}

#[test]
fn a_removed_marketplace_reads_dim_as_removed_never_as_a_failed_fetch() {
    let (mut app, round) = browsing();
    let mut fresh = catalog();
    fresh.marketplaces.push(CatalogMarketplace {
        skipped: Vec::new(),
        fetched: false,
        updated: None,
        error: Some("removed — its installed plugins stay".to_string()),
        is_default: false,
        ..market("old-market", "acme/old", 0)
    });
    let mut kept = plug("kept", 2, PluginStatus::Enabled, "Still installed");
    kept.marketplace = 2;
    fresh.plugins.push(kept);
    app.apply_plugin_event(round, PluginEvent::Catalog(fresh));
    press(&mut app, KeyCode::Left);
    let lines = plugins_view_lines(&app, 120, 200);
    let shown = texts(&lines);
    let row = shown
        .iter()
        .find(|t| t.contains("old-market") && t.contains("acme/old"))
        .unwrap_or_else(|| panic!("{shown:#?}"));
    assert!(row.ends_with("removed"), "{row:?}");
    assert!(!shown.iter().any(|t| t.contains('✗')), "{shown:#?}");
    assert!(!shown.iter().any(|t| t.contains("Ctrl+R")), "{shown:#?}");
}

#[test]
fn a_marketplace_never_fetched_is_named_alone_in_the_strip() {
    let (mut app, round) = browsing();
    let mut fresh = catalog();
    fresh.plugins.retain(|p| p.marketplace == 0);
    fresh.marketplaces[1].fetched = false;
    fresh.marketplaces[1].updated = None;
    app.apply_plugin_event(round, PluginEvent::Catalog(fresh));
    let strip = texts(&plugins_view_lines(&app, 120, 200))[4].clone();
    assert!(
        strip.contains("  openai-curated    Marketplaces"),
        "{strip:?}"
    );
}

#[test]
fn a_plugin_with_no_description_stacks_no_blank_rows() {
    let states = states();
    let (_, app) = states
        .iter()
        .find(|(what, _)| *what == "plugin: no description")
        .unwrap();
    let shown = texts(&plugins_view_lines(app, 96, 200));
    assert_eq!(
        shown[2],
        "  ✔ installed · enabled · v1.0.0 · claude-plugins-official"
    );
    assert_eq!(shown[3], "");
    assert!(shown[4].starts_with("  Author"), "{shown:#?}");
}

#[test]
fn what_a_plugin_brings_is_wrapped_whole_and_cut_to_a_row_on_a_short_terminal() {
    let mut detail = full_detail();
    let components = detail.components.as_mut().unwrap();
    components.commands = ["configure", "help", "list", "hookify"]
        .iter()
        .map(|c| format!("/hookify:{c}"))
        .collect();
    components.hooks = ["PreToolUse", "PostToolUse", "Stop", "UserPromptSubmit"]
        .iter()
        .map(ToString::to_string)
        .collect();
    let app = on_plugin(Some(Ok(detail)));
    let shown = texts(&plugins_view_lines(&app, 60, 40));
    let words = shown.join(" ");
    for name in ["/hookify:hookify", "UserPromptSubmit"] {
        assert!(words.contains(name), "{name} in {shown:#?}");
    }
    assert!(
        shown
            .iter()
            .any(|t| t == "  Hooks       4 · PreToolUse, PostToolUse, Stop,"),
        "{shown:#?}"
    );
    assert!(
        shown.iter().any(|t| t == "              UserPromptSubmit"),
        "{shown:#?}"
    );
    // A short terminal cuts each list to a row, the count still exact.
    let cut = (10..40_u16)
        .map(|h| texts(&plugins_view_lines(&app, 60, h)))
        .find(|shown| {
            shown
                .iter()
                .any(|t| t.starts_with("  Hooks       4 · ") && t.ends_with('…'))
        });
    assert!(cut.is_some(), "a one-row cut on some short terminal");
}

#[test]
fn an_update_sits_beside_the_status_never_in_its_place() {
    let (mut app, _) = browsing();
    for _ in 0..2 {
        press(&mut app, KeyCode::Down);
    }
    let shown = texts(&plugins_view_lines(&app, 96, 200));
    assert!(
        shown.iter().any(|t| t.starts_with("  → ✔ linear ↑")),
        "{shown:#?}"
    );
    // Space disables it: the row says so, and still that an update waits.
    press(&mut app, KeyCode::Char(' '));
    let shown = texts(&plugins_view_lines(&app, 96, 200));
    assert!(
        shown.iter().any(|t| t.starts_with("  → ◯ linear ↑")),
        "{shown:#?}"
    );
    let page = texts(&plugins_view_lines(&outdated_plugin(), 120, 200));
    assert_eq!(
        page[2],
        "  ✔ installed · enabled · ↑ update available · v1.0.0 → v2.0.0 · claude-plugins-official"
    );
    assert!(page.iter().any(|t| t.contains("1. Disable")), "{page:#?}");
}

#[test]
fn the_row_footer_names_both_versions_while_an_update_waits() {
    let (mut app, round) = browsing();
    let mut fresh = catalog();
    fresh.plugins[12].installed_version = Some("1.0.0".to_string());
    fresh.plugins[12].version = Some("2.0.0".to_string());
    app.apply_plugin_event(round, PluginEvent::Catalog(fresh));
    type_text(&mut app, "linear");
    let shown = texts(&plugins_view_lines(&app, 96, 200));
    assert!(
        shown.iter().any(|t| t.ends_with("v1.0.0 → v2.0.0")),
        "{shown:#?}"
    );
}

#[test]
fn hooks_and_servers_waiting_for_the_user_are_said_with_the_way_forward() {
    let shown = texts(&plugins_view_lines(&waiting_plugin(), 120, 200));
    assert_eq!(
        shown[3],
        "  2 hooks wait for your review — /hooks · 1 MCP server is off — /mcp"
    );
    let mut app = on_plugin(Some(Ok(full_detail())));
    app.set_plugin_review(&id("code-review", "claude-plugins-official"), 1, 0, true);
    let shown = texts(&plugins_view_lines(&app, 120, 200));
    assert_eq!(
        shown[3],
        "  1 hook waits for your review at the next launch"
    );
    app.set_plugin_review(&id("code-review", "claude-plugins-official"), 0, 0, false);
    let shown = texts(&plugins_view_lines(&app, 120, 200));
    assert_eq!(shown[3], "", "nothing waits: no line");
}

#[test]
fn a_plugin_from_a_removed_marketplace_says_why_it_has_no_update() {
    let shown = texts(&plugins_view_lines(&orphaned_plugin(), 120, 200));
    assert!(
        shown
            .iter()
            .any(|t| t.starts_with("  Updates     none — its marketplace was removed")),
        "{shown:#?}"
    );
    assert!(
        !shown.iter().any(|t| t.contains("Check for updates")),
        "{shown:#?}"
    );
}

#[test]
fn a_plugin_that_cannot_be_installed_promises_nothing_after_install() {
    let shown = texts(&plugins_view_lines(
        &on_plugin(Some(Ok(blocked_detail()))),
        120,
        200,
    ));
    assert!(
        !shown.iter().any(|t| t.contains("shown after install")),
        "{shown:#?}"
    );
}

#[test]
fn a_cancelled_job_is_said_on_the_page() {
    let states = states();
    let (_, app) = states
        .iter()
        .find(|(what, _)| *what == "plugin: an install cancelled")
        .unwrap();
    let shown = texts(&plugins_view_lines(app, 96, 200));
    assert!(
        shown.contains(&"  ✗ Cancelled — install code-review".to_string()),
        "{shown:#?}"
    );
    assert!(shown.iter().any(|t| t.contains("1. Install")), "{shown:#?}");
}

#[test]
fn at_eighty_columns_a_marketplace_s_failure_outranks_its_source() {
    let app = failing_markets();
    let shown = texts(&plugins_view_lines(&app, 80, 200));
    let row = |name: &str| {
        shown
            .iter()
            .find(|t| t.contains(name) && !t.contains("Marketplaces"))
            .unwrap_or_else(|| panic!("{name} in {shown:#?}"))
            .clone()
    };
    let team = row("team-tools");
    assert!(team.contains("✗ marketplace.json"), "{team:?}");
    assert!(team.contains('…'), "the source is cut first: {team:?}");
    assert!(row("old-market").ends_with("removed"));
    assert!(row("claude-plugins-official").contains("12 plugins · 2 skipped"));
    for (_, app) in [("80", &app)] {
        for line in plugins_view_lines(app, 80, 200) {
            assert!(line.spans.iter().map(|s| cols(&s.content)).sum::<usize>() <= 80);
        }
    }
}

#[test]
fn a_marketplace_page_lists_the_entries_it_skipped() {
    let states = states();
    let (_, app) = states
        .iter()
        .find(|(what, _)| *what == "marketplace: entries skipped")
        .unwrap();
    let shown = texts(&plugins_view_lines(app, 80, 200));
    assert!(
        shown[2].contains("12 plugins · 2 installed · 2 skipped"),
        "{shown:#?}"
    );
    assert!(
        shown
            .iter()
            .any(|t| t.starts_with("  Skipped     日本語 — a plugin name is ASCII")),
        "{shown:#?}"
    );
    let words = shown
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        words.contains("so there is nothing to fetch it from"),
        "{shown:#?}"
    );
}

#[test]
fn the_search_line_cuts_a_long_query_from_the_left_with_an_ellipsis() {
    let (mut app, _) = browsing();
    type_text(&mut app, &"review ".repeat(12));
    let shown = texts(&plugins_view_lines(&app, 60, 200));
    let search = shown.iter().find(|t| t.starts_with("  ❯")).unwrap();
    assert!(
        search.starts_with("  ❯ …") || search.starts_with("  ❯…"),
        "{search:?}"
    );
    assert!(cols(search) <= 60);
}

#[test]
fn a_narrow_detail_page_keeps_the_keys_that_act_in_its_hint() {
    let app = on_plugin(Some(Ok(full_detail())));
    // With room, the whole hint.
    let wide = texts(&plugins_view_lines(&app, 96, 200));
    assert!(
        wide.iter()
            .any(|t| t.trim() == "↑/↓ or 1–9 to choose · Enter to confirm · Esc to go back"),
        "{wide:#?}"
    );
    // At forty columns the clause the list already shows goes and the two
    // keys that act stay whole — never a hint cut in the middle of a word.
    let narrow = texts(&plugins_view_lines(&app, 40, 200));
    let hint = narrow
        .iter()
        .rev()
        .find(|t| t.contains("Enter to"))
        .expect("a hint row");
    assert_eq!(
        hint.trim(),
        "Enter to confirm · Esc to go back",
        "{narrow:#?}"
    );
}

#[test]
fn the_subtitle_counts_only_the_marketplaces_the_user_has() {
    let app = failing_markets();
    let shown = texts(&plugins_view_lines(&app, 120, 200));
    assert!(shown[2].starts_with("  3 marketplaces"), "{:?}", shown[2]);
}

#[test]
fn a_marketplace_that_could_not_be_read_wears_a_red_cross_in_the_strip() {
    let shown = texts(&plugins_view_lines(&failing_markets(), 120, 200));
    assert!(shown[4].contains("team-tools ✗"), "{:?}", shown[4]);
}
