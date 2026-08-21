//! The per-server connect sequence (`docs/mcp.md`): build the transport,
//! detect the server's era — a modern (2026-07-28) `server/discover` probe
//! first, falling back to the legacy `initialize` handshake when the server
//! answers with anything but a modern error — then drain `tools/list`; with
//! the streamable-HTTP → legacy-SSE fallback for a bare-`url` config, and a
//! 401 resolving as **NeedsAuth** for the OAuth flow.
//!
//! **Nothing here is remembered between connects.** The era and the
//! revision are re-derived from the server every time, because they are
//! facts about the server *now*: a remembered `legacy` verdict skipped the
//! probe against a dual-era server, which answers the handshake happily —
//! so the revision it reported (and the `/mcp` page showed) stayed a
//! revision behind the server it was talking to, for good. And since the
//! fallback arm is "anything that is not a modern error", one probe that
//! merely timed out — a proxy blip, a server still booting — wrote that
//! verdict just as permanently.

use std::time::Duration;

use serde_json::Value;

use crate::mcp::{
    LEGACY_PROTOCOL_VERSION, McpServerConfig, McpToolInfo, PROTOCOL_VERSION, ServerIdentity,
    choose_version, discover_params, initialize_params_for, is_modern_error, parse_discover,
    parse_initialize, parse_tools_page, tools_list_params, unsupported_versions,
};
use crate::stream::CancelToken;

use super::transport::{
    HttpTransport, RemoteHeaders, SseTransport, StdioTransport, Transport, TransportError,
};

/// The page/entry caps the `tools/list` drain enforces (codex's
/// `MAX_MCP_CATALOG_ITEMS` posture — a runaway cursor must terminate).
const MAX_TOOL_PAGES: usize = 100;
const MAX_TOOLS: usize = 2048;

/// The modern probe's deadline slice. A modern server answers
/// `server/discover` at once and a legacy one errors it at once, so the
/// slice only matters for a server that answers *nothing* (which JSON-RPC
/// forbids but the spec's era recipe plans for) — the remaining budget still
/// has to cover the legacy handshake that follows.
const MODERN_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a connect failed, as the manager records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    /// The server wants OAuth. Carries the `WWW-Authenticate` challenge when
    /// one was sent (the discovery seed).
    NeedsAuth(Option<String>),
    /// Everything else, with the story the `✘ failed` detail row shows.
    Failed(String),
}

impl ConnectError {
    fn from_transport(e: TransportError) -> Self {
        match e {
            TransportError::NeedsAuth(challenge) => Self::NeedsAuth(challenge),
            other => Self::Failed(other.to_string()),
        }
    }
}

/// A connected server: its live transport, identity, and tool list. The
/// revision this connect settled on rides `identity.protocol_version` — it
/// is the server's own answer, never a remembered one.
pub struct Connection {
    pub transport: Transport,
    pub identity: ServerIdentity,
    pub tools: Vec<McpToolInfo>,
}

/// Connect one server end to end. `bearer` is the stored OAuth access token
/// when one exists; `deadline` bounds the whole sequence.
pub fn connect(
    config: &McpServerConfig,
    bearer: Option<String>,
    cwd: Option<&std::path::Path>,
    deadline: Duration,
    cancel: &CancelToken,
) -> Result<Connection, ConnectError> {
    match config {
        McpServerConfig::Stdio { command, args, env } => {
            let transport = StdioTransport::spawn(command, args, env, cwd)
                .map_err(ConnectError::from_transport)?;
            // Fold the child's stderr into a handshake failure so a crashing
            // server explains itself (`npx` printing a download error, say).
            let stderr = transport.stderr_tail();
            handshake(Transport::Stdio(transport), deadline, cancel).map_err(|e| match e {
                ConnectError::Failed(detail) => {
                    let tail = stderr.trim();
                    if tail.is_empty() || detail.contains(tail) {
                        ConnectError::Failed(detail)
                    } else {
                        ConnectError::Failed(format!("{detail} ({tail})"))
                    }
                }
                other => other,
            })
        }
        McpServerConfig::Http {
            url,
            headers,
            sse_fallback,
        } => {
            let remote = RemoteHeaders {
                headers: headers.clone(),
                bearer: bearer.clone(),
            };
            let http = HttpTransport::new(url.clone(), remote.clone());
            match handshake(Transport::Http(http), deadline, cancel) {
                Ok(connection) => Ok(connection),
                // The spec's compat recipe: a bare-`url` config whose
                // streamable POST is refused retries the same URL as a
                // legacy SSE server. A 401 is NOT a refusal — it is an
                // answer (authenticate first).
                Err(ConnectError::Failed(detail))
                    if *sse_fallback && detail.contains("server refused") =>
                {
                    connect_sse(url, remote, deadline, cancel)
                }
                Err(e) => Err(e),
            }
        }
        McpServerConfig::Sse { url, headers } => connect_sse(
            url,
            RemoteHeaders {
                headers: headers.clone(),
                bearer,
            },
            deadline,
            cancel,
        ),
    }
}

