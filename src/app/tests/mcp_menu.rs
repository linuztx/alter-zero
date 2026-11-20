//! The `/mcp` manager's state machine (`docs/mcp.md`).

use super::*;
use crate::app::{McpClientField, McpClientForm, McpOp, McpPage, McpServerAction, server_actions};
use crate::mcp::{
    ClientNeed, ClientRequest, ClientSource, EnteredClient, McpAuthState, McpClientInfo, McpScope,
    McpServerConfig, McpServerSnapshot, McpServerStatus, McpToolInfo, SUGGESTED_CALLBACK_PORT,
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
        client: None,
        identity: None,
        excluded: Vec::new(),
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
    // Wherever a sign-in can be made, the client to make it as can be
    // given: the spec's "prompt the user", one row under the sign-in.
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
    // The finished flow closes the page back to the server detail — and
    // keeps nothing on it: the outcome is the boundary's toast.
    assert!(!app.finish_mcp_auth("linear", None));
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
    app.finish_mcp_auth("deepwiki", None);
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Auth);
    app.finish_mcp_auth("linear", None);
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

// ---------------------------------------------------------------------------
// the OAuth client form (`docs/mcp.md` *Pre-registered clients*)

const ISSUER: &str = "https://accounts.google.com";

fn request(need: ClientNeed) -> ClientRequest {
    ClientRequest {
        issuer: ISSUER.to_string(),
        need,
        ..Default::default()
    }
}

/// The menu on linear's auth page — Authenticate just pressed.
fn on_auth_page() -> App {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // Authenticate
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Auth);
    app
}

/// [`on_auth_page`], the sign-in having asked for a client.
fn on_client_form(request: ClientRequest) -> App {
    let mut app = on_auth_page();
    assert!(app.ask_mcp_client("linear", &request));
    app
}

fn form(app: &App) -> &McpClientForm {
    app.mcp_menu
        .as_ref()
        .and_then(|menu| menu.auth.client.as_ref())
        .expect("the client form")
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        assert_eq!(app.on_key(key(KeyCode::Char(c))), Action::None);
    }
}

fn with_client(source: ClientSource) -> McpServerSnapshot {
    let mut server = snapshot("l", McpServerStatus::NeedsAuth, 0);
    server.client = Some(McpClientInfo {
        client_id: "cid".to_string(),
        source,
    });
    server
}

/// A client on hand can be changed; one entered here can be forgotten —
/// an entry's own is its file's to remove.
#[test]
fn a_server_with_a_client_offers_to_change_it_and_to_forget_one_entered_here() {
    use McpServerAction as A;
    assert_eq!(
        server_actions(&with_client(ClientSource::Config)),
        [A::Authenticate, A::ChangeClient, A::Disable]
    );
    assert_eq!(
        server_actions(&with_client(ClientSource::Entered)),
        [
            A::Authenticate,
            A::ChangeClient,
            A::ForgetClient,
            A::Disable
        ]
    );
    assert_eq!(A::SetClient.label(), "Set OAuth client");
    assert_eq!(A::ChangeClient.label(), "Change OAuth client");
    assert_eq!(A::ForgetClient.label(), "Forget OAuth client");
    // A server that asks for no sign-in has no client to give.
    assert!(!server_actions(&snapshot("d", McpServerStatus::Connected, 1)).contains(&A::SetClient));
}

/// `Set OAuth client` opens the auth page **asking**: nothing is signed in
/// to, the manager only finds the authorization server — and Esc, while it
/// looks, cancels the lookup.
#[test]
fn set_oauth_client_opens_the_auth_page_asking() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // linear: [Authenticate, Set OAuth client, Disable]
    let action = app.on_key(key(KeyCode::Char('2')));
    assert_eq!(
        action,
        Action::McpOp(McpOp::AskAuthClient {
            server: "linear".to_string()
        })
    );
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Auth);
    assert!(menu.auth.asking);
    assert!(menu.auth.client.is_none());
    // No redirect field to type into while it looks.
    type_text(&mut app, "c");
    assert_eq!(app.mcp_menu.as_ref().unwrap().auth.input, "");
    assert!(!app.paste_into_mcp_auth("https://x/cb?code=1"));
    assert_eq!(
        app.on_key(key(KeyCode::Esc)),
        Action::McpOp(McpOp::CancelAuth)
    );
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Server);
    // The form arrives in the asking page's place.
    app.on_key(key(KeyCode::Char('2')));
    assert!(app.ask_mcp_client("linear", &request(ClientNeed::Asked)));
    let menu = app.mcp_menu.as_ref().unwrap();
    assert!(menu.auth.client.is_some());
    assert!(!menu.auth.asking);
}

