//! The [`ReplySource`] bridge: turns the [`OpenAiClient`]'s split deltas into the
//! app's [`StreamEvent`] protocol so a real model is a drop-in for `DummyAi`.
//!
//! Boundary code (it spawns the streaming thread), but the message assembly is
//! factored into the pure [`build_messages`] so it's unit-tested. See
//! `docs/llm.md`.

use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};

use tokio::sync::mpsc::UnboundedSender;

use super::agent::{self, RoundOutcome};
use super::config::ModelConfig;
use super::exec::{RealToolExecutor, ToolExecutor};
use super::openai::{Delta, OpenAiClient};
use super::retry::{self, AttemptResult, MAX_RETRIES};
use super::tools::{self, ToolCallRequest};
use super::{ChatMessage, ContentPart, LlmError, ToolCallSpec};
use crate::context::ContextMessage;
use crate::stream::{CancelToken, ReplySource, StreamEvent};

/// The default system prompt for the real backend — the "Alter Zero" agent
/// identity, authored in [`prompts/alter_zero.md`](../../prompts/alter_zero.md)
/// and compiled in with `include_str!` so the wording lives in a maintainable
/// markdown file (drop in a new `prompts/*.md` and point this const at it to
/// swap personas). Kept short to save tokens. Tool calls and `!` shell runs now
/// replay in the provider-native format (see `docs/context.md`), so the prompt
/// no longer has to explain any bracketed records. Override with
/// `INLINE_TUI_SYSTEM_PROMPT`.
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../../prompts/alter_zero.md");

/// A real OpenAI-compatible backend. Holds the streaming client (carrying the
/// tool definitions when tools are enabled) plus the model id it answers as
/// (for the session footer), whether tools are enabled, and an optional system
/// prompt.
#[derive(Debug, Clone)]
pub struct LlmBackend {
    client: OpenAiClient,
    model: String,
    system_prompt: Option<String>,
    tools_enabled: bool,
    /// The shared background-shell registry, when the boundary attached one —
    /// enables the `bash` tool's `run_in_background` and Ctrl+B handoff
    /// (`docs/background.md`).
    background: Option<crate::background::BackgroundRegistry>,
}

impl LlmBackend {
    /// Build a backend from a resolved [`ModelConfig`], with the default system
    /// prompt.
    #[must_use]
    pub fn new(cfg: ModelConfig) -> Self {
        Self::with_system_prompt(cfg, Some(DEFAULT_SYSTEM_PROMPT.to_string()))
    }

    /// Build a backend with an explicit system prompt (`None` sends no system
    /// message). The boundary passes `INLINE_TUI_SYSTEM_PROMPT` through here.
    /// Tools are enabled unless `INLINE_TUI_TOOLS` is falsy (see `docs/tools.md`).
    #[must_use]
    pub fn with_system_prompt(cfg: ModelConfig, system_prompt: Option<String>) -> Self {
        Self::configure(cfg, system_prompt, tools_enabled_from_env())
    }

    /// Build a backend with an explicit tools toggle (the tests' seam; the app
    /// goes through [`with_system_prompt`], reading the env).
    ///
    /// [`with_system_prompt`]: LlmBackend::with_system_prompt
    #[must_use]
    pub fn configure(cfg: ModelConfig, system_prompt: Option<String>, tools_enabled: bool) -> Self {
        let model = cfg.model.clone();
        let mut client = OpenAiClient::new(cfg);
        // Trim so the prompt-file trailing newline (or a whitespace-only
        // override) normalizes away; a now-empty prompt sends no system message.
        let mut system_prompt = system_prompt
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if tools_enabled {
            client = client.with_tools(tools::tool_specs());
            // Tell the model the tools exist (the schemas carry the detail);
            // the Ctrl+D debug view then shows the same augmented prompt.
            if let Some(prompt) = system_prompt.as_mut() {
                prompt.push_str("\n\n");
                prompt.push_str(TOOLS_SYSTEM_SUFFIX.trim());
            }
        }
        Self {
            client,
            model,
            system_prompt,
            tools_enabled,
            background: None,
        }
    }

