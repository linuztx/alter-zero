//! The conversation's value types: roles, messages, tool calls, the
//! status/summary of a turn, and the history items they are recorded into.
//!
//! Pure data with no behaviour beyond derives — the state machine that drives
//! them lives in [`App`](super::App) and its sibling modules.

use super::*;

/// Who authored a message — selects its bullet and colour when rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// A backend error notice (e.g. a real model failed mid-reply). Rendered like
    /// a message so it flows into scrollback and repaints on resize uniformly.
    Error,
    /// A system notice produced by a slash command (e.g. `/help`'s command list,
    /// or a stub's "not wired up yet" placeholder). Rendered like a message — a
    /// distinct bullet colour — so it flows into scrollback and repaints uniformly.
    System,
    /// A `!` shell command the user ran — the header line of its cell, rendered
    /// like a user message but with a `! ` bullet (`! pwd` on the dark
    /// user-style background). Its tool's `⎿` output sits **flush** below it
    /// (no blank spacer), forming the codex-style exec cell. Recorded by
    /// [`App::begin_shell`] when the command starts, so a mid-run resize
    /// repaints it. See `docs/shell-command.md`.
    Shell,
}

/// One finished message in the conversation.
///
/// Finished messages are echoed into the terminal's scrollback as they happen,
/// but a copy is kept here too so the conversation can be *repainted* after a
/// terminal resize (a width shrink makes ratatui clear the screen — see
/// `main.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// Wall-clock stamp of when this message was recorded. Displayed **only**
    /// for user messages — right-aligned below the message in the Ctrl+O
    /// transcript, never inline; other roles record it but don't show it. Empty
    /// when no clock is injected (the unit-test default); set from `App`'s
    /// clock at the I/O boundary. See `docs/timestamps.md`.
    pub timestamp: String,
    /// The temp-file paths of the Ctrl+V images attached to this message
    /// (user messages only — empty for every other role). The `[Image #N]`
    /// placeholders stay in `text`; the paths ride here so the conversation
    /// context can re-send the attachments to a vision backend on later turns
    /// and a `/resume` restores them. See `docs/image-paste.md`.
    pub images: Vec<PathBuf>,
}

/// The lifecycle of a tool call — selects its bullet colour when rendered:
/// waiting is dim, running is blue, success green, failure red.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    /// Queued in a parallel batch but not yet started — shown live as a dim
    /// `⎿ Waiting…` cell alongside the running call, until its own `ToolStart`
    /// flips it to [`Running`](ToolStatus::Running). Only a batched sibling is
    /// ever `Waiting`; a lone tool (the `!` shell, the dummy's single calls)
    /// starts `Running`. See `docs/parallel-tools.md`.
    Waiting,
    /// Executing — shown live (blue) in the bottom region while it runs.
    Running,
    /// Finished successfully (green).
    Ok,
    /// Finished with an error (red).
    Failed,
    /// Resolved by moving to the **background** (a `run_in_background` bash
    /// call, or Ctrl+B on a running command): the process keeps running under
    /// the [`App::background`] registry while the cell resolves with a green
    /// bullet and the fixed `⎿ Running in the background (↓ to manage)` row —
    /// the stored `output` is the model-facing text (task id + interim-output
    /// path), never displayed. See `docs/background.md`.
    Backgrounded,
}

/// One tool invocation: its `name`, a short `args` summary, its lifecycle
/// `status`, and the (possibly multi-line) `output` it produced.
///
/// While running, `output` is empty/partial and `status` is
/// [`ToolStatus::Running`]; once finished it is recorded in [`App::history`] so
/// it repaints on resize and is listed in full in the Ctrl+O tool-output view.
/// Inline it renders collapsed (a one-line peek); the full `output` is only shown
/// in that separate view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub name: String,
    pub args: String,
    pub status: ToolStatus,
    pub output: String,
    /// Wall-clock stamp of when the call finished (set in [`App::end_tool`]).
    /// Recorded but not currently displayed — only user-message stamps show.
    /// Empty while running and when no clock is injected. See
    /// `docs/timestamps.md`.
    pub timestamp: String,
    /// Whether this is a `!` shell command (set by [`App::begin_shell`]). A
    /// shell call renders **headerless** — just its `⎿` output lines, flush
    /// under the [`Role::Shell`] header message recorded with it — instead of
    /// the `● name(args)` bullet header. See `docs/shell-command.md`.
    pub shell: bool,
    /// Set when a `!` shell command's output **exceeded the in-memory cap** and
    /// was cut: `output` holds only the retained head, and the cell appends a
    /// dim `…` marker at the end of the expanded output (`ui::tool_full_lines`)
    /// to show more was dropped. `false` for output kept in full. See
    /// `docs/shell-command.md`.
    pub truncated: bool,
}

