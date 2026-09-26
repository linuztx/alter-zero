//! What are a session's processes blocked in? — the kernel's own answer to
//! *is the program waiting for input* (`docs/interactive-shell.md`), where
//! the screen can only offer a shape.
//!
//! Linux only. `/proc/PID/task/TID/syscall` names the system call a sleeping
//! thread is blocked in — its number and its arguments — and for a read the
//! first argument is the file descriptor, which `/proc/PID/fd/N` resolves to
//! a path. A thread blocked in `read` on the session's terminal is waiting for
//! input ([`Probe::Reading`]): exact, and the only tell for a program that
//! asks with no prompt at all — a bare `read x`, a `cat`. A thread in
//! `poll`/`select`/`epoll` may be waiting on the terminal or on anything
//! else — a REPL, `vim`, `ssh` and every network client wait that way. For an
//! epoll wait the kernel says which: the instance's interest list
//! (`/proc/PID/fdinfo/N`) names every file it watches, by inode, so a wait
//! whose list holds no terminal is on something else ([`Probe::Elsewhere`]):
//! an event loop between network calls or timers, the way every Go, Node,
//! libuv and asyncio program idles — `gh run watch` between redraws. A `poll`
//! or `select` over no descriptors is a sleep; one over some leaves the answer
//! open, since its list lives in the program's own memory. Every other state
//! (running, sleeping, reading a pipe, waiting on a child) is work, and when
//! every thread of every process is at work the program is busy however much
//! its screen looks like a prompt ([`Probe::Idle`]): `Compiling foo... ` left
//! open while the compiler runs, `Working... ` before a `sleep`.
//!
//! A tree with a thread in such a wait and no reader is [`Probe::Polling`]:
//! it may be at a prompt, or waiting on the network — which is why, since
//! every command runs in a terminal (`docs/bash-tools.md`), its silence ends
//! a launch only after a long quiet, where [`Probe::Idle`]'s never does. A
//! tree waiting elsewhere asks nothing, but may wait on the network as long
//! as a server idles, so its silence ends a launch the same way.
//!
//! The files are readable only for the user's own processes, so a
//! `sudo`-elevated program leaves the probe blind — its prompts are judged
//! by the screen and the terminal's mode, as before
//! ([`Probe::Unknown`]).
//!
//! The parse and the verdict are pure: [`classify`] asks about a process's
//! descriptors through [`Descriptors`], a table in the tests. The walk over
//! the session's processes ([`probe`]) is boundary code the monitor runs
//! while a call waits on a quiet session (`crate::background`).

use std::path::{Path, PathBuf};

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
    /// `poll`-family call that may be on it — a REPL, `vim`, a relay like
    /// `ssh` — or on anything else the probe could not see into.
    Polling,
    /// Every thread was seen, none reads the terminal or waits on anything
    /// that could be it, and one waits in an epoll call on something else —
    /// a socket, a pipe, a timer: an event loop between network calls or
    /// redraws. Not waiting for input; but a network wait may last as long as
    /// a server idles, so not busy the way [`Probe::Idle`] is either.
    Elsewhere,
    /// Nothing certain — not probed since the session last changed, a
    /// process the probe may not inspect, or no `/proc` to ask.
    #[default]
    Unknown,
}

/// A blocked thread's system call, from its `/proc/PID/syscall` line
/// (`281 0x3 0x7f… 0x80 0x752f 0x0 0x8 …`): its number and six arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Syscall {
    pub number: u64,
    pub args: [u64; 6],
}

/// Parse a `/proc/PID/syscall` line. `None` for a thread that is running
/// (`running`), not in a call (`-1 …`), or anything unparseable.
#[must_use]
pub fn parse_syscall(line: &str) -> Option<Syscall> {
    let mut fields = line.split_whitespace();
    let number = fields.next()?.parse().ok()?;
    let mut args = [0; 6];
    for arg in &mut args {
        *arg = u64::from_str_radix(fields.next()?.strip_prefix("0x")?, 16).ok()?;
    }
    Some(Syscall { number, args })
}

/// A system-call table: the calls the probe tells apart are numbered
/// differently on each architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
    /// Any other: no call known, which only blinds the probe.
    Other,
}

impl Arch {
    /// The architecture this build runs on.
    pub const HOST: Self = if cfg!(target_arch = "x86_64") {
        Self::X86_64
    } else if cfg!(target_arch = "aarch64") {
        Self::Aarch64
    } else {
        Self::Other
    };

