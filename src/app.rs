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
    /// The user asked to quit.
    Quit,
}

/// All mutable conversation state: the editable input line, the reply currently
/// being streamed, and the finished-message history.
///
/// Finished messages are shown via the terminal's scrollback, but [`history`]
/// keeps them so they can be repainted when a resize clears the screen.
///
/// [`history`]: App::history
#[derive(Debug, Default)]
pub struct App {
    /// The text the user is currently typing.
    pub input: String,
    /// `Some(buffer)` while the AI reply is streaming, accumulating chunks.
    pub streaming: Option<String>,
    /// Every finished message, oldest first — used to repaint after a resize.
    pub history: Vec<Message>,
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
    /// Sending is disabled while a reply streams; quitting always works.
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C quits regardless of which key character it is paired with.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        match key.code {
            KeyCode::Esc => Action::Quit,
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
                self.input.pop();
                Action::None
            }
            KeyCode::Char(c) => {
                self.input.push(c);
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Record a finished user message in the history.
    pub fn record_user_message(&mut self, text: &str) {
        self.history.push(Message {
            role: Role::User,
            text: text.to_string(),
        });
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

    /// Finish streaming, returning the completed reply text (if any), recording
    /// it in the history, and clearing the streaming state.
    pub fn finish_stream(&mut self) -> Option<String> {
        let text = self.streaming.take()?;
        self.history.push(Message {
            role: Role::Assistant,
            text: text.clone(),
        });
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
            self.history.push(Message {
                role: Role::Assistant,
                text: streamed.clone(),
            });
            Some(streamed)
        };
        self.history.push(Message {
            role: Role::Error,
            text: error.to_string(),
        });
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
        assert_eq!(app.history[0].role, Role::User);
        assert_eq!(app.history[0].text, "hello");
    }

    #[test]
    fn finish_stream_records_the_assistant_message_in_history() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("hi there");
        app.finish_stream();
        assert_eq!(
            app.history.last(),
            Some(&Message {
                role: Role::Assistant,
                text: "hi there".to_string(),
            })
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
        let roles: Vec<Role> = app.history.iter().map(|m| m.role).collect();
        assert_eq!(roles, vec![Role::User, Role::Assistant]);
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
        let roles: Vec<Role> = app.history.iter().map(|m| m.role).collect();
        assert_eq!(roles, vec![Role::Assistant, Role::Error]);
        assert_eq!(app.history[1].text, "network down");
    }

    #[test]
    fn fail_stream_with_no_partial_records_only_the_error() {
        let mut app = App::new();
        app.begin_stream(); // errored before any chunk arrived
        let failure = app.fail_stream("died early").expect("was streaming");
        assert!(failure.partial.is_none());
        let roles: Vec<Role> = app.history.iter().map(|m| m.role).collect();
        assert_eq!(roles, vec![Role::Error]);
    }

    #[test]
    fn fail_stream_when_idle_returns_none_and_records_nothing() {
        let mut app = App::new();
        assert!(app.fail_stream("ignored").is_none());
        assert!(app.history.is_empty());
    }
}
