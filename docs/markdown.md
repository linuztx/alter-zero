# Assistant markdown rendering

The model streams **markdown**, but assistant replies were word-wrapped as plain
prose by `ui::wrap_text`, which *collapses whitespace runs* (it is greedy
word-wrap for paragraphs). That destroyed the one construct where whitespace is
load-bearing: **fenced code blocks**. A Python snippet rendered flush-left with
its indentation gone and its lines re-flowed together — unreadable.

This is a prefix-stable markdown layer for assistant text. It now covers the full
common set — matching the clean codex/Claude-Code look:

- **Fenced + indented code blocks** — verbatim (indentation byte-for-byte),
  syntax-highlighted, fences hidden (see *Code* / *Headings* below).
- **ATX headings** — the `#` markers are **kept** and the line is styled per
  level, like codex (see *Headings*).
- **Inline** `**bold**`, `*italic*`, `~~strike~~`, `` `code` ``,
  `[text](url)`, `![alt](url)` (see *Inline markdown*).
- **Lists** (bullets, ordered, task lists) and **blockquotes**, with nesting
  indent preserved and hanging-indent wrapping (see *Lists and blockquotes*).
- **GFM pipe tables** — box-drawing grids (see *Tables*).
- **Thematic breaks** (`---` / `***` / `___`) as a `———` rule.

The hard constraint throughout is **prefix-stability** (CLAUDE.md invariant 2):
`StreamRender` flushes finished rows to the terminal's real scrollback as the
reply streams, so a construct whose rendering depends on *later* text must be
**held back** until it settles. Two constructs need this — **tables** (a later
row can widen a column) and **inline emphasis** (a closing marker can restyle an
already-wrapped row) — and both reuse the same withholding machinery the code
blocks introduced (`AssistantRenderer::in_code`). See *Prefix-stable holdback*.

## What it looks like

````text
● ## The Code                                  ← `## The Code` → `#`s kept, line bold
  Save this as snake.py:                        ← prose (word-wrapped, white)
  def main(stdscr):                             ← code, verbatim, syntax-highlighted
      curses.curs_set(0)                        ← 4-space indent preserved
      while True:
          sh, sw = stdscr.getmaxyx()            ← 8-space indent preserved
````

## Headings (codex parity)

Codex's terminal markdown renderer (`codex-rs/tui/src/markdown_render.rs`,
`start_heading`) does **not** strip the ATX `#` markers — it re-emits
`"#".repeat(level) + " "` as a styled prefix and styles the heading text with
that same style, so a heading reads as e.g. **`## The Code`** with the hashes
still visible. The per-level styles are **text modifiers only, no foreground
colour**:

| Level | Codex style       |
|-------|-------------------|
| `#`   | bold + underlined |
| `##`  | bold              |
| `###` | bold + italic     |
| `####`+ | italic          |

`ui::heading_style(level)` is a direct port of that mapping, and
`AssistantRenderer::content_rows` reconstructs the `# ` marker run (normalised to
one space, like codex) in front of the heading text before word-wrapping. This
deliberately **reverses** the earlier behaviour (drop the `#`s, bold in
`AI_COLOR`) so we match codex exactly — a user who reads codex output sees the
raw markers, so we show them too. It is still a pure per-line transform, so
prefix-stability and the batch/stream agreement are unchanged.

