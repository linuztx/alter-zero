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
    self, BashArgs, EditArgs, ToolCallRequest, WriteArgs, render_numbered_content,
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

/// The MCP tool-description lookup an MCP prompt's body needs
/// (`docs/mcp.md`): a wire name → the server's own one-line description of
/// that tool ([`crate::llm::mcp::McpManager::tool_description`]). `None` —
/// no manager attached, or a tool it doesn't know — simply leaves the dim
/// description row out of the prompt.
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
            })
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
        }),
        _ => None,
    }
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
) -> Approval {
    let Some(gate) = gate else {
        return Approval::Allow;
    };
    let Some(mut request) = permission_request(call, agent, describe) else {
        return Approval::Allow;
    };
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
        && matches!(request.kind, PermissionKind::Bash | PermissionKind::Mcp)
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
                &call
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
                &call
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
                &call
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
}
