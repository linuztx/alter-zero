//! OpenAI-compatible streaming client.
//!
//! Talks to anything accepting the `/chat/completions` shape (OpenAI, OpenRouter,
//! Groq, Together, Sambanova, Ollama, …). The endpoint/payload builders and the
//! SSE frame parse are pure and unit-tested; [`OpenAiClient::stream_chat`] is the
//! boundary that opens the blocking request and drains it. See `docs/llm.md`.

use std::borrow::Cow;
use std::io::Read;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use super::body::{BodyReader, BodySource, streamed_request};
use super::config::ModelConfig;
use super::thinking::ThinkingSplitter;
use super::tools::ToolCallRequest;
use super::{ChatMessage, LlmError, Result};
use crate::stream::{CancelToken, TokenUsage};

/// The HTTP client's per-operation deadline (see [`super::http_client`]): it
/// bounds the whole send/header exchange — connect, uploading the request
/// body, awaiting the response headers — and each individual body read. This
/// is a **stall detector**, not the cancel wake (the drain polls the
/// [`CancelToken`] on its own [`CANCEL_POLL_INTERVAL`] cadence), so it is set
/// generously: a vision request uploads megabytes of base64 `data:` URLs, and
/// a busy provider can sit far past any snappy deadline before its first
/// header or between tokens. Its predecessor — a 3 s `STREAM_OP_TIMEOUT`
/// doubling as the cancel wake — failed exactly those requests: a pasted
/// image whose upload couldn't fit 3 s spent its whole retry budget re-hitting
/// the same wall, and a slow header exchange flashed spurious
/// `retrying n/3` counters a few seconds into a normal turn.
///
/// The `/models` fetch ([`super::models::fetch_models`]) deliberately shares
/// this exact deadline: [`super::http_client`] caches one client **per
/// timeout**, so a models-only deadline built a second full client stack — its
/// own blocking-runtime thread, connection pool and TLS config — beside the
/// chat client, held for the life of the process. One timeout means one
/// client: `/model` and the chat turns ride the same pool, and the picker's
/// fetch warms the very connection the next turn reuses.
pub(crate) const NET_OP_TIMEOUT: Duration = Duration::from_secs(120);

/// How often [`drain_stream`] wakes to poll the [`CancelToken`] while the
/// transport channel is quiet — the Esc-interrupt/quit acknowledgement
/// latency, matching `retry::sleep_cancellable`'s slice.
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A split delta surfaced to the caller each SSE frame: the visible response
/// text and the (hidden) reasoning text, already peeled apart by the
/// [`ThinkingSplitter`], plus the raw tool-call fragment (`name`/`arguments`
/// pieces) streamed this frame — surfaced so the caller can count the tokens
/// the model spends *generating* a tool call, the same way reasoning is counted
/// (never rendered; see `docs/status-indicator.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Delta {
    pub response: String,
    pub reasoning: String,
    pub tool_call: String,
}

/// What one completed stream produced: the accumulated text/reasoning, the
/// tool calls the model requested (empty on a plain answer), the
/// `finish_reason` the provider reported, and the provider's real token
/// usage when its final frame carried one (`stream_options.include_usage` —
/// the backend forwards it as [`crate::stream::StreamEvent::Usage`]). The
/// backend inspects `tool_calls` to decide whether to run tools and loop
/// again (see `docs/tools.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamOutcome {
    pub text: super::thinking::ChatStreamResult,
    pub tool_calls: Vec<ToolCallRequest>,
    pub finish_reason: Option<String>,
    pub usage: Option<TokenUsage>,
}

/// A streaming chat client bound to one [`ModelConfig`], optionally carrying the
/// tool definitions (`tools` array) offered to the model.
#[derive(Debug, Clone)]
pub struct OpenAiClient {
    cfg: ModelConfig,
    /// The Chat Completions `tools` array (empty → no tools, `tool_choice`
    /// omitted). Set via [`OpenAiClient::with_tools`].
    tools: Vec<serde_json::Value>,
}

impl OpenAiClient {
    #[must_use]
    pub fn new(cfg: ModelConfig) -> Self {
        Self {
            cfg,
            tools: Vec::new(),
        }
    }

