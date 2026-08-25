# Overlay repaint — copying text out of Ctrl+O / Ctrl+D while a turn runs

## The bug

Open the Ctrl+O transcript (or the Ctrl+D context view) while the agent is
generating, try to select some text with the mouse, and the selection dies
under your cursor. You have to wait for the turn to finish before you can copy
anything out of the full-screen views — which is precisely when you least want
to wait, since the reply you wanted to keep is the one still streaming.

Nothing about the *selection* was broken. Terminals drop a mouse selection the
moment the cells under it are rewritten — that is how xterm, VTE, kitty, iTerm2
and tmux all behave, and it is the right behaviour: the highlighted text is no
longer the text that is there. The overlay was simply rewriting every cell of
the screen, over and over, whether or not anything had changed.

## Why it rewrote everything

Two independent things drove it.

**`draw_overlay` never diffed.** The inline live region has always diffed:
`paint_frame` keeps the previous frame in `InlineViewport::prev` and emits only
`prev.diff(buf)`. The alternate-screen path had no such baseline — `prev` is
deliberately `None` while an overlay is up, since it describes a different
screen — so every overlay frame built a fresh `Buffer` and pushed **every cell**
of it through `draw_cells`. On a 100×30 pane that is 3 000 cells, SGR runs and
all, per frame.

**Frames arrived constantly.** Two sources, neither of which looked at the view:

- every reply event ends in `schedule_frame()` (`src/tui/stream.rs`), clamped
  only by the 120 fps `MIN_FRAME_INTERVAL` — so a fast text stream drove frames
  at up to 120 Hz;
- the draw tick re-armed a 32 ms animation frame whenever a turn was active
  (`src/tui/view.rs`), which kept ~31 Hz flowing even through an event-less
  pause like a tool run.

That chain exists for the **inline** live region — the spinner's shimmer, the
elapsed counter, the pulsing tool bullet, a background shell's ticking runtime,
the agent roster's counters and linger sweep. None of it is on screen under an
overlay. So the app was repainting a full screen ~31 to 120 times a second to
animate things nobody could see, and taking the user's selection with it each
time.

The Ctrl+D context page is the purest form of it: `ContextSig` carries no
streaming state at all, so its *content* is provably unchanged for the whole
turn. It was the identical screen, re-serialized thousands of times over a long turn.

## The fix

Two changes, both small, in the two places named above.

### 1. The overlay diffs, and an unchanged frame writes nothing

**This change alone is the fix.** Everything in §2 is a separate saving; if you
only ever read one section, read this one.

`term::overlay_paint` — pure, unit-tested — turns the frame already on the
alternate screen and the one being drawn into a verdict:

- `Unchanged` — nothing moved;
- `Diff(updates)` — only these cells differ;
- `Full` — no comparable frame on screen, paint every cell.

`draw_overlay` returns on `Unchanged` **before it touches the backend at all**:
no cells, and no `BeginSynchronizedUpdate` / `Hide` / cursor-move escapes
either. A still page is silence on the wire, which is the whole point — a
terminal holding a selection over it sees no write.

The baseline lives in its own field, `InlineViewport::overlay_prev`. It cannot
share `prev`: the two describe different screens at different areas, and the
overlay entry/exit paths clear `prev` by design. It is dropped whenever the
alternate screen stops being what it records:

- `enter_overlay` — the queued `Clear(All)` blanks the screen;
- `exit_overlay` — the screen is gone, and a re-entry clears it anyway;
- `resized` — the emulator reflowed it out from under us;
- a frame whose write failed part-way — the screen is then in a state no buffer
  describes, so repaint in full rather than diff against a fiction.

The `resized` drop is not redundant with `overlay_paint`'s area check, and it is
deliberately unconditional (`changed` or not). A resize burst can coalesce into
a single frame — `on_resize` asks for one, but the scheduler's floor is 8 ms —
so the net size can land back on exactly the area the baseline records while the
terminal clipped and regrew the alternate screen in between. Diffing against
that baseline leaves the vacated rows stale until the overlay is closed and
reopened. Dropping it costs one full repaint on a resize, which is happening
anyway.

`Buffer::diff` skips wide-glyph shadow cells itself, which is the same rule
`visible_cells` enforces on the full-paint path, so the diff path inherits the
table-tearing fix rather than reopening it (`docs/table-streaming.md` *Wide
glyphs*).

### 2. The clock chain stops under an overlay — a CPU saving, not part of the fix

Be clear about what this is and is not. With §1 in place a re-armed frame over
an unchanged page already writes nothing, so **this buys no additional
copyability at all**. What it buys is the work: without it the app still
*builds* that frame 31 times a second for a screen nobody is looking at — a
full-screen `Buffer` allocation, the render closure, a diff over every cell, and
on the Ctrl+D classifier page a backend mutex and a fresh `String` render — to
conclude `Unchanged` every single time.

`App::wants_animation_frames` — pure, unit-tested — is the draw tick's re-arm
condition, and it is now `false` whenever `View::is_overlay()`.

Nothing an overlay shows goes stale, because nothing an overlay shows is
time-driven. That is not an accident of the current code, it is stated in it:
`tool_full_body` opens with `let pulse: Option<Duration> = None;` over a comment
explaining that animating in the transcript "would either not move or cost a
full-tail re-render every 32 ms", `TranscriptSig` carries no clock field, and
the two renderers that *do* take an elapsed — `shell_running_line` and
`running_command_lines` — are called only from `src/ui/live.rs`, the inline
strip. Content changes still arrive as events, and every event source
(`stream`, `agent`, `background`, `mcp`, the workers, every keypress) ends in
its own `schedule_frame()`. The chain re-seeds on the first draw after the
return, because `after_key` → `schedule_for_key` fires on the very keypress
that closes the overlay.

