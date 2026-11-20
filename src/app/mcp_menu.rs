//! The `/mcp` manager (`docs/mcp.md`) — the sixth composer-replacing inline
//! picker, and deliberately the hooks-menu twin: no text entry (except the
//! auth page's `URL >` field and its client form), ↑/↓/digits/Enter/Esc, a
//! page walk servers → server detail → tools → tool detail, plus the OAuth
//! page.
//!
//! The pure core holds the boundary-injected snapshot
//! ([`crate::mcp::McpServerSnapshot`] — the `open_skills_menu` seam) and the
//! page/selection state; every operation dispatches a typed
//! [`McpOp`](crate::app::Action) the boundary applies against the live
//! [`crate::llm::mcp::McpManager`], whose events re-inject a fresh snapshot,
//! so the menu is always looking at live state.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::mcp::{McpAuthState, McpClientRequest, McpServerSnapshot, McpServerStatus};
use crate::secrets::SecretValue;

use super::{Action, App, McpOp, wrap_step};

/// Which page the menu shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpPage {
    /// The grouped server list.
    List,
    /// One server's facts + actions.
    Server,
    /// The tools a server offers.
    Tools,
    /// One tool's full story (wire name, description, parameters).
    Tool { tool: usize },
    /// The OAuth flow: the authorize URL + the `URL >` paste fallback.
    Auth,
}

/// One action a server-detail page offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerAction {
    ViewTools,
    Authenticate,
    Reauthenticate,
    ClearAuth,
    /// Change the OAuth client entered for the server on the auth page.
    ChangeClient,
    /// Give the client the server's entry names its secret.
    SetClientSecret,
    /// Forget the entered client, and the login it was granted.
    ForgetClient,
    Reconnect,
    Disable,
    Enable,
}

impl McpServerAction {
    /// The option row's label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ViewTools => "View tools",
            Self::Authenticate => "Authenticate",
            Self::Reauthenticate => "Re-authenticate",
            Self::ClearAuth => "Clear authentication",
            Self::ChangeClient => "Change OAuth client",
            Self::SetClientSecret => "Set client secret",
            Self::ForgetClient => "Forget OAuth client",
            Self::Reconnect => "Reconnect",
            Self::Disable => "Disable",
            Self::Enable => "Enable",
        }
    }
}

/// The actions a server's state affords, in display order — the reference's
/// menus (`docs/mcp.md`).
#[must_use]
pub fn server_actions(server: &McpServerSnapshot) -> Vec<McpServerAction> {
    use McpServerAction as A;
    // What the auth row affords: a stored grant can be re-run and cleared, a
    // server that never issued one can only be logged into, and a written-in
    // header or a public server affords neither — there is nothing stored to
    // re-run (`docs/mcp.md`).
    let auth_actions = |out: &mut Vec<A>| match server.auth {
        Some(McpAuthState::Authenticated | McpAuthState::Expired) => {
            out.push(A::Reauthenticate);
            out.push(A::ClearAuth);
        }
        Some(McpAuthState::NotAuthenticated) => out.push(A::Authenticate),
        Some(McpAuthState::Header | McpAuthState::NotRequired) | None => {}
    };
    // The OAuth client the sign-in presents, where the user has one on hand
    // (`docs/mcp.md` *Pre-registered clients*): the entry's own outranks one
    // entered here, and can only be given its secret — unless the entry
    // spells that too, which is the file's to change; one entered on the
    // auth page can be changed or forgotten. Nothing for a server that signs
    // in with a written-in header, or not at all.
    let client_actions = |out: &mut Vec<A>| {
        if matches!(server.auth, None | Some(McpAuthState::Header)) {
            return;
        }
        match server.config.oauth() {
            Some(oauth) if oauth.client_id.is_some() => {
                if oauth.client_secret.is_none() {
                    out.push(A::SetClientSecret);
                }
            }
            _ if server.kept_client.is_some() => {
                out.push(A::ChangeClient);
                out.push(A::ForgetClient);
            }
            _ => {}
        }
    };
    match &server.status {
        McpServerStatus::Disabled => vec![A::Enable],
        McpServerStatus::NeedsAuth => {
            let mut out = Vec::new();
            out.push(A::Authenticate);
            // A grant the server is refusing must be removable from the page
            // that reports it — it used to be unclearable from here, so a
            // dead login could only be deleted by editing the store by hand.
            if server.auth.is_some_and(McpAuthState::has_grant) {
                out.push(A::ClearAuth);
            }
            client_actions(&mut out);
            out.push(A::Disable);
            out
        }
        McpServerStatus::Failed(_) => {
            let mut out = Vec::new();
            auth_actions(&mut out);
            client_actions(&mut out);
            out.push(A::Reconnect);
            out.push(A::Disable);
            out
        }
        McpServerStatus::Connected => {
            let mut out = Vec::new();
            if !server.tools.is_empty() {
                out.push(A::ViewTools);
            }
            auth_actions(&mut out);
            client_actions(&mut out);
            out.push(A::Reconnect);
            out.push(A::Disable);
            out
        }
        // Reconnect first: the page a Reconnect or an Enable lands on, so
        // a second Enter on it restarts the attempt rather than disabling.
        McpServerStatus::Pending => vec![A::Reconnect, A::Disable],
        // A project server waiting for the startup review has no actions
        // here: the review on screen is where it is answered
        // (`docs/config-review.md`).
        McpServerStatus::AwaitingReview => Vec::new(),
        // MCP is off for the session (`/settings`): nothing connects, so
        // the one decision left is the server's own switch — whether it
        // runs once MCP is back on (`docs/mcp.md`).
        McpServerStatus::Off => vec![A::Disable],
    }
}

