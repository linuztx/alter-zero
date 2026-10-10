//! herdr support — telling the multiplexer pane this session runs in what
//! the agent is doing (`docs/herdr.md`).
//!
//! [herdr](https://herdr.dev) is a terminal multiplexer for coding agents:
//! its sidebar shows every pane's agent as **working**, **blocked** on the
//! user, **done** (finished, not yet looked at) or **idle**, and notifies when
//! one needs attention or finishes. It recognises the agents it ships rules
//! for by their screens; any other agent reports for itself over herdr's
//! local socket, and its reports are then the pane's only authority.
//!
//! - [`pane`] — whether this process runs in a herdr pane at all, from the
//!   variables herdr sets on a pane's processes.
//! - [`activity`] — what the session is doing, read off the [`App`] at every
//!   loop bottom: derived, never tracked as events.
//! - [`Tracker`] — what herdr is told about that: the state (a failed turn
//!   held as blocked), the command that resumes the session, and nothing at
//!   all while nothing changed.
//! - [`Reporter`] — the worker thread that writes the reports: one request at
//!   a time and only the newest, each under a fresh [`next_seq`], re-sent on a
//!   backoff after a failure and on a keepalive after a success, and the
//!   release last — dropping it included.
//! - [`send`] — one request over the socket, every phase bounded.
//!
//! The boundary (`tui::herdr`) reads the environment and the binary's name,
//! holds one [`Tracker`] and one [`Reporter`], and feeds the first into the
//! second.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::agents::AgentRun;
use crate::app::{App, PathDisplay};
use crate::ask::AskQuestion;
use crate::permission::{self, PermissionKind, PermissionRequest};

/// The app's own off switch for a run.
pub const DISABLE_ENV: &str = "ALTER_ZERO_HERDR";

/// `1` on every process herdr starts in a pane — herdr's own marker, which
/// its integrations and its agent skill all require to be exactly `1`.
pub const HERDR_ENV: &str = "HERDR_ENV";

/// The pane's id, set on every process in a herdr pane.
pub const PANE_ID_ENV: &str = "HERDR_PANE_ID";

/// herdr's API socket, set on every process in a herdr pane.
pub const SOCKET_PATH_ENV: &str = "HERDR_SOCKET_PATH";

/// Who is reporting — the integration's label on every request. herdr keeps
/// one report per source; its own sources are `herdr:`-prefixed, so a
/// third-party source must not be.
pub const SOURCE: &str = "alter-zero";

/// Which agent holds the pane — the name herdr shows for it.
pub const AGENT: &str = "alter-zero";

/// The longest blocked message the reports carry, in characters: one line —
/// herdr 0.9.3 stores the message but shows it nowhere yet, and a sidebar row
/// or a notification is about this wide.
pub const MESSAGE_MAX_CHARS: usize = 80;

/// How long one request may take to write, and its reply to arrive. herdr
/// answers a local socket in microseconds; this only bounds a wedged one.
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(500);

/// How long a quit waits for the release to reach herdr before the process
/// exits anyway — a request in flight plus the release itself, each well
/// under this on a live herdr.
pub const RELEASE_WAIT: Duration = Duration::from_millis(400);

/// The first resend after a failed request ([`Timing::resend_after`]).
pub const FIRST_RETRY: Duration = Duration::from_secs(1);

/// How often the current report is sent again while nothing changes
/// ([`Timing::resend_after`]).
pub const KEEPALIVE: Duration = Duration::from_secs(30);

/// The most words herdr takes in a resume command.
pub const RESUME_MAX_ARGS: usize = 64;

/// The most bytes herdr takes across a resume command's words (separators
/// not counted).
pub const RESUME_MAX_BYTES: usize = 8 * 1024;

/// The flag the resume command reopens a recorded session with.
const RESUME_FLAG: &str = "--resume";

/// What a held failure's message opens with ([`Tracker::fail`]).
const FAILURE_PREFIX: &str = "Turn failed: ";

/// What the agent is doing, in herdr's three words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Ready for the user's next message — herdr shows it as done until the
    /// user has looked.
    Idle,
    /// Busy: a turn in flight, or a subagent still at work.
    Working,
    /// Waiting on the user: a permission prompt, a question, or a turn that
    /// failed.
    Blocked,
}

impl State {
    /// The state as herdr's API spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
        }
    }
}

/// The pane this process runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    /// herdr's id for the pane (`w1:p3`), sent verbatim on every request.
    pub id: String,
    /// herdr's API socket.
    pub socket: PathBuf,
}

/// The pane this process runs in, from herdr's environment — `var` is the
/// boundary's environment lookup. `None` outside herdr: unless [`HERDR_ENV`]
/// is `1` — herdr's own rule for its integrations — and the pane id and the
/// socket are both set; and when [`DISABLE_ENV`] turns the integration off
/// for the run.
#[must_use]
pub fn pane(var: impl Fn(&str) -> Option<String>) -> Option<Pane> {
    if var(DISABLE_ENV).is_some_and(|value| falsy(&value)) {
        return None;
    }
    if var(HERDR_ENV)?.trim() != "1" {
        return None;
    }
    let id = var(PANE_ID_ENV)?.trim().to_string();
    let socket = var(SOCKET_PATH_ENV)?;
    if id.is_empty() || socket.trim().is_empty() {
        return None;
    }
    Some(Pane {
        id,
        socket: PathBuf::from(socket),
    })
}

/// The off half of the grammar every `ALTER_ZERO_*` on/off flag uses.
fn falsy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

/// What the session is doing, read off the [`App`] by [`activity`] at the
/// bottom of every loop iteration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    /// A turn is in flight: a reply, a `!` command, a compaction, a
    /// background notice's follow-up.
    pub turn: bool,
    /// A subagent is still at work ([`agent_busy`]).
    pub agents: bool,
    /// What an open modal — a permission prompt or a question, whoever
    /// raised it — asks the user, in one line.
    pub waiting: Option<String>,
    /// Which conversation this is: the history generation, which moves when
    /// one is replaced or rewound (`/clear`, `/resume`, a backtrack;
    /// [`App::history_generation`]). A failed turn's hold lasts no longer
    /// than the conversation it ended.
    pub conversation: u64,
}

/// What `app` is doing, for herdr.
#[must_use]
pub fn activity(app: &App) -> Activity {
    Activity {
        turn: app.turn_active(),
        agents: app.agents().iter().any(agent_busy),
        waiting: waiting_message(app),
        conversation: app.history_generation(),
    }
}

/// What the open modal asks, in one line: a permission prompt's
/// [`permission_message`], a question modal's [`question_message`].
fn waiting_message(app: &App) -> Option<String> {
    if let Some(prompt) = app.permission() {
        return Some(one_line(&permission_message(
            &prompt.request,
            app.path_display(),
        )));
    }
    let prompt = app.ask()?;
    Some(one_line(&question_message(&prompt.request.questions)))
}

/// Is this subagent still at work? Announced or running — or settled with
/// a message queued for it, which starts it again: a steered message its
/// loop takes up, or a Tab follow-up turn the boundary hands over at the
/// next tick. Reporting idle in between would have herdr announce the
/// session finished just before it carries on.
#[must_use]
pub fn agent_busy(run: &AgentRun) -> bool {
    !run.status.is_final() || !run.queued.is_empty() || !run.followups.is_empty()
}

/// A permission prompt in a line: its title over what it names — the file
/// as the screen shows it (`paths`), the command, the server's tool, the
/// program a session runs.
fn permission_message(request: &PermissionRequest, paths: &PathDisplay) -> String {
    let subject = match request.kind {
        PermissionKind::Write | PermissionKind::Edit => paths.display(&request.target),
        PermissionKind::Bash => request.target.clone(),
        PermissionKind::Mcp => permission::mcp_display_label(&request.target),
        PermissionKind::Session => request
            .detail
            .clone()
            .unwrap_or_else(|| request.target.clone()),
    };
    format!("{}: {subject}", permission::title(request.kind))
}

/// A question modal in a line: the first question, and how many follow it.
fn question_message(questions: &[AskQuestion]) -> String {
    let first = questions.first().map_or("", |q| q.question.as_str());
    match questions.len() {
        0 | 1 => format!("Question: {first}"),
        n => format!("Question: {first} (+{} more)", n - 1),
    }
}

