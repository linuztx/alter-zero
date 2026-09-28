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
//! `splice` from the terminal can be waiting for input too (modern `cat`),
//! or for space in its output pipe: its task's `wchan` distinguishes the
//! terminal read from the full pipe, leaving unfamiliar channels uncertain.
//! A thread in
//! `poll`/`select`/`epoll` may be waiting on the terminal or on anything
//! else — a REPL, `vim`, `ssh` and every network client wait that way — and
//! the kernel says which. An epoll instance's interest list
//! (`/proc/PID/fdinfo/N`) names every file it watches, by inode; a `poll`'s
//! array and a `select`'s read set live in the program's own memory, at the
//! address the call's arguments give, which `/proc/PID/task/TID/mem` hands
//! to whoever may read the syscall line itself. A wait none of whose files
//! is a terminal is on something else ([`Probe::Elsewhere`]): an event loop
//! between network calls or timers, the way every Go, Node, libuv and asyncio
//! program idles — `gh run watch` between redraws — or a parent reading a
//! child's pipes. A `poll` or `select` over no descriptors is a sleep. Every
//! other state (running, sleeping, reading a pipe, waiting on a child) is
//! work, and when every thread of every process is at work the program is
//! busy however much its screen looks like a prompt ([`Probe::Idle`]):
//! `Compiling foo... ` left open while the compiler runs, `Working... ` before
//! a `sleep`.
//!
//! A wait no prompt can be in counts as work, too, in a process with a
//! descendant at work — one whose CPU grew since the last probe ([`Work`],
//! [`verdict`]): cargo polls rustc's stdout and stderr while rustc compiles
//! in silence, npm's event loop waits on webpack's, and neither is at a
//! prompt. So does a relay's: a wait on the terminal and a terminal's master
//! at once (`script`, `sudo`'s own terminal), passing keys on to a program in
//! a terminal of its own, where that program's prompt is seen. A wait on a terminal is never work, whatever the children do — bash
//! at its prompt over a job in the background, a REPL over the child it
//! started — nor is a read on one, nor a wait the probe could not see into.
//!
//! A tree with a thread waiting on a terminal, or in a wait the probe could
//! not see into, and no reader is [`Probe::Polling`]: it may be at a prompt,
//! or waiting on the network — which is why, since every command runs in a
//! terminal (`docs/bash-tools.md`), its silence ends a launch only after a
//! long quiet, where [`Probe::Idle`]'s never does. A tree waiting elsewhere
//! asks nothing, but may wait on the network as long as a server idles, so
//! its silence ends a launch the same way.
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
    /// Every thread was seen, none reads the terminal, and one waits on a
    /// terminal — a REPL, `vim`, a relay like `ssh` — or in a wait the probe
    /// could not see into.
    Polling,
    /// Every thread was seen, none reads the terminal or waits on anything
    /// that could be it, and one waits on something else — a socket, a pipe,
    /// a timer: an event loop between network calls or redraws, a parent
    /// reading a child's output. Not waiting for input; but a network wait
    /// may last as long as a server idles, so not busy the way
    /// [`Probe::Idle`] is either.
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
    /// `readv` or `pread64`; `splice`; `poll` or `ppoll`; `select` or `pselect6`;
    /// `epoll_wait` or a `pwait` sibling.
    #[must_use]
    pub fn wait(self, call: Syscall) -> Option<Wait> {
        let [first, second, ..] = call.args;
        match (self, call.number) {
            (Self::X86_64, 0 | 17 | 19) | (Self::Aarch64, 63 | 65 | 67) => {
                Some(Wait::Read { fd: first })
            }
            (Self::X86_64, 275) | (Self::Aarch64, 76) => Some(Wait::Splice { fd: first }),
            (Self::X86_64, 7 | 271) | (Self::Aarch64, 73) => Some(Wait::Poll {
                at: first,
                count: second,
            }),
            (Self::X86_64, 23 | 270) | (Self::Aarch64, 72) => Some(Wait::Select {
                count: first,
                read_set: second,
            }),
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
    /// A splice from descriptor `fd`: it may wait on its output pipe first.
    Splice { fd: u64 },
    /// `poll` or `ppoll` over the `count` entries of the `pollfd` array at
    /// address `at` — a sleep, over none.
    Poll { at: u64, count: u64 },
    /// `select` or `pselect6` over the descriptors below `count`, those it
    /// waits to read marked in the set at address `read_set` (none, at 0) —
    /// a sleep, over none.
    Select { count: u64, read_set: u64 },
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
    /// `len` bytes of the process's memory at `address` (`/proc/PID/mem`),
    /// where a `poll` or a `select` keeps the descriptors it waits on —
    /// `None` when fewer can be read.
    fn memory(&self, address: u64, len: usize) -> Option<Vec<u8>>;
    /// Where this task sleeps (`/proc/PID/task/TID/wchan`), when readable.
    /// A splice can wait for terminal input or space in its output pipe.
    fn wait_channel(&self) -> Option<String> {
        None
    }
}

/// How much of a `poll`'s array one entry takes: `int fd; short events;
/// short revents`, the same on every architecture the probe knows.
const POLLFD_SIZE: usize = 8;

/// The descriptors a `poll` waits to read, off its `pollfd` array: those
/// watched for input (`POLLIN`, `POLLPRI`, `POLLRDNORM`, `POLLRDBAND` — the
/// bits epoll's own read events are), a negative one being skipped as the
/// kernel skips it.
#[must_use]
pub fn poll_inputs(array: &[u8]) -> Vec<u64> {
    array
        .chunks_exact(POLLFD_SIZE)
        .filter_map(|entry| {
            let fd = i32::from_ne_bytes(entry[..4].try_into().ok()?);
            let events = u16::from_ne_bytes(entry[4..6].try_into().ok()?);
            (u32::from(events) & READ_EVENTS != 0)
                .then(|| u64::try_from(fd).ok())
                .flatten()
        })
        .collect()
}

/// How much of a `select` set one word takes — an `unsigned long`, whose bit
/// `fd % 64` marks descriptor `fd` of the word's sixty-four.
const FD_SET_WORD: usize = 8;

/// The descriptors a `select` waits to read below `count`, off its read set.
#[must_use]
pub fn select_inputs(set: &[u8], count: u64) -> Vec<u64> {
    let mut fds = Vec::new();
    for (index, word) in (0u64..).zip(set.chunks_exact(FD_SET_WORD)) {
        let Ok(word) = word.try_into().map(u64::from_ne_bytes) else {
            continue;
        };
        fds.extend(
            (0..64)
                .filter(|bit| word & (1 << bit) != 0)
                .map(|bit| index * 64 + bit)
                .filter(|&fd| fd < count),
        );
    }
    fds
}

/// The most descriptors of a `poll` or a `select` the probe looks up.
const WAIT_LIST_MAX: usize = 1024;

/// The session's terminal as the probe knows it: its path, as a descriptor's
/// link shows it, and the files it and `/dev/tty` — every process's name for
/// its controlling terminal — are; and, to know other terminals by their
/// files too, the filesystem their slave sides share with it and the files
/// their master sides open as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    pub path: PathBuf,
    pub files: Vec<FileId>,
    /// The filesystem every terminal's slave side is on (`devpts`), read off
    /// the session's own.
    pub slaves: Option<(u32, u32)>,
    /// The files a terminal's master side opens as: `/dev/ptmx`, and the
    /// `/dev/pts/ptmx` a container's `/dev/ptmx` leads to.
    pub masters: Vec<FileId>,
}

