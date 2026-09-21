//! Anthropic's **Messages** API as a wire format — the one both the pasted
//! `ANTHROPIC_API_KEY` and the Claude Pro/Max sign-in speak. See
//! `docs/claude.md`.
//!
//! Everything here is a **translation**, in both directions, between that
//! format and the Chat Completions currency the rest of the crate already
//! uses ([`ChatMessage`] in, [`Delta`] /
//! [`StreamOutcome`](super::openai::StreamOutcome) out) — the same contract
//! [`super::responses`] keeps for OpenAI's Responses API. Nothing above
//! [`super::openai::OpenAiClient`] learns that a third wire format exists:
//! the agent loop, the transcript, the rollout and the derived context are
//! untouched.
//!
//! Six shapes differ, and each is a place a naive port breaks:
//!
//! - **There is no `system` role.** The system prompt is a *top-level*
//!   `system` array of text blocks, so it is hoisted out of `messages`
//!   entirely (Responses spells the same idea `instructions`).
//! - **There is no `tool` role either.** A tool result is a `tool_result`
//!   *content block* inside a **user** message, and every result of one
//!   parallel batch must ride **one** such message — splitting them teaches
//!   the model to stop calling tools in parallel. That is why
//!   [`build_messages`] merges consecutive same-role messages rather than
//!   emitting one per [`ChatMessage`].
//! - **A tool call is a content block**, not a field on the assistant
//!   message, and its `input` is a **JSON object** where Chat Completions
//!   carries a JSON *string*.
//! - **An image is a `source` object**, not a URL string: a `data:` URL has
//!   to be split back into its media type and payload.
//! - **`max_tokens` is required.** There is no "as much as you like".
//! - **Thinking is spelled two ways** — see [`thinking_body`].
//!
//! Pure and unit-tested — the HTTP lives in [`super::openai`].

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::config::ModelConfig;
use super::openai::Delta;
use super::reasoning::ThinkingMode;
use super::tools::ToolCallRequest;
use super::{CacheControl, ChatMessage, ContentPart, MessageContent};
use crate::stream::TokenUsage;

/// The `max_tokens` every request carries, since the API requires one and the
/// app has no per-model output cap to hand ([`super::models::ModelEntry`]
/// keeps the *context* window, which is a different number).
///
/// Chosen to sit under the smallest output cap among the current models
/// (Haiku 4.5's 64K; the Opus/Sonnet families allow 128K) while leaving room
/// for the long file writes an agentic turn makes. A model whose own cap is
/// lower clamps rather than failing.
pub const MAX_TOKENS: u64 = 32_000;

/// The thinking budget [`ThinkingMode::On`] spends on a model that takes the
/// older `budget_tokens` form. Must stay below [`MAX_TOKENS`] — the API
/// refuses a budget that meets or exceeds the output cap — and above the
/// 1024-token floor it also enforces.
pub const THINKING_BUDGET: u64 = 10_000;

// ---------------------------------------------------------------------------
// The request body
// ---------------------------------------------------------------------------

/// Build the streamed request body for `messages` — as a JSON tree, the
/// tests' view of the request. The wire streams a [`RequestSource`], which
/// writes the same request from the messages **by reference**.
///
/// `max_tokens` and `stream` are mandatory here rather than conventional, and
/// `temperature` is deliberately **never sent**: the sampling parameters were
/// removed from every current Claude model, where they are now a 400 that
/// blames the request rather than the field. See `docs/claude.md`.
#[must_use]
pub fn build_payload(cfg: &ModelConfig, tools: &[Value], messages: &[ChatMessage]) -> Value {
    serde_json::to_value(request(cfg, tools, messages)).unwrap_or(Value::Null)
}

/// The Messages request as it is serialized: the hoisted `system` blocks and
/// the `messages` **borrowed from the conversation** — a picture's base64 is
/// the session's one shared encoding, a slice of it, never a copy into a tree
/// (`docs/memory.md`) — beside every other field in one small map.
#[derive(serde::Serialize)]
struct Request<'a> {
    #[serde(flatten)]
    fields: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    system: Vec<Block<'a>>,
    messages: Vec<Message<'a>>,
}

