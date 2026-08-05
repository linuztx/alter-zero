//! The dummy's **gated** turns: the offline permission demos
//! (`docs/permissions.md`).
//!
//! Unlike [`super::turns`] these can't be a pure event list — they *ask*, and
//! then block on the shared [`PermissionGate`](crate::permission::PermissionGate)
//! exactly as a real backend's tool thread does, so `scripts/smoke.sh` can drive
//! the whole approval round trip (draft stash, options, Tab's amend, the
//! restore) with no provider attached.

use tokio::sync::mpsc::UnboundedSender;

use super::super::{CancelToken, StreamEvent, ToolCallSummary};
use super::script::chunks;
use super::{CHUNK_DELAY, nap};

/// The dummy's scripted `write` for the permission demo (`docs/permissions.md`).
const DUMMY_PERMISSION_PATH: &str = "hello.py";
const DUMMY_PERMISSION_CONTENT: &str = "#!/usr/bin/env python3\n\n\
    def main():\n    \
        name = input(\"What's your name? \")\n    \
        print(f\"Hello, {name}! Welcome to Python.\")\n\n\
    if __name__ == \"__main__\":\n    \
        main()";

/// Play the offline approval round trip: announce the `Write` call, raise the
/// request, **block on the gate** exactly as a real backend's tool thread does,
/// then resolve the cell green (approved) or red (rejected). Lets `smoke.sh`
/// drive the whole prompt — draft stash, options, restore — with no provider.
pub(super) fn dummy_permission_turn(
    gate: &crate::permission::PermissionGate,
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
) {
    use crate::permission::{PermissionDecision, PermissionKind, PermissionRequest};
    let _ = tx.send(StreamEvent::ToolBatch(vec![ToolCallSummary {
        name: "Write".to_string(),
        args: DUMMY_PERMISSION_PATH.to_string(),
    }]));
    let mut request = PermissionRequest {
        id: gate.next_id(),
        kind: PermissionKind::Write,
        target: DUMMY_PERMISSION_PATH.to_string(),
        body: crate::llm::tools::render_numbered_content(DUMMY_PERMISSION_CONTENT),
        detail: None,
        agent: None,
    };
    let decision = if gate.allows(&request) {
        Some(PermissionDecision::Approve)
    } else {
        let _ = tx.send(StreamEvent::Permission(request.clone()));
        gate.wait(&request.id, &|| cancel.is_cancelled())
    };
    // `Ok(output)` ran; `Err((display, result))` was refused at the prompt —
    // the real backend's two-text shape, so the offline demo exercises Tab's
    // amend feedback all the way into the derived context (docs/permissions.md).
    let resolved = match &decision {
        Some(PermissionDecision::Approve | PermissionDecision::ApproveAlways) => {
            if matches!(decision, Some(PermissionDecision::ApproveAlways)) {
                request.id.clear();
                gate.remember(&request);
            }
            Ok(format!(
                "Created {DUMMY_PERMISSION_PATH} ({} lines)\n{}",
                DUMMY_PERMISSION_CONTENT.lines().count(),
                crate::llm::tools::render_numbered_content(DUMMY_PERMISSION_CONTENT),
            ))
        }
        Some(PermissionDecision::Deny(feedback)) => Err((
            crate::permission::denied_display(&request, feedback.as_deref()),
            crate::permission::denial_result(feedback.as_deref()),
        )),
        Some(PermissionDecision::Explain) => Err((
            crate::permission::explain_display(),
            crate::permission::explain_result(&request),
        )),
        // Cancelled out from under us — the channel is already abandoned.
        None => return,
    };
    let ok = resolved.is_ok();
    let _ = tx.send(StreamEvent::ToolStart {
        name: "Write".to_string(),
        args: DUMMY_PERMISSION_PATH.to_string(),
        detail: None,
    });
    match resolved {
        Ok(output) => {
            let _ = tx.send(StreamEvent::ToolEnd {
                output,
                ok: true,
                truncated: false,
            });
        }
        Err((display, result)) => {
            let _ = tx.send(StreamEvent::ToolRejected { display, result });
        }
    }
    for chunk in chunks(if ok {
        "Done — the file is written."
    } else {
        "Understood, I have left the file alone."
    }) {
        if cancel.is_cancelled() {
            return;
        }
        if tx.send(StreamEvent::Chunk(chunk)).is_err() {
            return;
        }
        nap(CHUNK_DELAY, cancel);
    }
    let _ = tx.send(StreamEvent::StreamDone);
}

