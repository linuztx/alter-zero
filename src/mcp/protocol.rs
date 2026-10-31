//! The MCP JSON-RPC protocol shapes (`docs/mcp.md`): request/notification
//! builders, the response parse, the `initialize` handshake, the `tools/list`
//! page, the `tools/call` result mapping — all pure over `serde_json::Value`,
//! so every transport speaks through the same tested vocabulary.

use serde_json::{Value, json};

/// The modern protocol revision this client proposes — 2026-07-28, the
/// current spec: **stateless**, no `initialize` handshake, the version and
/// the client's capabilities riding every request's `_meta`, and
/// `server/discover` as the identity/negotiation surface. Era detection is
/// the spec's own recipe and runs at **every** connect: probe modern first,
/// fall back to the legacy handshake when the server answers with anything
/// but a modern error. This is what the probe *proposes*; what a connect
/// settles on is whatever the server names back (`docs/mcp.md`).
pub const PROTOCOL_VERSION: &str = "2026-07-28";

/// The newest handshake-based ("legacy") revision — what the `initialize`
/// fallback proposes. The server answers with the newest revision *it*
/// supports and we accept whatever it says (the handshake's contract),
/// echoing it on the streamable-HTTP `MCP-Protocol-Version` header.
pub const LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";

/// Every revision this client speaks, newest first — the modern revision,
/// then the handshake ones (whose differences don't touch our tools-only
/// surface). What [`choose_version`] picks from when a server names its own
/// list. Revision dates compare lexicographically.
pub const SUPPORTED_VERSIONS: [&str; 5] = [
    "2026-07-28",
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];

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

/// A JSON-RPC error object, flattened for display. `data` is kept whole —
/// the 2026-07-28 errors carry their payload there (`-32022`'s `supported`
/// list is how a server names the versions it *does* speak).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
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
            result: Err(RpcError {
                code,
                message,
                data: error.get("data").cloned(),
            }),
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

