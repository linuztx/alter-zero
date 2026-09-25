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
pub(crate) const MERGE_SEPARATOR: &str = "\n\n";

/// The wire tool name (lowercase, as declared to the provider) for a finished
/// tool's display `name` (title-cased by `llm::tools::display_name`). The four
/// known tools reverse exactly; anything else falls back to a lowercase of the
/// display name.
fn wire_tool_name(display: &str) -> String {
    // An MCP cell's display name (`{server} - {tool} (MCP)`) inverts to the
    // `mcp__server__tool` the model actually called (`docs/mcp.md`).
    if let Some(wire) = crate::mcp::wire_from_display(display) {
        return wire;
    }
    match display {
        "Bash" => "bash".to_string(),
        "Read" => "read".to_string(),
        "Write" => "write".to_string(),
        "Edit" => "edit".to_string(),
        // The one wire name with an underscore, so the lowercase fallback
        // would miss it (docs/interactive-shell.md).
        crate::llm::tools::BASH_SESSION_TOOL_DISPLAY => {
            crate::llm::tools::BASH_SESSION_TOOL_NAME.to_string()
        }
        other => other.to_ascii_lowercase(),
    }
}

/// The JSON argument object a finished tool call replays with — the model's
/// **verbatim** [`ToolCall::arguments`] whenever it recorded them, which is
/// every live call now (`docs/context.md`). Only a record without them — a
/// rollout written before the field, the `!` shell, a hand-scripted event —
/// falls back to rebuilding one from the stored one-line summary
/// (`llm::tools::summarize_call`: the command for `bash`, the path for the
/// file tools), which is lossy; an unrecognised tool then replays with empty
/// arguments.
fn reconstruct_arguments(tool: &ToolCall) -> String {
    // The model's own arguments, when the call recorded them: nothing to
    // reconstruct, and nothing lost — a `write`'s whole `content`, an
    // `edit`'s two strings, a `bash` call's `timeout` all replay as sent.
    // This is what lets those tools' results collapse to one line: the
    // conversation carries the change on the *call* now, not in the result
    // (`docs/tools.md`). Guarded on parsing as an object, since a validating
    // provider rejects anything else — a damaged record falls through to the
    // per-tool table below, which is also what every pre-field rollout, the
    // `!` shell, and a hand-scripted event take.
    if let Some(arguments) = &tool.arguments
        && serde_json::from_str::<serde_json::Value>(arguments.trim()).is_ok_and(|v| v.is_object())
    {
        return arguments.trim().to_string();
    }
    // An MCP call's stored `args` **is** the raw arguments JSON
    // (`llm::tools::summarize_call` keeps it verbatim precisely so this
    // replay is lossless — the pretty `key: value` form is derived at render
    // time instead; `docs/mcp.md`). Guard on it parsing as an object so an
    // odd record still degrades to `{}` rather than an invalid request.
    if crate::mcp::is_mcp_display_name(&tool.name) {
        if serde_json::from_str::<serde_json::Value>(tool.args.trim()).is_ok_and(|v| v.is_object())
        {
            return tool.args.trim().to_string();
        }
        return "{}".to_string();
    }
    // The legacy table: what a record with no arguments of its own can be
    // rebuilt into from the one-line summary. Lossy by construction — it is
    // why the arguments are recorded now — but it keeps a pre-field rollout
    // replaying exactly as it always did.
    let key = match tool.name.as_str() {
        "Bash" => "command",
        "Read" | "Write" | "Edit" => "path",
        // A `skill` call's summary *is* the skill name (`docs/skills.md`), so
        // it reconstructs exactly — and it must, since `skill` is a required
        // parameter a validating provider rejects the call without. The
        // optional `args` are lossy the same way `bash`'s `timeout` is;
        // the result below carries the body they already rendered into.
        crate::skills::SKILL_TOOL_DISPLAY => "skill",
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

/// Whether `history` derives any conversation at all — i.e. whether
/// [`context_messages`] would return anything — without building it. The
/// gates that only need the yes/no (the context gauge's zero state, the
/// auto-compact "anything to summarize" check, `/compact`'s `Nothing to
/// compact` rejection) used to derive the whole window — every message text,
/// tool output, and shell transcript cloned into fresh `String`s — just to
/// test `.is_empty()`; on a long conversation that is hundreds of kilobytes
/// of transient allocation per check, and `should_auto_compact` runs at the
/// loop bottom. The match is deliberately exhaustive (no wildcard): adding a
/// history variant forces this answer to be decided alongside its
/// [`context_messages`] arm, and the equivalence test sweeps every kind so
/// the two can never quietly disagree.
#[must_use]
pub fn derives_conversation(history: &[HistoryItem]) -> bool {
    history.iter().any(|item| match item {
        // A `Role::Shell` header derives either itself (dangling) or through
        // the shell tool right after it — a Message always means conversation.
        HistoryItem::Message(_)
        | HistoryItem::Tool(_)
        | HistoryItem::TaskCall(_)
        | HistoryItem::Background(_)
        | HistoryItem::AgentNotice(_)
        | HistoryItem::HookNote(_)
        // The compacted shape always pushes the summary bridge, even over an
        // empty summary ("(no summary available)").
        | HistoryItem::Compaction(_) => true,
        // One `agent` call per entry — an entryless group derives nothing.
        HistoryItem::AgentGroup(group) => !group.agents.is_empty(),
        // TUI chrome / private chain-of-thought — never conversation.
        HistoryItem::Summary(_) | HistoryItem::Reasoning(_) => false,
    })
}

/// [`context_messages`] behind the session's **`<system-reminder>`**
/// ([`crate::reminder`]) — one block, composed here from its two inputs in
/// their fixed order: the project's AGENTS.md instructions section
/// (`project_doc::instructions_section`, `docs/project-doc.md`), then the
/// listing sections naming the skills the `Skill` tool can load and the
/// types the `Agent` tool can launch (`subagents::listing_sections`,
/// `docs/skills.md`, `docs/subagents.md`).
///
/// Composed at derivation rather than stored assembled because the two
/// inputs change at different moments — the instructions are re-read at every
/// turn start and dropped by a `/settings` toggle, the listings re-rendered on
/// every rescan and `/model` switch — so a block assembled at either site
/// would be stale at the other; built here, the request, Ctrl+D and the token
/// estimate can never disagree about it. The order is a prompt-cache
/// decision: every section is re-rendered per turn, and one that moved would
/// invalidate every token behind it. `None` or a blank on either side leaves
/// that section out; both absent is no block at all, which is what a session
/// with neither sends.
#[must_use]
pub fn context_messages_full(
    user_instructions: Option<&str>,
    listings: Option<&str>,
    history: &[HistoryItem],
) -> Vec<ContextMessage> {
    let reminder = crate::reminder::reminder_message(&[
        user_instructions.unwrap_or_default(),
        listings.unwrap_or_default(),
    ]);
    context_messages_led_by((!reminder.is_empty()).then_some(reminder), history)
}

/// [`context_messages`] behind an already-rendered leading fragment: the
/// **user** entry the window opens with, verbatim, in front of the normal
/// derivation *and* the post-`/compact` shape alike (codex keeps its initial
/// context through compaction the same way). It rides `push_text`, so a first
/// user message merges after it under the module's alternation convention.
/// `None` or a blank changes nothing.
///
/// The main window's fragment is composed by [`context_messages_full`]; a
/// viewed subagent's is its briefing exactly as the launch wrapped it
/// (`skills::listing_message`, `App::agent_system_reminder`), which is why
/// this seam takes the block whole rather than its sections — wrapping it
/// again would show a block the agent never read (`docs/subagents.md`).
#[must_use]
pub fn context_messages_behind(
    leading: Option<&str>,
    history: &[HistoryItem],
) -> Vec<ContextMessage> {
    context_messages_led_by(leading.map(str::to_string), history)
}

/// The shared tail of the two leading-fragment derivations: the fragment (if
/// it says anything) as the first user entry, then `history` behind it.
fn context_messages_led_by(
    leading: Option<String>,
    history: &[HistoryItem],
) -> Vec<ContextMessage> {
    let mut out: Vec<ContextMessage> = Vec::new();
    if let Some(fragment) = leading.filter(|fragment| !fragment.trim().is_empty()) {
        push_text(&mut out, ContextRole::User, fragment, vec![]);
    }
    derive_history_into(&mut out, history);
    out
}

/// [`context_messages_full`] with the project's AGENTS.md instructions alone
/// — the reminder's instructions section and no listings. What the
/// tools-free `/compact` turn sends: a summarizer never offered the `skill`
/// or `agent` tool must not read a roster naming them (`docs/compact.md`).
/// See `docs/project-doc.md`.
#[must_use]
pub fn context_messages_with(
    user_instructions: Option<&str>,
    history: &[HistoryItem],
) -> Vec<ContextMessage> {
    context_messages_full(user_instructions, None, history)
}

/// Derive `history` into `out` behind whatever leading fragments it already
/// holds — the compaction split, then the per-item mapping.
fn derive_history_into(out: &mut Vec<ContextMessage>, history: &[HistoryItem]) {
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
            push_text(out, ContextRole::User, text, vec![]);
        }
        push_text(
            out,
            ContextRole::User,
            summary_bridge(&compaction.summary),
            vec![],
        );
        derive_into(out, &history[cut + 1..]);
    } else {
        derive_into(out, history);
    }
}