    /// What `call` waits on, when it is a wait the probe knows: `read`,
    /// `readv` or `pread64`; `poll` or `ppoll`; `select` or `pselect6`;
    /// `epoll_wait` or a `pwait` sibling.
    #[must_use]
    pub fn wait(self, call: Syscall) -> Option<Wait> {
        let [first, second, ..] = call.args;
        match (self, call.number) {
            (Self::X86_64, 0 | 17 | 19) | (Self::Aarch64, 63 | 65 | 67) => {
                Some(Wait::Read { fd: first })
            }
            (Self::X86_64, 7 | 271) | (Self::Aarch64, 73) => Some(Wait::Poll { fds: second }),
            (Self::X86_64, 23 | 270) | (Self::Aarch64, 72) => Some(Wait::Select { fds: first }),
            (Self::X86_64, 232 | 281 | 441) | (Self::Aarch64, 22 | 441) => {
                Some(Wait::Epoll { epfd: first })
            }
            _ => None,
        }
    }
}

/// A wait the probe knows, with the argument that says what it waits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// A read of descriptor `fd`.
    Read { fd: u64 },
    /// `poll` or `ppoll` over `fds` descriptors — a sleep, over none.
    Poll { fds: u64 },
    /// `select` or `pselect6` over the descriptors below `fds` — a sleep,
    /// over none.
    Select { fds: u64 },
    /// An epoll wait on the instance at descriptor `epfd`.
    Epoll { epfd: u64 },
}

/// A file as the kernel knows it: its inode and its filesystem's device —
/// the same whatever descriptor, or descriptor number, leads to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileId {
    pub ino: u64,
    /// The filesystem's device, as `(major, minor)`.
    pub dev: (u32, u32),
}

impl FileId {
    /// From a `stat` of the file: `st_dev` in the encoding Linux hands user
    /// space, glibc's `major()` and `minor()`.
    #[must_use]
    pub fn from_stat(ino: u64, dev: u64) -> Self {
        let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & 0xffff_f000);
        let minor = (dev & 0xff) | ((dev >> 12) & 0xffff_ff00);
        Self {
            ino,
            dev: (major as u32, minor as u32),
        }
    }

    /// From an epoll instance's `fdinfo` line: `sdev` in the kernel's own
    /// encoding, twenty bits of minor under the major.
    #[must_use]
    pub fn from_kernel(ino: u64, sdev: u64) -> Self {
        Self {
            ino,
            dev: ((sdev >> 20) as u32, (sdev & 0xf_ffff) as u32),
        }
    }
}

/// One file an epoll instance watches — a `tfd:` line of its fdinfo
/// (`tfd: 4 events: 19 data: 986650  pos:0 ino:80e sdev:10`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpollTarget {
    /// Its descriptor's number when it was added — closed, or reused for
    /// another file, since, perhaps.
    pub fd: u64,
    /// The events it is watched for.
    pub events: u32,
    /// The file itself — named since Linux 4.13, and exact.
    pub file: Option<FileId>,
}

/// The events of input arriving: `EPOLLIN`, `EPOLLPRI`, `EPOLLRDNORM` and
/// `EPOLLRDBAND`.
const READ_EVENTS: u32 = 0x001 | 0x002 | 0x040 | 0x080;

impl EpollTarget {
    /// Is it watched for input — not only for room to write?
    #[must_use]
    pub fn reads(&self) -> bool {
        self.events & READ_EVENTS != 0
    }
}

/// The files an epoll instance watches, from its fdinfo — `None` when a
/// `tfd:` line will not parse, since what it names would be a guess.
#[must_use]
pub fn epoll_targets(fdinfo: &str) -> Option<Vec<EpollTarget>> {
    fdinfo
        .lines()
        .filter_map(|line| line.strip_prefix("tfd:"))
        .map(epoll_target)
        .collect()
}

/// A `tfd:` line after its label.
fn epoll_target(line: &str) -> Option<EpollTarget> {
    let mut fields = line.split_whitespace();
    let fd = fields.next()?.parse().ok()?;
    let (mut events, mut ino, mut sdev) = (None, None, None);
    while let Some(field) = fields.next() {
        if field == "events:" {
            events = Some(u32::from_str_radix(fields.next()?, 16).ok()?);
        } else if let Some(hex) = field.strip_prefix("ino:") {
            ino = Some(u64::from_str_radix(hex, 16).ok()?);
        } else if let Some(hex) = field.strip_prefix("sdev:") {
            sdev = Some(u64::from_str_radix(hex, 16).ok()?);
        }
    }
    Some(EpollTarget {
        fd,
        events: events?,
        file: ino
            .zip(sdev)
            .map(|(ino, sdev)| FileId::from_kernel(ino, sdev)),
    })
}

/// Where a descriptor of an epoll instance leads.
const EPOLL_LINK: &str = "anon_inode:[eventpoll]";

/// The most epoll instances inside epoll instances the probe follows.
const EPOLL_MAX_DEPTH: usize = 4;

/// The most files of one interest list the probe weighs; past it the wait
/// is left open.
const EPOLL_MAX_TARGETS: usize = 4096;

