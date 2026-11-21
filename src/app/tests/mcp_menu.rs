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
            A::SetClient,
            A::Reconnect,
            A::Disable
        ]
    );
    assert_eq!(
        server_actions(&snapshot("l", McpServerStatus::NeedsAuth, 0)),
        [A::Authenticate, A::SetClient, A::Disable]
    );
    // A grant the server is refusing must be clearable from the very page
    // that reports it — it used to be removable only by editing the store.
    let mut expired = snapshot("l", McpServerStatus::NeedsAuth, 0);
    expired.auth = Some(McpAuthState::Expired);
    assert_eq!(
        server_actions(&expired),
        [A::Authenticate, A::ClearAuth, A::SetClient, A::Disable]
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
        [A::Authenticate, A::SetClient, A::Reconnect, A::Disable]
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
        documentation: None,
        reason: None,
        entry: None,
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
    assert_eq!(opened.port.text(), "8976");
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
            keep_secret: false,
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
    assert_eq!(form(&app).port.text(), "");
    type_text(&mut app, "9x0-01 23");
    assert_eq!(form(&app).port.text(), "90012");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(form(&app).port.text(), "9001");
    // Enter on a field that is not the last moves on.
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(form(&app).field, ClientId);
    type_text(&mut app, "abc");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(form(&app).client_id.text(), "ab");
    app.on_key(key(KeyCode::Tab));
    type_text(&mut app, "pw");
    assert_eq!(form(&app).secret, SecretValue::new("pw"));
    app.on_key(ctrl('u'));
    assert!(form(&app).secret.is_empty());
    assert_eq!(form(&app).client_id.text(), "ab", "only the focused field");
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
            keep_secret: false,
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
            keep_secret: false,
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
            keep_secret: true,
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
    assert_eq!(form(&app).client_id.text(), "cid");
}