/// A sign-in with no client to sign in as turns its own auth page into the
/// form — and only its own: a request for a page no longer open, or another
/// server's, is the boundary's to say in a toast.
#[test]
fn a_sign_in_that_needs_a_client_turns_its_auth_page_into_the_form() {
    let mut app = on_auth_page();
    assert!(!app.ask_mcp_client("deepwiki", &request(ClientNeed::NoRegistration)));
    assert!(app.mcp_menu.as_ref().unwrap().auth.client.is_none());
    assert!(app.ask_mcp_client("linear", &request(ClientNeed::NoRegistration)));
    let form = form(&app);
    assert_eq!(form.request.issuer, ISSUER);
    assert_eq!(form.focus, McpClientField::ClientId);
    assert!(form.client_id.is_empty() && form.secret.is_empty());
    // The page was left: nothing to turn into a form.
    app.on_key(key(KeyCode::Esc));
    assert!(!app.ask_mcp_client("linear", &request(ClientNeed::NoRegistration)));
    let mut closed = App::new();
    assert!(!closed.ask_mcp_client("linear", &request(ClientNeed::NoRegistration)));
}

/// Enter moves on through the fields and signs in from the last one: the
/// client goes to the boundary whole — bound to the authorization server it
/// was asked for — and the page is the sign-in's again.
#[test]
fn the_form_walks_its_fields_and_hands_over_the_client() {
    let mut app = on_client_form(request(ClientNeed::NoRegistration));
    type_text(&mut app, "cid.apps.test");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(form(&app).focus, McpClientField::Secret);
    type_text(&mut app, "s3cret");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(form(&app).focus, McpClientField::Port);
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Action::McpOp(McpOp::SaveAuthClient {
            server: "linear".to_string(),
            client: EnteredClient {
                issuer: ISSUER.to_string(),
                client_id: "cid.apps.test".to_string(),
                client_secret: Some(SecretValue::new("s3cret")),
                keep_secret: false,
                // The port the form opened on, left as it was.
                callback_port: Some(SUGGESTED_CALLBACK_PORT),
            },
        })
    );
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Auth, "the sign-in starts on this page");
    assert!(menu.auth.client.is_none() && !menu.auth.asking);
    // A secret left empty is a public client; a port typed is the port.
    let mut app = on_client_form(request(ClientNeed::NoRegistration));
    type_text(&mut app, "public-cid");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));
    app.on_key(ctrl('u'));
    type_text(&mut app, "7000");
    let Action::McpOp(McpOp::SaveAuthClient { client, .. }) = app.on_key(key(KeyCode::Enter))
    else {
        panic!("the form did not submit");
    };
    assert_eq!(client.client_secret, None);
    assert!(!client.keep_secret);
    assert_eq!(client.callback_port, Some(7000));
}