Code sits under the assistant bullet (aligned with the wrapped-prose indent, no
gutter) and is **syntax-highlighted** (One Dark palette — keywords magenta,
strings green, comments dim, numbers orange, calls blue). Both the opening and
closing ` ``` ` fences are hidden, and the info-string language is used only to
pick the highlighter — it is **not** shown as a label.

**Tabs are expanded to spaces** on the render path (`expand_code_tabs`, a fixed
`CODE_TAB_WIDTH`-space substitution, like codex's `expand_tabs`). A tab is zero
display columns (unicode-width treats it as a control char), so tab-indented
code — Go, Makefiles — would otherwise collapse flush-left, losing all its
indentation. The substitution is display-only: the stored message text keeps its
tabs, so `/copy` is byte-exact. It is a pure per-line transform applied in the
one `AssistantRenderer` core, so batch and streaming stay identical and
prefix-stable (a tab-indented block is in the differential corpus).

Highlighting is hand-rolled (`highlight.rs`) — codex uses `syntect` (~250
TextMate grammars); this codebase takes no such dependency, so a **generic**
tokenizer classifies the token shapes common to mainstream languages (comments,
strings, numbers, control/declaration keywords, and function calls). It is
precise for the languages people paste (Python, JS/TS, Rust, Go, C/C++, Java,
Ruby, shell) and degrades to plain text for anything it doesn't recognise. The
tokenizer is colour-agnostic; `ui` maps each `highlight::Kind` to a colour, so
all styling stays centralized in `ui.rs`.

## Architecture

- **`src/markdown.rs`** — a pure, unit-tested module (like `file_search` /
  `session`). `parse_blocks(text) -> Vec<Block>` walks the text once into
  `Block::Prose(String)` runs and `Block::Code { lang, lines }` blocks;
  `fence_lang` extracts the info-string language (first token); `heading_level`
  classifies a single prose line as an ATX heading. `BlockScanner` is the
  **incremental** counterpart: `classify(line) -> LineKind` runs the same fence
  state machine one line at a time (for `StreamRender`). No terminal, no styling.
- **`src/highlight.rs`** — the pure, unit-tested syntax tokenizer.
  `highlight(lines, lang) -> Vec<Vec<Seg>>` classifies each code line's runs into
  `Kind`s (keyword / string / comment / number / function / plain), threading
  multi-line string/comment state left-to-right. `Highlighter` is the
  **incremental** counterpart (`line(text) -> Vec<Seg>`, threading the carry across
  calls); `highlight` is a thin batch wrapper over it. Colour-agnostic — `ui` owns
  the palette.
- **`ui::AssistantRenderer`** — the per-line render core: `feed_line(line) ->
  Vec<Line>` classifies via `BlockScanner`, highlights via `Highlighter`, wraps,
  and stamps the bullet (first row) / indent. Both **`ui::message_lines`** (the
  batch funnel every finished-message path goes through — streamed-then-committed
  scrollback, resize repaint, Ctrl+O transcript) and **`ui::StreamRender`** (the
  incremental streaming committer + preview) drive this one core, so they render
  *identically* by construction. `message_lines` routes `Role::Assistant` through
  `assistant_lines` (= drive `AssistantRenderer` over `text.split('\n')`); every
  **other** role keeps the old plain path unchanged — a user pasting ` ``` ` is
  never code-blocked, and the dark-bg padding math is untouched.

Because `AssistantRenderer` is the one core, scrollback, the streaming preview, the
resize repaint, and the Ctrl+O transcript all agree automatically (invariants 2–4).

Styling is centralized as `CODE_*` / `HEADING_COLOR` consts at the top of
`ui.rs`. Code hard-breaks at `content_width` rather than letting the terminal
wrap it — a code line longer than the terminal would otherwise be wrapped by the
emulator at the wrong column and lose its alignment under the bullet.

## Incremental rendering while streaming (`ui::StreamRender`)

`message_lines` renders a **finished** message from scratch — fine for the resize
repaint and the Ctrl+O transcript (one message, once). But a *streaming* reply was
also rendered whole on **every** path that runs per chunk and per frame:
`stable_commit` re-rendered the entire accumulated reply to find the newly-stable
lines, and the strip preview re-rendered it just to grab the last line. Each is
O(reply); over a stream arriving in ~N chunks that is **O(reply²)**, and it all
runs on the single-threaded event loop. On a long code reply this measured at
~30–40 ms *per animation frame* at 1200–1600 lines — past the 32 ms status-frame
budget — so the spinner visibly stuttered and input lagged. (See
`examples/perf_probe.rs` for the measurement, and the git history of this change.)

