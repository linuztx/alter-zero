//! The view-flow policy: which rows of a screen-tall framed view flow into
//! scrollback, and the signature the boundary tracks (`docs/view-flow.md`).

use super::*;
use crate::app::McpPage;
use crate::mcp::{McpServerConfig, McpServerSnapshot, McpServerStatus, McpToolInfo};
use crate::permission::{PermissionKind, PermissionRequest};
use crate::trust::{TrustFileReview, TrustReview};
use crate::ui::{mcp_view_lines, trust_view_lines, view_flow, view_flow_signature};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const NO_CAP: usize = usize::MAX;

/// An `/mcp` manager open on a tool page whose description wraps far past any
/// terminal used here.
fn tall_mcp_app() -> App {
    let mut app = App::new();
    app.open_mcp_menu(vec![McpServerSnapshot {
        name: "deepwiki".to_string(),
        scope: crate::mcp::McpScope::User,
        config_path: "~/.alter-zero/mcp.json".to_string(),
        config: McpServerConfig::Http {
            url: "https://deepwiki.test/mcp".to_string(),
            headers: Default::default(),
            sse_fallback: false,
        },
        status: McpServerStatus::Connected,
        auth: None,
        identity: None,
        tools: vec![McpToolInfo {
            name: "ask_question".to_string(),
            description: "describe ".repeat(300),
            input_schema: serde_json::json!({"type": "object"}),
        }],
    }]);
    app.mcp_menu.as_mut().unwrap().page = McpPage::Tool { tool: 0 };
    app
}

/// A `/trust` review with enough files to overflow a short terminal by a
/// wide margin, so the file listings sit well inside the flowed top.
fn tall_trust_app() -> App {
    let mut app = App::new();
    app.open_trust_menu(TrustReview {
        root: "~/repo".into(),
        trusted: false,
        files: (0..5)
            .map(|f| TrustFileReview {
                label: format!("Hooks {f}"),
                path: format!("~/repo/.alter-zero/hooks-{f}.json"),
                items: (0..8)
                    .map(|i| format!("PreToolUse (bash): ./guard-{f}-{i}.sh"))
                    .collect(),
                error: None,
                pending: true,
            })
            .collect(),
    });
    app
}

#[test]
fn no_flow_without_a_framed_view_or_when_the_page_fits() {
    let app = App::new();
    assert!(view_flow(&app, 80, 24, NO_CAP).is_none(), "no view open");
    assert!(view_flow_signature(&app, 80, 24, NO_CAP).is_none());

    // A short page fits the terminal whole — nothing flows.
    let mut app = App::new();
    app.open_mcp_menu(Vec::new());
    let lines = mcp_view_lines(&app, 80).len();
    assert!(lines < 40, "the empty list page is short: {lines}");
    assert!(view_flow(&app, 80, 40, NO_CAP).is_none());
}

#[test]
fn a_page_taller_than_the_terminal_flows_its_top() {
    // The rows the bottom anchor skips are exactly the rows that flow: the
    // page's top, in order, ending where the painted tail begins — so
    // scrollback + screen read as one contiguous page.
    let app = tall_mcp_app();
    let (width, height) = (60u16, 20u16);
    let lines = mcp_view_lines(&app, width);
    assert!(lines.len() > usize::from(height));
    let flow = view_flow(&app, width, height, NO_CAP).expect("the page overflows");
    let flowed: Vec<String> = flow.lines.iter().map(plain).collect();
    let expected: Vec<String> = lines[..lines.len() - usize::from(height)]
        .iter()
        .map(plain)
        .collect();
    assert_eq!(flowed, expected);
    assert_eq!(
        Some(flow.signature),
        view_flow_signature(&app, width, height, NO_CAP),
        "the signature helper matches the flow it stands for"
    );
}

#[test]
fn a_covering_modal_suppresses_the_flow() {
    // Eligibility mirrors the render precedence: an open permission prompt
    // paints INSTEAD of the menu, so nothing of the *menu* may flow while it
    // covers it. The prompt is flow-eligible itself now (docs/view-flow.md),
    // but this short bash prompt fits the terminal whole — so the flow the
    // menu had established must simply clean up, with nothing in its place.
    let mut app = tall_mcp_app();
    assert!(view_flow(&app, 60, 20, NO_CAP).is_some());
    app.open_permission(PermissionRequest {
        id: "perm_0".to_string(),
        kind: PermissionKind::Bash,
        target: "echo hi".to_string(),
        body: String::new(),
        detail: None,
        agent: None,
    });
    assert!(
        view_flow(&app, 60, 20, NO_CAP).is_none(),
        "the fitting prompt covers the menu and flows nothing"
    );
}