    /// Offer these tool definitions to the model (added to the payload as the
    /// `tools` array with `tool_choice:"auto"`). Empty leaves the request
    /// tool-free.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<serde_json::Value>) -> Self {
        self.tools = tools;
        self
    }

    /// Point this client at a different **model** on the same provider — an
    /// agent definition's `model:` (`docs/subagents.md`).
    ///
    /// The thinking mode, the vision verdict and the speed tier are dropped
    /// with the swap: all three were detected for the model being replaced,
    /// and sending a `reasoning` parameter to a model that doesn't reason —
    /// or a `service_tier` to one that lists none (`docs/fast-mode.md`) — is
    /// how a provider comes to reject the whole request. Everything else —
    /// base, key, headers, provider kwargs, temperature, cache key — is the
    /// provider's and stays.
    #[must_use]
    pub fn with_model(mut self, model: &str) -> Self {
        self.cfg.model = model.to_string();
        self.cfg.thinking = None;
        self.cfg.vision = None;
        self.cfg.service_tier = None;
        self
    }

    /// The chat-completions endpoint (`{api_base}/chat/completions`), falling
    /// back to OpenAI when the base is empty.
    #[must_use]
    pub fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url(None))
    }

    /// The base every request is built on: the auth seam's override when it
    /// named one (a Copilot Business seat's own host), else the configured
    /// base, else OpenAI itself. Trailing slash trimmed.
    fn base_url(&self, override_base: Option<&str>) -> String {
        let base = override_base
            .filter(|b| !b.is_empty())
            .unwrap_or(&self.cfg.api_base);
        if base.is_empty() {
            "https://api.openai.com/v1".to_string()
        } else {
            base.trim_end_matches('/').to_string()
        }
    }

    /// Where this request goes — the path is the wire format's, not the
    /// provider's: Responses and Chat Completions are different endpoints on
    /// the same base (`docs/chatgpt.md`).
    fn request_url(&self, override_base: Option<&str>) -> String {
        let base = self.base_url(override_base);
        match self.cfg.wire_api {
            super::WireApi::Responses => format!("{base}/responses"),
            super::WireApi::Anthropic => format!("{base}/messages"),
            super::WireApi::Ollama => format!("{base}/api/chat"),
            super::WireApi::Chat => format!("{base}/chat/completions"),
        }
    }

    /// The streamed request body as a JSON tree — `chat_request` serialized
    /// whole. The **tests'** view of the request; the wire takes
    /// [`request_stream`](Self::request_stream), which never builds this tree.
    #[must_use]
    pub fn build_payload(&self, messages: &[ChatMessage]) -> serde_json::Value {
        serde_json::to_value(self.chat_request(Cow::Borrowed(messages)))
            .unwrap_or(serde_json::Value::Null)
    }

    /// The Chat Completions request: model, messages, `stream: true` (with
    /// the standard `stream_options.include_usage` asking for the final
    /// usage frame the app's token tally snaps to), the optional
    /// temperature, the prompt-caching fields (`docs/prompt-caching.md` —
    /// explicit `cache_control` breakpoints for the models that need them,
    /// and the session's cache-affinity key as `prompt_cache_key` /
    /// OpenRouter's `session_id`), any provider `extra_body` (e.g.
    /// `venice_parameters`) merged in, and — for a reasoning-capable model —
    /// the active thinking mode (`docs/reasoning.md`).
    ///
    /// Every field but the messages lives in one small tree; the messages
    /// ride **by reference** and are serialized in place. A `Value` of the
    /// conversation copied a pasted picture's megabytes of base64 once more
    /// per round, which is what `docs/memory.md` traces the growing process
    /// to. Only an explicit-caching model takes a copy — a shallow one, its
    /// pictures shared — to carry the breakpoints.
    fn chat_request<'a>(&'a self, messages: Cow<'a, [ChatMessage]>) -> ChatRequest<'a> {
        let mut fields = serde_json::Map::new();
        fields.insert("stream".to_string(), json!(true));
        // Both shipped providers honour the standard OpenAI switch and
        // answer with a final usage frame (cache detail included). A
        // provider whose shim chokes can override it from its kwargs —
        // the extra_body merge below wins over this base.
        fields.insert("stream_options".to_string(), json!({"include_usage": true}));
        // An explicit-caching model (anthropic/qwen via an OpenRouter-style
        // aggregator) only caches marked blocks: mark a copy of the messages
        // with the ephemeral breakpoints. Implicit-caching providers (OpenAI,
        // Venice, …) keep the untouched plain-string form.
        let messages = if super::cache::needs_cache_breakpoints(&self.cfg.model) {
            let mut marked = messages.into_owned();
            super::cache::apply_cache_breakpoints(&mut marked);
            Cow::Owned(marked)
        } else {
            messages
        };
        if let Some(key) = self
            .cfg
            .cache_key
            .as_deref()
            .filter(|k| !k.is_empty())
            // GitHub Copilot's proxy answers a request shape it doesn't
            // recognise with `model_not_supported` — an error that blames the
            // *model* — and its own caching is server-side, so the affinity
            // key buys nothing there worth that risk. The request carries the
            // same value as `X-Request-Id` instead (`docs/copilot.md`).
            .filter(|_| self.cfg.auth != super::AuthScheme::GithubCopilot)
        {
            // The per-session affinity key: the standard OpenAI
            // `prompt_cache_key` (accepted by OpenRouter and Venice alike),
            // plus OpenRouter's own `session_id` so its routing pins the
            // session to one upstream provider — a cache written on one
            // provider is unreadable from another. Venice rejects unknown
            // keys ("Unrecognized key(s)"), so `session_id` stays
            // OpenRouter-gated.
            fields.insert("prompt_cache_key".to_string(), json!(key));
            if self
                .cfg
                .api_base
                .to_ascii_lowercase()
                .contains("openrouter")
            {
                fields.insert("session_id".to_string(), json!(key));
            }
        }
        if let Some(t) = self.cfg.temperature {
            fields.insert("temperature".to_string(), json!(t));
        }
        // The selected speed tier (`docs/fast-mode.md`) — OpenAI's own
        // `service_tier` parameter, which chat completions take exactly as
        // the Responses wire does. Only ever a tier the model's record
        // listed, so no provider here sees a field its shim would refuse.
        if let Some(tier) = self.cfg.service_tier.as_deref() {
            fields.insert("service_tier".to_string(), json!(tier));
        }
        if !self.tools.is_empty() {
            fields.insert("tools".to_string(), json!(self.tools));
            fields.insert("tool_choice".to_string(), json!("auto"));
        }
        for (k, v) in &self.cfg.extra_body {
            fields.insert(k.clone(), v.clone());
        }
        // The thinking mode is applied *after* the extra_body merge so the
        // user's Ctrl+T choice wins over a file-configured static (the
        // footer must never claim an effort a stale `disable_thinking: true`
        // silently vetoes).
        if let Some(mode) = self.cfg.thinking {
            // GitHub Copilot's chat-completions endpoint spells the mode as a
            // top-level `reasoning_effort` string, and the `reasoning` object
            // is an unknown field there — one its proxy answers with a
            // `model_not_supported` 400 that blames the model rather than the
            // payload. So it is the shape *instead of*, never alongside
            // (`docs/copilot.md`).
            if self.cfg.auth == super::AuthScheme::GithubCopilot {
                if let Some(effort) = super::reasoning::effort_label(mode) {
                    fields.insert("reasoning_effort".to_string(), json!(effort));
                }
                return ChatRequest {
                    model: Cow::Borrowed(&self.cfg.model),
                    messages,
                    fields,
                };
            }
            if let Some(body) = super::reasoning::reasoning_body(mode) {
                fields.insert("reasoning".to_string(), body);
            }
            // Venice ignores `reasoning.enabled` — `venice_parameters.
            // disable_thinking` is the toggle it honours. A provider carrying
            // a venice_parameters table (the Venice-family marker) gets it
            // synced to the mode, other table keys preserved; providers
            // without the table (OpenRouter) keep a clean payload.
            if let Some(venice) = fields
                .get_mut("venice_parameters")
                .and_then(serde_json::Value::as_object_mut)
            {
                venice.insert(
                    "disable_thinking".to_string(),
                    json!(mode == super::reasoning::ThinkingMode::Off),
                );
            }
        }
        ChatRequest {
            model: Cow::Borrowed(&self.cfg.model),
            messages,
            fields,
        }
    }

    /// This request's body **as a stream**, in whichever wire format the
    /// provider speaks, with the byte count the `Content-Length` header
    /// carries: a serializer thread writes the JSON into a bounded pipe of
    /// small chunks and the transport reads it out as it uploads
    /// ([`streamed_request`]). Every wire writes its request from the
    /// messages **by reference** — a picture's base64 is the session's one
    /// shared encoding, borrowed — so a body carrying a picture is never held
    /// whole and never copied: not built, not buffered, not freed behind the
    /// send, which is what keeps a turn from allocating anything
    /// picture-sized at all (`docs/memory.md`). `messages` is taken by
    /// value: the round's copy of the conversation, its pictures shared,
    /// moves to that thread.
    ///
    /// # Errors
    /// A message that can't be serialized — which no [`ChatMessage`] is.
    pub fn request_stream(&self, messages: Vec<ChatMessage>) -> Result<(BodyReader, u64)> {
        match self.cfg.wire_api {
            super::WireApi::Chat => {
                streamed_request(self.chat_request(Cow::Owned(messages)).into_owned())
            }
            super::WireApi::Responses => streamed_request(super::responses::RequestSource::new(
                self.cfg.clone(),
                self.tools.clone(),
                messages,
            )),
            super::WireApi::Anthropic => streamed_request(super::anthropic::RequestSource::new(
                self.cfg.clone(),
                self.tools.clone(),
                messages,
            )),
            super::WireApi::Ollama => streamed_request(super::ollama::RequestSource::new(
                self.cfg.clone(),
                self.tools.clone(),
                messages,
            )),
        }
    }

    /// The per-request headers GitHub Copilot needs on top of the static
    /// identity `providers.toml` carries — a no-op for every other provider.
    ///
    /// - `X-Initiator` is **billing-relevant**: GitHub charges a premium
    ///   request for a user-initiated round and nothing for the agent's own
    ///   tool round trips, so a turn that marked every round `user` would bill
    ///   the user several times over for one question.
    /// - `Copilot-Vision-Request` is mandatory whenever a round carries an
    ///   image; without it the API answers `400 missing required
    ///   Copilot-Vision-Request header for vision requests`.
    /// - `X-Request-Id` is what GitHub support asks for when a request
    ///   misbehaves; the cache key doubles as one, being per-session.
    fn copilot_request_headers(
        &self,
        req: reqwest::blocking::RequestBuilder,
        messages: &[ChatMessage],
    ) -> reqwest::blocking::RequestBuilder {
        if self.cfg.auth != super::AuthScheme::GithubCopilot {
            return req;
        }
        let mut req = req.header("x-initiator", super::copilot::initiator(messages));
        if super::copilot::has_image(messages) {
            req = req.header("copilot-vision-request", "true");
        }
        if let Some(key) = &self.cfg.cache_key {
            req = req.header("x-request-id", key);
        }
        req
    }

    /// The per-request headers the ChatGPT backend needs beside the
    /// credential's own — a no-op for every other provider.
    ///
    /// The backend keys its prompt cache on Codex's `session_id` /
    /// `conversation_id` headers, not on the body's `prompt_cache_key`
    /// (verified live: 0 cached tokens on an identical 6.7k-token prefix
    /// without them, 6.4k with them — `docs/chatgpt.md`). The per-session
    /// cache key rides both, so an agentic session's rounds land on the
    /// warm cache exactly as they do on every other provider.
    fn chatgpt_request_headers(
        &self,
        mut req: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        if self.cfg.auth != super::AuthScheme::ChatGptCodex {
            return req;
        }
        for (name, value) in super::chatgpt::session_headers(self.cfg.cache_key.as_deref()) {
            req = req.header(name, value);
        }
        // Codex's routing hint: the model on every request, and the speed
        // tier beside it when one is selected (`docs/fast-mode.md`) — the
        // edge's own note of where a priority request should go.
        req = req.header(
            super::chatgpt::ROUTING_HINT_HEADER,
            super::chatgpt::routing_hint(&self.cfg.model, self.cfg.service_tier.as_deref()),
        );
        req
    }

    /// Stream a completion, invoking `on_delta` for every non-empty split delta.
    /// Polls `cancel` between SSE frames and stops promptly when it trips.
    ///
    /// Returns the final accumulated split. Boundary code — real HTTP.
    ///
    /// # Errors
    /// Transport failures, non-2xx responses, and a tripped `cancel` become an
    /// [`LlmError`].
    pub fn stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        cancel: &CancelToken,
        on_delta: impl FnMut(Delta),
    ) -> Result<StreamOutcome> {
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        let client = super::http_client(NET_OP_TIMEOUT)?;
        // What the request authenticates with, and where it goes. For every
        // ordinary provider that is the stored key and the configured base,
        // resolved with no I/O; GitHub Copilot's stored value is an OAuth
        // token, exchanged (cached) here for the bearer its API takes and the
        // account's own host — a Business seat is served from a different one
        // than the file names. See `docs/copilot.md`.
        let auth = super::auth::request_auth(&self.cfg)?;
        let url = self.request_url(auth.base.as_deref());
        // Ollama streams NDJSON rather than SSE; the header names which.
        let accept = match self.cfg.wire_api {
            super::WireApi::Ollama => "application/x-ndjson",
            _ => "text/event-stream",
        };
        let mut req = client
            .post(url)
            .header("accept", accept)
            .header("content-type", "application/json");
        if let Some(key) = &auth.bearer {
            req = req.bearer_auth(key);
        }
        for (k, v) in &self.cfg.extra_headers {
            req = req.header(k, v);
        }
        // The credential's own identity — the account a ChatGPT request is
        // made on behalf of, and the client identity that account expects.
        for (k, v) in &auth.headers {
            req = req.header(k, v);
        }
        req = self.copilot_request_headers(req, &messages);
        req = self.chatgpt_request_headers(req);
        // The body streams out of a serializer thread as the transport
        // uploads it — never held whole (`docs/memory.md`).
        let (body, len) = self.request_stream(messages)?;
        req = req.body(reqwest::blocking::Body::sized(body, len));

        // All blocking network I/O — the send/header exchange and every body
        // read — runs on a detached transport thread feeding this channel, so
        // the drain below can poll `cancel` every CANCEL_POLL_INTERVAL no
        // matter how long the network blocks (a megabyte image upload, a
        // provider sitting on the headers, a mid-stream stall). On cancel the
        // receiver drops; the transport exits at its next channel send — or
        // when its current network operation hits NET_OP_TIMEOUT on a silent
        // socket — detached and bounded, never joined (the loop's
        // detach-don't-join discipline, docs/interrupt.md).
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || run_transport(req, &tx));
        let outcome = match self.cfg.wire_api {
            super::WireApi::Responses => drain_responses(&rx, cancel, on_delta),
            super::WireApi::Anthropic => drain_anthropic(&rx, cancel, on_delta),
            super::WireApi::Ollama => drain_ollama(&rx, cancel, on_delta),
            super::WireApi::Chat => drain_stream(&rx, cancel, on_delta),
        };
        outcome.map_err(|e| self.explain(e))
    }

    /// Rewrite a failure the provider explains badly. Only the two
    /// subscriptions have such a mapping — Copilot's wire bodies name a `code`
    /// and nothing a user can act on (and the commonest of them,
    /// `model_not_supported`, is not even about the request being malformed),
    /// and a ChatGPT 401 means a sign-in to redo rather than a request to fix.
    /// Every pasted-key provider's error passes through exactly as it did —
    /// except on the Ollama wire, whose failures are mostly about the
    /// *machine* (the server not running, a model not pulled, a capability
    /// the model lacks) and say so in words a user can act on
    /// (`docs/ollama.md`).
    fn explain(&self, error: LlmError) -> LlmError {
        if self.cfg.wire_api == super::WireApi::Ollama {
            return match error {
                LlmError::Http(message) => {
                    let base = self.base_url(None);
                    match super::ollama::connection_advice(&base, &message) {
                        Some(advice) => LlmError::Http(advice),
                        None => LlmError::Http(message),
                    }
                }
                LlmError::Api { status, body } => {
                    match super::ollama::advice(status, &body, &self.cfg.model) {
                        Some(advice) => LlmError::Api {
                            status,
                            body: advice,
                        },
                        None => LlmError::Api { status, body },
                    }
                }
                other => other,
            };
        }
        let LlmError::Api { status, body } = &error else {
            return error;
        };
        let advice = match self.cfg.auth {
            super::AuthScheme::GithubCopilot => {
                super::copilot::chat_advice(*status, body, &self.cfg.model)
            }
            // A ChatGPT request that 401s has an expired sign-in behind it,
            // and the wire body says only `invalid_token` — a sentence the
            // user cannot act on.
            super::AuthScheme::ChatGptCodex => super::chatgpt::auth_advice(*status, body),
            // The Anthropic refusals a user can do something about are
            // several and need different answers — an expired sign-in, a
            // scope, a spend cap (`docs/claude.md`).
            super::AuthScheme::AnthropicConsole => super::claude::auth_advice(*status, body),
            // A pasted Anthropic key meets the same refusals, minus the ones
            // about a sign-in: the advice module tells them apart by status
            // and code, so it serves both.
            super::AuthScheme::ApiKey if self.cfg.wire_api == super::WireApi::Anthropic => {
                super::claude::auth_advice(*status, body)
            }
            super::AuthScheme::ApiKey | super::AuthScheme::OptionalKey => None,
        };
        match advice {
            Some(advice) => LlmError::Api {
                status: *status,
                body: advice,
            },
            None => error,
        }
    }
}

/// The Chat Completions request as it is serialized: the model, the
/// conversation **by reference** (or the marked copy an explicit-caching
/// model takes — shallow, its pictures shared), and every other field in one
/// small tree flattened beside them. Built by
/// [`OpenAiClient::chat_request`], written by [`serialize_exact`].
#[derive(serde::Serialize)]
struct ChatRequest<'a> {
    model: Cow<'a, str>,
    messages: Cow<'a, [ChatMessage]>,
    #[serde(flatten)]
    fields: serde_json::Map<String, serde_json::Value>,
}

impl ChatRequest<'_> {
    /// The request with nothing borrowed — what the serializer thread takes.
    /// A shallow copy: the pictures stay shared.
    fn into_owned(self) -> ChatRequest<'static> {
        ChatRequest {
            model: Cow::Owned(self.model.into_owned()),
            messages: Cow::Owned(self.messages.into_owned()),
            fields: self.fields,
        }
    }
}

impl BodySource for ChatRequest<'static> {
    fn write_to(&self, out: &mut dyn std::io::Write) -> serde_json::Result<()> {
        serde_json::to_writer(out, self)
    }
}

/// The transport thread body: perform the blocking send, then forward the
/// response body to [`drain_stream`] in chunks. Every event on the channel is
/// a `Result` — body bytes, or the failure that ended the stream (a transport
/// error, or a non-2xx status with its body); a clean EOF just drops the
/// sender, the disconnect being the signal. Each blocking operation here is
/// bounded by [`NET_OP_TIMEOUT`]; a failed channel send (the drain dropped its
/// receiver after a cancel) exits early. Boundary code — real HTTP.
fn run_transport(req: reqwest::blocking::RequestBuilder, tx: &Sender<Result<Vec<u8>>>) {
    let mut resp = match req.send() {
        Ok(resp) => resp,
        Err(e) => {
            let _ = tx.send(Err(LlmError::Http(e.to_string())));
            return;
        }
    };
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        let _ = tx.send(Err(LlmError::Api { status, body }));
        return;
    }
    let mut buf = [0u8; 8192];
    loop {
        match resp.read(&mut buf) {
            Ok(0) => return, // EOF — dropping `tx` tells the drain
            Ok(n) => {
                if tx.send(Ok(buf[..n].to_vec())).is_err() {
                    return; // the drain is gone (cancelled) — stop reading
                }
            }
            Err(e) => {
                let _ = tx.send(Err(LlmError::Http(e.to_string())));
                return;
            }
        }
    }
}

