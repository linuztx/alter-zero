# The strip hold — a settled thought keeps its rows for its call

Date: 2026-11-07

## The symptom

A thinking model asked to load a skill before answering
(`cline-pass/deepseek-v4.1-flash-1017`, `medium`: *"Who is the current world
chess champion? use skill tool first"*) thinks for a moment, then calls the
`skill` tool. Between the thought and the `● Skill(web-search)` cell, the
input box, the status line and the footer **jumped up and dropped back
down**. It happened on some turns and not others.

A byte recording of the live turn (`tmux pipe-pane`, replayed one
synchronized frame at a time) pins it down. In the second round, a thought
followed by a `bash` call:

| frame | time | strip | footer row |
| --- | --- | --- | --- |
| #139 | 5175 ms | `● Thinking…` over two rows of thought | 29 |
| #140 | 5209 ms | `Thought for 0s · 59 tokens`, then **nothing** | **27** |
| #142 | 5251 ms | `● Bash(python3 …)` over `⎿ Waiting…` | 31 |

The box rose two rows for 42 ms, then fell four. In the first round,
thought → `Skill`, every event happened to land between two frames, so
nothing moved. That timing is why it only happened sometimes.

## Why

The backend closes a thinking phase when the first piece of the *next*
thing arrives (`llm::backend`'s `stream_chat` callback). For a round that
ends in a tool call, that first piece is the call's first `ToolCallDelta`.
The call is only **announced** (`ToolBatch`) once the stream has ended,
after the model has finished generating its arguments. So:

1. `ThinkingEnd` — `settle_reasoning` commits the one-line
   `Thought for …` cell and its spacer (2 rows), and the strip loses the
   live block (up to 4 rows plus its gap).
2. `ToolCallDelta` × n — counted, and previewed only for a `write`
   (`docs/write-streaming.md`). For anything else the preview slot is empty.
3. `ToolBatch` — the call's cell goes up (2+ rows plus its gap).

The live region is content-anchored (CLAUDE.md invariant 3): its top stays
put, a commit pushes it down by the rows it committed, and a shrink leaves
the rows it vacated blank below it. Step 1 gives back up to five rows while
committing two, so the box rises three. Step 3 takes them back. How long the
box stays up depends on how long step 2 takes. For a short `skill` call it
is often under one frame, so you never see it. For a long `bash` command,
or with network jitter, it lasts a frame or two. The offline dummy's default
turn generates its three calls for ~0.8 s, so there the rise holds still
long enough to measure.

## The rule

> When a thought settles and the slot has nothing else to show, the slot
> keeps the rows the thought gave back (the slot the last frame painted,
> less the rows committed since) as blank rows, until the call's cell, a
> reply or a new thought lands on them.

Everything in that sentence is a limit, and each one is there because the
first version of this fix went without it.

**Only across a settled thought.** The first version held *every* live cell
that gave way to a shorter one: a command's tailing cell (four rows, the
`+N lines (Ns · wait …)` clock, the ctrl+b hint) settling into its three-row
peek, a read group's two-row cell settling into its line, a thought's block
giving way to a reply's first line. When the command was the last call of
its round, nothing came to land on the held rows until the next request
answered. That can take seconds, and the screen showed three blank rows
between the cell and the status line where there should be one (the second
report). A thought is different: it settles mid-stream, and the call it led
to arrives at the end of that same stream.

**Only an empty slot.** A held row is never painted under content of its
own. A reply's first line or a read group's next call takes the slot whole,
and if it is shorter than the block it replaced, the box shifts once. That
is a single move, not a rise followed by a fall, so it does not flicker.
Holding it instead would put a blank row between that content and the
status line.

**Until something lands.** The cell, a reply line, a new thought or anything
else the conversation records ends the hold. A turn's end ends it too:
`StreamDone`, an error and an interrupt clear the status line before the
boundary reseats to the idle height, and the collapse to the idle composer
is the one shrink nothing resists.

## The pieces

**Pure (`ui::layout`, `ui::live`, `app`).**

- `ui::StripHold` — what the boundary keeps between two inline draws: the
  slot (preview rows plus gap) the last frame painted, and whether that frame
  showed the live thinking block or was already holding its rows.
  `hold(app, committed, width)` returns the hold for the next frame. It is
  that slot less the rows committed since, but only when the hold carries
  over and the strip is waiting on the thought's cell: a status line is up,
  no modal covers the strip, the newest thing the conversation on screen
  recorded is that thought (the viewed agent's history in its session view),
  and the slot is empty. Otherwise it returns 0. `record(app, width, height)`
  notes the frame just painted, and `reset()` forgets it.
- `App::set_strip_hold(rows)` / `App::strip_hold()` — the injected hold, the
  `set_stream_preview_rows` pattern.
- `preview_rows` is the slot's own content (`content_preview_rows`, the old
  walk) when there is any, else `held_preview_rows`. That function turns the
  hold into content rows (at least one, so the gap comes back with it) and
  returns nothing without a status line, under a modal, or when the slot has
  content. Every geometry helper sizes from `preview_rows`
  (`fitted_preview_rows`, `live_height`, `live_layout`, `input_box`,
  `cursor_position`, `strip_above_rows`), so the held rows are reserved
  everywhere at once.
- `preview_lines` pads an **empty** slot with blank rows up to the hold,
  within the rows it was given. The strip's `debug_assert` that reserved ==
  drawn holds by construction.
- `strip_content_preview_rows` builds live cells at their own full ask
  (`content_preview_rows`), and counts held rows only as far as
  `fitted_preview_rows` allows. The strip flow (`docs/strip-flow.md`) can
  never push a cell's header into scrollback ahead of blank rows.

**Boundary (`tui::view`).**

- `draw_conversation` injects `strip_hold.hold(&app, term.pending_rows(),
  width)` before anything sizes the frame. The pending rows are exactly what
  this frame's flush will commit above the region. After the draw it calls
  `strip_hold.record(…)`.
- `repaint_conversation` and `repaint_agent_view` (the purge rebuilds) reset
  the tracker and drop the hold to 0 first. A rebuild reseats the whole
  screen, so there is no box position to keep.

An overlay return is an ordinary draw. Everything committed under the
overlay is pending, so the hold comes out as the old slot less all of it.
The agent session view goes through the same draw, so a subagent's
thought → call holds the same way.

## What it does not touch

- **A round's end.** A call that settles into a shorter cell, with the next
  request still out, leaves the status line one spacer under the cell. The
  box moves once.
- **The rows around the slot.** The status line's hanging rows (the tip,
  the checklist, the offline row), the queued messages and the toast are
  decided by state the frame cannot negotiate with.
