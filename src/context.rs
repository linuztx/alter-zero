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

/// Codex's `/compact` summarization prompt (`prompts/compact_prompt.md`,
/// verbatim): the user message the compact turn sends over the whole current
/// context. See `docs/compact.md`.
pub const SUMMARIZATION_PROMPT: &str = include_str!("../prompts/compact_prompt.md");

/// Codex's summary prefix (`prompts/compact_summary_prefix.md`, verbatim — no
/// trailing newline): the compacted context's bridge message opens with it,
/// followed by a newline and the marker's summary. See `docs/compact.md`.
pub const SUMMARY_PREFIX: &str = include_str!("../prompts/compact_summary_prefix.md");

/// Codex's `COMPACT_USER_MESSAGE_MAX_TOKENS`: the approx-token budget for the
/// recent user messages replayed before the bridge after a `/compact`.
const COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000;

/// Codex's `APPROX_BYTES_PER_TOKEN` — the byte↔token heuristic the budget
/// uses (not the real tokenizer: the walk runs at every turn start and must
/// stay O(len); codex budgets with the same approximation).
const APPROX_BYTES_PER_TOKEN: usize = 4;

/// Codex's `approx_token_count`: bytes divided by four, rounded up.
const fn approx_token_count(text: &str) -> usize {
    text.len().div_ceil(APPROX_BYTES_PER_TOKEN)
}

/// Codex's `truncate_middle_with_token_budget`: fit `text` into `max_tokens`
/// (≈ 4 bytes each) by keeping the head and tail halves — split on char
/// boundaries — around an `…N tokens truncated…` marker. Text already within
/// budget passes through untouched.
fn truncate_middle_to_tokens(text: &str, max_tokens: usize) -> String {
    let max_bytes = max_tokens.saturating_mul(APPROX_BYTES_PER_TOKEN);
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let left_budget = max_bytes / 2;
    let right_budget = max_bytes - left_budget;
    // The widest char-aligned prefix within the left half of the budget…
    let mut prefix_end = 0;
    for (idx, ch) in text.char_indices() {
        let end = idx + ch.len_utf8();
        if end <= left_budget {
            prefix_end = end;
        } else {
            break;
        }
    }
    // …and the widest char-aligned suffix within the right half.
    let tail_target = text.len() - right_budget;
    let mut suffix_start = text.len();
    for (idx, _) in text.char_indices() {
        if idx >= tail_target {
            suffix_start = idx;
            break;
        }
    }
    if suffix_start < prefix_end {
        suffix_start = prefix_end;
    }
    let removed = (text.len() - max_bytes).div_ceil(APPROX_BYTES_PER_TOKEN);
    format!(
        "{}…{removed} tokens truncated…{}",
        &text[..prefix_end],
        &text[suffix_start..]
    )
}

/// The typed user messages of `items` that fit `budget` approx tokens —
/// codex's `build_compacted_history` selection: walk newest→oldest keeping
/// whole messages while they fit, middle-truncate the first overflowing one to
/// the remaining budget, then restore chronological order. Only real
/// [`Role::User`] messages collect (tool records, shell transcripts, notices,
/// and earlier compaction markers all drop — codex's `collect_user_messages`).
fn budgeted_user_texts(items: &[HistoryItem], budget: usize) -> Vec<String> {
    let mut selected = Vec::new();
    let mut remaining = budget;
    for item in items.iter().rev() {
        let HistoryItem::Message(message) = item else {
            continue;
        };
        if message.role != Role::User {
            continue;
        }
        if remaining == 0 {
            break;
        }
        let tokens = approx_token_count(&message.text);
        if tokens <= remaining {
            selected.push(message.text.clone());
            remaining -= tokens;
        } else {
            selected.push(truncate_middle_to_tokens(&message.text, remaining));
            break;
        }
    }
    selected.reverse();
    selected
}

