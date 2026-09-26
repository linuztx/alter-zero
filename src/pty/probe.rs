//! What are a session's processes blocked in? — the kernel's own answer to
//! *is the program waiting for input* (`docs/interactive-shell.md`), where
//! the screen can only offer a shape.
//!
//! Linux only. `/proc/PID/task/TID/syscall` names the system call a sleeping
//! thread is blocked in — its number and first argument — and for a read
//! that argument is the file descriptor, which `/proc/PID/fd/N` resolves to a
//! path. A thread blocked in `read` on the session's terminal is waiting for
//! input ([`Probe::Reading`]): exact, and the only tell for a program that
//! asks with no prompt at all — a bare `read x`, a `cat`. A thread in
//! `poll`/`select`/`epoll` may be waiting on the terminal or on anything
//! else — a REPL, `vim`, `ssh` and every network client wait that way — so
//! it leaves the answer open. Every other state (running, sleeping, reading
//! a pipe, waiting on a child) is work, and when every thread of every
//! process is at work the program is busy however much its screen looks
//! like a prompt ([`Probe::Idle`]): `Compiling foo... ` left open while the
//! compiler runs, `Working... ` before a `sleep`.
//!
//! A tree with a thread in such a wait and no reader is [`Probe::Polling`]:
//! it may be at a prompt, or waiting on the network — which is why, since
//! every command runs in a terminal (`docs/bash-tools.md`), its silence ends
//! a launch only after a long quiet, where [`Probe::Idle`]'s never does.
//!
//! The files are readable only for the user's own processes, so a
//! `sudo`-elevated program leaves the probe blind — its prompts are judged
//! by the screen and the terminal's mode, as before
//! ([`Probe::Unknown`]).
//!
//! The parse and the verdict are pure; the walk over the session's
//! processes ([`probe`]) is boundary code the monitor runs while a call
//! waits on a quiet session (`crate::background`).

/// What the probe learned about a session's processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Probe {
    /// A thread is blocked reading the session's terminal: the program waits
    /// for input, wherever its cursor sits.
    Reading,
    /// Every thread was seen and none is reading the terminal, or waiting on
    /// anything that could be it: the program is busy.
    Idle,
    /// Every thread was seen, none reads the terminal, and one waits in a
    /// `poll`-family call — on the terminal (a REPL, `vim`), or on anything
    /// else (a network client, an idle server).
    Polling,
    /// Nothing certain — not probed since the session last changed, a
    /// process the probe may not inspect, or no `/proc` to ask.
    #[default]
    Unknown,
}

/// The read-family system calls on this architecture — `read`, `readv`,
/// `pread64` — by number. Empty where unknown, which only blinds the probe.
#[must_use]
pub fn read_syscalls() -> &'static [u64] {
    if cfg!(target_arch = "x86_64") {
        &[0, 17, 19]
    } else if cfg!(target_arch = "aarch64") {
        &[63, 65, 67]
    } else {
        &[]
    }
}

/// The wait-for-any-descriptor system calls on this architecture — `poll`,
/// `select`, `ppoll`, `pselect6`, `epoll_wait` and its `pwait` siblings — by
/// number: a thread in one may be waiting on the terminal.
#[must_use]
pub fn poll_syscalls() -> &'static [u64] {
    if cfg!(target_arch = "x86_64") {
        &[7, 23, 232, 270, 271, 281, 441]
    } else if cfg!(target_arch = "aarch64") {
        &[22, 72, 73, 441]
    } else {
        &[]
    }
}

/// The blocked call's number and its first argument, from a
/// `/proc/PID/syscall` line (`0 0x0 0x7ffd… …`). `None` for a thread that is
/// running (`running`), not in a call (`-1 …`), or anything unparseable.
#[must_use]
pub fn parse_syscall(line: &str) -> Option<(u64, u64)> {
    let mut fields = line.split_whitespace();
    let number: u64 = fields.next()?.parse().ok()?;
    let first = fields.next()?;
    let first = u64::from_str_radix(first.strip_prefix("0x")?, 16).ok()?;
    Some((number, first))
}

/// What one thread's `/proc/…/syscall` line says about the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thread {
    /// Blocked in a read on the session's terminal.
    Reading,
    /// Blocked in a `poll`-family wait — on the terminal, or on anything.
    Maybe,
    /// Anything else: running, sleeping, reading a pipe, waiting on a child.
    Busy,
}

