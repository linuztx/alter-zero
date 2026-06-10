# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo run                                   # run the TUI
cargo test                                  # all unit tests
cargo test <name_substring>                 # run a single test by name fragment
cargo test app::tests                       # run one module's tests
cargo clippy --all-targets -- -D warnings   # lint (warnings are errors here)
cargo fmt --check                           # formatting gate
cargo build && bash scripts/smoke.sh        # drive the real binary in tmux
```

The standard pre-commit gate used throughout this project is: `cargo fmt --check`
+ `cargo clippy --all-targets -- -D warnings` + `cargo test` all clean.

Toolchain: Rust **edition 2024**, `ratatui = 0.30.1` (crossterm is re-exported as
`ratatui::crossterm` — import it from there, not as a separate crate), plus
`unicode-width` for display-width math, `unicode-segmentation` for the textarea's
grapheme-aware cursor/wrapping, and **`tokio`** (current-thread runtime) +
`tokio-stream` for the async event loop. The `Cargo.toml` `crossterm` entry exists
*only* to enable its `event-stream` feature (for `EventStream`); code still imports
crossterm through `ratatui::crossterm`, never as `crossterm::…`. `rust-toolchain.toml`
pins the toolchain; a `[lints]` table in `Cargo.toml` bakes the gate into every
build (`unsafe_code = "forbid"`, plus `warnings` and `clippy::all` denied).

## Architecture

A **library** (`src/lib.rs` → `app`, `stream`, `ui`, `term`, `frame`, `paste`,
`textarea`) holds the logic; **`src/main.rs`** is a thin terminal shell driving a
codex-style **async (tokio) `select!`** loop. The pure, unit-tested logic lives in
`app`/`stream`/`ui`/`textarea` (plus the pure cores of `frame`/`paste`) so behavior
is testable with a plain `Buffer`/`TestBackend` and no real terminal. `main.rs`
**and `term.rs`** are the I/O boundary — not unit-tested — verified via
`scripts/smoke.sh`; `frame`'s async scheduler **task** is smoke-covered too (its
rate-limit/coalesce math is unit-tested). Keep logic out of the boundary; every
geometry decision `term.rs` makes is a pure `ui` helper it calls.

The design rationale lives in `docs/design.md`; the async-loop design in
`docs/async-rewrite.md`; the editable input (textarea) design in
`docs/textarea.md`.

### The runtime model and its invariants

This is an **inline** TUI: finished messages *and tool calls* flow into the
terminal's real scrollback; a live region (a rule-framed input box — a codex-style
**`textarea`** whose cursor moves anywhere (←/→ by grapheme, ↑/↓ across *wrapped*
rows, Home/End) with insert/delete at the cursor, growing as the input wraps —
plus, *while a turn is in flight*, a strip above it — a streaming preview row (the
preview shows a running tool's blue header when one is executing), a blank gap row,
a codex-style **status line** (`● {verb}… ({elapsed}s · {↓|↑} {n} tokens · Thinking
for {m}s)` — white bullet, the verb text shimmering with a white sweep ported from
codex's `shimmer_spans`; on finish a dim `{done verb} for {n}s` summary commits to
scrollback — see `docs/status-indicator.md`), then another blank gap row so the
status clears the box's top rule — plus a
scrollable **slash-command palette** band *below* the box when the input is a bare
`/token`) stays pinned at the bottom. The alternate screen is used in exactly one
place: the **Ctrl+O tool-output view**, a full-screen overlay listing every tool
call's complete output while the conversation keeps streaming underneath (see
invariant 4). ratatui's `Viewport::Inline` can't change height after startup, so
`term::InlineViewport` is a *custom* inline viewport over a `CrosstermBackend`
whose height is **dynamic** — the input box grows with the wrapped input, the
streaming strip, and the palette band (`ui::live_height`). Four non-obvious
invariants hold the whole thing together — breaking any one reintroduces a class
of bug:

1. **One stdin reader, created after the init cursor query.** The async loop reads
   keys from a single crossterm `EventStream`; the reply backend (a
   `stream::ReplySource`, e.g. `DummyAi`) streams on a background thread that *only
   sends* `StreamEvent`s on a tokio channel. `InlineViewport::init` queries the
   cursor position (DSR) over stdin *once, synchronously, before the `EventStream`
   exists*; a second stdin reader would steal that reply and cause "cursor position
   could not be read". So: create the `EventStream` only after init, and never add
   another thread or task that reads stdin (`insert_before` tracks the viewport row
   itself and never queries the cursor).

2. **Greedy word-wrap is prefix-stable** (`ui::wrap_text`): appending text only
   ever changes the *last* wrapped line. This is what makes streaming-to-scrollback
   safe — `ui::stable_commit` flushes every line *except the last* to scrollback
   via `term::InlineViewport::insert_before` as the reply grows, tracking a
   `committed` count; `ui::final_commit` flushes the remainder on `StreamDone`. The
   test `ui::tests::incremental_commits_reconstruct_the_whole_reply` locks this
   property (committed lines + final flush == the fully-rendered message). Don't
   change the wrap algorithm without re-checking that invariant.

3. **The viewport is content-anchored (top fixed), and resize reflows both
   directions** (`main.rs::repaint_conversation`). Like Claude Code / codex, the
   box grows *downward* in place — `term::draw` keeps its top put and only scrolls
   the screen *up* (oldest chat into scrollback) once the box would overflow the
   bottom; a shrink blanks the rows it vacates (the decision is the pure
   `ui::repin`). Never force it to `screen.height - height` — that reintroduces
   the "box jumps to the bottom" bug. The streaming strip (preview + gap + status
   + gap) sits *above* the box, so it grows the region upward; when a reply ends the
   strip's rows become the committed final line + spacer + the `Done for Ns` summary
   and the box must **stay put**, so `StreamDone`/`Error` call `term::set_view_height`
   to reseat the viewport to its idle height *before* the final `insert_before` —
   skip it and `insert_before` over-scrolls, the box rises off the bottom, and blank
   rows appear beneath it (guarded by `smoke.sh` Phase 5). On a width change every wrapped line is
   stale, so `App` retains a `history: Vec<HistoryItem>` of finished messages *and
   tool calls* (kept for two reasons: this repaint, and listing tools in the Ctrl+O
   view) and `term::reflow` clears the screen, seats the viewport at the top, then
   `insert_before`s the re-wrapped tail (`ui::repaint_lines`). `committed` is reset
   so a mid-stream resize re-commits the reply.

4. **Tool calls interleave with text, and Ctrl+O opens a separate overlay.** A
   tool call splits the assistant text around it: `App::flush_streaming_segment`
   finalises the run of text before a `ToolStart` as its own history message so the
   tool slots *after* it in order (the scrollback and the resize/return repaint
   must agree). Inline a tool is collapsed (`ui::tool_lines` — coloured bullet +
   one-line peek). The Ctrl+O overlay (`ui::render_tool_view` on the alternate
   screen) shows the **full conversation transcript** — `ui::transcript_lines`
   walks `history` (messages + each tool's *expanded* output) plus the live tail
   (in-progress reply / running tool), each item's wall-clock `timestamp`
   **right-aligned** on its header (the *only* place stamps show — never inline;
   the clock is injected via `App::set_clock`, see `docs/timestamps.md`). **While
   the overlay is up the loop keeps
   draining reply events into `App` but does *not* commit to scrollback** (that
   would write into the alt screen); on return, `repaint_conversation` rebuilds the
   inline view from `history`. Never commit to scrollback while
   `app.view == View::ToolOutput`.

### Data flow

The loop is an async (`tokio`, current-thread) `select!` over three sources;
`select!`'s randomized branch order gives input/draw fairness for free. Every state
change calls `frame.schedule_frame()`; the `frame` scheduler coalesces those into a
single draw tick, rate-limited to 120 fps (`MIN_FRAME_INTERVAL`). A paste/fast-type
run is caught by `paste::PasteBurst` so its redraw defers to the burst tail
(`schedule_frame_in`). *While a turn is active* the draw branch **re-arms** the next
animation frame (`schedule_frame_in(32ms)`, codex's status-widget cadence) so the
status line's shimmer sweeps and its timer advances with no events; before each
draw the loop writes the computed `elapsed`/`thinking` `Duration`s onto the status
(`App::set_status_times`), keeping time out of the pure core (the timestamp-clock
pattern). `insert_before` stays inline (immediate scrollback); only the live-region
paint is tick-driven. (See `docs/async-rewrite.md`, `docs/status-indicator.md`.)

```
keyboard / resize ─► EventStream ─┐
reply backend ────► tokio mpsc ───┼─► select! ─► App::on_key / push_chunk / start_tool / set_status_times / … ─► schedule_frame
frame scheduler ──► draw-tick ────┘                                        coalesce + 120fps ─► draw / draw_overlay
                                       └─ turn active? re-arm a frame in 32ms (status shimmer + timer)
