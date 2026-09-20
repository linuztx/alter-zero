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

> A collapsed cell's output block **folds** at **`TOOL_FOLD_ROWS`** display
> rows — Claude Code's three — over the `… +N lines` hint, an output of
> exactly one more row shown whole; the hint counts the **rows** the
> expansion will add; a command cell spends none of those rows on a
> **blank** line; and a line that is a **JSON document** is reshaped for
> display before any of that. Only the numbered file cell keeps a
> per-line budget (**`TOOL_LINE_MAX_ROWS`**, its cut marked).

The rule arrived in layers, and the layers are the history of getting it
wrong: §1–§3 bounded the *line*; §0 (added after the mess came back in a
second dress) bounded the *block*; §4 decided what the block is *of*; §5 and
§6 are the port of Claude Code's own fold, which is what the block's budget
finally became. They are kept in that order because each explains the next.

### 0. Rows, not lines (the block has a row budget)

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

The block was given a row budget: so many display rows, whatever the output
looks like — first four, beside a four-source-line budget, whichever ran out
first; then, once Claude Code's own fold was measured (§5), **three over the
hint**. The ceiling of the settled block is `TOOL_PEEK_ROWS` (4) either way,
hint included, and that is also the window the **running** tail shows
(`running_command_lines` shows the last four *rows*), so head and tail stay
budgeted alike: a `bash` cell does not *grow* when it settles. (It can settle
shorter — §4 ends the block at the first blank line — but the tail is still
the ceiling.)

### 1. A per-line row budget (`TOOL_LINE_MAX_ROWS = 3`) — the file cells' now

The first cure for the blob was a *per-line* cap: no one source line may spend
more than `TOOL_LINE_MAX_ROWS` rows of a collapsed cell, so a blob shows its
head and stops and the line after it still gets a row. Inside the exec cell's
block budget that cap became redundant once the block folded at three rows
(§5): the fold bounds the blob and everything else alike, and Claude Code's
own cell shows the first three rows of a blob with no row left for the line
after it, so the exec cells dropped it. It **stays for the numbered file
cells** (`ui/file_cell.rs`): their peek is `FILE_PEEK_LINES` (10) *lines* with
no row ceiling of its own, so a minified `.json` line in a `read`/`write` peek
still needs bounding to `TOOL_LINE_MAX_ROWS` rows.

### 2. The cut is visible (`TOOL_LINE_ELLIPSIS`) — on a numbered row

Where a line *is* clipped (the file cells), the last kept row ends in `…`,
fitted so the row never overflows the width (the `ellipsize` rule every
one-row truncation in the inline views already follows). Without it a clipped
line reads as a line that simply ended. The exec fold has no marker: the
`… +N lines` hint sits right under the third row and already says the rest
follows — that is Claude Code's look, and a marker on top of it would replace
the row's last character to say the same thing twice.

### 3. `+N lines` counts what pressing Ctrl+O adds

The hint used to count **source lines** not fully shown, which is how 1.8 KB of
hidden JSON became `… +1 lines`. It now counts **display rows** not shown — the
rows the expansion actually paints — for the cells whose output has no line
numbers (`bash`, the diff fallback, an ask cell's answers; the `!` shell cell
shows its whole output inline and has no hint to count,
`docs/shell-command.md`). The two
numbers are *the same* for everyday output (short lines are one row each); they
diverge exactly where the old count lied.

The count is exact rather than estimated: `WrapMode` (`ui/wrap.rs`) pairs each
wrapper with its own row **counter** and **clip**, all three driven by one
range-emitting scan, so a hint can never count rows a different wrapper would
have produced. The scan's one rule beyond greedy breaking: a break that lands
on an **exactly-full** row consumes the whitespace run after it. Carrying the
run over used to seat the space at the head of the next row (`total` /
` 12`, one column out of line) or — before another full-width word — leave it
as a row of its own, a phantom blank row inside `hello` / ` ` / `world` that
the `+N lines` hint then counted; a space that *fits* still stays at the end
of the row it broke on, so column-aligned output that fits is untouched. The Ctrl+O view wraps command output with the same `wrap_output`
and diff bodies with the same `wrap_verbatim`, so `+N` is what the expansion
adds, row for row. Counting is allocation-free, which is what lets the running
tail's footer use it every animation frame without wrapping the retained buffer
into `String`s (`docs/tool-streaming.md`).

