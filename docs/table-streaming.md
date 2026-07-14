# Streaming GFM tables (progressive, wrapping grid)

GFM pipe tables (`docs/markdown.md`) used to be **buffered whole** and committed
to scrollback only once the block closed — a later row can widen a column, so the
grid wasn't prefix-stable (CLAUDE.md invariant 2). Two problems fell out of that:

1. **They didn't stream.** While a long table generated, the strip preview showed
   `clone.flush()`'s *last* row — the `└──┴──┘` bottom border — floating above the
   box, and the whole grid popped into scrollback at the close. For a big table
   that's a long stare at a lonely border.
2. **Narrow tables truncated with `…`.** When the natural grid overflowed the
   width, columns shrank and cells were cut with a trailing `…` (`fit_cell_spans`
   / `truncate_segments` / `shrink_table_columns`). Content was lost.

This change makes tables **stream row-by-row into scrollback** (like prose) and
**word-wrap their cells into taller rows** instead of truncating — matching how
codex renders tables (wide → a tight grid; narrow → the same grid with cells
wrapped across lines, never `…`). See the before/after in the repo's issue images
(img1 = the old `…` grid; img2 = the wrapped grid).

**Very narrow → key/value records.** When even the wrapped grid is too cramped to
scan — columns starved so narrow that cells fragment into a tall sliver — the block
renders as codex's **key/value records** instead: each data row becomes a vertical
`label  value` list (one field per column), rows divided by a dim `─` rule. This is
codex's `table_key_value` transpose (`codex-rs/tui/src/markdown_render/table_key_value.rs`),
adapted to our streaming model — the grid-vs-records choice is made **once, at the
same first-data-row lock** as the column widths, so records stream row-by-row and
stay prefix-stable too. Never `…`, always readable at any width. See *The records
fallback* below.

## The idea: lock column widths at the first data row

A table can't be prefix-stable *and* size its columns to every row — the later
rows aren't known when the early ones commit. So we **lock the column widths once,
at the first data row**, from the header + that row (fit to the width). After the
lock the grid is a pure left-to-right render: every subsequent row **wraps into
the fixed widths** (never widening them), so its rows are prefix-stable and commit
to scrollback one at a time, exactly like prose lines.

The trade-off (accepted deliberately): in a **wide** terminal, if a *later* row
has an unusually wide cell it wraps into the header+row-1 width rather than forcing
the whole column wider. In practice the first data row is representative, and this
is the price of true streaming with immutable scrollback. The batch renderer uses
the **same** lock rule (it drives the same `AssistantRenderer`), so a table looks
identical whether it streamed, was repainted after a resize, or is shown in the
Ctrl+O transcript — no reflow surprises.

## The state machine (`ui::AssistantRenderer` / `TableState`)

```
None ──(a table-row candidate)──▶ PendingHeader(header)
PendingHeader ──(matching delimiter)──▶ AwaitingRow{header, aligns}      // emits nothing yet
             ──(not a delimiter)──────▶ render the header as prose, reprocess the line
AwaitingRow  ──(first data row)───────▶ lock widths, then DECIDE:
               • scannable grid  ──▶ emit ┌header┤ + row1; ▶ Streaming{col_w, aligns}
               • too cramped     ──▶ emit row1's record block; ▶ Records{labels, label_width}
             ──(non-table line)───────▶ header-only table: ┌header┤ + └──┘, reprocess the line
Streaming    ──(data row)─────────────▶ wrap the row into col_w, emit it (stays Streaming)
             ──(non-table line)───────▶ emit └──┘ (close), reprocess the line
Records      ──(data row)─────────────▶ emit a `─` rule + the row's record block (stays Records)
             ──(non-table line)───────▶ close (no border), reprocess the line
```

`flush()` (end-of-message / a code fence interrupting) closes an open table the
same way: `AwaitingRow` → header-only grid, `Streaming` → the bottom border,
`Records` → nothing (records have no bottom border).

Because rows are emitted **incrementally** and flow through the normal `stamp` →
`frozen` pipeline, `assistant_lines` (batch) and `StreamRender` (streaming) drive
the one `AssistantRenderer` core and agree by construction — the differential test
`stream_render_matches_batch_render_on_every_prefix` (extended with wrapping-table
*and* records entries) still holds.

### The records fallback (codex's key/value transpose)

`table_should_use_records(header, first_row, col_w)` is the grid-vs-records
decision, made at the lock from the header + first row (like the width lock, and
with the same "the first row is representative" trade-off). It returns true when a
column is **both** narrow (`< TABLE_SCANNABLE_COL`, 12) **and** its header/first-row
content wraps into `≥ TABLE_RECORDS_MIN_LINES` (3) rows at the locked width — i.e.
the grid is growing tall because columns are *starved*, not merely because one wide
cell is a legitimately long narrative (a wide column, `≥ 12`, never triggers it). A
single-column table is a list, never records.

`table_record_block(labels, row, label_width, content_width)` renders one row as a
vertical record: for each column a `label  value` field — the **bold** label padded
to `label_width`, the value inline-parsed and wrapped, continuation lines aligned
under the value (`wrap_inline`, so nothing is ever `…`'d). When even
`label_width + gap + TABLE_RECORD_MIN_VALUE` won't fit, the field **stacks** (label
on its own line, value indented beneath) — codex's aligned-vs-stacked split. Between
records a dim `─`×`content_width` rule (`table_record_separator`), emitted **before**
each non-first record (the first row emits at the lock), so the block streams
row-by-row and is prefix-stable. `Records` is a post-lock state like `Streaming`, so
`in_table()` is false there and a **partial trailing data row** is the only holdback
(the same `markdown::is_table_row` check `Streaming` uses).