`ui::StreamRender` fixes this by rendering **incrementally**. It drives one
`AssistantRenderer` — the per-line core extracted from `assistant_lines`, so batch
and streaming render *identically* — feeding it source lines through
`markdown::BlockScanner` (the incremental fence state machine, the counterpart of
`parse_blocks`) and `highlight::Highlighter` (the incremental per-line highlighter,
the counterpart of `highlight`). It **caches** the rendered rows of every complete
source line and, on each `commit`/`preview`, advances over only the lines that
arrived since the last call. So a `commit` is O(new text) and a `preview` is O(one
line): streaming a whole reply is **O(reply)**, and redrawing the preview every
frame is nearly free. (The one residual case is a single source line with no
interior newline — a long paragraph or a minified one-liner: its still-growing
*trailing* line is re-wrapped each call, so it is O(line) per commit/frame. That is
bounded by one line, and no worse than the old whole-reply render for the same
input; real replies carry newlines, so it never bites in practice.)

The boundary (`main.rs`) holds one `StreamRender` for the turn (replacing the old
`committed: usize`): `commit` on each `Chunk`, `finish` on `StreamDone`/tool
split/interrupt, `preview` before each draw (its line handed to
`ui::render_live_with_preview`), and `reset` at every turn boundary (turn end,
tool split, interrupt, `/clear`) **and before a `Purge` repaint** (resize — the
purge dropped every committed row, so the follow-up commit re-emits the whole
reply at the new width). A mid-stream **overlay return** (`ReflowClear::InPlace`)
instead *keeps* the render's state: the repaint tail carries the rows already
committed (`StreamRender::committed_rows` via `ui::repaint_tail`) and only the
overlay-time delta is committed after — repainting from history alone blanked
the partial until its next chunk (the Ctrl+O disappear-then-flicker bug), and
the old reset-then-recommit duplicated the already-scrolled rows in the
terminal's kept scrollback (`smoke.sh` Phase 35).

## Why this is safe while streaming (prefix-stability)

`StreamRender` flushes every completed reply line to the terminal's real
scrollback as the reply streams, keeping only the **last** rendered line back
(CLAUDE.md invariant 2). That is only sound if the render is *prefix-stable*:
appending to the reply may change only the last produced line; every earlier line
is frozen forever. Because `StreamRender` caches a completed line's rows and never
revisits them, prefix-stability holds *by construction* — but only if each line's
rows genuinely depend on nothing after it:

Code blocks (including their **syntax colours**) and headings preserve this
because their rendering is a pure left-to-right function of the text *before*
each line:

- A line's prose/code **mode** is fixed by the fence state entering it — a scan
  of the lines before it, no lookahead. Appending never reclassifies an *earlier*
  line. An **unterminated** fence renders as code *identically* to a closed one,
  so nothing changes when the closing ` ``` ` finally arrives (it just emits zero
  rows). The one wrinkle is the *fence-opener line itself*: while it is still a
  **partial** marker run (`` ` `` or `` `` ``) it renders as prose, then flips to
  **zero rows** when the third marker arrives (the fence is hidden) — a within-line
  reclassification. At a normal width that 1–2-column prose run is a single
  (withheld) last row, but at content-width 1 it wraps into two, so
  `StreamRender::commit` also withholds a trailing line while
  `markdown::is_partial_fence` holds — the invariant then holds at *every* width
  (the differential test covers width 3 upward).
- `wrap_verbatim`'s width hard-break is greedy grapheme-by-grapheme, so appending
  extends or starts only the *last* row — exactly like `wrap_text`.
- A heading's style trigger (`#…` at the line start) is seen before any of that
  line's output rows are committed.
- Syntax colours are per-line: a line's tokens are a pure function of its own
  text plus the multi-line `Carry` (open triple-string / block-comment) entering
  it — itself a left-to-right scan of the prior lines. An unterminated triple
  string or `/* … */` colours the rest as string/comment, identically to a closed
  one, so a committed code line never recolours.
