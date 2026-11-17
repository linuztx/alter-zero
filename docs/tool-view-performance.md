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

Hidden is not enough on every terminal, though: some animate the cursor's
**move** regardless of visibility, and a full-screen cell paint used to leave
the (hidden) cursor wherever the last cell landed — the blank bottom-right
corner — so opening Ctrl+O / Ctrl+D streaked the animation from the composer's
`❯` to nowhere (the reported bug). `draw_overlay` therefore ends every frame by
**seating** the cursor at the frame's last text — just past the closing
`q/esc/… to quit` hint — via the pure `ui::overlay_cursor_seat` (the last
non-blank glyph of the rendered buffer, wide-glyph aware, clamped inside the
frame), exactly as the inline `paint_frame` seats it on the prompt via
`ui::cursor_position` and as `cursor_visible`'s hidden-but-seated rule keeps a
menu's seat sensible. The seat is a property of the frame, not the scroll, so
paging the transcript never moves it — one jump in (onto the hint), one jump
back out (onto the prompt).

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
- **Frozen prefix.** The first `frozen_rows` cached rows hold the banner chrome plus every
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
  (in-progress reply, the open thinking phase — `docs/thinking-stream.md` —
  live tool queue, queued backlog) off the end, appends
  any newly committed items to the prefix, and rebuilds just the tail. A
  streamed chunk re-renders only the tail; a scroll (unchanged signature)
  renders nothing at all.
- **The backtrack highlight is a style diff.** The Esc-Esc preview's REVERSED
  rows live *inside* the frozen prefix; stepping the selection un-reverses the
  old span and reverses the new one in place (tracked per-item row offsets),
  byte-identical to what a from-scratch build styles — never a re-render.
- **Entries ride the same split.** The transcript's entries — where each
  cell opens and where its header ends, what ←/→ step between and the pin
  reads (`docs/transcript-navigation.md`) — are recorded by the walk that
  renders the rows: the frozen prefix's once per item, the tail's rebuilt
  with it (truncated to `frozen_entries` beside the rows), so a scroll
  touches neither.
- **Retained across closes.** The overlay exit no longer frees the cache: the
  frozen prefix is exactly what makes the next open instant. Memory is the
  compact rendered transcript of the current conversation (the same order as
  `history` itself); a generation bump drops it wholesale.

### Compact rows preserve the warm cache

Retaining expanded ratatui rows made each highlighted token a separate text
allocation plus a full `Style`. A code-heavy session kept thousands of those
objects even with the overlay closed. `ui::cached_lines::CachedLines` now packs
each rendered item's text into a shared UTF-8 slab and interns repeated styles.
Span lengths and style indexes use lossless base-128 `usize` encodings; row
offsets permit direct access to any viewport without scanning earlier rows.
Empty spans, row alignment, line styles and invisible image/link carriers all
round-trip exactly. Backtrack selection changes the row's style index.

Rows and spans count their references to interned styles. Removing the live
tail or changing selection releases unused trailing style entries, so repeated
refreshes do not accumulate unique link/image carriers. Unused middle entries
keep their IDs while later styles remain in use; buffer capacities are retained
for reuse. This adds one reference count per interned style, not per span.

`TranscriptCache::render` expands only the visible window, borrowing its text
from the slab. Opening, scrolling and reopening keep the same pre-warmed
prefix and require no highlighting. The public `lines` accessor still serves
callers that explicitly need a complete owned transcript; the interactive
pager never materializes it and drops an earlier compatibility view when it
paints. A new commit or cache invalidation drops that owned view too.

