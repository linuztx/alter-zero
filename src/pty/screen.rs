//! A terminal's output as **a screen** — what a person would see
//! (`docs/interactive-shell.md`).
//!
//! [`Screen`] is the `vt100` emulator at the pseudo-terminal's size with no
//! scrollback (≈150 KB a session). It is the view a full-screen program is
//! shown through (the model gets its rows when it runs on the alternate
//! screen, the ↓ manager always), and it holds the terminal state input
//! depends on: the cursor-key mode arrows are encoded in, and where the
//! cursor sits — which is how a call tells a prompt waiting for an answer
//! from a program between two lines of output.
//!
//! It also **answers terminal queries**. A real terminal replies to a
//! cursor-position report (`ESC [ 6 n`), device attributes (`ESC [ c`), the
//! colour queries and a few more; programs such as `vim` and `prompt_toolkit`
//! REPLs ask, and some wait a long time for an answer a pipe never sends.
//! [`Screen::feed`] hands the replies back, in order, for the session to
//! write to the program — computed at the moment each query arrives, so a
//! cursor report names the cursor as it was *then*.

/// The pseudo-terminal's size: 120 columns — wide enough that ordinary
/// output rarely wraps — by 40 rows, tall enough for a full-screen program to
/// show something useful and short enough that its screen is a modest read.
pub const COLUMNS: u16 = 120;
/// See [`COLUMNS`].
pub const ROWS: u16 = 40;

/// The screen at one moment, as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Each row right-trimmed; trailing blank rows dropped.
    pub rows: Vec<String>,
    /// The cursor, 1-based `(line, column)` — how a person would name it.
    pub cursor: (u16, u16),
    /// The screen's size, `(rows, columns)`.
    pub size: (u16, u16),
    /// Was the program on the alternate screen?
    pub alternate: bool,
}

/// See the module docs.
pub struct Screen {
    parser: vt100::Parser<Replies>,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screen").finish_non_exhaustive()
    }
}

impl Default for Screen {
    fn default() -> Self {
        Self::new(ROWS, COLUMNS)
    }
}

