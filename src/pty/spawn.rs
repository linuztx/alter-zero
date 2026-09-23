//! Start a command in a **pseudo-terminal of its own**
//! (`docs/interactive-shell.md`) — the boundary half of an interactive
//! session: allocate the terminal, hand its slave side to the command as
//! stdin/stdout/stderr **and** controlling terminal, and give the caller the
//! master side to read the program's output from and write its input to.
//!
//! This crate forbids `unsafe`, so the terminal comes from `rustix`'s safe
//! wrappers (`openpt`/`grantpt`/`unlockpt`, then Linux's `TIOCGPTPEER` or the
//! portable `ptsname` path), and the controlling terminal from the same tier
//! chain the detach uses, in its TTY form ([`crate::subprocess::tty_tiers`]):
//! `setsid -c`, then the helper re-exec — never an attached shell, whose
//! `/dev/tty` would be the user's own terminal.
//!
//! Both tiers leave the invariant every kill relies on intact: the returned
//! child is the session leader, its process-group leader and — through `exec`
//! in place — the command's own shell, so `child.id()` names the group.
//!
//! Verified against real processes (the tests below, the `llm::exec`
//! pattern): a real terminal of the right size, a working `/dev/tty`, the
//! exit status through the layers.

use std::path::Path;
#[cfg(unix)]
use std::process::{Child, Command, Stdio};

use super::screen::{COLUMNS, ROWS};

/// What a session's environment says about its terminal — the one it
/// actually has — and the pagers turned into plain printing, so `git log`,
/// `man` or `systemctl status` print instead of waiting in a pager nobody
/// asked for. A program run *as* a pager (`less file`) is unaffected.
pub const TERMINAL_ENV: [(&str, &str); 5] = [
    ("TERM", "xterm-256color"),
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
    ("MANPAGER", "cat"),
    ("SYSTEMD_PAGER", "cat"),
];

/// Variables describing a **different** terminal — the one the TUI itself
/// runs in — which would steer a program toward features the session's
/// emulator does not have (tmux passthrough, kitty's protocols), or override
/// the session's size with a stale one.
pub const FOREIGN_TERMINAL_VARS: [&str; 14] = [
    "TMUX",
    "TMUX_PANE",
    "STY",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "KITTY_WINDOW_ID",
    "KITTY_PID",
    "KITTY_LISTEN_ON",
    "WEZTERM_PANE",
    "ITERM_SESSION_ID",
    "WT_SESSION",
    "VTE_VERSION",
    "COLUMNS",
    "LINES",
];

/// Apply the session environment to `command`: [`TERMINAL_ENV`] set,
/// [`FOREIGN_TERMINAL_VARS`] removed.
#[cfg(unix)]
pub fn apply_terminal_env(command: &mut Command) {
    for name in FOREIGN_TERMINAL_VARS {
        command.env_remove(name);
    }
    for (name, value) in TERMINAL_ENV {
        command.env(name, value);
    }
}

/// A command running in its own pseudo-terminal.
#[cfg(unix)]
#[derive(Debug)]
pub struct PtyProcess {
    /// The session leader — `id()` is also its process group's and session's.
    pub child: Child,
    /// The terminal's master side: the program's output is read from it, its
    /// input written to it. Reads end in `EIO` once the last process holding
    /// the terminal lets go — the end of the stream.
    pub master: std::fs::File,
}

/// Start `command` under `sh -c` in a new pseudo-terminal ([`ROWS`] ×
/// [`COLUMNS`]) as the leader of a session whose controlling terminal it is.
///
/// # Errors
/// The terminal could not be allocated, the spawn failed, or — `NotFound` —
/// no tier could start a session with a controlling terminal here (no
/// `setsid` binary and no helper).
#[cfg(unix)]
pub fn spawn(detach_helper: Option<&Path>, command: &str) -> std::io::Result<PtyProcess> {
    spawn_in(&crate::subprocess::tty_tiers(detach_helper), command)
}

