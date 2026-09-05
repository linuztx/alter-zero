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
  scripted stand-in for the model that isn't plugged in yet. Watch this, though:
  the file below has known who made me all along and never once mentioned it.

● Read(about.py)
  ⎿  Read 16 lines
       1 #!/usr/bin/env python3
       2 """Print the alter-zero calling card."""
       3
       4 NAME = "alter-zero"
       5 TAGLINE = "an autonomous AI agent that lives in your terminal"
       6 CREATOR = "linuztx"
       7 HOME = "https://github.com/linuztx"
       8
       9
      10 def card() -> str:
     … +6 lines (ctrl+o to expand)

● Edit(about.py)
  ⎿  Updated about.py (+1 -1)
       9
      10  def card() -> str:
      11      """Return the calling card."""
      12 -    return f"{NAME} — {TAGLINE}"
      12 +    return f"{NAME} — {TAGLINE}\n  created by {CREATOR} · {HOME}"
      13
      14
      15  if __name__ == "__main__":

● Bash(python3 about.py)
  ⎿  alter-zero — an autonomous AI agent that lives in your terminal
       created by linuztx · https://github.com/linuztx

● It does now — alter-zero is the work of linuztx, https://github.com/linuztx,
  and the command output is the proof the edit landed. Read, change, verify: a
  real agent works exactly like that, one small checked step at a time.

  Two commands away from the real thing: /login signs you in — a
  subscription, or a provider API key — then /model picks the model to run.

────────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────────
```

That turn is one errand in three steps — read alter-zero's calling card, notice
it never prints the credit it carries, wire it in and run it — and its cells are
the *real* cells: the bodies are numbered and syntax-highlighted exactly as the
live agent's are, the diff carries the same green/red tints, and a failed
command reports the exit code the executor would have. Ask it for a *diff*, a *table*, some *parallel* commands,
a couple of *agents*, or to *ask you some questions* (the Claude-Code-style
`AskUserQuestion` modal, `docs/ask.md`) to see the other scripted demos — then
run `/login` and `/model` to put a real model behind it
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
alter-zero --resume 18a9f2c33d41e5b6-1a2b
```

`alter-zero --continue` (`-c`) reopens the newest conversation recorded in the
current directory without asking, `alter-zero --resume {id}` (`-r`) a specific
one by that id (a unique prefix works too), and a bare `alter-zero --resume` boots
straight into the `/resume` session picker.

A quoted message is the **first turn** — `alter-zero "fix the failing test"`
boots straight into that conversation, and `alter-zero --resume {id} "and now
the docs"` (or `-c "…"`) sends it into the reopened one; the prompt is sent
verbatim, one quoted argument (`--` before it if it starts with a dash).
`alter-zero --help` lays it all out: the page opens on **Alter Zero** in the
app's cyan over a `Usage:` line, the `[PROMPT]` argument, the options and the
`mcp` subcommand, coloured on a terminal and plain through a pipe or under
`NO_COLOR` (`docs/cli.md`). Once you've chatted, **Esc Esc** steps back
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
| `src/llm/` | The real OpenAI-compatible backend: `providers.toml` config, the streaming SSE client, the reasoning splitter, the `/v1/models` listing, the `.env` key store (`/login`), the GitHub Copilot device sign-in and the OpenAI ChatGPT browser sign-in (each with its own token exchange), the Responses, Messages and native-Ollama wire formats, the `config.json` model store (`/model`), and the `ReplySource` bridge. | ✅ (pure cores) |
| `src/hooks/` | Lifecycle hooks (`docs/hooks.md`): the `hooks.json` format, which handlers an event selects, the JSON payload each writes to a handler's stdin and the verdict its stdout is parsed back into. The spawn lives in `src/llm/hooks.rs`. | ✅ |
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
them in one place to retheme. Two looks are chosen in-app rather than in
code, and persist across sessions: `/mascot` picks the startup banner's
mascot (`docs/mascot.md`) and `/spinner` the status line's spinner animation
— nine styles, previewed live in the picker (`docs/spinner.md`).

## Using a real model

The backend is a `ReplySource` trait in `src/stream/source.rs`. By default the app runs
the built-in `DummyAi` (canned, offline), but a real **OpenAI-compatible** model
is built in — the `llm` module (`src/llm/`, see `docs/llm.md`). It streams
`/chat/completions` over SSE, splits `<think>`/native reasoning out for the
thinking stream, and reports its model id in the footer.

Point it at a provider with environment variables (or `providers.toml`), and the
real backend takes over automatically:

```bash
export OPENROUTER_API_KEY=sk-...              # or <PROVIDER>_API_KEY / ALTER_ZERO_API_KEY
export ALTER_ZERO_PROVIDER=openrouter          # a provider from providers.toml
export ALTER_ZERO_MODEL=anthropic/claude-3.5-haiku
cargo run
```

Or sign in from inside the app with **`/login`**, which asks how first —
**Use a subscription** or **Use an API key**:

