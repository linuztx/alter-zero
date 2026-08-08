//! The task tools' app-side bookkeeping (`docs/task-tools.md`): the hidden
//! [`HistoryItem::TaskCall`] record, the live checklist snapshot, and the
//! three history rewinds that restore it.

use super::*;
use crate::tasks::TaskStore;

/// A store with `subjects` created in order (ids 1..), via the real ops so
/// the snapshots are genuine.
fn store_of(subjects: &[&str]) -> TaskStore {
    let mut store = TaskStore::new();
    for subject in subjects {
        store
            .run_create(&serde_json::json!({"subject": subject, "description": "d"}).to_string())
            .expect("create succeeds");
    }
    store
}

/// Record one task call on `app` whose snapshot is `store` (the loop-arm
/// shape: display name, args summary, result text, outcome, snapshot).
fn record(app: &mut App, ok: bool, store: &TaskStore) {
    app.record_task_call(
        "TaskCreate",
        "a subject",
        "{}",
        "Task #1 created successfully: a",
        ok,
        store.clone(),
    );
}

#[test]
fn record_task_call_appends_the_record_and_installs_the_snapshot() {
    let mut app = App::new();
    app.record_user_message("plan it");
    app.begin_stream();
    let store = store_of(&["Set up project structure"]);
    record(&mut app, true, &store);
    let Some(HistoryItem::TaskCall(rec)) = app.history.last() else {
        panic!(
            "a TaskCall record is appended, got {:?}",
            app.history.last()
        );
    };
    assert_eq!(rec.name, "TaskCreate");
    assert_eq!(rec.args, "a subject");
    assert!(rec.ok);
    assert_eq!(rec.tasks.tasks().len(), 1);
    assert_eq!(
        app.tasks().tasks().len(),
        1,
        "the checklist snapshot updated"
    );
}

#[test]
fn record_task_call_charges_the_result_to_the_tally_like_a_tool() {
    let mut app = App::new();
    app.record_user_message("plan");
    app.begin_stream();
    let before = app.status().unwrap().tokens;
    record(&mut app, true, &store_of(&["a"]));
    let status = app.status().unwrap();
    assert!(status.tokens > before, "the result text is counted");
    assert_eq!(
        status.arrow,
        TokenArrow::Up,
        "a tool result uploads back (↑)"
    );
}

#[test]
fn a_turn_with_a_task_call_interrupts_kept_not_undone() {
    // The undo branch requires the just-submitted user message to still be
    // the history tail; a task record after it means the turn produced
    // output, so Esc keeps it (docs/interrupt.md).
    let mut app = App::new();
    app.record_user_message("plan it");
    app.begin_stream();
    record(&mut app, true, &store_of(&["a"]));
    let interrupted = app.interrupt_turn().expect("a live turn was interrupted");
    assert!(
        matches!(interrupted, InterruptedTurn::Kept { .. }),
        "a task record is output — never the undo: {interrupted:?}"
    );
    assert_eq!(app.tasks().tasks().len(), 1, "the checklist survives");
}

