//! The startup read: what [`InlineViewport::init`] asks the terminal before
//! the first frame, and the pure scanner that sorts out its answer
//! (`docs/images.md`, *Asking the terminal*).
//!
//! Invariant 1 allows one stdin reader, and before the loop's `EventStream`
//! exists the cursor query's read is it — so that one synchronous read is
//! where any other question for the terminal has to go. The kitty graphics
//! query and `XTVERSION` ride in the same write, *ahead of* the cursor query,
//! and a terminal answers in order: the cursor report is always the last
//! answer, so the read stops the moment it completes. A terminal that does
//! not know a question says nothing, which is its answer — and costs no
//! wait, since the cursor report still closes the read.
//!
//! The read takes in whatever arrived before that report, which includes
//! keys typed while it waited — type-ahead the user sent before the TUI was
//! up. Those are not answers: [`ReplyScanner`] keeps them aside byte for
//! byte, and [`typed_events`] turns them into the key events the
//! `EventStream` would have read, which the loop replays ahead of its first
//! `select!`. Nothing after the cursor report is read here at all.
//!
//! [`InlineViewport::init`]: super::InlineViewport::init

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;

use crate::images::GraphicsReply;

/// The cursor position report request (DSR 6), answered
/// `ESC [ row ; col R`. Every terminal answers it, which is what makes it the
/// read's sentinel — and the reason it is always written last.
pub const CURSOR_QUERY: &str = "\x1b[6n";

/// The kitty graphics protocol's own support probe: a 1×1 RGB image sent
/// directly and only *queried* (`a=q`), so nothing is stored or drawn. A
/// terminal that speaks the protocol answers `ESC _ G i=31 ; OK ESC \`, or an
/// error in place of `OK`; one that does not ignores the APC string.
pub const KITTY_GRAPHICS_QUERY: &str = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";

/// The image id [`KITTY_GRAPHICS_QUERY`] names, which its answer echoes.
const KITTY_QUERY_ID: &[u8] = b"31";

/// `XTVERSION`, answered `ESC P > | {name} ESC \` — the terminal's own name,
/// which is what tells the *answering* terminal apart from whatever the
/// environment inherited from an outer one.
pub const VERSION_QUERY: &str = "\x1b[>q";

/// Save the window title on the terminal's title stack (`XTWINOPS` 22).
///
/// It brackets [`KITTY_GRAPHICS_QUERY`] with [`POP_TITLE`] because tmux
/// files an APC string it does not know as the pane's title. The query is
/// never sent under a multiplexer the environment names, but a session can
/// hide one (`docker exec` without `-e TERM`, `ssh` from a tmux whose
/// `default-terminal` is `xterm-256color`), and the pair puts the title back.
pub const PUSH_TITLE: &str = "\x1b[22;0t";

/// Restore the title [`PUSH_TITLE`] saved (`XTWINOPS` 23).
pub const POP_TITLE: &str = "\x1b[23;0t";

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// The longest escape sequence the scanner keeps. An answer is a few dozen
/// bytes; a longer sequence is not one of ours and is dropped at its end
/// rather than hoarded.
const SEQUENCE_MAX_BYTES: usize = 256;

/// The bytes the startup read writes: the graphics questions when `graphics`
/// (see [`crate::images::graphics_query_wanted`]), and always the cursor
/// query, last.
#[must_use]
pub fn startup_query(graphics: bool) -> String {
    if graphics {
        format!("{PUSH_TITLE}{KITTY_GRAPHICS_QUERY}{POP_TITLE}{VERSION_QUERY}{CURSOR_QUERY}")
    } else {
        CURSOR_QUERY.to_string()
    }
}

/// What the startup read learned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupReplies {
    /// Where the cursor was, 0-based — what crossterm's own `position()`
    /// returned before this read replaced it.
    pub cursor: Position,
    /// The answers to the graphics questions, empty when none was asked.
    pub graphics: GraphicsReply,
    /// The bytes typed while the read waited, in order, as the terminal sent
    /// them — [`typed_events`] turns them into keys.
    pub typed: Vec<u8>,
}

