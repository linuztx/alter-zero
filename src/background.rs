//! Background shell processes — the boundary registry behind
//! `run_in_background`, Ctrl+B, and the ↓ manager (see `docs/background.md`).
//!
//! A [`BackgroundRegistry`] is a cloneable handle shared by the event loop,
//! the model-tool executor ([`crate::llm::exec`]), and the `!` shell runner
//! (`main.rs`). Launching (or adopting, for Ctrl+B) a command spawns a
//! **monitor thread** that owns the child: it merges the stdout/stderr pipes
//! in arrival order, streams completed lines — folded the way a terminal
//! would show them ([`crate::pty::fold`]) — as [`BgEvent::Output`], tees
//! every byte to the task's `{id}.output` file under the session's `tasks`
//! directory ([`crate::scratchpad::tasks_dir`], so the model can `read` interim
//! output), and reports [`BgEvent::Exited`] when the child dies.
//! Events ride their own tokio channel — a dedicated `select!` source —
//! because background shells outlive turns: the reply channel is swapped on
//! every interrupt/`/clear`, and these events must survive that.
//!
//! Boundary code like `term.rs`: the process/file I/O is verified by the
//! process-spawning tests below (real `sh`, like `llm::exec`'s) and
//! `scripts/smoke.sh`; the pure state it feeds lives in [`crate::app`].

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::pty::keys::InputChunk;
use crate::pty::session::{Finish, SessionIo};
use crate::stream::CancelToken;

/// Where a background shell was launched from, when not the main
/// conversation: the launching **subagent**'s registry id (so the boundary
/// can route the completion note back into that agent's running loop) and
/// its type label (for display — the manager's `From:` field and the notice
/// cell's ` · from the {type} agent` suffix). `None` everywhere = the main
/// conversation, the pre-feature shape. See `docs/background.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgOrigin {
    /// The launching subagent's registry id (`a7k2m9x4q`).
    pub agent_id: String,
    /// Its subagent type (`general-purpose`, `explore`).
    pub agent_type: String,
}

/// What the registry reports to the event loop about its shells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BgEvent {
    /// A shell was launched (or adopted via Ctrl+B) and is now running.
    Started {
        id: String,
        command: String,
        /// The model-supplied `description` argument, if any.
        description: Option<String>,
        /// Whether the model launched it — its completion auto-starts a
        /// follow-up turn (`docs/background.md`).
        from_model: bool,
        /// The launching subagent, when one did (`None` = the main
        /// conversation).
        origin: Option<BgOrigin>,
    },
    /// A running shell's completed lines — folded the way a terminal shows
    /// them, so a `\r` progress bar arrives once, finished, with no escapes
    /// (`docs/interactive-shell.md`); the line a command left unfinished
    /// arrives at its exit.
    Output { id: String, chunk: String },
    /// A **TTY session's** screen as it now stands — the whole of it, not a
    /// delta: the ↓ manager shows what a terminal would
    /// (`docs/interactive-shell.md`). Sent throttled while output flows, and
    /// once more when it goes quiet.
    Screen { id: String, text: String },
    /// The shell exited: its `code` (`None` when a signal killed it),
    /// whether the registry's own [`BackgroundRegistry::kill`] did it, and
    /// whether the model already **observed** the exit — a `bash_session`
    /// call reported it — in which case no completion notice is owed
    /// (`docs/interactive-shell.md`).
    Exited {
        id: String,
        code: Option<i32>,
        killed: bool,
        observed: bool,
    },
    /// A TTY session nobody is waiting on stopped to **ask for input** it
    /// printed since the model last looked — quiet for
    /// [`WAITING_NOTICE_QUIET`] at a prompt — and runs on
    /// (`docs/bash-tools.md`). Sent once, and not again until the model has
    /// looked at the session ([`SessionIo::take_unseen_prompt`]).
    Waiting { id: String },
}

/// A successfully launched background task, for the model-facing tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchedTask {
    /// The task id (`bvyo7tkbe`, …) — internal only; the launch text names
    /// the shell by its interim-output path instead.
    pub id: String,
    /// Where the interim output streams — the model can `read` it mid-run.
    pub output_path: PathBuf,
}

/// A launched TTY session: the task, and the session state its launching
/// call waits on — handed over directly, so the call never has to find it in
/// the registry (where a command that already exited may be gone).
#[derive(Debug)]
pub struct TtyLaunch {
    pub task: LaunchedTask,
    pub io: Arc<SessionIo>,
}

/// Why [`BackgroundRegistry::launch_tty`] started no session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TtyLaunchError {
    /// No terminal could be given to the command here — off Unix, or neither
    /// `setsid` nor the helper can make one its controlling terminal. The
    /// caller runs it on a pipe instead (`docs/bash-tools.md`).
    NoTerminal(String),
    /// A session could have been started and was not: the model-facing
    /// reason (too many already running, listed).
    Refused(String),
}

/// [`BackgroundRegistry::register`]'s result — the task and its session state.
struct Registered {
    task: LaunchedTask,
    io: Arc<SessionIo>,
}

/// A completed shell's model-facing note on the registry's notice board,
/// waiting to be delivered: the event loop posts one the moment it handles a
/// shell's `Exited` event, and the **in-flight agent** takes the board before
/// each of its rounds — so a `kill`ed server is known to the model within the
/// same turn — while notes still on the board at a turn boundary (the model
/// never saw them) drive the automatic follow-up turn instead. See
/// `docs/background.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingNotice {
    /// The full context note (`BgCompletion::context_text` — the same text
    /// the settled notice replays into every later turn's derived context).
    pub context: String,
    /// Whether the model launched the shell (an untaken note from one is what
    /// warrants the automatic follow-up turn).
    pub from_model: bool,
}

/// The base36 alphabet task ids are drawn from.
const TASK_ID_ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// A Claude-Code-style task id — a `b` prefix + 8 lowercase base36 chars
/// (`bvyo7tkbe`). Deterministic in `seed` (pure, so the format is testable);
/// the registry feeds it fresh entropy per launch and re-rolls the rare
/// collision. See `docs/background.md`.
fn task_id(seed: u64) -> String {
    let mut mixed = splitmix64(seed);
    let mut id = String::with_capacity(9);
    id.push('b');
    for _ in 0..8 {
        id.push(TASK_ID_ALPHABET[(mixed % 36) as usize] as char);
        mixed /= 36;
    }
    id
}

/// Fresh entropy for one id roll: wall-clock nanos ⊕ pid ⊕ the launch
/// counter — the `session_id` pattern (unique-enough, no rand dependency);
/// the counter keeps two rolls inside one clock tick apart, and
/// [`splitmix64`] makes them look unrelated.
fn entropy_seed(counter: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let folded = (nanos as u64) ^ ((nanos >> 64) as u64);
    folded ^ (u64::from(std::process::id()) << 32) ^ counter
}

/// SplitMix64 — the classic one-shot mixer: consecutive seeds (the launch
/// counter) come out looking unrelated, so ids never read as a sequence.
fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// How often a monitor thread wakes to poll its child / kill flag when no
/// output is arriving — short enough that exits and kills surface promptly.
const MONITOR_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How long a background session must sit quiet at a prompt before the
/// model is told it waits ([`BgEvent::Waiting`]) — a pure wait's own
/// patience ([`crate::pty::settle::WAIT_PROMPT_QUIET`]): long enough that a
/// program pausing mid-output is not taken for one asking.
pub const WAITING_NOTICE_QUIET: Duration = crate::pty::settle::WAIT_PROMPT_QUIET;

/// One registered task, as the shared state sees it (the child itself is
/// owned by its monitor thread).
struct Task {
    /// The child's process-group id (== its pid — it leads its own group), so
    /// a kill reaps the whole tree, exactly like `llm::exec`'s group kill.
    pgid: u32,
    /// Tripped by [`BackgroundRegistry::kill`]; the monitor polls it and
    /// kills the group — the portable path beside the direct unix group kill.
    kill: CancelToken,
    /// Set by [`BackgroundRegistry::kill`] before the process dies, so the
    /// monitor reports `killed: true` (a user stop, not a failure).
    killed: bool,
    /// The process has exited — its exit waiting to be finalized by a
    /// `bash_session` call — so no kill may target the (reusable) group id.
    exited: bool,
    /// What the monitor and the model's calls share: both views of the
    /// output, and the exit handshake (`pty::session`).
    io: Arc<SessionIo>,
    /// A TTY session's writer — the only way input reaches its terminal.
    /// `None` for a pipe task, whose stdin is `/dev/null`.
    input: Option<mpsc::Sender<WriteOp>>,
    /// A TTY session's terminal (a handle on its master), kept to read the
    /// program's [`LineMode`](crate::pty::spawn::LineMode) — whether what the
    /// model typed has reached it yet.
    terminal: Option<File>,
    /// The launch facts, kept for an announcement made later (a `tty` call
    /// announces its session only once it outlives the call).
    launch: Launch,
    /// When the task started — how long `bashlist` says it has run.
    started: std::time::Instant,
}

/// What a [`BgEvent::Started`] says about a task.
#[derive(Clone)]
struct Launch {
    command: String,
    description: Option<String>,
    from_model: bool,
    origin: Option<BgOrigin>,
}

/// One write to a TTY session's terminal: bytes as they are (the terminal's
/// query replies), or a call's keys — typed the way a person types them
/// ([`type_keys`]), the session told once they are out
/// ([`SessionIo::typed`]).
#[derive(Debug)]
enum WriteOp {
    Bytes(Vec<u8>),
    Keys {
        chunks: Vec<InputChunk>,
        io: Arc<SessionIo>,
    },
}

/// How often the writer asks whether the program has read the key it typed
/// ([`type_keys`]).
const KEY_READ_POLL: Duration = Duration::from_micros(250);

/// The most TTY sessions that may run at once. One more is refused with the
/// list of the running ones, rather than letting forgotten REPLs pile up
/// unseen (`docs/interactive-shell.md`).
pub const MAX_TTY_SESSIONS: usize = 16;

