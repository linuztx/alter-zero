//! The three MCP transports (`docs/tools/mcp.md`) behind one blocking
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
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::mcp::{
    Incoming, LEGACY_PROTOCOL_VERSION, ParamHeader, RpcError, SseParser, header_value,
    initialize_params_for, notification, param_header_values, parse_incoming, request,
    server_request_answer, with_meta,
};
use crate::stream::CancelToken;

/// The cancel-poll cadence every blocking wait uses.
const POLL: Duration = Duration::from_millis(20);

/// How much of a stdio server's stderr is retained for a connect error.
const STDERR_TAIL_BYTES: usize = 4 * 1024;

/// The pooled HTTP client's own stall guard. A POST overrides it with its
/// request's budget (`post_blocking`) — what a tool may take is the call's
/// timeout, never this — so it only bounds the fire-and-forget answers.
/// Short under test, so a test can prove the override.
#[cfg(not(test))]
const HTTP_OP_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(test)]
const HTTP_OP_TIMEOUT: Duration = Duration::from_secs(1);

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

    /// Send a notification (no response expected), within `deadline` and
    /// polling `cancel` like a request — a server slow to accept it must not
    /// hold a handshake past its budget, nor Esc.
    pub fn notify(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<(), TransportError> {
        match self {
            Self::Stdio(t) => t.notify(method, params),
            Self::Http(t) => t.notify(method, params, deadline, cancel),
            Self::Sse(t) => t.notify(method, params, deadline, cancel),
        }
    }

    /// Re-arm the bearer a remote transport sends — the mid-session token
    /// refresh seam (`docs/tools/mcp.md`): a rotated access token reaches the next
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

    /// Does this transport mirror `x-mcp-header` parameters into
    /// `Mcp-Param-{Name}` headers — modern streamable HTTP, the one place
    /// the spec makes it a MUST (other transports MAY ignore the
    /// annotation, and a legacy revision never defined it)?
    #[must_use]
    pub const fn mirrors_param_headers(&self) -> bool {
        match self {
            Self::Http(t) => t.modern.is_some(),
            Self::Stdio(_) | Self::Sse(_) => false,
        }
    }

    /// Can this connection still carry a request? A stdio server that has
    /// exited (or whose pipe broke) can't, nor can a legacy SSE session
    /// whose stream ended — its endpoint died with it. Streamable HTTP holds
    /// nothing open between requests, so it always can.
    pub fn is_alive(&mut self) -> bool {
        match self {
            Self::Stdio(t) => {
                t.writer.is_some()
                    && t.child
                        .as_mut()
                        .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
            }
            Self::Http(_) => true,
            Self::Sse(t) => t.alive.load(Ordering::Relaxed),
        }
    }

    /// Stop the connection now: a stdio server the spec's way
    /// ([`StdioTransport::shutdown`]), a legacy SSE stream closed. A
    /// streamable-HTTP connection holds nothing open between requests.
    pub fn shutdown(&mut self, grace: Duration) {
        match self {
            Self::Stdio(t) => t.shutdown(grace),
            Self::Http(t) => t.stop_listening(),
            Self::Sse(t) => t.closed.store(true, Ordering::Relaxed),
        }
    }

    /// Hand the server's change notifications to `sink` from now on, read
    /// wherever this transport reads the server — and, when the server
    /// promised them (`subscribe`, its `tools.listChanged`), open what it
    /// sends them on: a modern stdio server's `subscriptions/listen`, a
    /// streamable-HTTP server's long-lived stream (`docs/tools/mcp.md`).
    pub fn watch_changes(&mut self, sink: ChangeSink, subscribe: bool) {
        match self {
            Self::Stdio(t) => t.watch_changes(sink, subscribe),
            Self::Http(t) => t.watch_changes(sink, subscribe),
            Self::Sse(t) => t.changes.set(sink),
        }
    }

    /// Record each tool's `x-mcp-header` declarations (by raw tool name) for
    /// the `tools/call` requests to mirror — a no-op off modern HTTP.
    pub fn set_param_headers(&mut self, declarations: HashMap<String, Vec<ParamHeader>>) {
        if let Self::Http(t) = self {
            t.param_headers = declarations;
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

/// Who hears a server's change notifications — the manager, which lists
/// what changed again (`docs/tools/mcp.md`). Called with the notification's
/// method, on whichever thread read it: it must return at once.
pub type ChangeSink = Arc<dyn Fn(&str) + Send + Sync>;

/// A transport's change sink, shared with the threads that read the server.
#[derive(Clone, Default)]
pub(super) struct Changes(Arc<Mutex<Option<ChangeSink>>>);

impl Changes {
    fn set(&self, sink: ChangeSink) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(sink);
        }
    }

    /// Hand `message` to the sink when it is a notification.
    fn deliver(&self, message: &str) {
        let Some(method) = crate::mcp::notification_method(message) else {
            return;
        };
        let sink = self.0.lock().ok().and_then(|slot| slot.clone());
        if let Some(sink) = sink {
            sink(&method);
        }
    }
}

/// A local child process speaking newline-delimited JSON-RPC on its stdio,
/// launched **detached from the TUI's terminal** — a session and process
/// group of its own ([`crate::subprocess`]'s tier walk, the one every shell
/// child takes), so a server running `sudo` or an SSH prompt fails fast on
/// `/dev/tty` instead of reading the user's keystrokes, and stopping it
/// takes everything it started.
pub struct StdioTransport {
    /// The server process; `None` once it has been shut down.
    child: Option<Child>,
    /// The write side: each message one line, written in order by a thread
    /// of its own, so a server that stops reading (a full pipe) costs a call
    /// its deadline rather than the turn's thread for good. Dropping it
    /// closes the server's stdin — the spec's graceful-shutdown signal.
    writer: Option<mpsc::Sender<Vec<u8>>>,
    /// The read side: each stdout line, or why one couldn't be read.
    lines: mpsc::Receiver<Result<String, String>>,
    /// Responses that arrived before the request they answer was waited on
    /// (a fixture printing ahead). A late answer to a request already given
    /// up on is dropped instead, as the spec says, rather than kept for good.
    parked: HashMap<u64, Result<Value, RpcError>>,
    /// The retained stderr tail — what a connect error shows.
    stderr: Arc<Mutex<String>>,
    /// Who hears the server's change notifications, fed by the reader
    /// thread — which reads every line whether or not a request waits.
    changes: Changes,
    /// The modern (2026-07-28) protocol version when the era detection
    /// settled modern — every request's params then carry the `_meta`
    /// block. Stdio has no header layer: the message body is everything.
    pub(super) modern: Option<String>,
    next_id: u64,
}

/// How long a stopping server gets at each step — to exit once its stdin
/// closes, then once it is sent `SIGTERM` — before the next.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

impl StdioTransport {
    /// Launch the configured command with piped stdio. The child inherits
    /// the session environment with the config's `env` overlaid and runs in
    /// `cwd`, in a session of its own (`detach_helper` is this binary, for
    /// the tier that re-execs it where no `setsid` binary exists); dropping
    /// the transport stops it in the background, [`Self::shutdown`] in the
    /// foreground.
    pub fn spawn(
        command: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: Option<&std::path::Path>,
        detach_helper: Option<&std::path::Path>,
    ) -> Result<Self, TransportError> {
        // The launch goes through a shell (`exec …`, so the shell *becomes*
        // the server), which turns a missing program into an exit after the
        // fact; say so up front, by name, instead.
        if !command_exists(command, env, cwd) {
            return Err(TransportError::Failed(format!(
                "could not start `{command}`: not found (is it installed and on PATH?)"
            )));
        }
        let configure = stdio_configure(env, cwd);
        let mut child = crate::subprocess::spawn_shell_with(
            detach_helper,
            &exec_line(command, args),
            configure,
        )
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
        let (writer, queue) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut stdin = stdin;
            for message in queue {
                if stdin
                    .write_all(&message)
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
            // Dropping `stdin` here closes the pipe: the server reads EOF.
        });
        let (tx, lines) = mpsc::channel();
        let changes = Changes::default();
        let heard = changes.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let line = match read_line_capped(&mut reader, MAX_MESSAGE_BYTES) {
                    Ok(Some(line)) => {
                        heard.deliver(&line);
                        Ok(line)
                    }
                    Ok(None) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                        // The rest of the oversized line is noise — skip it,
                        // so it can't fail the requests after this one too.
                        if reader.skip_until(b'\n').is_err() {
                            break;
                        }
                        Err(e.to_string())
                    }
                    Err(_) => break,
                };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        if let Some(mut stderr) = stderr {
            let tail = Arc::clone(&stderr_tail);
            // Read in chunks, not lines: a progress bar redrawn with `\r`
            // never ends one, and the pipe must be drained whatever the
            // server writes — a reader that stopped would leave it to die of
            // `EPIPE` on its next log line.
            std::thread::spawn(move || {
                let mut chunk = [0u8; 4096];
                loop {
                    match stderr.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Ok(mut tail) = tail.lock() {
                                tail.push_str(&String::from_utf8_lossy(&chunk[..n]));
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
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
            });
        }
        Ok(Self {
            child: Some(child),
            writer: Some(writer),
            lines,
            parked: HashMap::new(),
            stderr: stderr_tail,
            changes,
            modern: None,
            next_id: 1,
        })
    }

    /// [`Transport::watch_changes`] for stdio: the reader thread already
    /// sees every line, so a legacy server's unsolicited notification needs
    /// nothing more. A modern (2026-07-28) server sends none it was not asked
    /// for, so one that promised them is asked: a `subscriptions/listen`
    /// written now and never waited on — the subscription lasts as long as
    /// the process, and its eventual answer is a late reply the request
    /// loop drops.
    fn watch_changes(&mut self, sink: ChangeSink, subscribe: bool) {
        self.changes.set(sink);
        if subscribe && let Some(version) = self.modern.clone() {
            let id = self.next_id;
            self.next_id += 1;
            let params = with_meta(crate::mcp::listen_params(), &version);
            let _ = self.send(&request(id, "subscriptions/listen", params));
        }
    }

    /// The retained stderr tail — folded into a connect error so a crashing
    /// server explains itself.
    #[must_use]
    pub fn stderr_tail(&self) -> String {
        self.stderr.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// A handle on the stderr tail that outlives the transport — what a
    /// connect reads *after* the handshake failed, when there is something
    /// in it.
    #[must_use]
    pub fn stderr_handle(&self) -> Arc<Mutex<String>> {
        Arc::clone(&self.stderr)
    }

    /// Stop the server now, in the spec's order — close its stdin, give it
    /// `grace` to exit, `SIGTERM` its process group, `grace` again, then
    /// `SIGKILL` — and reap it, anything it left running in its group going
    /// with it.
    pub fn shutdown(&mut self, grace: Duration) {
        drop(self.writer.take());
        if let Some(child) = self.child.take() {
            stop_server(child, grace);
        }
    }

    fn send(&mut self, message: &Value) -> Result<(), TransportError> {
        let mut line = message.to_string();
        line.push('\n');
        match &self.writer {
            Some(writer) if writer.send(line.into_bytes()).is_ok() => Ok(()),
            // The writer thread is gone: the pipe broke, so the child died —
            // explain with its stderr rather than a bare broken pipe.
            _ => Err(self.exit_error()),
        }
    }

    /// A dead child's story: wait briefly for it to be reaped (so the stderr
    /// reader has drained), then report the tail — a crashing `npx` explains
    /// itself instead of reading as a bare broken pipe.
    fn exit_error(&mut self) -> TransportError {
        let waited = Instant::now();
        if let Some(child) = self.child.as_mut() {
            while waited.elapsed() < Duration::from_millis(500) {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => std::thread::sleep(POLL),
                    Err(_) => break,
                }
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

    /// Tell the server a request was given up on — on stdio there is no
    /// stream to close, so this is the only way it learns (a MUST in
    /// 2026-07-28, the 2025-11-25 cancellation in a legacy session). Never
    /// for `initialize`, which a legacy client may not cancel.
    fn cancel_request(&mut self, id: u64, method: &str, why: &TransportError) {
        if method == "initialize" {
            return;
        }
        let reason = match why {
            TransportError::Timeout => "the request timed out",
            _ => "the user cancelled the request",
        };
        let _ = self.send(&notification(
            "notifications/cancelled",
            json!({"requestId": id, "reason": reason}),
        ));
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<Value, TransportError> {
        // A turn already cancelled sends nothing: a side-effecting call the
        // user pressed Esc on must not run anyway.
        if cancel.is_cancelled() {
            return Err(TransportError::Cancelled);
        }
        let id = self.next_id;
        self.next_id += 1;
        let params = match &self.modern {
            Some(version) => with_meta(params, version),
            None => params,
        };
        self.send(&request(id, method, params))?;
        if let Some(result) = self.parked.remove(&id) {
            return result.map_err(TransportError::Rpc);
        }
        let started = Instant::now();
        loop {
            let line = match wait_channel(&self.lines, started, deadline, cancel) {
                Ok(Ok(line)) => line,
                Ok(Err(unreadable)) => return Err(TransportError::Failed(unreadable)),
                // A closed pipe usually means the child died — say so with
                // its stderr.
                Err(TransportError::Failed(_)) => return Err(self.exit_error()),
                Err(given_up) => {
                    self.cancel_request(id, method, &given_up);
                    return Err(given_up);
                }
            };
            match parse_incoming(&line) {
                Some(Incoming::Response { id: got, result }) if got == id => {
                    return result.map_err(TransportError::Rpc);
                }
                // An answer to a request not waited on yet — kept for it.
                Some(Incoming::Response { id: got, result }) if got > id => {
                    self.parked.insert(got, result);
                }
                // An error the server could not tie to an id: with one
                // request in flight, it is this one's answer.
                Some(Incoming::OrphanError(error)) => return Err(TransportError::Rpc(error)),
                // A legacy server's own request — `ping`, most likely — is
                // owed an answer; a modern client MUST NOT send responses.
                Some(Incoming::Request {
                    id: asked,
                    method: asked_for,
                }) if self.modern.is_none() => {
                    let _ = self.send(&server_request_answer(&asked, &asked_for));
                }
                // A late answer to a request already given up on, a
                // notification, noise: ignored.
                _ => {}
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), TransportError> {
        self.send(&notification(method, params))
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Off the dropping thread — a reconnect or a disable from the UI
        // must not wait out a server's grace period.
        let writer = self.writer.take();
        if let Some(child) = self.child.take() {
            std::thread::spawn(move || {
                drop(writer);
                stop_server(child, SHUTDOWN_GRACE);
            });
        }
    }
}

/// The spec's shutdown sequence for a server whose stdin is already closed:
/// `grace` to exit on its own, `SIGTERM` to its process group and `grace`
/// again, then `SIGKILL`. A server that exits cleanly still has its group
/// cleared — an `npx` wrapper's real server, a `sleep &` it forgot.
fn stop_server(mut child: Child, grace: Duration) {
    if wait_exit(&mut child, grace) {
        #[cfg(unix)]
        if crate::subprocess::group_outlives(&child) {
            signal_group(&child, rustix::process::Signal::TERM);
            std::thread::sleep(POLL * 5);
            crate::subprocess::kill_process_group(&mut child);
        }
        return;
    }
    #[cfg(unix)]
    {
        signal_group(&child, rustix::process::Signal::TERM);
        if wait_exit(&mut child, grace) {
            if crate::subprocess::group_outlives(&child) {
                crate::subprocess::kill_process_group(&mut child);
            }
            return;
        }
    }
    crate::subprocess::kill_process_group(&mut child);
}

/// Wait up to `grace` for `child` to exit (reaping it) — `true` if it did.
fn wait_exit(child: &mut Child, grace: Duration) -> bool {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if started.elapsed() < grace => std::thread::sleep(POLL),
            Ok(None) | Err(_) => return false,
        }
    }
}

/// Send `signal` to `child`'s process group — the server leads its own
/// (`setsid`, or `process_group(0)` on the attached tier), so its pid is the
/// group's. Best-effort, like every other kill here.
#[cfg(unix)]
fn signal_group(child: &Child, signal: rustix::process::Signal) {
    if let Some(pgid) = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(pgid, signal);
    }
}

/// The shell line a stdio server is launched with: `exec` and the command
/// line, each word quoted for a POSIX shell — the shell *becomes* the
/// server, so the pid the tier walk returns is the server's own.
fn exec_line(command: &str, args: &[String]) -> String {
    let mut line = String::from("exec");
    for word in std::iter::once(command).chain(args.iter().map(String::as_str)) {
        line.push(' ');
        line.push_str(&shell_word(word));
    }
    line
}

/// One word quoted for a POSIX shell: bare when it holds only characters no
/// shell treats specially, else single-quoted with `'` written `'\''`.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./=:,@%+-".contains(&b));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// Is `command` something the launch can run — a path to an executable
/// file (relative ones against `cwd`), or a program on the `PATH` the
/// server will get (the config's own, else the session's)?
fn command_exists(
    command: &str,
    env: &BTreeMap<String, String>,
    cwd: Option<&std::path::Path>,
) -> bool {
    use crate::subprocess::{find_on_path, is_executable_file};
    if command.contains('/') {
        let path = std::path::Path::new(command);
        return match cwd {
            Some(cwd) if path.is_relative() => is_executable_file(&cwd.join(path)),
            _ => is_executable_file(path),
        };
    }
    let path = env
        .get("PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"));
    find_on_path(command, path.as_deref(), is_executable_file).is_some()
}

/// What every tier's `Command` gets before a stdio server's launch: stdin
/// piped (the shared detach default is `/dev/null`), the config's `env`
/// over the session's — the herdr pane id already dropped by the tier
/// (`docs/architecture/herdr.md`; a server can be an agent whose herdr hook would take
/// the TUI's pane over), so only a config that sets it on purpose passes
/// one — and `cwd`.
fn stdio_configure(
    env: &BTreeMap<String, String>,
    cwd: Option<&std::path::Path>,
) -> impl Fn(&mut Command) + use<> {
    let env = env.clone();
    let cwd = cwd.map(std::path::Path::to_path_buf);
    move |cmd: &mut Command| {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &env {
            cmd.env(key, value);
        }
        if let Some(cwd) = &cwd {
            cmd.current_dir(cwd);
        }
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
/// session id a modern answer names anyway is never echoed back).
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
    /// Each tool's `x-mcp-header` declarations, by its raw name — what a
    /// modern `tools/call` mirrors into `Mcp-Param-{Name}` headers.
    param_headers: HashMap<String, Vec<ParamHeader>>,
    /// Who hears the server's change notifications.
    changes: Changes,
    /// The connection as the change stream should open it — kept current
    /// on every request, so a refreshed bearer or a new legacy session
    /// reaches its next reopening.
    live: Arc<Mutex<Option<Endpoint>>>,
    /// The change stream's stop flag, while one runs.
    listener: Option<Arc<AtomicBool>>,
    next_id: u64,
}

impl Drop for HttpTransport {
    fn drop(&mut self) {
        self.stop_listening();
    }
}

/// The modern request-metadata headers one POST carries.
struct ModernHeaders {
    /// `Mcp-Method` — the JSON-RPC method, verbatim.
    method: String,
    /// `Mcp-Name` — `params.name`/`params.uri` for the three methods that
    /// have one (`tools/call`, `resources/read`, `prompts/get`).
    name: Option<String>,
    /// The `Mcp-Param-{Name}` headers a `tools/call` mirrors from its
    /// arguments, already encoded.
    params: Vec<(String, String)>,
}

/// Where one POST goes and what rides it — copied onto the worker thread.
#[derive(Clone)]
struct Endpoint {
    url: String,
    remote: RemoteHeaders,
    session: Option<String>,
    protocol_version: Option<String>,
    /// A legacy (handshake-era) server may send its own requests on a
    /// response stream and waits for the answer; a modern one never does.
    legacy: bool,
    /// Who hears a notification that arrives on a response stream.
    changes: Changes,
}

/// What one POST resolved to, off the worker thread.
enum PostOutcome {
    /// The response for the request id (or an error object).
    Response(Result<Value, RpcError>),
    /// A notification-style acceptance (any 2xx to a notification, a 202 or
    /// 204, an empty body).
    Accepted,
    /// The server answered 404 to a request carrying our session id: the
    /// session is gone (a restart, an idle timeout) and the spec's answer is
    /// a fresh `initialize`. Only reachable in legacy mode — the modern era
    /// has no sessions, so a 404 there is an unknown method.
    SessionExpired,
    /// The response stream ended before the request's answer arrived. Under
    /// 2026-07-28 the request is lost — no resumption — and "MUST" be
    /// re-issued under a new id.
    StreamBroken,
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
            param_headers: HashMap::new(),
            changes: Changes::default(),
            live: Arc::new(Mutex::new(None)),
            listener: None,
            next_id: 1,
        }
    }

    /// The connection one request goes out on, and the one the change
    /// stream reopens with.
    fn endpoint(&self) -> Endpoint {
        Endpoint {
            url: self.url.clone(),
            remote: self.remote.clone(),
            session: self.session.clone(),
            protocol_version: self.protocol_version.clone(),
            legacy: self.modern.is_none(),
            changes: self.changes.clone(),
        }
    }

    /// Keep the change stream's view of the connection current.
    fn publish(&self) {
        if let Ok(mut live) = self.live.lock() {
            *live = Some(self.endpoint());
        }
    }

    /// [`Transport::watch_changes`] for streamable HTTP: a notification on
    /// any response stream reaches `sink`, and a server that promised them
    /// gets a stream of its own for them — a modern server's
    /// `subscriptions/listen`, an earlier one's GET stream — reopened when
    /// it ends, given up when the server offers none or fails it over and
    /// over (`listen_for_changes`).
    fn watch_changes(&mut self, sink: ChangeSink, subscribe: bool) {
        self.changes.set(sink);
        if subscribe && self.listener.is_none() {
            self.publish();
            let stop = Arc::new(AtomicBool::new(false));
            self.listener = Some(Arc::clone(&stop));
            let live = Arc::clone(&self.live);
            let modern = self.modern.clone();
            std::thread::spawn(move || listen_for_changes(&live, &stop, modern.as_deref()));
        }
    }

    /// End the change stream, if one runs — at its next line or wake-up.
    fn stop_listening(&mut self) {
        if let Some(stop) = self.listener.take() {
            stop.store(true, Ordering::Relaxed);
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
        self.publish();
    }

    fn post(
        &self,
        body: Value,
        want_id: Option<u64>,
        modern: Option<ModernHeaders>,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<PostOutcome, TransportError> {
        let endpoint = self.endpoint();
        if self.listener.is_some() {
            self.publish();
        }
        let (tx, rx) = mpsc::channel();
        let started = Instant::now();
        // Set when this wait gives up: the worker then drops the response —
        // closing a response stream is how streamable HTTP cancels — at its
        // next line instead of reading on until its own timeout.
        let abandoned = Arc::new(AtomicBool::new(false));
        let worker_abandoned = Arc::clone(&abandoned);
        std::thread::spawn(move || {
            let outcome = post_blocking(
                &endpoint,
                modern.as_ref(),
                &body,
                want_id,
                deadline,
                &worker_abandoned,
            );
            let _ = tx.send(outcome);
        });
        let outcome = wait_channel(&rx, started, deadline, cancel);
        if outcome.is_err() {
            abandoned.store(true, Ordering::Relaxed);
        }
        outcome
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
        let params = match (method, &name) {
            ("tools/call", Some(tool)) => {
                self.param_headers
                    .get(tool)
                    .map_or_else(Vec::new, |declarations| {
                        param_header_values(
                            declarations,
                            params.get("arguments").unwrap_or(&Value::Null),
                        )
                    })
            }
            _ => Vec::new(),
        };
        Some(ModernHeaders {
            method: method.to_string(),
            name,
            params,
        })
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<Value, TransportError> {
        let started = Instant::now();
        let left = || deadline.saturating_sub(started.elapsed());
        let params = match &self.modern {
            Some(version) => with_meta(params, version),
            None => params,
        };
        let mut outcome = self.send_once(method, &params, left(), cancel)?;
        if matches!(outcome, PostOutcome::SessionExpired) {
            // The session died under us. Start a new one and replay, so a
            // server restart or an idle timeout costs a round trip instead
            // of the whole connection.
            self.reinitialize(left(), cancel)?;
            outcome = self.send_once(method, &params, left(), cancel)?;
        }
        if matches!(outcome, PostOutcome::StreamBroken) && self.modern.is_some() {
            // 2026-07-28 has no resumption: the request was lost with its
            // stream, and goes again under a new id — once.
            outcome = self.send_once(method, &params, left(), cancel)?;
        }
        match outcome {
            PostOutcome::Response(result) => result.map_err(TransportError::Rpc),
            PostOutcome::Accepted => Err(TransportError::Failed(
                "the server accepted the request but sent no response".to_string(),
            )),
            PostOutcome::SessionExpired => Err(TransportError::Failed(
                "the server keeps expiring the session".to_string(),
            )),
            PostOutcome::StreamBroken => Err(TransportError::Failed(
                "the event stream ended before the response arrived".to_string(),
            )),
            PostOutcome::Err(e) => Err(e),
            PostOutcome::Session(..) => unreachable!("unwrapped in send_once"),
        }
    }

    /// One POST of `method`, capturing any session id a legacy server names.
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
                // Sessions are the legacy revisions' alone: a modern answer
                // naming one anyway is never echoed back, so a later 404 —
                // a modern server's unknown method — can't be mistaken for
                // an expired session.
                if self.modern.is_none()
                    && let Some(session) = session
                {
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
        let started = Instant::now();
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
            PostOutcome::SessionExpired | PostOutcome::StreamBroken => {
                return Err(TransportError::Failed(
                    "the server dropped the session it was opening".to_string(),
                ));
            }
            PostOutcome::Session(..) => unreachable!("unwrapped in send_once"),
        }
        self.notify(
            "notifications/initialized",
            Value::Null,
            deadline.saturating_sub(started.elapsed()),
            cancel,
        )
    }

    fn notify(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<(), TransportError> {
        // An acceptance failure is not worth failing the connect over, but a
        // transport error is surfaced. (Only the legacy handshake sends
        // notifications — the modern era has none of ours — but the `_meta`
        // and header rules are honoured all the same.)
        let params = match &self.modern {
            Some(version) => with_meta(params, version),
            None => params,
        };
        let modern = self.modern_headers(method, &params);
        match self.post(notification(method, params), None, modern, deadline, cancel)? {
            PostOutcome::Err(e) => Err(e),
            _ => Ok(()),
        }
    }
}

/// The most one MCP message — a JSON body, one SSE line — may hold: 16 MiB,
/// Claude Code's bound. Every network read is bounded (`docs/architecture/memory.md`), and
/// a server streaming without end must not grow this process without end.
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// How much of an error body is kept for the message that reports it.
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// How long the change stream waits on a quiet server before reopening —
/// keep-alive comments are encouraged by the spec, not promised.
const LISTEN_READ_TIMEOUT: Duration = Duration::from_secs(300);

/// How many openings of the change stream may fail in a row before the
/// listener gives up on it.
const LISTEN_MAX_FAILURES: u32 = 5;

/// A change stream that ends sooner than this after opening counts as a
/// failed opening — a server closing it at once must not be reopened in a
/// tight loop.
const LISTEN_MIN_LIFE: Duration = Duration::from_secs(2);

/// Why a change stream couldn't be opened.
enum ListenRefused {
    /// The server offers none, or won't let us have one (400/401/403/404/
    /// 405, or an answer that isn't an event stream): stop asking.
    Never,
    /// Anything else — a network failure, a 5xx: try again later.
    Later,
}

/// Hold a change stream open for as long as the transport lives, handing
/// every notification on it to the endpoint's sink (`docs/tools/mcp.md`): a
/// modern server's `subscriptions/listen` response stream, an earlier one's
/// GET stream. Reopened when it ends — a timeout, a server restart — with a
/// growing pause, and given up when the server offers no stream or every
/// opening in a row fails.
fn listen_for_changes(live: &Mutex<Option<Endpoint>>, stop: &AtomicBool, modern: Option<&str>) {
    // The listen request's id: a range of its own, never one a call uses.
    let mut id: u64 = 1 << 40;
    let mut failures: u32 = 0;
    while !stop.load(Ordering::Relaxed) {
        let Some(endpoint) = live.lock().ok().and_then(|endpoint| endpoint.clone()) else {
            return;
        };
        let opened = Instant::now();
        match open_change_stream(&endpoint, modern, id) {
            Ok(response) => {
                let mut parser = SseParser::new();
                let mut reader = BufReader::new(response);
                while !stop.load(Ordering::Relaxed) {
                    let Ok(Some(line)) = read_line_capped(&mut reader, MAX_MESSAGE_BYTES) else {
                        break;
                    };
                    if let Some(event) = parser.push_line(&line) {
                        endpoint.changes.deliver(&event.data);
                    }
                }
                if opened.elapsed() < LISTEN_MIN_LIFE {
                    failures += 1;
                } else {
                    failures = 0;
                }
            }
            Err(ListenRefused::Never) => return,
            Err(ListenRefused::Later) => failures += 1,
        }
        if failures >= LISTEN_MAX_FAILURES {
            return;
        }
        // A pause before reopening, growing with each failure in a row,
        // woken early by the stop flag.
        let pause = Duration::from_millis(500) * 2u32.saturating_pow(failures).min(120);
        let until = Instant::now() + pause;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
        id += 1;
    }
}

/// Open one change stream: `subscriptions/listen` POSTed to a modern
/// server (its response is the stream), a GET to an earlier one.
fn open_change_stream(
    endpoint: &Endpoint,
    modern: Option<&str>,
    id: u64,
) -> Result<reqwest::blocking::Response, ListenRefused> {
    let client =
        crate::llm::mcp_http_client(LISTEN_READ_TIMEOUT).map_err(|_| ListenRefused::Never)?;
    let mut req = match modern {
        Some(version) => client
            .post(&endpoint.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Method", "subscriptions/listen")
            .body(
                request(
                    id,
                    "subscriptions/listen",
                    with_meta(crate::mcp::listen_params(), version),
                )
                .to_string(),
            ),
        None => client
            .get(&endpoint.url)
            .header("Accept", "text/event-stream"),
    };
    req = endpoint.remote.apply(req.timeout(LISTEN_READ_TIMEOUT));
    if let Some(session) = &endpoint.session {
        req = req.header("Mcp-Session-Id", session);
    }
    if let Some(version) = &endpoint.protocol_version {
        req = req.header("MCP-Protocol-Version", version);
    }
    let response = req.send().map_err(|_| ListenRefused::Later)?;
    let status = response.status().as_u16();
    if matches!(status, 400 | 401 | 403 | 404 | 405) {
        return Err(ListenRefused::Never);
    }
    if !response.status().is_success() {
        return Err(ListenRefused::Later);
    }
    let streaming = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().contains("text/event-stream"));
    if streaming {
        Ok(response)
    } else {
        Err(ListenRefused::Never)
    }
}

/// Read one line of at most `cap` bytes (its `\n` dropped; a `\r` left for
/// the SSE parser), decoded lossily — invalid UTF-8 from a misbehaving
/// server costs a replacement character, not the stream. `Ok(None)` at the
/// end of the stream; an error for a failed read or a longer line.
fn read_line_capped(reader: &mut impl BufRead, cap: usize) -> std::io::Result<Option<String>> {
    let mut buf = Vec::new();
    let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
    let read = reader.by_ref().take(limit).read_until(b'\n', &mut buf)?;
    if read == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() > cap {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("a message from the server is over {}", byte_size(cap)),
        ));
    }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

/// Read a whole body of at most `cap` bytes, decoded lossily.
fn read_body_capped(response: impl Read, cap: usize) -> Result<String, String> {
    let mut buf = Vec::new();
    let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
    response
        .take(limit)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;
    if buf.len() > cap {
        return Err(format!("the server's answer is over {}", byte_size(cap)));
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// `16 MB`, for the over-the-cap messages.
fn byte_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{} MB", bytes / (1024 * 1024))
    } else {
        format!("{} KB", bytes / 1024)
    }
}

/// The `WWW-Authenticate` challenge on a response, when it carries one.
fn challenge_of(response: &reqwest::blocking::Response) -> Option<String> {
    response
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// A `403` that is an authentication state rather than a refusal: the
/// challenge's `insufficient_scope` (RFC 6750 §3.1) says the token is fine
/// but grants too little — the step-up the OAuth flow answers.
fn wants_more_scope(challenge: Option<&str>) -> bool {
    challenge.is_some_and(|c| c.to_ascii_lowercase().contains("insufficient_scope"))
}

/// Post a legacy server's due answer to its request, off the read loop —
/// best-effort: a server that drops it gets nothing worse than silence.
fn answer_server_request(endpoint: &Endpoint, id: &Value, method: &str) {
    let answer = server_request_answer(id, method);
    let endpoint = endpoint.clone();
    std::thread::spawn(move || {
        let Ok(client) = crate::llm::mcp_http_client(HTTP_OP_TIMEOUT) else {
            return;
        };
        let mut req = client
            .post(&endpoint.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        req = endpoint.remote.apply(req);
        if let Some(session) = &endpoint.session {
            req = req.header("Mcp-Session-Id", session);
        }
        if let Some(version) = &endpoint.protocol_version {
            req = req.header("MCP-Protocol-Version", version);
        }
        let _ = req.body(answer.to_string()).send();
    });
}

/// The blocking POST body — runs on a worker thread. `deadline` is the
/// request's own budget: it bounds the wait for the headers and every read
/// of the body, so a tool that takes longer than the pooled client's stall
/// guard — but not longer than its call was given — still answers.
fn post_blocking(
    endpoint: &Endpoint,
    modern: Option<&ModernHeaders>,
    body: &Value,
    want_id: Option<u64>,
    deadline: Duration,
    abandoned: &AtomicBool,
) -> PostOutcome {
    let client = match crate::llm::mcp_http_client(HTTP_OP_TIMEOUT) {
        Ok(client) => client,
        Err(e) => return PostOutcome::Err(TransportError::Failed(e.to_string())),
    };
    let mut req = client
        .post(&endpoint.url)
        .timeout(deadline.max(Duration::from_millis(50)))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    req = endpoint.remote.apply(req);
    if let Some(session) = &endpoint.session {
        req = req.header("Mcp-Session-Id", session);
    }
    if let Some(version) = &endpoint.protocol_version {
        req = req.header("MCP-Protocol-Version", version);
    }
    if let Some(headers) = modern {
        req = req.header("Mcp-Method", headers.method.as_str());
        if let Some(name) = &headers.name {
            req = req.header("Mcp-Name", header_value(name));
        }
        for (header, value) in &headers.params {
            req = req.header(header.as_str(), value.as_str());
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
    let challenge = challenge_of(&response);
    if status.as_u16() == 401 || (status.as_u16() == 403 && wants_more_scope(challenge.as_deref()))
    {
        return PostOutcome::Err(TransportError::NeedsAuth(challenge));
    }
    // A 404 to a request that carried a session id means that session is
    // gone — the spec's cue to open a new one and replay. Gated on having
    // sent one, because a modern (sessionless) server answers an unknown
    // *method* with 404 and re-handshaking would be nonsense there.
    if status.as_u16() == 404 && endpoint.session.is_some() {
        return PostOutcome::SessionExpired;
    }
    if !status.is_success() {
        let body = read_body_capped(response, MAX_ERROR_BODY_BYTES).unwrap_or_default();
        // A modern server answers a version/header problem as `400` with a
        // JSON-RPC error *body* (`-32022` naming its supported versions) —
        // surface that as the RPC error it is, so era detection can read the
        // code instead of a flattened refusal string. An `id: null` body
        // counts too: the SDKs send their transport-level errors that way,
        // and this POST's error is this request's.
        if status.is_client_error()
            && let Some(Incoming::Response { result: Err(e), .. } | Incoming::OrphanError(e)) =
                parse_incoming(&body)
        {
            return PostOutcome::Err(TransportError::Rpc(e));
        }
        let detail = format!("HTTP {}: {}", status.as_u16(), one_line(&body, 200));
        // Only 400, 404 and 405 without a modern error body are the spec's
        // cue to try the legacy HTTP+SSE transport; any other refusal — a
        // 403, a 429, a redirect not followed — is the server's answer.
        return if matches!(status.as_u16(), 400 | 404 | 405) {
            PostOutcome::Err(TransportError::HttpRefused(detail))
        } else {
            PostOutcome::Err(TransportError::Failed(detail))
        };
    }
    // A notification's 2xx is its acceptance, whatever the body says — and
    // the body is never read: a server that holds it open must not hold up
    // the handshake.
    let Some(want) = want_id else {
        return wrap(PostOutcome::Accepted);
    };
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
        let mut parser = SseParser::new();
        let mut reader = BufReader::new(response);
        loop {
            if abandoned.load(Ordering::Relaxed) {
                return PostOutcome::Err(TransportError::Cancelled);
            }
            let line = match read_line_capped(&mut reader, MAX_MESSAGE_BYTES) {
                Ok(Some(line)) => line,
                Ok(None) => return wrap(PostOutcome::StreamBroken),
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                    return PostOutcome::Err(TransportError::Failed(e.to_string()));
                }
                Err(_) => return wrap(PostOutcome::StreamBroken),
            };
            let Some(event) = parser.push_line(&line) else {
                continue;
            };
            match parse_incoming(&event.data) {
                Some(Incoming::Response { id, result }) if id == want => {
                    return wrap(PostOutcome::Response(result));
                }
                Some(Incoming::OrphanError(error)) => {
                    return wrap(PostOutcome::Response(Err(error)));
                }
                // A legacy server's own request mid-stream — a `ping`, most
                // likely — waits for its answer before the result comes.
                Some(Incoming::Request { id, method }) if endpoint.legacy => {
                    let mut endpoint = endpoint.clone();
                    if new_session.is_some() {
                        endpoint.session.clone_from(&new_session);
                    }
                    answer_server_request(&endpoint, &id, &method);
                }
                _ => endpoint.changes.deliver(&event.data),
            }
        }
    }
    let text = match read_body_capped(response, MAX_MESSAGE_BYTES) {
        Ok(text) => text,
        Err(e) => return PostOutcome::Err(TransportError::Failed(e)),
    };
    if text.trim().is_empty() {
        return wrap(PostOutcome::Accepted);
    }
    match parse_incoming(&text) {
        Some(Incoming::Response { id, result }) if id == want => {
            wrap(PostOutcome::Response(result))
        }
        Some(Incoming::OrphanError(error)) => wrap(PostOutcome::Response(Err(error))),
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
    /// Set when the transport is dropped: the stream thread stops at its
    /// next line (a keep-alive comment included) instead of holding the
    /// connection — and itself — for good.
    closed: Arc<AtomicBool>,
    /// Cleared when the stream thread ends, however it ends: the session's
    /// endpoint is gone with its stream.
    alive: Arc<AtomicBool>,
    /// Who hears the server's change notifications, fed by the stream
    /// thread — where a legacy SSE server sends them, unasked.
    changes: Changes,
    next_id: u64,
}

/// Clears its flag when dropped — the stream thread's every way out.
struct ClearOnDrop(Arc<AtomicBool>);

impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

impl Drop for SseTransport {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
    }
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
        let closed = Arc::new(AtomicBool::new(false));
        let stream_closed = Arc::clone(&closed);
        let alive = Arc::new(AtomicBool::new(true));
        let stream_alive = ClearOnDrop(Arc::clone(&alive));
        let stream_url = url.to_string();
        let stream_remote = remote.clone();
        let changes = Changes::default();
        let heard = changes.clone();
        std::thread::spawn(move || {
            let _alive = stream_alive;
            let client = match crate::llm::mcp_http_client(SSE_STREAM_TIMEOUT) {
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
            let challenge = challenge_of(&response);
            let status = response.status().as_u16();
            if status == 401 || (status == 403 && wants_more_scope(challenge.as_deref())) {
                let _ = endpoint_tx.send(Err(TransportError::NeedsAuth(challenge)));
                return;
            }
            if !response.status().is_success() {
                let _ = endpoint_tx.send(Err(TransportError::Failed(format!(
                    "HTTP {status} opening the event stream"
                ))));
                return;
            }
            let mut parser = SseParser::new();
            let mut answers: Option<Endpoint> = None;
            let mut reader = BufReader::new(response);
            while !stream_closed.load(Ordering::Relaxed) {
                let Ok(Some(line)) = read_line_capped(&mut reader, MAX_MESSAGE_BYTES) else {
                    break;
                };
                let Some(event) = parser.push_line(&line) else {
                    continue;
                };
                match (event.event.as_str(), &answers) {
                    ("endpoint", None) => {
                        let resolved = resolve_endpoint(&stream_url, &event.data);
                        if let Ok(endpoint) = &resolved {
                            answers = Some(Endpoint {
                                url: endpoint.clone(),
                                remote: stream_remote.clone(),
                                session: None,
                                protocol_version: None,
                                legacy: true,
                                changes: Changes::default(),
                            });
                        }
                        let _ = endpoint_tx.send(resolved);
                    }
                    (_, Some(endpoint)) => {
                        // The server's own requests — a `ping` — are owed an
                        // answer, posted where requests go.
                        if let Some(Incoming::Request { id, method }) = parse_incoming(&event.data)
                        {
                            answer_server_request(endpoint, &id, &method);
                            continue;
                        }
                        heard.deliver(&event.data);
                        if message_tx.send(event.data).is_err() {
                            return;
                        }
                    }
                    _ => {}
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
            closed,
            alive,
            changes,
            next_id: 1,
        })
    }

    /// Re-arm the bearer the POST side sends (a mid-session token refresh).
    /// The persistent GET stream keeps the credentials it opened with — auth
    /// only matters there at open.
    pub fn set_bearer(&mut self, bearer: Option<String>) {
        self.remote.bearer = bearer;
    }

    /// POST one message to the endpoint on a worker thread, waiting with the
    /// deadline and cancel polls every transport wait shares. `Some` when
    /// the server answered in the body rather than on the stream.
    fn post(
        &self,
        body: &Value,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<Option<Value>, TransportError> {
        let (endpoint, remote, body) =
            (self.endpoint.clone(), self.remote.clone(), body.to_string());
        let (tx, rx) = mpsc::channel();
        let started = Instant::now();
        std::thread::spawn(move || {
            let outcome = (|| {
                let client = crate::llm::mcp_http_client(HTTP_OP_TIMEOUT)
                    .map_err(|e| TransportError::Failed(e.to_string()))?;
                let mut req = client
                    .post(&endpoint)
                    .timeout(deadline.max(Duration::from_millis(50)))
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream");
                req = remote.apply(req);
                let response = req
                    .body(body)
                    .send()
                    .map_err(|e| TransportError::Failed(format!("request failed: {e}")))?;
                let challenge = challenge_of(&response);
                let status = response.status().as_u16();
                if status == 401 || (status == 403 && wants_more_scope(challenge.as_deref())) {
                    return Err(TransportError::NeedsAuth(challenge));
                }
                if !response.status().is_success() {
                    return Err(TransportError::Failed(format!(
                        "HTTP {status} posting to the server"
                    )));
                }
                // Most servers answer 202 and reply on the stream; a few
                // answer the JSON directly — accept both.
                let text = read_body_capped(response, MAX_MESSAGE_BYTES)
                    .map_err(TransportError::Failed)?;
                if text.trim().is_empty() {
                    return Ok(None);
                }
                Ok(serde_json::from_str(&text).ok())
            })();
            let _ = tx.send(outcome);
        });
        wait_channel(&rx, started, deadline, cancel)?
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<Value, TransportError> {
        let started = Instant::now();
        let id = self.next_id;
        self.next_id += 1;
        let direct = self.post(&request(id, method, params), deadline, cancel)?;
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
        loop {
            let data = wait_channel(&self.messages, started, deadline, cancel)?;
            match parse_incoming(&data) {
                Some(Incoming::Response { id: got, result }) if got == id => {
                    return result.map_err(TransportError::Rpc);
                }
                Some(Incoming::Response { id: got, result }) => {
                    self.parked.insert(got, result);
                }
                Some(Incoming::OrphanError(error)) => return Err(TransportError::Rpc(error)),
                Some(Incoming::Request { .. } | Incoming::Other) | None => {}
            }
        }
    }

    fn notify(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancelToken,
    ) -> Result<(), TransportError> {
        self.post(&notification(method, params), deadline, cancel)
            .map(|_| ())
    }
}

/// Resolve the `endpoint` event's URI against the stream URL (it is usually
/// relative — `/messages?sessionId=…`) — and refuse one on another origin:
/// every POST carries the bearer and the configured headers, which a server
/// naming someone else's endpoint (or downgrading `https` to `http`) would
/// hand to them. The reference TypeScript SDK refuses the same.
fn resolve_endpoint(base: &str, endpoint: &str) -> Result<String, TransportError> {
    let base = url::Url::parse(base)
        .map_err(|e| TransportError::Failed(format!("bad stream URL: {e}")))?;
    let resolved = base
        .join(endpoint.trim())
        .map_err(|e| TransportError::Failed(format!("bad endpoint URI: {e}")))?;
    if resolved.origin() != base.origin() {
        return Err(TransportError::Failed(format!(
            "the server named an endpoint on another origin ({}), which this client \
             won't send its requests to",
            resolved.origin().ascii_serialization()
        )));
    }
    Ok(resolved.to_string())
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

    /// The command a tier launches, configured the way a stdio server's is.
    fn configured(env: &BTreeMap<String, String>) -> Command {
        let mut cmd = crate::subprocess::command_for(
            &crate::subprocess::DetachTier::SetsidBinary,
            &exec_line("codex", &["mcp-server".to_string()]),
        );
        stdio_configure(env, None)(&mut cmd);
        cmd
    }

    #[test]
    fn a_stdio_server_never_inherits_the_herdr_pane() {
        // A server can be an agent of its own (`codex mcp-server`), whose
        // herdr hook would take the TUI's pane over (`docs/architecture/herdr.md`).
        let pane = std::ffi::OsStr::new(crate::herdr::PANE_ID_ENV);
        let command = configured(&BTreeMap::new());
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == pane && value.is_none()),
            "{:?}",
            command.get_envs().collect::<Vec<_>>()
        );
        // What the server's own config sets is still what it gets.
        let env = BTreeMap::from([(crate::herdr::PANE_ID_ENV.to_string(), "w1:p9".to_string())]);
        let command = configured(&env);
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == pane && value == Some(std::ffi::OsStr::new("w1:p9")))
        );
    }

    #[test]
    fn the_launch_line_execs_the_command_quoted_for_the_shell() {
        assert_eq!(
            exec_line("npx", &["-y".into(), "@scope/pkg@1.2".into()]),
            "exec npx -y @scope/pkg@1.2"
        );
        assert_eq!(
            exec_line("my server", &["it's".into(), String::new(), "$HOME".into()]),
            r"exec 'my server' 'it'\''s' '' '$HOME'"
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

    /// A `sh` server running `script`, its stdin piped to it — for the
    /// fixtures that read what the client wrote.
    fn stdio_script(script: &str) -> StdioTransport {
        StdioTransport::spawn(
            "sh",
            &["-c".to_string(), script.to_string()],
            &BTreeMap::new(),
            None,
            crate::subprocess::test_detach_helper(),
        )
        .expect("spawn sh")
    }

    /// A file in a fresh temp dir, for a fixture to write what it saw.
    fn fixture_file(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(name);
        (dir, path)
    }

    fn wait_for_file(path: &std::path::Path, needle: &str) -> String {
        for _ in 0..200 {
            if let Ok(text) = std::fs::read_to_string(path)
                && text.contains(needle)
            {
                return text;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// On stdio there is no stream to close: a request given up on — Esc or
    /// its deadline — "MUST" be cancelled with `notifications/cancelled`
    /// naming it, or the server keeps working on a call nobody awaits.
    #[test]
    fn a_request_given_up_on_is_cancelled_on_the_server() {
        let (_dir, seen) = fixture_file("seen");
        let mut t = stdio_script(&format!("cat > '{}'", seen.display()));
        let outcome = t.request(
            "tools/call",
            json!({"name": "slow"}),
            Duration::from_millis(200),
            &CancelToken::new(),
        );
        assert_eq!(outcome, Err(TransportError::Timeout));
        let text = wait_for_file(&seen, "notifications/cancelled");
        let cancelled = text
            .lines()
            .find(|line| line.contains("notifications/cancelled"))
            .unwrap_or_else(|| panic!("never cancelled: {text}"));
        let cancelled: Value = serde_json::from_str(cancelled).unwrap();
        assert_eq!(cancelled["params"]["requestId"], json!(1));
        // And a turn already cancelled sends nothing at all.
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(
            t.request("tools/call", json!({}), Duration::from_secs(1), &cancel),
            Err(TransportError::Cancelled)
        );
        std::thread::sleep(Duration::from_millis(100));
        let text = std::fs::read_to_string(&seen).unwrap_or_default();
        assert_eq!(text.matches("tools/call").count(), 1, "{text}");
    }

    /// A server that stops reading its stdin (busy in a synchronous tool)
    /// fills the pipe; a request bigger than the pipe used to block the
    /// turn's thread in `write_all` for good, Esc and deadline alike.
    #[test]
    fn a_server_that_stops_reading_cannot_wedge_a_call() {
        let mut t = stdio_script("sleep 30");
        let big = "x".repeat(512 * 1024);
        let started = Instant::now();
        let outcome = t.request(
            "tools/call",
            json!({"name": "t", "arguments": {"blob": big}}),
            Duration::from_millis(500),
            &CancelToken::new(),
        );
        assert_eq!(outcome, Err(TransportError::Timeout));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }

    /// One line of bytes that aren't UTF-8 (a log line in Latin-1) used to
    /// end the reader thread, and with it the session.
    #[test]
    fn invalid_utf8_on_stdout_does_not_end_the_session() {
        let mut t = stdio_script(
            r#"cat > /dev/null & printf '\377\376 not utf-8\n'; printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"ok":true}}'; sleep 5"#,
        );
        let answer = t
            .request(
                "x",
                Value::Null,
                Duration::from_secs(5),
                &CancelToken::new(),
            )
            .expect("still talking");
        assert_eq!(answer, json!({"ok": true}));
    }

    /// A legacy server may `ping` before it answers, waiting for the reply
    /// — JSON-RPC's "MUST reply". Unanswered, the request sat out its whole
    /// deadline.
    #[test]
    fn a_legacy_ping_on_stdio_is_answered() {
        let mut t = stdio_script(
            r#"read -r request
printf '%s\n' '{"jsonrpc":"2.0","id":"p1","method":"ping"}'
read -r pong
case "$pong" in
  *'"id":"p1"'*'"result":{}'*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"ponged":true}}' ;;
esac
sleep 5"#,
        );
        let answer = t
            .request(
                "tools/call",
                json!({}),
                Duration::from_secs(5),
                &CancelToken::new(),
            )
            .expect("answered after the pong");
        assert_eq!(answer, json!({"ponged": true}));
    }

    /// A stdio server is a process of its own session, with no controlling
    /// terminal — the bug class `docs/tools/tty-detach.md` fixed for shells: a
    /// server running `sudo` or an SSH prompt opened `/dev/tty` and read the
    /// keystrokes meant for the TUI.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_stdio_server_leads_its_own_session() {
        let mut t = stdio_script(
            r#"read -r request
sid=$(cut -d' ' -f6 /proc/$$/stat)
printf '{"jsonrpc":"2.0","id":1,"result":{"pid":%s,"sid":%s}}\n' "$$" "$sid"
sleep 5"#,
        );
        let answer = t
            .request(
                "x",
                Value::Null,
                Duration::from_secs(5),
                &CancelToken::new(),
            )
            .expect("answer");
        assert_eq!(answer["pid"], answer["sid"], "{answer}");
    }

    /// Shutdown is the spec's order — close stdin, let the server exit on
    /// its own, then SIGTERM, then SIGKILL — and takes the whole process
    /// group: `npx`/`uvx` wrappers run the real server as a grandchild.
    #[test]
    fn shutdown_closes_input_first_and_takes_the_whole_group() {
        let (_dir, seen) = fixture_file("events");
        // On EOF the server takes a moment to wind down, then exits on its
        // own — which only a stop that waits before it signals lets it do.
        let script = format!(
            r#"sleep 30 &
echo "child $!" > '{seen}'
while IFS= read -r line; do :; done
sleep 0.2
echo clean >> '{seen}'"#,
            seen = seen.display()
        );
        let mut t = stdio_script(&script);
        let text = wait_for_file(&seen, "child");
        let grandchild: i32 = text
            .split_whitespace()
            .nth(1)
            .and_then(|pid| pid.parse().ok())
            .expect("the background child's pid");
        t.shutdown(Duration::from_secs(2));
        let events = std::fs::read_to_string(&seen).unwrap();
        assert!(
            events.contains("clean"),
            "never exited on its own: {events}"
        );
        #[cfg(unix)]
        {
            let pid = rustix::process::Pid::from_raw(grandchild).unwrap();
            let mut gone = false;
            for _ in 0..100 {
                if rustix::process::test_kill_process(pid).is_err() {
                    gone = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(gone, "the server's background child outlived it");
        }
    }

    #[test]
    fn a_spawn_failure_names_the_command() {
        match StdioTransport::spawn(
            "definitely-not-a-real-binary-xyz",
            &[],
            &BTreeMap::new(),
            None,
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
        // Another origin — host, port or scheme — would carry the bearer
        // and the configured headers wherever the stream pointed them.
        for elsewhere in [
            "https://y.test/m",
            "http://x.test/m",
            "https://x.test:8443/m",
        ] {
            let refused = resolve_endpoint("https://x.test/sse", elsewhere).unwrap_err();
            assert!(
                refused.to_string().contains("origin"),
                "{elsewhere}: {refused}"
            );
        }
    }

    #[test]
    fn one_line_flattens_and_caps() {
        assert_eq!(one_line("a\n  b\t c", 10), "a b c");
        assert_eq!(one_line("abcdef", 3), "abc…");
    }

    // -----------------------------------------------------------------------
    // HTTP fixtures

    /// One request a fixture saw.
    struct Seen {
        method: String,
        headers: Vec<(String, String)>,
        body: Value,
    }

    impl Seen {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        }

        fn rpc_method(&self) -> &str {
            self.body["method"].as_str().unwrap_or_default()
        }
    }

    /// A one-request-per-connection HTTP/1.1 fixture: `respond` gets each
    /// request and writes the whole raw response itself (so it can stall,
    /// stream, or cut a stream short). Returns the `/mcp` URL.
    fn http_fixture(
        respond: impl Fn(&Seen, &mut std::net::TcpStream) + Send + Sync + 'static,
    ) -> String {
        use std::io::Read as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let respond = Arc::new(respond);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let respond = Arc::clone(&respond);
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let method = line
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    let mut headers = Vec::new();
                    let mut length = 0usize;
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                            break;
                        }
                        if let Some((key, value)) = header.split_once(':') {
                            if key.eq_ignore_ascii_case("content-length") {
                                length = value.trim().parse().unwrap_or(0);
                            }
                            headers.push((key.trim().to_string(), value.trim().to_string()));
                        }
                    }
                    let mut body = vec![0u8; length];
                    let _ = reader.read_exact(&mut body);
                    let seen = Seen {
                        method,
                        headers,
                        body: serde_json::from_slice(&body).unwrap_or_default(),
                    };
                    respond(&seen, &mut stream);
                });
            }
        });
        format!("http://127.0.0.1:{port}/mcp")
    }

    fn write_json(stream: &mut std::net::TcpStream, status: &str, extra: &str, body: &Value) {
        let body = body.to_string();
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        );
    }

    fn result_for(seen: &Seen, result: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": seen.body["id"], "result": result})
    }

    fn legacy_http(url: String) -> HttpTransport {
        let mut transport = HttpTransport::new(url, RemoteHeaders::default());
        transport.set_legacy();
        transport
    }

    /// The pooled client's own timeout is a stall guard, not the call's
    /// budget: a tool that takes longer than it — but not longer than the
    /// call was given — must still answer. (Under test that guard is 1 s.)
    #[test]
    fn a_slow_answer_gets_the_whole_budget_the_call_was_given() {
        let url = http_fixture(|seen, stream| {
            std::thread::sleep(Duration::from_millis(1500));
            write_json(
                stream,
                "200 OK",
                "",
                &result_for(seen, json!({"slow": true})),
            );
        });
        let mut t = legacy_http(url);
        let answer = t
            .request(
                "tools/call",
                json!({}),
                Duration::from_secs(6),
                &CancelToken::new(),
            )
            .expect("waited the call's own budget");
        assert_eq!(answer, json!({"slow": true}));
    }

    /// A notification's 2xx is acceptance whatever its body says — a
    /// server answering `{}` or `OK` used to fail the whole handshake.
    #[test]
    fn a_notification_is_accepted_whatever_the_body() {
        let url = http_fixture(|_, stream| {
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
            );
        });
        let mut t = legacy_http(url);
        t.notify(
            "notifications/initialized",
            Value::Null,
            Duration::from_secs(5),
            &CancelToken::new(),
        )
        .expect("accepted");
    }

    /// Only 400/404/405 without a modern error body is the spec's cue to try
    /// the legacy transport; a 403 is the server's answer, and a 403 asking
    /// for more scope is an authentication state.
    #[test]
    fn a_forbidden_answer_is_never_read_as_a_legacy_server() {
        let url = http_fixture(|seen, stream| {
            let extra = if seen.rpc_method() == "tools/call" {
                "WWW-Authenticate: Bearer error=\"insufficient_scope\", scope=\"files:write\"\r\n"
            } else {
                ""
            };
            let _ = stream.write_all(
                format!("HTTP/1.1 403 Forbidden\r\n{extra}Content-Length: 6\r\nConnection: close\r\n\r\ndenied")
                    .as_bytes(),
            );
        });
        let mut t = legacy_http(url);
        let cancel = CancelToken::new();
        match t.request("initialize", json!({}), Duration::from_secs(5), &cancel) {
            Err(TransportError::Failed(detail)) => assert!(detail.contains("403"), "{detail}"),
            other => panic!("unexpected: {other:?}"),
        }
        match t.request("tools/call", json!({}), Duration::from_secs(5), &cancel) {
            Err(TransportError::NeedsAuth(Some(challenge))) => {
                assert!(challenge.contains("insufficient_scope"), "{challenge}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// A legacy (2025-11-25) server may `ping` mid-call on the response
    /// stream and wait for the answer before sending its result — JSON-RPC's
    /// "MUST reply". Unanswered, the call sat out its whole deadline.
    #[test]
    fn a_legacy_ping_on_the_response_stream_is_answered() {
        let (pinged_tx, pinged_rx) = mpsc::channel::<()>();
        let pinged_rx = Arc::new(Mutex::new(pinged_rx));
        let url = http_fixture(move |seen, stream| {
            if seen.method == "POST" && seen.body.get("result").is_some() {
                // The client's answer to our ping: 202, and wake the call.
                assert_eq!(seen.body["id"], json!("ping-1"));
                assert_eq!(seen.header("mcp-session-id"), Some("s1"));
                let _ = pinged_tx.send(());
                let _ = stream.write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                return;
            }
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            let ping = json!({"jsonrpc": "2.0", "id": "ping-1", "method": "ping"});
            let _ = stream.write_all(format!("data: {ping}\n\n").as_bytes());
            let _ = stream.flush();
            let answered = pinged_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5));
            if answered.is_ok() {
                let done = result_for(seen, json!({"after": "ping"}));
                let _ = stream.write_all(format!("data: {done}\n\n").as_bytes());
            }
        });
        let mut t = legacy_http(url);
        t.session = Some("s1".to_string());
        let answer = t
            .request(
                "tools/call",
                json!({}),
                Duration::from_secs(8),
                &CancelToken::new(),
            )
            .expect("answered");
        assert_eq!(answer, json!({"after": "ping"}));
    }

    /// 2026-07-28 removed stream resumption: "a broken response stream
    /// loses the in-flight request; clients MUST re-issue it as a new
    /// request with a new request ID".
    #[test]
    fn a_broken_modern_stream_is_reissued_under_a_new_id() {
        let attempts = Arc::new(Mutex::new(Vec::<Value>::new()));
        let seen_ids = Arc::clone(&attempts);
        let url = http_fixture(move |seen, stream| {
            let mut ids = seen_ids.lock().unwrap();
            ids.push(seen.body["id"].clone());
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            if ids.len() > 1 {
                let done = result_for(seen, json!({"second": true}));
                let _ = stream.write_all(format!("data: {done}\n\n").as_bytes());
            }
            // The first stream just ends.
        });
        let mut t = HttpTransport::new(url, RemoteHeaders::default());
        t.set_modern(crate::mcp::PROTOCOL_VERSION);
        let answer = t
            .request(
                "tools/call",
                json!({"name": "x"}),
                Duration::from_secs(5),
                &CancelToken::new(),
            )
            .expect("re-issued");
        assert_eq!(answer, json!({"second": true}));
        let ids = attempts.lock().unwrap();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
    }

    /// A redirect to another origin is not followed: the configured headers
    /// (an API key, say) ride every MCP request, and reqwest only strips
    /// `Authorization` on a cross-host hop.
    #[test]
    fn a_redirect_to_another_origin_is_not_followed() {
        let elsewhere = http_fixture(|seen, _stream| {
            panic!("followed the redirect with {:?}", seen.header("x-api-key"));
        });
        let url = http_fixture(move |_, stream| {
            let _ = stream.write_all(
                format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {elsewhere}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            );
        });
        let mut t = HttpTransport::new(
            url,
            RemoteHeaders {
                headers: BTreeMap::from([("X-Api-Key".to_string(), "secret".to_string())]),
                bearer: None,
            },
        );
        t.set_legacy();
        match t.request(
            "tools/list",
            json!({}),
            Duration::from_secs(5),
            &CancelToken::new(),
        ) {
            Err(TransportError::Failed(detail)) => assert!(detail.contains("307"), "{detail}"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
