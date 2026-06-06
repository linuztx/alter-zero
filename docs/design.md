# Inline Conversation TUI — Design

Date: 2026-06-06

## Goal

A minimal, readable, well-documented inline terminal UI (ratatui) for a
user ↔ AI conversation. The AI side is a **dummy** that streams a canned
response chunk-by-chunk. The layout must be responsive to terminal width,
and the visual style should echo Claude Code (a bottom-pinned input field
framed by a top/bottom rule, messages flowing above it in normal scrollback).
Everything that can be unit-tested must be unit-tested.

## Behaviour

- The app runs in ratatui's **inline viewport** (no alternate screen). Normal
  terminal scrollback is preserved — finished messages scroll up into your
  real terminal history, exactly like Claude Code.
- A small fixed-height **live region** stays pinned at the bottom:
  - one **preview row** showing the in-progress AI line as it streams (blank
    when idle),
  - an **input field framed by a top/bottom rule** (`❯ ...`).
- Type a message, press **Enter** to send. The user message is flushed to
  scrollback, then the dummy AI streams a reply.
- **Streaming → scrollback, line by line.** As the reply grows, each wrapped
  line that can no longer change (under greedy word-wrap only the last line
  can still change) is committed to scrollback via `insert_before`. The
  current partial line is shown live in the preview row. On completion the
  final line is committed too, with a blank spacer line after it. A blank
  spacer is also committed after every user message.
- **Responsive:** every draw re-wraps to the current terminal width, measured in
  **display columns** (`unicode-width`) so CJK/emoji wrap and pad correctly.
- **Growing input box.** The input field is multi-line and grows downward as the
  text wraps (or as explicit newlines are added with **Alt+Enter** / Shift+Enter),
  so a long message is never lost off the right edge. The live region height is
  therefore dynamic: `LIVE_MIN_HEIGHT` (preview + a one-row box) at rest, growing
  one row per wrapped input line up to the terminal height, after which the box
  scrolls internally to keep the cursor (always at the end) in view. Geometry is
  pure (`ui::live_height`, `ui::input_layout`) and unit-tested.
- **Resize reflow (both directions).** Any width change must re-wrap the visible
  chat: ratatui clears the screen on a width *shrink* but not on a *grow*, and
  either way the on-screen lines keep their old wrapping until redrawn. So `App`
  retains a `history` of finished messages and, on any width change, parks the
  cursor at row 0 (so ratatui seats the inline viewport at the top), clears the
  screen via `resize`, and repaints the tail that fits above the live region —
  re-wrapped to the new width, wider or narrower. With the viewport at the top
  the repaint fills the screen without scrolling (the tmux-safe path). Lines that
  had already scrolled into the terminal's own scrollback keep their original
  wrapping.
- **Backend errors & cancellation.** A reply backend (`ReplySource`) may end with
  `Error(msg)` instead of `StreamDone`; the partial reply (if any) is kept and a
  red error notice is shown below it. The built-in `DummyAi` never errors — this is
  the seam for a real model. Quitting mid-stream trips a `CancelToken` so the
  backend stops promptly and its thread is reaped before exit.
- **Quit:** Esc or Ctrl+C. Sending is disabled while a reply is streaming.

## Architecture

Built as a small **library** (`lib.rs`) + a thin **binary** (`main.rs`) so the
logic is unit-testable without a real terminal.

| File        | Responsibility | Tested? |
|-------------|----------------|---------|
| `stream.rs` | The backend seam: the `ReplySource` trait + built-in `DummyAi` impl, a `CancelToken`, and the `StreamEvent` protocol (`Chunk`/`Error`/`StreamDone`); plus pure `dummy_response`/`chunks`. | Pure parts, token & dummy: yes |
| `app.rs`    | State + pure update logic: `App`, `on_key -> Action`, `push_chunk`, `finish_stream`, `fail_stream`, message `history`. `Action`/`Role` (incl. `Error`)/`Message`/`StreamError` types. | Yes |
| `ui.rs`     | Pure rendering: `wrap_text` (display-width via `cols`), `message_lines`, `stable_commit`/`final_commit`, `conversation_lines`/`repaint_lines`/`repaint_budget`, the growing-input geometry (`input_layout`, `live_height`, `repin_scroll`), `cursor_position`, and `render_live`. | Yes |
| `term.rs`   | The custom inline viewport over `CrosstermBackend`: dynamic bottom-pinned height, `insert_before` (scrollback), `draw` (re-pin + repaint + cursor), init/restore. | No (I/O boundary) |
| `main.rs`   | Thin glue: single-threaded poll loop, drives `term` (commits, draw, resize repaint), backend cancel/reap on quit. | No (tiny I/O boundary) |

### Data flow

Input is read on the **main thread** (`event::poll`); only the reply streams on a
background thread, which merely *sends* on a channel (it never reads stdin). This
avoids a stdin race with the cursor-position queries that terminal init and
`insert_before` make — the cause of the "cursor position could not be read" error.

```
keyboard / resize ─► event::poll ─► App::on_key ─► Action::{Submit,Quit,None}
reply backend ─────► mpsc<StreamEvent> ─► try_recv ─► push_chunk / finish_stream / fail_stream
```

