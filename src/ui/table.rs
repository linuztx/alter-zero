//! GFM table rendering: column sizing, borders, and the narrow-terminal
//! record fallback. See `docs/markdown.md` and `docs/table-streaming.md`.

use super::inline::tokenize_words;
use super::inline::{inline_spans, wrap_inline};
use super::theme::*;
use super::wrap::{cols, segments_cols};
use super::*;

/// Normalise a raw table `line` to exactly `ncols` styled cells under `base`
/// (bold for a header, plain for data): pad missing cells empty, drop extras,
/// each inline-parsed like prose so `` `code` `` / `**bold**` render styled.
fn normalize_row(line: &str, ncols: usize, base: Style) -> Vec<Vec<(String, Style)>> {
    let cells = markdown::table_cells(line);
    (0..ncols)
        .map(|i| table_cell_segments(cells.get(i).map_or("", String::as_str), base))
        .collect()
}

/// The per-column natural (unwrapped) width — the widest *rendered* cell over
/// `rows` (each already `ncols` styled cells), floored at 1.
fn natural_col_widths(rows: &[Vec<Vec<(String, Style)>>], ncols: usize) -> Vec<usize> {
    let mut w = vec![1usize; ncols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            w[i] = w[i].max(segments_cols(cell));
        }
    }
    w
}

/// The per-column **word** width — the widest single unbreakable word over
/// `rows`, words tokenized exactly as [`wrap_inline`] will break them. A column
/// allocated at least this many display columns wraps only at spaces; below it,
/// some word has to hard-break mid-token. [`allocate_column_widths`] seats every
/// column here first, which is what keeps a 33-column IPv6 (or a bare
/// `OPENROUTER_API_KEY`) whole while a column that *can* wrap gives way.
fn natural_word_widths(rows: &[Vec<Vec<(String, Style)>>], ncols: usize) -> Vec<usize> {
    let mut w = vec![1usize; ncols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            for word in tokenize_words(cell) {
                w[i] = w[i].max(segments_cols(&word));
            }
        }
    }
    w
}

/// Allocate the per-column widths for a grid that must fit `avail` display
/// columns, given each column's `natural` (widest cell) width and `word` width
/// (its widest single unbreakable word — [`natural_word_widths`]).
///
/// Three cases, in order — Claude Code's fit (docs/table-streaming.md):
///
/// 1. **The natural grid fits.** Return it: the table stays as narrow as its
///    content rather than stretching to the terminal.
/// 2. **It overflows but every column's word-safe floor fits.** Each column is
///    seated at `min(natural, word)` — the width it needs to wrap at *spaces*
///    only — and the surplus is split in proportion to each column's **unmet
///    demand** (`natural - floor`), largest-remainder so the columns fill the
///    budget exactly. This is what makes a table read like Claude Code's: a
///    column whose width is driven by one long outlier (`cargo clippy
///    --all-targets -- -D warnings`) doesn't hoard room every other row wastes,
///    and the column with the genuinely long content gets it instead. It also
///    keeps an unbreakable token (a 33-column IPv6) whole whenever another
///    column can give way at a space.
/// 3. **Even the floors overflow.** Some token has to hard-break, so level the
///    **widest column** down one display column at a time (ties leftmost-first),
///    floored at [`TABLE_MIN_COL`]: a short cell (`facebook.com`, a `Packets`
///    header) keeps its natural width for as long as possible. A proportional
///    shrink here would starve *every* column at once, breaking short words
///    mid-cell (an earlier bug).
///
/// Allocated widths are **wrap** widths throughout, so cells word-wrap into
/// them (taller rows) rather than truncating with a `…`.
pub(super) fn allocate_column_widths(
    natural: &[usize],
    word: &[usize],
    avail: usize,
) -> Vec<usize> {
    let n = natural.len();
    if n == 0 {
        return Vec::new();
    }
    let overhead = 3 * n + 1; // `│`×(n+1) plus two pad spaces × n
    let content_avail = avail.saturating_sub(overhead);
    let total: usize = natural.iter().sum();
    if total == 0 || total <= content_avail {
        return natural.to_vec();
    }
    // The width each column needs to wrap at spaces only — never more than its
    // natural width, never below the floor a bordered cell needs to be legible.
    let floor: Vec<usize> = natural
        .iter()
        .zip(word)
        .map(|(&nat, &w)| nat.min(w.max(TABLE_MIN_COL)))
        .collect();
    let seated: usize = floor.iter().sum();
    if seated > content_avail {
        return level_widest_columns(natural, content_avail);
    }
    // Spend the surplus where the content still doesn't fit. `demand_total`
    // exceeds `surplus` (the naturals overflow by definition), so no column can
    // be handed more than it asked for.
    let surplus = content_avail - seated;
    let demand: Vec<usize> = natural.iter().zip(&floor).map(|(&n, &f)| n - f).collect();
    let demand_total: usize = demand.iter().sum();
    if demand_total == 0 {
        return floor;
    }
    let mut w = floor;
    let mut spent = 0usize;
    // (remainder, index) — largest-remainder rounding hands out the columns
    // integer division dropped, so the grid fills `content_avail` exactly.
    let mut remainders: Vec<(usize, usize)> = Vec::with_capacity(n);
    for (i, &d) in demand.iter().enumerate() {
        let exact = surplus * d;
        w[i] += exact / demand_total;
        spent += exact / demand_total;
        remainders.push((exact % demand_total, i));
    }
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut left = surplus - spent;
    for &(_, i) in &remainders {
        if left == 0 {
            break;
        }
        if w[i] < natural[i] {
            w[i] += 1;
            left -= 1;
        }
    }
    w
}

