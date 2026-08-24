# Very long output lines — bounded rows, honest counts

A collapsed tool cell budgets its peek in **source lines** (`TOOL_PEEK_LINES` of
them) and renders each one fully wrapped, so a long line's tail never
disappears past the terminal edge (`docs/tool-streaming.md`). That is right
until one line is *pathological* — a minified bundle, a base64 blob, a 2 KB
JSON body from `curl`. Then a single source line eats the whole cell:

```
● Bash(which yt-dlp youtube-dl 2>/dev/null; curl -s
      "https://www.youtube.com/oembed?url=https://youtu.be/Vr-NEWVLN9Y&f
      ormat=json" 2>/dev/null)
  ⎿  /home/linuztx/.local/bin/yt-dlp
     {"title":"How to Use Burp MCP with Claude to Find Bugs from HTTP
     History","author_name":"\ud835\ude47\ud835\ude64\ud835\ude68\ud835\
     ude69\ud835\ude68\ud835\ude5a\ud835\ude58","author_url":"https://ww
     w.youtube.com/@lostsec_ftw","type":"video","height":113,"width":200
     ,"version":"1.0","provider_name":"YouTube","provider_url":"https://
     www.youtube.com/","thumbnail_height":360,"thumbnail_width":480,"thu
     mbnail_url":"https://i.ytimg.com/vi/Vr-NEWVLN9Y/hqdefault.jpg","htm
     l":"\u003ciframe width=\u0022200\u0022 height=\u0022113\u0022
     src=\u0022https://www.youtube.com/embed/Vr-NEWVLN9Y?feature=oembed\
     u0022 frameborder=\u00220\u0022 allow=\u0022accelerometer;
     autoplay; clipboard-write; encrypted-media; gyroscope;
     … +1 lines (ctrl+o to expand)
```

Twelve rows of unreadable wrapped JSON — the whole `TOOL_PEEK_MAX_ROWS`
ceiling spent on **one** line — closed by a hint claiming *one* line is
hidden. Both halves are wrong, and they are wrong in the same way: the cell
measures itself in source lines while the user reads it in **rows**.

## The rule

> A source line spends at most **`TOOL_LINE_MAX_ROWS`** display rows in a
> collapsed cell, the cut is **visible**, and an unnumbered cell's `+N lines`
> hint counts the **rows** the expansion will add.

Three parts, each fixing one half of the mess above.

### 1. A per-line row budget (`TOOL_LINE_MAX_ROWS = 3`)

The peek's budget stays `TOOL_PEEK_LINES` **source lines** — "the first 4 lines
of output", so a long first line never pushes its siblings out of the peek —
but each of those lines may now spend at most `TOOL_LINE_MAX_ROWS` rows of the
block. The everyday multi-row case is untouched: a `sudo: a terminal is
required…` error wrapping to 2–3 rows at a narrow width still shows in full
(the ceiling was already "three rows per budgeted line"; it is now enforced
*per line* instead of only for the block). What changes is the pathological
line: it shows its head and stops, and its siblings keep their rows.

`TOOL_PEEK_MAX_ROWS` stays the block's ceiling and is now literally the product
of the two budgets (`TOOL_PEEK_LINES * TOOL_LINE_MAX_ROWS`), enforced in the
same loop so a caller passing a bigger line budget (the numbered file cells,
`FILE_PEEK_LINES`) still can't run past it.

### 2. The cut is visible (`TOOL_LINE_ELLIPSIS`)

The last kept row of a clipped line ends in `…`, fitted so the row never
overflows the width (the `ellipsize` rule every one-row truncation in the
inline views already follows). Without it a clipped line reads as a line that
simply ended — the reader has no way to tell a complete `ls` row from the head
of a 2 KB blob.

### 3. `+N lines` counts what pressing Ctrl+O adds

The hint used to count **source lines** not fully shown, which is how 1.8 KB of
hidden JSON became `… +1 lines`. It now counts **display rows** not shown — the
rows the expansion actually paints — for the cells whose output has no line
numbers (`bash`, `!` shell, the diff fallback, an ask cell's answers). The two
numbers are *the same* for everyday output (short lines are one row each); they
diverge exactly where the old count lied.

The count is exact rather than estimated: `WrapMode` (`ui/wrap.rs`) pairs each
wrapper with its own row **counter** and **clip**, all three driven by one
range-emitting scan, so a hint can never count rows a different wrapper would
have produced. The Ctrl+O view wraps command output with the same `wrap_output`
and diff bodies with the same `wrap_verbatim`, so `+N` is what the expansion
adds, row for row. Counting is allocation-free, which is what lets the running
tail's footer use it every animation frame without wrapping the retained buffer
into `String`s (`docs/tool-streaming.md`).

**Numbered file cells keep counting lines.** A `read`/`write`/`edit` cell
(`ui/file_cell.rs`) paints a line-number gutter, and expanding it shows those
same numbered file lines — so `… +20 lines` there means twenty more *file*
lines, the unit the user is reading off the gutter. The rule is one rule in two
dresses: **count in the unit the expansion shows**. Those cells do get parts 1
and 2 — a minified `.json` line is clipped to `TOOL_LINE_MAX_ROWS` with a dim
`…`, where before it rendered in full (the first body row was exempt from the
`FILE_PEEK_LINES` budget entirely, so one long line could paint a hundred rows
inline).

## Where it does *not* apply

- **Ctrl+O** (`tool_full_lines`, `file_cell_lines(peek: false)`) — the
  expansion is the place the whole line lives. Clipping it would leave the
  text nowhere.
- **The permission prompt's preview** (`ui::permission_view`, which calls
  `numbered_body_lines` with no per-line cap) — you cannot approve what you
  cannot see. A prompt taller than the terminal already flows into real
  scrollback (`docs/view-flow.md`).
- **Assistant text** — a wrapped URL or a long prose line is the answer, not
  incidental output.

## The result

```
● Bash(curl -s
      "https://www.youtube.com/oembed?url=https://youtu.be/Vr-NEWVLN9Y&format=js
      on")
  ⎿  {"title":"How to Use Burp MCP with Claude to Find Bugs from HTTP
     History","author_name":"\ud835\ude47\ud835\ude64\ud835\ude68\ud835\ude69\ud
     835\ude68\ud835\ude5a\ud835\ude58","author_url":"https://www.youtube.com/@…
     … +11 lines (ctrl+o to expand)
```

Three rows instead of twelve, the cut marked, and a number that means what it
says: the expansion holds exactly fourteen rows, three of them shown. (Captured
from a live `openai/gpt-4o-mini` turn against the command from the report.)

## Code map

| Piece | Where |
| --- | --- |
| `TOOL_LINE_MAX_ROWS`, `TOOL_LINE_ELLIPSIS`, `TOOL_PEEK_MAX_ROWS` | `ui/theme.rs` |
| `WrapMode::{wrap, rows, clip}` — one scan, three uses | `ui/wrap.rs` |
| The collapsed output peek (`result_peek_block`) | `ui/tool.rs` |
| The running tail's `+N lines ({secs}s)` footer | `ui/tool.rs` (`running_command_lines`) |
| The numbered file cell's per-line clip | `ui/file_cell.rs` (`numbered_row_lines`) |

Tests: `ui::tests::wrap` (the counter/clip primitives) and `ui::tests::tool`
(every cell shape), plus `scripts/smoke.sh` Phase 92, which drives a real
750-character single-line command through the binary and checks both the
clipped cell and the whole line in Ctrl+O.
