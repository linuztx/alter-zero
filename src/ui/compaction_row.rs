//! The background compaction row (`docs/background-compaction.md` *What the
//! user sees*): while a summary of the conversation is written in the
//! background, a dim row hangs off the status line in the gutter a tool's
//! output hangs from — how long it has been writing, then, once written and
//! held, the share of the window it swaps in at.
//!
//! ```text
//! ⣤⣀⣀⣀⣀⣀⣀⣀ Hatching… (1m 50s · ↓ 58.1k tokens · esc to interrupt)
//!   ⎿  Compacting in the background · 40s
//! ```
//!
//! The spinner keeps its verb: the agent is still working. **Live only**, like
//! the lost-connection row beside it: it goes with the job, and the record is
//! the `● Context compacted · … · background` cell the swap commits. What the
//! row says is the pure state on `App` ([`App::compaction_row`]); this module
//! only dresses it.

use super::layout::strip_has_status;
use super::theme::*;
use super::wrap::cols;
use super::*;
use crate::app::CompactionRow;

/// How many strip rows the background compaction row takes for `app` at
/// `width` — 0 with no summary in hand or no status line to hang from, else
/// exactly `status_compaction_lines`'s count. Its share of
/// [`hang_rows`](super::layout::hang_rows).
#[must_use]
pub fn compaction_rows(app: &App, width: u16) -> u16 {
    u16::try_from(status_compaction_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// What the strip draws under the status line for `app` at `width`: the row
/// while the main session has a background summary in hand and a turn's
/// status line shows — never in an agent session view, whose status line is
/// another conversation's.
pub(super) fn status_compaction_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    if !strip_has_status(app) || app.viewed_agent().is_some() {
        return Vec::new();
    }
    app.compaction_row()
        .map(|row| compaction_row_lines(row, width))
        .unwrap_or_default()
}

/// The row for `row` at `width`: `⎿  Compacting in the background · 12s`, or
/// `⎿  Summary ready · swaps in at 85%`, dim, wrapped under its gutter.
#[must_use]
pub fn compaction_row_lines(row: CompactionRow, width: u16) -> Vec<Line<'static>> {
    let dim = Style::new().fg(tool_dim_color());
    let text = match row {
        CompactionRow::Writing { elapsed } => format!(
            "{COMPACTION_WRITING_ROW} · {}",
            format_elapsed(elapsed.as_secs())
        ),
        CompactionRow::Held { percent } => format!("{COMPACTION_HELD_ROW}{percent}%"),
    };
    let lead = cols(TOOL_RESULT_PREFIX);
    let room = u16::try_from(usize::from(width).saturating_sub(lead).max(1)).unwrap_or(u16::MAX);
    wrap_text(&text, room)
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let gutter = if i == 0 {
                TOOL_RESULT_PREFIX.to_string()
            } else {
                " ".repeat(lead)
            };
            Line::from(vec![Span::styled(gutter, dim), Span::styled(row, dim)])
        })
        .collect()
}
