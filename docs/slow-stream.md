# Slow streams — a few tokens a second through the inline pipeline

Date: 2026-09-20

## The question

The dummy backend streams its canned replies **a word every 45 ms**, and the
smoke suite drives every inline-rendering guarantee at that pace. A real model
on a struggling machine — a 7B model on a laptop CPU, a saturated API — streams
**tokens**, not words, and at two to seven of them a second. Two things change
at that speed, and neither is a matter of degree:

- **Every intermediate state is on screen long enough to read.** At 45 ms a
  half-open `**`, the first backtick of a fence, a `##` with no heading text
  yet, a table header before its delimiter row, all pass in one animation
  frame. At 300 ms a token each of them is a *frame the user looks at*, and a
  row committed to scrollback has a third of a second in which the strip
  above the box can contradict it — show it a second time, or show it
  differently.
- **The chunk boundaries land where a word split never puts them.** A real
  tokenizer's pieces cut `**bold` into `*`, `*bo`; a fence opener into one
  backtick at a time; a paragraph break rides one piece with the next line's
  first characters. `stream::chunks` — the word split every scripted demo
  streams by — can produce none of those, so nothing had ever driven the
  boundary through them at all.

So the question was not "does the renderer hold at every prefix" (it did —
`stream_render_is_prefix_stable_over_every_prefix` and the hostile-soup fuzz
already drove every *character* prefix of a corpus through it) but "does the
**terminal** hold when those states last": the frames the loop emits, the
rows it blanks, what a terminal that honours no synchronized update would
present.

## The rig

Three pieces, all reusable, so the answer stays checkable:

- **`ALTER_ZERO_CHUNK_DELAY_MS`** — the pause the dummy takes after every
  streamed piece of reply text (and every live tool-output line):
  `stream::CHUNK_DELAY`'s 45 ms unless set. The twin of
  `ALTER_ZERO_STARTUP_DELAY_MS`, threaded the same way
  (`DummyAi::with_chunk_delay`, `tui::config::chunk_delay`, the one
  `replay`/`pace` pair in `stream::dummy`). `ALTER_ZERO_CHUNK_DELAY_MS=400
  alter-zero` then `stream some markdown` is the interactive form of this
  document.
