//! Compaction **inside a running turn** (`docs/compact.md` *Compacting while
//! the agent works*).
//!
//! A turn that calls tools re-sends its whole conversation every round, so a
//! long one fills the model's window long before it ends — and a compaction
//! that waits for the turn to finish waits for a request that no longer
//! fits. This module is the agent loop's own answer:
//!
//! - **[`Compaction`]** is the seam [`run_agent`](super::agent::run_agent)
//!   consults — before every request, when one overflows, and before the
//!   turn ends. [`NoCompaction`] is the do-nothing default.
//! - **[`RollingCompactor`]** implements it. Past
//!   [`soft_threshold`] it has a summary
//!   written **in the background** while the agent keeps working; at the next
//!   round boundary after the summary lands it swaps it in — the user's
//!   recent messages and the summary in place of everything the summarizer
//!   read, the rounds run meanwhile kept as they happened. Past
//!   [`hard_threshold`] the loop waits for
//!   it instead of sending a request that may not fit. It does this as often
//!   as a turn needs: there is no count to run out, only a check that a
//!   compaction has something to gain.
//! - The summarizer never reads the conversation in its wire shape. It reads
//!   a **transcript** of it ([`summary_request`]) cut to a share of the
//!   window, so the request that makes room is never the one that does not
//!   fit, and no provider's rules about tool calls without tools apply.
//!
//! Pure but for the clock: the summarizer is a closure handed in
//! ([`Summarize`]), so the whole state machine is driven with fakes and no
//! network, like the loop it serves.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use super::{ChatMessage, ContentPart, LlmError, MessageContent};
use crate::compaction::{
    CompactionControl, CompactionSeed, hard_threshold, min_gain, soft_threshold,
    summary_input_budget, summary_max_tokens, user_message_budget,
};
use crate::context::{
    ContextMessage, ContextRole, SUMMARIZATION_PROMPT, budget_user_texts, summary_bridge,
};
use crate::hooks::HookReport;
use crate::stream::{CancelToken, StreamEvent, TokenUsage};
use crate::tokens::{ContextMeter, estimate};

/// What a round boundary may go on to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Send the request.
    Continue,
    /// The turn was cancelled while it waited on a summary: stop silently,
    /// as a cancel anywhere else does.
    Cancelled,
}

/// Whether room was made for a request that did not fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Room {
    /// The context is smaller now: send the request again.
    Made,
    /// Nothing could be taken out — auto compaction is off, there is nothing
    /// left to summarize, or the summary failed.
    Unable,
    /// The turn was cancelled while it waited.
    Cancelled,
}

/// The agent loop's compaction seam (`docs/compact.md`). Every method
/// defaults to "nothing happens", which is what [`NoCompaction`] is: a loop
/// nobody handed a compactor to runs exactly as it did before there was one.
pub trait Compaction {
    /// The top of a round, once what the conversation gained since the last
    /// request has been folded in and before the next request goes out. May
    /// start a summary, swap a finished one into `messages`, or hold the
    /// round until one lands.
    fn before_round(&mut self, _messages: &mut Vec<ChatMessage>) -> Flow {
        Flow::Continue
    }

    /// A request did not fit the model's window. Make room in `messages`,
    /// and say whether the request is worth sending again.
    fn make_room(&mut self, _messages: &mut Vec<ChatMessage>) -> Room {
        Room::Unable
    }

    /// The turn is about to end on an answer. A summary still being written
    /// is waited for and swapped in, so the work is not thrown away with the
    /// turn.
    fn before_done(&mut self, _messages: &mut Vec<ChatMessage>) -> Flow {
        Flow::Continue
    }

    /// The user said `text` mid-turn — a message the loop just folded into
    /// the conversation. The compacted shape keeps the user's own messages
    /// word for word, and this one is theirs.
    fn user_message(&mut self, _text: &str) {}
}

/// The do-nothing compactor: a subagent or an embedder that was handed none,
/// the summarizer's own loop, the tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCompaction;

impl Compaction for NoCompaction {}

// ----- telling an overflow from any other failure -----

/// What a provider says when a request is larger than the model's context
/// window — every wire's own words for it, lowercase. A request refused for
/// any other reason must match none: the answer to an overflow is to compact
/// and send again, which would only repeat any other mistake.
const OVERFLOW_TELLS: [&str; 16] = [
    "context_length_exceeded",
    "context length",
    "context window",
    "context size",
    "exceed_context",
    "prompt is too long",
    "input is too long",
    "input too long",
    "too many tokens",
    "too many input tokens",
    "maximum number of tokens",
    "exceed context limit",
    "exceeds the context",
    "reduce the length",
    "maximum prompt length",
    "token limit exceeded",
];

/// Does `text` — an error body, or the message made of one — say the request
/// overflowed the model's context window?
#[must_use]
pub fn names_context_overflow(text: &str) -> bool {
    let lower = text.to_lowercase();
    OVERFLOW_TELLS.iter().any(|tell| lower.contains(tell))
}

