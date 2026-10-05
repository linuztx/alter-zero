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
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use crate::app::{HistoryItem, Message, Role, StatusVerb, ToolCall, ToolStatus, TurnSummary};
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

/// How many **settled** agents stay resumable — their conversations in the
/// registry, their swept roster entries in `App` — so the lead can still send
/// a finished agent a follow-up (`docs/agent-tools.md` *Retention*). The
/// oldest settled one goes first; a running agent never counts.
pub const AGENT_RETAINED_MAX: usize = 16;

/// The most tool calls an `agentoutput` report lists — the newest; the
/// earlier ones are counted on one line (`docs/agent-tools.md`).
pub const AGENT_REPORT_CALLS_MAX: usize = 30;

/// The most characters a reported tool call keeps, on its one line.
pub const AGENT_REPORT_CALL_CHARS: usize = 160;

/// The line an `agentoutput` report marks a delivered message with — where,
/// among the agent's calls, a message reached it: the lead's `agentsend` or a
/// background shell's completion routed to it. One the user typed into its
/// session is quoted instead ([`FollowUp::from_user`], `docs/agent-tools.md`).
pub const AGENT_FOLLOW_UP_MARK: &str = "— message received —";

/// Where a delivered message came from, as the lead must know it: a message
/// the **user** typed into the agent's session view is one the lead never
/// wrote, so it is quoted rather than marked ([`FollowUp::from_user`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUp {
    /// How many of the agent's calls it had made when the message arrived.
    pub at: usize,
    /// The text, when the user sent it; `None` for the lead's own
    /// `agentsend` and a background shell's routed completion.
    pub from_user: Option<String>,
}

impl FollowUp {
    /// A message the lead (or a routed note) delivered after `at` calls.
    #[must_use]
    pub const fn lead(at: usize) -> Self {
        Self {
            at,
            from_user: None,
        }
    }
}

/// The note telling the lead which messages the **user** sent an agent
/// directly, in its session view (`docs/agent-tools.md`): the completion
/// notice and a foreground result both carry it, because a model that reads
/// an answer to a question it never asked cannot otherwise know who asked.
/// One `- ` bullet per message, a multi-line one indented beneath its own.
/// `None` when the user sent nothing.
#[must_use]
pub fn user_messages_note(messages: &[String]) -> Option<String> {
    if messages.is_empty() {
        return None;
    }
    let mut lines = vec![
        "The user messaged this agent directly in its session view — these came from the \
         user, not from you:"
            .to_string(),
    ];
    for message in messages {
        let mut rows = message.trim_end_matches('\n').lines();
        lines.push(format!("- {}", rows.next().unwrap_or_default()));
        lines.extend(rows.map(|row| format!("  {row}")));
    }
    Some(lines.join("\n"))
}

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
    /// The `agent` call this run answers, as the round announced it: the
    /// provider's id and the model's verbatim arguments, carried onto the
    /// group's record so the next request replays the launch as the
    /// provider saw it (`docs/prompt-caching.md`). `None` for a scripted
    /// launch.
    pub call_id: Option<String>,
    pub arguments: Option<String>,
    /// The call's index in its round (`ToolCall::position`'s twin), looked
    /// up by the loop from the round it announced.
    pub position: Option<usize>,
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
    /// The cache-write share of `turn_usage_tokens` — the `({c} written)`
    /// half ([`TurnSummary::cache_write`]).
    turn_usage_cache_write: u64,
    /// Its **context size** in tokens — the footer gauge's numerator while
    /// its session view is on screen (`docs/agent-context-gauge.md`), by the
    /// main session's exact rule ([`crate::app::App::context_used`]): the
    /// last usage frame's `input + output` — the provider's own accounting of
    /// the re-sent context plus the reply that joins the next request — which
    /// each frame **replaces** (the next round's `input` already carries the
    /// last one's whole context, so summing frames is the billed tally,
    /// [`tokens`](Self::tokens), not the context). A turn that settles having
    /// seen no frame (the offline dummy, a provider that omits usage) falls
    /// back to the tokenizer estimate over its own transcript
    /// (`estimate_context_tokens`). Zero until the first of either lands,
    /// where the main gauge starts too.
    context_used: u64,
    /// How long it has been running — boundary-injected each frame
    /// ([`crate::app::App::set_agent_runtime`], the `set_status_times`
    /// pattern). Frozen at its final value once the agent settles.
    pub runtime: Duration,
    /// The [`crate::app::STATUS_VERBS`] index this turn's session-view
    /// status line opened on: the table's first verb for the agent's first
    /// turn, one past the last verb a turn wore for each continuation
    /// ([`reopen`](Self::reopen)) — the main session's walk, per agent.
    verb_start: usize,
    /// The index of the verb that line wears now — moved on every
    /// [`crate::app::VERB_ROTATION`] of runtime by the per-frame injection
    /// ([`rotate_verb`](Self::rotate_verb)), never by an event's runtime
    /// freeze, so the turn's summary names the verb the view last drew
    /// (`docs/status-indicator.md`).
    verb_index: usize,
    /// Its own conversation: the prompt as a user message, its replies, its
    /// finished tool calls — everything the agent session view renders and
    /// the Ctrl+O agent cell expands.
    pub history: Vec<HistoryItem>,
    /// The in-flight reply text (its streaming buffer).
    pub streaming: Option<String>,
    /// Messages the user typed into this agent's session **while it was
    /// running** — the main session's [`steered`](crate::app::App::steered)
    /// queue, one level down (`docs/queue.md`). They show above the box in
    /// the agent's session view until its loop's next round boundary takes
    /// them, when [`StreamEvent::Steered`] turns each into a real user
    /// message on this transcript — the one event a **settled** run still
    /// folds, reopening it, because the continuation that delivered it is
    /// otherwise invisible. A run the user **stopped** instead drops these
    /// outright ([`interrupt`](AgentRun::interrupt)): its loop is cancelled,
    /// so a row left up would promise a delivery that cannot happen.
    pub queued: Vec<String>,
    /// **Follow-up turns** the user queued into this agent's session with
    /// **Tab** — the main session's [`queued`](crate::app::App::queued), one
    /// level down (`docs/queue.md`). Where [`queued`](Self::queued) rides the
    /// running loop's next round boundary, each of these waits for that loop
    /// to *settle* and then runs as its own chat continuation, one per
    /// settle, in submission order. They render above the box beneath the
    /// steered rows, blank-divided, exactly as a Tab follow-up does in the
    /// main view. A run the user **stopped** drops these too
    /// ([`interrupt`](AgentRun::interrupt)) — nothing will continue it.
    pub followups: VecDeque<String>,
    /// The messages the **user** sent this agent that its current run has
    /// read — a queued one once [`StreamEvent::Steered`] delivers it, the
    /// chat that started a continuation at once. What its settle tells the
    /// lead the answer also answered: the lead never wrote these, and its
    /// notice is the only place it can learn of them (`docs/agent-tools.md`).
    /// Cleared by [`reopen`](Self::reopen), so each notice names its own
    /// run's.
    pub user_messages: Vec<String>,
    /// Its live tool calls, front running — the parallel-batch queue shape.
    pub tool_queue: VecDeque<ToolCall>,
    /// The running call's live rows and when each last changed — the main
    /// session's [`App::live_tail`](crate::app::App::live_tail), stamped
    /// with this agent's own command clock ([`Self::command_elapsed`]) so its
    /// session view's running cell follows the rows still moving too
    /// (`docs/tool-streaming.md`).
    tool_live: crate::app::LiveTail,
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
    /// The **newest call** the agent started — what the sticky
    /// `{Name}: {detail}` activity row is built from ([`activity`] /
    /// [`activity_shown`], over `activity_line`: a `bash` call's
    /// model-supplied `description`, else its args summary; an MCP call as
    /// `{Server}: {tool}`). Kept until the *next* tool starts, so the tree
    /// row shows what the agent is doing (or just did) rather than dropping
    /// to a generic `Working…` between calls (`docs/agent-tool.md`). The
    /// record as it came: a file tool's path is the model's own, shown by
    /// the session's rule only when the row is painted.
    ///
    /// [`activity`]: AgentRun::activity
    /// [`activity_shown`]: AgentRun::activity_shown
    pub last_call: Option<AgentCall>,
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
    /// How long its **current running command** has executed —
    /// boundary-injected each frame from `Session::agent_command_clocks`, the
    /// [`thinking`](Self::thinking) pattern (cleared, then re-injected for the
    /// running ones), so the session view's `bash` tail counts its `+N lines
    /// (Ns · wait …)` clock row from the call's own `ToolStart` — never
    /// from the agent's whole [`runtime`](Self::runtime), which its status
    /// line shows
    /// (`docs/agent-view-streaming.md`). `None` when no command is running.
    pub command_elapsed: Option<Duration>,
    /// Set while this agent's failed request is being retried
    /// ([`StreamEvent::Retrying`]) — its session view's status line shows the
    /// same `retrying {attempt}/{max}` clause the main turn's does, and the
    /// next streamed content clears it (`docs/llm.md`).
    pub retry: Option<crate::app::RetryInfo>,
    /// Set while this agent's request **waits for a lost connection**
    /// ([`StreamEvent::Offline`]) — its session view's status line wears the
    /// same `Waiting for internet…` the main turn's does, over the same `No
    /// connection to {host}` row, the wait's start stamped off this run's
    /// [`runtime`](Self::runtime). Cleared the moment the host answers, and
    /// never `Some` beside [`retry`](Self::retry) (`docs/offline.md`).
    pub offline: Option<crate::app::OfflineInfo>,
}

