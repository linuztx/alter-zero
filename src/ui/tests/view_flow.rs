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
        agent_id: None,
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
        agent_id: None,
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
        agent_id: None,
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

/// The ↓ background manager open on one shell's **details** page — the
/// live-tailing page whose top used to be dropped into no buffer at all on a
/// short terminal (`docs/background.md`).
fn tall_background_app() -> App {
    let mut app = App::new();
    app.bg_started(
        "bash_1",
        "for i in $(seq 1 100); do echo $i; sleep 1; done",
        None,
        true,
        None,
    );
    for i in 1..=20 {
        app.bg_output("bash_1", &format!("{i}\n"));
    }
    app.set_background_runtime("bash_1", std::time::Duration::from_secs(64));
    app.open_background_view();
    app.background_view = Some(BackgroundView::Details {
        id: "bash_1".to_string(),
    });
    app
}

#[test]
fn the_background_managers_details_page_flows_its_top() {
    // The reported bug: a details page taller than the terminal bottom-
    // anchored, and the rows it skipped — the top rule, the `Shell details`
    // title, the status/runtime fields — went into NO buffer at all, so
    // scrolling the terminal up showed the conversation running straight
    // into a headless box. They flow into real scrollback now, like every
    // other framed view (`docs/view-flow.md`).
    let app = tall_background_app();
    let (width, height) = (60u16, 20u16);
    let lines = crate::ui::background_view_lines(&app, width);
    assert!(lines.len() > usize::from(height), "the page overflows");
    let flow = view_flow(&app, width, height, NO_CAP).expect("the details page overflows");
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
        flowed.iter().any(|r| r.contains("Shell details")),
        "the title rides the flow: {flowed:?}"
    );
    assert_eq!(
        Some(flow.signature),
        view_flow_signature(&app, width, height, NO_CAP),
        "the signature helper matches the flow it stands for"
    );
}

#[test]
fn the_details_pages_flowed_top_freezes_while_the_shell_ticks() {
    // The details page live-tails: its runtime advances every second and its
    // output box grows, at the 32ms animation cadence the open band keeps
    // running (`App::wants_animation_frames`). Signing those rows would
    // purge-rebuild the whole screen thirty times a second, so this one page
    // signs the **shell it describes** instead: the rows committed above the
    // region freeze there, which is what scrollback holds anyway. The page
    // is 24 rows, so an 18-row terminal flows its top six: the rule, the
    // title, the gap, the status, the runtime and the command.
    let mut app = tall_background_app();
    let (width, height) = (60u16, 18u16);
    let flowed: Vec<String> = view_flow(&app, width, height, NO_CAP)
        .expect("overflows")
        .lines
        .iter()
        .map(plain)
        .collect();
    assert!(
        flowed.iter().any(|r| r.contains("Runtime:")),
        "the ticking runtime row is inside the flow at this height: {flowed:?}"
    );
    let before = view_flow_signature(&app, width, height, NO_CAP).expect("overflows");
    app.set_background_runtime("bash_1", std::time::Duration::from_secs(65));
    app.bg_output("bash_1", "21\n");
    let after = view_flow_signature(&app, width, height, NO_CAP).expect("still overflows");
    assert_eq!(before, after, "a tick must not churn a purge rebuild");
}

#[test]
fn a_real_move_off_the_details_page_re_signs_the_flow() {
    // Freezing is only for the tick: everything that actually moves the page
    // re-signs, and the boundary's purge rebuild re-flows it — a different
    // shell, a walk back to the list, a resize.
    let (width, height) = (60u16, 20u16);
    let mut app = tall_background_app();
    let details = view_flow_signature(&app, width, height, NO_CAP).expect("overflows");

    let mut other = tall_background_app();
    other.bg_started("bash_2", "ping x.com", None, true, None);
    other.background_view = Some(BackgroundView::Details {
        id: "bash_2".to_string(),
    });
    assert_ne!(
        Some(details),
        view_flow_signature(&other, width, height, NO_CAP),
        "a different shell's page is a different flow"
    );

    assert_ne!(
        Some(details),
        view_flow_signature(&app, width, height - 2, NO_CAP),
        "a resize re-signs: two more rows flow"
    );

    app.background_view = Some(BackgroundView::List { selected: 0 });
    assert_ne!(
        Some(details),
        view_flow_signature(&app, width, height, NO_CAP),
        "walking back to the list re-signs"
    );
}

