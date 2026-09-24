//! A terminal's output as **lines of text** — the view an interactive
//! session hands the model (`docs/interactive-shell.md`).
//!
//! A pseudo-terminal's byte stream is written for a screen: a progress bar
//! redraws one line with `\r`, a REPL repaints its prompt with column moves
//! and erases, colour codes wrap every other word. [`Transcript`] folds that
//! stream into the lines a person would read back — carriage returns and
//! backspaces overwrite, erase-in-line/-display and cursor moves apply,
//! every escape sequence that only styles or titles is dropped — using the
//! same `vte` grammar the screen emulator parses with, so the two views never
//! disagree about where a sequence ends.
//!
//! It is not a screen: lines are logical (never wrapped at the terminal's
//! width), content written while a program is on the **alternate screen** is
//! left to the screen view, and absolute cursor addressing is only
//! *noticed* ([`Transcript::screen_addressed`]) so the session can show the
//! screen instead for that stretch.
//!
//! Two readers take from it:
//!
//! - the **model** — [`Transcript::take_update`]: every line that is new or
//!   whose text changed since the previous look, in order — a prompt the
//!   model answered comes back with its answer echoed after it, and a
//!   progress bar redrawn in place comes back once, as it stands now, while
//!   the lines around it that did not change are not repeated;
//! - the **stream** — [`Transcript::take_committed`]: each line once the
//!   cursor has left it, for the running cell's tail and the interim-output
//!   file; [`Transcript::take_rest`] flushes the last line at exit.
//!
//! It also tells a **redrawn** line from a written one. The session cuts the
//! output into bursts ([`Transcript::new_burst`] — output close together in
//! time); a burst that changes the text an earlier one left on a line —
//! overwriting it after a `\r`, erasing it, extending it — redraws that
//! line. A line redrawn by two bursts since the program was last typed into
//! ([`Transcript::new_input`]) is **animated** — a progress bar, a spinner, a
//! counter — and never a prompt, whatever the cursor next to it looks like
//! ([`Transcript::cursor_line_animated`]).
//!
//! Bounded: at most [`MAX_RETAINED_LINES`] lines are kept (older unread ones
//! are counted into the next update's `omitted_lines`), and a line stops
//! growing at [`MAX_LINE_CHARS`] columns.

/// How many lines a transcript keeps at most. Lines both readers have passed
/// are let go sooner (minus a small reach-back window a cursor-up redraw can
/// still land in); past this, the oldest are dropped and counted.
pub const MAX_RETAINED_LINES: usize = 2000;

/// The widest a single line may grow, in columns — a program writing a
/// megabyte with no newline keeps a bounded line, not a bounded process.
pub const MAX_LINE_CHARS: usize = 8192;

/// What the model is handed at a look: the lines new or changed since its
/// previous one, and how many unread lines were dropped by the retention cap
/// in between.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Update {
    /// The text — lines right-trimmed, blank lines at either end dropped.
    /// Empty when nothing changed since the previous look.
    pub text: String,
    /// Unread lines the retention cap let go before this look.
    pub omitted_lines: usize,
}

/// What a waiting call streams to the running cell
/// ([`Transcript::take_stream`]): `settled` text to append for good, and the
/// `live` rows that replace the ones it streamed last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stream {
    /// Lines that scrolled out of the cursor's reach since the last take,
    /// each ending in `\n`.
    pub settled: String,
    /// The lines still in reach, as they stand — no trailing newline.
    pub live: String,
}

/// See the module docs.
pub struct Transcript {
    parser: vte::Parser,
    lines: Lines,
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Transcript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transcript").finish_non_exhaustive()
    }
}

impl Transcript {
    #[must_use]
    pub fn new() -> Self {
        Self {
            parser: vte::Parser::new(),
            lines: Lines {
                reach: usize::from(super::screen::ROWS),
                ..Lines::default()
            },
        }
    }