/// A form that is back because the client was turned away opens with what
/// was entered and the reason, the caret on the secret — and no secret
/// offered as kept, the refused one included: Enter with the field empty
/// saves the client with none, a public client.
#[test]
fn a_form_reopened_with_a_reason_keeps_what_was_entered() {
    let request = McpClientRequest {
        client_id: Some("typed-cid".to_string()),
        has_secret: false,
        callback_port: Some(9001),
        reason: Some("as.test turned this client away (HTTP 401: invalid_client).".to_string()),
        ..client_request()
    };
    let mut app = form_app(&request);
    let reopened = form(&app);
    assert_eq!(reopened.client_id.text(), "typed-cid");
    assert_eq!(reopened.port.text(), "9001");
    assert_eq!(reopened.kept_secret_for, None);
    assert!(!reopened.keeps_secret());
    assert_eq!(reopened.reason, request.reason);
    assert_eq!(reopened.field, McpClientField::Secret);
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::SaveAuthClient {
            server: "linear".to_string(),
            issuer: "https://accounts.google.com".to_string(),
            client_id: "typed-cid".to_string(),
            client_secret: None,
            keep_secret: false,
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
    assert_eq!(form(&app).client_id.text(), "y");
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
    assert!(app.paste_into_mcp_auth("  123.apps\r\n.example\t \n"));
    assert_eq!(form(&app).client_id.text(), "123.apps.example");
    // At the caret, not at the end.
    app.on_key(key(KeyCode::Home));
    assert!(app.paste_into_mcp_auth("x-"));
    assert_eq!(form(&app).client_id.text(), "x-123.apps.example");
    // A pasted port replaces the field's value: the form opened on 8976,
    // and 12798 pasted is 12798, not 89761. One with no digits leaves it.
    app.on_key(key(KeyCode::Up)); // the port
    assert_eq!(form(&app).port.text(), "8976");
    assert!(app.paste_into_mcp_auth(" 12798\n"));
    assert_eq!(form(&app).port.text(), "12798");
    assert!(app.paste_into_mcp_auth(" 80 80\n"));
    assert_eq!(form(&app).port.text(), "8080");
    assert!(app.paste_into_mcp_auth("port"));
    assert_eq!(form(&app).port.text(), "8080");
    app.on_key(key(KeyCode::BackTab)); // the secret
    assert!(app.paste_into_mcp_auth("GOCSPX-abc\n"));
    assert!(app.paste_into_mcp_auth("\u{7}def"));
    assert_eq!(form(&app).secret, SecretValue::new("GOCSPX-abcdef"));
    assert_eq!(
        app.mcp_menu.as_ref().unwrap().auth.input,
        "",
        "never the URL field behind the form"
    );
}

/// The id and the port are edited at the caret: ←/→ move it, Home and End
/// go to the ends, Delete takes the character under it and Backspace the
/// one before. The secret has no caret to move: it takes keys at its end.
#[test]
fn the_id_and_port_fields_edit_at_the_caret() {
    let mut app = form_app(&client_request());
    type_text(&mut app, "acd");
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    type_text(&mut app, "b");
    assert_eq!(form(&app).client_id.text(), "abcd");
    app.on_key(key(KeyCode::Delete));
    assert_eq!(form(&app).client_id.text(), "abd");
    app.on_key(key(KeyCode::Home));
    type_text(&mut app, "<");
    app.on_key(key(KeyCode::End));
    type_text(&mut app, ">");
    assert_eq!(form(&app).client_id.text(), "<abd>");
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(form(&app).client_id.text(), "<ab>");
    assert_eq!(form(&app).client_id.cursor(), 3);
    app.on_key(key(KeyCode::Right));
    assert_eq!(form(&app).client_id.cursor(), 4);
    // The port: digits, at the caret.
    app.on_key(key(KeyCode::Up)); // the port: 8976
    app.on_key(key(KeyCode::Home));
    app.on_key(key(KeyCode::Delete));
    type_text(&mut app, "7");
    assert_eq!(form(&app).port.text(), "7976");
    // The secret: nothing to move through.
    app.on_key(key(KeyCode::BackTab)); // the secret
    type_text(&mut app, "pw");
    for press in [KeyCode::Left, KeyCode::Home, KeyCode::Delete] {
        app.on_key(key(press));
    }
    type_text(&mut app, "!");
    assert_eq!(form(&app).secret, SecretValue::new("pw!"));
}

/// Each field takes so much and no more, typed or pasted: an id 512
/// characters, a secret 4096, a port five digits.
#[test]
fn the_client_form_fields_have_their_limits() {
    let mut app = form_app(&client_request());
    assert!(app.paste_into_mcp_auth(&"i".repeat(600)));
    assert_eq!(form(&app).client_id.text().chars().count(), 512);
    type_text(&mut app, "x");
    assert_eq!(form(&app).client_id.text().chars().count(), 512);
    app.on_key(key(KeyCode::Up)); // the port
    app.on_key(ctrl('u'));
    type_text(&mut app, "1234567");
    assert_eq!(form(&app).port.text(), "12345");
    assert!(app.paste_into_mcp_auth("9876543"));
    assert_eq!(form(&app).port.text(), "98765");
    app.on_key(key(KeyCode::BackTab)); // the secret
    assert!(app.paste_into_mcp_auth(&"s".repeat(5000)));
    assert_eq!(form(&app).secret.char_count(), 4096);
    type_text(&mut app, "x");
    assert_eq!(form(&app).secret.char_count(), 4096);
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

/// linear's snapshot, its entry naming its own client — with the secret in
/// its file or not.
fn naming_its_client(secret: Option<&str>, port: Option<u16>) -> McpServerSnapshot {
    let mut server = snapshot("linear", McpServerStatus::NeedsAuth, 0);
    server.config = McpServerConfig::Http {
        url: "https://linear.test/mcp".to_string(),
        headers: Default::default(),
        sse_fallback: false,
        oauth: Some(OAuthClientConfig {
            client_id: Some("file-cid".to_string()),
            client_secret: secret.map(str::to_string),
            callback_port: port,
            ..Default::default()
        }),
    };
    server
}

/// The client rows, wherever a sign-in can be made: Set where no client is
/// in effect, Change where one is — entered here, or the entry's own, even
/// one whose file spells its secret — and Forget only for one entered here.
#[test]
fn server_actions_offer_the_client_rows_wherever_a_sign_in_can_be_made() {
    use McpServerAction as A;
    assert_eq!(
        server_actions(&snapshot("l", McpServerStatus::NeedsAuth, 0)),
        [A::Authenticate, A::SetClient, A::Disable]
    );
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
    for secret in [None, Some("${SECRET}")] {
        assert_eq!(
            server_actions(&naming_its_client(secret, None)),
            [A::Authenticate, A::ChangeClient, A::Disable]
        );
    }
    // One entered here outranks the entry's own, and can be forgotten.
    let mut both = naming_its_client(None, None);
    both.kept_client = Some("typed-cid".to_string());
    assert_eq!(
        server_actions(&both),
        [
            A::Authenticate,
            A::ChangeClient,
            A::ForgetClient,
            A::Disable
        ]
    );
    // No sign-in, no client rows: a written-in header, a server that let an
    // anonymous connect through, and one off, disabled or connecting.
    let mut header = snapshot("h", McpServerStatus::Connected, 1);
    header.auth = Some(McpAuthState::Header);
    header.kept_client = Some("typed-cid".to_string());
    assert_eq!(
        server_actions(&header),
        [A::ViewTools, A::Reconnect, A::Disable]
    );
    let public = snapshot("p", McpServerStatus::Connected, 1);
    assert_eq!(public.auth, Some(McpAuthState::NotRequired));
    assert_eq!(
        server_actions(&public),
        [A::ViewTools, A::Reconnect, A::Disable]
    );
    for status in [
        McpServerStatus::Disabled,
        McpServerStatus::Off,
        McpServerStatus::Pending,
    ] {
        let mut server = snapshot("l", status, 0);
        server.kept_client = Some("typed-cid".to_string());
        let actions = server_actions(&server);
        assert!(
            !actions.contains(&A::SetClient)
                && !actions.contains(&A::ChangeClient)
                && !actions.contains(&A::ForgetClient),
            "{actions:?}"
        );
    }
}

/// What the lookup answers for an entry naming its own client: the whole
/// form, prefilled with its id and its exact port, knowing that client.
fn entry_client_request(
    port: Option<u16>,
    redirect: Option<&str>,
    has_secret: bool,
) -> McpClientRequest {
    McpClientRequest {
        client_id: Some("file-cid".to_string()),
        has_secret,
        callback_port: port,
        entry: Some(crate::mcp::McpEntryClient {
            client_id: "file-cid".to_string(),
            port,
            redirect_uri: redirect.map(str::to_string),
            has_secret,
        }),
        ..client_request()
    }
}

/// Change OAuth client over the entry's own client opens the whole form on
/// it, and the form tells from what is typed which client it describes: the
/// entry's id on the entry's port is the entry's own — the redirect URI its
/// entry registered, and a submit that changes nothing of it — and anything
/// else is a client entered here, on `http://localhost:{port}/callback`.
#[test]
fn the_form_over_the_entrys_own_client_knows_when_it_still_describes_it() {
    let registered = "http://127.0.0.1:12799/callback/abc";
    let mut app = form_app(&entry_client_request(Some(12799), Some(registered), true));
    assert_eq!(form(&app).client_id.text(), "file-cid");
    assert_eq!(form(&app).port.text(), "12799");
    assert_eq!(form(&app).fixed_client_id, None, "the whole form");
    assert!(form(&app).describes_entry_client());
    assert_eq!(form(&app).redirect_uri().as_deref(), Some(registered));
    assert!(
        form(&app).keeps_secret(),
        "the file's secret stays in effect"
    );
    // Enter through the unedited form: the entry's own client, on its port,
    // its secret left as it is.
    app.on_key(key(KeyCode::Up)); // the id
    app.on_key(key(KeyCode::Up)); // the port
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::SaveAuthClient {
            server: "linear".to_string(),
            issuer: "https://accounts.google.com".to_string(),
            client_id: "file-cid".to_string(),
            client_secret: None,
            keep_secret: true,
            callback_port: Some(12799),
        })
    );
    // Another port: a client entered here, on its own redirect URI.
    let mut app = form_app(&entry_client_request(Some(12799), Some(registered), true));
    app.on_key(key(KeyCode::Up)); // the id
    app.on_key(key(KeyCode::Up)); // the port
    app.on_key(ctrl('u'));
    type_text(&mut app, "9000");
    assert!(!form(&app).describes_entry_client());
    assert_eq!(
        form(&app).redirect_uri().as_deref(),
        Some("http://localhost:9000/callback")
    );
    // The same id: its secret left as it is still means the one in effect.
    assert!(form(&app).keeps_secret());
    // Back to the entry's port, the entry's own again.
    app.on_key(ctrl('u'));
    type_text(&mut app, "12799");
    assert_eq!(form(&app).redirect_uri().as_deref(), Some(registered));
    // Another id on the entry's port: entered here, and nothing kept for it.
    app.on_key(key(KeyCode::Down)); // the id
    app.on_key(ctrl('u'));
    type_text(&mut app, "my-cid");
    assert!(!form(&app).describes_entry_client());
    assert!(!form(&app).keeps_secret());
    assert_eq!(
        form(&app).redirect_uri().as_deref(),
        Some("http://localhost:12799/callback")
    );
}

