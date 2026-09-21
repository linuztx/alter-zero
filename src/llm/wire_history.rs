//! Keep an active conversation's already-sent prefix byte-for-byte stable.
//!
//! The display history reconstructs generated tool IDs and splits parallel
//! calls into individual assistant/result pairs. Those are equivalent tool
//! conversations, but changing the wire shape drops prompt-cache hits at the
//! next human turn. Reuse the last request only when the reconstructed prefix
//! has exactly the same content and ordered calls/results. A mismatch always
//! keeps the newly derived history (including edits, rewinds and compaction).

use super::{ChatMessage, MessageContent};
use std::borrow::Cow;

/// One request, never a growing collection of past requests. Text is capped;
/// image encodings remain shared through `AttachmentUrl`.
const MAX_RETAINED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Default)]
pub(super) struct WireHistory {
    messages: Vec<ChatMessage>,
}

impl WireHistory {
    pub(super) fn restore(&mut self, messages: &mut Vec<ChatMessage>) {
        let Some(prefix_len) = matching_prefix(&self.messages, messages) else {
            self.messages.clear();
            return;
        };
        // Move the retained prefix, avoiding a third copy of the transcript.
        let mut restored = std::mem::take(&mut self.messages);
        restored.extend(messages.drain(prefix_len..));
        *messages = restored;
    }

    pub(super) fn remember(&mut self, messages: &[ChatMessage]) {
        self.messages = if retained_bytes(messages) <= MAX_RETAINED_BYTES {
            messages.to_vec()
        } else {
            Vec::new()
        };
    }
}

fn retained_bytes(messages: &[ChatMessage]) -> usize {
    fn content_bytes(content: &MessageContent) -> usize {
        match content {
            MessageContent::Text(text) => text.len(),
            MessageContent::Parts(parts) => parts.iter().fold(0usize, |size, part| {
                size.saturating_add(std::mem::size_of_val(part))
                    .saturating_add(match part {
                        super::ContentPart::Text { text, .. } => text.len(),
                        // Account for retained image handles and their payloads
                        // too: sharing does not make a retained allocation free.
                        super::ContentPart::ImageUrl { image_url } => image_url.url.len(),
                    })
            }),
        }
    }
    messages.iter().fold(0usize, |size, message| {
        let size = size
            .saturating_add(std::mem::size_of::<ChatMessage>())
            .saturating_add(message.role.len())
            .saturating_add(content_bytes(&message.content))
            .saturating_add(message.tool_call_id.as_ref().map_or(0, String::len));
        message.tool_calls.iter().fold(size, |size, call| {
            size.saturating_add(std::mem::size_of_val(call))
                .saturating_add(call.id.len())
                .saturating_add(call.kind.len())
                .saturating_add(call.function.name.len())
                .saturating_add(call.function.arguments.len())
        })
    })
}

/// IDs, batch envelopes, adjacent plain-message boundaries and a
/// whitespace-only lead before a batch's calls may differ. Argument JSON is
/// deliberately compared verbatim: edits, hook rewrites and lossy older
/// records must not silently resurrect an earlier request.
#[derive(Debug, PartialEq, Eq)]
enum Atom<'a> {
    Message(&'a str, Cow<'a, MessageContent>),
    Call {
        kind: &'a str,
        name: &'a str,
        arguments: &'a str,
        output: &'a MessageContent,
    },
}

/// `context::push_text` joins neighboring same-role text messages. Compare
/// those runs the same way, while retaining the original messages for reuse.
/// Only a merged run allocates; the usual alternating messages are borrowed.
fn push_message<'a>(
    out: &mut Vec<(Atom<'a>, Option<usize>)>,
    message: &'a ChatMessage,
    boundary: Option<usize>,
) {
    if matches!(message.role.as_str(), "user" | "assistant")
        && let MessageContent::Text(text) = &message.content
        && let Some((Atom::Message(role, content), previous_boundary)) = out.last_mut()
        && *role == message.role
        && matches!(content.as_ref(), MessageContent::Text(_))
    {
        let MessageContent::Text(previous) = content.to_mut() else {
            unreachable!("matched plain text above");
        };
        previous.push_str(crate::context::MERGE_SEPARATOR);
        previous.push_str(text);
        // A tool-call assistant's text ends inside its batch. Clear the old
        // boundary so matching just that text cannot discard the tool calls.
        *previous_boundary = boundary;
        return;
    }
    out.push((
        Atom::Message(&message.role, Cow::Borrowed(&message.content)),
        boundary,
    ));
}

