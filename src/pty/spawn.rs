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

/// The locale a session's programs are given when the environment names no
/// UTF-8 one ([`utf8_locale`]).
#[cfg(target_os = "macos")]
pub const UTF8_LOCALE: &str = "en_US.UTF-8";
/// See above.
#[cfg(not(target_os = "macos"))]
pub const UTF8_LOCALE: &str = "C.UTF-8";

/// Which locale variable a session sets, and to what, so its programs know
/// their terminal speaks UTF-8 — `None` when the environment (read through
/// `var`) already says so.
///
/// The session's terminal is the emulator, and it speaks UTF-8 whatever the
/// TUI's terminal does — the `TERM` rule. Without a UTF-8 locale `btop`
/// refuses to start ("No UTF-8 locale detected!") and ncurses draws boxes in
/// the line-drawing set; a container or a CI job that sets no `LANG` is the
/// common case. The effective setting is `LC_ALL`, else `LC_CTYPE`, else
/// `LANG`: with nothing set, `LANG` is given one; with `LANG` naming another
/// locale, only `LC_CTYPE` — the encoding, leaving its messages and sorting;
/// `LC_ALL` only when it is the one saying otherwise, since it outranks
/// every other.
pub fn utf8_locale(var: impl Fn(&str) -> Option<String>) -> Option<(&'static str, &'static str)> {
    let set = |name: &str| var(name).filter(|value| !value.is_empty());
    let effective = set("LC_ALL")
        .or_else(|| set("LC_CTYPE"))
        .or_else(|| set("LANG"));
    if effective.as_deref().is_some_and(names_utf8) {
        return None;
    }
    let name = if set("LC_ALL").is_some() {
        "LC_ALL"
    } else if set("LC_CTYPE").is_some() || set("LANG").is_some() {
        "LC_CTYPE"
    } else {
        "LANG"
    };
    Some((name, UTF8_LOCALE))
}

/// Does a locale name say UTF-8 (`en_US.UTF-8`, `C.utf8`)?
fn names_utf8(locale: &str) -> bool {
    let lower = locale.to_ascii_lowercase();
    lower.contains("utf-8") || lower.contains("utf8")
}

/// Apply the session environment to `command`: [`TERMINAL_ENV`] set,
/// [`FOREIGN_TERMINAL_VARS`] removed, and a UTF-8 locale given where the
/// environment names none ([`utf8_locale`]).
#[cfg(unix)]
pub fn apply_terminal_env(command: &mut Command) {
    for name in FOREIGN_TERMINAL_VARS {
        command.env_remove(name);
    }
    for (name, value) in TERMINAL_ENV {
        command.env(name, value);
    }
    if let Some((name, value)) = utf8_locale(|name| std::env::var(name).ok()) {
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

/// How the program at the other end of a terminal is reading it — the
/// termios flags a session report depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineMode {
    /// Canonical (`ICANON`): input is delivered a whole line at a time, so
    /// typed text reaches the program only when Enter submits it. Off for a
    /// menu, an editor or a readline prompt, which read key by key.
    pub canonical: bool,
    /// `ECHO`: what is typed shows on the screen — off at a password prompt.
    pub echo: bool,
    /// `OPOST`: the terminal processes output (turns `\n` into `\r\n`).
    pub processed_output: bool,
}

impl LineMode {
    /// Does a program read this terminal **key by key** at a prompt — a
    /// menu, a line editor, `read -n1`? Canonical mode off says so — unless
    /// output processing is off too: that is a **relay** holding the
    /// terminal raw (sudo running its command in a terminal of its own, ssh,
    /// `docker run -it`), passing through output already processed at the
    /// far end. Its raw mode lasts the whole command and says nothing of
    /// whether anything waits on a key; the program behind it is judged by
    /// its screen alone. A key-reading program leaves output processing on
    /// (readline, Node, `prompt_toolkit`, `stty -icanon`).
    #[must_use]
    pub fn reads_keys(self) -> bool {
        !self.canonical && self.processed_output
    }

    /// Is the terminal held raw with output processing off — a **relay's**
    /// mode (sudo running its command in a terminal of its own, ssh,
    /// `docker exec -it`, `script`)? A relay reads each key the moment it
    /// lands and hands it on, so the terminal's input queue says nothing of
    /// whether the program at the far end has read it
    /// (`crate::background`'s typing).
    #[must_use]
    pub fn relays(self) -> bool {
        !self.canonical && !self.processed_output
    }

    /// Is the program reading a whole line with echo off — a password
    /// prompt (sudo, ssh, `read -s`, getpass)? Readable off the terminal
    /// even when the program runs as another user, whose /proc files the
    /// probe cannot read (`docs/interactive-shell.md`). A key-reading
    /// program turns echo off too, to draw what it reads itself: not this.
    #[must_use]
    pub fn hides_input(self) -> bool {
        self.canonical && !self.echo
    }
}

/// The path of the terminal behind `master` (`/dev/pts/N`) — what a process
/// holding it shows in `/proc/PID/fd`, for the probe
/// ([`super::probe::probe`]). `None` when the system will not say.
#[cfg(unix)]
#[must_use]
pub fn terminal_path(master: &std::fs::File) -> Option<std::path::PathBuf> {
    let name = rustix::pty::ptsname(master, Vec::new()).ok()?;
    let name = name.into_string().ok()?;
    Some(std::path::PathBuf::from(name))
}

/// A terminal's **input queue** — what has been typed into it and not yet
/// read — asked of its slave side, opened for the question alone: a slave
/// kept open would keep the master from reading end-of-stream when the
/// session's processes let go of it. What paces typed keys at the rate the
/// program reads them (`crate::background`).
#[cfg(unix)]
#[derive(Debug)]
pub struct InputQueue(rustix::fd::OwnedFd);

#[cfg(unix)]
impl InputQueue {
    /// The input queue of the terminal behind `master` — `None` when the
    /// terminal cannot be opened (it is gone, or the system will not say).
    #[must_use]
    pub fn of(master: &std::fs::File) -> Option<Self> {
        use rustix::fs::{Mode, OFlags};
        let path = terminal_path(master)?;
        let flags = OFlags::RDONLY | OFlags::NOCTTY | OFlags::NONBLOCK | OFlags::CLOEXEC;
        rustix::fs::open(path, flags, Mode::empty()).ok().map(Self)
    }

    /// The bytes typed and not yet read — `None` when the terminal will not
    /// say. A program reading whole lines has an unfinished line counted as
    /// none: it cannot read it before its Enter.
    ///
    /// Linux hands a write on the master to the line discipline a moment
    /// later, from a worker thread, and the count alone would miss a key
    /// still on its way — nearly every time, right after the write. A poll
    /// that finds nothing to read waits for that hand-over first
    /// (`n_tty_poll`), so the count after it includes every key written
    /// before. What the poll answers is not needed, only that it ran.
    #[must_use]
    pub fn pending(&self) -> Option<u64> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let now = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let _ = poll(&mut [PollFd::new(&self.0, PollFlags::IN)], Some(&now));
        rustix::io::ioctl_fionread(&self.0).ok()
    }
}

