//! Shared subprocess spawning for every shell command the app runs — the
//! model's `bash` tool ([`crate::llm::exec`]), its `run_in_background`
//! launches ([`crate::background`]), and the user's `!` shell (`main.rs`) —
//! **detached from the TUI's controlling terminal** (`docs/tty-detach.md` —
//! the full rationale; summarized in docs/tools.md and
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
//! The same chain has a **TTY form** for `bash` with `tty`
//! (`docs/interactive-shell.md`): a command that must *have* a controlling
//! terminal — a pseudo-terminal of its own, never the user's. [`tty_tiers`]
//! drops the attached tier (whose `/dev/tty` would be the user's terminal,
//! this module's very bug), and [`tty_command_for`] asks each remaining tier
//! for the controlling terminal as well: `setsid -c`, or the helper under
//! [`DETACH_TTY_ARG`], which adds `TIOCSCTTY` on its stdin (the pty's slave)
//! after the `setsid()`.
//!
//! Which **shell** a command runs under is the caller's: the model's commands
//! run under [`tool_shell`] — `bash` where it is installed, since the tool is
//! called bash and models write bash (`docs/bash-tools.md`) — while the `!`
//! shell and the hooks runner keep [`DEFAULT_SHELL`], the `sh -c` their
//! commands were always written for. Every tier takes the shell as given
//! ([`command_in`], [`tty_command_for`]); the helper hears it as an optional
//! third argument.
//!
//! Boundary code like `term.rs`: the process I/O is exercised by the
//! real-`sh` tests below (the `llm::exec` pattern), `tests/detached_exec.rs`
//! (the real built binary via `CARGO_BIN_EXE`), and `scripts/smoke.sh`; the
//! pure decisions ([`tiers`], [`command_for`]'s argv, [`find_on_path`]) are
//! unit-tested.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The sentinel first argument that selects the detached-exec helper mode —
/// obscure enough that no real TUI invocation collides with it.
pub const DETACH_ARG: &str = "__alter-zero-detached-exec";

/// The shell a command runs under unless its caller names another: the `!`
/// shell's and the hooks runner's, whose commands were always `sh -c`.
pub const DEFAULT_SHELL: &str = "sh";

/// The first `program` on `path` — `{entry}/{program}` for an absolute entry
/// that `installed` says holds it. Relative and empty entries are skipped: to
/// a POSIX shell an empty entry is the current directory, and a program
/// planted in a project must never become what every command runs under.
#[must_use]
pub fn find_on_path(
    program: &str,
    path: Option<&OsStr>,
    installed: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| installed(candidate))
}

/// The shell the model's commands run under (`docs/bash-tools.md`): the first
/// `bash` on `PATH` — the tool is called bash, and models write bash, which
/// dash (`/bin/sh` on Debian, Ubuntu and Kali) refuses — else
/// [`DEFAULT_SHELL`]. Looked up once per process. Run non-login and
/// non-interactive (`bash -c`), so it reads no profile or rc file: the
/// environment the TUI was started with is the one every command gets.
#[must_use]
pub fn tool_shell() -> &'static Path {
    static SHELL: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    SHELL.get_or_init(|| {
        find_on_path(
            "bash",
            std::env::var_os("PATH").as_deref(),
            is_executable_file,
        )
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SHELL))
    })
}

/// Is `path` a file this process may execute?
fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

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

/// Build the `Command` for `command` under `tier` and [`DEFAULT_SHELL`], with
/// the shared stdio (stdin `/dev/null`, stdout/stderr piped) applied. Pure
/// argv/config — the caller (or [`spawn_detached_shell`]) spawns it.
#[must_use]
pub fn command_for(tier: &DetachTier<'_>, command: &str) -> Command {
    command_in(tier, Path::new(DEFAULT_SHELL), command)
}

