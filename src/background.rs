//! Background shell processes — the boundary registry behind
//! `run_in_background`, Ctrl+B, and the ↓ manager (see `docs/background.md`).
//!
//! A [`BackgroundRegistry`] is a cloneable handle shared by the event loop,
//! the model-tool executor ([`crate::llm::exec`]), and the `!` shell runner
//! (`main.rs`). Launching (or adopting, for Ctrl+B) a command spawns a
//! **monitor thread** that owns the child: it merges the stdout/stderr pipes
//! in arrival order, streams completed lines as [`BgEvent::Output`], tees
//! every byte to the task's `{id}.output` file (so the model can `read`
//! interim output), and reports [`BgEvent::Exited`] when the child dies.
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
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::stream::CancelToken;

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
    },
    /// A chunk of a running shell's output (completed lines where possible;
    /// an adopted command's prior output arrives as one leading chunk).
    Output { id: String, chunk: String },
    /// The shell exited: its `code` (`None` when a signal killed it) and
    /// whether the registry's own [`BackgroundRegistry::kill`] did it.
    Exited {
        id: String,
        code: Option<i32>,
        killed: bool,
    },
}

/// A successfully launched background task, for the model-facing tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchedTask {
    /// The task id (`bash_1`, …) the model uses to refer to it.
    pub id: String,
    /// Where the interim output streams — the model can `read` it mid-run.
    pub output_path: PathBuf,
}

/// How often a monitor thread wakes to poll its child / kill flag when no
/// output is arriving — short enough that exits and kills surface promptly.
const MONITOR_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A monitor's line-assembly buffer never holds more than this: a command
/// that emits an enormous line without a newline is force-flushed as a chunk
/// so memory stays bounded (the App caps its own display tail; the full
/// output is on disk anyway).
const MAX_PARTIAL_LINE_BYTES: usize = 64 * 1024;

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
}

struct Inner {
    next_id: u64,
    tasks: HashMap<String, Task>,
    /// The Ctrl+B latch: the loop raises it while a foreground command runs;
    /// the runner's poll loop consumes it (see `docs/background.md`).
    background_request: bool,
    /// Where the `{id}.output` interim files live.
    dir: PathBuf,
}