/// `text` on one line — every run of whitespace and control characters a
/// single space, so a terminal escape in a command line never reaches
/// herdr's screen — cut to [`MESSAGE_MAX_CHARS`] characters with a closing
/// `…`.
fn one_line(text: &str) -> String {
    let joined = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if joined.chars().count() <= MESSAGE_MAX_CHARS {
        return joined;
    }
    let mut cut: String = joined.chars().take(MESSAGE_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// What herdr is told about the session's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// What a blocked session waits on, in one line; `None` otherwise.
    pub message: Option<String>,
}

/// Would herdr take `name` as a resume command's first word? It types the
/// command into the pane's shell, which finds it on `PATH`: letters, digits,
/// `_`, `.` and `-`, not leading with `-` — never a path.
fn plain_command(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// The command herdr runs to bring this session back after a restart:
/// `{bin} --resume {id}` for a recorded conversation — the exit hint's own
/// command (`docs/cli.md`) — and `{bin}` alone before the first message or
/// after a `/clear`, which comes back as the fresh session it was. `None`
/// when `bin` is not a name herdr would run. herdr's validator is mirrored
/// because a refused command takes the state report it rode in with: no
/// word with an apostrophe or a control character, at most
/// [`RESUME_MAX_ARGS`] words and [`RESUME_MAX_BYTES`] bytes between them —
/// a session id that breaks a rule is left out, offering the fresh launch.
#[must_use]
pub fn resume_argv(bin: &str, session: Option<&str>) -> Option<Vec<String>> {
    if !plain_command(bin) {
        return None;
    }
    let resumable = session.filter(|id| {
        !id.is_empty()
            && !id.chars().any(|c| c == '\'' || c.is_control())
            && bin.len() + RESUME_FLAG.len() + id.len() <= RESUME_MAX_BYTES
    });
    let mut argv = vec![bin.to_string()];
    if let Some(id) = resumable {
        argv.extend([RESUME_FLAG.to_string(), id.to_string()]);
    }
    let bytes: usize = argv.iter().map(String::len).sum();
    (argv.len() <= RESUME_MAX_ARGS && bytes <= RESUME_MAX_BYTES).then_some(argv)
}

/// What one report tells herdr: the state, and the command that brings the
/// session back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub status: Status,
    /// The resume command ([`resume_argv`]); `None` when this binary has no
    /// name herdr could run it by.
    pub resume: Option<Vec<String>>,
}

/// A turn that ended on a backend error, held as the pane's state until the
/// user moves past it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Failure {
    message: String,
    conversation: u64,
}

/// What herdr is told, from each loop bottom's [`Activity`]: the state —
/// **blocked** on an open modal; else **working** while a turn runs; else
/// **blocked** on a turn that failed, until a turn starts again or its
/// conversation leaves the screen; else **working** while a subagent is at
/// work; else **idle** — and the resume command for the recorded session.
/// [`update`](Self::update) answers a report only when one of them changed:
/// herdr wants the current state, and an iteration that changed nothing
/// sends nothing.
#[derive(Debug, Clone, Default)]
pub struct Tracker {
    /// The resume command's first word — this binary's name, when herdr can
    /// run it by that name (`None` offers no resume command at all).
    resume_bin: Option<String>,
    failure: Option<Failure>,
    /// The last state reported, and the session it named.
    last: Option<(Status, Option<String>)>,
}

impl Tracker {
    /// A tracker whose resume commands start with `resume_bin`.
    #[must_use]
    pub fn new(resume_bin: Option<String>) -> Self {
        Self {
            resume_bin,
            ..Self::default()
        }
    }

    /// The turn ended on a backend `error` in `conversation` (the history
    /// generation, [`Activity::conversation`]): report it as blocked — what
    /// herdr flags as needing attention, where idle would announce the work
    /// done — until a turn starts again or that conversation is replaced or
    /// rewound.
    pub fn fail(&mut self, error: &str, conversation: u64) {
        self.failure = Some(Failure {
            message: one_line(&format!("{FAILURE_PREFIX}{error}")),
            conversation,
        });
    }

    /// The report for `activity` in the recorded `session` (the rollout id,
    /// once the conversation has a file), or `None` when herdr already holds
    /// exactly that.
    pub fn update(&mut self, activity: &Activity, session: Option<&str>) -> Option<Report> {
        // A turn starting means the user (or the queue) moved past the
        // failure — it is that turn's to report now — and so does the failed
        // turn leaving the screen with its conversation.
        let moved_on = activity.turn
            || self
                .failure
                .as_ref()
                .is_some_and(|failure| failure.conversation != activity.conversation);
        if moved_on {
            self.failure = None;
        }
        let (state, message) = self.desired(activity);
        // The common path — nothing changed — compares without building the
        // report: this runs at every loop iteration, every streamed chunk
        // and keystroke included.
        let held = self.last.as_ref().is_some_and(|(status, last_session)| {
            status.state == state
                && status.message.as_deref() == message
                && last_session.as_deref() == session
        });
        if held {
            return None;
        }
        let status = Status {
            state,
            message: message.map(str::to_string),
        };
        let resume = self
            .resume_bin
            .as_deref()
            .and_then(|bin| resume_argv(bin, session));
        self.last = Some((status.clone(), session.map(str::to_string)));
        Some(Report { status, resume })
    }

    /// The state `activity` puts the pane in, and the message that explains
    /// a block: a modal, then a turn, then a held failure, then a subagent.
    fn desired<'a>(&'a self, activity: &'a Activity) -> (State, Option<&'a str>) {
        if let Some(waiting) = &activity.waiting {
            (State::Blocked, Some(waiting))
        } else if activity.turn {
            (State::Working, None)
        } else if let Some(failure) = &self.failure {
            (State::Blocked, Some(&failure.message))
        } else if activity.agents {
            (State::Working, None)
        } else {
            (State::Idle, None)
        }
    }
}

/// The sequence number for the next request: `now` — the clock in
/// microseconds, the unit herdr's own plugin reporters count in — or one
/// past `last` when the clock has not moved on.
///
/// herdr drops a request whose `seq` is not above the highest it has taken
/// from this source for this pane, and keeps that mark for the pane's whole
/// life. So every request must outrank the one before — two in the same
/// microsecond, a clock stepped back — and a relaunch in the same pane must
/// outrank the process before it, which a counter starting at 1 never would.
/// The clock gives the second; the `+ 1` the first. (The mark is kept per
/// source, so only this source's own unit matters — and it must never move
/// to a coarser one: the old mark would outrank every new request.)
#[must_use]
pub const fn next_seq(last: u64, now: u64) -> u64 {
    let after = last.saturating_add(1);
    if now > after { now } else { after }
}

/// How the [`Reporter`]'s worker paces itself — [`Timing::DEFAULT`] in the
/// app; a test shortens it to watch every resend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// How long one request may take to write, and its reply to arrive.
    pub request_timeout: Duration,
    /// The wait before the first resend of a failed request; each further
    /// failure in a row doubles it.
    pub first_retry: Duration,
    /// How often the current report is sent again while nothing changes —
    /// and the backoff's ceiling.
    pub keepalive: Duration,
}

impl Timing {
    /// The app's pacing: [`REQUEST_TIMEOUT`], [`FIRST_RETRY`], [`KEEPALIVE`].
    pub const DEFAULT: Self = Self {
        request_timeout: REQUEST_TIMEOUT,
        first_retry: FIRST_RETRY,
        keepalive: KEEPALIVE,
    };

    /// How long the worker waits before sending the current report again,
    /// after `failures` failed sends in a row: the first retry, doubling, up
    /// to the keepalive — which is also the wait with none, since herdr holds
    /// a self-reported state with no expiry, and a report it lost (a live
    /// upgrade drops a third-party agent's state; a timeout drops one
    /// request) would otherwise stand wrong until the next change. A resend
    /// of an unchanged state notifies nobody.
    #[must_use]
    pub fn resend_after(&self, failures: u32) -> Duration {
        if failures == 0 {
            return self.keepalive;
        }
        let doubling = 1u32 << failures.saturating_sub(1).min(16);
        self.first_retry
            .saturating_mul(doubling)
            .min(self.keepalive)
    }
}

/// One request on the wire: `{"id", "method", "params"}`, the id a string
/// herdr echoes back.
#[derive(Serialize)]
struct Request<'a, P> {
    id: String,
    method: &'a str,
    params: P,
}

/// `pane.report_agent`'s params — the keys this agent knows, every other
/// one left out (herdr reads an absent key and a `null` alike, and an absent
/// one is what an older herdr has never heard of). No `agent_session_id`:
/// herdr keeps one only from its own integrations.
#[derive(Serialize)]
struct ReportParams<'a> {
    pane_id: &'a str,
    source: &'a str,
    agent: &'a str,
    state: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
    seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    resume_argv: Option<&'a [String]>,
}

/// `pane.release_agent`'s params.
#[derive(Serialize)]
struct ReleaseParams<'a> {
    pane_id: &'a str,
    source: &'a str,
    agent: &'a str,
    seq: u64,
}

/// A request as the compact one-line JSON herdr reads up to its newline —
/// the id the seq, which already never repeats.
fn request_line<P: Serialize>(method: &str, seq: u64, params: P) -> String {
    serde_json::to_string(&Request {
        id: format!("{SOURCE}:{seq}"),
        method,
        params,
    })
    .unwrap_or_default()
}

/// The `pane.report_agent` request for `report`: this agent holds the pane
/// and is in its state, with the blocked message and the resume command
/// when there are any. One request carries the whole picture — herdr
/// applies the state before it checks the resume command, so even the first
/// report can name one — and `seq` must be above every report this pane has
/// taken from [`SOURCE`] (`next_seq`).
#[must_use]
pub fn report_request(pane: &Pane, report: &Report, seq: u64) -> String {
    request_line(
        "pane.report_agent",
        seq,
        ReportParams {
            pane_id: &pane.id,
            source: SOURCE,
            agent: AGENT,
            state: report.status.state.as_str(),
            message: report.status.message.as_deref(),
            seq,
            resume_argv: report.resume.as_deref(),
        },
    )
}

