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
  - and, **only while a turn is in flight**, a strip above the box: a **preview
    row** showing the in-progress AI line (or a running tool's blue header), a
    blank **gap row**, a **status line** —
    `( ●    ) {verb}… ({elapsed}s · {↓|↑} {n} tokens · Thinking for {m}s · esc
    to interrupt)`, a
    codex/Claude-Code-style indicator (see *Status indicator* below) — then
    another blank gap row so the status clears the box's top rule. Idle, that
    strip collapses and the box sits directly under the chat.
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
- **Status indicator.** While a turn is in flight, the strip's status line opens
  with a **bouncing-ball spinner** (cli-spinners' `bouncingBall`: a white ball
  ping-ponging between dim walls, one frame per 80 ms — `ui::spinner_spans`),
  then a per-turn whimsical **verb** (`Working`, `Cooking`, …, picked
  deterministically by a turn counter) whose white text carries a codex-style
  **shimmer** — a bright-white raised-cosine band sweeping the chars every 2 s
  (`ui::shimmer_spans`, ported from openai/codex) — a
  **timer** in whole seconds, a cumulative **token** estimate (`↓` while the
  reply streams, flipping to `↑` right after a tool result — never reset mid-turn),
  `Thinking for Ns` *only* while the model is in a thinking phase, and a closing
  dim **`esc to interrupt`** hint (codex's discoverability hint). While a
  turn is active the draw branch re-arms an animation frame every 32 ms (codex's
  cadence), so the ball bounces, the shimmer sweeps, and the timer moves even
  with no events. On
  finish the line is replaced by a dim, committed **`{done verb} for Ns`** summary
  that flows into scrollback (a `HistoryItem::Summary`, so it survives a resize
  and lists in the Ctrl+O transcript — stamp-free, like every non-user item).
  Time is impure, so — like
  the timestamp clock — the loop owns the `Instant`s and feeds the pure status
  only computed `Duration`s (`App::set_status_times`; the same value drives the
  displayed seconds and both animation phases). See `docs/status-indicator.md`.
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
  view is repainted from `history` to catch up — and **so does quitting** (Ctrl+C)
  from the overlay, which repaints before exiting so a turn that finished while the
  overlay was up restores its `Done for Ns` summary rather than the stale streaming
  strip it left frozen on the main screen. The overlay is read-only (typing is
  ignored). Only the **user** message shows a **wall-clock timestamp** (12-hour,
  no seconds, e.g. `03:20 AM`): dim, **right-aligned on its own line below the
  message** (after a blank row) — the **only** stamp displayed anywhere (AI
  replies, tools, and turn summaries record one but never show it); the inline
  conversation never shows any. The clock is injected at the I/O boundary
  (`App::set_clock`, a real `chrono::Local` clock in `main.rs`; `None` in unit
  tests → empty stamp) and each item stores the pre-formatted string, so the
  pure library stays clock-free. See `docs/timestamps.md`.
- **Slash-command palette (Claude-Code style).** When the input is a **bare
  command token** — `/`, `/he`, `/help`, but *not* `ask /help` or anything past a
  space/newline — a scrollable command **palette opens below the input box** (a
  third band in the live region). It lists a registry of `SlashCommand`s
  (`app::COMMANDS`: name + description + effect — currently `/help`, `/clear`,
  and `/quit`),
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
  repaints), `/help` → `Notice` (lists the commands), and `/quit` → `Quit`
  (exits — codex's `/quit`/`/exit`, "exit Codex"). A `Notice` is recorded as
  a `Role::System` message and committed to scrollback like any other. Adding a
  command later is a one-line registry edit + an effect arm in
  `run_selected_command` — the palette, filtering, scrolling, and dispatch don't
  change.
- **`?` shows a shortcuts band** (codex's footer shortcut overlay — see
  `docs/shortcuts.md`): pressing `?` (shift-modified or not) in an **empty
  composer** toggles a keyboard-shortcuts overview in the palette's slot below
  the box — two aligned columns of `{key} for {thing}` entries (keys cyan,
  labels dim) listing `/`, `↑`, `alt+enter`, `ctrl+o`, `esc`, and `ctrl+c`;
  the `esc` entry reads `to interrupt` while a turn runs and `to quit` idle.
  With a draft in the box `?` is just a character. The band is display-only,
  never modal: any other key closes it and still performs its action — except
  Esc, which only dismisses (the palette's Esc rule; idle Esc would otherwise
  quit). The band and the palette never show together, and the cursor stays
  put when the band opens (it is reserved below the box).
- **Queue a message while a turn streams** (codex's `queued_user_messages` — see
  `docs/queue.md`): pressing Enter *during a turn* doesn't wait — the message
  joins `App::queued` (consuming the composer, recorded in `input_history` for ↑
  recall) and shows as a dim `↳ {msg}` row in the band below the box. When the
  turn ends the loop sends **one** queued message as its own turn
  (`App::dequeue`, FIFO — `main.rs::start_turn`, shared with `Submit`), and
  **Esc interrupts the current turn and sends the next queued one right away**.
  **Alt+Up** pulls the most-recent queued message back into an empty composer to
  edit, resend, or drop (codex's `edit_queued_message`). Slash commands aren't
  queued (they run inline via the palette).
- **Backend errors & cancellation.** A reply backend (`ReplySource`) may end with
  `Error(msg)` instead of `StreamDone`; the partial reply (if any) is kept and a
  red error notice is shown below it. The built-in `DummyAi` never errors — this is
  the seam for a real model. Quitting mid-stream trips a `CancelToken` so the
  backend stops promptly and its thread is reaped before exit.
- **Esc interrupts a streaming turn** (ported from openai/codex — see
  `docs/interrupt.md`): a single Esc while a turn is in flight cancels + reaps
  the backend, keeps the partial reply, resolves a still-running tool as failed
  (`Interrupted by user`), and commits a red
  `Conversation interrupted - tell the model what to do differently.` notice —
  with **no** `Done for Ns` summary (the notice is the turn's terminal state).
  The palette still wins: Esc with the palette open only dismisses it, even
  mid-turn. The status line's `esc to interrupt` hint advertises this.
- **↑/↓ recall submitted messages** (shell-style, ported from codex's
  `ChatComposerHistory` — see `docs/input-history.md`): with an **empty
  composer**, ↑ recalls the last submitted message (older with further
  presses, clamping at the oldest), ↓ steps back toward the newest and **past
  it clears the composer**. Recall replaces the draft with the cursor at the
  end; the gate (`InputHistory::should_navigate`) keeps the arrows' day job —
  a typed draft, an edited recall, or an interior cursor falls through to
  normal cursor movement, and the open palette still intercepts ↑/↓ first.
  Adjacent duplicate submissions collapse; the history survives `/clear`
  (codex's spans whole sessions); recalling a bare `/token` re-derives the
  palette like typing it.
- **Quit:** Esc (in the conversation, while **idle** — mid-turn it interrupts
  instead), Ctrl+C, or the `/quit` command. **Ctrl+C first clears a non-empty
  input** (codex's composer-clear step: a first press with a typed draft only
  empties the box — recording the draft so ↑ can bring it back — and closes
  the palette, since the emptied input is no longer a `/token`; the overlay
  has no input box, so Ctrl+C there always quits); with an empty input it
  quits from anywhere, even mid-stream. In the tool-output view Esc returns to
  the chat instead of quitting. Sending is disabled while a reply is
  streaming.

## Architecture

Built as a small **library** (`lib.rs`) + a thin **binary** (`main.rs`) so the
logic is unit-testable without a real terminal.

| File        | Responsibility | Tested? |
|-------------|----------------|---------|
| `stream.rs` | The backend seam: the `ReplySource` trait (sends on a **tokio** `UnboundedSender<StreamEvent>`) + built-in `DummyAi` impl, a `CancelToken`, and the `StreamEvent` protocol (`Chunk`/`ToolStart`/`ToolEnd`/`ThinkingStart`/`ThinkingEnd`/`Error`/`StreamDone`); plus pure `dummy_response`/`chunks`/`turn_events` (the interleaved thinking + tool script). | Pure parts, token & dummy: yes |
| `app.rs`    | State + pure update logic: `App` (its `input` is a `TextArea`), `on_key -> Action` (per `View`; routes editing/cursor keys to the textarea), `push_chunk`/`finish_stream`/`flush_streaming_segment`/`interrupt_turn`, `start_tool`/`end_tool`, the message+tool `history`, **the ↑/↓ input-history recall** (`InputHistory` — record/gate/up/down, `docs/input-history.md`), **the `?` shortcuts-band toggle** (`shortcuts_open`, `docs/shortcuts.md`), **the mid-turn message queue** (`queued`/`dequeue`, Enter-queues + Alt+Up edit, `docs/queue.md`), the tool-view scroll, **the slash-command palette** (`command_query`/`matching_commands`, `COMMANDS`, open/filter/scroll/dispatch). `Action`/`Role`/`Message`/`StreamError`/`InterruptedTurn`/`ToolStatus`/`ToolCall`/`HistoryItem`/`View`/`SlashCommand`/`CommandEffect`/`CommandMenu`/`InputHistory` types. | Yes |
| `textarea.rs` | The **codex-style editable input** (`TextArea`): `text` + a movable `cursor`, a width-keyed `wrap_cache`, and a `preferred_col` for vertical motion. Insert/delete at the cursor, grapheme ←/→, wrapped ↑/↓ (logical-line fallback when the cache is cold), Home/End, and byte-range wrapping (`wrapped_rows`/`display_rows`/`cursor_row_col`/`row_count`). Focused port of codex's editing core; see `docs/textarea.md`. | Yes |
| `ui.rs`     | Pure rendering: `wrap_text` (display-width via `cols`, for **messages**), `message_lines`, `tool_lines` (collapsed inline) / `transcript_lines` (full conversation + expanded tools), `stable_commit`/`final_commit`, `conversation_lines`/`repaint_lines`/`repaint_budget`, the growing-input geometry (`live_height`, `repin`, `cursor_position`, `restore_cursor_row`, `input_scroll` — follows the textarea cursor), the **command-palette band** (`menu_rows`, `menu_window`, `command_menu_lines`), the **`?` shortcuts band** sharing its slot (`shortcuts_rows`, `shortcuts_lines`), the **queued-message band** stacked below them (`queued_rows`, `queued_lines`), `render_live`, and `render_tool_view`. | Yes |
| `frame.rs`  | Frame scheduling (codex-style): `FrameRateLimiter` (120 fps floor) + `soonest` request-coalescing (pure), and the async `FrameRequester`/`run_scheduler` task that turns a flood of `schedule_frame` calls into one rate-limited draw tick. | Pure parts: yes (async task: smoke) |
| `paste.rs`  | Paste-burst detection: `PasteBurst`, a pure state machine — a run of characters within `BURST_CHAR_INTERVAL` is a burst once `BURST_MIN_CHARS` pile up, so the loop coalesces the run's redraw. | Yes |
| `term.rs`   | The custom inline viewport over `CrosstermBackend`: dynamic content-anchored height, `insert_before` (scrollback), `draw` (re-pin + diff + synchronized-update + cursor), the alternate-screen overlay (`enter_overlay`/`exit_overlay`/`draw_overlay`), init/restore. | No (I/O boundary) |
| `main.rs`   | Thin glue: single-threaded **async (tokio) `select!`** loop over input (`EventStream`), reply events (tokio channel), and draw ticks; drives `term` (commits, draw, resize/return repaint, overlay), branches rendering on `View`, backend cancel/reap on quit. | No (tiny I/O boundary) |

### Data flow

The loop is **async** (tokio, single-threaded `current_thread` runtime), built as a
`select!` over three sources — exactly how openai/codex drives its TUI. Terminal
input arrives on a crossterm **`EventStream`**, the reply streams in on a tokio
channel, and **draw ticks** come from the frame scheduler. `select!` polls its
branches in randomized order, so input and draws can't starve each other (codex's
explicit round-robin fairness, for free). The async-rewrite design lives in
`docs/async-rewrite.md`.

The `EventStream` is the **sole** stdin reader, created *after* `InlineViewport::init`
has queried the cursor position over stdin once, synchronously. The reply backend
runs on a background thread that only *sends* on its channel — it never reads stdin.
A second stdin reader would steal the cursor-position (DSR) reply — the cause of the
"cursor position could not be read" error. (`insert_before` tracks the viewport row
itself and never queries the cursor.)

Rendering is **tick-driven**: every state change calls `frame.schedule_frame()`,
and the scheduler coalesces a burst of those into a single draw, rate-limited to
120 fps (`MIN_FRAME_INTERVAL` = 8.33 ms). A paste / fast-type run is recognised by
`PasteBurst`, so its redraw is *deferred to the burst's tail* (`schedule_frame_in`)
— one repaint for the run instead of one per keystroke. `insert_before` stays inline
(it mutates scrollback immediately); only the live-region paint waits for a tick.
Guarded by `scripts/smoke.sh` Phase 6 (a 1000-char burst must finish rendering
near-instantly).

```
keyboard / resize ─► EventStream ─┐
reply backend ───► tokio mpsc ────┤─► select! ─► App::on_key / push_chunk / start_tool / … ─► schedule_frame
frame scheduler ─► draw-tick ─────┘                                                          │
                                                              draw tick ◄── coalesce + 120fps ┘ ─► draw / draw_overlay
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
- On `ThinkingStart`/`ThinkingEnd`: the loop flips its `thinking_start` `Instant`
  so the status line shows/drops `Thinking for Ns`; nothing is committed (thinking
  is live-only).
- On `StreamDone`: clear streaming state and end the turn (record the `Done for Ns`
  summary), then commit the final text segment + spacer + the summary. Because the
  streaming strip (preview + gap + status + gap) is drawn *above* the box, the loop first
  calls `term.set_view_height` to reseat the viewport to its idle height, so the
  final commit replaces the strip's rows in place and the box stays flush at the
  bottom rather than rising and leaving blank rows beneath it (see the
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
- On `Interrupt` (Esc while a turn is in flight — `docs/interrupt.md`): cancel +
  `join` the backend thread, **drain** the reply channel (events sent before the
  cancel was observed would otherwise arrive after the turn ended — a stale
  `ToolStart` would wedge a phantom running tool), then `App::interrupt_turn`
  and commit like `StreamDone` does: reseat the viewport to its idle height,
  flush the kept partial, the cancelled tool (collapsed, red), and the red
  `Conversation interrupted` notice. No `Done for Ns` summary.
- **In the tool-output view** every reply event still updates `App` (so the view
  shows tools live), but the commit-to-scrollback steps above are **skipped** —
  they would write into the alternate screen. The inline view is rebuilt from
  `history` on return.

### Key types

- `Role { User, Assistant, Error, System }` — drives bullet/colour (errors red,
  system notices cyan).
- `Action { None, Submit(String), ToggleToolView, Notice(String), Clear,
  Interrupt, Quit }` — returned by `App::on_key`.
- `View { Conversation, ToolOutput }` — which screen is showing (Ctrl+O toggles).
- `SlashCommand { name, description, effect }` + `CommandEffect { Clear, Help,
  Quit }` + the `COMMANDS` registry (`/help`, `/clear`, `/quit`) — the
  slash-command palette's data; adding a command is one registry entry (+ an
  effect arm).
- `CommandMenu { selected }` — the open palette's highlight (`App::command_menu`,
  `None` when closed); the matches are derived from the input on demand.
- `Message { role, text, timestamp }` — one finished message (the `timestamp` is
  displayed only for **user** messages, in the Ctrl+O transcript).
- `ToolStatus { Running, Ok, Failed }` — a tool's lifecycle (blue/green/red).
- `ToolCall { name, args, status, output, timestamp }` — one tool invocation;
  `current_tool` while running, then recorded in history (stamped when it finishes).
- `TokenArrow { Down, Up }` + `TurnStatus { verb, done_verb, tokens, arrow,
  elapsed, thinking }` — the live status of the turn in flight (`App::status`);
  the `Duration`s are written by the boundary each frame (one value drives the
  displayed seconds *and* the verb's shimmer phase). See
  `docs/status-indicator.md`.
- `TurnSummary { verb, secs, timestamp }` — the committed `"{verb} for Ns"` turn
  summary recorded at turn end.
- `HistoryItem { Message(Message), Tool(ToolCall), Summary(TurnSummary) }` — one
  ordered history entry; messages, tools, and per-turn summaries share
  `App::history` so they repaint interleaved in order.
- `StreamError { partial: Option<String>, error: String }` — what `App::fail_stream`
  hands the loop to flush after a backend failure.
- `InterruptedTurn { partial: Option<String>, tool: Option<ToolCall> }` — what
  `App::interrupt_turn` hands the loop to flush after an Esc interrupt (the
  `INTERRUPT_NOTICE` const is the committed notice text).
- `StreamEvent { Chunk(String), ToolStart{name,args}, ToolEnd{output,ok},
  ThinkingStart, ThinkingEnd, Error(String), StreamDone }` (in `stream.rs`) — what a
  backend sends to the loop (`ThinkingStart`/`ThinkingEnd` drive the live `Thinking
  for Ns`).
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
- `stream` (thinking): `turn_events` emits exactly one `ThinkingStart`/`ThinkingEnd`
  pair, before the first tool (so tool start/end stay adjacent); `DummyAi` emits it.
- `app`: typing appends; backspace; Enter with text → `Submit` + clears input;
  Alt+Enter / Shift+Enter insert a newline (box grows) without submitting; Enter
  while empty / while streaming → `None`; Esc/Ctrl+C → `Quit` when idle, while
  Esc mid-turn → `Interrupt` (palette-dismiss still wins); Ctrl+C with a
  non-empty input clears the draft instead (closing the palette, leaving a
  streaming turn untouched; from the tool view it still quits), and the next
  Ctrl+C quits;
  `push_chunk`/`finish_stream` transitions; `fail_stream` records partial + error;
  `interrupt_turn` keeps the partial, resolves a running tool as failed, records
  the notice with no summary, and is a no-op when idle.
- `app` (tools & view): `start_tool`/`end_tool` move a tool through running →
  ok/failed and into history; `flush_streaming_segment` records the text before a
  tool and reopens an empty buffer; `finish_stream` records nothing for an empty
  final segment; a turn interleaves text/tool/text in order. Ctrl+O toggles the
  view (even mid-stream, stream keeps running); Esc closes the overlay (vs quits
  in the chat); the viewer scrolls and ignores typing; it opens pinned to the
  bottom and `settle_tool_scroll` tail-follows (scrolling up disengages, reaching
  the bottom re-engages). With an injected stub clock (`set_clock`), every recorded
  message/tool is stamped with the clock's value; with no clock the stamp is empty.
- `app` (status): `begin_stream` opens a `TurnStatus` (a per-turn verb, 0 tokens,
  `↓`); the verb differs turn-to-turn; `push_chunk` grows the tally (`↓`); a tool
  *adds* its output to the tally and flips the arrow `↑` without resetting, and
  resuming text flips it back `↓`; `set_status_times` writes the boundary
  durations (no-op when idle); `end_turn` records a `Summary` and clears the status;
  `fail_stream` clears it with no summary; `estimate_tokens` grows with length.
- `app` (slash palette): `command_query` recognises a bare `/token` (rejecting
  past-a-space/newline and mid-line slashes); `matching_commands` prefix-filters
  case-insensitively; the registry has unique lowercase names. Typing `/` opens
  the palette and filters/clamps the selection; ↑/↓ move within bounds; Backspace
  past the slash closes it; Esc dismisses (not quits) and is **sticky** within the
  same token (re-entering command mode reopens it); Enter/Tab run the highlighted
  command, returning the right `Action` (`/clear`→`Clear` + history emptied,
  `/help`→`Notice` listing commands, `/quit`→`Quit`) and consuming the input; an
  empty-match Enter doesn't submit; with no palette open Enter still submits
  normally.
- `app` (input history): ↑ recalls the newest submission (cursor at the end)
  and steps older, clamping at the oldest; ↓ steps newer and clears past the
  newest; ↓ never *enters* history; a typed draft is never clobbered; editing
  a recall (or an interior cursor) returns the arrows to cursor movement, the
  text edges re-enable recall; submitting restarts browsing at the newest;
  adjacent duplicates collapse; blanks are never recorded; Ctrl+C's cleared
  draft is recallable; a recalled `/token` reopens the palette; `/clear`
  keeps the recall history.
- `app` (shortcuts band): `?` toggles it from an empty composer (shift-modified
  too) and types into a non-empty draft; any other key closes it but still
  acts (typing, ↑ recall, `/` palette); Esc only dismisses — no quit idle, no
  interrupt mid-turn (the turn is untouched); `?` is ignored in the tool view.
- `ui` (shortcuts band): `shortcuts_rows` 0 closed / entries-per-2 open;
  `shortcuts_lines` lists the bindings in two aligned columns, keys cyan and
  labels dim, the `esc` entry flipping `to quit`/`to interrupt` with the turn;
  `live_height` grows by the band; `render_live` paints it below the box;
  `cursor_position` stays put when it opens.
- `app` (message queue): Enter mid-turn queues (composer cleared, FIFO order,
  recorded for ↑ recall) while idle Enter still submits; `dequeue` pops front /
  `None` empty; Alt+Up pulls the last queued message into an empty composer and
  is a no-op against a draft or an empty queue.
- `ui` (queued band): `queued_rows` 0 empty / counts the queue / caps at
  `QUEUED_MAX_ROWS`; `queued_lines` lists dim `↳` rows, flattening newlines and
  showing a `… (+N more)` overflow line; `live_height` grows with the queue;
  `render_live` draws it below the box (and below the shortcuts band).
- `ui`: `wrap_text` (word wrap, hard-break long words, newlines, width 0, **wide
  & zero-width chars**); `message_lines` (bullet on first line, indented
  continuation; user lines carry a dark background padded to the full display
  width; error lines get a red bullet; system notices a cyan one); `tool_lines`
  (status-coloured bullet header, collapsed peek + `(ctrl+o to expand)` hint,
  width-truncated); `transcript_lines`/`render_tool_view` (full conversation —
  messages interleaved with each tool's complete output, plus the live tail —
  status colour, scroll; the **user** message's **timestamp right-aligned on its
  own line below it** in a dim colour — no stamp on assistant/tool/summary items,
  and **never** any in the inline `conversation_lines`); the
  **status indicator** — `status_line` formats each phase (`(0s)` with the token
  clause dropped at 0; `↓`/`↑` arrows; `Thinking for Ns` only when set; a
  bouncing-ball spinner — white bold ball between dim walls — that steps a frame
  per interval, reverses at the right wall, and loops; dim metrics; and a
  per-char bold greyscale-white shimmering verb whose
  crest outshines off-band chars and moves as `elapsed` advances), `summary_lines`
  is one dim bullet-less `"{verb} for
  Ns"` line, `render_live` stacks preview / gap / status / gap in the streaming
  strip, and a committed `Summary` flows through `conversation_lines`/`transcript`
  (stamp-free) like any non-user item; the
  **command palette** — `menu_window` keeps the
  selection visible, `menu_rows` reserves the band (0 closed, capped, 1 for no
  matches), `command_menu_lines` lists the matches in aligned columns and
  **highlights the whole selected row in one cyan colour** (name and description
  alike, vs dimmed grey, no caret; placeholder when empty), and `render_live` draws
  it below the box with the cursor unmoved; the
  growing-input geometry — `live_height` grows a row per wrapped line, adds the
  preview + gap + status + gap strip only while streaming and the palette band
  below the
  box, and clamps to the screen; `render_live` grows the box, scrolls the input to
  keep the end visible, stacks the streaming preview (or a running tool's blue
  header), a blank gap, the status line, then another blank gap above the box, and
  shows no strip when
  idle; `cursor_position` follows
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
  commits (to keep the box on screen), and the streaming strip (preview + gap +
  status + gap) inflates that height while a reply streams. So at `StreamDone`/`Error`
  `main` calls this first to drop the strip's rows, letting the final commit (the
  reply's last line, then the `Done for Ns` summary) replace them in place —
  otherwise `insert_before` over-scrolls and the box rises off the bottom (see
  *Known limitations*).
- `enter_overlay` / `draw_overlay` / `exit_overlay` — the Ctrl+O tool-output view.
  `enter_overlay` switches to the terminal's **alternate screen** (so the inline
  conversation — main screen + its real scrollback — is preserved untouched);
  `draw_overlay` paints a full-screen buffer (`ui::render_tool_view`) every frame;
  `exit_overlay` switches back, after which `main` reflows the inline view to catch
  up (on a normal return *and* on a quit-from-overlay, so the restored screen is
  rebuilt either way). This is the *only* use of the alternate screen — the
  conversation itself stays inline.
- `reflow` rebuilds the inline view from a re-wrapped `tail` (after a resize, a
  Ctrl+O return, or `/clear`). It lets `insert_before` **overwrite the screen in
  place** (draw top-down, then clear the rows below the tail) and `clear_region(All)`s
  *only* for an empty tail. A leading full clear before `insert_before`'s scroll
  makes tmux spill the on-screen frame into scrollback; after a Ctrl+O return that
  frame is the **stale streaming strip**, so the clear would push `Working… (…
  tokens)` into scrollback above the rebuilt conversation (`smoke.sh` Phase 7).

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
- **Streaming strip collapse.** The streaming strip (preview + gap + status + gap)
  is drawn *above* the box, so it grows the live region *upward*. When a reply finishes,
  that strip's rows are handed back to scrollback as the committed final line +
  spacer + the `Done for Ns` summary, and the box must stay put. The fix is to reseat the viewport to its idle height
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
  for now (`/help`, `/clear`, `/quit`) — adding a command is a one-line `COMMANDS`
  entry plus an effect arm in `run_selected_command`. Esc's dismissal reopens on
  the next keystroke only if you leave and re-enter command mode; and running
  `/help` mid-stream finalises the reply's current segment first (so the notice
  never splits the reply), the same ordering rule tool calls use.
- Timestamps are shown **only** in the Ctrl+O transcript, and only for the
  **user's** messages (12-hour `hh:mm AM/PM`, no seconds, right-aligned below the
  message); assistant/tool/summary items record a stamp but never display it. The
  inline conversation has none, and there is no per-token/relative time. See
  `docs/timestamps.md`.
- The status indicator's token counts are an app-side **estimate** (≈ chars/4), not
  real model usage — the dummy has no tokenizer; a real `ReplySource` could report
  exact counts later. The working/done verbs cycle deterministically (a turn
  counter), not at random. See `docs/status-indicator.md`.
- No markdown rendering or scrollback nav keys (YAGNI).
```