/// A form for a **new** client opens on a callback port: the redirect URI
/// it shows is then one that can be registered as it stands, where a port
/// left to chance fails in the browser, at a provider that matches redirect
/// URIs exactly, with nothing here to say why. It is a suggestion in an
/// ordinary field — typed over, or emptied for any free port — and only
/// where nothing says otherwise: a port already in effect is the one shown,
/// and a client on hand keeps the port it has, none included.
#[test]
fn a_form_for_a_new_client_suggests_a_callback_port() {
    assert_eq!(SUGGESTED_CALLBACK_PORT, 8976);
    for need in [
        ClientNeed::NoRegistration,
        ClientNeed::RegistrationRefused("HTTP 403".to_string()),
        ClientNeed::Asked,
    ] {
        let app = on_client_form(request(need.clone()));
        assert_eq!(form(&app).port.text(), "8976", "{need:?}");
    }
    // The entry names a port — one the user may have had to allow-list.
    let app = on_client_form(ClientRequest {
        callback_port: Some(8080),
        ..request(ClientNeed::NoRegistration)
    });
    assert_eq!(form(&app).port.text(), "8080");
    // A client on hand with no port signs in on any free one: a number
    // filled in under it would change that at the next Enter.
    for need in [ClientNeed::Asked, ClientNeed::Rejected("no".to_string())] {
        let app = on_client_form(ClientRequest {
            client_id: Some("cid".to_string()),
            ..request(need)
        });
        assert!(form(&app).port.is_empty());
    }
    // Emptied, it is any free port.
    let mut app = on_client_form(request(ClientNeed::NoRegistration));
    type_text(&mut app, "cid");
    app.on_key(key(KeyCode::Up)); // ClientId → Port, wrapping
    app.on_key(ctrl('u'));
    let Action::McpOp(McpOp::SaveAuthClient { client, .. }) = app.on_key(key(KeyCode::Enter))
    else {
        panic!("the form did not submit");
    };
    assert_eq!(client.callback_port, None);
}

/// A form the sign-in could not use never leaves: the reason shows under
/// the fields and the focus goes to the one at fault.
#[test]
fn the_form_refuses_a_missing_id_and_a_port_that_is_none() {
    let mut app = on_client_form(request(ClientNeed::NoRegistration));
    app.on_key(key(KeyCode::Enter)); // → secret
    app.on_key(key(KeyCode::Enter)); // → port
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(form(&app).focus, McpClientField::ClientId);
    assert!(form(&app).error.as_deref().unwrap().contains("client ID"));
    // Editing takes the complaint back.
    type_text(&mut app, "  cid  ");
    assert_eq!(form(&app).error, None);
    app.on_key(key(KeyCode::Up)); // ClientId → Port, wrapping
    assert_eq!(form(&app).focus, McpClientField::Port);
    app.on_key(ctrl('u'));
    type_text(&mut app, "70000");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(form(&app).focus, McpClientField::Port);
    assert!(form(&app).error.as_deref().unwrap().contains("65535"));
    app.on_key(key(KeyCode::Backspace));
    let Action::McpOp(McpOp::SaveAuthClient { client, .. }) = app.on_key(key(KeyCode::Enter))
    else {
        panic!("the form did not submit");
    };
    assert_eq!(client.client_id, "cid", "the id is trimmed");
    assert_eq!(client.callback_port, Some(7000));
}

/// Tab/↓ and Shift+Tab/↑ move, wrapping; the id is a real text field —
/// the caret moves through it — the port takes digits only, Ctrl+U empties
/// the field, and `c` is a letter here, not the auth page's copy.
#[test]
fn the_forms_keys_move_between_its_fields_and_edit_them() {
    let mut app = on_client_form(request(ClientNeed::NoRegistration));
    for (press, lands) in [
        (KeyCode::Tab, McpClientField::Secret),
        (KeyCode::Down, McpClientField::Port),
        (KeyCode::Tab, McpClientField::ClientId),
        (KeyCode::BackTab, McpClientField::Port),
        (KeyCode::Up, McpClientField::Secret),
        (KeyCode::BackTab, McpClientField::ClientId),
    ] {
        assert_eq!(app.on_key(key(press)), Action::None);
        assert_eq!(form(&app).focus, lands, "{press:?}");
    }
    type_text(&mut app, "acd");
    app.on_key(key(KeyCode::Home));
    app.on_key(key(KeyCode::Right));
    type_text(&mut app, "b");
    assert_eq!(form(&app).client_id.text(), "abcd");
    app.on_key(key(KeyCode::End));
    app.on_key(key(KeyCode::Backspace));
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Delete));
    assert_eq!(form(&app).client_id.text(), "ab");
    assert_eq!(
        app.mcp_menu.as_ref().unwrap().auth.input,
        "",
        "nothing leaks into the redirect field behind the form"
    );
    app.on_key(key(KeyCode::Tab));
    type_text(&mut app, "xyz");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(form(&app).secret.expose(), "xy");
    // A mask has nothing to move through: the arrows leave it alone.
    app.on_key(key(KeyCode::Left));
    type_text(&mut app, "!");
    assert_eq!(form(&app).secret.expose(), "xy!");
    app.on_key(ctrl('u'));
    assert!(form(&app).secret.is_empty());
    app.on_key(key(KeyCode::Tab));
    app.on_key(ctrl('u'));
    type_text(&mut app, "8a9-7 6x");
    assert_eq!(form(&app).port.text(), "8976");
    type_text(&mut app, "54321");
    assert_eq!(
        form(&app).port.text(),
        "89765",
        "a port has five digits at most"
    );
    app.on_key(ctrl('u'));
    assert!(form(&app).port.is_empty());
}

