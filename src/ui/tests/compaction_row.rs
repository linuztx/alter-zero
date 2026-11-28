//! The background compaction row under the status line
//! (`docs/background-compaction.md` *What the user sees*): what it says while
//! a summary is written and once it is held, and the strip reserving exactly
//! the rows it paints.

use super::*;
use crate::app::CompactionRow;
use crate::ui::compaction_row::{compaction_row_lines, compaction_rows};
use crate::ui::theme::{TOOL_RESULT_PREFIX, tool_dim_color};

#[test]
fn a_summary_being_written_says_so_with_its_time() {
    let lines = compaction_row_lines(
        CompactionRow::Writing {
            elapsed: Duration::from_secs(12),
        },
        120,
    );
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(
        texts,
        vec![format!(
            "{TOOL_RESULT_PREFIX}Compacting in the background · 12s"
        )]
    );
}

#[test]
fn a_held_summary_says_where_it_swaps_in() {
    let lines = compaction_row_lines(CompactionRow::Held { percent: 85 }, 120);
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(
        texts,
        vec![format!(
            "{TOOL_RESULT_PREFIX}Summary ready · swaps in at 85%"
        )]
    );
}

#[test]
fn the_row_is_dim_throughout() {
    // A fact about the turn, not an alert: the tool gutter's dim.
    let lines = compaction_row_lines(
        CompactionRow::Writing {
            elapsed: Duration::from_secs(3),
        },
        120,
    );
    for span in lines.iter().flat_map(|line| line.spans.iter()) {
        assert_eq!(span.style.fg, Some(tool_dim_color()), "{span:?}");
    }
}

#[test]
fn the_strip_reserves_the_row_only_while_a_turn_runs_with_a_summary_in_hand() {
    let mut app = App::new();
    app.set_context_window(Some(200_000));
    app.record_user_message("port the payments module");
    app.begin_stream();
    assert_eq!(compaction_rows(&app, 80), 0, "no summary started");
    let without = hang_rows(&app, 80);
    app.begin_background_compaction(1);
    assert_eq!(compaction_rows(&app, 80), 1);
    assert_eq!(hang_rows(&app, 80), without + 1, "one more row hangs");
    app.finish_stream();
    app.end_turn(1);
    assert_eq!(
        compaction_rows(&app, 80),
        0,
        "no status line to hang from between turns"
    );
}