/// Case 3 of [`allocate_column_widths`]: shrink the **widest** column one
/// display column at a time (ties leftmost-first, so equal columns level
/// evenly) until the grid fits `content_avail`, never below
/// [`TABLE_MIN_COL`].
fn level_widest_columns(natural: &[usize], content_avail: usize) -> Vec<usize> {
    // Pre-clamp to the whole content budget (a lone column can never usefully
    // exceed it), bounding the loop to O(content_avail · n) even for a
    // pathological megabyte-wide cell — the preview re-renders per frame.
    let mut w: Vec<usize> = natural
        .iter()
        .map(|&x| x.min(content_avail.max(1)).max(1))
        .collect();
    let mut sum: usize = w.iter().sum();
    while sum > content_avail {
        // The first of the widest columns still above the floor.
        let mut widest: Option<usize> = None;
        for (i, &x) in w.iter().enumerate() {
            if x > TABLE_MIN_COL && widest.is_none_or(|b| x > w[b]) {
                widest = Some(i);
            }
        }
        let Some(i) = widest else {
            break; // every column already at the floor — unavoidable at tiny widths
        };
        w[i] -= 1;
        sum -= 1;
    }
    w
}

/// A table border row: `left` + per-column `─`×(w+2) joined by `mid`, then
/// `right` — e.g. `┌──┬──┐` / `├──┼──┤` / `└──┴──┘`, rendered dim.
fn table_border_row(col_w: &[usize], left: char, mid: char, right: char) -> Vec<Span<'static>> {
    let mut s = String::new();
    s.push(left);
    for (i, &w) in col_w.iter().enumerate() {
        if i > 0 {
            s.push(mid);
        }
        s.extend(std::iter::repeat_n('─', w + 2));
    }
    s.push(right);
    vec![Span::styled(s, Style::new().fg(TABLE_BORDER_COLOR))]
}

/// Render one table row (its per-column styled `cells`) into box-drawing content
/// rows, each cell **word-wrapped** into its column width via [`wrap_inline`]
/// (which hard-breaks an over-wide token grapheme-by-grapheme, like codex/img2).
/// Returns as many rows as the tallest wrapped cell — so a data row spans several
/// rows when its cells wrap — each `│`-framed with single-space margins, its
/// styled segments padded/aligned per column.
///
/// A cell shorter than the row is **centred vertically** in it (Claude Code's
/// look): beside a neighbour that wrapped to three rows, a one-line cell sits on
/// the middle row rather than the top, so the two read as one record. The
/// leading blank rows round down, so a one-line cell in a two-row row stays on
/// top (`(height - lines) / 2`, matching how [`pad_cell_line`] rounds its
/// horizontal centring).
pub(super) fn table_row_lines(
    cells: &[Vec<(String, Style)>],
    col_w: &[usize],
    aligns: &[markdown::Alignment],
) -> Vec<Vec<Span<'static>>> {
    let border = Style::new().fg(TABLE_BORDER_COLOR);
    let wrapped: Vec<Vec<Vec<Span<'static>>>> = cells
        .iter()
        .enumerate()
        .map(|(i, cell)| wrap_inline(cell, col_w[i] as u16))
        .collect();
    let height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
    (0..height)
        .map(|r| {
            let mut spans = vec![Span::styled("│".to_string(), border)];
            for (i, cell_rows) in wrapped.iter().enumerate() {
                spans.push(Span::raw(" "));
                // Where this cell's own rows start, so a short cell is centred
                // in the row instead of hugging its top.
                let top = height.saturating_sub(cell_rows.len()) / 2;
                let line = r
                    .checked_sub(top)
                    .and_then(|k| cell_rows.get(k))
                    .cloned()
                    .unwrap_or_default();
                spans.extend(pad_cell_line(line, col_w[i], aligns[i]));
                spans.push(Span::raw(" "));
                spans.push(Span::styled("│".to_string(), border));
            }
            spans
        })
        .collect()
}

