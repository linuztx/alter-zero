//! The per-session LLM conversation context (see `docs/context.md`).
//!
//! A real backend must send the *whole* conversation each turn or the model
//! loses context. This module derives that raw message list from
//! [`App::history`] — the single source of truth the TUI already keeps
//! correct across `/clear`, the Esc-Esc backtrack rewind, an interrupt
//! rollback, and a `/resume` load — so the context can never drift from what
//! the user sees. Every history item type is represented: user and assistant
//! text verbatim (image placeholders included), a model's tool calls in the
//! **provider-native** shape (an assistant `tool_calls` entry paired to a
//! `tool`-role result — the same protocol the live agent loop streams, replayed
//! across turns), `!` shell runs as a natural `$ command` transcript, and
//! error/system notices as bracketed user-role notes. Pure and unit-tested; the
//! derivation runs at the boundary right before each [`ReplySource::spawn`].
//!
//! [`App::history`]: crate::app::App::history
//! [`ReplySource::spawn`]: crate::stream::ReplySource::spawn

use std::path::PathBuf;

use crate::app::{HistoryItem, Role, ToolCall};
use crate::llm::tools::{image_attachment_note, is_image_read_output};

/// The wire role a context message is sent as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextRole {
    /// The provider-side instructions — only ever the backend's system
    /// prompt, at the front of the request. TUI notices ride as bracketed
    /// `user` notes instead: strict OpenAI-compatible providers (alternation
    /// templates) reject mid-conversation system messages.
    System,
    /// The human: typed messages, `!` shell runs, and bracketed TUI notices.
    User,
    /// The model: reply text and the tool calls it requested.
    Assistant,
    /// A tool result — the output of a model tool call, paired back to its
    /// assistant `tool_calls` entry by [`ContextMessage::tool_call_id`]. The
    /// Chat Completions `role:"tool"` message.
    Tool,
}

impl ContextRole {
    /// The OpenAI-compatible `role` string.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// One entry of an assistant message's native `tool_calls` array: the model's
/// request to run a tool, replayed on later turns in the provider-native Chat
/// Completions shape (see `docs/context.md`). `id` pairs it to the following
/// `tool`-role result; `name` is the wire tool name (`bash`/`read`/`write`/
/// `edit`); `arguments` is a JSON argument object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl ContextToolCall {
    /// A tool call with the given id, wire name, and JSON arguments.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }
}

/// One raw message of the context window: the role it is sent as, the raw text
/// (verbatim user/assistant content, a `$ command` shell transcript, or a
/// bracketed notice), the temp-file paths of a user message's Ctrl+V image
/// attachments (a vision backend re-encodes them each turn; see
/// `docs/image-paste.md`), the native `tool_calls` an assistant entry requested,
/// and — on a `tool`-role entry — the id of the call it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMessage {
    pub role: ContextRole,
    pub text: String,
    pub images: Vec<PathBuf>,
    /// The native tool calls this assistant entry requested — non-empty only on
    /// an [`ContextRole::Assistant`] entry (its `content` may then be empty).
    pub tool_calls: Vec<ContextToolCall>,
    /// The call id a [`ContextRole::Tool`] result answers — `Some` only on a
    /// tool entry.
    pub tool_call_id: Option<String>,
}

impl ContextMessage {
    /// A plain (imageless, tool-less) context message.
    #[must_use]
    pub fn new(role: ContextRole, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// An assistant entry carrying native tool calls (its `text`/content may be
    /// empty when the model only called tools).
    #[must_use]
    pub fn assistant_tool_calls(text: impl Into<String>, tool_calls: Vec<ContextToolCall>) -> Self {
        Self {
            role: ContextRole::Assistant,
            text: text.into(),
            images: Vec::new(),
            tool_calls,
            tool_call_id: None,
        }
    }

    /// A `tool`-role result answering the call `id`.
    #[must_use]
    pub fn tool_result(id: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            role: ContextRole::Tool,
            text: output.into(),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: Some(id.into()),
        }
    }
}

/// How adjacent same-role plain-text context entries are joined when merged.
const MERGE_SEPARATOR: &str = "\n\n";

