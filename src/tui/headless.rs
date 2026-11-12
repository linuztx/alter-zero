//! `alter-zero -p`: the session with no screen (`docs/cli/headless.md`).
//!
//! A headless run is the TUI's own [`Session`] — the same bootstrap, the same
//! turn starts, the same stream folding, notice settling and follow-up turns
//! — over a viewport that paints nowhere ([`InlineViewport::headless`]) and
//! with no key reader at all. What this module replaces is the terminal half
//! of the event loop: [`run`] selects over the reply, background, subagent
//! and MCP channels plus the process signals, and hands what the session
//! settles to the library's [`Reporter`], which decides every line written.
//!
//! Three things happen here that the TUI never needs:
//!
//! - **The prompt is read before the runtime** ([`read_prompt`]): the
//!   argument, stdin, or both — and an open pipe that never says anything is
//!   left alone after [`STDIN_WAIT`](alter_zero::headless::STDIN_WAIT), so a
//!   parent that never closes it cannot hold the run forever.
//! - **The run decides when it is over**: no turn in flight, nothing queued,
//!   no subagent still working — then whatever background command still runs
//!   is stopped, and the process exits with the status the run earned.
//! - **Signals are handled**, not suffered: shells live in sessions of their
//!   own and MCP servers are children, so dying on SIGINT's default action
//!   would orphan them. The run shuts down the TUI's way and reports.

use std::io::{IsTerminal, Write};
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use alter_zero::app::{HistoryItem, Role};
use alter_zero::headless::{
    self as core, Denial, Emit, Reporter, RunEnd, RunOutcome, RunStart, RunUsage, StdinKind,
    StdinRead,
};
use alter_zero::mcp::McpServerStatus;
use alter_zero::stream::StreamEvent;
use alter_zero::term::InlineViewport;

use super::startup::{Halt, Startup, StartupSession};
use super::{Session, config};

/// What a run with no model to answer it says, and why it refuses rather
/// than fall back to the offline demo the TUI meets a first run with: a CI
/// job "succeeding" on a canned reply is the worst thing a misconfiguration
/// could do.
const NO_MODEL: &str = "no model is configured: sign in and pick one with /login and /model in \
     an interactive session, or set ALTER_ZERO_PROVIDER, ALTER_ZERO_MODEL and the provider's key \
     (ALTER_ZERO_DUMMY=1 runs the offline demo)";

/// How long past each MCP server's own connect budget the run waits for the
/// servers to settle before its first request — a bound on a server the
/// manager somehow never resolves.
const MCP_SETTLE_GRACE: Duration = Duration::from_secs(5);

/// SIGINT and SIGTERM — POSIX's numbers on every platform this builds for.
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

// ===== before the runtime: the prompt =====

/// Read a headless run's prompt (`docs/cli/headless.md` *The prompt*): the
/// argument, stdin, or both, by [`core::stdin_read`]'s rule. Runs in
/// `resolve_cli`, before the runtime and before anything starts, so a usage
/// mistake prints and exits like every other one: [`Halt::Usage`] for
/// nothing to answer, [`Halt::Failed`] for input that could not be read.
pub(crate) fn read_prompt(arg: Option<&str>) -> Result<String, Halt> {
    let stdin = std::io::stdin();
    let kind = stdin_kind(&stdin);
    let input = match core::stdin_read(arg, kind) {
        StdinRead::Skip => None,
        StdinRead::ToEnd => Some(read_stdin(&stdin, None)?),
        StdinRead::ToEndNoting(after) => Some(read_stdin(&stdin, Some(after))?),
        StdinRead::IfActiveWithin(wait) => {
            if readable_within(&stdin, wait) {
                Some(read_stdin(&stdin, Some(core::STDIN_NOTE_AFTER))?)
            } else {
                write(&[core::warning(&format!(
                    "stdin said nothing for {}s, so the run goes on without it \
                     (pass - as the prompt to wait for it)",
                    wait.as_secs()
                ))]);
                None
            }
        }
    };
    core::combine_prompt(arg, input.as_deref()).ok_or(Halt::Usage(if kind == StdinKind::Terminal {
        "--print needs a prompt: pass it as an argument, or pipe it on stdin"
    } else {
        "the prompt is empty"
    }))
}