/// [`command_for`] under `shell` — [`tool_shell`] for a model command.
#[must_use]
pub fn command_in(tier: &DetachTier<'_>, shell: &Path, command: &str) -> Command {
    let mut cmd = match tier {
        DetachTier::SetsidBinary => {
            // Deliberately NO process_group: a group leader can't setsid(2),
            // which would force the binary to fork instead of exec — breaking
            // pgid == child.id() (see the module docs).
            let mut c = Command::new("setsid");
            c.arg(shell).arg("-c").arg(command);
            c
        }
        DetachTier::HelperReexec(helper) => {
            // Same reasoning: the helper setsids itself, so it must not be
            // spawned as a group leader.
            let mut c = Command::new(helper);
            c.arg(DETACH_ARG).arg(command);
            push_shell_arg(&mut c, shell);
            c
        }
        DetachTier::Attached => {
            let mut c = Command::new(shell);
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

/// The sentinel first argument of the **TTY** helper mode — [`DETACH_ARG`]'s
/// twin for an interactive session (`docs/interactive-shell.md`): after the
/// `setsid`, stdin — the pseudo-terminal the spawner handed the child — is
/// made the new session's controlling terminal, so a `/dev/tty` prompt
/// reaches the session instead of failing.
pub const DETACH_TTY_ARG: &str = "__alter-zero-detached-tty-exec";

/// The spawn order for a **TTY session**: the `setsid` binary, then the
/// helper when one is installed — and nothing after them. [`tiers`]' last
/// resort, an attached `sh`, would hand a password prompt the user's own
/// terminal; a session that finds neither route is refused instead. Empty
/// off Unix, which has no pseudo-terminals to offer.
#[must_use]
pub fn tty_tiers(detach_helper: Option<&Path>) -> Vec<DetachTier<'_>> {
    tiers(detach_helper)
        .into_iter()
        .filter(|tier| *tier != DetachTier::Attached)
        .collect()
}

/// The `Command` that starts `command` under `tier` and `shell` as the leader
/// of a new session whose controlling terminal is its **stdin** — `setsid -c
/// {shell} -c {command}` (util-linux's and BusyBox's `-c`/`--ctty`), or the
/// helper's [`DETACH_TTY_ARG`] mode. Stdio is the caller's (the terminal's
/// slave side); `None` for the attached tier, which has no such form.
#[must_use]
pub fn tty_command_for(tier: &DetachTier<'_>, shell: &Path, command: &str) -> Option<Command> {
    match tier {
        DetachTier::SetsidBinary => {
            // No process_group, for the same reason as `command_for`: a
            // group leader cannot setsid(2).
            let mut c = Command::new("setsid");
            c.arg("-c").arg(shell).arg("-c").arg(command);
            Some(c)
        }
        DetachTier::HelperReexec(helper) => {
            let mut c = Command::new(helper);
            c.arg(DETACH_TTY_ARG).arg(command);
            push_shell_arg(&mut c, shell);
            Some(c)
        }
        DetachTier::Attached => None,
    }
}

/// The helper's optional third argument: the shell it becomes, when that is
/// not the [`DEFAULT_SHELL`] it becomes without one — so a helper invocation
/// for the `!` shell reads exactly as it always did.
fn push_shell_arg(cmd: &mut Command, shell: &Path) {
    if shell != Path::new(DEFAULT_SHELL) {
        cmd.arg(shell);
    }
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
    spawn_shell_with(detach_helper, command, |_| {})
}

/// [`spawn_detached_shell`] under `shell` — [`tool_shell`] for a model
/// command (`docs/bash-tools.md`).
///
/// # Errors
/// The underlying spawn error when even the last tier can't start.
pub fn spawn_detached_in(
    detach_helper: Option<&Path>,
    shell: &Path,
    command: &str,
) -> io::Result<Child> {
    spawn_tiers(detach_helper, shell, command, |_| {})
}

/// [`spawn_detached_shell`] with the caller adjusting each tier's `Command`
/// first — the same tier walk, the same fall-through rule, the same
/// `pgid == child.id()` invariant, but a stdio (or cwd, or environment) of the
/// caller's choosing.
///
/// This exists for the **lifecycle hooks** runner (`docs/hooks.md`), which
/// needs `stdin` *piped* rather than `/dev/null` — it writes the event payload
/// there — plus the session's cwd and the `*_PROJECT_DIR` variables. A hook is
/// a shell command like any other, so it must not be the one child that keeps
/// a controlling terminal and can hijack the TUI with a `sudo` prompt; going
/// through the tier walk is what prevents that.
///
/// `configure` runs on each tier's `Command` **after** the shared stdio is
/// applied, so it can override any of it, and before the spawn.
///
/// # Errors
/// The underlying spawn error when even the last tier can't start.
pub fn spawn_shell_with(
    detach_helper: Option<&Path>,
    command: &str,
    configure: impl Fn(&mut Command),
) -> io::Result<Child> {
    spawn_tiers(detach_helper, Path::new(DEFAULT_SHELL), command, configure)
}

/// The tier walk every detached spawn shares, under `shell`.
fn spawn_tiers(
    detach_helper: Option<&Path>,
    shell: &Path,
    command: &str,
    configure: impl Fn(&mut Command),
) -> io::Result<Child> {
    let order = tiers(detach_helper);
    let (last, rest) = order.split_last().expect("tiers is never empty");
    let build = |tier: &DetachTier<'_>| {
        let mut cmd = command_in(tier, shell, command);
        configure(&mut cmd);
        cmd
    };
    for tier in rest {
        match build(tier).spawn() {
            Ok(child) => return Ok(child),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {} // tier absent — fall through
            Err(err) => return Err(err),
        }
    }
    build(last).spawn()
}

/// Kill `child`'s entire process group and reap it. Because every child here
/// is spawned through [`spawn_detached_shell`] / [`spawn_shell_with`], it
/// leads its own group (pgid == pid — via `setsid` in the detached tiers or
/// `process_group(0)` in the attached one), so a **negative pid** targets the
/// whole tree — reaping any process the command forked or backgrounded, which
/// would otherwise keep the stdout/stderr pipe open and hang a reader-thread
/// join (defeating the timeout). Best-effort; errors are ignored.
///
/// This crate `forbid`s `unsafe`, so it can't call `libc::kill(-pid, …)`
/// directly; instead it uses the shell's POSIX `kill` builtin, which treats a
/// negative operand as a process group (`sh -c "kill -KILL -<pgid>"`). The
/// helper `sh` starts in its own group, so it never signals itself.
///
/// Shared by the `bash` tool's timeout/cancel path ([`crate::llm::exec`]) and
/// the hooks runner's ([`crate::llm::hooks`]) — the same invariant, so the
/// same reaper rather than two copies that can drift.
#[cfg(unix)]
pub fn kill_process_group(child: &mut Child) {
    let pgid = child.id();
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!("kill -KILL -{pgid} 2>/dev/null"))
        .status();
    let _ = child.kill(); // reap the direct child too (no-op if already gone)
    let _ = child.wait();
}

