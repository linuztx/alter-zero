//! Conversation state and the pure update logic that drives it.
//!
//! Everything here is free of I/O so it can be unit-tested directly: the event
//! loop in `main.rs` feeds key presses in and reacts to the returned
//! [`Action`]s, and pushes streamed chunks in via [`App::push_chunk`].

use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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

/// A rough token estimate for `text` (≈ 4 characters per token, the usual
/// heuristic). The dummy has no real tokenizer, so the status line's counts are
/// approximate — but accumulate faithfully as text and tool output arrive.
#[must_use]
fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// What a backend error leaves behind, handed to the event loop to flush to
/// scrollback. The partial reply (if any) and the error are also recorded in
/// [`App::history`] so a later resize repaints them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamError {
    /// The reply text streamed before the error, if any non-empty text arrived.
    pub partial: Option<String>,
    /// The error message to show the user.
    pub error: String,
}

/// The notice committed when the user interrupts a turn (Esc mid-generation) —
/// codex's wording, minus its `/feedback` plug. Recorded as a [`Role::Error`]
/// message: like a backend failure, the notice is the turn's terminal state
/// (no `Done for Ns` summary). See `docs/interrupt.md`.
pub const INTERRUPT_NOTICE: &str =
    "Conversation interrupted - tell the model what to do differently.";