impl Terminal {
    /// The terminal at `path` (its `/dev/pts/N`), with the files it,
    /// `/dev/tty` and the masters' names are read off them.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn of(path: &Path) -> Self {
        use std::os::unix::fs::MetadataExt;
        let file = |path: &Path| {
            std::fs::metadata(path)
                .ok()
                .map(|meta| FileId::from_stat(meta.ino(), meta.dev()))
        };
        let own = file(path);
        let mut masters: Vec<FileId> = ["/dev/ptmx", "/dev/pts/ptmx"]
            .iter()
            .filter_map(|master| file(Path::new(master)))
            .collect();
        masters.dedup();
        Self {
            path: path.to_path_buf(),
            files: own.into_iter().chain(file(Path::new("/dev/tty"))).collect(),
            slaves: own.map(|own| own.dev),
            masters,
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
    /// Waiting on a terminal — the session's, or the one a relay runs its
    /// program in — in a `poll`, `select` or epoll wait, or reading one a
    /// relay runs: a prompt, perhaps, whatever else is at work.
    Terminal,
    /// Waiting on a terminal and on a terminal's master side at once: a
    /// relay (`script`, `sudo`'s own terminal) passing keys on to a program
    /// in a terminal of its own, whose prompt, if any, is that program's.
    Relay,
    /// In a wait the probe could not see into: it may be on the terminal.
    Maybe,
    /// Waiting on files none of which is a terminal — pipes, sockets, the
    /// program's own wakeups.
    Elsewhere,
    /// Anything else: running, sleeping, reading a pipe, waiting on a child.
    Busy,
}

/// Classify one thread's syscall `line`, on `arch`, asking `fds` about its
/// process's descriptors.
#[must_use]
pub fn classify(arch: Arch, line: &str, fds: &impl Descriptors, terminal: &Terminal) -> Thread {
    match parse_syscall(line).and_then(|call| arch.wait(call)) {
        Some(wait @ (Wait::Read { fd } | Wait::Splice { fd })) => {
            let reading = match fds.link(fd) {
                Some(link) if terminal.leads_here(&link) => Thread::Reading,
                Some(link) if is_terminal_slave(&link) => Thread::Terminal,
                _ => Thread::Busy,
            };
            match (wait, reading) {
                (Wait::Splice { .. }, Thread::Reading | Thread::Terminal) => {
                    // Linux checks the output pipe's capacity before reading
                    // from the terminal. Only its terminal-read wait proves
                    // input is wanted; a hidden or unfamiliar channel does not.
                    match fds.wait_channel().as_deref().map(str::trim) {
                        Some("wait_woken" | "n_tty_read") => reading,
                        Some("pipe_wait_writable") => Thread::Busy,
                        _ => Thread::Maybe,
                    }
                }
                _ => reading,
            }
        }
        Some(Wait::Poll { count: 0, .. } | Wait::Select { count: 0, .. }) | None => Thread::Busy,
        Some(Wait::Poll { at, count }) => poll_watch(fds, terminal, at, count).thread(),
        Some(Wait::Select { count, read_set }) => {
            select_watch(fds, terminal, count, read_set).thread()
        }
        Some(Wait::Epoll { epfd }) => epoll_watch(fds, terminal, epfd, EPOLL_MAX_DEPTH).thread(),
    }
}

/// What a wait's files say about terminals, in the order a set of them takes
/// the strongest word of its files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Watch {
    /// Nothing it watches for input is a terminal.
    Elsewhere,
    /// Something it watches could not be told apart.
    Unknown,
    /// It watches a terminal for input.
    Terminal,
}

/// What a wait's files come to: the strongest word of them, and whether one
/// is a terminal's master side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Watched {
    watch: Watch,
    master: bool,
}

impl Watched {
    const ELSEWHERE: Self = Self {
        watch: Watch::Elsewhere,
        master: false,
    };
    const UNKNOWN: Self = Self {
        watch: Watch::Unknown,
        master: false,
    };
    const TERMINAL: Self = Self {
        watch: Watch::Terminal,
        master: false,
    };
    /// A terminal's master side: what the program in that terminal prints
    /// comes out of it, and what a relay passes on goes in.
    const MASTER: Self = Self {
        watch: Watch::Elsewhere,
        master: true,
    };

    /// The word over both sets of files.
    fn and(self, other: Self) -> Self {
        Self {
            watch: self.watch.max(other.watch),
            master: self.master || other.master,
        }
    }

    /// Would any file still to come change nothing?
    fn settled(self) -> bool {
        self.watch == Watch::Terminal && self.master
    }

    /// The thread these files make: a master waited on beside a terminal is
    /// a relay's, and one waited on alone is a wait on the program behind
    /// it, as on a pipe.
    fn thread(self) -> Thread {
        match self.watch {
            Watch::Terminal if self.master => Thread::Relay,
            Watch::Terminal => Thread::Terminal,
            Watch::Unknown => Thread::Maybe,
            Watch::Elsewhere => Thread::Elsewhere,
        }
    }
}

/// What the epoll instance at `epfd` watches — following an instance inside
/// it `depth` more levels.
fn epoll_watch(fds: &impl Descriptors, terminal: &Terminal, epfd: u64, depth: usize) -> Watched {
    // No instance there — a number misread, or a descriptor closed since.
    if fds.link(epfd).as_deref() != Some(EPOLL_LINK) {
        return Watched::UNKNOWN;
    }
    // Every eventpoll, eventfd, timerfd and signalfd is the one anonymous
    // inode this instance is: a file that is not it is no epoll instance.
    let Some(anonymous) = fds.file(epfd) else {
        return Watched::UNKNOWN;
    };
    // Every file is weighed, however long the list: a line naming a file of
    // its own is settled without a call, and only an anonymous file, or a
    // line from a kernel too old to name one, asks its descriptor.
    let Some(targets) = fds.fdinfo(epfd).and_then(|info| epoll_targets(&info)) else {
        return Watched::UNKNOWN;
    };
    let mut watched = Watched::ELSEWHERE;
    for target in targets.iter().filter(|target| target.reads()) {
        watched = watched.and(match target.file {
            Some(file) if terminal.files.contains(&file) => Watched::TERMINAL,
            Some(file) if terminal.masters.contains(&file) => Watched::MASTER,
            Some(file) if Some(file.dev) == terminal.slaves => Watched::TERMINAL,
            Some(file) if file != anonymous => Watched::ELSEWHERE,
            _ => target_watch(fds, terminal, target, depth),
        });
        if watched.settled() {
            break;
        }
    }
    watched
}

