//! The `/mcp` manager's state machine (`docs/mcp.md`).

use super::*;
use crate::app::{McpClientField, McpClientForm, McpOp, McpPage, McpServerAction, server_actions};
use crate::mcp::{
    McpAuthState, McpClientRequest, McpScope, McpServerConfig, McpServerSnapshot, McpServerStatus,
    McpToolInfo, OAuthClientConfig,
};
use crate::secrets::SecretValue;

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
            oauth: None,
        },
        status,
        auth,
        identity: None,
        excluded: Vec::new(),
        kept_client: None,
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
    // MCP is off for the session (`/settings` → MCP): nothing connects, so
    // the one decision left is the server's own switch, for when MCP is back
    // on — no Reconnect, and no sign-in for a server that cannot connect.
    assert_eq!(
        server_actions(&snapshot("o", McpServerStatus::Off, 0)),
        [A::Disable]
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
    assert!(!app.finish_mcp_auth("linear", true, "Authenticated with linear"));
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

/// Reconnect (or Enable) turns the server `connecting…`, and the snapshot
/// that says so lands under the user's finger: the selection followed its
/// *index* onto a connecting server's only action, Disable, so a second
/// Enter disabled the server and saved that. It follows the action now, and
/// a connecting server's first action is the harmless one.
#[test]
fn a_second_enter_after_reconnect_never_lands_on_disable() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Enter)); // deepwiki detail
    app.on_key(key(KeyCode::Down)); // Reconnect
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::Reconnect {
            server: "deepwiki".to_string()
        })
    );
    app.update_mcp_snapshot(vec![
        snapshot("deepwiki", McpServerStatus::Pending, 0),
        snapshot("linear", McpServerStatus::NeedsAuth, 0),
        snapshot("github", McpServerStatus::Disabled, 0),
    ]);
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::Reconnect {
            server: "deepwiki".to_string()
        })
    );
    // Enable from a disabled server: the connecting page's first action is
    // Reconnect, never Disable.
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // github detail
    app.on_key(key(KeyCode::Enter)); // Enable
    app.update_mcp_snapshot(vec![
        snapshot("deepwiki", McpServerStatus::Connected, 3),
        snapshot("linear", McpServerStatus::NeedsAuth, 0),
        snapshot("github", McpServerStatus::Pending, 0),
    ]);
    assert!(!matches!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::SetDisabled { .. })
    ));
}

/// A sign-in finishing closes only its own server's auth page: another
/// server's flow ending — a superseded one — must not close the page the
/// user is on now.
#[test]
fn a_finished_sign_in_closes_only_its_own_page() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // linear's Authenticate
    assert!(!app.finish_mcp_auth("deepwiki", true, "Authenticated with deepwiki"));
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Auth);
    assert!(!app.finish_mcp_auth("linear", true, "Authenticated with linear"));
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Server);
}

/// A paste the flow turned away gives the field back with the reason, and
/// editing the URL clears it.
#[test]
fn a_rejected_paste_gives_the_field_back_with_its_reason() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // Authenticate
    assert!(app.paste_into_mcp_auth("http://localhost:1/callback?code=a&state=old"));
    app.on_key(key(KeyCode::Enter));
    assert!(app.mcp_menu.as_ref().unwrap().auth.submitted);
    // Another server's rejection is not this page's.
    app.reject_mcp_auth_paste("deepwiki", "nope");
    assert_eq!(app.mcp_menu.as_ref().unwrap().auth.error, None);
    app.reject_mcp_auth_paste("linear", "the response belongs to another sign-in attempt");
    let auth = &app.mcp_menu.as_ref().unwrap().auth;
    assert!(!auth.submitted, "the field takes a new paste");
    assert_eq!(
        auth.error.as_deref(),
        Some("the response belongs to another sign-in attempt")
    );
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.mcp_menu.as_ref().unwrap().auth.error, None);
}

// --- a failed sign-in (`docs/mcp.md` *The `/mcp` manager*) ---