/// Each atom also names the message boundary that completely contains it.
/// A restored prefix may never cut a newly reconstructed parallel batch in
/// half (its assistant message would otherwise lose the remaining calls).
fn atoms(messages: &[ChatMessage]) -> Option<Vec<(Atom<'_>, Option<usize>)>> {
    let mut out = Vec::new();
    let mut index = 0;
    while let Some(message) = messages.get(index) {
        if message.role == "tool" || message.tool_call_id.is_some() {
            return None;
        }
        if message.tool_calls.is_empty() {
            push_message(&mut out, message, Some(index + 1));
            index += 1;
            continue;
        }
        if message.role != "assistant" {
            return None;
        }
        let end = index.checked_add(1 + message.tool_calls.len())?;
        let results = messages.get(index + 1..end)?;
        let mut by_id = std::collections::BTreeMap::new();
        for result in results {
            if result.role != "tool" || !result.tool_calls.is_empty() {
                return None;
            }
            if by_id
                .insert(result.tool_call_id.as_deref()?, &result.content)
                .is_some()
            {
                return None;
            }
        }
        // Nothing but whitespace before the calls is no message: the wire
        // carries whatever the model emitted (a `\n\n` on the Responses and
        // Messages wires), while the app records no segment for a
        // whitespace-only run (`App::flush_streaming_segment`), so the replay
        // comes back empty. Either way the round said nothing before its
        // calls.
        if !matches!(&message.content, MessageContent::Text(text) if text.trim().is_empty()) {
            push_message(&mut out, message, None);
        }
        for call in &message.tool_calls {
            let output = by_id.remove(call.id.as_str())?;
            out.push((
                Atom::Call {
                    kind: &call.kind,
                    name: &call.function.name,
                    arguments: &call.function.arguments,
                    output,
                },
                None,
            ));
        }
        out.last_mut()?.1 = Some(end);
        index = end;
    }
    Some(out)
}

