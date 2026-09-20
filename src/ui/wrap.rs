//! Width math and the wrapping primitives every renderer shares.
//!
//! All width math goes through [`cols`] (display columns via `unicode-width`),
//! never `chars().count()`, so CJK and emoji wrap and pad correctly.
//! [`wrap_text`] is greedy and **prefix-stable** — appending text only ever
//! changes the last wrapped line, which is what makes streaming to scrollback
//! safe (see `docs/design.md`).

use super::theme::*;
use super::*;
use std::ops::Range;

/// Display width of `s` in terminal columns.
///
/// All width math in this module goes through this instead of `chars().count()`:
/// CJK and many emoji are two columns wide and combining marks are zero, so a
/// raw char count would wrap and pad non-ASCII text incorrectly.
pub(super) fn cols(s: &str) -> usize {
    s.width()
}

/// Greedy word-wrap `text` to `width` columns.
///
/// - Existing `'\n'`s are honoured (and blank lines preserved).
/// - Words longer than `width` are hard-broken across lines.
/// - `width == 0` disables wrapping (text is only split on `'\n'`).
///
/// Crucially this is *prefix-stable*: appending more text only ever changes the
/// last produced line, which is what lets streaming commit completed lines to
/// scrollback (see `main.rs`).
#[must_use]
pub fn wrap_text(text: &str, width: u16) -> Vec<String> {
    if width == 0 {
        return text.split('\n').map(str::to_string).collect();
    }
    let width = width as usize;
    let mut out = Vec::new();
    for segment in text.split('\n') {
        if segment.split_whitespace().next().is_none() {
            out.push(String::new()); // blank line
            continue;
        }
        out.extend(wrap_segment(segment, width));
    }
    out
}

