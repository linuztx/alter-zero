//! OpenAI-compatible streaming client.
//!
//! Talks to anything accepting the `/chat/completions` shape (OpenAI, OpenRouter,
//! Groq, Together, Sambanova, Ollama, …). The endpoint/payload builders and the
//! SSE frame parse are pure and unit-tested; [`OpenAiClient::stream_chat`] is the
//! boundary that opens the blocking request and drains it. See `docs/llm.md`.

use std::io::{BufReader, Read};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use super::config::ModelConfig;
use super::thinking::ThinkingSplitter;
use super::{ChatMessage, LlmError, Result};
use crate::stream::CancelToken;

/// The streaming client's per-operation timeout (see [`super::http_client`]):
/// each body read wakes after this so the SSE drain can poll the [`CancelToken`]
/// (a timed-out read is retried, never fatal), which also caps how long an
/// Esc-interrupt / quit waits to reap the backend thread. It bounds the initial
/// send/header exchange too, so it's kept comfortably above a normal
/// connect-plus-headers latency rather than as short as possible.
const STREAM_OP_TIMEOUT: Duration = Duration::from_secs(3);

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
        let client = super::http_client(STREAM_OP_TIMEOUT)?;
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

        // Drain the SSE body a byte at a time, accumulating a line across the
        // read-timeout wakeups. Reading manually (not `read_line`) is what makes
        // cancellation prompt *without* losing a partial line: on a timeout the
        // `line` buffer is preserved and we just loop back to poll `cancel`.
        let mut splitter = ThinkingSplitter::new();
        let mut reader = BufReader::new(resp);
        let mut line: Vec<u8> = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            match reader.read(&mut byte) {
                Ok(0) => {
                    // EOF: flush any final line that had no trailing newline.
                    if !line.is_empty()
                        && let SseStep::Fail(err) =
                            process_sse_line(&line, &mut splitter, &mut on_delta)
                    {
                        return Err(err);
                    }
                    break;
                }
                Ok(_) => match byte[0] {
                    b'\n' => {
                        let step = process_sse_line(&line, &mut splitter, &mut on_delta);
                        line.clear();
                        match step {
                            SseStep::Continue => {}
                            SseStep::Done => break, // saw `data: [DONE]`
                            // The provider failed the stream in-band; surface
                            // it instead of letting EOF report a clean finish.
                            SseStep::Fail(err) => return Err(err),
                        }
                    }
                    b'\r' => {} // SSE line ending — ignore the CR
                    b => line.push(b),
                },
                // A per-read timeout (the cancel-poll wake) isn't a failure — the
                // partial `line` is intact, so loop back and re-check `cancel`.
                Err(e) if is_read_timeout(&e) => continue,
                Err(e) => return Err(LlmError::Http(e.to_string())),
            }
        }
        // Surface any tail buffered mid-tag so an EOF inside a partial
        // `<think>`/`</think>` fragment doesn't drop it.
        let (resp_tail, reason_tail) = splitter.flush();
        if !resp_tail.is_empty() || !reason_tail.is_empty() {
            on_delta(Delta {
                response: resp_tail,
                reasoning: reason_tail,
            });
        }
        Ok(splitter.finish())
    }
}

/// Is this read error the streaming client's per-read timeout (the cancel-poll
/// wake), rather than a real transport failure? `reqwest` surfaces the timeout
/// as an `ErrorKind::Other` wrapping a `reqwest::Error` whose `is_timeout()` is
/// true.
fn is_read_timeout(e: &std::io::Error) -> bool {
    e.get_ref()
        .and_then(|inner| inner.downcast_ref::<reqwest::Error>())
        .is_some_and(reqwest::Error::is_timeout)
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
/// deltas, feed the rest through `splitter`, and emit a non-empty split via
/// `on_delta`. Invalid UTF-8 is skipped, never fatal.
fn process_sse_line(
    line: &[u8],
    splitter: &mut ThinkingSplitter,
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
    if let Some(err) = parse_sse_error(data) {
        return SseStep::Fail(err);
    }
    let (content, reasoning) = parse_sse_data(data);
    if content.is_empty() && reasoning.is_empty() {
        return SseStep::Continue;
    }
    let (resp_delta, reason_delta) = splitter.feed(&content, &reasoning);
    if !resp_delta.is_empty() || !reason_delta.is_empty() {
        on_delta(Delta {
            response: resp_delta,
            reasoning: reason_delta,
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

    /// Run a line's bytes through `process_sse_line`, collecting emitted deltas.
    fn drive_line(line: &str) -> (SseStep, Vec<Delta>) {
        let mut splitter = ThinkingSplitter::new();
        let mut out = Vec::new();
        let step = process_sse_line(line.as_bytes(), &mut splitter, &mut |d| out.push(d));
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
        let mut emitted = false;
        // A lone 0xFF byte after the prefix isn't valid UTF-8.
        let step = process_sse_line(
            &[b'd', b'a', b't', b'a', b':', 0xFF],
            &mut splitter,
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
}
