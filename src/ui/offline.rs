//! The lost-connection row (`docs/offline.md`): while the backend waits for a
//! connection it cannot make, a row hangs off the status line in the gutter a
//! tool's output hangs from — an animated ripple, then which host is gone,
//! that the request will be re-sent on its own, and how many attempts have
//! failed so far.
//!
//! ```text
//! ⣤⣀⣀⣀⣀⣀⣀⣀ Waiting for internet… (2m 3s · ↑ 42 tokens · offline for 1m 10s · esc to interrupt)
//!   ⎿  ((·))  No connection to api.venice.ai — trying again · 14 attempts
//! ```
//!
//! **Live only**, like the tip it displaces: the row is the streaming strip's
//! and nothing else builds it, so it goes with the wait and never reaches
//! scrollback, the Ctrl+O transcript or the rollout. Which wait shows is the
//! pure state on `App` ([`App::offline`] — the viewed agent's inside its
//! session view, else the main turn's); this module only dresses it.

use super::layout::strip_has_status;
use super::theme::*;
use super::wrap::{blend_color, cols};
use super::*;
use crate::app::OfflineInfo;

/// How many strip rows the lost-connection row takes for `app` at `width` —
/// 0 while the connection is fine, else exactly `status_offline_lines`'s
/// count. The row's share of [`hang_rows`](super::layout::hang_rows), which
/// threads it through `live_height`/`live_layout`/`cursor_position` beside
/// the checklist and the tip.
#[must_use]
pub fn offline_rows(app: &App, width: u16) -> u16 {
    u16::try_from(status_offline_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// What the strip draws under the status line for `app` at `width`: the row
/// while a wait is on, else nothing — [`App::offline`] decides, so the rows
/// reserved and the rows painted come from one answer. The ripple's phase is
/// the status line's own clock (the turn's elapsed, or the viewed agent's
/// runtime), so the two animate in step.
pub(super) fn status_offline_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    if !strip_has_status(app) {
        return Vec::new();
    }
    let Some(outage) = app.offline() else {
        return Vec::new();
    };
    offline_lines(outage, strip_clock(app), width)
}

/// The clock the strip's status line runs on: the viewed agent's runtime
/// inside its session view, else the main turn's elapsed.
fn strip_clock(app: &App) -> Duration {
    app.viewed_agent().map_or_else(
        || app.status().map_or(Duration::ZERO, |status| status.elapsed),
        |run| run.runtime,
    )
}

/// `outage` as the rows hanging off the status line at `elapsed`: the `⎿`
/// gutter, the ripple frame for the moment (`ripple_spans`), a short gap,
/// then `No connection to {host} — trying again · {n} attempts` — the host
/// in the warning amber, the rest dim. The sentence
/// wraps to the width under its own column, so a long host or a narrow
/// terminal costs rows rather than the tail.
#[must_use]
pub fn offline_lines(outage: &OfflineInfo, elapsed: Duration, width: u16) -> Vec<Line<'static>> {
    let dim = Style::new().fg(tool_dim_color());
    let lit = Style::new().fg(status_offline_color());
    let ripple = ripple_spans(elapsed);
    let lead_cols = cols(TOOL_RESULT_PREFIX) + cols(OFFLINE_FRAMES[0]) + cols(OFFLINE_RIPPLE_GAP);
    let text_width = usize::from(width).saturating_sub(lead_cols).max(1);
    let text = format!(
        "{OFFLINE_ROW_LEAD}{}{OFFLINE_ROW_TAIL}{}",
        outage.host,
        attempts_text(outage.attempts)
    );
    wrap_text(&text, u16::try_from(text_width).unwrap_or(u16::MAX))
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            if i == 0 {
                spans.push(Span::styled(TOOL_RESULT_PREFIX.to_string(), dim));
                spans.extend(ripple.iter().cloned());
                spans.push(Span::raw(OFFLINE_RIPPLE_GAP.to_string()));
            } else {
                spans.push(Span::raw(" ".repeat(lead_cols)));
            }
            spans.extend(lit_once(&row, &outage.host, dim, lit));
            Line::from(spans)
        })
        .collect()
}

/// `{n} attempt` / `{n} attempts`.
fn attempts_text(attempts: u32) -> String {
    let plural = if attempts == 1 { "" } else { "s" };
    format!("{attempts} attempt{plural}")
}

/// `row` as spans in `base`, with the first occurrence of `needle` in `lit`
/// — the host lit on whichever wrapped row carries it (a host broken across
/// two rows stays in the base, a cut name being nothing to point at).
fn lit_once(row: &str, needle: &str, base: Style, lit: Style) -> Vec<Span<'static>> {
    let Some(start) = (!needle.is_empty()).then(|| row.find(needle)).flatten() else {
        return vec![Span::styled(row.to_string(), base)];
    };
    let end = start + needle.len();
    let mut spans = Vec::with_capacity(3);
    if start > 0 {
        spans.push(Span::styled(row[..start].to_string(), base));
    }
    spans.push(Span::styled(row[start..end].to_string(), lit));
    if end < row.len() {
        spans.push(Span::styled(row[end..].to_string(), base));
    }
    spans
}

/// The ripple's frame at `elapsed`, one span per cell: the dot in the warning
/// amber, each ring out faded [`OFFLINE_RING_FADE`] further toward the dim
/// by its distance from the dot, the blanks plain. Phase-driven by the
/// status line's clock ([`spinner_spans`](super::status::spinner_spans)'s
/// rule), so it is deterministic in tests and advances with the loop's
/// animation re-arm.
pub(super) fn ripple_spans(elapsed: Duration) -> Vec<Span<'static>> {
    let index = usize::try_from(elapsed.as_millis() / OFFLINE_FRAME_INTERVAL.as_millis().max(1))
        .unwrap_or(0)
        % OFFLINE_FRAMES.len();
    let frame = OFFLINE_FRAMES[index];
    let chars: Vec<char> = frame.chars().collect();
    let centre = chars.iter().position(|&c| c == '·').unwrap_or(0);
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            if c == ' ' {
                return Span::raw(c.to_string());
            }
            let distance = i.abs_diff(centre);
            #[allow(clippy::cast_precision_loss)] // a handful of cells
            let alpha = (1.0 - distance as f32 * OFFLINE_RING_FADE).max(0.0);
            Span::styled(
                c.to_string(),
                Style::new().fg(blend_color(status_offline_color(), tool_dim_color(), alpha)),
            )
        })
        .collect()
}
