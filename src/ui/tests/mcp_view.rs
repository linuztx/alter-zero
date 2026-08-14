//! The `/mcp` manager's rendering (`docs/mcp.md`).

use super::*;
use crate::app::{McpPage, server_actions};
use crate::mcp::{
    McpAuthState, McpScope, McpServerConfig, McpServerSnapshot, McpServerStatus, McpToolInfo,
};
use crate::ui::mcp_view_lines;
use crate::ui::theme::{
    MCP_DESCRIPTION_COLOR, MCP_DETAIL_LABEL_COLOR, MCP_DETAIL_STATE_COLOR, MCP_DETAIL_VALUE_COLOR,
    MCP_FIELD_COL, MCP_PARAM_BULLET, MCP_PARAM_INDENT, MCP_TITLE_COLOR, MCP_TOOL_FIELD_GAP,
    TOOL_FAIL_COLOR, TOOL_OK_COLOR,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn snapshot(name: &str, scope: McpScope, status: McpServerStatus) -> McpServerSnapshot {
    // Auth derives the way the manager derives it (`crate::mcp::auth_state`):
    // a row only when the server demands auth or tokens are stored.
    let auth = crate::mcp::auth_state(true, false, false, &status);
    McpServerSnapshot {
        name: name.to_string(),
        scope,
        config_path: match scope {
            McpScope::Project => "~/repo/.mcp.json".to_string(),
            McpScope::User => "~/.alter-zero/mcp.json".to_string(),
        },
        config: McpServerConfig::Http {
            url: format!("https://{name}.test/mcp"),
            headers: Default::default(),
            sse_fallback: false,
        },
        status,
        auth,
        identity: Some(crate::mcp::ServerIdentity {
            protocol_version: "2026-07-28".to_string(),
            name: name.to_string(),
            version: "1.0".to_string(),
            capabilities: vec!["tools".to_string()],
            instructions: None,
        }),
        tools: vec![McpToolInfo {
            name: "ask_question".to_string(),
            description: "Ask any question about a GitHub repository.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "repoName": {"description": "owner/repo"},
                    "question": {"type": "string", "description": "The question."}
                },
                "required": ["repoName", "question"]
            }),
        }],
    }
}

fn mcp_app() -> App {
    let mut app = App::new();
    app.open_mcp_menu(vec![
        snapshot("linear", McpScope::Project, McpServerStatus::NeedsAuth),
        snapshot("deepwiki", McpScope::User, McpServerStatus::Connected),
        snapshot("github", McpScope::User, McpServerStatus::Disabled),
    ]);
    app
}

fn texts(app: &App) -> Vec<String> {
    mcp_view_lines(app, 100).iter().map(plain).collect()
}

#[test]
fn the_list_groups_by_scope_with_status_rows() {
    let app = mcp_app();
    let lines = texts(&app);
    let all = lines.join("\n");
    assert!(all.contains("Manage MCP servers"));
    assert!(all.contains("3 servers"));
    assert!(all.contains("Project MCPs (~/repo/.mcp.json)"));
    assert!(all.contains("User MCPs (~/.alter-zero/mcp.json)"));
    assert!(all.contains("linear · △ needs authentication"));
    assert!(all.contains("deepwiki · ✔ connected · 1 tool"));
    assert!(all.contains("github · ◯ disabled"));
    // The selected row wears the ❯ marker.
    assert!(lines.iter().any(|l| l.contains("❯ linear")));
    assert!(all.contains("↑/↓ to navigate · Enter to confirm · Esc to cancel"));
}

#[test]
fn an_empty_list_names_the_config_files() {
    let mut app = App::new();
    app.open_mcp_menu(vec![]);
    let all = texts(&app).join("\n");
    assert!(all.contains("No MCP servers configured"));
    assert!(all.contains(".mcp.json"));
}

#[test]
fn the_server_detail_shows_facts_and_actions() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Enter)); // linear detail
    let all = texts(&app).join("\n");
    assert!(all.contains("Linear MCP Server"));
    assert!(all.contains("Status:"));
    assert!(all.contains("△ needs authentication"));
    assert!(all.contains("✘ not authenticated"));
    assert!(all.contains("URL:"));
    assert!(all.contains("https://linear.test/mcp"));
    assert!(all.contains("Config location:"));
    assert!(all.contains("1. Authenticate"));
    assert!(all.contains("2. Disable"));
    assert!(all.contains("❯ 1. Authenticate"), "first action selected");
}

