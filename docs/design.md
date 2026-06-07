# Inline Conversation TUI — Design

Date: 2026-06-07

## Goal

A minimal, readable, well-documented inline terminal UI (ratatui) for a
user ↔ AI conversation. The AI side is a **dummy** that streams a canned
response chunk-by-chunk, with **tool calls interleaved** in the reply. The
layout must be responsive to terminal width, and the visual style should echo
Claude Code (a bottom-pinned input field framed by a top/bottom rule, messages
and tool calls flowing above it in normal scrollback). Everything that can be
unit-tested must be unit-tested.

## Behaviour

- The app runs in ratatui's **inline viewport** (no alternate screen). Normal
  terminal scrollback is preserved — finished messages scroll up into your
  real terminal history, exactly like Claude Code.
- The **live region** stays pinned at the bottom:
  - an **input field framed by a top/bottom rule** (`❯ ...`),
  - and, **only while a reply streams**, a **preview row** showing the
    in-progress AI line plus a blank **gap row** below it, so the live reply
    never butts up against the box. Idle, that strip collapses and the box sits
    directly under the chat.
- Type a message, press **Enter** to send. The user message is flushed to
  scrollback, then the dummy AI streams a reply.
- **Streaming → scrollback, line by line.** As the reply grows, each wrapped
  line that can no longer change (under greedy word-wrap only the last line
  can still change) is committed to scrollback via `insert_before`. The
  current partial line is shown live in the preview row, held off the box by the
  blank gap row. On completion the final line is committed too, with a blank
  spacer line after it — and as the streaming strip collapses, that committed
  spacer becomes the single blank line between the reply and the box (no double
  blank). A blank spacer is also committed after every user message.
- **Responsive:** every draw re-wraps to the current terminal width, measured in
  **display columns** (`unicode-width`) so CJK/emoji wrap and pad correctly.
