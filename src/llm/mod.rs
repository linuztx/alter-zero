//! The real LLM backend: an OpenAI-compatible streaming client and the pieces
//! that plug it into the app's [`ReplySource`](crate::stream::ReplySource) seam.
//!
//! This is the **I/O boundary** for a real model (like `main.rs`/`term.rs`): the
//! only place that speaks HTTP. The pure cores — [`config`] (the `providers.toml`
//! parse), [`thinking`] (the reasoning splitter), [`models::parse_models`], and
//! the payload/endpoint builders in [`openai`] — are unit-tested; the network
//! calls are verified by hand. See `docs/llm.md`.
//!
//! Streaming runs on a plain OS thread (the
//! [`ReplySource::spawn`](crate::stream::ReplySource::spawn) contract), so
//! the HTTP client is **blocking** `reqwest`: a response is a `std::io::Read` we
//! drain line-by-line for SSE frames while polling the
//! [`CancelToken`](crate::stream::CancelToken) — the same cooperative-cancel
//! shape as `DummyAi`, with no nested tokio runtime.

pub mod agent;
pub mod approval;
pub mod ask;
pub mod backend;
pub mod cache;
pub mod classifier;
pub mod config;
pub mod exec;
pub mod keystore;
pub mod models;
pub mod openai;
pub mod reasoning;
pub mod retry;
pub mod settings;
pub mod task;
pub mod thinking;
pub mod tools;

use std::time::Duration;

pub use backend::LlmBackend;
pub use config::{ModelConfig, ProvidersFile, Selection};
pub use keystore::EnvFile;
pub use models::ModelEntry;
pub use reasoning::{ReasoningEffort, ReasoningSupport, ThinkingMode};
pub use settings::{Settings, ThinkingSettings};

/// One message in a chat-completion request.
///
/// The base shape is `{role, content}`; two optional fields carry the
/// tool-calling turn (see `docs/tools.md`), both `skip_serializing_if`-guarded
/// so an ordinary system/user/assistant message serializes byte-for-byte as
/// before:
/// - `tool_calls` — on an **assistant** message, the function calls the model
///   requested (echoed back verbatim so the provider can pair the results);
/// - `tool_call_id` — on a **`role:"tool"`** message, which call this output
///   answers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// One entry of an assistant message's `tool_calls` array — the Chat
/// Completions `{"id","type":"function","function":{"name","arguments"}}`
/// shape. `arguments` is the raw JSON string the model emitted.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ToolCallSpec {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

/// The `function` object of a [`ToolCallSpec`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

impl ToolCallSpec {
    /// A function tool call echoed back to the provider.
    #[must_use]
    pub fn function(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }
}

/// A chat message's content: the classic plain string, or the multimodal
/// parts array a vision request uses (text + `data:`-URL images). `untagged`
/// so `Text` serializes as a bare JSON string — the shape every
/// OpenAI-compatible endpoint accepts — and only image-carrying messages pay
/// the array form. See `docs/context.md`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

/// One element of a multimodal content array — OpenAI's
/// `{"type": "text", …}` / `{"type": "image_url", …}` shape.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

/// The `image_url` object of an image part. `url` is a base64 `data:` URL —
/// the attachment is embedded, never fetched.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImageUrl {
    pub url: String,
}

impl ContentPart {
    #[must_use]
    pub fn text(t: impl Into<String>) -> Self {
        Self::Text { text: t.into() }
    }

    #[must_use]
    pub fn image(url: impl Into<String>) -> Self {
        Self::ImageUrl {
            image_url: ImageUrl { url: url.into() },
        }
    }
}