/// Greedy-wrap a single newline-free segment that has at least one word.
///
/// All length comparisons are in display columns (see [`cols`]), so wide CJK
/// glyphs count as two and zero-width marks as zero.
fn wrap_segment(segment: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0; // display width of `cur` in columns

    for word in segment.split_whitespace() {
        let word_w = cols(word);

        if word_w > width {
            // Hard-break a word that can't fit on any line, splitting on
            // **grapheme** boundaries (like `textarea::place_word`) so a ZWJ
            // emoji cluster is never severed mid-joiner; measured in columns —
            // a single cluster that overflows a narrow line is placed alone (a
            // grapheme can't be split further).
            if cur_w > 0 {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            for g in word.graphemes(true) {
                let g_w = cols(g);
                if cur_w > 0 && cur_w + g_w > width {
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur.push_str(g);
                cur_w += g_w;
            }
            continue;
        }

        let needed = if cur_w == 0 {
            word_w
        } else {
            cur_w + 1 + word_w
        };
        if needed > width {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
            cur_w = word_w;
        } else {
            if cur_w > 0 {
                cur.push(' ');
                cur_w += 1;
            }
            cur.push_str(word);
            cur_w += word_w;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Wrap `text` to `width` columns **preserving whitespace verbatim** — the
/// counterpart of [`wrap_text`] for pre-formatted output (a tool's captured
/// stdout: `ls -l` columns, `tree` guides, indented code), where collapsing
/// space runs would destroy the alignment. Each `'\n'`-separated line keeps
/// its bytes exactly; a line wider than `width` is hard-broken on grapheme
/// boundaries, measured in display columns like [`wrap_segment`]'s hard break
/// (an overflowing cluster is placed alone — it can't be split further).
/// `width == 0` disables wrapping, like [`wrap_text`]. Used for **diff
/// bodies** (code — inline peek and Ctrl+O alike, coloured by source line);
/// command/shell output word-wraps via [`wrap_output`] instead, and
/// **messages** keep [`wrap_text`].
pub(super) fn wrap_verbatim(text: &str, width: u16) -> Vec<String> {
    WrapMode::Verbatim.wrap(text, width)
}

/// Wrap `text` to `width` columns at **word boundaries, preserving
/// whitespace** — the middle ground between [`wrap_text`] (word boundaries but
/// *collapses* space runs) and [`wrap_verbatim`] (preserves whitespace but
/// hard-breaks *mid-word*). A tool's captured output wants both: prose errors
/// (`sudo: …`) should break cleanly at spaces, yet a line's exact spaces must
/// survive so `ls -l` columns / indentation that already fit are untouched —
/// only an over-wide line reflows. The boundary space stays at the end of the
/// current row when it fits there; a run that overflows an exactly-full row is
/// consumed at the break, so continuation rows always start at a word and no
/// row is ever the boundary whitespace alone. A single word wider than `width` is
/// hard-broken on grapheme boundaries, measured in display columns (like
/// [`wrap_segment`]'s hard break). `width == 0` disables wrapping. Used by the
/// command/shell peek ([`result_peek_block`]), the running tail
/// ([`running_command_lines`]), and the Ctrl+O output ([`tool_full_lines`]),
/// so the three wrap identically.
pub(super) fn wrap_output(text: &str, width: u16) -> Vec<String> {
    WrapMode::Output.wrap(text, width)
}

/// [`wrap_output`] with a **hanging indent**: the first row of `text`'s first
/// line wraps to `first` columns — the room left beside a lead the caller
/// prints on that row (a tool header's `● {name}(`) — and every other row,
/// the later lines' included, to `rest`, the room left beside the indent they
/// are printed at. Equal widths are exactly [`wrap_output`].
///
/// A first word too wide for `first` but not for `rest` leaves the first row
/// **empty** and opens the continuation whole, instead of being hard-broken
/// across the two rows (`(repoName` / `: "…"` — the reported MCP header at
/// forty columns): the caller's lead then stands alone on its row and the
/// arguments spill under it. See
/// [`tool_header_lines`](super::tool::tool_header_lines).
pub(super) fn wrap_output_hanging(text: &str, first: u16, rest: u16) -> Vec<String> {
    let mut lines = text.split('\n');
    let mut out = Vec::new();
    if let Some(head) = lines.next() {
        out.extend(hanging_first_line(head, first, rest));
    }
    for line in lines {
        out.extend(WrapMode::Output.wrap(line, rest));
    }
    out
}

/// The rows of [`wrap_output_hanging`]'s first (`'\n'`-free) line: its first
/// row at `first`, every later row at `rest` — one scan at the two widths
/// ([`WrapMode::scan_hanging`]), so the decision to fill the first row with
/// an over-wide token knows the width of the rows it would continue on.
fn hanging_first_line(line: &str, first: u16, rest: u16) -> Vec<String> {
    if first == 0 || rest == 0 {
        return vec![line.to_string()];
    }
    if first == rest {
        return WrapMode::Output.wrap(line, rest);
    }
    // The spill: the leading word fits a continuation row but not the first.
    let head = line.split(char::is_whitespace).next().unwrap_or_default();
    let head_w = cols(head);
    if head_w > usize::from(first) && head_w <= usize::from(rest) {
        let mut rows = vec![String::new()];
        rows.extend(WrapMode::Output.wrap(line, rest));
        return rows;
    }
    let mut rows = Vec::new();
    WrapMode::Output.scan_hanging(line, usize::from(first), usize::from(rest), &mut |r| {
        rows.push(line[r].to_string());
    });
    rows
}

/// Which wrapper a collapsed cell measures and builds its rows with — the
/// pairing of [`wrap_output`] / [`wrap_verbatim`] with the row **count** that
/// has to agree with them, kept in one type so a `+N lines` hint can never
/// count rows a *different* wrapper would have produced
/// (`docs/long-lines.md`). Both ride one range-emitting scan per mode, so
/// counting builds no rows at all — which is what lets the running tail's
/// footer measure the whole retained buffer every animation frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum WrapMode {
    /// Word boundaries, whitespace preserved — command and shell output
    /// ([`wrap_output`]).
    Output,
    /// Hard break, whitespace preserved — diff bodies and code
    /// ([`wrap_verbatim`]).
    Verbatim,
}

impl WrapMode {
    /// Emit the byte range of every display row one `'\n'`-free `line` wraps to
    /// at `width` (> 0) columns — the single scan behind [`Self::wrap`] and
    /// [`Self::rows`]. Measured per grapheme cluster in display columns like
    /// every other width decision in this module.
    fn scan(self, line: &str, width: usize, emit: &mut dyn FnMut(Range<usize>)) {
        self.scan_hanging(line, width, width, emit);
    }

    /// [`Self::scan`] with a **hanging indent**: the first row has `first`
    /// columns, every later row `rest` (both > 0, `first <= rest`) — the two
    /// budgets a tool header's rows have ([`wrap_output_hanging`]).
    fn scan_hanging(
        self,
        line: &str,
        first: usize,
        rest: usize,
        emit: &mut dyn FnMut(Range<usize>),
    ) {
        let mut start = 0usize; // byte index the current row opens at
        let mut cur_w = 0usize; // display width of `line[start..idx]`
        let mut brk: Option<usize> = None; // byte index past the last space
        let mut idx = 0usize; // byte index just past the graphemes consumed
        let mut emitted = false; // whether any row has been emitted yet
        let mut width = first; // the current row's budget
        let mut graphemes = line.grapheme_indices(true).peekable();
        while let Some((_, g)) = graphemes.next() {
            let g_w = cols(g);
            let is_space = g.chars().all(char::is_whitespace);
            if cur_w > 0 && cur_w + g_w > width {
                match (self, brk) {
                    // The row is exactly full and the next thing is
                    // whitespace: the row ends here and the whitespace run is
                    // **consumed at the break**, so the continuation starts at
                    // a word. Carrying the run over used to leave the boundary
                    // space at the head of the next row — `total` / ` 12` —
                    // or, before another full-width word, as a row of its own:
                    // a phantom blank row inside `hello` / ` ` / `world` that
                    // the `+N lines` hint then counted (`docs/long-lines.md`).
                    (Self::Output, _) if is_space => {
                        emit(start..idx);
                        emitted = true;
                        while graphemes
                            .peek()
                            .is_some_and(|(_, next)| next.chars().all(char::is_whitespace))
                        {
                            graphemes.next();
                        }
                        start = graphemes.peek().map_or(line.len(), |(i, _)| *i);
                        idx = start;
                        cur_w = 0;
                        brk = None;
                        width = rest;
                        continue;
                    }
                    // Break at the last space: it stays at the end of the
                    // current row, the partial word after it carries to the
                    // next — unless that word fits on **no** row and moving
                    // it down would only cost a row, in which case it fills
                    // this row from where it stands (`fills_from_here`).
                    (Self::Output, Some(bp))
                        if !fills_from_here(
                            line,
                            bp,
                            width.saturating_sub(cols(&line[start..bp])),
                            rest,
                        ) =>
                    {
                        emit(start..bp);
                        start = bp;
                        cur_w = cols(&line[start..idx]);
                    }
                    // No space to break on — or a verbatim body, which never
                    // reflows at spaces: hard-break right here.
                    _ => {
                        emit(start..idx);
                        start = idx;
                        cur_w = 0;
                    }
                }
                emitted = true;
                brk = None;
                width = rest;
            }
            idx += g.len();
            cur_w += g_w;
            if self == Self::Output && is_space {
                brk = Some(idx);
            }
        }
        // The last row — unless a consumed trailing run left nothing after an
        // exactly-full row (an empty line still gets its one empty row).
        if !emitted || start < line.len() {
            emit(start..line.len());
        }
    }

    /// `text` wrapped to `width` display columns, this mode's way — the body of
    /// the free [`wrap_output`] / [`wrap_verbatim`] every renderer calls.
    /// `width == 0` disables wrapping (text is only split on `'\n'`).
    pub(super) fn wrap(self, text: &str, width: u16) -> Vec<String> {
        if width == 0 {
            return text.split('\n').map(str::to_string).collect();
        }
        let mut out = Vec::new();
        for line in text.split('\n') {
            self.scan(line, width as usize, &mut |r| out.push(line[r].to_string()));
        }
        out
    }

    /// How many display rows `text` occupies at `width` — exactly
    /// `self.wrap(text, width).len()`, counted without building one
    /// (`wrap_mode_rows_counts_exactly_what_it_would_build` pins the two
    /// together). This is what a `+N lines` hint counts: rows the reader would
    /// see, not source lines (`docs/long-lines.md`).
    pub(super) fn rows(self, text: &str, width: u16) -> usize {
        if width == 0 {
            return text.split('\n').count();
        }
        let mut n = 0usize;
        for line in text.split('\n') {
            self.scan(line, width as usize, &mut |_| n += 1);
        }
        n
    }
}

/// Whether the word opening at byte `bp` fills the current row from where it
/// stands rather than moving down whole — Ink's wrap (wrap-ansi's `hard`
/// mode), which is what gives Claude Code's `Bash(…)` header its filled rows
/// (`docs/tools.md` *Long headers*). A word that fits a `rest`-wide row
/// never does: it moves down whole, the ordinary word wrap. One that fits on
/// **no** row is going to be hard-broken anyway, so the only question is
/// where its first piece goes: it starts here, in the `remaining` columns
/// this row has left, unless starting on the next row would cost strictly
/// fewer rows — a tie moves it down, so a token that gains nothing from the
/// fill still opens a row of its own.
fn fills_from_here(line: &str, bp: usize, remaining: usize, rest: usize) -> bool {
    let word_end = line[bp..]
        .find(char::is_whitespace)
        .map_or(line.len(), |i| bp + i);
    let word_w = cols(&line[bp..word_end]);
    if word_w <= rest {
        return false;
    }
    // The rows the word adds beyond this one, starting here (its first piece
    // in `remaining`) versus starting on a fresh row (one row to get there,
    // then the pieces). wrap-ansi's own arithmetic, kept so the two agree.
    let breaks_here = 1 + word_w.saturating_sub(remaining + 1) / rest;
    let breaks_next = (word_w - 1) / rest;
    breaks_next >= breaks_here
}

/// Total display width of a cell's styled `segments` (the *rendered* width, so a
/// column sizes to `foo.db`, not `` `foo.db` ``).
pub(super) fn segments_cols(segments: &[(String, Style)]) -> usize {
    segments.iter().map(|(t, _)| cols(t)).sum()
}

/// Linear interpolation between two RGB colours at `t` in `[0, 1]`.
pub(super) fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(r0, g0, b0), Color::Rgb(r1, g1, b1)) => {
            let mix =
                |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
            Color::Rgb(mix(r0, r1), mix(g0, g1), mix(b0, b1))
        }
        // A named or indexed end has no components to mix (the terminal
        // owns its value — the ANSI theme, `docs/theme.md`): the gradient
        // steps from one end to the other at the midpoint instead.
        _ if t < 0.5 => a,
        _ => b,
    }
}

/// Clamp a run of spans to `width` display columns, appending a dim `…` when
/// they overflow — the header metadata rows, truncated exactly like
/// [`footer_line`]. Preserves each kept span's style.
pub(super) fn clamp_spans(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let total: usize = spans.iter().map(|s| cols(&s.content)).sum();
    if total <= width {
        return Line::from(spans);
    }
    let budget = width.saturating_sub(cols(STATUS_ELLIPSIS));
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let w = cols(&span.content);
        if used + w <= budget {
            used += w;
            out.push(span);
        } else {
            let cut = truncate_cols(&span.content, budget - used);
            if !cut.is_empty() {
                out.push(Span::styled(cut, span.style));
            }
            break;
        }
    }
    out.push(Span::styled(
        STATUS_ELLIPSIS.to_string(),
        Style::new().fg(header_meta_color()),
    ));
    Line::from(out)
}

