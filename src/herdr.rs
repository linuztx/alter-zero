//! herdr support — telling the multiplexer pane this session runs in what
//! the agent is doing (`docs/herdr.md`).
//!
//! [herdr](https://herdr.dev) is a terminal multiplexer for coding agents:
//! its sidebar shows every pane's agent as **working**, **blocked** on the
//! user, **done** (finished, not yet looked at) or **idle**, and notifies when
//! one needs attention or finishes. It recognises the agents it ships rules
//! for by their screens; any other agent reports for itself over herdr's
//! local socket, and its reports are then the pane's only authority. This is
//! the pure half of that report: which pane ([`pane`], from the variables
//! herdr sets on a pane's processes), what to say ([`status`], derived from
//! the [`App`] — never tracked as events), the request lines
//! ([`report_request`], [`release_request`]), the sequence every report
//! must climb ([`next_seq`]), the command herdr resumes the session with
//! after a restart ([`resume_argv`]) and the worker's single-slot
//! [`Outbox`]. No clock, no environment, no thread: the boundary
//! (`tui::herdr`) reads those, owns the worker, and calls the one impure
//! function here, [`send`].

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::ask::AskQuestion;
use crate::permission::{self, PermissionKind, PermissionRequest};

/// The app's own off switch for a run.
pub const DISABLE_ENV: &str = "ALTER_ZERO_HERDR";

/// Set (to `1`) on every process in a herdr pane.
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

/// The longest blocked message the reports carry, in characters.
pub const MESSAGE_MAX_CHARS: usize = 160;

/// How often the current report is sent again while nothing changes
/// ([`resend_after`]).
pub const KEEPALIVE: Duration = Duration::from_secs(30);

/// The most words herdr takes in a resume command.
pub const RESUME_MAX_ARGS: usize = 64;

/// The most bytes herdr takes across a resume command's words (separators
/// not counted).
pub const RESUME_MAX_BYTES: usize = 8 * 1024;

