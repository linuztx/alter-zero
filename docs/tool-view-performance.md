# Ctrl+O performance: the incremental transcript cache and the atomic switch

## The problem

Opening the Ctrl+O transcript overlay on a **big resumed session** was slow and
ugly. Loading a real 146 KB rollout (a session whose `write` tools carried
15–20 KB HTML files) and pressing Ctrl+O:

- The overlay took ~270 ms to appear (measured in tmux at 190×45; worse on
  slower machines) — `ui::transcript_build` walks *all* of `history` and
  renders every tool cell **expanded**, which means grammar-highlighting every
  written file through syntect. Five `Write` cells accounted for ~180 ms of it
  (HTML with embedded CSS/JS is syntect's slowest grammar). The build re-ran
  **on every open**, because the cache was freed when the overlay closed.
- The build ran **after** the terminal had already switched: `enter_overlay`
  flushed cursor-hide + `EnterAlternateScreen` + clear, and only then did
  `draw_tool_view` start the O(history) build. The user stared at a **blank
  alternate screen** for the whole build — and in kitty, the buffer switch
  moves the cursor, so kitty's cursor-trail animation visibly streaked up the
  empty screen before the overlay appeared.

## The fix, in two halves

### 1. The switch is atomic (`term::enter_overlay` queues, never flushes)

`enter_overlay` now only **queues** `Hide` + `EnterAlternateScreen` + `Clear`
on the backend and returns without flushing. Every caller paints the overlay
immediately after (`draw_tool_view` / `draw_context_view` /
`draw_resume_picker`), and `draw_overlay`'s flush delivers the switch and the
fully-painted first frame as **one write**: the terminal hops from the live
inline frame straight to the finished overlay. There is no flushed
blank-alt-screen state anymore — nothing for a cursor-trail animation to
streak across, no black flash — and if the first frame is ever slow to build,
the stall happens *before* any terminal write, with the inline view (composer,
footer, cursor) still intact on screen.

The cursor `Hide` leads the queued switch, so no flushed state ever shows the
cursor mid-hop (`draw_overlay` re-asserts the hide inside its synchronized
update; the inline reflow re-seats and re-shows it on the prompt after
`exit_overlay` — the term.rs module docs describe that discipline).

Verified A/B in tmux with the real session (old → new): overlay visible in
273 ms → 14 ms (capture-poll floor), and a capture taken immediately after the
keypress shows a blank screen on the old build vs the already-painted overlay
on the new one.

### 2. The transcript build is incremental (`ui::TranscriptCache`)

The signature-guarded full rebuild is replaced by a **frozen-prefix**
incremental build (the `StreamRender` pattern, applied to the overlay):

- **Per-item render.** `transcript_item_lines` renders one history item —
  message + timestamp stamp, or the tool's expanded cell, or a
  summary/background notice — plus its trailing spacer (skipped after a shell
  header, whose tool cell sits flush), and reports the user-message row span
  the backtrack preview may reverse. `transcript_build` itself is now a thin
  "fresh cache, refreshed once" wrapper, so the cached and from-scratch
  renders share one code path and can never drift.
- **Frozen prefix.** `lines[..frozen_rows]` holds the banner chrome plus every
  committed item, each rendered exactly once. Soundness comes from
  `App::history_generation`: committed items are immutable and history only
  ever grows, and every non-append mutation — `/clear`, a `/resume` load, a
  backtrack truncation, an interrupt-undo pop — bumps the generation. The
  cache pins its prefix on `(generation, width, session cwd)`; the volatile
  `TranscriptSig` also carries the generation, because a pop + re-push can
  land history back on the *same length* with different content (the
  `transcript_cache_rebuilds_when_history_is_replaced_at_the_same_length`
  test).
- **Refresh = truncate + append.** A refresh drops the volatile tail
  (in-progress reply, live tool queue, queued backlog) off the end, appends
  any newly committed items to the prefix, and rebuilds just the tail. A
  streamed chunk re-renders only the tail; a scroll (unchanged signature)
  renders nothing at all.
- **The backtrack highlight is a style diff.** The Esc-Esc preview's REVERSED
  rows live *inside* the frozen prefix; stepping the selection un-reverses the
  old span and reverses the new one in place (tracked per-item row offsets),
  byte-identical to what a from-scratch build styles — never a re-render.
- **Retained across closes.** The overlay exit no longer frees the cache: the
  frozen prefix is exactly what makes the next open instant. Memory is the
  rendered transcript of the current conversation (the same order as `history`
  itself); a generation bump drops it wholesale.

### 3. The cache is pre-warmed at the boundary (`TranscriptCache::warm`)

Rendering an item costs the same wherever it happens — so it happens where the
user can't feel it. The loop bottom (beside `recorder.sync`) calls
`transcript.warm(&app, width)` every iteration:

- nothing committed → a few integer compares;
- a turn committed items → one grammar-highlight per new item, amortized into
  the turn that produced them;
- a `/resume` load swapped history (a generation bump) → the whole loaded
  session renders once, inside the load's already-heavy moment (the same
  iteration purge-repaints the inline view), ~240 ms for the 146 KB session.

So the Ctrl+O keypress itself is always O(live tail) — ~1 µs after a warm on
the measured session — and the overlay never opens cold.

**Width is the one deliberate exception**: `warm` skips a *width-only*
mismatch. Resize events arrive in bursts (a drag delivers dozens), and a full
O(history) re-render per event would freeze the loop; the first overlay draw
at the new width pays the one rebuild instead — behind the still-painted
inline screen, thanks to half 1. A generation change re-warms at the current
width, so the stale-width state can't persist past the next real mutation
either.

## What guards it

- `app::tests::history_generation_bumps_on_every_non_append_mutation` — the
  soundness invariant of the frozen prefix.
- `ui::tests::transcript_cache_appends_new_items_without_rerendering_frozen_ones`,
  `…_streams_a_reply_without_rerendering_history`,
  `…_backtrack_selection_restyles_without_rerendering`,
  `…_rebuilds_when_history_is_replaced_at_the_same_length`,
  `…_warm_prebuilds_so_the_open_renders_nothing` — the incremental behaviour,
  each asserting byte-equality with a from-scratch build (`item_renders` /
  `builds` are the test-only counters proving reuse).
- The pre-existing transcript tests (interleaving, live tail, queued backlog,
  placeholder, stamps, selection) all run through the same single code path.
- `scripts/smoke.sh` Phase 48 — resume a code-heavy session in tmux, Ctrl+O,
  assert the expanded transcript appears promptly and the inline view returns
  intact (the boundary halves: the queued switch and the loop-bottom warm).
