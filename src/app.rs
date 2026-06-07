//! Conversation state and the pure update logic that drives it.
//!
//! Everything here is free of I/O so it can be unit-tested directly: the event
//! loop in `main.rs` feeds key presses in and reacts to the returned
//! [`Action`]s, and pushes streamed chunks in via [`App::push_chunk`].

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
}

/// One ordered entry of finished conversation history: a [`Message`] or a
/// [`ToolCall`]. They share a single ordered list so the inline conversation
/// repaints (after a resize, or when returning from the tool-output view) in the
/// exact order things streamed — assistant text and tool calls interleaved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryItem {
    Message(Message),
    Tool(ToolCall),
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
    /// The text the user is currently typing.
    pub input: String,
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

    /// Handle one key press and report what the event loop should do.
    ///
    /// Sending is disabled while a reply streams; quitting (Ctrl+C) and the
    /// tool-view toggle (Ctrl+O) always work, from either screen. Other keys are
    /// dispatched to the active [`View`].
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C quits regardless of which key character it is paired with.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
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
            // Esc dismisses the palette (and does *not* quit) when it's open;
            // otherwise it quits as before.
            KeyCode::Esc if menu_open => {
                self.command_menu = None;
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
            // newline so the input box grows on demand; a plain Enter submits.
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.input.push('\n');
                Action::None
            }
            KeyCode::Enter => {
                if self.is_streaming() || self.input.trim().is_empty() {
                    Action::None
                } else {
                    Action::Submit(std::mem::take(&mut self.input))
                }
            }
            KeyCode::Backspace => {
                let had_query = command_query(&self.input).is_some();
                self.input.pop();
                self.refresh_command_menu(had_query);
                Action::None
            }
            KeyCode::Char(c) => {
                let had_query = command_query(&self.input).is_some();
                self.input.push(c);
                self.refresh_command_menu(had_query);
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Re-derive the palette after an edit. Opens it when the input *becomes* a
    /// command token, clamps the highlight when the filter narrows, and closes it
    /// when the input stops being a command token. The `had_query` flag (the state
    /// *before* the edit) makes Esc sticky: once dismissed, editing within the same
    /// token won't reopen the palette — only leaving and re-entering command mode
    /// (a None→Some transition) does.
    fn refresh_command_menu(&mut self, had_query: bool) {
        match command_query(&self.input) {
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
        let Some(query) = command_query(&self.input) else {
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
        let query = command_query(&self.input)?;
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
        self.history.push(HistoryItem::Message(Message {
            role: Role::User,
            text: text.to_string(),
        }));
    }

    /// Record a system notice (from a slash command) in the history, so it
    /// repaints on resize like any other message. The loop also commits it to
    /// scrollback. Mirrors [`record_user_message`] for [`Action::Notice`].
    pub fn record_system_message(&mut self, text: &str) {
        self.history.push(HistoryItem::Message(Message {
            role: Role::System,
            text: text.to_string(),
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
        self.history.push(HistoryItem::Message(Message {
            role: Role::Assistant,
            text: text.clone(),
        }));
        Some(text)
    }

    /// Begin a reply: open an empty streaming buffer so the live region can
    /// show the assistant is responding even before the first chunk arrives.
    pub fn begin_stream(&mut self) {
        self.streaming = Some(String::new());
    }

    /// Append a streamed chunk to the in-progress reply. No-op if not streaming.
    pub fn push_chunk(&mut self, chunk: &str) {
        if let Some(buf) = self.streaming.as_mut() {
            buf.push_str(chunk);
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
        self.history.push(HistoryItem::Message(Message {
            role: Role::Assistant,
            text: text.clone(),
        }));
        Some(text)
    }

    /// End the in-progress stream because the backend reported an error.
    ///
    /// Records any non-empty partial reply as an assistant message, then records
    /// the error as a [`Role::Error`] message, and clears the streaming state.
    /// Returns the [`StreamError`] for the event loop to flush to scrollback, or
    /// `None` if no reply was in progress.
    pub fn fail_stream(&mut self, error: &str) -> Option<StreamError> {
        let streamed = self.streaming.take()?;
        let partial = if streamed.is_empty() {
            None
        } else {
            self.history.push(HistoryItem::Message(Message {
                role: Role::Assistant,
                text: streamed.clone(),
            }));
            Some(streamed)
        };
        self.history.push(HistoryItem::Message(Message {
            role: Role::Error,
            text: error.to_string(),
        }));
        Some(StreamError {
            partial,
            error: error.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// The roles of the `Message` items in history, in order (tool calls skipped).
    fn roles(app: &App) -> Vec<Role> {
        app.history
            .iter()
            .filter_map(|item| match item {
                HistoryItem::Message(m) => Some(m.role),
                HistoryItem::Tool(_) => None,
            })
            .collect()
    }

    /// The `Message` at history index `i` (panics if it is a tool call).
    fn message_at(app: &App, i: usize) -> &Message {
        match &app.history[i] {
            HistoryItem::Message(m) => m,
            HistoryItem::Tool(_) => panic!("expected a message at history[{i}]"),
        }
    }

    #[test]
    fn typing_appends_characters_to_input() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('h')));
        app.on_key(key(KeyCode::Char('i')));
        assert_eq!(app.input, "hi");
    }

    #[test]
    fn backspace_removes_last_character() {
        let mut app = App::new();
        app.input = "hi".to_string();
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.input, "h");
    }

    #[test]
    fn backspace_on_empty_input_is_harmless() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.input, "");
    }

    #[test]
    fn enter_with_text_submits_and_clears_input() {
        let mut app = App::new();
        app.input = "hello".to_string();
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Submit("hello".to_string()));
        assert_eq!(app.input, "");
    }

    #[test]
    fn enter_with_blank_input_does_nothing() {
        let mut app = App::new();
        app.input = "   ".to_string();
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::None);
        // whitespace-only input is left untouched
        assert_eq!(app.input, "   ");
    }

    #[test]
    fn enter_while_streaming_does_not_submit() {
        let mut app = App::new();
        app.input = "hello".to_string();
        app.begin_stream();
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::None);
        assert_eq!(app.input, "hello");
    }

    #[test]
    fn alt_enter_inserts_a_newline_instead_of_submitting() {
        let mut app = App::new();
        app.input = "line one".to_string();
        let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
        assert_eq!(app.on_key(alt_enter), Action::None);
        assert_eq!(app.input, "line one\n", "Alt+Enter appends a newline");
    }

    #[test]
    fn shift_enter_inserts_a_newline_too() {
        // Terminals with enhanced keyboard support report Shift+Enter; treat it as
        // a newline like Alt+Enter so the box grows on demand.
        let mut app = App::new();
        app.input = "a".to_string();
        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(app.on_key(shift_enter), Action::None);
        assert_eq!(app.input, "a\n");
    }

    #[test]
    fn plain_enter_submits_a_multi_line_message_intact() {
        // After Alt+Enter newlines, a plain Enter submits the whole thing.
        let mut app = App::new();
        app.input = "first\nsecond".to_string();
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Submit("first\nsecond".to_string()));
        assert_eq!(app.input, "");
    }

    #[test]
    fn alt_enter_grows_input_while_streaming_without_submitting() {
        // Editing (incl. newlines) is allowed mid-stream; only sending is blocked.
        let mut app = App::new();
        app.input = "draft".to_string();
        app.begin_stream();
        let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
        assert_eq!(app.on_key(alt_enter), Action::None);
        assert_eq!(app.input, "draft\n");
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
        assert_eq!(app.input, "", "the read-only viewer swallows typing");
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
        assert_eq!(app.input, "/zzz", "the input is left intact");
    }

    #[test]
    fn enter_still_submits_a_normal_message_when_no_palette_is_open() {
        let mut app = App::new();
        app.input = "hello".to_string();
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
            }))
        );
    }
}
