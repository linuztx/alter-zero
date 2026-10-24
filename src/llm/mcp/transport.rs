//! The three MCP transports (`docs/mcp.md`) behind one blocking
//! `request`/`notify` surface — boundary code, verified against in-process
//! fixtures (a scripted `sh` stdio server, `std::net::TcpListener` HTTP
//! servers) so the suite needs no network.
//!
//! Every wait polls the turn's [`CancelToken`] on the 20 ms cadence (the
//! hooks-runner contract — a hung server must not eat Esc) and enforces the
//! caller's deadline; the blocking HTTP work runs on detached threads that
//! report over channels (`openai::drain_stream`'s shape), so a cancelled
//! wait abandons the thread rather than joining it.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::mcp::{
    Incoming, LEGACY_PROTOCOL_VERSION, RpcError, SseParser, header_value, initialize_params_for,
    notification, parse_incoming, request, with_meta,
};
use crate::stream::CancelToken;

/// The cancel-poll cadence every blocking wait uses.
const POLL: Duration = Duration::from_millis(20);

/// How much of a stdio server's stderr is retained for a connect error.
const STDERR_TAIL_BYTES: usize = 4 * 1024;

/// The per-read stall deadline handed to the pooled HTTP client for POSTs.
const HTTP_OP_TIMEOUT: Duration = Duration::from_secs(60);

/// The legacy SSE stream's per-read deadline — long, because the stream idles
/// between replies (Claude Code deliberately un-timeouts it; we keep a stall
/// backstop so a dead peer is eventually noticed).
const SSE_STREAM_TIMEOUT: Duration = Duration::from_secs(600);

/// Why a transport operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The server answered 401 — OAuth is needed. Carries the
    /// `WWW-Authenticate` header when one was sent (the OAuth discovery
    /// seed).
    NeedsAuth(Option<String>),
    /// A streamable-HTTP `initialize` was refused in a way the spec says to
    /// retry as legacy SSE (4xx other than 401 on a `sse_fallback` config).
    HttpRefused(String),
    /// The JSON-RPC layer answered with an error object.
    Rpc(RpcError),
    /// The deadline passed with no answer.
    Timeout,
    /// The turn was cancelled (Esc) while waiting.
    Cancelled,
    /// Everything else — transport/protocol failures, with the story.
    Failed(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsAuth(_) => write!(f, "authentication required"),
            Self::HttpRefused(detail) => write!(f, "server refused the request: {detail}"),
            Self::Rpc(e) => write!(f, "server error: {e}"),
            Self::Timeout => write!(f, "timed out waiting for the server"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Failed(detail) => write!(f, "{detail}"),
        }
    }
}

/// One connected transport. Constructed by [`super::client::connect`];
/// requests are sequential (the callers hold it behind a `Mutex`).
pub enum Transport {
    Stdio(StdioTransport),
    Http(HttpTransport),
    Sse(SseTransport),
}

impl Transport {
    /// Send a request and wait for its response, honouring `deadline` and
    /// `cancel`.
    pub fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<Value, TransportError> {
        match self {
            Self::Stdio(t) => t.request(method, params, deadline, cancel),
            Self::Http(t) => t.request(method, params, deadline, cancel),
            Self::Sse(t) => t.request(method, params, deadline, cancel),
        }
    }

    /// Send a notification (no response expected).
    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), TransportError> {
        match self {
            Self::Stdio(t) => t.notify(method, params),
            Self::Http(t) => t.notify(method, params),
            Self::Sse(t) => t.notify(method, params),
        }
    }

    /// Re-arm the bearer a remote transport sends — the mid-session token
    /// refresh seam (`docs/mcp.md`): a rotated access token reaches the next
    /// request without a reconnect. A stdio transport has no auth story.
    pub fn set_bearer(&mut self, bearer: Option<String>) {
        match self {
            Self::Stdio(_) => {}
            Self::Http(t) => t.set_bearer(bearer),
            Self::Sse(t) => t.set_bearer(bearer),
        }
    }

    /// Enter the modern (2026-07-28) wire mode: every request carries the
    /// `_meta` version/capabilities block, and streamable HTTP adds the
    /// `Mcp-Method`/`Mcp-Name` request-metadata headers beside the
    /// `MCP-Protocol-Version` echo. The legacy SSE transport predates the
    /// modern era wholesale — a no-op there.
    pub fn set_modern(&mut self, version: &str) {
        match self {
            Self::Stdio(t) => t.modern = Some(version.to_string()),
            Self::Http(t) => t.set_modern(version),
            Self::Sse(_) => {}
        }
    }

    /// Back to the legacy handshake mode (the modern probe met a legacy
    /// server): no `_meta`, no request-metadata headers, and the
    /// streamable-HTTP version header returns only once `initialize`
    /// settles a revision.
    pub fn set_legacy(&mut self) {
        match self {
            Self::Stdio(t) => t.modern = None,
            Self::Http(t) => t.set_legacy(),
            Self::Sse(_) => {}
        }
    }
}