/// Classify one thread's syscall `line`; `is_terminal` says whether a file
/// descriptor of its process is the session's terminal.
#[must_use]
pub fn classify(line: &str, is_terminal: impl Fn(u64) -> bool) -> Thread {
    match parse_syscall(line) {
        Some((number, fd)) if read_syscalls().contains(&number) && is_terminal(fd) => {
            Thread::Reading
        }
        Some((number, _)) if poll_syscalls().contains(&number) => Thread::Maybe,
        _ => Thread::Busy,
    }
}

/// The verdict over every thread inspected: a reader anywhere is
/// [`Probe::Reading`]; otherwise one the probe could not inspect at all
/// (`blind`) leaves it [`Probe::Unknown`] — as does having seen nothing — a
/// thread that may be waiting makes it [`Probe::Polling`], and only threads
/// that are all busy make it [`Probe::Idle`].
#[must_use]
pub fn combine(threads: &[Thread], blind: bool) -> Probe {
    if threads.contains(&Thread::Reading) {
        Probe::Reading
    } else if blind || threads.is_empty() {
        Probe::Unknown
    } else if threads.contains(&Thread::Maybe) {
        Probe::Polling
    } else {
        Probe::Idle
    }
}

/// Does the descriptor link `target` name the session's terminal — its own
/// path, or `/dev/tty`, which is the controlling terminal of every process
/// in the session (where `ssh` and `git` read a password from)?
#[must_use]
pub fn is_session_terminal(target: &std::path::Path, terminal: &std::path::Path) -> bool {
    target == terminal || target == std::path::Path::new("/dev/tty")
}

/// The most threads one probe inspects; a tree larger than that is left
/// [`Probe::Unknown`] rather than walked forever.
#[cfg(target_os = "linux")]
const PROBE_MAX_TASKS: usize = 1024;

/// What the processes of the tree rooted at `pid` are blocked in, with
/// respect to the terminal at `terminal` (its `/dev/pts/N` path).
#[cfg(target_os = "linux")]
#[must_use]
pub fn probe(pid: u32, terminal: &std::path::Path) -> Probe {
    if read_syscalls().is_empty() {
        return Probe::Unknown;
    }
    let mut threads = Vec::new();
    let mut blind = false;
    let mut todo = vec![pid];
    let mut seen = 0usize;
    while let Some(pid) = todo.pop() {
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            continue; // gone since it was listed
        };
        let is_terminal = |fd: u64| {
            std::fs::read_link(format!("/proc/{pid}/fd/{fd}"))
                .is_ok_and(|target| is_session_terminal(&target, terminal))
        };
        for task in tasks.flatten() {
            seen += 1;
            if seen > PROBE_MAX_TASKS {
                return Probe::Unknown;
            }
            match std::fs::read_to_string(task.path().join("syscall")) {
                Ok(line) => threads.push(classify(&line, is_terminal)),
                // Another user's process — `sudo` and what it runs.
                Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => blind = true,
                Err(_) => continue, // exited, or a zombie: waiting on nothing
            }
            // A kernel without the `children` file hides the rest of the
            // tree, and a busy parent says nothing of a child that reads.
            match std::fs::read_to_string(task.path().join("children")) {
                Ok(children) => todo.extend(
                    children
                        .split_whitespace()
                        .filter_map(|child| child.parse::<u32>().ok()),
                ),
                Err(_) => blind = true,
            }
        }
    }
    combine(&threads, blind)
}

/// Elsewhere there is no `/proc` to ask: the other tells carry the verdict.
#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn probe(_pid: u32, _terminal: &std::path::Path) -> Probe {
    Probe::Unknown
}

/// The process group a `/proc/PID/stat` line names — its fifth field,
/// counted after the parenthesised name, which may hold anything.
#[must_use]
pub fn stat_group(stat: &str) -> Option<u32> {
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(2)?.parse().ok()
}

/// The name (`/proc/PID/comm`) of the program at the bottom of process
/// group `group` — its leader's line of descent within the group: what a
/// terminal's foreground program is. A shell with job control gives each
/// job a group of its own, whose leader is the program; a shell running one
/// command (`sh -c 'vim x.py'`) keeps it in the shell's own group, below
/// the shell. `None` when the processes cannot be read.
#[cfg(target_os = "linux")]
#[must_use]
pub fn group_program(group: u32) -> Option<String> {
    let in_group = |pid: u32| {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| stat_group(&stat))
            == Some(group)
    };
    let mut pid = group;
    for _ in 0..PROBE_MAX_DEPTH {
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            break;
        };
        let child = tasks
            .flatten()
            .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
            .flat_map(|children| {
                children
                    .split_whitespace()
                    .filter_map(|child| child.parse::<u32>().ok())
                    .collect::<Vec<_>>()
            })
            .filter(|&child| in_group(child))
            .last();
        match child {
            Some(child) => pid = child,
            None => break,
        }
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim_end().to_string())
}

