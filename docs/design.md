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
  so the inline conversation is preserved — showing the **full conversation
  transcript**: every user/AI message **and** every tool call's **complete**
  (expanded) output, interleaved in the exact order they happened, plus the live
  tail (in-progress reply / running tool), scrollable (↑/↓ PgUp/PgDn). It **opens
  pinned to the bottom** and tail-follows new content as it streams in (scroll up
  to read back; scrolling to the bottom re-engages following). It is the
  expanded counterpart of the inline view (where tools are collapsed). The
  conversation **keeps streaming and updating underneath**: while the overlay is up
  the event loop still drains reply events into `App` (so the view updates live)
  but holds off committing to scrollback; Ctrl+O (or Esc) returns, and the inline
  view is repainted from `history` to catch up. The overlay is read-only (typing is
  ignored).
- **Slash-command palette (Claude-Code style).** When the input is a **bare
  command token** — `/`, `/he`, `/help`, but *not* `ask /help` or anything past a
  space/newline — a scrollable command **palette opens below the input box** (a
  third band in the live region). It lists a registry of `SlashCommand`s
  (`app::COMMANDS`: name + description + effect — currently `/help` and `/clear`),
  filtered by name-prefix as you type after the `/`; `/` alone lists everything.
  ↑/↓ move the highlight (the window scrolls, capped at `MENU_MAX_ROWS`, to keep it
  visible); descriptions line up in a column (names padded to `MENU_DESC_COL`), and
  the selection is shown **by colour** — the whole highlighted row lights up cyan
  (name *and* description the same colour) while the others are dimmed grey — **no
  caret/arrow**.
  **Tab/Enter run** the highlighted command; **Esc** dismisses the palette (instead
  of quitting) and stays dismissed within the same token (delete the `/` and retype
  to reopen). The box's top is unchanged when the palette opens — it's reserved
  *below* the box — so the cursor never jumps. Running a command **consumes the
  input** and dispatches an `Action`: `/clear` → `Clear` (empties `history`,
  repaints) and `/help` → `Notice` (lists the commands). A `Notice` is recorded as
  a `Role::System` message and committed to scrollback like any other. Adding a
  command later is a one-line registry edit + an effect arm in
  `run_selected_command` — the palette, filtering, scrolling, and dispatch don't
  change.
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
| `app.rs`    | State + pure update logic: `App`, `on_key -> Action` (per `View`), `push_chunk`/`finish_stream`/`flush_streaming_segment`, `start_tool`/`end_tool`, the message+tool `history`, the tool-view scroll, **the slash-command palette** (`command_query`/`matching_commands`, `COMMANDS`, open/filter/scroll/dispatch). `Action`/`Role`/`Message`/`StreamError`/`ToolStatus`/`ToolCall`/`HistoryItem`/`View`/`SlashCommand`/`CommandEffect`/`CommandMenu` types. | Yes |
| `ui.rs`     | Pure rendering: `wrap_text` (display-width via `cols`), `message_lines`, `tool_lines` (collapsed inline) / `transcript_lines` (full conversation + expanded tools), `stable_commit`/`final_commit`, `conversation_lines`/`repaint_lines`/`repaint_budget`, the growing-input geometry (`live_height`, `repin`, `cursor_position`, `restore_cursor_row`), the **command-palette band** (`menu_rows`, `menu_window`, `command_menu_lines`), `render_live`, and `render_tool_view`. | Yes |
| `term.rs`   | The custom inline viewport over `CrosstermBackend`: dynamic content-anchored height, `insert_before` (scrollback), `draw` (re-pin + repaint + cursor), the alternate-screen overlay (`enter_overlay`/`exit_overlay`/`draw_overlay`), init/restore. | No (I/O boundary) |
| `main.rs`   | Thin glue: single-threaded poll loop, drives `term` (commits, draw, resize/return repaint, overlay), branches rendering on `View`, backend cancel/reap on quit. | No (tiny I/O boundary) |

### Data flow

Input is read on the **main thread** (`event::poll`); only the reply streams on a
background thread, which merely *sends* on a channel (it never reads stdin). This
avoids a stdin race with the cursor-position queries that terminal init and
`insert_before` make — the cause of the "cursor position could not be read" error.