/// An entry whose own client takes any free port: the form opens with no
/// port, and the empty port field is the entry's own client — no address to
/// show, nothing to copy, and a port needed for nothing. A port typed makes
/// a client entered here, which needs one.
#[test]
fn an_entry_on_any_free_port_needs_no_port_on_the_form() {
    let mut app = form_app(&entry_client_request(None, None, false));
    assert_eq!(form(&app).port.text(), "");
    assert!(form(&app).describes_entry_client());
    assert_eq!(form(&app).redirect_uri(), None);
    assert_eq!(app.on_key(ctrl('y')), Action::None);
    assert!(!form(&app).keeps_secret(), "no secret is in effect for it");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::SaveAuthClient {
            server: "linear".to_string(),
            issuer: "https://accounts.google.com".to_string(),
            client_id: "file-cid".to_string(),
            client_secret: None,
            keep_secret: false,
            callback_port: None,
        })
    );
    // Another id needs a port.
    let mut app = form_app(&entry_client_request(None, None, false));
    app.on_key(key(KeyCode::Up)); // the id
    app.on_key(ctrl('u'));
    type_text(&mut app, "my-cid");
    app.on_key(key(KeyCode::Down)); // the secret
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(form(&app).field, McpClientField::Port);
    assert!(
        form(&app)
            .error
            .as_deref()
            .is_some_and(|e| e.contains("1 to 65535"))
    );
}

