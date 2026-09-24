//! The real tool executor — the I/O boundary that actually runs a `bash`
//! command and reads/writes the filesystem for the model's tool calls (see
//! `docs/tools.md`). The pure cores it wraps (arg parse, the edit engine, the
//! read formatter, the diff) live in [`crate::llm::tools`].
//!
//! Boundary code like `main.rs`/`term.rs`: the process/file I/O is verified by
//! hand and `scripts/smoke.sh`, but the deterministic file operations
//! (read/write/edit round-trips) carry focused tests over temp files.

use std::io::Read;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::tools::{
    self, BashArgs, EditArgs, ReadArgs, TOOL_OUTPUT_MAX_BYTES, ToolCallRequest, ToolOutcome,
    WriteArgs,
};
use crate::background::{BackgroundRegistry, BgOrigin};
use crate::stream::CancelToken;

/// Runs the model's tool calls. Implemented by [`RealToolExecutor`] in
/// production and by fakes in the agent-loop tests.
pub trait ToolExecutor {
    /// Execute one tool call, returning the model-facing outcome. Never
    /// panics — every failure becomes a non-ok [`ToolOutcome`] the model reads
    /// and can recover from.
    ///
    /// `on_output` is a sink for the tool's **live output** ([`ToolProgress`]):
    /// a streaming tool (`bash`, `bash_session`) calls it as the command runs,
    /// so the TUI can tail the running cell (surfaced as
    /// [`ToolScreen`](crate::stream::StreamEvent::ToolScreen); see
    /// `docs/tool-streaming.md`). A tool that produces its result all at once
    /// (`read`/`write`/`edit`) simply never calls it — the full result still
    /// comes back in the returned [`ToolOutcome`].
    fn execute(
        &self,
        call: &ToolCallRequest,
        cancel: &CancelToken,
        on_output: &mut dyn FnMut(ToolProgress<'_>),
    ) -> ToolOutcome;
}

/// What a running tool reports while it runs — the executor's live sink
/// ([`ToolExecutor::execute`]'s `on_output`), mapped onto the matching
/// [`crate::stream::StreamEvent`] by the agent loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolProgress<'a> {
    /// A command's output as it builds up: `settled` text to append for
    /// good, `live` rows that replace the previous `live` — the line a plain
    /// command is still drawing, the rows a terminal can still redraw
    /// ([`crate::stream::StreamEvent::ToolScreen`],
    /// `docs/interactive-shell.md`).
    Screen { settled: &'a str, live: &'a str },
    /// The call's header, refined
    /// ([`crate::stream::StreamEvent::ToolTitle`]).
    Title(&'a str),
}

/// How often the `bash` runner polls a running child for completion, a cancel,
/// or a timeout — short enough that Esc / a timeout reaps promptly.
const BASH_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The real executor: runs commands under `sh -c` and touches the filesystem in
/// the app's working directory. Stateless — paths resolve against the process
/// cwd, the same trust model as the `!` shell (`docs/shell-command.md`).
/// An attached [`BackgroundRegistry`] enables `run_in_background` and the
/// Ctrl+B handoff for `bash` (see `docs/background.md`); without one those
/// resolve as recoverable errors / are unavailable.
#[derive(Debug, Clone, Default)]
pub struct RealToolExecutor {
    background: Option<BackgroundRegistry>,
    /// The launching **subagent**, when this executor runs one's tool calls
    /// (`docs/agent-tool.md`): its `run_in_background` launches carry this
    /// origin into the shared shell list, and the Ctrl+B latch is left alone
    /// (the handoff belongs to the main turn's foreground command — a
    /// subagent's concurrent `bash` must never steal it).
    bg_origin: Option<BgOrigin>,
    /// The terminal-detach helper (`crate::subprocess`): the TUI's own binary,
    /// available as the re-exec fallback tier when the `setsid` binary is
    /// absent (macOS, minimal images) — so `bash` children run with **no
    /// controlling terminal** and a `/dev/tty` password prompt (`sudo`) fails
    /// fast instead of hijacking the TUI. `None` (tests, embedders without
    /// the hook) just shortens the tier chain.
    detach_helper: Option<std::path::PathBuf>,
    /// The active model's image-input support (`ModelConfig::vision`):
    /// `Some(false)` makes an image `read` fail as a recoverable error
    /// instead of attaching pixels the provider would reject with the whole
    /// turn (`docs/tools.md`). `None`/`Some(true)` attach as normal.
    vision: Option<bool>,
}

impl RealToolExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach the shared background registry (the production path — `main.rs`
    /// threads it through the backend). See `docs/background.md`.
    #[must_use]
    pub fn with_background(mut self, registry: BackgroundRegistry) -> Self {
        self.background = Some(registry);
        self
    }

    /// Attribute this executor's background launches to a **subagent**
    /// (`docs/agent-tool.md`): its `run_in_background` shells join the same
    /// shared list — stacking into the footer count — tagged with the
    /// launcher, while the Ctrl+B handoff latch stays untouched (it belongs
    /// to the main turn's foreground command).
    #[must_use]
    pub fn with_background_origin(mut self, origin: BgOrigin) -> Self {
        self.bg_origin = Some(origin);
        self
    }

    /// Route `bash` children through the terminal-detach helper (production —
    /// `main.rs` resolves its own binary once at startup and the backend
    /// copies it off the registry). See `crate::subprocess`.
    #[must_use]
    pub fn with_detach_helper(mut self, helper: Option<std::path::PathBuf>) -> Self {
        self.detach_helper = helper;
        self
    }

    /// Tell the executor the active model's image-input support (the backend
    /// copies it off its `ModelConfig`). See `docs/tools.md`.
    #[must_use]
    pub fn with_vision(mut self, vision: Option<bool>) -> Self {
        self.vision = vision;
        self
    }
}

impl ToolExecutor for RealToolExecutor {
    fn execute(
        &self,
        call: &ToolCallRequest,
        cancel: &CancelToken,
        on_output: &mut dyn FnMut(ToolProgress<'_>),
    ) -> ToolOutcome {
        match call.name.as_str() {
            "bash" => run_bash(
                &call.arguments,
                cancel,
                self.background.as_ref(),
                self.bg_origin.as_ref(),
                self.detach_helper.as_deref(),
                on_output,
            ),
            super::tools::BASH_SESSION_TOOL_NAME => run_bash_session(
                &call.arguments,
                cancel,
                self.background.as_ref(),
                self.bg_origin.as_ref(),
                on_output,
            ),
            "read" => run_read(&call.arguments, self.vision),
            "write" => run_write(&call.arguments),
            "edit" => run_edit(&call.arguments),
            other => ToolOutcome::error(format!("unknown tool: {other}")),
        }
    }
}

/// The model-facing result of a backgrounded `bash` call — what the tool
/// result says (the cell shows the fixed backgrounded row instead): the
/// session id `bash_session` takes back (to wait on it, interrupt it, or end
/// it — `docs/interactive-shell.md`), the interim-output file to `read`
/// mid-run, and the promise of the final output. See `docs/background.md`.
#[must_use]
pub fn background_launch_text(task: &crate::background::LaunchedTask) -> String {
    format!(
        "Command running in the background as session {}. Output is streaming \
         to {} — check on it with bash_session, or read that file.\n\
         You will be notified with the final output when it finishes.",
        task.id,
        task.output_path.display(),
    )
}

/// The model-facing result of a `tty` + `run_in_background` launch: a
/// terminal the model can come back to, by its session id
/// (`docs/interactive-shell.md`).
#[must_use]
pub fn tty_background_launch_text(task: &crate::background::LaunchedTask) -> String {
    format!(
        "Command started in a terminal in the background as session {}. Use \
         bash_session with this session_id to type into it, read what it \
         printed, or kill it once you no longer need it.\n\
         You will be notified with its final output when it exits.",
        task.id,
    )
}

/// The model-facing result of a **Ctrl+B handoff** of a `tty` command that
/// had not yet settled: the user moved it on, the session keeps running,
/// and the model can come back to it (`docs/interactive-shell.md`).
#[must_use]
pub fn tty_handoff_text(session: &str) -> String {
    format!(
        "The user moved this command to the background while it was running — \
         it has not failed and keeps running as session {session}. Use \
         bash_session with this session_id to see its output, type into it, \
         or kill it once you no longer need it. Do not run the command again."
    )
}

/// The model-facing result of a **Ctrl+B handoff** — the *user* moved a
/// foreground `bash` call to the background mid-run. Unlike
/// [`background_launch_text`] (a `run_in_background` launch the model asked
/// for), the model here requested a foreground run and expects the full
/// output in this result — so the text must say who moved it and steer the
/// model off waiting, or it treats the acknowledgement as an anomaly and
/// re-reads the interim file round after round. The launch facts (interim
/// path, completion promise) are embedded verbatim so the two variants can
/// never drift. See `docs/background.md`.
#[must_use]
pub fn background_handoff_text(task: &crate::background::LaunchedTask) -> String {
    format!(
        "The user moved this command to the background while it was running — \
         it has not failed and keeps running there, so its remaining output \
         will not arrive in this result.\n\
         {}\n\
         Do not run the command again, and do not wait for it by repeatedly \
         reading the interim file — continue with the rest of the task, or \
         end the turn.",
        background_launch_text(task),
    )
}

/// A one-tool argument-parse error → a model-facing failure outcome.
fn arg_error(err: String) -> ToolOutcome {
    ToolOutcome::error(err)
}

/// `bash`: run the command under `sh -c`, capture combined stdout+stderr
/// (merged in arrival order, **folded** the way a terminal would show it and
/// byte-capped — [`PipeOutput`]), enforce the per-call timeout, and kill on
/// cancel. As output arrives it is **streamed** through `on_output` so the TUI
/// tails the running cell (`docs/tool-streaming.md`); the full output is still
/// framed as codex does (`Exit code: N` + output) for the model — a non-zero
/// exit or a timeout resolves the cell red.
fn run_bash(
    arguments: &str,
    cancel: &CancelToken,
    background: Option<&BackgroundRegistry>,
    origin: Option<&BgOrigin>,
    detach_helper: Option<&Path>,
    on_output: &mut dyn FnMut(ToolProgress<'_>),
) -> ToolOutcome {
    let args: BashArgs = match tools::parse_args(arguments) {
        Ok(a) => a,
        Err(e) => return arg_error(e),
    };
    // A Ctrl+B pressed before this command started belongs to nothing — drop
    // it so it can't instantly background us (docs/background.md). The latch
    // is the MAIN turn's alone: a subagent's executor (origin set) leaves it
    // for the foreground command it actually belongs to.
    if let Some(registry) = background
        && origin.is_none()
    {
        registry.clear_background_request();
    }
    // `run_in_background`: hand the whole run to the registry and return the
    // launch text at once — the model gets the interim-output path, and the
    // completion notification follows when the command exits. A subagent's
    // launch is attributed via `origin` (docs/agent-tool.md).
    // `tty`: the command gets a terminal of its own and the call returns
    // once it exits or waits for input (docs/interactive-shell.md).
    if args.tty {
        let Some(registry) = background else {
            return ToolOutcome::error(
                "interactive sessions are not available here — run the command without tty",
            );
        };
        return run_tty(&args, cancel, registry, origin, on_output);
    }
    if args.run_in_background {
        let Some(registry) = background else {
            return ToolOutcome::error(
                "run_in_background is not available here — run the command in the foreground",
            );
        };
        return match registry.launch_from(
            &args.command,
            args.description.clone(),
            true,
            origin.cloned(),
        ) {
            Ok(task) => ToolOutcome::backgrounded(task.id.clone(), background_launch_text(&task)),
            Err(err) => ToolOutcome::error(err),
        };
    }
    let timeout = Duration::from_millis(args.timeout_ms());

    // Group + terminal membership come from `subprocess::spawn_detached_shell`
    // (stdio too — stdin null, stdout/stderr piped): the child leads its own
    // process group (pgid == pid — a command that forks or backgrounds a
    // grandchild leaves it holding the stdout/stderr pipe, and only a
    // **group** kill reaps the whole tree, so the reader-thread joins can't
    // hang) and has **no controlling terminal** — a `/dev/tty` password
    // prompt (`sudo`) errors at once instead of hijacking the TUI (see
    // `crate::subprocess`, docs/tools.md).
    let mut child = match crate::subprocess::spawn_detached_shell(detach_helper, &args.command) {
        Ok(child) => child,
        Err(err) => return ToolOutcome::error(format!("failed to run command: {err}")),
    };

    // Drain both pipes on their own threads (so a chatty command can't deadlock
    // on a full pipe), forwarding raw chunks over a channel; the poll loop below
    // folds them in arrival order ([`PipeOutput`]) and streams the running
    // cell through `on_output`.
    let cap = TOOL_OUTPUT_MAX_BYTES;
    let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>();
    let out_pipe = child.stdout.take();
    let out_tx = chunk_tx.clone();
    let out_reader = std::thread::spawn(move || {
        if let Some(pipe) = out_pipe {
            drain_pipe(pipe, &out_tx);
        }
    });
    let err_pipe = child.stderr.take();
    let err_tx = chunk_tx.clone();
    let err_reader = std::thread::spawn(move || {
        if let Some(pipe) = err_pipe {
            drain_pipe(pipe, &err_tx);
        }
    });
    drop(chunk_tx); // only the readers hold senders now, so the channel ends at EOF

    let mut output = PipeOutput::new(cap);
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        // Absorb whatever output is available right now.
        while let Ok(chunk) = chunk_rx.try_recv() {
            output.absorb(&chunk, on_output);
        }
        if cancel.is_cancelled() {
            crate::subprocess::kill_process_group(&mut child);
            // The interrupt path owns the UI; return a terse outcome (the loop
            // discards it — the channel is already swapped). The group kill has
            // reaped any straggler, so the detached readers finish on their own.
            return ToolOutcome::error("Interrupted by user");
        }
        // Ctrl+B: hand the run off to the background registry mid-flight — it
        // replays what we already read (as read) and keeps streaming from
        // our pipe channel; the detached reader threads keep feeding it and
        // exit at EOF on their own. The call resolves as backgrounded, and
        // the agent loop keeps going with the HANDOFF text as the tool result
        // — the model asked for a foreground run, so it must be told the user
        // moved the command (not read a launch acknowledgement it never
        // requested). See `docs/background.md`. A subagent's executor never
        // consumes the latch — Ctrl+B targets the main turn's command (or its
        // agent group's wait loop), not whatever bash a subagent happens to
        // be running concurrently.
        if origin.is_none()
            && let Some(registry) = background
            && registry.take_background_request()
        {
            let task = registry.adopt(
                &args.command,
                args.description.clone(),
                true,
                child,
                chunk_rx,
                output.into_raw(),
            );
            return ToolOutcome::backgrounded(task.id.clone(), background_handoff_text(&task));
        }
        if start.elapsed() >= timeout {
            crate::subprocess::kill_process_group(&mut child);
            timed_out = true;
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            // Wait briefly for the next chunk (so we tail promptly) or wake to
            // re-poll the cancel/timeout above; `Disconnected` (both readers done
            // before the child is reaped) just paces the re-poll.
            Ok(None) => match chunk_rx.recv_timeout(BASH_POLL_INTERVAL) {
                Ok(chunk) => output.absorb(&chunk, on_output),
                // A quiet moment still delivers what the pacing held back.
                Err(mpsc::RecvTimeoutError::Timeout) => output.stream(on_output, false),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    output.stream(on_output, false);
                    std::thread::sleep(BASH_POLL_INTERVAL);
                }
            },
            Err(err) => {
                crate::subprocess::kill_process_group(&mut child);
                return ToolOutcome::error(format!("error waiting on command: {err}"));
            }
        }
    };
    // Even on a clean exit, reap any process the command backgrounded — it holds
    // the pipe open, so joining the readers below would otherwise block on it.
    crate::subprocess::kill_process_group(&mut child);
    let _ = out_reader.join();
    let _ = err_reader.join();
    // Drain any output buffered after the last poll, then settle the line
    // still being drawn (never newline-terminated) so `on_output` has seen it
    // all.
    while let Ok(chunk) = chunk_rx.try_recv() {
        output.absorb(&chunk, on_output);
    }
    let (combined, truncated) = output.finish(on_output);