/// Derive `history` (a compaction-free run of items) into `out` — the
/// per-item mapping documented on [`context_messages`], with the call-bearing
/// items walked **a round at a time**: the records of one tool round (a
/// backend tool, a task call, an agent group — whatever shares the round's
/// `batch` id) replay as the wire carried them, one assistant message holding
/// every call under the provider's own ids, then every result, so the request
/// re-sends the prefix the provider cached rather than one call per message
/// under synthesized ids (`docs/prompt-caching.md`). A record with no batch
/// is a round of its own, exactly as before the ids were recorded.
fn derive_into(out: &mut Vec<ContextMessage>, history: &[HistoryItem]) {
    let mut ids = CallIds::default();
    let mut position = 0;
    while position < history.len() {
        if let Some(round) = round_at(history, position) {
            derive_round(out, &round, &mut ids);
            position = round.end;
            continue;
        }
        derive_item(out, history, position);
        position += 1;
    }
}

/// The call ids a derivation hands out: a record's own **recorded** id
/// (the provider's, `ToolCall::call_id`) when it is still free, else a
/// synthesized `call_N` — dense from `call_0` for a history recorded before
/// the ids were, so an old rollout replays exactly as it always did — and
/// never one already used, since a provider pairs results by id.
#[derive(Default)]
struct CallIds {
    used: std::collections::HashSet<String>,
    next: usize,
}

impl CallIds {
    fn claim(&mut self, recorded: Option<&str>) -> String {
        if let Some(id) = recorded.filter(|id| !id.is_empty() && !self.used.contains(*id)) {
            self.used.insert(id.to_string());
            return id.to_string();
        }
        loop {
            let candidate = format!("call_{}", self.next);
            self.next += 1;
            if self.used.insert(candidate.clone()) {
                return candidate;
            }
        }
    }
}

/// One tool round's records — the consecutive call-bearing items sharing a
/// `batch` id — plus the completion notices that committed *between* them
/// (a tool resolution is a safe boundary for a background cell,
/// `docs/background.md`), which the model actually read at the next round's
/// top and so replay after the round. `end` is the index past the round.
struct Round<'a> {
    calls: Vec<&'a HistoryItem>,
    deferred: Vec<&'a HistoryItem>,
    end: usize,
}

/// The batch id a call-bearing item carries; `None` for anything else — and
/// for a call recorded before the ids were, which is then a round of its own.
fn batch_of(item: &HistoryItem) -> Option<u64> {
    match item {
        HistoryItem::Tool(tool) if !tool.shell => tool.batch,
        HistoryItem::TaskCall(record) => record.batch,
        HistoryItem::AgentGroup(group) => group.batch,
        _ => None,
    }
}

fn is_call_bearing(item: &HistoryItem) -> bool {
    matches!(
        item,
        HistoryItem::Tool(tool) if !tool.shell
    ) || matches!(item, HistoryItem::TaskCall(_) | HistoryItem::AgentGroup(_))
}

fn is_deferrable_notice(item: &HistoryItem) -> bool {
    matches!(
        item,
        HistoryItem::Background(_) | HistoryItem::AgentNotice(_)
    )
}

/// The round starting at `position`, when the item there bears calls.
fn round_at(history: &[HistoryItem], position: usize) -> Option<Round<'_>> {
    let first = history.get(position)?;
    if !is_call_bearing(first) {
        return None;
    }
    let mut round = Round {
        calls: vec![first],
        deferred: Vec::new(),
        end: position + 1,
    };
    let Some(batch) = batch_of(first) else {
        return Some(round);
    };
    loop {
        let mut next = round.end;
        while history.get(next).is_some_and(is_deferrable_notice) {
            next += 1;
        }
        match history.get(next) {
            Some(item) if is_call_bearing(item) && batch_of(item) == Some(batch) => {
                round.deferred.extend(&history[round.end..next]);
                round.calls.push(item);
                round.end = next + 1;
            }
            _ => return Some(round),
        }
    }
}