fn connect_sse(
    url: &str,
    remote: RemoteHeaders,
    deadline: Duration,
    cancel: &CancelToken,
) -> Result<Connection, ConnectError> {
    let transport = SseTransport::connect(url, remote, deadline, cancel)
        .map_err(ConnectError::from_transport)?;
    handshake(Transport::Sse(transport), deadline, cancel)
}

/// The protocol sequence over an open transport: era detection (the spec's
/// own recipe — probe modern, allowlist the modern error codes, anything
/// else is a legacy server), then the era's handshake, then the tools drain.
/// It runs in full at **every** connect: the era is a property of the server
/// as it is right now, and a server that gains (or drops) modern support
/// between launches has to be believed the next time we ask, not the last.
fn handshake(
    mut transport: Transport,
    deadline: Duration,
    cancel: &CancelToken,
) -> Result<Connection, ConnectError> {
    // The legacy HTTP+SSE transport predates the modern era by definition —
    // its servers speak 2024-11-05 — so only stdio and streamable HTTP probe.
    if matches!(transport, Transport::Sse(_)) {
        return legacy_handshake(transport, LEGACY_PROTOCOL_VERSION, deadline, cancel);
    }
    transport.set_modern(PROTOCOL_VERSION);
    let probe = transport.request(
        "server/discover",
        discover_params(PROTOCOL_VERSION),
        MODERN_PROBE_TIMEOUT.min(deadline),
        cancel,
    );
    let legacy_version = match probe {
        Ok(result) => {
            // A modern server. Its identity rides the discover result
            // (`serverInfo` in the `_meta`) — and so does the list of
            // revisions it actually speaks, which is the whole point of
            // asking: the settled revision is the newest **it names** that
            // we share, not the one we happened to propose. A list that
            // shares nothing modern with us is a server whose next request
            // would be refused, so it goes on through the handshake at the
            // revision it *did* name.
            let (identity, supported) = parse_discover(&result, PROTOCOL_VERSION);
            match choose_version(&supported) {
                Some(version) if version != PROTOCOL_VERSION => version,
                // Either it named our revision, or it named none we know —
                // and it answered a request sent under ours, which is the
                // spec's own definition of accepting it.
                _ => return finish(transport, identity, deadline, cancel),
            }
        }
        // Auth outranks era: the server wants credentials before it will
        // tell us anything — answer that first (era detection runs again on
        // the authenticated reconnect, like every other connect).
        Err(e @ (TransportError::NeedsAuth(_) | TransportError::Cancelled)) => {
            return Err(ConnectError::from_transport(e));
        }
        Err(TransportError::Rpc(error)) if is_modern_error(error.code) => {
            // A modern server that doesn't speak our modern revision names
            // what it does support (-32022): a shared legacy revision falls
            // back to the handshake proposing exactly that; nothing shared
            // is a genuine mismatch worth its words.
            let supported = unsupported_versions(&error).unwrap_or_default();
            match choose_version(&supported) {
                Some(version) if version != PROTOCOL_VERSION => version,
                _ => {
                    return Err(ConnectError::Failed(format!(
                        "the server rejected the modern handshake: {error} (it supports: {})",
                        if supported.is_empty() {
                            "unknown".to_string()
                        } else {
                            supported.join(", ")
                        }
                    )));
                }
            }
        }
        // Everything else — a legacy error code (commonly -32601), an HTTP
        // refusal, a probe nothing answered — is a legacy server. The spec
        // forbids keying this on one specific code.
        Err(_) => LEGACY_PROTOCOL_VERSION,
    };
    legacy_handshake(transport, legacy_version, deadline, cancel)
}

