# Interactive shells — `bash` with `tty`, and `bash_session`

The `bash` tool used to be **execute-only**: a command ran with stdin on
`/dev/null` and no terminal, and the call waited for it to exit. That is the
right default — most commands a coding agent runs want exactly that — but a
large class of useful programs needs more:

- **prompts** — `npm init`, `apt install` (`[Y/n]`), `git add -p`, `ssh`'s
  host-key question, a script's `read -p`, a password prompt;
- **REPLs** — `python3`, `node`, `psql`, `sqlite3`, `gdb`, `pdb`, `irb`,
  a shell that keeps its `cd` and its virtualenv between commands;
- **full-screen programs** — `vim`, `less`, `htop`, `top`, a `git rebase -i`
  editor, a scaffolder's arrow-key menu;
- **programs that only behave on a terminal** — line-buffered progress, colour,
  `isatty()` checks that change what a tool does, or refuse to run at all.

`bash` gains a `tty` flag that runs the command in a real **pseudo-terminal**
and returns while it is still running, and a companion tool, **`bash_session`**,
types into it, waits for it, reads it, and ends it.

## What the model sees

### `bash` — one new parameter

```json
{"command": "python3 -q", "tty": true}
```

`tty: true` runs the command in a pseudo-terminal (120 columns × 40 rows) as
the controlling terminal of a fresh session. The call returns as soon as the
command **exits**, **stops to wait for input**, or its `timeout` passes —
whichever comes first. A command that exited reports exactly what a plain
`bash` call reports (`Exit code: N` over the output). A command still running
reports the output so far under a one-line frame naming its session:

```
Running (session b7x2k9m1q, waiting for input)
>>>
```

`tty: false` (the default) is unchanged: stdin on `/dev/null`, no terminal, the
call waits for the exit and kills the command at `timeout`. Non-TTY stays the
default deliberately — a terminal changes how programs format output
(colour, progress bars, `\r` redraws), and a batch command gains nothing from
one. The `bash` description says when to reach for it in one bullet: prompts
(`[Y/n]`, passwords), REPLs, full-screen programs — *and retry with it when a
command says it needs a terminal* — preferring a non-interactive flag (`-y`,
`--yes`) when one exists.

`tty` also combines with `run_in_background`: the command starts in a terminal
and the call returns at once with its session id, the completion notice
following when it exits. Every background launch names its session id now, so
`bash_session` can take any of them back.

### `bash_session` — the companion tool

```json
{"session_id": "b7x2k9m1q", "input": "import math; print(math.pi)<Enter>"}
```

| parameter | meaning |
| --- | --- |
| `session_id` | required — the id a `bash` call reported |
| `input` | what to type; a newline presses Enter, and keys go in angle brackets: `<Enter>`, `<Tab>`, `<Esc>`, `<BS>`, `<Up>`/`<Down>`/`<Left>`/`<Right>`, `<PageUp>`, `<C-c>`, `<C-d>`, `<C-z>`, `<M-x>`, `<F1>`… Omit it to just wait for more output. |
| `timeout` | the most milliseconds to wait for the command to respond (default 10000, max 600000) |
| `kill` | `true` ends the command and everything it started — only to abandon it |

The result is **only the output printed since the model last looked**, under
the same frame: `Running (session …)` while it runs, `Exit code: N` once it has
exited (after which the session is gone), `Stopped (session …)` after a kill.
At a password prompt the frame says so — `Running (session …, waiting for a
password — typed input is hidden)` — which also tells the model why the
password it types will not show in its next look (*A password prompt says so
itself*, below).
A full-screen program — one on the terminal's alternate screen — returns its
**current screen** instead of a transcript, since a stream of cursor-addressed
redraws means nothing as text:

```
Running (session b7x2k9m1q, waiting for input)
Screen (40x120, cursor at line 3, column 14):
inserted line
line one
line two
~
                                                    1,13          All
```

`bash_session` works on every shell the registry holds, not only TTY ones: a
`run_in_background` command (or one the user moved to the background with
Ctrl+B) can be waited on and ended the same way. Only a TTY session accepts
typed text — a background command's stdin is `/dev/null` — but a lone `<C-c>`
still interrupts one, as a `SIGINT` to its process group.

The description the model reads is short and imperative (`src/llm/tools.rs`,
pinned under 900 characters by a test): the key notation, **answer one prompt
per call, ending the answer with `<Enter>`, and read what it asks next**, a
wait needs no input, `kill` is only for abandoning a command (*one that
finishes exits by itself*), and a password or secret it was not given is never
guessed — it asks the user. The `input` examples spell Enter as a key
(`"y<Enter>"`), never `\n` — see *What live models taught the design*.

## Why this shape

