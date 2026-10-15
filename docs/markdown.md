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
  level, like codex; the text after them is inline markdown, so a model's
  `## **Title**` reads `## Title` (see *Headings*).
- **Inline** `**bold**`, `*italic*`, `***both***`, `~~strike~~`,
  `` `code` ``, `[text](url)`, `![alt](url)`, `<url>` autolinks, backslash
  escapes and HTML entities, parsed by CommonMark's delimiter-run algorithm
  (see *Inline markdown*).
- **Lists** in codex's look — every bullet `•`, task items `☐`/`☑`, ordered
  items `N.` renumbered from the list's start, four columns per nesting level,
  hanging-indent wrapping — and **blockquotes** that hold lists and headings
  (see *Lists and blockquotes* and *Block layout*).
- **Codex's block spacing** — one blank row between blocks whatever the
  source had, loose lists compacted, a multi-row item set apart from its next
  sibling (see *Block layout*).
- **GFM pipe tables** — box-drawing grids (see *Tables*).
- **Thematic breaks** (`---` / `***` / `___`) as a `———` rule.

The hard constraint throughout is **prefix-stability** (CLAUDE.md invariant 2):
`StreamRender` flushes finished rows to the terminal's real scrollback as the
reply streams, so a construct whose rendering depends on *later* text must be
**held back** until it settles. **Inline emphasis** needs this (a closing marker
can restyle an already-wrapped row), reusing the same withholding machinery the
code blocks introduced (`AssistantRenderer::in_code`). **Tables** need it for
their whole block (a later row can re-fit every column, so the grid renders only
when it closes) — but the strip **previews the entire forming table live**, so
they still stream visually — see `docs/table-streaming.md`. See *Prefix-stable
holdback*.

## What it looks like

````text
● ## The Code                                  ← `## **The Code**` → `#`s kept, `**` consumed
                                                ← one blank row between blocks
  Save this as snake.py:                        ← prose (word-wrapped, white)
                                                ← (inserted: codex sets a fence apart)
  def main(stdscr):                             ← code, verbatim, syntax-highlighted
      curses.curs_set(0)                        ← 4-space indent preserved
      while True:
          sh, sw = stdscr.getmaxyx()            ← 8-space indent preserved

  • Status: stable                              ← `- **Status:** stable` → `•`, bold label
  • Strengths: a long item that wraps onto      ← hanging indent under the text
    a second row
                                                ← AfterMultiline: a wrapped item is set apart
  • Key features: one row
      1. nested, four columns in                ← `  1. x` → codex's per-depth geometry
  ☑ shipped                                     ← `- [x] shipped`
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
`AssistantRenderer::heading_rows` reconstructs the `# ` marker run (normalised
to one space, like codex) in front of the heading text before word-wrapping.
This deliberately **reverses** the earlier behaviour (drop the `#`s, bold in
`ai_color()`) so we match codex exactly — a user who reads codex output sees the
raw markers, so we show them too.