/// Did this request fail because it is larger than the model's window? Only
/// a provider's own refusal can say so; a transport failure, a decode
/// failure and a cancel never do.
#[must_use]
pub fn is_context_overflow(error: &LlmError) -> bool {
    match error {
        // 401/403/429 are about who is asking and how often, whatever the
        // body goes on to mention.
        LlmError::Api { status, body } => {
            !matches!(status, 401 | 403 | 429) && names_context_overflow(body)
        }
        LlmError::Http(_)
        | LlmError::Unreachable { .. }
        | LlmError::Decode(_)
        | LlmError::Cancelled => false,
    }
}

// ----- how full the context is -----

/// The estimate of one wire message, by the rule the footer gauge counts
/// with (`app::message_tokens`): its text, a flat rate per picture, each
/// tool call's name and arguments.
fn message_tokens(message: &ChatMessage) -> u64 {
    let content = match &message.content {
        MessageContent::Text(text) => estimate(text),
        MessageContent::Parts(parts) => parts
            .iter()
            .map(|part| match part {
                ContentPart::Text { text, .. } => estimate(text),
                ContentPart::ImageUrl { .. } => crate::app::IMAGE_INPUT_TOKENS,
            })
            .sum(),
    };
    let calls: usize = message
        .tool_calls
        .iter()
        .map(|call| estimate(&call.function.name) + estimate(call.function.arguments.as_str()))
        .sum();
    u64::try_from(content + calls).unwrap_or(u64::MAX)
}

/// The estimate of a run of wire messages.
#[must_use]
pub fn wire_tokens(messages: &[ChatMessage]) -> u64 {
    messages.iter().map(message_tokens).sum()
}

/// How many pictures a run of wire messages carries.
fn wire_images(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .map(|message| match &message.content {
            MessageContent::Text(_) => 0,
            MessageContent::Parts(parts) => parts
                .iter()
                .filter(|part| matches!(part, ContentPart::ImageUrl { .. }))
                .count(),
        })
        .sum()
}

/// The size of the context **in the provider's own tokens**, as the agent
/// loop's thread reads it: the last usage frame's count of the request it
/// measured, plus the calibrated estimate of what joined since — the footer
/// gauge's rule (`docs/token-estimate.md`), one level down, over the
/// messages as the wire carries them.
///
/// The meter behind it is the backend's, shared by every turn it runs, so
/// what one turn's frames taught about this provider's tokens is still known
/// when the next turn opens. Before any frame the reading is the estimate
/// alone, the tool schemas' among it.
#[derive(Debug, Clone)]
pub struct LoadMeter {
    meter: Arc<Mutex<ContextMeter>>,
    /// What every request carries beyond its messages — the tool schemas —
    /// in estimated tokens.
    overhead: u64,
}

impl LoadMeter {
    /// A meter over the backend's shared `meter`, for requests that carry
    /// `overhead` estimated tokens of tool schemas.
    #[must_use]
    pub const fn new(meter: Arc<Mutex<ContextMeter>>, overhead: u64) -> Self {
        Self { meter, overhead }
    }

    /// A meter of its own — a conversation no other turn continues.
    #[must_use]
    pub fn standalone(overhead: u64) -> Self {
        Self::new(Arc::default(), overhead)
    }

    fn estimate(&self, messages: &[ChatMessage]) -> u64 {
        wire_tokens(messages).saturating_add(self.overhead)
    }

    /// The provider counted `input` tokens for the request that carried
    /// `messages`.
    pub fn measured(&self, messages: &[ChatMessage], input: u64) {
        if let Ok(mut meter) = self.meter.lock() {
            meter.measured(input, self.estimate(messages), wire_images(messages));
        }
    }

    /// The size of a request carrying `messages`. A poisoned meter reads the
    /// estimate, which is where every meter starts.
    #[must_use]
    pub fn reading(&self, messages: &[ChatMessage]) -> u64 {
        let estimate = self.estimate(messages);
        self.meter
            .lock()
            .map_or(estimate, |meter| meter.reading(estimate))
    }

    /// `messages` were rewritten under the base (a compaction): take their
    /// reading as the new one, so what joins next is added to it.
    pub fn rebase(&self, messages: &[ChatMessage]) {
        if let Ok(mut meter) = self.meter.lock() {
            meter.rebase(self.estimate(messages));
        }
    }
}

// ----- what the summarizer reads -----

/// How much of one message the summarizer is shown, in approx tokens
/// (four bytes each). A tool's output is what fills a context, and its
/// middle is what a summary needs least.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Caps {
    /// A tool result.
    result: usize,
    /// A tool call's arguments — a `write`'s whole file rides here.
    arguments: usize,
    /// A message's own text.
    text: usize,
}

impl Caps {
    const DEFAULT: Self = Self {
        result: 2_000,
        arguments: 1_000,
        text: 6_000,
    };

    /// The least a message is cut to before whole messages are left out
    /// instead.
    const FLOOR: Self = Self {
        result: 150,
        arguments: 100,
        text: 300,
    };