/// Drain one streamed response from the transport channel: reassemble SSE
/// lines across arbitrary chunk boundaries, dispatch each through
/// [`process_sse_line`], and poll `cancel` every [`CANCEL_POLL_INTERVAL`]
/// while the channel is quiet — so an Esc/quit is acknowledged promptly even
/// while the transport is parked in a long network operation. A disconnected
/// channel is the clean EOF (any buffered final line flushed first, so a
/// stream ending without a trailing newline still parses). Pure with respect
/// to the network — unit-tested by feeding the channel directly.
/// Drain a **Responses** stream (`docs/chatgpt.md`). The same cancel cadence
/// and transport contract as [`drain_stream`]; only the frame vocabulary
/// differs, so the two produce the identical [`StreamOutcome`] currency and
/// nothing above this module can tell them apart.
///
/// The one structural difference: a tool call arrives **whole**, on a single
/// `response.output_item.done` frame, where Chat Completions dribbles it out
/// as `tool_calls` fragments. So there is no accumulator here — the argument
/// fragments that *do* stream are surfaced for the token tally only.
fn drain_responses(
    rx: &Receiver<Result<Vec<u8>>>,
    cancel: &CancelToken,
    mut on_delta: impl FnMut(Delta),
) -> Result<StreamOutcome> {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
    let mut finish_reason: Option<String> = None;
    let mut usage: Option<TokenUsage> = None;
    pump_lines(rx, cancel, &mut |line: &[u8]| {
        let Ok(frame) = std::str::from_utf8(line) else {
            return SseStep::Continue;
        };
        let Some(data) = sse_data(frame) else {
            return SseStep::Continue;
        };
        if data == "[DONE]" {
            return SseStep::Done;
        }
        match super::responses::parse_event(data) {
            super::responses::ResponseEvent::Text(chunk) => {
                if !chunk.is_empty() {
                    text.push_str(&chunk);
                    on_delta(Delta {
                        response: chunk,
                        ..Delta::default()
                    });
                }
            }
            super::responses::ResponseEvent::Reasoning(chunk) => {
                if !chunk.is_empty() {
                    reasoning.push_str(&chunk);
                    on_delta(Delta {
                        reasoning: chunk,
                        ..Delta::default()
                    });
                }
            }
            super::responses::ResponseEvent::ToolFragment(fragment) => {
                if !fragment.is_empty() {
                    on_delta(Delta {
                        tool_call: fragment,
                        ..Delta::default()
                    });
                }
            }
            super::responses::ResponseEvent::ToolCall(call) => {
                // The name arrives with the finished item, so this is also
                // the first moment the tally can see it.
                on_delta(Delta {
                    tool_call: call.name.clone(),
                    ..Delta::default()
                });
                tool_calls.push(call);
            }
            super::responses::ResponseEvent::Completed { usage: got, status } => {
                if got.is_some() {
                    usage = got;
                }
                // A round that requested tools finished *for* those tools,
                // which is what the agent loop reads to decide to loop again.
                finish_reason = Some(if tool_calls.is_empty() {
                    status.unwrap_or_else(|| "stop".to_string())
                } else {
                    "tool_calls".to_string()
                });
                return SseStep::Done;
            }
            super::responses::ResponseEvent::Failed(message) => {
                return SseStep::Fail(LlmError::Api {
                    status: 0,
                    body: message,
                });
            }
            super::responses::ResponseEvent::Ignored => {}
        }
        SseStep::Continue
    })?;
    Ok(StreamOutcome {
        text: super::thinking::ChatStreamResult {
            response: text,
            reasoning,
        },
        tool_calls,
        finish_reason,
        usage,
    })
}

/// [`drain_stream`]'s Anthropic sibling, over the same [`pump_lines`] byte
/// loop — so Esc is honoured identically on all three wire formats.
///
/// The fold itself lives in [`super::anthropic::MessageAccumulator`]: this
/// API's round is stateful (a tool call opens, accumulates and closes across
/// frames), so the classification is a fold rather than a pure per-frame
/// parse, and keeping it pure is what makes it unit-testable.
fn drain_anthropic(
    rx: &Receiver<Result<Vec<u8>>>,
    cancel: &CancelToken,
    mut on_delta: impl FnMut(Delta),
) -> Result<StreamOutcome> {
    use super::anthropic::Step;

    let mut acc = super::anthropic::MessageAccumulator::default();
    let mut text = String::new();
    let mut reasoning = String::new();
    pump_lines(rx, cancel, &mut |line: &[u8]| {
        let Ok(frame) = std::str::from_utf8(line) else {
            return SseStep::Continue;
        };
        // The `event:` line names the same thing the payload's own `type`
        // does, so only the data is read — one less place the two can
        // disagree.
        let Some(data) = sse_data(frame) else {
            return SseStep::Continue;
        };
        match acc.on_frame(data) {
            Step::Delta(delta) => {
                text.push_str(&delta.response);
                reasoning.push_str(&delta.reasoning);
                on_delta(delta);
                SseStep::Continue
            }
            Step::Continue => SseStep::Continue,
            Step::Done => SseStep::Done,
            Step::Failed(message) => SseStep::Fail(LlmError::Api {
                status: 0,
                body: message,
            }),
        }
    })?;
    let (tool_calls, finish_reason, usage) = acc.finish();
    Ok(StreamOutcome {
        text: super::thinking::ChatStreamResult {
            response: text,
            reasoning,
        },
        tool_calls,
        finish_reason,
        usage,
    })
}

/// [`drain_stream`]'s Ollama sibling, over the same [`pump_lines`] byte
/// loop — an NDJSON stream is one JSON object per line, which is exactly
/// what the line pump already hands out — so Esc is honoured identically on
/// all four wire formats. The fold is [`super::ollama::ChatAccumulator`]'s.
fn drain_ollama(
    rx: &Receiver<Result<Vec<u8>>>,
    cancel: &CancelToken,
    mut on_delta: impl FnMut(Delta),
) -> Result<StreamOutcome> {
    use super::ollama::Step;

    let mut acc = super::ollama::ChatAccumulator::default();
    pump_lines(rx, cancel, &mut |line: &[u8]| {
        let Ok(text) = std::str::from_utf8(line) else {
            return SseStep::Continue;
        };
        match acc.on_line(text) {
            Step::Delta(delta) => {
                on_delta(delta);
                // The final frame can carry text of its own; it is still the
                // final frame.
                if acc.is_finished() {
                    SseStep::Done
                } else {
                    SseStep::Continue
                }
            }
            Step::Continue => SseStep::Continue,
            Step::Done => SseStep::Done,
            Step::Failed(message) => SseStep::Fail(LlmError::Api {
                status: 0,
                body: message,
            }),
        }
    })?;
    if let Some(tail) = acc.flush() {
        on_delta(tail);
    }
    let text = acc.text();
    let (tool_calls, finish_reason, usage) = acc.finish();
    Ok(StreamOutcome {
        text,
        tool_calls,
        finish_reason,
        usage,
    })
}

/// Split the transport's bytes into SSE lines and hand each to `on_line`,
/// polling `cancel` between chunks so an Esc is honoured within
/// [`CANCEL_POLL_INTERVAL`] however long the network blocks. Returns at the
/// first [`SseStep::Done`], at clean EOF (the final partial line flushed
/// first), or with the failure that ended the stream.
///
/// Shared by both wire formats — the cancel discipline and the transport
/// contract are the same however the frames are spelled.
fn pump_lines(
    rx: &Receiver<Result<Vec<u8>>>,
    cancel: &CancelToken,
    on_line: &mut impl FnMut(&[u8]) -> SseStep,
) -> Result<()> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        match rx.recv_timeout(CANCEL_POLL_INTERVAL) {
            Ok(Ok(chunk)) => {
                for &byte in &chunk {
                    match byte {
                        b'\n' => {
                            let step = on_line(&line);
                            line.clear();
                            match step {
                                SseStep::Continue => {}
                                SseStep::Done => return Ok(()),
                                SseStep::Fail(err) => return Err(err),
                            }
                        }
                        b'\r' => {}
                        byte => line.push(byte),
                    }
                }
            }
            Ok(Err(err)) => return Err(err),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if !line.is_empty()
                    && let SseStep::Fail(err) = on_line(&line)
                {
                    return Err(err);
                }
                return Ok(());
            }
        }
    }
}

fn drain_stream(
    rx: &Receiver<Result<Vec<u8>>>,
    cancel: &CancelToken,
    mut on_delta: impl FnMut(Delta),
) -> Result<StreamOutcome> {
    let mut splitter = ThinkingSplitter::new();
    let mut tools = ToolCallAccumulator::default();
    let mut finish_reason: Option<String> = None;
    let mut usage: Option<TokenUsage> = None;
    // A response that ignored `stream:true` has no SSE framing at all —
    // collect its raw lines (bounded) so EOF can fall back to parsing one
    // plain JSON completion instead of reporting a clean empty stream.
    let mut saw_sse_framing = false;
    let mut raw_body: Vec<u8> = Vec::new();
    pump_lines(rx, cancel, &mut |line: &[u8]| {
        if !saw_sse_framing {
            if is_sse_framing(line) {
                saw_sse_framing = true;
                raw_body.clear();
            } else if raw_body.len() + line.len() < RAW_BODY_MAX_BYTES {
                raw_body.extend_from_slice(line);
                raw_body.push(b'\n');
            } // past the cap the tail is dropped — the fallback parse then errors
        }
        process_sse_line(
            line,
            &mut splitter,
            &mut tools,
            &mut finish_reason,
            &mut usage,
            &mut on_delta,
        )
    })?;
    // No SSE framing at all: the provider answered with one plain body — a
    // shim that ignored `stream:true` returning a whole JSON completion, or a
    // bare JSON error on a 200. Parse it as one payload; an unparseable
    // non-empty body is a decode failure, never a clean empty finish.
    if !saw_sse_framing {
        let body_text = String::from_utf8_lossy(&raw_body);
        let body = body_text.trim();
        if !body.is_empty() {
            let mut yielded = false;
            if let SseStep::Fail(err) = process_payload(
                body,
                &mut splitter,
                &mut tools,
                &mut finish_reason,
                &mut usage,
                &mut |d| {
                    yielded = true;
                    on_delta(d);
                },
            ) {
                return Err(err);
            }
            if !yielded && tools.is_empty() && finish_reason.is_none() {
                return Err(LlmError::Decode(
                    "the 200 response was not an SSE stream and did not parse as a chat completion"
                        .to_string(),
                ));
            }
        }
    }
    // Surface any tail buffered mid-tag so an EOF inside a partial
    // `<think>`/`</think>` fragment doesn't drop it.
    let (resp_tail, reason_tail) = splitter.flush();
    if !resp_tail.is_empty() || !reason_tail.is_empty() {
        on_delta(Delta {
            response: resp_tail,
            reasoning: reason_tail,
            tool_call: String::new(),
        });
    }
    Ok(StreamOutcome {
        text: splitter.finish(),
        tool_calls: tools.finish(),
        finish_reason,
        usage,
    })
}

/// Folds the streamed `tool_calls` deltas — each keyed by an `index`, its
/// `id`/`name` arriving on the first fragment and its `arguments` in string
/// pieces — into whole [`ToolCallRequest`]s. Pure and unit-tested (the
/// fragmentation is provider-specific and easy to get wrong). See
/// `docs/tools.md`.
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    calls: Vec<PartialToolCall>,
}

