//! Piped output, **folded** the way a terminal would show it
//! (`docs/interactive-shell.md`).
//!
//! A plain `bash` command's output reaches us through a pipe, not a
//! terminal, but plenty of programs draw on it as if it were one: curl's
//! progress meter, tqdm, ffmpeg and rsync redraw a line with `\r` whether or
//! not anyone is watching, and `--color=always` wraps words in escapes. Kept
//! raw, the model reads every frame a progress bar ever drew, joined on one
//! line. [`Fold`] replays the stream the way a terminal shows it — one line
//! at a time: `\r` returns to the start of the line, backspace and the
//! cursor-column escapes move along it, all overwriting; erase-in-line
//! applies; every other escape vanishes — and
//! hands out each line once it ends ([`Fold::take_settled`]), in its final
//! state, with the line still being drawn readable at any moment
//! ([`Fold::current`]).
//!
//! It is deliberately not the terminal-session [`Transcript`]: a piped
//! command's output is data the model may copy back — a `cat Makefile`, a
//! JSON line — so a tab stays a tab, trailing spaces stay, and a line is kept
//! whole up to the output cap rather than to a terminal's width. Cursor
//! movement between lines means nothing on a pipe and is ignored.
//!
//! [`Transcript`]: super::transcript::Transcript

/// The longest the line in progress may grow, in characters, by default —
/// the whole output's cap, which a single line can never usefully exceed.
const DEFAULT_LINE_CAP: usize = crate::llm::tools::TOOL_OUTPUT_MAX_BYTES;

/// See the module docs.
pub struct Fold {
    parser: vte::Parser,
    line: Line,
}

impl std::fmt::Debug for Fold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fold").finish_non_exhaustive()
    }
}

/// The line model the parser drives.
#[derive(Default)]
struct Line {
    /// The line in progress, one `char` per position.
    cells: Vec<char>,
    /// Where the next character lands.
    col: usize,
    /// Lines that ended since the last [`Fold::take_settled`], each with its
    /// `\n`.
    settled: String,
    /// The most characters the line in progress may hold.
    cap: usize,
    /// A character was dropped because the line was full.
    overflowed: bool,
}

impl Fold {
    #[must_use]
    pub fn new() -> Self {
        Self::with_line_cap(DEFAULT_LINE_CAP)
    }

    /// A fold whose line in progress stops growing at `cap` characters —
    /// what [`overflowed`](Self::overflowed) then reports.
    #[must_use]
    pub fn with_line_cap(cap: usize) -> Self {
        Self {
            parser: vte::Parser::new(),
            line: Line {
                cap,
                ..Line::default()
            },
        }
    }

    /// Fold a chunk of output in. Chunks may split anywhere — inside a UTF-8
    /// character or an escape sequence — the parser carries the partial state
    /// to the next call.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.line, bytes);
    }

    /// The lines that ended since the previous call, each ending in `\n`,
    /// in their final state.
    pub fn take_settled(&mut self) -> String {
        std::mem::take(&mut self.line.settled)
    }

    /// The line still being drawn, as it stands now.
    #[must_use]
    pub fn current(&self) -> String {
        self.line.cells.iter().collect()
    }

    /// Has a line outgrown the cap, losing characters?
    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.line.overflowed
    }

    /// Everything not taken yet — the settled lines, then the line still
    /// being drawn (no newline of its own) — the final flush once the
    /// command has exited.
    #[must_use]
    pub fn finish(mut self) -> String {
        let mut text = self.take_settled();
        text.push_str(&self.current());
        text
    }
}

impl Default for Fold {
    fn default() -> Self {
        Self::new()
    }
}

impl Line {
    /// Put `c` at the cursor — over what is there, or past the end with the
    /// gap padded — the way a terminal does.
    fn put(&mut self, c: char) {
        if self.col >= self.cap {
            self.overflowed = true;
            return;
        }
        if self.col < self.cells.len() {
            self.cells[self.col] = c;
        } else {
            self.cells.resize(self.col, ' ');
            self.cells.push(c);
        }
        self.col += 1;
    }

    /// The line ended: it is final.
    fn end_line(&mut self) {
        self.settled.extend(self.cells.drain(..));
        self.settled.push('\n');
        self.col = 0;
    }

    /// Erase in line: 0 cursor→end, 1 start→cursor, 2 the whole line.
    fn erase_in_line(&mut self, mode: u16) {
        match mode {
            0 => self.cells.truncate(self.col),
            1 => {
                let upto = (self.col + 1).min(self.cells.len());
                self.cells[..upto].fill(' ');
            }
            _ => self.cells.clear(),
        }
    }
}

