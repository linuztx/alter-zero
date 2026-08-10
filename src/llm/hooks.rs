//! Running the user's lifecycle hooks — the boundary half (`docs/hooks.md`).
//!
//! The pure half ([`crate::hooks`]) decides *which* handlers an event selects,
//! *what* JSON they are fed, and *how* their answers combine. This module owns
//! the only part that touches the world: spawning each handler, writing the
//! payload to its stdin, waiting against a deadline while the turn's
//! [`CancelToken`] stays answerable, and capturing what came back.
//!
//! Two things here are load-bearing and easy to get wrong:
//!
//! 1. **A blocking hook must poll the cancel token.** These run on the
//!    backend's own OS thread, where blocking is normal — the permission gate
//!    parks there on a `Condvar` today — but only because every such park
//!    re-checks cancellation. A hook runner that waited without that would
//!    silently break Esc for as long as the hook took. The wait loop below
//!    polls on `POLL_INTERVAL`, the same 20 ms cadence `llm::exec` uses.
//! 2. **A hook child is detached from the terminal like every other.** It
//!    goes through [`crate::subprocess::spawn_shell_with`], so a hook that
//!    tries to read `/dev/tty` (a stray `sudo`) fails fast instead of printing
//!    over the live region and fighting the event loop for the keyboard.
//!
//! The [`HookSink`] trait at the bottom is the seam the agent loop sees. Every
//! method defaults to doing nothing, so [`NoHooks`] costs nothing, tests need
//! no fixture, and **adding an event is one defaulted method plus one call
//! site** rather than a wider `run_agent` signature.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::hooks::{
    CommandHook, HOOK_OUTPUT_MAX_BYTES, HookContext, HookEvent, HookOutcome, HookPermission,
    HookRun, HooksFile, ParsedHook, merge, parse_run, truncate_output,
};
use crate::stream::CancelToken;
use crate::subprocess;

use super::tools::{ToolCallRequest, ToolOutcome};

/// How often the wait loop wakes to re-check the deadline and the turn's
/// cancel flag — `llm::exec`'s `BASH_POLL_INTERVAL`, for the same reason:
/// short enough that Esc reaps promptly, long enough not to spin.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The whole `SessionEnd` event's hard time budget: quitting (or `/clear`)
/// must never hang on a hook — Claude Code defaults this event to 1.5 s and
/// codex clamps it to 3 s, against the 600 s every other event gets.
const SESSION_END_BUDGET: Duration = Duration::from_secs(2);

/// The project-root variable a hook script reads. The `CLAUDE_PROJECT_DIR`
/// alias is set beside it deliberately: the point of implementing Claude
/// Code's contract is that a script written for it works here unchanged, and
/// finding the project root is the first thing such a script does.
const PROJECT_DIR_VARS: &[&str] = &["ALTER_ZERO_PROJECT_DIR", "CLAUDE_PROJECT_DIR"];

/// What the agent loop does about a `PreToolUse` verdict.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreToolVerdict {
    /// Refuse the call. The cell shows the short form, the model reads the
    /// long one — the [`crate::permission::Approval::Reject`] split.
    pub blocked: Option<String>,
    /// Skip the permission gate for this call (`permissionDecision: "allow"`).
    pub pre_approved: bool,
    /// Put it to the user even if a standing rule would have allowed it
    /// (`permissionDecision: "ask"`).
    pub force_ask: bool,
    /// A rewritten `tool_input`, as JSON object text.
    pub updated_input: Option<String>,
    /// Appended to what the model reads back from the call.
    pub context: Option<String>,
    /// A dim `⎿` row for the user, and the record of why this happened.
    pub note: Option<String>,
}

/// What the agent loop does about a `PostToolUse` verdict. The call already
/// ran, so there is nothing to refuse — only things to say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostToolVerdict {
    /// Appended to what the model reads back from the call.
    pub context: Option<String>,
    /// A dim `⎿` row for the user, and the record.
    pub note: Option<String>,
}

impl PostToolVerdict {
    /// Nothing to do downstream.
    #[must_use]
    pub fn is_quiet(&self) -> bool {
        self.context.is_none() && self.note.is_none()
    }
}

/// What a `UserPromptSubmit` hook said about the prompt (`docs/hooks.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptVerdict {
    /// Refuse the prompt: the turn never reaches the model, the submission
    /// rolls back, the reason is the red notice. (`continue: false` maps
    /// here too — codex's reading; the prompt is never recorded.)
    pub blocked: Option<String>,
    /// `additionalContext` entries, in handler order — injected **even on a
    /// block**, both references' rule.
    pub contexts: Vec<String>,
}

/// What a `PermissionRequest` hook decided in the user's stead, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookPermissionVerdict {
    /// Run it; `note` records that a hook — not the user — said so.
    Allow { note: String },
    /// Refuse it, with the cell text and the model-facing text.
    Deny { display: String, result: String },
}

/// The seam the agent loop and the approval gate consult.
///
/// Every method defaults to "nothing happened", which is why [`NoHooks`] is a
/// unit struct and why no existing test needed touching. A new event is a new
/// defaulted method plus its one call site — if a later change makes that
/// untrue, the change is wrong (`docs/hooks.md`).
pub trait HookSink: Send + Sync + std::fmt::Debug {
    /// Before a call is put to the permission gate.
    fn pre_tool_use(&self, _call: &ToolCallRequest, _cancel: &CancelToken) -> PreToolVerdict {
        PreToolVerdict::default()
    }

    /// After a call ran, with the outcome it produced.
    fn post_tool_use(
        &self,
        _call: &ToolCallRequest,
        _outcome: &ToolOutcome,
        _cancel: &CancelToken,
    ) -> PostToolVerdict {
        PostToolVerdict::default()
    }

    /// Instead of asking the user. `None` falls through to whatever would
    /// have happened anyway — the classifier, then the prompt.
    fn permission_request(
        &self,
        _call: &ToolCallRequest,
        _cancel: &CancelToken,
    ) -> Option<HookPermissionVerdict> {
        None
    }

    /// A subagent was launched; the returned strings are extra context for
    /// **that** agent's own conversation.
    ///
    /// Takes the agent's own [`CancelToken`], like every other blocking method
    /// here: an Esc while a subagent is starting must reap its hook rather
    /// than wait the handler's timeout out.
    fn subagent_start(
        &self,
        _agent_id: &str,
        _agent_type: &str,
        _cancel: &CancelToken,
    ) -> Vec<String> {
        Vec::new()
    }

    /// The model finished answering — `Stop` for the lead sink,
    /// `SubagentStop` for a [`for_subagent`](Self::for_subagent)-tagged one
    /// (Claude Code chooses the event the same way, by the presence of an
    /// agent id). Fired by `run_agent` at the point the turn would complete
    /// — never on a cancel, an error, a compact turn or a `!` shell.
    ///
    /// `Some(feedback)` is a **block**: the agent keeps going, the feedback
    /// its next user message. `stop_hook_active` is true on the second and
    /// later firings within one turn — the loop guard's flag, which the hook
    /// itself checks to avoid looping forever (the engine never refuses a
    /// re-block; that is Claude Code's exact posture, and Esc stays the stop
    /// button).
    fn stop(
        &self,
        _stop_hook_active: bool,
        _last_message: &str,
        _cancel: &CancelToken,
    ) -> Option<String> {
        None
    }