/// What a watched file is, asked of its descriptor: an anonymous file —
/// an epoll instance of its own, perhaps — or one on a kernel too old to
/// name its files.
fn target_watch(
    fds: &impl Descriptors,
    terminal: &Terminal,
    target: &EpollTarget,
    depth: usize,
) -> Watched {
    // A number closed or reused since hides what it named.
    if target.file.is_some() && fds.file(target.fd) != target.file {
        return Watched::UNKNOWN;
    }
    descriptor_watch(fds, terminal, target.fd, depth)
}

/// What descriptor `fd` is to a wait, asked of where it leads: a terminal —
/// the session's, or another — a terminal's master side, an epoll instance
/// looked into `depth` more levels, or anything else.
fn descriptor_watch(fds: &impl Descriptors, terminal: &Terminal, fd: u64, depth: usize) -> Watched {
    match fds.link(fd) {
        None => Watched::UNKNOWN,
        Some(link) if link == EPOLL_LINK => match depth.checked_sub(1) {
            Some(depth) => epoll_watch(fds, terminal, fd, depth),
            None => Watched::UNKNOWN,
        },
        Some(link) if terminal.leads_here(&link) || is_terminal_slave(&link) => Watched::TERMINAL,
        Some(link) if is_terminal_master(&link) => Watched::MASTER,
        Some(_) => Watched::ELSEWHERE,
    }
}

/// What the descriptors a `poll` or a `select` waits to read are.
fn list_watch(fds: &impl Descriptors, terminal: &Terminal, inputs: &[u64]) -> Watched {
    let mut watched = Watched::ELSEWHERE;
    for &fd in inputs {
        watched = watched.and(descriptor_watch(fds, terminal, fd, EPOLL_MAX_DEPTH));
        if watched.settled() {
            break;
        }
    }
    watched
}

/// What a `poll` over the `count` entries at `at` waits to read: its array,
/// read out of the process's memory where its arguments say it is.
fn poll_watch(fds: &impl Descriptors, terminal: &Terminal, at: u64, count: u64) -> Watched {
    let Some(count) = usize::try_from(count)
        .ok()
        .filter(|&count| count <= WAIT_LIST_MAX)
    else {
        return Watched::UNKNOWN;
    };
    match fds.memory(at, count * POLLFD_SIZE) {
        Some(array) => list_watch(fds, terminal, &poll_inputs(&array)),
        None => Watched::UNKNOWN,
    }
}

/// What a `select` over the descriptors below `count` waits to read: its
/// read set, read out of the process's memory — none, and it waits to read
/// nothing.
fn select_watch(fds: &impl Descriptors, terminal: &Terminal, count: u64, read_set: u64) -> Watched {
    if read_set == 0 {
        return Watched::ELSEWHERE;
    }
    let Some(bits) = usize::try_from(count)
        .ok()
        .filter(|&bits| bits <= WAIT_LIST_MAX)
    else {
        return Watched::UNKNOWN;
    };
    match fds.memory(read_set, bits.div_ceil(64) * FD_SET_WORD) {
        Some(set) => list_watch(fds, terminal, &select_inputs(&set, count)),
        None => Watched::UNKNOWN,
    }
}

/// Does `link` name a terminal's slave side, `/dev/pts/N` — the session's
/// kind of terminal, as the program behind a relay has it?
fn is_terminal_slave(link: &str) -> bool {
    link.strip_prefix("/dev/pts/")
        .is_some_and(|name| !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Does `link` name a terminal's master side — `/dev/ptmx`, or
/// `/dev/pts/ptmx`, where a container's `/dev/ptmx` leads?
fn is_terminal_master(link: &str) -> bool {
    link == "/dev/ptmx" || link == "/dev/pts/ptmx"
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
    } else if threads
        .iter()
        .any(|thread| matches!(thread, Thread::Terminal | Thread::Relay | Thread::Maybe))
    {
        Probe::Polling
    } else if threads.contains(&Thread::Elsewhere) {
        Probe::Elsewhere
    } else {
        Probe::Idle
    }
}

/// When a process started and the CPU it has used, off its `/proc/PID/stat`
/// line, in clock ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessClock {
    /// When it started — what tells a pid taken by a new process since from
    /// the one that held it.
    pub start: u64,
    /// User plus system time.
    pub cpu: u64,
}

/// One process of a session's tree as a probe found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSample {
    pub pid: u32,
    /// The process it was found under — `None` for the tree's root.
    pub parent: Option<u32>,
    /// Its clock — `None` when its stat line could not be read.
    pub clock: Option<ProcessClock>,
    /// What each of its threads is blocked in.
    pub threads: Vec<Thread>,
}

/// The CPU each process had used when a probe looked — what the next probe
/// measures work against ([`verdict`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Work {
    /// CPU used, by pid and start time.
    cpu: std::collections::HashMap<(u32, u64), u64>,
}

impl Work {
    /// What `samples` says each process had used.
    #[must_use]
    pub fn of(samples: &[ProcessSample]) -> Self {
        let cpu = samples
            .iter()
            .filter_map(|sample| {
                let clock = sample.clock?;
                Some(((sample.pid, clock.start), clock.cpu))
            })
            .collect();
        Self { cpu }
    }

    /// Has process `pid`, as `clock` finds it now, been at work since this
    /// reading? One the reading saw, if it used the CPU since; one it did not
    /// see — started since, or a pid taken by a new process since — at once:
    /// work begun, whatever it has used so far, since the linker rustc hands
    /// its objects to has not used its first tick in its first ten
    /// milliseconds. With no reading at all, nothing is new and nothing has
    /// grown.
    fn worked(&self, pid: u32, clock: ProcessClock) -> bool {
        match self.cpu.get(&(pid, clock.start)) {
            Some(&then) => clock.cpu > then,
            None => !self.cpu.is_empty(),
        }
    }
}

/// The verdict over a whole tree: [`combine`] over every thread, except that
/// a process with a descendant at work — one that used the CPU since
/// `before` — counts as work every wait of its that no prompt can be in. A
/// parent waiting on the output of a child that is compiling is part of that
/// work: cargo polls rustc's pipes, npm's event loop waits on webpack's —
/// waits on no terminal ([`Thread::Elsewhere`]). So is a relay's wait
/// ([`Thread::Relay`]): the program it passes keys to has a terminal of its
/// own, where its prompt, if it has one, is seen. A wait on a terminal is
/// never work, whatever the children do — bash at its prompt over a job in
/// the background, a REPL over the child it started — nor is one the probe
/// could not see into, nor a read on the terminal. Only a process *above* the
/// one at work counts: a program whose own thread works, or one beside a busy
/// sibling, waits on nothing of theirs.
#[must_use]
pub fn verdict(samples: &[ProcessSample], before: &Work, blind: bool) -> Probe {
    let parents: std::collections::HashMap<u32, Option<u32>> = samples
        .iter()
        .map(|sample| (sample.pid, sample.parent))
        .collect();
    let mut watching = std::collections::HashSet::new();
    for sample in samples {
        if !sample
            .clock
            .is_some_and(|clock| before.worked(sample.pid, clock))
        {
            continue;
        }
        let mut above = sample.parent;
        // An ancestor marked already has its own ancestors marked too.
        while let Some(pid) = above
            && watching.insert(pid)
        {
            above = parents.get(&pid).copied().flatten();
        }
    }
    let threads: Vec<Thread> = samples
        .iter()
        .flat_map(|sample| {
            let watching = watching.contains(&sample.pid);
            sample.threads.iter().map(move |&thread| match thread {
                Thread::Elsewhere | Thread::Relay if watching => Thread::Busy,
                other => other,
            })
        })
        .collect();
    combine(&threads, blind)
}