#[test]
fn the_managers_list_page_signs_its_rows_like_every_other_menu() {
    // Only the details page ticks. The list changes on a keystroke, so it
    // keeps the ordinary rule — a selection move inside the flowed top
    // re-signs, and the rebuild re-flows the moved `❯`.
    let mut app = App::new();
    for i in 0..3 {
        app.bg_started(
            &format!("bash_{i}"),
            &format!("sleep {i}"),
            None,
            true,
            None,
        );
    }
    app.open_background_view();
    // The page is 10 rows, so a 4-row terminal flows its top six — the
    // header and the first two shell rows.
    let (width, height) = (60u16, 4u16);
    let flowed: Vec<String> = view_flow(&app, width, height, NO_CAP)
        .expect("the list overflows a 4-row terminal")
        .lines
        .iter()
        .map(plain)
        .collect();
    assert!(
        flowed.iter().any(|r| r.contains("❯ sleep 0")),
        "the selected row is inside the flow at this height: {flowed:?}"
    );
    let before = view_flow_signature(&app, width, height, NO_CAP).expect("overflows");
    app.background_view = Some(BackgroundView::List { selected: 1 });
    let after = view_flow_signature(&app, width, height, NO_CAP).expect("still overflows");
    assert_ne!(before, after, "a selection move re-signs the list's flow");

    // …and so does a Details view whose shell has gone: it renders the LIST
    // (`background_view_lines`' defensive fallback), a page that moves — so
    // freezing it on the dead id would strand the roster mid-exit.
    app.background_view = Some(BackgroundView::Details {
        id: "ghost".to_string(),
    });
    let ghost = view_flow_signature(&app, width, height, NO_CAP).expect("the fallback list flows");
    app.bg_exited("bash_2", Some(0), false);
    assert_ne!(
        Some(ghost),
        view_flow_signature(&app, width, height, NO_CAP),
        "the fallback list re-signs as its rows change"
    );
}

/// A turn with a `bash` call streaming its output — the strip content the
/// user reported losing on a short terminal (`docs/strip-flow.md`).
fn streaming_bash_app() -> App {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "ping google.com -c 100 -4", None);
    for i in 1..=20 {
        app.push_tool_output(&format!("64 bytes from 1e100.net: icmp_seq={i}\n"));
    }
    app.set_status_times(std::time::Duration::from_secs(64), None);
    app.set_command_elapsed(Some(std::time::Duration::from_secs(64)));
    app
}

#[test]
fn a_squeezed_strip_flows_the_rows_it_cannot_paint() {
    // The reported bug: as the terminal shrinks, `preview_lines` trimmed the
    // running cell from the END — the `+N lines` footer and the ctrl+b hint
    // first, then the output rows, then the header — into no buffer at all.
    // They flow into real scrollback now, so the cell **scrolls**: its top
    // freezes above the region and the newest rows keep the screen.
    let app = streaming_bash_app();
    let (width, height) = (60u16, 11u16);
    let want = crate::ui::preview_rows(&app, width);
    let painted = crate::ui::fitted_preview_rows(&app, width, height);
    assert!(
        painted < want,
        "the fixture must squeeze the preview ({painted} of {want} rows)"
    );
    let flow = view_flow(&app, width, height, NO_CAP).expect("the strip overflows");
    let flowed: Vec<String> = flow.lines.iter().map(plain).collect();
    assert_eq!(
        flowed.len(),
        usize::from(want - painted),
        "exactly the rows the paint cannot show: {flowed:?}"
    );
    assert!(
        flowed[0].contains("Bash(ping google.com -c 100 -4)"),
        "the cell's header is what freezes above the region: {flowed:?}"
    );
}

