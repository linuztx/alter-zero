//! The shared shell spawn — every runner (the model's `bash` tool, its
//! `run_in_background` launches, and the `!` shell) builds its `sh -c` child
//! here, **detached from the TUI's controlling terminal** (docs/tools.md,
//! docs/shell-command.md).
//!
//! Why `stdin(Stdio::null())` is not enough: a password prompt (`sudo`, `ssh`,
//! git's credential helper) opens **`/dev/tty`** — the controlling terminal,
//! inherited through the process *session*, not through any fd — writes its
//! prompt straight onto the screen (over the live region) and then blocks
//! reading the same keyboard the event loop owns. The result is a `⎿ Running…`
//! cell that never resolves, a stray `[sudo] password for …:` glued to the
//! composer, and keystrokes raced byte-by-byte between the prompt and
//! crossterm's `EventStream`.
//!
//! The fix is to give the child **no controlling terminal at all**: `setsid()`
//! moves it into a fresh session, so opening `/dev/tty` fails (`ENXIO`) and a
//! prompting program errors out immediately — `sudo: a terminal is required to
//! read the password` — resolving the cell red with an actionable message
//! (Claude Code behaves the same way).
//!
//! `setsid()` must run **in the child, before the command**. The classic hook
//! for that is `pre_exec`, but it is `unsafe` (forbidden crate-wide), so the
//! child is instead spawned as a **re-exec of our own binary** in a helper
//! mode: `{current_exe} {DETACH_ARG} {command}`. `main()` calls
//! [`run_detached_exec_if_requested`] as its very first statement; in helper
//! mode that calls the safe [`rustix::process::setsid`] wrapper and `exec`s
//! `sh -c {command}` **in place** (same pid), never returning. Everything the
//! runners rely on is preserved: piped stdio is inherited across the re-exec,
//! and `setsid` makes the child a process-group leader (pgid == pid) exactly
//! like the `process_group(0)` it replaces, so the group-kill helpers and the
//! Ctrl+B adopt keep working off `child.id()` unchanged.
//!
//! The helper path is **opt-in per process** (`detach_helper` is threaded from
//! `main.rs`, resolved once at startup via `current_exe()`): a `cargo test`
//! binary has libtest's `main`, not ours — re-execing it would run the test
//! suite instead of the command — so tests (and any embedder that doesn't
//! install the hook) use the plain `sh -c` fallback in its own process group,
//! attached to the terminal as before. The argv protocol between
//! [`shell_command`] and the hook is locked by the unit tests below; the
//! actual detachment is boundary behavior, verified by `scripts/smoke.sh`.

use std::path::Path;
use std::process::Command;

/// The sentinel first argument that selects the detached-exec helper mode —
/// obscure enough that no real TUI invocation collides with it.
pub const DETACH_ARG: &str = "__inline-tui-detached-exec";

/// Build the `Command` that runs `command` under `sh -c`.
///
/// With a `detach_helper` (production: the TUI's own binary, whose `main`
/// installed [`run_detached_exec_if_requested`]) the child is the helper
/// re-exec — it `setsid()`s away from the terminal and becomes `sh -c
/// {command}` in place. Without one (unit tests; a failed `current_exe`) it is
/// a plain `sh -c` in its own process group — the same tree-kill contract,
/// still terminal-attached.
///
/// Either way the caller gets the group-kill invariant pgid == `child.id()`
/// (via the helper's `setsid` or the fallback's `process_group(0)`) and wires
/// up stdio itself.
#[must_use]
pub fn shell_command(detach_helper: Option<&Path>, command: &str) -> Command {
    #[cfg(unix)]
    if let Some(helper) = detach_helper {
        let mut cmd = Command::new(helper);
        cmd.arg(DETACH_ARG).arg(command);
        // Deliberately NOT process_group(0): a process-group *leader* cannot
        // `setsid()` — the helper makes itself leader of the fresh session
        // (and so of its own group) instead, restoring pgid == pid.
        return cmd;
    }
    #[cfg(not(unix))]
    let _ = detach_helper; // no controlling-terminal concept to escape
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    // Its own process group (pgid == pid) so the kill helpers can reap the
    // whole tree — the invariant every runner relies on (see
    // `llm::exec::kill_process_group`).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

/// The `main()` hook for the helper mode: when the process was invoked as
/// `{exe} {DETACH_ARG} {command}`, detach from the terminal and become
/// `sh -c {command}` — never returning. In every other invocation this is a
/// no-op. It MUST run before any terminal or runtime setup — nothing may
/// touch stdin/stdout first (invariant 1's cursor query included).
pub fn run_detached_exec_if_requested() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(DETACH_ARG)) {
        return;
    }
    let Some(command) = args.next() else {
        // A malformed helper invocation must never fall through and boot a
        // TUI into the caller's pipes — fail the spawn instead.
        eprintln!("inline-tui: {DETACH_ARG} requires a command");
        std::process::exit(2);
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Sever the controlling terminal: a fresh session has none, so the
        // command's `/dev/tty` opens fail fast (ENXIO) instead of prompting
        // over the TUI. Best-effort — refusing to run the command over a
        // failed setsid would help nobody, and the spawner never makes this
        // process a group leader, so in practice it succeeds.
        let _ = rustix::process::setsid();
        // Become `sh` in place (same pid): the parent's `child.id()` IS the
        // shell — and, post-setsid, the session + process-group leader the
        // group-kill helpers target.
        let err = Command::new("sh").arg("-c").arg(&command).exec();
        // exec only returns on failure.
        eprintln!("inline-tui: failed to exec sh: {err}");
        std::process::exit(127);
    }
    #[cfg(not(unix))]
    {
        // No controlling terminal to escape; run the command and mirror its
        // exit so the waiting parent sees the same status.
        match Command::new("sh").arg("-c").arg(&command).status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(err) => {
                eprintln!("inline-tui: failed to run sh: {err}");
                std::process::exit(127);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{OsStr, OsString};

    #[test]
    fn without_a_helper_its_a_plain_sh_dash_c() {
        let cmd = shell_command(None, "echo hi");
        assert_eq!(cmd.get_program(), "sh");
        let args: Vec<OsString> = cmd.get_args().map(OsStr::to_os_string).collect();
        assert_eq!(args, [OsString::from("-c"), OsString::from("echo hi")]);
    }

    #[test]
    fn the_fallback_actually_runs_the_command() {
        // Not just argv shape: the fallback must spawn and report the exit.
        let status = shell_command(None, "exit 7").status().expect("spawns");
        assert_eq!(status.code(), Some(7));
    }

    #[cfg(unix)]
    #[test]
    fn with_a_helper_the_child_is_the_reexec_protocol() {
        // The spawner side of the argv protocol the main() hook parses:
        // `{helper} {DETACH_ARG} {command}` — one command string, verbatim.
        let cmd = shell_command(Some(Path::new("/opt/bin/inline-tui")), "sudo -v 'a b'");
        assert_eq!(cmd.get_program(), "/opt/bin/inline-tui");
        let args: Vec<OsString> = cmd.get_args().map(OsStr::to_os_string).collect();
        assert_eq!(
            args,
            [OsString::from(DETACH_ARG), OsString::from("sudo -v 'a b'")]
        );
    }
}
