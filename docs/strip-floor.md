# The preview floor — the box holds its place between live cells

## The report

> prompt → thinking (medium) → flicker → the skill shows inline. It happens
> sometimes, not every time.

ClinePass, `cline-pass/deepseek-v4.1-flash-1017` at its default `medium`
thinking, asked *"Who is the current world chess champion? use skill tool
first dont produce intermediate response"*: the model thinks for a fraction of
a second, calls the `skill` tool to load `web-search`, and between the two the
screen jolts.

## What the frames showed

The run was recorded with `tmux pipe-pane` (each chunk timestamped) and
replayed through a terminal emulator one synchronized frame at a time — each
frame is everything up to an `ESC[?2026l`, exactly what a mode-2026 terminal
presents at once. Nothing was torn: no purge rebuild, no region clear, no
byte outside a synchronized update. What moved was the box (100×30, a fresh
screen):

| frame | time | footer row | what the frame shows |
| --- | --- | --- | --- |
| #32 | 13 145 ms | 16 | `● Thinking…` over three rows of thought |
| #33 | 13 169 ms | **13** | `Thought for 0s · 50 tokens` committed; the strip is the status line alone |
| #34–35 | to 13 236 ms | **13** | the same, the box three rows up with blank rows under the footer |
| #36 | 13 251 ms | 16 | `● Skill(web-search)` committed; the box back down |

Down, up three rows, down again ~80 ms later: the flicker. A second, smaller
one came later in the same turn — the second thought giving way to the
reply's first token lifted the box a row, until that line committed.

## Why

The live region is **content-anchored** (CLAUDE.md invariant 3): a frame that
commits `c` rows to scrollback pushes it down by `c`, and a frame whose region
is `h` rows shorter pulls its bottom up by `h`. So the box rises exactly when
the strip sheds more rows than went to scrollback in the same frame.

A thinking phase does that at every hand-off to a tool call:

- The backend closes the phase on the round's **first tool-call piece**
  (`llm::backend::stream_round` sends `ThinkingEnd` ahead of the first
  `ToolCallDelta`), and `Session::settle_reasoning` commits its collapsed
  `Thought for …` line and a spacer — two rows — at once.
- The block stood on five: the `● Thinking…` header, three rows of thought
  (`REASONING_PEEK_LINES`) and the gap under them.
- The call gets a cell only when its batch is **announced**, once its
  arguments have streamed and the stream has ended. Until then the strip has
  nothing to preview — only a `write` previews while it is generated
  (`docs/write-streaming.md`).

So for as long as the call takes to generate, the box sits three rows up; the
batch's cell then pushes it back down. That interval is also why it happens
*sometimes*: when the arguments, the stream's end and the batch all arrive
inside the same frame as the end of the thought (the loop coalesces at
120 fps), no frame ever shows the hole.

The same shape lived in every hand-off between live cells:

| hand-off | rows the box rose | until |
| --- | --- | --- |
| a thought → the round's tool call (the report) | up to 3 | the call's cell landed |
| a thought → the reply's first row | 1 | the reply's first line committed |
| the collapsed read group → the call behind it (`docs/tool-groups.md`) | 1 | that call's cell committed |
| a running command's tail → its settled peek | up to 2 | the next round's content arrived |
| any of the above inside a **subagent's session view** | the same | the same |

**Not a regression.** v0.18.0, built from its tag and driven through the same
offline turn at the same size, hops at the same frames by the same rows; the
commits since touched none of this path.

## The rule

`docs/slow-stream.md` already states it: *the preview never loses more rows
than that step committed*. It was enforced only **inside a reply**, by
`StreamRender`'s own floor, because only the renderer knew what it answered a
token ago. Between the strip's cells nothing enforced it — and the boundary is
the one place that knows both numbers it needs: what the preview slot
reserved last frame, and how many rows were queued for scrollback since
(`InlineViewport::pending_rows`).

## The design

`ui::PreviewFloor` (`ui::layout`) is a pure tracker the boundary owns beside
its `StreamRender`s, driven once per painted frame by `draw_conversation`:

1. **Before the frame** — `floor(app, committed, width, height)`: the slot
   the last frame reserved (its rows and their gap) less the `committed` rows,
   back in preview rows, clamped to the frame's `preview_budget`. Injected with
   `App::set_preview_floor`, the `set_stream_preview_rows` pattern.