/// Replay one round the way the wire carried it: its calls on one assistant
/// message (folded onto the open assistant text segment when there is one,
/// else a fresh assistant message with empty content), every result in call
/// order, the image reads' attachment notes after the results — the live loop
/// appends them after a round's results too (`docs/tools.md`) — and last the
/// notices that committed inside the round.
fn derive_round(out: &mut Vec<ContextMessage>, round: &Round<'_>, ids: &mut CallIds) {
    let mut entries = Vec::new();
    for item in &round.calls {
        record_calls(item, ids, &mut entries);
    }
    // The wire's order is the model's, and the records say where each call
    // sat (`ToolCall::position`); the recorded order stands where they
    // don't — a batch no round announced, a rollout written before the
    // field — and a positioned call sorts ahead of one without, which an
    // announced round never mixes. Stable, so ties keep the recorded order.
    entries.sort_by_key(|entry| entry.position.map_or((1, 0), |position| (0, position)));
    let mut calls = Vec::with_capacity(entries.len());
    let mut results = Vec::with_capacity(entries.len());
    let mut attachments = Vec::new();
    for entry in entries {
        calls.push(entry.call);
        results.push(entry.result);
        attachments.extend(entry.attachment);
    }
    if !calls.is_empty() {
        match out.last_mut() {
            Some(last) if last.role == ContextRole::Assistant && last.tool_call_id.is_none() => {
                last.tool_calls.extend(calls);
            }
            _ => out.push(ContextMessage::assistant_tool_calls("", calls)),
        }
        out.extend(results);
    }
    for (note, path) in attachments {
        push_text(out, ContextRole::User, note, vec![path]);
    }
    for item in &round.deferred {
        match item {
            HistoryItem::Background(notice) => {
                push_text(out, ContextRole::User, notice.context_text(), vec![]);
            }
            HistoryItem::AgentNotice(notice) => {
                push_text(out, ContextRole::User, notice.context_text(), vec![]);
            }
            _ => {}
        }
    }
}

/// One call of a round as the wire carries it — the call, its result, an
/// image read's attachment note — with the index the model gave it, when
/// the record knows it.
struct RoundEntry {
    position: Option<usize>,
    call: ContextToolCall,
    result: ContextMessage,
    attachment: Option<(String, PathBuf)>,
}

/// One record's calls and results, in the order the record holds them.
fn record_calls(item: &HistoryItem, ids: &mut CallIds, entries: &mut Vec<RoundEntry>) {
    match item {
        HistoryItem::Tool(tool) => {
            let id = ids.claim(tool.call_id.as_deref());
            // A legacy `bash_session` call replays as the tool that does the
            // same thing now, so the request names only tools it offers
            // (`docs/bash-tools.md`).
            let (name, arguments) = if tool.name == crate::llm::tools::BASH_SESSION_TOOL_DISPLAY {
                let (name, arguments) =
                    crate::llm::tools::legacy_session_call(&reconstruct_arguments(tool));
                (name.to_string(), arguments)
            } else {
                (wire_tool_name(&tool.name), reconstruct_arguments(tool))
            };
            let call = ContextToolCall::new(id.clone(), name, arguments);
            // The **model-facing** result, not the cell text: a permission
            // rejection's display is a one-liner while the model read the
            // full stop-and-wait instruction — with Tab's amend feedback
            // appended. Replaying the display would drop what the user
            // asked for from every later turn (`docs/permissions.md`).
            let result = ContextMessage::tool_result(id, tool.context_text());
            // An image `read` (docs/tools.md): the live agent loop attached
            // the pixels as a follow-up user message; replay the same shape
            // so later turns keep seeing them. The stored args are the path
            // — the backend re-encodes it each request (a gone file becomes
            // an `[image unavailable]` note there).
            let attachment = (tool.name == "Read" && is_image_read_output(&tool.output))
                .then(|| (image_attachment_note(&tool.args), PathBuf::from(&tool.args)));
            entries.push(RoundEntry {
                position: tool.position,
                call,
                result,
                attachment,
            });
        }
        // A task tool call (`docs/task-tools.md`): invisible inline, but the
        // model made the call and read the result — replay the same native
        // pair every other tool gets, so later turns keep its memory of the
        // plan — carrying the model's own arguments, the same way an
        // ordinary tool record now does. A record written before the field
        // existed — or one whose arguments didn't parse — replays as `{}`,
        // which is what it always did.
        HistoryItem::TaskCall(record) => {
            let id = ids.claim(record.call_id.as_deref());
            let arguments = Some(record.arguments.trim())
                .filter(|a| serde_json::from_str::<serde_json::Value>(a).is_ok())
                .unwrap_or("{}");
            entries.push(RoundEntry {
                position: record.position,
                call: ContextToolCall::new(id.clone(), wire_tool_name(&record.name), arguments),
                result: ContextMessage::tool_result(id, record.output.clone()),
                attachment: None,
            });
        }
        // A resolved subagent group (`docs/agent-tool.md`): the parent made
        // one `agent` call per entry and received one result — the native
        // tool-call pair replays exactly that, each result carrying the
        // **immutable** model-facing `output` (the framed response / launch
        // acknowledgement / stopped note the model actually read — never
        // the display fields a later completion updates).
        HistoryItem::AgentGroup(group) => {
            for agent in &group.agents {
                let id = ids.claim(agent.call_id.as_deref());
                entries.push(RoundEntry {
                    position: agent.position,
                    call: ContextToolCall::new(id.clone(), "agent", agent_arguments(agent, group)),
                    result: ContextMessage::tool_result(id, agent.output.clone()),
                    attachment: None,
                });
            }
        }
        _ => {}
    }
}