    /// A queued session boundary was crossed (`startup` / `resume` /
    /// `clear`), drained codex-style at the **next turn's** top on the
    /// backend thread — never at bootstrap, where seconds of silence read as
    /// a hang (the checkpoint probe's lesson). Returns the hooks'
    /// `additionalContext` strings; the matcher gates on the source.
    fn session_start(&self, _cancel: &CancelToken) -> Vec<String> {
        Vec::new()
    }

    /// The user submitted `prompt` — fired right after the SessionStart
    /// drain, before the first request. Never fired for a loop-initiated
    /// turn (a background completion's follow-up), a `/compact`
    /// summarization, a subagent's prompt, or a `!` command — none of those
    /// is a user prompt (Claude Code's bash-mode skips it too).
    fn user_prompt_submit(&self, _prompt: &str, _cancel: &CancelToken) -> PromptVerdict {
        PromptVerdict::default()
    }

    /// A view of this sink that reports its calls as coming from a subagent,
    /// so `agent_id` / `agent_type` reach every payload it builds. `None` —
    /// the default — means there is nothing to re-tag and the caller keeps
    /// the sink it has. Object-safe on purpose: the backend holds an
    /// `Arc<dyn HookSink>` and must be able to call this through it.
    fn for_subagent(&self, _agent_id: &str, _agent_type: &str) -> Option<Arc<dyn HookSink>> {
        None
    }

    /// The session is closing (`reason`: `clear` — a `/clear` starting a
    /// fresh conversation — or `prompt_input_exit`, a quit). Envelope-only,
    /// output ignored, and **bounded**: quitting must never hang on a hook,
    /// so the whole event runs under a hard budget (Claude Code caps it at
    /// 1.5 s, codex clamps to 3 s; ours is `SESSION_END_BUDGET`, 2 s). No
    /// cancel token — the session is past cancellation.
    fn session_end(&self, _reason: &str) {}

    /// The event name [`turn_start`] stamps on this sink's prompt-time
    /// context — `UserPromptSubmit` everywhere except the compact wrapper,
    /// whose "prompt" hook is really `PreCompact` (`docs/hooks.md`).
    fn prompt_hook_label(&self) -> &'static str {
        "UserPromptSubmit"
    }
}

/// The no-op sink: what a session with hooks disabled (or an embedder that
/// built the backend directly) uses. Costs one vtable dispatch per call site
/// and nothing else.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHooks;

impl HookSink for NoHooks {}

/// The compact turn's sink (`docs/hooks.md`): the summarization spawn maps
/// the turn lifecycle onto the compact events — its "prompt" hook is
/// `PreCompact` (whose context strings become extra summarization
/// instructions) and its completion is `PostCompact`. Everything else stays
/// quiet: the compact backend is tools-free, drains no session sources, and
/// never fires `Stop` (the wrapper's `stop` **is** PostCompact and never
/// blocks — a summarization must not be continued by a hook).
#[derive(Debug)]
pub struct CompactHooks {
    inner: CommandHooks,
    /// `manual` or `auto` — the payload's `trigger`, and its matcher.
    trigger: &'static str,
}

impl CompactHooks {
    #[must_use]
    pub fn new(inner: CommandHooks, auto: bool) -> Self {
        Self {
            inner,
            trigger: if auto { "auto" } else { "manual" },
        }
    }
}

impl HookSink for CompactHooks {
    fn user_prompt_submit(&self, _prompt: &str, cancel: &CancelToken) -> PromptVerdict {
        PromptVerdict {
            blocked: None,
            contexts: self.inner.pre_compact(self.trigger, "", cancel),
        }
    }

    fn stop(
        &self,
        _stop_hook_active: bool,
        last_message: &str,
        cancel: &CancelToken,
    ) -> Option<String> {
        self.inner.post_compact(self.trigger, last_message, cancel);
        None
    }

    fn prompt_hook_label(&self) -> &'static str {
        "PreCompact"
    }
}

/// The rollout path the recorder publishes for hook payloads: `None` until
/// anything was recorded (the file is created lazily on the first item), the
/// path from then on. Shared because the sink outlives — and predates — the
/// file (`docs/hooks.md` *Known gaps*, now closed).
pub type TranscriptCell = Arc<std::sync::RwLock<Option<String>>>;

/// The pending `SessionStart` sources (`startup` / `resume` / `clear`),
/// queued by the boundary and drained at the next turn's top — codex's
/// pending-source model, so nothing ever blocks the first paint.
pub type SessionSources = Arc<std::sync::Mutex<Vec<String>>>;

/// The live handles a [`CommandHooks`] resolves per dispatch — shared with
/// the boundary so the payloads report the session as it **is**, not as it
/// was when the sink was built (a `/model` switch rebuilds sinks; these
/// survive the rebuild).
#[derive(Debug, Clone, Default)]
pub struct HookHandles {
    /// The permission gate, read per payload so a Ctrl+A cycle reaches the
    /// very next call. `None` (gate off, an embedder) keeps the context's
    /// own value.
    pub gate: Option<crate::permission::PermissionGate>,
    /// The rollout path the recorder publishes.
    pub transcript: TranscriptCell,
    /// The queued `SessionStart` sources.
    pub sources: SessionSources,
    /// Set by the boundary just before it dispatches a **loop-initiated**
    /// turn (a background completion's follow-up): the next
    /// `user_prompt_submit` is skipped — that prompt is synthesized, not the
    /// user's. Cleared by the skip.
    pub synthetic_turn: Arc<std::sync::atomic::AtomicBool>,
}

/// The real sink: a loaded `hooks.json` plus everything a payload needs.
#[derive(Debug, Clone)]
pub struct CommandHooks {
    file: Arc<HooksFile>,
    context: HookContext,
    /// The detach-helper path threaded from `main` — see [`subprocess`].
    detach_helper: Option<PathBuf>,
    /// Where handlers run, and what `*_PROJECT_DIR` is set to.
    cwd: PathBuf,
    /// The live handles — gate, transcript path, pending session sources —
    /// resolved at **dispatch** time, so a payload reports the session as it
    /// is when the hook fires.
    handles: HookHandles,
}

impl CommandHooks {
    /// Build a sink over `file`. Returns `None` when there is nothing that
    /// could ever run, so the caller keeps [`NoHooks`] and pays nothing.
    #[must_use]
    pub fn new(
        file: Arc<HooksFile>,
        context: HookContext,
        detach_helper: Option<PathBuf>,
        cwd: PathBuf,
        handles: HookHandles,
    ) -> Option<Self> {
        if file.is_empty() {
            return None;
        }
        Some(Self {
            file,
            context,
            detach_helper,
            cwd,
            handles,
        })
    }

    /// The context as of **now**: the stored session facts with the
    /// permission mode and the transcript path resolved from their live
    /// handles. Every payload builds from this, never from the frozen
    /// snapshot.
    fn live_context(&self) -> HookContext {
        let mut context = self.context.clone();
        if let Some(gate) = &self.handles.gate {
            context.permission_mode = Some(gate.mode().label().to_string());
        }
        if let Some(path) = self
            .handles
            .transcript
            .read()
            .ok()
            .and_then(|cell| cell.clone())
        {
            context.transcript_path = Some(path);
        }
        context
    }

