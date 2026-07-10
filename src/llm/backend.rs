//! The [`ReplySource`] bridge: turns the [`OpenAiClient`]'s split deltas into the
//! app's [`StreamEvent`] protocol so a real model is a drop-in for `DummyAi`.
//!
//! Boundary code (it spawns the streaming thread), but the message assembly is
//! factored into the pure [`build_messages`] so it's unit-tested. See
//! `docs/llm.md`.

use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};

use tokio::sync::mpsc::UnboundedSender;

use super::config::ModelConfig;
use super::openai::OpenAiClient;
use super::retry::{self, AttemptResult, MAX_RETRIES};
use super::{ChatMessage, ContentPart, LlmError};
use crate::context::ContextMessage;
use crate::stream::{CancelToken, ReplySource, StreamEvent};

/// The default system prompt for the real backend — kept short and neutral.
/// The request carries the whole conversation context, including the raw
/// bracketed records of tool calls, `!` shell runs, and TUI notices (see
/// `docs/context.md`), so the prompt explains what those are. Override with
/// `INLINE_TUI_SYSTEM_PROMPT`.
pub const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful assistant running inside a terminal UI. The conversation \
     history may contain bracketed records — [tool …] entries for tools, \
     [shell …] entries for commands the user ran locally, and [system …]/\
     [error …] notes from the UI. Treat them as context; do not imitate their \
     format. Keep replies concise and well-formatted for a narrow terminal.";

/// A real OpenAI-compatible backend. Holds the streaming client plus the model
/// id it answers as (for the session footer) and an optional system prompt.
#[derive(Debug, Clone)]
pub struct LlmBackend {
    client: OpenAiClient,
    model: String,
    system_prompt: Option<String>,
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
    #[must_use]
    pub fn with_system_prompt(cfg: ModelConfig, system_prompt: Option<String>) -> Self {
        let model = cfg.model.clone();
        Self {
            client: OpenAiClient::new(cfg),
            model,
            system_prompt: system_prompt.filter(|s| !s.trim().is_empty()),
        }
    }
}

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

/// One context message as a wire [`ChatMessage`]: plain text when imageless,
/// the multimodal parts array when attachments encode.
fn chat_message(
    message: &ContextMessage,
    encode_image: &impl Fn(&Path) -> Option<String>,
) -> ChatMessage {
    let role = message.role.wire_name();
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
        thread::spawn(move || {
            // Encoding the attachments reads files — done here on the backend
            // thread so a large image never stalls the event loop.
            let messages = build_messages(system.as_deref(), &prompt, &context, image_data_url);
            // One streaming attempt: stream the deltas as events and report the
            // outcome plus whether it emitted any content. The retry driver
            // (`retry::run_stream`) calls this again — after announcing a
            // `Retrying` event and backing off — only for a retryable failure
            // that emitted nothing (a connection/send failure before the first
            // byte), so a retry can never duplicate streamed text. See
            // `docs/llm.md`.
            let attempt = || {
                let mut emitted = false;
                // Track the thinking phase so a reasoning burst opens exactly one
                // ThinkingStart and the first response text after it closes with a
                // ThinkingEnd — the pairing the status line expects.
                let mut thinking = false;
                let result = client.stream_chat(messages.clone(), &cancel, |delta| {
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
                });
                // A stream that ends while still "thinking" (reasoning only, no
                // response) must still close the phase. Thinking implies content
                // was emitted, so this only ever fires on the final attempt.
                if thinking {
                    let _ = tx.send(StreamEvent::ThinkingEnd);
                }
                let outcome = match result {
                    Ok(_) => AttemptResult::Ok,
                    Err(LlmError::Cancelled) => AttemptResult::Cancelled,
                    Err(e) => AttemptResult::Failed(e),
                };
                (outcome, emitted)
            };
            // The driver sends the terminal StreamDone/Error (or nothing on a
            // cancel) and every Retrying announcement itself.
            retry::run_stream(&tx, &cancel, MAX_RETRIES, attempt, retry::sleep_cancellable);
        })
    }

    fn model_name(&self) -> String {
        self.model.clone()
    }

    fn system_prompt(&self) -> Option<String> {
        self.system_prompt.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::{ContextMessage, ContextRole};
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
        let backend =
            LlmBackend::with_system_prompt(ModelConfig::fallback(), Some("be nice".into()));
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
}