The heading's **text is inline markdown**, parsed like any prose line under the
level's style: codex's pulldown-cmark pass emits `Heading → Strong → Text` for
`## **Bold title**`, so the `**` is consumed and the line reads `## Bold title`
(the level's dress and the inline styles compose — an h3 stays bold+italic).
Wrapping the heading as plain text showed those markers — the reported bug, a
model wrapping every heading in `**`. A closing `#` sequence (`## Title ##`) is
dropped, as CommonMark says. It is still a pure per-line transform, and an open
marker in a still-growing heading is withheld like any other
(`has_open_inline`), so prefix-stability and the batch/stream agreement are
unchanged.

Code sits under the assistant bullet (aligned with the wrapped-prose indent, no
gutter) and is **syntax-highlighted** by real TextMate grammars (the active
theme's own syntect theme — Catppuccin Mocha by default, codex parity, and
the matching theme under every other `/theme` entry, `docs/theme.md`;
tags/attributes/selectors coloured, embedded CSS/JS in HTML highlighted by
their own grammars). Both the opening and
closing ` ``` ` fences are hidden, and the info-string language is used only to
pick the grammar — it is **not** shown as a label.

**Tabs are expanded to spaces** on the render path (`expand_code_tabs`, a fixed
`CODE_TAB_WIDTH`-space substitution, like codex's `expand_tabs`). A tab is zero
display columns (unicode-width treats it as a control char), so tab-indented
code — Go, Makefiles — would otherwise collapse flush-left, losing all its
indentation. The substitution is display-only: the stored message text keeps its
tabs, so `/copy` is byte-exact. It is a pure per-line transform applied in the
one `AssistantRenderer` core, so batch and streaming stay identical and
prefix-stable (a tab-indented block is in the differential corpus).

Highlighting is a port of codex's stack (`highlight.rs`): **`syntect`** over the
**`two_face`** grammar + theme bundles (~250 TextMate grammars) — the same thing
codex uses (`codex-rs/tui/src/render/highlight.rs`). It **replaced** an earlier
hand-rolled generic tokenizer that had no real HTML/CSS grammar (it mis-parsed a
CSS `#id` selector as a `#` line comment and rendered tags as plain text). We use
syntect's **oniguruma** regex engine (`*-onig`), as codex does — the one C build
dependency in the crate (`onig_sys` bundles + compiles the oniguruma C source, so
a C compiler is needed to build). It's taken deliberately: syntect lazily compiles
a grammar's regexes on first use and caches them in the shared `SyntaxSet`, and
the pure-Rust `fancy-regex` alternative compiles that state far more heavily —
measured at **187 MB RSS across 28 languages with fancy vs ~23 MB with onig**, an
8x reduction. For a coding TUI where the model writes many languages, fancy's
footprint was the memory-growth bug this fixed. The grammar's scopes are resolved to colours by the theme
(`convert_style`: foreground + bold; italic/underline dropped, codex parity), so
each `highlight::Seg` now carries a resolved `ratatui::Style` — the tokenizer is
**no longer colour-agnostic** (a real grammar distinguishes far more than a fixed
six-colour enum could), and `ui` maps nothing. An unknown or `text`/`console`
language renders plain (the theme's default foreground).

## Architecture

- **`src/markdown.rs`** — a pure, unit-tested module (like `file_search` /
  `session`). `parse_blocks(text) -> Vec<Block>` walks the text once into
  `Block::Prose(String)` runs and `Block::Code { lang, lines }` blocks;
  `fence_lang` extracts the info-string language (first token); `heading_level`
  classifies a single prose line as an ATX heading. `BlockScanner` is the
  **incremental** counterpart: `classify(line) -> LineKind` runs the same fence
  state machine one line at a time (for `StreamRender`). No terminal, no styling.
- **`src/highlight.rs`** — the syntect-backed highlighter. `Highlighter::new(lang)`
  resolves the info string to a `two_face` grammar (`find_syntax`, with codex's
  alias patching) and holds syntect's per-line `ParseState` + `HighlightState`;
  `Highlighter::line(text) -> Vec<Seg>` parses one source line (re-appending the
  `\n` grammars anchor on, then stripping it) and resolves its scopes to styled
  `Seg`s via the theme. The state carries multi-line constructs (open strings,
  comments, HTML→CSS→JS context) across calls, and `Clone` lets `StreamRender`
  peek an in-progress line. `highlight(lines, lang) -> Vec<Vec<Seg>>` is a thin
  batch wrapper. Styling is baked in by the theme (not `ui`-owned); an unknown or
  plain language yields one plain `Seg` per line (`plain_style` = theme default
  foreground). The grammar DB, theme, and highlighter are process-global
  `LazyLock` singletons built once.
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

Styling is centralized in `ui/theme.rs` — the code geometry, the list glyphs
(`LIST_BULLET`, `LIST_TASK_OPEN`/`LIST_TASK_DONE`, `LIST_NEST_WIDTH`), and the
colour accessors; the heading styles are modifiers only (`heading_style`). Code hard-breaks at `content_width` rather than letting the terminal
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
tool split, interrupt, `/clear`) **and before every purge rebuild** (resize,
`/clear`, a history rewind — the purge dropped every committed row, so the
follow-up commit re-emits the whole reply at the new width). A mid-stream
**overlay return** doesn't touch it at all: the `Chunk` arm keeps committing
while the overlay is up — the rows merely queue on the viewport — and the
return's draw flushes them, so the restored screen keeps the already-streamed
partial and the overlay-time delta lands exactly once (repainting from history
alone blanked the partial until its next chunk — the Ctrl+O
disappear-then-flicker bug; `smoke.sh` Phases 35 and 82).

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
`highlight::tests::incremental_line_by_line_equals_batch_highlight` lock the
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
stay part of it. One exemption: a line that parses as a **list item**
(`markdown::list_item` — a `- `/`N. ` marker at any space indent) never *starts*
an indented block — a 4-space marker line after a blank is a loose list's
**nested item**, which the top-level indent rule would otherwise swallow (the
CommonMark rule is context-dependent; line-local, the marker is the better
evidence). A marker-looking line *inside* an already-open block still continues
it, so genuine code containing `- x` lines stays whole. Inside a list item the
rule applies past the item's own indent (the scanner is fed the line with its
container prefixes stripped, *Block layout*), so a paragraph indented 4 spaces
under `1. ` is the item's text, not code. This is prefix-stable (a line's
membership depends only on the lines before it) and plain (no highlighter → no
within-line lookahead), so its completed lines stream to scrollback per row
like prose.