- **GitHub Copilot** (`docs/copilot.md`). Enter runs GitHub's device flow and
  shows the one-time code in a box over the URL to enter it at — no browser is
  launched, `c` copies the code, and the page counts the code down while it
  waits for you to approve it. What is stored is the long-lived GitHub OAuth
  token; each request exchanges it for the short-lived bearer Copilot's API
  takes, and reads the model's context window, vision and *reasoning-effort
  ladder* off `api.githubcopilot.com/models` so Ctrl+T cycles exactly the
  levels that model accepts.
- **OpenAI (ChatGPT)** (`docs/chatgpt.md`) — sign in with a ChatGPT
  Plus/Pro/Team seat. Enter runs OpenAI's PKCE flow: the page shows a link to
  open (`c` copies it) and the browser redirects back to a loopback listener,
  so there is nothing to type. What is stored is the **refresh** token — and
  re-stored whenever OpenAI rotates it, since a reused one is terminal — while
  each request mints the ~1h access token the API takes and reads the account
  it routes on out of that token's own claims. The backend speaks OpenAI's
  **Responses** API rather than chat completions, translated in
  `src/llm/responses.rs` so nothing above the client notices; its
  `/models` listing feeds the same context-window gauge, vision degradation
  and Ctrl+T ladder every other provider gets.
- **An API key** is the old flow: pick a provider (**Agent Zero API**,
  **Anthropic**, **OpenRouter**, **Ollama Cloud**) and paste its key. Every row
  says whether it is reachable already — a green `✔` beside a dim
  `configured`, or a wholly dim `◯ unconfigured` — and the key page
  introduces the provider in a line
  and links the page its keys are created on, so "where do I get one?" is
  answered without leaving the flow.
