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
> display before any of that. The numbered file cell budgets rows too, and
> its ten are a **ceiling**: a line its peek starts shows whole when the rows
> left hold it, and the one that needs more is cut on the last of them,
> closed by a `…` (§7, §8).

The rule arrived in layers, and the layers are the history of getting it
wrong: §1–§3 bounded the *line*; §0 (added after the mess came back in a
second dress) bounded the *block*; §4 decided what the block is *of*; §5 and
§6 are the port of Claude Code's own fold, which is what the block's budget
finally became; §7 took the line budget out of the file cells, the last place
it was kept; §8 made their block budget a ceiling again, after one line §7
let through painted a screenful. They are kept in that order because each
explains the next.

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

### 1. A per-line row budget (`TOOL_LINE_MAX_ROWS = 3`) — retired

The first cure for the blob was a *per-line* cap: no one source line could
spend more than three rows of a collapsed cell, so a blob showed its head and
stopped and the line after it still got a row. Inside the exec cell's block
budget that cap became redundant once the block folded at three rows (§5):
the fold bounds the blob and everything else alike, and Claude Code's own
cell shows the first three rows of a blob with no row left for the line after
it, so the exec cells dropped it. It stayed for the numbered file cells
(`ui/file_cell.rs`) until §7 took it out there too.

### 2. The cut was visible (`TOOL_LINE_ELLIPSIS`) — retired with it

Where a line *was* clipped (the file cells), the last kept row ended in `…`,
fitted so the row never overflowed the width; without it a clipped line read
as a line that simply ended. The marker went with the cap (§7), and came
back for the one cut the file peek's ceiling makes (§8). The exec fold
never had one: the `… +N lines` hint sits right under the third row and
already says the rest follows — that is Claude Code's look, and a marker on
top of it would replace the row's last character to say the same thing twice.

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
dresses: **count in the unit the expansion shows**. The one line those cells
cut at their ceiling (§8) is marked on its own row instead, so a hidden line
is still always a whole line the hint counts.

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

### 7. The file cells show every line whole

The per-line cap (§1) was the wrong trade for the one cell kind that kept it.
A `read`/`write`/`edit` cell is where the user reads what the agent read or
wrote, and the lines that ran past three rows there were rarely blobs: a
Markdown paragraph (one line per paragraph), a long string, an HTML line, a
docstring. Each stopped at its third row on a `…`, so the end of the sentence
or the closing tag was nowhere on screen, and the report was plain: show the
full line instead of hiding it. So a numbered row is never cut now. Inline,
in Ctrl+O and in the permission prompt's preview alike, a line shows every
row it wraps to, the diff tint padding each of them.

What still bounds the cell is the peek's row budget, applied to whole lines:

> The peek **starts** a line while it has rows left of `FILE_PEEK_LINES`
> (10), and shows every line it starts **whole**.

So a short-lined file shows its first ten lines as before; a long line that
begins inside the ten rows is finished rather than cut, carrying the cell
past ten rows by the rest of itself; and once the rows are spent no further
line is started, however short. A `README.md` written at 72 columns, whose
paragraph on line 3 used to stop on its third row at `tool call an…`:

```
● Write(README.md)
  ⎿  Wrote 11 lines to README.md
       1 # alter-zero
       2
       3 An agent harness for coding, cybersecurity and automation that
         runs in your terminal, streams each reply and tool call inline
         as it happens, and leaves every finished message, tool call and
          file change in the terminal's own scrollback, where you can sc
         roll back and copy it like any other output.
       4
       5 ## Install
       6
     … +5 lines (ctrl+o to expand)
```

(The content rows hard-break at the column, as code always has in these
cells and in Ctrl+O.) The hint counts the file lines left, which are always
whole lines. In an edit, the `+` row of a replaced line is shown whenever the
context and the `-` row before it leave a row of the budget. A rule that only
took lines while they *fit* would have dropped it as soon as the two halves
together passed ten rows, showing what was removed but not what replaced it.

The cost was the case §1 was written for: a pathological line the peek
started was shown whole, a minified bundle painting every row of itself
inline. It was judged bounded — by the `read` tool's own cap
(`TOOL_OUTPUT_MAX_BYTES`, 64 KB, under a thousand rows at 80 columns), and
by a committed cell landing in scrollback rather than the live region. It
was not: a `write` has no such cap, and scrollback is where the user reads.
§8 is the report that said so, and the ceiling it put back.

A `write` streaming into its cell (`docs/write-streaming.md`) shows the
newest `FILE_PEEK_LINES` **rows** of the file as it stands — rows, not lines,
the way a terminal's tail does. A long line still arriving shows the text
that just arrived on its bottom row (under the cut it showed the line's first
three rows over a `…`, and none of what was streaming in), and the window's
top row may be the end of a line whose start has scrolled off. The window
never grows past ten rows however long the line, so the live strip keeps its
height; once the content has arrived the cell flips to the head, which is the
committed peek row for row.