#[test]
fn the_strips_flow_freezes_while_the_command_streams() {
    // The strip ticks at the turn's 32ms animation cadence — the output
    // tail, the elapsed, the breathing bullet. Signing its rows would
    // purge-rebuild the screen thirty times a second, so it is signed on
    // what the strip is *of* and the flowed rows freeze (the ↓ manager's
    // rule, `docs/view-flow.md`).
    let mut app = streaming_bash_app();
    let (width, height) = (60u16, 11u16);
    let before = view_flow_signature(&app, width, height, NO_CAP).expect("overflows");
    app.push_tool_output("64 bytes from 1e100.net: icmp_seq=21\n");
    app.set_status_times(std::time::Duration::from_secs(65), None);
    let after = view_flow_signature(&app, width, height, NO_CAP).expect("still overflows");
    assert_eq!(before, after, "a streamed line must not churn a rebuild");
}

#[test]
fn a_new_call_and_a_resize_re_sign_the_strips_flow() {
    let (width, height) = (60u16, 11u16);
    let app = streaming_bash_app();
    let first = view_flow_signature(&app, width, height, NO_CAP).expect("overflows");

    let mut next = streaming_bash_app();
    next.end_tool("done", true);
    next.start_tool("Bash", "ping example.com -c 100 -4", None);
    for i in 1..=20 {
        next.push_tool_output(&format!("64 bytes from example: icmp_seq={i}\n"));
    }
    assert_ne!(
        Some(first),
        view_flow_signature(&next, width, height, NO_CAP),
        "a different call is a different flow"
    );
    assert_ne!(
        Some(first),
        view_flow_signature(&app, width, height - 2, NO_CAP),
        "a resize re-signs: two more rows flow"
    );
}

#[test]
fn a_strip_that_fits_flows_nothing() {
    // The common case is untouched: a terminal with room for the whole cell
    // paints it whole and commits nothing above the region.
    let app = streaming_bash_app();
    assert!(
        view_flow(&app, 60, 40, NO_CAP).is_none(),
        "nothing flows while the strip fits"
    );
    // …and neither does an idle session with no strip at all.
    assert!(view_flow(&App::new(), 60, 6, NO_CAP).is_none());
}

#[test]
fn a_starved_strip_flows_its_status_line_too() {
    // Past the preview, `live_layout` starves the strip itself — on a
    // terminal this short the status line is dropped as well. It freezes
    // above the region rather than vanishing ("freeze the status indicator
    // if it's not visible in the active window").
    let app = streaming_bash_app();
    let (width, height) = (60u16, 4u16);
    let flow = view_flow(&app, width, height, NO_CAP).expect("the strip overflows");
    let flowed: Vec<String> = flow.lines.iter().map(plain).collect();
    assert!(
        flowed.iter().any(|r| r.contains("esc to interrupt")),
        "the status line rides the flow: {flowed:?}"
    );
    assert!(
        flowed[0].contains("Bash(ping google.com -c 100 -4)"),
        "the flow still opens at the strip's own top: {flowed:?}"
    );
}

#[test]
fn a_streaming_replys_frontier_never_flows() {
    // A reply's preview is its **uncommitted** frontier: `StreamRender`
    // commits every completed line to scrollback already, so its dropped
    // rows are not lost — and flowing a frontier that grows per chunk would
    // re-sign the flow on every chunk. Only live cells, which reach no
    // buffer until they resolve, flow.
    let mut app = App::new();
    app.begin_stream();
    for i in 0..40 {
        app.push_chunk(&format!("word{i} "));
    }
    app.set_stream_preview_rows(8);
    assert!(
        view_flow(&app, 60, 10, NO_CAP).is_none(),
        "the frontier is already committed row by row — nothing to freeze"
    );
}

