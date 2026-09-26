//! The usage tip under the status line (`docs/tips.md`): the `⎿  Tip: …`
//! row on the strip's gap row, its styling, its clip, and the strip
//! geometry it leaves exactly as it was.

use super::*;
use crate::app::{TIP_DELAY, TIP_ROTATION, TIPS};
use crate::ui::live::{strip_flow_key, strip_lines};
use crate::ui::theme::{INPUT_CHROME_ROWS, STATUS_GAP_ROWS, STATUS_ROWS, tool_dim_color};
use crate::ui::tips::{tip_line, tip_text_line};
use crate::ui::wrap::cols;

/// A turn `elapsed` in, the boundary's per-frame injection done.
fn turn_at(elapsed: Duration) -> App {
    let mut app = App::new();
    app.begin_stream();
    app.set_status_times(elapsed, None);
    app
}

/// The strip's rows as plain text.
fn strip_text(app: &App, width: u16) -> Vec<String> {
    strip_lines(app, width, None, 0).iter().map(plain).collect()
}

#[test]
fn the_tip_row_takes_the_gap_row_under_the_status_line() {
    // Claude Code's `⎿  Tip: …` under its spinner — here on the status
    // slot's own gap row, so the strip is status + tip, two rows, exactly
    // the status + gap it is without a tip.
    let app = turn_at(TIP_DELAY);
    let rows = strip_text(&app, 100);
    assert!(
        rows[0].contains("esc to interrupt"),
        "the status line leads: {rows:?}"
    );
    assert_eq!(rows[1], format!("  ⎿  Tip: {}", TIPS[0].text));
    assert_eq!(
        rows.len(),
        usize::from(STATUS_ROWS + STATUS_GAP_ROWS),
        "the tip is the gap row, not one more: {rows:?}"
    );
    // Without a tip the same slot ends on the blank gap.
    let bare = strip_text(&turn_at(Duration::from_secs(1)), 100);
    assert_eq!(bare.len(), rows.len(), "the same height either way");
    assert_eq!(bare[1], "", "the gap row is blank until a tip is up");
}

#[test]
fn the_tip_row_is_dim_throughout() {
    // A hint, not an alert: the gutter, the `Tip:` and the sentence all
    // wear the tool gutter's dim (Claude Code renders the whole row
    // `dimColor`).
    let app = turn_at(TIP_DELAY);
    let tip = tip_line(&app, 100).expect("a tip is up");
    assert!(!tip.spans.is_empty());
    for span in &tip.spans {
        assert_eq!(
            span.style.fg,
            Some(tool_dim_color()),
            "span {:?} is not dim",
            span.content
        );
        assert!(
            !span.style.add_modifier.contains(Modifier::BOLD),
            "nothing on the row is bold: {:?}",
            span.content
        );
    }
}

#[test]
fn no_tip_row_shows_before_the_delay() {
    let app = turn_at(Duration::from_secs(1));
    assert!(tip_line(&app, 100).is_none());
    assert!(
        !strip_text(&app, 100).iter().any(|r| r.contains("Tip:")),
        "nothing under the delay"
    );
}

#[test]
fn the_live_region_is_no_taller_with_a_tip_and_the_box_sits_where_it_did() {
    // The whole point of the gap-row placement: the geometry the box and the
    // cursor are seated by does not change when a tip comes up, so the
    // turn-end collapse refills exactly what it vacates (`smoke.sh` Phase 5).
    let app = turn_at(TIP_DELAY);
    assert_eq!(
        task_rows(&app, 100),
        0,
        "no checklist rows: the tip is not one"
    );
    let h = live_height(
        &app.input,
        100,
        24,
        true,
        0,
        task_rows(&app, 100),
        0,
        0,
        0,
        0,
        0,
    );
    assert_eq!(h, STATUS_ROWS + STATUS_GAP_ROWS + INPUT_CHROME_ROWS + 1);
    let mut buf = buffer(100, h);
    render_live(buf.area, &mut buf, &app);
    assert!(
        row(&buf, 0, 100).contains("esc to interrupt"),
        "{:?}",
        row(&buf, 0, 100)
    );
    assert_eq!(
        row(&buf, 1, 100).trim_end(),
        format!("  ⎿  Tip: {}", TIPS[0].text)
    );
    // The box's top rule sits right under the tip row — the tip is the row
    // that keeps the status line off it.
    assert!(
        row(&buf, 2, 100).starts_with('─'),
        "the box's top rule follows the tip row: {:?}",
        row(&buf, 2, 100)
    );
}