/// `text` cut to `max` display columns with a trailing `…` when anything was
/// cut — the **honest** [`truncate_cols`]: a row that had to lose text says
/// so in its last column instead of just ending, so a clipped path, command,
/// name, or description is never mistaken for the whole thing. The shared
/// primitive every single-row truncation in the inline views routes through
/// (wrapping is preferred where the page's height is content-driven —
/// `docs/view-flow.md`; this is the floor for the rows that must stay one
/// row).
pub(super) fn ellipsize(text: &str, max: usize) -> String {
    if cols(text) <= max {
        text.to_string()
    } else if max == 0 {
        String::new()
    } else {
        format!("{}…", truncate_cols(text, max - 1))
    }
}

/// Truncate `s` to at most `max` display columns (column-aware, so wide glyphs
/// count as two), returning the kept prefix. Measured per **grapheme cluster**
/// with [`cols`] — the same str-level width every fit-check, pad, and ratatui
/// paint uses — so a VS16 emoji (`❤️`, str width 2, char-sum 1) can't overflow
/// the budget and a ZWJ sequence (`👨‍👩‍👧`) is kept or dropped whole, never
/// split after a dangling joiner.
pub(super) fn truncate_cols(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for g in s.graphemes(true) {
        let gw = cols(g);
        if w + gw > max {
            break;
        }
        out.push_str(g);
        w += gw;
    }
    out
}