impl AgentRun {
    /// The `agent` call this run answers — the provider's id and the model's
    /// verbatim arguments the launch's [`AgentSpec`](crate::stream::AgentSpec)
    /// carried.
    #[must_use]
    pub fn with_call(
        mut self,
        call_id: Option<String>,
        arguments: Option<String>,
        position: Option<usize>,
    ) -> Self {
        self.call_id = call_id;
        self.arguments = arguments;
        self.position = position;
        self
    }

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
            call_id: None,
            position: None,
            arguments: None,
            status: AgentStatus::Pending,
            tool_uses: 0,
            tokens: 0,
            usage_tokens: 0,
            turn_usage_tokens: 0,
            turn_usage_cached: 0,
            turn_usage_cache_write: 0,
            context_used: 0,
            runtime: Duration::ZERO,
            verb_start: 0,
            verb_index: 0,
            history: vec![HistoryItem::Message(Message {
                role: Role::User,
                text: prompt,
                timestamp: String::new(),
                images: Vec::new(),
            })],
            streaming: None,
            queued: Vec::new(),
            followups: VecDeque::new(),
            user_messages: Vec::new(),
            tool_queue: VecDeque::new(),
            tool_live: crate::app::LiveTail::default(),
            result: None,
            error: None,
            hidden: false,
            stopped_by_user: false,
            last_call: None,
            reasoning: None,
            round_reasoning: Vec::new(),
            thinking: None,
            command_elapsed: None,
            retry: None,
            offline: None,
        }
    }

    /// Does folding `event` **finalise the run of streamed text before it**
    /// — the segment boundaries [`apply`](Self::apply) flushes at (CLAUDE.md
    /// invariant 4's flush-before-you-interleave)?
    ///
    /// One definition, two readers: `apply` consumes the buffer at these
    /// points, and the session view commits the tail its `StreamRender`
    /// withheld — then resets that render — at exactly the same ones
    /// (`tui::agent`). A hand-kept list in the boundary is what drifted
    /// before: [`StreamEvent::HookNote`] flushes the buffer in the fold but
    /// was missing from the view's list, so a `SubagentStop` continuation
    /// dropped the reply's withheld tail from the screen *and* left the
    /// render holding a prefix of a buffer that no longer existed
    /// (`docs/agent-view-streaming.md`).
    ///
    /// [`StreamEvent::ThinkingStart`] is deliberately not here: it flushes
    /// too, but only when the display is on — the boundary reads that gate
    /// itself (`/settings` **Hide thinking**), so this stays the question
    /// `apply` alone answers. `every_flush_point_is_declared` pins the pair.
    #[must_use]
    pub const fn flushes_segment(event: &StreamEvent) -> bool {
        matches!(
            event,
            StreamEvent::ToolBatch(_)
                | StreamEvent::ToolStart { .. }
                | StreamEvent::HookNote { .. }
                | StreamEvent::Steered { .. }
                | StreamEvent::StreamDone
                | StreamEvent::Error(_)
        )
    }

    /// Fold one of the subagent's own [`StreamEvent`]s into this entry —
    /// the roster-sized mirror of the main loop's `on_stream_event`. Returns
    /// `true` when the event settled the agent (reached a terminal status),
    /// so the boundary can post its completion notice / start the linger.
    /// Events for an already-settled agent are dropped (a late chunk racing
    /// a local kill).
    pub fn apply(&mut self, event: &StreamEvent) -> bool {
        if self.status.is_final() {
            // One event reaches a settled run: a message delivered into it
            // (`docs/queue.md`). A continuation is what delivered it — the
            // one its own thread starts for a message queued in the settle
            // window — and folding it is what keeps that continuation
            // visible: without this it runs with the model working, the
            // transcript showing nothing and the pending row never clearing.
            // A run the user **stopped** is closed for good: the `x` said so,
            // and nothing may resurrect it.
            if !matches!(event, StreamEvent::Steered { .. }) || self.stopped_by_user {
                return false;
            }
            self.reopen();
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
                // Content arrived — the reconnect, or the wait for the
                // connection, is over (the main turn's rule, `docs/llm.md`,
                // `docs/offline.md`).
                self.recovered();
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
            // The agent's loop took a message the user queued into its
            // session (docs/queue.md): it stops waiting above the box and
            // becomes a real user message on this transcript, after the run
            // of streamed text ahead of it (invariant 4).
            StreamEvent::Steered { text } => {
                // Only the user's own messages wait on `queued`: the lead's
                // `agentsend` and a routed shell note arrive unannounced.
                if self.queued.contains(text) {
                    self.user_messages.push(text.clone());
                }
                self.queued.retain(|pending| pending != text);
                self.push_user_message(text);
                self.tokens += crate::app::count_tokens(text) as u64;
            }
            // Never sent on an agent's channel — the prompt-submit hook fires
            // only at the main session's spawn top (docs/hooks.md); mapped so
            // the match stays total and honest if that ever changes.
            StreamEvent::PromptBlocked { .. } => {}
            // Never sent on an agent's channel either: only the main turn
            // reads the registry's board, and a shell a subagent launched
            // reports to it through its own queue, as a `Steered` message
            // (docs/background.md).
            StreamEvent::NoticeDelivered { .. } => {}
            // A reasoning delta: counted so the footer tally ticks while the
            // agent thinks (the status-line pattern) and — **while a phase is
            // open** — accumulated for the live `● Thinking…` block the
            // session view previews. With the display off no phase is ever
            // opened, so it stays counted-and-dropped exactly as before
            // (`docs/agent-view-streaming.md`).
            StreamEvent::ThinkingChunk(text) => {
                self.tokens += crate::app::count_tokens(text) as u64;
                self.recovered();
                if let Some(buffer) = self.reasoning.as_mut() {
                    buffer.push_str(text);
                }
            }
            // Opaque progress — counted so the footer tally ticks while the
            // agent generates a call (the status-line pattern).
            StreamEvent::ToolCallDelta(text) => {
                self.tokens += crate::app::count_tokens(text) as u64;
                self.recovered();
            }
            StreamEvent::ToolBatch(items) => {
                // The round came back (the wires that deliver a call whole
                // stream no delta first) — any wait is over.
                self.recovered();
                self.flush_segment();
                // The announcement is the round's **whole** queue, exactly as
                // `App::start_tool_batch` treats it: anything left over from
                // an earlier round would otherwise sit at the front and take
                // this round's resolutions, putting every cell off by one.
                self.tool_queue.clear();
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
                        call_id: None,
                        position: None,
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
                self.tool_live.reset();
                // A new command has run no time yet: the roster tick injects
                // its clock, and until then the previous call's reading must
                // not stamp this one's first live rows (the main session's
                // `start_tool` rule).
                self.command_elapsed = None;
                self.last_call = Some(AgentCall {
                    name: name.clone(),
                    args: args.clone(),
                    detail: detail.clone(),
                });
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
                        call_id: None,
                        position: None,
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
                    self.tool_live.reset();
                }
            }
            // A terminal session's live output and a refined header, folded
            // exactly as the main session folds them
            // (docs/interactive-shell.md).
            StreamEvent::ToolScreen { settled, live } => {
                if let Some(front) = self.tool_queue.front_mut()
                    && front.status == ToolStatus::Running
                {
                    let now = self.command_elapsed.unwrap_or_default();
                    self.tool_live.apply(&mut front.output, settled, live, now);
                }
            }
            StreamEvent::ToolTitle(title) => {
                if let Some(front) = self.tool_queue.front_mut()
                    && front.status == ToolStatus::Running
                {
                    front.args.clone_from(title);
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
                display,
                result,
                truncated,
            } => {
                self.tokens += crate::app::count_tokens(result) as u64;
                if let Some(mut front) = self.tool_queue.pop_front() {
                    front.status = ToolStatus::Failed;
                    front.output = display.clone();
                    front.context_output = Some(result.clone());
                    // The main boundary flags it on all three resolutions
                    // (`tui::stream`), so the expanded cell appends its dim
                    // `…` marker here too.
                    front.truncated = *truncated;
                    self.history.push(HistoryItem::Tool(front));
                }
            }
            // A subagent's `run_in_background` launch (its executor carries
            // the shared registry, attributed via `BgOrigin`): resolve the
            // front call as backgrounded — the shell itself lives on the
            // shared list (`docs/background.md`).
            StreamEvent::ToolBackgrounded { output, .. } => {
                // The launch acknowledgement is text the model reads, so it
                // is charged like every other resolution's — the main
                // session's `resolve_front_tool` charges it too.
                self.tokens += crate::app::count_tokens(output) as u64;
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
                self.turn_usage_cache_write += usage.cache_write;
                // The round's `input` is the whole re-sent context and its
                // `output` joins the next round's — their sum is this agent's
                // context size, the main session's `apply_usage` rule
                // (`docs/agent-context-gauge.md`).
                self.context_used = usage.input.saturating_add(usage.output);
            }
            StreamEvent::StreamDone => {
                // A phase still open when the round ended keeps what streamed
                // — settled first, so its cell sits ahead of the reply it
                // preceded (`docs/thinking-stream.md`).
                self.settle_thinking();
                self.result = self.flush_segment();
                self.tool_queue.clear();
                self.status = AgentStatus::Done;
                // No usage frame this turn (the dummy, a provider that omits
                // them): the tokenizer estimate over this transcript stands
                // in for the gauge — `App::take_turn_summary`'s rule
                // (`docs/agent-context-gauge.md`).
                if self.turn_usage_tokens == 0 {
                    self.context_used = self.estimate_context_tokens();
                }
                // The turn's dim receipt, the main turn-end's exact shape
                // (`Done for 59s · 6.1k tokens (2.8k cached)`): the agent
                // session view commits it and every rebuild/Ctrl+O renders it
                // from here, while the derived context skips it as chrome
                // like any summary (docs/agent-tool.md). The runtime was
                // frozen at its live value just before this event
                // (`tui::agent`), so it is the turn's elapsed; a failure or
                // interrupt records none, main parity.
                self.history.push(HistoryItem::Summary(TurnSummary {
                    // The past tense of the verb the view's line last wore.
                    verb: self.status_verb().done,
                    secs: self.runtime.as_secs(),
                    timestamp: String::new(),
                    shells: 0,
                    tokens: usize::try_from(self.turn_usage_tokens).unwrap_or(usize::MAX),
                    cached: usize::try_from(self.turn_usage_cached).unwrap_or(usize::MAX),
                    cache_write: usize::try_from(self.turn_usage_cache_write).unwrap_or(usize::MAX),
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
                display,
                result,
                truncated,
            } => {
                self.tokens += crate::app::count_tokens(result) as u64;
                if let Some(mut front) = self.tool_queue.pop_front() {
                    front.status = ToolStatus::Ok;
                    front.output = display.clone();
                    front.context_output = Some(result.clone());
                    front.truncated = *truncated;
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
                    call_id: None,
                    position: None,
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
                // The host answered (badly): any outage is over.
                self.recovered();
                self.retry = Some(crate::app::RetryInfo {
                    attempt: *attempt,
                    max: *max,
                });
            }
            // No connection could be made for this agent's request and its
            // loop is waiting for one: the session view says so, exactly
            // like the main turn's strip (`docs/offline.md`). The first
            // announcement stamps the wait's start with the runtime the view
            // shows; later ones move the count on and keep it.
            StreamEvent::Offline { host, attempts } => {
                let began = self
                    .offline
                    .as_ref()
                    .map_or(self.runtime, |outage| outage.began);
                self.recovered();
                self.offline = Some(crate::app::OfflineInfo {
                    host: host.clone(),
                    attempts: *attempts,
                    began,
                });
            }
            StreamEvent::RoundCalls(_)
            | StreamEvent::Permission(_)
            | StreamEvent::AskUser(_)
            | StreamEvent::ThinkingStart
            | StreamEvent::ThinkingEnd
            | StreamEvent::AgentBatch { .. }
            | StreamEvent::AgentGroupDone { .. } => {}
        }
        false
    }

    /// The agent's context size in tokens (see the field doc) — what its
    /// session view's footer gauges ([`crate::app::App::context_gauge`]).
    #[must_use]
    pub const fn context_used(&self) -> u64 {
        self.context_used
    }

    /// The tokenizer estimate of this agent's context: its own transcript
    /// derived exactly as a request would be
    /// ([`crate::context::context_messages`]), counted by the rule the main
    /// gauge counts with (`app::estimate_messages_tokens`) — and a **true
    /// zero** for a transcript that derives nothing, the main estimate's
    /// empty-conversation rule. The agent's system prompt and briefing are
    /// not in it: this type knows neither (they are the launch's, shown by
    /// the view's Ctrl+D), and the estimate is only the stand-in for the
    /// frame a live provider sends every round.
    fn estimate_context_tokens(&self) -> u64 {
        if !crate::context::derives_conversation(&self.history) {
            return 0;
        }
        let messages = crate::context::context_messages(&self.history);
        u64::try_from(crate::app::estimate_messages_tokens(&messages)).unwrap_or(u64::MAX)
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
        // A stopped agent's loop is cancelled, so nothing will ever take what
        // is still queued for it: drop the rows rather than leave them
        // promising a delivery that cannot happen (`docs/queue.md`). Only a
        // *stop* does this — a natural settle with a message still queued
        // hands it back for a continuation to deliver. The Tab follow-ups go
        // with them: each would have run as a continuation of a run that is
        // now cancelled.
        self.queued.clear();
        self.followups.clear();
        self.status = AgentStatus::Interrupted;
        true
    }

    /// Settle this run from its **group's** resolution — the path for a
    /// member whose own terminal event never arrived (a cancelled loop
    /// returns without one, and the offline dummy scripts none at all), so
    /// the foreground group's outcome is all there is to settle from.
    ///
    /// Every other settle resolves what is in flight; this one must too, or
    /// a *finished* agent goes on owning live cells — its session view
    /// previews a `⎿ Running…` that can never resolve, and the next chat
    /// continuation's batch queues behind the ghost. `output` is the resolved
    /// cell's text (the call did not complete, whatever the run reported).
    /// Inert once final.
    pub fn settle_from_group(&mut self, status: AgentStatus, output: &str) {
        if self.status.is_final() {
            return;
        }
        // A partial thought is what the user saw — the interrupt's rule.
        self.settle_thinking();
        self.resolve_tools(output);
        self.status = status;
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

    /// Park a message the user typed into this agent's session while it was
    /// running: it shows above the box until the agent's next round boundary
    /// takes it ([`StreamEvent::Steered`]). Deliberately **not** recorded on
    /// the transcript yet — claiming the agent has read something it has not
    /// is what the main session's queue avoids too (`docs/queue.md`).
    pub fn queue_chat(&mut self, text: &str) {
        self.queued.push(text.to_string());
    }

    /// Park a **follow-up turn** (Tab) for this agent: it waits below the
    /// steered rows until the agent's loop settles, when the boundary hands
    /// it over as a chat continuation — the main session's `queue_draft`, one
    /// level down (`docs/queue.md`). Always its own entry: Tab means "a
    /// separate turn", here as in the main view.
    pub fn queue_followup(&mut self, text: &str) {
        self.followups.push_back(text.to_string());
    }

    /// Take the **next** follow-up turn (the boundary drains one per settle,
    /// `App::drain_next_batch`'s twin). `None` when nothing is waiting.
    pub fn take_followup(&mut self) -> Option<String> {
        self.followups.pop_front()
    }

    /// Take the **last** follow-up back (Alt+Up's pull into the composer,
    /// `App::drain_last_batch`'s twin), leaving the earlier ones queued.
    pub fn take_last_followup(&mut self) -> Option<String> {
        self.followups.pop_back()
    }

    /// The host answered: the reconnect or the wait for the connection is
    /// over, so both live indicators come down together (the main turn's
    /// `TurnStatus::recovered`).
    fn recovered(&mut self) {
        self.retry = None;
        self.offline = None;
    }

    /// The status verb its session view's line wears now — the synthesized
    /// status line shows it and the turn's summary records its past tense.
    #[must_use]
    pub fn status_verb(&self) -> StatusVerb {
        StatusVerb::at(self.verb_index)
    }

    /// Move the status verb on to where the current
    /// [`runtime`](Self::runtime) puts it — one verb further every
    /// [`crate::app::VERB_ROTATION`]. The per-frame half of the runtime
    /// injection ([`crate::app::App::set_agent_runtime`]); an event's runtime
    /// freeze never calls it, so a turn that settles just past a rotation no
    /// frame drew still names the verb the view showed.
    pub fn rotate_verb(&mut self) {
        self.verb_index = StatusVerb::rotated(self.verb_start, self.runtime);
    }

    /// A follow-up chat turn began (a continuation run): reopen a settled
    /// agent so new events fold in again. The turn-scoped usage counters
    /// reset — the next settle's `Done for Ns · {n} tokens` receipt is the
    /// continuation's own — while the cumulative roster tally stands, and the
    /// status verb walks on one past the last verb the finished turn wore,
    /// as a new main turn's does.
    pub fn reopen(&mut self) {
        self.verb_start = self.verb_index + 1;
        self.verb_index = self.verb_start;
        self.status = AgentStatus::Running;
        self.stopped_by_user = false;
        self.result = None;
        self.error = None;
        self.turn_usage_tokens = 0;
        self.turn_usage_cached = 0;
        self.turn_usage_cache_write = 0;
        self.round_reasoning.clear();
        self.user_messages.clear();
    }

    /// The tree row's activity: what the agent is doing — or, between tool
    /// calls, what it just did (the sticky [`last_call`] holds until the
    /// next call starts, so the row keeps its context instead of dropping to
    /// `Working…` while the agent reasons over a result). The record's own
    /// text, a file tool's path as the model sent it.
    ///
    /// [`last_call`]: AgentRun::last_call
    #[must_use]
    pub fn activity(&self) -> String {
        self.activity_shown(|_, args| args.to_string())
    }

    /// [`activity`](Self::activity) with the newest call's `args` summary
    /// shown the way the caller says — `show_args(name, args)` maps it to
    /// its display form (the TUI hands in its session's path rule, so a
    /// file tool's row reads `Write: ~/x.py`; `docs/tools.md` *Path
    /// display*) — over the same grammar. The labels, a `bash` call's
    /// description and the record itself are untouched.
    #[must_use]
    pub fn activity_shown(&self, show_args: impl FnOnce(&str, &str) -> String) -> String {
        match self.status {
            AgentStatus::Pending => AgentStatus::Pending.label().to_string(),
            AgentStatus::Running => self.last_call.as_ref().map_or_else(
                || "Working…".to_string(),
                |call| {
                    activity_line(
                        &call.name,
                        &show_args(&call.name, &call.args),
                        call.detail.as_deref(),
                    )
                },
            ),
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

    /// Inject the running command's elapsed (the boundary's per-agent
    /// command clock, the [`set_thinking`](Self::set_thinking) pattern) —
    /// what the session view's running `bash` cell counts its `+N lines
    /// (Ns · wait …)` clock row with. `None` when no command is running.
    pub fn set_command_elapsed(&mut self, elapsed: Option<Duration>) {
        self.command_elapsed = elapsed;
    }

    /// The running call's live rows and when each last changed — what this
    /// agent's session view's running cell follows
    /// ([`crate::app::LiveTail`]).
    #[must_use]
    pub fn live_tail(&self) -> &crate::app::LiveTail {
        &self.tool_live
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
    /// backend error killed the run): the running — or permission-waiting —
    /// front call and each `⎿ Waiting…` sibling behind it, in the batch's
    /// order, each recorded on the agent's transcript. The main session's
    /// rule (`App::fail_live_queue`, `docs/interrupt.md`): every call the
    /// agent made keeps its cell in the session view, and its Ctrl+D agrees
    /// with what a chat continuation resumes from.
    fn resolve_tools(&mut self, output: &str) {
        while let Some(mut call) = self.tool_queue.pop_front() {
            call.status = ToolStatus::Failed;
            call.output = output.to_string();
            self.history.push(HistoryItem::Tool(call));
        }
    }
}

/// The call an agent's sticky activity row names ([`AgentRun::last_call`]):
/// the [`StreamEvent::ToolStart`] fields the row's grammar
/// (`activity_line`) reads, kept as they came.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCall {
    /// The display name (`Write`, `Bash`, an MCP `server - tool (MCP)`).
    pub name: String,
    /// The one-line args summary — a file tool's path, a command, JSON.
    pub args: String,
    /// The model's own description, when the call carried one (`bash`).
    pub detail: Option<String>,
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
    // One dim clipped row: a `bash` summary keeps the command's newlines and
    // space runs for the cell header, flattened here to the row's one line.
    let args = crate::llm::tools::flatten_one_line(args);
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
    /// One event of the subagent's own stream (chunks, tool calls, usage —
    /// and, from a scripted run, its terminal done/error).
    Stream { id: String, event: StreamEvent },
    /// A live run's **terminal** event (`StreamDone`/`Error`), sent once the
    /// registry has recorded the outcome ([`AgentRegistry::settle`]) — at
    /// once, or by the `agentoutput` call that was waiting on the agent.
    /// `observed` when that call already reported the outcome to the lead, so
    /// no completion notice is owed (`docs/agent-tools.md` *One notice per
    /// answer*).
    Settled {
        id: String,
        event: StreamEvent,
        observed: bool,
    },
    /// The **lead** stopped this agent (`agentkill`): its loop is cancelled,
    /// and a cancelled loop sends nothing of its own, so this is the roster's
    /// only word of it.
    Stopped { id: String },
    /// The lead **resumed** this settled agent (`agentsend`): a continuation
    /// is starting, its first round announcing the message (`Steered`).
    /// Carries what the roster needs to bring back a row its sweep already
    /// collected.
    Resumed {
        id: String,
        description: String,
        agent_type: String,
        prompt: String,
    },
}

/// Where an agent stands, as the lead's companions report it
/// (`docs/agent-tools.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    /// Its run (the launch or a continuation) is still going.
    Running,
    /// Finished — with its final response.
    Done(String),
    /// Its backend errored — with the error.
    Failed(String),
    /// Stopped: by the user's `x` (final), or by the lead's `agentkill`
    /// (resumable with `agentsend`).
    Stopped { by_user: bool },
}

/// One agent as the lead's companions see it — what the registry knows,
/// taken under its lock at one instant ([`AgentRegistry::snapshot`]), so a
/// report can be rendered from it without holding anything
/// ([`output_report`], [`list_report`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSnapshot {
    pub id: String,
    pub description: String,
    pub agent_type: String,
    pub state: AgentState,
    /// How long it has worked, over every run — the launch and each
    /// continuation — so the frame's two numbers both cover its whole life.
    pub runtime: Duration,
    /// How many tool calls it has started, over every run.
    pub tool_uses: usize,
    /// Each call's `Name(args)` one-liner, oldest first.
    pub calls: Vec<String>,
    /// Where each message it read arrived, and whether the user sent it, in
    /// order ([`AGENT_FOLLOW_UP_MARK`]).
    pub follow_ups: Vec<FollowUp>,
}

/// What stopping an agent found ([`AgentRegistry::kill`],
/// [`AgentRegistry::stop`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stop {
    /// Its run had not settled, so this stopped it.
    pub was_live: bool,
    /// An `agentoutput` call was waiting on it and will report the stop
    /// itself — no completion notice is owed (`docs/agent-tools.md`).
    pub watched: bool,
}

/// What became of an `agentsend` ([`AgentRegistry::resume`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    /// The agent is running: the message waits on its queue for its next
    /// round boundary.
    Queued,
    /// The agent had settled: its conversation is handed back to continue
    /// from — the message already on its queue, so the continuation's first
    /// round announces it — and the slot is busy again under `cancel`. The
    /// caller spawns the run.
    Continue {
        messages: Vec<ChatMessage>,
        cancel: CancelToken,
        agent_type: String,
    },
    /// The user's `x` stopped it, and a model may not undo that.
    StoppedByUser,
    /// The lead stopped it and its thread has not let go yet — try again in
    /// a moment.
    Stopping,
    /// No such agent here (or no longer).
    Unknown,
}

/// A message waiting for an agent's next round boundary.
#[derive(Debug, Clone)]
struct PendingInput {
    text: String,
    /// The user typed it into the agent's session view.
    from_user: bool,
    /// A note of this agent's own shell's, routed here unread (see
    /// [`RoutedNote`]).
    routed: Option<RoutedNote>,
}

impl PendingInput {
    fn lead(text: &str) -> Self {
        Self {
            text: text.to_string(),
            from_user: false,
            routed: None,
        }
    }
}

/// A shell's note routed to the agent that launched it
/// ([`AgentRegistry::route_shell_note`]) — what its own call into that
/// session takes back while the note is unread (`docs/bash-tools.md` *One
/// notice per exit*): a look takes back a `… is waiting for input` note it
/// just answered, and a call into a session that ended reports the exit
/// instead of being told it was already reported.
#[derive(Debug, Clone)]
struct RoutedNote {
    /// Its number, from the board's own count — what the loop holds the
    /// notice cell by, so a note taken back costs its cell too.
    seq: u64,
    /// The session it reports.
    shell: String,
    /// For an exit, what a claim needs.
    exit: Option<crate::background::ClaimedExit>,
}

/// Where [`AgentRegistry::route_shell_note`] sent a shell's note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Queued for the launching agent's next round boundary, as this note
    /// number — what the loop holds the notice cell by.
    Routed(u64),
    /// The launcher cannot hear it (settled, stopped): the shared board must
    /// take it, the note still owed.
    Unheard,
    /// A companion call already reported what the note says: none is owed.
    Covered,
}

/// One running subagent's shared slot: its cancel token, completion state,
/// stored conversation (for chat continuations), and pending chat inputs.
#[derive(Default)]
struct AgentSlot {
    cancel: CancelToken,
    /// The registered subagent type (`general-purpose` / `explore`) — a chat
    /// continuation rebuilds the same tool set from it.
    agent_type: String,
    /// The task label and the launch prompt — what `agentlist` names it by,
    /// and what the roster rebuilds a swept row from when the lead resumes it
    /// (`docs/agent-tools.md`). Empty for an identity-free registration.
    description: String,
    prompt: String,
    /// The run (initial or a chat continuation) is still executing.
    busy: bool,
    /// The agent has settled at least once (a result/failure is stored).
    done: bool,
    /// It was stopped ([`AgentRegistry::kill`] or [`AgentRegistry::stop`]).
    killed: bool,
    /// …by the **user** — final, where the lead's `agentkill` is resumable.
    killed_by_user: bool,
    /// Each tool call it has started, as its `Name(args)` one-liner — the
    /// `agentoutput` summary ([`AgentRegistry::record_call`]).
    calls: Vec<String>,
    /// Where each message it read arrived, in calls made by then
    /// ([`AgentSnapshot::follow_ups`]).
    follow_ups: Vec<FollowUp>,
    /// How many of `follow_ups` came before the current run — the user's
    /// messages *this* run answered are the ones after it
    /// ([`AgentRegistry::user_messages`]).
    run_follow_ups: usize,
    /// When its current run started; `None` once settled, the run's length
    /// then added to `active`, the time its finished runs took.
    run_started: Option<Instant>,
    active: Duration,
    /// Its launch order — `agentlist` lists by it.
    launched: u64,
    /// Its settle order — retention evicts by it; `None` while running.
    settled: Option<u64>,
    /// How many `agentoutput` calls are waiting on it to settle.
    waiters: usize,
    /// The terminal event a settle **held** for those waiters to release
    /// ([`AgentRegistry::end_wait`]), and whether one of them reported the
    /// outcome it carries.
    held: Option<StreamEvent>,
    held_observed: bool,
    /// Completion notices sent on the channel **unobserved** that the loop
    /// has not posted yet — what an `agentoutput` report cancels when it
    /// gets there first ([`AgentRegistry::report_settled`]).
    notices_owed: usize,
    /// Its final response text (present once `done`, unless it failed).
    result: Option<String>,
    /// Its failure message (present once `done` when the run errored).
    failed: Option<String>,
    /// The full request message list at settle — a chat continuation resumes
    /// from here.
    messages: Option<Vec<ChatMessage>>,
    /// Messages awaiting delivery into the running loop (taken per round via
    /// the `pending_notices` seam), each remembering whether the user sent it.
    pending_inputs: Vec<PendingInput>,
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

impl AgentSlot {
    /// How long it has worked, over every run.
    fn runtime(&self) -> Duration {
        self.active
            + self
                .run_started
                .map_or(Duration::ZERO, |started| started.elapsed())
    }

    /// Freeze the runtime: the run is over, its length added to the rest.
    fn freeze_runtime(&mut self) {
        if let Some(started) = self.run_started.take() {
            self.active += started.elapsed();
        }
    }

    /// Where it stands — a stop outranks the outcome its thread reported
    /// late.
    fn state(&self) -> AgentState {
        if self.killed {
            return AgentState::Stopped {
                by_user: self.killed_by_user,
            };
        }
        if self.busy || !self.done {
            return AgentState::Running;
        }
        match &self.failed {
            Some(error) => AgentState::Failed(error.clone()),
            None => AgentState::Done(self.result.clone().unwrap_or_default()),
        }
    }

    fn snapshot(&self, id: &str) -> AgentSnapshot {
        AgentSnapshot {
            id: id.to_string(),
            description: self.description.clone(),
            agent_type: self.agent_type.clone(),
            state: self.state(),
            runtime: self.runtime(),
            tool_uses: self.calls.len(),
            calls: self.calls.clone(),
            follow_ups: self.follow_ups.clone(),
        }
    }

    /// Begin a new run on this slot — a continuation: busy again under a
    /// fresh cancel token, the last outcome and any stop cleared, the clock
    /// restarted.
    fn restart(&mut self) -> CancelToken {
        let cancel = CancelToken::new();
        self.cancel = cancel.clone();
        self.busy = true;
        self.done = false;
        self.killed = false;
        self.killed_by_user = false;
        self.result = None;
        self.failed = None;
        self.run_started = Some(Instant::now());
        self.settled = None;
        self.run_follow_ups = self.follow_ups.len();
        cancel
    }

    /// May retention let it go? Settled, with nothing still owed on it — no
    /// waiter, no held event.
    fn evictable(&self) -> Option<u64> {
        (!self.busy && self.waiters == 0 && self.held.is_none())
            .then_some(self.settled)
            .flatten()
    }
}

struct Inner {
    /// The launch counter mixed into each id roll.
    next_id: u64,
    /// The order counter launches and settles are stamped from.
    seq: u64,
    slots: HashMap<String, AgentSlot>,
}

impl Inner {
    /// The next order stamp.
    fn stamp(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// Let go of the oldest settled agents past [`AGENT_RETAINED_MAX`]
    /// (`docs/agent-tools.md` *Retention*).
    fn evict_settled(&mut self) {
        let mut settled: Vec<(u64, String)> = self
            .slots
            .iter()
            .filter_map(|(id, slot)| slot.evictable().map(|seq| (seq, id.clone())))
            .collect();
        if settled.len() <= AGENT_RETAINED_MAX {
            return;
        }
        settled.sort_unstable();
        let excess = settled.len() - AGENT_RETAINED_MAX;
        for (_, id) in settled.into_iter().take(excess) {
            self.slots.remove(&id);
        }
    }
}

/// What [`AgentRegistry::post_notice`] did with one of an agent's
/// unobserved settles (`docs/agent-tools.md` *One notice per answer*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticePost {
    /// An `agentoutput` report already handed the lead this answer: nothing
    /// was posted, and no notice cell is owed.
    Covered,
    /// Owed: posted on the board as this
    /// [`seq`](crate::background::PendingNotice::seq) — `None` when the
    /// settle carried no notice to post.
    Posted(Option<u64>),
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
                seq: 0,
                slots: HashMap::new(),
            })),
            events,
        }
    }

    /// Register a fresh subagent of `agent_type` that names no task — a
    /// scripted launch, a test. See [`register_agent`](Self::register_agent).
    #[must_use]
    pub fn register(&self, agent_type: &str) -> (String, CancelToken) {
        self.register_agent(agent_type, "", "")
    }

    /// Register a fresh subagent of `agent_type`, launched to do
    /// `description` with `prompt`: allocate its id (collision re-roll) and
    /// its cancel token, and keep what it was launched as for the lead's
    /// companions (`docs/agent-tools.md`). The caller spawns the thread.
    #[must_use]
    pub fn register_agent(
        &self,
        agent_type: &str,
        description: &str,
        prompt: &str,
    ) -> (String, CancelToken) {
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
        let launched = inner.stamp();
        inner.slots.insert(
            id.clone(),
            AgentSlot {
                classifier: Arc::new(Mutex::new(ClassifierContext::new())),
                cancel: cancel.clone(),
                busy: true,
                agent_type: agent_type.to_string(),
                description: description.to_string(),
                prompt: prompt.to_string(),
                run_started: Some(Instant::now()),
                launched,
                ..AgentSlot::default()
            },
        );
        inner.evict_settled();
        (id, cancel)
    }

    /// The task `id` was launched to do — what a companion's cell header
    /// names the agent by (`tools::summarize_call_naming`). `None` for an id
    /// nothing answers to, or one registered without a task.
    #[must_use]
    pub fn description(&self, id: &str) -> Option<String> {
        let inner = self.inner.lock().expect("agent registry poisoned");
        inner
            .slots
            .get(id)
            .map(|slot| slot.description.clone())
            .filter(|description| !description.is_empty())
    }

    /// Record a tool call `id` started, as its `Name(args)` one-liner — the
    /// forwarder's, at each `ToolStart`, so `agentoutput` can summarize what
    /// the agent has been doing (`docs/agent-tools.md`).
    pub fn record_call(&self, id: &str, header: &str) {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        if let Some(slot) = inner.slots.get_mut(id) {
            slot.calls.push(header.to_string());
        }
    }

    /// `id` as the lead's companions see it, taken at one instant.
    #[must_use]
    pub fn snapshot(&self, id: &str) -> Option<AgentSnapshot> {
        let inner = self.inner.lock().expect("agent registry poisoned");
        inner.slots.get(id).map(|slot| slot.snapshot(id))
    }

    /// Every agent the registry holds — the running ones and the retained
    /// settled ones — in launch order (`agentlist`).
    #[must_use]
    pub fn snapshots(&self) -> Vec<AgentSnapshot> {
        let inner = self.inner.lock().expect("agent registry poisoned");
        let mut slots: Vec<(&String, &AgentSlot)> = inner.slots.iter().collect();
        slots.sort_unstable_by_key(|(_, slot)| slot.launched);
        slots
            .into_iter()
            .map(|(id, slot)| slot.snapshot(id))
            .collect()
    }

    /// An `agentoutput` call starts waiting on `id` to settle: a settle that
    /// lands meanwhile holds its terminal event for the wait to release
    /// ([`end_wait`](Self::end_wait)). `false` for an id nothing answers to.
    #[must_use]
    pub fn begin_wait(&self, id: &str) -> bool {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let Some(slot) = inner.slots.get_mut(id) else {
            return false;
        };
        slot.waiters += 1;
        true
    }

    /// The wait on `id` is over: read where the agent stands **and**, in the
    /// same locked step, release a terminal event its settle held — marked
    /// observed when this wait reports the settled outcome (`reporting`: the
    /// call will hand the lead what it read; a cancelled one won't). Taking
    /// both at one instant is the point: a settle the wait did not see
    /// cannot be marked observed, and one it did cannot be noticed twice
    /// (`docs/agent-tools.md` *One notice per answer*).
    pub fn end_wait(&self, id: &str, reporting: bool) -> Option<AgentSnapshot> {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let slot = inner.slots.get_mut(id)?;
        slot.waiters = slot.waiters.saturating_sub(1);
        let snapshot = slot.snapshot(id);
        // A held event is a settled run's (a run that starts over releases it
        // first), so a reporting wait is reporting exactly that outcome.
        if reporting && slot.held.is_some() {
            slot.held_observed = true;
        }
        if slot.waiters == 0
            && let Some(event) = slot.held.take()
        {
            let observed = std::mem::take(&mut slot.held_observed);
            if !observed {
                slot.notices_owed += 1;
            }
            let _ = self.events.send(AgentEvent::Settled {
                id: id.to_string(),
                event,
                observed,
            });
        }
        Some(snapshot)
    }

    /// Release a terminal event still held for a waiter, unobserved — a run
    /// is about to start over, and its events must never overtake the last
    /// run's end on the channel.
    fn release_held(&self, id: &str, slot: &mut AgentSlot) {
        if let Some(event) = slot.held.take() {
            slot.held_observed = false;
            slot.notices_owed += 1;
            let _ = self.events.send(AgentEvent::Settled {
                id: id.to_string(),
                event,
                observed: false,
            });
        }
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

    /// A run finished whose terminal event its caller already sent — the
    /// offline dummy's scripted runs. See [`settle`](Self::settle).
    pub fn finish(&self, id: &str, outcome: Result<String, String>, messages: Vec<ChatMessage>) {
        self.settle(id, outcome, messages, None);
    }

    /// A run finished: store its outcome + final message list so the parent's
    /// wait loop resolves and a chat continuation can resume — and deliver
    /// its `terminal` event (`docs/agent-tools.md` *One notice per answer*):
    /// sent here, **under the lock**, so whoever sees the agent done (the
    /// foreground group's wait loop) knows the event is already queued; or,
    /// with an `agentoutput` waiting, held for that wait to release once it
    /// has read the outcome. A slot already killed keeps its killed outcome
    /// (the thread noticed late).
    pub fn settle(
        &self,
        id: &str,
        outcome: Result<String, String>,
        messages: Vec<ChatMessage>,
        terminal: Option<StreamEvent>,
    ) {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let seq = inner.stamp();
        if let Some(slot) = inner.slots.get_mut(id) {
            slot.busy = false;
            slot.done = true;
            slot.messages = Some(messages);
            slot.freeze_runtime();
            slot.settled = Some(seq);
            if !slot.killed {
                match outcome {
                    Ok(result) => slot.result = Some(result),
                    Err(error) => slot.failed = Some(error),
                }
            }
            if let Some(event) = terminal {
                if slot.waiters > 0 {
                    slot.held = Some(event);
                } else {
                    slot.notices_owed += 1;
                    let _ = self.events.send(AgentEvent::Settled {
                        id: id.to_string(),
                        event,
                        observed: false,
                    });
                }
            }
        }
        inner.evict_settled();
    }

    /// The **user** stops one agent (the roster's `x`, Esc on its foreground
    /// group): cancel its token and mark it killed **and settled**, so the
    /// parent's wait loop resolves at once instead of waiting for the thread
    /// to notice (a blocking read can take seconds). Final: the lead may not
    /// resume it. Reports whether the agent was live, and whether a waiting
    /// `agentoutput` will report the stop — read in the same locked step, so
    /// the notice the stop would owe is never owed twice.
    pub fn kill(&self, id: &str) -> Stop {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let seq = inner.stamp();
        let Some(slot) = inner.slots.get_mut(id) else {
            return Stop::default();
        };
        slot.cancel.cancel();
        // Nothing will read them now, and the roster drops their rows.
        slot.pending_inputs.clear();
        let was_live = !slot.done;
        if was_live {
            slot.freeze_runtime();
            slot.settled = Some(seq);
        }
        slot.killed = true;
        slot.killed_by_user = true;
        slot.done = true;
        Stop {
            was_live,
            watched: was_live && slot.waiters > 0,
        }
    }

    /// The **lead** stops one agent (`agentkill`): cancelled and settled like
    /// the user's [`kill`](Self::kill), but its conversation stays resumable,
    /// and — since a cancelled loop sends nothing of its own — the loop is
    /// told ([`AgentEvent::Stopped`]). An agent already settled is left as it
    /// was (`was_live: false`). `None` for an id nothing answers to.
    pub fn stop(&self, id: &str) -> Option<Stop> {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let seq = inner.stamp();
        let slot = inner.slots.get_mut(id)?;
        if slot.done {
            return Some(Stop::default());
        }
        slot.cancel.cancel();
        // The roster drops their rows: a resume delivers only its own message.
        slot.pending_inputs.clear();
        slot.freeze_runtime();
        slot.settled = Some(seq);
        slot.killed = true;
        slot.killed_by_user = false;
        slot.done = true;
        let _ = self.events.send(AgentEvent::Stopped { id: id.to_string() });
        Some(Stop {
            was_live: true,
            watched: false,
        })
    }

    /// Stop every agent (quit).
    pub fn kill_all(&self) {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        for slot in inner.slots.values_mut() {
            slot.cancel.cancel();
            slot.killed = true;
            slot.killed_by_user = true;
            slot.done = true;
        }
    }

    /// Stop every agent **and forget them** — `/clear`: the conversation that
    /// knew their ids is gone, so nothing may list or resume them
    /// (`docs/agent-tools.md` *Retention*).
    pub fn clear(&self) {
        self.kill_all();
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        inner.slots.clear();
    }

    /// Deliver an `agentsend` (`docs/agent-tools.md`): onto a running
    /// agent's queue for its next round boundary, or — for a settled one —
    /// as the start of a **continuation** of its stored conversation, the
    /// message queued for that run's first round to announce. The roster is
    /// told of a continuation before it starts ([`AgentEvent::Resumed`]). A
    /// stop by the user is final; a stop by the lead resumes once its thread
    /// has let go.
    pub fn resume(&self, id: &str, text: &str) -> Resume {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let Some(slot) = inner.slots.get_mut(id) else {
            return Resume::Unknown;
        };
        if slot.killed_by_user {
            return Resume::StoppedByUser;
        }
        if slot.busy {
            if slot.killed {
                return Resume::Stopping;
            }
            slot.pending_inputs.push(PendingInput::lead(text));
            return Resume::Queued;
        }
        let Some(messages) = slot.messages.take() else {
            return Resume::Unknown;
        };
        self.release_held(id, slot);
        slot.pending_inputs.push(PendingInput::lead(text));
        let cancel = slot.restart();
        let _ = self.events.send(AgentEvent::Resumed {
            id: id.to_string(),
            description: slot.description.clone(),
            agent_type: slot.agent_type.clone(),
            prompt: slot.prompt.clone(),
        });
        Resume::Continue {
            messages,
            cancel,
            agent_type: slot.agent_type.clone(),
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

    /// Queue a message for a **running** agent — delivered into its loop at
    /// the next round boundary (the `pending_notices` seam). Returns `false`
    /// when the agent isn't currently running a turn (the caller should spawn
    /// a continuation instead). The lead's and the system's door: a
    /// background shell's completion routed to the agent that launched it.
    /// A message the user typed takes
    /// [`queue_user_input`](Self::queue_user_input).
    #[must_use]
    pub fn queue_input(&self, id: &str, text: &str) -> bool {
        self.queue(id, PendingInput::lead(text))
    }

    /// [`queue_input`](Self::queue_input) for a message the **user** typed
    /// into the agent's session view — remembered as theirs, so the lead is
    /// told who wrote it (`docs/agent-tools.md`).
    #[must_use]
    pub fn queue_user_input(&self, id: &str, text: &str) -> bool {
        self.queue(
            id,
            PendingInput {
                text: text.to_string(),
                from_user: true,
                routed: None,
            },
        )
    }

    /// Route the note of shell `shell` — one this agent launched, which
    /// exited or stopped to ask for input — onto the agent's queue
    /// (`docs/agent-tool.md`), asking the board under this registry's lock
    /// whether it is still owed ([`BackgroundRegistry::take_shell_notice`]):
    /// a companion call can never claim the exit in between, and an owed
    /// exit's claim details travel with the note so the agent's own call can
    /// claim it from here ([`claim_routed_exit`](Self::claim_routed_exit)).
    ///
    /// [`BackgroundRegistry::take_shell_notice`]: crate::background::BackgroundRegistry::take_shell_notice
    pub fn route_shell_note(
        &self,
        id: &str,
        shell: &str,
        text: &str,
        waiting: bool,
        board: &crate::background::BackgroundRegistry,
    ) -> Route {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let Some(slot) = inner
            .slots
            .get_mut(id)
            .filter(|slot| slot.busy && !slot.killed)
        else {
            return Route::Unheard;
        };
        match board.take_shell_notice(shell, waiting) {
            crate::background::ShellNotice::Covered => Route::Covered,
            crate::background::ShellNotice::Owed { seq, exit } => {
                slot.pending_inputs.push(PendingInput {
                    text: text.to_string(),
                    from_user: false,
                    routed: Some(RoutedNote {
                        seq,
                        shell: shell.to_string(),
                        exit,
                    }),
                });
                Route::Routed(seq)
            }
        }
    }

    /// Agent `id`'s own call named shell `shell`, which exited with its note
    /// still in the agent's queue, unread: take the note back and hand over
    /// what the call's report of the exit needs, with the note's number.
    /// `None` once the agent's loop has read it — then it was reported.
    pub fn claim_routed_exit(
        &self,
        id: &str,
        shell: &str,
    ) -> Option<(crate::background::ClaimedExit, u64)> {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let pending = &mut inner.slots.get_mut(id)?.pending_inputs;
        let index = pending.iter().position(|input| {
            input
                .routed
                .as_ref()
                .is_some_and(|note| note.shell == shell && note.exit.is_some())
        })?;
        let note = pending.remove(index).routed?;
        Some((note.exit?, note.seq))
    }

    /// Agent `id`'s own call just looked at shell `shell`: take back the
    /// shell's notes still unread in its queue — a `… is waiting for input`
    /// note says nothing the look did not show, and would reach the agent
    /// only after its answer. Their numbers, so the cells held for them go
    /// too.
    pub fn withdraw_routed_notes(&self, id: &str, shell: &str) -> Vec<u64> {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let Some(slot) = inner.slots.get_mut(id) else {
            return Vec::new();
        };
        let mut seqs = Vec::new();
        slot.pending_inputs.retain(|input| match &input.routed {
            Some(note) if note.shell == shell => {
                seqs.push(note.seq);
                false
            }
            _ => true,
        });
        seqs
    }

    fn queue(&self, id: &str, input: PendingInput) -> bool {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        match inner.slots.get_mut(id) {
            Some(slot) if slot.busy && !slot.killed => {
                slot.pending_inputs.push(input);
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
        let Some(slot) = inner.slots.get_mut(id) else {
            return Vec::new();
        };
        let taken = std::mem::take(&mut slot.pending_inputs);
        // Read now, at this round boundary: the report's timeline marks it
        // after the calls made so far — one mark for the lead's messages, one
        // per message the user sent, each quoted (docs/agent-tools.md).
        let at = slot.calls.len();
        if taken.iter().any(|input| !input.from_user) {
            slot.follow_ups.push(FollowUp::lead(at));
        }
        slot.follow_ups
            .extend(
                taken
                    .iter()
                    .filter(|input| input.from_user)
                    .map(|input| FollowUp {
                        at,
                        from_user: Some(input.text.clone()),
                    }),
            );
        taken.into_iter().map(|input| input.text).collect()
    }

    /// Mark that `id` read a message the user sent **outside** its queue — a
    /// session-view chat that continues it with the message already in its
    /// conversation (`spawn_agent_chat`); a queued one is marked as its loop
    /// drains it ([`take_pending_inputs`](Self::take_pending_inputs)).
    pub fn note_user_message(&self, id: &str, text: &str) {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        if let Some(slot) = inner.slots.get_mut(id) {
            let at = slot.calls.len();
            slot.follow_ups.push(FollowUp {
                at,
                from_user: Some(text.to_string()),
            });
        }
    }

    /// The messages the user sent `id` that its **current run** has read —
    /// what the run's answer is told it answered (the foreground result's
    /// note, `user_messages_note`). Empty for an unknown id.
    #[must_use]
    pub fn user_messages(&self, id: &str) -> Vec<String> {
        let inner = self.inner.lock().expect("agent registry poisoned");
        inner.slots.get(id).map_or_else(Vec::new, |slot| {
            slot.follow_ups[slot.run_follow_ups.min(slot.follow_ups.len())..]
                .iter()
                .filter_map(|follow_up| follow_up.from_user.clone())
                .collect()
        })
    }

    /// Does `id` still hold a message nobody has delivered?
    ///
    /// The **settle guard** (`docs/queue.md`): a slot stays `busy` between its
    /// loop's last drain and [`finish`](Self::finish), so a message typed in
    /// that window is accepted by [`queue_input`](Self::queue_input) and then
    /// read by nobody — the loop is over, and the roster's terminal event may
    /// already have been folded, so the boundary's own reconciliation has been
    /// and gone. The run's thread asks this before exiting and continues
    /// itself when the answer is yes.
    #[must_use]
    pub fn has_pending_inputs(&self, id: &str) -> bool {
        let inner = self.inner.lock().expect("agent registry poisoned");
        inner
            .slots
            .get(id)
            .is_some_and(|slot| !slot.pending_inputs.is_empty())
    }

    /// Take the **last** message queued into this agent's loop back — Alt+Up's
    /// pull into the composer inside its session view, and
    /// [`SteerQueue::take_last`](crate::steer::SteerQueue::take_last)'s twin.
    /// `None` once the round boundary has drained it, which is the only
    /// honest answer: a message the loop has read is in its context now.
    /// Only the **user's** own: a message the lead queued (`agentsend`, a
    /// routed shell note) has no row and no composer to come back to, so
    /// taking it would drop it silently.
    pub fn take_last_input(&self, id: &str) -> Option<String> {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let pending = &mut inner.slots.get_mut(id)?.pending_inputs;
        let index = pending.iter().rposition(|input| input.from_user)?;
        Some(pending.remove(index).text)
    }

    /// An `agentoutput` call handed the lead `id`'s settled outcome
    /// (`docs/agent-tools.md` *One notice per answer*): the completion
    /// notice saying the same must not reach it too. Not posted yet — the
    /// loop's [`post_notice`](Self::post_notice) is told to skip it; posted
    /// and still on `board` — taken back, the board remembering its number
    /// so the cell the loop holds for it is dropped too
    /// ([`BackgroundRegistry::take_retracted`]); already taken by the lead —
    /// nothing to do, this was a re-read. Under the registry lock, as the
    /// loop's post is, so the two can never cross.
    ///
    /// [`BackgroundRegistry::take_retracted`]: crate::background::BackgroundRegistry::take_retracted
    pub fn report_settled(&self, id: &str, board: Option<&crate::background::BackgroundRegistry>) {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        let Some(slot) = inner.slots.get_mut(id) else {
            return;
        };
        if slot.notices_owed > 0 {
            slot.notices_owed -= 1;
            return;
        }
        if let Some(board) = board {
            board.retract_agent_notice(id);
        }
    }

    /// The loop handled one of `id`'s **unobserved** settles: post its
    /// completion notice on `board` when it owes one (`context`) — unless a
    /// report already handed the lead that answer
    /// ([`report_settled`](Self::report_settled)). Called for every such
    /// settle, a notice owed or not, so each one is accounted for once.
    /// [`NoticePost::Covered`] when a report covered it: no notice cell is
    /// owed either.
    #[must_use]
    pub fn post_notice(
        &self,
        id: &str,
        board: &crate::background::BackgroundRegistry,
        context: Option<String>,
    ) -> NoticePost {
        let mut inner = self.inner.lock().expect("agent registry poisoned");
        if let Some(slot) = inner.slots.get_mut(id) {
            if slot.notices_owed == 0 {
                return NoticePost::Covered;
            }
            slot.notices_owed -= 1;
        }
        NoticePost::Posted(context.map(|context| board.post_agent_notice(context, id)))
    }

    /// Can this agent take a **new turn** right now — settled, not stopped,
    /// and with a stored conversation a continuation can resume from? The
    /// question the Tab follow-up queue's dispatch asks before it hands a
    /// message over (`docs/queue.md`): the registry is the only honest
    /// answer, exactly as it is for [`spawn_agent_chat`], and asking it is
    /// what keeps a follow-up its **own** turn instead of a steer folded into
    /// a run that is still going.
    ///
    /// [`spawn_agent_chat`]: crate::stream::ReplySource::spawn_agent_chat
    #[must_use]
    pub fn ready_for_turn(&self, id: &str) -> bool {
        let inner = self.inner.lock().expect("agent registry poisoned");
        inner
            .slots
            .get(id)
            .is_some_and(|slot| !slot.busy && !slot.killed && slot.messages.is_some())
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
        // The last run's end goes out ahead of anything this one sends.
        self.release_held(id, slot);
        Some((messages, slot.restart()))
    }
}

/// `1 tool use`, `3 tool uses`.
fn tool_uses_label(count: usize) -> String {
    match count {
        1 => "1 tool use".to_string(),
        n => format!("{n} tool uses"),
    }
}

impl AgentSnapshot {
    /// The report's frame line — where the agent stands, over everything
    /// else a report says: `Running (agent a7k2m9x4q · Fetch GitHub profile ·
    /// 42s · 3 tool uses)`.
    #[must_use]
    pub fn frame(&self) -> String {
        let state = match self.state {
            AgentState::Running => "Running",
            AgentState::Done(_) => "Done",
            AgentState::Failed(_) => "Failed",
            AgentState::Stopped { .. } => "Stopped",
        };
        let task = if self.description.is_empty() {
            String::new()
        } else {
            format!(" · {}", self.description)
        };
        format!(
            "{state} (agent {}{task} · {} · {})",
            self.id,
            crate::app::format_elapsed(self.runtime.as_secs()),
            tool_uses_label(self.tool_uses)
        )
    }
}

/// One reported call, on one line and cut at [`AGENT_REPORT_CALL_CHARS`] —
/// as the report lists it, and as a waiting `agentoutput` streams it.
#[must_use]
pub fn report_call(header: &str) -> String {
    let line = crate::llm::tools::flatten_one_line(header);
    match line.char_indices().nth(AGENT_REPORT_CALL_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}

/// The report line for one delivered message: the bare
/// [`AGENT_FOLLOW_UP_MARK`] for the lead's own, the user's quoted on one
/// line — the lead never saw it, so naming that it arrived is not enough.
fn follow_up_mark(follow_up: &FollowUp) -> String {
    follow_up.from_user.as_deref().map_or_else(
        || AGENT_FOLLOW_UP_MARK.to_string(),
        |text| format!("— message from the user: {} —", report_call(text)),
    )
}

/// `agentoutput`'s report (`docs/agent-tools.md`): the frame line, then what
/// the agent has been doing as a **summary** — one `Name(args)` line per tool
/// call, the newest [`AGENT_REPORT_CALLS_MAX`] with the earlier ones counted,
/// never their outputs — then, once it has settled, what it answered.
#[must_use]
pub fn output_report(snapshot: &AgentSnapshot) -> String {
    let mut lines = vec![snapshot.frame()];
    let hidden = snapshot.calls.len().saturating_sub(AGENT_REPORT_CALLS_MAX);
    match hidden {
        0 => {}
        1 => lines.push("… 1 earlier tool call".to_string()),
        n => lines.push(format!("… {n} earlier tool calls")),
    }
    // The calls, each delivered message marked where it arrived — a mark at
    // index `n` came after the `n`th call. Marks before the kept calls are
    // counted with them, never shown out of place.
    let mut follow_ups = snapshot
        .follow_ups
        .iter()
        .filter(|follow_up| follow_up.at >= hidden)
        .peekable();
    for (index, call) in snapshot.calls.iter().enumerate().skip(hidden) {
        while let Some(follow_up) = follow_ups.next_if(|follow_up| follow_up.at <= index) {
            lines.push(follow_up_mark(follow_up));
        }
        lines.push(report_call(call));
    }
    lines.extend(follow_ups.map(follow_up_mark));
    match &snapshot.state {
        AgentState::Running if snapshot.calls.is_empty() => {
            lines.push("No tool calls yet.".to_string());
        }
        AgentState::Running => {}
        AgentState::Done(response) => {
            let response = response.trim_end_matches('\n');
            lines.push(String::new());
            lines.push("Final response:".to_string());
            lines.push(if response.trim().is_empty() {
                "(no output)".to_string()
            } else {
                response.to_string()
            });
        }
        AgentState::Failed(error) => {
            lines.push(String::new());
            lines.push(format!("Error: {error}"));
        }
        AgentState::Stopped { by_user: true } => {
            lines.push(String::new());
            lines.push(
                "[Stopped by the user — it cannot be resumed; launch a new agent if the \
                 task still needs doing.]"
                    .to_string(),
            );
        }
        AgentState::Stopped { by_user: false } => {
            lines.push(String::new());
            lines.push("[Stopped — its conversation is kept: agentsend resumes it.]".to_string());
        }
    }
    lines.join("\n")
}

/// `agentlist`'s report (`docs/agent-tools.md`): every agent, launch order —
/// its id, type, task, where it stands and for how long, and how many tool
/// calls it has made.
#[must_use]
pub fn list_report(snapshots: &[AgentSnapshot]) -> String {
    if snapshots.is_empty() {
        return "No agents in this session.".to_string();
    }
    let rows: Vec<String> = snapshots
        .iter()
        .map(|snapshot| {
            let elapsed = crate::app::format_elapsed(snapshot.runtime.as_secs());
            let state = match snapshot.state {
                AgentState::Running => format!("running {elapsed}"),
                AgentState::Done(_) => format!("done in {elapsed}"),
                AgentState::Failed(_) => format!("failed after {elapsed}"),
                AgentState::Stopped { by_user: true } => {
                    format!("stopped by the user after {elapsed}")
                }
                AgentState::Stopped { by_user: false } => format!("stopped after {elapsed}"),
            };
            let task = if snapshot.description.is_empty() {
                String::new()
            } else {
                format!(
                    " {}",
                    crate::llm::tools::flatten_one_line(&snapshot.description)
                )
            };
            format!(
                "- {} ({}){task} — {state} · {}",
                snapshot.id,
                snapshot.agent_type,
                tool_uses_label(snapshot.tool_uses)
            )
        })
        .collect();
    let head = match snapshots.len() {
        1 => "1 agent:".to_string(),
        n => format!("{n} agents:"),
    };
    format!("{head}\n{}", rows.join("\n"))
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
    fn the_activity_row_shows_a_file_tools_args_the_way_the_caller_says() {
        // The record keeps the call as it came (`activity` — the roster's
        // own text); a renderer hands `activity_shown` how a file tool's
        // path should read (`docs/tools.md` "Path display") and the row is
        // built from the same grammar. Every other row passes through.
        let shorten = |name: &str, args: &str| {
            if name == "Write" {
                args.replace("/home/u/", "~/")
            } else {
                args.to_string()
            }
        };
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        assert_eq!(run.activity_shown(shorten), "Initializing…");
        assert!(!run.apply(&StreamEvent::ToolStart {
            name: "Write".into(),
            args: "/home/u/x.py".into(),
            detail: None,
            arguments: None,
        }));
        assert_eq!(run.activity(), "Write: /home/u/x.py");
        assert_eq!(run.activity_shown(shorten), "Write: ~/x.py");
        assert!(!run.apply(&StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "cat /home/u/x.py".into(),
            detail: Some("Reading /home/u/x.py".into()),
            arguments: None,
        }));
        assert_eq!(run.activity_shown(shorten), "Bash: Reading /home/u/x.py");
    }

    #[test]
    fn a_multiline_command_flattens_on_the_activity_row() {
        // The summary keeps a `bash` command's newlines for the cell header
        // (`docs/tools.md`); the roster's one dim clipped row flattens them.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        assert!(!run.apply(&StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "cd foo\n  ls   -la".into(),
            detail: None,
            arguments: None,
        }));
        assert_eq!(run.activity(), "Bash: cd foo ls -la");
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
            cache_write: 700,
            ..TokenUsage::default()
        }));
        assert!(run.apply(&StreamEvent::StreamDone));
        let Some(HistoryItem::Summary(summary)) = run.history.last() else {
            panic!("the settle records the summary: {:?}", run.history.last());
        };
        assert_eq!(
            summary.verb, "Worked",
            "the past tense of the verb the view wore (nothing rotated it)"
        );
        assert_eq!(summary.secs, 59);
        assert_eq!(summary.tokens, 6100);
        assert_eq!(summary.cached, 2800);
        assert_eq!(summary.cache_write, 700);
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
    fn usage_frames_seat_the_runs_context_size_and_a_later_frame_replaces_it() {
        // The agent's own context gauge (docs/agent-context-gauge.md): the
        // round's `input` is the whole re-sent context and its `output` joins
        // the next round's — their sum is the context size, the main
        // session's `apply_usage` rule. Unlike the billed tally it does NOT
        // accumulate: the next frame's `input` already carries everything
        // the previous one did.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        assert_eq!(
            run.context_used(),
            0,
            "nothing seeds it until a frame lands"
        );
        run.apply(&StreamEvent::Usage(TokenUsage {
            input: 5_000,
            output: 200,
            ..TokenUsage::default()
        }));
        assert_eq!(run.context_used(), 5_200);
        run.apply(&StreamEvent::Usage(TokenUsage {
            input: 6_000,
            output: 300,
            ..TokenUsage::default()
        }));
        assert_eq!(run.context_used(), 6_300, "replaced, not summed");
        assert_eq!(
            run.tokens, 11_500,
            "while the billed tally keeps accumulating"
        );
        // A turn that saw a frame settles on the frame, never on an estimate.
        run.apply(&chunk("done"));
        run.apply(&StreamEvent::StreamDone);
        assert_eq!(run.context_used(), 6_300, "the provider's count stands");
    }

    #[test]
    fn a_settle_without_a_usage_frame_estimates_the_context_from_the_transcript() {
        // The offline dummy scripts no usage frames on the agent channel, so
        // — the main session's `take_turn_summary` rule — the tokenizer
        // estimate over the agent's own derived context stands in at the
        // settle, and the gauge still moves offline.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "Get Warsaw weather", false);
        run.apply(&chunk("It is 12°C and cloudy in Warsaw."));
        assert_eq!(run.context_used(), 0, "mid-turn nothing has re-counted yet");
        run.apply(&StreamEvent::StreamDone);
        let estimate = run.context_used();
        assert!(estimate > 0, "the prompt and the reply are context");
        assert_eq!(
            estimate,
            crate::app::estimate_messages_tokens(&crate::context::context_messages(&run.history))
                as u64,
            "counted by the one rule the main gauge counts with"
        );
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
        assert!(registry.kill(&id).was_live);
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
        assert!(!registry.kill(&id).was_live, "second kill reports not-live");
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
    fn a_message_queued_into_a_running_agent_waits_on_its_own_transcript() {
        // The main session's queue, one level down (docs/queue.md): the
        // message shows above the box in that agent's session view and is
        // NOT on its transcript yet — the agent has not read it.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::Chunk("working".to_string()));
        run.queue_chat("also check Manila");
        assert_eq!(run.queued, ["also check Manila"]);
        assert!(
            !run.history.iter().any(|item| matches!(
                item,
                HistoryItem::Message(m) if m.role == Role::User && m.text == "also check Manila"
            )),
            "nothing is claimed on the transcript before the agent has it"
        );
    }

    #[test]
    fn its_round_boundary_turns_the_queued_message_into_a_user_bubble() {
        // `StreamEvent::Steered` on the agent channel — the same event the
        // main session's queue resolves through.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::Chunk("reading…".to_string()));
        run.queue_chat("also check Manila");
        run.apply(&StreamEvent::Steered {
            text: "also check Manila".to_string(),
        });
        assert!(run.queued.is_empty(), "it stops waiting above the box");
        assert!(
            matches!(
                run.history.last(),
                Some(HistoryItem::Message(m))
                    if m.role == Role::User && m.text == "also check Manila"
            ),
            "and lands on the agent's own transcript"
        );
        assert!(
            matches!(run.history.first(), Some(HistoryItem::Message(m)) if m.role == Role::User),
            "…after the launch prompt"
        );
    }

    #[test]
    fn a_taken_message_finalises_the_agents_streamed_reply_before_it() {
        // Invariant 4 on the agent's own transcript: the run of text ahead of
        // the interleaved message is finalised first.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::Chunk("Checking Cebu…".to_string()));
        run.apply(&StreamEvent::Steered {
            text: "also check Manila".to_string(),
        });
        let roles: Vec<Role> = run
            .history
            .iter()
            .filter_map(|item| match item {
                HistoryItem::Message(m) => Some(m.role),
                _ => None,
            })
            .collect();
        assert_eq!(
            roles,
            vec![Role::User, Role::Assistant, Role::User],
            "prompt, the finalised segment, then the message the agent just read"
        );
    }

    #[test]
    fn a_message_delivered_to_a_settled_agent_reopens_it() {
        // The settle window's other half (`docs/queue.md`): a message queued
        // between the loop's last drain and `finish` is delivered by a
        // continuation the run's own thread starts — but by then the roster
        // has folded the terminal event, and `apply` drops everything once a
        // status is final. Without the reopen that continuation runs
        // invisibly: the model works, the transcript shows nothing, and the
        // pending row never clears.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&chunk("done"));
        run.apply(&StreamEvent::StreamDone);
        assert!(run.status.is_final(), "settled");
        run.queue_chat("one more thing");
        run.apply(&StreamEvent::Steered {
            text: "one more thing".to_string(),
        });
        assert_eq!(run.status, AgentStatus::Running, "the run is live again");
        assert!(run.queued.is_empty(), "the pending row is spent");
        assert!(
            matches!(
                run.history.last(),
                Some(HistoryItem::Message(m))
                    if m.role == Role::User && m.text == "one more thing"
            ),
            "…and the message is on its transcript"
        );
    }

    #[test]
    fn a_stopped_agent_is_never_reopened_by_a_late_delivery() {
        // The `x` said stop. Nothing should resurrect it.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&chunk("working"));
        run.stopped_by_user = true;
        assert!(run.interrupt());
        run.apply(&StreamEvent::Steered {
            text: "too late".to_string(),
        });
        assert_eq!(run.status, AgentStatus::Interrupted);
        assert!(
            !run.history.iter().any(|item| matches!(
                item,
                HistoryItem::Message(m) if m.text == "too late"
            )),
            "a stopped agent's transcript is closed"
        );
    }

    #[test]
    fn stopping_an_agent_drops_the_messages_its_loop_will_never_read() {
        // The `x` cancels the loop, so nothing will ever take what is still
        // queued for it. Leaving the rows up would promise a delivery that
        // cannot happen — and the roster defers its sweep while the user is
        // inside that agent's view, so the promise would stand indefinitely.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&chunk("working"));
        run.queue_chat("one more thing");
        assert!(run.interrupt());
        assert!(run.queued.is_empty(), "a cancelled loop reads nothing more");
    }

    #[test]
    fn the_registry_reports_an_agent_holding_an_undelivered_message() {
        // The settle window (`docs/queue.md`): a slot stays `busy` between
        // its loop's last drain and `finish`, so a message typed in there is
        // accepted by `queue_input` and then read by nobody — the loop is
        // over, and the roster's terminal event may already have been folded.
        // The run's own thread checks this before exiting and continues.
        let registry = test_registry();
        let (id, _cancel) = registry.register(GENERAL_PURPOSE);
        assert!(!registry.has_pending_inputs(&id), "nothing queued yet");
        assert!(registry.queue_input(&id, "one more thing"));
        assert!(
            registry.has_pending_inputs(&id),
            "the run must not settle leaving this undelivered"
        );
        assert_eq!(registry.take_pending_inputs(&id), ["one more thing"]);
        assert!(!registry.has_pending_inputs(&id), "drained");
    }

    #[test]
    fn take_last_input_reclaims_only_an_undelivered_message() {
        // Alt+Up inside the agent's session view: the registry is the only
        // side that knows whether the round boundary has read it yet.
        let registry = test_registry();
        let (id, _cancel) = registry.register(GENERAL_PURPOSE);
        assert!(registry.queue_user_input(&id, "first"));
        assert!(registry.queue_user_input(&id, "second"));
        assert_eq!(registry.take_last_input(&id).as_deref(), Some("second"));
        assert_eq!(registry.take_pending_inputs(&id), ["first"]);
        assert_eq!(
            registry.take_last_input(&id),
            None,
            "a message the loop has read cannot come back"
        );
        assert_eq!(registry.take_last_input("nope"), None);
    }

    #[test]
    fn ready_for_turn_is_true_only_for_a_settled_unstopped_agent() {
        // The Tab follow-up queue asks this before dispatching, so a
        // follow-up becomes its own continuation turn rather than a steer
        // into a run still going (docs/queue.md).
        let registry = test_registry();
        let (id, _cancel) = registry.register(GENERAL_PURPOSE);
        assert!(!registry.ready_for_turn(&id), "still running");
        registry.finish(&id, Ok("done".into()), vec![ChatMessage::user("hi")]);
        assert!(registry.ready_for_turn(&id), "settled with a conversation");
        assert!(
            !registry.ready_for_turn("nope"),
            "an unknown id takes nothing"
        );
        let (killed, _c2) = registry.register(GENERAL_PURPOSE);
        registry.finish(&killed, Ok("done".into()), vec![ChatMessage::user("hi")]);
        let _ = registry.kill(&killed);
        assert!(
            !registry.ready_for_turn(&killed),
            "the `x` said stop — nothing continues it"
        );
    }

    #[test]
    fn a_stopped_agent_drops_its_follow_up_turns_too() {
        let mut run = AgentRun::new("a1", "Do the thing", GENERAL_PURPOSE, "prompt", false);
        run.queue_chat("read next");
        run.queue_followup("and then this");
        assert!(run.interrupt());
        assert!(run.queued.is_empty() && run.followups.is_empty());
    }

    #[test]
    fn follow_ups_drain_oldest_first_and_alt_up_takes_the_newest() {
        let mut run = AgentRun::new("a1", "Do the thing", GENERAL_PURPOSE, "prompt", false);
        run.queue_followup("first");
        run.queue_followup("second");
        assert_eq!(run.take_last_followup().as_deref(), Some("second"));
        assert_eq!(run.take_followup().as_deref(), Some("first"));
        assert_eq!(run.take_followup(), None);
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

    /// Every `StreamEvent` a subagent's channel can carry, one of each — the
    /// walk `every_flush_point_is_declared` uses so a new variant cannot slip
    /// past the predicate.
    fn one_of_every_event() -> Vec<(&'static str, StreamEvent)> {
        vec![
            ("Chunk", StreamEvent::Chunk("hi".to_string())),
            (
                "HookNote",
                StreamEvent::HookNote {
                    label: "SubagentStop".to_string(),
                    text: "keep going".to_string(),
                },
            ),
            (
                "Steered",
                StreamEvent::Steered {
                    text: "one more thing".to_string(),
                },
            ),
            ("ThinkingChunk", StreamEvent::ThinkingChunk("t".to_string())),
            ("ToolCallDelta", StreamEvent::ToolCallDelta("{".to_string())),
            (
                "ToolBatch",
                StreamEvent::ToolBatch(vec![ToolCallSummary {
                    name: "Bash".to_string(),
                    args: "ls".to_string(),
                }]),
            ),
            (
                "ToolStart",
                StreamEvent::ToolStart {
                    name: "Bash".to_string(),
                    args: "ls".to_string(),
                    detail: None,
                    arguments: None,
                },
            ),
            ("ToolNote", StreamEvent::ToolNote("noted".to_string())),
            ("ToolOutput", StreamEvent::ToolOutput("out".to_string())),
            (
                "ToolScreen",
                StreamEvent::ToolScreen {
                    settled: "done\n".to_string(),
                    live: "45%".to_string(),
                },
            ),
            ("ToolTitle", StreamEvent::ToolTitle("sudo x".to_string())),
            (
                "ToolEnd",
                StreamEvent::ToolEnd {
                    output: "out".to_string(),
                    ok: true,
                    truncated: false,
                },
            ),
            (
                "ToolRejected",
                StreamEvent::ToolRejected {
                    display: "no".to_string(),
                    result: "refused".to_string(),
                    truncated: false,
                },
            ),
            (
                "ToolAnswered",
                StreamEvent::ToolAnswered {
                    display: "done".to_string(),
                    result: "ack".to_string(),
                    truncated: false,
                },
            ),
            (
                "ToolBackgrounded",
                StreamEvent::ToolBackgrounded {
                    id: "b1".to_string(),
                    output: "backgrounded".to_string(),
                },
            ),
            ("StreamDone", StreamEvent::StreamDone),
            ("Error", StreamEvent::Error("boom".to_string())),
        ]
    }

    #[test]
    fn a_subagents_terminal_screen_redraws_in_place() {
        // The session view folds a subagent's terminal output exactly as the
        // main session does (docs/interactive-shell.md): settled text
        // appends, live rows replace the last ones, a refined header lands.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "sudo pacman -Syy".into(),
            detail: None,
            arguments: None,
        });
        run.apply(&StreamEvent::ToolScreen {
            settled: String::new(),
            live: " extra  10%".into(),
        });
        run.apply(&StreamEvent::ToolScreen {
            settled: ":: Synchronizing\n".into(),
            live: " extra  50%".into(),
        });
        assert_eq!(
            run.tool_queue.front().expect("running").output,
            ":: Synchronizing\n extra  50%"
        );
        run.apply(&StreamEvent::ToolTitle("sudo pacman -Syy ← y⏎".into()));
        assert_eq!(
            run.tool_queue.front().expect("running").args,
            "sudo pacman -Syy ← y⏎"
        );
        // The next call starts with no live tail of its own.
        run.apply(&StreamEvent::ToolEnd {
            output: "Exit code: 0".into(),
            ok: true,
            truncated: false,
        });
        run.apply(&StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "ls".into(),
            detail: None,
            arguments: None,
        });
        run.apply(&StreamEvent::ToolScreen {
            settled: String::new(),
            live: "x".into(),
        });
        assert_eq!(run.tool_queue.front().expect("running").output, "x");
    }

    #[test]
    fn a_subagents_window_follows_its_moving_rows() {
        // The session view's running cell anchors on the rows still moving
        // exactly as the main one does (docs/tool-streaming.md): each live
        // row is stamped with the agent's own command clock, and a new call
        // starts from scratch, clock included.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "sudo pacman -Syy".into(),
            detail: None,
            arguments: None,
        });
        run.set_command_elapsed(Some(std::time::Duration::from_secs(1)));
        run.apply(&StreamEvent::ToolScreen {
            settled: String::new(),
            live: " core 10%\n extra 100%".into(),
        });
        run.set_command_elapsed(Some(std::time::Duration::from_secs(4)));
        run.apply(&StreamEvent::ToolScreen {
            settled: String::new(),
            live: " core 20%\n extra 100%".into(),
        });
        assert_eq!(
            run.live_tail().anchor(2),
            0,
            "the window follows the row still moving"
        );
        run.apply(&StreamEvent::ToolEnd {
            output: "Exit code: 0".into(),
            ok: true,
            truncated: false,
        });
        run.apply(&StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "ls".into(),
            detail: None,
            arguments: None,
        });
        assert_eq!(
            run.command_elapsed, None,
            "a new command has run no time yet"
        );
        assert_eq!(*run.live_tail(), crate::app::LiveTail::default());
    }

    #[test]
    fn every_flush_point_is_declared() {
        // `AgentRun::flushes_segment` is the boundary's only source of truth
        // for where the session view must finish + reset its `StreamRender`
        // (`docs/agent-view-streaming.md`). It has to agree with what `apply`
        // actually does to the buffer for **every** event, or the view falls
        // behind the fold again — which is how `HookNote` came to drop a
        // reply's withheld tail and leave the render pointing at a buffer
        // that no longer existed.
        for (name, event) in one_of_every_event() {
            let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
            run.apply(&StreamEvent::Chunk("partial answer".to_string()));
            run.apply(&event);
            // `flush_segment` *takes* the buffer, so a finalised segment
            // leaves `None` — a `Chunk` merely appending to it has not
            // finalised anything.
            let consumed = run.streaming.is_none();
            assert_eq!(
                AgentRun::flushes_segment(&event),
                consumed,
                "{name}: the predicate and the fold disagree about the segment"
            );
        }
    }

    #[test]
    fn a_batch_announcement_is_the_rounds_whole_queue() {
        // `App::start_tool_batch` *replaces* the queue — the announcement is
        // the round's whole set of calls (`docs/parallel-tools.md`). The
        // agent's fold appended instead, so an entry left over from an
        // earlier round would sit at the front and every resolution in the
        // new round would land on the wrong cell.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::ToolBatch(vec![ToolCallSummary {
            name: "Bash".to_string(),
            args: "stale".to_string(),
        }]));
        run.apply(&StreamEvent::ToolBatch(vec![
            ToolCallSummary {
                name: "Read".to_string(),
                args: "a.md".to_string(),
            },
            ToolCallSummary {
                name: "Read".to_string(),
                args: "b.md".to_string(),
            },
        ]));
        let queued: Vec<&str> = run.tool_queue.iter().map(|t| t.args.as_str()).collect();
        assert_eq!(queued, ["a.md", "b.md"], "the new batch is the queue");
    }

    /// A run mid-batch: `args` announced, the first call started — or,
    /// with `started` false, still waiting on its permission prompt.
    fn run_mid_batch(args: &[&str], started: bool) -> AgentRun {
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::ToolBatch(
            args.iter()
                .map(|args| ToolCallSummary {
                    name: "Bash".to_string(),
                    args: (*args).to_string(),
                })
                .collect(),
        ));
        if started {
            run.apply(&StreamEvent::ToolStart {
                name: "Bash".to_string(),
                args: args[0].to_string(),
                detail: None,
                arguments: None,
            });
        }
        run
    }

    /// Every tool record on `run`'s transcript as `(args, status, output)`.
    fn resolved(run: &AgentRun) -> Vec<(String, ToolStatus, String)> {
        run.history
            .iter()
            .filter_map(|item| match item {
                HistoryItem::Tool(t) => Some((t.args.clone(), t.status, t.output.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn stopping_an_agent_mid_batch_resolves_every_call_in_it() {
        // The main session's rule, one level down (docs/interrupt.md): the
        // running call AND every `⎿ Waiting…` sibling behind it resolve as
        // `Interrupted by user` on the agent's own transcript, so its session
        // view and its Ctrl+D keep a cell per call the agent made.
        let interrupted = crate::app::INTERRUPT_TOOL_OUTPUT.to_string();
        for started in [true, false] {
            let mut run = run_mid_batch(&["ping a", "ls"], started);
            assert!(run.interrupt());
            assert_eq!(
                resolved(&run),
                vec![
                    (
                        "ping a".to_string(),
                        ToolStatus::Failed,
                        interrupted.clone()
                    ),
                    ("ls".to_string(), ToolStatus::Failed, interrupted.clone()),
                ],
                "front started: {started}"
            );
            assert!(run.tool_queue.is_empty());
        }
    }

    #[test]
    fn an_agent_error_mid_batch_resolves_every_call_in_it() {
        let mut run = run_mid_batch(&["ping a", "ls"], true);
        assert!(run.apply(&StreamEvent::Error("boom".into())));
        let failed: Vec<(String, ToolStatus)> = resolved(&run)
            .into_iter()
            .map(|(args, status, _)| (args, status))
            .collect();
        assert_eq!(
            failed,
            vec![
                ("ping a".to_string(), ToolStatus::Failed),
                ("ls".to_string(), ToolStatus::Failed),
            ]
        );
        assert!(run.tool_queue.is_empty());
    }

    #[test]
    fn every_resolution_records_a_truncated_result() {
        // The main boundary flags `truncated` on all three resolutions
        // (`tui::stream`'s ToolEnd/ToolRejected/ToolAnswered arms), so the
        // expanded cell appends its dim `…` marker. The agent's fold honoured
        // it on `ToolEnd` alone, so a subagent's capped `write`/`skill`
        // result silently claimed to be whole.
        let start = StreamEvent::ToolStart {
            name: "Write".to_string(),
            args: "f.py".to_string(),
            detail: None,
            arguments: None,
        };
        let resolutions: [(&str, StreamEvent); 3] = [
            (
                "ToolEnd",
                StreamEvent::ToolEnd {
                    output: "out".to_string(),
                    ok: true,
                    truncated: true,
                },
            ),
            (
                "ToolRejected",
                StreamEvent::ToolRejected {
                    display: "no".to_string(),
                    result: "refused".to_string(),
                    truncated: true,
                },
            ),
            (
                "ToolAnswered",
                StreamEvent::ToolAnswered {
                    display: "done".to_string(),
                    result: "ack".to_string(),
                    truncated: true,
                },
            ),
        ];
        for (name, event) in resolutions {
            let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
            run.apply(&start);
            run.apply(&event);
            let Some(HistoryItem::Tool(tool)) = run.history.last() else {
                panic!("{name} recorded no cell");
            };
            assert!(tool.truncated, "{name} dropped the truncation marker");
        }
    }

    #[test]
    fn a_backgrounded_call_charges_its_acknowledgement() {
        // The main session's `resolve_front_tool` folds every resolution's
        // output into the `↑` tally — a `run_in_background` launch included,
        // since its acknowledgement is text the model reads. The agent's fold
        // charged nothing for it.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.apply(&StreamEvent::ToolStart {
            name: "Bash".to_string(),
            args: "sleep 30".to_string(),
            detail: None,
            arguments: None,
        });
        let before = run.tokens;
        run.apply(&StreamEvent::ToolBackgrounded {
            id: "b1".to_string(),
            output: "Command running in the background with id b1".to_string(),
        });
        assert!(
            run.tokens > before,
            "the launch acknowledgement is uploaded text: {before} -> {}",
            run.tokens
        );
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
    fn an_agent_waiting_for_the_connection_says_so_until_content_arrives() {
        // Main parity (`docs/offline.md`): the wait begins at the runtime the
        // first announcement lands on, later announcements move the count
        // on and keep that start, a retry announcement or any content ends
        // it, and it never shows beside the bounded retry clause.
        let mut run = AgentRun::new("a1", "d", GENERAL_PURPOSE, "p", false);
        run.runtime = Duration::from_secs(7);
        run.apply(&StreamEvent::Retrying { attempt: 1, max: 3 });
        run.apply(&StreamEvent::Offline {
            host: "api.venice.ai".into(),
            attempts: 1,
        });
        assert_eq!(
            run.offline,
            Some(crate::app::OfflineInfo {
                host: "api.venice.ai".into(),
                attempts: 1,
                began: Duration::from_secs(7),
            })
        );
        assert!(run.retry.is_none(), "the outage replaces the retry clause");
        run.runtime = Duration::from_secs(19);
        run.apply(&StreamEvent::Offline {
            host: "api.venice.ai".into(),
            attempts: 2,
        });
        let info = run.offline.as_ref().expect("still waiting");
        assert_eq!((info.attempts, info.began), (2, Duration::from_secs(7)));
        run.apply(&chunk("back"));
        assert!(run.offline.is_none(), "content ends the wait");

        run.apply(&StreamEvent::Offline {
            host: "api.venice.ai".into(),
            attempts: 1,
        });
        run.apply(&StreamEvent::Retrying { attempt: 1, max: 3 });
        assert!(
            run.offline.is_none(),
            "the host answered: a retry ends the wait"
        );
        assert!(run.retry.is_some());
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

    // --- The lead's companions (`docs/agent-tools.md`) ---

    /// A registry and the receiving end of its channel, for the tests that
    /// read what it tells the loop.
    fn watched_registry() -> (
        AgentRegistry,
        tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (AgentRegistry::new(tx), rx)
    }

    fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    #[test]
    fn a_registered_agent_keeps_what_it_was_launched_as() {
        let registry = test_registry();
        let (id, _cancel) =
            registry.register_agent("explore", "Fetch GitHub profile", "Get linuztx's profile");
        assert_eq!(
            registry.description(&id).as_deref(),
            Some("Fetch GitHub profile")
        );
        let snap = registry.snapshot(&id).expect("registered");
        assert_eq!(snap.id, id);
        assert_eq!(snap.agent_type, "explore");
        assert_eq!(snap.description, "Fetch GitHub profile");
        assert_eq!(snap.state, AgentState::Running);
        assert_eq!(snap.tool_uses, 0);
        assert!(registry.snapshot("nope").is_none());
        assert!(registry.description("nope").is_none());
    }

    #[test]
    fn a_snapshot_lists_the_calls_made_and_how_the_agent_settled() {
        let registry = test_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        registry.record_call(&id, "Bash(curl -s https://api.github.com/users/linuztx)");
        registry.record_call(&id, "Read(notes.md)");
        let running = registry.snapshot(&id).unwrap();
        assert_eq!(running.tool_uses, 2);
        assert_eq!(
            running.calls,
            [
                "Bash(curl -s https://api.github.com/users/linuztx)",
                "Read(notes.md)"
            ]
        );
        registry.finish(&id, Ok("the answer".into()), vec![ChatMessage::user("p")]);
        assert_eq!(
            registry.snapshot(&id).unwrap().state,
            AgentState::Done("the answer".into())
        );
        let (failed, _c) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        registry.finish(&failed, Err("rate limited".into()), Vec::new());
        assert_eq!(
            registry.snapshot(&failed).unwrap().state,
            AgentState::Failed("rate limited".into())
        );
    }

    #[test]
    fn settling_with_nobody_waiting_sends_the_terminal_event_at_once() {
        // The foreground group's wait loop sees `is_done` under the same lock
        // the event is sent under, so its resolution always finds the event
        // already queued (docs/agent-tools.md *One notice per answer*).
        let (registry, mut rx) = watched_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        registry.settle(
            &id,
            Ok("done".into()),
            Vec::new(),
            Some(StreamEvent::StreamDone),
        );
        assert!(registry.is_done(&id));
        assert_eq!(
            drain(&mut rx),
            [AgentEvent::Settled {
                id: id.clone(),
                event: StreamEvent::StreamDone,
                observed: false,
            }]
        );
    }

    #[test]
    fn a_waiter_holds_the_terminal_event_and_releases_it_observed() {
        // An `agentoutput` waiting on the agent reports its outcome itself, so
        // the settle it saw owes no completion notice.
        let (registry, mut rx) = watched_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert!(registry.begin_wait(&id));
        registry.settle(
            &id,
            Ok("done".into()),
            Vec::new(),
            Some(StreamEvent::StreamDone),
        );
        assert!(drain(&mut rx).is_empty(), "held for the waiter");
        let snap = registry.end_wait(&id, true).expect("still known");
        assert_eq!(snap.state, AgentState::Done("done".into()));
        assert_eq!(
            drain(&mut rx),
            [AgentEvent::Settled {
                id: id.clone(),
                event: StreamEvent::StreamDone,
                observed: true,
            }]
        );
    }

    #[test]
    fn a_waiter_that_reports_nothing_releases_the_event_unobserved() {
        // Esc cancelled the wait: the lead never read the outcome, so the
        // notice is still owed.
        let (registry, mut rx) = watched_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert!(registry.begin_wait(&id));
        registry.settle(
            &id,
            Err("boom".into()),
            Vec::new(),
            Some(StreamEvent::Error("boom".into())),
        );
        let _ = registry.end_wait(&id, false);
        assert_eq!(
            drain(&mut rx),
            [AgentEvent::Settled {
                id,
                event: StreamEvent::Error("boom".into()),
                observed: false,
            }]
        );
    }

    #[test]
    fn a_wait_that_ends_first_leaves_the_settle_its_own_notice() {
        // The wait passed with the agent still running: what settles later
        // was never reported, so it goes out unobserved, at once.
        let (registry, mut rx) = watched_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert!(registry.begin_wait(&id));
        let snap = registry.end_wait(&id, true).expect("known");
        assert_eq!(snap.state, AgentState::Running);
        registry.settle(
            &id,
            Ok("done".into()),
            Vec::new(),
            Some(StreamEvent::StreamDone),
        );
        assert_eq!(
            drain(&mut rx),
            [AgentEvent::Settled {
                id,
                event: StreamEvent::StreamDone,
                observed: false,
            }]
        );
        assert!(!registry.begin_wait("nope"), "nothing to wait on");
        assert!(registry.end_wait("nope", true).is_none());
    }

    #[test]
    fn a_users_stop_says_whether_a_waiting_report_covers_it() {
        let registry = test_registry();
        let (watched, _c1) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert!(registry.begin_wait(&watched));
        let stop = registry.kill(&watched);
        assert!(stop.was_live && stop.watched, "{stop:?}");
        let (alone, _c2) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        let stop = registry.kill(&alone);
        assert!(stop.was_live && !stop.watched, "{stop:?}");
        assert_eq!(
            registry.snapshot(&alone).unwrap().state,
            AgentState::Stopped { by_user: true }
        );
    }

    #[test]
    fn the_leads_stop_tells_the_loop_and_keeps_the_agent_resumable() {
        // `agentkill`: no event of the run's own will say it stopped (a
        // cancelled loop returns silently), so the registry says so — and
        // unlike the user's `x`, the conversation stays resumable.
        let (registry, mut rx) = watched_registry();
        let (id, cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        let stop = registry.stop(&id).expect("known");
        assert!(stop.was_live);
        assert!(cancel.is_cancelled());
        assert_eq!(drain(&mut rx), [AgentEvent::Stopped { id: id.clone() }]);
        assert_eq!(
            registry.snapshot(&id).unwrap().state,
            AgentState::Stopped { by_user: false }
        );
        assert_eq!(
            registry.resume(&id, "redirect"),
            Resume::Stopping,
            "its thread has not let go yet"
        );
        registry.finish(&id, Err("stopped".into()), vec![ChatMessage::user("p")]);
        assert!(matches!(
            registry.resume(&id, "redirect"),
            Resume::Continue { .. }
        ));
        let again = registry.stop(&id).expect("known");
        assert!(again.was_live, "the resumed run is live again");
        assert!(registry.stop("nope").is_none());
    }

    #[test]
    fn resume_queues_into_a_running_agent() {
        let registry = test_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert_eq!(registry.resume(&id, "also check gists"), Resume::Queued);
        assert_eq!(registry.take_pending_inputs(&id), ["also check gists"]);
    }

    #[test]
    fn resume_continues_a_settled_agent_from_its_stored_conversation() {
        // The follow-up: a finished agent keeps its context. The message
        // rides the pending inputs so the continuation's first round
        // announces it (`Steered`) — which is what records it on the agent's
        // transcript and reopens its roster row.
        let (registry, mut rx) = watched_registry();
        let (id, _cancel) =
            registry.register_agent("explore", "Fetch GitHub profile", "Get the profile");
        registry.finish(
            &id,
            Ok("done".into()),
            vec![ChatMessage::user("Get the profile")],
        );
        let Resume::Continue {
            messages,
            cancel,
            agent_type,
        } = registry.resume(&id, "also list the repos")
        else {
            panic!("a settled agent continues");
        };
        assert_eq!(messages.len(), 1, "the stored conversation");
        assert_eq!(agent_type, "explore");
        assert!(!cancel.is_cancelled());
        assert!(!registry.is_done(&id), "running again");
        assert_eq!(registry.snapshot(&id).unwrap().state, AgentState::Running);
        assert_eq!(registry.take_pending_inputs(&id), ["also list the repos"]);
        assert_eq!(
            drain(&mut rx),
            [AgentEvent::Resumed {
                id: id.clone(),
                description: "Fetch GitHub profile".into(),
                agent_type: "explore".into(),
                prompt: "Get the profile".into(),
            }],
            "the roster is told before the continuation's first event"
        );
    }

    #[test]
    fn resume_refuses_an_agent_the_user_stopped_or_never_had() {
        let registry = test_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        let _ = registry.kill(&id);
        registry.finish(&id, Err("stopped".into()), vec![ChatMessage::user("p")]);
        assert_eq!(registry.resume(&id, "go on"), Resume::StoppedByUser);
        assert_eq!(registry.resume("nope", "go on"), Resume::Unknown);
    }

    #[test]
    fn a_resume_releases_a_terminal_event_still_held_for_a_waiter() {
        // The continuation's events must never overtake the previous run's
        // terminal event, or the roster settles a run that is going again.
        let (registry, mut rx) = watched_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert!(registry.begin_wait(&id));
        registry.settle(
            &id,
            Ok("done".into()),
            vec![ChatMessage::user("p")],
            Some(StreamEvent::StreamDone),
        );
        assert!(matches!(
            registry.resume(&id, "more"),
            Resume::Continue { .. }
        ));
        let events = drain(&mut rx);
        assert!(
            matches!(
                events.first(),
                Some(AgentEvent::Settled {
                    observed: false,
                    ..
                })
            ),
            "released first, unobserved: {events:?}"
        );
        let snap = registry.end_wait(&id, true).expect("known");
        assert_eq!(
            snap.state,
            AgentState::Running,
            "the waiter sees the resumed run"
        );
        assert!(drain(&mut rx).is_empty(), "nothing released twice");
    }

    #[test]
    fn the_registry_keeps_only_the_newest_settled_agents() {
        let registry = test_registry();
        let mut ids = Vec::new();
        for n in 0..=AGENT_RETAINED_MAX {
            let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, &format!("task {n}"), "p");
            registry.finish(&id, Ok("done".into()), Vec::new());
            ids.push(id);
        }
        assert!(
            registry.snapshot(&ids[0]).is_none(),
            "the oldest settled agent is let go"
        );
        for id in &ids[1..] {
            assert!(registry.snapshot(id).is_some(), "{id} is kept");
        }
        // A running agent never counts against the cap, and is never evicted.
        let (running, _cancel) = registry.register_agent(GENERAL_PURPOSE, "live", "p");
        assert!(registry.snapshot(&running).is_some());
        assert!(registry.snapshot(&ids[1]).is_some());
    }

    #[test]
    fn snapshots_list_the_agents_in_launch_order() {
        let registry = test_registry();
        let (first, _c1) = registry.register_agent(GENERAL_PURPOSE, "first", "p");
        let (second, _c2) = registry.register_agent("explore", "second", "p");
        let listed: Vec<String> = registry
            .snapshots()
            .into_iter()
            .map(|snap| snap.id)
            .collect();
        assert_eq!(listed, [first, second]);
    }

    #[test]
    fn clear_forgets_every_agent() {
        let registry = test_registry();
        let (id, cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        registry.clear();
        assert!(
            cancel.is_cancelled(),
            "a wiped conversation stops its agents"
        );
        assert!(registry.snapshot(&id).is_none());
        assert!(registry.snapshots().is_empty());
    }

    #[test]
    fn a_settled_agents_runtime_stands_still() {
        let registry = test_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        registry.finish(&id, Ok("done".into()), Vec::new());
        let first = registry.snapshot(&id).unwrap().runtime;
        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(registry.snapshot(&id).unwrap().runtime, first);
    }

    #[test]
    fn a_resumed_agents_runtime_and_calls_add_up_over_its_runs() {
        // One agent, one life: its report's numbers cover every run, so a
        // follow-up's frame never reads as that run's alone (a live model read
        // `9s · 5 tool uses` as five calls the follow-up made — it made none).
        let registry = test_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        registry.record_call(&id, "Bash(curl one)");
        std::thread::sleep(Duration::from_millis(20));
        registry.finish(&id, Ok("first".into()), vec![ChatMessage::user("p")]);
        let first = registry.snapshot(&id).unwrap().runtime;
        assert!(matches!(
            registry.resume(&id, "and then?"),
            Resume::Continue { .. }
        ));
        let _ = registry.take_pending_inputs(&id);
        std::thread::sleep(Duration::from_millis(20));
        registry.finish(&id, Ok("second".into()), Vec::new());
        let snap = registry.snapshot(&id).unwrap();
        assert!(snap.runtime > first, "{:?} after {first:?}", snap.runtime);
        assert_eq!(snap.tool_uses, 1);
        assert_eq!(
            snap.follow_ups,
            [FollowUp {
                at: 1,
                from_user: None
            }],
            "the message arrived after the first call"
        );
    }

    #[test]
    fn every_message_an_agent_reads_marks_its_timeline() {
        // A message the loop drains (a queued `agentsend`, a resume's, one the
        // user typed into its session) and a chat continuation alike: the
        // report shows where in the agent's calls each arrived.
        let registry = test_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert!(registry.take_pending_inputs(&id).is_empty());
        assert!(
            registry.snapshot(&id).unwrap().follow_ups.is_empty(),
            "nothing read"
        );
        registry.record_call(&id, "Read(a)");
        assert!(registry.queue_input(&id, "also b"));
        let _ = registry.take_pending_inputs(&id);
        registry.record_call(&id, "Read(b)");
        registry.note_user_message(&id, "and c");
        let at: Vec<usize> = registry
            .snapshot(&id)
            .unwrap()
            .follow_ups
            .iter()
            .map(|follow_up| follow_up.at)
            .collect();
        assert_eq!(at, [1, 2]);
    }

    #[test]
    fn a_message_the_user_typed_is_told_apart_from_the_leads() {
        // The lead must learn which messages the *user* sent its agent — a
        // session-view chat it never wrote (docs/agent-tools.md). The report
        // quotes the user's message where it arrived; the lead's own
        // `agentsend` (and a routed shell note) stays the bare mark.
        let registry = test_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert!(registry.queue_input(&id, "lead's own"));
        let _ = registry.take_pending_inputs(&id);
        registry.record_call(&id, "Read(a)");
        assert!(registry.queue_user_input(&id, "also check\nManila"));
        assert_eq!(
            registry.take_pending_inputs(&id),
            ["also check\nManila"],
            "the loop reads the text either way"
        );
        assert_eq!(registry.user_messages(&id), ["also check\nManila"]);
        let report = output_report(&registry.snapshot(&id).unwrap());
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(
            lines[1..4],
            [
                AGENT_FOLLOW_UP_MARK,
                "Read(a)",
                "— message from the user: also check Manila —"
            ],
            "{report}"
        );
    }

    #[test]
    fn the_users_messages_are_the_current_runs_own() {
        // What a run's answer is told it answered: a continuation starts
        // with none, then counts the chat that started it.
        let registry = test_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert!(registry.queue_user_input(&id, "first"));
        let _ = registry.take_pending_inputs(&id);
        registry.finish(&id, Ok("one".into()), vec![ChatMessage::user("p")]);
        assert_eq!(registry.user_messages(&id), ["first"]);
        assert!(registry.begin_continuation(&id).is_some());
        assert!(registry.user_messages(&id).is_empty(), "a new run");
        registry.note_user_message(&id, "second");
        assert_eq!(registry.user_messages(&id), ["second"]);
        assert!(registry.user_messages("nope").is_empty());
    }

    #[test]
    fn alt_up_takes_back_a_users_queued_message() {
        let registry = test_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert!(registry.queue_user_input(&id, "oops"));
        assert!(registry.has_pending_inputs(&id));
        assert_eq!(registry.take_last_input(&id).as_deref(), Some("oops"));
        assert!(registry.take_pending_inputs(&id).is_empty());
        assert!(registry.user_messages(&id).is_empty(), "never read");
    }

    #[test]
    fn alt_up_never_takes_back_the_leads_message() {
        // Alt+Up pulls back what the *user* typed: a message the lead's
        // `agentsend` queued has no row and no composer to return to, so
        // taking it would silently drop it.
        let registry = test_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert!(registry.queue_user_input(&id, "mine"));
        assert!(matches!(registry.resume(&id, "the lead's"), Resume::Queued));
        assert_eq!(registry.take_last_input(&id).as_deref(), Some("mine"));
        assert_eq!(
            registry.take_last_input(&id),
            None,
            "only the lead's is left"
        );
        assert_eq!(registry.take_pending_inputs(&id), ["the lead's"]);
    }

    #[test]
    fn a_stop_drops_the_messages_its_loop_never_read() {
        // A stopped loop reads nothing more, and the roster drops the rows
        // that promised a delivery — so the registry drops the messages too,
        // or a later continuation delivers them unannounced, the user's
        // passing as the lead's (docs/queue.md).
        let registry = test_registry();
        let (by_user, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert!(registry.queue_user_input(&by_user, "too late"));
        let _ = registry.kill(&by_user);
        assert!(!registry.has_pending_inputs(&by_user));

        let (by_lead, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert!(registry.queue_user_input(&by_lead, "too late"));
        let _ = registry.stop(&by_lead);
        assert!(!registry.has_pending_inputs(&by_lead));
        registry.settle(
            &by_lead,
            Ok(String::new()),
            vec![ChatMessage::user("p")],
            None,
        );
        assert!(matches!(
            registry.resume(&by_lead, "go on"),
            Resume::Continue { .. }
        ));
        assert_eq!(registry.take_pending_inputs(&by_lead), ["go on"]);
    }

    /// A finished agent whose completion notice went out unobserved, with a
    /// board to post it on.
    fn settled_with_notice() -> (AgentRegistry, String, crate::background::BackgroundRegistry) {
        let registry = test_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        registry.settle(
            &id,
            Ok("answer".into()),
            vec![ChatMessage::user("p")],
            Some(StreamEvent::StreamDone),
        );
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let board = crate::background::BackgroundRegistry::new(tx, std::env::temp_dir());
        (registry, id, board)
    }

    /// A board whose shell `echo done` exited with nobody waiting — its exit
    /// unread — reaching `agents` for the notes routed to an agent's queue.
    #[cfg(unix)]
    /// A shell `agent` launched, exited with nobody watching — its note not
    /// yet routed.
    fn exited_shell(
        agents: &AgentRegistry,
        agent: &str,
    ) -> (crate::background::BackgroundRegistry, String) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "alter-zero-agents-route-test-{}-{seq}",
            std::process::id()
        ));
        let board = crate::background::BackgroundRegistry::new(tx, dir).with_agents(agents.clone());
        let origin = crate::background::BgOrigin {
            agent_id: agent.to_string(),
            agent_type: GENERAL_PURPOSE.to_string(),
        };
        let task = board
            .launch_from("echo done", None, true, Some(origin))
            .expect("launches");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match rx.try_recv() {
                Ok(crate::background::BgEvent::Exited {
                    id,
                    observed: false,
                    ..
                }) if id == task.id => return (board, id),
                Ok(_) => {}
                Err(_) => {
                    assert!(std::time::Instant::now() < deadline, "never exited");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_routed_exit_is_claimed_by_its_launcher_from_its_own_queue() {
        // One level down, the reported race: a subagent's shell ended while
        // the subagent was writing a call to it, and the note went to the
        // subagent's own queue, unread. Its call reports the exit itself and
        // takes the note back (docs/bash-tools.md *One notice per exit*).
        let registry = test_registry();
        let (agent, _cancel) = registry.register(GENERAL_PURPOSE);
        let (board, shell) = exited_shell(&registry, &agent);
        assert!(matches!(
            registry.route_shell_note(&agent, &shell, "[background] note", false, &board),
            Route::Routed(_)
        ));
        assert!(
            board.claim_exit(&shell, None).is_none(),
            "the lead cannot take a subagent's note"
        );
        let exit = board
            .claim_exit(&shell, Some(&agent))
            .expect("its launcher claims it from its own queue");
        assert_eq!(exit.code, Some(0));
        assert!(
            registry.take_pending_inputs(&agent).is_empty(),
            "the note it would have read is gone"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_routed_exit_the_agent_already_read_is_not_claimed() {
        let registry = test_registry();
        let (agent, _cancel) = registry.register(GENERAL_PURPOSE);
        let (board, shell) = exited_shell(&registry, &agent);
        registry.route_shell_note(&agent, &shell, "[background] note", false, &board);
        assert_eq!(registry.take_pending_inputs(&agent), ["[background] note"]);
        assert!(board.claim_exit(&shell, Some(&agent)).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn an_exit_a_call_already_reported_routes_nothing() {
        let registry = test_registry();
        let (agent, _cancel) = registry.register(GENERAL_PURPOSE);
        let (board, shell) = exited_shell(&registry, &agent);
        assert!(board.claim_exit(&shell, Some(&agent)).is_some());
        assert_eq!(
            registry.route_shell_note(&agent, &shell, "[background] note", false, &board),
            Route::Covered
        );
        assert!(registry.take_pending_inputs(&agent).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_settled_launcher_leaves_its_note_to_the_board() {
        let registry = test_registry();
        let (agent, _cancel) = registry.register(GENERAL_PURPOSE);
        registry.settle(&agent, Ok("done".into()), Vec::new(), None);
        let (board, shell) = exited_shell(&registry, &agent);
        assert_eq!(
            registry.route_shell_note(&agent, &shell, "[background] note", false, &board),
            Route::Unheard
        );
        assert!(
            board
                .post_exit_notice(&shell, "[background] note".into(), true)
                .is_some(),
            "the board takes it, still tracked"
        );
    }

    /// The board number of a notice the loop posted as owed.
    fn posted(post: NoticePost) -> u64 {
        match post {
            NoticePost::Posted(Some(seq)) => seq,
            other => panic!("expected an owed, posted notice, got {other:?}"),
        }
    }

    #[test]
    fn a_report_before_the_notice_is_posted_cancels_it() {
        // `agentoutput` read the answer before the loop got to its notice:
        // the loop must not post it (docs/agent-tools.md *One notice per
        // answer*).
        let (registry, id, board) = settled_with_notice();
        registry.report_settled(&id, Some(&board));
        assert_eq!(
            registry.post_notice(&id, &board, Some("note".into())),
            NoticePost::Covered
        );
        assert!(board.take_pending_notices().is_empty());
    }

    #[test]
    fn a_report_retracts_a_posted_notice_the_lead_has_not_taken() {
        let (registry, id, board) = settled_with_notice();
        let seq = posted(registry.post_notice(&id, &board, Some("note".into())));
        registry.report_settled(&id, Some(&board));
        assert!(board.take_pending_notices().is_empty(), "off the board");
        assert!(board.take_retracted(seq), "its cell is not owed either");
        assert!(!board.take_retracted(seq), "once");
    }

    #[test]
    fn a_report_after_the_lead_took_the_notice_is_a_re_read() {
        let (registry, id, board) = settled_with_notice();
        let seq = posted(registry.post_notice(&id, &board, Some("note".into())));
        assert_eq!(board.take_pending_notices().len(), 1, "the lead read it");
        registry.report_settled(&id, Some(&board));
        assert!(!board.take_retracted(seq), "the cell records what it read");
    }

    #[test]
    fn a_report_retracts_only_its_own_agents_notice() {
        let (registry, id, board) = settled_with_notice();
        board.post_notice("a shell finished".into(), true);
        posted(registry.post_notice(&id, &board, Some("note".into())));
        registry.report_settled(&id, Some(&board));
        let left = board.take_pending_notices();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].context, "a shell finished");
    }

    #[test]
    fn a_settle_that_owed_no_notice_is_still_accounted_for() {
        // A foreground agent's settle posts nothing (its group carried the
        // answer); its resumed run's later settle must still be the one a
        // report retracts.
        let (registry, id, board) = settled_with_notice();
        assert_eq!(
            registry.post_notice(&id, &board, None),
            NoticePost::Posted(None)
        );
        assert!(matches!(
            registry.resume(&id, "again"),
            Resume::Continue { .. }
        ));
        registry.settle(
            &id,
            Ok("second".into()),
            vec![ChatMessage::user("p")],
            Some(StreamEvent::StreamDone),
        );
        let seq = posted(registry.post_notice(&id, &board, Some("note".into())));
        registry.report_settled(&id, Some(&board));
        assert!(board.take_pending_notices().is_empty());
        assert!(board.take_retracted(seq));
    }

    #[test]
    fn a_retraction_costs_only_the_notice_it_took_back() {
        // The loop holds a notice's cell until the lead reads it, so a
        // retracted notice is still held when the resumed agent's next one
        // is posted, both settling at the turn's end: the first must stay
        // taken back and the second keep its cell — which a mark kept per
        // agent could not tell apart.
        let (registry, id, board) = settled_with_notice();
        let first = posted(registry.post_notice(&id, &board, Some("first".into())));
        registry.report_settled(&id, Some(&board));
        assert!(matches!(
            registry.resume(&id, "again"),
            Resume::Continue { .. }
        ));
        registry.settle(
            &id,
            Ok("second".into()),
            vec![ChatMessage::user("p")],
            Some(StreamEvent::StreamDone),
        );
        let second = posted(registry.post_notice(&id, &board, Some("second".into())));
        assert!(board.take_retracted(first), "the first stays taken back");
        assert!(!board.take_retracted(second), "the second keeps its cell");
    }

    #[test]
    fn a_notice_a_wait_observed_is_never_retracted_or_suppressed() {
        // A held event released observed posts nothing; a report then has
        // nothing owed and nothing on the board.
        let registry = test_registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        assert!(registry.begin_wait(&id));
        registry.settle(
            &id,
            Ok("answer".into()),
            vec![ChatMessage::user("p")],
            Some(StreamEvent::StreamDone),
        );
        let _ = registry.end_wait(&id, true);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let board = crate::background::BackgroundRegistry::new(tx, std::env::temp_dir());
        registry.report_settled(&id, Some(&board));
        assert!(board.take_pending_notices().is_empty());
    }

    fn snapshot(state: AgentState, calls: &[&str]) -> AgentSnapshot {
        AgentSnapshot {
            id: "a7k2m9x4q".into(),
            description: "Fetch GitHub profile".into(),
            agent_type: GENERAL_PURPOSE.into(),
            state,
            runtime: Duration::from_secs(42),
            tool_uses: calls.len(),
            calls: calls.iter().map(|call| (*call).to_string()).collect(),
            follow_ups: Vec::new(),
        }
    }

    #[test]
    fn a_report_marks_where_each_follow_up_arrived() {
        let mut done = snapshot(
            AgentState::Done("alter-zero.".into()),
            &["Bash(curl one)", "Bash(curl two)"],
        );
        done.follow_ups = vec![FollowUp::lead(1), FollowUp::lead(2)];
        let report = output_report(&done);
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(
            lines[1..5],
            [
                "Bash(curl one)",
                AGENT_FOLLOW_UP_MARK,
                "Bash(curl two)",
                AGENT_FOLLOW_UP_MARK
            ],
            "a follow-up that started no calls still shows: {report}"
        );
        // One past the cap: the marks before the kept calls are counted with
        // them, never shown out of place.
        let calls: Vec<String> = (1..=AGENT_REPORT_CALLS_MAX + 1)
            .map(|n| format!("Read(file{n}.rs)"))
            .collect();
        let refs: Vec<&str> = calls.iter().map(String::as_str).collect();
        let mut long = snapshot(AgentState::Running, &refs);
        long.follow_ups = vec![
            FollowUp::lead(0),
            FollowUp::lead(AGENT_REPORT_CALLS_MAX + 1),
        ];
        let report = output_report(&long);
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(lines[1], "… 1 earlier tool call");
        assert_eq!(lines[2], "Read(file2.rs)");
        assert_eq!(lines.last().copied(), Some(AGENT_FOLLOW_UP_MARK));
        assert_eq!(
            lines
                .iter()
                .filter(|line| **line == AGENT_FOLLOW_UP_MARK)
                .count(),
            1
        );
    }

    #[test]
    fn report_call_lines_are_what_a_wait_streams() {
        // The waiting cell and the report show a call the same way: one line.
        assert_eq!(report_call("Bash(echo a\n  echo b)"), "Bash(echo a echo b)");
    }

    #[test]
    fn an_output_report_frames_the_state_over_the_calls() {
        // The user's picture: the tool calls as one-liners, no outputs.
        let report = output_report(&snapshot(
            AgentState::Running,
            &[
                "Bash(curl -s https://api.github.com/users/linuztx)",
                "Bash(curl -s https://api.github.com/users/linuztx/events/public)",
            ],
        ));
        assert_eq!(
            report,
            "Running (agent a7k2m9x4q · Fetch GitHub profile · 42s · 2 tool uses)\n\
             Bash(curl -s https://api.github.com/users/linuztx)\n\
             Bash(curl -s https://api.github.com/users/linuztx/events/public)"
        );
        let quiet = output_report(&snapshot(AgentState::Running, &[]));
        assert_eq!(
            quiet,
            "Running (agent a7k2m9x4q · Fetch GitHub profile · 42s · 0 tool uses)\n\
             No tool calls yet."
        );
    }

    #[test]
    fn a_settled_report_carries_its_answer() {
        let done = output_report(&snapshot(
            AgentState::Done("It has 12 repos.".into()),
            &["Bash(curl …)"],
        ));
        assert_eq!(
            done,
            "Done (agent a7k2m9x4q · Fetch GitHub profile · 42s · 1 tool use)\n\
             Bash(curl …)\n\
             \n\
             Final response:\n\
             It has 12 repos."
        );
        let failed = output_report(&snapshot(AgentState::Failed("rate limited".into()), &[]));
        assert!(failed.starts_with("Failed (agent a7k2m9x4q"), "{failed}");
        assert!(failed.ends_with("Error: rate limited"), "{failed}");
    }

    #[test]
    fn a_stopped_report_says_who_stopped_it_and_whether_it_resumes() {
        let by_lead = output_report(&snapshot(AgentState::Stopped { by_user: false }, &[]));
        assert!(by_lead.starts_with("Stopped (agent a7k2m9x4q"), "{by_lead}");
        assert!(by_lead.contains("agentsend resumes it"), "{by_lead}");
        let by_user = output_report(&snapshot(AgentState::Stopped { by_user: true }, &[]));
        assert!(by_user.contains("by the user"), "{by_user}");
        assert!(!by_user.contains("agentsend resumes"), "{by_user}");
    }

    #[test]
    fn a_long_history_keeps_the_newest_calls_and_counts_the_rest() {
        let calls: Vec<String> = (1..=AGENT_REPORT_CALLS_MAX + 5)
            .map(|n| format!("Read(file{n}.rs)"))
            .collect();
        let refs: Vec<&str> = calls.iter().map(String::as_str).collect();
        let report = output_report(&snapshot(AgentState::Running, &refs));
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(lines[1], "… 5 earlier tool calls");
        assert_eq!(lines[2], "Read(file6.rs)", "the newest are kept");
        assert_eq!(
            lines.last().copied(),
            Some(format!("Read(file{}.rs)", AGENT_REPORT_CALLS_MAX + 5).as_str())
        );
        assert_eq!(lines.len(), 2 + AGENT_REPORT_CALLS_MAX);
    }

    #[test]
    fn a_call_is_reported_on_one_line_cut_at_the_cap() {
        let long = format!("Bash(echo {}\nls)", "x".repeat(400));
        let report = output_report(&snapshot(AgentState::Running, &[long.as_str()]));
        let line = report.lines().nth(1).expect("the call");
        assert!(line.starts_with("Bash(echo xxx"), "{line}");
        assert!(line.ends_with('…'), "{line}");
        assert!(
            line.chars().count() <= AGENT_REPORT_CALL_CHARS + 1,
            "{line}"
        );
    }

    #[test]
    fn the_list_report_names_every_agent_and_its_state() {
        let mut stopped = snapshot(AgentState::Stopped { by_user: true }, &["Read(a)"]);
        stopped.id = "a9z8y7x6w".into();
        stopped.description = "Fix the tests".into();
        let mut done = snapshot(AgentState::Done("ok".into()), &["Read(a)", "Read(b)"]);
        done.id = "a1b2c3d4e".into();
        done.agent_type = "explore".into();
        done.description = "Search the repo".into();
        done.runtime = Duration::from_secs(65);
        let report = list_report(&[
            snapshot(AgentState::Running, &["Bash(curl …)"]),
            done,
            stopped,
        ]);
        assert_eq!(
            report,
            "3 agents:\n\
             - a7k2m9x4q (general-purpose) Fetch GitHub profile — running 42s · 1 tool use\n\
             - a1b2c3d4e (explore) Search the repo — done in 1m 5s · 2 tool uses\n\
             - a9z8y7x6w (general-purpose) Fix the tests — stopped by the user after 42s · 1 tool use"
        );
        assert_eq!(list_report(&[]), "No agents in this session.");
    }
}