#[derive(Debug, Default)]
struct PartialToolCall {
    /// The wire `index`, when the provider sent one — `None` for index-less
    /// entries (the non-streaming `message.tool_calls` array has no index).
    index: Option<usize>,
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    /// Fold one streamed tool-call delta fragment. `id`/`name` overwrite when
    /// non-empty (they arrive once); `arguments` fragments concatenate. A
    /// fragment carrying a *different* non-empty `id` than the entry it would
    /// continue is a NEW parallel call, never a merge — providers like
    /// Gemini's OpenAI layer reuse index 0 for every parallel call, and the
    /// non-streaming `message.tool_calls` shape omits `index` entirely, so
    /// keying by index alone corrupted distinct calls into one.
    fn push(
        &mut self,
        index: Option<usize>,
        id: Option<&str>,
        name: Option<&str>,
        arguments: Option<&str>,
    ) {
        // Which entry would this fragment continue? With an index, the latest
        // entry carrying it (so continuations follow a same-index split);
        // without one, the latest entry outright.
        let pos = match index {
            Some(i) => self.calls.iter().rposition(|c| c.index == Some(i)),
            None => self.calls.len().checked_sub(1),
        };
        let pos = pos.filter(|&p| match id {
            Some(id) if !id.is_empty() => self.calls[p].id.is_empty() || self.calls[p].id == id,
            _ => true,
        });
        let entry = match pos {
            Some(p) => &mut self.calls[p],
            None => {
                self.calls.push(PartialToolCall {
                    index,
                    ..Default::default()
                });
                self.calls.last_mut().expect("just pushed an entry")
            }
        };
        if let Some(id) = id.filter(|s| !s.is_empty()) {
            entry.id = id.to_string();
        }
        if let Some(name) = name.filter(|s| !s.is_empty()) {
            entry.name = name.to_string();
        }
        if let Some(args) = arguments {
            entry.arguments.push_str(args);
        }
    }

    /// Were any tool-call deltas seen?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// The finished tool calls, in stream order (a delta with no name is
    /// dropped — an incomplete fragment); a call with no id gets a synthetic
    /// `call_{index}` (its position when it had no wire index) so the result
    /// message can still pair to it.
    #[must_use]
    pub fn finish(self) -> Vec<ToolCallRequest> {
        self.calls
            .into_iter()
            .filter(|c| !c.name.is_empty())
            .enumerate()
            .map(|(pos, c)| ToolCallRequest {
                id: if c.id.is_empty() {
                    format!("call_{}", c.index.unwrap_or(pos))
                } else {
                    c.id
                },
                name: c.name,
                arguments: c.arguments,
            })
            .collect()
    }
}

/// What one processed SSE line means for the drain loop.
enum SseStep {
    /// Keep reading (a delta was emitted, or the line was skippable).
    Continue,
    /// The `data: [DONE]` sentinel — the stream is complete.
    Done,
    /// The provider reported a failure as an in-band `{"error": …}` frame
    /// (aggregators send these on an HTTP 200 stream, then close without
    /// `[DONE]`) — the stream failed and the turn must surface it.
    Fail(LlmError),
}

/// Parse and dispatch one SSE line's bytes: skip non-`data:` lines and empty
/// deltas, feed text/reasoning through `splitter` (emitting via `on_delta`),
/// fold any `tool_calls` fragments into `tools`, and record a `finish_reason`.
/// Invalid UTF-8 is skipped, never fatal.
fn process_sse_line(
    line: &[u8],
    splitter: &mut ThinkingSplitter,
    tools: &mut ToolCallAccumulator,
    finish_reason: &mut Option<String>,
    usage: &mut Option<TokenUsage>,
    on_delta: &mut impl FnMut(Delta),
) -> SseStep {
    let Ok(text) = std::str::from_utf8(line) else {
        return SseStep::Continue;
    };
    let Some(data) = sse_data(text) else {
        return SseStep::Continue;
    };
    if data == "[DONE]" {
        return SseStep::Done;
    }
    process_payload(data, splitter, tools, finish_reason, usage, on_delta)
}

/// Parse and dispatch one JSON payload (the body of a `data:` frame — or, at
/// EOF, a whole non-SSE response body) through the same machinery: surface an
/// in-band error, feed text/reasoning through `splitter`, fold `tool_calls`
/// fragments into `tools`, record a `finish_reason`.
fn process_payload(
    data: &str,
    splitter: &mut ThinkingSplitter,
    tools: &mut ToolCallAccumulator,
    finish_reason: &mut Option<String>,
    usage: &mut Option<TokenUsage>,
    on_delta: &mut impl FnMut(Delta),
) -> SseStep {
    if let Some(err) = parse_sse_error(data) {
        return SseStep::Fail(err);
    }
    // The round's usage rides one (usually final) frame — capture the last
    // non-empty report; delta frames carry `"usage": null` until then.
    if let Some(report) = parse_sse_usage(data) {
        *usage = Some(report);
    }
    // Tool-call fragments and the finish reason ride the same JSON frame as the
    // text delta — fold them in before the content early-return so a
    // tool-call-only frame (no content) is never skipped. The raw name/argument
    // pieces are also gathered into `tool_frag` so the caller can count the
    // model *generating* the call (docs/status-indicator.md).
    let (tool_deltas, reason) = parse_sse_tool_calls(data);
    let mut tool_frag = String::new();
    for td in tool_deltas {
        if let Some(name) = &td.name {
            tool_frag.push_str(name);
        }
        if let Some(args) = &td.arguments {
            tool_frag.push_str(args);
        }
        tools.push(
            td.index,
            td.id.as_deref(),
            td.name.as_deref(),
            td.arguments.as_deref(),
        );
    }
    if let Some(reason) = reason {
        *finish_reason = Some(reason);
    }
    let (content, reasoning) = parse_sse_data(data);
    // Nothing to surface this frame (a keep-alive, or a finish_reason-only frame).
    if content.is_empty() && reasoning.is_empty() && tool_frag.is_empty() {
        return SseStep::Continue;
    }
    let (resp_delta, reason_delta) = splitter.feed(&content, &reasoning);
    if !resp_delta.is_empty() || !reason_delta.is_empty() || !tool_frag.is_empty() {
        on_delta(Delta {
            response: resp_delta,
            reasoning: reason_delta,
            tool_call: tool_frag,
        });
    }
    SseStep::Continue
}

/// The wire shape of an in-band error frame: `{"error": {"message", "code"?}}`.
/// The code is a number for HTTP-like statuses (OpenRouter) but some providers
/// send a string tag — both map into [`LlmError::Api`] (a string code becomes
/// status 0, keeping the message).
#[derive(Debug, Deserialize)]
struct ErrorPayload {
    error: ErrorBody,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<serde_json::Value>,
}

/// Detect an in-band `{"error": …}` frame in one SSE `data:` payload. `None`
/// for ordinary delta frames (including unparseable ones — those stay skipped,
/// a keep-alive shouldn't kill the stream).
fn parse_sse_error(data: &str) -> Option<LlmError> {
    let payload: ErrorPayload = serde_json::from_str(data).ok()?;
    let status = payload
        .error
        .code
        .as_ref()
        .and_then(serde_json::Value::as_u64)
        .and_then(|c| u16::try_from(c).ok())
        .unwrap_or(0);
    let body = payload
        .error
        .message
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| data.to_string());
    Some(LlmError::Api { status, body })
}

/// The most raw non-SSE body bytes [`drain_stream`] retains for the EOF
/// JSON-completion fallback — a plain completion fits easily; past it the
/// fallback parse fails and surfaces a decode error instead of ballooning.
const RAW_BODY_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Is this line SSE framing (any field or comment line)? A plain JSON body
/// line never is: raw newlines cannot occur inside JSON strings, so a body
/// line starts with `{`, whitespace, or a quoted key — never `data:`/`:`.
fn is_sse_framing(line: &[u8]) -> bool {
    [b"data:" as &[u8], b"event:", b"id:", b"retry:", b":"]
        .iter()
        .any(|p| line.starts_with(p))
}

/// Extract the `data:` payload from one SSE line, or `None` for comments/blank
/// lines/other fields. The optional single space after the colon is stripped.
fn sse_data(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("data:")?;
    Some(
        rest.strip_prefix(' ')
            .unwrap_or(rest)
            .trim_end_matches(['\r', '\n']),
    )
}

/// The wire shape of one streamed chunk. `choices[].delta` carries the
/// incremental text; a non-streaming server may put it under `message` instead.
#[derive(Debug, Deserialize)]
struct StreamPayload {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: Option<StreamDelta>,
    #[serde(default)]
    message: Option<StreamDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCallDelta>>,
}

/// One streamed `tool_calls[]` entry — the fragmented Chat Completions shape.
/// `index` stays `None` when absent (the non-streaming `message.tool_calls`
/// array has none) so the accumulator can tell "no index" from "index 0".
#[derive(Debug, Deserialize)]
struct ToolCallDelta {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionDelta>,
}

#[derive(Debug, Default, Deserialize)]
struct FunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// One parsed tool-call fragment, flattened for the accumulator.
struct ToolCallDeltaParsed {
    index: Option<usize>,
    id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
}

/// Extract the `tool_calls` fragments and the `finish_reason` from one SSE
/// `data:` frame (a second parse of the same JSON as [`parse_sse_data`], which
/// stays focused on content/reasoning). Unparseable frames yield nothing.
fn parse_sse_tool_calls(data: &str) -> (Vec<ToolCallDeltaParsed>, Option<String>) {
    let Ok(payload) = serde_json::from_str::<StreamPayload>(data) else {
        return (Vec::new(), None);
    };
    let mut calls = Vec::new();
    let mut finish = None;
    for choice in payload.choices {
        if let Some(reason) = choice.finish_reason.filter(|r| !r.is_empty()) {
            finish = Some(reason);
        }
        let delta = choice.delta.or(choice.message).unwrap_or_default();
        if let Some(tool_calls) = delta.tool_calls {
            for tc in tool_calls {
                let (name, arguments) = match tc.function {
                    Some(f) => (f.name, f.arguments),
                    None => (None, None),
                };
                calls.push(ToolCallDeltaParsed {
                    index: tc.index,
                    id: tc.id,
                    name,
                    arguments,
                });
            }
        }
    }
    (calls, finish)
}

/// The `usage` block of a streamed frame (or a whole non-SSE completion) —
/// the OpenAI accounting shape with the prompt-cache detail nested under
/// `prompt_tokens_details`, plus the top-level `cache_read_input_tokens` /
/// `cache_creation_input_tokens` aliases Venice and Anthropic-style shims
/// send. All fields default so partial reports still parse.
#[derive(Debug, Default, Deserialize)]
struct UsagePayload {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default)]
    completion_tokens_details: Option<CompletionTokensDetails>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
}

/// The `completion_tokens_details` block: the breakdown of what the round
/// *generated*. Only the reasoning share interests us — it is what the
/// `Thought for …` cell reports (`docs/thinking-stream.md`).
#[derive(Debug, Default, Deserialize)]
struct CompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
    /// OpenRouter's cache-write spelling.
    #[serde(default)]
    cache_write_tokens: Option<u64>,
    /// The Anthropic-style spelling some shims nest here instead.
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
}

/// Extract the frame's usage report as a [`TokenUsage`], or `None` when the
/// frame carries none (`"usage": null` on delta frames) or an all-zero block
/// (meaningless — never a real round). A third targeted parse of the same
/// JSON, like [`parse_sse_tool_calls`].
fn parse_sse_usage(data: &str) -> Option<TokenUsage> {
    #[derive(Deserialize)]
    struct UsageEnvelope {
        #[serde(default)]
        usage: Option<UsagePayload>,
    }
    let envelope: UsageEnvelope = serde_json::from_str(data).ok()?;
    let u = envelope.usage?;
    let input = u.prompt_tokens.unwrap_or(0);
    let output = u.completion_tokens.unwrap_or(0);
    if input == 0 && output == 0 {
        return None;
    }
    let details = u.prompt_tokens_details.as_ref();
    let cached = details
        .and_then(|d| d.cached_tokens)
        .or(u.cache_read_input_tokens)
        .unwrap_or(0);
    let cache_write = details
        .and_then(|d| d.cache_write_tokens)
        .or_else(|| details.and_then(|d| d.cache_creation_input_tokens))
        .or(u.cache_creation_input_tokens)
        .unwrap_or(0);
    let reasoning = u
        .completion_tokens_details
        .as_ref()
        .and_then(|d| d.reasoning_tokens)
        .unwrap_or(0);
    Some(TokenUsage {
        input,
        output,
        cached,
        cache_write,
        reasoning,
    })
}