- **Growing input box.** The input field is multi-line and grows downward as the
  text wraps (or as explicit newlines are added with **Alt+Enter** / Shift+Enter),
  so a long message is never lost off the right edge. The live region height is
  therefore dynamic: `LIVE_MIN_HEIGHT` (a one-row box, no preview strip) at rest,
  growing one row per wrapped input line up to the terminal height (plus a preview
  + gap row while a reply streams), after which the box scrolls internally to keep
  the cursor (always at the end) in view. The box is
  **content-anchored** like Claude Code / codex — its top stays put and it grows
  *downward*, only scrolling the chat up once it reaches the screen bottom (never
  jumping to the bottom). Geometry is pure (`ui::live_height`, `ui::repin`) and
  unit-tested.
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
- **Tool calls (Claude-Code style).** A reply can interleave tool calls. Each
  renders inline as a **coloured bullet header** `● name(args)` — **blue** while
  it runs (shown live in the bottom region's preview row), **green** when it
  succeeds, **red** when it fails — plus a **collapsed** one-line `⎿` peek of its
  output with a `(ctrl+o to expand)` hint when more is hidden. The full output is
  never shown inline. A tool call splits the assistant text around it: the run of
  text before a tool is committed as its own message so the scrollback (and the
  resize repaint) keeps text and tools in the exact order they streamed. The dummy
  runs a `Read` (green) then a `Bash` (red) per turn so all three colours show.
- **The Ctrl+O tool-output view.** Ctrl+O (from either screen, even mid-stream)
  opens a **separate full-screen overlay** — on the terminal's *alternate screen*,
  so the inline conversation is preserved — listing **every** tool call's
  **complete** output, scrollable (↑/↓ PgUp/PgDn). The conversation **keeps
  streaming and updating underneath**: while the overlay is up the event loop
  still drains reply events into `App` (so the view shows tools appear, run, and
  resolve live) but holds off committing to scrollback; Ctrl+O (or Esc) returns,
  and the inline view is repainted from `history` to catch up on everything that
  streamed while away. The overlay is read-only (typing is ignored).
- **Backend errors & cancellation.** A reply backend (`ReplySource`) may end with
  `Error(msg)` instead of `StreamDone`; the partial reply (if any) is kept and a
  red error notice is shown below it. The built-in `DummyAi` never errors — this is
  the seam for a real model. Quitting mid-stream trips a `CancelToken` so the
  backend stops promptly and its thread is reaped before exit.
- **Quit:** Esc (in the conversation) or Ctrl+C (anywhere). In the tool-output
  view Esc returns to the chat instead of quitting. Sending is disabled while a
  reply is streaming.

## Architecture

Built as a small **library** (`lib.rs`) + a thin **binary** (`main.rs`) so the
logic is unit-testable without a real terminal.

| File        | Responsibility | Tested? |
|-------------|----------------|---------|
| `stream.rs` | The backend seam: the `ReplySource` trait + built-in `DummyAi` impl, a `CancelToken`, and the `StreamEvent` protocol (`Chunk`/`ToolStart`/`ToolEnd`/`Error`/`StreamDone`); plus pure `dummy_response`/`chunks`/`turn_events` (the interleaved tool script). | Pure parts, token & dummy: yes |
| `app.rs`    | State + pure update logic: `App`, `on_key -> Action` (per `View`), `push_chunk`/`finish_stream`/`flush_streaming_segment`, `start_tool`/`end_tool`, the message+tool `history`, the tool-view scroll. `Action`/`Role`/`Message`/`StreamError`/`ToolStatus`/`ToolCall`/`HistoryItem`/`View` types. | Yes |
| `ui.rs`     | Pure rendering: `wrap_text` (display-width via `cols`), `message_lines`, `tool_lines` (collapsed) / `tool_view_lines` (full), `stable_commit`/`final_commit`, `conversation_lines`/`repaint_lines`/`repaint_budget`, the growing-input geometry (`live_height`, `repin`, `cursor_position`), `render_live`, and `render_tool_view`. | Yes |
| `term.rs`   | The custom inline viewport over `CrosstermBackend`: dynamic content-anchored height, `insert_before` (scrollback), `draw` (re-pin + repaint + cursor), the alternate-screen overlay (`enter_overlay`/`exit_overlay`/`draw_overlay`), init/restore. | No (I/O boundary) |
| `main.rs`   | Thin glue: single-threaded poll loop, drives `term` (commits, draw, resize/return repaint, overlay), branches rendering on `View`, backend cancel/reap on quit. | No (tiny I/O boundary) |

### Data flow

Input is read on the **main thread** (`event::poll`); only the reply streams on a
background thread, which merely *sends* on a channel (it never reads stdin). This
avoids a stdin race with the cursor-position queries that terminal init and
`insert_before` make — the cause of the "cursor position could not be read" error.

```
keyboard / resize ─► event::poll ─► App::on_key ─► Action::{Submit,ToggleToolView,Quit,None}
reply backend ─────► mpsc<StreamEvent> ─► try_recv ─► push_chunk / start_tool / end_tool / finish_stream / fail_stream
```

- On `Submit(text)`: `insert_before` the user message and a blank spacer, then
  `backend.spawn(text, tx, cancel)` — a thread that sends `Chunk(..)*` with
  `ToolStart`/`ToolEnd` pairs interleaved, then `StreamDone` (or `Error(msg)`).
  The loop keeps the thread's `JoinHandle` and `CancelToken` so quitting
  mid-stream cancels and reaps it cleanly.
- On `Chunk`: append to the streaming buffer; commit any newly-stable lines to
  scrollback; redraw (preview row shows the partial last line).
- On `ToolStart{name,args}`: `flush_streaming_segment` finalises the run of text
  before the tool (so it slots ahead of the tool in order) and commits its
  remainder; `start_tool` shows the tool running (blue) in the preview row.
- On `ToolEnd{output,ok}`: `end_tool` records the finished tool; commit it
  *collapsed* (green/red) to scrollback. The full output is kept for the Ctrl+O
  view.
- On `StreamDone`: commit the final text segment + spacer; clear streaming state.
- On `Error(msg)`: `App::fail_stream` records any non-empty partial reply, flushes
  it, then commits a red `Role::Error` notice (and records it in `history` so it
  repaints on resize); clears streaming state.
- On `ToggleToolView` (Ctrl+O / Esc): enter or leave the alternate-screen
  overlay; on leaving, `repaint_conversation` reflows the inline view to catch up.
- **In the tool-output view** every reply event still updates `App` (so the view
  shows tools live), but the commit-to-scrollback steps above are **skipped** —
  they would write into the alternate screen. The inline view is rebuilt from
  `history` on return.

### Key types

- `Role { User, Assistant, Error }` — drives bullet/colour (errors get a red
  bullet).
- `Action { None, Submit(String), ToggleToolView, Quit }` — returned by
  `App::on_key`.
- `View { Conversation, ToolOutput }` — which screen is showing (Ctrl+O toggles).
- `Message { role, text }` — one finished message.
- `ToolStatus { Running, Ok, Failed }` — a tool's lifecycle (blue/green/red).
- `ToolCall { name, args, status, output }` — one tool invocation; `current_tool`
  while running, then recorded in history.
- `HistoryItem { Message(Message), Tool(ToolCall) }` — one ordered history entry;
  messages and tools share `App::history` so they repaint interleaved in order.
- `StreamError { partial: Option<String>, error: String }` — what `App::fail_stream`
  hands the loop to flush after a backend failure.
- `StreamEvent { Chunk(String), ToolStart{name,args}, ToolEnd{output,ok},
  Error(String), StreamDone }` (in `stream.rs`) — what a backend sends to the loop.
- `ReplySource` (trait) + `DummyAi` (impl) + `CancelToken` (in `stream.rs`) — the
  pluggable backend seam. `spawn(prompt, tx, cancel) -> JoinHandle<()>`; a real
  model is a drop-in `ReplySource` (emit `ToolStart`/`ToolEnd` for tool calls) and
  the loop never changes.

## Testing strategy

- `stream`: `dummy_response` deterministic & non-empty; `chunks` concatenates
  back to the original text and yields >1 chunk for multi-word input; `CancelToken`
  latches and is shared across clones; `DummyAi` delivers all chunks then
  `StreamDone`, and sends nothing when pre-cancelled; a custom `ReplySource` can
  report `Error`.
- `stream` (tools): `turn_events` interleaves ≥1 tool call whose `Chunk`s still
  concatenate to the reply, each `ToolStart` immediately resolved by a `ToolEnd`,
  with both a success and a failure; ends with `StreamDone`; `DummyAi` emits the
  tool calls.
- `app`: typing appends; backspace; Enter with text → `Submit` + clears input;
  Alt+Enter / Shift+Enter insert a newline (box grows) without submitting; Enter
  while empty / while streaming → `None`; Esc/Ctrl+C → `Quit`;
  `push_chunk`/`finish_stream` transitions; `fail_stream` records partial + error.
- `app` (tools & view): `start_tool`/`end_tool` move a tool through running →
  ok/failed and into history; `flush_streaming_segment` records the text before a
  tool and reopens an empty buffer; `finish_stream` records nothing for an empty
  final segment; a turn interleaves text/tool/text in order. Ctrl+O toggles the
  view (even mid-stream, stream keeps running); Esc closes the overlay (vs quits
  in the chat); the viewer scrolls and ignores typing; `tool_calls` lists finished
  then running.
- `ui`: `wrap_text` (word wrap, hard-break long words, newlines, width 0, **wide
  & zero-width chars**); `message_lines` (bullet on first line, indented
  continuation; user lines carry a dark background padded to the full display
  width; error lines get a red bullet); `tool_lines` (status-coloured bullet
  header, collapsed peek + `(ctrl+o to expand)` hint, width-truncated);
  `tool_view_lines`/`render_tool_view` (full output, status colour, scroll); the
  growing-input geometry — `live_height` grows a row per wrapped line, adds the
  preview + gap strip only while streaming, and clamps to the screen; `render_live`
  grows the box, scrolls the input to keep the end visible, separates a streaming
  preview (or a running tool's blue header) from the box with a blank gap, and
  shows no strip when idle; `cursor_position` follows the last wrapped row; and
  `repin` keeps the box top-anchored (scrolling up only on overflow, clearing rows
  on shrink); the `BULLET_WIDTH` / `repaint_budget`
  single-source-of-truth invariants; and `stable_commit`/`final_commit` proven to
  reconstruct a whole streamed reply with no gaps or duplicates, and to clamp
  safely under a mid-stream resize.

## The custom inline viewport (`term.rs`)

ratatui's `Viewport::Inline(h)` fixes `h` at startup — the field is private and
`Terminal::resize` reuses the stored height, so the live region cannot grow. To
get a **dynamic, content-anchored** live region we drop ratatui's `Terminal` and
own a tiny viewport over a `CrosstermBackend` (`term::InlineViewport`), reusing
the backend's cell→ANSI `draw`, `append_lines` (scroll-up-into-scrollback),
`clear`, and cursor ops:

- `insert_before(lines)` — commit finished lines into real scrollback above the
  viewport. A direct port of ratatui's portable (no-`scrolling-regions`)
  `insert_before` scroll math: it pushes the viewport *down* while there's room
  and scrolls only once the screen is full, including the tmux-safe "don't
  full-clear then scroll" ordering.
- `draw(height, render, cursor)` — repaint the live region at its new `height`,
  keeping its **top anchored** so it grows *downward* in place. It scrolls the
  screen up (via `append_lines`, oldest chat into scrollback) only when the box
  would overflow the bottom, and blanks the rows a shrink vacates just below it.
  The decision (`scroll_up` / new `top` / `clear_below`) comes from the pure
  `ui::repin` helper; the cursor is placed from the final viewport.
- `enter_overlay` / `draw_overlay` / `exit_overlay` — the Ctrl+O tool-output view.
  `enter_overlay` switches to the terminal's **alternate screen** (so the inline
  conversation — main screen + its real scrollback — is preserved untouched);
  `draw_overlay` paints a full-screen buffer (`ui::render_tool_view`) every frame;
  `exit_overlay` switches back, after which `main` reflows the inline view to catch
  up. This is the *only* use of the alternate screen — the conversation itself
  stays inline.

`term.rs` is, like `main.rs`, an I/O boundary verified via `scripts/smoke.sh`
rather than unit tests; all the geometry it consumes is pure and tested in `ui`.

## Known limitations (v1 — iterate later)

- Input has no cursor navigation: text is appended/deleted at the end only
  (Alt+Enter inserts a newline there). Arrow-key editing is future work.
- The box is content-anchored, so when it grows past the screen bottom the chat
  scrolls into the terminal's real scrollback; shrinking it again can't pull that
  chat back (terminals can't reverse-scroll their own scrollback), so after a
  grow-past-bottom-then-shrink the box stays where it scrolled to with blank rows
  below. Normal short messages never hit this.
- On resize the on-screen chat is repainted (wider or narrower), but lines
  already in the terminal's own scrollback keep their original wrapping (so
  after resizing a long chat, boundary messages can appear twice — once
  old-width above, once new-width below). Guarded against panics.
- Resizing *mid-stream* recovers by re-committing the in-progress reply, but may
  briefly flicker the partial line.
- Returning from the tool-output view repaints the inline conversation from
  `history` (the same path as a resize), so it shares the same edge: anything that
  had already scrolled into the terminal's own scrollback before the overlay
  opened keeps its old position, and a reply segment that was *partially* committed
  when the overlay opened is re-committed on return (a brief flicker, no data loss).
- Tool output shown inline is always collapsed to a one-line peek; the only way to
  read it in full is the Ctrl+O view (by design — keeps the chat compact).
- No spinner, timestamps, markdown rendering, or scrollback nav keys (YAGNI).
```