/// What the probe asks about a process's descriptors — `/proc` in [`probe`],
/// a table in the tests.
pub trait Descriptors {
    /// Where descriptor `fd` leads (`/proc/PID/fd/N`): a path, or a kernel
    /// object's name — `anon_inode:[eventpoll]`, `socket:[123]`.
    fn link(&self, fd: u64) -> Option<String>;
    /// The file descriptor `fd` leads to now.
    fn file(&self, fd: u64) -> Option<FileId>;
    /// Descriptor `fd`'s fdinfo (`/proc/PID/fdinfo/N`) — for an epoll
    /// instance, its interest list.
    fn fdinfo(&self, fd: u64) -> Option<String>;
}

/// The session's terminal as the probe knows it: its path, as a descriptor's
/// link shows it, and the files it and `/dev/tty` — every process's name for
/// its controlling terminal — are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    pub path: PathBuf,
    pub files: Vec<FileId>,
}

impl Terminal {
    /// The terminal at `path` (its `/dev/pts/N`), with its file and
    /// `/dev/tty`'s read off them.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn of(path: &Path) -> Self {
        use std::os::unix::fs::MetadataExt;
        let files = [path, Path::new("/dev/tty")]
            .iter()
            .filter_map(|path| std::fs::metadata(path).ok())
            .map(|meta| FileId::from_stat(meta.ino(), meta.dev()))
            .collect();
        Self {
            path: path.to_path_buf(),
            files,
        }
    }

    fn leads_here(&self, link: &str) -> bool {
        is_session_terminal(Path::new(link), &self.path)
    }
}

/// What one thread's `/proc/…/syscall` line says about the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thread {
    /// Blocked in a read on the session's terminal.
    Reading,
    /// Blocked in a `poll`-family wait that may be on the terminal.
    Maybe,
    /// Blocked in an epoll wait on files none of which is the terminal.
    Elsewhere,
    /// Anything else: running, sleeping, reading a pipe, waiting on a child.
    Busy,
}

/// Classify one thread's syscall `line`, on `arch`, asking `fds` about its
/// process's descriptors.
#[must_use]
pub fn classify(arch: Arch, line: &str, fds: &impl Descriptors, terminal: &Terminal) -> Thread {
    match parse_syscall(line).and_then(|call| arch.wait(call)) {
        Some(Wait::Read { fd }) if fds.link(fd).is_some_and(|link| terminal.leads_here(&link)) => {
            Thread::Reading
        }
        Some(Wait::Poll { fds: 0 } | Wait::Select { fds: 0 }) => Thread::Busy,
        Some(Wait::Poll { .. } | Wait::Select { .. }) => Thread::Maybe,
        Some(Wait::Epoll { epfd }) => match epoll_watch(fds, terminal, epfd, EPOLL_MAX_DEPTH) {
            Watch::Elsewhere => Thread::Elsewhere,
            Watch::Unknown | Watch::Terminal => Thread::Maybe,
        },
        Some(Wait::Read { .. }) | None => Thread::Busy,
    }
}

/// What an epoll instance's interest list says about the terminal, in the
/// order a list takes the strongest word of its files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Watch {
    /// Nothing it watches for input is the terminal.
    Elsewhere,
    /// Something it watches could not be told apart.
    Unknown,
    /// It watches the terminal for input.
    Terminal,
}

/// What the epoll instance at `epfd` watches — following an instance inside
/// it `depth` more levels.
fn epoll_watch(fds: &impl Descriptors, terminal: &Terminal, epfd: u64, depth: usize) -> Watch {
    // No instance there — a number misread, or a descriptor closed since.
    if fds.link(epfd).as_deref() != Some(EPOLL_LINK) {
        return Watch::Unknown;
    }
    // Every eventpoll, eventfd, timerfd and signalfd is the one anonymous
    // inode this instance is: a file that is not it is no epoll instance.
    let Some(anonymous) = fds.file(epfd) else {
        return Watch::Unknown;
    };
    let Some(targets) = fds.fdinfo(epfd).and_then(|info| epoll_targets(&info)) else {
        return Watch::Unknown;
    };
    if targets.len() > EPOLL_MAX_TARGETS {
        return Watch::Unknown;
    }
    let mut watch = Watch::Elsewhere;
    for target in targets.iter().filter(|target| target.reads()) {
        watch = watch.max(match target.file {
            Some(file) if terminal.files.contains(&file) => Watch::Terminal,
            Some(file) if file != anonymous => Watch::Elsewhere,
            _ => target_watch(fds, terminal, target, depth),
        });
        if watch == Watch::Terminal {
            break;
        }
    }
    watch
}

/// What a watched file is, asked of its descriptor: an anonymous file —
/// an epoll instance of its own, perhaps — or one on a kernel too old to
/// name its files.
fn target_watch(
    fds: &impl Descriptors,
    terminal: &Terminal,
    target: &EpollTarget,
    depth: usize,
) -> Watch {
    // A number closed or reused since hides what it named.
    if target.file.is_some() && fds.file(target.fd) != target.file {
        return Watch::Unknown;
    }
    match fds.link(target.fd) {
        None => Watch::Unknown,
        Some(link) if link == EPOLL_LINK => match depth.checked_sub(1) {
            Some(depth) => epoll_watch(fds, terminal, target.fd, depth),
            None => Watch::Unknown,
        },
        Some(link) if terminal.leads_here(&link) => Watch::Terminal,
        Some(_) => Watch::Elsewhere,
    }
}

