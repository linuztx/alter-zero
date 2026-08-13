//! The `/trust` review menu's rendering (`docs/project-config.md`).

use super::*;
use crate::trust::{TrustFileReview, TrustReview};
use crate::ui::theme::{TRUST_APPROVE_LABEL, TRUST_MENU_MAX_ITEMS, TRUST_REVOKE_LABEL};
use crate::ui::{render_trust_menu, trust_menu_height, trust_view_lines};

fn review() -> TrustReview {
    TrustReview {
        root: "~/repo".to_string(),
        trusted: true,
        files: vec![
            TrustFileReview {
                label: "Hooks".to_string(),
                path: "~/repo/.alter-zero/hooks.json".to_string(),
                items: vec!["PreToolUse (bash): ./guard.sh".to_string()],
                error: None,
                pending: true,
            },
            TrustFileReview {
                label: "MCP servers".to_string(),
                path: "~/repo/.mcp.json".to_string(),
                items: vec!["docs: npx -y docs-mcp".to_string()],
                error: None,
                pending: false,
            },
        ],
    }
}

fn trust_app() -> App {
    let mut app = App::new();
    app.open_trust_menu(review());
    app
}

/// The built body as trimmed plain rows.
fn texts(app: &App, width: u16) -> Vec<String> {
    trust_view_lines(app, width)
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

#[test]
fn the_review_names_the_root_the_files_and_what_would_run() {
    let app = trust_app();
    let texts = texts(&app, 100);
    let title = find(&texts, "~/repo");
    let hooks = find(&texts, "Hooks — ~/repo/.alter-zero/hooks.json");
    let hook_item = find(&texts, "PreToolUse (bash): ./guard.sh");
    let mcp = find(&texts, "MCP servers — ~/repo/.mcp.json");
    let server = find(&texts, "docs: npx -y docs-mcp");
    assert!(title < hooks && hooks < hook_item && hook_item < mcp && mcp < server);
    // The pending file is marked; the recorded one reads trusted.
    assert!(texts[hooks].contains("pending"), "{:?}", texts[hooks]);
    assert!(texts[mcp].contains("trusted"), "{:?}", texts[mcp]);
    // Both options render numbered, the first selected.
    let approve = find(&texts, &format!("1. {TRUST_APPROVE_LABEL}"));
    assert!(texts[approve].contains('❯'));
    find(&texts, &format!("2. {TRUST_REVOKE_LABEL}"));
}

#[test]
fn an_unparseable_file_shows_its_error_instead_of_items() {
    let mut app = App::new();
    app.open_trust_menu(TrustReview {
        root: "~/repo".to_string(),
        trusted: false,
        files: vec![TrustFileReview {
            label: "Hooks".to_string(),
            path: "~/repo/.alter-zero/hooks.json".to_string(),
            items: Vec::new(),
            error: Some("expected value at line 1".to_string()),
            pending: true,
        }],
    });
    let texts = texts(&app, 100);
    find(&texts, "expected value at line 1");
    // Nothing approvable — no option rows at all.
    assert!(!texts.iter().any(|l| l.contains(TRUST_APPROVE_LABEL)));
}

#[test]
fn the_empty_state_names_where_project_config_lives() {
    let mut app = App::new();
    app.open_trust_menu(TrustReview {
        root: "~/repo".to_string(),
        trusted: false,
        files: Vec::new(),
    });
    let texts = texts(&app, 100);
    find(&texts, "No project config found");
    find(&texts, "~/repo/.alter-zero/hooks.json");
    find(&texts, "~/repo/.alter-zero/mcp.json");
    find(&texts, "~/repo/.mcp.json");
}

#[test]
fn a_long_item_list_folds_into_a_more_row() {
    let mut app = App::new();
    app.open_trust_menu(TrustReview {
        root: "~/repo".to_string(),
        trusted: false,
        files: vec![TrustFileReview {
            label: "MCP servers".to_string(),
            path: "~/repo/.mcp.json".to_string(),
            items: (0..TRUST_MENU_MAX_ITEMS + 3)
                .map(|i| format!("server{i}: cmd{i}"))
                .collect(),
            error: None,
            pending: true,
        }],
    });
    let texts = texts(&app, 100);
    find(&texts, "server0: cmd0");
    find(&texts, "… +3 more");
    assert!(
        !texts
            .iter()
            .any(|l| l.contains(&format!("server{}: ", TRUST_MENU_MAX_ITEMS))),
        "rows past the cap stay folded"
    );
}

#[test]
fn the_height_is_the_built_lines_and_none_when_closed() {
    let app = trust_app();
    let lines = trust_view_lines(&app, 78).len() as u16;
    assert_eq!(trust_menu_height(&app, 78, 200), Some(lines));
    assert_eq!(
        trust_menu_height(&app, 78, 10),
        Some(10),
        "clamped to the terminal"
    );
    assert_eq!(trust_menu_height(&App::new(), 78, 200), None);
    assert!(trust_view_lines(&App::new(), 78).is_empty());
}

#[test]
fn render_paints_the_frame_and_the_cursor_parks_in_the_far_corner() {
    let app = trust_app();
    let height = trust_menu_height(&app, 78, 200).unwrap();
    let mut buf = buffer(78, height);
    render_trust_menu(buf.area, &mut buf, &app);
    assert!(row(&buf, 0, 78).starts_with('─'));
    let (x, y) = crate::ui::cursor_position(buf.area, &app);
    assert_eq!((x, y), (77, height - 1));
}