impl Screen {
    #[must_use]
    pub fn new(rows: u16, columns: u16) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(rows, columns, 0, Replies::default()),
        }
    }

    /// Fold a chunk of output in; returns what the terminal owes the program
    /// back (query replies), in the order the queries arrived.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.process(bytes);
        std::mem::take(&mut self.parser.callbacks_mut().pending)
    }

    /// The screen as text.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let screen = self.parser.screen();
        let (rows, columns) = screen.size();
        let mut lines: Vec<String> = screen
            .rows(0, columns)
            .map(|row| row.trim_end().to_string())
            .collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        let (row, col) = screen.cursor_position();
        Snapshot {
            rows: lines,
            cursor: (row + 1, col + 1),
            size: (rows, columns),
            alternate: screen.alternate_screen(),
        }
    }

    /// The row the cursor is on, right-trimmed — the prompt, when the
    /// program is sitting at one.
    #[must_use]
    pub fn cursor_line(&self) -> String {
        let screen = self.parser.screen();
        let (row, _) = screen.cursor_position();
        let (_, columns) = screen.size();
        screen
            .rows(0, columns)
            .nth(usize::from(row))
            .map(|line| line.trim_end().to_string())
            .unwrap_or_default()
    }

    /// Is the program on the alternate screen?
    #[must_use]
    pub fn alternate(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    /// The program's cursor-key mode (DECCKM) — how arrow keys are encoded.
    #[must_use]
    pub fn application_cursor(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    /// Does the screen look like it is **waiting for keys**? A full-screen
    /// program always is when it goes quiet; otherwise a cursor left past
    /// column 1 is a prompt (`>>> `, `Password: `, `[Y/n] `) — a program
    /// between lines of output leaves it at the start of a fresh line.
    #[must_use]
    pub fn awaiting_keys(&self) -> bool {
        let screen = self.parser.screen();
        screen.alternate_screen() || screen.cursor_position().1 > 0
    }
}

/// The query replies [`Screen::feed`] collects, filled in by `vt100` as it
/// meets sequences it does not act on itself.
#[derive(Default)]
struct Replies {
    pending: Vec<u8>,
}

impl Replies {
    fn reply(&mut self, text: &str) {
        self.pending.extend_from_slice(text.as_bytes());
    }
}

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        let (row, col) = screen.cursor_position();
        match (i1, i2, c) {
            // Device status: "OK", and the cursor position report.
            (None, None, 'n') if first == 5 => self.reply("\x1b[0n"),
            (None, None, 'n') if first == 6 => {
                self.reply(&format!("\x1b[{};{}R", row + 1, col + 1));
            }
            (Some(b'?'), None, 'n') if first == 6 => {
                self.reply(&format!("\x1b[?{};{}R", row + 1, col + 1));
            }
            // Primary device attributes: a VT100 with advanced video — the
            // answer every program accepts. Secondary: a generic terminal,
            // version 0, so nothing is enabled on the strength of a version.
            (None, None, 'c') if first == 0 => self.reply("\x1b[?1;2c"),
            (Some(b'>'), None, 'c') if first == 0 => self.reply("\x1b[>0;0;0c"),
            // The text area's size in characters.
            (None, None, 't') if first == 18 => {
                let (rows, columns) = screen.size();
                self.reply(&format!("\x1b[8;{rows};{columns}t"));
            }
            // A mode request: "not recognized", so the program keeps its
            // default rather than waiting on a feature it asked about.
            (Some(b'?'), Some(b'$'), 'p') => self.reply(&format!("\x1b[?{first};0$y")),
            (Some(b'$'), None, 'p') => self.reply(&format!("\x1b[{first};0$y")),
            _ => {}
        }
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        // The colour queries: light text on a dark background, the terminal
        // an agent is most likely to be sitting in.
        match params {
            [b"10", b"?"] => self.reply("\x1b]10;rgb:ffff/ffff/ffff\x1b\\"),
            [b"11", b"?"] => self.reply("\x1b]11;rgb:0000/0000/0000\x1b\\"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Screen {
        Screen::new(5, 20)
    }

    #[test]
    fn the_cursor_line_is_the_row_the_cursor_sits_on() {
        let mut s = screen();
        s.feed(b"one\r\nFull name: ");
        assert_eq!(s.cursor_line(), "Full name:");
        s.feed(b"Ada\r\n");
        assert_eq!(s.cursor_line(), "", "a fresh line");
    }

    #[test]
    fn text_lands_on_the_rows_it_was_written_to() {
        let mut s = screen();
        s.feed(b"hello\r\nworld   \r\n");
        let snap = s.snapshot();
        assert_eq!(
            snap.rows,
            vec!["hello", "world"],
            "right-trimmed, blank tail dropped"
        );
        assert_eq!(snap.cursor, (3, 1));
        assert_eq!(snap.size, (5, 20));
        assert!(!snap.alternate);
    }

    #[test]
    fn inner_blank_rows_are_kept() {
        let mut s = screen();
        s.feed(b"top\x1b[4;1Hbottom");
        assert_eq!(s.snapshot().rows, vec!["top", "", "", "bottom"]);
    }

    #[test]
    fn the_alternate_screen_is_its_own_canvas() {
        let mut s = screen();
        s.feed(b"shell$ ");
        s.feed(b"\x1b[?1049h\x1b[H\x1b[2J~\r\n~\x1b[5;1H-- INSERT --");
        assert!(s.alternate());
        let snap = s.snapshot();
        assert!(snap.alternate);
        assert_eq!(snap.rows, vec!["~", "~", "", "", "-- INSERT --"]);
        s.feed(b"\x1b[?1049l");
        assert!(!s.alternate());
        assert_eq!(
            s.snapshot().rows,
            vec!["shell$"],
            "the main screen comes back"
        );
    }

    #[test]
    fn a_cursor_position_report_names_the_cursor_where_it_was_asked() {
        let mut s = screen();
        let replies = s.feed(b"ab\x1b[6ncd");
        assert_eq!(replies, b"\x1b[1;3R", "row 1, column 3 at the query");
        let replies = s.feed(b"\r\n\x1b[?6n");
        assert_eq!(replies, b"\x1b[?2;1R", "the DEC form keeps its marker");
    }

    #[test]
    fn device_status_and_attribute_queries_are_answered() {
        let mut s = screen();
        assert_eq!(s.feed(b"\x1b[5n"), b"\x1b[0n");
        assert_eq!(s.feed(b"\x1b[c"), b"\x1b[?1;2c");
        assert_eq!(s.feed(b"\x1b[0c"), b"\x1b[?1;2c");
        assert_eq!(s.feed(b"\x1b[>c"), b"\x1b[>0;0;0c");
    }

    #[test]
    fn size_colour_and_mode_queries_are_answered() {
        let mut s = screen();
        assert_eq!(s.feed(b"\x1b[18t"), b"\x1b[8;5;20t");
        assert_eq!(
            s.feed(b"\x1b]10;?\x07"),
            b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\"
        );
        assert_eq!(
            s.feed(b"\x1b]11;?\x1b\\"),
            b"\x1b]11;rgb:0000/0000/0000\x1b\\"
        );
        assert_eq!(
            s.feed(b"\x1b[?2026$p"),
            b"\x1b[?2026;0$y",
            "a mode request is answered 'not recognized'"
        );
    }

    #[test]
    fn several_queries_in_one_chunk_are_answered_in_order() {
        let mut s = screen();
        let replies = s.feed(b"\x1b[5nx\x1b[6n");
        assert_eq!(replies, b"\x1b[0n\x1b[1;2R");
        assert!(s.feed(b"plain").is_empty(), "no query, no reply");
    }

    #[test]
    fn application_cursor_mode_follows_decckm() {
        let mut s = screen();
        assert!(!s.application_cursor());
        s.feed(b"\x1b[?1h");
        assert!(s.application_cursor());
        s.feed(b"\x1b[?1l");
        assert!(!s.application_cursor());
    }

    #[test]
    fn a_cursor_past_the_first_column_or_a_full_screen_program_awaits_keys() {
        let mut s = screen();
        assert!(!s.awaiting_keys(), "a blank screen with the cursor home");
        s.feed(b">>> ");
        assert!(s.awaiting_keys(), "a prompt");
        s.feed(b"\r\n");
        assert!(!s.awaiting_keys(), "the start of a fresh line");
        s.feed(b"\x1b[?1049h\x1b[H");
        assert!(s.awaiting_keys(), "a full-screen program, cursor anywhere");
    }
}
