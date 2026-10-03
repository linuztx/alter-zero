//! The permission gate's backend half (`docs/permissions.md`): turn a tool call
//! into the [`PermissionRequest`] the prompt shows, and block the tool thread on
//! the user's answer.
//!
//! Boundary code — building an `edit`/`write` preview means reading the file it
//! would change — but the preview itself is the same pure rendering the finished
//! cell uses ([`crate::llm::tools::render_numbered_content`] /
//! [`render_numbered_diff`]), so the
//! prompt shows exactly what the resulting cell will.

use tokio::sync::mpsc::UnboundedSender;

use super::hooks::{HookPermissionVerdict, HookSink};
use super::tools::{
    self, BashArgs, EditArgs, SessionArgs, ToolCallRequest, WriteArgs, render_numbered_content,
    render_numbered_diff,
};
use crate::permission::{
    Approval, CLASSIFIER_ALLOWED_NOTE, ClassifierVerdict, PermissionDecision, PermissionGate,
    PermissionKind, PermissionMode, PermissionRequest, SCRATCHPAD_ALLOWED_NOTE,
    classifier_denial_result, classifier_denied_display, denial_result, denied_display,
    explain_display, explain_result,
};
use crate::stream::{CancelToken, StreamEvent};

/// The display + model-facing texts used when a turn is cancelled out from
/// under a waiting request — the call never ran, and the events go to an
/// already-swapped channel, so this is only ever a well-formed placeholder.
const CANCELLED_DISPLAY: &str = "Interrupted by user";

/// The **auto mode classifier** seam ([`approve_call`]'s `classify`
/// parameter): given the request (the command in `target`, the model's
/// description in `detail`), return the safety verdict — or `Err` when the
/// classifier itself failed (network, an unparseable reply), which falls
/// back to the ordinary prompt. The real implementation is
/// [`crate::llm::classifier::SafetyClassifier`]; tests inject scripts. (The
/// explicit lifetime keeps the trait object borrow-friendly — a bare alias
/// would default it to `'static`, refusing the backend's stack closures.)
pub type ClassifyCommand<'a> = dyn Fn(&PermissionRequest) -> Result<ClassifierVerdict, String> + 'a;

/// The lookup a prompt's dim description row needs: an MCP wire name → the
/// server's own one-line description of that tool
/// ([`crate::llm::mcp::McpManager::tool_description`], `docs/mcp.md`), and a
/// `bash_session` call's session id → the command that session runs
/// (`docs/interactive-shell.md` — which its allowlist rule then covers). The
/// two key spaces never meet: a wire name starts `mcp__`, a session id is
/// letters and digits. `None` for an MCP tool leaves the row out; for a
/// session it means nothing answers to that id, so there is nothing to type
/// into and nothing to approve.
pub type DescribeTool<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// What the user is being asked to approve for `call`, or `None` when the tool
/// needs no approval — a `read`, an unknown tool, or an `edit` that cannot
/// apply (it will fail without touching the file, so there is nothing to
/// approve). `id` is left empty; the caller stamps it from the gate.
///
/// A `write` over an **existing** file is an [`PermissionKind::Edit`]: it shows
/// the diff, exactly like the `Updated {path} (+A -D)` cell it will produce.
#[must_use]
pub fn permission_request(
    call: &ToolCallRequest,
    agent: Option<&str>,
    describe: Option<&DescribeTool<'_>>,
) -> Option<PermissionRequest> {
    let agent = agent.map(str::to_string);
    match call.name.as_str() {
        "write" => {
            let args: WriteArgs = tools::parse_args(&call.arguments).ok()?;
            let old = std::fs::read_to_string(&args.path).ok();
            let (kind, body) = match &old {
                Some(old) => (
                    PermissionKind::Edit,
                    render_numbered_diff(&tools::diff_lines(old, &args.content)),
                ),
                None => (
                    PermissionKind::Write,
                    render_numbered_content(&args.content),
                ),
            };
            Some(PermissionRequest {
                id: String::new(),
                kind,
                target: args.path,
                body,
                detail: None,
                agent,
                agent_id: None,
            })
        }
        "edit" => {
            let args: EditArgs = tools::parse_args(&call.arguments).ok()?;
            let old = std::fs::read_to_string(&args.path).ok()?;
            let result =
                tools::apply_edit(&old, &args.old_string, &args.new_string, args.replace_all)
                    .ok()?;
            Some(PermissionRequest {
                id: String::new(),
                kind: PermissionKind::Edit,
                target: args.path,
                body: render_numbered_diff(&tools::diff_lines(&old, &result.new_content)),
                detail: None,
                agent,
                agent_id: None,
            })
        }
        "bash" => {
            let args: BashArgs = tools::parse_args(&call.arguments).ok()?;
            Some(PermissionRequest {
                id: String::new(),
                kind: PermissionKind::Bash,
                target: args.command,
                body: String::new(),
                detail: args
                    .description
                    .map(|d| d.trim().to_string())
                    .filter(|d| !d.is_empty()),
                agent,
                agent_id: None,
            })
        }
        // Typing into a running session asks like a command does
        // (`docs/bash-tools.md`) — what is typed there runs just the same.
        // `bashwait`, `bashkill` and `bashlist` never ask: a wait only reads,
        // a kill ends a command the model started (the ↓ manager's stop asks
        // nothing either), a list only names them — so they fall through to
        // the no-prompt arm below by name.
        tools::BASH_SEND_TOOL => {
            let args: tools::SendArgs = tools::parse_args(&call.arguments).ok()?;
            session_request(&args.session_id, &args.input, agent, describe)
        }
        // The legacy tool: only a call that types something asks.
        tools::BASH_SESSION_TOOL_NAME => {
            let args: SessionArgs = tools::parse_args(&call.arguments).ok()?;
            session_request(
                &args.session_id,
                args.input.as_deref().unwrap_or_default(),
                agent,
                describe,
            )
        }
        // An MCP call asks too (`docs/mcp.md`): the wire name is the target
        // (the rule key option 2 remembers), the arguments the one-line
        // `key: "value"` form the resulting cell header shows — so the prompt
        // and the cell name the call identically — and the detail the
        // server's own description of the tool.
        name if crate::mcp::is_mcp_tool(name) => Some(PermissionRequest {
            id: String::new(),
            kind: PermissionKind::Mcp,
            target: name.to_string(),
            body: crate::mcp::pretty_args(&call.arguments),
            detail: describe
                .and_then(|describe| describe(name))
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty()),
            agent,
            agent_id: None,
        }),
        _ => None,
    }
}