fn request<'a>(cfg: &ModelConfig, tools: &[Value], messages: &'a [ChatMessage]) -> Request<'a> {
    let (mut system, mut input) = blocks_of(messages);
    let mut fields = serde_json::Map::new();
    fields.insert("model".to_string(), json!(cfg.model));
    fields.insert("max_tokens".to_string(), json!(MAX_TOKENS));
    fields.insert("stream".to_string(), json!(true));
    if !tools.is_empty() {
        fields.insert("tools".to_string(), json!(anthropic_tools(tools)));
    }
    if let Some(mode) = cfg.thinking {
        fields.insert("thinking".to_string(), thinking_body(mode));
        // The effort ladder is `output_config`'s, not `thinking`'s — the two
        // are separate parameters, and putting the level inside the thinking
        // object is an unknown-field 400.
        if let Some(effort) = super::reasoning::effort_label(mode) {
            fields.insert("output_config".to_string(), json!({"effort": effort}));
        }
    }
    apply_cache_breakpoints(&mut system, &mut input);
    for (k, v) in &cfg.extra_body {
        fields.insert(k.clone(), v.clone());
    }
    Request {
        fields,
        system,
        messages: input,
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

/// The request's `thinking` object for a mode.
///
/// Anthropic spells extended thinking two ways, and which one a model takes
/// is a property of the model:
///
/// - **adaptive** (`{"type": "adaptive"}`) on every current model, where the
///   *depth* is `output_config.effort` rather than a token budget;
/// - **budgeted** (`{"type": "enabled", "budget_tokens": N}`) on the older
///   models, which take no effort parameter at all.
///
/// [`ThinkingMode`] already tells the two apart without a second lookup:
/// [`ThinkingMode::On`] is produced *only* for a reasoner whose record listed
/// no effort levels ([`ReasoningSupport::modes`]), which on this provider is
/// exactly the budgeted family — so `On` takes the budget form and
/// [`ThinkingMode::Effort`] the adaptive one.
///
/// [`ReasoningSupport::modes`]: super::reasoning::ReasoningSupport::modes
#[must_use]
pub fn thinking_body(mode: ThinkingMode) -> Value {
    match mode {
        ThinkingMode::Off => json!({"type": "disabled"}),
        ThinkingMode::On => json!({"type": "enabled", "budget_tokens": THINKING_BUDGET}),
        // `display` is opt-in: the newer models default it to `omitted`,
        // which streams thinking blocks whose text is empty — and an empty
        // live `● Thinking…` cell is worse than none at all
        // (`docs/thinking-stream.md`).
        ThinkingMode::Effort(_) => json!({"type": "adaptive", "display": "summarized"}),
    }
}

/// Split `messages` into the top-level `system` blocks and the `messages`
/// array — as JSON trees, the tests' view of `blocks_of`.
#[must_use]
pub fn build_messages(messages: &[ChatMessage]) -> (Vec<Value>, Vec<Value>) {
    let (system, input) = blocks_of(messages);
    (
        system
            .iter()
            .map(|block| serde_json::to_value(block).unwrap_or(Value::Null))
            .collect(),
        input
            .iter()
            .map(|message| serde_json::to_value(message).unwrap_or(Value::Null))
            .collect(),
    )
}

/// One content block of a Messages request. Every variant can carry the
/// prompt-caching breakpoint ([`apply_cache_breakpoints`] marks whichever
/// block comes last), and a block borrows the message it came from wherever
/// it can — the picture's bytes always.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Block<'a> {
    Text {
        text: Cow<'a, str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    Image {
        source: Source<'a>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolUse {
        id: &'a str,
        name: &'a str,
        input: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolResult {
        tool_use_id: &'a str,
        content: Cow<'a, str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

impl Block<'_> {
    /// Put an ephemeral breakpoint on this block.
    fn mark(&mut self) {
        let slot = match self {
            Block::Text { cache_control, .. }
            | Block::Image { cache_control, .. }
            | Block::ToolUse { cache_control, .. }
            | Block::ToolResult { cache_control, .. } => cache_control,
        };
        *slot = Some(CacheControl::EPHEMERAL);
    }
}

/// An `image` block's `source`: a `data:` URL **split** into its media type
/// and payload — both slices of the shared encoding — or an `http(s)` URL.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Source<'a> {
    Base64 { media_type: &'a str, data: &'a str },
    Url { url: &'a str },
}

/// One `messages` entry: a role over its content blocks.
#[derive(Debug, serde::Serialize)]
struct Message<'a> {
    role: &'static str,
    content: Vec<Block<'a>>,
}

/// Split `messages` into the top-level `system` blocks and the `messages`
/// array.
///
/// System messages are **hoisted** (each its own text block, in order) —
/// this API has no system role in `messages` at all. Everything else is
/// folded into `user`/`assistant` messages, with **consecutive same-role
/// messages merged**: a round's several `tool` results are one user message,
/// not several.
fn blocks_of(messages: &[ChatMessage]) -> (Vec<Block<'_>>, Vec<Message<'_>>) {
    let mut system: Vec<Block<'_>> = Vec::new();
    let mut out: Vec<Message<'_>> = Vec::new();
    for message in messages {
        match message.role.as_str() {
            "system" | "developer" => {
                if let Some(text) = non_empty(flatten_text(&message.content)) {
                    system.push(Block::Text {
                        text,
                        cache_control: None,
                    });
                }
            }
            "tool" => {
                let block = Block::ToolResult {
                    tool_use_id: message.tool_call_id.as_deref().unwrap_or(""),
                    content: flatten_text(&message.content),
                    cache_control: None,
                };
                push_block("user", block, &mut out);
            }
            "assistant" => {
                // The text the model wrote, then the calls it made — the
                // order they happened in, and the order the API expects.
                if let Some(text) = non_empty(flatten_text(&message.content)) {
                    push_block(
                        "assistant",
                        Block::Text {
                            text,
                            cache_control: None,
                        },
                        &mut out,
                    );
                }
                for call in &message.tool_calls {
                    push_block("assistant", tool_use_block(call), &mut out);
                }
            }
            _ => {
                for block in user_content(&message.content) {
                    push_block("user", block, &mut out);
                }
            }
        }
    }
    (system, out)
}

/// One assistant `tool_use` block. The call's `arguments` is a JSON *string*
/// in the Chat Completions currency and must arrive here as an **object**; a
/// string that doesn't parse (a call the model truncated) degrades to `{}`
/// rather than failing the whole request.
fn tool_use_block(call: &super::ToolCallSpec) -> Block<'_> {
    let input = serde_json::from_str::<Value>(&call.function.arguments)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    Block::ToolUse {
        id: &call.id,
        name: &call.function.name,
        input,
        cache_control: None,
    }
}

/// Append `block` to the last message when it already has `role`, else open a
/// new one. This is what merges a parallel batch's results into a single user
/// message.
fn push_block<'a>(role: &'static str, block: Block<'a>, out: &mut Vec<Message<'a>>) {
    if let Some(last) = out.last_mut()
        && last.role == role
    {
        last.content.push(block);
        return;
    }
    out.push(Message {
        role,
        content: vec![block],
    });
}

/// A user-side message's content blocks. Text is a `text` block; an image is
/// an `image` block whose `source` is the **split** `data:` URL — the media
/// type and the payload are separate fields here, where Chat Completions
/// carries one string.
fn user_content(content: &MessageContent) -> Vec<Block<'_>> {
    match content {
        MessageContent::Text(text) => non_empty(Cow::Borrowed(text.as_str()))
            .map(|text| {
                vec![Block::Text {
                    text,
                    cache_control: None,
                }]
            })
            .unwrap_or_default(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text, .. } => {
                    non_empty(Cow::Borrowed(text.as_str())).map(|text| Block::Text {
                        text,
                        cache_control: None,
                    })
                }
                ContentPart::ImageUrl { image_url } => {
                    image_source(&image_url.url).map(|source| Block::Image {
                        source,
                        cache_control: None,
                    })
                }
            })
            .collect(),
    }
}

/// One `image` content block for a URL as a JSON tree — the tests' view of
/// `image_source` — or `None` for a value this API has no source shape
/// for.
#[must_use]
pub fn image_block(url: &str) -> Option<Value> {
    let block = Block::Image {
        source: image_source(url)?,
        cache_control: None,
    };
    Some(serde_json::to_value(block).unwrap_or(Value::Null))
}