fn matching_prefix(previous: &[ChatMessage], current: &[ChatMessage]) -> Option<usize> {
    if previous.is_empty() {
        return None;
    }
    let previous_atoms = atoms(previous)?;
    let current_atoms = atoms(current)?;
    if previous_atoms.len() > current_atoms.len()
        || previous_atoms
            .iter()
            .zip(&current_atoms)
            .any(|(a, b)| a.0 != b.0)
    {
        return None;
    }
    let boundary = current_atoms.get(previous_atoms.len().checked_sub(1)?)?.1?;
    // A cancelled/tool-limited turn can leave newly reconstructed calls in
    // the suffix. Their synthetic IDs must not collide with the provider's
    // retained IDs after the splice, even though both inputs were valid.
    let suffix_ids: std::collections::BTreeSet<&str> = current[boundary..]
        .iter()
        .flat_map(|message| message.tool_calls.iter().map(|call| call.id.as_str()))
        .collect();
    if previous
        .iter()
        .flat_map(|message| &message.tool_calls)
        .any(|call| suffix_ids.contains(call.id.as_str()))
    {
        return None;
    }
    Some(boundary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ContentPart, ToolCallSpec};

    fn call(id: &str, name: &str, args: &str) -> ToolCallSpec {
        ToolCallSpec::function(id, name, args)
    }

    fn prior_request() -> Vec<ChatMessage> {
        vec![
            ChatMessage::system("stable system"),
            ChatMessage::user("inspect the source"),
            ChatMessage::assistant_tool_calls(
                "Reading both files.",
                vec![
                    call("provider_A", "read", r#"{"path":"a.rs"}"#),
                    call("provider_B", "read", r#"{"path":"b.rs"}"#),
                ],
            ),
            ChatMessage::tool_result("provider_A", "source A"),
            ChatMessage::tool_result("provider_B", "source B"),
        ]
    }

    fn replayed_request() -> Vec<ChatMessage> {
        use crate::app::{HistoryItem, Message, Role, ToolCall, ToolStatus};
        let message = |role, text: &str| {
            HistoryItem::Message(Message {
                role,
                text: text.into(),
                timestamp: String::new(),
                images: Vec::new(),
            })
        };
        let tool = |path: &str, output: &str| {
            HistoryItem::Tool(ToolCall {
                name: "Read".into(),
                args: path.into(),
                status: ToolStatus::Ok,
                output: output.into(),
                timestamp: String::new(),
                shell: false,
                truncated: false,
                context_output: None,
                arguments: Some(format!(r#"{{"path":"{path}"}}"#)),
                approval_note: None,
                batch: Some(0),
                call_id: None,
            })
        };
        let history = vec![
            message(Role::User, "inspect the source"),
            message(Role::Assistant, "Reading both files."),
            tool("a.rs", "source A"),
            tool("b.rs", "source B"),
            message(Role::Assistant, "Both files are fine."),
            message(Role::User, "now run the checks"),
        ];
        crate::llm::backend::build_messages(
            Some("stable system"),
            "now run the checks",
            &crate::context::context_messages(&history),
            |_| None::<String>,
        )
    }

    #[test]
    fn next_human_turn_keeps_original_ids_and_parallel_batch_bytes() {
        let prior = prior_request();
        let mut next = replayed_request();
        assert_ne!(&next[..prior.len()], prior.as_slice());
        let mut history = WireHistory::default();
        history.remember(&prior);
        history.restore(&mut next);
        assert_eq!(&next[..prior.len()], prior.as_slice());
        assert_eq!(
            next[prior.len()].content,
            MessageContent::Text("Both files are fine.".into())
        );
        assert_eq!(
            next.last().unwrap().content,
            MessageContent::Text("now run the checks".into())
        );
    }

    #[test]
    fn opening_hook_notes_keep_the_exact_prefix_after_history_merges_user_messages() {
        use crate::app::{HistoryItem, HookNote, Message, Role};
        let prior = vec![
            ChatMessage::system("stable system"),
            ChatMessage::user("inspect the source"),
            ChatMessage::user("session hook context"),
            ChatMessage::user("prompt hook context"),
        ];
        let message = |role, text: &str| {
            HistoryItem::Message(Message {
                role,
                text: text.into(),
                timestamp: String::new(),
                images: Vec::new(),
            })
        };
        let hook = |text: &str| {
            HistoryItem::HookNote(HookNote {
                label: "hook".into(),
                text: text.into(),
                timestamp: String::new(),
            })
        };
        let context = crate::context::context_messages(&[
            message(Role::User, "inspect the source"),
            hook("session hook context"),
            hook("prompt hook context"),
            message(Role::Assistant, "Looks good."),
            message(Role::User, "run the checks"),
        ]);
        assert_eq!(
            context[0].text,
            "inspect the source\n\nsession hook context\n\nprompt hook context"
        );
        let mut next = crate::llm::backend::build_messages(
            Some("stable system"),
            "run the checks",
            &context,
            |_| None::<String>,
        );
        let suffix = next[2..].to_vec();
        let mut history = WireHistory::default();
        history.remember(&prior);
        history.restore(&mut next);
        assert_eq!(&next[..prior.len()], prior.as_slice());
        assert_eq!(&next[prior.len()..], suffix.as_slice());
    }

    #[test]
    fn merged_plain_runs_never_restore_a_partial_message_or_tool_batch() {
        let previous = vec![ChatMessage::user("prompt"), ChatMessage::user("hook")];
        let current = vec![ChatMessage::user("prompt\n\nhook\n\nnew instruction")];
        assert_eq!(matching_prefix(&previous, &current), None);
        let previous = vec![ChatMessage::new("assistant", "intro\n\nreading")];
        let current = vec![
            ChatMessage::new("assistant", "intro"),
            ChatMessage::assistant_tool_calls("reading", vec![call("id", "read", "{}")]),
            ChatMessage::tool_result("id", "source"),
        ];
        assert_eq!(matching_prefix(&previous, &current), None);
    }

    #[test]
    fn newline_only_assistant_text_before_calls_matches_the_recorded_empty_one() {
        // On the Responses and Messages wires a model that emits only `\n\n`
        // before its calls sends that text back as the round's content, while
        // the app records no assistant segment for it
        // (`App::flush_streaming_segment` drops a whitespace-only run), so the
        // replay carries an empty content. Same round, same conversation: the
        // provider's copy must still be the one reused.
        let previous = vec![
            ChatMessage::system("stable"),
            ChatMessage::user("inspect"),
            ChatMessage::assistant_tool_calls(
                "\n\n",
                vec![call("provider_A", "read", r#"{"path":"a.rs"}"#)],
            ),
            ChatMessage::tool_result("provider_A", "source A"),
        ];
        let mut current = vec![
            ChatMessage::system("stable"),
            ChatMessage::user("inspect"),
            ChatMessage::assistant_tool_calls(
                "",
                vec![call("call_0", "read", r#"{"path":"a.rs"}"#)],
            ),
            ChatMessage::tool_result("call_0", "source A"),
            ChatMessage::new("assistant", "done"),
            ChatMessage::user("next"),
        ];
        assert_eq!(matching_prefix(&previous, &current), Some(4));
        let mut history = WireHistory::default();
        history.remember(&previous);
        history.restore(&mut current);
        assert_eq!(&current[..4], previous.as_slice());
        assert_eq!(current[4].content, MessageContent::Text("done".into()));
        // Words before the calls are content: a replay that lost them is a
        // different conversation and never matches.
        let spoken = vec![
            ChatMessage::system("stable"),
            ChatMessage::user("inspect"),
            ChatMessage::assistant_tool_calls(
                "Reading.",
                vec![call("provider_A", "read", r#"{"path":"a.rs"}"#)],
            ),
            ChatMessage::tool_result("provider_A", "source A"),
        ];
        assert_eq!(matching_prefix(&spoken, &current), None);
    }

    #[test]
    fn retained_provider_ids_must_not_collide_with_reconstructed_suffix_calls() {
        let previous = vec![
            ChatMessage::user("inspect"),
            ChatMessage::assistant_tool_calls("", vec![call("call_1", "read", "{}")]),
            ChatMessage::tool_result("call_1", "source"),
        ];
        let mut current = vec![
            ChatMessage::user("inspect"),
            ChatMessage::assistant_tool_calls("", vec![call("call_0", "read", "{}")]),
            ChatMessage::tool_result("call_0", "source"),
            ChatMessage::assistant_tool_calls("", vec![call("call_1", "bash", "{}")]),
            ChatMessage::tool_result("call_1", "stopped"),
        ];
        let mut history = WireHistory::default();
        history.remember(&previous);
        let expected = current.clone();
        history.restore(&mut current);
        assert_eq!(current, expected);
        current[3].tool_calls[0].id = "call_2".into();
        current[4].tool_call_id = Some("call_2".into());
        assert_eq!(matching_prefix(&previous, &current), Some(3));
    }

    #[test]
    fn changed_reordered_or_truncated_context_never_restores_old_content() {
        let prior = prior_request();
        let original = replayed_request();
        let mut cases = Vec::new();
        // Every index inside the retained prefix: the system prompt, the
        // prompt, and the two results of the batch (the reply after the
        // batch is the suffix, free to differ).
        for index in [0, 1, 3, 4] {
            let mut changed = original.clone();
            changed[index].content = MessageContent::Text("changed".into());
            cases.push(changed);
        }
        let mut changed_arguments = original.clone();
        changed_arguments[2].tool_calls[0].function.arguments = r#"{"path":"new.rs"}"#.into();
        cases.push(changed_arguments);
        cases.push(original[..4].to_vec());
        cases.push(vec![ChatMessage::user("fresh conversation")]);
        let mut reordered = original.clone();
        reordered[2..6].rotate_left(2);
        cases.push(reordered);
        for mut current in cases {
            let expected = current.clone();
            let mut history = WireHistory::default();
            history.remember(&prior);
            history.restore(&mut current);
            assert_eq!(current, expected);
            assert!(history.messages.is_empty());
        }
    }

    #[test]
    fn incomplete_duplicate_or_unmatched_tool_results_fail_closed() {
        let prior = prior_request();
        for current in [
            prior[..4].to_vec(),
            {
                let mut current = prior.clone();
                current[4].tool_call_id = Some("provider_A".into());
                current
            },
            {
                let mut current = prior.clone();
                current[4].tool_call_id = Some("unknown".into());
                current
            },
        ] {
            assert_eq!(matching_prefix(&prior, &current), None);
        }
    }

    #[test]
    fn matching_prefix_cannot_cut_a_parallel_batch_in_half() {
        let mut prior = prior_request();
        prior[2].tool_calls.truncate(1);
        prior.truncate(4);
        assert_eq!(matching_prefix(&prior, &prior_request()), None);
    }

    #[test]
    fn repeated_human_turns_keep_the_entire_growing_tool_prefix_stable() {
        let mut sent = vec![ChatMessage::system("stable"), ChatMessage::user("turn 0")];
        let mut replay = sent.clone();
        let mut history = WireHistory::default();
        // 64 turns with 16 parallel calls each; IDs differ on every replay,
        // as they do when App rebuilds its full history for a human turn.
        for turn in 0..64 {
            let calls: Vec<_> = (0..16)
                .map(|index| {
                    call(
                        &format!("wire-{turn}-{index}"),
                        "read",
                        &format!(r#"{{"path":"{turn}/{index}.rs"}}"#),
                    )
                })
                .collect();
            sent.push(ChatMessage::assistant_tool_calls("", calls.clone()));
            for (index, call) in calls.into_iter().enumerate() {
                let output = format!("source for {turn}/{index}");
                sent.push(ChatMessage::tool_result(&call.id, &output));
                let id = format!("call_{}", turn * 16 + index);
                replay.push(ChatMessage::assistant_tool_calls(
                    "",
                    vec![self::call(&id, "read", &call.function.arguments)],
                ));
                replay.push(ChatMessage::tool_result(id, output));
            }
            history.remember(&sent);
            let suffix = [
                ChatMessage::new("assistant", format!("done {turn}")),
                ChatMessage::user(format!("turn {}", turn + 1)),
            ];
            replay.extend(suffix.clone());
            let mut next = replay.clone();
            history.restore(&mut next);
            assert_eq!(&next[..sent.len()], sent.as_slice(), "turn {turn}");
            sent.extend(suffix);
            assert_eq!(next, sent);
        }
    }

    #[test]
    fn mixed_task_and_ordinary_calls_match_only_with_the_same_order_and_arguments() {
        let mut previous = prior_request();
        previous[2].tool_calls[0].function.name = "tasklist".into();
        previous[2].tool_calls[0].function.arguments = "{}".into();
        let mut current = replayed_request();
        current[2].tool_calls[0].function.name = "tasklist".into();
        current[2].tool_calls[0].function.arguments = "{}".into();
        assert_eq!(matching_prefix(&previous, &current), Some(5));
        // Agent launches can be recorded in execution order instead of the
        // model's order; reconstructed defaults can change their arguments.
        // Neither is permission to replay different context.
        current[2..6].rotate_left(2);
        assert_eq!(matching_prefix(&previous, &current), None);
        current[2..6].rotate_left(2);
        current[2].tool_calls[0].function.arguments = "{ }".into();
        assert_eq!(matching_prefix(&previous, &current), None);
    }

    #[test]
    fn images_are_shared_and_changed_images_invalidate_the_prefix() {
        let url = crate::images::AttachmentUrl::from("data:image/png;base64,old");
        let prior = vec![ChatMessage::with_parts(
            "user",
            vec![ContentPart::image(url.clone())],
        )];
        let mut history = WireHistory::default();
        history.remember(&prior);
        let MessageContent::Parts(parts) = &history.messages[0].content else {
            panic!()
        };
        let ContentPart::ImageUrl { image_url } = &parts[0] else {
            panic!()
        };
        assert!(crate::images::AttachmentUrl::ptr_eq(&url, &image_url.url));
        let mut current = vec![ChatMessage::with_parts(
            "user",
            vec![ContentPart::image("data:image/png;base64,new")],
        )];
        let expected = current.clone();
        history.restore(&mut current);
        assert_eq!(current, expected);
    }

    #[test]
    fn retained_request_has_a_memory_bound() {
        let mut history = WireHistory::default();
        history.remember(&prior_request());
        assert!(!history.messages.is_empty());
        history.remember(&[ChatMessage::user("x".repeat(MAX_RETAINED_BYTES + 1))]);
        assert!(history.messages.is_empty());
    }
}