    /// Fold a chunk of the terminal's output in. Chunks may split anywhere —
    /// inside a UTF-8 character or an escape sequence — the parser carries
    /// the partial state to the next call.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.lines, bytes);
    }

    /// The model's look: every line that is new or changed since the
    /// previous look (see the module docs), marking them delivered.
    pub fn take_update(&mut self) -> Update {
        let lines = &mut self.lines;
        // A look consumes the addressing notice whether or not any text
        // changed — the session has shown the screen for that stretch.
        lines.addressed = false;
        if !lines.dirty && lines.omitted == 0 {
            return Update::default();
        }
        let mut changed = Vec::new();
        let start = lines.look.max(lines.first) - lines.first;
        for row in lines.rows.iter_mut().skip(start) {
            if !std::mem::take(&mut row.touched) {
                continue;
            }
            let text = row.text();
            let hash = hash_of(&text);
            if row.delivered != Some(hash) {
                row.delivered = Some(hash);
                changed.push(text);
            }
        }
        lines.look = lines.row;
        lines.stream_from = lines.reach_top();
        lines.stream_started = false;
        lines.dirty = false;
        let omitted_lines = std::mem::take(&mut lines.omitted);
        lines.trim();
        Update {
            text: trim_blank_lines(&changed.join("\n")),
            omitted_lines,
        }
    }

    /// The lines the cursor has left since the previous call, each ending in
    /// `\n` — the append-only stream.
    pub fn take_committed(&mut self) -> String {
        let lines = &mut self.lines;
        let start = lines.commit.max(lines.first);
        if start >= lines.row {
            return String::new();
        }
        let mut out = lines.render(start, lines.row);
        out.push('\n');
        lines.commit = lines.row;
        lines.trim();
        out
    }

    /// Everything the stream has not taken yet, the cursor's own line
    /// included — the final flush once the program has exited.
    pub fn take_rest(&mut self) -> String {
        let lines = &mut self.lines;
        let start = lines.commit.max(lines.first);
        let end = lines.end();
        lines.commit = end;
        if start >= end {
            return String::new();
        }
        let text = trim_trailing_blank_lines(&lines.render(start, end));
        if text.is_empty() {
            return text;
        }
        text + "\n"
    }

    /// A transcript whose screen is `reach` rows tall — how far back the
    /// cursor can go to redraw a line, and so how much of the tail stays
    /// **live** in [`take_stream`](Self::take_stream). [`new`](Self::new)
    /// uses the session's own screen height.
    #[must_use]
    pub fn with_reach(reach: usize) -> Self {
        let mut transcript = Self::new();
        transcript.lines.reach = reach.max(1);
        transcript
    }

    /// The stream a waiting call shows while the next look builds up (see
    /// the module docs): the lines that look would carry, split at the
    /// screen's reach — `settled` the ones that scrolled out of it since the
    /// previous call, final and never streamed again, and `live` the ones
    /// still in reach, as they stand now. Taking it changes nothing the look
    /// will say.
    pub fn take_stream(&mut self) -> Stream {
        let lines = &mut self.lines;
        let end = lines.first + lines.rows.len();
        let top = lines.reach_top();
        let mut settled = String::new();
        let from = lines.stream_from.max(lines.look).max(lines.first);
        for index in from..top {
            let Some(text) = lines.rows[index - lines.first].pending_text() else {
                continue;
            };
            if !lines.stream_started && text.is_empty() {
                continue;
            }
            lines.stream_started = true;
            settled.push_str(&text);
            settled.push('\n');
        }
        lines.stream_from = lines.stream_from.max(top);
        let mut live: Vec<String> = (top.max(lines.first)..end)
            .filter_map(|index| lines.rows[index - lines.first].pending_text())
            .collect();
        if !lines.stream_started {
            let blank = live.iter().take_while(|text| text.is_empty()).count();
            live.drain(..blank);
        }
        while live.last().is_some_and(String::is_empty) {
            live.pop();
        }
        Stream {
            settled,
            live: live.join("\n"),
        }
    }

    /// Start a new burst: output from here on that changes a line an
    /// earlier burst wrote is a redraw of it (see the module docs).
    pub fn new_burst(&mut self) {
        self.lines.close_burst();
        self.lines.burst += 1;
    }

    /// The program was typed into: what it draws from here on answers the
    /// keys, so the redraws counted so far no longer say it is animating.
    pub fn new_input(&mut self) {
        self.new_burst();
        self.lines.epoch += 1;
    }

    /// Is the line under the cursor **animated** — redrawn in place by at
    /// least [`ANIMATION_REDRAWS`] bursts since the program was last typed
    /// into? A progress bar or a spinner, never a prompt. Never on the
    /// alternate screen, which the lines do not follow.
    #[must_use]
    pub fn cursor_line_animated(&self) -> bool {
        let lines = &self.lines;
        if lines.alt {
            return false;
        }
        let Some(row) = lines
            .row
            .checked_sub(lines.first)
            .and_then(|at| lines.rows.get(at))
        else {
            return false;
        };
        let counted = if row.epoch == lines.epoch {
            row.redraws
        } else {
            0
        };
        let now = hash_of(&row.text());
        let open = lines
            .burst_before
            .iter()
            .any(|&(index, before)| index == lines.row && before != now);
        counted + u32::from(open) >= ANIMATION_REDRAWS
    }

    /// Is the program on the alternate screen (a full-screen program's
    /// canvas) right now?
    #[must_use]
    pub fn in_alt_screen(&self) -> bool {
        self.lines.alt
    }

    /// Has the program addressed the screen absolutely — moved the cursor to
    /// a row, cleared the display, set a scroll region — since the previous
    /// look? The lines are then a poor record, and the session shows the
    /// screen instead.
    #[must_use]
    pub fn screen_addressed(&self) -> bool {
        self.lines.addressed
    }
}

/// `text` without the blank lines a trailing newline (or an erased tail)
/// leaves at its end.
fn trim_trailing_blank_lines(text: &str) -> String {
    text.trim_end_matches(['\n', ' ']).to_string()
}

/// `text` without blank lines at either end — a look's lines are what
/// changed, and a blank line opening them says nothing.
fn trim_blank_lines(text: &str) -> String {
    trim_trailing_blank_lines(text.trim_start_matches('\n'))
}