/// The scripted **staggered** permission demo's files: two gated `write`s in
/// one batch whose prompts differ wildly in height — the first's body is
/// [`staggered_big_content`] (long enough to cap the prompt on any sane
/// terminal, so it fills the screen), the second's a single line. The offline
/// mirror of the reported gap-under-the-prompt shape: answering the tall
/// prompt commits its cell and opens the short prompt in the same frame gap,
/// shrinking the pinned modal region hard (`docs/permissions.md`, the smoke
/// suite's staggered-prompt phase).
const DUMMY_STAGGERED_BIG_PATH: &str = "big_module.py";
const DUMMY_STAGGERED_TINY_PATH: &str = "tiny_note.py";
const DUMMY_STAGGERED_TINY_CONTENT: &str = "# Reserved for a later chapter of the demo.";

/// The tall `write`'s body — enough source lines that the first prompt's
/// numbered preview caps (and so pads itself to fill the terminal) at any
/// realistic height, maximising the height drop to the tiny second prompt.
fn staggered_big_content() -> String {
    let mut lines = vec![
        "\"\"\"A module long enough to cap the permission prompt's preview.\"\"\"".to_string(),
        String::new(),
    ];
    lines.extend((1..=58).map(|i| format!("VALUE_{i:02} = {i}")));
    lines.join("\n")
}

/// Play the staggered batch approval round trip ([`DUMMY_STAGGERED_BIG_PATH`]
/// / [`DUMMY_STAGGERED_TINY_PATH`]): announce both `Write` calls up front,
/// then — per call, exactly like [`dummy_parallel_permission_turn`] — ask,
/// block on the gate, and resolve the cell before moving on, with **no
/// scripted pause anywhere** so the tall prompt's answer and the tiny
/// prompt's request land in one frame gap.
pub(super) fn dummy_staggered_permission_turn(
    gate: &crate::permission::PermissionGate,
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
) {
    use crate::permission::{PermissionDecision, PermissionKind, PermissionRequest};
    let files = [
        (DUMMY_STAGGERED_BIG_PATH, staggered_big_content()),
        (
            DUMMY_STAGGERED_TINY_PATH,
            DUMMY_STAGGERED_TINY_CONTENT.to_string(),
        ),
    ];
    let _ = tx.send(StreamEvent::ToolBatch(
        files
            .iter()
            .map(|(path, _)| ToolCallSummary {
                name: "Write".to_string(),
                args: (*path).to_string(),
            })
            .collect(),
    ));
    for (path, content) in files {
        let mut request = PermissionRequest {
            id: gate.next_id(),
            kind: PermissionKind::Write,
            target: path.to_string(),
            body: crate::llm::tools::render_numbered_content(&content),
            detail: None,
            agent: None,
        };
        let decision = if gate.allows(&request) {
            Some(PermissionDecision::Approve)
        } else {
            let _ = tx.send(StreamEvent::Permission(request.clone()));
            gate.wait(&request.id, &|| cancel.is_cancelled())
        };
        let resolved = match &decision {
            Some(PermissionDecision::Approve | PermissionDecision::ApproveAlways) => {
                if matches!(decision, Some(PermissionDecision::ApproveAlways)) {
                    request.id.clear();
                    gate.remember(&request);
                }
                Ok(format!(
                    "Created {path} ({} lines)\n{}",
                    content.lines().count(),
                    crate::llm::tools::render_numbered_content(&content),
                ))
            }
            Some(PermissionDecision::Deny(feedback)) => Err((
                crate::permission::denied_display(&request, feedback.as_deref()),
                crate::permission::denial_result(feedback.as_deref()),
            )),
            Some(PermissionDecision::Explain) => Err((
                crate::permission::explain_display(),
                crate::permission::explain_result(&request),
            )),
            // Cancelled out from under us — the channel is already abandoned.
            None => return,
        };
        let _ = tx.send(StreamEvent::ToolStart {
            name: "Write".to_string(),
            args: path.to_string(),
            detail: None,
        });
        match resolved {
            Ok(output) => {
                let _ = tx.send(StreamEvent::ToolEnd {
                    output,
                    ok: true,
                    truncated: false,
                });
            }
            Err((display, result)) => {
                let _ = tx.send(StreamEvent::ToolRejected { display, result });
            }
        }
    }
    for chunk in chunks("Both files are written — the big module and the tiny note.") {
        if cancel.is_cancelled() {
            return;
        }
        if tx.send(StreamEvent::Chunk(chunk)).is_err() {
            return;
        }
        nap(CHUNK_DELAY, cancel);
    }
    let _ = tx.send(StreamEvent::StreamDone);
}