    const fn halved(self) -> Self {
        Self {
            result: Self::floored(self.result / 2, Self::FLOOR.result),
            arguments: Self::floored(self.arguments / 2, Self::FLOOR.arguments),
            text: Self::floored(self.text / 2, Self::FLOOR.text),
        }
    }

    /// Twice the room, for the newest messages: what was in progress is what
    /// the handoff most needs whole.
    const fn recent(self) -> Self {
        Self {
            result: self.result.saturating_mul(2),
            arguments: self.arguments.saturating_mul(2),
            text: self.text.saturating_mul(2),
        }
    }

    const fn floored(value: usize, floor: usize) -> usize {
        if value < floor { floor } else { value }
    }

    fn at_floor(self) -> bool {
        self == Self::FLOOR
    }
}

/// How many of the newest messages are shown with [`Caps::recent`].
const RECENT_MESSAGES: usize = 8;

/// What every mark of something missing from the transcript says, and what
/// the summarization prompt tells the summarizer to look for
/// (`prompts/compact_prompt.md`). The words are the contract: a live
/// summarizer read codex's `…N tokens truncated…` as the *tool's* output
/// having been cut short, and its summary told the agent that files it had
/// read whole were "truncated in the middle" — so a mark names the
/// transcript and never uses that word.
pub(crate) const TRANSCRIPT_CUT: &str = "left out of this transcript";

/// `text` within `cap` approx tokens: its head and its tail around a line
/// of its own counting what is [missing](TRANSCRIPT_CUT).
fn cut_middle(text: &str, cap: usize) -> String {
    crate::context::truncate_middle_with(text, cap, |removed| {
        format!("\n[… {removed} tokens {TRANSCRIPT_CUT} …]\n")
    })
}

/// `text`, cut in its middle when it is longer than `cap` approx tokens.
fn push_capped(out: &mut String, text: &str, cap: usize) {
    if text.len() <= cap.saturating_mul(4) {
        out.push_str(text);
    } else {
        out.push_str(&cut_middle(text, cap));
    }
}

/// The tool each call id names, so a result can say whose it is.
fn call_names(messages: &[ChatMessage]) -> HashMap<&str, &str> {
    messages
        .iter()
        .flat_map(|message| &message.tool_calls)
        .map(|call| (call.id.as_str(), call.function.name.as_str()))
        .collect()
}

/// One message as the summarizer reads it: who said it, then what — a tool
/// call on a line of its own, a picture as a mark where it sat.
fn transcript_block(message: &ChatMessage, names: &HashMap<&str, &str>, caps: Caps) -> String {
    let mut block = String::new();
    let (label, cap) = match (message.role.as_str(), message.tool_call_id.as_deref()) {
        (_, Some(id)) => {
            block.push_str("[tool result");
            if let Some(name) = names.get(id) {
                block.push_str(": ");
                block.push_str(name);
            }
            block.push(']');
            ("", caps.result)
        }
        ("assistant", None) => ("[assistant]", caps.text),
        ("user", None) => ("[user]", caps.text),
        (_, None) => ("[note]", caps.text),
    };
    block.push_str(label);
    match &message.content {
        MessageContent::Text(text) => {
            if !text.is_empty() {
                block.push('\n');
                push_capped(&mut block, text, cap);
            }
        }
        MessageContent::Parts(parts) => {
            for part in parts {
                block.push('\n');
                match part {
                    ContentPart::Text { text, .. } => push_capped(&mut block, text, cap),
                    ContentPart::ImageUrl { .. } => block.push_str("[image]"),
                }
            }
        }
    }
    for call in &message.tool_calls {
        block.push_str("\n[tool call] ");
        block.push_str(&call.function.name);
        block.push(' ');
        push_capped(&mut block, call.function.arguments.as_str(), caps.arguments);
    }
    block
}

/// The conversation in `messages` as one text the summarizer reads, within
/// `budget` estimated tokens.
///
/// The system prompt is left out: the summarizer is sent it as its own. Every
/// other message is a block — who said it, then what it said — with the long
/// ones cut in their middles (`Caps`), the newest `RECENT_MESSAGES` given
/// twice the room. A conversation still over `budget` is cut harder, and
/// when no cut is enough its oldest messages after the first are left out
/// under a line saying how many: the first carries the request everything
/// else answers, and the newest carry the work in progress.
#[must_use]
pub fn render_transcript(messages: &[ChatMessage], budget: usize) -> String {
    let conversation = match messages.first() {
        Some(first) if first.role == "system" => &messages[1..],
        _ => messages,
    };
    let names = call_names(conversation);
    let recent_from = conversation.len().saturating_sub(RECENT_MESSAGES);
    let mut caps = Caps::DEFAULT;
    loop {
        let blocks: Vec<String> = conversation
            .iter()
            .enumerate()
            .map(|(index, message)| {
                let caps = if index >= recent_from {
                    caps.recent()
                } else {
                    caps
                };
                transcript_block(message, &names, caps)
            })
            .collect();
        let sizes: Vec<usize> = blocks.iter().map(|block| estimate(block)).collect();
        if sizes.iter().sum::<usize>() <= budget {
            return blocks.join("\n\n");
        }
        if caps.at_floor() {
            return newest_within(blocks, &sizes, budget);
        }
        caps = caps.halved();
    }
}