/// What stdin is: a terminal, a regular file, a pipe, or another stream
/// that may never end — told apart by the descriptor's own metadata, read
/// through a duplicate so nothing of stdin itself is touched.
fn stdin_kind(stdin: &std::io::Stdin) -> StdinKind {
    use std::os::unix::fs::FileTypeExt;
    if stdin.is_terminal() {
        return StdinKind::Terminal;
    }
    let kind = stdin
        .as_fd()
        .try_clone_to_owned()
        .ok()
        .map(std::fs::File::from)
        .and_then(|file| file.metadata().ok())
        .map(|meta| meta.file_type());
    match kind {
        Some(kind) if kind.is_file() => StdinKind::File,
        Some(kind) if kind.is_fifo() => StdinKind::Pipe,
        _ => StdinKind::Stream,
    }
}

/// Whether stdin has anything to say — data, or its end — within `wait`.
fn readable_within(stdin: &std::io::Stdin, wait: Duration) -> bool {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let timeout = Timespec {
        tv_sec: i64::try_from(wait.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(wait.subsec_nanos()),
    };
    let fd = stdin.as_fd();
    // A hang-up counts: poll reports the end of a pipe whether asked or not.
    matches!(
        poll(&mut [PollFd::new(&fd, PollFlags::IN)], Some(&timeout)),
        Ok(ready) if ready > 0
    )
}

/// Read stdin to its end — at most [`core::STDIN_MAX_BYTES`] — and decode
/// it. With `note_after`, a stdin that has not ended by then is said on
/// stderr, once, and still read to its end. Read from the descriptor itself,
/// never through std's buffer, so `poll` sees everything still to come.
fn read_stdin(stdin: &std::io::Stdin, note_after: Option<Duration>) -> Result<String, Halt> {
    let began = Instant::now();
    let mut noted = note_after.is_none();
    let mut bytes = Vec::new();
    let mut chunk = vec![0; 64 * 1024];
    loop {
        if !noted {
            let left = note_after
                .unwrap_or_default()
                .saturating_sub(began.elapsed());
            if !readable_within(stdin, left) {
                write(&[core::warning(
                    "still reading stdin — the run starts once it ends (add < /dev/null \
                     if nothing is piped in)",
                )]);
                noted = true;
                continue;
            }
        }
        let read = match rustix::io::read(stdin.as_fd(), &mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(rustix::io::Errno::INTR) => continue,
            Err(err) => return Err(Halt::Failed(format!("Could not read stdin: {err}"))),
        };
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > core::STDIN_MAX_BYTES {
            return Err(Halt::Failed(format!(
                "stdin is over {} MiB — save it to a file and name the file in the prompt instead",
                core::STDIN_MAX_BYTES / (1024 * 1024)
            )));
        }
    }
    let decoded = core::decode_stdin(bytes);
    if decoded.replaced {
        write(&[core::warning(
            "stdin is not all UTF-8: what is not reads as \u{fffd}",
        )]);
    }
    Ok(decoded.text)
}

// ===== the streams =====

/// Write what the reporter decided, each line to its stream, flushed at
/// once — a pipeline reading the stream must not wait on a buffer. A write
/// that fails (a reader that went away) is dropped: the run still finishes
/// and shuts down what it started.
pub(crate) fn write(emits: &[Emit]) {
    if emits.is_empty() {
        return;
    }
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    for emit in emits {
        let _ = match emit {
            Emit::Stdout(line) => writeln!(stdout, "{line}"),
            Emit::Stderr(line) => writeln!(stderr, "{line}"),
        };
    }
    let _ = stdout.flush();
    let _ = stderr.flush();
}

// ===== the signals =====

/// The two signals a headless run answers by shutting down rather than
/// dying (`docs/cli/headless.md` *Exit status*). Either may fail to install —
/// the run then simply has the default action for it.
struct Signals {
    interrupt: Option<tokio::signal::unix::Signal>,
    terminate: Option<tokio::signal::unix::Signal>,
}

impl Signals {
    fn install() -> Self {
        use tokio::signal::unix::{SignalKind, signal};
        Self {
            interrupt: signal(SignalKind::interrupt()).ok(),
            terminate: signal(SignalKind::terminate()).ok(),
        }
    }

    /// The next signal's number — never, when neither could be installed.
    async fn recv(&mut self) -> i32 {
        let Self {
            interrupt,
            terminate,
        } = self;
        tokio::select! {
            Some(()) = next_signal(interrupt) => SIGINT,
            Some(()) = next_signal(terminate) => SIGTERM,
            else => std::future::pending().await,
        }
    }
}

async fn next_signal(signal: &mut Option<tokio::signal::unix::Signal>) -> Option<()> {
    match signal {
        Some(signal) => signal.recv().await,
        None => std::future::pending().await,
    }
}

// ===== the run =====

/// Run one headless session to its end and return the process's exit status
/// (`docs/cli/headless.md`): bootstrap, wait for the MCP servers, submit the
/// prompt, fold every event until the session is idle with nothing left
/// running that owes it a turn, then shut down and report.
pub(crate) async fn run(mut startup: Startup) -> i32 {
    let began = Instant::now();
    let options = startup.print.unwrap_or_default();
    let format = options.output_format;
    // The headless loop submits the prompt itself, once the MCP servers have
    // settled — bootstrap never sees it.
    let prompt = startup.prompt.take().unwrap_or_default();
    let resumed = matches!(startup.session, Some(StartupSession::Load(_)));
    // Before bootstrap, which starts the MCP servers: a Ctrl+C from here on
    // takes them down with the run rather than orphaning them.
    let mut signals = Signals::install();
    let mut term = InlineViewport::headless();
    let mut session = match Session::bootstrap(&mut term, startup) {
        Ok(session) => session,
        Err(err) => {
            let error = format!("could not start the session: {err}");
            write(&core::not_started(format, &error, began.elapsed()));
            return 1;
        }
    };
    if !session.models.is_real() && !config::dummy_forced() && config::stall_ms().is_none() {
        let _ = session.shutdown();
        write(&core::not_started(format, NO_MODEL, began.elapsed()));
        return 1;
    }
    session.report_held_for_review();
    let mut reporter = Reporter::new(format, options.verbose);
    let mut interrupted = session.settle_mcp(&mut signals).await;
    session.report_mcp_unavailable();
    let run_start = session.app.history.len();
    let mut emitted = run_start;
    let start = session.run_start_info(resumed);
    write(&reporter.start(&start.as_start()));
    // A conversation past its window compacts before the prompt goes out —
    // the TUI's idle-boundary rule, run once here (docs/architecture/compact.md).
    session.after_headless_iteration(true);
    let mut prompt = interrupted.is_none().then_some(prompt);
    // How long the run may sit with nothing left but background subagents
    // at work, and since when it has (docs/cli/headless.md).
    let agent_wait = core::agent_wait(std::env::var(core::AGENT_WAIT_ENV).ok().as_deref());
    let mut agents_since: Option<Instant> = None;
    let mut stopped_agents: Vec<String> = Vec::new();
    while interrupted.is_none() {
        if session.inflight.is_none()
            && let Some(prompt) = prompt.take()
        {
            session.submit_startup_prompt(prompt);
        }
        session.emit_settled(&mut reporter, run_start, &mut emitted);
        if prompt.is_none() && session.headless_idle() {
            // Something already queued — a shell that just ended, an agent
            // that just settled — may still start a turn: take it before
            // calling the run over. What decides is whether a turn started,
            // never that something arrived: a command still printing in the
            // background sends output for as long as it runs.
            session.drain_queued_events(&mut reporter, run_start, &mut emitted);
            if session.headless_idle() {
                break;
            }
            continue;
        }
        // Idle but for subagents still working: the wait on them is bounded.
        // A notice already in hand is not waited on — its turn is coming.
        let agents_deadline = if prompt.is_none() && session.idle_on_working_agents() {
            Some(*agents_since.get_or_insert_with(Instant::now) + agent_wait)
        } else {
            agents_since = None;
            None
        };
        if agents_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            // An agent that settled meanwhile still gets its turn.
            session.drain_queued_events(&mut reporter, run_start, &mut emitted);
            if session.idle_on_working_agents() {
                stopped_agents = session.working_agents();
                break;
            }
            continue;
        }
        tokio::select! {
            Some(event) = session.reply_rx.recv() => {
                session.on_headless_reply(event, &mut reporter, run_start, &mut emitted);
            }
            Some(event) = session.bg_rx.recv() => session.on_bg_event(event),
            Some(event) = session.agent_rx.recv() => session.on_headless_agent_message(event),
            Some(event) = session.mcp_rx.recv() => session.on_mcp_event(event),
            Some((provider, result)) = session.probe_rx.recv(),
                if session.models.probe_pending() =>
            {
                session.apply_capability_probe(&provider, result);
            }
            // The frame scheduler still ticks for a session that paints
            // nothing; its ticks are taken and dropped.
            Some(()) = session.draw_rx.recv() => {}
            signal = signals.recv() => interrupted = Some(signal),
            () = tokio::time::sleep_until(tokio::time::Instant::from_std(
                agents_deadline.unwrap_or_else(Instant::now),
            )), if agents_deadline.is_some() => {}
        }
        // Compaction at the loop bottom only while the run waits on its
        // subagents — the idle boundary a follow-up turn will come out of.
        let waiting = session.waiting_on_agents();
        session.after_headless_iteration(waiting);
    }
    session.emit_settled(&mut reporter, run_start, &mut emitted);
    session.finish(
        &mut reporter,
        run_start,
        &start,
        began,
        interrupted,
        &stopped_agents,
    )
}

