//! Subagents — the model's `Agent` tool (`docs/agent-tool.md`).
//!
//! The **pure state** ([`AgentRun`], [`AgentStatus`]) folds a subagent's own
//! [`StreamEvent`] stream into a roster entry: its transcript
//! (`Vec<HistoryItem>` — what the agent session view renders), its live tool
//! queue, its token tally and tool-use count, and its lifecycle status. The
//! **boundary** [`AgentRegistry`] (like [`crate::background::BackgroundRegistry`])
//! is the cloneable handle the event loop and the backend share: it allocates
//! ids, keeps each running subagent's [`CancelToken`] + completion slot +
//! pending chat inputs, and carries the dedicated [`AgentEvent`] channel the
//! subagent threads report on (agents outlive turns, so their events must
//! survive the reply channel's interrupt swaps).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::app::{HistoryItem, Message, Role, ToolCall, ToolStatus, TurnSummary};
use crate::llm::ChatMessage;
use crate::llm::classifier::ClassifierContext;
use crate::stream::{CancelToken, StreamEvent};

/// The default subagent type when the model omits `subagent_type`.
pub const GENERAL_PURPOSE: &str = "general-purpose";

/// How long a naturally finished agent lingers in the footer roster with its
/// green `◯` before the boundary sweeps it (`main.rs`, the toast-deadline
/// pattern). The same 30s window as a stop, for the same reason: the row is
/// the roster's only evidence the agent ran, and swept in a few seconds it
/// could vanish before the user had read it — mid-group, even before its
/// group cell committed. `x` clears it sooner (`docs/agent-tool.md`).
pub const AGENT_LINGER: Duration = Duration::from_secs(30);

/// How long an agent the user **stopped** (`x` on its roster row) lingers
/// with its red `◯` before the same sweep. Its own constant, not a reuse of
/// [`AGENT_LINGER`]: a stop's countdown must never be restarted or shortened
/// by the group resolving after it (`or_insert` at the arming sites keys off
/// this intent), and a row that vanished on the keypress would leave no
/// evidence of what was stopped — so the stopped agent stays put, and the
/// row's `x` becomes the *clear* that removes it at once
/// (`docs/agent-tool.md`).
pub const AGENT_STOPPED_LINGER: Duration = Duration::from_secs(30);

/// A subagent's lifecycle — the tree row / footer `◯` colour and the
/// `⎿ Done` / `⎿ Interrupted` footers key off it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    /// Announced but no event has arrived yet — `⎿ Initializing…`.
    Pending,
    /// Its loop is streaming rounds / running tools.
    Running,
    /// Finished with a final response (green).
    Done,
    /// Its backend errored (red).
    Failed,
    /// Stopped by the user (`x`, Esc on the foreground group, `/clear`) (red).
    Interrupted,
}

impl AgentStatus {
    /// Has the agent reached a terminal state?
    #[must_use]
    pub const fn is_final(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Interrupted)
    }

    /// Did it finish successfully? Picks the green vs red colouring.
    #[must_use]
    pub const fn ok(self) -> bool {
        matches!(self, Self::Done)
    }

    /// The `⎿ {label}` status word shown for a settled agent.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pending => "Initializing…",
            Self::Running => "Running…",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Interrupted => "Interrupted",
        }
    }
}

/// One subagent as the roster sees it: identity, live counters, and its own
/// growing transcript. Fed by [`AgentRun::apply`] from the agent channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRun {
    /// The registry id (`a7k2m9x4q`, …).
    pub id: String,
    /// The model's short (3-5 word) task label.
    pub description: String,
    /// The `subagent_type` argument (default [`GENERAL_PURPOSE`]).
    pub agent_type: String,
    /// The full task prompt — the first user message of its transcript.
    pub prompt: String,
    /// Whether it was launched `run_in_background` (the schema default).
    pub background: bool,
    pub status: AgentStatus,
    /// How many tool calls it has started.
    pub tool_uses: usize,
    /// Its live token tally: an app-side estimate that snaps to the
    /// provider's usage frames as they arrive (the [`crate::app::App`]
    /// pattern).
    pub tokens: u64,
    /// Billed tokens summed over its usage frames (what `tokens` snaps to).
    usage_tokens: u64,
    /// Billed tokens summed over the **current turn's** usage frames — the
    /// `Done for Ns · {n} tokens` receipt the settle records
    /// ([`TurnSummary::tokens`]); reset by [`reopen`](Self::reopen) so each
    /// chat continuation gets its own receipt while `usage_tokens` stays
    /// cumulative for the roster.
    turn_usage_tokens: u64,
    /// The cache-served share of `turn_usage_tokens` — the `({c} cached)`
    /// suffix ([`TurnSummary::cached`]).
    turn_usage_cached: u64,
    /// How long it has been running — boundary-injected each frame
    /// ([`crate::app::App::set_agent_runtime`], the `set_status_times`
    /// pattern). Frozen at its final value once the agent settles.
    pub runtime: Duration,
    /// Its own conversation: the prompt as a user message, its replies, its
    /// finished tool calls — everything the agent session view renders and
    /// the Ctrl+O agent cell expands.
    pub history: Vec<HistoryItem>,
    /// The in-flight reply text (its streaming buffer).
    pub streaming: Option<String>,
    /// Its live tool calls, front running — the parallel-batch queue shape.
    pub tool_queue: VecDeque<ToolCall>,
    /// The final response text once it settles (what the caller received).
    pub result: Option<String>,
    /// The error message when it failed.
    pub error: Option<String>,
    /// Set by the user's **second** `x` (the clear) — the footer roster drops
    /// the row at once, while the entry's data stays for its group's
    /// resolution until the boundary's sweep collects it.
    pub hidden: bool,
    /// Set by the user's **first** `x` (the stop) — it buys the row the
    /// longer [`AGENT_STOPPED_LINGER`] so the red `◯` is there to be read
    /// (and cleared) instead of vanishing under the keypress.
    pub stopped_by_user: bool,
    /// The **sticky** activity line — the `{Name}: {detail}` row
    /// `activity_line` builds from the newest [`StreamEvent::ToolStart`]
    /// (a `bash` call's model-supplied `description`, else its args summary;
    /// an MCP call as `{Server}: {tool}`). Kept until the *next* tool
    /// starts, so the tree row shows what the agent is doing (or just did)
    /// rather than dropping to a generic `Working…` between calls
    /// (`docs/agent-tool.md`).
    pub last_activity: Option<String>,
    /// The **open** thinking phase's chain-of-thought, or `None` outside one
    /// — what the agent session view's strip previews while the phase runs
    /// (`ui::live_reasoning_lines`). Opened only by
    /// [`begin_reasoning`](Self::begin_reasoning), which the boundary calls
    /// only when the display is on, so with `/settings` **Hide thinking**
    /// set a delta is counted and dropped exactly as it always was
    /// (`docs/thinking-stream.md`, `docs/agent-view-streaming.md`).
    reasoning: Option<String>,
    /// Where this round's settled thinking cells landed in
    /// [`history`](Self::history), so the round's own [`StreamEvent::Usage`]
    /// frame can snap their tokenizer estimates to the provider's
    /// `reasoning_tokens` (`app::reasoning::snap_reasoning_tokens`). Cleared by
    /// each frame — one frame ends one round.
    round_reasoning: Vec<usize>,
    /// How long the open thinking phase has run — boundary-injected each
    /// frame from `Session::agent_thinking_clocks`, the
    /// [`runtime`](Self::runtime) pattern — so the session view's status line
    /// shows `Thinking for Ns` and a settle needs no clock of its own.
    /// `None` outside a phase.
    pub thinking: Option<Duration>,
    /// Set while this agent's failed request is being retried
    /// ([`StreamEvent::Retrying`]) — its session view's status line shows the
    /// same `retrying {attempt}/{max}` clause the main turn's does, and the
    /// next streamed content clears it (`docs/llm.md`).
    pub retry: Option<crate::app::RetryInfo>,
}