/// A process's clock off its `/proc/PID/stat` line — fields 14 and 15 (user
/// and system time) and 22 (the start time), counted after the parenthesised
/// name, which may hold anything.
#[must_use]
pub fn stat_clock(stat: &str) -> Option<ProcessClock> {
    let (_, fields) = stat.rsplit_once(')')?;
    let fields: Vec<&str> = fields.split_whitespace().collect();
    let field = |n: usize| fields.get(n - 3)?.parse::<u64>().ok();
    Some(ProcessClock {
        start: field(22)?,
        cpu: field(14)?.checked_add(field(15)?)?,
    })
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
/// respect to the terminal at `terminal` (its `/dev/pts/N` path). `work` is
/// what the last probe read of each process's CPU, which this one measures
/// against and then replaces ([`verdict`]).
#[cfg(target_os = "linux")]
#[must_use]
pub fn probe(pid: u32, terminal: &Path, work: &mut Work) -> Probe {
    if Arch::HOST == Arch::Other {
        return Probe::Unknown;
    }
    let terminal = Terminal::of(terminal);
    let mut samples = Vec::new();
    let mut blind = false;
    let mut todo = vec![(pid, None)];
    let mut seen = 0usize;
    while let Some((pid, parent)) = todo.pop() {
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            continue; // gone since it was listed
        };
        let clock = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| stat_clock(&stat));
        let mut threads = Vec::new();
        for task in tasks.flatten() {
            seen += 1;
            if seen > PROBE_MAX_TASKS {
                return Probe::Unknown;
            }
            let dir = task.path();
            match std::fs::read_to_string(dir.join("syscall")) {
                // Taken as read, though the thread may leave the wait while
                // its descriptors are looked at: the next probe sees where it
                // went. Doubting a wait that moved would make a busy event
                // loop, in and out of its wait all the time, a possible
                // prompt again and again.
                Ok(line) => {
                    let fds = TaskDescriptors { dir: &dir };
                    threads.push(classify(Arch::HOST, &line, &fds, &terminal));
                }
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
                        .filter_map(|child| child.parse::<u32>().ok())
                        .map(|child| (child, Some(pid))),
                ),
                Err(_) => blind = true,
            }
        }
        samples.push(ProcessSample {
            pid,
            parent,
            clock,
            threads,
        });
    }
    let verdict = verdict(&samples, work, blind);
    *work = Work::of(&samples);
    verdict
}

/// A thread's descriptors, read off its `/proc/PID/task/TID` directory.
#[cfg(target_os = "linux")]
struct TaskDescriptors<'a> {
    dir: &'a Path,
}

#[cfg(target_os = "linux")]
impl Descriptors for TaskDescriptors<'_> {
    fn wait_channel(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("wchan")).ok()
    }

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

    fn memory(&self, address: u64, len: usize) -> Option<Vec<u8>> {
        use std::os::unix::fs::FileExt;
        let memory = std::fs::File::open(self.dir.join("mem")).ok()?;
        let mut bytes = vec![0; len];
        memory.read_exact_at(&mut bytes, address).ok()?;
        Some(bytes)
    }
}

