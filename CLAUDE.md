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
`session`, `history`, `textarea`, `file_search`, `clipboard`, `context`) holds the logic; **`src/main.rs`** is a thin terminal
shell driving a
codex-style **async (tokio) `select!`** loop. The pure, unit-tested logic lives in
`app`/`stream`/`ui`/`textarea`/`file_search`/`session`/`history`/`context` (plus the pure cores of `frame`/`paste`) so behavior
is testable with a plain `Buffer`/`TestBackend` and no real terminal. `main.rs`
**and `term.rs`** are the I/O boundary (as is `clipboard.rs`'s Ctrl+V read, the
`/resume` session recording + dir scan — `main.rs::SessionRecorder`/`list_sessions`,
whose JSONL format/parse core is the pure `session` module — and the
cross-session input-history file — `main.rs::InputHistoryStore`, whose JSONL
format/parse core is the pure `history` module, `docs/history-persistence.md`) — verified via `scripts/smoke.sh`, not
unit-tested save for the odd pure helper that has no terminal in it (like
`term`'s `keyboard_enhancement_disabled` env predicate — see
`docs/shift-enter.md`); `frame`'s async scheduler **task** is smoke-covered too
(its rate-limit/coalesce math is unit-tested). Keep logic out of the boundary;
the geometry *policy* `term.rs` acts on — live-region height, the box's re-pin,
the cursor seat — comes from pure `ui` helpers it calls (`ui::live_height`,
`ui::repin`, `ui::cursor_position`, `ui::restore_cursor_row`); only the viewport
bookkeeping (`init`'s anchor math, `write_above`'s scroll plan, `resized`'s
re-clamp) is its own, smoke-covered.

The design rationale lives in `docs/design.md`; the async-loop design in
`docs/async-rewrite.md`; the editable input (textarea) design in
`docs/textarea.md`; the Esc-interrupt design in `docs/interrupt.md`; the ↑/↓
input-history recall in `docs/input-history.md`; the Ctrl+R reverse search over
that history in `docs/history-search.md`; the **cross-session persistence** of
that input history (an on-disk `history.jsonl` seeding `App::input_history` at
startup, so ↑/↓ recall *and* Ctrl+R span sessions) in
`docs/history-persistence.md`; the `!` local shell commands in
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
the backend) in `docs/image-paste.md`; the **Esc-Esc backtrack** (edit a
previous user message: prime → transcript preview → rewind + prefill) in
`docs/backtrack.md`; the **`/resume` session picker** (every conversation
recorded to a rollout JSONL file, listed in a full-screen picker whose Enter
loads it back and appends the turns that follow to the same file) in
`docs/resume.md`; the **parallel tool-call batch** (the model's several tool
calls in one round announced up front so the running one shows live while the
not-yet-run ones show `⎿ Waiting…`, executed sequentially) in
`docs/parallel-tools.md`; the **live-streaming `bash` tool** (a running command
tails its output — the last lines + a `+N lines (Ns)` footer — via a
`StreamEvent::ToolOutput` channel, collapsing to the head peek `… +N lines
(ctrl+o to expand)` when it finishes, Claude-Code style) in
`docs/tool-streaming.md`.

### The runtime model and its invariants