impl AgentRun {
    /// A fresh, just-announced agent: transcript seeded with its prompt.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        description: impl Into<String>,
        agent_type: impl Into<String>,
        prompt: impl Into<String>,
        background: bool,
    ) -> Self {
        let prompt = prompt.into();
        Self {
            id: id.into(),
            description: description.into(),
            agent_type: agent_type.into(),
            prompt: prompt.clone(),
            background,
            status: AgentStatus::Pending,
            tool_uses: 0,
            tokens: 0,
            usage_tokens: 0,
            turn_usage_tokens: 0,
            turn_usage_cached: 0,
            runtime: Duration::ZERO,
            history: vec![HistoryItem::Message(Message {
                role: Role::User,
                text: prompt,
                timestamp: String::new(),
                images: Vec::new(),
            })],
            streaming: None,
            tool_queue: VecDeque::new(),
            result: None,
            error: None,
            hidden: false,
            stopped_by_user: false,
            last_activity: None,
            reasoning: None,
            round_reasoning: Vec::new(),
            thinking: None,
            retry: None,
        }
    }

    /// Fold one of the subagent's own [`StreamEvent`]s into this entry —
    /// the roster-sized mirror of the main loop's `on_stream_event`. Returns
    /// `true` when the event settled the agent (reached a terminal status),
    /// so the boundary can post its completion notice / start the linger.
    /// Events for an already-settled agent are dropped (a late chunk racing
    /// a local kill).
    pub fn apply(&mut self, event: &StreamEvent) -> bool {
        if self.status.is_final() {
            return false;
        }
        if self.status == AgentStatus::Pending && !matches!(event, StreamEvent::StreamDone) {
            self.status = AgentStatus::Running;
        }
        match event {
            StreamEvent::Chunk(chunk) => {
                self.streaming
                    .get_or_insert_with(String::new)
                    .push_str(chunk);
                self.tokens += crate::app::count_tokens(chunk) as u64;
                // Content arrived — the reconnect is over (the main turn's
                // rule, `docs/llm.md`).
                self.retry = None;
            }
            // A SubagentStop block's continuation feedback on this agent's
            // own loop (docs/hooks.md): the reply so far becomes its own
            // message — the continuation streams a fresh one — and the note
            // stays on the agent's transcript, exactly like the main turn's.
            StreamEvent::HookNote { label, text } => {
                self.flush_segment();
                self.history
                    .push(HistoryItem::HookNote(crate::app::HookNote {
                        label: label.clone(),
                        text: text.clone(),
                        timestamp: String::new(),
                    }));
                self.tokens += crate::app::count_tokens(text) as u64;
            }
            // Never sent on an agent's channel — the prompt-submit hook fires
            // only at the main session's spawn top (docs/hooks.md); mapped so
            // the match stays total and honest if that ever changes.
            StreamEvent::PromptBlocked { .. } => {}
            // A reasoning delta: counted so the footer tally ticks while the
            // agent thinks (the status-line pattern) and — **while a phase is
            // open** — accumulated for the live `● Thinking…` block the
            // session view previews. With the display off no phase is ever
            // opened, so it stays counted-and-dropped exactly as before
            // (`docs/agent-view-streaming.md`).
            StreamEvent::ThinkingChunk(text) => {
                self.tokens += crate::app::count_tokens(text) as u64;
                self.retry = None;
                if let Some(buffer) = self.reasoning.as_mut() {
                    buffer.push_str(text);
                }
            }
            // Opaque progress — counted so the footer tally ticks while the
            // agent generates a call (the status-line pattern).
            StreamEvent::ToolCallDelta(text) => {
                self.tokens += crate::app::count_tokens(text) as u64;
            }
            StreamEvent::ToolBatch(items) => {
                self.flush_segment();
                for item in items {
                    self.tool_queue.push_back(ToolCall {
                        name: item.name.clone(),
                        args: item.args.clone(),
                        status: ToolStatus::Waiting,
                        output: String::new(),
                        timestamp: String::new(),
                        shell: false,
                        truncated: false,
                        context_output: None,
                        arguments: None,
                        approval_note: None,
                        // A subagent's parallel calls are not aggregated: its
                        // session view keeps a cell per call (`docs/mcp.md`).
                        batch: None,
                    });
                }
            }
            StreamEvent::ToolStart {
                name,
                args,
                detail,
                arguments,
            } => {
                self.flush_segment();
                self.tool_uses += 1;
                self.last_activity = Some(activity_line(name, args, detail.as_deref()));
                match self.tool_queue.front_mut() {
                    Some(front) if front.status == ToolStatus::Waiting => {
                        front.status = ToolStatus::Running;
                        front.name.clone_from(name);
                        front.args.clone_from(args);
                        front.arguments.clone_from(arguments);
                    }
                    _ => self.tool_queue.push_front(ToolCall {
                        name: name.clone(),
                        args: args.clone(),
                        arguments: arguments.clone(),
                        status: ToolStatus::Running,
                        output: String::new(),
                        timestamp: String::new(),
                        shell: false,
                        truncated: false,
                        context_output: None,
                        approval_note: None,
                        batch: None,
                    }),
                }
            }
            // The auto mode classifier allowed this agent's command: the note
            // rides the running call so its resolved cell appends the
            // provenance row, exactly like the main turn's
            // (docs/permissions.md).
            StreamEvent::ToolNote(note) => {
                if let Some(front) = self.tool_queue.front_mut()
                    && front.status == ToolStatus::Running
                {
                    front.approval_note = Some(note.clone());
                }
            }
            StreamEvent::ToolOutput(chunk) => {
                if let Some(front) = self.tool_queue.front_mut()
                    && front.status == ToolStatus::Running
                {
                    front.output.push_str(chunk);
                }
            }
            StreamEvent::ToolEnd {
                output,
                ok,
                truncated,
            } => {
                self.tokens += crate::app::count_tokens(output) as u64;
                if let Some(mut front) = self.tool_queue.pop_front() {
                    front.status = if *ok {
                        ToolStatus::Ok
                    } else {
                        ToolStatus::Failed
                    };
                    front.output = output.clone();
                    front.truncated = *truncated;
                    self.history.push(HistoryItem::Tool(front));
                }
            }
            // The user refused this agent's call at the shared permission
            // prompt: red like any failure, with the model-facing `result`
            // kept beside the cell text so the agent's own derived context
            // (its Ctrl+D view, and a continuation run over its stored
            // messages) replays what it actually read (docs/permissions.md).
            StreamEvent::ToolRejected {
                display, result, ..
            } => {
                self.tokens += crate::app::count_tokens(result) as u64;
                if let Some(mut front) = self.tool_queue.pop_front() {
                    front.status = ToolStatus::Failed;
                    front.output = display.clone();
                    front.context_output = Some(result.clone());
                    self.history.push(HistoryItem::Tool(front));
                }
            }
            // A subagent's `run_in_background` launch (its executor carries
            // the shared registry, attributed via `BgOrigin`): resolve the
            // front call as backgrounded — the shell itself lives on the
            // shared list (`docs/background.md`).
            StreamEvent::ToolBackgrounded { output, .. } => {
                if let Some(mut front) = self.tool_queue.pop_front() {
                    front.status = ToolStatus::Backgrounded;
                    front.output = output.clone();
                    self.history.push(HistoryItem::Tool(front));
                }
            }
            StreamEvent::Usage(usage) => {
                // One frame ends one round: this is where the round's
                // `Thought for …` cells trade their tokenizer estimate for
                // the provider's own `reasoning_tokens`, the main session's
                // snap over this agent's transcript
                // (`docs/thinking-stream.md`).
                let targets = std::mem::take(&mut self.round_reasoning);
                crate::app::snap_reasoning_tokens(&mut self.history, &targets, usage.reasoning);
                self.usage_tokens += usage.total();
                self.tokens = self.usage_tokens;
                self.turn_usage_tokens += usage.total();
                self.turn_usage_cached += usage.cached;
            }
            StreamEvent::StreamDone => {
                // A phase still open when the round ended keeps what streamed
                // — settled first, so its cell sits ahead of the reply it
                // preceded (`docs/thinking-stream.md`).
                self.settle_thinking();
                self.result = self.flush_segment();
                self.tool_queue.clear();
                self.status = AgentStatus::Done;
                // The turn's dim receipt, the main turn-end's exact shape
                // (`Done for 59s · 6.1k tokens (2.8k cached)`): the agent
                // session view commits it and every rebuild/Ctrl+O renders it
                // from here, while the derived context skips it as chrome
                // like any summary (docs/agent-tool.md). The runtime was
                // frozen at its live value just before this event
                // (`tui::agent`), so it is the turn's elapsed; a failure or
                // interrupt records none, main parity.
                self.history.push(HistoryItem::Summary(TurnSummary {
                    verb: "Done",
                    secs: self.runtime.as_secs(),
                    timestamp: String::new(),
                    shells: 0,
                    tokens: usize::try_from(self.turn_usage_tokens).unwrap_or(usize::MAX),
                    cached: usize::try_from(self.turn_usage_cached).unwrap_or(usize::MAX),
                }));
                return true;
            }
            StreamEvent::Error(message) => {
                // The open thought survives the failure, ahead of the partial
                // reply and the red notice (the main session's order).
                self.settle_thinking();
                self.flush_segment();
                self.resolve_tools(message);
                self.error = Some(message.clone());
                self.status = AgentStatus::Failed;
                return true;
            }
            // The ToolRejected shape's green twin: a resolution whose
            // model-facing text differs from its cell. Subagents reach it on
            // every `write`/`edit` — the file tools' one-line ack over the
            // numbered body (docs/tools.md) — and on a loaded `skill`
            // (docs/skills.md). (The ask tool sends it too, but subagents are
            // never offered that one.) Keeping both texts is what lets the
            // agent's own Ctrl+D and a continuation run replay what it read.
            StreamEvent::ToolAnswered {
                display, result, ..
            } => {
                self.tokens += crate::app::count_tokens(result) as u64;
                if let Some(mut front) = self.tool_queue.pop_front() {
                    front.status = ToolStatus::Ok;
                    front.output = display.clone();
                    front.context_output = Some(result.clone());
                    self.history.push(HistoryItem::Tool(front));
                }
            }
            // Subagents are never offered the task tools (the lead agent
            // plans, subagents execute — docs/task-tools.md), so this is
            // unreachable today: recorded as an ordinary resolved tool cell
            // on the agent's own transcript so the mapping stays total and
            // honest if that ever changes. The snapshot is ignored — the
            // checklist is the main session's.
            StreamEvent::TaskCall {
                name,
                args,
                output,
                ok,
                ..
            } => {
                self.tokens += crate::app::count_tokens(output) as u64;
                self.tool_uses += 1;
                self.history.push(HistoryItem::Tool(ToolCall {
                    name: name.clone(),
                    args: args.clone(),
                    status: if *ok {
                        ToolStatus::Ok
                    } else {
                        ToolStatus::Failed
                    },
                    output: output.clone(),
                    timestamp: String::new(),
                    shell: false,
                    truncated: false,
                    context_output: None,
                    arguments: None,
                    approval_note: None,
                    batch: None,
                }));
            }
            // A permission request is the *user's* business, not the roster's:
            // the boundary lifts it out of the agent channel and raises the
            // shared inline prompt (docs/permissions.md). Nothing about this
            // agent's own state changes while it waits. An ask request would
            // be too (docs/ask.md) — unreachable, since subagents never get
            // the tool.
            // This agent's request failed before any content streamed and is
            // being retried: its session view's status line says so, exactly
            // like the main turn's (`docs/llm.md`). Nothing commits — the run
            // is still in flight.
            StreamEvent::Retrying { attempt, max } => {
                self.retry = Some(crate::app::RetryInfo {
                    attempt: *attempt,
                    max: *max,
                });
            }
            StreamEvent::Permission(_)
            | StreamEvent::AskUser(_)
            | StreamEvent::ThinkingStart
            | StreamEvent::ThinkingEnd
            | StreamEvent::AgentBatch { .. }
            | StreamEvent::AgentGroupDone { .. } => {}
        }
        false
    }

    /// Locally settle the agent as user-stopped (`x`, Esc on its foreground
    /// group, `/clear`): resolve any running tool, keep the partial, flip to
    /// [`AgentStatus::Interrupted`]. Returns `true` when this call did the
    /// settling (`false` if it was already final).
    pub fn interrupt(&mut self) -> bool {
        if self.status.is_final() {
            return false;
        }
        // Keep a partial thought: what streamed is what the user saw.
        self.settle_thinking();
        self.flush_segment();
        self.resolve_tools(crate::app::INTERRUPT_TOOL_OUTPUT);
        self.status = AgentStatus::Interrupted;
        true
    }

    /// How long this entry lingers on the roster once it settles, before the
    /// boundary's sweep drops it. A user stop earns
    /// [`AGENT_STOPPED_LINGER`] (the red row is there to be read and
    /// cleared); everything else keeps [`AGENT_LINGER`]. Both windows are
    /// 30s today — only the *reason* differs, and the stop's constant stays
    /// separate so its countdown can never be restarted by a later group
    /// resolution.
    #[must_use]
    pub const fn linger(&self) -> Duration {
        if self.stopped_by_user {
            AGENT_STOPPED_LINGER
        } else {
            AGENT_LINGER
        }
    }

    /// A user chat message sent into the agent's session — recorded into its
    /// transcript (the registry delivers the same text to the running loop).
    pub fn push_user_message(&mut self, text: &str) {
        self.flush_segment();
        self.history.push(HistoryItem::Message(Message {
            role: Role::User,
            text: text.to_string(),
            timestamp: String::new(),
            images: Vec::new(),
        }));
    }

    /// A follow-up chat turn began (a continuation run): reopen a settled
    /// agent so new events fold in again. The turn-scoped usage counters
    /// reset — the next settle's `Done for Ns · {n} tokens` receipt is the
    /// continuation's own — while the cumulative roster tally stands.
    pub fn reopen(&mut self) {
        self.status = AgentStatus::Running;
        self.stopped_by_user = false;
        self.result = None;
        self.error = None;
        self.turn_usage_tokens = 0;
        self.turn_usage_cached = 0;
        self.round_reasoning.clear();
    }

    /// The tree row's activity: what the agent is doing — or, between tool
    /// calls, what it just did (the sticky [`last_activity`] holds until the
    /// next call starts, so the row keeps its context instead of dropping to
    /// `Working…` while the agent reasons over a result).
    ///
    /// [`last_activity`]: AgentRun::last_activity
    #[must_use]
    pub fn activity(&self) -> String {
        match self.status {
            AgentStatus::Pending => AgentStatus::Pending.label().to_string(),
            AgentStatus::Running => self
                .last_activity
                .clone()
                .unwrap_or_else(|| "Working…".to_string()),
            settled => settled.label().to_string(),
        }
    }

    /// Finalise the streamed run of text into the transcript (the
    /// `flush_streaming_segment` shape) and return it.
    fn flush_segment(&mut self) -> Option<String> {
        let text = self.streaming.take()?;
        if text.is_empty() {
            return None;
        }
        self.history.push(HistoryItem::Message(Message {
            role: Role::Assistant,
            text: text.clone(),
            timestamp: String::new(),
            images: Vec::new(),
        }));
        Some(text)
    }

    /// Open a thinking phase on this agent — the boundary calls it at its
    /// [`StreamEvent::ThinkingStart`], and **only when the display is on**
    /// (`/settings` **Hide thinking**), which is the whole gate: with it off
    /// no buffer exists and every delta stays counted-and-dropped
    /// (`docs/agent-view-streaming.md`).
    ///
    /// **Flush before you interleave** (CLAUDE.md invariant 4): a model can
    /// reason after it has begun answering, so the run of text before the
    /// phase becomes its own message here, as the live block goes up — the
    /// `ThinkingStart` dance the main session does.
    pub fn begin_reasoning(&mut self) {
        self.flush_segment();
        self.reasoning = Some(String::new());
    }

    /// The chain-of-thought streamed so far in the open phase, or `None` when
    /// none is open — what the session view's strip previews. `Some("")`
    /// right after [`begin_reasoning`](Self::begin_reasoning): the phase is
    /// real before its first delta, and the block should say so.
    #[must_use]
    pub fn reasoning(&self) -> Option<&str> {
        self.reasoning.as_deref()
    }

    /// Inject the open phase's elapsed (the boundary's thinking clock, the
    /// [`crate::app::App::set_agent_runtime`] pattern) so the session view's
    /// status line shows `Thinking for Ns` and a settle reached from the pure
    /// side ([`interrupt`](Self::interrupt), an `Error`) needs no clock.
    pub fn set_thinking(&mut self, elapsed: Option<Duration>) {
        self.thinking = elapsed;
    }

    /// Close the open thinking phase, recording it as a
    /// [`HistoryItem::Reasoning`] on **this agent's** transcript and returning
    /// it for the session view to commit as the collapsed `Thought for …`
    /// cell. `secs` is the phase's wall-clock (the boundary's clock at a
    /// `ThinkingEnd`; the injected [`thinking`](Self::thinking) at every other
    /// settle point).
    ///
    /// `None` — recording nothing — when no phase was open, or when it
    /// produced no text: some providers open and close one without a single
    /// delta, and a `Thought for 0s` cell for that is noise. The main
    /// session's rule exactly (`docs/thinking-stream.md`).
    pub fn finish_reasoning(&mut self, secs: u64) -> Option<crate::app::Reasoning> {
        self.thinking = None;
        let text = self.reasoning.take()?;
        if text.trim().is_empty() {
            return None;
        }
        let reasoning = crate::app::Reasoning {
            tokens: crate::app::count_tokens(&text),
            text,
            secs,
            timestamp: String::new(),
        };
        // Remember where it landed so this round's usage frame can snap its
        // estimate to the provider's own count.
        self.round_reasoning.push(self.history.len());
        self.history.push(HistoryItem::Reasoning(reasoning.clone()));
        Some(reasoning)
    }

    /// Settle an open phase from the injected [`thinking`](Self::thinking)
    /// elapsed — the pure settle points (`StreamDone`, `Error`,
    /// [`interrupt`](Self::interrupt)), which have no clock of their own.
    /// A no-op when no phase is open.
    fn settle_thinking(&mut self) {
        let secs = self.thinking.unwrap_or_default().as_secs();
        self.finish_reasoning(secs);
    }

    /// Resolve every live tool call as failed with `output` (an interrupt or
    /// backend error killed the run); drops un-started `Waiting` siblings.
    fn resolve_tools(&mut self, output: &str) {
        if let Some(mut front) = self.tool_queue.pop_front()
            && front.status == ToolStatus::Running
        {
            front.status = ToolStatus::Failed;
            front.output = output.to_string();
            self.history.push(HistoryItem::Tool(front));
        }
        self.tool_queue.clear();
    }
}

