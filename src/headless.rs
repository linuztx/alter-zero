//! Headless mode's pure core — `alter-zero -p` (see `docs/cli/headless.md`).
//!
//! `-p`/`--print` runs one prompt with no screen: the same conversation the
//! TUI runs, its answer printed to stdout and its progress logged to stderr,
//! then the process exits with a status a script can branch on. The boundary
//! (`tui::headless`) owns the process — the streams, the signals, the loop —
//! and asks this module every question that has an answer without one: which
//! output format a flag names, what the prompt is when stdin carries one too,
//! what the run's answer was, what each line of output says, and which exit
//! status a run ends with.

use std::time::Duration;

use crate::app::{HistoryItem, Role};

/// How a headless run reports (`--output-format`, `docs/cli/headless.md`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputFormat {
    /// The answer alone on stdout, the run's log on stderr — what a shell
    /// pipeline or `$(…)` wants.
    #[default]
    Text,
    /// One JSON object on stdout when the run ends: the answer, the session
    /// id, the usage, how it ended.
    Json,
    /// One JSON event per line on stdout as the run happens, closed by the
    /// same object [`Json`](Self::Json) prints.
    StreamJson,
}

impl OutputFormat {
    /// Every format, in the order the help and the error list them.
    pub const ALL: [Self; 3] = [Self::Text, Self::Json, Self::StreamJson];

    /// The flag value naming this format.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::StreamJson => "stream-json",
        }
    }

    /// The format a `--output-format` value names, or `None` for anything
    /// else.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| format.label() == value)
    }
}

// ===== the prompt: the argument, stdin, or both =====

/// How long a socket given **beside** a prompt argument may stay silent
/// before the run goes on without it (`docs/cli/headless.md` *The prompt*): a
/// parent that leaves it open and never writes to it — Node's `exec`, so
/// n8n's command node — would otherwise hold the run forever, codex's hang.
/// Anything at all within it — data or end-of-file — and it is read to its
/// end. A pipe is never cut off: a shell's `|` ends when its writer does,
/// and data a slow writer has yet to send would be lost.
pub const STDIN_WAIT: Duration = Duration::from_secs(3);

/// How long a run that has nothing left but background subagents waits on
/// them before stopping them and ending on its last reply (`docs/cli/headless.md`
/// *What the run does*) — a bound on an agent that never finishes.
pub const AGENT_WAIT_DEFAULT: Duration = Duration::from_secs(600);

/// The variable that sets [`agent_wait`]: whole seconds, `0` for no wait.
pub const AGENT_WAIT_ENV: &str = "ALTER_ZERO_HEADLESS_AGENT_WAIT_SECS";

/// The wait on background subagents, from [`AGENT_WAIT_ENV`]'s value:
/// whole seconds, `0` for none — anything else is [`AGENT_WAIT_DEFAULT`].
#[must_use]
pub fn agent_wait(raw: Option<&str>) -> Duration {
    raw.and_then(|raw| raw.trim().parse::<u64>().ok())
        .map_or(AGENT_WAIT_DEFAULT, Duration::from_secs)
}

/// How long stdin may stay open before a run says on stderr that it is still
/// waiting for its end, and how to stop it waiting (`< /dev/null`).
pub const STDIN_NOTE_AFTER: Duration = Duration::from_secs(10);

/// The most stdin a run takes — far past any model's context window, so
/// input this large is a mistake to report, not a prompt to send.
pub const STDIN_MAX_BYTES: usize = 8 * 1024 * 1024;

/// The prompt argument that names stdin itself: `alter-zero -p -`.
pub const STDIN_ARG: &str = "-";

/// What stdin is, as far as reading it goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdinKind {
    /// A terminal — somebody's keyboard, never read for a prompt unasked.
    Terminal,
    /// A regular file (`< task.md`): finite, read whole without waiting.
    File,
    /// A pipe — a shell's `|`, Python's `Popen(stdin=PIPE)`: it ends when its
    /// writer does, however slow the command writing it.
    Pipe,
    /// A socket, a device — anything else, which may or may not end: Node's
    /// `spawn`/`exec` hand the commands they start a socket (n8n's Execute
    /// Command node), one the parent may hold open without a word.
    Stream,
}

/// Whether, and how, a run reads stdin ([`stdin_read`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdinRead {
    /// Leave it alone.
    Skip,
    /// Read it to its end, however long that takes.
    ToEnd,
    /// Read it to its end, saying once on stderr if it has not ended within
    /// the wait — a parent that never closes it holds the run, visibly.
    ToEndNoting(Duration),
    /// Read it to its end if it says anything — data or end-of-file —
    /// within the wait; else leave it alone.
    IfActiveWithin(Duration),
}

/// How a run reads stdin, given its prompt argument and what stdin is
/// (`docs/cli/headless.md`): never a terminal unless `-` asks for it; a file
/// always; a pipe to its end, however long its writer takes; and any other
/// stream in full when it *is* the prompt, but only if it is active within
/// [`STDIN_WAIT`] when an argument was given beside it.
#[must_use]
pub fn stdin_read(arg: Option<&str>, kind: StdinKind) -> StdinRead {
    let names_stdin = arg.is_none_or(|arg| arg == STDIN_ARG);
    match kind {
        // `-` asks for it explicitly, `cat`'s rule: typed until Ctrl+D.
        StdinKind::Terminal if arg == Some(STDIN_ARG) => StdinRead::ToEnd,
        StdinKind::Terminal => StdinRead::Skip,
        StdinKind::File => StdinRead::ToEnd,
        // A pipe ends when its writer does: waited for, however slow.
        StdinKind::Pipe => StdinRead::ToEndNoting(STDIN_NOTE_AFTER),
        StdinKind::Stream if names_stdin => StdinRead::ToEndNoting(STDIN_NOTE_AFTER),
        StdinKind::Stream => StdinRead::IfActiveWithin(STDIN_WAIT),
    }
}

/// What stdin carried, as text ([`decode_stdin`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdinText {
    pub text: String,
    /// Whether bytes that are not UTF-8 were replaced with `U+FFFD`.
    pub replaced: bool,
}

/// Decode what stdin carried: UTF-8, a leading byte-order mark dropped, and
/// any byte that is not UTF-8 replaced with `U+FFFD` — a log with one stray
/// byte is still worth reading, and the run says it was changed.
///
/// Taken by value: the input can be megabytes, and the decode of valid UTF-8
/// reuses its buffer rather than copying it.
#[must_use]
pub fn decode_stdin(bytes: Vec<u8>) -> StdinText {
    const BOM: &str = "\u{feff}";
    let (mut text, replaced) = match String::from_utf8(bytes) {
        Ok(text) => (text, false),
        Err(err) => (String::from_utf8_lossy(err.as_bytes()).into_owned(), true),
    };
    if text.starts_with(BOM) {
        text.drain(..BOM.len());
    }
    StdinText { text, replaced }
}

/// The prompt a run answers, from its argument and what stdin carried
/// (`docs/cli/headless.md`): the argument alone; stdin alone when there is no
/// argument (or it is `-`); both as codex joins them —
/// `{prompt}\n\n<stdin>\n{input}\n</stdin>` — so the model knows what
/// came down the pipe. Blank input adds nothing; `None` when nothing is
/// left to answer.
#[must_use]
pub fn combine_prompt(arg: Option<&str>, stdin: Option<&str>) -> Option<String> {
    let input = stdin.filter(|text| !text.trim().is_empty());
    match arg.filter(|arg| *arg != STDIN_ARG) {
        Some(arg) => Some(match input {
            Some(input) => {
                let close = if input.ends_with('\n') { "" } else { "\n" };
                format!("{arg}\n\n<stdin>\n{input}{close}</stdin>")
            }
            None => arg.to_string(),
        }),
        // Stdin is the whole prompt: its closing line break is the pipe's,
        // not the message's.
        None => input.map(|text| text.trim_end_matches(['\n', '\r']).to_string()),
    }
}

// ===== the run's answer and how it ended =====

/// Whether `item` can follow a turn's answer without being part of it — the
/// turn's own summary, a compaction, a notice that settled at its end, a
/// hook's cell, a thought — so the answer is still what came before it.
fn trails_an_answer(item: &HistoryItem) -> bool {
    match item {
        HistoryItem::Summary(_)
        | HistoryItem::Compaction(_)
        | HistoryItem::Background(_)
        | HistoryItem::AgentNotice(_)
        | HistoryItem::Reasoning(_) => true,
        // A hook's cell with nothing for the model; one whose text the
        // model read is a turn of its own (a Stop hook's continuation).
        HistoryItem::HookNote(note) => note.text.is_empty(),
        HistoryItem::Message(_)
        | HistoryItem::Tool(_)
        | HistoryItem::TaskCall(_)
        | HistoryItem::AgentGroup(_) => false,
    }
}

/// Whether `item` may sit **between** two parts of one answer: a thought
/// the model had mid-reply, a hook's cell with nothing for the model.
fn within_an_answer(item: &HistoryItem) -> bool {
    match item {
        HistoryItem::Reasoning(_) => true,
        HistoryItem::HookNote(note) => note.text.is_empty(),
        _ => false,
    }
}

/// What a run whose last turn never ended — neither completed nor failed —
/// reports as its failure.
const UNFINISHED: &str = "the run ended before its turn did";

/// The run's answer (`docs/cli/headless.md`): its last reply — the assistant
/// messages after the last thing that was not one, joined by a blank line,
/// with what may trail an answer (the turn's summary, a notice settled at
/// its end) looked past. `run` is the history this run added, so an older
/// reply a resumed conversation carried is never mistaken for it. `None`
/// when the last turn ended without one — it failed, or never answered.
#[must_use]
pub fn final_answer(run: &[HistoryItem]) -> Option<String> {
    let mut rest = run;
    while let Some((last, before)) = rest.split_last()
        && trails_an_answer(last)
    {
        rest = before;
    }
    let mut parts = Vec::new();
    for item in rest.iter().rev() {
        match item {
            HistoryItem::Message(message) if message.role == Role::Assistant => {
                let text = message.text.trim();
                if !text.is_empty() {
                    parts.push(text);
                }
            }
            item if within_an_answer(item) => {}
            _ => break,
        }
    }
    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    Some(parts.join("\n\n"))
}

/// How a headless run ended — what its exit status and the result's
/// `subtype` say (`docs/cli/headless.md` *Exit status*).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// The last turn completed.
    Success,
    /// The last turn failed: the provider, a hook that blocked the prompt.
    Failed(String),
    /// `--max-turns` ran out before the model finished.
    MaxTurns(String),
    /// A signal stopped the run (its number).
    Interrupted(i32),
}

impl RunOutcome {
    /// How the run's history says it ended: the last turn's summary means
    /// it completed, its error notice that it failed — `MaxTurns` when the
    /// `--max-turns` budget was what ran out. A run whose turn never ended
    /// either way failed.
    #[must_use]
    pub fn of(run: &[HistoryItem], budget_spent: bool) -> Self {
        // The newest item that says how a turn ended: its summary, or the
        // red notice a failure, a blocked prompt or an interrupt records.
        let ended = run.iter().rev().find_map(|item| match item {
            HistoryItem::Summary(_) => Some(None),
            HistoryItem::Message(message) if message.role == Role::Error => {
                Some(Some(message.text.clone()))
            }
            _ => None,
        });
        match ended {
            Some(None) => Self::Success,
            Some(Some(message)) if budget_spent => Self::MaxTurns(message),
            Some(Some(message)) => Self::Failed(message),
            None => Self::Failed(UNFINISHED.to_string()),
        }
    }