#[test]
fn the_detail_shows_the_negotiated_protocol_and_a_truthful_auth_row() {
    // deepwiki: connected while presenting nothing, so it was never
    // challenged. The row is *shown* and says so — `✘ not authenticated`
    // beside `✔ connected` reported a problem where there is none, and
    // hiding the row entirely just leaves the question unanswered.
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // deepwiki detail
    let all = texts(&app).join("\n");
    assert!(all.contains("Deepwiki MCP Server"));
    assert!(all.contains("Protocol:"), "{all}");
    assert!(all.contains("2026-07-28"));
    assert!(all.contains("Auth:"), "{all}");
    assert!(all.contains("✔ authenticated"), "{all}");
    assert!(!all.contains("not authenticated"), "{all}");
    // An authenticated server still shows its ✔ row.
    let mut authed = snapshot("vercel", McpScope::User, McpServerStatus::Connected);
    authed.auth = Some(McpAuthState::Authenticated);
    let mut app = App::new();
    app.open_mcp_menu(vec![authed]);
    app.on_key(key(KeyCode::Enter));
    let all = texts(&app).join("\n");
    assert!(all.contains("Auth:"));
    assert!(all.contains("✔ authenticated"));
    assert!(all.contains("Protocol:"));
    // A server that never initialized has no revision to report: no row.
    let mut pending = snapshot("slow", McpScope::User, McpServerStatus::Pending);
    pending.identity = None;
    let mut app = App::new();
    app.open_mcp_menu(vec![pending]);
    app.on_key(key(KeyCode::Enter));
    let all = texts(&app).join("\n");
    assert!(!all.contains("Protocol:"));
}

#[test]
fn the_auth_row_distinguishes_a_header_a_dead_grant_and_a_public_server() {
    let row = |auth: Option<McpAuthState>| {
        let mut server = snapshot("s", McpScope::User, McpServerStatus::Connected);
        server.auth = auth;
        let mut app = App::new();
        app.open_mcp_menu(vec![server]);
        app.on_key(key(KeyCode::Enter));
        texts(&app).join("\n")
    };
    // A written-down token is a login the user already has — never "not
    // authenticated", and nothing to re-run or clear.
    let header = row(Some(McpAuthState::Header));
    assert!(
        header.contains("✔ authenticated (config header)"),
        "{header}"
    );
    assert!(!header.contains("Re-authenticate"), "{header}");
    assert!(!header.contains("Clear authentication"), "{header}");
    // A public server reads settled — the same row a stored grant earns,
    // because from the user's side the answer is the same: you are cleared
    // to use it. It stays actionless, though: there is no grant to re-run.
    let public = row(Some(McpAuthState::NotRequired));
    assert!(public.contains("✔ authenticated"), "{public}");
    assert!(!public.contains("Re-authenticate"), "{public}");
    // A grant the server refuses reads expired — and stays removable.
    let expired = row(Some(McpAuthState::Expired));
    assert!(expired.contains("✘ expired"), "{expired}");
    assert!(expired.contains("Clear authentication"), "{expired}");
}

#[test]
fn the_tools_and_tool_detail_pages_render() {
    let mut app = mcp_app();
    // deepwiki (connected) → View tools → tool detail.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // View tools
    let all = texts(&app).join("\n");
    assert!(all.contains("Tools for deepwiki"));
    assert!(all.contains("1 tool"));
    assert!(all.contains("1. ask_question"));
    app.on_key(key(KeyCode::Enter)); // tool detail
    let all = texts(&app).join("\n");
    assert!(all.contains("Tool name:"));
    assert!(all.contains("ask_question"));
    assert!(all.contains("Full name:"));
    assert!(all.contains("mcp__deepwiki__ask_question"));
    assert!(all.contains("Description:"));
    assert!(all.contains("Parameters:"));
    assert!(all.contains("● repoName (required): unknown - owner/repo"));
    assert!(all.contains("● question (required): string - The question."));
    assert!(all.contains("Esc to go back"));
}