/// Where the scanner is in the byte stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum State {
    /// Between sequences.
    #[default]
    Ground,
    /// After an ESC.
    Escape,
    /// Inside `ESC [`, up to its final byte.
    Csi,
    /// After `ESC O`: one more byte completes it.
    Ss3,
    /// Inside a control string — APC `ESC _`, DCS `ESC P`, OSC `ESC ]`,
    /// PM `ESC ^`, SOS `ESC X` — up to its string terminator.
    Text,
    /// An ESC inside a control string: `\` completes the terminator, and
    /// anything else cuts the string short.
    TextEscape,
}

/// Sorts the startup read's bytes into the terminal's answers and the keys
/// typed meanwhile, one byte at a time — the boundary reads one at a time so
/// that it stops on the cursor report's last byte and leaves everything after
/// it to the `EventStream`.
#[derive(Debug, Default)]
pub struct ReplyScanner {
    state: State,
    /// The sequence in progress, from its ESC (a control string's terminator
    /// is not kept).
    sequence: Vec<u8>,
    /// Whether `sequence` outgrew [`SEQUENCE_MAX_BYTES`] — dropped at its end.
    overflowed: bool,
    typed: Vec<u8>,
    graphics: GraphicsReply,
    cursor: Option<Position>,
}

impl ReplyScanner {
    /// A scanner that has read nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Take in the next byte; `true` once the cursor report has completed,
    /// which is where the read ends.
    pub fn push(&mut self, byte: u8) -> bool {
        if self.cursor.is_none() {
            self.step(byte);
        }
        self.cursor.is_some()
    }

    /// The answer, once the cursor report has arrived.
    #[must_use]
    pub fn into_replies(self) -> Option<StartupReplies> {
        Some(StartupReplies {
            cursor: self.cursor?,
            graphics: self.graphics,
            typed: self.typed,
        })
    }

    fn step(&mut self, byte: u8) {
        match self.state {
            State::Ground => {
                if byte == ESC {
                    self.begin();
                } else {
                    self.typed.push(byte);
                }
            }
            State::Escape => match byte {
                b'[' => self.enter(State::Csi, byte),
                b'O' => self.enter(State::Ss3, byte),
                b'_' | b'P' | b']' | b'^' | b'X' => self.enter(State::Text, byte),
                ESC => {
                    // The ESC before was a key on its own; this one opens
                    // whatever follows.
                    self.typed.push(ESC);
                    self.begin();
                }
                _ => {
                    // Alt and a key.
                    self.typed.extend_from_slice(&[ESC, byte]);
                    self.reset();
                }
            },
            State::Csi => {
                if byte == ESC {
                    // Cut short by the next sequence: drop the fragment.
                    self.begin();
                } else {
                    self.keep(byte);
                    if (0x40..=0x7e).contains(&byte) {
                        self.finish_csi();
                    }
                }
            }
            State::Ss3 => {
                if byte == ESC {
                    self.begin();
                } else {
                    self.keep(byte);
                    self.typed.extend_from_slice(&self.sequence);
                    self.reset();
                }
            }
            State::Text => match byte {
                ESC => self.state = State::TextEscape,
                // OSC alone may end on BEL.
                BEL if self.sequence.get(1) == Some(&b']') => self.finish_text(),
                _ => self.keep(byte),
            },
            State::TextEscape => {
                if byte == b'\\' {
                    self.finish_text();
                } else {
                    // A string cut short by a new sequence (Alt+_ typed
                    // ahead opens an APC the terminal's own answer then
                    // interrupts): drop it, and read this byte as the new
                    // sequence's second — unless it is another ESC, which
                    // opens the sequence itself. The ESC that did the
                    // cutting is never a key: replayed as Esc into an empty
                    // session, it would quit.
                    self.begin();
                    if byte != ESC {
                        self.step(byte);
                    }
                }
            }
        }
    }

