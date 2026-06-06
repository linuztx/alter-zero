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
`unicode-width` for display-width math. `rust-toolchain.toml` pins the toolchain;
a `[lints]` table in `Cargo.toml` bakes the gate into every build
(`unsafe_code = "forbid"`, plus `warnings` and `clippy::all` denied).

## Architecture

A **library** (`src/lib.rs` → `app`, `stream`, `ui`, `term`) holds the logic;
**`src/main.rs`** is a thin terminal shell. The pure, unit-tested logic lives in
`app`/`stream`/`ui` so behavior is testable with a plain `Buffer`/`TestBackend`
and no real terminal. `main.rs` **and `term.rs`** are the I/O boundary — the only
files not unit-tested — verified via `scripts/smoke.sh`. Keep logic out of them;
every geometry decision `term.rs` makes is a pure `ui` helper it calls.

The design rationale lives in `docs/design.md`.

### The runtime model and its invariants

This is an **inline** TUI (no alternate screen): finished messages flow into the
terminal's real scrollback; a live region (a preview row + a rule-framed input
box) stays pinned at the bottom. ratatui's `Viewport::Inline` can't change height
after startup, so `term::InlineViewport` is a *custom* inline viewport over a
`CrosstermBackend` whose height is **dynamic** — the input box grows with the
wrapped input (`ui::live_height`). Three non-obvious invariants hold the whole
thing together — breaking any one reintroduces a class of bug:

1. **Only the main thread reads stdin.** The event loop reads keys with
   `event::poll`; the reply backend (a `stream::ReplySource`, e.g. `DummyAi`)
   streams on a background thread that *only sends* `StreamEvent`s on an mpsc
   channel. Viewport init and `insert_before` query the cursor position over
   stdin, so a second stdin reader would steal that reply and cause "cursor
   position could not be read". Never add a thread that reads stdin.

2. **Greedy word-wrap is prefix-stable** (`ui::wrap_text`): appending text only
   ever changes the *last* wrapped line. This is what makes streaming-to-scrollback
   safe — `ui::stable_commit` flushes every line *except the last* to scrollback
   via `term::InlineViewport::insert_before` as the reply grows, tracking a
   `committed` count; `ui::final_commit` flushes the remainder on `StreamDone`. The
   test `ui::tests::incremental_commits_reconstruct_the_whole_reply` locks this
   property (committed lines + final flush == the fully-rendered message). Don't
   change the wrap algorithm without re-checking that invariant.

3. **The viewport is always bottom-pinned, and resize reflows both directions**
   (`main.rs::reflow_after_resize`). `term::draw` re-pins the live region to the
   bottom every frame, scrolling the screen *up* (oldest chat into scrollback) to
   grow the box or *down* to shrink it (`ui::repin_scroll`). On a width change
   every wrapped line is stale, so `App` retains a `history: Vec<Message>` of
   finished messages (the *only* reason history is kept) and `term::reflow` clears
   the screen, seats the viewport at the top, then `insert_before`s the re-wrapped
   tail (`ui::repaint_lines`) so it fills the screen *without scrolling* — the
   tmux-safe path. `committed` is reset so a mid-stream resize re-commits the reply.

### Data flow

```
keyboard / resize ─► event::poll ─► App::on_key ─► Action::{Submit,Quit,None}
reply backend ─────► mpsc<StreamEvent> ─► try_recv ─► push_chunk / finish_stream / fail_stream
```

`Submit(text)` records the user message, `insert_before`s it, then spawns a reply
via the selected `ReplySource` (`backend.spawn(text, tx, cancel)`), keeping the
thread handle + `CancelToken` so a quit mid-stream cancels and reaps it. A backend
may send `StreamEvent::Error(msg)` instead of `StreamDone`; the loop turns that
into a red `Role::Error` notice via `App::fail_stream`. `App` (`app.rs`) is pure
state + `on_key`; `Action`, `Role`, `Message`, `StreamError` types live there too.

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
  prompt, colours (including the red error bullet), border, and the live-region
  row geometry (`PREVIEW_ROWS`/`INPUT_CHROME_ROWS`/`LIVE_MIN_HEIGHT`; the box's
  dynamic height comes from `live_height`, and `render_live`/`cursor_position`
  share the `input_box` helper). Retheme or re-size there, not inline.
- **All width math goes through `cols()`** (display columns via `unicode-width`),
  never `chars().count()` — so CJK/emoji wrap and pad correctly.
- **Swapping in a real AI** means implementing `stream::ReplySource` (use `DummyAi`
  as a template) and changing the single `let backend = …;` line in
  `main.rs::run`. Stream `StreamEvent::Chunk(..)` per token, poll the `CancelToken`
  so a quit can stop you, then send `StreamEvent::StreamDone` — or
  `StreamEvent::Error(msg)` on failure. The loop and rendering treat chunks as
  opaque text; nothing else changes.