impl vte::Perform for Line {
    fn print(&mut self, c: char) {
        self.put(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.end_line(),
            b'\r' => self.col = 0,
            0x08 => self.col = self.col.saturating_sub(1),
            b'\t' => self.put('\t'),
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
        if !intermediates.is_empty() {
            return;
        }
        let first = params
            .iter()
            .next()
            .and_then(|param| param.first().copied())
            .unwrap_or(0);
        // A count of 0 means 1, as a terminal reads it.
        let count = usize::from(first.max(1));
        match action {
            'K' => self.erase_in_line(first),
            'G' => self.col = count - 1,
            'C' => self.col += count,
            'D' => self.col = self.col.saturating_sub(count),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settled(bytes: &[u8]) -> String {
        let mut fold = Fold::new();
        fold.feed(bytes);
        fold.take_settled()
    }

    #[test]
    fn a_carriage_return_redraws_the_line_in_place() {
        // curl's meter, tqdm, ffmpeg: frames over one line, reaching the
        // model once, in their final state.
        assert_eq!(settled(b"  0%\r 45%\r100%\n"), "100%\n");
    }

    #[test]
    fn the_line_in_progress_shows_its_latest_frame() {
        let mut fold = Fold::new();
        fold.feed(b"Downloading  10%");
        assert_eq!(fold.current(), "Downloading  10%");
        fold.feed(b"\rDownloading  50%");
        assert_eq!(fold.current(), "Downloading  50%");
        assert_eq!(fold.take_settled(), "", "nothing finished yet");
        fold.feed(b"\rDownloading 100%\ndone\n");
        assert_eq!(fold.take_settled(), "Downloading 100%\ndone\n");
        assert_eq!(fold.current(), "");
        assert_eq!(fold.take_settled(), "", "each line is taken once");
    }

    #[test]
    fn crlf_line_endings_read_as_plain_lines() {
        assert_eq!(settled(b"a\r\nb\r\n"), "a\nb\n");
    }

    #[test]
    fn escapes_vanish_and_erase_in_line_applies() {
        assert_eq!(settled(b"\x1b[1;31mred\x1b[0m plain\n"), "red plain\n");
        assert_eq!(settled(b"\x1b]0;a title\x07text\n"), "text\n");
        assert_eq!(settled(b"abcdef\rxy\x1b[K\n"), "xy\n");
        assert_eq!(settled(b"abcdef\x1b[2K\rz\n"), "z\n");
    }

    #[test]
    fn moving_along_the_line_by_escape_redraws_like_a_carriage_return() {
        // `CSI G` (to a column), `CSI C`/`CSI D` (forward/back): what some
        // progress drawers use instead of `\r`.
        assert_eq!(settled(b" 10%\x1b[1G 99%\n"), " 99%\n");
        assert_eq!(settled(b"ab\x1b[G\x1b[3Cz\n"), "ab z\n");
        assert_eq!(settled(b"50%\x1b[3D99%\n"), "99%\n");
        assert_eq!(settled(b"x\x1b[9Dy\n"), "y\n", "never left of the line");
    }

    #[test]
    fn a_shorter_redraw_keeps_what_it_did_not_cover() {
        // A terminal's own rule — a program that means to shorten a line
        // erases the rest of it.
        assert_eq!(settled(b"abcdef\rxy\n"), "xycdef\n");
    }

    #[test]
    fn tabs_and_trailing_spaces_are_kept() {
        // What the model copies out of `cat Makefile` must still match the
        // file byte for byte.
        assert_eq!(
            settled(b"all:\n\tcc -o a a.c  \n"),
            "all:\n\tcc -o a a.c  \n"
        );
    }

    #[test]
    fn a_backspace_steps_back_over_what_it_overwrites() {
        assert_eq!(settled(b"working |\x08/\x08-\x08\\\n"), "working \\\n");
    }

    #[test]
    fn a_character_split_across_feeds_survives() {
        let mut fold = Fold::new();
        fold.feed(b"\xe6\x97");
        fold.feed(b"\xa5\n");
        assert_eq!(fold.take_settled(), "日\n");
    }

    #[test]
    fn bytes_that_are_not_utf8_read_as_replacement_characters() {
        // `String::from_utf8_lossy`'s answer, which the output had before it
        // was folded.
        assert_eq!(settled(b"a\xffb\n"), "a\u{FFFD}b\n");
    }

    #[test]
    fn finishing_flushes_the_line_in_progress() {
        let mut fold = Fold::new();
        fold.feed(b"a\nb 50%\rb 99%");
        assert_eq!(fold.finish(), "a\nb 99%");
    }

    #[test]
    fn an_endless_line_stops_growing_at_its_cap() {
        let mut fold = Fold::with_line_cap(8);
        fold.feed(&[b'x'; 20]);
        assert_eq!(fold.current(), "xxxxxxxx");
        assert!(fold.overflowed());
        assert!(!Fold::new().overflowed());
    }
}