/// Wait on a receiver with the deadline and cancel polls every transport
/// wait shares.
fn wait_channel<T>(
    rx: &mpsc::Receiver<T>,
    started: Instant,
    deadline: Duration,
    cancel: &CancelToken,
) -> Result<T, TransportError> {
    loop {
        if cancel.is_cancelled() {
            return Err(TransportError::Cancelled);
        }
        if started.elapsed() > deadline {
            return Err(TransportError::Timeout);
        }
        match rx.recv_timeout(POLL) {
            Ok(value) => return Ok(value),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(TransportError::Failed(
                    "the server closed the connection".to_string(),
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// stdio

/// A local child process speaking newline-delimited JSON-RPC on its stdio.
pub struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    /// Responses that arrived while we were waiting for a different id
    /// (a server may answer out of order).
    parked: HashMap<u64, Result<Value, RpcError>>,
    /// The retained stderr tail — what a connect error shows.
    stderr: Arc<Mutex<String>>,
    /// The modern (2026-07-28) protocol version when the era detection
    /// settled modern — every request's params then carry the `_meta`
    /// block. Stdio has no header layer: the message body is everything.
    pub(super) modern: Option<String>,
    next_id: u64,
}

impl StdioTransport {
    /// Spawn the configured command with piped stdio. The child inherits the
    /// session environment with the config's `env` overlaid, runs in `cwd`,
    /// and is killed on drop.
    pub fn spawn(
        command: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: Option<&std::path::Path>,
    ) -> Result<Self, TransportError> {
        let mut child = stdio_command(command, args, env, cwd)?
            .spawn()
            .map_err(|e| TransportError::Failed(format!("could not start `{command}`: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| TransportError::Failed("no stdin pipe".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TransportError::Failed("no stdout pipe".to_string()))?;
        let stderr = child.stderr.take();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        if let Some(stderr) = stderr {
            let tail = Arc::clone(&stderr_tail);
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if let Ok(mut tail) = tail.lock() {
                        tail.push_str(&line);
                        tail.push('\n');
                        if tail.len() > STDERR_TAIL_BYTES {
                            let cut = tail.len() - STDERR_TAIL_BYTES;
                            // Trim from the front on a char boundary.
                            let cut = (cut..tail.len())
                                .find(|i| tail.is_char_boundary(*i))
                                .unwrap_or(0);
                            tail.drain(..cut);
                        }
                    }
                }
            });
        }
        Ok(Self {
            child,
            stdin,
            lines,
            parked: HashMap::new(),
            stderr: stderr_tail,
            modern: None,
            next_id: 1,
        })
    }

    /// The retained stderr tail — folded into a connect error so a crashing
    /// server explains itself.
    #[must_use]
    pub fn stderr_tail(&self) -> String {
        self.stderr.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn send(&mut self, message: &Value) -> Result<(), TransportError> {
        let mut line = message.to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|()| self.stdin.flush())
            .map_err(|e| TransportError::Failed(format!("could not write to the server: {e}")))
    }

    /// A dead child's story: wait briefly for it to be reaped (so the stderr
    /// reader has drained), then report the tail — a crashing `npx` explains
    /// itself instead of reading as a bare broken pipe.
    fn exit_error(&mut self) -> TransportError {
        let waited = Instant::now();
        while waited.elapsed() < Duration::from_millis(500) {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => std::thread::sleep(POLL),
                Err(_) => break,
            }
        }
        // One more beat for the stderr thread to flush its last read.
        std::thread::sleep(POLL);
        let tail = self.stderr_tail();
        let tail = tail.trim();
        if tail.is_empty() {
            TransportError::Failed("the server exited".to_string())
        } else {
            TransportError::Failed(format!("the server exited: {tail}"))
        }
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<Value, TransportError> {
        let id = self.next_id;
        self.next_id += 1;
        let params = match &self.modern {
            Some(version) => with_meta(params, version),
            None => params,
        };
        if self.send(&request(id, method, params)).is_err() {
            // A write to a closed pipe: the child died — explain with its
            // stderr rather than a bare broken-pipe message.
            return Err(self.exit_error());
        }
        if let Some(result) = self.parked.remove(&id) {
            return result.map_err(TransportError::Rpc);
        }
        let started = Instant::now();
        loop {
            let line = wait_channel(&self.lines, started, deadline, cancel).map_err(|e| {
                match e {
                    // A closed pipe usually means the child died — say so
                    // with its stderr.
                    TransportError::Failed(_) => self.exit_error(),
                    other => other,
                }
            })?;
            match parse_incoming(&line) {
                Some(Incoming::Response { id: got, result }) if got == id => {
                    return result.map_err(TransportError::Rpc);
                }
                Some(Incoming::Response { id: got, result }) => {
                    self.parked.insert(got, result);
                }
                Some(Incoming::Other) | None => {}
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), TransportError> {
        self.send(&notification(method, params))
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Direct MCP launches do not yet own a pre-spawn Windows Job Object.
        // taskkill is best-effort tree cleanup (not orphan ownership); always
        // retain the owned direct-child kill/reap fallback as well.
        #[cfg(windows)]
        let _ = crate::subprocess::kill_windows_tree(self.child.id());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// streamable HTTP

/// The headers a remote transport sends: the config's own, plus the OAuth
/// bearer when one is stored.
#[derive(Debug, Clone, Default)]
pub struct RemoteHeaders {
    pub headers: BTreeMap<String, String>,
    pub bearer: Option<String>,
}

impl RemoteHeaders {
    fn apply(
        &self,
        mut req: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        for (key, value) in &self.headers {
            req = req.header(key, value);
        }
        // A configured Authorization header wins over a stored token — the
        // user wrote it down on purpose.
        let has_auth = self
            .headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("authorization"));
        if !has_auth && let Some(bearer) = &self.bearer {
            req = req.header("Authorization", format!("Bearer {bearer}"));
        }
        req
    }
}

/// A streamable-HTTP server: POST per message, the response either JSON or
/// an SSE stream carrying it. Legacy mode captures the `Mcp-Session-Id` at
/// initialize and echoes it thereafter; modern (2026-07-28) mode is
/// stateless — `_meta` in every request, the `Mcp-Method`/`Mcp-Name`
/// request-metadata headers beside the version echo, and no sessions (a
/// modern server never mints one).
pub struct HttpTransport {
    url: String,
    remote: RemoteHeaders,
    session: Option<String>,
    /// The protocol revision every request's `MCP-Protocol-Version` header
    /// echoes: the modern version while probing/settled modern, else the
    /// revision `initialize` settled (absent until it does — the pre-header
    /// servers' contract).
    protocol_version: Option<String>,
    /// Set while the transport speaks the modern era: the version whose
    /// `_meta` rides every request.
    modern: Option<String>,
    next_id: u64,
}

/// The modern request-metadata headers one POST carries.
struct ModernHeaders {
    /// `Mcp-Method` — the JSON-RPC method, verbatim.
    method: String,
    /// `Mcp-Name` — `params.name`/`params.uri` for the three methods that
    /// have one (`tools/call`, `resources/read`, `prompts/get`).
    name: Option<String>,
}

/// What one POST resolved to, off the worker thread.
enum PostOutcome {
    /// The response for the request id (or an error object).
    Response(Result<Value, RpcError>),
    /// A notification-style acceptance (202/204/empty body).
    Accepted,
    /// The server answered 404 to a request carrying our session id: the
    /// session is gone (a restart, an idle timeout) and the spec's answer is
    /// a fresh `initialize`. Only reachable in legacy mode — the modern era
    /// has no sessions, so a 404 there is an unknown method.
    SessionExpired,
    /// The captured session id rode along (initialize).
    Session(Option<String>, Box<PostOutcome>),
    Err(TransportError),
}

impl HttpTransport {
    #[must_use]
    pub fn new(url: String, remote: RemoteHeaders) -> Self {
        Self {
            url,
            remote,
            session: None,
            protocol_version: None,
            modern: None,
            next_id: 1,
        }
    }

    /// Record the protocol version the server settled on (echoed on every
    /// later request).
    pub fn set_protocol_version(&mut self, version: &str) {
        if !version.trim().is_empty() {
            self.protocol_version = Some(version.trim().to_string());
        }
    }

    /// Enter the modern wire mode at `version` — the era probe's first act,
    /// and the settled state when the probe succeeds. The version header
    /// rides every request from here (it MUST match the `_meta`).
    pub fn set_modern(&mut self, version: &str) {
        self.modern = Some(version.to_string());
        self.protocol_version = Some(version.to_string());
    }

    /// Back to legacy: no `_meta`, no request-metadata headers, and no
    /// version header until `initialize` settles a revision.
    pub fn set_legacy(&mut self) {
        self.modern = None;
        self.protocol_version = None;
    }

    /// Refresh the bearer the transport sends (a token refresh mid-session).
    pub fn set_bearer(&mut self, bearer: Option<String>) {
        self.remote.bearer = bearer;
    }

    fn post(
        &self,
        body: Value,
        want_id: Option<u64>,
        modern: Option<ModernHeaders>,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<PostOutcome, TransportError> {
        let url = self.url.clone();
        let remote = self.remote.clone();
        let session = self.session.clone();
        let protocol = self.protocol_version.clone();
        let (tx, rx) = mpsc::channel();
        let started = Instant::now();
        std::thread::spawn(move || {
            let outcome = post_blocking(
                &url,
                &remote,
                session.as_deref(),
                protocol.as_deref(),
                modern.as_ref(),
                &body,
                want_id,
            );
            let _ = tx.send(outcome);
        });
        wait_channel(&rx, started, deadline, cancel)
    }

    /// The modern request-metadata headers for one request, when the
    /// transport is in modern mode.
    fn modern_headers(&self, method: &str, params: &Value) -> Option<ModernHeaders> {
        self.modern.as_ref()?;
        let name = matches!(method, "tools/call" | "resources/read" | "prompts/get")
            .then(|| {
                params
                    .get("name")
                    .or_else(|| params.get("uri"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .flatten();
        Some(ModernHeaders {
            method: method.to_string(),
            name,
        })
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<Value, TransportError> {
        let params = match &self.modern {
            Some(version) => with_meta(params, version),
            None => params,
        };
        let mut outcome = self.send_once(method, &params, deadline, cancel)?;
        if matches!(outcome, PostOutcome::SessionExpired) {
            // The session died under us. Start a new one and replay, so a
            // server restart or an idle timeout costs a round trip instead
            // of the whole connection.
            self.reinitialize(deadline, cancel)?;
            outcome = self.send_once(method, &params, deadline, cancel)?;
        }
        match outcome {
            PostOutcome::Response(result) => result.map_err(TransportError::Rpc),
            PostOutcome::Accepted => Err(TransportError::Failed(
                "the server accepted the request but sent no response".to_string(),
            )),
            PostOutcome::SessionExpired => Err(TransportError::Failed(
                "the server keeps expiring the session".to_string(),
            )),
            PostOutcome::Err(e) => Err(e),
            PostOutcome::Session(..) => unreachable!("unwrapped in send_once"),
        }
    }

    /// One POST of `method`, capturing any session id the server names.
    fn send_once(
        &mut self,
        method: &str,
        params: &Value,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<PostOutcome, TransportError> {
        let id = self.next_id;
        self.next_id += 1;
        let modern = self.modern_headers(method, params);
        let outcome = self.post(
            request(id, method, params.clone()),
            Some(id),
            modern,
            deadline,
            cancel,
        )?;
        Ok(match outcome {
            PostOutcome::Session(session, inner) => {
                // The session id is captured whenever the server names one
                // (initialize, per spec — but echoing whatever it says is
                // harmless and simpler).
                if let Some(session) = session {
                    self.session = Some(session);
                }
                *inner
            }
            other => other,
        })
    }

    /// Open a fresh session: drop the dead id, handshake again at the
    /// settled revision, and repeat `notifications/initialized` so the
    /// server considers us initialized. The re-handshake's own failure is
    /// returned intact — masking it would hide a 401 underneath, which is
    /// exactly the signal the token refresh acts on.
    fn reinitialize(
        &mut self,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<(), TransportError> {
        self.session = None;
        let version = self
            .protocol_version
            .clone()
            .unwrap_or_else(|| LEGACY_PROTOCOL_VERSION.to_string());
        match self.send_once(
            "initialize",
            &initialize_params_for(&version),
            deadline,
            cancel,
        )? {
            PostOutcome::Response(Ok(_)) | PostOutcome::Accepted => {}
            PostOutcome::Response(Err(e)) => return Err(TransportError::Rpc(e)),
            PostOutcome::Err(e) => return Err(e),
            PostOutcome::SessionExpired => {
                return Err(TransportError::Failed(
                    "the server expired the session it just opened".to_string(),
                ));
            }
            PostOutcome::Session(..) => unreachable!("unwrapped in send_once"),
        }
        self.notify("notifications/initialized", Value::Null)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), TransportError> {
        // Fire-and-forget on a worker thread with a generous deadline; an
        // acceptance failure is not worth failing the connect over, but a
        // transport error is surfaced. (Only the legacy handshake sends
        // notifications — the modern era has none of ours — but the `_meta`
        // and header rules are honoured all the same.)
        let params = match &self.modern {
            Some(version) => with_meta(params, version),
            None => params,
        };
        let modern = self.modern_headers(method, &params);
        let outcome = self.post(
            notification(method, params),
            None,
            modern,
            HTTP_OP_TIMEOUT,
            &CancelToken::new(),
        )?;
        match outcome {
            PostOutcome::Err(e) => Err(e),
            _ => Ok(()),
        }
    }
}

/// The blocking POST body — runs on a worker thread.
fn post_blocking(
    url: &str,
    remote: &RemoteHeaders,
    session: Option<&str>,
    protocol_version: Option<&str>,
    modern: Option<&ModernHeaders>,
    body: &Value,
    want_id: Option<u64>,
) -> PostOutcome {
    let client = match crate::llm::http_client(HTTP_OP_TIMEOUT) {
        Ok(client) => client,
        Err(e) => return PostOutcome::Err(TransportError::Failed(e.to_string())),
    };
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    req = remote.apply(req);
    if let Some(session) = session {
        req = req.header("Mcp-Session-Id", session);
    }
    if let Some(version) = protocol_version {
        req = req.header("MCP-Protocol-Version", version);
    }
    if let Some(headers) = modern {
        req = req.header("Mcp-Method", header_value(&headers.method));
        if let Some(name) = &headers.name {
            req = req.header("Mcp-Name", header_value(name));
        }
    }
    let response = match req.body(body.to_string()).send() {
        Ok(response) => response,
        Err(e) => return PostOutcome::Err(TransportError::Failed(format!("request failed: {e}"))),
    };
    let status = response.status();
    let new_session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let wrap = |outcome: PostOutcome| match &new_session {
        Some(_) => PostOutcome::Session(new_session.clone(), Box::new(outcome)),
        None => outcome,
    };
    if status.as_u16() == 401 {
        let challenge = response
            .headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        return PostOutcome::Err(TransportError::NeedsAuth(challenge));
    }
    // A 404 to a request that carried a session id means that session is
    // gone — the spec's cue to open a new one and replay. Gated on having
    // sent one, because a modern (sessionless) server answers an unknown
    // *method* with 404 and re-handshaking would be nonsense there.
    if status.as_u16() == 404 && session.is_some() {
        return PostOutcome::SessionExpired;
    }
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        // A modern server answers a version/header problem as `400` with a
        // JSON-RPC error *body* (`-32022` naming its supported versions) —
        // surface that as the RPC error it is, so era detection can read the
        // code instead of a flattened refusal string.
        if status.is_client_error()
            && let Some(Incoming::Response { result: Err(e), .. }) = parse_incoming(&body)
        {
            return PostOutcome::Err(TransportError::Rpc(e));
        }
        let detail = format!("HTTP {}: {}", status.as_u16(), one_line(&body, 200));
        // A 4xx on POST is the spec's cue to try the legacy transport; the
        // client layer decides whether this config wants that.
        if status.is_client_error() {
            return PostOutcome::Err(TransportError::HttpRefused(detail));
        }
        return PostOutcome::Err(TransportError::Failed(detail));
    }
    if status.as_u16() == 202 || status.as_u16() == 204 {
        return wrap(PostOutcome::Accepted);
    }
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if content_type.contains("text/event-stream") {
        // Drain the SSE body until the request's id answers.
        let Some(want) = want_id else {
            return wrap(PostOutcome::Accepted);
        };
        let mut parser = SseParser::new();
        let reader = BufReader::new(response);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Some(event) = parser.push_line(&line)
                && let Some(Incoming::Response { id, result }) = parse_incoming(&event.data)
                && id == want
            {
                return wrap(PostOutcome::Response(result));
            }
        }
        return PostOutcome::Err(TransportError::Failed(
            "the event stream ended before the response arrived".to_string(),
        ));
    }
    let text = match response.text() {
        Ok(text) => text,
        Err(e) => return PostOutcome::Err(TransportError::Failed(format!("read failed: {e}"))),
    };
    if text.trim().is_empty() {
        return wrap(PostOutcome::Accepted);
    }
    match parse_incoming(&text) {
        Some(Incoming::Response { id, result }) if want_id == Some(id) => {
            wrap(PostOutcome::Response(result))
        }
        Some(_) | None => PostOutcome::Err(TransportError::Failed(format!(
            "unexpected response body: {}",
            one_line(&text, 200)
        ))),
    }
}

// ---------------------------------------------------------------------------
// legacy HTTP+SSE

/// A legacy (2024-11-05) HTTP+SSE server: a persistent GET stream carries
/// every server→client message (the first `endpoint` event naming the POST
/// target); requests POST there and answers arrive on the stream.
pub struct SseTransport {
    endpoint: String,
    remote: RemoteHeaders,
    messages: mpsc::Receiver<String>,
    parked: HashMap<u64, Result<Value, RpcError>>,
    next_id: u64,
}

impl SseTransport {
    /// Open the stream and resolve the POST endpoint. `deadline` bounds the
    /// whole connect.
    pub fn connect(
        url: &str,
        remote: RemoteHeaders,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<Self, TransportError> {
        let (endpoint_tx, endpoint_rx) = mpsc::channel::<Result<String, TransportError>>();
        let (message_tx, messages) = mpsc::channel::<String>();
        let stream_url = url.to_string();
        let stream_remote = remote.clone();
        std::thread::spawn(move || {
            let client = match crate::llm::http_client(SSE_STREAM_TIMEOUT) {
                Ok(client) => client,
                Err(e) => {
                    let _ = endpoint_tx.send(Err(TransportError::Failed(e.to_string())));
                    return;
                }
            };
            let mut req = client
                .get(&stream_url)
                .header("Accept", "text/event-stream");
            req = stream_remote.apply(req);
            let response = match req.send() {
                Ok(response) => response,
                Err(e) => {
                    let _ = endpoint_tx.send(Err(TransportError::Failed(format!(
                        "could not open the event stream: {e}"
                    ))));
                    return;
                }
            };
            if response.status().as_u16() == 401 {
                let challenge = response
                    .headers()
                    .get("www-authenticate")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let _ = endpoint_tx.send(Err(TransportError::NeedsAuth(challenge)));
                return;
            }
            if !response.status().is_success() {
                let _ = endpoint_tx.send(Err(TransportError::Failed(format!(
                    "HTTP {} opening the event stream",
                    response.status().as_u16()
                ))));
                return;
            }
            let mut parser = SseParser::new();
            let mut endpoint_sent = false;
            let reader = BufReader::new(response);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                for event in [parser.push_line(&line)].into_iter().flatten() {
                    match event.event.as_str() {
                        "endpoint" if !endpoint_sent => {
                            endpoint_sent = true;
                            let resolved = resolve_endpoint(&stream_url, &event.data);
                            let _ = endpoint_tx.send(resolved);
                        }
                        _ => {
                            if message_tx.send(event.data).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        });
        let started = Instant::now();
        let endpoint = wait_channel(&endpoint_rx, started, deadline, cancel)??;
        Ok(Self {
            endpoint,
            remote,
            messages,
            parked: HashMap::new(),
            next_id: 1,
        })
    }

    /// Re-arm the bearer the POST side sends (a mid-session token refresh).
    /// The persistent GET stream keeps the credentials it opened with — auth
    /// only matters there at open.
    pub fn set_bearer(&mut self, bearer: Option<String>) {
        self.remote.bearer = bearer;
    }

    fn post(&self, body: &Value) -> Result<Option<Value>, TransportError> {
        let client = crate::llm::http_client(HTTP_OP_TIMEOUT)
            .map_err(|e| TransportError::Failed(e.to_string()))?;
        let mut req = client
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        req = self.remote.apply(req);
        let response = req
            .body(body.to_string())
            .send()
            .map_err(|e| TransportError::Failed(format!("request failed: {e}")))?;
        if response.status().as_u16() == 401 {
            return Err(TransportError::NeedsAuth(None));
        }
        if !response.status().is_success() {
            return Err(TransportError::Failed(format!(
                "HTTP {} posting to the server",
                response.status().as_u16()
            )));
        }
        // Most servers answer 202 and reply on the stream; a few answer the
        // JSON directly — accept both.
        let text = response.text().unwrap_or_default();
        if text.trim().is_empty() {
            return Ok(None);
        }
        Ok(serde_json::from_str(&text).ok())
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<Value, TransportError> {
        let id = self.next_id;
        self.next_id += 1;
        let direct = self.post(&request(id, method, params))?;
        if let Some(direct) = direct
            && let Some(Incoming::Response { id: got, result }) =
                parse_incoming(&direct.to_string())
        {
            if got == id {
                return result.map_err(TransportError::Rpc);
            }
            self.parked.insert(got, result);
        }
        if let Some(result) = self.parked.remove(&id) {
            return result.map_err(TransportError::Rpc);
        }
        let started = Instant::now();
        loop {
            let data = wait_channel(&self.messages, started, deadline, cancel)?;
            match parse_incoming(&data) {
                Some(Incoming::Response { id: got, result }) if got == id => {
                    return result.map_err(TransportError::Rpc);
                }
                Some(Incoming::Response { id: got, result }) => {
                    self.parked.insert(got, result);
                }
                Some(Incoming::Other) | None => {}
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), TransportError> {
        self.post(&notification(method, params)).map(|_| ())
    }
}

/// Resolve the `endpoint` event's URI against the stream URL (it is usually
/// relative — `/messages?sessionId=…`).
fn resolve_endpoint(base: &str, endpoint: &str) -> Result<String, TransportError> {
    let base = url::Url::parse(base)
        .map_err(|e| TransportError::Failed(format!("bad stream URL: {e}")))?;
    base.join(endpoint.trim())
        .map(|u| u.to_string())
        .map_err(|e| TransportError::Failed(format!("bad endpoint URI: {e}")))
}

/// The command a stdio server runs as: piped stdio, the session
/// environment with the config's `env` overlaid, in `cwd` — minus the herdr
/// pane, which is the TUI's own: a server that is an agent of its own
/// (`codex mcp-server`) would take it over with its herdr hook
/// (`docs/herdr.md`). What the config itself sets still wins.
fn stdio_command(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&std::path::Path>,
) -> Result<Command, TransportError> {
    #[cfg(windows)]
    let mut cmd = windows_stdio_command(
        command,
        args,
        || {
            crate::subprocess::validate_tool_shell()
                .map_err(|error| TransportError::Failed(error.to_string()))?;
            Ok(crate::subprocess::tool_shell().to_path_buf())
        },
        |name| {
            let current = std::env::current_dir()
                .map_err(|error| TransportError::Failed(error.to_string()))?;
            let directory = cwd.map_or_else(|| current.clone(), |cwd| current.join(cwd));
            // Match the child's PATH overlay, rather than resolving the parent
            // PATH and then silently launching some other installation.
            let path = env
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
                .map(|(_, value)| std::ffi::OsString::from(value))
                .or_else(|| std::env::var_os("PATH"));
            resolve_windows_stdio_program(name, path.as_deref(), &directory, |path| path.is_file())
        },
    )?;
    #[cfg(not(windows))]
    let mut cmd = {
        let mut cmd = Command::new(command);
        cmd.args(args);
        cmd
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // A pipe server must not open a second console or access the TUI's.
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.env_remove(crate::herdr::PANE_ID_ENV);
    for (key, value) in env {
        cmd.env(key, value);
    }
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    Ok(cmd)
}

/// Windows argv planning, injectable so Linux tests prove argument boundaries
/// without pretending to execute a Windows program. Never interpolate config
/// data into Bash source or a `cmd /c` command string.
#[cfg(any(windows, test))]
fn windows_stdio_command(
    command: &str,
    args: &[String],
    bash: impl FnOnce() -> Result<std::path::PathBuf, TransportError>,
    resolve: impl FnOnce(&str) -> Result<std::path::PathBuf, TransportError>,
) -> Result<Command, TransportError> {
    let program = resolve(command)?;
    let name = program.to_string_lossy();
    let extension = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&name)
        .rsplit_once('.')
        .map(|(_, extension)| extension);
    if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("exe")) {
        let mut cmd = Command::new(&program);
        cmd.args(args);
        return Ok(cmd);
    }
    if extension.is_some_and(|extension| {
        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
    }) {
        // std::process documents special batch-aware escaping, but also warns
        // that cmd parsing cannot represent arbitrary argv safely. Keep only
        // a conservative subset, including spaces/Unicode/empty arguments.
        // Reject expansion, quote, redirection and control characters before
        // spawning; do not guess at escaping or depend on PATHEXT dispatch.
        let safe = |value: &str| {
            !value
                .chars()
                .any(|ch| ch.is_control() || "\"%!^&|<>()".contains(ch))
        };
        if !safe(&name) || args.iter().any(|arg| !safe(arg)) {
            return Err(TransportError::Failed(format!(
                "cannot safely pass these arguments to Windows batch server `{command}`; use its .exe or extensionless Git Bash shim instead"
            )));
        }
        let mut cmd = Command::new(&program);
        cmd.args(args);
        return Ok(cmd);
    }
    let mut cmd = Command::new(bash()?);
    cmd.args(["--noprofile", "--norc", "-c", r#"exec -- "$@""#, "--"])
        .arg(name.replace('\\', "/"))
        .args(args);
    // Git Bash must not rewrite e.g. /literal/path, URLs or --flag=/value in
    // a native server's argv. These are launcher policy, not shell source.
    cmd.env("MSYS2_ARG_CONV_EXCL", "*")
        .env("MSYS_NO_PATHCONV", "1");
    Ok(cmd)
}

/// Find an extensionless npm shell shim before its native exe/batch siblings.
/// Bare names search only absolute PATH entries; explicit relative paths still
/// resolve against the configured server cwd, as they do on Unix.
#[cfg(any(windows, test))]
fn resolve_windows_stdio_program(
    command: &str,
    path: Option<&std::ffi::OsStr>,
    cwd: &std::path::Path,
    installed: impl Fn(&std::path::Path) -> bool,
) -> Result<std::path::PathBuf, TransportError> {
    let named = std::path::Path::new(command);
    let explicit = named.is_absolute() || command.contains(['/', '\\', ':']);
    let bases = if explicit {
        vec![cwd.join(named)]
    } else {
        path.map(|path| {
            std::env::split_paths(path)
                .filter(|directory| directory.is_absolute())
                .map(|directory| directory.join(named))
                .collect()
        })
        .unwrap_or_default()
    };
    for base in bases {
        if installed(&base) {
            return Ok(base);
        }
        if named.extension().is_none() {
            for extension in ["exe", "cmd", "bat"] {
                let mut candidate = base.as_os_str().to_os_string();
                candidate.push(format!(".{extension}"));
                let candidate = std::path::PathBuf::from(candidate);
                if installed(&candidate) {
                    return Ok(candidate);
                }
            }
        }
    }
    Err(TransportError::Failed(format!(
        "could not start `{command}`: executable or Git Bash shim not found on absolute PATH entries"
    )))
}

/// Flatten a body to one bounded line for an error message.
fn one_line(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let head: String = flat.chars().take(max).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn windows_mcp_command(
        command: &str,
        args: &[&str],
        resolved: &str,
    ) -> Result<Command, TransportError> {
        windows_stdio_command(
            command,
            &args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>(),
            || {
                Ok(std::path::PathBuf::from(
                    r"C:\Program Files\Git\bin\bash.exe",
                ))
            },
            |_| Ok(std::path::PathBuf::from(resolved)),
        )
    }

    #[test]
    fn windows_mcp_executables_keep_direct_argv_without_a_shell() {
        let program = r"C:\Program Files\MCP\server.EXE";
        let args = [
            "two words",
            "",
            "東京",
            "a\"b",
            "%PATH%",
            "!x!",
            "x&y",
            "/literal/path",
        ];
        let command = windows_mcp_command(program, &args, program).expect("native executable");
        assert_eq!(command.get_program(), program);
        assert_eq!(command.get_args().collect::<Vec<_>>(), args);
        assert!(
            !command
                .get_envs()
                .any(|(key, _)| key == "MSYS2_ARG_CONV_EXCL")
        );
    }

    #[test]
    fn windows_mcp_npx_shims_forward_positional_arguments_not_shell_source() {
        let args = [
            "-y",
            "@fixture/server",
            "two words",
            "",
            "東京",
            "$(touch owned)",
            "a\"b",
            "%PATH%",
            "!x!",
            "x&y",
            "/literal/path",
        ];
        let command = windows_mcp_command("npx", &args, r"C:\Program Files\nodejs\npx")
            .expect("Git Bash shim");
        assert_eq!(command.get_program(), r"C:\Program Files\Git\bin\bash.exe");
        let expected = [
            "--noprofile",
            "--norc",
            "-c",
            r#"exec -- "$@""#,
            "--",
            "C:/Program Files/nodejs/npx",
        ]
        .into_iter()
        .chain(args)
        .collect::<Vec<_>>();
        assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
        for (key, value) in [("MSYS2_ARG_CONV_EXCL", "*"), ("MSYS_NO_PATHCONV", "1")] {
            assert!(
                command
                    .get_envs()
                    .any(|(name, set)| name == key && set == Some(std::ffi::OsStr::new(value)))
            );
        }
    }

    #[test]
    fn windows_mcp_batch_commands_use_std_command_with_bounded_arguments() {
        for program in [r"C:\MCP tools\npx.cmd", r"C:\MCP tools\server.BAT"] {
            let args = [
                "-y",
                "@fixture/server",
                "two words",
                "",
                "東京",
                r"C:\a path\",
            ];
            let command = windows_mcp_command(program, &args, program).expect("safe batch argv");
            assert_eq!(command.get_program(), program);
            assert_eq!(command.get_args().collect::<Vec<_>>(), args);
        }
        // A bare PATH name resolving to a batch file cannot bypass validation.
        let command =
            windows_mcp_command("server", &["--port", "4000"], r"C:\MCP\server.cmd").unwrap();
        assert_eq!(command.get_program(), r"C:\MCP\server.cmd");
    }

    #[test]
    fn windows_mcp_batch_rejects_unsafe_arguments_instead_of_guessing_cmd_quoting() {
        for arg in [
            "a\"b",
            "%PATH%",
            "!expanded!",
            "x&echo owned",
            "x|y",
            "x>file",
            "x<file",
            "x^y",
            "(x)",
            "x\ny",
            "x\ry",
            "x\0y",
        ] {
            for configured in ["npx.cmd", "npx"] {
                let error = windows_mcp_command(configured, &[arg], r"C:\nodejs\npx.cmd")
                    .expect_err("unsafe batch argument must be refused");
                assert!(
                    matches!(error, TransportError::Failed(ref detail) if detail.contains("batch") && detail.contains(".exe")),
                    "{error}"
                );
            }
        }
        assert!(windows_mcp_command("bad.cmd", &[], r"C:\bad&name\server.cmd").is_err());
    }

    #[test]
    fn windows_mcp_missing_bash_explains_the_prerequisite_without_blocking_exes() {
        let missing = || Err(TransportError::Failed("Git for Windows is required".into()));
        assert!(windows_stdio_command("server.exe", &[], missing, |name| Ok(name.into())).is_ok());
        assert_eq!(
            windows_stdio_command("npx", &[], missing, |name| Ok(name.into())).unwrap_err(),
            TransportError::Failed("Git for Windows is required".into())
        );
    }

    #[test]
    fn windows_mcp_resolution_prefers_a_shim_then_exe_then_batch_on_absolute_path() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("Node with spaces");
        let path =
            std::env::join_paths([std::path::PathBuf::from("relative"), bin.clone()]).unwrap();
        let shim = bin.join("npx");
        let exe = bin.join("npx.exe");
        let batch = bin.join("npx.cmd");
        for (installed, expected) in [
            (vec![shim.clone(), exe.clone(), batch.clone()], shim),
            (vec![exe.clone(), batch.clone()], exe),
            (vec![batch.clone()], batch),
        ] {
            let resolved = resolve_windows_stdio_program("npx", Some(&path), root.path(), |p| {
                installed.iter().any(|candidate| candidate == p)
            })
            .unwrap();
            assert_eq!(resolved, expected);
        }
        let error = resolve_windows_stdio_program("npx", Some(&path), root.path(), |p| {
            p == std::path::Path::new("relative/npx.cmd")
        })
        .unwrap_err();
        assert!(error.to_string().contains("npx"));
        let explicit = root.path().join("local/server.bat");
        assert_eq!(
            resolve_windows_stdio_program("./local/server.bat", None, root.path(), |p| p
                == explicit)
            .unwrap(),
            root.path().join("./local/server.bat")
        );
    }

    #[test]
    fn a_stdio_server_never_inherits_the_herdr_pane() {
        // A server can be an agent of its own (`codex mcp-server`), whose
        // herdr hook would take the TUI's pane over (`docs/herdr.md`).
        let pane = std::ffi::OsStr::new(crate::herdr::PANE_ID_ENV);
        // Only inspect environment setup; no installed server is needed.
        #[cfg(not(windows))]
        let server = "codex";
        #[cfg(windows)]
        let server = crate::subprocess::tool_shell().to_str().unwrap();
        let command =
            stdio_command(server, &["mcp-server".to_string()], &BTreeMap::new(), None).unwrap();
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == pane && value.is_none()),
            "{:?}",
            command.get_envs().collect::<Vec<_>>()
        );
        // What the server's own config sets is still what it gets.
        let env = BTreeMap::from([(crate::herdr::PANE_ID_ENV.to_string(), "w1:p9".to_string())]);
        let command = stdio_command(server, &[], &env, None).unwrap();
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == pane && value == Some(std::ffi::OsStr::new("w1:p9")))
        );
    }

    /// A scripted stdio server: a `sh` process that ignores its stdin and
    /// prints canned responses for ids 1 and 2 (our ids are deterministic).
    fn scripted_stdio(lines: &[&str]) -> StdioTransport {
        let prints = lines
            .iter()
            .map(|l| format!("printf '%s\\n' '{l}'; "))
            .collect::<String>();
        StdioTransport::spawn(
            "sh",
            &[
                "-c".to_string(),
                format!("cat > /dev/null & {prints}sleep 5"),
            ],
            &BTreeMap::new(),
            None,
        )
        .expect("spawn sh")
    }

    #[test]
    fn stdio_requests_match_responses_by_id_even_out_of_order() {
        let mut t = scripted_stdio(&[
            r#"{"jsonrpc":"2.0","method":"notifications/noise"}"#,
            r#"{"jsonrpc":"2.0","id":2,"result":{"second":true}}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":{"first":true}}"#,
        ]);
        let cancel = CancelToken::new();
        let first = t
            .request("one", Value::Null, Duration::from_secs(5), &cancel)
            .expect("first");
        assert_eq!(first, json!({"first": true}));
        // Id 2 was parked while we waited for id 1.
        let second = t
            .request("two", Value::Null, Duration::from_secs(5), &cancel)
            .expect("second");
        assert_eq!(second, json!({"second": true}));
    }

    #[test]
    fn stdio_rpc_errors_and_exits_surface() {
        let mut t = scripted_stdio(&[
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"no such method"}}"#,
        ]);
        let cancel = CancelToken::new();
        let err = t
            .request("nope", Value::Null, Duration::from_secs(5), &cancel)
            .unwrap_err();
        assert_eq!(
            err,
            TransportError::Rpc(RpcError {
                code: -32601,
                message: "no such method".to_string(),
                data: None,
            })
        );

        // A dead server (stderr retained) reads as "exited: {tail}".
        let mut dead = StdioTransport::spawn(
            "sh",
            &["-c".to_string(), "echo boom >&2; exit 1".to_string()],
            &BTreeMap::new(),
            None,
        )
        .expect("spawn");
        std::thread::sleep(Duration::from_millis(200));
        let err = dead
            .request("x", Value::Null, Duration::from_secs(2), &cancel)
            .unwrap_err();
        match err {
            TransportError::Failed(detail) => assert!(detail.contains("boom"), "{detail}"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn stdio_cancel_and_timeout_stop_the_wait() {
        let mut t = scripted_stdio(&[]);
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(
            t.request("x", Value::Null, Duration::from_secs(5), &cancel),
            Err(TransportError::Cancelled)
        );
        let cancel = CancelToken::new();
        let started = Instant::now();
        assert_eq!(
            t.request("x", Value::Null, Duration::from_millis(80), &cancel),
            Err(TransportError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_spawn_failure_names_the_command() {
        match StdioTransport::spawn(
            "definitely-not-a-real-binary-xyz",
            &[],
            &BTreeMap::new(),
            None,
        ) {
            Err(TransportError::Failed(detail)) => {
                assert!(detail.contains("definitely-not-a-real-binary-xyz"));
            }
            Err(other) => panic!("unexpected error: {other:?}"),
            Ok(_) => panic!("unexpectedly spawned"),
        }
    }

    #[test]
    fn endpoint_uris_resolve_against_the_stream_url() {
        assert_eq!(
            resolve_endpoint("https://x.test/sse", "/messages?s=1").unwrap(),
            "https://x.test/messages?s=1"
        );
        assert_eq!(
            resolve_endpoint("https://x.test/a/sse", "msgs").unwrap(),
            "https://x.test/a/msgs"
        );
        assert_eq!(
            resolve_endpoint("https://x.test/sse", "https://y.test/m").unwrap(),
            "https://y.test/m"
        );
    }

    #[test]
    fn one_line_flattens_and_caps() {
        assert_eq!(one_line("a\n  b\t c", 10), "a b c");
        assert_eq!(one_line("abcdef", 3), "abc…");
    }
}
