//! The spinner tip's row (`docs/tips.md`): Claude Code's dim `⎿  Tip: …`,
//! hanging off the status line in the gutter a tool's output hangs from.
//!
//! ```text
//! ⣤⣀⣀⣀⣀⣀⣀⣀ Working… (5s · ↓ 120 tokens · esc to interrupt)
//!   ⎿  Tip: Press ctrl+o to see the whole transcript and every tool's output
//! ```
//!
//! **Live only**: the row is part of the streaming strip and nothing else
//! builds it, so it goes with the turn and never reaches scrollback, the
//! Ctrl+O transcript or the rollout. Which tip shows, and when, is the pure
//! walk on `App` ([`App::tip`]); this module only dresses it.

use super::file_cell::gutter_row_styled;
use super::theme::*;
use super::wrap::cols;
use super::*;

/// How many strip rows the tip takes for `app` at `width` — 0 when no tip
/// shows, else exactly `status_tip_lines`'s count. The tip's half of
/// [`hang_rows`], which threads it through
/// `live_height`/`live_layout`/`cursor_position` beside the checklist.
#[must_use]
pub fn tip_rows(app: &App, width: u16) -> u16 {
    u16::try_from(status_tip_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// What the strip draws under the status line for `app` at `width`: the
/// tip's rows while one shows, else nothing — [`App::tip`] decides, so the
/// rows reserved and the rows painted come from one answer.
pub(super) fn status_tip_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    app.tip().map_or_else(Vec::new, |tip| tip_lines(tip, width))
}

/// `tip` as the rows hanging off the status line: `  ⎿  Tip: {tip}`, dim
/// throughout — Claude Code's dress: a hint beside the work, never a line
/// that competes with it. Word-wrapped under the gutter when the terminal is
/// too narrow for one row (the catalog is held to one row at 80 columns), the
/// continuation rows aligned under the label as a tool's output aligns under
/// its corner.
#[must_use]
pub fn tip_lines(tip: &str, width: u16) -> Vec<Line<'static>> {
    let style = Style::new().fg(tip_color());
    let text_width = usize::from(width)
        .saturating_sub(cols(TOOL_RESULT_PREFIX))
        .max(1);
    wrap_text(
        &format!("{TIP_LABEL}{tip}"),
        u16::try_from(text_width).unwrap_or(u16::MAX),
    )
    .into_iter()
    .enumerate()
    .map(|(i, row)| gutter_row_styled(i, row, style))
    .collect()
}
