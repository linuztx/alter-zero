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

`term::overlay_updates` — pure, unit-tested — answers "what must actually go on
the wire?" given the frame already on the alternate screen and the one being
drawn:

- `None` — no usable baseline, paint every cell (the first frame after
  `enter_overlay`'s clear, a resize, or a write that failed part-way);
- `Some(updates)` — the cell diff;
- `Some(empty)` — **nothing changed**.

`draw_overlay` returns immediately on the empty case, before it queues anything:
no cells, and no `BeginSynchronizedUpdate` / `Hide` / cursor-move escapes either.
A still page is silence on the wire, which is the whole point — a terminal
holding a selection over it sees no write at all.

The baseline lives in its own field, `InlineViewport::overlay_prev`. It cannot
share `prev`: the two describe different screens at different areas, and the
overlay entry/exit paths clear `prev` by design. It is dropped on every entry
(the queued `Clear(All)` blanks the screen), every exit (the screen is gone),
and any frame whose write failed part-way (the screen is then in a state no
buffer describes — better to repaint in full than to diff against a fiction).

`Buffer::diff` skips wide-glyph shadow cells itself, which is the same rule
`visible_cells` enforces on the full-paint path, so the diff path inherits the
table-tearing fix rather than reopening it (`docs/table-streaming.md` *Wide
glyphs*).

### 2. The clock chain stops under an overlay

`App::wants_animation_frames` — pure, unit-tested — is the draw tick's re-arm
condition, and it is now `false` whenever `View::is_overlay()`. Nothing goes
stale: every event source (`stream`, `agent`, `background`, `mcp`, the workers,
every keypress) already ends in its own `schedule_frame()`, so a page that has
something new to show still gets a frame the moment it does. The chain re-seeds
on the first draw after the return.

Note the ordering: with (1) in place, (2) is not what makes copying work — a
re-armed frame over an unchanged page now writes nothing anyway. (2) is what
stops the app *building* that frame: a full-screen `Buffer`, the render closure,
and on the Ctrl+D classifier page a backend mutex and a fresh `String` render,
31 times a second for a screen nobody is looking at.

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
| `overlay_updates` — diff or full paint | `src/term.rs` | **pure** |
| `overlay_prev` baseline, `draw_overlay` | `src/term.rs` | boundary |
| `View::is_overlay` | `src/app/types.rs` | **pure** |
| `App::wants_animation_frames` | `src/app/views.rs` | **pure** |
| the draw tick's re-arm | `src/tui/view.rs` | boundary |
| byte-level proof | `scripts/smoke.sh` Phase 94 | smoke |
