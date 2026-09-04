# Very long output lines — bounded rows, honest counts

A collapsed tool cell shows the head of its output and hides the rest behind a
`… +N lines (ctrl+o to expand)` hint. What "the head" *means* is the whole
subject of this document, and it has been wrong twice — both times because the
cell measured itself in **source lines** while the user reads it in **display
rows**.

The first version budgeted in source lines alone (`TOOL_PEEK_LINES` of them),
each rendered fully wrapped so a long line's tail never disappeared past the
terminal edge (`docs/tool-streaming.md`). That is right until one line is
*pathological* — a minified bundle, a base64 blob, a 2 KB JSON body from
`curl`. Then a single source line ate the whole cell:

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

Twelve rows of unreadable wrapped JSON — the whole block ceiling spent on
**one** line — closed by a hint claiming *one* line is hidden. Both halves are
wrong, and they are wrong in the same way: the cell measures itself in source
lines while the user reads it in **rows**.

## The rule

> A collapsed cell's output block spends at most **`TOOL_PEEK_ROWS`** display
> rows, of which any one source line may spend at most
> **`TOOL_LINE_MAX_ROWS`**; the cut is **visible**; an unnumbered cell's
> `+N lines` hint counts the **rows** the expansion will add; and a command
> cell spends none of those rows on a **blank** line.

Five parts. The first three landed together and bounded the *line*; the fourth
(§0 below, added after the mess came back in a second dress) bounds the
*block*; the fifth (§4) decides what the block is *of*.

### 0. Rows, not lines (`TOOL_PEEK_ROWS = 4`)

Bounding the line was not bounding the cell. Four source lines, each *within*
its 3-row budget, still cost twelve rows — and that is the everyday shape of a
`curl` of any web page:

```
● Bash(curl -s "https://en.wikipedia.org/wiki/World_Chess_Champion" |
      grep -i "champion" -A 5 | head -n 20)
  ⎿  <title>World Chess Championship - Wikipedia</title>
     <script>(function(){var className="client-js
     vector-feature-language-in-header-enabled
     vector-feature-language-in-main-menu-disabled…
     RLSTATE={"ext.globalCssJs.user.styles":"ready","site.styles":"ready
     ","user.styles":"ready","ext.globalCssJs.user":"ready","user":"read
     y","user.options":"loading","ext.cite.parsoid.styles":"ready","ext…
     <script>(RLQ=window.RLQ||[]).push(function(){mw.loader.impl(functio
     n(){return["user.options@12s5i",function($,jQuery,require,module){m
     w.user.tokens.set({"patrolToken":"+\\","watchToken":"+\\","csrfTok…
     … +122 lines (ctrl+o to expand)
  ⎿  Allowed by auto mode classifier
```

Ten rows of markup for a cell whose whole job is to say *the command ran, here
is a glimpse*. Nothing in it is a bug at the line level: every line is clipped
at 3 rows and marked. The bug is that the **block** had no row budget — its
ceiling was the *product* of the two line budgets (`TOOL_PEEK_LINES *
TOOL_LINE_MAX_ROWS` = 12), so the same four lines cost four rows when they were
short and twelve when they wrapped. A cell whose height depends on how the
output happens to be shaped is a cell you cannot skim.

`TOOL_PEEK_ROWS` is that budget, and it is now **the** budget: four display
rows, whatever the output looks like. The source-line budget still stands
beside it (`TOOL_PEEK_LINES`, the same 4) — whichever runs out first ends the
peek, so ordinary output still reads line for line and a wrapping one stops at
four rows.

It is also the window the **running** tail already used
(`running_command_lines` shows the last four *rows*), so head and tail are
budgeted the same: a `bash` cell does not *grow* when it settles. (It can
settle shorter — §4 ends the block at the first blank line — but the tail is
still the ceiling.)

The per-line budget keeps its job inside the smaller block: it is what
guarantees the blob does not take all four rows, leaving one for the line
*after* it — the peek shows that the output continues rather than spending
itself on one line.

### 1. A per-line row budget (`TOOL_LINE_MAX_ROWS = 3`)

No one source line may spend more than `TOOL_LINE_MAX_ROWS` rows of the block.
The everyday multi-row case is untouched: a `sudo: a terminal is required…`
error wrapping to 2–3 rows at a narrow width still shows in full. What changes
is the pathological line: it shows its head and stops, and the line after it
still gets a row — which is the whole point of a *per-line* cap inside the
block budget above. Without it, one blob would take all four rows and the peek
would never show that the output continues.

`TOOL_PEEK_ROWS` is the block's ceiling, enforced in the same loop, so the
per-line budget is really `min(TOOL_LINE_MAX_ROWS, rows left in the block)` —
a line arriving with one row left shows one row and its `…` — and a caller
passing a bigger line budget (the numbered file cells, `FILE_PEEK_LINES`)
still can't run past it.

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