/// The compacted context's bridge message: [`SUMMARY_PREFIX`], a newline, and
/// the marker's summary — codex's `format!("{SUMMARY_PREFIX}\n{summary}")`,
/// with its "(no summary available)" fallback when the model streamed nothing.
fn summary_bridge(summary: &str) -> String {
    let body = if summary.is_empty() {
        "(no summary available)"
    } else {
        summary
    };
    format!("{SUMMARY_PREFIX}\n{body}")
}

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
    context_messages_with(None, history)
}

/// [`context_messages`] with the project's AGENTS.md instructions in front —
/// codex's user-instructions fragment (`project_doc::instructions_message`,
/// rendered at the boundary) leads the derived context as its first **user**
/// entry, in front of the normal derivation *and* the post-`/compact` shape
/// alike (codex keeps its initial context through compaction the same way).
/// It rides `push_text`, so a first user message merges after it under the
/// module's alternation convention. `None` or a blank changes nothing. See
/// `docs/project-doc.md`.
#[must_use]
pub fn context_messages_with(
    user_instructions: Option<&str>,
    history: &[HistoryItem],
) -> Vec<ContextMessage> {
    let mut out: Vec<ContextMessage> = Vec::new();
    if let Some(instructions) = user_instructions
        && !instructions.trim().is_empty()
    {
        push_text(
            &mut out,
            ContextRole::User,
            instructions.to_string(),
            vec![],
        );
    }
    // A `/compact` marker (docs/compact.md): the *last* one wins, and
    // everything before it derives as codex's compacted shape — the budgeted
    // recent user texts, then the summary bridge — with the items after it
    // deriving normally. Earlier markers sit before the last one, so a prior
    // compaction's summary is structurally excluded (only real user texts
    // collect); codex needs an `is_summary_message` prefix check for the same
    // exclusion because its summaries are plain user messages.
    if let Some(cut) = history
        .iter()
        .rposition(|item| matches!(item, HistoryItem::Compaction(_)))
    {
        let HistoryItem::Compaction(compaction) = &history[cut] else {
            unreachable!("rposition matched a Compaction");
        };
        for text in budgeted_user_texts(&history[..cut], COMPACT_USER_MESSAGE_MAX_TOKENS) {
            push_text(&mut out, ContextRole::User, text, vec![]);
        }
        push_text(
            &mut out,
            ContextRole::User,
            summary_bridge(&compaction.summary),
            vec![],
        );
        derive_into(&mut out, &history[cut + 1..]);
    } else {
        derive_into(&mut out, history);
    }
    out
}