    /// The process's exit status: `0` for a completed run, `1` for a failed
    /// one, `128 + signal` for an interrupted one.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Success => 0,
            Self::Failed(_) | Self::MaxTurns(_) => 1,
            Self::Interrupted(signal) => 128 + *signal,
        }
    }

    /// The result object's `subtype` — Claude Code's vocabulary.
    #[must_use]
    pub const fn subtype(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failed(_) => "error_during_execution",
            Self::MaxTurns(_) => "error_max_turns",
            Self::Interrupted(_) => "error_interrupted",
        }
    }

    /// The failure's message, when the run has one to give.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Success => None,
            Self::Failed(message) | Self::MaxTurns(message) => Some(message),
            Self::Interrupted(2) => Some("interrupted by SIGINT"),
            Self::Interrupted(15) => Some("interrupted by SIGTERM"),
            Self::Interrupted(1) => Some("interrupted by SIGHUP"),
            Self::Interrupted(_) => Some("interrupted by a signal"),
        }
    }
}

// ===== what the run writes =====

/// One line a run writes, and to which stream. The reporter decides; the
/// boundary only writes (`docs/cli/headless.md` *Output*: stdout carries the
/// answer or the JSON and nothing else, stderr everything else).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Emit {
    Stdout(String),
    Stderr(String),
}

/// The prefix of every line the app itself — not the run's log — writes to
/// stderr: a warning, a retry, what a run left behind.
pub const STDERR_PREFIX: &str = "alter-zero: ";

/// A warning line for stderr — a startup toast, a held hook — in every format.
#[must_use]
pub fn warning(text: &str) -> Emit {
    Emit::Stderr(format!("{STDERR_PREFIX}{text}"))
}

/// What a run that never started writes — no model, no session to continue,
/// stdin that could not be read (`docs/cli/headless.md` *Exit status*): why, on
/// stderr in every format, and for the JSON formats the result object a
/// script reads, so `jq .result` meets an error rather than nothing.
#[must_use]
pub fn not_started(format: OutputFormat, error: &str, duration: Duration) -> Vec<Emit> {
    let mut out = vec![warning(error)];
    if format != OutputFormat::Text {
        out.push(Emit::Stdout(event_line(&Event::Result(ResultBody {
            subtype: RunOutcome::Failed(String::new()).subtype(),
            is_error: true,
            result: "",
            error: Some(error),
            session_id: None,
            model: None,
            num_turns: 0,
            duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
            usage: RunUsage::default(),
            permission_denials: &[],
        }))));
    }
    out
}

/// What a toast becomes in a headless run (`docs/cli/headless.md` *Output*):
/// an error is said on stderr always; anything else is the step log's, said
/// only with `--verbose`.
#[must_use]
pub fn toast_line(text: &str, error: bool, verbose: bool) -> Option<Emit> {
    (error || verbose).then(|| warning(text))
}

/// A tool call a headless run refused because it needed approval nobody
/// could give (`docs/cli/headless.md` *Permissions*) — the result's
/// `permission_denials` entry, in Claude Code's fields: the tool as the model
/// called it, the call's id and its input. `agent` adds the subagent type
/// that asked; the lead's own calls carry none.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Denial {
    pub tool_name: String,
    pub tool_use_id: String,
    /// The call's arguments, one compact JSON object — `{}` when what the
    /// model sent is not one.
    #[serde(serialize_with = "raw_json")]
    pub tool_input: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// What the closing warning calls it: `write`, `edit`, `bash`, `mcp` or
    /// `session`.
    #[serde(skip)]
    pub kind: &'static str,
    /// The file, the command, the MCP tool, the session.
    #[serde(skip)]
    pub target: String,
}

impl From<&crate::permission::UnattendedRefusal> for Denial {
    fn from(refusal: &crate::permission::UnattendedRefusal) -> Self {
        use crate::permission::PermissionKind;
        let request = &refusal.request;
        Self {
            tool_name: refusal.tool_name.clone(),
            tool_use_id: refusal.tool_use_id.clone(),
            tool_input: compact_json(&refusal.tool_input)
                .filter(|input| input.starts_with('{'))
                .unwrap_or_else(|| "{}".to_string()),
            agent: request.agent.clone(),
            kind: match request.kind {
                PermissionKind::Write => "write",
                PermissionKind::Edit => "edit",
                PermissionKind::Bash => "bash",
                PermissionKind::Mcp => "mcp",
                PermissionKind::Session => "session",
            },
            target: request.target.clone(),
        }
    }
}

/// Serialize `text` — JSON already, compacted by [`compact_json`] — as the
/// value it is rather than as a string of it.
fn raw_json<S: serde::Serializer>(text: &str, serializer: S) -> Result<S::Ok, S::Error> {
    use serde::Serialize;
    serde_json::value::RawValue::from_string(text.to_string())
        .map_err(serde::ser::Error::custom)?
        .serialize(serializer)
}

/// The provider's usage over the run — subagents included
/// ([`crate::context_usage::UsageLedger`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct RunUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
}

impl From<crate::context_usage::UsageLedger> for RunUsage {
    fn from(ledger: crate::context_usage::UsageLedger) -> Self {
        Self {
            input_tokens: ledger.input,
            output_tokens: ledger.output,
            cached_input_tokens: ledger.cached,
            cache_write_input_tokens: ledger.cache_write,
        }
    }
}

/// What the run tells the reporter once its prompt is submitted.
#[derive(Debug, Clone, Copy)]
pub struct RunStart<'a> {
    /// The app's version, for the log's header.
    pub version: &'a str,
    /// The conversation's id — what `--resume` takes.
    pub session_id: Option<&'a str>,
    pub model: &'a str,
    /// The working directory, absolute — the JSON's.
    pub cwd: &'a str,
    /// …and as the log names it, `~`-relative.
    pub cwd_display: &'a str,
    /// `None` when permissions are off altogether.
    pub permission_mode: Option<crate::permission::PermissionMode>,
    /// Whether the run continues a recorded conversation.
    pub resumed: bool,
}

/// What the reporter needs once the run is over.
#[derive(Debug, Clone, Copy)]
pub struct RunEnd<'a> {
    pub outcome: &'a RunOutcome,
    /// [`final_answer`] of the run's history.
    pub answer: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub model: &'a str,
    /// Model replies the `--max-turns` budget counted.
    pub num_turns: usize,
    pub duration: Duration,
    pub usage: RunUsage,
    pub denials: &'a [Denial],
    /// The background commands still running at the end, stopped with it.
    pub stopped_shells: &'a [String],
    /// The background subagents still running once the run had waited
    /// [`agent_wait`] on them — stopped, by their descriptions.
    pub stopped_agents: &'a [String],
}

/// Turns what a headless run does into the lines it writes, for one output
/// format (`docs/cli/headless.md` *Output*). Pure: the boundary feeds it the
/// events it sees and the history the session settles, and writes what it
/// hands back.
#[derive(Debug, Default)]
pub struct Reporter {
    format: OutputFormat,
    /// `--verbose`: the step log on stderr, in every format.
    verbose: bool,
    /// The next item's number — `item_N`, counted per process (codex's ids).
    next_item: u64,
    /// The lead's tool call whose start was announced and whose resolution
    /// has not settled yet — one at a time, since its calls run in turn.
    open_tool: Option<OpenTool>,
    /// The text log's answer candidate: the replies since the last thing
    /// that was not one. Printed as narration once something proves they
    /// were not the answer — never printed if they were (`final_answer`).
    held: Vec<String>,
    /// Whether something that can only trail an answer (the turn's summary,
    /// a notice) settled after `held` — a reply after it starts a new
    /// candidate, and proves the held one narration.
    held_closed: bool,
    /// The log lines of what settled after `held`, waiting with it so the
    /// log keeps the run's order.
    after: Vec<String>,
}

/// A started tool call, kept until its record settles.
#[derive(Debug)]
struct OpenTool {
    id: String,
    name: String,
    args: String,
    arguments: Option<String>,
}

/// The longest a log line's call summary runs, in columns, before `…`.
const LOG_ARGS_MAX_COLS: usize = 100;

/// The longest a log line's first line of output runs before `…`.
const LOG_LINE_MAX_COLS: usize = 120;

/// The longest a denial's target runs in the closing warning.
const LOG_TARGET_MAX_COLS: usize = 60;

/// How many denials the closing warning names before `…`.
const LOG_DENIALS_NAMED: usize = 3;

/// What the stream says of a tool call's output once the run stopped
/// under it.
const INTERRUPTED_OUTPUT: &str = "Interrupted";

impl Reporter {
    /// A reporter writing `format`; `verbose` logs each step of the run to
    /// stderr (`--verbose`) — without it stderr says only what went wrong or
    /// needs attention.
    #[must_use]
    pub fn new(format: OutputFormat, verbose: bool) -> Self {
        Self {
            format,
            verbose,
            ..Self::default()
        }
    }

    /// The run began: the log's header, the stream's `session.started`.
    pub fn start(&mut self, start: &RunStart<'_>) -> Vec<Emit> {
        let mut out = Vec::new();
        if self.verbose {
            out.push(Emit::Stderr(header_line(start)));
        }
        if self.format == OutputFormat::StreamJson {
            out.push(Emit::Stdout(event_line(&Event::SessionStarted {
                session_id: start.session_id,
                model: start.model,
                cwd: start.cwd,
                permission_mode: start
                    .permission_mode
                    .map(crate::permission::PermissionMode::label),
                resumed: start.resumed,
            })));
        }
        out
    }

    /// A tool call of the lead's started (its `ToolStart`): the log names
    /// it, the stream opens its item.
    pub fn tool_started(&mut self, name: &str, args: &str, arguments: Option<&str>) -> Vec<Emit> {
        let id = self.next_id();
        let open = OpenTool {
            id,
            name: name.to_string(),
            args: args.to_string(),
            arguments: arguments.map(str::to_string),
        };
        let mut out = Vec::new();
        if self.verbose {
            // A call follows: whatever reply was held was narration.
            out.extend(self.flush_held());
            out.push(Emit::Stderr(tool_header(name, args)));
        }
        if self.format == OutputFormat::StreamJson {
            out.push(Emit::Stdout(event_line(&Event::ItemStarted {
                item: ItemEnvelope {
                    id: &open.id,
                    body: ItemBody::ToolCall {
                        name: wire_name(&open.name),
                        summary: &open.args,
                        arguments: open.arguments.as_deref().and_then(raw_arguments),
                        status: "in_progress",
                        output: None,
                        note: None,
                    },
                },
            })));
        }
        self.open_tool = Some(open);
        out
    }

    /// History the session has settled since the last call — nothing it can
    /// still take back ([`crate::app::App::settled_history_len`]).
    pub fn settled(&mut self, items: &[HistoryItem]) -> Vec<Emit> {
        let mut out = Vec::new();
        for item in items {
            // The lead's call this item resolves, when its start was
            // announced — the log named it then, the stream opened its item.
            let announced = matches!(item, HistoryItem::Tool(_))
                .then(|| self.open_tool.take())
                .flatten();
            if self.verbose {
                self.log_item(item, announced.is_some(), &mut out);
            }
            if self.format == OutputFormat::StreamJson {
                self.stream_item(item, announced.map(|open| open.id), &mut out);
            }
        }
        out
    }

    /// A request failed and is being sent again — the step log's news: a
    /// retry that works needs nothing from anyone, and one that does not
    /// ends the run with its error.
    pub fn retrying(&mut self, attempt: u32, max: u32) -> Vec<Emit> {
        if !self.verbose {
            return Vec::new();
        }
        vec![warning(&format!(
            "the request failed — retrying ({attempt}/{max})"
        ))]
    }