/// Parse one SSE `data:` JSON payload into `(content, reasoning)`, concatenating
/// across choices (chat completions almost always have exactly one). Unparseable
/// payloads yield empty deltas (skipped by the caller), never an error — a
/// provider's keep-alive or non-delta frames shouldn't kill the stream.
fn parse_sse_data(data: &str) -> (String, String) {
    let Ok(payload) = serde_json::from_str::<StreamPayload>(data) else {
        return (String::new(), String::new());
    };
    let mut content = String::new();
    let mut reasoning = String::new();
    for choice in payload.choices {
        let delta = choice.delta.or(choice.message).unwrap_or_default();
        if let Some(c) = delta.content {
            content.push_str(&c);
        }
        if let Some(r) = delta.reasoning_content.or(delta.reasoning) {
            reasoning.push_str(&r);
        }
    }
    (content, reasoning)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_uses_default_when_empty() {
        let mut cfg = ModelConfig::fallback();
        cfg.api_base = String::new();
        let client = OpenAiClient::new(cfg);
        assert_eq!(
            client.endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn endpoint_trims_trailing_slash() {
        let mut cfg = ModelConfig::fallback();
        cfg.api_base = "https://example.com/v1/".into();
        let client = OpenAiClient::new(cfg);
        assert_eq!(client.endpoint(), "https://example.com/v1/chat/completions");
    }

    #[test]
    fn payload_contains_model_and_stream_flag() {
        let cfg = ModelConfig::fallback();
        let client = OpenAiClient::new(cfg);
        let p = client.build_payload(&[ChatMessage::user("hi")]);
        assert!(p["stream"].as_bool().unwrap());
        assert!(!p["model"].as_str().unwrap().is_empty());
        assert_eq!(p["messages"][0]["role"], "user");
        assert_eq!(p["messages"][0]["content"], "hi");
    }

    #[test]
    fn payload_includes_temperature_when_set() {
        let mut cfg = ModelConfig::fallback();
        cfg.temperature = Some(0.5);
        let client = OpenAiClient::new(cfg);
        let p = client.build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["temperature"], json!(0.5));
    }

    #[test]
    fn payload_omits_temperature_when_unset() {
        let client = OpenAiClient::new(ModelConfig::fallback());
        let p = client.build_payload(&[ChatMessage::user("hi")]);
        assert!(p.get("temperature").is_none());
    }

    #[test]
    fn payload_merges_provider_extra_body() {
        let mut cfg = ModelConfig::fallback();
        cfg.extra_body.insert(
            "venice_parameters".to_string(),
            json!({"disable_thinking": true}),
        );
        let client = OpenAiClient::new(cfg);
        let p = client.build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["venice_parameters"]["disable_thinking"], json!(true));
    }

    #[test]
    fn payload_carries_the_reasoning_effort() {
        use crate::llm::reasoning::{ReasoningEffort, ThinkingMode};
        let mut cfg = ModelConfig::fallback();
        cfg.thinking = Some(ThinkingMode::Effort(ReasoningEffort::High));
        let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["reasoning"], json!({"effort": "high"}));
        assert!(
            p.get("venice_parameters").is_none(),
            "no venice table is invented for a provider without one"
        );
    }

    #[test]
    fn payload_disables_reasoning_when_off() {
        use crate::llm::reasoning::ThinkingMode;
        let mut cfg = ModelConfig::fallback();
        cfg.thinking = Some(ThinkingMode::Off);
        let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["reasoning"], json!({"enabled": false}));
    }

    #[test]
    fn payload_sends_nothing_for_thinking_on() {
        // On = the model's own default: no reasoning key at all.
        use crate::llm::reasoning::ThinkingMode;
        let mut cfg = ModelConfig::fallback();
        cfg.thinking = Some(ThinkingMode::On);
        let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
        assert!(p.get("reasoning").is_none());
    }

    #[test]
    fn payload_omits_reasoning_when_no_mode_is_set() {
        let p =
            OpenAiClient::new(ModelConfig::fallback()).build_payload(&[ChatMessage::user("hi")]);
        assert!(p.get("reasoning").is_none());
    }

    #[test]
    fn a_venice_provider_gets_disable_thinking_toggled_with_the_mode() {
        // Venice ignores `reasoning.enabled` — `venice_parameters.
        // disable_thinking` is the toggle it honours. A provider carrying a
        // venice_parameters table (the Venice-family marker) gets it set to
        // match the mode, other table keys preserved; and a stale
        // file-configured `disable_thinking: true` must not silently veto an
        // effort the footer claims is active.
        use crate::llm::reasoning::{ReasoningEffort, ThinkingMode};
        let mut cfg = ModelConfig::fallback();
        cfg.extra_body.insert(
            "venice_parameters".to_string(),
            json!({"include_venice_system_prompt": false, "disable_thinking": true}),
        );
        cfg.thinking = Some(ThinkingMode::Effort(ReasoningEffort::Medium));
        let p = OpenAiClient::new(cfg.clone()).build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["reasoning"], json!({"effort": "medium"}));
        assert_eq!(p["venice_parameters"]["disable_thinking"], json!(false));
        assert_eq!(
            p["venice_parameters"]["include_venice_system_prompt"],
            json!(false),
            "the table's other keys survive"
        );

        cfg.thinking = Some(ThinkingMode::Off);
        let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["reasoning"], json!({"enabled": false}));
        assert_eq!(p["venice_parameters"]["disable_thinking"], json!(true));
    }

    #[test]
    fn without_a_mode_the_file_configured_venice_table_passes_through() {
        // No thinking mode (an unknown/non-reasoning model): the provider's
        // kwargs are forwarded untouched, as before.
        let mut cfg = ModelConfig::fallback();
        cfg.extra_body.insert(
            "venice_parameters".to_string(),
            json!({"disable_thinking": true}),
        );
        let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["venice_parameters"]["disable_thinking"], json!(true));
        assert!(p.get("reasoning").is_none());
    }

    #[test]
    fn the_shipped_venice_provider_sends_the_proxys_request_to_venice_itself() {
        // The direct Venice provider is the Agent Zero proxy's twin
        // (`docs/venice.md`). Resolved through the shipped file exactly as the
        // boundary resolves it, its request must carry everything the
        // proxy's does — the `venice_parameters` table with
        // `disable_thinking` synced to the mode, the session's
        // `prompt_cache_key` (Venice's own cache-affinity hint) — and nothing
        // Venice rejects: OpenRouter's `session_id` is an unrecognised key
        // there. Only the endpoint moves.
        use crate::llm::reasoning::ThinkingMode;
        use crate::llm::{ProvidersFile, Selection};
        let file = ProvidersFile::builtin();
        let resolve = |provider_id: &str| {
            file.model_config(&Selection {
                provider_id: provider_id.to_string(),
                model: "qwen3-235b".to_string(),
                api_key: Some("key".to_string()),
                thinking: Some(ThinkingMode::Off),
                cache_key: Some("alter-zero-7".to_string()),
                service_tier: None,
                ..Selection::default()
            })
            .expect("shipped")
        };
        let venice = OpenAiClient::new(resolve("venice"));
        assert_eq!(
            venice.endpoint(),
            "https://api.venice.ai/api/v1/chat/completions"
        );
        let direct = venice.build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(direct["venice_parameters"]["disable_thinking"], json!(true));
        assert_eq!(
            direct["venice_parameters"]["include_venice_system_prompt"],
            json!(false)
        );
        assert_eq!(direct["prompt_cache_key"], json!("alter-zero-7"));
        assert!(
            direct.get("session_id").is_none(),
            "Venice rejects unknown body keys"
        );
        // Byte-for-byte the proxy's request body: the two providers differ
        // only in where it goes.
        let proxy = OpenAiClient::new(resolve("a0_venice"));
        assert_eq!(direct, proxy.build_payload(&[ChatMessage::user("hi")]));
        assert_eq!(
            proxy.endpoint(),
            "https://api.agent-zero.ai/venice/v1/chat/completions"
        );
    }

    #[test]
    fn payload_asks_for_the_streamed_usage_frame() {
        // The standard OpenAI `stream_options.include_usage` — both shipped
        // providers honour it, delivering the final usage frame the app snaps
        // its token tally to (docs/prompt-caching.md).
        let p =
            OpenAiClient::new(ModelConfig::fallback()).build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["stream_options"], json!({"include_usage": true}));
    }

    #[test]
    fn a_provider_kwarg_can_override_stream_options() {
        // extra_body merges after the base payload, so a provider whose shim
        // chokes on stream_options can override it from providers.toml.
        let mut cfg = ModelConfig::fallback();
        cfg.extra_body
            .insert("stream_options".to_string(), serde_json::Value::Null);
        let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["stream_options"], serde_json::Value::Null);
    }

    #[test]
    fn payload_marks_cache_breakpoints_for_an_explicit_caching_model() {
        // An anthropic/ model via an OpenRouter-style aggregator only caches
        // marked blocks: the wire messages carry the ephemeral breakpoints
        // (system + frontier — the pure placement is cache.rs's, this test
        // pins the builder wiring). See docs/prompt-caching.md.
        let mut cfg = ModelConfig::fallback();
        cfg.model = "anthropic/claude-haiku-4.5".to_string();
        let p = OpenAiClient::new(cfg)
            .build_payload(&[ChatMessage::system("persona"), ChatMessage::user("hi")]);
        assert_eq!(
            p["messages"][0]["content"][0]["cache_control"],
            json!({"type": "ephemeral"}),
            "the system prompt carries a breakpoint: {p}"
        );
        assert_eq!(
            p["messages"][1]["content"][0]["cache_control"],
            json!({"type": "ephemeral"}),
            "the frontier carries a breakpoint: {p}"
        );
    }

    #[test]
    fn payload_leaves_messages_plain_for_implicit_caching_models() {
        // OpenAI/Venice-style models cache automatically — their messages
        // stay the classic bare-string form, byte-identical to before.
        let p = OpenAiClient::new(ModelConfig::fallback())
            .build_payload(&[ChatMessage::system("persona"), ChatMessage::user("hi")]);
        assert_eq!(p["messages"][0]["content"], json!("persona"));
        assert_eq!(p["messages"][1]["content"], json!("hi"));
    }

    #[test]
    fn payload_carries_the_cache_key_as_prompt_cache_key() {
        // The per-session affinity key rides the standard OpenAI
        // `prompt_cache_key` (verified accepted by OpenRouter and Venice) so
        // repeats hit the same warm cache. No OpenRouter-only `session_id`
        // for a non-OpenRouter base — Venice rejects unknown keys.
        let mut cfg = ModelConfig::fallback();
        cfg.cache_key = Some("alter-zero-42".to_string());
        let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["prompt_cache_key"], json!("alter-zero-42"));
        assert!(
            p.get("session_id").is_none(),
            "session_id is OpenRouter-only: {p}"
        );
    }

    #[test]
    fn an_openrouter_base_also_gets_the_session_id() {
        // OpenRouter routes a model across several upstream providers; its
        // `session_id` pins the session to one so cache writes are readable.
        let mut cfg = ModelConfig::fallback();
        cfg.api_base = "https://openrouter.ai/api/v1".to_string();
        cfg.cache_key = Some("alter-zero-42".to_string());
        let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["prompt_cache_key"], json!("alter-zero-42"));
        assert_eq!(p["session_id"], json!("alter-zero-42"));
    }

    #[test]
    fn payload_omits_affinity_fields_without_a_cache_key() {
        let mut cfg = ModelConfig::fallback();
        cfg.api_base = "https://openrouter.ai/api/v1".to_string();
        let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
        assert!(p.get("prompt_cache_key").is_none());
        assert!(p.get("session_id").is_none());
    }

    // --- GitHub Copilot's payload shape (docs/copilot.md) ---

    #[test]
    fn a_copilot_payload_carries_no_affinity_key() {
        // Copilot's proxy answers an unrecognised request shape with
        // `model_not_supported` — an error that blames the model — and its own
        // caching is server-side, so the affinity key buys nothing worth that.
        let mut cfg = ModelConfig::fallback();
        cfg.auth = crate::llm::AuthScheme::GithubCopilot;
        cfg.cache_key = Some("alter-zero-42".to_string());
        let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
        assert!(p.get("prompt_cache_key").is_none(), "{p}");
        assert!(p.get("session_id").is_none(), "{p}");
    }

    // --- the ChatGPT backend's per-request headers (docs/chatgpt.md) ---

    /// The headers a request built for `cfg` carries, by name.
    fn built_header(cfg: ModelConfig, name: &str) -> Option<String> {
        let client = OpenAiClient::new(cfg);
        let req = client
            .chatgpt_request_headers(reqwest::blocking::Client::new().post("https://example.test/"))
            .build()
            .expect("a request builds");
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    }

    #[test]
    fn a_chatgpt_request_carries_the_session_headers_its_backend_caches_on() {
        // The body's `prompt_cache_key` is not what chatgpt.com keys its
        // prompt cache on (verified live: 0 cached tokens on an identical
        // 6.7k prefix without these, 6.4k with them). The session key rides
        // Codex's two header names.
        let mut cfg = ModelConfig::fallback();
        cfg.auth = crate::llm::AuthScheme::ChatGptCodex;
        cfg.cache_key = Some("alter-zero-42".to_string());
        assert_eq!(
            built_header(cfg.clone(), "session_id").as_deref(),
            Some("alter-zero-42")
        );
        assert_eq!(
            built_header(cfg, "conversation_id").as_deref(),
            Some("alter-zero-42")
        );
    }

    #[test]
    fn an_ordinary_provider_gets_no_session_headers() {
        // A pasted-key provider keys on the body's `prompt_cache_key`; a
        // header it never asked for is a header a strict shim can refuse.
        let mut cfg = ModelConfig::fallback();
        cfg.cache_key = Some("alter-zero-42".to_string());
        assert_eq!(built_header(cfg.clone(), "session_id"), None);
        assert_eq!(built_header(cfg, "conversation_id"), None);
    }

    #[test]
    fn a_copilot_payload_spells_reasoning_as_a_top_level_effort() {
        // Not the `reasoning` object every other provider here takes — and
        // *instead of* it, since the object is an unknown field there.
        let mut cfg = ModelConfig::fallback();
        cfg.auth = crate::llm::AuthScheme::GithubCopilot;
        cfg.thinking = Some(crate::llm::ThinkingMode::Effort(
            crate::llm::ReasoningEffort::High,
        ));
        let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["reasoning_effort"], json!("high"));
        assert!(p.get("reasoning").is_none(), "{p}");
    }

    #[test]
    fn a_copilot_payload_omits_reasoning_when_it_is_off_or_default() {
        // Reasoning is opt-in on Copilot: On means the model's own default and
        // Off is the absence of the parameter, so neither sends one.
        for mode in [crate::llm::ThinkingMode::On, crate::llm::ThinkingMode::Off] {
            let mut cfg = ModelConfig::fallback();
            cfg.auth = crate::llm::AuthScheme::GithubCopilot;
            cfg.thinking = Some(mode);
            let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
            assert!(p.get("reasoning_effort").is_none(), "{mode:?}: {p}");
            assert!(p.get("reasoning").is_none(), "{mode:?}: {p}");
        }
    }

    #[test]
    fn an_ordinary_providers_payload_is_unchanged_by_the_copilot_branch() {
        let mut cfg = ModelConfig::fallback();
        cfg.cache_key = Some("alter-zero-42".to_string());
        cfg.thinking = Some(crate::llm::ThinkingMode::Effort(
            crate::llm::ReasoningEffort::High,
        ));
        let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["prompt_cache_key"], json!("alter-zero-42"));
        assert_eq!(p["reasoning"], json!({"effort": "high"}));
        assert!(p.get("reasoning_effort").is_none(), "{p}");
    }

    #[test]
    fn only_a_copilot_config_has_its_failures_rewritten() {
        let raw = r#"{"error":{"message":"The requested model is not supported.",
            "code":"model_not_supported"}}"#;
        let failure = || LlmError::Api {
            status: 400,
            body: raw.to_string(),
        };
        // Every other provider's error passes through byte-for-byte — its own
        // body is the best account of what went wrong.
        let mut plain = ModelConfig::fallback();
        plain.model = "gpt-4o-mini".to_string();
        let through = OpenAiClient::new(plain).explain(failure());
        assert_eq!(through.to_string(), failure().to_string());

        // Copilot's gets the sentence, naming the model it refused.
        let mut copilot = ModelConfig::fallback();
        copilot.auth = crate::llm::AuthScheme::GithubCopilot;
        copilot.model = "claude-sonnet-4.6".to_string();
        let explained = OpenAiClient::new(copilot).explain(failure()).to_string();
        assert!(explained.contains("claude-sonnet-4.6"), "{explained}");
        assert!(explained.contains("/model"), "{explained}");
    }

    #[test]
    fn a_copilot_failure_we_cannot_explain_keeps_its_own_body() {
        let failure = LlmError::Api {
            status: 500,
            body: r#"{"error":{"message":"upstream exploded"}}"#.to_string(),
        };
        let mut cfg = ModelConfig::fallback();
        cfg.auth = crate::llm::AuthScheme::GithubCopilot;
        let out = OpenAiClient::new(cfg).explain(failure).to_string();
        assert!(out.contains("upstream exploded"), "{out}");
    }

    #[test]
    fn sse_data_strips_the_prefix_and_optional_space() {
        assert_eq!(sse_data("data: hello\n"), Some("hello"));
        assert_eq!(sse_data("data:hello\n"), Some("hello"));
        assert_eq!(sse_data("data: [DONE]\n"), Some("[DONE]"));
    }

    #[test]
    fn sse_data_ignores_non_data_lines() {
        assert_eq!(sse_data(": keep-alive\n"), None);
        assert_eq!(sse_data("event: message\n"), None);
        assert_eq!(sse_data("\n"), None);
    }

    #[test]
    fn parse_sse_data_reads_content_delta() {
        let (c, r) = parse_sse_data(r#"{"choices":[{"delta":{"content":"Hi"}}]}"#);
        assert_eq!(c, "Hi");
        assert!(r.is_empty());
    }

    #[test]
    fn parse_sse_data_reads_native_reasoning() {
        let (c, r) = parse_sse_data(r#"{"choices":[{"delta":{"reasoning_content":"hmm"}}]}"#);
        assert!(c.is_empty());
        assert_eq!(r, "hmm");
    }

    #[test]
    fn parse_sse_data_reads_the_reasoning_alias() {
        let (_c, r) = parse_sse_data(r#"{"choices":[{"delta":{"reasoning":"why"}}]}"#);
        assert_eq!(r, "why");
    }

    #[test]
    fn parse_sse_data_falls_back_to_message_for_non_streaming() {
        let (c, _r) = parse_sse_data(r#"{"choices":[{"message":{"content":"full"}}]}"#);
        assert_eq!(c, "full");
    }

    #[test]
    fn parse_sse_data_of_garbage_is_empty_not_a_panic() {
        assert_eq!(parse_sse_data("not json"), (String::new(), String::new()));
        assert_eq!(parse_sse_data("{}"), (String::new(), String::new()));
    }

    /// Run a line's bytes through `process_sse_line`, collecting emitted deltas.
    fn drive_line(line: &str) -> (SseStep, Vec<Delta>) {
        let mut splitter = ThinkingSplitter::new();
        let mut tools = ToolCallAccumulator::default();
        let mut finish = None;
        let mut usage = None;
        let mut out = Vec::new();
        let step = process_sse_line(
            line.as_bytes(),
            &mut splitter,
            &mut tools,
            &mut finish,
            &mut usage,
            &mut |d| out.push(d),
        );
        (step, out)
    }

    #[test]
    fn process_sse_line_emits_a_content_delta() {
        let (step, deltas) = drive_line(r#"data: {"choices":[{"delta":{"content":"Hi"}}]}"#);
        assert!(matches!(step, SseStep::Continue));
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].response, "Hi");
    }

    #[test]
    fn process_sse_line_reports_the_done_sentinel() {
        let (step, deltas) = drive_line("data: [DONE]");
        assert!(matches!(step, SseStep::Done), "the caller stops on [DONE]");
        assert!(deltas.is_empty());
    }

    #[test]
    fn process_sse_line_surfaces_an_in_band_error_frame() {
        // OpenRouter (and other aggregators) report a mid-stream failure as a
        // normal-looking `data:` frame carrying an "error" object, then end
        // the stream without [DONE]. Swallowing it turned a failed/truncated
        // stream into a silently "successful" turn.
        let (step, deltas) =
            drive_line(r#"data: {"error":{"message":"Provider returned error","code":429}}"#);
        let SseStep::Fail(err) = step else {
            panic!("an in-band error frame must fail the stream, got a pass-through");
        };
        assert!(deltas.is_empty());
        let shown = err.to_string();
        assert!(shown.contains("429"), "carries the code: {shown}");
        assert!(
            shown.contains("Provider returned error"),
            "carries the message: {shown}"
        );
    }

    #[test]
    fn process_sse_line_in_band_error_with_a_string_code_still_fails() {
        let (step, _deltas) =
            drive_line(r#"data: {"error":{"message":"quota exhausted","code":"rate_limited"}}"#);
        let SseStep::Fail(err) = step else {
            panic!("expected a failure step");
        };
        assert!(err.to_string().contains("quota exhausted"));
    }

    #[test]
    fn process_sse_line_skips_non_data_and_empty_lines() {
        assert_eq!(drive_line(": keep-alive").1.len(), 0);
        assert_eq!(drive_line("").1.len(), 0);
        assert_eq!(drive_line(r#"data: {"choices":[]}"#).1.len(), 0);
    }

    #[test]
    fn process_sse_line_ignores_invalid_utf8() {
        let mut splitter = ThinkingSplitter::new();
        let mut tools = ToolCallAccumulator::default();
        let mut finish = None;
        let mut usage = None;
        let mut emitted = false;
        // A lone 0xFF byte after the prefix isn't valid UTF-8.
        let step = process_sse_line(
            &[b'd', b'a', b't', b'a', b':', 0xFF],
            &mut splitter,
            &mut tools,
            &mut finish,
            &mut usage,
            &mut |_| {
                emitted = true;
            },
        );
        assert!(matches!(step, SseStep::Continue));
        assert!(!emitted);
    }

    #[test]
    fn process_sse_line_routes_reasoning_through_the_splitter() {
        let (_done, deltas) =
            drive_line(r#"data: {"choices":[{"delta":{"reasoning_content":"why"}}]}"#);
        assert_eq!(deltas.len(), 1);
        assert!(deltas[0].response.is_empty());
        assert_eq!(deltas[0].reasoning, "why");
    }

    #[test]
    fn process_sse_line_surfaces_tool_call_fragments_for_counting() {
        // A tool-call-only frame (no content/reasoning) must still emit a Delta
        // carrying the streamed name/arguments fragment, so the live token tally
        // ticks while the model *generates* the call (see docs/status-indicator.md).
        let (step, deltas) = drive_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"bash","arguments":"{\"command\":"}}]}}]}"#,
        );
        assert!(matches!(step, SseStep::Continue));
        assert_eq!(deltas.len(), 1, "the frame surfaces one delta");
        assert!(deltas[0].response.is_empty(), "no visible reply text");
        assert!(deltas[0].reasoning.is_empty(), "not reasoning");
        assert_eq!(
            deltas[0].tool_call, r#"bash{"command":"#,
            "the name + arguments fragment rides for counting"
        );
    }

    #[test]
    fn process_sse_line_counts_an_argument_only_tool_fragment() {
        // A later fragment carries only more `arguments` (no name/id) — still
        // surfaced so its tokens count too.
        let (_step, deltas) = drive_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}"#,
        );
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].tool_call, r#""ls"}"#);
    }

    #[test]
    fn payload_includes_the_tools_array_and_auto_choice_when_set() {
        let client =
            OpenAiClient::new(ModelConfig::fallback()).with_tools(crate::llm::tools::tool_specs());
        let p = client.build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["tool_choice"], json!("auto"));
        assert_eq!(p["tools"][0]["function"]["name"], "bash");
        assert_eq!(p["tools"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn payload_omits_tools_when_none_are_offered() {
        let client = OpenAiClient::new(ModelConfig::fallback());
        let p = client.build_payload(&[ChatMessage::user("hi")]);
        assert!(p.get("tools").is_none());
        assert!(p.get("tool_choice").is_none());
    }

    /// Feed a sequence of SSE lines through one accumulator, as a real stream
    /// would, and return the finished tool calls.
    fn drive_tool_stream(lines: &[&str]) -> Vec<ToolCallRequest> {
        let mut splitter = ThinkingSplitter::new();
        let mut tools = ToolCallAccumulator::default();
        let mut finish = None;
        let mut usage = None;
        for line in lines {
            process_sse_line(
                line.as_bytes(),
                &mut splitter,
                &mut tools,
                &mut finish,
                &mut usage,
                &mut |_| {},
            );
        }
        tools.finish()
    }

    #[test]
    fn tool_call_arguments_accumulate_across_fragments() {
        // The name + id arrive on the first fragment; the arguments string
        // streams in pieces that must concatenate into valid JSON.
        let calls = drive_tool_stream(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"bash","arguments":"{\"comm"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"and\":\"ls\"}"}}]}}]}"#,
        ]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_a");
        assert_eq!(calls[0].name, "bash");
        assert_eq!(calls[0].arguments, r#"{"command":"ls"}"#);
    }

    #[test]
    fn two_parallel_tool_calls_accumulate_by_index() {
        let calls = drive_tool_stream(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"read","arguments":"{}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"bash","arguments":"{}"}}]}}]}"#,
        ]);
        assert_eq!(calls.len(), 2);
        assert_eq!(
            (calls[0].name.as_str(), calls[1].name.as_str()),
            ("read", "bash")
        );
    }

    #[test]
    fn a_tool_call_without_an_id_gets_a_synthetic_one() {
        let calls = drive_tool_stream(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":2,"function":{"name":"edit","arguments":"{}"}}]}}]}"#,
        ]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_2");
    }

    #[test]
    fn index_less_parallel_tool_calls_stay_distinct() {
        // A non-streaming `message.tool_calls` array (and some streaming
        // providers) omits `index` entirely — two distinct calls must not
        // merge into one corrupted entry.
        let calls = drive_tool_stream(&[
            r#"data: {"choices":[{"message":{"tool_calls":[{"id":"a","function":{"name":"read","arguments":"{\"path\":\"x\"}"}},{"id":"b","function":{"name":"bash","arguments":"{\"command\":\"ls\"}"}}]}}]}"#,
        ]);
        assert_eq!(calls.len(), 2);
        assert_eq!(
            (calls[0].id.as_str(), calls[0].name.as_str()),
            ("a", "read")
        );
        assert_eq!(calls[0].arguments, r#"{"path":"x"}"#);
        assert_eq!(
            (calls[1].id.as_str(), calls[1].name.as_str()),
            ("b", "bash")
        );
        assert_eq!(calls[1].arguments, r#"{"command":"ls"}"#);
    }

    #[test]
    fn a_new_id_at_the_same_index_starts_a_new_call() {
        // Some OpenAI-compat layers (Gemini's, for one) stream every parallel
        // call with index 0: the id is what separates them, and continuation
        // fragments (no id) belong to the latest call.
        let calls = drive_tool_stream(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"read","arguments":"{}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"b","function":{"name":"bash","arguments":"{\"comm"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"and\":\"ls\"}"}}]}}]}"#,
        ]);
        assert_eq!(calls.len(), 2);
        assert_eq!(
            (calls[0].id.as_str(), calls[0].name.as_str()),
            ("a", "read")
        );
        assert_eq!(calls[1].id, "b");
        assert_eq!(calls[1].arguments, r#"{"command":"ls"}"#);
    }

    #[test]
    fn a_content_only_stream_yields_no_tool_calls() {
        let calls = drive_tool_stream(&[r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#]);
        assert!(calls.is_empty());
    }

    /// Queue `events` on a transport channel, disconnect it (the clean-EOF
    /// signal), and drain — returning the outcome and every surfaced delta.
    fn drain_queued(events: Vec<Result<Vec<u8>>>) -> (Result<StreamOutcome>, Vec<Delta>) {
        let (tx, rx) = std::sync::mpsc::channel();
        for e in events {
            tx.send(e).unwrap();
        }
        drop(tx);
        let mut deltas = Vec::new();
        let out = drain_stream(&rx, &CancelToken::new(), |d| deltas.push(d));
        (out, deltas)
    }

    /// The Anthropic drain's twin of [`drain_queued`].
    fn drain_anthropic_queued(events: Vec<Result<Vec<u8>>>) -> (Result<StreamOutcome>, Vec<Delta>) {
        let (tx, rx) = std::sync::mpsc::channel();
        for e in events {
            tx.send(e).unwrap();
        }
        drop(tx);
        let mut deltas = Vec::new();
        let out = drain_anthropic(&rx, &CancelToken::new(), |d| deltas.push(d));
        (out, deltas)
    }

    #[test]
    fn drain_anthropic_reads_a_whole_round_off_the_wire() {
        // The frames are the ones the docs print, `event:` lines and all —
        // which this drain ignores, reading each payload's own `type`.
        let (out, deltas) = drain_anthropic_queued(vec![
            Ok(b"event: message_start
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":25,\"cache_read_input_tokens\":100,\"output_tokens\":1}}}

".to_vec()),
            Ok(b"event: ping
data: {\"type\":\"ping\"}

".to_vec()),
            Ok(b"event: content_block_delta
data: {\"type\":\"content_bl".to_vec()),
            Ok(b"ock_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}
".to_vec()),
            Ok(b"event: message_delta
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":9}}
".to_vec()),
            Ok(b"event: message_stop
data: {\"type\":\"message_stop\"}
".to_vec()),
        ]);
        let out = out.expect("a clean stream");
        assert_eq!(out.text.response, "Hi");
        assert_eq!(deltas.len(), 1, "the ping and the frames carry no text");
        assert_eq!(out.finish_reason.as_deref(), Some("end_turn"));
        let usage = out.usage.expect("usage");
        assert_eq!(
            usage.input, 125,
            "the uncached remainder plus the cache read"
        );
        assert_eq!(usage.cached, 100);
        assert_eq!(usage.output, 9);
    }

    #[test]
    fn drain_anthropic_surfaces_an_in_band_error_frame() {
        // A 529 arrives *after* a 200, as an SSE event.
        let (out, _) = drain_anthropic_queued(vec![Ok(b"event: error
data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}
"
        .to_vec())]);
        let Err(LlmError::Api { body, .. }) = out else {
            panic!("an in-band error must fail the stream: {out:?}");
        };
        assert_eq!(body, "Overloaded");
    }

    #[test]
    fn drain_anthropic_stops_at_message_stop_and_ignores_later_bytes() {
        let (out, deltas) = drain_anthropic_queued(vec![
            Ok(b"data: {\"type\":\"message_stop\"}
".to_vec()),
            Ok(b"data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"late\"}}
".to_vec()),
        ]);
        assert!(deltas.is_empty(), "nothing after the stop is read");
        assert_eq!(out.expect("a clean stream").text.response, "");
    }

    #[test]
    fn drain_stream_reassembles_a_frame_split_across_chunks() {
        // The transport thread hands over whatever each socket read returned —
        // a `data:` frame can split anywhere (even mid-JSON-key), and CRLF
        // endings must survive the reassembly.
        let (out, deltas) = drain_queued(vec![
            Ok(b"data: {\"choices\":[{\"delta\":{\"cont".to_vec()),
            Ok(b"ent\":\"Hello\"}}]}\r\n".to_vec()),
            Ok(b"data: [DONE]\n".to_vec()),
        ]);
        assert_eq!(deltas.len(), 1, "one frame, one delta");
        assert_eq!(deltas[0].response, "Hello");
        assert_eq!(
            out.expect("a [DONE] stream succeeds").text.response,
            "Hello"
        );
    }

    #[test]
    fn drain_stream_flushes_a_trailing_line_at_disconnect() {
        // EOF without [DONE] or a final newline — the buffered line still
        // parses (mirrors the old reader's EOF flush).
        let (out, deltas) = drain_queued(vec![Ok(
            br#"data: {"choices":[{"delta":{"content":"tail"}}]}"#.to_vec(),
        )]);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].response, "tail");
        assert!(out.is_ok());
    }

    #[test]
    fn drain_stream_stops_at_done_and_ignores_later_bytes() {
        let chunk = format!(
            "data: {first}\ndata: [DONE]\ndata: {never}\n",
            first = r#"{"choices":[{"delta":{"content":"first"}}]}"#,
            never = r#"{"choices":[{"delta":{"content":"never"}}]}"#,
        );
        let (out, deltas) = drain_queued(vec![Ok(chunk.into_bytes())]);
        assert_eq!(deltas.len(), 1, "nothing after [DONE] is processed");
        assert_eq!(deltas[0].response, "first");
        assert_eq!(out.unwrap().text.response, "first");
    }

    #[test]
    fn drain_stream_surfaces_a_transport_failure_after_content() {
        // A send/read failure travels the channel as an Err event; the deltas
        // streamed before it stand (the retry driver decides what happens next).
        let (out, deltas) = drain_queued(vec![
            Ok(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n".to_vec()),
            Err(LlmError::Http("connection reset".into())),
        ]);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].response, "Hi");
        assert!(
            matches!(out, Err(LlmError::Http(msg)) if msg.contains("connection reset")),
            "the transport error surfaces"
        );
    }

    #[test]
    fn drain_stream_surfaces_an_in_band_error_frame() {
        let (out, deltas) = drain_queued(vec![Ok(
            br#"data: {"error":{"message":"Provider returned error","code":429}}
"#
            .to_vec(),
        )]);
        assert!(deltas.is_empty());
        match out {
            Err(LlmError::Api { status, body }) => {
                assert_eq!(status, 429);
                assert!(body.contains("Provider returned error"));
            }
            other => panic!("expected the in-band Api error, got {other:?}"),
        }
    }

    #[test]
    fn drain_stream_returns_cancelled_without_waiting_for_the_transport() {
        // The channel is open but silent (a transport still parked in send());
        // a pre-tripped token must return immediately, not after any timeout.
        let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<u8>>>();
        let cancel = CancelToken::new();
        cancel.cancel();
        let out = drain_stream(&rx, &cancel, |_| {});
        drop(tx);
        assert!(matches!(out, Err(LlmError::Cancelled)));
    }

    #[test]
    fn drain_stream_wakes_to_observe_a_cancel_during_a_stall() {
        // A silent transport (stalled send / quiet socket) with a cancel
        // tripped mid-stall: the drain's poll wake must observe it promptly —
        // this is the Esc-interrupt latency, no longer tied to any network
        // timeout.
        let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<u8>>>();
        let cancel = CancelToken::new();
        let canceller = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(80));
                cancel.cancel();
            })
        };
        let started = std::time::Instant::now();
        let out = drain_stream(&rx, &cancel, |_| {});
        let waited = started.elapsed();
        canceller.join().unwrap();
        drop(tx);
        assert!(matches!(out, Err(LlmError::Cancelled)));
        assert!(
            waited < Duration::from_secs(2),
            "the cancel is observed within the poll cadence, waited {waited:?}"
        );
    }

    #[test]
    fn drain_stream_accumulates_tool_calls_and_finish_reason() {
        let (out, deltas) = drain_queued(vec![
            Ok(br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"bash","arguments":"{}"}}]}}]}
"#
            .to_vec()),
            Ok(b"data: {\"choices\":[{\"finish_reason\":\"tool_calls\"}]}\n".to_vec()),
            Ok(b"data: [DONE]\n".to_vec()),
        ]);
        let outcome = out.unwrap();
        assert_eq!(outcome.tool_calls.len(), 1);
        assert_eq!(outcome.tool_calls[0].name, "bash");
        assert_eq!(outcome.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(
            deltas.len(),
            1,
            "the generation fragment surfaced for counting"
        );
        assert_eq!(deltas[0].tool_call, "bash{}");
    }

    #[test]
    fn drain_stream_captures_the_final_usage_frame() {
        // With `stream_options.include_usage` the provider's last data frame
        // (choices empty or final) carries the round's real usage — captured
        // into the outcome, cache detail included (the OpenRouter shape).
        let (out, deltas) = drain_queued(vec![
            Ok(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n".to_vec()),
            Ok(br#"data: {"choices":[],"usage":{"prompt_tokens":8080,"completion_tokens":5,"total_tokens":8085,"prompt_tokens_details":{"cached_tokens":8063,"cache_write_tokens":17,"audio_tokens":0}}}
"#
            .to_vec()),
            Ok(b"data: [DONE]\n".to_vec()),
        ]);
        let outcome = out.expect("the stream succeeds");
        assert_eq!(
            outcome.usage,
            Some(TokenUsage {
                input: 8080,
                output: 5,
                cached: 8063,
                cache_write: 17,
                ..TokenUsage::default()
            })
        );
        assert_eq!(deltas.len(), 1, "the usage frame emits no delta");
        assert_eq!(outcome.text.response, "Hi");
    }

    #[test]
    fn usage_reports_the_reasoning_token_detail() {
        // A reasoning model bills its chain-of-thought under
        // `completion_tokens_details.reasoning_tokens` (verified live against
        // OpenRouter). It is what the committed `Thought for …` cell snaps
        // its estimate to — see docs/thinking-stream.md.
        let (out, _deltas) = drain_queued(vec![
            Ok(br#"data: {"choices":[],"usage":{"prompt_tokens":33,"completion_tokens":74,"total_tokens":107,"prompt_tokens_details":{"cached_tokens":0},"completion_tokens_details":{"reasoning_tokens":23,"image_tokens":0}}}
"#
            .to_vec()),
            Ok(b"data: [DONE]\n".to_vec()),
        ]);
        let usage = out.unwrap().usage.expect("the frame reports usage");
        assert_eq!(usage.reasoning, 23);
        assert_eq!(usage.output, 74, "the reasoning share is part of output");
    }

    #[test]
    fn usage_without_the_reasoning_detail_reports_none_of_it() {
        // A non-reasoning model (or a provider that omits the detail) reports
        // 0, which keeps the cell's tokenizer estimate.
        let (out, _deltas) = drain_queued(vec![
            Ok(br#"data: {"choices":[],"usage":{"prompt_tokens":13,"completion_tokens":2,"total_tokens":15}}
"#
            .to_vec()),
            Ok(b"data: [DONE]\n".to_vec()),
        ]);
        assert_eq!(out.unwrap().usage.unwrap().reasoning, 0);
    }

    #[test]
    fn drain_stream_reads_the_venice_style_usage_aliases() {
        // Venice reports the cache read both as prompt_tokens_details.
        // cached_tokens and a top-level cache_read_input_tokens; an
        // Anthropic-style shim may only send the top-level aliases. Both
        // spellings must land in the same TokenUsage.
        let (out, _deltas) = drain_queued(vec![
            Ok(br#"data: {"choices":[],"usage":{"prompt_tokens":3215,"completion_tokens":2,"total_tokens":3217,"cache_read_input_tokens":3072,"cache_creation_input_tokens":11,"cost":{"usd":0,"diem":0.0003}}}
"#
            .to_vec()),
            Ok(b"data: [DONE]\n".to_vec()),
        ]);
        assert_eq!(
            out.unwrap().usage,
            Some(TokenUsage {
                input: 3215,
                output: 2,
                cached: 3072,
                cache_write: 11,
                ..TokenUsage::default()
            })
        );
    }

    #[test]
    fn null_or_empty_usage_frames_are_not_a_report() {
        // OpenAI-style streams carry `"usage": null` on every delta frame
        // until the final one; an all-zero block is equally meaningless. The
        // outcome must not report usage for either.
        let (out, _deltas) = drain_queued(vec![
            Ok(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}],\"usage\":null}\n".to_vec()),
            Ok(br#"data: {"choices":[],"usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0}}
"#
            .to_vec()),
            Ok(b"data: [DONE]\n".to_vec()),
        ]);
        assert_eq!(out.unwrap().usage, None);
    }

    #[test]
    fn a_plain_json_completion_body_carries_its_usage_too() {
        // The non-SSE fallback (a shim that ignored stream:true): the whole
        // JSON completion's usage block is captured like a streamed one.
        let body = br#"{"choices":[{"message":{"content":"full"},"finish_reason":"stop"}],"usage":{"prompt_tokens":13,"completion_tokens":2,"total_tokens":15,"prompt_tokens_details":{"cached_tokens":0}}}"#
            .to_vec();
        let (out, _deltas) = drain_queued(vec![Ok(body)]);
        assert_eq!(
            out.unwrap().usage,
            Some(TokenUsage {
                input: 13,
                output: 2,
                cached: 0,
                cache_write: 0,
                ..TokenUsage::default()
            })
        );
    }

    #[test]
    fn a_plain_json_completion_body_is_parsed_not_dropped() {
        // An OpenAI-compatible shim that ignores `stream:true` answers with
        // one plain JSON completion — no SSE framing at all. The reply must
        // not vanish into a clean empty StreamDone.
        let body = br#"{"choices":[{"message":{"content":"full answer"},"finish_reason":"stop"}]}"#
            .to_vec();
        let (out, deltas) = drain_queued(vec![Ok(body)]);
        let outcome = out.expect("a parseable JSON completion succeeds");
        assert_eq!(outcome.text.response, "full answer");
        assert_eq!(outcome.finish_reason.as_deref(), Some("stop"));
        assert!(deltas.iter().any(|d| d.response.contains("full answer")));
    }

    #[test]
    fn a_plain_json_error_body_fails_the_stream() {
        // A 200 whose body is a bare JSON error object (no SSE framing).
        let body = br#"{"error":{"message":"quota exhausted","code":429}}"#.to_vec();
        let (out, deltas) = drain_queued(vec![Ok(body)]);
        assert!(deltas.is_empty());
        match out {
            Err(LlmError::Api { status, body }) => {
                assert_eq!(status, 429);
                assert!(body.contains("quota exhausted"));
            }
            other => panic!("expected the plain-body Api error, got {other:?}"),
        }
    }

    #[test]
    fn a_non_sse_garbage_body_is_an_error_not_a_clean_empty_finish() {
        let (out, deltas) = drain_queued(vec![Ok(b"<html>bad gateway</html>".to_vec())]);
        assert!(deltas.is_empty());
        assert!(
            out.is_err(),
            "an unparseable non-SSE body must surface, not finish clean: {out:?}"
        );
    }

    #[test]
    fn a_keep_alive_only_stream_still_finishes_clean() {
        // Real SSE framing (a comment line) with no data frames stays a clean
        // empty finish — only a body with NO framing takes the JSON fallback.
        let (out, deltas) = drain_queued(vec![Ok(b": keep-alive\n\n".to_vec())]);
        let outcome = out.expect("an empty keep-alive stream is not an error");
        assert!(outcome.text.response.is_empty());
        assert!(deltas.is_empty());
    }

    // --- the speed tier (docs/fast-mode.md) ---

    #[test]
    fn a_chat_payload_carries_the_selected_service_tier() {
        // OpenAI's chat completions take the same `service_tier` the
        // Responses wire does; a standard selection sends no field.
        let mut cfg = ModelConfig::fallback();
        cfg.service_tier = Some("priority".to_string());
        let p = OpenAiClient::new(cfg.clone()).build_payload(&[ChatMessage::user("hi")]);
        assert_eq!(p["service_tier"], "priority");
        cfg.service_tier = None;
        let p = OpenAiClient::new(cfg).build_payload(&[ChatMessage::user("hi")]);
        assert!(p.get("service_tier").is_none(), "{p}");
    }

    #[test]
    fn a_chatgpt_request_carries_the_routing_hint_naming_the_model_and_tier() {
        // Codex's `x-codex-routing-hint`: `model={model}` on every request to
        // the ChatGPT backend, `;tier={tier}` appended when a speed tier is
        // selected — the edge's own hint for where a fast request goes.
        let mut cfg = ModelConfig::fallback();
        cfg.auth = crate::llm::AuthScheme::ChatGptCodex;
        cfg.model = "gpt-5.5".to_string();
        assert_eq!(
            built_header(cfg.clone(), "x-codex-routing-hint").as_deref(),
            Some("model=gpt-5.5")
        );
        cfg.service_tier = Some("priority".to_string());
        assert_eq!(
            built_header(cfg, "x-codex-routing-hint").as_deref(),
            Some("model=gpt-5.5;tier=priority")
        );
        // A pasted-key provider never sees codex's header.
        let mut plain = ModelConfig::fallback();
        plain.service_tier = Some("priority".to_string());
        assert_eq!(built_header(plain, "x-codex-routing-hint"), None);
    }

    #[test]
    fn a_pinned_model_drops_the_sessions_speed_tier_with_its_thinking_mode() {
        // A subagent definition's `model:` swaps the model in; the tier was
        // detected for the model being replaced, and a tier the new model
        // does not offer is a request the backend may refuse — so it goes
        // the way the thinking mode and the vision verdict go.
        let mut cfg = ModelConfig::fallback();
        cfg.service_tier = Some("priority".to_string());
        let p = OpenAiClient::new(cfg)
            .with_model("other")
            .build_payload(&[ChatMessage::user("hi")]);
        assert!(p.get("service_tier").is_none(), "{p}");
    }
}