/// Does `child`'s process group still hold a process once `child` itself
/// has exited — one the command started and left running (`sleep 30 &`)?
/// The group is the command's alone ([`kill_process_group`]), so anything
/// still in it is what [`kill_process_group`] is about to stop. A probe with
/// signal 0, which sends nothing.
#[cfg(unix)]
#[must_use]
pub fn group_outlives(child: &Child) -> bool {
    i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|pgid| rustix::process::test_kill_process_group(pgid).is_ok())
}

/// Non-unix fallback: no process groups to outlive the command.
#[cfg(not(unix))]
#[must_use]
pub fn group_outlives(_child: &Child) -> bool {
    false
}

/// Non-unix fallback: no process groups — just kill and reap the direct child.
#[cfg(not(unix))]
pub fn kill_process_group(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// The `main()` hook for the helper mode: when the process was invoked as
/// `{exe} {DETACH_ARG} {command} [shell]`, detach from the terminal and
/// become `{shell} -c {command}` — `sh` when no shell is named — never
/// returning. In every other invocation this is a no-op. It MUST run before
/// any terminal or runtime setup — nothing may touch stdin/stdout first
/// (invariant 1's cursor query included).
pub fn run_detached_exec_if_requested() {
    let mut args = std::env::args_os().skip(1);
    let tty = match args.next() {
        Some(arg) if arg == DETACH_ARG => false,
        Some(arg) if arg == DETACH_TTY_ARG => true,
        _ => return,
    };
    let Some(command) = args.next() else {
        // A malformed helper invocation must never fall through and boot a
        // TUI into the caller's pipes — fail the spawn instead.
        eprintln!("alter-zero: {DETACH_ARG} requires a command");
        std::process::exit(2);
    };
    let shell = args.next().unwrap_or_else(|| OsString::from(DEFAULT_SHELL));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Sever the controlling terminal: a fresh session has none, so the
        // command's `/dev/tty` opens fail fast (ENXIO) instead of prompting
        // over the TUI. Best-effort — refusing to run the command over a
        // failed setsid would help nobody, and the spawner never makes this
        // process a group leader, so in practice it succeeds.
        let _ = rustix::process::setsid();
        // The TTY mode (docs/interactive-shell.md): the session gets a
        // terminal back — the pseudo-terminal the spawner put on stdin — so
        // `/dev/tty` reaches the session's own screen, never the TUI's. An
        // ioctl on fd 0 reads nothing from it. Best-effort like the setsid: a
        // program run without it still has a terminal for its stdio, only no
        // `/dev/tty`, whose opens then fail fast exactly as detached ones do.
        if tty {
            let _ = rustix::process::ioctl_tiocsctty(std::io::stdin());
        }
        // Become the shell in place (same pid): the parent's `child.id()` IS
        // the shell — and, post-setsid, the session + process-group leader
        // the group-kill helpers target.
        let err = Command::new(&shell).arg("-c").arg(&command).exec();
        // exec only returns on failure.
        eprintln!(
            "alter-zero: failed to exec {}: {err}",
            shell.to_string_lossy()
        );
        std::process::exit(127);
    }
    #[cfg(not(unix))]
    {
        // No controlling terminal to escape (and no TTY sessions off Unix);
        // run the command and mirror its exit so the waiting parent sees the
        // same status.
        let _ = tty;
        match Command::new(&shell).arg("-c").arg(&command).status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(err) => {
                eprintln!(
                    "alter-zero: failed to run {}: {err}",
                    shell.to_string_lossy()
                );
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
    use std::path::PathBuf;

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
        let helper = Path::new("/opt/bin/alter-zero");
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
        let helper = Path::new("/opt/bin/alter-zero");
        let (prog, args) = argv(&command_for(
            &DetachTier::HelperReexec(helper),
            "sudo -v 'a b'",
        ));
        assert_eq!(prog, "/opt/bin/alter-zero");
        assert_eq!(args, [DETACH_ARG, "sudo -v 'a b'"].map(OsString::from));
    }

    #[cfg(unix)]
    #[test]
    fn tty_tiers_have_no_attached_fallback() {
        // An attached shell would hand a password prompt the *user's*
        // terminal — the bug the detach chain exists for — so a TTY session
        // that finds no detach route is refused rather than attached
        // (docs/interactive-shell.md).
        assert_eq!(tty_tiers(None), vec![DetachTier::SetsidBinary]);
        let helper = Path::new("/opt/bin/alter-zero");
        assert_eq!(
            tty_tiers(Some(helper)),
            vec![DetachTier::SetsidBinary, DetachTier::HelperReexec(helper)]
        );
    }

    #[test]
    fn the_setsid_tty_tier_asks_for_a_controlling_terminal() {
        let command = tty_command_for(&DetachTier::SetsidBinary, Path::new("sh"), "python3 -q")
            .expect("a tier");
        let (prog, args) = argv(&command);
        assert_eq!(prog, "setsid");
        assert_eq!(args, ["-c", "sh", "-c", "python3 -q"].map(OsString::from));
    }

    #[test]
    fn the_helper_tty_tier_is_its_own_reexec_protocol() {
        // A second sentinel, so the helper knows to take stdin — the
        // pseudo-terminal — as its controlling terminal after the setsid.
        let helper = Path::new("/opt/bin/alter-zero");
        let command = tty_command_for(
            &DetachTier::HelperReexec(helper),
            Path::new("sh"),
            "vim notes.txt",
        )
        .expect("a tier");
        let (prog, args) = argv(&command);
        assert_eq!(prog, "/opt/bin/alter-zero");
        assert_eq!(args, [DETACH_TTY_ARG, "vim notes.txt"].map(OsString::from));
        assert_ne!(DETACH_TTY_ARG, DETACH_ARG);
    }

    #[test]
    fn there_is_no_attached_tty_tier() {
        assert!(tty_command_for(&DetachTier::Attached, Path::new("sh"), "sh").is_none());
    }

    #[test]
    fn the_tool_shell_is_the_first_bash_on_path() {
        let path = OsString::from("relative:/opt/none::/usr/local/bin:/usr/bin");
        let installed =
            |p: &Path| p == Path::new("/usr/local/bin/bash") || p == Path::new("/usr/bin/bash");
        assert_eq!(
            find_on_path("bash", Some(&path), installed),
            Some(PathBuf::from("/usr/local/bin/bash"))
        );
        // Only absolute entries are searched: an empty one means the current
        // directory to a POSIX shell, and a `bash` planted in a project must
        // never become what every command runs under.
        let planted = |p: &Path| p == Path::new("relative/bash") || p == Path::new("bash");
        assert_eq!(find_on_path("bash", Some(&path), planted), None);
        assert_eq!(find_on_path("bash", None, |_| true), None);
    }

    #[test]
    fn a_tier_runs_the_shell_it_is_handed() {
        let bash = Path::new("/usr/bin/bash");
        let (prog, args) = argv(&command_in(&DetachTier::SetsidBinary, bash, "echo hi"));
        assert_eq!(prog, "setsid");
        assert_eq!(args, ["/usr/bin/bash", "-c", "echo hi"].map(OsString::from));
        let (prog, args) = argv(&command_in(&DetachTier::Attached, bash, "echo hi"));
        assert_eq!(prog, "/usr/bin/bash");
        assert_eq!(args, ["-c", "echo hi"].map(OsString::from));
        // The helper is told which shell to become — a third argument, which
        // plain `sh` never needs, so the `!` shell's protocol is unchanged.
        let helper = Path::new("/opt/bin/alter-zero");
        let (_, args) = argv(&command_in(
            &DetachTier::HelperReexec(helper),
            bash,
            "echo hi",
        ));
        assert_eq!(
            args,
            [DETACH_ARG, "echo hi", "/usr/bin/bash"].map(OsString::from)
        );
        let (_, args) = argv(&command_in(
            &DetachTier::HelperReexec(helper),
            Path::new("sh"),
            "echo hi",
        ));
        assert_eq!(args, [DETACH_ARG, "echo hi"].map(OsString::from));
    }

    #[test]
    fn a_tty_tier_runs_the_shell_it_is_handed() {
        let bash = Path::new("/usr/bin/bash");
        let command =
            tty_command_for(&DetachTier::SetsidBinary, bash, "python3 -q").expect("a tier");
        let (prog, args) = argv(&command);
        assert_eq!(prog, "setsid");
        assert_eq!(
            args,
            ["-c", "/usr/bin/bash", "-c", "python3 -q"].map(OsString::from)
        );
        let helper = Path::new("/opt/bin/alter-zero");
        let command =
            tty_command_for(&DetachTier::HelperReexec(helper), bash, "vim").expect("a tier");
        let (_, args) = argv(&command);
        assert_eq!(
            args,
            [DETACH_TTY_ARG, "vim", "/usr/bin/bash"].map(OsString::from)
        );
    }

    #[test]
    fn bash_syntax_runs_under_the_tool_shell() {
        // What the tool's name promises (docs/bash-tools.md): `[[ ]]`, brace
        // expansion and `source` are bash, and dash — `/bin/sh` on Debian,
        // Ubuntu and Kali — refuses all three.
        if tool_shell() == Path::new(DEFAULT_SHELL) {
            return; // no bash on this machine: the fallback is the old shell
        }
        let mut child = spawn_detached_in(
            None,
            tool_shell(),
            "[[ 1 == 1 ]] && echo {1..3}; source /dev/null && echo sourced",
        )
        .expect("spawns");
        let mut out = String::new();
        child
            .stdout
            .take()
            .expect("piped")
            .read_to_string(&mut out)
            .expect("reads");
        let _ = child.wait();
        assert_eq!(out, "1 2 3\nsourced\n");
    }

    #[test]
    fn the_attached_tier_is_a_plain_sh_dash_c() {
        let (prog, args) = argv(&command_for(&DetachTier::Attached, "echo hi"));
        assert_eq!(prog, "sh");
        assert_eq!(args, ["-c", "echo hi"].map(OsString::from));
    }

    #[test]
    fn spawn_shell_with_hands_the_caller_a_stdin_pipe_to_write_on() {
        // The hooks runner's whole requirement: the shared stdio is `/dev/null`
        // on stdin, and `configure` must be able to replace it — the payload
        // goes there. Round-trip a line through `cat` to prove the pipe is
        // real and reaches the command.
        use std::io::Write;
        let mut child = spawn_shell_with(None, "cat", |cmd| {
            cmd.stdin(Stdio::piped());
        })
        .expect("spawns");
        child
            .stdin
            .take()
            .expect("configure asked for a stdin pipe")
            .write_all(b"{\"hook_event_name\":\"PreToolUse\"}")
            .expect("writes");
        let out = child.wait_with_output().expect("waits");
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "{\"hook_event_name\":\"PreToolUse\"}"
        );
    }

    #[test]
    fn spawn_shell_with_can_set_the_working_directory_and_environment() {
        let dir = std::env::temp_dir();
        let mut child = spawn_shell_with(None, "pwd; printf '%s' \"$HOOK_PROBE\"", |cmd| {
            cmd.current_dir(&dir).env("HOOK_PROBE", "seen");
        })
        .expect("spawns");
        let mut out = String::new();
        child
            .stdout
            .take()
            .expect("piped")
            .read_to_string(&mut out)
            .expect("reads");
        let _ = child.wait();
        assert!(
            out.contains("seen"),
            "the env var reached the child: {out:?}"
        );
    }

    #[test]
    fn spawn_detached_shell_still_gets_the_default_null_stdin() {
        // The other children must not have changed: a read on stdin sees EOF
        // rather than blocking.
        let mut child = spawn_detached_shell(None, "cat; echo done").expect("spawns");
        let mut out = String::new();
        child
            .stdout
            .take()
            .expect("piped")
            .read_to_string(&mut out)
            .expect("reads");
        let _ = child.wait();
        assert_eq!(out.trim(), "done");
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