/// A line's text, reduced to what a look compares it by.
fn hash_of(text: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::hash::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// The right half of a wide character: the column it covers, holding no
/// character of its own. A NUL can never be printed (it reaches `execute`,
/// not `print`), so it cannot collide with real content.
const WIDE_PAD: char = '\0';

/// How many lines above both readers' cursors stay retained, so a cursor-up
/// redraw (a multi-line progress display, a REPL repainting a wrapped input)
/// still has the lines it means to land on.
const REACH_BACK_LINES: usize = 64;

/// A tab stop every this many columns — a terminal's default.
const TAB_WIDTH: usize = 8;

/// How many bursts must redraw a line, since the program was last typed
/// into, before it counts as animated: one is a prompt redrawn once (a menu
/// answering a key, a REPL repainting after a terminal query), two is a
/// pattern.
pub const ANIMATION_REDRAWS: u32 = 2;

/// One line: its columns, and what the model and the animation count know
/// about it.
#[derive(Default)]
struct Row {
    /// One `char` per column ([`WIDE_PAD`] for a wide character's right half).
    cells: Vec<char>,
    /// Created or edited since the model's last look — its text may differ
    /// from what the model was handed.
    touched: bool,
    /// The text the model was last handed for this line, hashed — `None`
    /// until a look delivers it.
    delivered: Option<u64>,
    /// The burst that last edited it, so its text from before that burst is
    /// kept once.
    edited_in: Option<u64>,
    /// Bursts that redrew it, counted in the typing epoch `epoch`.
    redraws: u32,
    epoch: u64,
}

impl Row {
    /// A line the output just reached — news for the next look.
    fn new_line() -> Self {
        Self {
            touched: true,
            ..Self::default()
        }
    }

    /// Its text: wide-character halves dropped, right-trimmed.
    fn text(&self) -> String {
        let text: String = self.cells.iter().filter(|&&c| c != WIDE_PAD).collect();
        text.trim_end().to_string()
    }

    /// Its text if the next look would carry it — new, or changed since the
    /// model was last handed it.
    fn pending_text(&self) -> Option<String> {
        if !self.touched {
            return None;
        }
        let text = self.text();
        (self.delivered != Some(hash_of(&text))).then_some(text)
    }
}

/// The line model the parser drives: rows addressed absolutely (`first` is
/// the absolute index of `rows[0]`), the two readers' cursors, and the
/// bursts that tell a redrawn line from a written one.
#[derive(Default)]
struct Lines {
    rows: std::collections::VecDeque<Row>,
    /// The absolute index of `rows[0]` — how many rows were let go before it.
    first: usize,
    /// The cursor: an absolute row and a column.
    row: usize,
    col: usize,
    /// A saved cursor (`ESC 7`, `CSI s`, entering the alternate screen).
    saved: Option<(usize, usize)>,
    /// The model's cursor: no row before it has anything the model has not
    /// been handed — the cursor's row at the last look, or the first row
    /// touched since, if that is earlier.
    look: usize,
    /// The stream's cursor: the first row not yet taken.
    commit: usize,
    /// Did any content change since the model's last look?
    dirty: bool,
    /// Rows the model had not read that the retention cap let go.
    omitted: usize,
    /// On the alternate screen — content is the screen view's alone.
    alt: bool,
    /// Absolute addressing seen since the last look.
    addressed: bool,
    /// The current burst ([`Transcript::new_burst`]).
    burst: u64,
    /// The rows the current burst has edited that held text before it, with
    /// that text hashed — what closing the burst compares against.
    burst_before: Vec<(usize, u64)>,
    /// The typing epoch ([`Transcript::new_input`]).
    epoch: u64,
    /// The screen's height: the cursor reaches back no further, so a row
    /// above the last `reach` rows can never change again.
    reach: usize,
    /// The stream's cursor ([`Transcript::take_stream`]): the first row it
    /// has not settled yet.
    stream_from: usize,
    /// The stream has settled a line since the last look — from then on a
    /// blank line is part of the text, not a gap leading it.
    stream_started: bool,
}

impl Lines {
    /// One past the last retained row (the cursor's row always exists).
    fn end(&self) -> usize {
        (self.first + self.rows.len()).max(self.row + 1)
    }

    /// The first row still in the cursor's reach — the top of the screen,
    /// counted back from the last row.
    fn reach_top(&self) -> usize {
        (self.first + self.rows.len())
            .saturating_sub(self.reach)
            .max(self.first)
    }

    /// Make sure row `index` (absolute) exists, creating it — and any rows
    /// before it — as new lines.
    fn ensure(&mut self, index: usize) {
        while self.first + self.rows.len() <= index {
            self.look = self.look.min(self.first + self.rows.len());
            self.rows.push_back(Row::new_line());
            self.dirty = true;
        }
    }

    /// The cursor's row, to edit ([`edit_row`](Self::edit_row)).
    fn edit(&mut self) -> &mut Vec<char> {
        self.edit_row(self.row)
    }

    /// Row `index` (absolute), to edit: created when missing, its text from
    /// before this burst kept the first time the burst edits it, and marked
    /// for the next look.
    fn edit_row(&mut self, index: usize) -> &mut Vec<char> {
        self.ensure(index);
        self.look = self.look.min(index);
        self.dirty = true;
        let row = &mut self.rows[index - self.first];
        row.touched = true;
        if row.edited_in != Some(self.burst) {
            row.edited_in = Some(self.burst);
            let before = row.text();
            if !before.is_empty() {
                self.burst_before.push((index, hash_of(&before)));
            }
        }
        &mut row.cells
    }

    /// Close the current burst: every line it changed from the text an
    /// earlier burst left there counts one more redraw.
    fn close_burst(&mut self) {
        for (index, before) in std::mem::take(&mut self.burst_before) {
            let Some(row) = index
                .checked_sub(self.first)
                .and_then(|at| self.rows.get_mut(at))
            else {
                continue;
            };
            if hash_of(&row.text()) != before {
                if row.epoch != self.epoch {
                    row.epoch = self.epoch;
                    row.redraws = 0;
                }
                row.redraws += 1;
            }
        }
    }

    /// Rows `start..end` (absolute) as text, joined by newlines.
    fn render(&self, start: usize, end: usize) -> String {
        let mut out = String::new();
        for absolute in start..end {
            if absolute > start {
                out.push('\n');
            }
            if let Some(row) = absolute
                .checked_sub(self.first)
                .and_then(|index| self.rows.get(index))
            {
                out.push_str(&row.text());
            }
        }
        out
    }

    /// Let go of what nobody can read any more, and of the oldest rows past
    /// the retention cap (counting the ones the model had not read).
    fn trim(&mut self) {
        let floor = self
            .look
            .min(self.commit)
            .min(self.row)
            .saturating_sub(REACH_BACK_LINES);
        while self.first < floor && !self.rows.is_empty() {
            self.pop_front();
        }
        while self.rows.len() > MAX_RETAINED_LINES {
            self.pop_front();
        }
    }

    fn pop_front(&mut self) {
        if self.rows.pop_front().is_some_and(|row| row.touched) {
            self.omitted += 1;
        }
        self.first += 1;
        self.look = self.look.max(self.first);
        self.commit = self.commit.max(self.first);
        self.stream_from = self.stream_from.max(self.first);
        self.row = self.row.max(self.first);
        if let Some((row, _)) = &mut self.saved {
            *row = (*row).max(self.first);
        }
    }

    /// Put `c` at the cursor, the way a terminal does: overwrite, pad a gap
    /// with spaces, blank the other half of any wide character it lands on.
    fn put(&mut self, c: char) {
        use unicode_width::UnicodeWidthChar as _;
        let width = c.width().unwrap_or(0);
        if width == 0 || self.col + width > MAX_LINE_CHARS {
            return;
        }
        let col = self.col;
        let row = self.edit();
        if row.len() < col + width {
            row.resize(col + width, ' ');
        }
        split_wide_at(row, col);
        split_wide_at(row, col + width - 1);
        row[col] = c;
        if width == 2 {
            row[col + 1] = WIDE_PAD;
        }
        self.col += width;
        self.dirty = true;
    }

    /// A line feed: down one row, creating it at the end — and, now that a
    /// row may have been added, the retention cap applied.
    fn line_feed(&mut self) {
        self.row += 1;
        self.ensure(self.row);
        self.trim();
    }

    /// Move the cursor `n` rows up (negative) or down, never above the first
    /// retained row nor below the last.
    fn move_rows(&mut self, delta: isize) {
        let last = self.end() - 1;
        let target = self.row.saturating_add_signed(delta);
        self.row = target.clamp(self.first, last);
    }

    /// Erase in line: 0 cursor→end, 1 start→cursor, 2 the whole line.
    fn erase_in_line(&mut self, mode: u16) {
        let col = self.col;
        let row = self.edit();
        match mode {
            0 => {
                split_wide_at(row, col);
                row.truncate(col);
            }
            1 => {
                let upto = (col + 1).min(row.len());
                split_wide_at(row, col);
                row[..upto].fill(' ');
            }
            _ => row.clear(),
        }
        self.dirty = true;
    }

    /// Erase in display. Only "cursor to the end" is a line edit here (a REPL
    /// clearing below its redrawn prompt, a menu about to redraw its
    /// options) — the lines below go blank, as they do on a screen, so what
    /// is drawn there again is compared with what was there; "start to
    /// cursor" blanks the line up to the cursor; the rest clear the
    /// *screen*, which is the screen view's business.
    fn erase_in_display(&mut self, mode: u16) {
        match mode {
            0 => {
                self.erase_in_line(0);
                for index in self.row + 1..self.first + self.rows.len() {
                    self.edit_row(index).clear();
                }
            }
            1 => self.erase_in_line(1),
            _ => self.addressed = true,
        }
        self.dirty = true;
    }

    /// Delete `n` characters at the cursor, pulling the rest of the line left.
    fn delete_chars(&mut self, n: usize) {
        let col = self.col;
        let row = self.edit();
        if col < row.len() {
            split_wide_at(row, col);
            let end = (col + n).min(row.len());
            split_wide_at(row, end);
            row.drain(col..end);
            self.dirty = true;
        }
    }

    /// Insert `n` blanks at the cursor, pushing the rest of the line right.
    fn insert_blanks(&mut self, n: usize) {
        let col = self.col;
        let row = self.edit();
        if col < row.len() {
            split_wide_at(row, col);
            let n = n.min(MAX_LINE_CHARS.saturating_sub(row.len()));
            row.splice(col..col, std::iter::repeat_n(' ', n));
            self.dirty = true;
        }
    }

    /// Blank `n` characters from the cursor, in place.
    fn erase_chars(&mut self, n: usize) {
        let col = self.col;
        let row = self.edit();
        if col < row.len() {
            let end = (col + n).min(row.len());
            split_wide_at(row, col);
            split_wide_at(row, end.saturating_sub(1));
            row[col..end].fill(' ');
            self.dirty = true;
        }
    }

    fn save_cursor(&mut self) {
        self.saved = Some((self.row, self.col));
    }

    fn restore_cursor(&mut self) {
        if let Some((row, col)) = self.saved {
            self.row = row.max(self.first);
            self.col = col;
        }
    }

    /// DECSET/DECRST: only the alternate-screen modes matter here — 1049
    /// saves the cursor on the way in and restores it on the way out, like
    /// the terminal it describes.
    fn private_mode(&mut self, params: &vte::Params, set: bool) {
        for mode in params.iter().filter_map(|p| p.first().copied()) {
            if matches!(mode, 1049 | 1047 | 47) {
                if set && !self.alt {
                    if mode == 1049 {
                        self.save_cursor();
                    }
                    self.alt = true;
                } else if !set && self.alt {
                    self.alt = false;
                    if mode == 1049 {
                        self.restore_cursor();
                    }
                }
            }
        }
    }
}

/// If column `col` of `row` is half of a wide character, blank both halves —
/// what a terminal does when something is written over one of them.
fn split_wide_at(row: &mut [char], col: usize) {
    let Some(&cell) = row.get(col) else {
        return;
    };
    if cell == WIDE_PAD {
        row[col] = ' ';
        if col > 0 {
            row[col - 1] = ' ';
        }
    } else if row.get(col + 1) == Some(&WIDE_PAD) {
        row[col] = ' ';
        row[col + 1] = ' ';
    }
}

/// The first parameter of a sequence, or `default` when absent or zero —
/// the ECMA-48 rule for counts and positions.
fn first_param(params: &vte::Params, default: u16) -> u16 {
    match params.iter().next().and_then(|p| p.first().copied()) {
        None | Some(0) => default,
        Some(n) => n,
    }
}

impl vte::Perform for Lines {
    fn print(&mut self, c: char) {
        if !self.alt {
            self.put(c);
        }
    }

    fn execute(&mut self, byte: u8) {
        if self.alt {
            return;
        }
        match byte {
            b'\n' | 0x0b | 0x0c => self.line_feed(),
            b'\r' => self.col = 0,
            0x08 => self.col = self.col.saturating_sub(1),
            b'\t' => self.col = ((self.col / TAB_WIDTH + 1) * TAB_WIDTH).min(MAX_LINE_CHARS),
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        if intermediates.first() == Some(&b'?') {
            match action {
                'h' => self.private_mode(params, true),
                'l' => self.private_mode(params, false),
                _ => {}
            }
            return;
        }
        if self.alt || !intermediates.is_empty() {
            return;
        }
        let n = usize::from(first_param(params, 1));
        match action {
            'K' => self.erase_in_line(first_param(params, 0)),
            'J' => self.erase_in_display(first_param(params, 0)),
            'G' | '`' => self.col = n - 1,
            'C' | 'a' => self.col = (self.col + n).min(MAX_LINE_CHARS),
            'D' => self.col = self.col.saturating_sub(n),
            'A' => self.move_rows(-(n as isize)),
            'B' | 'e' => self.move_rows(n as isize),
            'E' => {
                self.move_rows(n as isize);
                self.col = 0;
            }
            'F' => {
                self.move_rows(-(n as isize));
                self.col = 0;
            }
            'H' | 'f' => {
                // Absolute addressing: the column still applies to the
                // line in hand, the row is the screen view's to honour.
                let col = params
                    .iter()
                    .nth(1)
                    .and_then(|p| p.first().copied())
                    .filter(|&c| c > 0)
                    .unwrap_or(1);
                self.col = usize::from(col) - 1;
                self.addressed = true;
            }
            'd' | 'r' | 'S' | 'T' | 'L' | 'M' => self.addressed = true,
            'P' => self.delete_chars(n),
            '@' => self.insert_blanks(n),
            'X' => self.erase_chars(n),
            's' => self.save_cursor(),
            'u' => self.restore_cursor(),
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if self.alt || !intermediates.is_empty() {
            return;
        }
        match byte {
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'M' => self.move_rows(-1),
            b'D' => self.line_feed(),
            b'E' => {
                self.col = 0;
                self.line_feed();
            }
            b'c' => {
                self.addressed = true;
                self.col = 0;
                self.line_feed();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fed(bytes: &[u8]) -> Transcript {
        let mut t = Transcript::new();
        t.feed(bytes);
        t
    }

    fn update(bytes: &[u8]) -> String {
        fed(bytes).take_update().text
    }

    #[test]
    fn plain_lines_read_back_as_lines() {
        assert_eq!(update(b"hello\r\nworld\r\n"), "hello\nworld");
    }

    #[test]
    fn a_carriage_return_overwrites_so_a_progress_bar_keeps_its_last_state() {
        assert_eq!(
            update(b"Downloading 10%\rDownloading 50%\rDownloading 100%\r\ndone\r\n"),
            "Downloading 100%\ndone"
        );
    }

    #[test]
    fn a_shorter_overwrite_keeps_the_rest_of_the_line_unless_erased() {
        assert_eq!(update(b"abcdef\rxy\r\n"), "xycdef", "a terminal's own rule");
        assert_eq!(update(b"abcdef\rxy\x1b[K\r\n"), "xy");
        assert_eq!(update(b"abcdef\x1b[3D\x1b[1K\r\n"), "    ef");
        assert_eq!(update(b"abcdef\x1b[2K\rz\r\n"), "z");
    }

    #[test]
    fn backspace_steps_back_over_what_it_overwrites() {
        assert_eq!(update(b"abc\x08\x08XY\r\n"), "aXY");
        // `man`'s overstrike bold: the letter, a backspace, the letter again.
        assert_eq!(update(b"N\x08NA\x08AM\x08ME\x08E\r\n"), "NAME");
    }

    #[test]
    fn colour_titles_and_hyperlinks_vanish() {
        assert_eq!(update(b"\x1b[1;31mred\x1b[0m plain\r\n"), "red plain");
        assert_eq!(update(b"\x1b]0;my title\x07text\r\n"), "text");
        assert_eq!(
            update(b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\\r\n"),
            "link"
        );
        assert_eq!(update(b"\x1bP1$r0m\x1b\\after\r\n"), "after", "a DCS reply");
    }

    #[test]
    fn a_node_style_prompt_redraw_reads_as_the_prompt_and_what_was_typed() {
        let bytes = b"Welcome\r\n\x1b[1G\x1b[0J> \x1b[3Glet x = 1\r\r\n\x1b[90mundefined\x1b[39m\r\n\x1b[1G\x1b[0J> \x1b[3G";
        assert_eq!(update(bytes), "Welcome\n> let x = 1\nundefined\n>");
    }

    #[test]
    fn column_moves_and_character_edits_apply_within_the_line() {
        assert_eq!(update(b"abc\x1b[1Gz\r\n"), "zbc", "CHA");
        assert_eq!(update(b"abc\x1b[2DZ\r\n"), "aZc", "CUB");
        assert_eq!(update(b"a\x1b[3Cb\r\n"), "a   b", "CUF pads");
        assert_eq!(update(b"abcdef\x1b[1G\x1b[2P\r\n"), "cdef", "DCH");
        assert_eq!(update(b"abc\x1b[1G\x1b[2@\r\n"), "  abc", "ICH");
        assert_eq!(update(b"abcdef\x1b[2G\x1b[3X\r\n"), "a   ef", "ECH");
    }

    #[test]
    fn a_tab_advances_to_the_next_stop() {
        assert_eq!(update(b"a\tb\r\n"), "a       b");
        assert_eq!(update(b"12345678\tx\r\n"), "12345678        x");
    }

    #[test]
    fn cursor_up_lets_a_multi_line_progress_display_redraw_in_place() {
        let bytes = b"a: 0%\r\nb: 0%\r\n\x1b[2A\ra: 50%\x1b[K\r\nb: 50%\x1b[K\r\n";
        assert_eq!(update(bytes), "a: 50%\nb: 50%");
        // Save/restore cursor does the same job.
        let bytes = b"x\r\n\x1b7one\r\ntwo\x1b8ONE\r\n";
        assert_eq!(update(bytes), "x\nONE\ntwo");
    }

    #[test]
    fn wide_characters_take_two_columns() {
        assert_eq!(update("日本\r\n".as_bytes()), "日本");
        // Overwriting half of a wide character blanks its other half.
        assert_eq!(update("日本\rx\r\n".as_bytes()), "x 本");
        assert_eq!(update("日本\x1b[2Gx\r\n".as_bytes()), " x本");
    }

    #[test]
    fn a_character_split_across_chunks_survives() {
        let mut t = Transcript::new();
        t.feed(b"\xe6\x97");
        t.feed(b"\xa5\r\n");
        assert_eq!(t.take_update().text, "日");
    }

    #[test]
    fn an_escape_sequence_split_across_chunks_survives() {
        let mut t = Transcript::new();
        t.feed(b"red\x1b[3");
        t.feed(b"1m text\r\n");
        assert_eq!(t.take_update().text, "red text");
    }

    #[test]
    fn a_look_starts_at_the_line_the_cursor_sat_on_at_the_previous_one() {
        // The prompt the model answered comes back with its answer echoed
        // after it — a terminal transcript, not a fragment.
        let mut t = fed(b"Python 3.11\r\n>>> ");
        assert_eq!(t.take_update().text, "Python 3.11\n>>>");
        t.feed(b"print(1)\r\n1\r\n>>> ");
        assert_eq!(t.take_update().text, ">>> print(1)\n1\n>>>");
    }

    #[test]
    fn a_look_with_nothing_new_is_empty() {
        let mut t = fed(b"$ ");
        assert_eq!(t.take_update().text, "$");
        assert_eq!(t.take_update(), Update::default(), "nothing changed");
    }

    #[test]
    fn a_look_after_a_finished_line_does_not_repeat_it() {
        let mut t = fed(b"one\r\n");
        assert_eq!(t.take_update().text, "one");
        t.feed(b"two\r\n");
        assert_eq!(t.take_update().text, "two");
    }

    #[test]
    fn a_look_repeats_no_line_that_did_not_change() {
        // pacman's download bars: the finished ones stay put while the
        // cursor parks on the one still moving and redraws it in place.
        let mut t = fed(b" core     100%\r\n extra      10%\r\n multilib 100%\r\n");
        assert_eq!(
            t.take_update().text,
            " core     100%\n extra      10%\n multilib 100%"
        );
        t.feed(b"\x1b[2F extra      50%\r");
        assert_eq!(
            t.take_update().text,
            " extra      50%",
            "the bar above the previous look's cursor, and only it"
        );
        t.feed(b" extra     100%\r");
        assert_eq!(
            t.take_update().text,
            " extra     100%",
            "not the unchanged bar below the cursor"
        );
    }

    #[test]
    fn a_line_repainted_unchanged_is_not_repeated() {
        let mut t = fed(b"$ ");
        assert_eq!(t.take_update().text, "$");
        t.feed(b"\r\x1b[K$ ");
        assert_eq!(t.take_update(), Update::default());
    }

    #[test]
    fn a_block_redrawn_below_an_erase_repeats_only_what_changed() {
        // An arrow-key menu answering a key: back up over its options, clear
        // below, draw them again with the highlight moved.
        let mut t = fed(b"? Pick one\r\n> Apple\r\n  Banana\r\n  Cherry\r\n");
        let _ = t.take_update();
        t.feed(b"\x1b[3F\x1b[J  Apple\r\n> Banana\r\n  Cherry\r\n");
        assert_eq!(t.take_update().text, "  Apple\n> Banana");
    }

    #[test]
    fn blank_lines_between_new_lines_are_kept_but_none_lead_a_look() {
        assert_eq!(update(b"a\r\n\r\nb\r\n"), "a\n\nb");
        let mut t = fed(b"one\r\n");
        let _ = t.take_update();
        t.feed(b"\r\n\r\ntwo\r\n");
        assert_eq!(t.take_update().text, "two");
    }

    fn stream(t: &mut Transcript) -> (String, String) {
        let Stream { settled, live } = t.take_stream();
        (settled, live)
    }

    #[test]
    fn a_stream_settles_what_scrolled_out_of_reach_and_keeps_the_rest_live() {
        // A screen three rows tall: the cursor can reach back two rows at
        // most, so anything further up is final — streamed once, appended —
        // while what is in reach may still be redrawn and is replaced whole.
        let mut t = Transcript::with_reach(3);
        t.feed(b"one\r\ntwo\r\n");
        assert_eq!(stream(&mut t), (String::new(), "one\ntwo".to_string()));
        t.feed(b"three\r\nfour\r\n");
        assert_eq!(
            stream(&mut t),
            ("one\ntwo\n".to_string(), "three\nfour".to_string())
        );
        // A redraw in reach changes the live rows only.
        t.feed(b"\x1b[1F four!\r\n");
        assert_eq!(stream(&mut t), (String::new(), "three\n four!".to_string()));
        assert_eq!(stream(&mut t), (String::new(), "three\n four!".to_string()));
    }

    #[test]
    fn a_stream_carries_only_what_the_next_look_would() {
        // pacman's bars: the look handed the model all three; afterwards the
        // stream shows the one that moved, never the unchanged ones.
        let mut t = fed(b" core     100%\r\n extra      10%\r\n multilib 100%\r\n");
        let _ = t.take_update();
        assert_eq!(stream(&mut t), (String::new(), String::new()));
        t.feed(b"\x1b[2F extra      50%\r");
        assert_eq!(
            stream(&mut t),
            (String::new(), " extra      50%".to_string())
        );
        t.feed(b" extra     100%\r");
        assert_eq!(
            stream(&mut t),
            (String::new(), " extra     100%".to_string())
        );
        assert_eq!(t.take_update().text, " extra     100%");
    }

    #[test]
    fn a_stream_never_repeats_a_settled_line_and_starts_again_after_a_look() {
        let mut t = Transcript::with_reach(2);
        t.feed(b"\r\na\r\nb\r\nc\r\n");
        let (settled, live) = stream(&mut t);
        assert_eq!(settled, "a\nb\n", "no blank line leads the stream");
        assert_eq!(live, "c");
        t.feed(b"d\r\n");
        assert_eq!(stream(&mut t), ("c\n".to_string(), "d".to_string()));
        assert_eq!(t.take_update().text, "a\nb\nc\nd");
        t.feed(b"e\r\n");
        assert_eq!(
            stream(&mut t),
            (String::new(), "e".to_string()),
            "the look delivered `d`: settling it later streams nothing"
        );
    }

    #[test]
    fn a_line_changed_in_place_by_two_later_bursts_is_animated() {
        let mut t = fed(b" 10% [#---] ");
        assert!(!t.cursor_line_animated(), "a fresh line");
        t.new_burst();
        t.feed(b"\r 20% [##--] ");
        assert!(!t.cursor_line_animated(), "one redraw is no pattern yet");
        t.new_burst();
        t.feed(b"\r 30% [###-] ");
        assert!(t.cursor_line_animated());
        t.new_burst();
        t.feed(b"\r\nContinue? [Y/n] ");
        assert!(!t.cursor_line_animated(), "a fresh line after it is not");
    }

    #[test]
    fn a_line_growing_across_bursts_is_animated() {
        let mut t = fed(b"Downloading .");
        for _ in 0..2 {
            t.new_burst();
            t.feed(b".");
        }
        assert!(t.cursor_line_animated());
    }

    #[test]
    fn writes_within_one_burst_or_unchanged_repaints_are_no_animation() {
        let mut t = Transcript::new();
        t.feed(b"Na");
        t.feed(b"me? ");
        t.feed(b"\rName? ");
        assert!(!t.cursor_line_animated(), "one burst, however many writes");
        for _ in 0..3 {
            t.new_burst();
            t.feed(b"\r\x1b[KName? ");
        }
        assert!(!t.cursor_line_animated(), "repainted, never changed");
    }

    #[test]
    fn typing_into_the_program_starts_the_count_again() {
        // A menu moves its highlight once per key it is sent: only the
        // redraws nobody asked for make an animation.
        let mut t = fed(b"? Pick: Apple");
        for fruit in ["Banana", "Cherry"] {
            t.new_burst();
            t.feed(format!("\r? Pick: {fruit}").as_bytes());
        }
        assert!(t.cursor_line_animated());
        t.new_input();
        t.feed(b"\r? Pick: Durian");
        assert!(!t.cursor_line_animated(), "one redraw since the key");
    }

    #[test]
    fn the_alternate_screen_is_left_to_the_screen_view() {
        let mut t = fed(b"before\r\n\x1b[?1049h\x1b[H\x1b[2Jvim stuff");
        assert!(t.in_alt_screen());
        t.feed(b"\x1b[?1049lafter\r\n");
        assert!(!t.in_alt_screen());
        assert_eq!(t.take_update().text, "before\nafter");
    }

    #[test]
    fn absolute_addressing_is_noticed_until_the_next_look() {
        let mut t = fed(b"plain\r\n");
        assert!(!t.screen_addressed());
        t.feed(b"\x1b[5;10Hx");
        assert!(t.screen_addressed());
        let _ = t.take_update();
        assert!(!t.screen_addressed(), "a look resets it");
        t.feed(b"\x1b[H\x1b[2J");
        assert!(t.screen_addressed(), "clearing the display counts");
        let _ = t.take_update();
        t.feed(b"\x1b[3;20r");
        assert!(t.screen_addressed(), "a scroll region counts");
        let _ = t.take_update();
        t.feed(b"\x1b[?1049h\x1b[5;10Hx\x1b[?1049l");
        assert!(
            !t.screen_addressed(),
            "the alternate screen's own addressing is the screen view's business"
        );
    }

    #[test]
    fn the_stream_takes_each_line_once_the_cursor_has_left_it() {
        let mut t = fed(b"one\r\ntwo");
        assert_eq!(t.take_committed(), "one\n");
        assert_eq!(t.take_committed(), "", "nothing new has been left");
        t.feed(b"\r\nthree\r\n>>> ");
        assert_eq!(t.take_committed(), "two\nthree\n");
        assert_eq!(t.take_rest(), ">>>\n");
        assert_eq!(t.take_rest(), "", "the rest is taken once");
    }

    #[test]
    fn the_two_readers_do_not_disturb_each_other() {
        let mut t = fed(b"a\r\nb\r\n");
        assert_eq!(t.take_committed(), "a\nb\n");
        assert_eq!(t.take_update().text, "a\nb", "the model still sees it all");
    }

    #[test]
    fn unread_lines_past_the_retention_cap_are_counted_not_kept() {
        let mut t = Transcript::new();
        for i in 0..(MAX_RETAINED_LINES + 500) {
            t.feed(format!("line {i}\r\n").as_bytes());
        }
        let update = t.take_update();
        assert!(update.omitted_lines >= 500, "{}", update.omitted_lines);
        assert!(
            update
                .text
                .ends_with(&format!("line {}", MAX_RETAINED_LINES + 499)),
            "the newest lines are kept"
        );
        assert!(
            update.text.lines().count() <= MAX_RETAINED_LINES,
            "{} lines kept",
            update.text.lines().count()
        );
    }

    #[test]
    fn an_endless_line_stops_growing() {
        let mut t = Transcript::new();
        let chunk = vec![b'x'; 4096];
        for _ in 0..8 {
            t.feed(&chunk);
        }
        assert_eq!(t.take_update().text.len(), MAX_LINE_CHARS);
    }
}