/// What the run announced itself as, kept for its report.
struct StartInfo {
    session_id: String,
    model: String,
    cwd: String,
    cwd_display: String,
    permission_mode: Option<alter_zero::permission::PermissionMode>,
    resumed: bool,
}

impl StartInfo {
    fn as_start(&self) -> RunStart<'_> {
        RunStart {
            version: env!("CARGO_PKG_VERSION"),
            session_id: Some(&self.session_id),
            model: &self.model,
            cwd: &self.cwd,
            cwd_display: &self.cwd_display,
            permission_mode: self.permission_mode,
            resumed: self.resumed,
        }
    }
}

impl Session<'_> {
    /// What a headless run announces as it starts.
    fn run_start_info(&self, resumed: bool) -> StartInfo {
        StartInfo {
            session_id: self.recorder.conversation_id().to_string(),
            model: self.models.model_name(),
            cwd: self.cwd.display().to_string(),
            cwd_display: self.cwd_display.clone(),
            permission_mode: self.permissions.mode(),
            resumed,
        }
    }

    /// Say what the startup review would have asked about and nobody could
    /// (`docs/cli/headless.md` *Trust*): it stays off, exactly as after a
    /// declined review, and an interactive run in this directory asks.
    fn report_held_for_review(&mut self) {
        let mut hooks = 0;
        let mut servers = 0;
        for page in self.review_pages(true, true) {
            let items: usize = page
                .sections
                .iter()
                .map(|section| section.items.len())
                .sum();
            match page.kind {
                alter_zero::app::ReviewKind::Hooks => hooks += items,
                alter_zero::app::ReviewKind::McpServers => servers += items,
            }
        }
        let mut waiting = Vec::new();
        if hooks > 0 {
            waiting.push(format!("{hooks} hook{}", if hooks == 1 { "" } else { "s" }));
        }
        if servers > 0 {
            waiting.push(format!(
                "{servers} MCP server{}",
                if servers == 1 { "" } else { "s" }
            ));
        }
        if !waiting.is_empty() {
            write(&[core::warning(&format!(
                "{} not trusted yet, so off for this run — run alter-zero here interactively \
                 to review them",
                waiting.join(" and ")
            ))]);
        }
    }

    /// Wait until no MCP server is still connecting — each is bounded by its
    /// own startup budget — so the first request carries every server's
    /// tools: a TUI turn simply runs without the ones still connecting, but
    /// a headless run is over in seconds. A signal meanwhile ends the wait
    /// and the run.
    async fn settle_mcp(&mut self, signals: &mut Signals) -> Option<i32> {
        let deadline = Instant::now() + config::mcp_startup_timeout() + MCP_SETTLE_GRACE;
        while self.mcp_connecting() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            tokio::select! {
                Some(event) = self.mcp_rx.recv() => self.on_mcp_event(event),
                Some(()) = self.draw_rx.recv() => {}
                () = tokio::time::sleep(left) => break,
                signal = signals.recv() => return Some(signal),
            }
        }
        None
    }

    fn mcp_connecting(&self) -> bool {
        self.mcp.is_active()
            && self
                .mcp
                .snapshot()
                .iter()
                .any(|server| server.status == McpServerStatus::Pending)
    }

    /// Name the MCP servers that will not serve this run — a failed connect,
    /// a sign-in only an interactive session can do.
    fn report_mcp_unavailable(&self) {
        let warnings: Vec<Emit> = self
            .mcp
            .snapshot()
            .iter()
            .filter_map(|server| match &server.status {
                McpServerStatus::Failed(reason) => Some(core::warning(&format!(
                    "MCP server {} is unavailable: {reason}",
                    server.name
                ))),
                McpServerStatus::NeedsAuth => Some(core::warning(&format!(
                    "MCP server {} needs a sign-in — /mcp in an interactive session",
                    server.name
                ))),
                McpServerStatus::Pending => Some(core::warning(&format!(
                    "MCP server {} is still connecting — the run goes on without it",
                    server.name
                ))),
                _ => None,
            })
            .collect();
        write(&warnings);
    }

    /// One subagent event, folded as the TUI folds it — except a permission
    /// request, which nobody here can answer: the unattended gate refuses
    /// before raising one, so one that arrived anyway is refused, never shown,
    /// or the agent's thread would wait on it for the rest of the run.
    fn on_headless_agent_message(&mut self, message: alter_zero::agents::AgentEvent) {
        if let alter_zero::agents::AgentEvent::Stream {
            event: StreamEvent::Permission(request),
            ..
        } = &message
        {
            self.permissions.resolve(
                &request.id,
                alter_zero::permission::PermissionDecision::Deny(None),
            );
            return;
        }
        self.on_agent_message(message);
    }

    /// One reply event, folded exactly as the TUI folds it — then reported.
    fn on_headless_reply(
        &mut self,
        event: StreamEvent,
        reporter: &mut Reporter,
        run_start: usize,
        emitted: &mut usize,
    ) {
        // Nobody can answer a prompt here. The unattended gate never raises
        // one and the ask tool is never offered, so these do not arrive —
        // but a thread parked on one would hold the run forever, so one that
        // somehow did is refused, never shown.
        match &event {
            StreamEvent::Permission(request) => {
                self.permissions.resolve(
                    &request.id,
                    alter_zero::permission::PermissionDecision::Deny(None),
                );
                return;
            }
            StreamEvent::AskUser(request) => {
                self.ask
                    .resolve(&request.id, alter_zero::ask::AskDecision::Declined);
                return;
            }
            _ => {}
        }
        let started = match &event {
            StreamEvent::ToolStart {
                name,
                args,
                arguments,
                ..
            } => Some((name.clone(), args.clone(), arguments.clone())),
            _ => None,
        };
        let notice = match &event {
            StreamEvent::Retrying { attempt, max } => reporter.retrying(*attempt, *max),
            StreamEvent::Offline { host, attempts } => reporter.offline(host, *attempts),
            _ => Vec::new(),
        };
        let blocked = matches!(event, StreamEvent::PromptBlocked { .. });
        self.on_reply_event(event);
        if blocked {
            // The hook's refusal took the prompt back out of the history and
            // put its notice in: everything before that notice was said.
            *emitted = self.app.history.len().saturating_sub(1).max(run_start);
        }
        self.emit_settled(reporter, run_start, emitted);
        if let Some((name, args, arguments)) = started {
            write(&reporter.tool_started(&name, &args, arguments.as_deref()));
        }
        write(&notice);
    }

    /// Hand the reporter whatever history has settled since the last call —
    /// never what a retried reply attempt could still take back
    /// ([`alter_zero::app::App::settled_history_len`]).
    fn emit_settled(&self, reporter: &mut Reporter, run_start: usize, emitted: &mut usize) {
        let settled = self.app.settled_history_len().max(run_start);
        if settled < *emitted {
            *emitted = settled;
        }
        if settled > *emitted {
            write(&reporter.settled(&self.app.history[*emitted..settled]));
            *emitted = settled;
        }
    }

    /// Whether the run has nothing left to wait for: no turn in flight,
    /// nothing queued, no subagent working (`alter_zero::herdr::agent_busy`:
    /// a subagent always finishes, and its result starts the next turn).
    /// Background shells do not count — a server started with `&` never ends.
    fn headless_idle(&self) -> bool {
        self.inflight.is_none()
            && !self.app.turn_active()
            && self.app.queued.is_empty()
            && !self.waiting_on_agents()
    }

    fn waiting_on_agents(&self) -> bool {
        self.app.agents().iter().any(alter_zero::herdr::agent_busy)
            || self.app.has_pending_agent_notices()
    }

    /// Idle but for subagents still at work — the wait
    /// [`core::agent_wait`] bounds.
    fn idle_on_working_agents(&self) -> bool {
        self.inflight.is_none()
            && !self.app.turn_active()
            && self.app.queued.is_empty()
            && self.app.agents().iter().any(alter_zero::herdr::agent_busy)
    }

    /// The subagents still at work, by their descriptions.
    fn working_agents(&self) -> Vec<String> {
        self.app
            .agents()
            .iter()
            .filter(|run| alter_zero::herdr::agent_busy(run))
            .map(|run| run.description.clone())
            .collect()
    }

    /// Fold every event already waiting on the background, subagent and
    /// reply channels — the last chance for one of them to start a
    /// follow-up turn. Only what is queued now: one that arrives meanwhile
    /// waits for the next call, so a stream of output cannot hold the run.
    fn drain_queued_events(
        &mut self,
        reporter: &mut Reporter,
        run_start: usize,
        emitted: &mut usize,
    ) {
        for _ in 0..self.bg_rx.len() {
            let Ok(event) = self.bg_rx.try_recv() else {
                break;
            };
            self.on_bg_event(event);
        }
        for _ in 0..self.agent_rx.len() {
            let Ok(event) = self.agent_rx.try_recv() else {
                break;
            };
            self.on_headless_agent_message(event);
        }
        for _ in 0..self.reply_rx.len() {
            let Ok(event) = self.reply_rx.try_recv() else {
                break;
            };
            self.on_headless_reply(event, reporter, run_start, emitted);
        }
    }

    /// The loop-bottom work a headless run needs from the TUI's
    /// `after_iteration`: the context gauge an auto-compaction reads, that
    /// compaction itself when `compact` allows it, the rollout, the reaping
    /// — and none of the screen's.
    fn after_headless_iteration(&mut self, compact: bool) {
        self.app
            .set_tool_definitions(self.models.backend().tool_definitions());
        self.app.sync_context_used();
        if compact
            && self.inflight.is_none()
            && !self.models.is_stalled_test_backend()
            && self.app.should_auto_compact()
        {
            self.start_compact_turn(/*auto=*/ true);
        }
        self.recorder
            .sync(&self.app.history, self.app.history_generation());
        self.reaping.retain(|handle| !handle.is_finished());
    }

    /// The run is over: gather its report, shut the session down the TUI's
    /// way — the backend cancelled, every shell and agent stopped,
    /// `SessionEnd` fired, the MCP servers reaped, the rollout flushed — and
    /// write the result. Returns the exit status.
    fn finish(
        mut self,
        reporter: &mut Reporter,
        run_start: usize,
        start: &StartInfo,
        began: Instant,
        interrupted: Option<i32>,
        stopped_agents: &[String],
    ) -> i32 {
        let denials: Vec<Denial> = self
            .permissions
            .gate()
            .map(alter_zero::permission::PermissionGate::take_unattended_refusals)
            .unwrap_or_default()
            .iter()
            .map(Denial::from)
            .collect();
        let stopped: Vec<String> = self
            .registry
            .running()
            .into_iter()
            .map(|shell| shell.command)
            .collect();
        let budget = self.round_budget.clone().unwrap_or_default();
        let run: &[HistoryItem] = self.app.history.get(run_start..).unwrap_or_default();
        let outcome = match interrupted {
            Some(signal) => RunOutcome::Interrupted(signal),
            None => RunOutcome::of(run, budget.spent()),
        };
        let answer = core::final_answer(run);
        let usage = RunUsage::from(self.app.usage_since_launch());
        // A run that never got as far as its prompt has nothing recorded.
        let recorded = run.iter().any(
            |item| matches!(item, HistoryItem::Message(message) if message.role == Role::User),
        ) || start.resumed;
        self.recorder
            .sync(&self.app.history, self.app.history_generation());
        let _ = self.shutdown();
        write(&reporter.finish(&RunEnd {
            outcome: &outcome,
            answer: answer.as_deref(),
            session_id: recorded.then_some(start.session_id.as_str()),
            model: &start.model,
            num_turns: budget.used(),
            duration: began.elapsed(),
            usage,
            denials: &denials,
            stopped_shells: &stopped,
            stopped_agents,
        }));
        outcome.exit_code()
    }
}