/// The verdict over every thread inspected: a reader anywhere is
/// [`Probe::Reading`]; otherwise one the probe could not inspect at all
/// (`blind`) leaves it [`Probe::Unknown`] — as does having seen nothing — a
/// thread that may be waiting on the terminal makes it [`Probe::Polling`],
/// one waiting only on other things [`Probe::Elsewhere`], and only threads
/// that are all busy make it [`Probe::Idle`].
#[must_use]
pub fn combine(threads: &[Thread], blind: bool) -> Probe {
    if threads.contains(&Thread::Reading) {
        Probe::Reading
    } else if blind || threads.is_empty() {
        Probe::Unknown
    } else if threads.contains(&Thread::Maybe) {
        Probe::Polling
    } else if threads.contains(&Thread::Elsewhere) {
        Probe::Elsewhere
    } else {
        Probe::Idle
    }
}

/// Does the descriptor link `target` name the session's terminal — its own
/// path, or `/dev/tty`, which is the controlling terminal of every process
/// in the session (where `ssh` and `git` read a password from)?
#[must_use]
pub fn is_session_terminal(target: &Path, terminal: &Path) -> bool {
    target == terminal || target == Path::new("/dev/tty")
}

/// The most threads one probe inspects; a tree larger than that is left
/// [`Probe::Unknown`] rather than walked forever.
#[cfg(target_os = "linux")]
const PROBE_MAX_TASKS: usize = 1024;

/// What the processes of the tree rooted at `pid` are blocked in, with
/// respect to the terminal at `terminal` (its `/dev/pts/N` path).
#[cfg(target_os = "linux")]
#[must_use]
pub fn probe(pid: u32, terminal: &Path) -> Probe {
    if Arch::HOST == Arch::Other {
        return Probe::Unknown;
    }
    let terminal = Terminal::of(terminal);
    let mut threads = Vec::new();
    let mut blind = false;
    let mut todo = vec![pid];
    let mut seen = 0usize;
    while let Some(pid) = todo.pop() {
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            continue; // gone since it was listed
        };
        for task in tasks.flatten() {
            seen += 1;
            if seen > PROBE_MAX_TASKS {
                return Probe::Unknown;
            }
            let dir = task.path();
            match std::fs::read_to_string(dir.join("syscall")) {
                Ok(line) => threads.push(classify_task(&dir, &line, &terminal)),
                // Another user's process — `sudo` and what it runs.
                Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => blind = true,
                Err(_) => continue, // exited, or a zombie: waiting on nothing
            }
            // A kernel without the `children` file hides the rest of the
            // tree, and a busy parent says nothing of a child that reads.
            match std::fs::read_to_string(dir.join("children")) {
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

/// Classify the thread at `dir`, blocked in `line`. A wait its descriptors
/// place elsewhere holds only while the thread is still in the call they
/// were read for: one that moved on meanwhile is left open.
#[cfg(target_os = "linux")]
fn classify_task(dir: &Path, line: &str, terminal: &Terminal) -> Thread {
    let thread = classify(Arch::HOST, line, &TaskDescriptors { dir }, terminal);
    if thread == Thread::Elsewhere
        && std::fs::read_to_string(dir.join("syscall")).ok().as_deref() != Some(line)
    {
        return Thread::Maybe;
    }
    thread
}

/// A thread's descriptors, read off its `/proc/PID/task/TID` directory.
#[cfg(target_os = "linux")]
struct TaskDescriptors<'a> {
    dir: &'a Path,
}

#[cfg(target_os = "linux")]
impl Descriptors for TaskDescriptors<'_> {
    fn link(&self, fd: u64) -> Option<String> {
        let target = std::fs::read_link(self.dir.join("fd").join(fd.to_string())).ok()?;
        Some(target.to_string_lossy().into_owned())
    }

    fn file(&self, fd: u64) -> Option<FileId> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(self.dir.join("fd").join(fd.to_string())).ok()?;
        Some(FileId::from_stat(meta.ino(), meta.dev()))
    }

    fn fdinfo(&self, fd: u64) -> Option<String> {
        std::fs::read_to_string(self.dir.join("fdinfo").join(fd.to_string())).ok()
    }
}