/// The output recorded on a tool that was still running when the user
/// interrupted: it resolves as [`ToolStatus::Failed`] with this explanation
/// (codex: an aborted tool "may have partially executed").
pub const INTERRUPT_TOOL_OUTPUT: &str = "Interrupted by user";

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
    /// The user submitted a (non-empty) message; start a reply for it.
    Submit(String),
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
    /// `Some(buffer)` while the AI reply is streaming, accumulating chunks.
    pub streaming: Option<String>,
    /// The tool currently executing (status [`ToolStatus::Running`]), shown live
    /// in the bottom region; `None` when no tool is in flight.
    pub current_tool: Option<ToolCall>,
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
    /// The open slash-command palette (when the input is a bare command token);
    /// `None` when closed. Esc dismisses it (and it stays dismissed within the
    /// same token); see [`App::refresh_command_menu`].
    pub command_menu: Option<CommandMenu>,
    /// The live status of the turn in flight (verb, token tally, arrow, and the
    /// boundary-supplied seconds), shown in the strip above the box. `Some` from
    /// [`begin_stream`] until the turn ends; `None` when idle. See
    /// `docs/status-indicator.md`.
    ///
    /// [`begin_stream`]: App::begin_stream
    pub status: Option<TurnStatus>,
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

    /// The current timestamp from the injected clock, or empty when none is set
    /// (the unit-test default — so equality tests on recorded items still hold).
    fn now_stamp(&self) -> String {
        self.clock.map_or_else(String::new, |clock| clock())
    }

    /// Handle one key press and report what the event loop should do.
    ///
    /// Sending is disabled while a reply streams; quitting (Ctrl+C) and the
    /// tool-view toggle (Ctrl+O) always work, from either screen. Other keys are
    /// dispatched to the active [`View`].
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C: in the conversation, a first press with text in the input
        // clears the draft instead of quitting (codex's composer-clear step —
        // see docs/design.md); otherwise it quits, from either screen. The
        // overlay never shows the input box, so there is nothing to clear there.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.view == View::Conversation && !self.input.is_empty() {
                // Record the cleared draft so ↑ can bring it back (codex's
                // clear_for_ctrl_c does the same).
                self.input_history.record(&self.input.take());
                self.command_menu = None; // an emptied input can't be a /token
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
        let menu_open = self.command_menu.is_some();
        match key.code {
            // Esc dismisses the palette when it's open (codex's "popup wins"
            // rule — even mid-turn); else it interrupts an in-flight turn
            // (codex-style, see docs/interrupt.md); else it quits as before.
            KeyCode::Esc if menu_open => {
                self.command_menu = None;
                Action::None
            }
            KeyCode::Esc if self.turn_active() => Action::Interrupt,
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
            KeyCode::Tab if menu_open => self.run_selected_command(),
            KeyCode::Enter if menu_open => {
                if self.highlighted_command().is_some() {
                    self.run_selected_command()
                } else {
                    // A query that matches nothing isn't a message — swallow Enter
                    // rather than submitting the literal "/typo".
                    Action::None
                }
            }
            // Alt+Enter (and Shift+Enter where the terminal reports it) inserts a
            // newline at the cursor so the input box grows on demand; a plain Enter
            // submits.
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.input.insert_newline();
                Action::None
            }
            KeyCode::Enter => {
                if self.is_streaming() || self.input.text().trim().is_empty() {
                    Action::None
                } else {
                    let text = self.input.take();
                    self.input_history.record(&text);
                    Action::Submit(text)
                }
            }
            // Editing and cursor movement, dispatched to the textarea. Backspace /
            // Delete / typing also re-derive the slash-command palette.
            KeyCode::Backspace => {
                let had_query = command_query(self.input.text()).is_some();
                self.input.delete_backward();
                self.refresh_command_menu(had_query);
                Action::None
            }
            KeyCode::Delete => {
                let had_query = command_query(self.input.text()).is_some();
                self.input.delete_forward();
                self.refresh_command_menu(had_query);
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
            // Plain (and Shift-modified) characters insert at the cursor; ALT/CONTROL
            // combos are not text, so they are ignored here.
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let had_query = command_query(self.input.text()).is_some();
                self.input.insert_char(c);
                self.refresh_command_menu(had_query);
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
    /// re-derived so recalling a bare `/token` reopens it like typing one.
    fn recall_input(&mut self, text: &str) {
        let had_query = command_query(self.input.text()).is_some();
        self.input.set_text(text);
        self.refresh_command_menu(had_query);
    }

    /// Re-derive the palette after an edit. Opens it when the input *becomes* a
    /// command token, clamps the highlight when the filter narrows, and closes it
    /// when the input stops being a command token. The `had_query` flag (the state
    /// *before* the edit) makes Esc sticky: once dismissed, editing within the same
    /// token won't reopen the palette — only leaving and re-entering command mode
    /// (a None→Some transition) does.
    fn refresh_command_menu(&mut self, had_query: bool) {
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
                self.history.clear();
                Action::Clear
            }
            CommandEffect::Help => Action::Notice(help_text()),
            CommandEffect::Quit => Action::Quit,
        }
    }

    /// Keys while the full-screen tool-output view is showing: it is a read-only
    /// scroller, so typing is ignored; Esc (like Ctrl+O) returns to the chat.
    fn on_key_tool_view(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.toggle_tool_view(); // Esc closes the overlay, back to chat
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
            _ => Action::None,
        }
    }

    /// Flip between the conversation and the tool-output view. Entering the view
    /// pins it to the bottom (tail-follow) so it opens on the latest content.
    fn toggle_tool_view(&mut self) {
        self.view = match self.view {
            View::Conversation => View::ToolOutput,
            View::ToolOutput => View::Conversation,
        };
        self.tool_scroll = 0;
        self.tool_follow = self.view == View::ToolOutput;
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
        let timestamp = self.now_stamp();
        self.history.push(HistoryItem::Message(Message {
            role: Role::User,
            text: text.to_string(),
            timestamp,
        }));
    }

    /// Record a system notice (from a slash command) in the history, so it
    /// repaints on resize like any other message. The loop also commits it to
    /// scrollback. Mirrors [`record_user_message`] for [`Action::Notice`].
    pub fn record_system_message(&mut self, text: &str) {
        let timestamp = self.now_stamp();
        self.history.push(HistoryItem::Message(Message {
            role: Role::System,
            text: text.to_string(),
            timestamp,
        }));
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
        });
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
        let timestamp = self.now_stamp();
        self.history.push(HistoryItem::Message(Message {
            role: Role::Assistant,
            text: text.clone(),
            timestamp,
        }));
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
        });
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
        let timestamp = self.now_stamp();
        self.history.push(HistoryItem::Message(Message {
            role: Role::Assistant,
            text: text.clone(),
            timestamp,
        }));
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
    /// Records any non-empty partial reply as an assistant message, then records
    /// the error as a [`Role::Error`] message, and clears the streaming state.
    /// Returns the [`StreamError`] for the event loop to flush to scrollback, or
    /// `None` if no reply was in progress.
    pub fn fail_stream(&mut self, error: &str) -> Option<StreamError> {
        let streamed = self.streaming.take()?;
        // An error is the turn's terminal state — clear the live status without a
        // "Done" summary; the red error notice is the summary.
        self.status = None;
        let timestamp = self.now_stamp();
        let partial = if streamed.is_empty() {
            None
        } else {
            self.history.push(HistoryItem::Message(Message {
                role: Role::Assistant,
                text: streamed.clone(),
                timestamp: timestamp.clone(),
            }));
            Some(streamed)
        };
        self.history.push(HistoryItem::Message(Message {
            role: Role::Error,
            text: error.to_string(),
            timestamp,
        }));
        Some(StreamError {
            partial,
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
            let timestamp = self.now_stamp();
            self.history.push(HistoryItem::Message(Message {
                role: Role::Assistant,
                text: text.clone(),
                timestamp,
            }));
        }
        let tool = self.end_tool(INTERRUPT_TOOL_OUTPUT, false);
        let timestamp = self.now_stamp();
        self.history.push(HistoryItem::Message(Message {
            role: Role::Error,
            text: INTERRUPT_NOTICE.to_string(),
            timestamp,
        }));
        self.status = None;
        Some(InterruptedTurn { partial, tool })
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
        let mut app = App::new();
        app.input = TextArea::from_text("hello");
        app.begin_stream();
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::None);
        assert_eq!(app.input.text(), "hello");
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
        let mut app = App::new();
        type_str(&mut app, "/cl");
        assert!(app.command_menu.is_some(), "still a command token");
        assert_eq!(matching_commands("cl").len(), 1, "only /clear matches");
        assert_eq!(app.command_menu.as_ref().unwrap().selected, 0, "clamped");
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
}