**In — thematic breaks.** A `---` / `***` / `___` line (CommonMark rule syntax —
`markdown::thematic_break`) renders as codex's unstyled `———` em-dash rule
(`Event::Rule`, `ui::THEMATIC_BREAK`), set apart by a blank row on each side.
A `-` rule renders **right after a paragraph line too**, where CommonMark would
read the run as a setext `H2` underline: in a model's reply that shape is a
separator that lost its blank line, not a heading, and the setext reading needs
next-line lookahead streaming can't afford — so where the old renderer left the
`---` showing as prose, the rule is drawn. While streaming, a trailing thematic
run — partial (`--`, `**`) or complete (`---`, which `--- x` would turn back
into prose) — is withheld from scrollback until its line settles
(`markdown::has_unsettled_block_prefix`, see *Prefix-stable holdback*).

**In — inline markdown.** `markdown::parse_inline` turns a prose line into a tree
of `Inline` nodes (`Text`, `Bold`, `Italic`, `Strike`, `Code`, `Link`, `Image`,
`Url` — nesting composes, e.g. `**bold _and italic_**`); `ui::inline_spans` maps
each to a `Style` (emphasis is modifier-only; code is cyan; a link shows its
text then a blue underlined ` (url)`; an autolink shows its bare URL in the
link dress; an image renders its alt), and `ui::wrap_inline` word-wraps the
styled segments **span-preserving** (a bold run keeps its style across a wrap
boundary; a word split by emphasis — `un**bold**` — stays one word).

Emphasis follows **CommonMark's delimiter-run algorithm** — the parse codex's
pulldown-cmark pass makes. Each `*`/`_` run (and `~~` pair) is classified by
the left/right-flanking rules (`_` never opens or closes inside a word, so
`snake_case` and `2 * 3` stay literal), then closers, left to right, take the
nearest compatible opener under the rule of three, the pieces between them
becoming one node. That is what fixes the old longest-marker-first matcher's
leaks: `***x***` is italic around bold (it used to show `*x*` in bold), a
closing `***` splits between the two openers it closes (`**x *y***`), and a
leftover marker of a longer run stays literal (`**x*` → `*` + *x*).
`openers_bottom` remembers how far back a search came up empty, so a line of
unmatched markers stays linear (`parse_inline_stays_linear_on_a_line_of_unmatched_markers`).
Link and image brackets are paired the same way: `Brackets` matches a
line's `[`/`]` and `(`/`)` once, in one stack pass, and every opener looks
its closer up there — a scan per opener made a line of unclosed `![`, `[` or
`[a](b` quadratic (a 25 KB one took 1.6 s to parse, now ~10 ms;
`parse_inline_stays_linear_on_a_line_of_unclosed_brackets`).
One relaxation: CJK text puts no spaces between words, so a run whose outer
neighbour is a CJK character is not held to the punctuation clause on that
side — `**标题：**内容` still renders bold, where strict CommonMark (and codex)
leaves the markers showing.

