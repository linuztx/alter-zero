# Streaming GFM tables (buffered whole, previewed live)

GFM pipe tables (`docs/markdown.md`) went through two designs before this one:

1. **Buffered whole, invisible while forming.** The block committed to scrollback
   only at the close (a later row can widen a column, so a grid isn't
   prefix-stable — CLAUDE.md invariant 2). Correct, but the strip preview was one
   row, so a long table was a blank stare at its last row until the whole grid
   popped in at the close.
2. **Column widths locked at the first data row, streamed row-by-row.** Each row
   committed as it arrived, wrapping into widths locked from the header + first
   data row. That made tables *stream*, but the lock is a guess about rows that
   haven't arrived: whenever a later row is wider than the first, its cells
   wrap into slivers. The reported issue is the canonical failure — a key/value
   weather table whose header is **empty** (`| | |`) and whose first row is the
   narrow `| **Time** | 03:00 (CEST, GMT+2) |`: the widths locked at [4, 19]
   and every later row shattered (`Temp`/`erat`/`ure`, `10.1 km/h from NNE`/
   `(28°)`). No early lock can be right in general, and because batch and
   streaming share one renderer, the resize/Ctrl+O repaint showed the same
   broken grid.

This design keeps the correctness of (1) and the streaming feel of (2), the way
Claude Code does it: **the table renders only when its widths are final, but the
user watches it form in the live region the whole time.**

## The idea: buffer the block, preview the whole forming table

- **Rendering**: a table's source lines are buffered (`TableState::Buffering`)
  and the block renders **whole** when it closes — a non-table line, an
  interrupting code fence, or end-of-message (`flush`). Column widths come from
  the header **and every data row** (`table_column_widths` →
  `allocate_column_widths`), so the grid always fits its real content: wide
  terminal → a tight natural grid; narrow → the overflow is taken from the
  **widest column first** (ties leveled leftmost-first, floored at
  `TABLE_MIN_COL`), so a short cell (`facebook.com`, a `Packets` header) keeps
  its natural width and the wrapping lands on the genuinely wide content (a
  33-column IPv6, a long description) — codex's fit; cells word-wrap into the
  allocated widths (taller rows, never `…`). The old proportional shrink
  starved *every* column when one was huge, breaking short words mid-cell.
  The grid-vs-records decision (below) is made from the same full knowledge.
- **Grid style**: Claude Code's full grid — every data row is framed, with a
  `├──┼──┤` rule between consecutive rows (not just under the header), so each
  cell reads as its own box.