/// Walk `app` to the `ask_question` detail page (list → deepwiki → tools →
/// the tool), the deepest page and the one the two-tone body lives on.
fn tool_detail_app() -> App {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down)); // deepwiki (connected — it has tools)
    app.on_key(key(KeyCode::Enter)); // its detail
    app.on_key(key(KeyCode::Enter)); // View tools
    app.on_key(key(KeyCode::Enter)); // ask_question
    app
}

#[test]
fn every_page_headline_is_cyan_and_the_server_one_is_capitalised() {
    // The manager is a four-page walk, so the headline is the only row that
    // says where you are — it wears the cyan the frame's white titles were
    // too quiet to carry (`docs/mcp.md`).
    let headline = |app: &App| {
        let line = mcp_view_lines(app, 100)
            .into_iter()
            .find(|line| {
                line.spans
                    .iter()
                    .any(|span| span.style.fg == Some(MCP_TITLE_COLOR))
            })
            .expect("a cyan headline");
        plain(&line).trim().to_string()
    };
    let mut app = mcp_app();
    assert_eq!(headline(&app), "Manage MCP servers");
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // deepwiki detail
    // A config key is lower-case by convention; a headline is a headline.
    assert_eq!(headline(&app), "Deepwiki MCP Server");
    app.on_key(key(KeyCode::Enter)); // View tools
    assert_eq!(headline(&app), "Tools for deepwiki");
    app.on_key(key(KeyCode::Enter)); // the tool detail
    assert_eq!(headline(&app), "ask_question");
    // …over the server it belongs to, dim: the name identifies, it doesn't
    // announce.
    let under = mcp_view_lines(&app, 100)
        .into_iter()
        .find(|line| plain(line).trim() == "deepwiki")
        .expect("the server subtitle");
    assert_eq!(under.spans[1].style.fg, Some(MCP_DETAIL_VALUE_COLOR));
}

#[test]
fn the_server_detail_lights_its_labels_and_only_the_state_values() {
    // deepwiki: connected, one tool, never challenged for credentials.
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    let lines = mcp_view_lines(&app, 100);
    let find = |text: &str| {
        lines
            .iter()
            .find(|line| plain(line).contains(text))
            .unwrap_or_else(|| panic!("no row containing {text:?}"))
            .clone()
    };
    // The tool count is the `Tools:` row's job — a page that says it twice
    // is a page with a duplicate on it.
    let status = find("Status:");
    assert_eq!(
        plain(&status),
        format!("  {:<MCP_FIELD_COL$}✔ connected", "Status:")
    );
    assert_eq!(status.spans[1].style.fg, Some(MCP_DETAIL_LABEL_COLOR));
    // The glyph keeps its state colour — it is the one thing on the row that
    // still has to shout when a server is failing — over white words.
    assert_eq!(status.spans[2].content.as_ref(), "✔ ");
    assert_eq!(status.spans[2].style.fg, Some(TOOL_OK_COLOR));
    assert_eq!(status.spans[3].content.as_ref(), "connected");
    assert_eq!(status.spans[3].style.fg, Some(MCP_DETAIL_STATE_COLOR));
    // A server that needs no login is one you are cleared to use, so it
    // reports the settled row rather than a `◯ not needed` shrug.
    let auth = find("Auth:");
    assert_eq!(
        plain(&auth),
        format!("  {:<MCP_FIELD_COL$}✔ authenticated", "Auth:")
    );
    assert_eq!(auth.spans[1].style.fg, Some(MCP_DETAIL_LABEL_COLOR));
    assert_eq!(auth.spans[2].style.fg, Some(TOOL_OK_COLOR));
    assert_eq!(auth.spans[3].style.fg, Some(MCP_DETAIL_STATE_COLOR));
    // The addresses, the revision and the count are what the labels lead to,
    // not what the page is about: bright label, quiet value.
    for row in ["Protocol:", "URL:", "Config location:", "Tools:"] {
        let line = find(row);
        assert_eq!(
            line.spans[1].style.fg,
            Some(MCP_DETAIL_LABEL_COLOR),
            "{row}"
        );
        assert_eq!(
            line.spans[2].style.fg,
            Some(MCP_DETAIL_VALUE_COLOR),
            "{row}"
        );
    }
    // What the server can actually do stays lit with the state rows.
    let caps = find("Capabilities:");
    assert_eq!(caps.spans[1].style.fg, Some(MCP_DETAIL_LABEL_COLOR));
    assert_eq!(caps.spans[2].style.fg, Some(MCP_DETAIL_STATE_COLOR));
}

