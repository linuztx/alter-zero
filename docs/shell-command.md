# `!` for shell commands — run a local command from the composer

Date: 2026-06-13

## Goal

Typing **`!command` and pressing Enter runs that command locally** — a shell
escape hatch in the composer, the way openai/codex's `!` shell mode works. The
command and its output land in the conversation like a tool call (green on
success, red on failure, expandable in the Ctrl+O view), and a long command is
interruptible with Esc.

## What codex does (from `/tmp/codex/codex-rs`)

- **Trigger**: typing `!` as the first char flips `DraftState::is_bash_mode`
  and *absorbs* the `!` out of the textarea (`chat_composer.rs:3349`,
  `sync_bash_mode_from_text`). The footer hint becomes `Shell mode` in
  **light red** (`shell_mode_footer_line`, `chat_composer.rs:3152`). Esc on the
  now-empty composer exits the mode.
- **Submit**: Enter strips the `!` and runs the rest
  (`input_submission.rs:24` `submit_queued_shell_prompt`). An empty `!` instead
  posts a help notice (`Prefix a command with ! to run it locally`).
- **Execution** (`core/src/tasks/user_shell.rs`): the user's login shell
  (`-lc`), **no sandbox**, a 1-hour timeout, spawned async so the model turn
  isn't blocked; multiple may run concurrently. Output streams back via
  `ExecCommandBegin`/`ExecCommandEnd`.
- **Rendering** (`exec_cell/render.rs:375`): a `You ran` exec cell, command
  syntax-highlighted, stdout+stderr aggregated, up to 50 lines, exit code shown.
- **Interrupt**: Esc/Ctrl+C aborts — exit code `-1`, `command aborted by user`.
- **Context / history**: the output is injected into the *model's* context for
  the next turn (`user_shell.rs:399`), and the full `!command` text is recorded
  in the input history for ↑ recall (`slash_commands.rs:214`).
- **Mid-turn**: allowed — Tab queues a `!command` while the model streams, and
  it dispatches after the turn (as an auxiliary task).

## The mapping onto this codebase

We have no real model, so "inject the output into the model's context" is moot
— a `!command` is **purely local**. Everything else maps cleanly onto the
existing turn + tool machinery, so a shell run reuses the streaming strip
(spinner, elapsed timer, `esc to interrupt`), the collapsed/expandable tool
cell, the resize repaint, and the queue — almost no new rendering.

### Detecting shell mode — `app::shell_query` (pure)

Mirrors `command_query` (the `/`-palette detector), but for `!` and **not**
stopping at whitespace (a command has spaces):

```rust
pub fn shell_query(input: &str) -> Option<&str> {
    input.strip_prefix('!')   // Some(rest) for "!…", None otherwise
}
```

We keep the `!` **in the textarea** (our `/token` convention) rather than
absorbing it like codex — simpler, no cursor arithmetic, and the visible `!`
plus the `Shell mode` footer reads just as clearly. `!`-prefixed `/text` is a
literal shell command (no palette), since `command_query` only fires on a
leading `/`.

### Submit routing — `Action::RunShell(String)`

A new `Action`. In `on_key_conversation`'s Enter arm, **after** the
mid-turn-queue check (so a `!command` typed while a turn runs queues like any
follow-up — a v1 limitation: it's then sent to the backend as literal text, not
run; codex instead dispatches queued shell commands locally) and **before** the
normal submit:

- idle + `shell_query` is `Some(rest)`, `rest.trim()` non-empty →
  `Action::RunShell(rest.trim())`, recording the full `!command` in
  `input_history` (codex records the full text → ↑ recall / Ctrl+R find it).
- idle + empty command (`!` or `! `) → `Action::Notice(SHELL_EMPTY_NOTICE)`.

### Running it — `App::begin_shell` + the boundary runner

`begin_shell(command)` (pure) sets up the turn so the existing strip/interrupt
paths just work, treating the command as the turn's single tool:

- `streaming = Some(String::new())` — an **empty** buffer (a shell turn has no
  assistant text). This is what makes `is_streaming()` true, so the strip shows
  and a second mid-run Enter queues, exactly like an AI turn; `finish_stream`
  returns `None` for the empty buffer so no phantom message commits.
- `status = Some(TurnStatus { verb: "Running", done_verb: "Ran", … })` — the
  live status reads `Running…`, the summary `Ran for Ns`.
- `start_tool(command, "")` — the command shows as the running (blue) tool in
  the preview immediately, with **no args** (`tool_header` omits the `()` when
  args are empty), so it renders `● {command}`.

`main.rs::run_shell` (the I/O boundary, like `start_turn`): echo
`❯ !command` to scrollback and history (`record_user_message`), `begin_shell`,
then spawn `spawn_shell_command` on a background thread that — reusing the
`StreamEvent` channel and `CancelToken` like `DummyAi` — runs
`sh -c {command}` with piped stdout/stderr drained on reader threads (no
pipe-buffer deadlock), and sends `ToolEnd { output, ok }` then `StreamDone`.
The existing `on_stream_event` arms commit the collapsed tool cell and the
`Ran for Ns` summary; **Esc** routes to the existing `Action::Interrupt`
(cancel kills the child, `interrupt_turn` resolves the tool as failed). Output
is stdout then stderr concatenated; a non-zero exit appends `[exit status: N]`
and resolves the cell red.

### Footer — the `Shell mode` line (`ui.rs`)

Like the Ctrl+R search line, the shell-mode hint takes the **footer slot**:
`footer_rows` returns 1 whenever `shell_query(input)` is `Some` (even with no
session info), and `render_live` paints `  Shell mode` (red `SHELL_MODE_COLOR`,
`FOOTER_INDENT`) there instead of the `{model} · {cwd}` line. The cursor stays
in the input box (unlike search, which owns the footer cursor). A band can't be
open in shell mode (the palette needs `/`, the shortcuts band an empty
composer), so the slot is always free.

The `?` shortcuts band gains a `! for shell command` entry.

## Testing

- `app`: `shell_query` strips a leading `!` (returning the rest incl. spaces),
  `None` otherwise, `Some("")` for a lone `!`; idle Enter on `!cmd` →
  `RunShell("cmd")` recording `!cmd` in history; `!` / `! ` → `Notice`;
  `!cmd` mid-turn queues as literal text (no `RunShell`); `begin_shell` makes
  `turn_active()` and `is_streaming()` true, sets the running tool with empty
  args and the `Running`/`Ran` verbs; a `!cmd` is recalled by ↑ and found by
  Ctrl+R; interrupting a shell turn resolves the tool failed.
- `ui`: `tool_header` omits `()` for an empty-args tool (`● cmd`), keeps it
  otherwise; `footer_rows` is 1 in shell mode without session info; the footer
  slot shows `Shell mode` (red) in shell mode, displacing `{model} · {cwd}`;
  the shortcuts band lists `!`.
- `main.rs` (smoke, Phase 19): `!ls` shows the `Shell mode` footer; `!echo
  hello` runs and the cell shows `● echo hello` + `hello` + a `Ran for` summary;
  a failing command resolves red; `!sleep 5` then Esc commits the interrupt
  notice without hanging.

## Known limitations (v1)

- **Mid-turn `!command` is not run** — it queues as a normal follow-up and is
  sent to the backend as literal text when the turn ends (codex dispatches
  queued shell commands locally). Shell dispatch happens only from an idle
  composer.
- stdout and stderr are **concatenated**, not truly interleaved.
- The interrupt notice is the shared `Conversation interrupted…` text.
- No timeout — a hung command runs until Esc (codex caps at 1 hour).