/// `blocks` with the oldest after the first left out until the rest fits
/// `budget`: the first block, a line counting what is missing, then as many
/// of the newest as there is room for.
fn newest_within(mut blocks: Vec<String>, sizes: &[usize], budget: usize) -> String {
    /// What the line counting the omitted messages costs, more or less.
    const NOTE_TOKENS: usize = 16;
    let Some((&first_size, rest)) = sizes.split_first() else {
        return String::new();
    };
    // The opening message keeps at most half the budget.
    let first_budget = budget / 2;
    let first = if first_size > first_budget {
        // Cut by the approx-token rule, which runs long on code: three
        // quarters of the share keeps the estimate inside it.
        cut_middle(&blocks[0], first_budget * 3 / 4)
    } else {
        std::mem::take(&mut blocks[0])
    };
    let mut remaining = budget
        .saturating_sub(estimate(&first))
        .saturating_sub(NOTE_TOKENS);
    let mut kept_from = blocks.len();
    for (index, &size) in rest.iter().enumerate().rev() {
        if size > remaining {
            break;
        }
        remaining -= size;
        kept_from = index + 1;
    }
    let omitted = kept_from - 1;
    let mut parts = vec![first];
    if omitted > 0 {
        parts.push(format!(
            "[… {omitted} earlier message{} {TRANSCRIPT_CUT} …]",
            if omitted == 1 { "" } else { "s" }
        ));
    }
    parts.extend(blocks.drain(kept_from..));
    parts.join("\n\n")
}

/// The one user message a summarization request is made of: the
/// conversation as a transcript within `budget` estimated tokens, then the
/// summarization prompt (`prompts/compact_prompt.md`).
///
/// A transcript rather than the conversation's own messages, for three
/// reasons. It can be **cut to fit** — the request that makes room must
/// never be the one that overflows. It is sent **without tools**, and a
/// provider that refuses tool calls in a request offering none has nothing
/// to refuse. And a model handed its own conversation with one more message
/// at the end tends to carry on with the task instead of summarizing it.
#[must_use]
pub fn summary_request(messages: &[ChatMessage], budget: usize) -> String {
    format!(
        "<conversation>\n{}\n</conversation>\n\n{}",
        render_transcript(messages, budget),
        SUMMARIZATION_PROMPT.trim_end()
    )
}

/// The context an idle `/compact` turn sends in place of the conversation
/// (`docs/compact.md` *The compact turn*): one user message, the derived
/// `context` as a transcript cut to the `window`'s share
/// ([`summary_input_budget`]) with the summarization prompt under it. A
/// conversation that has outgrown its model's window is still one that can
/// be compacted — the request that makes room is never the one that does
/// not fit. Pictures are named where they sat, never sent.
#[must_use]
pub fn summary_context(context: &[ContextMessage], window: Option<u64>) -> Vec<ContextMessage> {
    let unpictured: Vec<ContextMessage> = context
        .iter()
        .map(|message| {
            let mut message = message.clone();
            for path in std::mem::take(&mut message.images) {
                message
                    .text
                    .push_str(&format!("\n[image: {}]", path.display()));
            }
            message
        })
        .collect();
    let wire = super::backend::build_messages(None, "", &unpictured, |_| {
        None::<crate::images::AttachmentUrl>
    });
    vec![ContextMessage::new(
        ContextRole::User,
        summary_request(&wire, summary_input_budget(window)),
    )]
}

// ----- what a compaction leaves behind -----

/// What stands in for everything a summary covers: the system prompt as it
/// was, then **one** user message — the fragment the window opens with, the
/// user's own recent messages under `budget` approx tokens, and the bridge
/// carrying the summary — joined exactly as the derivation joins them
/// (`context::derive_history_into`), so the request a compacted turn goes on
/// to send is the one every later turn rebuilds from the history.
#[must_use]
pub fn compacted_prefix(
    system: Option<ChatMessage>,
    leading: Option<&str>,
    user_texts: &[String],
    summary: &str,
    budget: usize,
) -> Vec<ChatMessage> {
    let mut texts: Vec<String> = leading
        .filter(|fragment| !fragment.trim().is_empty())
        .map(str::to_string)
        .into_iter()
        .collect();
    texts.extend(budget_user_texts(
        user_texts.iter().map(String::as_str),
        budget,
    ));
    texts.push(summary_bridge(summary, true));
    let mut prefix: Vec<ChatMessage> = system.into_iter().collect();
    prefix.push(ChatMessage::user(
        texts.join(crate::context::MERGE_SEPARATOR),
    ));
    prefix
}