/// Pad a wrapped cell sub-line's `spans` to `w` columns per `align` — no
/// truncation, since [`wrap_inline`] already fit it to `w`. Padding is
/// default-styled (an invisible space).
fn pad_cell_line(
    spans: Vec<Span<'static>>,
    w: usize,
    align: markdown::Alignment,
) -> Vec<Span<'static>> {
    let body_w: usize = spans.iter().map(|s| cols(&s.content)).sum();
    let pad = w.saturating_sub(body_w);
    let (left, right) = match align {
        markdown::Alignment::Right => (pad, 0),
        markdown::Alignment::Center => (pad / 2, pad - pad / 2),
        markdown::Alignment::Left | markdown::Alignment::None => (0, pad),
    };
    let mut out = Vec::with_capacity(spans.len() + 2);
    if left > 0 {
        out.push(Span::raw(" ".repeat(left)));
    }
    out.extend(spans);
    if right > 0 {
        out.push(Span::raw(" ".repeat(right)));
    }
    out
}

/// The per-column widths for a table from its `header` and **every** data row,
/// fit to `content_width` — the widths each cell then wraps into. Computed only
/// when the block closes (the rows are buffered until then), so a wide later
/// row can never be shattered by widths guessed from an earlier one — the
/// failure of the old first-data-row lock (docs/table-streaming.md).
pub(super) fn table_column_widths(
    header: &str,
    rows: &[String],
    ncols: usize,
    content_width: u16,
) -> Vec<usize> {
    let mut all = vec![normalize_row(header, ncols, table_header_style())];
    all.extend(
        rows.iter()
            .map(|r| normalize_row(r, ncols, Style::default())),
    );
    allocate_column_widths(
        &natural_col_widths(&all, ncols),
        &natural_word_widths(&all, ncols),
        content_width as usize,
    )
}

/// The opening rows of a table: the top border, the (word-wrapped, bold) header
/// row, and the header/body separator.
fn table_open_rows(
    header: &str,
    col_w: &[usize],
    aligns: &[markdown::Alignment],
) -> Vec<Vec<Span<'static>>> {
    let header_style = Style::new().add_modifier(Modifier::BOLD);
    let header_cells = normalize_row(header, aligns.len(), header_style);
    let header_aligns: Vec<markdown::Alignment> =
        aligns.iter().copied().map(header_align).collect();
    let mut out = vec![table_border_row(col_w, '┌', '┬', '┐')];
    out.extend(table_row_lines(&header_cells, col_w, &header_aligns));
    out.push(table_border_row(col_w, '├', '┼', '┤'));
    out
}

/// How a **header** cell aligns given the column's delimiter `align`: centred
/// when the delimiter declared nothing (Claude Code's look — a centred label
/// over left-aligned data reads as a column heading rather than a first row),
/// otherwise the alignment the author actually asked for. Only the default
/// changes; a declared `:--`/`:-:`/`--:` still wins, since a markdown renderer
/// must not discard stated intent.
fn header_align(align: markdown::Alignment) -> markdown::Alignment {
    match align {
        markdown::Alignment::None => markdown::Alignment::Center,
        declared => declared,
    }
}

/// The bold header style shared by grid header cells and record labels.
fn table_header_style() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// Split `line` into exactly `ncols` raw cell texts (padding/truncating), for the
/// records fallback's stored labels.
fn normalize_raw_cells(line: &str, ncols: usize) -> Vec<String> {
    let mut cells = markdown::table_cells(line);
    cells.resize(ncols, String::new());
    cells
}