- On `Submit(text)`: `insert_before` the user message and a blank spacer, then
  `backend.spawn(text, tx, cancel)` — a thread that sends `Chunk(..)*` then
  `StreamDone` (or `Error(msg)`). The loop keeps the thread's `JoinHandle` and
  `CancelToken` so quitting mid-stream cancels and reaps it cleanly.
- On `Chunk`: append to the streaming buffer; commit any newly-stable lines to
  scrollback; redraw (preview row shows the partial last line).
- On `StreamDone`: commit the final line + spacer; clear streaming state.
- On `Error(msg)`: `App::fail_stream` records any non-empty partial reply, flushes
  it, then commits a red `Role::Error` notice (and records it in `history` so it
  repaints on resize); clears streaming state.

### Key types

- `Role { User, Assistant, Error }` — drives bullet/colour (errors get a red
  bullet).
- `Action { None, Submit(String), Quit }` — returned by `App::on_key`.
- `Message { role, text }` — one finished message, retained in `App::history`
  for repainting after a resize.
- `StreamError { partial: Option<String>, error: String }` — what `App::fail_stream`
  hands the loop to flush after a backend failure.
- `StreamEvent { Chunk(String), Error(String), StreamDone }` (in `stream.rs`) —
  what a backend sends to the loop.
- `ReplySource` (trait) + `DummyAi` (impl) + `CancelToken` (in `stream.rs`) — the
  pluggable backend seam. `spawn(prompt, tx, cancel) -> JoinHandle<()>`; a real
  model is a drop-in `ReplySource` and the loop never changes.

## Testing strategy

- `stream`: `dummy_response` deterministic & non-empty; `chunks` concatenates
  back to the original text and yields >1 chunk for multi-word input; `CancelToken`
  latches and is shared across clones; `DummyAi` delivers all chunks then
  `StreamDone`, and sends nothing when pre-cancelled; a custom `ReplySource` can
  report `Error`.
- `app`: typing appends; backspace; Enter with text → `Submit` + clears input;
  Enter while empty / while streaming → `None`; Esc/Ctrl+C → `Quit`;
  `push_chunk`/`finish_stream` transitions; `fail_stream` records partial + error.
- `ui`: `wrap_text` (word wrap, hard-break long words, newlines, width 0, **wide
  & zero-width chars**); `message_lines` (bullet on first line, indented
  continuation; user lines carry a dark background padded to the full display
  width; error lines get a red bullet); the `BULLET_WIDTH` / `live_layout` /
  `LIVE_HEIGHT` / `repaint_budget` single-source-of-truth invariants; `input_view`
  horizontal scroll (by columns); `cursor_position`; `render_live` asserted against
  a `Buffer`; and `stable_commit`/`final_commit` proven to reconstruct a whole
  streamed reply with no gaps or duplicates, and to clamp safely under a mid-stream
  resize.

## The custom inline viewport (`term.rs`)

ratatui's `Viewport::Inline(h)` fixes `h` at startup — the field is private and
`Terminal::resize` reuses the stored height, so the live region cannot grow. To
get a **dynamic, bottom-pinned** live region we drop ratatui's `Terminal` and own
a tiny viewport over a `CrosstermBackend` (`term::InlineViewport`), reusing the
backend's cell→ANSI `draw`, `append_lines` (scroll-up-into-scrollback), `clear`,
and cursor ops:

- `insert_before(lines)` — commit finished lines into real scrollback above the
  viewport, keeping it pinned to the bottom. This is a direct port of ratatui's
  portable (no-`scrolling-regions`) `insert_before` scroll math, including its
  tmux-safe "don't full-clear then scroll" ordering.
- `draw(height, render, cursor)` — re-pin the viewport to the bottom `height`
  rows (scrolling the screen *up* via `append_lines` to **grow**, or *down* via
  `ScrollDown` to **shrink**, by the height delta), repaint the live region, and
  place the hardware cursor. The scroll deltas come from the pure
  `ui::repin_scroll` helper.

`term.rs` is, like `main.rs`, an I/O boundary verified via `scripts/smoke.sh`
rather than unit tests; all the geometry it consumes is pure and tested in `ui`.

## Known limitations (v1 — iterate later)

- Input has no cursor navigation: text is appended/deleted at the end only
  (Alt+Enter inserts a newline there). Arrow-key editing is future work.
- Shrinking the input box (deleting wrapped lines) scrolls the on-screen chat
  back down, leaving blank rows at the very top where off-screen scrollback would
  be — terminals can't pull their own scrollback back onto the screen.
- On resize the on-screen chat is repainted (wider or narrower), but lines
  already in the terminal's own scrollback keep their original wrapping (so
  after resizing a long chat, boundary messages can appear twice — once
  old-width above, once new-width below). Guarded against panics.
- Resizing *mid-stream* recovers by re-committing the in-progress reply, but may
  briefly flicker the partial line.
- No spinner, timestamps, markdown rendering, or scrollback nav keys (YAGNI).
```