/// The name of the terminal's **foreground program** — the program at the
/// bottom of its foreground process group ([`super::probe::group_program`]):
/// a shell at its prompt, the job it started, or the command a shell runs.
/// `None` where no process table says (off Linux), or the terminal is gone.
/// What decides how text reaches the program (`super::keys::Reader`).
#[cfg(unix)]
#[must_use]
pub fn foreground_program(master: &std::fs::File) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let group = rustix::termios::tcgetpgrp(master).ok()?;
        let group = u32::try_from(group.as_raw_nonzero().get()).ok()?;
        super::probe::group_program(group)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = master;
        None
    }
}

/// The terminal's [`LineMode`], read through its master — `None` when the
/// terminal is gone.
#[cfg(unix)]
#[must_use]
pub fn line_mode(master: &std::fs::File) -> Option<LineMode> {
    use rustix::termios::{LocalModes, OutputModes};
    // The master's termios *is* the slave's on Linux and the BSDs — the pair
    // shares one line discipline — so the session needs no slave of its own.
    let termios = rustix::termios::tcgetattr(master).ok()?;
    let local = termios.local_modes;
    Some(LineMode {
        canonical: local.contains(LocalModes::ICANON),
        echo: local.contains(LocalModes::ECHO),
        processed_output: termios.output_modes.contains(OutputModes::OPOST),
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
        // key by key (a menu, an editor), whether it echoes, and whether the
        // terminal still processes output — read off the master, the only
        // side of the terminal the session keeps.
        let mut session = spawn(
            None,
            "printf 'one? '; read x; stty -icanon -echo; printf 'two> '; read y; \
             stty raw; printf 'three'; sleep 30",
        )
        .expect("spawns");
        let rx = reader(&session.master);
        read_until(&rx, "one? ");
        assert_eq!(
            line_mode(&session.master),
            Some(LineMode {
                canonical: true,
                echo: true,
                processed_output: true,
            }),
            "a fresh terminal is cooked"
        );
        session.master.write_all(b"\r").expect("types");
        read_until(&rx, "two> ");
        assert_eq!(
            line_mode(&session.master),
            Some(LineMode {
                canonical: false,
                echo: false,
                processed_output: true,
            }),
            "a key-reading program leaves output processing alone"
        );
        session.master.write_all(b"\r").expect("types");
        read_until(&rx, "three");
        assert_eq!(
            line_mode(&session.master),
            Some(LineMode {
                canonical: false,
                echo: false,
                processed_output: false,
            }),
            "raw through and through, the way a relay holds it"
        );
        crate::subprocess::kill_process_group(&mut session.child);
    }

    #[test]
    fn a_terminal_held_raw_with_output_processing_off_is_a_relays() {
        let mode = |canonical, processed_output| LineMode {
            canonical,
            echo: false,
            processed_output,
        };
        assert!(mode(false, false).relays(), "sudo, ssh, docker, script");
        assert!(!mode(false, true).relays(), "readline, a menu, btop");
        assert!(!mode(true, true).relays(), "a line-reading prompt");
    }

    #[test]
    fn the_input_queue_counts_what_was_typed_and_not_yet_read() {
        use rustix::fs::{Mode, OFlags};
        let mut session = spawn(None, "stty raw -echo; echo ready; sleep 5").expect("spawns");
        let rx = reader(&session.master);
        assert!(read_until(&rx, "ready").contains("ready"));
        let queue = InputQueue::of(&session.master).expect("the terminal opens");
        assert_eq!(queue.pending(), Some(0));
        // The program's side of the terminal, read here as the program would.
        let path = terminal_path(&session.master).expect("named");
        let flags = OFlags::RDONLY | OFlags::NOCTTY | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let mut program =
            std::fs::File::from(rustix::fs::open(path, flags, Mode::empty()).expect("opens"));
        // Linux hands a write to the line discipline a moment later, from a
        // worker thread: a key still on its way is counted all the same, or
        // typing paced by the count runs two keys into one read under load.
        for _ in 0..50 {
            (&session.master).write_all(b"k").expect("types");
            assert_eq!(queue.pending(), Some(1), "typed, not yet read");
            let mut key = [0u8; 8];
            assert_eq!(program.read(&mut key).expect("reads"), 1);
            assert_eq!(queue.pending(), Some(0), "read");
        }
        crate::subprocess::kill_process_group(&mut session.child);
        let _ = session.child.wait();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_foreground_program_is_named_by_the_process_table() {
        // Who reads the terminal: the program at the bottom of its
        // foreground process group — a shell at its prompt, the job it
        // started, or what a shell running one command runs (`sh` here does
        // not exec a lone command, and stays the group's leader).
        let mut session = spawn(None, "echo ready; sleep 30").expect("spawns");
        let rx = reader(&session.master);
        read_until(&rx, "ready");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut named = foreground_program(&session.master);
        while named.as_deref() != Some("sleep") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            named = foreground_program(&session.master);
        }
        assert_eq!(named.as_deref(), Some("sleep"));
        crate::subprocess::kill_process_group(&mut session.child);
        let _ = session.child.wait();
    }

    #[test]
    fn only_a_program_that_keeps_output_processing_reads_keys() {
        let mode = |canonical, processed_output| LineMode {
            canonical,
            echo: false,
            processed_output,
        };
        assert!(
            mode(false, true).reads_keys(),
            "readline, a menu, `read -n1`"
        );
        assert!(
            !mode(false, false).reads_keys(),
            "sudo, ssh or docker relaying another terminal — raw through and through"
        );
        assert!(!mode(true, true).reads_keys(), "a line-reading prompt");
    }

    #[test]
    fn a_line_read_with_echo_off_hides_its_input() {
        // sudo, ssh, `read -s`, getpass: a password prompt, told by the
        // terminal itself — which a program running as another user cannot
        // hide the way it hides its /proc files (docs/interactive-shell.md).
        let mode = |canonical, echo| LineMode {
            canonical,
            echo,
            processed_output: true,
        };
        assert!(mode(true, false).hides_input());
        assert!(!mode(true, true).hides_input(), "an ordinary prompt");
        assert!(
            !mode(false, false).hides_input(),
            "an editor or a menu turns echo off to draw what it reads itself"
        );
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

    /// An environment of `pairs` for [`utf8_locale`].
    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    }

    #[test]
    fn a_session_with_no_utf8_locale_is_given_one() {
        // btop refuses to start without one ("No UTF-8 locale detected!"),
        // and ncurses falls back to line-drawing letters — yet the session's
        // terminal, the emulator, speaks UTF-8 whatever the TUI's does.
        assert_eq!(utf8_locale(env(&[])), Some(("LANG", UTF8_LOCALE)));
        assert_eq!(
            utf8_locale(env(&[("LANG", "")])),
            Some(("LANG", UTF8_LOCALE)),
            "empty is unset"
        );
        assert_eq!(
            utf8_locale(env(&[("LANG", "en_US")])),
            Some(("LC_CTYPE", UTF8_LOCALE)),
            "the encoding alone: LANG keeps its messages and its sorting"
        );
        assert_eq!(
            utf8_locale(env(&[("LC_ALL", "C"), ("LANG", "en_US.UTF-8")])),
            Some(("LC_ALL", UTF8_LOCALE)),
            "LC_ALL outranks the rest — the one variable that would take"
        );
    }

    #[test]
    fn a_utf8_locale_the_environment_names_is_left_alone() {
        for pairs in [
            &[("LANG", "en_US.UTF-8")][..],
            &[("LANG", "de_DE.utf8")],
            &[("LC_CTYPE", "C.UTF-8"), ("LANG", "C")],
            &[("LC_ALL", "fr_FR.UTF-8"), ("LANG", "C")],
        ] {
            let pairs: &'static [(&str, &str)] = pairs;
            assert_eq!(utf8_locale(env(pairs)), None, "{pairs:?}");
        }
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