**Two tools, not one.** `bash` keeps meaning "run a command"; everything that
happens *to a running command* goes through `bash_session`. An overloaded
`bash` (OpenHands' `is_input` flag) serializes the agent onto one terminal and
makes "is this text a command or keystrokes?" a question the model must answer
in every call. Two tools with one id between them also make concurrency free:
several sessions, a subagent's sessions, and background shells all coexist.

**The yield is decided for the model.** Codex's `exec_command`/`write_stdin`
return after a fixed `yield_time_ms` the model picks, and models pick badly:
too short and they poll in a loop, too long and every keystroke into a REPL
waits out the clock. Here the call returns when the command **settles**
(`pty::settle`, pure and unit-tested):

- it **exited** — at once, once its output is drained;
- the kernel says a thread of it is **blocked reading its terminal**, or the
  terminal is **reading a line with echo off** — a password prompt, told by
  the terminal itself where the kernel cannot see — and it has been quiet for
  `PROMPT_QUIET` (0.5 s): no prompt and no output of the call's own needed,
  so a bare `read x` or a `cat` waits too, and so does a wait begun after the
  prompt came up (*The kernel's word* and *A password prompt says so itself*,
  below);
- it printed something and went quiet for `PROMPT_QUIET` (0.5 s) while the
  terminal **awaits keys** — a prompt (`>>> `, `Password: `, `[Y/n] `) left
  the cursor mid-line, a full-screen program is on the alternate screen, or
  the program reads the terminal **key by key** (a menu, an editor, a
  readline prompt), which is what `waiting for input` says;
- it has been silent for `LINE_QUIET` (2 s) since the call's input — a command
  that answered with whole lines and went quiet — or, after a password the
  call submitted, for `CHECK_QUIET` (10 s) until the program answers it;
- the call's `timeout` passed.

Three things look like a prompt and are not (`SessionIo`'s `awaiting_keys`):

- **a busy program.** `Compiling foo... ` left open while the compiler runs,
  `Working... ` before a `sleep`, a download stalled mid-bar: the cursor sits
  after text, as at a prompt. When the kernel sees every process of the
  session at work — running, sleeping, reading a pipe, waiting on a child —
  the command is busy whatever its screen shows, and nothing below is asked;

- **an animated line.** A progress bar redrawn after a `\r`, a spinner, a
  counter, dots appended to `Downloading…` — each leaves the cursor exactly
  where a prompt would, and pauses whenever the download does. The transcript
  cuts the output into **bursts** (`BURST_SPAN`, 100 ms — the writes of one
  frame fall inside one) and counts, per line, the bursts that changed the
  text an earlier burst had left on it — overwritten, erased or extended;
  a repaint to the same text is no change. A line changed by two bursts since
  the program was last typed into is **animated**
  (`Transcript::cursor_line_animated`), and a cursor on it is no prompt,
  whatever else the terminal says. Typing resets the count — a menu moves its
  highlight once per key, and that redraw is the answer to the key, not an
  animation — and so does a fresh line: a question printed after a progress
  bar is a prompt at once;
- **a relay's raw mode.** `sudo` (which runs its command in a terminal of its
  own and relays it, the default since sudo 1.9.14), `ssh` and
  `docker run -it` hold the session's terminal raw for as long as their
  command runs. A key-reading program switches off canonical mode but leaves
  **output processing** (`OPOST`) on — readline, Node, `prompt_toolkit`,
  `stty -icanon`, `read -n1` all do — while a relay switches that off too,
  passing through output already processed at the far end. So a terminal
  counts as read key by key only with `OPOST` still on
  (`LineMode::reads_keys`); behind a relay the program is judged by its
  screen alone.

A `bash_session` call with **no input** is a *wait*, and the silence rule does
not apply to it: a build that pauses between lines is still working, so a
wait returns only on an exit, a prompt, or its timeout. That is what lets the
model wait for a long command in one call instead of polling it. A wait also
asks more of a prompt: one that appears during it must stay quiet for
`WAIT_PROMPT_QUIET` (3 s), not 0.5 — the model waits because it believes the
command busy, and a line a busy command leaves open while it works
(`Reading package lists... `) looks just like a question. A real question
still ends the wait within seconds.

**A wait counts what the model has not seen** (`IoState::printed_since`), not
only what arrives during it. A command that works quietly past `LINE_QUIET`
returns its call `Running`, and its question often comes up in the seconds
the model spends deciding what to do next: by the time the model's wait
begins, the question is already on the screen and prints nothing more. A
wait that watched only for output of its own sat out its whole timeout
there — ten minutes, if the model asked for them — wherever the probe could
not see the program read (`sudo apt install`'s `[Y/n]`, a REPL waiting in
`poll`, anything off Linux). So the session remembers where the model last
looked (`SessionIo::look`), and a wait takes everything printed since then as
new: a question asked while the model decided is as new to it as one asked
while it waited, and ends the wait by the same `WAIT_PROMPT_QUIET` rule —
at once, once the question has sat that long. What the model has already
seen does not: a wait begun on a prompt its last report showed is the model
choosing to wait, and it waits. The wait still looks for `PROMPT_QUIET` itself
before it settles on output older than the call, which is what gives the probe
its word — the monitor probes within a few milliseconds of a wait beginning,
and a tree it sees at work is busy whatever its last line looks like.

**Where the session stands is read at report time.** `waiting for input` is
not only how the call's wait happened to end: a poll that saw nothing new ends
on its timeout, yet the program is no less at its prompt, so the frame is
recomputed from the terminal when the report is composed
(`SessionIo::waiting`), by the same rule as the wait — a pure wait that saw
new output asks for `WAIT_PROMPT_QUIET`, one that saw nothing found the
program sitting where it was throughout. And a look that found nothing new
names the line the terminal is still at — `(no new output — still at: Full
name:)` — so a model that lost track across its polls is told what it is
being asked.

**Keys are notation, not escape codes.** Models are unreliable at emitting
`\u0003` in JSON but fluent in Vim/tmux key notation, so `input` understands
`<Enter>`, `<C-c>`, `<M-x>`, `<F5>` and friends (case-insensitive; `<lt>` is a
literal `<`). Anything in angle brackets that is not a key name is typed as
written, so `a<b` and `<div>` survive. On a TTY a newline is sent as `\r` —
the byte a terminal sends for Enter, which cooked mode turns back into `\n`
and raw-mode programs (menus, editors) expect. Arrow keys follow the program's
cursor-key mode (`ESC O A` under DECCKM, `ESC [ A` otherwise), read off the
emulated screen. A lone `<Esc>` is followed by a short pause before the rest of
the input, so Vim does not read `<Esc>:` as Alt+`:`.

**Two views of one byte stream.** Everything the program writes is fed to two
parsers (`pty::screen` and `pty::transcript`):

- a **screen** — the `vt100` crate's emulator at the pty's size, with no
  scrollback (≈150 KB a session): what a human would see. It is what a
  full-screen program returns, what the ↓ manager shows, and where the
  terminal state lives that input depends on (cursor-key mode). It also
  **answers terminal queries**: a cursor-position report (`ESC [ 6 n`), device
  attributes, the colour queries — programs like `vim` and `prompt_toolkit`
  REPLs ask, and some wait a long time for an answer no pipe ever sends;
- a **transcript** — the stream as lines of text, built by our own
  `vte::Perform`: carriage returns and backspaces overwrite (a progress bar
  collapses to its final state), erase-in-line and column moves apply, colour
  and every other escape vanish. It is what an ordinary program returns: the
  model gets the output since its last look, not a 40-row window of it.

A look delivers **every line that is new or whose text changed** since the
previous look, in order, and nothing else: each line remembers (hashed) the
text the model was last handed. The prompt the model answered comes back with
its answer echoed after it — `>>> import math; print(math.pi)` then `3.14…`
then the new `>>> ` — because echoing the answer changed that line; a
progress bar redrawn in place comes back once, as it stands now, even when it
sits above the line the cursor was on (pacman moves up to redraw a bar), and
the finished bars around it, unchanged, are not repeated. So the reports and
the cells built from them append — each call adds what is new, and the
context never carries the same unchanged line twice. A full-screen program's
screen is shown under whatever the main screen printed before it took over
(`git commit`'s hints before the editor), so nothing is lost at the switch. A
delivery keeps the **tail** when it is over `SESSION_OUTPUT_MAX_BYTES` (32 KB),
since the newest lines and the prompt are what an interactive step is about,
and says how many lines it left out.

**The kernel's word** (`pty::probe`, Linux). The screen can only offer a
shape; the kernel knows. `/proc/PID/task/TID/syscall` names the system call a
sleeping thread is blocked in, and for a read its first argument is the file
descriptor, which `/proc/PID/fd/N` resolves to a path. So the monitor walks
the session's process tree (`/proc/…/children`) and classifies every thread:
blocked in `read` on the session's terminal — its `/dev/pts/N`, or
`/dev/tty`, where `ssh` and `git` read a password — is **reading**; in a
`poll`/`select`/`epoll` wait it **may** be waiting on the terminal (a REPL,
`vim`, `ssh` and every network client wait that way); anything else is at
**work**. One reader anywhere is `Probe::Reading`, a tree all at work is
`Probe::Idle`, and everything else — a possible poll, a process the probe may
not inspect, no `/proc` — is `Probe::Unknown`, which leaves the screen rules
in charge. The files are readable only for the user's own processes, so
`sudo` and everything it runs leave the probe blind: exactly where the relay
rule above already applies. The monitor probes only a terminal a call is
waiting on, once it has been quiet `PROBE_QUIET` (0.2 s), at most every
`PROBE_INTERVAL` (0.2 s) — a handful of file reads — and any output or input
makes the verdict stale at once. (Ported from the alternative design this one
was compared against, *Compared with the terminal-sessions design*, below.)

**A password prompt says so itself** (`LineMode::hides_input`). A program
asking for a password turns the terminal's echo off and reads a whole line —
`sudo`, `ssh`, `git`'s credential prompt, `getpass`, `read -s` — and that
mode belongs to the terminal, read through its master, where the probe needs
the program's own `/proc` files. So it holds exactly where the probe is
blind: `sudo` runs as root. That blindness once kept a model waiting ten
minutes. After a wrong password `sudo` checks it, says `Sorry, try again.`
and asks again — seconds later, after the call that typed the password had
returned on `LINE_QUIET` — and the model's next call, a wait, saw no output
of its own to settle on, so it rode out its whole timeout. Now a line read
with echo off ends any call once the terminal has been quiet `PROMPT_QUIET`
(0.5 s), as the kernel's read does, and the frame names it:
`Running (session …, waiting for a password — typed input is hidden)`. Two
guards keep the signal honest (`IoState::password_prompt`). An answer —
keys that end the line, an Enter or a signal key (`keys::reaches_line_reader`)
— reaches a program still in its mode, so the mode counts only once the
program has replied with output, which is also what makes `sudo`'s retry a
new prompt; text typed without its Enter has not reached it at all, and the
prompt stands, with the report's note about the missing Enter. And when the
probe sees the whole tree at work, the mode asks nothing: a script that turns
echo off to swallow type-ahead while it works is busy. The monitor reads the
mode after each chunk of output and on every idle poll (`refresh_line_mode`),
so a mode switched after the prompt was printed is seen within 20 ms. It is
detection only: what is typed shows as typed (*Compared with the
terminal-sessions design*).

**A password handed over is answered in the same call.** sudo breaks the line
the moment it has read a password, then checks it for a couple of seconds —
pam's delay on a refusal, longer for a network login — before it says a
word. The silence rule returned the call mid-check as `(no new output)`, so
every attempt cost the model a second call to learn the verdict, and a model
that took the silence for a yes typed its next answer while sudo was still
checking — with echo back on, into whatever prompt came next. So a call whose
keys submit a line (`keys::submits_line`, an Enter) at a password prompt
waits for the program's **answer**: visible text on either screen
(`Transcript::answered` — the bare line break, spaces and escapes that draw
nothing are no answer), a fresh prompt, or the exit. A refusal comes back as
`Sorry, try again.` over the next password prompt; a password let in comes
back with the command's first words. The silence rule gives it `CHECK_QUIET`
(10 s) instead of `LINE_QUIET` — room for any real check, while a command
that runs on silently once let in still returns `Running` rather than
holding the call to its timeout.

## Streaming the running cell

A waiting call streams what its report will say as it builds up, and the
cell shows it the way a terminal would: `ToolProgress::Screen { settled, live
}` from the executor, `StreamEvent::ToolScreen` on the wire, and
`App::push_tool_screen` — `settled` text appends for good, `live` rows
**replace** the `live` rows sent last. A progress bar is one row changing in
place, never a row per frame; the TUI and the model's context both read it
once.

For a session the split comes from the transcript (`Transcript::take_stream`):
lines within the screen's height of the end (`ROWS`, 40) can still be
redrawn — pacman moves up to redraw a bar — so they stream as `live`; a line
that scrolled out of reach is final and streams once as `settled`. Only what
the next look would report is streamed, so a cell never shows a line its
report will not carry. The wait sends at most every `STREAM_INTERVAL` (50 ms),
only when something changed, `settled` capped at `STREAM_MAX_BYTES` (64 KB) a
send, and once more as it settles, so the cell ends where the report begins.

**Plain commands fold the same way** (`pty::fold`). Plenty of programs draw on
a pipe as if it were a terminal — curl's meter, tqdm, ffmpeg and rsync
redraw with `\r` whether or not anyone watches, `--color=always` wraps words
in escapes — and kept raw, the model read every frame a bar ever drew, joined
on one line. `Fold` replays the stream one line at a time: `\r` returns to
the line's start, backspace and the cursor-column escapes (`CSI G`/`C`/`D`)
move along it, all overwriting; erase-in-line applies; every other escape
vanishes. Unlike the transcript it is built for data the model may copy back:
a tab stays a tab, trailing spaces stay, and a line is kept whole up to the
output cap rather than a terminal's width. A plain `bash` call streams its
ended lines as `settled` and the line still being drawn as `live`, paced like
a session (`PipeOutput`, `PIPE_STREAM_INTERVAL`); its result is the folded
text. A background shell's event stream — the ↓ manager's view and the
completion note the model reads — is folded by its monitor, while its
`.output` file keeps every byte as written; a Ctrl+B handoff replays what the
foreground runner read, raw, into that same fold, so the line in progress
finishes as one line. The user's `!` commands fold their output too
(`docs/shell-command.md`).

## What live models taught the design

The tool was tuned against real models (`examples/session_probe.rs` drives the
real agent loop with the real system prompt and prints every model-facing
result): OpenRouter's `gpt-4.1-mini`, `qwen3-coder`, `gemini-3.6-flash`,
`kimi-k2.6`, `deepseek-v4-flash` and `glm-5.3-flash`, Ollama Cloud's
`gpt-oss:120b`, and Venice's `qwen3-coder-480b`, over six scenarios: a signup
wizard that refuses a pipe, a raw-mode arrow-key menu, a `getpass` password
prompt (with and without the password in the request), `vim`, a `python3`
REPL kept open across steps, and a web server started in the background,
checked with `curl` and ended through `bash_session`. The first round of the
signup wizard completed in two of eight models; after the changes below,
seven of eight did (the eighth is a model that emits garbled tool names), and
every model found the vault code, edited the file in `vim` (or with its
non-interactive `-c`, which is the better tool) and ended its server. Each
change answers a failure seen on the wire:

- **A model gave up on "must be run in an interactive terminal".** The `bash`
  description says to retry with `tty` when a command asks for a terminal,
  and a failed plain `bash` call whose output says so (`not a tty`,
  `inappropriate ioctl for device`, `/dev/tty`, … — `tools::tty_hint`) gets a
  pointer to `tty` appended to the **model's** text only
  (`ToolOutcome::context`); the cell keeps the command's own words.
- **`stop` read as "stop waiting".** A model ended a wizard mid-answer. The
  parameter is `kill` (`stop` still parses), and its bullet says it is only
  for abandoning a command.
- **The last answer sent with `kill`.** A model paired what it believed was
  the final answer with `kill: true`; the program asked one more question and
  the kill threw the session away. Input with `kill` now types first, waits,
  and **does not kill a program that answered by asking for more** — the call
  reports it still waiting, and the model is told why nothing was killed
  (`pty::report::NOT_KILLED_NOTE`). A program the input ended reports its own
  exit.
- **Escaped twice.** A model wrote `"Ada Lovelace\\n"` in its JSON — a
  backslash and an `n` typed, and no Enter. Input that holds no control
  character of its own and **ends on an escaped one** (`\n`, `\r`, `\t`, `\e`,
  `\x03`, `\u0003`) has one level of escaping undone (`pty::keys::parse_input`);
  a backslash anywhere else — `print("a\nb")`, `C:\\` — is typed as written.
  The description's examples spell Enter as `<Enter>` too, since a `\n` shown
  in a description is exactly what that model copied.
- **Typed but never submitted.** A model typed its answer without Enter, saw
  it echoed on the screen, and took it as given. When the input ended on typed
  text and the line is still open, the model is told it is not submitted and
  to end each answer with `<Enter>` (`UNSUBMITTED_NOTE`). Two signals say the
  line is open: the terminal is in canonical mode (the program reads whole
  lines — read off the pty with `tcgetattr`, `pty::spawn::line_mode`), or —
  for a **line editor** that reads key by key, like Python's readline REPL,
  where the first signal is silent — the typed text sits echoed on the cursor's
  line with the cursor right after it (`SessionIo::holds_typed`; never on the
  alternate screen, where an editor inserts what it is typed). A menu reading
  key by key already has the keys, echoes nothing, and gets no note. Before
  the second signal a model polled a REPL three times "waiting for the output
  to flush" of a line it never submitted; after it, it pressed Enter at once.
- **Polling a program that waits on it.** A model polled ten times while the
  program sat at `Full name:`, each report saying only `Running` — the
  waiting state was recomputed at report time, and the prompt is re-read to
  the model when nothing is new (see *Where the session stands*).
- **A menu that never said it was waiting.** An arrow-key menu leaves the
  cursor at the start of a fresh line, so the cursor rule never called it a
  prompt, and every launch waited out the 2 s line-quiet fallback. The monitor
  now reads the terminal's line mode as output arrives: a program reading key
  by key is waiting when it goes quiet (a relay holding the terminal raw is
  not — see the pacman run below).
- **`sudo pacman -Syy` returned eleven times.** Reported from real use: after
  typing the sudo password, a model waited on the download with a ten-minute
  `timeout`, and every wait came back within a second or two, framed `waiting
  for input`, repeating the same finished `multilib … 100%` bar each time.
  Two causes. sudo relays with the terminal raw, which read as a program
  waiting on keys, so any pause in the download settled as a prompt; and a
  look re-sent every line from the one the cursor had sat on, so pacman's
  bars — the cursor parked on the one still moving — came back again and
  again. A relay's raw mode is now told apart by its output processing, an
  animated line is no prompt, a pure wait gives a prompt that appears during
  it longer, and a look carries only the lines that changed: the same run is
  one wait, ending on `Exit code: 0` over the bar that moved
  (`llm::exec`'s `a_relayed_progress_display_is_waited_out_and_reported_once`
  replays it).
- **Everything at once.** A model sent all four answers in one input. It works
  — a terminal buffers typeahead — but the echo lands before each prompt and
  the transcript reads garbled, and a password prompt that flushes its input
  (`getpass` does) loses what was typed ahead. The description asks for one
  prompt per call.

Some failures stay with the model, the tool having said what it could. A
REPL wants a blank line to close a Python block, and a model that sends a
`def` without one finds its next line swallowed at the `...` prompt — the
report shows that prompt, and the stronger models read it. A weaker
deployment (`qwen3-coder-480b` on Venice) ignored the unsubmitted-line note
three times running and typed three answers onto one line before confirming
the wrong account; the tool will not press Enter on a model's behalf, since
`input` is typed as-is and a line split across two calls is legitimate. The
same model through OpenRouter answered each prompt with `<Enter>` from the
start.

One failure is not ours: `kimi-k2.6` through OpenRouter is served by several
upstreams, and one of them (Inceptron) intermittently drops the `input`
argument from the model's call — its own reasoning says "let me type Ada
Lovelace" while the call arrives as a bare wait — so the session looks stuck
at its prompt until another upstream serves a request. Reproduced outside the
app with a raw request (`provider.order` pinned to each upstream).

## The terminal (`pty::spawn`)

This crate forbids `unsafe`, so the pseudo-terminal is built from `rustix`'s
safe wrappers (the `pty` feature: `openpt`, `grantpt`, `unlockpt`, `ptsname` —
Linux opens the peer with `TIOCGPTPEER`). The window size is set with
`tcsetwinsize` before the child starts, the master is close-on-exec, and our
copies of the slave are dropped the moment the child holds its own, so the
master reads `EIO` (end of stream) when the last process lets go of it. The
master is also where the session reads the program's **line mode**
(`line_mode` — `ICANON`, `ECHO`, `OPOST`): the pair shares one line
discipline on Linux and the BSDs, so no slave handle is kept.

The child must make the slave its **controlling terminal**, or a `/dev/tty`
prompt (`sudo`, `ssh`) would find no terminal at all. That needs
`setsid(2)` + `TIOCSCTTY` in the child before `exec` — the `pre_exec` hook
this crate cannot use — so the same tier chain `docs/tty-detach.md` built for
detaching does it (`subprocess::tty_tiers`):

1. **`setsid -c sh -c {command}`** — util-linux's (and BusyBox's) `-c`/
   `--ctty` makes stdin the controlling terminal after the `setsid`;
2. **the helper re-exec** — `{exe} __alter-zero-detached-tty-exec {command}`:
   `main()`'s first statement calls `rustix::process::setsid()` and
   `ioctl_tiocsctty(stdin)` and `exec`s `sh` in place (macOS and minimal
   images, where there is no `setsid` binary);
3. **no third tier.** The detach chain's last resort — an attached `sh` — would
   hand a password prompt the *user's* terminal, the bug the chain exists to
   prevent, so a TTY launch that finds neither tier is refused with an error
   the model can act on (run without `tty`).

Both tiers keep the invariant every kill relies on — `child.id()` is the
session leader, its process group and its session. Ending a session kills
that group and every process left in the session (`pkill -s`), so a shell's
own background jobs go too.

The environment is the parent's plus `TERM=xterm-256color` (the terminal the
emulator implements) and `PAGER=cat`/`GIT_PAGER=cat`/`MANPAGER=cat`/
`SYSTEMD_PAGER=cat`, so `git log` or `man` prints instead of opening a pager
nobody asked for — a program the model runs *as* a pager (`less file`) is
unaffected. Variables that describe the TUI's *own* terminal (`TMUX`,
`TERM_PROGRAM`, `KITTY_WINDOW_ID`, `COLUMNS`, …) are removed, since they would
steer a program toward features the session's emulator does not have.