/// This client's identity — the legacy `clientInfo` and the modern
/// `_meta`-carried one are the same object.
fn client_info() -> Value {
    json!({
        "name": "alter-zero",
        "title": "Alter Zero",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// The `_meta` object every modern (2026-07-28) request carries: the
/// protocol version, this client's identity, and its capabilities — an
/// empty object, the schema's own spelling for "no optional capabilities"
/// (servers MUST NOT infer capabilities from prior requests, so it is
/// required on every request, never elided).
#[must_use]
pub fn request_meta(version: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientInfo": client_info(),
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

/// Fold the modern `_meta` into a request's params. `Null` params become the
/// bare `{"_meta": …}` object — modern params are required, because `_meta`
/// is.
#[must_use]
pub fn with_meta(params: Value, version: &str) -> Value {
    let mut params = match params {
        params @ Value::Object(_) => params,
        _ => json!({}),
    };
    params["_meta"] = request_meta(version);
    params
}

/// The `server/discover` params — nothing but the `_meta`.
#[must_use]
pub fn discover_params(version: &str) -> Value {
    with_meta(Value::Null, version)
}

/// Parse a `server/discover` result into the server's identity and its
/// advertised `supportedVersions`. The identity's `serverInfo` rides
/// `result._meta` in this revision — never a top-level field — and the
/// settled `protocol_version` is stamped from `negotiated`, the version the
/// probe succeeded under.
#[must_use]
pub fn parse_discover(result: &Value, negotiated: &str) -> (ServerIdentity, Vec<String>) {
    let info = result
        .get("_meta")
        .and_then(|meta| meta.get("io.modelcontextprotocol/serverInfo"));
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
    let supported = result
        .get("supportedVersions")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    (
        ServerIdentity {
            protocol_version: negotiated.to_string(),
            name: field(info, "name"),
            version: field(info, "version"),
            capabilities,
            instructions: result
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        },
        supported,
    )
}

/// Is this one of the 2026-07-28 error codes (`-32020` header mismatch,
/// `-32021` missing client capability, `-32022` unsupported protocol
/// version)? Era detection keys on this **allowlist**: any other error from
/// a first modern request means a legacy server — they answer unknown
/// pre-`initialize` requests with implementation-defined errors (commonly
/// `-32601`), so the fallback must never hang off one specific code.
#[must_use]
pub const fn is_modern_error(code: i64) -> bool {
    // -32020 HeaderMismatch, -32021 MissingRequiredClientCapability,
    // -32022 UnsupportedProtocolVersion — a contiguous allocation.
    matches!(code, -32022..=-32020)
}

/// The `supported` version list out of an `UnsupportedProtocolVersion`
/// (`-32022`) error's data — how a server names what it *does* speak.
#[must_use]
pub fn unsupported_versions(error: &RpcError) -> Option<Vec<String>> {
    if error.code != -32022 {
        return None;
    }
    error
        .data
        .as_ref()?
        .get("supported")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
}

/// The newest revision both sides speak, from a server's advertised list —
/// `None` when nothing overlaps. [`SUPPORTED_VERSIONS`] is newest-first, so
/// the first hit wins.
#[must_use]
pub fn choose_version(supported: &[String]) -> Option<&'static str> {
    SUPPORTED_VERSIONS
        .iter()
        .find(|version| supported.iter().any(|s| s == *version))
        .copied()
}

/// A value for the streamable-HTTP `Mcp-Method`/`Mcp-Name` request-metadata
/// headers: verbatim when it is printable ASCII with no leading/trailing
/// whitespace and doesn't itself look like the sentinel; else the spec's
/// `=?base64?{value}?=` sentinel form. Tool names are only SHOULD-constrained
/// to header-safe characters, so the encoding path is real, not theoretical.
#[must_use]
pub fn header_value(text: &str) -> String {
    let ascii_printable = text.bytes().all(|b| (0x20..=0x7e).contains(&b));
    let whitespace_framed = text.starts_with(' ') || text.ends_with(' ');
    let looks_like_sentinel = text.starts_with("=?") && text.ends_with("?=");
    if !text.is_empty() && ascii_printable && !whitespace_framed && !looks_like_sentinel {
        return text.to_string();
    }
    format!("=?base64?{}?=", b64_standard(text.as_bytes()))
}

/// Standard base64 (RFC 4648 §4, with padding) — the sentinel encoding's
/// alphabet.
fn b64_standard(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The `initialize` request params — the legacy handshake, proposing
/// `version`. What this client is and can do; we advertise **no** client
/// capabilities, the tools surface needing none.
#[must_use]
pub fn initialize_params_for(version: &str) -> Value {
    json!({
        "protocolVersion": version,
        "capabilities": {},
        "clientInfo": client_info(),
    })
}

/// [`initialize_params_for`] at the newest legacy revision — the ordinary
/// fallback proposal.
#[must_use]
pub fn initialize_params() -> Value {
    initialize_params_for(LEGACY_PROTOCOL_VERSION)
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
    /// The schema's `x-mcp-header` annotations ([`header_params`]) — the
    /// arguments a modern streamable-HTTP call mirrors into
    /// `Mcp-Param-{Name}` headers.
    pub header_params: Vec<HeaderParam>,
}

/// One `x-mcp-header` annotation (2026-07-28, `docs/mcp.md`): the header's
/// name portion (`Mcp-Param-{name}`) and the chain of `properties` keys
/// leading to the argument it mirrors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderParam {
    pub name: String,
    pub path: Vec<String>,
}

/// The `x-mcp-header` annotations a tool's input schema carries, held to the
/// spec's constraints — a valid header-name token, unique without regard to
/// case, on an `integer`/`string`/`boolean` property reached from the root
/// through `properties` keys alone (never `items`, a composition, a
/// conditional or a `$ref`). `Err` names the violation: the spec has a
/// streamable-HTTP client **exclude** such a tool rather than guess at its
/// headers, and a server checking them would refuse every call anyway.
pub fn header_params(schema: &Value) -> Result<Vec<HeaderParam>, String> {
    fn scan(
        node: &Value,
        reachable: Option<&[String]>,
        found: &mut Vec<HeaderParam>,
    ) -> Result<(), String> {
        match node {
            Value::Object(object) => {
                if let Some(annotation) = object.get(HEADER_ANNOTATION) {
                    let path = reachable.filter(|path| !path.is_empty()).ok_or_else(|| {
                        "an x-mcp-header outside the schema's properties".to_string()
                    })?;
                    let name = annotation
                        .as_str()
                        .filter(|name| !name.is_empty() && name.bytes().all(is_tchar))
                        .ok_or_else(|| {
                            format!("x-mcp-header {annotation} is not a header-name token")
                        })?;
                    let kind = object.get("type").and_then(Value::as_str);
                    if !matches!(kind, Some("string" | "integer" | "boolean")) {
                        return Err(format!(
                            "x-mcp-header {name} is on a {} property, not a string, integer \
                             or boolean",
                            kind.unwrap_or("untyped")
                        ));
                    }
                    if found.iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
                        return Err(format!("x-mcp-header {name} appears twice"));
                    }
                    found.push(HeaderParam {
                        name: name.to_string(),
                        path: path.to_vec(),
                    });
                }
                for (key, value) in object {
                    match (key.as_str(), value, reachable) {
                        (HEADER_ANNOTATION, ..) => {}
                        ("properties", Value::Object(properties), Some(path)) => {
                            for (property, schema) in properties {
                                let mut path = path.to_vec();
                                path.push(property.clone());
                                scan(schema, Some(&path), found)?;
                            }
                        }
                        _ => scan(value, None, found)?,
                    }
                }
                Ok(())
            }
            Value::Array(items) => items.iter().try_for_each(|item| scan(item, None, found)),
            _ => Ok(()),
        }
    }
    let mut found = Vec::new();
    scan(schema, Some(&[]), &mut found)?;
    Ok(found)
}

/// The schema extension property naming a header ([`header_params`]).
const HEADER_ANNOTATION: &str = "x-mcp-header";

/// RFC 9110 `tchar` — what a header name may be made of.
const fn is_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// The `Mcp-Param-{Name}` headers one call's `arguments` produce: each
/// annotated argument's value at its exact path, a string as itself, an
/// integer in decimal and a boolean as `true`/`false`, all through
/// [`header_value`]'s encoding (the `Mcp-Name` rule). An argument that is
/// absent — or not the primitive its schema promised — sends no header.
#[must_use]
pub fn param_headers(params: &[HeaderParam], arguments: &Value) -> Vec<(String, String)> {
    params
        .iter()
        .filter_map(|param| {
            let value = param
                .path
                .iter()
                .try_fold(arguments, |node, key| node.get(key))?;
            let text = match value {
                Value::String(text) => text.clone(),
                Value::Bool(flag) => flag.to_string(),
                Value::Number(number) if number.is_i64() || number.is_u64() => number.to_string(),
                _ => return None,
            };
            Some((format!("Mcp-Param-{}", param.name), header_value(&text)))
        })
        .collect()
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
                    // A tool whose `x-mcp-header` annotations break the
                    // rules is left out, as the spec directs — never sent
                    // half its headers.
                    let header_params = header_params(&input_schema).ok()?;
                    Some(McpToolInfo {
                        name,
                        description,
                        input_schema,
                        header_params,
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
/// the content list is empty. `isError` flips the cell red. A modern
/// `resultType` of `input_required` (2026-07-28's multi-round-trip request)
/// resolves as a recoverable error — this client advertises no input
/// capabilities, so a compliant server shouldn't send one, and stalling a
/// tool call on input nobody will provide would hang the turn; an absent
/// `resultType` is `complete` (the compatibility rule for earlier servers).
#[must_use]
pub fn parse_call_result(result: &Value) -> CallOutcome {
    match result.get("resultType").and_then(Value::as_str) {
        None | Some("complete") => {}
        Some("input_required") => {
            return CallOutcome {
                text: "the server requested additional user input mid-call (a 2026-07-28 \
                       multi-round-trip request), which this client does not support — \
                       retry with complete arguments"
                    .to_string(),
                ok: false,
                image: None,
            };
        }
        // The spec's MUST: a result type this client does not know is
        // invalid, never read as `complete` (a `task` result's content is
        // not the tool's answer).
        Some(other) => {
            return CallOutcome {
                text: format!(
                    "the server answered with a result of type \"{other}\", which this client \
                     does not support"
                ),
                ok: false,
                image: None,
            };
        }
    }
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
                    data: None,
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
        // The handshake fallback proposes the newest *legacy* revision — the
        // modern one has no initialize to ride.
        let params = initialize_params();
        assert_eq!(params["protocolVersion"], LEGACY_PROTOCOL_VERSION);
        assert_eq!(params["clientInfo"]["name"], "alter-zero");
        assert_eq!(
            initialize_params_for("2025-03-26")["protocolVersion"],
            "2025-03-26"
        );
    }

    #[test]
    fn the_modern_revision_leads_and_the_legacy_fallback_is_named() {
        assert_eq!(PROTOCOL_VERSION, "2026-07-28");
        assert_eq!(LEGACY_PROTOCOL_VERSION, "2025-11-25");
        assert_eq!(SUPPORTED_VERSIONS[0], PROTOCOL_VERSION);
        assert!(SUPPORTED_VERSIONS.contains(&LEGACY_PROTOCOL_VERSION));
    }

    #[test]
    fn modern_meta_rides_every_request() {
        let params = with_meta(json!({"name": "t", "arguments": {}}), PROTOCOL_VERSION);
        assert_eq!(
            params["_meta"]["io.modelcontextprotocol/protocolVersion"],
            "2026-07-28"
        );
        assert_eq!(
            params["_meta"]["io.modelcontextprotocol/clientInfo"]["name"],
            "alter-zero"
        );
        // An empty capabilities object is the schema's own "no optional
        // capabilities" — required on every request, never elided.
        assert_eq!(
            params["_meta"]["io.modelcontextprotocol/clientCapabilities"],
            json!({})
        );
        assert_eq!(params["name"], "t");
        // Null params still become the bare `{"_meta": …}` object — modern
        // params are required, because `_meta` is.
        let bare = discover_params(PROTOCOL_VERSION);
        assert!(bare.get("_meta").is_some());
        assert_eq!(bare.as_object().unwrap().len(), 1);
    }

    #[test]
    fn discover_results_parse_identity_and_versions() {
        let (identity, supported) = parse_discover(
            &json!({
                "resultType": "complete",
                "supportedVersions": ["2026-07-28", "2025-11-25"],
                "capabilities": {"tools": {}, "prompts": {}},
                "_meta": {"io.modelcontextprotocol/serverInfo":
                    {"name": "vercel", "version": "2.0"}},
                "instructions": " deploy things ",
                "ttlMs": 3600000, "cacheScope": "public"
            }),
            PROTOCOL_VERSION,
        );
        assert_eq!(identity.protocol_version, "2026-07-28");
        assert_eq!(identity.name, "vercel");
        assert_eq!(identity.version, "2.0");
        assert_eq!(identity.capabilities, ["prompts", "tools"]);
        assert_eq!(identity.instructions.as_deref(), Some("deploy things"));
        assert_eq!(supported, ["2026-07-28", "2025-11-25"]);
        // serverInfo is never a top-level field in this revision — a result
        // without the _meta parses to an empty identity, not a panic.
        let (bare, none) = parse_discover(&json!({"supportedVersions": []}), PROTOCOL_VERSION);
        assert_eq!(bare.name, "");
        assert!(none.is_empty());
    }

    #[test]
    fn modern_errors_are_an_allowlist() {
        assert!(is_modern_error(-32020));
        assert!(is_modern_error(-32021));
        assert!(is_modern_error(-32022));
        // A legacy server's implementation-defined answers are NOT modern —
        // the spec forbids keying the fallback to one specific code.
        assert!(!is_modern_error(-32601));
        assert!(!is_modern_error(-32602));
        assert!(!is_modern_error(-32000));
    }

    #[test]
    fn unsupported_version_errors_name_the_servers_list() {
        let error = RpcError {
            code: -32022,
            message: "Unsupported protocol version".to_string(),
            data: Some(json!({"supported": ["2025-11-25", "2025-06-18"],
                              "requested": "2026-07-28"})),
        };
        assert_eq!(
            unsupported_versions(&error),
            Some(vec!["2025-11-25".to_string(), "2025-06-18".to_string()])
        );
        assert_eq!(
            unsupported_versions(&RpcError {
                code: -32601,
                message: "x".to_string(),
                data: None,
            }),
            None
        );
        // The wire parse keeps the data, so the list survives the transport.
        match parse_incoming(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32022,"message":"no",
                "data":{"supported":["2025-11-25"],"requested":"x"}}}"#,
        ) {
            Some(Incoming::Response {
                result: Err(error), ..
            }) => {
                assert_eq!(
                    unsupported_versions(&error),
                    Some(vec!["2025-11-25".to_string()])
                );
            }
            other => panic!("unexpected: {other:?}"),
        }
        // The newest mutual revision wins; nothing shared is None.
        assert_eq!(
            choose_version(&["2025-06-18".to_string(), "2025-11-25".to_string()]),
            Some("2025-11-25")
        );
        assert_eq!(choose_version(&["2099-01-01".to_string()]), None);
        assert_eq!(choose_version(&[]), None);
    }

    #[test]
    fn header_values_encode_only_when_unsafe() {
        assert_eq!(header_value("get_weather"), "get_weather");
        assert_eq!(header_value("tools/call"), "tools/call");
        // Non-ASCII, whitespace-framed, and sentinel-looking values take the
        // spec's base64 sentinel form.
        assert_eq!(header_value("héllo"), "=?base64?aMOpbGxv?=");
        assert!(header_value(" padded").starts_with("=?base64?"));
        // A value that *looks* like the sentinel is re-encoded so a decoder
        // can never mistake it for one.
        assert_eq!(header_value("=?base64?x?="), "=?base64?PT9iYXNlNjQ/eD89?=");
        assert!(header_value("").starts_with("=?base64?"));
    }

    #[test]
    fn an_input_required_result_fails_recoverably() {
        let outcome = parse_call_result(&json!({
            "resultType": "input_required",
            "inputRequests": {"login": {"method": "elicitation/create"}},
            "requestState": "s"
        }));
        assert!(!outcome.ok);
        assert!(outcome.text.contains("input"), "{}", outcome.text);
        // A modern complete result is the ordinary path, extra fields and
        // all; an absent resultType means complete (the compat rule).
        let ok = parse_call_result(&json!({
            "resultType": "complete",
            "content": [{"type": "text", "text": "hi"}]
        }));
        assert!(ok.ok);
        assert_eq!(ok.text, "hi");
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

    #[test]
    fn an_unknown_result_type_is_invalid_not_complete() {
        // The spec's MUST: an unrecognized `resultType` is invalid. A
        // `task` result's content is not the tool's answer.
        let task = parse_call_result(&json!({
            "resultType": "task",
            "content": [{"type": "text", "text": "queued"}]
        }));
        assert!(!task.ok);
        assert!(task.text.contains("\"task\""), "{}", task.text);
        let complete = parse_call_result(&json!({
            "resultType": "complete",
            "content": [{"type": "text", "text": "done"}]
        }));
        assert_eq!((complete.ok, complete.text.as_str()), (true, "done"));
    }

    #[test]
    fn x_mcp_header_annotations_are_read_and_held_to_the_rules() {
        // The spec's own example, plus a nested property reached through
        // `properties` alone.
        let schema = json!({
            "type": "object",
            "properties": {
                "region": {"type": "string", "x-mcp-header": "Region"},
                "query": {"type": "string"},
                "options": {"type": "object", "properties": {
                    "dryRun": {"type": "boolean", "x-mcp-header": "Dry-Run"}
                }}
            }
        });
        assert_eq!(
            header_params(&schema),
            Ok(vec![
                HeaderParam {
                    name: "Region".to_string(),
                    path: vec!["region".to_string()],
                },
                HeaderParam {
                    name: "Dry-Run".to_string(),
                    path: vec!["options".to_string(), "dryRun".to_string()],
                },
            ])
        );
        // Nothing annotated, nothing to send.
        assert_eq!(header_params(&json!({"type": "object"})), Ok(Vec::new()));
        // Every violation makes the tool definition invalid.
        for (why, schema) in [
            (
                "not a token",
                json!({"properties": {"a": {"type": "string", "x-mcp-header": "Has Space"}}}),
            ),
            (
                "empty",
                json!({"properties": {"a": {"type": "string", "x-mcp-header": ""}}}),
            ),
            (
                "a number",
                json!({"properties": {"a": {"type": "number", "x-mcp-header": "A"}}}),
            ),
            (
                "an object",
                json!({"properties": {"a": {"type": "object", "x-mcp-header": "A"}}}),
            ),
            (
                "a duplicate without regard to case",
                json!({"properties": {
                    "a": {"type": "string", "x-mcp-header": "Tenant"},
                    "b": {"type": "string", "x-mcp-header": "tenant"}
                }}),
            ),
            (
                "inside items",
                json!({"properties": {"a": {"type": "array",
                    "items": {"type": "string", "x-mcp-header": "A"}}}}),
            ),
            (
                "inside a composition",
                json!({"properties": {"a": {"anyOf": [
                    {"type": "string", "x-mcp-header": "A"}]}}}),
            ),
            (
                "under $defs",
                json!({"$defs": {"T": {"type": "string", "x-mcp-header": "A"}},
                       "properties": {"a": {"$ref": "#/$defs/T"}}}),
            ),
            (
                "on the root",
                json!({"type": "object", "x-mcp-header": "A"}),
            ),
        ] {
            assert!(header_params(&schema).is_err(), "{why}");
        }
    }

    #[test]
    fn annotated_arguments_become_encoded_param_headers() {
        let params = vec![
            HeaderParam {
                name: "Region".to_string(),
                path: vec!["region".to_string()],
            },
            HeaderParam {
                name: "Greeting".to_string(),
                path: vec!["greeting".to_string()],
            },
            HeaderParam {
                name: "Limit".to_string(),
                path: vec!["opts".to_string(), "limit".to_string()],
            },
            HeaderParam {
                name: "Dry".to_string(),
                path: vec!["dry".to_string()],
            },
            HeaderParam {
                name: "Absent".to_string(),
                path: vec!["absent".to_string()],
            },
        ];
        let headers = param_headers(
            &params,
            &json!({
                "region": "us-west1",
                "greeting": "Hello, 世界",
                "opts": {"limit": 42},
                "dry": false,
                "query": "SELECT 1"
            }),
        );
        // The spec's encoding table: plain ASCII as itself, anything else in
        // the base64 sentinel; an absent argument sends no header at all.
        assert_eq!(
            headers,
            [
                ("Mcp-Param-Region".to_string(), "us-west1".to_string()),
                (
                    "Mcp-Param-Greeting".to_string(),
                    "=?base64?SGVsbG8sIOS4lueVjA==?=".to_string()
                ),
                ("Mcp-Param-Limit".to_string(), "42".to_string()),
                ("Mcp-Param-Dry".to_string(), "false".to_string()),
            ]
        );
        // A value that is not the primitive its schema promised sends none.
        assert!(param_headers(&params[..1], &json!({"region": 1.5})).is_empty());
    }

    #[test]
    fn a_tool_with_an_invalid_header_annotation_is_left_out_of_the_list() {
        let (tools, _) = parse_tools_page(&json!({"tools": [
            {"name": "good", "inputSchema": {"type": "object", "properties": {
                "region": {"type": "string", "x-mcp-header": "Region"}}}},
            {"name": "bad", "inputSchema": {"type": "object", "properties": {
                "ratio": {"type": "number", "x-mcp-header": "Ratio"}}}},
            {"name": "plain", "inputSchema": {"type": "object"}}
        ]}));
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["good", "plain"]);
        assert_eq!(tools[0].header_params.len(), 1);
    }
}