/// linear's auth page, its sign-in running.
fn auth_page_app() -> App {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // Authenticate
    app
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

/// A failed sign-in stays on its page with the whole reason — a toast row
/// cut it at the width and took it away. Enter tries again; Esc goes back,
/// with no sign-in left to cancel.
#[test]
fn a_failed_sign_in_keeps_its_reason_on_the_page() {
    let reason = "token exchange failed: HTTP 400: invalid_grant: the code expired";
    let mut app = auth_page_app();
    app.set_mcp_auth_url("linear", "https://as.test/authorize?x=1");
    assert!(
        app.finish_mcp_auth("linear", false, reason),
        "the page took it"
    );
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Auth);
    assert_eq!(menu.auth.failed.as_deref(), Some(reason));
    assert_eq!(menu.auth.url, None, "the dead sign-in's URL is gone");
    // Nothing is typed on a failed page.
    type_text(&mut app, "c");
    assert_eq!(app.mcp_menu.as_ref().unwrap().auth.input, "");
    assert!(!app.paste_into_mcp_auth("https://x/cb?code=1"));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::Authenticate {
            server: "linear".to_string()
        })
    );
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Auth);
    assert_eq!(menu.auth, Default::default());
    // Esc from a failed page: back, nothing to cancel.
    assert!(app.finish_mcp_auth("linear", false, reason));
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Server);
}

/// A failure no open page takes — the user left, or it is another server's
/// — is the boundary's to toast; so is every success.
#[test]
fn a_failure_no_open_page_takes_is_left_to_the_toast() {
    let mut app = auth_page_app();
    assert!(!app.finish_mcp_auth("deepwiki", false, "nope"));
    assert_eq!(app.mcp_menu.as_ref().unwrap().auth.failed, None);
    app.on_key(key(KeyCode::Esc));
    assert!(!app.finish_mcp_auth("linear", false, "nope"));
    let mut closed = App::new();
    assert!(!closed.finish_mcp_auth("linear", false, "nope"));
}

// --- the client form (`docs/mcp.md` *Pre-registered clients*) ---

/// What a sign-in with no client to present asks for.
fn client_request() -> McpClientRequest {
    McpClientRequest {
        issuer: "https://accounts.google.com".to_string(),
        client_id: None,
        fixed_id: false,
        has_secret: false,
        callback_port: Some(8976),
        redirect_uri: None,
        reason: None,
    }
}

/// linear's auth page turned into the client form over `request`.
fn form_app(request: &McpClientRequest) -> App {
    let mut app = auth_page_app();
    assert!(app.ask_mcp_client("linear", request));
    app
}

fn form(app: &App) -> &McpClientForm {
    app.mcp_menu
        .as_ref()
        .unwrap()
        .auth
        .client
        .as_ref()
        .expect("the client form")
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// A sign-in that has no client to present turns its auth page into the
/// client form: who issues the client, the port its redirect URI names, the
/// id and then the secret — and submitting hands the boundary the client to
/// keep, the page waiting on the sign-in that follows.
#[test]
fn a_sign_in_with_no_client_turns_the_auth_page_into_the_client_form() {
    let mut app = form_app(&client_request());
    let opened = form(&app);
    assert_eq!(opened.issuer, "https://accounts.google.com");
    assert_eq!(opened.port, "8976");
    assert_eq!(opened.field, McpClientField::ClientId);
    assert_eq!(opened.fixed_client_id, None);
    type_text(&mut app, "123.apps.example");
    // Enter on the id moves on to the secret; nothing is sent yet.
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(form(&app).field, McpClientField::Secret);
    assert!(app.paste_into_mcp_auth("s3cret\n"));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::SaveAuthClient {
            server: "linear".to_string(),
            issuer: "https://accounts.google.com".to_string(),
            client_id: "123.apps.example".to_string(),
            client_secret: Some(SecretValue::new("s3cret")),
            callback_port: Some(8976),
        })
    );
    // The page is the auth page again, waiting on the sign-in that follows.
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Auth);
    assert_eq!(menu.auth, Default::default());
}