/// Decide, at the block's close, whether the grid is too cramped to scan and
/// should render as vertical key/value **records** instead (codex's key/value
/// transpose). Made from the header and **every** buffered row — the same full
/// knowledge the widths use (docs/table-streaming.md). True when a column is
/// both **narrow** (`< TABLE_SCANNABLE_COL`) **and** holds content that wraps
/// into `>= TABLE_RECORDS_MIN_LINES` rows at its allocated width — i.e. the
/// grid is growing tall because columns are starved, not because one wide cell
/// is a legitimately long narrative. Never for a single-column table (it's
/// just a list).
pub(super) fn table_should_use_records(header: &str, rows: &[String], col_w: &[usize]) -> bool {
    let ncols = col_w.len();
    if ncols < 2 {
        return false;
    }
    let all: Vec<Vec<Vec<(String, Style)>>> =
        std::iter::once(normalize_row(header, ncols, table_header_style()))
            .chain(
                rows.iter()
                    .map(|r| normalize_row(r, ncols, Style::default())),
            )
            .collect();
    col_w.iter().enumerate().any(|(i, &w)| {
        w < TABLE_SCANNABLE_COL
            && all
                .iter()
                .any(|cells| wrap_inline(&cells[i], w as u16).len() >= TABLE_RECORDS_MIN_LINES)
    })
}

/// A `─` rule separating two records, dim like a table border. Capped at
/// [`TABLE_RECORD_SEPARATOR_WIDTH`] (shrinking to fit a narrower `width`) rather
/// than spanning the full content width — Claude Code's cleaner short separator.
pub(super) fn table_record_separator(width: usize) -> Vec<Span<'static>> {
    let w = width.clamp(1, TABLE_RECORD_SEPARATOR_WIDTH);
    vec![Span::styled(
        "─".repeat(w),
        Style::new().fg(TABLE_BORDER_COLOR),
    )]
}

/// Render one data row as a vertical **key/value record** block (Claude Code's
/// key/value transpose): for each column a `label: value` field — the bold label,
/// a `: ` separator, then the value inline-parsed and wrapped into the remaining
/// width, continuation lines aligned under the value. No aligned label column and
/// no trailing padding, so it reads clean and compact. When even `label: ` plus a
/// minimum value can't fit (`content_width` too small), the field **stacks**: the
/// label (with its colon) on its own line, the value wrapped and indented beneath.
/// No box drawing, nothing truncated.
pub(super) fn table_record_block(
    labels: &[String],
    row: &str,
    content_width: u16,
) -> Vec<Vec<Span<'static>>> {
    let ncols = labels.len();
    let row_cells = normalize_row(row, ncols, Style::default());
    let content_width = content_width as usize;
    let mut out: Vec<Vec<Span<'static>>> = Vec::new();
    for (label, value) in labels.iter().zip(&row_cells) {
        let label_segs = table_cell_segments(label, table_header_style());
        // The `label: ` prefix — bold label, colon, single space.
        let prefix_w = segments_cols(&label_segs) + cols(": ");
        if prefix_w + TABLE_RECORD_MIN_VALUE <= content_width {
            let value_width = content_width.saturating_sub(prefix_w).max(1);
            for (k, vrow) in wrap_inline(value, value_width as u16)
                .into_iter()
                .enumerate()
            {
                let mut spans: Vec<Span<'static>> = Vec::new();
                if k == 0 {
                    spans.extend(label_segs.iter().map(|(t, s)| Span::styled(t.clone(), *s)));
                    spans.push(Span::raw(": "));
                } else {
                    spans.push(Span::raw(" ".repeat(prefix_w)));
                }
                spans.extend(vrow);
                out.push(spans);
            }
        } else {
            // Stacked: the label (with colon) on its own line, value indented beneath.
            let mut head: Vec<Span<'static>> = label_segs
                .iter()
                .map(|(t, s)| Span::styled(t.clone(), *s))
                .collect();
            head.push(Span::raw(":"));
            out.push(head);
            let value_width = content_width
                .saturating_sub(TABLE_RECORD_STACK_INDENT)
                .max(1);
            for vrow in wrap_inline(value, value_width as u16) {
                let mut spans = vec![Span::raw(" ".repeat(TABLE_RECORD_STACK_INDENT))];
                spans.extend(vrow);
                out.push(spans);
            }
        }
    }
    out
}