/// A form over a client already on hand starts from it — its id, its port,
/// the focus on the secret — and a secret left empty keeps the one kept for
/// it (a provider may show a secret only once), unless the id was changed:
/// that secret is another client's.
#[test]
fn a_form_over_a_client_on_hand_starts_from_it_and_keeps_its_secret() {
    let on_hand = ClientRequest {
        client_id: Some("cid".to_string()),
        has_secret: true,
        callback_port: Some(8976),
        ..request(ClientNeed::Asked)
    };
    let mut app = on_client_form(on_hand.clone());
    let form_now = form(&app);
    assert_eq!(form_now.client_id.text(), "cid");
    assert_eq!(form_now.port.text(), "8976");
    assert_eq!(form_now.focus, McpClientField::Secret);
    assert!(
        form_now.secret.is_empty(),
        "a kept secret is never loaded back"
    );
    assert!(form_now.keeps_secret());
    app.on_key(key(KeyCode::Enter)); // → port
    let Action::McpOp(McpOp::SaveAuthClient { client, .. }) = app.on_key(key(KeyCode::Enter))
    else {
        panic!("the form did not submit");
    };
    assert_eq!(client.client_secret, None);
    assert!(client.keep_secret);
    assert_eq!(client.callback_port, Some(8976));
    // Another id: its secret is not the kept one's.
    let mut app = on_client_form(on_hand.clone());
    app.on_key(key(KeyCode::Up));
    type_text(&mut app, "2");
    assert_eq!(form(&app).client_id.text(), "cid2");
    assert!(!form(&app).keeps_secret());
    // A secret typed replaces the kept one.
    let mut app = on_client_form(on_hand);
    type_text(&mut app, "new");
    assert!(!form(&app).keeps_secret());
}

/// A paste goes to the focused field, never to the redirect field behind
/// the form: the id on one line, the secret whole, the port its digits.
#[test]
fn a_paste_goes_to_the_focused_field() {
    let mut app = on_client_form(request(ClientNeed::NoRegistration));
    assert!(app.paste_into_mcp_auth("  cid.apps.test\n"));
    assert_eq!(form(&app).client_id.text(), "cid.apps.test");
    app.on_key(key(KeyCode::Tab));
    assert!(app.paste_into_mcp_auth("GOCSPX-s3cr3t\r\n"));
    assert_eq!(form(&app).secret.expose(), "GOCSPX-s3cr3t");
    app.on_key(key(KeyCode::Tab));
    // A pasted port is the whole port: it takes the place of the one the
    // form opened on, rather than running into it.
    assert_eq!(form(&app).port.text(), "8976");
    assert!(app.paste_into_mcp_auth("port 9000\n"));
    assert_eq!(form(&app).port.text(), "9000");
    // A paste with no port in it leaves the field alone.
    assert!(app.paste_into_mcp_auth("localhost"));
    assert_eq!(form(&app).port.text(), "9000");
    assert_eq!(app.mcp_menu.as_ref().unwrap().auth.input, "");
    // A paste is an edit: it takes a complaint back.
    let mut app = on_client_form(request(ClientNeed::NoRegistration));
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Enter));
    assert!(form(&app).error.is_some());
    app.paste_into_mcp_auth("cid");
    assert_eq!(form(&app).error, None);
}