Each loop turn **blocks for the first event** (a short wait while streaming so
chunks stay snappy, a longer one when idle so it doesn't spin), then **greedily
drains every other event already buffered** (`event::poll(Duration::ZERO)`) before
redrawing. So a burst of input — a paste, fast typing, an autorepeating key —
collapses into a *single* repaint instead of one redraw per keystroke. Without this
the redraw-per-key cost (each redraw re-wraps the whole input) made typing into a
growing draft lag super-linearly; the coalescing keeps it responsive. Guarded by
`scripts/smoke.sh` Phase 6 (a 1000-char burst must finish rendering near-instantly).

```
keyboard / resize ─► event::poll ─► App::on_key ─► Action::{Submit,ToggleToolView,Notice,Clear,Quit,None}
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
- On `StreamDone`: clear streaming state, then commit the final text segment +
  spacer. Because the streaming strip (preview + gap) is drawn *above* the box, the
  loop first calls `term.set_view_height` to reseat the viewport to its idle height,
  so the final commit replaces the strip's rows in place and the box stays flush at
  the bottom rather than rising and leaving blank rows beneath it (see the
  streaming-strip note under *Known limitations*).
- On `Error(msg)`: `App::fail_stream` records any non-empty partial reply, flushes
  it, then commits a red `Role::Error` notice (and records it in `history` so it
  repaints on resize); clears streaming state. Like `StreamDone` it reseats the
  viewport height first so the box doesn't rise as the strip clears.
- On `ToggleToolView` (Ctrl+O / Esc): enter or leave the alternate-screen overlay;
  on leaving, `repaint_conversation` reflows the inline view to catch up.
- On `Notice(text)` (a slash command's output — `/help`): if a reply is mid-flight,
  `flush_streaming_segment` finalises its current segment first (the same ordering
  trick a tool call uses), then `record_system_message` + an `insert_before` commit
  a `Role::System` notice to scrollback.
- On `Clear` (`/clear`): `App::history` is already empty; `repaint_conversation`
  reflows the now-blank inline view (clears the visible conversation).
- **In the tool-output view** every reply event still updates `App` (so the view
  shows tools live), but the commit-to-scrollback steps above are **skipped** —
  they would write into the alternate screen. The inline view is rebuilt from
  `history` on return.

### Key types

- `Role { User, Assistant, Error, System }` — drives bullet/colour (errors red,
  system notices cyan).
- `Action { None, Submit(String), ToggleToolView, Notice(String), Clear, Quit }` —
  returned by `App::on_key`.
- `View { Conversation, ToolOutput }` — which screen is showing (Ctrl+O toggles).
- `SlashCommand { name, description, effect }` + `CommandEffect { Clear, Help }` +
  the `COMMANDS` registry (`/help`, `/clear`) — the slash-command palette's data;
  adding a command is one registry entry (+ an effect arm).
- `CommandMenu { selected }` — the open palette's highlight (`App::command_menu`,
  `None` when closed); the matches are derived from the input on demand.
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
  in the chat); the viewer scrolls and ignores typing; it opens pinned to the
  bottom and `settle_tool_scroll` tail-follows (scrolling up disengages, reaching
  the bottom re-engages).
- `app` (slash palette): `command_query` recognises a bare `/token` (rejecting
  past-a-space/newline and mid-line slashes); `matching_commands` prefix-filters
  case-insensitively; the registry has unique lowercase names. Typing `/` opens
  the palette and filters/clamps the selection; ↑/↓ move within bounds; Backspace
  past the slash closes it; Esc dismisses (not quits) and is **sticky** within the
  same token (re-entering command mode reopens it); Enter/Tab run the highlighted
  command, returning the right `Action` (`/clear`→`Clear` + history emptied,
  `/help`→`Notice` listing commands) and consuming the input; an empty-match Enter
  doesn't submit; with no palette open Enter still submits normally.
- `ui`: `wrap_text` (word wrap, hard-break long words, newlines, width 0, **wide
  & zero-width chars**); `message_lines` (bullet on first line, indented
  continuation; user lines carry a dark background padded to the full display
  width; error lines get a red bullet; system notices a cyan one); `tool_lines`
  (status-coloured bullet header, collapsed peek + `(ctrl+o to expand)` hint,
  width-truncated); `transcript_lines`/`render_tool_view` (full conversation —
  messages interleaved with each tool's complete output, plus the live tail —
  status colour, scroll); the **command palette** — `menu_window` keeps the
  selection visible, `menu_rows` reserves the band (0 closed, capped, 1 for no
  matches), `command_menu_lines` lists the matches in aligned columns and
  **highlights the whole selected row in one cyan colour** (name and description
  alike, vs dimmed grey, no caret; placeholder when empty), and `render_live` draws
  it below the box with the cursor unmoved; the
  growing-input geometry — `live_height` grows a row per wrapped line, adds the
  preview + gap strip only while streaming and the palette band below the box, and
  clamps to the screen; `render_live` grows the box, scrolls the input to keep the
  end visible, separates a streaming preview (or a running tool's blue header) from
  the box with a blank gap, and shows no strip when idle; `cursor_position` follows
  the last wrapped row (and stays put when the palette opens); and
  `repin` keeps the box top-anchored (scrolling up only on overflow, clearing rows
  on shrink); `restore_cursor_row` lands the exit cursor just below the box (no
  blank gap on quit when the box is near the top); the `BULLET_WIDTH` / `repaint_budget`
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
  `ui::repin` helper; the cursor is placed from the final viewport. The region
  buffer is kept in `prev` and, when the geometry didn't move (no scroll, no
  vacated rows, same rect), only the cells that **changed** since the last draw are
  emitted (`Buffer::diff`) — so a keystroke ships a couple of cells (~35 bytes),
  not the whole region (~700 bytes). `prev` is invalidated (full repaint next) by
  anything that moves the screen under it: `insert_before`, `reflow`, the overlay,
  a resize. This is what keeps typing crisp on latency-bound terminals. The whole
  frame (prepare + cells + cursor, factored into `paint_frame`) is bracketed in a
  **synchronized update** (`BeginSynchronizedUpdate`/`EndSynchronizedUpdate`, DEC
  mode 2026), so the terminal swaps it in atomically and a fast keystroke burst
  never shows a half-painted frame or the cursor mid-flight — the same trick codex
  wraps its draws in (terminals without 2026 ignore the markers). `draw_overlay`
  brackets its full-screen paint the same way.
- `set_view_height(height)` — reseat the tracked viewport height *without*
  redrawing. `insert_before` reserves `view.height` rows *below* the lines it
  commits (to keep the box on screen), and the streaming strip (preview + gap)
  inflates that height while a reply streams. So at `StreamDone`/`Error` `main`
  calls this first to drop the strip's rows, letting the final commit replace them
  in place — otherwise `insert_before` over-scrolls and the box rises off the
  bottom (see *Known limitations*).
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
- **Streaming strip collapse.** The streaming strip (preview + gap) is drawn *above*
  the box, so it grows the live region *upward*. When a reply finishes, that strip's
  rows are handed back to scrollback as the committed final line + spacer, and the
  box must stay put. The fix is to reseat the viewport to its idle height
  (`term.set_view_height`) *before* the final `insert_before`, so the commit reserves
  only the idle box below it; without it the strip-still-counted height makes
  `insert_before` over-scroll and the box rises off the bottom, leaving blank rows
  beneath. Covered by `scripts/smoke.sh` Phase 5 (a short terminal where one
  exchange overflows the screen, asserting no blank rows below the settled box).
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
  read it in full is the Ctrl+O conversation view, which shows the whole transcript
  with every tool expanded (by design — keeps the inline chat compact).
- The slash-command palette only matches a **bare** `/token` (a leading slash, no
  whitespace); there's no argument parsing yet. The registry is intentionally small
  for now (`/help`, `/clear`) — adding a command is a one-line `COMMANDS` entry plus
  an effect arm in `run_selected_command`. Esc's dismissal reopens on the next
  keystroke only if you leave and re-enter command mode; and running `/help`
  mid-stream finalises the reply's current segment first (so the notice never
  splits the reply), the same ordering rule tool calls use.
- No spinner, timestamps, markdown rendering, or scrollback nav keys (YAGNI).
```