/// The scripted **parallel** permission demo: two gated `bash` commands in one
/// batch — `(command, description, output, ok)` each. The first fails the way
/// `sudo` does in a captured shell, the second succeeds; both ask.
const DUMMY_PARALLEL_COMMANDS: [(&str, &str, &str, bool); 2] = [
    (
        "sudo whoami",
        "Check root/sudo privileges",
        "Exit code: 1\nsudo: a terminal is required to read the password; either use the -S \
         option to read from standard input or configure an askpass helper\nsudo: a password \
         is required",
        false,
    ),
    (
        "ping -c 4 google.com",
        "Ping google.com 4 times to test connectivity",
        "Exit code: 0\nPING google.com (142.250.72.14) 56(84) bytes of data.\n64 bytes from \
         142.250.72.14: icmp_seq=1 ttl=115 time=12.3 ms\n\n--- google.com ping statistics \
         ---\n4 packets transmitted, 4 received, 0% packet loss",
        true,
    ),
];

/// Play the offline **batch** approval round trip ([`DUMMY_PARALLEL_COMMANDS`]):
/// announce both `Bash` calls up front, then — per call, exactly like
/// `llm::agent::run_agent`'s loop — ask, block on the gate, and resolve the
/// cell before moving to the next. Deliberately **no scripted pause
/// anywhere**: on an approval the cell's `ToolStart`/`ToolEnd` and the *next*
/// call's `Permission` land in the same instant, so the loop sees a resolved
/// commit and a reopening modal inside one frame gap — the covering
/// machinery's hardest timing (`docs/permissions.md`, the smoke suite's
/// back-to-back-prompt phase). A rejection resolves that cell red and still
/// asks for the next call, as the real loop does.
pub(super) fn dummy_parallel_permission_turn(
    gate: &crate::permission::PermissionGate,
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
) {
    use crate::permission::{PermissionDecision, PermissionKind, PermissionRequest};
    let _ = tx.send(StreamEvent::ToolBatch(
        DUMMY_PARALLEL_COMMANDS
            .iter()
            .map(|(cmd, ..)| ToolCallSummary {
                name: "Bash".to_string(),
                args: (*cmd).to_string(),
            })
            .collect(),
    ));
    for (cmd, detail, output, ok) in DUMMY_PARALLEL_COMMANDS {
        let mut request = PermissionRequest {
            id: gate.next_id(),
            kind: PermissionKind::Bash,
            target: cmd.to_string(),
            body: String::new(),
            detail: Some(detail.to_string()),
            agent: None,
        };
        let decision = if gate.allows(&request) {
            Some(PermissionDecision::Approve)
        } else {
            let _ = tx.send(StreamEvent::Permission(request.clone()));
            gate.wait(&request.id, &|| cancel.is_cancelled())
        };
        let resolved = match &decision {
            Some(PermissionDecision::Approve | PermissionDecision::ApproveAlways) => {
                if matches!(decision, Some(PermissionDecision::ApproveAlways)) {
                    request.id.clear();
                    gate.remember(&request);
                }
                Ok(output.to_string())
            }
            Some(PermissionDecision::Deny(feedback)) => Err((
                crate::permission::denied_display(&request, feedback.as_deref()),
                crate::permission::denial_result(feedback.as_deref()),
            )),
            Some(PermissionDecision::Explain) => Err((
                crate::permission::explain_display(),
                crate::permission::explain_result(&request),
            )),
            // Cancelled out from under us — the channel is already abandoned.
            None => return,
        };
        let _ = tx.send(StreamEvent::ToolStart {
            name: "Bash".to_string(),
            args: cmd.to_string(),
            detail: None,
        });
        match resolved {
            Ok(output) => {
                let _ = tx.send(StreamEvent::ToolEnd {
                    output,
                    ok,
                    truncated: false,
                });
            }
            Err((display, result)) => {
                let _ = tx.send(StreamEvent::ToolRejected { display, result });
            }
        }
    }
    for chunk in chunks(
        "Both commands are done — sudo whoami failed (no terminal here) and the ping to \
         google.com came back clean.",
    ) {
        if cancel.is_cancelled() {
            return;
        }
        if tx.send(StreamEvent::Chunk(chunk)).is_err() {
            return;
        }
        nap(CHUNK_DELAY, cancel);
    }
    let _ = tx.send(StreamEvent::StreamDone);
}

