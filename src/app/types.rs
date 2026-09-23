//! The conversation's shared vocabulary: roles, messages, and the history items
//! they are recorded into.
//!
//! Pure data with no behaviour beyond derives — the state machine that drives
//! them lives in [`App`](super::App) and its sibling modules (`docs/design.md`).
//! A type that belongs to one feature lives with it instead: `ToolCall` in
//! [`tools`](super::tools), `Compaction` in [`compact`](super::compact),
//! `TurnStatus` in [`status`](super::status).

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

/// One ordered entry of finished conversation history: a [`Message`], a
/// [`ToolCall`], or a turn [`TurnSummary`]. They share a single ordered list so
/// the inline conversation repaints (after a resize, or when returning from the
/// tool-output view) in the exact order things streamed — assistant text, tool
/// calls, and the per-turn "Done" summary interleaved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryItem {
    Message(Message),
    Tool(ToolCall),
    /// A resolved task tool call (`docs/task-tools.md`) — **cell-less
    /// inline** (the live checklist is its display; `conversation_lines`
    /// skips it entirely) but recorded for the Ctrl+O transcript, the
    /// context replay, the rollout, and the snapshot the rewinds restore
    /// the list from.
    TaskCall(TaskCallRecord),
    Summary(TurnSummary),
    /// A background shell's completion notice (`docs/background.md`).
    Background(BackgroundNotice),
    /// A resolved subagent group — the `agent` tool's tree cell
    /// (`docs/agent-tool.md`).
    AgentGroup(AgentGroup),
    /// A background agent's completion notice (`docs/agent-tool.md`).
    AgentNotice(AgentNotice),
    /// A settled thinking phase — the collapsed `Thought for {n} · {t}
    /// tokens` cell whose chain-of-thought expands in the Ctrl+O transcript
    /// (`docs/thinking-stream.md`).
    Reasoning(Reasoning),
    /// A `/compact` marker (`docs/compact.md`): from here back, the model's
    /// context is the compacted shape — [`crate::context::context_messages`]
    /// derives the budgeted recent user texts + the summary bridge in place of
    /// the earlier items. Appended (never a rewrite), so the transcript, the
    /// recorder, and the checkpoint keys are untouched.
    Compaction(Compaction),
    /// Conversation text a **lifecycle hook** injected mid-turn
    /// (`docs/hooks.md`) — a `Stop` block's continuation feedback, a
    /// `UserPromptSubmit`/`SessionStart` hook's additional context.
    /// **Cell-less inline** (Claude Code hides these from the normal view
    /// too): `conversation_lines` skips it, the Ctrl+O transcript shows it
    /// under its `label`, and [`crate::context::context_messages`] replays
    /// `text` verbatim as the user-role message the model actually read.
    HookNote(HookNote),
}

/// One hook-injected conversation entry — see
/// [`HistoryItem::HookNote`] and `docs/hooks.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookNote {
    /// The short transcript heading (`Stop hook`, `UserPromptSubmit hook`).
    pub label: String,
    /// Verbatim what the model reads as a user-role message — formatted by
    /// the producer (`Stop hook feedback:\n…`, the system-reminder-wrapped
    /// additional context), so the replay is exactly the wire text.
    pub text: String,
    /// Wall-clock stamp (recorded like every item's; never displayed).
    pub timestamp: String,
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
    /// The full-screen, read-only Git change review.
    DiffReview,
}

impl View {
    /// Whether this view is painted on the terminal's **alternate screen**
    /// rather than inline — the Ctrl+O transcript, the Ctrl+D context view and
    /// the `/resume` picker.
    ///
    /// The one predicate that answers "is the inline live region on screen at
    /// all?", which is what decides whether the draw tick keeps re-arming the
    /// status animation's clock chain: under an overlay nothing it animates is
    /// visible, and repainting there for no reason is what dropped the
    /// terminal's text selection mid-turn (`docs/overlay-repaint.md`).
    #[must_use]
    pub const fn is_overlay(self) -> bool {
        matches!(
            self,
            Self::ToolOutput | Self::ResumePicker | Self::ContextDebug | Self::DiffReview
        )
    }
}

/// Which page the Ctrl+D view is showing — Tab flips between them
/// (`docs/context.md`, `docs/permissions.md`). Two windows onto "what is
/// this turn actually sending", one key apart: the model's own context, and
/// the bounded task context auto mode's classifier reads before every
/// command and MCP call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DebugPage {
    /// The raw LLM context window (the view's original, and its default).
    #[default]
    Context,
    /// The classifier's task context, boundary-fed through
    /// [`App::set_classifier_context`].
    ///
    /// [`App::set_classifier_context`]: crate::app::App::set_classifier_context
    Classifier,
}

impl DebugPage {
    /// The other page — what Tab shows.
    #[must_use]
    pub const fn flipped(self) -> Self {
        match self {
            Self::Context => Self::Classifier,
            Self::Classifier => Self::Context,
        }
    }
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
/// row's colour in `ui::footer` (dim vs. a softened red). See `docs/toast.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    /// A neutral confirmation or soft rejection (dim) — `Copied last message to
    /// clipboard`, `Switched model to …`, `/resume is disabled …`, `No agent
    /// response to copy`.
    Info,
    /// A failure (the theme's red softened toward the dim) — `Copy failed: …`,
    /// `Can't switch to …`, a config file that would not parse.
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