    /// Attach the shared background-shell registry so the executor can launch
    /// and adopt background tasks (`docs/background.md`). The boundary calls
    /// this on every backend it builds; tests and offline tools skip it.
    #[must_use]
    pub fn with_background(mut self, registry: crate::background::BackgroundRegistry) -> Self {
        self.background = Some(registry);
        self
    }

    /// Whether the `bash`/`read`/`write`/`edit` tools are offered to the model.
    #[must_use]
    pub fn tools_enabled(&self) -> bool {
        self.tools_enabled
    }
}

/// Are the `bash`/`read`/`write`/`edit` tools enabled? On by default; disabled
/// by a falsy `INLINE_TUI_TOOLS` (`0`/`false`/`no`/`off`). See `docs/tools.md`.
fn tools_enabled_from_env() -> bool {
    match std::env::var("INLINE_TUI_TOOLS") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// The tool-capability note appended to the system prompt when tools are
/// enabled, authored in [`prompts/tools.md`](../../prompts/tools.md) and
/// compiled in with `include_str!` (same maintainable-markdown seam as
/// [`DEFAULT_SYSTEM_PROMPT`]). Joined after a blank line; the schemas carry the
/// per-parameter detail.
const TOOLS_SYSTEM_SUFFIX: &str = include_str!("../../prompts/tools.md");

/// Assemble the request messages for one turn: the optional system prompt,
/// then the whole conversation context in order — the multi-turn memory (see
/// `docs/context.md`). A context message with image attachments becomes the
/// multimodal parts form, each attachment encoded to a `data:` URL by
/// `encode_image` (the injected I/O seam — [`image_data_url`] in production,
/// a fake in tests, keeping this pure). An attachment that fails to encode
/// (its temp file may have been cleaned away) is noted in the text instead of
/// being dropped silently. Should the context ever be empty, the bare
/// `prompt` is sent so the request is never user-less.
#[must_use]
pub fn build_messages(
    system_prompt: Option<&str>,
    prompt: &str,
    context: &[ContextMessage],
    encode_image: impl Fn(&Path) -> Option<String>,
) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    if let Some(sys) = system_prompt {
        messages.push(ChatMessage::system(sys));
    }
    for message in context {
        messages.push(chat_message(message, &encode_image));
    }
    if context.is_empty() {
        messages.push(ChatMessage::user(prompt));
    }
    messages
}

/// One context message as a wire [`ChatMessage`]: an assistant tool-call turn
/// or a `tool`-role result in the provider-native shape, plain text when
/// imageless, or the multimodal parts array when attachments encode.
fn chat_message(
    message: &ContextMessage,
    encode_image: &impl Fn(&Path) -> Option<String>,
) -> ChatMessage {
    let role = message.role.wire_name();
    // A tool result carries its call id; an assistant entry may carry the native
    // tool calls it requested — both replay in the Chat Completions tool shape
    // (see docs/context.md), and neither ever has image attachments.
    if let Some(id) = &message.tool_call_id {
        return ChatMessage::tool_result(id, &message.text);
    }
    if !message.tool_calls.is_empty() {
        let specs = message
            .tool_calls
            .iter()
            .map(|c| ToolCallSpec::function(&c.id, &c.name, &c.arguments))
            .collect();
        return ChatMessage::assistant_tool_calls(&message.text, specs);
    }
    if message.images.is_empty() {
        return ChatMessage::new(role, &message.text);
    }
    let mut text = message.text.clone();
    let mut image_parts = Vec::new();
    for path in &message.images {
        match encode_image(path) {
            Some(url) => image_parts.push(ContentPart::image(url)),
            None => {
                // The temp file is gone (e.g. cleaned between sessions) —
                // tell the model rather than silently dropping the image.
                text.push_str(&format!("\n[image unavailable: {}]", path.display()));
            }
        }
    }
    if image_parts.is_empty() {
        return ChatMessage::new(role, text);
    }
    let mut parts = vec![ContentPart::text(text)];
    parts.extend(image_parts);
    ChatMessage::with_parts(role, parts)
}

