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
//! id lives in, and [`apply_cache_breakpoints`] marks the round's **copy** of
//! the [`crate::llm::ChatMessage`]s with the breakpoints — typed, on the
//! messages themselves, so the body is still serialized from them by
//! reference (a pasted picture's base64 is never copied into a tree of the
//! request; `docs/memory.md`) and an implicit-caching provider's messages
//! stay byte-identical. Pure and unit-tested; verified live against
//! OpenRouter's Anthropic routing (a breakpointed second request read its
//! whole prefix from cache at ~1/10th the input price).

use super::{CacheControl, ChatMessage, ContentPart, MessageContent};

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

/// Mark `messages` — the round's copy of the conversation, as the provider
/// receives it — with up to three `cache_control: {"type":"ephemeral"}`
/// breakpoints:
///
/// 1. the **system message** — the big stable prefix (persona + environment)
///    shared by every request;
/// 2. the **last cacheable message** — a moving breakpoint that tracks the
///    conversation frontier, so each agentic round caches everything so far
///    and the next round (or turn) reads it back;
/// 3. the last **user** message *before* that — insurance for the provider's
///    bounded lookback when many blocks land between two requests (a big
///    parallel tool batch).
///
/// A plain-string content converts to the one-text-part array form (the only
/// shape that can carry `cache_control`); a parts array gets the marker on its
/// last non-empty text part (image parts can't carry one, and their shared
/// bytes are never copied). Messages with empty content are skipped — an
/// empty marked text block would be rejected.
pub fn apply_cache_breakpoints(messages: &mut [ChatMessage]) {
    let mut targets: Vec<usize> = Vec::new();
    if let Some(system) = messages
        .iter()
        .position(|m| m.role == "system" && markable(m))
    {
        targets.push(system);
    }
    let last = messages.iter().rposition(markable);
    if let Some(last) = last {
        if !targets.contains(&last) {
            targets.push(last);
        }
        if let Some(prev_user) = messages[..last]
            .iter()
            .rposition(|m| m.role == "user" && markable(m))
            && !targets.contains(&prev_user)
        {
            targets.push(prev_user);
        }
    }
    debug_assert!(targets.len() < MAX_BREAKPOINTS);
    for index in targets {
        mark(&mut messages[index]);
    }
}

/// Can this message carry a breakpoint? — a non-empty string content, or a
/// parts array with at least one non-empty text part.
fn markable(message: &ChatMessage) -> bool {
    match &message.content {
        MessageContent::Text(text) => !text.trim().is_empty(),
        MessageContent::Parts(parts) => parts.iter().any(is_markable_text_part),
    }
}

/// Is this content part a text part whose text is non-empty?
fn is_markable_text_part(part: &ContentPart) -> bool {
    matches!(part, ContentPart::Text { text, .. } if !text.trim().is_empty())
}