#[test]
fn the_signature_tracks_the_flowed_rows_only() {
    // Stepping the selection restyles rows near the page's END — inside the
    // painted tail, not the flowed top — so the signature holds and the
    // boundary never purge-rebuilds for a ↑/↓ (the in-place diff repaint
    // handles it). Changing the content that actually flowed re-signs.
    let mut app = tall_trust_app();
    let (width, height) = (60u16, 20u16);
    assert!(trust_view_lines(&app, width).len() > usize::from(height));
    let before = view_flow_signature(&app, width, height, NO_CAP).expect("overflows");
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let after = view_flow_signature(&app, width, height, NO_CAP).expect("still overflows");
    assert_eq!(before, after, "a selection step keeps the flowed top");

    // A different page top — the first file's first verbatim item, a row
    // that flowed — is a different flow.
    let mut other = tall_trust_app();
    other.trust_menu.as_mut().unwrap().review.files[0].items[0] =
        "PostToolUse (bash): ./fmt.sh".to_string();
    let changed = view_flow_signature(&other, width, height, NO_CAP).expect("overflows");
    assert_ne!(before, changed, "changed flowed content re-signs");

    // A narrower terminal wraps differently — a different flow too.
    let narrower = view_flow_signature(&app, width - 10, height, NO_CAP).expect("overflows");
    assert_ne!(after, narrower);
}

#[test]
fn the_flow_keeps_its_newest_rows_under_the_cap() {
    // A pathological page can't turn one navigation into an unbounded write:
    // the flow keeps the rows nearest the painted tail (the newest — the
    // rebuild-cap rule), dropping the extreme top.
    let app = tall_mcp_app();
    let (width, height) = (60u16, 20u16);
    let lines = mcp_view_lines(&app, width);
    let full = lines.len() - usize::from(height);
    let cap = 5usize;
    assert!(full > cap);
    let flow = view_flow(&app, width, height, cap).expect("overflows");
    assert_eq!(flow.lines.len(), cap);
    let kept: Vec<String> = flow.lines.iter().map(plain).collect();
    let expected: Vec<String> = lines[full - cap..full].iter().map(plain).collect();
    assert_eq!(kept, expected, "the rows nearest the region are kept");
}

#[test]
fn a_picker_taller_than_the_terminal_flows_like_the_menus() {
    // The windowed pickers are framed bodies too (docs/view-flow.md): on a
    // terminal shorter than their page the skipped top flows, and their spot
    // in the precedence chain sits ahead of the browsing menus — a /settings
    // menu open over an /mcp one is the painted view, so it is the one that
    // flows.
    let mut app = App::new();
    app.open_settings();
    let width = 78u16;
    let page = crate::ui::settings_height(&app, width, 200).expect("open");
    let height = page - 3;
    let flow = view_flow(&app, width, height, NO_CAP).expect("the page overflows");
    assert_eq!(flow.lines.len(), 3, "exactly the skipped top rows flow");
    let top: Vec<String> = flow.lines.iter().map(plain).collect();
    assert!(
        top[0].starts_with("──"),
        "the flow starts at the page top (the opening rule): {top:?}"
    );
    assert!(
        top.iter().any(|r| r.contains('❯')),
        "the search line is in the flowed top, re-flowed per keystroke: {top:?}"
    );

    // Typing re-signs the flow (the search line lives in it).
    let before = view_flow_signature(&app, width, height, NO_CAP).expect("overflows");
    let mut typed = App::new();
    typed.open_settings();
    typed.on_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    let after = view_flow_signature(&typed, width, height, NO_CAP).expect("still overflows");
    assert_ne!(before, after, "the typed query re-signs the flow");

    // Precedence: the picker wins over a simultaneously-open menu.
    let mut both = App::new();
    both.open_mcp_menu(Vec::new());
    both.open_settings();
    let flow = view_flow(&both, width, height, NO_CAP).expect("the settings page flows");
    assert!(
        flow.lines.iter().map(plain).any(|r| r.contains('❯')),
        "the flowed rows are the settings page's, not the mcp list's"
    );
}