/// Linearly blend `fg` toward `bg` by `1 - alpha` (codex's `blend`): `alpha` 1
/// is pure `fg`, 0 pure `bg`. Over [`Color`]s so a palette that names no
/// RGB still resolves — see the match arms.
pub(super) fn blend_color(fg: Color, bg: Color, alpha: f32) -> Color {
    match (fg, bg) {
        (Color::Rgb(fr, fg_, fb), Color::Rgb(br, bg_, bb)) => {
            let mix = |f: u8, b: u8| (f32::from(f) * alpha + f32::from(b) * (1.0 - alpha)) as u8;
            Color::Rgb(mix(fr, br), mix(fg_, bg_), mix(fb, bb))
        }
        // No components to mix (a terminal-palette colour, `docs/theme.md`):
        // whichever end the blend is nearer.
        _ if alpha >= 0.5 => fg,
        _ => bg,
    }
}

/// Where a **breath** is at `elapsed`: a raised cosine easing 0 → 1 → 0 once
/// per `period`, so a colour driven by it swells and fades rather than
/// flicking on and off — the `pulse` spinner style's swell (`docs/spinner.md`;
/// the running tool bullet blinks instead, `tool::tool_pulse_visible`,
/// `docs/tool-pulse.md`). Pure: the phase derives entirely from the
/// boundary-supplied clock.
pub(super) fn breath(elapsed: std::time::Duration, period: std::time::Duration) -> f32 {
    let period = period.as_secs_f32();
    // `phase` is 0…1 through one breath; the cosine turns it into 0 → 1 → 0.
    let phase = (elapsed.as_secs_f32() % period) / period;
    0.5 * (1.0 - (std::f32::consts::TAU * phase).cos())
}

