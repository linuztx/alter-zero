//! The offline permission demos (`docs/permissions.md`).

use std::time::Duration;

use tokio::sync::mpsc::unbounded_channel;

use super::*;

#[test]
fn a_parallel_permission_prompt_asks_for_each_command_in_turn() {
    // The offline mirror of a real batch of gated `bash` calls
    // (docs/permissions.md): both commands are announced up front, each
    // asks before it starts, and the next request follows the previous
    // cell's resolution with **no scripted pause** — the back-to-back
    // prompt timing the covering machinery has to survive.
    let gate = crate::permission::PermissionGate::new();
    let dummy = DummyAi::with_startup_delay(Duration::ZERO).with_permissions(gate.clone());
    let (tx, mut rx) = unbounded_channel();
    let handle = dummy.spawn(
        "parallel permission demo".to_string(),
        vec![],
        vec![],
        tx,
        CancelToken::new(),
    );
    let mut order = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::ToolBatch(items) => order.push(format!("batch:{}", items.len())),
            StreamEvent::Permission(req) => {
                assert_eq!(req.kind, crate::permission::PermissionKind::Bash);
                assert!(
                    req.detail.as_deref().is_some_and(|d| !d.is_empty()),
                    "each command carries its own description"
                );
                order.push(format!("ask:{}", req.target));
                gate.resolve(&req.id, crate::permission::PermissionDecision::Approve);
            }
            StreamEvent::ToolStart { args, .. } => order.push(format!("start:{args}")),
            StreamEvent::ToolEnd { ok, .. } => order.push(format!("end:{ok}")),
            StreamEvent::Chunk(_) => {}
            StreamEvent::StreamDone => {
                order.push("done".to_string());
                break;
            }
            other => panic!("unexpected event in the demo: {other:?}"),
        }
    }
    handle.join().unwrap();
    assert_eq!(
        order,
        vec![
            "batch:2",
            "ask:sudo whoami",
            "start:sudo whoami",
            "end:false",
            "ask:ping -c 4 google.com",
            "start:ping -c 4 google.com",
            "end:true",
            "done",
        ],
        "two gated calls, asked and resolved in order"
    );
}

#[test]
fn the_auto_demo_classifies_instead_of_asking_in_auto_mode() {
    // The offline mirror of auto mode (docs/permissions.md): with the
    // gate in `Auto`, the heuristic classifier stands in for the user —
    // the read-only listing runs with the ToolNote riding it, the delete
    // rejects with the classifier texts, and NO Permission event is ever
    // raised.
    let gate = crate::permission::PermissionGate::new();
    gate.set_mode(crate::permission::PermissionMode::Auto);
    let dummy = DummyAi::with_startup_delay(Duration::ZERO).with_permissions(gate.clone());
    let (tx, mut rx) = unbounded_channel();
    let handle = dummy.spawn(
        "auto permission demo".to_string(),
        vec![],
        vec![],
        tx,
        CancelToken::new(),
    );
    let mut order = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::ToolBatch(items) => order.push(format!("batch:{}", items.len())),
            StreamEvent::ToolStart { args, .. } => order.push(format!("start:{args}")),
            StreamEvent::ToolNote(note) => order.push(format!("note:{note}")),
            StreamEvent::ToolEnd { ok, .. } => order.push(format!("end:{ok}")),
            StreamEvent::ToolRejected { display, result } => {
                assert!(
                    display.starts_with("Denied by auto mode classifier"),
                    "got {display}"
                );
                assert!(
                    result.contains("was not executed"),
                    "the model reads the denial: {result}"
                );
                order.push("rejected".to_string());
            }
            StreamEvent::Permission(req) => {
                panic!("auto mode must never raise a prompt: {req:?}")
            }
            StreamEvent::Chunk(_) => {}
            StreamEvent::StreamDone => {
                order.push("done".to_string());
                break;
            }
            other => panic!("unexpected event in the demo: {other:?}"),
        }
    }
    handle.join().unwrap();
    assert_eq!(
        order,
        vec![
            "batch:2",
            "start:ls -la",
            "note:Allowed by auto mode classifier",
            "end:true",
            "start:rm -rf /tmp/scratch",
            "rejected",
            "done",
        ],
        "the classifier decided both calls without the user"
    );
}

