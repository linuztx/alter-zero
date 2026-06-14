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
(spinner, elapsed timer, `esc to interrupt`), the `⎿` peek/expand cell, the
resize repaint, and the queue — almost no new rendering.

### The absorbed prefix — `App::shell_mode` (codex's `is_bash_mode`)

Typing `!` as the first character of an empty composer is **absorbed**: the
bang lives in the `App::shell_mode` flag, never in the textarea, and renders
back as the composer's prompt — the box reads `! pwd`, not `❯ !pwd`
(`SHELL_BULLET`, red). `sync_shell_mode` (codex's `sync_bash_mode_from_text`)
runs after every edit/recall, so any text gaining a leading `!` — typed,
recalled, or accepted from a Ctrl+R search — is absorbed the same way.
Backspace or Esc on the **empty** shell composer deletes the absorbed bang
(exits the mode; the Esc arm sits with the palette-dismiss precedence, before
interrupt/quit). While the mode is on, the palette never opens (`/` is a path
character) and `?` types instead of toggling the band. A Ctrl+R search
*suspends* the mode (`HistorySearch::snapshot_shell` — previews show entries
raw) and cancel restores it with the snapshot. `shell_query` (mirroring
`command_query`) is the one place the leading-`!` test lives.

### Submit routing — `Action::RunShell(String)`

In `on_key_conversation`'s Enter arm, in shell mode:

- empty/whitespace draft → `Action::Notice(SHELL_EMPTY_NOTICE)`, staying in the
  mode (codex's empty-bang help).
- mid-turn → the draft re-gains its `!` and **queues as literal text** (a v1
  limitation: codex dispatches queued shell commands locally), exiting the mode.
- idle → `Action::RunShell(draft.trim())`, recording the full `!command` in
  `input_history` (codex records the whole text; recall re-absorbs the bang).

### Running it — the exec cell (`App::begin_shell` + the boundary runner)

The committed result is one Claude-Code-style **exec cell** — the `! command`
dark header, then the output as a `⎿` block (the first line under the corner,
the rest aligned beneath it), capped inline at `TOOL_PEEK_LINES` (4) lines with
a `… +N lines (ctrl+o to expand)` hint when more is hidden:

```
! ls                           ← Role::Shell header: dark user-style line
  ⎿ index.html                 ← first output line, under the ⎿ corner
    script.js                  ← continuation lines aligned beneath it
    styles.css

! tree .
  ⎿ .
    ├── index.html
    ├── script.js
    ├── styles.css
    … +2 lines (ctrl+o to expand)   ← capped at TOOL_PEEK_LINES, rest in Ctrl+O
```

`begin_shell(command)` (pure) sets up the turn so the existing paths produce
exactly that:

- records the **`Role::Shell` header message** up front (so a mid-run resize
  repaints it) — `message_lines` renders it like a user message (dark
  full-width block) with the red `! ` bullet;
- `streaming = Some(String::new())` — an **empty** buffer: `is_streaming()` is
  true (the strip shows, a mid-run Enter queues) but `finish_stream` records no
  phantom message;
- a `shell`-flagged `TurnStatus` (`SHELL_VERB` → the live `Running…` status;
  the flag makes `end_turn` skip the `Ran for Ns` summary — **the cell is its
  own record**);
- the command as the running **shell-flagged tool**: `tool_lines` renders it
  **headerless** (no `● name(args)` — the Shell message above is the header),
  so while it runs the strip's preview row is just `  ⎿ Running…`, sitting
  flush under the committed header; on `ToolEnd` the committed `⎿` block (up to
  `TOOL_PEEK_LINES` aligned lines, then `… +N lines (ctrl+o to expand)`)
  replaces it (`result_row` does the corner/continuation alignment).
  `conversation_lines` skips the blank spacer after a Shell message so the
  repaint keeps the cell flush.

The **Ctrl+O view** renders the same shell cell **headerless** too
(`tool_full_lines` skips `tool_header` for a shell tool), so the overlay shows
the `! command` dark header (the `Role::Shell` message) and the **full** output
as an uncapped `⎿` block — never the `● command` bullet that a backend tool
gets.

`main.rs::run_shell` (the I/O boundary, like `start_turn`): `begin_shell`,
commit the header lines **without** a trailing blank, then spawn
`spawn_shell_command` on a background thread that — reusing the `StreamEvent`
channel and `CancelToken` like `DummyAi` — runs `sh -c {command}` with piped
stdout/stderr drained on reader threads (no pipe-buffer deadlock), and sends
`ToolEnd { output, ok }` then `StreamDone`. **Esc** routes to the existing
`Action::Interrupt`: the cancel kills the child (the runner returns at once
*without* joining its reader threads — a reparented grandchild like `sleep`
can hold the pipe open long after `sh` dies), and `interrupt_turn` resolves
the cell as `⎿ Interrupted by user`. Output is stdout then stderr
concatenated; a non-zero exit appends `[exit status: N]` and resolves the
cell red.

### Output too large — save to a file (Claude-Code's huge-output handling)

When a command's output exceeds `SHELL_OUTPUT_MAX_BYTES` (100KB), the runner
(`main.rs::save_if_too_large`, boundary I/O) writes the **full** output to a
unique temp file (`$TMPDIR/inline-tui-shell-{pid}-{nanos}.txt`) and streams back
only a preview — the first `ui::SHELL_PREVIEW_BYTES` (2KB), cut on a char
boundary — in `ToolEnd { output: preview, ok, saved: Some((path, total_bytes)) }`
(a normal backend tool sends `saved: None`). The loop calls
`App::set_tool_saved(path, total_bytes)` before `end_tool`, so the recorded
`ToolCall.saved` (a `SavedOutput { path, total_bytes }`) makes the cell render
the saved block instead of the normal peek:

```
! tree ~/
  ⎿
    Output too large (6.8MB). Full output saved to: /tmp/inline-tui-shell-…txt

    Preview (first 2KB):
    /home/me/
    ├── Codes
    …
```

`ui::saved_output_lines` builds it (the corner alone, then the `human_bytes`
size + path — prose so it wraps — a blank, the `Preview (first 2KB):` label, and
the preview kept verbatim/truncated, *not* whitespace-collapsed so a tree keeps
its shape). There is **no** `ctrl+o to expand` hint: the saved file is the full
output, and the Ctrl+O view renders the identical block (nothing more to
expand). On a write failure the runner falls back to streaming the full output
unsaved.

### Footer — the `Shell mode` line (`ui.rs`)

Like the Ctrl+R search line, the shell-mode hint takes the **footer slot**:
`footer_rows` returns 1 whenever `app.shell_mode` is on (even with no session
info), and `render_live` paints `  Shell mode` (red `SHELL_MODE_COLOR`,
`FOOTER_INDENT`) there instead of the `{model} · {cwd}` line. The cursor stays
in the input box (unlike search, which owns the footer cursor). A band can't be
open in shell mode (the palette is suppressed, the shortcuts band needs a
non-shell empty composer), so the slot is always free.

The `?` shortcuts band gains a `! for shell command` entry.

## Testing

- `app`: typing `!` first absorbs into the mode (mid-text it's a character; an
  edit creating a leading `!` absorbs too); Backspace/Esc on the empty shell
  composer exit the mode (Esc never quits from it); the palette and `?` band
  are suppressed in the mode; Ctrl+C records the re-prefixed draft; idle Enter
  → `RunShell(trimmed)` recording `!cmd` (recall re-enters the mode; a plain
  recall clears it; a Ctrl+R search suspends and restores it, and accepting a
  `!entry` re-enters it); an empty bang → the help `Notice`, staying in the
  mode; mid-turn Enter queues the re-prefixed literal text; `begin_shell`
  records the `Role::Shell` header, flags the status + tool, and `end_turn`
  then returns no summary; interrupting resolves the command failed.
- `ui`: `message_lines(Role::Shell…)` is the dark user-style line with the red
  `! ` bullet, width-padded; a shell tool renders headerless — inline a `⎿`
  block of up to `TOOL_PEEK_LINES` lines (continuation lines aligned under the
  corner) with a `… +N lines (ctrl+o to expand)` hint when more is hidden,
  `⎿ Running…` while running; the Ctrl+O `tool_full_lines` is headerless too
  (no `● ls` bullet) and shows the **full** output uncapped under `⎿`; a
  too-large output (`tool.saved` set) renders the `Output too large (…). Full
  output saved to: …` block + a `Preview (first 2KB):` instead (no expand hint),
  with `human_bytes` formatting the size; `conversation_lines` keeps the cell
  flush (no spacer after the Shell header); shell mode swaps the
  composer prompt to a red `! `; `footer_rows` is 1 in the mode without session
  info; the footer slot shows the red `Shell mode`, displacing
  `{model} · {cwd}`; the cursor stays in the box; the shortcuts band lists `!`;
  `tool_header` still omits `()` for an empty-args backend tool; `human_bytes`
  formats `512B`/`2KB`/`6.8MB`/`2.0GB`; a `tool.saved` cell renders the corner,
  the `Output too large (…). Full output saved to: …` notice, the blank, the
  `Preview (first 2KB):` label and the preview, with no expand hint.
- `main.rs` (smoke, Phase 19): typing `!echo …` shows the `! echo …` prompt and
  the `Shell mode` footer (never `❯ !echo`); the run commits the exec cell
  (`! echo …` header + `⎿` output, no `●` header, no `Ran for` summary); a
  failing command reports `[exit status: N]`; `!sleep 9` then Esc commits the
  interrupt notice promptly. **Phase 22**: a `!` command with ~200KB output
  renders the `Output too large (size). Full output saved to: <path>` block + a
  `Preview (first 2KB):` (no expand hint), and the full output lands in the named
  `/tmp/inline-tui-shell-*.txt` file.

## Known limitations (v1)

- **Mid-turn `!command` is not run** — it queues as a normal follow-up and is
  sent to the backend as literal text when the turn ends (codex dispatches
  queued shell commands locally). Shell dispatch happens only from an idle
  composer.
- stdout and stderr are **concatenated**, not truly interleaved.
- The interrupt notice is the shared `Conversation interrupted…` text.
- No timeout — a hung command runs until Esc (codex caps at 1 hour).
