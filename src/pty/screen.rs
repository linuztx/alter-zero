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
//!
//! Three things full-screen programs rely on that `vt100` leaves out are
//! done **in front of it** (`Prepass`): the DEC line-drawing set
//! ([`super::charset`]) a box is drawn in without a UTF-8 locale, REP
//! (`CSI n b`) — ncurses repeats a character that way for any run of it, so
//! without it indentation and the spaces between columns vanished and the
//! rest of the row slid left — and insert mode (`CSI 4 h`).

use unicode_width::UnicodeWidthChar as _;

use super::charset::Charsets;

/// The pseudo-terminal's size: 120 columns — wide enough that ordinary
/// output rarely wraps — by 40 rows, tall enough for a full-screen program to
/// show something useful and short enough that its screen is a modest read.
pub const COLUMNS: u16 = 120;
/// See [`COLUMNS`].
pub const ROWS: u16 = 40;

/// How far past the end of its row's text the cursor may sit and still hold
/// that text ([`Screen::holds_at_cursor`]) — the spaces typed after it.
const HELD_SLACK: usize = 2;

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
    /// What the screen **highlights**, as `(line, text)` — 1-based lines,
    /// top to bottom, at most [`MAX_HIGHLIGHTS`] and one more to say there
    /// were more: text on a background (or in reverse video) that neither
    /// the row above nor the row below has under it. A menu's selection, a
    /// focused button, a table's header — what a curses program shows in
    /// colour alone, which the rows' text cannot say.
    pub highlights: Vec<(u16, String)>,
}

/// The most highlights a report names ([`Snapshot::highlights`]).
pub const MAX_HIGHLIGHTS: usize = 8;