/// Tab and the arrows walk the three fields in the order they sit on the
/// page, wrapping; keys edit the focused one — digits alone in the port —
/// and Ctrl+U empties it.
#[test]
fn the_client_form_cycles_its_fields_and_edits_the_focused_one() {
    use McpClientField::{ClientId, Port, Secret};
    let mut app = form_app(&client_request());
    for (press, lands) in [
        (KeyCode::Tab, Secret),
        (KeyCode::Tab, Port),
        (KeyCode::Tab, ClientId),
        (KeyCode::BackTab, Port),
        (KeyCode::BackTab, Secret),
        (KeyCode::Up, ClientId),
        (KeyCode::Down, Secret),
        (KeyCode::Down, Port),
    ] {
        app.on_key(key(press));
        assert_eq!(form(&app).field, lands, "after {press:?}");
    }
    // The port takes digits, five at most.
    app.on_key(ctrl('u'));
    assert_eq!(form(&app).port, "");
    type_text(&mut app, "9x0-01 23");
    assert_eq!(form(&app).port, "90012");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(form(&app).port, "9001");
    // Enter on a field that is not the last moves on.
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(form(&app).field, ClientId);
    type_text(&mut app, "abc");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(form(&app).client_id, "ab");
    app.on_key(key(KeyCode::Tab));
    type_text(&mut app, "pw");
    assert_eq!(form(&app).secret, SecretValue::new("pw"));
    app.on_key(ctrl('u'));
    assert!(form(&app).secret.is_empty());
    assert_eq!(form(&app).client_id, "ab", "only the focused field");
}

/// The secret is optional: a public client submits without one.
#[test]
fn a_public_client_needs_no_secret() {
    let mut app = form_app(&client_request());
    type_text(&mut app, "  public-cid ");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::SaveAuthClient {
            server: "linear".to_string(),
            issuer: "https://accounts.google.com".to_string(),
            client_id: "public-cid".to_string(),
            client_secret: None,
            callback_port: Some(8976),
        })
    );
}

/// A submit that can't go says why and puts the caret in the field at
/// fault; the next edit clears it.
#[test]
fn the_client_form_says_what_stops_a_submit() {
    let mut app = form_app(&client_request());
    app.on_key(key(KeyCode::Tab)); // the secret: the last field
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(
        form(&app)
            .error
            .as_deref()
            .is_some_and(|e| e.contains("client ID"))
    );
    assert_eq!(form(&app).field, McpClientField::ClientId);
    type_text(&mut app, "two words");
    assert_eq!(form(&app).error, None, "an edit clears it");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(
        form(&app)
            .error
            .as_deref()
            .is_some_and(|e| e.contains("spaces"))
    );
    assert_eq!(form(&app).field, McpClientField::ClientId);
    // A port that is no port.
    app.on_key(ctrl('u'));
    type_text(&mut app, "cid");
    for bad in ["", "0", "70000"] {
        app.on_key(key(KeyCode::Up)); // the port
        assert_eq!(form(&app).field, McpClientField::Port);
        app.on_key(ctrl('u'));
        type_text(&mut app, bad);
        app.on_key(key(KeyCode::BackTab)); // wraps to the secret
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::None,
            "port {bad:?}"
        );
        assert!(
            form(&app)
                .error
                .as_deref()
                .is_some_and(|e| e.contains("1 to 65535")),
            "port {bad:?}: {:?}",
            form(&app).error
        );
        assert_eq!(form(&app).field, McpClientField::Port);
        app.on_key(key(KeyCode::Down)); // back on the id
    }
}

