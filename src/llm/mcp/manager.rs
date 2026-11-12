//! The session-owned MCP registry (`docs/tools/mcp.md`): every declared server's
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
    McpServerConfig, McpServerEntry, McpServerSnapshot, McpServerStatus, McpToolInfo,
    ServerIdentity, call_params_retrying, parse_call_result,
};
use crate::stream::CancelToken;

use super::client::{ConnectError, connect_with_helper};
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
    /// list, a toast reports the outcome. Never sent for a flow the user
    /// cancelled or a newer one superseded: neither has anything to report
    /// on a page that is no longer its own.
    AuthDone {
        server: String,
        ok: bool,
        detail: String,
    },
    /// A pasted redirect URL was turned away — not a redirect, or not this
    /// sign-in's — and the flow keeps waiting: the auth page shows why and
    /// takes another paste.
    AuthPasteRejected { server: String, reason: String },
}

/// Why the session is holding a project server back
/// (`docs/state/config-review.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpHold {
    /// New or changed: waiting for the startup review's answer
    /// ([`McpServerStatus::AwaitingReview`]).
    Review,
    /// Disabled in the review, or in `/mcp` — recorded in `trust.json`, not
    /// in the user's MCP file ([`McpServerStatus::Disabled`]).
    Declined,
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
    /// The project servers the review is holding (`docs/state/config-review.md`):
    /// never launched until enabled ([`McpManager::set_approved`]).
    pub held: BTreeMap<String, McpHold>,
    /// The absolute cwd — the disabled sets' project key.
    pub project: String,
    /// The user config file — where a disable persists. `None` = no
    /// persistence (still toggles for the session).
    pub user_file: Option<PathBuf>,
    /// The OAuth token store (`{config_home}/mcp-auth.json`).
    pub auth_path: Option<PathBuf>,
    /// The session cwd stdio children run in.
    pub cwd: Option<PathBuf>,
    /// This binary, for the launch tier that re-execs it to detach a stdio
    /// server from the terminal where no `setsid` binary exists — resolved at
    /// the boundary, never in a test binary (`docs/tools/tty-detach.md`).
    pub detach_helper: Option<PathBuf>,
    pub startup_timeout: Duration,
    pub tool_timeout: Duration,
    /// What opens the authorize URL in a browser — `None` is the system
    /// launcher; a test passes one that opens nothing.
    pub open_browser: Option<fn(&str)>,
    /// The session's own credentials' variable names (its providers' API
    /// keys): a project's remote server may not have them expanded into its
    /// URL or headers ([`crate::mcp::resolve_config`]).
    pub protected_env: std::collections::BTreeSet<String>,
}

/// The default per-server connect budget (both references use 30 s;
/// `ALTER_ZERO_MCP_STARTUP_TIMEOUT_MS` overrides).
pub const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// The default per-call budget (`ALTER_ZERO_MCP_TOOL_TIMEOUT_MS` overrides).
pub const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(120);

/// The cancel-poll cadence every blocking wait shares with the transports.
const POLL: Duration = Duration::from_millis(20);

/// How long a stdio server gets at each shutdown step when the session
/// quits — to exit once its stdin closes, then once it is sent `SIGTERM` —
/// shorter than a mid-session stop's, since the user is waiting on it.
const QUIT_GRACE: Duration = Duration::from_millis(300);

/// The least time between two automatic restarts of one server that died
/// under a call ([`McpManager`]'s `restart_dead`).
const RESTART_COOLDOWN: Duration = Duration::from_secs(30);

/// How many times a call answered with a `requestState`-only
/// `input_required` is repeated before its result is reported.
const MAX_STATE_RETRIES: u32 = 3;

/// The pause before the second and later of those repeats, growing by this
/// much each time — the first is immediate, as the spec allows.
const STATE_RETRY_PAUSE: Duration = Duration::from_millis(250);

/// Lock a server's connection, waiting for a call that holds it while
/// polling `cancel` on the transports' cadence — `None` once the turn is
/// cancelled, the call then never sent.
fn lock_unless_cancelled<'a>(
    transport: &'a Mutex<Transport>,
    cancel: &CancelToken,
) -> Option<std::sync::MutexGuard<'a, Transport>> {
    loop {
        if cancel.is_cancelled() {
            return None;
        }
        match transport.try_lock() {
            Ok(guard) => return Some(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(POLL),
        }
    }
}

/// One `tools/call` over a held transport. A `requestState`-only
/// `input_required` is the server asking for the same call again (load
/// shedding, 2026-07-28's MRTR): answered by repeating it with the state
/// echoed — a few times, a little further apart each time, inside `timeout`.
fn call_once(
    transport: &mut Transport,
    tool: &str,
    arguments: &Value,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<Value, TransportError> {
    let started = std::time::Instant::now();
    let mut state: Option<String> = None;
    let mut retries: u32 = 0;
    loop {
        let budget = timeout.saturating_sub(started.elapsed());
        let result = transport.request(
            "tools/call",
            call_params_retrying(tool, arguments, state.as_deref()),
            budget,
            cancel,
        )?;
        match crate::mcp::input_required(&result) {
            Some(crate::mcp::InputRequired::Retry { request_state })
                if retries < MAX_STATE_RETRIES =>
            {
                let pause = STATE_RETRY_PAUSE * retries;
                retries += 1;
                if pause >= timeout.saturating_sub(started.elapsed())
                    || !sleep_unless_cancelled(pause, cancel)
                {
                    return Ok(result);
                }
                state = Some(request_state);
            }
            _ => return Ok(result),
        }
    }
}

/// Sleep `pause`, polling `cancel` — `false` when the turn was cancelled.
fn sleep_unless_cancelled(pause: Duration, cancel: &CancelToken) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < pause {
        if cancel.is_cancelled() {
            return false;
        }
        std::thread::sleep(POLL.min(pause.saturating_sub(started.elapsed())));
    }
    !cancel.is_cancelled()
}

/// Run blocking work on a worker thread while polling `cancel` on the 20 ms
/// cadence — the transport's own contract (`docs/tools/mcp.md`), which anything
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
    /// The config a launch uses — the declared one with its `${VAR}`
    /// references expanded from the session's environment
    /// ([`crate::mcp::resolve_config`]) — or why there is none: a variable
    /// neither set nor defaulted, or a project's remote server asking for the
    /// session's own credentials; the server stays `Failed` with it.
    /// `entry.config` stays as written, for the `/mcp` page.
    resolved: Result<McpServerConfig, String>,
    status: McpServerStatus,
    identity: Option<ServerIdentity>,
    tools: Vec<McpToolInfo>,
    /// The live transport — its own lock, so a running tool call never
    /// blocks a snapshot.
    transport: Option<Arc<Mutex<Transport>>>,
    /// The listed tools the model is not offered, and why — the live
    /// connection's facts, like `tools`.
    excluded: Vec<crate::mcp::ExcludedTool>,
    /// When a call last restarted this server after it died under it — the
    /// crash-loop guard.
    last_restart: Option<std::time::Instant>,
    /// Why this server can never be offered under its name
    /// ([`crate::mcp::server_name_problems`]) — it stays `Failed` with this
    /// reason, whoever asks it to connect.
    name_problem: Option<String>,
    has_tokens: bool,
    /// The last 401's `WWW-Authenticate` challenge. Discovery runs blind
    /// without it: the challenge names the `resource_metadata` URL a server
    /// publishes off the well-known path, and the `scope` the resource
    /// actually wants (`docs/tools/mcp.md`).
    challenge: Option<String>,
    /// Bumped on disable/reconnect so a stale connect thread's result is
    /// dropped instead of resurrecting an old state.
    generation: u64,
    /// A listing prompted by the server's change notification is running —
    /// a burst of them is one listing, plus one more if any arrived while it
    /// ran (`relist_again`), never one each.
    relisting: bool,
    relist_again: bool,
}

impl ServerState {
    /// The resolved config's URL — a remote server's address, which its
    /// tokens are kept under; `None` for a stdio server, or one that
    /// couldn't be resolved.
    fn url(&self) -> Option<&str> {
        self.resolved.as_ref().ok()?.url()
    }

    /// Why this server can't be connected at all, whoever asks: a name it
    /// can't be offered under, or a config that couldn't be resolved.
    fn blocked(&self) -> Option<String> {
        self.name_problem
            .clone()
            .or_else(|| self.resolved.as_ref().err().cloned())
    }

    /// Every tool this server offers the model, under the wire name it is
    /// offered by — the one derivation the specs, the fingerprint, the
    /// descriptions and the call routing all read ([`crate::mcp::wire_names`]),
    /// so no two of them can disagree about a name. Empty unless connected.
    fn offered(&self) -> Vec<(String, &McpToolInfo)> {
        if self.status != McpServerStatus::Connected {
            return Vec::new();
        }
        let mut by_name: std::collections::HashMap<&str, &McpToolInfo> =
            std::collections::HashMap::new();
        for tool in &self.tools {
            by_name.entry(tool.name.as_str()).or_insert(tool);
        }
        let names: Vec<&str> = self.tools.iter().map(|tool| tool.name.as_str()).collect();
        crate::mcp::wire_names(&self.entry.name, &names)
            .into_iter()
            .filter_map(|(wire, raw)| by_name.get(raw).map(|tool| (wire, *tool)))
            .collect()
    }

    /// Forget the live connection and everything it told us — the transport
    /// (dropping it ends a stdio child), the identity and the tool list —
    /// and supersede any connect still in flight. What a disable, a
    /// needs-auth refusal and a fresh connect all start from: the `/mcp`
    /// page shows the *live* connection's facts, so none may outlive it.
    fn drop_connection(&mut self) {
        self.generation += 1;
        self.transport = None;
        self.identity = None;
        self.tools.clear();
        self.excluded.clear();
    }
}

struct Inner {
    servers: Vec<ServerState>,
    /// Whether MCP runs at all this session — the `/settings` **MCP** row
    /// (`docs/tools/mcp.md`). Off, nothing connects whoever asks, and every
    /// server the user hasn't switched off reads [`McpServerStatus::Off`].
    active: bool,
    errors: Vec<String>,
    disabled: std::collections::BTreeSet<String>,
    held: BTreeMap<String, McpHold>,
    project: String,
    user_file: Option<PathBuf>,
    auth_path: Option<PathBuf>,
    cwd: Option<PathBuf>,
    detach_helper: Option<PathBuf>,
    startup_timeout: Duration,
    tool_timeout: Duration,
    /// The sign-in running now, while one does.
    auth_flow: Option<AuthFlow>,
    /// The id the next sign-in gets.
    next_auth_flow: u64,
    open_browser: fn(&str),
}