/// The auth page's live state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpAuthView {
    /// The authorize URL, once the flow's discovery produced it.
    pub url: Option<String>,
    /// The `URL >` paste-fallback field.
    pub input: String,
    /// Set once the pasted URL was submitted — the field greys out while the
    /// exchange runs.
    pub submitted: bool,
    /// Why the flow turned the last paste away — shown under the field until
    /// the URL is edited.
    pub error: Option<String>,
    /// The client form, while the sign-in waits on the user for an OAuth
    /// client (`docs/mcp.md` *Pre-registered clients*) — the page is the
    /// form instead of the URL and its field.
    pub client: Option<McpClientForm>,
    /// Why the sign-in failed, shown whole in place of everything else on
    /// the page — the `/login` device page's rule: an explanation is never
    /// closed away with the page it explains, and the toast row it used to
    /// be was cut at the width.
    pub failed: Option<String>,
}

/// The client form's fields, in the order they sit on the page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum McpClientField {
    /// The port the redirect URI names.
    Port,
    #[default]
    ClientId,
    Secret,
}

/// The auth page's client form: the authorization server registers no
/// client for this app — or turned the user's own away — so the user enters
/// one they created with the provider, the spec's "prompt the user to enter
/// the client information". The secret is held redacted
/// ([`SecretValue`]) and drawn only as a mask.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpClientForm {
    /// The authorization server the client is created with, as its issuer
    /// names itself; empty when no sign-in has seen the server yet.
    pub issuer: String,
    /// The server entry's own client id, when only its secret is asked for:
    /// shown, never a field.
    pub fixed_client_id: Option<String>,
    pub client_id: String,
    pub secret: SecretValue,
    /// The callback port as typed — the redirect URI on the page follows it.
    /// Empty for an entry's own client whose sign-in takes any free port.
    pub port: String,
    /// A secret is already kept for this client: an empty field keeps it.
    pub has_secret: bool,
    /// The form opened over a client already on hand — to change it, or
    /// because it was turned away — rather than asking for the first one.
    pub on_hand: bool,
    /// The field taking keys.
    pub field: McpClientField,
    /// Why the form is back — the authorization server turned the client
    /// away, its port was taken — in the sign-in's own words.
    pub reason: Option<String>,
    /// What stopped the last submit, until the next edit.
    pub error: Option<String>,
}

impl McpClientForm {
    /// The form a request opens: the id and port it already knows filled
    /// in, the caret on the first field still to give.
    fn over(request: &McpClientRequest) -> Self {
        let known = request.client_id.clone().unwrap_or_default();
        // An id is only fixed when there is one: a form must never open
        // with no field its caret can reach.
        let fixed = request.fixed_id && !known.is_empty();
        Self {
            issuer: request.issuer.clone(),
            fixed_client_id: fixed.then(|| known.clone()),
            field: if known.is_empty() {
                McpClientField::ClientId
            } else {
                McpClientField::Secret
            },
            on_hand: !known.is_empty(),
            client_id: if fixed { String::new() } else { known },
            secret: SecretValue::default(),
            port: request
                .callback_port
                .map(|port| port.to_string())
                .unwrap_or_default(),
            has_secret: request.has_secret,
            reason: request.reason.clone(),
            error: None,
        }
    }