The window is built several times a frame, and wrapping a whole line to keep
its last rows costs the whole line: a 1 MB minified line took 38 ms a build,
where the cut it replaced had stopped after three rows. So the tail of a line
is found without wrapping its head (`code_content_tail_rows`). When every byte
is printable ASCII, the overwhelming case for a line that long, each
character is one grapheme one column wide, row `k` starts at byte
`k × width`, and only the rows from there are wrapped: 0.7 ms a build for the
same line. Any other text is wrapped whole and its head dropped. The rows are
the same either way, which a differential test holds the arithmetic to.

### 8. The file peek's ten rows are a ceiling

The cost §7 accepted came back as a report: a `Write` of a stylesheet whose
fourth line was the whole sheet minified, thousands of columns on one line.

```
● Write(src/styles.css)
  ⎿  Wrote 10 lines to src/styles.css
       1 @import url('https://fonts.googleapis.com/css2?family=DM+Sans:wght@400;450;500
         ;550;600;650;700&family=Manrope:wght@400;500;600;650;700;750;800&display=swap'
         );
       2
       3 :root{font-family:'DM Sans',sans-serif;color:#272932;background:#fafbfc;font-s
         ynthesis:none;font-weight:400;--purple:#6c5ce7;--purple-dark:#5948d6;--muted:#
         90919c;--line:#e8e9ee;--surface:#fff;--green:#2c9a73}
       4 *{box-sizing:border-box}body{margin:0;min-width:360px}button,a{-webkit-tap-hig
         hlight-color:transparent}button{font:inherit;cursor:pointer}button:disabled{cu
         rsor:not-allowed}button:focus-visible,a:focus-visible{outline:3px solid rgba(1
         08,92,231,.28);outline-offset:4px}button{color:inherit}button svg{flex-shrink:
         [… well over a hundred more rows of line 4 …]
     … +6 lines (ctrl+o to expand)
```

Line 4 began on the peek's eighth row, so the peek started it, and §7's
rule showed every row of it: a cell meant as a ten-row glimpse painted a
screenful of minified CSS into scrollback, the hint under it still counting
six lines. The bound §7 leaned on held for a `read`, whose output the tool
caps, and for nothing else: what a model *writes* has no cap, and the rows
of a committed cell stay in the user's scrollback for good.

So the ten rows are a ceiling now. Of §7's rule, only what happens to the
line that runs into it changes:

> The peek **starts** a line while it has rows left of `FILE_PEEK_LINES`
> (10), shows it **whole** when the rows left hold it, and otherwise shows it
> for the rows left and **cuts** it on the last of them, closing that row
> with a dim `…` (`FILE_PEEK_CUT_MARKER`).

```
● Write(src/styles.css)
  ⎿  Wrote 10 lines to src/styles.css
       1 @import url('https://fonts.googleapis.com/css2?family=DM+Sans:wght@400;450;500
         ;550;600;650;700&family=Manrope:wght@400;500;600;650;700;750;800&display=swap'
         );
       2
       3 :root{font-family:'DM Sans',sans-serif;color:#272932;background:#fafbfc;font-s
         ynthesis:none;font-weight:400;--purple:#6c5ce7;--purple-dark:#5948d6;--muted:#
         90919c;--line:#e8e9ee;--surface:#fff;--green:#2c9a73}
       4 *{box-sizing:border-box}body{margin:0;min-width:360px}button,a{-webkit-tap-hig
         hlight-color:transparent}button{font:inherit;cursor:pointer}button:disabled{cu
         rsor:not-allowed}button:focus-visible,a:focus-visible{outline:3px solid rgba(…
     … +6 lines (ctrl+o to expand)
```

What §7 was for survives it:

- A short-lined file shows its first ten lines, as it always has, and a long
  line the rows left can hold is shown whole: a paragraph is never cut at a
  per-line count again, as it was at three rows (§1). §7's `README.md`
  renders exactly as drawn there.
- At most one line is cut, and only by the ceiling, never by its own length:
  it is the last line the peek shows.
- An edit still shows the `+` row of a replaced line whenever the context and
  the `-` row before it leave a row — the start rule is §7's own — cut on the
  rows left instead of carried past the ceiling. A `-` row that reaches the
  ceiling itself leaves its `+` row unstarted, as it did under §7.

The marker is §2's, back for this one cut: without it a line stopped at the
ceiling reads as a line that ended there. It takes the row's last column,
dim, on the row's own tint, so an added line's green band runs unbroken to
the cut; the row's content gives up the columns it needs from its end.

**The hint still counts whole lines.** `+6 lines` is lines 5–10, the file
lines the peek never started, so the gutter's last number plus the hint is
the head's count, as in every file cell; the rest of the cut line is what
the `…` on its row stands for. A cut line with no line after it — a minified
one-line file, read back or written — hides no whole line, and `+0 lines`
would say nothing is behind Ctrl+O. The hint row there is `… (ctrl+o to
expand)`: no count, and still the way to the rest.