/// The legacy handshake: `initialize` (proposing `version`) →
/// `notifications/initialized`, the server's settled revision echoed on
/// later streamable-HTTP requests.
fn legacy_handshake(
    mut transport: Transport,
    version: &str,
    deadline: Duration,
    cancel: &CancelToken,
) -> Result<Connection, ConnectError> {
    transport.set_legacy();
    let result = transport
        .request(
            "initialize",
            initialize_params_for(version),
            deadline,
            cancel,
        )
        .map_err(ConnectError::from_transport)?;
    let mut identity = parse_initialize(&result);
    if let Transport::Http(http) = &mut transport {
        http.set_protocol_version(&identity.protocol_version);
    }
    transport
        .notify("notifications/initialized", Value::Null)
        .map_err(ConnectError::from_transport)?;
    // What the *server* answered is the settled revision — including when
    // it answered nothing at all (a pre-`protocolVersion` server), where the
    // proposal we sent is the only revision either side ever named. Filled
    // in after the header above deliberately: a server that named none gets
    // no `MCP-Protocol-Version` echoed back at it.
    if identity.protocol_version.trim().is_empty() {
        identity.protocol_version = version.to_string();
    }
    finish(transport, identity, deadline, cancel)
}

/// The `tools/list` drain over a settled transport — both eras end here.
fn finish(
    mut transport: Transport,
    identity: ServerIdentity,
    deadline: Duration,
    cancel: &CancelToken,
) -> Result<Connection, ConnectError> {
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_TOOL_PAGES {
        let page = transport
            .request(
                "tools/list",
                tools_list_params(cursor.as_deref()),
                deadline,
                cancel,
            )
            .map_err(ConnectError::from_transport)?;
        let (mut page_tools, next) = parse_tools_page(&page);
        tools.append(&mut page_tools);
        if tools.len() > MAX_TOOLS {
            tools.truncate(MAX_TOOLS);
            break;
        }
        // A server that repeats its cursor would loop forever.
        if next.is_none() || next == cursor {
            break;
        }
        cursor = next;
    }
    Ok(Connection {
        transport,
        identity,
        tools,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// A canned streamable-HTTP MCP server on a real socket: answers each
    /// POST by matching the request's method. Returns its URL.
    fn spawn_http_fixture() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let mut content_length = 0usize;
                // Request line + headers.
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
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
                let body = String::from_utf8_lossy(&body).to_string();
                let id = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| v.get("id").and_then(Value::as_u64));
                let method = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| v.get("method").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default();
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
                match method.as_str() {
                    "initialize" => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "protocolVersion": "2025-06-18",
                                "capabilities": {"tools": {}},
                                "serverInfo": {"name": "fixture", "version": "0.1"}
                            }
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", "Mcp-Session-Id: sess-1\r\n", &body);
                    }
                    "notifications/initialized" => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                    "tools/list" => {
                        // The response rides an SSE body — the streamable
                        // transport must drain it to the matching id.
                        let message = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"tools": [
                                {"name": "ask_question", "description": "Ask.",
                                 "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}}}
                            ]}
                        });
                        let body = format!("event: message\ndata: {message}\n\n");
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    "tools/call" => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"content": [{"type": "text", "text": "hello from fixture"}]}
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", "", &body);
                    }
                    _ => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": {"code": -32601, "message": "unknown"}
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", "", &body);
                    }
                }
            }
        });
        format!("http://127.0.0.1:{port}/mcp")
    }

    #[test]
    fn streamable_http_connects_and_lists_tools() {
        let url = spawn_http_fixture();
        let config = McpServerConfig::Http {
            url,
            headers: Default::default(),
            sse_fallback: false,
        };
        let cancel = CancelToken::new();
        let mut connection =
            connect(&config, None, None, Duration::from_secs(10), &cancel).expect("connect");
        assert_eq!(connection.identity.name, "fixture");
        assert_eq!(connection.identity.capabilities, ["tools"]);
        // The modern probe met -32601, so the legacy handshake ran and the
        // server's settled revision is the one on record.
        assert_eq!(connection.identity.protocol_version, "2025-06-18");
        assert_eq!(connection.tools.len(), 1);
        assert_eq!(connection.tools[0].name, "ask_question");
        // A call round-trips too.
        let result = connection
            .transport
            .request(
                "tools/call",
                crate::mcp::call_params("ask_question", &serde_json::json!({"q": "hi"})),
                Duration::from_secs(10),
                &cancel,
            )
            .expect("call");
        let outcome = crate::mcp::parse_call_result(&result);
        assert_eq!(outcome.text, "hello from fixture");
        assert!(outcome.ok);
    }

    /// A strict MODERN streamable-HTTP fixture: every request must carry the
    /// `Mcp-Method` header and the `_meta` version block (400 + a modern
    /// error otherwise), `tools/call` must carry `Mcp-Name` — so a green
    /// connect + call *proves* the client sends the 2026-07-28 request
    /// metadata.
    fn spawn_modern_http_fixture() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
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
                let mut method_header = String::new();
                let mut name_header = String::new();
                let mut version_header = String::new();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    let lower = header.to_ascii_lowercase();
                    if let Some(rest) = lower.strip_prefix("content-length:") {
                        content_length = rest.trim().parse().unwrap_or(0);
                    }
                    if let Some(rest) = lower.strip_prefix("mcp-method:") {
                        method_header = rest.trim().to_string();
                    }
                    if let Some(rest) = lower.strip_prefix("mcp-name:") {
                        name_header = rest.trim().to_string();
                    }
                    if let Some(rest) = lower.strip_prefix("mcp-protocol-version:") {
                        version_header = rest.trim().to_string();
                    }
                }
                let mut body = vec![0u8; content_length];
                let _ = reader.read_exact(&mut body);
                let body = String::from_utf8_lossy(&body).to_string();
                let parsed = serde_json::from_str::<Value>(&body).unwrap_or_default();
                let id = parsed.get("id").and_then(Value::as_u64);
                let method = parsed
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let meta_version = parsed
                    .get("params")
                    .and_then(|p| p.get("_meta"))
                    .and_then(|m| m.get("io.modelcontextprotocol/protocolVersion"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let respond = |stream: &mut std::net::TcpStream, status: &str, body: &str| {
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                };
                // The strictness: modern metadata or a modern refusal.
                if method_header != method
                    || meta_version != "2026-07-28"
                    || version_header != meta_version
                    || (method == "tools/call" && name_header.is_empty())
                {
                    let body = serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32020, "message": "header mismatch"}
                    })
                    .to_string();
                    respond(&mut stream, "400 Bad Request", &body);
                    continue;
                }
                match method.as_str() {
                    "server/discover" => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "resultType": "complete",
                                "supportedVersions": ["2026-07-28"],
                                "capabilities": {"tools": {}},
                                "ttlMs": 60000, "cacheScope": "public",
                                "_meta": {"io.modelcontextprotocol/serverInfo":
                                    {"name": "modern-http", "version": "1"}}
                            }
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", &body);
                    }
                    "tools/list" => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"resultType": "complete", "tools": [
                                {"name": "greet", "description": "Greet.",
                                 "inputSchema": {"type": "object", "properties": {}}}
                            ], "ttlMs": 60000, "cacheScope": "public"}
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", &body);
                    }
                    "tools/call" => {
                        // Echo the Mcp-Name header back so the test can pin
                        // its derivation.
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"resultType": "complete", "content": [
                                {"type": "text", "text": format!("named {name_header}")}
                            ]}
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", &body);
                    }
                    _ => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": {"code": -32601, "message": "unknown"}
                        })
                        .to_string();
                        respond(&mut stream, "404 Not Found", &body);
                    }
                }
            }
        });
        format!("http://127.0.0.1:{port}/mcp")
    }

    #[test]
    fn a_modern_http_server_connects_statelessly_with_request_metadata() {
        let url = spawn_modern_http_fixture();
        let config = McpServerConfig::Http {
            url,
            headers: Default::default(),
            sse_fallback: false,
        };
        let cancel = CancelToken::new();
        let mut connection =
            connect(&config, None, None, Duration::from_secs(10), &cancel).expect("connect");
        assert_eq!(connection.identity.name, "modern-http");
        assert_eq!(connection.identity.protocol_version, "2026-07-28");
        assert_eq!(connection.tools.len(), 1);
        // A call carries the Mcp-Name header (the fixture echoes it back).
        let result = connection
            .transport
            .request(
                "tools/call",
                crate::mcp::call_params("greet", &serde_json::json!({})),
                Duration::from_secs(10),
                &cancel,
            )
            .expect("call");
        assert_eq!(crate::mcp::parse_call_result(&result).text, "named greet");
    }

    /// A modern-only server that speaks an *older* revision than ours names
    /// it in `-32022`'s data; a server whose list overlaps ours on a legacy
    /// revision gets the `initialize` handshake proposing exactly that.
    #[test]
    fn an_unsupported_version_error_negotiates_the_fallback() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
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
                let proposed = parsed
                    .get("params")
                    .and_then(|p| p.get("protocolVersion"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let respond = |stream: &mut std::net::TcpStream, status: &str, body: &str| {
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                };
                match method.as_str() {
                    "server/discover" => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": {"code": -32022, "message": "Unsupported protocol version",
                                      "data": {"supported": ["2025-06-18"],
                                               "requested": "2026-07-28"}}
                        })
                        .to_string();
                        respond(&mut stream, "400 Bad Request", &body);
                    }
                    "initialize" => {
                        // Echo the client's proposal back through
                        // serverInfo.version so the test can pin it.
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "protocolVersion": "2025-06-18",
                                "capabilities": {"tools": {}},
                                "serverInfo": {"name": "negotiated", "version": proposed}
                            }
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", &body);
                    }
                    "notifications/initialized" => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                    _ => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"tools": []}
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", &body);
                    }
                }
            }
        });
        let config = McpServerConfig::Http {
            url: format!("http://127.0.0.1:{port}/mcp"),
            headers: Default::default(),
            sse_fallback: false,
        };
        let cancel = CancelToken::new();
        let connection =
            connect(&config, None, None, Duration::from_secs(10), &cancel).expect("connect");
        assert_eq!(connection.identity.name, "negotiated");
        // The fallback proposed the exact revision the server named.
        assert_eq!(connection.identity.version, "2025-06-18");
        assert_eq!(connection.identity.protocol_version, "2025-06-18");
    }

    /// A legacy server whose session dies under us: it mints `s1`, serves
    /// the handshake and the listing, then 404s any request carrying `s1` —
    /// a server restart or an idle timeout, from the client's side. The
    /// spec's answer is a fresh `initialize`, and the request replays.
    #[test]
    fn an_expired_session_re_initializes_and_replays_the_request() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut minted = 0u32;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut content_length = 0usize;
                let mut session = String::new();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    let lower = header.to_ascii_lowercase();
                    if let Some(rest) = lower.strip_prefix("content-length:") {
                        content_length = rest.trim().parse().unwrap_or(0);
                    }
                    if let Some(rest) = lower.strip_prefix("mcp-session-id:") {
                        session = rest.trim().to_string();
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
                // Only `s1` is dead; the session minted by the re-handshake
                // works, so a replay must succeed.
                if session == "s1" {
                    respond(&mut stream, "404 Not Found", "", "session not found");
                    continue;
                }
                match method.as_str() {
                    "server/discover" => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": {"code": -32601, "message": "method not found"}
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", "", &body);
                    }
                    "initialize" => {
                        minted += 1;
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "protocolVersion": "2025-11-25",
                                "capabilities": {"tools": {}},
                                "serverInfo": {"name": "sessioned", "version": "1"}
                            }
                        })
                        .to_string();
                        respond(
                            &mut stream,
                            "200 OK",
                            &format!("Mcp-Session-Id: s{minted}\r\n"),
                            &body,
                        );
                    }
                    "notifications/initialized" => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                    "tools/list" => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"tools": [{"name": "ping",
                                "inputSchema": {"type": "object", "properties": {}}}]}
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", "", &body);
                    }
                    _ => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"content": [{"type": "text", "text": "survived"}]}
                        })
                        .to_string();
                        respond(&mut stream, "200 OK", "", &body);
                    }
                }
            }
        });
        let config = McpServerConfig::Http {
            url: format!("http://127.0.0.1:{port}/mcp"),
            headers: Default::default(),
            sse_fallback: false,
        };
        let cancel = CancelToken::new();
        let mut connection =
            connect(&config, None, None, Duration::from_secs(10), &cancel).expect("connect");
        assert_eq!(connection.identity.name, "sessioned");
        // The call goes out under the now-dead `s1`; without the recovery it
        // fails outright instead of re-handshaking and replaying.
        let result = connection
            .transport
            .request(
                "tools/call",
                crate::mcp::call_params("ping", &serde_json::json!({})),
                Duration::from_secs(10),
                &cancel,
            )
            .expect("the request survives its session expiring");
        assert_eq!(crate::mcp::parse_call_result(&result).text, "survived");
    }

    #[test]
    fn every_connect_re_probes_the_servers_era() {
        // Era detection costs a round trip, and the spec allows remembering
        // the verdict across restarts — but a remembered `legacy` verdict is
        // exactly what left a dual-era server reported a revision behind
        // itself, for good: it answers the handshake happily, so the wrong
        // guess never fails and never corrects itself. The verdict is a fact
        // about the server *now*, so we pay the probe every time.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let probes = std::sync::Arc::new(AtomicUsize::new(0));
        let probes_t = std::sync::Arc::clone(&probes);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
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
                        probes_t.fetch_add(1, Ordering::SeqCst);
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": {"code": -32601, "message": "method not found"}
                        })
                        .to_string();
                        respond(&mut stream, &body);
                    }
                    "initialize" => {
                        let body = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "protocolVersion": "2025-11-25",
                                "capabilities": {"tools": {}},
                                "serverInfo": {"name": "cached", "version": "1"}
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
                            "jsonrpc": "2.0", "id": id, "result": {"tools": []}
                        })
                        .to_string();
                        respond(&mut stream, &body);
                    }
                }
            }
        });
        let config = McpServerConfig::Http {
            url: format!("http://127.0.0.1:{port}/mcp"),
            headers: Default::default(),
            sse_fallback: false,
        };
        let cancel = CancelToken::new();
        let first =
            connect(&config, None, None, Duration::from_secs(10), &cancel).expect("first connect");
        assert_eq!(probes.load(Ordering::SeqCst), 1);
        assert_eq!(first.identity.protocol_version, "2025-11-25");
        drop(first);
        // The next connect asks again — nothing carried over.
        let second =
            connect(&config, None, None, Duration::from_secs(10), &cancel).expect("second connect");
        assert_eq!(
            probes.load(Ordering::SeqCst),
            2,
            "the era must be re-detected at every connect"
        );
        assert_eq!(second.identity.name, "cached");
        assert_eq!(second.identity.protocol_version, "2025-11-25");
    }

    /// A 401 server resolves as NeedsAuth with the challenge captured.
    #[test]
    fn a_401_resolves_as_needs_auth() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok() && line.trim() != "" {
                    line.clear();
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer resource_metadata=\"https://x/.well-known/oauth-protected-resource\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let config = McpServerConfig::Http {
            url: format!("http://127.0.0.1:{port}/mcp"),
            headers: Default::default(),
            sse_fallback: false,
        };
        let cancel = CancelToken::new();
        match connect(&config, None, None, Duration::from_secs(10), &cancel) {
            Err(ConnectError::NeedsAuth(challenge)) => {
                assert!(challenge.unwrap().contains("resource_metadata"));
            }
            Err(other) => panic!("unexpected error: {other:?}"),
            Ok(_) => panic!("unexpectedly connected"),
        }
    }

    #[test]
    fn a_legacy_stdio_server_connects_through_the_fallback() {
        // A scripted `sh` LEGACY MCP server: our request ids are
        // deterministic (1 = the modern server/discover probe — answered
        // with the classic -32601, 2 = initialize, 3 = tools/list), so the
        // fixture can pre-print its responses without parsing stdin.
        let probe =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#;
        let init = r#"{"jsonrpc":"2.0","id":2,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"sh-fixture","version":"0"}}}"#;
        let tools = r#"{"jsonrpc":"2.0","id":3,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object","properties":{}}}]}}"#;
        let script = format!(
            "cat > /dev/null & printf '%s\\n' '{probe}'; printf '%s\\n' '{init}'; printf '%s\\n' '{tools}'; sleep 5"
        );
        let config = McpServerConfig::Stdio {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), script],
            env: Default::default(),
        };
        let cancel = CancelToken::new();
        let connection =
            connect(&config, None, None, Duration::from_secs(10), &cancel).expect("connect");
        assert_eq!(connection.identity.name, "sh-fixture");
        assert_eq!(connection.identity.protocol_version, "2025-06-18");
        assert_eq!(connection.tools.len(), 1);
        assert_eq!(connection.tools[0].name, "echo");
    }

    #[test]
    fn a_modern_stdio_server_connects_through_discover() {
        // A MODERN (2026-07-28) fixture: id 1 answers `server/discover`
        // itself, id 2 the tools listing — no initialize anywhere.
        let discover = r#"{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"modern-fixture","version":"2"}},"instructions":"be modern"}}"#;
        let tools = r#"{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo","inputSchema":{"type":"object","properties":{}}}],"ttlMs":60000,"cacheScope":"public"}}"#;
        let script = format!(
            "cat > /dev/null & printf '%s\\n' '{discover}'; printf '%s\\n' '{tools}'; sleep 5"
        );
        let config = McpServerConfig::Stdio {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), script],
            env: Default::default(),
        };
        let cancel = CancelToken::new();
        let connection =
            connect(&config, None, None, Duration::from_secs(10), &cancel).expect("connect");
        assert_eq!(connection.identity.name, "modern-fixture");
        assert_eq!(connection.identity.protocol_version, "2026-07-28");
        assert_eq!(connection.identity.capabilities, ["tools"]);
        assert_eq!(
            connection.identity.instructions.as_deref(),
            Some("be modern")
        );
        assert_eq!(connection.tools.len(), 1);
    }

    /// `server/discover` exists to let a client **pick a version up front**
    /// from the list the server returns. A server that answers the probe but
    /// names no revision we can speak modern has told us what it speaks —
    /// the settled revision is its answer, negotiated through the handshake,
    /// not the version we happened to propose.
    #[test]
    fn the_settled_revision_is_the_one_the_server_names() {
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"server/discover"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"resultType":"complete","supportedVersions":["2025-11-25","2025-06-18"],"capabilities":{"tools":{}},"_meta":{"io.modelcontextprotocol/serverInfo":{"name":"picky","version":"9"}}}}\n' "$id" ;;
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"picky","version":"9"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id" ;;
  esac