- **Ollama** (`docs/ollama.md`) — local models, no key: pick **Ollama** in the
  same list and press Enter to accept the default host (or type one in
  Ollama's own `OLLAMA_HOST` grammar), and `/model` lists what `ollama pull`
  fetched, each row carrying its context window, vision and thinking off
  `/api/tags` + `/api/show`. The backend speaks Ollama's **native** API rather
  than its OpenAI-compatible `/v1`, because only the native one lets a request
  set the context window (`options.num_ctx`) — without it every model runs at
  the server's 4096-token default and the conversation is silently truncated.
  The window the footer gauges against is the window the server holds.

Either way the secret lands in `~/.alter-zero/.env` (git-ignored) so it
persists across runs. The dummy stays the default and the fallback — the real backend
activates only when a provider, model, and key all resolve and `ALTER_ZERO_DUMMY`
isn't set, so the app always runs offline out of the box. Switch models live with
the **`/model`** picker: an inline search-and-select list of the provider's
`/v1/models` (it asks you to `/login` first if no key is configured), and your
choice persists to `~/.alter-zero/config.json` so it's the default next run —
**in that directory**: every working directory remembers its own model, and one
you launch in for the first time starts with the last model you picked anywhere
(`docs/per-directory-state.md`).

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
`/settings` → **Permission mode** (or **Shift+Tab**) picks the posture;
`ALTER_ZERO_PERMISSIONS=0` turns the gate off entirely.

One kind of write never asks: the agent has a **scratchpad** of its own
(`docs/scratchpad.md`) — a session-private directory at
`/tmp/alter-zero-{uid}/{session}/scratchpad`, named in its system prompt as
where every intermediate result, throwaway script and working note goes
instead of `/tmp`. It sits outside your project, so a `write`/`edit` there
runs unasked, saying so on the cell:

```
● Write(…/scratchpad/plan.md)
  ⎿  Wrote 12 lines to …/scratchpad/plan.md
  ⎿  Allowed in the session scratchpad
```

A `bash` command still asks, wherever it points. The background shells' interim
output sits beside it under `…/{session}/tasks/{id}.output`.
`ALTER_ZERO_SCRATCHPAD=0` turns the whole thing off.

The model can hand work to **subagents** (`docs/agent-tool.md`) — side
conversations with their own context and tool loop, whose final message comes
back as the tool's result — and *which* subagents exist is yours to decide
(`docs/subagents.md`). Each type is a markdown file, Claude-Code style, in
`~/.alter-zero/agents/` (or a project's `.alter-zero/agents/`):

```markdown
---
name: reviewer
description: Reviews a diff for correctness bugs and reports what it found.
model: inherit          # or a model id — kimi-k3, claude-sonnet-5, …
tools: Bash, Read       # omit for every tool; mcp__deepwiki__* globs a server
---

You are a reviewer. Read the diff, report real defects only …
```

The body (optional) becomes that type's system prompt; `tools:` is an
allowlist, so leaving `Write` and `Edit` out makes the type genuinely
read-only. The built-in `general-purpose` and `explore` are written into
`~/.alter-zero/agents/` on first run — real files, yours to edit, with
comments explaining every key. The roots are re-walked at every turn, so a
type you (or the agent) just added is launchable immediately, and each type's
name, description and tools ride the model's context so it can pick one.
`ALTER_ZERO_AGENTS_DIR` points the whole thing somewhere else.

You can also wedge **your own commands** into the tool loop
(`docs/hooks.md`) — Claude Code's `hooks.json` contract, so a script written
for either tool works here unchanged. Put one at `~/.alter-zero/hooks.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "bash",
        "hooks": [{ "type": "command", "command": "./guard.sh" }] }
    ]
  }
}
```

Each handler is fed its event as JSON on **stdin** and answers on **stdout**
(or just exits `2` with a reason on stderr to refuse):

```
● Bash(rm -rf build/)
  ⎿  Blocked by hook: no destructive deletes outside ./tmp
```

A hook can refuse a call, rewrite its arguments, approve it without asking
you, or hand the model extra context after it ran. `PreToolUse`,
`PostToolUse`, `PermissionRequest`, `SubagentStart` and `SubagentStop` fire
today; the session-level events are modelled but not yet wired.
`/settings` → **Hooks** toggles them, `ALTER_ZERO_HOOKS=0` turns them off.

A reasoning model's **thinking is shown** (`docs/thinking-stream.md`). While a
phase runs it wears the tool cell's shape — a breathing `●` bullet over the
chain-of-thought in the `⎿` gutter, tail-following the newest rows:

```
● Thinking…
  ⎿  I need to look at the file first. The user asked for a modern
      landing page, so the structure should be: a hero, three feature…
```

The bullet breathes and `Thinking…` carries the same white shimmer the
`Working…` verb below it wears — the live block is the only part of this that
moves, because nothing there is ever committed.

When the phase ends the whole block collapses into one bullet-less line —
nothing is happening any more, so what is left is a fact about the turn:

```
Thought for 1m 5s · 1.5k tokens (ctrl+o to expand)
```

Dim, like the `Done for Ns` summary that closes the turn — the weight belongs
to the live block, where something is still happening. **Ctrl+O** expands the
thought under the same line, minus the hint.

The thought is not lost — **Ctrl+O** expands it in the transcript, and it
survives a `/resume`. The token count is the provider's own
`completion_tokens_details.reasoning_tokens` once the round's usage frame
lands, a tokenizer estimate until then. `/settings` → **Hide thinking** (or
`ALTER_ZERO_SHOW_THINKING=0`) hides it all — that hides *showing* the
thinking; **Ctrl+T** to `off` is what stops the model doing it.

Everything the session can be tuned with lives behind **`/settings`**
(`docs/settings.md`) — the knobs that used to be `ALTER_ZERO_*` environment
variables you had to know about before launch, now visible and changeable
mid-session in the same inline frame `/model` uses:

```
  ❯

→ Hide thinking     false
  Error retry       3
  Tools             true
  Permission mode   manual
  Checkpoints       true
  Auto compact      true
  Project docs      true
  Temperature       default
  Max tool calls    0
  (1/9)

  Hide the model's chain-of-thought instead of streaming it above the composer

  Type to search · Enter/Space to change · Esc to cancel
```

Type to search (the label *and* the description — `agents.md` finds **Project
docs**), **Enter** or **Space** to cycle the highlighted value, **Esc** to
close. There is no text field anywhere: every value cycles, so one key means
one thing on every row. **Permission mode** is the same posture **Shift+Tab**
cycles — one state, two doors. A knob this machine can't serve (checkpoints
with no `git`, say) shows `false (unavailable)` and says so rather than
offering a toggle that does nothing. **Max tool calls** defaults to `0` — no
limit: a cap that trips mid-task abandons the work half-done, and **Esc** is
already the stop button. Changes take effect at once — the ones
that reshape a request rebind the *next* turn, so `/settings` is safe to open
mid-turn — and persist to `~/.alter-zero/settings.json` as a diff from the
defaults, **per working directory** (a knob set in one project is that
project's; a new directory starts from the defaults, `docs/per-directory-state.md`);
an `ALTER_ZERO_*` override still wins for the run it was set in, but never gets
saved on top of your choice. **Hooks** and **Checkpoints** are off until a
directory turns them on.

Providers live in `providers.toml` (repo root; an Agent-Zero/Venice proxy,
OpenRouter, Anthropic, the two subscriptions and Ollama ship by default). To plug in a *non*-OpenAI-shaped
backend instead, implement `ReplySource` (with `DummyAi`/`LlmBackend` as
templates) — `spawn(prompt, images, tx, cancel)` streams `StreamEvent::Chunk(..)`
per token, polls the `CancelToken`, then sends `StreamDone` (or `Error(msg)`);
tool calls are a `ToolStart`/`ToolEnd` pair, a thinking phase is
`ThinkingStart`/`ThinkingEnd` with `ThinkingChunk`s between (whose text drives
the thinking stream).

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