## The registry (`background.rs`)

A TTY session is a **background shell** — the registry that already owns every
process outliving its tool call (`docs/background.md`). That buys the ↓
manager, the footer count, `kill_all` on `/clear` and quit, subagent
attribution and the completion notice for free; what the registry gains is:

- **input** — each TTY task has a writer thread fed by a channel (a pty write
  blocks when the program is not reading, and nothing may block while the
  task's state is locked); the monitor's query replies ride the same channel;
- **the two views** — the monitor feeds every chunk to the session's screen and
  transcript under one lock (`pty::session::SessionIo`), refreshes the line
  mode, and a waiting call blocks on a condvar beside it;
- **announcement** — a TTY task launched by a foreground call is registered
  silently and **announced** (`BgEvent::Started`) only if it outlives the
  call. A `tty` command that finishes inside its own call never appears in the
  footer or the manager, never posts a notice, and costs nothing but its cell;
- **observed exits** — when the model *saw* the exit (a result framed
  `Exit code: N` or `Stopped`), the `Exited` event says so and no completion
  notice is posted: the model already has the result, and a `[background] …
  completed` note after it would be noise, or worse, an automatic follow-up
  turn. Who reports an exit is a small handshake (`SessionIo::finish`/
  `end_wait`): a call registers as a waiter *before* it acts on the session,
  so an exit its own input or kill caused is its to report;
- **the screen for the UI** — for a TTY shell the monitor sends
  `BgEvent::Screen` (throttled to ten a second) instead of line output, and
  the ↓ manager's details page shows the rendered screen: the prompt it is
  sitting at, the menu as drawn;
- **a cap** — `MAX_TTY_SESSIONS` (16) running sessions; one more is refused
  with the list of the running ones, rather than letting forgotten REPLs pile
  up unseen.

The interim-output file (`{tasks}/{id}.output`) of a TTY session holds the
cleaned transcript, not the raw escape stream, so `read`ing it is useful too.

## Permissions

Launching a TTY command is a `bash` call like any other and asks the same
question. **Typing into a session asks too** — otherwise approving `bash`
once would approve every command later typed into it. The request is its own
kind, `PermissionKind::Session`: the title `Session input`, the input on one
line as the body (`print(2 + 2)⏎`), a dim `into python3 · session b7x2k9m1q`
under it, and option 2 `Yes, and don't ask again for this session` —
remembered on the gate for the session's life, never written to
`permissions.json`. Three things keep it quiet in practice:

- a session whose **launch command** a standing allowlist rule covers
  inherits the rule — `python3 *` already allows `python3 -c "anything"`, so
  typing into `python3` adds nothing it did not grant;
- `master` mode never asks, and **auto mode's classifier** reviews session
  input the way it reviews commands, told the program the keys go to
  (`prompts/classifier.md`: typing `rm -rf ~⏎` into a shell *is* running it);
- a **wait**, a **kill**, and a lone **`<C-c>`** never ask: they only observe
  or end what was already approved.

Lifecycle hooks see `bash_session` calls like any other tool; a matcher may
name it `bash_session` or `BashSession`, the name its cell shows.

## The cells

- A `tty` `bash` call is an ordinary `● Bash(cmd)` cell. When it returns
  running, the report's frame line — the model's — is stripped, the program's
  output shows, and the cell closes on a fresh dim corner: `⎿ Waiting for
  input · session b7x2k9m1q` (or `Waiting for a password · …` at a prompt
  that reads with echo off, `Still running · …`, or `Stopped · …` after a
  kill). A command that exited reads exactly like a plain `bash` cell.
- A `bash_session` call is `● BashSession(python3 ← import math⏎)` — the
  command the session runs (one line, cut at 60 characters), then the input
  on one line (`⏎` for Enter, `<Down>` for keys), `· kill` after a kill,
  nothing more for a wait — over the same output peek and state row. The
  command is named from the moment the call is announced: the loop asks the
  registry what the session runs (`summarize_call_naming`), the lookup the
  permission prompt makes for its `into {command} · session {id}` row, so
  the header over that prompt names the program the keys go to, and a call
  the user refused — which never reaches the executor — keeps it too. An id
  shown to a person says nothing. The executor sends the same header as the
  call's refined title (`ToolProgress::Title` → `StreamEvent::ToolTitle`);
  only a session nothing answers to is named by its id (`bnope ← y⏎`). What
  the model was additionally told (a note about an unsubmitted line or a kill
  not carried out) is in its context only; Ctrl+D shows it.
- Both are command cells (`COMMAND_TOOL_NAMES`): the peek folds at
  `TOOL_FOLD_ROWS`, Ctrl+O shows everything, and a running call tails the
  transcript live under its clock row — a `bash_session` call's clock naming
  its own wait (`timeout 10s`), not `bash`'s two minutes.
- **Ctrl+B** on a running `tty` launch hands the session to the background
  (it keeps running, the model is told its id); on a `bash_session` call it
  ends the *wait* — the session was already in the background — and the model
  is told the user moved on.
- The ↓ manager's details page shows a TTY shell's current screen.

## Offline demo and tests

The dummy's `interactive` scenario (cue: `interactive` — the whole word,
since `tty` hides in "pretty" and `repl` in "reply") plays a setup wizard: a
`tty` launch stopping at `Project name:`, an answer met by the next question,
and the last answer ending the program — one call a round, every result the
real `pty::report::report`, so the offline cells are the live ones.
`scripts/smoke.sh` Phase 123 drives it in the real binary and pins what only
the screen shows: the dim state rows, the headers, and that no frame line or
`Exit code: 0` ever reaches a cell.