Rebuilding the transcript on demand would save closed-overlay storage but
restore the slow first open described above. Keeping compact rendered rows
preserves that performance guarantee. An item's rows are written straight
into the compact store as they are rendered (`transcript_item_into`, an
assistant reply's a line at a time), so not even one enormous item is first
built whole as expanded rows (`docs/input-latency.md` *Rows go straight onto
the page*).

### 3. The cache is pre-warmed at the boundary, off the loop thread

Rendering an item costs the same wherever it happens — so it happens where the
user can't feel it. That used to mean the loop bottom: `transcript.warm(&app,
width)` every iteration, one grammar-highlight per new item, amortized into
the turn that produced it. It was felt after all. The loop bottom sits
between two keystrokes, and a 2000-line `read` put 216 ms there, a long code
reply most of a second (`docs/input-latency.md`).

The loop bottom now only **hands the work out**
(`TranscriptCache::warm_jobs`, beside `recorder.sync`):

- nothing committed → a few integer compares;
- a turn committed items → a `TranscriptJob` per new item, for the worker
  thread `tui::workers::spawn_transcript_worker` runs for the session's life;
- a `/resume` load swapped history (a generation bump) → the whole loaded
  session, a window of eight items at a time.

Each job renders its item into compact rows on the worker, and the loop
splices them onto the frozen prefix when they come back
(`TranscriptCache::accept`) — a copy of some bytes. A result is taken only as
the very next item of the prefix as it stands: every reset of the prefix is a
new epoch, a job is of the epoch it was handed out in, and anything else is
refused.

So the Ctrl+O keypress itself is still O(live tail), the overlay still never
opens cold, and the cost of that no longer falls on the loop. Pressed before
the worker has caught up — right behind a long reply, or a big `/resume` —
the open renders what is missing itself, as it always did, and the worker's
copies of those items are refused when they arrive.

The synchronous `TranscriptCache::warm` remains for a caller with no worker
(the memory gate in `tests/transcript_memory.rs` drives it). It keeps its one
deliberate exception — it skips a *width-only* mismatch, since resize events
arrive in bursts and a full O(history) re-render per event would freeze its
caller. `warm_jobs` has no such exception: re-keying costs a few rows of
banner and the rendering is the worker's, so after a resize the pager is
warm at the new width too, where it used to rebuild the whole history on the
loop at the next Ctrl+O.

### 4. The live tail renders a line once

The frozen prefix was never the whole page: the reply still streaming is the
live tail, rebuilt on every refresh. It was rebuilt by rendering the reply
whole, so under an open pager a 60 KB code reply cost the grammar over all
of it per frame and a scroll key waited ~150 ms. The cache now keeps an
incremental render of the live reply (`LiveReply`, a `StreamRender` of its
own): a refresh renders the lines that completed since the last, and the
rows of finished lines stay on the page through the tail's rebuild
(`docs/input-latency.md` *The pager rendered a streaming reply whole*).

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
- `ui::tests::transcript::a_job_renders_an_item_exactly_as_the_cache_would`
  and its siblings — the job protocol: the same rows, handed out once and a
  window at a time, refused after a reset, out of turn, or once a draw has
  rendered the item itself; and the live reply rendered a line once
  (`docs/input-latency.md`).
- The pre-existing transcript tests (interleaving, live tail, queued backlog,
  placeholder, stamps, selection) all run through the same single code path.
- `scripts/smoke.sh` Phase 48 — resume a code-heavy session in tmux, Ctrl+O,
  assert the expanded transcript appears promptly and the inline view returns
  intact (the boundary halves: the queued switch and the loop-bottom jobs).

## What this doc does *not* cover: the paint

Everything above is about how fast the transcript's **rows** are built. What
happens to them afterwards — how many of those cells actually reach the
terminal — is a separate mechanism, and for a long time it undid much of the
saving: `draw_overlay` re-serialized every cell of the alternate screen on every
frame, so a cache hit that rendered nothing new still wrote a full screen. Worse
than wasted work, that write is what destroyed the user's mouse selection, which
made the overlay's text uncopyable while a turn streamed.

The overlay now diffs against the frame already on screen and an unchanged frame
writes nothing at all. The two mechanisms compose exactly as you would hope: a
cache hit produces a byte-identical buffer, which the diff then resolves to
`Unchanged`, which reaches the terminal as silence. See
`docs/overlay-repaint.md`.
