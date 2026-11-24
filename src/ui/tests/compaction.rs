//! The background compaction's row under the status line (`docs/compact.md`
//! *What the strip shows*): what it says in each state, its clock, and the
//! strip reserving exactly the rows it paints.

use super::*;
use crate::stream::StreamEvent;
use crate::ui::theme::{
    COMPACT_READY_ROW, COMPACT_REQUESTED_ROW, COMPACT_WAITING_ROW, COMPACTING_ROW,
    TOOL_RESULT_PREFIX, status_offline_color, tool_dim_color,
};

/// A turn under way on a 50k window, five seconds in.
fn working_turn() -> App {
    let mut app = App::new();
    app.record_user_message("go");
    app.set_context_window(Some(50_000));
    app.begin_stream();
    app.set_status_times(Duration::from_secs(5), None);
    app
}

/// The conversation view's live height for `app` at `width`.
fn height(app: &App, width: u16) -> u16 {
    live_height(
        &app.input,
        width,
        24,
        strip_has_status(app),
        0,
        hang_rows(app, width),
        0,
        0,
        0,
        0,
        0,
    )
}

/// The row's text for `app`, one row at 120 columns.
fn row_text(app: &App) -> Vec<String> {
    compacting_lines(app, 120).iter().map(plain).collect()
}

#[test]
fn nothing_hangs_while_no_compaction_runs() {
    let app = working_turn();
    assert_eq!(compacting_rows(&app, 80), 0);
}

#[test]
fn a_requested_compaction_says_it_starts_at_the_next_step() {
    let mut app = working_turn();
    app.request_compaction();
    assert_eq!(
        row_text(&app),
        vec![format!("{TOOL_RESULT_PREFIX}{COMPACT_REQUESTED_ROW}")]
    );
}

#[test]
fn a_running_compaction_says_so_with_its_own_clock() {
    let mut app = working_turn();
    app.begin_background_compaction(7);
    app.set_compaction_elapsed(Some(Duration::from_secs(12)));
    assert_eq!(
        row_text(&app),
        vec![format!("{TOOL_RESULT_PREFIX}{COMPACTING_ROW} (12s)")]
    );
    let lines = compacting_lines(&app, 120);
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|span| span.style.fg == Some(tool_dim_color())),
        "dim throughout: a fact beside the work, not an alert"
    );
}

#[test]
fn an_agent_waiting_at_the_hard_limit_is_said_in_amber() {
    let mut app = working_turn();
    app.begin_background_compaction(7);
    app.set_compaction_waiting(7);
    app.set_compaction_elapsed(Some(Duration::from_secs(65)));
    assert_eq!(
        row_text(&app),
        vec![format!("{TOOL_RESULT_PREFIX}{COMPACT_WAITING_ROW} (1m 5s)")]
    );
    let lines = compacting_lines(&app, 120);
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|span| span.style.fg == Some(status_offline_color())),
        "the one pause it costs is lit: {:?}",
        lines[0]
    );
}

#[test]
fn a_finished_summary_waits_for_the_agents_next_step() {
    let mut app = working_turn();
    app.begin_background_compaction(7);
    app.on_summary_event(&StreamEvent::Chunk("gist".to_string()));
    app.on_summary_event(&StreamEvent::StreamDone);
    assert_eq!(
        row_text(&app),
        vec![format!("{TOOL_RESULT_PREFIX}{COMPACT_READY_ROW}")]
    );
}

#[test]
fn the_row_wraps_under_its_gutter_and_never_past_the_width() {
    let mut app = working_turn();
    app.begin_background_compaction(7);
    app.on_summary_event(&StreamEvent::StreamDone);
    let lines = compacting_lines(&app, 30);
    assert!(lines.len() > 1);
    for line in &lines {
        assert!(
            crate::ui::wrap::cols(&plain(line)) <= 30,
            "{:?}",
            plain(line)
        );
    }
}

#[test]
fn the_strip_reserves_the_row_and_paints_it_under_the_status_line() {
    let mut app = working_turn();
    let idle = height(&app, 80);
    app.begin_background_compaction(7);
    app.set_compaction_elapsed(Some(Duration::from_secs(3)));
    assert_eq!(compacting_rows(&app, 80), 1);
    assert_eq!(hang_rows(&app, 80), 1, "the strip's one hanging row");
    let h = height(&app, 80);
    assert_eq!(h, idle + 1, "the region grows by the row it paints");
    let mut buf = buffer(80, h);
    render_live(buf.area, &mut buf, &app);
    let rows: Vec<String> = (0..h).map(|y| row(&buf, y, 80)).collect();
    assert!(
        rows[1].contains(COMPACTING_ROW),
        "the row hangs right under the status line: {rows:?}"
    );
    assert!(app.tip().is_none(), "the tip gives way to it");
}

#[test]
fn no_row_without_a_status_line_to_hang_from() {
    // The turn ended with a summary still being written: that compaction is
    // the session's own turn now, its status line `Compacting…` itself.
    let mut app = working_turn();
    app.begin_background_compaction(7);
    app.finish_stream();
    let _ = app.take_turn_summary(1);
    assert!(!app.turn_active());
    assert_eq!(compacting_rows(&app, 80), 0);
}
