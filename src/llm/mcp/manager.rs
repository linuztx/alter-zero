//! The session-owned MCP registry (`docs/mcp.md`): every declared server's
//! live state behind one `Arc<Mutex<…>>` handle (the `BackgroundRegistry`
//! shape), connected **concurrently on worker threads** at startup, every
//! state change reported on the loop's MCP event channel so the `/mcp`
//! manager is always looking at live state.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::mcp::{
    McpServerEntry, McpServerSnapshot, McpServerStatus, McpToolInfo, ServerIdentity, call_params,
    parse_call_result, tool_wire_name,
};
use crate::stream::CancelToken;

use super::client::{ConnectError, connect_with_era};
use super::oauth::{self, AuthProgress};
use super::transport::{Transport, TransportError};
use crate::llm::tools::{ToolCallRequest, ToolOutcome};

/// What the manager reports on the loop's MCP channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpEvent {
    /// Some server's state changed — re-inject the snapshot, maybe rebuild
    /// the backend (the offered tool set may have flipped).
    Changed,
    /// An auth flow produced its authorize URL — the auth page shows it.
    AuthUrl { server: String, url: String },
    /// An auth flow finished (either way) — the auth page closes to the
    /// list, a toast reports the outcome.
    AuthDone {
        server: String,
        ok: bool,
        detail: String,
    },
}

/// What the manager is built from — gathered by the boundary
/// (`tui`'s `load_mcp_sources` reads the real files; tests pass fixtures).
#[derive(Debug, Clone, Default)]
pub struct McpSources {
    pub entries: Vec<McpServerEntry>,
    /// Config parse problems, surfaced once as a red startup toast.
    pub errors: Vec<String>,
    /// The per-project disabled set (from the user file).
    pub disabled: std::collections::BTreeSet<String>,
    /// Project-declared servers the trust gate is holding
    /// (`docs/project-config.md`): never launched until `/trust` approves.
    pub untrusted: std::collections::BTreeSet<String>,
    /// The absolute cwd — the disabled sets' project key.
    pub project: String,
    /// The user config file — where a disable persists. `None` = no
    /// persistence (still toggles for the session).
    pub user_file: Option<PathBuf>,
    /// The OAuth token store (`{config_home}/mcp-auth.json`).
    pub auth_path: Option<PathBuf>,
    /// The era cache (`{config_home}/mcp-era.json`) — the remembered
    /// protocol-era verdict per server, so the probe runs once rather than
    /// at every launch (`docs/mcp.md`).
    pub era_path: Option<PathBuf>,
    /// The session cwd stdio children run in.
    pub cwd: Option<PathBuf>,
    pub startup_timeout: Duration,
    pub tool_timeout: Duration,
}

/// The default per-server connect budget (both references use 30 s;
/// `ALTER_ZERO_MCP_STARTUP_TIMEOUT_MS` overrides).
pub const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// The default per-call budget (`ALTER_ZERO_MCP_TOOL_TIMEOUT_MS` overrides).
pub const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(120);

/// The cancel-poll cadence every blocking wait shares with the transports.
const POLL: Duration = Duration::from_millis(20);

/// The remembered era for one server, if any. Best-effort: an unreadable or
/// unrecognised cache simply means "probe".
fn load_era(path: Option<&PathBuf>, key: &str) -> Option<crate::mcp::ServerEra> {
    let contents = std::fs::read_to_string(path?).ok()?;
    crate::mcp::parse_era_store(&contents).remove(key)
}

/// Remember one server's era, so the next launch skips the probe. Written
/// only when the verdict actually changed — a connect per launch per server
/// otherwise rewrites the file for nothing.
fn save_era(path: Option<&PathBuf>, key: &str, era: &crate::mcp::ServerEra) {
    let Some(path) = path else { return };
    if load_era(Some(path), key).as_ref() == Some(era) {
        return;
    }
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let updated = crate::mcp::record_era(&existing, key, Some(era));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, updated);
}

/// Run blocking work on a worker thread while polling `cancel` on the 20 ms
/// cadence — the transport's own contract (`docs/mcp.md`), which anything
/// that blocks a turn's thread must obey. `None` means the turn was
/// cancelled (or the worker died); the thread is abandoned rather than
/// joined, exactly as a cancelled transport wait abandons its POST.
///
/// A token refresh is a blocking HTTP call, so running it inline on the tool
/// thread made Esc dead for the token endpoint's whole timeout.
fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    cancel: &CancelToken,
) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    loop {
        if cancel.is_cancelled() {
            return None;
        }
        match rx.recv_timeout(POLL) {
            Ok(value) => return Some(value),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

struct ServerState {
    entry: McpServerEntry,
    status: McpServerStatus,
    identity: Option<ServerIdentity>,
    tools: Vec<McpToolInfo>,
    /// The live transport — its own lock, so a running tool call never
    /// blocks a snapshot.
    transport: Option<Arc<Mutex<Transport>>>,
    /// wire tool name → the server's raw tool name.
    wire_map: BTreeMap<String, String>,
    has_tokens: bool,
    /// The last 401's `WWW-Authenticate` challenge. Discovery runs blind
    /// without it: the challenge names the `resource_metadata` URL a server
    /// publishes off the well-known path, and the `scope` the resource
    /// actually wants (`docs/mcp.md`).
    challenge: Option<String>,
    /// Bumped on disable/reconnect so a stale connect thread's result is
    /// dropped instead of resurrecting an old state.
    generation: u64,
}

struct Inner {
    servers: Vec<ServerState>,
    errors: Vec<String>,
    disabled: std::collections::BTreeSet<String>,
    untrusted: std::collections::BTreeSet<String>,
    project: String,
    user_file: Option<PathBuf>,
    auth_path: Option<PathBuf>,
    era_path: Option<PathBuf>,
    cwd: Option<PathBuf>,
    startup_timeout: Duration,
    tool_timeout: Duration,
    /// The active auth flow's paste channel + cancel, while one runs.
    auth_paste: Option<mpsc::Sender<String>>,
    auth_cancel: Option<CancelToken>,
}

/// The shared handle. Clones are cheap and all see the same state.
#[derive(Clone)]
pub struct McpManager {
    inner: Arc<Mutex<Inner>>,
    events: UnboundedSender<McpEvent>,
}

impl std::fmt::Debug for McpManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpManager").finish_non_exhaustive()
    }
}