/// How far down a process group [`group_program`] follows it.
#[cfg(target_os = "linux")]
const PROBE_MAX_DEPTH: usize = 64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_group_is_read_off_its_stat_line() {
        assert_eq!(
            stat_group("4242 (python3) S 4200 4242 4200 34816 4242 4194560"),
            Some(4242)
        );
        // A name may hold spaces and parentheses: the fields follow the last.
        assert_eq!(stat_group("7 (a) b (c)) R 1 99 1 0 -1"), Some(99));
        assert_eq!(stat_group("garbage"), None);
    }

    #[test]
    fn a_blocked_read_names_its_call_and_descriptor() {
        assert_eq!(
            parse_syscall("0 0x0 0x7ffd3a0c6d37 0x1 0x0 0x0 0x0 0x7ffd3a0c6c78 0x7f2d"),
            Some((0, 0))
        );
        assert_eq!(
            parse_syscall("45 0x4 0x7f 0xa 0x0 0x0 0x0 0x7ff 0x7f"),
            Some((45, 4))
        );
    }

    #[test]
    fn a_thread_that_is_not_blocked_names_nothing() {
        assert_eq!(parse_syscall("running"), None);
        assert_eq!(parse_syscall("-1 0x7ffc 0x7f"), None);
        assert_eq!(parse_syscall(""), None);
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn a_read_is_reading_the_terminal_only_on_the_terminal() {
        let read = format!(
            "{} 0x0 0x7ffd 0x1 0x0 0x0 0x0 0x7ffd 0x7f2d",
            read_syscalls()[0]
        );
        assert_eq!(classify(&read, |fd| fd == 0), Thread::Reading);
        assert_eq!(classify(&read, |_| false), Thread::Busy, "a pipe or a file");
        let poll = format!("{} 0x7ffd 0x1 0xffffffff 0x0 0x0 0x0", poll_syscalls()[0]);
        assert_eq!(classify(&poll, |_| true), Thread::Maybe);
        assert_eq!(classify("running", |_| true), Thread::Busy);
    }

    #[test]
    fn one_reader_anywhere_is_reading() {
        assert_eq!(
            combine(&[Thread::Busy, Thread::Maybe, Thread::Reading], true),
            Probe::Reading
        );
    }

    #[test]
    fn only_a_whole_busy_tree_is_idle() {
        assert_eq!(combine(&[Thread::Busy, Thread::Busy], false), Probe::Idle);
        assert_eq!(
            combine(&[Thread::Busy, Thread::Maybe], false),
            Probe::Polling,
            "a poll may be on the terminal, or on a socket"
        );
        assert_eq!(
            combine(&[Thread::Busy], true),
            Probe::Unknown,
            "a process it could not see may be the one asking"
        );
        assert_eq!(combine(&[], false), Probe::Unknown, "it saw nothing");
    }

    #[test]
    fn the_controlling_terminal_is_the_session_terminal_too() {
        let pts = std::path::Path::new("/dev/pts/7");
        assert!(is_session_terminal(pts, pts));
        assert!(is_session_terminal(std::path::Path::new("/dev/tty"), pts));
        assert!(!is_session_terminal(
            std::path::Path::new("/dev/pts/8"),
            pts
        ));
        assert!(!is_session_terminal(
            std::path::Path::new("pipe:[123]"),
            pts
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_live_session_reading_its_terminal_is_seen_reading_and_a_sleeping_one_idle() {
        use crate::pty::spawn::{spawn, terminal_path};
        let probe_until = |command: &str, want: Probe| {
            let mut session = spawn(None, command).expect("spawns");
            let path = terminal_path(&session.master).expect("the terminal has a path");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut last = Probe::Unknown;
            while std::time::Instant::now() < deadline {
                last = probe(session.child.id(), &path);
                if last == want {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            crate::subprocess::kill_process_group(&mut session.child);
            last
        };
        assert_eq!(probe_until("read x", Probe::Reading), Probe::Reading);
        assert_eq!(
            probe_until("printf 'Working... '; sleep 30", Probe::Idle),
            Probe::Idle,
            "a prompt-shaped line over a sleeping command"
        );
        assert_eq!(
            probe_until("sh -c 'sleep 0.2; read x'; true", Probe::Reading),
            Probe::Reading,
            "a reader two processes down the tree"
        );
    }
}