### Holdback: only *before* the lock

`AssistantRenderer::in_table()` now reports **only** the pre-lock buffering states
(`PendingHeader` / `AwaitingRow`) — the phase that emits no rows. `StreamRender::commit`
withholds the trailing line while that holds (as before). Once `Streaming`, the
emitted rows commit through the normal prose path; the only thing still held back
is a **partial trailing data row** (`markdown::is_table_row(tail_src)`), because an
early wrapped row of a growing row could change once the rest of the row arrives.

## Column allocation + cell wrapping (`ui.rs`)

- `allocate_column_widths(natural, avail)` — fits the natural grid when it fits;
  otherwise distributes the content width across columns **proportionally to their
  natural width** (so a wide column stays wide, like img2's email column), floored
  at `TABLE_MIN_COL` (3). The shrunk widths are **wrap** widths, not truncation
  widths.
- `table_row_lines(cells, col_w, aligns)` — word-wraps each cell into its column
  (`wrap_inline`, which hard-breaks an over-wide token grapheme-by-grapheme, so a
  long email or a CJK run still fits by wrapping, never `…`). A data row spans as
  many rows as the tallest wrapped cell; each sub-row is `│`-framed, per-column
  padded/aligned, blank where a shorter cell has no line.
- `table_border_row` / `table_open_rows` — the `┌┬┐` / `├┼┤` / `└┴┘` borders and
  the opening (top border + wrapped bold header + separator). `lock_widths_for`
  computes the locked widths from the header (+ optional first row).
- `table_should_use_records` / `table_record_block` / `table_record_separator` —
  the records fallback (above): the decision, one row's `label value` block, and
  the inter-record `─` rule. `normalize_raw_cells` / `table_label_width` /
  `table_header_style` are the small shared helpers.

`truncate_segments` / `fit_cell_spans` / `shrink_table_columns` are gone (no more
`…`). `table_content_rows` survives only as a `#[cfg(test)]` convenience that
renders a complete **grid** table through the same helpers (it doesn't apply the
records decision — tests exercise records through the real `AssistantRenderer`).

## The preview never floats a bottom border (`StreamRender::preview`)

The strip preview is one row. While a table streams it must show the last **content**
row, never the not-yet-real `└──┘`. So `preview`:

- does **not** flush the clone (a `Streaming` table's rows already committed; its
  bottom border only becomes real at `finish`);
- does **not** feed an *empty* trailing line into an open table (a chunk boundary
  that ended right after a newline would otherwise close it and emit the border) —
  the last emitted row (in `frozen`) previews instead;
- renders the *forming* table for the pre-lock states via `flush_table_preview`
  (`PendingHeader` → the header as prose; `AwaitingRow` → the top border + header +
  separator), so it shows the table taking shape rather than falling back to an
  already-committed pre-table line (which would duplicate it).

`tail_rows` (which `commit`'s stable-boundary math uses) carries the **same**
empty-trailing-in-a-table guard, so `commit` and `preview` agree on where the
frontier is — otherwise `commit` would close the table and commit the last data row
that the strip is previewing (a duplicate). `finish` (end-of-turn) *does* feed the
trailing line and flush, so the real bottom border commits exactly once.

## Tests

- `narrow_table_wraps_cells_into_taller_rows_no_ellipsis` — narrow grid wraps, no
  `…`, cell content preserved across the wrapped rows.
- `very_narrow_table_renders_as_key_value_records` — a very narrow table flips to
  records: no box-drawing, every label present, content preserved (no `…`), a `─`
  rule between rows.
- `moderately_narrow_table_stays_a_wrapping_grid` — the fallback doesn't
  over-trigger: a moderately narrow table is still a grid.
- `table_should_use_records_only_when_narrow_and_cramped` — the pure decision
  (wide → grid, very narrow → records, single-column → never).
- `allocate_column_widths_fits_naturally_or_shrinks_proportionally` — the pure
  width math (fit vs. proportional shrink, floor).
- `table_cells_wrap_across_rows_instead_of_truncating` — a too-wide (CJK) cell
  wraps grapheme-by-grapheme, nothing lost.
- `table_streams_row_by_row_to_scrollback` — the top border + rows commit **before**
  the table closes (progressive), and the preview never shows a `└──┘` mid-stream.
- `stream_render_matches_batch_render_on_every_prefix` — extended with a
  wrapping-table entry; streamed commits + `finish` still reconstruct the batch
  render at every prefix/width (prefix-stability).
- `preview_never_shows_a_committed_row_while_streaming` — extended with a table;
  the progressive preview never duplicates a committed row.

## What's unchanged

A table wrapped in a code fence (```` ```table ```` ) is still **verbatim code**,
not a grid — code blocks take precedence (`CodeStart` flushes any open table).
Detection is still `markdown::is_table_row` / `table_delimiter`; a pipe-less or
column-mismatched "table" is still prose. Verified end-to-end against a real model
(`openai/gpt-4o-mini` via OpenRouter): a bare GFM table streams as a progressive
wrapped grid; a fenced one renders as code.
