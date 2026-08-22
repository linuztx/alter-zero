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
  `allocate_column_widths`, see "Fitting the columns" below), so the grid always
  fits its real content; cells word-wrap into the allocated widths (taller rows,
  never `…`). The grid-vs-records decision (below) is made from the same full
  knowledge.
- **Grid style**: Claude Code's full grid — every data row is framed, with a
  `├──┼──┤` rule between consecutive rows (not just under the header), so each
  cell reads as its own box. Two placement rules finish the look (both visible in
  the reported screenshot):
  - **Headers centre.** A header cell is centred in its column when the
    delimiter declared no alignment — `ui::header_align` maps
    `Alignment::None` → `Center` — so a centred label sits over left-aligned
    data and reads as a column heading rather than a first row. A *declared*
    `:--`/`:-:`/`--:` still wins for the header too: the default changes, stated
    intent doesn't.
  - **Short cells centre vertically.** `table_row_lines` seats each cell's rows
    at `(height - lines) / 2` inside the row, so beside a neighbour that wrapped
    to three rows a one-line cell lands on the **middle** row and the two read as
    one record. Rounding down means a one-line cell in a two-row row stays on
    top, matching how `pad_cell_line` rounds its horizontal centring.
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

## Fitting the columns (`allocate_column_widths`)

Given each column's `natural` width (its widest cell) and `word` width (its
widest single unbreakable word — `natural_word_widths`, tokenized exactly as
`wrap_inline` will break it), three cases in order:

1. **The natural grid fits.** Return it. The table stays as narrow as its
   content instead of stretching to the terminal — Claude Code's look.
2. **It overflows, but every column's word-safe floor fits.** Seat each column
   at `min(natural, word)` — the width it needs to wrap at *spaces* only — then
   split the surplus in proportion to each column's **unmet demand**
   (`natural - floor`), largest-remainder so the columns fill the budget
   exactly.
3. **Even the floors overflow.** Some token has to hard-break, so level the
   **widest** column down one display column at a time (ties leftmost-first,
   floored at `TABLE_MIN_COL` — `level_widest_columns`): a short cell
   (`facebook.com`, a `Packets` header) keeps its natural width for as long as
   possible. A proportional shrink here starves *every* column at once and
   breaks short words mid-cell (an earlier bug).

Case 2 is what makes a table read like Claude Code's, and it replaced
levelling-the-widest as the *general* fit. The reported screenshot is why:
`| Check | Result |` where the Check column's natural width is set by ONE
outlier (`cargo clippy --all-targets -- -D warnings`, 40 columns) while its
other cells are ≤ 17, and Result holds a ~104-column value. Levelling gave
[35, 36] — 18 columns of Check wasted on every other row while Result was
starved into four wrapped rows. Word-safe floors are [13, 18], so the surplus
splits 27:86 and the grid comes out [23, 48]: Result gets the room, matching
Claude Code's layout for the same table. It also fixes the ping-summary shape
more honestly than levelling did — a column holding one unbreakable 33-column
IPv6 has that as its floor, so the overflow now comes from a column that can
give way at a space instead of shattering the address mid-token.

## Wide glyphs: the emoji that cut the table

A ✅ is **two** terminal columns. Every width in the grid already measured it
that way (`cols()` → `unicode-width`), so the rendered rows were correct — and
yet an emoji row on screen came out one column wider *per emoji*, stepping its
right border out of line and, on a table that filled the width, wrapping the
border onto its own row. The reported "emoji cuts the table".

The cause was one layer down, at the write boundary. ratatui's `Buffer` stores a
wide glyph in a **single** cell and resets the cells it visually covers to a
blank **shadow** (`Buffer::set_stringn`); its own `Buffer::diff` then *skips*
those shadows — the backend prints the glyph once and the terminal advances the
remaining columns by itself. But `term.rs`'s hand-rolled full-cell paints hand
cells to `Backend::draw` **without** going through `diff`, so they printed the
shadow as a space: three columns spent on a two-column glyph.