/// How often, at most, a TTY session's screen is sent to the ↓ manager while
/// output flows — it is re-sent whole each time, so a flood costs ten a
/// second, not one per chunk.
const SCREEN_EVENT_INTERVAL: Duration = Duration::from_millis(100);

/// A running session as the model-tool executor reaches it
/// ([`BackgroundRegistry::session`]).
#[derive(Clone)]
pub struct SessionHandle {
    /// The shared output views and exit handshake.
    pub io: Arc<SessionIo>,
    /// The command it runs — what a permission prompt and an error name.
    pub command: String,
}

impl std::fmt::Debug for SessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionHandle")
            .field("command", &self.command)
            .finish_non_exhaustive()
    }
}

struct Inner {
    /// The launch counter — mixed into each id roll's entropy seed so two
    /// launches inside one clock tick still differ.
    next_id: u64,
    tasks: HashMap<String, Task>,
    /// The Ctrl+B latch: the loop raises it while a foreground command runs;
    /// the runner's poll loop consumes it (see `docs/background.md`).
    background_request: bool,
    /// The completion notice board (see [`PendingNotice`]).
    pending_notices: Vec<PendingNotice>,
    /// Where the `{id}.output` interim files live.
    dir: PathBuf,
}

/// The shared background-shell registry (see the module docs).
#[derive(Clone)]
pub struct BackgroundRegistry {
    inner: Arc<Mutex<Inner>>,
    events: UnboundedSender<BgEvent>,
    /// The terminal-detach helper (`crate::subprocess`), threaded from `main.rs`.
    /// The registry is the shell-infrastructure handle every runner already
    /// shares, so it also carries the helper: [`launch`] spawns through it,
    /// and the executor / `!` runner read it back via [`detach_helper`].
    ///
    /// [`launch`]: BackgroundRegistry::launch
    /// [`detach_helper`]: BackgroundRegistry::detach_helper
    detach: Option<PathBuf>,
    /// Whether a command may be given a terminal of its own — on by default
    /// (`docs/bash-tools.md`); off, every command runs on a pipe
    /// ([`without_terminals`](BackgroundRegistry::without_terminals)).
    terminals: bool,
}

impl std::fmt::Debug for BackgroundRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundRegistry").finish_non_exhaustive()
    }
}