    /// Run every handler `event` selects for `query`, in order, and merge
    /// their answers. Handlers run **sequentially**: they can block each
    /// other's tool call anyway, and a bounded pool is a phase-two concern.
    fn dispatch(
        &self,
        event: HookEvent,
        query: Option<&str>,
        payload: &str,
        cancel: &CancelToken,
    ) -> HookOutcome {
        let selection = self.file.select(event, query);
        let mut parsed: Vec<ParsedHook> = selection
            .warnings
            .into_iter()
            .map(|warning| ParsedHook {
                warning: Some(warning),
                ..ParsedHook::default()
            })
            .collect();
        for handler in &selection.handlers {
            if cancel.is_cancelled() {
                break;
            }
            let run = self.run_handler(handler, payload, cancel);
            parsed.push(parse_run(&run, event));
        }
        merge(parsed)
    }

    /// Spawn one handler, feed it `payload` on stdin, and wait for it.
    fn run_handler(&self, handler: &CommandHook, payload: &str, cancel: &CancelToken) -> HookRun {
        let mut run = HookRun {
            command: handler.command.clone(),
            ..HookRun::default()
        };
        let cwd = self.cwd.clone();
        let spawned = subprocess::spawn_shell_with(
            self.detach_helper.as_deref(),
            &handler.command,
            move |cmd| {
                cmd.stdin(std::process::Stdio::piped()).current_dir(&cwd);
                for var in PROJECT_DIR_VARS {
                    cmd.env(var, &cwd);
                }
            },
        );
        let mut child = match spawned {
            Ok(child) => child,
            Err(err) => {
                run.error = Some(format!("failed to spawn: {err}"));
                return run;
            }
        };

        // Write the payload on its own thread and close the pipe, so a handler
        // blocked on EOF (`cat`, `jq`, `read`) proceeds. A `BrokenPipe` means
        // the handler exited without reading its input, which is legitimate —
        // a guard that only cares *that* it was called never reads stdin.
        //
        // A thread, not an inline `write_all`: a payload past the pipe buffer
        // (a `PostToolUse` carrying a 64 KiB tool output) fed to a handler
        // that never reads stdin would park an inline write until the child
        // exited — before the deadline loop below, so neither Esc nor the
        // timeout could reap it. The wait loop owns cancellation; the writer
        // just finishes (or breaks) when the child is gone.
        let stdin_writer = child.stdin.take().map(|mut stdin| {
            let payload = payload.to_string();
            std::thread::spawn(move || {
                let wrote = stdin
                    .write_all(payload.as_bytes())
                    .and_then(|()| stdin.flush());
                match wrote {
                    Ok(()) => None,
                    Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => None,
                    Err(err) => Some(format!("failed to write payload: {err}")),
                }
            })
        });

        // Drain both pipes on their own threads: a handler that prints more
        // than a pipe buffer would otherwise deadlock against our wait.
        let out_pipe = child.stdout.take();
        let out_reader = std::thread::spawn(move || read_pipe(out_pipe));
        let err_pipe = child.stderr.take();
        let err_reader = std::thread::spawn(move || read_pipe(err_pipe));

        let deadline = Duration::from_secs(handler.timeout_secs);
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(err) => {
                    // Reap before bailing: the readers (and the writer) hold
                    // the pipes, and a child left running would park their
                    // joins below — codex kills on a wait error for the same
                    // reason.
                    subprocess::kill_process_group(&mut child);
                    run.error = Some(format!("failed to wait: {err}"));
                    break None;
                }
            }
            // Esc, `/clear`, a quit: reap the whole tree and stop. The turn is
            // being torn down, so the verdict is moot — but a hook left
            // running would keep a reader thread alive holding the pipe.
            if cancel.is_cancelled() {
                subprocess::kill_process_group(&mut child);
                run.error = Some("interrupted".to_string());
                break None;
            }
            if started.elapsed() >= deadline {
                subprocess::kill_process_group(&mut child);
                run.error = Some(format!("timed out after {}s", handler.timeout_secs));
                break None;
            }
            std::thread::sleep(POLL_INTERVAL);
        };

        // The group kill above has reaped anything holding the pipes, so the
        // joins terminate — the writer's pipe end is closed by the child's
        // death too, so it can never outlive this point.
        run.stdout = out_reader.join().unwrap_or_default();
        run.stderr = err_reader.join().unwrap_or_default();
        let write_error = stdin_writer.and_then(|writer| writer.join().unwrap_or_default());
        if run.error.is_none() {
            run.error = write_error;
        }
        if run.error.is_none() {
            run.exit_code = status.and_then(|s| s.code());
            if run.exit_code.is_none() {
                run.error = Some("killed by a signal".to_string());
            }
        }
        run
    }

    /// `PreCompact` (`docs/hooks.md`): fired by the compact wrapper on the
    /// summarization spawn's thread. It cannot block — **in either
    /// reference** (Claude Code's own in-app docs claim exit 2 blocks
    /// compaction, but the `blocked` field is never read at any call site;
    /// codex has no exit-2 arm for it) — its real contract is that stdout /
    /// `additionalContext` become extra instructions for the summarization
    /// prompt, which is what the returned strings are.
    fn pre_compact(&self, trigger: &str, custom: &str, cancel: &CancelToken) -> Vec<String> {
        let payload = crate::hooks::pre_compact_payload(&self.live_context(), trigger, custom);
        self.dispatch(HookEvent::PreCompact, Some(trigger), &payload, cancel)
            .additional_context
    }

    /// `PostCompact` (`docs/hooks.md`): the summary that replaced the
    /// conversation, envelope-only — nothing to return.
    fn post_compact(&self, trigger: &str, summary: &str, cancel: &CancelToken) {
        let payload = crate::hooks::post_compact_payload(&self.live_context(), trigger, summary);
        let _ = self.dispatch(HookEvent::PostCompact, Some(trigger), &payload, cancel);
    }

    /// A `HookContext` naming the subagent that is acting.
    fn tagged(&self, agent_id: &str, agent_type: &str) -> Self {
        let mut clone = self.clone();
        clone.context.agent_id = Some(agent_id.to_string());
        clone.context.agent_type = Some(agent_type.to_string());
        clone
    }
}

/// Read a pipe to a lossy `String`, **capped in memory as it is read**
/// (`tui::shell::append_capped`'s rule — a hook that spews gigabytes must not
/// spike RSS): the first [`HOOK_OUTPUT_MAX_BYTES`] are kept, the rest is
/// drained and dropped so the child never blocks on a full pipe, and
/// [`truncate_output`] stamps the marker. An absent pipe reads as empty.
fn read_pipe<R: Read>(pipe: Option<R>) -> String {
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    // One byte past the cap, so `truncate_output` can tell "exactly full"
    // from "overflowed" and only mark the latter.
    let mut kept: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = (HOOK_OUTPUT_MAX_BYTES + 1).saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
    }
    truncate_output(&String::from_utf8_lossy(&kept))
}

/// The cell/model text pair for a hook refusal — the same two-text shape the
/// permission gate's rejections use, so the whole recording, transcript and
/// `/resume` path is the one that already exists.
#[must_use]
pub fn block_texts(reason: &str) -> (String, String) {
    (
        format!("Blocked by hook: {reason}"),
        format!(
            "A configured lifecycle hook blocked this tool call.\n\nReason: {reason}\n\n\
             Do not retry the same call. Address the reason, or explain to the user why the \
             call is needed and wait for their instruction."
        ),
    )
}

/// What the turn's opening hooks decided (`docs/hooks.md`): the notes to
/// record + inject (SessionStart's first, then UserPromptSubmit's — each a
/// `(label, wire text)` pair), and the block reason when the prompt was
/// refused. The notes come back even on a block, both references' rule. The
/// glue in `LlmBackend::spawn` sends each note as a
/// [`crate::stream::StreamEvent::HookNote`] and appends it as a user message
/// after the prompt; a block sends `PromptBlocked` and returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnStart {
    pub notes: Vec<(String, String)>,
    pub blocked: Option<String>,
}