/// The `pane.release_agent` request: hand the pane back to herdr, which
/// forgets this agent and its resume command and reads the screen again.
/// Sent once, when the session ends — never on `/clear` or `/resume`, where
/// the next report names the new session instead.
#[must_use]
pub fn release_request(pane: &Pane, seq: u64) -> String {
    request_line(
        "pane.release_agent",
        seq,
        ReleaseParams {
            pane_id: &pane.id,
            source: SOURCE,
            agent: AGENT,
            seq,
        },
    )
}

/// herdr's reply: a `result`, or an `error` with its code and message.
#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    result: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    error: Option<ReplyError>,
}

#[derive(Deserialize)]
struct ReplyError {
    code: String,
    message: String,
}

/// What herdr's one-line reply says: `Ok` for a `result`, the refusal's
/// `{code}: {message}` for an `error`, and a failure for no reply at all —
/// herdr hangs up without a word on a request it could not read. An `ok` is
/// not proof the report was applied (a stale seq is answered `ok` too), but
/// a refusal is proof it was not.
///
/// # Errors
/// The refusal, or why the reply could not be read.
pub fn reply_outcome(reply: &[u8]) -> Result<(), String> {
    let reply: Reply = serde_json::from_slice(reply)
        .map_err(|error| format!("unreadable reply from herdr: {error}"))?;
    match (reply.error, reply.result) {
        (Some(error), _) => Err(format!("{}: {}", error.code, error.message)),
        (None, Some(_)) => Ok(()),
        (None, None) => Err("herdr replied with neither a result nor an error".to_string()),
    }
}

/// The most of herdr's reply read back — its replies are one short line.
const REPLY_MAX_BYTES: u64 = 64 * 1024;

/// May a request be written to this socket? Only to a socket, and only to
/// one this process's own user owns — herdr's socket is its owner's alone,
/// and a report can carry the command a prompt asks about.
#[must_use]
pub const fn socket_acceptable(is_socket: bool, owner: u32, me: u32) -> bool {
    is_socket && owner == me
}

/// Write one request to herdr's socket and read its one-line reply: one
/// connection per request, herdr's own model, with the write and the read
/// each bounded by `timeout` (herdr sets no deadline of its own on a
/// report). The path must name a socket this user owns
/// ([`socket_acceptable`]) — its own entry, a symlink never followed, since
/// what is checked must be what is connected to. The worker thread calls
/// it, never the event loop.
///
/// # Errors
/// The socket is refused (`PermissionDenied`), missing or unreachable, the
/// write or read fails, or herdr refuses the request ([`reply_outcome`]).
/// Off Unix, always — herdr's socket there is a named pipe this integration
/// does not speak.
pub fn send(socket: &Path, request: &str, timeout: Duration) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::{BufRead, Read, Write};
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let entry = std::fs::symlink_metadata(socket)?;
        let me = rustix::process::geteuid().as_raw();
        if !socket_acceptable(entry.file_type().is_socket(), entry.uid(), me) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "not a socket this user owns",
            ));
        }
        let mut stream = std::os::unix::net::UnixStream::connect(socket)?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_read_timeout(Some(timeout))?;
        let mut line = Vec::with_capacity(request.len() + 1);
        line.extend_from_slice(request.as_bytes());
        line.push(b'\n');
        stream.write_all(&line)?;
        let mut reply = Vec::new();
        io::BufReader::new(stream.take(REPLY_MAX_BYTES)).read_until(b'\n', &mut reply)?;
        reply_outcome(&reply).map_err(io::Error::other)
    }
    #[cfg(not(unix))]
    {
        let _ = (socket, request, timeout);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "herdr support needs a Unix socket",
        ))
    }
}

/// The worker's single slot: the newest report not yet sent, and the release
/// once it is queued. A report never queues behind another — the one the
/// worker has not got to is replaced — and nothing is taken after the
/// release.
#[derive(Debug, Default)]
struct Outbox {
    report: Option<Report>,
    release: bool,
    released: bool,
}

/// One unit of the worker's work.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Job {
    /// Send this report.
    Report(Report),
    /// Send the release, then stop.
    Release,
}

impl Outbox {
    /// Queue a report, replacing any the worker has not sent yet — herdr
    /// wants the current state, not its history. Ignored once the release is
    /// queued: a report after it would claim the pane back for a process on
    /// its way out.
    fn post(&mut self, report: Report) {
        if !self.released {
            self.report = Some(report);
        }
    }

    /// Queue the release, dropping any unsent report: the last job, queued
    /// once.
    fn release(&mut self) {
        if !self.released {
            self.released = true;
            self.release = true;
            self.report = None;
        }
    }

    /// The next job, if any: the release once queued, else the newest report.
    fn take(&mut self) -> Option<Job> {
        if std::mem::take(&mut self.release) {
            return Some(Job::Release);
        }
        self.report.take().map(Job::Report)
    }
}

/// The worker that writes the reports, so the event loop never touches the
/// socket: [`post`](Self::post) hands it a report without blocking, and it
/// writes them one at a time, in order, each under a fresh [`next_seq`] —
/// one writer, so every request outranks the one before.
///
/// - **Only the newest report matters.** It takes from a single slot: a
///   burst of changes while herdr is slow collapses to the last one rather
///   than queuing behind it.
/// - **Every failure is silent, bounded and repaired.** A request is one
///   connection, one line out and one line back, under the
///   [`Timing::request_timeout`]; a failed one is sent again on the
///   [`Timing::resend_after`] backoff, and a delivered one on the keepalive,
///   since herdr holds a self-reported state with no expiry.
/// - **The release is last.** [`release`](Self::release) replaces any report
///   not yet written, nothing is written after it, and a quit waits a
///   bounded time for it to land — and dropping the reporter releases too, so
///   a loop that bails out on an error still hands the pane back.
#[derive(Debug)]
pub struct Reporter {
    mailbox: Arc<Mailbox>,
    /// Signalled by the worker once it is done — the release written, or the
    /// mailbox gone; taken by the first [`release`](Self::release).
    done: Option<mpsc::Receiver<()>>,
}

/// The [`Outbox`] the worker drains, behind its lock and wake-up.
#[derive(Debug, Default)]
struct Mailbox {
    outbox: Mutex<Outbox>,
    ready: Condvar,
}

impl Reporter {
    /// The reporter for `pane`, its worker started at the app's
    /// [`Timing::DEFAULT`]. `None` when the thread cannot be started.
    #[must_use]
    pub fn spawn(pane: Pane) -> Option<Self> {
        Self::spawn_with(pane, Timing::DEFAULT)
    }

    /// [`spawn`](Self::spawn) at `timing`.
    #[must_use]
    pub fn spawn_with(pane: Pane, timing: Timing) -> Option<Self> {
        let mailbox = Arc::new(Mailbox::default());
        let (done_tx, done) = mpsc::channel();
        let worker = Arc::clone(&mailbox);
        std::thread::Builder::new()
            .name("herdr".to_string())
            .spawn(move || {
                run_worker(&pane, &worker, timing);
                let _ = done_tx.send(());
            })
            .ok()?;
        Some(Self {
            mailbox,
            done: Some(done),
        })
    }

    /// Hand the worker `report` — never blocks — replacing any it has not
    /// written yet. Ignored after the release.
    pub fn post(&self, report: Report) {
        if let Ok(mut outbox) = self.mailbox.outbox.lock() {
            outbox.post(report);
        }
        self.mailbox.ready.notify_one();
    }

    /// Hand the pane back to herdr, waiting at most `wait` for the worker to
    /// write it: the release replaces any report the worker has not sent,
    /// and nothing is reported after it. Only the first call does anything.
    pub fn release(&mut self, wait: Duration) {
        let Some(done) = self.done.take() else {
            return;
        };
        if let Ok(mut outbox) = self.mailbox.outbox.lock() {
            outbox.release();
        }
        self.mailbox.ready.notify_one();
        let _ = done.recv_timeout(wait);
    }
}

impl Drop for Reporter {
    fn drop(&mut self) {
        self.release(RELEASE_WAIT);
    }
}

/// The worker: take the next job and write it — or, when none comes before
/// the resend is due, write the current report again — until the release.
fn run_worker(pane: &Pane, mailbox: &Mailbox, timing: Timing) {
    let mut seq = 0u64;
    let mut current: Option<Report> = None;
    let mut failures = 0u32;
    let mut due = Instant::now() + timing.resend_after(0);
    loop {
        let report = match next_work(mailbox, due, current.is_some()) {
            None => return,
            Some(Work::Post(Job::Release)) => {
                seq = next_seq(seq, unix_micros());
                let request = release_request(pane, seq);
                let _ = send(&pane.socket, &request, timing.request_timeout);
                return;
            }
            Some(Work::Post(Job::Report(report))) => report,
            Some(Work::Resend) => match current.take() {
                Some(report) => report,
                None => continue,
            },
        };
        seq = next_seq(seq, unix_micros());
        let request = report_request(pane, &report, seq);
        // A refused or timed-out report is retried on the backoff; a
        // delivered one rests until the keepalive.
        failures = if send(&pane.socket, &request, timing.request_timeout).is_ok() {
            0
        } else {
            failures.saturating_add(1)
        };
        current = Some(report);
        due = Instant::now() + timing.resend_after(failures);
    }
}

