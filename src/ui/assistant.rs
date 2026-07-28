//! The assistant message renderer: markdown blocks, fenced code with syntax
//! highlighting, headings, lists, and quotes — driven line by line so it stays
//! prefix-stable while streaming. See `docs/markdown.md`.

use super::table::{join_wrapped_table_row, table_block_rows};
use super::theme::*;
use super::wrap::cols;
use super::*;

/// The style an ATX heading of `level` (1–6) renders with — a faithful port of
/// codex's `markdown_render.rs::start_heading`: the `#` markers are **kept**
/// (see [`AssistantRenderer::content_rows`]) and the whole line carries text
/// *modifiers only*, no foreground colour, so a heading reads like codex's:
/// h1 bold+underlined, h2 bold, h3 bold+italic, h4–h6 italic. See
/// `docs/markdown.md`.
fn heading_style(level: u8) -> Style {
    let modifier = match level {
        1 => Modifier::BOLD | Modifier::UNDERLINED,
        2 => Modifier::BOLD,
        3 => Modifier::BOLD | Modifier::ITALIC,
        _ => Modifier::ITALIC, // h4–h6 (and any deeper level clamps here)
    };
    Style::new().add_modifier(modifier)
}

/// Whether `line` should render as a [`THEMATIC_BREAK`]. A `-` rule is
/// indistinguishable from a **setext `H2` underline** (which we can't render —
/// it needs lookahead that would break streaming prefix-stability), so a `-` rule
/// is only honoured when the previous line was blank (`prev_blank`); `*`/`_` runs
/// are unambiguous and always render. Keeps a `text\n---` pair as literal prose
/// rather than fabricating a rule where codex would show a heading.
fn is_thematic_break(line: &str, prev_blank: bool) -> bool {
    match markdown::thematic_break(line) {
        Some('-') => prev_blank,
        Some(_) => true,
        None => false,
    }
}

/// Expand tabs in a single code line to spaces **for display** (see
/// [`CODE_TAB_WIDTH`]). A tab is zero display columns, so tab-indented code
/// rendered verbatim would collapse flush-left; this substitutes a fixed run of
/// spaces (not tab-stop alignment) so the indentation survives, matching codex's
/// `expand_tabs`. Borrows unchanged in the common (tab-free) case. Applied only
/// on the render path — the stored message text keeps its tabs, so `/copy` is
/// byte-exact.
pub(super) fn expand_code_tabs(line: &str) -> std::borrow::Cow<'_, str> {
    if line.contains('\t') {
        std::borrow::Cow::Owned(line.replace('\t', &" ".repeat(CODE_TAB_WIDTH)))
    } else {
        std::borrow::Cow::Borrowed(line)
    }
}

/// Hard-break a code line's **styled** segments into display rows of at most
/// `width` columns, preserving each run's style across the break — the verbatim,
/// whitespace-preserving counterpart of [`wrap_verbatim`] that keeps syntax
/// styling. Breaks on grapheme boundaries measured in display columns (an
/// overflowing cluster is placed alone); adjacent same-style graphemes coalesce
/// into one span. An empty line yields a single empty row (just the bullet/indent,
/// once stamped). Prefix-stable — appending only extends the last row.
pub(super) fn code_content_rows(
    segments: &[(String, Style)],
    width: u16,
) -> Vec<Vec<Span<'static>>> {
    let width = (width as usize).max(1);
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_style = Style::default();
    let mut w = 0usize;
    let flush = |row: &mut Vec<Span<'static>>, run: &mut String, style: Style| {
        if !run.is_empty() {
            row.push(Span::styled(std::mem::take(run), style));
        }
    };
    for (text, style) in segments {
        for g in text.graphemes(true) {
            let gw = cols(g);
            if w > 0 && w + gw > width {
                flush(&mut row, &mut run, run_style);
                rows.push(std::mem::take(&mut row));
                w = 0;
            }
            if *style != run_style {
                flush(&mut row, &mut run, run_style);
                run_style = *style;
            }
            run.push_str(g);
            w += gw;
        }
    }
    flush(&mut row, &mut run, run_style);
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows
}