/// The seed of a conversation that has no history but its messages — a
/// subagent's (`docs/compact.md`). `leading` is its briefing, `task` the
/// prompt it was launched on; the user's messages are the task and then
/// every plain user-role message after it that is neither of those, nor a
/// note the app wrote (`[system] …`, `[error] …`), nor what an earlier
/// compaction left — the messages someone actually sent it.
#[must_use]
pub fn seed_from_messages(
    messages: &[ChatMessage],
    leading: Option<String>,
    task: Option<&str>,
) -> CompactionSeed {
    let said = messages
        .iter()
        .filter_map(|message| match &message.content {
            MessageContent::Text(text) if message.role == "user" => Some(text.as_str()),
            _ => None,
        });
    let theirs = |text: &&str| {
        Some(*text) != task
            && Some(*text) != leading.as_deref()
            && !text.starts_with(crate::reminder::REMINDER_OPEN)
            && !text.starts_with("[system] ")
            && !text.starts_with("[error] ")
            && !text.contains(crate::context::SUMMARY_PREFIX)
    };
    let user_texts = task
        .into_iter()
        .chain(said.filter(theirs))
        .map(str::to_string)
        .collect();
    CompactionSeed {
        leading,
        user_texts,
    }
}

// ----- the summarizer's own stream -----

/// What became of one summarization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryOutcome {
    /// The summary, as the model wrote it.
    Summary(String),
    /// The summarizer's own request was larger than the window.
    Overflow(String),
    /// It failed, for the reason given.
    Failed(String),
    /// It was cancelled before it finished.
    Cancelled,
}

/// Everything one summarization produced: how it ended, what each of its
/// requests cost, and the cells of the compact hooks that ran around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryResult {
    pub outcome: SummaryOutcome,
    pub usage: Vec<TokenUsage>,
    /// `PreCompact` and `PostCompact`, when they said anything
    /// (`docs/hooks.md` *The hook-run cell*): each one's transcript heading
    /// and report.
    pub cells: Vec<(String, HookReport)>,
}

impl SummaryResult {
    /// A result that is only its outcome.
    #[must_use]
    pub const fn of(outcome: SummaryOutcome) -> Self {
        Self {
            outcome,
            usage: Vec::new(),
            cells: Vec::new(),
        }
    }
}

/// Folds a summarizer's reply events into its [`SummaryResult`] — what the
/// event loop does for an idle `/compact` turn (`App::push_chunk`'s
/// diversion, `App::withdraw_attempt`), for one that runs behind a turn.
///
/// The summarizer is an agent loop of its own with no tools, so it retries a
/// failed request, asks again for a summary that came back empty and carries
/// on one the output cap cut off (`docs/unfinished-replies.md`) — and its
/// events say so the way a turn's do: a [`StreamEvent::Retrying`] voids what
/// the failed attempt streamed, while a continuation keeps what came before
/// it.
#[derive(Debug, Default)]
pub struct SummaryCollector {
    text: String,
    /// Where the attempt now streaming began in `text` — what a retry
    /// withdraws back to (`App::track_attempt`'s rule).
    attempt: Option<usize>,
    usage: Vec<TokenUsage>,
    cells: Vec<(String, HookReport)>,
    end: Option<Result<(), String>>,
}

impl SummaryCollector {
    /// Fold one event of the summarizer's stream.
    pub fn fold(&mut self, event: StreamEvent) {
        if event.is_attempt_output() {
            self.attempt.get_or_insert(self.text.len());
        } else if !matches!(event, StreamEvent::Retrying { .. }) {
            self.attempt = None;
        }
        match event {
            StreamEvent::Chunk(chunk) => self.text.push_str(&chunk),
            StreamEvent::Retrying { .. } => {
                if let Some(start) = self.attempt.take() {
                    self.text.truncate(start);
                }
            }
            StreamEvent::Usage(usage) => self.usage.push(usage),
            StreamEvent::HookNote {
                label,
                report: Some(report),
                ..
            } => self.cells.push((label, report)),
            StreamEvent::StreamDone => self.end = Some(Ok(())),
            StreamEvent::Error(error) => self.end = Some(Err(error)),
            _ => {}
        }
    }

    /// What the stream came to. One that never ended was cancelled: a
    /// cancelled loop sends nothing.
    #[must_use]
    pub fn finish(self) -> SummaryResult {
        let outcome = match self.end {
            Some(Ok(())) => SummaryOutcome::Summary(self.text.trim().to_string()),
            Some(Err(error)) if names_context_overflow(&error) => SummaryOutcome::Overflow(error),
            Some(Err(error)) => SummaryOutcome::Failed(error),
            None => SummaryOutcome::Cancelled,
        };
        SummaryResult {
            outcome,
            usage: self.usage,
            cells: self.cells,
        }
    }
}

// ----- the compactor -----

/// One summarization to run: the summarizer's system prompt (the
/// conversation's own, when it has one), the single user message it answers
/// ([`summary_request`]), and whether the context asked for it (`auto`) or
/// the user did — the compact hooks' `trigger`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryJob {
    pub system: Option<String>,
    pub request: String,
    pub auto: bool,
}

