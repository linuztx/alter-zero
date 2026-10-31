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

use super::emphasis::emphasized;
use super::file_cell::gutter_row_spans;
use super::inline::wrap_inline;
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

/// `tip` as the rows hanging off the status line: `  ⎿  Tip: {tip}`, dim —
/// Claude Code's dress: a hint beside the work, never a line that competes
/// with it — but for what the tip teaches, the key or command the catalog
/// marks in backticks, which takes the theme's emphasis while the marks
/// themselves never show (`crate::tips::parts`, `docs/emphasis.md`).
/// Word-wrapped under the gutter when the terminal is too narrow for one row
/// (the catalog is held to one row at 80 columns), the continuation rows
/// aligned under the label as a tool's output aligns under its corner.
#[must_use]
pub fn tip_lines(tip: &str, width: u16) -> Vec<Line<'static>> {
    let dim = Style::new().fg(tip_color());
    let text_width = usize::from(width)
        .saturating_sub(cols(TOOL_RESULT_PREFIX))
        .max(1);
    let mut segments = vec![(TIP_LABEL.to_string(), dim)];
    for (text, marked) in crate::tips::parts(tip) {
        let style = if marked { emphasized(dim) } else { dim };
        segments.push((text.to_string(), style));
    }
    wrap_inline(&segments, u16::try_from(text_width).unwrap_or(u16::MAX))
        .into_iter()
        .enumerate()
        .map(|(i, spans)| {
            // The wrapper joins words with unstyled spaces; the row is dim
            // between its words too.
            let spans = spans
                .into_iter()
                .map(|span| {
                    if span.style == Style::default() {
                        span.style(dim)
                    } else {
                        span
                    }
                })
                .collect();
            gutter_row_spans(i, spans)
        })
        .collect()
}