What *does* pause, honestly: the agent roster's runtime counters and its linger
sweep (`tick_agent_roster`), which only the draw tick drives. Both are derived
from absolute `Instant`s and recomputed rather than accumulated — the runtime is
`started.elapsed()`, the sweep compares stored deadlines against `now` — so the
first frame after the return produces exactly the values it would have anyway.
They are also in the footer, invisible under an overlay. Nothing is lost, only
deferred while it cannot be seen. (Toast expiry is *not* affected:
`expire_toast` schedules its own frame independently of this predicate.)

The real cost is a coupling: if a clock is ever added to something an overlay
renders, it will freeze here rather than tick. The transcript is cached behind a
clock-free signature so a change there would have to defeat `TranscriptSig`
first and would be noticed, but `agent_transcript_lines` is built fresh per draw
and has no such tripwire. Anyone adding a live counter to an overlay body needs
to revisit this predicate.

## What it measures

`tmux pipe-pane` captures every byte the app writes to its pane. Driving a
deterministic 30-second turn (`!sleep 30`) at 100×30 and measuring a three-second
window in the middle of it:

| view | before | after |
| --- | ---: | ---: |
| inline conversation (control) | 4 533 B | 4 533 B |
| Ctrl+D context view | 316 217 B | **0 B** |
| Ctrl+O transcript | 389 019 B | **0 B** |
| Ctrl+O scrolled up (Home) | 380 277 B | **0 B** |

The inline control is unchanged, as it must be — its status line really is
animating, and those 4.5 KB are the shimmer and the timer doing their job.

The same measurement against a **real backend** — `openai/gpt-4o-mini` over
OpenRouter, driving the real agentic tool loop with a `bash sleep 25` call, the
tool asserted still running at the end of the window:

| view | before | after |
| --- | ---: | ---: |
| inline conversation (control) | 20 562 B | 20 700 B |
| Ctrl+O transcript | 550 970 B | **0 B** |
| Ctrl+D context view | 420 198 B | **0 B** |

Roughly 138 KB a second of screen rewriting, over a page that had not changed,
for as long as the turn lasted.

## What this does and does not give you

**Static page, zero writes.** The Ctrl+D context view does not change while a
turn streams (`ContextSig` keys only on committed history), so it is now
completely still for the whole turn: select and copy freely.

**Scrolled back in Ctrl+O, only what moved is rewritten.** Pressing ↑ / PageUp /
Home disengages tail-follow (`App::tool_follow`) and `settle_tool_scroll` then
holds the offset, so the window stops chasing the frontier. What reaches the
terminal is the diff and nothing else. Scrolled to the top of a transcript
longer than the screen — the frontier off-screen entirely — that is **zero
bytes**, measured. Scrolled to a spot where some of the growing text is still
visible, it is a few hundred bytes a second of *exactly those cells*: captured
mid-stream at the top of a short transcript, the whole 2 s of traffic is the
reply's own words landing one at a time on the one row they belong to
(`ESC[11;7Hmodel`, `ESC[11;13Hthat`, …). Every other row on the screen is left
alone, so a selection anywhere but on the line that is actively growing
survives.

**Pinned to the bottom in Ctrl+O, the body still moves.** Tail-follow is doing
exactly what it is for: new content arrives and the window scrolls to keep the
frontier in view. A selection over text that scrolls out from under it cannot
survive that, in this or any pager, and freezing the follow silently would be a
worse trade — you would be reading a stale page without being told. Even here
the diff helps, since a chunk that only extends the last line rewrites only that
line; it is a new *row* that shifts the window and costs the selection. Scroll
up to read back — what the footer's `↑/↓ to scroll` line already invites — and
the page holds still.

## What Phase 94 checks

Two different failures, neither of which subsumes the other:

- **Did it actually go quiet?** `tmux pipe-pane` captures every byte the app
  writes to the pane over two seconds of a turn held active by a 30-second
  `!sleep`. Ctrl+O and Ctrl+D must write exactly zero — and the inline control
  is asserted to write *something* first, so a zero can never come from a broken
  meter. Then that the clock chain re-seeds after the return, since a chain left
  broken would freeze the inline timer for the rest of the turn.
- **Did it paint the right cells?** Bytes say nothing about correctness. So a
  whole turn streams under the overlay (hundreds of diff frames), the view
  scrolls away from the tail and back, resizes away and straight back, and then
  the overlay is closed and reopened — `enter_overlay` clears the alternate
  screen, so that frame is a pure full repaint of the same content at the same
  seat. The two screens must be identical. A diff that dropped an update, or
  left a cell stale after a resize, fails here and nowhere else.

## Invariants this touches

Invariant 4 (`CLAUDE.md`) is unchanged in substance and slightly stronger in
practice: commits made under an overlay still only *queue* — `insert_before`
does no I/O and `draw_overlay` never flushes the queue — and the return's
ordinary draw still flushes the whole backlog above the live region. The overlay
now simply refuses to write cells it does not need to, which is the alternate
screen finally getting the rule `paint_frame` has always followed inline.

## Where it lives

| piece | file | kind |
| --- | --- | --- |
| `overlay_paint` → `OverlayPaint` | `src/term.rs` | **pure** |
| `overlay_prev` baseline, `draw_overlay`, `resized` | `src/term.rs` | boundary |
| `View::is_overlay` | `src/app/types.rs` | **pure** |
| `App::wants_animation_frames` | `src/app/views.rs` | **pure** |
| the draw tick's re-arm | `src/tui/view.rs` | boundary |
| byte-level proof + stale-cell check | `scripts/smoke.sh` Phase 94 | smoke |
