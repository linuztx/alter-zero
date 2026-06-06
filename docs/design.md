# Inline Conversation TUI — Design

Date: 2026-06-06

## Goal

A minimal, readable, well-documented inline terminal UI (ratatui) for a
user ↔ AI conversation. The AI side is a **dummy** that streams a canned
response chunk-by-chunk. The layout must be responsive to terminal width,
and the visual style should echo Claude Code (a bottom-pinned rounded input
box, messages flowing above it in normal scrollback). Everything that can be
unit-tested must be unit-tested.

## Behaviour

- The app runs in ratatui's **inline viewport** (no alternate screen). Normal
  terminal scrollback is preserved — finished messages scroll up into your
  real terminal history, exactly like Claude Code.
- A small fixed-height **live region** stays pinned at the bottom:
  - one **preview row** showing the in-progress AI line as it streams (or a
    dim hint when idle),
  - a **rounded-border input box** (`> ...`).
- Type a message, press **Enter** to send. The user message is flushed to
  scrollback, then the dummy AI streams a reply.
- **Streaming → scrollback, line by line.** As the reply grows, each wrapped
  line that can no longer change (under greedy word-wrap only the last line
  can still change) is committed to scrollback via `insert_before`. The
  current partial line is shown live in the preview row. On completion the
  final line is committed too, with a blank spacer line after it.
- **Responsive:** every draw re-wraps to the current terminal width. Width
  adapts live. The live region height is fixed (an inline-viewport constraint
  — `set_viewport_area` is not public), clamped to the terminal height.
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
- **Quit:** Esc or Ctrl+C. Sending is disabled while a reply is streaming.

## Architecture

Built as a small **library** (`lib.rs`) + a thin **binary** (`main.rs`) so the
logic is unit-testable without a real terminal.

| File        | Responsibility | Tested? |
|-------------|----------------|---------|
| `stream.rs` | Dummy AI: `dummy_response`, `chunks`, plus a thin `spawn_stream` background thread. | Pure parts: yes |
| `app.rs`    | State + pure update logic: `App`, `on_key -> Action`, `push_chunk`, `finish_stream`, message `history`. `Action`/`Role`/`Message` types. | Yes |
| `ui.rs`     | Pure rendering: `wrap_text`, `message_lines`, `stable_commit`/`final_commit`, `conversation_lines`/`repaint_lines`, `input_view`, `cursor_position`, and `render_live`. | Yes |
| `main.rs`   | Thin glue: terminal init/restore, single-threaded poll loop, `insert_before` commits, draw, resize repaint. | No (tiny I/O boundary) |

### Data flow

Input is read on the **main thread** (`event::poll`); only the reply streams on a
background thread, which merely *sends* on a channel (it never reads stdin). This
avoids a stdin race with the cursor-position queries that terminal init and
`insert_before` make — the cause of the "cursor position could not be read" error.

```
keyboard / resize ─► event::poll ─► App::on_key ─► Action::{Submit,Quit,None}
stream thread ─────► mpsc<StreamEvent> ─► try_recv ─► push_chunk / finish_stream
```

- On `Submit(text)`: `insert_before` the user message, then `spawn_stream` a
  thread that sends `Chunk(..)*` then `StreamDone`.
- On `Chunk`: append to the streaming buffer; commit any newly-stable lines to
  scrollback; redraw (preview row shows the partial last line).
- On `StreamDone`: commit the final line + spacer; clear streaming state.

### Key types

- `Role { User, Assistant }` — drives bullet/colour.
- `Action { None, Submit(String), Quit }` — returned by `App::on_key`.
- `Message { role, text }` — one finished message, retained in `App::history`
  for repainting after a resize.
- `StreamEvent { Chunk(String), StreamDone }` (in `stream.rs`) — what the
  streaming thread sends to the loop.

## Testing strategy

- `stream`: `dummy_response` deterministic & non-empty; `chunks` concatenates
  back to the original text and yields >1 chunk for multi-word input;
  `spawn_stream` delivers all chunks then `StreamDone`.
- `app`: typing appends; backspace; Enter with text → `Submit` + clears input;
  Enter while empty / while streaming → `None`; Esc/Ctrl+C → `Quit`;
  `push_chunk`/`finish_stream` transitions.
- `ui`: `wrap_text` (word wrap, hard-break long words, newlines, width 0);
  `message_lines` (bullet on first line, indented continuation); `input_view`
  horizontal scroll; `cursor_position`; `render_live` asserted against a
  `Buffer`; and `stable_commit`/`final_commit` proven to reconstruct a whole
  streamed reply with no gaps or duplicates.

## Known limitations (v1 — iterate later)

- Single-line input (horizontal scroll to keep the cursor visible).
- On resize the on-screen chat is repainted (wider or narrower), but the
  decorative header banner is not, and lines already in the terminal's own
  scrollback keep their original wrapping (so after resizing a long chat, boundary
  messages can appear twice — once old-width above, once new-width below). Guarded
  against panics.
- Resizing *mid-stream* recovers by re-committing the in-progress reply, but may
  briefly flicker the partial line.
- No spinner, timestamps, markdown rendering, or scrollback nav keys (YAGNI).
```
