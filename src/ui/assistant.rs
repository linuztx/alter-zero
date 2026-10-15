//! The assistant message renderer: markdown blocks, fenced code with syntax
//! highlighting, headings, lists, and quotes — driven line by line so it stays
//! prefix-stable while streaming. See `docs/markdown.md`.

use super::inline::{inline_spans, linkify_code_segments, wrap_inline_hanging};
use super::table::{join_wrapped_table_row, table_block_rows};
use super::theme::*;
use super::wrap::cols;
use super::*;

/// The style an ATX heading of `level` (1–6) renders with — a faithful port of
/// codex's `markdown_render.rs::start_heading`: the `#` markers are **kept**
/// (see `AssistantRenderer::heading_rows`) and the whole line carries text
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

/// Build an assistant reply's lines, markdown-aware (`docs/markdown.md`), by
/// driving one [`AssistantRenderer`] over its source lines: prose word-wraps
/// with its inline markdown styled, lists and quotes render codex's way, blocks
/// sit one blank row apart, and **code blocks render verbatim** — each source
/// line kept byte-for-byte, **syntax-highlighted** and hard-broken only on width
/// via [`code_content_rows`], with both fences (and the language info-string)
/// hidden. The bullet lands on row 0 and `INDENT` on the rest, exactly like the
/// plain path.
///
/// **Prefix-stable at the line level:** a line's rows are fixed by the text
/// before it and the line itself. (Highlighting uses one-char lookahead within
/// a line — a call's `(` — so an in-progress code *line* isn't safe to commit
/// until it completes; [`StreamRender`] withholds it, see there.)
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

/// What a scope's last block was — the fact codex's spacing reads
/// (`docs/markdown.md` *Block layout*): a paragraph an unbroken next text line
/// continues, a list a matching item joins, or any other block the next one
/// is set apart from by one blank row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Last {
    /// Nothing yet: the scope's first block needs no separator.
    #[default]
    Nothing,
    /// A paragraph — a soft-broken next line continues it.
    Paragraph,
    /// A list — the next item of the same kind is its sibling.
    List(ListState),
    /// Any other block: a heading, rule, code block, table or quote.
    Block,
}

/// An open list's running state in the scope holding it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ListState {
    /// What its items are; an item of another kind starts a new list.
    kind: ListKind,
    /// The number the next item shows — ordered lists renumber from their
    /// start, as codex's parser does (`1.`, `1.`, `1.` reads 1, 2, 3).
    next: u64,
    /// Rows the latest closed item rendered, nested content included: codex's
    /// AfterMultiline rule sets an item that took more than one row apart from
    /// its next sibling with a blank row.
    prev_rows: usize,
}

/// A list's type — CommonMark starts a new list when the bullet character or
/// the ordered delimiter changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKind {
    /// `-`, `*` or `+`.
    Bullet(char),
    /// `N.` or `N)`.
    Ordered(char),
}

/// One open container a later line has to continue in order to stay inside —
/// CommonMark's block structure, line by line: a quote continues on a `>`, a
/// list item on a line indented to its content (or a blank one).
#[derive(Clone, Debug)]
enum Container {
    /// A blockquote, and the last block inside it.
    Quote(Last),
    /// A list item.
    Item(Item),
}

/// An open list item.
#[derive(Clone, Debug)]
struct Item {
    /// Spaces a later line must be indented, past the enclosing containers'
    /// prefixes, to stay in this item — the source column its content starts at.
    indent: usize,
    /// The rendered marker: depth padding then the bullet, box or number.
    marker: Vec<(String, Style)>,
    /// The marker's display width — the hanging indent of every later row.
    hang: usize,
    /// Whether the item's first row is still to come (it carries the marker).
    marker_pending: bool,
    /// Rows rendered since the item opened, nested content included.
    rows: usize,
    /// The last block inside the item.
    last: Last,
}