**The cut line's tail is never wrapped.** `code_content_head_rows` builds the
rows the peek keeps and stops at the first grapheme that would open a row
past them: the greedy break only ever looks back, so the head of the whole
wrap is the wrap of the head, which a differential test holds it to. That
matters most for a `write` streaming into its cell, whose head the strip
rebuilds several times a frame while the call waits and runs. Under §7 that
head wrapped a 1 MB line whole every build and kept every row — 13,889 of
them at 80 columns — the cost §7 had already taken off the tail with
arithmetic (38 ms a build). Ten rows of the same line now take about a
quarter of a millisecond a build, less than the tail's arithmetic, and
`the_head_of_a_huge_finished_line_stays_inside_the_frame_budget` holds it
inside a frame beside the tail's gate.

Ctrl+O and the permission prompt's preview are untouched: the expansion
still shows every line whole, and you still see what you approve.

## Where it does *not* apply

- **Ctrl+O** (`tool_full_lines`, `file_cell_lines(peek: false)`) — the
  expansion is the place the whole line lives. Clipping it would leave the
  text nowhere. (It does share §5's trailing-blank trim and §6's JSON
  reshaping, so what the hint counts is what it paints.)
- **The permission prompt's preview** (`ui::permission_view`, over
  `numbered_body_lines`) — you cannot approve what you cannot see. A prompt
  taller than the terminal already flows into real scrollback
  (`docs/view-flow.md`).
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
| `TOOL_FOLD_ROWS`, `TOOL_PEEK_ROWS`, `TOOL_JSON_PRETTY_MAX_BYTES`, `FILE_PEEK_LINES` | `ui/theme.rs` |
| `WrapMode::{wrap, rows}` — one scan, two uses; `fills_from_here` | `ui/wrap.rs` |
| The display lines (`exec_display_lines`, `prettify_json_lines`, `pretty_json_line`) | `ui/tool.rs` |
| The collapsed output fold (`result_peek_block`) | `ui/tool.rs` |
| The first-block window (`BlankPolicy`, `is_blank_row`) | `ui/tool.rs` |
| The running tail's `+N lines ({secs}s · timeout …)` clock row | `ui/tool.rs` (`running_command_lines`) |
| The numbered file cell's whole-line peek (§7) and its ceiling (§8) | `ui/file_cell.rs` (`file_cell_lines`, the draft's head, `numbered_row_lines`' `LineRows`, `peek_hint_line`), `ui/assistant.rs` (`code_content_head_rows`) |
| The streaming `write`'s newest-rows tail (§7) | `ui/file_cell.rs` (`draft_cell_lines`), `ui/assistant.rs` (`code_content_tail_rows`) |

Tests: `ui::tests::wrap` (the counter primitives and the fill rule) and
`ui::tests::tool` (every cell shape —
`a_finished_peek_never_spends_more_rows_than_the_row_ceiling` is the block
budget's own, `an_exec_cell_pretty_prints_a_json_line_like_claude_code` and
`a_four_row_output_shows_whole_instead_of_hiding_one_row` the port's,
`a_read_cell_shows_a_very_long_file_line_whole`,
`the_peek_cuts_the_line_that_runs_past_its_tenth_row`,
`the_reported_minified_write_stays_inside_its_ten_rows`,
`a_cut_last_line_still_points_at_ctrl_o` and
`an_edit_of_a_long_line_shows_both_versions_inside_the_peek` the file
cells'), with the streaming cell's in `ui::tests::file_cell`
(`a_long_line_still_arriving_shows_its_newest_rows`,
`a_finished_write_with_a_long_line_shows_the_head_the_committed_cell_will`,
and the frame-budget gates
`the_tail_of_a_huge_streaming_line_stays_inside_the_frame_budget` and
`the_head_of_a_huge_finished_line_stays_inside_the_frame_budget`) and the
differential tests in `ui::tests::assistant`
(`a_code_lines_tail_rows_are_the_last_rows_of_the_whole_wrap`,
`a_code_lines_head_rows_are_the_first_rows_of_the_whole_wrap`). On
screen, the fold is Phase 38's settled `Bash(ping …)` cells (`… +3 lines
(ctrl+o to expand)` under the dummy backend's real commands); Phase 92 drives
the same three shapes — a 750-character single line, four wrapping lines, a
blank-shaped output — through `!` commands, which since
`docs/shell-command.md` retired the shell cell's fold assert the opposite
contract: every row inline, no hint, and Ctrl+O agreeing row for row. §7 is
Phase 138b's: the real backend, against a stub provider, streams a `write` of
a 467-character paragraph line and reads the file back, and neither the
streaming cell nor the committed `Write` and `Read` cells may cut a file row
with a `…`. §8 is Phase 138c's: the same stub streams the reported
stylesheet and reads it back, and no sample of the streaming cell may pass
ten body rows, the committed `Write` cell and the `Read` cell after it must
each show exactly ten — the tenth ending in `…` — over `… +6 lines (ctrl+o
to expand)` with the minified line's last word nowhere inline, and Ctrl+O
must show that word.