/// Flatten a parsed inline tree ([`markdown::parse_inline`]) into styled text
/// segments under `base`. Emphasis adds a modifier, `code` a colour, a link its
/// text plus a ` (url)` suffix, an image its alt text. Nesting composes styles.
pub(super) fn inline_spans(nodes: &[markdown::Inline], base: Style) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    for node in nodes {
        match node {
            markdown::Inline::Text(t) => out.push((t.clone(), base)),
            markdown::Inline::Bold(inner) => {
                out.extend(inline_spans(inner, base.add_modifier(Modifier::BOLD)));
            }
            markdown::Inline::Italic(inner) => {
                out.extend(inline_spans(inner, base.add_modifier(Modifier::ITALIC)));
            }
            markdown::Inline::Strike(inner) => {
                out.extend(inline_spans(
                    inner,
                    base.add_modifier(Modifier::CROSSED_OUT),
                ));
            }
            markdown::Inline::Code(c) => out.push((c.clone(), base.fg(INLINE_CODE_COLOR))),
            markdown::Inline::Link { text, url } => {
                out.extend(inline_spans(text, base));
                out.push((
                    format!(" ({url})"),
                    base.fg(LINK_URL_COLOR).add_modifier(Modifier::UNDERLINED),
                ));
            }
            markdown::Inline::Image { alt } => out.push((alt.clone(), base)),
        }
    }
    out
}

/// Word-wrap styled inline `segments` to `width` columns, preserving each run's
/// style across wraps — the span-aware counterpart of [`wrap_text`]. Words (runs
/// of non-whitespace, which may span several styled pieces, e.g. `un**bold**`)
/// stay intact where they fit; a word wider than `width` hard-breaks on grapheme
/// boundaries; whitespace collapses to single base-styled spaces between words.
/// An empty line yields one empty row (matching [`wrap_text`]).
pub(super) fn wrap_inline(segments: &[(String, Style)], width: u16) -> Vec<Vec<Span<'static>>> {
    let words = tokenize_words(segments);
    let to_spans = |rows: Vec<Vec<(String, Style)>>| -> Vec<Vec<Span<'static>>> {
        rows.into_iter()
            .map(|r| r.into_iter().map(|(t, s)| Span::styled(t, s)).collect())
            .collect()
    };
    if width == 0 {
        // No wrapping: one row, words rejoined by single spaces.
        let mut row: Vec<(String, Style)> = Vec::new();
        for (i, word) in words.iter().enumerate() {
            if i > 0 {
                push_piece(&mut row, " ", Style::default());
            }
            for (t, s) in word {
                push_piece(&mut row, t, *s);
            }
        }
        return to_spans(vec![row]);
    }
    let width = width as usize;
    let mut rows: Vec<Vec<(String, Style)>> = Vec::new();
    let mut row: Vec<(String, Style)> = Vec::new();
    let mut row_w = 0usize;
    for word in &words {
        let ww: usize = word.iter().map(|(t, _)| cols(t)).sum();
        if ww > width {
            // A word too wide for any line: hard-break it grapheme by grapheme.
            if row_w > 0 {
                rows.push(std::mem::take(&mut row));
                row_w = 0;
            }
            for (t, s) in word {
                for g in t.graphemes(true) {
                    let gw = cols(g);
                    if row_w > 0 && row_w + gw > width {
                        rows.push(std::mem::take(&mut row));
                        row_w = 0;
                    }
                    push_piece(&mut row, g, *s);
                    row_w += gw;
                }
            }
            continue;
        }
        let needed = if row_w == 0 { ww } else { row_w + 1 + ww };
        if needed > width {
            rows.push(std::mem::take(&mut row));
            row_w = 0;
        } else if row_w > 0 {
            push_piece(&mut row, " ", Style::default());
            row_w += 1;
        }
        for (t, s) in word {
            push_piece(&mut row, t, *s);
        }
        row_w += ww;
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    to_spans(rows)
}