The tokenizer also resolves **backslash escapes** (`\*` shows `*` and is never
a delimiter; a backslash before anything but ASCII punctuation stays),
**autolinks** (`<https://…>` and `<me@example.com>`, shown without the angle
brackets), **link titles** (`[a](url "title")` links to `url`), and the common
**HTML entities** (`&amp;`, `&nbsp;`, `&#169;`, `&#x2014;`, … — an unknown name
stays literal). Code spans keep their backslashes, as CommonMark says, and a
table cell passes every escape but `\|` through to this parser, so `\*x\*` in a
cell stays literal. It is **line-local** — an unclosed marker stays literal —
so a *complete* line's styling is final (its frozen rows never restyle). See
*Prefix-stable holdback* for the trailing-line case.

**In — lists and blockquotes.** `markdown::list_item` classifies a bullet
(`-`/`*`/`+`, its char kept) or ordered (`N.`/`N)`) item — its indent, marker,
and content — and `markdown::block_quote` strips a `>`, checked *after*
`thematic_break` so `- - -` / `* * *` stay rules. The renderer treats both as
**containers** a later line continues (see *Block layout*), and draws them
codex's way:

- **Bullets** render `•` at every depth, whichever of `-`/`*`/`+` the model
  wrote (`ui::LIST_BULLET`); a source `-` never shows.
- **Ordered items** render `N.` in the accent colour (`list_marker_color`),
  `N)` normalised to `N.`, and **renumbered** from the list's start the way
  codex counts: `1.`, `1.`, `1.` reads 1, 2, 3, and a list written `3.`, `4.`
  keeps 3 and 4. An item of another kind — a different bullet char or
  delimiter, or ordered after bullets — starts a new list (CommonMark).
- **Task items** (`- [ ]`, `- [x]`, `- [X]`) replace the bullet with `☐`/`☑`
  (an ordered item keeps its number: `1. ☑`). The box needs whitespace after
  it — a bare `- [x]` stays literal, as codex leaves it — and what follows a
  box is the item's text, never another marker (`markdown::task_box`).
- **Nesting** follows codex's geometry, not the source's: each level sits
  `LIST_NEST_WIDTH` (4) columns deeper — an ordered number right-aligned in
  that padding — whether the model nested by 2, 3 or 4 spaces. Continuation
  rows hang under the item's text.
