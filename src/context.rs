//! The per-session LLM conversation context (see `docs/context.md`).
//!
//! A real backend must send the *whole* conversation each turn or the model
//! loses context. This module derives that raw message list from
//! [`App::history`] — the single source of truth the TUI already keeps
//! correct across `/clear`, the Esc-Esc backtrack rewind, an interrupt
//! rollback, and a `/resume` load — so the context can never drift from what
//! the user sees. Every history item type is represented: user and assistant
//! text verbatim (placeholders like `[Image #N]` included), tool calls and
//! `!` shell runs in a bracketed **raw format** that is sent to the model and
//! shown only in the Ctrl+D context-debug view (the inline TUI renders them
//! prettily from `history` instead), and error/system notices as system-role
//! notes. Pure and unit-tested; the derivation runs at the boundary right
//! before each [`ReplySource::spawn`].
//!
//! [`App::history`]: crate::app::App::history
//! [`ReplySource::spawn`]: crate::stream::ReplySource::spawn

use std::path::PathBuf;

use crate::app::{HistoryItem, Role, ToolCall, ToolStatus};

/// The wire role a context message is sent as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextRole {
    /// The provider-side instructions (and TUI notices ride along as
    /// mid-conversation system notes).
    System,
    /// The human: typed messages and `!` shell runs.
    User,
    /// The model: reply text and the tools it ran.
    Assistant,
}

impl ContextRole {
    /// The OpenAI-compatible `role` string.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// One raw message of the context window: the role it is sent as, the raw
/// text (verbatim user/assistant content, or the bracketed tool/shell/notice
/// format), and — for a user message — the temp-file paths of its Ctrl+V
/// image attachments (a vision backend re-encodes and re-sends them each
/// turn; see `docs/image-paste.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMessage {
    pub role: ContextRole,
    pub text: String,
    pub images: Vec<PathBuf>,
}

impl ContextMessage {
    /// A plain (imageless) context message.
    #[must_use]
    pub fn new(role: ContextRole, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            images: Vec::new(),
        }
    }
}

/// The `ok`/`failed` tag a finished tool's raw format carries. Only finished
/// tools reach history, so `Running` (impossible here) reads as failed.
const fn outcome(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Ok => "ok",
        ToolStatus::Running | ToolStatus::Failed => "failed",
    }
}

/// The raw bracketed format of one finished tool call — the form the model
/// sees and the Ctrl+D view shows. A backend tool is something the model ran,
/// so it goes back as an `assistant` entry; a `!` shell command is something
/// the *user* ran locally, so it goes back as a `user` entry.
fn tool_message(tool: &ToolCall) -> ContextMessage {
    let (role, header) = if tool.shell {
        // A shell tool's `name` is the command line itself (args unused).
        (
            ContextRole::User,
            format!("[shell {}] $ {}", outcome(tool.status), tool.name),
        )
    } else {
        (
            ContextRole::Assistant,
            format!(
                "[tool {}({}) {}]",
                tool.name,
                tool.args,
                outcome(tool.status)
            ),
        )
    };
    let mut text = header;
    if !tool.output.is_empty() {
        text.push('\n');
        text.push_str(&tool.output);
    }
    ContextMessage::new(role, text)
}

/// Derive the raw context window from the conversation history, oldest first.
///
/// - [`Role::User`] / [`Role::Assistant`] messages carry their text verbatim
///   (image placeholders included) — a user message also carries its
///   attachment paths.
/// - A [`HistoryItem::Tool`] becomes its raw bracketed format (see
///   [`tool_message`]).
/// - [`Role::Error`] / [`Role::System`] notices become `[error]` / `[system]`
///   system-role notes, so the model knows about interrupts, failures, and
///   slash-command output without mistaking them for instructions.
/// - [`Role::Shell`] header messages are skipped — the shell *tool* recorded
///   with them already carries the command and its output.
/// - [`HistoryItem::Summary`] rows are TUI chrome and are skipped.
#[must_use]
pub fn context_messages(history: &[HistoryItem]) -> Vec<ContextMessage> {
    let mut out = Vec::new();
    for item in history {
        match item {
            HistoryItem::Message(message) => match message.role {
                Role::User => out.push(ContextMessage {
                    role: ContextRole::User,
                    text: message.text.clone(),
                    images: message.images.clone(),
                }),
                Role::Assistant => {
                    out.push(ContextMessage::new(ContextRole::Assistant, &message.text));
                }
                Role::Error => out.push(ContextMessage::new(
                    ContextRole::System,
                    format!("[error] {}", message.text),
                )),
                Role::System => out.push(ContextMessage::new(
                    ContextRole::System,
                    format!("[system] {}", message.text),
                )),
                Role::Shell => {} // its tool cell carries the command + output
            },
            HistoryItem::Tool(tool) => out.push(tool_message(tool)),
            HistoryItem::Summary(_) => {} // TUI chrome, not conversation
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Message, TurnSummary};

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
    fn a_backend_tool_becomes_a_raw_assistant_entry() {
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
            vec![ContextMessage::new(
                ContextRole::Assistant,
                "[tool Read(src/main.rs) ok]\nfn main() {}"
            )]
        );
    }

    #[test]
    fn a_failed_tool_is_tagged_failed() {
        let history = vec![tool(
            "Bash",
            "grep -n TODO",
            "no matches",
            ToolStatus::Failed,
            false,
        )];
        let ctx = context_messages(&history);
        assert_eq!(ctx[0].text, "[tool Bash(grep -n TODO) failed]\nno matches");
    }

    #[test]
    fn a_tool_with_no_output_is_just_the_header() {
        let history = vec![tool("Read", "a.txt", "", ToolStatus::Ok, false)];
        assert_eq!(context_messages(&history)[0].text, "[tool Read(a.txt) ok]");
    }

    #[test]
    fn a_shell_run_becomes_a_raw_user_entry_and_its_header_is_skipped() {
        // begin_shell records the Role::Shell header message *and* the shell
        // tool; only the tool reaches the context (it carries the command).
        let history = vec![
            message(Role::Shell, "pwd"),
            tool("pwd", "", "/home/user", ToolStatus::Ok, true),
        ];
        let ctx = context_messages(&history);
        assert_eq!(
            ctx,
            vec![ContextMessage::new(
                ContextRole::User,
                "[shell ok] $ pwd\n/home/user"
            )]
        );
    }

    #[test]
    fn notices_become_system_role_notes() {
        let history = vec![
            message(Role::Error, "Conversation interrupted"),
            message(Role::System, "help text"),
        ];
        let ctx = context_messages(&history);
        assert_eq!(
            ctx,
            vec![
                ContextMessage::new(ContextRole::System, "[error] Conversation interrupted"),
                ContextMessage::new(ContextRole::System, "[system] help text"),
            ]
        );
    }

    #[test]
    fn turn_summaries_are_skipped() {
        let history = vec![HistoryItem::Summary(TurnSummary {
            verb: "Done",
            secs: 3,
            timestamp: String::new(),
        })];
        assert!(context_messages(&history).is_empty());
    }

    #[test]
    fn wire_names_match_the_openai_roles() {
        assert_eq!(ContextRole::System.wire_name(), "system");
        assert_eq!(ContextRole::User.wire_name(), "user");
        assert_eq!(ContextRole::Assistant.wire_name(), "assistant");
    }
}