/// Derive `history` (a compaction-free run of items) into `out` — the
/// per-item mapping documented on [`context_messages`].
fn derive_into(out: &mut Vec<ContextMessage>, history: &[HistoryItem]) {
    let mut tool_seq = 0usize;
    for (position, item) in history.iter().enumerate() {
        match item {
            // Hook-injected conversation text (docs/hooks.md): the model read
            // it as a user message mid-turn, so every later turn replays it
            // verbatim in place.
            HistoryItem::HookNote(note) => {
                push_text(out, ContextRole::User, note.text.clone(), vec![]);
            }
            HistoryItem::Message(message) => match message.role {
                Role::User => push_text(
                    out,
                    ContextRole::User,
                    message.text.clone(),
                    message.images.clone(),
                ),
                Role::Assistant => {
                    push_text(out, ContextRole::Assistant, message.text.clone(), vec![]);
                }
                Role::Error => push_text(
                    out,
                    ContextRole::User,
                    format!("[error] {}", message.text),
                    vec![],
                ),
                Role::System => push_text(
                    out,
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
                            out,
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
                push_text(out, ContextRole::User, text, vec![]);
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
                // The **model-facing** result, not the cell text: a permission
                // rejection's display is a one-liner while the model read the
                // full stop-and-wait instruction — with Tab's amend feedback
                // appended. Replaying the display would drop what the user
                // asked for from every later turn (`docs/permissions.md`).
                out.push(ContextMessage::tool_result(id, tool.context_text()));
                // An image `read` (docs/tools.md): the live agent loop
                // attached the pixels as a follow-up user message; replay the
                // same shape so later turns keep seeing them. The stored args
                // are the path — the backend re-encodes it each request (a
                // gone file becomes an `[image unavailable]` note there).
                if tool.name == "Read" && is_image_read_output(&tool.output) {
                    push_text(
                        out,
                        ContextRole::User,
                        image_attachment_note(&tool.args),
                        vec![PathBuf::from(&tool.args)],
                    );
                }
            }
            // A task tool call (`docs/task-tools.md`): invisible inline, but
            // the model made the call and read the result — replay the same
            // native pair every other tool gets, so later turns keep its
            // memory of the plan. The stored args are the one-line summary,
            // so like the ask tool the replayed call carries `{}` arguments
            // — the result text below it is what the model reasons from.
            HistoryItem::TaskCall(record) => {
                let id = format!("call_{tool_seq}");
                tool_seq += 1;
                // The arguments the model actually sent, so its own plan
                // stays in the conversation rather than only in the result
                // line (docs/task-tools.md). A record written before the
                // field existed — or one whose arguments didn't parse —
                // replays as `{}`, which is what it always did.
                let arguments = Some(record.arguments.trim())
                    .filter(|a| serde_json::from_str::<serde_json::Value>(a).is_ok())
                    .unwrap_or("{}");
                let call =
                    ContextToolCall::new(id.clone(), wire_tool_name(&record.name), arguments);
                match out.last_mut() {
                    Some(last) if last.role == ContextRole::Assistant => {
                        last.tool_calls.push(call);
                    }
                    _ => out.push(ContextMessage::assistant_tool_calls("", vec![call])),
                }
                out.push(ContextMessage::tool_result(id, record.output.clone()));
            }
            HistoryItem::Summary(_) => {} // TUI chrome, not conversation
            // Unreachable through `context_messages` (it splits at the last
            // marker), but the mapping stays total: a marker inside a plain
            // run contributes nothing itself.
            HistoryItem::Compaction(_) => {}
            // A background shell's completion: a bracketed user-role note
            // carrying the outcome AND the output tail — the model reads the
            // result here (the rendered cell shows only the one-line headline).
            // User-role like the other notices: strict providers reject
            // mid-conversation system messages. See docs/background.md.
            HistoryItem::Background(notice) => {
                push_text(out, ContextRole::User, notice.context_text(), vec![]);
            }
            // A resolved subagent group (`docs/agent-tool.md`): the parent
            // made one `agent` call per entry and received one result — the
            // native tool-call pair replays exactly that: every call folded
            // onto the assistant entry, then the results in call order, each
            // carrying the **immutable** model-facing `output` (the framed
            // response / launch acknowledgement / stopped note the model
            // actually read — never the display fields a later completion
            // updates).
            HistoryItem::AgentGroup(group) => {
                let mut ids = Vec::with_capacity(group.agents.len());
                for entry in &group.agents {
                    let id = format!("call_{tool_seq}");
                    tool_seq += 1;
                    let call =
                        ContextToolCall::new(id.clone(), "agent", agent_arguments(entry, group));
                    match out.last_mut() {
                        Some(last) if last.role == ContextRole::Assistant => {
                            last.tool_calls.push(call);
                        }
                        _ => out.push(ContextMessage::assistant_tool_calls("", vec![call])),
                    }
                    ids.push(id);
                }
                for (id, entry) in ids.into_iter().zip(&group.agents) {
                    out.push(ContextMessage::tool_result(id, entry.output.clone()));
                }
            }
            // A background agent's completion: the bracketed user-role note
            // carrying the outcome and the final response, exactly like a
            // background shell's (docs/agent-tool.md).
            HistoryItem::AgentNotice(notice) => {
                push_text(out, ContextRole::User, notice.context_text(), vec![]);
            }
            // A settled thinking phase is **not** conversation: Chat
            // Completions has nowhere to put a previous round's raw
            // chain-of-thought, and re-sending it would burn context for
            // nothing. The Ctrl+D view showing no trace of it is the truth
            // about what the model receives (docs/thinking-stream.md).
            HistoryItem::Reasoning(_) => {}
        }
    }
}

/// Reconstruct an `agent` call's JSON argument object from its recorded
/// entry — the [`reconstruct_arguments`] twin for subagents (history keeps the
/// typed fields, not the raw argument JSON).
fn agent_arguments(entry: &crate::app::AgentGroupEntry, group: &crate::app::AgentGroup) -> String {
    serde_json::json!({
        "description": entry.description,
        "prompt": entry.prompt,
        "subagent_type": entry.agent_type,
        "run_in_background": group.background,
    })
    .to_string()
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
            context_output: None,
            approval_note: None,
        })
    }

    /// A permission-rejected call: the short red cell text plus the longer
    /// model-facing result the live loop actually sent (`docs/permissions.md`).
    fn rejected_tool(name: &str, args: &str, display: &str, result: &str) -> HistoryItem {
        HistoryItem::Tool(ToolCall {
            name: name.to_string(),
            args: args.to_string(),
            status: ToolStatus::Failed,
            output: display.to_string(),
            timestamp: String::new(),
            shell: false,
            truncated: false,
            context_output: Some(result.to_string()),
            approval_note: None,
        })
    }

    #[test]
    fn empty_history_derives_an_empty_context() {
        assert!(context_messages(&[]).is_empty());
    }

    #[test]
    fn a_hook_note_replays_verbatim_as_a_user_message() {
        // The model read it as a user message mid-turn (docs/hooks.md), so
        // every later turn replays exactly that — no prefix, no wrapper: the
        // producer already formatted the wire text.
        let history = vec![
            message(Role::User, "fix it"),
            message(Role::Assistant, "done"),
            HistoryItem::HookNote(crate::app::HookNote {
                label: "Stop hook".into(),
                text: "Stop hook feedback:\ntests are red".into(),
                timestamp: String::new(),
            }),
            message(Role::Assistant, "fixed for real"),
        ];
        let context = context_messages(&history);
        let roles_texts: Vec<(ContextRole, &str)> =
            context.iter().map(|m| (m.role, m.text.as_str())).collect();
        assert_eq!(
            roles_texts,
            vec![
                (ContextRole::User, "fix it"),
                (ContextRole::Assistant, "done"),
                (ContextRole::User, "Stop hook feedback:\ntests are red"),
                (ContextRole::Assistant, "fixed for real"),
            ]
        );
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
    fn a_task_call_replays_as_the_native_pair_the_model_keeps() {
        // A task tool call is invisible inline (docs/task-tools.md) but the
        // model made the call and read the result — the derived context
        // replays the same native pair every other tool gets, carrying the
        // **arguments it actually sent** so its own plan (subjects,
        // descriptions, the dependency it wired) stays in the conversation
        // rather than only in a result line. Interleaved with text, it folds
        // onto the open assistant segment like any tool call.
        let mut store = crate::tasks::TaskStore::new();
        let arguments = r#"{"subject":"Add tests","description":"d"}"#;
        let output = store.run_create(arguments).unwrap();
        let history = vec![
            message(Role::User, "plan it"),
            message(Role::Assistant, "On it."),
            HistoryItem::TaskCall(crate::app::TaskCallRecord {
                name: "TaskCreate".to_string(),
                args: "Add tests".to_string(),
                arguments: arguments.to_string(),
                output: output.clone(),
                ok: true,
                timestamp: String::new(),
                tasks: store,
            }),
        ];
        let ctx = context_messages(&history);
        assert_eq!(ctx[1].role, ContextRole::Assistant);
        assert_eq!(
            ctx[1].tool_calls,
            vec![ContextToolCall::new("call_0", "taskcreate", arguments)],
            "the call replays with the arguments the model sent"
        );
        assert_eq!(
            ctx[2],
            ContextMessage::tool_result("call_0", "Task #1 created successfully: Add tests")
        );
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
    fn a_rejected_tool_replays_the_model_facing_result_not_the_cell_text() {
        // A permission rejection resolves with TWO texts (`docs/permissions.md`):
        // the short red cell output the user reads, and the longer stop-and-wait
        // instruction the *model* read as the tool result — with Tab's amend
        // feedback appended. The replay must be the latter, or the next turn
        // silently drops what the user asked for.
        let history = vec![rejected_tool(
            "Write",
            "hello.py",
            "User rejected write to hello.py",
            "The user doesn't want to proceed with this tool use.\nThe user provided the \
             following instructions instead: just print it instead",
        )];
        let ctx = context_messages(&history);
        assert_eq!(ctx[1].role, ContextRole::Tool);
        assert_eq!(
            ctx[1].text,
            "The user doesn't want to proceed with this tool use.\nThe user provided the \
             following instructions instead: just print it instead"
        );
    }

    #[test]
    fn an_ordinary_tool_replays_its_own_output() {
        // The split only exists for a rejection: every other call's display
        // text *is* what the model read, so `context_output` stays None.
        let history = vec![tool("Read", "a.txt", "L1", ToolStatus::Ok, false)];
        let ctx = context_messages(&history);
        assert_eq!(ctx[1], ContextMessage::tool_result("call_0", "L1"));
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
    fn thinking_phases_are_skipped() {
        // Chat Completions has nowhere to put a previous round's raw
        // chain-of-thought, and re-sending it would burn context for nothing —
        // so the derived context (and the Ctrl+D view) shows no trace of it.
        // See docs/thinking-stream.md.
        let history = vec![
            HistoryItem::Reasoning(crate::app::Reasoning {
                text: "a long private deliberation".to_string(),
                secs: 65,
                tokens: 1_500,
                timestamp: String::new(),
            }),
            HistoryItem::Message(Message {
                role: Role::Assistant,
                text: "the answer".to_string(),
                timestamp: String::new(),
                images: Vec::new(),
            }),
        ];
        let messages = context_messages(&history);
        assert_eq!(messages.len(), 1, "only the reply is conversation");
        assert_eq!(messages[0].text, "the answer");
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
            origin: None,
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

    // --- /compact: the marker-aware derivation (docs/compact.md) ---

    fn compaction(summary: &str) -> HistoryItem {
        HistoryItem::Compaction(crate::app::Compaction {
            summary: summary.to_string(),
            timestamp: String::new(),
            before: 0,
            after: 0,
            auto: false,
            secs: 0,
        })
    }

    #[test]
    fn approx_token_count_is_bytes_over_four_rounded_up() {
        // Codex's `approx_token_count` (bytes/4, ceiling) — the budget's unit.
        assert_eq!(approx_token_count(""), 0);
        assert_eq!(approx_token_count("abc"), 1);
        assert_eq!(approx_token_count("abcd"), 1);
        assert_eq!(approx_token_count("abcde"), 2);
    }

    #[test]
    fn truncating_to_a_token_budget_keeps_head_and_tail_with_a_marker() {
        // Codex's `truncate_middle_with_token_budget`: half the byte budget from
        // the head, half from the tail, an `…N tokens truncated…` marker between.
        let text = "aaaaaaaaaa..bbbbbbbbbb"; // 22 bytes
        let out = truncate_middle_to_tokens(text, 5); // 20-byte budget
        assert!(out.starts_with("aaaaaaaaaa"), "{out:?}");
        assert!(out.ends_with("bbbbbbbbbb"), "{out:?}");
        assert!(out.contains("tokens truncated…"), "{out:?}");
    }

    #[test]
    fn a_text_within_the_budget_is_untouched() {
        assert_eq!(truncate_middle_to_tokens("short", 20_000), "short");
    }

    #[test]
    fn the_user_message_budget_keeps_the_newest_messages_within_the_cap() {
        // Newest→oldest walk keeping whole messages, re-reversed to
        // chronological order — codex's `build_compacted_history` selection.
        let items = vec![
            message(Role::User, "oldest message dropped"), // over budget
            message(Role::Assistant, "reply"),
            message(Role::User, "abcd"), // 1 token
            message(Role::User, "efgh"), // 1 token
        ];
        assert_eq!(budgeted_user_texts(&items, 2), vec!["abcd", "efgh"]);
    }

    #[test]
    fn an_overflowing_user_message_is_middle_truncated_to_the_remaining_budget() {
        let big = "x".repeat(400);
        let items = vec![
            message(Role::User, &big),
            message(Role::User, "abcd"), // 1 token, leaves 9 of 10
        ];
        let texts = budgeted_user_texts(&items, 10);
        assert_eq!(texts.len(), 2);
        assert!(texts[0].contains("tokens truncated…"), "{:?}", texts[0]);
        assert!(texts[0].len() < big.len());
        assert_eq!(texts[1], "abcd");
    }

    #[test]
    fn only_typed_user_messages_enter_the_budget_walk() {
        // Shell transcripts, notices, tool records, and summaries all drop —
        // codex collects only real user messages.
        let items = vec![
            message(Role::User, "keep me"),
            message(Role::Assistant, "reply"),
            message(Role::Error, "bang"),
            message(Role::System, "notice"),
            message(Role::Shell, "pwd"),
            tool("pwd", "", "/home", ToolStatus::Ok, true),
            tool("Read", "f", "L1", ToolStatus::Ok, false),
        ];
        assert_eq!(budgeted_user_texts(&items, 20_000), vec!["keep me"]);
    }

    #[test]
    fn a_compaction_marker_derives_the_bridge_in_place_of_the_prior_items() {
        // Everything before the marker collapses to the budgeted user texts +
        // the SUMMARY_PREFIX bridge (one merged user entry — our wire shape);
        // the tool record and the reply drop entirely.
        let history = vec![
            message(Role::User, "do the thing"),
            message(Role::Assistant, "let me check"),
            tool("Read", "f", "L1", ToolStatus::Ok, false),
            message(Role::Assistant, "all done"),
            compaction("we did the thing"),
        ];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 1, "{ctx:?}");
        assert_eq!(ctx[0].role, ContextRole::User);
        assert_eq!(
            ctx[0].text,
            format!("do the thing\n\n{SUMMARY_PREFIX}\nwe did the thing")
        );
        assert!(ctx[0].tool_calls.is_empty() && ctx[0].images.is_empty());
    }

    #[test]
    fn post_marker_items_derive_normally_after_the_bridge() {
        let history = vec![
            message(Role::User, "old question"),
            message(Role::Assistant, "old answer"),
            compaction("summary"),
            message(Role::User, "new question"),
            message(Role::Assistant, "new answer"),
            tool("Bash", "ls", "files", ToolStatus::Ok, false),
        ];
        let ctx = context_messages(&history);
        // [merged user: old question + bridge + new question], assistant with
        // the tool call, tool result.
        assert_eq!(ctx.len(), 3, "{ctx:?}");
        assert!(ctx[0].text.starts_with("old question\n\n"));
        assert!(
            ctx[0].text.ends_with("\n\nnew question"),
            "{:?}",
            ctx[0].text
        );
        assert_eq!(ctx[1].role, ContextRole::Assistant);
        assert_eq!(ctx[1].text, "new answer");
        assert_eq!(
            ctx[1].tool_calls,
            vec![ContextToolCall::new(
                "call_0",
                "bash",
                r#"{"command":"ls"}"#
            )]
        );
        assert_eq!(ctx[2], ContextMessage::tool_result("call_0", "files"));
    }

    #[test]
    fn an_empty_summary_bridges_as_no_summary_available() {
        // Codex's fallback when the model streamed nothing.
        let history = vec![message(Role::User, "hi"), compaction("")];
        let ctx = context_messages(&history);
        assert!(
            ctx[0]
                .text
                .ends_with(&format!("{SUMMARY_PREFIX}\n(no summary available)")),
            "{:?}",
            ctx[0].text
        );
    }

    #[test]
    fn only_the_last_marker_counts_and_prior_marker_summaries_are_excluded() {
        // A second /compact: the first marker (and its summary) sits before the
        // last one and is structurally excluded — only real user texts collect.
        let history = vec![
            message(Role::User, "first question"),
            compaction("first summary"),
            message(Role::User, "second question"),
            compaction("second summary"),
        ];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 1, "{ctx:?}");
        assert!(!ctx[0].text.contains("first summary"), "{:?}", ctx[0].text);
        assert!(ctx[0].text.contains("first question"));
        assert!(ctx[0].text.contains("second question"));
        assert!(
            ctx[0].text.ends_with("\nsecond summary"),
            "{:?}",
            ctx[0].text
        );
    }

    #[test]
    fn pre_marker_user_images_do_not_ride_the_compacted_context() {
        // Codex re-emits retained user messages as plain text — attachments
        // drop from the model's view (the paths stay owned by history).
        let history = vec![
            HistoryItem::Message(Message {
                role: Role::User,
                text: "[Image #1] look".to_string(),
                timestamp: String::new(),
                images: vec![PathBuf::from("/tmp/shot.png")],
            }),
            compaction("saw it"),
        ];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 1);
        assert!(ctx[0].images.is_empty(), "{:?}", ctx[0].images);
        assert!(ctx[0].text.starts_with("[Image #1] look"));
    }

    #[test]
    fn a_marker_first_in_history_derives_just_the_bridge() {
        let history = vec![compaction("from nothing")];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 1);
        assert_eq!(ctx[0].text, format!("{SUMMARY_PREFIX}\nfrom nothing"));
    }

    #[test]
    fn the_summarization_prompt_and_prefix_carry_codexs_text() {
        assert!(
            SUMMARIZATION_PROMPT.starts_with("You are performing a CONTEXT CHECKPOINT COMPACTION")
        );
        assert!(SUMMARY_PREFIX.starts_with("Another language model started to solve"));
        assert!(
            !SUMMARY_PREFIX.ends_with('\n'),
            "the prefix has no trailing newline — the bridge adds the separator"
        );
    }

    // --- AGENTS.md user instructions (docs/project-doc.md) ---

    #[test]
    fn user_instructions_lead_the_derived_context() {
        // The rendered AGENTS.md fragment is the context's first user entry;
        // a first user message merges after it under the module's alternation
        // convention (push_text), the markers keeping the boundary clear.
        let history = vec![message(Role::User, "hello"), message(Role::Assistant, "hi")];
        let ctx =
            context_messages_with(Some("<INSTRUCTIONS>\nUse TDD.\n</INSTRUCTIONS>"), &history);
        assert_eq!(ctx.len(), 2, "{ctx:?}");
        assert_eq!(ctx[0].role, ContextRole::User);
        assert_eq!(
            ctx[0].text,
            "<INSTRUCTIONS>\nUse TDD.\n</INSTRUCTIONS>\n\nhello"
        );
        assert_eq!(ctx[1].text, "hi");
    }

    #[test]
    fn user_instructions_alone_are_a_context_of_one() {
        let ctx = context_messages_with(Some("guide"), &[]);
        assert_eq!(ctx, vec![ContextMessage::new(ContextRole::User, "guide")]);
    }

    #[test]
    fn none_or_blank_instructions_leave_the_derivation_untouched() {
        let history = vec![message(Role::User, "hello")];
        assert_eq!(
            context_messages_with(None, &history),
            context_messages(&history)
        );
        assert_eq!(
            context_messages_with(Some("   "), &history),
            context_messages(&history)
        );
    }

    #[test]
    fn user_instructions_survive_compaction_at_the_front() {
        // Codex keeps its initial context (the instructions among it) through
        // a compaction; ours re-derives, so the fragment leads the compacted
        // shape too — before the budgeted user texts and the bridge. The
        // budget walk reads *history*, which the instructions never enter, so
        // they are never re-summarized.
        let history = vec![
            message(Role::User, "old question"),
            message(Role::Assistant, "old answer"),
            compaction("the summary"),
            message(Role::User, "new question"),
        ];
        let ctx = context_messages_with(Some("guide"), &history);
        assert_eq!(ctx.len(), 1, "{ctx:?}");
        assert!(
            ctx[0].text.starts_with("guide\n\nold question"),
            "{:?}",
            ctx[0].text
        );
        assert!(ctx[0].text.contains(SUMMARY_PREFIX));
        assert!(ctx[0].text.ends_with("new question"));
    }

    #[test]
    fn an_agent_group_replays_native_agent_calls_and_results() {
        use crate::agents::AgentStatus;
        let entry = |id: &str, desc: &str, output: &str| crate::app::AgentGroupEntry {
            id: id.to_string(),
            description: desc.to_string(),
            agent_type: "general-purpose".to_string(),
            prompt: format!("weather in {desc}?"),
            status: AgentStatus::Done,
            tool_uses: 1,
            tokens: 100,
            secs: 5,
            result: "later display update".to_string(),
            tool_headers: vec![],
            output: output.to_string(),
        };
        let history = vec![
            HistoryItem::Message(Message {
                role: Role::User,
                text: "check both".into(),
                timestamp: String::new(),
                images: vec![],
            }),
            HistoryItem::AgentGroup(crate::app::AgentGroup {
                background: false,
                agents: vec![entry("a1", "Warsaw", "19°C"), entry("a2", "Manila", "28°C")],
                timestamp: String::new(),
            }),
        ];
        let messages = context_messages(&history);
        assert_eq!(messages.len(), 4, "user, assistant calls, two results");
        assert_eq!(messages[1].role, ContextRole::Assistant);
        assert_eq!(messages[1].tool_calls.len(), 2);
        assert_eq!(messages[1].tool_calls[0].name, "agent");
        let args: serde_json::Value =
            serde_json::from_str(&messages[1].tool_calls[0].arguments).unwrap();
        assert_eq!(args["description"], "Warsaw");
        assert_eq!(args["prompt"], "weather in Warsaw?");
        assert_eq!(args["run_in_background"], false);
        // Each result answers its call — carrying the immutable wire output,
        // never the display fields a later completion updates.
        assert_eq!(messages[2].role, ContextRole::Tool);
        assert_eq!(
            messages[2].tool_call_id,
            messages[1].tool_calls[0].id.clone().into()
        );
        assert_eq!(messages[2].text, "19°C");
        assert_eq!(messages[3].text, "28°C");
    }

    #[test]
    fn an_agent_notice_replays_as_a_bracketed_user_note() {
        use crate::agents::AgentStatus;
        let history = vec![HistoryItem::AgentNotice(crate::app::AgentNotice {
            id: "a1".into(),
            description: "Fetch Warsaw".into(),
            status: AgentStatus::Done,
            secs: 35,
            result: "19°C and sunny".into(),
            timestamp: String::new(),
        })];
        let messages = context_messages(&history);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, ContextRole::User);
        assert!(
            messages[0]
                .text
                .starts_with("[background agent] Agent \"Fetch Warsaw\"")
        );
        assert!(messages[0].text.contains("19°C and sunny"));
    }
}