    /// Open a sequence at an ESC.
    fn begin(&mut self) {
        self.sequence.clear();
        self.sequence.push(ESC);
        self.overflowed = false;
        self.state = State::Escape;
    }

    fn enter(&mut self, state: State, byte: u8) {
        self.sequence.push(byte);
        self.state = state;
    }

    fn keep(&mut self, byte: u8) {
        if self.sequence.len() < SEQUENCE_MAX_BYTES {
            self.sequence.push(byte);
        } else {
            self.overflowed = true;
        }
    }

    fn reset(&mut self) {
        self.sequence.clear();
        self.overflowed = false;
        self.state = State::Ground;
    }

    /// A CSI sequence ended: the cursor report, or a key typed ahead (an
    /// arrow, a function key) kept for the replay.
    fn finish_csi(&mut self) {
        if !self.overflowed {
            match cursor_report(&self.sequence) {
                Some(cursor) => self.cursor = Some(cursor),
                None => self.typed.extend_from_slice(&self.sequence),
            }
        }
        self.reset();
    }

    /// A control string ended: the kitty answer (APC), the terminal's name
    /// (DCS `>|`), or something nobody asked for — never a key.
    fn finish_text(&mut self) {
        if !self.overflowed {
            let body = self.sequence.get(2..).unwrap_or_default();
            match self.sequence.get(1) {
                Some(b'_') => {
                    if let Some(ok) = kitty_answer(body) {
                        self.graphics.kitty = Some(ok);
                    }
                }
                Some(b'P') => {
                    if let Some(name) = body.strip_prefix(b">|") {
                        let name = String::from_utf8_lossy(name).trim().to_string();
                        self.graphics.version.get_or_insert(name);
                    }
                }
                _ => {}
            }
        }
        self.reset();
    }
}

/// `ESC [ row ; col R` as a 0-based position (crossterm's convention).
fn cursor_report(sequence: &[u8]) -> Option<Position> {
    let params = sequence.strip_prefix(b"\x1b[")?.strip_suffix(b"R")?;
    let (row, col) = std::str::from_utf8(params).ok()?.split_once(';')?;
    let digits = |field: &str| {
        (!field.is_empty() && field.bytes().all(|b| b.is_ascii_digit()))
            .then(|| field.parse::<u16>().ok())
            .flatten()
    };
    let (row, col) = (digits(row)?, digits(col)?);
    Some(Position::new(col.saturating_sub(1), row.saturating_sub(1)))
}

/// The kitty answer in an APC body (`G{keys};{message}`): `Some(true)` for
/// `OK`, `Some(false)` for an error — `None` when it is not an answer to
/// [`KITTY_GRAPHICS_QUERY`]'s image id.
fn kitty_answer(body: &[u8]) -> Option<bool> {
    let body = body.strip_prefix(b"G")?;
    let split = body.iter().position(|&b| b == b';')?;
    let (keys, message) = (&body[..split], &body[split + 1..]);
    keys.split(|&b| b == b',')
        .any(|pair| pair.strip_prefix(b"i=") == Some(KITTY_QUERY_ID))
        .then_some(message == b"OK")
}

