//! Background compaction's row (`docs/compact.md` *What the strip shows*):
//! while a summary is written beside the agent's work, a dim row hangs off
//! the status line in the gutter a tool's output hangs from — and says so in
//! the warning amber when the agent waits for it at the hard limit.
//!
//! ```text
//! ⣤⣀⣀⣀⣀⣀⣀⣀ Hatching… (2m 3s · ↓ 18k tokens · esc to interrupt)
//!   ⎿  Compacting context in the background… (12s)
//! ```
//!
//! **Live only**, like the tip it displaces: the row is the strip's and
//! nothing else builds it — the `● Context compacted` cell is the record,
//! committed where the summary lands. Which state shows is the pure state on
//! `App` ([`App::background_compaction`]); this module only dresses it.

use super::file_cell::gutter_row_styled;
use super::layout::strip_has_status;
use super::theme::*;
use super::wrap::cols;
use super::*;
use crate::app::CompactionPhase;

/// How many strip rows the row takes for `app` at `width` — 0 while no
/// compaction runs beside the turn, else exactly `compacting_lines`' count.
/// Its share of [`hang_rows`](super::layout::hang_rows).
#[must_use]
pub fn compacting_rows(app: &App, width: u16) -> u16 {
    u16::try_from(compacting_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// The row for `app` at `width`: what the compaction beside the turn is
/// doing, word-wrapped under the gutter — or nothing, with no compaction,
/// no status line of the main session's own to hang from, or the main
/// session not on screen.
#[must_use]
pub fn compacting_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    if !strip_has_status(app) || app.viewed_agent().is_some() {
        return Vec::new();
    }
    let elapsed = |row: &str| {
        let secs = app.compaction_elapsed().unwrap_or_default().as_secs();
        format!("{row} ({})", crate::app::format_elapsed(secs))
    };
    let dim = Style::new().fg(tool_dim_color());
    let (text, style) = match app.background_compaction() {
        Some(running) if running.waiting && running.phase == CompactionPhase::Summarizing => (
            elapsed(COMPACT_WAITING_ROW),
            Style::new().fg(status_offline_color()),
        ),
        Some(running) if running.phase == CompactionPhase::Summarizing => {
            (elapsed(COMPACTING_ROW), dim)
        }
        Some(_) => (COMPACT_READY_ROW.to_string(), dim),
        None if app.compaction_requested() => (COMPACT_REQUESTED_ROW.to_string(), dim),
        None => return Vec::new(),
    };
    let text_width = usize::from(width)
        .saturating_sub(cols(TOOL_RESULT_PREFIX))
        .max(1);
    wrap_text(&text, u16::try_from(text_width).unwrap_or(u16::MAX))
        .into_iter()
        .enumerate()
        .map(|(i, row)| gutter_row_styled(i, row, style))
        .collect()
}