/// The secret is in the menu's state and in the action the boundary is
/// handed — and in neither's `Debug` print.
#[test]
fn the_secret_never_reaches_a_debug_print() {
    let mut app = on_client_form(request(ClientNeed::NoRegistration));
    type_text(&mut app, "cid");
    app.on_key(key(KeyCode::Tab));
    type_text(&mut app, "hunter2");
    let printed = format!("{:?}", app.mcp_menu);
    assert!(!printed.contains("hunter2"), "{printed}");
    app.on_key(key(KeyCode::Tab));
    let action = app.on_key(key(KeyCode::Enter));
    assert!(matches!(
        action,
        Action::McpOp(McpOp::SaveAuthClient { .. })
    ));
    assert!(!format!("{action:?}").contains("hunter2"));
}

/// Esc on the form goes back to the server's page and cancels nothing — the
/// sign-in that asked has already ended — and the draft goes with the page.
#[test]
fn esc_on_the_form_goes_back_and_forgets_the_draft() {
    let mut app = on_client_form(request(ClientNeed::NoRegistration));
    type_text(&mut app, "cid");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Server);
    assert!(menu.auth.client.is_none());
    // Ctrl+C closes the whole menu from the form, as from anywhere.
    let mut app = on_client_form(request(ClientNeed::NoRegistration));
    app.on_key(ctrl('c'));
    assert!(app.mcp_menu.is_none());
}

/// A fresh snapshot landing under the form — a server reconnecting — leaves
/// what was typed alone; and a sign-in that asks again (its client turned
/// away) reopens the form on the client it tried.
#[test]
fn the_form_survives_a_snapshot_and_reopens_on_a_client_turned_away() {
    let mut app = on_client_form(request(ClientNeed::NoRegistration));
    type_text(&mut app, "cid");
    app.update_mcp_snapshot(vec![
        snapshot("deepwiki", McpServerStatus::Connected, 3),
        snapshot("linear", McpServerStatus::NeedsAuth, 0),
        snapshot("github", McpServerStatus::Disabled, 0),
    ]);
    assert_eq!(form(&app).client_id.text(), "cid");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // submitted: the sign-in runs
    let turned_away = ClientRequest {
        client_id: Some("cid".to_string()),
        ..request(ClientNeed::Rejected("HTTP 401: invalid_client".to_string()))
    };
    assert!(app.ask_mcp_client("linear", &turned_away));
    let form = form(&app);
    assert_eq!(form.client_id.text(), "cid");
    assert_eq!(
        form.request.need,
        ClientNeed::Rejected("HTTP 401: invalid_client".to_string())
    );
    assert_eq!(form.focus, McpClientField::Secret);
}

#[test]
fn forget_oauth_client_dispatches_with_the_server_name() {
    let mut app = App::new();
    app.open_mcp_menu(vec![with_client(ClientSource::Entered)]);
    app.on_key(key(KeyCode::Enter)); // [Authenticate, Change, Forget, Disable]
    assert_eq!(
        app.on_key(key(KeyCode::Char('3'))),
        Action::McpOp(McpOp::ForgetClient {
            server: "l".to_string()
        })
    );
    assert_eq!(
        app.mcp_menu.as_ref().unwrap().page,
        McpPage::Server,
        "forgetting is done from the page it is asked on"
    );
    // `Change OAuth client` asks exactly as `Set OAuth client` does.
    assert_eq!(
        app.on_key(key(KeyCode::Char('2'))),
        Action::McpOp(McpOp::AskAuthClient {
            server: "l".to_string()
        })
    );
}

