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
}

/// The lifecycle of a tool call — selects its bullet colour when rendered:
/// running is blue, success green, failure red.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    /// Executing — shown live (blue) in the bottom region while it runs.
    Running,
    /// Finished successfully (green).
    Ok,
    /// Finished with an error (red).
    Failed,
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

/// A rough token estimate for `text` (≈ 4 characters per token, the usual
/// heuristic). The dummy has no real tokenizer, so the status line's counts are
/// approximate — but accumulate faithfully as text and tool output arrive.
#[must_use]
fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
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
    /// point, `ToolEnd` still owed (the interrupt's [`InterruptedTurn::tool`]
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
/// the event loop to flush to scrollback — the kept partial reply and the
/// cancelled tool, both also recorded in [`App::history`] (followed by the
/// [`INTERRUPT_NOTICE`]) so a later resize repaints them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptedTurn {
    /// The reply text streamed before the interrupt, if any non-empty text
    /// arrived since the last flush. Kept, codex-style — never retracted.
    pub partial: Option<String>,
    /// The tool that was mid-run, now resolved as failed with
    /// [`INTERRUPT_TOOL_OUTPUT`], if one was running.
    pub tool: Option<ToolCall>,
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

/// How many lines PageUp/PageDown move the tool-output view.
const TOOL_VIEW_PAGE: usize = 10;

/// What running a slash command does. The palette dispatches one of these on
/// select; `App::run_selected_command` turns it into an [`Action`] for the loop.
/// Wiring a stub up later is just swapping its effect here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEffect {
    /// Clear the conversation history (`/clear`).
    Clear,
    /// Post the list of available commands as a system notice (`/help`).
    Help,
    /// Copy the last assistant response to the clipboard (`/copy`). See
    /// `docs/copy.md`.
    Copy,
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
        name: "quit",
        description: "Exit inline-tui",
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
}