/// What typing `input` into session `id` asks the user — `None` when it
/// types nothing, or only a lone Ctrl+C, which interrupts rather than runs
/// anything; or when nothing answers to the id (there is nothing to type
/// into, and so nothing to approve).
fn session_request(
    id: &str,
    input: &str,
    agent: Option<String>,
    describe: Option<&DescribeTool<'_>>,
) -> Option<PermissionRequest> {
    let parts = crate::pty::keys::parse_input(input);
    if parts.is_empty() || crate::pty::keys::is_interrupt(&parts) {
        return None;
    }
    let id = id.trim().to_string();
    let command = match describe {
        Some(describe) => Some(describe(&id)?),
        None => None,
    };
    Some(PermissionRequest {
        id: String::new(),
        kind: PermissionKind::Session,
        target: id,
        body: crate::pty::keys::display_input(input),
        detail: command,
        agent,
        agent_id: None,
    })
}

/// [`crate::llm::agent::run_agent`]'s `approve` seam: consult the session
/// rules, and when they don't already cover the call, raise the prompt and
/// **block this thread** until the user answers (or the turn is cancelled).
///
/// In [`PermissionMode::Auto`] a `bash` command — or an MCP tool call
/// (`docs/mcp.md`) — the rules don't cover goes to the **classifier** instead
/// of the user (`docs/permissions.md`): an allowed verdict runs the call with
/// the [`CLASSIFIER_ALLOWED_NOTE`] on its cell, a denial rejects it with the
/// classifier's reason on both texts, and a classifier *failure* falls
/// through to the ordinary prompt below — unless the turn was cancelled out
/// from under it, which resolves like a reaped gate wait.
///
/// With no `gate` — `ALTER_ZERO_PERMISSIONS` off, or an embedder that built the
/// backend directly — every call is allowed, exactly as before the feature.
#[must_use]
#[allow(clippy::too_many_arguments)] // the gate's full seam set (docs/hooks.md)
pub fn approve_call(
    gate: Option<&PermissionGate>,
    classify: Option<&ClassifyCommand<'_>>,
    describe: Option<&DescribeTool<'_>>,
    hooks: &dyn HookSink,
    force_ask: bool,
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
    agent: Option<&str>,
    call: &ToolCallRequest,
    secrets: Option<&crate::secrets::SecretRegistry>,
) -> Approval {
    let Some(gate) = gate else {
        return Approval::Allow;
    };
    // The preview is of the call that will actually run: an `edit` whose
    // `old_string` holds a placeholder only applies once expanded, and one
    // that could not be previewed would never ask. What the prompt, the
    // allowlist and the classifier then read is redacted back to the
    // placeholders — a file the call touches may already hold a value
    // (docs/secrets.md).
    let expanded = match secrets.map(|secrets| super::secret_exec::expand_call(secrets, call)) {
        // A placeholder naming nothing stored: nothing of the call may run,
        // so there is nothing to ask — and a refusal decided here cannot be
        // overtaken by a secret saved between the answer and the run.
        Some(Err(refusal)) => {
            return Approval::Reject {
                display: refusal.clone(),
                result: refusal,
            };
        }
        Some(Ok(expanded)) => expanded,
        None => None,
    };
    let Some(mut request) = permission_request(expanded.as_ref().unwrap_or(call), agent, describe)
    else {
        return Approval::Allow;
    };
    if let Some(secrets) = secrets {
        super::secret_exec::redact_request(secrets, &mut request);
    }
    // A `PreToolUse` hook's `permissionDecision: "ask"` means *put this to the
    // user* — so the standing allowlist is skipped for this call, which is the
    // whole point of a hook saying it (`docs/hooks.md`).
    if !force_ask && gate.allows(&request) {
        return Approval::Allow;
    }
    // The session scratchpad (docs/scratchpad.md) sits with the standing rules
    // rather than beside the classifier: a `write`/`edit` under the session's
    // own temp directory changes nothing of the user's, and the system prompt
    // sends every temporary file there — so it resolves here, before the
    // prompt, wearing the note that says no human approved it. A forced ask
    // still asks, exactly as it overrides the allowlist above.
    if !force_ask && gate.scratchpad_covers(&request) {
        return Approval::AllowNoted {
            note: SCRATCHPAD_ALLOWED_NOTE.to_string(),
        };
    }
    // `PermissionRequest` (docs/hooks.md) sits exactly where the auto-mode
    // classifier does — after the standing rules, before the user — because it
    // answers the same question: *may this run without asking?* A hook that
    // abstains falls through to the classifier and then the prompt, unchanged.
    match hooks.permission_request(call, cancel) {
        Some(HookPermissionVerdict::Allow { note }) => return Approval::AllowNoted { note },
        Some(HookPermissionVerdict::Deny { display, result }) => {
            return Approval::Reject { display, result };
        }
        None => {}
    }
    // …and the classifier: `ask` means *a human decides* (Claude Code's ask
    // overrides even bypassPermissions), so auto mode's machine reviewer is
    // skipped along with the allowlist and the call goes to the prompt. The
    // PermissionRequest hook above still answers first — a user-configured
    // decider outranks the forced ask, exactly as it does in the reference's
    // permission dialog.
    if !force_ask
        && gate.mode() == PermissionMode::Auto
        && matches!(
            request.kind,
            PermissionKind::Bash | PermissionKind::Mcp | PermissionKind::Session
        )
        && let Some(classify) = classify
    {
        match classify(&request) {
            Ok(ClassifierVerdict { allow: true, .. }) => {
                return Approval::AllowNoted {
                    note: CLASSIFIER_ALLOWED_NOTE.to_string(),
                };
            }
            Ok(ClassifierVerdict { reason, .. }) => {
                let reason = Some(reason.trim()).filter(|r| !r.is_empty());
                return Approval::Reject {
                    display: classifier_denied_display(reason),
                    result: classifier_denial_result(reason),
                };
            }
            // An Esc mid-classification resolves like a reaped gate wait —
            // never a prompt raised into a turn that is being torn down.
            Err(_) if cancel.is_cancelled() => {
                return Approval::Reject {
                    display: CANCELLED_DISPLAY.to_string(),
                    result: denial_result(None),
                };
            }
            // The classifier failed (network, an unparseable reply): fall
            // back to asking the user — the safe posture, and the one that
            // still works offline.
            Err(_) => {}
        }
    }
    request.id = gate.next_id();
    let _ = tx.send(StreamEvent::Permission(request.clone()));
    match gate.wait(&request.id, &|| cancel.is_cancelled()) {
        Some(PermissionDecision::Approve) => Approval::Allow,
        Some(PermissionDecision::ApproveAlways) => {
            gate.remember(&request);
            Approval::Allow
        }
        // Tab's amended rejection: the typed instructions ride BOTH texts —
        // the cell's second line (the transcript's only record of them) and
        // the model's tool result (docs/permissions.md).
        Some(PermissionDecision::Deny(feedback)) => Approval::Reject {
            display: denied_display(&request, feedback.as_deref()),
            result: denial_result(feedback.as_deref()),
        },
        Some(PermissionDecision::Explain) => Approval::Reject {
            display: explain_display(),
            result: explain_result(&request),
        },
        // The cancel that reaps a waiting thread (Esc, `/clear`, quit)
        // resolves the same way: nothing runs. The turn is being torn down, so
        // these texts only ever reach an abandoned channel.
        None => Approval::Reject {
            display: CANCELLED_DISPLAY.to_string(),
            result: denial_result(None),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::hooks::NoHooks;
    use super::*;

    fn call(name: &str, args: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: "c".to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        }
    }

    #[test]
    fn a_read_never_asks() {
        assert!(permission_request(&call("read", r#"{"path":"a.txt"}"#), None, None).is_none());
        assert!(permission_request(&call("nonsense", "{}"), None, None).is_none());
    }

    #[test]
    fn a_bash_call_carries_the_command_and_its_description() {
        let request = permission_request(
            &call(
                "bash",
                r#"{"command":"python3 script.py","description":"Run it"}"#,
            ),
            None,
            None,
        )
        .expect("a command asks");
        assert_eq!(request.kind, PermissionKind::Bash);
        assert_eq!(request.target, "python3 script.py");
        assert_eq!(request.detail.as_deref(), Some("Run it"));
        assert!(request.body.is_empty(), "the command is its own body");
        // An empty description is no description.
        let bare = permission_request(
            &call("bash", r#"{"command":"ls","description":"  "}"#),
            Some("general-purpose"),
            None,
        )
        .unwrap();
        assert_eq!(bare.detail, None);
        assert_eq!(bare.agent.as_deref(), Some("general-purpose"));
    }

    #[test]
    fn a_write_to_a_new_file_previews_its_numbered_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.py");
        let args = serde_json::json!({
            "path": path.to_str().unwrap(),
            "content": "one\ntwo\n",
        });
        let request = permission_request(&call("write", &args.to_string()), None, None).unwrap();
        assert_eq!(request.kind, PermissionKind::Write);
        assert_eq!(request.body, "1 one\n2 two");
        assert!(!path.exists(), "asking must not write anything");
    }

    #[test]
    fn a_write_over_an_existing_file_previews_the_diff_instead() {
        // The resulting cell will be an `Updated … (+A -D)` diff, so the
        // prompt shows the same thing rather than the whole new file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.py");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let args = serde_json::json!({
            "path": path.to_str().unwrap(),
            "content": "one\nTWO\n",
        });
        let request = permission_request(&call("write", &args.to_string()), None, None).unwrap();
        assert_eq!(request.kind, PermissionKind::Edit);
        assert!(request.body.contains("-two"), "got {}", request.body);
        assert!(request.body.contains("+TWO"), "got {}", request.body);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "one\ntwo\n",
            "asking must not change the file"
        );
    }

    #[test]
    fn an_edit_previews_the_diff_it_would_apply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.py");
        std::fs::write(&path, "alpha\nbeta\ngamma\n").unwrap();
        let args = serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_string": "beta",
            "new_string": "BETA",
        });
        let request = permission_request(&call("edit", &args.to_string()), None, None).unwrap();
        assert_eq!(request.kind, PermissionKind::Edit);
        assert!(request.body.contains("-beta"), "got {}", request.body);
        assert!(request.body.contains("+BETA"), "got {}", request.body);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "alpha\nbeta\ngamma\n",
            "asking must not change the file"
        );
    }

    #[test]
    fn an_edit_that_cannot_apply_is_not_worth_asking_about() {
        // It will fail recoverably without touching the file, so there is
        // nothing to approve — and no diff to show.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.py");
        std::fs::write(&path, "alpha\n").unwrap();
        let args = serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_string": "nowhere",
            "new_string": "x",
        });
        assert!(permission_request(&call("edit", &args.to_string()), None, None).is_none());
        // …as is an edit to a file that isn't there.
        let missing = serde_json::json!({
            "path": dir.path().join("gone.py").to_str().unwrap(),
            "old_string": "a",
            "new_string": "b",
        });
        assert!(permission_request(&call("edit", &missing.to_string()), None, None).is_none());
    }

    #[test]
    fn no_gate_means_every_call_runs_as_before() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let approval = approve_call(
            None,
            None,
            None,
            &NoHooks,
            false,
            &tx,
            &CancelToken::new(),
            None,
            &call("bash", r#"{"command":"rm -rf /"}"#),
            None,
        );
        assert_eq!(approval, Approval::Allow);
        assert!(rx.try_recv().is_err(), "and nothing is asked");
    }

    #[test]
    fn an_allow_listed_call_never_reaches_the_prompt() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = PermissionGate::new();
        let call = call("bash", r#"{"command":"cargo test"}"#);
        gate.remember(&permission_request(&call, None, None).unwrap());
        assert_eq!(
            approve_call(
                Some(&gate),
                None,
                None,
                &NoHooks,
                false,
                &tx,
                &CancelToken::new(),
                None,
                &call,
                None
            ),
            Approval::Allow
        );
        assert!(rx.try_recv().is_err(), "no request was raised");
    }

    #[test]
    fn a_write_inside_the_scratchpad_runs_unasked_with_a_note() {
        // docs/scratchpad.md: the session's own temp dir is outside the user's
        // project, so a file change there resolves before the prompt is ever
        // raised — visibly, wearing the dim note.
        let dir = std::env::temp_dir().join("alter-zero-0/s1/scratchpad");
        let path = dir.join("notes.md");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = PermissionGate::new();
        gate.set_scratchpad(Some(dir));
        let call = call(
            "write",
            &serde_json::json!({"path": path, "content": "scratch"}).to_string(),
        );
        // Pre-cancelled deliberately: the exemption resolves before the gate
        // is ever consulted, so this changes nothing here — but a regression
        // that raises the prompt fails fast instead of blocking the suite.
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(
            approve_call(
                Some(&gate),
                None,
                None,
                &NoHooks,
                false,
                &tx,
                &cancel,
                None,
                &call,
                None
            ),
            Approval::AllowNoted {
                note: SCRATCHPAD_ALLOWED_NOTE.to_string()
            }
        );
        assert!(rx.try_recv().is_err(), "no request was raised");
    }

    #[test]
    fn a_write_outside_the_scratchpad_still_asks() {
        let dir = std::env::temp_dir().join("alter-zero-0/s1/scratchpad");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = PermissionGate::new();
        gate.set_scratchpad(Some(dir));
        let cancel = CancelToken::new();
        let call = call(
            "write",
            r#"{"path":"/home/user/proj/main.rs","content":"fn main() {}"}"#,
        );
        let waiter = {
            let (gate, tx, cancel, call) = (gate.clone(), tx.clone(), cancel.clone(), call.clone());
            std::thread::spawn(move || {
                approve_call(
                    Some(&gate),
                    None,
                    None,
                    &NoHooks,
                    false,
                    &tx,
                    &cancel,
                    None,
                    &call,
                    None,
                )
            })
        };
        let event = loop {
            if let Ok(event) = rx.try_recv() {
                break event;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let StreamEvent::Permission(request) = event else {
            panic!("expected a permission request, got {event:?}");
        };
        assert_eq!(request.target, "/home/user/proj/main.rs");
        gate.resolve(&request.id, PermissionDecision::Approve);
        assert_eq!(waiter.join().unwrap(), Approval::Allow);
    }

    #[test]
    fn a_forced_ask_still_prompts_for_a_scratchpad_write() {
        // A PreToolUse hook's `permissionDecision: "ask"` means *a human
        // decides* — it outranks the standing allowlist, and the scratchpad
        // exemption sits right beside it (docs/hooks.md).
        let dir = std::env::temp_dir().join("alter-zero-0/s1/scratchpad");
        let path = dir.join("notes.md");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = PermissionGate::new();
        gate.set_scratchpad(Some(dir));
        let cancel = CancelToken::new();
        cancel.cancel(); // reap the wait at once — we only care that it waits
        let approval = approve_call(
            Some(&gate),
            None,
            None,
            &NoHooks,
            true,
            &tx,
            &cancel,
            None,
            &call(
                "write",
                &serde_json::json!({"path": path, "content": "scratch"}).to_string(),
            ),
            None,
        );
        assert!(matches!(approval, Approval::Reject { .. }), "{approval:?}");
        assert!(rx.try_recv().is_ok(), "the prompt was raised");
    }

    #[test]
    fn a_request_is_raised_and_the_posted_decision_comes_back() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = PermissionGate::new();
        let cancel = CancelToken::new();
        let call = call("bash", r#"{"command":"python3 script.py"}"#);
        let waiter = {
            let (gate, tx, cancel, call) = (gate.clone(), tx.clone(), cancel.clone(), call.clone());
            std::thread::spawn(move || {
                approve_call(
                    Some(&gate),
                    None,
                    None,
                    &NoHooks,
                    false,
                    &tx,
                    &cancel,
                    None,
                    &call,
                    None,
                )
            })
        };
        // The event names the request; answering it under that id unblocks.
        let event = loop {
            if let Ok(event) = rx.try_recv() {
                break event;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let StreamEvent::Permission(request) = event else {
            panic!("expected a permission request, got {event:?}");
        };
        assert_eq!(request.target, "python3 script.py");
        gate.resolve(&request.id, PermissionDecision::ApproveAlways);
        assert_eq!(waiter.join().unwrap(), Approval::Allow);
        // …and option 2 remembered the scope, so the next identical call is silent.
        assert_eq!(
            approve_call(
                Some(&gate),
                None,
                None,
                &NoHooks,
                false,
                &tx,
                &cancel,
                None,
                &call,
                None
            ),
            Approval::Allow
        );
    }

    #[test]
    fn a_rejection_carries_the_short_cell_text_and_the_long_instruction() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = PermissionGate::new();
        let cancel = CancelToken::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.py");
        let args = serde_json::json!({ "path": path.to_str().unwrap(), "content": "x\n" });
        let call = call("write", &args.to_string());
        let waiter = {
            let (gate, tx, cancel, call) = (gate.clone(), tx.clone(), cancel.clone(), call.clone());
            std::thread::spawn(move || {
                approve_call(
                    Some(&gate),
                    None,
                    None,
                    &NoHooks,
                    false,
                    &tx,
                    &cancel,
                    None,
                    &call,
                    None,
                )
            })
        };
        let request = loop {
            if let Ok(StreamEvent::Permission(request)) = rx.try_recv() {
                break request;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        gate.resolve(
            &request.id,
            PermissionDecision::Deny(Some("use pathlib".to_string())),
        );
        let Approval::Reject { display, result } = waiter.join().unwrap() else {
            panic!("expected a rejection");
        };
        // Tab's typed instructions ride both texts: the cell's second line
        // (the transcript's only record of them) and the model's tool result.
        assert_eq!(
            display,
            "User rejected write to hello.py\nInstructions: use pathlib"
        );
        assert!(result.contains("use pathlib"), "got {result}");
    }

    // ===== interactive-session input (docs/interactive-shell.md) =====

    #[test]
    fn typing_into_a_session_asks_with_its_command_beside_the_input() {
        let describe = |key: &str| (key == "b7x2k9m1q").then(|| "python3".to_string());
        let request = permission_request(
            &call(
                "bash_session",
                r#"{"session_id":"b7x2k9m1q","input":"print(1)\n"}"#,
            ),
            Some("general-purpose"),
            Some(&describe),
        )
        .expect("typing asks");
        assert_eq!(request.kind, PermissionKind::Session);
        assert_eq!(request.target, "b7x2k9m1q");
        assert_eq!(request.body, "print(1)⏎", "the input as the cell shows it");
        assert_eq!(request.detail.as_deref(), Some("python3"));
        assert_eq!(request.agent.as_deref(), Some("general-purpose"));
        // Typing, then a kill: the typing still asks.
        assert!(
            permission_request(
                &call(
                    "bash_session",
                    r#"{"session_id":"b7x2k9m1q","input":"q","kill":true}"#
                ),
                None,
                Some(&describe),
            )
            .is_some()
        );
    }

    #[test]
    fn waiting_ending_and_interrupting_a_session_never_ask() {
        // Nothing is typed: a wait reads, a kill ends a command the model
        // started, and a lone Ctrl+C only interrupts it — the ↓ manager's
        // own stop, which asks nothing either.
        let describe = |_: &str| Some("python3".to_string());
        for args in [
            r#"{"session_id":"b1"}"#,
            r#"{"session_id":"b1","input":""}"#,
            r#"{"session_id":"b1","kill":true}"#,
            r#"{"session_id":"b1","input":"<C-c>"}"#,
        ] {
            assert!(
                permission_request(&call("bash_session", args), None, Some(&describe)).is_none(),
                "{args}"
            );
        }
        // A session nothing answers to has nothing to type into: the
        // executor refuses the call, so there is nothing to approve.
        let nobody = |_: &str| None;
        assert!(
            permission_request(
                &call("bash_session", r#"{"session_id":"bnope","input":"y\n"}"#),
                None,
                Some(&nobody),
            )
            .is_none()
        );
    }

    #[test]
    fn the_agent_companions_never_ask() {
        // A message to an agent is a prompt, not a command — the agent's
        // own tool calls ask, as they always did — and a wait, a report, a
        // stop or a list runs nothing (docs/agent-tools.md).
        let describe = |_: &str| Some("Fetch Warsaw weather".to_string());
        for (name, args) in [
            (
                tools::AGENT_SEND_TOOL,
                r#"{"agent_id":"a1","message":"rm -rf ~"}"#,
            ),
            (tools::AGENT_WAIT_TOOL, r#"{"agent_id":"a1","wait":30}"#),
            (tools::AGENT_OUTPUT_TOOL, r#"{"agent_id":"a1"}"#),
            (tools::AGENT_KILL_TOOL, r#"{"agent_id":"a1"}"#),
            (tools::AGENT_LIST_TOOL, "{}"),
        ] {
            assert!(
                permission_request(&call(name, args), None, Some(&describe)).is_none(),
                "{name}"
            );
        }
    }

    #[test]
    fn auto_mode_classifies_session_input() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = gate_in(crate::permission::PermissionMode::Auto);
        let describe = |_: &str| Some("bash -i".to_string());
        let classify = |request: &PermissionRequest| {
            assert_eq!(request.kind, PermissionKind::Session);
            assert_eq!(request.body, "rm -rf ~⏎");
            Ok(ClassifierVerdict {
                allow: false,
                reason: "deletes the home directory".to_string(),
            })
        };
        let approval = approve_call(
            Some(&gate),
            Some(&classify),
            Some(&describe),
            &NoHooks,
            false,
            &tx,
            &CancelToken::new(),
            None,
            &call(
                "bash_session",
                r#"{"session_id":"b1","input":"rm -rf ~\n"}"#,
            ),
            None,
        );
        assert!(
            matches!(approval, Approval::Reject { .. }),
            "typed commands meet the classifier like run ones: {approval:?}"
        );
        assert!(rx.try_recv().is_err(), "the user was never asked");
    }

    // ===== the auto mode classifier (docs/permissions.md) =====

    /// A gate pre-set to `mode`, plus the plumbing every approve test needs.
    fn gate_in(mode: crate::permission::PermissionMode) -> PermissionGate {
        let gate = PermissionGate::new();
        gate.set_mode(mode);
        gate
    }

    #[test]
    fn auto_mode_runs_a_classifier_allowed_command_with_the_note() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = gate_in(crate::permission::PermissionMode::Auto);
        let classify = |request: &PermissionRequest| {
            assert_eq!(request.target, "ls -la");
            assert_eq!(request.detail.as_deref(), Some("List files"));
            Ok(ClassifierVerdict {
                allow: true,
                reason: "read-only".to_string(),
            })
        };
        let approval = approve_call(
            Some(&gate),
            Some(&classify),
            None,
            &NoHooks,
            false,
            &tx,
            &CancelToken::new(),
            None,
            &call("bash", r#"{"command":"ls -la","description":"List files"}"#),
            None,
        );
        assert_eq!(
            approval,
            Approval::AllowNoted {
                note: CLASSIFIER_ALLOWED_NOTE.to_string(),
            }
        );
        assert!(rx.try_recv().is_err(), "the user was never asked");
    }

    #[test]
    fn auto_mode_rejects_a_classifier_denied_command_with_the_reason() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = gate_in(crate::permission::PermissionMode::Auto);
        let classify = |_: &PermissionRequest| {
            Ok(ClassifierVerdict {
                allow: false,
                reason: "privilege escalation".to_string(),
            })
        };
        let approval = approve_call(
            Some(&gate),
            Some(&classify),
            None,
            &NoHooks,
            false,
            &tx,
            &CancelToken::new(),
            None,
            &call("bash", r#"{"command":"sudo rm -rf /"}"#),
            None,
        );
        let Approval::Reject { display, result } = approval else {
            panic!("a denied command must not run: {approval:?}");
        };
        assert_eq!(
            display,
            "Denied by auto mode classifier\nReason: privilege escalation"
        );
        assert!(
            result.contains("privilege escalation") && result.contains("was not executed"),
            "the model reads the reason and the outcome: {result}"
        );
        assert!(rx.try_recv().is_err(), "the user was never asked");
    }

    #[test]
    fn auto_mode_never_classifies_a_file_change_or_an_allowlisted_command() {
        // Files are covered by the mode itself (the edit-mode rule), and an
        // allow-listed command short-circuits before the classifier — both
        // must come back as a plain Allow with the classifier untouched.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = gate_in(crate::permission::PermissionMode::Auto);
        let classify = |_: &PermissionRequest| -> Result<ClassifierVerdict, String> {
            panic!("the classifier must not be consulted")
        };
        let dir = tempfile::tempdir().unwrap();
        let args = serde_json::json!({
            "path": dir.path().join("a.py").to_str().unwrap(),
            "content": "x\n",
        });
        assert_eq!(
            approve_call(
                Some(&gate),
                Some(&classify),
                None,
                &NoHooks,
                false,
                &tx,
                &CancelToken::new(),
                None,
                &call("write", &args.to_string()),
                None,
            ),
            Approval::Allow
        );
        let listed = call("bash", r#"{"command":"cargo test"}"#);
        gate.remember(&permission_request(&listed, None, None).unwrap());
        assert_eq!(
            approve_call(
                Some(&gate),
                Some(&classify),
                None,
                &NoHooks,
                false,
                &tx,
                &CancelToken::new(),
                None,
                &listed,
                None,
            ),
            Approval::Allow
        );
    }

    #[test]
    fn manual_and_edit_modes_never_consult_the_classifier() {
        // The classifier belongs to auto mode alone: in every other mode the
        // command goes to the user exactly as before the feature.
        for mode in [
            crate::permission::PermissionMode::Manual,
            crate::permission::PermissionMode::Edit,
        ] {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let gate = gate_in(mode);
            let cancel = CancelToken::new();
            let classify = move |_: &PermissionRequest| -> Result<ClassifierVerdict, String> {
                panic!("the classifier must not be consulted in {mode:?}")
            };
            let waiter = {
                let (gate, tx, cancel) = (gate.clone(), tx.clone(), cancel.clone());
                std::thread::spawn(move || {
                    approve_call(
                        Some(&gate),
                        Some(&classify),
                        None,
                        &NoHooks,
                        false,
                        &tx,
                        &cancel,
                        None,
                        &call("bash", r#"{"command":"python3 x.py"}"#),
                        None,
                    )
                })
            };
            let request = loop {
                if let Ok(StreamEvent::Permission(request)) = rx.try_recv() {
                    break request;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            };
            gate.resolve(&request.id, PermissionDecision::Approve);
            assert_eq!(waiter.join().unwrap(), Approval::Allow);
        }
    }

    #[test]
    fn master_mode_allows_everything_without_asking_or_classifying() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = gate_in(crate::permission::PermissionMode::Master);
        let classify = |_: &PermissionRequest| -> Result<ClassifierVerdict, String> {
            panic!("master mode has no classifier")
        };
        let dir = tempfile::tempdir().unwrap();
        let args = serde_json::json!({
            "path": dir.path().join("a.py").to_str().unwrap(),
            "content": "x\n",
        });
        for request in [
            call("bash", r#"{"command":"sudo rm -rf /"}"#),
            call("write", &args.to_string()),
        ] {
            assert_eq!(
                approve_call(
                    Some(&gate),
                    Some(&classify),
                    None,
                    &NoHooks,
                    false,
                    &tx,
                    &CancelToken::new(),
                    None,
                    &request,
                    None,
                ),
                Approval::Allow
            );
        }
        assert!(rx.try_recv().is_err(), "nothing was ever asked");
    }

    #[test]
    fn force_ask_goes_to_the_user_never_to_the_classifier() {
        // A PreToolUse hook's `permissionDecision: "ask"` means *a human
        // decides* — Claude Code's ask even overrides bypassPermissions. Auto
        // mode's classifier answering it instead would mean the hook's demand
        // for judgment was met by another automation.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = gate_in(crate::permission::PermissionMode::Auto);
        let cancel = CancelToken::new();
        let classify = |_: &PermissionRequest| -> Result<ClassifierVerdict, String> {
            panic!("the classifier must not answer a forced ask")
        };
        let waiter = {
            let (gate, tx, cancel) = (gate.clone(), tx.clone(), cancel.clone());
            std::thread::spawn(move || {
                approve_call(
                    Some(&gate),
                    Some(&classify),
                    None,
                    &NoHooks,
                    true,
                    &tx,
                    &cancel,
                    None,
                    &call("bash", r#"{"command":"ls"}"#),
                    None,
                )
            })
        };
        // Bounded, so a classifier consultation (the red state) fails the
        // test with its panic instead of hanging it on a prompt that never
        // comes.
        let mut request = None;
        for _ in 0..400 {
            if let Ok(StreamEvent::Permission(r)) = rx.try_recv() {
                request = Some(r);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let Some(request) = request else {
            match waiter.join() {
                Err(panic) => std::panic::resume_unwind(panic),
                Ok(approval) => panic!("no prompt was raised, got {approval:?}"),
            }
        };
        gate.resolve(&request.id, PermissionDecision::Approve);
        assert_eq!(waiter.join().unwrap(), Approval::Allow);
    }

    #[test]
    fn a_classifier_failure_falls_back_to_the_prompt() {
        // Network down, unparseable verdict — auto mode degrades to asking
        // the user, never to silently allowing (or wedging the thread).
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = gate_in(crate::permission::PermissionMode::Auto);
        let cancel = CancelToken::new();
        let classify = |_: &PermissionRequest| Err("boom".to_string());
        let waiter = {
            let (gate, tx, cancel) = (gate.clone(), tx.clone(), cancel.clone());
            std::thread::spawn(move || {
                approve_call(
                    Some(&gate),
                    Some(&classify),
                    None,
                    &NoHooks,
                    false,
                    &tx,
                    &cancel,
                    None,
                    &call("bash", r#"{"command":"python3 x.py"}"#),
                    None,
                )
            })
        };
        let request = loop {
            if let Ok(StreamEvent::Permission(request)) = rx.try_recv() {
                break request;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        gate.resolve(&request.id, PermissionDecision::Approve);
        assert_eq!(waiter.join().unwrap(), Approval::Allow);
    }

    #[test]
    fn a_classifier_failure_on_a_cancelled_turn_rejects_without_a_prompt() {
        // Esc lands while the classifier request is in flight: the classify
        // call errors out (cancelled), and the resolution is the reaped-wait
        // rejection — no prompt may be raised into the torn-down turn.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = gate_in(crate::permission::PermissionMode::Auto);
        let cancel = CancelToken::new();
        cancel.cancel();
        let classify = |_: &PermissionRequest| Err("cancelled".to_string());
        let approval = approve_call(
            Some(&gate),
            Some(&classify),
            None,
            &NoHooks,
            false,
            &tx,
            &cancel,
            None,
            &call("bash", r#"{"command":"python3 x.py"}"#),
            None,
        );
        assert!(
            matches!(approval, Approval::Reject { .. }),
            "a cancelled classification never allows the call: {approval:?}"
        );
        assert!(rx.try_recv().is_err(), "and no prompt was raised");
    }

    #[test]
    fn auto_mode_without_a_classifier_falls_back_to_the_prompt() {
        // A backend with no classifier attached (the dummy goes its own way;
        // an embedder) still asks rather than allowing or wedging.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = gate_in(crate::permission::PermissionMode::Auto);
        let cancel = CancelToken::new();
        let waiter = {
            let (gate, tx, cancel) = (gate.clone(), tx.clone(), cancel.clone());
            std::thread::spawn(move || {
                approve_call(
                    Some(&gate),
                    None,
                    None,
                    &NoHooks,
                    false,
                    &tx,
                    &cancel,
                    None,
                    &call("bash", r#"{"command":"python3 x.py"}"#),
                    None,
                )
            })
        };
        let request = loop {
            if let Ok(StreamEvent::Permission(request)) = rx.try_recv() {
                break request;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        gate.resolve(&request.id, PermissionDecision::Approve);
        assert_eq!(waiter.join().unwrap(), Approval::Allow);
    }

    #[test]
    fn a_cancelled_turn_releases_the_waiting_call_without_running_it() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = PermissionGate::new();
        let cancel = CancelToken::new();
        let call = call("bash", r#"{"command":"sleep 100"}"#);
        let waiter = {
            let (gate, tx, cancel, call) = (gate.clone(), tx.clone(), cancel.clone(), call.clone());
            std::thread::spawn(move || {
                approve_call(
                    Some(&gate),
                    None,
                    None,
                    &NoHooks,
                    false,
                    &tx,
                    &cancel,
                    None,
                    &call,
                    None,
                )
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(30));
        cancel.cancel();
        assert!(
            matches!(waiter.join().unwrap(), Approval::Reject { .. }),
            "a reaped wait never allows the call"
        );
    }
    // ===== the auto mode classifier on MCP calls (docs/mcp.md) =====

    #[test]
    fn auto_mode_classifies_an_mcp_call_and_runs_it_with_the_note() {
        // Auto mode's machine reviewer answers for a server tool exactly as it
        // does for a command: the user is never prompted, and the classifier
        // reads the whole request — the wire name, the arguments, and the
        // server's own description of the tool.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = gate_in(crate::permission::PermissionMode::Auto);
        let describe = |wire: &str| {
            (wire == "mcp__deepwiki__ask_question").then(|| "Ask about a repo.".to_string())
        };
        let classify = |request: &PermissionRequest| {
            assert_eq!(request.kind, PermissionKind::Mcp);
            assert_eq!(request.target, "mcp__deepwiki__ask_question");
            assert_eq!(request.body, r#"repoName: "a/b", question: "What?""#);
            assert_eq!(request.detail.as_deref(), Some("Ask about a repo."));
            Ok(ClassifierVerdict {
                allow: true,
                reason: "read-only query".to_string(),
            })
        };
        let cancel = CancelToken::new();
        let waiter = {
            let (gate, tx, cancel) = (gate.clone(), tx.clone(), cancel.clone());
            std::thread::spawn(move || {
                approve_call(
                    Some(&gate),
                    Some(&classify),
                    Some(&describe),
                    &NoHooks,
                    false,
                    &tx,
                    &cancel,
                    None,
                    &call(
                        "mcp__deepwiki__ask_question",
                        r#"{"repoName":"a/b","question":"What?"}"#,
                    ),
                    None,
                )
            })
        };
        // Bounded: a raised prompt (the red state) fails the test instead of
        // hanging it on a question nobody answers.
        let approval = wait_bounded(waiter, &gate, &mut rx);
        assert_eq!(
            approval,
            Approval::AllowNoted {
                note: CLASSIFIER_ALLOWED_NOTE.to_string(),
            }
        );
        assert!(rx.try_recv().is_err(), "the user was never asked");
    }

    /// Join `waiter` while watching `rx` for a permission prompt: a prompt is
    /// resolved (to unblock the thread) and failed loudly — auto mode's whole
    /// point is that the user is not asked — and a waiter panic propagates.
    fn wait_bounded(
        waiter: std::thread::JoinHandle<Approval>,
        gate: &PermissionGate,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
    ) -> Approval {
        for _ in 0..400 {
            if waiter.is_finished() {
                return waiter.join().unwrap();
            }
            if let Ok(StreamEvent::Permission(request)) = rx.try_recv() {
                gate.resolve(&request.id, PermissionDecision::Deny(None));
                let _ = waiter.join();
                panic!("the user was asked — auto mode must classify this call instead");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("approve_call neither finished nor asked within the window");
    }

    #[test]
    fn auto_mode_rejects_a_classifier_denied_mcp_call_with_the_reason() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = gate_in(crate::permission::PermissionMode::Auto);
        let classify = |_: &PermissionRequest| {
            Ok(ClassifierVerdict {
                allow: false,
                reason: "sends data to an external service".to_string(),
            })
        };
        let cancel = CancelToken::new();
        let waiter = {
            let (gate, tx, cancel) = (gate.clone(), tx.clone(), cancel.clone());
            std::thread::spawn(move || {
                approve_call(
                    Some(&gate),
                    Some(&classify),
                    None,
                    &NoHooks,
                    false,
                    &tx,
                    &cancel,
                    None,
                    &call("mcp__mail__send_email", r#"{"to":"x@y.z"}"#),
                    None,
                )
            })
        };
        let approval = wait_bounded(waiter, &gate, &mut rx);
        let Approval::Reject { display, result } = approval else {
            panic!("a denied MCP call must not run: {approval:?}");
        };
        assert_eq!(
            display,
            "Denied by auto mode classifier\nReason: sends data to an external service"
        );
        assert!(
            result.contains("sends data to an external service")
                && result.contains("was not executed"),
            "the model reads the reason and the outcome: {result}"
        );
        assert!(rx.try_recv().is_err(), "the user was never asked");
    }

    #[test]
    fn an_mcp_classifier_failure_falls_back_to_the_prompt() {
        // Same degradation as a command's: network down or an unparseable
        // verdict asks the user — never a silent allow, never a wedged thread.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = gate_in(crate::permission::PermissionMode::Auto);
        let cancel = CancelToken::new();
        let classify = |_: &PermissionRequest| Err("boom".to_string());
        let waiter = {
            let (gate, tx, cancel) = (gate.clone(), tx.clone(), cancel.clone());
            std::thread::spawn(move || {
                approve_call(
                    Some(&gate),
                    Some(&classify),
                    None,
                    &NoHooks,
                    false,
                    &tx,
                    &cancel,
                    None,
                    &call("mcp__s__t", "{}"),
                    None,
                )
            })
        };
        let request = loop {
            if let Ok(StreamEvent::Permission(request)) = rx.try_recv() {
                break request;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(request.kind, PermissionKind::Mcp);
        gate.resolve(&request.id, PermissionDecision::Approve);
        assert_eq!(waiter.join().unwrap(), Approval::Allow);
    }

    #[test]
    fn an_allow_listed_mcp_call_never_reaches_the_classifier() {
        // Option 2's exact wire-name rule short-circuits before the
        // classifier in auto mode, exactly as a command's allowlist does.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = gate_in(crate::permission::PermissionMode::Auto);
        let classify = |_: &PermissionRequest| -> Result<ClassifierVerdict, String> {
            panic!("the classifier must not be consulted for an allow-listed tool")
        };
        let listed = call("mcp__deepwiki__ask_question", r#"{"q":"?"}"#);
        gate.remember(&permission_request(&listed, None, None).unwrap());
        assert_eq!(
            approve_call(
                Some(&gate),
                Some(&classify),
                None,
                &NoHooks,
                false,
                &tx,
                &CancelToken::new(),
                None,
                &listed,
                None,
            ),
            Approval::Allow
        );
        assert!(rx.try_recv().is_err(), "no request was raised");
    }

    #[test]
    fn manual_and_edit_modes_still_prompt_for_mcp_calls() {
        // The classifier belongs to auto mode alone — in every other asking
        // mode a server tool goes to the user exactly as before.
        for mode in [
            crate::permission::PermissionMode::Manual,
            crate::permission::PermissionMode::Edit,
        ] {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let gate = gate_in(mode);
            let cancel = CancelToken::new();
            let classify = move |_: &PermissionRequest| -> Result<ClassifierVerdict, String> {
                panic!("the classifier must not be consulted in {mode:?}")
            };
            let waiter = {
                let (gate, tx, cancel) = (gate.clone(), tx.clone(), cancel.clone());
                std::thread::spawn(move || {
                    approve_call(
                        Some(&gate),
                        Some(&classify),
                        None,
                        &NoHooks,
                        false,
                        &tx,
                        &cancel,
                        None,
                        &call("mcp__s__t", "{}"),
                        None,
                    )
                })
            };
            let request = loop {
                if let Ok(StreamEvent::Permission(request)) = rx.try_recv() {
                    break request;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            };
            gate.resolve(&request.id, PermissionDecision::Approve);
            assert_eq!(waiter.join().unwrap(), Approval::Allow);
        }
    }

    #[test]
    fn an_mcp_call_raises_a_request_with_the_wire_name_args_and_description() {
        let describe = |wire: &str| {
            (wire == "mcp__deepwiki__ask_question").then(|| "  Ask about a repo.  ".to_string())
        };
        let request = permission_request(
            &call(
                "mcp__deepwiki__ask_question",
                r#"{"repoName":"a/b","question":"What?"}"#,
            ),
            None,
            Some(&describe),
        )
        .expect("MCP calls ask");
        assert_eq!(request.kind, PermissionKind::Mcp);
        assert_eq!(request.target, "mcp__deepwiki__ask_question");
        // The body is what the cell header will show — the model's own key
        // order, the one-line `key: "value"` form (`docs/mcp.md`).
        assert_eq!(request.body, r#"repoName: "a/b", question: "What?""#);
        // …and the detail is the server's own description, trimmed.
        assert_eq!(request.detail.as_deref(), Some("Ask about a repo."));
        // An argument-less call has no body, and an unknown tool no detail.
        let bare = permission_request(&call("mcp__s__t", "{}"), None, Some(&describe)).unwrap();
        assert!(bare.body.is_empty());
        assert_eq!(bare.detail, None);
        // A non-MCP unknown tool still never asks.
        assert!(permission_request(&call("mystery", "{}"), None, None).is_none());
    }

    // ===== secrets (docs/secrets.md) =====

    fn secrets() -> crate::secrets::SecretRegistry {
        let mut store = crate::secrets::SecretStore::new();
        for (name, value) in [
            ("ROOT_PASSWORD", "hunter22"),
            ("TOKEN", "sk-live-0123456789"),
        ] {
            store
                .apply(&crate::secrets::SecretDraft {
                    original: None,
                    name: name.into(),
                    value: Some(crate::secrets::SecretValue::new(value)),
                    context: String::new(),
                })
                .unwrap();
        }
        crate::secrets::SecretRegistry::new(store)
    }

    /// Put `call` to a fresh gate with the session's secrets, on a thread:
    /// the request the prompt was raised with, and the approval once
    /// `decision` answers it.
    fn ask_with_secrets(
        call: ToolCallRequest,
        decision: PermissionDecision,
    ) -> (PermissionRequest, Approval) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = PermissionGate::new();
        let cancel = CancelToken::new();
        let secrets = secrets();
        let waiter = {
            let (gate, tx, cancel) = (gate.clone(), tx.clone(), cancel.clone());
            std::thread::spawn(move || {
                approve_call(
                    Some(&gate),
                    None,
                    None,
                    &NoHooks,
                    false,
                    &tx,
                    &cancel,
                    None,
                    &call,
                    Some(&secrets),
                )
            })
        };
        let started = std::time::Instant::now();
        let request = loop {
            if let Ok(StreamEvent::Permission(request)) = rx.try_recv() {
                break request;
            }
            assert!(
                !waiter.is_finished() && started.elapsed() < std::time::Duration::from_secs(5),
                "no prompt was raised"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        gate.resolve(&request.id, decision);
        (request, waiter.join().unwrap())
    }

    #[test]
    fn an_edit_through_a_placeholder_still_asks_with_a_redacted_preview() {
        // The file holds the value; the model's `old_string` holds the
        // placeholder. Previewed as the model wrote it, the edit would not
        // apply — and an edit with no preview is one that never asks, which
        // the expansion would then have run unprompted.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.env");
        std::fs::write(&path, "HOST=db\nPASSWORD=hunter22\nPORT=5432\n").unwrap();
        let args = serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_string": "PASSWORD=<secret:ROOT_PASSWORD>",
            "new_string": "PASSWORD=<secret:TOKEN>",
        });
        let (request, approval) =
            ask_with_secrets(call("edit", &args.to_string()), PermissionDecision::Approve);
        assert_eq!(approval, Approval::Allow);
        assert_eq!(request.kind, PermissionKind::Edit);
        assert!(
            request.body.contains("-PASSWORD=<secret:ROOT_PASSWORD>"),
            "{}",
            request.body
        );
        assert!(
            request.body.contains("+PASSWORD=<secret:TOKEN>"),
            "{}",
            request.body
        );
        assert!(!request.body.contains("hunter22"), "{}", request.body);
        assert!(!request.body.contains("sk-live"), "{}", request.body);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "HOST=db\nPASSWORD=hunter22\nPORT=5432\n",
            "asking must not change the file"
        );
    }

    #[test]
    fn a_write_over_a_file_holding_a_value_previews_it_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "TOKEN=sk-live-0123456789\n").unwrap();
        let args = serde_json::json!({
            "path": path.to_str().unwrap(),
            "content": "TOKEN=<secret:TOKEN>\nDEBUG=1\n",
        });
        let (request, _) = ask_with_secrets(
            call("write", &args.to_string()),
            PermissionDecision::Approve,
        );
        assert!(
            request.body.contains("TOKEN=<secret:TOKEN>"),
            "{}",
            request.body
        );
        assert!(request.body.contains("+DEBUG=1"), "{}", request.body);
        assert!(!request.body.contains("sk-live"), "{}", request.body);
    }

    #[test]
    fn a_call_naming_a_secret_that_is_not_stored_is_refused_without_asking() {
        // Nothing of it may run, so there is nothing to put to the user — and
        // a refusal decided here cannot be overtaken by a secret saved between
        // the answer and the run.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = PermissionGate::new();
        let secrets = secrets();
        // A torn-down turn, so a prompt raised by mistake resolves at once
        // instead of waiting on nobody.
        let cancel = CancelToken::new();
        cancel.cancel();
        for (name, arguments) in [
            ("bash", r#"{"command":"echo <secret:TOKN>"}"#),
            (
                "bashsend",
                r#"{"session_id":"b1","input":"<secret:TOKN>\n"}"#,
            ),
        ] {
            let approval = approve_call(
                Some(&gate),
                None,
                None,
                &NoHooks,
                false,
                &tx,
                &cancel,
                None,
                &call(name, arguments),
                Some(&secrets),
            );
            let Approval::Reject { display, result } = approval else {
                panic!("{name}: {approval:?}");
            };
            assert!(
                display.starts_with("Not run: <secret:TOKN> is not a stored secret."),
                "{display}"
            );
            assert_eq!(display, result);
        }
        assert!(rx.try_recv().is_err(), "a prompt was raised");
    }

    #[test]
    fn a_command_asks_and_is_remembered_by_its_placeholder() {
        let command = r#"{"command":"curl -H 'Authorization: Bearer <secret:TOKEN>' https://api.example.com"}"#;
        let (request, approval) =
            ask_with_secrets(call("bash", command), PermissionDecision::Approve);
        assert_eq!(approval, Approval::Allow);
        assert_eq!(
            request.target,
            "curl -H 'Authorization: Bearer <secret:TOKEN>' https://api.example.com"
        );
    }
}
