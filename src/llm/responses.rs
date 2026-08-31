//! OpenAI's **Responses** API as a wire format — the one the ChatGPT backend
//! speaks, and the only one it speaks. See `docs/chatgpt.md`.
//!
//! Everything here is a **translation**, in both directions, between that
//! format and the Chat Completions currency the rest of the crate already
//! uses ([`ChatMessage`] in, [`Delta`](super::openai::Delta) /
//! [`StreamOutcome`](super::openai::StreamOutcome) out). Nothing above
//! [`super::openai::OpenAiClient`] learns that a second wire format exists:
//! the agent loop, the transcript, the rollout and the derived context are
//! untouched.
//!
//! Three shapes differ, and each is a place a naive port breaks:
//!
//! - **`messages` becomes `input`**, an array of *typed items* rather than
//!   role/content pairs — and a tool call is an item of its own, not a field
//!   on an assistant message.
//! - **The system prompt is `instructions`**, a top-level string. It is
//!   required, so a request with no system message still carries the field.
//! - **Content parts are `input_text`/`input_image`**, not `text`/`image_url`.
//!
//! Pure and unit-tested — the HTTP lives in [`super::openai`].

use serde_json::{Value, json};

use super::config::ModelConfig;
use super::tools::ToolCallRequest;
use super::{ChatMessage, ContentPart, MessageContent};
use crate::stream::TokenUsage;

/// Build the streamed request body for `messages`.
///
/// Three fields are **mandatory** on this backend rather than merely
/// conventional, and omitting any of them is a 400 that does not say which:
/// `instructions` (present even when empty), `store: false` (the ChatGPT
/// backend keeps nothing), and `stream: true`.
#[must_use]
pub fn build_payload(cfg: &ModelConfig, tools: &[Value], messages: &[ChatMessage]) -> Value {
    let (instructions, input) = build_input(messages);
    let mut payload = json!({
        "model": cfg.model,
        "instructions": instructions,
        "input": input,
        "store": false,
        "stream": true,
    });
    if !tools.is_empty() {
        payload["tools"] = json!(responses_tools(tools));
        payload["tool_choice"] = json!("auto");
        payload["parallel_tool_calls"] = json!(true);
    }
    if let Some(key) = cfg.cache_key.as_deref().filter(|k| !k.is_empty()) {
        payload["prompt_cache_key"] = json!(key);
    }
    // The Responses API spells reasoning as an object with an `effort`, the
    // same word Chat Completions uses — but `enabled: false` is not a thing
    // here, so `Off` sends *no* reasoning field rather than a disable the
    // server would reject.
    if let Some(mode) = cfg.thinking
        && let Some(effort) = super::reasoning::effort_label(mode)
    {
        payload["reasoning"] = json!({"effort": effort, "summary": "auto"});
    }
    if let Some(obj) = payload.as_object_mut() {
        for (k, v) in &cfg.extra_body {
            obj.insert(k.clone(), v.clone());
        }
    }
    payload
}

/// Split `messages` into the top-level `instructions` and the `input` array.
///
/// System messages are **hoisted** (joined by a blank line, in order): the
/// Responses API has no system role in `input`, and a request without
/// `instructions` is refused.
#[must_use]
pub fn build_input(messages: &[ChatMessage]) -> (String, Vec<Value>) {
    let mut instructions: Vec<&str> = Vec::new();
    let mut input: Vec<Value> = Vec::new();
    for message in messages {
        match message.role.as_str() {
            "system" | "developer" => {
                if let MessageContent::Text(text) = &message.content
                    && !text.is_empty()
                {
                    instructions.push(text);
                }
            }
            "tool" => {
                // A tool result is its own item, linked to the call by
                // `call_id` — there is no `role: "tool"` message here.
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": message.tool_call_id.clone().unwrap_or_default(),
                    "output": flatten_text(&message.content),
                }));
            }
            "assistant" => {
                if let Some(content) = assistant_content(&message.content) {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": content,
                    }));
                }
                // The calls the model made ride *after* the text it wrote,
                // which is the order they happened in.
                for call in &message.tool_calls {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.function.name,
                        "arguments": call.function.arguments,
                    }));
                }
            }
            role => input.push(json!({
                "type": "message",
                "role": role,
                "content": user_content(&message.content),
            })),
        }
    }
    (instructions.join("\n\n"), input)
}