- **`stream::tokens`** — the stress twin of `chunks`: token-sized pieces that
  walk a cycle of lengths averaging three characters (`TOKEN_LENGTHS`, one-
  and two-character pieces included), cut short wherever a run of marker
  characters (`*_~` `` ` `` `#-|=>:` and the digits) would ride one piece
  whole, so every run the renderer classifies a line by streams **one
  character at a time**: `**` as `*` + `*`, a fence as three pieces, `|---|`
  as five, `10.` as `1` + `0` + `.`. Every partial-marker predicate the
  committer carries — `is_partial_fence`, `is_partial_heading`,
  `is_partial_thematic_break`, `is_partial_list_marker`, `has_open_inline`,
  `has_forming_url` — is exercised on every occurrence. Deterministic (the
  smoke phase times a turn by its piece count), faithful (the pieces
  concatenate to the input), never longer than `TOKEN_MAX_CHARS`, never
  cutting a character in half.
- **The `markdown` scenario** — a prompt mentioning "markdown" plays
  `stream::MARKDOWN_TOUR`, a document that carries every block kind the
  renderer knows: an `#` heading over a wrapped paragraph, `**bold**`,
  `*italic*`, `~~strike~~`, `` `code` ``, a `[link](…)` and a bare URL, a
  bullet list that wraps and nests three deep, an ordered list with a
  two-digit ordinal, task items, a two-line blockquote, a Python fence with a
  **blank line inside it** (content, never a paragraph break), a Rust fence
  whose signature is **longer than eighty columns** (a withheld multi-row
  line), a three-column table with two-column emoji, a `---` rule, an `###`
  heading, and the hand-off. It is **one text-only message** — no thinking
  phase, no tool calls, nothing to split the reply — so the incremental
  renderer carries the whole document from its first character to its last,
  and it is written so that **no two content lines render alike**, which is
  what lets the smoke phase count each row exactly once. It sits below the
  `table` demo in the registry (a "markdown table" prompt means that one) and
  carries the `/init` guard the file and agent demos carry, since `/init`'s
  canned prompt asks for Markdown headings.

`scripts/smoke.sh` **Phase 121** streams the tour at 90 ms a piece on an
80×24 terminal (about 65 s, ~720 pieces) and, beside it, at 45 ms on a 40×12
one, where the signature wraps to three rows, the table falls back to
key/value records and the preview slot is a couple of rows. It watches the
wide turn three ways, about a dozen times a second (800-odd samples over
the turn):

1. **Scrollback is append-only.** Each sample of the terminal's history
   (one `capture-pane -S -` split at the screen height — every row that has
   scrolled off the visible screen) must be a prefix of the next. A
   committed row that changed, moved or was written twice shows up here.
   And **the box never hops up**: the region is content-anchored, so a
   commit pushes the footer down until it reaches the bottom and nothing
   pulls it back — a sample whose footer row is above the previous
   sample's is a bounce.
2. **No content row is on screen twice.** Thirty-three phrases, one per line
   of the tour, are counted across history plus the visible screen; any
   count above one is a duplicate. `committed ++ preview` is the reply at
   every instant (the one-frontier contract, `docs/markdown.md`), so a row
   committed while the strip still previews it — the slow-stream
   duplicate-line shape, which at this pace stays on screen for a third of a
   second — shows up as a count of two. A duplicate must survive **two
   consecutive samples** to count: `tmux capture-pane` reads the grid as
   bytes parse, so a single sample can catch a synchronized update
   half-applied, and that transient is exactly what mode 2026 exists to
   hide.
3. **The byte stream**, recorded with `tmux pipe-pane`: every live-region
   clear inside a synchronized update (Phase 15's rule, over a turn a hundred
   times longer) — and, after the change below, **no live-region clear at
   all**.

Then the settled transcript must carry all thirty-three phrases exactly once,
in document order, with the footer on the last row and one composer on
screen, and every grid row the same width.

## What the run showed

The **pure** layer was already right, and stayed right under the new split:
`the_markdown_tour_streams_prefix_stable_at_every_width` drives every
character prefix of the tour through the differential invariant at widths
20, 40 and 80, and
`the_markdown_tour_never_previews_a_committed_row_token_by_token` drives the
exact token pieces the dummy streams, with the strip capped to what an 80×24
region can reserve, asserting no non-blank preview row is ever already in
scrollback and that the capped preview always tail-follows.

The **boundary** rendered both sessions correctly: every phrase once, in
order, the box flush at the bottom, the grid aligned, 0 clears outside a
synchronized frame. What the recording also said, from one 65-second turn:

| frames | bytes | shape |
| --- | --- | --- |
| 2 184 | 361 KB | the whole turn, ~33 fps (the status chain's 32 ms) |
| 2 065 | — | under 300 bytes: the spinner and the timer, a diff each |
| 49 | 107 KB | a commit: `ESC[J` blanks the live region, then a full repaint |

That last row is the finding. `write_above` — ratatui's portable
`insert_before`, ported — scrolls, writes the committed rows, then clears
from the region's new top to the end of the screen, and `paint_frame` repaints
the region into the blank. Inside one synchronized update the blank is never
presented, which is what Phase 15 proves and `docs/flicker.md` explains. But
a terminal that does not honour mode 2026 — Terminal.app, xterm, an older
VTE — renders whatever it has parsed when its refresh comes due, and a commit
frame is 1.6 KB on average and 3.9 KB at the widest, against a 4 KB pty read.
A frame split across two reads with the `ESC[J` in the first one is a
presented frame with **no box** — the streaming blink the flicker fix
removed everywhere a synchronized update reaches, still alive everywhere it
doesn't. At 45 ms a word a commit lands every couple of seconds; at a slow
model's pace it lands once per completed line, so the box would blink about
once a second for the length of the reply.

The sampling found a second thing, and read it first as scrollback
changing: twice in a run, the history's *length* dropped by two rows, and
the frames at those samples showed the box sitting two rows above the
screen bottom with blank rows beneath it — right after the Python fence
closed, and again after the Rust one. That is the strip. A closing ```
leaves nothing uncommitted (every code row is in scrollback, and the fence
itself renders no row), so `StreamRender::preview` answered with **no
rows**, `preview_rows` reserved none, and the strip dropped its preview
slot *and its gap*: the region shrank by two, in place, the box with it.
The next line's first character brought the slot back, and the box dropped
two rows again. At 45 ms a word the whole excursion is one frame; at a
slow model's pace — with the tour streaming the fence one backtick at a
time, then a newline, then the next opener — the box sits up there for
half a second on every code block.

A watch for exactly that — the footer's row must never move *up* between
two samples — then caught the smaller sibling once the first was fixed: a
one-row hop at every fence **opener**. After a paragraph break the strip
holds the break's blank row plus the trailing line; streamed one backtick
at a time that line is `` ` `` and then `` `` `` — prose, one row — and at
the third backtick a fence marker, which renders **no** row. The preview
went from two rows to one for the length of a token, then back to two when
the opener's newline put an empty code row under it. And the same
invariant, written into the pure differential harness, found a third
shape the terminal runs never reached: at a narrow width a table header
wrapped as prose while its delimiter streams collapses into a shorter grid
the moment the delimiter confirms, and the grid into key/value records when
the first data row arrives — a re-layout of the same lines that happens
to be shorter.

## The change: repaint in place

The clear was never load-bearing. `paint_frame`'s blit writes **every cell**
of the region (`visible_cells` emits the blanks too; only a wide glyph's
shadow is skipped, and the glyph covers it), so the rows the region occupies
after a commit are overwritten whatever they held — the previous region's
lower rows, shifted up by the scroll. What the clear also covered was the
strip of rows *below* the new region that the previous, taller region had
occupied: a frame in which a forming table's whole block commits while the
strip collapses from its multi-row preview to one row pushes the region down
by fewer rows than it shrank, and the old region's tail is left beneath it.

So `write_above` no longer clears, and `paint_frame` paints **first** and
then blanks exactly those rows: from the new region's bottom to the larger of
`repin.clear_below`'s vacated rows (a plain shrink) and `painted_bottom` (the
bottom of the region as it was last painted, which `scroll_up` now carries
through every scroll since a painted row is a screen row). Below the region
is always free — the box is the bottom-most content — so over-clearing costs
at most a blank row, and a per-row `ESC[2K` there can never blink anything.
A terminal that presents a half-parsed commit frame now shows, at worst, a
box whose lower rows are one frame behind its upper ones.

Two paths needed a word. `clear_scrollback_and_screen` (every purge rebuild)
resets `painted_bottom` to zero, since a blank screen holds no region to clear
beneath; and `restore`, the one flush that no paint follows, blanks the region
itself after a pending flush at quit — what `write_above`'s clear had been
doing on that path — so an exit never leaves a torn box behind. The tmux
ordering bug the old clear stepped around ("a full clear immediately followed
by a scroll spills garbage to scrollback") is moot with no full clear at all.

Phase 15's liveness self-check moved with it: it used to fail on a recording
with no clears, as proof the meter had seen the commit path; it now fails on
one with no synchronized frames, and Phase 121 pins `clears=0` as the design.

## The change: the preview never shrinks between commits

The strip is the region's only elastic content, and between two tokens
nothing about a reply that is still streaming has changed except the
frontier — so the rows it reserves must never *shrink* unless rows moved
to scrollback. Stated as an invariant: **the preview never loses more rows
than that step committed.** All three shapes break it, and no per-shape
rule covers the third (a table's re-layout is shorter or taller by an
amount only the layout knows), so `StreamRender` keeps a **floor**: the
height of the last preview, lowered by every row `commit` moves to
scrollback, and `preview` pads its rows with blank ones up to it — the
rows the next content will land on. The closer's case keeps the slot and
its gap; the opener's case keeps the row the `` `` `` prose held a token
earlier; the table's case keeps the wrapped header's rows until its data
rows fill them. The padding is geometry, not content: the next content
takes those rows rather than stacking under them, the tail-follow cap
spends its rows on content first, the bullet-home case is untouched (a
reply that has committed nothing yet still previews its bullet), and
`finish` never consults the preview, so the settled reply is
byte-identical.

The rule lives in the renderer rather than the strip because only the
renderer knows what it answered a token ago and what committed since:
from a bare `Vec<Line>` the strip cannot tell "one withheld row and a
pending fence" from "one withheld row". The one-frontier contract
(`committed ++ preview == assistant_lines(prefix)`) gains its single
tolerance for it: the batch render trims a reply's trailing blank rows, so
the screen may hold **trailing blank rows** the batch render does not
(`matches_up_to_padding`) — that shape and nothing else; every content row
still has to match, and a blank tail row inside an open fence is content
on both sides. And the differential harness now carries the invariant
itself: `assert_stream_matches_batch` fails any step whose preview shrinks
by more than that step committed, over the whole hostile-soup fuzz corpus
at every width — which is how the table's shape was found at all.

The smoke phase's own hop watch confirms a lower footer against a second
capture a beat later, since `tmux capture-pane` can read a frame
half-parsed — the screen scrolled, the region not yet repainted, the old
footer a row up — and a real hop lasts a token where a torn frame lasts
microseconds.

## What stays as it was, and why

- **Every frame hides and re-shows the cursor.** 2 184 hides in the run. The
  Hide is what keeps a cursor-trail terminal (kitty) from streaking on a
  scroll (`docs/flicker.md`); the frames it brackets are tiny (under 300
  bytes, never split across a pty read), so a terminal without synchronized
  output cannot present the hidden state in practice.
- **A commit repaints the region in full** (`prev` is dropped, since the
  rows moved). The region's rows at its new position never overlap its old
  ones, so there is no diff to take; 1.6 KB a committed line is the cost.
- **The partial states themselves** — a `**` shown literal until it closes, a
  `##` styled as its level settles, a header row shown as prose until the
  delimiter confirms it — are the strip's, never scrollback's, and they are
  what any streaming renderer shows at a slow pace. Hiding them would mean
  showing nothing for the line, which is worse.

## Testing

- `stream::tests::script`: `tokens` round-trips, stays under
  `TOKEN_MAX_CHARS`, is finer than words, splits `**` and ``` ``` ``` runs and
  carries a newline mid-piece, is deterministic.
- `stream::tests::dummy`: `with_chunk_delay` paces successive chunks at
  least the delay apart.
- `stream::tests::turns` / `scenario`: the `markdown` cue plays a text-only,
  token-paced turn whose chunks reconstruct `MARKDOWN_TOUR`, carrying every
  element, the image acknowledgement and the hand-off; every content line of
  the tour is distinct; the registry example selects it and `/init`'s prompt
  never does.
- `ui::tests::stream_render`:
  `the_strip_never_shrinks_between_two_tokens_without_a_commit` (two
  fenced blocks between paragraphs and a two-row table, every prefix,
  three widths) and `a_fence_line_previews_one_blank_placeholder_row` (the
  closer's one blank row, the opener's break-plus-placeholder, the next
  line replacing it, the bullet home untouched).
- `ui::tests::stream_stress`: the two tour tests above, and the no-shrink
  invariant inside `assert_stream_matches_batch` over the fuzz corpus.
- `scripts/smoke.sh` Phase 121: the three watches, the settled transcript,
  the narrow session; Phase 15: the synchronized-update rule, its liveness
  check on the frames.
