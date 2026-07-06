//! OpenAI-compatible streaming client.
//!
//! Talks to anything accepting the `/chat/completions` shape (OpenAI, OpenRouter,
//! Groq, Together, Sambanova, Ollama, …). The endpoint/payload builders and the
//! SSE frame parse are pure and unit-tested; [`OpenAiClient::stream_chat`] is the
//! boundary that opens the blocking request and drains it. See `docs/llm.md`.

use std::io::{BufRead, BufReader};

use serde::Deserialize;
use serde_json::json;

use super::config::ModelConfig;
use super::thinking::ThinkingSplitter;
use super::{ChatMessage, LlmError, Result};
use crate::stream::CancelToken;

/// A split delta surfaced to the caller each SSE frame: the visible response
/// text and the (hidden) reasoning text, already peeled apart by the
/// [`ThinkingSplitter`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Delta {
    pub response: String,
    pub reasoning: String,
}

/// A streaming chat client bound to one [`ModelConfig`].
#[derive(Debug, Clone)]
pub struct OpenAiClient {
    cfg: ModelConfig,
}

impl OpenAiClient {
    #[must_use]
    pub fn new(cfg: ModelConfig) -> Self {
        Self { cfg }
    }

    /// The chat-completions endpoint (`{api_base}/chat/completions`), falling
    /// back to OpenAI when the base is empty.
    #[must_use]
    pub fn endpoint(&self) -> String {
        let base = if self.cfg.api_base.is_empty() {
            "https://api.openai.com/v1"
        } else {
            self.cfg.api_base.trim_end_matches('/')
        };
        format!("{base}/chat/completions")
    }

    /// The streamed request body: model, messages, `stream: true`, the optional
    /// temperature, and any provider `extra_body` (e.g. `venice_parameters`)
    /// merged in.
    #[must_use]
    pub fn build_payload(&self, messages: &[ChatMessage]) -> serde_json::Value {
        let mut payload = json!({
            "model": self.cfg.model,
            "messages": messages,
            "stream": true,
        });
        if let Some(t) = self.cfg.temperature {
            payload["temperature"] = json!(t);
        }
        if let Some(obj) = payload.as_object_mut() {
            for (k, v) in &self.cfg.extra_body {
                obj.insert(k.clone(), v.clone());
            }
        }
        payload
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
        mut on_delta: impl FnMut(Delta),
    ) -> Result<super::thinking::ChatStreamResult> {
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        let client = super::http_client()?;
        let mut req = client
            .post(self.endpoint())
            .header("accept", "text/event-stream")
            .header("content-type", "application/json")
            .json(&self.build_payload(&messages));
        if let Some(key) = &self.cfg.api_key {
            req = req.bearer_auth(key);
        }
        for (k, v) in &self.cfg.extra_headers {
            req = req.header(k, v);
        }

        let resp = req.send().map_err(|e| LlmError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().unwrap_or_default();
            return Err(LlmError::Api { status, body });
        }

        let mut splitter = ThinkingSplitter::new();
        let mut reader = BufReader::new(resp);
        let mut line = String::new();
        loop {
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            line.clear();
            let read = reader
                .read_line(&mut line)
                .map_err(|e| LlmError::Http(e.to_string()))?;
            if read == 0 {
                break; // stream closed
            }
            let Some(data) = sse_data(&line) else {
                continue;
            };
            if data == "[DONE]" {
                break;
            }
            let (content, reasoning) = parse_sse_data(data);
            if content.is_empty() && reasoning.is_empty() {
                continue;
            }
            let (resp_delta, reason_delta) = splitter.feed(&content, &reasoning);
            if !resp_delta.is_empty() || !reason_delta.is_empty() {
                on_delta(Delta {
                    response: resp_delta,
                    reasoning: reason_delta,
                });
            }
        }
        Ok(splitter.finish())
    }
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
}

#[derive(Debug, Default, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
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
}