done
"#;
        let config = McpServerConfig::Stdio {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            env: Default::default(),
        };
        let cancel = CancelToken::new();
        let connection =
            connect(&config, None, None, Duration::from_secs(10), &cancel).expect("connect");
        assert_eq!(connection.identity.protocol_version, "2025-11-25");
        assert_eq!(connection.identity.name, "picky");
        assert_eq!(connection.tools.len(), 1);
    }

    /// A legacy server that names no revision at all (the field is required
    /// from 2024-11-05 on, so this is a broken one) leaves the proposal as
    /// the only revision either side ever named — which is what the session
    /// is speaking, and so what the `Protocol:` row says.
    #[test]
    fn a_server_that_names_no_revision_settles_on_the_proposal() {
        let probe =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#;
        let init = r#"{"jsonrpc":"2.0","id":2,"result":{"capabilities":{"tools":{}},"serverInfo":{"name":"silent","version":"0"}}}"#;
        let tools = r#"{"jsonrpc":"2.0","id":3,"result":{"tools":[]}}"#;
        let script = format!(
            "cat > /dev/null & printf '%s\\n' '{probe}'; printf '%s\\n' '{init}'; printf '%s\\n' '{tools}'; sleep 5"
        );
        let config = McpServerConfig::Stdio {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), script],
            env: Default::default(),
        };
        let cancel = CancelToken::new();
        let connection =
            connect(&config, None, None, Duration::from_secs(10), &cancel).expect("connect");
        assert_eq!(connection.identity.name, "silent");
        assert_eq!(
            connection.identity.protocol_version,
            LEGACY_PROTOCOL_VERSION
        );
    }

    #[test]
    fn a_dead_stdio_command_fails_with_its_stderr() {
        let config = McpServerConfig::Stdio {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "echo install failed >&2; exit 3".to_string(),
            ],
            env: Default::default(),
        };
        let cancel = CancelToken::new();
        match connect(&config, None, None, Duration::from_secs(5), &cancel) {
            Err(ConnectError::Failed(detail)) => {
                assert!(detail.contains("install failed"), "{detail}");
            }
            Err(other) => panic!("unexpected error: {other:?}"),
            Ok(_) => panic!("unexpectedly connected"),
        }
    }
}