impl ChatMessage {
    #[must_use]
    pub fn new(role: &str, c: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: MessageContent::Text(c.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// A multimodal message — a text part plus image parts (vision).
    #[must_use]
    pub fn with_parts(role: &str, parts: Vec<ContentPart>) -> Self {
        Self {
            role: role.into(),
            content: MessageContent::Parts(parts),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// An assistant message that requested tool calls (its `content` may be
    /// empty when the model only called tools). Echoed back so the provider
    /// can pair each `role:"tool"` result to its call (see `docs/tools.md`).
    #[must_use]
    pub fn assistant_tool_calls(text: impl Into<String>, tool_calls: Vec<ToolCallSpec>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: MessageContent::Text(text.into()),
            tool_calls,
            tool_call_id: None,
        }
    }

    /// A `role:"tool"` result message answering the call `tool_call_id`.
    #[must_use]
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: MessageContent::Text(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    #[must_use]
    pub fn system(c: impl Into<String>) -> Self {
        Self::new("system", c)
    }

    #[must_use]
    pub fn user(c: impl Into<String>) -> Self {
        Self::new("user", c)
    }

    #[must_use]
    pub fn assistant(c: impl Into<String>) -> Self {
        Self::new("assistant", c)
    }
}

/// A failure talking to a provider. [`std::fmt::Display`] gives the user-facing
/// message surfaced as a red `StreamEvent::Error`.
#[derive(Debug)]
pub enum LlmError {
    /// A transport/connection failure (DNS, TLS, proxy, timeout).
    Http(String),
    /// A non-2xx HTTP response, with the provider's error body.
    Api { status: u16, body: String },
    /// The response body didn't match the expected shape.
    Decode(String),
    /// The request was cancelled (the user interrupted or closed the picker).
    Cancelled,
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "request failed: {e}"),
            Self::Api { status, body } => {
                let body = body.trim();
                if body.is_empty() {
                    write!(f, "provider returned HTTP {status}")
                } else {
                    // Keep the surfaced error to one tidy line.
                    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
                    let flat = truncate_chars(&flat, 300);
                    write!(f, "HTTP {status}: {flat}")
                }
            }
            Self::Decode(e) => write!(f, "could not decode the response: {e}"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for LlmError {}

/// The module's result alias.
pub type Result<T> = std::result::Result<T, LlmError>;

/// Truncate to at most `max` chars, adding an ellipsis when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…")
}

/// Build a blocking HTTP client: env proxies (`HTTPS_PROXY`) are picked up
/// automatically; the agent proxy's custom CA is added from `SSL_CERT_FILE` /
/// `ALTER_ZERO_CA_FILE` so `rustls` trusts it. A short connect timeout bounds a
/// hung connect (DNS + TCP + TLS — a blackholed host fails in seconds, not
/// after the full per-operation deadline).
///
/// `op_timeout` is applied via [`reqwest::blocking::ClientBuilder::timeout`],
/// which in the blocking client is a **per-operation** deadline — it bounds the
/// send/header exchange *and* each individual body `read()` (a fresh deadline
/// per read), **not** the total request. It is purely a **stall detector**: an
/// operation that sits longer than this is treated as failed and handed to the
/// retry policy. Cancellation does *not* ride on it — the streaming path runs
/// its blocking operations on a detached transport thread and polls the
/// `CancelToken` while receiving from it (`openai::drain_stream`), so
/// `op_timeout` can be generous (a vision request uploads megabytes of base64)
/// without costing interrupt latency.
///
/// # Errors
/// Returns [`LlmError::Http`] if the client can't be built.
///
/// Clients are cached per `op_timeout` for the life of the process (a
/// `reqwest` client is an `Arc` handle): building one per attempt re-read and
/// re-parsed the CA bundle every turn and — because each fresh client starts
/// an empty connection pool — paid a full TLS handshake per request, defeating
/// the `pool_idle_timeout` set here. The env config a client bakes in (proxy,
/// CA paths) never changes mid-process (this crate forbids `set_var`).
pub(crate) fn http_client(op_timeout: Duration) -> Result<reqwest::blocking::Client> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CLIENTS: OnceLock<Mutex<HashMap<Duration, reqwest::blocking::Client>>> = OnceLock::new();
    let cache = CLIENTS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(map) = cache.lock()
        && let Some(client) = map.get(&op_timeout)
    {
        return Ok(client.clone());
    }
    let mut builder = reqwest::blocking::Client::builder()
        .pool_idle_timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .timeout(op_timeout);
    for cert in extra_root_certificates() {
        builder = builder.add_root_certificate(cert);
    }
    let client = builder.build().map_err(|e| LlmError::Http(e.to_string()))?;
    if let Ok(mut map) = cache.lock() {
        map.insert(op_timeout, client.clone());
    }
    Ok(client)
}

/// Extra trust roots loaded from `ALTER_ZERO_CA_FILE` or `SSL_CERT_FILE` — the
/// agent proxy's CA bundle, so real calls work behind it. Best-effort: an
/// unreadable or malformed file yields no extra roots (the built-in Mozilla
/// roots still apply). Boundary code — reads the environment and the filesystem.
fn extra_root_certificates() -> Vec<reqwest::Certificate> {
    let Some(path) =
        std::env::var_os("ALTER_ZERO_CA_FILE").or_else(|| std::env::var_os("SSL_CERT_FILE"))
    else {
        return Vec::new();
    };
    let Ok(pem) = std::fs::read(&path) else {
        return Vec::new();
    };
    reqwest::Certificate::from_pem_bundle(&pem).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_constructors_set_role() {
        assert_eq!(ChatMessage::system("a").role, "system");
        assert_eq!(ChatMessage::user("a").role, "user");
        assert_eq!(ChatMessage::assistant("a").role, "assistant");
    }

    #[test]
    fn text_content_serializes_as_a_bare_string() {
        // The classic chat-completions shape: `"content": "hi"` — every
        // OpenAI-compatible endpoint accepts it, so imageless messages (the
        // overwhelmingly common case) never pay the parts-array form.
        let json = serde_json::to_value(ChatMessage::user("hi")).unwrap();
        assert_eq!(json, serde_json::json!({"role": "user", "content": "hi"}));
    }

    #[test]
    fn parts_content_serializes_as_the_openai_multimodal_array() {
        let msg = ChatMessage::with_parts(
            "user",
            vec![
                ContentPart::text("what is this?"),
                ContentPart::image("data:image/png;base64,AAAA"),
            ],
        );
        let json = serde_json::to_value(msg).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "what is this?"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}},
                ],
            })
        );
    }

    #[test]
    fn assistant_tool_call_message_serializes_the_tool_calls_array() {
        let msg = ChatMessage::assistant_tool_calls(
            "",
            vec![ToolCallSpec::function(
                "call_1",
                "bash",
                r#"{"command":"ls"}"#,
            )],
        );
        let json = serde_json::to_value(msg).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "bash", "arguments": "{\"command\":\"ls\"}"}
                }]
            })
        );
    }

    #[test]
    fn tool_result_message_carries_the_call_id() {
        let msg = ChatMessage::tool_result("call_1", "hello\n");
        let json = serde_json::to_value(msg).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "role": "tool",
                "content": "hello\n",
                "tool_call_id": "call_1"
            })
        );
    }

    #[test]
    fn a_plain_message_omits_the_tool_fields() {
        // The skip_serializing_if guards mean ordinary messages are unchanged.
        let json = serde_json::to_value(ChatMessage::user("hi")).unwrap();
        assert_eq!(json, serde_json::json!({"role": "user", "content": "hi"}));
        assert!(json.get("tool_calls").is_none());
        assert!(json.get("tool_call_id").is_none());
    }

    #[test]
    fn api_error_flattens_and_trims_the_body() {
        let e = LlmError::Api {
            status: 401,
            body: "  {\n  \"error\": \"bad key\"\n}  ".to_string(),
        };
        let shown = e.to_string();
        assert!(shown.starts_with("HTTP 401: "));
        assert!(!shown.contains('\n'), "flattened to one line");
        assert!(shown.contains("bad key"));
    }

    #[test]
    fn api_error_with_empty_body_reports_the_status() {
        let e = LlmError::Api {
            status: 500,
            body: String::new(),
        };
        assert_eq!(e.to_string(), "provider returned HTTP 500");
    }

    #[test]
    fn truncate_chars_adds_an_ellipsis_when_cut() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 3), "hel…");
    }
}
