//! Provider prompt caching — the pure request-side transforms.
//!
//! Two OpenAI-compatible caching worlds exist (see `docs/prompt-caching.md`):
//! **implicit** providers (OpenAI, DeepSeek, Grok, Gemini 2.5 — and Venice,
//! which injects Anthropic's markers itself for the Claude models it serves)
//! cache repeated prompt prefixes automatically, while **explicit** ones
//! (Anthropic and Qwen when reached through an OpenRouter-style aggregator)
//! only cache the prefix up to a text block marked with a
//! `cache_control: {"type": "ephemeral"}` breakpoint.
//!
//! [`needs_cache_breakpoints`] tells the payload builder which world a model
//! id lives in, and [`apply_cache_breakpoints`] rewrites the wire `messages`
//! array with the breakpoints — on the serialized JSON, so the whole
//! [`crate::llm::ChatMessage`] family stays byte-identical for every other
//! provider. Pure and unit-tested; verified live against OpenRouter's
//! Anthropic routing (a breakpointed second request read its whole prefix
//! from cache at ~1/10th the input price).

use serde_json::{Value, json};

/// The most `cache_control` breakpoints one request may carry (Anthropic's
/// limit is 4). [`apply_cache_breakpoints`] places at most 3 by construction —
/// this is the guard rail a future placement tweak must stay under.
pub const MAX_BREAKPOINTS: usize = 4;

/// Does this model id need explicit `cache_control` breakpoints for its
/// prompts to cache?
///
/// True for the OpenRouter-style namespaced ids of the providers OpenRouter
/// documents as explicit-caching: `anthropic/…` and `qwen/…`. Everything else
/// — OpenAI/DeepSeek/Gemini-2.5 ids (implicit caching) and Venice's bare
/// `claude-*` ids (Venice adds the markers itself, and doubling them could
/// blow the 4-breakpoint limit) — is left untouched.
#[must_use]
pub fn needs_cache_breakpoints(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.starts_with("anthropic/") || model.starts_with("qwen/")
}

/// Mark the wire `messages` array (the JSON the provider receives) with up to
/// three `cache_control: {"type":"ephemeral"}` breakpoints:
///
/// 1. the **system message** — the big stable prefix (persona + environment +
///    tools note) shared by every request;
/// 2. the **last cacheable message** — a moving breakpoint that tracks the
///    conversation frontier, so each agentic round caches everything so far
///    and the next round (or turn) reads it back;
/// 3. the last **user** message *before* that — insurance for the provider's
///    bounded lookback when many blocks land between two requests (a big
///    parallel tool batch).
///
/// A plain-string content converts to the one-text-part array form (the only
/// shape that can carry `cache_control`); a parts array gets the marker on its
/// last non-empty text part (image parts can't carry one). Messages with
/// empty/absent content are skipped — an empty marked text block would be
/// rejected. Anything that isn't a message array is left untouched.
pub fn apply_cache_breakpoints(messages: &mut Value) {
    let Some(list) = messages.as_array_mut() else {
        return;
    };
    let mut targets: Vec<usize> = Vec::new();
    if let Some(system) = list
        .iter()
        .position(|m| role_of(m) == Some("system") && markable(m))
    {
        targets.push(system);
    }
    let last = list.iter().rposition(markable);
    if let Some(last) = last {
        if !targets.contains(&last) {
            targets.push(last);
        }
        if let Some(prev_user) = list[..last]
            .iter()
            .rposition(|m| role_of(m) == Some("user") && markable(m))
            && !targets.contains(&prev_user)
        {
            targets.push(prev_user);
        }
    }
    debug_assert!(targets.len() < MAX_BREAKPOINTS);
    for index in targets {
        mark(&mut list[index]);
    }
}

/// The message's `role` string, when present.
fn role_of(message: &Value) -> Option<&str> {
    message.get("role").and_then(Value::as_str)
}

/// Can this message carry a breakpoint? — a non-empty string content, or a
/// parts array with at least one non-empty text part.
fn markable(message: &Value) -> bool {
    match message.get("content") {
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(parts)) => parts.iter().any(is_markable_text_part),
        _ => false,
    }
}

