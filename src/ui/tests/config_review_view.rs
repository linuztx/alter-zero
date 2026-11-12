//! The startup review's page (`docs/state/config-review.md`).

use super::*;
use crate::app::{ItemChange, ReviewItem, ReviewKey, ReviewKind, ReviewPage, ReviewSection};
use crate::trust::HookIdentity;
use crate::ui::theme::{
    REVIEW_DISABLE_LABEL, REVIEW_ENABLE_LABEL, REVIEW_SKIP_LABEL, REVIEW_TRUST_LABEL,
};
use crate::ui::{config_review_height, config_review_lines, render_config_review};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const TALL: u16 = 200;

fn hook(event: &str, matcher: &str, command: &str, previous: Option<&str>) -> ReviewItem {
    let identity = HookIdentity {
        event: event.to_string(),
        matcher: matcher.to_string(),
        command: command.to_string(),
    };
    ReviewItem {
        title: identity.label(),
        detail: vec![command.to_string()],
        change: previous.map(|command| ItemChange::Was(command.to_string())),
        checked: true,
        key: ReviewKey::Hook {
            source: "/home/u/.alter-zero/hooks.json".to_string(),
            identity,
        },
    }
}

fn server(name: &str, transport: &str, detail: &[&str]) -> ReviewItem {
    ReviewItem {
        key: ReviewKey::Server {
            source: "/home/u/repo/.mcp.json".to_string(),
            name: name.to_string(),
        },
        title: format!("{name} · {transport}"),
        detail: detail.iter().map(|d| (*d).to_string()).collect(),
        change: None,
        checked: true,
    }
}

fn hooks_page() -> ReviewPage {
    ReviewPage::new(
        ReviewKind::Hooks,
        vec![
            ReviewSection {
                scope: Some("User".to_string()),
                path: "~/.alter-zero/hooks.json".to_string(),
                items: vec![hook(
                    "Stop",
                    "",
                    "cargo fmt && cargo clippy --all-targets -- -D warnings",
                    Some("cargo fmt"),
                )],
            },
            ReviewSection {
                scope: Some("Project".to_string()),
                path: "~/repo/.alter-zero/hooks.json".to_string(),
                items: vec![hook(
                    "PreToolUse",
                    "bash",
                    "${ALTER_ZERO_PROJECT_DIR}/.alter-zero/hooks/guard.sh",
                    None,
                )],
            },
        ],
    )
}

fn servers_page() -> ReviewPage {
    ReviewPage::new(
        ReviewKind::McpServers,
        vec![ReviewSection {
            scope: None,
            path: "~/repo/.mcp.json".to_string(),
            items: vec![
                server(
                    "brave-devtools",
                    "stdio",
                    &[
                        "npx -y @brave/devtools-mcp",
                        "env  BRAVE_API_KEY=${BRAVE_API_KEY}",
                    ],
                ),
                server("github", "http", &["https://api.githubcopilot.com/mcp/"]),
            ],
        }],
    )
}

fn review_app(pages: Vec<ReviewPage>) -> App {
    let mut app = App::new();
    app.open_config_review(pages);
    app
}