This is an **inline** TUI: finished messages *and tool calls* flow into the
terminal's real scrollback; a live region (a rule-framed input box — a codex-style
**`textarea`** whose cursor moves anywhere (←/→ by grapheme, ↑/↓ across *wrapped*
rows, Home/End) with insert/delete at the cursor, growing as the input wraps;
from an **empty composer (or an unedited recall) ↑/↓ instead step through
previously submitted inputs** shell-style (`App::input_history`, codex's
`ChatComposerHistory` — ↓ past the newest clears; **persisted across sessions**
in an on-disk `history.jsonl` seeded at startup, `docs/history-persistence.md`;
see `docs/input-history.md`),
and **Ctrl+R reverse-searches them** codex-style (`App::history_search` — the
footer slot becomes a `reverse-i-search: {query}` line owning **every** key,
the newest case-insensitive substring match previews in the composer with the
query occurrences highlighted, Ctrl+R/↑ and Ctrl+S/↓ step older/newer clamping
at the ends, Enter accepts the preview as an editable draft seating ↑ at it,
Esc/Ctrl+C cancel restoring the pre-search draft and cursor; see
`docs/history-search.md`) —
plus, *while a turn is in flight*, a strip above it — a streaming preview row (the
preview shows a running tool's blue cell when one is executing — a backend tool's
**whole** collapsed cell, the wrapped `● name(args)` header *plus* its output;
before any output a `⎿ Running…` row, and once a `bash` command **streams** it
**tails** its output — the last `TOOL_PEEK_LINES` lines + a `+N lines (Ns)`
footer (`ui::running_command_lines`, `docs/tool-streaming.md`) — so a long
command isn't clipped and the running state shows; a **parallel
batch** previews the *whole* `tool_queue` — the running call over each dim
`⎿ Waiting…` sibling, blank-separated, `docs/parallel-tools.md`; the preview slot
is sized by `ui::preview_rows`, a running `!` shell/streaming reply staying one
row; `docs/tools.md`), a blank gap row,
a codex-style **status line** (`(●•·   ) {verb}… ({elapsed}s · {↓|↑} {n} tokens ·
Thinking for {m}s · esc to interrupt)` — opened by a comet spinner (a
Larson-scanner sweep: a white head dragging a fading grey tail back and forth
between dim walls), the verb text
shimmering with a white sweep ported from
codex's `shimmer_spans`; on finish a dim `{done verb} for {n}s` summary commits to
scrollback, while **Esc mid-turn interrupts** instead (codex-style — cancel + reap
the backend, drain the channel, keep the partial, resolve a running tool as
failed, commit the red `Conversation interrupted` notice, **no** summary — but
when **nothing had streamed** (no partial, no tool, empty queue) it instead
**undoes** the submission, the message back in the composer and no notice, and a
**`!` shell interrupt** commits no notice either (its `⎿ Interrupted by user`
cell is the record); see
`docs/interrupt.md`; **idle Esc instead arms the Esc-Esc backtrack** — a second
Esc previews previous user messages in the transcript overlay and Enter rewinds
the conversation to the highlighted one, its text back in the composer
(`App::backtrack`, codex's `BacktrackState`; see `docs/backtrack.md`) — Esc
quits only with an empty composer and no user message to backtrack to, a
typed draft making it a codex-style no-op) — see `docs/status-indicator.md`),
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
output **flush** below, `⎿ Running… (Ns)` while it runs (the elapsed rides the
preview — a shell turn **hides the spinner status line** entirely,
`ui::strip_has_status`), **no** `Ran for Ns` summary, Esc killing the child
(resolving `⎿ Interrupted by user` with **no** `Conversation interrupted`
notice — the cell is the record);
mid-turn it queues as a standalone `Shell` entry run locally when its turn comes
— codex parity, never merged into a text batch, Alt+Up over it re-enters shell
mode; see `docs/shell-command.md` and `docs/queue.md`); plus a **transient toast**
— a one-row, self-clearing status line pinned at the *bottom of the strip, just
above the box's top rule* (`App::toast`/`ui::toast_rows`/`toast_line`, threaded
beside `queued_rows`; dim for info, red for errors). It's raised for
confirmations and soft rejections the user should see but never keep — `/copy`
(`Copied last message to clipboard`), a model switch, a mid-turn `/resume`/`/help`
rejection — instead of committing a scrollback bullet; it never enters `history`,
and its expiry is timed at the boundary (`main.rs`'s `toast_deadline` +
`present_toast`, the timestamp pattern, cleared in the draw tick). See
`docs/toast.md`; plus a one-row
**session footer** on the region's last row —
codex's footer status line, `{model} · {cwd}` dim and two-space inset
(`dummy_model_name · ~/repo`) — whenever no band is open (the palette/shortcuts
band displaces it, and the Ctrl+R search line / `!` shell-mode hint take its
slot; `App::set_session_info` injects the strings at the boundary
like the clock, the model name coming from `ReplySource::model_name`; see
`docs/footer.md`) stays pinned at the bottom. The alternate screen hosts the
full-screen overlays: the **Ctrl+O tool-output view**, a full-screen overlay listing every tool
call's complete output while the conversation keeps streaming underneath (see
invariant 4), the **`/resume` picker**, and the **Ctrl+D context-debug view**
(the raw LLM context window — the derived conversation the real backend sends
each turn, tool calls in the provider-native `tool_calls`/`tool` wire format
(an assistant `→ name(args)` request + a `tool:` result entry) and `[Image #N]`
placeholders unrendered; `ui::render_context_view`/`ui::context_lines` over
the pure `context::context_messages`, see `docs/context.md`). ratatui's `Viewport::Inline` can't change height after startup, so
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
   safe — the **incremental** `ui::StreamRender` flushes every completed line to
   scrollback via `term::InlineViewport::insert_before` as the reply grows,
   *caching* the rendered rows of already-complete source lines and only rendering
   the newly-arrived tail (so streaming a reply is **O(reply)**, not O(reply²) — it
   replaced a per-chunk *whole-reply* re-render that starved the status animation on
   long code; see `docs/markdown.md`). `StreamRender::commit` withholds the
   still-growing last line (and, inside a fenced code block, the **whole**
   in-progress line — a code line's colour isn't final until its closing `(`/`//`
   streams in, and it may span several wrapped rows), plus any **trailing blank
   rows** (a model's `…\n\n` before a tool call — trimmed so they don't stack on
   the boundary's single spacer into three blank rows; the batch `assistant_lines`
   trims them too, both gated on `!in_code` so a blank inside an open fence
   survives), `StreamRender::preview` renders
   just that last line for the strip (cheap enough to redraw every animation frame),
   and `StreamRender::finish` flushes the remainder on `StreamDone`. The tests
   `ui::tests::incremental_commits_reconstruct_the_whole_reply`,
   `stream_render_is_prefix_stable_over_every_prefix`, and
   `streamed_code_never_recolours_a_committed_row` lock this property (committed rows
   + final flush == the fully-rendered message, and no committed row ever changes
   text *or* colour). Don't change the wrap algorithm — or the markdown/highlight
   line renderers (`AssistantRenderer`, driven by `markdown::BlockScanner` +
   `highlight::Highlighter`) — without re-checking those invariants.

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
   beneath it (guarded by `smoke.sh` Phase 5; and because a strip collapse can now
   coincide with a flush *mid-stream* too — a forming table's whole block commits
   at its close while the multi-row preview drops to one row — `term::paint_live`
   syncs the tracked height to the frame's height before `flush_pending` whenever
   lines are pending, `docs/table-streaming.md`, guarded by Phase 41). (`insert_before` itself only
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
   tail (`ui::repaint_lines`) and paints the live region below it — all in
   the same synchronized frame (stale queued lines are dropped: the tail regenerates
   them). How the screen is prepped first is a `term::ReflowClear` mode: **`InPlace`**
   (the Ctrl+O / `/resume` overlay return) **overwrites the screen top-down** then
   clears the rows below the tail, and `clear_region(All)`s *only* for an empty
   tail — because a leading full clear before the tail-write's scroll makes tmux
   spill the on-screen frame into scrollback, and after a Ctrl+O return (invariant 4)
   that pushes the **stale streaming strip** (`Working… (… tokens)`) into scrollback
   above the rebuilt conversation (guarded by `smoke.sh` Phase 7). **`Purge`** (`/clear`
   *and* every resize) instead **purges scrollback + clears the whole screen** first
   (codex's `clear_scrollback_and_visible_screen_ansi` — `ESC[2J` then the `ESC[3J`
   scrollback purge, emitted as one ANSI write) and rebuilds the **full** history
   (`RESIZE_REFLOW_MAX_ROWS`-capped) into the blank screen, `write_above` scrolling
   the overflow back into the now-empty scrollback: so `/clear` truly wipes
   scrollback (scrolling up shows nothing, not the old chat), and a resize can't
   leave the emulator's own reflowed copy of the old rows behind — the TUI-text
   duplication that in-place overwrite showed on a width change (guarded by
   `smoke.sh` Phases 16 and 17). A mid-stream repaint must not lose the
   in-flight partial reply (it lives in the streaming buffer, not `history`):
   the tail carries the rows the stream already committed
   (`ui::repaint_tail` / `StreamRender::committed_rows`) and the rows that
   arrived since — chunks drained under the overlay, or the whole partial after
   a purge reset the render — are queued right after via the normal
   `insert_before` pipeline, reaching scrollback exactly once (guarded by
   `smoke.sh` Phase 35; a resize that lands *while* an overlay is up upgrades
   that return's repaint from `InPlace` to `Purge` — `overlay_resized`).

4. **Tool calls interleave with text, and Ctrl+O opens a separate overlay.** A
   tool call splits the assistant text around it: `App::flush_streaming_segment`
   finalises the run of text before a `ToolStart` as its own history message so the
   tool slots *after* it in order (the scrollback and the resize/return repaint
   must agree). Inline a tool is collapsed (`ui::tool_lines` — coloured bullet +
   one-line peek). When the model requests a **parallel batch** of calls in one
   round, they are announced up front (`StreamEvent::ToolBatch` →
   `App::start_tool_batch`, filling the `App::tool_queue` `VecDeque`) so the live
   region shows *every* call at once — the running one live (blue), the not-yet-run
   siblings as dim `⎿ Waiting…` cells (`ToolStatus::Waiting`), each committing to
   scrollback as its `ToolEnd` arrives. Execution stays **sequential** (only the
   front of the queue is ever `Running`, so the invariant is "at most one running
   tool", not "at most one live tool"); an interrupt/error resolves the running
   call and drops the un-started `Waiting` siblings (`docs/parallel-tools.md`). The
   Ctrl+O overlay (`ui::render_tool_view` on the alternate
   screen) is codex's **Ctrl+T transcript pager** — a slash-tiled dim
   `/ T R A N S C R I P T` title row, the scrolling transcript body with
   vi-style `~` filler past its end, a `─` separator carrying the scroll
   percentage right-aligned, and two dim key-hint rows (↑/↓, pgup/pgdn,
   home/end jump; q/esc/ctrl+o close — though Esc when idle with a previous
   user message instead *begins the backtrack preview* in place, and while one
   highlights a message the second hint row swaps to the backtrack keys;
   `docs/backtrack.md`) — showing the **full conversation
   transcript**: `ui::transcript_lines`
   walks `history` (messages + each tool's *expanded* output) plus the live tail
   (in-progress reply / the running tool **and any `⎿ Waiting…` batch siblings**,
   the whole `tool_queue` in order) plus the still-queued backlog
   (`ui::queued_lines`' inset rows, so Ctrl+O never hides a queued message —
   `docs/queue.md`). That walk is **O(history)** and re-runs the markdown +
   syntax highlighter over the whole transcript, so the loop drives it through a
   **`ui::TranscriptCache`** (a `main.rs`-owned cache, like `StreamRender`): a
   scroll changes only the viewport window, not the content, so the cache rebuilds
   only when a cheap signature (history length, live-tail length, the tool
   queue's shape — its length + front-call status **+ front output length**, so a
   Waiting→Running flip, a batch call committing, *or a running `bash` call
   streaming its output* invalidates it — the last is what makes the overlay show
   the **live streaming output** (unlike Claude Code, which only shows a tool's
   output once it finishes; the overlay tail-follows the frontier,
   `docs/tool-streaming.md`) — backtrack selection,
   width) changes — a scroll keypress is then
   a cache hit (O(viewport)), not a full re-highlight. `draw_tool_view` builds it
   **once** per draw (shared by the scroll clamp and the render); it's freed on
   overlay close. History is append-only while the overlay is up (a backtrack
   rewind truncates it but also exits), which is what makes the length signature
   exact. Only the **user** message shows its
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

The loop is an async (`tokio`, current-thread) `select!` over five sources —
input, reply events, draw ticks, `@` file-search results, and finished Ctrl+V
clipboard reads (each paste's read + decode + encode runs on its own worker
thread so the loop — and the status animations — never block on it);
`select!`'s randomized branch order gives input/draw fairness for free. Every state
change calls `frame.schedule_frame()`; the `frame` scheduler coalesces those into a
single draw tick, rate-limited to 120 fps (`MIN_FRAME_INTERVAL`). A paste/fast-type
run is caught by `paste::PasteBurst` so its characters request relaxed frames
(`schedule_frame_in` — the rate-limit floor does the coalescing; the scheduler
keeps the soonest pending deadline). *While a turn is active* the draw branch **re-arms** the next
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
keyboard / resize ──► EventStream ─┐
reply backend ─────► tokio mpsc ───┼─► select! ─► App::on_key / push_chunk / start_tool / set_file_matches / attach_image / set_status_times / … ─► schedule_frame
frame scheduler ───► draw-tick ────┤                                        coalesce + 120fps ─► draw / draw_overlay
file-search worker ► tokio mpsc ───┤
image-paste worker ► tokio mpsc ───┘
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
backend interleaves `StreamEvent::ToolStart{name,args}`/`ToolEnd{output,ok,truncated}` pairs
(with `ToolOutput(chunk)` **live-output** deltas streamed in between — the
running `bash` cell tails them via `App::push_tool_output`, `docs/tool-streaming.md`)
and a `ThinkingStart`/`ThinkingEnd` pair (with opaque `ThinkingChunk` reasoning
deltas streamed in between) between `Chunk`s; the loop shows the tool
running (blue) then commits it collapsed (green/red), and flips its `thinking_start`
`Instant` so the status line shows/drops `Thinking for Ns`. Before a tool runs, the backend also streams the model **generating** the call as
`ToolCallDelta(fragment)` events (the `name`/`arguments` pieces of a `tool_calls`
delta — `openai::Delta::tool_call`, surfaced ahead of the `ToolStart`); the loop
counts them via `App::push_tool_call_progress` (never rendered) so the tally ticks
while the call is produced, exactly like reasoning. The just-sent
**user message is counted up front** (`App::count_user_input` after
`begin_stream`, arrow `↑` — uploaded input), so the status shows `↑ N tokens`
through the backend's **pre-stream pause** (`DummyAi` waits `STARTUP_DELAY`/3s
before its first chunk so the indicator is visibly working first — overridable
via `INLINE_TUI_STARTUP_DELAY_MS`; the strip reserves **no preview row** while
there's nothing to preview — `ui::preview_rows` 0 — so the pause is status +
gap only, no stray empty line, like codex). Then `Chunk`s,
`ThinkingChunk`s (counted via `App::push_thinking` — never rendered), the
tool-call generation deltas, and a tool's
output grow the cumulative token tally on `App::status` (`↓` while replying,
thinking, or generating a tool call, `↑` for the input and right after a tool — never reset); on `StreamDone` `App::end_turn`
records the `Done for Ns` summary. A backend may send `StreamEvent::Error(msg)` instead of
`StreamDone` — even mid-tool; the loop turns that into a red `Role::Error` notice via
`App::fail_stream` (which also resolves a still-running tool as failed —
`Interrupted by a backend error` — and clears the status). **Esc while the turn is in
flight returns `Action::Interrupt`** (palette-dismiss still wins; when idle Esc
arms the Esc-Esc backtrack instead, is a no-op over a typed draft, and quits
only with an empty composer and no user message to edit —
`docs/backtrack.md`): the loop cancels + detaches the backend, **swaps the channel** (a stale
`ToolStart` would wedge a phantom running tool), and `App::interrupt_turn` returns
either `Kept` — keep the partial, resolve a running tool as failed
(`Interrupted by user`), record the red `INTERRUPT_NOTICE` (**`None` for a shell
turn** — its cell is the record), clear the status with no summary — **or**
`Undone` when nothing had streamed and nothing is queued: the submission rolls
back, its message returned to the composer and dropped from history (the loop
purge-repaints, no notice; `docs/interrupt.md`). On any turn-end — `StreamDone`, `Error`, *or* the Esc
interrupt (its `Kept` branch) — the loop pops the front queued batch (`App::drain_next_batch`) and
`start_turn`s it as the next turn (the remaining batches iterate at the
following turn-ends), so **Esc sends the front batch right away**; the drain
runs **under the Ctrl+O overlay too** (codex's queue dispatches at turn end
regardless of its Ctrl+T view, the transcript following the new turn live) —
dispatching only records history and *queues* the user bubbles, which the
return's reflow drops + regenerates, so invariant 4 holds.
`App` (`app.rs`) is pure state +
`on_key` (dispatched per `View`); `Action`, `Role`, `Message`, `StreamError`,
`InterruptedTurn`, `ToolStatus`, `ToolCall`, `TokenArrow`, `TurnStatus`,
`TurnSummary`, `HistoryItem`, `QueuedTurn`, `Toast`, `ToastKind`, `FileSearch`, `ResumePicker`, `View` live there too
(the `@`-picker primitives `AtToken`/`FileMatch`/`at_token`/`fuzzy_match`/`rank_files`
live in the pure `file_search` module, and the `/resume` primitives
`SessionMeta`/`SessionSummary`/`meta_line`/`item_line`/`parse_session`/`preview_of`/
`relative_age`/`rollout_rel_path` in the pure `session` module — `docs/resume.md`).

Typing a bare `/token` opens a **slash-command palette** below the input box (a
third live-region band): `App::command_menu` holds the highlight, the registry
`app::COMMANDS` (`SlashCommand { name, description, effect }` — currently `/help`,
`/clear`, `/copy`, `/resume`, `/model`, `/login`, and `/quit`) is filtered by `matching_commands`, and ↑/↓ scroll / Tab+Enter run
the highlighted command. Descriptions line up in a column, and the selection is
shown **by colour** — the whole highlighted row lights up cyan (name *and*
description the same colour) while the others are dimmed grey, no caret. A command
dispatches an `Action`
(`/clear`→`Clear`, `/help`→`Notice(String)` committed as a `Role::System`
message — **but mid-turn `/help` is rejected with a `Toast`** (its list would
interleave with the reply; `docs/toast.md`),
`/quit`→`Quit`, **`/copy`→`Copy(Option<String>)`** — codex's `/copy`:
the pure core picks the last assistant message (`App::last_assistant_text`) and
the loop writes it to the system clipboard, arboard with an OSC 52 fallback for
headless/SSH/tmux, then shows a transient `Copied last message to clipboard` toast
(or a red `No agent response to copy`/`Copy failed` toast) — **a self-clearing
line above the box, not a scrollback bullet** (`docs/toast.md`); the clipboard write is
the I/O boundary, the `base64`/OSC 52 framing a tested pure core in `clipboard`;
see `docs/copy.md`, **and `/resume`→`OpenResumePicker`** — codex's `/resume`:
every conversation records to a rollout JSONL file (the recorder + dir scan at
the boundary, the format/parse in the pure `session` module) and the command
opens a full-screen alt-screen picker (`View::ResumePicker`,
`ui::render_resume_picker` — dense `❯ {age:12}{preview}` rows with the
selection lit on a full-width background tint, type-to-search, and codex's
Filter/Sort toolbar on the search row (`Filter: [Cwd] All   Sort: [Updated]
Created` — Tab moves the focus, ←/→ toggle, `Cwd`/`Updated` default),
Enter → `ResumeSession(path)` loading the file's history via
`App::load_session` and appending later turns to the same file; Esc clears the
query first then closes, Ctrl+C closes, mid-turn `/resume` is rejected with a
transient `Toast` (was a red `ErrorNotice`; `docs/toast.md`) like codex; see
`docs/resume.md`, `smoke.sh` Phase 31. **`/model` and `/login` (the inline
pickers, `docs/llm.md`) now open *mid-turn* too** — they only replace the
composer, never the running turn, so their old busy rejections are gone; their
confirmations are toasts (`smoke.sh` Phase 33)).
**`/clear` mid-turn is a kill**, not codex's
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
  border, the tool-call styling (`TOOL_*` — dim-waiting/blue/green/red status
  colours (`TOOL_WAITING_COLOR` for a batch's not-yet-run `⎿ Waiting…` calls,
  `docs/parallel-tools.md`), the
  `⎿` peek prefix, the `(ctrl+o to expand)` hint), tool-view chrome
  (`TOOL_VIEW_*`), the transcript timestamp (`TIMESTAMP_COLOR` — the dim
  `hh:mm AM/PM` stamp right-aligned on its own line under the *user* message,
  the only stamp shown, only in the Ctrl+O view), the status
  indicator (`STATUS_*` — the comet spinner's white head + mid-grey
  `SPINNER_TAIL_COLOR` fading tail + dim walls and the
  `SPINNER_FRAMES`/`SPINNER_INTERVAL` animation, dim metrics, the `↓`/`↑` arrows
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
  `~`-relative path), the transient toast row above the box (`TOAST_*` — the
  two-space `TOAST_INDENT`, the dim `TOAST_COLOR` (info) / red `TOAST_ERROR_COLOR`
  (failure); `toast_rows`/`toast_line`, ellipsis-truncated like the footer — see
  `docs/toast.md`), the Ctrl+R search line that takes the footer's slot while
  a search is open (`SEARCH_*` — the dim `SEARCH_PROMPT`, the cyan
  `SEARCH_QUERY_COLOR` shared by the bold accept/cancel hint keys, the red
  `SEARCH_NO_MATCH` notice, and `SEARCH_HIGHLIGHT` — the reversed+bold styling
  of the query occurrences in the previewed match; `search_line`, the
  query-end cursor in `cursor_position`, `highlight_row_spans`), the Esc-Esc
  backtrack (`BACKTRACK_*`/`SHORTCUTS_BACKTRACK`/`TOOL_VIEW_HINT_BACKTRACK` —
  the primed `esc again to edit previous message` hint that takes the same
  footer slot (`backtrack_hint_line`), the preview's reversed user-message
  highlight + scroll target from the single `transcript_build` walk
  (`transcript_selection`/`backtrack_scroll`), and the overlay's swapped
  key-hint row; see `docs/backtrack.md`), the `!`
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
  the live-region row geometry (`GAP_ROWS`/`STATUS_ROWS`/`STATUS_GAP_ROWS`/`INPUT_CHROME_ROWS`/`LIVE_MIN_HEIGHT`;
  the status + gap strip shows *while a turn is active*, and the preview + gap
  is added *only when there's content to preview* (`strip_rows(streaming,
  preview_rows)`/`preview_rows` — the **count** of preview content rows: 0 idle,
  1 for a streaming reply or `!` shell run, N for a running backend tool's whole
  cell (or the whole parallel `tool_queue`: every batched call's cell, running +
  `⎿ Waiting…`, blank-separated — `docs/parallel-tools.md`); the pre-stream pause
  reserves **no** empty preview row, like codex) —
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
  send `StreamEvent::StreamDone` — or `StreamEvent::Error(msg)` on failure. For tool calls, optionally
  stream `StreamEvent::ToolCallDelta(fragment)`s as the model *generates* the call
  (its `name`/`arguments` pieces — counted like reasoning so the tally ticks while
  the call is produced, the text never shown), then send a
  `StreamEvent::ToolStart{name,args}`, optionally stream `ToolOutput(chunk)`
  live-output deltas while the tool runs (the running cell tails them —
  `docs/tool-streaming.md`; the real `bash` executor streams completed lines),
  then a `ToolEnd{output,ok,truncated}`
  (`truncated: false` from a backend tool — only the `!` shell runner caps); wrap a reasoning
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
  rendering treat chunks and tool output as opaque text, and count the status
  tokens app-side with a real `tiktoken` `o200k_base` tokenizer (no usage
  reporting in the protocol) via the `app::count_tokens` → `tokenizer::count`
  seam — exact for OpenAI models, close for the rest; nothing else changes.
  **The real `LlmBackend` also drives an agentic tool loop** (`docs/tools.md`):
  it offers the model `bash`/`read`/`write`/`edit` as Chat Completions function
  tools, and `llm::agent::run_agent` streams a round, runs the tools the model
  requested (emitting the same `ToolStart`/`ToolEnd` events the dummy scripts,
  via the `llm::exec::ToolExecutor` seam), feeds the results back, and loops
  until the model answers with plain text. The pure pieces — the tool defs +
  edit engine (`llm::tools`), the streamed `tool_calls` accumulator
  (`llm::openai::ToolCallAccumulator`), and the loop itself — are unit-tested;
  the executor's file/process I/O is boundary code. Tools are on by default,
  off via `INLINE_TUI_TOOLS`. A `read`/`edit`/`write` cell renders its output as
  a **numbered file change** (codex's `diff_render` look in the `⎿` gutter —
  `ui.rs`'s `file_cell_lines`): the executor emits `Created {path} ({N} lines)`
  over the numbered contents, `Updated {path} (+A -D)` over numbered diff
  **hunks** (3 context lines, `⋮` between distant hunks — the pure
  `tools::render_numbered_content`/`render_numbered_diff`), or — for `read` —
  the file numbered by `tools::format_read` in the **same** `{n:>W} {text}`
  gutter (dynamic-width numbers + a space, not the old `cat -n` tab; the UI
  synthesizes the `Read {N} lines` corner). The cell re-styles those rows — dim
  line numbers, green/red signs (`read`/`created` have none), the content
  syntax-highlighted by the path's extension, added/removed rows on
  dark-green/red background tints (`TOOL_DIFF_*_BG`), a 10-row inline peek
  (`FILE_PEEK_LINES`) with the `… +N lines` hint, everything in Ctrl+O;
  unparseable output (old rollouts, error bodies, a `read` placeholder) keeps
  the legacy rendering.