/// The **auto-mode** demo's two commands: a read-only listing the offline
/// heuristic classifier allows (its output long enough for the collapsed
/// `… +N lines` hint, like the reference transcript) and a deletion it
/// denies. See [`dummy_auto_permission_turn`].
const DUMMY_AUTO_COMMANDS: &[(&str, &str, &str, bool)] = &[
    (
        "ls -la",
        "List the project files",
        "Exit code: 0\ntotal 40\ndrwxr-xr-x 1 user user  256 Aug  4 17:14 .\n\
         drwxr-xr-x 1 user user  126 Aug  1 14:35 ..\n\
         -rw-r--r-- 1 user user 1017 Aug  4 16:59 config.py\n\
         -rw-r--r-- 1 user user  188 Aug  4 16:59 README.md\n\
         -rw-r--r-- 1 user user 2044 Aug  4 17:01 game.py\n\
         -rw-r--r-- 1 user user  310 Aug  4 17:02 utils.py\n\
         -rw-r--r-- 1 user user  452 Aug  4 17:03 test_game.py\n\
         -rw-r--r-- 1 user user   96 Aug  4 17:05 Makefile\n\
         -rw-r--r-- 1 user user  120 Aug  4 17:08 .gitignore\n\
         -rw-r--r-- 1 user user  640 Aug  4 17:10 notes.md\n\
         -rw-r--r-- 1 user user  517 Aug  4 17:12 setup.py\n\
         drwxr-xr-x 1 user user  128 Aug  4 17:13 tests",
        true,
    ),
    (
        "rm -rf /tmp/scratch",
        "Clean up the scratch directory",
        // Only reachable when something other than the classifier cleared it
        // (master mode, an allowlist rule) — the classifier path rejects
        // before any output exists.
        "Exit code: 0",
        true,
    ),
];