/// Which way the live token tally is moving, selecting the arrow glyph in the
/// status line: [`Down`] (`↓`) while the reply streams (output tokens), flipping
/// to [`Up`] (`↑`) right after a tool result (its output "uploaded" back). The
/// tally itself is cumulative and never resets when the arrow flips.
///
/// [`Down`]: TokenArrow::Down
/// [`Up`]: TokenArrow::Up
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenArrow {
    /// `↓` — output tokens, while the reply text streams.
    Down,
    /// `↑` — after a tool result is folded back in.
    Up,
}

/// The live status of the turn in flight, shown as a codex-style status line in
/// the strip above the input box (`● {verb}… ({elapsed}s · {arrow} {n} tokens ·
/// Thinking for {m}s)`). `Some` on [`App`] from [`App::begin_stream`] until the
/// turn ends; see `docs/status-indicator.md`.
///
/// `verb`/`done_verb`, `tokens`, and `arrow` are pure turn state. `elapsed` and
/// `thinking` are **written by the I/O boundary each frame**
/// ([`App::set_status_times`]) — time is impure, so it never reaches the pure
/// core except as these already-computed values (mirrors the timestamp clock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnStatus {
    /// The whimsical working verb shown live (e.g. `Working`), fixed for the turn.
    pub verb: &'static str,
    /// The matching done verb for the committed summary (e.g. `Done`).
    pub done_verb: &'static str,
    /// Cumulative token estimate for the whole turn (text **and** tool output);
    /// never reset mid-turn. Shown only when > 0.
    pub tokens: usize,
    /// Which arrow the tally shows (`↓` streaming / `↑` after a tool).
    pub arrow: TokenArrow,
    /// Time since the turn was submitted — set by the boundary each frame. The
    /// renderer derives both the displayed whole seconds and the verb's shimmer
    /// phase from it (sub-second resolution drives the wave).
    pub elapsed: Duration,
    /// How long the *current* thinking phase has run, or `None` when not thinking
    /// — set by the boundary each frame. `Some` renders the `Thinking for Ns`
    /// suffix; cleared the moment thinking ends.
    pub thinking: Option<Duration>,
    /// Whether this is a `!` shell-command turn ([`App::begin_shell`]). A shell
    /// turn ends **without** a `"{verb} for Ns"` summary — the committed cell
    /// is its own record ([`App::end_turn`] returns `None`). See
    /// `docs/shell-command.md`.
    pub shell: bool,
    /// Set while a failed request is being retried — the connection/send failed
    /// (or a transient status came back) before any content streamed, so the
    /// backend is reconnecting. Rendered as a `retrying {attempt}/{max}` clause
    /// in the status line and cleared the moment content arrives ([`App::push_chunk`]
    /// / [`App::push_thinking`]). Set by [`App::set_retry`] from the backend's
    /// [`crate::stream::StreamEvent::Retrying`]. See `docs/llm.md`.
    pub retry: Option<RetryInfo>,
}

/// A live retry indicator for the status line: the 1-based retry number and the
/// ceiling (`retrying {attempt}/{max}`). See [`TurnStatus::retry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryInfo {
    /// Which retry this is (1-based — the first retry is `1`).
    pub attempt: u32,
    /// The maximum number of retries ([`crate::llm::retry::MAX_RETRIES`]).
    pub max: u32,
}

/// A finished turn's summary, committed to scrollback as a dim `"{verb} for
/// {secs}s"` line and kept in [`App::history`] so it survives a resize and lists
/// in the Ctrl+O transcript (with a timestamp, like every other item).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSummary {
    /// The done verb chosen for the turn (e.g. `Done`, `Finished`).
    pub verb: &'static str,
    /// The turn's total wall-clock duration in whole seconds.
    pub secs: u64,
    /// Wall-clock stamp of when the turn finished. Recorded but not currently
    /// displayed — only user-message stamps show. See `docs/timestamps.md`.
    pub timestamp: String,
    /// How many background shells were still running when the turn ended —
    /// rendered as a `· {n} shells still running` suffix when non-zero
    /// (`Done for 22s · 3 shells still running`). Snapshotted at
    /// [`App::end_turn`]; not persisted to the session file (a resumed
    /// session's shells are gone). See `docs/background.md`.
    pub shells: usize,
    /// The turn's **real** billed tokens (input + output summed over the
    /// turn's request rounds), from the provider's usage frames
    /// ([`App::apply_usage`]) — rendered as a `· {n} tokens` suffix. `0` when
    /// the backend reported none (the dummy, a `!` shell), which hides the
    /// clause. Persisted with the summary. See `docs/prompt-caching.md`.
    pub tokens: usize,
    /// How many of those input tokens were served from the provider's prompt
    /// cache — the `({n} cached)` suffix beside `tokens` when non-zero, the
    /// visible proof caching is working. See `docs/prompt-caching.md`.
    pub cached: usize,
}

