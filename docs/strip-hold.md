# The strip hold — the box never hops up mid-turn

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

The same shape appears wherever a live cell gives way to a shorter one with
more of the turn still to come: a thought followed by a reply whose first
line is a one-row preview; a running command whose tail (four rows, the
`+N lines (Ns · wait …)` clock and the ctrl+b hint) is taller than its
settled peek; and, with **Verbose output** off (the default), a collapsed
read group settling into its one line under the call behind it, a one-row
rise for ~0.4 s in the dummy's default turn (`docs/tool-groups.md`). A foreground agent tree does not have the problem. It settles
in place, each activity row becoming its `⎿ Done`, so it commits exactly
the rows it held.

## The rule

> While a turn's status line is up, the preview slot never gives back more
> rows than were committed since the last frame.

A committed row pushes the region down by exactly one, so a slot that
shrinks by no more than what committed keeps the box's bottom where it was.
A larger shrink is a rise. The rows it gives back beyond that are **held**:
padded as blank rows at the bottom of the preview, where the next cell lands
on them. It is the reply frontier's `preview_floor` (`docs/slow-stream.md`,
inside `StreamRender`) applied one level up, to everything the slot shows.

The hold ends with the turn. `StreamDone`, an error and an interrupt clear
the status line before the boundary reseats to the idle height, and the
collapse to the idle composer is the one shrink the hold never resists. That
is unchanged.

## The pieces

**Pure (`ui::layout`, `ui::live`, `app`).**

- `App::set_strip_hold(rows)` / `App::strip_hold()` — the injected hold, in
  slot rows (the preview plus its gap): the `set_stream_preview_rows`
  pattern.
- `ui::strip_hold(previous_slot, committed)` — the rule: what the last frame
  painted, less the rows committed since.
- `ui::strip_slot_rows(app, width, height)` — the slot a conversation frame
  paints. It is 0 when no status line is up, and 0 under a modal (the
  permission prompt and the ask modal replace the strip whole).
- `preview_rows` is now `content_preview_rows` (the old walk) floored at
  `held_preview_rows`. That function turns the hold into content rows (at
  least one, so the gap comes back with it) and returns nothing without a
  status line or under a modal. Every geometry helper sizes from
  `preview_rows`: `fitted_preview_rows`, `live_height`, `live_layout`,
  `input_box`, `cursor_position`, `strip_above_rows`. So the held rows are
  reserved everywhere at once.
- `preview_lines` pads its content (trimmed first, so a squeezed reply still
  tail-follows its content and not its padding) with blank rows up to the
  hold, within the rows it was given. The strip's `debug_assert` that
  reserved == drawn holds by construction.
- `strip_content_preview_rows` builds live cells at their own full ask
  (`content_preview_rows`), and counts the held rows only as far as
  `fitted_preview_rows` allows. The strip flow (`docs/strip-flow.md`) bottom-
  anchors an over-tall strip and flows its head into scrollback. Unfitted
  padding would have pushed a real cell's header out of the strip ahead of
  blank rows.

**Boundary (`tui::view`).**

- `draw_conversation` injects `ui::strip_hold(self.strip_slot,
  self.term.pending_rows())` before anything sizes the frame. The pending
  rows are exactly what this frame's flush will commit above the region.
  After the draw it records `ui::strip_slot_rows` into `Session::strip_slot`.
- `repaint_conversation` and `repaint_agent_view` (the purge rebuilds) drop
  the hold to 0 first. A rebuild reseats the whole screen, so there is no box
  position to keep. They record the slot they paint, so the next frame is
  measured from it.

An overlay return is an ordinary draw. Everything committed under the
overlay is pending, so the hold comes out as the old slot less all of it.
The agent session view goes through the same draw. Its strip previews the
viewed agent and `strip_has_status` follows that agent's status, so a
subagent's thought → call holds the same way.

## What it does not touch

- **The rows around the slot.** The status line's hanging rows (the tip,
  the checklist, the offline row), the queued messages and the toast are
  decided by state the frame cannot negotiate with, and are not held. A
  toast that expires mid-turn still gives its row back.
- **A composer-replacing view.** With `/model` or the ↓ band open mid-turn,
  the strip above the view uses the same held `preview_rows`, and the slot
  recorded for the next frame is the conversation path's own fit. That is
  never more than the content asked for, so a view closing mid-turn can at
  worst keep rows the composer would have shown anyway.
- **The turn-end reseat** (`StreamDone`'s `set_view_height`), which runs
  after the status line is gone.

## Alternatives weighed

- **Hold the thought until the next cell arrives.** Keep the `● Thinking…`
  block up past `ThinkingEnd` and settle it in the same frame the call's
  cell goes up. This would fix only this one transition. The block would
  also claim the model is still thinking while it is writing a call (the
  status line's `Thinking for` clause has already stopped). And a thought
  followed by reply text would still rise by a row, since a one-row reply
  preview is shorter than a four-row block.
- **Show a call's cell from its first `ToolCallDelta`.** Claude Code
  streams tool use this way. But a call's header summary needs its
  arguments, so the header would re-wrap as they arrived, and the height
  changes would come back in another form. `write` already previews because
  its content is the point (`docs/write-streaming.md`).
- **A time-limited hold.** Releasing held rows after ~1 s of nothing would
  make the box jump on its own clock with nothing new on screen, which is a
  movement nobody asked for. Held rows wait for content or for the turn to
  end.

## Testing

- `ui::tests::strip_hold` drives the boundary's two calls frame by frame
  through the reported round: think, settle, the call being written, the
  batch, the answer, the next thought. Every frame's rise must be zero. It
  also checks that the held rows are blank and that the next cell opens the
  slot. A thought → reply text keeps its rows under the reply's first line,
  and so do a running command settling into its shorter peek and a read
  group settling into its line. An agent tree settles without a rise. Every committed row releases a held one. The
  hold ends with the turn.
  Padding is fitted on a short terminal, and nothing flows. A modal frame
  paints no slot. Before the change: `the box rose between frames: [0, 3, 0,
  0, 0, 0, 1]`.
- `scripts/smoke.sh` **Phase 153** samples the footer row through the
  dummy's default turn (text, a thought, three calls generated for ~0.8 s,
  the batch), on a 100×40 terminal, an 80×16 one, and a 100×30 one with
  Verbose output off, where the turn's `Read` collapses into a read group.
  It counts every drop while the status line is up, confirmed by a second
  capture. The pre-change binary fails all three checks (row 20 → 18, 16 →
  14 off the bottom row, and 20 → 18 then 27 → 26 at the read group).
- Live: replaying the recorded byte stream (one frame per `ESC[?2026l`)
  found one rise in the pre-change recording of the reported prompt
  (`#140, 29 → 27`). Three runs after the change found none.