/// The incremental, prefix-stable core shared by the batch [`assistant_lines`]
/// and the streaming [`StreamRender`]: feed an assistant reply's source lines in
/// order with [`AssistantRenderer::feed_line`] and each returns that line's
/// finished rows. Fence state ([`markdown::BlockScanner`]), highlight carry
/// ([`highlight::Highlighter`]) and the **block layout** — the open quotes and
/// list items, each scope's last block, a pending blank line — thread across
/// the calls, so a completed line's rows never change: every row depends only
/// on the lines before it and the line itself. That is what lets scrollback
/// commits and the strip preview cost O(new line) instead of O(whole reply).
/// See `docs/markdown.md`.
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
    /// The open quotes and list items, outermost first.
    containers: Vec<Container>,
    /// The last block at the message's top level.
    root: Last,
    /// A blank source line since the last content line: the next block is set
    /// apart from the one before (unless it is a sibling item — codex compacts
    /// loose lists — or a lazy continuation, which a blank rules out).
    pending_blank: bool,
    /// Whether the last row emitted was blank, so a separator never stacks on
    /// one (a code block's own trailing blank line, say).
    last_row_blank: bool,
    /// The open fence still owes the blank row that sets it apart: it is
    /// emitted with the block's first code line, so a fence that is still
    /// empty — the reply so far ends on its opener — adds no trailing row.
    fence_gap: bool,
    /// The open fence's own indent past its containers' prefixes: CommonMark
    /// takes up to that many spaces off each of its code lines, so the code
    /// sits where the fence does.
    fence_indent: usize,
    /// In-progress GFM table accumulation (`docs/markdown.md`). A table is not
    /// prefix-stable — a later row can widen a column — so its source lines are
    /// buffered here and rendered **whole** only when the block closes (a line
    /// that does not continue it, or end-of-message via [`Self::flush`]).
    pub(super) table: TableState,
}

/// The [`AssistantRenderer`]'s GFM-table state machine (docs/table-streaming.md).
/// A candidate header is held in `PendingHeader` until the next line confirms it
/// (a matching [`markdown::table_delimiter`]) — the one-line lookahead a pipe
/// table needs — then `Buffering` accumulates the data rows, emitting **nothing**,
/// until a line that does not continue it (or end-of-message) closes the block
/// and it renders whole through [`table_block_rows`], its column widths fit to
/// **every** row. Nothing of an open table ever commits to scrollback, so a wide
/// later row can never invalidate a committed one (prefix-stability is trivial);
/// the streaming strip previews the forming block instead ([`StreamRender::preview`]).
/// The rows are the source lines with their container prefixes already
/// stripped, so a table inside a list item renders under the item's text.
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

/// A list item's rendered marker (`docs/markdown.md`), codex's geometry:
/// `depth` levels deep it sits [`LIST_NEST_WIDTH`] columns per level in; a
/// bullet is [`LIST_BULLET`] (or a task's box), an ordered item its number
/// right-aligned in the level's padding and dressed in [`list_marker_color`]
/// (a task's box follows it).
fn list_marker(
    kind: ListKind,
    number: u64,
    depth: usize,
    task: Option<bool>,
    base: Style,
) -> Vec<(String, Style)> {
    let pad = LIST_NEST_WIDTH * depth.saturating_sub(1);
    let task_box = |done: bool| if done { LIST_TASK_DONE } else { LIST_TASK_OPEN };
    match kind {
        ListKind::Bullet(_) => {
            let glyph = task.map_or(LIST_BULLET, task_box);
            vec![(format!("{}{glyph} ", " ".repeat(pad)), base)]
        }
        ListKind::Ordered(_) => {
            let width = pad + 1;
            let mut spans = vec![(format!("{number:>width$}. "), base.fg(list_marker_color()))];
            if let Some(done) = task {
                spans.push((format!("{} ", task_box(done)), base));
            }
            spans
        }
    }
}