/// The sticky `{Name}: {detail}` activity row for one starting call — the one
/// grammar every agent activity wears (`docs/agent-tool.md`): the model's own
/// description when it gave one (`Bash: Fetching current weather…`), else the
/// call's args summary (`Write: game.py` — never the cell header's
/// `Write({args})`, whose parens read as clutter on a dim clipped one-liner).
/// An MCP call's display name is a mouthful for that row, so it leads with
/// the **capitalized server** over the tool instead (`Deepwiki:
/// ask_question`); a call with nothing to say after the colon is the bare
/// name.
fn activity_line(name: &str, args: &str, detail: Option<&str>) -> String {
    if let Some(detail) = detail {
        return format!("{name}: {detail}");
    }
    if let (Some(server), Some(tool)) = (
        crate::mcp::display_server(name),
        crate::mcp::display_tool(name),
    ) {
        return format!("{}: {tool}", crate::mcp::capitalize_server(server));
    }
    if args.is_empty() {
        return name.to_string();
    }
    format!("{name}: {args}")
}

/// The base36 alphabet agent ids are drawn from (the task-id alphabet).
const AGENT_ID_ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// A Claude-Code-style agent id — an `a` prefix + 8 lowercase base36 chars
/// (`a7k2m9x4q`), the background registry's `task_id` shape with its own
/// prefix so a shell id can never be mistaken for an agent id. Deterministic
/// in `seed` (pure, testable); the registry feeds fresh entropy per launch.
#[must_use]
pub fn agent_id(seed: u64) -> String {
    let mut mixed = splitmix64(seed);
    let mut id = String::with_capacity(9);
    id.push('a');
    for _ in 0..8 {
        id.push(AGENT_ID_ALPHABET[(mixed % 36) as usize] as char);
        mixed /= 36;
    }
    id
}

