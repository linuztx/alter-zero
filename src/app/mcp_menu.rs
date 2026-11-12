//! The `/mcp` manager (`docs/tools/mcp.md`) — the sixth composer-replacing inline
//! picker, and deliberately the hooks-menu twin: no text entry (except the
//! auth page's `URL >` field), ↑/↓/digits/Enter/Esc, a page walk
//! servers → server detail → tools → tool detail, plus the OAuth page.
//!
//! The pure core holds the boundary-injected snapshot
//! ([`crate::mcp::McpServerSnapshot`] — the `open_skills_menu` seam) and the
//! page/selection state; every operation dispatches a typed
//! [`McpOp`](crate::app::Action) the boundary applies against the live
//! [`crate::llm::mcp::McpManager`], whose events re-inject a fresh snapshot,
//! so the menu is always looking at live state.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::mcp::{McpAuthState, McpServerSnapshot, McpServerStatus};

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
            Self::Reconnect => "Reconnect",
            Self::Disable => "Disable",
            Self::Enable => "Enable",
        }
    }
}

/// The actions a server's state affords, in display order — the reference's
/// menus (`docs/tools/mcp.md`).
#[must_use]
pub fn server_actions(server: &McpServerSnapshot) -> Vec<McpServerAction> {
    use McpServerAction as A;
    // What the auth row affords: a stored grant can be re-run and cleared, a
    // server that never issued one can only be logged into, and a written-in
    // header or a public server affords neither — there is nothing stored to
    // re-run (`docs/tools/mcp.md`).
    let auth_actions = |out: &mut Vec<A>| match server.auth {
        Some(McpAuthState::Authenticated | McpAuthState::Expired) => {
            out.push(A::Reauthenticate);
            out.push(A::ClearAuth);
        }
        Some(McpAuthState::NotAuthenticated) => out.push(A::Authenticate),
        Some(McpAuthState::Header | McpAuthState::NotRequired) | None => {}
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
            out.push(A::Disable);
            out
        }
        McpServerStatus::Failed(_) => {
            let mut out = Vec::new();
            auth_actions(&mut out);
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
            out.push(A::Reconnect);
            out.push(A::Disable);
            out
        }
        // Reconnect first: the page a Reconnect or an Enable lands on, so
        // a second Enter on it restarts the attempt rather than disabling.
        McpServerStatus::Pending => vec![A::Reconnect, A::Disable],
        // A project server waiting for the startup review has no actions
        // here: the review on screen is where it is answered
        // (`docs/state/config-review.md`).
        McpServerStatus::AwaitingReview => Vec::new(),
        // MCP is off for the session (`/settings`): nothing connects, so
        // the one decision left is the server's own switch — whether it
        // runs once MCP is back on (`docs/tools/mcp.md`).
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
}

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

    /// `server`'s auth flow finished — its auth page closes back to the
    /// server's detail (the boundary raises the outcome toast beside this).
    /// Another server's page is left alone.
    pub fn finish_mcp_auth(&mut self, server: &str) {
        if let Some(menu) = &mut self.mcp_menu
            && menu.page == McpPage::Auth
            && menu.current_server().is_some_and(|s| s.name == server)
        {
            menu.page = McpPage::Server;
            menu.selected = 0;
            menu.auth = McpAuthView::default();
        }
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

    /// Route a bracketed paste into the auth page's `URL >` field (the
    /// redirect URL is long — pasting is the point).
    pub fn paste_into_mcp_auth(&mut self, text: &str) -> bool {
        if let Some(menu) = &mut self.mcp_menu
            && menu.page == McpPage::Auth
        {
            menu.auth
                .input
                .push_str(text.replace(['\n', '\r'], "").trim());
            menu.auth.submitted = false;
            menu.auth.error = None;
            return true;
        }
        false
    }
}