/// [`spawn`] over an explicit tier list — the seam an integration test uses
/// to drive the helper tier alone, the way a machine without a `setsid`
/// binary (macOS) would.
///
/// # Errors
/// As [`spawn`].
#[cfg(unix)]
pub fn spawn_in(
    tiers: &[crate::subprocess::DetachTier<'_>],
    command: &str,
) -> std::io::Result<PtyProcess> {
    let (master, slave) = open_terminal()?;
    for tier in tiers {
        let Some(mut cmd) = crate::subprocess::tty_command_for(tier, command) else {
            continue;
        };
        cmd.stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave.try_clone()?));
        apply_terminal_env(&mut cmd);
        match cmd.spawn() {
            Ok(child) => {
                // Our copies of the slave go now — the `Command`'s three with
                // it — so the child holds the only ones, and the master reads
                // end-of-stream the moment its session lets go of them.
                drop(cmd);
                drop(slave);
                return Ok(PtyProcess {
                    child,
                    master: std::fs::File::from(master),
                });
            }
            // This tier is absent (no `setsid` binary; a helper path that no
            // longer exists) — try the next.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no way to give the command a terminal of its own here \
         (neither `setsid` nor the alter-zero helper is available)",
    ))
}

/// How the program at the other end of a terminal is reading it — the two
/// termios flags a session report depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineMode {
    /// Canonical (`ICANON`): input is delivered a whole line at a time, so
    /// typed text reaches the program only when Enter submits it. Off for a
    /// menu, an editor or a readline prompt, which read key by key.
    pub canonical: bool,
    /// `ECHO`: what is typed shows on the screen — off at a password prompt.
    pub echo: bool,
}

/// The terminal's [`LineMode`], read through its master — `None` when the
/// terminal is gone.
#[cfg(unix)]
#[must_use]
pub fn line_mode(master: &std::fs::File) -> Option<LineMode> {
    use rustix::termios::LocalModes;
    // The master's termios *is* the slave's on Linux and the BSDs — the pair
    // shares one line discipline — so the session needs no slave of its own.
    let local = rustix::termios::tcgetattr(master).ok()?.local_modes;
    Some(LineMode {
        canonical: local.contains(LocalModes::ICANON),
        echo: local.contains(LocalModes::ECHO),
    })
}

/// Allocate a pseudo-terminal: the master (close-on-exec, so no child
/// inherits it) and its slave, already sized to [`ROWS`] × [`COLUMNS`].
#[cfg(unix)]
fn open_terminal() -> std::io::Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    use rustix::pty::{OpenptFlags, grantpt, openpt, unlockpt};
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)?;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let master = {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
        rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC)?;
        master
    };
    grantpt(&master)?;
    unlockpt(&master)?;
    let slave = open_slave(&master)?;
    rustix::termios::tcsetwinsize(
        &slave,
        rustix::termios::Winsize {
            ws_row: ROWS,
            ws_col: COLUMNS,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )?;
    Ok((master, slave))
}