    /// The fields the caret visits, in page order — the secret alone when
    /// the entry names the client (and with it the port).
    fn fields(&self) -> &'static [McpClientField] {
        if self.fixed_client_id.is_some() {
            &[McpClientField::Secret]
        } else {
            &[
                McpClientField::Port,
                McpClientField::ClientId,
                McpClientField::Secret,
            ]
        }
    }

    /// Move the caret `step` fields on, wrapping.
    fn step(&mut self, step: isize) {
        let fields = self.fields();
        let at = fields.iter().position(|f| *f == self.field).unwrap_or(0);
        self.field = fields[wrap_step(at, fields.len(), step)];
    }

    /// Type `text` into the focused field: the port takes digits, five at
    /// most; the others whatever was typed.
    fn type_text(&mut self, text: &str) {
        match self.field {
            McpClientField::Port => {
                let room = PORT_DIGITS.saturating_sub(self.port.len());
                self.port
                    .extend(text.chars().filter(char::is_ascii_digit).take(room));
            }
            McpClientField::ClientId => self.client_id.push_str(text),
            McpClientField::Secret => self.secret.push_str(text),
        }
        self.error = None;
    }

    fn backspace(&mut self) {
        match self.field {
            McpClientField::Port => {
                self.port.pop();
            }
            McpClientField::ClientId => {
                self.client_id.pop();
            }
            McpClientField::Secret => {
                self.secret.pop();
            }
        }
        self.error = None;
    }

    fn clear_focused(&mut self) {
        match self.field {
            McpClientField::Port => self.port.clear(),
            McpClientField::ClientId => self.client_id.clear(),
            McpClientField::Secret => self.secret.clear(),
        }
        self.error = None;
    }

    /// The port the form names, when what is typed is one.
    #[must_use]
    pub fn callback_port(&self) -> Option<u16> {
        self.port.parse::<u16>().ok().filter(|port| *port != 0)
    }

    /// The redirect URI the sign-in will send for the port typed — what a
    /// provider that matches redirect URIs exactly must have registered.
    /// `None` while the port is no port, or the sign-in takes any.
    #[must_use]
    pub fn redirect_uri(&self) -> Option<String> {
        self.callback_port().map(crate::mcp::oauth_redirect_uri)
    }

    /// The client the form holds, checked — or the field at fault and what
    /// to say about it.
    fn checked(&self) -> Result<EnteredClient, (McpClientField, &'static str)> {
        let secret = self.secret.expose().trim();
        let client_secret = (!secret.is_empty()).then(|| SecretValue::new(secret));
        if let Some(client_id) = &self.fixed_client_id {
            if client_secret.is_none() && !self.has_secret {
                return Err((McpClientField::Secret, CLIENT_SECRET_MISSING));
            }
            return Ok(EnteredClient {
                client_id: client_id.clone(),
                client_secret,
                callback_port: self.callback_port(),
            });
        }
        let client_id = self.client_id.trim();
        if client_id.is_empty() {
            return Err((McpClientField::ClientId, CLIENT_ID_MISSING));
        }
        if client_id.contains(char::is_whitespace) {
            return Err((McpClientField::ClientId, CLIENT_ID_SPACES));
        }
        let Some(callback_port) = self.callback_port() else {
            return Err((McpClientField::Port, CLIENT_PORT_INVALID));
        };
        Ok(EnteredClient {
            client_id: client_id.to_string(),
            client_secret,
            callback_port: Some(callback_port),
        })
    }
}

/// What a checked form submits.
struct EnteredClient {
    client_id: String,
    client_secret: Option<SecretValue>,
    callback_port: Option<u16>,
}

/// The most digits a port has.
const PORT_DIGITS: usize = 5;

/// What stops a submit, in the form's own words.
const CLIENT_ID_MISSING: &str = "Enter the client ID the provider gave the client.";
const CLIENT_ID_SPACES: &str = "A client ID has no spaces in it.";
const CLIENT_PORT_INVALID: &str = "The callback port is a number from 1 to 65535.";
const CLIENT_SECRET_MISSING: &str = "Enter the client's secret.";