/// The sign-in a manager is running: its id (so a superseded flow ending
/// can tell it is no longer the one), its paste channel and its cancel.
struct AuthFlow {
    id: u64,
    paste: mpsc::Sender<String>,
    cancel: CancelToken,
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
        let names: Vec<&str> = sources.entries.iter().map(|e| e.name.as_str()).collect();
        let problems = crate::mcp::server_name_problems(&names);
        let servers = sources
            .entries
            .into_iter()
            .zip(problems)
            .map(|(entry, name_problem)| {
                let disabled = sources.disabled.contains(&entry.name);
                let held = sources.held.get(&entry.name).copied();
                // The environment is the session's own, fixed at launch, so
                // one expansion holds for every connect.
                let resolved = crate::mcp::resolve_config(
                    &entry.config,
                    entry.scope,
                    &|name| std::env::var(name).ok(),
                    &|name| sources.protected_env.contains(name),
                );
                let has_tokens = resolved
                    .as_ref()
                    .ok()
                    .and_then(McpServerConfig::url)
                    .is_some_and(|url| oauth::load_tokens(auth_path.as_deref(), url).is_some());
                let blocked = name_problem
                    .clone()
                    .or_else(|| resolved.as_ref().err().cloned());
                ServerState {
                    // The review outranks the user's own toggle: a held
                    // server may not run either way, and only an answer in
                    // the review (or Enable in `/mcp`) releases it. A server
                    // that can't be connected at all is refused next.
                    status: match (held, blocked) {
                        (Some(McpHold::Review), _) => McpServerStatus::AwaitingReview,
                        (Some(McpHold::Declined), _) => McpServerStatus::Disabled,
                        (None, _) if disabled => McpServerStatus::Disabled,
                        (None, Some(problem)) => McpServerStatus::Failed(problem),
                        (None, None) => McpServerStatus::Pending,
                    },
                    resolved,
                    identity: None,
                    tools: Vec::new(),
                    excluded: Vec::new(),
                    transport: None,
                    last_restart: None,
                    name_problem,
                    has_tokens,
                    challenge: None,
                    generation: 0,
                    relisting: false,
                    relist_again: false,
                    entry,
                }
            })
            .collect();
        Self {
            inner: Arc::new(Mutex::new(Inner {
                servers,
                active: true,
                errors: sources.errors,
                disabled: sources.disabled,
                held: sources.held,
                project: sources.project,
                user_file: sources.user_file,
                auth_path,
                cwd: sources.cwd,
                detach_helper: sources.detach_helper,
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
                auth_flow: None,
                next_auth_flow: 0,
                open_browser: sources.open_browser.unwrap_or(open_browser),
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

    /// Turn MCP on or off for the whole session — the `/settings` **MCP**
    /// row (`docs/tools/mcp.md`).
    ///
    /// Off stops every server — its connection dropped, a stdio child
    /// stopped with it, a connect still in flight superseded — cancels a
    /// sign-in, and marks each server [`McpServerStatus::Off`], so the model
    /// is offered nothing and nothing connects again whoever asks. A server
    /// the user switched off in `/mcp`, or one the startup review is holding,
    /// keeps that state: the session switch never rewrites a server's own.
    ///
    /// On connects every server that was off — a fresh start, the startup
    /// kick's, so one that had failed or wanted a sign-in is asked again —
    /// and a server that can never start reports its reason again.
    pub fn set_active(&self, active: bool) {
        let cancelled = {
            let mut inner = self.lock();
            if inner.active == active {
                return;
            }
            inner.active = active;
            if active {
                for server in &mut inner.servers {
                    if server.status == McpServerStatus::Off {
                        server.status = server
                            .blocked()
                            .map_or(McpServerStatus::Pending, McpServerStatus::Failed);
                    }
                }
                None
            } else {
                for server in &mut inner.servers {
                    if !matches!(
                        server.status,
                        McpServerStatus::Disabled | McpServerStatus::AwaitingReview
                    ) {
                        server.drop_connection();
                        server.status = McpServerStatus::Off;
                    }
                }
                inner.auth_flow.take()
            }
        };
        if let Some(flow) = cancelled {
            flow.cancel.cancel();
        }
        self.notify(McpEvent::Changed);
        if active {
            self.start_connections();
        }
    }

    /// Does MCP run at all this session (the `/settings` **MCP** row)?
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.lock().active
    }

    /// Connect (or re-connect) one server on a worker thread — never one the
    /// review is holding, and nothing at all while MCP is off, whoever asks.
    fn spawn_connect(&self, name: &str) {
        let (config, auth_path, cwd, timeout, generation) = {
            let mut inner = self.lock();
            if inner.held.contains_key(name) {
                return;
            }
            // MCP is off for the session: the server waits, off, for it to
            // come back on — unless its own switch says off, which it keeps.
            if !inner.active {
                let status = if inner.disabled.contains(name) {
                    McpServerStatus::Disabled
                } else {
                    McpServerStatus::Off
                };
                if let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == name) {
                    server.drop_connection();
                    server.status = status;
                }
                drop(inner);
                self.notify(McpEvent::Changed);
                return;
            }
            let (auth_path, cwd, timeout) = (
                inner.auth_path.clone(),
                (inner.cwd.clone(), inner.detach_helper.clone()),
                inner.startup_timeout,
            );
            let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == name) else {
                return;
            };
            if let Some(problem) = server.blocked() {
                server.status = McpServerStatus::Failed(problem);
                return;
            }
            // Drop the old transport now (kills a stdio child) — the new
            // connect starts clean, and so does what we say about the
            // server: the revision, capabilities and tools on the `/mcp`
            // page are the *live* connection's facts, so a reconnect that
            // never initializes must not leave the last one's behind.
            server.drop_connection();
            server.status = McpServerStatus::Pending;
            let generation = server.generation;
            server.has_tokens = server
                .url()
                .is_some_and(|url| oauth::load_tokens(auth_path.as_deref(), url).is_some());
            let Ok(config) = server.resolved.clone() else {
                return; // `blocked` said so above
            };
            (config, auth_path, cwd, timeout, generation)
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
            let (cwd, helper) = cwd;
            // A remote connect presenting nothing: no stored grant, no
            // `Authorization` header in the entry.
            let mut anonymous =
                config.url().is_some() && bearer.is_none() && !config.has_auth_header();
            let names_client = config
                .oauth()
                .is_some_and(|oauth| oauth.client_id.is_some());
            // Asked beside the handshake rather than after it, so a server
            // that publishes no metadata connects no slower.
            let probe = config
                .url()
                .filter(|_| anonymous && !names_client)
                .map(|url| {
                    let url = url.to_string();
                    std::thread::spawn(move || oauth::protected_without_registration(&url))
                });
            let launch = |bearer: Option<String>| {
                connect_with_helper(
                    &config,
                    bearer,
                    cwd.as_deref(),
                    helper.as_deref(),
                    timeout,
                    &cancel,
                )
            };
            let mut result = launch(bearer);
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
                        anonymous = false;
                        result = launch(Some(tokens.access_token));
                    }
                    Err(oauth::RefreshFailure::Transient(detail)) => {
                        transient_refresh = Some(detail);
                    }
                    Err(_) => {} // no grant, or a cleared dead one: needs-auth is truthful
                }
            }
            // An anonymous connect is no proof the calls will be served:
            // Google's MCP servers list their tools to anyone and refuse
            // every call. A server whose entry names its OAuth client, or an
            // OAuth resource whose authorization server can't register this
            // app, waits for a sign-in instead of offering the model tools
            // that only fail (`docs/tools/mcp.md`).
            if result.is_ok()
                && anonymous
                && (names_client || probe.is_some_and(|probe| probe.join().unwrap_or(false)))
            {
                result = Err(ConnectError::NeedsAuth(None));
            }
            let mut inner = manager.lock();
            let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == name) else {
                return;
            };
            if server.generation != generation {
                return; // a reconnect/disable superseded this attempt
            }
            match result {
                Ok(mut connection) => {
                    // The server's change notifications list its tools again;
                    // one that promised them is asked for them.
                    let subscribe = connection.identity.tools_list_changed;
                    connection
                        .transport
                        .watch_changes(manager.change_sink(&name, generation), subscribe);
                    server.identity = Some(connection.identity);
                    server.tools = connection.tools;
                    server.excluded = connection.excluded;
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

    /// What a connection's change notifications reach: a tool-list change
    /// lists `name` again (`relist`), while the connection of `generation`
    /// is still the one in use. Holds the manager weakly — the transport
    /// lives inside the manager, and a strong handle there would keep both
    /// alive for good.
    fn change_sink(&self, name: &str, generation: u64) -> super::transport::ChangeSink {
        let inner = Arc::downgrade(&self.inner);
        let events = self.events.clone();
        let name = name.to_string();
        Arc::new(move |method: &str| {
            if method != crate::mcp::TOOLS_LIST_CHANGED {
                return;
            }
            if let Some(inner) = inner.upgrade() {
                McpManager {
                    inner,
                    events: events.clone(),
                }
                .relist(&name, generation);
            }
        })
    }

    /// List a server's tools again after it said they changed, on a worker —
    /// the notification arrives on a reader thread that must not wait, and a
    /// call may hold the connection. The fresh listing replaces the tools
    /// the model is offered (the event loop re-attaches the backend when the
    /// set moved), and a burst of notifications costs one listing, plus one
    /// more for any that arrived while it ran.
    fn relist(&self, name: &str, generation: u64) {
        let (transport, budget) = {
            let mut inner = self.lock();
            let budget = inner.startup_timeout;
            let Some(server) = inner
                .servers
                .iter_mut()
                .find(|s| s.entry.name == name && s.generation == generation)
            else {
                return;
            };
            if server.status != McpServerStatus::Connected {
                return;
            }
            if server.relisting {
                server.relist_again = true;
                return;
            }
            let Some(transport) = server.transport.clone() else {
                return;
            };
            server.relisting = true;
            (transport, budget)
        };
        let manager = self.clone();
        let name = name.to_string();
        std::thread::spawn(move || {
            loop {
                let listing = transport.lock().map_err(|_| ()).and_then(|mut transport| {
                    let until = std::time::Instant::now() + budget;
                    super::client::list_tools(&mut transport, until, &CancelToken::new())
                        .map_err(|_| ())
                });
                let mut inner = manager.lock();
                let Some(server) = inner
                    .servers
                    .iter_mut()
                    .find(|s| s.entry.name == name && s.generation == generation)
                else {
                    return; // reconnected or disabled meanwhile
                };
                // A failed listing keeps the tools the server had: they are
                // the best knowledge there is.
                if let Ok((tools, excluded)) = listing {
                    server.tools = tools;
                    server.excluded = excluded;
                }
                if std::mem::take(&mut server.relist_again) {
                    continue;
                }
                server.relisting = false;
                drop(inner);
                manager.notify(McpEvent::Changed);
                return;
            }
        });
    }

    /// Reconnect one server (the `/mcp` action; also Enable's follow-up).
    pub fn reconnect(&self, name: &str) {
        self.spawn_connect(name);
    }

    /// Enable/disable one server for this project — at once for the session,
    /// and persisted in the user file over the file as it is now (one name,
    /// read-modify-write, written atomically). An error says the change could
    /// not be saved: the file doesn't parse (it is left alone rather than
    /// replaced) or couldn't be written, so it lasts this session only.
    pub fn set_disabled(&self, name: &str, disabled: bool) -> Result<(), String> {
        let (user_file, project) = {
            let mut inner = self.lock();
            if disabled {
                inner.disabled.insert(name.to_string());
            } else {
                inner.disabled.remove(name);
            }
            if let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == name) {
                server.generation += 1;
                if disabled {
                    server.drop_connection();
                    server.status = McpServerStatus::Disabled;
                }
            }
            (inner.user_file.clone(), inner.project.clone())
        };
        self.notify(McpEvent::Changed);
        if !disabled && !self.lock().held.contains_key(name) {
            self.spawn_connect(name);
        }
        match user_file {
            Some(path) => persist_disabled(&path, &project, name, disabled),
            None => Ok(()),
        }
    }

    /// The review's seam (`docs/state/config-review.md`): enable or disable one
    /// project server. Enabling releases the hold and hands the server back
    /// to its ordinary lifecycle — it connects, unless the user's own
    /// per-directory disable still holds it — while a server already running
    /// is left alone. Disabling holds it as declined: the transport goes (a
    /// stdio child dies with it) and its tools are withdrawn. Unlike
    /// [`Self::set_disabled`], nothing persists here — the decision lives in
    /// `trust.json`, written at the boundary, never in the user's MCP file.
    pub fn set_approved(&self, name: &str, approved: bool) {
        let connect = {
            let mut inner = self.lock();
            let user_disabled = inner.disabled.contains(name);
            if approved {
                inner.held.remove(name);
            } else {
                inner.held.insert(name.to_string(), McpHold::Declined);
            }
            let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == name) else {
                return;
            };
            if approved {
                let was_held = matches!(
                    server.status,
                    McpServerStatus::AwaitingReview | McpServerStatus::Disabled
                );
                if was_held && user_disabled {
                    server.status = McpServerStatus::Disabled;
                }
                was_held && !user_disabled
            } else {
                server.drop_connection();
                server.status = McpServerStatus::Disabled;
                false
            }
        };
        self.notify(McpEvent::Changed);
        if connect {
            self.spawn_connect(name);
        }
    }

    /// The servers waiting for the startup review's answer.
    #[must_use]
    pub fn awaiting_review(&self) -> Vec<String> {
        self.lock()
            .servers
            .iter()
            .filter(|s| s.status == McpServerStatus::AwaitingReview)
            .map(|s| s.entry.name.clone())
            .collect()
    }

    /// Is the review holding `name` back — waiting for its answer, or
    /// declined?
    #[must_use]
    pub fn is_held(&self, name: &str) -> bool {
        self.lock().held.contains_key(name)
    }

    /// Is `name` in the user file's per-directory disabled set — the `/mcp`
    /// toggle a project server could be turned off with before the review
    /// existed (`docs/state/config-review.md`)?
    #[must_use]
    pub fn is_user_disabled(&self, name: &str) -> bool {
        self.lock().disabled.contains(name)
    }

    /// Delete a server's stored OAuth tokens (the `/mcp` "Clear
    /// authentication" action).
    ///
    /// The live connection goes with the grant: it still carries the old
    /// bearer, so a connected (or failed) server reconnects at once — and
    /// what the server sees matches what the page says. One that now needs a
    /// login says so. An error when the store couldn't be rewritten — the
    /// grant is still there, and saying "cleared" would be a lie.
    pub fn clear_auth(&self, name: &str) -> Result<(), String> {
        let (url, auth_path) = {
            let inner = self.lock();
            let Some(server) = inner.servers.iter().find(|s| s.entry.name == name) else {
                return Ok(());
            };
            (server.url().map(str::to_string), inner.auth_path.clone())
        };
        // The store's I/O runs outside the manager's lock: its file lock may
        // wait on another session's write.
        if let Some(url) = &url {
            oauth::save_tokens(auth_path.as_deref(), url, None)?;
        }
        let reconnect = {
            let mut inner = self.lock();
            let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == name) else {
                return Ok(());
            };
            server.has_tokens = false;
            matches!(
                server.status,
                McpServerStatus::Connected | McpServerStatus::Failed(_)
            )
        };
        self.notify(McpEvent::Changed);
        if reconnect {
            self.spawn_connect(name);
        }
        Ok(())
    }

    /// Start the interactive OAuth flow for one server on a worker thread.
    /// The authorize URL and the outcome report on the event channel; a
    /// pasted redirect URL arrives via [`submit_auth_paste`]. A new flow
    /// supersedes one still running: the old one is cancelled, and when it
    /// ends it touches nothing of the new one's — its paste channel, its
    /// cancel, its page.
    ///
    /// [`submit_auth_paste`]: McpManager::submit_auth_paste
    pub fn authenticate(&self, name: &str) {
        let (url, challenge, auth_path, open_browser, configured) = {
            let inner = self.lock();
            if !inner.active {
                self.notify(McpEvent::AuthDone {
                    server: name.to_string(),
                    ok: false,
                    detail: "MCP is off for this session — turn it on in /settings".to_string(),
                });
                return;
            }
            let Some(server) = inner.servers.iter().find(|s| s.entry.name == name) else {
                return;
            };
            if let Err(problem) = &server.resolved {
                self.notify(McpEvent::AuthDone {
                    server: name.to_string(),
                    ok: false,
                    detail: problem.clone(),
                });
                return;
            }
            let Some(url) = server.url().map(str::to_string) else {
                self.notify(McpEvent::AuthDone {
                    server: name.to_string(),
                    ok: false,
                    detail: "a stdio server has no authentication".to_string(),
                });
                return;
            };
            (
                url,
                server.challenge.clone(),
                inner.auth_path.clone(),
                inner.open_browser,
                server
                    .resolved
                    .as_ref()
                    .ok()
                    .and_then(McpServerConfig::oauth)
                    .cloned(),
            )
        };
        // The entry's pre-registered client, its secret the file's or the
        // one `mcp add --client-secret` kept with the tokens.
        let client = configured.map(|mut client| {
            if client.client_secret.is_none() {
                client.client_secret = oauth::load_client_secret(auth_path.as_deref(), &url);
            }
            client
        });
        let (paste_tx, paste_rx) = mpsc::channel();
        let cancel = CancelToken::new();
        let id = {
            let mut inner = self.lock();
            if let Some(old) = inner.auth_flow.take() {
                old.cancel.cancel();
            }
            let id = inner.next_auth_flow;
            inner.next_auth_flow += 1;
            inner.auth_flow = Some(AuthFlow {
                id,
                paste: paste_tx,
                cancel: cancel.clone(),
            });
            id
        };
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
                fn on_paste_rejected(&self, reason: &str) {
                    self.manager.notify(McpEvent::AuthPasteRejected {
                        server: self.server.clone(),
                        reason: reason.to_string(),
                    });
                }
            }
            let progress = Progress {
                manager: manager.clone(),
                server: name.clone(),
            };
            let result = oauth::run_auth_flow(
                &oauth::SignIn {
                    server_url: &url,
                    challenge: challenge.as_deref(),
                    store_path: auth_path.as_deref(),
                    client: client.as_ref(),
                },
                &paste_rx,
                &progress,
                &open_browser,
                &cancel,
            );
            let current = {
                let mut inner = manager.lock();
                let current = inner.auth_flow.as_ref().is_some_and(|flow| flow.id == id);
                if current {
                    inner.auth_flow = None;
                }
                // A grant is a grant, whoever was waiting for it.
                if result.is_ok()
                    && let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == name)
                {
                    server.has_tokens = true;
                }
                current
            };
            match result {
                Ok(()) => {
                    manager.notify(McpEvent::AuthDone {
                        server: name.clone(),
                        ok: true,
                        detail: format!("Authenticated with {name}"),
                    });
                    manager.spawn_connect(&name);
                }
                // The user's own Esc, or a newer flow's page: nothing to say.
                Err(_) if !current || cancel.is_cancelled() => manager.notify(McpEvent::Changed),
                Err(detail) => manager.notify(McpEvent::AuthDone {
                    server: name.clone(),
                    ok: false,
                    detail,
                }),
            }
        });
    }

    /// Hand the running auth flow a pasted redirect URL (the `URL >`
    /// fallback).
    pub fn submit_auth_paste(&self, text: &str) {
        let sender = self
            .lock()
            .auth_flow
            .as_ref()
            .map(|flow| flow.paste.clone());
        if let Some(sender) = sender {
            let _ = sender.send(text.to_string());
        }
    }

    /// Abandon the running auth flow (Esc on the auth page).
    pub fn cancel_auth(&self) {
        let flow = self.lock().auth_flow.take();
        if let Some(flow) = flow {
            flow.cancel.cancel();
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
                excluded: server.excluded.clone(),
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
            for (wire, tool) in server.offered() {
                // The repaired copy — the root shape providers insist on —
                // never the server's own, which can fail every request.
                let (parameters, note) = crate::mcp::model_input_schema(&tool.input_schema);
                let description = match note {
                    Some(note) if tool.description.trim().is_empty() => note.to_string(),
                    Some(note) => format!("{note} {}", tool.description),
                    None => tool.description.clone(),
                };
                out.push(crate::llm::tools::function_spec(
                    &wire,
                    &description,
                    parameters,
                ));
            }
        }
        // A reconnect may enumerate the identical tools in another order.
        // Tool definitions precede the conversation in provider prompts, so
        // preserving a stable order keeps the whole cached prefix reusable.
        out.sort_by(|left, right| {
            left["function"]["name"]
                .as_str()
                .cmp(&right["function"]["name"].as_str())
        });
        out
    }

    /// Each server's own instructions the model is given
    /// (`mcp::instructions_section`, `docs/tools/mcp.md`): a connected server with
    /// at least one offered tool that `allowed` admits by wire name — every
    /// one for the lead, a subagent's own allowlist for its briefing — and
    /// instructions to give, in name order so the reminder holds still
    /// across reconnects.
    #[must_use]
    pub fn instructions(&self, allowed: &dyn Fn(&str) -> bool) -> Vec<(String, String)> {
        let inner = self.lock();
        let mut out: Vec<(String, String)> = inner
            .servers
            .iter()
            .filter(|server| server.offered().iter().any(|(wire, _)| allowed(wire)))
            .filter_map(|server| {
                let text = server.identity.as_ref()?.instructions.as_deref()?.trim();
                (!text.is_empty()).then(|| (server.entry.name.clone(), text.to_string()))
            })
            .collect();
        out.sort();
        out
    }

    /// The server's own one-line description of the tool `wire` names — the
    /// dim row the permission prompt shows under the call (`docs/tools/mcp.md`).
    /// `None` for a tool no connected server offers.
    #[must_use]
    pub fn tool_description(&self, wire: &str) -> Option<String> {
        let inner = self.lock();
        inner.servers.iter().find_map(|server| {
            server
                .offered()
                .into_iter()
                .find(|(offered, _)| offered == wire)
                .map(|(_, tool)| tool.description.clone())
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
        // Each offered tool's name **and** a digest of its description and
        // schema: a reconnect that changed only a schema — a stdio server's
        // own dev loop — must rebuild the backend too.
        let mut names: Vec<String> = inner
            .servers
            .iter()
            .flat_map(|s| {
                s.offered().into_iter().map(|(wire, tool)| {
                    use sha2::{Digest, Sha256};
                    let shape = serde_json::json!([tool.description, tool.input_schema]);
                    let digest = Sha256::digest(shape.to_string().as_bytes());
                    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
                    format!("{wire} {hex}")
                })
            })
            .collect();
        names.sort();
        names
    }

    /// Execute one `mcp__server__tool` call — the execute-closure arm
    /// (`docs/tools/mcp.md`). Every failure is a recoverable red outcome; only a
    /// 401 additionally flips the server to needs-auth.
    #[must_use]
    pub fn call_tool(&self, call: &ToolCallRequest, cancel: &CancelToken) -> ToolOutcome {
        let lookup = {
            let inner = self.lock();
            // A turn that began with the tools offered can still call one
            // after MCP was switched off under it.
            if !inner.active {
                return ToolOutcome::error(
                    "MCP is off for this session — ask the user to turn it on in /settings",
                );
            }
            inner.servers.iter().find_map(|server| {
                let raw_tool = server
                    .offered()
                    .into_iter()
                    .find(|(wire, _)| *wire == call.name)
                    .map(|(_, tool)| tool.name.clone())?;
                Some((
                    server.entry.name.clone(),
                    raw_tool,
                    server.transport.clone()?,
                    server.url().map(str::to_string),
                    inner.auth_path.clone(),
                    inner.tool_timeout,
                    server.generation,
                ))
            })
        };
        let Some((server_name, raw_tool, transport, server_url, auth_path, timeout, generation)) =
            lookup
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
        // What a `-32020 HeaderMismatch` made us list again, for the server
        // state once the call is over.
        let mut relisted: Option<(Vec<McpToolInfo>, Vec<crate::mcp::ExcludedTool>)> = None;
        // Set when the connection turned out dead under the call — a stdio
        // server that exited, a legacy stream that ended.
        let mut died = false;
        let mut request = |refreshed_bearer: Option<String>| {
            // Another call to this server holds its connection: wait for it
            // polling the turn's cancel, never blind — and a call cancelled
            // while it waited is never sent.
            let Some(mut transport) = lock_unless_cancelled(&transport, cancel) else {
                return Err(TransportError::Cancelled);
            };
            if let Some(bearer) = refreshed_bearer {
                transport.set_bearer(Some(bearer));
            }
            let started = std::time::Instant::now();
            let outcome = match call_once(&mut transport, &raw_tool, &arguments, timeout, cancel) {
                // The server wants headers the listing we hold didn't mark —
                // its schemas changed. The spec's answer: list again, then
                // retry the call once with what the fresh listing says.
                Err(TransportError::Rpc(error))
                    if error.code == -32020 && transport.mirrors_param_headers() =>
                {
                    let until =
                        std::time::Instant::now() + timeout.saturating_sub(started.elapsed());
                    match super::client::list_tools(&mut transport, until, cancel) {
                        Ok(listing) => {
                            relisted = Some(listing);
                            let budget = timeout.saturating_sub(started.elapsed());
                            call_once(&mut transport, &raw_tool, &arguments, budget, cancel)
                        }
                        Err(_) => Err(TransportError::Rpc(error)),
                    }
                }
                other => other,
            };
            if matches!(outcome, Err(TransportError::Failed(_))) && !transport.is_alive() {
                died = true;
            }
            outcome
        };
        // Proactive: a grant inside its expiry skew refreshes *before* the
        // call, so the token never goes over the wire dead — the 401 arm
        // below stays as the backstop for revocation, clock skew, and grants
        // with no known expiry (`docs/tools/mcp.md`). Off-thread with the turn's
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
        if died && let Err(TransportError::Failed(detail)) = &result {
            return self.restart_dead(&server_name, generation, detail);
        }
        if let Some((tools, excluded)) = relisted {
            {
                let mut inner = self.lock();
                if let Some(server) = inner
                    .servers
                    .iter_mut()
                    .find(|s| s.entry.name == server_name && s.generation == generation)
                {
                    server.tools = tools;
                    server.excluded = excluded;
                }
            }
            self.notify(McpEvent::Changed);
        }
        match result {
            Ok(result) => {
                let outcome = parse_call_result(&result);
                let (text, truncated) = crate::llm::tools::truncate_output(
                    outcome.text,
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
                    let auth_path = inner.auth_path.clone();
                    // Only the connection the call went out on: a reconnect
                    // that landed meanwhile is a newer one, and a 401 from
                    // the old must not tear it down.
                    if let Some(server) = inner
                        .servers
                        .iter_mut()
                        .find(|s| s.entry.name == server_name && s.generation == generation)
                    {
                        if challenge.is_some() {
                            server.challenge = challenge;
                        }
                        server.drop_connection();
                        server.status = McpServerStatus::NeedsAuth;
                        // The refresh dance above may have cleared the grant.
                        server.has_tokens = server.url().is_some_and(|url| {
                            oauth::load_tokens(auth_path.as_deref(), url).is_some()
                        });
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

    /// A call found its server's connection dead — a stdio server that
    /// exited, a legacy stream that ended. "The client SHOULD restart it":
    /// the server is marked failed with the reason and reconnected at once,
    /// so it doesn't sit `✔ connected` over a dead pipe failing every call
    /// until the user notices. At most once per [`RESTART_COOLDOWN`] — a
    /// server that dies again that soon stays failed for a manual Reconnect,
    /// never a crash loop. The model is told to try again.
    fn restart_dead(&self, server_name: &str, generation: u64, detail: &str) -> ToolOutcome {
        let restart = {
            let mut inner = self.lock();
            let Some(server) = inner
                .servers
                .iter_mut()
                .find(|s| s.entry.name == server_name && s.generation == generation)
            else {
                return ToolOutcome::error(format!(
                    "MCP tool call failed ({server_name}): {detail}"
                ));
            };
            let now = std::time::Instant::now();
            let restart = server
                .last_restart
                .is_none_or(|at| now.duration_since(at) >= RESTART_COOLDOWN);
            server.drop_connection();
            server.status = McpServerStatus::Failed(detail.to_string());
            if restart {
                server.last_restart = Some(now);
            }
            restart
        };
        self.notify(McpEvent::Changed);
        if restart {
            self.spawn_connect(server_name);
            ToolOutcome::error(format!(
                "the {server_name} MCP server stopped ({detail}); it is being restarted — \
                 try the call again in a moment"
            ))
        } else {
            ToolOutcome::error(format!(
                "the {server_name} MCP server stopped again ({detail}) — \
                 ask the user to check it in /mcp"
            ))
        }
    }

    /// Tear every connection down (session shutdown). Every stdio server is
    /// stopped the spec's way — its stdin closed so it can exit on its own
    /// (a server saving state on EOF gets to), then `SIGTERM`, then
    /// `SIGKILL`, its whole process group with it — all of them at once and
    /// inside one bound (`QUIT_GRACE` per step), so a quit waits for a well-behaved
    /// server and never for a stuck one.
    pub fn shutdown(&self) {
        let transports: Vec<Arc<Mutex<Transport>>> = {
            let mut inner = self.lock();
            if let Some(flow) = inner.auth_flow.take() {
                flow.cancel.cancel();
            }
            inner
                .servers
                .iter_mut()
                .filter_map(|server| {
                    server.generation += 1;
                    server.transport.take()
                })
                .collect()
        };
        let (done, finished) = mpsc::channel();
        let count = transports.len();
        for transport in transports {
            let done = done.clone();
            std::thread::spawn(move || {
                if let Ok(mut transport) = transport.lock() {
                    transport.shutdown(QUIT_GRACE);
                }
                let _ = done.send(());
            });
        }
        let until = std::time::Instant::now() + QUIT_GRACE * 2 + Duration::from_millis(300);
        for _ in 0..count {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if finished.recv_timeout(left).is_err() {
                break;
            }
        }
    }
}

/// Record one server's on/off for `project` in the user file — outside the
/// manager's lock, over the file as it is now.
fn persist_disabled(
    path: &std::path::Path,
    project: &str,
    name: &str,
    disabled: bool,
) -> Result<(), String> {
    let existing = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("couldn't read {}: {e}", path.display())),
    };
    let updated =
        crate::mcp::record_disabled(&existing, project, name, disabled).map_err(|e| match e {
            crate::mcp::McpWriteError::InvalidJson(reason) => {
                format!("{} ({reason}) was left as it is", path.display())
            }
            other => format!("{}: {other:?}", path.display()),
        })?;
    super::files::write_config_file(path, &updated)
        .map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

/// Open the system browser on the authorize URL — best-effort (`open` on
/// macOS, `xdg-open` elsewhere; never `open` on Linux, where it is
/// `openvt`); the URL is on screen either way. The launcher is waited for on
/// a thread of its own, so it never lingers as a zombie for the session.
fn open_browser(url: &str) {
    let launcher = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let spawned = std::process::Command::new(launcher)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut child) = spawned {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
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
        let names: Vec<String> = manager
            .fingerprint()
            .iter()
            .filter_map(|entry| entry.split(' ').next().map(str::to_string))
            .collect();
        assert_eq!(names, ["mcp__fix__echo"]);
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

    /// Every offered function name is unique and inside the provider limit —
    /// a duplicate or an over-long one fails the **whole** request — and the
    /// specs, the fingerprint and the description lookup all agree on them.
    /// A stdio server answering by method, whose tool list grows from
    /// `one` to `one`, `two` once it announces the change. `modern` makes it
    /// a 2026-07-28 server, which announces only on the `subscriptions/listen`
    /// it was sent; a legacy one announces unasked.
    fn changing_stdio_entry(name: &str, modern: bool) -> McpServerEntry {
        let discover = if modern {
            r#"{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"resultType\":\"complete\",\"supportedVersions\":[\"2026-07-28\"],\"capabilities\":{\"tools\":{\"listChanged\":true}},\"_meta\":{\"io.modelcontextprotocol/serverInfo\":{\"name\":\"dyn\",\"version\":\"0\"}}}}"#
        } else {
            r#"{\"jsonrpc\":\"2.0\",\"id\":$id,\"error\":{\"code\":-32601,\"message\":\"nope\"}}"#
        };
        let announce = r#"( sleep 0.2; printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/tools/list_changed","params":{}}' ) &"#;
        let (on_listen, on_first_list) = if modern {
            (announce, "")
        } else {
            ("", announce)
        };
        let script = format!(
            r#"n=0
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *notifications/*) ;;
    *'"server/discover"'*) printf '%s\n' "{discover}" ;;
    *'"initialize"'*) printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{{\"tools\":{{\"listChanged\":true}}}},\"serverInfo\":{{\"name\":\"dyn\",\"version\":\"0\"}}}}}}" ;;
    *'"subscriptions/listen"'*) {on_listen} ;;
    *'"tools/list"'*)
      n=$((n+1))
      if [ $n -eq 1 ]; then
        printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"tools\":[{{\"name\":\"one\",\"inputSchema\":{{\"type\":\"object\"}}}}]}}}}"
        {on_first_list}
      else
        printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"tools\":[{{\"name\":\"one\",\"inputSchema\":{{\"type\":\"object\"}}}},{{\"name\":\"two\",\"inputSchema\":{{\"type\":\"object\"}}}}]}}}}"
      fi ;;
  esac
done"#
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

    fn wait_for_tools(manager: &McpManager, name: &str, count: usize) {
        for _ in 0..200 {
            let snapshot = manager.snapshot();
            if snapshot
                .iter()
                .find(|s| s.name == name)
                .is_some_and(|s| s.tools.len() == count)
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "{name} never offered {count} tools (has {:?})",
            manager.snapshot().iter().find(|s| s.name == name).map(|s| s
                .tools
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>())
        );
    }

    /// A server that says its tools changed is listed again, and the model
    /// is offered what it lists now — unasked from a legacy server, on the
    /// `subscriptions/listen` it was sent by a modern one (`docs/tools/mcp.md`).
    #[test]
    fn a_server_announcing_new_tools_is_listed_again() {
        for modern in [false, true] {
            let (manager, _rx) = manager_with(vec![changing_stdio_entry("dyn", modern)]);
            manager.start_connections();
            wait_connected(&manager, "dyn");
            wait_for_tools(&manager, "dyn", 2);
            let names: Vec<String> = manager
                .tool_specs()
                .iter()
                .map(|spec| spec["function"]["name"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(names, ["mcp__dyn__one", "mcp__dyn__two"], "modern {modern}");
            manager.shutdown();
        }
    }

    /// A modern streamable-HTTP server whose tools grow from `one` to `one`,
    /// `two`, announced on the `subscriptions/listen` stream it is sent —
    /// each connection served on its own thread, since that stream stays
    /// open while the listing goes out beside it. Returns its MCP URL.
    fn spawn_changing_http() -> String {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!(
            "http://127.0.0.1:{}/mcp",
            listener.local_addr().unwrap().port()
        );
        let lists = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let lists = Arc::clone(&lists);
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let mut length = 0usize;
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                            break;
                        }
                        if let Some(rest) =
                            header.to_ascii_lowercase().strip_prefix("content-length:")
                        {
                            length = rest.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut body = vec![0u8; length];
                    let _ = reader.read_exact(&mut body);
                    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    let id = request.get("id").cloned().unwrap_or(Value::Null);
                    let method = request["method"].as_str().unwrap_or_default().to_string();
                    let tools = |names: &[&str]| -> Value {
                        names
                            .iter()
                            .map(|name| serde_json::json!({"name": name, "inputSchema": {"type": "object"}}))
                            .collect()
                    };
                    let result = match method.as_str() {
                        "server/discover" => serde_json::json!({
                            "resultType": "complete",
                            "supportedVersions": ["2026-07-28"],
                            "capabilities": {"tools": {"listChanged": true}},
                            "_meta": {"io.modelcontextprotocol/serverInfo": {"name": "dyn", "version": "0"}}
                        }),
                        "tools/list" => {
                            let n = lists.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            let names: &[&str] = if n == 0 { &["one"] } else { &["one", "two"] };
                            serde_json::json!({"resultType": "complete", "tools": tools(names)})
                        }
                        "subscriptions/listen" => {
                            let _ = stream.write_all(
                                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n",
                            );
                            let ack = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/subscriptions/acknowledged", "params": {"_meta": {"io.modelcontextprotocol/subscriptionId": id}, "notifications": {"toolsListChanged": true}}});
                            let _ = stream.write_all(format!("data: {ack}\n\n").as_bytes());
                            std::thread::sleep(Duration::from_millis(300));
                            let changed = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed", "params": {"_meta": {"io.modelcontextprotocol/subscriptionId": id}}});
                            let _ = stream.write_all(format!("data: {changed}\n\n").as_bytes());
                            std::thread::sleep(Duration::from_secs(10));
                            return;
                        }
                        _ => {
                            let _ = stream.write_all(
                                b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            );
                            return;
                        }
                    };
                    let body = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
                        .to_string();
                    let _ = stream.write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    );
                });
            }
        });
        url
    }

    /// Over streamable HTTP the change arrives on the server's own stream:
    /// a server that promised changes is sent `subscriptions/listen`, and
    /// what it announces there lists its tools again.
    #[test]
    fn a_remote_server_announcing_new_tools_is_listed_again() {
        let url = spawn_changing_http();
        let (manager, _rx) = manager_with(vec![McpServerEntry {
            name: "remote".to_string(),
            config: McpServerConfig::Http {
                url,
                headers: Default::default(),
                sse_fallback: false,
                oauth: None,
            },
            scope: McpScope::User,
            config_path: "~/.alter-zero/mcp.json".to_string(),
        }]);
        manager.start_connections();
        wait_connected(&manager, "remote");
        wait_for_tools(&manager, "remote", 2);
        manager.shutdown();
    }

    fn http_entry(
        name: &str,
        url: &str,
        oauth: Option<crate::mcp::OAuthClientConfig>,
    ) -> McpServerEntry {
        McpServerEntry {
            name: name.to_string(),
            config: McpServerConfig::Http {
                url: url.to_string(),
                headers: Default::default(),
                sse_fallback: false,
                oauth,
            },
            scope: McpScope::User,
            config_path: "~/.alter-zero/mcp.json".to_string(),
        }
    }

    /// The tool names the model is offered, in order.
    fn offered_names(manager: &McpManager) -> Vec<String> {
        manager
            .tool_specs()
            .iter()
            .map(|spec| spec["function"]["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// A streamable-HTTP server in the shape of Google's MCP servers: it
    /// answers the connect and lists its tool to anyone, and publishes
    /// protected-resource metadata naming an authorization server on its own
    /// origin — one offering dynamic registration only when `registers`
    /// (Hugging Face's, Context7's and Exa's do; Google's doesn't). Returns
    /// its MCP URL.
    fn spawn_oauth_resource_http(registers: bool) -> String {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let url = format!("{origin}/mcp");
        let resource =
            serde_json::json!({"resource": url, "authorization_servers": [origin]}).to_string();
        let mut authorization = serde_json::json!({
            "issuer": origin,
            "authorization_endpoint": format!("{origin}/authorize"),
            "token_endpoint": format!("{origin}/token"),
            "code_challenge_methods_supported": ["S256"],
        });
        if registers {
            authorization["registration_endpoint"] = Value::String(format!("{origin}/register"));
        }
        let authorization = authorization.to_string();
        std::thread::spawn(move || {
            let reply = |status: &str, body: &str| {
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut length = 0usize;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    if let Some(rest) = header.to_ascii_lowercase().strip_prefix("content-length:")
                    {
                        length = rest.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let path = line.split_whitespace().nth(1).unwrap_or_default();
                let response = if line.starts_with("GET ") {
                    match path {
                        "/.well-known/oauth-protected-resource/mcp" => reply("200 OK", &resource),
                        "/.well-known/oauth-authorization-server" => {
                            reply("200 OK", &authorization)
                        }
                        _ => reply("404 Not Found", "{}"),
                    }
                } else {
                    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    let id = request.get("id").cloned().unwrap_or(Value::Null);
                    let result = match request["method"].as_str().unwrap_or_default() {
                        "server/discover" => serde_json::json!({
                            "resultType": "complete",
                            "supportedVersions": ["2026-07-28"],
                            "capabilities": {"tools": {}},
                            "_meta": {"io.modelcontextprotocol/serverInfo": {"name": "drive", "version": "0"}}
                        }),
                        "tools/list" => serde_json::json!({
                            "resultType": "complete",
                            "tools": [{"name": "one", "inputSchema": {"type": "object"}}]
                        }),
                        _ => Value::Null,
                    };
                    if result.is_null() {
                        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    } else {
                        reply(
                            "200 OK",
                            &serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
                                .to_string(),
                        )
                    }
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        url
    }

    /// Google's MCP servers list their tools to anyone and refuse every
    /// call. An anonymous connect to an OAuth resource whose authorization
    /// server can't register this app is held as needing a sign-in, its
    /// tools never offered to a model that could only watch them fail
    /// (`docs/tools/mcp.md`).
    #[test]
    fn an_oauth_resource_this_app_cant_register_with_waits_for_sign_in() {
        let url = spawn_oauth_resource_http(false);
        let (manager, _rx) = manager_with(vec![http_entry("drive", &url, None)]);
        manager.start_connections();
        wait_status(&manager, "drive", |status| {
            *status != McpServerStatus::Pending
        });
        let server = &manager.snapshot()[0];
        assert_eq!(server.status, McpServerStatus::NeedsAuth);
        assert_eq!(
            server.auth,
            Some(crate::mcp::McpAuthState::NotAuthenticated)
        );
        assert!(server.tools.is_empty(), "{:?}", server.tools);
        assert!(offered_names(&manager).is_empty());
        manager.shutdown();
    }

    /// Hugging Face, Context7 and Exa publish the same metadata yet answer
    /// calls without a token: an authorization server that can register
    /// this app leaves the anonymous connect usable, as it always was.
    #[test]
    fn an_oauth_resource_this_app_can_register_with_stays_connected() {
        let url = spawn_oauth_resource_http(true);
        let (manager, _rx) = manager_with(vec![http_entry("hub", &url, None)]);
        manager.start_connections();
        wait_connected(&manager, "hub");
        assert_eq!(offered_names(&manager), ["mcp__hub__one"]);
        manager.shutdown();
    }

    /// An entry naming its OAuth client asked for a login, so it waits for
    /// one even from a server that publishes no metadata and let an
    /// anonymous connect through — never "not authenticated" while its
    /// tools are offered anyway.
    #[test]
    fn a_server_naming_its_oauth_client_waits_for_sign_in() {
        let url = spawn_changing_http();
        let oauth = crate::mcp::OAuthClientConfig {
            client_id: Some("cid".to_string()),
            ..Default::default()
        };
        let (manager, _rx) = manager_with(vec![http_entry("calendar", &url, Some(oauth))]);
        manager.start_connections();
        wait_status(&manager, "calendar", |status| {
            *status != McpServerStatus::Pending
        });
        let server = &manager.snapshot()[0];
        assert_eq!(server.status, McpServerStatus::NeedsAuth);
        assert_eq!(
            server.auth,
            Some(crate::mcp::McpAuthState::NotAuthenticated)
        );
        assert!(offered_names(&manager).is_empty());
        manager.shutdown();
    }

    /// The instructions the reminder carries: only a connected server with
    /// a tool it offers, in name order, and — for a subagent — only a server
    /// whose tools the agent may call (`docs/tools/mcp.md`).
    #[test]
    fn instructions_come_from_servers_whose_tools_are_offered() {
        let (manager, _rx) = manager_with(vec![
            stdio_entry("zeta"),
            stdio_entry("alpha"),
            stdio_entry("idle"),
            stdio_entry("bare"),
        ]);
        {
            let mut inner = manager.lock();
            for server in &mut inner.servers {
                let name = server.entry.name.clone();
                server.identity = Some(ServerIdentity {
                    instructions: Some(format!("Use {name} wisely.")),
                    ..Default::default()
                });
                if name != "idle" {
                    server.status = McpServerStatus::Connected;
                }
                if name != "bare" {
                    server.tools = vec![McpToolInfo {
                        name: "go".into(),
                        description: "Go.".into(),
                        input_schema: serde_json::json!({"type": "object"}),
                    }];
                }
            }
        }
        let all = manager.instructions(&|_| true);
        assert_eq!(
            all,
            [
                ("alpha".to_string(), "Use alpha wisely.".to_string()),
                ("zeta".to_string(), "Use zeta wisely.".to_string()),
            ]
        );
        let only_alpha = manager.instructions(&|wire| wire.starts_with("mcp__alpha__"));
        assert_eq!(only_alpha.len(), 1);
        assert_eq!(only_alpha[0].0, "alpha");
    }

    #[test]
    fn offered_names_are_unique_short_and_agree_everywhere() {
        let (manager, _rx) = manager_with(vec![stdio_entry("atlassian-confluence-cloud")]);
        {
            let mut inner = manager.lock();
            let server = &mut inner.servers[0];
            server.status = McpServerStatus::Connected;
            server.tools = [
                "files.read",
                "files_read",
                "confluence_search_using_cql_query_with_pagination",
            ]
            .into_iter()
            .map(|name| McpToolInfo {
                name: name.into(),
                description: format!("{name} tool"),
                input_schema: serde_json::json!({"type": "object"}),
            })
            .collect();
        }
        let specs = manager.tool_specs();
        let names: Vec<String> = specs
            .iter()
            .map(|spec| spec["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names.len(), 3);
        let unique: std::collections::BTreeSet<&String> = names.iter().collect();
        assert_eq!(unique.len(), 3, "{names:?}");
        assert!(
            names
                .iter()
                .all(|n| n.len() <= crate::mcp::MAX_WIRE_NAME_LEN),
            "{names:?}"
        );
        let mut sorted = names.clone();
        sorted.sort();
        let fingerprinted: Vec<String> = manager
            .fingerprint()
            .iter()
            .filter_map(|entry| entry.split(' ').next().map(str::to_string))
            .collect();
        assert_eq!(fingerprinted, sorted);
        for spec in &specs {
            let wire = spec["function"]["name"].as_str().unwrap();
            assert_eq!(
                manager.tool_description(wire).as_deref(),
                spec["function"]["description"].as_str()
            );
        }
    }

    /// A stdio server scripted per method: `slow` answers after `delay`,
    /// anything else at once, and every call's tool name is appended to
    /// `calls` — so a test can see which calls the server ever received.
    fn recording_entry(name: &str, calls: &std::path::Path, on_call: &str) -> McpServerEntry {
        let script = format!(
            r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"server/discover"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{{"tools":{{}}}},"ttlMs":0,"cacheScope":"public"}}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"resultType":"complete","tools":[{{"name":"slow","inputSchema":{{"type":"object"}}}},{{"name":"fast","inputSchema":{{"type":"object"}}}}],"ttlMs":0,"cacheScope":"public"}}}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      tool=$(printf '%s' "$line" | sed -n 's/.*"name":"\([a-z]*\)".*/\1/p')
      echo "$tool" >> '{calls}'
      {on_call} ;;
  esac
done"#,
            calls = calls.display()
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

    fn call(name: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: "c".to_string(),
            name: name.to_string(),
            arguments: "{}".to_string(),
        }
    }

    /// Two calls to one server share its connection, so the second waits for
    /// the first — and that wait honours Esc. It didn't: the lock was taken
    /// blind, and once it was free the cancelled call still went out, a
    /// side-effecting tool running after the user had stopped it.
    #[test]
    fn a_call_cancelled_while_waiting_for_the_server_is_never_sent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let calls = dir.path().join("calls");
        let answer = r#"if [ "$tool" = slow ]; then sleep 1.5; fi; printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"ok"}]}}\n' "$id""#;
        let (manager, _rx) = manager_with(vec![recording_entry("rec", &calls, answer)]);
        manager.start_connections();
        wait_connected(&manager, "rec");
        let slow = {
            let manager = manager.clone();
            std::thread::spawn(move || {
                manager.call_tool(&call("mcp__rec__slow"), &CancelToken::new())
            })
        };
        std::thread::sleep(Duration::from_millis(300));
        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            canceller.cancel();
        });
        let started = std::time::Instant::now();
        let outcome = manager.call_tool(&call("mcp__rec__fast"), &cancel);
        assert!(!outcome.ok);
        assert!(outcome.output.contains("cancelled"), "{}", outcome.output);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert!(slow.join().unwrap().ok);
        std::thread::sleep(Duration::from_millis(200));
        let seen = std::fs::read_to_string(&calls).unwrap_or_default();
        assert_eq!(seen.lines().collect::<Vec<_>>(), ["slow"], "{seen}");
        manager.shutdown();
    }

    /// A stdio server that dies is restarted ("the client SHOULD restart
    /// it") instead of staying `✔ connected` over a dead pipe, every later
    /// call failing until the user reconnected by hand.
    #[test]
    fn a_server_that_dies_is_restarted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let calls = dir.path().join("calls");
        let (manager, mut rx) = manager_with(vec![recording_entry("crashy", &calls, "exit 3")]);
        manager.start_connections();
        wait_connected(&manager, "crashy");
        while rx.try_recv().is_ok() {}
        let outcome = manager.call_tool(&call("mcp__crashy__fast"), &CancelToken::new());
        assert!(!outcome.ok);
        assert!(outcome.output.contains("restart"), "{}", outcome.output);
        // The restart reconnects it.
        wait_connected(&manager, "crashy");
        manager.shutdown();
    }

    /// A 401 is about the connection the call went out on. A reconnect that
    /// landed while the call ran is a newer connection, and the old call's
    /// refusal used to tear it down to `needs authentication`.
    #[test]
    fn a_401_on_a_replaced_connection_leaves_the_new_one_alone() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let counted = Arc::clone(&calls);
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Read, Write};
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let counted = Arc::clone(&counted);
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    let _ = reader.read_line(&mut line);
                    let mut length = 0usize;
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                            break;
                        }
                        if let Some(rest) =
                            header.to_ascii_lowercase().strip_prefix("content-length:")
                        {
                            length = rest.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut body = vec![0u8; length];
                    let _ = reader.read_exact(&mut body);
                    let parsed: Value = serde_json::from_slice(&body).unwrap_or_default();
                    let id = parsed.get("id").cloned().unwrap_or(Value::Null);
                    let result = match parsed["method"].as_str().unwrap_or_default() {
                        "tools/call" => {
                            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(800));
                            let _ = stream.write_all(
                                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            );
                            return;
                        }
                        "server/discover" => serde_json::json!({"resultType": "complete",
                            "supportedVersions": ["2026-07-28"], "capabilities": {"tools": {}},
                            "ttlMs": 0, "cacheScope": "public"}),
                        _ => serde_json::json!({"resultType": "complete", "ttlMs": 0,
                            "cacheScope": "public",
                            "tools": [{"name": "t", "inputSchema": {"type": "object"}}]}),
                    };
                    let reply = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
                        .to_string();
                    let _ = stream.write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                            reply.len()
                        )
                        .as_bytes(),
                    );
                });
            }
        });
        let entry = McpServerEntry {
            name: "remote".to_string(),
            config: McpServerConfig::Http {
                url: format!("http://127.0.0.1:{port}/mcp"),
                headers: Default::default(),
                sse_fallback: false,
                oauth: None,
            },
            scope: McpScope::User,
            config_path: "~/.alter-zero/mcp.json".to_string(),
        };
        let (manager, _rx) = manager_with(vec![entry]);
        manager.start_connections();
        wait_connected(&manager, "remote");
        let in_flight = {
            let manager = manager.clone();
            std::thread::spawn(move || {
                manager.call_tool(&call("mcp__remote__t"), &CancelToken::new())
            })
        };
        while calls.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
        manager.reconnect("remote");
        wait_connected(&manager, "remote");
        assert!(!in_flight.join().unwrap().ok);
        assert_eq!(manager.snapshot()[0].status, McpServerStatus::Connected);
    }

    /// "Clear authentication" deletes the stored grant — and the live
    /// connection with it, which still sent the old bearer: the server kept
    /// working on a login the page said was gone.
    #[test]
    fn clearing_authentication_stops_sending_the_old_token() {
        let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let record = Arc::clone(&seen);
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Read, Write};
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                let (mut length, mut auth) = (0usize, None);
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    let lower = header.to_ascii_lowercase();
                    if let Some(rest) = lower.strip_prefix("content-length:") {
                        length = rest.trim().parse().unwrap_or(0);
                    }
                    if lower.starts_with("authorization:") {
                        auth = header.split_once(':').map(|(_, v)| v.trim().to_string());
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                record.lock().unwrap().push(auth);
                let parsed: Value = serde_json::from_slice(&body).unwrap_or_default();
                let id = parsed.get("id").cloned().unwrap_or(Value::Null);
                let result = match parsed["method"].as_str().unwrap_or_default() {
                    "server/discover" => serde_json::json!({"resultType": "complete",
                        "supportedVersions": ["2026-07-28"], "capabilities": {"tools": {}},
                        "ttlMs": 0, "cacheScope": "public"}),
                    _ => serde_json::json!({"resultType": "complete", "tools": [],
                        "ttlMs": 0, "cacheScope": "public"}),
                };
                let reply =
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    )
                    .as_bytes(),
                );
            }
        });
        let url = format!("http://127.0.0.1:{port}/mcp");
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("mcp-auth.json");
        oauth::save_tokens(
            Some(&store),
            &url,
            Some(&oauth::StoredTokens {
                access_token: "old-token".to_string(),
                ..Default::default()
            }),
        )
        .expect("the store write");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                entries: vec![McpServerEntry {
                    name: "remote".to_string(),
                    config: McpServerConfig::Http {
                        url: url.clone(),
                        headers: Default::default(),
                        sse_fallback: false,
                        oauth: None,
                    },
                    scope: McpScope::User,
                    config_path: "~/.alter-zero/mcp.json".to_string(),
                }],
                project: "/proj".to_string(),
                auth_path: Some(store),
                ..Default::default()
            },
        );
        manager.start_connections();
        wait_connected(&manager, "remote");
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .all(|a| a.as_deref() == Some("Bearer old-token"))
        );
        let before = seen.lock().unwrap().len();
        manager.clear_auth("remote").expect("the grant cleared");
        wait_connected(&manager, "remote");
        let after = seen.lock().unwrap();
        assert!(after.len() > before, "never reconnected");
        assert!(after[before..].iter().all(Option::is_none), "{after:?}");
    }

    /// The fingerprint is what decides whether the backend is rebuilt, so a
    /// reconnect that changed a tool's schema or description — the stdio
    /// server's own dev loop — must change it, not only a new name.
    #[test]
    fn the_fingerprint_moves_when_a_schema_changes() {
        let (manager, _rx) = manager_with(vec![stdio_entry("srv")]);
        let set_schema = |schema: Value| {
            let mut inner = manager.lock();
            let server = &mut inner.servers[0];
            server.status = McpServerStatus::Connected;
            server.tools = vec![McpToolInfo {
                name: "t".into(),
                description: "T.".into(),
                input_schema: schema,
            }];
        };
        set_schema(serde_json::json!({"type": "object"}));
        let before = manager.fingerprint();
        set_schema(serde_json::json!({"type": "object", "properties": {"x": {}}}));
        assert_ne!(manager.fingerprint(), before);
    }

    /// Quitting stops every stdio server the spec's way — stdin closed first,
    /// so a server that saves its state on EOF gets to — and has finished by
    /// the time `shutdown` returns, all of them at once inside one bound.
    #[test]
    fn shutdown_lets_a_server_exit_on_its_own_before_returning() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marks = dir.path().join("marks");
        let entry = |name: &str| {
            let script = format!(
                r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"server/discover"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{{"tools":{{}}}},"ttlMs":0,"cacheScope":"public"}}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"resultType":"complete","tools":[],"ttlMs":0,"cacheScope":"public"}}}}\n' "$id" ;;
  esac