/// Re-join a hard-wrapped table row (docs/table-streaming.md). A model echoing
/// terminal-wrapped source can carry a line break MID-ROW, so the row arrives
/// as a leading-pipe line plus a fragment (`| google | … | 86.8 / 89.8 /` ␤
/// `96.4 ms |`). Strict GFM reads the fragment as a row of its own — a phantom
/// one-cell row — but in a leading-pipe table a genuine row *starts* with `|`;
/// a pipe-carrying line that doesn't is really the previous row's tail. Returns
/// the space-joined row when `prev` is a leading-pipe row, `line` isn't, and
/// the merged row still fits the delimiter's `ncols` (a fragment that would
/// overflow the column count is a real — if style-mixed — row, e.g. a no-pipe
/// `c | d` after a complete `| a | b |`; GFM keeps it a row and so do we).
pub(super) fn join_wrapped_table_row(prev: &str, line: &str, ncols: usize) -> Option<String> {
    if !prev.trim_start().starts_with('|') || line.trim_start().starts_with('|') {
        return None;
    }
    let joined = format!("{} {}", prev.trim_end(), line.trim_start());
    (markdown::table_cells(&joined).len() <= ncols).then_some(joined)
}

/// Render a complete GFM table block — the `header`, the delimiter's `aligns`,
/// and the buffered data `rows` — into content rows (`docs/markdown.md`).
/// Column widths are allocated from the header **and every data row** (fit to
/// `content_width`), so the grid always fits its real content — the same full
/// knowledge then decides grid vs. key/value records. THE table renderer:
/// the batch path, the streaming close/`flush`, and the forming-table preview
/// all emit a table only through here, so they can never disagree
/// (docs/table-streaming.md).
pub(super) fn table_block_rows(
    header: &str,
    aligns: &[markdown::Alignment],
    rows: &[String],
    content_width: u16,
) -> Vec<Vec<Span<'static>>> {
    let ncols = aligns.len();
    let col_w = table_column_widths(header, rows, ncols, content_width);
    if let Some(first) = rows.first()
        && table_should_use_records(header, rows, &col_w)
    {
        let labels = normalize_raw_cells(header, ncols);
        let mut out = table_record_block(&labels, first, content_width);
        for row in &rows[1..] {
            out.push(table_record_separator(content_width as usize));
            out.extend(table_record_block(&labels, row, content_width));
        }
        return out;
    }
    let mut out = table_open_rows(header, &col_w, aligns);
    for (i, row) in rows.iter().enumerate() {
        // Claude Code's full grid: every data row is framed — a `├──┼──┤` rule
        // between consecutive rows, not just under the header.
        if i > 0 {
            out.push(table_border_row(&col_w, '├', '┼', '┤'));
        }
        out.extend(table_row_lines(
            &normalize_row(row, ncols, Style::default()),
            &col_w,
            aligns,
        ));
    }
    out.push(table_border_row(&col_w, '└', '┴', '┘'));
    out
}

/// [`table_block_rows`] over raw table `lines` (`lines[0]` the header,
/// `lines[1]` the delimiter, `lines[2..]` the data rows) — a test-only
/// convenience; production buffers a table's lines in [`AssistantRenderer`]
/// and renders through [`table_block_rows`] when the block closes.
#[cfg(test)]
pub(super) fn table_content_rows(lines: &[String], width: u16) -> Vec<Vec<Span<'static>>> {
    let Some(aligns) = lines.get(1).and_then(|l| markdown::table_delimiter(l)) else {
        return Vec::new(); // not a confirmed table (never reached in practice)
    };
    if aligns.is_empty() {
        return Vec::new();
    }
    let header = lines.first().map_or("", String::as_str);
    table_block_rows(header, &aligns, lines.get(2..).unwrap_or_default(), width)
}

/// A table cell's markdown inline-parsed into styled segments (markers removed),
/// under `base` (bold for a header cell) — the rendered counterpart of the raw
/// cell text, used both to size columns and to draw them.
pub(super) fn table_cell_segments(cell: &str, base: Style) -> Vec<(String, Style)> {
    inline_spans(&markdown::parse_inline(cell), base)
}