/// Split styled `segments` into words — each a run of non-whitespace pieces
/// (`Vec<(text, style)>`) that may cross piece/style boundaries — dropping the
/// whitespace between them ([`wrap_inline`] re-inserts single spaces).
pub(super) fn tokenize_words(segments: &[(String, Style)]) -> Vec<Vec<(String, Style)>> {
    let mut words: Vec<Vec<(String, Style)>> = Vec::new();
    let mut word: Vec<(String, Style)> = Vec::new();
    let mut piece = String::new();
    let mut piece_style = Style::default();
    for (text, style) in segments {
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !piece.is_empty() {
                    word.push((std::mem::take(&mut piece), piece_style));
                }
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            } else {
                if !piece.is_empty() && *style != piece_style {
                    word.push((std::mem::take(&mut piece), piece_style));
                }
                piece_style = *style;
                piece.push(ch);
            }
        }
    }
    if !piece.is_empty() {
        word.push((piece, piece_style));
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Append `text` to a row of styled pieces, coalescing with the last piece when
/// the style matches so a run of same-styled graphemes stays one span.
fn push_piece(row: &mut Vec<(String, Style)>, text: &str, style: Style) {
    if let Some((last_text, last_style)) = row.last_mut()
        && *last_style == style
    {
        last_text.push_str(text);
        return;
    }
    row.push((text.to_string(), style));
}

/// Build an assistant reply's lines, markdown-aware (`docs/markdown.md`):
/// [`markdown::parse_blocks`] splits prose from fenced code; prose word-wraps via
/// [`wrap_text`] (ATX headings keep their `#`s and style per level, codex-style)
/// while **code
/// blocks render verbatim** — each source line kept byte-for-byte,
/// **syntax-highlighted** ([`highlight::highlight`]) and hard-broken only on width
/// via [`code_content_rows`], with both fences (and the language info-string)
/// hidden. The bullet lands on row 0 and `INDENT` on the rest, exactly like the
/// plain path — so fence/heading-free text is byte-identical to before.
///
/// **Prefix-stable at the line level:** a line's prose/code mode and its highlight
/// are fixed by the text before it. (Highlighting uses one-char lookahead within a
/// line — a call's `(` — so an in-progress code *line* isn't safe to commit until
/// it completes; [`StreamRender`] withholds it, see there.)
pub(super) fn assistant_lines(
    text: &str,
    width: u16,
    bullet: &str,
    color: Color,
) -> Vec<Line<'static>> {
    let mut renderer = AssistantRenderer::new(width, bullet, color);
    let mut rows: Vec<Line<'static>> = Vec::new();
    for line in text.split('\n') {
        rows.extend(renderer.feed_line(line));
    }
    // Flush a trailing table (one the reply ended on, with no closing line) — its
    // rows were buffered pending a close that never came. The streaming
    // `StreamRender::finish` flushes the same way, so the two never disagree.
    rows.extend(renderer.flush());
    // Trim trailing blank rows (a model's `…\n\n` before a tool call, say): the
    // caller adds exactly one spacer, so trailing blanks would stack. Skipped
    // when the reply ends inside an open code fence — its blank lines are
    // content. The streaming [`StreamRender`] trims the same way, so the two
    // never disagree.
    if !renderer.in_code() {
        while rows.last().is_some_and(row_is_blank) {
            rows.pop();
        }
    }
    // A reply that renders to zero rows still gets one bullet row so the bullet
    // always has a home — an empty reply, or (now that fences render nothing) a
    // reply that is only a code fence. The streaming [`StreamRender`] applies the
    // same fallback, so the two never disagree on such a reply.
    if rows.is_empty() {
        rows.push(empty_assistant_row(bullet, color));
    }
    rows
}

/// The lone bullet row an assistant message falls back to when its body renders
/// to **zero** rows (an empty reply, or one that is only a hidden code fence), so
/// the role bullet always has a home. Shared by the batch [`assistant_lines`] and
/// the streaming [`StreamRender`] so they agree on such a reply.
pub(super) fn empty_assistant_row(bullet: &str, color: Color) -> Line<'static> {
    Line::from(vec![Span::styled(
        bullet.to_string(),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )])
}