- **Streaming**: while the block is open, nothing of it commits to scrollback
  (scrollback is immutable; a committed row can't be re-widened). Instead the
  streaming **strip previews the entire forming table** — `StreamRender::preview`
  returns the batch render's *uncommitted tail*, which during a table is the
  whole block rendered from the rows seen so far (a partial trailing `| cell…`
  row included). The strip is a live region redrawn every frame, so the grid
  visibly grows row-by-row and its columns re-fit as wider cells arrive —
  exactly the Claude Code experience. On the close, the finished block (identical
  to the last preview) commits in one shot and the strip empties: visually the
  table "freezes" in place.

The preview generalizes from "the last rendered row" (one row) to "the batch
render's uncommitted tail" (n rows) only while a table is open; prose, code, and
every other construct keep the old single-row preview. A running tool's cell
already previews multi-row, so the strip machinery was ready for this.

### Why this is prefix-stable by construction

`StreamRender::commit` withholds the trailing line while the renderer is inside
an open table (`AssistantRenderer::in_table`, every table state) or while the
trailing line is a table-row candidate (`markdown::is_table_row`). Since the
buffering states emit **no rows**, `frozen` simply doesn't grow while the table
accumulates — there is no committed table row to invalidate. The whole block
lands in `frozen` at the close (or `finish`), after which it commits like any
settled rows. Batch (`assistant_lines`) and streaming drive the one
`AssistantRenderer`, so scrollback, the strip, the resize repaint, and the
Ctrl+O transcript agree by construction.

## The state machine (`ui::AssistantRenderer` / `TableState`)

```
None ──(a table-row candidate)──▶ PendingHeader(header)
PendingHeader ──(matching delimiter)──▶ Buffering{header, aligns, rows: []}
             ──(not a delimiter)──────▶ render the header as prose, reprocess the line
Buffering    ──(data row)─────────────▶ rows.push(line)            // emits nothing
             ──(non-table line)───────▶ emit table_block_rows(header, aligns, rows),
                                        reprocess the line
```

`flush()` (end-of-message / a code fence interrupting) closes an open table the
same way: `PendingHeader` → the prose line it actually was, `Buffering` → the
whole block (a header-only table renders as the opening + bottom border).

`table_block_rows(header, aligns, rows, width)` is THE table renderer — both the
batch path and the close/flush/preview paths emit a table only through it:
widths from all rows, then either the box-drawing grid (`table_open_rows` +
`table_row_lines` per row, a `├──┼──┤` rule between consecutive rows, + the
bottom border) or the key/value records.

## The records fallback (key/value transpose)

When even the wrapped grid is too cramped to scan — a column **both** narrow
(`< TABLE_SCANNABLE_COL`, 12) **and** holding a cell that wraps into
`≥ TABLE_RECORDS_MIN_LINES` (3) rows at its allocated width — the block renders
as vertical **key/value records** instead (Claude Code's compact `label: value`
form, codex's `table_key_value.rs`): each data row becomes one record — per
column a **bold** label, `: `, the value wrapped with continuation lines aligned
under it (stacking label-over-value when even that won't fit) — records divided
by a dim `─` rule capped at `TABLE_RECORD_SEPARATOR_WIDTH` (40). The decision
(`table_should_use_records`) now checks **every** buffered row, not just the
first — full knowledge, same as the widths. A single-column table is a list,
never records. Nothing is ever `…`'d.

## The multi-row preview (`StreamRender::preview`)

`preview(text, width, max_rows)` returns the strip's preview **lines**:

- Outside a table: the old single row — the last non-blank rendered row (the one
  `commit` withholds via `stable_keeping_preview_row`, so a completed line is
  never in scrollback and the strip at once), with the bullet-home fallback for
  a zero-row reply.
- While a table is open (the renderer's state entering the trailing line, or the
  clone's state after feeding it): **every uncommitted row** — `frozen` past
  `committed` (e.g. the withheld blank between the pre-table prose and the
  table), the trailing line fed to a clone, then the clone's `flush()` rendering
  the block-so-far, closing border included. That is exactly the batch render's
  tail, so `committed ++ preview == assistant_lines(prefix)` — the whole reply
  is visible at every moment, split between scrollback and the strip.
  An **empty** trailing line is *not* fed to the clone (a chunk boundary right
  after a newline would close the block early); the flush renders the buffered
  rows either way.
- The result is capped to its **last** `max_rows` rows, so a table taller than
  the screen tail-follows its frontier (the newest rows stay visible; the top
  border scrolls out of the strip and reappears when the block commits whole).

### Geometry: the strip must reserve what the preview draws

`ui::preview_rows` sizes the strip's preview slot, and `live_height` /
`cursor_position` / `render_live_with_preview` all consume it — but only the
boundary's `StreamRender` knows the forming table's height. The boundary
computes the preview once per frame (`main.rs::stream_preview_lines`, passing
`ui::stream_preview_max_rows(screen.height)` as the cap — the screen minus the
strip/box/footer chrome, floored) and injects its row count into the pure state
via `App::set_stream_preview_rows` — the `set_status_times` /`set_clock`
boundary-injection pattern. `preview_rows` reports that count while a reply
streams (1 when nothing multi-row was injected, so unit tests and the
render fallback keep the old single-row behaviour), and the strip's
`debug_assert` still pins drawn-lines == reserved-rows.

## Trade-offs (accepted deliberately)

- A table's rows reach **scrollback** only at the close. They are never
  invisible — the strip shows every row the moment it arrives, correctly laid
  out — but terminal scrollback (and anything reading it, e.g. tmux copy mode)
  sees the block appear at once. That is the price of immutable scrollback +
  correct widths, and it is how Claude Code behaves.
- While forming, the grid's columns may visibly re-fit as wider rows arrive
  (the strip re-renders per frame). That is the point: the *committed* grid is
  final and correct.
- A table taller than `stream_preview_max_rows` previews only its newest rows
  while forming (tail-follow). The committed block is always complete.

## Tests

- `assistant_table_sizes_columns_from_all_rows` — the reported bug: the weather
  table (empty header, narrow first row, wider later rows) renders with
  `Temperature` / `10.1 km/h from NNE (28°)` on single lines at width 80.
- `stream_render_withholds_a_table_until_it_closes` — an open table commits
  nothing; `finish` flushes the whole block, matching batch.
- `table_commits_whole_and_previews_while_forming` — mid-stream: no table row
  in scrollback, the preview shows the forming grid (borders + rows so far),
  and `committed ++ preview` equals the batch render of every prefix; at the
  close the whole grid commits.
- `table_preview_caps_to_its_newest_rows` — the `max_rows` cap keeps the tail.
- `stream_render_matches_batch_render_on_every_prefix` — the differential
  guardrail, strengthened: the preview is now asserted to be a **suffix** of the
  batch render at every prefix/width (and, while a table is open,
  `committed + preview` to be the *whole* batch render).
- `preview_never_shows_a_committed_row_while_streaming` — unchanged property,
  now over every preview row.
- The pure helpers keep their tests (`allocate_column_widths_*`,
  `table_cells_wrap_*`, records deciders/renderers, `table_content_rows_*`).

## What's unchanged

A table wrapped in a code fence is still verbatim code (a `CodeStart` flushes
any open table first). Detection is still `markdown::is_table_row` /
`table_delimiter`; a pipe-less or column-mismatched "table" is still prose.
Cells are still inline-parsed and columns sized to *rendered* widths
(`docs/markdown.md`). Verified end-to-end against a real model via OpenRouter:
the issue's weather table streams as a live forming grid and commits with
columns fit to every row; a fenced one renders as code.