/// What the worker does next.
enum Work {
    /// Write what the loop posted.
    Post(Job),
    /// Write the current report again: the resend came due with nothing new.
    Resend,
}

/// Block until there is work: a posted job, or — once `due` passes with a
/// report to repeat — a resend. `None` when the mailbox is poisoned (the loop
/// thread panicked holding it; there is no one left to report for).
fn next_work(mailbox: &Mailbox, due: Instant, resendable: bool) -> Option<Work> {
    let mut outbox = mailbox.outbox.lock().ok()?;
    loop {
        if let Some(job) = outbox.take() {
            return Some(Work::Post(job));
        }
        let now = Instant::now();
        if resendable && now >= due {
            return Some(Work::Resend);
        }
        outbox = if resendable {
            mailbox.ready.wait_timeout(outbox, due - now).ok()?.0
        } else {
            mailbox.ready.wait(outbox).ok()?
        };
    }
}

/// Microseconds since the Unix epoch — the clock [`next_seq`] follows, so a
/// relaunch in the same pane outranks the process before it.
fn unix_micros() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::agents::AgentStatus;
    use crate::ask::{AskOption, AskQuestion, AskRequest};
    use crate::permission::{PermissionKind, PermissionRequest};
    use crate::stream::{AgentSpec, StreamEvent};

    /// An environment lookup over string pairs.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    /// The three variables herdr sets on a pane's processes.
    const IN_PANE: [(&str, &str); 3] = [
        (HERDR_ENV, "1"),
        (PANE_ID_ENV, "w1:p3"),
        (SOCKET_PATH_ENV, "/run/user/1000/herdr/herdr.sock"),
    ];

    fn permission(kind: PermissionKind, target: &str) -> PermissionRequest {
        PermissionRequest {
            id: "p1".to_string(),
            kind,
            target: target.to_string(),
            body: String::new(),
            detail: None,
            agent: None,
            agent_id: None,
        }
    }

    fn ask(questions: &[&str]) -> AskRequest {
        AskRequest {
            id: "q1".to_string(),
            questions: questions
                .iter()
                .map(|question| AskQuestion {
                    question: (*question).to_string(),
                    header: "Pick".to_string(),
                    options: vec![
                        AskOption {
                            label: "A".to_string(),
                            description: String::new(),
                            preview: None,
                        },
                        AskOption {
                            label: "B".to_string(),
                            description: String::new(),
                            preview: None,
                        },
                    ],
                    multi_select: false,
                })
                .collect(),
        }
    }

    fn agent(id: &str, background: bool) -> AgentSpec {
        AgentSpec {
            id: id.to_string(),
            description: "Survey the tests".to_string(),
            agent_type: "explore".to_string(),
            prompt: "Look around.".to_string(),
            background,
            call_id: None,
            arguments: None,
        }
    }

    /// What the open modal of `app` asks, as the report would carry it.
    fn waiting(app: &App) -> Option<String> {
        activity(app).waiting
    }

    // ===== the states' wire spelling =====

    #[test]
    fn states_are_herdrs_three_words() {
        assert_eq!(State::Idle.as_str(), "idle");
        assert_eq!(State::Working.as_str(), "working");
        assert_eq!(State::Blocked.as_str(), "blocked");
    }

    #[test]
    fn the_source_passes_herdrs_strictest_source_rule() {
        // herdr validates a metadata source as at most 80 ASCII letters,
        // digits, `:`, `.`, `_` and `-`, and keeps the `herdr:` prefix for
        // its own integrations.
        assert!(SOURCE.len() <= 80);
        assert!(
            SOURCE
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b":._-".contains(&b))
        );
        assert!(!SOURCE.starts_with("herdr:"));
    }

    // ===== finding the pane =====

    #[test]
    fn a_herdr_pane_is_found_from_its_three_variables() {
        assert_eq!(
            pane(env(&IN_PANE)),
            Some(Pane {
                id: "w1:p3".to_string(),
                socket: PathBuf::from("/run/user/1000/herdr/herdr.sock"),
            })
        );
    }

    #[test]
    fn outside_herdr_there_is_no_pane() {
        assert_eq!(pane(env(&[])), None);
        // A leftover pane id or socket without the flag is not a pane: the
        // flag is what herdr sets on the processes it starts.
        assert_eq!(pane(env(&IN_PANE[1..])), None);
        assert_eq!(pane(env(&[IN_PANE[0], IN_PANE[2]])), None, "no pane id");
        assert_eq!(pane(env(&[IN_PANE[0], IN_PANE[1]])), None, "no socket");
    }

    #[test]
    fn herdr_env_must_say_exactly_one() {
        // herdr sets `1`, and its integrations report only on `1`: any other
        // value is not herdr's, however truthy it reads.
        for flag in [
            "0", "false", "off", "", "  ", "true", "yes", "on", "11", "2",
        ] {
            let mut vars = IN_PANE.to_vec();
            vars[0] = (HERDR_ENV, flag);
            assert_eq!(pane(env(&vars)), None, "HERDR_ENV={flag:?}");
        }
        let mut vars = IN_PANE.to_vec();
        vars[0] = (HERDR_ENV, " 1 ");
        assert!(pane(env(&vars)).is_some(), "padding is not part of it");
    }

    #[test]
    fn a_blank_pane_id_or_socket_is_no_pane() {
        let mut vars = IN_PANE.to_vec();
        vars[1] = (PANE_ID_ENV, "  ");
        assert_eq!(pane(env(&vars)), None, "a blank pane id");
        let mut vars = IN_PANE.to_vec();
        vars[2] = (SOCKET_PATH_ENV, "");
        assert_eq!(pane(env(&vars)), None, "a blank socket path");
    }

    #[test]
    fn the_off_switch_hides_the_pane() {
        for off in ["0", "false", "no", "off", " OFF "] {
            let mut vars = IN_PANE.to_vec();
            vars.push((DISABLE_ENV, off));
            assert_eq!(pane(env(&vars)), None, "{DISABLE_ENV}={off:?}");
        }
        // Anything else leaves it on — the grammar every ALTER_ZERO_* flag
        // uses.
        let mut vars = IN_PANE.to_vec();
        vars.push((DISABLE_ENV, "1"));
        assert!(pane(env(&vars)).is_some());
    }

    #[test]
    fn the_pane_id_is_trimmed_but_kept_verbatim_otherwise() {
        let mut vars = IN_PANE.to_vec();
        vars[1] = (PANE_ID_ENV, " w2:p10 ");
        assert_eq!(pane(env(&vars)).map(|pane| pane.id), Some("w2:p10".into()));
    }

    // ===== what the session is doing =====

    #[test]
    fn a_fresh_session_is_doing_nothing() {
        assert_eq!(activity(&App::new()), Activity::default());
    }

    #[test]
    fn every_kind_of_turn_is_a_turn() {
        let mut reply = App::new();
        reply.begin_stream();
        assert!(activity(&reply).turn, "a reply");
        let mut shell = App::new();
        shell.begin_shell("cargo test");
        assert!(activity(&shell).turn, "a `!` command");
        let mut compact = App::new();
        compact.begin_compact(false);
        assert!(activity(&compact).turn, "a compaction");
    }

    #[test]
    fn a_turn_that_ended_is_no_longer_a_turn() {
        let mut done = App::new();
        done.begin_stream();
        done.end_turn(3);
        assert!(!activity(&done).turn);
        let mut failed = App::new();
        failed.begin_stream();
        failed.fail_stream("boom");
        assert!(!activity(&failed).turn);
    }

    #[test]
    fn a_permission_prompt_is_what_the_session_waits_on() {
        let mut app = App::new();
        app.begin_stream();
        app.open_permission(permission(PermissionKind::Bash, "cargo test --all"));
        assert_eq!(
            waiting(&app).as_deref(),
            Some("Bash command: cargo test --all")
        );
    }

    #[test]
    fn a_file_prompt_names_the_file_as_the_screen_shows_it() {
        // The prompt's own target row: relative under the cwd, `~`-relative
        // under home — the path display rule (`docs/tools.md`).
        let paths = PathDisplay::new("/home/u/repo", Some(PathBuf::from("/home/u")));
        let mut app = App::new();
        app.set_path_display(paths.clone());
        app.open_permission(permission(PermissionKind::Write, "/home/u/repo/src/new.rs"));
        assert_eq!(waiting(&app).as_deref(), Some("Create file: src/new.rs"));
        let mut app = App::new();
        app.set_path_display(paths);
        app.open_permission(permission(PermissionKind::Edit, "/home/u/notes.md"));
        assert_eq!(waiting(&app).as_deref(), Some("Edit file: ~/notes.md"));
        // With no rule injected, the path the model sent.
        let mut app = App::new();
        app.open_permission(permission(PermissionKind::Edit, "/repo/src/main.rs"));
        assert_eq!(
            waiting(&app).as_deref(),
            Some("Edit file: /repo/src/main.rs")
        );
    }

    #[test]
    fn an_mcp_prompt_names_the_server_and_tool() {
        let mut app = App::new();
        app.open_permission(permission(
            PermissionKind::Mcp,
            "mcp__deepwiki__ask_question",
        ));
        assert_eq!(
            waiting(&app).as_deref(),
            Some("Tool use: Deepwiki - ask_question")
        );
    }

    #[test]
    fn a_session_prompt_names_the_program_it_types_into() {
        let mut app = App::new();
        let mut request = permission(PermissionKind::Session, "s1");
        request.detail = Some("python3".to_string());
        app.open_permission(request);
        assert_eq!(waiting(&app).as_deref(), Some("Session input: python3"));
        // With no command known, the session id is all there is to name.
        let mut app = App::new();
        app.open_permission(permission(PermissionKind::Session, "s1"));
        assert_eq!(waiting(&app).as_deref(), Some("Session input: s1"));
    }

    #[test]
    fn a_question_is_what_the_session_waits_on() {
        let mut app = App::new();
        app.begin_stream();
        app.open_ask(ask(&["Which database should the cache use?"]));
        assert_eq!(
            waiting(&app).as_deref(),
            Some("Question: Which database should the cache use?")
        );
        let mut app = App::new();
        app.open_ask(ask(&["First?", "Second?", "Third?"]));
        assert_eq!(waiting(&app).as_deref(), Some("Question: First? (+2 more)"));
    }

    #[test]
    fn a_waiting_message_is_one_short_line() {
        let message = |command: &str| {
            let mut app = App::new();
            app.open_permission(permission(PermissionKind::Bash, command));
            waiting(&app).expect("a message")
        };
        assert_eq!(
            message("for f in *.rs; do\n  rustfmt \"$f\"\ndone"),
            "Bash command: for f in *.rs; do rustfmt \"$f\" done"
        );
        // A control character is a separator, never sent: a terminal escape
        // in a command line must not reach herdr's screen.
        assert_eq!(
            message("printf '\u{1b}[31mred'\u{7}"),
            "Bash command: printf ' [31mred'"
        );
        let prefix = "Bash command: ".chars().count();
        let exact = "x".repeat(MESSAGE_MAX_CHARS - prefix);
        assert_eq!(message(&exact).chars().count(), MESSAGE_MAX_CHARS, "uncut");
        let cut = message(&"x".repeat(500));
        assert_eq!(cut.chars().count(), MESSAGE_MAX_CHARS);
        assert!(cut.ends_with('…'), "{cut}");
        // Counted in characters, never bytes: a cut can't split one.
        let wide = message(&"é".repeat(500));
        assert_eq!(wide.chars().count(), MESSAGE_MAX_CHARS);
        assert_eq!(MESSAGE_MAX_CHARS, 80, "a sidebar row, not a paragraph");
    }

    #[test]
    fn a_subagent_at_work_is_work_in_flight() {
        // The lead's turn ended, but the agents it launched are still at
        // it — and their results will start a follow-up turn on their own.
        let mut app = App::new();
        app.start_agent_group(true, &[agent("a1", true), agent("a2", true)]);
        assert!(activity(&app).agents);
        app.apply_agent_event("a1", &StreamEvent::StreamDone);
        assert!(activity(&app).agents, "a2 still runs");
        app.apply_agent_event("a2", &StreamEvent::Error("boom".to_string()));
        assert!(!activity(&app).agents);
    }

    #[test]
    fn an_agent_is_busy_until_it_settles_with_nothing_queued() {
        let mut run = AgentRun::new("a1", "Survey", "general-purpose", "Look around", true);
        assert!(agent_busy(&run), "announced, about to run");
        run.status = AgentStatus::Running;
        assert!(agent_busy(&run));
        for settled in [
            AgentStatus::Done,
            AgentStatus::Failed,
            AgentStatus::Interrupted,
        ] {
            run.status = settled;
            assert!(!agent_busy(&run), "{settled:?}");
        }
        run.status = AgentStatus::Done;
        run.queue_followup("and now the docs");
        assert!(agent_busy(&run), "a follow-up turn starts it again");
        run.followups.clear();
        run.queue_chat("one more thing");
        assert!(agent_busy(&run), "so does a message its loop takes up");
    }

    #[test]
    fn a_settled_agent_with_a_follow_up_keeps_the_session_working() {
        // The boundary hands a Tab follow-up over at the next tick after the
        // settle: idle in between would read as "finished" to herdr.
        let mut app = App::new();
        app.start_agent_group(true, &[agent("a1", true)]);
        app.open_agent_view("a1");
        app.queue_agent_followup("and now the docs");
        app.apply_agent_event("a1", &StreamEvent::StreamDone);
        assert!(activity(&app).agents);
        assert_eq!(
            app.take_agent_followup("a1").as_deref(),
            Some("and now the docs")
        );
        assert!(!activity(&app).agents);
    }

    #[test]
    fn a_subagents_prompt_is_waited_on_with_no_turn_running() {
        let mut app = App::new();
        app.start_agent_group(true, &[agent("a1", true)]);
        let mut request = permission(PermissionKind::Bash, "ls -la");
        request.agent = Some("explore".to_string());
        request.agent_id = Some("a1".to_string());
        app.open_permission(request);
        assert_eq!(waiting(&app).as_deref(), Some("Bash command: ls -la"));
    }

    #[test]
    fn the_activity_names_the_conversation() {
        let mut app = App::new();
        let before = activity(&app).conversation;
        app.begin_stream();
        app.fail_stream("boom");
        assert_eq!(activity(&app).conversation, before, "a failure appends");
        app.load_session(Vec::new());
        assert_ne!(activity(&app).conversation, before, "/resume replaces");
    }

    // ===== the resume command =====

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    #[test]
    fn the_resume_command_reopens_the_recorded_session() {
        assert_eq!(
            resume_argv("alter-zero", Some("18a9f2c33d41e5b6-1a2b")),
            Some(words(&["alter-zero", "--resume", "18a9f2c33d41e5b6-1a2b"]))
        );
        // A renamed install keeps the name it was run as.
        assert_eq!(
            resume_argv("az", Some("abc")),
            Some(words(&["az", "--resume", "abc"]))
        );
    }

    #[test]
    fn a_fresh_session_comes_back_as_a_fresh_launch() {
        // Before the first message — and after a `/clear` — there is nothing
        // to resume, and leaving the last command in herdr's hands would
        // bring back the conversation the user just cleared.
        assert_eq!(
            resume_argv("alter-zero", None),
            Some(words(&["alter-zero"]))
        );
    }

    #[test]
    fn the_resume_command_starts_with_a_plain_command_name() {
        // herdr's own rule for the first word: letters, digits, `_`, `.` and
        // `-`, not leading with `-` — a path, a blank, whitespace or any
        // other character is refused (and the refusal takes the state
        // report it rode in with).
        for bin in [
            "target/debug/alter-zero",
            "/usr/local/bin/alter-zero",
            "",
            "alter zero",
            "-alter-zero",
            "alter+zero",
            "altér-zero",
        ] {
            assert_eq!(resume_argv(bin, Some("abc")), None, "{bin:?}");
            assert_eq!(resume_argv(bin, None), None, "{bin:?}");
        }
        assert!(resume_argv("alter_zero.2", Some("abc")).is_some());
    }

    #[test]
    fn a_session_id_herdr_would_refuse_is_left_out() {
        for id in ["", "it's", "a\nb", "a\tb", "a\u{7f}b"] {
            assert_eq!(
                resume_argv("alter-zero", Some(id)),
                Some(words(&["alter-zero"])),
                "{id:?}"
            );
        }
        let long = "a".repeat(RESUME_MAX_BYTES);
        assert_eq!(
            resume_argv("alter-zero", Some(&long)),
            Some(words(&["alter-zero"])),
            "over 8 KiB"
        );
    }

    // ===== what herdr is told =====

    /// A wall clock, in microseconds — any value; a tracker has no clock.
    fn tracker() -> Tracker {
        Tracker::new(Some("alter-zero".to_string()))
    }

    fn idle() -> Activity {
        Activity::default()
    }

    fn turn() -> Activity {
        Activity {
            turn: true,
            ..Activity::default()
        }
    }

    fn modal(message: &str) -> Activity {
        Activity {
            waiting: Some(message.to_string()),
            ..Activity::default()
        }
    }

    fn agents() -> Activity {
        Activity {
            agents: true,
            ..Activity::default()
        }
    }

    fn status(state: State, message: Option<&str>) -> Status {
        Status {
            state,
            message: message.map(str::to_string),
        }
    }

    /// What `update` reported, as a state and its message.
    fn told(report: Option<Report>) -> (State, Option<String>) {
        let report = report.expect("a report");
        (report.status.state, report.status.message)
    }

    #[test]
    fn the_first_update_claims_the_pane() {
        assert_eq!(
            tracker().update(&idle(), None),
            Some(Report {
                status: status(State::Idle, None),
                resume: Some(words(&["alter-zero"])),
            })
        );
    }

    #[test]
    fn nothing_is_reported_while_nothing_changes() {
        let mut tracker = tracker();
        assert!(tracker.update(&idle(), None).is_some());
        assert_eq!(tracker.update(&idle(), None), None);
        assert_eq!(tracker.update(&idle(), None), None);
    }

    #[test]
    fn a_turn_is_working_and_its_end_is_idle() {
        let mut tracker = tracker();
        tracker.update(&idle(), None);
        assert_eq!(told(tracker.update(&turn(), None)), (State::Working, None));
        assert_eq!(tracker.update(&turn(), None), None, "still working");
        assert_eq!(told(tracker.update(&idle(), None)), (State::Idle, None));
    }

    #[test]
    fn a_modal_is_blocked_and_says_what_it_asks() {
        let mut tracker = tracker();
        assert_eq!(
            told(tracker.update(&modal("Bash command: cargo publish"), None)),
            (
                State::Blocked,
                Some("Bash command: cargo publish".to_string())
            )
        );
        // The next prompt in a batch is news even in the same state.
        assert_eq!(
            told(tracker.update(&modal("Edit file: main.rs"), None)),
            (State::Blocked, Some("Edit file: main.rs".to_string()))
        );
        // Answered: back to the turn, no message.
        assert_eq!(told(tracker.update(&turn(), None)), (State::Working, None));
    }

    #[test]
    fn a_running_subagent_keeps_the_pane_working() {
        let mut tracker = tracker();
        tracker.update(&turn(), None);
        assert_eq!(
            tracker.update(&agents(), None),
            None,
            "the lead's turn ended but a subagent still works: no change"
        );
        assert_eq!(told(tracker.update(&idle(), None)), (State::Idle, None));
    }

    #[test]
    fn a_failed_turn_holds_blocked_until_the_next_turn_starts() {
        // Idle would tell herdr the work is done — its "finished" toast —
        // when the turn died on a rate limit with the work half-done.
        let mut tracker = tracker();
        tracker.update(&turn(), None);
        tracker.fail("HTTP 429: rate limited\n  try again later", 0);
        assert_eq!(
            told(tracker.update(&idle(), None)),
            (
                State::Blocked,
                Some("Turn failed: HTTP 429: rate limited try again later".to_string())
            )
        );
        assert_eq!(tracker.update(&idle(), None), None, "held, not resent");
        assert_eq!(told(tracker.update(&turn(), None)), (State::Working, None));
        assert_eq!(
            told(tracker.update(&idle(), None)),
            (State::Idle, None),
            "the failure went with the turn that moved past it"
        );
    }

    #[test]
    fn a_failure_goes_with_the_conversation_it_ended() {
        // `/clear`, `/resume` and a backtrack each replace or rewind the
        // conversation (a new history generation): the failed turn is no
        // longer on screen, so neither is the block.
        let mut tracker = tracker();
        tracker.fail("boom", 3);
        let same = Activity {
            conversation: 3,
            ..idle()
        };
        assert_eq!(told(tracker.update(&same, None)).0, State::Blocked);
        let replaced = Activity {
            conversation: 4,
            ..idle()
        };
        assert_eq!(told(tracker.update(&replaced, None)).0, State::Idle);
        assert_eq!(
            tracker.update(&same, None),
            None,
            "gone for good, not hidden while the generation differs"
        );
    }

    #[test]
    fn the_states_rank_modal_turn_failure_agents_idle() {
        let mut tracker = tracker();
        tracker.fail("boom", 0);
        let everything = Activity {
            turn: true,
            agents: true,
            waiting: Some("Question: which?".to_string()),
            conversation: 0,
        };
        assert_eq!(
            told(tracker.update(&everything, None)),
            (State::Blocked, Some("Question: which?".to_string())),
            "a modal outranks everything"
        );
        // A modal open over a failed turn's hold names the modal…
        tracker.fail("boom", 0);
        assert_eq!(
            told(tracker.update(&modal("Edit file: a.rs"), None)),
            (State::Blocked, Some("Edit file: a.rs".to_string()))
        );
        // …and once answered, the failure outranks a subagent still at work.
        assert_eq!(
            told(tracker.update(&agents(), None)),
            (State::Blocked, Some("Turn failed: boom".to_string()))
        );
    }

    #[test]
    fn the_resume_command_follows_the_conversation() {
        let resume = |report: Option<Report>| report.expect("a report").resume;
        let mut tracker = tracker();
        assert_eq!(
            resume(tracker.update(&idle(), None)),
            Some(words(&["alter-zero"]))
        );
        assert_eq!(
            resume(tracker.update(&idle(), Some("18a9f2c3-1a2b"))),
            Some(words(&["alter-zero", "--resume", "18a9f2c3-1a2b"])),
            "the first message gave the conversation a file: news"
        );
        assert_eq!(tracker.update(&idle(), Some("18a9f2c3-1a2b")), None);
        assert_eq!(
            resume(tracker.update(&idle(), None)),
            Some(words(&["alter-zero"])),
            "/clear: the next restore is a fresh launch"
        );
        assert_eq!(
            resume(tracker.update(&idle(), Some("0c1d"))),
            Some(words(&["alter-zero", "--resume", "0c1d"])),
            "/resume switched conversations"
        );
    }

    #[test]
    fn without_a_runnable_name_no_resume_command_is_offered() {
        // A binary run from a checkout: a herdr restart would type a command
        // the pane's shell cannot find, and herdr replays no history for a
        // pane with a resume command.
        let mut tracker = Tracker::new(None);
        assert_eq!(
            tracker.update(&turn(), Some("18a9f2c3-1a2b")),
            Some(Report {
                status: status(State::Working, None),
                resume: None,
            })
        );
    }

    // ===== the sequence number =====

    #[test]
    fn the_sequence_follows_the_clock() {
        assert_eq!(next_seq(0, 1_780_000_000_000_000), 1_780_000_000_000_000);
        assert_eq!(
            next_seq(1_780_000_000_000_000, 1_780_000_000_000_500),
            1_780_000_000_000_500
        );
    }

    #[test]
    fn the_sequence_never_repeats_or_goes_back() {
        // Two reports in one microsecond, and a clock stepped backwards:
        // herdr drops a report whose seq is not above the last one it took,
        // so each must still be newer.
        assert_eq!(
            next_seq(1_780_000_000_000_000, 1_780_000_000_000_000),
            1_780_000_000_000_001
        );
        assert_eq!(
            next_seq(1_780_000_000_000_000, 1_600_000_000_000_000),
            1_780_000_000_000_001
        );
        assert_eq!(next_seq(u64::MAX, 5), u64::MAX);
    }

    // ===== the resend schedule =====

    #[test]
    fn a_delivered_report_is_kept_alive_at_the_slow_cadence() {
        assert_eq!(Timing::DEFAULT.resend_after(0), KEEPALIVE);
    }

    #[test]
    fn a_failed_report_is_retried_soon_then_less_often() {
        let timing = Timing::DEFAULT;
        assert_eq!(timing.resend_after(1), Duration::from_secs(1));
        assert_eq!(timing.resend_after(2), Duration::from_secs(2));
        assert_eq!(timing.resend_after(3), Duration::from_secs(4));
        assert_eq!(timing.resend_after(5), Duration::from_secs(16));
        // An absent herdr costs a failed connect every half minute, no more.
        assert_eq!(timing.resend_after(6), KEEPALIVE);
        assert_eq!(timing.resend_after(u32::MAX), KEEPALIVE);
    }

    #[test]
    fn the_backoff_scales_with_the_first_retry_and_stops_at_the_keepalive() {
        let timing = Timing {
            request_timeout: REQUEST_TIMEOUT,
            first_retry: Duration::from_millis(50),
            keepalive: Duration::from_secs(1),
        };
        assert_eq!(timing.resend_after(0), Duration::from_secs(1));
        assert_eq!(timing.resend_after(1), Duration::from_millis(50));
        assert_eq!(timing.resend_after(2), Duration::from_millis(100));
        assert_eq!(timing.resend_after(5), Duration::from_millis(800));
        assert_eq!(timing.resend_after(6), Duration::from_secs(1));
        assert_eq!(timing.resend_after(u32::MAX), Duration::from_secs(1));
    }

    // ===== the requests =====

    fn test_pane() -> Pane {
        Pane {
            id: "w1:p3".to_string(),
            socket: PathBuf::from("/run/herdr.sock"),
        }
    }

    /// A request, parsed back — one line, one JSON object.
    fn parsed(request: &str) -> serde_json::Value {
        assert!(!request.contains('\n'), "one line per request: {request:?}");
        serde_json::from_str(request).expect("a JSON request")
    }

    fn report(state: State) -> Report {
        Report {
            status: status(state, None),
            resume: None,
        }
    }

    #[test]
    fn a_state_report_names_the_pane_the_agent_and_the_state() {
        let request = parsed(&report_request(
            &test_pane(),
            &report(State::Working),
            1_780_000_000_000_000,
        ));
        assert_eq!(request["method"], "pane.report_agent");
        let params = &request["params"];
        assert_eq!(params["pane_id"], "w1:p3");
        assert_eq!(params["source"], SOURCE);
        assert_eq!(params["agent"], AGENT);
        assert_eq!(params["state"], "working");
        assert_eq!(params["seq"], 1_780_000_000_000_000_u64);
        // Nothing it does not know: an absent key, never a null.
        for key in ["message", "agent_session_id", "resume_argv"] {
            assert!(params.get(key).is_none(), "{key} in {params}");
        }
    }

    #[test]
    fn a_blocked_report_carries_its_message() {
        let report = Report {
            status: status(State::Blocked, Some("Bash command: say \"hi\"")),
            resume: None,
        };
        let request = parsed(&report_request(&test_pane(), &report, 7));
        assert_eq!(request["params"]["state"], "blocked");
        assert_eq!(request["params"]["message"], "Bash command: say \"hi\"");
    }

    #[test]
    fn a_report_carries_its_resume_command_and_no_session_id() {
        // herdr keeps an `agent_session_id` only from its own integrations;
        // the resume command is what brings this session back.
        let report = Report {
            status: status(State::Idle, None),
            resume: Some(words(&["alter-zero", "--resume", "18a9f2c3-1a2b"])),
        };
        let request = parsed(&report_request(&test_pane(), &report, 9));
        let params = &request["params"];
        assert_eq!(
            params["resume_argv"],
            serde_json::json!(["alter-zero", "--resume", "18a9f2c3-1a2b"])
        );
        assert!(params.get("agent_session_id").is_none(), "{params}");
    }

    #[test]
    fn the_release_names_the_pane_the_agent_and_its_seq() {
        let request = parsed(&release_request(&test_pane(), 42));
        assert_eq!(request["method"], "pane.release_agent");
        let params = &request["params"];
        assert_eq!(params["pane_id"], "w1:p3");
        assert_eq!(params["source"], SOURCE);
        assert_eq!(params["agent"], AGENT);
        assert_eq!(params["seq"], 42);
    }

    #[test]
    fn a_requests_id_is_its_sequence() {
        // herdr echoes the id in its reply and wants it a string; the seq
        // already never repeats, so it makes the id unique for free.
        let request = parsed(&report_request(&test_pane(), &report(State::Idle), 41));
        assert_eq!(request["id"], "alter-zero:41");
        assert_eq!(
            parsed(&release_request(&test_pane(), 42))["id"],
            "alter-zero:42"
        );
    }

    // ===== the reply =====

    #[test]
    fn an_ok_reply_is_success() {
        assert_eq!(
            reply_outcome(br#"{"id":"alter-zero:1","result":{"type":"ok"}}"#),
            Ok(())
        );
    }

    #[test]
    fn an_error_reply_is_its_code_and_message() {
        assert_eq!(
            reply_outcome(
                br#"{"id":"alter-zero:1","error":{"code":"pane_not_found","message":"pane w9:p9 not found"}}"#
            ),
            Err("pane_not_found: pane w9:p9 not found".to_string())
        );
    }

    #[test]
    fn no_reply_or_an_unreadable_one_is_a_failure() {
        // herdr hangs up without a word on a request it could not read.
        assert!(reply_outcome(b"").is_err());
        assert!(reply_outcome(b"not json").is_err());
        assert!(
            reply_outcome(br#"{"id":"x"}"#).is_err(),
            "neither result nor error"
        );
    }

    // ===== the worker's outbox =====

    #[test]
    fn an_empty_outbox_has_no_work() {
        assert_eq!(Outbox::default().take(), None);
    }

    #[test]
    fn a_report_is_taken_once() {
        let mut outbox = Outbox::default();
        outbox.post(report(State::Working));
        assert_eq!(outbox.take(), Some(Job::Report(report(State::Working))));
        assert_eq!(outbox.take(), None);
    }

    #[test]
    fn only_the_newest_unsent_report_is_kept() {
        // herdr wants the current state, not the history of it: a report the
        // worker never got to is superseded, never queued behind.
        let mut outbox = Outbox::default();
        outbox.post(report(State::Working));
        outbox.post(report(State::Blocked));
        outbox.post(report(State::Idle));
        assert_eq!(outbox.take(), Some(Job::Report(report(State::Idle))));
        assert_eq!(outbox.take(), None);
    }

    #[test]
    fn the_release_replaces_an_unsent_report_and_ends_the_work() {
        let mut outbox = Outbox::default();
        outbox.post(report(State::Working));
        outbox.release();
        assert_eq!(outbox.take(), Some(Job::Release));
        assert_eq!(outbox.take(), None);
    }

    #[test]
    fn nothing_is_reported_after_the_release() {
        // A report posted after the release would claim the pane back for a
        // process that is on its way out.
        let mut outbox = Outbox::default();
        outbox.release();
        outbox.post(report(State::Idle));
        assert_eq!(outbox.take(), Some(Job::Release));
        outbox.post(report(State::Idle));
        outbox.release();
        assert_eq!(outbox.take(), None, "one release, ever");
    }

    // ===== the socket =====

    #[test]
    fn only_a_socket_this_user_owns_is_written_to() {
        assert!(socket_acceptable(true, 1000, 1000));
        assert!(!socket_acceptable(false, 1000, 1000), "not a socket");
        assert!(!socket_acceptable(true, 0, 1000), "someone else's");
        assert!(!socket_acceptable(true, 1000, 0), "not even root's to take");
    }

    /// A one-connection herdr stand-in on a fresh socket: it hands back what
    /// the client wrote (up to the newline) after answering with `reply` —
    /// or, given `None`, holding the connection open without a word. Every
    /// wait in it is bounded, so a client that never connects fails the test
    /// instead of hanging the suite.
    fn stub(
        reply: Option<&'static str>,
    ) -> (
        tempfile::TempDir,
        PathBuf,
        std::thread::JoinHandle<Option<Vec<u8>>>,
    ) {
        use std::io::{BufRead, Write};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(_) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return None,
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = std::io::BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).ok()?;
            let mut stream = reader.into_inner();
            match reply {
                Some(reply) => stream.write_all(reply.as_bytes()).ok()?,
                None => std::thread::sleep(Duration::from_secs(2)),
            }
            Some(line)
        });
        (dir, path, server)
    }

    const OK: &str = "{\"id\":\"alter-zero:1\",\"result\":{\"type\":\"ok\"}}\n";

    #[test]
    fn send_writes_one_line_and_reads_the_reply() {
        let (_dir, path, server) = stub(Some(OK));
        let request = release_request(&test_pane(), 1);
        assert!(send(&path, &request, Duration::from_secs(2)).is_ok());
        assert_eq!(
            server.join().unwrap(),
            Some(format!("{request}\n").into_bytes())
        );
    }

    #[test]
    fn send_reports_herdrs_refusal() {
        let (_dir, path, _server) = stub(Some(
            "{\"id\":\"x\",\"error\":{\"code\":\"pane_not_found\",\"message\":\"pane w1:p3 not found\"}}\n",
        ));
        let error = send(&path, "{}", Duration::from_secs(2)).unwrap_err();
        assert!(error.to_string().contains("pane_not_found"), "{error}");
    }

    #[test]
    fn send_gives_up_on_a_silent_herdr() {
        // A wedged server must cost the worker its timeout, never a hang.
        let (_dir, path, _server) = stub(None);
        let began = std::time::Instant::now();
        assert!(send(&path, "{}", Duration::from_millis(100)).is_err());
        assert!(
            began.elapsed() < Duration::from_secs(1),
            "{:?}",
            began.elapsed()
        );
    }

    #[test]
    fn send_fails_at_once_without_a_socket() {
        let dir = tempfile::tempdir().unwrap();
        let began = std::time::Instant::now();
        assert!(send(&dir.path().join("gone.sock"), "{}", Duration::from_secs(2)).is_err());
        assert!(began.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn send_never_follows_a_symlink_to_a_socket() {
        // What is checked is what is connected to: the path's own entry. A
        // link, wherever it points, is not a socket this user owns.
        let (dir, path, _server) = stub(Some(OK));
        let link = dir.path().join("link.sock");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let error = send(&link, "{}", Duration::from_secs(2)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
    }

    #[test]
    fn send_never_writes_to_a_plain_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("herdr.sock");
        std::fs::write(&file, "").unwrap();
        assert!(send(&file, "{}", Duration::from_secs(2)).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"");
    }

    // ===== the worker, against a real socket =====

    /// How the stand-in answers one request.
    enum Answer {
        /// `ok` — a healthy herdr.
        Ok,
        /// An `error` reply — herdr refusing the request.
        Refuse,
        /// Nothing, the connection held open — a wedged herdr.
        Hold,
    }

    /// A herdr stand-in serving one request per connection, as herdr does:
    /// each request's line is parsed and handed to the test on the returned
    /// channel, then answered as `answer` decides for it by its index.
    /// `answer` may block — waiting on a token from the test — to hold the
    /// worker in a send while more work queues behind it.
    fn server(
        mut answer: impl FnMut(usize) -> Answer + Send + 'static,
    ) -> (
        tempfile::TempDir,
        PathBuf,
        std::sync::mpsc::Receiver<serde_json::Value>,
    ) {
        use std::io::{BufRead, Write};
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("herdr.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let (requests, received) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for (index, stream) in listener.incoming().enumerate() {
                let Ok(stream) = stream else { return };
                let mut reader = std::io::BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line.is_empty() {
                    continue;
                }
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].clone();
                if requests.send(request).is_err() {
                    return;
                }
                let mut stream = reader.into_inner();
                let reply = match answer(index) {
                    Answer::Ok => serde_json::json!({"id": id, "result": {"type": "ok"}}),
                    Answer::Refuse => serde_json::json!({
                        "id": id,
                        "error": {"code": "pane_not_found", "message": "pane w1:p3 not found"},
                    }),
                    Answer::Hold => {
                        held.push(stream);
                        continue;
                    }
                };
                let _ = writeln!(stream, "{reply}");
            }
        });
        (dir, socket, received)
    }

    fn pane_at(socket: PathBuf) -> Pane {
        Pane {
            id: "w1:p3".to_string(),
            socket,
        }
    }

    /// A pacing a test can watch: a request may hang for seconds, a failed
    /// one is retried within 50 ms, and nothing else is resent for a minute.
    fn quick() -> Timing {
        Timing {
            request_timeout: Duration::from_secs(5),
            first_retry: Duration::from_millis(50),
            keepalive: Duration::from_secs(60),
        }
    }

    fn reporter_on(socket: PathBuf, timing: Timing) -> Reporter {
        Reporter::spawn_with(pane_at(socket), timing).expect("a reporter")
    }

    fn next(requests: &std::sync::mpsc::Receiver<serde_json::Value>) -> serde_json::Value {
        requests
            .recv_timeout(Duration::from_secs(5))
            .expect("a request")
    }

    fn nothing_more(requests: &std::sync::mpsc::Receiver<serde_json::Value>, within: Duration) {
        if let Ok(request) = requests.recv_timeout(within) {
            panic!("nothing more was due, got {request}");
        }
    }

    fn state_of(request: &serde_json::Value) -> &str {
        request["params"]["state"].as_str().unwrap_or("")
    }

    fn seq_of(request: &serde_json::Value) -> u64 {
        request["params"]["seq"].as_u64().expect("a seq")
    }

    #[test]
    fn the_reporter_sends_each_report_then_releases_last() {
        let (_dir, socket, requests) = server(|_| Answer::Ok);
        let mut reporter = reporter_on(socket, quick());
        // Each lands before the next is posted, so none is collapsed away.
        reporter.post(report(State::Working));
        let first = next(&requests);
        reporter.post(report(State::Idle));
        let second = next(&requests);
        reporter.release(Duration::from_secs(5));
        let last = next(&requests);
        let seen: Vec<_> = [&first, &second, &last]
            .iter()
            .map(|request| (request["method"].clone(), state_of(request).to_string()))
            .collect();
        assert_eq!(
            seen,
            [
                ("pane.report_agent".into(), "working".to_string()),
                ("pane.report_agent".into(), "idle".to_string()),
                ("pane.release_agent".into(), String::new()),
            ]
        );
        let seqs = [seq_of(&first), seq_of(&second), seq_of(&last)];
        assert!(seqs.windows(2).all(|w| w[0] < w[1]), "climbing: {seqs:?}");
        nothing_more(&requests, Duration::from_millis(200));
    }

    #[test]
    fn reports_queued_behind_a_slow_send_collapse_to_the_newest() {
        // herdr's own advice: send only the latest state, and drop the older
        // ones that queued while a report was in flight.
        let (go, gate) = std::sync::mpsc::channel::<()>();
        let (_dir, socket, requests) = server(move |_| {
            let _ = gate.recv();
            Answer::Ok
        });
        let mut reporter = reporter_on(socket, quick());
        reporter.post(report(State::Working));
        assert_eq!(state_of(&next(&requests)), "working");
        // The worker waits on herdr's answer now; three more queue behind it.
        reporter.post(report(State::Blocked));
        reporter.post(report(State::Working));
        reporter.post(report(State::Idle));
        go.send(()).unwrap();
        assert_eq!(state_of(&next(&requests)), "idle", "only the newest");
        go.send(()).unwrap();
        go.send(()).unwrap();
        reporter.release(Duration::from_secs(5));
        assert_eq!(next(&requests)["method"], "pane.release_agent");
    }

    #[test]
    fn the_release_supersedes_a_report_still_waiting_to_go_out() {
        // Quitting mid-turn: the release clears the pane's state anyway, so a
        // report it overtook is not worth a round trip.
        let (go, gate) = std::sync::mpsc::channel::<()>();
        let (_dir, socket, requests) = server(move |_| {
            let _ = gate.recv();
            Answer::Ok
        });
        let mut reporter = reporter_on(socket, quick());
        reporter.post(report(State::Working));
        assert_eq!(state_of(&next(&requests)), "working");
        reporter.post(report(State::Idle));
        let answers = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let _ = go.send(());
            let _ = go.send(());
        });
        reporter.release(Duration::from_secs(5));
        answers.join().unwrap();
        assert_eq!(next(&requests)["method"], "pane.release_agent");
        nothing_more(&requests, Duration::from_millis(200));
    }

    #[test]
    fn dropping_the_reporter_releases_the_pane() {
        // A loop that bails out on an error never reaches the shutdown's
        // release; the drop still hands the pane back.
        let (_dir, socket, requests) = server(|_| Answer::Ok);
        let reporter = reporter_on(socket, quick());
        reporter.post(report(State::Working));
        assert_eq!(state_of(&next(&requests)), "working");
        drop(reporter);
        assert_eq!(next(&requests)["method"], "pane.release_agent");
    }

    #[test]
    fn a_release_waits_no_longer_than_it_is_told_to() {
        // herdr gone silent: the quit must not hang on it.
        let (_dir, socket, requests) = server(|_| Answer::Hold);
        let mut reporter = reporter_on(socket, quick());
        reporter.post(report(State::Working));
        assert_eq!(state_of(&next(&requests)), "working");
        let began = std::time::Instant::now();
        reporter.release(Duration::from_millis(200));
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "{:?}",
            began.elapsed()
        );
    }

    #[test]
    fn an_unreachable_herdr_never_blocks_the_caller() {
        let dir = tempfile::tempdir().unwrap();
        let mut reporter = reporter_on(dir.path().join("gone.sock"), quick());
        let began = std::time::Instant::now();
        for _ in 0..100 {
            reporter.post(report(State::Working));
            reporter.post(report(State::Idle));
        }
        assert!(
            began.elapsed() < Duration::from_millis(250),
            "posting waits"
        );
        let began = std::time::Instant::now();
        reporter.release(Duration::from_secs(5));
        assert!(
            began.elapsed() < Duration::from_secs(1),
            "a refused connect fails at once: {:?}",
            began.elapsed()
        );
    }

    #[test]
    fn a_refused_report_is_sent_again_soon() {
        // A report herdr did not take leaves the pane showing the state
        // before it: the worker tries again within the first retry, under a
        // fresh seq (herdr ignores one it has seen), and once it lands rests
        // until the keepalive.
        let (_dir, socket, requests) = server(|index| {
            if index == 0 {
                Answer::Refuse
            } else {
                Answer::Ok
            }
        });
        let mut reporter = reporter_on(socket, quick());
        reporter.post(report(State::Blocked));
        let refused = next(&requests);
        let again = next(&requests);
        assert_eq!(state_of(&again), "blocked");
        assert!(seq_of(&again) > seq_of(&refused));
        nothing_more(&requests, Duration::from_millis(300));
        reporter.release(Duration::from_secs(5));
    }

    #[test]
    fn a_delivered_report_is_sent_again_on_the_keepalive() {
        // herdr keeps no expiry on a self-reported state but loses it on a
        // live upgrade: the keepalive puts it back within one period.
        let (_dir, socket, requests) = server(|_| Answer::Ok);
        let timing = Timing {
            keepalive: Duration::from_millis(100),
            ..quick()
        };
        let mut reporter = reporter_on(socket, timing);
        reporter.post(report(State::Idle));
        let first = next(&requests);
        let again = next(&requests);
        assert_eq!(state_of(&again), "idle");
        assert!(seq_of(&again) > seq_of(&first));
        reporter.release(Duration::from_secs(5));
    }

    #[test]
    fn nothing_is_sent_before_the_first_report() {
        let (_dir, socket, requests) = server(|_| Answer::Ok);
        let timing = Timing {
            keepalive: Duration::from_millis(50),
            ..quick()
        };
        let mut reporter = reporter_on(socket, timing);
        nothing_more(&requests, Duration::from_millis(300));
        reporter.release(Duration::from_secs(5));
        assert_eq!(next(&requests)["method"], "pane.release_agent");
    }
}