- **But highlighting uses one-char lookahead *within* a line** (an identifier
  becomes a call at the `(`, `/` a comment at the next `/`). A code line longer
  than the width wraps into several rows, and committing an early row before the
  lookahead char streams in would recolour it. So `StreamRender::commit` withholds
  an **in-progress code line** entirely whenever the trailing line is inside a
  fenced block (`AssistantRenderer::in_code` — the `Highlighter` is present iff a
  fence is open); prose has no such lookahead and still streams per wrapped row.
  This is the one place code needs source-line gating — the same mechanism inline
  emphasis would need everywhere.

### Trailing blank rows are trimmed

A model often ends an assistant message with a paragraph break (`…\n\n`) right
before it calls a tool. Those trailing blank source lines render as blank rows,
and the boundary already inserts **one** spacer between the message and the tool
cell — so without trimming they stacked into three blank rows (a reported bug).
Both render paths now drop a message's **trailing** blank rows: the batch
`assistant_lines` pops them after rendering, and the streaming `StreamRender`
**withholds** them in `commit` (they commit only once real content follows, so
they're never a committed-row regression) and trims them in `finish`. Interior
blank lines (a paragraph break *between* two paragraphs) are untouched, and the
trim is gated on `!in_code()` so a blank line inside an open fence — which is
content — survives. `preview` skips trailing blanks the same way (checking the
fence state *after* the trailing line, since a trailing `` ``` `` opens a fence
and keeps the blank before it), so the strip's last row matches the trimmed
batch render. A whitespace-only segment records no message at all
(`App::flush_streaming_segment`/`finish_stream`), so a repaint can't resurrect a
stray `●` bullet the live view never showed. `stream_render_matches_batch_render_on_every_prefix`
covers this with trailing-`\n\n` corpus entries.

`ui::tests::stream_render_is_prefix_stable_over_every_prefix` drives `StreamRender`
over *every* character-prefix of a prose and a fenced-code reply and asserts no
committed row ever changes text **or colour**;
`ui::tests::streamed_code_never_recolours_a_committed_row` pins the specific
call-`(` recolour case (the bug that first exposed the need for the `in_code`
gate); and `ui::tests::incremental_commits_reconstruct_a_fenced_code_reply` proves
the streamed commits plus the final flush equal the fully-rendered message.
`markdown::tests::block_scanner_agrees_with_parse_blocks_on_code_membership` and
`highlight::tests::highlighter_line_by_line_equals_batch_highlight` lock the
incremental scanners to their batch counterparts.

## Scope (and what is deliberately out)

**In:** fenced code blocks (` ``` ` and `~~~`, verbatim), **indented (4-space)
code blocks**, ATX headings, and **thematic breaks** (`---` / `***` / `___`).

Per CommonMark, a *backtick* fence's info string may not contain a backtick —
such a line (`` ```rust`x ``) is inline-code prose, not an opener (treating it
as one swallowed the rest of the reply as an unterminated block); tilde fences
may. A **leading tab** (≥4 columns of indent) makes a would-be `---` line
indented code, never a rule. And while streaming, a trailing **bare `#` run**
(1–6 hashes) is withheld like a partial fence (`markdown::is_partial_heading`):
its heading *level* — and so its bold/italic style — isn't settled until a
space, text, or a 7th `#` (which flips it to prose) arrives, so at a width
narrower than the run its wrapped rows must not reach scrollback yet.

**In — indented (4-space) code blocks.** A run of lines each indented ≥4 spaces
(or a leading tab) is a CommonMark indented code block, rendered **verbatim**
(plain, no language) like a fenced block. It only starts after a blank line or
at start-of-text (`prev_blank` in `markdown::BlockScanner` / `parse_blocks`), so
it never mis-classifies a lazy paragraph continuation as code — matching codex's
`CodeBlockKind::Indented` (which strips the 4-space marker then re-adds a 4-space
prefix; net = the source indentation, which we keep). Blank lines inside the run
stay part of it. This is prefix-stable (a line's membership depends only on the
lines before it) and plain (no highlighter → no within-line lookahead), so its
completed lines stream to scrollback per row like prose.

**In — thematic breaks.** A `---` / `***` / `___` line (CommonMark rule syntax —
`markdown::thematic_break`) renders as codex's unstyled `———` em-dash rule
(`Event::Rule`, `ui::THEMATIC_BREAK`). A `-` rule is ambiguous with a setext `H2`
underline (which we can't render — it needs next-line lookahead that breaks
streaming), so a `-` rule is honoured **only after a blank line**
(`ui::is_thematic_break`'s `prev_blank` gate); `*`/`_` runs are unambiguous and
always render. While streaming, a *partial* marker run (`--`, `**`) is withheld
from scrollback until settled (`markdown::is_partial_thematic_break`, wired into
`StreamRender::commit` beside `is_partial_fence`) — otherwise at content-width 1
its wrapped prose rows could reach scrollback before the third marker collapses
them into the one `———` row (the differential test covers width 3 up).

**In — inline markdown.** `markdown::parse_inline` turns a prose line into a tree
of `Inline` nodes (`Text`, `Bold`, `Italic`, `Strike`, `Code`, `Link`, `Image` —
nesting composes, e.g. `**bold _and italic_**`); `ui::inline_spans` maps each to
a `Style` (emphasis is modifier-only; code is cyan; a link shows its text then a
blue underlined ` (url)`; an image renders its alt), and `ui::wrap_inline`
word-wraps the styled segments **span-preserving** (a bold run keeps its style
across a wrap boundary; a word split by emphasis — `un**bold**` — stays one word).
Parsing is CommonMark-lite: `*`/`**` need a non-whitespace flank (so `2 * 3` and
`snake_case` stay literal), backtick runs match by exact length, `[text](url)`
and `![alt](url)` parse their brackets. It is **line-local** — an unclosed marker
stays literal — so a *complete* line's styling is final (its frozen rows never
restyle). See *Prefix-stable holdback* for the trailing-line case.

**In — lists and blockquotes.** `markdown::list_item` classifies a bullet
(`-`/`*`/`+`) or ordered (`N.`/`N)`) item — its nesting **indent**, marker, and
content — and `markdown::block_quote` strips a `>`. Both are **line-local** (the
line's own prefix decides), checked *after* `thematic_break` so `- - -` / `* * *`
stay rules. `ui::render_list_item` keeps the nesting indent, styles the marker
(`-` plain, `N.` accent-coloured), inline-parses the content and wraps it with a
**hanging indent** (continuation rows align under the text). `ui::render_block_quote`
prefixes each wrapped row with a dim `> `. **Task lists** (`- [x]`) need no
special code — the inline parser leaves `[x]` literal (no `(` follows), so they
render as bullets with a checkbox.

**In — GFM pipe tables.** A header row, a **delimiter** row (`|---|:-:|`, its
column count + alignments govern the grid), and data rows render as a box-drawing
grid (`ui::table_content_rows` — dim borders, bold header cells, per-column
alignment, width-shrunk with `…` truncation when the natural grid overflows). See
*Prefix-stable holdback* — a table is buffered whole and committed only when it
closes, because a later row can widen an already-emitted column.

**Cell content is inline-parsed** — a cell's `` `code` ``, `**bold**`, `*italic*`,
`~~strike~~`, `[text](url)` etc. render styled like prose (codex parity), not as
literal markers (`ui::table_cell_segments` = `inline_spans(parse_inline(cell))`).
Because the markers are stripped, a column is sized to the cell's **rendered**
display width (`segments_cols`, so `` `foo.db` `` measures `foo.db` = 6, not 8),
and `fit_cell_spans`/`truncate_segments` truncate (`…`) + pad the styled segments
per alignment — the span-styled counterpart of the plain-string sizing the older
`pad_table_cell` did. Header cells carry a **bold** base style that the inline
styling composes onto. This is inside the whole-table render, so batch and
streaming stay identical and the holdback keeps it prefix-stable (a table with
inline-markdown cells is in the differential corpus, widths 3–40). *Detection*
still requires the row shapes `markdown::is_table_row`/`table_delimiter` accept
(a delimiter row must contain a pipe); a pipe-less GFM table is rendered as
prose, unchanged by this.

**Syntax highlighting is generic, not per-grammar.** No `syntect`/TextMate
grammars, so a few shapes are approximate: multi-line backtick strings (Go raw
strings, JS template literals) only colour their opening line — they don't carry
across lines like `"""` does (a low-severity cosmetic limitation, still
prefix-stable); string interpolation (`f"{x}"`, `${x}`) isn't parsed; and an
unknown/`text` language renders plain. An in-progress code line is withheld from
scrollback until it completes, because highlighting uses one-char lookahead
within a line (a call's `(`) — see the *streaming* note above.

## Prefix-stable holdback (tables + inline emphasis)

`StreamRender` flushes finished rows to real scrollback as the reply streams, so
anything whose rendering depends on *later* text must be **held back** until it
settles. Two constructs need it, and both reuse the code-block withholding
mechanism (`AssistantRenderer::in_code`, which `StreamRender::commit` already
consults to keep an in-progress *code* line out of scrollback):

- **Tables — block holdback.** `AssistantRenderer` buffers a table's source
  lines (a candidate header confirmed by the next line's delimiter, then rows) in
  a `TableState`, returning **no rows** until the block closes (a non-table line,
  a code fence, or end-of-message via `flush`). `in_table()` exposes that state;
  `StreamRender::commit` withholds the trailing line while `in_table()` (or the
  trailing line is a fresh `is_table_row` candidate), so the whole grid reaches
  scrollback **at once** with final column widths. Batch (`assistant_lines`) and
  streaming share the one `AssistantRenderer`, so they render identically; the
  batch of a *partial* prefix flushes the table-so-far, which the strip `preview`
  matches by flushing its clone.
- **Inline emphasis — line-newline gating.** Because emphasis is line-local, a
  *complete* line is always final; only the **trailing partial** line can restyle
  (an open `**` could still close). `markdown::has_open_inline` reports an
  unclosed emphasis/code/link delimiter on the trailing line, and
  `StreamRender::commit` withholds the whole line while it holds — so an in-flight
  `**bold` never reaches scrollback half-styled. When it settles (the closer
  arrives, or the line ends leaving the marker literal), it commits.

Two partial-marker withholds join the existing `is_partial_fence` /
`is_partial_thematic_break` / `is_partial_heading` set: `has_open_inline` (above)
and `is_partial_list_marker` (a bare digit run like `10`, which a following
`.`/`)` would flip into an ordered marker — recolouring the digits and, at a
narrow width, collapsing their wrapped rows into one marker span).

The differential test `ui::tests::stream_render_matches_batch_render_on_every_prefix`
drives `StreamRender` over **every char-prefix** of a large corpus (tables,
inline emphasis, nested lists, blockquotes, links, task lists, the marker-reveal
flip) at widths 3–40 and asserts the committed rows are always a stable prefix of
the batch render, the final flush reconstructs it exactly, and the preview equals
the batch's last row. It is the guardrail for every construct here — extend it,
never weaken it, when touching the renderer.

### The preview row is never also committed

The strip previews the reply's **last rendered row** while the box shows the
input. That row must be exactly the row `commit` is *withholding* from scrollback
— otherwise a line shows twice, once in scrollback and once in the strip, until
the next chunk supersedes it (the **slow-stream duplicate-line bug**: a chunk
ending in `\n` completes a line, and on a slow model the gap before the next
chunk makes the duplicate linger). So `commit` withholds the **last non-blank
row** (`StreamRender::stable_keeping_preview_row`), not merely the still-growing
trailing line: a just-completed line stays in the preview and only commits once
newer content arrives (or at `finish`). `preview` then always reports an
uncommitted row. `ui::tests::preview_never_shows_a_committed_row_while_streaming`
drives real streaming order (commit before the draw's preview) over every prefix
and asserts the preview is never a row already committed.