```

`Submit(text)` records the user message, `insert_before`s it, then spawns a reply
via the selected `ReplySource` (`backend.spawn(text, tx, cancel)`), keeping the
thread handle + `CancelToken` so a quit mid-stream cancels and reaps it. The
backend interleaves `StreamEvent::ToolStart{name,args}`/`ToolEnd{output,ok}` pairs
and a `ThinkingStart`/`ThinkingEnd` pair between `Chunk`s; the loop shows the tool
running (blue) then commits it collapsed (green/red), and flips its `thinking_start`
`Instant` so the status line shows/drops `Thinking for Ns`. `Chunk`s and a tool's
output grow the cumulative token tally on `App::status` (`↓` while replying, `↑`
right after a tool — never reset); on `StreamDone` `App::end_turn` records the
`Done for Ns` summary. A backend may send `StreamEvent::Error(msg)` instead of
`StreamDone`; the loop turns that into a red `Role::Error` notice via
`App::fail_stream` (which also clears the status). `App` (`app.rs`) is pure state +
`on_key` (dispatched per `View`); `Action`, `Role`, `Message`, `StreamError`,
`ToolStatus`, `ToolCall`, `TokenArrow`, `TurnStatus`, `TurnSummary`, `HistoryItem`,
`View` live there too.

Typing a bare `/token` opens a **slash-command palette** below the input box (a
third live-region band): `App::command_menu` holds the highlight, the registry
`app::COMMANDS` (`SlashCommand { name, description, effect }` — currently `/help`
and `/clear`) is filtered by `matching_commands`, and ↑/↓ scroll / Tab+Enter run
the highlighted command. Descriptions line up in a column, and the selection is
shown **by colour** — the whole highlighted row lights up cyan (name *and*
description the same colour) while the others are dimmed grey, no caret. A command
dispatches an `Action`
(`/clear`→`Clear`, `/help`→`Notice(String)` committed as a `Role::System`
message). Adding a command later is a one-line `COMMANDS` entry plus an effect arm
in `App::run_selected_command`; the palette/filter/scroll don't change.

## Working style

Two practices shaped this codebase — TDD and design-first. Follow them as
standing instructions.

### Test-driven development (rigid — this is how the tested crates are built)

The Iron Law: **no production code without a failing test first.** Work in
Red → Green → Refactor cycles:

1. **Red** — write one minimal test naming the behavior you want, then run it and
   **watch it fail for the right reason** (feature missing, not a typo). A test
   you didn't see fail proves nothing.
2. **Green** — write the *minimal* code to pass. No speculative features (YAGNI).
3. **Refactor** — clean up with tests staying green.

If you wrote production code before its test, delete it and re-derive it from the
test. `main.rs` is the **only** exception (terminal I/O boundary) — verify changes
there by running the app and `scripts/smoke.sh` in tmux, not unit tests. Every
gate (`fmt`, `clippy -D warnings`, `test`) must be clean before you call work done.

### Design before implementing (for features / non-trivial changes)

"Add X" / "build X" states *what*, not "skip the design." Before non-trivial work:
explore the existing code, ask clarifying questions one at a time, propose 2–3
approaches with a recommendation, and get agreement on a design first. Capture the
agreed design in `docs/` and keep `docs/design.md` updated when behavior changes
(it already documents the architecture and known limitations). Trivial,
self-evident edits and bug fixes with an obvious cause don't need this ceremony —
but bug fixes still get a failing test first (TDD applies to fixes too).

## Conventions

- **All styling is centralized** as `const`s at the top of `ui.rs` — bullets,
  prompt, colours (including the red error bullet and the cyan system bullet),
  border, the tool-call styling (`TOOL_*` — blue/green/red status colours, the
  `⎿` peek prefix, the `(ctrl+o to expand)` hint), tool-view chrome
  (`TOOL_VIEW_*`), the transcript timestamp (`TIMESTAMP_COLOR`/`STAMP_GAP` — the
  dim, right-aligned per-item stamp shown only in the Ctrl+O view), the status
  indicator (`STATUS_*` — the white bullet, dim metrics, the `↓`/`↑` arrows
  and `…` ellipsis, the dim committed-summary colour, and `STATUS_ROWS`/`STATUS_GAP_ROWS`;
  the verb's white shimmer wave is the `SHIMMER_*` consts — base/highlight
  colours, sweep period, padding, band half-width, max blend — a port of codex's
  `shimmer_spans`; the verbs themselves are `WORKING_VERBS`/`DONE_VERBS` in
  `app.rs`, picked per-turn), the
  slash-command palette (`MENU_*` — the `MENU_DESC_COL`
  description column, the cyan/dimmed colours that light up the whole selected row
  — name and description alike — and the `MENU_MAX_ROWS` cap), and
  the live-region row geometry (`PREVIEW_ROWS`/`GAP_ROWS`/`STATUS_ROWS`/`STATUS_GAP_ROWS`/`INPUT_CHROME_ROWS`/`LIVE_MIN_HEIGHT`;
  the preview + gap + status + gap strip shows *only while a turn streams* — `strip_rows`
  (`render_live` draws the status line under the preview's gap, a blank row above
  the box) — and the
  command palette is a third band *below* the box — `menu_rows` — so the box's
  dynamic `live_height` is streaming- and palette-aware, and idle with no palette
  there is exactly one blank above the box: the committed spacer after the last
  message. `render_live` and `cursor_position` share the `input_box` helper, which
  reserves the menu band so the cursor stays put when the palette opens; `tool_lines`
  and `tool_view_lines` share `tool_header`). Retheme or re-size there, not inline.
- **All width math goes through `cols()`** (display columns via `unicode-width`),
  never `chars().count()` — so CJK/emoji wrap and pad correctly.
- **The input line is a `textarea::TextArea`, not a `String`.** Route all editing
  through it (`insert_char`/`delete_backward`/`move_*`/`take`/…), never raw string
  `push`/`pop`; read it with `.text()`. Its cursor is a byte offset on a grapheme
  boundary and the wrap cache is filled by the render path (`wrapped_rows`), which
  is why `App::on_key` (and so `move_up`/`move_down`) stays width-agnostic. The
  textarea wraps faithfully (preserving spaces) into byte ranges — distinct from
  `ui::wrap_text`, which is for **messages** and collapses whitespace. See
  `docs/textarea.md`.
- **Swapping in a real AI** means implementing `stream::ReplySource` (use `DummyAi`
  as a template) and changing the single `let backend = …;` line in
  `main.rs::run`. Stream `StreamEvent::Chunk(..)` per token on the `tokio`
  `UnboundedSender` (its `send` is sync — callable straight from your background
  thread, no runtime needed), poll the `CancelToken` so a quit can stop you, then
  send `StreamEvent::StreamDone` — or `StreamEvent::Error(msg)` on failure. For tool calls, send a
  `StreamEvent::ToolStart{name,args}` then a `ToolEnd{output,ok}`; wrap a reasoning
  phase in a `ThinkingStart`/`ThinkingEnd` pair to drive the `Thinking for Ns`
  status (see `stream::turn_events` for the dummy's interleaved script). The loop and
  rendering treat chunks and tool output as opaque text, and estimate the status
  token counts app-side (no usage reporting in the protocol); nothing else changes.