impl McpManager {
    #[must_use]
    pub fn new(events: UnboundedSender<McpEvent>, sources: McpSources) -> Self {
        let auth_path = sources.auth_path.clone();
        let servers = sources
            .entries
            .into_iter()
            .map(|entry| {
                let disabled = sources.disabled.contains(&entry.name);
                let untrusted = sources.untrusted.contains(&entry.name);
                let has_tokens = entry
                    .config
                    .url()
                    .is_some_and(|url| oauth::load_tokens(auth_path.as_deref(), url).is_some());
                ServerState {
                    // The trust gate outranks the user's own toggle: an
                    // untrusted server may not run either way, and `/trust`
                    // is the only way in.
                    status: if untrusted {
                        McpServerStatus::Untrusted
                    } else if disabled {
                        McpServerStatus::Disabled
                    } else {
                        McpServerStatus::Pending
                    },
                    identity: None,
                    tools: Vec::new(),
                    transport: None,
                    wire_map: BTreeMap::new(),
                    has_tokens,
                    challenge: None,
                    generation: 0,
                    entry,
                }
            })
            .collect();
        Self {
            inner: Arc::new(Mutex::new(Inner {
                servers,
                errors: sources.errors,
                disabled: sources.disabled,
                untrusted: sources.untrusted,
                project: sources.project,
                user_file: sources.user_file,
                auth_path,
                era_path: sources.era_path,
                cwd: sources.cwd,
                startup_timeout: if sources.startup_timeout.is_zero() {
                    DEFAULT_STARTUP_TIMEOUT
                } else {
                    sources.startup_timeout
                },
                tool_timeout: if sources.tool_timeout.is_zero() {
                    DEFAULT_TOOL_TIMEOUT
                } else {
                    sources.tool_timeout
                },
                auth_paste: None,
                auth_cancel: None,
            })),
            events,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn notify(&self, event: McpEvent) {
        let _ = self.events.send(event);
    }

    /// The config parse problems, for the one red startup toast.
    #[must_use]
    pub fn config_errors(&self) -> Vec<String> {
        self.lock().errors.clone()
    }

    /// Were any servers declared at all (the `/mcp` empty-list question)?
    #[must_use]
    pub fn has_servers(&self) -> bool {
        !self.lock().servers.is_empty()
    }

    /// Kick off the startup connects — one worker thread per enabled server.
    /// Returns at once; completions report on the event channel.
    pub fn start_connections(&self) {
        let names: Vec<String> = self
            .lock()
            .servers
            .iter()
            .filter(|s| s.status == McpServerStatus::Pending)
            .map(|s| s.entry.name.clone())
            .collect();
        for name in names {
            self.spawn_connect(&name);
        }
    }

    /// Connect (or re-connect) one server on a worker thread.
    fn spawn_connect(&self, name: &str) {
        let (config, auth_path, era_path, cwd, timeout, generation) = {
            let mut inner = self.lock();
            let (auth_path, era_path, cwd, timeout) = (
                inner.auth_path.clone(),
                inner.era_path.clone(),
                inner.cwd.clone(),
                inner.startup_timeout,
            );
            let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == name) else {
                return;
            };
            server.status = McpServerStatus::Pending;
            server.generation += 1;
            let generation = server.generation;
            // Drop the old transport now (kills a stdio child) — the new
            // connect starts clean.
            server.transport = None;
            server.has_tokens = server
                .entry
                .config
                .url()
                .is_some_and(|url| oauth::load_tokens(auth_path.as_deref(), url).is_some());
            (
                server.entry.config.clone(),
                auth_path,
                era_path,
                cwd,
                timeout,
                generation,
            )
        };
        self.notify(McpEvent::Changed);
        let manager = self.clone();
        let name = name.to_string();
        std::thread::spawn(move || {
            let cancel = CancelToken::new();
            // The stored access token, refreshed first when it is inside the
            // expiry skew (a transient refresh failure still sends the
            // stored token — the 401 dance below is the backstop). On this
            // worker, never under the manager lock: a refresh is a blocking
            // HTTP call, and holding the lock across it would freeze every
            // snapshot for the duration.
            let bearer = config
                .url()
                .and_then(|url| oauth::connect_bearer(auth_path.as_deref(), url));
            // The remembered era, so a legacy server is not re-probed at
            // every launch (`docs/mcp.md`).
            let era_key = config.target();
            let cached_era = load_era(era_path.as_ref(), &era_key);
            let mut result = connect_with_era(
                &config,
                bearer,
                cwd.as_deref(),
                cached_era.clone(),
                timeout,
                &cancel,
            );
            // A 401 with a stored grant gets one forced refresh and one
            // retry before anyone is asked to re-authenticate — an access
            // token expiring between sessions is ordinary, not an auth
            // failure. Only a refresh the AS *rejected* (or no grant at all)
            // resolves as needs-auth; a transient failure keeps the grant
            // and reads as an ordinary connect failure Reconnect retries.
            let mut transient_refresh: Option<String> = None;
            if matches!(result, Err(ConnectError::NeedsAuth(_)))
                && let Some(url) = config.url()
            {
                // The token the store held when this connect was refused —
                // if another thread has rotated past it by the time the
                // refresh lock is ours, we take theirs instead of replaying.
                let presented =
                    oauth::load_tokens(auth_path.as_deref(), url).map(|t| t.access_token);
                match oauth::refresh_grant(auth_path.as_deref(), url, presented.as_deref()) {
                    Ok(tokens) => {
                        result = connect_with_era(
                            &config,
                            Some(tokens.access_token),
                            cwd.as_deref(),
                            cached_era.clone(),
                            timeout,
                            &cancel,
                        );
                    }
                    Err(oauth::RefreshFailure::Transient(detail)) => {
                        transient_refresh = Some(detail);
                    }
                    Err(_) => {} // no grant, or a cleared dead one: needs-auth is truthful
                }
            }
            let mut inner = manager.lock();
            let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == name) else {
                return;
            };
            if server.generation != generation {
                return; // a reconnect/disable superseded this attempt
            }
            match result {
                Ok(connection) => {
                    save_era(era_path.as_ref(), &era_key, &connection.era);
                    server.identity = Some(connection.identity);
                    server.wire_map = connection
                        .tools
                        .iter()
                        .map(|tool| (tool_wire_name(&name, &tool.name), tool.name.clone()))
                        .collect();
                    server.tools = connection.tools;
                    server.transport = Some(Arc::new(Mutex::new(connection.transport)));
                    server.status = McpServerStatus::Connected;
                }
                Err(ConnectError::NeedsAuth(challenge)) => {
                    // Keep what the server told us: the flow needs it to
                    // find the resource metadata and the wanted scope.
                    if challenge.is_some() {
                        server.challenge = challenge;
                    }
                    server.status = match transient_refresh {
                        // The grant survives a blip: a Failed row whose
                        // Reconnect retries, never a needs-auth that walks
                        // the user into discarding a working refresh token.
                        Some(detail) => {
                            McpServerStatus::Failed(format!("token refresh failed: {detail}"))
                        }
                        None => McpServerStatus::NeedsAuth,
                    };
                }
                Err(ConnectError::Failed(detail)) => {
                    server.status = McpServerStatus::Failed(detail);
                }
            }
            // The dance above may have rotated or cleared the grant — the
            // snapshot's auth row re-derives from what the store now holds.
            server.has_tokens = server
                .entry
                .config
                .url()
                .is_some_and(|url| oauth::load_tokens(auth_path.as_deref(), url).is_some());
            drop(inner);
            manager.notify(McpEvent::Changed);
        });
    }