done
sleep 0.1
echo {name} >> '{marks}'"#,
                marks = marks.display()
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
        };
        let (manager, _rx) = manager_with(vec![entry("one"), entry("two")]);
        manager.start_connections();
        wait_connected(&manager, "one");
        wait_connected(&manager, "two");
        let started = std::time::Instant::now();
        manager.shutdown();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        let marks = std::fs::read_to_string(&marks).unwrap_or_default();
        assert!(marks.contains("one") && marks.contains("two"), "{marks:?}");
    }

    /// A server that started requiring an `x-mcp-header` after we listed its
    /// tools answers the call `400 HeaderMismatch`. The client re-lists
    /// ("SHOULD call `tools/list` to check for changes"), records what it
    /// learned, and retries the call once with the header.
    #[test]
    fn a_header_mismatch_relists_the_tools_and_retries_once() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Read, Write};
            let mut lists = 0;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let (mut length, mut region) = (0usize, None::<String>);
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    let lower = header.to_ascii_lowercase();
                    if let Some(rest) = lower.strip_prefix("content-length:") {
                        length = rest.trim().parse().unwrap_or(0);
                    }
                    if lower.starts_with("mcp-param-region:") {
                        region = header.split_once(':').map(|(_, v)| v.trim().to_string());
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let parsed: Value = serde_json::from_slice(&body).unwrap_or_default();
                let id = parsed.get("id").cloned().unwrap_or(Value::Null);
                let (status, reply) = match parsed["method"].as_str().unwrap_or_default() {
                    "server/discover" => (
                        "200 OK",
                        serde_json::json!({"jsonrpc": "2.0", "id": id,
                        "result": {"resultType": "complete", "supportedVersions": ["2026-07-28"],
                        "capabilities": {"tools": {}}, "ttlMs": 0, "cacheScope": "public"}}),
                    ),
                    "tools/list" => {
                        lists += 1;
                        let region_schema = if lists == 1 {
                            serde_json::json!({"type": "string"})
                        } else {
                            serde_json::json!({"type": "string", "x-mcp-header": "Region"})
                        };
                        (
                            "200 OK",
                            serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {
                            "resultType": "complete", "ttlMs": 0, "cacheScope": "public",
                            "tools": [{"name": "query", "inputSchema": {"type": "object",
                                "properties": {"region": region_schema}}}]}}),
                        )
                    }
                    "tools/call"
                        if region.as_deref()
                            == parsed["params"]["arguments"]["region"].as_str() =>
                    {
                        (
                            "200 OK",
                            serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {
                            "resultType": "complete",
                            "content": [{"type": "text", "text": "routed"}]}}),
                        )
                    }
                    _ => (
                        "400 Bad Request",
                        serde_json::json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32020, "message": "Mcp-Param-Region missing"}}),
                    ),
                };
                let reply = reply.to_string();
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    )
                    .as_bytes(),
                );
            }
        });
        let entry = McpServerEntry {
            name: "gw".to_string(),
            config: McpServerConfig::Http {
                url: format!("http://127.0.0.1:{port}/mcp"),
                headers: Default::default(),
                sse_fallback: false,
                oauth: None,
            },
            scope: McpScope::User,
            config_path: "~/.alter-zero/mcp.json".to_string(),
        };
        let (manager, _rx) = manager_with(vec![entry]);
        manager.start_connections();
        wait_connected(&manager, "gw");
        let call = ToolCallRequest {
            id: "c1".to_string(),
            name: "mcp__gw__query".to_string(),
            arguments: r#"{"region":"eu-west1"}"#.to_string(),
        };
        let outcome = manager.call_tool(&call, &CancelToken::new());
        assert!(outcome.ok, "{}", outcome.output);
        assert_eq!(outcome.output, "routed");
        // The fresh listing replaced the stale one on the snapshot.
        let schema = &manager.snapshot()[0].tools[0].input_schema;
        assert_eq!(schema["properties"]["region"]["x-mcp-header"], "Region");
        manager.shutdown();
    }

    /// A modern server shedding load answers `input_required` with only a
    /// `requestState`: the same call again, the state echoed, is the whole
    /// answer — a client declaring no capabilities can give it.
    #[test]
    fn a_request_state_only_answer_is_retried_with_the_state_echoed() {
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"server/discover"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"public"}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"resultType":"complete","tools":[{"name":"echo","inputSchema":{"type":"object"}}],"ttlMs":0,"cacheScope":"public"}}\n' "$id" ;;
    *'"requestState":"busy-1"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"resultType":"complete","content":[{"type":"text","text":"done after retry"}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"resultType":"input_required","requestState":"busy-1"}}\n' "$id" ;;
  esac
