//! The `/mcp` manager's state machine (`docs/mcp.md`).

use super::*;
use crate::app::{McpOp, McpPage, McpServerAction, server_actions};
use crate::mcp::{
    McpAuthState, McpScope, McpServerConfig, McpServerSnapshot, McpServerStatus, McpToolInfo,
};

fn snapshot(name: &str, status: McpServerStatus, tools: usize) -> McpServerSnapshot {
    // The auth row derives exactly as the manager derives it: only a server
    // that demands auth (or holds tokens) shows one.
    let auth = crate::mcp::auth_state(true, false, false, &status);
    McpServerSnapshot {
        name: name.to_string(),
        scope: McpScope::User,
        config_path: "~/.alter-zero/mcp.json".to_string(),
        config: McpServerConfig::Http {
            url: format!("https://{name}.test/mcp"),
            headers: Default::default(),
            sse_fallback: false,
        },
        status,
        auth,
        identity: None,
        tools: (0..tools)
            .map(|i| McpToolInfo {
                name: format!("tool_{i}"),
                description: format!("Tool {i}."),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            })
            .collect(),
    }
}

fn mcp_app() -> App {
    let mut app = App::new();
    app.open_mcp_menu(vec![
        snapshot("deepwiki", McpServerStatus::Connected, 3),
        snapshot("linear", McpServerStatus::NeedsAuth, 0),
        snapshot("github", McpServerStatus::Disabled, 0),
    ]);
    app
}

#[test]
fn opening_replaces_the_composer_bands() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_mcp_menu(vec![]);
    assert!(app.mcp_menu.is_some());
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none());
}

#[test]
fn enter_walks_list_to_server_to_tools_to_tool_and_esc_walks_back() {
    let mut app = mcp_app();
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Server);
    assert_eq!(menu.server, 0);
    // The connected server's first action is View tools — Enter opens them.
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Tools);
    // Down to the second tool, Enter opens its detail.
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(
        app.mcp_menu.as_ref().unwrap().page,
        McpPage::Tool { tool: 1 }
    );
    // Esc walks back up, remembering the selections.
    app.on_key(key(KeyCode::Esc));
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Tools);
    assert_eq!(menu.selected, 1);
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Server);
    app.on_key(key(KeyCode::Esc));
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::List);
    assert_eq!(menu.selected, 0, "back on the row that was entered");
    // Esc from the list closes.
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseMcpMenu);
    assert!(app.mcp_menu.is_none());
}

#[test]
fn server_actions_follow_the_state() {
    use McpServerAction as A;
    let connected = snapshot("d", McpServerStatus::Connected, 2);
    assert_eq!(
        server_actions(&connected),
        [A::ViewTools, A::Reconnect, A::Disable]
    );
    let mut authed = snapshot("d", McpServerStatus::Connected, 2);
    authed.auth = Some(McpAuthState::Authenticated);
    assert_eq!(
        server_actions(&authed),
        [
            A::ViewTools,
            A::Reauthenticate,
            A::ClearAuth,
            A::Reconnect,
            A::Disable
        ]
    );
    assert_eq!(
        server_actions(&snapshot("l", McpServerStatus::NeedsAuth, 0)),
        [A::Authenticate, A::Disable]
    );
    // A grant the server is refusing must be clearable from the very page
    // that reports it — it used to be removable only by editing the store.
    let mut expired = snapshot("l", McpServerStatus::NeedsAuth, 0);
    expired.auth = Some(McpAuthState::Expired);
    assert_eq!(
        server_actions(&expired),
        [A::Authenticate, A::ClearAuth, A::Disable]
    );
    assert_eq!(
        server_actions(&snapshot("g", McpServerStatus::Disabled, 0)),
        [A::Enable]
    );
    // A remote server that failed with no stored grant can now be logged
    // into from here; before, authenticating it was simply unreachable.
    assert_eq!(
        server_actions(&snapshot("f", McpServerStatus::Failed("x".to_string()), 0)),
        [A::Authenticate, A::Reconnect, A::Disable]
    );
    // A written-in header affords neither login nor clear.
    let mut header = snapshot("h", McpServerStatus::Connected, 1);
    header.auth = Some(McpAuthState::Header);
    assert_eq!(
        server_actions(&header),
        [A::ViewTools, A::Reconnect, A::Disable]
    );
}