/// A summarization in flight: where its [`SummaryResult`] arrives, and the
/// flag that stops it.
#[derive(Debug)]
pub struct SummaryHandle {
    pub result: Receiver<SummaryResult>,
    pub cancel: CancelToken,
}

/// Starts a summarization and returns at once. The backend's sends a
/// request with no tools on a thread of its own; a test's hands back a
/// channel it resolves when it likes.
pub type Summarize<'a> = dyn FnMut(SummaryJob) -> SummaryHandle + 'a;

/// Told the messages a swap left behind, each time one is made: whatever
/// else was read out of the old ones — which skills' bodies the conversation
/// still holds, say — is stale from then on.
pub type OnSwap<'a> = dyn FnMut(&[ChatMessage]) + 'a;

/// How often a wait on a summary looks at the cancel flag.
const WAIT_POLL: Duration = Duration::from_millis(20);

/// How many rounds a turn goes on for after a summary failed before it
/// starts another by itself: long enough that a provider's hiccup has
/// passed, short enough that a context still filling is caught again.
const FAILURE_COOLDOWN_ROUNDS: u32 = 3;

/// How many summaries in a row may fail before a turn stops starting them
/// by itself. A request that then overflows still gets one more try.
const MAX_FAILURES: u32 = 3;

/// How many times a summarization whose own request overflowed is asked for
/// again with half as much to read.
const MAX_SHRINKS: u32 = 3;

/// What a [`RollingCompactor`] is built over.
pub struct RollingSetup {
    /// The session's live policy: auto compaction, the window, a pending
    /// `/compact`.
    pub control: CompactionControl,
    /// Whether this conversation runs on the session's own model, and so
    /// against the session's window (`docs/agent-context-gauge.md`).
    pub inherits: bool,
    /// Whether a `/compact` typed mid-turn is this conversation's to take —
    /// the lead's, never a subagent's.
    pub takes_requests: bool,
    /// How full its context is.
    pub load: LoadMeter,
    /// The fragment the window opens with and the user's own messages so far.
    pub seed: CompactionSeed,
    pub tx: UnboundedSender<StreamEvent>,
    pub cancel: CancelToken,
}

/// A summary being written behind the turn.
struct Running {
    handle: SummaryHandle,
    /// How many messages the summarizer read: `messages[..covered]`.
    covered: usize,
    /// How many of the user's messages had been said by then.
    user_texts: usize,
    started: Instant,
    auto: bool,
    /// The transcript budget it was asked for under.
    budget: usize,
    /// How many times it has been asked for again with less.
    shrinks: u32,
    /// Whether the loop has said it is waiting on this one.
    waiting: bool,
    /// The requests its earlier, overflowed attempts were billed for.
    usage: Vec<TokenUsage>,
}

/// The compactor a real turn runs with (the module doc).
pub struct RollingCompactor<'a> {
    control: CompactionControl,
    inherits: bool,
    takes_requests: bool,
    load: LoadMeter,
    leading: Option<String>,
    user_texts: Vec<String>,
    summarize: Box<Summarize<'a>>,
    /// Told the new messages after every swap — whatever else was read out
    /// of the old ones is stale now.
    on_swap: Box<OnSwap<'a>>,
    tx: UnboundedSender<StreamEvent>,
    cancel: CancelToken,
    running: Option<Running>,
    /// Rounds left before another summary may start by itself.
    cooldown: u32,
    /// Summaries that failed in a row.
    failures: u32,
    /// How many messages the last swap left standing for what it covered —
    /// `None` until this turn has compacted. Nothing appended past them
    /// means there is nothing new to summarize.
    compacted: Option<usize>,
}

impl<'a> RollingCompactor<'a> {
    /// A compactor over `setup`, summarizing with `summarize` and telling
    /// `on_swap` each time it rewrites the messages.
    pub fn new(
        setup: RollingSetup,
        summarize: impl FnMut(SummaryJob) -> SummaryHandle + 'a,
        on_swap: impl FnMut(&[ChatMessage]) + 'a,
    ) -> Self {
        Self {
            control: setup.control,
            inherits: setup.inherits,
            takes_requests: setup.takes_requests,
            load: setup.load,
            leading: setup.seed.leading,
            user_texts: setup.seed.user_texts,
            summarize: Box::new(summarize),
            on_swap: Box::new(on_swap),
            tx: setup.tx,
            cancel: setup.cancel,
            running: None,
            cooldown: 0,
            failures: 0,
            compacted: None,
        }
    }

    fn window(&self) -> Option<u64> {
        self.control.window_for(self.inherits)
    }

    fn send(&self, event: StreamEvent) {
        let _ = self.tx.send(event);
    }