/// An `image` block's source for a URL, or `None` for a value this API has
/// no source shape for — dropping the part beats 400ing the whole turn over
/// it.
///
/// A `data:` URL becomes a `base64` source (its media type and payload
/// separated, both borrowed from the URL); an `http(s)` URL becomes a `url`
/// source.
fn image_source(url: &str) -> Option<Source<'_>> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (media_type, data) = rest.split_once(";base64,")?;
        if media_type.is_empty() || data.is_empty() {
            return None;
        }
        return Some(Source::Base64 { media_type, data });
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return Some(Source::Url { url });
    }
    None
}

/// A message's text, with any image parts dropped — what a `tool_result` and
/// an assistant echo need. Borrowed when the content is one string, joined
/// (and so owned) when it is parts.
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

/// `Some(text)` when it carries anything — an empty text block is a 400 here.
fn non_empty(text: Cow<'_, str>) -> Option<Cow<'_, str>> {
    (!text.trim().is_empty()).then_some(text)
}

/// Flatten Chat Completions tool specs into the Messages shape: the
/// `function` object's fields are lifted to the top level and `parameters` is
/// renamed `input_schema`. There is no `type: "function"` wrapper here.
#[must_use]
pub fn anthropic_tools(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|spec| {
            let function = spec.get("function")?;
            let mut out = json!({
                "name": function.get("name")?.clone(),
                "input_schema": function
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
            });
            if let Some(description) = function.get("description") {
                out["description"] = description.clone();
            }
            Some(out)
        })
        .collect()
}

/// Mark the request with up to three `cache_control` breakpoints —
/// [`super::cache`]'s placement rule, applied to this API's own shape rather
/// than to a chat-completions `messages` array:
///
/// 1. the **last system block** — the big stable prefix (persona,
///    environment, the skills reminder) every request shares;
/// 2. the **last content block of the last message** — a moving breakpoint
///    that tracks the conversation frontier, so each agentic round caches
///    everything so far and the next one reads it back;
/// 3. the last block of the **user message before that** — insurance for the
///    bounded lookback when many blocks land between two requests.
///
/// Unlike the OpenRouter path this needs no model-id sniff: on Anthropic's
/// own API caching is *always* explicit. The four-breakpoint ceiling is the
/// same one [`super::cache::MAX_BREAKPOINTS`] states, and three is what this
/// places by construction.
fn apply_cache_breakpoints(system: &mut [Block<'_>], messages: &mut [Message<'_>]) {
    if let Some(block) = system.last_mut() {
        block.mark();
    }
    let last = messages.len().checked_sub(1);
    // The newest user message *before* the frontier, so the two breakpoints
    // are never the same block.
    let previous = last.and_then(|last| messages[..last].iter().rposition(|m| m.role == "user"));
    for index in [previous, last].into_iter().flatten() {
        if let Some(block) = messages[index].content.last_mut() {
            block.mark();
        }
    }
}

// ---------------------------------------------------------------------------
// The event stream
// ---------------------------------------------------------------------------

/// What one frame of a Messages stream did to the round. The vocabulary is
/// event-typed like the Responses API's, but the round's *shape* is
/// stateful — a tool call opens on one frame, accumulates over several, and
/// closes on another — so the classifier is a fold rather than a pure
/// `parse_event`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Text to surface (reply and/or thinking, already split apart).
    Delta(Delta),
    /// Nothing this caller needs (a `ping`, a signature, a block boundary).
    Continue,
    /// The stream finished cleanly.
    Done,
    /// The server failed the response in-band.
    Failed(String),
}

/// Folds a Messages SSE stream into the crate's [`StreamOutcome`] currency.
///
/// The stateful part is the **content-block index**: a `tool_use` block opens
/// with its id and name on `content_block_start`, its arguments arrive as
/// `input_json_delta` *partial JSON strings* over any number of frames, and
/// it closes on `content_block_stop`. Several blocks can be open at once, so
/// they are tracked by index rather than as one "current" call.
///
/// [`StreamOutcome`]: super::openai::StreamOutcome
#[derive(Debug, Default)]
pub struct MessageAccumulator {
    /// In-flight `tool_use` blocks by content-block index.
    open: BTreeMap<u64, PartialToolUse>,
    /// The calls that have closed, in the order they closed.
    calls: Vec<ToolCallRequest>,
    /// The `stop_reason` the final `message_delta` reported.
    stop_reason: Option<String>,
    /// The round's usage, merged across `message_start` and `message_delta`.
    usage: Option<TokenUsage>,
    /// Whether any reply text streamed — the one thing that decides whether a
    /// refusal is a failure or merely a short answer.
    saw_text: bool,
}

/// One `tool_use` content block being built.
#[derive(Debug, Default)]
struct PartialToolUse {
    id: String,
    name: String,
    arguments: String,
}