The pure cores — key notation, the transcript, the screen, the settle policy,
the frames — are unit-tested; the pty spawn, the registry's TTY tasks and the
executor are tested against real processes (`sh`, `stty`, `python3` when
present); `tests/detached_exec.rs` proves the helper tier gives the session its
own controlling terminal. `examples/session_probe.rs` is the live harness.

## Compared with the terminal-sessions design

This design was compared against another built for the same job — a
codex-style `write_stdin` over numbered sessions, the command itself run in a
terminal by default (branch `update/optimistic-planck-fiq8pr`). Three of its
ideas were better and are ported: streaming the running cell as settled text
plus live rows (*Streaming the running cell*), a header naming the session's
command (*The cells*), and the `/proc` probe (*The kernel's word*). Plain
commands folding their `\r` frames goes one step further than it did: it ran
every `bash` call in a terminal to get that, where here a plain call keeps its
pipe — no terminal emulation, no changed `isatty` answer, output that is
data — and folds it instead. One idea was tried and left out: masking what is
typed at a prompt that reads with echo off. In use it masked ordinary input
that was no password, hiding exactly what the header and the permission
prompt are there to show (a header has no terminal to ask until the executor
runs, so it masked by default), so typed keys show as typed. What it detected
is kept for what it can say without hiding anything: a wait ends on a
password prompt, and the frame names one (*A password prompt says so
itself*).

Three of this design's choices were kept over it:

- **Changed lines, not a prefix.** It compared each report with the last by
  their common prefix and resent everything after the first difference; a
  bar redrawn above finished lines (pacman) resent those lines too. Here each
  line remembers what the model was handed, so only new or changed lines
  go out.
- **A real terminal.** It gave programs `TERM=dumb`, which keeps them simple
  but breaks what a terminal is for — `vim`, `less`, `top`, arrow-key menus.
  Here the program gets `xterm-256color` and a real screen (`vt100`),
  query replies and all.
- **`tty` is asked for.** A plain call stays on its pipe, so `git`, `ls` and
  every tool that changes its output for a terminal behave as in a script,
  and the model reaches for a terminal only for a program that needs one.

## Limits

- **Unix only** — pseudo-terminals are a Unix facility; elsewhere `tty` is
  refused with an error the model can act on.
- **The prompt heuristic, where the probe is blind** — off Linux, for a
  process run as another user (`sudo` and what it runs), and behind a
  `poll`-family wait, the screen decides alone, a password prompt aside
  (the terminal's mode names that one). There a program that prints
  its question, a newline, and then waits in canonical mode looks like one
  between lines of output: it settles on `LINE_QUIET` and reports `Running`
  rather than `waiting for input` (the output still shows the question); and
  a busy command that leaves a line open (`Reading package lists... `) reads
  as a prompt to a launch or an input call after 0.5 s of quiet, and to a
  wait after 3 s.
- **A reader that is not waiting** — a program whose key-listening thread
  sits in `read` while another thread works reads as waiting to the probe.
- **A command that runs on silently after its password** holds the call that
  submitted it for `CHECK_QUIET` (10 s) rather than `LINE_QUIET` (2 s) before
  it returns `Running`.
- **A password prompt the terminal cannot show** — one that reads key by key
  to echo `*` for each (`sudo`'s `pwfeedback`) reads as `waiting for input`;
  one asked behind a relay (`sudo` inside `ssh`, a `docker run -it` shell)
  sits on the relay's far terminal and is judged by the screen; one that
  prints nothing before reading is a plain read to the probe; and a program
  run as another user that turns echo off to swallow type-ahead, then goes
  quiet, reads as asking for a password — the probe that would veto it is
  blind there.
- **A plain command's fold is one line at a time** — cursor movement between
  lines (`docker pull`'s multi-line progress drawn with cursor-up) means
  nothing on a pipe and is dropped; such programs print plain lines when
  their output is not a terminal, and one that does not can be run with
  `tty`.
- **A question drawn over an animation** — a prompt that replaces a spinner on
  the spinner's own line inherits its redraw count and reads as animated: a
  launch or input call returns on `LINE_QUIET` reporting `Running`, and a
  wait rides it out to its timeout. One printed on a fresh line is a prompt at
  once.
- **Behind a relay** — under `sudo`, `ssh` or `docker run -it` the terminal's
  raw mode says nothing, so a program there waiting for a key with its cursor
  at the start of a line is not recognised as waiting (one with its prompt
  text before the cursor is).
- **Typeahead** — several answers in one input are delivered at once, as a
  terminal would; a program that flushes pending input before a prompt loses
  them. The description steers models to one answer per call instead of
  pacing input line by line.
