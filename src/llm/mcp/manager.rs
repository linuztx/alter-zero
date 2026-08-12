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
    McpAuthState, McpServerEntry, McpServerSnapshot, McpServerStatus, McpToolInfo, ServerIdentity,
    call_params, parse_call_result, tool_wire_name,
};
use crate::stream::CancelToken;

use super::client::{ConnectError, connect};
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
    /// The absolute cwd — the disabled sets' project key.
    pub project: String,
    /// The user config file — where a disable persists. `None` = no
    /// persistence (still toggles for the session).
    pub user_file: Option<PathBuf>,
    /// The OAuth token store (`{config_home}/mcp-auth.json`).
    pub auth_path: Option<PathBuf>,
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
    /// Bumped on disable/reconnect so a stale connect thread's result is
    /// dropped instead of resurrecting an old state.
    generation: u64,
}

struct Inner {
    servers: Vec<ServerState>,
    errors: Vec<String>,
    disabled: std::collections::BTreeSet<String>,
    project: String,
    user_file: Option<PathBuf>,
    auth_path: Option<PathBuf>,
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
                let has_tokens = entry
                    .config
                    .url()
                    .is_some_and(|url| oauth::load_tokens(auth_path.as_deref(), url).is_some());
                ServerState {
                    status: if disabled {
                        McpServerStatus::Disabled
                    } else {
                        McpServerStatus::Pending
                    },
                    identity: None,
                    tools: Vec::new(),
                    transport: None,
                    wire_map: BTreeMap::new(),
                    has_tokens,
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
                project: sources.project,
                user_file: sources.user_file,
                auth_path,
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
        let (config, bearer, cwd, timeout, generation) = {
            let mut inner = self.lock();
            let (auth_path, cwd, timeout) = (
                inner.auth_path.clone(),
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
            let bearer = server
                .entry
                .config
                .url()
                .and_then(|url| oauth::fresh_bearer(auth_path.as_deref(), url));
            server.has_tokens = bearer.is_some()
                || server
                    .entry
                    .config
                    .url()
                    .is_some_and(|url| oauth::load_tokens(auth_path.as_deref(), url).is_some());
            (
                server.entry.config.clone(),
                bearer,
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
            let result = connect(&config, bearer, cwd.as_deref(), timeout, &cancel);
            let mut inner = manager.lock();
            let Some(server) = inner.servers.iter_mut().find(|s| s.entry.name == name) else {
                return;
            };
            if server.generation != generation {
                return; // a reconnect/disable superseded this attempt
            }
            match result {
                Ok(connection) => {
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
                Err(ConnectError::NeedsAuth(_)) => {
                    server.status = McpServerStatus::NeedsAuth;
                }
                Err(ConnectError::Failed(detail)) => {
                    server.status = McpServerStatus::Failed(detail);
                }
            }
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
        if !disabled {
            self.spawn_connect(name);
        }
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
        let (url, auth_path) = {
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
            (url, inner.auth_path.clone())
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
                None,
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
                auth: server
                    .entry
                    .config
                    .is_remote()
                    .then_some(if server.has_tokens {
                        McpAuthState::Authenticated
                    } else {
                        McpAuthState::NotAuthenticated
                    }),
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
                        inner.tool_timeout,
                    ))
                })
        };
        let Some((server_name, raw_tool, transport, timeout)) = lookup else {
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
        let result = {
            let mut transport = transport
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            transport.request(
                "tools/call",
                call_params(&raw_tool, &arguments),
                timeout,
                cancel,
            )
        };
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
            Err(TransportError::NeedsAuth(_)) => {
                {
                    let mut inner = self.lock();
                    if let Some(server) = inner
                        .servers
                        .iter_mut()
                        .find(|s| s.entry.name == server_name)
                    {
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
        // The deterministic-ids `sh` fixture: initialize = 1, tools/list = 2,
        // then every tools/call (ids 3, 4, …) is answered from a canned list.
        let init = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"0"}}}"#;
        let tools = r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"Echo.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]}}"#;
        let call =
            r#"{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"echoed"}]}}"#;
        let script = format!(
            "cat > /dev/null & printf '%s\\n' '{init}'; printf '%s\\n' '{tools}'; printf '%s\\n' '{call}'; sleep 10"
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
