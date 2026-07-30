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

use crate::app::{HistoryItem, Message, Role, ToolCall, ToolStatus};
use crate::llm::ChatMessage;
use crate::stream::{CancelToken, StreamEvent};

/// The default subagent type when the model omits `subagent_type`.
pub const GENERAL_PURPOSE: &str = "general-purpose";

/// How long a naturally finished agent lingers in the footer roster with its
/// coloured `◯` before the boundary sweeps it (`main.rs`, the toast-deadline
/// pattern). A user `x` removes the entry immediately instead.
pub const AGENT_LINGER: Duration = Duration::from_secs(5);

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
    /// Set by the user's `x` — the footer roster hides the row at once
    /// (while the entry's data stays for its group's resolution).
    pub hidden: bool,
    /// The **sticky** activity line — `{Name}: {detail}` from the newest
    /// [`StreamEvent::ToolStart`] (a `bash` call's model-supplied
    /// `description`, else its args summary). Kept until the *next* tool
    /// starts, so the tree row shows what the agent is doing (or just did)
    /// rather than dropping to a generic `Working…` between calls
    /// (`docs/agent-tool.md`).
    pub last_activity: Option<String>,
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
            last_activity: None,
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
            }
            // Opaque progress — counted so the footer tally ticks while the
            // agent thinks / generates a call (the status-line pattern).
            StreamEvent::ThinkingChunk(text) | StreamEvent::ToolCallDelta(text) => {
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
                    });
                }
            }
            StreamEvent::ToolStart { name, args, detail } => {
                self.flush_segment();
                self.tool_uses += 1;
                // The sticky tree-row activity: the model's own description
                // when it gave one (`Bash: Fetching current weather…`), else
                // the args summary (`Write: game.py`).
                self.last_activity = Some(format!(
                    "{name}: {}",
                    detail.as_deref().unwrap_or(args.as_str())
                ));
                match self.tool_queue.front_mut() {
                    Some(front) if front.status == ToolStatus::Waiting => {
                        front.status = ToolStatus::Running;
                        front.name.clone_from(name);
                        front.args.clone_from(args);
                    }
                    _ => self.tool_queue.push_front(ToolCall {
                        name: name.clone(),
                        args: args.clone(),
                        status: ToolStatus::Running,
                        output: String::new(),
                        timestamp: String::new(),
                        shell: false,
                        truncated: false,
                    }),
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
            // A subagent never backgrounds a bash call (its executor has no
            // registry), but stay total: resolve the front call like an end.
            StreamEvent::ToolBackgrounded { output, .. } => {
                if let Some(mut front) = self.tool_queue.pop_front() {
                    front.status = ToolStatus::Backgrounded;
                    front.output = output.clone();
                    self.history.push(HistoryItem::Tool(front));
                }
            }
            StreamEvent::Usage(usage) => {
                self.usage_tokens += usage.total();
                self.tokens = self.usage_tokens;
            }
            StreamEvent::StreamDone => {
                self.result = self.flush_segment();
                self.tool_queue.clear();
                self.status = AgentStatus::Done;
                return true;
            }
            StreamEvent::Error(message) => {
                self.flush_segment();
                self.resolve_tools(message);
                self.error = Some(message.clone());
                self.status = AgentStatus::Failed;
                return true;
            }
            // A permission request is the *user's* business, not the roster's:
            // the boundary lifts it out of the agent channel and raises the
            // shared inline prompt (docs/permissions.md). Nothing about this
            // agent's own state changes while it waits.
            StreamEvent::Permission(_)
            | StreamEvent::ThinkingStart
            | StreamEvent::ThinkingEnd
            | StreamEvent::Retrying { .. }
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
        self.flush_segment();
        self.resolve_tools(crate::app::INTERRUPT_TOOL_OUTPUT);
        self.status = AgentStatus::Interrupted;
        true
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
    /// agent so new events fold in again.
    pub fn reopen(&mut self) {
        self.status = AgentStatus::Running;
        self.result = None;
        self.error = None;
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
                cancel: cancel.clone(),
                busy: true,
                agent_type: agent_type.to_string(),
                ..AgentSlot::default()
            },
        );
        (id, cancel)
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
        }));
        assert_eq!(run.tokens, 1000, "snapped to the billed total");
        run.apply(&StreamEvent::Usage(TokenUsage {
            input: 400,
            output: 100,
            cached: 0,
            cache_write: 0,
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
        // The transcript kept the whole exchange in order.
        let roles: Vec<&str> = run
            .history
            .iter()
            .map(|item| match item {
                HistoryItem::Message(m) if m.role == Role::User => "user",
                HistoryItem::Message(_) => "assistant",
                _ => "other",
            })
            .collect();
        assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
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
}
