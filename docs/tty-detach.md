# Detaching subprocesses from the controlling terminal

Every shell command this app runs — the model's `bash` tool
([`llm::exec::run_bash`](../src/llm/exec.rs), `docs/tools.md`), the user's `!`
shell ([`main.rs::spawn_shell_command`](../src/main.rs),
`docs/shell-command.md`), and background tasks
([`background::BackgroundRegistry::launch`](../src/background.rs),
`docs/background.md`) — spawns through one shared boundary helper,
[`subprocess::spawn_detached_shell`](../src/subprocess.rs), which runs the
command in a **new session with no controlling terminal**. This document is
why, and how the crate's `forbid(unsafe)` shapes the mechanism.

## The bug: `sudo` (and friends) hijack the TUI and hang

Running `sudo pacman -S …` (or anything that needs a password) through the
`bash` tool wrote

```
[sudo] password for linuztx:
```

straight onto the real terminal — **garbling the live TUI** — and then hung
until the per-command timeout (30 s by default) fired. The tool "ran too long"
and the terminal was left corrupted.

The root cause is a Unix subtlety. We already spawn with
`stdin(Stdio::null())`, so a program that reads a password from **stdin** gets
EOF immediately. But `sudo` (and `ssh`, `git`'s credential prompt, `passwd`, …)
deliberately do **not** read the password from stdin — they open the process's
**controlling terminal**, `/dev/tty`, directly, precisely so a script can't
feed them a password on a pipe. Redirecting stdin does nothing to that.
Because our child inherits the TUI's controlling terminal — through the
process *session*, not through any file descriptor — `sudo` opens `/dev/tty`,
the very terminal the TUI is drawing on in raw mode, prints its prompt there,
and blocks reading a keystroke the TUI's input loop has already captured.
Deadlock until the timeout.

## The fix: a new session, so `/dev/tty` can't be opened

A process's `/dev/tty` is the controlling terminal **of its session**. A
process that leads a session with *no* controlling terminal has no `/dev/tty`,
so opening it fails immediately:

```
sh: cannot open /dev/tty: No such device or address
```

So we run each command in a fresh session. `sudo` then can't reach a terminal
and fails **fast** with a captured, model-readable error on stderr instead of
hanging and scribbling on the TUI:

```
sudo: a terminal is required to read the password; either use the -S option
to read from standard input or configure an askpass helper
```

Exit 1, in milliseconds, output captured like any other command — never
touching the real terminal. (The cell then leads with the display's
`Error: Exit code 1` reframe and word-wraps the message —
`docs/tool-streaming.md`.)

## Why not `pre_exec(setsid)` — and the tier chain that replaces it

Creating a new session in-process means calling `setsid(2)` in the child
between `fork` and `exec` — i.e. `CommandExt::pre_exec`, an `unsafe fn`. This
crate **`forbid`s `unsafe`** (`Cargo.toml` `[lints]`), and `forbid` can't be
locally overridden. So [`subprocess::tiers`](../src/subprocess.rs) tries, in
order, falling through **only** on a `NotFound` spawn error (any other error
is real and surfaces):

1. **`setsid sh -c "<command>"`** — util-linux's `setsid(1)` binary calls
   `setsid(2)` and `exec`s `sh` into the new session. Simple, the everyday
   tier on Linux, and independent of our own binary (a `cargo build` replacing
   the running TUI's file mid-session can't break it).
2. **The helper re-exec** — `{current_exe} __alter-zero-detached-exec
   "<command>"`: our own binary, whose `main()` runs
   `subprocess::run_detached_exec_if_requested()` as its **first statement**.
   In helper mode that calls the safe `rustix::process::setsid()` wrapper and
   `exec`s `sh -c "<command>"` **in place** (same pid), never returning. This
   is what keeps macOS and stripped-down images — no `setsid` binary there —
   from degrading back to the attached bug. It is opt-in per process: the
   helper path is resolved once at startup and threaded on the
   `BackgroundRegistry` (`with_detach_helper` → `detach_helper()`), which
   every runner already shares. A `cargo test` binary has libtest's `main`,
   not ours — re-execing it would run the test suite instead of the command —
   so test processes simply don't install a helper.
3. **Attached `sh -c` + `process_group(0)`** — the pre-fix behavior, the last
   resort where neither detach route exists (still fully killable, but
   sharing the controlling terminal).

Non-Unix (Windows) has no `/dev/tty` and no sessions to escape; it uses the
plain path only.

## The load-bearing invariant: `child.id() == pgid`

All three spawn sites kill the **whole process group** on timeout / Esc / a
registry stop — `kill -KILL -<pgid>` — and every site computes `pgid` as
`child.id()`, relying on the child leading its own group (`pgid == pid`). The
pre-fix code guaranteed that with `process_group(0)`. Both detach tiers
preserve it **for free**, but only if used correctly:

- `setsid(2)` makes the caller a new session **and** process-group leader
  (`pgid == pid`), then the wrapper `exec`s `sh` — same pid. So the `Child`
  Rust holds *is* the group leader: `child.id() == pgid`. ✓
- **The wrapper must not be a process-group leader when it starts**, or
  `setsid(2)` would `EPERM` — `setsid(1)` would then `fork` (a new pid we
  don't track) and the helper would run attached. A freshly
  `Command::spawn`ed child is never a group leader **unless we ask**, so
  [`subprocess::command_for`](../src/subprocess.rs) deliberately does **not**
  set `process_group(0)` on the detached tiers — only the attached tier
  claims its own group.

Because the invariant holds identically in every tier, the group-kill helpers
(`exec::kill_process_group`, `main::kill_shell_group`, `background::kill_group`)
and the Ctrl+B adopt (`docs/background.md`) are **unchanged**. Exit status
also propagates unchanged: both detach tiers `exec` (no fork), so
`child.wait()` yields the command's real status (`exit 7` → `Some(7)`), which
the `Exit code: N` framing the model reads depends on.

## What does *not* change

Normal commands (`ls`, `grep`, `cargo`, `git status`) never touch `/dev/tty`,
so they behave exactly as before — same output, same exit codes.
`isatty(0/1/2)` is unchanged too: stdin was already `/dev/null` and
stdout/stderr already pipes (never a tty), so commands that vary on "am I on
a terminal?" saw non-tty fds before and after. The new session only removes
`/dev/tty` access, nothing else. Stdio is centralized in the spawner
(`command_for`), so all three runners get the identical contract.

## Testing

- **Pure** — `subprocess::tiers` orders the chain (`SetsidBinary` →
  `HelperReexec` when installed → `Attached`); `command_for`'s argv per tier,
  including the helper's `{exe} __alter-zero-detached-exec {command}`
  protocol the `main()` hook parses.
- **Boundary (real `sh`)** — a spawned command reports itself a **session
  leader** (`/proc`'s session field == `$$`, which only holds detached — the
  pgrp field equals `$$` even attached, so only the session distinguishes);
  output + a non-zero exit propagate through the layers; the group kill still
  reaps a backgrounded grandchild. The executor/registry survive a **stale
  helper path** (the chain falls through). `tests/detached_exec.rs` drives
  the **real built binary** (`CARGO_BIN_EXE`): the executor end-to-end, and
  the helper re-exec tier directly — the macOS-representative path.
- **Smoke** — `scripts/smoke.sh` Phase 44 drives a `!`-shell command that
  writes to `/dev/tty` inside a real tmux pty and asserts it fails fast
  inside the cell (`No such device or address`) with nothing printed over
  the TUI.
- **Live** — `tests/live_openrouter.rs`'s
  `live_sudo_style_tty_prompt_fails_fast`: a real model's tty-prompting
  `bash` call resolves failed in milliseconds through the production chain.