impl BackgroundRegistry {
    /// A registry that reports on `events` and tees output files into `dir`
    /// (created lazily on the first launch; a failure just skips the file).
    #[must_use]
    pub fn new(events: UnboundedSender<BgEvent>, dir: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                next_id: 0,
                tasks: HashMap::new(),
                background_request: false,
                pending_notices: Vec::new(),
                dir,
            })),
            events,
            detach: None,
            terminals: true,
        }
    }

    /// A registry that gives no command a terminal: every command runs on a
    /// pipe, as it did before a terminal was the default
    /// (`docs/bash-tools.md`) — for an embedder that wants that, and the
    /// fallback's own tests. [`launch_tty`](Self::launch_tty) then answers
    /// [`TtyLaunchError::NoTerminal`].
    #[must_use]
    pub fn without_terminals(mut self) -> Self {
        self.terminals = false;
        self
    }

    /// Install the terminal-detach helper — the TUI's own binary
    /// (`crate::subprocess`), the fallback tier that keeps a `/dev/tty`
    /// password prompt (`sudo`) failing fast even where the `setsid` binary
    /// is absent (macOS). Production only (`main.rs` resolves `current_exe`
    /// once at startup); without it the chain is just shorter.
    #[must_use]
    pub fn with_detach_helper(mut self, helper: Option<PathBuf>) -> Self {
        self.detach = helper;
        self
    }

    /// The terminal-detach helper this registry spawns through, for the other
    /// runners (the model-tool executor, the `!` shell) to spawn through too.
    #[must_use]
    pub fn detach_helper(&self) -> Option<PathBuf> {
        self.detach.clone()
    }

    /// Spawn `command` as a background task: the model's shell
    /// ([`crate::subprocess::tool_shell`]) in its own process group
    /// (so a kill reaps the whole tree), monitored on its own thread. Returns
    /// the interim-output path for the model-facing result.
    ///
    /// # Errors
    /// The spawn error text when the shell can't start.
    pub fn launch(
        &self,
        command: &str,
        description: Option<String>,
        from_model: bool,
    ) -> Result<LaunchedTask, String> {
        self.launch_from(command, description, from_model, None)
    }

    /// [`launch`](BackgroundRegistry::launch) with the launching subagent
    /// attributed: the `origin` rides the `Started` event into the shared
    /// shell list, so every session view shows where the shell came from and
    /// the boundary can route its completion note back to the launcher
    /// (`docs/background.md`).
    ///
    /// # Errors
    /// The spawn error text when the shell can't start.
    pub fn launch_from(
        &self,
        command: &str,
        description: Option<String>,
        from_model: bool,
        origin: Option<BgOrigin>,
    ) -> Result<LaunchedTask, String> {
        // Group + terminal membership (and stdio) come from
        // `subprocess::spawn_detached_shell`: its own process group
        // (pgid == pid) so kill() reaps grandchildren too, and no controlling
        // terminal, so a `/dev/tty` prompt errors instead of wedging the task
        // (see `crate::subprocess`, the `llm::exec` pattern).
        let mut child = crate::subprocess::spawn_detached_in(
            self.detach.as_deref(),
            crate::subprocess::tool_shell(),
            command,
        )
        .map_err(|err| format!("failed to run command: {err}"))?;

        // Drain both pipes on their own threads (a chatty command must never
        // block on a full pipe), forwarding raw chunks for the monitor to
        // merge in arrival order — the same shape as `llm::exec::run_bash`.
        let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>();
        if let Some(pipe) = child.stdout.take() {
            let tx = chunk_tx.clone();
            std::thread::spawn(move || drain_pipe(pipe, &tx));
        }
        if let Some(pipe) = child.stderr.take() {
            let tx = chunk_tx.clone();
            std::thread::spawn(move || drain_pipe(pipe, &tx));
        }
        drop(chunk_tx);

        Ok(self
            .register(
                Launch {
                    command: command.to_string(),
                    description,
                    from_model,
                    origin,
                },
                child,
                chunk_rx,
                Vec::new(),
                Terminal::Pipe,
                true,
            )
            .task)
    }

    /// Start `command` in a **pseudo-terminal of its own** — an interactive
    /// session the model types into with `bash_session`
    /// (`docs/interactive-shell.md`). The session is registered at once but
    /// **announced** (`Started`) only when `announce` says so: a `tty` call
    /// waiting in the foreground announces it only if it outlives the call
    /// ([`announce`](Self::announce)), so a command that finishes inside its
    /// own call never shows up in the footer or the manager at all.
    ///
    /// # Errors
    /// [`TtyLaunchError::Refused`] with the model-facing reason — too many
    /// sessions already running, listed — or [`TtyLaunchError::NoTerminal`]
    /// when no terminal can be given to the command here.
    pub fn launch_tty(
        &self,
        command: &str,
        description: Option<String>,
        origin: Option<BgOrigin>,
        announce: bool,
    ) -> Result<TtyLaunch, TtyLaunchError> {
        if !self.terminals {
            return Err(TtyLaunchError::NoTerminal(
                "this registry gives no command a terminal".to_string(),
            ));
        }
        let running = self.running_ttys();
        if running.len() >= MAX_TTY_SESSIONS {
            let list: Vec<String> = running
                .iter()
                .map(|(id, command)| format!("{id} ({command})"))
                .collect();
            return Err(TtyLaunchError::Refused(format!(
                "{MAX_TTY_SESSIONS} sessions are already running — stop one with bashkill \
                 first: {}",
                list.join(", ")
            )));
        }
        #[cfg(unix)]
        {
            let no_terminal = |err: std::io::Error| {
                TtyLaunchError::NoTerminal(format!(
                    "failed to start the command in a terminal: {err}"
                ))
            };
            let session =
                crate::pty::spawn::spawn(self.detach.as_deref(), command).map_err(no_terminal)?;
            let reader = session.master.try_clone().map_err(no_terminal)?;
            // The terminal's output: one stream (stdout and stderr share the
            // terminal), read until the session lets go of it (`EIO`).
            let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>();
            std::thread::spawn(move || drain_pipe(reader, &chunk_tx));
            // Its input: a writer thread, since a terminal write blocks while
            // the program is not reading, and no call may block holding state.
            let (input_tx, input_rx) = mpsc::channel::<WriteOp>();
            let probe = session.master.try_clone().map_err(no_terminal)?;
            let writer = session.master;
            std::thread::spawn(move || write_terminal(writer, &input_rx));
            let registered = self.register(
                Launch {
                    command: command.to_string(),
                    description,
                    from_model: true,
                    origin,
                },
                session.child,
                chunk_rx,
                Vec::new(),
                Terminal::Tty {
                    input: input_tx,
                    terminal: probe,
                },
                announce,
            );
            Ok(TtyLaunch {
                task: registered.task,
                io: registered.io,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (command, description, origin, announce);
            Err(TtyLaunchError::NoTerminal(
                "a terminal needs a Unix pseudo-terminal".to_string(),
            ))
        }
    }

    /// Adopt a **running** foreground command (the Ctrl+B transfer): take
    /// ownership of its child and pipe channel mid-run, replaying the output
    /// read so far (`prior`) into the task's stream + file so nothing is
    /// lost. The caller's own pipe-reader threads keep feeding `chunk_rx` and
    /// exit at EOF on their own. Always main-conversation-owned (only the
    /// main turn's foreground runners poll the Ctrl+B latch — a subagent's
    /// executor never hands off). See `docs/background.md`.
    pub fn adopt(
        &self,
        command: &str,
        description: Option<String>,
        from_model: bool,
        child: Child,
        chunk_rx: mpsc::Receiver<Vec<u8>>,
        prior: Vec<u8>,
    ) -> LaunchedTask {
        self.register(
            Launch {
                command: command.to_string(),
                description,
                from_model,
                origin: None,
            },
            child,
            chunk_rx,
            prior,
            Terminal::Pipe,
            true,
        )
        .task
    }

    /// The shared tail of [`launch_from`]/[`launch_tty`]/[`adopt`]: allocate
    /// the id, open the interim-output file, record the task, announce it
    /// (when asked to), and hand the child to its monitor thread.
    ///
    /// [`launch_from`]: BackgroundRegistry::launch_from
    /// [`launch_tty`]: BackgroundRegistry::launch_tty
    /// [`adopt`]: BackgroundRegistry::adopt
    fn register(
        &self,
        launch: Launch,
        child: Child,
        chunk_rx: mpsc::Receiver<Vec<u8>>,
        prior: Vec<u8>,
        terminal: Terminal,
        announce: bool,
    ) -> Registered {
        let kill = CancelToken::new();
        let (input, terminal, tty) = match terminal {
            Terminal::Pipe => (None, None, false),
            Terminal::Tty { input, terminal } => (Some(input), Some(terminal), true),
        };
        // The monitor reads the terminal's line mode as output arrives; a
        // handle of its own, so it never takes the registry lock to do it.
        let monitor_terminal = terminal.as_ref().and_then(|t| t.try_clone().ok());
        // A TTY session its launching call waits on is a waiter from birth,
        // so an exit before the call reaches its wait is still the call's to
        // report (`pty::session`).
        let io = Arc::new(if tty && !announce {
            SessionIo::waited(tty)
        } else {
            SessionIo::new(tty)
        });
        // An adopted command's output so far is in the session before anyone
        // can look at it: the call that handed it over reports it as what the
        // model has seen, and a look after that is only what is new
        // (docs/bash-tools.md). The monitor replays it to the file and the
        // event loop, not to the session a second time.
        if !prior.is_empty() {
            let _ = io.absorb(&prior);
        }
        let (id, output_path) = {
            let mut inner = self.inner.lock().expect("registry lock");
            // Roll a fresh Claude-Code-style id, re-rolling the (vanishingly
            // rare) collision with a task that is still running.
            let id = loop {
                inner.next_id += 1;
                let candidate = task_id(entropy_seed(inner.next_id));
                if !inner.tasks.contains_key(&candidate) {
                    break candidate;
                }
            };
            let path = inner.dir.join(format!("{id}.output"));
            inner.tasks.insert(
                id.clone(),
                Task {
                    pgid: child.id(),
                    kill: kill.clone(),
                    killed: false,
                    exited: false,
                    io: Arc::clone(&io),
                    input: input.clone(),
                    terminal,
                    launch: launch.clone(),
                    started: std::time::Instant::now(),
                },
            );
            (id, path)
        };
        // Best-effort tee file — a failure only loses the interim read path.
        if let Some(parent) = output_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = File::create(&output_path).ok();
        // A report that has to cut the output names the file with all of it
        // (`pty::fold::HeadTail`, docs/bash-tools.md).
        if file.is_some() {
            io.set_log(output_path.clone());
        }
        if announce {
            self.announce(&id);
        }
        let monitor = MonitorHandle {
            registry: self.clone(),
            id: id.clone(),
            io: Arc::clone(&io),
            input,
            terminal: monitor_terminal,
        };
        std::thread::spawn(move || monitor.run(child, prior, chunk_rx, kill, file));
        Registered {
            task: LaunchedTask { id, output_path },
            io,
        }
    }

    /// Tell the event loop about a task registered without announcement (a
    /// `tty` session that outlived its call): `Started`, then — for a TTY
    /// session — its screen as it stands. A task already announced, or
    /// already gone, sends nothing.
    pub fn announce(&self, id: &str) {
        let Some((io, launch)) = self
            .inner
            .lock()
            .expect("registry lock")
            .tasks
            .get(id)
            .map(|task| (Arc::clone(&task.io), task.launch.clone()))
        else {
            return;
        };
        if !io.announce() {
            return;
        }
        let _ = self.events.send(BgEvent::Started {
            id: id.to_string(),
            command: launch.command,
            description: launch.description,
            from_model: launch.from_model,
            origin: launch.origin,
        });
        if let Some(text) = io.screen_text() {
            let _ = self.events.send(BgEvent::Screen {
                id: id.to_string(),
                text,
            });
        }
    }

    /// A running session, for the model-tool executor's `bash_session` —
    /// `None` when no task has this id (it ended, or never existed).
    #[must_use]
    pub fn session(&self, id: &str) -> Option<SessionHandle> {
        let inner = self.inner.lock().expect("registry lock");
        inner.tasks.get(id).map(|task| SessionHandle {
            io: Arc::clone(&task.io),
            command: task.launch.command.clone(),
        })
    }

    /// How a TTY session's program is reading its terminal
    /// (`pty::spawn::line_mode`) — `None` for a pipe task, or a session gone.
    #[must_use]
    pub fn line_mode(&self, id: &str) -> Option<crate::pty::spawn::LineMode> {
        #[cfg(unix)]
        {
            let inner = self.inner.lock().expect("registry lock");
            let terminal = inner.tasks.get(id)?.terminal.as_ref()?;
            crate::pty::spawn::line_mode(terminal)
        }
        #[cfg(not(unix))]
        {
            let _ = id;
            None
        }
    }

    /// Every running task as `(id, command)`, oldest id first — what an
    /// unknown-session error lists so the model can find the right one.
    #[must_use]
    pub fn sessions(&self) -> Vec<(String, String)> {
        let inner = self.inner.lock().expect("registry lock");
        let mut list: Vec<(String, String)> = inner
            .tasks
            .iter()
            .filter(|(_, task)| !task.exited)
            .map(|(id, task)| (id.clone(), task.launch.command.clone()))
            .collect();
        list.sort();
        list
    }

    /// The running TTY sessions that outlived their call, as `(id,
    /// command)` — what the session cap counts and lists. A command still
    /// inside its own `bash` call is not one: every command runs in a
    /// terminal (`docs/bash-tools.md`), and a batch of subagents' ordinary
    /// commands must never be refused for the sessions they are not.
    fn running_ttys(&self) -> Vec<(String, String)> {
        let candidates: Vec<(String, String, Arc<SessionIo>)> = {
            let inner = self.inner.lock().expect("registry lock");
            inner
                .tasks
                .iter()
                .filter(|(_, task)| !task.exited && task.input.is_some())
                .map(|(id, task)| {
                    (
                        id.clone(),
                        task.launch.command.clone(),
                        Arc::clone(&task.io),
                    )
                })
                .collect()
        };
        let mut list: Vec<(String, String)> = candidates
            .into_iter()
            .filter(|(_, _, io)| io.announced())
            .map(|(id, command, _)| (id, command))
            .collect();
        list.sort();
        list
    }

    /// Every running task as `bashlist` names it (`docs/bash-tools.md`): id,
    /// command, how long it has run, and whether it sits at a prompt —
    /// oldest id first.
    #[must_use]
    pub fn running(&self) -> Vec<crate::pty::report::Listed> {
        use crate::pty::report::{Listed, Waiting};
        use crate::pty::settle::WaitKind;
        let tasks: Vec<(String, String, std::time::Instant, Arc<SessionIo>)> = {
            let inner = self.inner.lock().expect("registry lock");
            inner
                .tasks
                .iter()
                .filter(|(_, task)| !task.exited)
                .map(|(id, task)| {
                    (
                        id.clone(),
                        task.launch.command.clone(),
                        task.started,
                        Arc::clone(&task.io),
                    )
                })
                .collect()
        };
        let mut list: Vec<Listed> = tasks
            .into_iter()
            .map(|(id, command, started, io)| Listed {
                id,
                command,
                running_for: started.elapsed(),
                waiting: Waiting::of(io.waiting(WaitKind::Input, io.mark()), io.password_prompt()),
            })
            .collect();
        list.sort_by(|a, b| b.running_for.cmp(&a.running_for));
        list
    }

    /// Ask every process of a running task to stop with `signal` (a name
    /// `kill -s` takes: `INT`, `TERM`) — a TTY session's whole session, a
    /// pipe task's process group — and mark it stopped by request, so its
    /// exit reads as a stop, not a failure (`docs/bash-tools.md`).
    pub fn signal(&self, id: &str, signal: &str) {
        let (pgid, tty) = {
            let mut inner = self.inner.lock().expect("registry lock");
            let Some(task) = inner.tasks.get_mut(id) else {
                return;
            };
            if task.exited {
                return;
            }
            task.killed = true;
            (task.pgid, task.input.is_some())
        };
        #[cfg(unix)]
        if tty {
            signal_session(pgid, signal);
        } else {
            signal_group(pgid, signal);
        }
        #[cfg(not(unix))]
        let _ = (pgid, tty, signal);
    }

    /// Type into a TTY session: `chunks` in order, pausing where a chunk asks
    /// to (`pty::keys`).
    ///
    /// # Errors
    /// The model-facing reason when the session is gone or has no terminal.
    pub fn send_input(&self, id: &str, chunks: Vec<InputChunk>) -> Result<(), String> {
        let (input, io) = {
            let inner = self.inner.lock().expect("registry lock");
            let task = inner
                .tasks
                .get(id)
                .filter(|task| !task.exited)
                .ok_or_else(|| format!("session {id} is not running"))?;
            let input = task.input.clone().ok_or_else(|| {
                format!(
                    "session {id} has no terminal to type into — it runs on a pipe (its \
                     stdin is /dev/null); you can still wait on it with bashwait, send <C-c> \
                     with bashsend, or stop it with bashkill"
                )
            })?;
            (input, Arc::clone(&task.io))
        };
        // What the program draws from here on answers these keys.
        let typed: Vec<u8> = chunks
            .iter()
            .flat_map(|chunk| chunk.bytes.clone())
            .collect();
        io.note_input(&typed);
        io.typing_for(crate::pty::keys::typing_bound(&chunks));
        let keys = WriteOp::Keys {
            chunks,
            io: Arc::clone(&io),
        };
        if input.send(keys).is_err() {
            io.typed();
            return Err(format!("session {id} is no longer accepting input"));
        }
        Ok(())
    }

    /// Who reads what is typed into session `id`: its terminal's foreground
    /// program (`pty::spawn::foreground_program`) — [`Reader::Unknown`]
    /// for a pipe task, a session gone, or a system that will not say.
    ///
    /// [`Reader::Unknown`]: crate::pty::keys::Reader::Unknown
    #[must_use]
    pub fn reader(&self, id: &str) -> crate::pty::keys::Reader {
        #[cfg(unix)]
        {
            let inner = self.inner.lock().expect("registry lock");
            inner
                .tasks
                .get(id)
                .and_then(|task| task.terminal.as_ref())
                .and_then(crate::pty::spawn::foreground_program)
                .map_or(crate::pty::keys::Reader::Unknown, |program| {
                    crate::pty::keys::Reader::of(&program)
                })
        }
        #[cfg(not(unix))]
        {
            let _ = id;
            crate::pty::keys::Reader::Unknown
        }
    }

    /// Interrupt a task as Ctrl+C would — `SIGINT` to its process group. How
    /// `<C-c>` reaches a pipe task, which has no terminal to type it into.
    pub fn interrupt(&self, id: &str) {
        let pgid = {
            let inner = self.inner.lock().expect("registry lock");
            match inner.tasks.get(id).filter(|task| !task.exited) {
                Some(task) => task.pgid,
                None => return,
            }
        };
        signal_group(pgid, "INT");
    }

    /// Finalize an exit a waiting call was left to report
    /// (`pty::session::SessionIo::end_wait`): drop the task and — if the
    /// event loop knew of it — send its `Exited`, `observed` saying whether
    /// the model already read it.
    pub fn finalize(&self, id: &str, observed: bool) {
        let Some(task) = self.inner.lock().expect("registry lock").tasks.remove(id) else {
            return;
        };
        if task.io.announced() {
            let _ = self.events.send(BgEvent::Exited {
                id: id.to_string(),
                code: task.io.exit().flatten(),
                killed: task.killed,
                observed,
            });
        }
    }

    /// Stop a background task: mark it user-killed and SIGKILL its whole
    /// process group (synchronously — plus the monitor's kill token as the
    /// portable backstop). The monitor then reports `Exited {killed: true}`.
    pub fn kill(&self, id: &str) {
        let (pgid, tty) = {
            let mut inner = self.inner.lock().expect("registry lock");
            let Some(task) = inner.tasks.get_mut(id) else {
                return;
            };
            if task.exited {
                return;
            }
            task.killed = true;
            task.kill.cancel();
            (task.pgid, task.input.is_some())
        };
        kill_group(pgid);
        if tty {
            kill_session(pgid);
        }
    }

    /// Stop every running task — the quit and `/clear` sweep. Synchronous
    /// (direct group kills), so the quit path can't orphan a `ping`.
    pub fn kill_all(&self) {
        let groups: Vec<(u32, bool)> = {
            let mut inner = self.inner.lock().expect("registry lock");
            inner
                .tasks
                .values_mut()
                .filter(|task| !task.exited)
                .map(|task| {
                    task.killed = true;
                    task.kill.cancel();
                    (task.pgid, task.input.is_some())
                })
                .collect()
        };
        for (pgid, tty) in groups {
            kill_group(pgid);
            if tty {
                kill_session(pgid);
            }
        }
    }

    /// Raise the Ctrl+B latch: the running foreground command's poll loop
    /// consumes it via [`take_background_request`] and hands itself off.
    ///
    /// [`take_background_request`]: BackgroundRegistry::take_background_request
    pub fn request_background(&self) {
        self.inner.lock().expect("registry lock").background_request = true;
    }

    /// Consume the Ctrl+B latch (returns whether one was pending).
    #[must_use]
    pub fn take_background_request(&self) -> bool {
        let mut inner = self.inner.lock().expect("registry lock");
        std::mem::take(&mut inner.background_request)
    }

    /// Drop a stale Ctrl+B latch — every foreground command clears it as it
    /// starts, so a press that missed one command can't background the next.
    pub fn clear_background_request(&self) {
        self.inner.lock().expect("registry lock").background_request = false;
    }

    /// Post a completed shell's model-facing note onto the notice board (the
    /// event loop, as it handles the shell's `Exited` event).
    pub fn post_notice(&self, context: String, from_model: bool) {
        self.inner
            .lock()
            .expect("registry lock")
            .pending_notices
            .push(PendingNotice {
                context,
                from_model,
            });
    }

    /// Take every posted note, in arrival order, each delivered exactly once —
    /// the in-flight agent before each round, or the turn-boundary dispatch
    /// (whose untaken, model-launched notes warrant the automatic follow-up
    /// turn). `/clear` takes-and-drops so a wiped conversation owes nothing.
    #[must_use]
    pub fn take_pending_notices(&self) -> Vec<PendingNotice> {
        std::mem::take(&mut self.inner.lock().expect("registry lock").pending_notices)
    }
}

/// How a task's process is wired: pipes (stdout/stderr, stdin on
/// `/dev/null`), or a terminal of its own with a writer for its input.
enum Terminal {
    Pipe,
    Tty {
        input: mpsc::Sender<WriteOp>,
        terminal: File,
    },
}

/// The monitor thread's registry access, bundled.
struct MonitorHandle {
    registry: BackgroundRegistry,
    id: String,
    io: Arc<SessionIo>,
    /// A TTY session's writer — where the terminal's query replies go.
    input: Option<mpsc::Sender<WriteOp>>,
    /// A TTY session's terminal, to read how the program is reading it
    /// ([`MonitorHandle::refresh_line_mode`]).
    terminal: Option<File>,
}

impl MonitorHandle {
    /// Own the child to its end: merge + forward output, poll the kill token,
    /// and report the exit. An adopted command's `prior` output — what the
    /// foreground runner read before the handoff — goes first, into the same
    /// fold as the rest. See the module docs.
    fn run(
        self,
        mut child: Child,
        prior: Vec<u8>,
        chunk_rx: mpsc::Receiver<Vec<u8>>,
        kill: CancelToken,
        mut file: Option<File>,
    ) {
        let mut fold = crate::pty::fold::Fold::new();
        let mut screen = ScreenPacer::default();
        let mut probe = self.prober(child.id());
        if !prior.is_empty() {
            self.replay(&prior, &mut fold, file.as_mut());
        }
        let status = loop {
            while let Ok(chunk) = chunk_rx.try_recv() {
                self.absorb(&chunk, &mut fold, file.as_mut(), &mut screen);
            }
            if kill.is_cancelled() {
                kill_group(child.id());
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    // A process the command started and left running (`server
                    // & …`) stops with it below: the report tells the model
                    // how to keep one running (docs/bash-tools.md).
                    if crate::subprocess::group_outlives(&child) {
                        self.io.set_stranded();
                    }
                    break Some(status);
                }
                Ok(None) => match chunk_rx.recv_timeout(MONITOR_POLL_INTERVAL) {
                    Ok(chunk) => self.absorb(&chunk, &mut fold, file.as_mut(), &mut screen),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        self.refresh_line_mode();
                        self.send_screen(&mut screen, true);
                        if let Some(probe) = probe.as_mut() {
                            probe.refresh(&self.io);
                        }
                        // After the screen: the event loop reads the question
                        // off the screen it already has.
                        if self.input.is_some() && self.io.take_unseen_prompt(WAITING_NOTICE_QUIET)
                        {
                            let _ = self.registry.events.send(BgEvent::Waiting {
                                id: self.id.clone(),
                            });
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(MONITOR_POLL_INTERVAL);
                    }
                },
                Err(_) => break None,
            }
        };
        // Reap any straggler holding the pipes (the group kill is idempotent),
        // then flush what the readers still buffered plus the line left
        // unfinished. A terminal's last output can trail the exit by a
        // moment — its reader drains the master after the program closed it —
        // so a TTY session gives it that moment before the exit is recorded.
        kill_group(child.id());
        let _ = child.kill();
        let _ = child.wait();
        let settle_until = std::time::Instant::now() + TTY_DRAIN_GRACE;
        loop {
            match chunk_rx.recv_timeout(MONITOR_POLL_INTERVAL) {
                Ok(chunk) => self.absorb(&chunk, &mut fold, file.as_mut(), &mut screen),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self.input.is_none() || std::time::Instant::now() >= settle_until {
                        break;
                    }
                }
            }
        }
        self.send_output(fold.finish());
        let code = status.and_then(|s| s.code());
        let (rest, finish) = self.io.finish(code);
        if self.input.is_some() {
            if let Some(f) = file.as_mut() {
                let _ = f.write_all(rest.as_bytes());
            }
            self.send_screen(&mut screen, true);
        }
        let mut inner = self.registry.inner.lock().expect("registry lock");
        match finish {
            // Nobody is waiting on the exit: report it now, unobserved.
            Finish::Now => {
                let killed = inner.tasks.remove(&self.id).is_some_and(|task| task.killed);
                drop(inner);
                if self.io.announced() {
                    let _ = self.registry.events.send(BgEvent::Exited {
                        id: self.id,
                        code,
                        killed,
                        observed: false,
                    });
                }
            }
            // A `bash_session` call is waiting: it reports the exit and
            // finalizes it (`BackgroundRegistry::finalize`). Until then no
            // kill may reach the group id, which the system may reuse.
            Finish::Waiter => {
                if let Some(task) = inner.tasks.get_mut(&self.id) {
                    task.exited = true;
                }
            }
        }
    }

    /// An adopted command's output so far — already in the session
    /// ([`BackgroundRegistry::register`]) — to the tee file and the event
    /// loop's stream, as [`absorb`](Self::absorb) sends a pipe task's output.
    fn replay(&self, chunk: &[u8], fold: &mut crate::pty::fold::Fold, file: Option<&mut File>) {
        if let Some(f) = file {
            let _ = f.write_all(chunk);
        }
        fold.feed(chunk);
        self.send_output(fold.take_settled());
    }

    /// Fold one chunk in: both views (answering the terminal's queries on a
    /// TTY), the tee file, and the event loop's stream.
    fn absorb(
        &self,
        chunk: &[u8],
        fold: &mut crate::pty::fold::Fold,
        file: Option<&mut File>,
        screen: &mut ScreenPacer,
    ) {
        let (replies, committed) = self.io.absorb(chunk);
        let Some(input) = &self.input else {
            // A pipe task: the raw bytes to the file (the stream as written,
            // every byte of it), and to the event loop each line once it
            // ends, as a terminal would show it.
            if let Some(f) = file {
                let _ = f.write_all(chunk);
            }
            fold.feed(chunk);
            self.send_output(fold.take_settled());
            return;
        };
        // A TTY session: the terminal owes the program its query replies,
        // the file gets the transcript's finished lines (readable text, not
        // escape soup), and the event loop the screen.
        if !replies.is_empty() {
            let _ = input.send(WriteOp::Bytes(replies));
        }
        if let Some(f) = file {
            let _ = f.write_all(committed.as_bytes());
        }
        self.refresh_line_mode();
        screen.dirty = true;
        self.send_screen(screen, false);
    }

    /// Tell the session how its program is reading the terminal: key by key
    /// (a menu, an editor — not a relay holding it raw, see
    /// `LineMode::reads_keys`), which waits on keys wherever its cursor
    /// sits, or a line with echo off (`LineMode::hides_input`), a password
    /// prompt — readable here even when the program runs as root and the
    /// probe is blind (`pty::session`). Read after each chunk of output and
    /// on every idle poll, so a mode switched without printing is seen
    /// within [`MONITOR_POLL_INTERVAL`].
    fn refresh_line_mode(&self) {
        #[cfg(unix)]
        if let Some(mode) = self
            .terminal
            .as_ref()
            .and_then(crate::pty::spawn::line_mode)
        {
            self.io.set_reading_keys(mode.reads_keys());
            self.io.set_hidden_input(mode.hides_input());
        }
    }

    /// A TTY session's [`Prober`]: its program's process and the path of its
    /// terminal. `None` for a pipe task, or a terminal with no path to name.
    fn prober(&self, pid: u32) -> Option<Prober> {
        #[cfg(unix)]
        {
            let terminal = crate::pty::spawn::terminal_path(self.terminal.as_ref()?)?;
            Some(Prober {
                pid,
                terminal,
                last: None,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            None
        }
    }

    /// Send the session's screen to the event loop — if it changed, the
    /// session is announced, and (unless `now`) the last send was at least
    /// [`SCREEN_EVENT_INTERVAL`] ago.
    fn send_screen(&self, pacer: &mut ScreenPacer, now: bool) {
        if !pacer.dirty || self.input.is_none() || !self.io.announced() {
            return;
        }
        if !now
            && pacer
                .sent
                .is_some_and(|sent| sent.elapsed() < SCREEN_EVENT_INTERVAL)
        {
            return;
        }
        if let Some(text) = self.io.screen_text() {
            let _ = self.registry.events.send(BgEvent::Screen {
                id: self.id.clone(),
                text,
            });
        }
        pacer.dirty = false;
        pacer.sent = Some(std::time::Instant::now());
    }

    fn send_output(&self, chunk: String) {
        if chunk.is_empty() || !self.io.announced() {
            return;
        }
        let _ = self.registry.events.send(BgEvent::Output {
            id: self.id.clone(),
            chunk,
        });
    }
}

/// How often a monitor probes what a quiet session's program is blocked in
/// while a call waits on it ([`crate::pty::probe`]) — a handful of `/proc`
/// reads each time.
const PROBE_INTERVAL: Duration = Duration::from_millis(200);

/// What a TTY session's monitor needs to ask the kernel what its program is
/// blocked in (`docs/interactive-shell.md`).
struct Prober {
    /// The session leader — the root of the process tree walked.
    pid: u32,
    /// The terminal's path (`/dev/pts/N`), as its processes hold it.
    terminal: PathBuf,
    /// When the last probe ran.
    last: Option<std::time::Instant>,
}

impl Prober {
    /// Probe, if the session wants it ([`SessionIo::wants_probe`]) and the
    /// last probe is [`PROBE_INTERVAL`] old, and record what it saw.
    fn refresh(&mut self, io: &SessionIo) {
        if !io.wants_probe()
            || self
                .last
                .is_some_and(|last| last.elapsed() < PROBE_INTERVAL)
        {
            return;
        }
        self.last = Some(std::time::Instant::now());
        io.set_probe(crate::pty::probe::probe(self.pid, &self.terminal));
    }
}

/// When a TTY session's screen was last sent, and whether it changed since.
#[derive(Default)]
struct ScreenPacer {
    sent: Option<std::time::Instant>,
    dirty: bool,
}

/// How long a TTY session's monitor keeps reading after the exit for output
/// still in the terminal — the master drains after the program has gone.
const TTY_DRAIN_GRACE: Duration = Duration::from_millis(150);

/// A TTY session's writer thread: every [`WriteOp`] in order, until the
/// session's task is dropped (the channel closes) or the terminal refuses a
/// write (the session is gone).
fn write_terminal(mut terminal: File, ops: &mpsc::Receiver<WriteOp>) {
    while let Ok(op) = ops.recv() {
        let written = match op {
            WriteOp::Bytes(bytes) => terminal.write_all(&bytes).is_ok(),
            WriteOp::Keys { chunks, io } => {
                let typed = type_keys(&mut terminal, &chunks);
                io.typed();
                typed
            }
        };
        if !written {
            break;
        }
    }
    // Keys that will never be typed are not being typed either.
    for op in ops.try_iter() {
        if let WriteOp::Keys { io, .. } = op {
            io.typed();
        }
    }
}

/// Type `chunks` into `terminal` the way a person types
/// (`pty::keys::Pace`): each write once the program has read the one before
/// — asked of the terminal's input queue (`pty::spawn::InputQueue`) — and
/// at least [`KEY_PAUSE`] after it where the queue cannot answer for the
/// program: it cannot be asked, or a relay holds the terminal
/// (`pty::spawn::LineMode::relays`), taking each key the moment it lands
/// and handing it to a program out of sight. A program that has not read a
/// write within [`KEY_READ_WAIT`] is not reading: the rest go as typeahead,
/// [`KEY_PAUSE`] apart. A lone Esc that more keys follow is held
/// [`ESC_PAUSE`] past its read. `false` when the terminal refused a write —
/// the session is gone.
///
/// [`KEY_PAUSE`]: crate::pty::keys::KEY_PAUSE
/// [`KEY_READ_WAIT`]: crate::pty::keys::KEY_READ_WAIT
/// [`ESC_PAUSE`]: crate::pty::keys::ESC_PAUSE
fn type_keys(terminal: &mut File, chunks: &[InputChunk]) -> bool {
    use crate::pty::keys::{ESC_PAUSE, KEY_PAUSE, Pace};
    #[cfg(unix)]
    let mut queue = crate::pty::spawn::InputQueue::of(terminal);
    for chunk in chunks {
        if terminal.write_all(&chunk.bytes).is_err() {
            return false;
        }
        if chunk.then == Pace::Last {
            continue;
        }
        let written = std::time::Instant::now();
        #[cfg(unix)]
        let seen = {
            if queue.as_ref().is_some_and(|queue| !read_in_time(queue)) {
                queue = None;
            }
            queue.is_some()
                && !crate::pty::spawn::line_mode(terminal)
                    .is_some_and(crate::pty::spawn::LineMode::relays)
        };
        #[cfg(not(unix))]
        let seen = false;
        if !seen {
            std::thread::sleep(KEY_PAUSE.saturating_sub(written.elapsed()));
        }
        if chunk.then == Pace::Esc {
            std::thread::sleep(ESC_PAUSE);
        }
    }
    true
}

/// Wait until the program has read what was just typed into `queue`'s
/// terminal — the queue empty again (a key still on its way to it counts as
/// in it) — `false` when it has not within
/// [`KEY_READ_WAIT`](crate::pty::keys::KEY_READ_WAIT), or the terminal will
/// not say.
#[cfg(unix)]
fn read_in_time(queue: &crate::pty::spawn::InputQueue) -> bool {
    let typed = std::time::Instant::now();
    loop {
        match queue.pending() {
            None => return false,
            Some(0) => return true,
            Some(_) if typed.elapsed() >= crate::pty::keys::KEY_READ_WAIT => return false,
            Some(_) => std::thread::sleep(KEY_READ_POLL),
        }
    }
}

/// Read `pipe` to EOF, forwarding raw chunks (the `llm::exec` shape). Stops
/// early if the receiver hung up.
fn drain_pipe(mut pipe: impl Read, tx: &mpsc::Sender<Vec<u8>>) {
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if tx.send(chunk[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// SIGKILL the whole process group `pgid` — the `llm::exec` pattern (this
/// crate forbids `unsafe`, so the shell's POSIX `kill` builtin does it; the
/// helper `sh` starts in its own group and never signals itself). Best-effort.
#[cfg(unix)]
fn kill_group(pgid: u32) {
    signal_group(pgid, "KILL");
}

/// Send `signal` (a name `kill -s` accepts: `INT`, `KILL`) to the whole
/// process group `pgid`. Best-effort.
#[cfg(unix)]
fn signal_group(pgid: u32, signal: &str) {
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!("kill -{signal} -{pgid} 2>/dev/null"))
        .status();
}

/// SIGKILL everything in the **session** `sid` — a TTY session's jobs, which
/// an interactive shell puts in process groups of their own, where the group
/// kill cannot reach them. Best-effort: `pkill -s` where there is one.
#[cfg(unix)]
fn kill_session(sid: u32) {
    signal_session(sid, "KILL");
}

/// Send `signal` to everything in the **session** `sid` — a stop request
/// reaching a TTY session's jobs too (`docs/bash-tools.md`). Best-effort:
/// `pkill -s` where there is one.
#[cfg(unix)]
fn signal_session(sid: u32, signal: &str) {
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!("pkill -{signal} -s {sid} 2>/dev/null"))
        .status();
}

/// Non-unix fallback: no process groups — the monitor's own `child.kill()`
/// (via the kill token) reaps the direct child.
#[cfg(not(unix))]
fn kill_group(_pgid: u32) {}

/// Non-unix fallback: no process groups to signal.
#[cfg(not(unix))]
fn signal_group(_pgid: u32, _signal: &str) {}

/// Non-unix fallback: no sessions to sweep.
#[cfg(not(unix))]
fn kill_session(_sid: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    fn registry() -> (
        BackgroundRegistry,
        tokio::sync::mpsc::UnboundedReceiver<BgEvent>,
    ) {
        // A unique dir per test: the suite runs in parallel, so give every
        // registry its own tee-file dir (ids are collision-rolled per
        // registry, not globally).
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let dir =
            std::env::temp_dir().join(format!("alter-zero-bg-test-{}-{seq}", std::process::id()));
        (BackgroundRegistry::new(tx, dir), rx)
    }

    /// Wait (bounded) for the next event.
    fn next(rx: &mut tokio::sync::mpsc::UnboundedReceiver<BgEvent>) -> BgEvent {
        for _ in 0..500 {
            if let Ok(ev) = rx.try_recv() {
                return ev;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no BgEvent arrived in time");
    }

    #[test]
    fn task_id_is_a_deterministic_b_plus_eight_base36() {
        // Claude Code's id shape (`bvyo7tkbe`): a `b` prefix + 8 lowercase
        // base36 chars. Pure in the seed — the registry feeds it fresh
        // entropy per launch (docs/background.md).
        assert_eq!(task_id(42), task_id(42), "pure in the seed");
        assert_ne!(task_id(1), task_id(2), "seeds differentiate");
        for seed in 0..64 {
            let id = task_id(seed);
            assert_eq!(id.len(), 9, "b + 8 chars: {id}");
            assert!(id.starts_with('b'), "the b prefix: {id}");
            assert!(
                id[1..]
                    .chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()),
                "a base36 body: {id}"
            );
        }
    }

    #[test]
    fn launched_ids_are_claude_code_style_and_unique() {
        // A launch mints a `bvyo7tkbe`-style id (never a counter like the old
        // `bash_1`), a second launch never repeats it, and the interim file is
        // named after it (docs/background.md).
        let (reg, _rx) = registry();
        let a = reg.launch("true", None, false).expect("launches");
        let b = reg.launch("true", None, false).expect("launches");
        assert_ne!(a.id, b.id, "two launches never share an id");
        for id in [&a.id, &b.id] {
            assert_eq!(id.len(), 9, "b + 8 chars: {id}");
            assert!(id.starts_with('b'), "the b prefix: {id}");
            assert!(
                id[1..]
                    .chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()),
                "a base36 body: {id}"
            );
        }
        assert!(
            a.output_path.ends_with(format!("{}.output", a.id)),
            "the interim file is named after the id: {}",
            a.output_path.display()
        );
    }

    #[test]
    fn launch_streams_started_output_and_a_clean_exit() {
        let (reg, mut rx) = registry();
        let task = reg
            .launch("printf 'a\\nb\\n'", Some("print two lines".into()), true)
            .expect("launches");

        let started = next(&mut rx);
        assert_eq!(
            started,
            BgEvent::Started {
                id: task.id.clone(),
                command: "printf 'a\\nb\\n'".into(),
                description: Some("print two lines".into()),
                from_model: true,
                origin: None,
            }
        );
        // Output (possibly split across chunks) then a clean exit.
        let mut output = String::new();
        loop {
            match next(&mut rx) {
                BgEvent::Output { id, chunk } => {
                    assert_eq!(id, task.id);
                    output.push_str(&chunk);
                }
                BgEvent::Exited {
                    id,
                    code,
                    killed,
                    observed,
                } => {
                    assert_eq!(id, task.id);
                    assert_eq!(code, Some(0));
                    assert!(!killed);
                    assert!(!observed, "nobody was waiting on it");
                    break;
                }
                other => panic!("unexpected event {other:?}"),
            }
        }
        assert_eq!(output, "a\nb\n");
        // The interim file got the full output too.
        let teed = std::fs::read_to_string(&task.output_path).expect("tee file exists");
        assert_eq!(teed, "a\nb\n");
        std::fs::remove_file(&task.output_path).ok();
    }

    #[test]
    fn launch_from_carries_the_subagent_origin_on_started() {
        // A subagent's `run_in_background` launch attributes itself: the
        // Started event carries the launcher's id + type so the shell list
        // (and the completion routing) know where it came from
        // (docs/background.md).
        let (reg, mut rx) = registry();
        let origin = BgOrigin {
            agent_id: "a7k2m9x4q".into(),
            agent_type: "general-purpose".into(),
        };
        let task = reg
            .launch_from("true", Some("noop".into()), true, Some(origin.clone()))
            .expect("launches");
        match next(&mut rx) {
            BgEvent::Started {
                id, origin: got, ..
            } => {
                assert_eq!(id, task.id);
                assert_eq!(got, Some(origin));
            }
            other => panic!("expected Started, got {other:?}"),
        }
        std::fs::remove_file(&task.output_path).ok();
    }

    #[test]
    fn a_nonzero_exit_reports_its_code() {
        let (reg, mut rx) = registry();
        reg.launch("exit 3", None, false).expect("launches");
        loop {
            if let BgEvent::Exited { code, killed, .. } = next(&mut rx) {
                assert_eq!(code, Some(3));
                assert!(!killed);
                break;
            }
        }
    }

    #[test]
    fn kill_stops_the_task_and_reports_killed() {
        let (reg, mut rx) = registry();
        let task = reg.launch("sleep 30", None, true).expect("launches");
        assert!(matches!(next(&mut rx), BgEvent::Started { .. }));
        let start = std::time::Instant::now();
        reg.kill(&task.id);
        loop {
            if let BgEvent::Exited { id, killed, .. } = next(&mut rx) {
                assert_eq!(id, task.id);
                assert!(killed, "a registry kill reports killed: true");
                break;
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the kill reaps promptly, not after the sleep"
        );
        std::fs::remove_file(&task.output_path).ok();
    }

    #[test]
    fn kill_reaps_a_backgrounded_grandchild_via_the_process_group() {
        // `sleep 30 &` puts a grandchild in the group; killing the group must
        // end the task promptly (the monitor would otherwise wait out the
        // grandchild holding the pipe).
        let (reg, mut rx) = registry();
        let task = reg
            .launch("sleep 30 & wait", None, false)
            .expect("launches");
        assert!(matches!(next(&mut rx), BgEvent::Started { .. }));
        let start = std::time::Instant::now();
        reg.kill(&task.id);
        loop {
            if let BgEvent::Exited { .. } = next(&mut rx) {
                break;
            }
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        std::fs::remove_file(&task.output_path).ok();
    }

    #[test]
    fn kill_all_sweeps_every_running_task() {
        let (reg, mut rx) = registry();
        reg.launch("sleep 30", None, true).expect("launches");
        reg.launch("sleep 30", None, true).expect("launches");
        let start = std::time::Instant::now();
        reg.kill_all();
        let mut exits = 0;
        while exits < 2 {
            if let BgEvent::Exited { killed, .. } = next(&mut rx) {
                assert!(killed);
                exits += 1;
            }
        }
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn posted_notices_are_taken_once_in_order() {
        // The completion notice board (docs/background.md): the loop posts a
        // finished shell's model-facing note the moment its Exited event is
        // handled; the in-flight agent (or the turn-boundary dispatch —
        // whoever gets there first) takes them, each exactly once, in
        // arrival order, with the from_model flag riding along for the
        // auto-follow-up-turn decision.
        let (reg, _rx) = registry();
        assert!(reg.take_pending_notices().is_empty(), "starts empty");
        reg.post_notice("[background] a terminated".to_string(), true);
        reg.post_notice("[background] b completed".to_string(), false);
        assert_eq!(
            reg.take_pending_notices(),
            vec![
                PendingNotice {
                    context: "[background] a terminated".to_string(),
                    from_model: true,
                },
                PendingNotice {
                    context: "[background] b completed".to_string(),
                    from_model: false,
                },
            ]
        );
        assert!(
            reg.take_pending_notices().is_empty(),
            "a take drains the board — nothing is delivered twice"
        );
    }

    #[test]
    fn launch_survives_a_stale_detach_helper() {
        // The helper is a fallback tier (`subprocess::tiers`): a path that no
        // longer exists (the TUI binary replaced mid-session) must not break
        // a launch — the chain lands on a working tier either way. The real
        // helper's conduct is covered by tests/detached_exec.rs and smoke.
        let (reg, mut rx) = registry();
        let reg = reg.with_detach_helper(Some(PathBuf::from(
            "/definitely/not/a/real/alter-zero-helper",
        )));
        let task = reg
            .launch("echo hi", None, true)
            .expect("the chain survives a stale helper");
        // The task runs to completion like any other launch.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match rx.try_recv() {
                Ok(BgEvent::Exited { code, .. }) => {
                    assert_eq!(code, Some(0));
                    break;
                }
                Ok(_) => {}
                Err(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the launched task exits"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        std::fs::remove_file(&task.output_path).ok();
    }

    #[test]
    fn the_background_request_latch_is_consumed_once() {
        let (reg, _rx) = registry();
        assert!(!reg.take_background_request(), "starts lowered");
        reg.request_background();
        assert!(reg.take_background_request(), "a raise is consumed");
        assert!(!reg.take_background_request(), "…exactly once");
        reg.request_background();
        reg.clear_background_request();
        assert!(
            !reg.take_background_request(),
            "a stale raise can be cleared"
        );
    }

    #[test]
    fn adopt_replays_prior_output_then_keeps_streaming() {
        // Model a Ctrl+B transfer: a child mid-run, some output already read
        // by the foreground runner, the rest still coming through the pipes.
        let (reg, mut rx) = registry();
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("sleep 0.3; printf 'later\\n'")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().expect("spawns");
        let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>();
        if let Some(pipe) = child.stdout.take() {
            let tx = chunk_tx.clone();
            std::thread::spawn(move || drain_pipe(pipe, &tx));
        }
        if let Some(pipe) = child.stderr.take() {
            let tx = chunk_tx.clone();
            std::thread::spawn(move || drain_pipe(pipe, &tx));
        }
        drop(chunk_tx);

        let task = reg.adopt("demo", None, false, child, chunk_rx, b"earlier\n".to_vec());
        assert!(matches!(next(&mut rx), BgEvent::Started { .. }));
        // The prior output replays first, then the still-streaming tail, then
        // the exit.
        let mut output = String::new();
        loop {
            match next(&mut rx) {
                BgEvent::Output { chunk, .. } => output.push_str(&chunk),
                BgEvent::Exited { code, .. } => {
                    assert_eq!(code, Some(0));
                    break;
                }
                other => panic!("unexpected event {other:?}"),
            }
        }
        assert_eq!(output, "earlier\nlater\n");
        let teed = std::fs::read_to_string(&task.output_path).expect("tee file");
        assert_eq!(teed, "earlier\nlater\n");
        std::fs::remove_file(&task.output_path).ok();
    }

    /// Every `Output` chunk until the task exits, joined.
    fn output_until_exit(rx: &mut tokio::sync::mpsc::UnboundedReceiver<BgEvent>) -> String {
        let mut output = String::new();
        loop {
            match next(rx) {
                BgEvent::Output { chunk, .. } => output.push_str(&chunk),
                BgEvent::Exited { .. } => return output,
                _ => {}
            }
        }
    }

    #[test]
    fn a_pipe_tasks_output_streams_folded_the_way_a_terminal_shows_it() {
        // curl or pip in the background: a `\r` progress bar reaches the
        // manager and the completion note once, in its final state, and
        // colour escapes never — while the tee file keeps the stream as
        // written (docs/interactive-shell.md).
        let (reg, mut rx) = registry();
        let task = reg
            .launch_from(
                r"printf 'get\n'; for p in 10 60 100; do printf '\r%3d%%' $p; done; printf '\n\033[1mok\033[0m\n'",
                None,
                true,
                None,
            )
            .expect("launches");
        assert_eq!(output_until_exit(&mut rx), "get\n100%\nok\n");
        let teed = std::fs::read_to_string(&task.output_path).expect("tee file");
        assert!(
            teed.contains("\r 10%"),
            "the file is the raw stream: {teed:?}"
        );
        std::fs::remove_file(&task.output_path).ok();
    }

    #[test]
    fn a_pipe_tasks_unfinished_last_line_arrives_at_the_exit() {
        let (reg, mut rx) = registry();
        let task = reg
            .launch_from(r"printf 'a\n 50%%\r 99%%'", None, true, None)
            .expect("launches");
        assert_eq!(output_until_exit(&mut rx), "a\n 99%");
        std::fs::remove_file(&task.output_path).ok();
    }

    #[test]
    fn an_adopted_commands_replay_and_the_rest_fold_as_one_stream() {
        // Ctrl+B mid-bar: the foreground runner read the line up to its
        // 40% frame, the pipe still carries the rest of it — the background
        // stream shows the line once, finished.
        let (reg, mut rx) = registry();
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(r"sleep 0.2; printf '\r100%%\n'")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().expect("spawns");
        let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>();
        if let Some(pipe) = child.stdout.take() {
            let tx = chunk_tx.clone();
            std::thread::spawn(move || drain_pipe(pipe, &tx));
        }
        drop(chunk_tx);
        let task = reg.adopt(
            "fetch",
            None,
            false,
            child,
            chunk_rx,
            b"get\n 10%\r 40%".to_vec(),
        );
        assert_eq!(output_until_exit(&mut rx), "get\n100%\n");
        std::fs::remove_file(&task.output_path).ok();
    }

    // --- TTY sessions (docs/interactive-shell.md) ---

    use crate::pty::keys::{encode, parse_input};
    use crate::pty::report::{Status, Waiting};
    use crate::pty::session::WaitEnd;
    use crate::pty::settle::{Settle, WaitKind};

    const LONG: Duration = Duration::from_secs(10);

    fn never() -> bool {
        false
    }

    /// Type `input` (the `bash_session` notation) into session `id`.
    fn type_into(reg: &BackgroundRegistry, id: &str, input: &str) {
        let io = reg.session(id).expect("a running session").io;
        let chunks = encode(&parse_input(input), io.modes());
        reg.send_input(id, chunks).expect("types");
    }

    /// Drain every event that arrives within `window`.
    fn drain(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<BgEvent>,
        window: Duration,
    ) -> Vec<BgEvent> {
        let deadline = std::time::Instant::now() + window;
        let mut events = Vec::new();
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(event) => events.push(event),
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        events
    }

    #[cfg(unix)]
    #[test]
    fn a_tty_session_answered_and_finished_inside_its_calls_is_never_announced() {
        // The whole life of a prompt, the way `bash` + `bash_session` drive
        // it: launch, settle at the prompt, type the answer, see the exit.
        // Nothing outlived a call, so the event loop never hears of it.
        let (reg, mut rx) = registry();
        let launch = reg
            .launch_tty("printf 'Name? '; read n; echo \"hi $n\"", None, None, false)
            .expect("launches");
        let (task, io) = (launch.task, launch.io);
        let end = io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert_eq!(
            io.look(
                &task.id,
                Status::Running {
                    waiting: Waiting::Input
                }
            ),
            format!("Running (session {}, waiting for input)\nName?", task.id)
        );
        assert_eq!(io.end_wait(), None);

        let since = io.begin_wait();
        type_into(&reg, &task.id, "World\n");
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Exited));
        let code = io.exit().expect("finished");
        assert_eq!(
            io.look(&task.id, Status::Exited(code)),
            "Exit code: 0\nName? World\nhi World"
        );
        let observed = io.end_wait().expect("this call finalizes");
        assert!(observed);
        reg.finalize(&task.id, observed);
        assert!(reg.session(&task.id).is_none(), "the task is gone");
        assert!(
            drain(&mut rx, Duration::from_millis(200)).is_empty(),
            "never announced, so never reported"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_background_session_that_stops_to_ask_is_reported_once() {
        // A session nobody waits on that stops at a question says so
        // (docs/bash-tools.md); one printing a log line does not.
        let (reg, mut rx) = registry();
        let asking = reg
            .launch_tty(
                "printf 'Port 3000 is in use. Use another? (Y/n) '; read answer; sleep 30",
                None,
                None,
                true,
            )
            .expect("launches")
            .task;
        let logging = reg
            .launch_tty("echo 'listening on :3000'; sleep 30", None, None, true)
            .expect("launches")
            .task;
        let events = drain(&mut rx, WAITING_NOTICE_QUIET + Duration::from_secs(2));
        let waiting: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                BgEvent::Waiting { id } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(waiting, [asking.id.as_str()], "{events:?}");
        reg.kill(&asking.id);
        reg.kill(&logging.id);
    }

    #[cfg(unix)]
    #[test]
    fn announcing_a_session_that_outlived_its_call_lists_it_with_its_screen() {
        let (reg, mut rx) = registry();
        let launch = reg
            .launch_tty(
                "printf 'ready> '; sleep 30",
                Some("wait".into()),
                None,
                false,
            )
            .expect("launches");
        let (task, io) = (launch.task, launch.io);
        let _ = io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        let _ = io.end_wait();
        assert!(drain(&mut rx, Duration::from_millis(100)).is_empty());
        reg.announce(&task.id);
        let events = drain(&mut rx, Duration::from_millis(200));
        assert!(
            matches!(&events[0], BgEvent::Started { id, from_model: true, .. } if *id == task.id),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, BgEvent::Screen { text, .. } if text == "ready>")),
            "the screen as it stands: {events:?}"
        );
        reg.announce(&task.id);
        assert!(
            drain(&mut rx, Duration::from_millis(100)).is_empty(),
            "announced once"
        );
        reg.kill(&task.id);
        let events = drain(&mut rx, Duration::from_secs(2));
        assert!(
            events.iter().any(|e| matches!(
                e,
                BgEvent::Exited {
                    killed: true,
                    observed: false,
                    ..
                }
            )),
            "{events:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_exit_a_waiting_call_reports_is_announced_as_observed() {
        let (reg, mut rx) = registry();
        let launch = reg
            .launch_tty("read line; echo got-$line", None, None, true)
            .expect("launches");
        let (task, io) = (launch.task, launch.io);
        let since = io.begin_wait();
        type_into(&reg, &task.id, "x\n");
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Exited));
        let _ = io.look(&task.id, Status::Exited(io.exit().flatten()));
        let observed = io.end_wait().expect("the call finalizes");
        reg.finalize(&task.id, observed);
        let events = drain(&mut rx, Duration::from_millis(300));
        assert!(
            events.iter().any(|e| matches!(
                e,
                BgEvent::Exited {
                    observed: true,
                    code: Some(0),
                    ..
                }
            )),
            "{events:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_session_nobody_waits_on_reports_its_exit_unobserved() {
        let (reg, mut rx) = registry();
        reg.launch_tty("sleep 0.2; echo done", None, None, true)
            .expect("launches");
        let events = drain(&mut rx, Duration::from_secs(2));
        assert!(
            events.iter().any(|e| matches!(
                e,
                BgEvent::Exited {
                    observed: false,
                    code: Some(0),
                    ..
                }
            )),
            "{events:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_task_takes_no_typing_but_does_take_ctrl_c() {
        let (reg, mut rx) = registry();
        let task = reg.launch("sleep 30", None, true).expect("launches");
        let err = reg
            .send_input(&task.id, encode(&parse_input("y\n"), Default::default()))
            .unwrap_err();
        assert!(err.contains("runs on a pipe"), "{err}");
        assert!(
            err.contains("bashkill"),
            "what the model can do instead: {err}"
        );
        // Give the child its `setsid` first: until it leads its own group, a
        // group signal has no group to reach (the model's `<C-c>` comes long
        // after a launch; a test's comes at once).
        std::thread::sleep(Duration::from_millis(300));
        let start = std::time::Instant::now();
        reg.interrupt(&task.id);
        let events = drain(&mut rx, Duration::from_secs(2));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, BgEvent::Exited { killed: false, .. })),
            "SIGINT ends it without counting as a user stop: {events:?}"
        );
        assert!(start.elapsed() < Duration::from_secs(5));
        std::fs::remove_file(&task.output_path).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_session_answers_the_terminal_queries_its_program_sends() {
        // A raw-mode program asks where the cursor is and waits for the
        // answer — which only a terminal (here, the session's screen) gives.
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let (reg, _rx) = registry();
        let launch = reg
            .launch_tty(
                "python3 -c 'import os,sys,tty,termios\n\
                 fd=sys.stdin.fileno(); old=termios.tcgetattr(fd); tty.setraw(fd)\n\
                 os.write(1,b\"ab\\x1b[6n\"); r=b\"\"\n\
                 while not r.endswith(b\"R\"): r+=os.read(fd,1)\n\
                 termios.tcsetattr(fd,termios.TCSADRAIN,old); print(\"\\nreply\", r[2:].decode())'",
                None,
                None,
                false,
            )
            .expect("launches");
        let (task, io) = (launch.task, launch.io);
        let end = io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Exited));
        let look = io.look(&task.id, Status::Exited(io.exit().flatten()));
        assert!(look.contains("reply 1;3R"), "{look}");
        if let Some(observed) = io.end_wait() {
            reg.finalize(&task.id, observed);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_tty_sessions_interim_file_holds_readable_text() {
        let (reg, _rx) = registry();
        let launch = reg
            .launch_tty(
                "printf '\\033[31mred\\033[0m 10%%\\r100%%\\r\\ndone\\r\\n'",
                None,
                None,
                false,
            )
            .expect("launches");
        let (task, io) = (launch.task, launch.io);
        let _ = io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        if let Some(observed) = io.end_wait() {
            reg.finalize(&task.id, observed);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let text = std::fs::read_to_string(&task.output_path).unwrap_or_default();
            if text.contains("done") {
                // `red 10%` overwritten from column 1 by `100%` reads `100%10%` —
                // the colour codes gone, the carriage return applied.
                assert_eq!(text, "100%10%\ndone\n");
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the file fills: {text:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::remove_file(&task.output_path).ok();
    }

    #[cfg(unix)]
    #[test]
    fn the_tty_session_cap_refuses_one_more_and_names_the_running_ones() {
        let (reg, _rx) = registry();
        for _ in 0..MAX_TTY_SESSIONS {
            reg.launch_tty("sleep 30", None, None, true)
                .expect("launches");
        }
        let Err(TtyLaunchError::Refused(err)) = reg.launch_tty("sleep 30", None, None, true) else {
            panic!("one too many is refused");
        };
        assert!(err.contains("already running"), "{err}");
        assert!(
            err.contains("(sleep 30)"),
            "the running ones are listed: {err}"
        );
        assert!(
            err.contains("bashkill"),
            "the tool that frees one, so the model can act on it: {err}"
        );
        assert!(
            reg.launch("sleep 0", None, true).is_ok(),
            "a background pipe task is not a session and is not capped"
        );
        reg.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn a_command_still_inside_its_call_is_not_counted_against_the_cap() {
        // Every command runs in a terminal (docs/bash-tools.md): a batch of
        // subagents' ordinary commands, each still inside its own call, must
        // never be refused for the sessions they are not.
        let (reg, _rx) = registry();
        let mut launches = Vec::new();
        for _ in 0..MAX_TTY_SESSIONS {
            launches.push(
                reg.launch_tty("sleep 30", None, None, false)
                    .expect("launches"),
            );
        }
        assert!(
            reg.launch_tty("sleep 30", None, None, true).is_ok(),
            "none of them has outlived its call"
        );
        reg.kill_all();
        drop(launches);
    }

    #[cfg(unix)]
    #[test]
    fn a_tty_sessions_line_mode_is_read_off_its_terminal() {
        let (reg, _rx) = registry();
        let launch = reg
            .launch_tty("printf 'x? '; read x", None, None, true)
            .expect("launches");
        let (task, io) = (launch.task, launch.io);
        let _ = io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(
            reg.line_mode(&task.id),
            Some(crate::pty::spawn::LineMode {
                canonical: true,
                echo: true,
                processed_output: true,
            })
        );
        let pipe = reg.launch("sleep 30", None, true).expect("launches");
        assert_eq!(reg.line_mode(&pipe.id), None, "a pipe has no terminal");
        assert_eq!(reg.line_mode("bnope"), None);
        reg.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn each_key_goes_once_a_slow_program_has_read_the_last() {
        // Seen with a program taking 50 ms over each key: keys a fixed 20 ms
        // apart ran two and three into one read, which it took for no key.
        // Each read here comes 100 ms after the last and must hold one key.
        use crate::pty::keys::{InputChunk, Pace};
        let (reg, _rx) = registry();
        let launch = reg
            .launch_tty(
                "stty -icanon -echo; echo ready; for i in 1 2 3 4 5; do \
                 n=$(dd bs=64 count=1 2>/dev/null | wc -c); printf '%s ' $n; sleep 0.1; \
                 done; echo done",
                None,
                None,
                false,
            )
            .expect("launches");
        let (task, io) = (launch.task, launch.io);
        let _ = io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        let since = io.begin_wait();
        let chunks = ["a", "b", "c", "d", "e"]
            .iter()
            .enumerate()
            .map(|(i, key)| InputChunk {
                bytes: key.as_bytes().to_vec(),
                then: if i == 4 { Pace::Last } else { Pace::Read },
            })
            .collect();
        reg.send_input(&task.id, chunks).expect("types");
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Exited));
        let report = io.look(&task.id, Status::Exited(io.exit().flatten()));
        assert!(report.contains("1 1 1 1 1 done"), "{report}");
        reg.kill_all();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_sessions_reader_is_its_terminals_foreground_program() {
        use crate::pty::keys::Reader;
        let (reg, _rx) = registry();
        let shell = reg
            .launch_tty("exec bash --norc --noprofile -i", None, None, false)
            .expect("launches");
        let program = reg
            .launch_tty("echo ready; cat", None, None, false)
            .expect("launches");
        for launch in [&shell, &program] {
            let _ = launch.io.wait(
                WaitKind::Launch,
                launch.io.origin_mark(),
                LONG,
                &never,
                &never,
                &mut |_, _| {},
            );
        }
        assert_eq!(reg.reader(&shell.task.id), Reader::Shell);
        assert_eq!(reg.reader(&program.task.id), Reader::Program);
        assert_eq!(reg.reader("bnope"), Reader::Unknown);
        reg.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn a_relay_holding_the_terminal_raw_makes_no_pause_a_prompt() {
        // sudo (ssh, `docker run -it`) holds the terminal raw — output
        // processing off too — for as long as its command runs: a pause
        // between two of the command's lines is still just a pause.
        let (reg, _rx) = registry();
        let launch = reg
            .launch_tty(
                "stty raw -echo; printf 'working\\r\\n'; sleep 0.8; \
                 printf 'still working\\r\\n'; sleep 0.8; printf 'done\\r\\n'",
                None,
                None,
                false,
            )
            .expect("launches");
        let (task, io) = (launch.task, launch.io);
        let end = io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Exited));
        if let Some(observed) = io.end_wait() {
            reg.finalize(&task.id, observed);
        }
        reg.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn a_picker_reading_its_terminal_waits_however_its_line_animated() {
        // A picker that animated its line before asking has the screen of a
        // progress bar — but the kernel sees it blocked reading the
        // terminal (`pty::probe`), and that is the word that counts; typed
        // into, it redraws and waits again.
        let (reg, _rx) = registry();
        let launch = reg
            .launch_tty(
                "stty -icanon -echo; printf 'Pick: Apple'; sleep 0.2; \
                 printf '\\rPick: Banana'; sleep 0.2; printf '\\rPick: Cherry'; \
                 dd bs=1 count=1 >/dev/null 2>&1; printf '\\rPick: Durian'; \
                 dd bs=1 count=1 >/dev/null 2>&1; sleep 30",
                None,
                None,
                false,
            )
            .expect("launches");
        let (task, io) = (launch.task, launch.io);
        let deadline = std::time::Instant::now() + LONG;
        while !io.screen_text().unwrap_or_default().contains("Cherry") {
            assert!(std::time::Instant::now() < deadline, "the picker drew");
            std::thread::sleep(Duration::from_millis(20));
        }
        let since = io.begin_wait();
        std::thread::sleep(crate::pty::settle::PROMPT_QUIET + Duration::from_millis(300));
        assert!(
            io.waiting(WaitKind::Input, io.origin_mark()),
            "a program reading its terminal is waiting, its line animated or not"
        );
        type_into(&reg, &task.id, "x");
        let started = std::time::Instant::now();
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(
            started.elapsed() < crate::pty::settle::LINE_QUIET,
            "{:?}",
            started.elapsed()
        );
        let _ = io.end_wait();
        let _ = io.end_wait();
        reg.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn a_menu_in_raw_mode_settles_as_waiting_for_input() {
        // An arrow-key menu leaves the cursor at the start of a fresh line
        // — no prompt by the cursor rule — but it reads key by key, which
        // the monitor reads off the terminal (`pty::spawn::line_mode`), and
        // the kernel sees it blocked in that read (`pty::probe`).
        let (reg, _rx) = registry();
        let launch = reg
            .launch_tty(
                "stty -icanon -echo; printf 'pick one\\r\\n> a\\r\\n  b\\r\\n'; \
                 dd bs=1 count=1 >/dev/null 2>&1; sleep 30",
                None,
                None,
                false,
            )
            .expect("launches");
        let (task, io) = (launch.task, launch.io);
        let started = std::time::Instant::now();
        let end = io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(
            started.elapsed() < crate::pty::settle::LINE_QUIET,
            "a prompt, not the line-quiet fallback: {:?}",
            started.elapsed()
        );
        if let Some(observed) = io.end_wait() {
            reg.finalize(&task.id, observed);
        }
        reg.kill_all();
    }
}