### 4. The peek is the output's first block, not its first four lines

Everything above budgets the peek. This part decides *what it is a peek of* —
and the honest answer had been "whatever the first four lines happen to be,
including the empty ones".

A blank line costs a full row of a four-row cell and says nothing. Two shapes
are ordinary enough to hit constantly:

```
● Bash(./build.sh)                              ● Bash(git status -sb)
  ⎿                                               ⎿  ## main...origin/main
     Building…                                       M src/ui/tool.rs
     Linking…
     Done                                            Untracked files:
     … +16 lines (ctrl+o to expand)                  … +16 lines (ctrl+o to expand)
```

The left cell spends its first row on a leading `\n` — a quarter of the whole
budget on nothing — and shows three lines where four would fit. The right one
paints a gap in the middle and then a fragment of the *next* section, which
reads as one run of lines that isn't one: `M src/ui/tool.rs` and `Untracked
files:` are not adjacent in the output, and the cell says they are.

So the block is the output's **first block**:

> Leading blank lines are skipped; the first blank line after them closes the
> peek.

```
● Bash(./build.sh)                              ● Bash(git status -sb)
  ⎿  Building…                                    ⎿  ## main...origin/main
     Linking…                                         M src/ui/tool.rs
     Done                                             … +18 lines (ctrl+o to expand)
     … +17 lines (ctrl+o to expand)
```

Four content rows where the output has four, two where the first block has two
— never a row of nothing, and never two stretches of output presented as one.

Three details make it hold up:

- **The count stays exact.** A skipped leading blank and the blank that closed
  the block are rows Ctrl+O will paint, so they are counted like any other
  hidden row (§3). The hint answers "how much more is there", not "how much
  more that I judged interesting" — the version of this rule that quietly
  dropped blanks from the count would have re-opened the `+1 lines` lie in a
  politer dress.
- **An all-blank output is left exactly as it was.** With no non-blank line
  anywhere there is no first block to prefer, and an empty window would leave
  the `… +N lines` hint hanging with no `⎿` corner above it. The policy
  no-ops (`BlankPolicy::window` returns the whole slice).
- **Only command output gets it.** A diff body's spacing is content, and an
  ask cell's `· Q → A` rows have no blanks to skip — those cells pass
  `BlankPolicy::Keep`, so their rendering is byte-identical. The two that pass
  `FirstBlock` are the two exec cells: the backend `bash` tool and the `!`
  shell command, which are one cell shape by design
  (`docs/shell-command.md`).

The rule is applied to **source** lines, before wrapping, so a wrapping line
inside the block still shows whole up to its own budget (§1) and the blank
after it still ends the cell.

The running *tail* is deliberately untouched: it shows the last rows as they
stream, where a blank line is part of what the command just printed and
dropping it would make the tail disagree with the terminal.

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

And the multi-line report the block budget answers — the same page, the same
grep, four rows instead of ten:

```
● Bash(grep -i "champion" -A 5 /tmp/wiki.html | head -n 20)
  ⎿  <title>World Chess Championship - Wikipedia</title>
     <script>(function(){var className="client-js
     vector-feature-language-in-header-enabled
     vector-feature-language-in-main-menu-disabled…
     … +113 lines (ctrl+o to expand)
  ⎿  Allowed by auto mode classifier
```

(Captured from a live `openai-gpt-4o-mini` turn in auto permission mode,
against a local copy of the page from the report.)

## Code map

| Piece | Where |
| --- | --- |
| `TOOL_PEEK_ROWS`, `TOOL_LINE_MAX_ROWS`, `TOOL_LINE_ELLIPSIS` | `ui/theme.rs` |
| `WrapMode::{wrap, rows, clip}` — one scan, three uses | `ui/wrap.rs` |
| The collapsed output peek (`result_peek_block`) | `ui/tool.rs` |
| The first-block window (`BlankPolicy`, `is_blank_row`) | `ui/tool.rs` |
| The running tail's `+N lines ({secs}s)` footer | `ui/tool.rs` (`running_command_lines`) |
| The numbered file cell's per-line clip | `ui/file_cell.rs` (`numbered_row_lines`) |

Tests: `ui::tests::wrap` (the counter/clip primitives) and `ui::tests::tool`
(every cell shape — `a_finished_peek_never_spends_more_rows_than_the_row_ceiling`
is the block budget's own), plus `scripts/smoke.sh` Phase 92, which drives two
real commands through the binary: a 750-character single line (the clipped cell
and the whole line in Ctrl+O) and four wrapping lines (the 4-row ceiling, the
honest count, and every row still in Ctrl+O).