/// An empty secret field keeps a kept secret only over the very client the
/// form opened with: a client of another id has none — a public client.
#[test]
fn a_kept_secret_stays_only_with_its_own_client() {
    let request = McpClientRequest {
        client_id: Some("typed-cid".to_string()),
        has_secret: true,
        callback_port: Some(9001),
        ..client_request()
    };
    let mut app = form_app(&request);
    assert!(form(&app).keeps_secret());
    app.on_key(key(KeyCode::Up)); // the id
    app.on_key(key(KeyCode::Backspace));
    assert!(!form(&app).keeps_secret());
    type_text(&mut app, "d");
    assert!(form(&app).keeps_secret());
    type_text(&mut app, "2");
    app.on_key(key(KeyCode::Down)); // the secret
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::SaveAuthClient {
            server: "linear".to_string(),
            issuer: "https://accounts.google.com".to_string(),
            client_id: "typed-cid2".to_string(),
            client_secret: None,
            keep_secret: false,
            callback_port: Some(9001),
        })
    );
}

/// linear's page, with a client entered for it.
fn entered_client_app() -> App {
    let mut app = App::new();
    let mut linear = snapshot("linear", McpServerStatus::NeedsAuth, 0);
    linear.kept_client = Some("typed-cid".to_string());
    app.open_mcp_menu(vec![linear]);
    app.on_key(key(KeyCode::Enter)); // linear's page
    app
}

