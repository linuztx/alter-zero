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
`textarea`, `file_search`, `clipboard`) holds the logic; **`src/main.rs`** is a thin terminal
shell driving a
codex-style **async (tokio) `select!`** loop. The pure, unit-tested logic lives in
`app`/`stream`/`ui`/`textarea`/`file_search` (plus the pure cores of `frame`/`paste`) so behavior
is testable with a plain `Buffer`/`TestBackend` and no real terminal. `main.rs`
**and `term.rs`** are the I/O boundary (as is `clipboard.rs`'s Ctrl+V read) — verified via `scripts/smoke.sh`, not
unit-tested save for the odd pure helper that has no terminal in it (like
`term`'s `keyboard_enhancement_disabled` env predicate — see
`docs/shift-enter.md`); `frame`'s async scheduler **task** is smoke-covered too
(its rate-limit/coalesce math is unit-tested). Keep logic out of the boundary;
every geometry decision `term.rs` makes is a pure `ui` helper it calls.

The design rationale lives in `docs/design.md`; the async-loop design in
`docs/async-rewrite.md`; the editable input (textarea) design in
`docs/textarea.md`; the Esc-interrupt design in `docs/interrupt.md`; the ↑/↓
input-history recall in `docs/input-history.md`; the Ctrl+R reverse search over
that history in `docs/history-search.md`; the `!` local shell commands in
`docs/shell-command.md`; the `?` shortcuts band in
`docs/shortcuts.md`; the Shift+Enter / Ctrl+J newline keys in
`docs/shift-enter.md`; the mid-turn message queue in `docs/queue.md`; the
session-context footer in `docs/footer.md`; the flicker-free frame pipeline
(scrollback commits deferred into the draw's synchronized update) in
`docs/flicker.md`; the `@` file-path picker (async walk+rank file search below
the box) in `docs/file-search.md`; the large-paste `[Pasted Content N chars]`
placeholder (bracketed paste → a compact placeholder, expanded back on send) in
`docs/paste.md`; the **Ctrl+V image paste** (clipboard image → temp PNG → an
`[Image #N]` composer placeholder whose path rides a separate typed channel to
the backend) in `docs/image-paste.md`.

### The runtime model and its invariants

This is an **inline** TUI: finished messages *and tool calls* flow into the
terminal's real scrollback; a live region (a rule-framed input box — a codex-style
**`textarea`** whose cursor moves anywhere (←/→ by grapheme, ↑/↓ across *wrapped*
rows, Home/End) with insert/delete at the cursor, growing as the input wraps;
from an **empty composer (or an unedited recall) ↑/↓ instead step through
previously submitted inputs** shell-style (`App::input_history`, codex's
`ChatComposerHistory` — ↓ past the newest clears; see `docs/input-history.md`),
and **Ctrl+R reverse-searches them** codex-style (`App::history_search` — the
footer slot becomes a `reverse-i-search: {query}` line owning **every** key,
the newest case-insensitive substring match previews in the composer with the
query occurrences highlighted, Ctrl+R/↑ and Ctrl+S/↓ step older/newer clamping
at the ends, Enter accepts the preview as an editable draft seating ↑ at it,
Esc/Ctrl+C cancel restoring the pre-search draft and cursor; see
`docs/history-search.md`) —
plus, *while a turn is in flight*, a strip above it — a streaming preview row (the
preview shows a running tool's blue header when one is executing), a blank gap row,
a codex-style **status line** (`( ●    ) {verb}… ({elapsed}s · {↓|↑} {n} tokens ·
Thinking for {m}s · esc to interrupt)` — opened by a bouncing-ball spinner (cli-spinners'
`bouncingBall`: a white ball ping-ponging between dim walls), the verb text
shimmering with a white sweep ported from
codex's `shimmer_spans`; on finish a dim `{done verb} for {n}s` summary commits to
scrollback, while **Esc mid-turn interrupts** instead (codex-style — cancel + reap
the backend, drain the channel, keep the partial, resolve a running tool as
failed, commit the red `Conversation interrupted` notice, **no** summary; see
`docs/interrupt.md`; Esc only quits when idle) — see `docs/status-indicator.md`),
then another blank gap row so the
status clears the box's top rule — plus a
scrollable **slash-command palette** band *below* the box when the input is a bare
`/token` (the same slot shows a **`?` shortcuts band** — codex's footer shortcut
overlay, two dim columns of `{key} for {thing}` entries — when `?` is pressed in
an empty composer; any other key dismisses it, Esc dismiss-only; see
`docs/shortcuts.md`; **and the same slot shows an `@` file picker** — a fuzzy
file list — whenever the cursor is in an `@token`: the boundary's background
worker walks the cwd once and ranks it per query off-thread (codex's
`StartFileSearch`/`FileSearchResult` round-trip — `App::file_search_query`
changes drive a `dispatch_file_search`, results come back via
`App::set_file_matches` with a staleness guard), ↑/↓ move and **Tab/Enter insert
the path** (replacing the `@token`, a trailing space added, whitespace paths
quoted), Esc dismisses sticky-per-token; the matched characters are bolded in
each row; suppressed in `!` shell mode and mutually exclusive with the palette;
see `docs/file-search.md`); plus, *above* the box while a turn streams, **messages
submitted with Enter queue** instead of waiting (shown like sent user messages,
inset two columns — `  ❯ {msg}` rows in the strip under the status line —
`App::queued`, a `VecDeque<QueuedTurn>` of **typed entries** (text `Messages`
batches and standalone `Shell` commands — codex's action-tagged
`queued_user_messages`): **Enter appends to the current batch** (those messages
auto-sent together as one next turn) while **Tab opens a new batch** (codex's
Tab-to-queue — its message runs as a *separate follow-up turn* after the first
queue, a blank row dividing them), the loop dispatching one entry per turn end
(a text batch to the model, a queued `!command` run locally) —
**Esc interrupts and sends the front batch right away**, Alt+Up pulls the **last
batch** (its messages newline-joined) back into the composer to edit, leaving
earlier batches queued; see
`docs/queue.md`; plus **`!command` runs a local shell command** (codex's `!`
shell mode: a leading `!` is **absorbed** into `App::shell_mode` and rendered
back as the composer's red `! ` prompt — `! pwd`, never `❯ !pwd` — with a red
`Shell mode` hint in the footer slot (Backspace/Esc on the empty shell composer
exit the mode; the palette/`?` band are suppressed in it); Enter from an idle
composer runs the draft under `sh -c` on a background thread as a turn
(`App::begin_shell`) committing a codex-style **exec cell** — the `! command`
header on the dark user-style line (a `Role::Shell` message) with the `⎿`
output **flush** below, `⎿ Running…` while it runs, the `Running…`/`esc to
interrupt` status, **no** `Ran for Ns` summary, Esc killing the child;
mid-turn it queues as a standalone `Shell` entry run locally when its turn comes
— codex parity, never merged into a text batch, Alt+Up over it re-enters shell
mode; see `docs/shell-command.md` and `docs/queue.md`); plus a one-row
**session footer** on the region's last row —
codex's footer status line, `{model} · {cwd}` dim and two-space inset
(`dummy_model_name · ~/repo`) — whenever no band is open (the palette/shortcuts
band displaces it, and the Ctrl+R search line / `!` shell-mode hint take its
slot; `App::set_session_info` injects the strings at the boundary
like the clock, the model name coming from `ReplySource::model_name`; see
`docs/footer.md`) stays pinned at the bottom. The alternate screen is used in exactly one
place: the **Ctrl+O tool-output view**, a full-screen overlay listing every tool
call's complete output while the conversation keeps streaming underneath (see
invariant 4). ratatui's `Viewport::Inline` can't change height after startup, so
`term::InlineViewport` is a *custom* inline viewport over a `CrosstermBackend`
whose height is **dynamic** — the input box grows with the wrapped input, the
streaming strip, the palette band, and the session footer (`ui::live_height`). Four non-obvious
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
   to reseat the viewport to its idle height *before* the final `insert_before`s —
   the queued lines flush with the *latest* tracked height, so skipping the reseat
   makes the flush over-scroll, the box rise off the bottom, and blank rows appear
   beneath it (guarded by `smoke.sh` Phase 5). (`insert_before` itself only
   **queues**: the next `term::draw` writes the lines and repaints the live region
   inside one synchronized update, so a commit can never flash a boxless frame —
   `docs/flicker.md`, guarded by `smoke.sh` Phase 15.) On **any** size change the
   conversation repaints from source — a width change stales every wrapped line, and
   a height-only change moves the screen contents out from under the tracked
   viewport row (the emulator scrolls/clips to fit; repainting at the stale row
   leaves phantom input boxes — `term::resized` re-clamps the viewport like codex,
   and the repaint reseats it; guarded by `smoke.sh` Phase 17). `App` retains a
   `history: Vec<HistoryItem>` of finished messages *and
   tool calls* (kept for two reasons: this repaint, and listing tools in the Ctrl+O
   view) and `term::reflow` seats the viewport at the top, writes the re-wrapped
   tail (`ui::repaint_lines`) **overwriting the screen in place** (top-down, then
   clearing the rows below the tail), and paints the live region below it — all in
   the same synchronized frame (stale queued lines are dropped: the tail regenerates
   them). `reflow` only `clear_region(All)`s for an *empty* tail (`/clear`): a leading full
   clear before the tail-write's scroll makes tmux spill the on-screen frame into
   scrollback — harmless on a resize, but after a Ctrl+O return (invariant 4) it
   pushes the **stale streaming strip** (`Working… (… tokens)`) into scrollback above
   the rebuilt conversation (guarded by `smoke.sh` Phase 7). `committed` is reset so
   a mid-stream resize re-commits the reply.

4. **Tool calls interleave with text, and Ctrl+O opens a separate overlay.** A
   tool call splits the assistant text around it: `App::flush_streaming_segment`
   finalises the run of text before a `ToolStart` as its own history message so the
   tool slots *after* it in order (the scrollback and the resize/return repaint
   must agree). Inline a tool is collapsed (`ui::tool_lines` — coloured bullet +
   one-line peek). The Ctrl+O overlay (`ui::render_tool_view` on the alternate
   screen) shows the **full conversation transcript** — `ui::transcript_lines`
   walks `history` (messages + each tool's *expanded* output) plus the live tail
   (in-progress reply / running tool). Only the **user** message shows its
   wall-clock `timestamp` (`hh:mm AM/PM`, no seconds): dim, **right-aligned on
   its own line below the message** — the *only* stamp displayed anywhere
   (AI/tool/summary stamps are recorded but never shown; never inline; the
   clock is injected via `App::set_clock`, see `docs/timestamps.md`). **While
   the overlay is up the loop keeps
   draining reply events into `App` but does *not* commit to scrollback** (that
   would write into the alt screen); on return, `repaint_conversation` rebuilds the
   inline view from `history`. Never commit to scrollback while
   `app.view == View::ToolOutput`. **Quitting from the overlay is also a return**:
   the `Action::Quit` arm must `exit_overlay` *then* `repaint_conversation` + `draw`
   before breaking — otherwise `restore` lands on the stale streaming strip a turn
   that finished in the overlay left behind, instead of the committed `Done for Ns`
   summary (`smoke.sh` Phase 7).

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
pattern). `insert_before` only **queues** its lines: the draw tick writes them and
repaints the live region in **one synchronized frame** (codex's pending-history
pattern — no flushed state ever lacks the box, the flicker fix; `reflow` paints the
same way, and `restore` flushes any quit-before-tick leftovers; `docs/flicker.md`).
(See `docs/async-rewrite.md`, `docs/status-indicator.md`.)

```
keyboard / resize ─► EventStream ─┐
reply backend ────► tokio mpsc ───┼─► select! ─► App::on_key / push_chunk / start_tool / set_status_times / … ─► schedule_frame
frame scheduler ──► draw-tick ────┘                                        coalesce + 120fps ─► draw / draw_overlay
                                       └─ turn active? re-arm a frame in 32ms (status shimmer + timer)
```

`Submit(text)` runs `main.rs::start_turn` — a batch of one: it records each
user message (the Enter arm also records the text into `App::input_history` for
the ↑/↓ shell-style recall — adjacent duplicates collapse, `/clear` doesn't
touch it), `insert_before`s it, then spawns one reply via the selected
`ReplySource` (`backend.spawn(texts.join("\n"), images, tx, cancel)` — the
Ctrl+V image paths drained from `App::take_submission_images`, see
`docs/image-paste.md`), keeping the
thread handle + `CancelToken` so a quit mid-stream cancels and reaps it.
**Enter *while a turn is in flight* queues** the message into `App::queued`
(codex's `queued_user_messages`) instead of producing `Submit` — shown like
sent user messages (`❯` rows) in the strip above the box. The queue is a
sequence of **typed entries** (`QueuedTurn`): Enter appends to the last
`Messages` batch (`queue_draft(false)`), **Tab opens a new batch**
(`queue_draft(true)`, a separate follow-up turn), and a **mid-turn `!command`
queues as a standalone `Shell` entry** (`queue_shell`, run locally, never
merged); `main.rs::flush_next_queued` dispatches **one entry**
(`drain_next_batch`) per turn end — a `Messages` batch via `start_turn`, a
`Shell` via `run_shell` — so Enter messages batch into one turn while Tab
follow-ups and `!` commands iterate in order (Alt+Up pulls the **last entry**
(`drain_last_batch`, `pop_back`) back into the composer to edit — a `Messages`
batch newline-joined, a `Shell` entry as `!command` re-entering shell mode —
earlier entries stay queued; see `docs/queue.md`). The
backend interleaves `StreamEvent::ToolStart{name,args}`/`ToolEnd{output,ok}` pairs
and a `ThinkingStart`/`ThinkingEnd` pair (with opaque `ThinkingChunk` reasoning
deltas streamed in between) between `Chunk`s; the loop shows the tool
running (blue) then commits it collapsed (green/red), and flips its `thinking_start`
`Instant` so the status line shows/drops `Thinking for Ns`. The just-sent
**user message is counted up front** (`App::count_user_input` after
`begin_stream`, arrow `↑` — uploaded input), so the status shows `↑ N tokens`
through the backend's **pre-stream pause** (`DummyAi` waits `STARTUP_DELAY`/3s
before its first chunk so the indicator is visibly working first — overridable
via `INLINE_TUI_STARTUP_DELAY_MS`; the strip reserves **no preview row** while
there's nothing to preview — `ui::strip_has_preview` — so the pause is status +
gap only, no stray empty line, like codex). Then `Chunk`s,
`ThinkingChunk`s (counted via `App::push_thinking` — never rendered), and a tool's
output grow the cumulative token tally on `App::status` (`↓` while replying or
thinking, `↑` for the input and right after a tool — never reset); on `StreamDone` `App::end_turn`
records the `Done for Ns` summary. A backend may send `StreamEvent::Error(msg)` instead of
`StreamDone`; the loop turns that into a red `Role::Error` notice via
`App::fail_stream` (which also clears the status). **Esc while the turn is in
flight returns `Action::Interrupt`** (palette-dismiss still wins; Esc only quits
when idle): the loop cancels + joins the backend, **drains the channel** (a stale
`ToolStart` would wedge a phantom running tool), and `App::interrupt_turn` keeps
the partial, resolves a running tool as failed (`Interrupted by user`), records
the red `INTERRUPT_NOTICE`, and clears the status with no summary
(`docs/interrupt.md`). On any turn-end — `StreamDone`, `Error`, *or* the Esc
interrupt — the loop pops the front queued batch (`App::drain_next_batch`) and
`start_turn`s it as the next turn (the remaining batches iterate at the
following turn-ends), so **Esc sends the front batch right away**; a turn that
ends under the Ctrl+O overlay defers its flush to the return (invariant 4).
`App` (`app.rs`) is pure state +
`on_key` (dispatched per `View`); `Action`, `Role`, `Message`, `StreamError`,
`InterruptedTurn`, `ToolStatus`, `ToolCall`, `TokenArrow`, `TurnStatus`,
`TurnSummary`, `HistoryItem`, `QueuedTurn`, `FileSearch`, `View` live there too
(the `@`-picker primitives `AtToken`/`FileMatch`/`at_token`/`fuzzy_match`/`rank_files`
live in the pure `file_search` module).

Typing a bare `/token` opens a **slash-command palette** below the input box (a
third live-region band): `App::command_menu` holds the highlight, the registry
`app::COMMANDS` (`SlashCommand { name, description, effect }` — currently `/help`,
`/clear`, `/copy`, and `/quit`) is filtered by `matching_commands`, and ↑/↓ scroll / Tab+Enter run
the highlighted command. Descriptions line up in a column, and the selection is
shown **by colour** — the whole highlighted row lights up cyan (name *and*
description the same colour) while the others are dimmed grey, no caret. A command
dispatches an `Action`
(`/clear`→`Clear`, `/help`→`Notice(String)` committed as a `Role::System`
message, `/quit`→`Quit`, **`/copy`→`Copy(Option<String>)`** — codex's `/copy`:
the pure core picks the last assistant message (`App::last_assistant_text`) and
the loop writes it to the system clipboard, arboard with an OSC 52 fallback for
headless/SSH/tmux, committing a `Copied last message to clipboard` system notice
or a red `No agent response to copy`/`Copy failed` error; the clipboard write is
the I/O boundary, the `base64`/OSC 52 framing a tested pure core in `clipboard`;
see `docs/copy.md`). **`/clear` mid-turn is a kill**, not codex's
"disabled while a task is in progress" rejection: `App::clear_conversation`
wipes history, the streaming buffer, the running tool, the status, and the
queued backlog (recording no partial/notice/summary; ↑-recall survives), and
the loop's `Clear` arm cancels + reaps the backend and drains its channel —
the Esc-interrupt dance minus the commits — before the blank repaint, so
nothing can stream into the cleared screen (`smoke.sh` Phase 16). Adding a command later is a one-line `COMMANDS` entry plus an effect arm
in `App::run_selected_command`; the palette/filter/scroll don't change. **Ctrl+C
first clears a non-empty input** (codex's composer-clear: one press empties the
draft — recording it in `App::input_history` so ↑ brings it back, closing the
palette, never touching a streaming turn — and only an empty-input Ctrl+C quits;
an open Ctrl+R search wins over both: that Ctrl+C only cancels the search,
restoring the pre-search draft; the Ctrl+O overlay has no input box, so Ctrl+C
there always quits).

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
  (`TOOL_VIEW_*`), the transcript timestamp (`TIMESTAMP_COLOR` — the dim
  `hh:mm AM/PM` stamp right-aligned on its own line under the *user* message,
  the only stamp shown, only in the Ctrl+O view), the status
  indicator (`STATUS_*` — the bouncing-ball spinner's white ball + dim walls and
  the `SPINNER_FRAMES`/`SPINNER_INTERVAL` animation, dim metrics, the `↓`/`↑` arrows
  and `…` ellipsis, the `STATUS_INTERRUPT_HINT` (`esc to interrupt`, the detail's
  closing clause), the dim committed-summary colour, and `STATUS_ROWS`/`STATUS_GAP_ROWS`;
  the verb's white shimmer wave is the `SHIMMER_*` consts — base/highlight
  colours, sweep period, padding, band half-width, max blend — a port of codex's
  `shimmer_spans`; the verbs themselves are `WORKING_VERBS`/`DONE_VERBS` in
  `app.rs`, picked per-turn), the
  slash-command palette (`MENU_*` — the `MENU_DESC_COL`
  description column, the cyan/dimmed colours that light up the whole selected row
  — name and description alike — and the `MENU_MAX_ROWS` cap), the `@` file
  picker (`FILE_MENU_*` — it reuses the palette's `MENU_SELECTED_COLOR`/
  `MENU_DIM_COLOR`, additionally bolding the query-matched characters, with a
  `FILE_MENU_MAX_ROWS` cap and the `FILE_MENU_SEARCHING`/`FILE_MENU_NO_MATCH`
  placeholder rows; `file_menu_rows`/`file_menu_lines`/`file_menu_row` mirror the
  palette helpers — see `docs/file-search.md`), the `?` shortcuts
  band (`SHORTCUTS*` — the entry list, the second-entry column, and the cyan
  key / dim label colours), the queued entries (the `QUEUED_INDENT` two-space
  inset, `queued_rows`/`queued_lines` — uncapped; a text `Messages` batch
  rendered by `message_lines(Role::User…)` and a standalone `Shell` command by
  `message_lines(Role::Shell…)` (the red `! ` header), so they reuse the
  user-/shell-message style, with a blank row dividing each entry from the next),
  the
  session footer (`FOOTER_*` — the two-space `FOOTER_INDENT`, the ` · `
  `FOOTER_SEPARATOR`, the dim `FOOTER_COLOR`; `footer_rows`/`footer_line`,
  ellipsis-truncated at narrow widths, with `display_cwd` formatting the
  `~`-relative path), the Ctrl+R search line that takes the footer's slot while
  a search is open (`SEARCH_*` — the dim `SEARCH_PROMPT`, the cyan
  `SEARCH_QUERY_COLOR` shared by the bold accept/cancel hint keys, the red
  `SEARCH_NO_MATCH` notice, and `SEARCH_HIGHLIGHT` — the reversed+bold styling
  of the query occurrences in the previewed match; `search_line`, the
  query-end cursor in `cursor_position`, `highlight_row_spans`), the `!`
  shell mode (`SHELL_MODE_*`/`SHELL_BULLET` — the red `Shell mode` footer
  hint (`shell_mode_line`) and the red `! ` that doubles as the composer
  prompt while `App::shell_mode` is on and as the `Role::Shell` exec-cell
  header bullet in `message_lines`; shell `tool_lines`/`tool_full_lines` are
  headerless `⎿` blocks — inline up to `TOOL_PEEK_LINES` aligned rows
  (`result_row` does the corner/continuation indent) then `… +N lines (ctrl+o
  to expand)`, `⎿ Running…` live, the retained output uncapped in the Ctrl+O view;
  output over `main.rs`'s `SHELL_OUTPUT_MAX_BYTES` is **capped in memory** as it's
  read (`main.rs::read_capped`, codex's pattern — bounds peak RSS so `! tree ~/`
  can't spike memory; the dropped tail is gone, not saved) and the expanded cell
  appends a dim `TOOL_TRUNCATED_MARKER` (`…`) when `tool.truncated` —
  kept flush by `conversation_lines`), and
  the live-region row geometry (`PREVIEW_ROWS`/`GAP_ROWS`/`STATUS_ROWS`/`STATUS_GAP_ROWS`/`INPUT_CHROME_ROWS`/`LIVE_MIN_HEIGHT`;
  the status + gap strip shows *while a turn is active*, and the preview + gap
  is added *only when there's content to preview* (`strip_rows(streaming,
  has_preview)`/`strip_has_preview` — a running tool or a non-empty reply; the
  pre-stream pause reserves **no** empty preview row, like codex) —
  (`render_live` draws the status line under the preview's gap, or at the strip
  top during the pause) — with the
  **queued messages stacked below the status, *above* the box** (`queued_rows`,
  user-message style), the
  command palette *or* the shortcuts band forms the band *below* the box —
  `menu_rows` + `shortcuts_rows` — and the session footer takes the region's
  **last** row whenever no band is open (`footer_rows` — the band displaces
  it) — so the box's
  dynamic `live_height` is streaming-, queue-, band- and footer-aware, and idle with no band
  there is exactly one blank above the box: the committed spacer after the last
  message. `render_live` and `cursor_position` share the `input_box` helper, which
  reserves the band and footer so the cursor stays put when they open; `tool_lines`
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
  `main.rs::run`. `spawn(prompt, images, tx, cancel)` hands you the text prompt
  **plus** the paths of any Ctrl+V-pasted images (`images: Vec<PathBuf>` — codex's
  `UserInput::LocalImage` typed channel; a real vision backend reads each file and
  attaches it, the dummy only acknowledges the count — see `docs/image-paste.md`).
  Stream `StreamEvent::Chunk(..)` per token on the `tokio`
  `UnboundedSender` (its `send` is sync — callable straight from your background
  thread, no runtime needed), poll the `CancelToken` so a quit can stop you, then
  send `StreamEvent::StreamDone` — or `StreamEvent::Error(msg)` on failure. For tool calls, send a
  `StreamEvent::ToolStart{name,args}` then a `ToolEnd{output,ok}`; wrap a reasoning
  phase in a `ThinkingStart`/`ThinkingEnd` pair to drive the `Thinking for Ns`
  status, streaming each reasoning delta as a `ThinkingChunk(text)` in between so
  the token tally keeps ticking while the model thinks (the text is never shown —
  only counted; see `stream::turn_events` for the dummy's interleaved script). Return
  your real model id from `model_name()` — the session footer under the box
  displays it. A real backend's own first-token latency replaces `DummyAi`'s
  artificial `STARTUP_DELAY` (the deliberate pre-stream pause that shows off the
  status indicator); the loop already counts the user's input into the tally
  (`↑`) at turn start, so the status reads `↑ N tokens` until your first chunk.
  The loop and
  rendering treat chunks and tool output as opaque text, and estimate the status
  token counts app-side (no usage reporting in the protocol); nothing else changes.