done
"#;
        let entry = McpServerEntry {
            name: "mrtr".to_string(),
            config: McpServerConfig::Stdio {
                command: "sh".to_string(),
                args: vec!["-c".to_string(), script.to_string()],
                env: Default::default(),
            },
            scope: McpScope::User,
            config_path: "~/.alter-zero/mcp.json".to_string(),
        };
        let (manager, _rx) = manager_with(vec![entry]);
        manager.start_connections();
        wait_connected(&manager, "mrtr");
        let call = ToolCallRequest {
            id: "c1".to_string(),
            name: "mcp__mrtr__echo".to_string(),
            arguments: "{}".to_string(),
        };
        let outcome = manager.call_tool(&call, &CancelToken::new());
        assert!(outcome.ok, "{}", outcome.output);
        assert_eq!(outcome.output, "done after retry");
        manager.shutdown();
    }

    /// The spec the model is offered carries the repaired schema — the
    /// object root providers insist on — and a folded root combinator's note
    /// at the front of the description, while the server's own schema stays
    /// on the snapshot for the `/mcp` page.
    #[test]
    fn the_offered_spec_carries_the_repaired_schema() {
        let (manager, _rx) = manager_with(vec![stdio_entry("srv")]);
        {
            let mut inner = manager.lock();
            let server = &mut inner.servers[0];
            server.status = McpServerStatus::Connected;
            server.tools = vec![
                McpToolInfo {
                    name: "bare".into(),
                    description: "Bare.".into(),
                    input_schema: serde_json::json!({}),
                },
                McpToolInfo {
                    name: "either".into(),
                    description: "Either.".into(),
                    input_schema: serde_json::json!({
                        "anyOf": [{"properties": {"a": {}}}, {"properties": {"b": {}}}]
                    }),
                },
            ];
        }
        let specs = manager.tool_specs();
        let spec = |name: &str| {
            specs
                .iter()
                .find(|s| s["function"]["name"] == name)
                .cloned()
                .unwrap()
        };
        assert_eq!(
            spec("mcp__srv__bare")["function"]["parameters"],
            serde_json::json!({"type": "object", "properties": {}})
        );
        let either = spec("mcp__srv__either");
        assert!(either["function"]["parameters"].get("anyOf").is_none());
        let description = either["function"]["description"].as_str().unwrap();
        assert!(
            description.starts_with(crate::mcp::FOLDED_SCHEMA_NOTE),
            "{description}"
        );
        assert!(description.ends_with("Either."), "{description}");
        assert_eq!(
            manager.snapshot()[0].tools[0].input_schema,
            serde_json::json!({})
        );
    }

    /// Two servers whose names normalize alike would hand the model one set
    /// of names twice. The later one is refused, says why, and stays refused
    /// when the user asks it to reconnect.
    #[test]
    fn a_server_whose_name_collides_never_connects_and_says_why() {
        let (manager, _rx) = manager_with(vec![stdio_entry("my.server"), stdio_entry("my_server")]);
        manager.start_connections();
        wait_connected(&manager, "my.server");
        let snapshot = manager.snapshot();
        let McpServerStatus::Failed(reason) = &snapshot[1].status else {
            panic!("{:?}", snapshot[1].status);
        };
        assert!(reason.contains("my.server"), "{reason}");
        manager.reconnect("my_server");
        std::thread::sleep(Duration::from_millis(100));
        assert!(matches!(
            manager.snapshot()[1].status,
            McpServerStatus::Failed(_)
        ));
        manager.shutdown();
    }

    #[test]
    fn offered_tool_order_is_stable_when_servers_reorder_their_listing() {
        let (manager, _rx) = manager_with(vec![stdio_entry("zeta"), stdio_entry("alpha")]);
        {
            let mut inner = manager.lock();
            for server in &mut inner.servers {
                server.status = McpServerStatus::Connected;
                server.tools = ["second", "first"]
                    .into_iter()
                    .map(|name| McpToolInfo {
                        name: name.into(),
                        description: format!("{name} tool"),
                        input_schema: serde_json::json!({"type": "object"}),
                    })
                    .collect();
            }
        }
        let before = manager.tool_specs();
        {
            let mut inner = manager.lock();
            inner.servers.reverse();
            for server in &mut inner.servers {
                server.tools.reverse();
            }
        }
        assert_eq!(manager.tool_specs(), before);
        let names: Vec<_> = before
            .iter()
            .map(|spec| spec["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "mcp__alpha__first",
                "mcp__alpha__second",
                "mcp__zeta__first",
                "mcp__zeta__second"
            ]
        );
    }

    /// A dual-era HTTP fixture that can be **upgraded** mid-test: it starts
    /// legacy-only (`server/discover` answered with the classic `-32601`),
    /// and once `upgraded` flips it answers the probe with a modern
    /// (2026-07-28) discover result — while **still serving the legacy
    /// `initialize`**, exactly like a real server that added the modern
    /// revision without dropping the handshake its old clients need. That
    /// last part is the whole bug: it is why a stale `legacy` verdict kept
    /// succeeding instead of failing into a re-probe. `probes` counts the
    /// `server/discover` requests, so a test can assert the probe *ran*
    /// rather than inferring it from the revision.
    fn spawn_upgradable_http_fixture(
        upgraded: Arc<std::sync::atomic::AtomicBool>,
        probes: Arc<std::sync::atomic::AtomicUsize>,
    ) -> String {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut content_length = 0usize;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    if let Some(rest) = header.to_ascii_lowercase().strip_prefix("content-length:")
                    {
                        content_length = rest.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; content_length];
                let _ = reader.read_exact(&mut body);
                let parsed = serde_json::from_str::<Value>(&String::from_utf8_lossy(&body))
                    .unwrap_or_default();
                let id = parsed.get("id").and_then(Value::as_u64);
                let method = parsed
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let respond = |stream: &mut std::net::TcpStream, body: &str| {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                };
                match method.as_str() {
                    "server/discover" => {
                        probes.fetch_add(1, Ordering::SeqCst);
                        let body = if upgraded.load(Ordering::SeqCst) {
                            serde_json::json!({
                                "jsonrpc": "2.0", "id": id,
                                "result": {
                                    "resultType": "complete",
                                    "supportedVersions": ["2026-07-28", "2025-11-25"],
                                    "capabilities": {"tools": {}},
                                    "_meta": {"io.modelcontextprotocol/serverInfo":
                                        {"name": "upgradable", "version": "2"}}
                                }
                            })
                        } else {
                            serde_json::json!({
                                "jsonrpc": "2.0", "id": id,
                                "error": {"code": -32601, "message": "method not found"}
                            })
                        }
                        .to_string();
                        respond(&mut stream, &body);
                    }
                    "initialize" => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "protocolVersion": "2025-11-25",
                                "capabilities": {"tools": {}},
                                "serverInfo": {"name": "upgradable", "version": "1"}
                            }
                        })
                        .to_string();
                        respond(&mut stream, &body);
                    }
                    "notifications/initialized" => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                    _ => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"tools": [{"name": "ping",
                                "inputSchema": {"type": "object", "properties": {}}}]}
                        })
                        .to_string();
                        respond(&mut stream, &body);
                    }
                }
            }
        });
        format!("http://127.0.0.1:{port}/mcp")
    }

    /// The reported bug: a dual-era server reads `2025-11-25` forever. The
    /// era verdict must be re-detected at every connect — a server that
    /// speaks the modern revision today is reported at it today, whatever
    /// it answered the last time we asked. The probe counter is the half
    /// the revision alone doesn't prove: that the question was asked again.
    #[test]
    fn a_server_that_gained_the_modern_era_is_re_detected_at_the_next_connect() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let upgraded = Arc::new(AtomicBool::new(false));
        let probes = Arc::new(AtomicUsize::new(0));
        let url = spawn_upgradable_http_fixture(Arc::clone(&upgraded), Arc::clone(&probes));
        let entry = McpServerEntry {
            name: "up".to_string(),
            config: McpServerConfig::Http {
                url,
                headers: Default::default(),
                sse_fallback: false,
                oauth: None,
            },
            scope: McpScope::User,
            config_path: "~/.alter-zero/mcp.json".to_string(),
        };
        let (manager, _rx) = manager_with(vec![entry]);
        manager.start_connections();
        wait_connected(&manager, "up");
        let first = manager.snapshot()[0].identity.clone().unwrap();
        assert_eq!(
            first.protocol_version, "2025-11-25",
            "the launch it was legacy at"
        );
        assert_eq!(probes.load(Ordering::SeqCst), 1);
        // The server ships modern support (a redeploy between sessions).
        // Nothing about the previous connect may decide this one.
        // (`reconnect` flips the row to Pending under the lock before it
        // returns, so the wait below cannot read the connection it replaces.)
        upgraded.store(true, Ordering::SeqCst);
        manager.reconnect("up");
        wait_connected(&manager, "up");
        assert_eq!(
            probes.load(Ordering::SeqCst),
            2,
            "every connect must run era detection afresh"
        );
        let second = manager.snapshot()[0].identity.clone().unwrap();
        assert_eq!(
            second.protocol_version, "2026-07-28",
            "the Protocol row must show what the server returned this connect"
        );
        manager.shutdown();
    }

    /// A fixture that serves one good launch and then refuses to start —
    /// the reconnect that fails.
    fn one_launch_entry(name: &str, dir: &std::path::Path) -> McpServerEntry {
        let script = SERVER_THAT_DIES.replace("MARK", &dir.join("launched").display().to_string());
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

    const SERVER_THAT_DIES: &str = r#"
if [ -f 'MARK' ]; then echo 'the server is gone' >&2; exit 3; fi
: > 'MARK'
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"server/discover"'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"method not found"}}\n' "$id" ;;
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"once","version":"1"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id" ;;
  esac
done
"#;

    /// The facts a connection reported are that connection's — a reconnect
    /// that never initializes must not leave the previous one's protocol
    /// revision and tool list on the detail page under a red `✘ failed`.
    #[test]
    fn a_failed_reconnect_drops_the_previous_connections_facts() {
        let dir = tempfile::tempdir().unwrap();
        let (manager, _rx) = manager_with(vec![one_launch_entry("fix", dir.path())]);
        manager.start_connections();
        wait_connected(&manager, "fix");
        assert!(manager.snapshot()[0].identity.is_some());
        manager.reconnect("fix");
        wait_status(&manager, "fix", |status| {
            matches!(status, McpServerStatus::Failed(_))
        });
        let snapshot = manager.snapshot();
        assert_eq!(snapshot[0].identity, None);
        assert!(snapshot[0].tools.is_empty());
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
    fn a_server_awaiting_review_never_launches_until_enabled() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                entries: vec![stdio_entry("fix")],
                held: [("fix".to_string(), McpHold::Review)].into(),
                project: "/proj".to_string(),
                ..Default::default()
            },
        );
        manager.start_connections();
        // A beat for any wrongly-spawned connect to land before we look.
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            manager.snapshot()[0].status,
            McpServerStatus::AwaitingReview
        );
        assert_eq!(manager.awaiting_review(), vec!["fix".to_string()]);
        assert!(!manager.has_tools());
        // Enabled in the review, it connects…
        manager.set_approved("fix", true);
        wait_connected(&manager, "fix");
        assert!(manager.has_tools());
        assert!(manager.awaiting_review().is_empty());
        // …and disabled again, it is killed back to disabled, tools withdrawn.
        manager.set_approved("fix", false);
        assert_eq!(manager.snapshot()[0].status, McpServerStatus::Disabled);
        assert!(!manager.has_tools());
        manager.shutdown();
    }

    #[test]
    fn a_declined_server_is_disabled_and_enabling_it_never_writes_the_user_file() {
        let dir = tempfile::tempdir().unwrap();
        let user_file = dir.path().join("mcp.json");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                entries: vec![stdio_entry("fix")],
                held: [("fix".to_string(), McpHold::Declined)].into(),
                project: "/proj".to_string(),
                user_file: Some(user_file.clone()),
                ..Default::default()
            },
        );
        manager.start_connections();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(manager.snapshot()[0].status, McpServerStatus::Disabled);
        assert!(manager.is_held("fix"));
        assert!(!manager.has_tools());
        // The user file's toggle cannot run it while the review's decline
        // holds it…
        manager
            .set_disabled("fix", false)
            .expect("the toggle saved");
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(manager.snapshot()[0].status, McpServerStatus::Disabled);
        // …the approval does, and the decision lives in trust.json, never
        // in the user's own MCP file.
        manager.set_approved("fix", true);
        wait_connected(&manager, "fix");
        assert!(!manager.is_held("fix"));
        let written = std::fs::read_to_string(&user_file).unwrap_or_default();
        assert!(!written.contains("fix"), "{written}");
        manager.shutdown();
    }

    #[test]
    fn enabling_a_server_the_user_disabled_respects_the_disable() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                entries: vec![stdio_entry("fix")],
                held: [("fix".to_string(), McpHold::Review)].into(),
                disabled: ["fix".to_string()].into(),
                project: "/proj".to_string(),
                ..Default::default()
            },
        );
        manager.start_connections();
        // The review outranks the toggle while it is unanswered…
        assert_eq!(
            manager.snapshot()[0].status,
            McpServerStatus::AwaitingReview
        );
        assert!(manager.is_user_disabled("fix"));
        // …and enabling it there hands the server back to the user's
        // disable rather than launching over it.
        manager.set_approved("fix", true);
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
        manager.set_disabled("fix", true).expect("the toggle saved");
        let snapshot = manager.snapshot();
        assert_eq!(snapshot[0].status, McpServerStatus::Disabled);
        let written = std::fs::read_to_string(&user_file).unwrap();
        assert!(crate::mcp::parse_disabled(&written, "/proj").contains("fix"));
        // Enable clears the persisted set and reconnects.
        manager
            .set_disabled("fix", false)
            .expect("the toggle saved");
        let written = std::fs::read_to_string(&user_file).unwrap();
        assert!(crate::mcp::parse_disabled(&written, "/proj").is_empty());
        wait_connected(&manager, "fix");
        manager.shutdown();
    }

    /// A modern stdio server (discover = 1, tools/list = 2, with
    /// instructions) that touches `marker` once its stdin closes — the
    /// observable half of being stopped rather than merely hidden.
    fn closing_stdio_entry(name: &str, marker: &std::path::Path) -> McpServerEntry {
        let discover = r#"{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"_meta":{"io.modelcontextprotocol/serverInfo":{"name":"fixture","version":"1.0"}},"instructions":"Echo only."}}"#;
        let tools = r#"{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo","description":"Echo.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]}}"#;
        let script = format!(
            "printf '%s\\n' '{discover}'; printf '%s\\n' '{tools}'; while IFS= read -r line; do :; done; : > '{}'",
            marker.display()
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

    fn status_of(manager: &McpManager, name: &str) -> McpServerStatus {
        manager
            .snapshot()
            .into_iter()
            .find(|s| s.name == name)
            .expect("declared")
            .status
    }

    /// Turning MCP off for the session (`/settings` → **MCP**,
    /// `docs/tools/mcp.md`) stops every server — its stdin closed, not merely
    /// hidden — and offers the model nothing; turning it back on connects
    /// it again.
    #[test]
    fn turning_mcp_off_stops_every_server_and_on_connects_them_again() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("stopped");
        let (manager, _rx) = manager_with(vec![closing_stdio_entry("fix", &marker)]);
        assert!(manager.is_active(), "on by default");
        manager.start_connections();
        wait_connected(&manager, "fix");
        assert!(manager.has_tools());
        assert!(!manager.instructions(&|_| true).is_empty());

        manager.set_active(false);
        assert!(!manager.is_active());
        let snapshot = manager.snapshot();
        assert_eq!(snapshot[0].status, McpServerStatus::Off);
        assert!(snapshot[0].identity.is_none(), "the live facts went too");
        assert!(snapshot[0].tools.is_empty());
        assert!(manager.tool_specs().is_empty());
        assert!(manager.fingerprint().is_empty());
        assert!(manager.instructions(&|_| true).is_empty());
        assert!(!manager.has_tools());
        // Stopped, not hidden: the server saw its stdin close.
        for _ in 0..200 {
            if marker.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(marker.exists(), "the server was never stopped");

        manager.set_active(true);
        assert!(manager.is_active());
        wait_connected(&manager, "fix");
        assert!(manager.has_tools());
        manager.shutdown();
    }

    /// While MCP is off nothing connects, whoever asks — the startup kick, a
    /// Reconnect, an Enable — while each server's own switch still moves: an
    /// enabled server waits, `◯ off`, for MCP to come back on.
    #[test]
    fn while_mcp_is_off_nothing_connects_and_each_switch_still_moves() {
        let (manager, _rx) = manager_with(vec![stdio_entry("fix")]);
        manager.set_active(false);
        manager.start_connections();
        manager.reconnect("fix");
        // A beat for any wrongly-spawned connect to land before we look.
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(status_of(&manager, "fix"), McpServerStatus::Off);
        assert!(!manager.has_tools());
        manager.set_disabled("fix", true).expect("no file to save");
        assert_eq!(status_of(&manager, "fix"), McpServerStatus::Disabled);
        manager.set_disabled("fix", false).expect("no file to save");
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(status_of(&manager, "fix"), McpServerStatus::Off);
        manager.set_active(true);
        wait_connected(&manager, "fix");
        manager.shutdown();
    }

    /// The session switch never rewrites a server's own: one disabled in
    /// `/mcp` stays disabled through MCP going off and on, one waiting for
    /// the startup review keeps waiting, and only the enabled one returns.
    #[test]
    fn each_server_keeps_its_own_switch_through_mcp_off_and_on() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                entries: vec![stdio_entry("on"), stdio_entry("off"), stdio_entry("asked")],
                disabled: ["off".to_string()].into(),
                held: [("asked".to_string(), McpHold::Review)].into(),
                project: "/proj".to_string(),
                ..Default::default()
            },
        );
        manager.start_connections();
        wait_connected(&manager, "on");
        manager.set_active(false);
        assert_eq!(status_of(&manager, "on"), McpServerStatus::Off);
        assert_eq!(status_of(&manager, "off"), McpServerStatus::Disabled);
        assert_eq!(
            status_of(&manager, "asked"),
            McpServerStatus::AwaitingReview
        );
        assert!(
            manager.is_user_disabled("off"),
            "its own switch is untouched"
        );
        manager.set_active(true);
        wait_connected(&manager, "on");
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(status_of(&manager, "off"), McpServerStatus::Disabled);
        assert_eq!(
            status_of(&manager, "asked"),
            McpServerStatus::AwaitingReview
        );
        manager.shutdown();
    }

    /// A project server enabled in the review while MCP is off is released
    /// from the review, and waits for MCP rather than launching.
    #[test]
    fn a_server_approved_while_mcp_is_off_connects_once_it_is_on() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                entries: vec![stdio_entry("fix")],
                held: [("fix".to_string(), McpHold::Review)].into(),
                project: "/proj".to_string(),
                ..Default::default()
            },
        );
        manager.set_active(false);
        assert_eq!(status_of(&manager, "fix"), McpServerStatus::AwaitingReview);
        manager.set_approved("fix", true);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(status_of(&manager, "fix"), McpServerStatus::Off);
        assert!(manager.awaiting_review().is_empty());
        manager.set_active(true);
        wait_connected(&manager, "fix");
        manager.shutdown();
    }

    /// A server that can never start (here, a `${VAR}` nobody set) has
    /// nothing to report while MCP is off, and its reason is back once MCP
    /// is on again.
    #[test]
    fn a_server_that_cannot_start_reads_off_until_mcp_is_on_again() {
        let mut entry = stdio_entry("fix");
        entry.config = McpServerConfig::Stdio {
            command: "${ALTER_ZERO_TEST_MCP_OFF_NEVER_SET}".to_string(),
            args: Vec::new(),
            env: Default::default(),
        };
        let (manager, _rx) = manager_with(vec![entry]);
        assert!(matches!(
            status_of(&manager, "fix"),
            McpServerStatus::Failed(_)
        ));
        manager.set_active(false);
        assert_eq!(status_of(&manager, "fix"), McpServerStatus::Off);
        manager.set_active(true);
        match status_of(&manager, "fix") {
            McpServerStatus::Failed(reason) => {
                assert!(
                    reason.contains("ALTER_ZERO_TEST_MCP_OFF_NEVER_SET"),
                    "{reason}"
                );
            }
            other => panic!("expected the reason back, got {other:?}"),
        }
    }

    /// A call that reaches the manager while MCP is off — a turn that began
    /// with the tools offered — says where MCP is turned back on, rather
    /// than the generic unknown-tool answer.
    #[test]
    fn a_call_while_mcp_is_off_says_where_to_turn_it_on() {
        let (manager, _rx) = manager_with(vec![stdio_entry("fix")]);
        manager.start_connections();
        wait_connected(&manager, "fix");
        manager.set_active(false);
        let call = ToolCallRequest {
            id: "c1".to_string(),
            name: "mcp__fix__echo".to_string(),
            arguments: r#"{"text":"hi"}"#.to_string(),
        };
        let outcome = manager.call_tool(&call, &CancelToken::new());
        assert!(!outcome.ok);
        assert!(outcome.output.contains("MCP is off"), "{}", outcome.output);
        assert!(outcome.output.contains("/settings"), "{}", outcome.output);
        manager.shutdown();
    }

    /// No sign-in starts while MCP is off: the answer comes back at once,
    /// naming where MCP is turned on, and no browser is opened.
    #[test]
    fn signing_in_while_mcp_is_off_is_refused_with_the_reason() {
        let (manager, mut rx) =
            manager_with(vec![http_entry("remote", "http://127.0.0.1:9/mcp", None)]);
        manager.set_active(false);
        while rx.try_recv().is_ok() {}
        manager.authenticate("remote");
        match rx.try_recv() {
            Ok(McpEvent::AuthDone { server, ok, detail }) => {
                assert_eq!(server, "remote");
                assert!(!ok);
                assert!(detail.contains("/settings"), "{detail}");
            }
            other => panic!("expected the refusal, got {other:?}"),
        }
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
                resource: None,
                ..Default::default()
            }),
        )
        .expect("the store write");
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
                        oauth: None,
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
        // The transport contract (`docs/tools/mcp.md`): every blocking wait polls
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

    /// `${VAR}` references are expanded before a launch — here a default
    /// standing in for an unset variable is the command that runs — while a
    /// server naming a variable that is neither set nor defaulted doesn't
    /// start, the variable named, and `/mcp` keeps showing the config as
    /// written.
    #[test]
    fn variables_expand_before_a_launch() {
        let mut entry = stdio_entry("fix");
        if let McpServerConfig::Stdio { command, .. } = &mut entry.config {
            *command = "${ALTER_ZERO_TEST_NEVER_SET_SHELL:-sh}".to_string();
        }
        let unset = McpServerEntry {
            name: "remote".to_string(),
            config: McpServerConfig::Http {
                url: "https://${ALTER_ZERO_TEST_NEVER_SET_TENANT}.test/mcp".to_string(),
                headers: Default::default(),
                sse_fallback: false,
                oauth: None,
            },
            scope: McpScope::User,
            config_path: "~/.alter-zero/mcp.json".to_string(),
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                entries: vec![entry, unset],
                project: "/proj".to_string(),
                ..Default::default()
            },
        );
        manager.start_connections();
        wait_connected(&manager, "fix");
        let snapshot = manager.snapshot();
        let remote = snapshot.iter().find(|s| s.name == "remote").unwrap();
        assert!(
            matches!(&remote.status, McpServerStatus::Failed(why) if why.contains("ALTER_ZERO_TEST_NEVER_SET_TENANT")),
            "{:?}",
            remote.status
        );
        assert_eq!(
            remote.config.url(),
            Some("https://${ALTER_ZERO_TEST_NEVER_SET_TENANT}.test/mcp")
        );
        manager.shutdown();
    }

    /// A toggle over a user file that doesn't parse turns the server off for
    /// the session and says the file was left alone — it used to replace
    /// the file, and every server in it, with `{}`.
    #[test]
    fn a_toggle_never_rewrites_a_broken_user_file() {
        let dir = tempfile::tempdir().unwrap();
        let user_file = dir.path().join("mcp.json");
        let broken = r#"{"mcpServers": {"fix": {"command": "sh"},}}"#;
        std::fs::write(&user_file, broken).unwrap();
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
        let error = manager
            .set_disabled("fix", true)
            .expect_err("the file doesn't parse");
        assert!(error.contains("left as it is"), "{error}");
        assert_eq!(std::fs::read_to_string(&user_file).unwrap(), broken);
        assert_eq!(manager.snapshot()[0].status, McpServerStatus::Disabled);
        manager.shutdown();
    }

    /// A fake authorization server for the manager's sign-in: resource and
    /// AS metadata for `{base}/mcp`, registration, and a token endpoint.
    /// Everything else 404s — the reconnect after a sign-in fails, which no
    /// test here reads.
    fn spawn_signin_server() -> String {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let served = base.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut length = 0usize;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    if let Some(rest) = header.to_ascii_lowercase().strip_prefix("content-length:")
                    {
                        length = rest.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let path = line.split_whitespace().nth(1).unwrap_or_default();
                let reply = match path {
                    "/.well-known/oauth-protected-resource/mcp" => format!(
                        r#"{{"resource":"{served}/mcp","authorization_servers":["{served}"]}}"#
                    ),
                    "/.well-known/oauth-authorization-server" => format!(
                        r#"{{"issuer":"{served}","authorization_endpoint":"{served}/authorize","token_endpoint":"{served}/token","registration_endpoint":"{served}/register","code_challenge_methods_supported":["S256"]}}"#
                    ),
                    "/register" => r#"{"client_id":"cid"}"#.to_string(),
                    "/token" => r#"{"access_token":"tok","refresh_token":"rt","expires_in":3600}"#
                        .to_string(),
                    _ => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                        continue;
                    }
                };
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    )
                    .as_bytes(),
                );
            }
        });
        base
    }

    fn signin_manager(
        base: &str,
        auth_path: &std::path::Path,
    ) -> (McpManager, tokio::sync::mpsc::UnboundedReceiver<McpEvent>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = McpManager::new(
            tx,
            McpSources {
                entries: vec![McpServerEntry {
                    name: "srv".to_string(),
                    config: McpServerConfig::Http {
                        url: format!("{base}/mcp"),
                        headers: Default::default(),
                        sse_fallback: false,
                        oauth: None,
                    },
                    scope: McpScope::User,
                    config_path: "~/.alter-zero/mcp.json".to_string(),
                }],
                project: "/proj".to_string(),
                auth_path: Some(auth_path.to_path_buf()),
                // Never a real browser from a test.
                open_browser: Some(|_| {}),
                ..Default::default()
            },
        );
        (manager, rx)
    }

    /// The next event `pick` accepts within `within`, skipping the rest.
    fn next_event<T>(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<McpEvent>,
        within: Duration,
        mut pick: impl FnMut(&McpEvent) -> Option<T>,
    ) -> Option<T> {
        let until = std::time::Instant::now() + within;
        while std::time::Instant::now() < until {
            match rx.try_recv() {
                Ok(event) => {
                    if let Some(found) = pick(&event) {
                        return Some(found);
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        None
    }

    fn auth_url(event: &McpEvent) -> Option<String> {
        match event {
            McpEvent::AuthUrl { url, .. } => Some(url.clone()),
            _ => None,
        }
    }

    fn auth_done(event: &McpEvent) -> Option<(bool, String)> {
        match event {
            McpEvent::AuthDone { ok, detail, .. } => Some((*ok, detail.clone())),
            _ => None,
        }
    }

    /// A second Authenticate supersedes the first sign-in — and the first,
    /// ending, must neither take the new one's paste channel and cancel
    /// with it nor close the new one's page with a "cancelled" failure. A
    /// pasted URL from the old attempt is turned away with a reason; the new
    /// attempt's completes.
    #[test]
    fn a_superseded_sign_in_never_disturbs_the_new_one() {
        let base = spawn_signin_server();
        let dir = tempfile::tempdir().unwrap();
        let (manager, mut rx) = signin_manager(&base, &dir.path().join("mcp-auth.json"));
        manager.authenticate("srv");
        let first = next_event(&mut rx, Duration::from_secs(5), auth_url).expect("first URL");
        manager.authenticate("srv");
        let second = next_event(&mut rx, Duration::from_secs(5), auth_url).expect("second URL");
        // The first flow has had time to notice it was superseded and end.
        assert_eq!(
            next_event(&mut rx, Duration::from_millis(400), auth_done),
            None,
            "the superseded sign-in reported on the new one's page"
        );
        let state = |url: &str| crate::llm::mcp::redirect_param(url, "state").unwrap();
        let callback = |url: &str| {
            format!(
                "http://localhost:1/callback?code=c&state={}",
                crate::llm::mcp::url_encode(&state(url))
            )
        };
        manager.submit_auth_paste(&callback(&first));
        let reason = next_event(&mut rx, Duration::from_secs(3), |event| match event {
            McpEvent::AuthPasteRejected { server, reason } if server == "srv" => {
                Some(reason.clone())
            }
            _ => None,
        })
        .expect("the old attempt's URL is turned away with a reason");
        assert!(reason.contains("another sign-in"), "{reason}");
        manager.submit_auth_paste(&callback(&second));
        let (ok, detail) =
            next_event(&mut rx, Duration::from_secs(5), auth_done).expect("the new one finishes");
        assert!(ok, "{detail}");
        manager.shutdown();
    }

    /// Esc on the auth page is the user's own decision: the flow ends
    /// without a red "cancelled" toast.
    #[test]
    fn a_cancelled_sign_in_ends_quietly() {
        let base = spawn_signin_server();
        let dir = tempfile::tempdir().unwrap();
        let (manager, mut rx) = signin_manager(&base, &dir.path().join("mcp-auth.json"));
        manager.authenticate("srv");
        next_event(&mut rx, Duration::from_secs(5), auth_url).expect("the URL");
        manager.cancel_auth();
        assert_eq!(
            next_event(&mut rx, Duration::from_millis(500), auth_done),
            None
        );
        manager.shutdown();
    }

    /// A sign-in ends in a connect, and nothing connects while MCP is off:
    /// turning it off abandons a sign-in already running — quietly, the
    /// redirect it was waiting for landing nowhere.
    #[test]
    fn turning_mcp_off_abandons_a_sign_in_in_flight() {
        let base = spawn_signin_server();
        let dir = tempfile::tempdir().unwrap();
        let (manager, mut rx) = signin_manager(&base, &dir.path().join("mcp-auth.json"));
        manager.authenticate("srv");
        let url = next_event(&mut rx, Duration::from_secs(5), auth_url).expect("the URL");
        manager.set_active(false);
        let state = crate::llm::mcp::redirect_param(&url, "state").unwrap();
        manager.submit_auth_paste(&format!(
            "http://localhost:1/callback?code=c&state={}",
            crate::llm::mcp::url_encode(&state)
        ));
        assert_eq!(
            next_event(&mut rx, Duration::from_millis(500), auth_done),
            None,
            "the abandoned sign-in still finished"
        );
        assert_eq!(status_of(&manager, "srv"), McpServerStatus::Off);
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