/// Whether a rendered row is visually blank — every span is whitespace (an
/// indent-only continuation row for an empty source line). Used to trim a
/// message's **trailing** blank rows so a model's `…\n\n` before a tool call
/// doesn't stack blank rows on top of the caller's single spacer (the
/// 3-newline bug). The bullet-home fallback row (`● `) is *not* blank, so it is
/// never trimmed. Shared by [`assistant_lines`] (repaint) and [`StreamRender`]
/// (live) so they agree.
pub(super) fn row_is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|s| s.content.trim().is_empty())
}

/// The incremental, prefix-stable core shared by the batch [`assistant_lines`]
/// and the streaming [`StreamRender`]: feed an assistant reply's source lines in
/// order with [`AssistantRenderer::feed_line`] and each returns that line's
/// finished rows. Fence state ([`markdown::BlockScanner`]) and highlight carry
/// ([`highlight::Highlighter`]) thread across the calls, so a completed line's
/// rows never change — which is what lets scrollback commits and the strip
/// preview cost O(new line) instead of O(whole reply). See `docs/markdown.md`.
///
/// The very first row emitted across the whole message carries the coloured
/// role bullet; every later row is indented under it. `Clone` lets [`StreamRender`]
/// *peek* an in-progress (not yet newline-terminated) line without advancing the
/// state it will resume from.
#[derive(Clone)]
pub(super) struct AssistantRenderer {
    /// Columns available to prose (and code) after the bullet.
    pub(super) content_width: u16,
    /// The role bullet stamped on the first row.
    pub(super) bullet: String,
    /// The bullet's colour.
    pub(super) color: Color,
    /// Fence state entering the next line.
    scanner: markdown::BlockScanner,
    /// The open code block's highlighter (`None` outside a fence).
    highlighter: Option<highlight::Highlighter>,
    /// Whether any row has been emitted yet (the first gets the bullet).
    emitted_any: bool,
    /// Whether the previous source line was blank — the gate that lets a `-` rule
    /// (`---`, ambiguous with a setext underline) render as an em-dash break.
    prev_blank: bool,
    /// In-progress GFM table accumulation (`docs/markdown.md`). A table is not
    /// prefix-stable — a later row can widen a column — so its source lines are
    /// buffered here and rendered **whole** only when the block closes (a
    /// non-table line, a code fence, or end-of-message via [`Self::flush`]).
    pub(super) table: TableState,
}

/// The [`AssistantRenderer`]'s GFM-table state machine (docs/table-streaming.md).
/// A candidate header is held in `PendingHeader` until the next line confirms it
/// (a matching [`markdown::table_delimiter`]) — the one-line lookahead a pipe
/// table needs — then `Buffering` accumulates the data rows, emitting **nothing**,
/// until a non-table line (or a code fence, or end-of-message) closes the block
/// and it renders whole through [`table_block_rows`], its column widths fit to
/// **every** row. Nothing of an open table ever commits to scrollback, so a wide
/// later row can never invalidate a committed one (prefix-stability is trivial);
/// the streaming strip previews the forming block instead ([`StreamRender::preview`]).
#[derive(Clone)]
pub(super) enum TableState {
    /// Not inside a table.
    None,
    /// A row that *looks* like a table header, buffered pending its delimiter.
    PendingHeader(String),
    /// Header + delimiter confirmed; the data rows accumulate here (raw source
    /// lines) until the block closes. `aligns` are the delimiter's alignments —
    /// their count is the column count.
    Buffering {
        header: String,
        aligns: Vec<markdown::Alignment>,
        rows: Vec<String>,
    },
}

impl AssistantRenderer {
    pub(super) fn new(width: u16, bullet: &str, color: Color) -> Self {
        Self {
            content_width: width.saturating_sub(BULLET_WIDTH).max(1),
            bullet: bullet.to_string(),
            color,
            scanner: markdown::BlockScanner::new(),
            highlighter: None,
            emitted_any: false,
            prev_blank: true, // start-of-message is a blank boundary
            table: TableState::None,
        }
    }