/// The shared background-shell registry (see the module docs).
#[derive(Clone)]
pub struct BackgroundRegistry {
    inner: Arc<Mutex<Inner>>,
    events: UnboundedSender<BgEvent>,
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
                dir,
            })),
            events,
        }
    }

    /// Spawn `command` as a background task: `sh -c` in its own process group
    /// (so a kill reaps the whole tree), monitored on its own thread. Returns
    /// the task id + interim-output path for the model-facing result.
    ///
    /// # Errors
    /// The spawn error text when the shell can't start.
    pub fn launch(
        &self,
        command: &str,
        description: Option<String>,
        from_model: bool,
    ) -> Result<LaunchedTask, String> {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Its own process group (pgid == pid) so kill() reaps grandchildren
        // too — the `llm::exec` pattern.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd
            .spawn()
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

        Ok(self.register(
            command,
            description,
            from_model,
            child,
            chunk_rx,
            Vec::new(),
        ))
    }

    /// Adopt a **running** foreground command (the Ctrl+B transfer): take
    /// ownership of its child and pipe channel mid-run, replaying the output
    /// read so far (`prior`) into the task's stream + file so nothing is
    /// lost. The caller's own pipe-reader threads keep feeding `chunk_rx` and
    /// exit at EOF on their own. See `docs/background.md`.
    pub fn adopt(
        &self,
        command: &str,
        description: Option<String>,
        from_model: bool,
        child: Child,
        chunk_rx: mpsc::Receiver<Vec<u8>>,
        prior: Vec<u8>,
    ) -> LaunchedTask {
        self.register(command, description, from_model, child, chunk_rx, prior)
    }

    /// The shared tail of [`launch`]/[`adopt`]: allocate the id, open the
    /// interim-output file, record the task, announce it, and hand the child
    /// to its monitor thread.
    ///
    /// [`launch`]: BackgroundRegistry::launch
    /// [`adopt`]: BackgroundRegistry::adopt
    fn register(
        &self,
        command: &str,
        description: Option<String>,
        from_model: bool,
        child: Child,
        chunk_rx: mpsc::Receiver<Vec<u8>>,
        prior: Vec<u8>,
    ) -> LaunchedTask {
        let kill = CancelToken::new();
        let (id, output_path) = {
            let mut inner = self.inner.lock().expect("registry lock");
            inner.next_id += 1;
            let id = format!("bash_{}", inner.next_id);
            let path = inner.dir.join(format!("{id}.output"));
            inner.tasks.insert(
                id.clone(),
                Task {
                    pgid: child.id(),
                    kill: kill.clone(),
                    killed: false,
                },
            );
            (id, path)
        };
        // Best-effort tee file — a failure only loses the interim read path.
        if let Some(parent) = output_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut file = File::create(&output_path).ok();
        let _ = self.events.send(BgEvent::Started {
            id: id.clone(),
            command: command.to_string(),
            description,
            from_model,
        });
        if !prior.is_empty() {
            if let Some(f) = file.as_mut() {
                let _ = f.write_all(&prior);
            }
            let _ = self.events.send(BgEvent::Output {
                id: id.clone(),
                chunk: String::from_utf8_lossy(&prior).into_owned(),
            });
        }
        let monitor = MonitorHandle {
            inner: Arc::clone(&self.inner),
            events: self.events.clone(),
            id: id.clone(),
        };
        std::thread::spawn(move || monitor.run(child, chunk_rx, kill, file));
        LaunchedTask { id, output_path }
    }

    /// Stop a background task: mark it user-killed and SIGKILL its whole
    /// process group (synchronously — plus the monitor's kill token as the
    /// portable backstop). The monitor then reports `Exited {killed: true}`.
    pub fn kill(&self, id: &str) {
        let pgid = {
            let mut inner = self.inner.lock().expect("registry lock");
            let Some(task) = inner.tasks.get_mut(id) else {
                return;
            };
            task.killed = true;
            task.kill.cancel();
            task.pgid
        };
        kill_group(pgid);
    }

    /// Stop every running task — the quit and `/clear` sweep. Synchronous
    /// (direct group kills), so the quit path can't orphan a `ping`.
    pub fn kill_all(&self) {
        let pgids: Vec<u32> = {
            let mut inner = self.inner.lock().expect("registry lock");
            inner
                .tasks
                .values_mut()
                .map(|task| {
                    task.killed = true;
                    task.kill.cancel();
                    task.pgid
                })
                .collect()
        };
        for pgid in pgids {
            kill_group(pgid);
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
}

/// The monitor thread's registry access, bundled.
struct MonitorHandle {
    inner: Arc<Mutex<Inner>>,
    events: UnboundedSender<BgEvent>,
    id: String,
}

impl MonitorHandle {
    /// Own the child to its end: merge + forward output, poll the kill token,
    /// and report the exit. See the module docs.
    fn run(
        self,
        mut child: Child,
        chunk_rx: mpsc::Receiver<Vec<u8>>,
        kill: CancelToken,
        mut file: Option<File>,
    ) {
        let mut partial: Vec<u8> = Vec::new();
        let status = loop {
            while let Ok(chunk) = chunk_rx.try_recv() {
                self.absorb(&chunk, &mut partial, file.as_mut());
            }
            if kill.is_cancelled() {
                kill_group(child.id());
            }
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => match chunk_rx.recv_timeout(MONITOR_POLL_INTERVAL) {
                    Ok(chunk) => self.absorb(&chunk, &mut partial, file.as_mut()),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(MONITOR_POLL_INTERVAL);
                    }
                },
                Err(_) => break None,
            }
        };
        // Reap any straggler holding the pipes (the group kill is idempotent),
        // then flush what the readers still buffered plus the trailing
        // partial line.
        kill_group(child.id());
        let _ = child.kill();
        let _ = child.wait();
        while let Ok(chunk) = chunk_rx.try_recv() {
            self.absorb(&chunk, &mut partial, file.as_mut());
        }
        if !partial.is_empty() {
            self.send_output(&partial);
            partial.clear();
        }
        let killed = {
            let mut inner = self.inner.lock().expect("registry lock");
            inner.tasks.remove(&self.id).is_some_and(|task| task.killed)
        };
        let _ = self.events.send(BgEvent::Exited {
            id: self.id,
            code: status.and_then(|s| s.code()),
            killed,
        });
    }

    /// Tee `chunk` to the output file and forward every newly-completed line
    /// (bounding the partial-line buffer at [`MAX_PARTIAL_LINE_BYTES`]).
    fn absorb(&self, chunk: &[u8], partial: &mut Vec<u8>, file: Option<&mut File>) {
        if let Some(f) = file {
            let _ = f.write_all(chunk);
        }
        partial.extend_from_slice(chunk);
        if let Some(pos) = partial.iter().rposition(|&b| b == b'\n') {
            let complete: Vec<u8> = partial.drain(..=pos).collect();
            self.send_output(&complete);
        }
        if partial.len() > MAX_PARTIAL_LINE_BYTES {
            let overflow = std::mem::take(partial);
            self.send_output(&overflow);
        }
    }

    fn send_output(&self, bytes: &[u8]) {
        let _ = self.events.send(BgEvent::Output {
            id: self.id.clone(),
            chunk: String::from_utf8_lossy(bytes).into_owned(),
        });
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
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!("kill -KILL -{pgid} 2>/dev/null"))
        .status();
}

/// Non-unix fallback: no process groups — the monitor's own `child.kill()`
/// (via the kill token) reaps the direct child.
#[cfg(not(unix))]
fn kill_group(_pgid: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> (
        BackgroundRegistry,
        tokio::sync::mpsc::UnboundedReceiver<BgEvent>,
    ) {
        // A unique dir per test: the suite runs in parallel and every fresh
        // registry starts its ids at `bash_1`, so a shared dir would collide.
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let dir =
            std::env::temp_dir().join(format!("inline-tui-bg-test-{}-{seq}", std::process::id()));
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
    fn launch_streams_started_output_and_a_clean_exit() {
        let (reg, mut rx) = registry();
        let task = reg
            .launch("printf 'a\\nb\\n'", Some("print two lines".into()), true)
            .expect("launches");
        assert_eq!(task.id, "bash_1");

        let started = next(&mut rx);
        assert_eq!(
            started,
            BgEvent::Started {
                id: "bash_1".into(),
                command: "printf 'a\\nb\\n'".into(),
                description: Some("print two lines".into()),
                from_model: true,
            }
        );
        // Output (possibly split across chunks) then a clean exit.
        let mut output = String::new();
        loop {
            match next(&mut rx) {
                BgEvent::Output { id, chunk } => {
                    assert_eq!(id, "bash_1");
                    output.push_str(&chunk);
                }
                BgEvent::Exited { id, code, killed } => {
                    assert_eq!(id, "bash_1");
                    assert_eq!(code, Some(0));
                    assert!(!killed);
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
}
