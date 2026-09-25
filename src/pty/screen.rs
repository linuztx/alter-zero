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
//! What full-screen programs rely on that `vt100` leaves out is done **in
//! front of it** (`Prepass`): the DEC line-drawing set ([`super::charset`])
//! a box is drawn in without a UTF-8 locale; REP (`CSI n b`) — ncurses
//! repeats a character that way for any run of it, so without it
//! indentation and the spaces between columns vanished and the rest of the
//! row slid left; insert mode (`CSI 4 h`); autowrap off (`CSI ? 7 l`), a
//! line that stops at the margin instead of running onto the next; the tab
//! stops a program sets and the tabs that move by them (CBT, CHT); the other
//! names cursor moves go by (HVP, HPA, HPR, VPR, IND, NEL, SCOSC); and the
//! screen/tmux window title (`ESC k`), which is not text. The differential
//! check against tmux over a sweep of programs — `scripts/pty_oracle.sh`,
//! `docs/interactive-shell.md` — is how the list was found.

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
    /// The cursor's line with [`CURSOR_MARK`] where the cursor is, cut to
    /// [`CURSOR_BEFORE`] characters before it and [`CURSOR_AFTER`] after —
    /// `None` when the cursor is hidden or its line blank. Where typing
    /// lands, without counting down forty rows to find it: a prompt that
    /// holds a default (`File Name to Write: notes.txt‸`), the character an
    /// editor's cursor is on (`alpha ‸beta`).
    pub cursor_text: Option<String>,
    /// The program hides the cursor (`ESC [ ? 25 l` — htop, mc, whiptail):
    /// where it parked it says nothing.
    pub cursor_hidden: bool,
}