/// A turn generating a 30-line `write`, its content still open.
fn streaming_write_app(lines: usize, closed: bool) -> App {
    let content: String = (1..=lines).map(|i| format!("row {i}\n")).collect();
    let encoded = serde_json::to_string(&content).expect("a string encodes");
    let content = if closed {
        format!("{encoded}}}")
    } else {
        encoded[..encoded.len() - 1].to_string()
    };
    let mut app = App::new();
    app.begin_stream();
    app.push_tool_call_fragment(&crate::stream::ToolCallFragment {
        index: 0,
        name: "write".to_string(),
        arguments: format!(r#"{{"path":"a.py","content":{content}"#),
    });
    app
}

#[test]
fn a_streaming_write_is_a_live_cell_the_strip_flows_rather_than_trims() {
    let app = streaming_write_app(30, false);
    let (width, height) = (60u16, 8u16);
    assert_eq!(
        crate::ui::live::strip_content_preview_rows(&app, width, height),
        crate::ui::preview_rows(&app, width),
        "the content is built at its full ask"
    );
    let flow = view_flow(&app, width, height, NO_CAP).expect("the strip overflows");
    assert!(
        plain(&flow.lines[0]).contains("Write(a.py)"),
        "the header freezes above the region: {:?}",
        flow.lines.iter().map(plain).collect::<Vec<_>>()
    );
}

#[test]
fn a_streaming_writes_flow_holds_while_lines_arrive_and_re_signs_when_it_finishes() {
    let (width, height) = (60u16, 8u16);
    let before = view_flow_signature(&streaming_write_app(30, false), width, height, NO_CAP)
        .expect("overflows");
    let after = view_flow_signature(&streaming_write_app(31, false), width, height, NO_CAP)
        .expect("still overflows");
    assert_eq!(before, after, "a streamed line must not churn a rebuild");
    let finished = view_flow_signature(&streaming_write_app(31, true), width, height, NO_CAP)
        .expect("still overflows");
    assert_ne!(
        after, finished,
        "the flip from tail to head replaces every row, so it re-signs"
    );
}

// ===== every framed page sits flush against its rules =====

/// One key press — how the per-view tests walk a page through its levels.
fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

/// A bare app with a permission prompt of `kind` open.
fn prompt_app(kind: PermissionKind, target: &str, body: &str, detail: Option<&str>) -> App {
    let mut app = App::new();
    app.open_permission(PermissionRequest {
        id: "perm_frame".to_string(),
        kind,
        target: target.to_string(),
        body: body.to_string(),
        detail: detail.map(str::to_string),
        agent: None,
        agent_id: None,
    });
    app
}

/// A bare app with the ask modal open on `questions`.
fn ask_app(questions: Vec<crate::ask::AskQuestion>) -> App {
    let mut app = App::new();
    app.open_ask(crate::ask::AskRequest {
        id: "ask_frame".to_string(),
        questions,
    });
    app
}

/// A one-question ask, its options previewed side by side when `preview`.
fn ask_question(header: &str, preview: bool) -> crate::ask::AskQuestion {
    crate::ask::AskQuestion {
        question: format!("Which {header} do you prefer?"),
        header: header.to_string(),
        options: ["One", "Two"]
            .iter()
            .map(|label| crate::ask::AskOption {
                label: (*label).to_string(),
                description: format!("The {label} option"),
                preview: preview.then(|| format!("fn {label}() {{}}")),
            })
            .collect(),
        multi_select: false,
    }
}

/// An `/mcp` server with one tool, for the manager's four pages.
fn frame_server() -> McpServerSnapshot {
    McpServerSnapshot {
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
            description: "Ask about a repository.".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }],
    }
}

