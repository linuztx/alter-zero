//! The MCP JSON-RPC protocol shapes (`docs/mcp.md`): request/notification
//! builders, the response parse, the `initialize` handshake, the `tools/list`
//! page, the `tools/call` result mapping — all pure over `serde_json::Value`,
//! so every transport speaks through the same tested vocabulary.

use serde_json::{Value, json};

/// The protocol revision this client proposes — the current spec at the time
/// of writing, and the one both references initialize with. A server may
/// answer with an older one; we accept whatever it says (the handshake's
/// contract) and echo it on the streamable-HTTP `MCP-Protocol-Version` header.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// How many characters of a server's tool description ride the tool spec —
/// Claude Code's `MAX_MCP_DESCRIPTION_LENGTH`: a wiki-length description
/// would spend the context budget the schemas share.
pub const MAX_TOOL_DESCRIPTION_CHARS: usize = 2048;

/// A JSON-RPC 2.0 **request** — `id` pairs the eventual response.
#[must_use]
pub fn request(id: u64, method: &str, params: Value) -> Value {
    let mut out = json!({"jsonrpc": "2.0", "id": id, "method": method});
    if !params.is_null() {
        out["params"] = params;
    }
    out
}

/// A JSON-RPC 2.0 **notification** — no `id`, no response.
#[must_use]
pub fn notification(method: &str, params: Value) -> Value {
    let mut out = json!({"jsonrpc": "2.0", "method": method});
    if !params.is_null() {
        out["params"] = params;
    }
    out
}

/// One message parsed off a transport: the response to a request we made, or
/// something else (a server notification/request — logged and dropped in v1;
/// the tools surface needs neither).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    /// A response carrying the `id` it answers and the outcome.
    Response {
        id: u64,
        result: Result<Value, RpcError>,
    },
    /// A notification or server→client request — not one of ours to answer.
    Other,
}

/// A JSON-RPC error object, flattened for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

/// Parse one incoming JSON-RPC message. `None` when the text isn't JSON at
/// all (transport noise — the caller decides how loud to be).
#[must_use]
pub fn parse_incoming(text: &str) -> Option<Incoming> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    let id = value.get("id").and_then(Value::as_u64);
    let Some(id) = id else {
        return Some(Incoming::Other);
    };
    if let Some(error) = value.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string();
        return Some(Incoming::Response {
            id,
            result: Err(RpcError { code, message }),
        });
    }
    // A message with an id and a method is a server→client REQUEST (an
    // elicitation, a ping) — not a response to us. v1 drops it; answering
    // would need capabilities we don't advertise.
    if value.get("method").is_some() {
        return Some(Incoming::Other);
    }
    Some(Incoming::Response {
        id,
        result: Ok(value.get("result").cloned().unwrap_or(Value::Null)),
    })
}

/// The `initialize` request params: what this client is and can do. We
/// advertise **no** client capabilities — the tools surface needs none.
#[must_use]
pub fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {
            "name": "alter-zero",
            "title": "Alter Zero",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// What the `initialize` response tells us about the server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerIdentity {
    /// The protocol revision the server settled on (echoed on later
    /// streamable-HTTP requests).
    pub protocol_version: String,
    pub name: String,
    pub version: String,
    /// The capability keys the server advertises (`tools`, `resources`,
    /// `prompts`, …) — the detail page's `Capabilities:` row.
    pub capabilities: Vec<String>,
    /// The server's optional usage instructions — shown in the detail page,
    /// never injected into the system prompt (v1).
    pub instructions: Option<String>,
}

/// Parse an `initialize` result.
#[must_use]
pub fn parse_initialize(result: &Value) -> ServerIdentity {
    let info = result.get("serverInfo");
    let field = |v: Option<&Value>, key: &str| {
        v.and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut capabilities: Vec<String> = result
        .get("capabilities")
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    capabilities.sort();
    ServerIdentity {
        protocol_version: field(Some(result), "protocolVersion"),
        name: field(info, "name"),
        version: field(info, "version"),
        capabilities,
        instructions: result
            .get("instructions")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    }
}

/// One tool a server offers, as `tools/list` described it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolInfo {
    /// The server's own (raw) tool name — what `tools/call` sends back.
    pub name: String,
    pub description: String,
    /// The tool's JSON-Schema input, passed through as the Chat Completions
    /// `parameters` — normalized to an object schema when absent/malformed
    /// (a validating provider rejects anything else).
    pub input_schema: Value,
}

