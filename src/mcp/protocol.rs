//! The MCP JSON-RPC protocol shapes (`docs/tools/mcp.md`): request/notification
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
/// settles on is whatever the server names back (`docs/tools/mcp.md`).
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
/// Claude Code's `MAX_MCP_DESCRIPTION_LENGTH`, 4,096 since it doubled the
/// old 2,048 (2.1.296): enough for the usage notes a server puts there, while
/// a wiki-length description would spend the context budget the schemas
/// share.
pub const MAX_TOOL_DESCRIPTION_CHARS: usize = 4096;

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

/// One message parsed off a transport: the response to a request we made, a
/// request the server is making of us, or something else (a notification —
/// dropped; the tools surface needs none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    /// A response carrying the `id` it answers and the outcome.
    Response {
        id: u64,
        result: Result<Value, RpcError>,
    },
    /// An error response with no usable `id` — the `id: null` a server sends
    /// when it could not read the request at all (a parse error, a missing
    /// header). With one request in flight, it is that request's answer.
    OrphanError(RpcError),
    /// A server→client request (`ping`, `roots/list`, …), its `id` kept
    /// exactly as it came so an answer can echo it. A legacy-era server
    /// waits for one — JSON-RPC's "MUST reply" — while a modern server
    /// never sends any (they ride `input_required` results instead).
    Request { id: Value, method: String },
    /// A notification — not one of ours to answer.
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
        write!(f, "{} (code {})", self.message, self.code)?;
        // `-32021` names the capabilities the server needed and didn't get —
        // the one part of the error the model can act on (by saying so).
        if self.code == -32021
            && let Some(needed) = self
                .data
                .as_ref()
                .and_then(|data| data.get("requiredCapabilities"))
                .and_then(Value::as_object)
            && !needed.is_empty()
        {
            let names: Vec<&str> = needed.keys().map(String::as_str).collect();
            write!(f, " — it needs: {}", names.join(", "))?;
        }
        Ok(())
    }
}

/// Parse one incoming JSON-RPC message. `None` when the text isn't JSON at
/// all (transport noise — the caller decides how loud to be).
///
/// Read the way real servers write it rather than the way JSON-RPC says to:
/// an id echoed as `"5"` or `5.0` is still request 5, an `"error": null`
/// beside a result (Go structs without `omitempty`) is no error, and an
/// error with `id: null` is surfaced as [`Incoming::OrphanError`] rather than
/// dropped — dropped, it left the request waiting out its whole deadline.
#[must_use]
pub fn parse_incoming(text: &str) -> Option<Incoming> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    let error = value.get("error").filter(|error| !error.is_null());
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        // A message with a method is the server's: with an id it is a
        // request of us, without one a notification.
        return Some(match value.get("id").filter(|id| !id.is_null()) {
            Some(id) => Incoming::Request {
                id: id.clone(),
                method: method.to_string(),
            },
            None => Incoming::Other,
        });
    }
    let Some(id) = value.get("id").and_then(request_id) else {
        return Some(match error {
            Some(error) if value.get("id").is_none_or(Value::is_null) => {
                Incoming::OrphanError(rpc_error(error))
            }
            _ => Incoming::Other,
        });
    };
    if let Some(error) = error {
        return Some(Incoming::Response {
            id,
            result: Err(rpc_error(error)),
        });
    }
    Some(Incoming::Response {
        id,
        result: Ok(value.get("result").cloned().unwrap_or(Value::Null)),
    })
}