/// See the module docs.
pub struct Screen {
    parser: vt100::Parser<Replies>,
    /// What `vt100` does not implement, applied before it sees the bytes.
    prepass: Prepass,
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
            prepass: Prepass::new(usize::from(rows) * usize::from(columns)),
        }
    }

    /// Fold a chunk of output in; returns what the terminal owes the program
    /// back (query replies), in the order the queries arrived.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        let bytes = self.prepass.rewrite(bytes);
        self.parser.process(&bytes);
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
            // A cursor waiting to wrap sits past the last column; a person
            // would name the last column.
            cursor: (row + 1, col.min(columns.saturating_sub(1)) + 1),
            size: (rows, columns),
            alternate: screen.alternate_screen(),
            highlights: highlights(screen),
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

    /// Does the cursor's row end with `typed`, the cursor **right after** it
    /// — a line editor holding what it echoed until its Enter? Within
    /// `HELD_SLACK` columns, for trailing spaces typed after it. A row that
    /// merely ends the same way, the cursor parked elsewhere (a key typed
    /// into `top`), holds nothing.
    #[must_use]
    pub fn holds_at_cursor(&self, typed: &str) -> bool {
        use unicode_width::UnicodeWidthStr as _;
        let line = self.cursor_line();
        let (_, col) = self.parser.screen().cursor_position();
        let end = line.width();
        !typed.is_empty()
            && line.ends_with(typed)
            && (end..=end + HELD_SLACK).contains(&usize::from(col))
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

    /// The modes the program set that change what its keys look like
    /// ([`super::keys::encode`]): cursor-key mode, bracketed paste.
    #[must_use]
    pub fn modes(&self) -> super::keys::Modes {
        let screen = self.parser.screen();
        super::keys::Modes {
            app_cursor: screen.application_cursor(),
            bracketed_paste: screen.bracketed_paste(),
        }
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

/// How many blank columns may part two highlighted runs and still leave
/// them one — dialog pads an item's parts apart inside its selection.
const HIGHLIGHT_GAP: u16 = 3;

/// More separate highlights than this on one row make it a legend — nano's
/// `^G Help  ^O Write Out`, a function-key bar — rather than a selection.
const LEGEND_RUNS: usize = 3;

/// The longest text one highlight is named by, in characters.
const HIGHLIGHT_MAX_CHARS: usize = 80;

/// The background a cell shows — its colour, or its text's colour in
/// reverse video — which is what makes a selection stand out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shade {
    /// The terminal's own background.
    Background,
    /// The terminal's own foreground, shown as background (reverse video).
    Foreground,
    Color(vt100::Color),
}

fn shade(screen: &vt100::Screen, row: u16, col: u16) -> Shade {
    let Some(cell) = screen.cell(row, col) else {
        return Shade::Background;
    };
    let (color, default) = if cell.inverse() {
        (cell.fgcolor(), Shade::Foreground)
    } else {
        (cell.bgcolor(), Shade::Background)
    };
    match color {
        vt100::Color::Default => default,
        color => Shade::Color(color),
    }
}

/// The screen's highlights ([`Snapshot::highlights`]): on each row, the runs
/// of one [`Shade`] — never the terminal's own background — that stand out
/// from the row above **and** the row below, each judged whole: most of the
/// cells above it and most of those below it differ. A one-row selection
/// bar stands out; the box or the backdrop it sits on does not, and a tab
/// over the start of a header does not cut the header short. Joined when
/// they touch or part by a short blank gap, named by their text; runs with
/// no letter or digit in them, and rows holding more than [`LEGEND_RUNS`]
/// runs, left out.
fn highlights(screen: &vt100::Screen) -> Vec<(u16, String)> {
    let (rows, columns) = screen.size();
    let mut found = Vec::new();
    if rows < 2 {
        return found;
    }
    for row in 0..rows {
        // Does most of `[start, end)` on `other` differ from `here`? A row
        // off the screen differs.
        let differs = |other: Option<u16>, start: u16, end: u16, here: Shade| {
            other.is_none_or(|other| {
                let unlike = (start..end)
                    .filter(|&col| shade(screen, other, col) != here)
                    .count();
                unlike * 2 > usize::from(end - start)
            })
        };
        let blank = |col: u16| {
            screen
                .cell(row, col)
                .is_none_or(|cell| cell.contents().trim().is_empty())
        };
        let below = (row + 1 < rows).then_some(row + 1);
        // The runs that stand out, as `[start, end)` columns.
        let mut runs: Vec<(u16, u16)> = Vec::new();
        let mut col = 0;
        while col < columns {
            let here = shade(screen, row, col);
            let start = col;
            while col < columns && shade(screen, row, col) == here {
                col += 1;
            }
            if here == Shade::Background
                || !differs(row.checked_sub(1), start, col, here)
                || !differs(below, start, col, here)
            {
                continue;
            }
            match runs.last_mut() {
                Some((_, end)) if *end == start => *end = col,
                Some((_, end)) if start - *end <= HIGHLIGHT_GAP && (*end..start).all(&blank) => {
                    *end = col;
                }
                _ => runs.push((start, col)),
            }
        }
        // A run naming nothing — blank, or a stretch of border caught
        // between two colours — is no highlight worth a word.
        let named: Vec<String> = runs
            .iter()
            .map(|&(start, end)| run_text(screen, row, start, end))
            .filter(|text| text.chars().any(char::is_alphanumeric))
            .collect();
        if named.len() > LEGEND_RUNS {
            continue;
        }
        for text in named {
            found.push((row + 1, text));
            if found.len() > MAX_HIGHLIGHTS {
                return found;
            }
        }
    }
    found
}

/// The text of `row`'s cells `[start, end)`, trimmed, and cut at
/// [`HIGHLIGHT_MAX_CHARS`].
fn run_text(screen: &vt100::Screen, row: u16, start: u16, end: u16) -> String {
    let mut text = String::new();
    for col in start..end {
        match screen.cell(row, col) {
            Some(cell) if cell.is_wide_continuation() => {}
            Some(cell) if cell.has_contents() => text.push_str(cell.contents()),
            _ => text.push(' '),
        }
    }
    let text = text.trim();
    match text.char_indices().nth(HIGHLIGHT_MAX_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

/// What `vt100` leaves out that full-screen programs use, applied to the
/// byte stream on its way in (see the module docs): the line-drawing set,
/// REP, and insert mode. Everything else passes through byte for byte.
///
/// It follows the stream with a `vte` parser of its own, a byte at a time,
/// so it knows which bytes make up each character: a character read whole is
/// written anew (translated, repeated, or made room for), and every other
/// byte — sequences, controls, text too malformed to be sure of — is handed
/// on exactly as it came, for the emulator to make of it what it always did.
struct Prepass {
    parser: vte::Parser,
    state: PrepassState,
    /// Bytes read since the last character or sequence completed.
    pending: Vec<u8>,
    /// The most characters one repeat writes: a screenful.
    max_repeat: usize,
}

/// What the [`Prepass`]'s parser tracks.
#[derive(Default)]
struct PrepassState {
    charsets: Charsets,
    /// Insert mode (IRM) is on.
    insert: bool,
    /// The last character printed, as shown — what REP repeats.
    last: Option<char>,
    /// What the byte just read completed.
    events: Vec<Event>,
    /// The parser is known to be between characters and sequences: a
    /// character or a sequence was the last thing completed, and nothing
    /// has been read since that completed nothing. Only then is printable
    /// ASCII sure to be printed text — mid-sequence it is parameters, and a
    /// control executed inside a sequence (`CSI 4 BEL h`) completes an
    /// event without ending it.
    ground: bool,
}

/// One thing the parser completed.
#[derive(Debug, Clone, Copy)]
enum Event {
    Print(char),
    /// REP: the last character, this many times more.
    Repeat(usize),
    /// Any other control or sequence.
    Other,
}

impl Prepass {
    fn new(max_repeat: usize) -> Self {
        Self {
            parser: vte::Parser::new(),
            state: PrepassState {
                ground: true,
                ..PrepassState::default()
            },
            pending: Vec::new(),
            max_repeat,
        }
    }

    /// `bytes` as the emulator should see them.
    fn rewrite(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        let mut at = 0;
        while at < bytes.len() {
            // Plain text between sequences, with nothing to translate or make
            // room for, goes through as it is: the parser would print each
            // byte and stay where it is.
            if self.state.ground && !self.state.insert && !self.state.charsets.translates() {
                let run = bytes[at..]
                    .iter()
                    .take_while(|&&b| (0x20..0x7f).contains(&b))
                    .count();
                if run > 0 {
                    out.extend_from_slice(&bytes[at..at + run]);
                    self.state.last = Some(char::from(bytes[at + run - 1]));
                    at += run;
                    continue;
                }
            }
            let byte = bytes[at];
            at += 1;
            self.pending.push(byte);
            self.parser
                .advance(&mut self.state, std::slice::from_ref(&byte));
            let whole = match self.state.events.as_slice() {
                [] => {
                    self.state.ground = false;
                    continue;
                }
                &[Event::Print(c)] if encodes(c, &self.pending) => Some(Event::Print(c)),
                &[Event::Repeat(n)] => Some(Event::Repeat(n)),
                _ => None,
            };
            match whole {
                Some(Event::Print(c)) => {
                    let shown = self.state.charsets.map(c);
                    self.state.last = Some(shown);
                    self.write_char(&mut out, shown);
                    self.pending.clear();
                }
                Some(Event::Repeat(n)) => {
                    // The REP itself is nothing to the emulator; what it
                    // stands for is written after it.
                    out.append(&mut self.pending);
                    if let Some(c) = self.state.last {
                        for _ in 0..n.min(self.max_repeat) {
                            self.write_char(&mut out, c);
                        }
                    }
                }
                _ => {
                    if let Some(c) = self
                        .state
                        .events
                        .iter()
                        .rev()
                        .find_map(|event| match event {
                            Event::Print(c) => Some(*c),
                            _ => None,
                        })
                    {
                        self.state.last = Some(c);
                    }
                    out.append(&mut self.pending);
                }
            }
            self.state.events.clear();
        }
        out
    }

    /// Write `c` — in insert mode after making room for it, as the terminal
    /// would (ICH, which the emulator implements).
    fn write_char(&self, out: &mut Vec<u8>, c: char) {
        let width = c.width().unwrap_or(0);
        if self.state.insert && width > 0 {
            out.extend_from_slice(format!("\x1b[{width}@").as_bytes());
        }
        let mut buf = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    }
}

/// Are `bytes` exactly `c`'s UTF-8 — a character read whole, with nothing
/// malformed or unfinished in front of it?
fn encodes(c: char, bytes: &[u8]) -> bool {
    let mut buf = [0u8; 4];
    c.encode_utf8(&mut buf).as_bytes() == bytes
}

impl vte::Perform for PrepassState {
    fn print(&mut self, c: char) {
        self.ground = true;
        self.events.push(Event::Print(c));
    }

    fn execute(&mut self, byte: u8) {
        self.charsets.execute(byte);
        self.events.push(Event::Other);
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        self.ground = true;
        if intermediates.is_empty() {
            match action {
                'b' => {
                    let count = params
                        .iter()
                        .next()
                        .and_then(|p| p.first().copied())
                        .unwrap_or(1)
                        .max(1);
                    self.events.push(Event::Repeat(usize::from(count)));
                    return;
                }
                'h' | 'l' if params.iter().any(|p| p.first() == Some(&4)) => {
                    self.insert = action == 'h';
                }
                _ => {}
            }
        }
        self.events.push(Event::Other);
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        self.ground = true;
        match intermediates {
            [slot] => {
                self.charsets.designate(*slot, byte);
            }
            // A full reset (RIS) resets these too.
            [] if byte == b'c' => {
                self.charsets = Charsets::default();
                self.insert = false;
            }
            _ => {}
        }
        self.events.push(Event::Other);
    }

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {
        self.ground = true;
        self.events.push(Event::Other);
    }

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _c: char) {
        self.ground = false;
        self.events.push(Event::Other);
    }

    fn put(&mut self, _byte: u8) {
        self.ground = false;
        self.events.push(Event::Other);
    }

    fn unhook(&mut self) {
        self.ground = true;
        self.events.push(Event::Other);
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
    fn a_box_drawn_in_the_line_drawing_set_reads_as_a_box() {
        // What dialog, whiptail and mc draw with no UTF-8 locale: letters in
        // the DEC Special Graphics set, not `lqqk`.
        let mut s = screen();
        s.feed(b"\x1b(0lqqk\r\nx  x\r\nmqqj\x1b(B ok");
        assert_eq!(s.snapshot().rows, vec!["┌──┐", "│  │", "└──┘ ok"]);
    }

    #[test]
    fn the_line_drawing_set_parked_in_g1_shows_while_shifted_in() {
        let mut s = screen();
        s.feed(b"\x1b)0a\x0eqq\x0fb");
        assert_eq!(s.snapshot().rows, vec!["a──b"]);
    }

    #[test]
    fn a_repeat_writes_the_last_character_again() {
        // REP (`CSI n b`), which ncurses sends for any run of one character
        // — indentation, the gaps between columns, a rule. Dropped, the rest
        // of the row slid left over it.
        let mut s = screen();
        s.feed(b"a\x1b[4b|");
        assert_eq!(s.snapshot().rows, vec!["aaaaa|"]);
        let mut s = screen();
        s.feed(b"\x1b(0q\x1b[5b\x1b(B!");
        assert_eq!(s.snapshot().rows, vec!["──────!"], "a repeated rule");
    }

    #[test]
    fn a_repeat_keeps_the_gap_it_stands_for() {
        // Recorded from nano redrawing its shortcut bar: a space and five
        // more, then the next label.
        let mut s = Screen::new(3, 40);
        s.feed(b"M-U Undo \x1b[6bM-A Set Mark");
        assert_eq!(s.snapshot().rows, vec!["M-U Undo       M-A Set Mark"]);
    }

    #[test]
    fn a_repeat_never_runs_past_the_screen() {
        let mut s = screen();
        s.feed(b"x\x1b[65535b");
        let rows = s.snapshot().rows;
        assert_eq!(rows.len(), 5, "{rows:?}");
        assert!(rows.iter().all(|row| row.chars().all(|c| c == 'x')));
    }

    #[test]
    fn insert_mode_pushes_the_line_right() {
        // IRM (`CSI 4 h`): what is written moves the rest of the line along
        // instead of overwriting it.
        let mut s = screen();
        s.feed(b"world\r\x1b[4hhello \x1b[4lX");
        assert_eq!(s.snapshot().rows, vec!["hello Xorld"]);
    }

    #[test]
    fn a_highlighted_menu_item_is_named() {
        // A curses menu shows its selection in reverse video and nothing
        // else — the text of the three rows is the same whichever is picked.
        let mut s = screen();
        s.feed(b"  apple\r\n\x1b[7m  banana\x1b[m\r\n  cherry");
        assert_eq!(s.snapshot().highlights, vec![(2, "banana".to_string())]);
    }

    #[test]
    fn only_what_stands_out_from_the_rows_around_it_is_highlighted() {
        // whiptail: a coloured box, the selected item a colour of its own.
        // The box is the background here, not a highlight.
        let mut s = screen();
        s.feed(b"\x1b[44m        \r\n  \x1b[41mitem\x1b[44m  \r\n        \x1b[m");
        assert_eq!(s.snapshot().highlights, vec![(2, "item".to_string())]);
        let mut s = screen();
        s.feed(b"\x1b[44m        \r\n        \r\n        \x1b[m");
        assert!(s.snapshot().highlights.is_empty(), "a plain box");
    }

    #[test]
    fn a_row_of_many_short_highlights_is_a_legend_not_a_selection() {
        // nano's shortcut rows, htop's function-key bar.
        let mut s = Screen::new(3, 40);
        s.feed(b"\r\n\x1b[7m^G\x1b[m Help \x1b[7m^O\x1b[m Out \x1b[7m^W\x1b[m Find \x1b[7m^K\x1b[m Cut");
        assert!(s.snapshot().highlights.is_empty());
    }

    #[test]
    fn touching_or_nearly_touching_highlights_read_as_one() {
        // htop's header in two colours; dialog's item split by its own gaps.
        let mut s = Screen::new(3, 40);
        s.feed(b"\r\n\x1b[42mPID USER \x1b[46mCPU%\x1b[42m MEM%\x1b[m");
        assert_eq!(
            s.snapshot().highlights,
            vec![(2, "PID USER CPU% MEM%".to_string())]
        );
        let mut s = Screen::new(3, 40);
        s.feed(b"\r\n\x1b[44m[*]\x1b[m \x1b[44mb\x1b[m  \x1b[44mBeta\x1b[m");
        assert_eq!(
            s.snapshot().highlights,
            vec![(2, "[*] b  Beta".to_string())]
        );
    }

    #[test]
    fn a_highlight_is_judged_whole_not_cell_by_cell() {
        // htop: a green tab over the start of a green header row. Where the
        // two meet the cells match, but the header as a whole stands out —
        // named whole, not from wherever the tab above it ends.
        let mut s = Screen::new(4, 40);
        s.feed(b"\r\n  \x1b[42m[Main]\x1b[m\r\n\x1b[42m  PID USER     COMMAND  \x1b[m");
        assert_eq!(
            s.snapshot().highlights,
            vec![(3, "PID USER     COMMAND".to_string())]
        );
    }

    #[test]
    fn a_border_between_a_button_and_a_shadow_is_no_highlight() {
        // dialog: its bottom border under the focused button sits between
        // that button's colour and the drop shadow's, unlike either — but it
        // names nothing a model could pick.
        let mut s = Screen::new(3, 20);
        s.feed(b"\x1b[47m \x1b[44m< OK >\x1b[47m \r\n");
        s.feed(b"\x1b[47m\x1b(0mqqqqqqj\x1b(B\r\n");
        s.feed(b"\x1b[40m        \x1b[m");
        assert_eq!(s.snapshot().highlights, vec![(1, "< OK >".to_string())]);
    }

    #[test]
    fn a_cursor_waiting_to_wrap_is_named_on_the_last_column() {
        // After a write to the last column the cursor waits past it; a
        // person would say it is on that column, not one the screen lacks.
        let mut s = Screen::new(3, 5);
        s.feed(b"abcde");
        assert_eq!(s.snapshot().cursor, (1, 5));
    }

    #[test]
    fn a_sequence_a_control_interrupts_is_still_read_whole() {
        // A BEL inside `CSI 4 h`: the sequence goes on after it, and the
        // `h` that ends it is no printed letter — insert mode goes on.
        let mut s = screen();
        s.feed(b"world\r\x1b[4\x07hX");
        assert_eq!(s.snapshot().rows, vec!["Xworld"]);
        let mut s = screen();
        s.feed(b"\x1b(\x07");
        s.feed(b"0qq");
        assert_eq!(
            s.snapshot().rows,
            vec!["──"],
            "a designation split by a control"
        );
    }

    #[test]
    fn the_prepass_is_invisible_to_everything_it_does_not_implement() {
        // Differential: whatever the chunking, a stream using none of REP,
        // line drawing or insert mode draws exactly what `vt100` alone draws
        // — a control inside a sequence, strings, split characters included.
        let streams: [&[u8]; 6] = [
            b"plain text\r\nand more\r\n",
            b"\x1b[1\n2mX\x1b[m after a control inside a sequence",
            b"\x1bP1$r0m\x1b\\dcs\x1b]0;title\x07osc \x1b]8;;u\x1b\\link\x1b]8;;\x1b\\",
            "\u{2502} wide \u{4e2d}\u{6587} and \u{00e9}".as_bytes(),
            b"\x1b[2;5Hmoved\x1b[1;1H\x1b[K\x1b[3@ins\x1b[2Pdel\r\n\x1b[?25l\x1b[?1049hA",
            b"\x1b[5;31;1mdense\x1b[0;4mu\x1b[24m\x1b[38;2;1;2;3mrgb\x1b[38:5:9mcolon",
        ];
        for stream in streams {
            let mut expected = vt100::Parser::new(5, 40, 0);
            expected.process(stream);
            let expected: Vec<String> = expected
                .screen()
                .rows(0, 40)
                .map(|row| row.trim_end().to_string())
                .collect();
            for size in [1, 2, 3, 7, stream.len()] {
                let mut s = Screen::new(5, 40);
                for chunk in stream.chunks(size) {
                    s.feed(chunk);
                }
                let got: Vec<String> = s
                    .parser
                    .screen()
                    .rows(0, 40)
                    .map(|row| row.trim_end().to_string())
                    .collect();
                assert_eq!(got, expected, "{stream:?} in chunks of {size}");
            }
        }
    }

    #[test]
    fn everything_else_passes_to_the_emulator_untouched() {
        // Colour, moves, a title, a malformed character — the pre-pass
        // changes only what it implements.
        let mut s = screen();
        s.feed("\x1b[1;31mred\x1b[m \x1b]0;title\x07\x1b[2;3Hé\u{2502}".as_bytes());
        s.feed(b"\xe2\x94");
        s.feed(b"\x80!");
        assert_eq!(s.snapshot().rows, vec!["red", "  é│─!"]);
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
