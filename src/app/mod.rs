//! Conversation state and the pure update logic that drives it.
//!
//! Everything here is free of I/O so it can be unit-tested directly: the event
//! loop in `main.rs` feeds key presses in and reacts to the returned
//! [`Action`]s, and pushes streamed chunks in via [`App::push_chunk`].
//!
//! One module per area — see `docs/module-layout.md` for the map and
//! `docs/design.md` for how this sits in the loop. The [`App`] struct stays here
//! so every submodule and the test tree keeps its private-field access.

use std::collections::{HashSet, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::agents::{AgentRun, AgentStatus};
use crate::file_search::{FileMatch, at_token};
use crate::llm::{ModelEntry, ReasoningSupport, ThinkingMode};
use crate::permission::{PermissionDecision, PermissionKind, PermissionMode, PermissionRequest};
use crate::session::SessionSummary;
use crate::stream::{AgentCallDone, AgentSpec, StreamEvent, ToolCallSummary};
use crate::textarea::TextArea;

mod action;
mod agent;
mod background;
mod backtrack;
mod commands;
mod compact;
mod composer;
mod file_picker;
mod input_history;
mod keys;
mod login;
mod model_picker;
mod permission;
mod queue;
mod resume;
mod status;
mod tools;
mod turn;
mod types;
mod views;

pub use self::action::Action;
pub use self::agent::{
    AGENT_STOPPED_OUTPUT, AgentGroup, AgentGroupEntry, AgentGroupLive, AgentNotice,
};
pub use self::background::{BackgroundNotice, BackgroundShell, BackgroundView, BgCompletion};
pub use self::backtrack::{Backtrack, CHECKPOINT_RESTORED_NOTICE, CHECKPOINT_REWOUND_NOTICE};
pub use self::commands::{
    COMMANDS, COMPACT_BUSY_NOTICE, COMPACT_EMPTY_NOTICE, COPY_EMPTY_NOTICE, COPY_OK_NOTICE,
    CommandEffect, CommandMenu, HELP_BUSY_NOTICE, INIT_BUSY_NOTICE, INIT_PROMPT,
    RESUME_BUSY_NOTICE, SlashCommand, command_query, matching_commands,
};
pub use self::compact::{COMPACT_VERB, Compaction};
pub use self::composer::{SHELL_EMPTY_NOTICE, shell_query};
pub use self::file_picker::FileSearch;
pub use self::input_history::{HistorySearch, InputHistory, SearchState};
pub use self::login::{KeyOnboarding, KeyStep, ProviderChoice};
pub use self::model_picker::{ModelFetchError, ModelLoad, ModelPicker};
pub use self::permission::PermissionPrompt;
pub use self::queue::QueuedTurn;
pub use self::resume::{ResumeControl, ResumeFilter, ResumePicker, ResumeSort};
pub use self::status::{RetryInfo, ThinkingState, TokenArrow, TurnStatus, TurnSummary};
pub use self::tools::{ERROR_TOOL_OUTPUT, INTERRUPT_TOOL_OUTPUT, ToolCall, ToolStatus};
pub use self::turn::{
    DONE_VERBS, INTERRUPT_NOTICE, InterruptedTurn, SHELL_VERB, StreamError, WORKING_VERBS,
    format_elapsed,
};
pub(crate) use self::types::count_tokens;
pub use self::types::{HistoryItem, Message, Role, SessionInfo, Toast, ToastKind, View};

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
    /// (`on_key_search`) and the footer slot shows the query line. See
    /// `docs/history-search.md`.
    ///
    /// [`input_history`]: App::input_history
    pub history_search: Option<HistorySearch>,
    /// Whether the composer is in `!` shell mode — codex's absorbed-prefix
    /// `is_bash_mode`: the leading `!` is held here, *not* in the textarea, so
    /// the box renders `! pwd` (the bang as the prompt) instead of `❯ !pwd`.
    /// Entered by typing `!` first (`sync_shell_mode` absorbs it), exited by
    /// Backspace/Esc on an empty composer or by submitting; the footer slot
    /// shows a red `Shell mode` hint while it's on. See `docs/shell-command.md`.
    ///
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
    /// `{used}/{window} ({pct}%)` gauge and the auto-compact trigger; `None`
    /// hides both. See `docs/compact.md`.
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
    /// The system prompt a launched **subagent** is sent — the main prompt
    /// with the subagent note appended — injected beside
    /// [`system_prompt`](Self::system_prompt)
    /// ([`App::set_agent_system_prompt`], from
    /// `ReplySource::agent_system_prompt`) so the *agent session view's*
    /// Ctrl+D shows what that agent actually gets. See `docs/agent-tool.md`.
    pub agent_system_prompt: Option<String>,
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
    /// same token); see `App::refresh_command_menu`.
    pub command_menu: Option<CommandMenu>,
    /// The open `@` file picker (when the cursor is in an `@token`); `None` when
    /// closed. Esc dismisses it (sticky within the token); the boundary fetches
    /// its matches asynchronously. See `App::refresh_file_search` and
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
    /// ([`on_paste`]) while its text waits here; `take_input` splices it back in
    /// when the draft is sent. Cleared whenever the composer empties (every
    /// `take`), but **not** by `/clear` (the draft survives `/clear`, so its
    /// pastes do too). See `docs/paste.md`.
    ///
    /// [`on_paste`]: App::on_paste
    pub pasted: Vec<(String, String)>,
    /// Ctrl+V-pasted images currently in the composer, as `(placeholder, path)`
    /// pairs in insertion order — the image analogue of [`pasted`] (codex's
    /// `AttachedImage`). [`attach_image`] inserts an `[Image #N]` placeholder and
    /// records the temp-PNG path here; unlike a text paste the placeholder is
    /// **not** expanded on send (it stays in the message text), and the path is
    /// surfaced separately via [`take_submission_images`]. Cleared by
    /// `take_input` (any draft-take drops attachments), so the idle submit path
    /// stages them into `submission_images` first. See `docs/image-paste.md`.
    ///
    /// [`pasted`]: App::pasted
    /// [`attach_image`]: App::attach_image
    /// [`take_submission_images`]: App::take_submission_images
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
    /// Is the footer's `{n} shell(s)` indicator **focused** — lit on cyan,
    /// waiting for the Enter that opens the manager band? ↓ from an idle
    /// composer sets it (Claude-Code-style: step onto the indicator first,
    /// don't jump straight into the band), any other key clears it, and the
    /// last shell exiting clears it with the indicator. See
    /// `docs/background.md`.
    background_focus: bool,
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
    /// The **animation frame clock** — time since the loop started, injected
    /// before every draw ([`set_pulse`](App::set_pulse)). Purely a phase: the
    /// live region's running bullets breathe against it, in unison, and nothing
    /// ever displays it. See `docs/tool-pulse.md`.
    pulse: Duration,
    /// The subagent roster (`docs/agent-tool.md`): one [`AgentRun`] per
    /// launched agent, created at its [`StreamEvent::AgentBatch`]
    /// announcement and updated from the dedicated agent channel
    /// ([`apply_agent_event`](App::apply_agent_event)). Drives the footer
    /// list, the live group cell's tree rows, and the agent session view. A
    /// finished agent lingers a few seconds (the boundary sweeps it via
    /// [`remove_agent`](App::remove_agent)); a user `x` hides it at once.
    agents: Vec<AgentRun>,
    /// Bumped on every roster mutation — what the Ctrl+O transcript cache's
    /// signature fingerprints so a streaming agent invalidates it.
    agents_generation: u64,
    /// The round's live agent group between its announcement and its
    /// resolution — the strip's blue `● Running {n} agents…` tree cell.
    agent_group: Option<AgentGroupLive>,
    /// The ↓ roster navigation: `Some(0)` highlights the `● main` row,
    /// `Some(i+1)` the i-th **visible** roster entry. The footer line swaps
    /// to the selection hints while it is `Some`. See `docs/agent-tool.md`.
    agent_selection: Option<usize>,
    /// The agent session view: `Some(id)` while the inline screen shows that
    /// agent's own conversation (the composer chats with it; Esc returns).
    /// See `docs/agent-tool.md`.
    pub agent_view: Option<String>,
    /// Background-agent completions awaiting a safe boundary — settled
    /// beside [`pending_bg`](Self::pending_bg) (same sites, same rules).
    pending_agents: VecDeque<AgentNotice>,
    /// The open tool-permission prompt (`docs/permissions.md`): a `write`,
    /// `edit`, or `bash` call whose thread is blocked on the user's answer.
    /// Inline like [`model_picker`](Self::model_picker) — it replaces the
    /// whole live region and owns every key while open. Read through
    /// [`permission`](Self::permission).
    permission: Option<PermissionPrompt>,
    /// Requests that arrived while one was already open (two agents asking at
    /// once), oldest first — each opens as the one before it resolves.
    pending_permissions: VecDeque<PermissionRequest>,
    /// Ids of requests dropped without an answer (Esc, `/clear`), drained by
    /// the boundary and released on the gate so no tool thread parks forever —
    /// see [`take_abandoned_permissions`](Self::take_abandoned_permissions).
    abandoned_permissions: Vec<String>,
    /// The session's permission posture (`manual`/`edit`) — pinned at the
    /// footer's right edge, toggled with Ctrl+A. `None` while permissions are
    /// disabled (`ALTER_ZERO_PERMISSIONS=0`), which hides the segment and
    /// makes the toggle explain itself instead. Injected at startup and kept
    /// in sync by the boundary (the gate owns the live rules); see
    /// `docs/permissions.md`.
    permission_mode: Option<PermissionMode>,
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

    /// Inject the active backend's system prompt (from
    /// `ReplySource::system_prompt`, at startup and on a `/model` switch) so
    /// the Ctrl+D view shows the whole context window. See `docs/context.md`.
    pub fn set_system_prompt(&mut self, prompt: Option<String>) {
        self.system_prompt = prompt;
    }

    /// Inject the prompt a launched subagent is sent (from
    /// `ReplySource::agent_system_prompt`, beside every
    /// [`set_system_prompt`](Self::set_system_prompt)) so the agent session
    /// view's Ctrl+D shows it — the main prompt plus the subagent note. See
    /// `docs/agent-tool.md`.
    pub fn set_agent_system_prompt(&mut self, prompt: Option<String>) {
        self.agent_system_prompt = prompt;
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

    /// Record a system notice (from a slash command) in the history, so it
    /// repaints on resize like any other message. The loop also commits it to
    /// scrollback. Mirrors [`record_user_message`](App::record_user_message) for
    /// [`Action::Notice`].
    pub fn record_system_message(&mut self, text: &str) {
        self.record_message(Role::System, text);
    }

    /// Record a red error notice in the history (so a resize repaints it), like
    /// [`record_system_message`](App::record_system_message) but [`Role::Error`]. Used
    /// for a Ctrl+V clipboard
    /// failure — codex's `new_error_event`. See `docs/image-paste.md`.
    pub fn record_error_message(&mut self, text: &str) {
        self.record_message(Role::Error, text);
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
        self.background_focus = false;
        self.pending_bg.clear();
        // And the subagents — the loop's Clear arm kills their threads via
        // the registry; the wiped roster drops the late events
        // (docs/agent-tool.md).
        self.agents.clear();
        self.agents_generation += 1;
        self.agent_group = None;
        self.agent_selection = None;
        self.agent_view = None;
        self.pending_agents.clear();
        // The blocked tool threads are reaped by the loop's Clear arm (their
        // cancel token trips); the prompt they were waiting on goes with them
        // (docs/permissions.md). The composer keeps its draft, as `/clear`
        // always has — `discard_permissions` restores it first.
        self.discard_permissions();
    }
}

#[cfg(test)]
mod tests;