#[test]
fn a_long_tip_clips_to_one_row_with_an_ellipsis() {
    // One row, never a wrap: a second row would be the blank band under the
    // box that the gap-row placement exists to avoid. A narrow terminal cuts
    // the tail the way a task subject's row does.
    let text = "Press Ctrl+R to search everything you have typed, across sessions";
    let line = plain(&tip_text_line(text, 40));
    assert!(line.starts_with("  ⎿  Tip: Press Ctrl+R"), "{line:?}");
    assert!(line.ends_with('…'), "{line:?}");
    assert!(cols(&line) <= 40, "the row fits the width: {line:?}");
    // Wide enough, the sentence is whole — the catalog keeps every tip
    // inside an 80-column row.
    let whole = plain(&tip_text_line(text, 80));
    assert_eq!(whole, format!("  ⎿  Tip: {text}"));
    let rows = strip_text(&turn_at(TIP_DELAY), 40);
    assert_eq!(rows.len(), 2, "still two rows at a narrow width: {rows:?}");
}

#[test]
fn the_checklist_displaces_the_tip_and_keeps_its_gap() {
    // Claude Code's spinner shows the task list *instead of* the tip: the
    // plan is what the user is watching, and two things hanging off one
    // spinner is a pile. The list keeps the blank gap under its rows.
    let mut app = turn_at(TIP_DELAY);
    assert!(tip_line(&app, 100).is_some(), "a tip is up before the plan");
    let mut store = crate::tasks::TaskStore::new();
    store
        .run_create(r#"{"subject":"Write core logic","description":"d"}"#)
        .unwrap();
    app.record_task_call(
        "TaskCreate",
        "Write core logic",
        "{}",
        "Task #1 created successfully: Write core logic",
        true,
        store,
    );
    assert!(
        tip_line(&app, 100).is_none(),
        "the checklist takes the slot"
    );
    let rows = strip_text(&app, 100);
    assert!(rows[1].contains("◻ Write core logic"), "{rows:?}");
    assert_eq!(rows[2], "", "the list keeps its gap: {rows:?}");
    assert!(!rows.iter().any(|r| r.contains("Tip:")), "{rows:?}");
}

#[test]
fn tips_off_shows_no_row() {
    let mut app = turn_at(TIP_DELAY);
    app.settings_mut().tips = false;
    assert!(tip_line(&app, 100).is_none());
    let rows = strip_text(&app, 100);
    assert_eq!(rows[1], "", "the switch puts the blank gap back at once");
}

#[test]
fn a_shell_turn_and_an_idle_screen_show_no_tip_row() {
    let mut app = App::new();
    assert!(
        tip_line(&app, 100).is_none(),
        "idle: no status line to hang from"
    );
    app.begin_shell("sleep 9");
    app.set_status_times(TIP_DELAY + TIP_ROTATION, None);
    assert!(
        tip_line(&app, 100).is_none(),
        "a `!` turn hides the status line"
    );
}

#[test]
fn the_strip_flow_signs_on_the_tip_it_shows() {
    // A tip coming up or moving on is a structural change to the strip
    // (`docs/strip-flow.md`): a flowed strip must re-sign so scrollback
    // never freezes a stale tip row.
    let before = strip_flow_key(&turn_at(Duration::from_secs(1)));
    let first = strip_flow_key(&turn_at(TIP_DELAY));
    let second = strip_flow_key(&turn_at(TIP_DELAY + TIP_ROTATION));
    assert_ne!(before, first);
    assert_ne!(first, second);
    assert_eq!(
        strip_flow_key(&turn_at(TIP_DELAY + Duration::from_secs(1))),
        first,
        "the same tip a second later signs the same"
    );
}