// ---------------------------------------------------------------------------
// a failed sign-in (`docs/mcp.md` *A sign-in that failed*)

const REFUSAL: &str = "token exchange failed: HTTP 400: redirect_uri_mismatch: The redirect URI \
    in the request did not match a registered redirect URI.";

/// A failed sign-in **stays on its page** — the reason whole, where a
/// one-row toast cut it at the terminal's width and closed the page that
/// explained it — and the page is a reason to read, not a field: nothing
/// types into it, Enter tries again, Esc goes back.
#[test]
fn a_failed_sign_in_keeps_its_reason_on_the_page() {
    let mut app = on_auth_page();
    app.set_mcp_auth_url("linear", "https://as.test/authorize?x=1");
    assert!(
        app.finish_mcp_auth("linear", Some(REFUSAL)),
        "the page took it"
    );
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Auth);
    assert_eq!(menu.auth.failure.as_deref(), Some(REFUSAL));
    assert_eq!(menu.auth.url, None, "the dead sign-in's URL is gone");
    // No field behind the reason: keys and pastes land nowhere, and `c` has
    // no URL to copy.
    type_text(&mut app, "c x");
    assert!(!app.paste_into_mcp_auth("https://x/cb?code=1"));
    let auth = &app.mcp_menu.as_ref().unwrap().auth;
    assert_eq!(auth.input, "");
    assert!(auth.failure.is_some());
    // Enter tries again: the same sign-in, on a page that is waiting anew.
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::Authenticate {
            server: "linear".to_string()
        })
    );
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Auth);
    assert_eq!(menu.auth.failure, None);
    // Esc goes back, and cancels nothing: the sign-in has already ended.
    app.finish_mcp_auth("linear", Some(REFUSAL));
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    let menu = app.mcp_menu.as_ref().unwrap();
    assert_eq!(menu.page, McpPage::Server);
    assert_eq!(menu.auth.failure, None);
}

/// A failure only its own open page can take: another server's, or one
/// whose page the user left, is the boundary's to say in a toast.
#[test]
fn a_failure_no_open_page_takes_is_left_to_the_toast() {
    let mut app = on_auth_page();
    assert!(!app.finish_mcp_auth("deepwiki", Some(REFUSAL)));
    assert_eq!(app.mcp_menu.as_ref().unwrap().auth.failure, None);
    app.on_key(key(KeyCode::Esc));
    assert!(!app.finish_mcp_auth("linear", Some(REFUSAL)));
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Server);
    let mut closed = App::new();
    assert!(!closed.finish_mcp_auth("linear", Some(REFUSAL)));
}

/// The form's own sign-in failing — a client that could not be kept, a
/// callback port already taken — lands on the same page, and the retry is
/// the sign-in again, with the client that was kept.
#[test]
fn a_failure_after_the_form_shows_on_its_page() {
    let mut app = on_client_form(request(ClientNeed::NoRegistration));
    type_text(&mut app, "cid");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::SaveAuthClient { .. })
    ));
    assert!(app.finish_mcp_auth("linear", Some("the callback port 8976 isn't free")));
    let menu = app.mcp_menu.as_ref().unwrap();
    assert!(menu.auth.failure.is_some() && menu.auth.client.is_none());
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::Authenticate {
            server: "linear".to_string()
        })
    );
}

/// A lookup that failed — `Set OAuth client` could not find where the server
/// signs in — is tried again **as a lookup**: Enter repeats what failed,
/// never a sign-in the user did not ask for.
#[test]
fn a_failed_lookup_is_tried_again_as_a_lookup() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('2'))); // Set OAuth client
    assert!(app.finish_mcp_auth(
        "linear",
        Some("could not discover the authorization server")
    ));
    assert!(app.mcp_menu.as_ref().unwrap().auth.failure.is_some());
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::McpOp(McpOp::AskAuthClient {
            server: "linear".to_string()
        })
    );
    let menu = app.mcp_menu.as_ref().unwrap();
    assert!(menu.auth.asking, "the page is looking again");
    assert_eq!(menu.auth.failure, None);
}