    /// Reconnect one server (the `/mcp` action; also Enable's follow-up).
    pub fn reconnect(&self, name: &str) {
        self.spawn_connect(name);
    }

    /// Enable/disable one server for this project, persisting the choice in
    /// the user file (read-modify-write) and reporting the change.
    pub fn set_disabled(&self, name: &str, disabled: bool) {
        {
            let mut inner = self.lock();
            if disabled {
                inner.disabled.insert(name.to_string());
            } else {
                inner.disabled.remove(name);
            }
            let (user_file, project, set) = (
                inner.user_file.clone(),
                inner.project.clone(),
                inner.disabled.clone(),
            );
            if let Some(path) = user_file {
                let existing = std::fs::read_to_string(&path).unwrap_or_default();
                let updated = crate::mcp::record_disabled(&existing, &project, &set);
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&path, updated);
            }
            if let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == name) {
                server.generation += 1;
                if disabled {
                    server.status = McpServerStatus::Disabled;
                    server.transport = None; // kills a stdio child
                    server.tools.clear();
                    server.wire_map.clear();
                    server.identity = None;
                }
            }
        }
        self.notify(McpEvent::Changed);
        if !disabled && !self.lock().untrusted.contains(name) {
            self.spawn_connect(name);
        }
    }

    /// The `/trust` seam (`docs/project-config.md`): grant or revoke the
    /// trust gate's hold on the named project servers. Granting hands each
    /// server back to its ordinary lifecycle — connect, unless the user's
    /// own disable toggle holds it; revoking kills the transport (a stdio
    /// child dies with it) and returns the server to `untrusted`. Unlike
    /// [`Self::set_disabled`], nothing persists here — trust lives in
    /// `trust.json` at the boundary, not the user's MCP file.
    pub fn set_trusted(&self, names: &[String], trusted: bool) {
        let mut connect = Vec::new();
        {
            let mut inner = self.lock();
            for name in names {
                if trusted {
                    inner.untrusted.remove(name);
                } else {
                    inner.untrusted.insert(name.clone());
                }
                let disabled = inner.disabled.contains(name);
                let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == *name) else {
                    continue;
                };
                if trusted {
                    if server.status != McpServerStatus::Untrusted {
                        continue;
                    }
                    if disabled {
                        server.status = McpServerStatus::Disabled;
                    } else {
                        connect.push(name.clone());
                    }
                } else {
                    server.generation += 1;
                    server.status = McpServerStatus::Untrusted;
                    server.transport = None; // kills a stdio child
                    server.tools.clear();
                    server.wire_map.clear();
                    server.identity = None;
                }
            }
        }
        self.notify(McpEvent::Changed);
        for name in connect {
            self.spawn_connect(&name);
        }
    }

    /// The names the trust gate is currently holding — what a `/trust`
    /// approval should release.
    #[must_use]
    pub fn untrusted_names(&self) -> Vec<String> {
        let inner = self.lock();
        inner
            .servers
            .iter()
            .filter(|s| s.status == McpServerStatus::Untrusted)
            .map(|s| s.entry.name.clone())
            .collect()
    }

    /// Delete a server's stored OAuth tokens (the `/mcp` "Clear
    /// authentication" action).
    pub fn clear_auth(&self, name: &str) {
        {
            let mut inner = self.lock();
            let auth_path = inner.auth_path.clone();
            if let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == name) {
                if let Some(url) = server.entry.config.url() {
                    oauth::save_tokens(auth_path.as_deref(), url, None);
                }
                server.has_tokens = false;
            }
        }
        self.notify(McpEvent::Changed);
    }

    /// Start the interactive OAuth flow for one server on a worker thread.
    /// The authorize URL and the outcome report on the event channel; a
    /// pasted redirect URL arrives via [`submit_auth_paste`].
    ///
    /// [`submit_auth_paste`]: McpManager::submit_auth_paste
    pub fn authenticate(&self, name: &str) {
        let (url, challenge, auth_path) = {
            let inner = self.lock();
            let Some(server) = inner.servers.iter().find(|s| s.entry.name == name) else {
                return;
            };
            let Some(url) = server.entry.config.url().map(str::to_string) else {
                self.notify(McpEvent::AuthDone {
                    server: name.to_string(),
                    ok: false,
                    detail: "a stdio server has no authentication".to_string(),
                });
                return;
            };
            (url, server.challenge.clone(), inner.auth_path.clone())
        };
        let (paste_tx, paste_rx) = mpsc::channel();
        let cancel = CancelToken::new();
        {
            let mut inner = self.lock();
            // A new flow supersedes a stuck one.
            if let Some(old) = inner.auth_cancel.take() {
                old.cancel();
            }
            inner.auth_paste = Some(paste_tx);
            inner.auth_cancel = Some(cancel.clone());
        }
        let manager = self.clone();
        let name = name.to_string();
        std::thread::spawn(move || {
            struct Progress {
                manager: McpManager,
                server: String,
            }
            impl AuthProgress for Progress {
                fn on_url(&self, url: &str) {
                    self.manager.notify(McpEvent::AuthUrl {
                        server: self.server.clone(),
                        url: url.to_string(),
                    });
                }
            }
            let progress = Progress {
                manager: manager.clone(),
                server: name.clone(),
            };
            let result = oauth::run_auth_flow(
                &url,
                challenge.as_deref(),
                auth_path.as_deref(),
                &paste_rx,
                &progress,
                &open_browser,
                &cancel,
            );
            {
                let mut inner = manager.lock();
                inner.auth_paste = None;
                inner.auth_cancel = None;
                if result.is_ok()
                    && let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == name)
                {
                    server.has_tokens = true;
                }
            }
            let (ok, detail) = match result {
                Ok(()) => (true, format!("Authenticated with {name}")),
                Err(detail) => (false, detail),
            };
            manager.notify(McpEvent::AuthDone {
                server: name.clone(),
                ok,
                detail,
            });
            if ok {
                manager.spawn_connect(&name);
            }
        });
    }

    /// Hand the auth flow a pasted redirect URL (the `URL >` fallback).
    pub fn submit_auth_paste(&self, text: &str) {
        let sender = self.lock().auth_paste.clone();
        if let Some(sender) = sender {
            let _ = sender.send(text.to_string());
        }
    }

    /// Abandon the running auth flow (Esc on the auth page).
    pub fn cancel_auth(&self) {
        let cancel = self.lock().auth_cancel.take();
        if let Some(cancel) = cancel {
            cancel.cancel();
        }
    }

    /// The per-server snapshots the `/mcp` manager renders, in declaration
    /// order (project scope first — the merge's order).
    #[must_use]
    pub fn snapshot(&self) -> Vec<McpServerSnapshot> {
        let inner = self.lock();
        inner
            .servers
            .iter()
            .map(|server| McpServerSnapshot {
                name: server.entry.name.clone(),
                scope: server.entry.scope,
                config_path: server.entry.config_path.clone(),
                config: server.entry.config.clone(),
                status: server.status.clone(),
                auth: crate::mcp::auth_state(
                    server.entry.config.is_remote(),
                    server.entry.config.has_auth_header(),
                    server.has_tokens,
                    &server.status,
                ),
                identity: server.identity.clone(),
                tools: server.tools.clone(),
            })
            .collect()
    }

    /// The Chat Completions tool defs for every connected server's tools —
    /// what [`crate::llm::LlmBackend::with_mcp`] folds into the offered set.
    #[must_use]
    pub fn tool_specs(&self) -> Vec<Value> {
        let inner = self.lock();
        let mut out = Vec::new();
        for server in &inner.servers {
            if server.status != McpServerStatus::Connected {
                continue;
            }
            for tool in &server.tools {
                let wire = tool_wire_name(&server.entry.name, &tool.name);
                out.push(crate::llm::tools::function_spec(
                    &wire,
                    &tool.description,
                    tool.input_schema.clone(),
                ));
            }
        }
        out
    }

    /// The server's own one-line description of the tool `wire` names — the
    /// dim row the permission prompt shows under the call (`docs/mcp.md`).
    /// `None` for a tool no connected server offers.
    #[must_use]
    pub fn tool_description(&self, wire: &str) -> Option<String> {
        let inner = self.lock();
        inner.servers.iter().find_map(|server| {
            let name = server.wire_map.get(wire)?;
            server
                .tools
                .iter()
                .find(|tool| &tool.name == name)
                .map(|tool| tool.description.clone())
        })
    }

    /// Is any tool currently offered?
    #[must_use]
    pub fn has_tools(&self) -> bool {
        let inner = self.lock();
        inner
            .servers
            .iter()
            .any(|s| s.status == McpServerStatus::Connected && !s.tools.is_empty())
    }

    /// A cheap identity of the offered tool set — the boundary rebuilds the
    /// backend only when this flips (the `skills_attached` pattern).
    #[must_use]
    pub fn fingerprint(&self) -> Vec<String> {
        let inner = self.lock();
        let mut names: Vec<String> = inner
            .servers
            .iter()
            .filter(|s| s.status == McpServerStatus::Connected)
            .flat_map(|s| s.wire_map.keys().cloned())
            .collect();
        names.sort();
        names
    }

    /// Execute one `mcp__server__tool` call — the execute-closure arm
    /// (`docs/mcp.md`). Every failure is a recoverable red outcome; only a
    /// 401 additionally flips the server to needs-auth.
    #[must_use]
    pub fn call_tool(&self, call: &ToolCallRequest, cancel: &CancelToken) -> ToolOutcome {
        let lookup = {
            let inner = self.lock();
            inner
                .servers
                .iter()
                .find(|s| s.wire_map.contains_key(&call.name))
                .and_then(|server| {
                    Some((
                        server.entry.name.clone(),
                        server.wire_map.get(&call.name)?.clone(),
                        server.transport.clone()?,
                        server.entry.config.url().map(str::to_string),
                        inner.auth_path.clone(),
                        inner.tool_timeout,
                    ))
                })
        };
        let Some((server_name, raw_tool, transport, server_url, auth_path, timeout)) = lookup
        else {
            return ToolOutcome::error(format!(
                "unknown MCP tool: {} (run /mcp to see the connected servers)",
                call.name
            ));
        };
        let arguments: Value = match serde_json::from_str(call.arguments.trim()) {
            Ok(value) => value,
            Err(_) if call.arguments.trim().is_empty() => Value::Object(Default::default()),
            Err(e) => return ToolOutcome::error(format!("invalid tool arguments: {e}")),
        };
        let request = |refreshed_bearer: Option<String>| {
            let mut transport = transport
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(bearer) = refreshed_bearer {
                transport.set_bearer(Some(bearer));
            }
            transport.request(
                "tools/call",
                call_params(&raw_tool, &arguments),
                timeout,
                cancel,
            )
        };
        // Proactive: a grant inside its expiry skew refreshes *before* the
        // call, so the token never goes over the wire dead — the 401 arm
        // below stays as the backstop for revocation, clock skew, and grants
        // with no known expiry (`docs/mcp.md`). Off-thread with the turn's
        // cancel polled, because a wedged token endpoint must not eat Esc.
        let stale_refresh = match &server_url {
            Some(url) => {
                let (path, url) = (auth_path.clone(), url.clone());
                match off_thread(
                    move || oauth::refresh_if_stale(path.as_deref(), &url),
                    cancel,
                ) {
                    Some(refreshed) => refreshed,
                    None => return ToolOutcome::error("tool call cancelled"),
                }
            }
            None => None,
        };
        let mut result = request(stale_refresh);
        // Reactive: a 401 mid-session gets one forced refresh and one retry
        // before anyone is told to re-authenticate — the reference client's
        // rule, and the difference between a session that renews itself and
        // one that asks the user again every hour.
        if matches!(result, Err(TransportError::NeedsAuth(_)))
            && let Some(url) = server_url.clone()
        {
            let path = auth_path.clone();
            let refreshed = match off_thread(
                move || {
                    // The token the store held when the server refused
                    // us; a concurrent rotation past it wins the
                    // single-flight and we take its grant instead.
                    let presented =
                        oauth::load_tokens(path.as_deref(), &url).map(|t| t.access_token);
                    oauth::refresh_grant(path.as_deref(), &url, presented.as_deref())
                },
                cancel,
            ) {
                Some(refreshed) => refreshed,
                None => return ToolOutcome::error("tool call cancelled"),
            };
            match refreshed {
                Ok(tokens) => result = request(Some(tokens.access_token)),
                Err(oauth::RefreshFailure::Transient(detail)) => {
                    // A blip is not an auth state: keep the grant AND the
                    // connection, and let the model (or the next call) retry.
                    return ToolOutcome::error(format!(
                        "the {server_name} MCP server rejected the stored token and the \
                         refresh failed transiently ({detail}) — retry shortly"
                    ));
                }
                Err(_) => {} // no grant / a cleared dead one: fall through to needs-auth
            }
        }
        match result {
            Ok(result) => {
                let outcome = parse_call_result(&result);
                let (text, truncated) = crate::llm::tools::truncate_output(
                    &outcome.text,
                    crate::llm::tools::TOOL_OUTPUT_MAX_BYTES,
                );
                let mut out = if outcome.ok {
                    ToolOutcome::ok(text)
                } else {
                    ToolOutcome::error(text)
                };
                out = out.with_truncated(truncated);
                if let Some(image) = outcome.image {
                    out = out.with_image(image);
                }
                out
            }
            Err(TransportError::NeedsAuth(challenge)) => {
                {
                    let mut inner = self.lock();
                    if let Some(server) = inner
                        .servers
                        .iter_mut()
                        .find(|s| s.entry.name == server_name)
                    {
                        if challenge.is_some() {
                            server.challenge = challenge;
                        }
                        server.status = McpServerStatus::NeedsAuth;
                        server.generation += 1;
                        server.transport = None;
                        server.tools.clear();
                        server.wire_map.clear();
                    }
                }
                self.notify(McpEvent::Changed);
                ToolOutcome::error(format!(
                    "the {server_name} MCP server requires authentication — \
                     ask the user to run /mcp and authenticate"
                ))
            }
            Err(TransportError::Cancelled) => ToolOutcome::error("tool call cancelled"),
            Err(other) => {
                ToolOutcome::error(format!("MCP tool call failed ({server_name}): {other}"))
            }
        }
    }

    /// Tear every connection down (session shutdown — kills stdio children).
    pub fn shutdown(&self) {
        let mut inner = self.lock();
        if let Some(cancel) = inner.auth_cancel.take() {
            cancel.cancel();
        }
        for server in &mut inner.servers {
            server.generation += 1;
            server.transport = None;
        }
    }
}

