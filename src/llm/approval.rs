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

use super::tools::{
    self, BashArgs, EditArgs, ToolCallRequest, WriteArgs, render_numbered_content,
    render_numbered_diff,
};
use crate::permission::{
    Approval, PermissionDecision, PermissionGate, PermissionKind, PermissionRequest, denial_result,
    denied_display, explain_display, explain_result,
};
use crate::stream::{CancelToken, StreamEvent};

/// The display + model-facing texts used when a turn is cancelled out from
/// under a waiting request — the call never ran, and the events go to an
/// already-swapped channel, so this is only ever a well-formed placeholder.
const CANCELLED_DISPLAY: &str = "Interrupted by user";

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
        _ => None,
    }
}

/// [`crate::llm::agent::run_agent`]'s `approve` seam: consult the session
/// rules, and when they don't already cover the call, raise the prompt and
/// **block this thread** until the user answers (or the turn is cancelled).
///
/// With no `gate` — `ALTER_ZERO_PERMISSIONS` off, or an embedder that built the
/// backend directly — every call is allowed, exactly as before the feature.
#[must_use]
pub fn approve_call(
    gate: Option<&PermissionGate>,
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
    agent: Option<&str>,
    call: &ToolCallRequest,
) -> Approval {
    let Some(gate) = gate else {
        return Approval::Allow;
    };
    let Some(mut request) = permission_request(call, agent) else {
        return Approval::Allow;
    };
    if gate.allows(&request) {
        return Approval::Allow;
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
        assert!(permission_request(&call("read", r#"{"path":"a.txt"}"#), None).is_none());
        assert!(permission_request(&call("nonsense", "{}"), None).is_none());
    }

    #[test]
    fn a_bash_call_carries_the_command_and_its_description() {
        let request = permission_request(
            &call(
                "bash",
                r#"{"command":"python3 script.py","description":"Run it"}"#,
            ),
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
        let request = permission_request(&call("write", &args.to_string()), None).unwrap();
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
        let request = permission_request(&call("write", &args.to_string()), None).unwrap();
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
        let request = permission_request(&call("edit", &args.to_string()), None).unwrap();
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
        assert!(permission_request(&call("edit", &args.to_string()), None).is_none());
        // …as is an edit to a file that isn't there.
        let missing = serde_json::json!({
            "path": dir.path().join("gone.py").to_str().unwrap(),
            "old_string": "a",
            "new_string": "b",
        });
        assert!(permission_request(&call("edit", &missing.to_string()), None).is_none());
    }

    #[test]
    fn no_gate_means_every_call_runs_as_before() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let approval = approve_call(
            None,
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
        gate.remember(&permission_request(&call, None).unwrap());
        assert_eq!(
            approve_call(Some(&gate), &tx, &CancelToken::new(), None, &call),
            Approval::Allow
        );
        assert!(rx.try_recv().is_err(), "no request was raised");
    }

    #[test]
    fn a_request_is_raised_and_the_posted_decision_comes_back() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = PermissionGate::new();
        let cancel = CancelToken::new();
        let call = call("bash", r#"{"command":"python3 script.py"}"#);
        let waiter = {
            let (gate, tx, cancel, call) = (gate.clone(), tx.clone(), cancel.clone(), call.clone());
            std::thread::spawn(move || approve_call(Some(&gate), &tx, &cancel, None, &call))
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
            approve_call(Some(&gate), &tx, &cancel, None, &call),
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
            std::thread::spawn(move || approve_call(Some(&gate), &tx, &cancel, None, &call))
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

    #[test]
    fn a_cancelled_turn_releases_the_waiting_call_without_running_it() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = PermissionGate::new();
        let cancel = CancelToken::new();
        let call = call("bash", r#"{"command":"sleep 100"}"#);
        let waiter = {
            let (gate, tx, cancel, call) = (gate.clone(), tx.clone(), cancel.clone(), call.clone());
            std::thread::spawn(move || approve_call(Some(&gate), &tx, &cancel, None, &call))
        };
        std::thread::sleep(std::time::Duration::from_millis(30));
        cancel.cancel();
        assert!(
            matches!(waiter.join().unwrap(), Approval::Reject { .. }),
            "a reaped wait never allows the call"
        );
    }
}
