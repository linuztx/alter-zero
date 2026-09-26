# The bash tools — a real terminal for every command, one tool per action

The model runs commands through five tools. Every command runs in a **real
terminal** under **bash**, and each tool does one thing:

| tool | arguments | what it does | asks permission |
| --- | --- | --- | --- |
| `bash` | `command`, `wait?`, `description?` | run a command in a terminal of its own; return when it exits, stops to ask for input, or `wait` seconds pass | yes — as a command |
| `bashsend` | `session_id`, `input` | type into a running command, return its answer | yes — as session input (a lone `<C-c>` does not) |
| `bashwait` | `session_id`, `wait?` | wait for new output, the exit, or a question; `wait: 0` just checks | never |
| `bashkill` | `session_id` | stop a command and everything it started | never |
| `bashlist` | — | the running sessions, and which wait for input | never |

The companions' wire names are one lowercase word each — `bashsend`,
`bashwait`, `bashkill`, `bashlist` — the task tools' convention (`taskcreate`,
`tasklist`): the cell shows `BashSend`, and the display name lowercases to the
wire name, so the context replay needs no table for them.

It replaces `bash`'s `tty`, `timeout` and `run_in_background` flags and the
four-actions-in-one `bash_session` tool (`docs/interactive-shell.md` has that
design's history, and the pty engine both share).

## Why

What the old shape asked of a model, measured through the real executor
(`examples/pty_drive.rs`):

- **A mode picked before the command ran.** `tty` × `run_in_background` made
  four behaviours with different stdin, timeout and return rules, and
  `tools::tty_hint` existed to rescue a model that picked the wrong one.
- **`timeout` meant three things, in milliseconds.** A kill deadline on a
  plain call, a wait budget with `tty`, ignored in the background — and a
  model that meant seconds lost its command: `"timeout": 30` killed
  `sleep 1; echo finished` after 30 ms.
- **`bash_session` was four actions in one call shape** — type, wait, kill,
  type-then-kill — told apart by which optional fields were set. Most of its
  live-model failures came from exactly that: `stop` read as *stop waiting*,
  a last answer sent with `kill`, `"input": ""` meant as Enter, each patched
  with an alias or a note.
- **`bash` ran `sh -c`**, which is dash on Debian, Ubuntu, Kali and this
  project's own Docker image: `[[ … ]]`, `source`, `{1..3}` and `pipefail`
  failed with `sh: 1: [[: not found`.
- **Long output was cut silently.** A plain call kept the first 64 KB,
  stopped mid-line and told the model nothing (the `…` was the cell's alone);
  a `tty` call kept only the last 2000 lines. Either way one end was lost.

## The contract

### `bash`

```json
{"command": "cargo test", "wait": 300, "description": "Run the test suite"}
```

The command runs as `bash -c {command}` — `sh -c` where no `bash` is
installed (`subprocess::tool_shell`) — in a **pseudo-terminal of its own**
(120×40, `docs/interactive-shell.md` *The terminal*), in the working
directory. The call returns when the first of these happens:

- **the command exits** — `Exit code: N` over its output, exactly the frame a
  plain call always had;
- **it waits for input** — `Running (session {id}, waiting for input)` over
  what it printed (`waiting for a password — only bashsend can type it` at
  a password prompt), the command still running;
- **`wait` seconds pass** — `Running (session {id})` over the output so far.
  **The command is not stopped**: it keeps running as a session, the model
  continues it with `bashsend`/`bashwait`/`bashkill`, and it is told when
  the command exits. `wait` is seconds (default 120, max 600), a count models
  write in one unit only.

`wait: 0` starts the command **in the background**: the call returns at once
with the session id and the path of its log, and the completion notice
follows when it exits. A command that ends in a lone `&` is taken the same
way — the `&` dropped — since that is what the model asked for, and `&` in a
command whose shell then exits would otherwise stop what it started.

Nothing a model wrote before is refused. The executor still reads the old
spellings — `timeout` / `timeout_ms` in milliseconds, `run_in_background:
true` as `wait: 0`, `tty` (every command has a terminal now) — so a hook's
`updatedInput`, an older fixture or a model's habit keeps working; the schema
simply no longer offers them.

### `bashsend`, `bashwait`, `bashkill`, `bashlist`

```json
{"session_id": "b7x2k9m1q", "input": "import math; print(math.pi)<Enter>"}
{"session_id": "b7x2k9m1q", "wait": 600}
{"session_id": "b7x2k9m1q"}
{}
```

- `bashsend` types `input` (the key notation of `docs/interactive-shell.md`
  — `<Enter>`, `<C-c>`, `<Up>`, a newline pressing Enter) and returns the
  answer once the program waits again, exits, or its fixed budget
  ([`SEND_WAIT`], 10 s) passes. It never kills.
- `bashwait` types nothing and waits up to `wait` seconds (default 120) for
  the exit or a question — silence does not end it, so one call waits out a
  long build. `wait: 0` returns what is new at once. A program already
  waiting on keys the model has seen — a REPL at its prompt, nothing new
  printed — ends the wait at once rather than holding it to its budget.
- `bashkill` asks the command to stop — `SIGINT`, then `SIGTERM` — and
  `SIGKILL`s whatever is left after [`KILL_GRACE`] (2 s), the session's whole
  process tree included: a `docker compose up` stopped with `SIGKILL` alone
  leaves its containers running.
- `bashlist` names every running session — its id, its command, how long it
  has run, and whether it waits for input — so a model that lost an id to a
  `/compact` or a `/resume` finds it without guessing. A session at a password
  prompt is listed with the report's own clause (`waiting for a password —
  only bashsend can type it`), so a model that learns of the prompt from the
  list is told the same thing a report would tell it.

Every result is the same small grammar: `Exit code: N`, `Running (session …)`
with or without `waiting for input`, `Stopped (session …)`, and the output
since the model's previous look.

The legacy `bash_session` tool is still **executed** (a resumed conversation
may hold calls to it, and a model may remember it) but no longer offered, and
its recorded calls replay as the tool that does the same thing now: a kill as
`bashkill`, typed input as `bashsend`, anything else as `bashwait`
(`context::record_calls`), so every call a request carries names a tool the
request offers.

## What a terminal by default needed

Running every command in a terminal was the other design
`docs/interactive-shell.md` compared this one against, and turned down for
three reasons: output should be *data*, programs format differently for a
terminal, and emulating one costs something. Flipping the default alone would
also have broken quiet commands. What came with the flip:

1. **A quiet command is not a waiting one** (`pty::settle`). A terminal
   launch used to return after 2 s of silence (`LINE_QUIET`) whatever the
   command was doing: `sleep 4; echo done` came back `Running` at 2.02 s and
   then exited with nobody watching — a completion notice, and possibly an
   automatic follow-up turn, for every quiet `curl`, link step or `terraform
   plan`. Now silence ends a launch or an input only when the kernel cannot
   see the program at work: a tree the probe sees **all at work**
   (`Probe::Idle`) runs on to its exit or its `wait`; a tree waiting on
   descriptors (`Probe::Polling` — a network client, an idle server) or one
   the probe cannot see (`sudo`, macOS) must be silent for [`LAUNCH_QUIET`]
   (10 s) before a launch returns. A prompt still ends a call in half a
   second, as before.
2. **Finished output is data** (`pty::session`'s data view). The terminal's
   transcript is what a person reads — tabs expanded to stops, trailing
   spaces trimmed — so `printf 'a\tb   \n'` came back as `a       b`. The
   same stream is folded a second time the way a plain call always folded a
   pipe (`pty::fold`), and a command that **exits inside its own call**
   without drawing a screen — no cursor addressing, no moves back over its
   lines, no alternate screen — reports that fold: tabs, trailing spaces and
   all, exactly what the pipe returned.
3. **Long output keeps both ends** (`pty::fold::HeadTail`). The first
   [`HEAD_BYTES`] and the last [`TAIL_BYTES`] are kept, and the cut says so on
   a line of its own — `[… 12345 lines omitted — the full output is in
   /tmp/alter-zero-1000/{session}/tasks/{id}.output]` — naming the log every
   session already writes (`crate::background`), which the model can `read`
   or search. The log keeps every line: a burst read in one chunk used to
   lose lines from its middle to the transcript's 2000-line retention cap,
   which a line feed applies before the chunk's finished lines are taken, so
   `SessionIo::absorb` feeds it a thousand lines at a time. A pipe fallback
   with no log says how much it dropped.
4. **A background session that stops to ask something says so.** Before, a
   notice was posted only at an exit; a dev server asking `Port 3000 is in
   use. Use another? (Y/n)` would have waited unseen. The monitor now posts a
   notice (`BgEvent::Waiting`) when an announced session nobody is waiting on
   has sat quiet for 3 s at a prompt it printed since the model last looked —
   reported the way an exit is: an amber `● Background command "…" is
   waiting for input` cell, a note naming the session, the idle follow-up
   turn for a model launch. Only a line shaped like a prompt counts, which
   the probe then checks is not a program at work, and a session told of is
   not told of again until the model has looked at it, so a program that
   keeps asking cannot start turn after turn.
5. **The session cap counts what outlived its call.** The 16-session cap
   counted every terminal, including a command still inside its own call, so
   concurrent subagents' ordinary commands could have hit it.
6. **bash, not sh.** Resolved once per process from `PATH`; a non-login,
   non-interactive shell, so the Docker image's virtualenv (`ENV`, not
   `.bashrc` — `docs/docker.md`) still reaches every command.
7. **A pipe where no terminal can be had.** Off Unix, or where neither
   `setsid` nor the helper can give the command a controlling terminal, the
   command runs on a pipe as it always did — same frames, same data view —
   and a `wait` that passes hands it to the background registry rather than
   killing it; with no registry at all (an embedder, a test) it is killed at
   its `wait`, the one thing that can be done with it.

The trade-offs that stay are the terminal's own, and the result says what
happened each time, so none of them is silent:

- **Terminal formatting.** `ls` prints columns, `ps` cuts at 120 columns,
  colours are stripped, a progress bar shows its final state. `| cat` gives a
  program a pipe to format for.
- **A program that reads stdin waits for it.** `grep needle` with no file
  returns `Running (session …, waiting for input)` in half a second instead of
  `Exit code: 1`; `< /dev/null` gives it end-of-file.
- **`sudo` asks for a password** instead of failing: the frame says `waiting
  for a password — only bashsend can type it`, and the model is told never to
  type one it was not given. The clause names the one way in because the
  user has none: told only that typed input was hidden, a model asked the
  user to "enter it in the terminal prompt", a terminal they cannot reach —
  now it asks for the password in chat.
- **Output streams.** A program that line-buffers only on a terminal
  (Python, most CLIs) shows its output as it goes.
- **A job left behind is not always named.** `server & curl …` stops
  `server` when the command exits, and the report says so
  (`tools::REAPED_NOTE`) when the monitor finds the job still in the
  command's process group. The terminal hangs that group up in the same
  instant (the job is in its foreground process group), so a job that does
  not ignore the hangup can already be gone when the monitor looks — on a
  machine whose init reaps orphans at once, a plain `sleep 30 &` went
  unnamed in 4 runs of 20. It is stopped either way; only the note is
  missing. On a pipe nothing hung the job up, so the note was exact there.

The emulation's cost measured nothing on a trivial command: `echo hi` took
0.00 s either way through the executor.

## Wiring

- **Permissions** (`llm::approval::permission_request`): `bash` asks as a
  command, `bashsend` as session input (inheriting the launch command's
  allowlist rule, `docs/interactive-shell.md` *Permissions*); `bashwait`,
  `bashkill` and `bashlist` never ask — what used to be a special case
  inside one tool is now the tool's name. Saved rules are keyed by command
  text, not tool name, so every rule survives.
- **Auto mode's classifier** reviews `bash` commands and `bashsend` input,
  the two calls that ask.
- **Hooks** (`hooks::claude_code_alias`): each tool answers to its cell's
  display name as a second exact name — `Bash`, `BashSend`, `BashWait`,
  `BashKill`, `BashList` (and `BashSession` for the legacy tool).
- **Subagent tool lists** (`subagents::AgentTools::allows`): `Bash` grants the
  whole family, and each companion answers to its display spelling too
  (`BashSend`), which lowercases to its wire name.
- **Cells.** `● Bash(npm run dev)`, `● BashSend(python3 ← print(1)⏎)`,
  `● BashWait(cargo build)`, `● BashKill(npm run dev)` — a session named by
  its command from the moment the call is announced
  (`tools::summarize_call_naming`). The running cell's clock names the wait:
  `(22s · wait 2m)`.

[`SEND_WAIT`]: ../src/llm/tools.rs
[`KILL_GRACE`]: ../src/background.rs
[`LAUNCH_QUIET`]: ../src/pty/settle.rs
[`HEAD_BYTES`]: ../src/pty/fold.rs
[`TAIL_BYTES`]: ../src/pty/fold.rs