#[test]
fn authenticate_opens_the_auth_page_and_dispatches_the_op() {
    let mut app = mcp_app();
    // Down to linear (needs auth), enter its detail.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    // Option 1 is Authenticate.
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Action::McpOp(McpOp::Authenticate {
            server: "linear".to_string()
        })
    );
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Auth);
    // The flow's URL lands on the page.
    app.set_mcp_auth_url("linear", "https://as.test/authorize?x=1");
    assert_eq!(
        app.mcp_menu.as_ref().unwrap().auth.url.as_deref(),
        Some("https://as.test/authorize?x=1")
    );
    // `c` with an empty field copies it.
    let action = app.on_key(key(KeyCode::Char('c')));
    assert_eq!(
        action,
        Action::McpOp(McpOp::CopyAuthUrl {
            url: "https://as.test/authorize?x=1".to_string()
        })
    );
    // Typing + Enter submits the pasted redirect.
    for c in "http://localhost:1/cb?code=abc".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Action::McpOp(McpOp::SubmitAuthUrl {
            text: "http://localhost:1/cb?code=abc".to_string()
        })
    );
    assert!(app.mcp_menu.as_ref().unwrap().auth.submitted);
    // The finished flow closes the page back to the server detail.
    app.finish_mcp_auth();
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Server);
}

#[test]
fn esc_on_the_auth_page_cancels_the_flow() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // Authenticate
    let action = app.on_key(key(KeyCode::Esc));
    assert_eq!(action, Action::McpOp(McpOp::CancelAuth));
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Server);
}

#[test]
fn disable_and_enable_dispatch_with_the_server_name() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Enter)); // deepwiki detail
    // Digit 3 = Disable (ViewTools, Reconnect, Disable).
    let action = app.on_key(key(KeyCode::Char('3')));
    assert_eq!(
        action,
        Action::McpOp(McpOp::SetDisabled {
            server: "deepwiki".to_string(),
            disabled: true
        })
    );
    // github (disabled): option 1 is Enable.
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Action::McpOp(McpOp::SetDisabled {
            server: "github".to_string(),
            disabled: false
        })
    );
}

#[test]
fn a_snapshot_update_clamps_selections_in_place() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down)); // selected = 2 (github)
    // The registry now reports only one server.
    app.update_mcp_snapshot(vec![snapshot("deepwiki", McpServerStatus::Connected, 1)]);
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.selected, 0);
    assert_eq!(menu.servers.len(), 1);
    // A tool page whose tool vanished falls back to the tools list.
    app.on_key(key(KeyCode::Enter)); // server
    app.on_key(key(KeyCode::Enter)); // tools
    app.on_key(key(KeyCode::Enter)); // tool 0
    app.update_mcp_snapshot(vec![snapshot("deepwiki", McpServerStatus::Connected, 0)]);
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Tools);
    // An empty snapshot resets to the list without panicking.
    app.update_mcp_snapshot(vec![]);
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::List);
}

#[test]
fn ctrl_c_closes_and_cancels_an_active_auth() {
    let mut app = mcp_app();
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.on_key(ctrl_c), Action::CloseMcpMenu);
    assert!(app.mcp_menu.is_none());
    // From the auth page the close also abandons the flow.
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // Authenticate
    assert_eq!(app.on_key(ctrl_c), Action::McpOp(McpOp::CancelAuth));
    assert!(app.mcp_menu.is_none());
}

#[test]
fn pastes_reach_only_the_auth_field() {
    let mut app = mcp_app();
    assert!(
        !app.paste_into_mcp_auth("https://x/cb?code=1"),
        "list page swallows"
    );
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // auth page
    assert!(app.paste_into_mcp_auth("https://x/cb?code=1\n"));
    assert_eq!(
        app.mcp_menu.as_ref().unwrap().auth.input,
        "https://x/cb?code=1"
    );
}

#[test]
fn digits_jump_to_a_server_on_the_list() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Char('2')));
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Server);
    assert_eq!(menu.server, 1);
}

#[test]
fn arrows_wrap_at_the_ends_on_every_page() {
    // List: 3 servers.
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.mcp_menu.as_ref().unwrap().selected,
        2,
        "Up from the first server wraps to the last"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.mcp_menu.as_ref().unwrap().selected,
        0,
        "Down from the last wraps back to the first"
    );
    // Server page: deepwiki's action rows.
    app.on_key(key(KeyCode::Enter));
    let actions = server_actions(&snapshot("deepwiki", McpServerStatus::Connected, 3)).len();
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.mcp_menu.as_ref().unwrap().selected,
        actions - 1,
        "the action rows wrap too"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.mcp_menu.as_ref().unwrap().selected, 0);
    // Tools page: deepwiki's 3 tools.
    app.on_key(key(KeyCode::Enter)); // View tools is the first action
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Tools);
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.mcp_menu.as_ref().unwrap().selected,
        2,
        "the tool rows wrap too"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.mcp_menu.as_ref().unwrap().selected, 0);
}