impl MessageAccumulator {
    /// Fold one `data:` payload in.
    #[must_use]
    pub fn on_frame(&mut self, data: &str) -> Step {
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return Step::Continue;
        };
        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "message_start" => {
                self.merge_usage(value.get("message").and_then(|m| m.get("usage")));
                Step::Continue
            }
            "content_block_start" => self.open_block(&value),
            "content_block_delta" => self.block_delta(&value),
            "content_block_stop" => {
                self.close_block(block_index(&value));
                Step::Continue
            }
            "message_delta" => {
                self.merge_usage(value.get("usage"));
                let reason = value
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str);
                if let Some(reason) = reason {
                    self.stop_reason = Some(reason.to_string());
                    if reason == "refusal" && !self.saw_text {
                        return Step::Failed(refusal_notice(value.get("delta")));
                    }
                }
                Step::Continue
            }
            "message_stop" => Step::Done,
            "error" => Step::Failed(failure_message(&value, data)),
            // `ping`, and every frame a future API version adds.
            _ => Step::Continue,
        }
    }

    /// A content block opened. A `tool_use` starts accumulating (its name is
    /// also the first thing the token tally can count for the call); text and
    /// thinking blocks need nothing.
    fn open_block(&mut self, value: &Value) -> Step {
        let Some(block) = value.get("content_block") else {
            return Step::Continue;
        };
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            return Step::Continue;
        }
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.open.insert(
            block_index(value),
            PartialToolUse {
                id: block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: name.clone(),
                arguments: String::new(),
            },
        );
        Step::Delta(Delta {
            tool_call: name,
            ..Delta::default()
        })
    }

    /// A delta landed on an open block.
    fn block_delta(&mut self, value: &Value) -> Step {
        let Some(delta) = value.get("delta") else {
            return Step::Continue;
        };
        let text = |key: &str| {
            delta
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        match delta
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "text_delta" => {
                let chunk = text("text");
                if chunk.is_empty() {
                    return Step::Continue;
                }
                self.saw_text = true;
                Step::Delta(Delta {
                    response: chunk,
                    ..Delta::default()
                })
            }
            "thinking_delta" => {
                let chunk = text("thinking");
                if chunk.is_empty() {
                    return Step::Continue;
                }
                Step::Delta(Delta {
                    reasoning: chunk,
                    ..Delta::default()
                })
            }
            "input_json_delta" => {
                let fragment = text("partial_json");
                if fragment.is_empty() {
                    return Step::Continue;
                }
                if let Some(open) = self.open.get_mut(&block_index(value)) {
                    open.arguments.push_str(&fragment);
                }
                // Never rendered — counted, so the tally ticks while the call
                // is produced (`docs/status-indicator.md`).
                Step::Delta(Delta {
                    tool_call: fragment,
                    ..Delta::default()
                })
            }
            // `signature_delta` verifies the thinking block's integrity and
            // is not text; nothing here needs it.
            _ => Step::Continue,
        }
    }

    /// A content block closed: a `tool_use` becomes a finished call.
    fn close_block(&mut self, index: u64) {
        let Some(open) = self.open.remove(&index) else {
            return;
        };
        self.calls.push(ToolCallRequest {
            id: open.id,
            name: open.name,
            // An empty accumulation is a call with no parameters, which is
            // `{}` — not the empty string, which no provider can parse back.
            arguments: if open.arguments.trim().is_empty() {
                "{}".to_string()
            } else {
                open.arguments
            },
        });
    }

    /// Merge one frame's `usage` object in.
    ///
    /// The counts arrive in two places: the input side lands whole on
    /// `message_start`, and `message_delta` repeats the counters
    /// **cumulatively** (verified live: the delta carries all four, and the
    /// docs' server-tool example grows `input_tokens` on it). Anthropic
    /// reports `input_tokens` as the *uncached remainder* — the prompt's real
    /// size is that plus both cache figures — so the sum is what the crate's
    /// OpenAI-shaped [`TokenUsage::input`] means.
    ///
    /// Every counter is **monotonic**: a later frame can only report more,
    /// and a frame that omits one leaves what an earlier frame said. That is
    /// what keeps a delta naming the remainder *without* the cache keys — the
    /// older documented shape, or a shim's abbreviation — from reading as
    /// "the cached share is now zero" and shrinking the whole-prompt `input`
    /// to a few dozen tokens on the receipt and the gauge.
    fn merge_usage(&mut self, usage: Option<&Value>) {
        let Some(usage) = usage else { return };
        let count = |key: &str| usage.get(key).and_then(Value::as_u64);
        let merged = self.usage.get_or_insert_with(TokenUsage::default);
        if let Some(cached) = count("cache_read_input_tokens") {
            merged.cached = merged.cached.max(cached);
        }
        if let Some(cache_write) = count("cache_creation_input_tokens") {
            merged.cache_write = merged.cache_write.max(cache_write);
        }
        if let Some(remainder) = count("input_tokens") {
            let input = remainder
                .saturating_add(merged.cached)
                .saturating_add(merged.cache_write);
            merged.input = merged.input.max(input);
        }
        if let Some(output) = count("output_tokens") {
            merged.output = merged.output.max(output);
        }
    }

    /// What the round produced: its tool calls, the `finish_reason` the rest
    /// of the crate speaks, and the provider's own usage.
    #[must_use]
    pub fn finish(self) -> (Vec<ToolCallRequest>, Option<String>, Option<TokenUsage>) {
        // The crate's own spelling, so the agent loop and the transcript read
        // one vocabulary whatever the provider called it.
        let reason = self.stop_reason.map(|reason| {
            if reason == "tool_use" {
                "tool_calls".to_string()
            } else {
                reason
            }
        });
        (self.calls, reason, self.usage)
    }
}

/// The content-block index a frame names, defaulting to 0 — the index is
/// always present in practice, and a frame that lost it is better folded into
/// the first block than dropped.
fn block_index(value: &Value) -> u64 {
    value.get("index").and_then(Value::as_u64).unwrap_or(0)
}

/// What to tell the user when the model declined the request outright. The
/// category is an open set, so it is quoted rather than mapped.
fn refusal_notice(delta: Option<&Value>) -> String {
    let details = delta.and_then(|d| d.get("stop_details"));
    let category = details
        .and_then(|d| d.get("category"))
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty());
    let explanation = details
        .and_then(|d| d.get("explanation"))
        .and_then(Value::as_str)
        .filter(|e| !e.trim().is_empty());
    match (category, explanation) {
        (_, Some(explanation)) => format!("the model declined this request: {explanation}"),
        (Some(category), None) => {
            format!("the model declined this request ({category})")
        }
        (None, None) => "the model declined this request".to_string(),
    }
}