/// Change OAuth client looks the server's authorization server up before
/// the form opens: the auth page waits on the lookup with nothing to type
/// into, and the form opens over its answer — the client in effect, under
/// the issuer and the documentation link the lookup found. Esc calls a
/// lookup off; Forget dispatches its own op.
#[test]
fn the_client_rows_look_up_the_authorization_server_before_the_form() {
    let mut app = entered_client_app();
    // 2. Change OAuth client
    assert_eq!(
        app.on_key(key(KeyCode::Char('2'))),
        Action::McpOp(McpOp::LookUpAuthClient {
            server: "linear".to_string()
        })
    );
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Auth);
    assert!(menu.auth.lookup);
    // Nothing to type into while it looks.
    type_text(&mut app, "x");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(!app.paste_into_mcp_auth("https://x.test/cb?code=1"));
    let auth = &app.mcp_menu.as_ref().unwrap().auth;
    assert!(auth.lookup && auth.input.is_empty() && auth.client.is_none());
    // The answer opens the form.
    let request = McpClientRequest {
        client_id: Some("typed-cid".to_string()),
        has_secret: true,
        callback_port: Some(9001),
        documentation: Some("https://accounts.google.com/docs".to_string()),
        ..client_request()
    };
    assert!(
        !app.ask_mcp_client("deepwiki", &request),
        "another server's"
    );
    assert!(app.ask_mcp_client("linear", &request));
    assert!(!app.mcp_menu.as_ref().unwrap().auth.lookup);
    assert_eq!(form(&app).client_id.text(), "typed-cid");
    assert_eq!(form(&app).port.text(), "9001");
    assert_eq!(form(&app).issuer, "https://accounts.google.com");
    assert_eq!(
        form(&app).documentation.as_deref(),
        Some("https://accounts.google.com/docs")
    );
    // Back out, look again, and call that lookup off.
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Char('2')));
    assert_eq!(
        app.on_key(key(KeyCode::Esc)),
        Action::McpOp(McpOp::CancelAuth)
    );
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Server);
    assert_eq!(menu.auth, Default::default());
    // 3. Forget OAuth client
    assert_eq!(
        app.on_key(key(KeyCode::Char('3'))),
        Action::McpOp(McpOp::ForgetAuthClient {
            server: "linear".to_string()
        })
    );
}

/// A lookup that fails lands on the failed page, as a sign-in does — and
/// its Enter repeats the lookup, never a sign-in, which would sign in to a
/// server that registers clients itself instead of opening the form.
#[test]
fn a_failed_lookup_is_tried_again_as_a_lookup() {
    let mut app = entered_client_app();
    app.on_key(key(KeyCode::Char('2'))); // Change OAuth client: the lookup
    let reason = "the authorization server http://127.0.0.1:1 couldn't be reached";
    assert!(app.finish_mcp_auth("linear", false, reason));
    let auth = &app.mcp_menu.as_ref().unwrap().auth;
    assert_eq!(auth.failed.as_deref(), Some(reason));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::LookUpAuthClient {
            server: "linear".to_string()
        })
    );
    let auth = &app.mcp_menu.as_ref().unwrap().auth;
    assert!(auth.lookup && auth.failed.is_none());
    // A failed sign-in still tries the sign-in again.
    let mut app = auth_page_app();
    assert!(app.finish_mcp_auth("linear", false, reason));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::Authenticate {
            server: "linear".to_string()
        })
    );
}
