//! Ollama's **native** chat API as a wire format (`wire_api = "ollama"`) —
//! the fourth after Chat Completions, Responses and Messages. See
//! `docs/ollama.md`.
//!
//! Ollama also speaks an OpenAI-compatible `/v1`, and this module exists
//! because of the one thing that endpoint cannot carry: `options.num_ctx`,
//! the context window a model is loaded with. Its server default is 4096
//! tokens on most machines, and a prompt past it is **silently** truncated
//! from the front — the system prompt and the tools survive, the
//! conversation does not. The native wire sets the window per request, so
//! the number the footer gauges against is the number the server holds.
//!
//! Everything here is a **translation**, in both directions, between the
//! native shape and the Chat Completions currency the rest of the crate
//! uses ([`ChatMessage`] in, [`Delta`] / `StreamOutcome` out) — the same
//! contract [`super::responses`] and [`super::anthropic`] keep. Five shapes
//! differ:
//!
//! - **The stream is NDJSON**, one JSON object per line, not SSE.
//! - **A tool call arrives whole** on one frame, its `arguments` a JSON
//!   **object** — and must be sent back as one: a JSON string there is a 400.
//! - **An image is bare base64** in an `images` array on the message, not a
//!   `data:` URL part.
//! - **Thinking is a field** (`message.thinking`) with a request-side
//!   `think` that is a bool — or, for gpt-oss, a level string.
//! - **A tool result names its tool** (`tool_name`), beside the call id.
//!
//! Pure and unit-tested — the HTTP lives in [`super::openai`] (the chat
//! stream) and [`super::models`] (the catalog fetch).

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::Deserialize;
use serde_json::{Value, json};

use super::config::ModelConfig;
use super::models::ModelEntry;
use super::openai::Delta;
use super::reasoning::{ReasoningEffort, ReasoningSupport, ThinkingMode};
use super::thinking::{ChatStreamResult, ThinkingSplitter};
use super::tools::ToolCallRequest;
use super::{ChatMessage, ContentPart, LlmError, MessageContent, Result};
use crate::stream::TokenUsage;

/// Where a local server listens when nothing says otherwise — Ollama's own
/// default, and what the `/login` host field offers.
pub const DEFAULT_HOST: &str = "http://127.0.0.1:11434";

/// Ollama's default port, for a host given without one.
const DEFAULT_PORT: &str = "11434";

/// The most `options.num_ctx` a request asks for when nothing names a
/// window — neither a Modelfile `num_ctx` nor `OLLAMA_CONTEXT_LENGTH` —
/// clamped to what the model can hold. A bare install serves 4096, which
/// an agentic turn overflows on its first file read; the model's own
/// maximum (128K, 256K) is what OOMs a GPU. 32K is Ollama's own middle
/// VRAM tier and Cline's default, and it is raised by
/// `ALTER_ZERO_CONTEXT_WINDOW`. See `docs/ollama.md`.
pub const DEFAULT_NUM_CTX_CAP: u64 = 32_768;

/// The server-side default's own variable, read from this process's
/// environment as a mirror of what `ollama serve` was started with.
pub const CONTEXT_LENGTH_ENV: &str = "OLLAMA_CONTEXT_LENGTH";

/// The model family whose thinking takes a **level** rather than a switch.
const LEVELLED_FAMILY: &str = "gptoss";

// ---------------------------------------------------------------------------
// The host
// ---------------------------------------------------------------------------

/// A base URL out of an `OLLAMA_HOST`-style value, in Ollama's own grammar
/// (`envconfig.Host()`): no scheme means `http`, no port means 11434 — but
/// an explicit scheme brings its own default port — the bare `ollama.com`
/// is the cloud over https, a path survives, an invalid port falls back to
/// the default, and an unspecified bind address (`0.0.0.0`, `::`) becomes
/// the loopback a client can actually connect to. Empty is the local
/// default. A full URL comes out unchanged, so the file's own base takes
/// the same road as an override.
#[must_use]
pub fn host_url(raw: &str) -> String {
    let raw = raw.trim();
    let mut default_port = DEFAULT_PORT;
    let (scheme, rest) = match raw.split_once("://") {
        None if raw == "ollama.com" => ("https", "ollama.com:443"),
        None => ("http", raw),
        Some(("http", rest)) => {
            default_port = "80";
            ("http", rest)
        }
        Some(("https", rest)) => {
            default_port = "443";
            ("https", rest)
        }
        Some((scheme, rest)) => (scheme, rest),
    };
    let (hostport, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = match split_host_port(hostport) {
        Some((host, port)) => (host.to_string(), port.to_string()),
        None => {
            let bare = hostport.trim_matches(|c| c == '[' || c == ']');
            let host = if hostport.is_empty() {
                "127.0.0.1".to_string()
            } else if bare.parse::<IpAddr>().is_ok() {
                bare.to_string()
            } else {
                hostport.to_string()
            };
            (host, default_port.to_string())
        }
    };
    let port = match port.parse::<u32>() {
        Ok(n) if n <= 65_535 => port,
        _ => default_port.to_string(),
    };
    let host = match host.as_str() {
        "0.0.0.0" => "127.0.0.1".to_string(),
        "::" => "::1".to_string(),
        _ => host,
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        format!("{scheme}://{host}:{port}")
    } else {
        format!("{scheme}://{host}:{port}/{path}")
    }
}

/// Go's `net.SplitHostPort`: `host:port`, or `[v6]:port`. `None` when there
/// is no port to split off (or too many colons for an unbracketed host).
fn split_host_port(hostport: &str) -> Option<(&str, &str)> {
    if let Some(rest) = hostport.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        return Some((host, after.strip_prefix(':')?));
    }
    let (host, port) = hostport.rsplit_once(':')?;
    (!host.contains(':')).then_some((host, port))
}

/// Is this base the hosted service? The cloud serves a model at its
/// maximum window, so nothing caps it there.
#[must_use]
pub fn is_cloud_host(base: &str) -> bool {
    base.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split(['/', ':'])
        .next()
        .is_some_and(|host| host.eq_ignore_ascii_case("ollama.com"))
}

// ---------------------------------------------------------------------------
// The request body
// ---------------------------------------------------------------------------

/// Build the streamed `/api/chat` body for `messages` — as a JSON tree, the
/// tests' view of the request. The wire streams a [`RequestSource`], which
/// writes the same request from the messages **by reference**.
///
/// The window rides as `options.num_ctx` and the temperature beside it; the
/// tool specs pass through untouched (Ollama's schema *is* the Chat
/// Completions one, minus `tool_choice`); the thinking mode becomes
/// [`think_value`]. A file-configured `options` table merges **under** the
/// session's own keys, so a `num_gpu` in `providers.toml` rides along
/// without a stale `num_ctx` beside it overriding the gauge.
#[must_use]
pub fn build_payload(cfg: &ModelConfig, tools: &[Value], messages: &[ChatMessage]) -> Value {
    serde_json::to_value(request(cfg, tools, messages)).unwrap_or(Value::Null)
}

/// The `/api/chat` request as it is serialized: the `messages` **borrowed
/// from the conversation** — a picture's base64 is a slice of the session's
/// one shared encoding, never a copy into a tree (`docs/memory.md`) — beside
/// every other field in one small map.
#[derive(serde::Serialize)]
struct Request<'a> {
    #[serde(flatten)]
    fields: serde_json::Map<String, Value>,
    messages: Vec<Message<'a>>,
}

