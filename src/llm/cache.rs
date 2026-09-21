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
/// documents as explicit-caching: `anthropic/…` and `qwen/…` — and their
/// `~vendor/model-latest` **alias** ids, which resolve to the same routing
/// (`~anthropic/claude-haiku-latest` answers as `anthropic/claude-haiku-4.5`
/// on Amazon Bedrock, verified live; without the markers that routing
/// caches nothing, so an alias the sniff missed paid full price on every
/// agentic round). Everything else — OpenAI/DeepSeek/Gemini-2.5 ids (implicit
/// caching) and Venice's bare `claude-*` ids (Venice adds the markers itself,
/// and doubling them could blow the 4-breakpoint limit) — is left untouched.
#[must_use]
pub fn needs_cache_breakpoints(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    let vendor_id = model.strip_prefix('~').unwrap_or(&model);
    vendor_id.starts_with("anthropic/") || vendor_id.starts_with("qwen/")
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
/// 3. the **previous request's frontier** — the last cacheable message before
///    the newest assistant response. This can be a tool result, not just a
///    user message: retaining that boundary lets a provider find the previous
///    write even when the new suffix exceeds its bounded lookback.
///
/// A plain-string content converts to the one-text-part array form (the only
/// shape that can carry `cache_control`); a parts array gets the marker on its
/// last non-empty text part (image parts can't carry one, and their shared
/// bytes are never copied). Messages with empty content are skipped — an
/// empty marked text block would be rejected.
/// Existing markers are replaced, so reusing an already prepared copy never
/// accumulates breakpoints beyond the provider's limit.
pub fn apply_cache_breakpoints(messages: &mut [ChatMessage]) {
    for message in messages.iter_mut() {
        if let MessageContent::Parts(parts) = &mut message.content {
            for part in parts {
                if let ContentPart::Text { cache_control, .. } = part {
                    *cache_control = None;
                }
            }
        }
    }
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
        let previous = messages
            .iter()
            .rposition(|m| m.role == "assistant")
            .and_then(|assistant| messages[..assistant].iter().rposition(markable))
            .or_else(|| {
                messages[..last]
                    .iter()
                    .rposition(|m| m.role == "user" && markable(m))
            });
        if let Some(previous) = previous
            && !targets.contains(&previous)
        {
            targets.push(previous);
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
    fn openrouters_latest_alias_ids_are_the_same_explicit_caching_models() {
        // OpenRouter's `~vendor/model-latest` aliases resolve to the vendor's
        // newest model on the same routing (`~anthropic/claude-haiku-latest`
        // answered as `anthropic/claude-haiku-4.5` on Amazon Bedrock,
        // verified live). Anthropic's routing caches nothing without the
        // markers, so an alias the sniff missed paid full price every round.
        assert!(needs_cache_breakpoints("~anthropic/claude-haiku-latest"));
        assert!(needs_cache_breakpoints("~anthropic/claude-sonnet-latest"));
        assert!(needs_cache_breakpoints("~Anthropic/Claude-Opus-Latest"));
        // The `~` alone is not a vendor: an implicit-caching vendor's alias
        // stays untouched, exactly like its plain id.
        assert!(!needs_cache_breakpoints("~openai/gpt-mini-latest"));
        assert!(!needs_cache_breakpoints(
            "~deepseek/deepseek-v4-flash-latest"
        ));
        assert!(!needs_cache_breakpoints("~"));
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

    fn breakpoint_positions(messages: &[ChatMessage]) -> Vec<usize> {
        messages
            .iter()
            .enumerate()
            .flat_map(|(index, message)| match &message.content {
                MessageContent::Text(_) => Vec::new(),
                MessageContent::Parts(parts) => parts
                    .iter()
                    .filter_map(|part| {
                        matches!(
                            part,
                            ContentPart::Text {
                                cache_control: Some(_),
                                ..
                            }
                        )
                        .then_some(index)
                    })
                    .collect(),
            })
            .collect()
    }

    /// A deterministic prefix-cache oracle: content blocks are exact cache
    /// keys, and only marked prefixes are written. A subsequent breakpoint
    /// can read a previous write at itself or its 19 preceding positions.
    /// Token thresholds, TTLs and routing are deliberately outside this
    /// request-shape test; each tool block counts separately (the stricter
    /// case, without the Claude API's tool-run position collapsing).
    fn cache_blocks(messages: &[ChatMessage]) -> (Vec<Value>, Vec<usize>) {
        let mut blocks = Vec::new();
        let mut writes = Vec::new();
        for message in messages {
            let parts = match &message.content {
                MessageContent::Text(text) => vec![ContentPart::text(text)],
                MessageContent::Parts(parts) => parts.clone(),
            };
            for part in parts {
                let mut value = serde_json::to_value(part).unwrap();
                let marked = value
                    .as_object_mut()
                    .unwrap()
                    .remove("cache_control")
                    .is_some();
                blocks.push(json!([message.role, message.tool_call_id, value]));
                if marked {
                    writes.push(blocks.len());
                }
            }
            for call in &message.tool_calls {
                blocks.push(json!([message.role, call]));
            }
        }
        (blocks, writes)
    }

    fn longest_cached_prefix(previous: &[ChatMessage], current: &[ChatMessage]) -> usize {
        let (written, writes) = cache_blocks(previous);
        let (requested, lookups) = cache_blocks(current);
        writes
            .into_iter()
            .filter(|&end| {
                lookups
                    .iter()
                    .any(|&lookup| lookup >= end && lookup - end < 20)
                    && requested.get(..end) == Some(&written[..end])
            })
            .max()
            .unwrap_or_default()
    }

    #[test]
    fn long_tool_rounds_retain_the_previous_request_frontier() {
        // Model a provider with a bounded 20-position prefix lookup. The
        // original user-only anchor missed every intermediate tool round
        // once the next batch put that round's write outside the window.
        // Keep the exact previous frontier explicitly marked regardless of
        // batch size; this also works on providers that collapse tool runs
        // into one lookup position.
        for batch_size in [1, 19, 20, 21, 64] {
            let mut history = vec![ChatMessage::system("persona"), ChatMessage::user("run it")];
            for round in 0..32 {
                let previous_frontier = history.len() - 1;
                let mut prior_request = history.clone();
                apply_cache_breakpoints(&mut prior_request);
                assert!(breakpoint_positions(&prior_request).contains(&previous_frontier));
                let expected_read = cache_blocks(&prior_request).0.len();

                let calls: Vec<_> = (0..batch_size)
                    .map(|call| ToolCallSpec::function(format!("r{round}-c{call}"), "bash", "{}"))
                    .collect();
                history.push(ChatMessage::assistant_tool_calls("checking", calls.clone()));
                for call in calls {
                    history.push(ChatMessage::tool_result(
                        &call.id,
                        format!("result {}", call.id),
                    ));
                }
                let mut request = history.clone();
                apply_cache_breakpoints(&mut request);
                let positions = breakpoint_positions(&request);
                assert_eq!(
                    positions,
                    [0, previous_frontier, history.len() - 1],
                    "round {round}, batch size {batch_size}: the previous write must remain reachable"
                );
                assert_eq!(
                    content(&prior_request[previous_frontier]),
                    content(&request[previous_frontier]),
                    "the anchor still ends at the same text"
                );
                assert_eq!(
                    longest_cached_prefix(&prior_request, &request),
                    expected_read
                );
            }
        }
    }

    #[test]
    fn a_large_followup_after_tools_keeps_the_last_tool_result_anchor() {
        let mut messages = vec![
            ChatMessage::system("persona"),
            ChatMessage::user("run it"),
            tool_call_message(),
            ChatMessage::tool_result("c1", "large result"),
            ChatMessage::assistant("done"),
            ChatMessage::with_parts(
                "user",
                (0..64)
                    .map(|index| ContentPart::text(format!("followup part {index}")))
                    .collect(),
            ),
        ];
        let mut previous = messages[..4].to_vec();
        apply_cache_breakpoints(&mut previous);
        let expected_read = cache_blocks(&previous).0.len();
        apply_cache_breakpoints(&mut messages);
        // Unlike consecutive tool blocks on the Claude API, these text
        // blocks each consume a lookback position. The frontier's lookup
        // cannot reach the previous tool result without its own marker.
        assert_eq!(breakpoint_positions(&messages), [0, 3, 5]);
        assert_eq!(longest_cached_prefix(&previous, &messages), expected_read);
    }

    #[test]
    fn repeated_preparation_replaces_stale_and_invalid_markers() {
        let mut messages = vec![ChatMessage::system("persona"), ChatMessage::user("first")];
        for round in 0..64 {
            apply_cache_breakpoints(&mut messages);
            let prepared = messages.clone();
            apply_cache_breakpoints(&mut messages);
            assert_eq!(messages, prepared, "retries must be idempotent");
            assert!(breakpoint_positions(&messages).len() <= 3);
            messages.push(ChatMessage::assistant(format!("answer {round}")));
            messages.push(ChatMessage::user(format!("question {round}")));
        }
        // A stale mark on an empty text part must not survive merely
        // because the part is ineligible for placement this time.
        messages.push(ChatMessage::with_parts(
            "user",
            vec![ContentPart::Text {
                text: " ".to_string(),
                cache_control: Some(CacheControl::EPHEMERAL),
            }],
        ));
        apply_cache_breakpoints(&mut messages);
        assert!(breakpoint_positions(&messages).len() <= 3);
        assert!(
            content(messages.last().unwrap())[0]
                .get("cache_control")
                .is_none()
        );
        assert!(content(&messages[1])[0].get("cache_control").is_none());
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