#[test]
fn a_resumed_or_rewound_finished_plan_stays_retired() {
    // A rewind lands between turns, so a snapshot that was already finished
    // there is finished now — restoring its ticks would put back exactly
    // what the turn-boundary retirement removed (docs/task-tools.md).
    let mut app = App::new();
    let mut done = store_of(&["a", "b"]);
    for id in 1..=2 {
        done.run_update(&format!(r#"{{"taskId":"{id}","status":"completed"}}"#))
            .unwrap();
    }
    app.load_session(vec![HistoryItem::TaskCall(TaskCallRecord {
        name: "TaskUpdate".into(),
        arguments: "{}".to_string(),
        args: "#2 → completed".into(),
        output: "Updated task #2 status".into(),
        ok: true,
        timestamp: String::new(),
        tasks: done,
    })]);
    assert!(app.tasks().is_empty(), "a finished plan does not come back");
    assert_eq!(
        app.tasks().high_water(),
        2,
        "…but its ids are still spent, so the next plan continues at #3"
    );
    // An OPEN plan resumes whole.
    app.load_session(vec![HistoryItem::TaskCall(TaskCallRecord {
        name: "TaskCreate".into(),
        arguments: "{}".to_string(),
        args: "b".into(),
        output: "Task #2 created successfully: b".into(),
        ok: true,
        timestamp: String::new(),
        tasks: store_of(&["a", "b"]),
    })]);
    assert_eq!(app.tasks().tasks().len(), 2);
}

#[test]
fn task_verb_is_the_running_forms_label_while_a_task_is_in_progress() {
    let mut app = App::new();
    let mut store = store_of(&["Write tests"]);
    store
        .run_update(r#"{"taskId":"1","status":"in_progress"}"#)
        .unwrap();
    app.record_user_message("go");
    app.begin_stream();
    app.record_task_call(
        "TaskUpdate",
        "#1 → in_progress",
        "{}",
        "Updated task #1 status",
        true,
        store,
    );
    assert_eq!(app.task_verb(), Some("Write tests"));
    // Completing it drops the override — the turn's own verb stands again.
    let mut done = app.tasks().clone();
    done.run_update(r#"{"taskId":"1","status":"completed"}"#)
        .unwrap();
    app.record_task_call(
        "TaskUpdate",
        "#1 → completed",
        "{}",
        "Updated task #1 status",
        true,
        done,
    );
    assert_eq!(app.task_verb(), None);
}

#[test]
fn clear_conversation_wipes_the_task_list() {
    let mut app = App::new();
    app.record_user_message("plan");
    app.begin_stream();
    record(&mut app, true, &store_of(&["a"]));
    app.clear_conversation();
    assert!(app.tasks().is_empty(), "a cleared slate holds no tasks");
}

#[test]
fn load_session_restores_the_last_records_snapshot() {
    let mut app = App::new();
    let items = vec![
        HistoryItem::TaskCall(TaskCallRecord {
            name: "TaskCreate".into(),
            arguments: "{}".to_string(),
            args: "a".into(),
            output: "Task #1 created successfully: a".into(),
            ok: true,
            timestamp: String::new(),
            tasks: store_of(&["a"]),
        }),
        HistoryItem::TaskCall(TaskCallRecord {
            name: "TaskCreate".into(),
            arguments: "{}".to_string(),
            args: "b".into(),
            output: "Task #2 created successfully: b".into(),
            ok: true,
            timestamp: String::new(),
            tasks: store_of(&["a", "b"]),
        }),
    ];
    app.load_session(items);
    assert_eq!(
        app.tasks().tasks().len(),
        2,
        "the LAST record's snapshot is the resumed state"
    );
    assert_eq!(
        app.tasks().high_water(),
        2,
        "ids keep counting after a resume"
    );
    // A history without task records loads an empty list.
    app.load_session(vec![HistoryItem::Message(Message {
        role: Role::User,
        text: "hi".into(),
        timestamp: String::new(),
        images: Vec::new(),
    })]);
    assert!(app.tasks().is_empty());
}

#[test]
fn a_backtrack_rewind_resets_the_list_to_the_snapshot_at_the_cut() {
    let mut app = App::new();
    // Turn 1: a user message, then one task created.
    app.record_user_message("first");
    app.begin_stream();
    record(&mut app, true, &store_of(&["a"]));
    app.finish_stream();
    app.end_turn(1);
    // Turn 2: another user message, then a second task.
    app.record_user_message("second");
    app.begin_stream();
    app.record_task_call(
        "TaskCreate",
        "b",
        "{}",
        "Task #2 created successfully: b",
        true,
        store_of(&["a", "b"]),
    );
    app.finish_stream();
    app.end_turn(1);
    assert_eq!(app.tasks().tasks().len(), 2);
    // Esc-Esc back to the SECOND user message: everything after it drops,
    // and the list resets to the snapshot before the cut — one task.
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc));
    let action = app.on_key(key(KeyCode::Enter));
    assert!(matches!(action, Action::ConfirmBacktrack), "got {action:?}");
    assert_eq!(
        app.tasks().tasks().len(),
        1,
        "the checklist rewound with the conversation"
    );
    // Rewinding past every task record empties the list. (The confirm
    // recalled the rewound message into the composer; Esc over a typed
    // draft is a no-op, so drop it first.)
    let _ = app.input.take();
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc));
    let action = app.on_key(key(KeyCode::Enter));
    assert!(matches!(action, Action::ConfirmBacktrack), "got {action:?}");
    assert!(app.tasks().is_empty());
}