/// Run the turn's opening hooks — the SessionStart drain, then
/// UserPromptSubmit — and fold their answers into one [`TurnStart`]. Pure
/// over the sink, so the whole spawn-top dance is testable with a fake.
#[must_use]
pub fn turn_start(hooks: &dyn HookSink, prompt: &str, cancel: &CancelToken) -> TurnStart {
    let mut notes = Vec::new();
    if let Some(note) = context_note_texts("SessionStart", &hooks.session_start(cancel)) {
        notes.push(note);
    }
    let verdict = hooks.user_prompt_submit(prompt, cancel);
    if let Some(note) = context_note_texts(hooks.prompt_hook_label(), &verdict.contexts) {
        notes.push(note);
    }
    TurnStart {
        notes,
        blocked: verdict.blocked,
    }
}

/// A hook's `additionalContext` as the transcript label + the user-role wire
/// text the model reads — Claude Code's exact `{hookName} hook additional
/// context:` wording inside its system-reminder wrapper, so a script's
/// injected facts land in the shape models already know.
#[must_use]
pub fn context_note_texts(event: &str, contexts: &[String]) -> Option<(String, String)> {
    if contexts.is_empty() {
        return None;
    }
    Some((
        format!("{event} hook"),
        format!(
            "<system-reminder>\n{event} hook additional context: {}\n</system-reminder>",
            contexts.join("\n")
        ),
    ))
}

/// A `Stop`/`SubagentStop` block's continuation feedback, as the transcript
/// label + the verbatim user-role message the model reads — Claude Code's
/// exact `Stop hook feedback:` wording, so a hook written against it drives
/// the same conversation here. The dummy scenario calls this too, keeping the
/// offline cell byte-for-byte the live one.
#[must_use]
pub fn stop_feedback_texts(reason: &str) -> (String, String) {
    (
        "Stop hook".to_string(),
        format!("Stop hook feedback:\n{reason}"),
    )
}

/// The dim `⎿` provenance row a hook's context or note leaves on the cell —
/// the auto-mode classifier's `⎿ Allowed by auto mode classifier` pattern.
fn note_line(warnings: &[String], context: Option<&String>, systems: &[String]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if context.is_some() {
        parts.push("Context added by hook".to_string());
    }
    parts.extend(systems.iter().cloned());
    parts.extend(warnings.iter().cloned());
    (!parts.is_empty()).then(|| parts.join(" · "))
}

impl HookSink for CommandHooks {
    fn pre_tool_use(&self, call: &ToolCallRequest, cancel: &CancelToken) -> PreToolVerdict {
        let payload = crate::hooks::pre_tool_use_payload(
            &self.live_context(),
            &call.name,
            &call.arguments,
            &call.id,
        );
        let outcome = self.dispatch(HookEvent::PreToolUse, Some(&call.name), &payload, cancel);
        if outcome.is_quiet() {
            return PreToolVerdict::default();
        }
        let context = outcome.context_note();
        PreToolVerdict {
            blocked: outcome.block_reason,
            pre_approved: outcome.permission == Some(HookPermission::Allow),
            force_ask: outcome.permission == Some(HookPermission::Ask),
            updated_input: outcome.updated_input,
            note: note_line(
                &outcome.warnings,
                context.as_ref(),
                &outcome.system_messages,
            ),
            context,
        }
    }

    fn post_tool_use(
        &self,
        call: &ToolCallRequest,
        outcome: &ToolOutcome,
        cancel: &CancelToken,
    ) -> PostToolVerdict {
        let payload = crate::hooks::post_tool_use_payload(
            &self.live_context(),
            &call.name,
            &call.arguments,
            &outcome.output,
            outcome.ok,
            &call.id,
        );
        let hooked = self.dispatch(HookEvent::PostToolUse, Some(&call.name), &payload, cancel);
        if hooked.is_quiet() {
            return PostToolVerdict::default();
        }
        // The call already ran, so a `block` here is not a refusal — it is
        // feedback the model must read. Fold it in beside any context.
        let mut context: Vec<String> = Vec::new();
        if let Some(reason) = &hooked.block_reason {
            context.push(format!("A lifecycle hook flagged this result: {reason}"));
        }
        context.extend(hooked.additional_context.iter().cloned());
        let context = (!context.is_empty()).then(|| context.join("\n\n"));
        let mut warnings = hooked.warnings.clone();
        if let Some(reason) = hooked.block_reason {
            warnings.insert(0, format!("Flagged by hook: {reason}"));
        }
        PostToolVerdict {
            note: note_line(&warnings, context.as_ref(), &hooked.system_messages),
            context,
        }
    }

    fn permission_request(
        &self,
        call: &ToolCallRequest,
        cancel: &CancelToken,
    ) -> Option<HookPermissionVerdict> {
        let payload = crate::hooks::permission_request_payload(
            &self.live_context(),
            &call.name,
            &call.arguments,
            &call.id,
        );
        let outcome = self.dispatch(
            HookEvent::PermissionRequest,
            Some(&call.name),
            &payload,
            cancel,
        );
        match outcome.permission? {
            HookPermission::Allow => Some(HookPermissionVerdict::Allow {
                // A broken sibling's warning rides the note — the one channel
                // this verdict has — so "my guard is broken" is never silent.
                // (A verdict-less outcome has no channel at all; its warnings
                // are dropped, the documented fail-open posture.)
                note: std::iter::once(outcome.permission_reason.map_or_else(
                    || "Allowed by hook".to_string(),
                    |reason| format!("Allowed by hook: {reason}"),
                ))
                .chain(outcome.warnings)
                .collect::<Vec<_>>()
                .join(" · "),
            }),
            HookPermission::Deny => {
                let reason = outcome
                    .block_reason
                    .or(outcome.permission_reason)
                    .unwrap_or_else(|| "denied by a hook".to_string());
                let (display, result) = block_texts(&reason);
                Some(HookPermissionVerdict::Deny { display, result })
            }
            // An explicit `ask` is the default path stated out loud.
            HookPermission::Ask => None,
        }
    }

    fn subagent_start(
        &self,
        agent_id: &str,
        agent_type: &str,
        cancel: &CancelToken,
    ) -> Vec<String> {
        let tagged = self.tagged(agent_id, agent_type);
        let payload =
            crate::hooks::subagent_start_payload(&tagged.live_context(), agent_id, agent_type);
        tagged
            .dispatch(HookEvent::SubagentStart, Some(agent_type), &payload, cancel)
            .additional_context
    }

    fn stop(
        &self,
        stop_hook_active: bool,
        last_message: &str,
        cancel: &CancelToken,
    ) -> Option<String> {
        // The sink's tag picks the event — Claude Code's own rule (`Stop`
        // for the lead, `SubagentStop` when an agent id is present); the
        // subagent event matches on its agent type, the lead's on nothing.
        let context = self.live_context();
        let (event, query, payload) = match (&context.agent_id, &context.agent_type) {
            (Some(id), Some(kind)) => (
                HookEvent::SubagentStop,
                Some(kind.clone()),
                crate::hooks::subagent_stop_payload(
                    &context,
                    id,
                    kind,
                    stop_hook_active,
                    last_message,
                ),
            ),
            _ => (
                HookEvent::Stop,
                None,
                crate::hooks::stop_payload(&context, stop_hook_active, last_message),
            ),
        };
        let outcome = self.dispatch(event, query.as_deref(), &payload, cancel);
        // `continue: false` outranks a block (codex's aggregate rule): a
        // hook that says *stop everything* is not asking for another round.
        if outcome.stopped {
            return None;
        }
        outcome.block_reason
    }