    /// No connection to the provider: said once per outage, on its first
    /// failed attempt.
    pub fn offline(&mut self, host: &str, attempts: u32) -> Vec<Emit> {
        if attempts == 1 {
            vec![warning(&format!(
                "no connection to {host} — waiting for it"
            ))]
        } else {
            Vec::new()
        }
    }

    /// The run is over: the answer or the result, and what it left behind.
    pub fn finish(&mut self, end: &RunEnd<'_>) -> Vec<Emit> {
        let mut out = Vec::new();
        let open = self.open_tool.take();
        let success = *end.outcome == RunOutcome::Success;
        if self.verbose {
            if open.is_some() {
                out.push(Emit::Stderr(format!(
                    "  ⎿ {}",
                    INTERRUPTED_OUTPUT.to_lowercase()
                )));
            }
            // A run that did not succeed prints no answer, so what the log
            // held back is narration after all.
            if success {
                out.extend(self.after.drain(..).map(Emit::Stderr));
                self.held.clear();
            } else {
                out.extend(self.flush_held());
            }
        }
        out.extend(left_behind(end));
        if self.verbose {
            out.push(Emit::Stderr(footer_line(end)));
        } else if let Some(error) = end.outcome.error() {
            // Quiet, a run that went wrong still says what went wrong.
            out.push(warning(error));
        }
        match self.format {
            OutputFormat::Text => {
                if success && let Some(answer) = end.answer {
                    out.push(Emit::Stdout(answer.to_string()));
                }
            }
            OutputFormat::Json => out.push(Emit::Stdout(result_line(end))),
            OutputFormat::StreamJson => {
                if let Some(open) = open {
                    out.push(Emit::Stdout(event_line(&Event::ItemCompleted {
                        item: ItemEnvelope {
                            id: &open.id,
                            body: ItemBody::ToolCall {
                                name: wire_name(&open.name),
                                summary: &open.args,
                                arguments: open.arguments.as_deref().and_then(raw_arguments),
                                status: "failed",
                                output: Some(INTERRUPTED_OUTPUT),
                                note: None,
                            },
                        },
                    })));
                }
                out.push(Emit::Stdout(result_line(end)));
            }
        }
        out
    }

    fn next_id(&mut self) -> String {
        let id = format!("item_{}", self.next_item);
        self.next_item += 1;
        id
    }

    /// The held replies as narration, then what settled after them — the
    /// log's order — and a fresh candidate.
    fn flush_held(&mut self) -> Vec<Emit> {
        self.held_closed = false;
        self.held
            .drain(..)
            .chain(self.after.drain(..))
            .map(Emit::Stderr)
            .collect()
    }

    /// One settled item into the step log; `announced`: the call it
    /// resolves was named when it started.
    fn log_item(&mut self, item: &HistoryItem, announced: bool, out: &mut Vec<Emit>) {
        match item {
            HistoryItem::Message(message) if message.role == Role::Assistant => {
                let text = message.text.trim();
                if text.is_empty() {
                    return;
                }
                if self.held_closed {
                    out.extend(self.flush_held());
                }
                self.held.push(text.to_string());
            }
            item if within_an_answer(item) => {}
            item if trails_an_answer(item) => {
                let line = trailing_line(item);
                if self.held.is_empty() {
                    out.extend(line.map(Emit::Stderr));
                } else {
                    self.held_closed = true;
                    self.after.extend(line);
                }
            }
            item => {
                out.extend(self.flush_held());
                out.extend(
                    Self::item_lines(item, announced)
                        .into_iter()
                        .map(Emit::Stderr),
                );
            }
        }
    }

    /// The log lines of an item that is no reply and no trailing one.
    fn item_lines(item: &HistoryItem, announced: bool) -> Vec<String> {
        match item {
            HistoryItem::Tool(call) => {
                let mut lines = Vec::new();
                if !announced {
                    lines.push(tool_header(&call.name, &call.args));
                }
                lines.push(tool_result_line(call));
                lines
            }
            HistoryItem::TaskCall(record) => {
                let status = if record.ok {
                    first_line(&record.output).unwrap_or_default()
                } else {
                    format!("failed: {}", first_line(&record.output).unwrap_or_default())
                };
                vec![
                    tool_header(&record.name, &record.args),
                    format!("  ⎿ {status}"),
                ]
            }
            HistoryItem::AgentGroup(group) => group
                .agents
                .iter()
                .flat_map(|agent| {
                    [
                        format!("● Agent({})", cut(&agent.description, LOG_ARGS_MAX_COLS)),
                        format!("  ⎿ {}", agent_status_text(agent)),
                    ]
                })
                .collect(),
            HistoryItem::HookNote(note) => vec![format!("● {}", note.label)],
            HistoryItem::Message(message) => match message.role {
                Role::Error => vec![format!("Error: {}", message.text.trim())],
                Role::System => vec![message.text.trim().to_string()],
                Role::User | Role::Shell | Role::Assistant => Vec::new(),
            },
            HistoryItem::Summary(_)
            | HistoryItem::Background(_)
            | HistoryItem::AgentNotice(_)
            | HistoryItem::Reasoning(_)
            | HistoryItem::Compaction(_) => trailing_line(item).into_iter().collect(),
        }
    }

    /// One settled item into the event stream; `announced`: the id of the
    /// item the call's start opened.
    fn stream_item(&mut self, item: &HistoryItem, announced: Option<String>, out: &mut Vec<Emit>) {
        let event = |body: ItemBody<'_>, id: &str| {
            event_line(&Event::ItemCompleted {
                item: ItemEnvelope { id, body },
            })
        };
        match item {
            HistoryItem::Summary(summary) => {
                out.push(Emit::Stdout(event_line(&Event::TurnCompleted {
                    duration_ms: summary.secs.saturating_mul(1000),
                    usage: TurnUsage {
                        total_tokens: summary.tokens as u64,
                        cached_input_tokens: summary.cached as u64,
                        cache_write_input_tokens: summary.cache_write as u64,
                    },
                })))
            }
            HistoryItem::Message(message) if message.role == Role::Error => {
                out.push(Emit::Stdout(event_line(&Event::TurnFailed {
                    error: ErrorBody {
                        message: &message.text,
                    },
                })));
            }
            HistoryItem::Message(message) if message.role == Role::Shell => {}
            HistoryItem::Tool(call) => {
                let id = match announced {
                    Some(id) => id,
                    None => self.next_id(),
                };
                let arguments = call.arguments.as_deref().and_then(raw_arguments);
                out.push(Emit::Stdout(event(
                    ItemBody::ToolCall {
                        name: wire_name(&call.name),
                        summary: &call.args,
                        arguments,
                        status: tool_status_label(call),
                        output: Some(call.context_text()),
                        note: call.approval_note.as_deref(),
                    },
                    &id,
                )));
            }
            item => {
                if let Some(body) = item_body(item) {
                    let id = self.next_id();
                    out.push(Emit::Stdout(event(body, &id)));
                }
            }
        }
    }
}

/// The name a tool call goes by in the stream — the one the model called,
/// recovered from the screen's label the session records it under.
fn wire_name(label: &str) -> std::borrow::Cow<'static, str> {
    std::borrow::Cow::Owned(crate::context::wire_tool_name(label))
}

/// The stream item for everything but a tool call, a turn's end and a `!`
/// command (`None`) — those take [`Reporter::stream_item`]'s own arms.
fn item_body(item: &HistoryItem) -> Option<ItemBody<'_>> {
    Some(match item {
        HistoryItem::Message(message) => match message.role {
            Role::User => ItemBody::UserMessage {
                text: &message.text,
            },
            Role::Assistant => ItemBody::AgentMessage {
                text: &message.text,
            },
            Role::System => ItemBody::Notice {
                text: &message.text,
            },
            Role::Error | Role::Shell => return None,
        },
        HistoryItem::Reasoning(reasoning) => ItemBody::Reasoning {
            text: &reasoning.text,
        },
        HistoryItem::TaskCall(record) => ItemBody::ToolCall {
            name: wire_name(&record.name),
            summary: &record.args,
            arguments: raw_arguments(&record.arguments),
            status: if record.ok { "completed" } else { "failed" },
            output: Some(&record.output),
            note: None,
        },
        HistoryItem::AgentGroup(group) => ItemBody::Subagents {
            background: group.background,
            agents: group
                .agents
                .iter()
                .map(|agent| SubagentItem {
                    id: &agent.id,
                    agent_type: &agent.agent_type,
                    description: &agent.description,
                    status: agent_status_label(agent.status),
                    result: &agent.result,
                    tool_uses: agent.tool_uses,
                    duration_ms: agent.secs.saturating_mul(1000),
                })
                .collect(),
        },
        HistoryItem::Background(notice) => ItemBody::BackgroundNotice {
            shell_id: &notice.id,
            description: &notice.description,
            exit_code: notice.code,
            killed: notice.killed,
            waiting: notice.waiting,
        },
        HistoryItem::AgentNotice(notice) => ItemBody::AgentNotice {
            agent_id: &notice.id,
            description: &notice.description,
            status: agent_status_label(notice.status),
            result: &notice.result,
        },
        HistoryItem::Compaction(compaction) => ItemBody::Compaction {
            auto: compaction.auto,
            before: compaction.before,
            after: compaction.after,
            summary: &compaction.summary,
        },
        HistoryItem::HookNote(note) => ItemBody::Hook {
            label: &note.label,
            text: &note.text,
        },
        HistoryItem::Tool(_) | HistoryItem::Summary(_) => return None,
    })
}

/// A tool call's `status`: `declined` for one that never ran — refused at
/// the gate, by a hook or the classifier, its record carrying the text the
/// model read instead of an output.
fn tool_status_label(call: &crate::app::ToolCall) -> &'static str {
    use crate::app::ToolStatus;
    match call.status {
        ToolStatus::Failed if call.context_output.is_some() => "declined",
        ToolStatus::Failed => "failed",
        ToolStatus::Backgrounded => "background",
        ToolStatus::Ok | ToolStatus::Running | ToolStatus::Waiting => "completed",
    }
}

fn agent_status_label(status: crate::agents::AgentStatus) -> &'static str {
    use crate::agents::AgentStatus;
    match status {
        AgentStatus::Pending => "pending",
        AgentStatus::Running => "running",
        AgentStatus::Done => "completed",
        AgentStatus::Failed => "failed",
        AgentStatus::Interrupted => "interrupted",
    }
}

/// The log's `⎿` text for one subagent of a resolved group.
fn agent_status_text(agent: &crate::app::AgentGroupEntry) -> String {
    use crate::agents::AgentStatus;
    match agent.status {
        AgentStatus::Done => format!(
            "done · {} tool use{} · {}",
            agent.tool_uses,
            if agent.tool_uses == 1 { "" } else { "s" },
            crate::app::format_elapsed(agent.secs)
        ),
        AgentStatus::Failed => match first_line(&agent.result) {
            Some(line) => format!("failed: {line}"),
            None => "failed".to_string(),
        },
        AgentStatus::Interrupted => "interrupted".to_string(),
        AgentStatus::Pending | AgentStatus::Running => "running in the background".to_string(),
    }
}