#[test]
fn a_failing_server_keeps_its_red_glyph_beside_the_white_words() {
    // The two-tone must not launder a failure into a calm white row.
    let mut server = snapshot(
        "broken",
        McpScope::User,
        McpServerStatus::Failed("no".into()),
    );
    server.auth = Some(McpAuthState::Expired);
    let mut app = App::new();
    app.open_mcp_menu(vec![server]);
    app.on_key(key(KeyCode::Enter));
    let lines = mcp_view_lines(&app, 100);
    let find = |text: &str| {
        lines
            .iter()
            .find(|line| plain(line).contains(text))
            .unwrap_or_else(|| panic!("no row containing {text:?}"))
            .clone()
    };
    let status = find("Status:");
    assert_eq!(status.spans[2].style.fg, Some(TOOL_FAIL_COLOR));
    assert_eq!(status.spans[3].style.fg, Some(MCP_DETAIL_STATE_COLOR));
    let auth = find("Auth:");
    assert_eq!(
        plain(&auth),
        format!("  {:<MCP_FIELD_COL$}✘ expired", "Auth:")
    );
    assert_eq!(auth.spans[2].style.fg, Some(TOOL_FAIL_COLOR));
}

#[test]
fn the_tool_detail_reads_label_bright_and_value_dim() {
    let app = tool_detail_app();
    let lines = mcp_view_lines(&app, 100);
    let find = |text: &str| {
        lines
            .iter()
            .find(|line| plain(line).contains(text))
            .unwrap_or_else(|| panic!("no row containing {text:?}"))
            .clone()
    };
    // One space after the label, not the server page's 18-column pad — the
    // two labels are the same width, so they line up on their own.
    let name = find("Tool name:");
    assert_eq!(
        plain(&name),
        format!("  Tool name:{MCP_TOOL_FIELD_GAP}ask_question")
    );
    assert_eq!(name.spans[1].style.fg, Some(MCP_DETAIL_LABEL_COLOR));
    assert_eq!(name.spans[2].style.fg, Some(MCP_DETAIL_VALUE_COLOR));
    let full = find("Full name:");
    assert_eq!(
        plain(&full),
        format!("  Full name:{MCP_TOOL_FIELD_GAP}mcp__deepwiki__ask_question")
    );
    assert_eq!(full.spans[1].style.fg, Some(MCP_DETAIL_LABEL_COLOR));
    assert_eq!(full.spans[2].style.fg, Some(MCP_DETAIL_VALUE_COLOR));
    // The description's label is bright like every other; the prose under it
    // is **half white** — a third tone, so it can't be mistaken for either
    // the labels organising the page or the schema boilerplate below it.
    assert_eq!(
        find("Description:").spans[1].style.fg,
        Some(MCP_DETAIL_LABEL_COLOR)
    );
    assert_eq!(
        find("Ask any question").spans[1].style.fg,
        Some(MCP_DESCRIPTION_COLOR)
    );
    assert_ne!(MCP_DESCRIPTION_COLOR, MCP_DETAIL_LABEL_COLOR);
    assert_ne!(MCP_DESCRIPTION_COLOR, MCP_DETAIL_VALUE_COLOR);
    assert_eq!(
        find("Parameters:").spans[1].style.fg,
        Some(MCP_DETAIL_LABEL_COLOR)
    );
}