/// The keys `bytes` — type-ahead the startup read took in — stand for, as
/// crossterm's legacy parser reads them: text and control keys, Esc, and
/// Alt with a key. An escape **sequence** (an arrow, a function key) is
/// dropped whole rather than replayed as stray characters; replaying it
/// would mean a second copy of crossterm's CSI parser for keys nobody types
/// before the TUI is up. The kitty keyboard protocol is pushed only after
/// the read, so legacy encoding is all there is to read.
#[must_use]
pub fn typed_events(bytes: &[u8]) -> Vec<Event> {
    let mut events = Vec::new();
    let mut rest = bytes;
    while let Some(&first) = rest.first() {
        let used = if first == ESC {
            match rest.get(1) {
                None => {
                    events.push(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
                    1
                }
                Some(&ESC) => {
                    events.push(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
                    2
                }
                Some(b'[') => {
                    let body = &rest[2..];
                    2 + body
                        .iter()
                        .position(|b| (0x40..=0x7e).contains(b))
                        .map_or(body.len(), |end| end + 1)
                }
                Some(b'O') => rest.len().min(3),
                Some(_) => {
                    let (key, used) = plain_key(&rest[1..]);
                    if let Some(mut key) = key {
                        key.modifiers |= KeyModifiers::ALT;
                        events.push(Event::Key(key));
                    }
                    1 + used
                }
            }
        } else {
            let (key, used) = plain_key(rest);
            events.extend(key.map(Event::Key));
            used
        };
        rest = &rest[used..];
    }
    events
}

/// The key at the head of `bytes` (which does not start with ESC), and how
/// many bytes it took — crossterm's legacy mapping.
fn plain_key(bytes: &[u8]) -> (Option<KeyEvent>, usize) {
    let key = |code, modifiers| Some(KeyEvent::new(code, modifiers));
    let ctrl = |c: u8| key(KeyCode::Char(char::from(c)), KeyModifiers::CONTROL);
    match bytes[0] {
        b'\r' => (key(KeyCode::Enter, KeyModifiers::NONE), 1),
        // Raw mode is on, so a bare line feed is Ctrl+J, as crossterm reads it.
        b'\n' => (ctrl(b'j'), 1),
        b'\t' => (key(KeyCode::Tab, KeyModifiers::NONE), 1),
        0x7f => (key(KeyCode::Backspace, KeyModifiers::NONE), 1),
        0 => (ctrl(b' '), 1),
        c @ 0x01..=0x1a => (ctrl(c - 0x01 + b'a'), 1),
        c @ 0x1c..=0x1f => (ctrl(c - 0x1c + b'4'), 1),
        lead => {
            let len = match lead {
                0x20..=0x7f => 1,
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => return (None, 1),
            };
            let Some(c) = bytes
                .get(..len)
                .and_then(|text| std::str::from_utf8(text).ok())
                .and_then(|text| text.chars().next())
            else {
                return (None, 1);
            };
            let modifiers = if c.is_uppercase() {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            };
            (key(KeyCode::Char(c), modifiers), len)
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Position;

    use super::*;

    /// Feed `bytes` to a scanner one at a time, the way the boundary reads
    /// them, stopping where the boundary stops — the byte that completes the
    /// cursor report — and return the answer beside whatever was left unread.
    fn scan(bytes: &[u8]) -> (Option<StartupReplies>, &[u8]) {
        let mut scanner = ReplyScanner::new();
        for (at, &byte) in bytes.iter().enumerate() {
            if scanner.push(byte) {
                return (scanner.into_replies(), &bytes[at + 1..]);
            }
        }
        (scanner.into_replies(), &[])
    }

    fn answered(bytes: &[u8]) -> StartupReplies {
        let (replies, rest) = scan(bytes);
        assert!(rest.is_empty(), "read past the cursor report: {rest:?}");
        replies.expect("a cursor report ends the read")
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn char_key(c: char) -> Event {
        key(KeyCode::Char(c), KeyModifiers::NONE)
    }

    // ===== the query =====

    #[test]
    fn the_cursor_query_closes_every_question_so_its_answer_ends_the_read() {
        assert_eq!(startup_query(false), CURSOR_QUERY);
        // The kitty query sits inside a pushed and popped title: tmux files
        // an APC string it does not know as the pane's title, and a session
        // that hides tmux from the environment would otherwise wear
        // `Gi=31,s=1,…` as its title for good.
        assert_eq!(
            startup_query(true),
            format!("{PUSH_TITLE}{KITTY_GRAPHICS_QUERY}{POP_TITLE}{VERSION_QUERY}{CURSOR_QUERY}")
        );
    }

    // ===== the answer =====

    #[test]
    fn a_bare_cursor_report_is_the_whole_answer() {
        let replies = answered(b"\x1b[12;5R");
        assert_eq!(replies.cursor, Position::new(4, 11), "1-based on the wire");
        assert_eq!(replies.graphics, GraphicsReply::default());
        assert!(replies.typed.is_empty());
    }

    #[test]
    fn a_kitty_terminal_answers_ok_and_names_itself_before_the_cursor() {
        // herdr 0.9.3's answer to `startup_query(true)`, byte for byte, as
        // recorded from a real pane: its emulator is libghostty, which keeps
        // the answers in the order the questions were asked.
        let replies = answered(b"\x1b_Gi=31;OK\x1b\\\x1bP>|libghostty\x1b\\\x1b[2;1R");
        assert_eq!(replies.graphics.kitty, Some(true));
        assert_eq!(replies.graphics.version.as_deref(), Some("libghostty"));
        assert_eq!(replies.cursor, Position::new(0, 1));
        assert!(replies.typed.is_empty());
    }

    #[test]
    fn a_kitty_error_answer_is_a_no() {
        let replies = answered(b"\x1b_Gi=31;ENOTSUPPORTED:unsupported format\x1b\\\x1b[1;1R");
        assert_eq!(replies.graphics.kitty, Some(false));
    }

    #[test]
    fn an_answer_about_another_image_is_not_ours() {
        let replies = answered(b"\x1b_Gi=7;OK\x1b\\\x1b[1;1R");
        assert_eq!(replies.graphics.kitty, None);
    }

    #[test]
    fn keys_typed_around_the_answers_are_kept_and_never_read_as_one() {
        // Type-ahead: `hi` and an arrow before the terminal answered, an `x`
        // between two answers. The arrow is a CSI sequence too, and must not
        // be mistaken for the cursor report.
        let replies = answered(b"hi\x1b[A\x1b_Gi=31;OK\x1b\\x\x1b[2;3R");
        assert_eq!(replies.typed, b"hi\x1b[Ax");
        assert_eq!(replies.graphics.kitty, Some(true));
        assert_eq!(replies.cursor, Position::new(2, 1));
    }

    #[test]
    fn the_read_stops_at_the_cursor_report_and_leaves_what_follows_unread() {
        // What the user types after the answer belongs to the loop's
        // EventStream, which reads it with crossterm's whole parser.
        let (replies, rest) = scan(b"\x1b[5;5Rabc");
        assert_eq!(rest, b"abc");
        assert!(replies.expect("answered").typed.is_empty());
    }

    #[test]
    fn nothing_is_an_answer_until_the_cursor_report_completes() {
        let mut scanner = ReplyScanner::new();
        for &byte in b"\x1b_Gi=31;OK\x1b\\\x1b[7;7" {
            assert!(!scanner.push(byte), "{:?}", char::from(byte));
        }
        assert!(scanner.push(b'R'));
    }

    #[test]
    fn a_typed_escape_that_opens_a_string_cannot_swallow_the_answer() {
        // Alt+_ typed ahead is `ESC _` — the opening of an APC string. The
        // terminal's own answer starts with an ESC that ends it, so the
        // answers after it are still read.
        let replies = answered(b"\x1b_z\x1b_Gi=31;OK\x1b\\\x1b[1;2R");
        assert_eq!(replies.graphics.kitty, Some(true));
        assert_eq!(replies.cursor, Position::new(1, 0));
    }

    #[test]
    fn the_escape_that_cuts_a_string_short_is_never_replayed_as_a_key() {
        // Inside a control string an ESC is half a terminator, so one that
        // turns out to cut the string short is not a key the user pressed —
        // and an Esc replayed into an empty session would quit it.
        let replies = answered(b"\x1b_x\x1b\x1b[1;1R");
        assert!(replies.typed.is_empty(), "{:?}", replies.typed);
        assert_eq!(replies.cursor, Position::new(0, 0));
    }

    #[test]
    fn a_sequence_cut_short_by_the_next_one_is_dropped() {
        // A CSI fragment with no final byte is interrupted by the cursor
        // report itself, which still reads.
        let replies = answered(b"\x1b[1;\x1b[3;4R");
        assert_eq!(replies.cursor, Position::new(3, 2));
        assert!(replies.typed.is_empty());
    }

    #[test]
    fn a_string_too_long_to_be_an_answer_is_dropped_not_hoarded() {
        let mut bytes = b"\x1bP>|".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', 4 * SEQUENCE_MAX_BYTES));
        bytes.extend_from_slice(b"\x1b\\\x1b[1;1R");
        let replies = answered(&bytes);
        assert_eq!(replies.graphics.version, None);
        assert!(replies.typed.is_empty());
    }

    #[test]
    fn an_unasked_for_string_is_skipped_not_typed() {
        // An OSC answer (here ended by BEL, which OSC allows) is the
        // terminal talking, not the user typing.
        let replies = answered(b"\x1b]11;rgb:0000/0000/0000\x07\x1b[1;1R");
        assert!(replies.typed.is_empty());
    }

    #[test]
    fn escape_keys_typed_ahead_are_kept_whole() {
        // A lone Esc, Alt+x, and an SS3 arrow.
        let replies = answered(b"\x1b\x1bx\x1bOA\x1b[1;1R");
        assert_eq!(replies.typed, b"\x1b\x1bx\x1bOA");
    }

    // ===== the typed keys, replayed =====

    #[test]
    fn typed_text_replays_as_the_keys_the_event_stream_would_have_read() {
        assert_eq!(
            typed_events("hé".as_bytes()),
            vec![char_key('h'), char_key('é')]
        );
        // crossterm marks an uppercase letter shifted.
        assert_eq!(
            typed_events(b"A"),
            vec![key(KeyCode::Char('A'), KeyModifiers::SHIFT)]
        );
    }

    #[test]
    fn typed_controls_map_the_way_crossterms_legacy_parser_maps_them() {
        let ctrl = |c| key(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(
            typed_events(b"\r\n\t\x7f\x03\x08\x00\x1c"),
            vec![
                key(KeyCode::Enter, KeyModifiers::NONE),
                // Raw mode is on, so a bare line feed is Ctrl+J — which the
                // composer answers with a newline.
                ctrl('j'),
                key(KeyCode::Tab, KeyModifiers::NONE),
                key(KeyCode::Backspace, KeyModifiers::NONE),
                ctrl('c'),
                ctrl('h'),
                ctrl(' '),
                ctrl('4'),
            ]
        );
    }

    #[test]
    fn typed_escapes_replay_as_esc_and_alt_while_sequences_are_dropped() {
        assert_eq!(
            typed_events(b"\x1b"),
            vec![key(KeyCode::Esc, KeyModifiers::NONE)]
        );
        assert_eq!(
            typed_events(b"\x1bx"),
            vec![key(KeyCode::Char('x'), KeyModifiers::ALT)]
        );
        assert_eq!(
            typed_events(b"\x1b\x1b"),
            vec![key(KeyCode::Esc, KeyModifiers::NONE)]
        );
        // An arrow, a modified arrow, an SS3 arrow: dropped whole, never
        // replayed as stray characters.
        assert_eq!(
            typed_events(b"a\x1b[A\x1b[1;5C\x1bOAb"),
            vec![char_key('a'), char_key('b')]
        );
    }

    #[test]
    fn a_byte_that_is_not_text_is_skipped() {
        assert_eq!(typed_events(&[0xff, b'a']), vec![char_key('a')]);
    }
}