fn request<'a>(cfg: &ModelConfig, tools: &[Value], messages: &'a [ChatMessage]) -> Request<'a> {
    let mut fields = serde_json::Map::new();
    fields.insert("model".to_string(), json!(cfg.model));
    fields.insert("stream".to_string(), json!(true));
    if !tools.is_empty() {
        fields.insert("tools".to_string(), json!(tools));
    }
    if let Some(mode) = cfg.thinking {
        fields.insert("think".to_string(), think_value(mode));
    }
    let mut options = serde_json::Map::new();
    if let Some(window) = cfg.context {
        options.insert("num_ctx".to_string(), json!(window));
    }
    if let Some(temperature) = cfg.temperature {
        options.insert("temperature".to_string(), json!(temperature));
    }
    for (key, value) in &cfg.extra_body {
        if key == "options" {
            if let Some(table) = value.as_object() {
                for (k, v) in table {
                    options.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            continue;
        }
        fields.insert(key.clone(), value.clone());
    }
    if !options.is_empty() {
        fields.insert("options".to_string(), Value::Object(options));
    }
    Request {
        fields,
        messages: messages_of(messages),
    }
}

/// The request as owned data the serializer thread writes from
/// ([`super::body::BodySource`]): the config, the tool specs and the round's
/// messages, their pictures shared.
pub struct RequestSource {
    cfg: ModelConfig,
    tools: Vec<Value>,
    messages: Vec<ChatMessage>,
}

impl RequestSource {
    #[must_use]
    pub fn new(cfg: ModelConfig, tools: Vec<Value>, messages: Vec<ChatMessage>) -> Self {
        Self {
            cfg,
            tools,
            messages,
        }
    }
}

impl super::body::BodySource for RequestSource {
    fn write_to(&self, out: &mut dyn std::io::Write) -> serde_json::Result<()> {
        serde_json::to_writer(out, &request(&self.cfg, &self.tools, &self.messages))
    }
}

/// The request's `think` field for a mode: a bool for the on/off reasoners
/// (`think: false` is what stops a qwen3 or deepseek-r1 from thinking, and
/// the field omitted means the model's default, which is *on*), a level
/// string for gpt-oss, whose thinking is a depth rather than a switch.
#[must_use]
pub fn think_value(mode: ThinkingMode) -> Value {
    match mode {
        ThinkingMode::Off => json!(false),
        ThinkingMode::On => json!(true),
        ThinkingMode::Effort(effort) => json!(think_level(effort)),
    }
}

/// Ollama's three thinking levels, with the crate's wider ladder clamped
/// onto them the way Ollama's own OpenAI layer clamps `reasoning_effort`:
/// `minimal` is `low`, and everything above `high` is `high`.
#[must_use]
pub const fn think_level(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal | ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High
        | ReasoningEffort::XHigh
        | ReasoningEffort::Max
        | ReasoningEffort::Ultra => "high",
    }
}

/// The `messages` array as JSON trees — the tests' view of `messages_of`.
#[must_use]
pub fn build_messages(messages: &[ChatMessage]) -> Vec<Value> {
    messages_of(messages)
        .iter()
        .map(|message| serde_json::to_value(message).unwrap_or(Value::Null))
        .collect()
}

/// One `messages` entry, borrowed from the conversation: the role and text
/// every message has, the call id and tool name a result names, the calls
/// an assistant made, and the bare base64 of a user message's pictures.
#[derive(Debug, serde::Serialize)]
struct Message<'a> {
    role: &'a str,
    content: Cow<'a, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<&'a str>,
}

impl<'a> Message<'a> {
    fn plain(role: &'a str, content: Cow<'a, str>) -> Self {
        Self {
            role,
            content,
            tool_call_id: None,
            tool_name: None,
            tool_calls: Vec::new(),
            images: Vec::new(),
        }
    }
}

/// The `messages` array. System and user messages keep their roles; an
/// image part becomes the message's `images` array of bare base64; an
/// assistant call carries its `arguments` as an **object** (a string is a
/// 400 here); and a tool result names the tool it answers, read off the
/// assistant call seen earlier in the same list.
fn messages_of(messages: &[ChatMessage]) -> Vec<Message<'_>> {
    let mut names: BTreeMap<&str, &str> = BTreeMap::new();
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        out.push(match message.role.as_str() {
            "system" | "developer" => Message::plain("system", flatten_text(&message.content)),
            "tool" => {
                let id = message.tool_call_id.as_deref();
                Message {
                    tool_call_id: id,
                    tool_name: id.and_then(|id| names.get(id).copied()),
                    ..Message::plain("tool", flatten_text(&message.content))
                }
            }
            "assistant" => {
                let calls: Vec<Value> = message
                    .tool_calls
                    .iter()
                    .map(|call| {
                        names.insert(&call.id, &call.function.name);
                        tool_call_value(call)
                    })
                    .collect();
                Message {
                    tool_calls: calls,
                    ..Message::plain("assistant", flatten_text(&message.content))
                }
            }
            role => user_message(role, &message.content),
        });
    }
    out
}

/// One assistant `tool_calls` entry. The call's `arguments` is a JSON string
/// in the Chat Completions currency and must arrive here as an object; one
/// that doesn't parse (a call the model truncated) degrades to `{}` rather
/// than failing the whole request.
fn tool_call_value(call: &super::ToolCallSpec) -> Value {
    let arguments = serde_json::from_str::<Value>(&call.function.arguments)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let mut out = json!({"function": {"name": call.function.name, "arguments": arguments}});
    if !call.id.is_empty() {
        out["id"] = json!(call.id);
    }
    out
}

/// A user-side message: plain text as-is; a parts message flattened to its
/// text with the images split off into `images` — the payload of each
/// `data:` URL, no prefix, no media type, a slice of the shared encoding. A
/// URL that is not a `data:` one has no pixels to send and is dropped rather
/// than 400ing the turn.
fn user_message<'a>(role: &'a str, content: &'a MessageContent) -> Message<'a> {
    match content {
        MessageContent::Text(text) => Message::plain(role, Cow::Borrowed(text.as_str())),
        MessageContent::Parts(parts) => Message {
            images: parts
                .iter()
                .filter_map(|part| match part {
                    ContentPart::ImageUrl { image_url } => base64_payload(&image_url.url),
                    ContentPart::Text { .. } => None,
                })
                .collect(),
            ..Message::plain(role, flatten_text(content))
        },
    }
}

/// The base64 body of a `data:` URL, or `None` for anything else.
fn base64_payload(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("data:")?;
    let (_, payload) = rest.split_once(";base64,")?;
    (!payload.is_empty()).then_some(payload)
}

/// A message's text, with any image parts dropped. Borrowed when the
/// content is one string, joined (and so owned) when it is parts.
fn flatten_text(content: &MessageContent) -> Cow<'_, str> {
    match content {
        MessageContent::Text(text) => Cow::Borrowed(text.as_str()),
        MessageContent::Parts(parts) => Cow::Owned(
            parts
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text, .. } => Some(text.as_str()),
                    ContentPart::ImageUrl { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    }
}

// ---------------------------------------------------------------------------
// The stream
// ---------------------------------------------------------------------------

