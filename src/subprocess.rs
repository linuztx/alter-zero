//! Shared subprocess spawning for every shell command the app runs — the
//! model's `bash` tool ([`crate::llm::exec`]), its `run_in_background`
//! launches ([`crate::background`]), and the user's `!` shell (`main.rs`) —
//! **detached from the TUI's controlling terminal** (docs/tools.md,
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
//! (Claude Code behaves the same way). `setsid()` must run **in the child,
//! before the command**; the classic hook for that is `pre_exec`, but it is
//! `unsafe` (forbidden crate-wide). So [`spawn_detached_shell`] works through
//! a chain of [`DetachTier`]s, most-preferred first ([`tiers`]):
//!
//! 1. **`SetsidBinary`** — `setsid sh -c {command}` via util-linux's `setsid`
//!    binary. Simple, robust (independent of our own binary being replaced by
//!    a rebuild mid-session), and the everyday tier on Linux. The spawner must
//!    NOT also claim a process group: a group *leader* can't `setsid(2)`,
//!    which would force `setsid` to fork (a pid we don't track) instead of
//!    exec'ing `sh` in place.
//! 2. **`HelperReexec`** — a re-exec of **our own binary** in a helper mode:
//!    `{current_exe} {DETACH_ARG} {command}`. `main()` calls
//!    [`run_detached_exec_if_requested`] as its very first statement; in
//!    helper mode that calls the safe [`rustix::process::setsid`] wrapper and
//!    `exec`s `sh -c {command}` **in place** (same pid), never returning. This
//!    is what keeps macOS / minimal images — no `setsid` binary there — from
//!    degrading back to the attached bug. Opt-in per process (`detach_helper`
//!    is threaded from `main.rs`, resolved once at startup via
//!    `current_exe()`): a `cargo test` binary has libtest's `main`, not ours —
//!    re-execing it would run the test suite instead of the command.
//! 3. **`Attached`** — the pre-fix plain `sh -c` in its own process group
//!    (`process_group(0)`), terminal-attached. The last resort so a command
//!    still runs where neither detach route exists.
//!
//! Only a `NotFound` spawn error falls through to the next tier (no `setsid`
//! binary; a helper path whose file was deleted); any other error is real and
//! surfaces. Every tier preserves the invariant the group-kill helpers and
//! the Ctrl+B adopt rely on: **pgid == `child.id()`** — via `setsid` making
//! the child a session *and group* leader in tiers 1–2, or `process_group(0)`
//! in tier 3 — and every child gets the same stdio: stdin `/dev/null` (a
//! stdin read gets EOF, never blocks), stdout/stderr piped for the caller to
//! drain.
//!
//! Boundary code like `term.rs`: the process I/O is exercised by the
//! real-`sh` tests below (the `llm::exec` pattern), `tests/detached_exec.rs`
//! (the real built binary via `CARGO_BIN_EXE`), and `scripts/smoke.sh`; the
//! pure decisions ([`tiers`], [`command_for`]'s argv) are unit-tested.

use std::io;
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// The sentinel first argument that selects the detached-exec helper mode —
/// obscure enough that no real TUI invocation collides with it.
pub const DETACH_ARG: &str = "__inline-tui-detached-exec";

/// One way to spawn the shell child, in [`tiers`]' preference order. See the
/// module docs for the full rationale per tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetachTier<'a> {
    /// `setsid sh -c {command}` — util-linux's binary; no controlling
    /// terminal, pgid == pid via its `setsid(2)` + exec.
    SetsidBinary,
    /// `{helper} {DETACH_ARG} {command}` — our own binary's helper mode
    /// ([`run_detached_exec_if_requested`]); the macOS-safe route.
    HelperReexec(&'a Path),
    /// Plain `sh -c {command}` + `process_group(0)` — terminal-attached, the
    /// pre-fix behavior and the last resort.
    Attached,
}

