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
/// current row (so concatenating the rows reconstructs the line byte-exactly)
/// and continuation rows start at a word. A single word wider than `width` is
/// hard-broken on grapheme boundaries, measured in display columns (like
/// [`wrap_segment`]'s hard break). `width == 0` disables wrapping. Used by the
/// command/shell peek ([`result_peek_block`]), the running tail
/// ([`running_command_lines`]), and the Ctrl+O output ([`tool_full_lines`]),
/// so the three wrap identically.
pub(super) fn wrap_output(text: &str, width: u16) -> Vec<String> {
    WrapMode::Output.wrap(text, width)
}

/// Which wrapper a collapsed cell measures and builds its rows with — the
/// pairing of [`wrap_output`] / [`wrap_verbatim`] with the row **count** and
/// the **clip** that have to agree with them, kept in one type so a
/// `+N lines` hint can never count rows a *different* wrapper would have
/// produced (`docs/long-lines.md`). All three ride one range-emitting scan per
/// mode, so counting builds no rows at all — which is what lets the running
/// tail's footer measure the whole retained buffer every animation frame.
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
    /// at `width` (> 0) columns — the single scan behind [`Self::wrap`],
    /// [`Self::rows`] and [`Self::clip`]. Measured per grapheme cluster in
    /// display columns like every other width decision in this module.
    fn scan(self, line: &str, width: usize, emit: &mut dyn FnMut(Range<usize>)) {
        let mut start = 0usize; // byte index the current row opens at
        let mut cur_w = 0usize; // display width of `line[start..idx]`
        let mut brk: Option<usize> = None; // byte index past the last space
        let mut idx = 0usize; // byte index just past the graphemes consumed
        for g in line.graphemes(true) {
            let g_w = cols(g);
            if cur_w > 0 && cur_w + g_w > width {
                match (self, brk) {
                    // Break at the last space: it stays at the end of the
                    // current row, the partial word after it carries to the
                    // next (so concatenating rows rebuilds the line exactly).
                    (Self::Output, Some(bp)) => {
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
                brk = None;
            }
            idx += g.len();
            cur_w += g_w;
            if self == Self::Output && g.chars().all(char::is_whitespace) {
                brk = Some(idx);
            }
        }
        emit(start..line.len());
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

    /// One source `line` wrapped to `width` and clipped to `max_rows` display
    /// rows: the rows to paint — the last ending in [`TOOL_LINE_ELLIPSIS`] when
    /// anything was cut — plus the number of rows dropped.
    ///
    /// The per-line budget a collapsed cell spends on one source line
    /// (`docs/long-lines.md`): without it a single pathological line (a
    /// minified bundle, a 2 KB JSON body) fills the whole cell with wrapped
    /// noise and reports it as one hidden "line". A line that fits comes back
    /// byte-identical to [`Self::wrap`], unmarked.
    pub(super) fn clip(self, line: &str, width: u16, max_rows: usize) -> (Vec<String>, usize) {
        let mut kept: Vec<String> = Vec::new();
        let mut hidden = 0usize;
        {
            let mut push = |text: &str| {
                if kept.len() < max_rows {
                    kept.push(text.to_string());
                } else {
                    hidden += 1;
                }
            };
            for seg in line.split('\n') {
                if width == 0 {
                    push(seg);
                } else {
                    self.scan(seg, width as usize, &mut |r| push(&seg[r]));
                }
            }
        }
        if hidden > 0
            && let Some(last) = kept.last_mut()
        {
            *last = mark_clipped(last, width as usize);
        }
        (kept, hidden)
    }
}

/// The last kept row of a clipped line, marked: its trailing whitespace
/// replaced by [`TOOL_LINE_ELLIPSIS`] and the text cut back far enough that the
/// row still ends inside `width` columns (`0` = unbounded, matching the
/// wrappers' "no wrapping"). The row-level sibling of [`ellipsize`] — which
/// marks a row cut *at* the width, where this marks one cut *below* its own
/// wrapped remainder.
fn mark_clipped(row: &str, width: usize) -> String {
    let room = if width == 0 {
        usize::MAX
    } else {
        width.saturating_sub(cols(TOOL_LINE_ELLIPSIS))
    };
    let mut out = truncate_cols(row.trim_end(), room);
    out.push_str(TOOL_LINE_ELLIPSIS);
    out
}

/// Total display width of a cell's styled `segments` (the *rendered* width, so a
/// column sizes to `foo.db`, not `` `foo.db` ``).
pub(super) fn segments_cols(segments: &[(String, Style)]) -> usize {
    segments.iter().map(|(t, _)| cols(t)).sum()
}

/// Linear interpolation between two RGB colours at `t` in `[0, 1]`.
pub(super) fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color::Rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
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
        Style::new().fg(HEADER_META_COLOR),
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

/// Keep the leading graphemes of `spans` that fit within `max` display columns,
/// preserving each span's style (a span-aware [`truncate_cols`]). Used to make
/// room for the `…)` when a header is truncated at [`TOOL_HEADER_MAX_ROWS`].
pub(super) fn truncate_spans(spans: &[Span<'static>], max: usize) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let w = cols(&span.content);
        if used + w <= max {
            out.push(span.clone());
            used += w;
        } else {
            let kept = truncate_cols(&span.content, max - used);
            if !kept.is_empty() {
                out.push(Span::styled(kept, span.style));
            }
            break;
        }
    }
    out
}

/// Linearly blend `fg` toward `bg` by `1 - alpha` (codex's `blend`): `alpha` 1
/// is pure `fg`, 0 pure `bg`.
pub(super) fn blend(fg: (u8, u8, u8), bg: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let mix = |f: u8, b: u8| (f32::from(f) * alpha + f32::from(b) * (1.0 - alpha)) as u8;
    (mix(fg.0, bg.0), mix(fg.1, bg.1), mix(fg.2, bg.2))
}

/// Display columns a span list occupies.
pub(super) fn spans_cols(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| cols(&span.content)).sum()
}