    fn session_start(&self, cancel: &CancelToken) -> Vec<String> {
        let sources: Vec<String> = match self.handles.sources.lock() {
            Ok(mut queue) => queue.drain(..).collect(),
            Err(_) => Vec::new(),
        };
        let mut contexts = Vec::new();
        for source in sources {
            let payload = crate::hooks::session_start_payload(&self.live_context(), &source);
            let outcome = self.dispatch(HookEvent::SessionStart, Some(&source), &payload, cancel);
            contexts.extend(outcome.additional_context);
        }
        contexts
    }

    fn user_prompt_submit(&self, prompt: &str, cancel: &CancelToken) -> PromptVerdict {
        // A loop-initiated turn's prompt is synthesized, not the user's —
        // the boundary marked it, and the mark clears with the skip.
        if self
            .handles
            .synthetic_turn
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return PromptVerdict::default();
        }
        let payload = crate::hooks::user_prompt_submit_payload(&self.live_context(), prompt);
        // Matcher-less, both references' rule: every configured group fires.
        let outcome = self.dispatch(HookEvent::UserPromptSubmit, None, &payload, cancel);
        let blocked = outcome.block_reason.clone().or_else(|| {
            outcome.stopped.then(|| {
                outcome
                    .stop_reason
                    .clone()
                    .unwrap_or_else(|| "stopped by a hook".to_string())
            })
        });
        PromptVerdict {
            blocked,
            contexts: outcome.additional_context,
        }
    }

    fn session_end(&self, reason: &str) {
        let payload = crate::hooks::session_end_payload(&self.live_context(), reason);
        let selection = self.file.select(HookEvent::SessionEnd, Some(reason));
        let started = Instant::now();
        let cancel = CancelToken::new();
        for handler in &selection.handlers {
            let remaining = SESSION_END_BUDGET.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            // Each handler gets what is left of the event budget, never more
            // than its own timeout (and at least the 1 s floor a zero would
            // defeat).
            let capped = CommandHook {
                timeout_secs: handler.timeout_secs.min(remaining.as_secs().max(1)),
                ..handler.clone()
            };
            let _ = self.run_handler(&capped, &payload, &cancel);
        }
    }

    fn for_subagent(&self, agent_id: &str, agent_type: &str) -> Option<Arc<dyn HookSink>> {
        Some(Arc::new(self.tagged(agent_id, agent_type)))
    }
}