/// Set the breakpoint on one (already [`markable`]) message: a string content
/// is wrapped into the one-text-part array carrying `cache_control`; a parts
/// array gets it on the **last** non-empty text part.
fn mark(message: &mut ChatMessage) {
    match &mut message.content {
        MessageContent::Text(text) => {
            let text = std::mem::take(text);
            message.content = MessageContent::Parts(vec![ContentPart::Text {
                text,
                cache_control: Some(CacheControl::EPHEMERAL),
            }]);
        }
        MessageContent::Parts(parts) => {
            if let Some(ContentPart::Text { cache_control, .. }) =
                parts.iter_mut().rev().find(|p| is_markable_text_part(p))
            {
                *cache_control = Some(CacheControl::EPHEMERAL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::llm::{AttachmentUrl, ToolCallSpec};

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
    /// breakpoint, as the wire sees it.
    fn marked(text: &str) -> Value {
        json!([{"type": "text", "text": text, "cache_control": {"type": "ephemeral"}}])
    }

    /// A message's `content` as the wire carries it.
    fn content(message: &ChatMessage) -> Value {
        serde_json::to_value(&message.content).unwrap()
    }

    fn tool_call_message() -> ChatMessage {
        ChatMessage::assistant_tool_calls("", vec![ToolCallSpec::function("c1", "bash", "{}")])
    }

    #[test]
    fn the_system_prompt_and_the_last_message_get_breakpoints() {
        let mut messages = vec![
            ChatMessage::system("persona"),
            ChatMessage::user("hi"),
            ChatMessage::assistant("hello!"),
            ChatMessage::user("and again"),
        ];
        apply_cache_breakpoints(&mut messages);
        assert_eq!(
            content(&messages[0]),
            marked("persona"),
            "the system prefix caches"
        );
        assert_eq!(
            content(&messages[3]),
            marked("and again"),
            "the frontier caches"
        );
        assert_eq!(
            content(&messages[2]),
            json!("hello!"),
            "in-between messages stay plain strings"
        );
    }

    #[test]
    fn the_previous_user_message_gets_the_third_breakpoint() {
        let mut messages = vec![
            ChatMessage::system("persona"),
            ChatMessage::user("first"),
            ChatMessage::assistant("one"),
            ChatMessage::user("second"),
        ];
        apply_cache_breakpoints(&mut messages);
        assert_eq!(
            content(&messages[1]),
            marked("first"),
            "the previous user message caches"
        );
        assert_eq!(content(&messages[3]), marked("second"));
    }

    #[test]
    fn at_most_three_breakpoints_are_placed_on_a_long_conversation() {
        let mut messages = vec![
            ChatMessage::system("persona"),
            ChatMessage::user("a"),
            ChatMessage::assistant("b"),
            ChatMessage::user("c"),
            ChatMessage::assistant("d"),
            ChatMessage::user("e"),
            ChatMessage::assistant("f"),
            ChatMessage::user("g"),
        ];
        apply_cache_breakpoints(&mut messages);
        let marks = messages
            .iter()
            .filter(|m| matches!(m.content, MessageContent::Parts(_)))
            .count();
        assert_eq!(marks, 3, "system + frontier + previous user, nothing else");
        assert!(
            matches!(messages[1].content, MessageContent::Text(_)),
            "older messages untouched"
        );
    }

    #[test]
    fn a_tool_result_frontier_is_marked_in_place() {
        // Mid-agentic-round the request ends on a tool result; the moving
        // breakpoint lands on it so the round's results cache too (verified
        // accepted by OpenRouter's Anthropic routing).
        let mut messages = vec![
            ChatMessage::system("persona"),
            ChatMessage::user("run it"),
            tool_call_message(),
            ChatMessage::tool_result("c1", "Exit code: 0"),
        ];
        apply_cache_breakpoints(&mut messages);
        assert_eq!(content(&messages[3]), marked("Exit code: 0"));
        assert_eq!(
            content(&messages[1]),
            marked("run it"),
            "previous user message still caches"
        );
        assert_eq!(
            content(&messages[2]),
            json!(""),
            "the empty assistant tool-call content is never marked (the provider rejects empty text blocks)"
        );
    }

    #[test]
    fn an_empty_frontier_falls_back_to_the_previous_cacheable_message() {
        // A trailing assistant message with empty content (a pure tool-call
        // request) can't carry a breakpoint — the frontier walks back.
        let mut messages = vec![ChatMessage::user("hi"), tool_call_message()];
        apply_cache_breakpoints(&mut messages);
        assert_eq!(content(&messages[0]), marked("hi"));
        assert_eq!(content(&messages[1]), json!(""));
    }

    #[test]
    fn a_parts_message_gets_the_marker_on_its_last_text_part_only() {
        // A vision message: the image part can't carry cache_control — the
        // marker rides the last non-empty text part, parts order preserved.
        let mut messages = vec![ChatMessage::with_parts(
            "user",
            vec![
                ContentPart::text("look at this"),
                ContentPart::image("data:image/png;base64,AA"),
            ],
        )];
        apply_cache_breakpoints(&mut messages);
        assert_eq!(
            content(&messages[0]),
            json!([
                {"type": "text", "text": "look at this", "cache_control": {"type": "ephemeral"}},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA"}},
            ])
        );
    }

    #[test]
    fn a_message_with_only_image_parts_is_not_markable() {
        let mut messages = vec![
            ChatMessage::user("hi"),
            ChatMessage::with_parts("user", vec![ContentPart::image("data:image/png;base64,AA")]),
        ];
        apply_cache_breakpoints(&mut messages);
        assert_eq!(
            content(&messages[0]),
            marked("hi"),
            "the frontier walked back past the image-only message"
        );
        assert!(
            content(&messages[1]).as_array().unwrap()[0]
                .get("cache_control")
                .is_none()
        );
    }

    #[test]
    fn a_lone_system_message_is_marked_once_not_twice() {
        // system == frontier: the two rules dedupe to one breakpoint.
        let mut messages = vec![ChatMessage::system("persona")];
        apply_cache_breakpoints(&mut messages);
        assert_eq!(content(&messages[0]), marked("persona"));
    }

    #[test]
    fn whitespace_only_content_is_skipped() {
        let mut messages = vec![
            ChatMessage::system("   "),
            ChatMessage::with_parts("user", vec![ContentPart::text(" ")]),
            ChatMessage::user("real"),
        ];
        apply_cache_breakpoints(&mut messages);
        assert_eq!(
            content(&messages[0]),
            json!("   "),
            "a blank system prompt is not marked"
        );
        assert!(
            content(&messages[1])[0].get("cache_control").is_none(),
            "a blank text part is not marked either"
        );
        assert_eq!(content(&messages[2]), marked("real"));
    }

    #[test]
    fn marking_shares_the_image_bytes_rather_than_copying_them() {
        // The breakpoints are applied to the round's copy of the messages;
        // that copy, marked or not, must never duplicate an attachment.
        let original = ChatMessage::with_parts(
            "user",
            vec![
                ContentPart::text("look"),
                ContentPart::image("data:image/png;base64,AA"),
            ],
        );
        let mut messages = vec![original.clone()];
        apply_cache_breakpoints(&mut messages);
        let url_of = |m: &ChatMessage| match &m.content {
            MessageContent::Parts(parts) => match &parts[1] {
                ContentPart::ImageUrl { image_url } => image_url.url.clone(),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        };
        assert!(AttachmentUrl::ptr_eq(
            &url_of(&original),
            &url_of(&messages[0])
        ));
    }
}
