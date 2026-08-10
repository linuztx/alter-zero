//! The reply protocol: what a backend sends the event loop.
//!
//! [`StreamEvent`] and its payload types are the whole wire between a
//! [`ReplySource`](super::ReplySource) and the app — plain data with no I/O, so
//! a real backend, the offline dummy and the tests all speak the same language.

use crate::ask::AskRequest;
use crate::permission::PermissionRequest;
use crate::tasks::TaskStore;

/// One call in a [`StreamEvent::ToolBatch`] announcement: the `name` + short
/// `args` summary the `● name(args)` header shows — the *same* two strings the
/// call's own [`StreamEvent::ToolStart`] carries, so a `⎿ Waiting…` cell's header
/// matches the header it shows once it starts running. Carries no output/status —
/// those arrive later via `ToolStart`/`ToolEnd`. See `docs/parallel-tools.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallSummary {
    pub name: String,
    pub args: String,
}

/// One subagent announced by a [`StreamEvent::AgentBatch`]: the identity the
/// roster entry ([`crate::agents::AgentRun`]) is created from. `id` is the
/// registry id its agent-channel events will carry; `background` mirrors the
/// batch's flag (every spec in one batch shares it). See `docs/agent-tool.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    pub id: String,
    pub description: String,
    pub agent_type: String,
    pub prompt: String,
    pub background: bool,
}

/// One `agent` tool call's resolution inside a [`StreamEvent::AgentGroupDone`]:
/// the model-facing tool-result text for the agent `id` — the framed final
/// response (foreground), the launch acknowledgement (background), or the
/// stopped/failed note. `ok` picks the recorded entry's red/green when the
/// roster can no longer say (it mirrors the subagent's own outcome).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCallDone {
    pub id: String,
    pub output: String,
    pub ok: bool,
}

/// Real token usage reported by the provider for one completed request round
/// — the final `usage` frame of an OpenAI-compatible stream (asked for via
/// `stream_options.include_usage`). Unlike the app-side tiktoken estimate it
/// counts **everything** the request billed — system prompt, full context,
/// reasoning — and carries the prompt-cache detail. The loop folds it into
/// the live tally via [`crate::app::App::apply_usage`]. See
/// `docs/prompt-caching.md`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    /// Prompt tokens billed for the round (`prompt_tokens`) — cached reads
    /// included, per the OpenAI accounting shape.
    pub input: u64,
    /// Completion tokens the round generated (`completion_tokens`).
    pub output: u64,
    /// Input tokens served from the provider's prompt cache (a subset of
    /// `input`): `prompt_tokens_details.cached_tokens`, or the
    /// `cache_read_input_tokens` alias Venice/Anthropic-style shims use.
    pub cached: u64,
    /// Input tokens written to the cache by this round (Anthropic-style
    /// explicit caching): `prompt_tokens_details.cache_write_tokens` /
    /// `cache_creation_input_tokens`.
    pub cache_write: u64,
    /// Output tokens the round spent **thinking** (a subset of `output`):
    /// `completion_tokens_details.reasoning_tokens`. The committed
    /// `Thought for …` cell snaps its tokenizer estimate to this
    /// ([`crate::app::App::apply_usage`]); 0 when the provider reports no such
    /// detail, which keeps the estimate. See `docs/thinking-stream.md`.
    pub reasoning: u64,
}

impl TokenUsage {
    /// The round's billed total — what the tally grows by.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.input + self.output
    }
}

