//! The usage tip under the status line (`docs/tips.md`) — Claude Code's
//! `⎿  Tip: …` row beneath its spinner, on the strip's own gap row:
//!
//! ```text
//! ⣀⣀⣰⣆⣀⣀⣀⣀ Working… (6s · ↑ 1 tokens · esc to interrupt)
//!   ⎿  Tip: Press Ctrl+O to open the full transcript, every tool output included
//! ────────────────────────────────────────────────────────────────────────────
//! ```
//!
//! Which tip, and when, is the pure core's ([`App::tip`] — the walk in
//! `app::tips`); this module only lays the row out: the `⎿` gutter, the dim
//! `Tip: ` label, the sentence clipped to the width, all on **one** row.
//!
//! One row, and the gap's row at that, on purpose: the strip's rows are
//! handed back to scrollback at the turn's end as the committed final line,
//! its spacer, the summary and its spacer — exactly the preview, its gap, the
//! status line and its gap — which is what keeps the box flush at the bottom
//! when the strip collapses (`docs/status-indicator.md` *Strip geometry*,
//! `smoke.sh` Phase 5). A tip that added a row of its own would collapse one
//! row more than the commit refills and leave a blank band under the box
//! after every turn that showed one. So the tip takes the blank gap row's
//! place instead, the strip stays two rows tall with or without it, and the
//! dim row is still what keeps the status line off the box's top rule.

use super::layout::strip_has_status;
use super::theme::*;
use super::wrap::{cols, truncate_cols};
use super::*;

/// The tip row for `app` at `width` — the current tip under the main turn's
/// status line, on the status slot's gap row — or `None`, and the gap stays
/// blank: when there is no status line to hang from (idle, a `!` shell
/// turn), when the task checklist is showing (the plan takes the slot —
/// Claude Code shows its task list *instead of* the tip), inside an agent
/// session view (the tip is the main session's), or when no tip is up yet
/// ([`App::tip`], which also applies the `/settings` **Tips** switch).
pub(super) fn tip_line(app: &App, width: u16) -> Option<Line<'static>> {
    if app.viewed_agent().is_some() || !strip_has_status(app) || !app.tasks().is_empty() {
        return None;
    }
    app.tip().map(|tip| tip_text_line(tip.text, width))
}

/// `text` as the tip row: `  ⎿  Tip: {text}`, the sentence **clipped** to
/// `width` with a closing `…` — one row, never a wrap, since the row is the
/// strip's gap row and a second one would be the blank band under the box
/// this module's header explains. The catalog keeps every tip at seventy
/// columns or fewer, so an 80-column terminal never clips; a narrower one
/// cuts the tail the way a task subject's row does. Dim throughout, the tool
/// gutter's grey: a hint, not an alert.
#[must_use]
pub fn tip_text_line(text: &str, width: u16) -> Line<'static> {
    let lead = format!("{TOOL_RESULT_PREFIX}{TIP_PREFIX}");
    let avail = usize::from(width).saturating_sub(cols(&lead)).max(1);
    let body = if cols(text) > avail {
        format!("{}…", truncate_cols(text, avail.saturating_sub(1)))
    } else {
        text.to_string()
    };
    let dim = Style::new().fg(tool_dim_color());
    Line::from(vec![Span::styled(lead, dim), Span::styled(body, dim)])
}