/// The log line of something that can trail an answer — `None` for the
/// ones the log leaves out (a turn's summary, a thought, a hook's cell).
fn trailing_line(item: &HistoryItem) -> Option<String> {
    match item {
        HistoryItem::Background(notice) => {
            let outcome = if notice.waiting {
                "is waiting for input".to_string()
            } else if notice.killed {
                "was stopped".to_string()
            } else {
                match notice.code {
                    Some(0) => "completed".to_string(),
                    Some(code) => format!("failed with exit {code}"),
                    None => "ended by a signal".to_string(),
                }
            };
            Some(format!(
                "● Background command \"{}\" {outcome}",
                cut(&notice.description, LOG_ARGS_MAX_COLS)
            ))
        }
        HistoryItem::AgentNotice(notice) => {
            use crate::agents::AgentStatus;
            let outcome = if notice.resumed {
                "was resumed by the user".to_string()
            } else {
                let ended = match notice.status {
                    AgentStatus::Done => "finished",
                    AgentStatus::Failed => "failed",
                    AgentStatus::Interrupted => "was stopped",
                    AgentStatus::Pending | AgentStatus::Running => "is running",
                };
                format!("{ended} · {}", crate::app::format_elapsed(notice.secs))
            };
            Some(format!(
                "● Agent \"{}\" {outcome}",
                cut(&notice.description, LOG_ARGS_MAX_COLS)
            ))
        }
        HistoryItem::Compaction(compaction) => Some(format!(
            "● Context compacted · {} → {} tokens",
            crate::ui::format_token_count(usize::try_from(compaction.before).unwrap_or(usize::MAX)),
            crate::ui::format_token_count(usize::try_from(compaction.after).unwrap_or(usize::MAX)),
        )),
        _ => None,
    }
}

/// The log's first line: what is running, where, on what.
fn header_line(start: &RunStart<'_>) -> String {
    let mut line = format!(
        "alter-zero {} · {} · {}",
        start.version, start.model, start.cwd_display
    );
    if let Some(mode) = start.permission_mode {
        line.push_str(" · ");
        line.push_str(mode.label());
    }
    if let Some(id) = start.session_id {
        line.push_str(" · session ");
        line.push_str(id);
    }
    line
}

/// `● Bash(cargo test)` — the TUI cell's header, one line, cut to fit a log.
fn tool_header(name: &str, args: &str) -> String {
    let mut lines = args.lines();
    let first = lines.next().unwrap_or_default();
    let summary = if lines.next().is_some() {
        format!("{}…", cut(first, LOG_ARGS_MAX_COLS))
    } else {
        cut(first, LOG_ARGS_MAX_COLS)
    };
    format!("● {}({summary})", crate::llm::tools::display_name(name))
}

/// `  ⎿ exit 0 · 238 lines` — what became of a call, in one line.
fn tool_result_line(call: &crate::app::ToolCall) -> String {
    use crate::app::ToolStatus;
    let text = match call.status {
        ToolStatus::Backgrounded => "running in the background".to_string(),
        _ if call.status == ToolStatus::Failed && call.context_output.is_some() => {
            first_line(&call.output).unwrap_or_else(|| "not run".to_string())
        }
        // A file read's output is the numbered file: say how much it read,
        // the cell's own synthesized corner (an image read says itself).
        ToolStatus::Ok
            if call.name.eq_ignore_ascii_case("read") && !call.output.starts_with("Read ") =>
        {
            match call.output.lines().count() {
                1 => "read 1 line".to_string(),
                lines => format!("read {lines} lines"),
            }
        }
        _ => match exit_frame(&call.output) {
            Some((code, 0)) => format!("exit {code} · no output"),
            Some((code, 1)) => format!("exit {code} · 1 line"),
            Some((code, lines)) => format!("exit {code} · {lines} lines"),
            None => {
                let first = first_line(&call.output).unwrap_or_else(|| "(no output)".to_string());
                if call.status == ToolStatus::Failed {
                    format!("failed: {first}")
                } else {
                    first
                }
            }
        },
    };
    format!("  ⎿ {text}")
}

/// A command's `Exit code: N` frame (`llm::exec`) and how many lines of
/// output follow it.
fn exit_frame(output: &str) -> Option<(i64, usize)> {
    let (first, rest) = output.split_once('\n').unwrap_or((output, ""));
    let code = first.strip_prefix("Exit code: ")?.trim().parse().ok()?;
    Some((code, rest.lines().count()))
}

/// The first line of `text` with anything on it, trimmed and cut to fit.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| cut(line, LOG_LINE_MAX_COLS))
}

/// Display width of `s` in terminal columns — all width math here goes
/// through this, never `chars().count()`, so CJK and emoji measure true.
fn cols(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    s.width()
}

/// `text` cut to `max` display columns, `…` (one column) closing a cut.
fn cut(text: &str, max: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if cols(text) <= max {
        return text.to_string();
    }
    let mut used = 0;
    let mut out = String::new();
    for ch in text.chars() {
        let width = ch.width().unwrap_or(0);
        if used + width + 1 > max {
            break;
        }
        used += width;
        out.push(ch);
    }
    out.push('…');
    out
}