/// The `hooks.json` path: `ALTER_ZERO_HOOKS_FILE`, else
/// `{config_home}/hooks.json`. Mirrors how every other config file resolves.
#[must_use]
pub fn hooks_file_path(config_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ALTER_ZERO_HOOKS_FILE") {
        return Some(PathBuf::from(path));
    }
    config_home.map(|dir| dir.join("hooks.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hooks_for(json: &str) -> CommandHooks {
        let file = HooksFile::parse(json).expect("fixture parses");
        CommandHooks::new(
            Arc::new(file),
            HookContext {
                session_id: "s".into(),
                cwd: "/tmp".into(),
                model: "m".into(),
                permission_mode: Some("manual".into()),
                ..HookContext::default()
            },
            None,
            std::env::temp_dir(),
            HookHandles::default(),
        )
        .expect("the fixture has a runnable handler")
    }

    fn call(name: &str, arguments: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: "call-1".into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    fn pre_hook(command: &str) -> String {
        format!(
            r#"{{"hooks":{{"PreToolUse":[{{"matcher":"bash",
                 "hooks":[{{"type":"command","command":{command},"timeout":20}}]}}]}}}}"#,
            command = serde_json::to_string(command).unwrap()
        )
    }

    #[test]
    fn a_file_with_nothing_runnable_declines_to_build_a_sink() {
        let file = HooksFile::parse(r#"{"hooks":{"PreToolUse":[]}}"#).unwrap();
        assert!(
            CommandHooks::new(
                Arc::new(file),
                HookContext::default(),
                None,
                std::env::temp_dir(),
                HookHandles::default(),
            )
            .is_none()
        );
    }

    #[test]
    fn the_no_op_sink_says_nothing_about_anything() {
        let cancel = CancelToken::new();
        let c = call("bash", "{}");
        assert_eq!(NoHooks.pre_tool_use(&c, &cancel), PreToolVerdict::default());
        assert!(NoHooks.permission_request(&c, &cancel).is_none());
        assert!(NoHooks.subagent_start("a", "explore", &cancel).is_empty());
    }

    #[test]
    fn a_hook_reads_the_payload_on_stdin_and_can_block_on_what_it_finds() {
        // The end-to-end contract in one command: read stdin, inspect
        // `tool_input.command`, refuse with exit 2 + a stderr reason.
        let hooks = hooks_for(&pre_hook(
            r#"read -r line; case "$line" in *"rm -rf"*) echo "no destructive deletes" >&2; exit 2;; esac"#,
        ));
        let verdict = hooks.pre_tool_use(
            &call("bash", r#"{"command":"rm -rf build/"}"#),
            &CancelToken::new(),
        );
        assert_eq!(
            verdict.blocked.as_deref(),
            Some("no destructive deletes"),
            "the hook's stderr is the reason"
        );

        let allowed = hooks.pre_tool_use(&call("bash", r#"{"command":"ls"}"#), &CancelToken::new());
        assert_eq!(allowed, PreToolVerdict::default(), "a silent hook allows");
    }

    #[test]
    fn a_hook_whose_matcher_does_not_fire_never_runs_at_all() {
        // `false` would block if it ran — the matcher is what saves the call.
        let hooks = hooks_for(&pre_hook("echo blocked >&2; exit 2"));
        let verdict = hooks.pre_tool_use(&call("read", "{}"), &CancelToken::new());
        assert_eq!(verdict, PreToolVerdict::default());
    }

    #[test]
    fn a_json_verdict_can_deny_rewrite_and_add_context_at_once() {
        let hooks = hooks_for(&pre_hook(
            r#"printf '%s' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","additionalContext":"ran under review","updatedInput":{"command":"ls -la"}}}'"#,
        ));
        let verdict = hooks.pre_tool_use(&call("bash", r#"{"command":"ls"}"#), &CancelToken::new());
        assert!(verdict.pre_approved);
        assert_eq!(verdict.blocked, None);
        assert_eq!(verdict.context.as_deref(), Some("ran under review"));
        let updated = verdict.updated_input.expect("a rewrite");
        assert!(updated.contains("ls -la"), "{updated}");
        assert!(verdict.note.is_some(), "the user sees that a hook spoke");
    }

    #[test]
    fn a_hook_that_exceeds_its_timeout_is_killed_and_fails_open() {
        let json = r#"{"hooks":{"PreToolUse":[{"hooks":[
            {"type":"command","command":"sleep 30","timeout":1}]}]}}"#;
        let hooks = hooks_for(json);
        let started = Instant::now();
        let verdict = hooks.pre_tool_use(&call("bash", "{}"), &CancelToken::new());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the timeout must actually reap it: {:?}",
            started.elapsed()
        );
        assert_eq!(
            verdict.blocked, None,
            "a broken hook must not wedge the agent"
        );
        let note = verdict.note.expect("the failure is surfaced");
        assert!(note.contains("timed out"), "{note}");
    }

    #[test]
    fn a_payload_bigger_than_the_pipe_buffer_never_wedges_the_cancel() {
        // The regression: the payload write used to run on the calling thread
        // *before* the deadline loop, so a payload past the pipe buffer
        // (64 KiB on Linux) fed to a hook that never reads stdin parked
        // `write_all` until the child exited — Esc dead, timeout never armed.
        // A `PostToolUse` payload embeds the tool's output, whose cap *is*
        // 64 KiB, so this is reachable from an ordinary `read`/`bash` call.
        let json = r#"{"hooks":{"PostToolUse":[{"hooks":[
            {"type":"command","command":"sleep 8","timeout":600}]}]}}"#;
        let hooks = hooks_for(json);
        let outcome = ToolOutcome::ok("x".repeat(HOOK_OUTPUT_MAX_BYTES));
        let cancel = CancelToken::new();
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            flag.cancel();
        });
        let started = Instant::now();
        let verdict = hooks.post_tool_use(&call("bash", "{}"), &outcome, &cancel);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "Esc must reap a hook stuck on its stdin write: {:?}",
            started.elapsed()
        );
        assert!(verdict.is_quiet() || verdict.note.is_some());
    }

    #[test]
    fn a_cancelled_turn_reaps_a_running_hook_promptly() {
        let json = r#"{"hooks":{"PreToolUse":[{"hooks":[
            {"type":"command","command":"sleep 30","timeout":600}]}]}}"#;
        let hooks = hooks_for(json);
        let cancel = CancelToken::new();
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            flag.cancel();
        });
        let started = Instant::now();
        let verdict = hooks.pre_tool_use(&call("bash", "{}"), &cancel);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Esc must not wait out the hook: {:?}",
            started.elapsed()
        );
        assert_eq!(verdict.blocked, None);
    }

    #[test]
    fn a_missing_hook_command_fails_open_with_a_warning() {
        let hooks = hooks_for(&pre_hook("/definitely/not/a/real/binary"));
        let verdict = hooks.pre_tool_use(&call("bash", "{}"), &CancelToken::new());
        assert_eq!(verdict.blocked, None);
        assert!(verdict.note.is_some(), "the user hears about it");
    }

    #[test]
    fn two_matching_hooks_both_run_and_the_first_block_wins() {
        let json = r#"{"hooks":{"PreToolUse":[
            {"hooks":[{"type":"command","command":"echo first >&2; exit 2"}]},
            {"hooks":[{"type":"command","command":"echo second >&2; exit 2"}]}]}}"#;
        let hooks = hooks_for(json);
        let verdict = hooks.pre_tool_use(&call("bash", "{}"), &CancelToken::new());
        assert_eq!(verdict.blocked.as_deref(), Some("first"));
    }

    #[test]
    fn post_tool_use_sees_the_output_and_turns_a_block_into_model_feedback() {
        let json = r#"{"hooks":{"PostToolUse":[{"matcher":"bash","hooks":[
            {"type":"command","command":"grep -q 'FAILED' && printf '%s' '{\"decision\":\"block\",\"reason\":\"tests are red\"}' || true"}]}]}}"#;
        let hooks = hooks_for(json);
        let outcome = ToolOutcome {
            output: "3 passed, 1 FAILED".into(),
            ok: true,
            ..ToolOutcome::ok(String::new())
        };
        let verdict = hooks.post_tool_use(&call("bash", "{}"), &outcome, &CancelToken::new());
        let context = verdict.context.expect("the model is told");
        assert!(context.contains("tests are red"), "{context}");
        assert!(verdict.note.is_some_and(|n| n.contains("tests are red")));
    }

    #[test]
    fn permission_request_can_answer_in_the_users_stead_both_ways() {
        let deny = hooks_for(
            r#"{"hooks":{"PermissionRequest":[{"hooks":[{"type":"command","command":
              "printf '%s' '{\"hookSpecificOutput\":{\"hookEventName\":\"PermissionRequest\",\"decision\":{\"behavior\":\"deny\",\"message\":\"never in prod\"}}}'"}]}]}}"#,
        );
        match deny.permission_request(&call("bash", "{}"), &CancelToken::new()) {
            Some(HookPermissionVerdict::Deny { display, result }) => {
                assert!(display.contains("never in prod"), "{display}");
                assert!(result.contains("never in prod"), "{result}");
            }
            other => panic!("expected a deny, got {other:?}"),
        }

        let allow = hooks_for(
            r#"{"hooks":{"PermissionRequest":[{"hooks":[{"type":"command","command":
              "printf '%s' '{\"hookSpecificOutput\":{\"hookEventName\":\"PermissionRequest\",\"decision\":{\"behavior\":\"allow\"}}}'"}]}]}}"#,
        );
        assert!(matches!(
            allow.permission_request(&call("bash", "{}"), &CancelToken::new()),
            Some(HookPermissionVerdict::Allow { .. })
        ));
    }

    #[test]
    fn a_broken_permission_request_sibling_rides_the_allow_note() {
        // Two handlers: one that cannot spawn (a warning) and one that
        // allows. The warning must not vanish — it rides the visible note so
        // the user hears their guard is broken.
        let hooks = hooks_for(
            r#"{"hooks":{"PermissionRequest":[{"hooks":[
              {"type":"command","command":"/definitely/not/a/real/binary"},
              {"type":"command","command":
              "printf '%s' '{\"hookSpecificOutput\":{\"hookEventName\":\"PermissionRequest\",\"decision\":{\"behavior\":\"allow\"}}}'"}]}]}}"#,
        );
        match hooks.permission_request(&call("bash", "{}"), &CancelToken::new()) {
            Some(HookPermissionVerdict::Allow { note }) => {
                assert!(note.starts_with("Allowed by hook"), "{note}");
                assert!(
                    note.contains("/definitely/not/a/real/binary"),
                    "the broken sibling is named: {note}"
                );
            }
            other => panic!("expected an allow, got {other:?}"),
        }
    }

    #[test]
    fn a_subagent_start_hooks_plain_output_becomes_that_agents_context() {
        // SubagentStart is one of the events whose plain stdout *is* the
        // answer — a hook that just prints is briefing the agent.
        let hooks = hooks_for(
            r#"{"hooks":{"SubagentStart":[{"matcher":"explore","hooks":[{"type":"command",
              "command":"echo 'the repo root is /srv/app'"}]}]}}"#,
        );
        assert_eq!(
            hooks.subagent_start("a1", "explore", &CancelToken::new()),
            vec!["the repo root is /srv/app".to_string()]
        );
        // A different agent type is not this hook's business.
        assert!(
            hooks
                .subagent_start("a2", "reviewer", &CancelToken::new())
                .is_empty(),
            "the matcher gates on agent_type"
        );
    }

    #[test]
    fn a_lead_sinks_stop_fires_the_stop_event_and_a_block_is_the_feedback() {
        let probe = std::env::temp_dir().join("alter-zero-hook-stop-probe.json");
        let _ = std::fs::remove_file(&probe);
        let hooks = hooks_for(&format!(
            r#"{{"hooks":{{"Stop":[{{"hooks":[{{"type":"command",
              "command":"tee {} | grep -q '\"stop_hook_active\":false' && {{ echo 'tests are red' >&2; exit 2; }} || exit 0"}}]}}]}}}}"#,
            probe.display()
        ));
        // First firing: the flag is false, the hook blocks — the reason is
        // the continuation feedback.
        let feedback = hooks.stop(false, "the answer", &CancelToken::new());
        assert_eq!(feedback.as_deref(), Some("tests are red"));
        let written = std::fs::read_to_string(&probe).expect("the stop hook ran");
        let value: serde_json::Value = serde_json::from_str(&written).expect("valid JSON");
        assert_eq!(value["hook_event_name"], serde_json::json!("Stop"));
        assert_eq!(value["stop_hook_active"], serde_json::json!(false));
        assert_eq!(
            value["last_assistant_message"],
            serde_json::json!("the answer")
        );
        // Second firing: the flag is true, the hook lets go.
        assert_eq!(hooks.stop(true, "fixed it", &CancelToken::new()), None);
        let _ = std::fs::remove_file(&probe);
    }

    #[test]
    fn a_tagged_sinks_stop_fires_subagent_stop_and_matches_the_agent_type() {
        let probe = std::env::temp_dir().join("alter-zero-hook-substop-probe.json");
        let _ = std::fs::remove_file(&probe);
        let hooks = hooks_for(&format!(
            r#"{{"hooks":{{"SubagentStop":[{{"matcher":"explore","hooks":[{{"type":"command",
              "command":"cat > {}"}}]}}]}}}}"#,
            probe.display()
        ));
        let sink = hooks
            .for_subagent("a1", "explore")
            .expect("a real sink re-tags itself");
        assert_eq!(sink.stop(false, "found it", &CancelToken::new()), None);
        let written = std::fs::read_to_string(&probe).expect("the stop hook ran");
        let value: serde_json::Value = serde_json::from_str(&written).expect("valid JSON");
        assert_eq!(value["hook_event_name"], serde_json::json!("SubagentStop"));
        assert_eq!(value["agent_id"], serde_json::json!("a1"));
        assert_eq!(value["agent_type"], serde_json::json!("explore"));
        assert_eq!(value["stop_hook_active"], serde_json::json!(false));
        assert_eq!(
            value["last_assistant_message"],
            serde_json::json!("found it")
        );
        // The old `success` extension is gone — the event fires only on a
        // natural completion now.
        assert!(!value.as_object().unwrap().contains_key("success"));
        let _ = std::fs::remove_file(&probe);

        // A different agent type is not this hook's business.
        let other = hooks.for_subagent("a2", "reviewer").expect("re-tags");
        let _ = std::fs::remove_file(&probe);
        assert_eq!(other.stop(false, "done", &CancelToken::new()), None);
        assert!(!probe.exists(), "the matcher gates on agent_type");
    }

    #[test]
    fn a_stop_hooks_continue_false_outranks_its_own_block() {
        // codex's aggregate rule: a hook that says *stop everything* is not
        // asking for another round, even when it also spelled a block.
        let hooks = hooks_for(
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":
              "printf '%s' '{\"continue\":false,\"decision\":\"block\",\"reason\":\"keep going\"}'"}]}]}}"#,
        );
        assert_eq!(hooks.stop(false, "answer", &CancelToken::new()), None);
    }

    #[test]
    fn a_subagents_calls_carry_its_identity_into_the_payload() {
        // The hook writes what it was told to a file; we read it back.
        let probe = std::env::temp_dir().join("alter-zero-hook-agent-probe.json");
        let _ = std::fs::remove_file(&probe);
        let hooks = hooks_for(&pre_hook(&format!("cat > {}", probe.display())));
        let sink = hooks
            .for_subagent("agent-7", "explore")
            .expect("a real sink re-tags itself");
        let _ = sink.pre_tool_use(&call("bash", "{}"), &CancelToken::new());
        let written = std::fs::read_to_string(&probe).expect("the hook received a payload");
        let value: serde_json::Value = serde_json::from_str(&written).expect("valid JSON");
        assert_eq!(value["agent_id"], serde_json::json!("agent-7"));
        assert_eq!(value["agent_type"], serde_json::json!("explore"));
        let _ = std::fs::remove_file(&probe);
    }

    #[test]
    fn payloads_report_the_live_permission_mode_and_transcript_path() {
        // The regression: the sink used to freeze `permission_mode: None`
        // ("disabled") at build time, so a Ctrl+A cycle never reached a
        // payload — and `transcript_path` stayed null forever because the
        // rollout file is created after the sink. Both now resolve at
        // dispatch time from live handles.
        let probe = std::env::temp_dir().join("alter-zero-hook-live-ctx-probe.json");
        let _ = std::fs::remove_file(&probe);
        let gate = crate::permission::PermissionGate::new();
        gate.set_mode(crate::permission::PermissionMode::Edit);
        let transcript: TranscriptCell = TranscriptCell::default();
        let file = HooksFile::parse(&pre_hook(&format!("cat > {}", probe.display()))).unwrap();
        let hooks = CommandHooks::new(
            Arc::new(file),
            HookContext {
                session_id: "s".into(),
                cwd: "/tmp".into(),
                model: "m".into(),
                permission_mode: None,
                ..HookContext::default()
            },
            None,
            std::env::temp_dir(),
            HookHandles {
                gate: Some(gate.clone()),
                transcript: Arc::clone(&transcript),
                ..HookHandles::default()
            },
        )
        .expect("runnable");

        let _ = hooks.pre_tool_use(&call("bash", "{}"), &CancelToken::new());
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&probe).expect("hook ran")).unwrap();
        assert_eq!(value["permission_mode"], serde_json::json!("edit"));
        assert_eq!(value["transcript_path"], serde_json::Value::Null);

        gate.set_mode(crate::permission::PermissionMode::Master);
        *transcript.write().expect("not poisoned") = Some("/r/sess.jsonl".to_string());
        let _ = hooks.pre_tool_use(&call("bash", "{}"), &CancelToken::new());
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&probe).expect("hook ran")).unwrap();
        assert_eq!(
            value["permission_mode"],
            serde_json::json!("master"),
            "a Ctrl+A cycle reaches the next payload"
        );
        assert_eq!(
            value["transcript_path"],
            serde_json::json!("/r/sess.jsonl"),
            "the recorder's published rollout path reaches the payload"
        );
        let _ = std::fs::remove_file(&probe);
    }

    #[test]
    fn session_start_drains_the_queued_sources_once_and_matches_on_them() {
        // codex's pending-source model: bootstrap/`/clear`/`/resume` queue a
        // source, the next turn's top drains them all, and the matcher gates
        // on the source string. A second drain finds nothing.
        let file = HooksFile::parse(
            r#"{"hooks":{"SessionStart":[{"matcher":"startup","hooks":[{"type":"command",
              "command":"echo 'the build id is ZX-4417'"}]}]}}"#,
        )
        .unwrap();
        let handles = HookHandles::default();
        handles
            .sources
            .lock()
            .unwrap()
            .extend(["startup".to_string(), "clear".to_string()]);
        let hooks = CommandHooks::new(
            Arc::new(file),
            HookContext::default(),
            None,
            std::env::temp_dir(),
            handles,
        )
        .expect("runnable");
        assert_eq!(
            hooks.session_start(&CancelToken::new()),
            vec!["the build id is ZX-4417".to_string()],
            "plain stdout is the context; the clear source matched nothing"
        );
        assert_eq!(
            hooks.session_start(&CancelToken::new()),
            Vec::<String>::new(),
            "the queue drained"
        );
    }

    #[test]
    fn user_prompt_submit_blocks_adds_context_and_skips_synthetic_turns() {
        let file = HooksFile::parse(
            r#"{"hooks":{"UserPromptSubmit":[{"matcher":"ignored-for-this-event","hooks":[{"type":"command",
              "command":"grep -q secret && { echo 'no secrets in prompts' >&2; exit 2; }; echo 'shipped today: v2'"}]}]}}"#,
        )
        .unwrap();
        let handles = HookHandles::default();
        let hooks = CommandHooks::new(
            Arc::new(file),
            HookContext::default(),
            None,
            std::env::temp_dir(),
            handles.clone(),
        )
        .expect("runnable");

        // Matcher-less event: the group fires despite its matcher text.
        let blocked = hooks.user_prompt_submit("here is my secret", &CancelToken::new());
        assert_eq!(blocked.blocked.as_deref(), Some("no secrets in prompts"));

        let clean = hooks.user_prompt_submit("hello", &CancelToken::new());
        assert_eq!(clean.blocked, None);
        assert_eq!(
            clean.contexts,
            vec!["shipped today: v2".to_string()],
            "plain stdout is context for this event"
        );

        // A loop-initiated turn is marked synthetic: the hook never fires and
        // the mark clears with the skip.
        handles
            .synthetic_turn
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            hooks.user_prompt_submit("here is my secret", &CancelToken::new()),
            PromptVerdict::default()
        );
        assert_eq!(
            hooks
                .user_prompt_submit("here is my secret", &CancelToken::new())
                .blocked
                .as_deref(),
            Some("no secrets in prompts"),
            "the mark cleared: the next real prompt is gated again"
        );
    }

    #[test]
    fn turn_start_folds_the_opening_hooks_into_notes_and_a_verdict() {
        // The spawn-top glue in one call: SessionStart's context first, then
        // UserPromptSubmit's, each already formatted as the wire text the
        // model reads — and the notes come back even when the prompt is
        // blocked (both references inject on block).
        #[derive(Debug)]
        struct Fake;
        impl HookSink for Fake {
            fn session_start(&self, _cancel: &CancelToken) -> Vec<String> {
                vec!["the repo root is /srv/app".to_string()]
            }
            fn user_prompt_submit(&self, prompt: &str, _cancel: &CancelToken) -> PromptVerdict {
                PromptVerdict {
                    blocked: prompt.contains("secret").then(|| "no secrets".to_string()),
                    contexts: vec!["shipped today: v2".to_string()],
                }
            }
        }
        let opening = turn_start(&Fake, "hello", &CancelToken::new());
        assert_eq!(opening.blocked, None);
        assert_eq!(opening.notes.len(), 2);
        assert_eq!(opening.notes[0].0, "SessionStart hook");
        assert!(
            opening.notes[0].1.starts_with("<system-reminder>\n")
                && opening.notes[0]
                    .1
                    .contains("SessionStart hook additional context: the repo root is /srv/app"),
            "{:?}",
            opening.notes[0].1
        );
        assert_eq!(opening.notes[1].0, "UserPromptSubmit hook");

        let blocked = turn_start(&Fake, "my secret", &CancelToken::new());
        assert_eq!(blocked.blocked.as_deref(), Some("no secrets"));
        assert_eq!(blocked.notes.len(), 2, "context injects even on a block");
    }

    #[test]
    fn session_end_runs_under_a_hard_budget_and_matches_on_the_reason() {
        // Quitting must never hang on a hook: the whole event is capped at
        // ~2 s whatever the handler's own timeout says. The matcher gates on
        // the reason, Claude Code's rule.
        let probe = std::env::temp_dir().join("alter-zero-hook-end-probe.json");
        let _ = std::fs::remove_file(&probe);
        let hooks = hooks_for(&format!(
            r#"{{"hooks":{{"SessionEnd":[
              {{"matcher":"clear","hooks":[{{"type":"command","command":"cat > {}"}}]}},
              {{"matcher":"prompt_input_exit","hooks":[{{"type":"command","command":"sleep 30","timeout":600}}]}}]}}}}"#,
            probe.display()
        ));
        let started = Instant::now();
        hooks.session_end("prompt_input_exit");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the budget reaps a slow shutdown hook: {:?}",
            started.elapsed()
        );
        assert!(!probe.exists(), "the clear-matched handler never fired");

        hooks.session_end("clear");
        let written = std::fs::read_to_string(&probe).expect("the clear hook ran");
        let value: serde_json::Value = serde_json::from_str(&written).expect("valid JSON");
        assert_eq!(value["hook_event_name"], serde_json::json!("SessionEnd"));
        assert_eq!(value["reason"], serde_json::json!("clear"));
        let _ = std::fs::remove_file(&probe);
    }

    #[test]
    fn the_compact_wrapper_maps_the_turn_onto_the_compact_events() {
        // PreCompact rides the wrapper's prompt hook (context = extra
        // summarization instructions; it can never block — neither reference
        // honours a block for it) and PostCompact rides its stop (which
        // never continues a summarization).
        let probe = std::env::temp_dir().join("alter-zero-hook-compact-probe.jsonl");
        let _ = std::fs::remove_file(&probe);
        let file = HooksFile::parse(&format!(
            r#"{{"hooks":{{
              "PreCompact":[{{"matcher":"auto","hooks":[{{"type":"command",
                "command":"tee -a {p} >/dev/null; echo >> {p}; echo 'keep the API notes'"}}]}}],
              "PostCompact":[{{"hooks":[{{"type":"command","command":"tee -a {p} >/dev/null; echo >> {p}"}}]}}]}}}}"#,
            p = probe.display()
        ))
        .unwrap();
        let inner = CommandHooks::new(
            Arc::new(file),
            HookContext::default(),
            None,
            std::env::temp_dir(),
            HookHandles::default(),
        )
        .expect("runnable");
        let wrapper = CompactHooks::new(inner, true);

        let verdict =
            wrapper.user_prompt_submit("summarize this conversation", &CancelToken::new());
        assert_eq!(verdict.blocked, None, "PreCompact can never block");
        assert_eq!(verdict.contexts, vec!["keep the API notes".to_string()]);
        assert_eq!(wrapper.prompt_hook_label(), "PreCompact");

        assert_eq!(
            wrapper.stop(false, "the handoff summary", &CancelToken::new()),
            None,
            "a summarization is never continued by a hook"
        );
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(&probe)
            .expect("both hooks ran")
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid JSON"))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["hook_event_name"], serde_json::json!("PreCompact"));
        assert_eq!(lines[0]["trigger"], serde_json::json!("auto"));
        assert_eq!(
            lines[1]["hook_event_name"],
            serde_json::json!("PostCompact")
        );
        assert_eq!(
            lines[1]["compact_summary"],
            serde_json::json!("the handoff summary")
        );
        let _ = std::fs::remove_file(&probe);
    }

    #[test]
    fn the_project_dir_is_exported_under_both_names() {
        let probe = std::env::temp_dir().join("alter-zero-hook-env-probe.txt");
        let _ = std::fs::remove_file(&probe);
        let hooks = hooks_for(&pre_hook(&format!(
            "printf '%s|%s' \"$ALTER_ZERO_PROJECT_DIR\" \"$CLAUDE_PROJECT_DIR\" > {}",
            probe.display()
        )));
        let _ = hooks.pre_tool_use(&call("bash", "{}"), &CancelToken::new());
        let written = std::fs::read_to_string(&probe).expect("the hook ran");
        let (ours, theirs) = written.split_once('|').expect("both vars");
        assert!(!ours.is_empty(), "ALTER_ZERO_PROJECT_DIR is set");
        assert_eq!(ours, theirs, "the Claude Code alias matches");
        let _ = std::fs::remove_file(&probe);
    }
}
