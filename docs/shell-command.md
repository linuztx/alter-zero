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
- mid-turn → **queues as a standalone `QueuedTurn::Shell` entry** (`queue_shell`),
  run locally when its turn comes — codex's action-tagged `RunShell` dispatch
  (`submit_queued_shell_prompt`) — exiting the mode and recording the full
  `!command` for ↑ recall. **Never merged** into a text batch; see `docs/queue.md`.
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

### Output too large — cap it in memory (the peak-memory fix)

A command like `! tree ~/` can emit tens of MB. Reading that whole thing into a
`String` (what `read_to_string` did) spikes **peak** RSS by the output size: a
~10MB `tree ~/` took the process from ~5MB to ~12MB (measured via
`/proc/self/status`'s `VmHWM`, scaling linearly — ~31MB peak for 30MB of output).
It is **not** a leak — RSS returns to baseline and repeated runs don't accumulate
— but it is a real, avoidable peak (a big enough command could OOM a constrained
box). The earlier "save the full output to a file" approach didn't help: it still
read everything into memory *first*, then wrote the file.

So the runner **caps what it keeps as it reads** (`main.rs::read_capped`, codex's
`read_capped`/`append_capped` pattern). Each pipe reader retains at most
`SHELL_OUTPUT_MAX_BYTES` (100KB) and **drains the rest** — so the child never
blocks on a full pipe (the reason for the two reader threads) — tracking whether
anything was dropped. The combined stdout+stderr is re-capped to the same limit
(cut on a char boundary). Peak RSS is now flat regardless of output size (a
`VmHWM` repro: ~2.5MB for 7/10/30MB outputs, vs 9/12/31MB before). The trade-off
is that a truncated command's *retained* output is up to 100KB (vs the old 2KB
preview) — but it lives only in `App.history`, and the peak is what mattered.

The runner sends `ToolEnd { output: head, ok, truncated }` (`truncated` true when
bytes were dropped; a normal backend tool always sends `false`). The loop calls
`App::set_tool_truncated()` before `end_tool`, so the recorded
`ToolCall.truncated` makes the **expanded** (Ctrl+O) cell append a dim `…` marker
after the last retained line:

```
! tree ~/
  ⎿ /home/me/
    ├── Codes
    ├── Downloads
    ├── Documents
    … +18514 lines (ctrl+o to expand)   ← inline: the usual peek hint

(Ctrl+O view, pinned to the bottom)
    …
    └── zzz/last-retained-line
    …                                   ← TOOL_TRUNCATED_MARKER: the cap cut here
```

`ui::tool_full_lines` appends `ui::TOOL_TRUNCATED_MARKER` (`…`) when
`tool.truncated`. Inline (`tool_lines`) needs no extra marker — the existing
`… +N lines (ctrl+o to expand)` peek hint already signals more (its count is of
the *retained* lines, so it under-counts a truncated output). The dropped bytes
are **not recoverable**: unlike a paged file there is nothing to expand to; the
`…` only says "this is where the cap cut it".

Reading goes through `String::from_utf8_lossy`, so non-UTF-8 output no longer
errors (`read_to_string` did), and a cap that splits a multi-byte char yields a
single `U+FFFD`.

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
  mode; **mid-turn Enter queues a standalone `QueuedTurn::Shell` entry** (run
  locally, never merged — two `!` commands make two entries, a text Enter after
  one opens a fresh batch, Alt+Up over it re-enters shell mode; see
  `docs/queue.md`); `begin_shell` records the `Role::Shell` header, flags the
  status + tool, and `end_turn` then returns no summary; interrupting resolves
  the command failed.
- `ui`: `message_lines(Role::Shell…)` is the dark user-style line with the red
  `! ` bullet, width-padded; a shell tool renders headerless — inline a `⎿`
  block of up to `TOOL_PEEK_LINES` lines (continuation lines aligned under the
  corner) with a `… +N lines (ctrl+o to expand)` hint when more is hidden,
  `⎿ Running…` while running; the Ctrl+O `tool_full_lines` is headerless too
  (no `● ls` bullet) and shows the retained output uncapped under `⎿`,
  whitespace preserved verbatim (`wrap_verbatim` — `ls -l`/`tree` alignment
  survives; `wrap_text` stays for messages); a
  truncated output (`tool.truncated` set) appends a dim `…`
  (`TOOL_TRUNCATED_MARKER`) line after the last retained line in the expanded
  view, while a complete output appends nothing; `conversation_lines` and
  `transcript_lines` keep the
  cell flush (no spacer after the Shell header); shell mode swaps the
  composer prompt to a red `! `; `footer_rows` is 1 in the mode without session
  info; the footer slot shows the red `Shell mode`, displacing
  `{model} · {cwd}`; the cursor stays in the box; the shortcuts band lists `!`;
  `tool_header` still omits `()` for an empty-args backend tool.
- `app`: `set_tool_truncated` flags the running tool and the flag survives
  `end_tool`; it is a no-op when no tool is running.
- `main.rs` (smoke, Phase 19): typing `!echo …` shows the `! echo …` prompt and
  the `Shell mode` footer (never `❯ !echo`); the run commits the exec cell
  (`! echo …` header + `⎿` output, no `●` header, no `Ran for` summary); a
  failing command reports `[exit status: N]`; `!sleep 9` then Esc commits the
  interrupt notice promptly. **Phase 22**: a `!` command with >100KB output
  (`seq 1 50000`) renders its retained head with the `+N lines (ctrl+o to
  expand)` peek hint, writes **no** `/tmp/inline-tui-shell-*.txt` file (the
  output is capped in memory, never saved), and the Ctrl+O view ends with the `…`
  truncation marker.

## Known limitations (v1)

- ~~**Mid-turn `!command` is not run**~~ — **resolved** (codex parity): a
  `!command` submitted while a turn streams now queues as a standalone
  `QueuedTurn::Shell` entry and runs locally when its turn comes (never merged
  into a text batch). See `docs/queue.md`. Idle dispatch is the same `run_shell`
  path.
- **Output past `SHELL_OUTPUT_MAX_BYTES` (100KB) is dropped**, not saved — only
  the retained head is kept (with a `…` marker). The cap bounds peak memory; the
  trade-off is the tail is unrecoverable. Bump the const (or add head+tail
  retention, codex's `HeadTailBuffer`) if more is needed.
- stdout and stderr are **concatenated**, not truly interleaved.
- The interrupt notice is the shared `Conversation interrupted…` text.
- No timeout — a hung command runs until Esc (codex caps at 1 hour).
