# Async runtime rewrite (codex-style)

The event loop is built on **tokio** with a `select!` over five sources (input,
reply, draw ticks, `@` file-search results — `docs/file-search.md` — and
finished Ctrl+V clipboard reads — `docs/image-paste.md`), mirroring how
openai/codex drives its TUI. This replaces the previous synchronous
`event::poll` + drain loop. The *render* path (`term.rs`: diff + synchronized
update) is unchanged — only how input/stream/redraw are scheduled changed.

## Why

Codex achieves a very responsive TUI with: cell-diffing, synchronized frames,
a 120 fps frame scheduler that coalesces redraw requests, a paste-burst
detector, and round-robin fairness between input and draw. The first two were
already in this codebase; this change ports the rest.

> Note: for *typing latency* on this app the win is marginal (already ~1 ms,
> dominated by terminal I/O). This rewrite is for architectural fidelity to
> codex, chosen deliberately.

## Pieces

- **`frame.rs`** — `FrameRateLimiter` (`MIN_FRAME_INTERVAL = 8.333 ms` = 120 fps)
  with a pure `clamp_deadline` (never emit sooner than the floor past the last
  frame) and `soonest` (fold a new request into the pending deadline, keeping
  the earliest). A thin async `run_scheduler` task receives requested deadlines
  on an mpsc channel, coalesces + rate-limits them, sleeps until due, then sends
  one draw tick. `FrameRequester` is the cloneable send handle
  (`schedule_frame` / `schedule_frame_in`). Pure parts are unit-tested; the
  async task is covered by `smoke.sh`.

- **`paste.rs`** — `PasteBurst`, a pure state machine: characters arriving within
  `BURST_CHAR_INTERVAL` (8 ms) of each other are a *burst* once `BURST_MIN_CHARS`
  (3) accumulate. The loop uses this to coalesce a paste/fast-type run into a
  single scheduled frame instead of one per char. Unit-tested with injected
  `Instant`s.

- **`stream/`** — `StreamEvent` now travels a `tokio::sync::mpsc::Unbounded`
  channel. The backend still runs on a plain `std::thread` that *only sends*
  (tokio's unbounded `send` is sync, callable off-runtime), preserving "the
  backend never reads stdin".

- **`main.rs` + `src/tui/`** — `#[tokio::main(flavor = "current_thread")]`. After
  the (sync) viewport init queries the cursor, an `EventStream` becomes the sole
  stdin reader. The loop lives in `tui::event_loop::run`, one handler call per
  branch on the `Session` that holds the loop's state (`docs/module-layout.md`):

  ```text
  tokio::select! {
    Some(Ok(ev)) = events.next()      => on_key / resize → schedule_frame
    Some(se)     = reply_rx.recv()    => stream event   → (commit) + schedule_frame
    Some(())     = draw_rx.recv()     => render current view
    Some(res)    = file_rx.recv()     => @ file-search results → set_file_matches
  }
  ```

  (The fourth branch arrived with the `@` file picker — its background worker
  answers ranked matches on a tokio channel the loop can `select!` on; see
  `docs/file-search.md`.)

  `schedule_frame()` after every state change requests a redraw; the scheduler
  emits a coalesced tick that drives the actual paint. `insert_before` queues its
  lines and `set_view_height` adjusts the tracked geometry inline; the tick then
  writes the queued lines *and* repaints the live region in one synchronized
  frame (the flicker fix — `docs/flicker.md`).

## Invariant 1, restated

The previous "only the main thread reads stdin" becomes: **the `EventStream` is
the sole stdin reader, created *after* the init cursor-position query; the reply
backend only ever sends on its channel.** The reason is unchanged — a second
stdin reader would steal the cursor-position (DSR) reply. `init()` queries the
cursor once, synchronously, before the `EventStream` exists; `insert_before`
tracks the viewport row itself and never queries.

## Fairness

`tokio::select!` polls its branches in a randomized order each iteration, so
neither input nor draw ticks can starve the other — the effect codex gets from
its explicit round-robin `poll_draw_first` toggle.