/// The built page as trimmed plain rows.
fn texts(app: &App, width: u16, height: u16) -> Vec<String> {
    config_review_lines(app, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

fn find(texts: &[String], needle: &str) -> usize {
    texts
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line contains {needle:?}: {texts:#?}"))
}

/// A page of `n` hooks, each with a two-row command at 60 columns.
fn many_hooks(n: usize) -> ReviewPage {
    ReviewPage::new(
        ReviewKind::Hooks,
        vec![ReviewSection {
            scope: Some("User".to_string()),
            path: "~/.alter-zero/hooks.json".to_string(),
            items: (0..n)
                .map(|i| {
                    hook(
                        "PreToolUse",
                        "bash",
                        &format!("./scripts/guard-{i}.sh --strict --report /tmp/guard-{i}.log"),
                        None,
                    )
                })
                .collect(),
        }],
    )
}

#[test]
fn the_hooks_page_shows_each_hook_verbatim_under_its_file() {
    let app = review_app(vec![hooks_page(), servers_page()]);
    let texts = texts(&app, 80, TALL);
    let title = find(&texts, "2 hooks to review · 1 new, 1 changed");
    assert!(texts[title].ends_with("1/2"), "{:?}", texts[title]);
    let info = find(&texts, "Hooks run shell commands on their own");
    let user = find(&texts, "User · ~/.alter-zero/hooks.json");
    let stop = find(&texts, "[✔] Stop");
    let command = find(
        &texts,
        "cargo fmt && cargo clippy --all-targets -- -D warnings",
    );
    let was = find(&texts, "was: cargo fmt");
    let project = find(&texts, "Project · ~/repo/.alter-zero/hooks.json");
    let guard = find(&texts, "[✔] PreToolUse · bash");
    let trust = find(&texts, REVIEW_TRUST_LABEL);
    let skip = find(&texts, REVIEW_SKIP_LABEL);
    let hint = find(&texts, "Esc to continue without trusting");
    assert!(
        title < info
            && info < user
            && user < stop
            && stop < command
            && command < was
            && was < project
            && project < guard
            && guard < trust
            && trust < skip
            && skip < hint,
        "{texts:#?}"
    );
    // The highlighted item wears the ❯, and a changed one says so.
    assert!(texts[stop].contains('❯'), "{:?}", texts[stop]);
    assert!(texts[stop].ends_with("changed"), "{:?}", texts[stop]);
    assert!(
        !texts[guard].contains("changed"),
        "a new hook wears no badge"
    );
    assert!(!texts[guard].contains('❯'));
    assert_frame_hugs_content("the hooks page", &config_review_lines(&app, 80, TALL));
}

#[test]
fn the_title_wears_the_title_dress() {
    let app = review_app(vec![hooks_page()]);
    let lines = config_review_lines(&app, 80, TALL);
    let title = lines
        .iter()
        .find(|line| plain(line).contains("hooks to review"))
        .expect("the title row");
    assert!(wears_the_title_dress(title), "{title:?}");
    // A one-page review counts no pages.
    assert!(!plain(title).contains("1/1"), "{:?}", plain(title));
}

#[test]
fn the_titles_count_what_is_new_and_what_changed() {
    let only_new = ReviewPage::new(
        ReviewKind::Hooks,
        vec![ReviewSection {
            scope: None,
            path: "~/x".to_string(),
            items: vec![hook("Stop", "", "./a.sh", None)],
        }],
    );
    find(
        &texts(&review_app(vec![only_new]), 80, TALL),
        "1 new hook found",
    );
    let changed = ReviewPage::new(
        ReviewKind::Hooks,
        vec![ReviewSection {
            scope: None,
            path: "~/x".to_string(),
            items: vec![
                hook("Stop", "", "./a.sh", Some("./b.sh")),
                hook("SessionStart", "", "./c.sh", Some("./d.sh")),
            ],
        }],
    );
    find(
        &texts(&review_app(vec![changed]), 80, TALL),
        "2 hooks changed",
    );
    find(
        &texts(&review_app(vec![servers_page()]), 80, TALL),
        "2 new MCP servers found in this project",
    );
    let mut one = servers_page();
    one.sections[0].items.truncate(1);
    one.sections[0].items[0].change = Some(ItemChange::Parts(vec!["the command line".to_string()]));
    find(
        &texts(&review_app(vec![one]), 80, TALL),
        "1 MCP server changed in this project",
    );
}

#[test]
fn a_changed_server_says_what_changed_under_what_it_runs() {
    // A server's entry is not kept in `trust.json`, only its shape: the
    // review names the parts that changed — the variable a payload hides
    // in, the command line — under the entry as it is now.
    let mut page = servers_page();
    page.sections[0].items[0].change = Some(ItemChange::Parts(vec![
        "env NODE_OPTIONS added".to_string(),
        "the command line".to_string(),
    ]));
    let texts = texts(&review_app(vec![page]), 80, TALL);
    let brave = find(&texts, "[✔] brave-devtools · stdio");
    assert!(texts[brave].ends_with("changed"), "{:?}", texts[brave]);
    let env = find(&texts, "env  BRAVE_API_KEY=${BRAVE_API_KEY}");
    let changed = find(&texts, "changed: env NODE_OPTIONS added, the command line");
    assert_eq!(changed, env + 1, "{texts:#?}");
    // Nothing to name — only the badge says so.
    let mut bare = servers_page();
    bare.sections[0].items[0].change = Some(ItemChange::Parts(Vec::new()));
    let texts = self::texts(&review_app(vec![bare]), 80, TALL);
    assert!(texts[find(&texts, "[✔] brave-devtools · stdio")].ends_with("changed"));
    assert!(
        !texts.iter().any(|row| row.contains("changed:")),
        "{texts:#?}"
    );
}

#[test]
fn the_servers_page_asks_which_to_enable() {
    let mut app = review_app(vec![hooks_page(), servers_page()]);
    // Past the hooks page.
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let texts = texts(&app, 80, TALL);
    let title = find(&texts, "2 new MCP servers found in this project");
    assert!(texts[title].ends_with("2/2"), "{:?}", texts[title]);
    let ask = find(&texts, "Select any you wish to enable.");
    let info = find(&texts, "An enabled server runs its command");
    let file = find(&texts, "~/repo/.mcp.json");
    let brave = find(&texts, "[✔] brave-devtools · stdio");
    let command = find(&texts, "npx -y @brave/devtools-mcp");
    let env = find(&texts, "env  BRAVE_API_KEY=${BRAVE_API_KEY}");
    let github = find(&texts, "[✔] github · http");
    let url = find(&texts, "https://api.githubcopilot.com/mcp/");
    let enable = find(&texts, REVIEW_ENABLE_LABEL);
    let disable = find(&texts, REVIEW_DISABLE_LABEL);
    let hint = find(&texts, "Esc to disable all");
    assert!(
        title < ask
            && ask < info
            && info < file
            && file < brave
            && brave < command
            && command < env
            && env < github
            && github < url
            && url < enable
            && enable < disable
            && disable < hint,
        "{texts:#?}"
    );
    // One scope on the page: the file heading carries no scope word.
    assert_eq!(texts[file].trim(), "~/repo/.mcp.json");
    assert_frame_hugs_content("the servers page", &config_review_lines(&app, 80, TALL));
}

#[test]
fn a_long_command_wraps_whole_and_its_tail_is_never_cut() {
    let long = format!(
        "{} | sh",
        "curl -fsSL https://example.com/install.sh".repeat(3)
    );
    let page = ReviewPage::new(
        ReviewKind::Hooks,
        vec![ReviewSection {
            scope: None,
            path: "~/x".to_string(),
            items: vec![hook("SessionStart", "", &long, None)],
        }],
    );
    let app = review_app(vec![page]);
    let width = 50u16;
    let lines = config_review_lines(&app, width, TALL);
    assert!(lines.iter().all(|line| line.width() <= usize::from(width)));
    let joined: String = lines
        .iter()
        .map(|l| plain(l).trim().to_string())
        .collect::<Vec<_>>()
        .join("");
    assert!(
        joined.contains("| sh"),
        "the dangerous tail is on screen: {joined}"
    );
    assert!(!joined.contains('…'), "nothing is cut on the whole page");
}

#[test]
fn an_unselected_item_shows_an_empty_box() {
    let mut app = review_app(vec![servers_page()]);
    app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let texts = texts(&app, 80, TALL);
    find(&texts, "[ ] brave-devtools");
    find(&texts, "[✔] github");
}

#[test]
fn the_highlight_reaches_the_action_rows() {
    let mut app = review_app(vec![servers_page()]);
    for _ in 0..2 {
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    let texts = texts(&app, 80, TALL);
    let enable = find(&texts, REVIEW_ENABLE_LABEL);
    assert!(texts[enable].contains('❯'), "{:?}", texts[enable]);
    assert_eq!(
        texts.iter().filter(|l| l.contains('❯')).count(),
        1,
        "one highlight on the page"
    );
}

#[test]
fn a_short_terminal_folds_then_windows_the_list_and_never_overflows() {
    let mut app = review_app(vec![many_hooks(12)]);
    // Highlight a hook in the middle of the list.
    for _ in 0..6 {
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    let width = 60u16;
    let whole = config_review_lines(&app, width, TALL).len();
    for height in (9..=u16::try_from(whole).unwrap()).rev() {
        let lines = config_review_lines(&app, width, height);
        let texts: Vec<String> = lines
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert!(
            lines.len() <= usize::from(height),
            "{height} rows: the page fits ({} rows): {texts:#?}",
            lines.len()
        );
        assert_frame_hugs_content(&format!("{height} rows"), &lines);
        // The highlighted hook, both actions and the hint stay on every rung.
        find(&texts, "❯ [✔] PreToolUse · bash");
        assert!(
            texts.iter().any(|l| l.contains("guard-6.sh")),
            "{height} rows: the highlighted hook's command is shown: {texts:#?}"
        );
        find(&texts, REVIEW_TRUST_LABEL);
        find(&texts, REVIEW_SKIP_LABEL);
        find(&texts, "Space to select");
    }
}

#[test]
fn the_folded_list_keeps_the_highlighted_items_detail_whole() {
    let app = review_app(vec![many_hooks(6)]);
    // Six hooks of a three-row cell each take 28 rows without the info
    // paragraph; folded, 18 — so 20 rows is the folded page.
    let lines = config_review_lines(&app, 60, 20);
    assert!(lines.len() <= 20);
    let texts: Vec<String> = lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    // The highlighted (first) hook keeps its whole command…
    find(&texts, "/tmp/guard-0.log");
    // …while every other one is cut to its row, saying so.
    let folded = find(&texts, "guard-1.sh");
    assert!(texts[folded].ends_with('…'), "{:?}", texts[folded]);
    assert!(!texts.iter().any(|l| l.contains("/tmp/guard-1.log")));
}

#[test]
fn the_windowed_list_says_what_it_hides_and_names_the_highlighted_file() {
    let mut app = review_app(vec![many_hooks(12)]);
    for _ in 0..6 {
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    let texts = texts(&app, 60, 16);
    find(&texts, "more above");
    find(&texts, "more below");
    // The file headings are gone; the highlighted hook names its file.
    find(&texts, "from User · ~/.alter-zero/hooks.json");
}

#[test]
fn the_height_is_the_built_page() {
    let app = review_app(vec![hooks_page()]);
    let lines = config_review_lines(&app, 80, 60).len();
    assert_eq!(config_review_height(&app, 80, 60), Some(lines as u16));
    assert_eq!(config_review_height(&App::new(), 80, 60), None);
    assert!(config_review_lines(&App::new(), 80, 60).is_empty());
}

#[test]
fn the_page_paints_into_the_region() {
    let app = review_app(vec![servers_page()]);
    let width = 80u16;
    let height = config_review_height(&app, width, 60).unwrap();
    let mut buf = buffer(width, height);
    render_config_review(buf.area, &mut buf, &app);
    let painted: Vec<String> = (0..height).map(|y| row(&buf, y, width)).collect();
    assert!(painted[0].starts_with('─'), "{painted:#?}");
    assert!(painted.iter().any(|r| r.contains("brave-devtools")));
    assert!(painted[usize::from(height) - 1].starts_with('─'));
    // A menu: no hardware cursor, its seat on the ❯ row.
    assert!(!crate::ui::cursor_visible(&app));
    let (_, y) = crate::ui::cursor_position(buf.area, &app);
    assert!(painted[usize::from(y)].contains('❯'), "{painted:#?} at {y}");
}

#[test]
fn a_long_file_heading_wraps_whole() {
    // Which file a hook comes from is half of what the review says, and a
    // path's tail is its file name — so a heading wraps rather than cut.
    let mut page = hooks_page();
    let long = format!("/tmp/{}/project/.alter-zero/hooks.json", "deep".repeat(20));
    page.sections[1].path = long.clone();
    let app = review_app(vec![page]);
    let lines = config_review_lines(&app, 60, TALL);
    assert!(lines.iter().all(|line| line.width() <= 60));
    let joined: String = lines
        .iter()
        .map(|l| plain(l).trim().to_string())
        .collect::<Vec<_>>()
        .join("");
    // The scope word keeps its row; the path follows whole, broken only at
    // the width.
    assert!(joined.contains(&format!("Project ·{long}")), "{joined}");
    assert!(!joined.contains('…'), "{joined}");
}