/// Open the system browser on the authorize URL — best-effort, detached
/// (`xdg-open` on Linux, `open` on macOS); the URL is on screen either way.
fn open_browser(url: &str) {
    for launcher in ["xdg-open", "open"] {
        let spawned = std::process::Command::new(launcher)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if spawned.is_ok() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{McpScope, McpServerConfig};

    fn stdio_entry(name: &str) -> McpServerEntry {
        // The deterministic-ids `sh` fixture, a LEGACY server: the modern
        // server/discover probe = 1 (answered -32601), initialize = 2,
        // tools/list = 3, then the first tools/call (id 4) is answered from
        // a canned line.
        let probe =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#;
        let init = r#"{"jsonrpc":"2.0","id":2,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"0"}}}"#;
        let tools = r#"{"jsonrpc":"2.0","id":3,"result":{"tools":[{"name":"echo","description":"Echo.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]}}"#;
        let call =
            r#"{"jsonrpc":"2.0","id":4,"result":{"content":[{"type":"text","text":"echoed"}]}}"#;
        let script = format!(
            "cat > /dev/null & printf '%s\\n' '{probe}'; printf '%s\\n' '{init}'; printf '%s\\n' '{tools}'; printf '%s\\n' '{call}'; sleep 10"
        );
        McpServerEntry {
            name: name.to_string(),
            config: McpServerConfig::Stdio {
                command: "sh".to_string(),
                args: vec!["-c".to_string(), script],
                env: Default::default(),
            },
            scope: McpScope::User,
            config_path: "~/.alter-zero/mcp.json".to_string(),
        }
    }

    fn manager_with(
        entries: Vec<McpServerEntry>,
    ) -> (McpManager, tokio::sync::mpsc::UnboundedReceiver<McpEvent>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                entries,
                project: "/proj".to_string(),
                ..Default::default()
            },
        );
        (manager, rx)
    }

    fn wait_connected(manager: &McpManager, name: &str) {
        for _ in 0..200 {
            let snapshot = manager.snapshot();
            let server = snapshot.iter().find(|s| s.name == name).unwrap();
            match &server.status {
                McpServerStatus::Connected => return,
                McpServerStatus::Failed(e) => panic!("connect failed: {e}"),
                _ => std::thread::sleep(Duration::from_millis(25)),
            }
        }
        panic!("server never connected");
    }

    #[test]
    fn connects_snapshots_and_calls_through_the_wire_name() {
        let (manager, mut rx) = manager_with(vec![stdio_entry("fix")]);
        manager.start_connections();
        wait_connected(&manager, "fix");
        // Events reported the state changes.
        assert!(matches!(rx.try_recv(), Ok(McpEvent::Changed)));
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].status_line(), "✔ connected · 1 tool");
        assert_eq!(snapshot[0].identity.as_ref().unwrap().name, "fixture");
        // The offered specs carry the wire name and the server's schema.
        let specs = manager.tool_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0]["function"]["name"], "mcp__fix__echo");
        assert_eq!(
            specs[0]["function"]["parameters"]["required"],
            serde_json::json!(["text"])
        );
        assert_eq!(manager.fingerprint(), ["mcp__fix__echo"]);
        assert!(manager.has_tools());
        // A call routes to the raw tool and maps the result.
        let call = ToolCallRequest {
            id: "c1".to_string(),
            name: "mcp__fix__echo".to_string(),
            arguments: r#"{"text":"hi"}"#.to_string(),
        };
        let outcome = manager.call_tool(&call, &CancelToken::new());
        assert!(outcome.ok, "{}", outcome.output);
        assert_eq!(outcome.output, "echoed");
        manager.shutdown();
    }

    #[test]
    fn unknown_tools_and_disabled_servers_resolve_recoverably() {
        let (manager, _rx) = manager_with(vec![stdio_entry("fix")]);
        // Never connected: the tool is unknown.
        let call = ToolCallRequest {
            id: "c1".to_string(),
            name: "mcp__fix__echo".to_string(),
            arguments: "{}".to_string(),
        };
        let outcome = manager.call_tool(&call, &CancelToken::new());
        assert!(!outcome.ok);
        assert!(outcome.output.contains("unknown MCP tool"));
        assert!(!manager.has_tools());
        assert!(manager.fingerprint().is_empty());
    }

    #[test]
    fn an_untrusted_server_never_launches_until_trusted() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                entries: vec![stdio_entry("fix")],
                untrusted: ["fix".to_string()].into(),
                project: "/proj".to_string(),
                ..Default::default()
            },
        );
        manager.start_connections();
        // A beat for any wrongly-spawned connect to land before we look.
        std::thread::sleep(Duration::from_millis(150));
        let snapshot = manager.snapshot();
        assert_eq!(snapshot[0].status, McpServerStatus::Untrusted);
        assert!(!manager.has_tools());
        // `/trust` approval connects it…
        manager.set_trusted(&["fix".to_string()], true);
        wait_connected(&manager, "fix");
        assert!(manager.has_tools());
        // …and a revoke kills it back to untrusted, tools withdrawn.
        manager.set_trusted(&["fix".to_string()], false);
        let snapshot = manager.snapshot();
        assert_eq!(snapshot[0].status, McpServerStatus::Untrusted);
        assert!(!manager.has_tools());
        manager.shutdown();
    }

    #[test]
    fn trusting_a_disabled_server_respects_the_disable() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                entries: vec![stdio_entry("fix")],
                untrusted: ["fix".to_string()].into(),
                disabled: ["fix".to_string()].into(),
                project: "/proj".to_string(),
                ..Default::default()
            },
        );
        manager.start_connections();
        // The gate outranks the toggle while untrusted…
        assert_eq!(manager.snapshot()[0].status, McpServerStatus::Untrusted);
        // …and trusting hands the server back to the user's disable rather
        // than launching over it.
        manager.set_trusted(&["fix".to_string()], true);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(manager.snapshot()[0].status, McpServerStatus::Disabled);
        assert!(!manager.has_tools());
        manager.shutdown();
    }

    #[test]
    fn disable_persists_per_project_and_enable_reconnects() {
        let dir = tempfile::tempdir().unwrap();
        let user_file = dir.path().join("mcp.json");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                entries: vec![stdio_entry("fix")],
                project: "/proj".to_string(),
                user_file: Some(user_file.clone()),
                ..Default::default()
            },
        );
        manager.set_disabled("fix", true);
        let snapshot = manager.snapshot();
        assert_eq!(snapshot[0].status, McpServerStatus::Disabled);
        let written = std::fs::read_to_string(&user_file).unwrap();
        assert!(crate::mcp::parse_disabled(&written, "/proj").contains("fix"));
        // Enable clears the persisted set and reconnects.
        manager.set_disabled("fix", false);
        let written = std::fs::read_to_string(&user_file).unwrap();
        assert!(crate::mcp::parse_disabled(&written, "/proj").is_empty());
        wait_connected(&manager, "fix");
        manager.shutdown();
    }

    #[test]
    fn a_disabled_server_starts_disabled_and_offers_nothing() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                entries: vec![stdio_entry("fix")],
                disabled: ["fix".to_string()].into_iter().collect(),
                project: "/proj".to_string(),
                ..Default::default()
            },
        );
        manager.start_connections();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(manager.snapshot()[0].status, McpServerStatus::Disabled);
        assert!(manager.tool_specs().is_empty());
    }

    /// How the refresh fixtures' token endpoint answers a refresh grant.
    #[derive(Clone, Copy)]
    enum TokenMode {
        /// Rotate: issue `tok-{n+1}` / `rt-{n+1}`.
        Rotate,
        /// The AS's permanent verdict: 400 `invalid_grant`.
        InvalidGrant,
        /// A transient outage: 503.
        Outage,
        /// The token endpoint accepts the connection and never answers —
        /// what a wedged authorization server looks like from here.
        Hang,
    }

    struct OauthFixture {
        /// The MCP endpoint URL (`{base}/mcp`).
        url: String,
        /// The token endpoint URL (`{base}/token`).
        token_url: String,
        /// The token generation `/mcp` currently accepts (`tok-{required}`).
        required: Arc<std::sync::atomic::AtomicUsize>,
        /// How many requests `/mcp` answered 401.
        unauthorized: Arc<std::sync::atomic::AtomicUsize>,
    }

    /// A canned streamable-HTTP MCP server plus its OAuth token endpoint on
    /// one socket: `/mcp` serves the handshake and calls only under
    /// `Authorization: Bearer tok-{required}` (anything else 401s with a
    /// challenge), `/token` answers a refresh grant per [`TokenMode`].
    fn spawn_oauth_fixture(mode: TokenMode) -> OauthFixture {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let required = Arc::new(AtomicUsize::new(1));
        let issued_t = Arc::new(AtomicUsize::new(1));
        let unauthorized = Arc::new(AtomicUsize::new(0));
        let (required_t, unauthorized_t) = (Arc::clone(&required), Arc::clone(&unauthorized));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut content_length = 0usize;
                let mut bearer = String::new();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    let lower = header.to_ascii_lowercase();
                    if let Some(rest) = lower.strip_prefix("content-length:") {
                        content_length = rest.trim().parse().unwrap_or(0);
                    }
                    if let Some(rest) = lower.strip_prefix("authorization: bearer ") {
                        bearer = rest.trim().to_string();
                    }
                }
                let mut body = vec![0u8; content_length];
                let _ = reader.read_exact(&mut body);
                let body = String::from_utf8_lossy(&body).to_string();
                let respond = |stream: &mut std::net::TcpStream,
                               status: &str,
                               extra: &str,
                               body: &str| {
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                };
                if path.starts_with("/token") {
                    match mode {
                        TokenMode::Rotate => {
                            let n = issued_t.fetch_add(1, Ordering::SeqCst) + 1;
                            let body = format!(
                                r#"{{"access_token":"tok-{n}","refresh_token":"rt-{n}","token_type":"Bearer","expires_in":3600}}"#
                            );
                            respond(&mut stream, "200 OK", "", &body);
                        }
                        TokenMode::InvalidGrant => respond(
                            &mut stream,
                            "400 Bad Request",
                            "",
                            r#"{"error":"invalid_grant","error_description":"revoked"}"#,
                        ),
                        TokenMode::Outage => {
                            respond(&mut stream, "503 Service Unavailable", "", "down");
                        }
                        TokenMode::Hang => std::thread::sleep(Duration::from_secs(30)),
                    }
                    continue;
                }
                // The MCP endpoint: auth first, then the method dispatch.
                let expected = format!("tok-{}", required_t.load(Ordering::SeqCst));
                if bearer != expected {
                    unauthorized_t.fetch_add(1, Ordering::SeqCst);
                    let _ = stream.write_all(
                        b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer error=\"invalid_token\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    continue;
                }
                let id = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| v.get("id").and_then(Value::as_u64));
                let method = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| v.get("method").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default();
                match method.as_str() {
                    "initialize" => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "protocolVersion": "2025-06-18",
                                "capabilities": {"tools": {}},
                                "serverInfo": {"name": "authed", "version": "0"}
                            }
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", "", &body);
                    }
                    "notifications/initialized" => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                    "tools/list" => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"tools": [
                                {"name": "hello", "description": "Say hello.",
                                 "inputSchema": {"type": "object", "properties": {}}}
                            ]}
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", "", &body);
                    }
                    "tools/call" => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"content": [{"type": "text", "text": "authed hello"}]}
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", "", &body);
                    }
                    _ => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": {"code": -32601, "message": "method not found"}
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", "", &body);
                    }
                }
            }
        });
        OauthFixture {
            url: format!("http://127.0.0.1:{port}/mcp"),
            token_url: format!("http://127.0.0.1:{port}/token"),
            required,
            unauthorized,
        }
    }

    fn seed_tokens(path: &std::path::Path, fixture: &OauthFixture, expires_at: Option<u64>) {
        oauth::save_tokens(
            Some(path),
            &fixture.url,
            Some(&super::oauth::StoredTokens {
                access_token: "tok-1".to_string(),
                refresh_token: Some("rt-1".to_string()),
                expires_at,
                client_id: "cid".to_string(),
                client_secret: None,
                token_endpoint: fixture.token_url.clone(),
                scope: None,
            }),
        );
    }

    fn oauth_manager(fixture: &OauthFixture, auth_path: &std::path::Path) -> McpManager {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        McpManager::new(
            tx,
            McpSources {
                entries: vec![McpServerEntry {
                    name: "fix".to_string(),
                    config: McpServerConfig::Http {
                        url: fixture.url.clone(),
                        headers: Default::default(),
                        sse_fallback: false,
                    },
                    scope: McpScope::User,
                    config_path: "~/.alter-zero/mcp.json".to_string(),
                }],
                project: "/proj".to_string(),
                auth_path: Some(auth_path.to_path_buf()),
                ..Default::default()
            },
        )
    }

    fn wait_status(manager: &McpManager, name: &str, want: impl Fn(&McpServerStatus) -> bool) {
        for _ in 0..200 {
            let snapshot = manager.snapshot();
            let server = snapshot.iter().find(|s| s.name == name).unwrap();
            if want(&server.status) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "server never reached the wanted status (at {:?})",
            manager.snapshot()[0].status
        );
    }

    #[test]
    fn a_mid_session_401_refreshes_and_retries_without_reauth() {
        let dir = tempfile::tempdir().unwrap();
        let auth_path = dir.path().join("mcp-auth.json");
        let fixture = spawn_oauth_fixture(TokenMode::Rotate);
        // A fresh grant: the connect goes out on tok-1 untouched.
        seed_tokens(&auth_path, &fixture, Some(u64::MAX / 4));
        let manager = oauth_manager(&fixture, &auth_path);
        manager.start_connections();
        wait_connected(&manager, "fix");
        // The server rotates its expectation mid-session (the access token
        // "expired" server-side).
        fixture
            .required
            .store(2, std::sync::atomic::Ordering::SeqCst);
        let call = ToolCallRequest {
            id: "c1".to_string(),
            name: "mcp__fix__hello".to_string(),
            arguments: "{}".to_string(),
        };
        let outcome = manager.call_tool(&call, &CancelToken::new());
        assert!(outcome.ok, "{}", outcome.output);
        assert_eq!(outcome.output, "authed hello");
        // Exactly one 401 was seen — the refresh + retry absorbed it: the
        // server never left Connected and nobody was asked to re-auth.
        assert_eq!(
            fixture
                .unauthorized
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        let snapshot = manager.snapshot();
        assert_eq!(snapshot[0].status, McpServerStatus::Connected);
        // The rotated grant was persisted (rt-2 replaced rt-1).
        let stored = oauth::load_tokens(Some(&auth_path), &fixture.url).unwrap();
        assert_eq!(stored.access_token, "tok-2");
        assert_eq!(stored.refresh_token.as_deref(), Some("rt-2"));
        manager.shutdown();
    }

    #[test]
    fn a_stale_grant_refreshes_proactively_before_the_call() {
        let dir = tempfile::tempdir().unwrap();
        let auth_path = dir.path().join("mcp-auth.json");
        let fixture = spawn_oauth_fixture(TokenMode::Rotate);
        seed_tokens(&auth_path, &fixture, Some(u64::MAX / 4));
        let manager = oauth_manager(&fixture, &auth_path);
        manager.start_connections();
        wait_connected(&manager, "fix");
        // The grant goes stale on disk and the server moves on to tok-2:
        // the pre-call refresh must mint tok-2 *before* the request, so the
        // server never sees a dead token.
        seed_tokens(&auth_path, &fixture, Some(1));
        fixture
            .required
            .store(2, std::sync::atomic::Ordering::SeqCst);
        let call = ToolCallRequest {
            id: "c1".to_string(),
            name: "mcp__fix__hello".to_string(),
            arguments: "{}".to_string(),
        };
        let outcome = manager.call_tool(&call, &CancelToken::new());
        assert!(outcome.ok, "{}", outcome.output);
        assert_eq!(
            fixture
                .unauthorized
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the proactive refresh means no 401 ever happened"
        );
        manager.shutdown();
    }

    #[test]
    fn an_expired_token_at_connect_refreshes_and_connects() {
        let dir = tempfile::tempdir().unwrap();
        let auth_path = dir.path().join("mcp-auth.json");
        let fixture = spawn_oauth_fixture(TokenMode::Rotate);
        // tok-1 is long expired; the server already demands tok-2.
        seed_tokens(&auth_path, &fixture, Some(1));
        fixture
            .required
            .store(2, std::sync::atomic::Ordering::SeqCst);
        let manager = oauth_manager(&fixture, &auth_path);
        manager.start_connections();
        wait_connected(&manager, "fix");
        let snapshot = manager.snapshot();
        assert_eq!(
            snapshot[0].auth,
            Some(crate::mcp::McpAuthState::Authenticated)
        );
        manager.shutdown();
    }

    #[test]
    fn esc_reaps_a_call_whose_token_refresh_is_wedged() {
        // The transport contract (`docs/mcp.md`): every blocking wait polls
        // the turn's CancelToken on the 20 ms cadence, or Esc silently stops
        // working for as long as the peer takes. A refresh is a blocking
        // HTTP call on the tool thread, so it has to obey the same rule —
        // run inline, a wedged token endpoint ate Esc for the whole 20 s
        // HTTP timeout.
        let dir = tempfile::tempdir().unwrap();
        let auth_path = dir.path().join("mcp-auth.json");
        let fixture = spawn_oauth_fixture(TokenMode::Hang);
        seed_tokens(&auth_path, &fixture, Some(u64::MAX / 4));
        let manager = oauth_manager(&fixture, &auth_path);
        manager.start_connections();
        wait_connected(&manager, "fix");
        // The grant goes stale, so the next call refreshes first — into a
        // token endpoint that never answers.
        seed_tokens(&auth_path, &fixture, Some(1));
        let cancel = CancelToken::new();
        let reaper = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                cancel.cancel();
            })
        };
        let started = std::time::Instant::now();
        let call = ToolCallRequest {
            id: "c1".to_string(),
            name: "mcp__fix__hello".to_string(),
            arguments: "{}".to_string(),
        };
        let outcome = manager.call_tool(&call, &cancel);
        let elapsed = started.elapsed();
        reaper.join().unwrap();
        assert!(
            elapsed < Duration::from_secs(5),
            "Esc must reap the call while the refresh hangs (took {elapsed:?})"
        );
        assert!(!outcome.ok, "a cancelled call is not a success");
        manager.shutdown();
    }

    #[test]
    fn a_dead_refresh_token_resolves_needs_auth_and_clears_the_grant() {
        let dir = tempfile::tempdir().unwrap();
        let auth_path = dir.path().join("mcp-auth.json");
        let fixture = spawn_oauth_fixture(TokenMode::InvalidGrant);
        // The server rejects tok-1 and the AS rejects the refresh: the grant
        // is dead — needs-auth is the truthful state, and the dead grant is
        // cleared so it can't loop the failure.
        seed_tokens(&auth_path, &fixture, Some(u64::MAX / 4));
        fixture
            .required
            .store(2, std::sync::atomic::Ordering::SeqCst);
        let manager = oauth_manager(&fixture, &auth_path);
        manager.start_connections();
        wait_status(&manager, "fix", |status| {
            *status == McpServerStatus::NeedsAuth
        });
        assert!(oauth::load_tokens(Some(&auth_path), &fixture.url).is_none());
        let snapshot = manager.snapshot();
        assert_eq!(
            snapshot[0].auth,
            Some(crate::mcp::McpAuthState::NotAuthenticated)
        );
        manager.shutdown();
    }

    #[test]
    fn a_transient_refresh_failure_keeps_the_grant_and_never_demands_reauth() {
        let dir = tempfile::tempdir().unwrap();
        let auth_path = dir.path().join("mcp-auth.json");
        let fixture = spawn_oauth_fixture(TokenMode::Outage);
        seed_tokens(&auth_path, &fixture, Some(u64::MAX / 4));
        fixture
            .required
            .store(2, std::sync::atomic::Ordering::SeqCst);
        let manager = oauth_manager(&fixture, &auth_path);
        manager.start_connections();
        // The 503 from the token endpoint is a blip, not a verdict: the
        // server reads failed (Reconnect retries), never needs-auth, and the
        // grant survives untouched.
        wait_status(
            &manager,
            "fix",
            |status| matches!(status, McpServerStatus::Failed(detail) if detail.contains("token refresh failed")),
        );
        let stored = oauth::load_tokens(Some(&auth_path), &fixture.url).unwrap();
        assert_eq!(stored.access_token, "tok-1");
        assert_eq!(stored.refresh_token.as_deref(), Some("rt-1"));
        let snapshot = manager.snapshot();
        assert_eq!(
            snapshot[0].auth,
            Some(crate::mcp::McpAuthState::Authenticated)
        );
        manager.shutdown();
    }

    #[test]
    fn config_errors_surface_for_the_toast() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                errors: vec!["bad: entry needs a \"command\"".to_string()],
                ..Default::default()
            },
        );
        assert_eq!(manager.config_errors().len(), 1);
        assert!(!manager.has_servers());
    }
}