/// A server whose entry names its client is asked only for the secret: the
/// id is the entry's, no field moves to it or to the port, and a secret is
/// required unless one is already kept.
#[test]
fn a_configured_client_is_asked_only_for_its_secret() {
    let request = McpClientRequest {
        client_id: Some("file-cid".to_string()),
        fixed_id: true,
        callback_port: Some(8080),
        ..client_request()
    };
    let mut app = form_app(&request);
    assert_eq!(form(&app).fixed_client_id.as_deref(), Some("file-cid"));
    assert_eq!(form(&app).field, McpClientField::Secret);
    for press in [KeyCode::Tab, KeyCode::BackTab, KeyCode::Up, KeyCode::Down] {
        app.on_key(key(press));
        assert_eq!(form(&app).field, McpClientField::Secret, "after {press:?}");
    }
    // No secret typed and none kept: nothing to sign in with.
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(
        form(&app)
            .error
            .as_deref()
            .is_some_and(|e| e.contains("secret"))
    );
    type_text(&mut app, "shh");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::SaveAuthClient {
            server: "linear".to_string(),
            issuer: "https://accounts.google.com".to_string(),
            client_id: "file-cid".to_string(),
            client_secret: Some(SecretValue::new("shh")),
            callback_port: Some(8080),
        })
    );
    // With one kept, an empty field keeps it.
    let kept = McpClientRequest {
        has_secret: true,
        ..request
    };
    let mut app = form_app(&kept);
    assert!(matches!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::SaveAuthClient {
            client_secret: None,
            ..
        })
    ));
}

/// A request that calls its id fixed without naming one is a request for a
/// whole client: the form never opens with no field the caret can reach.
#[test]
fn a_fixed_id_with_no_id_asks_for_the_whole_client() {
    let request = McpClientRequest {
        fixed_id: true,
        client_id: None,
        ..client_request()
    };
    let mut app = form_app(&request);
    assert_eq!(form(&app).fixed_client_id, None);
    assert_eq!(form(&app).field, McpClientField::ClientId);
    type_text(&mut app, "cid");
    assert_eq!(form(&app).client_id, "cid");
}

/// A form that is back because the client was turned away opens with what
/// was entered and the reason, the caret on the secret — one Enter tries
/// the same client again, its kept secret untouched.
#[test]
fn a_form_reopened_with_a_reason_keeps_what_was_entered() {
    let request = McpClientRequest {
        client_id: Some("typed-cid".to_string()),
        has_secret: true,
        callback_port: Some(9001),
        reason: Some("as.test turned this client away (HTTP 401: invalid_client).".to_string()),
        ..client_request()
    };
    let mut app = form_app(&request);
    let reopened = form(&app);
    assert_eq!(reopened.client_id, "typed-cid");
    assert_eq!(reopened.port, "9001");
    assert!(reopened.has_secret);
    assert_eq!(reopened.reason, request.reason);
    assert_eq!(reopened.field, McpClientField::Secret);
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::SaveAuthClient {
            server: "linear".to_string(),
            issuer: "https://accounts.google.com".to_string(),
            client_id: "typed-cid".to_string(),
            client_secret: None,
            callback_port: Some(9001),
        })
    );
}

/// Esc on the form goes back to the server's page. No sign-in is running —
/// the one that asked has ended — so nothing is cancelled.
#[test]
fn esc_on_the_client_form_goes_back_without_cancelling_anything() {
    let mut app = form_app(&client_request());
    type_text(&mut app, "half-typed");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Server);
    assert_eq!(menu.auth, Default::default());
}

/// The request reaches only the auth page of the server it is for: a page
/// the user has left, another server's, or a closed menu leaves it to the
/// boundary's toast.
#[test]
fn a_client_request_for_a_page_no_longer_open_is_not_shown() {
    let request = client_request();
    let mut app = auth_page_app();
    assert!(
        !app.ask_mcp_client("deepwiki", &request),
        "another server's"
    );
    assert!(app.mcp_menu.as_ref().unwrap().auth.client.is_none());
    app.on_key(key(KeyCode::Esc)); // back on linear's page
    assert!(!app.ask_mcp_client("linear", &request), "the page was left");
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Server);
    let mut closed = App::new();
    assert!(!closed.ask_mcp_client("linear", &request));
}

