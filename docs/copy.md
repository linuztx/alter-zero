# `/copy` — copy the last response to the clipboard

Date: 2026-06-20

## Goal

A `/copy` slash command that copies the **last assistant response** to the
system clipboard — a port of openai/codex's `/copy` ("copy last response as
markdown"). The outcome surfaces as a **transient toast** above the box (an info
`Copied last message to clipboard` or `No agent response to copy`; an error
`Copy failed: {e}`), which self-clears after a few seconds instead of committing
a scrollback bullet. See `docs/toast.md`.

> **Update (2026-11-24):** inside tmux on a remote host, `/copy` toasted
> "Copied" and the user's clipboard never changed. `/export`'s clipboard
> row and every page that copies something had the same bug. tmux's
> defaults drop the OSC 52 escape an application writes. The copy now goes
> into tmux's own buffer (`tmux load-buffer -w -`), and tmux sends it on to
> the terminal under those same defaults. It also reaches the terminal
> inside a multiplexer and over SSH even when the native clipboard took the
> text. *Reaching the user's clipboard from tmux, screen and SSH* below has
> the measurements and the routing. It supersedes the clipboard-write steps
> and the tmux/SSH divergences further down.

> **Update (2026-09-23):** with no response to copy, `/copy` now returns
> `Action::Toast(COPY_EMPTY_NOTICE)` from the pure core — the same path
> `/export`'s `Nothing to export` and `/compact`'s `Nothing to compact` take —
> so it shows as an info toast, not an error. Nothing failed; there was just
> nothing to copy yet. Codex's wording is kept, but not its error kind.
> `Action::Copy` now carries the text itself (`Copy(String)`), since the empty
> case never reaches the boundary. `Copy failed: {e}` is still an error.