/// One of our request ids as a server echoed it: the integer we sent, or
/// that integer spelled as a string or an integral float.
fn request_id(id: &Value) -> Option<u64> {
    match id {
        Value::Number(number) => number.as_u64().or_else(|| {
            number
                .as_f64()
                .filter(|f| *f >= 0.0 && f.fract() == 0.0 && *f < 9.007_199_254_740_992e15)
                .map(|f| f as u64)
        }),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// A JSON-RPC error object, flattened.
fn rpc_error(error: &Value) -> RpcError {
    RpcError {
        code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
        message: error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string(),
        data: error.get("data").cloned(),
    }
}

/// This client's answer to a server→client request — what a **legacy**
/// (handshake-era) server is owed: JSON-RPC's "MUST reply", and 2025-11-25's
/// `ping` "MUST respond promptly". `ping` gets the empty result; anything
/// else gets method-not-found, since this client declares no capability
/// that would make it a request it could serve. A modern server never sends
/// one (its requests ride `input_required` results).
#[must_use]
pub fn server_request_answer(id: &Value, method: &str) -> Value {
    if method == "ping" {
        return json!({"jsonrpc": "2.0", "id": id, "result": {}});
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32601,
            "message": format!("Method not found: {method} (this client declares no capabilities)")
        }
    })
}

/// Why a result can't be read as an ordinary one — `None` for an absent
/// `resultType` (an earlier-revision server; "MUST treat … as complete") and
/// for `"complete"`. Anything else is "considered invalid" (2026-07-28), so a
/// task handle or a value from a revision to come never passes for an empty
/// success. `input_required` is the one other value this client handles,
/// and only on `tools/call` ([`input_required`]).
#[must_use]
pub fn result_type_problem(result: &Value) -> Option<String> {
    match result.get("resultType") {
        None | Some(Value::Null) => None,
        Some(Value::String(kind)) if kind == "complete" => None,
        Some(other) => Some(format!(
            "the server answered with a result type this client does not know ({other})"
        )),
    }
}

/// What an `input_required` result asks of the client (2026-07-28's
/// multi-round-trip requests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputRequired {
    /// Only a `requestState`: the server wants the same call again with the
    /// state echoed — a load-shedding server's "ask me again", which a
    /// client declaring no capabilities can still answer.
    Retry { request_state: String },
    /// `inputRequests` this client can't fulfil (it declares no elicitation,
    /// sampling or roots) — named by method for the error the model reads.
    Unsupported { methods: Vec<String> },
}