#[cfg(test)]
mod body_tests {
    //! The request body on the wire (`docs/memory.md`): serialized from the
    //! messages by reference, streamed through a pipe of small chunks, never
    //! held whole.

    use super::*;
    use crate::llm::{ContentPart, MessageContent};

    fn client(model: &str) -> OpenAiClient {
        let mut cfg = ModelConfig::fallback();
        cfg.model = model.to_string();
        cfg.temperature = Some(0.5);
        cfg.extra_body.insert(
            "venice_parameters".into(),
            json!({"include_venice_system_prompt": false}),
        );
        OpenAiClient::new(cfg).with_tools(vec![
            json!({"type": "function", "function": {"name": "bash"}}),
        ])
    }

    fn vision_messages() -> Vec<ChatMessage> {
        vec![
            ChatMessage::system("persona"),
            ChatMessage::with_parts(
                "user",
                vec![
                    ContentPart::text("[Image #1: /tmp/1.png] what is this?"),
                    ContentPart::image("data:image/png;base64,AAAA"),
                ],
            ),
            ChatMessage::assistant("a cat"),
            ChatMessage::user("and this?"),
        ]
    }

    use crate::llm::body::drain;

    #[test]
    fn the_streamed_body_is_the_payload_serialized_without_a_tree_of_the_messages() {
        let client = client("gpt-4o-mini");
        let messages = vision_messages();
        let (body, len) = client.request_stream(messages.clone()).expect("streams");
        let bytes = drain(body);
        assert_eq!(
            bytes,
            serde_json::to_vec(&client.build_payload(&messages)).unwrap(),
            "byte-for-byte what the tree form would have sent"
        );
        assert_eq!(len, bytes.len() as u64, "the declared length is the truth");
    }

    #[test]
    fn a_breakpoint_model_gets_its_markers_in_the_body_too() {
        let client = client("anthropic/claude-sonnet-4.6");
        let messages = vision_messages();
        let (body, _) = client.request_stream(messages.clone()).expect("streams");
        let tree: serde_json::Value = serde_json::from_slice(&drain(body)).unwrap();
        assert_eq!(tree, client.build_payload(&messages));
        assert_eq!(
            tree["messages"][0]["content"][0]["cache_control"],
            json!({"type": "ephemeral"}),
            "the system prefix is marked"
        );
        assert_eq!(
            tree["messages"][1]["content"][0]["cache_control"],
            json!({"type": "ephemeral"}),
            "the previous user message is marked on its text part"
        );
        assert!(
            tree["messages"][1]["content"][1]
                .get("cache_control")
                .is_none(),
            "never the image part"
        );
        assert_eq!(
            tree["messages"][3]["content"][0]["cache_control"],
            json!({"type": "ephemeral"})
        );
        assert!(
            messages[0].content == MessageContent::Text("persona".into()),
            "the caller's messages are untouched"
        );
    }
}
