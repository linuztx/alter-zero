# Character-level diff marking in the `edit`/`write` cell

## The problem

A numbered `edit` cell paints the whole `-` row on a dark-red tint and the
whole `+` row on a dark-green one (`docs/tools.md` → *Rendering*). That tells
you a line changed. It does not tell you **what** changed:

```
● Edit(/home/linuztx/hello.txt)
  ⎿  Updated hello.txt (+1 -1)
      1 -Bruce Rivera
      1 +Bruce Rivero
```

Both rows are one flat block of colour, so finding the edit means reading the
two lines character by character and spotting that `Rivero` became `Rivera`.
On a long line — a 120-column function signature where one argument moved —
that is genuinely hard, and it is the single most common shape an `edit`
produces: a *modification* of a line, not a wholesale replacement.

## What it does now

The pair above renders with the characters that actually differ lifted onto a
**brighter** tint and bolded, while everything the edit left alone keeps the
muted row tint:

```
      1 -Bruce River█            ← the `a` on the bright red, bold, undimmed
      1 +Bruce River█            ← the `o` on the bright green, bold
```

So the row tint still answers "did this line change?" at a glance, and the mark
answers "where?" without reading.

The granularity is deliberate and it is the whole design constraint: **only
what changed is coloured.** `Bruce River` is identical on both sides, so
marking it — as a word-level pass would, painting all of `Rivera`/`Rivero` —
says the surname changed when only its last letter did. Every rule below falls
out of that one: no merging across unchanged text, no marking a character that
was merely *near* an edit, and no marking at all on a line that was replaced
rather than edited.

## Where it lives

Purely presentational, entirely inside `src/ui/`:

- **`src/ui/inline_diff.rs`** — the pure core. Tokenizing, the token-level LCS,
  the similarity guard, and the `-`/`+` run pairing. No terminal, no styles,
  no `ToolCall`: it maps `(sign, text)` rows to changed **byte ranges**.
- **`src/ui/file_cell.rs`** — applies those ranges as styles
  (`row_segments`), which is the only place the theme is touched.

Nothing about the **model-facing** output changes: `llm::tools`'
`render_numbered_diff` still emits the same `{n:>W} {sign}{text}` body, so the
wire format, `ToolCall::context_output`, the rollout and the derived context
are all untouched. The refinement is derived from the *parsed body rows*, which
is also why it lights up on **old rollouts** — a `/resume`d session from before
this change renders with the new highlight, because there is no new field to
have been recorded.

## Pairing (which two lines get compared)

`refine_rows` scans the parsed body for a maximal run of consecutive `-` rows
immediately followed by a run of consecutive `+` rows, and pairs them
positionally: `removed[i]` with `added[i]`, up to `min(n, m)`. A `⋮` hunk gap,
a `…` note, a context row or an unparseable line ends the run (`RefineRow::Break`).

git's `diff-highlight` refuses the whole block when the two runs differ in
length. We pair the overlap instead and let the **similarity guard** below
decide per pair, because a `-1 +2` change is usually still a modification of
the first line plus an insertion, and refusing it loses the one highlight the
user wanted. A mispairing is not harmful: dissimilar lines fail the guard and
fall back to the plain row tint — exactly what they rendered as before.

## The pair refinement

`refine_pair(old, new)`:

1. **Split** both lines into grapheme clusters (`tokens`) — one unit per
   cluster, so a combining mark or an emoji-ZWJ sequence stays whole and every
   range lands on a boundary the renderer can slice at.
2. **Trim** the common prefix and suffix tokens. This is what keeps the cost
   linear in practice — a typical edit's changed middle is a token or two,
   however long the line — and mirrors `llm::tools::diff_lines`' own trim.
3. **Bound, then LCS** the changed middle only. Before filling any table, the
   guard's cheap half runs: the LCS can only match tokens present on both
   sides, so the smaller middle's ink is an upper bound on what it can still
   contribute, and a pair that fails the threshold *even at that bound* is
   rejected outright. It rejects nothing the guard below wouldn't, and it is
   what stops an `edit` over a file of long, heavily rewritten lines from
   paying a quadratic table per row. What survives is LCS'd, bounded by
   `INLINE_DIFF_MAX_CELLS`; past *that* bound (a minified bundle line) the whole
   middle is marked changed on both sides rather than skipped — the prefix and
   suffix are still real common context, so that answer is honest and costs
   nothing.