/// Read an `input_required` result — `None` for any other result.
#[must_use]
pub fn input_required(result: &Value) -> Option<InputRequired> {
    if result.get("resultType").and_then(Value::as_str) != Some("input_required") {
        return None;
    }
    let methods: Vec<String> = result
        .get("inputRequests")
        .and_then(Value::as_object)
        .map(|requests| {
            requests
                .values()
                .map(|request| {
                    request
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or("an unknown request")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    if !methods.is_empty() {
        return Some(InputRequired::Unsupported { methods });
    }
    match result.get("requestState").and_then(Value::as_str) {
        Some(state) => Some(InputRequired::Retry {
            request_state: state.to_string(),
        }),
        // Neither: a malformed interim result — nothing to answer it with.
        None => Some(InputRequired::Unsupported {
            methods: Vec::new(),
        }),
    }
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
            tools_list_changed: promises_tool_changes(result),
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
    /// The server's optional usage instructions — shown in the detail page
    /// and carried by the `<system-reminder>` (`mcp::instructions_section`).
    pub instructions: Option<String>,
    /// `capabilities.tools.listChanged`: the server says when its tools
    /// change — what the client opens a change stream for (`docs/tools/mcp.md`).
    pub tools_list_changed: bool,
}

/// Does a result's `capabilities` promise tool-list change notifications?
fn promises_tool_changes(result: &Value) -> bool {
    result
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("tools"))
        .and_then(|tools| tools.get("listChanged"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The notification a server sends when its tool list changed.
pub const TOOLS_LIST_CHANGED: &str = "notifications/tools/list_changed";

/// A notification's method — a message with a `method` and no `id` — when
/// `message` is one.
#[must_use]
pub fn notification_method(message: &str) -> Option<String> {
    let value: Value = serde_json::from_str(message.trim()).ok()?;
    if value.get("id").is_some() {
        return None;
    }
    value.get("method")?.as_str().map(str::to_string)
}

/// The `subscriptions/listen` params (2026-07-28) asking for the tool list's
/// change notifications — the one kind this client acts on, and a server
/// MUST NOT send a kind the client did not ask for.
#[must_use]
pub fn listen_params() -> Value {
    json!({"notifications": {"toolsListChanged": true}})
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
        tools_list_changed: promises_tool_changes(result),
    }
}

/// One tool a server offers, as `tools/list` described it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolInfo {
    /// The server's own (raw) tool name — what `tools/call` sends back.
    pub name: String,
    pub description: String,
    /// The tool's JSON-Schema input **as the server sent it** (a missing or
    /// non-object one replaced by the empty object schema) — what the
    /// `/mcp` page and the `x-mcp-header` mirror read. The model is offered
    /// the repaired copy [`model_input_schema`] makes, never this.
    pub input_schema: Value,
}

/// A tool the server listed that the model is **not** offered, and why — set
/// aside at connect, named on the `/mcp` server page, so one broken
/// definition costs only itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludedTool {
    pub name: String,
    pub reason: String,
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
    // An empty string is a cursor like any other (2026-07-28: it "MUST NOT
    // be treated as the end of results"); only an absent one ends the list.
    let cursor = result
        .get("nextCursor")
        .and_then(Value::as_str)
        .map(str::to_string);
    (tools, cursor)
}

/// What a folded root combinator adds to the front of the tool's description
/// ([`model_input_schema`]) — the one fact the flat object loses.
pub const FOLDED_SCHEMA_NOTE: &str =
    "Arguments come in one of several shapes; pass the parameters one of them needs.";

/// A tool's input schema **as the model is offered it** — every request
/// carries it as the function's `parameters`, so a shape a provider refuses
/// fails every turn, not just this tool's calls. The server's own schema
/// stays on [`McpToolInfo`] (the `/mcp` page and the `x-mcp-header` mirror
/// read it); this is the repaired copy:
///
/// - the root is always `{"type": "object", "properties": {…}}` — both API
///   families insist on the pair, and MCP requires the type anyway;
/// - `$schema` goes (a dialect tag no function-calling API reads, which
///   some refuse as an unknown keyword), as do a `required` that is not a
///   list of names and every `x-mcp-header` annotation (transport metadata,
///   [`strip_header_annotations`](crate::mcp::strip_header_annotations));
/// - a root `oneOf`/`anyOf`/`allOf` — refused at the top of a schema by
///   Anthropic — folds into the one object: every branch's properties, and
///   required what `allOf` requires or what *every* alternative requires.
///   For alternatives the returned note says so, for the caller to put at
///   the front of the description.
///
/// Everything else passes through in the server's order, so the same
/// listing always yields the same bytes (the prompt-cache prefix).
#[must_use]
pub fn model_input_schema(schema: &Value) -> (Value, Option<&'static str>) {
    let mut source = match super::headers::strip_header_annotations(schema) {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    source.shift_remove("$schema");
    source.shift_remove("type");
    let mut properties = match source.shift_remove("properties") {
        Some(Value::Object(properties)) => properties,
        _ => serde_json::Map::new(),
    };
    let mut required = string_list(source.shift_remove("required"));
    let mut note = None;
    for key in ["allOf", "anyOf", "oneOf"] {
        let Some(branches) = source.shift_remove(key) else {
            continue;
        };
        let branches = match branches {
            Value::Array(branches) => branches,
            _ => Vec::new(),
        };
        let mut shared: Option<Vec<String>> = None;
        for branch in &branches {
            let branch_required = string_list(branch.get("required").cloned());
            if let Some(Value::Object(more)) = branch.get("properties") {
                for (name, property) in more {
                    properties
                        .entry(name.clone())
                        .or_insert_with(|| property.clone());
                }
            }
            if key == "allOf" {
                extend_unique(&mut required, branch_required);
            } else {
                shared = Some(match shared {
                    None => branch_required,
                    Some(so_far) => so_far
                        .into_iter()
                        .filter(|name| branch_required.contains(name))
                        .collect(),
                });
            }
        }
        if key != "allOf" {
            extend_unique(&mut required, shared.unwrap_or_default());
            if !branches.is_empty() {
                note = Some(FOLDED_SCHEMA_NOTE);
            }
        }
    }
    let mut out = serde_json::Map::new();
    out.insert("type".to_string(), Value::String("object".to_string()));
    out.insert("properties".to_string(), Value::Object(properties));
    if !required.is_empty() {
        out.insert(
            "required".to_string(),
            Value::Array(required.into_iter().map(Value::String).collect()),
        );
    }
    out.extend(source);
    (Value::Object(out), note)
}

/// The names in a `required` value — anything that is not a string dropped.
fn string_list(value: Option<Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| match item {
                Value::String(name) => Some(name),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Append `more` to `list`, skipping names already in it.
fn extend_unique(list: &mut Vec<String>, more: Vec<String>) {
    for name in more {
        if !list.contains(&name) {
            list.push(name);
        }
    }
}

/// Why a tool can't be offered at all — `None` when its schema is usable
/// once [`model_input_schema`] has repaired it. Two things no repair fixes:
/// a root that declares a type other than `object` (the arguments the model
/// would send could never be what the tool takes), and a parameter whose
/// name Anthropic refuses (`^[a-zA-Z0-9_.-]{1,64}$`, the stricter of the two
/// API families) — such a tool is left out by name rather than failing
/// every request it rides in.
#[must_use]
pub fn input_schema_problem(schema: &Value) -> Option<String> {
    match schema.get("type") {
        None | Some(Value::Null) => {}
        Some(Value::String(kind)) if kind == "object" => {}
        Some(Value::Array(kinds)) if kinds.iter().any(|kind| kind == "object") => {}
        Some(other) => {
            return Some(format!("its input schema has type {other}, not \"object\""));
        }
    }
    let (schema, _) = model_input_schema(schema);
    let properties = schema.get("properties").and_then(Value::as_object)?;
    properties
        .keys()
        .find(|name| !is_model_parameter_name(name))
        .map(|name| {
            format!(
                "its parameter \"{name}\" has a name model APIs refuse \
                 (letters, digits, '_', '.', '-'; at most 64)"
            )
        })
}

/// `^[a-zA-Z0-9_.-]{1,64}$` — the parameter names every model API accepts.
fn is_model_parameter_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
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
    call_params_retrying(tool, arguments, None)
}

/// [`call_params`] for the retry an [`InputRequired::Retry`] asks for: the
/// same name and arguments, the server's `requestState` echoed **exactly**
/// (and none at all when it sent none — the MRTR rules).
#[must_use]
pub fn call_params_retrying(tool: &str, arguments: &Value, request_state: Option<&str>) -> Value {
    let arguments = match arguments {
        Value::Object(_) => arguments.clone(),
        Value::Null => json!({}),
        other => json!({"value": other}),
    };
    let mut params = json!({"name": tool, "arguments": arguments});
    if let Some(state) = request_state {
        params["requestState"] = Value::String(state.to_string());
    }
    params
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

/// Map a `tools/call` result. Text items are their text, in order (a lone
/// one — the overwhelmingly common shape — verbatim); `structuredContent`
/// stands in when nothing else came. Binary content is **described, never
/// dumped**: the first image rides [`CallOutcome::image`] (the `read`
/// tool's channel, so a vision model sees it) under a `[image: …]` line, and
/// a second image, audio, a blob resource — base64 that would spend the
/// output budget on noise — each leave a one-line placeholder, while a
/// resource link says where it points and an embedded text resource is its
/// text. `isError` flips the cell red.
///
/// The `resultType` rules (2026-07-28): absent means complete (an earlier
/// server), `input_required` is a recoverable failure here — the caller
/// answers the retryable kind before it gets this far
/// ([`input_required`]) — and anything else is invalid
/// ([`result_type_problem`]), never a silent empty success.
#[must_use]
pub fn parse_call_result(result: &Value) -> CallOutcome {
    let fail = |text: String| CallOutcome {
        text,
        ok: false,
        image: None,
    };
    match input_required(result) {
        Some(InputRequired::Unsupported { methods }) if !methods.is_empty() => {
            return fail(format!(
                "the server asked for input this client can't provide ({}) — \
                 try the call another way, or tell the user it needs this",
                methods.join(", ")
            ));
        }
        Some(_) => {
            return fail(
                "the server kept asking for the call to be repeated — try again shortly"
                    .to_string(),
            );
        }
        None => {}
    }
    if let Some(problem) = result_type_problem(result) {
        return fail(problem);
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
                if let Some(text) = content_text(item, &mut image) {
                    texts.push(text);
                }
            }
            texts.join("\n")
        }
        None => String::new(),
    };
    let text = if text.trim().is_empty() {
        match result.get("structuredContent") {
            Some(structured) if !structured.is_null() => {
                serde_json::to_string_pretty(structured).unwrap_or_else(|_| "{}".to_string())
            }
            _ if ok => "(no content)".to_string(),
            _ => "tool call failed".to_string(),
        }
    } else {
        text
    };
    CallOutcome { text, ok, image }
}

/// One content item as the model reads it — see [`parse_call_result`].
/// `image` takes the first image's `data:` URL.
fn content_text(item: &Value, image: &mut Option<String>) -> Option<String> {
    let field = |key: &str| item.get(key).and_then(Value::as_str);
    let mime = || field("mimeType").unwrap_or("unknown type");
    match field("type") {
        Some("text") => field("text").map(str::to_string),
        Some("image") => {
            let data = field("data")?;
            if image.is_none() {
                *image = Some(format!("data:{};base64,{data}", mime()));
                Some(format!("[image: {}]", mime()))
            } else {
                Some(format!(
                    "[image: {}, {} — only the first image is shown]",
                    mime(),
                    encoded_size(data)
                ))
            }
        }
        Some("audio") => Some(format!(
            "[audio: {}, {} — not supported here]",
            mime(),
            encoded_size(field("data").unwrap_or_default())
        )),
        Some("resource_link") => {
            let uri = field("uri").unwrap_or_default();
            let name = field("title").or_else(|| field("name")).unwrap_or(uri);
            let mut line = format!("[resource link: {name} — {uri}");
            if let Some(mime) = field("mimeType") {
                line.push_str(&format!(" ({mime})"));
            }
            line.push(']');
            if let Some(description) = field("description").filter(|d| !d.trim().is_empty()) {
                line.push_str(&format!(" {}", description.trim()));
            }
            Some(line)
        }
        Some("resource") => {
            let resource = item.get("resource")?;
            let uri = resource
                .get("uri")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(text) = resource.get("text").and_then(Value::as_str) {
                return Some(format!("[resource: {uri}]\n{text}"));
            }
            let mime = resource
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("unknown type");
            let blob = resource
                .get("blob")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some(format!(
                "[resource: {uri} ({mime}), {} of binary data — not shown]",
                encoded_size(blob)
            ))
        }
        // An item of a kind this client doesn't know: kept as JSON so the
        // model still sees what arrived, with any long string (a payload,
        // most likely) cut down to its length.
        _ => Some(
            serde_json::to_string_pretty(&elide_long_strings(item))
                .unwrap_or_else(|_| "{}".to_string()),
        ),
    }
}

/// The decoded size of a base64 payload, for a placeholder line.
fn encoded_size(base64: &str) -> String {
    let bytes = base64.trim().len() / 4 * 3;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

/// `value` with every string over 1 KiB replaced by a note of its length.
fn elide_long_strings(value: &Value) -> Value {
    match value {
        Value::String(text) if text.len() > 1024 => {
            Value::String(format!("[{} characters]", text.chars().count()))
        }
        Value::Array(items) => Value::Array(items.iter().map(elide_long_strings).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), elide_long_strings(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A change notification is told apart from everything else a server
    /// sends — a response, its own request — by its method and the `id` it
    /// lacks.
    #[test]
    fn notifications_are_read_by_method() {
        assert_eq!(
            notification_method(r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#)
                .as_deref(),
            Some(TOOLS_LIST_CHANGED)
        );
        assert_eq!(
            notification_method(
                r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed","params":{"_meta":{"io.modelcontextprotocol/subscriptionId":3}}}"#
            )
            .as_deref(),
            Some(TOOLS_LIST_CHANGED)
        );
        assert_eq!(
            notification_method(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
            None
        );
        assert_eq!(
            notification_method(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            None
        );
        assert_eq!(notification_method("not json"), None);
        assert_eq!(
            listen_params(),
            serde_json::json!({"notifications": {"toolsListChanged": true}})
        );
    }

    /// `capabilities.tools.listChanged` — the server's promise to say when
    /// its tools change — is read in both eras.
    #[test]
    fn a_server_promising_tool_changes_says_so_in_either_era() {
        let legacy = parse_initialize(&serde_json::json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {"listChanged": true}},
            "serverInfo": {"name": "s", "version": "1"}
        }));
        assert!(legacy.tools_list_changed);
        let (modern, _) = parse_discover(
            &serde_json::json!({"capabilities": {"tools": {"listChanged": true}}}),
            "2026-07-28",
        );
        assert!(modern.tools_list_changed);
        let quiet = parse_initialize(&serde_json::json!({"capabilities": {"tools": {}}}));
        assert!(!quiet.tools_list_changed);
    }

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
        // A server→client request is one of its own — a legacy server's
        // `ping` must be answered, with its id as it came.
        assert_eq!(
            parse_incoming(r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#),
            Some(Incoming::Request {
                id: json!(9),
                method: "ping".to_string(),
            })
        );
        assert_eq!(parse_incoming("not json"), None);
    }

    /// Real servers stretch JSON-RPC in ways that cost a whole connect when
    /// read strictly: Go structs serialize `"error": null` beside a result,
    /// some libraries echo the id as a string or a float, and transport-level
    /// errors (a body that failed to parse, a missing header) carry `id: null`.
    #[test]
    fn incoming_reads_the_shapes_real_servers_send() {
        assert_eq!(
            parse_incoming(r#"{"jsonrpc":"2.0","id":2,"result":{"ok":1},"error":null}"#),
            Some(Incoming::Response {
                id: 2,
                result: Ok(json!({"ok": 1})),
            })
        );
        for raw in [
            r#"{"jsonrpc":"2.0","id":"5","result":{}}"#,
            r#"{"jsonrpc":"2.0","id":5.0,"result":{}}"#,
        ] {
            assert!(
                matches!(parse_incoming(raw), Some(Incoming::Response { id: 5, .. })),
                "{raw}"
            );
        }
        assert_eq!(
            parse_incoming(
                r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}"#
            ),
            Some(Incoming::OrphanError(RpcError {
                code: -32700,
                message: "Parse error".to_string(),
                data: None,
            }))
        );
    }

    /// A legacy server's request is answered the way JSON-RPC requires:
    /// `ping` with the empty result, anything else (a `roots/list` from a
    /// server ignoring our empty capabilities) with method-not-found — the
    /// id echoed as it came.
    #[test]
    fn a_server_request_is_answered_ping_with_empty_the_rest_unknown() {
        assert_eq!(
            server_request_answer(&json!("p-1"), "ping"),
            json!({"jsonrpc": "2.0", "id": "p-1", "result": {}})
        );
        let refused = server_request_answer(&json!(7), "roots/list");
        assert_eq!(refused["id"], json!(7));
        assert_eq!(refused["error"]["code"], json!(-32601));
        assert!(refused.get("result").is_none());
    }

    /// A server that needs a capability this client didn't declare says
    /// which (`-32021`'s `requiredCapabilities`) — the message the model
    /// reads names it instead of a bare code.
    #[test]
    fn a_missing_capability_error_names_the_capability() {
        let error = RpcError {
            code: -32021,
            message: "Missing required client capability".to_string(),
            data: Some(json!({"requiredCapabilities": {"elicitation": {}, "sampling": {}}})),
        };
        let shown = error.to_string();
        assert!(
            shown.contains("elicitation") && shown.contains("sampling"),
            "{shown}"
        );
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

    /// "A `resultType` of any value unrecognized by the client MUST be
    /// considered invalid" — a task handle or a future value must never
    /// read as a silent `(no content)` success.
    #[test]
    fn an_unknown_result_type_is_invalid() {
        let outcome = parse_call_result(&json!({
            "resultType": "task",
            "task": {"taskId": "t1"}
        }));
        assert!(!outcome.ok);
        assert!(outcome.text.contains("task"), "{}", outcome.text);
        assert_eq!(
            result_type_problem(&json!({"resultType": "complete"})),
            None
        );
        assert_eq!(result_type_problem(&json!({})), None);
        assert!(result_type_problem(&json!({"resultType": "later"})).is_some());
    }

    /// An `input_required` result carrying only `requestState` is a server
    /// asking to be asked again (load shedding) — retryable by a client that
    /// declared nothing; one carrying `inputRequests` asks for what this
    /// client can't give, and names it.
    #[test]
    fn input_required_tells_a_retry_from_a_request_for_input() {
        assert_eq!(
            input_required(&json!({"resultType": "input_required", "requestState": "s1"})),
            Some(InputRequired::Retry {
                request_state: "s1".to_string()
            })
        );
        assert_eq!(
            input_required(&json!({
                "resultType": "input_required",
                "inputRequests": {"who": {"method": "elicitation/create", "params": {}}},
                "requestState": "s2"
            })),
            Some(InputRequired::Unsupported {
                methods: vec!["elicitation/create".to_string()]
            })
        );
        assert_eq!(input_required(&json!({"resultType": "complete"})), None);
        // The retry echoes the state exactly, beside the same name and
        // arguments.
        assert_eq!(
            call_params_retrying("t", &json!({"a": 1}), Some("s1")),
            json!({"name": "t", "arguments": {"a": 1}, "requestState": "s1"})
        );
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

    /// The schema rides **every** request as the function's `parameters`,
    /// so a shape a provider refuses fails every turn, not just the call:
    /// the root must say `type: "object"` and carry `properties` (both
    /// OpenAI- and Anthropic-shaped APIs insist), whatever the server left out.
    #[test]
    fn the_model_schema_always_has_an_object_root_with_properties() {
        let cases = [
            (json!({}), json!({"type": "object", "properties": {}})),
            (json!(null), json!({"type": "object", "properties": {}})),
            (json!(true), json!({"type": "object", "properties": {}})),
            (
                json!({"properties": {"q": {"type": "string"}}}),
                json!({"type": "object", "properties": {"q": {"type": "string"}}}),
            ),
            (
                json!({"type": "object"}),
                json!({"type": "object", "properties": {}}),
            ),
            (
                json!({"type": ["object", "null"], "properties": null}),
                json!({"type": "object", "properties": {}}),
            ),
        ];
        for (input, want) in cases {
            assert_eq!(model_input_schema(&input).0, want, "{input}");
        }
    }

    #[test]
    fn the_model_schema_drops_what_providers_refuse_and_keeps_the_rest() {
        let (schema, note) = model_input_schema(&json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {"q": {"type": "string", "format": "uri"}},
            "required": ["q", 7],
            "additionalProperties": false
        }));
        assert_eq!(note, None);
        assert_eq!(
            schema,
            json!({
                "type": "object",
                "properties": {"q": {"type": "string", "format": "uri"}},
                "required": ["q"],
                "additionalProperties": false
            })
        );
        let (schema, _) = model_input_schema(&json!({"type": "object", "required": "q"}));
        assert_eq!(schema, json!({"type": "object", "properties": {}}));
    }

    /// Anthropic refuses `oneOf`/`anyOf`/`allOf` at the top of a tool's
    /// schema, so the branches fold into one object — every branch's
    /// properties, required only where every alternative requires it — and
    /// the description says the shapes are alternatives.
    #[test]
    fn root_combinators_fold_into_one_object() {
        let (schema, note) = model_input_schema(&json!({
            "type": "object",
            "oneOf": [
                {"properties": {"id": {"type": "string"}, "kind": {"type": "string"}},
                 "required": ["id", "kind"]},
                {"properties": {"url": {"type": "string"}, "kind": {"type": "string"}},
                 "required": ["url", "kind"]}
            ]
        }));
        assert_eq!(
            schema,
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "kind": {"type": "string"},
                    "url": {"type": "string"}
                },
                "required": ["kind"]
            })
        );
        assert!(note.is_some_and(|note| note.contains("one of")));
        // allOf: every branch applies, so everything any branch requires is
        // required.
        let (schema, _) = model_input_schema(&json!({
            "allOf": [
                {"properties": {"a": {"type": "string"}}, "required": ["a"]},
                {"properties": {"b": {"type": "integer"}}, "required": ["b"]}
            ]
        }));
        assert_eq!(schema["required"], json!(["a", "b"]));
        assert_eq!(schema["type"], "object");
    }

    /// What can't be repaired is refused, by tool and with the reason, so
    /// one broken tool never takes the rest of the session down with it.
    #[test]
    fn a_tool_whose_input_can_never_be_an_object_is_refused() {
        assert!(input_schema_problem(&json!({"type": "object"})).is_none());
        assert!(input_schema_problem(&json!({})).is_none());
        let refused = input_schema_problem(&json!({"type": "string"})).expect("refused");
        assert!(refused.contains("string"), "{refused}");
        // A parameter name Anthropic refuses (`^[a-zA-Z0-9_.-]{1,64}$`).
        let refused = input_schema_problem(&json!({
            "type": "object",
            "properties": {"file path": {"type": "string"}}
        }))
        .expect("refused");
        assert!(refused.contains("file path"), "{refused}");
        let long = "p".repeat(65);
        assert!(
            input_schema_problem(&json!({"type": "object", "properties": {long: {}}})).is_some()
        );
    }

    #[test]
    fn a_huge_description_is_capped() {
        // Claude Code's cap since 2.1.296 — a description long enough to
        // carry real usage notes, short of a wiki page.
        assert_eq!(MAX_TOOL_DESCRIPTION_CHARS, 4096);
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

    /// Binary content never reaches the model as base64 text: a second
    /// image, audio and a blob resource are named in one line each, a link
    /// says where it points, and an embedded text resource is its text.
    #[test]
    fn non_text_content_is_described_never_dumped() {
        let base64 = "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFB".repeat(40);
        let outcome = parse_call_result(&json!({"content": [
            {"type": "image", "data": "AAAA", "mimeType": "image/png"},
            {"type": "image", "data": base64, "mimeType": "image/jpeg"},
            {"type": "audio", "data": base64, "mimeType": "audio/wav"},
            {"type": "resource_link", "uri": "file:///r/a.txt", "name": "a.txt",
             "mimeType": "text/plain"},
            {"type": "resource", "resource": {"uri": "file:///r/b.md", "text": "# B"}},
            {"type": "resource", "resource": {"uri": "file:///r/c.bin", "blob": base64,
             "mimeType": "application/octet-stream"}}
        ]}));
        assert!(outcome.ok);
        assert_eq!(outcome.image.as_deref(), Some("data:image/png;base64,AAAA"));
        assert!(!outcome.text.contains("QUFB"), "{}", outcome.text);
        assert!(
            outcome.text.contains("[image: image/jpeg"),
            "{}",
            outcome.text
        );
        assert!(
            outcome.text.contains("[audio: audio/wav"),
            "{}",
            outcome.text
        );
        assert!(outcome.text.contains("a.txt") && outcome.text.contains("file:///r/a.txt"));
        assert!(outcome.text.contains("# B"), "{}", outcome.text);
        assert!(outcome.text.contains("file:///r/c.bin"), "{}", outcome.text);
    }

    /// 2026-07-28: "an empty string is a valid cursor and thus MUST NOT be
    /// treated as the end of results".
    #[test]
    fn an_empty_cursor_continues_the_listing() {
        let (_, cursor) = parse_tools_page(&json!({"tools": [], "nextCursor": ""}));
        assert_eq!(cursor.as_deref(), Some(""));
        let (_, cursor) = parse_tools_page(&json!({"tools": []}));
        assert_eq!(cursor, None);
    }
}