/// Every framed inline view built at `width`, one entry per page shape: each
/// picker with a selection and with a search that matched nothing, every
/// `/login` step, each menu's levels and pages, both modals' kinds and
/// pages. The views `flow_page` lists — everything that replaces the
/// composer — so a new framed page belongs here too.
fn framed_pages(width: u16) -> Vec<(&'static str, Vec<Line<'static>>)> {
    use crate::ui::model_view::model_view_lines;
    let mut pages: Vec<(&'static str, Vec<Line<'static>>)> = Vec::new();

    // /model — a real selection, every placeholder, a failed provider.
    let model = |picker: &ModelPicker| model_view_lines(picker, width);
    pages.push(("/model", model(&model_picker(three_models(), 0, "x"))));
    let mut no_match = model_picker(three_models(), 0, "x");
    no_match.query = "zzzz".into();
    pages.push(("/model no match", model(&no_match)));
    let loading = ModelPicker {
        active_id: "x".into(),
        ..ModelPicker::default()
    };
    pages.push(("/model loading", model(&loading)));
    let failed = ModelPicker {
        status: ModelLoad::Error("401 bad key".into()),
        active_id: "x".into(),
        ..ModelPicker::default()
    };
    pages.push(("/model error", model(&failed)));
    let login_first = ModelPicker {
        status: ModelLoad::NeedsLogin,
        active_id: "x".into(),
        ..ModelPicker::default()
    };
    pages.push(("/model needs login", model(&login_first)));
    let mut partial = model_picker(three_models(), 0, "x");
    partial.errors = vec![ModelFetchError {
        provider: "Together AI".into(),
        message: "503 upstream unavailable".into(),
    }];
    pages.push(("/model failed provider", model(&partial)));

    // /login — the three lists, the method choice, the sign-in page before
    // and after its code, the key and host fields.
    let login = |app: &App| {
        crate::ui::login_view::key_onboarding_lines(app.key_onboarding.as_ref().unwrap(), width)
    };
    pages.push(("/login method", login(&login_app())));
    pages.push(("/login subscription", login(&login_app_subscription())));
    pages.push(("/login provider", login(&login_app_provider())));
    pages.push(("/login sign-in method", login(&login_app_signin_method())));
    pages.push(("/login device", login(&login_app_device())));
    let mut starting = login_app_subscription();
    {
        let onboarding = starting.key_onboarding.as_mut().unwrap();
        onboarding.step = KeyStep::Device;
        onboarding.device = Some(crate::app::DeviceLogin {
            provider_id: "github_copilot".into(),
            provider_name: "GitHub Copilot".into(),
            ..Default::default()
        });
    }
    pages.push(("/login device starting", login(&starting)));
    let mut key = login_app_provider();
    {
        let onboarding = key.key_onboarding.as_mut().unwrap();
        onboarding.step = KeyStep::Key;
        onboarding.chosen = Some(0);
    }
    pages.push(("/login key", login(&key)));

    // /settings — a selection, then a search that matched nothing.
    let mut settings = App::new();
    settings.open_settings();
    pages.push((
        "/settings",
        crate::ui::settings_view::settings_view_lines(&settings, width),
    ));
    settings.settings_picker.as_mut().unwrap().query = "zzzz".into();
    pages.push((
        "/settings no match",
        crate::ui::settings_view::settings_view_lines(&settings, width),
    ));

    // /skills — a catalog, a search that matched nothing, an empty catalog.
    let skill = crate::skills::SkillMetadata {
        name: "dataviz".to_string(),
        description: "Charts and dashboards".to_string(),
        dir: std::path::PathBuf::from("/skills/dataviz"),
        path: std::path::PathBuf::from("/skills/dataviz/SKILL.md"),
    };
    let roots = vec!["~/.claude/skills".to_string()];
    let mut skills = App::new();
    skills.open_skills_menu(vec![skill], Default::default(), true, roots.clone());
    pages.push((
        "/skills",
        crate::ui::skills_view::skills_view_lines(&skills, width),
    ));
    skills.skills_menu.as_mut().unwrap().query = "zzzz".into();
    pages.push((
        "/skills no match",
        crate::ui::skills_view::skills_view_lines(&skills, width),
    ));
    let mut no_skills = App::new();
    no_skills.open_skills_menu(Vec::new(), Default::default(), false, roots);
    pages.push((
        "/skills none installed",
        crate::ui::skills_view::skills_view_lines(&no_skills, width),
    ));

    // /mascot, /spinner, /theme — a selection, then no match.
    let mut mascot = with_session();
    mascot.open_mascot_picker();
    pages.push((
        "/mascot",
        crate::ui::mascot_view::mascot_view_lines(&mascot, width),
    ));
    mascot.mascot_picker.as_mut().unwrap().query = "zzzz".into();
    pages.push((
        "/mascot no match",
        crate::ui::mascot_view::mascot_view_lines(&mascot, width),
    ));
    let mut spinner = with_session();
    spinner.open_spinner_picker();
    pages.push((
        "/spinner",
        crate::ui::spinner_view::spinner_view_lines(&spinner, width),
    ));
    spinner.spinner_picker.as_mut().unwrap().query = "zzzz".into();
    pages.push((
        "/spinner no match",
        crate::ui::spinner_view::spinner_view_lines(&spinner, width),
    ));
    let mut theme = with_session();
    theme.open_theme_picker();
    pages.push((
        "/theme",
        crate::ui::theme_view::theme_view_lines(&theme, width),
    ));
    theme.theme_picker.as_mut().unwrap().query = "zzzz".into();
    pages.push((
        "/theme no match",
        crate::ui::theme_view::theme_view_lines(&theme, width),
    ));

    // /hooks — the four levels down one configured row, and an event with
    // nothing configured.
    let file = crate::hooks::HooksFile::parse(
        r#"{"hooks":{"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command","command":"./guard.sh"}]}]}}"#,
    )
    .expect("the fixture parses");
    let overview = crate::hooks::HooksOverview::from_file(&file);
    let mut hooks = App::new();
    hooks.open_hooks_menu(
        overview.clone(),
        Some("~/.alter-zero/hooks.json".into()),
        true,
    );
    pages.push(("/hooks events", crate::ui::hooks_view_lines(&hooks, width)));
    press(&mut hooks, KeyCode::Enter);
    pages.push((
        "/hooks matchers",
        crate::ui::hooks_view_lines(&hooks, width),
    ));
    press(&mut hooks, KeyCode::Enter);
    pages.push(("/hooks hooks", crate::ui::hooks_view_lines(&hooks, width)));
    press(&mut hooks, KeyCode::Enter);
    pages.push(("/hooks detail", crate::ui::hooks_view_lines(&hooks, width)));
    let mut empty_event = App::new();
    empty_event.open_hooks_menu(overview, None, true);
    press(&mut empty_event, KeyCode::Down); // PostToolUse — nothing configured
    press(&mut empty_event, KeyCode::Enter);
    pages.push((
        "/hooks empty event",
        crate::ui::hooks_view_lines(&empty_event, width),
    ));

    // /mcp — the list (empty and not), and each page a server opens.
    let mut no_servers = App::new();
    no_servers.open_mcp_menu(Vec::new());
    pages.push(("/mcp empty", mcp_view_lines(&no_servers, width)));
    let mut mcp = App::new();
    mcp.open_mcp_menu(vec![frame_server()]);
    pages.push(("/mcp list", mcp_view_lines(&mcp, width)));
    for (name, page) in [
        ("/mcp server", McpPage::Server),
        ("/mcp tools", McpPage::Tools),
        ("/mcp tool", McpPage::Tool { tool: 0 }),
        ("/mcp auth", McpPage::Auth),
    ] {
        mcp.mcp_menu.as_mut().unwrap().page = page;
        pages.push((name, mcp_view_lines(&mcp, width)));
    }

    // /trust — a review with files, and one with none.
    let mut trust = App::new();
    trust.open_trust_menu(TrustReview {
        root: "~/repo".into(),
        trusted: false,
        files: vec![TrustFileReview {
            label: "Hooks".into(),
            path: "~/repo/.alter-zero/hooks.json".into(),
            items: vec!["PreToolUse (bash): ./guard.sh".into()],
            error: None,
            pending: true,
        }],
    });
    pages.push(("/trust", trust_view_lines(&trust, width)));
    let mut no_files = App::new();
    no_files.open_trust_menu(TrustReview {
        root: "~/repo".into(),
        trusted: false,
        files: Vec::new(),
    });
    pages.push(("/trust no files", trust_view_lines(&no_files, width)));

    // /donate and /export.
    let mut donate = with_session();
    donate.open_donate_picker();
    pages.push(("/donate", crate::ui::donate_view_lines(&donate, width)));
    let mut export = with_session();
    export.record_user_message("hello");
    export.open_export_picker();
    pages.push(("/export", crate::ui::export_view_lines(&export, width)));

    // /secrets — the list (empty and not), and the form.
    let mut secrets = with_session();
    secrets.open_secrets_page();
    pages.push((
        "/secrets empty",
        crate::ui::secrets_view::secrets_view_lines(&secrets, width),
    ));
    press(&mut secrets, KeyCode::Enter); // the add row → the form
    pages.push((
        "/secrets form",
        crate::ui::secrets_view::secrets_view_lines(&secrets, width),
    ));
    let mut listed = with_session();
    listed.set_secret_metas(vec![crate::secrets::SecretMeta {
        name: "GITHUB_TOKEN".into(),
        context: "the GitHub API".into(),
    }]);
    listed.open_secrets_page();
    pages.push((
        "/secrets list",
        crate::ui::secrets_view::secrets_view_lines(&listed, width),
    ));

    // The ↓ background manager — the list, a shell's details, the empty list.
    let mut background = App::new();
    background.bg_started("bash_1", "ping x.com", None, true, None);
    background.open_background_view();
    pages.push((
        "↓ manager list",
        crate::ui::background_view_lines(&background, width),
    ));
    background.background_view = Some(BackgroundView::Details {
        id: "bash_1".to_string(),
    });
    pages.push((
        "↓ manager details",
        crate::ui::background_view_lines(&background, width),
    ));
    let mut no_shells = App::new();
    no_shells.bg_started("bash_1", "ping x.com", None, true, None);
    no_shells.bg_exited("bash_1", Some(0), false);
    no_shells.open_background_view();
    pages.push((
        "↓ manager empty",
        crate::ui::background_view_lines(&no_shells, width),
    ));

    // The permission prompt — every kind, and Tab's amend field.
    let prompt = |app: &App| crate::ui::permission_lines(app, width, 200);
    pages.push((
        "permission bash",
        prompt(&prompt_app(PermissionKind::Bash, "echo hi", "", None)),
    ));
    pages.push((
        "permission write",
        prompt(&prompt_app(
            PermissionKind::Write,
            "hello.py",
            "1 print(\"hi\")",
            None,
        )),
    ));
    pages.push((
        "permission edit",
        prompt(&prompt_app(
            PermissionKind::Edit,
            "hello.py",
            "1 -old = 1\n1 +new = 2",
            None,
        )),
    ));
    pages.push((
        "permission mcp",
        prompt(&prompt_app(
            PermissionKind::Mcp,
            "mcp__deepwiki__ask_question",
            r#"question: "why""#,
            Some("Ask about a repository."),
        )),
    ));
    pages.push((
        "permission session",
        prompt(&prompt_app(
            PermissionKind::Session,
            "b7x2k9m1q",
            "y⏎",
            Some("python3"),
        )),
    ));
    let mut amend = prompt_app(PermissionKind::Bash, "echo hi", "", None);
    press(&mut amend, KeyCode::Tab);
    pages.push(("permission amend", prompt(&amend)));

    // The ask modal — the list page, the preview page, the review page.
    pages.push((
        "ask list",
        crate::ui::ask_lines(&ask_app(vec![ask_question("Coffee", false)]), width),
    ));
    pages.push((
        "ask preview",
        crate::ui::ask_lines(&ask_app(vec![ask_question("Style", true)]), width),
    ));
    let mut review = ask_app(vec![
        ask_question("Coffee", false),
        ask_question("Tea", false),
    ]);
    press(&mut review, KeyCode::Right);
    press(&mut review, KeyCode::Right); // → the Submit page
    pages.push(("ask review", crate::ui::ask_lines(&review, width)));

    pages
}

#[test]
fn every_framed_page_sits_flush_against_its_rules() {
    // A framed page's two rules ARE its padding: a blank row just inside
    // either one costs a terminal row on every page and says nothing, so
    // each page opens on its first content row and closes on its last —
    // the frame the user asked for, the rows saved for the content
    // (`docs/view-flow.md`).
    for width in [40u16, 80] {
        let mut padded = Vec::new();
        for (name, lines) in framed_pages(width) {
            let rows: Vec<String> = lines.iter().map(plain).collect();
            assert!(rows.len() >= 3, "{name} @{width}: no content: {rows:#?}");
            assert!(
                rows[0].starts_with('─') && rows[rows.len() - 1].starts_with('─'),
                "{name} @{width}: not framed by rules: {rows:#?}"
            );
            if rows[1].trim().is_empty() {
                padded.push(format!("{name} @{width}: a blank row under the top rule"));
            }
            if rows[rows.len() - 2].trim().is_empty() {
                padded.push(format!("{name} @{width}: a blank row over the bottom rule"));
            }
        }
        assert!(padded.is_empty(), "padded frames: {padded:#?}");
    }
}