impl InputHistory {
    /// Record a submitted (or Ctrl+C-cleared) input and exit browsing. Blank
    /// texts are ignored and an entry identical to the newest is collapsed,
    /// like codex's `record_local_submission`.
    pub fn record(&mut self, text: &str) {
        self.cursor = None;
        self.last_recall = None;
        if text.is_empty() || self.entries.last().is_some_and(|prev| prev == text) {
            return;
        }
        self.entries.push(text.to_string());
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
    /// The tool currently executing (status [`ToolStatus::Running`]), shown live
    /// in the bottom region; `None` when no tool is in flight. Private: read
    /// through [`current_tool`](App::current_tool).
    current_tool: Option<ToolCall>,
    /// Every finished message and tool call, oldest first — used to repaint after
    /// a resize or on returning from the tool-output view.
    pub history: Vec<HistoryItem>,
    /// Which screen is showing (Ctrl+O toggles to the tool-output view).
    pub view: View,
    /// The tool-output view's vertical scroll offset, in lines from the top.
    pub tool_scroll: usize,
    /// Whether the tool-output view is pinned to the bottom (tail-follow): it
    /// opens this way and re-streams keep the latest content in view, until you
    /// scroll up to read back (and re-engages when you scroll to the bottom).
    pub tool_follow: bool,
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
    /// The just-submitted turn's image paths, staged by the idle submit path
    /// (moved out of [`images`] before [`take_input`] clears the composer) for the
    /// loop to drain via [`take_submission_images`] — codex's
    /// `recent_submission_images`. See `docs/image-paste.md`.
    ///
    /// [`images`]: App::images
    /// [`take_input`]: App::take_input
    /// [`take_submission_images`]: App::take_submission_images
    submission_images: Vec<PathBuf>,
    /// Temp-PNG paths of attachments that were **dropped without being
    /// submitted** — an atomic placeholder delete, a Ctrl+C-cleared draft, a
    /// `/clear`'d queue. The pure core only records the drops; the boundary
    /// drains [`take_discarded_images`] and removes the files (the file I/O
    /// stays out of the library). Submitted images are *not* recorded here —
    /// their files outlive the send. See `docs/image-paste.md`.
    ///
    /// [`take_discarded_images`]: App::take_discarded_images
    discarded_images: Vec<PathBuf>,
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

    /// Inject the wall-clock used to stamp recorded items (called once at the I/O
    /// boundary in `main.rs`). Each recorded message/tool then stores `clock()`'s
    /// value, which is shown **only** in the Ctrl+O transcript.
    pub fn set_clock(&mut self, clock: fn() -> String) {
        self.clock = Some(clock);
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
        let timestamp = self.now_stamp();
        self.history.push(HistoryItem::Message(Message {
            role,
            text: text.into(),
            timestamp,
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
            let placeholder = self.delete_span(span);
            self.pasted.retain(|(ph, _)| ph != &placeholder);
            return true;
        }
        if let Some(span) = crate::paste::placeholder_to_delete(
            self.input.text(),
            self.input.cursor(),
            &self.images,
            backward,
        ) {
            let placeholder = self.delete_span(span);
            // The dropped attachment's temp file is orphaned now — hand its
            // path to the boundary for removal (docs/image-paste.md).
            let (dropped, kept) = std::mem::take(&mut self.images)
                .into_iter()
                .partition(|(ph, _)| ph == &placeholder);
            self.images = kept;
            self.discarded_images
                .extend(dropped.into_iter().map(|(_, path)| path));
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

    /// Take the image paths staged by the last idle submit (codex's
    /// `take_recent_submission_images`): the loop drains these into
    /// [`crate::stream::ReplySource::spawn`] alongside the text prompt. See
    /// `docs/image-paste.md`.
    pub fn take_submission_images(&mut self) -> Vec<PathBuf> {
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
        // Ctrl+C: in the conversation, a first press with text in the input
        // clears the draft instead of quitting (codex's composer-clear step —
        // see docs/design.md); otherwise it quits, from either screen. The
        // overlay never shows the input box, so there is nothing to clear there.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.view == View::Conversation && !self.input.is_empty() {
                // Record the cleared draft so ↑ can bring it back (codex's
                // clear_for_ctrl_c does the same). A shell-mode draft re-gains
                // its `!` so the recall re-enters the mode.
                let mut text = self.take_input();
                if self.shell_mode {
                    self.shell_mode = false;
                    text = format!("!{text}");
                }
                self.input_history.record(&text);
                self.command_menu = None; // an emptied input can't be a /token
                self.file_search = None; // …nor an @token, so close the picker too
                return Action::None;
            }
            return Action::Quit;
        }
        // Ctrl+O toggles the full-screen tool-output view from either screen —
        // even mid-stream, so the conversation keeps updating underneath it.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
            self.toggle_tool_view();
            return Action::ToggleToolView;
        }
        match self.view {
            View::Conversation => self.on_key_conversation(key),
            View::ToolOutput => self.on_key_tool_view(key),
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
                    // composer (the placeholder text stays; only the paths travel
                    // the side channel — docs/image-paste.md).
                    self.submission_images = std::mem::take(&mut self.images)
                        .into_iter()
                        .map(|(_, path)| path)
                        .collect();
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
                self.input.move_down();
                Action::None
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
        self.input_history
            .should_navigate(self.input.text(), self.input.cursor())
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
            CommandEffect::Help => Action::Notice(help_text()),
            CommandEffect::Copy => Action::Copy(self.last_assistant_text()),
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
        self.refresh_command_menu(false);
        // An accepted `!entry` re-enters shell mode (the recall rule).
        self.sync_shell_mode();
    }

    /// Show the pre-search draft again without closing the search — what a
    /// no-match query (and a query cleared back to empty) displays.
    fn restore_search_snapshot(&mut self) {
        if let Some(search) = self.history_search.as_ref() {
            self.input = search.snapshot.clone();
        }
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
                Action::ToggleToolView
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
            View::ToolOutput => View::Conversation,
        };
        self.tool_scroll = 0;
        self.tool_follow = self.view == View::ToolOutput;
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
    /// repaint rebuilds the inline view from the truncated history. See
    /// `docs/backtrack.md`.
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
        self.history.truncate(position);
        self.backtrack = Backtrack::default();
        self.view = View::Conversation;
        self.recall_input(&text);
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

    /// Record a finished user message in the history.
    pub fn record_user_message(&mut self, text: &str) {
        self.record_message(Role::User, text);
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

    /// Begin a tool call: record it as the currently-running tool so the bottom
    /// region can show it live (blue) before any output arrives.
    pub fn start_tool(&mut self, name: &str, args: &str) {
        self.current_tool = Some(ToolCall {
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
        if let Some(tool) = self.current_tool.as_mut() {
            tool.truncated = true;
        }
    }

    /// The tool currently executing, if any.
    #[must_use]
    pub fn current_tool(&self) -> Option<&ToolCall> {
        self.current_tool.as_ref()
    }

    /// Finish the in-flight tool call with its final `output` and outcome
    /// (`ok` → [`ToolStatus::Ok`], else [`ToolStatus::Failed`]), record it in the
    /// history, and clear the running slot. Returns the finished call (for the
    /// event loop to commit to scrollback), or `None` if no tool was running.
    pub fn end_tool(&mut self, output: &str, ok: bool) -> Option<ToolCall> {
        let mut tool = self.current_tool.take()?;
        tool.output = output.to_string();
        tool.status = if ok {
            ToolStatus::Ok
        } else {
            ToolStatus::Failed
        };
        tool.timestamp = self.now_stamp();
        // Fold the tool's output into the cumulative tally (arrow up — uploaded
        // back); the count is *added to*, never reset (see docs/status-indicator.md).
        if let Some(status) = self.status.as_mut() {
            status.tokens += estimate_tokens(output);
            status.arrow = TokenArrow::Up;
        }
        self.history.push(HistoryItem::Tool(tool.clone()));
        Some(tool)
    }

    /// Finalise the current run of assistant text as a history message so a
    /// following tool call slots after it in order, then start a fresh empty
    /// buffer for the text that follows. Returns the finalised segment text (for
    /// the loop to flush to scrollback) or `None` if there was nothing buffered.
    /// The stream stays open.
    pub fn flush_streaming_segment(&mut self) -> Option<String> {
        let buf = self.streaming.as_mut()?;
        if buf.is_empty() {
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
        });
        self.start_tool(command, "");
        if let Some(tool) = self.current_tool.as_mut() {
            tool.shell = true;
        }
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
            status.tokens += estimate_tokens(text);
            status.arrow = TokenArrow::Up;
        }
    }

    /// Append a streamed chunk to the in-progress reply (and grow the live token
    /// tally, arrow pointing down — output streaming). No-op if not streaming.
    pub fn push_chunk(&mut self, chunk: &str) {
        if let Some(buf) = self.streaming.as_mut() {
            buf.push_str(chunk);
        }
        if let Some(status) = self.status.as_mut() {
            status.tokens += estimate_tokens(chunk);
            status.arrow = TokenArrow::Down;
        }
    }

    /// Count a streamed reasoning delta into the live token tally (arrow down —
    /// it is model output, streaming) **without** touching the reply buffer:
    /// the text itself is opaque and never rendered. This is what keeps the
    /// count ticking while the status line shows `Thinking for Ns`. No-op when
    /// no turn is in flight.
    pub fn push_thinking(&mut self, chunk: &str) {
        if let Some(status) = self.status.as_mut() {
            status.tokens += estimate_tokens(chunk);
            status.arrow = TokenArrow::Down;
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
        // record (or later commit) a phantom empty assistant message for it.
        if text.is_empty() {
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
        let status = self.status.take()?;
        // A `!` shell turn ends without a summary: its committed cell
        // (`! cmd` + `⎿ output`) is the record (docs/shell-command.md).
        if status.shell {
            return None;
        }
        let summary = TurnSummary {
            verb: status.done_verb,
            secs: elapsed_secs,
            timestamp: self.now_stamp(),
        };
        self.history.push(HistoryItem::Summary(summary.clone()));
        Some(summary)
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
        let partial = if streamed.is_empty() {
            None
        } else {
            self.record_message(Role::Assistant, streamed.clone());
            Some(streamed)
        };
        let tool = self.end_tool(ERROR_TOOL_OUTPUT, false);
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

    /// End the in-progress turn because the user interrupted it (Esc).
    ///
    /// Keeps any non-empty partial reply as an assistant message (codex keeps
    /// what already streamed), resolves a still-running tool as
    /// [`ToolStatus::Failed`] with [`INTERRUPT_TOOL_OUTPUT`], records the
    /// [`INTERRUPT_NOTICE`] as a [`Role::Error`] message, and clears the live
    /// status **without** a `Done for Ns` summary (like [`App::fail_stream`],
    /// the notice is the turn's terminal state). Returns the
    /// [`InterruptedTurn`] for the event loop to flush to scrollback, or
    /// `None` if no turn was in flight. See `docs/interrupt.md`.
    pub fn interrupt_turn(&mut self) -> Option<InterruptedTurn> {
        if !self.is_streaming() && !self.turn_active() {
            return None;
        }
        // Keep the partial in stream order: the buffer's text streamed before
        // the running tool started (in practice a ToolStart flushed it, so the
        // two are never both non-empty — but the order holds regardless).
        let partial = self.streaming.take().filter(|text| !text.is_empty());
        if let Some(text) = &partial {
            self.record_message(Role::Assistant, text.clone());
        }
        let tool = self.end_tool(INTERRUPT_TOOL_OUTPUT, false);
        self.record_message(Role::Error, INTERRUPT_NOTICE);
        self.status = None;
        Some(InterruptedTurn { partial, tool })
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
        self.streaming = None;
        self.current_tool = None;
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
    }
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
                HistoryItem::Tool(_) | HistoryItem::Summary(_) => None,
            })
            .collect()
    }

    /// The `Message` at history index `i` (panics if it is not a message).
    fn message_at(app: &App, i: usize) -> &Message {
        match &app.history[i] {
            HistoryItem::Message(m) => m,
            HistoryItem::Tool(_) | HistoryItem::Summary(_) => {
                panic!("expected a message at history[{i}]")
            }
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
            vec![PathBuf::from("/tmp/a.png")]
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
        let interrupted = app.interrupt_turn().expect("a turn was active");
        assert_eq!(interrupted.partial.as_deref(), Some("half a rep"));
        assert!(interrupted.tool.is_none(), "no tool was running");
        assert!(!app.is_streaming());
        assert!(!app.turn_active(), "the live status cleared");
        assert_eq!(roles(&app), vec![Role::Assistant, Role::Error]);
        assert_eq!(message_at(&app, 0).text, "half a rep");
        assert_eq!(message_at(&app, 1).text, INTERRUPT_NOTICE);
    }

    #[test]
    fn interrupt_turn_with_no_partial_records_only_the_notice() {
        let mut app = App::new();
        app.begin_stream(); // interrupted before any chunk arrived
        let interrupted = app.interrupt_turn().expect("a turn was active");
        assert!(interrupted.partial.is_none());
        assert_eq!(roles(&app), vec![Role::Error]);
    }

    #[test]
    fn interrupt_turn_resolves_a_running_tool_as_failed() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("before the tool ");
        app.start_tool("Bash", "sleep 100"); // flush happens loop-side; buffer keeps streaming
        let interrupted = app.interrupt_turn().expect("a turn was active");
        let tool = interrupted.tool.expect("the running tool was resolved");
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
            vec![PathBuf::from("/tmp/pic.png")]
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
        assert_eq!(matching_commands("c").len(), 2, "/clear and /copy");
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
        app.set_session_info("dummy_model_name", "~/inline-tui");
        let session = app.session.as_ref().expect("session info stored");
        assert_eq!(session.model, "dummy_model_name");
        assert_eq!(session.cwd, "~/inline-tui");
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
    fn estimate_tokens_is_zero_for_empty_and_grows_with_length() {
        assert_eq!(estimate_tokens(""), 0);
        assert!(estimate_tokens("a") >= 1);
        assert!(estimate_tokens("a much longer string") > estimate_tokens("a"));
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
        let interrupted = app.interrupt_turn().expect("a turn was in flight");
        let tool = interrupted.tool.expect("the running command is resolved");
        assert_eq!(tool.name, "sleep 5");
        assert_eq!(tool.status, ToolStatus::Failed);
        assert!(app.current_tool().is_none());
        assert!(!app.turn_active());
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
    fn esc_with_a_draft_never_primes() {
        // Priming requires an empty composer (codex's composer_is_empty guard);
        // with a draft the idle fall-through keeps its old meaning.
        let mut app = App::new();
        exchange(&mut app, "hello", "hi");
        app.input = TextArea::from_text("draft");
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Quit);
        assert!(!app.backtrack.primed);
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
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::ToggleToolView);
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
}