/// What one NDJSON line did to the round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Text to surface (reply and/or thinking), or a tool call to count.
    Delta(Delta),
    /// Nothing this caller needs (an empty frame, a line that isn't JSON).
    Continue,
    /// The `done` frame: the round is over.
    Done,
    /// The server failed the response in-band (`{"error": …}` on a 200).
    Failed(String),
}

/// Folds a `/api/chat` NDJSON stream into the crate's `StreamOutcome`
/// currency: the reply and thinking text (through the same
/// [`ThinkingSplitter`] the Chat Completions wire runs, since a custom GGUF
/// import can lack the parser that fills `thinking` and emit `<think>` tags
/// in the content instead), the tool calls — each whole on the frame it
/// lands on, its object arguments re-serialized to the string the executor
/// parses, its id kept or minted — and the usage the final frame carries.
#[derive(Debug)]
pub struct ChatAccumulator {
    splitter: ThinkingSplitter,
    calls: Vec<ToolCallRequest>,
    done_reason: Option<String>,
    usage: Option<TokenUsage>,
    finished: bool,
}

impl Default for ChatAccumulator {
    fn default() -> Self {
        Self {
            splitter: ThinkingSplitter::new(),
            calls: Vec::new(),
            done_reason: None,
            usage: None,
            finished: false,
        }
    }
}

/// One streamed frame, decoded to the fields this fold reads.
#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(default)]
    message: Option<FrameMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
struct FrameMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<FrameToolCall>>,
}

#[derive(Debug, Deserialize)]
struct FrameToolCall {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: FrameFunction,
}

#[derive(Debug, Default, Deserialize)]
struct FrameFunction {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: Value,
}

impl ChatAccumulator {
    /// Fold one line in.
    pub fn on_line(&mut self, line: &str) -> Step {
        let line = line.trim();
        if line.is_empty() {
            return Step::Continue;
        }
        let Ok(frame) = serde_json::from_str::<Frame>(line) else {
            return Step::Continue;
        };
        if let Some(error) = frame.error {
            return Step::Failed(error_text(&error));
        }
        let mut delta = Delta::default();
        if let Some(message) = frame.message {
            let content = message.content.unwrap_or_default();
            let thinking = message.thinking.unwrap_or_default();
            let (response, reasoning) = self.splitter.feed(&content, &thinking);
            delta.response = response;
            delta.reasoning = reasoning;
            for call in message.tool_calls.unwrap_or_default() {
                if call.function.name.is_empty() {
                    continue;
                }
                let arguments = match call.function.arguments {
                    Value::Null => "{}".to_string(),
                    Value::String(text) => text,
                    other => other.to_string(),
                };
                let id = call
                    .id
                    .filter(|id| !id.is_empty())
                    .unwrap_or_else(|| format!("call_{}", self.calls.len()));
                // Counted as it lands, never rendered
                // (`docs/status-indicator.md`).
                delta.tool_call.push_str(&call.function.name);
                delta.tool_call.push_str(&arguments);
                self.calls.push(ToolCallRequest {
                    id,
                    name: call.function.name,
                    arguments,
                });
            }
        }
        if frame.done {
            self.finished = true;
            self.done_reason = frame.done_reason;
            let input = frame.prompt_eval_count.unwrap_or(0);
            let output = frame.eval_count.unwrap_or(0);
            if input > 0 || output > 0 {
                self.usage = Some(TokenUsage {
                    input,
                    output,
                    ..TokenUsage::default()
                });
            }
        }
        if delta.response.is_empty() && delta.reasoning.is_empty() && delta.tool_call.is_empty() {
            if self.finished {
                return Step::Done;
            }
            return Step::Continue;
        }
        Step::Delta(delta)
    }

    /// Has the `done` frame landed? A final frame that also carried text
    /// surfaces as a [`Step::Delta`], so the drain asks this beside it.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finished
    }

    /// Any text the splitter held back mid-tag at the end of the stream.
    pub fn flush(&mut self) -> Option<Delta> {
        let (response, reasoning) = self.splitter.flush();
        (!response.is_empty() || !reasoning.is_empty()).then_some(Delta {
            response,
            reasoning,
            tool_call: String::new(),
        })
    }

    /// The accumulated reply and thinking text so far.
    #[must_use]
    pub fn text(&self) -> ChatStreamResult {
        ChatStreamResult {
            response: self.splitter.response().to_string(),
            reasoning: self.splitter.reasoning().to_string(),
        }
    }

    /// What the round produced: its tool calls, the `finish_reason` the rest
    /// of the crate speaks, and the server's own usage. A round that asked
    /// for tools finished *for* those tools, whatever `done_reason` said —
    /// Ollama reports `stop` for one — which is what the agent loop reads.
    #[must_use]
    pub fn finish(self) -> (Vec<ToolCallRequest>, Option<String>, Option<TokenUsage>) {
        let reason = if self.calls.is_empty() {
            self.done_reason
        } else {
            Some("tool_calls".to_string())
        };
        (self.calls, reason, self.usage)
    }
}

/// An `error` value's message: the string it usually is, else the whole
/// value.
fn error_text(error: &Value) -> String {
    error
        .as_str()
        .map_or_else(|| error.to_string(), str::to_string)
}

// ---------------------------------------------------------------------------
// The catalog
// ---------------------------------------------------------------------------

/// One model as the server describes it — from a `/api/tags` record, filled
/// in (or replaced) by its `/api/show`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRecord {
    /// The name a request uses (`qwen3:8b`).
    pub name: String,
    /// `details.family` (`qwen3`, `gptoss`, …) — what tells a levelled
    /// reasoner from a switched one.
    pub family: String,
    /// The `capabilities` list (`completion`, `tools`, `vision`,
    /// `thinking`, `embedding`, …), or `None` when the server never said —
    /// an older release, or a record its `/api/show` failed for.
    pub capabilities: Option<Vec<String>>,
    /// The model's own maximum window: `details.context_length`, or the
    /// architecture's `model_info` key.
    pub context_length: Option<u64>,
    /// A `num_ctx` the model's Modelfile pins — the user's own per-model
    /// choice, read off `/api/show`'s `parameters`.
    pub modelfile_num_ctx: Option<u64>,
    /// A `-cloud` model a local server forwards to ollama.com: nothing is
    /// loaded locally, so the window rule leaves it at the cloud's maximum.
    pub remote: bool,
}

/// The `/api/tags` envelope. Each record stays an unparsed slice of the
/// body and is decoded one at a time (the `parse_models` posture,
/// `docs/memory.md`).
#[derive(Debug, Deserialize)]
struct TagsResponse<'a> {
    #[serde(default, borrow)]
    models: Vec<&'a serde_json::value::RawValue>,
}