/// What a backend sends to the event loop. Only the *reply* travels this channel
/// — keyboard input arrives separately via the terminal event stream (see
/// `main.rs`). It is a tokio unbounded channel so the async loop can `select!` on
/// it; the backend thread sends without ever touching the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// A piece of the reply (typically one word).
    Chunk(String),
    /// The model requested a **parallel batch** of tool calls this round,
    /// announced up front — *before* the first [`StreamEvent::ToolStart`] — so the
    /// UI can show every requested call at once, the ones not yet executing as
    /// dim `⎿ Waiting…` cells. Each [`ToolCallSummary`] is the `name`/`args` the
    /// `● name(args)` header shows (the same summary the matching `ToolStart`
    /// carries). Sequential execution then transitions the calls one at a time via
    /// [`StreamEvent::ToolStart`]/[`StreamEvent::ToolEnd`]. A backend that never
    /// batches (the `!` shell, the dummy's lone calls) simply omits this — a lone
    /// `ToolStart` still works. See `docs/parallel-tools.md`.
    ToolBatch(Vec<ToolCallSummary>),
    /// A tool call has started executing. The loop shows it live (blue) until the
    /// matching [`StreamEvent::ToolEnd`] arrives. `args` is a short summary for
    /// the `name(args)` header. When a [`StreamEvent::ToolBatch`] announced this
    /// call, this flips its `⎿ Waiting…` cell to `⎿ Running…`.
    ///
    /// `detail` is the call's **human-readable description** when the model
    /// supplied one (a `bash` call's `description` argument) — `None`
    /// otherwise. The main tool cells ignore it (their headers show the
    /// command, Claude-Code style); the agent roster's tree rows prefer it
    /// for their sticky `{Name}: {detail}` activity line
    /// (`docs/agent-tool.md`).
    ToolStart {
        name: String,
        args: String,
        detail: Option<String>,
    },
    /// The in-flight tool call finished with this `output` and outcome (`ok` →
    /// green, else red). Always follows a [`StreamEvent::ToolStart`].
    ///
    /// `truncated` is `true` when the output exceeded the in-memory cap and was
    /// cut — `output` then holds only the retained head, and the cell appends a
    /// `…` marker (see `docs/shell-command.md`). The `!` shell runner sets it; a
    /// normal backend tool always sends `false`.
    ToolEnd {
        output: String,
        ok: bool,
        truncated: bool,
    },
    /// The in-flight `AskUserQuestion` call resolved with the user's
    /// **submitted answers** (`docs/ask.md`) — sent **in place of**
    /// [`StreamEvent::ToolEnd`], the green twin of
    /// [`StreamEvent::ToolRejected`]: `display` is the committed cell's text
    /// (`User answered Claude's questions:` over the `· Q → A` rows) while
    /// `result` is the model-facing answers JSON the tool call returns. The
    /// loop keeps both on the recorded call
    /// ([`crate::app::App::answer_tool`]) so the derived context replays what
    /// the model actually read.
    ///
    /// `truncated` mirrors [`StreamEvent::ToolEnd`]'s: this event also carries
    /// an **executed** call's resolution when a `PostToolUse` hook amended it
    /// (`docs/hooks.md`), and such a call's output can have hit the byte cap,
    /// so the expanded cell still needs its `…` marker. It is `false` for the
    /// ask tool, which never truncates.
    ToolAnswered {
        display: String,
        result: String,
        truncated: bool,
    },
    /// The in-flight tool call was **refused at the permission prompt** (option
    /// 3, a Tab-amended rejection, or Ctrl+E's explain-instead) — sent **in
    /// place of** [`StreamEvent::ToolEnd`], since the tool never ran. The
    /// `AskUserQuestion` tool resolves through here too when the user
    /// declines (or picks `Chat about this`) — nothing was answered, and the
    /// model reads the stop-and-wait `result` (`docs/ask.md`).
    ///
    /// The two texts are deliberately different (`docs/permissions.md`):
    /// `display` is the short red cell output the user reads
    /// (`User rejected write to hello.py`, plus the amended instructions when
    /// Tab supplied them), while `result` is the longer stop-and-wait text the
    /// *model* receives as the tool result. The loop keeps both on the
    /// recorded call ([`crate::app::App::reject_tool`]) so
    /// [`crate::context::context_messages`] replays what was really sent — a
    /// later turn would otherwise see only the one-liner and lose the user's
    /// instructions entirely.
    ///
    /// `truncated` mirrors [`StreamEvent::ToolEnd`]'s, for the same reason as
    /// [`StreamEvent::ToolAnswered`]'s: a **failed** call a `PostToolUse` hook
    /// amended resolves through here (`docs/hooks.md`), and its output can
    /// have hit the byte cap. `false` for every refusal, where nothing ran.
    ToolRejected {
        display: String,
        result: String,
        truncated: bool,
    },
    /// A provenance note for the **in-flight** tool call — today only the
    /// auto mode classifier's `Allowed by auto mode classifier`
    /// (`docs/permissions.md`). Sent right after the call's
    /// [`StreamEvent::ToolStart`]; the loop stores it on the running call
    /// ([`crate::app::App::set_tool_note`]) so the resolved cell appends it
    /// as a dim `⎿` row — the transcript's record that no human approved
    /// the call. A backend with no classifier never sends it.
    ToolNote(String),
    /// The in-flight tool call resolved by **moving to the background**
    /// (a `run_in_background` bash call, or Ctrl+B on a running command) —
    /// sent **in place of** [`StreamEvent::ToolEnd`]. `id` is the registry
    /// task id; `output` is the model-facing launch text (the task id +
    /// interim-output path) that becomes the tool result — the cell instead
    /// renders the fixed `⎿ Running in the background (↓ to manage)` row
    /// ([`crate::app::ToolStatus::Backgrounded`]). The process itself reports
    /// through the separate background channel. See `docs/background.md`.
    ToolBackgrounded { id: String, output: String },
    /// A chunk of the **currently-running** tool's output, streamed live as it
    /// is produced (one or more complete lines, stdout+stderr merged in arrival
    /// order), between the call's [`StreamEvent::ToolStart`] and its
    /// [`StreamEvent::ToolEnd`]. The loop appends it to the front running call
    /// ([`crate::app::App::push_tool_output`]) so the live cell **tails** it —
    /// Claude-Code's running-command look (see `docs/tool-streaming.md`). The
    /// authoritative full output still arrives in `ToolEnd`, which overwrites
    /// the tailed partial, so a dropped chunk never corrupts the final cell.
    /// Only the real `bash` executor emits this today; a backend that never
    /// streams simply omits it.
    ToolOutput(String),
    /// One **task tool** call resolved (`taskcreate`/`taskget`/`tasklist`/
    /// `taskupdate` — `docs/task-tools.md`), carried whole in a single event
    /// **instead of** the `ToolBatch`/`ToolStart`/`ToolEnd` trio: a task call
    /// is instant, needs no permission, and streams no output, and — Claude
    /// Code's rule — it renders **no tool cell** anywhere inline. `name`/`args`
    /// are the display header (`TaskCreate` + its one-line summary) the Ctrl+O
    /// transcript shows, `arguments` the **raw JSON the model sent** (kept
    /// beside the summary so the derived context can replay the call as it
    /// was made — the summary is lossy by design, being a header), `output`
    /// the model-facing result text (`Task #1
    /// created successfully: …`), `ok` the outcome, and `tasks` the
    /// **post-call snapshot** the live checklist under the status line renders
    /// ([`crate::app::App::record_task_call`]). Emitted by
    /// `llm::agent::run_agent` in the model's call order; the dummy scripts it
    /// for the offline demo.
    TaskCall {
        name: String,
        args: String,
        arguments: String,
        output: String,
        ok: bool,
        tasks: TaskStore,
    },
    /// A lifecycle hook injected **conversation text** mid-turn
    /// (`docs/hooks.md`): a `Stop`/`SubagentStop` block's continuation
    /// feedback, or a `UserPromptSubmit`/`SessionStart` hook's additional
    /// context. `text` is verbatim what the model reads as a user-role
    /// message — the loop finalises the assistant run before it (invariant
    /// 4's flush-before-you-interleave) and records the cell-less
    /// [`crate::app::HistoryItem::HookNote`], so the Ctrl+O transcript shows
    /// why the turn kept going, the derived context replays it on every
    /// later turn, and a `/resume` restores it. `label` is the short
    /// transcript heading (`Stop hook`, `UserPromptSubmit hook`).
    HookNote { label: String, text: String },
    /// A `UserPromptSubmit` hook **blocked the prompt** (`docs/hooks.md`):
    /// the turn is over before the first request. The loop rolls the
    /// just-recorded user message back out of history (the recorder's
    /// shrink-rewrite erases it from the rollout — Claude Code's "erased
    /// from context"), purge-repaints, and commits a red notice carrying
    /// `reason` and the original prompt so the user sees exactly what was
    /// refused and why. Sent **instead of** [`StreamEvent::StreamDone`] —
    /// nothing else follows it.
    PromptBlocked { reason: String },
    /// The model launched a **group of subagents** this round (its `agent`
    /// tool calls), announced up front like a [`StreamEvent::ToolBatch`]: the
    /// loop seeds the roster (one [`crate::agents::AgentRun`] per spec) and
    /// shows the live group cell — `● Running {n} agents…` over the per-agent
    /// tree rows — while each agent's own stream reports on the dedicated
    /// agent channel. A `background` group resolves immediately (its
    /// [`StreamEvent::AgentGroupDone`] follows at once); a foreground group
    /// stays live until every agent finishes. See `docs/agent-tool.md`.
    AgentBatch {
        background: bool,
        agents: Vec<AgentSpec>,
    },
    /// The announced agent group resolved: every foreground agent finished
    /// (or the background launch was acknowledged, or Ctrl+B moved the rest
    /// to the background — `background` reports the *resolved* mode). Carries
    /// each call's model-facing tool-result text; the loop drains the agent
    /// channel first (the terminal roster events were enqueued before this
    /// was sent), then records the [`crate::app::AgentGroup`] history item
    /// and commits the group cell. See `docs/agent-tool.md`.
    AgentGroupDone {
        background: bool,
        agents: Vec<AgentCallDone>,
    },
    /// The model began a "thinking" (reasoning) phase. The loop shows
    /// `· Thinking for Ns` in the live status line until the matching
    /// [`StreamEvent::ThinkingEnd`] arrives.
    ThinkingStart,
    /// A piece of reasoning text streamed during the thinking phase (a real
    /// API's reasoning delta). Opaque — never rendered — but counted into the
    /// live token tally, so the count keeps ticking while the model thinks.
    /// Only sent between a [`StreamEvent::ThinkingStart`] and its
    /// [`StreamEvent::ThinkingEnd`].
    ThinkingChunk(String),
    /// The model's thinking phase ended. Always follows a
    /// [`StreamEvent::ThinkingStart`]; drops the `Thinking for Ns` suffix.
    ThinkingEnd,
    /// A fragment of a tool call the model is **generating** — the streamed
    /// `name`/`arguments` pieces of a `tool_calls` delta, before the tool runs.
    /// Opaque JSON, never rendered, but counted into the live token tally (like
    /// [`StreamEvent::ThinkingChunk`]) so the status keeps ticking while the
    /// model generates the call. Emitted by a real backend during a
    /// tool-calling round, ahead of the [`StreamEvent::ToolStart`] that begins
    /// executing it. See `docs/status-indicator.md`.
    ToolCallDelta(String),
    /// A failed request is being retried (the connection/send failed before any
    /// content streamed, or the server returned a transient status). Carries the
    /// 1-based retry number and the ceiling, shown live in the status line as
    /// `retrying {attempt}/{max}`. Only a real backend sends this (see
    /// `llm::retry`); the loop shows it in the status and keeps the turn alive.
    Retrying { attempt: u32, max: u32 },
    /// The provider's real token usage for one completed request round (the
    /// final usage frame of the stream — sent once per round by a real
    /// backend, so an agentic turn reports one per tool round). The loop
    /// snaps the live tally to it ([`crate::app::App::apply_usage`]), turning
    /// the app-side estimate into the provider's own accounting, prompt-cache
    /// detail included. The dummy never sends it. See `docs/prompt-caching.md`.
    Usage(TokenUsage),
    /// A `write`/`edit`/`bash` call is **waiting on the user's approval**
    /// (`docs/permissions.md`). Sent by the backend thread just before it
    /// blocks on the [`crate::permission::PermissionGate`]; the loop raises
    /// the inline prompt ([`crate::app::App::open_permission`]) and posts the
    /// answer back on the gate under this request's `id`. Nothing is running
    /// while it is up — the call's `ToolStart` follows only if the user says
    /// yes. A backend with no gate attached never sends it.
    Permission(PermissionRequest),
    /// An `AskUserQuestion` call is **waiting on the user's answers**
    /// (`docs/ask.md`). Sent by the backend thread just before it blocks on
    /// the [`crate::ask::AskGate`]; the loop raises the inline question modal
    /// ([`crate::app::App::open_ask`]) and posts the [`crate::ask::AskDecision`]
    /// back on the gate under this request's `id`. Unlike a permission
    /// request the call's `ToolStart` has already fired — the ask *is* the
    /// tool's execution. A backend with no ask gate attached never offers the
    /// tool, so it never sends this.
    AskUser(AskRequest),
    /// The backend failed; carries a human-readable message to show the user.
    Error(String),
    /// The reply is complete.
    StreamDone,
}