/// What the agent is doing, in herdr's three words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Ready for the user's next message — herdr shows it as done until the
    /// user has looked.
    Idle,
    /// Busy: a turn in flight, or a subagent still running.
    Working,
    /// Waiting on the user's decision: a permission prompt or a question.
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
/// boundary's environment lookup. `None` outside herdr, when any of the
/// three variables is missing or blank, when [`HERDR_ENV`] is falsy, and when
/// [`DISABLE_ENV`] turns the integration off for the run.
#[must_use]
pub fn pane(var: impl Fn(&str) -> Option<String>) -> Option<Pane> {
    if var(DISABLE_ENV).is_some_and(|value| falsy(&value)) {
        return None;
    }
    let flag = var(HERDR_ENV)?;
    if flag.trim().is_empty() || falsy(&flag) {
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

/// What herdr is told about the session right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// What a blocked session waits on, in one line; `None` otherwise.
    pub message: Option<String>,
}

/// The session's state as herdr should show it.
///
/// **Blocked** while a thread is parked on the user — a permission prompt or
/// a question ([`App::modal_open`]), whoever raised it, a background agent
/// included — with the prompt's own words as the message. **Working** while
/// a turn is in flight (a reply, a `!` command, a compaction) or any subagent
/// is still running: the lead's turn can end with agents at work whose
/// results start the next turn on their own, and reporting idle there would
/// tell herdr the work is done while it goes on. **Idle** otherwise.
/// Background shells don't count — a dev server runs until it is stopped.
#[must_use]
pub fn status(app: &App) -> Status {
    if let Some(prompt) = app.permission() {
        return blocked(&permission_message(&prompt.request));
    }
    if let Some(prompt) = app.ask() {
        return blocked(&question_message(&prompt.request.questions));
    }
    let working = app.turn_active() || app.agents().iter().any(|run| !run.status.is_final());
    Status {
        state: if working { State::Working } else { State::Idle },
        message: None,
    }
}

fn blocked(message: &str) -> Status {
    Status {
        state: State::Blocked,
        message: Some(one_line(message, MESSAGE_MAX_CHARS)),
    }
}

/// A permission prompt in a line: its title over what it names — the file,
/// the command, the server's tool, the program a session runs.
fn permission_message(request: &PermissionRequest) -> String {
    let subject = match request.kind {
        PermissionKind::Write | PermissionKind::Edit => {
            permission::file_name(&request.target).to_string()
        }
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

/// `text` on one line — every whitespace run a single space — cut to `max`
/// characters with a closing `…`.
fn one_line(text: &str, max: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= max {
        return joined;
    }
    let mut cut: String = joined.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The sequence number for the next request: `now` — the boundary's clock in
/// microseconds, herdr's own reporters' unit — or one past `last` when the
/// clock has not moved on.
///
/// herdr drops a request whose `seq` is not above the highest it has taken
/// from this source for this pane, and keeps that mark for the pane's whole
/// life. So every request must outrank the one before — two in the same
/// microsecond, a clock stepped back — and a relaunch in the same pane must
/// outrank the process before it, which a counter starting at 1 never would.
/// The clock gives the second; the `+ 1` the first. (Never move to a finer
/// unit later: the old mark would outrank every new request.)
#[must_use]
pub const fn next_seq(last: u64, now: u64) -> u64 {
    let after = last.saturating_add(1);
    if now > after { now } else { after }
}

/// How long the worker waits before sending the current report again, after
/// `failures` failed sends in a row: one second, doubling, up to
/// [`KEEPALIVE`] — which is also the wait with none, since herdr holds a
/// self-reported state with no expiry, and a report it lost (a live upgrade
/// drops a third-party agent's state; a timeout drops one request) would
/// otherwise stand wrong until the next change. A resend of an unchanged
/// state notifies nobody.
#[must_use]
pub fn resend_after(failures: u32) -> Duration {
    if failures == 0 {
        return KEEPALIVE;
    }
    let doubled = 1u64 << failures.saturating_sub(1).min(16);
    Duration::from_secs(doubled).min(KEEPALIVE)
}

/// The command herdr runs to bring this session back after a restart —
/// `{bin} --resume {id}`, the exit hint's own command (`docs/cli.md`) — or
/// `None` when herdr would refuse it. herdr's validator is the rule, mirrored
/// here because a refused command takes the state report it rode in with:
/// the first word a plain command name (`[A-Za-z0-9_.-]`, not leading with
/// `-` — herdr types it into the pane's shell, which finds it on `PATH`), no
/// word with an apostrophe or a control character, at most
/// [`RESUME_MAX_ARGS`] words and [`RESUME_MAX_BYTES`] bytes between them.
#[must_use]
pub fn resume_argv(bin: &str, session_id: &str) -> Option<Vec<String>> {
    let plain_name = !bin.is_empty()
        && !bin.starts_with('-')
        && bin
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    if !plain_name || session_id.is_empty() {
        return None;
    }
    let argv = vec![
        bin.to_string(),
        "--resume".to_string(),
        session_id.to_string(),
    ];
    let clean = argv
        .iter()
        .all(|word| !word.chars().any(|c| c == '\'' || c.is_control()));
    let bytes: usize = argv.iter().map(String::len).sum();
    (clean && argv.len() <= RESUME_MAX_ARGS && bytes <= RESUME_MAX_BYTES).then_some(argv)
}

/// What one report tells herdr: the state, and — once the conversation is
/// recorded — the session and the command that brings it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub status: Status,
    /// The rollout's id — what `--resume` takes — once the conversation has
    /// a file.
    pub session: Option<String>,
    /// The command herdr resumes the session with (`resume_argv`).
    pub resume: Option<Vec<String>>,
}

impl Report {
    /// The report for `status` in the recorded `session`, if any — whose
    /// resume command starts with `resume_bin`, the name herdr can run this
    /// binary by (`None` when it cannot, which names the session but offers
    /// no way back).
    #[must_use]
    pub fn new(status: Status, session: Option<&str>, resume_bin: Option<&str>) -> Self {
        let resume = session
            .zip(resume_bin)
            .and_then(|(id, bin)| resume_argv(bin, id));
        Self {
            status,
            session: session.map(str::to_string),
            resume,
        }
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
/// one is what an older herdr has never heard of).
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
    agent_session_id: Option<&'a str>,
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
/// and is in its state, with the blocked message, the session and its resume
/// command when there are any. One request carries the whole picture — herdr
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
            agent_session_id: report.session.as_deref(),
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

/// Write one request to herdr's socket and read its one-line reply: one
/// connection per request, herdr's own model, with the write and the read
/// each bounded by `timeout` (herdr sets no deadline of its own on a
/// report). The one impure function here — the boundary's worker thread
/// calls it, never the event loop.
///
/// # Errors
/// The connect, write or read failure, or herdr's refusal
/// ([`reply_outcome`]). Off Unix, always — herdr's socket there is a named
/// pipe this integration does not speak.
pub fn send(socket: &Path, request: &str, timeout: Duration) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::{BufRead, Read, Write};
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
pub struct Outbox {
    report: Option<Report>,
    release: bool,
    released: bool,
}

/// One unit of the worker's work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
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
    pub fn post(&mut self, report: Report) {
        if !self.released {
            self.report = Some(report);
        }
    }

    /// Queue the release, dropping any unsent report: the last job, queued
    /// once.
    pub fn release(&mut self) {
        if !self.released {
            self.released = true;
            self.release = true;
            self.report = None;
        }
    }

    /// The next job, if any: the release once queued, else the newest report.
    pub fn take(&mut self) -> Option<Job> {
        if std::mem::take(&mut self.release) {
            return Some(Job::Release);
        }
        self.report.take().map(Job::Report)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
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

    // ===== the states' wire spelling =====

    #[test]
    fn states_are_herdrs_three_words() {
        assert_eq!(State::Idle.as_str(), "idle");
        assert_eq!(State::Working.as_str(), "working");
        assert_eq!(State::Blocked.as_str(), "blocked");
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
    fn blank_or_falsy_values_are_no_pane() {
        for flag in ["0", "false", "off", "no", "", "  "] {
            let mut vars = IN_PANE.to_vec();
            vars[0] = (HERDR_ENV, flag);
            assert_eq!(pane(env(&vars)), None, "HERDR_ENV={flag:?}");
        }
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

    // ===== the state =====

    #[test]
    fn a_fresh_session_is_idle() {
        let app = App::new();
        assert_eq!(
            status(&app),
            Status {
                state: State::Idle,
                message: None,
            }
        );
    }

    #[test]
    fn a_turn_in_flight_is_working() {
        let mut app = App::new();
        app.begin_stream();
        assert_eq!(status(&app).state, State::Working);
        assert_eq!(status(&app).message, None);
        app.end_turn(3);
        assert_eq!(status(&app).state, State::Idle);
    }

    #[test]
    fn a_shell_command_is_working() {
        let mut app = App::new();
        app.begin_shell("cargo test");
        assert_eq!(status(&app).state, State::Working);
    }

    #[test]
    fn a_compaction_is_working() {
        let mut app = App::new();
        app.begin_compact(false);
        assert_eq!(status(&app).state, State::Working);
    }

    #[test]
    fn a_permission_prompt_blocks_and_says_what_it_asks() {
        let mut app = App::new();
        app.begin_stream();
        app.open_permission(permission(PermissionKind::Bash, "cargo test --all"));
        assert_eq!(
            status(&app),
            Status {
                state: State::Blocked,
                message: Some("Bash command: cargo test --all".to_string()),
            }
        );
    }

    #[test]
    fn a_file_prompt_names_the_file_not_its_path() {
        let mut app = App::new();
        app.open_permission(permission(PermissionKind::Write, "/repo/src/hello.py"));
        assert_eq!(
            status(&app).message.as_deref(),
            Some("Create file: hello.py")
        );
        let mut app = App::new();
        app.open_permission(permission(PermissionKind::Edit, "/repo/src/main.rs"));
        assert_eq!(status(&app).message.as_deref(), Some("Edit file: main.rs"));
    }

    #[test]
    fn an_mcp_prompt_names_the_server_and_tool() {
        let mut app = App::new();
        app.open_permission(permission(
            PermissionKind::Mcp,
            "mcp__deepwiki__ask_question",
        ));
        assert_eq!(
            status(&app).message.as_deref(),
            Some("Tool use: Deepwiki - ask_question")
        );
    }

    #[test]
    fn a_session_prompt_names_the_program_it_types_into() {
        let mut app = App::new();
        let mut request = permission(PermissionKind::Session, "s1");
        request.detail = Some("python3".to_string());
        app.open_permission(request);
        assert_eq!(
            status(&app).message.as_deref(),
            Some("Session input: python3")
        );
        // With no command known, the session id is all there is to name.
        let mut app = App::new();
        app.open_permission(permission(PermissionKind::Session, "s1"));
        assert_eq!(status(&app).message.as_deref(), Some("Session input: s1"));
    }

    #[test]
    fn a_question_blocks_and_quotes_the_question() {
        let mut app = App::new();
        app.begin_stream();
        app.open_ask(ask(&["Which database should the cache use?"]));
        assert_eq!(
            status(&app),
            Status {
                state: State::Blocked,
                message: Some("Question: Which database should the cache use?".to_string()),
            }
        );
    }

    #[test]
    fn several_questions_count_the_rest() {
        let mut app = App::new();
        app.open_ask(ask(&["First?", "Second?", "Third?"]));
        assert_eq!(
            status(&app).message.as_deref(),
            Some("Question: First? (+2 more)")
        );
    }

    #[test]
    fn a_blocked_message_is_one_line_and_bounded() {
        let mut app = App::new();
        app.open_permission(permission(
            PermissionKind::Bash,
            "for f in *.rs; do\n  rustfmt \"$f\"\ndone",
        ));
        assert_eq!(
            status(&app).message.as_deref(),
            Some("Bash command: for f in *.rs; do rustfmt \"$f\" done")
        );

        let mut app = App::new();
        app.open_permission(permission(PermissionKind::Bash, &"x".repeat(500)));
        let message = status(&app).message.expect("a blocked message");
        assert_eq!(message.chars().count(), MESSAGE_MAX_CHARS);
        assert!(message.ends_with('…'), "{message}");
    }

    #[test]
    fn a_running_subagent_keeps_the_session_working_between_turns() {
        // The lead's turn ended, but the agents it launched are still at
        // it — and their results will start a follow-up turn on their own.
        // Reporting idle here would announce "done" while the work goes on.
        let mut app = App::new();
        app.start_agent_group(true, &[agent("a1", true), agent("a2", true)]);
        assert_eq!(status(&app).state, State::Working);
        app.apply_agent_event("a1", &StreamEvent::StreamDone);
        assert_eq!(status(&app).state, State::Working, "a2 still runs");
        app.apply_agent_event("a2", &StreamEvent::Error("boom".to_string()));
        assert_eq!(status(&app).state, State::Idle);
    }

    #[test]
    fn a_subagents_permission_prompt_blocks_an_idle_session() {
        let mut app = App::new();
        app.start_agent_group(true, &[agent("a1", true)]);
        let mut request = permission(PermissionKind::Bash, "ls -la");
        request.agent = Some("explore".to_string());
        request.agent_id = Some("a1".to_string());
        app.open_permission(request);
        assert_eq!(status(&app).state, State::Blocked);
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
        assert_eq!(resend_after(0), KEEPALIVE);
    }

    #[test]
    fn a_failed_report_is_retried_soon_then_less_often() {
        assert_eq!(resend_after(1), Duration::from_secs(1));
        assert_eq!(resend_after(2), Duration::from_secs(2));
        assert_eq!(resend_after(3), Duration::from_secs(4));
        assert_eq!(resend_after(5), Duration::from_secs(16));
        // An absent herdr costs a failed connect every half minute, no more.
        assert_eq!(resend_after(6), KEEPALIVE);
        assert_eq!(resend_after(u32::MAX), KEEPALIVE);
    }

    // ===== the resume command =====

    #[test]
    fn the_resume_command_is_the_cli_flag() {
        assert_eq!(
            resume_argv("alter-zero", "18a9f2c33d41e5b6-1a2b"),
            Some(vec![
                "alter-zero".to_string(),
                "--resume".to_string(),
                "18a9f2c33d41e5b6-1a2b".to_string(),
            ])
        );
    }

    #[test]
    fn the_resume_command_starts_with_a_plain_command_name() {
        // herdr's own rule for the first word: letters, digits, `_`, `.` and
        // `-`, not leading with `-` — a path, a blank, whitespace or any
        // other character is refused (and the refusal takes the state
        // report it rode in with).
        assert_eq!(resume_argv("target/debug/alter-zero", "abc"), None);
        assert_eq!(resume_argv("/usr/local/bin/alter-zero", "abc"), None);
        assert_eq!(resume_argv("", "abc"), None);
        assert_eq!(resume_argv("alter zero", "abc"), None);
        assert_eq!(resume_argv("-alter-zero", "abc"), None);
        assert_eq!(resume_argv("alter+zero", "abc"), None);
        assert_eq!(resume_argv("altér-zero", "abc"), None);
        assert!(resume_argv("alter_zero.2", "abc").is_some());
    }

    #[test]
    fn the_resume_command_refuses_what_herdr_refuses() {
        assert_eq!(resume_argv("alter-zero", ""), None, "no id");
        assert_eq!(resume_argv("alter-zero", "it's"), None, "an apostrophe");
        assert_eq!(resume_argv("alter-zero", "a\nb"), None, "a control char");
        assert_eq!(resume_argv("alter-zero", "a\tb"), None, "a tab");
        assert_eq!(resume_argv("alter-zero", "a\u{7f}b"), None, "DEL");
        let long = "a".repeat(RESUME_MAX_BYTES);
        assert_eq!(resume_argv("alter-zero", &long), None, "over 8 KiB");
    }

    // ===== the report =====

    fn idle() -> Status {
        Status {
            state: State::Idle,
            message: None,
        }
    }

    #[test]
    fn a_report_carries_the_session_and_how_to_resume_it() {
        assert_eq!(
            Report::new(idle(), Some("18a9f2c3-1a2b"), Some("alter-zero")),
            Report {
                status: idle(),
                session: Some("18a9f2c3-1a2b".to_string()),
                resume: resume_argv("alter-zero", "18a9f2c3-1a2b"),
            }
        );
    }

    #[test]
    fn a_report_resumes_nothing_without_a_session_or_a_runnable_name() {
        // No conversation recorded yet: nothing to come back to.
        let report = Report::new(idle(), None, Some("alter-zero"));
        assert_eq!((report.session, report.resume), (None, None));
        // A name herdr could not run still names the session.
        let report = Report::new(idle(), Some("abc"), None);
        assert_eq!(
            (report.session.as_deref(), report.resume),
            (Some("abc"), None)
        );
        let report = Report::new(idle(), Some("abc"), Some("target/debug/alter-zero"));
        assert_eq!(report.resume, None);
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

    #[test]
    fn a_state_report_names_the_pane_the_agent_and_the_state() {
        let report = Report::new(
            Status {
                state: State::Working,
                message: None,
            },
            None,
            None,
        );
        let request = parsed(&report_request(
            &test_pane(),
            &report,
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
        let report = Report::new(
            Status {
                state: State::Blocked,
                message: Some("Bash command: say \"hi\"\nthen exit".to_string()),
            },
            None,
            None,
        );
        let request = parsed(&report_request(&test_pane(), &report, 7));
        assert_eq!(request["params"]["state"], "blocked");
        assert_eq!(
            request["params"]["message"],
            "Bash command: say \"hi\"\nthen exit"
        );
    }

    #[test]
    fn a_state_report_names_the_session_and_its_resume_command() {
        let report = Report::new(idle(), Some("18a9f2c3-1a2b"), Some("alter-zero"));
        let request = parsed(&report_request(&test_pane(), &report, 9));
        let params = &request["params"];
        assert_eq!(params["agent_session_id"], "18a9f2c3-1a2b");
        assert_eq!(
            params["resume_argv"],
            serde_json::json!(["alter-zero", "--resume", "18a9f2c3-1a2b"])
        );
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
        let report = parsed(&report_request(
            &test_pane(),
            &Report::new(idle(), None, None),
            41,
        ));
        assert_eq!(report["id"], "alter-zero:41");
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

    fn report(state: State) -> Report {
        Report::new(
            Status {
                state,
                message: None,
            },
            None,
            None,
        )
    }

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

    #[test]
    fn send_writes_one_line_and_reads_the_reply() {
        let (_dir, path, server) = stub(Some(
            "{\"id\":\"alter-zero:1\",\"result\":{\"type\":\"ok\"}}\n",
        ));
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
}