2. `ui::preview_rows` reserves `max(content, floor)`, and `preview_lines` pads
   the content with **blank rows, under it**, up to the floor — the rows the
   next content lands on. Every consumer (`fitted_preview_rows`, the cursor
   seat, the strip above a composer-replacing view, the agent session view's
   strip) reads `preview_rows`, so the reserved rows and the drawn rows agree
   by construction, and the strip's `debug_assert` still holds.
3. **After the frame** — `record(app, width, height)`: the preview rows it
   reserved, the next frame's starting point.

What the user sees now: the thought's line commits, the rows the block stood
on stay blank under it, the status line, the box and the footer do not move,
and the call's cell lands in the held rows.

### What it holds, and what it never does

- **Only while a live strip is up** (`holds_strip`): a turn in flight — the
  viewed agent's run inside its session view, the main turn otherwise — and
  no modal prompt, which replaces the whole region and purge-rebuilds on its
  close. `preview_rows` applies the injected value only under the same gate,
  so a value from a frame ago can never pad an idle composer.
- **Never past the preview budget.** The budget protects the composer first
  (`docs/table-streaming.md`); a floor past it would squeeze the box.
- **Never as strip-flow content.** Held rows are geometry. The strip flow
  (`docs/strip-flow.md`) sizes a live cell's overflow from the content's own
  ask (`layout::content_preview_rows`), so padding can never make the strip
  outgrow its rect — a flow there would purge-rebuild the screen over blank
  rows.
- **Not across a purge rebuild.** `repaint_conversation` and
  `repaint_agent_view` reseat the region from scratch, so they paint without a
  floor and record afresh (`PreviewFloor::reset`).
- **Not at the turn's end.** The strip collapses into the summary; that
  settle is the end of the turn, not a hop.
- **A single owed row holds two.** A preview row brings its gap, so when the
  commits leave one row owing the slot keeps a row and its gap and the box
  steps *down* a row rather than up.
- **It lets go as soon as the commits cover it** — a cell committing at least
  the rows it stood on leaves no floor at all, so nothing lingers.

## Alternatives considered

1. **Keep the `● Thinking…` block up until the next cell arrives.** The
   transition would be one frame, but the block would claim the model is
   thinking while it is generating a call; the cell's order in history would
   need a second settle point; and the thought → reply and read-group cases
   would remain.
2. **A hold for reasoning alone, kept in `App`.** `App` records; the boundary
   commits, view-gated (`Session::commits_allowed`) — only the boundary knows
   how many rows actually went to scrollback. And every other hand-off would
   need a hold of its own.
3. **One floor over the whole preview slot, at the boundary** (chosen): the
   rule `StreamRender` already proved inside a reply, lifted to the strip as a
   whole and applied at the one place that knows both numbers — one rule for
   every hand-off, present and future, the agent session view included.

## Testing

- `ui::tests::preview_floor` plays frames the way the boundary paints them
  and asserts the invariant on each: a thought giving way to its call (and
  the call landing in the held rows, which are blank), to the reply's first
  row, the read group settling under the next call, a subagent's thought in
  its session view; commits covering the slot release it at once; and the
  guards — idle, a modal prompt, the budget (with no flow), a rebuild — each
  mutation-checked (each fails with its guard removed).
- `scripts/smoke.sh` **Phase 153** drives the default offline turn on the
  real binary with Verbose output off — a two-line thought, then a
  Read+Edit+Bash batch generated over about a second — sampling the footer
  ~20 times a second with Phase 121's double-capture confirmation: no hop
  mid-turn, no purge rebuild in the byte stream, and the thought, the group's
  line and each cell exactly once. It fails on the pre-fix binary (the box
  hopped 20 → 18 at the thought and 24 → 23 at the group) and passes with the
  fix.

## Known limits

- **The turn's end with the spinner tip on.** The tip row hangs off the
  status line (`docs/tips.md`) and has no committed counterpart, so when the
  strip collapses into the summary the idle box settles one row higher. A
  settle, not a bounce — nothing brings it back down — and unchanged by this.
- **A toast expiring mid-turn** lifts the box its one row, one way.
- **A composer-replacing view's strip** records the composer path's fit
  (`fitted_preview_rows`), which can differ from the rows `view_split` left
  it; it holds at most what the composer path would have.