/// Read an attached image and embed it as a base64 `data:` URL — the OpenAI
/// vision shape. Boundary code (file I/O); `None` when the file is unreadable,
/// which [`chat_message`] surfaces as a text note.
fn image_data_url(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(format!(
        "data:{};base64,{}",
        image_mime(path),
        crate::clipboard::base64_encode(&bytes)
    ))
}

/// The MIME type for an attachment path, by extension. The clipboard paste
/// only ever writes the accepted formats (`docs/image-paste.md`); PNG — its
/// transcode target — is the default.
fn image_mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

impl ReplySource for LlmBackend {
    fn spawn(
        &self,
        prompt: String,
        _images: Vec<PathBuf>, // the context's last user message carries them
        context: Vec<ContextMessage>,
        tx: UnboundedSender<StreamEvent>,
        cancel: CancelToken,
    ) -> JoinHandle<()> {
        let client = self.client.clone();
        let system = self.system_prompt.clone();
        let background = self.background.clone();
        thread::spawn(move || {
            // Encoding the attachments reads files — done here on the backend
            // thread so a large image never stalls the event loop.
            let messages = build_messages(system.as_deref(), &prompt, &context, image_data_url);
            let mut executor = RealToolExecutor::new();
            let notices = background.clone();
            if let Some(registry) = background {
                executor = executor.with_background(registry);
            }
            // The agentic loop: `run_agent` streams one round, runs any tool
            // calls the model requested (via `executor`, emitting the
            // ToolStart/ToolEnd pair the TUI renders), appends the results, and
            // loops until the model answers with plain text — sending the
            // terminal StreamDone/Error itself. Each round retries transient
            // failures internally (see `stream_round`). With tools disabled the
            // model never asks for any, so this collapses to a single round —
            // the old plain-stream behaviour. See `docs/tools.md`. Before each
            // round it takes the registry's completion notice board, so a
            // background shell that finished (or was killed) since the last
            // request is known to the model within this same turn
            // (docs/background.md).
            agent::run_agent(
                &tx,
                &cancel,
                agent::MAX_TOOL_ITERATIONS,
                messages,
                |msgs| stream_round(&client, msgs, &tx, &cancel),
                |call, on_output| executor.execute(call, &cancel, on_output),
                || match &notices {
                    Some(registry) => registry
                        .take_pending_notices()
                        .into_iter()
                        .map(|note| note.context)
                        .collect(),
                    None => Vec::new(),
                },
            );
        })
    }

    fn model_name(&self) -> String {
        self.model.clone()
    }

    fn system_prompt(&self) -> Option<String> {
        self.system_prompt.clone()
    }
}

