# Assistant markdown rendering (code blocks + headings)

The model streams **markdown**, but assistant replies were word-wrapped as plain
prose by `ui::wrap_text`, which *collapses whitespace runs* (it is greedy
word-wrap for paragraphs). That destroyed the one construct where whitespace is
load-bearing: **fenced code blocks**. A Python snippet rendered flush-left with
its indentation gone and its lines re-flowed together — unreadable.

This adds a small, prefix-stable markdown layer for assistant text: fenced code
blocks render **verbatim** (indentation byte-for-byte, no reflow) and ATX
headings render bold with their `#` markers dropped. It deliberately does **not**
do inline emphasis, lists, or tables (see *Scope* below).

## What it looks like

````text
● The Code                                     ← `## The Code` → bold, `#`s dropped
  Save this as snake.py:                        ← prose (word-wrapped, white)
  ▏ python                                       ← dim language label (hidden fence)
  ▏ def main(stdscr):                            ← code, verbatim, dim gutter + grey text
  ▏     curses.curs_set(0)                       ← 4-space indent preserved
  ▏     while True:
  ▏         sh, sw = stdscr.getmaxyx()           ← 8-space indent preserved
````

Code sits under the assistant bullet behind a dim left **gutter** (`▏ `). We
skip syntax highlighting (codex uses `syntect`; we take no new dependency), so
the gutter plus a slightly-dim code colour carry the "this is code" signal
instead of per-token colour. The opening ` ``` ` fence becomes the dim language
label; the closing fence is hidden.

## Architecture

- **`src/markdown.rs`** — a pure, unit-tested module (like `file_search` /
  `session`). `parse_blocks(text) -> Vec<Block>` walks the text once into
  `Block::Prose(String)` runs and `Block::Code { lang, lines }` blocks;
  `fence_lang` extracts the info-string language (first token); `heading_level`
  classifies a single prose line as an ATX heading. No terminal, no styling.
- **`ui::message_lines`** — the single funnel every render path already goes
  through (streamed scrollback, resize repaint, Ctrl+O transcript, preview). It
  now routes `Role::Assistant` through `ui::assistant_lines`, which walks the
  blocks: prose word-wraps via `wrap_text` (headings bold, `#`s stripped), code
  renders via `wrap_verbatim` (whitespace-preserving hard-break on width) behind
  the gutter. Every **other** role keeps the old plain path unchanged — a user
  pasting ` ``` ` is never code-blocked, and the dark-bg padding math is
  untouched.

Because `message_lines` is the one funnel, scrollback, resize repaint, the Ctrl+O
transcript, and the preview row all agree automatically (invariants 2–4).

Styling is centralized as `CODE_*` / `HEADING_COLOR` consts at the top of
`ui.rs`. Code hard-breaks at `content_width - gutter` rather than letting the
terminal wrap it — a code line longer than the terminal would otherwise be
wrapped by the emulator at the wrong column and misalign the gutter.

## Why this is safe while streaming (prefix-stability)

`ui::stable_commit` flushes every completed reply line to the terminal's real
scrollback as the reply streams, keeping only the **last** rendered line back
(CLAUDE.md invariant 2). That is only sound if `message_lines(text)` is
*prefix-stable*: appending to `text` may change only the last produced line;
every earlier line is frozen forever.

Code blocks and headings preserve this because their rendering is a pure
left-to-right function of the text *before* each line:

- A line's prose/code **mode** is fixed by the fence state entering it — a scan
  of the lines before it, no lookahead. Appending never reclassifies an earlier
  line. An **unterminated** fence renders as code *identically* to a closed one,
  so nothing changes when the closing ` ``` ` finally arrives (it just emits zero
  rows).
- `wrap_verbatim`'s width hard-break is greedy grapheme-by-grapheme, so appending
  extends or starts only the *last* row — exactly like `wrap_text`.
- A heading's style trigger (`#…` at the line start) is seen before any of that
  line's output rows are committed.

`markdown::tests::prefix_stability_no_committed_line_ever_changes` models
`stable_commit` exactly (commit all-but-last, monotonic high-water) and proves no
committed line is ever rewritten as a code-block reply streams one char at a
time; `ui::tests::incremental_commits_reconstruct_a_fenced_code_reply` proves the
streamed commits plus the final flush equal the fully-rendered message.

## Scope (and what is deliberately out)

**In:** fenced code blocks (` ``` ` and `~~~`, verbatim) and ATX headings.

**Out — inline `**bold**` / `*italic*` / `` `code` ``.** These are prose-level
and would need *span-preserving* word-wrap (flatten a styled line to text + span
ranges, wrap, re-slice the spans — codex's `word_wrap_line`). Worse, an emphasis
run can straddle a wrap boundary *mid-source-line*: `stable_commit` could commit
the opening half before the closing marker streams, then a later repaint would
style it differently — a prefix-stability break. Codex only avoids this with
**source-newline-gating** (never commit a line whose source line lacks a trailing
newline), which this codebase does not do. Adding inline emphasis therefore means
first reworking the streaming commit; it is a separate change.

**Out — lists / blockquotes.** Prefix-stable but not the reported bug; deferred.

**Out — tables.** A new table row rewrites the column widths of *already-emitted*
rows, so tables are inherently non-prefix-stable. Codex quarantines them in a
re-renderable "tail" until finalized (a two-region streaming model). Do **not**
add tables without porting that holdback — under the current `stable_commit` they
would corrupt committed scrollback.
