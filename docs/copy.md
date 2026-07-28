# `/copy` — copy the last response to the clipboard

Date: 2026-06-20

## Goal

A `/copy` slash command that copies the **last assistant response** to the
system clipboard — a port of openai/codex's `/copy` ("copy last response as
markdown"). The outcome surfaces as a **transient toast** above the box (an info
`Copied last message to clipboard`; an error `No agent response to copy` /
`Copy failed: {e}`), which self-clears after a few seconds instead of committing
a scrollback bullet. See `docs/toast.md`.

> **Update (2026-07-08):** `/copy`'s confirmation moved from a committed
> `Role::System` / `Role::Error` message to a transient toast (`present_toast`
> in `main.rs`, `App::show_toast`). The clipboard I/O below is unchanged; only
> the surface differs. `commit_system_notice` / the `COPY_OK_NOTICE` /
> `COPY_EMPTY_NOTICE` strings still exist (the strings feed the toast now; the
> commit helper still serves idle `/help`). The rest of this doc describes the
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
- **No SSH/WSL detection.** We always try arboard first; on a remote host that
  *has* a clipboard server, arboard would set the *remote* clipboard (codex skips
  arboard over SSH). The common headless case (no display) falls through to
  OSC 52 correctly. No WSL PowerShell path.
- **Plain OSC 52, no tmux passthrough wrap.** We rely on tmux's native OSC 52
  forwarding (`set-clipboard external`, the modern default) rather than codex's
  `\x1bPtmux;…` DCS wrap — simpler, and it's what tmux's own buffer captures (so
  it's smoke-testable). Needs `set-clipboard` ≠ `off` inside tmux.
- **No arboard stderr suppression.** Codex silences X11/NSLog noise with a libc
  `dup2`; this crate `forbid`s `unsafe`, so we skip it.
- **No rewind history.** We have no conversation rollback, so codex's "only the
  most recent 32 responses are available to /copy" eviction case doesn't exist.

## Testing

- `app` (unit): `last_assistant_text` returns the last assistant message,
  skipping later user/system/tool items, and is `None` with no assistant message
  (and for an empty one). `/copy` via Enter (and Tab) returns
  `Action::Copy(Some(text))`, consumes the input, closes the palette; with no
  assistant message it returns `Action::Copy(None)`. `/copy` typed mid-turn still
  dispatches (the palette wins over the queue).
- `clipboard` (unit, pure helpers only): `base64_encode` against known vectors
  (the 0/1/2 trailing-pad cases and empty); `osc52_sequence` frames the base64 as
  `\x1b]52;c;…\x07` and refuses input over `OSC52_MAX_BYTES`.
- `main.rs` (smoke, new Phase 28): in a tmux session with `set-clipboard on`,
  send a message, let the dummy reply finish, type `/copy`, Enter — assert (a)
  the pane shows `Copied last message to clipboard` and (b) `tmux show-buffer`
  holds the tail of the reply (the OSC 52 fallback reached the clipboard, since
  the headless env has no arboard). A `/copy` with no reply yet shows `No agent
  response to copy`.