/// Stream one round of the conversation: one HTTP request (with the existing
/// per-request retry), emitting the `Chunk`/`Thinking*` events as text and
/// reasoning arrive, and reporting the [`RoundOutcome`] the agent loop acts on
/// — `Complete` (plain answer), `ToolCalls` (the model wants to run tools),
/// `Cancelled`, or `Failed`. Boundary code (real HTTP); the loop that calls it
/// and the retry it wraps are unit-tested separately.
fn stream_round(
    client: &OpenAiClient,
    messages: &[ChatMessage],
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
) -> RoundOutcome {
    // The successful attempt's outcome (text + tool calls) is stashed here so
    // it survives the retry driver, which only reports the disposition.
    let mut captured = None;
    // One streaming attempt: stream the deltas as events and report the outcome
    // plus whether it emitted any content. `retry::run_attempts` re-runs it —
    // after a `Retrying` announcement + backoff — only for a retryable failure
    // that emitted nothing, so a retry can never duplicate streamed text.
    let attempt = || {
        let mut emitted = false;
        // Track the thinking phase so a reasoning burst opens exactly one
        // ThinkingStart and the first response text after it closes with a
        // ThinkingEnd — the pairing the status line expects.
        let mut thinking = false;
        let result = client.stream_chat(messages.to_vec(), cancel, |delta: Delta| {
            if !delta.reasoning.is_empty() {
                emitted = true;
                if !thinking {
                    thinking = true;
                    let _ = tx.send(StreamEvent::ThinkingStart);
                }
                let _ = tx.send(StreamEvent::ThinkingChunk(delta.reasoning));
            }
            if !delta.response.is_empty() {
                emitted = true;
                if thinking {
                    thinking = false;
                    let _ = tx.send(StreamEvent::ThinkingEnd);
                }
                let _ = tx.send(StreamEvent::Chunk(delta.response));
            }
            if !delta.tool_call.is_empty() {
                // The model is generating a tool call: count its tokens so the
                // status keeps ticking (docs/status-indicator.md). Close the
                // thinking phase first if a reasoning burst preceded the call.
                emitted = true;
                if thinking {
                    thinking = false;
                    let _ = tx.send(StreamEvent::ThinkingEnd);
                }
                let _ = tx.send(StreamEvent::ToolCallDelta(delta.tool_call));
            }
        });
        // A stream that ends while still "thinking" (reasoning only, no
        // response) must still close the phase.
        if thinking {
            let _ = tx.send(StreamEvent::ThinkingEnd);
        }
        match result {
            Ok(outcome) => {
                // A tool-calling round emits no visible reply text, so treat it
                // as "emitted" too — a retry would re-run tools, which we never
                // want (it can never duplicate here since Ok isn't retried).
                let had_tools = !outcome.tool_calls.is_empty();
                captured = Some(outcome);
                (AttemptResult::Ok, emitted || had_tools)
            }
            Err(LlmError::Cancelled) => (AttemptResult::Cancelled, emitted),
            Err(e) => (AttemptResult::Failed(e), emitted),
        }
    };
    match retry::run_attempts(tx, cancel, MAX_RETRIES, attempt, retry::sleep_cancellable) {
        AttemptResult::Ok => {
            let outcome = captured.unwrap_or_default();
            if outcome.tool_calls.is_empty() {
                RoundOutcome::Complete
            } else {
                let assistant = ChatMessage::assistant_tool_calls(
                    outcome.text.response,
                    to_tool_call_specs(&outcome.tool_calls),
                );
                RoundOutcome::ToolCalls {
                    assistant,
                    calls: outcome.tool_calls,
                }
            }
        }
        AttemptResult::Cancelled => RoundOutcome::Cancelled,
        AttemptResult::Failed(e) => RoundOutcome::Failed(e),
    }
}

