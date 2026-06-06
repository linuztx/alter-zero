# inline-tui

A tiny, well-documented **inline terminal chat UI** built with
[ratatui](https://ratatui.rs). You type a message, press Enter, and a *dummy*
AI streams a canned reply word by word. The layout reflows responsively to the
terminal width and the style echoes Claude Code: finished messages flow up into
your normal terminal scrollback, with a rule-framed input field pinned at the bottom.

It's deliberately minimal so the whole thing is easy to read, and almost all of
the logic is pure and unit-tested.

```
❯ what is ratatui?
● Great question. There's no AI behind this yet — these words are streamed from
  a canned response to show off the inline TUI. Finished messages scroll up into
  your normal terminal history, just like Claude Code.

────────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────────
```

## Run it

```bash
cargo run
```

Type a message and press **Enter**. Press **Esc** or **Ctrl+C** to quit.

## How it works

ratatui's **inline viewport** (`Viewport::Inline`) renders a fixed-height region
at the bottom of the terminal *without* taking over the screen (no alternate
screen), so your scrollback is preserved. Two ideas make the chat feel live:

1. **Finished text goes into real scrollback** via `Terminal::insert_before`,
   which inserts lines *above* the viewport — so completed messages become part
   of your terminal history.
2. **Streaming commits line by line.** As the reply grows we re-wrap it to the
   current width and flush every line that can no longer change. Greedy
   word-wrap is *prefix-stable* — only the last wrapped line can still change as
   more words arrive — so committing "everything but the last line" is always
   safe. The in-progress last line is shown live in the preview row above the
   input box.

**Input is read on the main thread** (`event::poll`), and only the streaming
reply runs on a background thread — and that thread merely *sends* on a channel,
it never reads stdin. This matters: terminal init and `insert_before` query the
cursor position over stdin, so a *second* thread reading stdin would steal the
reply to that query (the classic "cursor position could not be read" error).
Keeping all stdin reads on one thread avoids the race entirely.

3. **Resize reflows the conversation — both ways.** When the terminal width
   changes, the on-screen chat must be re-wrapped. ratatui clears the screen on a
   width *shrink* but not on a *grow*, and either way the visible lines keep their
   old wrapping until they're redrawn. So `App` keeps a `history` of finished
   messages, and on any width change we clear and repaint the tail that fits above
   the input box — re-wrapped to the new width, wider *or* narrower. Older lines
   remain in the terminal's own scrollback with their original wrapping, exactly
   like any past terminal output.

### Architecture

Built as a small **library** plus a thin **binary**, so the logic is testable
without a real terminal.

| File         | Responsibility                                                                 | Tested |
|--------------|--------------------------------------------------------------------------------|--------|
| `src/app.rs` | Conversation state + pure update logic (`on_key`, `push_chunk`, `finish_stream`, `fail_stream`, message `history`). | ✅ |
| `src/ui.rs`  | Pure rendering: display-width word-wrap, styled message lines, the live region, commit bookkeeping. | ✅ |
| `src/stream.rs` | The backend seam: the `ReplySource` trait + built-in `DummyAi`, a `CancelToken`, and the `StreamEvent` protocol. | ✅ (pure parts, token & dummy) |
| `src/main.rs` | Thin glue: terminal init/restore, the single-threaded poll loop, backend cancel/reap on quit. | ⚪ I/O boundary |

The loop reads keys directly and drains streamed events from a channel:

```
keyboard / resize ─► event::poll ─► App::on_key ─► Action::{Submit,Quit,None}
reply backend ─────► mpsc<StreamEvent> ─► try_recv ─► push_chunk / finish_stream / fail_stream
```

## Tests

```bash
cargo test       # run the full unit-test suite
cargo clippy --all-targets -- -D warnings
```

The interesting one is `ui::tests::incremental_commits_reconstruct_the_whole_reply`,
which streams a reply word-by-word and asserts the committed lines (plus the
final flush) exactly equal the fully-rendered message — proving the streaming
commit logic never drops, duplicates, or reorders a line.

### Manual smoke test

`main.rs` itself isn't unit-tested (it's the terminal I/O boundary), so there's
a script that drives the real binary inside `tmux`:

```bash
cargo build
bash scripts/smoke.sh
```

It types a message, lets the reply stream, and prints the captured pane.

## Theming

All styling lives as constants at the top of `src/ui.rs` (bullets, prompt,
colours, border) — change them in one place to retheme.

## Plugging in a real AI later

The backend is a `ReplySource` trait in `src/stream.rs`; the demo uses the
built-in `DummyAi`. To use a real model, implement `ReplySource` (with `DummyAi`
as a template) and change the single `let backend = DummyAi;` line in `main.rs`.
Your `spawn` runs on a background thread that sends `StreamEvent::Chunk(..)` per
token, polls the `CancelToken` so a quit can stop it, then sends
`StreamEvent::StreamDone` — or `StreamEvent::Error(msg)` on failure, which the app
shows as a red error notice. Nothing else changes; the loop and rendering treat
chunks as opaque text.

## Known limitations (v1)

- Single-line input (it scrolls horizontally to keep the cursor visible).
- **Resizing the width** (wider or narrower) repaints the on-screen conversation
  re-wrapped to the new width. One honest caveat: lines that had already scrolled
  into the terminal's own scrollback keep their original wrapping, so after
  resizing a *long* chat you may see the boundary messages once in the (old-width)
  scrollback above and again in the (new-width) repaint — exactly the behaviour of
  any program that writes to terminal scrollback.
- **Resizing mid-stream** recovers (the in-progress reply re-commits itself),
  but the moment of resize may briefly flicker the partial line.
- The inline viewport height is fixed (ratatui doesn't expose a runtime setter),
  so the live region is a fixed 4 rows.
- No spinner, timestamps, markdown rendering, or scrollback-navigation keys yet.
```