/// Marks the cursor's place in [`Snapshot::cursor_text`] — the caret
/// proofreaders use for an insertion point.
pub const CURSOR_MARK: char = '\u{2038}';
/// How much of the cursor's line before it [`Snapshot::cursor_text`] keeps.
pub const CURSOR_BEFORE: usize = 40;
/// How much of the cursor's line after it [`Snapshot::cursor_text`] keeps.
pub const CURSOR_AFTER: usize = 20;

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
        self.prepass.rewrite(bytes, &mut self.parser);
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
            cursor_text: self.cursor_text(),
            cursor_hidden: screen.hide_cursor(),
        }
    }

    /// See [`Snapshot::cursor_text`].
    fn cursor_text(&self) -> Option<String> {
        let screen = self.parser.screen();
        if screen.hide_cursor() {
            return None;
        }
        let (row, col) = screen.cursor_position();
        let (_, columns) = screen.size();
        let (mut before, mut after) = (Vec::new(), Vec::new());
        for at in 0..columns {
            let Some(cell) = screen.cell(row, at) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let side = if at < col { &mut before } else { &mut after };
            match cell.contents() {
                "" => side.push(' '),
                text => side.extend(text.chars()),
            }
        }
        while after.last() == Some(&' ') {
            after.pop();
        }
        if after.is_empty() && before.iter().all(|c| c.is_whitespace()) {
            return None;
        }
        // Cut to the window around the cursor; the blanks at a cut say
        // nothing beside the ellipsis, so they go, and the text between
        // stays exactly as drawn.
        let cut = before.len() > CURSOR_BEFORE;
        let kept: String = before[before.len().saturating_sub(CURSOR_BEFORE)..]
            .iter()
            .collect();
        let before = if cut {
            format!("\u{2026}{}", kept.trim_start())
        } else {
            kept.trim_start().to_string()
        };
        let cut = after.len() > CURSOR_AFTER;
        let kept: String = after[..after.len().min(CURSOR_AFTER)].iter().collect();
        let after = if cut {
            format!("{}\u{2026}", kept.trim_end())
        } else {
            kept
        };
        Some(format!("{before}{CURSOR_MARK}{after}"))
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
    /// ([`super::keys::encode`]): cursor-key mode, bracketed paste, the
    /// alternate screen — what the screen shows; the terminal's line mode
    /// and its reader are the session's to add.
    #[must_use]
    pub fn modes(&self) -> super::keys::Modes {
        let screen = self.parser.screen();
        super::keys::Modes {
            app_cursor: screen.application_cursor(),
            bracketed_paste: screen.bracketed_paste(),
            full_screen: screen.alternate_screen(),
            ..super::keys::Modes::default()
        }
    }

    /// Has the program switched to the alternate screen and drawn nothing on
    /// it — getting ready (btop gathers its first frame for seconds), or
    /// between a clear and its frame? There is nothing there to read yet,
    /// and nothing to answer.
    #[must_use]
    pub fn undrawn(&self) -> bool {
        let screen = self.parser.screen();
        let (_, columns) = screen.size();
        screen.alternate_screen() && screen.rows(0, columns).all(|row| row.trim().is_empty())
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
/// byte stream on its way in (see the module docs). Everything else passes
/// through byte for byte.
///
/// It follows the stream with a `vte` parser of its own, a byte at a time,
/// so it knows which bytes make up each character: a character read whole is
/// written anew (translated, repeated, or made room for), and every other
/// byte — sequences, controls, text too malformed to be sure of — is handed
/// on exactly as it came, for the emulator to make of it what it always did.
/// Where the right bytes depend on where the cursor is — a tab, a line
/// reaching the margin with autowrap off — it hands the emulator what it has
/// so far and asks ([`Out::cursor`]).
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
    /// The sequence an [`Event::Alias`] is written as.
    alias: Vec<u8>,
    /// The parser is known to be between characters and sequences: a
    /// character or a sequence was the last thing completed, and nothing
    /// has been read since that completed nothing. Only then is printable
    /// ASCII sure to be printed text — mid-sequence it is parameters, and a
    /// control executed inside a sequence (`CSI 4 BEL h`) completes an
    /// event without ending it.
    ground: bool,
    /// Autowrap (DECAWM) is off: a character past the right margin takes
    /// the last column instead of starting the next row.
    autowrap_off: bool,
    /// The tab stops, as 0-based columns, once the program has set or
    /// cleared one; until then every eighth column, which the emulator
    /// keeps itself.
    tabs: Option<std::collections::BTreeSet<usize>>,
    /// Inside a screen/tmux window title (`ESC k`): everything up to the
    /// BEL or the ESC that ends it is dropped.
    title: bool,
}

/// One thing the parser completed.
#[derive(Debug, Clone, Copy)]
enum Event {
    Print(char),
    /// REP: the last character, this many times more.
    Repeat(usize),
    /// A sequence `vt100` knows under another name: written as
    /// [`PrepassState::alias`] in its place.
    Alias,
    /// A tab the stops decide — HT once the program has stops of its own,
    /// CHT, CBT — this many stops on (or back, negative).
    Tab(isize),
    /// HTS (true) or TBC (false): a stop set or cleared at the cursor.
    Stop(bool),
    /// Any other control or sequence.
    Other,
}

/// The [`Prepass`]'s output on its way to the emulator.
struct Out<'p> {
    bytes: Vec<u8>,
    emulator: &'p mut vt100::Parser<Replies>,
}