**Numbered file cells keep counting lines.** A `read`/`write`/`edit` cell
(`ui/file_cell.rs`) paints a line-number gutter, and expanding it shows those
same numbered file lines — so `… +20 lines` there means twenty more *file*
lines, the unit the user is reading off the gutter. The rule is one rule in two
dresses: **count in the unit the expansion shows**. Those cells are where
parts 1 and 2 live now — a minified `.json` line is clipped to
`TOOL_LINE_MAX_ROWS` with a dim `…`, where before it rendered in full (the
first body row was exempt from the `FILE_PEEK_LINES` budget entirely, so one
long line could paint a hundred rows inline).

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
  `BlankPolicy::Keep`, so their rendering is byte-identical. The one that
  passes `FirstBlock` is the backend `bash` cell. The `!` shell command,
  one cell shape by design, **does not fold at all** any more — its whole
  output shows inline, blank lines and all, the `…` cap marker closing a
  cut one (`docs/shell-command.md`) — so it takes no policy: a peek window
  is a fold's concern, and the shell cell has none.

The rule is applied to **source** lines, before wrapping, so a wrapping line
inside the block still shows whole up to the fold and the blank after it still
ends the cell. It yields to §5's one exception: an output of four rows or
fewer is shown whole, blank rows and all — there is nothing to fold, and a
hint would cost more than it hides.

The running *tail* is deliberately untouched: it shows the last rows as they
stream, where a blank line is part of what the command just printed and
dropping it would make the tail disagree with the terminal.

### 5. Claude Code's fold (`TOOL_FOLD_ROWS = 3`, and the one exception)

The reference was measured rather than eyeballed. Claude Code's tool result
(`OutputLine` → `renderTruncatedContent`, in its bundle) wraps the output at
the terminal width, keeps the first **three** rows, and hangs
`… +N lines (ctrl+o to expand)` under them with `N` the rows past the third —
with one exception: when exactly **one** row is past the fold it shows that
row instead, since a hint hiding one row costs the very row it hides. Ours
folds the same way now, and the exception is kept: a four-row output shows
whole, a five-row one shows three and `+2 lines`. Two of its details are
deliberately *not* ported. It hard-breaks the folded rows at the column (its
own transcript view word-wraps them — the hard break is its fold helper's
simplicity, not a look); ours word-wraps, spaces preserved, as everywhere
else, and fills a row before a token wider than any row the way its header
wrap does (`docs/tools.md` *Long headers*), which is what makes the two agree
on token-heavy output. And its hidden count is an estimate past a character
budget (`ceil(len / width) - 3`); ours is exact, since `WrapMode::rows`
counts without building.