/// Elsewhere there is no `/proc` to ask: the other tells carry the verdict.
#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn probe(_pid: u32, _terminal: &Path) -> Probe {
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
    fn a_blocked_call_names_its_number_and_all_six_arguments() {
        // `gh run watch` between redraws, seen live: the Go runtime parked
        // in `epoll_pwait` on its instance at descriptor 3 until the next
        // timer, 29 999 ms away.
        assert_eq!(
            parse_syscall("281 0x3 0x7f1cd77fd5dc 0x80 0x752f 0x0 0x8 0x7f1cd77fd4f8 0x46f4a3"),
            Some(Syscall {
                number: 281,
                args: [3, 0x7f1c_d77f_d5dc, 0x80, 0x752f, 0, 8]
            })
        );
        assert_eq!(
            parse_syscall("0 0x0 0x7ffd3a0c6d37 0x1 0x0 0x0 0x0 0x7ffd3a0c6c78 0x7f2d"),
            Some(Syscall {
                number: 0,
                args: [0, 0x7ffd_3a0c_6d37, 1, 0, 0, 0]
            })
        );
    }

    #[test]
    fn a_thread_that_is_not_blocked_names_nothing() {
        assert_eq!(parse_syscall("running"), None);
        assert_eq!(parse_syscall("-1 0x7ffc 0x7f"), None);
        assert_eq!(parse_syscall(""), None);
        assert_eq!(parse_syscall("0 0x0 0x1"), None, "a line cut short");
    }

    #[test]
    fn each_architecture_numbers_the_waits_its_own_way() {
        let call = |number| Syscall {
            number,
            args: [3, 5, 0, 0, 0, 0],
        };
        let read = Some(Wait::Read { fd: 3 });
        let poll = Some(Wait::Poll { fds: 5 });
        let select = Some(Wait::Select { fds: 3 });
        let epoll = Some(Wait::Epoll { epfd: 3 });
        for (number, wait) in [
            (0, read),
            (17, read),
            (19, read),
            (7, poll),
            (271, poll),
            (23, select),
            (270, select),
            (232, epoll),
            (281, epoll),
            (441, epoll),
            (202, None), // a futex: parked on no descriptor
        ] {
            assert_eq!(Arch::X86_64.wait(call(number)), wait, "x86_64 {number}");
        }
        for (number, wait) in [
            (63, read),
            (65, read),
            (67, read),
            (73, poll),
            (72, select),
            (22, epoll),
            (441, epoll),
            (7, None), // poll's x86_64 number: another call here
        ] {
            assert_eq!(Arch::Aarch64.wait(call(number)), wait, "aarch64 {number}");
        }
        assert_eq!(Arch::Other.wait(call(0)), None);
    }

    /// The files of a session seen live: its terminal, `/dev/tty`, the one
    /// anonymous inode every eventpoll and eventfd is, and a socket.
    const PTS: FileId = FileId {
        ino: 3,
        dev: (0, 27),
    };
    const DEV_TTY: FileId = FileId {
        ino: 9,
        dev: (0, 6),
    };
    const ANON: FileId = FileId {
        ino: 0x80e,
        dev: (0, 16),
    };
    const SOCKET: FileId = FileId {
        ino: 0x303c,
        dev: (0, 9),
    };

    /// A process's descriptors as a table: where each leads, the file it
    /// is, and an epoll instance's interest list.
    #[derive(Default)]
    struct Table {
        links: std::collections::HashMap<u64, String>,
        files: std::collections::HashMap<u64, FileId>,
        infos: std::collections::HashMap<u64, String>,
    }

    impl Table {
        fn fd(mut self, fd: u64, link: &str, file: FileId) -> Self {
            self.links.insert(fd, link.to_string());
            self.files.insert(fd, file);
            self
        }

        /// An epoll instance at `fd` whose interest list is `lines`.
        fn epoll(self, fd: u64, lines: &[&str]) -> Self {
            let mut table = self.fd(fd, EPOLL_LINK, ANON);
            let info = format!(
                "pos:\t0\nflags:\t02000002\nmnt_id:\t15\nino:\t2062\n{}\n",
                lines.join("\n")
            );
            table.infos.insert(fd, info);
            table
        }
    }

    impl Descriptors for Table {
        fn link(&self, fd: u64) -> Option<String> {
            self.links.get(&fd).cloned()
        }

        fn file(&self, fd: u64) -> Option<FileId> {
            self.files.get(&fd).copied()
        }

        fn fdinfo(&self, fd: u64) -> Option<String> {
            self.infos.get(&fd).cloned()
        }
    }

    fn terminal() -> Terminal {
        Terminal {
            path: std::path::PathBuf::from("/dev/pts/0"),
            files: vec![PTS, DEV_TTY],
        }
    }

    /// A process with the terminal on its first three descriptors.
    fn process() -> Table {
        Table::default()
            .fd(0, "/dev/pts/0", PTS)
            .fd(1, "/dev/pts/0", PTS)
            .fd(2, "/dev/pts/0", PTS)
    }

    /// `epoll_pwait` (x86_64) on the instance at `epfd`.
    fn epoll_wait_on(epfd: u64) -> String {
        format!("281 {epfd:#x} 0x7f1cd77fd5dc 0x80 0x752f 0x0 0x8 0x7f1cd77fd4f8 0x46f4a3")
    }

    fn classify_x86(line: &str, fds: &Table) -> Thread {
        classify(Arch::X86_64, line, fds, &terminal())
    }

    #[test]
    fn an_epoll_line_names_its_descriptor_its_events_and_its_file() {
        let info = "pos:\t0\nflags:\t02000002\nmnt_id:\t15\nino:\t2062\n\
                    tfd:        4 events:       19 data:           986650  pos:0 ino:80e sdev:10\n\
                    tfd:        0 events: 8000201d data:     7f0000000000  pos:0 ino:3 sdev:1b\n";
        assert_eq!(
            epoll_targets(info),
            Some(vec![
                EpollTarget {
                    fd: 4,
                    events: 0x19,
                    file: Some(ANON)
                },
                EpollTarget {
                    fd: 0,
                    events: 0x8000_201d,
                    file: Some(PTS)
                },
            ])
        );
        // Before Linux 4.13 a line named no file.
        assert_eq!(
            epoll_targets("tfd:        5 events:       19 data:               5\n"),
            Some(vec![EpollTarget {
                fd: 5,
                events: 0x19,
                file: None
            }])
        );
        assert_eq!(
            epoll_targets("pos:\t0\nflags:\t02\n"),
            Some(vec![]),
            "an empty list"
        );
        assert_eq!(
            epoll_targets("tfd: four events: 19 data: 0\n"),
            None,
            "a line that will not parse"
        );
        assert_eq!(epoll_targets("tfd:        4 data: 0\n"), None, "no events");
    }

    #[test]
    fn a_kernel_device_number_and_a_stat_one_name_the_same_device() {
        // fdinfo prints the kernel's own encoding, twenty bits of minor under
        // the major, where stat hands out glibc's.
        assert_eq!(FileId::from_kernel(3, 0x1b), FileId::from_stat(3, 27));
        assert_eq!(FileId::from_kernel(3, 0x1b), PTS);
        assert_eq!(
            FileId::from_kernel(7, (253 << 20) | 1),
            FileId::from_stat(7, 0xfd01),
            "a device-mapper disk"
        );
        assert_eq!(
            FileId::from_kernel(7, 300),
            FileId::from_stat(7, 0x10_002c),
            "a minor past 255"
        );
        assert_eq!(
            FileId::from_kernel(7, 4096 << 20),
            FileId::from_stat(7, 0x1000_0000_0000),
            "a major past 4095"
        );
        assert_ne!(FileId::from_kernel(3, 0x1b), FileId::from_stat(3, 28));
    }

    #[test]
    fn only_an_interest_in_input_counts() {
        let target = |events| EpollTarget {
            fd: 0,
            events,
            file: None,
        };
        assert!(
            target(0x19).reads(),
            "EPOLLIN, and the hangups the kernel adds"
        );
        assert!(
            target(0x8000_201d).reads(),
            "Go's edge-triggered read and write"
        );
        assert!(target(0x2).reads(), "EPOLLPRI");
        assert!(
            !target(0x8000_001c).reads(),
            "watched for room to write alone"
        );
    }

    #[test]
    fn an_event_loop_sleeping_on_its_own_wakeup_waits_elsewhere() {
        // `gh run watch` between redraws, seen live: threads parked in
        // futexes and one in `epoll_pwait` on an instance watching the Go
        // runtime's eventfd — nothing that is the terminal.
        let gh = process()
            .epoll(
                3,
                &["tfd:        4 events:       19 data:           986650  pos:0 ino:80e sdev:10"],
            )
            .fd(4, "anon_inode:[eventfd]", ANON);
        assert_eq!(classify_x86(&epoll_wait_on(3), &gh), Thread::Elsewhere);
        assert_eq!(
            classify_x86("202 0x9667f8 0x80 0x0 0x0 0x0 0x0 0x7ffd 0x46f4a3", &gh),
            Thread::Busy,
            "a futex: parked"
        );
    }

    #[test]
    fn an_event_loop_between_network_calls_waits_elsewhere() {
        // asyncio asleep, seen live: its instance watches a socket, which
        // the line's own file says is no terminal.
        let asyncio = process().epoll(
            3,
            &["tfd:        4 events:       19 data:     7fd800000004  pos:0 ino:303c sdev:9"],
        );
        assert_eq!(
            classify_x86(
                "232 0x3 0x7fd8aa92c7e0 0x2 0xffffffff 0x0 0x0 0x7ffd 0x7f",
                &asyncio
            ),
            Thread::Elsewhere
        );
    }

    #[test]
    fn an_epoll_watching_the_terminal_may_be_waiting_on_it() {
        // `epoll.register(0)`, seen live — and the tcell pattern, Go opening
        // `/dev/tty`, named by the file `/dev/tty` is.
        let stdin = process().epoll(
            3,
            &["tfd:        0 events:       19 data:     7f0000000000  pos:0 ino:3 sdev:1b"],
        );
        assert_eq!(classify_x86(&epoll_wait_on(3), &stdin), Thread::Maybe);
        let tty = process()
            .epoll(
                3,
                &[
                    "tfd:        4 events:       19 data:           986650  pos:0 ino:80e sdev:10",
                    "tfd:        7 events: 8000201d data:           986650  pos:0 ino:9 sdev:6",
                ],
            )
            .fd(4, "anon_inode:[eventfd]", ANON)
            .fd(7, "/dev/tty", DEV_TTY);
        assert_eq!(classify_x86(&epoll_wait_on(3), &tty), Thread::Maybe);
    }

    #[test]
    fn a_number_reused_since_still_names_the_terminal_it_was() {
        // A child sharing its parent's instance closed descriptor 0 and
        // opened /dev/null there: the line's number misleads, its file does
        // not.
        let child = Table::default()
            .fd(
                0,
                "/dev/null",
                FileId {
                    ino: 5,
                    dev: (0, 6),
                },
            )
            .epoll(
                3,
                &["tfd:        0 events:       19 data:               0  pos:0 ino:3 sdev:1b"],
            );
        assert_eq!(classify_x86(&epoll_wait_on(3), &child), Thread::Maybe);
    }

    #[test]
    fn a_terminal_watched_only_for_room_to_write_is_not_read() {
        let writer = process().epoll(
            3,
            &["tfd:        1 events: 8000001c data:               1  pos:0 ino:3 sdev:1b"],
        );
        assert_eq!(classify_x86(&epoll_wait_on(3), &writer), Thread::Elsewhere);
    }

    #[test]
    fn an_epoll_inside_an_epoll_is_looked_into() {
        let nested = |inner: &str| {
            process()
                .epoll(3, &["tfd:        5 events:       19 data:               5  pos:0 ino:80e sdev:10"])
                .epoll(5, &[inner])
        };
        let terminal =
            nested("tfd:        0 events:       19 data:               0  pos:0 ino:3 sdev:1b");
        assert_eq!(classify_x86(&epoll_wait_on(3), &terminal), Thread::Maybe);
        let socket =
            nested("tfd:        6 events:       19 data:               6  pos:0 ino:303c sdev:9");
        assert_eq!(classify_x86(&epoll_wait_on(3), &socket), Thread::Elsewhere);
        // Nested past what the probe follows: left open.
        let mut deep = process();
        let last = 10 + EPOLL_MAX_DEPTH as u64 + 1;
        for fd in 10..last {
            let line = format!(
                "tfd: {:8} events:       19 data:               0  pos:0 ino:80e sdev:10",
                fd + 1
            );
            deep = deep.epoll(fd, &[&line]);
        }
        deep = deep.epoll(
            last,
            &["tfd:        6 events:       19 data:               6  pos:0 ino:303c sdev:9"],
        );
        assert_eq!(classify_x86(&epoll_wait_on(10), &deep), Thread::Maybe);
    }

    #[test]
    fn what_the_probe_cannot_see_into_stays_open() {
        let eventfd =
            "tfd:        4 events:       19 data:           986650  pos:0 ino:80e sdev:10";
        let mut unreadable = process()
            .epoll(3, &[eventfd])
            .fd(4, "anon_inode:[eventfd]", ANON);
        unreadable.infos.clear();
        assert_eq!(
            classify_x86(&epoll_wait_on(3), &unreadable),
            Thread::Maybe,
            "no list to read"
        );
        let not_epoll = process().fd(3, "socket:[9]", SOCKET);
        assert_eq!(
            classify_x86(&epoll_wait_on(3), &not_epoll),
            Thread::Maybe,
            "no epoll instance there: a number misread, or closed since"
        );
        let reused = process().epoll(3, &[eventfd]).fd(4, "socket:[9]", SOCKET);
        assert_eq!(
            classify_x86(&epoll_wait_on(3), &reused),
            Thread::Maybe,
            "an anonymous file whose number now names another: an epoll instance, perhaps"
        );
        let closed = process().epoll(3, &[eventfd]);
        assert_eq!(classify_x86(&epoll_wait_on(3), &closed), Thread::Maybe);
        let garbled = process().epoll(3, &["tfd: four events: 19"]);
        assert_eq!(classify_x86(&epoll_wait_on(3), &garbled), Thread::Maybe);
    }

    #[test]
    fn an_old_kernels_list_is_read_off_the_descriptors() {
        // Before Linux 4.13 a line named no file: its descriptor is asked.
        let old = |fd: u64| {
            process()
                .epoll(
                    3,
                    &[&format!(
                        "tfd: {fd:8} events:       19 data:               0"
                    )],
                )
                .fd(4, "socket:[9]", SOCKET)
        };
        assert_eq!(
            classify_x86(&epoll_wait_on(3), &old(0)),
            Thread::Maybe,
            "the terminal"
        );
        assert_eq!(
            classify_x86(&epoll_wait_on(3), &old(4)),
            Thread::Elsewhere,
            "a socket"
        );
        assert_eq!(
            classify_x86(&epoll_wait_on(3), &old(8)),
            Thread::Maybe,
            "a descriptor gone"
        );
    }

    #[test]
    fn a_poll_over_no_descriptors_is_a_sleep() {
        // perl's `select(undef, undef, undef, 30)`, seen live: `pselect6`
        // over no descriptors waits on nothing at all.
        let fds = process();
        assert_eq!(
            classify_x86("270 0x0 0x0 0x0 0x0 0x7ffd 0x0 0x7ffd 0x7f", &fds),
            Thread::Busy
        );
        assert_eq!(
            classify_x86("7 0x0 0x0 0x7530 0x0 0x0 0x0 0x7ffd 0x7f", &fds),
            Thread::Busy,
            "poll"
        );
        assert_eq!(
            classify_x86("270 0x1 0x7ffd15110d40 0x0 0x0 0x0 0x0 0x7ffd 0x7f", &fds),
            Thread::Maybe,
            "select on stdin, seen live"
        );
        assert_eq!(
            classify_x86("271 0x7ffd 0x2 0x0 0x0 0x8 0x0 0x7ffd 0x7f", &fds),
            Thread::Maybe,
            "ppoll over two"
        );
    }

    #[test]
    fn a_read_is_reading_the_terminal_only_on_the_terminal() {
        let fds = process().fd(
            5,
            "pipe:[77]",
            FileId {
                ino: 77,
                dev: (0, 13),
            },
        );
        let read = |fd: u64| format!("0 {fd:#x} 0x7ffd 0x1 0x0 0x0 0x0 0x7ffd 0x7f2d");
        assert_eq!(classify_x86(&read(0), &fds), Thread::Reading);
        assert_eq!(classify_x86(&read(5), &fds), Thread::Busy, "a pipe");
        assert_eq!(
            classify_x86(&read(9), &fds),
            Thread::Busy,
            "a descriptor gone"
        );
        assert_eq!(classify_x86("running", &fds), Thread::Busy);
        assert_eq!(
            classify(
                Arch::Aarch64,
                "63 0x0 0x7ffd 0x1 0x0 0x0 0x0 0x7ffd 0x7f2d",
                &fds,
                &terminal()
            ),
            Thread::Reading,
            "aarch64's read"
        );
    }

    #[test]
    fn one_reader_anywhere_is_reading() {
        assert_eq!(
            combine(
                &[
                    Thread::Busy,
                    Thread::Maybe,
                    Thread::Elsewhere,
                    Thread::Reading
                ],
                true
            ),
            Probe::Reading
        );
    }

    #[test]
    fn a_tree_waiting_only_on_other_things_waits_elsewhere() {
        assert_eq!(
            combine(&[Thread::Busy, Thread::Elsewhere], false),
            Probe::Elsewhere
        );
        assert_eq!(
            combine(&[Thread::Elsewhere, Thread::Maybe], false),
            Probe::Polling,
            "one wait may still be on the terminal"
        );
        assert_eq!(combine(&[Thread::Elsewhere], true), Probe::Unknown);
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

    /// Probe `command`, run in a terminal of its own, until it says `want`
    /// or five seconds pass; the last verdict.
    #[cfg(target_os = "linux")]
    fn probe_until(command: &str, want: Probe) -> Probe {
        use crate::pty::spawn::{spawn, terminal_path};
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
    }

    /// Is `program` on `PATH`? The live tests that need an interpreter skip
    /// without one.
    #[cfg(target_os = "linux")]
    fn on_path(program: &str) -> bool {
        std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_live_event_loop_waits_elsewhere_and_one_watching_the_terminal_polls() {
        // Real interpreters, where installed: what the kernel prints for
        // them is what the parse is written against.
        for (program, command, want) in [
            (
                "python3",
                "python3 -c 'import asyncio; asyncio.run(asyncio.sleep(30))'",
                Probe::Elsewhere,
            ),
            (
                "python3",
                "python3 -c 'import select; e = select.epoll(); \
                 e.register(0, select.EPOLLIN); e.poll(30)'",
                Probe::Polling,
            ),
            (
                "python3",
                "python3 -c 'import select, sys; select.select([sys.stdin], [], [])'",
                Probe::Polling,
            ),
            (
                "perl",
                "perl -e 'select(undef, undef, undef, 30)'",
                Probe::Idle,
            ),
        ] {
            if !on_path(program) {
                eprintln!("skipped, no {program}: {command}");
                continue;
            }
            assert_eq!(probe_until(command, want), want, "{command}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_live_session_reading_its_terminal_is_seen_reading_and_a_sleeping_one_idle() {
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
