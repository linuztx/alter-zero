# alter-zero

A tiny, well-documented **inline terminal chat UI** built with
[ratatui](https://ratatui.rs). You type a message, press Enter, and — until you
point it at a real model — a *dummy* backend streams a canned demo turn word by
word, tool cells and all. The layout reflows responsively to the terminal width
and the style echoes Claude Code: finished messages flow up into your normal
terminal scrollback, with a rule-framed input field pinned at the bottom.

It's deliberately minimal so the whole thing is easy to read, and almost all of
the logic is pure and unit-tested.

```
❯ hello there

● Happy to help — with one asterisk: I'm alter-zero's built-in demo backend, a
  scripted stand-in for the model that isn't plugged in yet.

● Read(src/main.rs)
  ⎿  Read 7 lines
      1 #[tokio::main(flavor = "current_thread")]
      2 async fn tui_main(startup: Option<Startup>) -> io::Result<()> {
      3     let mut term = InlineViewport::init(ui::LIVE_MIN_HEIGHT)?;
      4     let result = tui::event_loop::run(&mut term, startup).await;
      5     let restored = term.restore();
      6     result.map(|_| ()).and(restored)
      7 }

● Bash(ping -c 3 x.invalid)
  ⎿  Error: Exit code 68
     ping: cannot resolve x.invalid: Unknown host

● That is a whole turn: a thinking phase, a tool batch with live output, then
  finished cells committed into your terminal's own scrollback — canned words,
  real interface.

  Two commands away from the real thing: /login saves a provider API key, then
  /model picks the model to run.

────────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────────
```

The demo's cells are the *real* cells: the `Read` body is numbered and
syntax-highlighted exactly as the live agent's is, and a failed command carries
the exit code the executor would have reported. Ask it for a *diff*, a *table*,
some *parallel* commands or a couple of *agents* to see the other scripted
demos — then run `/login` and `/model` to put a real model behind it
(`docs/dummy-backend.md`).

## Run it

```bash
cargo run
```

Type a message and press **Enter**. Press **Ctrl+C** (or **Esc** on an empty
composer before you've sent anything) to quit — `?` in an empty composer lists the keyboard
shortcuts, `/` the slash commands.

Quitting a session that recorded a conversation prints how to get back into it
(`docs/cli.md`):

```
Resume this session with:
alter0 --resume 18a9f2c33d41e5b6-1a2b
```

`alter0 --continue` (`-c`) reopens the newest conversation recorded in the
current directory without asking, `alter0 --resume {id}` (`-r`) a specific one
by that id (a unique prefix works too), and a bare `alter0 --resume` boots
straight into the `/resume` session picker. Once you've chatted, **Esc Esc** steps back
to edit a previous message, codex-style (see `docs/backtrack.md`): the first
Esc arms it, the second previews the conversation with the last user message
highlighted, Esc/←/→ pick an older/newer one, and Enter rewinds the
conversation to that point with the message back in the composer.

## How it works

An **inline viewport** renders a live region at the bottom of the terminal
*without* taking over the screen (no alternate screen), so your scrollback is
preserved. ratatui's own `Viewport::Inline` fixes its height at startup, so
`term::InlineViewport` is a custom take on it whose height is **dynamic**: the
input box (a full multi-line editor — cursor keys, Home/End, newlines) grows as
your draft wraps, and the streaming strip, the `/`-palette / `?`-shortcuts /
`@`-file-picker bands, and the session footer come and go around it. Three ideas
make the chat feel live:

1. **Finished text goes into real scrollback** via `insert_before`,
   which inserts lines *above* the viewport — so completed messages become part
   of your terminal history.
2. **Streaming commits line by line.** As the reply grows we re-wrap it to the
   current width and flush every line that can no longer change. Greedy
   word-wrap is *prefix-stable* — only the last wrapped line can still change as
   more words arrive — so committing "everything but the last line" is always
   safe. The in-progress last line is shown live in the preview row above the
   input box.
3. **Resize reflows the conversation — both ways.** When the terminal size
   changes, the visible lines keep their old wrapping until they're redrawn. So
   `App` keeps a `history` of finished messages, and on any size change we
   repaint the tail that fits above the input box — re-wrapped to the new
   width, wider *or* narrower. Older lines remain in the terminal's own
   scrollback with their original wrapping, exactly like any past terminal
   output.

**The event loop is async** (tokio, current-thread): a `select!` over a
crossterm `EventStream` (keyboard/resize), the streamed reply (a tokio
channel), coalesced draw ticks from the frame scheduler, and `@` file-search
results — the codex-style loop described in `docs/async-rewrite.md`. One
invariant matters most: the `EventStream` is the **sole stdin reader**, created
only after terminal init has queried the cursor position over stdin — the reply
backend merely *sends* on its channel from a background thread. A second stdin
reader would steal the reply to that cursor query (the classic "cursor position
could not be read" error).

### Architecture

Built as a small **library** plus a thin **binary**, so the logic is testable
without a real terminal.

| File         | Responsibility                                                                 | Tested |
|--------------|--------------------------------------------------------------------------------|--------|
| `src/app/`  | Conversation state + pure update logic, one module per area (`docs/module-layout.md`): `on_key` → `Action`, streaming, tool calls, the slash palette, input history + Ctrl+R search, the mid-turn queue, `!` shell mode, message `history`. | ✅ |
| `src/textarea.rs` | The editable multi-line input: a movable grapheme-aware cursor, wrapped ↑/↓, insert/delete anywhere. | ✅ |
| `src/ui/`    | Pure rendering, one module per area (`docs/module-layout.md`): display-width word-wrap, styled message/tool lines, the live-region geometry, the status line, the bands + footer, commit bookkeeping. All styling lives in `ui/theme.rs`. | ✅ |
| `src/stream/` | The backend seam, one module per area (`docs/module-layout.md`): the `StreamEvent` protocol, the `ReplySource` trait, a `CancelToken` — and the offline `DummyAi` in its own `dummy/` subtree, whose scenario registry decides which canned demo a prompt plays (`docs/dummy-backend.md`). | ✅ (pure parts, token & dummy) |
| `src/llm/` | The real OpenAI-compatible backend: `providers.toml` config, the streaming SSE client, the reasoning splitter, the `/v1/models` listing, the `.env` key store (`/login`), the `config.json` model store (`/model`), and the `ReplySource` bridge. | ✅ (pure cores) |
| `src/file_search.rs` | The pure core of the `@` file picker: token detection, fuzzy matching, ranking. | ✅ |
| `src/frame.rs` | The frame scheduler: coalesces redraw requests into ticks, rate-limited to 120 fps. | ✅ (pure parts) |
| `src/paste.rs` | Paste handling: burst detection + the `[Pasted Content N chars]` / `[Image #N]` placeholders. | ✅ |
| `src/clipboard.rs` | Clipboard I/O: the Ctrl+V image read and the `/copy` write (arboard + OSC 52 fallback). | ✅ (pure parts) |
| `src/term.rs` | The custom dynamic-height inline viewport: scrollback commits, synchronized draws, the Ctrl+O overlay. | ⚪ I/O boundary |
| `src/main.rs` | 77-line shell: the detached-exec hook, the CLI resolution, the viewport, the loop. | ⚪ I/O boundary |
| `src/tui/` | The terminal shell, 20 area modules over one `Session` struct: the async `select!` loop, the `Action` dispatch, turns, the stream/agent folds, drawing, the backend selection, the stores and workers (`docs/module-layout.md`). | ⚪ I/O boundary |

The loop `select!`s over its four sources and redraws on coalesced ticks:

```
keyboard / resize ──► EventStream ─┐
reply backend ─────► tokio mpsc ───┼─► select! ─► App update ─► schedule_frame
frame scheduler ───► draw-tick ────┤
file-search worker ► tokio mpsc ───┘      coalesce + 120 fps ─► draw
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

`src/main.rs` and `src/tui/` aren't unit-tested (they're the terminal I/O
boundary), so there's a script that drives the real binary inside `tmux`:

```bash
cargo build
bash scripts/smoke.sh
```

It drives the real binary through a series of phases — streaming, resizes, Esc
interrupts, the palette, the queue, `!` shell commands, pastes — and asserts on
captured panes.

## Theming

All styling lives as constants in `src/ui/theme.rs` — bullets, prompt,
colours, border, and the tool / status-line / palette / footer chrome — change
them in one place to retheme.

## Using a real model

The backend is a `ReplySource` trait in `src/stream/source.rs`. By default the app runs
the built-in `DummyAi` (canned, offline), but a real **OpenAI-compatible** model
is built in — the `llm` module (`src/llm/`, see `docs/llm.md`). It streams
`/chat/completions` over SSE, splits `<think>`/native reasoning into the
*Thinking* status, and reports its model id in the footer.

Point it at a provider with environment variables (or `providers.toml`), and the
real backend takes over automatically:

```bash
export OPENROUTER_API_KEY=sk-...              # or <PROVIDER>_API_KEY / ALTER_ZERO_API_KEY
export ALTER_ZERO_PROVIDER=openrouter          # a provider from providers.toml
export ALTER_ZERO_MODEL=anthropic/claude-3.5-haiku
cargo run
```

Or add the key in-app with **`/login`** — an inline flow to pick a provider and
paste its API key; it saves to `~/.alter-zero/.env` (git-ignored) so it persists
across runs. The dummy stays the default and the fallback — the real backend
activates only when a provider, model, and key all resolve and `ALTER_ZERO_DUMMY`
isn't set, so the app always runs offline out of the box. Switch models live with
the **`/model`** picker: an inline search-and-select list of the provider's
`/v1/models` (it asks you to `/login` first if no key is configured), and your
choice persists to `~/.alter-zero/config.json` so it's the default next run.

Anything the model would **change** asks first (`docs/permissions.md`): a
`write`, an `edit`, or a `bash` command stops the turn and puts an inline
approval prompt where the composer was — under the cell that raised it, which
stays on screen — showing the action, the file (or the command and the model's
own description of it), the whole numbered content or diff, and
`1. Yes` / `2. Yes, allow all edits during this session` (for a command,
`2. Yes, and don't ask again for: {prefix}`) / `3. No`. **Tab** amends — reject
with instructions typed into the same composer; **Esc** cancels the turn;
**ctrl+e** asks the model to explain a command instead of running it. Whatever
you had typed when the prompt appeared is stashed and handed straight back.
An amended rejection is kept, not just delivered: the instructions show on the
red cell and the model-facing denial rides the conversation, so Ctrl+D shows
what the model was told and every later turn — and a `/resume` — still carries
it.
`ALTER_ZERO_PERMISSIONS=0` turns the gate off.

Providers live in `providers.toml` (repo root; an Agent-Zero/Venice proxy and
OpenRouter ship by default). To plug in a *non*-OpenAI-shaped
backend instead, implement `ReplySource` (with `DummyAi`/`LlmBackend` as
templates) — `spawn(prompt, images, tx, cancel)` streams `StreamEvent::Chunk(..)`
per token, polls the `CancelToken`, then sends `StreamDone` (or `Error(msg)`);
tool calls are a `ToolStart`/`ToolEnd` pair, a thinking phase is
`ThinkingStart`/`ThinkingEnd` with `ThinkingChunk`s between.

## Known limitations (v1)

- **Resizing** repaints the on-screen conversation re-wrapped to the new size.
  One honest caveat: lines that had already scrolled into the terminal's own
  scrollback keep their original wrapping, so after resizing a *long* chat you
  may see the boundary messages once in the (old-width) scrollback above and
  again in the (new-width) repaint — exactly the behaviour of any program that
  writes to terminal scrollback.
- **Resizing mid-stream** recovers, but the partial reply's already-committed
  lines reappear only when the next chunk re-commits them (the repaint itself
  is atomic — the box never flashes).
- Tool output is collapsed inline to a one-line peek; the full transcript (with
  every tool expanded, and the user messages' timestamps) lives in the Ctrl+O
  view — by design.
- The status line's token counts are an app-side estimate (≈ chars/4), not real
  model usage — even with a real backend (the streaming protocol's `usage` block
  isn't surfaced).
- The real LLM backend is single-turn (the seam passes one turn's text, no prior
  history) and has no vision or model-side tool calls yet — see `docs/llm.md`.
- No markdown rendering or scrollback-navigation keys yet.

The full list lives in `docs/design.md` under *Known limitations*.