    /// Render the next source `line` into its finished rows, advancing fence +
    /// highlight state. The first row ever emitted carries the bullet; the rest
    /// are indented under it.
    pub(super) fn feed_line(&mut self, line: &str) -> Vec<Line<'static>> {
        let rows = self.content_rows(line);
        self.stamp(rows)
    }

    /// The row *content* spans for `line` (no bullet/indent prefix yet),
    /// advancing fence/highlight state. Prose word-wraps (headings keep their
    /// `#`s and style per level, codex-style); code renders verbatim,
    /// syntax-highlighted; both the opening and
    /// closing fence render nothing (no gutter, and the language info-string is
    /// not shown).
    fn content_rows(&mut self, line: &str) -> Vec<Vec<Span<'static>>> {
        // Track blank-line boundaries for the `-` thematic-break gate below. This
        // mirrors the scanner's own `prev_blank` (used for indented code) but is
        // kept here because the rule decision lives in the prose branch, like
        // headings.
        let was_blank = self.prev_blank;
        self.prev_blank = line.trim().is_empty();
        match self.scanner.classify(line) {
            markdown::LineKind::CodeStart(lang) => {
                // A fence can't sit inside a table, so it closes any in-progress
                // one first; then it opens the block silently (primes highlighting
                // for the info-string language, emits no row — no gutter, no label).
                let out = self.flush_table();
                self.highlighter = Some(highlight::Highlighter::new(lang.as_deref()));
                out
            }
            markdown::LineKind::CodeEnd => {
                self.highlighter = None;
                Vec::new()
            }
            // A code line never arrives with a table open (a fence flushed it, or
            // the blank before indented code did), so no flush is needed here.
            markdown::LineKind::Code => self.render_code_line(line),
            markdown::LineKind::Prose => self.prose_or_table(line, was_blank),
        }
    }

    /// Render a fenced/indented **code** line: tabs expanded so indentation
    /// survives, syntax-highlighted by the open fence's language (plain for an
    /// indented block), hard-broken on width.
    fn render_code_line(&mut self, line: &str) -> Vec<Vec<Span<'static>>> {
        let expanded = expand_code_tabs(line);
        let segs = match self.highlighter.as_mut() {
            Some(h) => h.line(&expanded),
            None => vec![highlight::Seg {
                text: expanded.into_owned(),
                style: highlight::plain_style(),
            }],
        };
        let styled: Vec<(String, Style)> = segs.into_iter().map(|s| (s.text, s.style)).collect();
        code_content_rows(&styled, self.content_width)
    }

    /// Render a **non-table** prose line: an ATX heading (markers kept, styled per
    /// level — codex parity), a thematic break (`———`), or word-wrapped plain
    /// prose. Pure per-line, so it stays prefix-stable.
    fn render_prose_line(&self, line: &str, was_blank: bool) -> Vec<Vec<Span<'static>>> {
        if let Some((level, htext)) = markdown::heading_level(line) {
            // Codex keeps the `#` markers visible (`"#".repeat(level)`) and styles
            // the whole line per level — no colour, just modifiers. Normalise the
            // marker run + a single space, then word-wrap the reconstructed heading.
            let style = heading_style(level);
            let hashes = "#".repeat(level as usize);
            let content = if htext.is_empty() {
                hashes
            } else {
                format!("{hashes} {htext}")
            };
            wrap_text(&content, self.content_width)
                .into_iter()
                .map(|l| vec![Span::styled(l, style)])
                .collect()
        } else if is_thematic_break(line, was_blank) {
            // Codex renders `---`/`***`/`___` as an unstyled `———` rule on its own
            // row (`Event::Rule`). A single settled row, so prefix-stable. Checked
            // before lists so `- - -` / `* * *` stay rules, not bullets.
            vec![vec![Span::raw(THEMATIC_BREAK.to_string())]]
        } else if let Some(inner) = markdown::block_quote(line) {
            self.render_block_quote(inner)
        } else if let Some(item) = markdown::list_item(line) {
            self.render_list_item(&item)
        } else {
            // Plain prose: parse inline `**bold**`/`*italic*`/`~~strike~~`/
            // `` `code` ``/`[link](url)` into styled spans, then span-preserving
            // word-wrap. Emphasis is line-local, so a complete line's styling is
            // final; a trailing line with an open marker is withheld by
            // `StreamRender` (`markdown::has_open_inline`).
            let segs = inline_spans(&markdown::parse_inline(line), Style::default());
            wrap_inline(&segs, self.content_width)
        }
    }

    /// Render a list item (`docs/markdown.md`): the nesting indent, then the
    /// marker (`-` for bullets, an accent-coloured `N.` for ordered items), then
    /// the inline-parsed content span-wrapped with a **hanging indent** — every
    /// continuation row aligns under the text, not the marker. Line-local, so
    /// prefix-stable.
    fn render_list_item(&self, item: &markdown::ListItem) -> Vec<Vec<Span<'static>>> {
        let (marker_text, marker_style) = match item.marker {
            markdown::ListMarker::Bullet => ("- ".to_string(), Style::default()),
            markdown::ListMarker::Ordered(n, delim) => {
                (format!("{n}{delim} "), Style::new().fg(LIST_MARKER_COLOR))
            }
        };
        let hang = item.indent + cols(&marker_text);
        let text_w = (self.content_width as usize).saturating_sub(hang).max(1) as u16;
        let text_rows = wrap_inline(
            &inline_spans(&markdown::parse_inline(item.content), Style::default()),
            text_w,
        );
        text_rows
            .into_iter()
            .enumerate()
            .map(|(i, mut text)| {
                let mut row = Vec::new();
                if i == 0 {
                    if item.indent > 0 {
                        row.push(Span::raw(" ".repeat(item.indent)));
                    }
                    row.push(Span::styled(marker_text.clone(), marker_style));
                } else {
                    row.push(Span::raw(" ".repeat(hang))); // hang under the text
                }
                row.append(&mut text);
                row
            })
            .collect()
    }

    /// Render a blockquote (`docs/markdown.md`): a dim `> ` marker on every
    /// wrapped row, the inline-parsed text dim. Line-local, so prefix-stable.
    fn render_block_quote(&self, inner: &str) -> Vec<Vec<Span<'static>>> {
        let marker = "> ";
        let text_w = (self.content_width as usize)
            .saturating_sub(cols(marker))
            .max(1) as u16;
        let base = Style::new().fg(QUOTE_COLOR);
        let text_rows = wrap_inline(&inline_spans(&markdown::parse_inline(inner), base), text_w);
        text_rows
            .into_iter()
            .map(|mut text| {
                let mut row = vec![Span::styled(marker.to_string(), base)];
                row.append(&mut text);
                row
            })
            .collect()
    }

    /// Run the GFM-table state machine for a prose `line`
    /// (docs/table-streaming.md): buffer a candidate header, confirm it against
    /// the next line's delimiter, then **buffer every data row** — a non-table
    /// line closes the block, rendering it whole (column widths fit to every
    /// row) and recursing to render that closing line in place. Returns the
    /// rows to emit now (empty while the table accumulates — the streaming
    /// strip previews the forming block instead, [`StreamRender::preview`]).
    fn prose_or_table(&mut self, line: &str, was_blank: bool) -> Vec<Vec<Span<'static>>> {
        match std::mem::replace(&mut self.table, TableState::None) {
            TableState::None => {
                if markdown::is_table_row(line) {
                    self.table = TableState::PendingHeader(line.to_string());
                    Vec::new()
                } else {
                    self.render_prose_line(line, was_blank)
                }
            }
            TableState::PendingHeader(header) => {
                let ncols = markdown::table_cells(&header).len();
                if let Some(aligns) = markdown::table_delimiter(line).filter(|a| a.len() == ncols) {
                    // Header + a matching delimiter → confirmed; buffer the data
                    // rows until the block closes (the widths need them all).
                    self.table = TableState::Buffering {
                        header,
                        aligns,
                        rows: Vec::new(),
                    };
                    Vec::new()
                } else {
                    // Not a table — the buffered header was ordinary prose (a line
                    // with pipes is never a heading or rule, so `was_blank` is moot),
                    // then process the current line (it may start a fresh table).
                    let mut out = self.render_prose_line(&header, false);
                    out.extend(self.prose_or_table(line, was_blank));
                    out
                }
            }
            TableState::Buffering {
                header,
                aligns,
                mut rows,
            } => {
                if markdown::is_table_row(line) {
                    // A pipe-carrying line that doesn't start with `|`, inside a
                    // leading-pipe table, is the previous row's hard-wrapped tail
                    // — re-join it instead of minting a phantom one-cell row.
                    match rows
                        .last()
                        .and_then(|prev| join_wrapped_table_row(prev, line, aligns.len()))
                    {
                        Some(joined) => {
                            rows.pop();
                            rows.push(joined);
                        }
                        None => rows.push(line.to_string()),
                    }
                    self.table = TableState::Buffering {
                        header,
                        aligns,
                        rows,
                    };
                    Vec::new()
                } else {
                    // A non-table line closes the block: render it whole (grid or
                    // records, widths from every row), then the closing line.
                    let mut out = table_block_rows(&header, &aligns, &rows, self.content_width);
                    out.extend(self.prose_or_table(line, was_blank));
                    out
                }
            }
        }
    }

    /// Emit any open table's rows, clearing the state — called when a code
    /// fence interrupts a table (and by [`Self::flush`] at end-of-message).
    /// A never-confirmed `PendingHeader` renders as the plain prose line it
    /// actually was; a `Buffering` table renders whole (a header-only table is
    /// the opening + bottom border).
    fn flush_table(&mut self) -> Vec<Vec<Span<'static>>> {
        match std::mem::replace(&mut self.table, TableState::None) {
            TableState::None => Vec::new(),
            TableState::PendingHeader(header) => self.render_prose_line(&header, false),
            TableState::Buffering {
                header,
                aligns,
                rows,
            } => table_block_rows(&header, &aligns, &rows, self.content_width),
        }
    }

    /// Flush any buffered table at end-of-message, stamping bullet/indent. Both
    /// the batch [`assistant_lines`] and the streaming [`StreamRender::finish`]
    /// call this, so a trailing table (one with no closing line) still renders.
    pub(super) fn flush(&mut self) -> Vec<Line<'static>> {
        let rows = self.flush_table();
        self.stamp(rows)
    }

    /// Whether the **next** line to be fed sits inside an open fenced code block.
    /// Such a line's rows aren't safe to commit until it completes: the
    /// highlighter's within-line lookahead (a call's `(`, a `//` comment, a
    /// closing `*/`) can recolour an *earlier* wrapped row of the same line. The
    /// streaming committer uses this to withhold an in-progress code line whole.
    pub(super) fn in_code(&self) -> bool {
        self.highlighter.is_some()
    }

    /// Whether a GFM table block is open (`PendingHeader`/`Buffering`) — the
    /// phase that emits **no rows**: the block renders whole only when it
    /// closes, so [`StreamRender::commit`] withholds the trailing line while
    /// this holds (like [`Self::in_code`]) and [`StreamRender::preview`] shows
    /// the forming block instead (docs/table-streaming.md). It also gates
    /// feeding an *empty* trailing line into a clone — that would close the
    /// block early on a chunk boundary that landed right after a newline.
    pub(super) fn in_table(&self) -> bool {
        !matches!(self.table, TableState::None)
    }

    /// Stamp the bullet (first row of the message) or `INDENT` (every later row)
    /// onto each content row.
    fn stamp(&mut self, rows: Vec<Vec<Span<'static>>>) -> Vec<Line<'static>> {
        rows.into_iter()
            .map(|mut spans| {
                let prefix = if self.emitted_any {
                    Span::raw(INDENT.to_string())
                } else {
                    self.emitted_any = true;
                    Span::styled(
                        self.bullet.clone(),
                        Style::new().fg(self.color).add_modifier(Modifier::BOLD),
                    )
                };
                let mut all = vec![prefix];
                all.append(&mut spans);
                Line::from(all)
            })
            .collect()
    }
}