/// The wire tool name (lowercase, as declared to the provider) for a finished
/// tool's display `name` (title-cased by `llm::tools::display_name`). The four
/// known tools reverse exactly; anything else falls back to a lowercase of the
/// display name.
fn wire_tool_name(display: &str) -> String {
    match display {
        "Bash" => "bash".to_string(),
        "Read" => "read".to_string(),
        "Write" => "write".to_string(),
        "Edit" => "edit".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

/// Reconstruct a JSON argument object for a finished tool call from its stored
/// one-line summary. History keeps only the summary (`llm::tools::summarize_call`
/// — the command for `bash`, the path for the file tools), not the raw argument
/// JSON, so the replayed call carries the essential argument; the tool
/// **result** (below it) carries the full outcome the model reasons from. An
/// unrecognised tool replays with empty arguments.
fn reconstruct_arguments(tool: &ToolCall) -> String {
    let key = match tool.name.as_str() {
        "Bash" => "command",
        "Read" | "Write" | "Edit" => "path",
        _ => return "{}".to_string(),
    };
    serde_json::json!({ key: tool.args }).to_string()
}

/// Push a plain text entry, merging it into the previous entry when they share a
/// role and neither carries tool-call data — so message batches and adjacent
/// notices collapse and the plain-text portions stay alternating. Tool-call
/// assistant messages and tool-result messages are never merge targets (a tool
/// result must sit between them).
fn push_text(out: &mut Vec<ContextMessage>, role: ContextRole, text: String, images: Vec<PathBuf>) {
    if let Some(last) = out.last_mut()
        && last.role == role
        && last.tool_calls.is_empty()
        && last.tool_call_id.is_none()
    {
        last.text.push_str(MERGE_SEPARATOR);
        last.text.push_str(&text);
        last.images.extend(images);
        return;
    }
    out.push(ContextMessage {
        role,
        text,
        images,
        tool_calls: Vec::new(),
        tool_call_id: None,
    });
}

/// Derive the raw context window from the conversation history, oldest first.
///
/// - [`Role::User`] / [`Role::Assistant`] messages carry their text verbatim
///   (image placeholders included) — a user message also carries its
///   attachment paths.
/// - A backend [`HistoryItem::Tool`] becomes a native tool-call pair: its
///   request folded onto the preceding assistant segment (or a fresh assistant
///   message with empty content) as a [`ContextToolCall`], immediately followed
///   by a [`ContextRole::Tool`] result. Ids are synthesized per derivation
///   (`call_0`, `call_1`, …) — the pairing only has to be internally consistent
///   within the request, which is rebuilt each turn.
/// - A `!` shell [`HistoryItem::Tool`] becomes a `$ command` / output transcript
///   under the **user** role (the user ran it locally — it is not a model tool
///   call, so it cannot be a native `tool` message).
/// - [`Role::Error`] / [`Role::System`] notices become `[error]` / `[system]`
///   **user-role** notes, so the model knows about interrupts, failures, and
///   slash-command output. (Not system-role: strict OpenAI-compatible providers
///   reject mid-conversation system messages; the bracket prefix marks them as
///   UI, not the human.)
/// - [`Role::Shell`] header messages are skipped — the shell *tool* recorded
///   with them already carries the command and its output.
/// - [`HistoryItem::Summary`] rows are TUI chrome and are skipped.
///
/// Adjacent same-role plain-text entries are **merged** (texts joined with a
/// blank line, attachments concatenated). What Ctrl+D shows *is* this derived
/// form — exactly the wire messages [`crate::llm::backend::build_messages`]
/// sends.
#[must_use]
pub fn context_messages(history: &[HistoryItem]) -> Vec<ContextMessage> {
    let mut out: Vec<ContextMessage> = Vec::new();
    let mut tool_seq = 0usize;
    for (position, item) in history.iter().enumerate() {
        match item {
            HistoryItem::Message(message) => match message.role {
                Role::User => push_text(
                    &mut out,
                    ContextRole::User,
                    message.text.clone(),
                    message.images.clone(),
                ),
                Role::Assistant => {
                    push_text(
                        &mut out,
                        ContextRole::Assistant,
                        message.text.clone(),
                        vec![],
                    );
                }
                Role::Error => push_text(
                    &mut out,
                    ContextRole::User,
                    format!("[error] {}", message.text),
                    vec![],
                ),
                Role::System => push_text(
                    &mut out,
                    ContextRole::User,
                    format!("[system] {}", message.text),
                    vec![],
                ),
                Role::Shell => {
                    // Its tool cell (recorded when the command resolves)
                    // carries the command + output — except for a *dangling*
                    // header: the app quit mid-command, so no tool ever
                    // landed (docs/resume.md). Emit the bare command then, or
                    // the run would vanish from the derived context while the
                    // transcript still shows it.
                    let resolved = matches!(
                        history.get(position + 1),
                        Some(HistoryItem::Tool(tool)) if tool.shell
                    );
                    if !resolved {
                        push_text(
                            &mut out,
                            ContextRole::User,
                            format!("$ {}", message.text),
                            vec![],
                        );
                    }
                }
            },
            HistoryItem::Tool(tool) if tool.shell => {
                // The user's local `!` command: a natural shell transcript under
                // the user role (no native `tool` message — the model didn't
                // call it).
                let mut text = format!("$ {}", tool.name);
                if !tool.output.is_empty() {
                    text.push('\n');
                    text.push_str(&tool.output);
                }
                push_text(&mut out, ContextRole::User, text, vec![]);
            }
            HistoryItem::Tool(tool) => {
                let id = format!("call_{tool_seq}");
                tool_seq += 1;
                let call = ContextToolCall::new(
                    id.clone(),
                    wire_tool_name(&tool.name),
                    reconstruct_arguments(tool),
                );
                // Fold onto the open assistant text segment when there is one,
                // else start a fresh assistant message (empty content is fine).
                match out.last_mut() {
                    Some(last) if last.role == ContextRole::Assistant => {
                        last.tool_calls.push(call);
                    }
                    _ => out.push(ContextMessage::assistant_tool_calls("", vec![call])),
                }
                out.push(ContextMessage::tool_result(id, tool.output.clone()));
                // An image `read` (docs/tools.md): the live agent loop
                // attached the pixels as a follow-up user message; replay the
                // same shape so later turns keep seeing them. The stored args
                // are the path — the backend re-encodes it each request (a
                // gone file becomes an `[image unavailable]` note there).
                if tool.name == "Read" && is_image_read_output(&tool.output) {
                    push_text(
                        &mut out,
                        ContextRole::User,
                        image_attachment_note(&tool.args),
                        vec![PathBuf::from(&tool.args)],
                    );
                }
            }
            HistoryItem::Summary(_) => {} // TUI chrome, not conversation
            // A background shell's completion: a bracketed user-role note
            // carrying the outcome AND the output tail — the model reads the
            // result here (the rendered cell shows only the one-line headline).
            // User-role like the other notices: strict providers reject
            // mid-conversation system messages. See docs/background.md.
            HistoryItem::Background(notice) => {
                push_text(&mut out, ContextRole::User, notice.context_text(), vec![]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Message, ToolStatus, TurnSummary};

    fn message(role: Role, text: &str) -> HistoryItem {
        HistoryItem::Message(Message {
            role,
            text: text.to_string(),
            timestamp: String::new(),
            images: Vec::new(),
        })
    }

    fn tool(name: &str, args: &str, output: &str, status: ToolStatus, shell: bool) -> HistoryItem {
        HistoryItem::Tool(ToolCall {
            name: name.to_string(),
            args: args.to_string(),
            status,
            output: output.to_string(),
            timestamp: String::new(),
            shell,
            truncated: false,
        })
    }

    #[test]
    fn empty_history_derives_an_empty_context() {
        assert!(context_messages(&[]).is_empty());
    }

    #[test]
    fn user_and_assistant_text_map_verbatim_in_order() {
        let history = vec![
            message(Role::User, "hello there"),
            message(Role::Assistant, "hi! how can I help?"),
            message(Role::User, "what's 2+2?"),
        ];
        let ctx = context_messages(&history);
        assert_eq!(
            ctx,
            vec![
                ContextMessage::new(ContextRole::User, "hello there"),
                ContextMessage::new(ContextRole::Assistant, "hi! how can I help?"),
                ContextMessage::new(ContextRole::User, "what's 2+2?"),
            ]
        );
    }

    #[test]
    fn a_user_message_carries_its_image_attachments() {
        let history = vec![HistoryItem::Message(Message {
            role: Role::User,
            text: "[Image #1] what is this?".to_string(),
            timestamp: String::new(),
            images: vec![PathBuf::from("/tmp/shot.png")],
        })];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 1);
        assert_eq!(ctx[0].role, ContextRole::User);
        assert_eq!(ctx[0].text, "[Image #1] what is this?");
        assert_eq!(ctx[0].images, vec![PathBuf::from("/tmp/shot.png")]);
    }

    #[test]
    fn a_backend_tool_becomes_a_native_call_plus_tool_result() {
        // No preceding assistant text: a fresh empty-content assistant message
        // carries the tool call, and a `tool`-role result follows, paired by id.
        let history = vec![tool(
            "Read",
            "src/main.rs",
            "fn main() {}",
            ToolStatus::Ok,
            false,
        )];
        let ctx = context_messages(&history);
        assert_eq!(
            ctx,
            vec![
                ContextMessage::assistant_tool_calls(
                    "",
                    vec![ContextToolCall::new(
                        "call_0",
                        "read",
                        r#"{"path":"src/main.rs"}"#
                    )]
                ),
                ContextMessage::tool_result("call_0", "fn main() {}"),
            ]
        );
    }

    #[test]
    fn a_bash_tool_reconstructs_a_command_argument() {
        let history = vec![tool(
            "Bash",
            "grep -n TODO",
            "no matches",
            ToolStatus::Ok,
            false,
        )];
        let ctx = context_messages(&history);
        assert_eq!(ctx[0].role, ContextRole::Assistant);
        assert_eq!(
            ctx[0].tool_calls,
            vec![ContextToolCall::new(
                "call_0",
                "bash",
                r#"{"command":"grep -n TODO"}"#
            )]
        );
        assert_eq!(ctx[1], ContextMessage::tool_result("call_0", "no matches"));
    }

    #[test]
    fn a_failed_tool_conveys_the_failure_in_the_result_content() {
        // The native shape carries no ok/failed tag — the error rides in the
        // tool result content (as the executor framed it), where the model reads
        // it, and the red cell is a TUI-only concern.
        let history = vec![tool(
            "Bash",
            "grep -n TODO",
            "Exit code: 1\nno matches",
            ToolStatus::Failed,
            false,
        )];
        let ctx = context_messages(&history);
        assert_eq!(ctx[1].role, ContextRole::Tool);
        assert_eq!(ctx[1].text, "Exit code: 1\nno matches");
    }

    #[test]
    fn a_tool_with_no_output_still_pairs_an_empty_result() {
        let history = vec![tool("Read", "a.txt", "", ToolStatus::Ok, false)];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 2);
        assert_eq!(ctx[1], ContextMessage::tool_result("call_0", ""));
    }

    #[test]
    fn a_tool_folds_onto_the_preceding_assistant_text() {
        // The assistant text that streamed before the call and the call itself
        // become one assistant message (content + tool_calls) — the canonical
        // Chat Completions shape.
        let history = vec![
            message(Role::Assistant, "let me check"),
            tool("Read", "f", "L1", ToolStatus::Ok, false),
        ];
        let ctx = context_messages(&history);
        assert_eq!(
            ctx,
            vec![
                ContextMessage::assistant_tool_calls(
                    "let me check",
                    vec![ContextToolCall::new("call_0", "read", r#"{"path":"f"}"#)]
                ),
                ContextMessage::tool_result("call_0", "L1"),
            ]
        );
    }

    #[test]
    fn consecutive_tools_each_get_their_own_call_and_result_with_distinct_ids() {
        // The dummy's Read-then-Bash shape: each tool gets its own assistant
        // message + result, ids synthesized in order.
        let history = vec![
            message(Role::Assistant, "first"),
            tool("Read", "src/main.rs", "L1", ToolStatus::Ok, false),
            tool(
                "Bash",
                "grep -n TODO",
                "no matches",
                ToolStatus::Failed,
                false,
            ),
            message(Role::Assistant, "second"),
        ];
        let ctx = context_messages(&history);
        assert_eq!(
            ctx,
            vec![
                ContextMessage::assistant_tool_calls(
                    "first",
                    vec![ContextToolCall::new(
                        "call_0",
                        "read",
                        r#"{"path":"src/main.rs"}"#
                    )]
                ),
                ContextMessage::tool_result("call_0", "L1"),
                ContextMessage::assistant_tool_calls(
                    "",
                    vec![ContextToolCall::new(
                        "call_1",
                        "bash",
                        r#"{"command":"grep -n TODO"}"#
                    )]
                ),
                ContextMessage::tool_result("call_1", "no matches"),
                ContextMessage::new(ContextRole::Assistant, "second"),
            ]
        );
    }

    #[test]
    fn an_image_read_replays_its_attachment_as_a_user_note() {
        // The live turn attached the pixels as a follow-up user message
        // (llm::agent); the replay must reconstruct the same wire shape from
        // the stored record — the note text via the shared
        // `llm::tools::image_attachment_note`, the path as an `images`
        // attachment the backend re-encodes each turn (a gone file becomes an
        // `[image unavailable]` note there). See docs/tools.md.
        let history = vec![tool(
            "Read",
            "assets/shot.png",
            "Read image assets/shot.png (PNG, 3x2, 90 B)\n\
             The image is attached as the next user message.",
            ToolStatus::Ok,
            false,
        )];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 3, "call + result + attachment note: {ctx:?}");
        assert_eq!(ctx[1].role, ContextRole::Tool);
        let note = &ctx[2];
        assert_eq!(note.role, ContextRole::User);
        assert_eq!(
            note.text,
            crate::llm::tools::image_attachment_note("assets/shot.png"),
            "the note text matches the live turn's exactly"
        );
        assert_eq!(note.images, vec![PathBuf::from("assets/shot.png")]);
        assert!(note.tool_calls.is_empty() && note.tool_call_id.is_none());
    }

    #[test]
    fn a_text_read_replays_without_an_attachment() {
        let history = vec![tool(
            "Read",
            "src/main.rs",
            "1 fn main() {}",
            ToolStatus::Ok,
            false,
        )];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 2, "no injected note for a text read: {ctx:?}");
    }

    #[test]
    fn a_bash_echo_of_the_marker_is_not_an_attachment() {
        // Only a `read` cell can carry an image — a bash command whose output
        // merely starts with the marker text must not inject a phantom note.
        let history = vec![tool(
            "Bash",
            "cat log.txt",
            "Read image assets/shot.png (PNG, 3x2, 90 B)",
            ToolStatus::Ok,
            false,
        )];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 2, "no note for a bash cell: {ctx:?}");
    }

    #[test]
    fn the_users_next_message_merges_into_the_attachment_note() {
        // Alternation safety: the injected note and the user's next typed
        // message collapse into one user entry (like every other adjacent
        // same-role pair), the attachment riding along.
        let history = vec![
            tool(
                "Read",
                "shot.png",
                "Read image shot.png (PNG, 1x1, 68 B)",
                ToolStatus::Ok,
                false,
            ),
            message(Role::User, "what color is it?"),
        ];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 3, "call + result + merged user entry: {ctx:?}");
        let merged = &ctx[2];
        assert_eq!(merged.role, ContextRole::User);
        assert!(merged.text.ends_with("what color is it?"), "{merged:?}");
        assert_eq!(merged.images, vec![PathBuf::from("shot.png")]);
    }

    #[test]
    fn a_shell_run_becomes_a_natural_transcript_and_its_header_is_skipped() {
        // begin_shell records the Role::Shell header message *and* the shell
        // tool; only the tool reaches the context (it carries the command), as a
        // `$ command` transcript under the user role — no bracket, no native
        // `tool` message.
        let history = vec![
            message(Role::Shell, "pwd"),
            tool("pwd", "", "/home/user", ToolStatus::Ok, true),
        ];
        let ctx = context_messages(&history);
        assert_eq!(
            ctx,
            vec![ContextMessage::new(ContextRole::User, "$ pwd\n/home/user")]
        );
    }

    #[test]
    fn a_shell_run_with_no_output_is_just_the_command_line() {
        let history = vec![tool("true", "", "", ToolStatus::Ok, true)];
        let ctx = context_messages(&history);
        assert_eq!(ctx, vec![ContextMessage::new(ContextRole::User, "$ true")]);
    }

    #[test]
    fn a_dangling_shell_header_still_reaches_the_context() {
        // Quitting mid-`!` command records the Role::Shell header with no
        // tool cell behind it (the tool only lands when the command
        // resolves); after /resume the transcript shows `! make build` but
        // the derived context must not silently omit the run.
        let history = vec![message(Role::Shell, "make build")];
        let ctx = context_messages(&history);
        assert_eq!(
            ctx,
            vec![ContextMessage::new(ContextRole::User, "$ make build")]
        );
    }

    #[test]
    fn a_resolved_shell_headers_command_is_not_doubled() {
        // The header before a resolved shell tool stays skipped — only the
        // dangling case emits.
        let history = vec![
            message(Role::Shell, "pwd"),
            tool("pwd", "", "/home/user", ToolStatus::Ok, true),
        ];
        let ctx = context_messages(&history);
        assert_eq!(
            ctx,
            vec![ContextMessage::new(ContextRole::User, "$ pwd\n/home/user")]
        );
    }

    #[test]
    fn notices_become_bracketed_user_role_notes() {
        // User-role, not system-role: strict OpenAI-compatible providers
        // reject mid-conversation system messages, and adjacent notes merge
        // into one entry (the alternation-safe wire shape).
        let history = vec![
            message(Role::Error, "Conversation interrupted"),
            message(Role::System, "help text"),
        ];
        let ctx = context_messages(&history);
        assert_eq!(
            ctx,
            vec![ContextMessage::new(
                ContextRole::User,
                "[error] Conversation interrupted\n\n[system] help text"
            )]
        );
    }

    #[test]
    fn a_full_turn_derives_a_valid_native_tool_sequence() {
        // user, assistant segment, tool record, closing segment: the tool splits
        // the assistant into a call-carrying message + its result, then the
        // final answer — the exact assistant→tool→assistant shape the live loop
        // streams.
        let history = vec![
            message(Role::User, "do the thing"),
            message(Role::Assistant, "let me check"),
            tool("Read", "f", "L1", ToolStatus::Ok, false),
            message(Role::Assistant, "all done"),
        ];
        let ctx = context_messages(&history);
        assert_eq!(
            ctx,
            vec![
                ContextMessage::new(ContextRole::User, "do the thing"),
                ContextMessage::assistant_tool_calls(
                    "let me check",
                    vec![ContextToolCall::new("call_0", "read", r#"{"path":"f"}"#)]
                ),
                ContextMessage::tool_result("call_0", "L1"),
                ContextMessage::new(ContextRole::Assistant, "all done"),
            ]
        );
    }

    #[test]
    fn merged_user_entries_concatenate_their_image_attachments() {
        // A batch's messages merge into one user entry; both attachments ride.
        let history = vec![
            HistoryItem::Message(Message {
                role: Role::User,
                text: "[Image #1] first".to_string(),
                timestamp: String::new(),
                images: vec![PathBuf::from("/a.png")],
            }),
            HistoryItem::Message(Message {
                role: Role::User,
                text: "[Image #1] second".to_string(),
                timestamp: String::new(),
                images: vec![PathBuf::from("/b.png")],
            }),
        ];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 1);
        assert_eq!(ctx[0].text, "[Image #1] first\n\n[Image #1] second");
        assert_eq!(
            ctx[0].images,
            vec![PathBuf::from("/a.png"), PathBuf::from("/b.png")]
        );
    }

    #[test]
    fn turn_summaries_are_skipped() {
        let history = vec![HistoryItem::Summary(TurnSummary {
            verb: "Done",
            tokens: 0,
            cached: 0,
            secs: 3,
            timestamp: String::new(),
            shells: 0,
        })];
        assert!(context_messages(&history).is_empty());
    }

    #[test]
    fn a_background_notice_becomes_a_bracketed_note_with_the_output_tail() {
        // The completion notice reaches the model as a user-role note carrying
        // the outcome and the final output tail — that's how the model can
        // summarise the result in the automatic follow-up turn
        // (docs/background.md). The rendered cell shows only the headline.
        let history = vec![HistoryItem::Background(crate::app::BackgroundNotice {
            description: "Ping x.com 200 times".to_string(),
            id: "bash_1".to_string(),
            code: Some(0),
            killed: false,
            output_tail: "64 bytes from x.com\n200 packets transmitted".to_string(),
            timestamp: String::new(),
        })];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 1);
        assert_eq!(ctx[0].role, ContextRole::User);
        assert_eq!(
            ctx[0].text,
            "[background] Background command \"Ping x.com 200 times\" (id bash_1) \
             completed (exit code 0).\nFinal output (tail):\n\
             64 bytes from x.com\n200 packets transmitted"
        );
    }

    #[test]
    fn a_backgrounded_tool_replays_its_model_facing_launch_text() {
        // A backgrounded bash call is still a native tool-call pair — the
        // stored output IS the model-facing launch text (task id + interim
        // file), so no special casing is needed.
        let history = vec![tool(
            "Bash",
            "ping -c 200 x.com",
            "Command running in the background with ID: bash_1.",
            ToolStatus::Backgrounded,
            false,
        )];
        let ctx = context_messages(&history);
        assert_eq!(
            ctx[1],
            ContextMessage::tool_result(
                "call_0",
                "Command running in the background with ID: bash_1."
            )
        );
    }

    #[test]
    fn wire_names_match_the_openai_roles() {
        assert_eq!(ContextRole::System.wire_name(), "system");
        assert_eq!(ContextRole::User.wire_name(), "user");
        assert_eq!(ContextRole::Assistant.wire_name(), "assistant");
        assert_eq!(ContextRole::Tool.wire_name(), "tool");
    }
}