/// Play the offline **auto mode** round trip (`docs/permissions.md`): the
/// [`DUMMY_AUTO_COMMANDS`] batch announced up front, then — per call, the
/// real approve seam's disposition — the allowlist first, then in
/// [`PermissionMode::Auto`] the offline heuristic classifier
/// ([`crate::permission::auto_verdict`]) in the user's stead: an allowed
/// command runs with the [`CLASSIFIER_ALLOWED_NOTE`] riding a `ToolNote`, a
/// denied one resolves red via `ToolRejected` with the classifier texts. In
/// any other mode the call asks like the ordinary demos, so the same prompt
/// exercises the Ctrl+A switch (smoke drives it in auto).
///
/// [`PermissionMode::Auto`]: crate::permission::PermissionMode::Auto
/// [`CLASSIFIER_ALLOWED_NOTE`]: crate::permission::CLASSIFIER_ALLOWED_NOTE
pub(super) fn dummy_auto_permission_turn(
    gate: &crate::permission::PermissionGate,
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
) {
    use crate::permission::{
        CLASSIFIER_ALLOWED_NOTE, PermissionDecision, PermissionKind, PermissionMode,
        PermissionRequest, auto_verdict, classifier_denial_result, classifier_denied_display,
    };
    let _ = tx.send(StreamEvent::ToolBatch(
        DUMMY_AUTO_COMMANDS
            .iter()
            .map(|(cmd, ..)| ToolCallSummary {
                name: "Bash".to_string(),
                args: (*cmd).to_string(),
            })
            .collect(),
    ));
    let mut any_rejected = false;
    for (cmd, detail, output, ok) in DUMMY_AUTO_COMMANDS {
        let mut request = PermissionRequest {
            id: gate.next_id(),
            kind: PermissionKind::Bash,
            target: cmd.to_string(),
            body: String::new(),
            detail: Some(detail.to_string()),
            agent: None,
        };
        // The approve seam's disposition, offline: standing approvals first,
        // then auto mode's classifier — here the deterministic heuristic —
        // and the prompt only outside auto mode.
        let mut note = None;
        let resolved = if gate.allows(&request) {
            Ok(output.to_string())
        } else if gate.mode() == PermissionMode::Auto {
            let verdict = auto_verdict(cmd);
            if verdict.allow {
                note = Some(CLASSIFIER_ALLOWED_NOTE.to_string());
                Ok(output.to_string())
            } else {
                let reason = Some(verdict.reason.as_str()).filter(|r| !r.trim().is_empty());
                Err((
                    classifier_denied_display(reason),
                    classifier_denial_result(reason),
                ))
            }
        } else {
            let decision = {
                let _ = tx.send(StreamEvent::Permission(request.clone()));
                gate.wait(&request.id, &|| cancel.is_cancelled())
            };
            match &decision {
                Some(PermissionDecision::Approve | PermissionDecision::ApproveAlways) => {
                    if matches!(decision, Some(PermissionDecision::ApproveAlways)) {
                        request.id.clear();
                        gate.remember(&request);
                    }
                    Ok(output.to_string())
                }
                Some(PermissionDecision::Deny(feedback)) => Err((
                    crate::permission::denied_display(&request, feedback.as_deref()),
                    crate::permission::denial_result(feedback.as_deref()),
                )),
                Some(PermissionDecision::Explain) => Err((
                    crate::permission::explain_display(),
                    crate::permission::explain_result(&request),
                )),
                // Cancelled out from under us — the channel is abandoned.
                None => return,
            }
        };
        let _ = tx.send(StreamEvent::ToolStart {
            name: "Bash".to_string(),
            args: cmd.to_string(),
            detail: None,
        });
        if let Some(note) = note {
            let _ = tx.send(StreamEvent::ToolNote(note));
        }
        match resolved {
            Ok(output) => {
                let _ = tx.send(StreamEvent::ToolEnd {
                    output,
                    ok: *ok,
                    truncated: false,
                });
            }
            Err((display, result)) => {
                any_rejected = true;
                let _ = tx.send(StreamEvent::ToolRejected { display, result });
            }
        }
    }
    // The closing reply reports what actually happened — in auto mode the
    // delete is blocked, while master (or an allowlist) runs both.
    let closing = if any_rejected {
        "Done — the listing ran and the delete was blocked, so nothing was removed."
    } else {
        "Done — both commands ran to completion."
    };
    for chunk in chunks(closing) {
        if cancel.is_cancelled() {
            return;
        }
        if tx.send(StreamEvent::Chunk(chunk)).is_err() {
            return;
        }
        nap(CHUNK_DELAY, cancel);
    }
    let _ = tx.send(StreamEvent::StreamDone);
}