/// The open `/mcp` menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpMenu {
    /// The boundary-injected snapshot, in declaration order (project scope
    /// first — the merge's order).
    pub servers: Vec<McpServerSnapshot>,
    pub page: McpPage,
    /// The selected **server** (an index into `servers`) — kept across the
    /// page walk so Esc returns to the row that was entered.
    pub server: usize,
    /// The selected row on the current page (list: server; server: action;
    /// tools: tool).
    pub selected: usize,
    /// The auth page's state, while one runs.
    pub auth: McpAuthView,
}

impl McpMenu {
    /// The snapshot under the current server selection.
    #[must_use]
    pub fn current_server(&self) -> Option<&McpServerSnapshot> {
        self.servers.get(self.server)
    }
}

impl App {
    /// Open the `/mcp` manager over the boundary's snapshot — the
    /// `open_skills_menu` seam: the data is injected whole, the pure core
    /// only holds it. Closes the composer bands it replaces.
    pub fn open_mcp_menu(&mut self, servers: Vec<McpServerSnapshot>) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = super::Backtrack::default();
        self.mcp_menu = Some(McpMenu {
            servers,
            page: McpPage::List,
            server: 0,
            selected: 0,
            auth: McpAuthView::default(),
        });
    }

    /// Close the manager (Esc from the list, Ctrl+C anywhere).
    pub fn close_mcp_menu(&mut self) {
        self.mcp_menu = None;
    }

    /// Re-inject a fresh snapshot (an MCP event landed while the menu is
    /// open) — selections clamp, the page holds, so a reconnect resolving
    /// under the user never yanks them elsewhere.
    pub fn update_mcp_snapshot(&mut self, servers: Vec<McpServerSnapshot>) {
        let Some(menu) = &mut self.mcp_menu else {
            return;
        };
        // On a server's page the selection is an *action*: a state change
        // reorders the list under it (a Reconnect turns the server
        // `connecting…`, whose list is shorter), and following the index
        // landed a second Enter on Disable. It stays on the same action, or
        // the first when that one is gone.
        let chosen = (menu.page == McpPage::Server)
            .then(|| {
                menu.current_server()
                    .and_then(|s| server_actions(s).get(menu.selected).copied())
            })
            .flatten();
        menu.servers = servers;
        let server_count = menu.servers.len();
        if server_count == 0 {
            menu.page = McpPage::List;
            menu.server = 0;
            menu.selected = 0;
            return;
        }
        menu.server = menu.server.min(server_count - 1);
        let clamp = |selected: usize, len: usize| {
            if len == 0 { 0 } else { selected.min(len - 1) }
        };
        match menu.page {
            McpPage::List => menu.selected = clamp(menu.selected, server_count),
            McpPage::Server => {
                let actions = menu
                    .current_server()
                    .map(server_actions)
                    .unwrap_or_default();
                menu.selected = chosen
                    .and_then(|chosen| actions.iter().position(|action| *action == chosen))
                    .unwrap_or(0);
            }
            McpPage::Tools => {
                let len = menu.current_server().map_or(0, |s| s.tools.len());
                menu.selected = clamp(menu.selected, len);
            }
            McpPage::Tool { tool } => {
                let len = menu.current_server().map_or(0, |s| s.tools.len());
                if tool >= len {
                    menu.page = McpPage::Tools;
                    menu.selected = clamp(tool, len);
                }
            }
            McpPage::Auth => {}
        }
    }

    /// The auth flow produced its authorize URL — the auth page shows it.
    pub fn set_mcp_auth_url(&mut self, server: &str, url: &str) {
        if let Some(menu) = &mut self.mcp_menu
            && menu.page == McpPage::Auth
            && menu.current_server().is_some_and(|s| s.name == server)
        {
            menu.auth.url = Some(url.to_string());
        }
    }

    /// `server`'s auth flow finished. Success closes its auth page back to
    /// the server's detail; a failure **stays** on the page, its reason
    /// whole ([`McpAuthView::failed`]). Returns whether the open page took
    /// the failure — the boundary toasts whatever it did not: a success, a
    /// page the user already left, another server's.
    pub fn finish_mcp_auth(&mut self, server: &str, ok: bool, detail: &str) -> bool {
        let Some(menu) = &mut self.mcp_menu else {
            return false;
        };
        if menu.page != McpPage::Auth || menu.current_server().is_none_or(|s| s.name != server) {
            return false;
        }
        if ok {
            menu.page = McpPage::Server;
            menu.selected = 0;
            menu.auth = McpAuthView::default();
            return false;
        }
        menu.auth = McpAuthView {
            failed: Some(detail.to_string()),
            ..McpAuthView::default()
        };
        true
    }

    /// `server`'s sign-in has no OAuth client to present, or the user's own
    /// was turned away: its auth page becomes the client form over
    /// `request`. `false` when that page is no longer open for the server —
    /// the boundary says it in a toast instead.
    pub fn ask_mcp_client(&mut self, server: &str, request: &McpClientRequest) -> bool {
        self.open_mcp_client_form(server, request, &McpPage::Auth)
    }

    /// The client form opened from `server`'s own page (`Change OAuth
    /// client`, `Set client secret`) over the client on hand. `false` unless
    /// that page is the one open.
    pub fn edit_mcp_client(&mut self, server: &str, request: &McpClientRequest) -> bool {
        self.open_mcp_client_form(server, request, &McpPage::Server)
    }

    /// Put the client form on the auth page of `server`, when the menu is
    /// open on that server's `from` page.
    fn open_mcp_client_form(
        &mut self,
        server: &str,
        request: &McpClientRequest,
        from: &McpPage,
    ) -> bool {
        let Some(menu) = &mut self.mcp_menu else {
            return false;
        };
        if menu.page != *from || menu.current_server().is_none_or(|s| s.name != server) {
            return false;
        }
        menu.page = McpPage::Auth;
        menu.selected = 0;
        menu.auth = McpAuthView {
            client: Some(McpClientForm::over(request)),
            ..McpAuthView::default()
        };
        true
    }

    /// `server`'s flow turned the pasted URL away: the field takes another
    /// paste, with the reason under it.
    pub fn reject_mcp_auth_paste(&mut self, server: &str, reason: &str) {
        if let Some(menu) = &mut self.mcp_menu
            && menu.page == McpPage::Auth
            && menu.current_server().is_some_and(|s| s.name == server)
        {
            menu.auth.submitted = false;
            menu.auth.error = Some(reason.to_string());
        }
    }

    /// The open menu's key handler — routed first from
    /// [`on_key`](App::on_key) while the menu is up.
    pub(super) fn on_key_mcp(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Ctrl+C closes the menu, never quits (the picker grammar). An
        // active auth flow is abandoned with it.
        if ctrl && key.code == KeyCode::Char('c') {
            let auth_active = self
                .mcp_menu
                .as_ref()
                .is_some_and(|m| m.page == McpPage::Auth);
            self.close_mcp_menu();
            if auth_active {
                return Action::McpOp(McpOp::CancelAuth);
            }
            return Action::CloseMcpMenu;
        }
        let Some(menu) = &mut self.mcp_menu else {
            return Action::None;
        };
        match menu.page.clone() {
            McpPage::List => self.on_key_mcp_list(key),
            McpPage::Server => self.on_key_mcp_server(key),
            McpPage::Tools => self.on_key_mcp_tools(key),
            McpPage::Tool { .. } => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Backspace)
                    && let Some(menu) = &mut self.mcp_menu
                {
                    let tool = match menu.page {
                        McpPage::Tool { tool } => tool,
                        _ => 0,
                    };
                    menu.page = McpPage::Tools;
                    menu.selected = tool;
                }
                Action::None
            }
            McpPage::Auth => self.on_key_mcp_auth(key),
        }
    }

    fn on_key_mcp_list(&mut self, key: KeyEvent) -> Action {
        let Some(menu) = &mut self.mcp_menu else {
            return Action::None;
        };
        let count = menu.servers.len();
        match key.code {
            KeyCode::Esc => {
                self.close_mcp_menu();
                return Action::CloseMcpMenu;
            }
            KeyCode::Up => menu.selected = wrap_step(menu.selected, count, -1),
            KeyCode::Down => menu.selected = wrap_step(menu.selected, count, 1),
            KeyCode::Home => menu.selected = 0,
            KeyCode::End if count > 0 => menu.selected = count - 1,
            KeyCode::Char(c @ '1'..='9') => {
                let index = (c as usize) - ('1' as usize);
                if index < count {
                    menu.server = index;
                    menu.page = McpPage::Server;
                    menu.selected = 0;
                }
            }
            KeyCode::Enter if count > 0 => {
                menu.server = menu.selected;
                menu.page = McpPage::Server;
                menu.selected = 0;
            }
            _ => {}
        }
        Action::None
    }

    fn on_key_mcp_server(&mut self, key: KeyEvent) -> Action {
        let Some(menu) = &mut self.mcp_menu else {
            return Action::None;
        };
        let actions = menu.current_server().map_or_else(Vec::new, server_actions);
        let count = actions.len();
        let run = |menu: &mut McpMenu, action: McpServerAction| -> Action {
            let Some(server) = menu.current_server() else {
                return Action::None;
            };
            let name = server.name.clone();
            match action {
                McpServerAction::ViewTools => {
                    menu.page = McpPage::Tools;
                    menu.selected = 0;
                    Action::None
                }
                McpServerAction::Authenticate | McpServerAction::Reauthenticate => {
                    menu.page = McpPage::Auth;
                    menu.selected = 0;
                    menu.auth = McpAuthView::default();
                    Action::McpOp(McpOp::Authenticate { server: name })
                }
                McpServerAction::ClearAuth => Action::McpOp(McpOp::ClearAuth { server: name }),
                // The form opens over the client on hand, which only the
                // boundary knows: the page stays until it answers.
                McpServerAction::ChangeClient | McpServerAction::SetClientSecret => {
                    Action::McpOp(McpOp::EditAuthClient { server: name })
                }
                McpServerAction::ForgetClient => {
                    Action::McpOp(McpOp::ForgetAuthClient { server: name })
                }
                McpServerAction::Reconnect => Action::McpOp(McpOp::Reconnect { server: name }),
                McpServerAction::Disable => Action::McpOp(McpOp::SetDisabled {
                    server: name,
                    disabled: true,
                }),
                McpServerAction::Enable => Action::McpOp(McpOp::SetDisabled {
                    server: name,
                    disabled: false,
                }),
            }
        };
        match key.code {
            KeyCode::Esc | KeyCode::Backspace => {
                menu.page = McpPage::List;
                menu.selected = menu.server;
            }
            KeyCode::Up => menu.selected = wrap_step(menu.selected, count, -1),
            KeyCode::Down => menu.selected = wrap_step(menu.selected, count, 1),
            KeyCode::Char(c @ '1'..='9') => {
                let index = (c as usize) - ('1' as usize);
                if let Some(action) = actions.get(index).copied() {
                    return run(menu, action);
                }
            }
            KeyCode::Enter => {
                if let Some(action) = actions.get(menu.selected).copied() {
                    return run(menu, action);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn on_key_mcp_tools(&mut self, key: KeyEvent) -> Action {
        let Some(menu) = &mut self.mcp_menu else {
            return Action::None;
        };
        let count = menu.current_server().map_or(0, |s| s.tools.len());
        match key.code {
            KeyCode::Esc | KeyCode::Backspace => {
                menu.page = McpPage::Server;
                menu.selected = 0;
            }
            KeyCode::Up => menu.selected = wrap_step(menu.selected, count, -1),
            KeyCode::Down => menu.selected = wrap_step(menu.selected, count, 1),
            KeyCode::Home => menu.selected = 0,
            KeyCode::End if count > 0 => menu.selected = count - 1,
            KeyCode::Char(c @ '1'..='9') => {
                let index = (c as usize) - ('1' as usize);
                if index < count {
                    menu.page = McpPage::Tool { tool: index };
                }
            }
            KeyCode::Enter if count > 0 => {
                menu.page = McpPage::Tool {
                    tool: menu.selected,
                };
            }
            _ => {}
        }
        Action::None
    }

    fn on_key_mcp_auth(&mut self, key: KeyEvent) -> Action {
        let Some(menu) = &mut self.mcp_menu else {
            return Action::None;
        };
        // A failed sign-in's page: Enter tries again, Esc goes back. Nothing
        // is typed there, and no sign-in is running to cancel.
        if menu.auth.failed.is_some() {
            match key.code {
                KeyCode::Enter => {
                    let Some(server) = menu.current_server().map(|s| s.name.clone()) else {
                        return Action::None;
                    };
                    menu.auth = McpAuthView::default();
                    return Action::McpOp(McpOp::Authenticate { server });
                }
                KeyCode::Esc | KeyCode::Backspace => {
                    menu.page = McpPage::Server;
                    menu.selected = 0;
                    menu.auth = McpAuthView::default();
                }
                _ => {}
            }
            return Action::None;
        }
        if menu.auth.client.is_some() {
            return self.on_key_mcp_client(key);
        }
        match key.code {
            KeyCode::Esc => {
                menu.page = McpPage::Server;
                menu.selected = 0;
                menu.auth = McpAuthView::default();
                return Action::McpOp(McpOp::CancelAuth);
            }
            // `c` copies the authorize URL — only while the field is empty,
            // so typing a URL containing `c` still works.
            KeyCode::Char('c') if menu.auth.input.is_empty() => {
                if let Some(url) = menu.auth.url.clone() {
                    return Action::McpOp(McpOp::CopyAuthUrl { url });
                }
            }
            KeyCode::Enter => {
                let text = menu.auth.input.trim().to_string();
                if !text.is_empty() && !menu.auth.submitted {
                    menu.auth.submitted = true;
                    return Action::McpOp(McpOp::SubmitAuthUrl { text });
                }
            }
            KeyCode::Backspace => {
                menu.auth.input.pop();
                menu.auth.submitted = false;
                menu.auth.error = None;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                menu.auth.input.push(c);
                menu.auth.submitted = false;
                menu.auth.error = None;
            }
            _ => {}
        }
        Action::None
    }

    /// The client form's keys: Tab and the arrows walk the fields, Enter
    /// moves on and — on the last — submits, Ctrl+U empties the focused
    /// field, Ctrl+Y copies the redirect URI, and Esc goes back to the
    /// server's page: the sign-in that asked has ended, so there is nothing
    /// to cancel.
    fn on_key_mcp_client(&mut self, key: KeyEvent) -> Action {
        let Some(menu) = &mut self.mcp_menu else {
            return Action::None;
        };
        let Some(server) = menu.current_server().map(|s| s.name.clone()) else {
            return Action::None;
        };
        let Some(form) = &mut menu.auth.client else {
            return Action::None;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                menu.page = McpPage::Server;
                menu.selected = 0;
                menu.auth = McpAuthView::default();
            }
            KeyCode::Tab | KeyCode::Down => form.step(1),
            KeyCode::BackTab | KeyCode::Up => form.step(-1),
            KeyCode::Enter => {
                // Enter on a field that is not the last moves on, the
                // `/secrets` form's rule.
                if form.fields().last() != Some(&form.field) {
                    form.step(1);
                    return Action::None;
                }
                match form.checked() {
                    Ok(entered) => {
                        let issuer = form.issuer.clone();
                        // The auth page again, waiting on the sign-in the
                        // boundary starts once the client is kept.
                        menu.auth = McpAuthView::default();
                        return Action::McpOp(McpOp::SaveAuthClient {
                            server,
                            issuer,
                            client_id: entered.client_id,
                            client_secret: entered.client_secret,
                            callback_port: entered.callback_port,
                        });
                    }
                    Err((field, problem)) => {
                        form.field = field;
                        form.error = Some(problem.to_string());
                    }
                }
            }
            KeyCode::Backspace => form.backspace(),
            KeyCode::Char('u') if ctrl => form.clear_focused(),
            KeyCode::Char('y') if ctrl => {
                if let Some(uri) = form.redirect_uri() {
                    return Action::McpOp(McpOp::CopyRedirectUri { uri });
                }
            }
            KeyCode::Char(c) if !ctrl => form.type_text(c.encode_utf8(&mut [0; 4])),
            _ => {}
        }
        Action::None
    }

    /// Route a bracketed paste into the auth page's field — the client
    /// form's focused one, else the `URL >` field (the redirect URL is long —
    /// pasting is the point). A failed page takes none.
    pub fn paste_into_mcp_auth(&mut self, text: &str) -> bool {
        if let Some(menu) = &mut self.mcp_menu
            && menu.page == McpPage::Auth
            && menu.auth.failed.is_none()
        {
            let text = text.replace(['\n', '\r'], "");
            if let Some(form) = &mut menu.auth.client {
                form.type_text(text.trim());
                return true;
            }
            menu.auth.input.push_str(text.trim());
            menu.auth.submitted = false;
            menu.auth.error = None;
            return true;
        }
        false
    }
}