#[derive(Debug, Deserialize)]
struct TagRecord {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    remote_model: Option<String>,
    #[serde(default)]
    remote_host: Option<String>,
    #[serde(default)]
    details: Option<Details>,
    #[serde(default)]
    capabilities: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct Details {
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    context_length: Option<u64>,
}

/// Parse a `/api/tags` body into records. A record without a usable name is
/// skipped, never fatal.
///
/// # Errors
/// A decode error when the body isn't the envelope.
pub fn tags_records(body: &str) -> Result<Vec<CatalogRecord>> {
    let parsed: TagsResponse =
        serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
    Ok(parsed
        .models
        .iter()
        .filter_map(|raw| {
            let record: TagRecord = serde_json::from_str(raw.get()).ok()?;
            let name = record
                .model
                .or(record.name)
                .filter(|name| !name.is_empty())?;
            let details = record.details.unwrap_or_default();
            Some(CatalogRecord {
                name,
                family: details.family.unwrap_or_default(),
                capabilities: record.capabilities,
                context_length: details.context_length.filter(|&n| n > 0),
                modelfile_num_ctx: None,
                remote: record.remote_model.is_some_and(|m| !m.is_empty())
                    || record.remote_host.is_some_and(|h| !h.is_empty()),
            })
        })
        .collect())
}

/// The `/api/show` body, decoded to what the record needs. `model_info` is
/// a flat map of scalars (the tokenizer arrays are null unless `verbose`),
/// and the `tensors` list beside it is skipped unread.
#[derive(Debug, Deserialize)]
struct ShowResponse {
    #[serde(default)]
    parameters: Option<String>,
    #[serde(default)]
    details: Option<Details>,
    #[serde(default)]
    model_info: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    capabilities: Option<Vec<String>>,
    #[serde(default)]
    remote_model: Option<String>,
    #[serde(default)]
    remote_host: Option<String>,
}

/// The record a `/api/show` body describes, for the model `name`.
///
/// # Errors
/// A decode error when the body isn't a show response.
pub fn show_record(name: &str, body: &str) -> Result<CatalogRecord> {
    let show: ShowResponse =
        serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
    let details = show.details.unwrap_or_default();
    let info = show.model_info.unwrap_or_default();
    // The window lives under an architecture-prefixed key
    // (`qwen3.context_length`); the architecture names which, and any
    // `.context_length` key stands in when it doesn't.
    let arch_key = info
        .get("general.architecture")
        .and_then(Value::as_str)
        .map(|arch| format!("{arch}.context_length"));
    let context_length = arch_key
        .as_deref()
        .and_then(|key| info.get(key))
        .or_else(|| {
            info.iter()
                .find(|(key, _)| key.ends_with(".context_length"))
                .map(|(_, value)| value)
        })
        .and_then(Value::as_u64)
        .or(details.context_length)
        .filter(|&n| n > 0);
    Ok(CatalogRecord {
        name: name.to_string(),
        family: details.family.unwrap_or_default(),
        capabilities: show.capabilities,
        context_length,
        modelfile_num_ctx: show.parameters.as_deref().and_then(modelfile_num_ctx),
        remote: show.remote_model.is_some_and(|m| !m.is_empty())
            || show.remote_host.is_some_and(|h| !h.is_empty()),
    })
}

/// The `num_ctx` line of a Modelfile `parameters` block — one `name value`
/// pair per line, whitespace-aligned.
fn modelfile_num_ctx(parameters: &str) -> Option<u64> {
    parameters.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next()? == "num_ctx")
            .then(|| words.next()?.parse::<u64>().ok())
            .flatten()
            .filter(|&n| n > 0)
    })
}

/// The picker row for a record, or `None` for a model with no chat surface
/// (an embedding model: it lists, and fails every turn).
///
/// The capabilities list is explicit, so absence *is* the answer — a record
/// with the list and no `vision` cannot see; one with no list at all stays
/// unknown, the optimistic default every other provider gets. `thinking`
/// makes an on/off reasoner, except for gpt-oss, whose thinking is a
/// **level** it cannot turn off. The window is [`context_window`]'s.
#[must_use]
pub fn entry_of(
    record: &CatalogRecord,
    provider: &str,
    server_default: Option<u64>,
    uncapped: bool,
) -> Option<ModelEntry> {
    let has = |capability: &str| {
        record
            .capabilities
            .as_ref()
            .map(|caps| caps.iter().any(|c| c == capability))
    };
    if has("completion") == Some(false) {
        return None;
    }
    let reasoning = (has("thinking") == Some(true)).then(|| {
        if record.family == LEVELLED_FAMILY {
            ReasoningSupport {
                efforts: vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ],
                can_disable: false,
                default_effort: None,
            }
        } else {
            ReasoningSupport {
                efforts: Vec::new(),
                can_disable: true,
                default_effort: None,
            }
        }
    });
    Some(ModelEntry {
        id: record.name.clone(),
        provider: provider.to_string(),
        display_name: record.name.clone(),
        reasoning,
        vision: has("vision"),
        context: context_window(record, server_default, uncapped),
        // Ollama's `/api/show` names no speed tier (`docs/fast-mode.md`).
        service_tiers: Vec::new(),
    })
}

/// The window a session gauges against — and, on this wire, asks the
/// server for. In order: the Modelfile's own `num_ctx` (the user's per-model
/// choice), the server's configured default (`OLLAMA_CONTEXT_LENGTH`), else
/// [`DEFAULT_NUM_CTX_CAP`] — never past what the model can hold, since the
/// server clamps there anyway and a gauge against more would lie. A model
/// whose maximum is unknown takes the first two or nothing: inventing a
/// window for it would gauge against a number nobody stated. The cloud
/// (reached directly, or through a local server's `-cloud` proxy) serves a
/// model at its maximum, so nothing caps it there.
#[must_use]
pub fn context_window(
    record: &CatalogRecord,
    server_default: Option<u64>,
    uncapped: bool,
) -> Option<u64> {
    let max = record.context_length.filter(|&n| n > 0);
    if uncapped || record.remote {
        return max;
    }
    let named = record
        .modelfile_num_ctx
        .or(server_default)
        .filter(|&n| n > 0);
    match max {
        Some(max) => Some(named.unwrap_or(DEFAULT_NUM_CTX_CAP).min(max)),
        None => named,
    }
}

// ---------------------------------------------------------------------------
// The errors worth explaining
// ---------------------------------------------------------------------------

/// Rewrite an HTTP refusal a user can act on: a model not pulled, a
/// capability the model lacks, a cloud key missing. `None` keeps the
/// server's own body, which for everything else is the best account.
#[must_use]
pub fn advice(status: u16, body: &str, model: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    match status {
        404 if lower.contains("not found") => Some(format!(
            "Ollama has no model named {model} — pull it with `ollama pull {model}`, \
             or pick one it has with /model."
        )),
        400 if lower.contains("does not support tools") => Some(format!(
            "{model} does not support tool calling on Ollama — turn Tools off in \
             /settings, or pick a model that does with /model."
        )),
        400 if lower.contains("does not support thinking") => Some(format!(
            "{model} does not support thinking — press ctrl+t to turn it off, or \
             pick a model that does with /model."
        )),
        400 if lower.contains("multimodal") || lower.contains("does not support vision") => {
            Some(format!(
                "{model} cannot see images — Ollama refused the attachment. Pick a \
                 vision model with /model."
            ))
        }
        401 | 403 => Some(
            "Ollama refused the request as unauthorized — set OLLAMA_API_KEY to a key \
             from ollama.com/settings/keys (or paste one with /login), then try again."
                .to_string(),
        ),
        429 => {
            Some("Ollama is rate-limiting this account — wait a moment and try again.".to_string())
        }
        502 => Some(format!(
            "Ollama could not reach the cloud model behind {model} (502) — try again in \
             a moment."
        )),
        _ => None,
    }
}

