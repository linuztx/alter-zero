//! The spinner tip's row (`docs/tips.md`): its text and dress, the wrap, and
//! the strip geometry it occupies under the status line.

use super::*;
use crate::tasks::TaskStore;
use crate::tips::{TIP_DELAY, TIPS};
use crate::ui::theme::tool_dim_color;
use crate::ui::wrap::cols;

fn row(buf: &Buffer, y: u16, width: u16) -> String {
    (0..width)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// An app mid-turn whose walk was seeded at `next`, its clock `elapsed` in.
fn turn_at(next: usize, elapsed: Duration) -> App {
    let mut app = App::new();
    app.seed_tips(next);
    app.record_user_message("go");
    app.begin_stream();
    app.set_status_times(elapsed, None);
    app
}

/// The conversation view's live height for `app` at `width`, the way the
/// boundary asks it.
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

#[test]
fn the_tip_hangs_in_the_gutter_behind_its_label() {
    let lines = tip_lines(
        "Press ? on an empty prompt to see the keyboard shortcuts",
        80,
    );
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(
        texts,
        vec!["  ⎿  Tip: Press ? on an empty prompt to see the keyboard shortcuts".to_string()]
    );
}

#[test]
fn the_tip_is_dim_throughout() {
    // Claude Code dims the whole row: it is a hint beside the work, never a
    // line that competes with it.
    for line in tip_lines(TIPS[0], 80) {
        for span in &line.spans {
            assert_eq!(span.style.fg, Some(tool_dim_color()), "{:?}", span.content);
        }
    }
}

#[test]
fn a_tip_too_wide_for_the_terminal_wraps_under_the_gutter() {
    let tip = "Press ctrl+o to see the whole transcript and every tool's output";
    let width = 40;
    let texts: Vec<String> = tip_lines(tip, width).iter().map(plain).collect();
    assert!(texts.len() > 1, "{texts:?}");
    assert!(texts[0].starts_with("  ⎿  Tip: Press"), "{texts:?}");
    for text in &texts[1..] {
        assert!(
            text.starts_with("     "),
            "aligned under the text: {text:?}"
        );
        assert!(!text[5..].starts_with(' '), "no deeper indent: {text:?}");
    }
    for text in &texts {
        assert!(cols(text) <= usize::from(width), "{text:?} overflows");
    }
    let words: Vec<&str> = texts.iter().flat_map(|t| t.split_whitespace()).collect();
    assert_eq!(words.join(" "), format!("⎿ Tip: {tip}"), "nothing lost");
}

#[test]
fn every_tip_fits_one_row_at_eighty_columns() {
    // The catalog's discipline (docs/tips.md): a tip that wraps on the most
    // common terminal width costs the strip a row for nothing.
    for tip in TIPS {
        assert_eq!(tip_lines(tip, 80).len(), 1, "{tip:?} wraps at 80 columns");
    }
}

#[test]
fn render_live_hangs_the_tip_directly_under_the_status() {
    let app = turn_at(0, TIP_DELAY + Duration::from_secs(2));
    let width = 80;
    assert_eq!(tip_rows(&app, width), 1);
    assert_eq!(hang_rows(&app, width), 1);
    let h = height(&app, width);
    let mut buf = buffer(width, h);
    render_live(buf.area, &mut buf, &app);
    assert!(
        row(&buf, 0, width).contains("Working…"),
        "{:?}",
        row(&buf, 0, width)
    );
    assert_eq!(row(&buf, 1, width), format!("  ⎿  Tip: {}", TIPS[0]));
    assert_eq!(row(&buf, 2, width), "", "the status slot's trailing gap");
    assert!(row(&buf, 3, width).starts_with('─'), "then the box's rule");
    // The caret stays on the prompt row, the tip counted above it.
    assert_eq!(cursor_position(buf.area, &app).1, 4);
}

#[test]
fn before_the_delay_the_strip_is_the_status_alone() {
    let app = turn_at(0, Duration::from_secs(1));
    let width = 80;
    assert_eq!(tip_rows(&app, width), 0);
    let h = height(&app, width);
    let mut buf = buffer(width, h);
    render_live(buf.area, &mut buf, &app);
    assert!(row(&buf, 0, width).contains("Working…"));
    assert_eq!(row(&buf, 1, width), "");
    assert!(row(&buf, 2, width).starts_with('─'));
}

#[test]
fn the_strip_counts_the_tip_it_paints() {
    // `strip_content_rows` is the flow check's cheap early-out; it must agree
    // with what `strip_lines` builds, tip included.
    let app = turn_at(0, TIP_DELAY);
    let width = 80;
    let lines = crate::ui::live::strip_lines(&app, width, None, 0);
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(texts.len(), 3, "{texts:?}");
    assert_eq!(texts[1], format!("  ⎿  Tip: {}", TIPS[0]));
    assert_eq!(
        crate::ui::live::strip_content_rows(&app, width, 0),
        u16::try_from(lines.len()).unwrap()
    );
}

#[test]
fn the_checklist_takes_the_slot_from_the_tip() {
    let mut app = turn_at(0, Duration::ZERO);
    let mut store = TaskStore::new();
    store
        .run_create(r#"{"subject": "Write tests", "description": "d"}"#)
        .expect("create succeeds");
    app.record_task_call(
        "TaskCreate",
        "Write tests",
        "{}",
        "Task #1 created successfully: Write tests",
        true,
        store,
    );
    app.set_status_times(TIP_DELAY * 4, None);
    let width = 80;
    assert_eq!(tip_rows(&app, width), 0);
    assert_eq!(hang_rows(&app, width), task_rows(&app, width));
    let h = height(&app, width);
    let mut buf = buffer(width, h);
    render_live(buf.area, &mut buf, &app);
    assert_eq!(row(&buf, 1, width), "  ⎿  ◻ Write tests");
    for y in 0..h {
        assert!(!row(&buf, y, width).contains("Tip:"), "row {y}");
    }
}

#[test]
fn a_picker_opened_mid_turn_keeps_the_tip_above_itself() {
    // The composer-replacing views keep the whole strip above them
    // (`docs/llm.md`) — the tip hangs off the status line there too.
    let mut app = turn_at(0, TIP_DELAY);
    app.open_settings();
    let width = 80;
    let h = crate::ui::settings_height(&app, width, 40).expect("the menu is open");
    let mut buf = buffer(width, h);
    render_live(buf.area, &mut buf, &app);
    assert!(row(&buf, 0, width).contains("Working…"));
    assert_eq!(row(&buf, 1, width), format!("  ⎿  Tip: {}", TIPS[0]));
}