/// The warnings a run's end owes stderr in every format: the calls nobody
/// could approve, the background commands it stopped.
fn left_behind(end: &RunEnd<'_>) -> Vec<Emit> {
    let mut out = Vec::new();
    if !end.denials.is_empty() {
        let mut named: Vec<String> = end
            .denials
            .iter()
            .take(LOG_DENIALS_NAMED)
            .map(|denial| {
                format!(
                    "{} {}",
                    denial.kind,
                    cut(&denial.target, LOG_TARGET_MAX_COLS)
                )
            })
            .collect();
        if end.denials.len() > LOG_DENIALS_NAMED {
            named.push("…".to_string());
        }
        let n = end.denials.len();
        out.push(warning(&format!(
            "{n} tool call{} needed approval and did not run: {} (--permission-mode decides what runs unasked)",
            if n == 1 { "" } else { "s" },
            named.join(", ")
        )));
    }
    if !end.stopped_shells.is_empty() {
        let n = end.stopped_shells.len();
        out.push(warning(&format!(
            "stopped {n} background command{} still running: {}",
            if n == 1 { "" } else { "s" },
            end.stopped_shells
                .iter()
                .map(|command| cut(command, LOG_TARGET_MAX_COLS))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if !end.stopped_agents.is_empty() {
        let n = end.stopped_agents.len();
        out.push(warning(&format!(
            "stopped {n} background agent{} still running: {} ({AGENT_WAIT_ENV} sets how long \
             a run waits for them)",
            if n == 1 { "" } else { "s" },
            end.stopped_agents
                .iter()
                .map(|description| cut(description, LOG_TARGET_MAX_COLS))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    out
}

/// The log's last line: how the run ended, how long it took, what it cost.
fn footer_line(end: &RunEnd<'_>) -> String {
    let verb = match end.outcome {
        RunOutcome::Success => "Done in",
        RunOutcome::Failed(_) => "Failed after",
        RunOutcome::MaxTurns(_) => "Stopped by --max-turns after",
        RunOutcome::Interrupted(_) => "Interrupted after",
    };
    let mut line = format!(
        "{verb} {}",
        crate::app::format_elapsed(end.duration.as_secs())
    );
    if end.num_turns > 0 {
        line.push_str(&format!(
            " · {} model repl{}",
            end.num_turns,
            if end.num_turns == 1 { "y" } else { "ies" }
        ));
    }
    let total = end
        .usage
        .input_tokens
        .saturating_add(end.usage.output_tokens);
    if total > 0 {
        let count =
            |n: u64| crate::ui::format_token_count(usize::try_from(n).unwrap_or(usize::MAX));
        line.push_str(&format!(" · {} tokens", count(total)));
        if end.usage.cached_input_tokens > 0 {
            line.push_str(&format!(
                " ({} cached)",
                count(end.usage.cached_input_tokens)
            ));
        }
    }
    line
}

/// The `result` object — the `json` format's whole output and the stream's
/// last line.
fn result_line(end: &RunEnd<'_>) -> String {
    let answer = match end.outcome {
        RunOutcome::Success => end.answer.unwrap_or_default(),
        _ => "",
    };
    event_line(&Event::Result(ResultBody {
        subtype: end.outcome.subtype(),
        is_error: *end.outcome != RunOutcome::Success,
        result: answer,
        error: end.outcome.error(),
        session_id: end.session_id,
        model: Some(end.model),
        num_turns: end.num_turns,
        duration_ms: u64::try_from(end.duration.as_millis()).unwrap_or(u64::MAX),
        usage: end.usage,
        permission_denials: end.denials,
    }))
}

/// One event as its JSON line. Serializing these shapes cannot fail; a
/// failure would still be a line, codex's rule.
fn event_line(event: &Event<'_>) -> String {
    serde_json::to_string(event).unwrap_or_else(|err| {
        format!("{{\"type\":\"error\",\"message\":\"failed to serialize an event: {err}\"}}")
    })
}

/// A call's verbatim arguments as an embeddable JSON value, compacted onto
/// one line ([`compact_json`]); `None` when they are not JSON.
fn raw_arguments(arguments: &str) -> Option<Box<serde_json::value::RawValue>> {
    compact_json(arguments)
        .and_then(|compact| serde_json::value::RawValue::from_string(compact).ok())
}

/// Compact a JSON document onto one line — the whitespace between its
/// tokens dropped, every string byte for byte — so a tool call's arguments
/// embed in a one-line event whatever the model sent. `None` when `raw` is
/// not JSON.
#[must_use]
pub fn compact_json(raw: &str) -> Option<String> {
    // Validated without building a tree (`docs/architecture/memory.md`): a borrowed raw
    // value is a scan of the text.
    serde_json::from_str::<&serde_json::value::RawValue>(raw).ok()?;
    let mut out = String::with_capacity(raw.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in raw.chars() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
            out.push(ch);
        } else if !ch.is_whitespace() {
            out.push(ch);
        }
    }
    Some(out)
}

// ----- the JSON shapes -----

#[derive(serde::Serialize)]
#[serde(tag = "type")]
enum Event<'a> {
    #[serde(rename = "session.started")]
    SessionStarted {
        session_id: Option<&'a str>,
        model: &'a str,
        cwd: &'a str,
        permission_mode: Option<&'static str>,
        resumed: bool,
    },
    #[serde(rename = "item.started")]
    ItemStarted { item: ItemEnvelope<'a> },
    #[serde(rename = "item.completed")]
    ItemCompleted { item: ItemEnvelope<'a> },
    #[serde(rename = "turn.completed")]
    TurnCompleted { duration_ms: u64, usage: TurnUsage },
    #[serde(rename = "turn.failed")]
    TurnFailed { error: ErrorBody<'a> },
    #[serde(rename = "result")]
    Result(ResultBody<'a>),
}

#[derive(serde::Serialize)]
struct ItemEnvelope<'a> {
    id: &'a str,
    #[serde(flatten)]
    body: ItemBody<'a>,
}

#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ItemBody<'a> {
    UserMessage {
        text: &'a str,
    },
    AgentMessage {
        text: &'a str,
    },
    Reasoning {
        text: &'a str,
    },
    ToolCall {
        /// The tool as the model called it (`bash`, `mcp__github__create_issue`)
        /// — never the screen's label.
        name: std::borrow::Cow<'a, str>,
        summary: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        arguments: Option<Box<serde_json::value::RawValue>>,
        status: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<&'a str>,
    },
    Subagents {
        background: bool,
        agents: Vec<SubagentItem<'a>>,
    },
    // Never a field named `id`: the envelope's flattened `id` is the
    // item's, and a second one is a duplicate key (codex's `web_search`).
    BackgroundNotice {
        shell_id: &'a str,
        description: &'a str,
        exit_code: Option<i32>,
        killed: bool,
        waiting: bool,
    },
    AgentNotice {
        agent_id: &'a str,
        description: &'a str,
        status: &'static str,
        result: &'a str,
    },
    Compaction {
        auto: bool,
        before: u64,
        after: u64,
        summary: &'a str,
    },
    Hook {
        label: &'a str,
        text: &'a str,
    },
    Notice {
        text: &'a str,
    },
}

#[derive(serde::Serialize)]
struct SubagentItem<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    agent_type: &'a str,
    description: &'a str,
    status: &'static str,
    result: &'a str,
    tool_uses: usize,
    duration_ms: u64,
}

#[derive(serde::Serialize)]
struct TurnUsage {
    total_tokens: u64,
    cached_input_tokens: u64,
    cache_write_input_tokens: u64,
}

#[derive(serde::Serialize)]
struct ErrorBody<'a> {
    message: &'a str,
}

#[derive(serde::Serialize)]
struct ResultBody<'a> {
    subtype: &'static str,
    is_error: bool,
    result: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    session_id: Option<&'a str>,
    /// `null` for a run that never started — it may have had no model.
    model: Option<&'a str>,
    num_turns: usize,
    duration_ms: u64,
    usage: RunUsage,
    permission_denials: &'a [Denial],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_format_parses_back_from_its_own_label() {
        for format in OutputFormat::ALL {
            assert_eq!(OutputFormat::parse(format.label()), Some(format));
        }
    }

    #[test]
    fn a_format_label_is_exact() {
        // A typo is a usage error naming the choices, never a guess.
        for wrong in ["", "JSON", "jsonl", "stream_json", "streamjson", " text"] {
            assert_eq!(OutputFormat::parse(wrong), None, "{wrong:?}");
        }
    }

    // ===== the prompt =====

    #[test]
    fn a_terminal_is_never_read_unless_dash_asks_for_it() {
        assert_eq!(
            stdin_read(Some("fix it"), StdinKind::Terminal),
            StdinRead::Skip
        );
        assert_eq!(stdin_read(None, StdinKind::Terminal), StdinRead::Skip);
        // `-` is an explicit request: `cat`'s rule, typed until Ctrl+D.
        assert_eq!(stdin_read(Some("-"), StdinKind::Terminal), StdinRead::ToEnd);
    }

    #[test]
    fn a_file_is_read_whole_whatever_the_argument() {
        for arg in [Some("summarize"), None, Some("-")] {
            assert_eq!(
                stdin_read(arg, StdinKind::File),
                StdinRead::ToEnd,
                "{arg:?}"
            );
        }
    }

    #[test]
    fn a_pipe_is_read_to_its_end_whatever_the_argument() {
        // A shell `|` ends when its writer does: however slow the command
        // before it, waiting loses nothing — and a pipe still open after a
        // while is said on stderr.
        for arg in [Some("summarize this"), None, Some("-")] {
            assert_eq!(
                stdin_read(arg, StdinKind::Pipe),
                StdinRead::ToEndNoting(STDIN_NOTE_AFTER),
                "{arg:?}"
            );
        }
    }

    #[test]
    fn a_stream_beside_an_argument_is_read_only_if_it_says_something_in_time() {
        // A socket (what Node gives the commands it starts — n8n's) may be
        // held open by a parent that never writes to it.
        assert_eq!(
            stdin_read(Some("review this diff"), StdinKind::Stream),
            StdinRead::IfActiveWithin(STDIN_WAIT)
        );
        // With no argument the stream IS the prompt: read to its end.
        for arg in [None, Some("-")] {
            assert_eq!(
                stdin_read(arg, StdinKind::Stream),
                StdinRead::ToEndNoting(STDIN_NOTE_AFTER),
                "{arg:?}"
            );
        }
    }

    #[test]
    fn stdin_decodes_as_utf8_without_its_byte_order_mark() {
        let plain = decode_stdin(b"plain\n".to_vec());
        assert_eq!((plain.text.as_str(), plain.replaced), ("plain\n", false));
        let bom = decode_stdin("\u{feff}caf\u{e9}".as_bytes().to_vec());
        assert_eq!((bom.text.as_str(), bom.replaced), ("caf\u{e9}", false));
    }

    #[test]
    fn stdin_that_is_not_all_utf8_keeps_what_is_and_says_so() {
        // A log with one stray byte is still worth reading.
        let mixed = decode_stdin(b"\xef\xbb\xbfok\xffno\n".to_vec());
        assert_eq!(mixed.text, "ok\u{fffd}no\n");
        assert!(mixed.replaced);
    }

    #[test]
    fn the_argument_alone_is_the_prompt() {
        assert_eq!(
            combine_prompt(Some("fix it"), None).as_deref(),
            Some("fix it")
        );
        // Blank input adds nothing.
        assert_eq!(
            combine_prompt(Some("fix it"), Some(" \n\t\n")).as_deref(),
            Some("fix it")
        );
    }

    #[test]
    fn stdin_beside_an_argument_rides_in_a_tagged_block() {
        assert_eq!(
            combine_prompt(Some("Review this diff"), Some("+a\n-b\n")).as_deref(),
            Some("Review this diff\n\n<stdin>\n+a\n-b\n</stdin>")
        );
        // Input without a final newline still closes on its own line.
        assert_eq!(
            combine_prompt(Some("Explain"), Some("x = 1")).as_deref(),
            Some("Explain\n\n<stdin>\nx = 1\n</stdin>")
        );
    }

    #[test]
    fn without_an_argument_stdin_is_the_prompt() {
        assert_eq!(
            combine_prompt(None, Some("Run the tests\n")).as_deref(),
            Some("Run the tests")
        );
        assert_eq!(
            combine_prompt(Some(STDIN_ARG), Some("Run the tests")).as_deref(),
            Some("Run the tests")
        );
    }

    #[test]
    fn nothing_to_answer_is_none() {
        assert_eq!(combine_prompt(None, None), None);
        assert_eq!(combine_prompt(None, Some("  \n")), None);
        assert_eq!(combine_prompt(Some(STDIN_ARG), None), None);
        assert_eq!(combine_prompt(Some(STDIN_ARG), Some("")), None);
    }

    // ===== the answer and the outcome =====

    use crate::app::{
        BackgroundNotice, Compaction, HookNote, Message, Reasoning, ToolCall, ToolStatus,
        TurnSummary,
    };

    fn message(role: Role, text: &str) -> HistoryItem {
        HistoryItem::Message(Message {
            role,
            text: text.to_string(),
            timestamp: String::new(),
            images: Vec::new(),
        })
    }

    fn user(text: &str) -> HistoryItem {
        message(Role::User, text)
    }

    fn reply(text: &str) -> HistoryItem {
        message(Role::Assistant, text)
    }

    fn failure(text: &str) -> HistoryItem {
        message(Role::Error, text)
    }

    fn tool(name: &str, status: ToolStatus, output: &str) -> HistoryItem {
        HistoryItem::Tool(ToolCall {
            name: name.to_string(),
            args: "cargo test".to_string(),
            status,
            output: output.to_string(),
            timestamp: String::new(),
            shell: false,
            truncated: false,
            context_output: None,
            approval_note: None,
            arguments: None,
            batch: None,
            call_id: None,
            position: None,
            run: crate::stream::ToolRun::default(),
        })
    }

    fn summary(secs: u64) -> HistoryItem {
        HistoryItem::Summary(TurnSummary {
            verb: "Worked",
            secs,
            timestamp: String::new(),
            shells: 0,
            tokens: 0,
            estimated: 0,
            cached: 0,
            cache_write: 0,
        })
    }

    fn thought() -> HistoryItem {
        HistoryItem::Reasoning(Reasoning {
            text: "think".to_string(),
            secs: 1,
            tokens: 3,
            timestamp: String::new(),
        })
    }

    fn hook(text: &str) -> HistoryItem {
        HistoryItem::HookNote(HookNote {
            label: "Stop hook".to_string(),
            text: text.to_string(),
            report: None,
            timestamp: String::new(),
        })
    }

    fn shell_notice() -> HistoryItem {
        HistoryItem::Background(BackgroundNotice {
            description: "npm test".to_string(),
            id: "b1".to_string(),
            code: Some(0),
            killed: false,
            output_tail: String::new(),
            origin: None,
            waiting: false,
            timestamp: String::new(),
        })
    }

    fn compaction() -> HistoryItem {
        HistoryItem::Compaction(Compaction {
            summary: "so far".to_string(),
            timestamp: String::new(),
            before: 9000,
            after: 900,
            auto: true,
            secs: 4,
        })
    }

    #[test]
    fn the_answer_is_the_last_reply() {
        let run = [user("fix it"), reply("Fixed."), summary(3)];
        assert_eq!(final_answer(&run).as_deref(), Some("Fixed."));
    }

    #[test]
    fn narration_before_a_tool_call_is_not_the_answer() {
        let run = [
            user("fix it"),
            reply("Running the tests first."),
            tool("bash", ToolStatus::Ok, "Exit code: 0"),
            reply("All green."),
            summary(9),
        ];
        assert_eq!(final_answer(&run).as_deref(), Some("All green."));
    }

    #[test]
    fn a_reply_split_by_a_thought_is_one_answer() {
        let run = [
            user("q"),
            reply("First half."),
            thought(),
            reply("Second."),
            summary(2),
        ];
        assert_eq!(
            final_answer(&run).as_deref(),
            Some("First half.\n\nSecond.")
        );
    }

    #[test]
    fn what_settles_after_an_answer_is_looked_past() {
        let run = [
            user("q"),
            reply("Answer."),
            shell_notice(),
            hook(""),
            summary(2),
            compaction(),
        ];
        assert_eq!(final_answer(&run).as_deref(), Some("Answer."));
    }

    #[test]
    fn a_follow_up_turn_answers_for_the_run() {
        // A background completion started a second turn: its reply is the
        // answer, and the first turn's is narration — never joined.
        let run = [
            user("launch the critic"),
            reply("Launched; waiting."),
            shell_notice(),
            summary(2),
            reply("The critic found two bugs."),
            summary(5),
        ];
        assert_eq!(
            final_answer(&run).as_deref(),
            Some("The critic found two bugs.")
        );
    }

    #[test]
    fn a_stop_hook_continuation_answers_with_its_own_reply() {
        let run = [
            user("q"),
            reply("Done?"),
            hook("Stop hook feedback: run the tests"),
            reply("Tests pass."),
            summary(4),
        ];
        assert_eq!(final_answer(&run).as_deref(), Some("Tests pass."));
    }

    #[test]
    fn a_failed_turn_has_no_answer() {
        let run = [
            user("q"),
            reply("Partial"),
            failure("request failed: HTTP 500"),
        ];
        assert_eq!(final_answer(&run), None);
        assert_eq!(final_answer(&[user("q")]), None);
        assert_eq!(final_answer(&[]), None);
    }

    #[test]
    fn a_completed_last_turn_is_a_success() {
        let run = [user("q"), reply("A"), summary(1)];
        assert_eq!(RunOutcome::of(&run, false), RunOutcome::Success);
        // What trails a turn's end does not change how it ended.
        let run = [
            user("q"),
            reply("A"),
            summary(1),
            shell_notice(),
            compaction(),
        ];
        assert_eq!(RunOutcome::of(&run, false), RunOutcome::Success);
    }

    #[test]
    fn a_failed_last_turn_fails_the_run_with_its_message() {
        let run = [
            user("q"),
            reply("A"),
            summary(1),
            failure("request failed: HTTP 500"),
        ];
        assert_eq!(
            RunOutcome::of(&run, false),
            RunOutcome::Failed("request failed: HTTP 500".to_string())
        );
        // …and a spent budget names the limit as the cause.
        assert_eq!(
            RunOutcome::of(&run, true),
            RunOutcome::MaxTurns("request failed: HTTP 500".to_string())
        );
    }

    #[test]
    fn a_spent_budget_on_a_completed_run_is_still_a_success() {
        // The last reply was the budget's last: the model answered in time.
        let run = [user("q"), reply("A"), summary(1)];
        assert_eq!(RunOutcome::of(&run, true), RunOutcome::Success);
    }

    #[test]
    fn a_run_whose_turn_never_ended_failed() {
        let outcome = RunOutcome::of(&[user("q"), reply("A")], false);
        assert!(matches!(outcome, RunOutcome::Failed(_)), "{outcome:?}");
        assert!(outcome.error().is_some_and(|e| !e.is_empty()));
    }

    #[test]
    fn each_outcome_has_its_exit_status_and_subtype() {
        let cases = [
            (RunOutcome::Success, 0, "success", None),
            (
                RunOutcome::Failed("boom".into()),
                1,
                "error_during_execution",
                Some("boom"),
            ),
            (
                RunOutcome::MaxTurns("limit".into()),
                1,
                "error_max_turns",
                Some("limit"),
            ),
            (
                RunOutcome::Interrupted(2),
                130,
                "error_interrupted",
                Some("interrupted by SIGINT"),
            ),
            (
                RunOutcome::Interrupted(15),
                143,
                "error_interrupted",
                Some("interrupted by SIGTERM"),
            ),
        ];
        for (outcome, code, subtype, error) in cases {
            assert_eq!(outcome.exit_code(), code, "{outcome:?}");
            assert_eq!(outcome.subtype(), subtype, "{outcome:?}");
            assert_eq!(outcome.error(), error, "{outcome:?}");
        }
    }

    // ===== the reporter =====

    use crate::agents::AgentStatus;
    use crate::app::{AgentGroup, AgentGroupEntry, AgentNotice};
    use crate::permission::{PermissionKind, PermissionMode, PermissionRequest};

    fn start_info() -> RunStart<'static> {
        RunStart {
            version: "0.18.0",
            session_id: Some("18a9-1a2b"),
            model: "gpt-5.5",
            cwd: "/home/u/proj",
            cwd_display: "~/proj",
            permission_mode: Some(PermissionMode::Auto),
            resumed: false,
        }
    }

    fn end_info<'a>(outcome: &'a RunOutcome, answer: Option<&'a str>) -> RunEnd<'a> {
        RunEnd {
            outcome,
            answer,
            session_id: Some("18a9-1a2b"),
            model: "gpt-5.5",
            num_turns: 3,
            duration: Duration::from_secs(72),
            usage: RunUsage {
                input_tokens: 40_000,
                output_tokens: 2_000,
                cached_input_tokens: 31_000,
                cache_write_input_tokens: 0,
            },
            denials: &[],
            stopped_shells: &[],
            stopped_agents: &[],
        }
    }

    fn stderr(emits: &[Emit]) -> Vec<String> {
        emits
            .iter()
            .filter_map(|emit| match emit {
                Emit::Stderr(line) => Some(line.clone()),
                Emit::Stdout(_) => None,
            })
            .collect()
    }

    fn stdout(emits: &[Emit]) -> Vec<String> {
        emits
            .iter()
            .filter_map(|emit| match emit {
                Emit::Stdout(line) => Some(line.clone()),
                Emit::Stderr(_) => None,
            })
            .collect()
    }

    fn json(line: &str) -> serde_json::Value {
        serde_json::from_str(line).unwrap_or_else(|err| panic!("{err}: {line}"))
    }

    fn declined(name: &str, display: &str) -> HistoryItem {
        let HistoryItem::Tool(mut call) = tool(name, ToolStatus::Failed, display) else {
            unreachable!()
        };
        call.context_output = Some("The call was not run.".to_string());
        HistoryItem::Tool(call)
    }

    fn with_arguments(item: HistoryItem, arguments: &str) -> HistoryItem {
        let HistoryItem::Tool(mut call) = item else {
            unreachable!()
        };
        call.arguments = Some(arguments.to_string());
        HistoryItem::Tool(call)
    }

    #[test]
    fn a_warning_is_a_prefixed_stderr_line() {
        assert_eq!(
            warning("2 hooks wait for review"),
            Emit::Stderr("alter-zero: 2 hooks wait for review".to_string())
        );
    }

    #[test]
    fn a_text_run_logs_its_work_to_stderr_and_prints_only_the_answer() {
        let mut reporter = Reporter::new(OutputFormat::Text, true);
        let mut emits = reporter.start(&start_info());
        emits.extend(reporter.settled(&[user("fix it")]));
        // The narration settles as the tool call starts…
        emits.extend(reporter.settled(&[reply("Running the tests first.")]));
        emits.extend(reporter.tool_started(
            "bash",
            "cargo test",
            Some(r#"{"command":"cargo test"}"#),
        ));
        emits.extend(reporter.settled(&[tool(
            "bash",
            ToolStatus::Failed,
            "Exit code: 101\nfailed\nparse::empty\n",
        )]));
        emits.extend(reporter.settled(&[reply("Fixed the empty-input panic."), summary(72)]));
        let outcome = RunOutcome::Success;
        emits.extend(reporter.finish(&end_info(&outcome, Some("Fixed the empty-input panic."))));
        assert_eq!(
            stderr(&emits),
            vec![
                "alter-zero 0.18.0 · gpt-5.5 · ~/proj · auto · session 18a9-1a2b",
                "Running the tests first.",
                "● Bash(cargo test)",
                "  ⎿ exit 101 · 2 lines",
                "Done in 1m 12s · 3 model replies · 42k tokens (31k cached)",
            ]
        );
        assert_eq!(stdout(&emits), vec!["Fixed the empty-input panic."]);
    }

    #[test]
    fn a_tool_that_never_announced_its_start_is_logged_whole() {
        let mut reporter = Reporter::new(OutputFormat::Text, true);
        let emits = reporter.settled(&[tool("bash", ToolStatus::Ok, "Exit code: 0\nok")]);
        assert_eq!(
            stderr(&emits),
            vec!["● Bash(cargo test)", "  ⎿ exit 0 · 1 line"]
        );
    }

    #[test]
    fn a_file_read_logs_how_much_it_read() {
        // The read tool's output is the numbered file; its cell's corner —
        // `Read 12 lines` — is synthesized, and so is the log's.
        let mut reporter = Reporter::new(OutputFormat::Text, true);
        let emits = reporter.settled(&[
            tool("read", ToolStatus::Ok, "1 fn main() {\n2     run();\n3 }"),
            tool("Read", ToolStatus::Ok, "1 one line"),
            tool("read", ToolStatus::Ok, "Read image (png, 640x480, 12 KB)"),
        ]);
        assert_eq!(
            stderr(&emits),
            vec![
                "● Read(cargo test)",
                "  ⎿ read 3 lines",
                "● Read(cargo test)",
                "  ⎿ read 1 line",
                "● Read(cargo test)",
                "  ⎿ Read image (png, 640x480, 12 KB)",
            ]
        );
    }

    #[test]
    fn declined_backgrounded_and_silent_calls_read_as_such() {
        let mut reporter = Reporter::new(OutputFormat::Text, true);
        let emits = reporter.settled(&[
            declined("bash", "Not run: needs approval (headless run)"),
            tool(
                "bash",
                ToolStatus::Backgrounded,
                "Command running in background",
            ),
            tool("bash", ToolStatus::Ok, "Exit code: 0"),
            tool("edit", ToolStatus::Failed, "Error: old_string not found"),
        ]);
        assert_eq!(
            stderr(&emits),
            vec![
                "● Bash(cargo test)",
                "  ⎿ Not run: needs approval (headless run)",
                "● Bash(cargo test)",
                "  ⎿ running in the background",
                "● Bash(cargo test)",
                "  ⎿ exit 0 · no output",
                "● Edit(cargo test)",
                "  ⎿ failed: Error: old_string not found",
            ]
        );
    }

    #[test]
    fn a_long_or_multiline_header_is_cut_to_one_line() {
        let mut reporter = Reporter::new(OutputFormat::Text, true);
        let long = "x".repeat(300);
        let line = &stderr(&reporter.tool_started("bash", &long, None))[0];
        assert!(line.ends_with("…)") && line.chars().count() < 140, "{line}");
        let line = &stderr(&reporter.tool_started("bash", "cat <<EOF\nhi\nEOF", None))[0];
        assert_eq!(line, "● Bash(cat <<EOF…)");
    }

    #[test]
    fn a_notice_after_a_reply_keeps_its_place_once_the_reply_proves_narration() {
        let mut reporter = Reporter::new(OutputFormat::Text, true);
        let mut emits = reporter.settled(&[
            user("q"),
            reply("Waiting on the build."),
            shell_notice(),
            summary(2),
        ]);
        // Until something follows, that reply may be the answer: held.
        assert!(stderr(&emits).is_empty(), "{emits:?}");
        emits.extend(reporter.settled(&[reply("The build passed.")]));
        assert_eq!(
            stderr(&emits),
            vec![
                "Waiting on the build.",
                "● Background command \"npm test\" completed",
            ]
        );
        let outcome = RunOutcome::Success;
        let finished = reporter.finish(&end_info(&outcome, Some("The build passed.")));
        assert_eq!(stdout(&finished), vec!["The build passed."]);
    }

    #[test]
    fn a_failed_run_logs_its_error_and_prints_no_answer() {
        let mut reporter = Reporter::new(OutputFormat::Text, true);
        let mut emits = reporter.settled(&[
            user("q"),
            reply("Partial"),
            failure("request failed: HTTP 500"),
        ]);
        let outcome = RunOutcome::Failed("request failed: HTTP 500".into());
        emits.extend(reporter.finish(&end_info(&outcome, None)));
        assert_eq!(
            stderr(&emits),
            vec![
                "Partial",
                "Error: request failed: HTTP 500",
                "Failed after 1m 12s · 3 model replies · 42k tokens (31k cached)",
            ]
        );
        assert!(stdout(&emits).is_empty(), "{emits:?}");
    }

    #[test]
    fn the_footer_names_how_the_run_ended() {
        let footer = |outcome: RunOutcome, turns: usize, usage: RunUsage| {
            let mut end = end_info(&outcome, None);
            end.num_turns = turns;
            end.usage = usage;
            stderr(&Reporter::new(OutputFormat::Text, true).finish(&end))
                .last()
                .cloned()
                .unwrap()
        };
        assert_eq!(
            footer(RunOutcome::MaxTurns("limit".into()), 1, RunUsage::default()),
            "Stopped by --max-turns after 1m 12s · 1 model reply"
        );
        assert_eq!(
            footer(RunOutcome::Interrupted(2), 0, RunUsage::default()),
            "Interrupted after 1m 12s"
        );
    }

    /// The gate's record of a refused call, as the model's `call_7` made it —
    /// its input pretty-printed, as some models send arguments.
    fn refusal(
        kind: PermissionKind,
        target: &str,
        agent: Option<&str>,
    ) -> crate::permission::UnattendedRefusal {
        crate::permission::UnattendedRefusal {
            request: PermissionRequest {
                id: "p1".into(),
                kind,
                target: target.into(),
                body: String::new(),
                detail: None,
                agent: agent.map(Into::into),
                agent_id: agent.map(|_| "a1".into()),
            },
            tool_name: "bash".into(),
            tool_use_id: "call_7".into(),
            tool_input: format!("{{\n  \"command\": \"{target}\"\n}}"),
        }
    }

    #[test]
    fn a_tool_call_is_named_as_the_model_called_it() {
        // The live stream and the history carry the screen's label; the
        // stream names each tool the way the model called it.
        let label = crate::llm::tools::display_name;
        let mut reporter = Reporter::new(OutputFormat::StreamJson, false);
        let started = stdout(&reporter.tool_started(&label("bash"), "ls", None));
        assert_eq!(json(&started[0])["item"]["name"], "bash");
        let settled = stdout(&reporter.settled(&[tool(&label("bash"), ToolStatus::Ok, "ok")]));
        assert_eq!(json(&settled[0])["item"]["name"], "bash");
        // An MCP tool's label turns back into the name it was called by,
        // its case kept.
        let wire = "mcp__GitHub__create_issue";
        let mcp = stdout(&reporter.settled(&[tool(&label(wire), ToolStatus::Ok, "{}")]));
        assert_eq!(json(&mcp[0])["item"]["name"], wire);
        // A run stopped under an open call names it the same way.
        reporter.tool_started(&label("write"), "a.txt", None);
        let outcome = RunOutcome::Interrupted(15);
        let end = end_info(&outcome, None);
        let closed = stdout(&reporter.finish(&end));
        assert_eq!(json(&closed[0])["item"]["name"], "write");
    }

    #[test]
    fn what_a_run_left_behind_is_said_on_stderr_in_every_format() {
        let denials = [
            Denial::from(&refusal(PermissionKind::Bash, "rm -rf build", None)),
            Denial::from(&refusal(
                PermissionKind::Write,
                "/home/u/proj/a.rs",
                Some("general-purpose"),
            )),
        ];
        let shells = ["npm run dev".to_string()];
        let agents = ["Review the diff".to_string()];
        for format in OutputFormat::ALL {
            let outcome = RunOutcome::Success;
            let mut end = end_info(&outcome, Some("ok"));
            end.denials = &denials;
            end.stopped_shells = &shells;
            end.stopped_agents = &agents;
            let lines = stderr(&Reporter::new(format, false).finish(&end));
            assert!(
                lines.contains(
                    &"alter-zero: 2 tool calls needed approval and did not run: bash rm -rf build, \
                      write /home/u/proj/a.rs (--permission-mode decides what runs unasked)"
                        .to_string()
                ),
                "{format:?}: {lines:?}"
            );
            assert!(
                lines.contains(
                    &"alter-zero: stopped 1 background command still running: npm run dev"
                        .to_string()
                ),
                "{format:?}: {lines:?}"
            );
            assert!(
                lines.contains(
                    &"alter-zero: stopped 1 background agent still running: Review the diff \
                      (ALTER_ZERO_HEADLESS_AGENT_WAIT_SECS sets how long a run waits for them)"
                        .to_string()
                ),
                "{format:?}: {lines:?}"
            );
        }
    }

    #[test]
    fn a_run_that_never_started_says_why_and_still_hands_scripts_a_result() {
        let error = "no model is configured";
        let took = Duration::from_millis(5);
        // Text: why, on stderr — stdout has no answer to carry.
        assert_eq!(
            not_started(OutputFormat::Text, error, took),
            vec![warning(error)]
        );
        for format in [OutputFormat::Json, OutputFormat::StreamJson] {
            let out = not_started(format, error, took);
            assert_eq!(
                stderr(&out),
                vec![format!("alter-zero: {error}")],
                "{format:?}"
            );
            let lines = stdout(&out);
            assert_eq!(lines.len(), 1, "{format:?}: {lines:?}");
            let result = json(&lines[0]);
            assert_eq!(result["type"], "result");
            assert_eq!(result["subtype"], "error_during_execution");
            assert_eq!(result["is_error"], true);
            assert_eq!(result["result"], "");
            assert_eq!(result["error"], error);
            assert!(result["session_id"].is_null(), "{result}");
            assert!(result["model"].is_null(), "{result}");
            assert_eq!(result["num_turns"], 0);
            assert_eq!(result["duration_ms"], 5);
            assert_eq!(result["permission_denials"], serde_json::json!([]));
        }
    }

    #[test]
    fn the_wait_on_background_agents_is_ten_minutes_unless_set() {
        assert_eq!(agent_wait(None), Duration::from_secs(600));
        assert_eq!(agent_wait(Some("30")), Duration::from_secs(30));
        assert_eq!(agent_wait(Some(" 30 ")), Duration::from_secs(30));
        assert_eq!(agent_wait(Some("0")), Duration::ZERO, "0 waits not at all");
        for bad in ["", "ten", "-5", "1.5"] {
            assert_eq!(agent_wait(Some(bad)), AGENT_WAIT_DEFAULT, "{bad:?}");
        }
    }

    #[test]
    fn a_toast_reaches_stderr_when_it_is_an_error_or_the_run_is_verbose() {
        assert_eq!(
            toast_line("providers.toml: bad", true, false),
            Some(warning("providers.toml: bad"))
        );
        assert_eq!(
            toast_line("Thinking: high", false, true),
            Some(warning("Thinking: high"))
        );
        assert_eq!(toast_line("Thinking: high", false, false), None);
    }

    #[test]
    fn a_retry_is_logged_when_verbose_and_an_outage_always_once() {
        // A retry that works is no news: only the step log tells of it.
        let mut quiet = Reporter::new(OutputFormat::StreamJson, false);
        assert!(quiet.retrying(2, 5).is_empty());
        let mut verbose = Reporter::new(OutputFormat::StreamJson, true);
        assert_eq!(
            verbose.retrying(2, 5),
            vec![Emit::Stderr(
                "alter-zero: the request failed — retrying (2/5)".into()
            )]
        );
        // An outage can hold the run for as long as it lasts: said either way.
        for mut reporter in [quiet, verbose] {
            assert_eq!(
                reporter.offline("api.openai.com", 1),
                vec![Emit::Stderr(
                    "alter-zero: no connection to api.openai.com — waiting for it".into()
                )]
            );
            assert!(reporter.offline("api.openai.com", 2).is_empty());
        }
    }

    /// Everything a reporter writes over a run that read, narrated, ran a
    /// command and answered — then ended as `outcome`.
    fn whole_run(format: OutputFormat, verbose: bool, outcome: &RunOutcome) -> Vec<Emit> {
        let mut reporter = Reporter::new(format, verbose);
        let mut out = reporter.start(&start_info());
        out.extend(reporter.settled(&[user("q"), reply("Let me look.")]));
        out.extend(reporter.tool_started("Bash", "ls", Some(r#"{"command":"ls"}"#)));
        out.extend(reporter.settled(&[
            tool("Bash", ToolStatus::Ok, "Exit code: 0\nCargo.toml\n"),
            reply("Answer."),
            summary(1),
        ]));
        let answer = (*outcome == RunOutcome::Success).then_some("Answer.");
        out.extend(reporter.finish(&end_info(outcome, answer)));
        out
    }

    #[test]
    fn a_quiet_text_run_prints_its_answer_and_nothing_else() {
        let out = whole_run(OutputFormat::Text, false, &RunOutcome::Success);
        assert_eq!(out, vec![Emit::Stdout("Answer.".into())]);
    }

    #[test]
    fn a_quiet_run_that_failed_says_why_on_stderr_in_every_format() {
        let outcome = RunOutcome::Failed("HTTP 400: bad request".into());
        for format in OutputFormat::ALL {
            let lines = stderr(&whole_run(format, false, &outcome));
            assert_eq!(
                lines,
                vec!["alter-zero: HTTP 400: bad request".to_string()],
                "{format:?}"
            );
        }
        // Verbose, the log's own error line and footer say it — once.
        let lines = stderr(&whole_run(OutputFormat::Text, true, &outcome));
        assert!(
            !lines
                .iter()
                .any(|line| line.starts_with("alter-zero: HTTP 400")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.starts_with("Failed after")),
            "{lines:?}"
        );
    }

    #[test]
    fn verbose_logs_each_step_to_stderr_in_every_format() {
        for format in OutputFormat::ALL {
            let out = whole_run(format, true, &RunOutcome::Success);
            let lines = stderr(&out);
            assert!(
                lines[0].starts_with("alter-zero 0.18.0 · gpt-5.5"),
                "{format:?}: {lines:?}"
            );
            assert!(
                lines.contains(&"Let me look.".to_string()),
                "{format:?}: {lines:?}"
            );
            assert!(
                lines.contains(&"● Bash(ls)".to_string()),
                "{format:?}: {lines:?}"
            );
            assert!(
                lines.last().is_some_and(|l| l.starts_with("Done in")),
                "{format:?}: {lines:?}"
            );
            // The answer is stdout's, never the log's.
            assert!(
                !lines.contains(&"Answer.".to_string()),
                "{format:?}: {lines:?}"
            );
            // stdout is the format's own, whatever stderr says.
            let quiet = whole_run(format, false, &RunOutcome::Success);
            assert_eq!(stdout(&out), stdout(&quiet), "{format:?}");
        }
    }

    #[test]
    fn the_answer_the_log_holds_back_is_the_final_answer() {
        // Whatever the run's shape, the reply the log never printed as
        // narration is exactly what `final_answer` returns.
        let runs: Vec<Vec<HistoryItem>> = vec![
            vec![user("q"), reply("A"), summary(1)],
            vec![
                user("q"),
                reply("A"),
                tool("bash", ToolStatus::Ok, "Exit code: 0"),
                reply("B"),
                summary(1),
            ],
            vec![user("q"), reply("A"), thought(), reply("B"), summary(1)],
            vec![
                user("q"),
                reply("A"),
                shell_notice(),
                summary(1),
                reply("B"),
                summary(1),
            ],
            vec![
                user("q"),
                reply("A"),
                hook("Stop hook feedback: x"),
                reply("B"),
                summary(1),
            ],
            vec![user("q"), reply("A"), failure("boom")],
        ];
        for run in runs {
            let mut reporter = Reporter::new(OutputFormat::Text, true);
            let mut logged = Vec::new();
            for item in &run {
                logged.extend(stderr(&reporter.settled(std::slice::from_ref(item))));
            }
            let answer = final_answer(&run);
            for item in &run {
                if let HistoryItem::Message(m) = item
                    && m.role == Role::Assistant
                {
                    let in_answer = answer.as_deref().is_some_and(|a| a.contains(&m.text));
                    assert_eq!(
                        logged.contains(&m.text),
                        !in_answer,
                        "{:?} in {run:?}: logged {logged:?}",
                        m.text
                    );
                }
            }
        }
    }

    #[test]
    fn stream_json_opens_with_the_session_and_pairs_a_tool_with_its_completion() {
        let mut reporter = Reporter::new(OutputFormat::StreamJson, false);
        let mut emits = reporter.start(&start_info());
        emits.extend(reporter.settled(&[user("fix it")]));
        emits.extend(reporter.tool_started(
            "bash",
            "cargo test",
            Some("{\n  \"command\": \"cargo test\"\n}"),
        ));
        emits.extend(reporter.settled(&[with_arguments(
            tool("bash", ToolStatus::Ok, "Exit code: 0\nok"),
            "{\n  \"command\": \"cargo test\"\n}",
        )]));
        assert!(stderr(&emits).is_empty(), "{emits:?}");
        let lines = stdout(&emits);
        assert!(lines.iter().all(|line| !line.contains('\n')), "{lines:?}");
        let events: Vec<serde_json::Value> = lines.iter().map(|l| json(l)).collect();
        assert_eq!(events[0]["type"], "session.started");
        assert_eq!(events[0]["session_id"], "18a9-1a2b");
        assert_eq!(events[0]["model"], "gpt-5.5");
        assert_eq!(events[0]["cwd"], "/home/u/proj");
        assert_eq!(events[0]["permission_mode"], "auto");
        assert_eq!(events[0]["resumed"], false);
        assert_eq!(events[1]["type"], "item.completed");
        assert_eq!(events[1]["item"]["id"], "item_0");
        assert_eq!(events[1]["item"]["type"], "user_message");
        assert_eq!(events[1]["item"]["text"], "fix it");
        assert_eq!(events[2]["type"], "item.started");
        assert_eq!(events[2]["item"]["id"], "item_1");
        assert_eq!(events[2]["item"]["type"], "tool_call");
        assert_eq!(events[2]["item"]["name"], "bash");
        assert_eq!(events[2]["item"]["summary"], "cargo test");
        assert_eq!(events[2]["item"]["arguments"]["command"], "cargo test");
        assert_eq!(events[2]["item"]["status"], "in_progress");
        assert_eq!(events[3]["type"], "item.completed");
        assert_eq!(events[3]["item"]["id"], "item_1", "the same item completes");
        assert_eq!(events[3]["item"]["status"], "completed");
        assert_eq!(events[3]["item"]["output"], "Exit code: 0\nok");
        // `id` first, `type` second — codex's layout.
        assert!(
            lines[3].starts_with(
                r#"{"type":"item.completed","item":{"id":"item_1","type":"tool_call""#
            ),
            "{}",
            lines[3]
        );
    }

    #[test]
    fn every_history_item_maps_to_its_stream_item() {
        let mut reporter = Reporter::new(OutputFormat::StreamJson, false);
        let group = HistoryItem::AgentGroup(AgentGroup {
            background: false,
            agents: vec![AgentGroupEntry {
                id: "a1".into(),
                description: "Review the diff".into(),
                agent_type: "critic".into(),
                prompt: "review".into(),
                status: AgentStatus::Done,
                tool_uses: 4,
                tokens: 1200,
                secs: 31,
                result: "Two bugs.".into(),
                tool_headers: Vec::new(),
                output: "Two bugs.".into(),
                call_id: None,
                arguments: None,
                position: None,
            }],
            batch: None,
            timestamp: String::new(),
        });
        let notice = HistoryItem::AgentNotice(AgentNotice {
            id: "a2".into(),
            description: "Fuzz the parser".into(),
            status: AgentStatus::Failed,
            secs: 9,
            result: "provider error".into(),
            timestamp: String::new(),
            user_messages: Vec::new(),
            resumed: false,
        });
        let items = [
            reply("A"),
            thought(),
            declined("bash", "Not run: needs approval (headless run)"),
            group,
            shell_notice(),
            notice,
            compaction(),
            hook("Stop hook feedback: run the tests"),
            message(Role::System, "Checkpoint restored"),
        ];
        let events: Vec<serde_json::Value> = stdout(&reporter.settled(&items))
            .iter()
            .map(|l| json(l))
            .collect();
        let item = |n: usize| &events[n]["item"];
        assert_eq!(item(0)["type"], "agent_message");
        assert_eq!(item(1)["type"], "reasoning");
        assert_eq!(item(1)["text"], "think");
        assert_eq!(item(2)["type"], "tool_call");
        assert_eq!(item(2)["status"], "declined");
        assert_eq!(
            item(2)["output"],
            "The call was not run.",
            "what the model read"
        );
        assert_eq!(item(3)["type"], "subagents");
        assert_eq!(item(3)["background"], false);
        assert_eq!(item(3)["agents"][0]["id"], "a1");
        assert_eq!(item(3)["agents"][0]["type"], "critic");
        assert_eq!(item(3)["agents"][0]["description"], "Review the diff");
        assert_eq!(item(3)["agents"][0]["status"], "completed");
        assert_eq!(item(3)["agents"][0]["result"], "Two bugs.");
        assert_eq!(item(4)["type"], "background_notice");
        assert_eq!(item(4)["description"], "npm test");
        assert_eq!(item(4)["exit_code"], 0);
        assert_eq!(item(4)["shell_id"], "b1");
        assert_eq!(item(5)["type"], "agent_notice");
        assert_eq!(item(5)["status"], "failed");
        assert_eq!(item(5)["agent_id"], "a2");
        assert_eq!(item(6)["type"], "compaction");
        assert_eq!(item(6)["before"], 9000);
        assert_eq!(item(6)["after"], 900);
        assert_eq!(item(7)["type"], "hook");
        assert_eq!(item(7)["label"], "Stop hook");
        assert_eq!(item(8)["type"], "notice");
        assert_eq!(item(8)["text"], "Checkpoint restored");
        let ids: Vec<&str> = events
            .iter()
            .map(|e| e["item"]["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            [
                "item_0", "item_1", "item_2", "item_3", "item_4", "item_5", "item_6", "item_7",
                "item_8"
            ]
        );
    }

    #[test]
    fn a_turn_ends_in_turn_completed_or_turn_failed() {
        let mut reporter = Reporter::new(OutputFormat::StreamJson, false);
        let HistoryItem::Summary(mut done) = summary(71) else {
            unreachable!()
        };
        done.tokens = 42_000;
        done.cached = 31_000;
        let events: Vec<serde_json::Value> = stdout(&reporter.settled(&[
            HistoryItem::Summary(done),
            failure("request failed: HTTP 500"),
        ]))
        .iter()
        .map(|l| json(l))
        .collect();
        assert_eq!(events[0]["type"], "turn.completed");
        assert_eq!(events[0]["duration_ms"], 71_000);
        assert_eq!(events[0]["usage"]["total_tokens"], 42_000);
        assert_eq!(events[0]["usage"]["cached_input_tokens"], 31_000);
        assert_eq!(events[1]["type"], "turn.failed");
        assert_eq!(events[1]["error"]["message"], "request failed: HTTP 500");
    }

    #[test]
    fn a_tool_still_open_at_the_end_completes_as_interrupted_before_the_result() {
        let mut reporter = Reporter::new(OutputFormat::StreamJson, false);
        let _ = reporter.tool_started("bash", "sleep 60", None);
        let outcome = RunOutcome::Interrupted(2);
        let events: Vec<serde_json::Value> = stdout(&reporter.finish(&end_info(&outcome, None)))
            .iter()
            .map(|l| json(l))
            .collect();
        assert_eq!(events[0]["type"], "item.completed");
        assert_eq!(events[0]["item"]["id"], "item_0");
        assert_eq!(events[0]["item"]["status"], "failed");
        assert_eq!(events[0]["item"]["output"], "Interrupted");
        assert_eq!(events[1]["type"], "result");
        assert_eq!(events[1]["subtype"], "error_interrupted");
    }

    #[test]
    fn json_prints_one_result_object_and_nothing_else() {
        let mut reporter = Reporter::new(OutputFormat::Json, false);
        let mut emits = reporter.start(&start_info());
        emits.extend(reporter.tool_started("bash", "ls", None));
        emits.extend(reporter.settled(&[user("q"), reply("A"), summary(1)]));
        let denials = [Denial::from(&refusal(PermissionKind::Bash, "rm x", None))];
        let outcome = RunOutcome::Success;
        let mut end = end_info(&outcome, Some("A"));
        end.denials = &denials;
        emits.extend(reporter.finish(&end));
        let out = stdout(&emits);
        assert_eq!(out.len(), 1, "{out:?}");
        let result = json(&out[0]);
        assert_eq!(result["type"], "result");
        assert_eq!(result["subtype"], "success");
        assert_eq!(result["is_error"], false);
        assert_eq!(result["result"], "A");
        assert!(result.get("error").is_none(), "{result}");
        assert_eq!(result["session_id"], "18a9-1a2b");
        assert_eq!(result["model"], "gpt-5.5");
        assert_eq!(result["num_turns"], 3);
        assert_eq!(result["duration_ms"], 72_000);
        assert_eq!(result["usage"]["input_tokens"], 40_000);
        assert_eq!(result["usage"]["output_tokens"], 2_000);
        assert_eq!(result["usage"]["cached_input_tokens"], 31_000);
        assert_eq!(result["usage"]["cache_write_input_tokens"], 0);
        let denial = &result["permission_denials"][0];
        assert_eq!(denial["tool_name"], "bash");
        assert_eq!(denial["tool_use_id"], "call_7");
        assert_eq!(denial["tool_input"]["command"], "rm x");
        assert!(denial.get("agent").is_none(), "{denial}");
        assert!(
            out[0].starts_with(r#"{"type":"result","subtype":"success","is_error":false"#),
            "{}",
            out[0]
        );
    }

    #[test]
    fn a_failed_result_carries_its_error_and_an_empty_result() {
        let outcome = RunOutcome::MaxTurns("Stopped after 3 model replies".into());
        let mut end = end_info(&outcome, None);
        end.session_id = None;
        let result = json(&stdout(&Reporter::new(OutputFormat::Json, false).finish(&end))[0]);
        assert_eq!(result["subtype"], "error_max_turns");
        assert_eq!(result["is_error"], true);
        assert_eq!(result["result"], "");
        assert_eq!(result["error"], "Stopped after 3 model replies");
        assert!(result["session_id"].is_null());
    }

    #[test]
    fn a_denial_reports_the_call_in_claude_codes_terms() {
        // The lead's own call: Claude Code's entry, its input compacted.
        let lead = Denial::from(&refusal(PermissionKind::Bash, "rm -rf build", None));
        assert_eq!(
            serde_json::to_string(&lead).expect("serializes"),
            r#"{"tool_name":"bash","tool_use_id":"call_7","tool_input":{"command":"rm -rf build"}}"#
        );
        // A subagent's: the type that asked rides beside it.
        let sub = Denial::from(&refusal(
            PermissionKind::Bash,
            "rm -rf build",
            Some("explore"),
        ));
        assert_eq!(
            serde_json::to_value(&sub).expect("serializes")["agent"],
            "explore"
        );
        // The closing warning names each kind as it always did.
        let kinds = [
            (PermissionKind::Write, "write"),
            (PermissionKind::Edit, "edit"),
            (PermissionKind::Bash, "bash"),
            (PermissionKind::Mcp, "mcp"),
            (PermissionKind::Session, "session"),
        ];
        for (kind, label) in kinds {
            let denial = Denial::from(&refusal(kind, "rm -rf build", None));
            assert_eq!(
                (denial.kind, denial.target.as_str()),
                (label, "rm -rf build")
            );
        }
    }

    #[test]
    fn a_denial_whose_input_is_not_an_object_reports_an_empty_one() {
        for input in ["not json", "[1, 2]", "\"rm\"", ""] {
            let mut refused = refusal(PermissionKind::Bash, "rm", None);
            refused.tool_input = input.to_string();
            let denial = serde_json::to_value(Denial::from(&refused)).expect("serializes");
            assert_eq!(denial["tool_input"], serde_json::json!({}), "{input:?}");
        }
    }

    #[test]
    fn compact_json_drops_the_whitespace_between_tokens_only() {
        assert_eq!(
            compact_json("{\n  \"a\": [1, 2],\n  \"b\": \"x  y\\n\\\"z\"\n}").as_deref(),
            Some(r#"{"a":[1,2],"b":"x  y\n\"z"}"#)
        );
        assert_eq!(compact_json("not json"), None);
        assert_eq!(compact_json(""), None);
    }

    #[test]
    fn cut_measures_display_columns_and_marks_the_cut() {
        assert_eq!(cut("short", 10), "short");
        assert_eq!(cut("exactly10!", 10), "exactly10!");
        assert_eq!(cut("eleven cols", 10), "eleven co…");
        // A wide glyph is two columns: four of them are eight, plus the mark.
        assert_eq!(cut("漢字漢字漢字", 9), "漢字漢字…");
        assert_eq!(cols(&cut("漢字漢字漢字", 9)), 9);
    }
}