    if timed_out {
        let body = tools::format_exec_output(None, &combined);
        return ToolOutcome::error(format!(
            "command timed out after {} ms\n{body}",
            timeout.as_millis()
        ))
        .with_truncated(truncated);
    }
    let exit_code = status.as_ref().and_then(std::process::ExitStatus::code);
    let ok = status
        .as_ref()
        .is_some_and(std::process::ExitStatus::success);
    let output = tools::format_exec_output(exit_code, &combined);
    // A command that failed saying it wanted a terminal: the model reads a
    // pointer to `tty` beside the output, where it decides what to do next —
    // the cell keeps the command's own words (docs/interactive-shell.md).
    // Only with a registry, the one place a `tty` session can live.
    let context = (!ok && background.is_some())
        .then(|| tools::tty_hint(&output))
        .flatten()
        .map(|hint| format!("{}\n\n{hint}", output.trim_end()));
    ToolOutcome {
        output,
        ok,
        truncated,
        background: None,
        image: None,
        tasks: None,
        context,
    }
}

/// `bash` with `tty` (`docs/interactive-shell.md`): start the command in a
/// terminal of its own and wait for it to **settle** — exit, stop at a
/// prompt, go quiet, or reach `timeout`. A command that finished reports
/// exactly as a plain `bash` call would; one still running is announced to
/// the event loop (the footer, the ↓ manager) and reported under its
/// session's frame, for `bash_session` to continue. Esc takes the session
/// down with the call — it never outlived it; Ctrl+B (the main turn's alone)
/// hands it to the background as it stands.
fn run_tty(
    args: &BashArgs,
    cancel: &CancelToken,
    registry: &BackgroundRegistry,
    origin: Option<&BgOrigin>,
    on_output: &mut dyn FnMut(ToolProgress<'_>),
) -> ToolOutcome {
    use crate::pty::report::{Status, Waiting};
    use crate::pty::session::WaitEnd;
    use crate::pty::settle::{Settle, WaitKind};
    let background = args.run_in_background;
    let launch = match registry.launch_tty(
        &args.command,
        args.description.clone(),
        origin.cloned(),
        background,
    ) {
        Ok(launch) => launch,
        Err(err) => return ToolOutcome::error(err),
    };
    let (task, io) = (launch.task, launch.io);
    if background {
        return ToolOutcome::backgrounded(task.id.clone(), tty_background_launch_text(&task));
    }
    // The session was created with this call as its waiter, so even a
    // command that exits before the wait below begins leaves its exit here.
    let handoff = || origin.is_none() && registry.take_background_request();
    // The running cell shows what the report will say as it builds up —
    // settled lines appended, the rows in reach redrawn in place.
    let on_output =
        &mut |settled: &str, live: &str| on_output(ToolProgress::Screen { settled, live });
    let end = io.wait(
        WaitKind::Launch,
        io.origin_mark(),
        Duration::from_millis(args.timeout_ms()),
        &|| cancel.is_cancelled(),
        &handoff,
        on_output,
    );
    let outcome = match end {
        WaitEnd::Cancelled => {
            registry.kill(&task.id);
            ToolOutcome::error("Interrupted by user")
        }
        WaitEnd::Handoff => {
            registry.announce(&task.id);
            ToolOutcome::backgrounded(task.id.clone(), tty_handoff_text(&task.id))
        }
        WaitEnd::Settled(Settle::Exited) => {
            let code = io.exit().flatten();
            let report = io.look(&task.id, Status::Exited(code));
            ToolOutcome {
                ok: code == Some(0),
                ..ToolOutcome::ok(report)
            }
        }
        WaitEnd::Settled(settled) => {
            registry.announce(&task.id);
            let at_prompt =
                settled == Settle::Prompt || io.waiting(WaitKind::Launch, io.origin_mark());
            ToolOutcome::ok(io.look(
                &task.id,
                Status::Running {
                    waiting: Waiting::of(at_prompt, io.password_prompt()),
                },
            ))
        }
    };
    if let Some(observed) = io.end_wait() {
        registry.finalize(&task.id, observed);
    }
    outcome
}

/// `bash_session` (`docs/interactive-shell.md`): type into a session, wait
/// on it, or kill it — then report what it printed since the model's last
/// look, under the frame saying where it stands. Works on every shell the
/// registry holds: a TTY session takes typed input, a background pipe
/// command only a `<C-c>` (as `SIGINT`). Esc stops the *waiting*, never the
/// session — it outlived its launch, and the ↓ manager is where the user
/// stops it.
fn run_bash_session(
    arguments: &str,
    cancel: &CancelToken,
    background: Option<&BackgroundRegistry>,
    origin: Option<&BgOrigin>,
    on_output: &mut dyn FnMut(ToolProgress<'_>),
) -> ToolOutcome {
    use crate::pty::keys::{
        encode, html_escaped, is_interrupt, leaves_line_open, parse_input, strip_output_codes,
        typed_tail,
    };
    use crate::pty::report::{
        HTML_ESCAPED_NOTE, NOT_KILLED_NOTE, Status, UNSUBMITTED_NOTE, WAIT_ENDED_NOTE, Waiting,
        dropped_codes_note,
    };
    use crate::pty::session::WaitEnd;
    use crate::pty::settle::{Settle, WaitKind};
    let args: super::tools::SessionArgs = match tools::parse_args(arguments) {
        Ok(a) => a,
        Err(e) => return arg_error(e),
    };
    let Some(registry) = background else {
        return ToolOutcome::error("there are no sessions here — bash with tty is not available");
    };
    let id = args.session_id.trim();
    let Some(session) = registry.session(id) else {
        return ToolOutcome::error(unknown_session_text(id, &registry.sessions()));
    };
    let io = session.io;
    let mut parts = parse_input(args.input.as_deref().unwrap_or_default());
    // Codes a terminal writes to programs are no keys: a program would take
    // them as typing (docs/interactive-shell.md).
    let dropped = strip_output_codes(&mut parts);
    // The header names the command the keys go to
    // (docs/interactive-shell.md).
    on_output(ToolProgress::Title(&tools::session_title(
        &session.command,
        args.input.as_deref(),
        args.kill,
    )));
    let on_output =
        &mut |settled: &str, live: &str| on_output(ToolProgress::Screen { settled, live });
    if !parts.is_empty() && !io.is_tty() && !is_interrupt(&parts) {
        return ToolOutcome::error(format!(
            "session {id} has no terminal to type into — it was not started with tty: \
             true (its stdin is /dev/null); you can still wait on it, send <C-c>, or kill it"
        ));
    }
    // A waiter before anything is done to the session, so an exit the call
    // causes — typing `exit`, a kill — is this call's to report
    // (`pty::session`).
    let since = io.begin_wait();
    let cancelled = || cancel.is_cancelled();
    // Ctrl+B on a waiting call just ends the wait — the session is already
    // in the background; only the main turn's call answers the latch.
    if origin.is_none() {
        registry.clear_background_request();
    }
    let handoff = || origin.is_none() && registry.take_background_request();
    let end = if parts.is_empty() {
        // A bare kill has nothing to wait for before it; a bare wait waits.
        (!args.kill).then(|| {
            io.wait(
                WaitKind::Wait,
                since,
                Duration::from_millis(args.timeout_ms()),
                &cancelled,
                &handoff,
                on_output,
            )
        })
    } else {
        if io.is_tty() {
            // What the screen shows, and what only the terminal knows: whether
            // the program reads key by key, and who it is.
            let mut modes = io.modes();
            modes.key_by_key = registry.line_mode(id).is_some_and(|mode| !mode.canonical);
            modes.reader = registry.reader(id);
            let chunks = encode(&parts, modes);
            if let Err(err) = registry.send_input(id, chunks) {
                if let Some(observed) = io.end_wait() {
                    registry.finalize(id, observed);
                }
                return ToolOutcome::error(err);
            }
        } else {
            registry.interrupt(id);
        }
        // Input then `kill` means "type this, then end it": the answer is
        // given its chance to land before the kill — and a command it ends by
        // itself reports its own exit.
        Some(io.wait(
            WaitKind::Input,
            since,
            Duration::from_millis(args.timeout_ms()),
            &cancelled,
            &handoff,
            on_output,
        ))
    };
    // Where the session stands now: at a prompt even when the wait ended on
    // its timeout (a poll that saw nothing new), since the report must keep
    // saying so.
    let kind = if parts.is_empty() {
        WaitKind::Wait
    } else {
        WaitKind::Input
    };
    let at_prompt =
        matches!(end, Some(WaitEnd::Settled(Settle::Prompt))) || io.waiting(kind, since);
    let waiting = Waiting::of(at_prompt, io.password_prompt());
    // A `kill` sent with an answer the program met by asking for more is
    // not carried out: the program is waiting on the model, not done.
    let typed = !parts.is_empty();
    let spare = args.kill && typed && at_prompt;
    let status = match end {
        Some(WaitEnd::Cancelled) => {
            if let Some(observed) = io.end_wait() {
                registry.finalize(id, observed);
            }
            return ToolOutcome::error("Interrupted by user");
        }
        Some(WaitEnd::Settled(Settle::Exited)) => Status::Exited(io.exit().flatten()),
        _ if args.kill && !spare => {
            registry.kill(id);
            let never = || false;
            let _ = io.wait(
                WaitKind::Wait,
                io.mark(),
                SESSION_STOP_WAIT,
                &cancelled,
                &never,
                on_output,
            );
            Status::Stopped
        }
        _ => Status::Running { waiting },
    };
    let report = io.look(id, status);
    // What the model is told beyond the report — never the cell, which shows
    // what happened (its state row says where the session stands).
    let note = match status {
        Status::Running { .. } if spare => Some(NOT_KILLED_NOTE),
        Status::Running { .. } if matches!(end, Some(WaitEnd::Handoff)) => Some(WAIT_ENDED_NOTE),
        // Typed text without its Enter, still sitting on the line being
        // edited: a terminal in line mode holds it for the program, and a
        // line editor (a REPL's readline, in raw mode) shows it echoed at its
        // prompt with the cursor right after it.
        Status::Running { .. }
            if leaves_line_open(&parts)
                && (registry.line_mode(id).is_some_and(|mode| mode.canonical)
                    || typed_tail(&parts).is_some_and(|tail| io.holds_typed(tail))) =>
        {
            Some(UNSUBMITTED_NOTE)
        }
        _ => None,
    };
    let escaped = html_escaped(args.input.as_deref().unwrap_or_default());
    let notes: Vec<String> = escaped
        .then(|| HTML_ESCAPED_NOTE.to_string())
        .into_iter()
        .chain((!dropped.is_empty()).then(|| dropped_codes_note(&dropped)))
        .chain(note.map(str::to_string))
        .collect();
    if let Some(observed) = io.end_wait() {
        registry.finalize(id, observed);
    }
    ToolOutcome {
        ok: !matches!(status, Status::Exited(code) if code != Some(0)),
        context: (!notes.is_empty()).then(|| format!("{report}\n{}", notes.join("\n"))),
        ..ToolOutcome::ok(report)
    }
}

/// How long a `kill` waits for the killed command's last output and exit.
const SESSION_STOP_WAIT: Duration = Duration::from_secs(3);

/// The model-facing error for a session id nothing answers to — naming the
/// ones that are running, so a model that lost track (after a `/compact`, a
/// `/resume`) can find its way back rather than guess.
fn unknown_session_text(id: &str, running: &[(String, String)]) -> String {
    let head = format!(
        "No running session {id} — it has exited (its final output was already reported) \
         or never existed."
    );
    if running.is_empty() {
        return format!("{head} No sessions are running.");
    }
    let list: Vec<String> = running
        .iter()
        .map(|(id, command)| format!("{id} ({command})"))
        .collect();
    format!("{head} Running sessions: {}.", list.join(", "))
}

/// Read `pipe` to EOF in chunks, forwarding each raw chunk over `tx` so the
/// caller can merge and stream it. Draining to EOF (rather than stopping at a
/// cap) keeps a chatty command from blocking on a full pipe; the caller bounds
/// what it *retains*. Stops early if the receiver has hung up.
fn drain_pipe(mut pipe: impl Read, tx: &mpsc::Sender<Vec<u8>>) {
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break, // EOF
            Ok(n) => {
                if tx.send(chunk[..n].to_vec()).is_err() {
                    break; // receiver gone — stop draining
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// How often a plain command's running cell is sent its output while it
/// flows — every line still arrives, batched; a burst of progress frames
/// costs one redraw per interval, not one per frame.
const PIPE_STREAM_INTERVAL: Duration = Duration::from_millis(50);

/// A plain command's output as it arrives (`docs/interactive-shell.md`):
/// folded the way a terminal would show it ([`Fold`] — a `\r`-redrawn
/// progress bar is one line in its final state, colour escapes gone), each
/// line kept once it ends, within the output cap, and streamed to the
/// running cell as [`ToolProgress::Screen`] — the ended lines appended, the
/// line still being drawn redrawn in place.
///
/// [`Fold`]: crate::pty::fold::Fold
struct PipeOutput {
    fold: crate::pty::fold::Fold,
    /// The lines that ended, within `cap` bytes — what the model reads.
    text: String,
    /// Output the cap cut.
    truncated: bool,
    /// Ended lines the running cell has not been sent yet.
    pending: String,
    /// The line in progress as the running cell last showed it.
    shown: String,
    /// When the running cell was last sent anything.
    sent: Option<Instant>,
    /// The bytes as they were read, within `cap` — what a Ctrl+B handoff
    /// replays, so the background shell folds the whole stream itself.
    raw: Vec<u8>,
    cap: usize,
}

impl PipeOutput {
    fn new(cap: usize) -> Self {
        Self {
            fold: crate::pty::fold::Fold::with_line_cap(cap),
            text: String::new(),
            truncated: false,
            pending: String::new(),
            shown: String::new(),
            sent: None,
            raw: Vec::new(),
            cap,
        }
    }

    /// Fold a chunk in, streaming the running cell (paced).
    fn absorb(&mut self, chunk: &[u8], on_output: &mut dyn FnMut(ToolProgress<'_>)) {
        let room = self.cap.saturating_sub(self.raw.len()).min(chunk.len());
        self.raw.extend_from_slice(&chunk[..room]);
        self.fold.feed(chunk);
        self.truncated |= self.fold.overflowed();
        let lines = self.fold.take_settled();
        self.keep(&lines);
        self.stream(on_output, false);
    }

    /// Keep ended lines, within the cap.
    fn keep(&mut self, lines: &str) {
        let room = self.cap.saturating_sub(self.text.len());
        let take = lines.floor_char_boundary(room);
        self.truncated |= take < lines.len();
        self.text.push_str(&lines[..take]);
        self.pending.push_str(&lines[..take]);
    }

    /// Send the running cell what changed since the last send — unless one
    /// went out within [`PIPE_STREAM_INTERVAL`] and this is not the `last`.
    fn stream(&mut self, on_output: &mut dyn FnMut(ToolProgress<'_>), last: bool) {
        if !last
            && self
                .sent
                .is_some_and(|sent| sent.elapsed() < PIPE_STREAM_INTERVAL)
        {
            return;
        }
        // Past the cap the line in progress can never be kept, so it is not
        // shown either.
        let live = if self.text.len() < self.cap {
            self.fold.current()
        } else {
            String::new()
        };
        if self.pending.is_empty() && live == self.shown {
            return;
        }
        on_output(ToolProgress::Screen {
            settled: &self.pending,
            live: &live,
        });
        self.pending.clear();
        self.shown = live;
        self.sent = Some(Instant::now());
    }

    /// What a Ctrl+B handoff replays: the output as it was read.
    fn into_raw(self) -> Vec<u8> {
        self.raw
    }

    /// The command is done: the line still being drawn ends where it stands.
    /// Returns the output and whether the cap cut it.
    fn finish(mut self, on_output: &mut dyn FnMut(ToolProgress<'_>)) -> (String, bool) {
        let rest = std::mem::take(&mut self.fold).finish();
        self.keep(&rest);
        self.stream(on_output, true);
        (self.text, self.truncated)
    }
}

/// `read`: an image file (`tools::is_image_path`) is returned visually — see
/// [`read_image`]; a text file returns `cat -n`-style numbered lines,
/// byte-capped. On a model whose `/v1/models` record said "no image input"
/// (`vision == Some(false)`), an image read fails as a recoverable error
/// before touching the file — attaching would make the provider fail the
/// whole turn instead (`docs/tools.md`).
fn run_read(arguments: &str, vision: Option<bool>) -> ToolOutcome {
    let args: ReadArgs = match tools::parse_args(arguments) {
        Ok(a) => a,
        Err(e) => return arg_error(e),
    };
    if tools::is_image_path(&args.path) && vision == Some(false) {
        return ToolOutcome::error(format!(
            "cannot view {}: the current model does not support image input — \
             work with the file another way, or ask the user to switch to a \
             vision-capable model with /model",
            args.path
        ));
    }
    let path = Path::new(&args.path);
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(err) => return ToolOutcome::error(format!("could not read {}: {err}", args.path)),
    };
    if tools::is_image_path(&args.path) {
        return read_image(&args.path, &bytes);
    }
    let content = String::from_utf8_lossy(&bytes);
    if content.is_empty() {
        return ToolOutcome::ok(format!("(file {} is empty)", args.path));
    }
    let numbered = tools::format_read(&content, args.offset, args.limit);
    let (output, truncated) = tools::truncate_output(numbered, TOOL_OUTPUT_MAX_BYTES);
    ToolOutcome::ok(output).with_truncated(truncated)
}

/// The image branch of `read` (`docs/tools.md`): bound the size, sniff the
/// real format from the bytes (the MIME must match the content, not the
/// extension, or a provider decodes garbage), read the header dimensions, and
/// return the facts as the text output with the base64 `data:` URL riding
/// [`ToolOutcome::image`] — the agent loop attaches it as a follow-up user
/// message. Every failure is a recoverable error the model reads.
fn read_image(path: &str, bytes: &[u8]) -> ToolOutcome {
    const MB: f64 = 1024.0 * 1024.0;
    // Sniff the content (magic bytes), the clipboard module's pattern — an
    // io::Error is impossible over an in-memory cursor but handled anyway.
    let reader = match image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format() {
        Ok(reader) => reader,
        Err(err) => return ToolOutcome::error(format!("could not inspect {path}: {err}")),
    };
    // The sniffed format is kept, not just its MIME: the auto-resize below
    // needs a decoder for it.
    let Some((format, (mime, label))) = reader
        .format()
        .and_then(|format| Some((format, vision_format(format)?)))
    else {
        return ToolOutcome::error(format!(
            "{path} is not a supported image (its content is not png/jpeg/gif/webp) — \
             read it as text or convert it first"
        ));
    };
    let (width, height) = match reader.into_dimensions() {
        Ok(dimensions) => dimensions,
        Err(err) => {
            return ToolOutcome::error(format!("could not decode {path} as an image: {err}"));
        }
    };
    // `/settings` **Auto-resize images**: a 12-megapixel photo is megabytes of
    // base64 the model reads no better than the same picture at 2000 pixels,
    // and past the byte cap it could not be sent at all. The *file* is
    // untouched — only what rides the request shrinks — so the fact line
    // leads with the file's own size and names the sent one after
    // (`docs/images.md`). Through the session's payload cache: a picture the
    // model reads twice is shrunk once.
    let sent = crate::images::downscale_for_model_at(std::path::Path::new(path), bytes, format);
    let (payload, mime, label) = match &sent {
        Some(small) => {
            let (mime, label) = vision_format(small.format).unwrap_or((mime, label));
            (small.bytes.as_slice(), mime, label)
        }
        None => (bytes, mime, label),
    };
    if payload.len() > tools::READ_IMAGE_MAX_BYTES {
        return ToolOutcome::error(format!(
            "image {path} is too large to attach ({:.1} MB; the limit is {:.2} MB) — \
             turn on Auto-resize images in /settings, or downscale it with a bash command first",
            payload.len() as f64 / MB,
            tools::READ_IMAGE_MAX_BYTES as f64 / MB,
        ));
    }
    // The session keeps this one encoding: every later turn re-sends the
    // picture, and reads it back from the cache rather than from the file
    // (`docs/memory.md`).
    let url = crate::images::remember_attachment(std::path::Path::new(path), payload, mime);
    let text = match &sent {
        Some(small) => tools::format_read_image_resized(
            label,
            width,
            height,
            bytes.len(),
            small.size,
            small.bytes.len(),
        ),
        None => tools::format_read_image(label, width, height, bytes.len()),
    };
    ToolOutcome::ok(text).with_image(url)
}

/// The `(MIME, display label)` for a sniffed format — `None` for anything a
/// vision-capable OpenAI-compatible endpoint doesn't accept.
fn vision_format(format: image::ImageFormat) -> Option<(&'static str, &'static str)> {
    match format {
        image::ImageFormat::Png => Some(("image/png", "PNG")),
        image::ImageFormat::Jpeg => Some(("image/jpeg", "JPEG")),
        image::ImageFormat::Gif => Some(("image/gif", "GIF")),
        image::ImageFormat::WebP => Some(("image/webp", "WebP")),
        _ => None,
    }
}

/// `write`: create parent dirs and write the file, reporting a diff vs the old
/// contents (or a `Wrote …` summary for a new file).
fn run_write(arguments: &str) -> ToolOutcome {
    let args: WriteArgs = match tools::parse_args(arguments) {
        Ok(a) => a,
        Err(e) => return arg_error(e),
    };
    let path = Path::new(&args.path);
    let existed = path.exists();
    let old = if existed {
        std::fs::read_to_string(path).unwrap_or_default()
    } else {
        String::new()
    };
    if let Err(err) = create_parents(path) {
        return ToolOutcome::error(format!("could not create parent directories: {err}"));
    }
    if let Err(err) = std::fs::write(path, &args.content) {
        return ToolOutcome::error(format!("could not write {}: {err}", args.path));
    }
    let display = describe_change(&args.path, &old, &args.content, !existed);
    // The cell keeps the numbered body; the model reads one line, because the
    // content it just sent replays verbatim on the call itself
    // (`docs/tools.md`, `docs/context.md`). A write that changed nothing has
    // no body to spare it, so it stays single-text.
    if old == args.content {
        return ToolOutcome::ok(display);
    }
    ToolOutcome::ok(display).with_context(tools::write_ack(&args.path, !existed))
}

/// `edit`: exact-string replacement (the pure [`tools::apply_edit`]), written
/// back to disk, reported as a diff.
fn run_edit(arguments: &str) -> ToolOutcome {
    let args: EditArgs = match tools::parse_args(arguments) {
        Ok(a) => a,
        Err(e) => return arg_error(e),
    };
    let path = Path::new(&args.path);
    let old = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(err) => return ToolOutcome::error(format!("could not read {}: {err}", args.path)),
    };
    let result = match tools::apply_edit(&old, &args.old_string, &args.new_string, args.replace_all)
    {
        Ok(r) => r,
        Err(e) => return ToolOutcome::error(e.to_string()),
    };
    if let Err(err) = std::fs::write(path, &result.new_content) {
        return ToolOutcome::error(format!("could not write {}: {err}", args.path));
    }
    // [`run_write`]'s split: the diff hunks are the cell, the ack is the
    // conversation. `apply_edit` refuses a no-op, so there is always a change.
    ToolOutcome::ok(describe_change(
        &args.path,
        &old,
        &result.new_content,
        false,
    ))
    .with_context(tools::edit_ack(&args.path, result.replacements))
}

/// The model-facing result of a `write`/`edit` — also exactly what the cell
/// shows (the TUI restyles the rows; see `docs/tools.md`). A brand-new file is
/// a `Wrote {N} lines to {path}` head over the numbered contents
/// ([`tools::write_report`]); a change to existing content is an
/// `Updated {path} (+A -D)` head over the numbered diff hunks
/// ([`tools::update_report`]). The numbers match the `read` tool's, so the
/// model can cite them in a follow-up `edit`; the head's path is the compact
/// cwd-relative display form ([`tools::display_path`] — `../` climbs for a
/// target outside the cwd), while the cell header keeps the model's own
/// argument verbatim.
fn describe_change(path: &str, old: &str, new: &str, created: bool) -> String {
    let shown = shown_path(path);
    if created {
        tools::write_report(&shown, new)
    } else {
        tools::update_report(&shown, old, new)
    }
}

/// The display form of a tool path — [`tools::display_path`] against the
/// process cwd (the same directory every relative tool path resolves in), the
/// path unchanged when the cwd is unreadable.
fn shown_path(path: &str) -> String {
    match std::env::current_dir() {
        Ok(cwd) => tools::display_path(path, &cwd),
        Err(_) => path.to_string(),
    }
}

/// Create the parent directories of `path`, if any (a bare filename has none).
fn create_parents(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => std::fs::create_dir_all(parent),
        _ => Ok(()),
    }
}

/// A unique temp path under the system temp dir for a test file.
#[cfg(test)]
fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("alter-zero-exec-test-{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: "c".to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        }
    }

    fn exec(name: &str, args: &str) -> ToolOutcome {
        RealToolExecutor::new().execute(&call(name, args), &CancelToken::new(), &mut |_| {})
    }

    /// A running cell as the TUI keeps it: a `ToolScreen`'s settled text
    /// appended and its live rows replacing the last ones
    /// (`App::push_tool_screen`), every state it passed through kept as a
    /// frame.
    #[derive(Default)]
    struct LiveCell {
        view: String,
        live_len: usize,
        frames: Vec<String>,
    }

    impl LiveCell {
        fn take(&mut self, progress: ToolProgress<'_>) {
            match progress {
                ToolProgress::Screen { settled, live } => {
                    self.live_len =
                        crate::app::apply_tool_screen(&mut self.view, self.live_len, settled, live);
                }
                ToolProgress::Title(_) => return,
            }
            self.frames.push(self.view.clone());
        }
    }

    #[test]
    fn a_plain_commands_progress_bar_reaches_the_model_once() {
        // curl's meter, tqdm, ffmpeg: `\r` frames on a pipe. The model reads
        // the line as a terminal would show it — its final frame — and the
        // running cell redraws it in place (docs/interactive-shell.md).
        let command = r"printf 'fetching\n'; for p in 10 40 70 100; do printf '\r%3d%%' $p; sleep 0.15; done; printf '\n'";
        let mut cell = LiveCell::default();
        let out = RealToolExecutor::new().execute(
            &call(
                "bash",
                &serde_json::json!({ "command": command }).to_string(),
            ),
            &CancelToken::new(),
            &mut |progress| cell.take(progress),
        );
        assert_eq!(out.output, "Exit code: 0\nfetching\n100%\n");
        assert!(
            cell.frames.iter().any(|frame| frame == "fetching\n 40%"),
            "the bar streamed as it moved: {:?}",
            cell.frames
        );
        assert!(
            cell.frames
                .iter()
                .all(|frame| frame.matches('%').count() <= 1),
            "one row per bar: {:?}",
            cell.frames
        );
    }

    #[test]
    fn a_plain_commands_escapes_never_reach_the_model_but_its_tabs_do() {
        let command = r"printf '\033[1;31merror\033[0m: bad\n\tindented  \n'";
        let out = exec(
            "bash",
            &serde_json::json!({ "command": command }).to_string(),
        );
        assert_eq!(out.output, "Exit code: 0\nerror: bad\n\tindented  \n");
    }

    #[test]
    fn bash_streams_its_output_to_the_sink() {
        // The live-output sink receives the command's output as it runs; by the
        // time execute returns it has seen all of it, and the framed final still
        // carries the same body (docs/tool-streaming.md).
        let mut cell = LiveCell::default();
        let out = RealToolExecutor::new().execute(
            &call("bash", r#"{"command":"printf 'a\nb\nc\n'"}"#),
            &CancelToken::new(),
            &mut |progress| cell.take(progress),
        );
        assert!(out.ok, "got {}", out.output);
        assert_eq!(
            cell.view, "a\nb\nc\n",
            "the sink tails the full output as complete lines"
        );
        assert!(
            out.output.contains("a\nb\nc"),
            "the framed final still carries the body: {}",
            out.output
        );
    }

    #[test]
    fn bash_streams_a_trailing_line_without_a_newline() {
        // A final line with no trailing '\n' is still flushed to the sink once,
        // at the end (before the ToolEnd overwrite), so the sink sees everything.
        let mut cell = LiveCell::default();
        let out = RealToolExecutor::new().execute(
            &call("bash", r#"{"command":"printf 'x\ny'"}"#),
            &CancelToken::new(),
            &mut |progress| cell.take(progress),
        );
        assert!(out.ok, "got {}", out.output);
        assert_eq!(
            cell.view, "x\ny",
            "the partial trailing line is flushed too"
        );
    }

    #[test]
    fn bash_survives_a_stale_detach_helper() {
        // The helper is a fallback TIER (`subprocess::tiers`), not the only
        // route: with the `setsid` binary present the chain never needs it,
        // and even a helper path that no longer exists (the TUI binary
        // replaced or deleted mid-session) must not break the tool — the
        // chain lands on a working tier either way. The real helper's conduct
        // (setsid → no controlling terminal, then exec `sh -c`) is covered by
        // tests/detached_exec.rs against the built binary, and by smoke.
        let out = RealToolExecutor::new()
            .with_detach_helper(Some(temp_path("no-such-helper")))
            .execute(
                &call("bash", r#"{"command":"echo hi"}"#),
                &CancelToken::new(),
                &mut |_| {},
            );
        assert!(out.ok, "the chain survives a stale helper: {}", out.output);
        assert!(out.output.contains("hi"), "got {}", out.output);
    }

    #[test]
    fn bash_runs_a_command_and_frames_the_exit_code() {
        let out = exec("bash", r#"{"command":"echo hello"}"#);
        assert!(out.ok);
        assert!(out.output.starts_with("Exit code: 0"), "got {}", out.output);
        assert!(out.output.contains("hello"));
    }

    #[test]
    fn bash_reports_a_nonzero_exit_as_a_failure() {
        let out = exec("bash", r#"{"command":"exit 3"}"#);
        assert!(!out.ok);
        assert!(out.output.contains("Exit code: 3"));
    }

    #[test]
    fn bash_merges_stderr_into_the_output() {
        let out = exec("bash", r#"{"command":"echo oops 1>&2"}"#);
        assert!(out.output.contains("oops"));
    }

    #[test]
    fn bash_times_out_a_slow_command() {
        // The live schema spelling — the sibling tests below keep the
        // `timeout_ms` alias covered.
        let out = exec("bash", r#"{"command":"sleep 5","timeout":150}"#);
        assert!(!out.ok);
        assert!(out.output.contains("timed out"), "got {}", out.output);
    }

    #[test]
    fn bash_does_not_hang_on_a_backgrounded_grandchild() {
        // `sleep 10 &` makes `sh` fork `sleep` into the background and exit 0 at
        // once. `sleep` inherits the stdout/stderr pipe, so read_capped never
        // sees EOF — joining the reader would block ~10s (the timeout defeated).
        // Killing the whole process group reaps the straggler, so the call
        // returns promptly. Assert it finishes well under the 10s straggler.
        let start = std::time::Instant::now();
        let out = exec("bash", r#"{"command":"sleep 10 &","timeout_ms":30000}"#);
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "run_bash returned in {elapsed:?} — a backgrounded grandchild made it hang"
        );
        assert!(out.ok, "the command itself exits 0");
    }

    #[test]
    fn bash_timeout_is_honoured_even_when_a_child_survives() {
        // A foreground long child that `sh` forked (not exec'd): on timeout,
        // killing `sh` alone would orphan the child holding the pipe and the
        // join would wait it out. The group kill must make the timeout prompt.
        let start = std::time::Instant::now();
        let out = exec(
            "bash",
            r#"{"command":"(sleep 10 & wait)","timeout_ms":300}"#,
        );
        let elapsed = start.elapsed();
        assert!(!out.ok);
        assert!(out.output.contains("timed out"), "got {}", out.output);
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "the timeout took {elapsed:?} — a surviving child defeated it"
        );
    }

    #[test]
    fn read_returns_numbered_lines() {
        let path = temp_path("read.txt");
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(out.ok);
        assert!(out.output.contains("1 alpha"), "got {}", out.output);
        assert!(out.output.contains("2 beta"));
    }

    // ===== image reads (docs/tools.md) =====

    #[test]
    fn read_of_a_png_attaches_the_image() {
        let path = temp_path("read-image.png");
        image::RgbaImage::from_pixel(3, 2, image::Rgba([220, 20, 20, 255]))
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(out.ok, "got {}", out.output);
        assert!(
            out.output.starts_with("Read image "),
            "the marker head leads: {}",
            out.output
        );
        assert!(out.output.contains("PNG"), "sniffed format: {}", out.output);
        assert!(out.output.contains("3x2"), "dimensions: {}", out.output);
        // Concise, Claude-Code style: one fact line, the path left to the
        // `● Read({path})` header and the follow-up `[image]` note.
        assert_eq!(
            out.output.lines().count(),
            1,
            "one concise line: {}",
            out.output
        );
        assert!(
            !out.output.contains(&path.display().to_string()),
            "the path never repeats in the fact line: {}",
            out.output
        );
        let url = out.image.expect("the data: URL rides beside the text");
        assert!(url.starts_with("data:image/png;base64,"), "got {url}");
        assert!(
            !out.output.contains("base64"),
            "the URL never pollutes the text output: {}",
            out.output
        );
    }

    #[test]
    fn read_image_mime_follows_the_sniffed_content_not_the_extension() {
        // JPEG bytes misnamed .png: the data: URL must say image/jpeg or a
        // provider decodes garbage.
        let path = temp_path("read-image-mislabeled.png");
        image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]))
            .save_with_format(&path, image::ImageFormat::Jpeg)
            .unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(out.ok, "got {}", out.output);
        assert!(out.output.contains("JPEG"), "got {}", out.output);
        let url = out.image.expect("attached");
        assert!(url.starts_with("data:image/jpeg;base64,"), "got {url}");
    }

    #[test]
    fn read_of_a_misnamed_non_image_is_a_recoverable_error() {
        let path = temp_path("read-not-an-image.png");
        std::fs::write(&path, "just text pretending to be pixels").unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(!out.ok, "got {}", out.output);
        assert!(out.image.is_none());
        assert!(
            out.output.contains("not"),
            "the model learns the file isn't an image: {}",
            out.output
        );
    }

    #[test]
    fn an_oversized_file_that_is_not_an_image_says_exactly_that() {
        // Megabytes of zeroes named `.png`. The sniff runs before the size
        // check now — auto-resize can rescue an oversized *picture*, so
        // "too large" would be the wrong first answer — and what this file
        // actually is is not a picture at all.
        let path = temp_path("read-image-huge.png");
        std::fs::write(&path, vec![0u8; tools::READ_IMAGE_MAX_BYTES + 1]).unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(!out.ok, "got {}", out.output);
        assert!(out.image.is_none());
        assert!(
            out.output.contains("not a supported image"),
            "the model learns what is actually wrong: {}",
            out.output
        );
    }

    #[test]
    fn an_image_past_the_resize_cap_is_downscaled_not_refused() {
        // `/settings` **Auto-resize images**, on by default: the file keeps
        // its own size (which is what the cell draws), and the fact line names
        // the original first and what was uploaded after (`docs/images.md`).
        let path = temp_path("read-image-big.png");
        image::RgbImage::from_pixel(3000, 2000, image::Rgb([10, 120, 220]))
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(out.ok, "got {}", out.output);
        assert!(
            out.output.starts_with("Read image (PNG, 3000x2000,"),
            "the file's own facts lead: {}",
            out.output
        );
        assert!(
            out.output.contains("sent resized to 2000x1333"),
            "and the uploaded size is named: {}",
            out.output
        );
        assert_eq!(
            crate::images::read_image_size(&out.output),
            Some((3000, 2000)),
            "the renderer still reads the size of the file it will draw"
        );
        assert!(out.image.is_some(), "the pixels still ride along");
    }

    #[test]
    fn an_image_read_on_a_known_non_vision_model_is_a_recoverable_error() {
        // The model's /v1/models record said "no image input": attaching
        // anyway would make the provider fail the whole turn (OpenRouter
        // 404s "No endpoints found that support image input"), so the read
        // resolves as an error the model reads and can act on instead.
        let path = temp_path("read-image-no-vision.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        let out = RealToolExecutor::new().with_vision(Some(false)).execute(
            &call("read", &format!(r#"{{"path":"{}"}}"#, path.display())),
            &CancelToken::new(),
            &mut |_| {},
        );
        std::fs::remove_file(&path).ok();
        assert!(!out.ok, "got {}", out.output);
        assert!(out.image.is_none(), "nothing is attached");
        assert!(
            out.output.contains("does not support image input"),
            "the model learns why: {}",
            out.output
        );
        assert!(
            out.output.contains("/model"),
            "and how the user could fix it: {}",
            out.output
        );
    }

    #[test]
    fn vision_true_or_unknown_still_attaches_and_text_reads_ignore_the_gate() {
        let path = temp_path("read-image-vision-ok.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([9, 9, 9, 255]))
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        for vision in [Some(true), None] {
            let out = RealToolExecutor::new().with_vision(vision).execute(
                &call("read", &format!(r#"{{"path":"{}"}}"#, path.display())),
                &CancelToken::new(),
                &mut |_| {},
            );
            assert!(out.ok, "vision {vision:?}: {}", out.output);
            assert!(out.image.is_some(), "vision {vision:?} attaches");
        }
        std::fs::remove_file(&path).ok();
        // The gate never touches a text read.
        let text = temp_path("read-text-no-vision.txt");
        std::fs::write(&text, "alpha\n").unwrap();
        let out = RealToolExecutor::new().with_vision(Some(false)).execute(
            &call("read", &format!(r#"{{"path":"{}"}}"#, text.display())),
            &CancelToken::new(),
            &mut |_| {},
        );
        std::fs::remove_file(&text).ok();
        assert!(out.ok, "got {}", out.output);
        assert!(out.output.contains("1 alpha"));
    }

    #[test]
    fn a_text_read_carries_no_image() {
        let path = temp_path("read-plain.txt");
        std::fs::write(&path, "alpha\n").unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(out.ok);
        assert!(out.image.is_none(), "text reads stay text-only");
    }

    #[test]
    fn read_of_a_missing_file_is_a_recoverable_error() {
        let out = exec("read", r#"{"path":"/definitely/not/here.txt"}"#);
        assert!(!out.ok);
        assert!(out.output.contains("could not read"));
    }

    /// The head's expected display path — the same cwd-relative form
    /// `describe_change` derives, computed against the test process's cwd so
    /// the assertion holds wherever the temp dir lives.
    fn shown(path: &Path) -> String {
        tools::display_path(
            &path.display().to_string(),
            &std::env::current_dir().unwrap(),
        )
    }

    #[test]
    fn write_creates_a_new_file_and_reports_it() {
        let path = temp_path("write-new.txt");
        std::fs::remove_file(&path).ok();
        let out = exec(
            "write",
            &format!(r#"{{"path":"{}","content":"one\ntwo\n"}}"#, path.display()),
        );
        let written = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(out.ok);
        assert_eq!(written, "one\ntwo\n");
        // The Claude-Code head over the cwd-relative display path — the temp
        // dir is outside the cwd, so the path shows as a `../` climb, never
        // the whole absolute argument.
        assert_eq!(
            out.output.lines().next(),
            Some(format!("Wrote 2 lines to {}", shown(&path)).as_str()),
            "got {}",
            out.output
        );
        assert!(
            shown(&path).starts_with("../"),
            "the temp file lives outside the cwd: {}",
            shown(&path)
        );
        // The body echoes the new file as numbered lines (the TUI's preview
        // and the model's reference for follow-up edits).
        assert!(out.output.contains("1 one"), "got {}", out.output);
        assert!(out.output.contains("2 two"));
    }

    #[test]
    fn write_over_existing_content_shows_a_diff() {
        let path = temp_path("write-over.txt");
        std::fs::write(&path, "keep\nold\n").unwrap();
        let out = exec(
            "write",
            &format!(r#"{{"path":"{}","content":"keep\nnew\n"}}"#, path.display()),
        );
        std::fs::remove_file(&path).ok();
        assert!(out.ok);
        assert_eq!(
            out.output.lines().next(),
            Some(format!("Updated {} (+1 -1)", shown(&path)).as_str()),
            "got {}",
            out.output
        );
        // The diff body carries line numbers (codex's numbered hunks).
        assert!(out.output.contains("2 -old"), "got {}", out.output);
        assert!(out.output.contains("2 +new"));
    }

    #[test]
    fn edit_replaces_and_writes_back() {
        let path = temp_path("edit.txt");
        std::fs::write(&path, "let x = 1;\nlet y = 2;\n").unwrap();
        let out = exec(
            "edit",
            &format!(
                r#"{{"path":"{}","old_string":"let x = 1;","new_string":"let x = 42;"}}"#,
                path.display()
            ),
        );
        let after = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(out.ok);
        assert_eq!(after, "let x = 42;\nlet y = 2;\n");
        assert!(out.output.contains("1 +let x = 42;"), "got {}", out.output);
    }

    #[test]
    fn write_hands_the_model_one_line_and_the_cell_the_numbered_body() {
        // The two-text split (`ToolOutcome::context`, docs/tools.md): the
        // numbered content is the *cell*, and what rides the conversation is
        // one line — the arguments already carry the content verbatim.
        let path = temp_path("write-ack-new.txt");
        std::fs::remove_file(&path).ok();
        let out = exec(
            "write",
            &format!(r#"{{"path":"{}","content":"one\ntwo\n"}}"#, path.display()),
        );
        std::fs::remove_file(&path).ok();
        assert!(out.output.contains("1 one"), "the cell keeps the body");
        assert_eq!(
            out.context.as_deref(),
            Some(tools::write_ack(&path.display().to_string(), /*created=*/ true).as_str()),
            "the model reads the ack, naming its own absolute argument"
        );
    }

    #[test]
    fn write_over_existing_content_acknowledges_an_update() {
        let path = temp_path("write-ack-over.txt");
        std::fs::write(&path, "keep\nold\n").unwrap();
        let out = exec(
            "write",
            &format!(r#"{{"path":"{}","content":"keep\nnew\n"}}"#, path.display()),
        );
        std::fs::remove_file(&path).ok();
        assert!(out.output.contains("2 +new"), "the cell keeps the diff");
        assert_eq!(
            out.context.as_deref(),
            Some(tools::write_ack(&path.display().to_string(), /*created=*/ false).as_str())
        );
    }

    #[test]
    fn a_write_that_changes_nothing_has_no_second_text() {
        // `No changes to …` is already the whole truth: there is no body to
        // spare the model, so the outcome stays a plain single-text ToolEnd.
        let path = temp_path("write-ack-same.txt");
        std::fs::write(&path, "same\n").unwrap();
        let out = exec(
            "write",
            &format!(r#"{{"path":"{}","content":"same\n"}}"#, path.display()),
        );
        std::fs::remove_file(&path).ok();
        assert!(
            out.output.starts_with("No changes to "),
            "got {}",
            out.output
        );
        assert_eq!(out.context, None);
    }

    #[test]
    fn a_failed_write_or_edit_carries_no_second_text() {
        // An error's display *is* what the model must read to recover, so the
        // split would hide the reason. Both tools return the error before
        // they can attach an ack — this pins that they keep doing so.
        let missing = temp_path("edit-no-file.txt");
        std::fs::remove_file(&missing).ok();
        let no_file = exec(
            "edit",
            &format!(
                r#"{{"path":"{}","old_string":"a","new_string":"b"}}"#,
                missing.display()
            ),
        );
        assert!(!no_file.ok && no_file.context.is_none(), "{no_file:?}");

        let seeded = temp_path("edit-no-match.txt");
        std::fs::write(&seeded, "abc\n").unwrap();
        let no_match = exec(
            "edit",
            &format!(
                r#"{{"path":"{}","old_string":"zzz","new_string":"y"}}"#,
                seeded.display()
            ),
        );
        std::fs::remove_file(&seeded).ok();
        assert!(!no_match.ok && no_match.context.is_none(), "{no_match:?}");

        // A `write` that cannot create its parent (a file stands where the
        // directory would go) fails the same way.
        let blocker = temp_path("write-blocked.txt");
        std::fs::write(&blocker, "not a dir\n").unwrap();
        let blocked = exec(
            "write",
            &format!(
                r#"{{"path":"{}/inside.txt","content":"x"}}"#,
                blocker.display()
            ),
        );
        std::fs::remove_file(&blocker).ok();
        assert!(!blocked.ok && blocked.context.is_none(), "{blocked:?}");
    }

    #[test]
    fn edit_hands_the_model_one_line_and_counts_a_replace_all() {
        let path = temp_path("edit-ack.txt");
        std::fs::write(&path, "a\na\n").unwrap();
        let out = exec(
            "edit",
            &format!(
                r#"{{"path":"{}","old_string":"a","new_string":"b","replace_all":true}}"#,
                path.display()
            ),
        );
        std::fs::remove_file(&path).ok();
        assert!(out.output.contains("1 +b"), "the cell keeps the diff");
        assert_eq!(
            out.context.as_deref(),
            Some(tools::edit_ack(&path.display().to_string(), 2).as_str()),
            "the ack counts what `replace_all` really touched"
        );
    }

    #[test]
    fn edit_reports_a_missing_match_without_writing() {
        let path = temp_path("edit-miss.txt");
        std::fs::write(&path, "abc\n").unwrap();
        let out = exec(
            "edit",
            &format!(
                r#"{{"path":"{}","old_string":"zzz","new_string":"y"}}"#,
                path.display()
            ),
        );
        let after = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(!out.ok);
        assert_eq!(after, "abc\n", "the file is untouched on a failed edit");
        assert!(out.output.contains("not found"));
    }

    #[test]
    fn an_unknown_tool_is_a_recoverable_error() {
        let out = exec("teleport", "{}");
        assert!(!out.ok);
        assert!(out.output.contains("unknown tool"));
    }

    #[test]
    fn bad_arguments_are_a_recoverable_error() {
        let out = exec("bash", "not json");
        assert!(!out.ok);
        assert!(out.output.contains("invalid tool arguments"));
    }

    // ===== background (docs/background.md) =====

    #[test]
    fn background_launch_text_names_the_session_bash_session_takes_back() {
        // The id is model-facing now: `bash_session` takes it back to check
        // on the command, read what it printed, or end it
        // (docs/interactive-shell.md). It appears as the session id and as
        // the interim file's basename — twice, never as a separate label.
        let task = crate::background::LaunchedTask {
            id: "bvyo7tkbe".to_string(),
            output_path: std::path::PathBuf::from("/tmp/alter-zero-0/s1/bvyo7tkbe.output"),
        };
        let text = background_launch_text(&task);
        assert!(text.contains("as session bvyo7tkbe"), "{text}");
        assert!(text.contains("bash_session"), "{text}");
        assert_eq!(
            text.matches("bvyo7tkbe").count(),
            2,
            "the session id, and the interim file's basename: {text}"
        );
        assert!(
            text.contains("/tmp/alter-zero-0/s1/bvyo7tkbe.output"),
            "the interim path is the model's progress channel: {text}"
        );
        assert!(text.lines().count() <= 2, "short: {text}");
    }

    #[test]
    fn background_launch_text_promises_a_completion_notice() {
        // The second line is the one the model acts on, so it states what
        // happens to *it*: the final output arrives on its own when the
        // command finishes. "You will be re-invoked" named the harness's
        // own mechanism instead — a word a model can do nothing with, and
        // one that reads as a threat of interruption rather than a promise
        // of delivery (docs/background.md).
        let task = crate::background::LaunchedTask {
            id: "bash_3".to_string(),
            output_path: std::path::PathBuf::from("/tmp/tasks/bash_3.output"),
        };
        let text = background_launch_text(&task);
        assert!(
            text.contains("notified with the final output when it finishes"),
            "got {text}"
        );
        assert!(!text.contains("re-invoked"), "got {text}");
    }

    #[test]
    fn background_handoff_text_leads_with_the_user_move_over_the_launch_facts() {
        // Ctrl+B: the model expected a foreground run's full output, so the
        // handoff must say the USER moved the command, carry the launch facts
        // (interim path + completion promise) verbatim, and steer
        // the model off re-running/polling (docs/background.md).
        let task = crate::background::LaunchedTask {
            id: "bash_7".to_string(),
            output_path: std::path::PathBuf::from("/tmp/tasks/bash_7.output"),
        };
        let text = background_handoff_text(&task);
        assert!(
            text.starts_with("The user moved this command to the background"),
            "got {text}"
        );
        assert!(
            text.contains(&background_launch_text(&task)),
            "the launch facts ride along verbatim: {text}"
        );
        assert!(
            text.contains("Do not run the command again"),
            "the model is steered off re-running/waiting: {text}"
        );
    }

    fn test_registry() -> (
        crate::background::BackgroundRegistry,
        tokio::sync::mpsc::UnboundedReceiver<crate::background::BgEvent>,
    ) {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = std::env::temp_dir().join(format!(
            "alter-zero-exec-bg-test-{}-{seq}",
            std::process::id()
        ));
        (crate::background::BackgroundRegistry::new(tx, dir), rx)
    }

    #[test]
    fn run_in_background_returns_a_backgrounded_launch_at_once() {
        let (registry, mut rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let start = std::time::Instant::now();
        let out = executor.execute(
            &call(
                "bash",
                r#"{"command":"sleep 0.2; printf done","run_in_background":true,"description":"nap"}"#,
            ),
            &CancelToken::new(),
            &mut |_| {},
        );
        assert!(
            start.elapsed() < std::time::Duration::from_millis(150),
            "the call returns before the command finishes"
        );
        assert!(out.ok);
        let task_id = out
            .background
            .clone()
            .expect("the outcome carries the task id");
        assert!(
            task_id.starts_with('b') && task_id.len() == 9,
            "a claude-code-style id: {task_id}"
        );
        assert!(
            out.output.contains(&format!("session {task_id}")) && out.output.contains(".output"),
            "the session id bash_session takes back, and the interim file: {}",
            out.output
        );
        assert!(
            !out.output.contains("user moved"),
            "a launch the model asked for must not claim a user action: {}",
            out.output
        );
        // The registry reports the launch and, later, the completion.
        let mut saw_started = false;
        let mut saw_exit = false;
        for _ in 0..500 {
            match rx.try_recv() {
                Ok(crate::background::BgEvent::Started {
                    id,
                    from_model,
                    description,
                    ..
                }) => {
                    assert_eq!(id, task_id);
                    assert!(from_model, "an executor launch is model-launched");
                    assert_eq!(description.as_deref(), Some("nap"));
                    saw_started = true;
                }
                Ok(crate::background::BgEvent::Exited { code, killed, .. }) => {
                    assert_eq!(code, Some(0));
                    assert!(!killed);
                    saw_exit = true;
                    break;
                }
                Ok(crate::background::BgEvent::Output { .. })
                | Ok(crate::background::BgEvent::Screen { .. }) => {}
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        assert!(
            saw_started && saw_exit,
            "the registry reported the lifecycle"
        );
    }

    #[test]
    fn run_in_background_without_a_registry_is_a_recoverable_error() {
        let out = exec("bash", r#"{"command":"echo hi","run_in_background":true}"#);
        assert!(!out.ok);
        assert!(out.background.is_none());
        assert!(
            out.output.contains("not available"),
            "the model is told to run it in the foreground: {}",
            out.output
        );
    }

    #[test]
    fn a_background_request_hands_a_running_command_off_mid_run() {
        // The Ctrl+B path: raise the latch mid-run; the poll loop consumes it
        // and adopts the child — the outcome flips to backgrounded and the
        // already-produced output replays into the background stream.
        let (registry, mut rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let flag = registry.clone();
        // Raise the latch shortly after the command starts producing output.
        let raiser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(120));
            flag.request_background();
        });
        let mut cell = LiveCell::default();
        let out = executor.execute(
            &call(
                "bash",
                r#"{"command":"printf 'early\n'; sleep 3; printf 'late\n'","timeout_ms":30000}"#,
            ),
            &CancelToken::new(),
            &mut |progress| cell.take(progress),
        );
        raiser.join().unwrap();
        assert!(out.ok, "got {}", out.output);
        let task_id = out
            .background
            .clone()
            .expect("the outcome carries the task id");
        assert!(
            task_id.starts_with('b') && task_id.len() == 9,
            "a claude-code-style id: {task_id}"
        );
        // The model asked for a FOREGROUND run — the result must say the
        // *user* moved it (not read like a run_in_background acknowledgement),
        // or the model keeps waiting for the full output and polls the interim
        // file round after round (docs/background.md).
        assert!(
            out.output
                .starts_with("The user moved this command to the background"),
            "the model is told the user moved it: {}",
            out.output
        );
        assert!(
            out.output.contains(&format!("session {task_id}")) && out.output.contains(".output"),
            "the session id and the interim file ride the handoff text: {}",
            out.output
        );
        assert!(cell.view.contains("early"), "the foreground tail ran first");
        // The adopted task replays the prior output and finishes on its own.
        let mut replayed = String::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match rx.try_recv() {
                Ok(crate::background::BgEvent::Output { chunk, .. }) => replayed.push_str(&chunk),
                Ok(crate::background::BgEvent::Exited { code, .. }) => {
                    assert_eq!(code, Some(0));
                    break;
                }
                Ok(crate::background::BgEvent::Started { .. })
                | Ok(crate::background::BgEvent::Screen { .. }) => {}
                Err(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the adopted task finishes on its own"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
        assert!(
            replayed.contains("early") && replayed.contains("late"),
            "prior output replays and the tail keeps streaming: {replayed:?}"
        );
    }

    #[test]
    fn a_stale_background_request_is_cleared_when_a_command_starts() {
        // A Ctrl+B that missed its command must not background the next one.
        let (registry, _rx) = test_registry();
        registry.request_background();
        let executor = RealToolExecutor::new().with_background(registry);
        let out = executor.execute(
            &call("bash", r#"{"command":"echo hi"}"#),
            &CancelToken::new(),
            &mut |_| {},
        );
        assert!(out.ok);
        assert!(
            out.background.is_none(),
            "the stale request was dropped, the command ran in the foreground"
        );
    }

    #[test]
    fn a_subagent_executor_launches_in_background_with_its_origin() {
        // A subagent's run_in_background works like the main turn's — the
        // shell joins the shared registry — with the launcher attributed on
        // the Started event (docs/agent-tool.md).
        let (registry, mut rx) = test_registry();
        let executor = RealToolExecutor::new()
            .with_background(registry)
            .with_background_origin(crate::background::BgOrigin {
                agent_id: "a7k2m9x4q".into(),
                agent_type: "general-purpose".into(),
            });
        let out = executor.execute(
            &call(
                "bash",
                r#"{"command":"echo bg","run_in_background":true,"description":"noop"}"#,
            ),
            &CancelToken::new(),
            &mut |_| {},
        );
        assert!(out.ok, "got {}", out.output);
        assert!(out.background.is_some(), "resolves as backgrounded");
        let mut saw_origin = false;
        for _ in 0..500 {
            match rx.try_recv() {
                Ok(crate::background::BgEvent::Started { origin, .. }) => {
                    let origin = origin.expect("the launcher rides the event");
                    assert_eq!(origin.agent_id, "a7k2m9x4q");
                    assert_eq!(origin.agent_type, "general-purpose");
                    saw_origin = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        assert!(saw_origin, "the Started event carried the origin");
    }

    #[test]
    fn a_subagent_executor_never_consumes_the_ctrl_b_latch() {
        // Ctrl+B belongs to the MAIN turn's foreground command: a subagent's
        // concurrently-running bash must neither clear a pending request as
        // it starts nor take it mid-run (docs/agent-tool.md).
        let (registry, _rx) = test_registry();
        registry.request_background();
        let executor = RealToolExecutor::new()
            .with_background(registry.clone())
            .with_background_origin(crate::background::BgOrigin {
                agent_id: "a1".into(),
                agent_type: "explore".into(),
            });
        let out = executor.execute(
            &call("bash", r#"{"command":"echo hi"}"#),
            &CancelToken::new(),
            &mut |_| {},
        );
        assert!(out.ok);
        assert!(
            out.background.is_none(),
            "the pending latch never backgrounds a subagent's command"
        );
        assert!(
            registry.take_background_request(),
            "…and the latch is still raised for the main turn's runner"
        );
    }

    // --- interactive shells: `bash` with `tty`, and `bash_session`
    // (docs/interactive-shell.md) ---

    fn exec_with(executor: &RealToolExecutor, name: &str, args: &str) -> ToolOutcome {
        executor.execute(&call(name, args), &CancelToken::new(), &mut |_| {})
    }

    /// The session id a running report names on its frame line.
    fn session_of(output: &str) -> String {
        let first = output.lines().next().unwrap_or_default();
        match crate::pty::report::parse_frame(first) {
            Some(crate::pty::report::Frame::Running { session, .. }) => session.to_string(),
            other => panic!("not a running frame ({other:?}): {output}"),
        }
    }

    /// Every registry event that arrives within `window`.
    fn events_within(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::background::BgEvent>,
        window: std::time::Duration,
    ) -> Vec<crate::background::BgEvent> {
        let deadline = std::time::Instant::now() + window;
        let mut events = Vec::new();
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(event) => events.push(event),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        events
    }

    #[cfg(unix)]
    #[test]
    fn a_tty_command_that_finishes_in_its_call_reports_like_bash() {
        let (registry, mut rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"tty >/dev/null && echo on-a-terminal; exit 3","tty":true}"#,
        );
        assert!(!out.ok, "exit 3 is a failure: {}", out.output);
        assert_eq!(out.output, "Exit code: 3\non-a-terminal");
        assert!(out.background.is_none());
        assert!(
            events_within(&mut rx, std::time::Duration::from_millis(200)).is_empty(),
            "a command that finished inside its call is never listed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_prompt_returns_a_session_the_next_call_answers() {
        let (registry, mut rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"printf 'Name? '; read n; echo \"hi $n\"","tty":true}"#,
        );
        assert!(out.ok, "{}", out.output);
        let id = session_of(&out.output);
        assert_eq!(
            out.output,
            format!("Running (session {id}, waiting for input)\nName?")
        );
        let started = events_within(&mut rx, std::time::Duration::from_millis(200));
        assert!(
            started.iter().any(|e| matches!(
                e,
                crate::background::BgEvent::Started { id: started, .. } if *started == id
            )),
            "a session that outlived its call is listed: {started:?}"
        );
        let answered = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "World\n"}).to_string(),
        );
        assert!(answered.ok, "{}", answered.output);
        assert_eq!(answered.output, "Exit code: 0\nName? World\nhi World");
        let ended = events_within(&mut rx, std::time::Duration::from_millis(300));
        assert!(
            ended
                .iter()
                .any(|e| matches!(e, crate::background::BgEvent::Exited { observed: true, .. })),
            "the model saw the exit, so no notice is owed: {ended:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn codes_a_terminal_sends_programs_are_not_typed_and_the_model_hears_why() {
        // A model appended `\u001b[?25h` ("show the cursor") to its keys and
        // the program took it as typing (docs/interactive-shell.md).
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"printf 'Name? '; read n; echo \"hi [$n]\"","tty":true}"#,
        );
        let id = session_of(&out.output);
        let answered = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({
                "session_id": id,
                "input": "World\u{1b}[0m<Enter>\u{1b}[?25h",
            })
            .to_string(),
        );
        assert_eq!(answered.output, "Exit code: 0\nName? World\nhi [World]");
        let context = answered
            .context
            .expect("the model is told what was dropped");
        assert!(
            context.starts_with(&answered.output)
                && context.contains("\\e[0m")
                && context.contains("\\e[?25h"),
            "{context}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn html_escaped_keys_are_pressed_and_the_model_is_told() {
        // Seen live: `&lt;Esc&gt;` for `<Esc>`, typed into vim as text.
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"printf 'Name? '; read n; echo \"hi [$n]\"","tty":true}"#,
        );
        let id = session_of(&out.output);
        let answered = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "a &amp; b&lt;Enter&gt;"}).to_string(),
        );
        assert_eq!(answered.output, "Exit code: 0\nName? a & b\nhi [a & b]");
        let context = answered.context.expect("the model is told");
        assert!(context.contains("HTML-escaped"), "{context}");
    }

    #[cfg(unix)]
    #[test]
    fn indented_lines_reach_a_program_that_takes_pastes_as_one_paste() {
        // vim, bash, Python 3.13: a program in bracketed-paste mode is sent
        // typed code the way a person pastes it, so its auto-indent leaves
        // the indentation alone (docs/interactive-shell.md). This one turns
        // the mode on and shows exactly what reached it.
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let script = std::env::temp_dir().join(format!("paste-probe-{}.py", std::process::id()));
        std::fs::write(
            &script,
            "import os, tty\n\
             tty.setraw(0)\n\
             os.write(1, b'\\x1b[?2004h> ')\n\
             got = b''\n\
             while not got.endswith(b'\\x1b[201~'):\n\
             \x20   got += os.read(0, 1)\n\
             os.write(1, b'\\x1b[?2004l\\r\\n' + repr(got).encode() + b'\\r\\n')\n",
        )
        .expect("writes the probe");
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let out = exec_with(
            &executor,
            "bash",
            &serde_json::json!({"command": format!("python3 {}", script.display()), "tty": true})
                .to_string(),
        );
        let id = session_of(&out.output);
        let typed = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "def f():\n    return 1\nEND"})
                .to_string(),
        );
        std::fs::remove_file(&script).ok();
        assert!(
            typed
                .output
                .contains(r"b'def f():\x1b[200~\r    return 1\rEND\x1b[201~'"),
            "the first line typed, the rest one paste: {}",
            typed.output
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_wait_rides_out_a_silence_until_the_command_finishes() {
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"sleep 3; echo done","tty":true}"#,
        );
        let id = session_of(&out.output);
        assert!(out.output.ends_with("(no new output)"), "{}", out.output);
        let waited = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "timeout": 10_000}).to_string(),
        );
        assert_eq!(waited.output, "Exit code: 0\ndone");
    }

    #[cfg(unix)]
    #[test]
    fn kill_ends_a_session_and_says_so() {
        let (registry, mut rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"printf 'ready> '; sleep 60","tty":true}"#,
        );
        let id = session_of(&out.output);
        let stopped = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "kill": true}).to_string(),
        );
        assert!(stopped.ok, "a stop the model asked for is not a failure");
        assert!(
            stopped
                .output
                .starts_with(&format!("Stopped (session {id})")),
            "{}",
            stopped.output
        );
        assert!(registry.session(&id).is_none(), "the session is gone");
        let events = events_within(&mut rx, std::time::Duration::from_millis(300));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, crate::background::BgEvent::Exited { observed: true, .. })),
            "{events:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn input_with_kill_is_typed_first_and_the_kill_comes_after() {
        // Caught live: a model answered the last prompt and asked to stop in
        // the same call — and the stop used to win, so the answer was never
        // typed. "Type this, then end it" is what the pair means.
        let dir = tempfile::tempdir().unwrap();
        let out_file = dir.path().join("got.txt");
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let command = format!(
            "printf 'Sure? '; read x; echo \"got-$x\" > {}; sleep 60",
            out_file.display()
        );
        let out = exec_with(
            &executor,
            "bash",
            &serde_json::json!({"command": command, "tty": true}).to_string(),
        );
        let id = session_of(&out.output);
        let stopped = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "y\n", "kill": true}).to_string(),
        );
        assert_eq!(
            std::fs::read_to_string(&out_file).unwrap_or_default(),
            "got-y\n",
            "the input reached the program before the kill"
        );
        assert!(
            stopped
                .output
                .starts_with(&format!("Stopped (session {id})")),
            "{}",
            stopped.output
        );
        assert!(registry.session(&id).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn input_with_kill_reports_an_exit_the_input_caused_as_an_exit() {
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"printf 'Sure? '; read x; echo bye","tty":true}"#,
        );
        let id = session_of(&out.output);
        let ended = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "y\n", "kill": true}).to_string(),
        );
        assert_eq!(ended.output, "Exit code: 0\nSure? y\nbye");
    }

    #[cfg(unix)]
    #[test]
    fn a_poll_that_sees_nothing_new_still_names_the_prompt_it_waits_at() {
        // Caught live: a model polled a waiting program ten times, each
        // report saying only "Running" — the "waiting for input" of the
        // launch had gone, and nothing reminded it what was asked.
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"printf 'Full name: '; read n","tty":true}"#,
        );
        let id = session_of(&out.output);
        let polled = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "timeout": 300}).to_string(),
        );
        assert_eq!(
            polled.output,
            format!(
                "Running (session {id}, waiting for input)\n\
                 (no new output — still at: Full name:)"
            )
        );
        registry.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn input_with_kill_leaves_a_program_that_asks_for_more_running() {
        // Caught live: a model sent what it thought was the last answer with
        // `kill`, the program asked one more question, and the kill threw
        // the whole session away. A program waiting on the model is not done.
        let dir = tempfile::tempdir().unwrap();
        let out_file = dir.path().join("got.txt");
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let command = format!(
            "printf 'A? '; read a; printf 'Sure? '; read b; echo \"$a$b\" > {}",
            out_file.display()
        );
        let out = exec_with(
            &executor,
            "bash",
            &serde_json::json!({"command": command, "tty": true}).to_string(),
        );
        let id = session_of(&out.output);
        let asked = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "1\n", "kill": true}).to_string(),
        );
        assert!(asked.ok, "{}", asked.output);
        let report = format!("Running (session {id}, waiting for input)\nA? 1\nSure?");
        assert_eq!(asked.output, report, "the cell: what happened");
        assert_eq!(
            asked.context,
            Some(format!("{report}\n{}", crate::pty::report::NOT_KILLED_NOTE)),
            "the model: why its kill was not carried out"
        );
        assert!(registry.session(&id).is_some(), "still running");
        let done = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "y\n"}).to_string(),
        );
        assert_eq!(done.output, "Exit code: 0\nSure? y");
        assert_eq!(std::fs::read_to_string(&out_file).unwrap(), "1y\n");
    }

    #[cfg(unix)]
    #[test]
    fn typed_text_a_line_reading_prompt_has_not_received_is_pointed_out() {
        // Caught live: a model typed its answer without Enter, saw it on the
        // screen, and took it as given.
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"printf 'Name? '; read n; echo \"hi $n\"","tty":true}"#,
        );
        let id = session_of(&out.output);
        let typed = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "World", "timeout": 2000}).to_string(),
        );
        let report = format!("Running (session {id}, waiting for input)\nName? World");
        assert_eq!(typed.output, report);
        assert_eq!(
            typed.context,
            Some(format!(
                "{report}\n{}",
                crate::pty::report::UNSUBMITTED_NOTE
            ))
        );
        let submitted = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "<Enter>"}).to_string(),
        );
        assert_eq!(
            submitted.output, "Exit code: 0\nhi World",
            "the prompt line, already reported as it stands, is not repeated"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_command_reading_with_no_prompt_is_seen_waiting() {
        // `read x` prints nothing: no prompt by the cursor — but the kernel
        // says it is blocked reading the terminal (`pty::probe`), so the
        // launch reports it waiting, and soon.
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let started = Instant::now();
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"read x; echo \"got $x\"","tty":true}"#,
        );
        assert!(
            out.output.starts_with("Running (session ") && out.output.contains("waiting for input"),
            "{}",
            out.output
        );
        assert!(
            started.elapsed() < crate::pty::settle::LINE_QUIET,
            "settled on the read, not the quiet fallback"
        );
        let id = session_of(&out.output);
        let typed = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "hi\n"}).to_string(),
        );
        assert_eq!(typed.output, "Exit code: 0\nhi\ngot hi");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_busy_command_behind_a_prompt_shaped_line_is_not_waiting() {
        // `Working... ` left open while a sleep runs has a prompt's shape;
        // the kernel says every process is at work (`pty::probe`), so the
        // call never claims the command waits for input.
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"printf 'Working... '; sleep 3; echo done","tty":true}"#,
        );
        assert!(
            out.output.starts_with("Running (session ")
                && !out.output.contains("waiting for input"),
            "{}",
            out.output
        );
        let id = session_of(&out.output);
        let waited = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "timeout": 10000}).to_string(),
        );
        assert!(
            waited.output.starts_with("Exit code: 0"),
            "{}",
            waited.output
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_password_prompt_put_up_between_calls_ends_the_next_wait() {
        // sudo after a wrong password: `Sorry, try again.` and a fresh
        // prompt come up after the call that typed the password returned.
        // The kernel cannot see into sudo — a timed `read -s` waits in
        // `pselect6`, blind the same way — and a wait begun after the prompt
        // sees no output of its own: only the terminal's mode, a line read
        // with echo off, says it asks (docs/interactive-shell.md).
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let script = r#"bash -c 'read -p "Continue? " a; sleep 3; read -s -t 60 -p "Password: " p; echo; echo "got ${#p}"'"#;
        let out = exec_with(
            &executor,
            "bash",
            &serde_json::json!({"command": script, "tty": true}).to_string(),
        );
        let id = session_of(&out.output);
        let typed = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "y\n"}).to_string(),
        );
        assert!(
            !typed.output.contains("Password"),
            "returned before the prompt: {}",
            typed.output
        );
        let io = registry.session(&id).expect("still running").io;
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while !(registry
            .line_mode(&id)
            .is_some_and(crate::pty::spawn::LineMode::hides_input)
            && io.screen_text().unwrap_or_default().contains("Password:"))
        {
            assert!(
                Instant::now() < deadline,
                "the prompt never came up: {:?} {:?}",
                registry.line_mode(&id),
                io.screen_text()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let started = Instant::now();
        let waited = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "timeout": 8000}).to_string(),
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "{:?}: {}",
            started.elapsed(),
            waited.output
        );
        assert_eq!(
            waited.output,
            format!(
                "Running (session {id}, waiting for a password — typed input is hidden)\nPassword:"
            )
        );
        let answered = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "hunter2\n"}).to_string(),
        );
        assert_eq!(answered.output, "Exit code: 0\ngot 7");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_prompt_put_up_between_calls_ends_the_next_wait() {
        // The command works quietly, then asks — after the call that
        // started it returned, while the model was deciding what to do. The
        // kernel cannot see the timed read (`pselect6`, blind the way it is
        // behind sudo), and it is no password: a wait begun after the
        // question still ends on it, since the model has not seen it
        // (docs/interactive-shell.md).
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let script =
            r#"bash -c 'echo working; sleep 3; read -t 60 -p "Proceed? [Y/n] " a; echo "got $a"'"#;
        let out = exec_with(
            &executor,
            "bash",
            &serde_json::json!({"command": script, "tty": true}).to_string(),
        );
        let id = session_of(&out.output);
        assert!(
            !out.output.contains("Proceed"),
            "returned before the question: {}",
            out.output
        );
        let io = registry.session(&id).expect("still running").io;
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while !io.screen_text().unwrap_or_default().contains("Proceed?") {
            assert!(Instant::now() < deadline, "the question never came up");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let started = Instant::now();
        let waited = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "timeout": 8000}).to_string(),
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(6),
            "{:?}: {}",
            started.elapsed(),
            waited.output
        );
        assert_eq!(
            waited.output,
            format!("Running (session {id}, waiting for input)\nProceed? [Y/n]")
        );
        let answered = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "y\n"}).to_string(),
        );
        assert_eq!(answered.output, "Exit code: 0\nProceed? [Y/n] y\ngot y");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_submitted_password_is_answered_in_the_call_that_typed_it() {
        // sudo checks a password for seconds before it answers — longer
        // than a call waits on silence. The call that submitted it waits for
        // the verdict, a refusal and a fresh prompt or the command itself,
        // rather than returning "no new output" mid-check.
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let script = r#"bash -c 'for i in 1 2; do read -s -t 60 -p "Password: " p; echo; sleep 3; [ "$p" = hunter2 ] && { echo ok; exit 0; }; echo "Sorry, try again."; done; exit 1'"#;
        let out = exec_with(
            &executor,
            "bash",
            &serde_json::json!({"command": script, "tty": true}).to_string(),
        );
        let id = session_of(&out.output);
        let refused = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "letmein\n"}).to_string(),
        );
        assert_eq!(
            refused.output,
            format!(
                "Running (session {id}, waiting for a password — typed input is hidden)\n\
                 Sorry, try again.\nPassword:"
            )
        );
        let accepted = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "hunter2\n"}).to_string(),
        );
        assert_eq!(accepted.output, "Exit code: 0\nok");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_launch_that_asks_for_a_password_says_so() {
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let script = r#"bash -c 'read -s -t 60 -p "Password: " p; echo; echo "got ${#p}"'"#;
        let started = Instant::now();
        let out = exec_with(
            &executor,
            "bash",
            &serde_json::json!({"command": script, "tty": true}).to_string(),
        );
        assert!(
            started.elapsed() < crate::pty::settle::LINE_QUIET,
            "{:?}",
            started.elapsed()
        );
        let id = session_of(&out.output);
        assert_eq!(
            out.output,
            format!(
                "Running (session {id}, waiting for a password — typed input is hidden)\nPassword:"
            )
        );
        let answered = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "hunter2\n"}).to_string(),
        );
        assert_eq!(answered.output, "Exit code: 0\ngot 7");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_password_typed_without_its_enter_leaves_the_prompt_asking() {
        // The terminal holds the line until Enter: nothing has reached the
        // program, which still asks — said at once, not on the quiet
        // fallback, beside the model's note about the missing Enter.
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let script = r#"bash -c 'read -s -t 60 -p "Password: " p; echo; echo "got ${#p}"'"#;
        let out = exec_with(
            &executor,
            "bash",
            &serde_json::json!({"command": script, "tty": true}).to_string(),
        );
        let id = session_of(&out.output);
        let started = Instant::now();
        let typed = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "hunter2"}).to_string(),
        );
        assert!(
            started.elapsed() < crate::pty::settle::LINE_QUIET,
            "{:?}",
            started.elapsed()
        );
        assert!(
            typed.output.starts_with(&format!(
                "Running (session {id}, waiting for a password — typed input is hidden)\n"
            )),
            "{}",
            typed.output
        );
        assert!(
            typed
                .context
                .as_deref()
                .is_some_and(|context| context.ends_with(crate::pty::report::UNSUBMITTED_NOTE)),
            "{:?}",
            typed.context
        );
        let answered = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "<Enter>"}).to_string(),
        );
        assert_eq!(answered.output, "Exit code: 0\ngot 7");
    }

    /// The refined headers a call sent, in order.
    fn titles_of(
        executor: &RealToolExecutor,
        name: &str,
        args: &str,
    ) -> (ToolOutcome, Vec<String>) {
        let mut titles = Vec::new();
        let out = executor.execute(&call(name, args), &CancelToken::new(), &mut |progress| {
            if let ToolProgress::Title(title) = progress {
                titles.push(title.to_string());
            }
        });
        (out, titles)
    }

    #[cfg(unix)]
    #[test]
    fn a_session_calls_header_names_its_command_and_shows_the_keys_typed() {
        // The header says what the keys go to, and shows them as typed — at
        // a prompt that reads with echo off too: masking there hid ordinary
        // input as often as a password (docs/interactive-shell.md).
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"printf 'Code: '; stty -echo; read c; stty echo; echo; echo \"got $c\"","tty":true}"#,
        );
        let id = session_of(&out.output);
        let (typed, titles) = titles_of(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "1234\n"}).to_string(),
        );
        assert_eq!(titles.len(), 1, "{titles:?}");
        assert!(titles[0].starts_with("printf 'Code: '"), "{titles:?}");
        assert!(titles[0].ends_with(" ← 1234⏎"), "{titles:?}");
        assert!(typed.output.contains("got 1234"), "{}", typed.output);
    }

    #[cfg(unix)]
    #[test]
    fn ctrl_b_on_a_session_call_ends_the_wait_and_tells_the_model() {
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"printf 'working'; sleep 30","tty":true}"#,
        );
        let id = session_of(&out.output);
        let latch = registry.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            latch.request_background();
        });
        let started = std::time::Instant::now();
        let waited = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "timeout": 20_000}).to_string(),
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the wait ended at the key"
        );
        assert!(waited.ok);
        assert!(registry.session(&id).is_some(), "the session runs on");
        assert!(
            waited
                .context
                .as_deref()
                .is_some_and(|text| text.ends_with(crate::pty::report::WAIT_ENDED_NOTE)),
            "{:?}",
            waited.context
        );
        registry.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn text_a_line_editor_holds_unsubmitted_is_pointed_out_too() {
        // Caught live: Python's REPL reads key by key (readline), so the
        // line-mode check never saw its unsubmitted line — the model polled
        // three times, "waiting for the output to flush". The echo at the
        // prompt, with the cursor right after it, is what gives it away.
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let editor = r#"bash -c 'stty -icanon -echo; printf ">>> "; while IFS= read -r -s -n1 c; do printf "%s" "$c"; done'"#;
        let out = exec_with(
            &executor,
            "bash",
            &serde_json::json!({"command": editor, "tty": true}).to_string(),
        );
        let id = session_of(&out.output);
        let typed = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "print(1)"}).to_string(),
        );
        let report = format!("Running (session {id}, waiting for input)\n>>> print(1)");
        assert_eq!(typed.output, report);
        assert_eq!(
            typed.context,
            Some(format!(
                "{report}\n{}",
                crate::pty::report::UNSUBMITTED_NOTE
            ))
        );
        registry.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn a_program_reading_key_by_key_gets_no_submit_note() {
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"stty -icanon -echo; printf 'key> '; k=$(dd bs=1 count=1 2>/dev/null); printf \"got $k\\nkey> \"; dd bs=1 count=1 >/dev/null 2>&1; sleep 30","tty":true}"#,
        );
        let id = session_of(&out.output);
        let pressed = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "x"}).to_string(),
        );
        assert_eq!(
            pressed.output,
            format!("Running (session {id}, waiting for input)\nkey> got x\nkey>"),
            "the key already reached it"
        );
        assert_eq!(pressed.context, None, "nothing to add for the model");
        registry.kill_all();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_program_started_and_typed_into_in_one_call_reads_its_lines() {
        // Seen driving an interactive bash: `python3` and a loop for it in
        // one call went to bash as one paste — python3 started with nothing
        // to read, and the loop ran as shell commands. Typed as a person
        // types, the lines after the one that starts python3 are its to read.
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"exec bash --norc --noprofile -i","tty":true}"#,
        );
        let id = session_of(&out.output);
        let typed = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({
                "session_id": id,
                "input": "python3 -q\nfor i in range(3):\n    print(i * 7)\n\n"
            })
            .to_string(),
        );
        assert!(typed.output.contains("\n14\n"), "{}", typed.output);
        assert!(!typed.output.contains("syntax error"), "{}", typed.output);
        registry.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn keys_typed_to_a_program_reading_key_by_key_arrive_one_read_each() {
        // Seen driving top: `Mq` written at once was one read, which top
        // took for no key — it never sorted and never quit. A program in raw
        // mode on the main screen gets short text a character at a time.
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"stty -icanon -echo; printf 'keys> '; while :; do k=$(dd bs=64 count=1 2>/dev/null); [ \"$k\" = q ] && exit 0; printf \"[$k]\"; done","tty":true}"#,
        );
        let id = session_of(&out.output);
        let typed = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "Mq"}).to_string(),
        );
        assert_eq!(typed.output, "Exit code: 0\nkeys> [M]");
        registry.kill_all();
    }
    #[cfg(unix)]
    #[test]
    fn an_unknown_session_lists_the_running_ones() {
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let none = exec_with(&executor, BASH_SESSION, r#"{"session_id":"bnope"}"#);
        assert!(!none.ok);
        assert!(none.output.contains("bnope"), "{}", none.output);
        assert!(
            none.output.contains("No sessions are running"),
            "{}",
            none.output
        );
        let task = registry
            .launch("sleep 30", Some("nap".into()), true)
            .expect("launches");
        let listed = exec_with(&executor, BASH_SESSION, r#"{"session_id":"bnope"}"#);
        assert!(
            listed.output.contains(&format!("{} (sleep 30)", task.id)),
            "{}",
            listed.output
        );
        registry.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn tty_with_run_in_background_returns_the_session_at_once() {
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"cat","tty":true,"run_in_background":true}"#,
        );
        assert!(out.ok, "{}", out.output);
        let id = out.background.clone().expect("resolves as backgrounded");
        assert!(
            out.output.contains(&format!("session {id}")),
            "{}",
            out.output
        );
        assert!(out.output.contains("bash_session"), "{}", out.output);
        let echoed = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "hello\n"}).to_string(),
        );
        assert!(
            echoed.output.ends_with("hello\nhello"),
            "the terminal's echo, then cat's copy: {}",
            echoed.output
        );
        registry.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn a_background_pipe_command_takes_ctrl_c_but_no_typing() {
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"sleep 30","run_in_background":true}"#,
        );
        let id = out.background.expect("backgrounded");
        let refused = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "y\n"}).to_string(),
        );
        assert!(!refused.ok);
        assert!(refused.output.contains("tty: true"), "{}", refused.output);
        // The child needs its moment to lead its own group (see the
        // registry's own interrupt test).
        std::thread::sleep(std::time::Duration::from_millis(300));
        let interrupted = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": id, "input": "<C-c>"}).to_string(),
        );
        assert!(
            interrupted.output.starts_with("Exit code:"),
            "SIGINT ended it: {}",
            interrupted.output
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_cancelled_tty_launch_takes_its_session_down() {
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            canceller.cancel();
        });
        let start = std::time::Instant::now();
        let out = executor.execute(
            &call("bash", r#"{"command":"sleep 60","tty":true}"#),
            &cancel,
            &mut |_| {},
        );
        assert!(!out.ok);
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
        // The kill is immediate; the monitor lets go of the task a moment
        // later, once it has reaped the process.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !registry.sessions().is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "nothing left running: {:?}",
                registry.sessions()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn a_plain_command_that_wanted_a_terminal_hints_at_tty_for_the_model() {
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let out = exec_with(
            &executor,
            "bash",
            r#"{"command":"echo 'error: must be run in an interactive terminal' >&2; exit 2"}"#,
        );
        assert!(!out.ok);
        assert_eq!(
            out.output, "Exit code: 2\nerror: must be run in an interactive terminal\n",
            "the cell shows the command's own output"
        );
        assert_eq!(
            out.context.as_deref(),
            Some(
                format!(
                    "Exit code: 2\nerror: must be run in an interactive terminal\n\n{}",
                    crate::llm::tools::TTY_HINT
                )
                .as_str()
            ),
            "the model reads the hint beside it"
        );
        // Without the registry there is no tty to point at.
        let bare = exec(
            "bash",
            r#"{"command":"echo 'error: must be run in an interactive terminal' >&2; exit 2"}"#,
        );
        assert_eq!(bare.context, None);
        // A failure that says nothing of terminals gets nothing.
        let plain = exec_with(&executor, "bash", r#"{"command":"exit 3"}"#);
        assert_eq!(plain.context, None);
    }

    #[cfg(unix)]
    #[test]
    fn a_terminal_calls_cell_redraws_a_progress_bar_in_place() {
        // The running cell streams what the report will say: the bar is one
        // row replaced as it moves, never its every frame
        // (docs/interactive-shell.md).
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let command = r"printf 'fetching\n'; for p in 10 40 70 100; do printf '\r%3d%%' $p; sleep 0.2; done; printf '\n'";
        let mut view = String::new();
        let mut live_len = 0;
        let mut frames = Vec::new();
        let out = executor.execute(
            &call(
                "bash",
                &serde_json::json!({"command": command, "tty": true}).to_string(),
            ),
            &CancelToken::new(),
            &mut |progress| {
                if let ToolProgress::Screen { settled, live } = progress {
                    live_len = crate::app::apply_tool_screen(&mut view, live_len, settled, live);
                    frames.push(view.clone());
                }
            },
        );
        assert_eq!(out.output, "Exit code: 0\nfetching\n100%");
        assert!(
            frames.iter().any(|frame| frame == "fetching\n 40%"),
            "the bar streamed as it moved: {frames:?}"
        );
        assert!(
            frames.iter().all(|frame| frame.matches('%').count() <= 1),
            "one row per bar, never two frames at once: {frames:?}"
        );
        registry.kill_all();
    }

    #[cfg(unix)]
    #[test]
    fn a_relayed_progress_display_is_waited_out_and_reported_once() {
        // `sudo pacman -Syy` as its terminal sees it: sudo relays with the
        // terminal raw and its output processing off, and pacman redraws its
        // download bars in place, parking the cursor at the start of a bar
        // between updates. Neither is a prompt — a wait rides it out to the
        // exit — and a look carries only the lines that changed since the
        // previous one, so the finished bars are not repeated.
        let (registry, _rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let script = concat!(
            r"stty raw -echo; ",
            r"printf ':: Synchronizing package databases...\r\n core\r\n extra\r\n multilib\r\n'; ",
            r"printf '\033[3F core     100%%\r\033[2E multilib 100%%\r\033[1F extra      10%%\r'; ",
            r"sleep 2.5; printf ' extra      50%%\r'; sleep 0.8; ",
            r"printf ' extra     100%%\r\033[2E'",
        );
        let launch = exec_with(
            &executor,
            "bash",
            &serde_json::json!({"command": script, "tty": true}).to_string(),
        );
        let first = launch.output.lines().next().unwrap_or_default();
        let Some(crate::pty::report::Frame::Running { session, waiting }) =
            crate::pty::report::parse_frame(first)
        else {
            panic!("still running at the launch's return: {}", launch.output);
        };
        assert_eq!(
            waiting,
            crate::pty::report::Waiting::No,
            "a relay's raw terminal is no prompt: {}",
            launch.output
        );
        assert!(
            launch
                .output
                .ends_with(" core     100%\n extra      10%\n multilib 100%"),
            "{}",
            launch.output
        );
        let session = session.to_string();
        let waited = exec_with(
            &executor,
            BASH_SESSION,
            &serde_json::json!({"session_id": session, "timeout": 10000}).to_string(),
        );
        assert_eq!(
            waited.output, "Exit code: 0\n extra     100%",
            "one wait to the exit, and only the bar that moved"
        );
        registry.kill_all();
    }

    #[test]
    fn tty_and_sessions_need_the_registry() {
        let out = exec("bash", r#"{"command":"python3","tty":true}"#);
        assert!(!out.ok);
        assert!(out.output.contains("without tty"), "{}", out.output);
        let out = exec(BASH_SESSION, r#"{"session_id":"b1"}"#);
        assert!(!out.ok);
    }

    const BASH_SESSION: &str = crate::llm::tools::BASH_SESSION_TOOL_NAME;
}