#[test]
fn a_screen_tall_permission_prompt_flows_its_top() {
    // The permission prompt is a framed view like the rest now
    // (docs/view-flow.md): a page taller than the terminal flows its static
    // top — the opening rule, the title, the body's first rows — while the
    // tail block stays painted. Stepping the selection restyles option rows
    // near the page's end, so the flowed top holds its signature and ↑/↓
    // never purge-rebuilds.
    let body: String = (1..=200).map(|n| format!("{n:>3} line {n}\n")).collect();
    let mut app = App::new();
    app.open_permission(PermissionRequest {
        id: "perm_flow".to_string(),
        kind: PermissionKind::Write,
        target: "big.py".to_string(),
        body: body.trim_end().to_string(),
        detail: None,
        agent: None,
    });
    let (width, height) = (70u16, 24u16);
    let flow = view_flow(&app, width, height, NO_CAP).expect("the prompt overflows");
    let top: Vec<String> = flow.lines.iter().map(plain).collect();
    assert!(
        top[0].starts_with('─'),
        "the flow opens at the rule: {top:?}"
    );
    assert!(
        top.iter().any(|r| r.contains("Create file")),
        "the title flowed: {top:?}"
    );
    let before = view_flow_signature(&app, width, height, NO_CAP).expect("overflows");
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let after = view_flow_signature(&app, width, height, NO_CAP).expect("still overflows");
    assert_eq!(before, after, "a selection step keeps the flowed top");

    // A prompt that fits keeps the ordinary modal geometry — no flow.
    let mut small = App::new();
    small.open_permission(PermissionRequest {
        id: "perm_small".to_string(),
        kind: PermissionKind::Bash,
        target: "echo hi".to_string(),
        body: String::new(),
        detail: None,
        agent: None,
    });
    assert!(view_flow(&small, width, height, NO_CAP).is_none());
}

/// An ask modal whose one question carries enough described options to
/// overflow every terminal height used here.
fn tall_ask_app() -> App {
    let mut app = App::new();
    app.open_ask(crate::ask::AskRequest {
        id: "ask_flow".to_string(),
        questions: vec![crate::ask::AskQuestion {
            question: "Which of these would you like to pick from this long list?".to_string(),
            header: "Long list".to_string(),
            options: (0..8)
                .map(|i| crate::ask::AskOption {
                    label: format!("Option number {i}"),
                    description: "A description that explains this option".to_string(),
                    preview: None,
                })
                .collect(),
            multi_select: false,
        }],
    });
    app
}

#[test]
fn a_screen_tall_ask_modal_flows_its_top() {
    // The ask modal is the permission prompt's sibling here too
    // (docs/view-flow.md): a page taller than the terminal used to drop its
    // top rows — the chip strip, the question, the first options — into no
    // buffer at all (the reported "small terminal hides the texts"). Now the
    // skipped top flows into real scrollback, where the terminal's own
    // scrolling reads it, and the painted tail keeps the interactive rows.
    let app = tall_ask_app();
    let (width, height) = (60u16, 12u16);
    let lines = crate::ui::ask_lines(&app, width);
    assert!(lines.len() > usize::from(height), "the page overflows");
    let flow = view_flow(&app, width, height, NO_CAP).expect("the page overflows");
    let flowed: Vec<String> = flow.lines.iter().map(plain).collect();
    let expected: Vec<String> = lines[..lines.len() - usize::from(height)]
        .iter()
        .map(plain)
        .collect();
    assert_eq!(flowed, expected, "the skipped top is what flows");
    assert!(
        flowed[0].starts_with('─'),
        "the flow opens at the page's top rule: {flowed:?}"
    );
    assert!(
        flowed.iter().any(|r| r.contains("Long list")),
        "the chip strip rides the flow: {flowed:?}"
    );
    assert!(
        flowed.iter().any(|r| r.contains("Which of these")),
        "the question rides the flow: {flowed:?}"
    );
    assert_eq!(
        Some(flow.signature),
        view_flow_signature(&app, width, height, NO_CAP),
        "the signature helper matches the flow it stands for"
    );

    // Stepping between two rows inside the painted tail holds the flowed
    // top's signature — no purge rebuild for a tail-only ↑/↓ (the
    // permission prompt's rule). ↑ wraps to Chat, ↑ again to the Other row;
    // both sit in the tail, and the first ↑ took the `❯` out of the flow.
    let mut app = tall_ask_app();
    app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    let a = view_flow_signature(&app, width, height, NO_CAP).expect("overflows");
    app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    let b = view_flow_signature(&app, width, height, NO_CAP).expect("still overflows");
    assert_eq!(a, b, "a tail-only step keeps the flowed top");

    // A modal that fits flows nothing — the region alone shows it whole.
    let mut small = App::new();
    small.open_ask(crate::ask::AskRequest {
        id: "ask_small".to_string(),
        questions: vec![crate::ask::AskQuestion {
            question: "Coffee?".to_string(),
            header: "Coffee".to_string(),
            options: vec![crate::ask::AskOption {
                label: "Yes".to_string(),
                description: String::new(),
                preview: None,
            }],
            multi_select: false,
        }],
    });
    assert!(view_flow(&small, width, 40, NO_CAP).is_none());
}