    /// Whether summarizing `messages` could take out enough to be worth a
    /// request: what the summary would replace, against what it would leave
    /// — the opening fragment, the user's kept messages, a summary's worth —
    /// and the least gain worth making ([`min_gain`]).
    fn worth_compacting(&self, messages: &[ChatMessage]) -> bool {
        let window = self.window();
        let conversation = match messages.first() {
            Some(first) if first.role == "system" => &messages[1..],
            _ => messages,
        };
        let replaced = wire_tokens(conversation);
        let kept: usize = self.leading.as_deref().map_or(0, estimate)
            + budget_user_texts(
                self.user_texts.iter().map(String::as_str),
                user_message_budget(window),
            )
            .iter()
            .map(|text| estimate(text))
            .sum::<usize>()
            + summary_max_tokens(window) / 4;
        replaced
            > u64::try_from(kept)
                .unwrap_or(u64::MAX)
                .saturating_add(min_gain(window))
    }

    /// Whether there is anything in `messages` a summary could stand in
    /// for at all — the bar a `/compact` the user typed is held to, where
    /// one the context asks for must also be [worth
    /// making](Self::worth_compacting): a conversation beyond its system
    /// prompt, and, once this turn has compacted, something said since.
    fn has_something_to_compact(&self, messages: &[ChatMessage]) -> bool {
        match self.compacted {
            Some(standing) => messages.len() > standing,
            None => messages.iter().any(|message| message.role != "system"),
        }
    }

    /// Have a summary of `messages` written, under `budget` tokens of
    /// transcript.
    fn start(&mut self, messages: &[ChatMessage], auto: bool) {
        let budget = summary_input_budget(self.window());
        let handle = self.spawn(messages, budget, auto);
        self.running = Some(Running {
            handle,
            covered: messages.len(),
            user_texts: self.user_texts.len(),
            started: Instant::now(),
            auto,
            budget,
            shrinks: 0,
            waiting: false,
            usage: Vec::new(),
        });
        self.send(StreamEvent::CompactionStarted { auto });
    }

    fn spawn(&mut self, messages: &[ChatMessage], budget: usize, auto: bool) -> SummaryHandle {
        let system = messages
            .first()
            .filter(|first| first.role == "system")
            .and_then(|first| match &first.content {
                MessageContent::Text(text) => Some(text.clone()),
                MessageContent::Parts(_) => None,
            });
        (self.summarize)(SummaryJob {
            system,
            request: summary_request(messages, budget),
            auto,
        })
    }

    /// The running summary's result, if it has landed. A summarizer that
    /// went away without one failed.
    fn landed(&mut self) -> Option<SummaryResult> {
        let running = self.running.as_ref()?;
        match running.handle.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Self::vanished()),
        }
    }

    fn vanished() -> SummaryResult {
        SummaryResult::of(SummaryOutcome::Failed(
            "the summarizer stopped before it answered".to_string(),
        ))
    }

    /// Wait for the running summary, saying so once. `None` when the turn
    /// was cancelled first — the summary is stopped with it.
    fn wait(&mut self) -> Option<SummaryResult> {
        let running = self.running.as_mut()?;
        if !running.waiting {
            running.waiting = true;
            let _ = self.tx.send(StreamEvent::CompactionWaiting);
        }
        loop {
            match running.handle.result.recv_timeout(WAIT_POLL) {
                Ok(result) => return Some(result),
                Err(RecvTimeoutError::Disconnected) => return Some(Self::vanished()),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if self.cancel.is_cancelled() {
                running.handle.cancel.cancel();
                self.running = None;
                return None;
            }
        }
    }

    /// Act on the running summary's `result`: swap it into `messages`, ask
    /// for it again with less when its own request overflowed, or say it
    /// failed. Returns whether the context is smaller for it.
    fn land(&mut self, messages: &mut Vec<ChatMessage>, result: SummaryResult) -> bool {
        let Some(mut running) = self.running.take() else {
            return false;
        };
        running.usage.extend(result.usage);
        match result.outcome {
            SummaryOutcome::Summary(summary) => {
                self.report(&running.usage, result.cells);
                self.swap(messages, &running, &summary);
                self.failures = 0;
                self.cooldown = 0;
                true
            }
            // The summarizer read too much: the same conversation, half as
            // much of it. `messages` only grew since, so what it covered is
            // still there to render again.
            SummaryOutcome::Overflow(_) if running.shrinks < MAX_SHRINKS => {
                running.budget /= 2;
                running.shrinks += 1;
                running.handle =
                    self.spawn(&messages[..running.covered], running.budget, running.auto);
                self.running = Some(running);
                false
            }
            SummaryOutcome::Overflow(reason) | SummaryOutcome::Failed(reason) => {
                self.report(&running.usage, result.cells);
                self.failures += 1;
                self.cooldown = FAILURE_COOLDOWN_ROUNDS;
                self.send(StreamEvent::CompactionFailed { reason });
                false
            }
            // The turn is going away, and its events with it.
            SummaryOutcome::Cancelled => false,
        }
    }

    /// What a summarization cost and what its hooks said, ahead of the event
    /// that says how it ended.
    fn report(&self, usage: &[TokenUsage], cells: Vec<(String, HookReport)>) {
        for usage in usage {
            self.send(StreamEvent::CompactionUsage(*usage));
        }
        for (label, report) in cells {
            self.send(StreamEvent::HookNote {
                label,
                text: String::new(),
                report: Some(report),
            });
        }
    }

    /// Put the compacted prefix in place of everything `running` covered,
    /// keeping what was appended since.
    fn swap(&mut self, messages: &mut Vec<ChatMessage>, running: &Running, summary: &str) {
        let window = self.window();
        let budget = user_message_budget(window);
        let summary = clamp_summary(summary, summary_max_tokens(window));
        let tail = messages.split_off(running.covered.min(messages.len()));
        let system = messages
            .first()
            .filter(|first| first.role == "system")
            .cloned();
        let kept = running.user_texts.min(self.user_texts.len());
        *messages = compacted_prefix(
            system,
            self.leading.as_deref(),
            &self.user_texts[..kept],
            &summary,
            budget,
        );
        self.compacted = Some(messages.len());
        messages.extend(tail);
        self.load.rebase(messages);
        (self.on_swap)(messages);
        self.send(StreamEvent::Compacted {
            summary,
            secs: running.started.elapsed().as_secs(),
            budget,
        });
    }

    /// Start the summary the user asked for with `/compact` — or say there
    /// is nothing to compact, the command's own answer at rest.
    fn start_asked_for(&mut self, messages: &[ChatMessage]) {
        if self.has_something_to_compact(messages) {
            self.start(messages, false);
        } else {
            self.send(StreamEvent::CompactionFailed {
                reason: crate::app::COMPACT_EMPTY_NOTICE.to_string(),
            });
        }
    }

    /// Whether a summary may start by itself for a context at `load`.
    fn wants_summary(&self, load: u64) -> bool {
        self.control.auto()
            && self.cooldown == 0
            && self.failures < MAX_FAILURES
            && self
                .window()
                .is_some_and(|window| load > soft_threshold(window))
    }

    /// Whether the context at `load` is past where another request may go
    /// out.
    fn past_hard(&self, load: u64) -> bool {
        self.window()
            .is_some_and(|window| load > hard_threshold(window))
    }
}