/// Is this content part a text part whose text is non-empty?
fn is_markable_text_part(part: &Value) -> bool {
    part.get("type").and_then(Value::as_str) == Some("text")
        && part
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|t| !t.trim().is_empty())
}

/// Set the breakpoint on one (already [`markable`]) message: a string content
/// is wrapped into the one-text-part array carrying `cache_control`; a parts
/// array gets it on the **last** non-empty text part.
fn mark(message: &mut Value) {
    let Some(content) = message.get_mut("content") else {
        return;
    };
    match content {
        Value::String(text) => {
            *content = json!([{
                "type": "text",
                "text": std::mem::take(text),
                "cache_control": {"type": "ephemeral"},
            }]);
        }
        Value::Array(parts) => {
            if let Some(part) = parts.iter_mut().rev().find(|p| is_markable_text_part(p)) {
                part["cache_control"] = json!({"type": "ephemeral"});
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_caching_models_are_the_namespaced_anthropic_and_qwen_ids() {
        // OpenRouter routes these to providers that only cache marked blocks.
        assert!(needs_cache_breakpoints("anthropic/claude-haiku-4.5"));
        assert!(needs_cache_breakpoints("anthropic/claude-sonnet-4.6"));
        assert!(needs_cache_breakpoints("qwen/qwen3.6-flash"));
        assert!(
            needs_cache_breakpoints("Anthropic/Claude-Opus-4.8"),
            "id casing is normalized"
        );
    }

    #[test]
    fn implicit_caching_models_get_no_breakpoints() {
        // OpenAI/DeepSeek/Gemini-2.5 cache automatically; Venice's bare
        // claude-* ids get their markers from Venice itself — adding ours
        // could double past the 4-breakpoint limit.
        assert!(!needs_cache_breakpoints("openai/gpt-4o-mini"));
        assert!(!needs_cache_breakpoints("deepseek/deepseek-chat"));
        assert!(!needs_cache_breakpoints("google/gemini-2.5-flash"));
        assert!(
            !needs_cache_breakpoints("claude-opus-4-5"),
            "Venice's bare claude ids stay implicit"
        );
        assert!(
            !needs_cache_breakpoints("qwen3-6-27b"),
            "Venice's bare qwen ids too"
        );
        assert!(!needs_cache_breakpoints(""));
    }

    /// The marked form of `text` — the one-text-part array with the ephemeral
    /// breakpoint.
    fn marked(text: &str) -> Value {
        json!([{"type": "text", "text": text, "cache_control": {"type": "ephemeral"}}])
    }

    #[test]
    fn the_system_prompt_and_the_last_message_get_breakpoints() {
        let mut messages = json!([
            {"role": "system", "content": "persona"},
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "hello!"},
            {"role": "user", "content": "and again"},
        ]);
        apply_cache_breakpoints(&mut messages);
        assert_eq!(
            messages[0]["content"],
            marked("persona"),
            "the system prefix caches"
        );
        assert_eq!(
            messages[3]["content"],
            marked("and again"),
            "the frontier caches"
        );
        assert_eq!(
            messages[2]["content"],
            json!("hello!"),
            "in-between messages stay plain strings"
        );
    }

    #[test]
    fn the_previous_user_message_gets_the_third_breakpoint() {
        let mut messages = json!([
            {"role": "system", "content": "persona"},
            {"role": "user", "content": "first"},
            {"role": "assistant", "content": "one"},
            {"role": "user", "content": "second"},
        ]);
        apply_cache_breakpoints(&mut messages);
        assert_eq!(
            messages[1]["content"],
            marked("first"),
            "the previous user message caches"
        );
        assert_eq!(messages[3]["content"], marked("second"));
    }

    #[test]
    fn at_most_three_breakpoints_are_placed_on_a_long_conversation() {
        let mut messages = json!([
            {"role": "system", "content": "persona"},
            {"role": "user", "content": "a"},
            {"role": "assistant", "content": "b"},
            {"role": "user", "content": "c"},
            {"role": "assistant", "content": "d"},
            {"role": "user", "content": "e"},
            {"role": "assistant", "content": "f"},
            {"role": "user", "content": "g"},
        ]);
        apply_cache_breakpoints(&mut messages);
        let marks = messages
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["content"].is_array())
            .count();
        assert_eq!(marks, 3, "system + frontier + previous user, nothing else");
        assert!(
            messages[1]["content"].is_string(),
            "older messages untouched"
        );
    }

    #[test]
    fn a_tool_result_frontier_is_marked_in_place() {
        // Mid-agentic-round the request ends on a tool result; the moving
        // breakpoint lands on it so the round's results cache too (verified
        // accepted by OpenRouter's Anthropic routing).
        let mut messages = json!([
            {"role": "system", "content": "persona"},
            {"role": "user", "content": "run it"},
            {"role": "assistant", "content": "", "tool_calls": [{"id": "c1"}]},
            {"role": "tool", "tool_call_id": "c1", "content": "Exit code: 0"},
        ]);
        apply_cache_breakpoints(&mut messages);
        assert_eq!(messages[3]["content"], marked("Exit code: 0"));
        assert_eq!(
            messages[1]["content"],
            marked("run it"),
            "previous user message still caches"
        );
        assert_eq!(
            messages[2]["content"],
            json!(""),
            "the empty assistant tool-call content is never marked (the provider rejects empty text blocks)"
        );
    }

    #[test]
    fn an_empty_frontier_falls_back_to_the_previous_cacheable_message() {
        // A trailing assistant message with empty content (a pure tool-call
        // request) can't carry a breakpoint — the frontier walks back.
        let mut messages = json!([
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "", "tool_calls": [{"id": "c1"}]},
        ]);
        apply_cache_breakpoints(&mut messages);
        assert_eq!(messages[0]["content"], marked("hi"));
        assert_eq!(messages[1]["content"], json!(""));
    }

    #[test]
    fn a_parts_message_gets_the_marker_on_its_last_text_part_only() {
        // A vision message: the image part can't carry cache_control — the
        // marker rides the last non-empty text part, parts order preserved.
        let mut messages = json!([
            {"role": "user", "content": [
                {"type": "text", "text": "look at this"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA"}},
            ]},
        ]);
        apply_cache_breakpoints(&mut messages);
        assert_eq!(
            messages[0]["content"],
            json!([
                {"type": "text", "text": "look at this", "cache_control": {"type": "ephemeral"}},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA"}},
            ])
        );
    }

    #[test]
    fn a_message_with_only_image_parts_is_not_markable() {
        let mut messages = json!([
            {"role": "user", "content": "hi"},
            {"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA"}},
            ]},
        ]);
        apply_cache_breakpoints(&mut messages);
        assert_eq!(
            messages[0]["content"],
            marked("hi"),
            "the frontier walked back past the image-only message"
        );
        assert!(
            messages[1]["content"].as_array().unwrap()[0]
                .get("cache_control")
                .is_none()
        );
    }

    #[test]
    fn a_lone_system_message_is_marked_once_not_twice() {
        // system == frontier: the two rules dedupe to one breakpoint.
        let mut messages = json!([{"role": "system", "content": "persona"}]);
        apply_cache_breakpoints(&mut messages);
        assert_eq!(messages[0]["content"], marked("persona"));
    }

    #[test]
    fn whitespace_only_and_absent_content_are_skipped() {
        let mut messages = json!([
            {"role": "system", "content": "   "},
            {"role": "user"},
            {"role": "user", "content": "real"},
        ]);
        apply_cache_breakpoints(&mut messages);
        assert_eq!(
            messages[0]["content"],
            json!("   "),
            "a blank system prompt is not marked"
        );
        assert!(messages[1].get("content").is_none());
        assert_eq!(messages[2]["content"], marked("real"));
    }

    #[test]
    fn a_non_array_value_is_left_untouched() {
        let mut not_messages = json!("oops");
        apply_cache_breakpoints(&mut not_messages);
        assert_eq!(not_messages, json!("oops"));
    }
}