`term::visible_cells` is the missing skip. It walks the buffer and drops each
wide cell's `cell_width - 1` followers, measuring with ratatui's own `CellWidth`
— the same measure `set_stringn` reserved them with, so the reservation and the
skip can never disagree — clamped to the row so a shadow never crosses a line
boundary. With the skip, the backend's position check issues an absolute
`MoveTo` across the gap, so the next glyph lands on its true column however wide
the terminal actually drew the cluster.

**All three** paints go through it, and that count is the whole point:

| paint | what it draws |
| --- | --- |
| `draw_lines` | `insert_before`'s scrollback commits, and `reflow` |
| `blit` | a full live-region repaint (the forming table's strip) |
| `draw_overlay` | the alt screen — Ctrl+O transcript, `/resume`, Ctrl+D |

Fixing only the first two leaves the tear alive in the Ctrl+O transcript, the
one view you open to read a table in full: measured at 80 columns, its emoji
rows came out 29 against the borders' 28. `smoke.sh` Phase 41 therefore checks
the inline pane **and** the overlay. (The incremental `prev.diff(buf)` path was
always correct, which is why the live region sometimes looked right and
sometimes didn't.)

One targeted exception, ported from `Buffer::diff`'s VS16 workaround: terminals
disagree on the width of an emoji **presentation sequence** (`…U+FE0F`), so on
one that draws it narrow a skipped shadow could keep stale screen content
visible beside the glyph. For a VS16-bearing wide cell the shadow cells are
emitted **first** — scrubbing what the glyph may not cover — then the glyph
itself, last, so it lands whole on the terminals that do render it wide. This is
insurance rather than a fix for anything reproducible here: measured via cursor
position, tmux 3.6b draws ⚠️ at two columns, agreeing with `unicode-width`.

This was never table-specific — every emoji and CJK character in prose was drawn
a column too wide too (the doubled space after `✅`). A table just makes it
visible, because a grid has a right edge to tear.

### The limit: clusters the terminal measures differently

The skip keeps our model and the terminal in step only where they agree on the
cluster's width. They mostly do — measured against tmux 3.6b, `✅ ❌ ⚠️ 🇵🇭 世
👍🏽 1️⃣` are all two columns, exactly what `cols()` reports. A **ZWJ sequence** is
the exception: `👨‍👩‍👧‍👦` is 2 columns to `unicode-width` and 4 to tmux, so a
table row holding one still drifts. Nothing in the app can fix that — the width
of a ZWJ sequence is not portable, and Claude Code has the same limit — which is
why `cols_measures_emoji_clusters_as_two_columns` pins the policy we *do*
control rather than pretending to solve it.

### Why this is prefix-stable by construction

`StreamRender::commit` withholds the trailing line while the renderer is inside
an open table (`AssistantRenderer::in_table`, every table state) or while the
trailing line is a header candidate (`markdown::is_table_header_candidate` — a
**leading pipe**). Since the buffering states emit **no rows**, `frozen` simply
doesn't grow while the table accumulates — there is no committed table row to
invalidate. The whole block lands in `frozen` at the close (or `finish`), after
which it commits like any settled rows. Batch (`assistant_lines`) and streaming
drive the one `AssistantRenderer`, so scrollback, the strip, the resize
repaint, and the Ctrl+O transcript agree by construction.

The leading pipe is what makes candidacy *decidable while streaming*: under
GFM's optional-leading-pipe (headerless) form, any growing prose line could
turn into a table header the moment a later `|` streamed in — with a delimiter
on the next line, the batch render would re-draw as a grid the prose rows the
committer had already frozen into scrollback (a fuzz-caught divergence). A
`|`-led header decides at the line's first character instead; the rare
headerless table renders as prose, identically in batch and stream. Data rows
inside a confirmed block keep the loose `is_table_row`, so the hard-wrapped
tail re-join below is untouched.

## The state machine (`ui::AssistantRenderer` / `TableState`)

```
None ──(a |-led header candidate)──▶ PendingHeader(header)
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

### Hard-wrapped rows re-join

A model echoing terminal-wrapped source can carry a line break **mid-row**, so
one row arrives as a leading-pipe line plus a fragment —
`| google.com | … | 86.8 / 89.8 /` ␤ `96.4 ms |` (a mid-cell wrap, the first
piece unterminated) or `| facebook.com | … | 0% |` ␤ `15.0 / 16.4 / 17.9 ms |`
(a wrap at the cell boundary, the first piece `|`-terminated but short). Strict
GFM reads every line as a row, so each fragment minted a phantom one-cell row
(the reported `│ 96.4 ms │ │ │ …` — GitHub renders this source just as broken).
But in a leading-pipe table a genuine row *starts* with `|`: `Buffering`
therefore **joins** a pipe-carrying line that doesn't start with `|` onto the
previous buffered row (`join_wrapped_table_row`, space-separated) — provided
the merged row still fits the delimiter's column count. A fragment that would
overflow `ncols` is a real (style-mixed) row — `c | d` after a complete
`| a | b |` stays its own row per GFM — and in a **no-leading-pipe** table no
line ever looks like a fragment (its rows legitimately lack the `|`), so
nothing there joins. Only confirmed data rows re-join: a hard-wrapped *header*
still fails delimiter confirmation and renders as the prose it may well be
(repairing `PendingHeader` would change how ordinary pipe-carrying prose
renders). The join happens in the buffer, before any render, so widths, the
records decision, batch, the strip preview, and the Ctrl+O transcript all see
the repaired row — and since `Buffering` emits nothing, prefix-stability is
untouched (the differential corpus includes a hard-wrapped table; a
mid-fragment prefix simply previews the fragment as prose until its pipe
arrives, matching the batch render of that prefix exactly).

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

`preview(text, width, max_rows)` returns the strip's preview **lines**: **every
uncommitted row** — `frozen` past `committed` (e.g. the withheld blank between
the pre-table prose and the table), then the trailing line fed to a clone. That
is exactly the batch render's tail, so `committed ++ preview ==
assistant_lines(prefix)` — the whole reply is visible at every moment, split
between scrollback and the strip.

A table needs no branch of its own any more, only its **flush**: when one is
open (the renderer's state entering the trailing line, or the clone's after
feeding it) the clone's `flush()` renders the block-so-far, closing border
included, and since none of it has committed it falls inside the uncommitted
range like everything else. An **empty** trailing line is *not* fed to the clone
(a chunk boundary right after a newline would close the block early); the flush
renders the buffered rows either way.

What generalised is the other direction: this used to be the table's *special*
case, with every other construct keeping a single-row preview chosen
independently of the commit frontier — which lost rows off the screen whenever a
withheld source line wrapped past one row, and duplicated one when a withheld
line rendered to none (see *Scrollback and the strip share one frontier* in
`docs/markdown.md`). Trailing blank rows are trimmed (they are a paragraph break
`commit` withholds too), except inside a fence where blank lines are content;
a reply that renders to zero rows with nothing committed still previews the
bullet home, matching `assistant_lines`.

The result is capped to its **last** `max_rows` rows, so a tail taller than the
screen — a big table, a very long withheld code line — tail-follows its frontier
(the newest rows stay visible; a table's top border scrolls out of the strip and
reappears when the block commits whole). When the frontier is clean the preview
is **empty** and the strip reserves no preview row at all.

### The close-flush must not strand the box (the blank-band bug)

The moment the block closes, two things happen in one frame: the whole grid
(often dozens of rows) lands in the pending scrollback queue, and the strip
collapses from the tall forming-table preview to a single row. The pending
flush (`term::write_above_chunk`) reserves the **tracked** viewport height
below the lines it writes — if that still counted the tall strip, the flush
landed the viewport a strip-height too high and the shrink blanked the vacated
rows, leaving a blank band between the box and the screen bottom (the reported
bug; the turn-end `set_view_height` reseat covers only the turn-end flush).
`term::paint_live` therefore syncs the tracked height to the frame's height
*before* `flush_pending` whenever lines are pending — the flush then reserves
exactly the collapsed region and the box stays flush at the bottom. Without
pending lines the height change still flows through `ui::repin`, whose shrink
is what blanks rows an in-place shrink vacates. Guarded by `smoke.sh` Phase 41
(the dummy's `"table"` demo reply streams a 10-row grid with prose after it).

### Geometry: the strip must reserve what the preview draws

`ui::preview_rows` sizes the strip's preview slot, and `live_height` /
`cursor_position` / `render_live_with_preview` all consume it — but only the
boundary's `StreamRender` knows the forming table's height. The boundary
computes the preview once per frame (`tui::view::Session::stream_preview_lines`, passing
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
- `assistant_table_joins_hard_wrapped_rows` — the phantom-row bug: the ping
  table with both wrap flavours re-joins its rows (three data rows, whole RTT
  cells, no `│ 96.4 ms` row); `wrapped_row_join_accepts_fragments_and_rejects_real_rows`
  pins the join/reject rules (leading-pipe fragments join, declared rows and
  `ncols` overflows don't, no-pipe style never joins).
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
- The fit: `allocate_column_widths_spends_the_surplus_where_the_content_is`
  (the screenshot's shape → [23, 48], not a levelled [35, 36]),
  `allocate_column_widths_wraps_words_before_shattering_a_token` (the ping
  table keeps its IPv6 whole), `allocate_column_widths_levels_the_widest_when_no_floor_fits`
  (the case-3 fallback), `allocate_column_widths_keeps_a_grid_that_fits_natural`,
  and end-to-end `assistant_table_gives_the_content_heavy_column_the_room`.
- Wide glyphs, the `ui` side: `emoji_table_rows_all_end_at_the_same_column`
  (every grid row of an emoji/CJK table is the same width at five widths),
  `table_rows_with_mixed_emoji_clusters_render_the_same_width` (one grid mixing
  VS16, ZWJ and skin-tone clusters), and
  `cols_measures_emoji_clusters_as_two_columns` — the dependency-regression
  guard pinning the two-column policy across seven cluster kinds, since all the
  column math rests on it.
- Wide glyphs, the `term` side: six tests pin `visible_cells` —
  `visible_cells_skips_the_cells_shadowed_by_a_wide_glyph`,
  `..._skips_the_shadow_of_every_wide_cluster_kind` (CJK, a ZWJ family, a
  halfwidth-katakana dakuten pair), `visible_cells_emits_every_cell_of_a_narrow_row`,
  `back_to_back_wide_glyphs_each_skip_one_shadow`,
  `visible_cells_scrubs_then_redraws_a_vs16_shadow` (the emission *order*), and
  `visible_cells_shadow_stays_in_its_row_and_the_origin_offsets` (the row clamp
  + `blit`'s area offset).
- The boundary half is `smoke.sh` Phase 41: the dummy's demo table carries ✅/❌
  status cells, and the phase asserts every grid row is the same display width
  in the inline pane **and** in the Ctrl+O overlay — the second check is what
  catches a fix that covered `draw_lines`/`blit` but not `draw_overlay`. Against
  the unfixed emitters the inline check reports `[1 65 69 71 72 78]` (the torn
  border wrapped onto its own row) and the overlay check `[65 69 71 72 78]`.
- Placement: `table_header_cells_center_by_default`,
  `table_header_keeps_an_explicitly_declared_alignment` (a `:---` column stays
  left), `table_row_centers_a_short_cell_against_a_wrapped_one` (the one-liner
  lands on the middle of three rows), and end-to-end
  `assistant_table_centers_headers_and_short_cells_like_claude_code` — the
  screenshot's table, asserting `✅ pass (exit 0)` shares a row with
  `--all-targets` and `cargo test` with the middle line of its Result cell.
- The other pure helpers keep their tests (`table_cells_wrap_*`, records
  deciders/renderers, `table_content_rows_*`).

## What's unchanged

A table wrapped in a code fence is still verbatim code (a `CodeStart` flushes
any open table first). Row/delimiter parsing is still `markdown::is_table_row`
/ `table_delimiter` (a block *opens* only on a `|`-led header,
`is_table_header_candidate` — see the prefix-stability section); a pipe-less or
column-mismatched "table" is still prose.
Cells are still inline-parsed and columns sized to *rendered* widths
(`docs/markdown.md`). Verified end-to-end against a real model via OpenRouter:
the issue's weather table streams as a live forming grid and commits with
columns fit to every row; a fenced one renders as code. The emoji table from the
report was re-driven the same way — pre-fix its emoji rows measured 29 terminal
columns against the borders' 28, post-fix every row measures 28, and a
20-row ✅/❌ table previews in the strip with its grid intact while forming.