/// SplitMix64 (the `background::splitmix64` mixer — duplicated rather than
/// exported so the two modules stay independent).
fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Fresh entropy for one id roll (the `background::entropy_seed` pattern).
fn entropy_seed(counter: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let folded = (nanos as u64) ^ ((nanos >> 64) as u64);
    folded ^ (u64::from(std::process::id()) << 32) ^ counter
}

/// What a subagent thread reports on the dedicated agent channel — the
/// seventh `select!` source. Every event is tagged with its agent's id; the
/// loop folds it into the roster entry ([`AgentRun::apply`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// One event of the subagent's own stream (chunks, tool calls, usage,
    /// the terminal done/error).
    Stream { id: String, event: StreamEvent },
}

/// One running subagent's shared slot: its cancel token, completion state,
/// stored conversation (for chat continuations), and pending chat inputs.
#[derive(Default)]
struct AgentSlot {
    cancel: CancelToken,
    /// The registered subagent type (`general-purpose` / `explore`) — a chat
    /// continuation rebuilds the same tool set from it.
    agent_type: String,
    /// The run (initial or a chat continuation) is still executing.
    busy: bool,
    /// The agent has settled at least once (a result/failure is stored).
    done: bool,
    /// The user stopped it ([`AgentRegistry::kill`]).
    killed: bool,
    /// Its final response text (present once `done`, unless it failed).
    result: Option<String>,
    /// Its failure message (present once `done` when the run errored).
    failed: Option<String>,
    /// The full request message list at settle — a chat continuation resumes
    /// from here.
    messages: Option<Vec<ChatMessage>>,
    /// User chat messages awaiting delivery into the running loop (taken per
    /// round via the `pending_notices` seam).
    pending_inputs: Vec<String>,
    /// The auto-mode classifier's task context for **this agent's** run — its
    /// own launch prompt and its own executed calls, never the lead's
    /// (`docs/permissions.md`). It lives here rather than as a local in the
    /// agent's thread for one reason: the Ctrl+D classifier page reads it from
    /// the event loop while that agent's session view is open, and a page that
    /// showed the lead's log there would be describing decisions no one on
    /// screen made. Shared with the thread, which seeds it from the launch
    /// prompt and records one line per call; a chat continuation pushes onto
    /// the same rolling windows, because it is the same agent's conversation.
    classifier: Arc<Mutex<ClassifierContext>>,
}

struct Inner {
    /// The launch counter mixed into each id roll.
    next_id: u64,
    slots: HashMap<String, AgentSlot>,
}

/// The shared subagent registry (see the module docs). Cloneable like the
/// background registry; the boundary (`main.rs`) creates it with the agent
/// channel's sender and hands clones to the backend.
#[derive(Clone)]
pub struct AgentRegistry {
    inner: Arc<Mutex<Inner>>,
    events: UnboundedSender<AgentEvent>,
}

impl std::fmt::Debug for AgentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRegistry").finish_non_exhaustive()
    }
}

