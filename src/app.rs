//! Conversation state and the pure update logic that drives it.
//!
//! Everything here is free of I/O so it can be unit-tested directly: the event
//! loop in `main.rs` feeds key presses in and reacts to the returned
//! [`Action`]s, and pushes streamed chunks in via [`App::push_chunk`].

use std::collections::{HashSet, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::file_search::{FileMatch, at_token};
use crate::llm::{ModelEntry, ReasoningSupport, ThinkingMode};
use crate::session::SessionSummary;
use crate::stream::ToolCallSummary;
use crate::textarea::TextArea;

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

/// One background shell's completion, committed to history as a one-line
/// notice cell (`● Background command "{description}" completed (exit code
/// 0)` — green bullet on success, red on failure or a user stop). The
/// `output_tail` rides the item for the model's context
/// ([`crate::context::context_messages`]) but is never rendered — the model
/// summarises it in the automatic follow-up turn. See `docs/background.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundNotice {
    /// The human description shown in the headline: the model-supplied
    /// `description` argument, falling back to the command line.
    pub description: String,
    /// The registry task id (`bvyo7tkbe`, …) — lets the model pair the notice
    /// to the launch result it received.
    pub id: String,
    /// The exit code, or `None` when the process died to a signal.
    pub code: Option<i32>,
    /// Whether the user stopped it (the manager's `x` / a `/clear` sweep).
    pub killed: bool,
    /// The last lines of output at exit — context-only, never rendered.
    pub output_tail: String,
    /// Wall-clock stamp of when the completion was recorded. Recorded but not
    /// displayed, like tool stamps. See `docs/timestamps.md`.
    pub timestamp: String,
}

impl BackgroundNotice {
    /// Did the command succeed (exit code 0, not stopped by the user)? Picks
    /// the notice bullet colour: green for success, red otherwise.
    #[must_use]
    pub fn ok(&self) -> bool {
        !self.killed && self.code == Some(0)
    }

    /// The rendered one-liner: `Background command "{description}" {outcome}`.
    #[must_use]
    pub fn headline(&self) -> String {
        let outcome = self.outcome_phrase();
        format!("Background command \"{}\" {outcome}", self.description)
    }

    /// The outcome clause of the headline / context note.
    #[must_use]
    pub fn outcome_phrase(&self) -> String {
        if self.killed {
            return "was stopped by the user".to_string();
        }
        match self.code {
            Some(0) => "completed (exit code 0)".to_string(),
            Some(code) => format!("failed (exit code {code})"),
            None => "was terminated by a signal".to_string(),
        }
    }

    /// The model-facing context note: the headline plus the output tail (the
    /// bracketed user-role form `context::context_messages` sends). Also the
    /// automatic follow-up turn's prompt text.
    #[must_use]
    pub fn context_text(&self) -> String {
        let tail = if self.output_tail.trim().is_empty() {
            "(no output)"
        } else {
            self.output_tail.trim_end_matches('\n')
        };
        format!(
            "[background] Background command \"{}\" (id {}) {}.\nFinal output (tail):\n{tail}",
            self.description,
            self.id,
            self.outcome_phrase(),
        )
    }
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

/// The whimsical working verbs, one chosen per turn (by [`App::turn_count`]) for
/// the live status line. Cycled deterministically so the demo varies yet stays
/// testable — no RNG (mirrors how [`dummy_response`] picks a reply).
pub const WORKING_VERBS: &[&str] = &[
    "Working",
    "Generating",
    "Pondering",
    "Cooking",
    "Brewing",
    "Crunching",
    "Conjuring",
    "Churning",
    "Computing",
    "Synthesizing",
];

/// The done verbs, one chosen per turn for the committed `"{verb} for Ns"` summary.
pub const DONE_VERBS: &[&str] = &["Done", "Finished", "Completed", "Wrapped up", "Ready"];

/// The live-status verb for a `!` shell command (fixed, not cycled like the AI
/// [`WORKING_VERBS`]): the status line reads `Running…`. It doubles as the
/// status's (never rendered) done verb — a shell turn ends **without** a
/// summary, [`App::end_turn`] returning `None` for it. See
/// `docs/shell-command.md`.
pub const SHELL_VERB: &str = "Running";

/// The notice shown when Enter is pressed on a bare `!` (no command) — codex's
/// `Prefix a command with ! to run it locally`.
pub const SHELL_EMPTY_NOTICE: &str = "Type a command after ! to run it locally (e.g. !ls)";

/// The number of tokens in `text`, via the real `tiktoken` `o200k_base`
/// tokenizer ([`crate::tokenizer::count`]) — the single seam every status-line
/// tally funnels through (input, reply, reasoning, and tool output). Exact for
/// current OpenAI models and a close approximation for the other models the
/// configured providers serve; the running total still accumulates faithfully
/// as text and tool output arrive. See `docs/status-indicator.md`.
#[must_use]
fn count_tokens(text: &str) -> usize {
    crate::tokenizer::count(text)
}

/// A flat token estimate charged per Ctrl+V-attached image
/// ([`App::count_input_images`]). The protocol carries no real usage and an
/// image has no text to size, so this is a demo stand-in for a vision model's
/// per-image cost. See `docs/image-paste.md`.
const IMAGE_INPUT_TOKENS: usize = 256;

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
    /// The error message to show the user.
    pub error: String,
}

/// The notice committed when the user interrupts a turn (Esc mid-generation) —
/// codex's wording, minus its `/feedback` plug. Recorded as a [`Role::Error`]
/// message: like a backend failure, the notice is the turn's terminal state
/// (no `Done for Ns` summary). See `docs/interrupt.md`.
pub const INTERRUPT_NOTICE: &str =
    "Conversation interrupted - tell the model what to do differently.";

/// The notice committed when `/copy` writes the last response to the clipboard —
/// codex's info event, verbatim. Recorded as a [`Role::System`] message. See
/// `docs/copy.md`.
pub const COPY_OK_NOTICE: &str = "Copied last message to clipboard";

/// The notice committed when `/copy` finds no assistant response to copy —
/// codex's error event, verbatim. Recorded as a [`Role::Error`] message. See
/// `docs/copy.md`.
pub const COPY_EMPTY_NOTICE: &str = "No agent response to copy";

/// The transient toast shown when `/resume` is run while a turn is active —
/// codex blocks the command mid-task (`slash_command_blocked_by_active_task`)
/// instead of racing the stream (it would swap the whole conversation). Shown as
/// an [`Action::Toast`] rather than a committed message — a soft rejection the
/// user needn't keep. See `docs/resume.md` / `docs/toast.md`.
pub const RESUME_BUSY_NOTICE: &str = "/resume is disabled while a task is in progress";

/// The transient toast shown when `/help` is run while a turn is active — its
/// multi-line command list would interleave with the streaming reply in
/// scrollback, so mid-turn it is rejected like `/resume` (idle it still commits
/// the full list). Shown as an [`Action::Toast`]. See `docs/toast.md`.
pub const HELP_BUSY_NOTICE: &str = "/help is disabled while a task is in progress";

/// Codex's `/init` prompt (`prompts/init.md`, its `prompt_for_init_command.md`
/// verbatim): generate an `AGENTS.md` contributor guide — never overwriting an
/// existing one. Submitted as a **regular user turn** ([`Action::Submit`]), so
/// the model's agentic tool loop does the exploring and writing; the prompt is
/// the whole feature. See `docs/init.md`.
pub const INIT_PROMPT: &str = include_str!("../prompts/init.md");

/// The transient toast shown when `/init` is run while a turn is active —
/// codex disables it during a task (`available_during_task`); submitting would
/// race the running stream with a second turn. The `/compact` toast pattern.
/// See `docs/init.md` / `docs/toast.md`.
pub const INIT_BUSY_NOTICE: &str = "/init is disabled while a task is in progress";

/// The transient toast shown when `/compact` is run while a turn is active —
/// codex disables it during a task (`available_during_task`); ours rejects with
/// the `/help`/`/resume` toast pattern. See `docs/compact.md` / `docs/toast.md`.
pub const COMPACT_BUSY_NOTICE: &str = "/compact is disabled while a task is in progress";

/// The transient toast shown when `/compact` finds nothing to summarize — an
/// empty conversation would send the bare summarization prompt to the model
/// and "summarize" nothing. See `docs/compact.md`.
pub const COMPACT_EMPTY_NOTICE: &str = "Nothing to compact";

/// The fixed live-status verb for a `/compact` turn (the [`SHELL_VERB`]
/// pattern — not cycled): the status line reads `Compacting…`. It doubles as
/// the never-rendered done verb — a compact turn ends without a summary, the
/// `● Context compacted` cell being its record. See `docs/compact.md`.
pub const COMPACT_VERB: &str = "Compacting";

/// The auto-compact trigger's numerator/denominator: codex's threshold is
/// **90% of the context window** (`(context_window * 9) / 10`,
/// `ModelInfo::auto_compact_token_limit`). Past it, the loop starts a compact
/// turn on its own at the next idle boundary. See `docs/compact.md`.
const AUTO_COMPACT_NUMERATOR: u64 = 9;
const AUTO_COMPACT_DENOMINATOR: u64 = 10;

/// The transient toast shown when a `/resume` restored the working directory to
/// the session's checkpoint (`docs/checkpoint.md`) — feedback that the code, not
/// just the transcript, was rewound.
pub const CHECKPOINT_RESTORED_NOTICE: &str = "Restored files to this session's checkpoint";

/// The transient toast shown when an Esc-Esc backtrack reset the working
/// directory to the rewound-to message's checkpoint (`docs/checkpoint.md`).
pub const CHECKPOINT_REWOUND_NOTICE: &str = "Reset files to the checkpoint for that message";

/// The output recorded on a tool that was still running when the user
/// interrupted: it resolves as [`ToolStatus::Failed`] with this explanation
/// (codex: an aborted tool "may have partially executed").
pub const INTERRUPT_TOOL_OUTPUT: &str = "Interrupted by user";

/// The output recorded on a tool that was still running when the backend
/// reported a [`crate::stream::StreamEvent::Error`] — the turn died before the
/// tool's `ToolEnd` could arrive, so it resolves as [`ToolStatus::Failed`]
/// with this explanation ([`INTERRUPT_TOOL_OUTPUT`]'s error-path twin).
pub const ERROR_TOOL_OUTPUT: &str = "Interrupted by a backend error";

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
    },
}

/// The result of handling a key press, interpreted by the event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing to do.
    None,
    /// The user submitted a (non-empty) message; start a reply for it. Any
    /// Ctrl+V-attached images travel separately — the loop drains
    /// [`App::take_submission_images`] for their paths. See `docs/image-paste.md`.
    Submit(String),
    /// The user pressed Ctrl+V (or Ctrl+Alt+V) to paste an image. The decision is
    /// pure; the loop does the clipboard I/O ([`crate::clipboard`]) and calls
    /// [`App::attach_image`] on success (or commits a red notice on failure). See
    /// `docs/image-paste.md`.
    PasteImage,
    /// The user toggled the tool-output view (Ctrl+O, or Esc to leave it). The
    /// loop syncs the full-screen overlay to the now-updated [`App::view`].
    ToggleToolView,
    /// Enter confirmed an Esc-Esc backtrack: [`App::history`] is already
    /// truncated to the chosen message and the composer prefilled. The loop
    /// resets the code to that point's checkpoint (`docs/checkpoint.md`), then
    /// leaves the overlay and repaints — the same return path as
    /// [`Action::ToggleToolView`]'s exit branch. Distinguished from a plain
    /// overlay toggle so the loop knows a rewind (not just a view change)
    /// happened. See `docs/backtrack.md`.
    ConfirmBacktrack,
    /// The user toggled the Ctrl+D context-debug view (or closed it with
    /// q/Esc). The loop syncs the overlay to the now-updated [`App::view`],
    /// exactly like [`Action::ToggleToolView`]. See `docs/context.md`.
    ToggleContextDebug,
    /// A slash command produced a one-off system notice (e.g. `/help`'s command
    /// list, or a stub's placeholder). The loop records it as a [`Role::System`]
    /// message and commits it to scrollback, like a normal message.
    Notice(String),
    /// A slash command cleared the conversation (`/clear`). [`App::history`] is
    /// already empty; the loop repaints the now-blank inline view.
    Clear,
    /// `/copy` — write the last assistant response to the system clipboard.
    /// `Some(text)` is the text to copy ([`App::last_assistant_text`]); `None`
    /// means there was no response to copy. The decision is pure; the loop does
    /// the clipboard I/O ([`crate::clipboard::copy_to_clipboard`]) and commits
    /// the success/empty/failure notice — codex's `/copy`. See `docs/copy.md`.
    Copy(Option<String>),
    /// The user pressed Enter on a `!`-prefixed line from an idle composer: run
    /// the carried command (the text after the `!`, trimmed) locally. The loop
    /// echoes `❯ !command`, calls [`App::begin_shell`], and spawns it. See
    /// `docs/shell-command.md`.
    RunShell(String),
    /// The user pressed Esc while a turn was in flight: stop the generation
    /// (cancel + reap the backend, then [`App::interrupt_turn`]) — codex-style.
    Interrupt,
    /// `/compact` from an idle composer with a non-empty context: run the
    /// summarization turn. The loop calls [`App::begin_compact`], derives the
    /// context, pushes codex's summarization prompt as its final user entry,
    /// and spawns the request on a one-off **tools-free** backend — the
    /// summary streams into the compact buffer (never rendered) and
    /// [`App::finish_compact`] appends the marker at `StreamDone`. See
    /// `docs/compact.md`.
    Compact,
    /// `/resume` from an idle composer: open the session picker. The *loop*
    /// scans the sessions dir (the fs I/O stays at the boundary) and hands the
    /// result to [`App::open_resume_picker`]. See `docs/resume.md`.
    OpenResumePicker,
    /// The `/resume` picker was dismissed (Esc on an empty query, or Ctrl+C —
    /// codex's from-a-session picker closes rather than quits): [`App::view`]
    /// is already back on the conversation; the loop leaves the alternate
    /// screen and repaints, the Ctrl+O return.
    CloseResumePicker,
    /// Enter in the `/resume` picker: swap the conversation to the rollout
    /// file at this path. The loop reads + parses it (the I/O), calls
    /// [`App::load_session`], and adopts the file for further recording —
    /// or commits a red notice if the read fails, the current conversation
    /// unharmed (codex). See `docs/resume.md`.
    ResumeSession(PathBuf),
    /// Raise a transient info [`Toast`](crate::app::Toast) above the box — a
    /// confirmation or soft rejection that self-clears (e.g. `/resume` or `/help`
    /// run while a turn is active). The loop calls [`App::show_toast`] and arms
    /// the boundary's expiry timer; nothing is committed to scrollback. Error
    /// toasts (a `/copy` failure, a bad model switch) are raised by the boundary
    /// directly, so this carries only the info text. See `docs/toast.md`.
    Toast(String),
    /// `/model` from an idle composer: open the inline model picker. The *loop*
    /// spawns a worker to fetch the provider's model list (the HTTP stays at the
    /// boundary) and feeds it back via [`App::set_models`]. Unlike `/resume`,
    /// the picker is **inline** (it grows the bottom region in place), not an
    /// alternate-screen overlay. See `docs/llm.md`.
    OpenModelPicker,
    /// The inline model picker was dismissed (Esc on an empty query, or Ctrl+C):
    /// [`App::model_picker`] is already cleared; the loop just repaints the
    /// collapsed region.
    CloseModelPicker,
    /// Enter in the model picker: switch the active backend to this
    /// provider/model. The loop rebuilds the backend, updates the footer
    /// ([`App::set_session_info`]), and collapses the picker. See `docs/llm.md`.
    /// `reasoning` is the picked entry's parsed thinking capability (from the
    /// same `/v1/models` fetch that listed it), so a successful switch seeds
    /// the Shift+Tab cycle without refetching — `None` for a model with no
    /// reasoning. See `docs/reasoning.md`. `vision` is the entry's parsed
    /// image-input support, gating attachments on the rebuilt backend —
    /// `None` when the record didn't say. See `docs/tools.md`.
    SelectModel {
        provider: String,
        id: String,
        reasoning: Option<ReasoningSupport>,
        vision: Option<bool>,
        /// The picked entry's context window (`/v1/models` `context_length`),
        /// seeding the footer gauge + auto-compact without a refetch — `None`
        /// when the record didn't report one. See `docs/compact.md`.
        context: Option<u64>,
    },
    /// Shift+Tab cycled the thinking mode ([`App::thinking`] already advanced
    /// to the carried mode). The loop rebinds the *next* turn's backend to it,
    /// persists the choice, and presents the `Thinking: {mode}` toast (arming
    /// its expiry — why this isn't a direct `show_toast`). See
    /// `docs/reasoning.md`.
    SetThinking(ThinkingMode),
    /// `/login` from an idle composer: open the inline API-key onboarding flow.
    /// The *loop* builds the provider choices (which need boundary key
    /// resolution to mark the already-configured ones) and hands them to
    /// [`App::open_key_onboarding`]. See `docs/llm.md`.
    OpenKeyOnboarding,
    /// The onboarding flow was dismissed (Esc/Ctrl+C): [`App::key_onboarding`]
    /// is already cleared; the loop just repaints the collapsed region.
    CloseKeyOnboarding,
    /// Enter on the key-entry step: persist `key` to `env_var` in the `.env`
    /// file. The loop writes the file, updates its in-memory secrets, and
    /// commits a system notice. See `docs/llm.md`.
    SaveApiKey {
        /// The provider id the key belongs to (for the confirmation notice).
        provider: String,
        /// The environment variable to store it under (e.g. `OPENROUTER_API_KEY`).
        env_var: String,
        /// The API key the user entered.
        key: String,
    },
    /// `x` on a shell in the ↓ background manager: stop the background task
    /// with this registry id. The decision is pure; the loop kills the
    /// process group via the `background::BackgroundRegistry`, and the
    /// resulting `Exited` event removes the row / commits the stopped notice.
    /// See `docs/background.md`.
    KillBackground(String),
    /// Ctrl+B while a command is running (a model `bash` call or a `!` shell
    /// turn): move it to the background. The loop raises the registry's
    /// background request; the runner's poll loop consumes it, hands the
    /// child off, and resolves the cell as
    /// [`ToolStatus::Backgrounded`]. See `docs/background.md`.
    MoveToBackground,
    /// The user asked to quit.
    Quit,
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

/// The Esc-Esc backtrack gesture's state — codex's `BacktrackState`
/// (`tui/src/app_backtrack.rs`): Esc from an idle, empty composer *primes* the
/// gesture, a second Esc previews previous user messages highlighted in the
/// transcript overlay, Esc/← / → step the highlight, and Enter rewinds the
/// conversation to the highlighted message and puts its text back in the
/// composer to edit. See `docs/backtrack.md`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Backtrack {
    /// The first Esc armed the gesture (codex's `primed` — no timeout); any
    /// non-Esc key disarms it. While armed the footer slot shows the
    /// `esc again to edit previous message` hint.
    pub primed: bool,
    /// `Some(i)` while the overlay preview is active: the highlighted user
    /// message, as an index into the conversation's user messages (oldest =
    /// 0 — codex's `nth_user_message`). `None` when not previewing.
    pub selected: Option<usize>,
    /// Set when the preview opens or the highlight moves; the next overlay
    /// draw scrolls the highlight into view and consumes it
    /// ([`App::take_backtrack_scroll`] — codex's `scroll_chunk_into_view`),
    /// so it never fights the user's own scrolling afterwards.
    pub scroll_pending: bool,
}

/// One queued turn awaiting its slot while a turn streams — codex's
/// action-tagged queued message (`QueuedInputAction`). The queue is a
/// `VecDeque<QueuedTurn>` drained FIFO, one entry per turn-end; the variant is
/// the dispatch discriminator, so a text batch goes to the model while a `!`
/// command runs locally. See `docs/queue.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueuedTurn {
    /// One or more Enter-batched text messages — sent to the backend as a
    /// single turn (newline-joined). Consecutive Enters append to the last such
    /// batch; **Tab** opens a new one (a separate follow-up turn). The Ctrl+V
    /// images attached to the queued drafts ride along as their
    /// `(placeholder, path)` pairs, in attach order: the paths travel the typed
    /// image channel when the batch dispatches, and an Alt+Up pull-back
    /// re-attaches them to the composer. See `docs/image-paste.md`.
    Messages {
        /// The batch's messages, oldest first.
        texts: Vec<String>,
        /// The attachments of those messages, oldest first.
        images: Vec<(String, PathBuf)>,
    },
    /// A standalone `!` shell command, run locally as its own turn (via
    /// [`App::begin_shell`]). **Never merged** with a neighbouring entry — the
    /// next Enter-text starts a fresh [`Messages`] batch — matching codex's
    /// per-completion shell dispatch (`submit_queued_shell_prompt`).
    ///
    /// [`Messages`]: QueuedTurn::Messages
    Shell(String),
}

/// One **running** background shell, as the pure state sees it (the process
/// itself lives in the boundary's `background::BackgroundRegistry`; its
/// events — start, output lines, exit — are applied here). An exited shell
/// leaves the list ([`App::bg_exited`]): the completion notice is the record.
/// See `docs/background.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundShell {
    /// The registry task id (`bvyo7tkbe`, …).
    pub id: String,
    /// The command line, shown in the ↓ manager's list and details view.
    pub command: String,
    /// The model-supplied description (notices fall back to the command).
    pub description: Option<String>,
    /// Whether the model launched it — its completion then auto-starts a
    /// follow-up turn; a user-launched (`!` + Ctrl+B) shell only commits the
    /// notice.
    pub from_model: bool,
    /// The live output **tail** (capped at [`BG_TAIL_MAX_BYTES`], trimmed to
    /// line boundaries) — what the details view's output box tails and the
    /// completion notice snapshots. The full output is teed to the task's
    /// interim-output file at the boundary.
    pub output: String,
    /// How long the shell has been running — boundary-injected before each
    /// draw ([`App::set_background_runtime`], the `set_status_times` pattern).
    pub runtime: Duration,
}

/// The retained size of a background shell's in-memory output tail. Trimmed
/// from the **front** on line boundaries, so the details view / completion
/// notice always see the newest lines.
const BG_TAIL_MAX_BYTES: usize = 16 * 1024;

/// How much of a finished shell's tail rides its completion notice into the
/// model's context — enough to summarise from without bloating every later
/// turn (the full output is still in the task's interim-output file).
const BG_NOTICE_TAIL_MAX_BYTES: usize = 4 * 1024;
/// …and at most this many lines of it.
const BG_NOTICE_TAIL_MAX_LINES: usize = 30;

/// What a background shell left behind when it exited ([`App::bg_exited`]) —
/// held in [`App::pending_bg`] and settled at the next **safe boundary**
/// (a tool resolution / segment flush mid-turn, else the turn end; at once
/// while idle): the loop records a [`BackgroundNotice`] for it, its
/// [`context_text`](BgCompletion::context_text) having already been posted
/// onto the registry's notice board for the in-flight agent. See
/// `docs/background.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgCompletion {
    pub id: String,
    pub command: String,
    pub description: Option<String>,
    pub from_model: bool,
    /// Exit code, or `None` when the process died to a signal.
    pub code: Option<i32>,
    /// Whether the user stopped it (`x` in the manager, or a kill sweep).
    pub killed: bool,
    /// The output tail at exit (already capped for the notice).
    pub output_tail: String,
}

impl BgCompletion {
    /// The notice's display description: the model's `description` argument,
    /// falling back to the command line.
    #[must_use]
    pub fn display_description(&self) -> &str {
        self.description.as_deref().unwrap_or(&self.command)
    }

    /// The model-facing context note — byte-identical to the
    /// [`BackgroundNotice::context_text`] the settle later records for this
    /// completion, so the note the in-flight agent injects mid-turn and the
    /// one every later turn's derived context replays never diverge (the
    /// timestamp, stamped only at settle, plays no part in the text).
    #[must_use]
    pub fn context_text(&self) -> String {
        BackgroundNotice {
            description: self.display_description().to_string(),
            id: self.id.clone(),
            code: self.code,
            killed: self.killed,
            output_tail: self.output_tail.clone(),
            timestamp: String::new(),
        }
        .context_text()
    }
}

/// Which page of the ↓ background manager band is showing. The band is
/// **inline** (it replaces the composer, exactly like the `/model` picker —
/// never an alternate-screen [`View`]) and owns every key while open. See
/// `docs/background.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundView {
    /// The shell list (`Background` / `{n} active shells` / selectable rows),
    /// or the `No tasks currently running` empty state.
    List {
        /// The highlighted row (clamped as shells exit).
        selected: usize,
    },
    /// One shell's details: status/runtime/command fields over a live output
    /// box. Keyed by id so a *different* shell exiting never retargets the
    /// view; when this shell exits the view falls back to the list.
    Details { id: String },
}

/// How many lines PageUp/PageDown move the tool-output view.
const TOOL_VIEW_PAGE: usize = 10;

/// What running a slash command does. The palette dispatches one of these on
/// select; `App::run_selected_command` turns it into an [`Action`] for the loop.
/// Wiring a stub up later is just swapping its effect here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEffect {
    /// Clear the conversation history (`/clear`).
    Clear,
    /// Post the list of available commands as a system notice (`/help`) — or,
    /// while a turn is active, reject with a [`HELP_BUSY_NOTICE`] toast (its
    /// multi-line list would interleave with the streaming reply). See
    /// `docs/toast.md`.
    Help,
    /// Copy the last assistant response to the clipboard (`/copy`) — the
    /// confirmation surfaces as a toast, not a scrollback message. See
    /// `docs/copy.md` / `docs/toast.md`.
    Copy,
    /// Run codex's `/init`: submit the canned [`INIT_PROMPT`] as a regular
    /// user turn asking the model to generate an `AGENTS.md` contributor
    /// guide (`docs/init.md`) — or reject with an [`INIT_BUSY_NOTICE`] toast
    /// while a turn is active (codex's `available_during_task` is `false`).
    Init,
    /// Run codex's `/compact`: a summarization turn whose reply becomes the
    /// context bridge (`docs/compact.md`) — or reject with a
    /// [`COMPACT_BUSY_NOTICE`] toast while a turn is active (codex disables
    /// it mid-task) / a [`COMPACT_EMPTY_NOTICE`] toast when the derived
    /// context is empty.
    Compact,
    /// Open the `/resume` session picker — or reject with a
    /// [`RESUME_BUSY_NOTICE`] toast while a turn is active (codex blocks it
    /// mid-task; it swaps the whole conversation). See `docs/resume.md`.
    Resume,
    /// Open the inline `/model` picker. Works **mid-turn** — it only replaces the
    /// composer, never the running turn (which streams on its own thread); a
    /// switch only rebinds the *next* turn's backend. See `docs/llm.md` /
    /// `docs/toast.md`.
    Model,
    /// Open the inline `/login` API-key onboarding flow. Works **mid-turn** like
    /// `/model` — saving a key never touches the running turn. See `docs/llm.md`
    /// / `docs/toast.md`.
    Login,
    /// Exit the app (`/quit` — codex's `/quit`/`/exit`, "exit Codex").
    Quit,
}

/// One entry in the slash-command palette: how it shows (`name`/`description`)
/// and what it does (`effect`). Adding a command is a one-line addition to
/// [`COMMANDS`]; the palette, filtering, and scrolling don't change.
#[derive(Debug, Clone, Copy)]
pub struct SlashCommand {
    /// The command name **without** the leading slash (e.g. `"help"`), lowercase.
    pub name: &'static str,
    /// A one-line description shown dimmed beside the name in the palette.
    pub description: &'static str,
    /// What selecting it does.
    pub effect: CommandEffect,
}

/// The available slash commands, in the order they list in the palette. Adding a
/// command is a one-line entry here plus an effect arm in `run_selected_command`;
/// the palette, filtering, and scrolling don't change.
pub const COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "help",
        description: "List the available commands",
        effect: CommandEffect::Help,
    },
    SlashCommand {
        name: "clear",
        description: "Clear the conversation",
        effect: CommandEffect::Clear,
    },
    SlashCommand {
        name: "copy",
        description: "Copy the last response to the clipboard",
        effect: CommandEffect::Copy,
    },
    SlashCommand {
        name: "init",
        // Codex's description, its product name swapped for ours (the /quit
        // "Exit alter-zero" pattern).
        description: "create an AGENTS.md file with instructions for alter-zero",
        effect: CommandEffect::Init,
    },
    SlashCommand {
        name: "compact",
        // Codex's description, verbatim.
        description: "summarize conversation to prevent hitting the context limit",
        effect: CommandEffect::Compact,
    },
    SlashCommand {
        name: "resume",
        description: "Resume a saved chat",
        effect: CommandEffect::Resume,
    },
    SlashCommand {
        name: "model",
        description: "Switch the active model",
        effect: CommandEffect::Model,
    },
    SlashCommand {
        name: "login",
        description: "Add or update a provider API key",
        effect: CommandEffect::Login,
    },
    SlashCommand {
        name: "quit",
        description: "Exit alter-zero",
        effect: CommandEffect::Quit,
    },
];

/// The open slash-command palette: which match row is highlighted. The matches
/// themselves are derived from the input on demand ([`matching_commands`]); only
/// the highlight is stored. `None` on [`App`] means the palette is closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandMenu {
    /// Index of the highlighted command within the current filtered matches.
    pub selected: usize,
}

/// The `/resume` picker's sort key — codex's `Sort: [Updated] Created`
/// toolbar tab. `Updated` (the default) orders by file mtime, so a resumed
/// old session floats back up; `Created` by session start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResumeSort {
    /// Newest-modified first (codex's default).
    #[default]
    Updated,
    /// Newest-started first.
    Created,
}

/// The `/resume` picker's directory filter — codex's `Filter: [Cwd] All`
/// toolbar tab. `Cwd` (the default) lists only sessions whose meta recorded
/// the picker's own working directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResumeFilter {
    /// Only sessions recorded in this working directory (codex's default).
    #[default]
    Cwd,
    /// Every saved session.
    All,
}

/// Which toolbar control ←/→ act on — codex's Tab-cycled `ToolbarControl`.
/// Two controls, so Tab and BackTab both just swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResumeControl {
    /// The `Filter: [Cwd] All` tab pair (the initial focus, codex's).
    #[default]
    Filter,
    /// The `Sort: [Updated] Created` tab pair.
    Sort,
}

/// The open `/resume` session picker ([`View::ResumePicker`]): the saved
/// sessions the boundary scanned when it opened, the highlighted row, the
/// type-to-search query, and the Filter/Sort toolbar — codex's
/// `resume_picker.rs` picker state, sized down (see `docs/resume.md`). The
/// filtered rows derive on demand ([`matches`], the palette's
/// `matching_commands` pattern); `selected` indexes that filtered list.
///
/// [`matches`]: ResumePicker::matches
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResumePicker {
    /// Every eligible saved session, as scanned at open (mtime order; the
    /// active [`sort`] re-orders the derived rows). The seconds-ago values
    /// are frozen from that scan.
    ///
    /// [`sort`]: ResumePicker::sort
    pub sessions: Vec<SessionSummary>,
    /// Index of the highlighted row within the current filtered matches.
    pub selected: usize,
    /// The type-to-search query — any plain printable key appends, Backspace
    /// pops, Esc clears (codex's always-on picker search).
    pub query: String,
    /// The picker's own working directory, in the meta-line format — what
    /// the `Cwd` filter compares each session's recorded cwd against.
    pub cwd: String,
    /// The active directory filter (`Cwd` default — codex's).
    pub filter: ResumeFilter,
    /// The active sort key (`Updated` default — codex's).
    pub sort: ResumeSort,
    /// The toolbar control Tab focus is on (←/→ toggle its value).
    pub focus: ResumeControl,
}

impl ResumePicker {
    /// The rows the picker shows: the [`filter`]-passing sessions matching
    /// the query (a case-insensitive substring test on the preview — codex's
    /// client-side `Row::matches_query`; every session for an empty query),
    /// ordered by the active [`sort`] key, newest first.
    ///
    /// [`filter`]: ResumePicker::filter
    /// [`sort`]: ResumePicker::sort
    #[must_use]
    pub fn matches(&self) -> Vec<&SessionSummary> {
        let query = self.query.to_lowercase();
        let mut rows: Vec<&SessionSummary> = self
            .sessions
            .iter()
            .filter(|session| self.filter == ResumeFilter::All || session.cwd == self.cwd)
            .filter(|session| session.preview.to_lowercase().contains(&query))
            .collect();
        // Seconds-ago ascending = newest first; the sort is stable, so
        // same-second ties keep the scan's mtime order.
        match self.sort {
            ResumeSort::Updated => rows.sort_by_key(|session| session.updated_secs),
            ResumeSort::Created => rows.sort_by_key(|session| session.created_secs),
        }
        rows
    }
}

/// How many rows PageUp/PageDown move the `/resume` picker (the tool view's
/// page stride).
const RESUME_PAGE: usize = TOOL_VIEW_PAGE;

/// How many rows PageUp/PageDown move the inline `/model` picker.
const MODEL_PAGE: usize = TOOL_VIEW_PAGE;

/// The load state of the inline `/model` picker's list: the boundary spawns a
/// worker that fetches the provider's models, so the picker shows a placeholder
/// until the result lands. See `docs/llm.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ModelLoad {
    /// The fetch is in flight — the picker shows `Loading models…`.
    #[default]
    Loading,
    /// The models arrived (possibly an empty list).
    Ready,
    /// The fetch failed — the picker shows the message in red.
    Error(String),
    /// No provider has a resolvable API key yet, so no list is fetched — the
    /// picker points the user at `/login` instead of showing models they can't
    /// use. See `docs/llm.md`.
    NeedsLogin,
}

/// The inline `/model` picker's state (`None` on [`App`] when closed). Unlike
/// the alternate-screen `/resume` picker, this one **replaces the composer**
/// in the bottom live region with its own `>` search prompt and a scrolling
/// model list. Its rows come from the boundary's `/v1/models` fetch
/// ([`App::set_models`]); the filtered view derives on demand ([`matches`]).
///
/// [`matches`]: ModelPicker::matches
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelPicker {
    /// Every fetched model, provider-tagged and sorted (empty while loading).
    /// Grows as each provider's fetch lands ([`App::add_models`]).
    pub models: Vec<ModelEntry>,
    /// Whether the list is still loading, ready, or errored.
    pub status: ModelLoad,
    /// Index of the highlighted row within the current filtered matches.
    pub selected: usize,
    /// The type-to-search query — any plain printable key appends, Backspace
    /// pops, Esc clears (the `/resume` picker's search grammar).
    pub query: String,
    /// The currently active model id, marked with a ✓ in the list.
    pub active_id: String,
    /// The provider the active model belongs to, so the ✓ marks the exact active
    /// row (a merged multi-provider list can carry the same id twice). Empty when
    /// unknown — the ✓ then falls back to matching the id alone.
    pub active_provider: String,
    /// Provider fetches still outstanding — the `/model` picker fetches **every**
    /// configured provider in parallel and merges their lists as they arrive
    /// (docs/llm.md). Drives the `loading more…` counter hint, and at zero with
    /// no models it settles the all-failed error.
    pub pending: usize,
    /// Providers whose model fetch failed, kept so the picker can show which
    /// lists are missing (a `⚠ … unavailable` note beside a partial list, or
    /// one red row per provider when they all fail).
    pub errors: Vec<ModelFetchError>,
}

/// A provider whose `/model` fetch failed, with the concise reason to surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFetchError {
    /// The provider's human label (its `name`, e.g. `OpenRouter`).
    pub provider: String,
    /// The one-line failure reason (an [`crate::llm::LlmError`] rendering).
    pub message: String,
}

impl ModelPicker {
    /// The rows the picker shows: models whose id, provider, or friendly name
    /// contains the query (case-insensitive substring; every model for an empty
    /// query), keeping the fetched (alphabetical) order.
    #[must_use]
    pub fn matches(&self) -> Vec<&ModelEntry> {
        let query = self.query.to_lowercase();
        self.models
            .iter()
            .filter(|m| {
                query.is_empty()
                    || m.id.to_lowercase().contains(&query)
                    || m.provider.to_lowercase().contains(&query)
                    || m.display_name.to_lowercase().contains(&query)
            })
            .collect()
    }

    /// The highlighted model in the current filtered view, if any (an empty
    /// list — loading, error, or no match — has none).
    #[must_use]
    pub fn highlighted(&self) -> Option<&ModelEntry> {
        let matches = self.matches();
        matches.get(self.selected).copied()
    }

    /// Whether `m` is the active model (the ✓ row): the id matches and — when the
    /// active provider is known — so does the provider, so a shared id in the
    /// merged multi-provider list marks only the row actually in use.
    #[must_use]
    pub fn is_active(&self, m: &ModelEntry) -> bool {
        m.id == self.active_id
            && (self.active_provider.is_empty() || m.provider == self.active_provider)
    }

    /// Recompute the display status from the merge so far: `Ready` the moment any
    /// model is present (shown while the rest still load), else `Loading` while
    /// fetches are outstanding, else `Error` when every provider failed with no
    /// models, else `Ready` (all done, genuinely empty). The `Error` string is
    /// left empty — the picker renders the per-provider [`errors`] instead.
    ///
    /// [`errors`]: ModelPicker::errors
    fn recompute_status(&mut self) {
        self.status = if !self.models.is_empty() {
            ModelLoad::Ready
        } else if self.pending > 0 {
            ModelLoad::Loading
        } else if !self.errors.is_empty() {
            ModelLoad::Error(String::new())
        } else {
            ModelLoad::Ready
        };
    }
}

/// How many rows PageUp/PageDown move the `/login` provider list.
const LOGIN_PAGE: usize = TOOL_VIEW_PAGE;

/// Which step of the inline `/login` onboarding flow is showing (docs/llm.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyStep {
    /// Choosing which provider to set a key for (a filterable list).
    #[default]
    Provider,
    /// Entering (pasting) the API key for the chosen provider.
    Key,
}

/// One selectable provider row in the `/login` flow — plain data injected by the
/// boundary (the `configured` flag needs boundary key resolution, like the
/// `/model` list). See `docs/llm.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderChoice {
    /// The provider id (e.g. `openrouter`).
    pub id: String,
    /// The human-readable label (the `providers.toml` `name`).
    pub name: String,
    /// The environment variable its key is stored under (e.g. `OPENROUTER_API_KEY`).
    pub env_var: String,
    /// Whether a key already resolves for it (shown with a ✓).
    pub configured: bool,
}

/// The inline `/login` onboarding flow's state (`None` on [`App`] when closed).
/// Like the `/model` picker it **replaces the composer** in the bottom live
/// region; unlike it, it's a two-step flow — pick a provider
/// ([`KeyStep::Provider`]), then enter its API key masked ([`KeyStep::Key`]).
/// The chosen key is persisted to `.env` by the boundary
/// ([`Action::SaveApiKey`]). See `docs/llm.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyOnboarding {
    /// The providers to choose from, injected at open
    /// ([`App::open_key_onboarding`]).
    pub providers: Vec<ProviderChoice>,
    /// Which step is showing.
    pub step: KeyStep,
    /// Index of the highlighted provider within the current filtered matches.
    pub selected: usize,
    /// The provider filter query (step 1).
    pub query: String,
    /// The provider being keyed (an index into `providers`), set when step 2
    /// opens so the key entry keeps its provider even as the (unused) filter
    /// would otherwise reorder matches.
    pub chosen: Option<usize>,
    /// The API key being typed / pasted (step 2). Rendered masked.
    pub key_input: String,
    /// Where saved keys land (the `~`-relative `.env` path), injected at open by
    /// the boundary so the provider-step hint names the real file even under an
    /// `ALTER_ZERO_ENV_FILE` override. See `docs/llm.md`.
    pub env_path: String,
}

impl KeyOnboarding {
    /// Providers whose id or name contains the query (case-insensitive
    /// substring; every provider for an empty query), keeping the injected
    /// order.
    #[must_use]
    pub fn matches(&self) -> Vec<&ProviderChoice> {
        let query = self.query.to_lowercase();
        self.providers
            .iter()
            .filter(|p| {
                query.is_empty()
                    || p.id.to_lowercase().contains(&query)
                    || p.name.to_lowercase().contains(&query)
            })
            .collect()
    }

    /// The highlighted provider in the current filtered view (step 1).
    #[must_use]
    pub fn highlighted(&self) -> Option<&ProviderChoice> {
        self.matches().get(self.selected).copied()
    }

    /// The provider chosen for key entry (step 2), if any.
    #[must_use]
    pub fn chosen_provider(&self) -> Option<&ProviderChoice> {
        self.chosen.and_then(|i| self.providers.get(i))
    }
}

/// The open `@` file picker (when the cursor is in an `@token`); `None` when
/// closed. Unlike the slash palette — whose matches derive from the input —
/// these come from the **filesystem** asynchronously, so they're stored here.
/// The boundary dispatches a search whenever [`App::file_search_query`] changes
/// and feeds results back via [`App::set_file_matches`]. See
/// `docs/file-search.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileSearch {
    /// Index of the highlighted match.
    pub selected: usize,
    /// The `@token` query the current `matches` are for (staleness guard).
    pub query: String,
    /// The ranked file matches for `query`, capped by the boundary.
    pub matches: Vec<FileMatch>,
    /// A search for the current query is in flight (show *Searching…*).
    pub waiting: bool,
}

/// The slash-command query in `input`, if it is a **bare command token**: a
/// leading `/` followed by no whitespace (so `/`, `/he`, `/help` qualify, but
/// `ask /help`, `/help me`, and `/a\nb` do not — a space or newline ends it).
/// `Some("")` for a lone `/` (lists everything).
#[must_use]
pub fn command_query(input: &str) -> Option<&str> {
    let rest = input.strip_prefix('/')?;
    if rest.chars().any(char::is_whitespace) {
        None
    } else {
        Some(rest)
    }
}

/// The commands whose name starts with `query` (case-insensitive), in registry
/// order. An empty query matches everything.
#[must_use]
pub fn matching_commands(query: &str) -> Vec<&'static SlashCommand> {
    let q = query.to_lowercase();
    COMMANDS.iter().filter(|c| c.name.starts_with(&q)).collect()
}

/// The shell command in `input`, if it is a **`!`-prefixed** line: a leading
/// `!` followed by the rest of the line (spaces and all — unlike
/// [`command_query`], a shell command obviously contains whitespace). `Some("")`
/// for a lone `!`. This is what flips the composer into "shell mode" (the red
/// footer hint) and, on Enter from an idle composer, runs the rest locally. A
/// port of codex's `is_bash_shell_command`; see `docs/shell-command.md`.
#[must_use]
pub fn shell_query(input: &str) -> Option<&str> {
    input.strip_prefix('!')
}

/// The `/help` notice: a header followed by every command's `/name — description`.
fn help_text() -> String {
    let mut text = String::from("Available commands:");
    for cmd in COMMANDS {
        text.push_str(&format!("\n/{} — {}", cmd.name, cmd.description));
    }
    text
}

/// Shell-style ↑/↓ recall of previously submitted inputs — a port of codex's
/// `ChatComposerHistory` (its in-session `local_history`; no cross-session
/// persistence or Ctrl+R search). See `docs/input-history.md`.
#[derive(Debug, Default)]
pub struct InputHistory {
    /// The recorded texts, oldest first (newest at the end).
    entries: Vec<String>,
    /// The entry currently recalled; `None` when not browsing.
    cursor: Option<usize>,
    /// What navigation last wrote into the composer. The gate in
    /// [`should_navigate`] compares against it so an *edited* recall counts as
    /// a fresh draft and stops browsing.
    ///
    /// [`should_navigate`]: InputHistory::should_navigate
    last_recall: Option<String>,
    /// Entries recorded *this session* that the boundary has not yet flushed to
    /// the persistent history file, newest last — the "core queues, boundary
    /// does the I/O" pattern (like `insert_before`'s pending lines). Filled by
    /// [`record`] on a genuine append, drained by [`take_unpersisted`]. Seeded
    /// entries (already on disk) never land here. See `docs/history-persistence.md`.
    ///
    /// [`record`]: InputHistory::record
    /// [`take_unpersisted`]: InputHistory::take_unpersisted
    unpersisted: Vec<String>,
    /// The last text queued for (or seeded from) the persistent file — the
    /// dedup target for persistence, kept **separate** from `entries`'s
    /// in-memory dedup so a never-persisted [`record_ephemeral`] draft can't
    /// suppress a genuine submission's write. See `docs/history-persistence.md`.
    ///
    /// [`record_ephemeral`]: InputHistory::record_ephemeral
    last_persisted: Option<String>,
}

impl InputHistory {
    /// Record a submitted input and exit browsing, **queuing it for the
    /// persistent history file**. Blank texts are ignored and an entry
    /// identical to the newest is collapsed in memory, like codex's
    /// `record_local_submission`. The persist dedup is against
    /// [`last_persisted`], **not** `entries` — so a never-persisted
    /// [`record_ephemeral`] draft can't mask a genuine submission's write (the
    /// file still collapses adjacent duplicates). See `docs/history-persistence.md`.
    ///
    /// [`last_persisted`]: InputHistory::last_persisted
    /// [`record_ephemeral`]: InputHistory::record_ephemeral
    pub fn record(&mut self, text: &str) {
        self.record_inner(text);
        if text.is_empty() || self.last_persisted.as_deref() == Some(text) {
            return;
        }
        self.last_persisted = Some(text.to_string());
        self.unpersisted.push(text.to_string());
    }

    /// Record an input that should recall this session but **never persist** —
    /// the Ctrl+C-cleared draft. codex keeps cleared drafts in its in-session
    /// `local_history` only, so an abandoned draft doesn't pollute the
    /// cross-session history file, and it never advances [`last_persisted`]
    /// (`docs/history-persistence.md`).
    ///
    /// [`last_persisted`]: InputHistory::last_persisted
    pub fn record_ephemeral(&mut self, text: &str) {
        self.record_inner(text);
    }

    /// The shared record body: exit browsing, drop blanks and adjacent
    /// duplicates, append otherwise.
    fn record_inner(&mut self, text: &str) {
        self.cursor = None;
        self.last_recall = None;
        if text.is_empty() || self.entries.last().is_some_and(|prev| prev == text) {
            return;
        }
        self.entries.push(text.to_string());
    }

    /// Seed `entries` from the persistent history file at startup (oldest
    /// first), as a faithful **replay of [`record`]**: each text runs the same
    /// blank-skip + adjacent-duplicate collapse, so a messy or concurrently
    /// written file (adjacent dups) seeds the same clean buffer a fresh session
    /// would build. These are already on disk, so they are **not** queued for
    /// persistence — but the newest seeded entry becomes the [`last_persisted`]
    /// dedup target, so a first submission identical to it isn't re-written.
    /// Both ↑/↓ recall and Ctrl+R search read `entries`, so seeding makes both
    /// span sessions with no other change. See `docs/history-persistence.md`.
    ///
    /// [`record`]: InputHistory::record
    /// [`last_persisted`]: InputHistory::last_persisted
    pub fn seed(&mut self, entries: Vec<String>) {
        for text in entries {
            self.record_inner(&text);
        }
        self.last_persisted = self.entries.last().cloned();
    }

    /// Drain the entries recorded this session that are not yet on disk, for
    /// the boundary to append. See `docs/history-persistence.md`.
    pub fn take_unpersisted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.unpersisted)
    }

    /// Should an ↑/↓ press browse history instead of moving the cursor? Yes
    /// for an empty composer; for a non-empty one only when the text is
    /// exactly the last recalled entry (unedited) with the cursor at either
    /// end — so a typed draft is never clobbered and the arrows still move
    /// within an edited recall (codex's `should_handle_navigation`).
    #[must_use]
    pub fn should_navigate(&self, text: &str, cursor: usize) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        if text.is_empty() {
            return true;
        }
        if cursor != 0 && cursor != text.len() {
            return false;
        }
        self.last_recall.as_deref() == Some(text)
    }

    /// Step to the older entry (↑), entering browsing at the newest. `None` at
    /// the oldest — the caller falls back to cursor movement, like codex.
    pub fn up(&mut self) -> Option<String> {
        let next = match self.cursor {
            None => self.entries.len().checked_sub(1)?,
            Some(0) => return None,
            Some(index) => index - 1,
        };
        self.cursor = Some(next);
        let text = self.entries[next].clone();
        self.last_recall = Some(text.clone());
        Some(text)
    }

    /// Indices of the entries whose text contains `query` (case-insensitively),
    /// newest first, keeping only the **newest** occurrence of duplicated
    /// texts — codex's Ctrl+R traversal (`chat_composer_history.rs::search`:
    /// lowercased substring match, `seen_texts` dedup). An empty query matches
    /// every entry, though the search session treats that as Idle. See
    /// `docs/history-search.md`.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<usize> {
        let needle = query.to_lowercase();
        let mut seen = HashSet::new();
        self.entries
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, text)| text.to_lowercase().contains(&needle))
            .filter(|(_, text)| seen.insert(text.as_str()))
            .map(|(index, _)| index)
            .collect()
    }

    /// The recorded text at `index` (0 = oldest — as [`search`] indexes them).
    ///
    /// [`search`]: InputHistory::search
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str)
    }

    /// Seat ↑/↓ browsing at `index`, as if that entry had just been recalled:
    /// the next ↑ steps to the entry older than it. Accepting a Ctrl+R match
    /// lands here — codex's search shares its navigation cursor the same way
    /// (`search_match` sets `history_cursor` + `last_history_text`).
    pub fn resume_at(&mut self, index: usize) {
        if let Some(text) = self.entries.get(index) {
            self.cursor = Some(index);
            self.last_recall = Some(text.clone());
        }
    }

    /// Step to the newer entry (↓). Past the newest returns an empty string —
    /// "clear the composer and stop browsing" (codex's `navigate_down`);
    /// `None` when not browsing at all.
    pub fn down(&mut self) -> Option<String> {
        let index = self.cursor?;
        if index + 1 < self.entries.len() {
            self.cursor = Some(index + 1);
            let text = self.entries[index + 1].clone();
            self.last_recall = Some(text.clone());
            Some(text)
        } else {
            self.cursor = None;
            self.last_recall = None;
            Some(String::new())
        }
    }
}

/// One open Ctrl+R reverse history search — codex's `HistorySearchSession`
/// (`chat_composer/history_search.rs`). While it is `Some` on [`App`] the
/// search owns every key: typed characters edit [`query`], Ctrl+R/↑ and
/// Ctrl+S/↓ step between matches, Enter accepts the previewed match as an
/// editable draft, and Esc/Ctrl+C restore the [`snapshot`]. The footer slot
/// renders it as `reverse-i-search: {query}` (`ui::search_line`). See
/// `docs/history-search.md`.
///
/// [`query`]: HistorySearch::query
/// [`snapshot`]: HistorySearch::snapshot
#[derive(Debug)]
pub struct HistorySearch {
    /// The draft (text **and** cursor) from before the search opened, restored
    /// verbatim on cancel — and shown again while a query has no match
    /// (codex's `original_draft`).
    snapshot: TextArea,
    /// Whether the composer was in `!` shell mode when the search opened. The
    /// mode is suspended during the search (previews show entries raw) and
    /// restored with the snapshot on cancel; accepting re-derives it from the
    /// accepted text instead. See `docs/shell-command.md`.
    snapshot_shell: bool,
    /// The footer-owned query typed while the search is active.
    pub query: String,
    /// The user-visible phase: drives the footer hints and the preview.
    pub state: SearchState,
}

/// Byte ranges in `text` where `query` matches case-insensitively — a port of
/// codex's `case_insensitive_match_ranges`: both sides are folded with
/// `char::to_lowercase`, and a span map keeps the folded match positions
/// aligned with the original bytes even when a fold changes length (e.g. `İ`
/// lowercases to two chars). Non-overlapping, left to right.
fn case_insensitive_match_ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    let query_lower: String = query.chars().flat_map(char::to_lowercase).collect();
    if query_lower.is_empty() {
        return Vec::new();
    }
    // The folded text, and each folded char's originating byte range.
    let mut folded = String::new();
    let mut spans: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    for (start, ch) in text.char_indices() {
        let original = start..start + ch.len_utf8();
        for lower in ch.to_lowercase() {
            let folded_start = folded.len();
            folded.push(lower);
            spans.push((folded_start..folded.len(), original.clone()));
        }
    }
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(found) = folded.get(from..).and_then(|rest| rest.find(&query_lower)) {
        let fold = from + found..from + found + query_lower.len();
        let hit = |f: &Range<usize>| f.end > fold.start && f.start < fold.end;
        let first = spans.iter().find(|(f, _)| hit(f));
        let last = spans.iter().rev().find(|(f, _)| hit(f));
        if let (Some((_, first)), Some((_, last))) = (first, last) {
            ranges.push(first.start..last.end);
        }
        from = fold.end;
    }
    ranges
}

/// The phase of an open [`HistorySearch`] (codex's `HistorySearchStatus`,
/// minus `Searching` — we have no async persistent history to wait on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchState {
    /// Empty query: nothing searched yet — the original draft still shows
    /// (opening Ctrl+R never previews the latest entry by itself).
    Idle,
    /// A match is previewed in the composer; `selected` indexes the
    /// newest-first unique match list ([`InputHistory::search`]).
    Match {
        /// Index into the current query's match list (0 = newest).
        selected: usize,
    },
    /// The query matches nothing: the original draft shows again, but the
    /// search stays open for more typing.
    NoMatch,
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

/// All mutable conversation state: the editable input line, the reply currently
/// being streamed, the tool (if any) currently executing, and the finished
/// history of messages and tool calls.
///
/// Finished items are shown via the terminal's scrollback, but [`history`] keeps
/// them so they can be repainted when a resize clears the screen, or when
/// returning from the tool-output view.
///
/// [`history`]: App::history
#[derive(Debug, Default)]
pub struct App {
    /// The editable input line — a codex-style cursor you can move anywhere, with
    /// insert/delete at the cursor and movement across wrapped rows. See
    /// [`crate::textarea`].
    pub input: TextArea,
    /// Shell-style ↑/↓ recall of submitted inputs (and Ctrl+C-cleared drafts).
    /// Survives `/clear` — like codex, whose history even spans sessions. See
    /// `docs/input-history.md`.
    pub input_history: InputHistory,
    /// The open Ctrl+R reverse search over [`input_history`], if one is —
    /// `None` when closed. While `Some`, every key routes to it
    /// ([`on_key_search`]) and the footer slot shows the query line. See
    /// `docs/history-search.md`.
    ///
    /// [`input_history`]: App::input_history
    /// [`on_key_search`]: App::on_key_search
    pub history_search: Option<HistorySearch>,
    /// Whether the composer is in `!` shell mode — codex's absorbed-prefix
    /// `is_bash_mode`: the leading `!` is held here, *not* in the textarea, so
    /// the box renders `! pwd` (the bang as the prompt) instead of `❯ !pwd`.
    /// Entered by typing `!` first ([`sync_shell_mode`] absorbs it), exited by
    /// Backspace/Esc on an empty composer or by submitting; the footer slot
    /// shows a red `Shell mode` hint while it's on. See `docs/shell-command.md`.
    ///
    /// [`sync_shell_mode`]: App::sync_shell_mode
    pub shell_mode: bool,
    /// Messages submitted while a turn was in flight, awaiting their own turns
    /// (codex's `queued_user_messages`). The queue is a sequence of **turn
    /// batches**: an Enter mid-turn appends to the last batch (consecutive
    /// Enters share one next turn), while **Tab opens a new batch** so its
    /// message runs as a *separate follow-up turn* after the ones already
    /// queued. The loop pops the front batch ([`drain_next_batch`]) into one
    /// turn whenever the current one ends, so the batches iterate in order — and
    /// Esc-interrupt sends the front batch right away. Only ever non-empty while
    /// a turn is active. See `docs/queue.md`.
    ///
    /// [`drain_next_batch`]: App::drain_next_batch
    pub queued: VecDeque<QueuedTurn>,
    /// Whether the `?` shortcuts band (the keyboard-shortcuts overview below
    /// the input box — codex's footer shortcut overlay) is showing. Toggled by
    /// `?` from an empty composer; any other key closes it. See
    /// `docs/shortcuts.md`.
    pub shortcuts_open: bool,
    /// `Some(buffer)` while the AI reply is streaming, accumulating chunks.
    /// Private: read through [`streaming_text`]/[`is_streaming`] — the turn
    /// invariants (an open buffer implies a live status, a running tool is
    /// resolved on every turn death) hold only if mutation stays in here.
    ///
    /// [`streaming_text`]: App::streaming_text
    /// [`is_streaming`]: App::is_streaming
    streaming: Option<String>,
    /// `Some` while a `/compact` turn is in flight — the flag *and* the
    /// accumulator: [`push_chunk`](App::push_chunk) diverts the streamed
    /// summary here so the visible reply buffer stays empty (the summary is
    /// never rendered, codex parity) while the token tally still ticks.
    /// [`finish_compact`](App::finish_compact) takes it into the appended
    /// [`HistoryItem::Compaction`] marker; an interrupt, a backend error, or
    /// a `/clear` drops it — the old context stands. See `docs/compact.md`.
    compact_buffer: Option<String>,
    /// Whether the in-flight `/compact` turn was **auto-triggered** (the gauge
    /// crossed the threshold) vs the manual command — recorded onto the marker
    /// so its cell can say `· auto`. Meaningful only while
    /// [`compact_buffer`](Self::compact_buffer) is `Some`.
    compact_auto: bool,
    /// The context gauge when the in-flight compaction began — the marker's
    /// `before` count.
    compact_before: u64,
    /// The active model's context window in tokens, when known (the provider's
    /// `/v1/models` `context_length`, the saved settings, or the
    /// `ALTER_ZERO_CONTEXT_WINDOW` override — injected at the boundary via
    /// [`set_context_window`](Self::set_context_window)). Drives the footer's
    /// `{used}%/{window}` gauge and the auto-compact trigger; `None` hides
    /// both. See `docs/compact.md`.
    context_window: Option<u64>,
    /// The current context size in tokens: the last usage frame's
    /// `input + output` (the provider's own accounting of the re-sent context
    /// plus the reply that joins the next request), or — when no usage arrived
    /// (the dummy) or the history just mutated (a compaction, a `/clear`, a
    /// backtrack, a `/resume`) — the tokenizer estimate over the derived
    /// context ([`estimate_context_tokens`](Self::estimate_context_tokens)).
    context_used: u64,
    /// One auto-compact attempt per user turn (codex's per-turn semantics):
    /// set when a compact turn ends **any** way — landed, interrupted, or
    /// failed — and cleared when the next real turn begins, so a compaction
    /// that didn't (or couldn't) shrink the context never re-triggers in a
    /// tight loop, and an Esc'd one isn't immediately restarted against the
    /// user's wishes. See `docs/compact.md`.
    auto_compact_blocked: bool,
    /// The streaming strip preview's row count, injected by the boundary before
    /// each draw ([`set_stream_preview_rows`], the [`set_status_times`]
    /// pattern): 1 for a normal reply's single-row preview, the forming block's
    /// height while a table streams — so `ui::preview_rows` reserves exactly
    /// the rows `ui::StreamRender::preview` renders. See
    /// `docs/table-streaming.md`.
    ///
    /// [`set_stream_preview_rows`]: App::set_stream_preview_rows
    /// [`set_status_times`]: App::set_status_times
    stream_preview_rows: u16,
    /// The live tool calls of the current turn, front-first: the front is the
    /// running (status [`ToolStatus::Running`]) or about-to-run call, and any
    /// calls behind it are [`ToolStatus::Waiting`] siblings of a **parallel
    /// batch** (`start_tool_batch`), shown as `⎿ Waiting…` until each starts.
    /// Empty when no tool is in flight. A lone tool (the `!` shell, the dummy's
    /// single calls) is a one-element queue. Execution is sequential, so at most
    /// one call is ever `Running`, and it is always the front. Private: read the
    /// active call through [`current_tool`](App::current_tool) and the whole
    /// batch through [`tool_queue`](App::tool_queue). See `docs/parallel-tools.md`.
    tool_queue: VecDeque<ToolCall>,
    /// Every finished message and tool call, oldest first — used to repaint after
    /// a resize or on returning from the tool-output view.
    pub history: Vec<HistoryItem>,
    /// Bumped by every **non-append** [`history`](Self::history) mutation — a
    /// `/clear`, a `/resume` load, a backtrack truncation, an interrupt-undo
    /// pop. Committed items are immutable and otherwise only ever appended, so
    /// `(generation, len)` identifies a history prefix exactly — what lets the
    /// Ctrl+O transcript cache ([`crate::ui::TranscriptCache`]) keep rendered
    /// items frozen across refreshes (and overlay closes) instead of
    /// re-highlighting all of history every open. Read via
    /// [`history_generation`](Self::history_generation).
    history_generation: u64,
    /// Which screen is showing (Ctrl+O toggles to the tool-output view).
    pub view: View,
    /// The tool-output view's vertical scroll offset, in lines from the top.
    pub tool_scroll: usize,
    /// Whether the tool-output view is pinned to the bottom (tail-follow): it
    /// opens this way and re-streams keep the latest content in view, until you
    /// scroll up to read back (and re-engages when you scroll to the bottom).
    pub tool_follow: bool,
    /// The Ctrl+D context-debug view's vertical scroll offset, in lines from
    /// the top — its own state so flipping between overlays never clobbers the
    /// transcript pager's place. See `docs/context.md`.
    pub debug_scroll: usize,
    /// Whether the context-debug view is pinned to the bottom (tail-follow),
    /// exactly like [`tool_follow`](Self::tool_follow).
    pub debug_follow: bool,
    /// The active backend's system prompt, injected at the boundary
    /// ([`App::set_system_prompt`], from `ReplySource::system_prompt`) so the
    /// Ctrl+D view can show the *whole* context window. `None` for the dummy.
    pub system_prompt: Option<String>,
    /// The project's AGENTS.md instructions, rendered as codex's
    /// user-instructions fragment and injected at the boundary
    /// ([`App::set_user_instructions`], from `project_doc::load_user_instructions`
    /// — loaded at startup and refreshed at every turn start, so a `/init`-
    /// generated guide rides the very next turn). The context derivation
    /// prepends it as the window's first user entry
    /// (`context::context_messages_with`), the Ctrl+D view shows it there,
    /// and the offline token estimate counts it. See `docs/project-doc.md`.
    pub user_instructions: Option<String>,
    /// The Esc-Esc backtrack gesture (edit a previous message): primed by Esc
    /// from an idle empty composer when a previous user message exists,
    /// previewing in the transcript overlay, confirmed with Enter. Reset by
    /// any view toggle and by any non-Esc key. See `docs/backtrack.md`.
    pub backtrack: Backtrack,
    /// The open slash-command palette (when the input is a bare command token);
    /// `None` when closed. Esc dismisses it (and it stays dismissed within the
    /// same token); see [`App::refresh_command_menu`].
    pub command_menu: Option<CommandMenu>,
    /// The open `@` file picker (when the cursor is in an `@token`); `None` when
    /// closed. Esc dismisses it (sticky within the token); the boundary fetches
    /// its matches asynchronously. See [`App::refresh_file_search`] and
    /// `docs/file-search.md`.
    pub file_search: Option<FileSearch>,
    /// The open `/resume` session picker; `Some` exactly while
    /// [`View::ResumePicker`] is showing ([`App::open_resume_picker`] /
    /// [`App::close_resume_picker`] keep the two in step). See
    /// `docs/resume.md`.
    pub resume_picker: Option<ResumePicker>,
    /// The open inline `/model` picker; `None` when closed. Unlike the resume
    /// picker it is **not** a [`View`] — it renders inline, replacing the
    /// composer in the bottom region, and owns every key while open (like the
    /// Ctrl+R search). See `docs/llm.md`.
    pub model_picker: Option<ModelPicker>,
    /// The open inline `/login` API-key onboarding flow; `None` when closed.
    /// Like [`model_picker`](Self::model_picker) it renders inline and owns
    /// every key while open. Mutually exclusive with the picker. See
    /// `docs/llm.md`.
    pub key_onboarding: Option<KeyOnboarding>,
    /// The live status of the turn in flight (verb, token tally, arrow, and the
    /// boundary-supplied seconds), shown in the strip above the box. `Some` from
    /// [`begin_stream`] until the turn ends; `None` when idle. See
    /// `docs/status-indicator.md`.
    ///
    /// [`begin_stream`]: App::begin_stream
    /// Private: read through [`status`](App::status).
    status: Option<TurnStatus>,
    /// How many turns have started — drives the deterministic per-turn verb pick
    /// ([`WORKING_VERBS`]/[`DONE_VERBS`]). Incremented by [`begin_stream`].
    ///
    /// [`begin_stream`]: App::begin_stream
    turn_count: usize,
    /// The turn's accumulated **real** usage — billed tokens summed over the
    /// provider's per-round usage frames ([`App::apply_usage`]), which the
    /// live tally snaps to (replacing the estimate ticked so far) and the
    /// turn summary records. Reset by [`begin_stream`]/[`begin_shell`]. See
    /// `docs/prompt-caching.md`.
    ///
    /// [`begin_stream`]: App::begin_stream
    /// [`begin_shell`]: App::begin_shell
    turn_usage_tokens: usize,
    /// The cached-read share of [`turn_usage_tokens`](Self::turn_usage_tokens).
    turn_usage_cached: usize,
    /// Wall-clock used to stamp recorded items, injected at the I/O boundary
    /// ([`App::set_clock`]). `None` in unit tests (→ empty stamp, keeping the
    /// pure logic deterministic); `main.rs` sets a real local-time clock. The
    /// stamp is only ever shown in the Ctrl+O transcript (see
    /// `docs/timestamps.md`).
    clock: Option<fn() -> String>,
    /// The session context shown in the footer under the input box (codex's
    /// footer status line), injected at the I/O boundary
    /// ([`App::set_session_info`]). `None` — the unit-test default — means no
    /// footer row. See `docs/footer.md`.
    pub session: Option<SessionInfo>,
    /// The active model's reasoning capability and chosen thinking mode —
    /// `None` when the model doesn't support reasoning (or support is
    /// unknown, e.g. the dummy backend). Injected at the boundary
    /// ([`App::set_thinking`], the [`set_session_info`] pattern), cycled by
    /// Shift+Tab, and shown beside the model name in the footer. See
    /// `docs/reasoning.md`.
    ///
    /// [`set_session_info`]: App::set_session_info
    pub thinking: Option<ThinkingState>,
    /// Real text behind each large-paste placeholder currently in the composer,
    /// as `(placeholder, real_text)` pairs in insertion order (codex's
    /// `pending_pastes`). A paste over [`crate::paste::LARGE_PASTE_CHAR_THRESHOLD`]
    /// shows a compact `[Pasted Content N chars]` placeholder
    /// ([`on_paste`]) while its text waits here; [`take_input`] splices it back in
    /// when the draft is sent. Cleared whenever the composer empties (every
    /// `take`), but **not** by `/clear` (the draft survives `/clear`, so its
    /// pastes do too). See `docs/paste.md`.
    ///
    /// [`on_paste`]: App::on_paste
    /// [`take_input`]: App::take_input
    pub pasted: Vec<(String, String)>,
    /// Ctrl+V-pasted images currently in the composer, as `(placeholder, path)`
    /// pairs in insertion order — the image analogue of [`pasted`] (codex's
    /// `AttachedImage`). [`attach_image`] inserts an `[Image #N]` placeholder and
    /// records the temp-PNG path here; unlike a text paste the placeholder is
    /// **not** expanded on send (it stays in the message text), and the path is
    /// surfaced separately via [`take_submission_images`]. Cleared by
    /// [`take_input`] (any draft-take drops attachments), so the idle submit path
    /// stages them into [`submission_images`] first. See `docs/image-paste.md`.
    ///
    /// [`pasted`]: App::pasted
    /// [`attach_image`]: App::attach_image
    /// [`take_input`]: App::take_input
    /// [`take_submission_images`]: App::take_submission_images
    /// [`submission_images`]: App::submission_images
    pub images: Vec<(String, PathBuf)>,
    /// The just-submitted turn's image attachments — the whole
    /// `(placeholder, path)` pairs, staged by the idle submit path (moved out
    /// of [`images`] before [`take_input`] clears the composer) for the loop
    /// to drain via [`take_submission_images`] — codex's
    /// `recent_submission_images`. The names ride along so recording can
    /// match each path to the message text that carries its placeholder
    /// (`paste::distribute_images`). See `docs/image-paste.md`.
    ///
    /// [`images`]: App::images
    /// [`take_input`]: App::take_input
    /// [`take_submission_images`]: App::take_submission_images
    submission_images: Vec<(String, PathBuf)>,
    /// Temp-PNG paths of attachments that were **dropped without being
    /// submitted** — an atomic placeholder delete, a Ctrl+C-cleared draft, a
    /// `/clear`'d queue. The pure core only records the drops; the boundary
    /// drains [`take_discarded_images`] and removes the files (the file I/O
    /// stays out of the library). Submitted images are *not* recorded here —
    /// their files outlive the send. See `docs/image-paste.md`.
    ///
    /// [`take_discarded_images`]: App::take_discarded_images
    discarded_images: Vec<PathBuf>,
    /// The transient status line shown just above the box, if one is live —
    /// a confirmation or soft rejection that self-clears after a few seconds
    /// ([`show_toast`]/[`clear_toast`]). Never recorded in [`history`]: a toast
    /// is UI, not conversation. Its expiry is timed at the I/O boundary
    /// (`main.rs`'s `toast_deadline`), the timestamp pattern. See
    /// `docs/toast.md`.
    ///
    /// [`show_toast`]: App::show_toast
    /// [`clear_toast`]: App::clear_toast
    /// [`history`]: App::history
    toast: Option<Toast>,
    /// The [`history`] index below which the interrupt-undo may never pop.
    /// Usually 0 — a previous turn's tail (summary/notice/tool) already stops
    /// [`take_trailing_user_messages`] — but a `/resume` can install a history
    /// *ending* with a user message (a rollout cut short mid-turn), and a
    /// backtrack rewind can leave an older batch-sibling user message as the
    /// tail; the floor keeps the undo from swallowing those into the composer.
    ///
    /// [`history`]: App::history
    /// [`take_trailing_user_messages`]: App::take_trailing_user_messages
    undo_floor: usize,
    /// The **running** background shells, oldest first — fed by the
    /// boundary's `BgEvent`s ([`bg_started`]/[`bg_output`]/[`bg_exited`]).
    /// Drives the footer's `· {n} shells` count, the `Done for Ns · {n}
    /// shells still running` summary suffix, and the ↓ manager band. See
    /// `docs/background.md`.
    ///
    /// [`bg_started`]: App::bg_started
    /// [`bg_output`]: App::bg_output
    /// [`bg_exited`]: App::bg_exited
    background: Vec<BackgroundShell>,
    /// Whether any background shell has ever started this session — the ↓
    /// gate: once true, ↓ from an empty composer opens the manager (showing
    /// the `No tasks currently running` empty state after they all finish).
    /// Reset by `/clear` (which also kills the shells).
    had_background: bool,
    /// The open ↓ background manager band; `None` when closed. Inline like
    /// [`model_picker`](Self::model_picker) — it replaces the composer and
    /// owns every key while open. See `docs/background.md`.
    pub background_view: Option<BackgroundView>,
    /// Completions that landed while a turn was in flight, awaiting the turn
    /// end: the loop settles them there — notices committed in arrival order,
    /// and (for model-launched shells with nothing queued) the automatic
    /// follow-up turn dispatched. See `docs/background.md`.
    pending_bg: VecDeque<BgCompletion>,
    /// How long the **current running command** (a model `bash` call or a `!`
    /// shell run) has been executing — boundary-injected each frame
    /// ([`set_command_elapsed`](App::set_command_elapsed), the
    /// [`set_status_times`](App::set_status_times) pattern; the clock lives in
    /// `main.rs`). `None` when no command is running. Gates the delayed
    /// `(ctrl+b to run in background)` preview hint so a fast command never
    /// flashes it (`docs/background.md`).
    command_elapsed: Option<Duration>,
}

impl App {
    /// A fresh app: empty input, not streaming.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Is an AI reply currently being streamed?
    #[must_use]
    pub const fn is_streaming(&self) -> bool {
        self.streaming.is_some()
    }

    /// The [`history`](Self::history) mutation generation: unchanged by
    /// appends, bumped by every clear/replace/truncate/pop — see the field
    /// docs. `(generation, history.len())` pins a rendered prefix exactly.
    #[must_use]
    pub const fn history_generation(&self) -> u64 {
        self.history_generation
    }

    /// Inject the wall-clock used to stamp recorded items (called once at the I/O
    /// boundary in `main.rs`). Each recorded message/tool then stores `clock()`'s
    /// value, which is shown **only** in the Ctrl+O transcript.
    pub fn set_clock(&mut self, clock: fn() -> String) {
        self.clock = Some(clock);
    }

    /// Seed the input history from the persistent file at startup (called once
    /// at the I/O boundary, the [`set_clock`] pattern): both ↑/↓ recall and
    /// Ctrl+R search then span past sessions. See `docs/history-persistence.md`.
    ///
    /// [`set_clock`]: App::set_clock
    pub fn seed_input_history(&mut self, entries: Vec<String>) {
        self.input_history.seed(entries);
    }

    /// Drain the inputs recorded this session that are not yet on disk, for the
    /// boundary to append to the persistent history file (called once per loop
    /// iteration beside `recorder.sync`). See `docs/history-persistence.md`.
    pub fn take_unpersisted_inputs(&mut self) -> Vec<String> {
        self.input_history.take_unpersisted()
    }

    /// Inject the session context shown in the footer under the input box
    /// (called once at the I/O boundary in `main.rs`, like [`set_clock`]):
    /// the backend's model name and the display-ready working directory.
    ///
    /// [`set_clock`]: App::set_clock
    pub fn set_session_info(&mut self, model: impl Into<String>, cwd: impl Into<String>) {
        self.session = Some(SessionInfo {
            model: model.into(),
            cwd: cwd.into(),
        });
    }

    /// Inject the active model's reasoning capability + mode (a `/model`
    /// switch, the startup seed, or the boundary's support probe) — `None`
    /// for a model with no reasoning, which also blanks the footer's mode and
    /// makes Shift+Tab explain instead of cycle. See `docs/reasoning.md`.
    pub fn set_thinking(&mut self, thinking: Option<(ReasoningSupport, ThinkingMode)>) {
        self.thinking = thinking.map(|(support, mode)| ThinkingState { support, mode });
    }

    /// Shift+Tab: advance the thinking mode through the model's cycle and
    /// hand the loop the new mode ([`Action::SetThinking`]) — or, on a model
    /// with no reasoning, an explanatory transient toast.
    fn cycle_thinking(&mut self) -> Action {
        match self.thinking.as_mut() {
            Some(state) => {
                state.mode = state.support.next_mode(state.mode);
                Action::SetThinking(state.mode)
            }
            None => {
                let model = self
                    .session
                    .as_ref()
                    .map_or("This model", |s| s.model.as_str());
                Action::Toast(format!("{model} does not support thinking"))
            }
        }
    }

    /// Raise a transient [`Toast`] above the box (replacing any current one). The
    /// message self-clears after a few seconds — the *when* is timed at the I/O
    /// boundary (`main.rs`'s `toast_deadline`), so this only records *what* to
    /// show. Deliberately **not** pushed to [`history`](Self::history): a toast is
    /// UI, never conversation. See `docs/toast.md`.
    pub fn show_toast(&mut self, text: impl Into<String>, kind: ToastKind) {
        self.toast = Some(Toast {
            text: text.into(),
            kind,
        });
    }

    /// Drop the live toast (the boundary's expiry, and `/clear`).
    pub fn clear_toast(&mut self) {
        self.toast = None;
    }

    /// The transient toast currently shown above the box, if any — read by the
    /// renderer (`ui::toast_rows`/`toast_line`).
    #[must_use]
    pub const fn toast(&self) -> Option<&Toast> {
        self.toast.as_ref()
    }

    /// The current timestamp from the injected clock, or empty when none is set
    /// (the unit-test default — so equality tests on recorded items still hold).
    fn now_stamp(&self) -> String {
        self.clock.map_or_else(String::new, |clock| clock())
    }

    /// Record a message of `role` in the history, stamped with the injected
    /// clock — the one place a [`HistoryItem::Message`] is built, so every
    /// recording path (user echo, assistant text, notices, shell headers)
    /// stays in one shape.
    fn record_message(&mut self, role: Role, text: impl Into<String>) {
        self.record_message_with_images(role, text, Vec::new());
    }

    /// [`record_message`](Self::record_message) with image attachments — the
    /// submit path threads the turn's Ctrl+V temp-file paths onto the user
    /// message so later turns' context can re-send them (`docs/context.md`).
    fn record_message_with_images(
        &mut self,
        role: Role,
        text: impl Into<String>,
        images: Vec<PathBuf>,
    ) {
        let timestamp = self.now_stamp();
        self.history.push(HistoryItem::Message(Message {
            role,
            text: text.into(),
            timestamp,
            images,
        }));
    }

    /// Handle one key press and report what the event loop should do.
    ///
    /// Sending is disabled while a reply streams; quitting (Ctrl+C) and the
    /// tool-view toggle (Ctrl+O) always work, from either screen. Other keys are
    /// dispatched to the active [`View`].
    /// Handle a bracketed-paste event. A paste over
    /// [`crate::paste::LARGE_PASTE_CHAR_THRESHOLD`] characters is shown in the
    /// composer as a compact `[Pasted Content N chars]` placeholder, with the
    /// real text remembered in [`pasted`] for [`take_input`] to splice back in on
    /// send; a smaller paste is inserted verbatim, indistinguishable from typing
    /// it. The loop only calls this in the conversation view (the Ctrl+O overlay
    /// has no composer, like typing there). See `docs/paste.md`.
    ///
    /// [`pasted`]: App::pasted
    /// [`take_input`]: App::take_input
    pub fn on_paste(&mut self, pasted: &str) {
        // An open Ctrl+R search owns *every* key ([`on_key`] routes them all to
        // on_key_search) — a bracketed paste is input too, so it extends the
        // query (readline's paste-into-isearch) instead of editing the doomed
        // preview underneath (the next rerun/cancel set_texts over the
        // composer). Control characters would corrupt the single-row search
        // line, so they flatten to spaces. See `docs/history-search.md`.
        //
        // [`on_key`]: App::on_key
        if self.history_search.is_some() {
            let sanitised = pasted.replace(|c: char| c.is_control(), " ");
            self.edit_search_query(|query| query.push_str(&sanitised));
            return;
        }
        // Terminals such as iTerm2 send CR (or CRLF) for newlines in a paste;
        // normalise to LF so the char count and stored text match the display.
        let pasted = pasted.replace("\r\n", "\n").replace('\r', "\n");
        // An edit may change the active /command or @token, like the Char arm.
        let had_query = command_query(self.input.text()).is_some();
        let had_token = self.in_at_token();
        let char_count = pasted.chars().count();
        if char_count > crate::paste::LARGE_PASTE_CHAR_THRESHOLD {
            let placeholder = crate::paste::next_paste_placeholder(char_count, &self.pasted);
            self.input.insert_str(&placeholder);
            self.pasted.push((placeholder, pasted));
        } else {
            // Control characters other than '\n' (tabs above all) reach us only
            // via a paste, and they break the cursor math: unicode-width counts
            // '\t' as one column while ratatui renders zero cells, drifting the
            // hardware cursor right of the text. Sanitise what the composer will
            // actually render; the large-paste branch above keeps its payload
            // verbatim since only the placeholder is displayed.
            self.input
                .insert_str(&pasted.replace(|c: char| c.is_control() && c != '\n', " "));
        }
        self.refresh_command_menu(had_query);
        self.sync_shell_mode();
        self.refresh_file_search(had_token);
    }

    /// Take the composer draft, splicing every large-paste placeholder back to
    /// its real text and clearing [`pasted`] (the composer is now empty). The one
    /// choke point every send/queue path uses in place of a bare
    /// `self.input.take()`, so the model — and the committed record of what was
    /// sent — receives the real content, never a `[Pasted Content N chars]`
    /// placeholder. See `docs/paste.md`.
    ///
    /// [`pasted`]: App::pasted
    fn take_input(&mut self) -> String {
        let text = self.input.take();
        // Taking the draft consumes its attachments too (the idle submit and
        // queue paths stage image paths out first); whatever is still attached
        // here is a *drop* — record it so the boundary can remove the temp
        // file. Image placeholders are *not* expanded — they stay in `text`,
        // only the paths travel a separate channel. See `docs/image-paste.md`.
        self.discard_attachments();
        if self.pasted.is_empty() {
            return text;
        }
        let expanded = crate::paste::expand_pastes(&text, &self.pasted);
        self.pasted.clear();
        expanded
    }

    /// If the cursor is on an atomic placeholder — a large-paste
    /// `[Pasted Content N chars]` **or** a Ctrl+V `[Image #N]` — delete the
    /// **whole** placeholder atomically (one keystroke removes it, not one
    /// character) and drop its remembered text/path, returning `true`. Text
    /// pastes are tried first, then images; both are matched by string.
    /// `backward` is Backspace (vs Delete; see [`crate::paste::placeholder_to_delete`]
    /// for the cursor rules). Returns `false` when the cursor isn't on a
    /// placeholder, leaving the keypress to the normal per-grapheme edit. See
    /// `docs/paste.md` and `docs/image-paste.md`.
    fn delete_placeholder(&mut self, backward: bool) -> bool {
        if let Some(span) = crate::paste::placeholder_to_delete(
            self.input.text(),
            self.input.cursor(),
            &self.pasted,
            backward,
        ) {
            let ordinal = occurrence_ordinal(self.input.text(), &span);
            let placeholder = self.delete_span(span);
            remove_nth_pair(&mut self.pasted, &placeholder, ordinal);
            return true;
        }
        if let Some(span) = crate::paste::placeholder_to_delete(
            self.input.text(),
            self.input.cursor(),
            &self.images,
            backward,
        ) {
            let ordinal = occurrence_ordinal(self.input.text(), &span);
            let placeholder = self.delete_span(span);
            // Only the deleted occurrence's own pair goes (occurrences in
            // text order pair with list entries in order — the string-keyed
            // scheme's convention): a merged batch's duplicate-named pairs
            // keep backing their other occurrences. The dropped attachment's
            // temp file is orphaned now — hand its path to the boundary for
            // removal (docs/image-paste.md).
            if let Some(path) = remove_nth_pair(&mut self.images, &placeholder, ordinal) {
                self.discarded_images.push(path);
            }
            return true;
        }
        false
    }

    /// Splice the byte `span` out of the composer, returning the text it held —
    /// the shared half of [`delete_placeholder`] across the text/image lists.
    fn delete_span(&mut self, span: Range<usize>) -> String {
        let removed = self.input.text()[span.clone()].to_string();
        self.input.replace_range(span, "");
        removed
    }

    /// Attach a Ctrl+V-pasted image: insert an `[Image #N]` placeholder at the
    /// cursor and remember its temp-PNG `path` in [`images`] (codex's
    /// `attach_image`). The placeholder stays in the message text on send; the
    /// path is delivered separately (see [`take_submission_images`]). The loop
    /// calls this at the I/O boundary after [`crate::clipboard`] reads the
    /// image. See `docs/image-paste.md`.
    ///
    /// [`images`]: App::images
    /// [`take_submission_images`]: App::take_submission_images
    pub fn attach_image(&mut self, path: PathBuf) {
        // Like `on_paste`, an insert next to a `/command` or `@token` re-derives
        // the same menu/shell/file-search state every composer edit runs.
        let had_query = command_query(self.input.text()).is_some();
        let had_token = self.in_at_token();
        let placeholder = crate::paste::next_image_placeholder(&self.images);
        self.input.insert_str(&placeholder);
        self.images.push((placeholder, path));
        self.refresh_command_menu(had_query);
        self.sync_shell_mode();
        self.refresh_file_search(had_token);
    }

    /// Take the image attachments staged by the last idle submit (codex's
    /// `take_recent_submission_images`), as `(placeholder, path)` pairs: the
    /// loop threads them into `start_turn`, which records each path onto the
    /// message carrying its placeholder and hands the bare paths to
    /// [`crate::stream::ReplySource::spawn`]. See `docs/image-paste.md`.
    pub fn take_submission_images(&mut self) -> Vec<(String, PathBuf)> {
        std::mem::take(&mut self.submission_images)
    }

    /// Move every still-attached image into the discarded list — the shared
    /// tail of the drop paths (a `take_input` with nothing staged out first).
    fn discard_attachments(&mut self) {
        self.discarded_images
            .extend(self.images.drain(..).map(|(_, path)| path));
    }

    /// Drain the temp-PNG paths of attachments dropped since the last drain —
    /// the boundary deletes these files after handling each key event (the
    /// pure core records the drops, the file I/O stays in `main.rs`). See
    /// `docs/image-paste.md`.
    pub fn take_discarded_images(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.discarded_images)
    }

    /// Count `count` attached images into the live token tally as uploaded input
    /// (arrow ↑), like [`count_user_input`] does for the text, so the status
    /// reflects the images during the backend's pre-stream pause. No-op when no
    /// turn is in flight. See `docs/image-paste.md`.
    ///
    /// [`count_user_input`]: App::count_user_input
    pub fn count_input_images(&mut self, count: usize) {
        if let Some(status) = self.status.as_mut() {
            status.tokens += count * IMAGE_INPUT_TOKENS;
            status.arrow = TokenArrow::Up;
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        // An open Ctrl+R search owns *every* key (codex consumes them all in
        // handle_history_search_key) — including the global Ctrl+C/Ctrl+O
        // below, which it redefines: Ctrl+C cancels the search instead of
        // clearing the draft or quitting, and Ctrl+O cancels before the
        // overlay opens so no search state leaks into it.
        if self.view == View::Conversation && self.history_search.is_some() {
            return self.on_key_search(key);
        }
        // The inline `/model` picker likewise owns every key while it's open —
        // including the global Ctrl+C/Ctrl+O below, which it redefines (Ctrl+C
        // closes the picker instead of clearing the draft or quitting). It only
        // opens from the conversation view and blocks a turn from starting, so
        // this is the whole of its key handling. See `docs/llm.md`.
        if self.view == View::Conversation && self.model_picker.is_some() {
            return self.on_key_model_picker(key);
        }
        // The inline `/login` onboarding flow owns every key while open too, the
        // same way the `/model` picker does. See `docs/llm.md`.
        if self.view == View::Conversation && self.key_onboarding.is_some() {
            return self.on_key_key_onboarding(key);
        }
        // The ↓ background manager band owns every key while open, the same
        // way the pickers do. See `docs/background.md`.
        if self.view == View::Conversation && self.background_view.is_some() {
            return self.on_key_background(key);
        }
        // Ctrl+C: in the conversation, a first press with text in the input
        // clears the draft instead of quitting (codex's composer-clear step —
        // see docs/design.md); otherwise it quits, from either screen. The
        // overlay never shows the input box, so there is nothing to clear there.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            // The /resume picker closes on Ctrl+C — codex's from-a-session
            // picker "exit" leaves the picker, never the app (only its
            // startup picker quits). See docs/resume.md.
            if self.view == View::ResumePicker {
                self.close_resume_picker();
                return Action::CloseResumePicker;
            }
            if self.view == View::Conversation && !self.input.is_empty() {
                // Record the cleared draft so ↑ can bring it back (codex's
                // clear_for_ctrl_c does the same). A shell-mode draft re-gains
                // its `!` so the recall re-enters the mode. Recorded
                // *ephemerally*: a cleared, abandoned draft recalls this session
                // but is not persisted (codex keeps cleared drafts in
                // local_history only — docs/history-persistence.md).
                let mut text = self.take_input();
                if self.shell_mode {
                    self.shell_mode = false;
                    text = format!("!{text}");
                }
                self.input_history.record_ephemeral(&text);
                self.command_menu = None; // an emptied input can't be a /token
                self.file_search = None; // …nor an @token, so close the picker too
                return Action::None;
            }
            return Action::Quit;
        }
        // Ctrl+O toggles the full-screen tool-output view from either screen —
        // even mid-stream, so the conversation keeps updating underneath it.
        // (Not from the /resume picker or the Ctrl+D view: the full-screen
        // views share the alternate screen, so they never stack.)
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
            if matches!(self.view, View::ResumePicker | View::ContextDebug) {
                return Action::None;
            }
            self.toggle_tool_view();
            return Action::ToggleToolView;
        }
        // Ctrl+D toggles the full-screen context-debug view — the raw LLM
        // context window — with the same rules as Ctrl+O: works mid-stream,
        // inert under the other full-screen views. See docs/context.md.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('d') {
            if matches!(self.view, View::ResumePicker | View::ToolOutput) {
                return Action::None;
            }
            self.toggle_context_debug();
            return Action::ToggleContextDebug;
        }
        match self.view {
            View::Conversation => self.on_key_conversation(key),
            View::ToolOutput => self.on_key_tool_view(key),
            View::ResumePicker => self.on_key_resume_picker(key),
            View::ContextDebug => self.on_key_context_debug(key),
        }
    }

    /// Keys while the inline conversation is showing.
    ///
    /// When the slash-command palette is open it intercepts the navigation/select
    /// keys (↑/↓ move, Tab/Enter run, Esc dismisses); typing still edits the input
    /// (which filters the palette). With no palette open every key behaves as it
    /// always has.
    fn on_key_conversation(&mut self, key: KeyEvent) -> Action {
        // Any non-Esc key disarms a primed backtrack — codex resets its
        // priming on any other keypress, no timeout (docs/backtrack.md). The
        // key still does its normal job below.
        if self.backtrack.primed && key.code != KeyCode::Esc {
            self.backtrack.primed = false;
        }
        // The `?` shortcuts band (docs/shortcuts.md): `?` from an *empty*
        // composer toggles it (SHIFT allowed — terminals differ in reporting
        // Shift+/; with a draft `?` falls through and types). Any other key
        // closes an open band first and then acts normally (codex's
        // reset-after-activity) — except Esc, which only dismisses, since our
        // idle Esc would otherwise quit (the palette's Esc rule).
        let shortcuts_toggle = key.code == KeyCode::Char('?')
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && self.input.is_empty()
            // In shell mode `?` is a shell character (e.g. a glob), not the band.
            && !self.shell_mode;
        if shortcuts_toggle {
            self.shortcuts_open = !self.shortcuts_open;
            return Action::None;
        }
        if self.shortcuts_open {
            self.shortcuts_open = false;
            if key.code == KeyCode::Esc {
                return Action::None;
            }
        }
        let menu_open = self.command_menu.is_some();
        // The `@` file picker (docs/file-search.md): when its band is open it
        // intercepts the same navigation/select keys as the palette (they are
        // mutually exclusive — a bare `/token` has no whitespace, so an `@` in
        // it isn't at a token boundary).
        let file_open = self.file_search.is_some();
        match key.code {
            // Esc dismisses the palette when it's open (codex's "popup wins"
            // rule — even mid-turn); else it interrupts an in-flight turn
            // (codex-style, see docs/interrupt.md); else it quits as before.
            KeyCode::Esc if menu_open => {
                self.command_menu = None;
                Action::None
            }
            // Esc likewise dismisses the file picker (sticky within the token —
            // see refresh_file_search), before the interrupt/quit fallbacks.
            KeyCode::Esc if file_open => {
                self.file_search = None;
                Action::None
            }
            // Esc on an empty shell-mode composer exits the mode (codex's
            // bash-mode escape) — before the interrupt/quit fallbacks, like the
            // palette dismissal. With a draft, Esc keeps its normal meaning.
            KeyCode::Esc if self.shell_mode && self.input.is_empty() => {
                self.shell_mode = false;
                Action::None
            }
            KeyCode::Esc if self.turn_active() => Action::Interrupt,
            // Esc-Esc backtrack (docs/backtrack.md): a primed second Esc opens
            // the transcript overlay previewing the newest user message; the
            // first Esc primes when the composer is empty and a previous user
            // message exists. Only with *nothing* to backtrack to does idle
            // Esc keep its historical meaning — quit.
            KeyCode::Esc if self.backtrack.primed && self.input.is_empty() => {
                self.open_backtrack_preview();
                Action::ToggleToolView
            }
            KeyCode::Esc if self.input.is_empty() && self.has_backtrack_target() => {
                self.backtrack.primed = true;
                Action::None
            }
            // Esc with a typed draft is a no-op, like codex (its composer
            // only acts on Esc when empty): never a quit that throws typed
            // work away — Ctrl+C is the composer-clear, Ctrl+C/`/quit` the
            // exits. Quit below needs an *empty* composer with no target.
            KeyCode::Esc if !self.input.is_empty() => Action::None,
            KeyCode::Esc => Action::Quit,
            // Palette navigation / selection (only while it's open).
            KeyCode::Up if menu_open => {
                self.move_command_selection(-1);
                Action::None
            }
            KeyCode::Down if menu_open => {
                self.move_command_selection(1);
                Action::None
            }
            // File-picker navigation (only while it's open and has matches).
            KeyCode::Up if file_open => {
                self.move_file_selection(-1);
                Action::None
            }
            KeyCode::Down if file_open => {
                self.move_file_selection(1);
                Action::None
            }
            // Shift+Tab cycles the thinking mode (docs/reasoning.md). Legacy
            // terminals report it as BackTab (`ESC[Z`), the kitty protocol can
            // report Tab+SHIFT — both bind (the Shift+Enter pattern), and the
            // Tab+SHIFT arm sits before every plain-Tab arm so a shifted Tab
            // never queues or completes. Like /model, cycling never touches a
            // running turn — the mode rides the *next* request.
            KeyCode::BackTab => self.cycle_thinking(),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => self.cycle_thinking(),
            KeyCode::Tab if menu_open => self.run_selected_command(),
            // Tab/Enter accept the highlighted file when the picker is open and
            // a match is selected (codex's accept) — replacing the `@token` with
            // the path. With no matches they fall through to the normal Tab/Enter
            // (queue / submit), so an unmatched `@query` is still sendable text.
            KeyCode::Tab if file_open && self.highlighted_file().is_some() => {
                self.accept_file_selection()
            }
            // Tab while a turn streams queues the draft as a *new* follow-up
            // batch — a separate turn that runs after the batches already queued,
            // instead of merging into the current one like Enter (codex's
            // Tab-to-queue). Empty/idle Tab falls through to a no-op.
            KeyCode::Tab if self.is_streaming() && !self.input.text().trim().is_empty() => {
                self.queue_draft(/*new_batch*/ true);
                Action::None
            }
            KeyCode::Enter if menu_open => {
                if self.highlighted_command().is_some() {
                    self.run_selected_command()
                } else {
                    // A query that matches nothing isn't a message — swallow Enter
                    // rather than submitting the literal "/typo".
                    Action::None
                }
            }
            KeyCode::Enter if file_open && self.highlighted_file().is_some() => {
                self.accept_file_selection()
            }
            // Alt+Enter and Shift+Enter insert a newline at the cursor so the input
            // box grows on demand; a plain Enter submits. Shift+Enter only reaches
            // us when keyboard enhancement is on (pushed by `term::init`); Ctrl+J
            // below is the universal fallback. See docs/shift-enter.md.
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.input.insert_newline();
                Action::None
            }
            KeyCode::Enter => {
                if self.shell_mode && self.input.text().trim().is_empty() {
                    // A bare `!` runs nothing — post the help notice and stay
                    // in the mode (codex's empty-bang help).
                    return Action::Notice(SHELL_EMPTY_NOTICE.to_string());
                }
                if self.input.text().trim().is_empty() {
                    Action::None
                } else if self.is_streaming() {
                    // A turn is in flight — queue the draft for a later turn
                    // instead of dropping it (codex's queued_user_messages).
                    // Enter appends to the batch being accumulated, so
                    // consecutive Enters batch into one next turn; Tab instead
                    // opens a new follow-up batch (see the Tab arm above).
                    self.queue_draft(/*new_batch*/ false);
                    Action::None
                } else if self.shell_mode {
                    // Shell mode: run the draft locally (docs/shell-command.md).
                    // Record the full `!command` for ↑ recall (codex records the
                    // whole text — recall re-absorbs the bang).
                    let raw = self.take_input();
                    self.shell_mode = false;
                    self.file_search = None;
                    self.input_history.record(&format!("!{raw}"));
                    Action::RunShell(raw.trim().to_string())
                } else {
                    // Stage any Ctrl+V-attached images for the boundary to
                    // deliver alongside the text, *before* take_input clears the
                    // composer (the placeholder text stays; the pairs travel
                    // the side channel — docs/image-paste.md).
                    self.submission_images = std::mem::take(&mut self.images);
                    let text = self.take_input();
                    self.file_search = None;
                    self.input_history.record(&text);
                    Action::Submit(text)
                }
            }
            // Ctrl+J is the *universal* newline key: in raw mode every terminal
            // delivers it as Char('j')+CONTROL (no keyboard enhancement needed), so
            // it inserts a newline like Alt/Shift+Enter even where the terminal
            // can't report a modified Enter (codex binds Ctrl+J the same way; see
            // docs/shift-enter.md).
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.insert_newline();
                Action::None
            }
            // Ctrl+V (and Ctrl+Alt+V — the WSL-friendly alias codex also binds)
            // pastes an image from the system clipboard. The decision is pure; the
            // loop performs the clipboard I/O (`crate::clipboard`) and calls
            // `attach_image` on success. See docs/image-paste.md.
            KeyCode::Char(c)
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && c.eq_ignore_ascii_case(&'v') =>
            {
                Action::PasteImage
            }
            // Editing and cursor movement, dispatched to the textarea. Backspace /
            // Delete / typing also re-derive the slash-command palette.
            // Backspace on an empty shell-mode composer deletes the absorbed
            // `!` — i.e. exits the mode (the natural inverse of typing it).
            KeyCode::Backspace if self.shell_mode && self.input.is_empty() => {
                self.shell_mode = false;
                Action::None
            }
            KeyCode::Backspace => {
                let had_query = command_query(self.input.text()).is_some();
                let had_token = self.in_at_token();
                // A Backspace on a large-paste placeholder removes the whole
                // placeholder atomically (docs/paste.md); otherwise one grapheme.
                if !self.delete_placeholder(/*backward*/ true) {
                    self.input.delete_backward();
                }
                self.refresh_command_menu(had_query);
                self.sync_shell_mode();
                self.refresh_file_search(had_token);
                Action::None
            }
            KeyCode::Delete => {
                let had_query = command_query(self.input.text()).is_some();
                let had_token = self.in_at_token();
                if !self.delete_placeholder(/*backward*/ false) {
                    self.input.delete_forward();
                }
                self.refresh_command_menu(had_query);
                self.sync_shell_mode();
                self.refresh_file_search(had_token);
                Action::None
            }
            KeyCode::Left => {
                self.input.move_left();
                Action::None
            }
            KeyCode::Right => {
                self.input.move_right();
                Action::None
            }
            // Alt+Up pulls the *last* queued batch back into an *empty* composer
            // as one newline-joined draft (its own messages oldest first) to
            // edit, extend, or drop — codex's edit_queued_message pops the most
            // recent entry, leaving the earlier batches queued. Guarded on an
            // empty composer so it never clobbers a draft (the composer is empty
            // in the normal flow — Enter/Tab emptied it on queue).
            KeyCode::Up
                if key.modifiers.contains(KeyModifiers::ALT)
                    && self.input.is_empty()
                    && !self.queued.is_empty() =>
            {
                match self.drain_last_batch() {
                    // A text batch returns newline-joined (oldest first), its
                    // image attachments re-attached so the placeholders in the
                    // restored draft are backed again (docs/image-paste.md).
                    Some(QueuedTurn::Messages { texts, images }) => {
                        self.recall_input(&texts.join("\n"));
                        self.images = images;
                    }
                    // A shell entry re-enters shell mode: recalling `!command`
                    // re-absorbs the bang (sync_shell_mode), so the composer
                    // shows the red `! command` prompt again, ready to edit/re-run.
                    Some(QueuedTurn::Shell(cmd)) => self.recall_input(&format!("!{cmd}")),
                    None => {}
                }
                Action::None
            }
            // ↑/↓ first try shell-style history recall — only from an empty
            // composer or an unedited recall (docs/input-history.md) — then
            // fall back to cursor movement. The menu-open arms above still
            // take precedence (codex's "popups win").
            KeyCode::Up => {
                if self.should_browse_history()
                    && let Some(text) = self.input_history.up()
                {
                    self.recall_input(&text);
                    return Action::None;
                }
                self.input.move_up();
                Action::None
            }
            KeyCode::Down => {
                if self.should_browse_history()
                    && let Some(text) = self.input_history.down()
                {
                    self.recall_input(&text);
                    return Action::None;
                }
                // ↓ from an empty composer opens the background manager once
                // any shell has run (`docs/background.md`) — history recall
                // was tried first, so a mid-recall ↓ still steps the history.
                if self.background_openable() {
                    self.open_background_view();
                    return Action::None;
                }
                self.input.move_down();
                Action::None
            }
            // Ctrl+B moves the running command (a model `bash` call or a `!`
            // shell turn) to the background — the loop raises the registry
            // request the runner's poll loop consumes. A no-op when nothing
            // backgroundable is running. See `docs/background.md`.
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.can_move_to_background() {
                    Action::MoveToBackground
                } else {
                    Action::None
                }
            }
            KeyCode::Home => {
                self.input.move_home();
                Action::None
            }
            KeyCode::End => {
                self.input.move_end();
                Action::None
            }
            // Ctrl+R opens the reverse history search (docs/history-search.md);
            // once open, every key routes to on_key_search instead, where
            // Ctrl+R steps to older matches.
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.begin_history_search();
                Action::None
            }
            // Plain (and Shift-modified) characters insert at the cursor; ALT/CONTROL
            // combos are not text, so they are ignored here. An insert that
            // leaves the text starting with `!` is absorbed into shell mode
            // (sync_shell_mode — codex's bash-mode sync after every edit).
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let had_query = command_query(self.input.text()).is_some();
                let had_token = self.in_at_token();
                self.input.insert_char(c);
                self.refresh_command_menu(had_query);
                self.sync_shell_mode();
                self.refresh_file_search(had_token);
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Should this ↑/↓ press browse [`input_history`] instead of moving the
    /// cursor?
    ///
    /// [`input_history`]: App::input_history
    fn should_browse_history(&self) -> bool {
        let text = self.input.text();
        // A recalled `!command`'s bang lives in [`shell_mode`], not the text
        // (sync_shell_mode absorbed it), while the recorded entry keeps it —
        // reconstruct the `!`-prefixed form for the unedited-recall
        // comparison, mapping the composer's ends onto the recorded ends, or
        // browsing strands permanently on a shell entry. An empty shell
        // composer browses like any empty composer.
        //
        // [`shell_mode`]: App::shell_mode
        if self.shell_mode && !text.is_empty() {
            let recorded = format!("!{text}");
            let cursor = self.input.cursor();
            let cursor = if cursor == 0 { 0 } else { cursor + 1 };
            return self.input_history.should_navigate(&recorded, cursor);
        }
        self.input_history
            .should_navigate(text, self.input.cursor())
    }

    /// Replace the draft with a recalled history entry. `set_text` puts the
    /// cursor at the end (codex's recall placement), and the palette is
    /// re-derived so recalling a bare `/token` reopens it like typing one. The
    /// shell mode is re-derived from scratch — a recalled `!command` re-enters
    /// it (codex re-absorbs the bang), a plain entry leaves it.
    fn recall_input(&mut self, text: &str) {
        self.shell_mode = false;
        // A recalled draft never pops the `@` picker (recall isn't typing an
        // `@token`); close any open one, like the search snapshot restore.
        self.file_search = None;
        let had_query = command_query(self.input.text()).is_some();
        self.input.set_text(text);
        self.refresh_command_menu(had_query);
        self.sync_shell_mode();
    }

    /// Absorb a leading `!` out of the textarea into [`shell_mode`] — codex's
    /// `sync_bash_mode_from_text`, run after every edit that could produce one:
    /// the bang lives in the flag (rendered as the `! ` prompt), never in the
    /// text. Only ever *enters* the mode; leaving it is an explicit gesture
    /// (Backspace/Esc on empty, or submitting). No-op when already in the mode
    /// or when the text doesn't start with `!`.
    ///
    /// [`shell_mode`]: App::shell_mode
    fn sync_shell_mode(&mut self) {
        if self.shell_mode {
            return;
        }
        if let Some(rest) = shell_query(self.input.text()) {
            let rest = rest.to_string();
            self.shell_mode = true;
            // Only the 1-byte bang leaves the text — keep the cursor where the
            // user had it (shifted back past the removed prefix) instead of
            // letting a plain set_text teleport it to the end.
            let cursor = self.input.cursor().saturating_sub(1);
            self.input.set_text_with_cursor(&rest, cursor);
            self.command_menu = None; // the draft is a literal command now
        }
    }

    /// Re-derive the palette after an edit. Opens it when the input *becomes* a
    /// command token, clamps the highlight when the filter narrows, and closes it
    /// when the input stops being a command token. The `had_query` flag (the state
    /// *before* the edit) makes Esc sticky: once dismissed, editing within the same
    /// token won't reopen the palette — only leaving and re-entering command mode
    /// (a None→Some transition) does.
    fn refresh_command_menu(&mut self, had_query: bool) {
        // In shell mode the whole draft is a literal command — a leading `/`
        // there (e.g. `!/usr/bin/env`) is a path, never the palette.
        if self.shell_mode {
            self.command_menu = None;
            return;
        }
        match command_query(self.input.text()) {
            None => self.command_menu = None,
            Some(query) => {
                let matches = matching_commands(query).len();
                match &mut self.command_menu {
                    Some(menu) => menu.selected = menu.selected.min(matches.saturating_sub(1)),
                    // Just entered command mode → open at the top.
                    None if !had_query => self.command_menu = Some(CommandMenu { selected: 0 }),
                    // Dismissed earlier and still in the same token → stay closed.
                    None => {}
                }
            }
        }
    }

    /// Move the palette highlight by `delta`, clamped to the current matches.
    fn move_command_selection(&mut self, delta: isize) {
        let Some(query) = command_query(self.input.text()) else {
            return;
        };
        let matches = matching_commands(query).len();
        if let Some(menu) = &mut self.command_menu {
            let last = matches.saturating_sub(1) as isize;
            menu.selected = (menu.selected as isize + delta).clamp(0, last) as usize;
        }
    }

    /// The command currently highlighted in the palette, if one is (the palette is
    /// open and the query matches at least one command).
    #[must_use]
    pub fn highlighted_command(&self) -> Option<&'static SlashCommand> {
        let menu = self.command_menu.as_ref()?;
        let query = command_query(self.input.text())?;
        matching_commands(query).get(menu.selected).copied()
    }

    /// Run the highlighted command: consume the input, close the palette, and
    /// dispatch its effect as an [`Action`] for the loop. `Action::None` if no
    /// command is highlighted (an empty query).
    fn run_selected_command(&mut self) -> Action {
        let Some(cmd) = self.highlighted_command() else {
            return Action::None;
        };
        let effect = cmd.effect;
        self.input.clear();
        self.command_menu = None;
        match effect {
            CommandEffect::Clear => {
                self.clear_conversation();
                Action::Clear
            }
            CommandEffect::Help => {
                // Idle, /help commits its multi-line command list. Mid-turn that
                // list would interleave with the streaming reply, so it's
                // rejected with a transient toast instead. See docs/toast.md.
                if self.turn_active() {
                    Action::Toast(HELP_BUSY_NOTICE.to_string())
                } else {
                    Action::Notice(help_text())
                }
            }
            CommandEffect::Copy => Action::Copy(self.last_assistant_text()),
            CommandEffect::Init => {
                // Codex's /init is submit_user_message(INIT_PROMPT): the canned
                // prompt rides the normal Submit → start_turn path — echoed as
                // the user ❯ message, recorded, checkpointed — and the model's
                // tool loop generates AGENTS.md. Trimmed because the file's
                // final newline would wrap into an empty last line that
                // message_lines pads into a stray full-width dark row under
                // the ❯ cell (codex trims the same way at render time). Mid-turn
                // it is disabled like /compact (codex's available_during_task =
                // false); the ↑ recall history is untouched (the user typed
                // "/init", not the prompt). See docs/init.md.
                if self.turn_active() {
                    Action::Toast(INIT_BUSY_NOTICE.to_string())
                } else {
                    Action::Submit(INIT_PROMPT.trim_end().to_string())
                }
            }
            CommandEffect::Compact => {
                // Codex disables /compact while a task runs (the summarize
                // request would race the stream over the same history); the
                // rejection is a transient toast like /resume's. An empty
                // derived context has nothing to summarize. See
                // docs/compact.md / docs/toast.md.
                if self.turn_active() {
                    Action::Toast(COMPACT_BUSY_NOTICE.to_string())
                } else if crate::context::context_messages(&self.history).is_empty() {
                    Action::Toast(COMPACT_EMPTY_NOTICE.to_string())
                } else {
                    Action::Compact
                }
            }
            CommandEffect::Resume => {
                // Codex blocks /resume while a task runs (it swaps the whole
                // conversation, racing the stream); the rejection is a transient
                // toast. Idle, the *loop* scans the sessions dir and opens the
                // picker. See docs/resume.md / docs/toast.md.
                if self.turn_active() {
                    Action::Toast(RESUME_BUSY_NOTICE.to_string())
                } else {
                    Action::OpenResumePicker
                }
            }
            CommandEffect::Model => {
                // /model works mid-turn: it only replaces the composer with the
                // inline picker, never the running turn (which streams on its own
                // thread). Selecting rebinds only the *next* turn's backend. The
                // *loop* fetches the model list. See docs/llm.md / docs/toast.md.
                Action::OpenModelPicker
            }
            CommandEffect::Login => {
                // /login works mid-turn like /model — saving a key never touches
                // the running turn. The *loop* builds the provider choices and
                // opens the onboarding inline. See docs/llm.md / docs/toast.md.
                Action::OpenKeyOnboarding
            }
            CommandEffect::Quit => Action::Quit,
        }
    }

    /// The text of the last assistant message in [`history`](Self::history), if
    /// any non-empty one exists — what `/copy` writes to the clipboard. Our turn
    /// model splits assistant prose around tool calls, so this is the last
    /// recorded assistant *segment*, mirroring codex's `last_agent_markdown`
    /// (likewise the latest agent message). The in-progress streaming buffer is
    /// not considered — only finished history. `None` (the nothing-to-copy path)
    /// when there's no assistant message, or the latest one is empty. See
    /// `docs/copy.md`.
    #[must_use]
    pub fn last_assistant_text(&self) -> Option<String> {
        let latest = self.history.iter().rev().find_map(|item| match item {
            HistoryItem::Message(m) if m.role == Role::Assistant => Some(&m.text),
            _ => None,
        })?;
        (!latest.is_empty()).then(|| latest.clone())
    }

    /// Is the cursor currently inside an `@token`? (The `had_token` state the
    /// editing arms snapshot *before* a change so [`refresh_file_search`]'s
    /// Esc-sticky logic mirrors the palette's `had_query`.)
    ///
    /// [`refresh_file_search`]: App::refresh_file_search
    fn in_at_token(&self) -> bool {
        at_token(self.input.text(), self.input.cursor()).is_some()
    }

    /// Re-derive the `@` file picker after an edit — the palette's
    /// [`refresh_command_menu`] logic, applied to [`at_token`]. Opens it when the
    /// cursor *enters* an `@token` (the None→Some transition, so an Esc-dismiss
    /// stays dismissed within the same token), updates the query as it changes
    /// (marking it `waiting` and resetting the highlight so the boundary's next
    /// results refresh the band), and closes it when the token's gone. Suppressed
    /// in shell mode (a `!command` draft is a literal command).
    ///
    /// [`refresh_command_menu`]: App::refresh_command_menu
    fn refresh_file_search(&mut self, had_token: bool) {
        if self.shell_mode {
            self.file_search = None;
            return;
        }
        match at_token(self.input.text(), self.input.cursor()) {
            None => self.file_search = None,
            Some(tok) => match &mut self.file_search {
                Some(fs) if fs.query != tok.query => {
                    fs.query = tok.query;
                    fs.waiting = true;
                    fs.selected = 0;
                }
                Some(_) => {}
                // Just entered an `@token` → open at the top.
                None if !had_token => {
                    self.file_search = Some(FileSearch {
                        selected: 0,
                        query: tok.query,
                        matches: Vec::new(),
                        waiting: true,
                    });
                }
                // Dismissed earlier and still in the same token → stay closed.
                None => {}
            },
        }
    }

    /// The active `@token` query the boundary should search for — `None` when the
    /// picker is closed (or in shell mode). The loop dispatches a file search
    /// whenever this changes; see `docs/file-search.md`.
    #[must_use]
    pub fn file_search_query(&self) -> Option<String> {
        if self.file_search.is_none() || self.shell_mode {
            return None;
        }
        at_token(self.input.text(), self.input.cursor()).map(|t| t.query)
    }

    /// Feed asynchronously-fetched file matches into the open picker — codex's
    /// `on_file_search_result`. Stale results (the token moved on since the
    /// search was dispatched) are dropped; otherwise they replace the band's
    /// matches, clear the `waiting` flag, and clamp the highlight.
    pub fn set_file_matches(&mut self, query: &str, matches: Vec<FileMatch>) {
        // Compare against the *live* token so a result that raced the user's
        // typing is discarded (the loop also re-dispatches on every change).
        let active = at_token(self.input.text(), self.input.cursor()).map(|t| t.query);
        if active.as_deref() != Some(query) {
            return;
        }
        if let Some(fs) = &mut self.file_search {
            fs.selected = fs.selected.min(matches.len().saturating_sub(1));
            fs.matches = matches;
            fs.query = query.to_string();
            fs.waiting = false;
        }
    }

    /// Move the file-picker highlight by `delta`, clamped to the current matches.
    fn move_file_selection(&mut self, delta: isize) {
        if let Some(fs) = &mut self.file_search {
            let len = fs.matches.len();
            if len == 0 {
                return;
            }
            let last = (len - 1) as isize;
            fs.selected = (fs.selected as isize + delta).clamp(0, last) as usize;
        }
    }

    /// The file match currently highlighted in the picker, if one is.
    #[must_use]
    pub fn highlighted_file(&self) -> Option<&FileMatch> {
        let fs = self.file_search.as_ref()?;
        fs.matches.get(fs.selected)
    }

    /// Accept the highlighted file: replace the `@token` under the cursor with the
    /// path plus a trailing space (codex's `insert_selected_path`; paths with
    /// whitespace are quoted), and close the picker. `Action::None` if nothing is
    /// highlighted.
    fn accept_file_selection(&mut self) -> Action {
        let Some(path) = self.highlighted_file().map(|m| m.path.clone()) else {
            return Action::None;
        };
        if let Some(tok) = at_token(self.input.text(), self.input.cursor()) {
            let inserted = if path.chars().any(char::is_whitespace) && !path.contains('"') {
                format!("\"{path}\"")
            } else {
                path
            };
            self.input.replace_range(tok.range, &format!("{inserted} "));
        }
        self.file_search = None;
        Action::None
    }

    /// Open the Ctrl+R reverse history search: snapshot the draft — text and
    /// cursor (codex's `snapshot_draft`) — close the palette (the search owns
    /// the keys from here), and start Idle with an empty query: no preview
    /// until something is typed. See `docs/history-search.md`.
    fn begin_history_search(&mut self) {
        self.command_menu = None;
        self.file_search = None; // the search owns the keys from here
        self.history_search = Some(HistorySearch {
            snapshot: self.input.clone(),
            snapshot_shell: self.shell_mode,
            query: String::new(),
            state: SearchState::Idle,
        });
        // Suspend shell mode while the search owns the composer: previewed
        // entries are raw text (`❯ !cmd`), not shell-mode drafts. Cancel
        // restores the flag with the snapshot; accept re-derives it.
        self.shell_mode = false;
    }

    /// Every key while the search is open — codex's
    /// `handle_history_search_key`: all of them are consumed here, so the
    /// normal composer handling (and the global Ctrl+C/Ctrl+O arms) never see
    /// a keystroke mid-search.
    fn on_key_search(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // The global overlay toggle still works, but the search cancels first
        // so its preview never leaks into (or lingers under) the overlay.
        if ctrl && key.code == KeyCode::Char('o') {
            self.cancel_history_search();
            self.toggle_tool_view();
            return Action::ToggleToolView;
        }
        // Ctrl+R/↑ step to an older match, Ctrl+S/↓ back to a newer one.
        if (ctrl && key.code == KeyCode::Char('r')) || key.code == KeyCode::Up {
            self.step_history_search(true);
            return Action::None;
        }
        if (ctrl && key.code == KeyCode::Char('s')) || key.code == KeyCode::Down {
            self.step_history_search(false);
            return Action::None;
        }
        match key.code {
            // Esc and Ctrl+C cancel, restoring the snapshotted draft: Ctrl+C
            // neither clears it nor quits, and Esc never reaches the
            // interrupt/quit arms (the palette-dismiss precedent).
            KeyCode::Esc => {
                self.cancel_history_search();
                Action::None
            }
            KeyCode::Char('c') if ctrl => {
                self.cancel_history_search();
                Action::None
            }
            KeyCode::Enter => {
                self.accept_history_search();
                Action::None
            }
            // Backspace (or Ctrl+H, its classic alias) pops the query; Ctrl+U
            // clears it; plain characters extend it. Every edit restarts the
            // search from the newest entry.
            KeyCode::Backspace => self.edit_search_query(|query| {
                query.pop();
            }),
            KeyCode::Char('h') if ctrl => self.edit_search_query(|query| {
                query.pop();
            }),
            KeyCode::Char('u') if ctrl => self.edit_search_query(String::clear),
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.edit_search_query(|query| query.push(c))
            }
            // Everything else is swallowed (codex consumes unknown keys too).
            _ => Action::None,
        }
    }

    /// Apply `edit` to the query and re-run the search from the newest entry
    /// (codex's `update_history_search_query` — every query edit restarts).
    fn edit_search_query(&mut self, edit: impl FnOnce(&mut String)) -> Action {
        if let Some(search) = self.history_search.as_mut() {
            edit(&mut search.query);
            self.rerun_history_search();
        }
        Action::None
    }

    /// Re-run the search after a query edit, restarting at the newest match.
    /// An empty query goes back to Idle with the original draft showing.
    fn rerun_history_search(&mut self) {
        let Some(search) = self.history_search.as_ref() else {
            return;
        };
        if search.query.is_empty() {
            self.restore_search_snapshot();
            if let Some(search) = self.history_search.as_mut() {
                search.state = SearchState::Idle;
            }
            return;
        }
        let matches = self.input_history.search(&search.query);
        self.show_search_match(&matches, 0);
    }

    /// Step the previewed match older/newer, clamping at both ends so the
    /// current match is kept (codex's `AtBoundary` — never a "no match"
    /// flicker at the end of the list). Stepping with an empty query stays
    /// Idle: opening Ctrl+R never previews the latest entry by itself.
    fn step_history_search(&mut self, older: bool) {
        let Some(search) = self.history_search.as_ref() else {
            return;
        };
        if search.query.is_empty() {
            return;
        }
        let matches = self.input_history.search(&search.query);
        let selected = match search.state {
            SearchState::Match { selected } if older => {
                (selected + 1).min(matches.len().saturating_sub(1))
            }
            SearchState::Match { selected } => selected.saturating_sub(1),
            // NoMatch: the matches are still empty (the query hasn't changed
            // since they came up empty), so this re-shows NoMatch below.
            _ => 0,
        };
        self.show_search_match(&matches, selected);
    }

    /// Preview match `selected` of `matches` (entry indices, newest first) in
    /// the composer — or show NoMatch when there is none: the original draft
    /// comes back but the search stays open for more typing (codex's
    /// `apply_history_search_result`).
    fn show_search_match(&mut self, matches: &[usize], selected: usize) {
        let entry = matches
            .get(selected)
            .and_then(|&index| self.input_history.entry(index))
            .map(str::to_string);
        match entry {
            Some(text) => {
                // Cursor at the end — codex's recall placement.
                self.input.set_text(&text);
                if let Some(search) = self.history_search.as_mut() {
                    search.state = SearchState::Match { selected };
                }
            }
            None => {
                self.restore_search_snapshot();
                if let Some(search) = self.history_search.as_mut() {
                    search.state = SearchState::NoMatch;
                }
            }
        }
    }

    /// Cancel the search, restoring the draft — text *and* cursor — from
    /// before it opened (codex's `cancel_history_search` → `restore_draft`).
    fn cancel_history_search(&mut self) {
        if let Some(search) = self.history_search.take() {
            self.input = search.snapshot;
            self.shell_mode = search.snapshot_shell;
            self.reconcile_images_with_input();
        }
    }

    /// Accept the previewed match (Enter): the search closes, the text stays
    /// as an ordinary editable draft (cursor already at the end), ↑/↓
    /// browsing is seated at the accepted entry — so ↑ continues *older* from
    /// it, codex's shared history cursor — and the palette re-derives like
    /// ↑-recall, so accepting a bare `/token` reopens it. Enter on
    /// Idle/NoMatch is swallowed: only an actual match accepts.
    fn accept_history_search(&mut self) {
        let Some(search) = self.history_search.as_ref() else {
            return;
        };
        let SearchState::Match { selected } = search.state else {
            return;
        };
        let entry = self
            .input_history
            .search(&search.query)
            .get(selected)
            .copied();
        let Some(entry) = entry else {
            return;
        };
        self.history_search = None;
        self.input_history.resume_at(entry);
        // The accepted entry replaced the pre-search draft, unanchoring the
        // pairs that backed it — discard them (the boundary deletes the
        // orphaned temp files) so a stale attachment can't silently ride the
        // next submission; any `[Image #N]` in the accepted text is an
        // unbacked marker (docs/image-paste.md). Mirrors the interrupt-undo
        // and backtrack draft-replacement paths.
        self.discard_attachments();
        self.refresh_command_menu(false);
        // An accepted `!entry` re-enters shell mode (the recall rule).
        self.sync_shell_mode();
    }

    /// Show the pre-search draft again without closing the search — what a
    /// no-match query (and a query cleared back to empty) displays.
    fn restore_search_snapshot(&mut self) {
        if let Some(search) = self.history_search.as_ref() {
            self.input = search.snapshot.clone();
            self.reconcile_images_with_input();
        }
    }

    /// Drop image pairs no longer anchored to a placeholder occurrence in the
    /// composer (keeping at most one pair per occurrence, in order) — the
    /// reconcile a search-snapshot restore needs when an async Ctrl+V
    /// completion landed while the Ctrl+R search owned the composer: its
    /// placeholder went with the preview text, so the pair must not linger
    /// invisibly and ride the next submission. Orphaned temp files go to the
    /// boundary's discard list (docs/image-paste.md).
    fn reconcile_images_with_input(&mut self) {
        let text = self.input.text().to_string();
        let mut kept: Vec<(String, PathBuf)> = Vec::new();
        for (placeholder, path) in std::mem::take(&mut self.images) {
            let occurrences = text.matches(placeholder.as_str()).count();
            let backed = kept.iter().filter(|(ph, _)| *ph == placeholder).count();
            if backed < occurrences {
                kept.push((placeholder, path));
            } else {
                self.discarded_images.push(path);
            }
        }
        self.images = kept;
    }

    /// Byte ranges of the query's occurrences in the composer text, **only
    /// while a match is previewed** — once the search closes the accepted text
    /// is an ordinary draft again, so this returns nothing (codex's
    /// `history_search_highlight_ranges`). `ui::render_live` styles these
    /// reversed+bold in the input box.
    #[must_use]
    pub fn search_highlight_ranges(&self) -> Vec<Range<usize>> {
        let Some(search) = self.history_search.as_ref() else {
            return Vec::new();
        };
        if !matches!(search.state, SearchState::Match { .. }) || search.query.is_empty() {
            return Vec::new();
        }
        case_insensitive_match_ranges(self.input.text(), &search.query)
    }

    /// Keys while the full-screen tool-output view is showing: it is a read-only
    /// scroller (codex's transcript pager), so typing is ignored; Esc and `q`
    /// (like Ctrl+O) return to the chat, Home/End jump to the transcript's edges.
    fn on_key_tool_view(&mut self, key: KeyEvent) -> Action {
        match key.code {
            // Backtrack preview keys (docs/backtrack.md): Esc/← step the
            // highlight to the next-older user message, → back toward the
            // newest, Enter confirms the rewind (truncate + prefill — the
            // view flips back inside, so the ToggleToolView lands on the
            // loop's normal return-from-overlay path). The scroll keys below
            // keep working; q (or Ctrl+O) still closes, cancelling.
            KeyCode::Esc | KeyCode::Left if self.backtrack.selected.is_some() => {
                self.step_backtrack(-1);
                Action::None
            }
            KeyCode::Right if self.backtrack.selected.is_some() => {
                self.step_backtrack(1);
                Action::None
            }
            KeyCode::Enter if self.backtrack.selected.is_some() => {
                self.confirm_backtrack();
                // A dedicated action (not ToggleToolView) so the loop resets
                // the code to this point's checkpoint before the shared
                // return-from-overlay repaint (docs/checkpoint.md).
                Action::ConfirmBacktrack
            }
            // Esc in a plain Ctrl+O view *begins* the preview in place when
            // idle with a target — codex's Ctrl+T → Esc path; without one
            // (or mid-turn) it keeps closing the overlay below.
            KeyCode::Esc if !self.turn_active() && self.has_backtrack_target() => {
                self.begin_backtrack_preview();
                Action::None
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.toggle_tool_view(); // close the overlay, back to chat
                Action::ToggleToolView
            }
            KeyCode::Up => {
                self.tool_follow = false; // reading back — stop tailing the bottom
                self.tool_scroll = self.tool_scroll.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                // Don't touch `tool_follow`: reaching the bottom re-engages it in
                // `settle_tool_scroll`.
                self.tool_scroll = self.tool_scroll.saturating_add(1);
                Action::None
            }
            KeyCode::PageUp => {
                self.tool_follow = false;
                self.tool_scroll = self.tool_scroll.saturating_sub(TOOL_VIEW_PAGE);
                Action::None
            }
            KeyCode::PageDown => {
                self.tool_scroll = self.tool_scroll.saturating_add(TOOL_VIEW_PAGE);
                Action::None
            }
            KeyCode::Home => {
                self.tool_follow = false;
                self.tool_scroll = 0;
                Action::None
            }
            KeyCode::End => {
                // Past any end — `settle_tool_scroll` pins it to the bottom
                // and re-engages tail-follow (codex's jump_bottom).
                self.tool_scroll = usize::MAX;
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Flip between the conversation and the tool-output view. Entering the view
    /// pins it to the bottom (tail-follow) so it opens on the latest content.
    /// Ctrl+O is "activity" like any other key, so an open `?` shortcuts band
    /// closes rather than re-showing after the round-trip (docs/shortcuts.md).
    fn toggle_tool_view(&mut self) {
        self.shortcuts_open = false;
        // Any toggle abandons an in-flight backtrack gesture — Ctrl+O/q close
        // a preview without truncating, and Ctrl+O over a primed composer is
        // "any other key" activity (codex resets its BacktrackState on
        // overlay close the same way; docs/backtrack.md).
        self.backtrack = Backtrack::default();
        self.view = match self.view {
            View::Conversation => View::ToolOutput,
            // The Ctrl+O guard in on_key keeps the picker and the Ctrl+D view
            // out of here; the arm is only exhaustiveness.
            View::ToolOutput | View::ResumePicker | View::ContextDebug => View::Conversation,
        };
        self.tool_scroll = 0;
        self.tool_follow = self.view == View::ToolOutput;
    }

    /// Keys while the Ctrl+D context-debug view is showing: the transcript
    /// pager's scroll set, with q/Esc (or Ctrl+D itself, handled globally)
    /// closing it. It has no backtrack — the view shows the derived context,
    /// not the editable conversation. See `docs/context.md`.
    fn on_key_context_debug(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.toggle_context_debug();
                Action::ToggleContextDebug
            }
            KeyCode::Up => {
                self.debug_follow = false;
                self.debug_scroll = self.debug_scroll.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                self.debug_scroll = self.debug_scroll.saturating_add(1);
                Action::None
            }
            KeyCode::PageUp => {
                self.debug_follow = false;
                self.debug_scroll = self.debug_scroll.saturating_sub(TOOL_VIEW_PAGE);
                Action::None
            }
            KeyCode::PageDown => {
                self.debug_scroll = self.debug_scroll.saturating_add(TOOL_VIEW_PAGE);
                Action::None
            }
            KeyCode::Home => {
                self.debug_follow = false;
                self.debug_scroll = 0;
                Action::None
            }
            KeyCode::End => {
                // Past any end — `settle_debug_scroll` pins it to the bottom
                // and re-engages tail-follow, like the pager's End.
                self.debug_scroll = usize::MAX;
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Flip between the conversation and the Ctrl+D context-debug view —
    /// [`toggle_tool_view`](Self::toggle_tool_view)'s sibling, with the same
    /// activity rules (the `?` band closes, a backtrack gesture is abandoned)
    /// and the same open-at-the-bottom tail-follow.
    fn toggle_context_debug(&mut self) {
        self.shortcuts_open = false;
        self.backtrack = Backtrack::default();
        self.view = match self.view {
            View::Conversation => View::ContextDebug,
            View::ContextDebug | View::ToolOutput | View::ResumePicker => View::Conversation,
        };
        self.debug_scroll = 0;
        self.debug_follow = self.view == View::ContextDebug;
    }

    /// Open the `/resume` picker over `sessions` (the boundary's scan of the
    /// sessions dir, newest first) — swaps to [`View::ResumePicker`] on the
    /// alternate screen. `cwd` (the meta-line format) seeds the default `Cwd`
    /// filter. Any `?` band or in-flight backtrack gesture is abandoned, like
    /// the Ctrl+O toggle. See `docs/resume.md`.
    pub fn open_resume_picker(&mut self, sessions: Vec<SessionSummary>, cwd: String) {
        self.shortcuts_open = false;
        self.backtrack = Backtrack::default();
        self.resume_picker = Some(ResumePicker {
            sessions,
            cwd,
            ..ResumePicker::default()
        });
        self.view = View::ResumePicker;
    }

    /// Dismiss the `/resume` picker (Esc/Ctrl+C, a failed load, or right
    /// after a successful one): back to the conversation view. The loop
    /// leaves the alternate screen and repaints, the Ctrl+O return.
    pub fn close_resume_picker(&mut self) {
        self.resume_picker = None;
        self.view = View::Conversation;
    }

    /// Install a loaded session as the conversation — the `/resume` swap.
    /// The `/clear` reset shape ([`clear_conversation`]: wipe the streaming
    /// buffer/tool/status, drain the queue into discarded images) with
    /// `items` as the new history, and the picker closed. A turn can't be
    /// *active* here (`/resume` is rejected mid-task) — the wipes are
    /// belt-and-braces. The composer draft, its attachments, and the ↑-recall
    /// history survive, like `/clear`. See `docs/resume.md`.
    ///
    /// [`clear_conversation`]: App::clear_conversation
    pub fn load_session(&mut self, items: Vec<HistoryItem>) {
        self.clear_conversation();
        self.history = items;
        // A rollout cut short mid-turn ends with its user message; that tail
        // belongs to the resumed conversation, not to any new turn — fence it
        // off from the interrupt-undo (docs/interrupt.md).
        self.undo_floor = self.history.len();
        // The gauge re-seats on the loaded conversation (docs/compact.md).
        self.refresh_context_used();
        self.close_resume_picker();
    }

    /// Keys while the `/resume` session picker is showing — codex's picker
    /// key handling, sized down: ↑/↓ move the highlight (clamped),
    /// PageUp/PageDown jump by [`RESUME_PAGE`], Home/End jump to the ends,
    /// Enter resumes the highlighted session, and Esc clears a non-empty
    /// search before it closes anything. Any plain printable character types
    /// into the search (Backspace pops) — navigation lives on the
    /// non-printable keys, codex's `allow_plain_char_navigation`. Ctrl+C and
    /// Ctrl+O are handled globally in [`on_key`] (close / inert).
    ///
    /// [`on_key`]: App::on_key
    fn on_key_resume_picker(&mut self, key: KeyEvent) -> Action {
        let Some(picker) = self.resume_picker.as_mut() else {
            return Action::None;
        };
        let last = picker.matches().len().saturating_sub(1);
        match key.code {
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => picker.selected = (picker.selected + 1).min(last),
            KeyCode::PageUp => picker.selected = picker.selected.saturating_sub(RESUME_PAGE),
            KeyCode::PageDown => picker.selected = (picker.selected + RESUME_PAGE).min(last),
            KeyCode::Home => picker.selected = 0,
            KeyCode::End => picker.selected = last,
            KeyCode::Enter => {
                // Resume the highlighted (filtered) row; an empty list has
                // nothing to resume and the picker stays up.
                if let Some(selected) = picker.matches().get(picker.selected) {
                    return Action::ResumeSession(selected.path.clone());
                }
            }
            KeyCode::Esc => {
                if picker.query.is_empty() {
                    self.close_resume_picker();
                    return Action::CloseResumePicker;
                }
                // Esc clears the search first (codex); the next Esc closes.
                picker.query.clear();
                picker.selected = 0;
            }
            KeyCode::Backspace => {
                picker.query.pop();
                picker.selected = 0;
            }
            // The Filter/Sort toolbar (codex's): Tab moves the focus between
            // the two controls (BackTab too — prev == next with two), and
            // ←/→ toggle the focused control's value, reseating the
            // selection like a query edit (the rows re-derive).
            KeyCode::Tab | KeyCode::BackTab => {
                picker.focus = match picker.focus {
                    ResumeControl::Filter => ResumeControl::Sort,
                    ResumeControl::Sort => ResumeControl::Filter,
                };
            }
            KeyCode::Left | KeyCode::Right => {
                match picker.focus {
                    ResumeControl::Filter => {
                        picker.filter = match picker.filter {
                            ResumeFilter::Cwd => ResumeFilter::All,
                            ResumeFilter::All => ResumeFilter::Cwd,
                        };
                    }
                    ResumeControl::Sort => {
                        picker.sort = match picker.sort {
                            ResumeSort::Updated => ResumeSort::Created,
                            ResumeSort::Created => ResumeSort::Updated,
                        };
                    }
                }
                picker.selected = 0;
            }
            // Plain printable characters are search input, never navigation
            // (so `q`/`j`/`k` filter instead of closing/moving — codex).
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                picker.query.push(c);
                picker.selected = 0;
            }
            _ => {}
        }
        Action::None
    }

    /// A bracketed paste while the `/resume` picker is up: the text joins the
    /// type-to-search query — codex's `normalize_pasted_search_query` —
    /// whitespace runs collapsed to single spaces (so a multiline paste stays
    /// one query), a non-empty query gaining a separating space, and a
    /// whitespace-only paste ignored. Reseats the selection like typed input.
    pub fn paste_into_resume_search(&mut self, pasted: &str) {
        let flat = pasted.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.is_empty() {
            return;
        }
        let Some(picker) = self.resume_picker.as_mut() else {
            return;
        };
        if !picker.query.is_empty() {
            picker.query.push(' ');
        }
        picker.query.push_str(&flat);
        picker.selected = 0;
    }

    /// Open the inline `/model` picker, marking `active_id` as the current
    /// model. The list starts empty in the [`ModelLoad::Loading`] state; the
    /// boundary's fetch fills it via [`set_models`]. Abandons any `?` band /
    /// palette / file picker (they share the composer the picker takes over),
    /// but stays in [`View::Conversation`] — the picker is inline, not an
    /// overlay. See `docs/llm.md`.
    ///
    /// [`set_models`]: App::set_models
    pub fn open_model_picker(&mut self, active_id: impl Into<String>) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.backtrack = Backtrack::default();
        self.model_picker = Some(ModelPicker {
            active_id: active_id.into(),
            ..ModelPicker::default()
        });
    }

    /// Record which provider the active model belongs to (the boundary knows it,
    /// the pure open call only has the id) so the picker's ✓ marks the exact
    /// active row in the merged multi-provider list. No-op if the picker closed.
    pub fn set_active_provider(&mut self, provider: impl Into<String>) {
        if let Some(picker) = self.model_picker.as_mut() {
            picker.active_provider = provider.into();
        }
    }

    /// Dismiss the inline `/model` picker (Esc/Ctrl+C, or right after a
    /// selection): the composer returns. No view change — it was never an
    /// overlay.
    pub fn close_model_picker(&mut self) {
        self.model_picker = None;
    }

    /// Install the fetched model list into the open picker (the boundary's
    /// worker result), marking it [`ModelLoad::Ready`]. The highlight seats on
    /// the currently-active model when present, else the top — so the picker
    /// opens focused on what's in use. No-op if the picker was closed meanwhile.
    pub fn set_models(&mut self, models: Vec<ModelEntry>) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.models = models;
        picker.status = ModelLoad::Ready;
        picker.pending = 0;
        picker.errors.clear();
        // Seat the highlight on the active model if it's in the (unfiltered)
        // list, so the picker opens focused on the current choice.
        picker.selected = picker
            .matches()
            .iter()
            .position(|m| m.id == picker.active_id)
            .unwrap_or(0);
    }

    /// Begin a multi-provider model load: record how many provider fetches are
    /// in flight and reset to the empty `Loading` state. The boundary spawns one
    /// worker per configured provider after this; each result arrives via
    /// [`add_models`] / [`add_model_error`]. No-op if the picker closed meanwhile.
    ///
    /// [`add_models`]: App::add_models
    /// [`add_model_error`]: App::add_model_error
    pub fn begin_model_load(&mut self, pending: usize) {
        if let Some(picker) = self.model_picker.as_mut() {
            picker.models.clear();
            picker.errors.clear();
            picker.selected = 0;
            picker.pending = pending;
            picker.status = ModelLoad::Loading;
        }
    }

    /// Merge one provider's fetched models into the open picker: append, re-sort
    /// by id then provider, drop exact `(provider, id)` dupes, and mark one
    /// outstanding fetch done. The list appears as soon as the first provider
    /// lands (status → `Ready`) and grows as the rest arrive. The highlight rides
    /// the same model across the merge, unless the user hasn't touched the picker
    /// yet — then it re-seats on the active model once its provider loads. No-op
    /// if the picker closed meanwhile. See `docs/llm.md`.
    pub fn add_models(&mut self, models: Vec<ModelEntry>) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.pending = picker.pending.saturating_sub(1);
        // An untouched picker (no search, highlight at the top) re-seats on the
        // active model when its provider lands; once the user navigates or
        // filters, the current highlight is preserved across the merge instead.
        let untouched = picker.query.is_empty() && picker.selected == 0;
        let keep = picker
            .highlighted()
            .map(|m| (m.provider.clone(), m.id.clone()));
        picker.models.extend(models);
        picker
            .models
            .sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.provider.cmp(&b.provider)));
        picker
            .models
            .dedup_by(|a, b| a.id == b.id && a.provider == b.provider);
        picker.recompute_status();
        picker.selected = if untouched {
            picker
                .matches()
                .iter()
                .position(|m| m.id == picker.active_id)
                .unwrap_or(0)
        } else {
            let clamped = picker
                .selected
                .min(picker.matches().len().saturating_sub(1));
            keep.and_then(|(p, i)| {
                picker
                    .matches()
                    .iter()
                    .position(|m| m.provider == p && m.id == i)
            })
            .unwrap_or(clamped)
        };
    }

    /// Record that one provider's model fetch failed: keep the reason (shown as a
    /// `⚠ … unavailable` note beside a partial list, or a red row when every
    /// provider fails) and mark the fetch done. No-op if the picker closed. See
    /// `docs/llm.md`.
    pub fn add_model_error(&mut self, provider: impl Into<String>, message: impl Into<String>) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.pending = picker.pending.saturating_sub(1);
        picker.errors.push(ModelFetchError {
            provider: provider.into(),
            message: message.into(),
        });
        picker.recompute_status();
        let last = picker.matches().len().saturating_sub(1);
        picker.selected = picker.selected.min(last);
    }

    /// Record that the model fetch failed — the picker shows `message` in red.
    /// No-op if the picker was closed meanwhile.
    pub fn set_models_error(&mut self, message: impl Into<String>) {
        if let Some(picker) = self.model_picker.as_mut() {
            picker.status = ModelLoad::Error(message.into());
            picker.selected = 0;
        }
    }

    /// Record that no provider is configured yet — the picker shows a `/login`
    /// hint in place of a model list (the boundary skips the fetch entirely, so
    /// the user isn't offered models they have no key for). No-op if the picker
    /// was closed meanwhile. See `docs/llm.md`.
    pub fn set_models_needs_login(&mut self) {
        if let Some(picker) = self.model_picker.as_mut() {
            picker.status = ModelLoad::NeedsLogin;
            picker.models.clear();
            picker.errors.clear();
            picker.pending = 0;
            picker.selected = 0;
        }
    }

    /// Keys while the inline `/model` picker is open. Mirrors the `/resume`
    /// picker's grammar: ↑/↓ move (clamped), PageUp/PageDown jump by
    /// [`MODEL_PAGE`], Home/End to the ends, Enter selects the highlighted
    /// model, Esc clears a non-empty search before it closes, Backspace pops,
    /// Ctrl+C closes, and any plain printable character types into the search.
    /// Owns **every** key while open (routed at the top of [`on_key`]).
    ///
    /// [`on_key`]: App::on_key
    fn on_key_model_picker(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the picker (never quits — the composer-clear/quit rules
        // don't apply while the picker owns the keys), like the /resume picker.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_model_picker();
            return Action::CloseModelPicker;
        }
        let Some(picker) = self.model_picker.as_mut() else {
            return Action::None;
        };
        let last = picker.matches().len().saturating_sub(1);
        match key.code {
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => picker.selected = (picker.selected + 1).min(last),
            KeyCode::PageUp => picker.selected = picker.selected.saturating_sub(MODEL_PAGE),
            KeyCode::PageDown => picker.selected = (picker.selected + MODEL_PAGE).min(last),
            KeyCode::Home => picker.selected = 0,
            KeyCode::End => picker.selected = last,
            KeyCode::Enter => {
                if let Some(model) = picker.matches().get(picker.selected) {
                    let action = Action::SelectModel {
                        provider: model.provider.clone(),
                        id: model.id.clone(),
                        reasoning: model.reasoning.clone(),
                        vision: model.vision,
                        context: model.context,
                    };
                    self.close_model_picker();
                    return action;
                }
            }
            KeyCode::Esc => {
                if picker.query.is_empty() {
                    self.close_model_picker();
                    return Action::CloseModelPicker;
                }
                picker.query.clear();
                picker.selected = 0;
            }
            KeyCode::Backspace => {
                picker.query.pop();
                picker.selected = 0;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                picker.query.push(c);
                picker.selected = 0;
            }
            _ => {}
        }
        Action::None
    }

    /// Open the inline `/login` onboarding flow with the given provider choices
    /// (built at the boundary so the `configured` ✓ reflects the real env / `.env`
    /// key resolution) and the `~`-relative `.env` path the provider-step hint
    /// names. Starts on the provider step; abandons any band / palette / file
    /// picker / model picker it shares the composer with, staying in
    /// [`View::Conversation`]. See `docs/llm.md`.
    pub fn open_key_onboarding(
        &mut self,
        providers: Vec<ProviderChoice>,
        env_path: impl Into<String>,
    ) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.model_picker = None;
        self.backtrack = Backtrack::default();
        self.key_onboarding = Some(KeyOnboarding {
            providers,
            env_path: env_path.into(),
            ..KeyOnboarding::default()
        });
    }

    /// Dismiss the inline `/login` flow (Esc on the empty provider filter,
    /// Ctrl+C, or right after saving a key): the composer returns. No view
    /// change — it was never an overlay.
    pub fn close_key_onboarding(&mut self) {
        self.key_onboarding = None;
    }

    /// Keys while the inline `/login` flow is open. The two steps have distinct
    /// grammars:
    ///
    /// - **Provider** (a filterable list): ↑/↓/PageUp/PageDown/Home/End move,
    ///   Enter advances to key entry for the highlighted provider, type-to-filter
    ///   with Backspace, Esc clears a non-empty filter then closes, Ctrl+C closes.
    /// - **Key** (masked entry): printable keys and Backspace edit the key, Enter
    ///   saves a non-empty key ([`Action::SaveApiKey`]) and closes, Esc steps
    ///   *back* to the provider list, Ctrl+C closes.
    ///
    /// Owns **every** key while open (routed at the top of [`on_key`]).
    ///
    /// [`on_key`]: App::on_key
    fn on_key_key_onboarding(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the whole flow (like the /model picker), from either step.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_key_onboarding();
            return Action::CloseKeyOnboarding;
        }
        let Some(onboarding) = self.key_onboarding.as_mut() else {
            return Action::None;
        };
        match onboarding.step {
            KeyStep::Provider => {
                let last = onboarding.matches().len().saturating_sub(1);
                match key.code {
                    KeyCode::Up => onboarding.selected = onboarding.selected.saturating_sub(1),
                    KeyCode::Down => onboarding.selected = (onboarding.selected + 1).min(last),
                    KeyCode::PageUp => {
                        onboarding.selected = onboarding.selected.saturating_sub(LOGIN_PAGE);
                    }
                    KeyCode::PageDown => {
                        onboarding.selected = (onboarding.selected + LOGIN_PAGE).min(last);
                    }
                    KeyCode::Home => onboarding.selected = 0,
                    KeyCode::End => onboarding.selected = last,
                    KeyCode::Enter => {
                        // Pin the highlighted provider's index in the *unfiltered*
                        // list (the filter can reorder/shrink the matches), then
                        // switch to masked key entry for it.
                        let chosen = onboarding
                            .highlighted()
                            .map(|c| c.id.clone())
                            .and_then(|id| onboarding.providers.iter().position(|p| p.id == id));
                        if let Some(idx) = chosen {
                            onboarding.chosen = Some(idx);
                            onboarding.step = KeyStep::Key;
                            onboarding.key_input.clear();
                        }
                    }
                    KeyCode::Esc => {
                        if onboarding.query.is_empty() {
                            self.close_key_onboarding();
                            return Action::CloseKeyOnboarding;
                        }
                        onboarding.query.clear();
                        onboarding.selected = 0;
                    }
                    KeyCode::Backspace => {
                        onboarding.query.pop();
                        onboarding.selected = 0;
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        onboarding.query.push(c);
                        onboarding.selected = 0;
                    }
                    _ => {}
                }
                Action::None
            }
            KeyStep::Key => match key.code {
                KeyCode::Enter => {
                    let entered = onboarding.key_input.trim().to_string();
                    if entered.is_empty() {
                        return Action::None;
                    }
                    let Some(choice) = onboarding.chosen_provider() else {
                        return Action::None;
                    };
                    let action = Action::SaveApiKey {
                        provider: choice.id.clone(),
                        env_var: choice.env_var.clone(),
                        key: entered,
                    };
                    self.close_key_onboarding();
                    action
                }
                KeyCode::Esc => {
                    // Step back to the provider list rather than closing outright.
                    onboarding.step = KeyStep::Provider;
                    onboarding.key_input.clear();
                    onboarding.chosen = None;
                    Action::None
                }
                KeyCode::Backspace => {
                    onboarding.key_input.pop();
                    Action::None
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    onboarding.key_input.push(c);
                    Action::None
                }
                _ => Action::None,
            },
        }
    }

    /// A bracketed paste while the `/login` flow is open. On the key step the
    /// pasted text is the API key — interior whitespace and control characters
    /// (a trailing newline from the paste, say) are dropped and the rest
    /// appended. On the provider step it extends the filter query like a
    /// [`paste_into_resume_search`], whitespace collapsed.
    ///
    /// [`paste_into_resume_search`]: App::paste_into_resume_search
    pub fn paste_into_key_onboarding(&mut self, pasted: &str) {
        let Some(onboarding) = self.key_onboarding.as_mut() else {
            return;
        };
        match onboarding.step {
            KeyStep::Key => {
                let cleaned: String = pasted
                    .chars()
                    .filter(|c| !c.is_whitespace() && !c.is_control())
                    .collect();
                onboarding.key_input.push_str(&cleaned);
            }
            KeyStep::Provider => {
                let flat = pasted.split_whitespace().collect::<Vec<_>>().join(" ");
                if flat.is_empty() {
                    return;
                }
                if !onboarding.query.is_empty() {
                    onboarding.query.push(' ');
                }
                onboarding.query.push_str(&flat);
                onboarding.selected = 0;
            }
        }
    }

    /// The history indices of the conversation's user messages ([`Role::User`]
    /// prompts, oldest first) — the backtrack gesture's target list (codex's
    /// `user_positions_iter`). A `!` shell header is user-*typed* but not a
    /// user *prompt*, so it is never a target (codex only walks user cells).
    fn user_message_positions(&self) -> Vec<usize> {
        self.history
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, HistoryItem::Message(m) if m.role == Role::User))
            .map(|(index, _)| index)
            .collect()
    }

    /// Is there a previous user message for Esc-Esc to edit? Gates priming
    /// (codex's `has_backtrack_target`) — with no target, idle Esc keeps its
    /// historical meaning here: quit. See `docs/backtrack.md`.
    #[must_use]
    pub fn has_backtrack_target(&self) -> bool {
        self.history
            .iter()
            .any(|item| matches!(item, HistoryItem::Message(m) if m.role == Role::User))
    }

    /// Start previewing with the newest user message highlighted, requesting
    /// a scroll to bring it into view. No-op with no target (the key arms
    /// guard, but a stale call must not underflow).
    fn begin_backtrack_preview(&mut self) {
        let count = self.user_message_positions().len();
        if count == 0 {
            return;
        }
        self.backtrack.selected = Some(count - 1);
        self.backtrack.scroll_pending = true;
        self.tool_follow = false; // pin to the highlight, not the tail
    }

    /// The primed second Esc: open the transcript overlay as a backtrack
    /// preview (codex's `open_backtrack_preview`). The caller returns
    /// [`Action::ToggleToolView`] so the loop enters the overlay screen.
    fn open_backtrack_preview(&mut self) {
        self.view = View::ToolOutput;
        self.tool_scroll = 0;
        self.begin_backtrack_preview();
    }

    /// Step the preview highlight (`-1` older, `+1` newer), clamped at both
    /// ends (codex saturates the same way). A move requests a scroll-into-view.
    fn step_backtrack(&mut self, delta: isize) {
        let count = self.user_message_positions().len();
        if let Some(selected) = self.backtrack.selected
            && count > 0
        {
            let stepped = selected.saturating_add_signed(delta).min(count - 1);
            if stepped != selected {
                self.backtrack.selected = Some(stepped);
                self.backtrack.scroll_pending = true;
            }
        }
    }

    /// Confirm the preview (Enter): drop the highlighted user message and
    /// everything after it from the history, reset the gesture, return to the
    /// conversation view, and put the message's text back in the composer to
    /// edit — codex's rollback + `set_composer_text`, except there is no
    /// backend session to fork: one prompt per turn means truncating
    /// [`history`] *is* the whole rewind. The loop's return-from-overlay
    /// repaint rebuilds the inline view from the truncated history. The
    /// message's image attachments come back with it (the interrupt-undo
    /// dance): any pairs backing the clobbered draft are discarded, the
    /// restored placeholders are re-keyed to the message's recorded paths,
    /// and the *later* dropped user messages' attachments — now referenced by
    /// nothing — are queued for temp-file deletion. See `docs/backtrack.md`.
    ///
    /// [`history`]: App::history
    fn confirm_backtrack(&mut self) {
        let positions = self.user_message_positions();
        let Some(&position) = self.backtrack.selected.and_then(|i| positions.get(i)) else {
            return;
        };
        let HistoryItem::Message(message) = &self.history[position] else {
            return;
        };
        let text = message.text.clone();
        let images = message.images.clone();
        // Attachments of user messages *after* the rewound one leave the
        // conversation entirely — orphaned temp files, queued for deletion.
        for item in &self.history[position + 1..] {
            if let HistoryItem::Message(m) = item
                && m.role == Role::User
            {
                self.discarded_images.extend(m.images.iter().cloned());
            }
        }
        self.history.truncate(position);
        self.history_generation += 1;
        // The rewound history's tail can be an older batch-sibling user
        // message — fence it off from the interrupt-undo like a resumed tail.
        self.undo_floor = self.history.len();
        // The gauge re-seats on the rewound conversation (docs/compact.md).
        self.refresh_context_used();
        self.backtrack = Backtrack::default();
        self.view = View::Conversation;
        self.recall_input(&text);
        // recall_input replaced the draft: discard any pairs that backed it,
        // then re-key the rewound message's attachments to the placeholders
        // now back in the composer (paths were recorded in occurrence order).
        let stale = std::mem::take(&mut self.images);
        self.discarded_images
            .extend(stale.into_iter().map(|(_, path)| path));
        self.images = crate::paste::image_placeholder_occurrences(&text)
            .into_iter()
            .zip(images)
            .collect();
    }

    /// Take the pending scroll-into-view request, if any. The overlay draw
    /// calls this once per frame and, when it returns true, applies
    /// `ui::backtrack_scroll`'s decision via [`apply_backtrack_scroll`] —
    /// consumed so it runs once per selection change, like codex's
    /// `scroll_chunk_into_view`, never fighting manual scrolling.
    ///
    /// [`apply_backtrack_scroll`]: App::apply_backtrack_scroll
    #[must_use]
    pub fn take_backtrack_scroll(&mut self) -> bool {
        std::mem::take(&mut self.backtrack.scroll_pending)
    }

    /// Seat the overlay scroll on the previewed highlight, releasing the
    /// tail-follow pin: previewing the last message parks the scroll at max,
    /// which re-engages following ([`settle_tool_scroll`]) — left set, it
    /// would yank the next step-older's scroll straight back to the bottom.
    ///
    /// [`settle_tool_scroll`]: App::settle_tool_scroll
    pub fn apply_backtrack_scroll(&mut self, scroll: usize) {
        self.tool_scroll = scroll;
        self.tool_follow = false;
    }

    /// Settle the tool-view scroll for a draw given the largest offset the current
    /// screen allows. While following, it stays pinned to the bottom (`max`);
    /// otherwise it's capped to `max`, and reaching the bottom re-engages
    /// following so new content keeps scrolling into view.
    pub fn settle_tool_scroll(&mut self, max: usize) {
        if self.tool_scroll >= max {
            self.tool_follow = true; // at (or past) the bottom — stick to it
        }
        self.tool_scroll = if self.tool_follow {
            max
        } else {
            self.tool_scroll.min(max)
        };
    }

    /// Settle the context-debug scroll for a draw —
    /// [`settle_tool_scroll`](Self::settle_tool_scroll) for the Ctrl+D view.
    pub fn settle_debug_scroll(&mut self, max: usize) {
        if self.debug_scroll >= max {
            self.debug_follow = true;
        }
        self.debug_scroll = if self.debug_follow {
            max
        } else {
            self.debug_scroll.min(max)
        };
    }

    /// Inject the active backend's system prompt (from
    /// `ReplySource::system_prompt`, at startup and on a `/model` switch) so
    /// the Ctrl+D view shows the whole context window. See `docs/context.md`.
    pub fn set_system_prompt(&mut self, prompt: Option<String>) {
        self.system_prompt = prompt;
    }

    /// Inject the project's rendered AGENTS.md instructions (from
    /// `project_doc::load_user_instructions`, at startup and refreshed at
    /// every turn start) so the context derivation, the Ctrl+D view, and the
    /// token estimate all carry them. See `docs/project-doc.md`.
    pub fn set_user_instructions(&mut self, instructions: Option<String>) {
        self.user_instructions = instructions;
    }

    /// Record a finished user message in the history.
    pub fn record_user_message(&mut self, text: &str) {
        self.record_message(Role::User, text);
    }

    /// Record a finished user message together with its Ctrl+V image
    /// attachments — the submit path, so the message keeps its images for the
    /// conversation context and the session file (`docs/context.md`).
    pub fn record_user_message_with_images(&mut self, text: &str, images: Vec<PathBuf>) {
        self.record_message_with_images(Role::User, text, images);
    }

    /// Pop the front queued batch — the messages of the next turn — for the loop
    /// to send when the current turn ends; empty when nothing is queued. Each
    /// batch is its own turn, so popping one per turn-end iterates the
    /// Tab-separated follow-ups in order; a batch's own messages (consecutive
    /// Enters) join with newlines into that single turn (`start_turn`). See
    /// `docs/queue.md`.
    #[must_use]
    pub fn drain_next_batch(&mut self) -> Option<QueuedTurn> {
        self.queued.pop_front()
    }

    /// Take the **last** queued batch (`pop_back`), for Alt+Up to pull just that
    /// most-recent turn-batch into the composer as one editable draft — its
    /// messages newline-joined, the earlier batches left queued (codex's
    /// `edit_queued_message`, which pops the most recent entry). Empty when
    /// nothing is queued.
    fn drain_last_batch(&mut self) -> Option<QueuedTurn> {
        self.queued.pop_back()
    }

    /// Queue the composer draft while a turn streams. `new_batch` picks the
    /// Enter vs Tab semantics: Enter (`false`) appends to the batch currently
    /// being accumulated so consecutive Enters batch into one next turn; **Tab
    /// (`true`) opens a new batch** so the message runs as its own follow-up
    /// turn after the batches already queued. An empty queue starts a fresh
    /// batch either way. A **shell-mode draft** instead queues as a standalone
    /// [`QueuedTurn::Shell`] entry ([`queue_shell`]) — run locally, never merged
    /// — so this only ever touches the [`QueuedTurn::Messages`] batches. The text
    /// is recorded for ↑ recall, like a normal submit.
    ///
    /// [`queue_shell`]: App::queue_shell
    fn queue_draft(&mut self, new_batch: bool) {
        if self.shell_mode {
            return self.queue_shell();
        }
        // The draft's image attachments ride with the batch — staged before
        // take_input clears the composer, exactly like the idle submit path
        // (docs/image-paste.md).
        let images = std::mem::take(&mut self.images);
        let text = self.take_input();
        self.file_search = None; // the composer is consumed into the queue
        self.input_history.record(&text);
        match self.queued.back_mut() {
            Some(QueuedTurn::Messages {
                texts,
                images: batch_images,
            }) if !new_batch => {
                texts.push(text);
                batch_images.extend(images);
            }
            _ => self.queued.push_back(QueuedTurn::Messages {
                texts: vec![text],
                images,
            }),
        }
    }

    /// Queue the shell-mode draft as a standalone [`QueuedTurn::Shell`] entry —
    /// run locally as its own turn when the queue drains (`main.rs`'s
    /// `flush_next_queued` → `run_shell`), **never merged** with a neighbouring
    /// batch (codex's `submit_queued_shell_prompt` on a queued `RunShell`
    /// action: the next Enter-text starts a fresh batch since the back is a
    /// `Shell`). Exits the mode and records the full `!command` for ↑ recall,
    /// mirroring the idle [`Action::RunShell`] path. See `docs/shell-command.md`.
    fn queue_shell(&mut self) {
        let raw = self.take_input();
        self.shell_mode = false;
        self.input_history.record(&format!("!{raw}"));
        self.queued
            .push_back(QueuedTurn::Shell(raw.trim().to_string()));
    }

    /// Record a system notice (from a slash command) in the history, so it
    /// repaints on resize like any other message. The loop also commits it to
    /// scrollback. Mirrors [`record_user_message`] for [`Action::Notice`].
    pub fn record_system_message(&mut self, text: &str) {
        self.record_message(Role::System, text);
    }

    /// Record a red error notice in the history (so a resize repaints it), like
    /// [`record_system_message`] but [`Role::Error`]. Used for a Ctrl+V clipboard
    /// failure — codex's `new_error_event`. See `docs/image-paste.md`.
    pub fn record_error_message(&mut self, text: &str) {
        self.record_message(Role::Error, text);
    }

    /// Announce a **parallel batch** of tool calls the model requested this
    /// round (each [`ToolCallSummary`]'s `name`/`args` for the `● name(args)`
    /// header), all queued as [`ToolStatus::Waiting`] so the live region shows
    /// every call at once — the ones not yet running as `⎿ Waiting…`. Sequential
    /// execution then flips them to `Running` one at a time via
    /// [`start_tool`](App::start_tool). Called by the boundary on a
    /// [`crate::stream::StreamEvent::ToolBatch`]; a backend that never batches
    /// (the `!` shell, the dummy's lone calls) skips it and a lone
    /// [`start_tool`](App::start_tool) still works. See `docs/parallel-tools.md`.
    pub fn start_tool_batch(&mut self, items: &[ToolCallSummary]) {
        self.tool_queue = items
            .iter()
            .map(|item| ToolCall {
                name: item.name.clone(),
                args: item.args.clone(),
                status: ToolStatus::Waiting,
                output: String::new(),
                timestamp: String::new(), // stamped when it finishes (see end_tool)
                shell: false,
                truncated: false,
            })
            .collect();
    }

    /// Begin a tool call: mark it the running (blue) call at the front of the
    /// live queue so the bottom region shows it before any output arrives.
    ///
    /// If the front call is a `Waiting` batch sibling (`start_tool_batch`
    /// announced it), it is flipped to `Running` — the batch's `(name, args)` are
    /// authoritative and equal the ones passed here (both come from the same
    /// backend summary), so only the status changes. Otherwise (an empty queue —
    /// the `!` shell, the dummy's lone calls) a fresh `Running` call is pushed, so
    /// the single-tool path is unchanged.
    pub fn start_tool(&mut self, name: &str, args: &str) {
        if let Some(front) = self.tool_queue.front_mut()
            && front.status == ToolStatus::Waiting
        {
            front.status = ToolStatus::Running;
            return;
        }
        self.tool_queue.push_back(ToolCall {
            name: name.to_string(),
            args: args.to_string(),
            status: ToolStatus::Running,
            output: String::new(),
            timestamp: String::new(), // stamped when it finishes (see end_tool)
            shell: false,
            truncated: false,
        });
    }

    /// Mark the running tool's output as **truncated** (set by the boundary just
    /// before [`end_tool`] when a `!` command's output exceeded the in-memory
    /// cap): the cell will append a dim `…` marker at the end of the expanded
    /// output. No-op when no tool is running. See `docs/shell-command.md`.
    ///
    /// [`end_tool`]: App::end_tool
    pub fn set_tool_truncated(&mut self) {
        if let Some(tool) = self.tool_queue.front_mut() {
            tool.truncated = true;
        }
    }

    /// Append a chunk of live output to the **running** tool at the front of the
    /// queue so its cell tails the output as it streams (the boundary's handler
    /// for [`crate::stream::StreamEvent::ToolOutput`]; see
    /// `docs/tool-streaming.md`). A no-op unless the front call is
    /// [`ToolStatus::Running`] — a `Waiting` batch sibling has not started and an
    /// empty queue has nothing to tail. Does **not** count tokens: the tally is
    /// charged once from the authoritative `ToolEnd` output in
    /// [`end_tool`](App::end_tool), which overwrites this partial, so the live
    /// tail and the final cell never double-count.
    pub fn push_tool_output(&mut self, chunk: &str) {
        if let Some(tool) = self.tool_queue.front_mut()
            && tool.status == ToolStatus::Running
        {
            tool.output.push_str(chunk);
        }
    }

    /// The tool currently at the front of the live queue — the running (or, in
    /// the brief gap between a batch's calls, about-to-run) call, if any.
    #[must_use]
    pub fn current_tool(&self) -> Option<&ToolCall> {
        self.tool_queue.front()
    }

    /// The whole live tool queue, front-first: the running/next call followed by
    /// any [`ToolStatus::Waiting`] batch siblings. The renderer walks this to show
    /// every call in a parallel batch (the running one live, the rest as
    /// `⎿ Waiting…`). Empty when no tool is in flight. See `docs/parallel-tools.md`.
    #[must_use]
    pub fn tool_queue(&self) -> &VecDeque<ToolCall> {
        &self.tool_queue
    }

    /// Finish the in-flight tool call with its final `output` and outcome
    /// (`ok` → [`ToolStatus::Ok`], else [`ToolStatus::Failed`]), record it in the
    /// history, and remove it from the front of the live queue — the next batch
    /// sibling (if any) becomes the front. Returns the finished call (for the
    /// event loop to commit to scrollback), or `None` if no tool was running.
    pub fn end_tool(&mut self, output: &str, ok: bool) -> Option<ToolCall> {
        let status = if ok {
            ToolStatus::Ok
        } else {
            ToolStatus::Failed
        };
        self.resolve_front_tool(output, status)
    }

    /// Resolve the in-flight tool call as **moved to the background** (a
    /// `run_in_background` bash call, or Ctrl+B on a running command):
    /// [`ToolStatus::Backgrounded`], with `output` holding the model-facing
    /// launch text (task id + interim-output path) that the cell never shows —
    /// it renders the fixed `⎿ Running in the background (↓ to manage)` row.
    /// The boundary's handler for `StreamEvent::ToolBackgrounded`. See
    /// `docs/background.md`.
    pub fn background_tool(&mut self, output: &str) -> Option<ToolCall> {
        self.resolve_front_tool(output, ToolStatus::Backgrounded)
    }

    /// The shared tail of [`end_tool`]/[`background_tool`]: pop the front
    /// call, stamp + record it with `status`, and fold its output into the
    /// token tally (arrow up — uploaded back; the count is *added to*, never
    /// reset — see `docs/status-indicator.md`).
    ///
    /// [`end_tool`]: App::end_tool
    /// [`background_tool`]: App::background_tool
    fn resolve_front_tool(&mut self, output: &str, status: ToolStatus) -> Option<ToolCall> {
        let mut tool = self.tool_queue.pop_front()?;
        tool.output = output.to_string();
        tool.status = status;
        tool.timestamp = self.now_stamp();
        if let Some(turn) = self.status.as_mut() {
            turn.tokens += count_tokens(output);
            turn.arrow = TokenArrow::Up;
        }
        self.history.push(HistoryItem::Tool(tool.clone()));
        Some(tool)
    }

    /// The running background shells, oldest first (see `docs/background.md`).
    #[must_use]
    pub fn background(&self) -> &[BackgroundShell] {
        &self.background
    }

    /// The running background shell with this registry id, if it still runs.
    #[must_use]
    pub fn background_shell(&self, id: &str) -> Option<&BackgroundShell> {
        self.background.iter().find(|shell| shell.id == id)
    }

    /// Has any background shell ever started this session? The ↓ manager
    /// opens once true (showing the empty state after they all finish).
    #[must_use]
    pub const fn background_ever(&self) -> bool {
        self.had_background
    }

    /// A background shell started (the registry's `BgEvent::Started`): list it
    /// so the footer count, the summary suffix, and the ↓ manager see it.
    pub fn bg_started(
        &mut self,
        id: &str,
        command: &str,
        description: Option<String>,
        from_model: bool,
    ) {
        self.had_background = true;
        self.background.push(BackgroundShell {
            id: id.to_string(),
            command: command.to_string(),
            description,
            from_model,
            output: String::new(),
            runtime: Duration::ZERO,
        });
    }

    /// Append a chunk of a background shell's live output (the registry's
    /// `BgEvent::Output`), keeping only the newest [`BG_TAIL_MAX_BYTES`] —
    /// trimmed from the front on line boundaries so the details view always
    /// tails whole lines. Unknown ids (a chunk racing its shell's removal)
    /// are dropped.
    pub fn bg_output(&mut self, id: &str, chunk: &str) {
        let Some(shell) = self.background.iter_mut().find(|shell| shell.id == id) else {
            return;
        };
        shell.output.push_str(chunk);
        if shell.output.len() > BG_TAIL_MAX_BYTES {
            let cut = shell.output.len() - BG_TAIL_MAX_BYTES;
            // Trim to the next line boundary past the cut so the tail never
            // opens mid-line (fall back to a char boundary when one line
            // exceeds the whole cap).
            let boundary = shell.output[cut..]
                .find('\n')
                .map_or_else(|| ceil_char_boundary(&shell.output, cut), |nl| cut + nl + 1);
            shell.output.drain(..boundary);
        }
    }

    /// Inject a background shell's runtime before a draw (the
    /// [`set_status_times`](App::set_status_times) pattern — the started
    /// clocks live at the boundary). Unknown ids are ignored.
    pub fn set_background_runtime(&mut self, id: &str, runtime: Duration) {
        if let Some(shell) = self.background.iter_mut().find(|shell| shell.id == id) {
            shell.runtime = runtime;
        }
    }

    /// A background shell exited (the registry's `BgEvent::Exited`): remove it
    /// from the list and return its completion for the loop to settle —
    /// deferred to the next safe boundary while a turn is in flight
    /// ([`defer_bg_completion`](App::defer_bg_completion)), else settled at
    /// once. A details view watching this shell falls back to the list (and
    /// the list selection re-clamps); `None` for an unknown id (already
    /// swept — e.g. by `/clear` — so no notice is owed).
    pub fn bg_exited(&mut self, id: &str, code: Option<i32>, killed: bool) -> Option<BgCompletion> {
        let index = self.background.iter().position(|shell| shell.id == id)?;
        let shell = self.background.remove(index);
        match &mut self.background_view {
            Some(BackgroundView::Details { id: watched }) if *watched == id => {
                self.background_view = Some(BackgroundView::List {
                    selected: index.min(self.background.len().saturating_sub(1)),
                });
            }
            Some(BackgroundView::List { selected }) => {
                *selected = (*selected).min(self.background.len().saturating_sub(1));
            }
            _ => {}
        }
        Some(BgCompletion {
            id: shell.id,
            command: shell.command,
            description: shell.description,
            from_model: shell.from_model,
            code,
            killed,
            output_tail: notice_tail(&shell.output),
        })
    }

    /// Hold a completion that landed mid-turn for the next boundary settle.
    pub fn defer_bg_completion(&mut self, completion: BgCompletion) {
        self.pending_bg.push_back(completion);
    }

    /// Drain the held completions (empty when none landed) — the loop
    /// settles them at every safe boundary: each tool resolution and
    /// segment-flush point mid-turn, and every turn end (`StreamDone`,
    /// `Error`, and the Esc interrupt alike). See `docs/background.md`.
    pub fn take_pending_bg_completions(&mut self) -> Vec<BgCompletion> {
        self.pending_bg.drain(..).collect()
    }

    /// Record a completion's [`BackgroundNotice`] in history (stamped like
    /// every recorded item) and return it for the loop to commit to
    /// scrollback. The notice is what repaints on resize, lists in the Ctrl+O
    /// transcript, and rides the derived context to the model.
    pub fn record_background_notice(&mut self, completion: &BgCompletion) -> BackgroundNotice {
        let notice = BackgroundNotice {
            description: completion.display_description().to_string(),
            id: completion.id.clone(),
            code: completion.code,
            killed: completion.killed,
            output_tail: completion.output_tail.clone(),
            timestamp: self.now_stamp(),
        };
        self.history.push(HistoryItem::Background(notice.clone()));
        notice
    }

    /// Can Ctrl+B move the current command to the background? True while the
    /// front tool is a **running command** — a model `bash` call or a `!`
    /// shell turn — the only runners that poll the registry's background
    /// request. See `docs/background.md`.
    #[must_use]
    pub fn can_move_to_background(&self) -> bool {
        self.tool_queue.front().is_some_and(|tool| {
            tool.status == ToolStatus::Running && (tool.shell || tool.name == "Bash")
        })
    }

    /// Should ↓ open the background manager? Only from an idle-looking
    /// composer — empty, not in shell mode, no palette/file band open — and
    /// only once a background shell has ever run ([`background_ever`]), so ↓
    /// keeps its history-recall/cursor meaning otherwise.
    ///
    /// [`background_ever`]: App::background_ever
    fn background_openable(&self) -> bool {
        self.had_background
            && self.input.is_empty()
            && !self.shell_mode
            && self.command_menu.is_none()
            && self.file_search.is_none()
    }

    /// Open the ↓ manager band on the shell list, dismissing whatever shared
    /// the composer (the shortcuts band; the pickers own their keys, so they
    /// can't be open here).
    pub fn open_background_view(&mut self) {
        self.shortcuts_open = false;
        self.backtrack = Backtrack::default();
        self.background_view = Some(BackgroundView::List { selected: 0 });
    }

    /// Close the ↓ manager band; the composer returns on the next draw.
    pub fn close_background_view(&mut self) {
        self.background_view = None;
    }

    /// Keys while the ↓ background manager band is open — it owns **every**
    /// key (routed at the top of [`on_key`](App::on_key)), like the `/model`
    /// picker. List: ↑/↓ move, Enter views the highlighted shell, `x` stops
    /// it, Esc/Ctrl+C close. Details: ← back to the list, Esc/Enter/Space
    /// close, `x` stops. See `docs/background.md`.
    fn on_key_background(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_background_view();
            return Action::None;
        }
        let Some(view) = self.background_view.as_mut() else {
            return Action::None;
        };
        match view {
            BackgroundView::List { selected } => {
                let last = self.background.len().saturating_sub(1);
                match key.code {
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Down => *selected = (*selected + 1).min(last),
                    KeyCode::Enter => {
                        if let Some(shell) = self.background.get(*selected) {
                            let id = shell.id.clone();
                            self.background_view = Some(BackgroundView::Details { id });
                        }
                    }
                    KeyCode::Char('x') => {
                        if let Some(shell) = self.background.get(*selected) {
                            return Action::KillBackground(shell.id.clone());
                        }
                    }
                    KeyCode::Esc => self.close_background_view(),
                    _ => {}
                }
            }
            BackgroundView::Details { id } => match key.code {
                KeyCode::Left => {
                    let selected = self
                        .background
                        .iter()
                        .position(|shell| shell.id == *id)
                        .unwrap_or(0);
                    self.background_view = Some(BackgroundView::List { selected });
                }
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ') => self.close_background_view(),
                KeyCode::Char('x') => return Action::KillBackground(id.clone()),
                _ => {}
            },
        }
        Action::None
    }

    /// Finalise the current run of assistant text as a history message so a
    /// following tool call slots after it in order, then start a fresh empty
    /// buffer for the text that follows. Returns the finalised segment text (for
    /// the loop to flush to scrollback) or `None` if there was nothing buffered.
    /// The stream stays open.
    pub fn flush_streaming_segment(&mut self) -> Option<String> {
        let buf = self.streaming.as_mut()?;
        // A whitespace-only run (a model emitting `\n\n` before a tool call)
        // records no message: it renders to zero rows once trailing blanks are
        // trimmed (`ui::assistant_lines`), so recording it would leave a stray
        // `● ` bullet on a repaint that the live view never showed.
        if buf.trim().is_empty() {
            buf.clear(); // leaves Some("") — the stream stays open
            return None;
        }
        let text = std::mem::take(buf); // leaves Some("") — the stream stays open
        self.record_message(Role::Assistant, text.clone());
        Some(text)
    }

    /// Begin a reply: open an empty streaming buffer so the live region can
    /// show the assistant is responding even before the first chunk arrives, and
    /// start the live turn status (pick this turn's verbs, reset the tally).
    pub fn begin_stream(&mut self) {
        self.streaming = Some(String::new());
        // A new real turn re-arms the auto-compact trigger (one attempt per
        // user turn — docs/compact.md).
        self.auto_compact_blocked = false;
        // A fresh turn starts with the single-row preview; the boundary
        // re-injects the real count each frame (a forming table grows it).
        self.stream_preview_rows = 1;
        // The usage accumulators are per-turn (docs/prompt-caching.md).
        self.turn_usage_tokens = 0;
        self.turn_usage_cached = 0;
        let verb = WORKING_VERBS[self.turn_count % WORKING_VERBS.len()];
        let done_verb = DONE_VERBS[self.turn_count % DONE_VERBS.len()];
        self.turn_count = self.turn_count.wrapping_add(1);
        self.status = Some(TurnStatus {
            verb,
            done_verb,
            tokens: 0,
            arrow: TokenArrow::Down,
            elapsed: Duration::ZERO,
            thinking: None,
            shell: false,
            retry: None,
        });
    }

    /// Begin a `!` shell command as a turn, reusing the AI turn machinery so the
    /// command gets the live status strip, the spinner/timer, `esc to
    /// interrupt`, and the resize repaint for free (see `docs/shell-command.md`):
    ///
    /// - the command itself is recorded **up front** as a [`Role::Shell`]
    ///   header message (`! pwd` on the dark user-style line) — the top of the
    ///   codex-style exec cell, in history so a mid-run resize repaints it;
    /// - an **empty** streaming buffer (a shell turn has no assistant text) — so
    ///   [`is_streaming`] is true (the strip shows, a mid-run Enter queues) but
    ///   [`finish_stream`] records no phantom message;
    /// - a fixed-verb, `shell`-flagged status ([`SHELL_VERB`] → `Running…`; the
    ///   flag makes [`end_turn`] skip the summary — the cell is its own record);
    /// - the command as the running **shell** tool: headerless, so its
    ///   `⎿ Running…` peek previews flush under the header, resolving into the
    ///   `⎿ output` lines on completion.
    ///
    /// The boundary's runner then streams a `ToolEnd`/`StreamDone` pair back,
    /// committing the cell through the existing paths.
    ///
    /// [`is_streaming`]: App::is_streaming
    /// [`finish_stream`]: App::finish_stream
    /// [`end_turn`]: App::end_turn
    pub fn begin_shell(&mut self, command: &str) {
        self.record_message(Role::Shell, command);
        self.streaming = Some(String::new());
        // A shell turn never receives usage, but the per-turn accumulators
        // reset with every turn machinery start all the same.
        self.turn_usage_tokens = 0;
        self.turn_usage_cached = 0;
        self.status = Some(TurnStatus {
            verb: SHELL_VERB,
            // Never rendered: a shell turn ends without a summary (end_turn
            // returns None for it), so no dedicated done verb exists.
            done_verb: SHELL_VERB,
            tokens: 0,
            arrow: TokenArrow::Down,
            elapsed: Duration::ZERO,
            thinking: None,
            shell: true,
            retry: None,
        });
        self.start_tool(command, "");
        if let Some(tool) = self.tool_queue.front_mut() {
            tool.shell = true;
        }
    }

    /// Begin a `/compact` turn (`docs/compact.md`), reusing the AI turn
    /// machinery like [`begin_shell`](App::begin_shell) so the summarization
    /// request gets the live status strip, `esc to interrupt`, and the
    /// mid-turn queueing for free:
    ///
    /// - an **empty** streaming buffer — [`is_streaming`](App::is_streaming)
    ///   is true (the strip shows, Enter queues, [`fail_stream`] works) but
    ///   the visible reply stays empty: chunks divert into the summary buffer
    ///   via [`push_chunk`](App::push_chunk), so the summary is never rendered
    ///   (codex parity) while the tally ticks;
    /// - a fixed-verb status ([`COMPACT_VERB`] → `Compacting…`) that does
    ///   **not** advance the cycled per-turn verbs (`turn_count` untouched).
    ///
    /// The boundary spawns the summarize request; [`finish_compact`] lands the
    /// marker at `StreamDone`, while an interrupt/error/`/clear` drops the
    /// buffer — the old context stands (codex swaps only at the very end).
    ///
    /// [`fail_stream`]: App::fail_stream
    /// [`finish_compact`]: App::finish_compact
    pub fn begin_compact(&mut self, auto: bool) {
        self.streaming = Some(String::new());
        self.compact_buffer = Some(String::new());
        self.compact_auto = auto;
        self.compact_before = self.context_used;
        self.stream_preview_rows = 1;
        // Per-turn usage accumulators reset with every turn machinery start.
        self.turn_usage_tokens = 0;
        self.turn_usage_cached = 0;
        self.status = Some(TurnStatus {
            verb: COMPACT_VERB,
            // Never rendered: a compact turn ends without a summary (the
            // `● Context compacted` cell is its record).
            done_verb: COMPACT_VERB,
            tokens: 0,
            arrow: TokenArrow::Down,
            elapsed: Duration::ZERO,
            thinking: None,
            shell: false,
            retry: None,
        });
    }

    /// Whether a `/compact` turn is in flight (the summary buffer is open).
    #[must_use]
    pub const fn is_compacting(&self) -> bool {
        self.compact_buffer.is_some()
    }

    /// End a `/compact` turn: take the streamed summary, append the
    /// [`HistoryItem::Compaction`] marker (an *append* — the transcript, the
    /// recorder watermark, and the checkpoint keys all stay valid), and clear
    /// the turn state with **no** `Done for Ns` summary. Returns the appended
    /// marker for the boundary to commit the `● Context compacted` cell, or
    /// `None` when no compact turn was in flight. The context derivation
    /// ([`crate::context::context_messages`]) applies the compacted shape from
    /// the marker on. See `docs/compact.md`.
    pub fn finish_compact(&mut self) -> Option<Compaction> {
        let summary = self.compact_buffer.take()?;
        self.streaming = None;
        self.status = None;
        let mut compaction = Compaction {
            // A model reply often ends with a trailing newline; the bridge
            // adds its own separators, so store the summary trimmed.
            summary: summary.trim().to_string(),
            timestamp: self.now_stamp(),
            before: self.compact_before,
            after: 0,
            auto: self.compact_auto,
        };
        self.history
            .push(HistoryItem::Compaction(compaction.clone()));
        // The gauge drops to the compacted derivation's estimate (codex's
        // recompute_token_usage) — the marker records the shrink.
        self.refresh_context_used();
        compaction.after = self.context_used;
        if let Some(HistoryItem::Compaction(recorded)) = self.history.last_mut() {
            recorded.after = self.context_used;
        }
        // One attempt per user turn: no immediate auto re-trigger.
        self.auto_compact_blocked = true;
        Some(compaction)
    }

    /// Inject the active model's context window in tokens (the boundary's
    /// `/v1/models` `context_length`, the saved settings, or the
    /// `ALTER_ZERO_CONTEXT_WINDOW` override) — `None` (or a meaningless 0)
    /// hides the footer gauge and disables auto-compact. See `docs/compact.md`.
    pub fn set_context_window(&mut self, window: Option<u64>) {
        self.context_window = window.filter(|&w| w > 0);
    }

    /// The active model's context window in tokens, when known.
    #[must_use]
    pub const fn context_window(&self) -> Option<u64> {
        self.context_window
    }

    /// The current context size in tokens (see the field doc) — the footer
    /// gauge's numerator.
    #[must_use]
    pub const fn context_used(&self) -> u64 {
        self.context_used
    }

    /// Estimate the current context size with the tokenizer: the system
    /// prompt, plus every derived context message's text (the AGENTS.md
    /// instructions among them — they ride every request), tool calls, and a
    /// flat [`IMAGE_INPUT_TOKENS`] per attachment. The stand-in between real
    /// usage frames — and the only measure right after a history mutation
    /// (compaction, `/clear`, backtrack, `/resume`) made the last frame stale.
    #[must_use]
    fn estimate_context_tokens(&self) -> u64 {
        let mut total = self.system_prompt.as_deref().map_or(0, count_tokens);
        for message in
            crate::context::context_messages_with(self.user_instructions.as_deref(), &self.history)
        {
            total += count_tokens(&message.text);
            total += message.images.len() * IMAGE_INPUT_TOKENS;
            for call in &message.tool_calls {
                total += count_tokens(&call.name) + count_tokens(&call.arguments);
            }
        }
        u64::try_from(total).unwrap_or(u64::MAX)
    }

    /// Re-seat the gauge on the tokenizer estimate (the last usage frame no
    /// longer describes the derived context).
    fn refresh_context_used(&mut self) {
        self.context_used = self.estimate_context_tokens();
    }

    /// Whether the loop should start an **auto-compact** turn at the next idle
    /// boundary: the window is known, the gauge is past codex's 90% threshold,
    /// no turn is in flight, the last compact attempt isn't still blocking
    /// (one per user turn), and the conversation derives a non-empty context
    /// to summarize. See `docs/compact.md`.
    #[must_use]
    pub fn should_auto_compact(&self) -> bool {
        let Some(window) = self.context_window else {
            return false;
        };
        if self.auto_compact_blocked || self.turn_active() || self.is_streaming() {
            return false;
        }
        let threshold = window.saturating_mul(AUTO_COMPACT_NUMERATOR) / AUTO_COMPACT_DENOMINATOR;
        if self.context_used <= threshold {
            return false;
        }
        !crate::context::context_messages(&self.history).is_empty()
    }

    /// Count a just-submitted user message into the live tally as **uploaded
    /// input** — the tokens grow and the arrow points `↑` (like a tool result
    /// folded back in, the reverse of streaming output). Called right after
    /// [`begin_stream`] with the turn's prompt, so the status shows
    /// `↑ N tokens` while the model spins up before its first chunk (the first
    /// [`push_chunk`] flips the arrow back to `↓`). The tally is **added to**,
    /// never reset (see `docs/status-indicator.md`). No-op when no turn is in
    /// flight.
    ///
    /// [`begin_stream`]: App::begin_stream
    /// [`push_chunk`]: App::push_chunk
    pub fn count_user_input(&mut self, text: &str) {
        if let Some(status) = self.status.as_mut() {
            status.tokens += count_tokens(text);
            status.arrow = TokenArrow::Up;
        }
    }

    /// Append a streamed chunk to the in-progress reply (and grow the live token
    /// tally, arrow pointing down — output streaming). No-op if not streaming.
    /// A `/compact` turn diverts the chunk into the summary buffer instead —
    /// the visible reply stays empty (the summary is never rendered) while the
    /// tally still ticks. See `docs/compact.md`.
    pub fn push_chunk(&mut self, chunk: &str) {
        if let Some(buf) = self.compact_buffer.as_mut() {
            buf.push_str(chunk);
        } else if let Some(buf) = self.streaming.as_mut() {
            buf.push_str(chunk);
        }
        if let Some(status) = self.status.as_mut() {
            status.tokens += count_tokens(chunk);
            status.arrow = TokenArrow::Down;
            // Content arrived, so the retrying request just succeeded — drop the
            // live retry indicator.
            status.retry = None;
        }
    }

    /// Count a streamed reasoning delta into the live token tally (arrow down —
    /// it is model output, streaming) **without** touching the reply buffer:
    /// the text itself is opaque and never rendered. This is what keeps the
    /// count ticking while the status line shows `Thinking for Ns`. No-op when
    /// no turn is in flight.
    pub fn push_thinking(&mut self, chunk: &str) {
        if let Some(status) = self.status.as_mut() {
            status.tokens += count_tokens(chunk);
            status.arrow = TokenArrow::Down;
            // Reasoning is content too — the retrying request recovered.
            status.retry = None;
        }
    }

    /// Count a streamed tool-call generation delta (the model emitting a tool
    /// call's `name`/`arguments` fragments) into the live token tally (arrow
    /// down — it is model output, streaming) **without** touching the reply
    /// buffer: the fragment is opaque JSON, never rendered. This keeps the
    /// status count ticking while the model *generates* a tool call, exactly
    /// like reasoning ticks it while the model thinks (see
    /// `docs/status-indicator.md`). No-op when no turn is in flight.
    pub fn push_tool_call_progress(&mut self, chunk: &str) {
        if let Some(status) = self.status.as_mut() {
            status.tokens += count_tokens(chunk);
            status.arrow = TokenArrow::Down;
            // Generating the call is content too — a retrying request recovered.
            status.retry = None;
        }
    }

    /// Fold one provider usage report (the round's final `usage` frame,
    /// [`crate::stream::StreamEvent::Usage`]) into the turn: accumulate the
    /// billed total and its cached share, and **snap the live tally to the
    /// accumulated real number** — replacing the tiktoken estimate ticked so
    /// far, which never saw the system prompt or the re-sent context. Later
    /// estimates (the next round's stream) tick on top of the snapped base,
    /// and that round's own usage frame snaps again — so the tally is always
    /// "everything billed so far, plus the current round's live estimate".
    /// No-op when no turn is in flight. See `docs/prompt-caching.md`.
    pub fn apply_usage(&mut self, usage: &crate::stream::TokenUsage) {
        if self.status.is_none() {
            return;
        }
        self.turn_usage_tokens += usize::try_from(usage.total()).unwrap_or(usize::MAX);
        self.turn_usage_cached += usize::try_from(usage.cached).unwrap_or(usize::MAX);
        if let Some(status) = self.status.as_mut() {
            status.tokens = self.turn_usage_tokens;
        }
        // The round's `input` is the whole re-sent context and its `output`
        // joins the next round's — their sum is the live context gauge
        // (docs/compact.md), authoritative until a history mutation stales it.
        self.context_used = usage.input.saturating_add(usage.output);
    }

    /// Record that a failed request is being retried, so the status line shows
    /// `retrying {attempt}/{max}` while the backend reconnects. The 1-based
    /// `attempt` and the ceiling `max` come straight from the backend's
    /// [`crate::stream::StreamEvent::Retrying`]. Cleared by the next
    /// [`push_chunk`](App::push_chunk)/[`push_thinking`](App::push_thinking) once
    /// content arrives. No-op when no turn is in flight. See `docs/llm.md`.
    pub fn set_retry(&mut self, attempt: u32, max: u32) {
        if let Some(status) = self.status.as_mut() {
            status.retry = Some(RetryInfo { attempt, max });
        }
    }

    /// The live turn status, if a turn is in flight.
    #[must_use]
    pub const fn status(&self) -> Option<&TurnStatus> {
        self.status.as_ref()
    }

    /// Is a turn in flight (its status line should show)? True from
    /// [`begin_stream`] until the turn ends — drives the loop's per-second tick.
    ///
    /// [`begin_stream`]: App::begin_stream
    #[must_use]
    pub const fn turn_active(&self) -> bool {
        self.status.is_some()
    }

    /// Write the boundary-computed times onto the live status before a draw: how
    /// long the turn has run (`elapsed` — it also drives the verb's shimmer
    /// phase), and the current thinking-phase duration (`Some` while thinking,
    /// `None` otherwise). No-op when no turn is in flight. Time is impure, so it
    /// only ever reaches the status this way.
    pub fn set_status_times(&mut self, elapsed: Duration, thinking: Option<Duration>) {
        if let Some(status) = self.status.as_mut() {
            status.elapsed = elapsed;
            status.thinking = thinking;
        }
    }

    /// Inject the current running command's elapsed each frame (the
    /// [`set_status_times`](App::set_status_times) pattern — the clock lives at
    /// the boundary). `None` when no command is running. Read by the preview to
    /// delay the `(ctrl+b to run in background)` hint until a command has run a
    /// few seconds, so a fast command never flashes it (`docs/background.md`).
    pub fn set_command_elapsed(&mut self, elapsed: Option<Duration>) {
        self.command_elapsed = elapsed;
    }

    /// How long the current running command has executed, or `None` when no
    /// command is running (the boundary hasn't injected one). See
    /// [`set_command_elapsed`](App::set_command_elapsed).
    #[must_use]
    pub fn command_elapsed(&self) -> Option<Duration> {
        self.command_elapsed
    }

    /// Inject the streaming strip preview's row count before a draw (the
    /// [`set_status_times`] pattern): only the boundary's `ui::StreamRender`
    /// knows how many rows the preview renders — one for a normal reply, the
    /// whole forming block while a table streams — and `ui::preview_rows` /
    /// `ui::live_height` / `ui::cursor_position` must all reserve exactly what
    /// the strip draws. See `docs/table-streaming.md`.
    ///
    /// [`set_status_times`]: App::set_status_times
    pub fn set_stream_preview_rows(&mut self, rows: u16) {
        self.stream_preview_rows = rows;
    }

    /// The boundary-injected streaming-preview row count (see
    /// [`set_stream_preview_rows`]; 0 until a draw injects one — consumers
    /// floor at 1, the single-row preview).
    ///
    /// [`set_stream_preview_rows`]: App::set_stream_preview_rows
    #[must_use]
    pub const fn stream_preview_rows(&self) -> u16 {
        self.stream_preview_rows
    }

    /// The reply text accumulated so far, or `None` when idle.
    #[must_use]
    pub fn streaming_text(&self) -> Option<&str> {
        self.streaming.as_deref()
    }

    /// Finish streaming, returning the completed final-segment text (if any),
    /// recording it in the history, and clearing the streaming state. Returns
    /// `None` — recording nothing — when the final segment is empty (a turn that
    /// ended right after a tool call) or nothing was streaming.
    pub fn finish_stream(&mut self) -> Option<String> {
        let text = self.streaming.take()?;
        // A turn can end right after a tool call with no trailing text; don't
        // record (or later commit) a phantom empty assistant message for it. A
        // whitespace-only tail counts as empty too — it renders to zero rows once
        // trailing blanks are trimmed (`ui::assistant_lines`), so recording it
        // would leave a stray `● ` bullet on a repaint.
        if text.trim().is_empty() {
            return None;
        }
        self.record_message(Role::Assistant, text.clone());
        Some(text)
    }

    /// End the turn: record its `"{done verb} for {elapsed_secs}s"` summary in the
    /// history (so it persists across a resize and lists in the transcript) and
    /// clear the live status. Returns the recorded [`TurnSummary`] for the loop to
    /// commit to scrollback, or `None` if no turn was active. The boundary calls
    /// this on `StreamDone`, just after [`finish_stream`] flushes any final text.
    ///
    /// [`finish_stream`]: App::finish_stream
    pub fn end_turn(&mut self, elapsed_secs: u64) -> Option<TurnSummary> {
        let summary = self.take_turn_summary(elapsed_secs)?;
        self.record_turn_summary(summary.clone());
        Some(summary)
    }

    /// Clear the live status and **build** (but do not record) the turn's
    /// `"{done verb} for {n}s"` summary. Split out of [`end_turn`] so the
    /// boundary can settle a background completion that was still pending at
    /// turn end **between** clearing the status and recording the summary —
    /// landing that notice above the `Done for Ns` summary in both history
    /// and scrollback (invariant 3), the same placement a mid-turn tool
    /// boundary gives it (`docs/background.md`). Returns `None` (still
    /// clearing the status) for an idle composer or a `!` shell turn, whose
    /// committed cell is its own record (`docs/shell-command.md`).
    pub fn take_turn_summary(&mut self, elapsed_secs: u64) -> Option<TurnSummary> {
        let status = self.status.take()?;
        // No usage frame arrived this turn (the dummy, or a provider that
        // omits them): the tokenizer estimate stands in for the context gauge
        // so it — and the auto-compact trigger — still work (docs/compact.md).
        if self.turn_usage_tokens == 0 {
            self.refresh_context_used();
        }
        if status.shell {
            return None;
        }
        Some(TurnSummary {
            verb: status.done_verb,
            secs: elapsed_secs,
            timestamp: self.now_stamp(),
            // Snapshot the running background shells for the `· {n} shells
            // still running` suffix (docs/background.md). A shell that just
            // finished has already left `self.background` (at `bg_exited`),
            // so settling its notice next doesn't change this count.
            shells: self.background.len(),
            // The turn's real billed usage (`apply_usage`) — 0 (hidden) when
            // the backend reported none. See docs/prompt-caching.md.
            tokens: self.turn_usage_tokens,
            cached: self.turn_usage_cached,
        })
    }

    /// Record a summary built by [`take_turn_summary`] into history (so it
    /// survives a resize and lists in the transcript).
    pub fn record_turn_summary(&mut self, summary: TurnSummary) {
        self.history.push(HistoryItem::Summary(summary));
    }

    /// End the in-progress stream because the backend reported an error.
    ///
    /// Records any non-empty partial reply as an assistant message, resolves a
    /// still-running tool as [`ToolStatus::Failed`] with [`ERROR_TOOL_OUTPUT`]
    /// (the contract allows `Error` in place of `StreamDone` with a `ToolEnd`
    /// still owed — leaving it Running would wedge a phantom in the preview
    /// strip, the transcript, and a later turn's interrupt record), then
    /// records the error as a [`Role::Error`] message and clears the streaming
    /// state. Returns the [`StreamError`] for the event loop to flush to
    /// scrollback, or `None` if no reply was in progress. The stream-order
    /// mirror of [`App::interrupt_turn`].
    pub fn fail_stream(&mut self, error: &str) -> Option<StreamError> {
        let streamed = self.streaming.take()?;
        // A /compact turn's half summary dies with the request — the marker
        // lands only at StreamDone, so the old context stands (docs/compact.md).
        // A failed compaction also blocks the auto re-trigger until the next
        // real turn (it would fail the same way in a tight loop).
        if self.compact_buffer.take().is_some() {
            self.auto_compact_blocked = true;
        }
        let partial = if streamed.is_empty() {
            None
        } else {
            self.record_message(Role::Assistant, streamed.clone());
            Some(streamed)
        };
        let tool = self.end_tool(ERROR_TOOL_OUTPUT, false);
        // Any un-started `Waiting` batch siblings never ran — drop them (they
        // only ever lived in the live region, never committed). See
        // `docs/parallel-tools.md`.
        self.tool_queue.clear();
        self.record_message(Role::Error, error);
        // An error is the turn's terminal state — clear the live status without a
        // "Done" summary; the red error notice is the summary.
        self.status = None;
        Some(StreamError {
            partial,
            tool,
            error: error.to_string(),
        })
    }

    /// End the in-progress turn because the user interrupted it (Esc). Returns
    /// the [`InterruptedTurn`] telling the loop how to settle the screen, or
    /// `None` if no turn was in flight. Two outcomes (see `docs/interrupt.md`):
    ///
    /// - **No output yet** — nothing streamed (no non-empty partial reply, no
    ///   running tool) and nothing is queued behind this turn: the submission is
    ///   **undone** rather than interrupted. The turn's just-submitted user
    ///   message(s) are pulled back into the composer
    ///   ([`take_trailing_user_messages`] + [`recall_input`]) and dropped from
    ///   history, the status clears, and **no** `Conversation interrupted`
    ///   notice is recorded — there is nothing to keep, so we roll back to the
    ///   pre-submit state (the user's "move it back to the textarea" case). A
    ///   non-empty queue opts out: the user wants their follow-ups sent, so the
    ///   keep path runs instead.
    /// - **Something streamed** — keep any non-empty partial reply as an
    ///   assistant message (codex never retracts streamed text), resolve a
    ///   still-running tool as [`ToolStatus::Failed`] with
    ///   [`INTERRUPT_TOOL_OUTPUT`], and record the [`INTERRUPT_NOTICE`] as a
    ///   [`Role::Error`] message — **except for a `!` shell turn**, whose
    ///   `⎿ Interrupted by user` cell already says it, so no redundant notice is
    ///   committed. The live status clears **without** a `Done for Ns` summary
    ///   (like [`App::fail_stream`], the notice — or the shell cell — is the
    ///   turn's terminal state).
    ///
    /// [`take_trailing_user_messages`]: App::take_trailing_user_messages
    /// [`recall_input`]: App::recall_input
    pub fn interrupt_turn(&mut self) -> Option<InterruptedTurn> {
        if !self.is_streaming() && !self.turn_active() {
            return None;
        }
        // A /compact turn: drop the half-streamed summary — the swap happens
        // only at StreamDone (codex replaces history at the very end), so an
        // interrupt leaves the old context standing. Taking the buffer also
        // keeps the undo branch below from firing: the history tail may be an
        // unrelated user message (the marker was never appended), and pulling
        // it into the composer would undo a submission this turn never made.
        // See docs/compact.md.
        let compacting = self.compact_buffer.take().is_some();
        if compacting {
            // The user said no — don't restart it at this same turn end
            // (docs/compact.md's one-attempt-per-turn rule).
            self.auto_compact_blocked = true;
        }
        // Take the partial first (empties the streaming buffer either way).
        let partial = self.streaming.take().filter(|text| !text.is_empty());

        // No output produced (no partial, no running or waiting tool) and
        // nothing queued behind it → undo the submission wholesale: roll the
        // user's message(s) back into the composer and drop them from history,
        // recording no notice. The tool queue is peeked (not resolved) so the
        // undo path never touches history or the token tally. A shell turn
        // always has a running tool, so it can never land here — it takes the
        // keep path below (and skips the notice there); likewise any tool of a
        // parallel batch (running *or* still `Waiting`) keeps the queue
        // non-empty, so the undo can't fire mid-batch.
        //
        // The trailing-user-message guard is what makes this safe under the
        // real backend's **multi-round tool loop**: after an earlier round
        // committed an assistant segment + a tool, the streaming buffer is
        // `Some("")` and the tool queue is empty again, so the three empties
        // alone would wrongly fire the undo mid-turn — dropping the notice,
        // orphaning the committed output, and (via `recall_input("")`) wiping a
        // typed draft. When the just-submitted user message(s) are still the
        // history tail, nothing has been committed since, so the undo is the
        // right call; otherwise this turn already produced output and the Kept
        // path below records the notice. See `docs/tools.md` / `docs/interrupt.md`.
        let submission_is_intact =
            matches!(self.history.last(), Some(HistoryItem::Message(m)) if m.role == Role::User);
        if !compacting
            && submission_is_intact
            && partial.is_none()
            && self.tool_queue.is_empty()
            && self.queued.is_empty()
        {
            self.status = None;
            let (text, pairs) = self.take_trailing_user_messages();
            // An /init turn's recorded message is the canned prompt, but the
            // user typed "/init" — restoring 1.8KB of text the composer never
            // held would flood it (and a follow-up Ctrl+C would record the
            // prompt into ↑ recall, breaking docs/init.md's no-recall
            // guarantee). Recall the command instead; refresh_command_menu
            // reopens the palette on it — the exact pre-submit state. (A
            // hand-typed submission of the identical text lands here too;
            // "/init" resubmits the same prompt, so nothing is lost.)
            let text = if text == INIT_PROMPT.trim_end() {
                "/init".to_string()
            } else {
                text
            };
            self.recall_input(&text);
            // recall_input replaced the draft, unanchoring any pairs that
            // backed it — discard those (the boundary deletes the orphaned
            // temp files) and restore the undone submission's own pairs so
            // the placeholders back in the composer are backed again.
            let stale = std::mem::take(&mut self.images);
            self.discarded_images
                .extend(stale.into_iter().map(|(_, path)| path));
            self.images = pairs;
            return Some(InterruptedTurn::Undone);
        }

        // Keep what streamed, in stream order: the partial (Assistant) before
        // the tool the interrupt resolves (a ToolStart flushes the buffer, so
        // the two are never both non-empty — but the order holds regardless).
        if let Some(text) = &partial {
            self.record_message(Role::Assistant, text.clone());
        }
        let tool = self.end_tool(INTERRUPT_TOOL_OUTPUT, false);
        // Any un-started `Waiting` batch siblings never ran — drop them (they
        // only lived in the live region, never committed). See
        // `docs/parallel-tools.md`.
        self.tool_queue.clear();
        // A `!` shell turn's `⎿ Interrupted by user` cell is its own record —
        // committing a second `Conversation interrupted` line would be
        // redundant, so skip the notice for it.
        let notice = if self.status.as_ref().is_some_and(|s| s.shell) {
            None
        } else {
            self.record_message(Role::Error, INTERRUPT_NOTICE);
            Some(INTERRUPT_NOTICE)
        };
        self.status = None;
        Some(InterruptedTurn::Kept {
            partial,
            tool,
            notice,
        })
    }

    /// Remove and return the turn's just-submitted user message(s) — the
    /// maximal run of trailing [`Role::User`] messages in [`history`] — joined
    /// with newlines (a batch flushed as one turn records several; codex's
    /// Alt+Up joins them the same way), plus their image attachments rebuilt
    /// as `(placeholder, path)` pairs: each message's paths are recorded in
    /// its text's placeholder-occurrence order (`paste::distribute_images`),
    /// so zipping the occurrences back over them per message is exact — a
    /// duplicate `[Image #1]` across a merged batch re-keys to each message's
    /// own path. The undo path of [`interrupt_turn`] hands the text to
    /// [`recall_input`] and the pairs to the composer. Empty when there is no
    /// trailing user message (a bare `begin_stream` with no submission). A
    /// previous turn always ends with a summary, notice, or tool — never a
    /// user message — so this only ever reclaims the current turn's own input.
    ///
    /// [`history`]: App::history
    /// [`interrupt_turn`]: App::interrupt_turn
    /// [`recall_input`]: App::recall_input
    fn take_trailing_user_messages(&mut self) -> (String, Vec<(String, PathBuf)>) {
        let mut messages = Vec::new();
        while self.history.len() > self.undo_floor
            && matches!(self.history.last(), Some(HistoryItem::Message(m)) if m.role == Role::User)
        {
            if let Some(HistoryItem::Message(m)) = self.history.pop() {
                messages.push(m);
            }
        }
        if !messages.is_empty() {
            self.history_generation += 1;
        }
        messages.reverse();
        let mut pairs = Vec::new();
        let mut texts = Vec::with_capacity(messages.len());
        for message in messages {
            pairs.extend(
                crate::paste::image_placeholder_occurrences(&message.text)
                    .into_iter()
                    .zip(message.images),
            );
            texts.push(message.text);
        }
        (texts.join("\n"), pairs)
    }

    /// Wipe the conversation to a fresh slate — the `/clear` effect. History,
    /// any in-flight turn state (the streaming buffer, a running tool, the
    /// live status), and the queued backlog all go, and *nothing* is recorded
    /// — no partial, no interrupt notice, no summary: the user asked for a
    /// blank screen, not a finished turn. Mid-turn the loop also kills the
    /// backend and drains its channel before repainting (`main.rs`), so a
    /// stale chunk can't repopulate the cleared state. (Codex instead
    /// *disables* `/new`/`/clear` while a task runs — `available_during_task`
    /// — killing is our deliberate divergence.) The ↑-recall input history
    /// survives: drafts aren't part of the conversation
    /// (`clear_command_keeps_the_recall_history`).
    fn clear_conversation(&mut self) {
        self.history.clear();
        self.history_generation += 1;
        self.streaming = None;
        self.compact_buffer = None;
        // A fresh slate — but the system prompt and the standing AGENTS.md
        // instructions still ride the next request, so the gauge re-seats on
        // the estimate rather than hard-zeroing (a bare session still reads
        // 0). Auto-compact re-arms.
        self.refresh_context_used();
        self.auto_compact_blocked = false;
        self.tool_queue.clear();
        self.status = None;
        // The wiped batches' image attachments will never dispatch — record
        // their temp files as discarded so the boundary removes them.
        for entry in self.queued.drain(..) {
            if let QueuedTurn::Messages { images, .. } = entry {
                self.discarded_images
                    .extend(images.into_iter().map(|(_, path)| path));
            }
        }
        self.file_search = None;
        self.undo_floor = 0;
        // A cleared slate shows nothing lingering above the box.
        self.toast = None;
        // The background shells go too — the loop's Clear arm kills their
        // processes; wiping the state here means the late Exited events find
        // nothing and owe no notice (docs/background.md).
        self.background.clear();
        self.background_view = None;
        self.pending_bg.clear();
        self.had_background = false;
    }
}

/// The completion-notice tail of a shell's output: the last
/// [`BG_NOTICE_TAIL_MAX_LINES`] lines, additionally capped at
/// [`BG_NOTICE_TAIL_MAX_BYTES`] (front-trimmed on line, then char,
/// boundaries) — what rides the notice into the model's context.
fn notice_tail(output: &str) -> String {
    let trimmed = output.trim_end_matches('\n');
    if trimmed.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = trimmed.split('\n').collect();
    let keep = lines.len().min(BG_NOTICE_TAIL_MAX_LINES);
    let mut tail = lines[lines.len() - keep..].join("\n");
    if tail.len() > BG_NOTICE_TAIL_MAX_BYTES {
        let cut = tail.len() - BG_NOTICE_TAIL_MAX_BYTES;
        let boundary = tail[cut..]
            .find('\n')
            .map_or_else(|| ceil_char_boundary(&tail, cut), |nl| cut + nl + 1);
        tail.drain(..boundary);
    }
    tail
}

/// The smallest char boundary in `s` at or after `at` (a dependency-free
/// `str::ceil_char_boundary`, which is still unstable).
fn ceil_char_boundary(s: &str, at: usize) -> usize {
    let mut at = at.min(s.len());
    while at < s.len() && !s.is_char_boundary(at) {
        at += 1;
    }
    at
}

/// How many earlier occurrences of the placeholder at `span` precede it in
/// `text` — the ordinal pairing a placeholder occurrence to its list entry
/// (occurrences in text order correspond to `(placeholder, value)` pairs in
/// list order; see `paste::distribute_images`).
fn occurrence_ordinal(text: &str, span: &Range<usize>) -> usize {
    let placeholder = &text[span.clone()];
    text[..span.start].matches(placeholder).count()
}

/// Remove and return the value of the pair backing the `ordinal`-th occurrence
/// of `placeholder`; `None` when no pair sits at that ordinal (the occurrence
/// was an unbacked marker recalled as plain text).
fn remove_nth_pair<V>(
    pairs: &mut Vec<(String, V)>,
    placeholder: &str,
    ordinal: usize,
) -> Option<V> {
    let pos = pairs
        .iter()
        .enumerate()
        .filter(|(_, (ph, _))| ph == placeholder)
        .map(|(i, _)| i)
        .nth(ordinal)?;
    Some(pairs.remove(pos).1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// The roles of the `Message` items in history, in order (tools/summaries
    /// skipped).
    fn roles(app: &App) -> Vec<Role> {
        app.history
            .iter()
            .filter_map(|item| match item {
                HistoryItem::Message(m) => Some(m.role),
                _ => None,
            })
            .collect()
    }

    /// The `Message` at history index `i` (panics if it is not a message).
    fn message_at(app: &App, i: usize) -> &Message {
        match &app.history[i] {
            HistoryItem::Message(m) => m,
            _ => panic!("expected a message at history[{i}]"),
        }
    }

    #[test]
    fn typing_appends_characters_to_input() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('h')));
        app.on_key(key(KeyCode::Char('i')));
        assert_eq!(app.input.text(), "hi");
    }

    #[test]
    fn backspace_removes_last_character() {
        let mut app = App::new();
        app.input = TextArea::from_text("hi");
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.input.text(), "h");
    }

    #[test]
    fn backspace_on_empty_input_is_harmless() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.input.text(), "");
    }

    // ===== large-paste placeholders (docs/paste.md) =====

    #[test]
    fn large_paste_shows_a_placeholder_not_the_raw_text() {
        let mut app = App::new();
        let big = "x".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
        app.on_paste(&big);
        assert_eq!(app.input.text(), "[Pasted Content 1001 chars]");
        assert_eq!(app.pasted.len(), 1, "the real text is remembered");
        assert_eq!(app.pasted[0].1, big);
    }

    #[test]
    fn small_paste_inserts_inline() {
        let mut app = App::new();
        app.on_paste("just a little");
        assert_eq!(app.input.text(), "just a little");
        assert!(app.pasted.is_empty());
    }

    #[test]
    fn paste_inserts_at_the_cursor() {
        let mut app = App::new();
        app.input = TextArea::from_text("ab");
        app.input.move_left(); // cursor between a and b
        app.on_paste("XY");
        assert_eq!(app.input.text(), "aXYb");
    }

    #[test]
    fn paste_normalises_crlf_newlines() {
        let mut app = App::new();
        app.on_paste("a\r\nb");
        assert_eq!(app.input.text(), "a\nb", "CRLF becomes LF");
    }

    #[test]
    fn paste_sanitises_tabs_to_spaces_in_the_composer() {
        // unicode-width counts '\t' as one column but ratatui renders zero
        // cells, so a tab in the composer drifts the hardware cursor one
        // column right of the text. Pastes are the only way a tab can get in
        // (the Tab key is intercepted); sanitise them to a space.
        let mut app = App::new();
        app.on_paste("ab\tcd");
        assert_eq!(app.input.text(), "ab cd");
    }

    #[test]
    fn paste_sanitises_other_control_characters_too() {
        let mut app = App::new();
        app.on_paste("a\u{7f}b\u{1b}c");
        assert_eq!(app.input.text(), "a b c", "DEL and ESC become spaces");
    }

    #[test]
    fn paste_keeps_newlines_while_sanitising() {
        let mut app = App::new();
        app.on_paste("a\t\nb");
        assert_eq!(app.input.text(), "a \nb", "'\\n' is the one control kept");
    }

    #[test]
    fn large_paste_stores_the_raw_text_but_sends_it_verbatim() {
        // The placeholder path never renders the payload in the composer, so
        // the stored text keeps its tabs for send fidelity.
        let mut app = App::new();
        let big = format!(
            "x\t{}",
            "y".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD)
        );
        app.on_paste(&big);
        assert_eq!(app.pasted[0].1, big, "the remembered text is untouched");
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Submit(big), "expanded with the tab intact");
    }

    #[test]
    fn large_paste_expands_to_full_text_on_submit() {
        let mut app = App::new();
        let big = "y".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 5);
        app.on_paste(&big);
        assert!(
            app.input.text().starts_with("[Pasted Content"),
            "the composer shows the placeholder"
        );
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Submit(big), "but the full text is sent");
        assert!(app.pasted.is_empty(), "consumed on submit");
    }

    #[test]
    fn two_large_pastes_both_expand_on_submit() {
        let mut app = App::new();
        let a = "a".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
        let b = "b".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 2);
        app.on_paste(&a);
        app.on_key(key(KeyCode::Char(' ')));
        app.on_paste(&b);
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Submit(format!("{a} {b}")));
    }

    #[test]
    fn backspace_removes_a_whole_placeholder_atomically() {
        let mut app = App::new();
        let big = "z".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
        app.on_paste(&big);
        assert_eq!(app.input.text(), "[Pasted Content 1001 chars]");
        // A single Backspace removes the entire placeholder, not one character.
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.input.text(), "");
        assert!(app.pasted.is_empty(), "the remembered paste is dropped too");
    }

    #[test]
    fn delete_removes_a_whole_placeholder_atomically() {
        let mut app = App::new();
        let big = "z".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
        app.on_paste(&big);
        app.on_key(key(KeyCode::Home)); // cursor to the placeholder's start
        app.on_key(key(KeyCode::Delete));
        assert_eq!(app.input.text(), "");
        assert!(app.pasted.is_empty());
    }

    #[test]
    fn backspace_before_a_placeholder_deletes_only_the_preceding_char() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('x')));
        let big = "z".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
        app.on_paste(&big); // input: "x[Pasted Content 1001 chars]", cursor at end
        app.on_key(key(KeyCode::Home));
        app.on_key(key(KeyCode::Right)); // between 'x' and the placeholder
        app.on_key(key(KeyCode::Backspace)); // deletes 'x', placeholder intact
        assert_eq!(app.input.text(), "[Pasted Content 1001 chars]");
        assert_eq!(app.pasted.len(), 1, "the placeholder and its paste survive");
    }

    // ===== Ctrl+V image paste (docs/image-paste.md) =====

    #[test]
    fn record_user_message_with_images_attaches_the_paths() {
        // The submit path records the turn's attachments onto the user
        // message so the conversation context re-sends them (docs/context.md).
        let mut app = App::new();
        app.record_user_message_with_images("[Image #1] look", vec![PathBuf::from("/tmp/a.png")]);
        let Some(HistoryItem::Message(message)) = app.history.last() else {
            panic!("a user message was recorded");
        };
        assert_eq!(message.role, Role::User);
        assert_eq!(message.text, "[Image #1] look");
        assert_eq!(message.images, vec![PathBuf::from("/tmp/a.png")]);
    }

    #[test]
    fn record_user_message_records_no_images() {
        let mut app = App::new();
        app.record_user_message("plain");
        let Some(HistoryItem::Message(message)) = app.history.last() else {
            panic!("a user message was recorded");
        };
        assert!(message.images.is_empty());
    }

    #[test]
    fn ctrl_v_requests_an_image_paste() {
        let mut app = App::new();
        assert_eq!(app.on_key(ctrl('v')), Action::PasteImage);
    }

    #[test]
    fn ctrl_alt_v_also_requests_an_image_paste() {
        // The WSL-friendly alias — codex binds both Ctrl+V and Ctrl+Alt+V.
        let mut app = App::new();
        let key = KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert_eq!(app.on_key(key), Action::PasteImage);
    }

    #[test]
    fn attach_image_inserts_a_placeholder_and_records_the_path() {
        let mut app = App::new();
        app.attach_image(PathBuf::from("/tmp/a.png"));
        assert_eq!(app.input.text(), "[Image #1]");
        assert_eq!(app.images.len(), 1);
        assert_eq!(app.images[0].1, PathBuf::from("/tmp/a.png"));
    }

    #[test]
    fn attach_image_numbers_each_attachment() {
        let mut app = App::new();
        app.attach_image(PathBuf::from("/tmp/a.png"));
        app.attach_image(PathBuf::from("/tmp/b.png"));
        assert_eq!(app.input.text(), "[Image #1][Image #2]");
        assert_eq!(app.images.len(), 2);
    }

    #[test]
    fn attach_image_inserts_at_the_cursor() {
        let mut app = App::new();
        app.input = TextArea::from_text("ab");
        app.input.move_left(); // cursor between a and b
        app.attach_image(PathBuf::from("/tmp/a.png"));
        assert_eq!(app.input.text(), "a[Image #1]b");
    }

    #[test]
    fn submitting_keeps_the_image_placeholder_and_surfaces_the_path() {
        // Unlike a text paste (expanded on send), an image placeholder STAYS in
        // the message text; the path travels the separate submission channel.
        let mut app = App::new();
        app.attach_image(PathBuf::from("/tmp/a.png"));
        for c in " describe".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Submit("[Image #1] describe".to_string()));
        assert_eq!(
            app.take_submission_images(),
            vec![("[Image #1]".to_string(), PathBuf::from("/tmp/a.png"))]
        );
    }

    #[test]
    fn one_backspace_removes_the_whole_image_placeholder_and_drops_the_path() {
        let mut app = App::new();
        app.attach_image(PathBuf::from("/tmp/a.png")); // cursor at the end of it
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.input.text(), "");
        assert!(
            app.images.is_empty(),
            "the path is dropped with the placeholder"
        );
    }

    #[test]
    fn clearing_the_draft_with_ctrl_c_drops_attached_images() {
        // Ctrl+C empties the composer; its attachments go too, so they can't leak
        // onto a later submit.
        let mut app = App::new();
        app.attach_image(PathBuf::from("/tmp/a.png"));
        app.on_key(ctrl('c'));
        assert!(app.images.is_empty());
        assert!(app.take_submission_images().is_empty());
    }

    // ===== discarded temp-PNG bookkeeping (docs/image-paste.md: the boundary
    // deletes the files of attachments that will never be submitted) =====

    #[test]
    fn deleting_an_image_placeholder_discards_its_temp_path() {
        let mut app = App::new();
        app.attach_image(PathBuf::from("/tmp/a.png"));
        app.on_key(key(KeyCode::Backspace)); // atomic placeholder delete
        assert_eq!(
            app.take_discarded_images(),
            vec![PathBuf::from("/tmp/a.png")],
            "the orphaned temp file is handed to the boundary to remove"
        );
        assert!(app.take_discarded_images().is_empty(), "drained once");
    }

    #[test]
    fn ctrl_c_clearing_a_draft_discards_its_image_paths() {
        let mut app = App::new();
        app.attach_image(PathBuf::from("/tmp/a.png"));
        app.on_key(ctrl('c'));
        assert_eq!(
            app.take_discarded_images(),
            vec![PathBuf::from("/tmp/a.png")]
        );
    }

    #[test]
    fn clear_command_discards_the_queued_batches_image_paths() {
        // /clear wipes the queued backlog; the images riding those batches
        // will never dispatch, so their temp files must not leak.
        let mut app = App::new();
        app.begin_stream();
        app.attach_image(PathBuf::from("/tmp/q.png"));
        app.on_key(key(KeyCode::Enter)); // queue the draft mid-turn
        type_str(&mut app, "/clear");
        app.on_key(key(KeyCode::Enter)); // run the highlighted /clear
        assert!(app.queued.is_empty());
        assert_eq!(
            app.take_discarded_images(),
            vec![PathBuf::from("/tmp/q.png")]
        );
    }

    #[test]
    fn submitted_and_queued_images_are_never_discarded() {
        // The idle submit stages its paths; a queued batch carries its own —
        // neither is a drop, so no file may be deleted underneath them.
        let mut app = App::new();
        app.attach_image(PathBuf::from("/tmp/sent.png"));
        app.on_key(key(KeyCode::Enter)); // idle submit
        assert!(app.take_discarded_images().is_empty());
        app.begin_stream();
        app.attach_image(PathBuf::from("/tmp/queued.png"));
        app.on_key(key(KeyCode::Enter)); // queue mid-turn
        assert!(app.drain_next_batch().is_some());
        assert!(app.take_discarded_images().is_empty());
    }

    #[test]
    fn count_input_images_adds_to_the_up_tally() {
        let mut app = App::new();
        app.begin_stream();
        app.count_input_images(2);
        let status = app.status().unwrap();
        assert!(status.tokens > 0, "attached images are counted as input");
        assert_eq!(status.arrow, TokenArrow::Up, "uploaded input → ↑");
    }

    #[test]
    fn record_error_message_appends_a_red_error_to_history() {
        // The loop records a Ctrl+V clipboard failure here (so it repaints on a
        // resize) before committing the red notice to scrollback.
        let mut app = App::new();
        app.record_error_message("Failed to paste image: no image on the clipboard");
        match app.history.last() {
            Some(HistoryItem::Message(m)) => {
                assert_eq!(m.role, Role::Error);
                assert!(m.text.starts_with("Failed to paste image"));
            }
            other => panic!("expected an error message, got {other:?}"),
        }
    }

    #[test]
    fn enter_with_text_submits_and_clears_input() {
        let mut app = App::new();
        app.input = TextArea::from_text("hello");
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Submit("hello".to_string()));
        assert_eq!(app.input.text(), "");
    }

    #[test]
    fn enter_with_blank_input_does_nothing() {
        let mut app = App::new();
        app.input = TextArea::from_text("   ");
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::None);
        // whitespace-only input is left untouched
        assert_eq!(app.input.text(), "   ");
    }

    #[test]
    fn enter_while_streaming_does_not_submit() {
        // A turn is in flight: Enter never produces Submit — it queues the
        // message (codex's queued_user_messages) and consumes the composer.
        let mut app = App::new();
        app.input = TextArea::from_text("hello");
        app.begin_stream();
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::None);
        assert_eq!(
            app.input.text(),
            "",
            "the composer is consumed into the queue"
        );
        assert_eq!(app.queued.front(), Some(&batch(&["hello"])));
    }

    #[test]
    fn alt_enter_inserts_a_newline_instead_of_submitting() {
        let mut app = App::new();
        app.input = TextArea::from_text("line one");
        let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
        assert_eq!(app.on_key(alt_enter), Action::None);
        assert_eq!(
            app.input.text(),
            "line one\n",
            "Alt+Enter appends a newline"
        );
    }

    #[test]
    fn shift_enter_inserts_a_newline_too() {
        // Terminals with enhanced keyboard support report Shift+Enter; treat it as
        // a newline like Alt+Enter so the box grows on demand.
        let mut app = App::new();
        app.input = TextArea::from_text("a");
        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(app.on_key(shift_enter), Action::None);
        assert_eq!(app.input.text(), "a\n");
    }

    #[test]
    fn ctrl_j_inserts_a_newline_too() {
        // Ctrl+J is the *universal* newline key: in raw mode the byte 0x0A parses
        // to Char('j')+CONTROL on every terminal (no keyboard enhancement needed),
        // so it's the reliable fallback when a terminal can't report Shift+Enter
        // (codex binds Ctrl+J the same way). See docs/shift-enter.md.
        let mut app = App::new();
        app.input = TextArea::from_text("a");
        let ctrl_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(app.on_key(ctrl_j), Action::None);
        assert_eq!(app.input.text(), "a\n");
    }

    #[test]
    fn ctrl_j_grows_input_while_streaming_without_submitting() {
        // Editing (incl. newlines) is allowed mid-stream; only sending is blocked.
        let mut app = App::new();
        app.input = TextArea::from_text("draft");
        app.begin_stream();
        let ctrl_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(app.on_key(ctrl_j), Action::None);
        assert_eq!(app.input.text(), "draft\n");
    }

    #[test]
    fn plain_enter_submits_a_multi_line_message_intact() {
        // After Alt+Enter newlines, a plain Enter submits the whole thing.
        let mut app = App::new();
        app.input = TextArea::from_text("first\nsecond");
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Submit("first\nsecond".to_string()));
        assert_eq!(app.input.text(), "");
    }

    #[test]
    fn alt_enter_grows_input_while_streaming_without_submitting() {
        // Editing (incl. newlines) is allowed mid-stream; only sending is blocked.
        let mut app = App::new();
        app.input = TextArea::from_text("draft");
        app.begin_stream();
        let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
        assert_eq!(app.on_key(alt_enter), Action::None);
        assert_eq!(app.input.text(), "draft\n");
    }

    #[test]
    fn esc_quits() {
        let mut app = App::new();
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Quit);
    }

    #[test]
    fn ctrl_c_quits() {
        let mut app = App::new();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.on_key(ctrl_c), Action::Quit);
    }

    // --- Ctrl+C clears a non-empty input before it quits (codex-style) ---

    #[test]
    fn ctrl_c_with_text_in_the_input_clears_it_instead_of_quitting() {
        let mut app = App::new();
        app.input = TextArea::from_text("a long draft the user no longer wants");
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.on_key(ctrl_c), Action::None, "first Ctrl+C only clears");
        assert!(app.input.is_empty(), "the draft is gone");
        assert_eq!(
            app.on_key(ctrl_c),
            Action::Quit,
            "the next Ctrl+C (empty input) quits as before"
        );
    }

    #[test]
    fn ctrl_c_clearing_a_command_token_also_closes_the_palette() {
        let mut app = App::new();
        type_str(&mut app, "/qu");
        assert!(app.command_menu.is_some(), "the palette opened");
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.on_key(ctrl_c), Action::None);
        assert!(app.input.is_empty());
        assert!(
            app.command_menu.is_none(),
            "an emptied input is no longer a /token — the palette closes"
        );
    }

    #[test]
    fn ctrl_c_clearing_an_at_token_also_closes_the_file_picker() {
        let mut app = App::new();
        type_str(&mut app, "@src");
        assert!(app.file_search.is_some(), "the @ picker opened");
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.on_key(ctrl_c), Action::None);
        assert!(app.input.is_empty());
        assert!(
            app.file_search.is_none(),
            "an emptied input is no longer an @token — the picker closes"
        );
    }

    #[test]
    fn ctrl_c_clears_the_draft_even_mid_stream() {
        let mut app = App::new();
        app.begin_stream();
        app.input = TextArea::from_text("draft");
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.on_key(ctrl_c), Action::None);
        assert!(app.input.is_empty());
        assert!(
            app.turn_active(),
            "clearing the draft never touches the turn"
        );
    }

    #[test]
    fn ctrl_c_still_quits_from_the_tool_view_even_with_a_draft() {
        // The overlay never shows the input box, so there is nothing to clear
        // there — Ctrl+C keeps meaning quit (smoke Phase 7 relies on it).
        let mut app = App::new();
        app.input = TextArea::from_text("draft");
        app.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert_eq!(app.view, View::ToolOutput);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.on_key(ctrl_c), Action::Quit);
    }

    // ===== ↑/↓ input history (shell-style recall — docs/input-history.md) =====

    /// Submit `text` through the real Enter path so it gets recorded.
    fn submit(app: &mut App, text: &str) {
        app.input = TextArea::from_text(text);
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Submit(text.to_string())
        );
    }

    /// A bare file match (no score/indices) for the file-picker tests.
    fn fm(path: &str) -> FileMatch {
        FileMatch {
            path: path.to_string(),
            score: 0,
            indices: Vec::new(),
        }
    }

    #[test]
    fn up_recalls_the_last_submitted_message() {
        let mut app = App::new();
        submit(&mut app, "first message");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "first message");
        assert_eq!(
            app.input.cursor(),
            "first message".len(),
            "recall puts the cursor at the end (codex's placement)"
        );
    }

    #[test]
    fn up_steps_back_through_older_messages_and_clamps_at_the_oldest() {
        let mut app = App::new();
        submit(&mut app, "first");
        submit(&mut app, "second");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "second", "newest first");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "first", "then older");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "first", "the oldest entry clamps");
    }

    #[test]
    fn down_steps_forward_and_clears_past_the_newest() {
        let mut app = App::new();
        submit(&mut app, "first");
        submit(&mut app, "second");
        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Up)); // browsing at "first"
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.input.text(), "second", "down steps newer");
        app.on_key(key(KeyCode::Down));
        assert_eq!(
            app.input.text(),
            "",
            "past the newest the composer clears (codex's exit-browsing)"
        );
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.input.text(), "", "a further down is a no-op");
    }

    #[test]
    fn down_when_not_browsing_does_not_recall() {
        let mut app = App::new();
        submit(&mut app, "sent");
        app.on_key(key(KeyCode::Down));
        assert_eq!(
            app.input.text(),
            "",
            "down from an empty composer never recalls — only up enters history"
        );
    }

    #[test]
    fn up_does_not_clobber_a_typed_draft() {
        let mut app = App::new();
        submit(&mut app, "sent earlier");
        type_str(&mut app, "a fresh draft");
        app.on_key(key(KeyCode::Up));
        assert_eq!(
            app.input.text(),
            "a fresh draft",
            "a typed draft is never replaced — the arrow moves the cursor"
        );
    }

    #[test]
    fn editing_a_recalled_message_returns_arrows_to_cursor_movement() {
        let mut app = App::new();
        submit(&mut app, "first");
        submit(&mut app, "second");
        app.on_key(key(KeyCode::Up)); // recall "second"
        app.on_key(key(KeyCode::Char('X'))); // edit it
        assert_eq!(app.input.text(), "secondX");
        app.on_key(key(KeyCode::Up));
        assert_eq!(
            app.input.text(),
            "secondX",
            "an edited recall is a draft — up no longer browses"
        );
    }

    #[test]
    fn recall_resumes_only_from_the_text_edges() {
        let mut app = App::new();
        submit(&mut app, "first");
        submit(&mut app, "second");
        app.on_key(key(KeyCode::Up)); // recall "second", cursor at the end
        app.on_key(key(KeyCode::Left)); // cursor now inside the text
        app.on_key(key(KeyCode::Up));
        assert_eq!(
            app.input.text(),
            "second",
            "an interior cursor means cursor movement, not history"
        );
        app.on_key(key(KeyCode::End)); // back to an edge
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "first", "an edge cursor browses again");
    }

    #[test]
    fn submitting_restarts_browsing_at_the_newest() {
        let mut app = App::new();
        submit(&mut app, "alpha");
        submit(&mut app, "beta");
        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Up)); // browsing at "alpha"
        assert_eq!(app.input.text(), "alpha");
        app.on_key(key(KeyCode::Enter)); // resubmit it
        app.on_key(key(KeyCode::Up));
        assert_eq!(
            app.input.text(),
            "alpha",
            "after a submit, up starts from the newest entry again"
        );
    }

    #[test]
    fn adjacent_duplicate_submissions_collapse_in_history() {
        let mut app = App::new();
        submit(&mut app, "same");
        submit(&mut app, "same");
        assert_eq!(
            app.input_history.entries,
            ["same"],
            "an entry identical to the newest is not re-recorded (codex)"
        );
    }

    #[test]
    fn blank_texts_are_never_recorded() {
        let mut history = InputHistory::default();
        history.record("");
        assert_eq!(history.up(), None, "nothing to recall");
    }

    #[test]
    fn record_queues_the_entry_for_persistence() {
        let mut history = InputHistory::default();
        history.record("git status");
        history.record("cargo test");
        assert_eq!(
            history.take_unpersisted(),
            ["git status", "cargo test"],
            "each genuine append queues its text for the boundary to flush"
        );
        assert!(
            history.take_unpersisted().is_empty(),
            "draining twice yields nothing the second time"
        );
    }

    #[test]
    fn blank_and_duplicate_records_queue_nothing() {
        let mut history = InputHistory::default();
        history.record("");
        history.record("dup");
        history.record("dup"); // adjacent duplicate, collapsed
        assert_eq!(
            history.take_unpersisted(),
            ["dup"],
            "only the one genuine append is queued (blanks + dups are not)"
        );
    }

    #[test]
    fn record_ephemeral_records_but_never_queues_for_disk() {
        let mut history = InputHistory::default();
        history.record_ephemeral("cleared draft");
        assert_eq!(
            history.up(),
            Some("cleared draft".to_string()),
            "an ephemerally-recorded draft still recalls this session"
        );
        assert!(
            history.take_unpersisted().is_empty(),
            "but it is never queued for the persistent file (codex parity)"
        );
    }

    #[test]
    fn ctrl_c_cleared_draft_is_not_persisted() {
        // The cleared draft recalls this session (see the test above) but must
        // not reach disk — it goes through record_ephemeral.
        let mut app = App::new();
        app.input = TextArea::from_text("abandoned");
        app.on_key(ctrl('c'));
        assert!(
            app.take_unpersisted_inputs().is_empty(),
            "a Ctrl+C-cleared draft is recorded ephemerally, never persisted"
        );
    }

    #[test]
    fn submitted_and_queued_inputs_are_persisted() {
        let mut app = App::new();
        submit(&mut app, "sent message");
        assert_eq!(
            app.take_unpersisted_inputs(),
            ["sent message"],
            "an idle submit persists its text"
        );
    }

    #[test]
    fn seed_populates_entries_without_queuing_them() {
        let mut app = App::new();
        app.seed_input_history(vec!["old-a".to_string(), "old-b".to_string()]);
        assert!(
            app.take_unpersisted_inputs().is_empty(),
            "seeded entries are already on disk — never re-queued"
        );
        // Both recall and search see the seeded entries.
        app.on_key(key(KeyCode::Up));
        assert_eq!(
            app.input.text(),
            "old-b",
            "↑ recalls the newest seeded entry"
        );
        assert_eq!(
            app.input_history.search("old"),
            vec![1, 0],
            "Ctrl+R search spans the seeded (cross-session) entries"
        );
    }

    #[test]
    fn a_sent_message_duplicating_a_cleared_draft_is_still_persisted() {
        // A Ctrl+C-cleared draft is recorded ephemerally (not persisted). If the
        // user then sends the SAME text, the persist dedup is against
        // last_persisted (None here), NOT the ephemeral entries tail — so the
        // genuine submission still reaches disk. (The in-memory entries collapse
        // the visual duplicate; persistence does not.)
        let mut app = App::new();
        app.input = TextArea::from_text("deploy prod");
        app.on_key(ctrl('c')); // ephemeral clear — nothing to persist
        assert!(app.take_unpersisted_inputs().is_empty());
        submit(&mut app, "deploy prod"); // the same text, genuinely sent
        assert_eq!(
            app.take_unpersisted_inputs(),
            ["deploy prod"],
            "a sent message is persisted even when it duplicates a cleared ephemeral draft"
        );
    }

    #[test]
    fn a_resent_message_is_not_re_persisted_adjacently() {
        // The persist stream still collapses adjacent duplicates (like the file
        // dedup): sending the same text twice in a row writes it once.
        let mut app = App::new();
        submit(&mut app, "again");
        submit(&mut app, "again");
        assert_eq!(
            app.take_unpersisted_inputs(),
            ["again"],
            "an immediately re-sent message is persisted once, not twice"
        );
    }

    #[test]
    fn seed_collapses_adjacent_duplicates_like_record() {
        // A messy or concurrently-written file can carry adjacent duplicates;
        // seeding replays them through record's collapse, so the buffer looks
        // exactly as a fresh session would build it.
        let mut app = App::new();
        app.seed_input_history(vec!["a".to_string(), "a".to_string(), "b".to_string()]);
        assert_eq!(
            app.input_history.entries,
            ["a", "b"],
            "seed collapses adjacent duplicates like record"
        );
    }

    #[test]
    fn a_seeded_duplicate_of_the_first_submission_is_not_re_persisted() {
        // Seed the last entry, then submit the same text: record collapses it
        // (adjacent duplicate) and queues nothing, so it isn't written twice.
        let mut app = App::new();
        app.seed_input_history(vec!["repeat".to_string()]);
        submit(&mut app, "repeat");
        assert!(
            app.take_unpersisted_inputs().is_empty(),
            "a submission identical to the newest seeded entry is not re-persisted"
        );
    }

    #[test]
    fn ctrl_c_cleared_draft_is_recallable_with_up() {
        // codex's clear_for_ctrl_c records the cleared draft so ↑ undoes it.
        let mut app = App::new();
        app.input = TextArea::from_text("draft the user cleared");
        app.on_key(ctrl('c'));
        assert!(app.input.is_empty());
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "draft the user cleared");
    }

    #[test]
    fn recalling_a_slash_token_reopens_the_palette() {
        let mut app = App::new();
        type_str(&mut app, "/he");
        assert!(app.command_menu.is_some());
        app.on_key(ctrl('c')); // clear (and record) the token, palette closes
        assert!(app.command_menu.is_none());
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "/he");
        assert!(
            app.command_menu.is_some(),
            "a recalled /token re-derives the palette, like typing it"
        );
    }

    #[test]
    fn clear_command_keeps_the_recall_history() {
        let mut app = App::new();
        submit(&mut app, "kept across clear");
        type_str(&mut app, "/clear");
        app.on_key(key(KeyCode::Enter)); // run /clear (wipes the conversation)
        assert!(app.history.is_empty());
        app.on_key(key(KeyCode::Up));
        assert_eq!(
            app.input.text(),
            "kept across clear",
            "/clear wipes the conversation, not the composer's recall"
        );
    }

    // ===== `?` shortcuts band (codex's footer shortcut overlay — docs/shortcuts.md) =====

    #[test]
    fn question_mark_with_an_empty_composer_toggles_the_shortcuts_band() {
        let mut app = App::new();
        assert!(!app.shortcuts_open);
        assert_eq!(app.on_key(key(KeyCode::Char('?'))), Action::None);
        assert!(app.shortcuts_open, "first ? opens the band");
        assert!(app.input.is_empty(), "the ? was consumed, not typed");
        assert_eq!(app.on_key(key(KeyCode::Char('?'))), Action::None);
        assert!(!app.shortcuts_open, "second ? closes it");
        assert!(app.input.is_empty());
    }

    #[test]
    fn shift_question_mark_also_toggles_the_band() {
        // Terminals differ in whether Shift+/ reports SHIFT — codex binds both.
        let mut app = App::new();
        let shift_q = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);
        app.on_key(shift_q);
        assert!(app.shortcuts_open);
    }

    #[test]
    fn question_mark_types_into_a_non_empty_draft() {
        let mut app = App::new();
        type_str(&mut app, "what is this");
        app.on_key(key(KeyCode::Char('?')));
        assert_eq!(app.input.text(), "what is this?");
        assert!(!app.shortcuts_open, "with a draft, ? is just a character");
    }

    #[test]
    fn any_other_key_closes_the_band_but_still_acts() {
        // codex's reset_mode_after_activity: the overlay is display-only, never
        // modal — the key that dismisses it still does its normal job.
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('?')));
        assert!(app.shortcuts_open);
        app.on_key(key(KeyCode::Char('h')));
        assert!(!app.shortcuts_open, "typing closes the band");
        assert_eq!(app.input.text(), "h", "and the character still lands");
    }

    #[test]
    fn esc_only_dismisses_the_shortcuts_band_when_idle() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('?')));
        assert_eq!(
            app.on_key(key(KeyCode::Esc)),
            Action::None,
            "Esc dismisses the band instead of quitting"
        );
        assert!(!app.shortcuts_open);
        assert_eq!(
            app.on_key(key(KeyCode::Esc)),
            Action::Quit,
            "the next Esc (band closed) quits as before"
        );
    }

    #[test]
    fn esc_dismissing_the_band_wins_over_interrupt_mid_turn() {
        let mut app = App::new();
        app.begin_stream();
        app.on_key(key(KeyCode::Char('?'))); // the toggle works mid-turn too
        assert!(app.shortcuts_open);
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        assert!(!app.shortcuts_open);
        assert!(
            app.turn_active(),
            "dismissing the band never touches the turn"
        );
        assert_eq!(
            app.on_key(key(KeyCode::Esc)),
            Action::Interrupt,
            "the next Esc interrupts as usual"
        );
    }

    #[test]
    fn up_recall_closes_the_band_and_still_recalls() {
        let mut app = App::new();
        submit(&mut app, "recall me");
        app.on_key(key(KeyCode::Char('?')));
        app.on_key(key(KeyCode::Up));
        assert!(!app.shortcuts_open);
        assert_eq!(app.input.text(), "recall me");
    }

    #[test]
    fn a_slash_closes_the_band_and_opens_the_palette() {
        // The band and the palette never show together.
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('?')));
        app.on_key(key(KeyCode::Char('/')));
        assert!(!app.shortcuts_open);
        assert!(app.command_menu.is_some());
        assert_eq!(app.input.text(), "/");
    }

    #[test]
    fn question_mark_is_ignored_in_the_tool_view() {
        let mut app = App::new();
        app.on_key(ctrl('o'));
        app.on_key(key(KeyCode::Char('?')));
        assert!(!app.shortcuts_open, "the tool view has no composer or band");
    }

    #[test]
    fn ctrl_o_dismisses_the_band_like_any_other_key() {
        // The band's rule is "any key but Esc closes it, then acts normally"
        // (docs/shortcuts.md) — the global Ctrl+O arm is no exception, so the
        // band must not re-show under the box after the overlay round-trip.
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('?')));
        assert!(app.shortcuts_open);
        app.on_key(ctrl('o')); // into the overlay
        assert!(!app.shortcuts_open, "opening the overlay closes the band");
        app.on_key(ctrl('o')); // and back
        assert!(
            !app.shortcuts_open,
            "the band stays closed after the round-trip"
        );
    }

    #[test]
    fn begin_stream_starts_empty_streaming_buffer() {
        let mut app = App::new();
        assert!(!app.is_streaming());
        app.begin_stream();
        assert!(app.is_streaming());
        assert_eq!(app.streaming_text(), Some(""));
    }

    #[test]
    fn push_chunk_accumulates_into_streaming_buffer() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("Hello, ");
        app.push_chunk("world");
        assert_eq!(app.streaming_text(), Some("Hello, world"));
    }

    #[test]
    fn finish_stream_returns_text_and_clears_state() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("done");
        let finished = app.finish_stream();
        assert_eq!(finished, Some("done".to_string()));
        assert!(!app.is_streaming());
        assert_eq!(app.streaming_text(), None);
    }

    #[test]
    fn finish_stream_when_idle_returns_none() {
        let mut app = App::new();
        assert_eq!(app.finish_stream(), None);
    }

    #[test]
    fn record_user_message_appends_to_history() {
        let mut app = App::new();
        app.record_user_message("hello");
        assert_eq!(app.history.len(), 1);
        assert_eq!(message_at(&app, 0).role, Role::User);
        assert_eq!(message_at(&app, 0).text, "hello");
    }

    #[test]
    fn finish_stream_records_the_assistant_message_in_history() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("hi there");
        app.finish_stream();
        assert_eq!(
            app.history.last(),
            Some(&HistoryItem::Message(Message {
                role: Role::Assistant,
                text: "hi there".to_string(),
                timestamp: String::new(),
                images: Vec::new(),
            }))
        );
    }

    #[test]
    fn finish_stream_when_idle_records_nothing() {
        let mut app = App::new();
        app.finish_stream();
        assert!(app.history.is_empty());
    }

    #[test]
    fn a_full_turn_records_user_then_assistant_in_order() {
        let mut app = App::new();
        app.record_user_message("q");
        app.begin_stream();
        app.push_chunk("a");
        app.finish_stream();
        assert_eq!(roles(&app), vec![Role::User, Role::Assistant]);
    }

    #[test]
    fn fail_stream_records_partial_then_error_and_clears_state() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("half a rep");
        let failure = app.fail_stream("network down").expect("was streaming");
        assert_eq!(failure.partial.as_deref(), Some("half a rep"));
        assert_eq!(failure.error, "network down");
        assert!(!app.is_streaming());
        assert_eq!(roles(&app), vec![Role::Assistant, Role::Error]);
        assert_eq!(message_at(&app, 1).text, "network down");
    }

    #[test]
    fn fail_stream_with_no_partial_records_only_the_error() {
        let mut app = App::new();
        app.begin_stream(); // errored before any chunk arrived
        let failure = app.fail_stream("died early").expect("was streaming");
        assert!(failure.partial.is_none());
        assert_eq!(roles(&app), vec![Role::Error]);
    }

    #[test]
    fn fail_stream_when_idle_returns_none_and_records_nothing() {
        let mut app = App::new();
        assert!(app.fail_stream("ignored").is_none());
        assert!(app.history.is_empty());
    }

    #[test]
    fn fail_stream_resolves_a_running_tool_as_failed() {
        // The stream contract allows Error in place of StreamDone with a tool
        // still running (a real backend's mid-tool network failure). Like an
        // Esc interrupt, the turn's death must resolve the tool — leaving it
        // Running would wedge a phantom in the preview strip, the transcript,
        // and a later turn's interrupt record.
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Read", "src/app.rs");
        let failure = app.fail_stream("network down").expect("was streaming");
        assert!(app.current_tool().is_none(), "no phantom running tool");
        let tool = failure.tool.expect("the failed tool rides the failure");
        assert_eq!(tool.status, ToolStatus::Failed);
        assert_eq!(tool.output, ERROR_TOOL_OUTPUT);
        // History order: the failed tool slots before the error notice, the
        // same shape a resize repaint (and the scrollback commit) renders.
        assert!(matches!(
            (&app.history[0], &app.history[1]),
            (HistoryItem::Tool(t), HistoryItem::Message(m))
                if t.status == ToolStatus::Failed && m.role == Role::Error
        ));
    }

    #[test]
    fn fail_stream_orders_partial_then_tool_then_notice() {
        // Buffered text streamed before the tool started (in practice a
        // ToolStart flushes it, but the order holds regardless) — mirror
        // interrupt_turn's stream-order: partial, tool, error notice.
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("half a rep");
        app.start_tool("Bash", "ls");
        let failure = app.fail_stream("boom").expect("was streaming");
        assert_eq!(failure.partial.as_deref(), Some("half a rep"));
        assert!(failure.tool.is_some());
        assert!(matches!(
            (&app.history[0], &app.history[1], &app.history[2]),
            (HistoryItem::Message(p), HistoryItem::Tool(_), HistoryItem::Message(e))
                if p.role == Role::Assistant && e.role == Role::Error
        ));
    }

    #[test]
    fn fail_stream_without_a_tool_carries_none() {
        let mut app = App::new();
        app.begin_stream();
        let failure = app.fail_stream("died early").expect("was streaming");
        assert!(failure.tool.is_none());
    }

    // --- Esc interrupts the in-flight turn, codex-style (docs/interrupt.md) ---

    #[test]
    fn esc_interrupts_while_a_turn_is_active() {
        let mut app = App::new();
        app.begin_stream();
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Interrupt);
    }

    #[test]
    fn esc_with_the_palette_open_only_dismisses_it_even_mid_turn() {
        // Codex's "popup wins" rule: dismissing the palette takes precedence
        // over interrupting; the turn keeps running.
        let mut app = App::new();
        app.begin_stream();
        app.on_key(key(KeyCode::Char('/')));
        assert!(app.command_menu.is_some(), "the palette opened");
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        assert!(app.command_menu.is_none(), "Esc closed the palette");
        assert!(app.turn_active(), "the turn was not interrupted");
    }

    #[test]
    fn interrupt_turn_keeps_the_partial_and_records_the_notice() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("half a rep");
        let InterruptedTurn::Kept {
            partial,
            tool,
            notice,
        } = app.interrupt_turn().expect("a turn was active")
        else {
            panic!("a streamed partial is kept, not undone");
        };
        assert_eq!(partial.as_deref(), Some("half a rep"));
        assert!(tool.is_none(), "no tool was running");
        assert_eq!(notice, Some(INTERRUPT_NOTICE), "a normal turn's notice");
        assert!(!app.is_streaming());
        assert!(!app.turn_active(), "the live status cleared");
        assert_eq!(roles(&app), vec![Role::Assistant, Role::Error]);
        assert_eq!(message_at(&app, 0).text, "half a rep");
        assert_eq!(message_at(&app, 1).text, INTERRUPT_NOTICE);
    }

    #[test]
    fn interrupt_turn_with_no_output_undoes_the_submission() {
        // The user submitted "Hi", the backend produced nothing, then Esc:
        // instead of a `Conversation interrupted` notice the whole turn is
        // undone — "Hi" goes back into the composer and out of history, and
        // nothing is recorded (docs/interrupt.md, the "no output yet" case).
        let mut app = App::new();
        app.record_user_message("Hi");
        app.begin_stream();
        app.count_user_input("Hi");
        let outcome = app.interrupt_turn().expect("a turn was active");
        assert_eq!(outcome, InterruptedTurn::Undone);
        assert_eq!(
            app.input.text(),
            "Hi",
            "the message is back in the composer"
        );
        assert!(
            app.history.is_empty(),
            "the user message is dropped from history"
        );
        assert!(!app.turn_active(), "the live status cleared");
        assert!(!app.is_streaming());
    }

    #[test]
    fn interrupt_between_tool_rounds_keeps_the_output_and_records_the_notice() {
        // The real backend's multi-round tool loop can leave the streaming
        // buffer empty (Some("")) with no running tool *after* an earlier round
        // already committed an assistant segment + a tool to history. An Esc in
        // the gap before the next round must NOT undo the turn (that would drop
        // the notice, orphan the committed output, and wipe a typed draft) — it
        // must take the Kept path and record the interrupt notice.
        let mut app = App::new();
        app.record_user_message("fix the bug");
        app.begin_stream();
        // Round 1: an assistant segment, then a finished tool.
        app.push_chunk("let me look");
        app.flush_streaming_segment(); // records the segment, leaves streaming = Some("")
        app.start_tool("Read", "src/app.rs");
        app.end_tool("fn main() {}", true); // pushes HistoryItem::Tool, current_tool = None
        // Round 2 is about to start; nothing has streamed yet this round.
        assert!(
            app.is_streaming(),
            "the stream is still open between rounds"
        );
        assert!(app.current_tool().is_none());

        let outcome = app.interrupt_turn().expect("a turn was active");
        match outcome {
            InterruptedTurn::Kept { notice, .. } => {
                assert_eq!(
                    notice,
                    Some(INTERRUPT_NOTICE),
                    "a turn that already produced output records the interrupt notice"
                );
            }
            InterruptedTurn::Undone => panic!("must not undo a turn that already committed output"),
        }
        // The committed round-1 output survives, plus the interrupt notice.
        assert!(
            app.history
                .iter()
                .any(|h| matches!(h, HistoryItem::Message(m) if m.role == Role::Error)),
            "the interrupt notice is in history"
        );
        assert!(
            app.input.text().is_empty(),
            "the composer draft is left untouched (not clobbered by a bogus recall)"
        );
    }

    #[test]
    fn interrupt_turn_undo_restores_the_submissions_image_attachments() {
        // An undone submission's Ctrl+V attachments come back with it: the
        // placeholders in the restored draft are backed again, so resubmitting
        // sends the images (docs/context.md).
        let mut app = App::new();
        app.attach_image(PathBuf::from("/tmp/a.png"));
        for c in " look".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Submit("[Image #1] look".to_string()));
        let paths = app
            .take_submission_images()
            .into_iter()
            .map(|(_, path)| path)
            .collect();
        app.record_user_message_with_images("[Image #1] look", paths);
        app.begin_stream();
        assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
        assert_eq!(app.input.text(), "[Image #1] look");
        assert_eq!(
            app.images,
            vec![("[Image #1]".to_string(), PathBuf::from("/tmp/a.png"))],
            "the placeholder is backed by its path again"
        );
    }

    #[test]
    fn interrupt_turn_undo_rekeys_a_batchs_duplicate_placeholders_per_message() {
        // Placeholder numbering restarts per draft, so a merged batch can hold
        // two "[Image #1]"s with different paths. Recording stores each path
        // on its own message; the undo re-key zips per message, so neither
        // path is dropped or swapped (the review's duplicate-placeholder bug).
        let mut app = App::new();
        app.record_user_message_with_images("[Image #1] first", vec![PathBuf::from("/a.png")]);
        app.record_user_message_with_images("[Image #1] second", vec![PathBuf::from("/b.png")]);
        app.begin_stream();
        assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
        assert_eq!(app.input.text(), "[Image #1] first\n[Image #1] second");
        assert_eq!(
            app.images,
            vec![
                ("[Image #1]".to_string(), PathBuf::from("/a.png")),
                ("[Image #1]".to_string(), PathBuf::from("/b.png")),
            ],
            "both duplicate-named pairs survive, in message order"
        );
        assert!(
            app.take_discarded_images().is_empty(),
            "nothing leaks to the discard list"
        );
    }

    #[test]
    fn interrupt_turn_undo_discards_a_mid_turn_drafts_attachments() {
        // recall_input clobbers whatever draft was typed mid-turn; its
        // attachments must not linger as invisible pairs — they are discarded
        // (and their temp files handed to the boundary for deletion).
        let mut app = App::new();
        app.record_user_message("Hi");
        app.begin_stream();
        app.attach_image(PathBuf::from("/tmp/draft.png")); // typed mid-turn
        assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
        assert_eq!(app.input.text(), "Hi");
        assert!(app.images.is_empty(), "no unanchored pairs survive");
        assert_eq!(
            app.take_discarded_images(),
            vec![PathBuf::from("/tmp/draft.png")],
            "the clobbered draft's temp file is queued for deletion"
        );
    }

    #[test]
    fn interrupt_turn_undo_rejoins_a_batch_with_newlines() {
        // A queued batch flushes several user messages as one turn; undoing it
        // rejoins them with newlines (codex's Alt+Up shape).
        let mut app = App::new();
        app.record_user_message("first");
        app.record_user_message("second");
        app.begin_stream();
        assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
        assert_eq!(app.input.text(), "first\nsecond");
        assert!(app.history.is_empty());
    }

    #[test]
    fn interrupt_turn_undo_only_reclaims_the_current_turns_message() {
        // A finished prior turn stays put; only the just-submitted message is
        // rolled back (trailing user messages belong to the current turn).
        let mut app = App::new();
        app.record_user_message("old");
        app.record_message(Role::Assistant, "a reply");
        app.record_user_message("new");
        app.begin_stream();
        assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
        assert_eq!(app.input.text(), "new");
        assert_eq!(
            roles(&app),
            vec![Role::User, Role::Assistant],
            "only the new user message was removed"
        );
        assert_eq!(message_at(&app, 0).text, "old");
    }

    #[test]
    fn interrupt_turn_keeps_the_notice_when_a_message_is_queued() {
        // With follow-ups queued the user wants them sent, so Esc interrupts
        // normally (notice committed) rather than undoing — even with no output.
        let mut app = App::new();
        app.record_user_message("Hi");
        app.begin_stream();
        app.queued.push_back(QueuedTurn::Shell("ls".to_string()));
        let outcome = app.interrupt_turn().expect("a turn was active");
        assert!(
            matches!(
                outcome,
                InterruptedTurn::Kept {
                    notice: Some(_),
                    ..
                }
            ),
            "a non-empty queue opts out of the undo"
        );
        assert!(app.input.text().is_empty(), "the composer is untouched");
        assert!(
            roles(&app).contains(&Role::Error),
            "the interrupt notice is recorded"
        );
    }

    #[test]
    fn interrupt_turn_resolves_a_running_tool_as_failed() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("before the tool ");
        app.start_tool("Bash", "sleep 100"); // flush happens loop-side; buffer keeps streaming
        let InterruptedTurn::Kept { tool, .. } = app.interrupt_turn().expect("a turn was active")
        else {
            panic!("streamed output is kept, not undone");
        };
        let tool = tool.expect("the running tool was resolved");
        assert_eq!(tool.status, ToolStatus::Failed);
        assert_eq!(tool.output, INTERRUPT_TOOL_OUTPUT);
        assert!(app.current_tool().is_none(), "no tool left running");
        assert!(
            app.history
                .iter()
                .any(|item| matches!(item, HistoryItem::Tool(t) if t.status == ToolStatus::Failed)),
            "the cancelled tool is recorded in history"
        );
    }

    #[test]
    fn interrupt_mid_batch_keeps_the_running_call_and_drops_waiting_siblings() {
        // Esc during a parallel batch: the running (front) call resolves Failed
        // and is recorded; the un-started `Waiting` siblings never ran, so they
        // are dropped — no cell recorded, the live queue cleared. See
        // `docs/parallel-tools.md`.
        let mut app = App::new();
        app.begin_stream();
        app.start_tool_batch(&ping_batch());
        app.start_tool("Bash", "ping google.com"); // the front is now Running
        let InterruptedTurn::Kept { tool, .. } = app.interrupt_turn().expect("a turn was active")
        else {
            panic!("a running tool means output streamed — kept, not undone");
        };
        let tool = tool.expect("the running call was resolved");
        assert_eq!(tool.args, "ping google.com");
        assert_eq!(tool.status, ToolStatus::Failed);
        assert!(
            app.tool_queue().is_empty(),
            "the waiting siblings were dropped"
        );
        let recorded_tools = app
            .history
            .iter()
            .filter(|i| matches!(i, HistoryItem::Tool(_)))
            .count();
        assert_eq!(
            recorded_tools, 1,
            "only the running call is recorded; waiting siblings leave no cell"
        );
    }

    #[test]
    fn interrupt_turn_records_no_done_summary() {
        // An interrupt has no "Done for Ns" line — the notice is the
        // turn's terminal state, exactly like fail_stream.
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("text");
        app.interrupt_turn().expect("a turn was active");
        assert!(
            !app.history
                .iter()
                .any(|item| matches!(item, HistoryItem::Summary(_))),
            "no summary for an interrupted turn"
        );
    }

    #[test]
    fn interrupt_turn_when_idle_returns_none_and_records_nothing() {
        let mut app = App::new();
        assert!(app.interrupt_turn().is_none());
        assert!(app.history.is_empty());
    }

    #[test]
    fn interrupt_turn_stamps_the_records_with_the_clock() {
        let mut app = App::new();
        app.set_clock(|| "03:20 AM".to_string());
        app.begin_stream();
        app.push_chunk("partial");
        app.interrupt_turn().expect("a turn was active");
        assert_eq!(message_at(&app, 0).timestamp, "03:20 AM");
        assert_eq!(message_at(&app, 1).timestamp, "03:20 AM");
    }

    // --- cursor editing (the codex-style textarea, dispatched from on_key) ---

    #[test]
    fn left_and_right_arrows_move_the_input_cursor() {
        let mut app = App::new();
        type_str(&mut app, "hi");
        assert_eq!(app.input.cursor(), 2, "typing leaves the cursor at the end");
        app.on_key(key(KeyCode::Left));
        assert_eq!(app.input.cursor(), 1);
        app.on_key(key(KeyCode::Right));
        assert_eq!(app.input.cursor(), 2);
    }

    #[test]
    fn typing_inserts_at_the_cursor_not_just_the_end() {
        let mut app = App::new();
        type_str(&mut app, "ac");
        app.on_key(key(KeyCode::Left)); // cursor between a and c
        app.on_key(key(KeyCode::Char('b')));
        assert_eq!(app.input.text(), "abc", "the char lands at the cursor");
    }

    #[test]
    fn backspace_deletes_before_the_cursor_mid_text() {
        let mut app = App::new();
        type_str(&mut app, "axbc");
        app.on_key(key(KeyCode::Home));
        app.on_key(key(KeyCode::Right));
        app.on_key(key(KeyCode::Right)); // cursor just after the 'x'
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(
            app.input.text(),
            "abc",
            "backspace removes the char before it"
        );
    }

    #[test]
    fn delete_key_removes_the_character_at_the_cursor() {
        let mut app = App::new();
        type_str(&mut app, "abc");
        app.on_key(key(KeyCode::Home)); // cursor at the start
        app.on_key(key(KeyCode::Delete));
        assert_eq!(
            app.input.text(),
            "bc",
            "delete removes the char at the cursor"
        );
    }

    #[test]
    fn home_and_end_move_to_the_input_line_bounds() {
        let mut app = App::new();
        type_str(&mut app, "hello");
        app.on_key(key(KeyCode::Home));
        assert_eq!(app.input.cursor(), 0);
        app.on_key(key(KeyCode::End));
        assert_eq!(app.input.cursor(), 5);
    }

    #[test]
    fn up_and_down_move_the_cursor_when_no_palette_is_open() {
        // Two logical lines; the cursor starts at the end (on the 2nd line). With
        // no palette open and a cold wrap cache, ↑/↓ navigate logical lines.
        let mut app = App::new();
        app.input = TextArea::from_text("abc\nde");
        assert!(app.command_menu.is_none());
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.cursor(), 2, "up to column 2 of the first line");
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.input.cursor(), 6, "down to column 2 of the second line");
    }

    #[test]
    fn down_drives_the_palette_not_the_cursor_when_it_is_open() {
        // With the palette open, ↓ moves the highlight (not the text cursor).
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('/'))); // opens the palette, lists all commands
        let before = app.input.cursor();
        app.on_key(key(KeyCode::Down));
        assert_eq!(
            app.command_menu.as_ref().unwrap().selected,
            1,
            "palette moved"
        );
        assert_eq!(app.input.cursor(), before, "the text cursor stayed put");
    }

    #[test]
    fn ctrl_modified_characters_are_not_typed_into_the_input() {
        // Ctrl+<char> (other than the global Ctrl+C/Ctrl+O) is not text.
        let mut app = App::new();
        app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(
            app.input.text(),
            "",
            "control combos don't insert a character"
        );
    }

    // --- tool calls ---

    #[test]
    fn start_tool_marks_a_running_tool_not_yet_in_history() {
        let mut app = App::new();
        app.start_tool("Bash", "cargo test");
        let tool = app.current_tool().expect("a tool is running");
        assert_eq!(tool.name, "Bash");
        assert_eq!(tool.args, "cargo test");
        assert_eq!(tool.status, ToolStatus::Running);
        assert!(app.history.is_empty(), "a running tool is not yet history");
    }

    /// A three-call parallel batch, as the header `name`/`args` summaries.
    fn ping_batch() -> Vec<ToolCallSummary> {
        ["ping google.com", "ping facebook.com", "ping x.com"]
            .iter()
            .map(|cmd| ToolCallSummary {
                name: "Bash".to_string(),
                args: (*cmd).to_string(),
            })
            .collect()
    }

    #[test]
    fn start_tool_batch_queues_every_call_as_waiting() {
        // A parallel batch registers all calls up front, all `Waiting`, so the
        // live region can show each — the not-yet-run ones as `⎿ Waiting…`. The
        // front is the first call (about to run); none are in history yet.
        let mut app = App::new();
        app.start_tool_batch(&ping_batch());
        assert_eq!(app.tool_queue().len(), 3, "all three calls are live");
        assert!(
            app.tool_queue()
                .iter()
                .all(|t| t.status == ToolStatus::Waiting),
            "every batched call starts Waiting"
        );
        let front = app
            .current_tool()
            .expect("the front call is the current one");
        assert_eq!(front.args, "ping google.com");
        assert!(app.history.is_empty(), "a queued batch is not yet history");
    }

    #[test]
    fn start_tool_flips_the_front_waiting_call_to_running_without_adding_one() {
        // Executing the first batch call flips the front `Waiting`→`Running` (it
        // does not push a second call): the siblings stay `Waiting`.
        let mut app = App::new();
        app.start_tool_batch(&ping_batch());
        app.start_tool("Bash", "ping google.com");
        assert_eq!(app.tool_queue().len(), 3, "no extra call was pushed");
        assert_eq!(app.current_tool().unwrap().status, ToolStatus::Running);
        assert_eq!(
            app.tool_queue()[1].status,
            ToolStatus::Waiting,
            "the siblings are still waiting"
        );
        assert_eq!(app.tool_queue()[2].status, ToolStatus::Waiting);
    }

    #[test]
    fn end_tool_pops_the_front_and_the_next_batch_call_becomes_current() {
        // Finishing the running call removes it from the live queue and records
        // it; the next `Waiting` sibling becomes the front (about to run).
        let mut app = App::new();
        app.start_tool_batch(&ping_batch());
        app.start_tool("Bash", "ping google.com");
        let finished = app.end_tool("pong", true).expect("the front call finished");
        assert_eq!(finished.args, "ping google.com");
        assert_eq!(finished.status, ToolStatus::Ok);
        assert_eq!(
            app.tool_queue().len(),
            2,
            "the finished call left the queue"
        );
        assert_eq!(
            app.current_tool().unwrap().args,
            "ping facebook.com",
            "the next sibling is now the front"
        );
        assert_eq!(app.current_tool().unwrap().status, ToolStatus::Waiting);
        assert!(
            matches!(app.history.last(), Some(HistoryItem::Tool(t)) if t.args == "ping google.com"),
            "the finished call is recorded in history"
        );
    }

    #[test]
    fn a_full_batch_runs_in_order_leaving_three_history_tools_and_an_empty_queue() {
        // Drive the whole batch: each call flips to Running then finishes, in
        // order, committing three tools; the live queue ends empty.
        let mut app = App::new();
        app.start_tool_batch(&ping_batch());
        for item in ping_batch() {
            app.start_tool(&item.name, &item.args);
            app.end_tool("done", true);
        }
        assert!(app.tool_queue().is_empty(), "the batch fully drained");
        let tool_args: Vec<&str> = app
            .history
            .iter()
            .filter_map(|i| match i {
                HistoryItem::Tool(t) => Some(t.args.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            tool_args,
            vec!["ping google.com", "ping facebook.com", "ping x.com"],
            "all three committed, in request order"
        );
    }

    #[test]
    fn a_lone_start_tool_without_a_batch_pushes_a_running_call() {
        // The single-tool path (the `!` shell, the dummy's lone Read) is
        // unchanged: with no batch queued, start_tool pushes one Running call.
        let mut app = App::new();
        app.start_tool("Read", "src/main.rs");
        assert_eq!(app.tool_queue().len(), 1);
        assert_eq!(app.current_tool().unwrap().status, ToolStatus::Running);
    }

    #[test]
    fn end_tool_records_a_successful_tool_call_and_clears_the_slot() {
        let mut app = App::new();
        app.start_tool("Read", "src/main.rs");
        let finished = app
            .end_tool("line1\nline2", true)
            .expect("a tool was running");
        assert_eq!(finished.status, ToolStatus::Ok);
        assert!(app.current_tool().is_none(), "running slot cleared");
        assert_eq!(
            app.history.last(),
            Some(&HistoryItem::Tool(ToolCall {
                name: "Read".to_string(),
                args: "src/main.rs".to_string(),
                status: ToolStatus::Ok,
                output: "line1\nline2".to_string(),
                timestamp: String::new(),
                shell: false,
                truncated: false,
            }))
        );
    }

    #[test]
    fn end_tool_marks_a_failure_red() {
        let mut app = App::new();
        app.start_tool("Bash", "false");
        let finished = app.end_tool("boom", false).expect("a tool was running");
        assert_eq!(finished.status, ToolStatus::Failed);
    }

    #[test]
    fn end_tool_when_idle_returns_none_and_records_nothing() {
        let mut app = App::new();
        assert!(app.end_tool("ignored", true).is_none());
        assert!(app.history.is_empty());
    }

    #[test]
    fn set_tool_truncated_marks_the_running_tool_and_end_tool_keeps_it() {
        let mut app = App::new();
        app.begin_shell("tree ~/");
        app.set_tool_truncated();
        let finished = app
            .end_tool("/home/me\n├── a", true)
            .expect("a tool was running");
        assert!(finished.truncated, "the over-cap flag survives end_tool");
    }

    #[test]
    fn set_tool_truncated_is_a_noop_when_no_tool_is_running() {
        let mut app = App::new();
        app.set_tool_truncated(); // must not panic
        assert!(app.current_tool().is_none());
    }

    #[test]
    fn push_tool_output_tails_the_running_tool() {
        // Live streaming: each ToolOutput chunk appends to the running call's
        // output so the cell tails it (docs/tool-streaming.md).
        let mut app = App::new();
        app.start_tool("Bash", "ping -c 2 x");
        app.push_tool_output("line 1\n");
        app.push_tool_output("line 2\n");
        assert_eq!(app.current_tool().unwrap().output, "line 1\nline 2\n");
    }

    #[test]
    fn push_tool_output_is_a_noop_when_no_tool_is_running() {
        let mut app = App::new();
        app.push_tool_output("stray"); // must not panic
        assert!(app.current_tool().is_none());
    }

    #[test]
    fn push_tool_output_does_not_tail_a_waiting_batch_sibling() {
        // Only the front, *running* call tails output — a Waiting batch sibling
        // (not yet executing) is never targeted, even though it is the front in
        // the brief gap before start_tool flips it. The queue only ever runs its
        // front, so streamed output belongs to the running call.
        let mut app = App::new();
        app.start_tool_batch(&ping_batch());
        app.push_tool_output("early"); // front is Waiting, not Running yet
        assert!(
            app.current_tool().unwrap().output.is_empty(),
            "a Waiting sibling does not accumulate output"
        );
    }

    #[test]
    fn push_tool_output_does_not_charge_the_token_tally() {
        // The tally is charged once, from the authoritative ToolEnd output in
        // end_tool — never from the streamed chunks (which would double-count).
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Bash", "echo hi");
        app.push_tool_output("hi\n");
        assert_eq!(
            app.status().unwrap().tokens,
            0,
            "streaming does not charge tokens"
        );
        app.end_tool("Exit code: 0\nhi", true);
        assert!(
            app.status().unwrap().tokens > 0,
            "end_tool charges the final output once"
        );
    }

    #[test]
    fn flush_streaming_segment_records_text_and_reopens_an_empty_buffer() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("before the tool");
        let flushed = app.flush_streaming_segment().expect("buffered text");
        assert_eq!(flushed, "before the tool");
        // The stream stays open with a fresh empty buffer for the next segment.
        assert!(app.is_streaming());
        assert_eq!(app.streaming_text(), Some(""));
        assert_eq!(
            app.history.last(),
            Some(&HistoryItem::Message(Message {
                role: Role::Assistant,
                text: "before the tool".to_string(),
                timestamp: String::new(),
                images: Vec::new(),
            }))
        );
    }

    #[test]
    fn flush_streaming_segment_with_an_empty_buffer_records_nothing() {
        let mut app = App::new();
        app.begin_stream(); // empty buffer
        assert!(app.flush_streaming_segment().is_none());
        assert!(app.history.is_empty(), "nothing buffered, nothing recorded");
        assert!(app.is_streaming(), "the stream stays open");
    }

    #[test]
    fn flush_streaming_segment_when_idle_returns_none() {
        let mut app = App::new();
        assert!(app.flush_streaming_segment().is_none());
    }

    #[test]
    fn finish_stream_with_an_empty_final_segment_records_nothing() {
        // A turn that ends right after a tool call (no trailing text) must not
        // leave a phantom empty assistant message behind.
        let mut app = App::new();
        app.begin_stream();
        assert!(app.finish_stream().is_none());
        assert!(app.history.is_empty());
        assert!(!app.is_streaming());
    }

    // --- tool-output view (Ctrl+O) ---

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_o_toggles_into_and_out_of_the_tool_view() {
        let mut app = App::new();
        assert_eq!(app.view, View::Conversation);
        assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
        assert_eq!(app.view, View::ToolOutput);
        assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
        assert_eq!(app.view, View::Conversation);
    }

    // ===== Ctrl+D context-debug view (docs/context.md) =====

    #[test]
    fn ctrl_d_toggles_into_and_out_of_the_context_debug_view() {
        let mut app = App::new();
        assert_eq!(app.view, View::Conversation);
        assert_eq!(app.on_key(ctrl('d')), Action::ToggleContextDebug);
        assert_eq!(app.view, View::ContextDebug);
        assert_eq!(app.on_key(ctrl('d')), Action::ToggleContextDebug);
        assert_eq!(app.view, View::Conversation);
    }

    #[test]
    fn q_and_esc_close_the_context_debug_view() {
        for code in [KeyCode::Char('q'), KeyCode::Esc] {
            let mut app = App::new();
            app.on_key(ctrl('d'));
            assert_eq!(app.on_key(key(code)), Action::ToggleContextDebug);
            assert_eq!(app.view, View::Conversation, "{code:?} closes");
        }
    }

    #[test]
    fn ctrl_d_is_inert_in_the_other_overlays_and_ctrl_o_in_it() {
        // All three full-screen views share the alternate screen, so they
        // never stack: Ctrl+D does nothing under the tool view or the resume
        // picker, and Ctrl+O does nothing under the context-debug view.
        let mut app = App::new();
        app.on_key(ctrl('o'));
        assert_eq!(app.on_key(ctrl('d')), Action::None);
        assert_eq!(app.view, View::ToolOutput);

        let mut app = App::new();
        app.open_resume_picker(Vec::new(), String::new());
        assert_eq!(app.on_key(ctrl('d')), Action::None);
        assert_eq!(app.view, View::ResumePicker);

        let mut app = App::new();
        app.on_key(ctrl('d'));
        assert_eq!(app.on_key(ctrl('o')), Action::None);
        assert_eq!(app.view, View::ContextDebug);
    }

    #[test]
    fn ctrl_d_works_mid_turn_like_ctrl_o() {
        // The conversation keeps streaming underneath; the debug view only
        // reads state, so it opens even while a turn is active.
        let mut app = App::new();
        app.record_user_message("hi");
        app.begin_stream();
        assert_eq!(app.on_key(ctrl('d')), Action::ToggleContextDebug);
        assert_eq!(app.view, View::ContextDebug);
    }

    #[test]
    fn scroll_keys_move_the_context_debug_offset() {
        let mut app = App::new();
        app.on_key(ctrl('d'));
        assert!(app.debug_follow, "opens pinned to the bottom");
        app.on_key(key(KeyCode::Up));
        assert!(!app.debug_follow, "scrolling up unpins");
        app.debug_scroll = 5;
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.debug_scroll, 4);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.debug_scroll, 5);
        app.on_key(key(KeyCode::PageUp));
        assert_eq!(app.debug_scroll, 0);
        app.on_key(key(KeyCode::PageDown));
        assert_eq!(app.debug_scroll, TOOL_VIEW_PAGE);
        app.on_key(key(KeyCode::Home));
        assert_eq!(app.debug_scroll, 0);
        app.on_key(key(KeyCode::End));
        assert_eq!(app.debug_scroll, usize::MAX, "End settles at the draw");
    }

    #[test]
    fn settle_debug_scroll_caps_a_stale_offset_and_repins_follow() {
        let mut app = App::new();
        app.on_key(ctrl('d'));
        app.debug_follow = false;
        app.debug_scroll = 100;
        app.settle_debug_scroll(7);
        assert_eq!(app.debug_scroll, 7);
        assert!(app.debug_follow, "hitting the end re-engages follow");
        app.debug_follow = false;
        app.debug_scroll = 3;
        app.settle_debug_scroll(7);
        assert_eq!(app.debug_scroll, 3, "an in-range offset is kept");
    }

    #[test]
    fn set_system_prompt_stores_the_backends_prompt_for_the_debug_view() {
        let mut app = App::new();
        assert!(app.system_prompt.is_none());
        app.set_system_prompt(Some("be nice".to_string()));
        assert_eq!(app.system_prompt.as_deref(), Some("be nice"));
        app.set_system_prompt(None);
        assert!(app.system_prompt.is_none());
    }

    #[test]
    fn ctrl_o_opens_the_tool_view_even_while_streaming() {
        let mut app = App::new();
        app.begin_stream();
        app.on_key(ctrl('o'));
        assert_eq!(app.view, View::ToolOutput, "the overlay opens mid-stream");
        assert!(
            app.is_streaming(),
            "and the stream keeps running underneath"
        );
    }

    #[test]
    fn esc_in_the_tool_view_returns_to_the_conversation_not_quit() {
        let mut app = App::new();
        app.on_key(ctrl('o'));
        assert_eq!(app.view, View::ToolOutput);
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::ToggleToolView);
        assert_eq!(app.view, View::Conversation, "esc closes the overlay");
    }

    #[test]
    fn esc_in_the_conversation_still_quits() {
        let mut app = App::new();
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Quit);
    }

    #[test]
    fn ctrl_c_quits_from_the_tool_view_too() {
        let mut app = App::new();
        app.on_key(ctrl('o'));
        assert_eq!(app.on_key(ctrl('c')), Action::Quit);
    }

    #[test]
    fn scroll_keys_move_the_tool_view_offset() {
        let mut app = App::new();
        app.on_key(ctrl('o'));
        app.tool_scroll = 5;
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.tool_scroll, 4, "up scrolls toward the top");
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.tool_scroll, 5, "down scrolls toward the bottom");
        app.on_key(key(KeyCode::PageDown));
        assert_eq!(app.tool_scroll, 5 + TOOL_VIEW_PAGE);
        app.on_key(key(KeyCode::Up)); // saturating, never underflows below 0
        app.on_key(key(KeyCode::PageUp));
        assert_eq!(app.tool_scroll, 5 + TOOL_VIEW_PAGE - 1 - TOOL_VIEW_PAGE);
    }

    #[test]
    fn home_and_end_jump_the_tool_view_to_the_edges() {
        // codex's pager jump keys: Home to the very top (leaving tail-follow),
        // End back to the bottom (re-engaging it).
        let mut app = App::new();
        app.on_key(ctrl('o'));
        app.settle_tool_scroll(20); // pinned to the bottom
        app.on_key(key(KeyCode::Home));
        assert!(!app.tool_follow, "home stops tailing");
        app.settle_tool_scroll(20);
        assert_eq!(app.tool_scroll, 0, "home jumps to the top");

        app.on_key(key(KeyCode::End));
        app.settle_tool_scroll(20);
        assert!(app.tool_follow, "end re-engages tailing");
        assert_eq!(app.tool_scroll, 20, "end jumps to the bottom");
    }

    #[test]
    fn q_closes_the_tool_view_like_esc() {
        // codex's pager close key: q quits the overlay, back to the chat.
        let mut app = App::new();
        app.on_key(ctrl('o'));
        assert_eq!(app.on_key(key(KeyCode::Char('q'))), Action::ToggleToolView);
        assert_eq!(app.view, View::Conversation);
    }

    #[test]
    fn typing_is_ignored_in_the_tool_view() {
        let mut app = App::new();
        app.on_key(ctrl('o'));
        app.on_key(key(KeyCode::Char('x')));
        assert_eq!(app.input.text(), "", "the read-only viewer swallows typing");
    }

    #[test]
    fn opening_the_tool_view_follows_the_bottom() {
        let mut app = App::new();
        app.on_key(ctrl('o'));
        assert!(app.tool_follow, "the view opens pinned to the bottom");
        // Settling against any screen pins the offset to the last line.
        app.settle_tool_scroll(12);
        assert_eq!(app.tool_scroll, 12, "opens on the latest content");
    }

    #[test]
    fn re_opening_the_tool_view_follows_the_bottom_again() {
        let mut app = App::new();
        app.on_key(ctrl('o'));
        app.tool_scroll = 7;
        app.tool_follow = false; // pretend the user scrolled up
        app.on_key(ctrl('o')); // leave
        app.on_key(ctrl('o')); // re-enter
        assert!(app.tool_follow, "re-opening tails the bottom again");
        assert_eq!(app.tool_scroll, 0);
    }

    #[test]
    fn scrolling_up_leaves_the_bottom_then_reaching_it_re_engages() {
        let mut app = App::new();
        app.on_key(ctrl('o')); // follow
        app.settle_tool_scroll(20); // pinned to the bottom (20)
        assert_eq!(app.tool_scroll, 20);

        app.on_key(key(KeyCode::Up)); // read back
        assert!(!app.tool_follow, "scrolling up stops tailing");
        app.settle_tool_scroll(20);
        assert_eq!(app.tool_scroll, 19, "moved one off the bottom, stays put");

        app.on_key(key(KeyCode::Down)); // back down to the bottom
        app.settle_tool_scroll(20);
        assert!(app.tool_follow, "reaching the bottom re-engages tailing");
        assert_eq!(app.tool_scroll, 20);
    }

    #[test]
    fn settle_tool_scroll_caps_a_stale_offset() {
        let mut app = App::new();
        // Not following, but the stored offset is past the end → snaps to the
        // bottom (and resumes tailing, since it was at/past the last line).
        app.tool_scroll = 100;
        app.settle_tool_scroll(12);
        assert_eq!(app.tool_scroll, 12);
    }

    #[test]
    fn a_turn_interleaves_text_and_a_tool_call_in_order() {
        // text → tool → text, the way the dummy backend streams it. History must
        // hold the two assistant segments with the tool between them, in order.
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("let me check");
        app.flush_streaming_segment(); // text before the tool becomes its own message
        app.start_tool("Bash", "ls");
        app.end_tool("a\nb", true);
        app.push_chunk("all done");
        app.finish_stream();

        match app.history.as_slice() {
            [
                HistoryItem::Message(first),
                HistoryItem::Tool(tool),
                HistoryItem::Message(last),
            ] => {
                assert_eq!(first.text, "let me check");
                assert_eq!(first.role, Role::Assistant);
                assert_eq!(tool.name, "Bash");
                assert_eq!(tool.status, ToolStatus::Ok);
                assert_eq!(last.text, "all done");
            }
            other => panic!("unexpected interleaving: {other:?}"),
        }
    }

    // --- message queue (docs/queue.md) ---

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    #[test]
    fn enter_when_idle_submits_and_never_queues() {
        let mut app = App::new();
        app.input = TextArea::from_text("hello");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Submit("hello".to_string())
        );
        assert!(
            app.queued.is_empty(),
            "an idle submit doesn't touch the queue"
        );
    }

    /// A queued text batch with no attachments — most queue tests' shape.
    fn batch(texts: &[&str]) -> QueuedTurn {
        QueuedTurn::Messages {
            texts: texts.iter().map(|s| (*s).to_string()).collect(),
            images: Vec::new(),
        }
    }

    #[test]
    fn queueing_a_draft_mid_turn_carries_its_image_attachments() {
        // A mid-turn Enter must not silently drop a Ctrl+V attachment: the
        // (placeholder, path) pairs ride with the batch and dispatch when its
        // turn comes (docs/image-paste.md).
        let mut app = App::new();
        app.begin_stream();
        app.attach_image(PathBuf::from("/tmp/img1.png"));
        for c in " describe".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Enter));
        assert!(app.images.is_empty(), "the attachment left the composer");
        assert_eq!(
            app.drain_next_batch(),
            Some(QueuedTurn::Messages {
                texts: vec!["[Image #1] describe".to_string()],
                images: vec![("[Image #1]".to_string(), PathBuf::from("/tmp/img1.png"))],
            })
        );
    }

    #[test]
    fn a_tab_follow_up_batch_carries_its_images_too() {
        let mut app = App::new();
        app.begin_stream();
        app.attach_image(PathBuf::from("/tmp/pic.png"));
        app.on_key(key(KeyCode::Tab));
        assert_eq!(
            app.drain_next_batch(),
            Some(QueuedTurn::Messages {
                texts: vec!["[Image #1]".to_string()],
                images: vec![("[Image #1]".to_string(), PathBuf::from("/tmp/pic.png"))],
            })
        );
    }

    #[test]
    fn merging_enters_merges_their_images_in_order() {
        let mut app = App::new();
        app.begin_stream();
        app.attach_image(PathBuf::from("/a.png"));
        app.on_key(key(KeyCode::Enter)); // batch 1, first message
        app.attach_image(PathBuf::from("/b.png"));
        app.on_key(key(KeyCode::Enter)); // appends to batch 1
        match app.drain_next_batch() {
            Some(QueuedTurn::Messages { texts, images }) => {
                assert_eq!(texts.len(), 2, "one merged batch");
                let paths: Vec<_> = images.into_iter().map(|(_, p)| p).collect();
                assert_eq!(
                    paths,
                    vec![PathBuf::from("/a.png"), PathBuf::from("/b.png")]
                );
            }
            other => panic!("expected the merged batch, got {other:?}"),
        }
    }

    #[test]
    fn alt_up_restores_a_queued_batchs_images_to_the_composer() {
        // The pull-back re-attaches the batch's images so the placeholders in
        // the restored draft are backed again — an idle re-submit stages them.
        let mut app = App::new();
        app.begin_stream();
        app.attach_image(PathBuf::from("/tmp/pic.png"));
        app.on_key(key(KeyCode::Enter)); // queue it
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));
        assert_eq!(app.input.text(), "[Image #1]");
        assert_eq!(
            app.images,
            vec![("[Image #1]".to_string(), PathBuf::from("/tmp/pic.png"))]
        );
        app.finish_stream();
        app.end_turn(1); // the turn ends; the composer is idle again
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Submit("[Image #1]".to_string())
        );
        assert_eq!(
            app.take_submission_images(),
            vec![("[Image #1]".to_string(), PathBuf::from("/tmp/pic.png"))]
        );
    }

    #[test]
    fn enter_mid_turn_appends_to_one_batch_in_order() {
        // Consecutive Enters share a single turn-batch, oldest first — they
        // flush together as one next turn.
        let mut app = App::new();
        app.begin_stream();
        app.input = TextArea::from_text("first");
        app.on_key(key(KeyCode::Enter));
        app.input = TextArea::from_text("second");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.queued.len(), 1, "both Enters land in one batch");
        assert_eq!(
            app.queued[0],
            batch(&["first", "second"]),
            "FIFO, oldest first"
        );
    }

    #[test]
    fn drain_next_batch_takes_the_front_batch_in_order_and_empties() {
        // A batch (the Enters that share a turn) flushes whole as the next turn.
        let mut app = App::new();
        app.begin_stream();
        app.input = TextArea::from_text("a");
        app.on_key(key(KeyCode::Enter));
        app.input = TextArea::from_text("b");
        app.on_key(key(KeyCode::Enter));
        app.input = TextArea::from_text("c");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            app.drain_next_batch(),
            Some(batch(&["a", "b", "c"])),
            "the whole front batch, FIFO"
        );
        assert!(app.queued.is_empty(), "the queue is emptied");
        assert!(app.drain_next_batch().is_none(), "nothing left to flush");
    }

    #[test]
    fn tab_mid_turn_opens_a_new_follow_up_batch() {
        // Enter accumulates into the current batch; Tab starts a *new* batch so
        // its message runs as its own follow-up turn after the first queue.
        let mut app = App::new();
        app.begin_stream();
        app.input = TextArea::from_text("first");
        app.on_key(key(KeyCode::Enter)); // batch 1
        app.input = TextArea::from_text("follow");
        assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
        assert_eq!(app.input.text(), "", "Tab consumes the composer like Enter");
        assert_eq!(app.queued.len(), 2, "Tab opened a second batch");
        assert_eq!(app.queued[0], batch(&["first"]));
        assert_eq!(app.queued[1], batch(&["follow"]));
    }

    #[test]
    fn enter_after_tab_appends_to_the_follow_up_batch() {
        // Once Tab opens a new batch, a plain Enter joins *that* batch (the one
        // now being accumulated), not the first.
        let mut app = App::new();
        app.begin_stream();
        app.input = TextArea::from_text("a");
        app.on_key(key(KeyCode::Enter)); // batch 1 = [a]
        app.input = TextArea::from_text("b");
        app.on_key(key(KeyCode::Tab)); // batch 2 = [b]
        app.input = TextArea::from_text("c");
        app.on_key(key(KeyCode::Enter)); // batch 2 = [b, c]
        assert_eq!(app.queued.len(), 2);
        assert_eq!(app.queued[0], batch(&["a"]));
        assert_eq!(app.queued[1], batch(&["b", "c"]));
    }

    #[test]
    fn tab_follow_ups_drain_one_turn_at_a_time() {
        // Each batch is its own turn: drain yields the first queue, then the Tab
        // follow-up, in order — sequential turns, not one merged blob.
        let mut app = App::new();
        app.begin_stream();
        app.input = TextArea::from_text("first");
        app.on_key(key(KeyCode::Enter));
        app.input = TextArea::from_text("later");
        app.on_key(key(KeyCode::Tab));
        assert_eq!(
            app.drain_next_batch(),
            Some(batch(&["first"])),
            "the first queue goes first"
        );
        assert_eq!(
            app.drain_next_batch(),
            Some(batch(&["later"])),
            "the Tab follow-up next"
        );
        assert!(app.drain_next_batch().is_none());
    }

    #[test]
    fn tab_queued_message_is_recorded_for_up_recall() {
        // Like Enter, a Tab-queued message is recorded so ↑ brings it back.
        let mut app = App::new();
        app.begin_stream();
        app.input = TextArea::from_text("tabbed");
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input.text(), "");
        app.on_key(key(KeyCode::Up)); // empty composer → history recall
        assert_eq!(app.input.text(), "tabbed");
    }

    #[test]
    fn tab_when_idle_does_not_queue() {
        // Tab only queues while a turn streams; idle it's a no-op (you queue
        // follow-ups against a *running* turn).
        let mut app = App::new();
        app.input = TextArea::from_text("hello");
        assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
        assert!(app.queued.is_empty(), "idle Tab queues nothing");
        assert_eq!(app.input.text(), "hello", "and leaves the draft intact");
    }

    #[test]
    fn tab_on_an_empty_composer_mid_turn_is_a_no_op() {
        let mut app = App::new();
        app.begin_stream();
        assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
        assert!(
            app.queued.is_empty(),
            "nothing to queue from an empty composer"
        );
    }

    #[test]
    fn a_queued_message_is_recorded_for_up_recall() {
        // Queueing records into input_history (like a submit), so ↑ brings it back.
        let mut app = App::new();
        app.begin_stream();
        app.input = TextArea::from_text("queued line");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.input.text(), "");
        app.on_key(key(KeyCode::Up)); // empty composer → history recall
        assert_eq!(app.input.text(), "queued line");
    }

    #[test]
    fn alt_up_pulls_only_the_last_batch_into_the_composer_for_editing() {
        // Alt+Up edits the *last* turn-batch (codex's edit_queued_message
        // pop_back), not the whole backlog: with an Enter batch then a Tab
        // batch, Alt+Up yanks back only the Tab batch — the earlier Enter batch
        // stays queued, untouched.
        let mut app = App::new();
        app.begin_stream();
        app.input = TextArea::from_text("hello");
        app.on_key(key(KeyCode::Enter)); // batch 1 = [hello]
        app.input = TextArea::from_text("world");
        app.on_key(key(KeyCode::Enter)); // batch 1 = [hello, world]
        app.input = TextArea::from_text("deploy");
        app.on_key(key(KeyCode::Tab)); // batch 2 = [deploy]
        assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
        assert_eq!(
            app.input.text(),
            "deploy",
            "only the last batch returns — not hello/world"
        );
        assert_eq!(
            app.input.cursor(),
            "deploy".len(),
            "the cursor lands at the end, ready to edit"
        );
        assert_eq!(app.queued.len(), 1, "the earlier batch stays queued");
        assert_eq!(
            app.queued[0],
            batch(&["hello", "world"]),
            "and is left untouched"
        );
    }

    #[test]
    fn alt_up_concats_the_last_batchs_messages() {
        // The last batch can itself hold several messages (a Tab opened it, an
        // Enter extended it): Alt+Up returns them newline-joined, oldest first.
        let mut app = App::new();
        app.begin_stream();
        app.input = TextArea::from_text("deploy");
        app.on_key(key(KeyCode::Tab)); // batch 1 = [deploy]
        app.input = TextArea::from_text("rollback");
        app.on_key(key(KeyCode::Enter)); // batch 1 = [deploy, rollback]
        assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
        assert_eq!(
            app.input.text(),
            "deploy\nrollback",
            "the last batch's messages return newline-joined, oldest first"
        );
        assert_eq!(
            app.input.cursor(),
            "deploy\nrollback".len(),
            "the cursor lands at the end, ready to edit"
        );
        assert!(
            app.queued.is_empty(),
            "popping the only batch empties the queue"
        );
    }

    #[test]
    fn alt_up_does_not_clobber_a_draft() {
        // With text already in the composer, Alt+Up must not yank a queued message
        // over the draft (it falls through to cursor movement instead).
        let mut app = App::new();
        app.begin_stream();
        app.input = TextArea::from_text("queued");
        app.on_key(key(KeyCode::Enter));
        app.input = TextArea::from_text("a draft");
        assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
        assert_eq!(app.input.text(), "a draft", "the draft is untouched");
        assert_eq!(app.queued.len(), 1, "and the queue is untouched");
    }

    #[test]
    fn alt_up_on_an_empty_queue_is_harmless() {
        let mut app = App::new();
        app.begin_stream();
        assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
        assert_eq!(app.input.text(), "");
        assert!(app.queued.is_empty());
    }

    // --- slash-command palette ---

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn command_query_detects_a_bare_slash_token() {
        assert_eq!(command_query("/"), Some(""));
        assert_eq!(command_query("/he"), Some("he"));
        assert_eq!(command_query("/help"), Some("help"));
    }

    #[test]
    fn command_query_is_none_for_non_command_input() {
        assert_eq!(command_query(""), None);
        assert_eq!(command_query("hello"), None);
        assert_eq!(command_query("ask /help"), None); // not at the start
        assert_eq!(command_query("/help me"), None); // past a space → args, not a token
        assert_eq!(command_query("/a\nb"), None); // a newline ends the token too
    }

    #[test]
    fn the_command_registry_is_non_empty_with_unique_lowercase_names() {
        assert!(!COMMANDS.is_empty());
        let mut seen = std::collections::HashSet::new();
        for c in COMMANDS {
            assert!(!c.name.is_empty());
            assert!(
                !c.name.starts_with('/'),
                "names are stored without the slash"
            );
            assert_eq!(c.name, c.name.to_lowercase(), "names are lowercase");
            assert!(seen.insert(c.name), "duplicate command /{}", c.name);
        }
    }

    #[test]
    fn matching_commands_filters_by_name_prefix_case_insensitively() {
        assert_eq!(
            matching_commands("").len(),
            COMMANDS.len(),
            "an empty query lists everything"
        );
        let hits = matching_commands("HE");
        assert!(hits.iter().any(|c| c.name == "help"));
        assert!(
            hits.iter().all(|c| c.name.starts_with("he")),
            "every match shares the prefix"
        );
    }

    #[test]
    fn typing_a_slash_opens_the_command_palette_at_the_top() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('/')));
        let menu = app.command_menu.as_ref().expect("the palette is open");
        assert_eq!(menu.selected, 0);
    }

    #[test]
    fn typing_filters_and_clamps_the_selection() {
        // Seat the highlight on a NON-zero row first, then narrow the filter
        // past it — the refresh must pull the selection back in bounds, or
        // the highlight (and Enter) lands on nothing.
        let mut app = App::new();
        type_str(&mut app, "/c");
        assert_eq!(matching_commands("c").len(), 3, "/clear, /copy, /compact");
        app.on_key(key(KeyCode::Down)); // highlight /copy (index 1)
        assert_eq!(app.command_menu.as_ref().unwrap().selected, 1);
        type_str(&mut app, "l"); // "/cl" — only /clear matches now
        assert_eq!(matching_commands("cl").len(), 1, "only /clear matches");
        assert_eq!(app.command_menu.as_ref().unwrap().selected, 0, "clamped");
        assert_eq!(
            app.highlighted_command().map(|c| c.name),
            Some("clear"),
            "the highlight lands on a real row, so Enter still runs something"
        );
    }

    #[test]
    fn backspacing_the_slash_closes_the_palette() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('/')));
        assert!(app.command_menu.is_some());
        app.on_key(key(KeyCode::Backspace));
        assert!(app.command_menu.is_none(), "removing the slash closes it");
    }

    #[test]
    fn arrow_keys_move_the_palette_selection_within_bounds() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('/'))); // all commands listed
        let n = COMMANDS.len();
        for _ in 0..(n + 3) {
            app.on_key(key(KeyCode::Down));
        }
        assert_eq!(
            app.command_menu.as_ref().unwrap().selected,
            n - 1,
            "Down clamps at the last command"
        );
        for _ in 0..(n + 3) {
            app.on_key(key(KeyCode::Up));
        }
        assert_eq!(
            app.command_menu.as_ref().unwrap().selected,
            0,
            "Up clamps at the first"
        );
    }

    #[test]
    fn esc_closes_the_palette_instead_of_quitting() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('/')));
        assert_eq!(
            app.on_key(key(KeyCode::Esc)),
            Action::None,
            "esc dismisses the palette, it does not quit"
        );
        assert!(app.command_menu.is_none());
    }

    #[test]
    fn esc_is_sticky_typing_within_the_same_token_does_not_reopen() {
        let mut app = App::new();
        type_str(&mut app, "/he");
        app.on_key(key(KeyCode::Esc)); // dismiss
        assert!(app.command_menu.is_none());
        app.on_key(key(KeyCode::Char('l'))); // still within "/hel"
        assert!(
            app.command_menu.is_none(),
            "stays dismissed while editing the same token"
        );
    }

    #[test]
    fn leaving_and_re_entering_command_mode_reopens_the_palette() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('/')));
        app.on_key(key(KeyCode::Esc)); // dismissed
        app.on_key(key(KeyCode::Backspace)); // delete '/', input now empty
        assert!(app.command_menu.is_none());
        app.on_key(key(KeyCode::Char('/'))); // re-enter command mode
        assert!(
            app.command_menu.is_some(),
            "re-entering reopens the palette"
        );
    }

    #[test]
    fn enter_runs_the_clear_command_emptying_history() {
        let mut app = App::new();
        app.record_user_message("old message");
        type_str(&mut app, "/clear");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Clear);
        assert!(app.history.is_empty(), "clear emptied the conversation");
    }

    #[test]
    fn clear_mid_turn_wipes_the_streaming_state_and_records_nothing() {
        // `/clear` during a turn is a kill: the loop cancels + reaps the
        // backend (main.rs); the app side must leave no trace of the
        // half-done turn — no partial, no running tool, no live status, no
        // interrupt notice — or the fresh slate isn't fresh.
        let mut app = App::new();
        app.record_user_message("old message");
        app.begin_stream();
        app.push_chunk("half a rep");
        app.start_tool("read_file", "src/app.rs");
        type_str(&mut app, "/clear");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Clear);
        assert!(app.history.is_empty(), "no partial/notice/summary recorded");
        assert!(!app.is_streaming(), "the streaming buffer was dropped");
        assert!(!app.turn_active(), "the live status cleared");
        assert!(app.current_tool().is_none(), "the running tool was dropped");
    }

    #[test]
    fn clear_mid_turn_drops_the_queued_backlog() {
        // The backlog belonged to the conversation being wiped — flushing it
        // as the next turn would resurrect what `/clear` just removed.
        let mut app = App::new();
        app.begin_stream();
        app.input = TextArea::from_text("queued follow-up");
        app.on_key(key(KeyCode::Enter)); // a turn is in flight — this queues
        assert_eq!(app.queued.len(), 1, "the message queued");
        type_str(&mut app, "/clear");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Clear);
        assert!(app.drain_next_batch().is_none(), "the backlog was wiped");
    }

    #[test]
    fn enter_runs_help_posting_a_notice_that_lists_commands() {
        let mut app = App::new();
        type_str(&mut app, "/help");
        match app.on_key(key(KeyCode::Enter)) {
            Action::Notice(text) => {
                assert!(text.contains("Available commands"), "{text:?}");
                assert!(
                    text.contains("/clear"),
                    "the notice lists commands: {text:?}"
                );
            }
            other => panic!("expected a Notice, got {other:?}"),
        }
        assert!(app.input.is_empty());
        assert!(app.command_menu.is_none());
    }

    #[test]
    fn help_mid_turn_is_rejected_with_a_toast_not_the_command_list() {
        // Mid-turn the multi-line list would interleave with the streaming
        // reply, so /help is rejected with a transient toast instead.
        let mut app = App::new();
        app.begin_stream();
        type_str(&mut app, "/help");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Toast(HELP_BUSY_NOTICE.to_string()),
        );
        assert!(app.input.is_empty());
    }

    // --- /init (docs/init.md) ---

    #[test]
    fn the_palette_lists_init_with_codexs_description() {
        let init = COMMANDS
            .iter()
            .position(|c| c.name == "init")
            .expect("/init is registered");
        let cmd = &COMMANDS[init];
        assert_eq!(
            cmd.description,
            "create an AGENTS.md file with instructions for alter-zero"
        );
        assert_eq!(cmd.effect, CommandEffect::Init);
        // Codex's palette adjacency: Init lists immediately before Compact
        // (its enum order) — the documented order, pinned like MENU_MAX_ROWS.
        let compact = COMMANDS
            .iter()
            .position(|c| c.name == "compact")
            .expect("/compact is registered");
        assert_eq!(init + 1, compact, "/init lists right before /compact");
    }

    #[test]
    fn the_init_prompt_is_codexs_agents_md_generator() {
        // The canned prompt *is* the feature — everything after the submit is
        // the normal turn machinery. Codex's prompt_for_init_command.md asks
        // the model to generate AGENTS.md and to leave an existing one alone.
        assert!(INIT_PROMPT.contains("AGENTS.md"));
        assert!(
            INIT_PROMPT.contains("do not overwrite"),
            "the prompt guards an existing AGENTS.md"
        );
    }

    #[test]
    fn slash_init_submits_the_canned_prompt_when_idle() {
        // Codex's /init is submit_user_message(INIT_PROMPT): the whole canned
        // prompt goes out as a regular user turn — echoed as the ❯ message,
        // recorded, checkpointed — and the model's tool loop does the work.
        // Trailing whitespace is trimmed: the file's final newline would wrap
        // into an empty last line that message_lines pads into a stray
        // full-width dark row under the ❯ cell (codex trims the same way at
        // render time, its display_lines' trim_end_matches).
        let mut app = App::new();
        type_str(&mut app, "/init");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Submit(INIT_PROMPT.trim_end().to_string())
        );
        assert!(app.input.is_empty());
        assert!(app.command_menu.is_none());
    }

    #[test]
    fn esc_undo_of_an_init_turn_restores_the_command_not_the_prompt() {
        // Esc in the pre-stream window undoes the submission into the
        // composer (docs/interrupt.md). For an /init turn the user typed
        // "/init", not the 1.8KB canned prompt — flooding the composer with
        // text the user never held (which a follow-up Ctrl+C would then
        // record into ↑ recall) breaks the no-recall guarantee, so the undo
        // restores the command itself, palette reopened: the exact
        // pre-submit state.
        let mut app = App::new();
        type_str(&mut app, "/init");
        let Action::Submit(text) = app.on_key(key(KeyCode::Enter)) else {
            panic!("idle /init submits");
        };
        app.record_user_message(&text); // mirror start_turn's recording
        app.begin_stream();
        assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
        assert_eq!(app.input.text(), "/init", "the command, not the prompt");
        assert!(
            app.command_menu.is_some(),
            "the palette reopens on the recalled command"
        );
        assert!(app.history.is_empty(), "the submission rolled back");
    }

    #[test]
    fn init_mid_turn_tab_is_rejected_like_enter() {
        // Tab is the palette's other accept key, and mid-turn it is also the
        // queue-as-new-batch key — the menu_open arm must keep winning, or
        // Tab on "/init" would silently queue the literal text as a message
        // for the model. Both accept keys reject with the busy toast.
        let mut app = App::new();
        app.record_user_message("hello");
        app.begin_stream();
        type_str(&mut app, "/init");
        assert_eq!(
            app.on_key(key(KeyCode::Tab)),
            Action::Toast(INIT_BUSY_NOTICE.to_string()),
        );
        assert!(
            app.queued.is_empty(),
            "the literal \"/init\" was not queued as a message"
        );
    }

    #[test]
    fn slash_init_does_not_enter_the_up_arrow_recall_history() {
        // Palette commands never record into the ↑ recall history — the
        // composer held "/init", not the canned prompt, and recalling a
        // 40-line prompt the user never typed would be noise.
        let mut app = App::new();
        type_str(&mut app, "/init");
        app.on_key(key(KeyCode::Enter));
        app.on_key(key(KeyCode::Up));
        assert!(app.input.is_empty(), "nothing to recall");
    }

    #[test]
    fn init_mid_turn_is_rejected_with_a_toast() {
        // Codex's available_during_task is false for /init — submitting would
        // race the running stream with a second turn. Ours rejects with the
        // /compact toast pattern; the running turn is untouched.
        let mut app = App::new();
        app.record_user_message("hello");
        app.begin_stream();
        type_str(&mut app, "/init");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Toast(INIT_BUSY_NOTICE.to_string()),
        );
        assert!(app.turn_active(), "the running turn is untouched");
    }

    // --- /compact (docs/compact.md) ---

    #[test]
    fn the_palette_lists_compact_with_codexs_description() {
        let cmd = COMMANDS
            .iter()
            .find(|c| c.name == "compact")
            .expect("/compact is registered");
        assert_eq!(
            cmd.description,
            "summarize conversation to prevent hitting the context limit"
        );
        assert_eq!(cmd.effect, CommandEffect::Compact);
    }

    #[test]
    fn slash_compact_dispatches_the_compact_action_when_idle() {
        let mut app = App::new();
        app.record_user_message("hello");
        type_str(&mut app, "/compact");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Compact);
        assert!(app.input.is_empty());
    }

    #[test]
    fn compact_mid_turn_is_rejected_with_a_toast() {
        let mut app = App::new();
        app.record_user_message("hello");
        app.begin_stream();
        type_str(&mut app, "/compact");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Toast(COMPACT_BUSY_NOTICE.to_string()),
        );
        assert!(app.turn_active(), "the running turn is untouched");
    }

    #[test]
    fn compact_with_nothing_to_compact_is_rejected_with_a_toast() {
        let mut app = App::new();
        type_str(&mut app, "/compact");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Toast(COMPACT_EMPTY_NOTICE.to_string()),
        );
    }

    #[test]
    fn begin_compact_starts_a_fixed_verb_turn_without_advancing_the_cycle() {
        let mut app = App::new();
        app.begin_compact(false);
        assert!(app.turn_active());
        assert!(
            app.is_streaming(),
            "the strip shows while the summary streams"
        );
        assert!(app.is_compacting());
        assert_eq!(app.status().expect("a compact status").verb, COMPACT_VERB);
        app.finish_compact();
        // The cycled per-turn verbs are unaffected: the next real turn still
        // picks the first (the verb-sequence contract).
        app.begin_stream();
        assert_eq!(app.status().unwrap().verb, WORKING_VERBS[0]);
    }

    #[test]
    fn compact_chunks_divert_to_the_buffer_and_never_render() {
        let mut app = App::new();
        app.begin_compact(false);
        app.push_chunk("the summary ");
        app.push_chunk("text");
        assert_eq!(
            app.streaming_text(),
            Some(""),
            "the visible reply buffer stays empty — the summary is never rendered"
        );
        assert!(app.status().unwrap().tokens > 0, "the tally still ticks");
        let compaction = app.finish_compact().expect("a marker");
        assert_eq!(compaction.summary, "the summary text");
    }

    #[test]
    fn finish_compact_appends_the_marker_and_ends_the_turn_without_a_summary() {
        let mut app = App::new();
        app.record_user_message("hello");
        let before = app.history.len();
        app.begin_compact(false);
        app.push_chunk("gist\n");
        let compaction = app.finish_compact().expect("a marker");
        assert_eq!(compaction.summary, "gist", "the streamed text, trimmed");
        assert_eq!(app.history.len(), before + 1);
        assert!(
            matches!(app.history.last(), Some(HistoryItem::Compaction(c)) if c.summary == "gist")
        );
        assert!(!app.turn_active(), "the status cleared");
        assert!(!app.is_streaming());
        assert!(!app.is_compacting());
        assert!(
            app.end_turn(3).is_none(),
            "no Done-for-Ns summary for a compact turn — the marker cell is the record"
        );
    }

    #[test]
    fn a_compact_turn_with_no_streamed_text_still_appends_an_empty_marker() {
        // The derivation substitutes codex's "(no summary available)" for the
        // empty summary — the marker still lands so the state is visible.
        let mut app = App::new();
        app.record_user_message("hello");
        app.begin_compact(false);
        let compaction = app.finish_compact().expect("a marker");
        assert_eq!(compaction.summary, "");
    }

    #[test]
    fn esc_mid_compact_keeps_the_old_history_and_records_the_interrupt_notice() {
        // The swap lives only at StreamDone (codex: replace-at-the-very-end) —
        // an interrupt drops the half summary and leaves the context as it was.
        // The tail being a user message must NOT trigger the interrupt-undo
        // (that would pull an unrelated old message into the composer).
        let mut app = App::new();
        app.record_user_message("hello");
        let before = app.history.clone();
        app.begin_compact(false);
        app.push_chunk("half a summ");
        let outcome = app.interrupt_turn().expect("an interrupt outcome");
        assert!(
            matches!(
                &outcome,
                InterruptedTurn::Kept {
                    partial: None,
                    tool: None,
                    notice: Some(_)
                }
            ),
            "{outcome:?}"
        );
        assert!(!app.is_compacting(), "the half summary is dropped");
        assert_eq!(app.history[..before.len()], before[..]);
        assert!(
            !app.history
                .iter()
                .any(|i| matches!(i, HistoryItem::Compaction(_))),
            "no marker on interrupt"
        );
        assert!(app.input.is_empty(), "no interrupt-undo composer refill");
    }

    #[test]
    fn a_backend_error_mid_compact_drops_the_buffer_and_records_the_notice() {
        let mut app = App::new();
        app.record_user_message("hello");
        app.begin_compact(false);
        app.push_chunk("half");
        let failure = app.fail_stream("boom").expect("a failure record");
        assert_eq!(
            failure.partial, None,
            "the half summary never becomes an assistant message"
        );
        assert!(!app.is_compacting());
        assert!(
            !app.history
                .iter()
                .any(|i| matches!(i, HistoryItem::Compaction(_)))
        );
        assert!(
            matches!(app.history.last(), Some(HistoryItem::Message(m)) if m.role == Role::Error)
        );
    }

    #[test]
    fn clear_conversation_drops_the_compact_state() {
        let mut app = App::new();
        app.record_user_message("hi");
        app.begin_compact(false);
        app.push_chunk("half");
        app.clear_conversation();
        assert!(!app.is_compacting());
        assert!(!app.turn_active());
        assert!(app.history.is_empty());
    }

    // --- auto-compact + the context gauge (docs/compact.md) ---

    fn usage_of(total_input: u64, output: u64) -> crate::stream::TokenUsage {
        crate::stream::TokenUsage {
            input: total_input,
            output,
            cached: 0,
            cache_write: 0,
        }
    }

    #[test]
    fn apply_usage_tracks_the_context_size_from_the_usage_frame() {
        // The round's `input` is the whole re-sent context; `output` joins the
        // next round's context — their sum is the live gauge value.
        let mut app = App::new();
        app.begin_stream();
        app.apply_usage(&usage_of(5_000, 200));
        assert_eq!(app.context_used(), 5_200);
    }

    #[test]
    fn a_turn_with_no_usage_frame_estimates_the_context_at_turn_end() {
        // The dummy sends no usage — the tokenizer estimate stands in so the
        // gauge (and the auto trigger) still work offline.
        let mut app = App::new();
        app.record_user_message("hello there");
        app.begin_stream();
        app.push_chunk("a reply");
        app.finish_stream();
        app.end_turn(1);
        assert!(app.context_used() > 0);
    }

    #[test]
    fn set_user_instructions_stores_the_agents_md_fragment() {
        // The boundary injects the rendered AGENTS.md instructions (the
        // set_system_prompt pattern); everyone deriving context reads them.
        // See docs/project-doc.md.
        let mut app = App::new();
        assert_eq!(app.user_instructions, None);
        app.set_user_instructions(Some("<INSTRUCTIONS>\nguide\n</INSTRUCTIONS>".to_string()));
        assert_eq!(
            app.user_instructions.as_deref(),
            Some("<INSTRUCTIONS>\nguide\n</INSTRUCTIONS>")
        );
        app.set_user_instructions(None);
        assert_eq!(app.user_instructions, None);
    }

    #[test]
    fn the_context_estimate_counts_the_user_instructions() {
        // The instructions ride every request, so the footer gauge's offline
        // estimate must count them like the system prompt.
        let run_turn = |instructions: Option<&str>| {
            let mut app = App::new();
            app.set_user_instructions(instructions.map(str::to_string));
            app.record_user_message("hello there");
            app.begin_stream();
            app.push_chunk("a reply");
            app.finish_stream();
            app.end_turn(1);
            app.context_used()
        };
        let without = run_turn(None);
        let with = run_turn(Some("a long AGENTS.md contributor guide to count"));
        assert!(with > without, "{with} vs {without}");
    }

    #[test]
    fn clear_re_seats_the_gauge_on_what_still_rides_the_next_request() {
        // /clear wipes the conversation, but the system prompt and the
        // standing AGENTS.md instructions still ride the very next request —
        // hard-zeroing the gauge would under-report them. It re-seats on the
        // estimate instead; a bare session still reads 0.
        let mut app = App::new();
        app.set_system_prompt(Some("a system prompt".to_string()));
        app.set_user_instructions(Some("standing project instructions".to_string()));
        app.record_user_message("hello there");
        app.begin_stream();
        app.finish_stream();
        app.end_turn(1);
        app.clear_conversation();
        assert!(
            app.context_used() > 0,
            "the prompt + instructions still count after a clear"
        );

        let mut bare = App::new();
        bare.record_user_message("hello there");
        bare.begin_stream();
        bare.finish_stream();
        bare.end_turn(1);
        bare.clear_conversation();
        assert_eq!(bare.context_used(), 0, "nothing standing → a true zero");
    }

    #[test]
    fn an_instructions_only_context_has_nothing_to_compact() {
        // The instructions are derived context, not conversation: with no
        // history they must not make /compact or auto-compact think there is
        // something to summarize (both emptiness checks deliberately derive
        // WITHOUT them — this pins that).
        let mut app = App::new();
        app.set_user_instructions(Some("standing project instructions".to_string()));
        type_str(&mut app, "/compact");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Toast(COMPACT_EMPTY_NOTICE.to_string()),
            "an instructions-only context is still nothing to compact"
        );
        // And the auto trigger stays put even far past the threshold.
        app.set_context_window(Some(10));
        app.begin_stream();
        app.apply_usage(&usage_of(1_000, 0));
        app.finish_stream();
        app.end_turn(1);
        assert!(
            !app.should_auto_compact(),
            "auto-compact never fires on an instructions-only context"
        );
    }

    #[test]
    fn set_context_window_ignores_a_zero() {
        let mut app = App::new();
        app.set_context_window(Some(0));
        assert_eq!(app.context_window(), None);
        app.set_context_window(Some(100));
        assert_eq!(app.context_window(), Some(100));
    }

    #[test]
    fn auto_compact_triggers_past_ninety_percent_of_the_window() {
        // Codex's threshold: (window * 9) / 10.
        let mut app = App::new();
        app.record_user_message("hello");
        app.set_context_window(Some(1_000));
        app.begin_stream();
        app.apply_usage(&usage_of(950, 0));
        app.finish_stream();
        app.end_turn(1);
        assert!(app.should_auto_compact());
    }

    #[test]
    fn auto_compact_does_not_trigger_at_or_below_the_threshold() {
        let mut app = App::new();
        app.record_user_message("hello");
        app.set_context_window(Some(1_000));
        app.begin_stream();
        app.apply_usage(&usage_of(900, 0));
        app.finish_stream();
        app.end_turn(1);
        assert!(
            !app.should_auto_compact(),
            "900 of 1000 is exactly the line"
        );
    }

    #[test]
    fn auto_compact_needs_a_window_and_derivable_content() {
        let mut app = App::new();
        // Over any threshold but no window known → never.
        app.begin_stream();
        app.apply_usage(&usage_of(1_000_000, 0));
        app.finish_stream();
        app.end_turn(1);
        assert!(!app.should_auto_compact(), "no window, no trigger");
        // A window but an empty conversation → nothing to summarize.
        app.set_context_window(Some(100));
        assert!(!app.should_auto_compact(), "empty context, no trigger");
    }

    #[test]
    fn auto_compact_never_fires_mid_turn() {
        let mut app = App::new();
        app.record_user_message("hello");
        app.set_context_window(Some(100));
        app.begin_stream();
        app.apply_usage(&usage_of(990, 0));
        assert!(!app.should_auto_compact(), "a turn is in flight");
    }

    #[test]
    fn auto_compact_is_blocked_after_a_compaction_until_the_next_turn() {
        // One attempt per user turn (codex's per-turn semantics): a compaction
        // that leaves the gauge high must not immediately re-trigger — the
        // next completed turn re-arms it.
        let mut app = App::new();
        app.record_user_message("hello there my friend");
        app.set_context_window(Some(10)); // tiny: the bridge alone exceeds it
        app.begin_compact(false);
        app.push_chunk("a summary");
        app.finish_compact();
        assert!(
            app.context_used() > 9,
            "precondition: still over the threshold after compacting"
        );
        assert!(
            !app.should_auto_compact(),
            "blocked right after a compaction"
        );
        app.begin_stream();
        app.finish_stream();
        app.end_turn(1);
        assert!(
            app.should_auto_compact(),
            "the next turn re-arms the trigger"
        );
    }

    #[test]
    fn an_interrupted_compaction_blocks_the_auto_retrigger() {
        // Esc'ing a compaction must not spawn another one at the same turn end
        // — the user just said no.
        let mut app = App::new();
        app.record_user_message("hello");
        app.set_context_window(Some(10));
        app.begin_stream();
        app.apply_usage(&usage_of(100, 0));
        app.finish_stream();
        app.end_turn(1);
        assert!(app.should_auto_compact(), "precondition: over threshold");
        app.begin_compact(true);
        app.push_chunk("half");
        app.interrupt_turn();
        assert!(!app.should_auto_compact(), "an Esc'd compaction stays down");
    }

    #[test]
    fn a_failed_compaction_blocks_the_auto_retrigger() {
        let mut app = App::new();
        app.record_user_message("hello");
        app.set_context_window(Some(10));
        app.begin_compact(true);
        app.push_chunk("half");
        app.fail_stream("boom");
        assert!(!app.should_auto_compact(), "a failed compaction stays down");
    }

    #[test]
    fn finish_compact_records_before_after_and_the_auto_tag() {
        let mut app = App::new();
        app.record_user_message("hello there friend");
        app.set_context_window(Some(1_000));
        app.begin_stream();
        app.apply_usage(&usage_of(500, 0));
        app.finish_stream();
        app.end_turn(1);
        app.begin_compact(true);
        app.push_chunk("the gist");
        let compaction = app.finish_compact().expect("a marker");
        assert_eq!(
            compaction.before, 500,
            "the gauge value when compacting began"
        );
        assert!(
            compaction.after > 0,
            "re-estimated from the compacted derivation"
        );
        assert!(compaction.auto);
        assert_eq!(
            app.context_used(),
            compaction.after,
            "the gauge drops to the fresh estimate"
        );
    }

    #[test]
    fn a_manual_compaction_is_not_tagged_auto() {
        let mut app = App::new();
        app.record_user_message("hi");
        app.begin_compact(false);
        app.push_chunk("s");
        assert!(!app.finish_compact().expect("a marker").auto);
    }

    #[test]
    fn clear_conversation_resets_the_context_gauge() {
        let mut app = App::new();
        app.record_user_message("hello");
        app.begin_stream();
        app.apply_usage(&usage_of(5_000, 0));
        app.finish_stream();
        app.end_turn(1);
        app.clear_conversation();
        assert_eq!(app.context_used(), 0);
    }

    #[test]
    fn show_toast_holds_the_text_and_kind_without_recording_history() {
        let mut app = App::new();
        app.show_toast("Copied last message to clipboard", ToastKind::Info);
        let toast = app.toast().expect("a toast is live");
        assert_eq!(toast.text, "Copied last message to clipboard");
        assert_eq!(toast.kind, ToastKind::Info);
        assert!(
            app.history.is_empty(),
            "a toast is UI, never a conversation entry"
        );
    }

    #[test]
    fn show_toast_overwrites_the_previous_one() {
        let mut app = App::new();
        app.show_toast("first", ToastKind::Info);
        app.show_toast("second", ToastKind::Error);
        let toast = app.toast().expect("a toast is live");
        assert_eq!(toast.text, "second");
        assert_eq!(toast.kind, ToastKind::Error);
    }

    #[test]
    fn clear_toast_removes_it() {
        let mut app = App::new();
        app.show_toast("hi", ToastKind::Info);
        app.clear_toast();
        assert!(app.toast().is_none());
    }

    #[test]
    fn clear_conversation_drops_a_live_toast() {
        let mut app = App::new();
        app.show_toast("Copied", ToastKind::Info);
        app.clear_conversation();
        assert!(app.toast().is_none(), "a cleared slate shows nothing");
    }

    #[test]
    fn enter_runs_the_quit_command() {
        // `/quit` exits the app, like codex's `/quit` ("exit Codex").
        let mut app = App::new();
        type_str(&mut app, "/quit");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Quit);
        assert!(app.input.is_empty(), "the command was consumed");
        assert!(app.command_menu.is_none());
    }

    #[test]
    fn tab_runs_the_highlighted_command_like_enter() {
        let mut app = App::new();
        app.record_user_message("old message");
        type_str(&mut app, "/clear");
        assert_eq!(app.on_key(key(KeyCode::Tab)), Action::Clear);
        assert!(app.history.is_empty(), "Tab ran the command, like Enter");
    }

    #[test]
    fn last_assistant_text_returns_the_last_assistant_message() {
        let mut app = App::new();
        app.record_user_message("q1");
        app.begin_stream();
        app.push_chunk("first answer");
        app.finish_stream();
        app.record_user_message("q2");
        app.begin_stream();
        app.push_chunk("second answer");
        app.finish_stream();
        assert_eq!(app.last_assistant_text().as_deref(), Some("second answer"));
    }

    #[test]
    fn last_assistant_text_skips_trailing_non_assistant_items() {
        // A later user message, tool call, or system notice must not shadow the
        // last *assistant* message — `/copy` copies the model's last response.
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("the answer");
        app.finish_stream();
        app.start_tool("Read", "f");
        app.end_tool("out", true);
        app.record_user_message("a follow-up");
        app.record_system_message("a notice");
        assert_eq!(app.last_assistant_text().as_deref(), Some("the answer"));
    }

    #[test]
    fn last_assistant_text_is_none_without_an_assistant_message() {
        let mut app = App::new();
        app.record_user_message("only a user message");
        assert!(app.last_assistant_text().is_none());
    }

    #[test]
    fn enter_runs_copy_returning_the_last_assistant_text() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("copy this answer");
        app.finish_stream();
        type_str(&mut app, "/copy");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Copy(Some("copy this answer".to_string()))
        );
        assert!(app.input.is_empty(), "the command was consumed");
        assert!(app.command_menu.is_none());
    }

    #[test]
    fn copy_with_no_assistant_message_returns_copy_none() {
        // Nothing to copy → Action::Copy(None); the loop turns that into the red
        // "No agent response to copy" notice (codex's empty case).
        let mut app = App::new();
        app.record_user_message("just me");
        type_str(&mut app, "/copy");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Copy(None));
    }

    #[test]
    fn copy_dispatches_mid_turn_instead_of_queuing() {
        // `/copy` is available during a task (codex): the palette's Enter wins
        // over the mid-turn queue, copying the last *completed* answer (not the
        // streaming buffer).
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("earlier answer");
        app.finish_stream();
        app.begin_stream();
        app.push_chunk("streaming, not yet recorded");
        type_str(&mut app, "/copy");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Copy(Some("earlier answer".to_string())),
            "the palette runs /copy mid-turn rather than queuing it"
        );
        assert!(app.queued.is_empty(), "nothing was queued");
    }

    #[test]
    fn enter_with_no_matching_command_does_not_submit() {
        let mut app = App::new();
        type_str(&mut app, "/zzz");
        assert!(app.command_menu.is_some(), "palette is open but empty");
        assert!(matching_commands("zzz").is_empty());
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::None,
            "a no-match query is not submitted as a message"
        );
        assert_eq!(app.input.text(), "/zzz", "the input is left intact");
    }

    #[test]
    fn enter_still_submits_a_normal_message_when_no_palette_is_open() {
        let mut app = App::new();
        app.input = TextArea::from_text("hello");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Submit("hello".to_string())
        );
    }

    #[test]
    fn record_system_message_appends_a_system_role_message() {
        let mut app = App::new();
        app.record_system_message("a notice");
        assert_eq!(
            app.history.last(),
            Some(&HistoryItem::Message(Message {
                role: Role::System,
                text: "a notice".to_string(),
                timestamp: String::new(),
                images: Vec::new(),
            }))
        );
    }

    // --- timestamps (shown only in the Ctrl+O transcript; see docs/timestamps.md) ---

    /// A fixed stub clock so timestamp behaviour is deterministic in tests.
    const STAMP: &str = "2026-06-09 02:32:05 PM";

    #[test]
    fn set_clock_stamps_every_recorded_message_and_tool() {
        let mut app = App::new();
        app.set_clock(|| STAMP.to_string());

        app.record_user_message("hi");
        app.begin_stream();
        app.push_chunk("answer");
        app.finish_stream();
        app.start_tool("Read", "f");
        app.end_tool("out", true);

        assert_eq!(app.history.len(), 3, "user, assistant, tool");
        for item in &app.history {
            let ts = match item {
                HistoryItem::Message(m) => &m.timestamp,
                HistoryItem::Tool(t) => &t.timestamp,
                HistoryItem::Summary(s) => &s.timestamp,
                HistoryItem::Background(n) => &n.timestamp,
                HistoryItem::Compaction(c) => &c.timestamp,
            };
            assert_eq!(ts, STAMP, "every recorded item carries the clock's stamp");
        }
    }

    #[test]
    fn without_a_clock_recorded_timestamps_are_empty() {
        // The pure default used by unit tests: no clock injected → empty stamp,
        // so existing equality assertions on Message/ToolCall still hold.
        let mut app = App::new();
        app.record_user_message("hi");
        assert_eq!(message_at(&app, 0).timestamp, "");
    }

    // --- session info (the footer under the box; see docs/footer.md) ---

    #[test]
    fn session_info_is_unset_by_default() {
        // The unit-test default: no footer until the I/O boundary injects the
        // display strings (the set_clock pattern).
        assert!(App::new().session.is_none());
    }

    #[test]
    fn set_session_info_stores_the_display_strings() {
        let mut app = App::new();
        app.set_session_info("dummy_model_name", "~/alter-zero");
        let session = app.session.as_ref().expect("session info stored");
        assert_eq!(session.model, "dummy_model_name");
        assert_eq!(session.cwd, "~/alter-zero");
    }

    // --- live status indicator (see docs/status-indicator.md) ---

    #[test]
    fn idle_has_no_status_and_begin_stream_starts_one() {
        let mut app = App::new();
        assert!(!app.turn_active(), "no turn in flight when idle");
        assert!(app.status().is_none());
        app.begin_stream();
        assert!(app.turn_active(), "a turn is in flight while streaming");
        let status = app.status().expect("a status once streaming");
        assert!(
            WORKING_VERBS.contains(&status.verb),
            "a working verb is chosen: {:?}",
            status.verb
        );
        assert_eq!(status.tokens, 0, "no tokens counted yet (the '0s' state)");
        assert_eq!(status.arrow, TokenArrow::Down);
        assert_eq!(status.thinking, None, "not thinking yet");
    }

    #[test]
    fn the_per_turn_verb_changes_from_one_turn_to_the_next() {
        // Deterministic but varied: consecutive turns pick the next verb in the
        // registry, so the demo isn't monotonous.
        let mut app = App::new();
        app.begin_stream();
        let first = app.status().unwrap().verb;
        app.finish_stream();
        app.end_turn(1);
        app.begin_stream();
        let second = app.status().unwrap().verb;
        assert_ne!(first, second, "the next turn picks a different verb");
        assert_eq!(first, WORKING_VERBS[0]);
        assert_eq!(second, WORKING_VERBS[1]);
    }

    #[test]
    fn push_chunk_grows_the_token_tally_pointing_down() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("some words here");
        let before = app.status().unwrap().tokens;
        assert!(before > 0, "streamed text counts tokens");
        assert_eq!(app.status().unwrap().arrow, TokenArrow::Down);
        app.push_chunk(" and more");
        assert!(
            app.status().unwrap().tokens > before,
            "the tally only grows"
        );
    }

    #[test]
    fn count_user_input_adds_tokens_pointing_the_arrow_up() {
        // The user's just-sent message is counted into the tally as uploaded
        // input (arrow ↑), so the status shows `↑ N tokens` while the model
        // spins up before its first chunk.
        let mut app = App::new();
        app.begin_stream();
        app.count_user_input("a user message worth several tokens");
        let status = app.status().unwrap();
        assert!(status.tokens > 0, "the user message is counted");
        assert_eq!(status.arrow, TokenArrow::Up, "uploaded input → ↑");
    }

    #[test]
    fn count_user_input_is_a_noop_when_no_turn_is_active() {
        let mut app = App::new();
        app.count_user_input("nothing is streaming yet");
        assert!(app.status.is_none(), "no status to count into");
    }

    #[test]
    fn the_first_chunk_flips_the_arrow_back_down_after_the_user_input() {
        let mut app = App::new();
        app.begin_stream();
        app.count_user_input("hello");
        let input_tokens = app.status().unwrap().tokens;
        assert_eq!(app.status().unwrap().arrow, TokenArrow::Up);
        app.push_chunk("hi there");
        let status = app.status().unwrap();
        assert_eq!(status.arrow, TokenArrow::Down, "streaming output → ↓");
        assert!(
            status.tokens > input_tokens,
            "the reply's tokens add on top of the counted input"
        );
    }

    #[test]
    fn a_tool_adds_to_the_tally_and_flips_the_arrow_up_without_resetting() {
        // "dont reset the existing token count from response count just add and
        // use up arrow" — the tool's output is *added* to the running total.
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("the streamed reply so far");
        let after_text = app.status().unwrap().tokens;
        app.start_tool("Read", "f");
        app.end_tool("a multi line\ntool output blob", true);
        let status = app.status().unwrap();
        assert!(
            status.tokens > after_text,
            "the tool's output is added on top: {} !> {after_text}",
            status.tokens
        );
        assert_eq!(status.arrow, TokenArrow::Up, "arrow flips up after a tool");
    }

    #[test]
    fn streaming_again_after_a_tool_points_the_arrow_back_down() {
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Read", "f");
        app.end_tool("out", true);
        assert_eq!(app.status().unwrap().arrow, TokenArrow::Up);
        app.push_chunk("more reply text");
        assert_eq!(
            app.status().unwrap().arrow,
            TokenArrow::Down,
            "resuming the reply points the arrow back down"
        );
    }

    #[test]
    fn thinking_chunks_grow_the_tally_pointing_down_without_touching_the_reply() {
        // Reasoning deltas count into the live tally like reply text (they are
        // streamed output, so ↓ — even right after a tool's ↑), but the text
        // itself is opaque: it never reaches the reply buffer.
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("reply so far ");
        app.start_tool("Read", "f");
        app.end_tool("out", true);
        let before = app.status().unwrap().tokens;
        assert_eq!(app.status().unwrap().arrow, TokenArrow::Up);
        app.push_thinking("weighing the options carefully");
        let status = app.status().unwrap();
        assert!(status.tokens > before, "reasoning text counts tokens");
        assert_eq!(status.arrow, TokenArrow::Down, "thinking streams down");
        assert_eq!(
            app.streaming_text(),
            Some("reply so far "),
            "the reply buffer is untouched by reasoning text"
        );
    }

    #[test]
    fn push_thinking_is_a_no_op_when_idle() {
        let mut app = App::new();
        app.push_thinking("stray reasoning after the turn ended");
        assert!(app.status().is_none(), "no status conjured up");
        assert_eq!(app.streaming_text(), None);
    }

    #[test]
    fn push_tool_call_progress_grows_the_tally_pointing_down_without_touching_the_reply() {
        // While the model *generates* a tool call, the streamed name/argument
        // fragments count into the tally (arrow ↓ — model output) so the status
        // keeps ticking, but the opaque JSON never reaches the reply buffer.
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("let me check ");
        let before = app.status().unwrap().tokens;
        app.push_tool_call_progress(r#"bash{"command":"ls -la"}"#);
        let status = app.status().unwrap();
        assert!(
            status.tokens > before,
            "the tool-call fragment counts tokens"
        );
        assert_eq!(status.arrow, TokenArrow::Down, "generation streams down");
        assert_eq!(
            app.streaming_text(),
            Some("let me check "),
            "the reply buffer is untouched by tool-call JSON"
        );
    }

    #[test]
    fn push_tool_call_progress_is_a_no_op_when_idle() {
        let mut app = App::new();
        app.push_tool_call_progress(r#"{"command":"ls"}"#);
        assert!(app.status().is_none(), "no status conjured up");
        assert_eq!(app.streaming_text(), None);
    }

    #[test]
    fn apply_usage_snaps_the_tally_to_the_real_total() {
        // The provider's usage frame counts what the estimate never saw (the
        // system prompt, the re-sent context), so it REPLACES the ticked
        // estimate rather than adding to it — and later estimates tick on
        // top of the snapped base (docs/prompt-caching.md).
        use crate::stream::TokenUsage;
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("a streamed reply estimate ");
        app.apply_usage(&TokenUsage {
            input: 8080,
            output: 20,
            cached: 8063,
            cache_write: 0,
        });
        assert_eq!(
            app.status().unwrap().tokens,
            8100,
            "the tally snapped to the real input+output"
        );
        let before = app.status().unwrap().tokens;
        app.push_chunk("next round streaming ");
        assert!(
            app.status().unwrap().tokens > before,
            "the next round's estimate ticks on top of the snapped base"
        );
    }

    #[test]
    fn apply_usage_accumulates_across_agent_rounds() {
        // An agentic turn reports one usage frame per round; the tally is the
        // billed sum, not the last round alone.
        use crate::stream::TokenUsage;
        let mut app = App::new();
        app.begin_stream();
        let round = |input, output, cached| TokenUsage {
            input,
            output,
            cached,
            cache_write: 0,
        };
        app.apply_usage(&round(1000, 50, 0));
        app.apply_usage(&round(1200, 30, 900));
        assert_eq!(app.status().unwrap().tokens, 2280, "both rounds billed");
    }

    #[test]
    fn apply_usage_is_a_no_op_when_idle() {
        use crate::stream::TokenUsage;
        let mut app = App::new();
        app.apply_usage(&TokenUsage {
            input: 10,
            output: 10,
            cached: 0,
            cache_write: 0,
        });
        assert!(app.status().is_none(), "no status conjured up");
        app.begin_stream();
        assert_eq!(
            app.status().unwrap().tokens,
            0,
            "an idle report never leaks into the next turn"
        );
    }

    #[test]
    fn the_turn_summary_carries_the_real_usage() {
        // The committed `Done for Ns` summary appends the billed tokens and
        // their cached share when the provider reported usage — and the next
        // turn starts from zero (the accumulators are per-turn).
        use crate::stream::TokenUsage;
        let mut app = App::new();
        app.begin_stream();
        app.apply_usage(&TokenUsage {
            input: 8080,
            output: 123,
            cached: 8063,
            cache_write: 0,
        });
        let summary = app.end_turn(12).expect("a turn was active");
        assert_eq!(summary.tokens, 8203);
        assert_eq!(summary.cached, 8063);

        app.begin_stream();
        let summary = app.end_turn(1).expect("second turn");
        assert_eq!(summary.tokens, 0, "a usage-less turn reports none");
        assert_eq!(summary.cached, 0);
    }

    #[test]
    fn set_status_times_writes_the_boundary_times_onto_the_status() {
        let mut app = App::new();
        app.begin_stream();
        app.set_status_times(Duration::from_secs(7), Some(Duration::from_secs(2)));
        let status = app.status().unwrap();
        assert_eq!(status.elapsed, Duration::from_secs(7));
        assert_eq!(status.thinking, Some(Duration::from_secs(2)));
        // Thinking ends → the suffix is dropped.
        app.set_status_times(Duration::from_secs(8), None);
        assert_eq!(app.status().unwrap().thinking, None);
    }

    #[test]
    fn set_status_times_is_a_no_op_when_idle() {
        let mut app = App::new();
        // No turn → nothing to write.
        app.set_status_times(Duration::from_secs(5), Some(Duration::from_secs(1)));
        assert!(app.status().is_none());
    }

    #[test]
    fn a_fresh_turn_has_no_retry() {
        let mut app = App::new();
        app.begin_stream();
        assert_eq!(app.status().unwrap().retry, None);
    }

    #[test]
    fn set_retry_records_the_attempt_on_the_status() {
        let mut app = App::new();
        app.begin_stream();
        app.set_retry(2, 3);
        assert_eq!(
            app.status().unwrap().retry,
            Some(RetryInfo { attempt: 2, max: 3 })
        );
    }

    #[test]
    fn set_retry_is_a_no_op_when_idle() {
        let mut app = App::new();
        app.set_retry(1, 3);
        assert!(app.status().is_none());
    }

    #[test]
    fn streamed_content_clears_the_retry_indicator() {
        // A retry means a request failed before any byte; once content flows the
        // attempt has succeeded, so the live "retrying" indicator must clear.
        let mut app = App::new();
        app.begin_stream();
        app.set_retry(1, 3);
        app.push_chunk("hello");
        assert_eq!(app.status().unwrap().retry, None, "a chunk clears it");

        app.set_retry(2, 3);
        app.push_thinking("hmm");
        assert_eq!(
            app.status().unwrap().retry,
            None,
            "a reasoning delta clears it too"
        );
    }

    #[test]
    fn end_turn_records_a_summary_and_clears_the_status() {
        let mut app = App::new();
        app.begin_stream();
        let done_verb = app.status().unwrap().done_verb;
        app.push_chunk("a reply");
        app.finish_stream();
        let summary = app.end_turn(20).expect("a turn was active");
        assert_eq!(summary.verb, done_verb, "the summary uses the done verb");
        assert_eq!(summary.secs, 20, "the boundary-supplied duration");
        assert_eq!(
            app.history.last(),
            Some(&HistoryItem::Summary(summary.clone())),
            "the summary is recorded in history so it survives a resize"
        );
        assert!(!app.turn_active(), "the status clears when the turn ends");
    }

    #[test]
    fn end_turn_when_idle_returns_none_and_records_nothing() {
        let mut app = App::new();
        assert!(app.end_turn(3).is_none());
        assert!(app.history.is_empty());
    }

    #[test]
    fn take_turn_summary_builds_without_recording_then_record_pushes() {
        // The split behind end_turn (docs/background.md): take_turn_summary
        // clears the status and builds the summary but does NOT record it, so
        // the boundary can settle a background completion pending at turn end
        // *between* the two — landing the notice above the Done summary in
        // history. record_turn_summary then pushes it.
        let mut app = App::new();
        app.begin_stream();
        let summary = app.take_turn_summary(5).expect("a turn was active");
        assert!(!app.turn_active(), "the status is cleared");
        assert!(
            !app.history
                .iter()
                .any(|i| matches!(i, HistoryItem::Summary(_))),
            "take_turn_summary does not record the summary"
        );
        app.record_turn_summary(summary.clone());
        assert_eq!(
            app.history.last(),
            Some(&HistoryItem::Summary(summary)),
            "record_turn_summary pushes it into history"
        );
    }

    #[test]
    fn take_turn_summary_is_none_for_an_idle_or_shell_turn() {
        let mut idle = App::new();
        assert!(idle.take_turn_summary(1).is_none(), "no turn active");
        let mut shell = App::new();
        shell.begin_shell("sleep 1");
        assert!(
            shell.take_turn_summary(1).is_none(),
            "a `!` shell turn has no Done summary — its cell is the record"
        );
        assert!(!shell.turn_active(), "but the status still clears");
    }

    #[test]
    fn a_completion_pending_at_turn_end_records_above_the_summary() {
        // The turn-end settle ordering (docs/background.md): a background
        // shell that finished during the final assistant text — with no tool
        // call after it to settle at — must still land its notice ABOVE the
        // Done summary, in both history and (via the same order) scrollback,
        // matching the mid-turn tool-boundary placement. This replays the
        // exact app-call sequence the StreamDone arm runs.
        let mut app = App::new();
        app.set_clock(|| STAMP.to_string());
        app.record_user_message("start the server then stop it");
        app.begin_stream();
        app.bg_started(
            "bash_1",
            "python3 server.py",
            Some("API server".into()),
            true,
        );
        let completion = app.bg_exited("bash_1", None, false).expect("it finished");
        app.defer_bg_completion(completion);
        app.push_chunk("Done — the server was stopped.");
        let _ = app.finish_stream();
        // The StreamDone sequence: build the summary (status cleared, not yet
        // recorded), settle the held completion, then record the summary.
        let summary = app.take_turn_summary(7).expect("a turn was active");
        for completion in app.take_pending_bg_completions() {
            app.record_background_notice(&completion);
        }
        app.record_turn_summary(summary);
        let kinds: Vec<&str> = app
            .history
            .iter()
            .map(|item| match item {
                HistoryItem::Message(_) => "message",
                HistoryItem::Tool(_) => "tool",
                HistoryItem::Background(_) => "background",
                HistoryItem::Summary(_) => "summary",
                HistoryItem::Compaction(_) => "compaction",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["message", "message", "background", "summary"],
            "user, assistant reply, THEN the notice, THEN the Done summary"
        );
    }

    #[test]
    fn end_turn_uses_the_clock_for_the_summary_timestamp() {
        let mut app = App::new();
        app.set_clock(|| STAMP.to_string());
        app.begin_stream();
        app.finish_stream();
        let summary = app.end_turn(5).unwrap();
        assert_eq!(summary.timestamp, STAMP, "stamped like every recorded item");
    }

    #[test]
    fn fail_stream_clears_the_status_without_a_summary() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("half a reply");
        app.fail_stream("network down").expect("was streaming");
        assert!(!app.turn_active(), "an error clears the live status");
        assert!(
            !app.history
                .iter()
                .any(|i| matches!(i, HistoryItem::Summary(_))),
            "no Done summary is recorded for a failed turn"
        );
    }

    #[test]
    fn a_full_turn_records_user_assistant_then_a_summary_in_order() {
        let mut app = App::new();
        app.record_user_message("q");
        app.begin_stream();
        app.push_chunk("a");
        app.finish_stream();
        app.end_turn(3);
        match app.history.as_slice() {
            [
                HistoryItem::Message(u),
                HistoryItem::Message(a),
                HistoryItem::Summary(s),
            ] => {
                assert_eq!(u.role, Role::User);
                assert_eq!(a.role, Role::Assistant);
                assert!(DONE_VERBS.contains(&s.verb));
            }
            other => panic!("unexpected history: {other:?}"),
        }
    }

    #[test]
    fn count_tokens_is_zero_for_empty_and_grows_with_length() {
        assert_eq!(count_tokens(""), 0);
        assert!(count_tokens("a") >= 1);
        assert!(count_tokens("a much longer string") > count_tokens("a"));
    }

    // --- Ctrl+R history search: InputHistory::search / entry / resume_at
    // (docs/history-search.md — codex's chat_composer_history search) ---

    /// An InputHistory with `texts` recorded oldest → newest.
    fn history_of(texts: &[&str]) -> InputHistory {
        let mut history = InputHistory::default();
        for text in texts {
            history.record(text);
        }
        history
    }

    #[test]
    fn search_lists_matching_entries_newest_first() {
        let history = history_of(&["git status", "cargo build", "git push"]);
        assert_eq!(history.search("git"), vec![2, 0]);
        assert_eq!(history.entry(2), Some("git push"));
        assert_eq!(history.entry(0), Some("git status"));
    }

    #[test]
    fn search_is_case_insensitive() {
        let history = history_of(&["Build Release", "other"]);
        assert_eq!(history.search("release"), vec![0]);
        assert_eq!(history.search("BUILD"), vec![0]);
    }

    #[test]
    fn search_skips_older_duplicates_of_the_same_text() {
        let history = history_of(&["dup", "other dup", "dup"]);
        // "dup" was recorded at 0 and again at 2 (not adjacent, so both kept);
        // the search keeps only the newest occurrence — codex's seen_texts.
        assert_eq!(history.search("dup"), vec![2, 1]);
    }

    #[test]
    fn search_with_no_match_or_no_entries_is_empty() {
        assert_eq!(history_of(&["alpha"]).search("zzz"), Vec::<usize>::new());
        assert_eq!(InputHistory::default().search("a"), Vec::<usize>::new());
    }

    #[test]
    fn search_with_an_empty_query_matches_everything() {
        let history = history_of(&["one", "two"]);
        assert_eq!(history.search(""), vec![1, 0]);
    }

    #[test]
    fn resume_at_seats_arrow_browsing_at_the_entry() {
        let mut history = history_of(&["oldest", "middle", "newest"]);
        history.resume_at(1);
        // Browsing resumes as if "middle" had just been recalled: ↑ steps to
        // the entry older than it (codex's shared history cursor on accept).
        assert!(history.should_navigate("middle", "middle".len()));
        assert_eq!(history.up(), Some("oldest".to_string()));
    }

    // --- Ctrl+R history search: the session over App (docs/history-search.md —
    // codex's HistorySearchSession in chat_composer/history_search.rs) ---

    /// An App with `texts` submitted (each its own finished-enough turn — the
    /// submit helper records them), composer empty again.
    fn searchable_app(texts: &[&str]) -> App {
        let mut app = App::new();
        for text in texts {
            submit(&mut app, text);
        }
        app
    }

    /// Type `text` into the open search query through the real key path.
    fn type_query(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn ctrl_r_opens_an_idle_search_without_previewing() {
        let mut app = searchable_app(&["git status"]);
        assert_eq!(app.on_key(ctrl('r')), Action::None);
        let search = app.history_search.as_ref().expect("search open");
        assert_eq!(search.query, "");
        assert_eq!(search.state, SearchState::Idle);
        assert_eq!(app.input.text(), "", "no preview until a query is typed");
    }

    #[test]
    fn typing_builds_the_query_and_previews_the_newest_match() {
        let mut app = searchable_app(&["git status", "cargo build", "git push"]);
        app.on_key(ctrl('r'));
        type_query(&mut app, "git");
        let search = app.history_search.as_ref().expect("search open");
        assert_eq!(search.query, "git");
        assert_eq!(search.state, SearchState::Match { selected: 0 });
        assert_eq!(app.input.text(), "git push", "newest match previews");
    }

    #[test]
    fn a_paste_mid_search_extends_the_query_not_the_preview() {
        // The search owns *every* key — a bracketed paste is input too, so it
        // extends the query (readline's paste-into-isearch) instead of editing
        // the previewed match, whose text the next rerun/cancel would discard.
        let mut app = searchable_app(&["git status", "git push"]);
        app.on_key(ctrl('r'));
        app.on_paste("git st");
        let search = app.history_search.as_ref().expect("search stays open");
        assert_eq!(search.query, "git st");
        assert_eq!(search.state, SearchState::Match { selected: 0 });
        assert_eq!(app.input.text(), "git status", "the match previews");
    }

    #[test]
    fn a_paste_mid_search_never_flips_shell_mode() {
        // begin_history_search suspends shell mode; a pasted `!` must not
        // re-enter it through on_paste's sync_shell_mode while the search owns
        // the composer.
        let mut app = searchable_app(&["!ls"]);
        app.on_key(ctrl('r'));
        app.on_paste("!ls");
        assert!(!app.shell_mode, "the search owns the composer");
        let search = app.history_search.as_ref().expect("search open");
        assert_eq!(search.query, "!ls");
        assert_eq!(app.input.text(), "!ls", "previewed raw, bang and all");
    }

    #[test]
    fn a_paste_mid_search_never_opens_the_file_picker() {
        let mut app = searchable_app(&["look at @src/app.rs please"]);
        app.on_key(ctrl('r'));
        app.on_paste("@src");
        assert!(app.file_search.is_none(), "the search owns the band slot");
    }

    #[test]
    fn a_large_paste_mid_search_leaves_no_orphaned_placeholder() {
        let mut app = searchable_app(&["hello"]);
        app.on_key(ctrl('r'));
        let big = "z".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
        app.on_paste(&big);
        assert!(app.pasted.is_empty(), "no placeholder pair is recorded");
        assert!(
            !app.input.text().starts_with("[Pasted Content"),
            "no placeholder lands in the preview"
        );
    }

    #[test]
    fn a_paste_mid_search_flattens_control_characters_into_the_query() {
        // The query renders on a single footer row — a pasted newline or tab
        // would corrupt it (and '\t' breaks the cursor math, like the composer).
        let mut app = searchable_app(&["a b"]);
        app.on_key(ctrl('r'));
        app.on_paste("a\tb\nc");
        let search = app.history_search.as_ref().expect("search open");
        assert_eq!(search.query, "a b c");
    }

    #[test]
    fn ctrl_r_steps_older_and_clamps_at_the_oldest_match() {
        let mut app = searchable_app(&["git status", "cargo build", "git push"]);
        app.on_key(ctrl('r'));
        type_query(&mut app, "git");
        app.on_key(ctrl('r'));
        assert_eq!(app.input.text(), "git status");
        // At the boundary the match is kept (codex's AtBoundary — no flicker).
        app.on_key(ctrl('r'));
        assert_eq!(app.input.text(), "git status");
        let search = app.history_search.as_ref().expect("search open");
        assert_eq!(search.state, SearchState::Match { selected: 1 });
    }

    #[test]
    fn ctrl_s_steps_back_newer_and_clamps_at_the_newest_match() {
        let mut app = searchable_app(&["git status", "cargo build", "git push"]);
        app.on_key(ctrl('r'));
        type_query(&mut app, "git");
        app.on_key(ctrl('r'));
        assert_eq!(app.input.text(), "git status");
        app.on_key(ctrl('s'));
        assert_eq!(app.input.text(), "git push");
        app.on_key(ctrl('s'));
        assert_eq!(app.input.text(), "git push", "newest end clamps too");
    }

    #[test]
    fn up_and_down_step_the_search_instead_of_browsing_or_moving() {
        let mut app = searchable_app(&["git status", "cargo build", "git push"]);
        app.on_key(ctrl('r'));
        type_query(&mut app, "git");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "git status", "↑ steps older, like Ctrl+R");
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.input.text(), "git push", "↓ steps newer, like Ctrl+S");
        assert!(
            app.history_search.is_some(),
            "arrows never close the search"
        );
    }

    #[test]
    fn backspace_pops_the_query_and_restarts_from_the_newest() {
        let mut app = searchable_app(&["git status", "git push"]);
        app.on_key(ctrl('r'));
        type_query(&mut app, "gitz");
        let search = app.history_search.as_ref().expect("search open");
        assert_eq!(search.state, SearchState::NoMatch);
        app.on_key(key(KeyCode::Backspace));
        let search = app.history_search.as_ref().expect("search open");
        assert_eq!(search.query, "git");
        assert_eq!(search.state, SearchState::Match { selected: 0 });
        assert_eq!(app.input.text(), "git push");
    }

    #[test]
    fn ctrl_u_clears_the_query_back_to_idle_and_restores_the_draft() {
        let mut app = searchable_app(&["git status"]);
        app.input = TextArea::from_text("a draft");
        app.on_key(ctrl('r'));
        type_query(&mut app, "git");
        assert_eq!(app.input.text(), "git status");
        app.on_key(ctrl('u'));
        let search = app.history_search.as_ref().expect("search open");
        assert_eq!(search.query, "");
        assert_eq!(search.state, SearchState::Idle);
        assert_eq!(app.input.text(), "a draft");
    }

    #[test]
    fn a_no_match_query_restores_the_draft_and_keeps_the_search_open() {
        let mut app = searchable_app(&["git status"]);
        app.input = TextArea::from_text("a draft");
        app.on_key(ctrl('r'));
        type_query(&mut app, "zzz");
        let search = app.history_search.as_ref().expect("search stays open");
        assert_eq!(search.state, SearchState::NoMatch);
        assert_eq!(app.input.text(), "a draft");
    }

    #[test]
    fn enter_accepts_the_match_into_the_composer_without_submitting() {
        let mut app = searchable_app(&["git status", "git push"]);
        app.on_key(ctrl('r'));
        type_query(&mut app, "status");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert!(app.history_search.is_none(), "accepting closes the search");
        assert_eq!(app.input.text(), "git status", "the match stays as a draft");
        assert_eq!(app.input.cursor(), "git status".len());
        assert!(app.queued.is_empty());
    }

    #[test]
    fn enter_without_a_match_is_swallowed_and_the_search_stays_open() {
        let mut app = searchable_app(&["git status"]);
        app.on_key(ctrl('r'));
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert!(app.history_search.is_some(), "Idle: Enter does nothing");
        type_query(&mut app, "zzz");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert!(app.history_search.is_some(), "NoMatch: Enter does nothing");
    }

    #[test]
    fn accepting_seats_arrow_browsing_at_the_match() {
        let mut app = searchable_app(&["alpha one", "beta two", "alpha three"]);
        app.on_key(ctrl('r'));
        type_query(&mut app, "two");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.input.text(), "beta two");
        // ↑ continues *older* from the accepted entry, codex-style.
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text(), "alpha one");
    }

    #[test]
    fn esc_cancels_and_restores_the_draft_text_and_cursor() {
        let mut app = searchable_app(&["git status"]);
        app.input = TextArea::from_text("hello");
        app.input.move_left();
        app.input.move_left();
        let cursor = app.input.cursor();
        app.on_key(ctrl('r'));
        type_query(&mut app, "git");
        assert_eq!(app.input.text(), "git status");
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        assert!(app.history_search.is_none());
        assert_eq!(app.input.text(), "hello");
        assert_eq!(app.input.cursor(), cursor, "the exact cursor is restored");
    }

    #[test]
    fn ctrl_c_cancels_the_search_instead_of_clearing_or_quitting() {
        let mut app = searchable_app(&["git status"]);
        app.input = TextArea::from_text("a draft");
        app.on_key(ctrl('r'));
        type_query(&mut app, "git");
        assert_eq!(app.on_key(ctrl('c')), Action::None);
        assert!(app.history_search.is_none());
        assert_eq!(app.input.text(), "a draft", "the restored draft survives");
    }

    #[test]
    fn esc_mid_turn_cancels_the_search_not_the_turn() {
        let mut app = searchable_app(&["git status"]);
        app.begin_stream();
        app.on_key(ctrl('r'));
        assert_eq!(
            app.on_key(key(KeyCode::Esc)),
            Action::None,
            "search-cancel wins over interrupt, like palette-dismiss"
        );
        assert!(app.history_search.is_none());
        assert!(app.is_streaming(), "the turn keeps streaming");
    }

    #[test]
    fn ctrl_o_during_a_search_cancels_it_and_opens_the_overlay() {
        let mut app = searchable_app(&["git status"]);
        app.input = TextArea::from_text("a draft");
        app.on_key(ctrl('r'));
        type_query(&mut app, "git");
        assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
        assert_eq!(app.view, View::ToolOutput);
        assert!(
            app.history_search.is_none(),
            "no search leaks into the overlay"
        );
        assert_eq!(app.input.text(), "a draft");
    }

    #[test]
    fn opening_the_search_closes_the_palette_and_canceling_keeps_it_closed() {
        let mut app = searchable_app(&["git status"]);
        app.input = TextArea::new();
        type_query(&mut app, "/he"); // typing a bare /token opens the palette
        assert!(app.command_menu.is_some());
        app.on_key(ctrl('r'));
        assert!(app.command_menu.is_none(), "search owns the keys");
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.input.text(), "/he");
        assert!(app.command_menu.is_none(), "cancel restores the draft only");
    }

    #[test]
    fn a_previewed_slash_token_does_not_open_the_palette_but_accepting_does() {
        let mut app = App::new();
        // A bare /token can enter the history via Ctrl+C's composer-clear.
        app.input = TextArea::from_text("/help");
        app.on_key(ctrl('c'));
        app.on_key(ctrl('r'));
        type_query(&mut app, "help");
        assert_eq!(app.input.text(), "/help");
        assert!(app.command_menu.is_none(), "previews never pop the palette");
        app.on_key(key(KeyCode::Enter));
        assert!(
            app.command_menu.is_some(),
            "accepting re-derives the palette, like ↑-recall"
        );
    }

    #[test]
    fn searching_with_no_history_shows_no_match() {
        let mut app = App::new();
        app.on_key(ctrl('r'));
        type_query(&mut app, "a");
        let search = app.history_search.as_ref().expect("search open");
        assert_eq!(search.state, SearchState::NoMatch);
    }

    #[test]
    fn highlight_ranges_cover_the_query_in_the_preview_case_insensitively() {
        let mut app = searchable_app(&["Git status on git repo"]);
        app.on_key(ctrl('r'));
        type_query(&mut app, "git");
        let text = app.input.text();
        let ranges = app.search_highlight_ranges();
        let covered: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(covered, vec!["Git", "git"]);
    }

    #[test]
    fn highlight_ranges_are_empty_unless_a_match_is_previewed() {
        let mut app = searchable_app(&["git status"]);
        assert!(app.search_highlight_ranges().is_empty(), "no search open");
        app.on_key(ctrl('r'));
        assert!(app.search_highlight_ranges().is_empty(), "Idle");
        type_query(&mut app, "zzz");
        assert!(app.search_highlight_ranges().is_empty(), "NoMatch");
        type_query(&mut app, ""); // unchanged
        app.on_key(ctrl('u'));
        type_query(&mut app, "git");
        app.on_key(key(KeyCode::Enter));
        assert!(
            app.search_highlight_ranges().is_empty(),
            "accepted = plain draft"
        );
    }

    // --- `!` shell commands (docs/shell-command.md — codex's bang shell mode,
    // with the absorbed `!` prefix: the bang becomes the prompt, not text) ---

    #[test]
    fn shell_query_strips_a_leading_bang_keeping_the_rest_verbatim() {
        assert_eq!(shell_query("!ls -la"), Some("ls -la"));
        assert_eq!(shell_query("!"), Some(""), "a lone ! is empty shell mode");
        assert_eq!(shell_query("! echo hi"), Some(" echo hi"), "spaces kept");
        assert_eq!(shell_query("!/usr/bin/env"), Some("/usr/bin/env"));
    }

    #[test]
    fn shell_query_is_none_without_a_leading_bang() {
        assert_eq!(shell_query(""), None);
        assert_eq!(shell_query("ls"), None);
        assert_eq!(shell_query("ask !ls"), None, "the ! must lead");
        assert_eq!(shell_query("/help"), None);
    }

    #[test]
    fn typing_a_bang_into_an_empty_composer_enters_shell_mode() {
        // codex's absorbed prefix: the `!` flips the mode flag and is *not*
        // inserted — the prompt renders it instead (`! pwd`, not `❯ !pwd`).
        let mut app = App::new();
        type_query(&mut app, "!pwd");
        assert!(app.shell_mode);
        assert_eq!(app.input.text(), "pwd", "the bang is absorbed, not typed");
    }

    #[test]
    fn a_bang_typed_mid_text_is_just_a_character() {
        let mut app = App::new();
        type_query(&mut app, "hi!");
        assert!(!app.shell_mode);
        assert_eq!(app.input.text(), "hi!");
    }

    #[test]
    fn an_edit_that_creates_a_leading_bang_absorbs_it() {
        // codex syncs the mode from the text after every edit, so inserting a
        // `!` in front of an existing draft enters shell mode too.
        let mut app = App::new();
        type_query(&mut app, "ls");
        app.on_key(key(KeyCode::Home));
        type_query(&mut app, "!");
        assert!(app.shell_mode);
        assert_eq!(app.input.text(), "ls");
    }

    #[test]
    fn absorbing_a_typed_bang_keeps_the_cursor_where_it_was() {
        // Only the bang leaves the text — the cursor must not teleport to the
        // end: type "ls", Home, "!" (cursor now before the "l"), then "x"
        // continues typing there, not after the "s".
        let mut app = App::new();
        type_query(&mut app, "ls");
        app.on_key(key(KeyCode::Home));
        type_query(&mut app, "!");
        assert_eq!(app.input.cursor(), 0, "the cursor stays before the command");
        type_query(&mut app, "x");
        assert_eq!(app.input.text(), "xls");
    }

    #[test]
    fn absorbing_a_bang_exposed_by_backspace_keeps_the_cursor_at_the_front() {
        // Draft "x!ls" with the cursor after the "x": Backspace exposes the
        // leading bang, which is absorbed — the cursor stays at the front of
        // the remaining command rather than jumping past "ls".
        let mut app = App::new();
        type_query(&mut app, "x!ls");
        for _ in 0..3 {
            app.on_key(key(KeyCode::Left));
        }
        app.on_key(key(KeyCode::Backspace));
        assert!(app.shell_mode);
        assert_eq!(app.input.text(), "ls");
        assert_eq!(app.input.cursor(), 0);
    }

    #[test]
    fn backspace_on_an_empty_shell_composer_exits_shell_mode() {
        let mut app = App::new();
        type_query(&mut app, "!");
        app.on_key(key(KeyCode::Backspace));
        assert!(!app.shell_mode, "backspace deletes the absorbed bang");
        assert_eq!(app.input.text(), "");
    }

    #[test]
    fn esc_on_an_empty_shell_composer_exits_the_mode_instead_of_quitting() {
        let mut app = App::new();
        type_query(&mut app, "!");
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        assert!(!app.shell_mode, "Esc exits shell mode (codex)");
        // A second Esc, now out of the mode, quits as usual.
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Quit);
    }

    #[test]
    fn the_palette_never_opens_in_shell_mode() {
        let mut app = App::new();
        type_query(&mut app, "!/he");
        assert!(app.shell_mode);
        assert_eq!(app.input.text(), "/he");
        assert!(
            app.command_menu.is_none(),
            "a /token inside a shell command is literal"
        );
    }

    #[test]
    fn question_mark_types_into_shell_mode_not_the_band() {
        let mut app = App::new();
        type_query(&mut app, "!");
        type_query(&mut app, "?");
        assert!(!app.shortcuts_open, "`?` is a shell character here");
        assert_eq!(app.input.text(), "?");
    }

    #[test]
    fn ctrl_c_in_shell_mode_records_the_prefixed_draft_and_exits_the_mode() {
        let mut app = App::new();
        type_query(&mut app, "!ls");
        assert_eq!(app.on_key(ctrl('c')), Action::None);
        assert!(!app.shell_mode);
        assert_eq!(app.input.text(), "");
        app.on_key(key(KeyCode::Up));
        assert!(app.shell_mode, "↑ recalls the cleared draft back into mode");
        assert_eq!(app.input.text(), "ls");
    }

    #[test]
    fn idle_enter_in_shell_mode_runs_the_command() {
        let mut app = App::new();
        type_query(&mut app, "!echo hello");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::RunShell("echo hello".to_string())
        );
        assert_eq!(app.input.text(), "", "the composer is consumed");
        assert!(!app.shell_mode, "running exits the mode");
    }

    #[test]
    fn recalling_a_bang_entry_restores_shell_mode() {
        let mut app = App::new();
        type_query(&mut app, "!echo hello");
        app.on_key(key(KeyCode::Enter));
        // ↑ recalls the recorded "!echo hello" — absorbed back into the mode
        // (codex records the full text and re-absorbs it on recall).
        app.on_key(key(KeyCode::Up));
        assert!(app.shell_mode);
        assert_eq!(app.input.text(), "echo hello");
    }

    #[test]
    fn recalling_a_plain_entry_clears_shell_mode() {
        let mut app = App::new();
        submit(&mut app, "hello there");
        type_query(&mut app, "!");
        app.on_key(key(KeyCode::Up));
        assert!(
            !app.shell_mode,
            "a recalled plain message replaces the mode"
        );
        assert_eq!(app.input.text(), "hello there");
    }

    #[test]
    fn the_shell_command_is_trimmed_before_running() {
        let mut app = App::new();
        type_query(&mut app, "!  ls -la  ");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::RunShell("ls -la".to_string())
        );
    }

    #[test]
    fn an_empty_bang_posts_the_help_notice_and_stays_in_the_mode() {
        let mut app = App::new();
        type_query(&mut app, "!");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Notice(SHELL_EMPTY_NOTICE.to_string())
        );
        assert!(
            app.shell_mode,
            "codex keeps the mode open on the help notice"
        );
        type_query(&mut app, "   ");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Notice(SHELL_EMPTY_NOTICE.to_string())
        );
    }

    #[test]
    fn a_bang_command_mid_turn_queues_as_a_standalone_shell_entry() {
        // codex parity (submit_queued_shell_prompt): a !command typed while a
        // turn streams queues as its own Shell entry — run locally when its turn
        // comes — NOT concatenated into a text batch and sent to the backend as
        // literal text (the old v1 limitation). The full `!command` is recorded
        // for ↑ recall, and submitting exits the mode.
        let mut app = App::new();
        app.begin_stream();
        type_query(&mut app, "!echo hi");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert!(!app.shell_mode, "submitting exits shell mode");
        assert_eq!(
            app.drain_next_batch(),
            Some(QueuedTurn::Shell("echo hi".to_string())),
            "queued as a local shell command, not literal text"
        );
        assert_eq!(
            app.input_history.up(),
            Some("!echo hi".to_string()),
            "the full !command is recorded for ↑ recall"
        );
    }

    #[test]
    fn a_queued_shell_entry_is_never_merged_with_text() {
        // A Shell entry stands alone: an Enter-text after it opens a *fresh*
        // Messages batch (the back is a Shell, not a Messages), so the command
        // never gets concatenated into a text turn — codex's per-completion
        // shell dispatch ("cannot be added to").
        let mut app = App::new();
        app.begin_stream();
        app.input = TextArea::from_text("hello");
        app.on_key(key(KeyCode::Enter)); // Messages(["hello"])
        type_query(&mut app, "!ls");
        app.on_key(key(KeyCode::Enter)); // Shell("ls") — standalone
        app.input = TextArea::from_text("world");
        app.on_key(key(KeyCode::Enter)); // a NEW Messages(["world"]), not merged
        assert_eq!(
            app.queued,
            VecDeque::from(vec![
                batch(&["hello"]),
                QueuedTurn::Shell("ls".to_string()),
                batch(&["world"]),
            ]),
            "the shell entry stands alone between the two text batches"
        );
    }

    #[test]
    fn two_mid_turn_shell_commands_queue_as_separate_entries() {
        // Each !command is individual: two of them mid-turn become two Shell
        // entries (each its own local run), never one merged blob.
        let mut app = App::new();
        app.begin_stream();
        type_query(&mut app, "!one");
        app.on_key(key(KeyCode::Enter));
        type_query(&mut app, "!two");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            app.queued,
            VecDeque::from(vec![
                QueuedTurn::Shell("one".to_string()),
                QueuedTurn::Shell("two".to_string()),
            ])
        );
    }

    #[test]
    fn alt_up_pulls_a_queued_shell_entry_back_into_shell_mode() {
        // The user chose: Alt+Up over a queued !command yanks it back into the
        // composer *re-entering shell mode* (the red `! ` prompt), ready to
        // edit/re-run — recalling `!command` re-absorbs the bang.
        let mut app = App::new();
        app.begin_stream();
        type_query(&mut app, "!deploy --prod");
        app.on_key(key(KeyCode::Enter)); // Shell("deploy --prod")
        assert_eq!(app.queued.len(), 1);
        assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
        assert!(app.shell_mode, "Alt+Up re-enters shell mode");
        assert_eq!(
            app.input.text(),
            "deploy --prod",
            "the command returns with its bang absorbed into the mode"
        );
        assert!(
            app.queued.is_empty(),
            "the entry was pulled out of the queue"
        );
    }

    #[test]
    fn searching_from_shell_mode_previews_plainly_and_esc_restores_the_mode() {
        let mut app = App::new();
        submit(&mut app, "git status");
        type_query(&mut app, "!pw");
        app.on_key(ctrl('r'));
        assert!(!app.shell_mode, "previews show raw text, outside the mode");
        type_query(&mut app, "git");
        assert_eq!(app.input.text(), "git status");
        app.on_key(key(KeyCode::Esc));
        assert!(app.shell_mode, "cancel restores the shell-mode draft");
        assert_eq!(app.input.text(), "pw");
    }

    #[test]
    fn accepting_a_bang_match_restores_shell_mode() {
        let mut app = App::new();
        type_query(&mut app, "!echo hello");
        app.on_key(key(KeyCode::Enter));
        app.on_key(ctrl('r'));
        type_query(&mut app, "echo");
        app.on_key(key(KeyCode::Enter)); // accept "!echo hello"
        assert!(app.shell_mode, "the accepted !entry re-enters the mode");
        assert_eq!(app.input.text(), "echo hello");
    }

    #[test]
    fn begin_shell_records_the_command_and_runs_it_as_the_turn_tool() {
        let mut app = App::new();
        app.begin_shell("ls -la");
        assert!(app.turn_active(), "the status strip shows");
        assert!(
            app.is_streaming(),
            "an empty stream buffer so the strip shows and mid-run Enter queues"
        );
        // The command itself is the cell's header — a Role::Shell message
        // recorded up front so a mid-run resize repaints it.
        match app.history.as_slice() {
            [HistoryItem::Message(m)] => {
                assert_eq!(m.role, Role::Shell);
                assert_eq!(m.text, "ls -la");
            }
            other => panic!("unexpected history: {other:?}"),
        }
        let tool = app.current_tool().expect("the command runs as a tool");
        assert_eq!(tool.name, "ls -la");
        assert!(tool.shell, "marked shell so it renders headerless (⎿ only)");
        assert_eq!(tool.status, ToolStatus::Running);
        let status = app.status().expect("a live status");
        assert_eq!(status.verb, SHELL_VERB);
        assert!(status.shell);
    }

    #[test]
    fn a_shell_turn_ends_without_a_summary_or_phantom_message() {
        let mut app = App::new();
        app.begin_shell("echo hi");
        app.end_tool("hi", true);
        assert!(app.finish_stream().is_none(), "no assistant text to record");
        assert!(
            app.end_turn(2).is_none(),
            "no `Ran for Ns` line — the cell itself is the record"
        );
        assert!(!app.turn_active(), "the status is still cleared");
        // history = [Message(Shell), Tool] — the mock's two-line cell.
        match app.history.as_slice() {
            [HistoryItem::Message(m), HistoryItem::Tool(t)] => {
                assert_eq!(m.role, Role::Shell);
                assert_eq!(t.name, "echo hi");
            }
            other => panic!("unexpected history: {other:?}"),
        }
    }

    #[test]
    fn interrupting_a_shell_turn_resolves_the_command_as_failed() {
        let mut app = App::new();
        app.begin_shell("sleep 5");
        let InterruptedTurn::Kept { tool, notice, .. } =
            app.interrupt_turn().expect("a turn was in flight")
        else {
            panic!("a shell turn has a running tool, so it is kept, not undone");
        };
        let tool = tool.expect("the running command is resolved");
        assert_eq!(tool.name, "sleep 5");
        assert_eq!(tool.status, ToolStatus::Failed);
        assert_eq!(tool.output, INTERRUPT_TOOL_OUTPUT);
        assert!(app.current_tool().is_none());
        assert!(!app.turn_active());
        // Req 2: the `⎿ Interrupted by user` cell is the record — a shell turn
        // commits no redundant `Conversation interrupted` notice.
        assert_eq!(notice, None, "shell interrupt commits no notice");
        assert!(
            !roles(&app).contains(&Role::Error),
            "no `Conversation interrupted` error message for a shell interrupt"
        );
    }

    // ===== `@` file picker (docs/file-search.md) =====

    #[test]
    fn typing_at_opens_the_file_picker() {
        let mut app = App::new();
        type_str(&mut app, "see @al");
        assert!(app.file_search.is_some());
        assert_eq!(app.file_search_query().as_deref(), Some("al"));
    }

    #[test]
    fn the_file_picker_does_not_open_for_an_email() {
        let mut app = App::new();
        type_str(&mut app, "mail@host");
        assert!(app.file_search.is_none());
        assert_eq!(app.file_search_query(), None);
    }

    #[test]
    fn esc_dismisses_the_file_picker_and_stays_dismissed_in_the_token() {
        let mut app = App::new();
        type_str(&mut app, "@a");
        assert!(app.file_search.is_some());
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        assert!(app.file_search.is_none());
        // Editing within the same token does not reopen it (sticky, like the palette).
        type_str(&mut app, "b");
        assert!(app.file_search.is_none());
    }

    #[test]
    fn enter_accepts_the_highlighted_file_replacing_the_token() {
        let mut app = App::new();
        type_str(&mut app, "see @ma");
        app.set_file_matches("ma", vec![fm("src/main.rs")]);
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert_eq!(app.input.text(), "see src/main.rs ");
        assert!(app.file_search.is_none());
    }

    #[test]
    fn tab_accepts_the_highlighted_file_too() {
        let mut app = App::new();
        type_str(&mut app, "@m");
        app.set_file_matches("m", vec![fm("a.rs"), fm("b.rs")]);
        app.on_key(key(KeyCode::Down)); // pick the second match
        assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
        assert_eq!(app.input.text(), "b.rs ");
    }

    #[test]
    fn up_down_move_the_file_selection_clamped() {
        let mut app = App::new();
        type_str(&mut app, "@x");
        app.set_file_matches("x", vec![fm("x1"), fm("x2")]);
        assert_eq!(app.file_search.as_ref().unwrap().selected, 0);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.file_search.as_ref().unwrap().selected, 1);
        app.on_key(key(KeyCode::Down)); // clamp at the last match
        assert_eq!(app.file_search.as_ref().unwrap().selected, 1);
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.file_search.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn stale_file_matches_are_dropped() {
        let mut app = App::new();
        type_str(&mut app, "@ab");
        app.set_file_matches("a", vec![fm("axe")]); // results for the OLD query
        assert!(app.file_search.as_ref().unwrap().matches.is_empty());
        app.set_file_matches("ab", vec![fm("abc")]); // results for the live query
        assert_eq!(app.file_search.as_ref().unwrap().matches.len(), 1);
    }

    #[test]
    fn a_shrinking_result_refresh_clamps_the_file_selection() {
        // An async refresh can come back with fewer matches than the row the
        // highlight sits on — the highlight must be pulled back in bounds or
        // Tab/Enter accept nothing (and the render highlights no row).
        let mut app = App::new();
        type_str(&mut app, "@x");
        app.set_file_matches("x", vec![fm("x1"), fm("x2"), fm("x3")]);
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Down)); // highlight the third match
        assert_eq!(app.file_search.as_ref().unwrap().selected, 2);
        app.set_file_matches("x", vec![fm("x9")]); // the list shrank to one
        assert_eq!(app.file_search.as_ref().unwrap().selected, 0, "clamped");
        assert_eq!(
            app.highlighted_file().map(|m| m.path.as_str()),
            Some("x9"),
            "the highlight lands on a real row"
        );
    }

    #[test]
    fn an_empty_result_refresh_leaves_a_harmless_selection() {
        let mut app = App::new();
        type_str(&mut app, "@x");
        app.set_file_matches("x", vec![fm("x1"), fm("x2")]);
        app.on_key(key(KeyCode::Down));
        app.set_file_matches("x", Vec::new()); // everything filtered away
        assert_eq!(app.file_search.as_ref().unwrap().selected, 0);
        assert!(app.highlighted_file().is_none(), "nothing to accept");
        assert_eq!(
            app.on_key(key(KeyCode::Tab)),
            Action::None,
            "Tab with no match falls through harmlessly"
        );
    }

    #[test]
    fn the_file_picker_stays_closed_in_shell_mode() {
        let mut app = App::new();
        type_str(&mut app, "!ls @a");
        assert!(app.shell_mode);
        assert!(app.file_search.is_none());
    }

    #[test]
    fn accepting_a_path_with_spaces_quotes_it() {
        let mut app = App::new();
        type_str(&mut app, "@my");
        app.set_file_matches("my", vec![fm("my docs/notes.md")]);
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.input.text(), "\"my docs/notes.md\" ");
    }

    #[test]
    fn submitting_an_unmatched_at_query_closes_the_picker() {
        let mut app = App::new();
        type_str(&mut app, "hi @zzz"); // no matches → nothing highlighted
        assert!(app.file_search.is_some());
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Submit("hi @zzz".to_string()));
        assert!(app.file_search.is_none());
    }

    // --- Esc-Esc backtrack: edit a previous message (docs/backtrack.md) ---

    /// Run one full finished exchange (user → assistant → summary) so the app
    /// ends idle again, exactly as a real completed turn leaves it.
    fn exchange(app: &mut App, user: &str, reply: &str) {
        app.record_user_message(user);
        app.begin_stream();
        app.push_chunk(reply);
        app.finish_stream();
        app.end_turn(1);
    }

    #[test]
    fn esc_with_a_previous_user_message_primes_instead_of_quitting() {
        let mut app = App::new();
        exchange(&mut app, "hello", "hi");
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        assert!(app.backtrack.primed, "first Esc arms the gesture (codex)");
    }

    #[test]
    fn esc_with_only_non_user_history_still_quits() {
        // Nothing to backtrack to — a summary/error/system-only history has no
        // user prompt to edit, so Esc keeps its old idle meaning: quit.
        let mut app = App::new();
        app.begin_stream();
        app.finish_stream();
        app.end_turn(1); // history holds just the turn summary
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Quit);
        assert!(!app.backtrack.primed);
    }

    #[test]
    fn esc_with_a_draft_neither_primes_nor_quits() {
        // Priming requires an empty composer (codex's composer_is_empty
        // guard) — and idle Esc with a draft is a no-op like codex's, never
        // a quit that throws typed work away (Ctrl+C is the composer-clear).
        let mut app = App::new();
        exchange(&mut app, "hello", "hi");
        app.input = TextArea::from_text("draft");
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        assert!(!app.backtrack.primed);
        assert_eq!(app.input.text(), "draft", "the draft is untouched");
    }

    #[test]
    fn esc_with_a_draft_and_no_history_does_not_quit_either() {
        // The no-op holds regardless of whether a backtrack target exists:
        // quitting on Esc requires an *empty* composer (codex never quits on
        // Esc at all; ours only does with nothing typed and nothing to edit).
        let mut app = App::new();
        app.input = TextArea::from_text("draft");
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        assert_eq!(app.input.text(), "draft");
    }

    #[test]
    fn any_other_key_unprimes() {
        // Codex resets priming on any non-Esc key — no timeout, no sticky arm.
        let mut app = App::new();
        exchange(&mut app, "hello", "hi");
        app.on_key(key(KeyCode::Esc));
        assert!(app.backtrack.primed);
        app.on_key(key(KeyCode::Char('x')));
        assert!(!app.backtrack.primed, "typing disarms");
        assert_eq!(app.input.text(), "x", "and the key still does its job");
    }

    #[test]
    fn esc_mid_turn_still_interrupts_not_primes() {
        let mut app = App::new();
        exchange(&mut app, "hello", "hi");
        app.begin_stream();
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Interrupt);
        assert!(!app.backtrack.primed, "interrupt wins while a turn runs");
    }

    #[test]
    fn second_esc_opens_the_transcript_preview_on_the_last_user_message() {
        let mut app = App::new();
        exchange(&mut app, "first", "a");
        exchange(&mut app, "second", "b");
        app.on_key(key(KeyCode::Esc)); // prime
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::ToggleToolView);
        assert_eq!(app.view, View::ToolOutput, "the transcript overlay opens");
        assert_eq!(
            app.backtrack.selected,
            Some(1),
            "the newest user message is highlighted first"
        );
        assert!(
            !app.tool_follow,
            "previewing pins to the highlight, not the tail"
        );
    }

    #[test]
    fn esc_and_left_step_the_preview_older_saturating() {
        let mut app = App::new();
        exchange(&mut app, "one", "a");
        exchange(&mut app, "two", "b");
        exchange(&mut app, "three", "c");
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.backtrack.selected, Some(2));
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        assert_eq!(app.backtrack.selected, Some(1), "Esc steps older");
        app.on_key(key(KeyCode::Left));
        assert_eq!(app.backtrack.selected, Some(0), "← steps older too");
        app.on_key(key(KeyCode::Esc));
        assert_eq!(
            app.backtrack.selected,
            Some(0),
            "stepping stops at the oldest (codex saturates)"
        );
    }

    #[test]
    fn right_steps_the_preview_newer_clamped() {
        let mut app = App::new();
        exchange(&mut app, "one", "a");
        exchange(&mut app, "two", "b");
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Esc)); // step older → 0
        app.on_key(key(KeyCode::Right));
        assert_eq!(app.backtrack.selected, Some(1), "→ steps newer");
        app.on_key(key(KeyCode::Right));
        assert_eq!(app.backtrack.selected, Some(1), "…clamped at the newest");
    }

    #[test]
    fn enter_confirms_truncating_history_and_prefilling_the_composer() {
        let mut app = App::new();
        exchange(&mut app, "first", "a");
        exchange(&mut app, "second", "b");
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Esc)); // preview on "second"
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::ConfirmBacktrack);
        assert_eq!(app.view, View::Conversation, "back to the inline view");
        assert_eq!(app.input.text(), "second", "the message is back to edit");
        assert_eq!(app.input.cursor(), "second".len(), "cursor at the end");
        // The selected message and everything after it are gone; the first
        // exchange (user + assistant + summary) survives untouched.
        assert_eq!(app.history.len(), 3);
        assert_eq!(message_at(&app, 0).text, "first");
        assert_eq!(
            app.backtrack,
            Backtrack::default(),
            "the gesture state fully resets"
        );
    }

    #[test]
    fn enter_restores_the_rewound_messages_image_attachments() {
        // The rewound message's Ctrl+V attachments come back with its text —
        // the interrupt-undo dance — so resubmitting still sends the image;
        // and the dropped *later* user message's attachment is orphaned, so
        // its temp file is queued for deletion.
        let mut app = App::new();
        app.record_user_message_with_images("[Image #1] look", vec![PathBuf::from("/a.png")]);
        app.begin_stream();
        app.push_chunk("a reply");
        app.finish_stream();
        app.end_turn(1);
        app.record_user_message_with_images("[Image #1] later", vec![PathBuf::from("/b.png")]);
        app.begin_stream();
        app.push_chunk("b reply");
        app.finish_stream();
        app.end_turn(1);
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Esc)); // preview on the later message
        app.on_key(key(KeyCode::Esc)); // step to the first
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.input.text(), "[Image #1] look");
        assert_eq!(
            app.images,
            vec![("[Image #1]".to_string(), PathBuf::from("/a.png"))],
            "the placeholder is backed by its path again"
        );
        assert_eq!(
            app.take_discarded_images(),
            vec![PathBuf::from("/b.png")],
            "the dropped later message's temp file is queued for deletion"
        );
    }

    #[test]
    fn enter_discards_a_drafted_attachment_the_rewind_clobbers() {
        // A draft with its own attachment can be open when the preview is
        // begun from the Ctrl+O view; confirming clobbers the draft, so its
        // pair must not linger as a ghost image on the rewound message.
        let mut app = App::new();
        exchange(&mut app, "hello", "hi");
        app.attach_image(PathBuf::from("/tmp/draft.png"));
        app.on_key(ctrl('o')); // the overlay opens over the non-empty draft
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None); // begin preview
        app.on_key(key(KeyCode::Enter)); // rewind to "hello"
        assert_eq!(app.input.text(), "hello");
        assert!(app.images.is_empty(), "no unanchored pairs survive");
        assert_eq!(
            app.take_discarded_images(),
            vec![PathBuf::from("/tmp/draft.png")],
            "the clobbered draft's temp file is queued for deletion"
        );
    }

    #[test]
    fn confirming_the_oldest_message_empties_the_history() {
        let mut app = App::new();
        exchange(&mut app, "first", "a");
        exchange(&mut app, "second", "b");
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Esc)); // step to "first"
        app.on_key(key(KeyCode::Enter));
        assert!(
            app.history.is_empty(),
            "everything from `first` on is dropped"
        );
        assert_eq!(app.input.text(), "first");
    }

    #[test]
    fn q_cancels_the_preview_without_truncating() {
        let mut app = App::new();
        exchange(&mut app, "first", "a");
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.on_key(key(KeyCode::Char('q'))), Action::ToggleToolView);
        assert_eq!(app.view, View::Conversation);
        assert_eq!(app.history.len(), 3, "nothing was dropped");
        assert!(app.input.is_empty(), "nothing was prefilled");
        assert_eq!(app.backtrack, Backtrack::default());
    }

    #[test]
    fn ctrl_o_cancels_the_preview_too() {
        let mut app = App::new();
        exchange(&mut app, "first", "a");
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
        assert_eq!(app.view, View::Conversation);
        assert_eq!(app.history.len(), 3);
        assert_eq!(app.backtrack, Backtrack::default());
    }

    #[test]
    fn esc_in_the_overlay_begins_the_preview_when_idle_with_a_target() {
        // Codex's Ctrl+T → Esc path: Esc inside an already-open transcript
        // view starts backtracking in place instead of closing the view.
        let mut app = App::new();
        exchange(&mut app, "hello", "hi");
        app.on_key(ctrl('o'));
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        assert_eq!(app.view, View::ToolOutput, "the overlay stays up");
        assert_eq!(app.backtrack.selected, Some(0));
    }

    #[test]
    fn esc_in_the_overlay_still_closes_it_with_no_target() {
        let mut app = App::new();
        app.on_key(ctrl('o'));
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::ToggleToolView);
        assert_eq!(app.view, View::Conversation);
    }

    #[test]
    fn esc_in_the_overlay_still_closes_it_mid_turn() {
        // While a turn streams the transcript can't be rewound mid-flight;
        // Esc keeps meaning "back to the chat" (interrupting stays a
        // conversation-view gesture).
        let mut app = App::new();
        exchange(&mut app, "hello", "hi");
        app.begin_stream();
        app.on_key(ctrl('o'));
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::ToggleToolView);
        assert_eq!(app.view, View::Conversation);
        assert_eq!(app.backtrack.selected, None);
    }

    #[test]
    fn scroll_keys_keep_scrolling_while_previewing() {
        let mut app = App::new();
        exchange(&mut app, "one", "a");
        exchange(&mut app, "two", "b");
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Esc));
        app.tool_scroll = 5;
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.tool_scroll, 4, "↑ still scrolls the transcript");
        assert_eq!(app.backtrack.selected, Some(1), "the highlight stays put");
    }

    #[test]
    fn shell_headers_are_not_backtrack_targets() {
        // A `!` command is user-typed but not a user *prompt* (codex only
        // targets user messages); with nothing else in history Esc still quits.
        let mut app = App::new();
        app.begin_shell("pwd");
        app.start_tool("shell", "pwd");
        app.end_tool("/home", true);
        app.end_turn(1);
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Quit);
    }

    #[test]
    fn the_preview_requests_a_scroll_to_the_highlight_once_per_step() {
        let mut app = App::new();
        exchange(&mut app, "one", "a");
        exchange(&mut app, "two", "b");
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Esc));
        assert!(app.take_backtrack_scroll(), "opening requests a scroll");
        assert!(!app.take_backtrack_scroll(), "…consumed by the draw");
        app.on_key(key(KeyCode::Esc)); // step older
        assert!(app.take_backtrack_scroll(), "stepping requests another");
    }

    #[test]
    fn applying_a_backtrack_scroll_disengages_tail_follow() {
        // Previewing the *last* message can leave the scroll at max, which
        // re-engages tail-follow (`settle_tool_scroll`); a step older must
        // not have its scroll-into-view yanked back to the bottom by it.
        let mut app = App::new();
        exchange(&mut app, "one", "a");
        exchange(&mut app, "two", "b");
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Esc));
        app.tool_follow = true; // the open landed at the bottom
        app.apply_backtrack_scroll(3);
        app.settle_tool_scroll(10);
        assert!(!app.tool_follow, "the follow pin is released");
        assert_eq!(app.tool_scroll, 3, "the highlight's scroll survives");
    }

    // ===== /resume session picker (docs/resume.md) =====

    fn summary(path: &str, preview: &str) -> crate::session::SessionSummary {
        crate::session::SessionSummary {
            path: PathBuf::from(path),
            updated_secs: 300,
            created_secs: 300,
            cwd: "/repo".into(),
            preview: preview.into(),
        }
    }

    /// An app with the picker open over one session per `(path, preview)`,
    /// all recorded in the picker's own cwd (`/repo`).
    fn picker_app(sessions: &[(&str, &str)]) -> App {
        let mut app = App::new();
        app.open_resume_picker(
            sessions
                .iter()
                .map(|(path, preview)| summary(path, preview))
                .collect(),
            "/repo".into(),
        );
        app
    }

    /// Type `text` into the composer key by key.
    fn type_chars(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn slash_resume_runs_to_open_the_picker_when_idle() {
        let mut app = App::new();
        type_chars(&mut app, "/resume");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenResumePicker);
        assert!(app.input.is_empty(), "running a command clears the draft");
    }

    #[test]
    fn slash_resume_mid_turn_is_rejected_with_a_toast() {
        // Codex blocks /resume while a task runs (it swaps the whole
        // conversation) — the picker never opens over an active turn, and the
        // rejection is a transient toast, not a scrollback bullet.
        let mut app = App::new();
        app.begin_stream();
        type_chars(&mut app, "/resume");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Toast(RESUME_BUSY_NOTICE.to_string()),
        );
        assert_eq!(app.view, View::Conversation);
        assert!(app.resume_picker.is_none());
    }

    #[test]
    fn open_resume_picker_enters_the_view_and_disarms_a_primed_backtrack() {
        let mut app = App::new();
        app.backtrack.primed = true;
        app.open_resume_picker(vec![summary("a.jsonl", "hello")], "/repo".into());
        assert_eq!(app.view, View::ResumePicker);
        assert!(!app.backtrack.primed, "any view swap disarms the gesture");
        let picker = app.resume_picker.as_ref().expect("picker state is open");
        assert_eq!(picker.selected, 0);
        assert!(picker.query.is_empty());
        // Codex's defaults: filter Cwd, sort Updated, focus on the Filter tab.
        assert_eq!(picker.filter, ResumeFilter::Cwd);
        assert_eq!(picker.sort, ResumeSort::Updated);
        assert_eq!(picker.focus, ResumeControl::Filter);
    }

    #[test]
    fn the_cwd_filter_hides_other_directories_until_toggled_to_all() {
        // Codex's Filter: [Cwd] All — the default mode lists only sessions
        // recorded in the picker's own cwd; → on the focused Filter control
        // switches to All (and back), reseating the selection.
        let mut app = picker_app(&[("a", "here one"), ("b", "here two")]);
        {
            let picker = app.resume_picker.as_mut().unwrap();
            picker.sessions.push(crate::session::SessionSummary {
                path: PathBuf::from("c"),
                updated_secs: 10,
                created_secs: 10,
                cwd: "/elsewhere".into(),
                preview: "other repo".into(),
            });
        }
        let picker = app.resume_picker.as_ref().unwrap();
        assert_eq!(picker.matches().len(), 2, "Cwd default hides /elsewhere");
        app.on_key(key(KeyCode::Down)); // move off the top
        app.on_key(key(KeyCode::Right)); // toggle the focused Filter control
        let picker = app.resume_picker.as_ref().unwrap();
        assert_eq!(picker.filter, ResumeFilter::All);
        assert_eq!(picker.matches().len(), 3, "All shows every session");
        assert_eq!(picker.selected, 0, "a mode change reseats the selection");
        app.on_key(key(KeyCode::Left)); // toggle back
        assert_eq!(
            app.resume_picker.as_ref().unwrap().filter,
            ResumeFilter::Cwd
        );
    }

    #[test]
    fn tab_moves_the_toolbar_focus_and_arrows_toggle_the_sort() {
        let mut app = picker_app(&[("a", "old"), ("b", "new")]);
        {
            // "a" was modified most recently but created earlier; "b" the
            // reverse — so the two sort keys order them differently.
            let picker = app.resume_picker.as_mut().unwrap();
            picker.sessions[0].updated_secs = 10; // a: touched just now
            picker.sessions[0].created_secs = 900; // …but started earlier
            picker.sessions[1].updated_secs = 500;
            picker.sessions[1].created_secs = 100; // b: the newer session
        }
        let updated_order: Vec<&str> = app
            .resume_picker
            .as_ref()
            .unwrap()
            .matches()
            .iter()
            .map(|s| s.preview.as_str())
            .collect();
        assert_eq!(updated_order, vec!["old", "new"], "Updated: mtime order");
        app.on_key(key(KeyCode::Tab)); // focus: Filter → Sort
        assert_eq!(
            app.resume_picker.as_ref().unwrap().focus,
            ResumeControl::Sort
        );
        app.on_key(key(KeyCode::Right)); // Sort: Updated → Created
        let picker = app.resume_picker.as_ref().unwrap();
        assert_eq!(picker.sort, ResumeSort::Created);
        let created_order: Vec<&str> = picker
            .matches()
            .iter()
            .map(|s| s.preview.as_str())
            .collect();
        assert_eq!(created_order, vec!["new", "old"], "Created: start order");
        assert_eq!(picker.filter, ResumeFilter::Cwd, "filter untouched");
        app.on_key(key(KeyCode::BackTab)); // two controls: prev == next
        assert_eq!(
            app.resume_picker.as_ref().unwrap().focus,
            ResumeControl::Filter
        );
    }

    #[test]
    fn picker_up_down_moves_clamp_at_both_ends() {
        let mut app = picker_app(&[("a", "one"), ("b", "two"), ("c", "three")]);
        for _ in 0..4 {
            app.on_key(key(KeyCode::Down));
        }
        assert_eq!(app.resume_picker.as_ref().unwrap().selected, 2);
        for _ in 0..5 {
            app.on_key(key(KeyCode::Up));
        }
        assert_eq!(app.resume_picker.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn picker_page_home_and_end_jump() {
        let sessions: Vec<(String, String)> = (0..25)
            .map(|i| (format!("s{i}"), format!("message {i}")))
            .collect();
        let refs: Vec<(&str, &str)> = sessions
            .iter()
            .map(|(p, v)| (p.as_str(), v.as_str()))
            .collect();
        let mut app = picker_app(&refs);
        app.on_key(key(KeyCode::PageDown));
        assert_eq!(app.resume_picker.as_ref().unwrap().selected, 10);
        app.on_key(key(KeyCode::End));
        assert_eq!(app.resume_picker.as_ref().unwrap().selected, 24);
        app.on_key(key(KeyCode::Home));
        assert_eq!(app.resume_picker.as_ref().unwrap().selected, 0);
        app.on_key(key(KeyCode::PageUp));
        assert_eq!(app.resume_picker.as_ref().unwrap().selected, 0, "clamped");
    }

    #[test]
    fn typing_filters_the_rows_and_reseats_the_selection() {
        let mut app = picker_app(&[("a", "wrap bug"), ("b", "shell fun"), ("c", "WRAP fix")]);
        app.on_key(key(KeyCode::Down)); // move off the top first
        type_chars(&mut app, "wrap");
        let picker = app.resume_picker.as_ref().unwrap();
        assert_eq!(picker.query, "wrap");
        assert_eq!(picker.selected, 0, "a query edit reseats the selection");
        // Case-insensitive substring over the preview (codex's matches_query).
        let matches = picker.matches();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].preview, "wrap bug");
        assert_eq!(matches[1].preview, "WRAP fix");
    }

    #[test]
    fn enter_resumes_the_selected_filtered_row() {
        let mut app = picker_app(&[("a", "wrap bug"), ("b", "shell fun"), ("c", "wrap fix")]);
        type_chars(&mut app, "wrap");
        app.on_key(key(KeyCode::Down)); // second match = "wrap fix" at path c
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::ResumeSession(PathBuf::from("c")),
        );
    }

    #[test]
    fn enter_on_an_empty_picker_does_nothing() {
        let mut app = picker_app(&[]);
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert_eq!(app.view, View::ResumePicker, "the picker stays up");
    }

    #[test]
    fn backspace_pops_the_query() {
        let mut app = picker_app(&[("a", "wrap bug")]);
        type_chars(&mut app, "wx");
        app.on_key(key(KeyCode::Backspace));
        let picker = app.resume_picker.as_ref().unwrap();
        assert_eq!(picker.query, "w");
        assert_eq!(picker.matches().len(), 1, "the widened query matches again");
    }

    #[test]
    fn esc_clears_the_query_first_and_closes_second() {
        let mut app = picker_app(&[("a", "hello")]);
        type_chars(&mut app, "zzz");
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        assert_eq!(app.view, View::ResumePicker, "first Esc only clears");
        assert!(app.resume_picker.as_ref().unwrap().query.is_empty());
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseResumePicker);
        assert_eq!(app.view, View::Conversation);
        assert!(app.resume_picker.is_none());
    }

    #[test]
    fn a_paste_joins_the_picker_search_query_flattened() {
        // Codex normalizes a pasted search query: whitespace runs collapse to
        // single spaces, a non-empty query gains a separating space, and a
        // whitespace-only paste is ignored.
        let mut app = picker_app(&[("a", "wrap bug"), ("b", "other")]);
        app.on_key(key(KeyCode::Down));
        app.paste_into_resume_search("wrap\n   bug");
        let picker = app.resume_picker.as_ref().unwrap();
        assert_eq!(picker.query, "wrap bug");
        assert_eq!(picker.selected, 0, "a query edit reseats the selection");
        app.paste_into_resume_search("  \n ");
        assert_eq!(app.resume_picker.as_ref().unwrap().query, "wrap bug");
        app.paste_into_resume_search("fix");
        assert_eq!(app.resume_picker.as_ref().unwrap().query, "wrap bug fix");
    }

    #[test]
    fn ctrl_c_closes_the_picker_not_the_app() {
        // Codex's from-a-session picker: Ctrl+C leaves the picker, never the
        // app (the startup picker's quit has no equivalent here).
        let mut app = picker_app(&[("a", "hello")]);
        assert_eq!(app.on_key(ctrl('c')), Action::CloseResumePicker);
        assert_eq!(app.view, View::Conversation);
    }

    #[test]
    fn ctrl_o_is_inert_while_the_picker_is_up() {
        // Both overlays share the alternate screen — the transcript view must
        // not open on top of the picker.
        let mut app = picker_app(&[("a", "hello")]);
        assert_eq!(app.on_key(ctrl('o')), Action::None);
        assert_eq!(app.view, View::ResumePicker);
    }

    #[test]
    fn load_session_installs_history_and_returns_to_the_conversation() {
        let mut app = picker_app(&[("a", "hello")]);
        // Belt-and-braces: leftovers from a dead turn must not survive the
        // swap (a turn can't be *active* — /resume is rejected mid-task).
        app.begin_stream();
        app.push_chunk("partial");
        let items = vec![
            HistoryItem::Message(Message {
                role: Role::User,
                text: "hello".into(),
                timestamp: String::new(),
                images: Vec::new(),
            }),
            HistoryItem::Message(Message {
                role: Role::Assistant,
                text: "hi".into(),
                timestamp: String::new(),
                images: Vec::new(),
            }),
        ];
        app.load_session(items.clone());
        assert_eq!(app.history, items);
        assert_eq!(app.view, View::Conversation);
        assert!(app.resume_picker.is_none());
        assert!(!app.is_streaming(), "no stream survives the swap");
        assert!(app.status().is_none(), "no status survives the swap");
    }

    // ===== /model picker (docs/llm.md) =====

    fn model(id: &str, provider: &str, name: &str) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            provider: provider.into(),
            display_name: name.into(),
            reasoning: None,
            vision: None,
            context: None,
        }
    }

    /// An app with the model picker open (loaded) over the given models, the
    /// first marked active.
    fn model_app(models: &[ModelEntry]) -> App {
        let mut app = App::new();
        let active = models.first().map(|m| m.id.clone()).unwrap_or_default();
        app.open_model_picker(active);
        app.set_models(models.to_vec());
        app
    }

    fn sample_models() -> Vec<ModelEntry> {
        vec![
            model(
                "anthropic/claude-3.5-haiku",
                "openrouter",
                "Anthropic: Claude 3.5 Haiku",
            ),
            model(
                "anthropic/claude-fable-5",
                "openrouter",
                "Anthropic: Claude Fable 5",
            ),
            model(
                "moonshotai/kimi-k2.6",
                "openrouter",
                "MoonshotAI: Kimi K2.6",
            ),
        ]
    }

    // ===== Shift+Tab thinking mode (docs/reasoning.md) =====

    use crate::llm::ReasoningEffort;

    /// The common support shape: the low/medium/high ladder, disableable.
    fn trio_support() -> ReasoningSupport {
        ReasoningSupport {
            efforts: vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ],
            can_disable: true,
            default_effort: None,
        }
    }

    fn backtab() -> KeyEvent {
        key(KeyCode::BackTab)
    }

    #[test]
    fn set_thinking_seeds_the_state() {
        let mut app = App::new();
        assert!(app.thinking.is_none(), "unknown/unsupported by default");
        app.set_thinking(Some((
            trio_support(),
            ThinkingMode::Effort(ReasoningEffort::Medium),
        )));
        let state = app.thinking.as_ref().expect("seeded");
        assert_eq!(state.mode, ThinkingMode::Effort(ReasoningEffort::Medium));
        assert_eq!(state.support, trio_support());
        app.set_thinking(None);
        assert!(
            app.thinking.is_none(),
            "a switch to a non-reasoner clears it"
        );
    }

    #[test]
    fn backtab_cycles_the_thinking_mode() {
        let mut app = App::new();
        app.set_thinking(Some((
            trio_support(),
            ThinkingMode::Effort(ReasoningEffort::Medium),
        )));
        assert_eq!(
            app.on_key(backtab()),
            Action::SetThinking(ThinkingMode::Effort(ReasoningEffort::High)),
            "medium steps to high"
        );
        assert_eq!(
            app.thinking.as_ref().unwrap().mode,
            ThinkingMode::Effort(ReasoningEffort::High),
            "the state advanced too"
        );
        assert_eq!(
            app.on_key(backtab()),
            Action::SetThinking(ThinkingMode::Off),
            "high wraps to off"
        );
        assert_eq!(
            app.on_key(backtab()),
            Action::SetThinking(ThinkingMode::Effort(ReasoningEffort::Low)),
            "off steps to low"
        );
    }

    #[test]
    fn shift_tab_reported_as_tab_plus_shift_also_cycles() {
        // Terminals differ: legacy sends BackTab (ESC[Z), the kitty protocol
        // can report Tab+SHIFT — both must cycle (the shift-enter pattern).
        let mut app = App::new();
        app.set_thinking(Some((
            trio_support(),
            ThinkingMode::Effort(ReasoningEffort::Low),
        )));
        let shift_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        assert_eq!(
            app.on_key(shift_tab),
            Action::SetThinking(ThinkingMode::Effort(ReasoningEffort::Medium))
        );
    }

    #[test]
    fn backtab_keeps_the_draft_intact() {
        let mut app = App::new();
        app.set_thinking(Some((trio_support(), ThinkingMode::Off)));
        type_chars(&mut app, "keep me");
        app.on_key(backtab());
        assert_eq!(app.input.text(), "keep me");
    }

    #[test]
    fn backtab_without_support_raises_an_info_toast() {
        // A model with no reasoning (or the dummy backend): Shift+Tab explains
        // instead of dying silently. The *loop* presents the toast (arming its
        // expiry), so this is an Action, not a direct show_toast.
        let mut app = App::new();
        app.set_session_info("dummy_model_name", "~/repo");
        assert_eq!(
            app.on_key(backtab()),
            Action::Toast("dummy_model_name does not support thinking".into())
        );
        assert!(app.thinking.is_none());
    }

    #[test]
    fn backtab_mid_turn_cycles_for_the_next_turn() {
        // Like /model, the cycle never touches the running turn — the new mode
        // simply rides the next request.
        let mut app = App::new();
        app.set_thinking(Some((
            trio_support(),
            ThinkingMode::Effort(ReasoningEffort::Medium),
        )));
        app.begin_stream();
        assert_eq!(
            app.on_key(backtab()),
            Action::SetThinking(ThinkingMode::Effort(ReasoningEffort::High))
        );
        assert!(app.turn_active(), "the turn keeps running underneath");
    }

    #[test]
    fn backtab_with_the_model_picker_open_is_inert() {
        // The picker owns every key while open — BackTab must not cycle the
        // mode out from under it.
        let mut app = model_app(&sample_models());
        app.set_thinking(Some((
            trio_support(),
            ThinkingMode::Effort(ReasoningEffort::Medium),
        )));
        assert_eq!(app.on_key(backtab()), Action::None);
        assert_eq!(
            app.thinking.as_ref().unwrap().mode,
            ThinkingMode::Effort(ReasoningEffort::Medium),
            "unchanged"
        );
    }

    #[test]
    fn selecting_a_model_carries_its_reasoning_support() {
        // Enter on a picker row hands the loop the entry's parsed support, so
        // a successful switch can seed the cycle without refetching /models.
        let mut with_support = model("thinker", "openrouter", "Thinker");
        with_support.reasoning = Some(trio_support());
        let mut app = model_app(&[
            model("anthropic/claude-3.5-haiku", "openrouter", "Haiku"),
            with_support,
        ]);
        type_chars(&mut app, "thinker");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::SelectModel {
                provider: "openrouter".into(),
                id: "thinker".into(),
                reasoning: Some(trio_support()),
                vision: None,
                context: None,
            }
        );
    }

    #[test]
    fn selecting_a_model_carries_its_vision_support() {
        // Enter also hands the loop the entry's image-input support, so the
        // rebuilt backend gates attachments without refetching /models
        // (docs/tools.md).
        let mut blind = model("openai/gpt-oss-120b", "openrouter", "GPT OSS");
        blind.vision = Some(false);
        let mut app = model_app(&[blind]);
        let action = app.on_key(key(KeyCode::Enter));
        assert!(
            matches!(
                action,
                Action::SelectModel {
                    vision: Some(false),
                    ..
                }
            ),
            "the entry's vision rides the action: {action:?}"
        );
    }

    #[test]
    fn slash_model_runs_to_open_the_picker_when_idle() {
        let mut app = App::new();
        type_chars(&mut app, "/model");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenModelPicker);
        assert!(app.input.is_empty(), "running a command clears the draft");
    }

    #[test]
    fn slash_model_opens_the_picker_mid_turn() {
        // /model only swaps the composer, never the running turn, so it opens
        // regardless of turn state (docs/toast.md). The *loop* does the fetch.
        let mut app = App::new();
        app.begin_stream();
        type_chars(&mut app, "/model");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenModelPicker);
        assert!(app.turn_active(), "the turn keeps running underneath");
    }

    #[test]
    fn open_model_picker_starts_loading_and_stays_in_the_conversation() {
        let mut app = App::new();
        app.open_model_picker("m1");
        let picker = app.model_picker.as_ref().expect("picker open");
        assert_eq!(picker.status, ModelLoad::Loading);
        assert_eq!(picker.active_id, "m1");
        assert_eq!(
            app.view,
            View::Conversation,
            "the picker is inline, not an overlay"
        );
    }

    #[test]
    fn opening_the_picker_abandons_the_palette_and_shortcuts() {
        let mut app = App::new();
        app.shortcuts_open = true;
        app.open_model_picker("m1");
        assert!(!app.shortcuts_open);
        assert!(app.command_menu.is_none());
    }

    #[test]
    fn set_models_marks_ready_and_seats_on_the_active_model() {
        let mut app = App::new();
        app.open_model_picker("anthropic/claude-fable-5");
        app.set_models(sample_models());
        let picker = app.model_picker.as_ref().unwrap();
        assert_eq!(picker.status, ModelLoad::Ready);
        // claude-fable-5 is index 1 in the alphabetical list.
        assert_eq!(picker.selected, 1);
        assert_eq!(picker.highlighted().unwrap().id, "anthropic/claude-fable-5");
    }

    #[test]
    fn set_models_seats_on_top_when_active_is_absent() {
        let mut app = App::new();
        app.open_model_picker("not/present");
        app.set_models(sample_models());
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn set_models_error_shows_the_message() {
        let mut app = App::new();
        app.open_model_picker("m");
        app.set_models_error("boom");
        assert_eq!(
            app.model_picker.as_ref().unwrap().status,
            ModelLoad::Error("boom".into())
        );
    }

    #[test]
    fn set_models_is_a_noop_when_the_picker_is_closed() {
        let mut app = App::new();
        app.set_models(sample_models()); // no panic, no state
        assert!(app.model_picker.is_none());
    }

    #[test]
    fn set_models_needs_login_clears_the_list_and_points_at_login() {
        let mut app = App::new();
        app.open_model_picker("m");
        app.set_models(sample_models()); // pretend a stale list was there
        app.set_models_needs_login();
        let picker = app.model_picker.as_ref().unwrap();
        assert_eq!(picker.status, ModelLoad::NeedsLogin);
        assert!(picker.models.is_empty(), "no models offered without a key");
        assert!(picker.matches().is_empty());
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn enter_does_nothing_when_no_provider_is_configured() {
        let mut app = App::new();
        app.open_model_picker("m");
        app.set_models_needs_login();
        // Enter has nothing to select — it must not emit a SelectModel action.
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert!(app.model_picker.is_some(), "picker stays open");
    }

    // --- Multi-provider parallel `/model` load (docs/llm.md). ---

    #[test]
    fn add_models_shows_the_first_provider_and_merges_the_rest() {
        let mut app = App::new();
        app.open_model_picker("x");
        app.begin_model_load(2);
        assert_eq!(
            app.model_picker.as_ref().unwrap().status,
            ModelLoad::Loading
        );
        // OpenRouter lands first — its list shows immediately, one fetch still out.
        app.add_models(vec![
            model("c/z", "openrouter", "C Z"),
            model("a/x", "openrouter", "A X"),
        ]);
        let picker = app.model_picker.as_ref().unwrap();
        assert_eq!(
            picker.status,
            ModelLoad::Ready,
            "shown before the rest arrive"
        );
        assert_eq!(picker.pending, 1);
        assert_eq!(picker.models.len(), 2);
        // Agent Zero lands second — merged into one sorted list, no fetches left.
        app.add_models(vec![model("b/y", "a0_venice", "B Y")]);
        let picker = app.model_picker.as_ref().unwrap();
        assert_eq!(picker.pending, 0);
        let ids: Vec<&str> = picker.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            ["a/x", "b/y", "c/z"],
            "merged + sorted across providers"
        );
    }

    #[test]
    fn add_model_error_keeps_a_partial_list_and_notes_the_failure() {
        let mut app = App::new();
        app.open_model_picker("x");
        app.begin_model_load(2);
        app.add_models(vec![model("a/x", "openrouter", "A X")]);
        app.add_model_error("Agent Zero API", "HTTP 401: unauthorized");
        let picker = app.model_picker.as_ref().unwrap();
        // The good provider's list stays; the failure is recorded, not fatal.
        assert_eq!(picker.status, ModelLoad::Ready);
        assert_eq!(picker.pending, 0);
        assert_eq!(picker.models.len(), 1);
        assert_eq!(picker.errors.len(), 1);
        assert_eq!(picker.errors[0].provider, "Agent Zero API");
    }

    #[test]
    fn all_providers_failing_settles_into_an_error() {
        let mut app = App::new();
        app.open_model_picker("x");
        app.begin_model_load(2);
        app.add_model_error("OpenRouter", "HTTP 500");
        // Still loading — one fetch outstanding, nothing to show yet.
        assert_eq!(
            app.model_picker.as_ref().unwrap().status,
            ModelLoad::Loading
        );
        app.add_model_error("Agent Zero API", "HTTP 401");
        let picker = app.model_picker.as_ref().unwrap();
        assert!(matches!(picker.status, ModelLoad::Error(_)));
        assert!(picker.models.is_empty());
        assert_eq!(picker.errors.len(), 2);
    }

    #[test]
    fn add_models_reseats_on_the_active_model_once_its_provider_lands() {
        let mut app = App::new();
        app.open_model_picker("b/y"); // active lives in the second provider
        app.begin_model_load(2);
        app.add_models(vec![model("a/x", "openrouter", "A X")]);
        // Active isn't here yet — seated on top.
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
        app.add_models(vec![model("b/y", "a0_venice", "B Y")]);
        // Its provider landed and the user hadn't moved — jump to the active row.
        let picker = app.model_picker.as_ref().unwrap();
        assert_eq!(picker.highlighted().unwrap().id, "b/y");
    }

    #[test]
    fn active_mark_matches_the_provider_not_just_the_id() {
        // The merged list can carry the same id under two providers; only the
        // row from the active provider is the ✓ row.
        let mut app = App::new();
        app.open_model_picker("shared/id");
        app.set_active_provider("a0_venice");
        app.begin_model_load(2);
        app.add_models(vec![model("shared/id", "openrouter", "OR")]);
        app.add_models(vec![model("shared/id", "a0_venice", "A0")]);
        let picker = app.model_picker.as_ref().unwrap();
        let or = picker
            .models
            .iter()
            .find(|m| m.provider == "openrouter")
            .unwrap();
        let a0 = picker
            .models
            .iter()
            .find(|m| m.provider == "a0_venice")
            .unwrap();
        assert!(
            !picker.is_active(or),
            "same id, other provider is not active"
        );
        assert!(picker.is_active(a0), "the active provider's row is marked");
    }

    #[test]
    fn add_models_preserves_the_highlight_after_the_user_navigates() {
        let mut app = App::new();
        app.open_model_picker("x");
        app.begin_model_load(2);
        app.add_models(vec![
            model("a/x", "openrouter", "A X"),
            model("c/z", "openrouter", "C Z"),
        ]);
        app.on_key(key(KeyCode::Down)); // highlight c/z (index 1)
        assert_eq!(
            app.model_picker.as_ref().unwrap().highlighted().unwrap().id,
            "c/z"
        );
        // A merge pushes a row in between — the highlight rides c/z, not the index.
        app.add_models(vec![model("b/y", "a0_venice", "B Y")]);
        assert_eq!(
            app.model_picker.as_ref().unwrap().highlighted().unwrap().id,
            "c/z"
        );
    }

    #[test]
    fn typing_filters_the_models_case_insensitively() {
        let mut app = model_app(&sample_models());
        type_chars(&mut app, "KIMI");
        let picker = app.model_picker.as_ref().unwrap();
        assert_eq!(picker.matches().len(), 1);
        assert_eq!(picker.matches()[0].id, "moonshotai/kimi-k2.6");
    }

    #[test]
    fn filtering_matches_provider_and_display_name_too() {
        let mut app = model_app(&sample_models());
        type_chars(&mut app, "openrouter");
        assert_eq!(app.model_picker.as_ref().unwrap().matches().len(), 3);
        app.model_picker.as_mut().unwrap().query.clear();
        type_chars(&mut app, "MoonshotAI");
        assert_eq!(app.model_picker.as_ref().unwrap().matches().len(), 1);
    }

    #[test]
    fn arrows_move_the_selection_clamped() {
        let mut app = model_app(&sample_models());
        app.model_picker.as_mut().unwrap().selected = 0;
        app.on_key(key(KeyCode::Up)); // clamps at top
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 1);
        app.on_key(key(KeyCode::End));
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 2);
        app.on_key(key(KeyCode::Down)); // clamps at bottom
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 2);
        app.on_key(key(KeyCode::Home));
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn enter_selects_the_highlighted_model_and_closes() {
        let mut app = model_app(&sample_models());
        app.model_picker.as_mut().unwrap().selected = 2;
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(
            action,
            Action::SelectModel {
                provider: "openrouter".into(),
                id: "moonshotai/kimi-k2.6".into(),
                reasoning: None,
                vision: None,
                context: None,
            }
        );
        assert!(app.model_picker.is_none(), "selecting closes the picker");
    }

    #[test]
    fn enter_on_an_empty_list_keeps_the_picker_open() {
        let mut app = App::new();
        app.open_model_picker("m");
        app.set_models(vec![]);
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert!(app.model_picker.is_some());
    }

    #[test]
    fn esc_clears_the_query_first_then_closes() {
        let mut app = model_app(&sample_models());
        type_chars(&mut app, "kimi");
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        assert!(app.model_picker.as_ref().unwrap().query.is_empty());
        assert!(
            app.model_picker.is_some(),
            "first Esc only clears the query"
        );
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseModelPicker);
        assert!(app.model_picker.is_none());
    }

    #[test]
    fn ctrl_c_closes_the_model_picker_not_the_app() {
        let mut app = model_app(&sample_models());
        assert_eq!(app.on_key(ctrl('c')), Action::CloseModelPicker);
        assert!(app.model_picker.is_none());
    }

    #[test]
    fn backspace_pops_the_model_query() {
        let mut app = model_app(&sample_models());
        type_chars(&mut app, "kim");
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.model_picker.as_ref().unwrap().query, "ki");
    }

    #[test]
    fn typing_reseats_the_selection_to_the_top() {
        let mut app = model_app(&sample_models());
        app.model_picker.as_mut().unwrap().selected = 2;
        type_chars(&mut app, "anthropic");
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
    }

    // --- The `/login` API-key onboarding flow (docs/llm.md). ---

    fn sample_choices() -> Vec<ProviderChoice> {
        vec![
            ProviderChoice {
                id: "a0_venice".into(),
                name: "Agent Zero API".into(),
                env_var: "A0_VENICE_API_KEY".into(),
                configured: false,
            },
            ProviderChoice {
                id: "openrouter".into(),
                name: "OpenRouter".into(),
                env_var: "OPENROUTER_API_KEY".into(),
                configured: true,
            },
            ProviderChoice {
                id: "together".into(),
                name: "Together AI".into(),
                env_var: "TOGETHER_API_KEY".into(),
                configured: false,
            },
        ]
    }

    fn login_app() -> App {
        let mut app = App::new();
        app.open_key_onboarding(sample_choices(), "~/.alter-zero/.env");
        app
    }

    /// Drive the flow to the key-entry step for the given provider id.
    fn key_app(provider_id: &str) -> App {
        let mut app = login_app();
        let idx = sample_choices()
            .iter()
            .position(|p| p.id == provider_id)
            .unwrap();
        {
            let onboarding = app.key_onboarding.as_mut().unwrap();
            onboarding.selected = onboarding
                .matches()
                .iter()
                .position(|p| p.id == provider_id)
                .unwrap();
        }
        app.on_key(key(KeyCode::Enter));
        let onboarding = app.key_onboarding.as_ref().unwrap();
        assert_eq!(onboarding.step, KeyStep::Key);
        assert_eq!(onboarding.chosen, Some(idx));
        app
    }

    #[test]
    fn slash_login_opens_the_onboarding_when_idle() {
        let mut app = App::new();
        type_chars(&mut app, "/login");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenKeyOnboarding);
        assert!(app.input.is_empty(), "running a command clears the draft");
    }

    #[test]
    fn slash_login_opens_the_onboarding_mid_turn() {
        // /login works mid-turn like /model — saving a key never touches the
        // running turn (docs/toast.md). The *loop* builds the provider choices.
        let mut app = App::new();
        app.begin_stream();
        type_chars(&mut app, "/login");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenKeyOnboarding);
        assert!(app.turn_active(), "the turn keeps running underneath");
    }

    #[test]
    fn open_key_onboarding_starts_on_the_provider_step() {
        let app = login_app();
        let onboarding = app.key_onboarding.as_ref().expect("open");
        assert_eq!(onboarding.step, KeyStep::Provider);
        assert_eq!(onboarding.providers.len(), 3);
        assert_eq!(app.view, View::Conversation, "inline, not an overlay");
    }

    #[test]
    fn opening_onboarding_abandons_other_bands_and_the_model_picker() {
        let mut app = App::new();
        app.shortcuts_open = true;
        app.open_model_picker("m");
        app.open_key_onboarding(sample_choices(), "~/.alter-zero/.env");
        assert!(!app.shortcuts_open);
        assert!(app.command_menu.is_none());
        assert!(app.model_picker.is_none(), "the model picker is dismissed");
    }

    #[test]
    fn typing_filters_providers_case_insensitively() {
        let mut app = login_app();
        type_chars(&mut app, "OPEN");
        let onboarding = app.key_onboarding.as_ref().unwrap();
        assert_eq!(onboarding.matches().len(), 1);
        assert_eq!(onboarding.matches()[0].id, "openrouter");
    }

    #[test]
    fn enter_advances_to_the_key_step_for_the_highlighted_provider() {
        let mut app = login_app();
        app.key_onboarding.as_mut().unwrap().selected = 1; // openrouter
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        let onboarding = app.key_onboarding.as_ref().unwrap();
        assert_eq!(onboarding.step, KeyStep::Key);
        assert_eq!(onboarding.chosen_provider().unwrap().id, "openrouter");
    }

    #[test]
    fn enter_pins_the_index_from_the_filtered_matches() {
        let mut app = login_app();
        // Filter to a single match whose *unfiltered* index is 2 (together).
        type_chars(&mut app, "toget");
        assert_eq!(app.key_onboarding.as_ref().unwrap().matches().len(), 1);
        app.on_key(key(KeyCode::Enter));
        let onboarding = app.key_onboarding.as_ref().unwrap();
        assert_eq!(onboarding.chosen, Some(2));
        assert_eq!(onboarding.chosen_provider().unwrap().id, "together");
    }

    #[test]
    fn arrows_move_the_provider_selection_clamped() {
        let mut app = login_app();
        app.on_key(key(KeyCode::Up)); // clamp at top
        assert_eq!(app.key_onboarding.as_ref().unwrap().selected, 0);
        app.on_key(key(KeyCode::End));
        assert_eq!(app.key_onboarding.as_ref().unwrap().selected, 2);
        app.on_key(key(KeyCode::Down)); // clamp at bottom
        assert_eq!(app.key_onboarding.as_ref().unwrap().selected, 2);
        app.on_key(key(KeyCode::Home));
        assert_eq!(app.key_onboarding.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn key_step_types_and_backspaces_the_key() {
        let mut app = key_app("openrouter");
        type_chars(&mut app, "sk-abc");
        assert_eq!(app.key_onboarding.as_ref().unwrap().key_input, "sk-abc");
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.key_onboarding.as_ref().unwrap().key_input, "sk-ab");
    }

    #[test]
    fn enter_on_the_key_step_saves_and_closes() {
        let mut app = key_app("openrouter");
        type_chars(&mut app, "sk-secret");
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(
            action,
            Action::SaveApiKey {
                provider: "openrouter".into(),
                env_var: "OPENROUTER_API_KEY".into(),
                key: "sk-secret".into(),
            }
        );
        assert!(app.key_onboarding.is_none(), "saving closes the flow");
    }

    #[test]
    fn empty_key_enter_is_a_noop() {
        let mut app = key_app("openrouter");
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert!(app.key_onboarding.is_some(), "still waiting for a key");
    }

    #[test]
    fn esc_on_the_key_step_steps_back_to_the_provider_list() {
        let mut app = key_app("openrouter");
        type_chars(&mut app, "half-typed");
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        let onboarding = app.key_onboarding.as_ref().unwrap();
        assert_eq!(onboarding.step, KeyStep::Provider, "back to the list");
        assert!(onboarding.key_input.is_empty(), "the draft key is dropped");
        assert!(onboarding.chosen.is_none());
    }

    #[test]
    fn esc_on_the_provider_step_clears_the_query_then_closes() {
        let mut app = login_app();
        type_chars(&mut app, "open");
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
        assert!(app.key_onboarding.as_ref().unwrap().query.is_empty());
        assert!(app.key_onboarding.is_some(), "first Esc only clears");
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseKeyOnboarding);
        assert!(app.key_onboarding.is_none());
    }

    #[test]
    fn ctrl_c_closes_the_onboarding_not_the_app() {
        let mut app = key_app("openrouter"); // even mid-key-entry
        assert_eq!(app.on_key(ctrl('c')), Action::CloseKeyOnboarding);
        assert!(app.key_onboarding.is_none());
    }

    #[test]
    fn paste_into_the_key_step_strips_whitespace_and_newlines() {
        let mut app = key_app("openrouter");
        app.paste_into_key_onboarding("sk-abc\n  def\t");
        assert_eq!(app.key_onboarding.as_ref().unwrap().key_input, "sk-abcdef");
    }

    #[test]
    fn paste_into_the_provider_step_extends_the_filter() {
        let mut app = login_app();
        app.paste_into_key_onboarding("open router");
        assert_eq!(app.key_onboarding.as_ref().unwrap().query, "open router");
    }

    // ===== audited-defect regressions (2026-07 review) =====

    #[test]
    fn history_browsing_steps_past_a_recalled_shell_entry() {
        // Recalling "!ls" absorbs the bang into shell_mode (composer "ls"),
        // but the recorded entry is "!ls" — the unedited-recall comparison
        // must account for the absorbed bang or browsing strands on the
        // shell entry (docs/input-history.md: unedited recalls keep browsing).
        let mut app = App::new();
        submit(&mut app, "hello");
        type_query(&mut app, "!ls");
        app.on_key(key(KeyCode::Enter)); // records "!ls"
        app.on_key(key(KeyCode::Up)); // recall "!ls" → shell mode, text "ls"
        assert!(app.shell_mode);
        assert_eq!(app.input.text(), "ls");
        app.on_key(key(KeyCode::Up)); // must step OLDER, not move the cursor
        assert_eq!(app.input.text(), "hello");
        assert!(!app.shell_mode, "the older plain entry leaves the mode");
        app.on_key(key(KeyCode::Down)); // and ↓ steps back to the shell entry
        assert!(app.shell_mode);
        assert_eq!(app.input.text(), "ls");
    }

    #[test]
    fn down_past_a_recalled_shell_entry_clears_the_composer() {
        let mut app = App::new();
        type_query(&mut app, "!ls");
        app.on_key(key(KeyCode::Enter));
        app.on_key(key(KeyCode::Up)); // recall the newest ("!ls")
        assert!(app.shell_mode);
        app.on_key(key(KeyCode::Down)); // ↓ past the newest clears + exits
        assert_eq!(app.input.text(), "");
        assert!(!app.shell_mode);
    }

    #[test]
    fn deleting_one_duplicate_named_placeholder_keeps_the_other_pair() {
        // An undone batch can hold two "[Image #1]"s with different paths
        // (numbering restarts per draft). Backspacing over ONE occurrence
        // must drop only its own pair — not every pair sharing the name,
        // which deleted the temp file backing the occurrence still in the
        // composer (docs/image-paste.md).
        let mut app = App::new();
        app.record_user_message_with_images("[Image #1] first", vec![PathBuf::from("/a.png")]);
        app.record_user_message_with_images("[Image #1] second", vec![PathBuf::from("/b.png")]);
        app.begin_stream();
        assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
        // Seat the cursor right after the SECOND [Image #1] and Backspace it.
        let text = app.input.text().to_string();
        let second_end = text.rfind("[Image #1]").unwrap() + "[Image #1]".len();
        app.input.set_text_with_cursor(&text, second_end);
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.input.text(), "[Image #1] first\n second");
        assert_eq!(
            app.images,
            vec![("[Image #1]".to_string(), PathBuf::from("/a.png"))],
            "the first occurrence's pair survives"
        );
        assert_eq!(
            app.take_discarded_images(),
            vec![PathBuf::from("/b.png")],
            "only the deleted occurrence's temp file is discarded"
        );
    }

    #[test]
    fn accepting_a_search_match_discards_the_replaced_drafts_attachments() {
        // Enter on a Ctrl+R match replaces the pre-search draft; the pairs
        // that backed it are unanchored and must be discarded — otherwise the
        // stale attachment silently rides the next submission
        // (paste::distribute_images parks unclaimed pairs on the first text).
        let mut app = App::new();
        submit(&mut app, "old entry");
        app.attach_image(PathBuf::from("/tmp/img.png"));
        app.on_key(ctrl('r'));
        type_query(&mut app, "old");
        app.on_key(key(KeyCode::Enter)); // accept the previewed match
        assert_eq!(app.input.text(), "old entry");
        assert!(app.images.is_empty(), "no unanchored pairs survive");
        assert_eq!(
            app.take_discarded_images(),
            vec![PathBuf::from("/tmp/img.png")]
        );
    }

    #[test]
    fn cancelling_a_search_discards_an_image_attached_while_it_was_open() {
        // A Ctrl+V decode finishing while the search owns the composer lands
        // its placeholder in the preview text; the snapshot restore drops the
        // text, so the pair must go too — not linger invisibly and ride the
        // next submission.
        let mut app = App::new();
        submit(&mut app, "old entry");
        app.on_key(ctrl('r'));
        app.attach_image(PathBuf::from("/tmp/late.png"));
        app.on_key(key(KeyCode::Esc)); // cancel → snapshot (empty draft) restored
        assert_eq!(app.input.text(), "");
        assert!(app.images.is_empty());
        assert_eq!(
            app.take_discarded_images(),
            vec![PathBuf::from("/tmp/late.png")]
        );
    }

    #[test]
    fn cancelling_a_search_keeps_the_snapshot_drafts_attachments() {
        let mut app = App::new();
        submit(&mut app, "old entry");
        app.attach_image(PathBuf::from("/tmp/keep.png"));
        app.on_key(ctrl('r'));
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.input.text(), "[Image #1]");
        assert_eq!(
            app.images,
            vec![("[Image #1]".to_string(), PathBuf::from("/tmp/keep.png"))],
            "the restored draft's own pair stays backed"
        );
        assert!(app.take_discarded_images().is_empty());
    }

    #[test]
    fn interrupt_undo_stops_at_a_resumed_sessions_trailing_user_message() {
        // A rollout can end with a user message (quit mid-turn before any
        // reply); resuming installs it as the history tail. A NEW submission
        // undone by an early Esc must pull back only its own message — not
        // merge the resumed conversation's tail into the composer and drop it
        // from history.
        let mut app = App::new();
        app.record_user_message("old");
        let items = std::mem::take(&mut app.history);
        app.load_session(items);
        app.record_user_message("new");
        app.begin_stream();
        assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
        assert_eq!(app.input.text(), "new");
        assert_eq!(roles(&app), vec![Role::User], "the resumed tail survives");
        assert_eq!(message_at(&app, 0).text, "old");
    }

    #[test]
    fn interrupt_undo_stops_at_a_backtrack_rewind_boundary() {
        // A batch records two user messages; backtracking to the second
        // leaves the first as the history tail. A new submission undone by an
        // early Esc must not pull that older message back with it.
        let mut app = App::new();
        app.record_user_message("first");
        app.record_user_message("second");
        app.on_key(key(KeyCode::Esc)); // arm
        app.on_key(key(KeyCode::Esc)); // preview at the newest ("second")
        app.on_key(key(KeyCode::Enter)); // rewind: history = [first]
        assert_eq!(app.input.text(), "second");
        app.record_user_message("new");
        app.begin_stream();
        assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
        assert_eq!(app.input.text(), "new");
        assert_eq!(roles(&app), vec![Role::User]);
        assert_eq!(message_at(&app, 0).text, "first");
    }

    #[test]
    fn history_generation_bumps_on_every_non_append_mutation() {
        // The Ctrl+O transcript cache freezes rendered history items and only
        // appends — sound *only if* every non-append history mutation is
        // observable. Appends keep the generation; anything that clears,
        // replaces, truncates, or pops history must bump it (a pop + re-push
        // can land on the same length, so lengths alone can't be trusted).
        let mut app = App::new();
        let start = app.history_generation();
        app.record_user_message("one");
        app.record_user_message("two");
        assert_eq!(app.history_generation(), start, "appends never bump");

        // The Esc-Esc backtrack rewind truncates history.
        app.on_key(key(KeyCode::Esc)); // arm
        app.on_key(key(KeyCode::Esc)); // preview at the newest ("two")
        app.on_key(key(KeyCode::Enter)); // rewind: history = [one]
        let after_rewind = app.history_generation();
        assert_ne!(after_rewind, start, "a backtrack truncation bumps");

        // The interrupt-undo pops the just-submitted user message.
        app.record_user_message("new");
        app.begin_stream();
        assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
        let after_undo = app.history_generation();
        assert_ne!(after_undo, after_rewind, "an interrupt-undo pop bumps");

        // /resume replaces the whole conversation (load_session clears first).
        app.load_session(vec![HistoryItem::Message(Message {
            role: Role::User,
            text: "loaded".to_string(),
            timestamp: String::new(),
            images: Vec::new(),
        })]);
        let after_load = app.history_generation();
        assert_ne!(after_load, after_undo, "a session load bumps");

        // /clear wipes it.
        app.clear_conversation();
        assert_ne!(app.history_generation(), after_load, "/clear bumps");
    }

    // --- background shells (docs/background.md) ---

    /// A registered running shell for the manager tests.
    fn app_with_shells(commands: &[&str]) -> App {
        let mut app = App::new();
        for (i, cmd) in commands.iter().enumerate() {
            app.bg_started(&format!("bash_{}", i + 1), cmd, None, true);
        }
        app
    }

    #[test]
    fn bg_started_lists_the_shell_and_sets_the_ever_gate() {
        let mut app = App::new();
        assert!(!app.background_ever());
        app.bg_started("bash_1", "ping x.com", Some("Ping x".into()), true);
        assert!(app.background_ever());
        assert_eq!(app.background().len(), 1);
        let shell = &app.background()[0];
        assert_eq!(shell.id, "bash_1");
        assert_eq!(shell.command, "ping x.com");
        assert_eq!(shell.description.as_deref(), Some("Ping x"));
        assert!(shell.from_model);
    }

    #[test]
    fn bg_output_appends_and_caps_the_tail_on_line_boundaries() {
        let mut app = app_with_shells(&["cmd"]);
        app.bg_output("bash_1", "hello\n");
        app.bg_output("bash_1", "world\n");
        assert_eq!(app.background()[0].output, "hello\nworld\n");
        // Overflow the cap: the retained tail starts on a line boundary.
        let long = "x".repeat(1024);
        for _ in 0..32 {
            app.bg_output("bash_1", &format!("{long}\n"));
        }
        let tail = &app.background()[0].output;
        assert!(tail.len() <= 16 * 1024, "tail stays capped: {}", tail.len());
        assert!(
            !tail.starts_with('x') || tail.split('\n').next().unwrap().len() == 1024,
            "the tail opens on a whole line"
        );
        // Output for an unknown id is dropped, not panicking.
        app.bg_output("nope", "zzz");
    }

    #[test]
    fn bg_exited_removes_the_shell_and_returns_its_completion() {
        let mut app = App::new();
        app.bg_started("bash_1", "ping x.com", Some("Ping x".into()), true);
        app.bg_output("bash_1", "64 bytes\n");
        let completion = app.bg_exited("bash_1", Some(0), false).expect("completes");
        assert!(
            app.background().is_empty(),
            "the exited shell leaves the list"
        );
        assert_eq!(completion.id, "bash_1");
        assert_eq!(completion.code, Some(0));
        assert!(!completion.killed);
        assert!(completion.from_model);
        assert_eq!(completion.output_tail, "64 bytes");
        assert_eq!(completion.display_description(), "Ping x");
        // An unknown id (already swept by /clear) owes nothing.
        assert!(app.bg_exited("bash_1", Some(0), false).is_none());
    }

    #[test]
    fn completion_notice_headline_covers_every_outcome() {
        let mut notice = BackgroundNotice {
            description: "Ping x".into(),
            id: "bash_1".into(),
            code: Some(0),
            killed: false,
            output_tail: String::new(),
            timestamp: String::new(),
        };
        assert!(notice.ok());
        assert_eq!(
            notice.headline(),
            "Background command \"Ping x\" completed (exit code 0)"
        );
        notice.code = Some(2);
        assert!(!notice.ok());
        assert_eq!(
            notice.headline(),
            "Background command \"Ping x\" failed (exit code 2)"
        );
        notice.killed = true;
        assert_eq!(
            notice.headline(),
            "Background command \"Ping x\" was stopped by the user"
        );
        notice.killed = false;
        notice.code = None;
        assert_eq!(
            notice.headline(),
            "Background command \"Ping x\" was terminated by a signal"
        );
    }

    #[test]
    fn completion_notice_context_text_carries_the_tail() {
        let notice = BackgroundNotice {
            description: "Ping x".into(),
            id: "bash_1".into(),
            code: Some(0),
            killed: false,
            output_tail: "line1\nline2".into(),
            timestamp: String::new(),
        };
        assert_eq!(
            notice.context_text(),
            "[background] Background command \"Ping x\" (id bash_1) completed (exit code 0).\n\
             Final output (tail):\nline1\nline2"
        );
        let silent = BackgroundNotice {
            output_tail: String::new(),
            ..notice
        };
        assert!(silent.context_text().ends_with("(no output)"));
    }

    #[test]
    fn completion_context_text_matches_the_recorded_notices() {
        // The Exited arm posts the completion's context note to the registry
        // board the moment it lands, so the in-flight agent can inject it into
        // its next round (docs/background.md); the settle later records a
        // BackgroundNotice for the same completion. Both must read
        // identically, or the model would see one text mid-turn and a
        // different one in every later turn's derived context.
        let mut app = App::new();
        app.bg_started(
            "bash_1",
            "python3 server.py",
            Some("Start the API".into()),
            true,
        );
        app.bg_output("bash_1", "listening on 8888\n");
        let completion = app.bg_exited("bash_1", None, false).unwrap();
        let notice = app.record_background_notice(&completion);
        assert_eq!(completion.context_text(), notice.context_text());
        assert!(
            completion
                .context_text()
                .contains("was terminated by a signal"),
            "a signal death reads as terminated: {}",
            completion.context_text()
        );
    }

    #[test]
    fn record_background_notice_lands_in_history_stamped() {
        let mut app = App::new();
        app.set_clock(|| "01:02 PM".to_string());
        app.bg_started("bash_1", "ping x.com", None, true);
        let completion = app.bg_exited("bash_1", Some(0), false).unwrap();
        let notice = app.record_background_notice(&completion);
        assert_eq!(
            notice.description, "ping x.com",
            "falls back to the command"
        );
        assert_eq!(notice.timestamp, "01:02 PM");
        assert_eq!(
            app.history.last(),
            Some(&HistoryItem::Background(notice)),
            "the notice is a history item — it repaints and rides the context"
        );
    }

    #[test]
    fn completions_defer_and_drain_in_arrival_order() {
        let mut app = app_with_shells(&["a", "b"]);
        let first = app.bg_exited("bash_1", Some(0), false).unwrap();
        let second = app.bg_exited("bash_2", Some(1), false).unwrap();
        app.defer_bg_completion(first.clone());
        app.defer_bg_completion(second.clone());
        assert_eq!(app.take_pending_bg_completions(), vec![first, second]);
        assert!(app.take_pending_bg_completions().is_empty(), "drained once");
    }

    #[test]
    fn down_from_an_empty_composer_opens_the_manager_once_a_shell_ever_ran() {
        let mut app = App::new();
        // Before any shell: ↓ keeps its old meaning (a cursor no-op here).
        app.on_key(key(KeyCode::Down));
        assert!(app.background_view.is_none());
        app.bg_started("bash_1", "ping x.com", None, true);
        app.on_key(key(KeyCode::Down));
        assert_eq!(
            app.background_view,
            Some(BackgroundView::List { selected: 0 })
        );
        // …and it opens on the empty state even after every shell finished.
        app.close_background_view();
        app.bg_exited("bash_1", Some(0), false);
        app.on_key(key(KeyCode::Down));
        assert!(app.background_view.is_some(), "the empty state still opens");
    }

    #[test]
    fn down_with_a_draft_or_in_shell_mode_never_opens_the_manager() {
        let mut app = App::new();
        app.bg_started("bash_1", "ping x.com", None, true);
        app.input = TextArea::from_text("draft");
        app.on_key(key(KeyCode::Down));
        assert!(
            app.background_view.is_none(),
            "a draft keeps ↓ for the cursor"
        );
        app.input.clear();
        app.on_key(key(KeyCode::Char('!')));
        app.on_key(key(KeyCode::Down));
        assert!(app.background_view.is_none(), "shell mode keeps ↓ too");
    }

    #[test]
    fn manager_list_keys_select_view_stop_and_close() {
        let mut app = app_with_shells(&["a", "b", "c"]);
        app.open_background_view();
        // ↓/↑ move and clamp.
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Down));
        assert_eq!(
            app.background_view,
            Some(BackgroundView::List { selected: 2 }),
            "the selection clamps at the last row"
        );
        // x stops the highlighted shell (the view stays).
        let action = app.on_key(key(KeyCode::Char('x')));
        assert_eq!(action, Action::KillBackground("bash_3".into()));
        assert!(app.background_view.is_some());
        // Enter opens the details of the highlighted shell.
        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            app.background_view,
            Some(BackgroundView::Details {
                id: "bash_2".into()
            })
        );
        // ← goes back to the list, seated on that shell.
        app.on_key(key(KeyCode::Left));
        assert_eq!(
            app.background_view,
            Some(BackgroundView::List { selected: 1 })
        );
        // Esc closes the band.
        app.on_key(key(KeyCode::Esc));
        assert!(app.background_view.is_none());
    }

    #[test]
    fn manager_details_keys_close_and_stop() {
        let mut app = app_with_shells(&["a"]);
        app.open_background_view();
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(
            app.background_view,
            Some(BackgroundView::Details { .. })
        ));
        // x stops this shell.
        assert_eq!(
            app.on_key(key(KeyCode::Char('x'))),
            Action::KillBackground("bash_1".into())
        );
        // Space closes outright (Esc and Enter do too — spec).
        app.on_key(key(KeyCode::Char(' ')));
        assert!(app.background_view.is_none());
        app.open_background_view();
        app.on_key(key(KeyCode::Enter));
        app.on_key(key(KeyCode::Enter));
        assert!(app.background_view.is_none(), "Enter closes the details");
        // Ctrl+C closes from either page.
        app.open_background_view();
        app.on_key(ctrl('c'));
        assert!(app.background_view.is_none());
    }

    #[test]
    fn a_details_view_falls_back_to_the_list_when_its_shell_exits() {
        let mut app = app_with_shells(&["a", "b"]);
        app.open_background_view();
        app.on_key(key(KeyCode::Enter)); // details of bash_1
        app.bg_exited("bash_1", Some(0), false);
        assert_eq!(
            app.background_view,
            Some(BackgroundView::List { selected: 0 }),
            "the watched shell exited — back to the (re-clamped) list"
        );
        // The last shell exiting leaves the empty-state list open.
        app.bg_exited("bash_2", Some(0), false);
        assert!(matches!(
            app.background_view,
            Some(BackgroundView::List { .. })
        ));
    }

    #[test]
    fn ctrl_b_moves_only_a_running_command_to_the_background() {
        let mut app = App::new();
        assert_eq!(app.on_key(ctrl('b')), Action::None, "idle: nothing to move");
        // A running model bash call can move.
        app.begin_stream();
        app.start_tool("Bash", "ping x.com");
        assert!(app.can_move_to_background());
        assert_eq!(app.on_key(ctrl('b')), Action::MoveToBackground);
        app.end_tool("done", true);
        // A running non-command tool can't.
        app.start_tool("Read", "src/main.rs");
        assert!(!app.can_move_to_background());
        assert_eq!(app.on_key(ctrl('b')), Action::None);
    }

    #[test]
    fn ctrl_b_moves_a_running_shell_turn_too() {
        let mut app = App::new();
        app.begin_shell("ping x.com");
        assert!(app.can_move_to_background());
        assert_eq!(app.on_key(ctrl('b')), Action::MoveToBackground);
    }

    #[test]
    fn background_tool_resolves_the_front_call_as_backgrounded() {
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Bash", "ping x.com");
        let tool = app
            .background_tool("Command running in background with ID: bash_1.")
            .expect("resolves the running call");
        assert_eq!(tool.status, ToolStatus::Backgrounded);
        assert_eq!(
            tool.output,
            "Command running in background with ID: bash_1."
        );
        assert!(
            app.current_tool().is_none(),
            "the live queue is empty again"
        );
        assert_eq!(
            app.history.last(),
            Some(&HistoryItem::Tool(tool)),
            "the backgrounded cell is history — it repaints and rides context"
        );
    }

    #[test]
    fn end_turn_snapshots_the_running_shell_count() {
        let mut app = app_with_shells(&["a", "b", "c"]);
        app.begin_stream();
        let summary = app.end_turn(22).expect("a summary");
        assert_eq!(summary.shells, 3, "Done for 22s · 3 shells still running");
        // With none running the suffix stays off.
        let mut idle = App::new();
        idle.begin_stream();
        assert_eq!(idle.end_turn(2).unwrap().shells, 0);
    }

    #[test]
    fn clear_conversation_wipes_the_background_state() {
        let mut app = app_with_shells(&["a"]);
        let completion = app.bg_exited("bash_1", Some(0), false).unwrap();
        app.bg_started("bash_2", "b", None, false);
        app.defer_bg_completion(completion);
        // Run /clear the real way: type it (the palette opens) and Enter.
        for c in "/clear".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Clear);
        assert!(app.background().is_empty());
        assert!(app.background_view.is_none());
        assert!(app.take_pending_bg_completions().is_empty());
        assert!(!app.background_ever(), "the ↓ gate resets with the slate");
    }

    #[test]
    fn set_background_runtime_targets_one_shell() {
        let mut app = app_with_shells(&["a", "b"]);
        app.set_background_runtime("bash_2", Duration::from_secs(7));
        assert_eq!(app.background()[0].runtime, Duration::ZERO);
        assert_eq!(app.background()[1].runtime, Duration::from_secs(7));
        app.set_background_runtime("nope", Duration::from_secs(9)); // no panic
    }

    #[test]
    fn the_manager_band_owns_every_key_while_open() {
        let mut app = app_with_shells(&["a"]);
        app.open_background_view();
        // Typing does not reach the composer.
        app.on_key(key(KeyCode::Char('h')));
        assert!(app.input.is_empty());
        // Enter does not submit — it navigates to the details page instead.
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert!(matches!(
            app.background_view,
            Some(BackgroundView::Details { .. })
        ));
    }
}