/// Echo the model's requested tool calls back as the assistant message's
/// `tool_calls` array, so the provider can pair each `role:"tool"` result to
/// its call.
fn to_tool_call_specs(calls: &[ToolCallRequest]) -> Vec<ToolCallSpec> {
    calls
        .iter()
        .map(|c| ToolCallSpec::function(&c.id, &c.name, &c.arguments))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::{ContextMessage, ContextRole, ContextToolCall};
    use crate::llm::MessageContent;

    /// A fake encoder for the pure tests: every path "encodes" to a data URL
    /// naming its file, so assertions can tell attachments apart.
    fn fake_encode(path: &Path) -> Option<String> {
        Some(format!("data:image/png;base64,{}", path.display()))
    }

    /// An encoder whose files are all unreadable.
    fn failing_encode(_path: &Path) -> Option<String> {
        None
    }

    #[test]
    fn build_messages_sends_the_system_prompt_then_the_whole_context() {
        let context = vec![
            ContextMessage::new(ContextRole::User, "hello"),
            ContextMessage::new(ContextRole::Assistant, "hi!"),
            ContextMessage::new(ContextRole::User, "and again"),
        ];
        let msgs = build_messages(Some("be nice"), "and again", &context, fake_encode);
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[0].content, MessageContent::Text("be nice".into()));
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[1].content, MessageContent::Text("hello".into()));
        assert_eq!(msgs[2].role, "assistant");
        assert_eq!(msgs[2].content, MessageContent::Text("hi!".into()));
        assert_eq!(msgs[3].role, "user");
        assert_eq!(msgs[3].content, MessageContent::Text("and again".into()));
    }

    #[test]
    fn build_messages_translates_native_tool_calls_and_results() {
        // A replayed tool round from history: the assistant `tool_calls` entry
        // and its `tool`-role result become the provider-native wire messages
        // (not the old bracketed text) — see docs/context.md.
        let context = vec![
            ContextMessage::assistant_tool_calls(
                "let me check",
                vec![ContextToolCall::new("call_0", "read", r#"{"path":"f"}"#)],
            ),
            ContextMessage::tool_result("call_0", "L1"),
        ];
        let msgs = build_messages(None, "", &context, fake_encode);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "assistant");
        assert_eq!(msgs[0].content, MessageContent::Text("let me check".into()));
        assert_eq!(
            msgs[0].tool_calls,
            vec![ToolCallSpec::function("call_0", "read", r#"{"path":"f"}"#)]
        );
        assert!(msgs[0].tool_call_id.is_none());
        assert_eq!(msgs[1].role, "tool");
        assert_eq!(msgs[1].content, MessageContent::Text("L1".into()));
        assert!(msgs[1].tool_calls.is_empty());
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("call_0"));
    }

    #[test]
    fn build_messages_omits_the_system_prompt_when_none() {
        let context = vec![ContextMessage::new(ContextRole::User, "hello")];
        let msgs = build_messages(None, "hello", &context, fake_encode);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn build_messages_falls_back_to_the_prompt_on_an_empty_context() {
        // The context always ends with the just-recorded user message, but if
        // it were ever empty the request must still carry the user's text.
        let msgs = build_messages(None, "hello", &[], fake_encode);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, MessageContent::Text("hello".into()));
    }

    #[test]
    fn a_context_message_with_images_becomes_multimodal_parts() {
        let context = vec![ContextMessage {
            role: ContextRole::User,
            text: "[Image #1] what is this?".into(),
            images: vec![PathBuf::from("/tmp/shot.png")],
            tool_calls: vec![],
            tool_call_id: None,
        }];
        let msgs = build_messages(None, "", &context, fake_encode);
        assert_eq!(
            msgs[0].content,
            MessageContent::Parts(vec![
                ContentPart::text("[Image #1] what is this?"),
                ContentPart::image("data:image/png;base64,/tmp/shot.png"),
            ])
        );
    }

    #[test]
    fn an_unreadable_image_is_noted_in_the_text_not_dropped_silently() {
        let context = vec![ContextMessage {
            role: ContextRole::User,
            text: "look".into(),
            images: vec![PathBuf::from("/tmp/gone.png")],
            tool_calls: vec![],
            tool_call_id: None,
        }];
        let msgs = build_messages(None, "", &context, failing_encode);
        // No image encoded → back to plain text, with the loss noted.
        assert_eq!(
            msgs[0].content,
            MessageContent::Text("look\n[image unavailable: /tmp/gone.png]".into())
        );
    }

    #[test]
    fn a_readable_and_an_unreadable_image_mix_into_parts_plus_a_note() {
        let context = vec![ContextMessage {
            role: ContextRole::User,
            text: "both".into(),
            images: vec![PathBuf::from("/tmp/gone.png"), PathBuf::from("/tmp/ok.png")],
            tool_calls: vec![],
            tool_call_id: None,
        }];
        let encode = |path: &Path| {
            (path == Path::new("/tmp/ok.png")).then(|| "data:image/png;base64,OK".to_string())
        };
        let msgs = build_messages(None, "", &context, encode);
        assert_eq!(
            msgs[0].content,
            MessageContent::Parts(vec![
                ContentPart::text("both\n[image unavailable: /tmp/gone.png]"),
                ContentPart::image("data:image/png;base64,OK"),
            ])
        );
    }

    #[test]
    fn image_mime_maps_the_accepted_extensions() {
        assert_eq!(image_mime(Path::new("a.png")), "image/png");
        assert_eq!(image_mime(Path::new("a.JPG")), "image/jpeg");
        assert_eq!(image_mime(Path::new("a.jpeg")), "image/jpeg");
        assert_eq!(image_mime(Path::new("a.gif")), "image/gif");
        assert_eq!(image_mime(Path::new("a.webp")), "image/webp");
        assert_eq!(image_mime(Path::new("no-extension")), "image/png");
    }

    #[test]
    fn image_data_url_embeds_the_file_as_base64() {
        // The one boundary helper, exercised with a real temp file.
        let dir = std::env::temp_dir();
        let path = dir.join("inline-tui-test-image-data-url.png");
        std::fs::write(&path, b"foobar").unwrap();
        let url = image_data_url(&path).expect("readable file encodes");
        std::fs::remove_file(&path).ok();
        assert_eq!(url, "data:image/png;base64,Zm9vYmFy");
        assert!(image_data_url(Path::new("/definitely/not/here.png")).is_none());
    }

    #[test]
    fn backend_reports_the_configured_model_name() {
        let mut cfg = ModelConfig::fallback();
        cfg.model = "anthropic/claude-fable-5".to_string();
        let backend = LlmBackend::new(cfg);
        assert_eq!(backend.model_name(), "anthropic/claude-fable-5");
    }

    #[test]
    fn backend_surfaces_its_system_prompt_for_the_debug_view() {
        // Tools off so the prompt isn't augmented — deterministic regardless of
        // the ambient INLINE_TUI_TOOLS (the augmented case has its own test).
        let backend = LlmBackend::configure(ModelConfig::fallback(), Some("be nice".into()), false);
        assert_eq!(
            ReplySource::system_prompt(&backend).as_deref(),
            Some("be nice")
        );
    }

    #[test]
    fn blank_system_prompt_is_dropped() {
        let backend = LlmBackend::with_system_prompt(ModelConfig::fallback(), Some("  ".into()));
        assert!(backend.system_prompt.is_none());
    }

    #[test]
    fn enabling_tools_augments_the_system_prompt_and_flags_the_backend() {
        let backend = LlmBackend::configure(ModelConfig::fallback(), Some("be nice".into()), true);
        assert!(backend.tools_enabled());
        let prompt = backend.system_prompt.as_deref().unwrap();
        assert!(prompt.starts_with("be nice"));
        assert!(prompt.contains("bash"), "the tools are named in the prompt");
        assert!(prompt.contains("edit"));
    }

    #[test]
    fn disabling_tools_leaves_the_prompt_untouched() {
        let backend = LlmBackend::configure(ModelConfig::fallback(), Some("be nice".into()), false);
        assert!(!backend.tools_enabled());
        assert_eq!(backend.system_prompt.as_deref(), Some("be nice"));
    }

    #[test]
    fn to_tool_call_specs_echoes_id_name_and_arguments() {
        let calls = vec![ToolCallRequest {
            id: "c1".into(),
            name: "bash".into(),
            arguments: r#"{"command":"ls"}"#.into(),
        }];
        let specs = to_tool_call_specs(&calls);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id, "c1");
        assert_eq!(specs[0].function.name, "bash");
        assert_eq!(specs[0].function.arguments, r#"{"command":"ls"}"#);
    }
}