impl Out<'_> {
    /// Hand the emulator what is written so far.
    fn flush(&mut self) {
        if !self.bytes.is_empty() {
            self.emulator.process(&self.bytes);
            self.bytes.clear();
        }
    }

    /// Where the cursor is once the emulator has taken what is written so
    /// far: `(column, columns)`, 0-based — past the last column when a
    /// character there left it waiting to wrap.
    fn cursor(&mut self) -> (usize, usize) {
        self.flush();
        let screen = self.emulator.screen();
        (
            usize::from(screen.cursor_position().1),
            usize::from(screen.size().1),
        )
    }

    /// CHA: the cursor to `column`, 0-based.
    fn column(&mut self, column: usize) {
        self.bytes
            .extend_from_slice(format!("\x1b[{}G", column + 1).as_bytes());
    }
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

    /// Hand `bytes` to `emulator` as it should see them.
    fn rewrite(&mut self, bytes: &[u8], emulator: &mut vt100::Parser<Replies>) {
        let mut out = Out {
            bytes: Vec::with_capacity(bytes.len()),
            emulator,
        };
        let mut at = 0;
        while at < bytes.len() {
            // A window title is for a multiplexer's status line, not the
            // screen: dropped up to the BEL that ends it (taken with it) or
            // the ESC (read on — it starts the ST, or whatever comes next).
            if self.state.title {
                match bytes[at..].iter().position(|&b| b == 0x07 || b == 0x1b) {
                    Some(end) => {
                        self.state.title = false;
                        at += end + usize::from(bytes[at + end] == 0x07);
                    }
                    None => at = bytes.len(),
                }
                continue;
            }
            // Plain text between sequences, with nothing to translate or make
            // room for, goes through as it is: the parser would print each
            // byte and stay where it is.
            if self.state.ground
                && !self.state.insert
                && !self.state.autowrap_off
                && !self.state.charsets.translates()
            {
                let run = bytes[at..]
                    .iter()
                    .take_while(|&&b| (0x20..0x7f).contains(&b))
                    .count();
                if run > 0 {
                    out.bytes.extend_from_slice(&bytes[at..at + run]);
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
            // An ESC leaves the parser in an escape sequence, whatever it
            // completed: the ST (`ESC \`) that ends a link or a title ends
            // the string on its ESC, and the `\` is still the parser's.
            if byte == 0x1b {
                self.state.ground = false;
            }
            let whole = match self.state.events.as_slice() {
                [] => {
                    self.state.ground = false;
                    continue;
                }
                &[Event::Print(c)] if encodes(c, &self.pending) => Some(Event::Print(c)),
                &[Event::Repeat(n)] => Some(Event::Repeat(n)),
                &[Event::Alias] => Some(Event::Alias),
                // Not a tab executed inside another sequence (`CSI 1 HT 2 m`):
                // that one goes on as it came.
                &[event @ (Event::Tab(_) | Event::Stop(_))] if self.state.ground => Some(event),
                _ => None,
            };
            match whole {
                Some(Event::Alias) => {
                    // The sequence under the name the emulator knows; a
                    // sequence a control interrupted went on already, and
                    // the one written here cuts it short.
                    self.pending.clear();
                    out.bytes.append(&mut self.state.alias);
                }
                Some(Event::Tab(stops)) => {
                    self.pending.clear();
                    let (column, columns) = out.cursor();
                    let to = self.tab(column, columns, stops);
                    out.column(to);
                }
                Some(Event::Stop(set)) => {
                    // The emulator makes nothing of HTS or TBC; they go on
                    // as they came.
                    out.bytes.append(&mut self.pending);
                    let (column, columns) = out.cursor();
                    let column = column.min(columns.saturating_sub(1));
                    let stops = self
                        .state
                        .tabs
                        .get_or_insert_with(|| (8..columns).step_by(8).collect());
                    if set {
                        stops.insert(column);
                    } else {
                        stops.remove(&column);
                    }
                }
                Some(Event::Print(c)) => {
                    let shown = self.state.charsets.map(c);
                    self.state.last = Some(shown);
                    self.write_char(&mut out, shown);
                    self.pending.clear();
                }
                Some(Event::Repeat(n)) => {
                    // The REP itself is nothing to the emulator; what it
                    // stands for is written after it.
                    out.bytes.append(&mut self.pending);
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
                    out.bytes.append(&mut self.pending);
                }
            }
            self.state.events.clear();
        }
        out.flush();
    }

    /// Write `c` — in insert mode after making room for it, as the terminal
    /// would (ICH, which the emulator implements); with autowrap off, never
    /// past the margin: a narrow character with no room takes the last
    /// column, a wide one is dropped (tmux's rule), and the cursor stays on
    /// the last column rather than waiting to wrap, so what comes next —
    /// an erase, a report — finds it there.
    fn write_char(&self, out: &mut Out<'_>, c: char) {
        let width = c.width().unwrap_or(0);
        let mut at = None;
        if self.state.autowrap_off && width > 0 {
            let (column, columns) = out.cursor();
            if column + width > columns {
                if width > 1 || columns == 0 {
                    return;
                }
                out.column(columns - 1);
                at = Some((columns - 1, columns));
            } else {
                at = Some((column, columns));
            }
        }
        if self.state.insert && width > 0 {
            out.bytes
                .extend_from_slice(format!("\x1b[{width}@").as_bytes());
        }
        let mut buf = [0u8; 4];
        out.bytes
            .extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        if let Some((column, columns)) = at
            && column + width >= columns
        {
            out.column(columns - 1);
        }
    }

    /// The column `stops` tab stops on from `column` (back, when negative):
    /// past the last stop, the last column; before the first, the first.
    fn tab(&self, column: usize, columns: usize, stops: isize) -> usize {
        let last = columns.saturating_sub(1);
        let is_stop = |c: usize| match &self.state.tabs {
            Some(set) => set.contains(&c),
            None => c.is_multiple_of(8),
        };
        let mut to = column.min(last);
        for _ in 0..stops.unsigned_abs().min(columns) {
            to = if stops > 0 {
                ((to + 1)..=last).find(|&c| is_stop(c)).unwrap_or(last)
            } else {
                (0..to).rev().find(|&c| is_stop(c)).unwrap_or(0)
            };
        }
        to
    }
}

/// Are `bytes` exactly `c`'s UTF-8 — a character read whole, with nothing
/// malformed or unfinished in front of it?
fn encodes(c: char, bytes: &[u8]) -> bool {
    let mut buf = [0u8; 4];
    c.encode_utf8(&mut buf).as_bytes() == bytes
}

impl PrepassState {
    /// The sequence just read is written as `known` instead.
    fn aliased(&mut self, known: Vec<u8>) {
        self.alias = known;
        self.events.push(Event::Alias);
    }
}

/// `CSI {params} {action}` — a sequence rebuilt with another final byte.
fn csi(params: &vte::Params, action: char) -> Vec<u8> {
    let mut out = String::from("\x1b[");
    for (index, param) in params.iter().enumerate() {
        if index > 0 {
            out.push(';');
        }
        for (sub, value) in param.iter().enumerate() {
            if sub > 0 {
                out.push(':');
            }
            out.push_str(&value.to_string());
        }
    }
    out.push(action);
    out.into_bytes()
}

impl vte::Perform for PrepassState {
    fn print(&mut self, c: char) {
        self.ground = true;
        self.events.push(Event::Print(c));
    }

    fn execute(&mut self, byte: u8) {
        self.charsets.execute(byte);
        self.events.push(if byte == b'\t' && self.tabs.is_some() {
            Event::Tab(1)
        } else {
            Event::Other
        });
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
                // CHT and CBT: tabs forward and back.
                'I' | 'Z' => {
                    let count = params
                        .iter()
                        .next()
                        .and_then(|p| p.first().copied())
                        .unwrap_or(1)
                        .max(1);
                    let count = isize::from(i16::try_from(count).unwrap_or(i16::MAX));
                    self.events
                        .push(Event::Tab(if action == 'I' { count } else { -count }));
                    return;
                }
                // TBC: the stop at the cursor, or (3) every stop.
                'g' => match params.iter().next().and_then(|p| p.first().copied()) {
                    None | Some(0) => {
                        self.events.push(Event::Stop(false));
                        return;
                    }
                    Some(3) => self.tabs = Some(std::collections::BTreeSet::new()),
                    Some(_) => {}
                },
                // Cursor moves under their other names: HVP is CUP, HPA is
                // CHA, HPR is CUF, VPR is CUD.
                'f' | '`' | 'a' | 'e' => {
                    let known = match action {
                        'f' => 'H',
                        '`' => 'G',
                        'a' => 'C',
                        _ => 'B',
                    };
                    return self.aliased(csi(params, known));
                }
                // SCOSC and SCORC: DECSC and DECRC — with no parameters
                // (the parser hands on a lone default zero).
                's' | 'u' if params.iter().all(|p| p == [0]) => {
                    let known: &[u8] = if action == 's' { b"\x1b7" } else { b"\x1b8" };
                    return self.aliased(known.to_vec());
                }
                _ => {}
            }
        }
        // The alternate screen and the saved cursor, set one at a time.
        if intermediates == [b'?'] && matches!(action, 'h' | 'l') {
            let set = action == 'h';
            if params.iter().any(|p| p == [7]) {
                self.autowrap_off = !set;
            }
            let only = |mode: u16| params.len() == 1 && params.iter().next() == Some(&[mode][..]);
            if only(1047) {
                return self.aliased(if set {
                    b"\x1b[?47h".to_vec()
                } else {
                    b"\x1b[?47l".to_vec()
                });
            }
            if only(1048) {
                return self.aliased(if set {
                    b"\x1b7".to_vec()
                } else {
                    b"\x1b8".to_vec()
                });
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
                self.autowrap_off = false;
                self.tabs = None;
            }
            // HTS: a tab stop at the cursor.
            [] if byte == b'H' => {
                self.events.push(Event::Stop(true));
                return;
            }
            // A screen/tmux window title follows; not the emulator's.
            [] if byte == b'k' => {
                self.title = true;
                return self.aliased(Vec::new());
            }
            // IND and NEL: a line feed, and a new line.
            [] if byte == b'D' => return self.aliased(b"\n".to_vec()),
            [] if byte == b'E' => return self.aliased(b"\r\n".to_vec()),
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
    fn a_cursor_placed_with_hvp_lands_where_cup_would_put_it() {
        // btop places every piece of text with HVP (`CSI row;col f`), never
        // CUP (`CSI row;col H`): the same command under another name, which
        // `vt100` ignores — btop's panels ran together across rows.
        let mut s = screen();
        s.feed(b"\x1b[2;4fbtop\x1b[1;1fcpu");
        assert_eq!(s.snapshot().rows, vec!["cpu", "   btop"]);
        assert_eq!(s.snapshot().cursor, (1, 4));
        let mut s = screen();
        s.feed(b"x\x1b[3fy");
        assert_eq!(s.snapshot().rows, vec!["x", "", "y"], "the column defaults");
    }

    #[test]
    fn every_other_name_for_a_cursor_move_moves_it() {
        // HPA, HPR, VPR, SCOSC/SCORC, IND, NEL: what `vt100` knows as CHA,
        // CUF, CUD, DECSC/DECRC, a line feed, a new line.
        let cases: [(&[u8], &[&str]); 6] = [
            (b"abc\x1b[6`X", &["abc  X"]),
            (b"a\x1b[3aX", &["a   X"]),
            (b"ab\x1b[2eX", &["ab", "", "  X"]),
            (b"ab\x1b[s\r\ncd\x1b[uX", &["abX", "cd"]),
            (b"ab\x1bDX", &["ab", "  X"]),
            (b"ab\x1bEX", &["ab", "X"]),
        ];
        for (bytes, rows) in cases {
            let mut s = screen();
            s.feed(bytes);
            assert_eq!(s.snapshot().rows, rows.to_vec(), "{bytes:?}");
        }
    }

    #[test]
    fn the_alternate_screen_by_its_other_numbers_is_the_alternate_screen() {
        // Modes 1047 (the alternate screen) and 1048 (the saved cursor),
        // which some programs switch one at a time instead of 1049.
        let mut s = screen();
        s.feed(b"shell$ \x1b[?1048h\x1b[?1047h\x1b[Hfull screen");
        assert!(s.snapshot().alternate);
        s.feed(b"\x1b[?1047l\x1b[?1048lX");
        let snap = s.snapshot();
        assert!(!snap.alternate);
        assert_eq!(snap.rows, vec!["shell$ X"], "the cursor restored");
    }

    /// `stream` fed to a fresh `rows` × `columns` screen in chunks of each
    /// size in turn, every result checked by `check`.
    fn fed_in_chunks(rows: u16, columns: u16, stream: &[u8], check: impl Fn(&Screen, usize)) {
        for size in [1, 2, 3, 7, stream.len()] {
            let mut s = Screen::new(rows, columns);
            for chunk in stream.chunks(size) {
                s.feed(chunk);
            }
            check(&s, size);
        }
    }

    #[test]
    fn with_autowrap_off_a_line_stops_at_the_last_column() {
        // DECAWM off: what runs past the right margin overwrites the last
        // column instead of wrapping onto the next row, and the cursor
        // stays on that column — so the erase after it takes the `m`.
        // ranger writes the screen's bottom-right cell this way.
        let stream = b"\x1b[?7labcdefghijklm\x1b[K\r\nnext\x1b[?7h\r\n0123456789wrap";
        fed_in_chunks(4, 10, stream, |s, size| {
            assert_eq!(
                s.snapshot().rows,
                vec!["abcdefghi", "next", "0123456789", "wrap"],
                "chunks of {size}"
            );
        });
    }

    #[test]
    fn with_autowrap_off_repeats_and_wide_characters_stay_on_the_row() {
        // A repeat runs up to the margin and no further; a wide character
        // with no room left is dropped, and a narrow one after it takes the
        // last column — over the right half of the wide one before it.
        fed_in_chunks(3, 10, b"\x1b[?7lab\x1b[20bZ\r\n", |s, size| {
            assert_eq!(s.snapshot().rows, vec!["abbbbbbbbZ"], "chunks of {size}");
        });
        fed_in_chunks(3, 10, "\x1b[?7labcdefgh中文X".as_bytes(), |s, size| {
            assert_eq!(s.snapshot().rows, vec!["abcdefgh X"], "chunks of {size}");
        });
    }

    #[test]
    fn tabs_move_to_the_stops_the_program_set() {
        // HTS sets a stop at the cursor, TBC clears the one there (`CSI g`)
        // or all of them (`CSI 3 g`); past the last stop a tab goes to the
        // last column.
        let stream = b"\x1b[3g\x1b[1;5H\x1bH\x1b[1;20H\x1bH\r\tA\tB\tC";
        fed_in_chunks(3, 40, stream, |s, size| {
            assert_eq!(
                s.snapshot().rows,
                vec![format!("{:4}A{:14}B{:19}C", "", "", "")],
                "chunks of {size}"
            );
        });
        fed_in_chunks(3, 40, b"\x1b[1;9H\x1b[g\r\tX\tY", |s, size| {
            assert_eq!(
                s.snapshot().rows,
                vec![format!("{:16}X{:7}Y", "", "")],
                "chunks of {size}"
            );
        });
    }

    #[test]
    fn back_and_forward_tabs_move_by_tab_stops() {
        // CBT (`CSI n Z`) and CHT (`CSI n I`): two stops back from past
        // the `B` lands on the `A`, three on from there on column 33.
        fed_in_chunks(3, 40, b"\tA\tB\x1b[2ZC\x1b[3ID\x1b[9ZE", |s, size| {
            assert_eq!(
                s.snapshot().rows,
                vec![format!("E{:7}C{:7}B{:15}D", "", "", "")],
                "chunks of {size}"
            );
        });
    }

    #[test]
    fn a_full_reset_restores_autowrap_and_the_tab_stops() {
        fed_in_chunks(
            3,
            20,
            b"\x1b[3g\x1b[?7l\x1bc\tX\r\n0123456789012345678901",
            |s, size| {
                assert_eq!(
                    s.snapshot().rows,
                    vec![
                        format!("{:8}X", ""),
                        "01234567890123456789".to_string(),
                        "01".to_string()
                    ],
                    "chunks of {size}"
                );
            },
        );
    }

    #[test]
    fn a_window_title_for_screen_or_tmux_is_not_text() {
        // `ESC k … ESC \` (or BEL) names a screen/tmux window. A program
        // that thinks it runs under one sends it — ranger does regardless —
        // and read as text the title landed at the cursor.
        fed_in_chunks(
            5,
            20,
            b"before\x1bkranger\x1b\\after\x1bkx\x07!",
            |s, size| {
                assert_eq!(s.snapshot().rows, vec!["beforeafter!"], "chunks of {size}");
            },
        );
    }

    fn wide() -> Screen {
        Screen::new(5, 60)
    }

    #[test]
    fn the_cursor_is_quoted_in_its_line() {
        // nano's save prompt, its file name filled in already: the caret
        // says typing goes after it — a model that could not see that typed
        // the name again and saved `sysinfo.shsysinfo.sh`.
        let mut s = wide();
        s.feed(b"\x1b[7mFile Name to Write: sysinfo.sh\x1b[27m");
        assert_eq!(
            s.snapshot().cursor_text.as_deref(),
            Some("File Name to Write: sysinfo.sh\u{2038}")
        );
        // vim's normal mode: on a character, the caret sits before it.
        s.feed(b"\r\nalpha beta\x1b[2;7H");
        assert_eq!(
            s.snapshot().cursor_text.as_deref(),
            Some("alpha \u{2038}beta")
        );
    }

    #[test]
    fn a_long_cursor_line_is_cut_around_the_cursor() {
        let mut s = Screen::new(5, 120);
        let line: String = ('a'..='z').cycle().take(110).collect();
        s.feed(line.as_bytes());
        s.feed(b"\x1b[1;61H");
        let text = s.snapshot().cursor_text.expect("a cursor line");
        let (before, after) = text.split_once('\u{2038}').expect("the caret");
        assert_eq!(before, format!("\u{2026}{}", &line[20..60]), "forty before");
        assert_eq!(after, format!("{}\u{2026}", &line[60..80]), "twenty after");
    }

    #[test]
    fn a_cut_cursor_line_drops_the_padding_at_its_cuts() {
        // dialog's buttons, far across a padded row: the cut lands in blank
        // cells, which say nothing beside the ellipsis.
        let mut s = Screen::new(5, 120);
        let row = format!(
            "{}\u{2502}   <  OK  >      <Cancel>{}\u{2502}",
            " ".repeat(40),
            " ".repeat(15)
        );
        s.feed(row.as_bytes());
        s.feed(b"\x1b[1;48H");
        assert_eq!(
            s.snapshot().cursor_text.as_deref(),
            Some("\u{2026}\u{2502}   <  \u{2038}OK  >      <Cancel>\u{2026}")
        );
    }

    #[test]
    fn a_blank_cursor_line_or_a_hidden_cursor_is_not_quoted() {
        let mut s = wide();
        s.feed(b"text\r\n   ");
        assert_eq!(s.snapshot().cursor_text, None, "nothing on the line");
        s.feed(b"\x1b[1;3H\x1b[?25l");
        let snap = s.snapshot();
        assert_eq!(snap.cursor_text, None, "no cursor to show");
        assert!(snap.cursor_hidden, "htop, mc and whiptail hide theirs");
    }

    #[test]
    fn an_alternate_screen_with_nothing_drawn_on_it_is_undrawn() {
        let mut s = screen();
        assert!(!s.undrawn(), "the main screen, blank or not");
        s.feed(b"\x1b[?1049h\x1b[?25l");
        assert!(s.undrawn(), "switched to, nothing drawn yet");
        s.feed(b"\x1b[44m\x1b[2J\x1b[5;1H   ");
        assert!(s.undrawn(), "a coloured blank is nothing to read");
        s.feed(b"\x1b[1;1HCPU");
        assert!(!s.undrawn(), "drawn");
    }

    #[test]
    fn text_after_a_string_ended_by_esc_backslash_is_text() {
        // A link (OSC 8), a title or a DCS ended by ST (`ESC \`): the `\`
        // ends the string, and what follows is text. Read as the start of an
        // escape sequence, `│D` of `│Disk` became IND — a line feed.
        fed_in_chunks(
            5,
            40,
            "\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\\u{2502}Disk \x1b]0;t\x1b\\\u{e9}t\u{e9} \x1bP1$r0m\x1b\\\u{2502}D"
                .as_bytes(),
            |s, size| {
                assert_eq!(
                    s.snapshot().rows,
                    vec!["link\u{2502}Disk \u{e9}t\u{e9} \u{2502}D"],
                    "chunks of {size}"
                );
            },
        );
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
        let streams: [&[u8]; 7] = [
            b"plain text\r\nand more\r\n",
            b"\x1b[1\n2mX\x1b[m after a control inside a sequence",
            b"\x1bP1$r0m\x1b\\dcs\x1b]0;title\x07osc \x1b]8;;u\x1b\\link\x1b]8;;\x1b\\",
            "\u{2502} wide \u{4e2d}\u{6587} and \u{00e9}\tat\tdefault\ttab stops".as_bytes(),
            b"\x1b[2;5Hmoved\x1b[1;1H\x1b[K\x1b[3@ins\x1b[2Pdel\r\n\x1b[?25l\x1b[?1049hA",
            b"\x1b[5;31;1mdense\x1b[0;4mu\x1b[24m\x1b[38;2;1;2;3mrgb\x1b[38:5:9mcolon",
            "\x1b]8;;u\x1b\\\u{2502}link\x1b]8;;\x1b\\\u{2502}D\x1bP1$r0m\x1b\\\u{e9}E".as_bytes(),
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
