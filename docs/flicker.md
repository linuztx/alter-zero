# Flicker-free rendering — scrollback commits join the draw frame

Date: 2026-06-11

## The symptom

While a reply streams, the live region (input box + footer, plus the streaming
strip) **blinks**: every time a finished line commits to scrollback the box
vanishes for a few milliseconds and reappears on the next draw tick. A byte
trace of one pre-fix streaming turn shows why: **14 of 14** live-region clears
(`ESC[J`) were written *outside* any synchronized-update block, each followed
by its box repaint on a separate, later flush — a presentable boxless state
every time a line committed; at a 60 Hz terminal refresh that is a visible
blink every couple of seconds.

## Diagnosis — three windows where a flushed frame lacks the live region

The project already had two anti-flicker layers: `term::draw` brackets the
live-region repaint in a synchronized update (BSU/ESU, DEC mode 2026), and a
kept `prev` buffer diffs still frames so a keystroke ships a couple of cells.
Both only cover the *draw* path. The gaps:

1. **Streaming commits.** `insert_before` performed its terminal I/O
   immediately, outside any synchronized update: scroll the screen up, draw the
   committed lines, then **clear the live region** (`ClearType::AfterCursor`
   from the viewport top — the box, queue rows, and footer all blank), each
   step flushed. The box repaint then waited for the next coalesced draw tick
   (0–8.3 ms at the 120 fps cap, plus terminal refresh). Every committed line
   blinked the region; the mid-insert states (screen scrolled, lines not yet
   drawn) could tear; and the hardware cursor — never hidden, and outside any
   BSU — visibly hopped around the screen as the insert wrote.
2. **Reflow** (resize, Ctrl+O return, `/clear`): same shape — `reflow` rewrote
   the screen and left the live region cleared, the box repainted a tick later.
3. **Ctrl+O enter**: `enter_overlay` cleared the alternate screen and the first
   overlay paint came a tick later — a black flash.

## What codex does (ported from `/tmp/codex/codex-rs/tui`)

- `Tui::insert_history_lines` does **no terminal I/O**: it appends to
  `pending_history_lines` and schedules a frame (`tui.rs`).
- `Tui::draw` performs *everything* inside **one** synchronized update
  (`stdout().sync_update(..)` — crossterm's `SynchronizedUpdate`, the same
  BSU/ESU pair): re-pin the viewport, `flush_pending_history_lines` (the real
  scrollback writes, via `insert_history.rs`), then the live-region
  `terminal.draw`. One atomic frame — the terminal never displays the
  "history written, box missing" intermediate state, and the cursor's travels
  during insertion are invisible (`insert_history_lines` is deliberately
  "cursor-position-neutral").
- The synchronized update also makes the cursor handling safe: nothing needs
  to be hidden/shown around the writes because no intermediate state is ever
  presented.

## The port

`term::InlineViewport` adopts codex's pending-lines model; the public call
shape in `main.rs` barely changes.

- **`insert_before(lines)` queues.** It appends to a new
  `InlineViewport::pending` buffer and returns — infallible now (the I/O moved
  into the frame). Every existing caller already schedules a frame after it.
  The old body (ratatui's portable insert-before scroll math, with the
  tmux-safe draw-then-clear ordering) survives as the private `write_above`.
- **`draw` flushes the queue inside its frame.** The existing BSU/ESU bracket
  now contains: `flush_pending` (write the queued lines above the viewport →
  the region on screen is cleared), then re-pin, render, and repaint the live
  region, then place the cursor — one flush at the end. Scrollback growth and
  the box repaint land as a single atomic update; there is no longer any
  flushed state without the box. Frames with nothing pending keep the `prev`
  diff fast path (a keystroke still ships only changed cells).
- **`reflow` paints the live region too.** It takes the render closure + `App`
  (like `draw`) and brackets the whole rebuild — the empty-tail clear, seating
  the viewport at the top, `write_above(tail)`, and the live-region paint at
  the final viewport — in one synchronized update. A resize repaint or Ctrl+O
  return is now a single atomic frame including the box. Stale `pending`
  lines are dropped first: everything queued is regenerated from `history`
  (the caller resets `committed`), so flushing them too would duplicate.
- **`restore` flushes leftovers.** A quit can land between queueing and the
  next tick (e.g. `/help` then an instant Ctrl+C); `restore` writes any
  pending lines before leaving raw mode so committed content is never lost.
- **Ctrl+O enter paints immediately.** The `ToggleToolView` arm draws the
  overlay right after `enter_overlay` instead of waiting for the tick, closing
  the black-flash window. (Exit was already followed by `repaint_conversation`,
  which now paints the box in the same frame via `reflow`.)
- `paint_frame` queues `Show` instead of ratatui's `show_cursor` (which
  `execute!`s — an extra mid-frame flush), so a frame is one write.

### Why deferral is safe (ordering)

- `set_view_height` mutates the *tracked* height immediately, so queued lines
  flush with the latest height — the `StreamDone` "reseat to idle before the
  final inserts" dance still works, and now applies to every line in the
  batch (strictly less over-scroll than inserting at the stale strip height).
- Pending lines only accumulate in the conversation view (the overlay defers
  commits by design — invariant 4), and only `draw`/`reflow`/`restore` flush
  them — never `draw_overlay` — so queued lines can never be written into the
  alternate screen.
- Pending lines are always the newest content, so `reflow`'s
  regenerate-from-history covers them; dropping the queue there loses nothing.
- Every queueing path schedules a frame already; `restore` is the backstop.

## Known divergences from codex

- Codex wraps pending lines itself (`HistoryLineWrapPolicy`, hyperlink-aware)
  and scrolls with margin regions where supported; our `write_above` keeps the
  simpler ratatui portable scroll math we already had.
- Codex *does* rebuild scrollback on resize — it purges the screen + scrollback
  and replays the transcript (`clear_terminal_for_resize_replay` +
  `reflow_transcript_now`), which our `ReflowClear::Purge` mode now mirrors (see
  `docs/design.md`). What has no codex counterpart is the **atomic-frame**
  treatment: our `reflow` is homegrown and wraps the purge + rebuild in one
  synchronized update, where codex reflows without that framing.
- No zellij special-casing (codex's `InsertHistoryMode::ZellijRaw`).

## Testing

`term.rs`/`main.rs` are the I/O boundary (no unit tests — `scripts/smoke.sh`).
A note on *how* this is testable: `tmux capture-pane` reads tmux's **grid**,
which mutates as bytes parse — synchronized updates protect what a terminal
*presents*, not what a mid-parse capture samples. So capture sampling cannot
distinguish a harmless in-frame transient from a genuinely flushed blank frame
(a hammer probe still catches ~0.4% boxless grid states post-fix). The guard
asserts at the **byte level** instead, where the property is exact:

- **Phase 15** records the raw output stream of a whole streaming turn
  (`tmux pipe-pane`) and walks it: every `ESC[J` live-region clear must sit
  between `ESC[?2026h` and `ESC[?2026l`, so a terminal honouring mode 2026
  can never present the boxless intermediate state. Deterministic in both
  directions — the pre-fix build recorded **14/14 clears outside** any sync
  block; the fix records **0 outside** (and fewer clears overall, since a
  frame batches its commits). The recording closes before the quit:
  `restore`'s teardown clear is legitimately unbracketed.
- All existing phases (streaming bottom-pin, resize, overlay round-trips,
  interrupt, queue flushes, `/clear`) exercise the deferred-flush ordering
  end to end — they prove the queued lines land, in order, with the box
  seated correctly.
