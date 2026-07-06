//! The [`ReplySource`] bridge: turns the [`OpenAiClient`]'s split deltas into the
//! app's [`StreamEvent`] protocol so a real model is a drop-in for `DummyAi`.
//!
//! Boundary code (it spawns the streaming thread), but the message assembly is
//! factored into the pure [`build_messages`] so it's unit-tested. See
//! `docs/llm.md`.

use std::path::PathBuf;
use std::thread::{self, JoinHandle};

use tokio::sync::mpsc::UnboundedSender;

use super::config::ModelConfig;
use super::openai::OpenAiClient;
use super::{ChatMessage, LlmError};
use crate::stream::{CancelToken, ReplySource, StreamEvent};

/// The default system prompt for the real backend — kept short and neutral. The
/// app is a single-turn demo shell (the seam passes only the current turn's
/// text, no history), so the model is told as much. Override with
/// `INLINE_TUI_SYSTEM_PROMPT`.
pub const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful assistant running inside a terminal UI. Keep replies concise \
     and well-formatted for a narrow terminal.";

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

/// Assemble the request messages for one turn: the optional system prompt, then
/// the user's prompt (with a note appended when images are attached — this
/// backend has no vision, so it acknowledges them in text rather than silently
/// dropping them, mirroring the dummy). Pure, so it's testable.
#[must_use]
pub fn build_messages(
    system_prompt: Option<&str>,
    prompt: &str,
    image_count: usize,
) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    if let Some(sys) = system_prompt {
        messages.push(ChatMessage::system(sys));
    }
    let content = if image_count > 0 {
        let noun = if image_count == 1 { "image" } else { "images" };
        format!("{prompt}\n\n[{image_count} {noun} attached — this backend has no vision support]")
    } else {
        prompt.to_string()
    };
    messages.push(ChatMessage::user(content));
    messages
}

impl ReplySource for LlmBackend {
    fn spawn(
        &self,
        prompt: String,
        images: Vec<PathBuf>,
        tx: UnboundedSender<StreamEvent>,
        cancel: CancelToken,
    ) -> JoinHandle<()> {
        let client = self.client.clone();
        let system = self.system_prompt.clone();
        let image_count = images.len();
        thread::spawn(move || {
            let messages = build_messages(system.as_deref(), &prompt, image_count);
            // Track the thinking phase so a reasoning burst opens exactly one
            // ThinkingStart and the first response text after it closes with a
            // ThinkingEnd — the pairing the status line expects.
            let mut thinking = false;
            let result = client.stream_chat(messages, &cancel, |delta| {
                if !delta.reasoning.is_empty() {
                    if !thinking {
                        thinking = true;
                        let _ = tx.send(StreamEvent::ThinkingStart);
                    }
                    let _ = tx.send(StreamEvent::ThinkingChunk(delta.reasoning));
                }
                if !delta.response.is_empty() {
                    if thinking {
                        thinking = false;
                        let _ = tx.send(StreamEvent::ThinkingEnd);
                    }
                    let _ = tx.send(StreamEvent::Chunk(delta.response));
                }
            });
            // A stream that ends while still "thinking" (reasoning only, no
            // response) must still close the phase.
            if thinking {
                let _ = tx.send(StreamEvent::ThinkingEnd);
            }
            match result {
                Ok(_) => {
                    let _ = tx.send(StreamEvent::StreamDone);
                }
                // A cancel is a silent stop, like the dummy — the loop's
                // interrupt path commits the notice, not the backend.
                Err(LlmError::Cancelled) => {}
                Err(e) => {
                    let _ = tx.send(StreamEvent::Error(e.to_string()));
                }
            }
        })
    }

    fn model_name(&self) -> String {
        self.model.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_messages_includes_the_system_prompt() {
        let msgs = build_messages(Some("be nice"), "hello", 0);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[0].content, "be nice");
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[1].content, "hello");
    }

    #[test]
    fn build_messages_omits_the_system_prompt_when_none() {
        let msgs = build_messages(None, "hello", 0);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn build_messages_notes_attached_images() {
        let msgs = build_messages(None, "what is this", 2);
        assert!(msgs[0].content.contains("what is this"));
        assert!(msgs[0].content.contains("2 images attached"));
    }

    #[test]
    fn build_messages_uses_the_singular_for_one_image() {
        let msgs = build_messages(None, "x", 1);
        assert!(msgs[0].content.contains("1 image attached"));
    }

    #[test]
    fn backend_reports_the_configured_model_name() {
        let mut cfg = ModelConfig::fallback();
        cfg.model = "anthropic/claude-fable-5".to_string();
        let backend = LlmBackend::new(cfg);
        assert_eq!(backend.model_name(), "anthropic/claude-fable-5");
    }

    #[test]
    fn blank_system_prompt_is_dropped() {
        let backend = LlmBackend::with_system_prompt(ModelConfig::fallback(), Some("  ".into()));
        assert!(backend.system_prompt.is_none());
    }
}
