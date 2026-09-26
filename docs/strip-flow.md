# Strip flow — the streaming strip freezes instead of vanishing

## The problem

The live region's streaming strip — a running tool's cell, the `● Thinking…`
block, a live agent tree, the spinner status line, the queued messages, the
toast — is the region's only **elastic** content. Everything else it holds is
decided by state a frame cannot negotiate with (the composer's two rules and
its wrapped rows, the band, the footer, the agent roster), so
`ui::preview_budget` spends what is left on the preview slot and
`ui::fitted_preview_rows` clamps the content to it. That clamp is load-bearing:
a fixed allowance is what once let a forming table squeeze the composer off the
screen (`docs/table-streaming.md`).

What the clamp did with the rows it could not afford was **throw them away**.
`preview_lines` trimmed a live cell from the end, so as a terminal got shorter
a running `bash` call lost its rows in this order:

| rows | what the strip lost |
| --- | --- |
| 16 | nothing — the whole cell |
| 13 | the `+16 lines (15s)` footer **and** `(ctrl+b to run in background)` |
| 11 | two of the four output rows |
| 9 | the whole `⎿` output block — header only |
| 7 | the entire cell |
| 4 | the **status line** (`live_layout` starves the strip itself) |

The `+N lines` row — the one row that says output is being hidden — was the
*first* casualty, and none of it was anywhere: not on screen, not in
scrollback, and not in the resolved cell either (which commits a four-row head
peek behind `ctrl+o to expand`). The reported bug:

> when the terminal window is very short it slowly removing the streaming
> texts instead of freezing the top and make the bash tool scrolls

## The design

The same rule the framed views already follow (`docs/view-flow.md`), applied to
the strip. Two halves:

1. **The strip is one line builder, painted bottom-anchored.**
   `ui::live::strip_lines` produces every strip row in paint order — the
   preview slot and its gap, the status line with the task checklist hanging
   off it and its gap, the queued messages, the toast — exactly
   `strip_rows(has_status, preview_n, task_rows) + queued_rows + toast_rows` of
   them, so the rows `live_height`/`live_layout` reserve and the rows drawn
   agree by construction. `render_strip` paints it through the framed views'
   own `render_framed_tail`, so a strip whose content outgrew its rows keeps
   its **last** ones: the newest streamed rows, the `+N lines` footer, the
   ctrl+b hint, the status line.

2. **The head flows into real scrollback, frozen.** `ui::view_flow` gains a
   branch for the strip, so the rows the anchor skips are committed above the
   live region — where the terminal's own scrolling reads them — and the
   boundary's existing machinery does the rest: `Session::flowed_view` holds
   the signature, `Session::commits_allowed` pauses commits under it, and any
   signature change answers with the standard purge rebuild.

The cell **scrolls**, which is what was asked for: its top freezes above the
region and its newest rows keep the screen.

### What the geometry does *not* change

`preview_n` — the rows the region **reserves** — is still
`fitted_preview_rows`, so `live_height`, `live_layout`, `input_box` and
`cursor_position` are untouched and the composer is protected exactly as
before. Only the strip's **content** is built at the full ask
(`strip_content_preview_rows`), which is what makes it taller than its rect and
gives the anchor something to skip. On a terminal with room for the turn the
two are the same list and nothing flows at all.

### Frozen, not re-signed

The strip is the most volatile thing on the screen: output streams into the
running cell, the elapsed and the token tally advance, the bullet blinks —
all at the 32 ms cadence an active turn re-arms. Signing the flow on its rows
would purge-rebuild the whole screen thirty times a second, so it takes the ↓
manager's `FlowSign::Frozen` (`docs/view-flow.md`) over
`ui::live::strip_flow_key`: which conversation is on screen, whether a status
line is up and which usage tip hangs off it (`docs/tips.md`), each queued
call's name/arguments/status, the live agent group's members, and whether a
thinking phase is open. A new call, a resolution, a new
round or the turn ending re-signs it; a streamed line does not. The width and
the flowed row count are hashed by the caller, so a resize re-signs too.

Frozen rows therefore age: at 11 rows the scrollback may end on `icmp_seq=9`
while the screen shows `icmp_seq=12`, and the `+N lines` count covers the gap.
That is the trade the bug report asked for — stale beats absent — and it is
temporary: the resolution purges the frozen rows and commits the real cell.

### A streaming reply's frontier is the one exception

A reply's preview is its **uncommitted** frontier. `StreamRender` commits every
completed line to scrollback as it lands (invariant 2), so the rows the clamp
drops there are not lost — they are the rows *about to be* committed — and a
frontier that grows on every chunk would re-sign the flow, and purge-rebuild
the screen, once per chunk. So `strip_content_preview_rows` gives the frontier
the **reserved** count and it contributes nothing to the flow; only live cells,
which reach no buffer until they resolve, flow.

### Scope: the composer path only

The strip flows while the composer is on screen. A composer-replacing view
(`/model`, `/settings`, the ↓ band …) splits the region with `view_split`
instead, which pins its frame and squeezes the strip, and there the **view's**
page is the one that flows — one flow at a time, so scrollback can never
interleave two pages. `ui::layout::strip_paint_rows` is deliberately the
composer path's `live_layout` slice and nothing more; teaching it the whole
`live_region_height` precedence chain would be a second spelling of a sum that
can drift.

## The cost, and the guard against it

`view_flow_stale()` runs on **every draw tick**, so the strip branch must not
cost a build per frame. `ui::live::strip_content_rows` is the early-out: it
reports the strip's height without building it (the same `strip_rows` sum the
region already reserves), and a strip that fits returns before
`strip_lines` is ever called. It over-estimates at worst — for a reply's
frontier the reserved count can exceed what the fallback build produces — so
"fits" always means "fits".

## The invariants it leans on

- Invariant 3 (content-anchored viewport, rebuilds via `term::reflow`): the
  flow rides the same purge rebuild, and `write_above` already seats the region
  below an arbitrarily long tail.
- Invariant 4 (`commits_allowed` gating): a cell resolving under an active flow
  waits in history and the flow-exit rebuild regenerates it — which is exactly
  what keeps the frozen preview rows from being duplicated by the real
  committed cell.
- The preview budget (`docs/table-streaming.md`): untouched. The flow changes
  where the unaffordable rows *go*, never how many the region reserves.