> **Update (2026-07-08):** `/copy`'s confirmation moved from a committed
> `Role::System` / `Role::Error` message to a transient toast (`present_toast`
> in `main.rs`, `App::show_toast`). The clipboard I/O below is unchanged; only
> the surface differs. `commit_system_notice` / the `COPY_OK_NOTICE` /
> `COPY_EMPTY_NOTICE` strings still exist (the strings feed the toast now; the
> commit helper still serves the empty `!` shell's hint). The rest of this doc describes the
> original committed-notice design for context.

## What codex does (ported from `/tmp/codex/codex-rs/tui`)

- `slash_command.rs`: a `Copy` variant, description `"copy last response as
  markdown"`, **no inline args**, available in side conversations and **during a
  task**, hidden only on Android.
- `chatwidget/interaction.rs::copy_last_agent_markdown_with` copies
  `transcript.last_agent_markdown` — the most recent **completed** agent
  message. Empty/absent → error `No agent response to copy`; the copy closure
  `Ok` → info `Copied last message to clipboard`; `Err(e)` → `Copy failed: {e}`.
- `clipboard_copy.rs`: an **environment-aware fallback chain** — over SSH it uses
  tmux/OSC 52 (never the native clipboard, which would write to the *remote*
  box); locally it tries **arboard** → WSL PowerShell → OSC 52/tmux. It holds a
  `ClipboardLease` (the live `arboard::Clipboard` on Linux) for the app's
  lifetime so the X11/Wayland selection isn't dropped the instant it's set.
- Default keybinding **Ctrl+O** (remappable).

## The mapping onto this codebase

### What we copy (`App::last_assistant_text`)

> **Update (2026-08-26):** it copies **the conversation the screen is
> showing**. Inside an agent session view that is the *viewed agent's* own
> transcript, not `App::history` — the `viewed_agent()` branch every other
> view-aware reader takes. Copying the lead's last answer while the screen
> showed a subagent's was the reported bug (`docs/agent-view-streaming.md`).
> Everything below still describes the main view's case, which is unchanged.

The text of the **last `HistoryItem::Message` with `role == Role::Assistant`**
in `App::history`. Our turn model splits assistant prose around tool calls
(`flush_streaming_segment` records the run of text before each `ToolStart` as its
own message), so "last response" is the last recorded assistant *segment* —
mirroring codex's `last_agent_markdown`, which is likewise just the latest agent
message. The in-progress `streaming` buffer is **not** copied: like codex copies
the last *recorded* markdown, we only copy finished history. An empty/absent last
assistant message → `None` (the nothing-to-copy path). Pure and unit-tested.

### Command + dispatch (pure)

- `COMMANDS` gains `{ name: "copy", description: "Copy the last response to the
  clipboard", effect: CommandEffect::Copy }`.
- A new `Action::Copy(Option<String>)`: `Some(text)` = "write this to the
  clipboard", `None` = "nothing to copy". `run_selected_command`'s `Copy` arm
  returns `Action::Copy(self.last_assistant_text())` after consuming the input
  and closing the palette like every command. The *decision* (what text, or
  nothing) stays in the pure core; the loop only does the I/O + commits the
  notice — the same split as `Action::Notice`/`PasteImage`.
- **Runs mid-turn.** The palette's `Enter`/`Tab` arms (`menu_open`) are matched
  *before* the queue/stream branch in `on_key`, so a `/copy` typed during a turn
  dispatches immediately instead of queuing — codex's "available during a task"
  for free.

### Keybinding — deliberately omitted

Codex binds `/copy` to **Ctrl+O**, but Ctrl+O is already the **tool-output
view** here. So `/copy` ships as a **slash command only** (codex supports that
form too). No key is rebound.

### Clipboard write — the I/O boundary (`clipboard.rs`)

> **Update (2026-11-24):** these steps are now only the desktop's case. The
> write side lives in `src/clipboard/copy.rs`. Inside tmux or screen, and
> over SSH, the terminal half runs after a native copy too, and inside tmux
> it goes through `tmux load-buffer -w -` (*Reaching the user's clipboard
> from tmux, screen and SSH*). The lease is held by `Session::copy_text`,
> the one call every page that copies makes. It is replaced only by a new
> lease, never dropped by a copy only the terminal took.

`pub fn copy_to_clipboard(text: &str) -> Result<Option<ClipboardLease>, String>`:

1. **arboard first** (`arboard_copy`): `Clipboard::new()?.set_text(text)?`. On
   **Linux** it returns `Ok(Some(ClipboardLease))` holding the live `Clipboard`
   so the selection survives (X11/Wayland serve it from the owning process);
   elsewhere `Ok(None)`.
2. **OSC 52 fallback** (`osc52_copy`), only when arboard errs (headless / no
   display / an SSH server with no clipboard): base64-encode the text into the
   `\x1b]52;c;{b64}\x07` set-clipboard escape and write it to `/dev/tty` (falling
   back to stdout). Terminal-native — works over SSH and inside tmux (with
   `set-clipboard` ≠ `off`). Capped at `OSC52_MAX_BYTES` (100 000) like codex.
3. Both fail → `Err` naming both failures.

`main.rs` keeps the returned lease in a loop-lifetime
`clipboard_lease: Option<clipboard::ClipboardLease>` (replaced on each `/copy`),
exactly like codex's `chatwidget.clipboard_lease`.

The **pure** pieces — `base64_encode` and `osc52_sequence` — are unit-tested in
`clipboard.rs` (like `frame`'s rate-limit math is, while its task is
smoke-covered); the actual arboard/tty writes stay boundary-only and
smoke-covered, per this module's "no I/O unit tests" rule.

### Feedback — the loop's `Action::Copy` arm (`main.rs`)

- `None` → `commit_error_notice("No agent response to copy")` (red `Role::Error`).
- `Some(text)` → `clipboard::copy_to_clipboard(&text)`:
  - `Ok(lease)` → `clipboard_lease = lease;` then
    `commit_system_notice("Copied last message to clipboard")` (cyan
    `Role::System`, codex's info event).
  - `Err(e)` → `commit_error_notice(&format!("Copy failed: {e}"))`.

`commit_system_notice` is a new boundary helper mirroring `commit_error_notice`
(flush the current streaming segment first so the notice slots *after* it, then
`record_system_message` + `insert_before`); the existing `Action::Notice` arm is
refactored onto it. The two fixed strings live as `pub const COPY_OK_NOTICE` /
`COPY_EMPTY_NOTICE` in `app/commands.rs`, codex-verbatim.

## Known divergences from codex

- **No keybinding** — Ctrl+O is the tool-output view here, so `/copy` is a slash
  command only.
- **No WSL path.** There is no PowerShell route: under WSL a copy goes through
  WSLg's display when there is one, and through the terminal's escape
  otherwise. SSH, tmux and GNU screen are detected (2026-11-24, *Reaching the
  user's clipboard from tmux, screen and SSH*).
- **tmux goes through its own buffer.** This bullet once said tmux's default,
  `set-clipboard external`, forwards an application's OSC 52 escape. It does
  not: `external` drops it. That was the remote-tmux bug. A copy inside tmux
  is now `tmux load-buffer -w -`, with the escape, wrapped in tmux's
  passthrough and then bare, as its fallback.
- **No arboard stderr suppression.** Codex silences X11/NSLog noise with a libc
  `dup2`; this crate `forbid`s `unsafe`, so we skip it.
- **No rewind history.** We have no conversation rollback, so codex's "only the
  most recent 32 responses are available to /copy" eviction case doesn't exist.

## Reaching the user's clipboard from tmux, screen and SSH

Date: 2026-11-24

### The bug

The report: alter-zero inside tmux on a VPS, reached over SSH. `/copy`
toasted `Copied last message to clipboard` and nothing reached the clipboard
on the user's machine. `/export`'s clipboard row behaved the same.

The copy tried arboard, which fails on a server with no display, then wrote
an OSC 52 escape to `/dev/tty`. Inside tmux that is a tmux pane, and tmux's
defaults drop the escape:

- **`set-clipboard external`** is the default. Since tmux 2.6 it means
  "tmux sends outside but does not accept inside": tmux sets the terminal's
  clipboard for its own copies and ignores an escape an application writes.
  Only `on` accepts one.
- **`allow-passthrough off`** has been the default since tmux 3.3, when
  the option arrived. The escape wrapped in tmux's passthrough
  (`ESC P tmux; … ESC \`) is dropped too.

The write to `/dev/tty` itself succeeded, so the copy reported success. The
smoke phases never saw it, because each set `set-clipboard on` on its own
server. Phase 28's comment said the default forwards an application's escape
and only declines to store it.

### What tmux lets through

`tmux load-buffer -w -` makes the text on stdin a paste buffer. Its `-w`
(tmux 3.2 and later) has tmux send that buffer to its client's terminal as
tmux's own clipboard escape. `external` lets that through, since it is
exactly "tmux may set the terminal's clipboard, an application may not".

These were measured on tmux 3.4 through two nested servers: the outer one
stands in for the user's terminal (`set-clipboard on`, so whatever escape
reaches it lands in its buffer), and the inner one, at its defaults, is the
VPS's tmux:

| in the inner tmux's pane | the inner tmux's setting | reaches the outer terminal |
|---|---|---|
| a bare OSC 52 escape | `set-clipboard external` (default) | no |
| a bare OSC 52 escape | `set-clipboard on` | yes |
| the escape in tmux's passthrough | `allow-passthrough off` (default) | no |
| the escape in tmux's passthrough | `allow-passthrough on` | yes |
| `tmux load-buffer -w -` | `set-clipboard external` (default) | yes |
| `tmux load-buffer -w -` | `set-clipboard off` | yes |

A 300 KB text went through `load-buffer -w` whole into both buffers. So the
tmux route carries no size cap of its own: a long `/export` goes through.
The escape keeps its `OSC52_MAX_BYTES` (100 000), since a terminal that
cannot take a long escape drops it silently.

tmux sends its escape only to a client whose terminal it believes can take
one. Its default `terminal-features` names `xterm*` as clipboard-capable, and
a terminal type whose terminfo has `Ms` counts too. A terminal type outside
both needs `set -as terminal-features ',<TERM>:clipboard'` in the user's
`~/.tmux.conf`. The terminal must also accept OSC 52 itself; most do, and
some ask first or need it switched on.

### The routing (`src/clipboard/copy.rs`)

`copy_to_clipboard` reads three facts from the environment at the boundary
(`CopyEnvironment`): tmux (`TMUX` or `TMUX_PANE`), GNU screen (`STY`), and
an SSH login (`SSH_CONNECTION`, `SSH_CLIENT` or `SSH_TTY`). A variable set
but empty names nothing. The pure `copy_plan` turns them into the order of
the attempts:

| the session | first | then | the terminal half |
|---|---|---|---|
| a desktop: no multiplexer, no SSH | arboard | only if arboard failed | the bare escape |
| an SSH login | arboard | always | the bare escape |
| inside GNU screen | arboard | always | the escape in screen's passthrough, in pieces of 252 bytes |
| inside tmux, whatever else is true | arboard | always | `tmux load-buffer -w -`; if that fails, the escape framed for tmux |

The terminal half runs even after a native copy, except on a desktop, for
two reasons:

- Over SSH the machine's clipboard may belong to the server, where nobody
  pastes from it.
- A tmux or screen session outlives the terminal that started it. A session
  started at a desktop and attached later over SSH still has the desktop's
  `DISPLAY` and no SSH variable. The native copy succeeds on the desktop's
  clipboard, and only the multiplexer reaches the terminal actually
  attached.

arboard still runs first everywhere. Under `ssh -X` its display is the
user's own, and a desktop tmux user keeps the native copy they always had.
A tmux that sends its escape on to the same desktop's terminal sets the
clipboard a second time, to the same text, after the native copy. That
order keeps the terminal's ownership of the selection from racing arboard's.

Inside tmux, tmux wins over screen and SSH: whatever else is true, the
process writes to a tmux pane.

When `tmux load-buffer` fails, the escape framed for tmux is written: once in
tmux's passthrough (each `ESC` inside doubled), then once bare. The first gets
through where tmux allows passthrough (always before 3.3, with
`allow-passthrough on` since), and the second where `set-clipboard on`. It
covers two cases:

- a tmux before 3.2, which has no `load-buffer -w`;
- a session that names tmux but cannot run it. An example is a container
  whose environment carries the host's `TMUX` (`docker/run.sh` forwards it)
  with no `tmux` and no socket inside.

A tmux allowing both sets the clipboard twice, to the same text.

A copy succeeds when either half did, and the page's toast is unchanged. An
escape is unconfirmable either way: a terminal that drops one says nothing.
When every route fails, the error names each in the order tried:
`clipboard unavailable: …; tmux: …; OSC 52: …`.

### The `tmux` command

`tmux` comes from `PATH`, the same one the user's shell runs, so a tmux
built into `~/.local/bin` on a server with an older packaged one is found.
`TMUX` and `TMUX_PANE`, which tmux set for the app's pane, name the server
and the pane. tmux sends its escape to the most recently active client
attached to the pane's session.

The command's stdout goes nowhere, since the TUI owns the terminal. Its
stderr, flattened to one line, is the reason in the error. The text is
written from a thread of its own, so a tmux that never reads it cannot hold
the copy. The wait is bounded at two seconds (`TMUX_COPY_TIMEOUT`); past
that the command is killed and the escape is written instead. tmux answers
in milliseconds, so the copy stays on the event loop, like the rest of
`/copy`: every page raises its toast from the same call.

### The lease

Every page that copies something calls `Session::copy_text`. A copy the
native clipboard took replaces the held `ClipboardLease`. A copy only the
terminal took leaves the last lease held. Dropping the last arboard handle
offers its text to a clipboard manager, which could land over what the
terminal was just given, and it blocks while it does. Before, eight of the
nine callers dropped the lease on every terminal-only copy.

### Testing

- `clipboard::copy` (unit) covers:
  - reading the environment;
  - the plan for each kind of session;
  - the order of the attempts, the fallbacks and the combined error, over
    injected effects;
  - the three framings of the escape;
  - the `tmux` runner over stand-in commands: input fed, stderr reported, a
    command that never finishes killed at the timeout, a missing program.
- **Phase 169** (smoke) plays the report with two tmux servers:
  - the phase's own server is the user's terminal (`set-clipboard on`);
  - a second server at tmux's defaults is the VPS's, its client attached
    inside the first as an xterm;
  - the app runs in the second server's pane with an `SSH_CONNECTION`.

  `/copy` and `/export`'s clipboard row must land in the **outer** buffer.
  Before the fix both toasted "Copied" and left it empty.
- **Phase 28** covers the bare escape: an SSH login without tmux's
  variables, the phase's tmux (`set-clipboard on`) playing a terminal that
  takes the escape.
- **Phases 112, 122, 156 and 167** read the copy back out of tmux's buffer,
  where `load-buffer` puts it. They keep `set-clipboard on` only for a tmux
  older than 3.2, whose copy falls back to the escape. Phase 169 is the one
  held to tmux's defaults, and it guards its own version-bound checks: the
  remote buffer needs 3.2, and `allow-passthrough` exists from 3.3.

## Testing

- `app` (unit): `last_assistant_text` returns the last assistant message,
  skipping later user/system/tool items, and is `None` with no assistant message
  (and for an empty one). `/copy` via Enter (and Tab) returns
  `Action::Copy(text)`, consumes the input, closes the palette; with no
  assistant message it returns `Action::Toast(COPY_EMPTY_NOTICE)`, in an
  agent's session view as in the main one. `/copy` typed mid-turn still
  dispatches (the palette wins over the queue).
- `clipboard` (unit, pure helpers only): `base64_encode` against known vectors
  (the 0/1/2 trailing-pad cases and empty); `osc52_sequence` frames the base64 as
  `\x1b]52;c;…\x07` and refuses input over `OSC52_MAX_BYTES`.
- `src/tui/` (smoke, new Phase 28): in a tmux session with `set-clipboard on`,
  send a message, let the dummy reply finish, type `/copy`, Enter — assert (a)
  the pane shows `Copied last message to clipboard` and (b) `tmux show-buffer`
  holds the tail of the reply (the OSC 52 fallback reached the clipboard, since
  the headless env has no arboard). A `/copy` with no reply yet shows `No agent
  response to copy` in the dim info-toast colour.