- **A reply's own floor** (`StreamRender::preview`, `docs/slow-stream.md`),
  which keeps a streaming reply's frontier from shrinking between tokens.
- **The turn-end reseat** (`StreamDone`'s `set_view_height`), which runs
  after the status line is gone.

## Alternatives weighed

- **Hold every hand-off** — the first version, retired by the second report
  for the reason under *The rule*.
- **Keep the `● Thinking…` block up until the next cell arrives**, settling
  both in one frame. The block would claim the model is still thinking
  while it is writing a call (the status line's `Thinking for` clause has
  already stopped), and a frame drawn between `ThinkingEnd` and the first
  argument piece would still collapse it.
- **Show a call's cell from its first `ToolCallDelta`.** Claude Code
  streams tool use this way. But a call's header summary needs its
  arguments, so the header would re-wrap as they arrived, and the height
  changes would come back in another form. `write` already previews because
  its content is the point (`docs/write-streaming.md`).
- **A time-limited hold.** Releasing held rows after ~1 s of nothing would
  make the box jump on its own clock with nothing new on screen.

## Testing

- `ui::tests::strip_hold` drives the boundary's two calls frame by frame
  through the reported round: think, settle, the call being written (its
  `skill` pieces streaming into the drafts), the batch, the answer, the
  next thought. Every frame's rise must be zero. The held rows are blank,
  and the next cell opens the slot. The tests also pin what is not held: a
  round ending on a shorter cell (the second report), a call that landed on
  the hold and then settled, a reply's first line and a read group's next
  call. An agent tree settles without a rise. Every committed row releases a
  held one, the hold ends with the turn, it is fitted on a short terminal,
  and a modal frame paints no slot.
- `scripts/smoke.sh` **Phase 153** samples the footer row through the dummy's
  default turn (text, a thought, three calls generated for ~0.8 s, the batch)
  on a 100×40 terminal and an 80×16 one. It then drives the real backend
  against a paced stub provider: a three-line thought, a `bash` call written
  over about a second, a command whose tailing cell is taller than its peek,
  and three seconds before the second round. It counts every drop while the
  status line is up, confirmed by a second capture, and it measures the
  blank rows between the settled peek and the status line during that wait.
  The pre-change binary rises at the thought (20 → 18, 16 → 14 off the
  bottom row, 17 → 14 through the real backend). The first version of the
  fix leaves three blank rows at the round boundary. This one leaves one.
- Live: replaying the recorded byte stream (one frame per `ESC[?2026l`) found
  one rise in the pre-change recording of the reported prompt
  (`#140, 29 → 27`). Two runs of the narrowed rule found none, with one blank
  row over the status line in every frame but the ~10 ms the thought's rows
  were held for the `Skill` cell.