/// Ctrl+Y copies the redirect URI the form shows — the one to register
/// with the provider — and does nothing while the port is no port.
#[test]
fn ctrl_y_copies_the_redirect_uri_the_form_shows() {
    let mut app = form_app(&client_request());
    assert_eq!(
        app.on_key(ctrl('y')),
        Action::McpOp(McpOp::CopyRedirectUri {
            uri: "http://localhost:8976/callback".to_string()
        })
    );
    app.on_key(key(KeyCode::Up)); // the port
    app.on_key(ctrl('u'));
    assert_eq!(app.on_key(ctrl('y')), Action::None);
    type_text(&mut app, "3000");
    assert_eq!(
        app.on_key(ctrl('y')),
        Action::McpOp(McpOp::CopyRedirectUri {
            uri: "http://localhost:3000/callback".to_string()
        })
    );
    // A typed `y` is a letter, never the copy.
    app.on_key(key(KeyCode::Down));
    type_text(&mut app, "y");
    assert_eq!(form(&app).client_id, "y");
}

/// An entry whose own client names its redirect URI (`callbackUrl`) is
/// asked for its secret beside that very URI — the one the sign-in sends —
/// and Ctrl+Y copies it.
#[test]
fn the_entrys_own_redirect_uri_is_the_one_the_form_names() {
    let request = McpClientRequest {
        client_id: Some("file-cid".to_string()),
        fixed_id: true,
        callback_port: Some(5555),
        redirect_uri: Some("http://127.0.0.1:5555/callback/file-cid".to_string()),
        ..client_request()
    };
    let mut app = form_app(&request);
    assert_eq!(
        form(&app).redirect_uri().as_deref(),
        Some("http://127.0.0.1:5555/callback/file-cid")
    );
    assert_eq!(
        app.on_key(ctrl('y')),
        Action::McpOp(McpOp::CopyRedirectUri {
            uri: "http://127.0.0.1:5555/callback/file-cid".to_string()
        })
    );
}

/// A paste lands in the focused field: line breaks dropped and the ends
/// trimmed; the port keeps digits alone.
#[test]
fn pastes_into_the_client_form_land_in_the_focused_field() {
    let mut app = form_app(&client_request());
    assert!(app.paste_into_mcp_auth("  123.apps\r\n.example \n"));
    assert_eq!(form(&app).client_id, "123.apps.example");
    app.on_key(key(KeyCode::Up)); // the port
    app.on_key(ctrl('u'));
    assert!(app.paste_into_mcp_auth(" 80 80\n"));
    assert_eq!(form(&app).port, "8080");
    app.on_key(key(KeyCode::BackTab)); // the secret
    assert!(app.paste_into_mcp_auth("GOCSPX-abc\n"));
    assert_eq!(form(&app).secret, SecretValue::new("GOCSPX-abc"));
    assert_eq!(
        app.mcp_menu.as_ref().unwrap().auth.input,
        "",
        "never the URL field behind the form"
    );
}

/// The typed secret is held redacted: nothing that prints the menu's state
/// — a test failure, a debug log — can spell it.
#[test]
fn the_typed_secret_never_shows_in_the_menus_debug_output() {
    let mut app = form_app(&client_request());
    type_text(&mut app, "cid");
    app.on_key(key(KeyCode::Enter));
    type_text(&mut app, "GOCSPX-very-secret");
    let printed = format!("{:?}", app.mcp_menu);
    assert!(!printed.contains("GOCSPX"), "{printed}");
    let action = app.on_key(key(KeyCode::Enter));
    assert!(!format!("{action:?}").contains("GOCSPX"), "{action:?}");
}

/// A client that could not be kept comes back as a failed sign-in, on the
/// page the form was on: the reason whole, the form gone with the attempt.
#[test]
fn a_client_that_cannot_be_kept_fails_on_the_forms_page() {
    let mut app = form_app(&client_request());
    assert!(app.finish_mcp_auth("linear", false, "couldn't keep the client"));
    let auth = &app.mcp_menu.as_ref().unwrap().auth;
    assert!(auth.client.is_none());
    assert_eq!(auth.failed.as_deref(), Some("couldn't keep the client"));
}