/// Whether `rest` would continue a paragraph — no block starts on it — the
/// test a **lazy** continuation line must pass to stay inside the containers
/// it no longer indents for (CommonMark: `- item` then an unindented `text`
/// line is the item's text).
fn is_paragraph_text(rest: &str) -> bool {
    markdown::block_quote(rest).is_none()
        && markdown::list_item(rest).is_none()
        && markdown::thematic_break(rest).is_none()
        && markdown::heading_level(rest).is_none()
        && !markdown::is_fence_opener(rest)
        && !markdown::is_table_header_candidate(rest)
}

/// The number of leading spaces of `s`.
fn leading_spaces(s: &str) -> usize {
    s.len() - s.trim_start_matches(' ').len()
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
            containers: Vec::new(),
            root: Last::Nothing,
            pending_blank: false,
            last_row_blank: false,
            fence_gap: false,
            fence_indent: 0,
            table: TableState::None,
        }
    }

    /// Render the next source `line` into its finished rows, advancing the
    /// block-layout, fence and highlight state. The first row ever emitted
    /// carries the bullet; the rest are indented under it.
    pub(super) fn feed_line(&mut self, line: &str) -> Vec<Line<'static>> {
        let rows = self.content_rows(line);
        self.stamp(rows)
    }

    /// The row *content* spans for `line` (no bullet/indent prefix yet).
    fn content_rows(&mut self, line: &str) -> Vec<Vec<Span<'static>>> {
        // A CRLF reply's lines arrive ending in '\r' (the source is split on
        // '\n' alone): the CR is part of the line ending, not content — drop
        // it so fences, rules, and tables classify exactly as their LF twins
        // (a `---\r` used to render as literal prose) and no code row carries
        // a stray CR into scrollback or a /copy. A *mid*-line CR stays: it is
        // content (prose collapses it as whitespace). Batch and streaming
        // share this entry, so the two never disagree.
        let line = line.strip_suffix('\r').unwrap_or(line);
        // Inside a fence everything is code until the close.
        if self.highlighter.is_some() {
            return self.fenced_line(line);
        }
        let mut out = Vec::new();
        if self.in_table() {
            if self.continue_table(line) {
                return out;
            }
            // Not a table line: the block closes — render it whole (grid or
            // records, widths from every row), then the closing line.
            out = self.flush_table();
        }
        out.extend(self.block_line(line));
        out
    }

    /// A line inside an open fence: its container prefixes stripped
    /// **leniently** — a quote's `>` where present, up to an item's indent of
    /// spaces — so a model that under-indents a fenced block inside a list item
    /// still gets code, never a fence torn open into prose — then up to the
    /// fence's own indent (CommonMark); the rest is code, or the closing fence
    /// (which renders nothing).
    fn fenced_line(&mut self, line: &str) -> Vec<Vec<Span<'static>>> {
        let mut rest = line;
        for container in &self.containers {
            match container {
                Container::Quote(_) => {
                    if let Some(inner) = markdown::block_quote(rest) {
                        rest = inner;
                    }
                }
                Container::Item(item) => rest = &rest[leading_spaces(rest).min(item.indent)..],
            }
        }
        let rest = &rest[leading_spaces(rest).min(self.fence_indent)..];
        match self.scanner.classify(rest) {
            markdown::LineKind::CodeEnd => {
                self.highlighter = None;
                self.fence_gap = false;
                Vec::new()
            }
            _ => {
                let mut out = Vec::new();
                if std::mem::take(&mut self.fence_gap) {
                    out.extend(self.emit_separator());
                }
                out.extend(self.code_line(rest));
                out
            }
        }
    }

    /// How many open containers `line` continues, and what is left of it past
    /// their prefixes. A quote continues on a `>` (its marker and one space
    /// consumed); an item on a blank line or one indented to its content (that
    /// indent consumed).
    fn match_containers<'a>(&self, line: &'a str) -> (usize, &'a str) {
        let mut rest = line;
        for (i, container) in self.containers.iter().enumerate() {
            match container {
                Container::Quote(_) => match markdown::block_quote(rest) {
                    Some(inner) => rest = inner,
                    None => return (i, rest),
                },
                Container::Item(item) => {
                    if rest.trim().is_empty() {
                        continue;
                    }
                    if leading_spaces(rest) < item.indent {
                        return (i, rest);
                    }
                    rest = &rest[item.indent..];
                }
            }
        }
        (self.containers.len(), rest)
    }

    /// Whether `line` continues the open table — every container still matches
    /// and the rest is the delimiter a pending header needs, or another row —
    /// buffering it if so.
    fn continue_table(&mut self, line: &str) -> bool {
        let (matched, rest) = self.match_containers(line);
        if matched < self.containers.len() {
            return false;
        }
        let continued = match std::mem::replace(&mut self.table, TableState::None) {
            TableState::None => false,
            TableState::PendingHeader(header) => {
                let ncols = markdown::table_cells(&header).len();
                match markdown::table_delimiter(rest).filter(|a| a.len() == ncols) {
                    // Header + a matching delimiter → confirmed; buffer the data
                    // rows until the block closes (the widths need them all).
                    Some(aligns) => {
                        self.table = TableState::Buffering {
                            header,
                            aligns,
                            rows: Vec::new(),
                        };
                        true
                    }
                    None => {
                        self.table = TableState::PendingHeader(header);
                        false
                    }
                }
            }
            TableState::Buffering {
                header,
                aligns,
                mut rows,
            } => {
                let row = markdown::is_table_row(rest);
                if row {
                    // A pipe-carrying line that doesn't start with `|`, inside a
                    // leading-pipe table, is the previous row's hard-wrapped tail
                    // — re-join it instead of minting a phantom one-cell row.
                    match rows
                        .last()
                        .and_then(|prev| join_wrapped_table_row(prev, rest, aligns.len()))
                    {
                        Some(joined) => {
                            rows.pop();
                            rows.push(joined);
                        }
                        None => rows.push(rest.to_string()),
                    }
                }
                self.table = TableState::Buffering {
                    header,
                    aligns,
                    rows,
                };
                row
            }
        };
        if continued {
            self.scanner.classify(rest);
        }
        continued
    }

    /// Render one non-code, non-table line: continue (or close) the open
    /// containers, open the quotes and list items the line starts, then render
    /// its leaf — a heading, rule, fence, paragraph line or table header.
    fn block_line(&mut self, line: &str) -> Vec<Vec<Span<'static>>> {
        let (matched, rest) = self.match_containers(line);
        let blank = rest.trim().is_empty();
        if matched < self.containers.len() {
            // A lazy continuation: paragraph text with no blank before it
            // continues the open paragraph whatever its indent, so every
            // container stays open (`- item` then an unindented `more`).
            if !blank
                && !self.pending_blank
                && self.last() == Last::Paragraph
                && is_paragraph_text(rest)
            {
                self.scanner.classify(rest);
                return self.paragraph_line(rest.trim_start(), true);
            }
            self.close_containers(matched);
        }
        if blank {
            self.scanner.classify(rest);
            if self.scanner.in_indented() {
                // A blank line inside an indented code block is code content.
                return self.code_line("");
            }
            self.pending_blank = true;
            return Vec::new();
        }
        // An open indented code block runs on through every still-indented
        // line, one that looks like a list item included.
        if self.scanner.in_indented() && markdown::is_indented_code_line(rest) {
            self.scanner.classify(rest);
            return self.code_line(rest);
        }
        let mut out = Vec::new();
        let mut rest = rest;
        let mut opened = false;
        loop {
            // A thematic break outranks a bullet (`- - -`, `* * *`).
            if markdown::thematic_break(rest).is_some() {
                break;
            }
            if let Some(inner) = markdown::block_quote(rest) {
                out.extend(self.open_quote());
                rest = inner;
                opened = true;
                continue;
            }
            if let Some(item) = markdown::list_item(rest) {
                let (rows, content, task) = self.open_item(rest, &item);
                out.extend(rows);
                rest = content;
                opened = true;
                if task {
                    // A task box's text is the item's paragraph, whatever it
                    // looks like (`- [x] 10)` is not a nested list).
                    self.scanner.note_prose(rest);
                    if rest.trim().is_empty() {
                        out.extend(self.emit(vec![Vec::new()]));
                    } else {
                        out.extend(self.paragraph_line(rest, false));
                    }
                    return out;
                }
                if rest.trim().is_empty() {
                    break;
                }
                continue;
            }
            break;
        }
        out.extend(self.leaf_line(rest, opened));
        out
    }

    /// Render a line's leaf — what is left past its container prefixes and
    /// markers. `opened` says the line opened a container, whose first block
    /// this is: never a paragraph continuation, never a table.
    fn leaf_line(&mut self, rest: &str, opened: bool) -> Vec<Vec<Span<'static>>> {
        if rest.trim().is_empty() {
            self.scanner.classify(rest);
            if self.marker_pending() {
                // An item with nothing after its marker: the marker row alone.
                return self.emit(vec![Vec::new()]);
            }
            self.pending_blank = true;
            return Vec::new();
        }
        match self.scanner.classify(rest) {
            markdown::LineKind::CodeStart(lang) => {
                // A fence opens a block, set apart like any other — by a blank
                // row its first code line brings (`fence_gap`); its own line
                // renders nothing. On an item's marker line the marker takes a
                // row of its own first.
                self.fence_gap = self.last() != Last::Nothing && !self.last_row_blank;
                self.fence_indent = leading_spaces(rest);
                let mut out = Vec::new();
                if self.marker_pending() {
                    out.extend(self.emit(vec![Vec::new()]));
                }
                self.highlighter = Some(highlight::Highlighter::new(
                    lang.as_deref(),
                    super::palette::palette().code,
                ));
                self.close_block(Last::Block);
                out
            }
            markdown::LineKind::Code => {
                // An indented code block's first line.
                let mut out = self.separator();
                out.extend(self.code_line(rest));
                self.close_block(Last::Block);
                out
            }
            markdown::LineKind::CodeEnd => Vec::new(),
            markdown::LineKind::Prose => self.prose_leaf(rest, opened),
        }
    }

    /// A prose leaf: an ATX heading (markers kept, styled per level — codex
    /// parity — its text inline markdown), a thematic break (`———`), a table
    /// header candidate (buffered for its delimiter), or a paragraph line.
    fn prose_leaf(&mut self, rest: &str, opened: bool) -> Vec<Vec<Span<'static>>> {
        if let Some((level, text)) = markdown::heading_level(rest) {
            let mut out = self.separator();
            out.extend(self.heading_rows(level, text));
            self.close_block(Last::Block);
            return out;
        }
        if markdown::thematic_break(rest).is_some() {
            // Codex renders `---`/`***`/`___` as an unstyled `———` rule on its
            // own row (`Event::Rule`) — also right after a paragraph line, where
            // CommonMark would read a `-` run as a setext underline: in a reply
            // it is a separator that lost its blank line, and showing `---`
            // would be the one raw marker left.
            let mut out = self.separator();
            let rule = vec![Span::styled(THEMATIC_BREAK.to_string(), self.base_style())];
            out.extend(self.emit(vec![rule]));
            self.close_block(Last::Block);
            return out;
        }
        // Header candidacy needs the leading pipe (`is_table_header_candidate`):
        // the headerless GFM form would let a later `|` re-shape a prose line
        // whose rows the streaming committer already froze into scrollback.
        // Tables live at the top level and in list items, never in a quote or
        // on a marker line.
        if !opened && !self.in_quote() && markdown::is_table_header_candidate(rest) {
            self.table = TableState::PendingHeader(rest.to_string());
            return Vec::new();
        }
        self.paragraph_line(rest, !opened)
    }

    /// Render a paragraph line: inline-parsed, span-wrapped under the
    /// containers' prefixes. With `may_continue`, a line right after a
    /// paragraph line (no blank between) continues it — codex's soft break, a
    /// new row with no separator; otherwise it opens a new paragraph, set apart
    /// from the block before. Emphasis is line-local, so a complete line's
    /// styling is final; a trailing line with an open marker is withheld by
    /// `StreamRender` (`markdown::has_open_inline`).
    fn paragraph_line(&mut self, text: &str, may_continue: bool) -> Vec<Vec<Span<'static>>> {
        let continues = may_continue && !self.pending_blank && self.last() == Last::Paragraph;
        let mut out = if continues {
            Vec::new()
        } else {
            self.separator()
        };
        let segments = inline_spans(&markdown::parse_inline(text), self.base_style());
        let rows = self.wrap(&segments);
        out.extend(self.emit(rows));
        self.close_block(Last::Paragraph);
        out
    }

    /// A heading's rows: the `#` run (normalised to one space after it) and
    /// the inline-parsed text, both in the level's dress, word-wrapped.
    fn heading_rows(&mut self, level: u8, text: &str) -> Vec<Vec<Span<'static>>> {
        let style = self.base_style().patch(heading_style(level));
        let hashes = "#".repeat(level as usize);
        let mut segments = if text.is_empty() {
            vec![(hashes, style)]
        } else {
            vec![(format!("{hashes} "), style)]
        };
        segments.extend(inline_spans(&markdown::parse_inline(text), style));
        let rows = self
            .wrap(&segments)
            .into_iter()
            .map(|row| {
                // Restore the heading dress on word separators, then coalesce
                // equal styles: a combining mark must share the preceding
                // space's span or ratatui drops its zero-width grapheme.
                let mut merged: Vec<Span<'static>> = Vec::with_capacity(row.len());
                for mut span in row {
                    span.style = style.patch(span.style);
                    if let Some(previous) = merged.last_mut()
                        && previous.style == span.style
                    {
                        previous.content.to_mut().push_str(&span.content);
                    } else {
                        merged.push(span);
                    }
                }
                merged
            })
            .collect();
        self.emit(rows)
    }

    /// Render one fenced/indented **code** line under the containers'
    /// prefixes: tabs expanded so indentation survives, syntax-highlighted by
    /// the open fence's language (plain for an indented block), hard-broken on
    /// the width left beside the prefixes.
    fn code_line(&mut self, line: &str) -> Vec<Vec<Span<'static>>> {
        let expanded = expand_code_tabs(line);
        let segs = match self.highlighter.as_mut() {
            Some(h) => h.line(&expanded),
            None => vec![highlight::Seg {
                text: expanded.into_owned(),
                style: highlight::plain_style(super::palette::palette().code),
            }],
        };
        let styled: Vec<(String, Style)> = segs.into_iter().map(|s| (s.text, s.style)).collect();
        let width = self.width_beside(false);
        let rows = code_content_rows(&linkify_code_segments(styled), width);
        self.emit(rows)
    }

    /// Open a blockquote in the current scope — set apart from the block before.
    fn open_quote(&mut self) -> Vec<Vec<Span<'static>>> {
        let out = self.separator();
        self.close_block(Last::Block);
        self.containers.push(Container::Quote(Last::Nothing));
        out
    }

    /// Open the list item `item` (parsed from `rest`) in the current scope,
    /// returning the separator rows it needs, the content after its marker
    /// (and task box), and whether it had a task box. A matching item after another is a sibling: it takes the
    /// list's next number and a blank row only when the item before it took
    /// more than one (codex's AfterMultiline). Otherwise it starts a new list —
    /// numbered from its own marker, set apart from the block before unless it
    /// is nested in an item (nested lists hug their parent, as in codex).
    fn open_item<'a>(
        &mut self,
        rest: &'a str,
        item: &markdown::ListItem<'a>,
    ) -> (Vec<Vec<Span<'static>>>, &'a str, bool) {
        let (kind, start, marker_len) = match item.marker {
            markdown::ListMarker::Bullet(c) => (ListKind::Bullet(c), 1, 1),
            markdown::ListMarker::Ordered(n, delim) => {
                let digits = rest[item.indent..]
                    .bytes()
                    .take_while(u8::is_ascii_digit)
                    .count();
                (ListKind::Ordered(delim), n, digits + 1)
            }
        };
        let nested = matches!(self.containers.last(), Some(Container::Item(_)));
        let (number, separate) = match self.last() {
            Last::List(list) if list.kind == kind => (list.next, list.prev_rows > 1),
            last => (start, last != Last::Nothing && !nested),
        };
        let out = if separate && !self.last_row_blank {
            self.emit_separator()
        } else {
            Vec::new()
        };
        self.set_last(Last::List(ListState {
            kind,
            next: number.saturating_add(1),
            prev_rows: 0,
        }));
        self.pending_blank = false;
        // Where a later line must indent to stay in the item: the content's
        // column, CommonMark's rule — one column past the marker when nothing
        // follows it, or when 5+ spaces do (that content is indented code).
        let content_at = rest.len() - item.content.len();
        let spaces = content_at - (item.indent + marker_len);
        let indent = if item.content.is_empty() || spaces > 4 {
            item.indent + marker_len + 1
        } else {
            content_at
        };
        let (task, content) = markdown::task_box(item.content);
        let depth = 1 + self
            .containers
            .iter()
            .rev()
            .take_while(|c| !matches!(c, Container::Quote(_)))
            .count();
        let marker = list_marker(kind, number, depth, task, self.base_style());
        let hang = marker.iter().map(|(text, _)| cols(text)).sum();
        self.containers.push(Container::Item(Item {
            indent,
            marker,
            hang,
            marker_pending: true,
            rows: 0,
            last: Last::Nothing,
        }));
        (out, content, task.is_some())
    }

    /// Close every container past the first `keep`, innermost first. A closing
    /// item hands its row count to its list, for the next sibling's spacing.
    fn close_containers(&mut self, keep: usize) {
        while self.containers.len() > keep {
            if let Some(Container::Item(item)) = self.containers.pop()
                && let Last::List(list) = self.last_mut()
            {
                list.prev_rows = item.rows;
            }
        }
    }

    /// Record that a block of kind `last` was just rendered in the current
    /// scope, settling any pending blank line.
    fn close_block(&mut self, last: Last) {
        self.set_last(last);
        self.pending_blank = false;
    }

    /// The blank row that sets a new block apart from the block before it in
    /// the current scope — none for a scope's first block, and never stacked on
    /// a blank row already there.
    fn separator(&mut self) -> Vec<Vec<Span<'static>>> {
        if self.last() == Last::Nothing || self.last_row_blank {
            return Vec::new();
        }
        self.emit_separator()
    }

    /// Emit one separator row: empty, or — inside a quote — the quote's prefix,
    /// so the quote's `>` column stays unbroken (codex's blank-line rule).
    fn emit_separator(&mut self) -> Vec<Vec<Span<'static>>> {
        let row: Vec<Span<'static>> = if self.in_quote() {
            self.row_prefix(false)
                .into_iter()
                .map(|(text, style)| Span::styled(text, style))
                .collect()
        } else {
            Vec::new()
        };
        self.count_row();
        self.last_row_blank = true;
        vec![row]
    }

    /// Prefix each content row with the containers' prefixes (the first row
    /// carrying a pending item marker), counting the rows into every open item.
    fn emit(&mut self, rows: Vec<Vec<Span<'static>>>) -> Vec<Vec<Span<'static>>> {
        let mut out = Vec::with_capacity(rows.len());
        for (i, row) in rows.into_iter().enumerate() {
            let marker_row = i == 0 && self.marker_pending();
            let mut full: Vec<Span<'static>> = self
                .row_prefix(i == 0)
                .into_iter()
                .map(|(text, style)| Span::styled(text, style))
                .collect();
            if i == 0 {
                for container in &mut self.containers {
                    if let Container::Item(item) = container {
                        item.marker_pending = false;
                    }
                }
            }
            self.last_row_blank = !marker_row && row.iter().all(|s| s.content.trim().is_empty());
            full.extend(row);
            out.push(full);
            self.count_row();
        }
        out
    }

    /// The containers' prefix for a row (`first` = a block's first row, which
    /// carries a pending marker). Quotes add `> ` each; of the items between
    /// two quotes only the innermost speaks — its marker on its first row, its
    /// hang after — since a nested marker already carries its depth's padding
    /// (codex's `prefix_spans`).
    fn row_prefix(&self, first: bool) -> Vec<(String, Style)> {
        let quote = Style::new().fg(quote_color());
        let mut out = Vec::new();
        for (i, container) in self.containers.iter().enumerate() {
            match container {
                Container::Quote(_) => out.push(("> ".to_string(), quote)),
                Container::Item(item) => {
                    let innermost = self.containers[i + 1..]
                        .iter()
                        .take_while(|c| !matches!(c, Container::Quote(_)))
                        .all(|c| !matches!(c, Container::Item(_)));
                    if !innermost {
                        continue;
                    }
                    if first && item.marker_pending {
                        out.extend(item.marker.iter().cloned());
                    } else {
                        out.push((" ".repeat(item.hang), Style::default()));
                    }
                }
            }
        }
        out
    }

    /// The columns left for content beside a row's prefix (`first` = the
    /// block's first row).
    fn width_beside(&self, first: bool) -> u16 {
        let used: usize = self.row_prefix(first).iter().map(|(t, _)| cols(t)).sum();
        (self.content_width as usize).saturating_sub(used).max(1) as u16
    }

    /// Span-wrap styled segments to the widths left beside the prefixes — the
    /// first row beside a pending marker, the rest beside the hang.
    fn wrap(&self, segments: &[(String, Style)]) -> Vec<Vec<Span<'static>>> {
        wrap_inline_hanging(segments, self.width_beside(true), self.width_beside(false))
    }

    /// Count one emitted row into every open item (codex's AfterMultiline
    /// reads an item's rows, nested content included).
    fn count_row(&mut self) {
        for container in &mut self.containers {
            if let Container::Item(item) = container {
                item.rows += 1;
            }
        }
    }

    /// Whether an open item still owes its marker row.
    fn marker_pending(&self) -> bool {
        self.containers
            .iter()
            .any(|c| matches!(c, Container::Item(item) if item.marker_pending))
    }

    /// Whether the current line sits inside a blockquote.
    fn in_quote(&self) -> bool {
        self.containers
            .iter()
            .any(|c| matches!(c, Container::Quote(_)))
    }

    /// The text style a block starts from — dim inside a quote.
    fn base_style(&self) -> Style {
        if self.in_quote() {
            Style::new().fg(quote_color())
        } else {
            Style::default()
        }
    }

    /// The last block of the current (innermost) scope.
    fn last(&self) -> Last {
        match self.containers.last() {
            Some(Container::Quote(last)) => *last,
            Some(Container::Item(item)) => item.last,
            None => self.root,
        }
    }

    /// The current scope's last block, to update.
    fn last_mut(&mut self) -> &mut Last {
        match self.containers.last_mut() {
            Some(Container::Quote(last)) => last,
            Some(Container::Item(item)) => &mut item.last,
            None => &mut self.root,
        }
    }

    /// Set the current scope's last block.
    fn set_last(&mut self, last: Last) {
        *self.last_mut() = last;
    }

    /// Emit any open table's rows, clearing the state — called when a line
    /// does not continue it (and by [`Self::flush`] at end-of-message). A
    /// never-confirmed `PendingHeader` renders as the plain prose line it
    /// actually was; a `Buffering` table renders whole (a header-only table is
    /// the opening + bottom border), set apart like any block and drawn under
    /// the containers' prefixes.
    fn flush_table(&mut self) -> Vec<Vec<Span<'static>>> {
        match std::mem::replace(&mut self.table, TableState::None) {
            TableState::None => Vec::new(),
            TableState::PendingHeader(header) => self.paragraph_line(&header, true),
            TableState::Buffering {
                header,
                aligns,
                rows,
            } => {
                let mut out = self.separator();
                let width = self.width_beside(false);
                out.extend(self.emit(table_block_rows(&header, &aligns, &rows, width)));
                self.close_block(Last::Block);
                out
            }
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