/// One ordered entry of finished conversation history: a [`Message`], a
/// [`ToolCall`], or a turn [`TurnSummary`]. They share a single ordered list so
/// the inline conversation repaints (after a resize, or when returning from the
/// tool-output view) in the exact order things streamed — assistant text, tool
/// calls, and the per-turn "Done" summary interleaved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryItem {
    Message(Message),
    Tool(ToolCall),
    Summary(TurnSummary),
    /// A background shell's completion notice (`docs/background.md`).
    Background(BackgroundNotice),
    /// A resolved subagent group — the `agent` tool's tree cell
    /// (`docs/agent-tool.md`).
    AgentGroup(AgentGroup),
    /// A background agent's completion notice (`docs/agent-tool.md`).
    AgentNotice(AgentNotice),
    /// A `/compact` marker (`docs/compact.md`): from here back, the model's
    /// context is the compacted shape — [`crate::context::context_messages`]
    /// derives the budgeted recent user texts + the summary bridge in place of
    /// the earlier items. Appended (never a rewrite), so the transcript, the
    /// recorder, and the checkpoint keys are untouched.
    Compaction(Compaction),
}

/// A `/compact` marker's payload: the model-written handoff summary the
/// context derivation bridges into every later request, codex's local
/// compaction (`docs/compact.md`). Renders as the `● Context compacted` cell
/// (the summary body expands in the Ctrl+O transcript only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    /// The compact turn's full streamed reply — the handoff summary (may be
    /// empty when the model streamed nothing; the derivation substitutes
    /// codex's "(no summary available)").
    pub summary: String,
    /// Wall-clock stamp (recorded like every item's; never displayed).
    pub timestamp: String,
    /// The context gauge when the compaction began (tokens) — the cell shows
    /// `· {before} → {after} tokens`. 0 = unknown (an old rollout), hiding
    /// the clause.
    pub before: u64,
    /// The re-estimated context size right after the compaction (tokens).
    pub after: u64,
    /// Whether this compaction was **auto-triggered** by the context gauge
    /// crossing the threshold (the cell appends `· auto`), vs the manual
    /// `/compact` command. See `docs/compact.md`.
    pub auto: bool,
}

/// The number of tokens in `text`, via the real `tiktoken` `o200k_base`
/// tokenizer ([`crate::tokenizer::count`]) — the single seam every status-line
/// tally funnels through (input, reply, reasoning, and tool output). Exact for
/// current OpenAI models and a close approximation for the other models the
/// configured providers serve; the running total still accumulates faithfully
/// as text and tool output arrive. See `docs/status-indicator.md`.
#[must_use]
pub(crate) fn count_tokens(text: &str) -> usize {
    crate::tokenizer::count(text)
}

/// A flat token estimate charged per Ctrl+V-attached image
/// ([`App::count_input_images`]). The protocol carries no real usage and an
/// image has no text to size, so this is a demo stand-in for a vision model's
/// per-image cost. See `docs/image-paste.md`.
pub(super) const IMAGE_INPUT_TOKENS: usize = 256;

/// What a backend error leaves behind, handed to the event loop to flush to
/// scrollback. The partial reply (if any), the tool that died mid-run (if one
/// was), and the error are also recorded in [`App::history`] so a later resize
/// repaints them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamError {
    /// The reply text streamed before the error, if any non-empty text arrived.
    pub partial: Option<String>,
    /// The tool that was mid-run when the backend died, now resolved as
    /// [`ToolStatus::Failed`] with [`ERROR_TOOL_OUTPUT`], if one was running —
    /// the stream contract allows `Error` in place of `StreamDone` at any
    /// point, `ToolEnd` still owed (the interrupt's [`InterruptedTurn::Kept`]
    /// twin).
    pub tool: Option<ToolCall>,
    /// The live agent group the error resolved (its foreground agents marked
    /// interrupted, the [`AgentGroup`] recorded in history) — the loop
    /// commits its tree cell like the interrupt path. `None` when no group
    /// was live. See `docs/agent-tool.md`.
    pub agents: Option<AgentGroup>,
    /// The error message to show the user.
    pub error: String,
}