/// The per-item mapping for everything that bears no calls (see
/// [`context_messages`]); `position` is the item's index, which the
/// `Role::Shell` header needs to see whether its tool cell follows.
fn derive_item(out: &mut Vec<ContextMessage>, history: &[HistoryItem], position: usize) {
    match &history[position] {
        // Hook-injected conversation text (docs/hooks.md): the model read
        // it as a user message mid-turn, so every later turn replays it
        // verbatim in place.
        HistoryItem::HookNote(note) => {
            push_text(out, ContextRole::User, note.text.clone(), vec![]);
        }
        HistoryItem::Message(message) => match message.role {
            // A pasted picture's placeholder names the file it was saved
            // at *here*, not in the request builder: the derived context
            // is what the model reads, so annotating any lower would
            // leave Ctrl+D showing text the wire never carried
            // (`docs/image-paste.md`). Per message, before the merge, so
            // each draft's `[Image #1]` names its own picture.
            Role::User => push_text(
                out,
                ContextRole::User,
                crate::paste::annotate_image_placeholders(&message.text, &message.images),
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
        // The user's local `!` command: a natural shell transcript under
        // the user role (no native `tool` message — the model didn't
        // call it). A backend tool never reaches here: `round_at` takes
        // every call-bearing item.
        HistoryItem::Tool(tool) => {
            let mut text = format!("$ {}", tool.name);
            if !tool.output.is_empty() {
                text.push('\n');
                text.push_str(&tool.output);
            }
            push_text(out, ContextRole::User, text, vec![]);
        }
        HistoryItem::TaskCall(_) | HistoryItem::AgentGroup(_) => {
            unreachable!("call-bearing items are derived a round at a time")
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

/// Reconstruct an `agent` call's JSON argument object from its recorded
/// entry — the [`reconstruct_arguments`] twin for subagents (history keeps the
/// typed fields, not the raw argument JSON).
fn agent_arguments(entry: &crate::app::AgentGroupEntry, group: &crate::app::AgentGroup) -> String {
    // The model's own arguments when the launch recorded them (the round
    // announced its calls), so the replay is what the provider saw byte for
    // byte; the rebuilt object below is what a record made before the field
    // existed replays as. Guarded on parsing as an object, like every other
    // recorded argument string.
    if let Some(arguments) = &entry.arguments
        && serde_json::from_str::<serde_json::Value>(arguments.trim()).is_ok_and(|v| v.is_object())
    {
        return arguments.trim().to_string();
    }
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
            arguments: None,
            approval_note: None,
            batch: None,
            call_id: None,
            position: None,
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
            arguments: None,
            approval_note: None,
            batch: None,
            call_id: None,
            position: None,
        })
    }

    #[test]
    fn empty_history_derives_an_empty_context() {
        assert!(context_messages(&[]).is_empty());
    }

    #[test]
    fn a_message_steered_mid_turn_replays_between_the_rounds_it_landed_between() {
        // Steering makes a history shape nothing else does (`docs/queue.md`):
        // a user entry between one round's tool result and the next round's
        // call. The derivation must keep that order **and** open a fresh
        // assistant message for the call after it — folding that call onto the
        // assistant segment before the user entry would put a tool result
        // after an intervening user message, which strict providers reject.
        let history = vec![
            message(Role::User, "run the tests"),
            message(Role::Assistant, "running them"),
            tool("Bash", "cargo test", "Exit code: 0", ToolStatus::Ok, false),
            message(Role::User, "also check clippy"),
            tool(
                "Bash",
                "cargo clippy",
                "Exit code: 0",
                ToolStatus::Ok,
                false,
            ),
        ];
        let out = context_messages(&history);
        assert_eq!(
            out.iter().map(|m| m.role).collect::<Vec<_>>(),
            vec![
                ContextRole::User,
                ContextRole::Assistant,
                ContextRole::Tool,
                ContextRole::User,
                ContextRole::Assistant,
                ContextRole::Tool,
            ],
            "the steered message sits between the two rounds: {out:?}"
        );
        assert_eq!(out[3].text, "also check clippy");
        assert_eq!(
            out[1].tool_calls.len(),
            1,
            "the first round's call stayed on its own assistant message"
        );
        assert_eq!(
            out[4].tool_calls.len(),
            1,
            "…and the next round's call opened a fresh one"
        );
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
        // The derived context *is* the wire text, so the placeholder names
        // the file the picture was saved at here — not one layer lower in
        // the request builder, where Ctrl+D could not see it
        // (`docs/image-paste.md`).
        assert_eq!(ctx[0].text, "[Image #1: /tmp/shot.png] what is this?");
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
                call_id: None,
                position: None,
                batch: None,
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

    /// A finished backend tool stamped with the wire identity its round
    /// announced — the provider's call id and the round's batch
    /// (`docs/prompt-caching.md`).
    fn batched(name: &str, args: &str, output: &str, batch: u64, call_id: &str) -> HistoryItem {
        let HistoryItem::Tool(mut call) = tool(name, args, output, ToolStatus::Ok, false) else {
            unreachable!("tool() builds a tool item");
        };
        call.batch = Some(batch);
        call.call_id = Some(call_id.to_string());
        HistoryItem::Tool(call)
    }

    #[test]
    fn a_batch_replays_as_one_assistant_message_carrying_its_recorded_ids() {
        // A parallel batch is one assistant message on the wire, its calls
        // under the provider's own ids; the records carry both, so the
        // derived context sends the provider the prefix it already cached
        // rather than one call per message under `call_N` ids.
        let history = vec![
            message(Role::User, "read both"),
            message(Role::Assistant, "Reading both."),
            batched("Read", "a.rs", "source A", 1, "call_provider_a"),
            batched("Read", "b.rs", "source B", 1, "call_provider_b"),
            message(Role::Assistant, "Both fine."),
        ];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 5, "{ctx:?}");
        assert_eq!(ctx[1].role, ContextRole::Assistant);
        assert_eq!(ctx[1].text, "Reading both.");
        assert_eq!(
            ctx[1].tool_calls,
            vec![
                ContextToolCall::new("call_provider_a", "read", r#"{"path":"a.rs"}"#),
                ContextToolCall::new("call_provider_b", "read", r#"{"path":"b.rs"}"#),
            ]
        );
        assert_eq!(
            ctx[2],
            ContextMessage::tool_result("call_provider_a", "source A")
        );
        assert_eq!(
            ctx[3],
            ContextMessage::tool_result("call_provider_b", "source B")
        );
        assert_eq!(ctx[4].text, "Both fine.");
    }

    #[test]
    fn records_from_different_batches_stay_separate_rounds() {
        let history = vec![
            message(Role::User, "go"),
            batched("Bash", "ls", "a", 1, "call_first"),
            batched("Bash", "pwd", "/x", 2, "call_second"),
        ];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 5, "{ctx:?}");
        assert_eq!(ctx[1].tool_calls.len(), 1);
        assert_eq!(ctx[1].tool_calls[0].id, "call_first");
        assert_eq!(ctx[2], ContextMessage::tool_result("call_first", "a"));
        assert_eq!(ctx[3].tool_calls.len(), 1);
        assert_eq!(ctx[3].tool_calls[0].id, "call_second");
        assert_eq!(ctx[4], ContextMessage::tool_result("call_second", "/x"));
    }

    #[test]
    fn a_task_call_in_the_batch_rides_the_same_assistant_message() {
        let arguments = r#"{"taskId":"1","status":"completed"}"#;
        let history = vec![
            message(Role::User, "go"),
            batched("Bash", "cargo test", "ok", 1, "call_bash"),
            HistoryItem::TaskCall(crate::app::TaskCallRecord {
                name: "TaskUpdate".to_string(),
                args: "#1 → completed".to_string(),
                arguments: arguments.to_string(),
                output: "Updated task #1 status".to_string(),
                ok: true,
                timestamp: String::new(),
                tasks: crate::tasks::TaskStore::new(),
                call_id: Some("call_task".to_string()),
                position: None,
                batch: Some(1),
            }),
        ];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 4, "{ctx:?}");
        assert_eq!(
            ctx[1].tool_calls,
            vec![
                ContextToolCall::new("call_bash", "bash", r#"{"command":"cargo test"}"#),
                ContextToolCall::new("call_task", "taskupdate", arguments),
            ]
        );
        assert_eq!(ctx[2], ContextMessage::tool_result("call_bash", "ok"));
        assert_eq!(
            ctx[3],
            ContextMessage::tool_result("call_task", "Updated task #1 status")
        );
    }

    #[test]
    fn an_agent_group_in_the_batch_replays_its_verbatim_arguments_and_id() {
        // The launch's own arguments replay as the model sent them (a
        // rebuilt object never matches the wire byte for byte), under the
        // provider's id, inside the round's one assistant message.
        let arguments = r#"{"description":"Fetch","prompt":"p","subagent_type":"explore"}"#;
        let history = vec![
            message(Role::User, "go"),
            HistoryItem::AgentGroup(crate::app::AgentGroup {
                background: false,
                agents: vec![crate::app::AgentGroupEntry {
                    id: "a1".into(),
                    description: "Fetch".into(),
                    agent_type: "explore".into(),
                    prompt: "p".into(),
                    status: crate::agents::AgentStatus::Done,
                    tool_uses: 0,
                    tokens: 0,
                    secs: 0,
                    result: String::new(),
                    tool_headers: Vec::new(),
                    output: "framed response".into(),
                    call_id: Some("call_agent".into()),
                    position: None,
                    arguments: Some(arguments.into()),
                }],
                timestamp: String::new(),
                batch: Some(1),
            }),
            batched("Bash", "ls", "files", 1, "call_bash"),
        ];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 4, "{ctx:?}");
        assert_eq!(
            ctx[1].tool_calls,
            vec![
                ContextToolCall::new("call_agent", "agent", arguments),
                ContextToolCall::new("call_bash", "bash", r#"{"command":"ls"}"#),
            ]
        );
        assert_eq!(
            ctx[2],
            ContextMessage::tool_result("call_agent", "framed response")
        );
        assert_eq!(ctx[3], ContextMessage::tool_result("call_bash", "files"));
    }

    #[test]
    fn a_round_replays_its_calls_in_the_models_order_not_the_records() {
        // The wire had [bash, agent, read] in one assistant message. The
        // records land agent-first (a group resolves before the round's
        // ordinary calls run), so without the position the replay would
        // send [agent, bash, read] — a different prefix from the one the
        // provider cached (docs/prompt-caching.md).
        let arguments = r#"{"description":"Fetch","prompt":"p","subagent_type":"explore"}"#;
        let mut bash = batched("Bash", "ls", "files", 1, "call_bash");
        let mut read = batched("Read", "a.rs", "source", 1, "call_read");
        for (item, position) in [(&mut bash, 0), (&mut read, 2)] {
            let HistoryItem::Tool(tool) = item else {
                unreachable!()
            };
            tool.position = Some(position);
        }
        let history = vec![
            message(Role::User, "go"),
            HistoryItem::AgentGroup(crate::app::AgentGroup {
                background: false,
                agents: vec![crate::app::AgentGroupEntry {
                    id: "a1".into(),
                    description: "Fetch".into(),
                    agent_type: "explore".into(),
                    prompt: "p".into(),
                    status: crate::agents::AgentStatus::Done,
                    tool_uses: 0,
                    tokens: 0,
                    secs: 0,
                    result: String::new(),
                    tool_headers: Vec::new(),
                    output: "framed response".into(),
                    call_id: Some("call_agent".into()),
                    arguments: Some(arguments.into()),
                    position: Some(1),
                }],
                timestamp: String::new(),
                batch: Some(1),
            }),
            bash,
            read,
        ];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 5, "{ctx:?}");
        let ids: Vec<&str> = ctx[1]
            .tool_calls
            .iter()
            .map(|call| call.id.as_str())
            .collect();
        assert_eq!(ids, ["call_bash", "call_agent", "call_read"]);
        assert_eq!(ctx[2], ContextMessage::tool_result("call_bash", "files"));
        assert_eq!(
            ctx[3],
            ContextMessage::tool_result("call_agent", "framed response")
        );
        assert_eq!(ctx[4], ContextMessage::tool_result("call_read", "source"));
    }

    #[test]
    fn records_without_a_position_keep_their_recorded_order() {
        // A batch no round announced (the offline dummy) and every rollout
        // written before the field: the records say nothing about the
        // wire's order, so the replay keeps theirs — exactly as before.
        let history = vec![
            message(Role::User, "go"),
            batched("Bash", "ls", "a", 1, "call_first"),
            batched("Bash", "pwd", "/x", 1, "call_second"),
        ];
        let ctx = context_messages(&history);
        let ids: Vec<&str> = ctx[1]
            .tool_calls
            .iter()
            .map(|call| call.id.as_str())
            .collect();
        assert_eq!(ids, ["call_first", "call_second"]);
    }

    #[test]
    fn a_notice_committed_inside_a_batch_replays_after_it() {
        // A completion cell can commit between two cells of one batch (a
        // tool resolution is a safe boundary, docs/background.md), but the
        // model read that note at the next round's top — after every result
        // — so the replay keeps the batch whole and the note behind it.
        let history = vec![
            message(Role::User, "go"),
            batched("Bash", "ls", "a", 1, "call_a"),
            HistoryItem::AgentNotice(crate::app::AgentNotice {
                id: "a1".into(),
                description: "Fetch Warsaw".into(),
                status: crate::agents::AgentStatus::Done,
                secs: 35,
                result: "19°C".into(),
                timestamp: String::new(),
            }),
            batched("Bash", "pwd", "/x", 1, "call_b"),
        ];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 5, "{ctx:?}");
        assert_eq!(ctx[1].tool_calls.len(), 2);
        assert_eq!(ctx[2], ContextMessage::tool_result("call_a", "a"));
        assert_eq!(ctx[3], ContextMessage::tool_result("call_b", "/x"));
        assert_eq!(ctx[4].role, ContextRole::User);
        assert!(ctx[4].text.contains("19°C"), "{ctx:?}");
    }

    #[test]
    fn a_recorded_id_already_used_is_replaced_by_a_synthetic_one() {
        // Two records claiming one id cannot both keep it — a provider pairs
        // results by id — so the second falls back to a synthesized id that
        // no other call in the derivation carries.
        let history = vec![
            message(Role::User, "go"),
            batched("Bash", "ls", "a", 1, "call_0"),
            batched("Bash", "pwd", "/x", 2, "call_0"),
        ];
        let ctx = context_messages(&history);
        let first = ctx[1].tool_calls[0].id.clone();
        let second = ctx[3].tool_calls[0].id.clone();
        assert_eq!(first, "call_0");
        assert_ne!(second, first);
        assert_eq!(ctx[2].tool_call_id.as_deref(), Some(first.as_str()));
        assert_eq!(ctx[4].tool_call_id.as_deref(), Some(second.as_str()));
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
        assert_eq!(
            ctx[0].text, "[Image #1: /a.png] first\n\n[Image #1: /b.png] second",
            "each draft's placeholder names its own picture — the annotation \
             runs per message, before the merge"
        );
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
            cache_write: 0,
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
    fn derives_conversation_agrees_with_the_full_derivation_for_every_item_kind() {
        // The cheap predicate must answer exactly what
        // `context_messages(history).is_empty()` would — the gates that only
        // need the yes/no (the gauge's zero state, auto-compact's
        // anything-to-summarize check, `/compact`'s `Nothing to compact`)
        // must never disagree with the derivation itself. One singleton per
        // history-item kind, plus the edges: empty history, chrome-only
        // histories, and the entryless agent group that derives nothing.
        use crate::agents::AgentStatus;
        let agent_entry = || crate::app::AgentGroupEntry {
            id: "a1".to_string(),
            description: "Fetch Warsaw".to_string(),
            agent_type: "general-purpose".to_string(),
            prompt: "weather in Warsaw?".to_string(),
            status: AgentStatus::Done,
            tool_uses: 1,
            tokens: 10,
            secs: 2,
            result: "19°C".to_string(),
            tool_headers: Vec::new(),
            output: "19°C".to_string(),
            call_id: None,
            position: None,
            arguments: None,
        };
        let cases: Vec<(&str, Vec<HistoryItem>)> = vec![
            ("empty history", Vec::new()),
            ("user message", vec![message(Role::User, "hi")]),
            ("assistant message", vec![message(Role::Assistant, "hello")]),
            ("error notice", vec![message(Role::Error, "boom")]),
            ("system notice", vec![message(Role::System, "note")]),
            ("dangling shell header", vec![message(Role::Shell, "pwd")]),
            (
                "resolved shell run",
                vec![
                    message(Role::Shell, "pwd"),
                    tool("pwd", "", "/home", ToolStatus::Ok, true),
                ],
            ),
            (
                "backend tool",
                vec![tool("bash", "ls", "ok", ToolStatus::Ok, false)],
            ),
            (
                "turn summary",
                vec![HistoryItem::Summary(TurnSummary {
                    verb: "Done",
                    tokens: 0,
                    cached: 0,
                    cache_write: 0,
                    secs: 3,
                    timestamp: String::new(),
                    shells: 0,
                })],
            ),
            (
                "settled reasoning",
                vec![HistoryItem::Reasoning(crate::app::Reasoning {
                    text: "private deliberation".to_string(),
                    secs: 5,
                    tokens: 100,
                    timestamp: String::new(),
                })],
            ),
            ("compaction marker", vec![compaction("handoff")]),
            (
                "hook note",
                vec![HistoryItem::HookNote(crate::app::HookNote {
                    label: "Stop hook".into(),
                    text: "tests are red".into(),
                    timestamp: String::new(),
                })],
            ),
            (
                "background notice",
                vec![HistoryItem::Background(crate::app::BackgroundNotice {
                    description: "Ping".to_string(),
                    id: "bash_1".to_string(),
                    code: Some(0),
                    killed: false,
                    output_tail: "pong".to_string(),
                    origin: None,
                    waiting: false,
                    timestamp: String::new(),
                })],
            ),
            (
                "agent group",
                vec![HistoryItem::AgentGroup(crate::app::AgentGroup {
                    background: false,
                    agents: vec![agent_entry()],
                    timestamp: String::new(),
                    batch: None,
                })],
            ),
            (
                "entryless agent group",
                vec![HistoryItem::AgentGroup(crate::app::AgentGroup {
                    background: false,
                    agents: Vec::new(),
                    timestamp: String::new(),
                    batch: None,
                })],
            ),
            (
                "agent notice",
                vec![HistoryItem::AgentNotice(crate::app::AgentNotice {
                    id: "a1".into(),
                    description: "Fetch Warsaw".into(),
                    status: AgentStatus::Done,
                    secs: 35,
                    result: "19°C".into(),
                    timestamp: String::new(),
                })],
            ),
            (
                "task call",
                vec![{
                    let mut store = crate::tasks::TaskStore::new();
                    let arguments = r#"{"subject":"Add tests","description":"d"}"#;
                    let output = store.run_create(arguments).unwrap();
                    HistoryItem::TaskCall(crate::app::TaskCallRecord {
                        name: "TaskCreate".to_string(),
                        args: "Add tests".to_string(),
                        arguments: arguments.to_string(),
                        output,
                        ok: true,
                        timestamp: String::new(),
                        tasks: store,
                        call_id: None,
                        position: None,
                        batch: None,
                    })
                }],
            ),
            (
                "chrome-only history",
                vec![
                    HistoryItem::Summary(TurnSummary {
                        verb: "Done",
                        tokens: 0,
                        cached: 0,
                        cache_write: 0,
                        secs: 3,
                        timestamp: String::new(),
                        shells: 0,
                    }),
                    HistoryItem::Reasoning(crate::app::Reasoning {
                        text: "thoughts".to_string(),
                        secs: 1,
                        tokens: 10,
                        timestamp: String::new(),
                    }),
                ],
            ),
            (
                "chrome around one real message",
                vec![
                    HistoryItem::Reasoning(crate::app::Reasoning {
                        text: "thoughts".to_string(),
                        secs: 1,
                        tokens: 10,
                        timestamp: String::new(),
                    }),
                    message(Role::Assistant, "the answer"),
                ],
            ),
        ];
        for (label, history) in cases {
            assert_eq!(
                derives_conversation(&history),
                !context_messages(&history).is_empty(),
                "predicate disagrees with the derivation for: {label}"
            );
        }
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
            waiting: false,
            timestamp: String::new(),
        })];
        let ctx = context_messages(&history);
        assert_eq!(ctx.len(), 1);
        assert_eq!(ctx[0].role, ContextRole::User);
        assert_eq!(
            ctx[0].text,
            "[background] Background command \"Ping x.com 200 times\" \
             completed (exit code 0).\nFinal output (tail):\n\
             64 bytes from x.com\n200 packets transmitted"
        );
    }

    #[test]
    fn a_backgrounded_tool_replays_its_model_facing_launch_text() {
        // A backgrounded bash call is still a native tool-call pair — the
        // stored output IS the model-facing launch text (the interim-output
        // path), so no special casing is needed.
        let history = vec![tool(
            "Bash",
            "ping -c 200 x.com",
            "Command running in the background. Output is streaming to /tmp/a0/s1/bash_1.output.",
            ToolStatus::Backgrounded,
            false,
        )];
        let ctx = context_messages(&history);
        assert_eq!(
            ctx[1],
            ContextMessage::tool_result(
                "call_0",
                "Command running in the background. Output is streaming to /tmp/a0/s1/bash_1.output."
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
        // The rendered AGENTS.md section rides the context's first user entry
        // inside the one `<system-reminder>` (`crate::reminder`); a first user
        // message merges after it under the module's alternation convention
        // (push_text), the tags keeping the boundary clear.
        let history = vec![message(Role::User, "hello"), message(Role::Assistant, "hi")];
        let section = "Contents of /repo/AGENTS.md (project instructions, checked into the codebase):\n\nUse TDD.";
        let ctx = context_messages_with(Some(section), &history);
        assert_eq!(ctx.len(), 2, "{ctx:?}");
        assert_eq!(ctx[0].role, ContextRole::User);
        assert_eq!(
            ctx[0].text,
            "<system-reminder>\n\
             Use the following contexts and instructions:\n\n\
             Contents of /repo/AGENTS.md (project instructions, checked into the codebase):\n\n\
             Use TDD.\n\
             </system-reminder>\n\n\
             hello"
        );
        assert_eq!(ctx[1].text, "hi");
    }

    #[test]
    fn a_session_with_neither_fragment_sends_no_reminder_at_all() {
        // No AGENTS.md, no skills, no agent types: nothing to remind the model
        // of, so the context is exactly the derived conversation — no empty
        // block, no bare preamble.
        let history = vec![message(Role::User, "hello"), message(Role::Assistant, "hi")];
        let ctx = context_messages_full(None, None, &history);
        assert_eq!(ctx, context_messages(&history));
        assert!(
            !ctx.iter().any(|m| m.text.contains("<system-reminder>")),
            "{ctx:?}"
        );
    }

    #[test]
    fn an_already_rendered_reminder_leads_verbatim() {
        // A subagent's briefing is wrapped where its launch is built
        // (`skills::listing_message`), so the agent view's window takes it
        // as-is — wrapping it again would show a block the agent never read.
        let history = vec![message(Role::User, "task?")];
        let briefing = "<system-reminder>\nskills: dataviz\n</system-reminder>";
        let ctx = context_messages_behind(Some(briefing), &history);
        assert_eq!(ctx.len(), 1, "{ctx:?}");
        assert_eq!(ctx[0].text, format!("{briefing}\n\ntask?"));
        assert_eq!(
            context_messages_behind(None, &history),
            context_messages(&history)
        );
        assert_eq!(
            context_messages_behind(Some("  "), &history),
            context_messages(&history),
            "a blank fragment changes nothing"
        );
    }

    /// A [`tool`] carrying the model's verbatim arguments — what every live
    /// call now records beside the header summary.
    fn tool_with_arguments(name: &str, args: &str, arguments: &str, output: &str) -> HistoryItem {
        let HistoryItem::Tool(mut call) = tool(name, args, output, ToolStatus::Ok, false) else {
            unreachable!()
        };
        call.arguments = Some(arguments.to_string());
        HistoryItem::Tool(call)
    }

    #[test]
    fn a_write_replays_with_the_content_it_actually_sent() {
        // The bug: the stored summary is only the *path*, so the replay was
        // `write({"path": …})` — the model watching itself write a file with
        // no content, while the whole content rode the result as a numbered
        // body. The verbatim arguments make the replay lossless, which is
        // what lets the result collapse to one line (`docs/tools.md`).
        let arguments = r#"{"path":"/tmp/a.py","content":"print(1)\n"}"#;
        let history = vec![tool_with_arguments(
            "Write",
            "/tmp/a.py",
            arguments,
            "Wrote 1 line to a.py\n1 print(1)",
        )];
        let ctx = context_messages(&history);
        assert_eq!(ctx[0].tool_calls[0].name, "write");
        assert_eq!(ctx[0].tool_calls[0].arguments, arguments);
        assert_eq!(
            ctx[1].text, "Wrote 1 line to a.py\n1 print(1)",
            "the result is whatever was recorded — the executor decides its size"
        );
    }

    #[test]
    fn an_edit_replays_with_both_strings_and_the_replace_all_flag() {
        let arguments = r#"{"path":"/tmp/n.txt","old_string":"Rivero","new_string":"Rivera","replace_all":true}"#;
        let history = vec![tool_with_arguments("Edit", "/tmp/n.txt", arguments, "ok")];
        let ctx = context_messages(&history);
        assert_eq!(ctx[0].tool_calls[0].arguments, arguments);
    }

    #[test]
    fn a_call_without_recorded_arguments_falls_back_to_the_old_reconstruction() {
        // Rollouts written before the field, the `!` shell, the offline
        // dummy: the per-tool table still answers, so an old session replays
        // exactly as it did.
        let history = vec![tool(
            "Write",
            "hello.py",
            "Wrote 1 line",
            ToolStatus::Ok,
            false,
        )];
        let ctx = context_messages(&history);
        assert_eq!(ctx[0].tool_calls[0].arguments, r#"{"path":"hello.py"}"#);
    }

    #[test]
    fn unparseable_recorded_arguments_fall_back_rather_than_break_the_request() {
        // A validating provider rejects a tool call whose arguments are not a
        // JSON object, so a damaged record degrades to the reconstruction.
        let history = vec![tool_with_arguments("Bash", "ls", "not json", "ok")];
        let ctx = context_messages(&history);
        assert_eq!(ctx[0].tool_calls[0].arguments, r#"{"command":"ls"}"#);
    }

    #[test]
    fn a_companion_call_replays_under_its_own_wire_name_with_its_arguments() {
        // `BashSend` lowercases to `bashsend`, the name the model called — the
        // task tools' convention, so the replay needs no table for it
        // (docs/bash-tools.md).
        let arguments = r#"{"session_id":"b7x2k9m1q","input":"print(1)<Enter>"}"#;
        let history = vec![tool_with_arguments(
            crate::llm::tools::BASH_SEND_DISPLAY,
            "b7x2k9m1q ← print(1)⏎",
            arguments,
            "Running (session b7x2k9m1q, waiting for input)\n>>> print(1)\n1\n>>>",
        )];
        let ctx = context_messages(&history);
        assert_eq!(ctx[0].tool_calls[0].name, crate::llm::tools::BASH_SEND_TOOL);
        assert_eq!(ctx[0].tool_calls[0].arguments, arguments);
    }

    #[test]
    fn a_legacy_session_call_replays_as_the_tool_that_does_it_now() {
        // `bash_session` is executed but no longer offered: a resumed
        // conversation's request must name only tools it offers, or a
        // validating provider rejects the round (docs/bash-tools.md).
        let replayed = |arguments: &str| {
            let history = vec![tool_with_arguments(
                crate::llm::tools::BASH_SESSION_TOOL_DISPLAY,
                "b1",
                arguments,
                "Running (session b1)\n(no new output)",
            )];
            let ctx = context_messages(&history);
            let call = &ctx[0].tool_calls[0];
            (call.name.clone(), call.arguments.clone())
        };
        assert_eq!(
            replayed(r#"{"session_id":"b1","input":"print(1)\n"}"#),
            (
                "bashsend".to_string(),
                r#"{"session_id":"b1","input":"print(1)\n"}"#.to_string()
            )
        );
        assert_eq!(
            replayed(r#"{"session_id":"b1","timeout":30000}"#),
            (
                "bashwait".to_string(),
                r#"{"session_id":"b1","wait":30}"#.to_string()
            )
        );
        assert_eq!(
            replayed(r#"{"session_id":"b1","input":"q","kill":true}"#),
            ("bashkill".to_string(), r#"{"session_id":"b1"}"#.to_string())
        );
    }

    #[test]
    fn a_skill_call_replays_with_the_skill_it_loaded() {
        // Caught live: the replay carried `skill({})`, so a later round saw
        // the model call the tool with no arguments — and a validating
        // provider rejects that outright, `skill` being required. The stored
        // summary IS the name (`summarize_call`), so it reconstructs exactly;
        // the optional `args` are lossy the same way `bash`'s `timeout`
        // is, and the result below carries the rendered body they produced.
        let history = vec![tool(
            "Skill",
            "haiku-writer",
            crate::skills::SKILL_LOADED_DISPLAY,
            ToolStatus::Ok,
            false,
        )];
        let ctx = context_messages(&history);
        assert_eq!(ctx[0].tool_calls.len(), 1, "{ctx:?}");
        assert_eq!(ctx[0].tool_calls[0].name, crate::skills::SKILL_TOOL_NAME);
        assert_eq!(
            ctx[0].tool_calls[0].arguments,
            r#"{"skill":"haiku-writer"}"#
        );
    }

    #[test]
    fn the_listings_follow_the_instructions_inside_the_one_reminder() {
        // One block, sections in a fixed order — the project instructions,
        // then the skills and agent-type listings: every section is
        // re-rendered per turn, so one that moved would invalidate the prompt
        // cache behind it (docs/context.md).
        let history = vec![message(Role::User, "hello")];
        let ctx = context_messages_full(
            Some("guide"),
            Some("The following skills are available for use with the Skill tool:\n\n- x: X"),
            &history,
        );
        assert_eq!(
            ctx.len(),
            1,
            "the block and the message merge as one user entry: {ctx:?}"
        );
        assert_eq!(
            ctx[0].text,
            "<system-reminder>\n\
             Use the following contexts and instructions:\n\n\
             guide\n\n\
             The following skills are available for use with the Skill tool:\n\n\
             - x: X\n\
             </system-reminder>\n\n\
             hello"
        );
    }

    #[test]
    fn a_session_with_no_listings_sends_the_instructions_alone() {
        // The listings are optional sections: without them the reminder is
        // the instructions section by itself, and a blank listing is no
        // listing.
        let history = vec![message(Role::User, "hello"), message(Role::Assistant, "hi")];
        assert_eq!(
            context_messages_full(Some("guide"), None, &history),
            context_messages_with(Some("guide"), &history)
        );
        assert_eq!(
            context_messages_full(None, Some("   "), &history),
            context_messages(&history),
            "a blank listing changes nothing"
        );
    }

    #[test]
    fn the_listings_survive_compaction_at_the_front() {
        // Like the project doc: the reminder leads the post-`/compact` shape
        // too, so a compacted session still knows its skills and agent types.
        let history = vec![
            message(Role::User, "old"),
            compaction("we did things"),
            message(Role::User, "new"),
        ];
        let ctx = context_messages_full(None, Some("SKILLS"), &history);
        assert!(ctx[0].text.starts_with("<system-reminder>\n"), "{ctx:?}");
        assert!(
            ctx[0]
                .text
                .contains("\n\nSKILLS\n</system-reminder>\n\nold"),
            "{ctx:?}"
        );
    }

    #[test]
    fn user_instructions_alone_are_a_context_of_one() {
        let ctx = context_messages_with(Some("guide"), &[]);
        assert_eq!(
            ctx,
            vec![ContextMessage::new(
                ContextRole::User,
                crate::reminder::reminder_message(&["guide"])
            )]
        );
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
            ctx[0].text.starts_with("<system-reminder>\n"),
            "{:?}",
            ctx[0].text
        );
        assert!(
            ctx[0]
                .text
                .contains("guide\n</system-reminder>\n\nold question"),
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
            call_id: None,
            position: None,
            arguments: None,
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
                batch: None,
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
    #[test]
    fn an_mcp_call_replays_with_its_wire_name_and_verbatim_arguments() {
        // The recorded display name inverts to the wire name and the stored
        // args ARE the raw JSON — a validating provider re-reads exactly what
        // the model sent (docs/mcp.md).
        let history = vec![
            message(Role::User, "ask the wiki"),
            tool(
                "deepwiki - ask_question (MCP)",
                r#"{"repoName":"a/b","question":"What?"}"#,
                "the answer",
                ToolStatus::Ok,
                false,
            ),
        ];
        let ctx = context_messages(&history);
        let call = &ctx[1].tool_calls[0];
        assert_eq!(call.name, "mcp__deepwiki__ask_question");
        assert_eq!(call.arguments, r#"{"repoName":"a/b","question":"What?"}"#);
        assert_eq!(ctx[2].text, "the answer");
        assert_eq!(ctx[2].tool_call_id.as_deref(), Some("call_0"));
    }

    #[test]
    fn an_mcp_call_with_unparseable_args_degrades_to_empty_arguments() {
        let history = vec![
            message(Role::User, "x"),
            tool(
                "deepwiki - ask_question (MCP)",
                "corrupted…",
                "out",
                ToolStatus::Ok,
                false,
            ),
        ];
        let ctx = context_messages(&history);
        assert_eq!(ctx[1].tool_calls[0].arguments, "{}");
    }
}