/// `summary`, cut in its middle when it is longer than `max` tokens.
fn clamp_summary(summary: &str, max: usize) -> String {
    if estimate(summary) <= max {
        summary.to_string()
    } else {
        // The cut counts four bytes a token, which runs long on code: three
        // quarters of the ceiling keeps the estimate under it.
        crate::context::truncate_middle_to_tokens(summary, max * 3 / 4)
    }
}

impl Compaction for RollingCompactor<'_> {
    fn before_round(&mut self, messages: &mut Vec<ChatMessage>) -> Flow {
        let mut manual = self.takes_requests && self.control.take_request();
        loop {
            if let Some(result) = self.landed() {
                self.land(messages, result);
            }
            let load = self.load.reading(messages);
            if self.running.is_none() {
                if manual {
                    self.start_asked_for(messages);
                } else if self.wants_summary(load) && self.worth_compacting(messages) {
                    self.start(messages, true);
                }
            }
            manual = false;
            // Past the hard limit nothing more is sent until the summary in
            // flight has landed — and what it leaves may still be too much,
            // when the rounds run meanwhile were large: go round again.
            if self.running.is_some() && self.past_hard(load) {
                match self.wait() {
                    Some(result) => {
                        self.land(messages, result);
                        continue;
                    }
                    None => return Flow::Cancelled,
                }
            }
            self.cooldown = self.cooldown.saturating_sub(1);
            return Flow::Continue;
        }
    }

    fn make_room(&mut self, messages: &mut Vec<ChatMessage>) -> Room {
        if !self.control.auto() {
            return Room::Unable;
        }
        // A summary already being written is the fastest room there is;
        // otherwise one is started now, whatever a recent failure said.
        if self.running.is_none() {
            if !self.worth_compacting(messages) {
                return Room::Unable;
            }
            self.start(messages, true);
        }
        loop {
            let Some(result) = self.wait() else {
                return Room::Cancelled;
            };
            if self.land(messages, result) {
                return Room::Made;
            }
            // Asked for again with less to read: wait for that one too.
            if self.running.is_none() {
                return Room::Unable;
            }
        }
    }

    fn before_done(&mut self, messages: &mut Vec<ChatMessage>) -> Flow {
        // A `/compact` typed while the last reply streamed has no round
        // boundary left to be taken at.
        if self.running.is_none() && self.takes_requests && self.control.take_request() {
            self.start_asked_for(messages);
        }
        while self.running.is_some() {
            match self.wait() {
                Some(result) => {
                    self.land(messages, result);
                }
                None => return Flow::Cancelled,
            }
        }
        Flow::Continue
    }

    fn user_message(&mut self, text: &str) {
        self.user_texts.push(text.to_string());
    }
}

impl Drop for RollingCompactor<'_> {
    /// The turn is over — finished, failed or cancelled — and a summary
    /// still being written has nowhere left to land.
    fn drop(&mut self) {
        if let Some(running) = &self.running {
            running.handle.cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests;