Trailing blank lines are dropped before the fold (Claude Code's `trimEnd`):
`echo; echo` tails and a formatter's closing newlines are nothing to show and
nothing to hide, so they count in neither the cell nor the hint — and an
output that is blank lines only reads `(no output)`, in the cell and in
Ctrl+O alike.

### 6. A JSON line is reshaped for display (`TOOL_JSON_PRETTY_MAX_BYTES`)

The report's other half. A `curl` of the Wikipedia API printed one compact
line — `{"batchcomplete":"","query":{"pages":{"75936044":{"pageid":…` — and
where ours folded three rows of braces, Claude Code showed

```
  ⎿  {
       "batchcomplete": "",
       "query": {
     … +16 lines (ctrl+o to expand)
```

because its result renderer **pretty-prints a line that is a JSON document**
before folding (two-space indentation, `JSON.stringify(v, null, 2)`). Ours
does the same, in `ui::exec_display_lines`: every line of the display body
that parses as a JSON object or array **and round-trips** — re-serializes to
the same text once whitespace is ignored, Claude Code's own guard — is
replaced by its indented form, for the settled inline cell and the Ctrl+O
expansion alike (so the hint still counts exactly what the expansion adds).
The guard is what keeps the display honest: duplicate keys (the last wins in
the parse), a number the serializer would spell differently, an escape it
would resolve, all stay verbatim, because the reshaped text would then be a
document the command never printed. An output longer than
`TOOL_JSON_PRETTY_MAX_BYTES` (10 000 — Claude Code's cap) is left alone whole,
which bounds the `Value` tree a render parses (`docs/memory.md`'s rule is
about trees kept to read a few fields of a body; this one is transient and
small by construction). The running tail shows the output as printed —
what is still streaming is partial, and a line reshaping under the cursor
would make the tail disagree with the terminal. The record, the model-facing
result, the rollout and the context replay are untouched: this is a render
seam, like the header's path display.

## Where it does *not* apply

- **Ctrl+O** (`tool_full_lines`, `file_cell_lines(peek: false)`) — the
  expansion is the place the whole line lives. Clipping it would leave the
  text nowhere. (It does share §5's trailing-blank trim and §6's JSON
  reshaping, so what the hint counts is what it paints.)
- **The permission prompt's preview** (`ui::permission_view`, which calls
  `numbered_body_lines` with no per-line cap) — you cannot approve what you
  cannot see. A prompt taller than the terminal already flows into real
  scrollback (`docs/view-flow.md`).
- **Assistant text** — a wrapped URL or a long prose line is the answer, not
  incidental output.

## The result

The report's command, at the 72 columns it was captured in — Claude Code's
cell and ours agree row for row on the header (`docs/tools.md`) and on the
fold:

```
● Bash(curl -s --max-time 15 "https://en.wikipedia.org/w/api.php?action=
      query&prop=extracts&exintro&explaintext&format=json&titles=World%2
      0Chess%20Championship%202026"…)
  ⎿  {
       "batchcomplete": "",
       "query": {
     … +9 lines (ctrl+o to expand)
```

(The unit test `an_exec_cell_pretty_prints_a_json_line_like_claude_code`
pins those rows.) A blob that is *not* JSON folds the same way, the cut
implied by the hint under it rather than marked on the row:

```
● Bash(curl -s "https://www.youtube.com/oembed?url=https://youtu.be/Vr-NEWVL
      N9Y&format=json")
  ⎿  {"title":"How to Use Burp MCP with Claude to Find Bugs from HTTP
     History","author_name":"\ud835\ude47\ud835\ude64\ud835\ude68\ud835\ude69\ud
     835\ude68\ud835\ude5a\ud835\ude58","author_url":"https://www.youtube.com/@
     … +11 lines (ctrl+o to expand)
```

Three rows instead of twelve, and a number that means what it says: the
expansion holds exactly fourteen rows, three of them shown. (An oEmbed body
whose escapes the serializer would resolve fails the round-trip guard, so it
stays as printed.)

And the multi-line report the block budget answers — the same page, the same
grep, three rows instead of ten:

```
● Bash(grep -i "champion" -A 5 /tmp/wiki.html | head -n 20)
  ⎿  <title>World Chess Championship - Wikipedia</title>
     <script>(function(){var className="client-js
     vector-feature-language-in-header-enabled
     … +114 lines (ctrl+o to expand)
  ⎿  Allowed by auto mode classifier
```

## Code map

| Piece | Where |
| --- | --- |
| `TOOL_FOLD_ROWS`, `TOOL_PEEK_ROWS`, `TOOL_JSON_PRETTY_MAX_BYTES`, `TOOL_LINE_MAX_ROWS`, `TOOL_LINE_ELLIPSIS` | `ui/theme.rs` |
| `WrapMode::{wrap, rows}` — one scan, two uses; `fills_from_here` | `ui/wrap.rs` |
| The display lines (`exec_display_lines`, `prettify_json_lines`, `pretty_json_line`) | `ui/tool.rs` |
| The collapsed output fold (`result_peek_block`) | `ui/tool.rs` |
| The first-block window (`BlankPolicy`, `is_blank_row`) | `ui/tool.rs` |
| The running tail's `+N lines ({secs}s · timeout …)` clock row | `ui/tool.rs` (`running_command_lines`) |
| The numbered file cell's per-line clip | `ui/file_cell.rs` (`numbered_row_lines`) |

Tests: `ui::tests::wrap` (the counter primitives and the fill rule) and
`ui::tests::tool` (every cell shape —
`a_finished_peek_never_spends_more_rows_than_the_row_ceiling` is the block
budget's own, `an_exec_cell_pretty_prints_a_json_line_like_claude_code` and
`a_four_row_output_shows_whole_instead_of_hiding_one_row` the port's). On
screen, the fold is Phase 38's settled `Bash(ping …)` cells (`… +3 lines
(ctrl+o to expand)` under the dummy backend's real commands); Phase 92 drives
the same three shapes — a 750-character single line, four wrapping lines, a
blank-shaped output — through `!` commands, which since
`docs/shell-command.md` retired the shell cell's fold assert the opposite
contract: every row inline, no hint, and Ctrl+O agreeing row for row.