/// Elsewhere there is no `/proc` to ask: the other tells carry the verdict.
#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn probe(_pid: u32, _terminal: &Path, _work: &mut Work) -> Probe {
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

    /// cargo's stat line, seen live mid-build: 150 ticks of user time and 30
    /// of system time used, started 8 765 432 ticks after boot.
    const CARGO_STAT: &str = "397 (cargo) S 396 397 1 34816 397 4194560 12345 0 0 0 150 \
        30 0 0 20 0 5 0 8765432 123456789 5000 18446744073709551615";

    #[test]
    fn a_stat_line_gives_the_start_time_and_the_cpu_used() {
        assert_eq!(
            stat_clock(CARGO_STAT),
            Some(ProcessClock {
                start: 8_765_432,
                cpu: 180
            })
        );
        let odd = CARGO_STAT.replace("(cargo)", "(a (b) c)");
        assert_eq!(
            stat_clock(&odd),
            stat_clock(CARGO_STAT),
            "a name may hold spaces and parentheses"
        );
        assert_eq!(stat_clock("397 (cargo) S 396 397"), None, "cut short");
        assert_eq!(stat_clock("garbage"), None);
    }

    /// A process of a sampled tree: its pid, the one it hangs under, its
    /// start time and CPU used so far, and what its threads are blocked in.
    fn sample(pid: u32, parent: Option<u32>, cpu: u64, threads: &[Thread]) -> ProcessSample {
        ProcessSample {
            pid,
            parent,
            clock: Some(ProcessClock {
                start: u64::from(pid) * 10,
                cpu,
            }),
            threads: threads.to_vec(),
        }
    }

    #[test]
    fn a_wait_on_a_working_child_is_work() {
        // `cargo build`, seen live: bash waits for cargo, two of cargo's
        // threads poll rustc's stdout and stderr, and rustc compiles in
        // silence. Alone, a poll on two pipes says only that cargo waits
        // elsewhere — a network client's wait, as far as it goes — but a
        // child that used the CPU since the last probe is what it waits on.
        let build = |rustc_cpu| {
            vec![
                sample(1, None, 5, &[Thread::Busy]),
                sample(
                    2,
                    Some(1),
                    300,
                    &[Thread::Elsewhere, Thread::Elsewhere, Thread::Busy],
                ),
                sample(3, Some(2), rustc_cpu, &[Thread::Busy, Thread::Busy]),
            ]
        };
        let before = Work::of(&build(900));
        assert_eq!(verdict(&build(920), &before, false), Probe::Idle);
        assert_eq!(
            verdict(&build(900), &before, false),
            Probe::Elsewhere,
            "a child alive but idle proves nothing"
        );
        assert_eq!(
            verdict(&build(920), &Work::default(), false),
            Probe::Elsewhere,
            "nothing to measure against yet"
        );
    }

    #[test]
    fn an_event_loop_over_a_working_grandchild_is_work() {
        // `npm run build`: npm's event loop waits on sh's pipes, sh on
        // webpack, which works.
        let build = |webpack_cpu| {
            vec![
                sample(1, None, 40, &[Thread::Elsewhere, Thread::Busy]),
                sample(2, Some(1), 1, &[Thread::Busy]),
                sample(3, Some(2), webpack_cpu, &[Thread::Busy]),
            ]
        };
        let before = Work::of(&build(650));
        assert_eq!(verdict(&build(700), &before, false), Probe::Idle);
        assert_eq!(verdict(&build(650), &before, false), Probe::Elsewhere);
    }

    #[test]
    fn only_a_process_above_the_working_one_counts_its_waits_as_work() {
        // An event loop beside a busy sibling, and one whose own thread
        // works: neither waits on a working child of its own.
        let siblings = |cpu| {
            vec![
                sample(1, None, 5, &[Thread::Busy]),
                sample(2, Some(1), 50, &[Thread::Elsewhere]),
                sample(3, Some(1), cpu, &[Thread::Busy]),
            ]
        };
        let before = Work::of(&siblings(100));
        assert_eq!(verdict(&siblings(150), &before, false), Probe::Elsewhere);
        let alone = |cpu| vec![sample(1, None, cpu, &[Thread::Elsewhere, Thread::Busy])];
        assert_eq!(
            verdict(&alone(150), &Work::of(&alone(100)), false),
            Probe::Elsewhere,
            "its own work is no child's"
        );
    }

    #[test]
    fn a_process_new_since_the_last_probe_is_work_begun() {
        // rustc hands its objects to the linker, seen live at the end of a
        // build: rustc polls the linker's stdout and stderr, cc waits for
        // ld, and ld, just started, has not used its first tick yet. A
        // process the last probe did not see was started since — work
        // begun, whatever it has used so far. Counting only what it had used
        // found nothing at work in that one probe, and a build silent for
        // ten seconds was handed back at its link.
        let before = Work::of(&[
            sample(1, None, 5, &[Thread::Busy]),
            sample(2, Some(1), 300, &[Thread::Elsewhere, Thread::Elsewhere]),
            sample(3, Some(2), 900, &[Thread::Busy]),
        ]);
        let linking = |ld_cpu| {
            vec![
                sample(1, None, 5, &[Thread::Busy]),
                sample(2, Some(1), 300, &[Thread::Elsewhere, Thread::Elsewhere]),
                sample(3, Some(2), 900, &[Thread::Elsewhere]),
                sample(4, Some(3), 0, &[Thread::Busy]),
                sample(5, Some(4), ld_cpu, &[Thread::Busy]),
            ]
        };
        assert_eq!(verdict(&linking(30), &before, false), Probe::Idle);
        assert_eq!(
            verdict(&linking(0), &before, false),
            Probe::Idle,
            "before its first tick"
        );
        assert_eq!(
            verdict(&linking(0), &Work::of(&linking(0)), false),
            Probe::Elsewhere,
            "one seen before that has used nothing since proves nothing"
        );
        assert_eq!(
            verdict(&linking(30), &Work::default(), false),
            Probe::Elsewhere,
            "with no reading at all, nothing is new"
        );
        // A pid taken by a new process is a new process: it is measured by
        // what it has used, never against the old one's CPU.
        let mut reused = sample(3, Some(2), 700, &[Thread::Busy]);
        reused.clock = Some(ProcessClock {
            start: 999,
            cpu: 700,
        });
        let now = [
            sample(1, None, 5, &[Thread::Busy]),
            sample(2, Some(1), 300, &[Thread::Elsewhere]),
            reused,
        ];
        assert_eq!(verdict(&now, &before, false), Probe::Idle);
    }

    #[test]
    fn a_reader_stays_a_reader_and_a_blind_probe_blind() {
        let tree = |first: Thread, cpu| {
            vec![
                sample(1, None, 5, &[first]),
                sample(2, Some(1), cpu, &[Thread::Busy]),
            ]
        };
        let before = Work::of(&tree(Thread::Reading, 50));
        assert_eq!(
            verdict(&tree(Thread::Reading, 60), &before, false),
            Probe::Reading,
            "a read on the terminal is a question, whatever its children do"
        );
        assert_eq!(
            verdict(&tree(Thread::Maybe, 60), &before, true),
            Probe::Unknown,
            "a process the probe could not see may be the one asking"
        );
    }

    #[test]
    fn a_wait_on_a_terminal_is_never_work_whatever_the_children_do() {
        // bash at its prompt over a background job at work, seen live — and
        // Node's REPL over the child it started, beside the idle scheduler
        // thread every Node process has: the prompt is theirs, the work the
        // child's.
        let bash = |job_cpu| {
            vec![
                sample(1, None, 5, &[Thread::Terminal]),
                sample(2, Some(1), job_cpu, &[Thread::Busy]),
            ]
        };
        let before = Work::of(&bash(100));
        assert_eq!(verdict(&bash(140), &before, false), Probe::Polling);
        let node = |child_cpu| {
            vec![
                sample(1, None, 60, &[Thread::Terminal, Thread::Elsewhere]),
                sample(2, Some(1), child_cpu, &[Thread::Busy]),
            ]
        };
        let before = Work::of(&node(100));
        assert_eq!(verdict(&node(140), &before, false), Probe::Polling);
    }

    #[test]
    fn a_relay_over_a_program_at_work_is_at_work() {
        // `script` running a build, seen live: it waits on the terminal and
        // the master of its own; below it a shell waits on the build, which
        // works.
        let relayed = |cpu| {
            vec![
                sample(1, None, 2, &[Thread::Relay]),
                sample(2, Some(1), 1, &[Thread::Busy]),
                sample(3, Some(2), cpu, &[Thread::Busy]),
            ]
        };
        let before = Work::of(&relayed(400));
        assert_eq!(verdict(&relayed(440), &before, false), Probe::Idle);
        assert_eq!(
            verdict(&relayed(400), &before, false),
            Probe::Polling,
            "a relay over nothing at work may be over a prompt"
        );
        // A prompt behind the relay stays a prompt: bash at its prompt in
        // `script`, over a background job at work.
        let prompt = |cpu| {
            vec![
                sample(1, None, 2, &[Thread::Relay]),
                sample(2, Some(1), 3, &[Thread::Terminal]),
                sample(3, Some(2), cpu, &[Thread::Busy]),
            ]
        };
        let before = Work::of(&prompt(400));
        assert_eq!(verdict(&prompt(440), &before, false), Probe::Polling);
    }

    #[test]
    fn a_wait_the_probe_could_not_see_into_is_left_as_it_was() {
        // Its array unreadable, the wait may be on the terminal: a child at
        // work does not make it anything else.
        let tree = |cpu| {
            vec![
                sample(1, None, 5, &[Thread::Maybe]),
                sample(2, Some(1), cpu, &[Thread::Busy]),
            ]
        };
        let before = Work::of(&tree(100));
        assert_eq!(verdict(&tree(140), &before, false), Probe::Polling);
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
        let poll = Some(Wait::Poll { at: 3, count: 5 });
        let select = Some(Wait::Select {
            count: 3,
            read_set: 5,
        });
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
    /// A child's stdout and stderr, as a parent reading them holds them.
    const PIPE: FileId = FileId {
        ino: 77,
        dev: (0, 13),
    };
    const PIPE_2: FileId = FileId {
        ino: 78,
        dev: (0, 13),
    };
    /// Another terminal on the session's `devpts` — the one `script` runs
    /// its command in, `/dev/pts/1`.
    const OTHER_PTS: FileId = FileId {
        ino: 4,
        dev: (0, 27),
    };
    /// The master side of a terminal, opened as `/dev/ptmx`.
    const PTMX: FileId = FileId {
        ino: 0x57,
        dev: (0, 6),
    };

    /// A process's descriptors as a table: where each leads, the file it
    /// is, an epoll instance's interest list, and the process's memory where
    /// a `poll` or a `select` keeps its descriptors.
    #[derive(Default)]
    struct Table {
        links: std::collections::HashMap<u64, String>,
        files: std::collections::HashMap<u64, FileId>,
        infos: std::collections::HashMap<u64, String>,
        memory: std::collections::HashMap<u64, Vec<u8>>,
        wait_channel: Option<String>,
    }

    impl Table {
        fn fd(mut self, fd: u64, link: &str, file: FileId) -> Self {
            self.links.insert(fd, link.to_string());
            self.files.insert(fd, file);
            self
        }

        /// `bytes` of the process's memory, at `address`.
        fn memory(mut self, address: u64, bytes: Vec<u8>) -> Self {
            self.memory.insert(address, bytes);
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
        fn wait_channel(&self) -> Option<String> {
            self.wait_channel.clone()
        }

        fn link(&self, fd: u64) -> Option<String> {
            self.links.get(&fd).cloned()
        }

        fn file(&self, fd: u64) -> Option<FileId> {
            self.files.get(&fd).copied()
        }

        fn fdinfo(&self, fd: u64) -> Option<String> {
            self.infos.get(&fd).cloned()
        }

        fn memory(&self, address: u64, len: usize) -> Option<Vec<u8>> {
            let bytes = self.memory.get(&address)?;
            Some(bytes.get(..len)?.to_vec())
        }
    }

    fn terminal() -> Terminal {
        Terminal {
            path: std::path::PathBuf::from("/dev/pts/0"),
            files: vec![PTS, DEV_TTY],
            slaves: Some(PTS.dev),
            masters: vec![PTMX],
        }
    }

    /// A `poll`'s array as the kernel reads it: `int fd; short events;
    /// short revents` for each of `entries`.
    fn pollfds(entries: &[(i32, u16)]) -> Vec<u8> {
        entries
            .iter()
            .flat_map(|&(fd, events)| {
                let mut entry = fd.to_ne_bytes().to_vec();
                entry.extend_from_slice(&events.to_ne_bytes());
                entry.extend_from_slice(&0u16.to_ne_bytes());
                entry
            })
            .collect()
    }

    /// A `select` set of `words` words marking `fds`: bit `fd % 64` of word
    /// `fd / 64`.
    fn fd_set(fds: &[u64], words: usize) -> Vec<u8> {
        let mut set = vec![0u64; words];
        for &fd in fds {
            set[usize::try_from(fd / 64).expect("small")] |= 1 << (fd % 64);
        }
        set.iter().flat_map(|word| word.to_ne_bytes()).collect()
    }

    /// `poll` (x86_64) over the `count` entries of the array at `at`.
    fn poll_on(at: u64, count: u64) -> String {
        format!("7 {at:#x} {count:#x} 0xffffffff 0x0 0x0 0x0 0x7ffd 0x7f")
    }

    /// `pselect6` (x86_64) over the descriptors below `count`, those it waits
    /// to read marked in the set at `read_set`.
    fn select_on(count: u64, read_set: u64) -> String {
        format!("270 {count:#x} {read_set:#x} 0x0 0x0 0x0 0x7ffd 0x7ffd 0x7f")
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
        assert_eq!(classify_x86(&epoll_wait_on(3), &stdin), Thread::Terminal);
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
        assert_eq!(classify_x86(&epoll_wait_on(3), &tty), Thread::Terminal);
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
        assert_eq!(classify_x86(&epoll_wait_on(3), &child), Thread::Terminal);
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
        assert_eq!(classify_x86(&epoll_wait_on(3), &terminal), Thread::Terminal);
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
    fn every_file_of_a_large_interest_list_is_weighed() {
        // A server holding thousands of connections: each line names its
        // file, so weighing them all costs no call of its own. A list too
        // long to weigh was left open, and a prompt-shaped line over it read
        // as waiting for input.
        let sockets: Vec<String> = (10..6010u64)
            .map(|fd| {
                format!(
                    "tfd: {fd:8} events: 8000201d data: {fd:16x}  pos:0 ino:{:x} sdev:9",
                    0x10_0000 + fd
                )
            })
            .collect();
        let mut lines: Vec<&str> = sockets.iter().map(String::as_str).collect();
        let server = process().epoll(3, &lines);
        assert_eq!(classify_x86(&epoll_wait_on(3), &server), Thread::Elsewhere);
        lines.push("tfd:        0 events:       19 data:               0  pos:0 ino:3 sdev:1b");
        let terminal_last = process().epoll(3, &lines);
        assert_eq!(
            classify_x86(&epoll_wait_on(3), &terminal_last),
            Thread::Terminal,
            "the terminal, last of them all"
        );
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
            Thread::Terminal,
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
            "ppoll over two, whose array could not be read"
        );
    }

    #[test]
    fn a_poll_waits_to_read_what_its_array_watches_for_input() {
        // Python's `subprocess.run(capture_output=True)`, seen live: POLLIN on
        // the child's stdout and stderr — beside entries a poll reads
        // nothing from: one watched for room to write alone, and a negative
        // descriptor, which the kernel skips.
        let array = pollfds(&[(3, 0x1), (5, 0x1), (1, 0x4), (-1, 0x1), (7, 0x2), (8, 0x40)]);
        assert_eq!(poll_inputs(&array), vec![3, 5, 7, 8]);
        assert_eq!(poll_inputs(&[]), Vec::<u64>::new());
        assert_eq!(poll_inputs(&array[..12]), vec![3], "a torn entry is none");
    }

    #[test]
    fn a_select_waits_to_read_what_its_read_set_marks() {
        // make waiting for a jobserver token, seen live: `pselect6` over
        // four, its read set marking descriptor 3.
        assert_eq!(select_inputs(&fd_set(&[3], 1), 4), vec![3]);
        let set = fd_set(&[0, 5, 64, 70], 2);
        assert_eq!(select_inputs(&set, 71), vec![0, 5, 64, 70]);
        assert_eq!(
            select_inputs(&set, 70),
            vec![0, 5, 64],
            "only those below the count"
        );
    }

    #[test]
    fn a_poll_on_the_terminal_may_be_a_prompt_and_one_on_pipes_waits_elsewhere() {
        // Python 3.13's REPL at its prompt, seen live, polls the terminal at
        // descriptor 3; cargo polls rustc's stdout and stderr.
        let repl = process()
            .fd(3, "/dev/pts/0", PTS)
            .memory(0x7ffd_0000, pollfds(&[(3, 0x1)]));
        assert_eq!(
            classify_x86(&poll_on(0x7ffd_0000, 1), &repl),
            Thread::Terminal
        );
        let cargo = process()
            .fd(11, "pipe:[77]", PIPE)
            .fd(13, "pipe:[78]", PIPE_2)
            .memory(0x7f00_0000, pollfds(&[(11, 0x1), (13, 0x1)]));
        assert_eq!(
            classify_x86(&poll_on(0x7f00_0000, 2), &cargo),
            Thread::Elsewhere
        );
        let writer = process().memory(0x7f00_1000, pollfds(&[(1, 0x4)]));
        assert_eq!(
            classify_x86(&poll_on(0x7f00_1000, 1), &writer),
            Thread::Elsewhere,
            "the terminal, watched for room to write alone"
        );
    }

    #[test]
    fn a_select_on_the_terminal_may_be_a_prompt_and_one_on_pipes_waits_elsewhere() {
        // bash's readline at its prompt, seen live: `pselect6` over one, the
        // read set marking descriptor 0 — and make on its jobserver pipe.
        let bash = process().memory(0x7ffd_1000, fd_set(&[0], 1));
        assert_eq!(
            classify_x86(&select_on(1, 0x7ffd_1000), &bash),
            Thread::Terminal
        );
        let make = process()
            .fd(3, "pipe:[77]", PIPE)
            .memory(0x7ffd_2000, fd_set(&[3], 1));
        assert_eq!(
            classify_x86(&select_on(4, 0x7ffd_2000), &make),
            Thread::Elsewhere
        );
        assert_eq!(
            classify_x86(&select_on(4, 0), &make),
            Thread::Elsewhere,
            "no read set: waiting to read nothing"
        );
    }

    #[test]
    fn a_wait_on_the_terminal_and_a_master_is_a_relay() {
        // `script`, seen live: a poll on its signalfd, the master of the
        // terminal it runs its command in, and the session's terminal.
        let script = process()
            .fd(3, "/dev/ptmx", PTMX)
            .fd(5, "anon_inode:[signalfd]", ANON)
            .memory(0x7ffd_3000, pollfds(&[(5, 0x1), (3, 0x1), (0, 0x1)]));
        assert_eq!(
            classify_x86(&poll_on(0x7ffd_3000, 3), &script),
            Thread::Relay
        );
        // sudo's own terminal, seen live: a socket, the master, and the
        // session's terminal as /dev/tty.
        let sudo = Table::default()
            .fd(7, "/dev/tty", DEV_TTY)
            .fd(8, "/dev/ptmx", PTMX)
            .fd(10, "socket:[9]", SOCKET)
            .memory(0x7ffd_4000, pollfds(&[(10, 0x1), (8, 0x1), (7, 0x1)]));
        assert_eq!(classify_x86(&poll_on(0x7ffd_4000, 3), &sudo), Thread::Relay);
        // A master alone is a wait on the program behind it, like a pipe.
        let unbuffer = Table::default()
            .fd(3, "/dev/ptmx", PTMX)
            .memory(0x7ffd_5000, pollfds(&[(3, 0x1)]));
        assert_eq!(
            classify_x86(&poll_on(0x7ffd_5000, 1), &unbuffer),
            Thread::Elsewhere
        );
        // An event loop's relay: its instance watches the terminal and a
        // master, named by their files.
        let relay = process().epoll(
            3,
            &[
                "tfd:        0 events:       19 data:               0  pos:0 ino:3 sdev:1b",
                "tfd:        6 events:       19 data:               6  pos:0 ino:57 sdev:6",
            ],
        );
        assert_eq!(classify_x86(&epoll_wait_on(3), &relay), Thread::Relay);
    }

    #[test]
    fn a_wait_on_another_terminal_may_be_a_prompt_behind_a_relay() {
        // bash at its prompt inside `script`, seen live: readline's select
        // on the terminal `script` gave it.
        let inner = Table::default()
            .fd(0, "/dev/pts/1", OTHER_PTS)
            .memory(0x7ffd_6000, fd_set(&[0], 1));
        assert_eq!(
            classify_x86(&select_on(1, 0x7ffd_6000), &inner),
            Thread::Terminal
        );
        assert_eq!(
            classify_x86("0 0x0 0x7ffd 0x1 0x0 0x0 0x0 0x7ffd 0x7f2d", &inner),
            Thread::Terminal,
            "a read on it"
        );
        // Node's REPL behind a relay: an epoll naming that terminal's file.
        let node = Table::default().epoll(
            3,
            &["tfd:        0 events:       19 data:               0  pos:0 ino:4 sdev:1b"],
        );
        assert_eq!(classify_x86(&epoll_wait_on(3), &node), Thread::Terminal);
    }

    #[test]
    fn a_poll_the_probe_cannot_read_stays_open() {
        let unread = process();
        assert_eq!(
            classify_x86(&poll_on(0x7ffd_7000, 1), &unread),
            Thread::Maybe,
            "no memory to read"
        );
        let gone = process().memory(0x7ffd_7000, pollfds(&[(9, 0x1)]));
        assert_eq!(
            classify_x86(&poll_on(0x7ffd_7000, 1), &gone),
            Thread::Maybe,
            "a descriptor gone since"
        );
        let torn = process().memory(0x7ffd_7000, pollfds(&[(0, 0x1)])[..4].to_vec());
        assert_eq!(
            classify_x86(&poll_on(0x7ffd_7000, 1), &torn),
            Thread::Maybe,
            "less memory than the array"
        );
        let many = WAIT_LIST_MAX + 1;
        let long = process().memory(0x7ffd_8000, pollfds(&vec![(1, 0x4); many]));
        assert_eq!(
            classify_x86(&poll_on(0x7ffd_8000, many as u64), &long),
            Thread::Maybe,
            "more descriptors than the probe looks up"
        );
    }

    #[test]
    fn a_terminal_splice_waits_for_input_only_on_its_read_side() {
        // Modern cat splices stdin into an internal pipe before copying it
        // out. The same syscall can instead block on a full output pipe.
        let splice = |number, input| {
            let output = if input == 0 { 4 } else { 0 };
            format!("{number} {input:#x} 0x0 {output:#x} 0x0 0x80000 0x0 0x7ffd 0x7f2d")
        };
        for (arch, number) in [(Arch::X86_64, 275), (Arch::Aarch64, 76)] {
            for (channel, want) in [
                (Some("wait_woken"), Thread::Reading),
                (Some("n_tty_read"), Thread::Reading),
                (Some("pipe_wait_writable"), Thread::Busy),
                (Some("0"), Thread::Maybe),
                (None, Thread::Maybe),
            ] {
                let mut fds = process().fd(4, "pipe:[77]", PIPE);
                fds.wait_channel = channel.map(str::to_string);
                assert_eq!(
                    classify(arch, &splice(number, 0), &fds, &terminal()),
                    want,
                    "{arch:?}, {channel:?}"
                );
                assert_eq!(
                    classify(arch, &splice(number, 4), &fds, &terminal()),
                    Thread::Busy,
                    "a splice from a pipe is not terminal input"
                );
            }
        }
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
            "a wait the probe could not see into may be on the terminal"
        );
        assert_eq!(
            combine(&[Thread::Busy, Thread::Terminal], false),
            Probe::Polling,
            "a wait on a terminal"
        );
        assert_eq!(
            combine(&[Thread::Busy, Thread::Relay], false),
            Probe::Polling,
            "a relay: the program behind it may be at its prompt"
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
            last = probe(session.child.id(), &path, &mut Work::default());
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
    fn a_live_event_loop_waking_every_millisecond_never_polls() {
        // A busy event loop — a download landing, a timer firing — leaves
        // its wait and comes back hundreds of times a second, with a new
        // timeout each time. Wherever a probe catches it, it is at work or
        // waiting on its own set, never on the terminal: a possible prompt
        // (`Probe::Polling`) here is what read a silent download behind an
        // open line as waiting for input.
        use crate::pty::spawn::{spawn, terminal_path};
        use std::time::{Duration, Instant};
        if !on_path("python3") {
            eprintln!("skipped, no python3");
            return;
        }
        let mut session = spawn(
            None,
            "python3 -c 'import asyncio\nasync def tick():\n    while True:\n        \
             await asyncio.sleep(0.001)\nasyncio.run(tick())'",
        )
        .expect("spawns");
        let path = terminal_path(&session.master).expect("the terminal has a path");
        let pid = session.child.id();
        let started = Instant::now();
        let mut work = Work::default();
        let mut verdicts = Vec::new();
        while started.elapsed() < Duration::from_secs(5) && !verdicts.contains(&Probe::Elsewhere) {
            verdicts.push(probe(pid, &path, &mut work));
            std::thread::sleep(Duration::from_millis(20));
        }
        let looping = Instant::now();
        while looping.elapsed() < Duration::from_secs(2) {
            verdicts.push(probe(pid, &path, &mut work));
            std::thread::sleep(Duration::from_millis(2));
        }
        crate::subprocess::kill_process_group(&mut session.child);
        assert!(verdicts.contains(&Probe::Elsewhere), "never seen waiting");
        let polls = verdicts.iter().filter(|&&v| v == Probe::Polling).count();
        assert_eq!(
            polls,
            0,
            "{polls} of {} probes said Polling",
            verdicts.len()
        );
    }

    /// Five verdicts on `command`, run in a terminal of its own with `input`
    /// typed into it, 300 ms apart once its programs have had a second and a
    /// half to start and settle — the reading of their CPU kept between
    /// probes, as the monitor keeps it.
    #[cfg(target_os = "linux")]
    fn settled_verdicts(command: &str, input: &str) -> Vec<Probe> {
        use crate::pty::spawn::{spawn, terminal_path};
        use std::io::Write;
        use std::time::{Duration, Instant};
        let mut session = spawn(None, command).expect("spawns");
        let path = terminal_path(&session.master).expect("the terminal has a path");
        let pid = session.child.id();
        (&session.master)
            .write_all(input.as_bytes())
            .expect("types");
        let mut work = Work::default();
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(1500) {
            let _ = probe(pid, &path, &mut work);
            std::thread::sleep(Duration::from_millis(300));
        }
        let seen = (0..5)
            .map(|_| {
                std::thread::sleep(Duration::from_millis(300));
                probe(pid, &path, &mut work)
            })
            .collect();
        crate::subprocess::kill_process_group(&mut session.child);
        seen
    }

    /// A Python one-liner computing for ten seconds — bounded, since a job
    /// in a process group of its own outlives the session's group kill.
    #[cfg(target_os = "linux")]
    const COMPUTES: &str = "import time\nt = time.time()\nwhile time.time() - t < 10: pass";

    #[cfg(target_os = "linux")]
    #[test]
    fn a_live_prompt_over_a_working_child_is_still_a_prompt() {
        // A REPL, reduced: a program that started a child which computes,
        // then waits in `select` on the terminal for what is typed next.
        // The child's work is not the prompt's.
        if !on_path("python3") {
            eprintln!("skipped, no python3");
            return;
        }
        let command = format!(
            "python3 -c 'import select, subprocess, sys\n\
             subprocess.Popen([sys.executable, \"-c\", \"{}\"])\n\
             select.select([sys.stdin], [], [])'",
            COMPUTES.replace('\n', "\\n")
        );
        assert_eq!(settled_verdicts(&command, ""), [Probe::Polling; 5]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_live_shell_prompt_over_a_job_at_work_is_still_a_prompt() {
        // bash's readline back at its prompt while a job computes in the
        // background.
        if !on_path("python3") || !on_path("timeout") {
            eprintln!("skipped, no python3 or timeout");
            return;
        }
        assert_eq!(
            settled_verdicts(
                "bash --norc --noprofile -i",
                "timeout 10 python3 -c 'while True: pass' &\n"
            ),
            [Probe::Polling; 5]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_live_relay_over_a_program_at_work_is_at_work_and_one_over_a_prompt_is_not() {
        // `script` running a command in a terminal of its own: over one that
        // computes it is at work; over a shell reading that terminal, with a
        // job computing beside the read, the prompt behind it stands.
        if !on_path("script") || !on_path("python3") || !on_path("timeout") {
            eprintln!("skipped, no script, python3 or timeout");
            return;
        }
        assert_eq!(
            settled_verdicts(
                r#"script -qfc "timeout 10 python3 -c 'while True: pass'" /dev/null"#,
                ""
            ),
            [Probe::Idle; 5]
        );
        assert_eq!(
            settled_verdicts(
                r#"script -qfc "bash --norc --noprofile -c 'timeout 10 python3 -c \"while True: pass\" & read x'" /dev/null"#,
                ""
            ),
            [Probe::Polling; 5]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_live_parent_waiting_on_a_working_child_is_at_work() {
        // cargo, reduced: a parent polling a child's stdout and stderr — the
        // way cargo reads rustc's — while the child computes, and the same
        // parent over a child that only sleeps.
        if !on_path("python3") {
            eprintln!("skipped, no python3");
            return;
        }
        let verdicts = |child: &str| {
            settled_verdicts(
                &format!(
                    "python3 -c 'import subprocess; subprocess.run([\"python3\", \"-c\", \
                     \"{child}\"], capture_output=True)'"
                ),
                "",
            )
        };
        assert_eq!(verdicts("while True: pass"), [Probe::Idle; 5]);
        assert_eq!(
            verdicts("import time; time.sleep(30)"),
            [Probe::Elsewhere; 5],
            "a child alive but asleep proves nothing: the parent's poll, read \
             off its memory, is on two pipes and no terminal"
        );
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