impl AgentRegistry {
    /// A registry that reports subagent events on `events`.
    #[must_use]
    pub fn new(events: UnboundedSender<AgentEvent>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                next_id: 0,
                slots: HashMap::new(),
            })),
            events,
        }
    }

    /// Register a fresh subagent of `agent_type`: allocate its id (collision
    /// re-roll) and its cancel token. The caller spawns the thread.
    #[must_use]
    pub fn register(&self, agent_type: &str) -> (String, CancelToken) {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let id = loop {
            let seed = entropy_seed(inner.next_id);
            inner.next_id = inner.next_id.wrapping_add(1);
            let id = agent_id(seed);
            if !inner.slots.contains_key(&id) {
                break id;
            }
        };
        let cancel = CancelToken::new();
        inner.slots.insert(
            id.clone(),
            AgentSlot {
                classifier: Arc::new(Mutex::new(ClassifierContext::new())),
                cancel: cancel.clone(),
                busy: true,
                agent_type: agent_type.to_string(),
                ..AgentSlot::default()
            },
        );
        (id, cancel)
    }

    /// The shared handle to `id`'s classifier context — for the agent's own
    /// thread, which seeds it from the launch prompt and records one line per
    /// executed call. `None` once the slot is gone.
    #[must_use]
    pub fn classifier_handle(&self, id: &str) -> Option<Arc<Mutex<ClassifierContext>>> {
        let inner = self.inner.lock().expect("agent registry poisoned");
        inner.slots.get(id).map(|slot| Arc::clone(&slot.classifier))
    }

    /// `id`'s classifier context, rendered — exactly what its own auto-mode
    /// verdicts are reviewed against. This is what the Ctrl+D classifier page
    /// shows while that agent's session view is open; `None` for an agent the
    /// registry no longer holds, which the page renders as its empty
    /// placeholder rather than falling back to the lead's log
    /// (`docs/permissions.md`).
    #[must_use]
    pub fn classifier_context(&self, id: &str) -> Option<String> {
        // Take the handle first, so the registry lock is released before the
        // context's own is acquired: the draw tick calls this every frame the
        // classifier page is open, and it must never queue behind the whole
        // map while an agent thread records a call.
        let handle = self.classifier_handle(id)?;
        handle.lock().ok().map(|log| log.render())
    }

    /// The registered subagent type (for a chat continuation's tool set).
    #[must_use]
    pub fn agent_type(&self, id: &str) -> String {
        let inner = self.inner.lock().expect("agent registry poisoned");
        inner
            .slots
            .get(id)
            .map(|slot| slot.agent_type.clone())
            .unwrap_or_else(|| GENERAL_PURPOSE.to_string())
    }

    /// Send one subagent event to the loop (used by the forwarder threads).
    pub fn send(&self, event: AgentEvent) {
        let _ = self.events.send(event);
    }

    /// A run finished: store its outcome + final message list so the parent's
    /// wait loop resolves and a chat continuation can resume. A slot the user
    /// already killed keeps its killed outcome (the thread noticed late).
    pub fn finish(&self, id: &str, outcome: Result<String, String>, messages: Vec<ChatMessage>) {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        if let Some(slot) = inner.slots.get_mut(id) {
            slot.busy = false;
            slot.done = true;
            slot.messages = Some(messages);
            if !slot.killed {
                match outcome {
                    Ok(result) => slot.result = Some(result),
                    Err(error) => slot.failed = Some(error),
                }
            }
        }
    }

    /// Stop one agent: cancel its token and mark it killed **and settled**,
    /// so the parent's wait loop resolves at once instead of waiting for the
    /// thread to notice (a blocking read can take seconds). Returns whether
    /// the id was a live, not-yet-settled agent.
    pub fn kill(&self, id: &str) -> bool {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let Some(slot) = inner.slots.get_mut(id) else {
            return false;
        };
        slot.cancel.cancel();
        let was_live = !slot.done;
        slot.killed = true;
        slot.done = true;
        was_live
    }

    /// Stop every agent (`/clear`, quit).
    pub fn kill_all(&self) {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        for slot in inner.slots.values_mut() {
            slot.cancel.cancel();
            slot.killed = true;
            slot.done = true;
        }
    }

    /// Drop an agent's slot entirely (its roster entry was swept).
    pub fn remove(&self, id: &str) {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        inner.slots.remove(id);
    }

    /// Has this agent's current run settled (finished, failed, or killed)?
    #[must_use]
    pub fn is_done(&self, id: &str) -> bool {
        let inner = self.inner.lock().expect("agent registry poisoned");
        inner.slots.get(id).is_none_or(|slot| slot.done)
    }

    /// Was this agent stopped by the user?
    #[must_use]
    pub fn is_killed(&self, id: &str) -> bool {
        let inner = self.inner.lock().expect("agent registry poisoned");
        inner.slots.get(id).is_some_and(|slot| slot.killed)
    }

    /// The settled outcome for the parent's tool result: `Ok(final response)`
    /// / `Err(error)` — `Err("stopped by the user")` for a kill, `None` while
    /// still running.
    #[must_use]
    pub fn outcome(&self, id: &str) -> Option<Result<String, String>> {
        let inner = self.inner.lock().expect("agent registry poisoned");
        let slot = inner.slots.get(id)?;
        if !slot.done {
            return None;
        }
        if slot.killed {
            return Some(Err("stopped by the user".to_string()));
        }
        if let Some(error) = &slot.failed {
            return Some(Err(error.clone()));
        }
        Some(Ok(slot.result.clone().unwrap_or_default()))
    }

    /// Queue a user chat message for a **running** agent — delivered into its
    /// loop at the next round boundary (the `pending_notices` seam). Returns
    /// `false` when the agent isn't currently running a turn (the caller
    /// should spawn a continuation instead).
    #[must_use]
    pub fn queue_input(&self, id: &str, text: &str) -> bool {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        match inner.slots.get_mut(id) {
            Some(slot) if slot.busy && !slot.killed => {
                slot.pending_inputs.push(text.to_string());
                true
            }
            _ => false,
        }
    }

    /// Drain the pending chat inputs for one agent (its loop calls this per
    /// round).
    #[must_use]
    pub fn take_pending_inputs(&self, id: &str) -> Vec<String> {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        inner
            .slots
            .get_mut(id)
            .map(|slot| std::mem::take(&mut slot.pending_inputs))
            .unwrap_or_default()
    }

    /// Begin a chat continuation on a settled agent: reclaim its stored
    /// message list, mark it busy again with a fresh cancel token, and return
    /// what the continuation thread needs. `None` while it is still busy or
    /// no conversation is stored.
    #[must_use]
    pub fn begin_continuation(&self, id: &str) -> Option<(Vec<ChatMessage>, CancelToken)> {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let slot = inner.slots.get_mut(id)?;
        if slot.busy {
            return None;
        }
        let messages = slot.messages.take()?;
        let cancel = CancelToken::new();
        slot.cancel = cancel.clone();
        slot.busy = true;
        slot.done = false;
        slot.killed = false;
        slot.result = None;
        slot.failed = None;
        Some((messages, cancel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{TokenUsage, ToolCallSummary};

    fn chunk(text: &str) -> StreamEvent {
        StreamEvent::Chunk(text.to_string())
    }

    #[test]
    fn agent_id_is_a_deterministic_a_plus_eight_base36() {
        let id = agent_id(42);
        assert_eq!(id, agent_id(42), "deterministic in the seed");
        assert_eq!(id.len(), 9);
        assert!(id.starts_with('a'));
        assert!(
            id.chars()
                .skip(1)
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
        assert_ne!(agent_id(1), agent_id(2), "different seeds differ");
    }

    #[test]
    fn new_agent_seeds_its_transcript_with_the_prompt() {
        let run = AgentRun::new(
            "a1",
            "Fetch weather",
            GENERAL_PURPOSE,
            "Get Warsaw weather",
            true,
        );
        assert_eq!(run.status, AgentStatus::Pending);
        assert_eq!(run.activity(), "Initializing…");
        assert_eq!(run.history.len(), 1);
        let HistoryItem::Message(m) = &run.history[0] else {
            panic!("prompt seeds a user message");
        };
        assert_eq!(m.role, Role::User);
        assert_eq!(m.text, "Get Warsaw weather");
    }

    #[test]
    fn apply_keeps_the_classifier_note_on_the_resolved_call() {
        // A subagent's command allowed by the auto mode classifier
        // (docs/permissions.md): the ToolNote lands on the running front and
        // rides into the transcript, so the agent's own cells show the same
        // provenance row as the main turn's.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::ToolStart {
            name: "Bash".to_string(),
            args: "ls -la".to_string(),
            detail: None,
            arguments: None,
        });
        run.apply(&StreamEvent::ToolNote(
            "Allowed by auto mode classifier".to_string(),
        ));
        run.apply(&StreamEvent::ToolEnd {
            output: "Exit code: 0\ntotal 40".to_string(),
            ok: true,
            truncated: false,
        });
        let HistoryItem::Tool(tool) = run.history.last().expect("the call recorded") else {
            panic!("a tool call lands in the transcript");
        };
        assert_eq!(
            tool.approval_note.as_deref(),
            Some("Allowed by auto mode classifier")
        );
    }

    #[test]
    fn apply_folds_a_full_turn_into_the_transcript() {
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        assert!(!run.apply(&chunk("Let me check. ")));
        assert_eq!(run.status, AgentStatus::Running);
        assert!(run.tokens > 0, "chunks tick the tally");
        // A tool round: batch announced, runs, resolves.
        assert!(!run.apply(&StreamEvent::ToolBatch(vec![ToolCallSummary {
            name: "Bash".into(),
            args: "curl wttr.in".into(),
        }])));
        assert!(!run.apply(&StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "curl wttr.in".into(),
            detail: Some("Fetching Warsaw weather".into()),
            arguments: None,
        }));
        assert_eq!(run.tool_uses, 1);
        assert_eq!(
            run.activity(),
            "Bash: Fetching Warsaw weather",
            "the model's own description leads the activity line"
        );
        assert!(!run.apply(&StreamEvent::ToolOutput("+19°C\n".into())));
        assert!(!run.apply(&StreamEvent::ToolEnd {
            output: "+19°C".into(),
            ok: true,
            truncated: false,
        }));
        // The pre-tool text flushed as its own message, the tool follows it.
        assert!(matches!(
            (&run.history[1], &run.history[2]),
            (HistoryItem::Message(m), HistoryItem::Tool(t))
                if m.role == Role::Assistant && t.status == ToolStatus::Ok
        ));
        assert!(!run.apply(&chunk("It is 19°C.")));
        assert!(run.apply(&StreamEvent::StreamDone), "done settles it");
        assert_eq!(run.status, AgentStatus::Done);
        assert_eq!(run.result.as_deref(), Some("It is 19°C."));
        assert_eq!(run.activity(), "Done");
        // Late events after settling are dropped.
        assert!(!run.apply(&chunk("late")));
        assert_eq!(run.result.as_deref(), Some("It is 19°C."));
    }

    #[test]
    fn the_sticky_activity_is_always_the_clean_name_colon_detail_row() {
        // Every activity row wears one grammar — `{Name}: {detail}`: the
        // model's own description when it gave one (`Bash: Fetching…`), else
        // the call's args summary (`Write: game.py` — never the header's
        // `Write(game.py)`, whose parens read as clutter on a dim one-liner;
        // `docs/agent-tool.md`).
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        assert!(!run.apply(&StreamEvent::ToolStart {
            name: "Write".into(),
            args: "game.py".into(),
            detail: None,
            arguments: None,
        }));
        assert_eq!(run.activity(), "Write: game.py");
        assert!(!run.apply(&StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "curl wttr.in".into(),
            detail: Some("Fetching Warsaw weather".into()),
            arguments: None,
        }));
        assert_eq!(run.activity(), "Bash: Fetching Warsaw weather");
        // Nothing to say after the colon → the bare name, not `Name: `.
        assert!(!run.apply(&StreamEvent::ToolStart {
            name: "TaskList".into(),
            args: String::new(),
            detail: None,
            arguments: None,
        }));
        assert_eq!(run.activity(), "TaskList");
    }

    #[test]
    fn an_mcp_calls_activity_leads_with_the_server_name() {
        // An MCP call's display name (`deepwiki - ask_question (MCP)`) is a
        // mouthful for a clipped dim row: the activity shows the capitalized
        // server over the tool instead — `Deepwiki: ask_question`
        // (`docs/agent-tool.md`).
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        assert!(!run.apply(&StreamEvent::ToolStart {
            name: "deepwiki - ask_question (MCP)".into(),
            args: "{\"repoName\":\"facebook/react\"}".into(),
            detail: None,
            arguments: None,
        }));
        assert_eq!(run.activity(), "Deepwiki: ask_question");
    }

    #[test]
    fn a_rejected_call_keeps_the_model_facing_result_on_the_agents_transcript() {
        // A subagent's request goes to the same shared prompt, so its
        // transcript needs the same two texts: the cell line, and what the
        // agent itself read — its Ctrl+D view and any continuation run derive
        // from this history (docs/permissions.md).
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::ToolStart {
            name: "Write".into(),
            args: "hello.py".into(),
            detail: None,
            arguments: None,
        });
        assert!(!run.apply(&StreamEvent::ToolRejected {
            display: "User rejected write to hello.py\nInstructions: use pathlib".into(),
            result: "The user doesn't want to proceed… instructions instead: use pathlib".into(),
            truncated: false,
        }));
        let Some(HistoryItem::Tool(tool)) = run.history.last() else {
            panic!("the refused call still lands on the transcript");
        };
        assert_eq!(tool.status, ToolStatus::Failed);
        assert_eq!(
            tool.output,
            "User rejected write to hello.py\nInstructions: use pathlib"
        );
        assert_eq!(
            tool.context_text(),
            "The user doesn't want to proceed… instructions instead: use pathlib"
        );
        assert!(run.tokens > 0, "the uploaded instruction ticks the tally");
    }

    #[test]
    fn stream_done_records_the_turns_summary_on_the_transcript() {
        // The agent session view ends its turns the way the main one does: a
        // dim `Done for 59s · 6.1k tokens (2.8k cached)` summary lands on the
        // agent's own transcript at StreamDone, carrying the turn's billed
        // usage (`docs/agent-tool.md`).
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.runtime = Duration::from_secs(59);
        run.apply(&chunk("It is 19°C."));
        run.apply(&StreamEvent::Usage(TokenUsage {
            input: 6000,
            output: 100,
            cached: 2800,
            ..TokenUsage::default()
        }));
        assert!(run.apply(&StreamEvent::StreamDone));
        let Some(HistoryItem::Summary(summary)) = run.history.last() else {
            panic!("the settle records the summary: {:?}", run.history.last());
        };
        assert_eq!(summary.verb, "Done", "the view's synthesized done verb");
        assert_eq!(summary.secs, 59);
        assert_eq!(summary.tokens, 6100);
        assert_eq!(summary.cached, 2800);
        assert_eq!(summary.shells, 0, "shells are the main session's business");
        // The summary sits AFTER the flushed final reply.
        assert!(matches!(
            &run.history[run.history.len() - 2],
            HistoryItem::Message(m) if m.text == "It is 19°C."
        ));
    }

    #[test]
    fn a_continuation_turns_summary_counts_only_its_own_usage() {
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&chunk("first"));
        run.apply(&StreamEvent::Usage(TokenUsage {
            input: 900,
            output: 100,
            cached: 400,
            ..TokenUsage::default()
        }));
        run.apply(&StreamEvent::StreamDone);
        run.push_user_message("and Manila?");
        run.reopen();
        run.apply(&chunk("second"));
        run.apply(&StreamEvent::Usage(TokenUsage {
            input: 450,
            output: 50,
            cached: 0,
            ..TokenUsage::default()
        }));
        run.apply(&StreamEvent::StreamDone);
        let summaries: Vec<_> = run
            .history
            .iter()
            .filter_map(|item| match item {
                HistoryItem::Summary(s) => Some((s.tokens, s.cached)),
                _ => None,
            })
            .collect();
        assert_eq!(
            summaries,
            [(1000, 400), (500, 0)],
            "each turn's summary is its own receipt"
        );
        assert_eq!(run.tokens, 1500, "the roster tally stays cumulative");
    }

    #[test]
    fn a_failed_or_interrupted_turn_records_no_summary() {
        // Main parity: an error commits its red notice, an interrupt its
        // record — neither earns a `Done for Ns` receipt.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&chunk("partial"));
        run.apply(&StreamEvent::Error("boom".into()));
        assert!(
            !run.history
                .iter()
                .any(|item| matches!(item, HistoryItem::Summary(_))),
            "no summary on a failure"
        );
        let mut run = AgentRun::new("a2", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&chunk("partial"));
        run.interrupt();
        assert!(
            !run.history
                .iter()
                .any(|item| matches!(item, HistoryItem::Summary(_))),
            "no summary on an interrupt"
        );
    }

    #[test]
    fn usage_frames_snap_the_estimated_tally() {
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&chunk("some streamed text here"));
        let est = run.tokens;
        assert!(est > 0);
        run.apply(&StreamEvent::Usage(TokenUsage {
            input: 900,
            output: 100,
            cached: 0,
            cache_write: 0,
            ..TokenUsage::default()
        }));
        assert_eq!(run.tokens, 1000, "snapped to the billed total");
        run.apply(&StreamEvent::Usage(TokenUsage {
            input: 400,
            output: 100,
            cached: 0,
            cache_write: 0,
            ..TokenUsage::default()
        }));
        assert_eq!(run.tokens, 1500, "frames accumulate");
    }

    #[test]
    fn error_settles_the_agent_failed_and_resolves_its_tool() {
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "sleep 99".into(),
            detail: None,
            arguments: None,
        });
        assert!(run.apply(&StreamEvent::Error("boom".into())));
        assert_eq!(run.status, AgentStatus::Failed);
        assert_eq!(run.error.as_deref(), Some("boom"));
        assert!(matches!(
            run.history.last(),
            Some(HistoryItem::Tool(t)) if t.status == ToolStatus::Failed
        ));
        assert!(run.tool_queue.is_empty());
    }

    #[test]
    fn interrupt_settles_locally_and_keeps_the_partial() {
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&chunk("partial "));
        assert!(run.interrupt());
        assert_eq!(run.status, AgentStatus::Interrupted);
        assert_eq!(run.activity(), "Interrupted");
        assert!(matches!(
            run.history.last(),
            Some(HistoryItem::Message(m)) if m.text == "partial "
        ));
        assert!(!run.interrupt(), "idempotent");
    }

    #[test]
    fn chat_reopens_a_settled_agent() {
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&chunk("hi"));
        run.apply(&StreamEvent::StreamDone);
        assert_eq!(run.status, AgentStatus::Done);
        run.push_user_message("and in Manila?");
        run.reopen();
        assert_eq!(run.status, AgentStatus::Running);
        assert!(run.result.is_none());
        assert!(!run.apply(&chunk("28°C")));
        assert!(run.apply(&StreamEvent::StreamDone));
        assert_eq!(run.result.as_deref(), Some("28°C"));
        // The transcript kept the whole exchange in order — each turn closed
        // by its own `Done for Ns` summary (`docs/agent-tool.md`).
        let roles: Vec<&str> = run
            .history
            .iter()
            .map(|item| match item {
                HistoryItem::Message(m) if m.role == Role::User => "user",
                HistoryItem::Message(_) => "assistant",
                HistoryItem::Summary(_) => "summary",
                _ => "other",
            })
            .collect();
        assert_eq!(
            roles,
            [
                "user",
                "assistant",
                "summary",
                "user",
                "assistant",
                "summary"
            ]
        );
    }

    fn test_registry() -> AgentRegistry {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        AgentRegistry::new(tx)
    }

    #[test]
    fn register_allocates_unique_ids_and_finish_stores_the_outcome() {
        let registry = test_registry();
        let (id1, _c1) = registry.register(GENERAL_PURPOSE);
        let (id2, _c2) = registry.register(GENERAL_PURPOSE);
        assert_ne!(id1, id2);
        assert!(!registry.is_done(&id1), "busy while running");
        registry.finish(&id1, Ok("the answer".into()), vec![ChatMessage::user("p")]);
        assert!(registry.is_done(&id1));
        assert_eq!(registry.outcome(&id1), Some(Ok("the answer".into())));
        assert_eq!(registry.outcome(&id2), None, "still running");
    }

    #[test]
    fn each_agent_gets_its_own_classifier_context() {
        // A subagent's auto-mode verdicts are reviewed against ITS run — the
        // launch prompt and the calls it has made — not the lead's
        // (`docs/permissions.md`). The registry is where that context has to
        // live, because the Ctrl+D classifier page reads it from outside the
        // agent's thread while the agent's session view is open.
        let registry = test_registry();
        let (id1, _c1) = registry.register(GENERAL_PURPOSE);
        let (id2, _c2) = registry.register(GENERAL_PURPOSE);
        let handle = registry.classifier_handle(&id1).expect("registered");
        {
            let mut log = handle.lock().expect("poisoned");
            log.push_request("count the rust files");
            log.record_call("bash", r#"{"command":"ls -1 src"}"#);
        }
        let first = registry.classifier_context(&id1).expect("registered");
        assert!(
            first.contains("count the rust files") && first.contains("ls -1 src"),
            "the agent's own request and action are in its context: {first:?}"
        );
        let second = registry.classifier_context(&id2).expect("registered");
        assert!(
            !second.contains("ls -1 src"),
            "a sibling agent's actions never leak into it: {second:?}"
        );
    }

    #[test]
    fn an_unknown_agent_has_no_classifier_context() {
        // The page falls back to its empty placeholder rather than to the
        // lead's context — showing the wrong agent's review log is the bug
        // this exists to fix.
        let registry = test_registry();
        assert!(registry.classifier_context("nope").is_none());
        assert!(registry.classifier_handle("nope").is_none());
    }

    #[test]
    fn removing_an_agent_drops_its_classifier_context() {
        let registry = test_registry();
        let (id, _cancel) = registry.register(GENERAL_PURPOSE);
        assert!(registry.classifier_context(&id).is_some());
        registry.remove(&id);
        assert!(registry.classifier_context(&id).is_none());
    }

    #[test]
    fn kill_settles_at_once_and_wins_over_a_late_finish() {
        let registry = test_registry();
        let (id, cancel) = registry.register(GENERAL_PURPOSE);
        assert!(registry.kill(&id));
        assert!(cancel.is_cancelled(), "the token cancels the thread");
        assert!(registry.is_done(&id), "resolved without the thread");
        assert!(registry.is_killed(&id));
        assert_eq!(
            registry.outcome(&id),
            Some(Err("stopped by the user".into()))
        );
        // The thread notices later and reports — the kill outcome stands.
        registry.finish(&id, Ok("late result".into()), Vec::new());
        assert_eq!(
            registry.outcome(&id),
            Some(Err("stopped by the user".into()))
        );
        assert!(!registry.kill(&id), "second kill reports not-live");
    }

    #[test]
    fn chat_inputs_queue_while_busy_and_continuations_resume_the_stored_list() {
        let registry = test_registry();
        let (id, _cancel) = registry.register(GENERAL_PURPOSE);
        assert!(registry.queue_input(&id, "also check Manila"));
        assert_eq!(registry.take_pending_inputs(&id), vec!["also check Manila"]);
        assert!(registry.take_pending_inputs(&id).is_empty(), "drained");
        assert!(
            registry.begin_continuation(&id).is_none(),
            "no continuation while busy"
        );
        registry.finish(&id, Ok("done".into()), vec![ChatMessage::user("p")]);
        assert!(
            !registry.queue_input(&id, "x"),
            "not busy — caller must spawn a continuation"
        );
        let (messages, cancel) = registry.begin_continuation(&id).expect("stored list");
        assert_eq!(messages.len(), 1);
        assert!(!cancel.is_cancelled());
        assert!(!registry.is_done(&id), "busy again");
        registry.finish(&id, Ok("follow-up".into()), messages);
        assert_eq!(registry.outcome(&id), Some(Ok("follow-up".into())));
    }

    #[test]
    fn kill_all_sweeps_every_slot_and_remove_drops_one() {
        let registry = test_registry();
        let (id1, c1) = registry.register(GENERAL_PURPOSE);
        let (id2, c2) = registry.register(GENERAL_PURPOSE);
        registry.kill_all();
        assert!(c1.is_cancelled() && c2.is_cancelled());
        assert!(registry.is_done(&id1) && registry.is_done(&id2));
        registry.remove(&id1);
        assert!(registry.is_done(&id1), "unknown ids read as done");
        assert!(registry.is_killed(&id2));
    }

    // ===== The agent's thinking stream (docs/agent-view-streaming.md) =====

    #[test]
    fn every_resolution_leaves_the_resolved_cell_last_on_the_transcript() {
        // The session view commits what the fold **recorded**, not what event
        // arrived (`docs/agent-view-streaming.md`), so every way a call can
        // resolve must leave its cell as the transcript's last item — else
        // the view drops it and only a resize brings it back. That is the
        // reported bug: `write`/`edit` moved onto `ToolAnswered` and the old
        // event-keyed arm never listed it.
        let start = StreamEvent::ToolStart {
            name: "Write".to_string(),
            args: "f.py".to_string(),
            detail: None,
            arguments: None,
        };
        let resolutions: [(&str, StreamEvent); 4] = [
            (
                "ToolEnd",
                StreamEvent::ToolEnd {
                    output: "out".to_string(),
                    ok: true,
                    truncated: false,
                },
            ),
            (
                "ToolAnswered",
                StreamEvent::ToolAnswered {
                    display: "Wrote 1 lines to f.py".to_string(),
                    result: "File created successfully at: f.py".to_string(),
                    truncated: false,
                },
            ),
            (
                "ToolRejected",
                StreamEvent::ToolRejected {
                    display: "User rejected write to f.py".to_string(),
                    result: "the user refused".to_string(),
                    truncated: false,
                },
            ),
            (
                "ToolBackgrounded",
                StreamEvent::ToolBackgrounded {
                    id: "b1".to_string(),
                    output: "Running in the background".to_string(),
                },
            ),
        ];
        for (name, event) in resolutions {
            let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
            run.apply(&start);
            let before = run.history.len();
            run.apply(&event);
            assert_eq!(run.history.len(), before + 1, "{name} recorded no cell");
            assert!(
                matches!(run.history.last(), Some(HistoryItem::Tool(_))),
                "{name} did not leave its cell last: {:?}",
                run.history
            );
            assert!(run.tool_queue.is_empty(), "{name} left the call live");
        }
    }

    #[test]
    fn a_backend_error_leaves_the_call_it_killed_last() {
        // The failure resolves the running call red — and the session view
        // commits that cell before the red notice, which is the one thing a
        // failure does *not* record (`docs/agent-view-streaming.md`).
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::ToolStart {
            name: "Bash".to_string(),
            args: "sleep 30".to_string(),
            detail: None,
            arguments: None,
        });
        run.apply(&StreamEvent::Error("boom".to_string()));
        let Some(HistoryItem::Tool(tool)) = run.history.last() else {
            panic!("the killed call is the last item: {:?}", run.history);
        };
        assert_eq!(tool.status, ToolStatus::Failed);
        assert!(
            !run.history
                .iter()
                .any(|item| matches!(item, HistoryItem::Message(m) if m.role == Role::Error)),
            "the notice is the view's, not the transcript's"
        );
    }

    #[test]
    fn a_retrying_agent_says_so_and_the_next_content_clears_it() {
        // Main parity (`docs/llm.md`): a subagent reconnecting shows
        // `retrying {n}/{max}` in its session view's status line instead of a
        // bare spinner, and the first streamed content takes it back down.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::Retrying { attempt: 2, max: 5 });
        assert_eq!(
            run.retry,
            Some(crate::app::RetryInfo { attempt: 2, max: 5 })
        );
        run.apply(&chunk("here we go"));
        assert!(run.retry.is_none(), "content ends the reconnect");
    }

    #[test]
    fn a_thinking_delta_is_only_buffered_while_a_phase_is_open() {
        // The display gate lives at the boundary: with it off nothing calls
        // `begin_reasoning`, so a delta is counted and dropped, exactly as
        // before (docs/thinking-stream.md).
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::ThinkingChunk("unseen".into()));
        assert!(run.reasoning().is_none());
        assert!(run.tokens > 0, "still counted");
        run.begin_reasoning();
        run.apply(&StreamEvent::ThinkingChunk("weighing ".into()));
        run.apply(&StreamEvent::ThinkingChunk("it".into()));
        assert_eq!(run.reasoning(), Some("weighing it"));
    }

    #[test]
    fn opening_a_phase_finalises_the_text_before_it() {
        // Flush before you interleave: a model that reasons *after* it has
        // begun answering must not have the cell spliced into the paragraph
        // that was streaming (CLAUDE.md invariant 4).
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&chunk("Let me think. "));
        run.begin_reasoning();
        assert!(run.streaming.is_none(), "the segment was flushed");
        let HistoryItem::Message(m) = &run.history[1] else {
            panic!("the text became its own message");
        };
        assert_eq!(m.text, "Let me think. ");
    }

    #[test]
    fn the_rounds_usage_frame_snaps_the_thought_to_the_providers_count() {
        // The main session's snap (docs/thinking-stream.md): the tokenizer
        // estimate gives way to `completion_tokens_details.reasoning_tokens`.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.begin_reasoning();
        run.apply(&StreamEvent::ThinkingChunk(
            "a long chain of thought".into(),
        ));
        let settled = run.finish_reasoning(2).expect("text settles");
        assert!(settled.tokens > 0, "estimated first");
        run.apply(&StreamEvent::Usage(TokenUsage {
            input: 10,
            output: 20,
            cached: 0,
            cache_write: 0,
            reasoning: 169,
        }));
        let HistoryItem::Reasoning(r) = &run.history[1] else {
            panic!("the cell is on the transcript");
        };
        assert_eq!(r.tokens, 169, "snapped to the provider's own count");
    }

    #[test]
    fn an_interrupt_keeps_the_partial_thought() {
        // An Esc/`x` mid-thought settles the phase rather than dropping it —
        // what streamed is what the user saw.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.begin_reasoning();
        run.apply(&StreamEvent::ThinkingChunk("half a thought".into()));
        run.set_thinking(Some(Duration::from_secs(4)));
        assert!(run.interrupt());
        let HistoryItem::Reasoning(r) = &run.history[1] else {
            panic!("the partial thought settled: {:?}", run.history);
        };
        assert_eq!(r.text, "half a thought");
        assert_eq!(r.secs, 4, "the injected elapsed is the phase's");
        assert!(run.reasoning().is_none());
    }

    #[test]
    fn a_backend_error_settles_the_open_phase_first() {
        // The thought is recorded ahead of the partial reply and the failure
        // — and *ahead of any text still buffered*, which is the order the
        // boundary commits in too (`Session::settle_agent_reasoning` runs
        // before the segment flush). Screen and history must agree or a
        // rebuild reorders them (CLAUDE.md invariant 4).
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.begin_reasoning();
        run.apply(&StreamEvent::ThinkingChunk("thinking".into()));
        run.apply(&chunk("half an answer"));
        run.apply(&StreamEvent::Error("boom".into()));
        let kinds: Vec<&str> = run
            .history
            .iter()
            .map(|item| match item {
                HistoryItem::Reasoning(_) => "reasoning",
                HistoryItem::Message(_) => "message",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            ["message", "reasoning", "message"],
            "prompt, the thought, then the partial: {:?}",
            run.history
        );
    }
}