#[test]
fn the_auto_demo_still_asks_outside_auto_mode() {
    // The same prompt under `Manual` goes to the user — so smoke can
    // start the demo, flip Ctrl+A, and watch the difference.
    let gate = crate::permission::PermissionGate::new();
    let dummy = DummyAi::with_startup_delay(Duration::ZERO).with_permissions(gate.clone());
    let (tx, mut rx) = unbounded_channel();
    let handle = dummy.spawn(
        "auto permission demo".to_string(),
        vec![],
        vec![],
        tx,
        CancelToken::new(),
    );
    let mut asked = 0;
    let mut notes = 0;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Permission(req) => {
                asked += 1;
                gate.resolve(&req.id, crate::permission::PermissionDecision::Approve);
            }
            StreamEvent::ToolNote(_) => notes += 1,
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().unwrap();
    assert_eq!(asked, 2, "both commands prompt in manual mode");
    assert_eq!(notes, 0, "no classifier note when the user approved");
}

#[test]
fn the_auto_demo_runs_everything_silently_in_master_mode() {
    let gate = crate::permission::PermissionGate::new();
    gate.set_mode(crate::permission::PermissionMode::Master);
    let dummy = DummyAi::with_startup_delay(Duration::ZERO).with_permissions(gate.clone());
    let (tx, mut rx) = unbounded_channel();
    let handle = dummy.spawn(
        "auto permission demo".to_string(),
        vec![],
        vec![],
        tx,
        CancelToken::new(),
    );
    let mut ends = 0;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Permission(req) => panic!("master mode never asks: {req:?}"),
            StreamEvent::ToolRejected { display, .. } => {
                panic!("master mode never rejects: {display}")
            }
            StreamEvent::ToolNote(note) => panic!("…and never notes: {note}"),
            StreamEvent::ToolEnd { .. } => ends += 1,
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().unwrap();
    assert_eq!(ends, 2, "both calls ran unasked");
}

#[test]
fn a_staggered_permission_demo_asks_a_tall_write_then_a_tiny_one() {
    // The offline mirror of the reported gap-under-the-prompt shape
    // (docs/permissions.md): a batch of two gated `write`s whose prompts
    // differ wildly in height — the first's body is long enough to cap on
    // any sane terminal (the prompt fills it), the second's is a single
    // line — with **no scripted pause** between the first cell's
    // resolution and the second request, so the loop sees the pinned
    // modal region shrink hard inside one frame gap (the smoke suite's
    // staggered-prompt phase drives this end to end).
    let gate = crate::permission::PermissionGate::new();
    let dummy = DummyAi::with_startup_delay(Duration::ZERO).with_permissions(gate.clone());
    let (tx, mut rx) = unbounded_channel();
    let handle = dummy.spawn(
        "staggered permission demo".to_string(),
        vec![],
        vec![],
        tx,
        CancelToken::new(),
    );
    let mut order = Vec::new();
    let mut body_lines = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::ToolBatch(items) => order.push(format!("batch:{}", items.len())),
            StreamEvent::Permission(req) => {
                assert_eq!(req.kind, crate::permission::PermissionKind::Write);
                body_lines.push(req.body.lines().count());
                order.push(format!("ask:{}", req.target));
                gate.resolve(&req.id, crate::permission::PermissionDecision::Approve);
            }
            StreamEvent::ToolStart { args, .. } => order.push(format!("start:{args}")),
            StreamEvent::ToolEnd { ok, .. } => order.push(format!("end:{ok}")),
            StreamEvent::Chunk(_) => {}
            StreamEvent::StreamDone => {
                order.push("done".to_string());
                break;
            }
            other => panic!("unexpected event in the demo: {other:?}"),
        }
    }
    handle.join().unwrap();
    assert_eq!(
        order,
        vec![
            "batch:2",
            "ask:big_module.py",
            "start:big_module.py",
            "end:true",
            "ask:tiny_note.py",
            "start:tiny_note.py",
            "end:true",
            "done",
        ],
        "two gated writes, asked and resolved in order"
    );
    assert!(
        body_lines[0] >= 50,
        "the first prompt's body caps any sane terminal, got {} lines",
        body_lines[0]
    );
    assert_eq!(body_lines[1], 1, "the second prompt is a single line");
}