/// The message out of an in-band `error` frame, however it is nested. Falls
/// back to the whole payload rather than reporting an empty reason.
fn failure_message(value: &Value, data: &str) -> String {
    value
        .get("error")
        .and_then(|e| {
            e.get("message")
                .and_then(Value::as_str)
                .or_else(|| e.as_str())
        })
        .filter(|m| !m.trim().is_empty())
        .map_or_else(|| data.to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::config::AuthScheme;
    use crate::llm::reasoning::ReasoningEffort;
    use crate::llm::{ImageUrl, ToolCallSpec};

    fn text(role: &str, body: &str) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: MessageContent::Text(body.to_string()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    fn tool_result(id: &str, body: &str) -> ChatMessage {
        ChatMessage {
            role: "tool".to_string(),
            content: MessageContent::Text(body.to_string()),
            tool_calls: Vec::new(),
            tool_call_id: Some(id.to_string()),
        }
    }

    fn cfg() -> ModelConfig {
        let mut cfg = ModelConfig::fallback();
        cfg.model = "claude-opus-5".to_string();
        cfg.api_base = "https://api.anthropic.com/v1".to_string();
        cfg.wire_api = super::super::WireApi::Anthropic;
        cfg
    }

    // --- the request body -------------------------------------------------

    #[test]
    fn a_payload_carries_the_required_fields() {
        // `max_tokens` is mandatory here rather than conventional: without it
        // the API answers 400 rather than defaulting.
        let payload = build_payload(&cfg(), &[], &[text("user", "hi")]);
        assert_eq!(payload["model"], "claude-opus-5");
        assert_eq!(payload["max_tokens"], MAX_TOKENS);
        assert_eq!(payload["stream"], true);
        assert_eq!(payload["messages"][0]["role"], "user");
        assert_eq!(payload["messages"][0]["content"][0]["type"], "text");
        assert_eq!(payload["messages"][0]["content"][0]["text"], "hi");
    }

    #[test]
    fn a_payload_never_sends_temperature() {
        // Sampling parameters were removed from the current models, where
        // sending one is a 400 that blames the request, not the field.
        let mut cfg = cfg();
        cfg.temperature = Some(0.7);
        let payload = build_payload(&cfg, &[], &[text("user", "hi")]);
        assert!(payload.get("temperature").is_none());
        assert!(payload.get("top_p").is_none());
    }

    #[test]
    fn the_system_prompt_is_hoisted_out_of_the_messages() {
        // There is no `system` role in this API's `messages` array.
        let messages = [text("system", "be helpful"), text("user", "hi")];
        let payload = build_payload(&cfg(), &[], &messages);
        assert_eq!(payload["system"][0]["text"], "be helpful");
        assert_eq!(payload["messages"].as_array().unwrap().len(), 1);
        assert_eq!(payload["messages"][0]["role"], "user");
    }

    #[test]
    fn no_client_identity_is_ever_injected_into_the_system_prompt() {
        // The Claude Code subscription endpoint mandates an identity line;
        // this app does not sign in to that endpoint and must not claim to
        // be that client. What goes on the wire is the user's own prompt.
        // See `docs/claude.md`.
        let mut sub = cfg();
        sub.auth = AuthScheme::AnthropicConsole;
        let messages = [text("system", "be helpful"), text("user", "hi")];
        for cfg in [&sub, &cfg()] {
            let payload = build_payload(cfg, &[], &messages);
            assert_eq!(payload["system"].as_array().unwrap().len(), 1);
            assert_eq!(payload["system"][0]["text"], "be helpful");
            assert!(
                !payload.to_string().contains("official CLI"),
                "no identity claim rides the request"
            );
        }
    }

    #[test]
    fn an_empty_system_prompt_sends_no_system_field() {
        // An empty array is not the same as an absent one, and one empty text
        // block is a 400.
        let payload = build_payload(&cfg(), &[], &[text("user", "hi")]);
        assert!(payload.get("system").is_none());
    }

    #[test]
    fn a_tool_result_becomes_a_tool_result_block_in_a_user_message() {
        // There is no `tool` role: the result is a content block of a *user*
        // message, keyed to the call by `tool_use_id`.
        let (_, messages) = build_messages(&[tool_result("toolu_1", "42")]);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"][0]["type"], "tool_result");
        assert_eq!(messages[0]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(messages[0]["content"][0]["content"], "42");
    }

    #[test]
    fn a_parallel_batchs_results_ride_one_user_message() {
        // Splitting them across messages silently teaches the model to stop
        // calling tools in parallel — the whole reason consecutive same-role
        // messages merge.
        let batch = [
            tool_result("toolu_1", "one"),
            tool_result("toolu_2", "two"),
            tool_result("toolu_3", "three"),
        ];
        let (_, messages) = build_messages(&batch);
        assert_eq!(messages.len(), 1, "one message, three blocks");
        let blocks = messages[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[2]["tool_use_id"], "toolu_3");
    }

    #[test]
    fn consecutive_user_messages_merge_into_one() {
        let (_, messages) = build_messages(&[text("user", "a"), text("user", "b")]);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn an_assistant_tool_call_becomes_a_tool_use_block_with_an_object_input() {
        // `arguments` is a JSON *string* in the crate's currency and must
        // arrive here as an object.
        let message = ChatMessage {
            role: "assistant".to_string(),
            content: MessageContent::Text("let me look".to_string()),
            tool_calls: vec![ToolCallSpec::function(
                "toolu_1",
                "read",
                r#"{"path":"/tmp/x"}"#,
            )],
            tool_call_id: None,
        };
        let (_, messages) = build_messages(&[message]);
        assert_eq!(messages.len(), 1);
        let blocks = messages[0]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "let me look");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["id"], "toolu_1");
        assert_eq!(blocks[1]["name"], "read");
        assert_eq!(blocks[1]["input"]["path"], "/tmp/x");
        assert!(blocks[1]["input"].is_object(), "an object, not a string");
    }

    #[test]
    fn a_tool_call_whose_arguments_do_not_parse_degrades_to_an_empty_object() {
        // A call the model truncated must not fail the whole request.
        let message = ChatMessage {
            role: "assistant".to_string(),
            content: MessageContent::Text(String::new()),
            tool_calls: vec![ToolCallSpec::function("toolu_1", "read", "{\"path\":")],
            tool_call_id: None,
        };
        let (_, messages) = build_messages(&[message]);
        assert_eq!(messages[0]["content"][0]["input"], json!({}));
    }

    #[test]
    fn an_assistant_message_with_no_content_at_all_is_dropped() {
        // An empty `content` array is a 400.
        let message = ChatMessage {
            role: "assistant".to_string(),
            content: MessageContent::Text("   ".to_string()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        };
        let (_, messages) = build_messages(&[message]);
        assert!(messages.is_empty());
    }

    #[test]
    fn a_whole_agentic_round_translates_to_the_shape_the_api_documents() {
        // The pieces are covered above; this pins how they *compose*, which
        // is where an ordering or merging slip would actually land.
        let assistant = ChatMessage {
            role: "assistant".to_string(),
            content: MessageContent::Text("Checking both.".to_string()),
            tool_calls: vec![
                ToolCallSpec::function("toolu_1", "read", r#"{"path":"a.rs"}"#),
                ToolCallSpec::function("toolu_2", "read", r#"{"path":"b.rs"}"#),
            ],
            tool_call_id: None,
        };
        let round = [
            text("system", "You are Alter Zero."),
            text("user", "read both files"),
            assistant,
            tool_result("toolu_1", "fn a() {}"),
            tool_result("toolu_2", "fn b() {}"),
            text("user", "now summarise"),
        ];
        let payload = build_payload(&cfg(), &[], &round);

        assert_eq!(payload["system"][0]["text"], "You are Alter Zero.");
        let wire = payload["messages"].as_array().unwrap();
        // user → assistant(text + 2 tool_use) → user(2 tool_result) → user,
        // with the two adjacent user messages at the end merged.
        assert_eq!(wire.len(), 3, "{}", payload["messages"]);

        assert_eq!(wire[0]["role"], "user");
        assert_eq!(wire[0]["content"][0]["text"], "read both files");

        assert_eq!(wire[1]["role"], "assistant");
        let said = wire[1]["content"].as_array().unwrap();
        assert_eq!(said.len(), 3, "the text it wrote, then the calls it made");
        assert_eq!(said[0]["type"], "text");
        assert_eq!(said[1]["type"], "tool_use");
        assert_eq!(said[1]["id"], "toolu_1");
        assert_eq!(said[2]["id"], "toolu_2");

        // Both results in ONE user message, contiguous, then the new turn's
        // own text folded onto the end of it.
        assert_eq!(wire[2]["role"], "user");
        let back = wire[2]["content"].as_array().unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[0]["type"], "tool_result");
        assert_eq!(back[0]["tool_use_id"], "toolu_1");
        assert_eq!(back[1]["tool_use_id"], "toolu_2");
        assert_eq!(back[2]["type"], "text");
        assert_eq!(back[2]["text"], "now summarise");

        // And nothing anywhere claims to be a role this API doesn't have.
        for message in wire {
            let role = message["role"].as_str().unwrap();
            assert!(matches!(role, "user" | "assistant"), "role {role}");
        }
    }

    #[test]
    fn a_data_url_image_is_split_into_a_base64_source() {
        let block = image_block("data:image/png;base64,AAAA").expect("a base64 source");
        assert_eq!(block["type"], "image");
        assert_eq!(block["source"]["type"], "base64");
        assert_eq!(block["source"]["media_type"], "image/png");
        assert_eq!(block["source"]["data"], "AAAA");
    }

    #[test]
    fn an_http_image_url_becomes_a_url_source_and_anything_else_is_dropped() {
        let block = image_block("https://example.com/cat.png").expect("a url source");
        assert_eq!(block["source"]["type"], "url");
        assert_eq!(block["source"]["url"], "https://example.com/cat.png");
        // Dropping the part beats 400ing the whole turn over it.
        assert!(image_block("cat.png").is_none());
        assert!(image_block("data:image/png,notbase64").is_none());
        assert!(image_block("data:;base64,AAAA").is_none());
    }

    #[test]
    fn a_multimodal_user_message_carries_text_and_image_blocks() {
        let message = ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Parts(vec![
                ContentPart::text("what is this"),
                ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "data:image/jpeg;base64,ZZZZ".into(),
                    },
                },
            ]),
            tool_calls: Vec::new(),
            tool_call_id: None,
        };
        let (_, messages) = build_messages(&[message]);
        let blocks = messages[0]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["source"]["media_type"], "image/jpeg");
    }

    #[test]
    fn tool_specs_flatten_and_rename_parameters_to_input_schema() {
        let spec = json!({
            "type": "function",
            "function": {
                "name": "bash",
                "description": "run a command",
                "parameters": {"type": "object", "properties": {"cmd": {"type": "string"}}},
            }
        });
        let tools = anthropic_tools(std::slice::from_ref(&spec));
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "bash");
        assert_eq!(tools[0]["description"], "run a command");
        assert_eq!(
            tools[0]["input_schema"]["properties"]["cmd"]["type"],
            "string"
        );
        assert!(tools[0].get("parameters").is_none());
        assert!(tools[0].get("type").is_none(), "no function wrapper here");
    }

    #[test]
    fn a_tool_spec_with_no_parameters_still_gets_an_object_schema() {
        let spec = json!({"type": "function", "function": {"name": "ping"}});
        let tools = anthropic_tools(&[spec]);
        assert_eq!(tools[0]["input_schema"]["type"], "object");
    }

    // --- thinking ---------------------------------------------------------

    #[test]
    fn an_effort_mode_sends_adaptive_thinking_with_the_level_in_output_config() {
        let mut cfg = cfg();
        cfg.thinking = Some(ThinkingMode::Effort(ReasoningEffort::XHigh));
        let payload = build_payload(&cfg, &[], &[text("user", "hi")]);
        assert_eq!(payload["thinking"]["type"], "adaptive");
        assert_eq!(payload["thinking"]["display"], "summarized");
        // The level is `output_config`'s — inside `thinking` it is a 400.
        assert_eq!(payload["output_config"]["effort"], "xhigh");
        assert!(payload["thinking"].get("effort").is_none());
        assert!(payload["thinking"].get("budget_tokens").is_none());
    }

    #[test]
    fn an_effort_less_reasoner_sends_the_budgeted_form() {
        // `On` is only ever produced for a model whose record listed no
        // effort levels, which on this provider is the budgeted family.
        let mut cfg = cfg();
        cfg.thinking = Some(ThinkingMode::On);
        let payload = build_payload(&cfg, &[], &[text("user", "hi")]);
        assert_eq!(payload["thinking"]["type"], "enabled");
        assert_eq!(payload["thinking"]["budget_tokens"], THINKING_BUDGET);
        assert!(payload.get("output_config").is_none());
    }

    #[test]
    fn thinking_off_disables_it_explicitly() {
        let mut cfg = cfg();
        cfg.thinking = Some(ThinkingMode::Off);
        let payload = build_payload(&cfg, &[], &[text("user", "hi")]);
        assert_eq!(payload["thinking"]["type"], "disabled");
        assert!(payload.get("output_config").is_none());
    }

    #[test]
    fn no_thinking_mode_sends_no_thinking_field() {
        let payload = build_payload(&cfg(), &[], &[text("user", "hi")]);
        assert!(payload.get("thinking").is_none());
    }

    #[test]
    fn the_thinking_budget_fits_inside_the_output_cap() {
        // The API refuses a budget that meets or exceeds `max_tokens`, and
        // one below 1024.
        const { assert!(THINKING_BUDGET < MAX_TOKENS) };
        const { assert!(THINKING_BUDGET >= 1024) };
    }

    // --- prompt caching ---------------------------------------------------

    #[test]
    fn cache_breakpoints_mark_the_system_prefix_and_the_frontier() {
        let messages = [
            text("system", "persona"),
            text("user", "one"),
            text("assistant", "answer"),
            text("user", "two"),
        ];
        let payload = build_payload(&cfg(), &[], &messages);
        let ephemeral = json!({"type": "ephemeral"});
        assert_eq!(payload["system"][0]["cache_control"], ephemeral);
        let wire = payload["messages"].as_array().unwrap();
        assert_eq!(wire.len(), 3);
        // The frontier (the newest message) and the user message before it.
        assert_eq!(wire[2]["content"][0]["cache_control"], ephemeral);
        assert_eq!(wire[0]["content"][0]["cache_control"], ephemeral);
        // Never the one in between, so two breakpoints are never the same.
        assert!(wire[1]["content"][0].get("cache_control").is_none());
    }

    #[test]
    fn cache_breakpoints_stay_under_the_api_limit() {
        let messages = [
            text("system", "persona"),
            text("user", "one"),
            text("assistant", "a"),
            text("user", "two"),
            text("assistant", "b"),
            text("user", "three"),
        ];
        let payload = build_payload(&cfg(), &[], &messages);
        let count = payload.to_string().matches("cache_control").count();
        assert!(
            count <= super::super::cache::MAX_BREAKPOINTS,
            "{count} breakpoints placed"
        );
        assert_eq!(count, 3, "system + frontier + the user before it");
    }

    #[test]
    fn whitespace_system_blocks_cannot_take_the_cache_breakpoint() {
        let messages = [
            text("system", "persona"),
            text("system", " \n\t "),
            text("developer", "   "),
            text("user", "hi"),
        ];
        let payload = build_payload(&cfg(), &[], &messages);
        assert_eq!(payload["system"].as_array().unwrap().len(), 1);
        assert_eq!(payload["system"][0]["text"], "persona");
        assert_eq!(
            payload["system"][0]["cache_control"],
            json!({"type": "ephemeral"})
        );

        let blank = build_payload(&cfg(), &[], &[text("system", " \n "), text("user", "hi")]);
        assert!(blank.get("system").is_none());
    }

    #[test]
    fn merged_tool_batches_and_large_image_followups_keep_the_previous_frontier() {
        let mut messages = vec![text("system", "persona"), text("user", "run it")];
        for round in 0..32 {
            let calls: Vec<_> = (0..32)
                .map(|call| {
                    super::super::ToolCallSpec::function(format!("r{round}-c{call}"), "bash", "{}")
                })
                .collect();
            messages.push(ChatMessage::assistant_tool_calls("checking", calls.clone()));
            for call in calls {
                messages.push(ChatMessage::tool_result(
                    &call.id,
                    format!("result {}", call.id),
                ));
            }
            let payload = build_payload(&cfg(), &[], &messages);
            let wire = payload["messages"].as_array().unwrap();
            let frontier = wire.last().unwrap()["content"].as_array().unwrap();
            assert_eq!(
                frontier.len(),
                32,
                "a batch is merged into one user message"
            );
            let previous = wire[wire.len() - 3]["content"].as_array().unwrap();
            assert!(previous.last().unwrap().get("cache_control").is_some());
            assert!(frontier.last().unwrap().get("cache_control").is_some());
            assert_eq!(payload.to_string().matches("cache_control").count(), 3);
        }

        messages.push(text("assistant", "done"));
        messages.push(ChatMessage::with_parts(
            "user",
            (0..32)
                .map(|_| ContentPart::image("https://example.test/image.png"))
                .collect(),
        ));
        let payload = build_payload(&cfg(), &[], &messages);
        let wire = payload["messages"].as_array().unwrap();
        let previous = wire[wire.len() - 3]["content"].as_array().unwrap();
        assert!(previous.last().unwrap().get("cache_control").is_some());
        let frontier = wire.last().unwrap()["content"].as_array().unwrap();
        assert_eq!(frontier.last().unwrap()["type"], "image");
        assert!(frontier.last().unwrap().get("cache_control").is_some());
        assert_eq!(payload.to_string().matches("cache_control").count(), 3);
    }

    // --- the event stream -------------------------------------------------

    fn fold(frames: &[&str]) -> (Vec<Delta>, MessageAccumulator, Vec<Step>) {
        let mut acc = MessageAccumulator::default();
        let mut deltas = Vec::new();
        let mut steps = Vec::new();
        for frame in frames {
            let step = acc.on_frame(frame);
            if let Step::Delta(delta) = &step {
                deltas.push(delta.clone());
            }
            steps.push(step);
        }
        (deltas, acc, steps)
    }

    #[test]
    fn text_deltas_become_response_text() {
        let (deltas, _, _) = fold(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}"#,
        ]);
        let joined: String = deltas.iter().map(|d| d.response.as_str()).collect();
        assert_eq!(joined, "Hello");
        assert!(deltas.iter().all(|d| d.reasoning.is_empty()));
    }

    #[test]
    fn thinking_deltas_become_reasoning_and_signatures_are_ignored() {
        let (deltas, _, steps) = fold(&[
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"EqQB"}}"#,
        ]);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].reasoning, "hmm");
        assert_eq!(steps[1], Step::Continue, "a signature is not text");
    }

    #[test]
    fn a_tool_use_block_accumulates_partial_json_into_one_call() {
        // The name arrives whole on the block start; the arguments dribble in
        // as *partial JSON strings* and only make sense concatenated.
        let (deltas, acc, _) = fold(&[
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"get_weather","input":{}}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"location\":"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" \"Paris\"}"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
        ]);
        let (calls, _, _) = acc.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "toolu_1");
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].arguments, r#"{"location": "Paris"}"#);
        // Counted, never rendered — the tally ticks while the call is made.
        assert_eq!(deltas[0].tool_call, "get_weather");
        assert!(deltas.iter().all(|d| d.response.is_empty()));
    }

    #[test]
    fn two_tool_use_blocks_are_kept_apart_by_their_index() {
        // A parallel batch interleaves its blocks, so a single "current call"
        // would splice one call's arguments into another's.
        let (_, acc, _) = fold(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"read","input":{}}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"b","name":"bash","input":{}}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"ls\"}"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"x\"}"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_stop","index":1}"#,
        ]);
        let (calls, _, _) = acc.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "a");
        assert_eq!(calls[0].arguments, r#"{"path":"x"}"#);
        assert_eq!(calls[1].id, "b");
        assert_eq!(calls[1].arguments, r#"{"cmd":"ls"}"#);
    }

    #[test]
    fn a_parameterless_call_closes_with_an_empty_object() {
        // The empty string is not JSON any provider can read back.
        let (_, acc, _) = fold(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"ping","input":{}}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
        ]);
        let (calls, _, _) = acc.finish();
        assert_eq!(calls[0].arguments, "{}");
    }

    #[test]
    fn a_text_block_stop_closes_no_call() {
        let (_, acc, _) = fold(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
        ]);
        let (calls, _, _) = acc.finish();
        assert!(calls.is_empty());
    }

    #[test]
    fn the_usage_sums_the_uncached_remainder_with_both_cache_figures() {
        // Anthropic reports `input_tokens` as what was NOT served from or
        // written to cache; the crate's `input` means the whole prompt.
        let (_, acc, _) = fold(&[
            r#"{"type":"message_start","message":{"usage":{"input_tokens":25,"cache_creation_input_tokens":100,"cache_read_input_tokens":900,"output_tokens":1}}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#,
        ]);
        let (_, reason, usage) = acc.finish();
        let usage = usage.expect("a usage frame landed");
        assert_eq!(usage.input, 1025);
        assert_eq!(usage.cached, 900);
        assert_eq!(usage.cache_write, 100);
        // The output count is cumulative — the later frame wins.
        assert_eq!(usage.output, 42);
        assert_eq!(reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn a_delta_that_repeats_the_input_side_without_the_cache_keys_keeps_them() {
        // The real wire repeats all four counters on `message_delta`
        // (verified live: `{"input_tokens":10,"cache_creation_input_tokens":7998,
        // "cache_read_input_tokens":0,"output_tokens":5}`), cumulative. A
        // frame that names the uncached remainder *without* the cache keys —
        // the documented older shape, a shim's abbreviation — must not read
        // as "the cache figures are now zero": the whole-prompt `input` and
        // the cached share are what the receipt and the gauge show.
        let (_, acc, _) = fold(&[
            r#"{"type":"message_start","message":{"usage":{"input_tokens":25,"cache_creation_input_tokens":100,"cache_read_input_tokens":900,"output_tokens":1}}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":25,"output_tokens":42}}"#,
        ]);
        let usage = acc.finish().2.expect("usage");
        assert_eq!(usage.input, 1025, "the whole prompt, not the remainder");
        assert_eq!(usage.cached, 900);
        assert_eq!(usage.cache_write, 100);
        assert_eq!(usage.output, 42);
    }

    #[test]
    fn a_delta_that_grows_the_input_side_wins_over_the_start() {
        // Cumulative means a later frame can only report *more*: a server
        // tool round grows the prompt mid-message (the docs' own example
        // goes 2679 → 10682 on the delta), and the delta's larger figures
        // are the ones the round billed.
        let (_, acc, _) = fold(&[
            r#"{"type":"message_start","message":{"usage":{"input_tokens":2679,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":3}}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":10682,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":510}}"#,
        ]);
        let usage = acc.finish().2.expect("usage");
        assert_eq!(usage.input, 10_682);
        assert_eq!(usage.output, 510);
    }

    #[test]
    fn a_usage_frame_without_the_input_side_keeps_what_the_start_reported() {
        let (_, acc, _) = fold(&[
            r#"{"type":"message_start","message":{"usage":{"input_tokens":500,"cache_read_input_tokens":0}}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
        ]);
        let usage = acc.finish().2.expect("usage");
        assert_eq!(usage.input, 500, "not zeroed by the later frame");
        assert_eq!(usage.output, 7);
    }

    #[test]
    fn a_tool_use_stop_reason_speaks_the_crates_own_vocabulary() {
        // The agent loop and the transcript read one word for this whatever
        // the provider called it.
        let (_, acc, _) = fold(&[
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}"#,
        ]);
        assert_eq!(acc.finish().1.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn message_stop_ends_the_stream() {
        let (_, _, steps) = fold(&[r#"{"type":"message_stop"}"#]);
        assert_eq!(steps[0], Step::Done);
    }

    #[test]
    fn a_ping_and_an_unknown_frame_are_ignored() {
        let (_, _, steps) = fold(&[
            r#"{"type":"ping"}"#,
            r#"{"type":"from_the_future"}"#,
            "not json",
        ]);
        assert!(steps.iter().all(|s| *s == Step::Continue));
    }

    #[test]
    fn an_in_band_error_frame_fails_the_stream_with_its_message() {
        let (_, _, steps) = fold(&[
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        ]);
        assert_eq!(steps[0], Step::Failed("Overloaded".to_string()));
    }

    #[test]
    fn a_refusal_with_nothing_streamed_fails_rather_than_answering_blank() {
        // A refusal is an HTTP 200; without this the turn ends with an empty
        // bubble and no reason.
        let (_, _, steps) = fold(&[
            r#"{"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber"}},"usage":{"output_tokens":0}}"#,
        ]);
        let Step::Failed(reason) = &steps[0] else {
            panic!("a bare refusal must fail the turn: {steps:?}");
        };
        assert!(reason.contains("declined"), "{reason}");
        assert!(reason.contains("cyber"), "{reason}");
    }

    #[test]
    fn a_refusal_after_text_keeps_what_was_streamed() {
        let (_, _, steps) = fold(&[
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"refusal"},"usage":{"output_tokens":3}}"#,
        ]);
        assert_eq!(steps[1], Step::Continue, "the partial is the answer");
    }

    #[test]
    fn the_request_borrows_a_pictures_bytes_rather_than_copying_them() {
        // The whole point of the typed request (`docs/memory.md`): an image
        // block's `data` is a slice of the session's one shared encoding.
        let url = crate::llm::AttachmentUrl::from("data:image/png;base64,AAAA");
        let messages_in = [ChatMessage::with_parts(
            "user",
            vec![ContentPart::text("look"), ContentPart::image(url.clone())],
        )];
        let (_, messages) = blocks_of(&messages_in);
        let Block::Image {
            source: Source::Base64 { data, media_type },
            ..
        } = &messages[0].content[1]
        else {
            panic!("an image block: {:?}", messages[0].content);
        };
        let payload = &url["data:image/png;base64,".len()..];
        assert!(
            std::ptr::eq(data.as_ptr(), payload.as_ptr()) && data.len() == payload.len(),
            "the block points into the shared encoding"
        );
        assert_eq!(*media_type, "image/png");
    }
}