- **Text under an item** — an indented continuation line, a *lazy* one
  (unindented, no blank before: CommonMark keeps it in the item's paragraph),
  a second paragraph after a blank, a fence or a quote indented to the item's
  content — renders under the item's text. A paragraph indented 4 spaces under
  `1. ` is the item's text, not indented code, and a fence nested under a
  second-level item is still a fence (both used to break: code-coloured prose,
  and backticks on screen).
- **Blockquotes** prefix every row with a dim `> ` (nested quotes repeat it)
  over dim text, and their content is markdown too: `> - a` is a bullet inside
  the quote, `> ## Note` a heading, a blank `>` line separates paragraphs, and
  a lazy line continues the quoted paragraph.

**In — GFM pipe tables.** A header row, a **delimiter** row (`|---|:-:|`, its
column count + alignments govern the grid), and data rows render as a box-drawing
grid (dim borders, bold header cells, per-column alignment, a `├──┼──┤` rule
between every row — Claude Code's full grid). Two touches complete that look: a
**header cell centres** in its column when the delimiter declared no alignment
(`ui::header_align` — a declared `:--`/`:-:`/`--:` still wins, so stated intent
survives), and a cell shorter than its row is **centred vertically** in it, so a
one-line value sits on the middle row of a neighbour that wrapped to three
rather than hugging its top. The block is
**buffered whole** and renders when it closes, with column widths fit to the
header **and every data row** — so a wide later row can never shatter the grid.
When the natural grid overflows the width, each column is first seated at the
width its longest **word** needs and the surplus split by unmet demand
(`allocate_column_widths`), so the column with the genuinely long content gets
the room instead of every column levelling to the same size; only when even
those floors don't fit does it fall back to shrinking the widest column. Cells
**word-wrap** into the allocated widths (taller rows) instead of truncating with
`…`. A **hard-wrapped row** — source carrying a
line break mid-row, so a pipe-carrying fragment line (`96.4 ms |`) follows its
leading-pipe row — **re-joins** the row it wrapped from instead of minting a
phantom one-cell row (`ui::join_wrapped_table_row`, `docs/table-streaming.md`).
While the block is open the strip
**previews the entire forming table** (re-rendered per frame, widths re-fitting
as rows arrive), so a table still streams start-to-finish visually. At the close
it also decides grid vs. **key/value records**: when the grid would be too
cramped to scan (columns starved narrow), each row instead renders as a Claude
Code-style vertical `label: value` block (a capped `─` rule between rows) —
never `…`. See `docs/table-streaming.md` (the state machine, the width
allocation, the records fallback, and the multi-row preview) — this replaced the
lock-widths-at-the-first-data-row streaming, whose guessed widths wrapped any
wider later row into slivers (the reported issue).

**Cell content is inline-parsed** — a cell's `` `code` ``, `**bold**`, `*italic*`,
`~~strike~~`, `[text](url)` etc. render styled like prose (codex parity), not as
literal markers (`ui::table_cell_segments` = `inline_spans(parse_inline(cell))`).
Because the markers are stripped, a column is sized to the cell's **rendered**
display width (`segments_cols`, so `` `foo.db` `` measures `foo.db` = 6, not 8),
and `table_row_lines` word-wraps + pads the styled segments per alignment
(`pad_cell_line`) — the span-styled counterpart of the plain-string sizing the
older `pad_table_cell` did. Header cells carry a **bold** base style that the
inline styling composes onto. Batch (`assistant_lines`) and streaming drive the
one `AssistantRenderer`, so they render identically (a table with inline-markdown
cells is in the differential corpus, widths 3–40). *Detection* still requires the
row shapes `markdown::is_table_row`/`table_delimiter` accept (a delimiter row must
contain a pipe); a pipe-less GFM table is rendered as prose, unchanged by this.

**Syntax highlighting is grammar-accurate (`syntect` + `two_face`).** Real
TextMate grammars, so per-language shapes (multi-line strings/comments, string
interpolation, embedded languages like CSS/JS in HTML) are handled by the
grammar itself and carry across lines via syntect's `ParseState`. An unknown or
`text`/`console` language renders plain (theme default foreground). An
in-progress code line is still withheld from scrollback until it completes
(`AssistantRenderer::in_code`), because a grammar parses the whole line at once
(scopes can change as later characters on the line arrive) — only completed
lines commit; see the *streaming* note above. Guardrail: a single line over
4 KB (a minified bundle pasted into a fence) is left unhighlighted rather than
handed to the regex engine — the bound is a **frame budget**, not just a safety
net: while such a line is the trailing one of a streaming reply, the strip
preview re-highlights it on every frame that arrives with a new chunk, and a
62 KB minified JSON line measured >150 ms per oniguruma pass. Editors cap
tokenization the same way; past the cap the line renders in the plain code
style, byte-identical text.

## Block layout (codex's lists, quotes and spacing)

Codex renders through pulldown-cmark, so its picture of a reply is a tree:
blocks inside list items inside quotes. Ours renders one source line at a time,
so the renderer carries the part of that tree a line can still join —
`AssistantRenderer`'s **container stack** — and reads each line the way
CommonMark's block parser does:

1. **Continue the open containers**, outermost first: a quote needs a `>` (its
   marker and one space consumed), a list item a blank line or one indented to
   the item's content column (that indent consumed). What is left is the line
   relative to the innermost container that matched.
2. **Laziness.** A line that fails to continue every container but is
   paragraph text right after a paragraph line (no blank between) is a *lazy
   continuation*: every container stays open and it continues the paragraph —
   `- item` then an unindented `more` is the item's text, `> a` then `b` the
   quote's. Anything else closes the unmatched containers, innermost first.
3. **Open new containers** the rest starts — quotes and list items, as many as
   it holds (`> - x`, `- > x`).
4. **Render the leaf** — what is left: a fence, indented code, a heading, a
   rule, a table header, or a paragraph line — under the containers' prefixes.

Every scope — the message, each quote, each item — remembers its **last
block**, and that is all codex's spacing needs:

- A new block is set apart from the block before it in the same scope by
  **one blank row** — a heading from its text, a list from the paragraph that
  introduces it, a fence from the prose around it — whatever the source had:
  a missing blank line gains one, a run of them collapses to one. A blank line
  is never rendered where it was; it only ends a paragraph (`pending_blank`)
  and the next block emits the separator. A separator is never stacked on a
  blank row already there (a code block's own trailing blank line), and a
  fence owes its separator until its first code line (`fence_gap`), so a fence
  still empty when a reply pauses leaves no trailing row.
- A **soft break** — a text line right after a paragraph line — is a new row
  with no separator, codex's `soft_break`.
- **Lists** follow codex's AfterMultiline rule: the blank line between two
  items in the source is dropped (loose lists compact), and an item whose
  rows — wrapped text, nested items, continuation paragraphs, code — came to
  more than one is set apart from its **next sibling** by a blank row. A
  nested list hugs its parent; a top-level one is set apart like any block.
- Inside a quote a separator keeps the quote's `> ` column unbroken.

The prefix a row wears composes the containers codex's way: each quote adds
`> `; of the items between two quotes only the innermost speaks — its marker on
its first row, its hang after — because a nested marker already carries its
depth's padding. A **fenced** block is lenient where CommonMark is strict: a
code line strips a quote's `>` where present and up to an item's indent of
spaces, so a model that under-indents the code inside an item's fence still
gets code rather than a fence torn open into prose.

**Why it stays prefix-stable.** Every row a line renders — the separator before
it included — depends only on the lines before it (the stack, the scopes' last
blocks, the pending blank, the previous item's row count) and the line itself.
A separator is emitted *with the line that starts the next block*, never after
the block it follows, so nothing already committed is ever revised. What can
change while a line is still streaming is its **block prefix** — whether `1.`
becomes an item (`1. x`) or prose (`1.5`), `- 1` a nested list, `---` a rule or
`--- x` prose — which the committer withholds until it settles
(`markdown::has_unsettled_block_prefix`, below). The differential test's corpus
covers every rule here at every prefix and width, and the fuzzer's soup grew
fragments for the containers (quoted items, task boxes, lazy and continuation
lines, bare markers).

## Prefix-stable holdback (tables + inline emphasis)

`StreamRender` flushes finished rows to real scrollback as the reply streams, so
anything whose rendering depends on *later* text must be **held back** until it
settles. Inline emphasis and tables both need it, reusing the code-block
withholding mechanism (`AssistantRenderer::in_code`, which `StreamRender::commit`
already consults to keep an in-progress *code* line out of scrollback):

- **Tables — buffer the block, preview it whole (`docs/table-streaming.md`).**
  `AssistantRenderer` buffers the whole block (`PendingHeader` → `Buffering`,
  emitting **no rows**); `in_table()` reports any open table and
  `StreamRender::commit` withholds the trailing line while it holds (plus a
  trailing **header candidate**, `markdown::is_table_header_candidate(tail_src)`,
  before the renderer has consumed it). A block only *opens* on a header with a
  **leading pipe**: GFM's optional-leading-pipe (headerless) form is
  incompatible with streaming — any growing prose line could turn into a table
  header the moment a later `|` streamed in (`` `span`a `` → `` `span`a | b ``
  + a delimiter on the next line), retroactively re-drawing rows already frozen
  into scrollback as a grid — so header candidacy is decided at the line's
  first character, ordinary prose commits progressively, and the rare
  headerless table renders as prose in batch and stream alike. Data rows
  *inside* a confirmed table keep the loose `is_table_row` (the
  hard-wrapped-tail re-join included). Because the buffering states emit
  nothing, no table row can commit before the block closes — at which point it
  renders whole, with column widths fit to **every** row. The strip `preview`
  meanwhile shows the entire forming table (the batch render's uncommitted
  tail — a multi-row preview), so the table still streams visually. Batch
  (`assistant_lines`) and streaming drive the one `AssistantRenderer`, so they
  render identically.
- **Inline emphasis — line-newline gating.** Because emphasis is line-local, a
  *complete* line is always final; only the **trailing partial** line can restyle
  (an open `**` could still close). `markdown::has_open_inline` reports an
  unclosed emphasis/code/link delimiter on the trailing line, and
  `StreamRender::commit` withholds the whole line while it holds — so an in-flight
  `**bold` never reaches scrollback half-styled. When it settles (the closer
  arrives, or the line ends leaving the marker literal), it commits. A marker at
  the very **end** of the growing line is *undecided*, not settled — its
  flanking is the next character's call (`~~` + `s` hides the tildes and
  strikes what follows, `~~` + space keeps them literal), a trailing `!` becomes
  an image opener if a `[` lands next, and a trailing backtick may be a closing
  run the next char **extends** (`` `x` `` + `` ` `` → `` `x`` `` breaks the
  exact-run match and the settled-looking span reverts to literal) — so
  `has_open_inline` also flags a line ending in `*`/`_`/`~`/`!`/`` ` ``.
- **Bare URLs — the autolink withhold (`docs/links.md`).** URL detection flips
  on retroactively (`ht` is prose, `http://e` is a link — the whole word
  restyles blue+underlined), and a URL body reaching the line's end keeps
  growing (its trimmed tail punctuation can re-join: `http://e.` →
  `http://e.com`). `links::has_forming_url` reports both shapes and
  `StreamRender::commit` withholds the trailing line while one holds.

The block-level withholds are one predicate,
**`markdown::has_unsettled_block_prefix`**: a trailing line whose block prefix
the next character could still change, checked at the top level *and behind
the `>`s and list markers the line already holds* (`> -`, `- 1`, `> > ##`) —
a partial fence (`` ` ``/`` `` ``), a thematic run (`--`, `**`, even a complete
`---`), a bare `#` run (its level, so its style, is unsettled), or a list
marker with nothing after it (`is_partial_list_marker`: a digit run, `1.`,
`2)`, or a lone `-`/`*`/`+` — a space makes it an item, a container with its
own spacing and indent, anything else prose). A task box ends the descent: what
follows it is the item's text. The inline withhold, `has_open_inline` (above),
also covers the new inline forms — an unmatched opener after the delimiter-run
algorithm, an autolink or entity still forming, a trailing `\`, `<` or `&`.

**CRLF replies render like their LF twins.** A model echoing Windows files (or
a provider normalising to CRLF) streams `\r\n` line endings; the source is
split on `\n` alone, so each line arrives ending in `\r`.
`AssistantRenderer::content_rows` strips that one trailing CR — it is part of
the line ending, not content — so fences, rules, and tables classify exactly
as their LF twins (a `---\r` used to render as literal prose) and no code row
carries a stray CR into scrollback or a `/copy`. A *mid*-line CR stays: it is
content (prose collapses it as whitespace, and the paint layer drops
zero-width glyphs). Batch and streaming share the one entry point, so the two
never disagree.

The differential test `ui::tests::stream_render_matches_batch_render_on_every_prefix`
drives `StreamRender` over **every char-prefix** of a large corpus (tables,
inline emphasis, nested lists, blockquotes, links, task lists, the marker-reveal
flip) at widths 3–40 and asserts the committed rows are always a stable prefix of
the batch render, the final flush reconstructs it exactly, and the preview is a
**suffix** of the batch render — the whole uncommitted tail, so
`committed + preview` spans the entire render. It is the guardrail for every construct here — extend it, never
weaken it, when touching the renderer. Its adversarial sibling
`ui::tests::stream_stress` fuzzes the same invariants over **generated**
hostile fragment soup (unclosed markers, fence/table shards, CRLF joins,
control characters and ANSI escapes, zero-width/BiDi codepoints, emoji/CJK,
forming URLs, setext underlines, escaped markers, angle autolinks,
reference-style links, nested quotes, lone ZWJs, stacked combining marks,
pipes inside code spans, mixed tab indentation) with a seeded PRNG, so
combinations no hand-written corpus thought of are still machine-checked —
the marker-at-end-of-line, forming-URL, headerless-table and
fence-trailing-blank withholds were all fuzz finds.

It also covers the paths the live-screen assertions cannot see. **The purge
rebuild**: a resize, `/clear`, or history rewind re-renders scrollback from
`committed_rows` while the strip repaints beside it, so
`committed_rows ++ preview` must be the batch render too
(`a_purge_rebuild_plus_the_strip_still_shows_the_whole_reply`); a mid-stream
width change must still reconstruct the reply at the new width
(`a_mid_stream_width_change_reconstructs_the_reply`); and a 0- or 1-column
screen — a tiling WM mid-animation — must not panic a draw
(`a_degenerate_terminal_width_is_survivable`).

And it carries the volume bounds. A 250 KB code reply and a 130 KB
single-line reply must stream in linear time (the `commit` amortizer +
`advance`'s newline high-water + the `preview` memo in `StreamRender`), one
`preview` of a huge code line must stay inside a frame budget (the 4 KB
highlight cap), and the amortizer must never hide rows from the screen — the
commits it defers stay visible in the strip, which is what makes deferring
them safe (`the_commit_amortizer_never_hides_rows_from_the_screen`). One
**known linear bound** is pinned rather than engineered away
(`the_strip_redraw_stays_inside_a_frame_on_a_huge_prose_line`): a huge
*prose* line has no equivalent of the syntect cap, so each frame that lands
on a new chunk re-renders it through `parse_inline`/`wrap_inline` — measured
~6 ms per frame at 130 KB against the ~32 ms cadence. It cannot be cached
incrementally in general, because a later emphasis marker restyles text
already wrapped (the very reason `commit` withholds such a line), so the test
fixes a ceiling that catches a regression into a visible freeze.

`tests/live_openrouter.rs`'s
`live_streamed_reply_render_is_prefix_stable_at_real_chunk_boundaries` and
`live_streaming_never_loses_or_repeats_a_row` prove the same invariants over
real providers' chunk framing, and `scripts/live_smoke.sh` drives the built
binary through a live torture reply, a mid-stream Esc, and mid-stream resizes
in tmux.

### Scrollback and the strip share one frontier

The strip previews **exactly the rows `commit` is withholding** from scrollback:
`preview` returns the uncommitted tail of the same virtual `frozen ++ tail`
sequence `committed` indexes. That one shared frontier is the whole contract —
`committed ++ preview == assistant_lines(prefix)` at every instant, so the reply
is always fully on screen, split between scrollback and the strip, with nothing
shown twice.

It has to be *derived* from the frontier rather than chosen alongside it. The
preview used to pick its own row — "the last rendered row" — and both ways of
disagreeing with `commit` were reachable, because `commit` withholds by **source
line**, not by row:

- A withheld line that wraps to **several** rows previewed only its last, and
  the rest were simply absent from the screen until the line settled. Any line
  `commit` holds back whole does this: a fenced code line (its colour isn't
  final until the line ends), a line with an open `**`/`` ` ``/`[`, a partial
  fence or heading run. Measured against a real OpenRouter reply, a wide
  `vec![…]` literal at width 40 lost **28 rows** — the text visibly vanished as
  the line grew, then landed at once.
- A withheld line that renders to **no** rows — a closing ``` — left the
  preview with nothing of its own, and the old fallback reached back to the last
  `frozen` row, which was already in scrollback. Every fenced block drew its
  last code line twice at the moment it closed.

`commit` still withholds the **last non-blank row**
(`StreamRender::stable_keeping_preview_row`) rather than merely the growing
trailing line, so a just-completed line stays in the preview and commits only
once newer content supersedes it (the **slow-stream duplicate-line bug**: a
chunk ending in `\n` completes a line, and on a slow model the gap before the
next chunk would make the duplicate linger).

When the frontier is clean — everything rendered is committed — the preview is
**empty** and the strip reserves no preview row, the pre-stream pause's shape.
`ui::preview_rows` therefore reports the injected count verbatim; a floor there
would reserve a row `preview_lines` does not draw and trip its `debug_assert`.
A **ceiling** is the one adjustment the geometry does make — `ui::preview_budget`
caps the slot at the rows the region has left, and `preview_lines` trims the
frontier to the same number (tail-first, so the newest rows survive), which is
what keeps a tall preview from squeezing the composer off a small terminal
(`docs/table-streaming.md` *The preview slot is budgeted*).

`ui::tests::preview_and_scrollback_together_show_the_whole_reply` drives real
streaming order (commit before the draw's preview) over every prefix of a corpus
covering each withheld construct and asserts the equality above;
`preview_never_shows_a_committed_row_while_streaming` pins the no-duplicate half,
and `a_wrapped_code_line_previews_every_row_not_just_its_last` /
`a_closing_fence_never_previews_an_already_committed_row` pin the two regressions
by name. `live_streaming_never_loses_or_repeats_a_row` (tests/live_openrouter.rs)
re-checks the same equality at **real** provider chunk boundaries.