4. **Guard.** Refine only when the two lines are actually related:
   - at least one common **non-whitespace** display column, and
   - `common_nonspace * 100 >= longest_nonspace * INLINE_DIFF_MIN_COMMON_PCT`.

   Whitespace is excluded from the count so that two unrelated lines sharing
   an indent don't read as similar, and punctuation weighs what it is worth —
   the `(` and `)` shared by `alpha()` and `zulu_bravo(charlie)` are two
   columns against nineteen, so that pair stays flat. Without the guard a
   replaced line would come back speckled with the handful of letters the two
   texts coincidentally share, which points at nothing.

   Being a *ratio*, the guard judges proportion rather than absolute size:
   `old = 1` → `new = 2` shares only its `=`, four fifths of the line having
   changed, so it stays flat too — "the whole line changed" is what the row
   tint already says, and marking all but one character adds nothing. The
   trade-off is that the verdict near the threshold moves with identifier
   length; that band is exactly where flat and refined are worth the same, so
   it is left as one number to tune rather than a second heuristic.

   Below the threshold `refine_pair` returns `None` and the row keeps the plain
   tint — a line that was *replaced* rather than *edited* has no "what changed"
   to point at, and highlighting all of it is strictly noisier than
   highlighting none of it.
5. **Coalesce adjacent** changed units into runs, so a changed `z(1)` paints as
   one block rather than four flecks of colour.

   Adjacent *only*. A run of unchanged text between two changes is never
   bridged, however short: the `0`s of `8080` → `9090` stay plain, and so does
   the `and beta = ` between two values changed at opposite ends of a line.
   Bridging would colour characters the edit did not touch — the one thing
   this marking exists not to do.

## Rendering

`row_segments` walks the syntax-highlighter's segments for the row, splitting
them at the changed-range boundaries, and gives each piece its final style:

| | background | text |
|---|---|---|
| `+` row, untouched | `TOOL_DIFF_ADD_BG` | syntax colour |
| `+` row, **added characters** | `TOOL_DIFF_ADD_MARK_BG` | syntax colour + **bold** |
| `-` row, untouched | `TOOL_DIFF_DEL_BG` | syntax colour, dimmed |
| `-` row, **removed characters** | `TOOL_DIFF_DEL_MARK_BG` | syntax colour + **bold**, *not* dimmed |

The removed row's changed text deliberately escapes the `DIM` the rest of the
row carries: dimming the one run the eye is meant to find would defeat the
point of marking it.

Because the styles are baked into the segments *before* `code_content_rows`,
the mark survives the wrap — a changed run split across two display rows keeps
its tint on both — and the row's trailing pad keeps the plain row tint,
so the bright block ends where the changed text ends instead of bleeding to the
terminal's edge.

## Where it shows

Everywhere the numbered body renders, since both builders share the pass:

- the collapsed inline cell (`file_cell_lines`, peeked),
- the Ctrl+O transcript expansion (same builder, uncapped),
- the tool-permission prompt's preview (`numbered_body_lines`), so you can see
  what an `edit` is about to change *before* approving it — which is the one
  place the question "what exactly changed?" is load-bearing.

A `write` body (brand-new content, no sign column) has no pairs and is
untouched, as is a `read`.

## Cost

The pass is O(rows) plus, per pair, O(characters) for the trim and O(middle²)
for the LCS — where the middle is empty for an unchanged tail and a character
or two for a typical edit. Character granularity makes the middle longer than a
word-level one would, which is exactly why the prefix/suffix trim and the
pre-LCS bound below carry the weight. `INLINE_DIFF_MAX_CELLS` caps the worst case. The live cell
re-renders every animation frame, so the trim is not an optimization but the
thing that makes the feature affordable at 32 ms.

## Tuning

All in `ui/theme.rs`: `TOOL_DIFF_ADD_MARK_BG`, `TOOL_DIFF_DEL_MARK_BG`,
`INLINE_DIFF_MIN_COMMON_PCT`, `INLINE_DIFF_MAX_CELLS`.