/// The slave side of `master`: Linux opens it straight off the master
/// (`TIOCGPTPEER` — no path, so no race over who opens it); elsewhere, or on
/// a kernel older than 4.13, by the path `ptsname` names.
#[cfg(unix)]
fn open_slave(master: &std::os::fd::OwnedFd) -> std::io::Result<std::os::fd::OwnedFd> {
    #[cfg(target_os = "linux")]
    {
        use rustix::pty::OpenptFlags;
        if let Ok(slave) = rustix::pty::ioctl_tiocgptpeer(
            master,
            OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC,
        ) {
            return Ok(slave);
        }
    }
    use rustix::fs::{Mode, OFlags};
    let path = rustix::pty::ptsname(master, Vec::new())?;
    Ok(rustix::fs::open(
        path.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?)
}

/// Pseudo-terminals are a Unix facility.
///
/// # Errors
/// Always.
#[cfg(not(unix))]
pub fn spawn(detach_helper: Option<&Path>, command: &str) -> std::io::Result<()> {
    let _ = (detach_helper, command, ROWS, COLUMNS);
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "interactive sessions need a Unix pseudo-terminal",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Read the master on a thread, forwarding chunks.
    fn reader(master: &std::fs::File) -> mpsc::Receiver<Vec<u8>> {
        let mut master = master.try_clone().expect("clones");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = master.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        rx
    }

    /// Collect output until `until` shows up in it or the stream ends.
    fn read_until(rx: &mpsc::Receiver<Vec<u8>>, until: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut out = Vec::new();
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(chunk) => out.extend_from_slice(&chunk),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if String::from_utf8_lossy(&out).contains(until) {
                break;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn a_session_gets_a_real_terminal_of_the_session_size() {
        let mut session = spawn(None, "tty; stty size; echo END").expect("spawns");
        let rx = reader(&session.master);
        let out = read_until(&rx, "END");
        let _ = session.child.wait();
        assert!(!out.contains("not a tty"), "stdin is a terminal: {out:?}");
        assert!(out.contains("/dev/"), "tty names a device: {out:?}");
        assert!(
            out.contains(&format!("{ROWS} {COLUMNS}")),
            "the session's size: {out:?}"
        );
    }

    #[test]
    fn the_terminal_is_the_sessions_controlling_terminal() {
        // `/dev/tty` is what a password prompt opens — it must reach this
        // session's terminal (so the model can answer it), never fail and
        // never reach the TUI's.
        let mut session = spawn(
            None,
            "printf 'pw: ' > /dev/tty; read p < /dev/tty; echo got-$p",
        )
        .expect("spawns");
        let rx = reader(&session.master);
        let prompt = read_until(&rx, "pw: ");
        assert!(prompt.contains("pw: "), "{prompt:?}");
        session.master.write_all(b"secret\r").expect("types");
        let out = read_until(&rx, "got-secret");
        let _ = session.child.wait();
        assert!(out.contains("got-secret"), "{out:?}");
    }

    #[test]
    fn the_line_mode_is_read_through_the_master() {
        // Whether the program reads whole lines (a `read`, an `input()`) or
        // key by key (a menu, an editor), and whether it echoes — read off
        // the master, the only side of the terminal the session keeps.
        let mut session = spawn(
            None,
            "printf 'one? '; read x; stty -icanon -echo; printf 'two> '; read y",
        )
        .expect("spawns");
        let rx = reader(&session.master);
        read_until(&rx, "one? ");
        assert_eq!(
            line_mode(&session.master),
            Some(LineMode {
                canonical: true,
                echo: true
            }),
            "a fresh terminal is cooked"
        );
        session.master.write_all(b"\r").expect("types");
        read_until(&rx, "two> ");
        assert_eq!(
            line_mode(&session.master),
            Some(LineMode {
                canonical: false,
                echo: false
            })
        );
        crate::subprocess::kill_process_group(&mut session.child);
    }

    #[test]
    fn the_session_leads_its_own_group_and_session() {
        let mut session = spawn(None, "ps -o pid=,pgid=,sid= -p $$; echo END").expect("spawns");
        let rx = reader(&session.master);
        let out = read_until(&rx, "END");
        let _ = session.child.wait();
        let ids: Vec<&str> = out
            .lines()
            .next()
            .expect("a ps row")
            .split_whitespace()
            .collect();
        let pid = session.child.id().to_string();
        assert_eq!(ids, vec![pid.as_str(); 3], "pid == pgid == sid: {out:?}");
    }

    #[test]
    fn the_exit_status_survives_the_layers() {
        let mut session = spawn(None, "exit 7").expect("spawns");
        let _rx = reader(&session.master);
        let status = session.child.wait().expect("waits");
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn a_stale_helper_path_falls_back_to_setsid() {
        let stale = std::path::PathBuf::from("/definitely/not/a/real/helper");
        let mut session = spawn(Some(&stale), "echo ok").expect("spawns");
        let rx = reader(&session.master);
        assert!(read_until(&rx, "ok").contains("ok"));
        let _ = session.child.wait();
    }

    #[test]
    fn the_session_environment_names_its_own_terminal() {
        let mut command = Command::new("sh");
        command.env("TMUX", "/tmp/tmux-0/default,1,0");
        command.env("COLUMNS", "80");
        apply_terminal_env(&mut command);
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        let value = |name: &str| envs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        assert_eq!(value("TERM"), Some(Some("xterm-256color".to_string())));
        assert_eq!(value("PAGER"), Some(Some("cat".to_string())));
        assert_eq!(value("GIT_PAGER"), Some(Some("cat".to_string())));
        assert_eq!(value("TMUX"), Some(None), "removed, not inherited");
        assert_eq!(value("COLUMNS"), Some(None), "a stale size is removed");
    }

    #[test]
    fn the_spawned_command_sees_the_session_environment() {
        let mut session = spawn(None, "echo \"[$TERM] [$PAGER]\"; echo END").expect("spawns");
        let rx = reader(&session.master);
        let out = read_until(&rx, "END");
        let _ = session.child.wait();
        assert!(out.contains("[xterm-256color] [cat]"), "{out:?}");
    }
}