#[test]
fn a_parameter_lights_its_name_and_dims_what_it_introduces() {
    // Narrow enough that the first parameter wraps: the name leads its row
    // bright, the schema prose after it dims, and the continuation row —
    // carrying no name — is dim throughout.
    let app = tool_detail_app();
    let lines = mcp_view_lines(&app, 40);
    let at = lines
        .iter()
        .position(|line| plain(line).contains("● repoName"))
        .expect("the repoName row");
    let head = &lines[at];
    assert_eq!(head.spans[1].content.as_ref(), MCP_PARAM_BULLET);
    assert_eq!(head.spans[1].style.fg, Some(MCP_DETAIL_LABEL_COLOR));
    assert_eq!(head.spans[2].content.as_ref(), "repoName");
    assert_eq!(head.spans[2].style.fg, Some(MCP_DETAIL_LABEL_COLOR));
    assert!(plain(head).contains("(required): unknown"), "{head:?}");
    assert_eq!(head.spans[3].style.fg, Some(MCP_DETAIL_VALUE_COLOR));
    let tail = &lines[at + 1];
    assert_eq!(plain(tail).trim(), "owner/repo");
    assert_eq!(tail.spans[1].content.as_ref(), MCP_PARAM_INDENT);
    for span in tail.spans.iter().skip(1) {
        assert_eq!(span.style.fg, Some(MCP_DETAIL_VALUE_COLOR), "{span:?}");
    }
}

#[test]
fn the_menu_hides_the_cursor_seated_on_the_selection_and_auth_brings_it_back() {
    let app = mcp_app();
    // The permission prompt's rule (docs/mcp.md): a menu shows no hardware
    // cursor — a kitty cursor animation blinks at whatever seat it picks —
    // while the seat itself tracks the highlighted `❯` row.
    assert!(!crate::ui::cursor_visible(&app));
    let height = crate::ui::mcp_menu_height(&app, 100, 60).unwrap();
    let mut buf = buffer(100, height);
    render_mcp_menu(buf.area, &mut buf, &app);
    let marker_row = (0..height)
        .find(|&y| row(&buf, y, 100).trim_start().starts_with('❯'))
        .expect("the selected server wears the marker");
    assert_eq!(crate::ui::cursor_position(buf.area, &app), (2, marker_row));

    // The Auth page's `URL >` field is typed into, so its caret comes back
    // (the permission prompt's amend-field exception).
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Enter)); // linear
    app.on_key(key(KeyCode::Enter)); // Authenticate
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Auth);
    assert!(crate::ui::cursor_visible(&app));
}

#[test]
fn the_auth_page_shows_the_url_and_paste_field() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Enter)); // linear
    app.on_key(key(KeyCode::Enter)); // Authenticate
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Auth);
    let all = texts(&app).join("\n");
    assert!(all.contains("Authenticating with linear…"));
    assert!(all.contains("A browser window will open"));
    // No URL yet: the waiting note stands in.
    assert!(all.contains("Preparing the authorization request…"));
    app.set_mcp_auth_url("linear", "https://as.test/authorize?client_id=c1");
    let all = texts(&app).join("\n");
    assert!(all.contains("copy this URL manually (c to copy)"));
    assert!(all.contains("https://as.test/authorize?client_id=c1"));
    assert!(all.contains("URL > "));
    assert!(all.contains("Press Esc to go back"));
    // Typed text lands on the field row.
    app.on_key(key(KeyCode::Char('h')));
    app.on_key(key(KeyCode::Char('i')));
    let all = texts(&app).join("\n");
    assert!(all.contains("URL > hi"));
}

#[test]
fn height_agrees_with_the_built_lines_and_none_when_closed() {
    let mut app = mcp_app();
    let height = crate::ui::mcp_menu_height(&app, 100, 60).expect("open menu has a height");
    assert!(height > 0);
    app.close_mcp_menu();
    assert!(crate::ui::mcp_menu_height(&app, 100, 60).is_none());
    assert!(mcp_view_lines(&app, 100).is_empty());
}

#[test]
fn a_long_list_windows_with_overflow_markers() {
    let mut app = App::new();
    let servers: Vec<_> = (0..9)
        .map(|i| {
            snapshot(
                &format!("srv{i}"),
                McpScope::User,
                McpServerStatus::Connected,
            )
        })
        .collect();
    app.open_mcp_menu(servers);
    // Jump to the end so the window slides.
    app.on_key(key(KeyCode::End));
    let all = texts(&app).join("\n");
    assert!(all.contains("more above"), "{all}");
    assert!(all.contains("srv8"));
    // The action list still derives (sanity: server_actions is exercised).
    assert_eq!(
        server_actions(&app.mcp_menu.as_ref().unwrap().servers[0]).len(),
        3
    );
}
