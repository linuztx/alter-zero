//! The task tools' live checklist (`docs/task-tools.md`): the `⎿ ◻ subject`
//! rows under the status line, their styling, the cap, and the strip
//! geometry they occupy.

use super::*;
use crate::tasks::TaskStore;
use crate::ui::theme::{TASK_COMPLETED_COLOR, TASK_MAX_ROWS, TOOL_DIM_COLOR};

fn store_of(subjects: &[&str]) -> TaskStore {
    let mut store = TaskStore::new();
    for subject in subjects {
        store
            .run_create(&serde_json::json!({"subject": subject, "description": "d"}).to_string())
            .expect("create succeeds");
    }
    store
}

fn buffer(width: u16, height: u16) -> Buffer {
    Buffer::empty(Rect::new(0, 0, width, height))
}

fn row(buf: &Buffer, y: u16, width: u16) -> String {
    (0..width)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn checklist_rows_wear_the_gutter_and_the_status_glyphs() {
    let mut store = store_of(&["Set up project structure", "Write core logic", "Add tests"]);
    store
        .run_update(r#"{"taskId":"1","status":"in_progress"}"#)
        .unwrap();
    store
        .run_update(r#"{"taskId":"3","status":"completed"}"#)
        .unwrap();
    let lines = checklist_lines(&store, 60);
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(
        texts,
        vec![
            "  ⎿  ◼ Set up project structure".to_string(),
            "     ◻ Write core logic".to_string(),
            "     ✔ Add tests".to_string(),
        ],
        "the reference layout: corner on row one, aligned glyphs below"
    );
    // The in-progress subject is bold; the completed one struck through and
    // dim; the completed glyph green.
    let bold = &lines[0].spans[2];
    assert!(bold.style.add_modifier.contains(Modifier::BOLD));
    let struck = &lines[2].spans[2];
    assert!(struck.style.add_modifier.contains(Modifier::CROSSED_OUT));
    assert_eq!(struck.style.fg, Some(TOOL_DIM_COLOR));
    assert_eq!(lines[2].spans[1].style.fg, Some(TASK_COMPLETED_COLOR));
}

#[test]
fn a_blocked_task_names_its_open_blockers_dim() {
    let mut store = store_of(&["Set up project structure", "Write core logic", "Add tests"]);
    store
        .run_update(r#"{"taskId":"2","addBlockedBy":["1"]}"#)
        .unwrap();
    store
        .run_update(r#"{"taskId":"3","addBlockedBy":["2"]}"#)
        .unwrap();
    let texts: Vec<String> = checklist_lines(&store, 60).iter().map(plain).collect();
    assert_eq!(texts[1], "     ◻ Write core logic › blocked by #1");
    assert_eq!(texts[2], "     ◻ Add tests › blocked by #2");
    // Completing #1 clears #2's suffix on the next render.
    store
        .run_update(r#"{"taskId":"1","status":"completed"}"#)
        .unwrap();
    let texts: Vec<String> = checklist_lines(&store, 60).iter().map(plain).collect();
    assert_eq!(texts[1], "     ◻ Write core logic");
    // The blocked subject and suffix are dim (stuck work reads recessed).
    let lines = checklist_lines(&store, 60);
    let blocked_subject = &lines[2].spans[2];
    assert_eq!(blocked_subject.style.fg, Some(TOOL_DIM_COLOR));
}

#[test]
fn a_long_subject_truncates_to_one_row() {
    let store = store_of(&["A very long subject that cannot possibly fit the width"]);
    let lines = checklist_lines(&store, 30);
    assert_eq!(lines.len(), 1, "one row per task, never a wrap");
    let text = plain(&lines[0]);
    assert!(text.ends_with('…'), "got {text:?}");
    assert!(
        crate::ui::wrap::cols(&text) <= 30,
        "the row fits the width: {text:?}"
    );
}

#[test]
fn past_the_cap_the_list_prioritises_and_folds_the_rest() {
    let mut store = store_of(&[
        "t1", "t2", "t3", "t4", "t5", "t6", "t7", "t8", "t9", "t10", "t11", "t12",
    ]);
    // t1..t4 completed; t5 in progress; t6 blocked by t5; the rest pending.
    for id in 1..=4 {
        store
            .run_update(&format!(r#"{{"taskId":"{id}","status":"completed"}}"#))
            .unwrap();
    }
    store
        .run_update(r#"{"taskId":"5","status":"in_progress"}"#)
        .unwrap();
    store
        .run_update(r#"{"taskId":"6","addBlockedBy":["5"]}"#)
        .unwrap();
    let lines = checklist_lines(&store, 60);
    assert_eq!(
        lines.len(),
        TASK_MAX_ROWS + 1,
        "the cap plus the summary row"
    );
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert!(
        texts[0].contains("◼ t5"),
        "what is happening now leads: {texts:?}"
    );
    let summary = texts.last().unwrap();
    assert!(
        summary.contains("… +") && summary.contains("completed"),
        "the fold names what it hides: {summary:?}"
    );
    // The summary row is dim.
    let last = lines.last().unwrap();
    assert_eq!(last.spans[1].style.fg, Some(TOOL_DIM_COLOR));
}

#[test]
fn task_rows_gates_on_an_active_main_turn() {
    let mut app = App::new();
    // Idle with tasks: nothing (the checklist is a turn-time display).
    app.record_user_message("plan");
    app.begin_stream();
    app.record_task_call(
        "TaskCreate",
        "a",
        "Task #1 created successfully: a",
        true,
        store_of(&["a"]),
    );
    assert_eq!(task_rows(&app, 60), 1, "a live turn shows the list");
    app.finish_stream();
    app.end_turn(1);
    assert_eq!(task_rows(&app, 60), 0, "idle, the strip collapses");
    // A `!` shell turn hides the status line — and the checklist with it.
    app.begin_shell("pwd");
    assert_eq!(task_rows(&app, 60), 0);
}

#[test]
fn render_live_stacks_the_checklist_directly_under_the_status() {
    let mut app = App::new();
    app.record_user_message("plan");
    app.begin_stream();
    let mut store = store_of(&["Set up project structure", "Write core logic"]);
    store
        .run_update(r#"{"taskId":"2","addBlockedBy":["1"]}"#)
        .unwrap();
    store
        .run_update(r#"{"taskId":"1","status":"in_progress"}"#)
        .unwrap();
    app.record_task_call(
        "TaskUpdate",
        "#1 → in_progress",
        "Updated task #1 status",
        true,
        store,
    );
    app.set_status_times(Duration::from_secs(3), None);
    let width = 60;
    let h = live_height(
        &app.input,
        width,
        24,
        true,
        0,
        task_rows(&app, width),
        0,
        0,
        0,
        0,
        0,
    );
    let mut buf = buffer(width, h);
    render_live(buf.area, &mut buf, &app);
    // Row 0: the status line wearing the active task's subject as its verb
    // (no activeForm was set, so the subject stands in).
    let status = row(&buf, 0, width);
    assert!(
        status.contains("Set up project structure…"),
        "the spinner wears the active task: {status:?}"
    );
    // Rows 1-2: the checklist, flush under the status (no gap between).
    assert_eq!(row(&buf, 1, width), "  ⎿  ◼ Set up project structure");
    assert_eq!(
        row(&buf, 2, width),
        "     ◻ Write core logic › blocked by #1"
    );
    // Row 3: the status slot's trailing gap, then the box's top rule.
    assert_eq!(row(&buf, 3, width), "");
    assert!(row(&buf, 4, width).starts_with('─'));
}

#[test]
fn the_status_verb_prefers_the_active_form_and_reverts_when_done() {
    let mut app = App::new();
    app.record_user_message("go");
    app.begin_stream();
    let mut store = TaskStore::new();
    store
        .run_create(r#"{"subject":"Run tests","description":"d","activeForm":"Running tests"}"#)
        .unwrap();
    store
        .run_update(r#"{"taskId":"1","status":"in_progress"}"#)
        .unwrap();
    app.record_task_call(
        "TaskUpdate",
        "#1 → in_progress",
        "Updated task #1 status",
        true,
        store,
    );
    let line = status_line_with_verb(app.status().unwrap(), app.task_verb());
    let text = plain(&line);
    assert!(
        text.contains("Running tests…"),
        "the activeForm is the verb: {text:?}"
    );
    // Completed → the turn's own verb returns.
    let mut done = app.tasks().clone();
    done.run_update(r#"{"taskId":"1","status":"completed"}"#)
        .unwrap();
    app.record_task_call(
        "TaskUpdate",
        "#1 → completed",
        "Updated task #1 status",
        true,
        done,
    );
    let line = status_line_with_verb(app.status().unwrap(), app.task_verb());
    let text = plain(&line);
    assert!(
        text.contains("Working…"),
        "the turn verb stands again: {text:?}"
    );
}