/// Parse one `tools/list` page: the tools plus the `nextCursor` when the
/// listing continues.
#[must_use]
pub fn parse_tools_page(result: &Value) -> (Vec<McpToolInfo>, Option<String>) {
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item.get("name").and_then(Value::as_str)?.to_string();
                    let description: String = item
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .chars()
                        .take(MAX_TOOL_DESCRIPTION_CHARS)
                        .collect();
                    let input_schema = match item.get("inputSchema") {
                        Some(schema @ Value::Object(_)) => schema.clone(),
                        _ => json!({"type": "object", "properties": {}}),
                    };
                    Some(McpToolInfo {
                        name,
                        description,
                        input_schema,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let cursor = result
        .get("nextCursor")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    (tools, cursor)
}

/// The `tools/list` params for a page.
#[must_use]
pub fn tools_list_params(cursor: Option<&str>) -> Value {
    match cursor {
        Some(cursor) => json!({"cursor": cursor}),
        None => Value::Null,
    }
}

/// The `tools/call` params. `arguments` must be an object; anything else the
/// model sent is wrapped defensively so the request stays schema-valid.
#[must_use]
pub fn call_params(tool: &str, arguments: &Value) -> Value {
    let arguments = match arguments {
        Value::Object(_) => arguments.clone(),
        Value::Null => json!({}),
        other => json!({"value": other}),
    };
    json!({"name": tool, "arguments": arguments})
}

/// A `tools/call` result mapped for the cell and the model: the text, the
/// error flag, and a first image content item as a `data:` URL (riding
/// [`crate::llm::tools::ToolOutcome::image`], the `read` tool's channel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallOutcome {
    pub text: String,
    pub ok: bool,
    pub image: Option<String>,
}

/// Map a `tools/call` result. A lone `text` content item is its text
/// verbatim (the overwhelmingly common shape); anything richer renders as
/// pretty JSON so nothing is silently dropped; `structuredContent` wins when
/// the content list is empty. `isError` flips the cell red.
#[must_use]
pub fn parse_call_result(result: &Value) -> CallOutcome {
    let ok = !result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let content = result.get("content").and_then(Value::as_array);
    let mut image = None;
    let text = match content {
        Some(items) => {
            let mut texts: Vec<String> = Vec::new();
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            texts.push(text.to_string());
                        }
                    }
                    Some("image") if image.is_none() => {
                        if let (Some(data), Some(mime)) = (
                            item.get("data").and_then(Value::as_str),
                            item.get("mimeType").and_then(Value::as_str),
                        ) {
                            image = Some(format!("data:{mime};base64,{data}"));
                            texts.push(format!("[image: {mime}]"));
                        }
                    }
                    _ => {
                        // A resource/audio/unknown item: keep it as JSON so
                        // the model still sees what arrived.
                        texts.push(
                            serde_json::to_string_pretty(item).unwrap_or_else(|_| "{}".to_string()),
                        );
                    }
                }
            }
            texts.join("\n")
        }
        None => String::new(),
    };
    let text = if text.trim().is_empty() {
        match result.get("structuredContent") {
            Some(structured) => {
                serde_json::to_string_pretty(structured).unwrap_or_else(|_| "{}".to_string())
            }
            None if ok => "(no content)".to_string(),
            None => "tool call failed".to_string(),
        }
    } else {
        text
    };
    CallOutcome { text, ok, image }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_notifications_frame_correctly() {
        assert_eq!(
            request(1, "initialize", json!({"a": 1})),
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"a": 1}})
        );
        assert_eq!(
            request(2, "tools/list", Value::Null),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
        );
        assert_eq!(
            notification("notifications/initialized", Value::Null),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        );
    }

    #[test]
    fn incoming_splits_responses_errors_and_noise() {
        assert_eq!(
            parse_incoming(r#"{"jsonrpc":"2.0","id":3,"result":{"ok":true}}"#),
            Some(Incoming::Response {
                id: 3,
                result: Ok(json!({"ok": true})),
            })
        );
        assert_eq!(
            parse_incoming(r#"{"jsonrpc":"2.0","id":4,"error":{"code":-32601,"message":"nope"}}"#),
            Some(Incoming::Response {
                id: 4,
                result: Err(RpcError {
                    code: -32601,
                    message: "nope".to_string(),
                }),
            })
        );
        // A notification and a server→client request are Other.
        assert_eq!(
            parse_incoming(r#"{"jsonrpc":"2.0","method":"notifications/progress"}"#),
            Some(Incoming::Other)
        );
        assert_eq!(
            parse_incoming(r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#),
            Some(Incoming::Other)
        );
        assert_eq!(parse_incoming("not json"), None);
    }

    #[test]
    fn initialize_params_name_this_client() {
        let params = initialize_params();
        assert_eq!(params["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(params["clientInfo"]["name"], "alter-zero");
    }

    #[test]
    fn initialize_result_parses_identity_and_capabilities() {
        let identity = parse_initialize(&json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {"tools": {"listChanged": true}, "resources": {}},
            "serverInfo": {"name": "deepwiki", "version": "1.2.0"},
            "instructions": "  be nice  "
        }));
        assert_eq!(identity.protocol_version, "2025-03-26");
        assert_eq!(identity.name, "deepwiki");
        assert_eq!(identity.version, "1.2.0");
        assert_eq!(identity.capabilities, ["resources", "tools"]);
        assert_eq!(identity.instructions.as_deref(), Some("be nice"));
        assert_eq!(
            parse_initialize(&json!({})),
            ServerIdentity {
                protocol_version: String::new(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn tools_pages_parse_and_chain() {
        let (tools, cursor) = parse_tools_page(&json!({
            "tools": [
                {"name": "ask_question", "description": "Ask.",
                 "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}},
                                  "required": ["q"]}},
                {"name": "bare"}
            ],
            "nextCursor": "page2"
        }));
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "ask_question");
        assert_eq!(tools[0].input_schema["required"], json!(["q"]));
        // A missing schema normalizes to an empty object schema.
        assert_eq!(
            tools[1].input_schema,
            json!({"type": "object", "properties": {}})
        );
        assert_eq!(cursor.as_deref(), Some("page2"));
        assert_eq!(
            tools_list_params(cursor.as_deref()),
            json!({"cursor": "page2"})
        );
        assert_eq!(tools_list_params(None), Value::Null);
    }

    #[test]
    fn a_huge_description_is_capped() {
        let long = "x".repeat(MAX_TOOL_DESCRIPTION_CHARS + 100);
        let (tools, _) = parse_tools_page(&json!({"tools": [{"name": "t", "description": long}]}));
        assert_eq!(
            tools[0].description.chars().count(),
            MAX_TOOL_DESCRIPTION_CHARS
        );
    }

    #[test]
    fn call_params_keep_objects_and_wrap_everything_else() {
        assert_eq!(
            call_params("ask", &json!({"q": "hi"})),
            json!({"name": "ask", "arguments": {"q": "hi"}})
        );
        assert_eq!(
            call_params("ask", &Value::Null),
            json!({"name": "ask", "arguments": {}})
        );
        assert_eq!(
            call_params("ask", &json!([1])),
            json!({"name": "ask", "arguments": {"value": [1]}})
        );
    }

    #[test]
    fn call_results_map_text_errors_and_images() {
        let lone = parse_call_result(&json!({"content": [{"type": "text", "text": "hello"}]}));
        assert_eq!(lone.text, "hello");
        assert!(lone.ok);
        assert!(lone.image.is_none());

        let err = parse_call_result(&json!({
            "isError": true,
            "content": [{"type": "text", "text": "boom"}]
        }));
        assert_eq!(err.text, "boom");
        assert!(!err.ok);

        let multi = parse_call_result(&json!({"content": [
            {"type": "text", "text": "a"},
            {"type": "text", "text": "b"}
        ]}));
        assert_eq!(multi.text, "a\nb");

        let image = parse_call_result(&json!({"content": [
            {"type": "image", "data": "AAAA", "mimeType": "image/png"}
        ]}));
        assert_eq!(image.image.as_deref(), Some("data:image/png;base64,AAAA"));
        assert_eq!(image.text, "[image: image/png]");

        let structured = parse_call_result(&json!({
            "content": [],
            "structuredContent": {"result": "ok"}
        }));
        assert!(structured.text.contains("\"result\""));

        let empty = parse_call_result(&json!({}));
        assert_eq!(empty.text, "(no content)");
        let empty_err = parse_call_result(&json!({"isError": true}));
        assert_eq!(empty_err.text, "tool call failed");
    }
}
