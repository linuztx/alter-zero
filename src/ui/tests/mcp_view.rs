//! The `/mcp` manager's rendering (`docs/mcp.md`).

use super::*;
use crate::app::{McpPage, server_actions};
use crate::mcp::{
    McpAuthState, McpScope, McpServerConfig, McpServerSnapshot, McpServerStatus, McpToolInfo,
};
use crate::ui::mcp_view_lines;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn snapshot(name: &str, scope: McpScope, status: McpServerStatus) -> McpServerSnapshot {
    // Auth derives the way the manager derives it (`crate::mcp::auth_state`):
    // a row only when the server demands auth or tokens are stored.
    let auth = crate::mcp::auth_state(true, false, &status);
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
    assert!(all.contains("linear MCP Server"));
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
fn the_detail_shows_the_negotiated_protocol_and_hides_an_irrelevant_auth_row() {
    // deepwiki: connected, no auth story — the detail shows the settled
    // protocol revision and NO `Auth:` row (`✘ not authenticated` beside
    // `✔ connected` reads as a problem where there is none).
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // deepwiki detail
    let all = texts(&app).join("\n");
    assert!(all.contains("deepwiki MCP Server"));
    assert!(all.contains("Protocol:"), "{all}");
    assert!(all.contains("2026-07-28"));
    assert!(!all.contains("Auth:"), "{all}");
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