/// Where a **ping-pong** is at `elapsed`: 0 → 1 → 0 over one `period` at
/// constant speed, reversing hard at the ends — how the `gravity` ball and
/// the `wave`'s phase travel the status line's braille track
/// (`docs/spinner.md`). Computed in whole milliseconds so the way back
/// retraces the way out bit for bit: the frame at `T − d` equals the frame
/// at `T + d`, which is what makes the wave's reflection seamless.
pub(super) fn ping_pong(elapsed: std::time::Duration, period: std::time::Duration) -> f32 {
    let period_ms = period.as_millis().max(1);
    let ms = elapsed.as_millis() % period_ms;
    let toward = if ms * 2 <= period_ms {
        ms
    } else {
        period_ms - ms
    };
    (toward * 2) as f32 / period_ms as f32
}

/// Where a **hop** is at `elapsed`: one parabolic arc per `period`, 0 at
/// take-off and landing, 1 at the apex — gravity with no damping, the
/// `gravity` ball's bounce (`docs/spinner.md`). Whole-millisecond
/// arithmetic like [`ping_pong`], so the fall mirrors the rise exactly.
pub(super) fn hop(elapsed: std::time::Duration, period: std::time::Duration) -> f32 {
    let period_ms = period.as_millis().max(1);
    let ms = elapsed.as_millis() % period_ms;
    (4 * ms * (period_ms - ms)) as f32 / (period_ms * period_ms) as f32
}

/// Display columns a span list occupies.
pub(super) fn spans_cols(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| cols(&span.content)).sum()
}