/// The sentence for a transport failure that means the server is not there:
/// a refused connection is `ollama serve` not running (or the wrong host),
/// and a name that won't resolve is a mistyped `OLLAMA_HOST`. A timeout or a
/// dropped connection is neither, and keeps its own message.
#[must_use]
pub fn connection_advice(base: &str, error: &str) -> Option<String> {
    let lower = error.to_ascii_lowercase();
    if lower.contains("connection refused") {
        return Some(format!(
            "Could not reach Ollama at {base} — is `ollama serve` running? Point \
             OLLAMA_HOST at the server if it listens elsewhere."
        ));
    }
    if lower.contains("dns error") || lower.contains("failed to lookup") {
        return Some(format!(
            "Could not resolve the Ollama host in {base} — check OLLAMA_HOST."
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::config::{AuthScheme, ModelConfig, WireApi};
    use crate::llm::reasoning::{ReasoningEffort, ThinkingMode};
    use crate::llm::{ChatMessage, ContentPart, MessageContent, ToolCallSpec};
    use crate::stream::TokenUsage;
    use serde_json::json;

    // --- the host grammar (`OLLAMA_HOST`) -----------------------------------

    #[test]
    fn an_empty_host_is_the_local_default() {
        assert_eq!(host_url(""), "http://127.0.0.1:11434");
        assert_eq!(host_url("   "), "http://127.0.0.1:11434");
        assert_eq!(DEFAULT_HOST, "http://127.0.0.1:11434");
    }

    #[test]
    fn a_bare_host_gets_ollamas_scheme_and_port() {
        // Ollama's own `envconfig.Host()` grammar: no scheme means http, and
        // no port means 11434 — but an explicit scheme brings its own default
        // port, so `http://x` is port 80, not 11434.
        assert_eq!(host_url("myhost"), "http://myhost:11434");
        assert_eq!(host_url("localhost:11434"), "http://localhost:11434");
        assert_eq!(host_url("http://myhost"), "http://myhost:80");
        assert_eq!(host_url("https://myhost"), "https://myhost:443");
        assert_eq!(host_url("https://myhost:8443"), "https://myhost:8443");
    }

    #[test]
    fn the_cloud_shorthand_is_https() {
        // `OLLAMA_HOST=ollama.com` is how the CLI names the cloud, and the
        // grammar special-cases it onto https.
        assert_eq!(host_url("ollama.com"), "https://ollama.com:443");
        assert_eq!(host_url("https://ollama.com"), "https://ollama.com:443");
    }

    #[test]
    fn an_unspecified_bind_address_becomes_the_loopback() {
        // `0.0.0.0` is what a *server* binds; a client connecting to it fails
        // on some platforms, so Ollama's own client swaps in the loopback.
        assert_eq!(host_url("0.0.0.0"), "http://127.0.0.1:11434");
        assert_eq!(host_url("0.0.0.0:11434"), "http://127.0.0.1:11434");
        assert_eq!(host_url("[::]:11434"), "http://[::1]:11434");
        assert_eq!(host_url("[::1]:11434"), "http://[::1]:11434");
    }

    #[test]
    fn a_path_is_kept_and_a_trailing_slash_is_not() {
        assert_eq!(host_url("http://host:11434/"), "http://host:11434");
        assert_eq!(host_url("host:11434/ollama/"), "http://host:11434/ollama");
        assert_eq!(host_url(" http://host:11434 "), "http://host:11434");
    }

    #[test]
    fn an_invalid_port_falls_back_to_the_default() {
        assert_eq!(host_url("host:99999"), "http://host:11434");
        assert_eq!(host_url("host:abc"), "http://host:11434");
    }

    // --- the request body ---------------------------------------------------

    fn cfg() -> ModelConfig {
        let mut cfg = ModelConfig::fallback();
        cfg.provider_id = "ollama".to_string();
        cfg.model = "qwen3:8b".to_string();
        cfg.api_base = DEFAULT_HOST.to_string();
        cfg.api_model_base = DEFAULT_HOST.to_string();
        cfg.api_key = None;
        cfg.auth = AuthScheme::OptionalKey;
        cfg.wire_api = WireApi::Ollama;
        cfg
    }

    fn text(role: &str, body: &str) -> ChatMessage {
        ChatMessage::new(role, body)
    }

    #[test]
    fn a_payload_carries_the_native_chat_fields() {
        let payload = build_payload(&cfg(), &[], &[text("user", "hi")]);
        assert_eq!(payload["model"], "qwen3:8b");
        assert_eq!(payload["stream"], true);
        assert_eq!(
            payload["messages"],
            json!([{"role": "user", "content": "hi"}])
        );
        // Nothing to say about sampling or the window: no options object at
        // all, and no `think` — the model's own default stands.
        assert!(payload.get("options").is_none(), "{payload}");
        assert!(payload.get("think").is_none(), "{payload}");
        assert!(payload.get("tools").is_none(), "{payload}");
    }

    #[test]
    fn the_context_window_rides_as_num_ctx() {
        // The whole point of the native wire: Ollama serves a model at a
        // 4096-token context unless the request says otherwise, and the
        // window the footer gauges against must be the one the server holds.
        let mut cfg = cfg();
        cfg.context = Some(32_768);
        cfg.temperature = Some(0.5);
        let payload = build_payload(&cfg, &[], &[text("user", "hi")]);
        assert_eq!(payload["options"]["num_ctx"], 32_768);
        assert_eq!(payload["options"]["temperature"], 0.5);
    }

    #[test]
    fn tools_ride_verbatim_in_the_chat_completions_shape() {
        // Ollama's tool schema *is* the Chat Completions one, so the specs
        // pass through untouched.
        let tools = crate::llm::tools::tool_specs();
        let payload = build_payload(&cfg(), &tools, &[text("user", "hi")]);
        assert_eq!(payload["tools"], json!(tools));
        assert!(payload.get("tool_choice").is_none(), "not a field here");
    }

    #[test]
    fn think_maps_the_mode_onto_ollamas_field() {
        let with = |mode: ThinkingMode| {
            let mut cfg = cfg();
            cfg.thinking = Some(mode);
            build_payload(&cfg, &[], &[text("user", "hi")])["think"].clone()
        };
        assert_eq!(with(ThinkingMode::Off), json!(false));
        assert_eq!(with(ThinkingMode::On), json!(true));
        // gpt-oss takes a level string; the three it names.
        assert_eq!(
            with(ThinkingMode::Effort(ReasoningEffort::Low)),
            json!("low")
        );
        assert_eq!(
            with(ThinkingMode::Effort(ReasoningEffort::Medium)),
            json!("medium")
        );
        assert_eq!(
            with(ThinkingMode::Effort(ReasoningEffort::High)),
            json!("high")
        );
        // Rungs off Ollama's three-step ladder clamp to the nearest one it
        // has rather than sending a level the server would refuse.
        assert_eq!(
            with(ThinkingMode::Effort(ReasoningEffort::Minimal)),
            json!("low")
        );
        assert_eq!(
            with(ThinkingMode::Effort(ReasoningEffort::Max)),
            json!("high")
        );
        assert_eq!(
            with(ThinkingMode::Effort(ReasoningEffort::Ultra)),
            json!("high")
        );
    }

    #[test]
    fn provider_kwargs_merge_in_with_the_options_table_folded_under_ours() {
        // A file-configured `options` table (num_gpu, say) must not clobber
        // the session's own num_ctx/temperature — they merge, ours winning.
        let mut cfg = cfg();
        cfg.context = Some(8192);
        cfg.extra_body
            .insert("keep_alive".to_string(), json!("30m"));
        cfg.extra_body.insert(
            "options".to_string(),
            json!({"num_gpu": 1, "num_ctx": 2048}),
        );
        let payload = build_payload(&cfg, &[], &[text("user", "hi")]);
        assert_eq!(payload["keep_alive"], "30m");
        assert_eq!(payload["options"]["num_gpu"], 1);
        assert_eq!(
            payload["options"]["num_ctx"], 8192,
            "the session's window wins"
        );
    }

    // --- the messages -------------------------------------------------------

    #[test]
    fn system_and_user_messages_keep_their_roles() {
        let messages = build_messages(&[text("system", "be terse"), text("user", "hi")]);
        assert_eq!(
            messages,
            vec![
                json!({"role": "system", "content": "be terse"}),
                json!({"role": "user", "content": "hi"}),
            ]
        );
    }

    #[test]
    fn an_image_part_becomes_the_images_array_of_bare_base64() {
        // Ollama takes the payload alone — no `data:` prefix, no media type.
        let message = ChatMessage::with_parts(
            "user",
            vec![
                ContentPart::text("what is this?"),
                ContentPart::image("data:image/png;base64,AAAA"),
            ],
        );
        let messages = build_messages(&[message]);
        assert_eq!(
            messages,
            vec![json!({"role": "user", "content": "what is this?", "images": ["AAAA"]})]
        );
    }

    #[test]
    fn a_non_data_image_url_is_dropped_rather_than_sent_as_pixels() {
        let message = ChatMessage::with_parts(
            "user",
            vec![
                ContentPart::text("look"),
                ContentPart::image("https://example.com/a.png"),
            ],
        );
        let messages = build_messages(&[message]);
        assert_eq!(messages, vec![json!({"role": "user", "content": "look"})]);
    }

    #[test]
    fn an_assistant_tool_call_carries_its_arguments_as_an_object() {
        // A JSON *string* in `arguments` is a 400 here ("Value looks like
        // object, but can't find closing '}'"), where Chat Completions
        // carries exactly that string.
        let call = ChatMessage::assistant_tool_calls(
            "checking",
            vec![ToolCallSpec::function(
                "call_1",
                "bash",
                r#"{"command":"ls"}"#,
            )],
        );
        let messages = build_messages(&[call]);
        assert_eq!(
            messages,
            vec![json!({
                "role": "assistant",
                "content": "checking",
                "tool_calls": [{"id": "call_1", "function": {"name": "bash", "arguments": {"command": "ls"}}}],
            })]
        );
    }

    #[test]
    fn unparseable_arguments_degrade_to_an_empty_object() {
        let call = ChatMessage::assistant_tool_calls(
            "",
            vec![ToolCallSpec::function("call_1", "bash", "{\"command\": ")],
        );
        let messages = build_messages(&[call]);
        assert_eq!(
            messages[0]["tool_calls"][0]["function"]["arguments"],
            json!({})
        );
    }

    #[test]
    fn a_tool_result_names_the_call_it_answers() {
        // `tool_name` pairs a result to its call on Ollama; the name comes
        // from the assistant call seen earlier in the same list.
        let messages = build_messages(&[
            ChatMessage::assistant_tool_calls(
                "",
                vec![ToolCallSpec::function("call_1", "read", r#"{"path":"f"}"#)],
            ),
            ChatMessage::tool_result("call_1", "L1\n"),
        ]);
        assert_eq!(
            messages[1],
            json!({"role": "tool", "content": "L1\n", "tool_call_id": "call_1", "tool_name": "read"})
        );
        // A result whose call was never seen (an old rollout) omits the name
        // rather than guessing one.
        let orphan = build_messages(&[ChatMessage::tool_result("call_9", "x")]);
        assert_eq!(
            orphan[0],
            json!({"role": "tool", "content": "x", "tool_call_id": "call_9"})
        );
    }

    #[test]
    fn a_parts_message_with_no_images_is_flattened_to_text() {
        let message =
            ChatMessage::with_parts("user", vec![ContentPart::text("a"), ContentPart::text("b")]);
        assert_eq!(
            build_messages(&[message]),
            vec![json!({"role": "user", "content": "a\nb"})]
        );
        assert!(matches!(
            ChatMessage::with_parts("user", vec![]).content,
            MessageContent::Parts(_)
        ));
    }

    // --- the stream ---------------------------------------------------------

    /// Fold `lines` through one accumulator, collecting the deltas.
    fn drive(lines: &[&str]) -> (ChatAccumulator, Vec<Delta>, Vec<Step>) {
        let mut acc = ChatAccumulator::default();
        let mut deltas = Vec::new();
        let mut steps = Vec::new();
        for line in lines {
            let step = acc.on_line(line);
            if let Step::Delta(delta) = &step {
                deltas.push(delta.clone());
            }
            steps.push(step);
        }
        (acc, deltas, steps)
    }

    #[test]
    fn thinking_and_content_frames_become_the_two_delta_kinds() {
        let (acc, deltas, steps) = drive(&[
            r#"{"model":"qwen3:0.6b","message":{"role":"assistant","content":"","thinking":"Okay"},"done":false}"#,
            r#"{"model":"qwen3:0.6b","message":{"role":"assistant","content":"Hi"},"done":false}"#,
            r#"{"model":"qwen3:0.6b","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":141,"eval_count":120}"#,
        ]);
        assert_eq!(deltas.len(), 2);
        assert_eq!(deltas[0].reasoning, "Okay");
        assert_eq!(deltas[1].response, "Hi");
        assert!(matches!(steps[2], Step::Done), "{steps:?}");
        let (calls, reason, usage) = acc.finish();
        assert!(calls.is_empty());
        assert_eq!(reason.as_deref(), Some("stop"));
        assert_eq!(
            usage,
            Some(TokenUsage {
                input: 141,
                output: 120,
                ..TokenUsage::default()
            })
        );
    }

    #[test]
    fn a_tool_call_frame_lands_whole_with_its_id() {
        // Observed on 0.33: the call arrives complete on one frame, its
        // arguments an object and its id already minted.
        let (acc, deltas, _) = drive(&[
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"id":"call_v1gr5rr3","function":{"index":0,"name":"get_weather","arguments":{"city":"Paris"}}}]},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":10,"eval_count":5}"#,
        ]);
        assert_eq!(deltas.len(), 1, "the call is counted as it lands");
        assert_eq!(deltas[0].tool_call, r#"get_weather{"city":"Paris"}"#);
        let (calls, reason, _) = acc.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_v1gr5rr3");
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].arguments, r#"{"city":"Paris"}"#);
        assert_eq!(
            reason.as_deref(),
            Some("tool_calls"),
            "the crate's own spelling, whatever the server said"
        );
    }

    #[test]
    fn a_call_without_an_id_gets_a_synthetic_one() {
        let (acc, _, _) = drive(&[
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read","arguments":{}}},{"function":{"name":"bash","arguments":{"command":"ls"}}}]},"done":false}"#,
        ]);
        let (calls, _, _) = acc.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_0");
        assert_eq!(calls[1].id, "call_1");
        assert_eq!(calls[0].arguments, "{}");
    }

    #[test]
    fn an_error_line_fails_the_stream() {
        let (_, _, steps) = drive(&[r#"{"error":"model 'nope:latest' not found"}"#]);
        assert!(
            matches!(&steps[0], Step::Failed(m) if m == "model 'nope:latest' not found"),
            "{steps:?}"
        );
    }

    #[test]
    fn garbage_and_empty_lines_are_skipped() {
        let (_, deltas, steps) = drive(&[
            "",
            "not json",
            r#"{"message":{"role":"assistant","content":""},"done":false}"#,
        ]);
        assert!(deltas.is_empty());
        assert!(steps.iter().all(|s| matches!(s, Step::Continue)));
    }

    #[test]
    fn inline_think_tags_are_still_split_out() {
        // A custom GGUF import can lack the parser that fills `thinking`, so
        // its chain-of-thought arrives as `<think>` tags in the content —
        // the same splitter the Chat Completions wire runs catches it.
        let (acc, deltas, _) = drive(&[
            r#"{"message":{"role":"assistant","content":"<think>hmm</think>Hi"},"done":false}"#,
        ]);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].reasoning, "hmm");
        assert_eq!(deltas[0].response, "Hi");
        let text = acc.text();
        assert_eq!(text.response, "Hi");
        assert_eq!(text.reasoning, "hmm");
    }

    #[test]
    fn a_zero_usage_frame_reports_nothing() {
        let (acc, _, _) = drive(&[
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"load"}"#,
        ]);
        let (_, reason, usage) = acc.finish();
        assert_eq!(usage, None);
        assert_eq!(reason.as_deref(), Some("load"));
    }

    // --- the catalog --------------------------------------------------------

    /// The `/api/tags` body Ollama 0.33 sends: capabilities and the window
    /// ride each record, so no second request is needed.
    const TAGS: &str = r#"{"models":[
        {"name":"moondream:latest","model":"moondream:latest","size":1,"digest":"d",
         "details":{"family":"phi2","families":["phi2","clip"],"parameter_size":"1.9B","quantization_level":"Q4_0","context_length":2048},
         "capabilities":["completion","vision"]},
        {"name":"qwen3:0.6b","model":"qwen3:0.6b","size":1,"digest":"d",
         "details":{"family":"qwen3","families":["qwen3"],"parameter_size":"751.63M","quantization_level":"Q4_K_M","context_length":40960},
         "capabilities":["completion","tools","thinking"]},
        {"name":"nomic-embed-text:latest","model":"nomic-embed-text:latest","size":1,"digest":"d",
         "details":{"family":"nomic-bert","context_length":2048},
         "capabilities":["embedding"]},
        {"name":"gpt-oss:20b","model":"gpt-oss:20b","size":1,"digest":"d",
         "details":{"family":"gptoss","families":["gptoss"],"context_length":131072},
         "capabilities":["completion","tools","thinking"]}
    ]}"#;

    #[test]
    fn tags_records_read_the_name_capabilities_and_window() {
        let records = tags_records(TAGS).unwrap();
        assert_eq!(records.len(), 4);
        let qwen = &records[1];
        assert_eq!(qwen.name, "qwen3:0.6b");
        assert_eq!(qwen.family, "qwen3");
        assert_eq!(qwen.context_length, Some(40_960));
        assert_eq!(
            qwen.capabilities.as_deref(),
            Some(
                &[
                    "completion".to_string(),
                    "tools".to_string(),
                    "thinking".to_string()
                ][..]
            )
        );
    }

    #[test]
    fn an_older_tags_record_says_nothing_about_capabilities() {
        // Before capabilities rode `/api/tags` (and on the cloud listing,
        // whose `details` are empty) the record says nothing about what the
        // model can do — `/api/show` is what fills it in, and this is what
        // the fetch falls back to when that call fails.
        let body = r#"{"models":[{"name":"llama3:8b","model":"llama3:8b","size":1,"digest":"d",
            "details":{"family":"llama","families":["llama"]}}]}"#;
        let records = tags_records(body).unwrap();
        assert_eq!(records[0].capabilities, None);
        assert_eq!(records[0].context_length, None);
        assert!(!records[0].remote);
    }

    #[test]
    fn a_proxied_cloud_model_is_marked_remote() {
        // A local server signed in to ollama.com lists the `-cloud` models it
        // forwards; nothing is loaded locally for them, so the window rule
        // must not cap what the cloud serves at its maximum.
        let body = r#"{"models":[{"name":"gpt-oss:120b-cloud","model":"gpt-oss:120b-cloud",
            "remote_model":"gpt-oss:120b","remote_host":"https://ollama.com:443","size":0,"digest":"d",
            "details":{"family":"gptoss","context_length":131072},"capabilities":["completion","tools","thinking"]}]}"#;
        let records = tags_records(body).unwrap();
        assert!(records[0].remote);
        let entry = entry_of(&records[0], "ollama", None, false).unwrap();
        assert_eq!(entry.context, Some(131_072), "uncapped");
    }

    #[test]
    fn a_malformed_tags_record_is_skipped_not_fatal() {
        let body = r#"{"models":[{"name":"ok","model":"ok","details":{}},{"model":42},"junk"]}"#;
        let records = tags_records(body).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "ok");
        assert!(tags_records("not json").is_err());
    }

    #[test]
    fn a_show_body_fills_a_record_in() {
        // The observed `/api/show` shape: capabilities, the architecture-keyed
        // `model_info`, and the Modelfile parameters as one text block.
        let show = r#"{"license":"...","modelfile":"...",
            "parameters":"top_k                          20\nnum_ctx                        16384\nstop                           \"<|im_end|>\"",
            "details":{"family":"qwen3","families":["qwen3"],"parameter_size":"751.63M","quantization_level":"Q4_K_M"},
            "model_info":{"general.architecture":"qwen3","qwen3.context_length":40960,"qwen3.embedding_length":1024},
            "capabilities":["completion","tools","thinking"],"modified_at":"2026-09-01T20:56:46Z"}"#;
        let record = show_record("qwen3:0.6b", show).unwrap();
        assert_eq!(record.name, "qwen3:0.6b");
        assert_eq!(record.family, "qwen3");
        assert_eq!(record.context_length, Some(40_960));
        assert_eq!(record.modelfile_num_ctx, Some(16_384));
        assert_eq!(record.capabilities.as_deref().map(<[String]>::len), Some(3));
    }

    #[test]
    fn a_show_body_for_a_cloud_model_has_no_parameters() {
        let show = r#"{"capabilities":["completion","tools","thinking"],
            "details":{"parent_model":"gpt-oss:120b","family":"gptoss","families":null},
            "model_info":{"general.architecture":"gptoss","gptoss.context_length":131072}}"#;
        let record = show_record("gpt-oss:120b", show).unwrap();
        assert_eq!(record.family, "gptoss");
        assert_eq!(record.context_length, Some(131_072));
        assert_eq!(record.modelfile_num_ctx, None);
        assert!(show_record("x", "nope").is_err());
    }

    #[test]
    fn entries_carry_vision_reasoning_and_the_effective_window() {
        let records = tags_records(TAGS).unwrap();
        let entries: Vec<_> = records
            .iter()
            .filter_map(|r| entry_of(r, "ollama", None, false))
            .collect();
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["moondream:latest", "qwen3:0.6b", "gpt-oss:20b"],
            "an embedding model has no chat surface and is not offered"
        );
        let moondream = &entries[0];
        assert_eq!(moondream.vision, Some(true));
        assert_eq!(moondream.reasoning, None);
        assert_eq!(
            moondream.context,
            Some(2048),
            "under the cap: the model's own"
        );
        let qwen = &entries[1];
        assert_eq!(
            qwen.vision,
            Some(false),
            "the list is explicit: absence is no"
        );
        let support = qwen.reasoning.clone().expect("thinking");
        assert!(support.efforts.is_empty(), "on/off only");
        assert!(support.can_disable);
        assert_eq!(
            qwen.context,
            Some(DEFAULT_NUM_CTX_CAP),
            "capped: 40960 → the cap"
        );
        assert_eq!(qwen.provider, "ollama");
        assert_eq!(qwen.display_name, "qwen3:0.6b");
    }

    #[test]
    fn gpt_oss_takes_the_three_level_ladder() {
        let records = tags_records(TAGS).unwrap();
        let entry = entry_of(&records[3], "ollama", None, false).unwrap();
        let support = entry.reasoning.expect("thinking");
        assert_eq!(
            support.efforts,
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High
            ]
        );
        assert!(
            !support.can_disable,
            "gpt-oss cannot stop thinking — a footer reading `off` would lie"
        );
        assert_eq!(
            support.default_mode(),
            ThinkingMode::Effort(ReasoningEffort::Medium)
        );
    }

    #[test]
    fn a_record_without_capabilities_reports_unknown_rather_than_no() {
        // Only an explicit list can say "no vision"; a record that never
        // said (an old server whose show also lacked the field) stays
        // unknown, so the backend attaches optimistically as it always did.
        let record = CatalogRecord {
            name: "old:latest".to_string(),
            family: "llama".to_string(),
            capabilities: None,
            context_length: Some(8192),
            modelfile_num_ctx: None,
            remote: false,
        };
        let entry = entry_of(&record, "ollama", None, false).unwrap();
        assert_eq!(entry.vision, None);
        assert_eq!(entry.reasoning, None);
        assert_eq!(entry.context, Some(8192));
    }

    #[test]
    fn the_window_rule_prefers_the_modelfile_then_the_server_default_then_the_cap() {
        let record = |max: Option<u64>, modelfile: Option<u64>| CatalogRecord {
            name: "m".to_string(),
            family: String::new(),
            capabilities: None,
            context_length: max,
            modelfile_num_ctx: modelfile,
            remote: false,
        };
        // A Modelfile `num_ctx` is the user's own per-model choice.
        assert_eq!(
            context_window(&record(Some(131_072), Some(16_384)), Some(65_536), false),
            Some(16_384)
        );
        // Else the server's configured default (`OLLAMA_CONTEXT_LENGTH`).
        assert_eq!(
            context_window(&record(Some(131_072), None), Some(65_536), false),
            Some(65_536)
        );
        // Else the cap, which is what a bare install gets.
        assert_eq!(
            context_window(&record(Some(131_072), None), None, false),
            Some(DEFAULT_NUM_CTX_CAP)
        );
        // Never past what the model can hold: the server clamps there anyway,
        // and a gauge against more would lie.
        assert_eq!(
            context_window(&record(Some(8192), Some(16_384)), None, false),
            Some(8192)
        );
        assert_eq!(
            context_window(&record(Some(8192), None), Some(65_536), false),
            Some(8192)
        );
        // No declared window: the candidate stands, or nothing.
        assert_eq!(
            context_window(&record(None, None), Some(4096), false),
            Some(4096)
        );
        assert_eq!(context_window(&record(None, None), None, false), None);
        assert_eq!(context_window(&record(Some(0), None), None, false), None);
        // The cloud serves a model at its maximum, so nothing caps it there —
        // whether reached directly (`uncapped`) or through a local proxy.
        assert_eq!(
            context_window(&record(Some(131_072), None), None, true),
            Some(131_072)
        );
        let mut proxied = record(Some(131_072), None);
        proxied.remote = true;
        assert_eq!(context_window(&proxied, None, false), Some(131_072));
    }

    #[test]
    fn a_thinking_model_defaults_to_on_so_the_footer_and_the_wire_agree() {
        // Ollama thinks by default on a thinking model when `think` is
        // omitted; the seeded mode must say so rather than showing `off`.
        let records = tags_records(TAGS).unwrap();
        let qwen = entry_of(&records[1], "ollama", None, false).unwrap();
        assert_eq!(qwen.reasoning.unwrap().default_mode(), ThinkingMode::On);
    }

    // --- the errors worth explaining ----------------------------------------

    #[test]
    fn a_missing_model_says_how_to_pull_it() {
        let text = advice(404, r#"{"error":"model 'qwen3:8b' not found"}"#, "qwen3:8b").unwrap();
        assert!(text.contains("ollama pull qwen3:8b"), "{text}");
        assert!(text.contains("/model"), "{text}");
    }

    #[test]
    fn capability_refusals_name_what_to_change() {
        let tools = advice(
            400,
            r#"{"error":"registry.ollama.ai/library/gemma3:270m does not support tools"}"#,
            "gemma3:270m",
        )
        .unwrap();
        assert!(tools.contains("tool"), "{tools}");
        assert!(tools.contains("/settings"), "{tools}");
        let thinking = advice(
            400,
            r#"{"error":"\"gemma3:270m\" does not support thinking"}"#,
            "gemma3:270m",
        )
        .unwrap();
        assert!(thinking.contains("ctrl+t"), "{thinking}");
        let vision = advice(400, r#"{"error":"{\"error\":{\"code\":400,\"message\":\"Multimodal data provided, but model does not support multimodal requests.\"}}"}"#, "gemma3:270m").unwrap();
        assert!(vision.contains("image"), "{vision}");
        assert_eq!(
            advice(500, r#"{"error":"boom"}"#, "m"),
            None,
            "an unknown failure keeps its own body"
        );
    }

    #[test]
    fn a_cloud_refusal_points_at_the_key() {
        let text = advice(401, r#"{"error":"Unauthorized"}"#, "gpt-oss:120b").unwrap();
        assert!(text.contains("OLLAMA_API_KEY"), "{text}");
    }

    #[test]
    fn a_refused_connection_says_to_start_the_server() {
        let text = connection_advice(
            "http://127.0.0.1:11434",
            "error sending request for url (http://127.0.0.1:11434/api/chat): client error (Connect): tcp connect error: Connection refused (os error 111)",
        )
        .unwrap();
        assert!(text.contains("http://127.0.0.1:11434"), "{text}");
        assert!(text.contains("ollama serve"), "{text}");
        assert_eq!(
            connection_advice("http://127.0.0.1:11434", "operation timed out"),
            None,
            "only a refusal is the server being down"
        );
    }

    #[test]
    fn the_request_borrows_a_pictures_bytes_rather_than_copying_them() {
        // The whole point of the typed request (`docs/memory.md`): a
        // message's `images` entry is a slice of the shared encoding.
        let url = crate::llm::AttachmentUrl::from("data:image/png;base64,AAAA");
        let messages_in = [ChatMessage::with_parts(
            "user",
            vec![ContentPart::text("look"), ContentPart::image(url.clone())],
        )];
        let messages = messages_of(&messages_in);
        let payload = &url["data:image/png;base64,".len()..];
        let image = messages[0].images[0];
        assert!(
            std::ptr::eq(image.as_ptr(), payload.as_ptr()) && image.len() == payload.len(),
            "the entry points into the shared encoding"
        );
    }
}