/// What interrupting a turn leaves behind ([`App::interrupt_turn`]), handed to
/// the event loop to decide how to settle the screen.
///
/// Two outcomes, mirroring codex's "keep what streamed" versus this codebase's
/// "nothing streamed yet, so undo it" divergence (see `docs/interrupt.md`):
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterruptedTurn {
    /// The turn had produced **no output** (no partial reply, no tool) and
    /// nothing was queued behind it, so the whole submission is rolled back
    /// rather than interrupted: [`App::interrupt_turn`] has already pulled the
    /// turn's user message(s) back into the composer and dropped them from
    /// [`App::history`], recording **no** `Conversation interrupted` notice
    /// (there was nothing to keep). The loop repaints scrollback without the
    /// undone message. The user's "there's no output yet, move it back to the
    /// textarea" case.
    Undone,
    /// Some output had streamed — a partial reply and/or a running tool — so it
    /// is **kept** in the transcript (codex never retracts what streamed). Both
    /// are also recorded in [`App::history`] so a later resize repaints them.
    Kept {
        /// The reply text streamed before the interrupt, if any non-empty text
        /// arrived since the last flush.
        partial: Option<String>,
        /// The tool that was mid-run, now resolved as failed with
        /// [`INTERRUPT_TOOL_OUTPUT`], if one was running.
        tool: Option<ToolCall>,
        /// The red terminal notice to commit to scrollback — [`INTERRUPT_NOTICE`]
        /// for a normal turn, or **`None`** for a `!` shell turn, whose
        /// `⎿ Interrupted by user` cell already says it (so a second
        /// `Conversation interrupted` line would be redundant). Whatever this
        /// holds, [`App::interrupt_turn`] has already recorded it in
        /// [`App::history`] to match.
        notice: Option<&'static str>,
        /// The live agent group the interrupt resolved (its foreground agents
        /// marked interrupted, the [`AgentGroup`] recorded in history) — the
        /// loop commits its tree cell before the notice, and kills the
        /// group's subagents via the registry. `None` when no group was live.
        /// See `docs/agent-tool.md`.
        agents: Option<AgentGroup>,
    },
}

/// Which screen is showing. The conversation is the inline TUI; the tool-output
/// view is a separate full-screen overlay listing every tool call's full output
/// (Ctrl+O toggles between them). The conversation keeps streaming underneath
/// either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// The inline conversation (default).
    #[default]
    Conversation,
    /// The full-screen tool-output viewer.
    ToolOutput,
    /// The full-screen `/resume` session picker — the other alternate-screen
    /// overlay (codex's `resume_picker`). See `docs/resume.md`.
    ResumePicker,
    /// The full-screen Ctrl+D context-debug view: the raw LLM context window
    /// (system prompt + every derived context message, placeholders and
    /// bracketed tool formats unrendered). See `docs/context.md`.
    ContextDebug,
}

/// The active model's reasoning state: what the `/v1/models` record said it
/// supports and the mode the user has cycled to (Shift+Tab). Lives on
/// [`App::thinking`]; the footer shows `mode.label()` beside the model name
/// and the boundary threads the mode into each request. See
/// `docs/reasoning.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingState {
    /// The capability parsed from the provider's model record.
    pub support: ReasoningSupport,
    /// The currently selected mode (defaults to medium where offered).
    pub mode: ThinkingMode,
}

/// The session context shown in the footer under the input box: the backend's
/// model name and the (display-ready, home-relativized) working directory.
/// Plain strings — `main.rs` formats them at the I/O boundary
/// ([`App::set_session_info`]) so the pure core never reads the environment.
/// See `docs/footer.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// The model id the active [`crate::stream::ReplySource`] answers as.
    pub model: String,
    /// The working directory, already formatted for display (`~`-relative).
    pub cwd: String,
}

/// How a [`Toast`] is styled — a neutral confirmation or a failure. Drives the
/// row's colour in `ui.rs` (dim vs. red). See `docs/toast.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    /// A neutral confirmation or soft rejection (dim) — `Copied last message to
    /// clipboard`, `Switched model to …`, `/resume is disabled …`.
    Info,
    /// A failure (red) — `No agent response to copy`, `Copy failed: …`.
    Error,
}

/// A transient status line shown just above the input box that clears itself
/// after a few seconds — the ephemeral counterpart to a committed `Role::System`
/// / `Role::Error` message. Raised for confirmations and soft rejections the
/// user should see but never wants to keep (`/copy`, a model switch, a mid-turn
/// `/resume`), so nothing lands in [`App::history`]. The expiry is driven at the
/// I/O boundary via the timestamp pattern (`main.rs`'s `toast_deadline`); the
/// pure core only holds *what* it says. See `docs/toast.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    /// The message shown (truncated to the width by the renderer).
    pub text: String,
    /// Whether it reads as a neutral info line or a red failure.
    pub kind: ToastKind,
}
