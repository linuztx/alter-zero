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
    CommandHook, HookContext, HookEvent, HookOutcome, HookPermission, HookRun, HooksFile,
    ParsedHook, merge, parse_run, truncate_output,
};
use crate::stream::CancelToken;
use crate::subprocess;

use super::tools::{ToolCallRequest, ToolOutcome};

/// How often the wait loop wakes to re-check the deadline and the turn's
/// cancel flag — `llm::exec`'s `BASH_POLL_INTERVAL`, for the same reason:
/// short enough that Esc reaps promptly, long enough not to spin.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

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
    fn subagent_start(&self, _agent_id: &str, _agent_type: &str) -> Vec<String> {
        Vec::new()
    }

    /// A subagent finished.
    fn subagent_stop(&self, _agent_id: &str, _agent_type: &str, _ok: bool, _last_message: &str) {}

    /// A view of this sink that reports its calls as coming from a subagent,
    /// so `agent_id` / `agent_type` reach every payload it builds. `None` —
    /// the default — means there is nothing to re-tag and the caller keeps
    /// the sink it has. Object-safe on purpose: the backend holds an
    /// `Arc<dyn HookSink>` and must be able to call this through it.
    fn for_subagent(&self, _agent_id: &str, _agent_type: &str) -> Option<Arc<dyn HookSink>> {
        None
    }
}

/// The no-op sink: what a session with hooks disabled (or an embedder that
/// built the backend directly) uses. Costs one vtable dispatch per call site
/// and nothing else.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHooks;

impl HookSink for NoHooks {}

/// The real sink: a loaded `hooks.json` plus everything a payload needs.
#[derive(Debug, Clone)]
pub struct CommandHooks {
    file: Arc<HooksFile>,
    context: HookContext,
    /// The detach-helper path threaded from `main` — see [`subprocess`].
    detach_helper: Option<PathBuf>,
    /// Where handlers run, and what `*_PROJECT_DIR` is set to.
    cwd: PathBuf,
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
    ) -> Option<Self> {
        if file.is_empty() {
            return None;
        }
        Some(Self {
            file,
            context,
            detach_helper,
            cwd,
        })
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

        // Write the payload and close the pipe, so a handler blocked on EOF
        // (`cat`, `jq`, `read`) proceeds. A `BrokenPipe` means the handler
        // exited without reading its input, which is legitimate — a guard
        // that only cares *that* it was called never reads stdin.
        if let Some(mut stdin) = child.stdin.take() {
            let wrote = stdin
                .write_all(payload.as_bytes())
                .and_then(|()| stdin.flush());
            drop(stdin);
            if let Err(err) = wrote
                && err.kind() != std::io::ErrorKind::BrokenPipe
            {
                subprocess::kill_process_group(&mut child);
                run.error = Some(format!("failed to write payload: {err}"));
                return run;
            }
        }

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

        // The group kill above has reaped anything holding the pipes, so both
        // joins terminate.
        run.stdout = truncate_output(&out_reader.join().unwrap_or_default());
        run.stderr = truncate_output(&err_reader.join().unwrap_or_default());
        if run.error.is_none() {
            run.exit_code = status.and_then(|s| s.code());
            if run.exit_code.is_none() {
                run.error = Some("killed by a signal".to_string());
            }
        }
        run
    }

    /// A `HookContext` naming the subagent that is acting.
    fn tagged(&self, agent_id: &str, agent_type: &str) -> Self {
        let mut clone = self.clone();
        clone.context.agent_id = Some(agent_id.to_string());
        clone.context.agent_type = Some(agent_type.to_string());
        clone
    }
}

/// Read a pipe to a lossy `String`; an absent pipe reads as empty.
fn read_pipe<R: Read>(pipe: Option<R>) -> String {
    let mut buffer = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buffer);
    }
    String::from_utf8_lossy(&buffer).into_owned()
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
            &self.context,
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
            &self.context,
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
            &self.context,
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
                note: outcome.permission_reason.map_or_else(
                    || "Allowed by hook".to_string(),
                    |reason| format!("Allowed by hook: {reason}"),
                ),
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

    fn subagent_start(&self, agent_id: &str, agent_type: &str) -> Vec<String> {
        let tagged = self.tagged(agent_id, agent_type);
        let payload = crate::hooks::subagent_start_payload(&tagged.context, agent_id, agent_type);
        let cancel = CancelToken::new();
        tagged
            .dispatch(
                HookEvent::SubagentStart,
                Some(agent_type),
                &payload,
                &cancel,
            )
            .additional_context
    }

    fn subagent_stop(&self, agent_id: &str, agent_type: &str, ok: bool, last_message: &str) {
        let tagged = self.tagged(agent_id, agent_type);
        let payload = crate::hooks::subagent_stop_payload(
            &tagged.context,
            agent_id,
            agent_type,
            last_message,
            !ok,
        );
        let cancel = CancelToken::new();
        tagged.dispatch(HookEvent::SubagentStop, Some(agent_type), &payload, &cancel);
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
                std::env::temp_dir()
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
        assert!(NoHooks.subagent_start("a", "explore").is_empty());
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