/// The client rows appear only where a client is on hand: one entered here
/// can be changed or forgotten; an entry naming its client without a
/// secret can be given one; a secret the entry spells is the file's.
#[test]
fn server_actions_offer_the_client_rows_where_a_client_is_on_hand() {
    use McpServerAction as A;
    let mut entered = snapshot("l", McpServerStatus::NeedsAuth, 0);
    entered.kept_client = Some("typed-cid".to_string());
    assert_eq!(
        server_actions(&entered),
        [
            A::Authenticate,
            A::ChangeClient,
            A::ForgetClient,
            A::Disable
        ]
    );
    let mut signed_in = snapshot("l", McpServerStatus::Connected, 1);
    signed_in.auth = Some(McpAuthState::Authenticated);
    signed_in.kept_client = Some("typed-cid".to_string());
    assert_eq!(
        server_actions(&signed_in),
        [
            A::ViewTools,
            A::Reauthenticate,
            A::ClearAuth,
            A::ChangeClient,
            A::ForgetClient,
            A::Reconnect,
            A::Disable
        ]
    );
    let naming = |secret: Option<&str>| {
        let mut server = snapshot("l", McpServerStatus::NeedsAuth, 0);
        server.config = McpServerConfig::Http {
            url: "https://l.test/mcp".to_string(),
            headers: Default::default(),
            sse_fallback: false,
            oauth: Some(OAuthClientConfig {
                client_id: Some("file-cid".to_string()),
                client_secret: secret.map(str::to_string),
                ..Default::default()
            }),
        };
        server
    };
    assert_eq!(
        server_actions(&naming(None)),
        [A::Authenticate, A::SetClientSecret, A::Disable]
    );
    assert_eq!(
        server_actions(&naming(Some("${SECRET}"))),
        [A::Authenticate, A::Disable]
    );
    // The entry's own client outranks one entered here: nothing to change.
    let mut both = naming(None);
    both.kept_client = Some("typed-cid".to_string());
    assert_eq!(
        server_actions(&both),
        [A::Authenticate, A::SetClientSecret, A::Disable]
    );
    // A written-in header signs in with no OAuth at all.
    let mut header = snapshot("h", McpServerStatus::Connected, 1);
    header.auth = Some(McpAuthState::Header);
    header.kept_client = Some("typed-cid".to_string());
    assert_eq!(
        server_actions(&header),
        [A::ViewTools, A::Reconnect, A::Disable]
    );
    // A server that is off, disabled or connecting offers none of them.
    for status in [
        McpServerStatus::Disabled,
        McpServerStatus::Off,
        McpServerStatus::Pending,
    ] {
        let mut server = snapshot("l", status, 0);
        server.kept_client = Some("typed-cid".to_string());
        let actions = server_actions(&server);
        assert!(
            !actions.contains(&A::ChangeClient) && !actions.contains(&A::ForgetClient),
            "{actions:?}"
        );
    }
}

/// The client rows hand the boundary their ops; the form the server's page
/// opens comes back from it, over the client on hand.
#[test]
fn the_client_rows_dispatch_their_ops_and_open_the_form() {
    let mut app = App::new();
    let mut linear = snapshot("linear", McpServerStatus::NeedsAuth, 0);
    linear.kept_client = Some("typed-cid".to_string());
    app.open_mcp_menu(vec![linear]);
    app.on_key(key(KeyCode::Enter)); // linear's page
    // 2. Change OAuth client
    assert_eq!(
        app.on_key(key(KeyCode::Char('2'))),
        Action::McpOp(McpOp::EditAuthClient {
            server: "linear".to_string()
        })
    );
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Server);
    let request = McpClientRequest {
        client_id: Some("typed-cid".to_string()),
        has_secret: true,
        callback_port: Some(9001),
        ..client_request()
    };
    assert!(
        !app.edit_mcp_client("deepwiki", &request),
        "another server's"
    );
    assert!(app.edit_mcp_client("linear", &request));
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Auth);
    assert_eq!(form(&app).client_id, "typed-cid");
    assert_eq!(form(&app).port, "9001");
    // Back out, and forget it instead: 3. Forget OAuth client.
    app.on_key(key(KeyCode::Esc));
    assert_eq!(
        app.on_key(key(KeyCode::Char('3'))),
        Action::McpOp(McpOp::ForgetAuthClient {
            server: "linear".to_string()
        })
    );
    // The form opens only from the server's own page.
    let mut app = auth_page_app();
    assert!(!app.edit_mcp_client("linear", &request));
}
