//! The per-server connect sequence (`docs/mcp.md`): build the transport,
//! `initialize`, `notifications/initialized`, drain `tools/list` — with the
//! streamable-HTTP → legacy-SSE fallback for a bare-`url` config, and a 401
//! resolving as **NeedsAuth** for the OAuth flow.

use std::time::Duration;

use serde_json::Value;

use crate::mcp::{
    McpServerConfig, McpToolInfo, ServerIdentity, initialize_params, parse_initialize,
    parse_tools_page, tools_list_params,
};
use crate::stream::CancelToken;

use super::transport::{
    HttpTransport, RemoteHeaders, SseTransport, StdioTransport, Transport, TransportError,
};

/// The page/entry caps the `tools/list` drain enforces (codex's
/// `MAX_MCP_CATALOG_ITEMS` posture — a runaway cursor must terminate).
const MAX_TOOL_PAGES: usize = 100;
const MAX_TOOLS: usize = 2048;

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

/// A connected server: its live transport, identity, and tool list.
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

/// The protocol sequence over an open transport.
fn handshake(
    mut transport: Transport,
    deadline: Duration,
    cancel: &CancelToken,
) -> Result<Connection, ConnectError> {
    let result = transport
        .request("initialize", initialize_params(), deadline, cancel)
        .map_err(ConnectError::from_transport)?;
    let identity = parse_initialize(&result);
    if let Transport::Http(http) = &mut transport {
        http.set_protocol_version(&identity.protocol_version);
    }
    transport
        .notify("notifications/initialized", Value::Null)
        .map_err(ConnectError::from_transport)?;
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
    fn a_stdio_server_connects_end_to_end() {
        // A scripted `sh` MCP server: our request ids are deterministic
        // (1 = initialize, 2 = tools/list), so the fixture can pre-print its
        // responses without parsing stdin.
        let init = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"sh-fixture","version":"0"}}}"#;
        let tools = r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object","properties":{}}}]}}"#;
        let script =
            format!("cat > /dev/null & printf '%s\\n' '{init}'; printf '%s\\n' '{tools}'; sleep 5");
        let config = McpServerConfig::Stdio {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), script],
            env: Default::default(),
        };
        let cancel = CancelToken::new();
        let connection =
            connect(&config, None, None, Duration::from_secs(10), &cancel).expect("connect");
        assert_eq!(connection.identity.name, "sh-fixture");
        assert_eq!(connection.tools.len(), 1);
        assert_eq!(connection.tools[0].name, "echo");
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