/// The spawn order for this process: `SetsidBinary`, then `HelperReexec` when
/// a helper is installed, then `Attached`. Never empty. Non-Unix has no
/// controlling-terminal concept to escape (and no `setsid`), so it is
/// `Attached` only.
#[must_use]
pub fn tiers(detach_helper: Option<&Path>) -> Vec<DetachTier<'_>> {
    #[cfg(unix)]
    {
        let mut order = vec![DetachTier::SetsidBinary];
        if let Some(helper) = detach_helper {
            order.push(DetachTier::HelperReexec(helper));
        }
        order.push(DetachTier::Attached);
        order
    }
    #[cfg(not(unix))]
    {
        let _ = detach_helper;
        vec![DetachTier::Attached]
    }
}

/// Build the `Command` for `command` under `tier`, with the shared stdio
/// (stdin `/dev/null`, stdout/stderr piped) applied. Pure argv/config — the
/// caller (or [`spawn_detached_shell`]) spawns it.
#[must_use]
pub fn command_for(tier: &DetachTier<'_>, command: &str) -> Command {
    let mut cmd = match tier {
        DetachTier::SetsidBinary => {
            // Deliberately NO process_group: a group leader can't setsid(2),
            // which would force the binary to fork instead of exec — breaking
            // pgid == child.id() (see the module docs).
            let mut c = Command::new("setsid");
            c.arg("sh").arg("-c").arg(command);
            c
        }
        DetachTier::HelperReexec(helper) => {
            // Same reasoning: the helper setsids itself, so it must not be
            // spawned as a group leader.
            let mut c = Command::new(helper);
            c.arg(DETACH_ARG).arg(command);
            c
        }
        DetachTier::Attached => {
            let mut c = Command::new("sh");
            c.arg("-c").arg(command);
            // Its own process group (pgid == pid) so the kill helpers can
            // reap the whole tree — the detached tiers get this from setsid.
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                c.process_group(0);
            }
            c
        }
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Spawn `command` under `sh -c`, detached from the controlling terminal where
/// possible — the [`tiers`] chain, falling through **only** on `NotFound` (a
/// missing `setsid` binary; a `detach_helper` whose file was replaced or
/// deleted mid-session). Any other spawn error surfaces immediately — masking
/// it behind a degraded retry would hide the real failure.
///
/// In every tier the returned child's `id()` is its process-group id, so
/// callers kill the whole tree with `kill -KILL -{child.id()}`; its stdin is
/// `/dev/null` and its stdout/stderr are pipes for the caller to drain.
///
/// # Errors
/// The underlying spawn error when even the last tier can't start.
pub fn spawn_detached_shell(detach_helper: Option<&Path>, command: &str) -> io::Result<Child> {
    let order = tiers(detach_helper);
    let (last, rest) = order.split_last().expect("tiers is never empty");
    for tier in rest {
        match command_for(tier, command).spawn() {
            Ok(child) => return Ok(child),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {} // tier absent — fall through
            Err(err) => return Err(err),
        }
    }
    command_for(last, command).spawn()
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
    use std::io::Read;

    fn argv(cmd: &Command) -> (String, Vec<OsString>) {
        (
            cmd.get_program().to_string_lossy().into_owned(),
            cmd.get_args().map(OsStr::to_os_string).collect(),
        )
    }

    #[cfg(unix)]
    #[test]
    fn tiers_prefer_setsid_then_the_helper_then_attached() {
        assert_eq!(
            tiers(None),
            vec![DetachTier::SetsidBinary, DetachTier::Attached]
        );
        let helper = Path::new("/opt/bin/inline-tui");
        assert_eq!(
            tiers(Some(helper)),
            vec![
                DetachTier::SetsidBinary,
                DetachTier::HelperReexec(helper),
                DetachTier::Attached,
            ]
        );
    }

    #[test]
    fn the_setsid_tier_wraps_sh_dash_c() {
        let (prog, args) = argv(&command_for(&DetachTier::SetsidBinary, "echo hi"));
        assert_eq!(prog, "setsid");
        assert_eq!(args, ["sh", "-c", "echo hi"].map(OsString::from));
    }

    #[test]
    fn the_helper_tier_is_the_reexec_protocol() {
        // The spawner side of the argv protocol the main() hook parses:
        // `{helper} {DETACH_ARG} {command}` — one command string, verbatim.
        let helper = Path::new("/opt/bin/inline-tui");
        let (prog, args) = argv(&command_for(
            &DetachTier::HelperReexec(helper),
            "sudo -v 'a b'",
        ));
        assert_eq!(prog, "/opt/bin/inline-tui");
        assert_eq!(args, [DETACH_ARG, "sudo -v 'a b'"].map(OsString::from));
    }

    #[test]
    fn the_attached_tier_is_a_plain_sh_dash_c() {
        let (prog, args) = argv(&command_for(&DetachTier::Attached, "echo hi"));
        assert_eq!(prog, "sh");
        assert_eq!(args, ["-c", "echo hi"].map(OsString::from));
    }

    #[test]
    fn the_attached_tier_actually_runs_the_command() {
        // Not just argv shape: the last-resort tier must spawn and report the
        // exit (`exit 7` writes nothing, so a status wait can't deadlock on
        // the piped stdio).
        let status = command_for(&DetachTier::Attached, "exit 7")
            .status()
            .expect("spawns");
        assert_eq!(status.code(), Some(7));
    }

    /// The chain's real product: the command runs in its **own session** (a
    /// session leader has `/proc`'s session field == its pid) — true only
    /// when it spawned detached, the whole point of the fix. Reads the
    /// session field of `/proc/$$/stat` — the 4th token after the
    /// parenthesized comm (`… (comm) state ppid pgrp session …`; pgrp, the
    /// 3rd, equals `$$` even in the attached tier, so only the session field
    /// distinguishes detach) — and compares it to `$$`.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_detached_command_leads_its_own_session() {
        let mut child = spawn_detached_shell(
            None,
            r#"stat=$(cat /proc/$$/stat); rest=${stat##*) }; set -- $rest; [ "$4" = "$$" ] && echo LEADER || echo MEMBER"#,
        )
        .expect("spawns");
        let mut out = String::new();
        child
            .stdout
            .take()
            .expect("piped stdout")
            .read_to_string(&mut out)
            .expect("reads output");
        let _ = child.wait();
        assert_eq!(
            out.trim(),
            "LEADER",
            "a detached command must lead its own session (no controlling tty)"
        );
    }

    /// The command's stdout and **exit status** survive the `setsid`/`sh`
    /// layers. `setsid` (never spawned as a group leader) execs `sh` in place
    /// — a wrapper that instead forked and waited would swallow the code and
    /// break the `Exit code: N` framing the model reads.
    #[test]
    fn output_and_exit_status_propagate_through_the_chain() {
        let mut child = spawn_detached_shell(None, "echo hi; exit 7").expect("spawns");
        let mut out = String::new();
        child
            .stdout
            .take()
            .expect("piped stdout")
            .read_to_string(&mut out)
            .expect("reads output");
        let status = child.wait().expect("waits");
        assert_eq!(out.trim(), "hi");
        assert_eq!(status.code(), Some(7), "the exit survives the layers");
    }

    /// pgid == `child.id()` in the detached tiers too, so `kill -KILL -pgid`
    /// reaps a backgrounded grandchild — the invariant every caller's
    /// group-kill and the Ctrl+B adopt rely on. `sleep 30 & wait` leaves
    /// `sleep` holding the pipe; the group kill must end it promptly.
    #[cfg(unix)]
    #[test]
    fn the_group_kill_reaps_a_backgrounded_grandchild() {
        let mut child = spawn_detached_shell(None, "sleep 30 & wait").expect("spawns");
        let pgid = child.id();
        let start = std::time::Instant::now();
        let _ = Command::new("sh")
            .arg("-c")
            .arg(format!("kill -KILL -{pgid} 2>/dev/null"))
            .status();
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "the whole process group must die on kill -pgid"
        );
    }
}