/// A user-side message's content parts. Text is `input_text` and an image is
/// `input_image` carrying the `data:` URL directly — **not** the nested
/// `image_url: {url}` object Chat Completions uses.
fn user_content(content: &MessageContent) -> Vec<Value> {
    match content {
        MessageContent::Text(text) => vec![json!({"type": "input_text", "text": text})],
        MessageContent::Parts(parts) => parts
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => json!({"type": "input_text", "text": text}),
                ContentPart::ImageUrl { image_url } => {
                    json!({"type": "input_image", "image_url": image_url.url})
                }
            })
            .collect(),
    }
}

/// An assistant message's content, or `None` when it said nothing (a round
/// that was only tool calls) — an empty `content` array is a 400 here, where
/// Chat Completions tolerates `""`.
fn assistant_content(content: &MessageContent) -> Option<Vec<Value>> {
    let text = flatten_text(content);
    (!text.is_empty()).then(|| vec![json!({"type": "output_text", "text": text})])
}

/// A message's text, with any image parts dropped — what a `function_call_output`
/// and an assistant echo need.
fn flatten_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(text) => text.clone(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                ContentPart::ImageUrl { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Flatten Chat Completions tool specs into the Responses shape: the
/// `function` object's fields are lifted to the top level beside
/// `type: "function"`, where Chat Completions nests them.
#[must_use]
pub fn responses_tools(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|spec| {
            let function = spec.get("function")?;
            let mut out = json!({
                "type": "function",
                "name": function.get("name")?.clone(),
                "parameters": function.get("parameters").cloned().unwrap_or(json!({})),
                // Every schema here has optional fields, so strict mode (which
                // demands each property be required) would refuse them all.
                "strict": false,
            });
            if let Some(description) = function.get("description") {
                out["description"] = description.clone();
            }
            Some(out)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The event stream
// ---------------------------------------------------------------------------

/// What one `data:` frame of a Responses stream means. The vocabulary is
/// event-typed rather than delta-shaped: the frame names what happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseEvent {
    /// Reply text to show.
    Text(String),
    /// Reasoning summary text (the model's own summary of its thinking).
    Reasoning(String),
    /// A fragment of a tool call being *generated* — never rendered, only
    /// counted, so the tally ticks while the call is produced.
    ToolFragment(String),
    /// A tool call the model finished asking for.
    ToolCall(ToolCallRequest),
    /// The stream finished; the round's usage, when the frame carried one.
    Completed {
        usage: Option<TokenUsage>,
        status: Option<String>,
    },
    /// The server failed the response in-band.
    Failed(String),
    /// A frame this client has no use for (there are many).
    Ignored,
}

/// Classify one `data:` payload.
#[must_use]
pub fn parse_event(data: &str) -> ResponseEvent {
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return ResponseEvent::Ignored;
    };
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let delta = || {
        value
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match kind {
        "response.output_text.delta" => ResponseEvent::Text(delta()),
        // Both spellings occur: models that stream a *summary* of their
        // reasoning, and models that stream the reasoning text itself.
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            ResponseEvent::Reasoning(delta())
        }
        "response.function_call_arguments.delta" => ResponseEvent::ToolFragment(delta()),
        "response.output_item.done" => value
            .get("item")
            .and_then(function_call_of)
            .map_or(ResponseEvent::Ignored, ResponseEvent::ToolCall),
        "response.completed" | "response.incomplete" => ResponseEvent::Completed {
            usage: value.get("response").and_then(parse_usage),
            status: value
                .get("response")
                .and_then(|r| r.get("status"))
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        "response.failed" | "error" => ResponseEvent::Failed(failure_message(&value, data)),
        _ => ResponseEvent::Ignored,
    }
}

/// One completed `function_call` output item as a [`ToolCallRequest`], or
/// `None` for any other item type (a finished message, a reasoning block).
fn function_call_of(item: &Value) -> Option<ToolCallRequest> {
    if item.get("type").and_then(Value::as_str)? != "function_call" {
        return None;
    }
    let name = item.get("name").and_then(Value::as_str)?.to_string();
    Some(ToolCallRequest {
        // `call_id` is the key a `function_call_output` links back with; the
        // item's own `id` is a different (and, with `store: false`,
        // uninteresting) thing.
        id: item
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        name,
        arguments: item
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}")
            .to_string(),
    })
}

/// The message out of a failure frame, however it is nested. Falls back to the
/// whole payload rather than reporting an empty reason.
fn failure_message(value: &Value, data: &str) -> String {
    let error = value
        .get("response")
        .and_then(|r| r.get("error"))
        .or_else(|| value.get("error"));
    error
        .and_then(|e| {
            e.get("message")
                .and_then(Value::as_str)
                .or_else(|| e.as_str())
        })
        .filter(|m| !m.trim().is_empty())
        .map_or_else(|| data.to_string(), str::to_string)
}

/// The round's token usage out of a completed response. The Responses API
/// names its counts differently from Chat Completions —
/// `input_tokens`/`output_tokens` rather than `prompt_tokens`/
/// `completion_tokens` — with the cache and reasoning detail nested the same
/// way.
fn parse_usage(response: &Value) -> Option<TokenUsage> {
    let usage = response.get("usage")?;
    let count = |value: Option<&Value>| value.and_then(Value::as_u64).unwrap_or(0);
    Some(TokenUsage {
        input: count(usage.get("input_tokens")),
        output: count(usage.get("output_tokens")),
        cached: count(
            usage
                .get("input_tokens_details")
                .and_then(|d| d.get("cached_tokens")),
        ),
        // Reported beside `cached_tokens` on this API, where chat completions
        // leaves it to Anthropic-style aliases.
        cache_write: count(
            usage
                .get("input_tokens_details")
                .and_then(|d| d.get("cache_write_tokens")),
        ),
        reasoning: count(
            usage
                .get("output_tokens_details")
                .and_then(|d| d.get("reasoning_tokens")),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ImageUrl, ToolCallSpec};

    fn user(text: &str) -> ChatMessage {
        ChatMessage::user(text)
    }

    // --- the request body ---

    #[test]
    fn the_payload_carries_the_three_fields_this_backend_refuses_to_do_without() {
        let cfg = ModelConfig::fallback();
        let payload = build_payload(&cfg, &[], &[user("hi")]);
        // Each of these is a 400 that does not say which one is missing.
        assert_eq!(payload["store"], json!(false));
        assert_eq!(payload["stream"], json!(true));
        assert!(payload.get("instructions").is_some(), "always present");
        // And none of Chat Completions' own field names survive.
        assert!(payload.get("messages").is_none());
        assert!(payload.get("stream_options").is_none());
    }

    #[test]
    fn the_system_prompt_is_hoisted_into_instructions() {
        let messages = vec![
            ChatMessage::system("be helpful"),
            user("hi"),
            ChatMessage::system("also be brief"),
        ];
        let (instructions, input) = build_input(&messages);
        assert_eq!(instructions, "be helpful\n\nalso be brief");
        assert_eq!(input.len(), 1, "no system item survives in the input array");
        assert_eq!(input[0]["role"], "user");
    }

    #[test]
    fn user_text_is_an_input_text_part() {
        // `text` (the Chat Completions spelling) is refused here.
        let (_, input) = build_input(&[user("hello")]);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[0]["content"][0]["text"], "hello");
    }

    #[test]
    fn an_image_part_becomes_a_flat_input_image_url() {
        // Chat Completions nests `image_url: {url}`; here the URL is the value.
        let message = ChatMessage {
            role: "user".to_string(),
            content: MessageContent::Parts(vec![
                ContentPart::text("look"),
                ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "data:image/png;base64,AAA".to_string(),
                    },
                },
            ]),
            tool_calls: Vec::new(),
            tool_call_id: None,
        };
        let (_, input) = build_input(&[message]);
        let parts = input[0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "input_text");
        assert_eq!(parts[1]["type"], "input_image");
        assert_eq!(parts[1]["image_url"], "data:image/png;base64,AAA");
    }

    #[test]
    fn a_tool_round_becomes_a_function_call_item_and_its_output_item() {
        let assistant = ChatMessage {
            role: "assistant".to_string(),
            content: MessageContent::Text("running it".to_string()),
            tool_calls: vec![ToolCallSpec::function(
                "call_1",
                "bash",
                r#"{"command":"ls"}"#,
            )],
            tool_call_id: None,
        };
        let result = ChatMessage {
            role: "tool".to_string(),
            content: MessageContent::Text("a.txt".to_string()),
            tool_calls: Vec::new(),
            tool_call_id: Some("call_1".to_string()),
        };
        let (_, input) = build_input(&[user("ls"), assistant, result]);
        assert_eq!(input.len(), 4, "user, assistant text, the call, its output");
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "call_1");
        assert_eq!(input[2]["name"], "bash");
        assert_eq!(input[2]["arguments"], r#"{"command":"ls"}"#);
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "call_1");
        assert_eq!(input[3]["output"], "a.txt");
    }

    #[test]
    fn an_assistant_round_that_only_called_tools_carries_no_empty_message() {
        // An empty `content` array is a 400 here, where Chat Completions
        // tolerates `""`.
        let assistant = ChatMessage {
            role: "assistant".to_string(),
            content: MessageContent::Text(String::new()),
            tool_calls: vec![ToolCallSpec::function("c", "bash", "{}")],
            tool_call_id: None,
        };
        let (_, input) = build_input(&[assistant]);
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "function_call");
    }

    #[test]
    fn tool_specs_are_flattened_out_of_their_function_object() {
        let specs = crate::llm::tools::tool_specs();
        let flat = responses_tools(&specs);
        assert_eq!(flat.len(), specs.len());
        assert_eq!(flat[0]["type"], "function");
        assert_eq!(flat[0]["name"], "bash", "the name is top-level now");
        assert!(flat[0].get("function").is_none(), "and never nested");
        assert!(flat[0]["parameters"].is_object());
        assert_eq!(flat[0]["strict"], json!(false));
    }

    #[test]
    fn offering_tools_asks_for_parallel_calls_and_omitting_them_sends_neither_field() {
        let cfg = ModelConfig::fallback();
        let with = build_payload(&cfg, &crate::llm::tools::tool_specs(), &[user("hi")]);
        assert_eq!(with["tool_choice"], "auto");
        assert_eq!(with["parallel_tool_calls"], json!(true));
        let without = build_payload(&cfg, &[], &[user("hi")]);
        assert!(without.get("tools").is_none());
        assert!(without.get("tool_choice").is_none());
    }

    #[test]
    fn the_thinking_mode_rides_as_a_reasoning_effort() {
        use crate::llm::{ReasoningEffort, ThinkingMode};
        let mut cfg = ModelConfig::fallback();
        cfg.thinking = Some(ThinkingMode::Effort(ReasoningEffort::High));
        let payload = build_payload(&cfg, &[], &[user("hi")]);
        assert_eq!(payload["reasoning"]["effort"], "high");
        assert_eq!(payload["reasoning"]["summary"], "auto");
        // `Off` sends no reasoning field at all: this API has no
        // `enabled: false`, and inventing one is a 400.
        cfg.thinking = Some(ThinkingMode::Off);
        assert!(
            build_payload(&cfg, &[], &[user("hi")])
                .get("reasoning")
                .is_none()
        );
    }

    // --- the event stream ---

    #[test]
    fn a_text_delta_is_reply_text() {
        assert_eq!(
            parse_event(r#"{"type":"response.output_text.delta","delta":"Hi"}"#),
            ResponseEvent::Text("Hi".to_string())
        );
    }

    #[test]
    fn both_reasoning_deltas_are_reasoning() {
        for kind in [
            "response.reasoning_summary_text.delta",
            "response.reasoning_text.delta",
        ] {
            let frame = format!(r#"{{"type":"{kind}","delta":"why"}}"#);
            assert_eq!(
                parse_event(&frame),
                ResponseEvent::Reasoning("why".to_string()),
                "{kind}"
            );
        }
    }

    #[test]
    fn a_finished_output_item_yields_the_whole_tool_call() {
        let frame = r#"{"type":"response.output_item.done","item":{"type":"function_call",
            "id":"fc_1","call_id":"call_abc","name":"bash","arguments":"{\"command\":\"ls\"}"}}"#;
        let ResponseEvent::ToolCall(call) = parse_event(frame) else {
            panic!("expected a tool call");
        };
        // `call_id` links the result back, not the item's own `id`.
        assert_eq!(call.id, "call_abc");
        assert_eq!(call.name, "bash");
        assert_eq!(call.arguments, r#"{"command":"ls"}"#);
    }

    #[test]
    fn a_finished_message_item_is_not_a_tool_call() {
        let frame = r#"{"type":"response.output_item.done","item":{"type":"message","id":"m"}}"#;
        assert_eq!(parse_event(frame), ResponseEvent::Ignored);
    }

    #[test]
    fn argument_fragments_surface_for_counting_only() {
        assert_eq!(
            parse_event(r#"{"type":"response.function_call_arguments.delta","delta":"{\"c"}"#),
            ResponseEvent::ToolFragment("{\"c".to_string())
        );
    }

    #[test]
    fn a_completed_response_carries_the_rounds_usage() {
        // The shape a live `response.completed` actually carries.
        let frame = r#"{"type":"response.completed","response":{"status":"completed","usage":{
            "input_tokens":100,"input_tokens_details":{"cached_tokens":40,"cache_write_tokens":7},
            "output_tokens":20,"output_tokens_details":{"reasoning_tokens":12},
            "total_tokens":120}}}"#;
        let ResponseEvent::Completed { usage, status } = parse_event(frame) else {
            panic!("expected completion");
        };
        let usage = usage.expect("usage present");
        // The names differ from Chat Completions; the meaning does not.
        assert_eq!(usage.input, 100);
        assert_eq!(usage.output, 20);
        assert_eq!(usage.cached, 40);
        assert_eq!(usage.cache_write, 7);
        assert_eq!(usage.reasoning, 12);
        assert_eq!(status.as_deref(), Some("completed"));
    }

    #[test]
    fn a_completion_without_usage_still_ends_the_stream() {
        let ResponseEvent::Completed { usage, .. } =
            parse_event(r#"{"type":"response.completed","response":{}}"#)
        else {
            panic!("expected completion");
        };
        assert!(usage.is_none());
    }

    #[test]
    fn an_in_band_failure_surfaces_its_message() {
        for frame in [
            r#"{"type":"response.failed","response":{"error":{"message":"rate limited"}}}"#,
            r#"{"type":"error","error":{"message":"rate limited"}}"#,
        ] {
            assert_eq!(
                parse_event(frame),
                ResponseEvent::Failed("rate limited".to_string()),
                "{frame}"
            );
        }
    }

    #[test]
    fn a_failure_with_no_message_reports_the_payload_rather_than_nothing() {
        let frame = r#"{"type":"response.failed","response":{}}"#;
        assert_eq!(parse_event(frame), ResponseEvent::Failed(frame.to_string()));
    }

    #[test]
    fn the_many_frames_this_client_has_no_use_for_are_skipped() {
        for frame in [
            r#"{"type":"response.created","response":{}}"#,
            r#"{"type":"response.in_progress"}"#,
            r#"{"type":"response.output_item.added","item":{"type":"message"}}"#,
            r#"{"type":"response.content_part.added"}"#,
            "not json",
            "",
        ] {
            assert_eq!(parse_event(frame), ResponseEvent::Ignored, "{frame}");
        }
    }
}
