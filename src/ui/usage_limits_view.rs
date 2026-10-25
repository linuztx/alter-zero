//! The read-only `/usage` page. See `docs/usage-limits.md`.
//!
//! The `/context` page's frame — the family's rules and two-column inset,
//! the `❯` on the highlighted section, the pages' title dress, the dim hint
//! — over every signed-in subscription's limits:
//!
//! - a **subtitle** counting the subscriptions and saying how fresh the
//!   numbers are (`updated just now`, `refreshing…`);
//! - one **section** per subscription: its name over its plan, the session's
//!   own marked `in use`, then a row per **window** — the label, a bar of
//!   what is used in the accent, the caution amber or the error red, the
//!   share, and when it resets (`resets in 6d 18h · Sun 8:00 AM`) — the
//!   counts behind a share under its bar, and the plan's **notes** (a credit
//!   balance, an allowance the plan does not count) in the same columns;
//! - the highlighted section's own **usage page**, linked, over the hint.
//!
//! [`usage_limits_lines`] builds the whole framed page line by line, so its
//! height is content-driven and falls out as `lines.len()` — and on a
//! terminal too short for it, the first of its shorter pages that fits.

use super::header::{page_title_line, page_title_spans};
use super::model_view::{
    compact_page, first_page_that_fits, list_window, model_linked_rows, model_placeholder_row,
    model_rule,
};
use super::theme::*;
use super::wrap::{clamp_spans, cols, ellipsize, wrap_text};
use super::*;

use crate::app::{SubscriptionLimits, UsageLimitsPage};
use crate::llm::usage_limits::{LimitNote, LimitWindow};

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

// ---------------------------------------------------------------------------
// The words
// ---------------------------------------------------------------------------

/// A share as the page says it: a whole percent, a sliver above nothing
/// `<1%` rather than a `0%` that would say nothing is used, and a share
/// short of whole `99%`, never a `100%` that would say the window is spent.
pub(super) fn share_text(used: f64) -> String {
    if used <= 0.0 {
        "0%".to_string()
    } else if used < 0.5 {
        "<1%".to_string()
    } else if used < 100.0 {
        format!("{:.0}%", used.round().min(99.0))
    } else {
        format!("{:.0}%", used.round())
    }
}

/// How long until a reset, in its two largest units — `42m`, `3h 12m`,
/// `6d 18h` — a zero second unit dropped (`3h`, `2d`), the days alone past a
/// week (`18d`), and anything under a minute `<1m`. A part minute rounds up:
/// the window is not back a minute early.
pub(super) fn countdown(seconds: i64) -> String {
    if seconds < MINUTE {
        return "<1m".to_string();
    }
    let minutes = (seconds + MINUTE - 1) / MINUTE;
    let days = minutes / (24 * 60);
    let hours = minutes % (24 * 60) / 60;
    let mins = minutes % 60;
    match (days, hours, mins) {
        (d, _, _) if d >= 7 => format!("{d}d"),
        (d, 0, _) if d > 0 => format!("{d}d"),
        (d, h, _) if d > 0 => format!("{d}d {h}h"),
        (_, h, 0) if h > 0 => format!("{h}h"),
        (_, h, m) if h > 0 => format!("{h}h {m}m"),
        (_, _, m) => format!("{m}m"),
    }
}

/// When a reset lands, as local wall-clock time said by how far off it is:
/// the time within a day (`5:12 PM`), the weekday and time within a week
/// (`Tue 2:00 PM`), the date beyond (`Nov 12`). `offset` is the local UTC
/// offset in force at a given Unix time — the boundary's time zone
/// (`App::set_local_offset`).
pub(super) fn reset_moment(now: i64, at: i64, offset: impl Fn(i64) -> i32) -> String {
    let Some(stamp) = chrono::DateTime::from_timestamp(at + i64::from(offset(at)), 0) else {
        return String::new();
    };
    let ahead = at - now;
    if ahead < DAY {
        stamp.format("%-I:%M %p").to_string()
    } else if ahead < 7 * DAY {
        stamp.format("%a %-I:%M %p").to_string()
    } else {
        stamp.format("%b %-d").to_string()
    }
}

/// How long ago the numbers came back, in one unit: `just now`, `3m ago`,
/// `2h ago`, `1d ago`.
pub(super) fn age_text(seconds: i64) -> String {
    if seconds < MINUTE {
        "just now".to_string()
    } else if seconds < HOUR {
        format!("{}m ago", seconds / MINUTE)
    } else if seconds < DAY {
        format!("{}h ago", seconds / HOUR)
    } else {
        format!("{}d ago", seconds / DAY)
    }
}

/// The filled cells of a `len`-cell bar at `used` percent: its own share,
/// rounded — a share above nothing at least one cell, and a share short of
/// whole never the whole bar.
pub(super) fn filled_cells(used: f64, len: usize) -> usize {
    if len == 0 || used <= 0.0 {
        return 0;
    }
    if used >= 100.0 {
        return len;
    }
    #[allow(clippy::cast_precision_loss)] // a bar's handful of cells
    let exact = used / 100.0 * len as f64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // in 0..=len
    let cells = exact.round() as usize;
    cells.clamp(1, len.saturating_sub(1).max(1))
}

// ---------------------------------------------------------------------------
// The page's inputs
// ---------------------------------------------------------------------------

/// What one build of the page reads, gathered once.
struct Page<'a> {
    app: &'a App,
    subs: &'a [SubscriptionLimits],
    selected: usize,
    now: i64,
}

impl<'a> Page<'a> {
    fn of(app: &'a App, state: &UsageLimitsPage) -> Self {
        let subs = app.subscription_limits();
        // With no clock injected (a test that never set one), the newest
        // answer stands in for now, so a countdown is never reckoned from
        // the epoch.
        let now = app
            .usage_limits_now()
            .unwrap_or_else(|| subs.iter().filter_map(|s| s.fetched_at).max().unwrap_or(0));
        Self {
            app,
            subs,
            selected: state.selected.min(subs.len().saturating_sub(1)),
            now,
        }
    }

    /// `subscription(s) · refreshing…` or `· updated {age}` — the freshness
    /// clause on its own.
    fn freshness(&self) -> Option<String> {
        if self.subs.iter().any(|s| s.refreshing) {
            return Some(USAGE_LIMITS_REFRESHING.to_string());
        }
        self.subs
            .iter()
            .filter_map(|s| s.fetched_at)
            .min()
            .map(|oldest| format!("updated {}", age_text(self.now - oldest)))
    }
}

/// `text` padded with spaces to `width` display columns.
fn pad(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(cols(text))))
}

/// The section body's inset: the page's, then the body's own.
fn body_indent() -> String {
    format!("{MODEL_INDENT}{USAGE_LIMITS_BODY_INDENT}")
}

// ---------------------------------------------------------------------------
// The columns
// ---------------------------------------------------------------------------

/// How a window row's reset clause is laid out at a width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResetFit {
    /// On the row, relative then the local moment.
    Full,
    /// On the row, relative alone.
    Short,
    /// Under the bar, on a row of its own — the row has no room for it.
    Below,
}

/// The page's shared columns: every window and note it shows lines up on
/// them, so the bars start in one column and the values with them.
#[derive(Debug, Clone, Copy)]
struct Columns {
    label: usize,
    bar: usize,
    fit: ResetFit,
}

impl Columns {
    /// Where a bar — and a note's value, and a window's counts — start.
    fn value_column(self) -> usize {
        cols(&body_indent()) + self.label + cols(USAGE_LIMITS_GAP)
    }
}

/// The share column's width: `100%` right-aligned, then ` used`.
const SHARE_COLS: usize = 4;

/// Lay out the windows and notes of the `shown` sections at `width`: the
/// label column the widest label (capped), the bar as long as the row
/// leaves it — with both parts of every reset clause on it if the bar keeps
/// [`USAGE_LIMITS_BAR_MIN`] cells, else with the relative part alone, else
/// with each clause on a row of its own under its bar.
fn columns(page: &Page<'_>, shown: &[usize], width: u16) -> Columns {
    let width = usize::from(width);
    let mut label = 0;
    let (mut full, mut short) = (0, 0);
    for limits in shown.iter().filter_map(|&i| page.subs[i].limits.as_ref()) {
        for window in &limits.windows {
            label = label.max(cols(&window.label));
            full = full.max(clause_cols(&reset_pieces(page, window, true)));
            short = short.max(clause_cols(&reset_pieces(page, window, false)));
        }
        for note in &limits.notes {
            label = label.max(cols(&note.label));
        }
    }
    let gap = cols(USAGE_LIMITS_GAP);
    let share = SHARE_COLS + cols(USAGE_LIMITS_USED);
    let around = cols(&body_indent()) + gap + gap + share;
    // A narrow terminal cuts the labels before it starves the bar.
    let label = label
        .min(USAGE_LIMITS_LABEL_MAX)
        .min(width.saturating_sub(around + USAGE_LIMITS_BAR_MIN).max(4));
    let room = |clause: usize| {
        let clause = if clause == 0 { 0 } else { gap + clause };
        width.saturating_sub(around + label + clause)
    };
    if room(full) >= USAGE_LIMITS_BAR_MIN {
        Columns {
            label,
            bar: room(full).min(USAGE_LIMITS_BAR_MAX),
            fit: ResetFit::Full,
        }
    } else if room(short) >= USAGE_LIMITS_BAR_MIN {
        Columns {
            label,
            bar: room(short).min(USAGE_LIMITS_BAR_MAX),
            fit: ResetFit::Short,
        }
    } else {
        Columns {
            label,
            bar: room(0).clamp(1, USAGE_LIMITS_BAR_MAX),
            fit: ResetFit::Below,
        }
    }
}

// ---------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------

/// A window's reset clause, as its parts: `limit reached` in red when the
/// window is used up, then `resets in {countdown}` and — when `full` — the
/// local moment, both dim. No parts at all for a window that names no
/// reset and is not used up.
fn reset_pieces(page: &Page<'_>, window: &LimitWindow, full: bool) -> Vec<(String, Style)> {
    let dim = Style::new().fg(model_meta_color());
    let mut pieces = Vec::new();
    if window.reached {
        pieces.push((
            USAGE_LIMITS_REACHED.to_string(),
            Style::new().fg(error_color()),
        ));
    }
    if let Some(at) = window.resets_at {
        let ahead = at - page.now;
        if ahead <= 0 {
            pieces.push(("resets now".to_string(), dim));
        } else {
            pieces.push((format!("resets in {}", countdown(ahead)), dim));
            if full {
                let moment = reset_moment(page.now, at, |t| page.app.local_offset(t));
                if !moment.is_empty() {
                    pieces.push((moment, dim));
                }
            }
        }
    }
    pieces
}

/// The display width of a clause's parts joined by ` · `.
fn clause_cols(pieces: &[(String, Style)]) -> usize {
    let separators = pieces.len().saturating_sub(1) * cols(USAGE_LIMITS_DOT);
    pieces.iter().map(|(text, _)| cols(text)).sum::<usize>() + separators
}

/// A clause's parts as spans, ` · ` between them.
fn clause_spans(pieces: Vec<(String, Style)>) -> Vec<Span<'static>> {
    let dim = Style::new().fg(model_meta_color());
    let mut spans = Vec::new();
    for (i, (text, style)) in pieces.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(USAGE_LIMITS_DOT, dim));
        }
        spans.push(Span::styled(text, style));
    }
    spans
}

/// A row under the bar's column — a window's counts, or a reset clause that
/// had no room on its row.
fn value_row(columns: Columns, spans: Vec<Span<'static>>, width: u16) -> Line<'static> {
    let mut row = vec![Span::raw(" ".repeat(columns.value_column()))];
    row.extend(spans);
    clamp_spans(row, usize::from(width))
}

/// One window's rows: `{label}  {bar}  {share} used  {reset}`, the reset
/// clause under the bar when the row has no room for it, and — with
/// `details` — the counts behind the share under that.
fn window_rows(
    page: &Page<'_>,
    window: &LimitWindow,
    columns: Columns,
    details: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let filled = filled_cells(window.used_percent, columns.bar);
    let bar = usage_limits_bar_color(window.used_percent, window.reached);
    let share = usage_limits_share_color(window.used_percent, window.reached);
    let mut spans = vec![
        Span::raw(body_indent()),
        Span::styled(
            pad(&ellipsize(&window.label, columns.label), columns.label),
            Style::new().fg(model_id_color()),
        ),
        Span::raw(USAGE_LIMITS_GAP),
        Span::styled(USAGE_LIMITS_BAR_FILL.repeat(filled), Style::new().fg(bar)),
        Span::styled(
            USAGE_LIMITS_BAR_EMPTY.repeat(columns.bar - filled),
            Style::new().fg(usage_limits_empty_color()),
        ),
        Span::raw(USAGE_LIMITS_GAP),
        Span::styled(
            format!("{:>SHARE_COLS$}", share_text(window.used_percent)),
            Style::new().fg(share),
        ),
        Span::styled(USAGE_LIMITS_USED, Style::new().fg(model_meta_color())),
    ];
    let mut rows = Vec::new();
    match columns.fit {
        ResetFit::Full | ResetFit::Short => {
            let pieces = reset_pieces(page, window, columns.fit == ResetFit::Full);
            if !pieces.is_empty() {
                spans.push(Span::raw(USAGE_LIMITS_GAP));
                spans.extend(clause_spans(pieces));
            }
            rows.push(clamp_spans(spans, usize::from(width)));
        }
        ResetFit::Below => {
            rows.push(clamp_spans(spans, usize::from(width)));
            let room = usize::from(width).saturating_sub(columns.value_column());
            let full = reset_pieces(page, window, true);
            let pieces = if clause_cols(&full) <= room {
                full
            } else {
                reset_pieces(page, window, false)
            };
            if clause_cols(&pieces) <= room {
                if !pieces.is_empty() {
                    rows.push(value_row(columns, clause_spans(pieces), width));
                }
            } else {
                // Still too long: a part per row, so nothing is cut.
                for piece in pieces {
                    rows.push(value_row(columns, clause_spans(vec![piece]), width));
                }
            }
        }
    }
    if details && let Some(detail) = &window.detail {
        let room = usize::from(width)
            .saturating_sub(columns.value_column())
            .max(1);
        let dim = Style::new().fg(model_meta_color());
        for row in wrap_text(detail, u16::try_from(room).unwrap_or(u16::MAX)) {
            rows.push(value_row(columns, vec![Span::styled(row, dim)], width));
        }
    }
    rows
}

/// One note's rows: `{label}  {value} · {detail}`, the value in the bars'
/// column — and, when the row cannot hold the detail, the detail under the
/// value instead, wrapped there, so nothing is cut.
fn note_rows(note: &LimitNote, columns: Columns, width: u16) -> Vec<Line<'static>> {
    let dim = Style::new().fg(model_meta_color());
    let mut spans = vec![
        Span::raw(body_indent()),
        Span::styled(
            pad(&ellipsize(&note.label, columns.label), columns.label),
            Style::new().fg(model_id_color()),
        ),
        Span::raw(USAGE_LIMITS_GAP),
        Span::styled(note.value.clone(), Style::new().fg(ai_color())),
    ];
    let Some(detail) = &note.detail else {
        return vec![clamp_spans(spans, usize::from(width))];
    };
    let used = columns.value_column() + cols(&note.value);
    if used + cols(USAGE_LIMITS_DOT) + cols(detail) <= usize::from(width) {
        spans.push(Span::styled(USAGE_LIMITS_DOT, dim));
        spans.push(Span::styled(detail.clone(), dim));
        return vec![clamp_spans(spans, usize::from(width))];
    }
    let mut rows = vec![clamp_spans(spans, usize::from(width))];
    let room = usize::from(width)
        .saturating_sub(columns.value_column())
        .max(1);
    for row in wrap_text(detail, u16::try_from(room).unwrap_or(u16::MAX)) {
        rows.push(value_row(columns, vec![Span::styled(row, dim)], width));
    }
    rows
}

/// A sentence in the section body — loading, a remark, an alert, an error —
/// word-wrapped at the body's inset, never cut.
fn body_sentence(text: &str, color: Color, width: u16) -> Vec<Line<'static>> {
    let indent = body_indent();
    let room = usize::from(width).saturating_sub(cols(&indent)).max(1);
    wrap_text(text, u16::try_from(room).unwrap_or(u16::MAX))
        .into_iter()
        .map(|row| {
            clamp_spans(
                vec![
                    Span::raw(indent.clone()),
                    Span::styled(row, Style::new().fg(color)),
                ],
                usize::from(width),
            )
        })
        .collect()
}

/// A folded section's clause: its tightest window's label and share, or
/// that it is loading or could not load — nothing for a plan with no
/// windows.
fn summary_spans(sub: &SubscriptionLimits) -> Vec<Span<'static>> {
    let dim = Style::new().fg(model_meta_color());
    let tightest = sub.limits.as_ref().and_then(|limits| {
        limits.windows.iter().reduce(|tightest, window| {
            if window.used_percent > tightest.used_percent {
                window
            } else {
                tightest
            }
        })
    });
    match (tightest, &sub.limits, &sub.error) {
        (Some(window), _, _) => vec![
            Span::styled(format!("{USAGE_LIMITS_DOT}{} ", window.label), dim),
            Span::styled(
                share_text(window.used_percent),
                Style::new().fg(usage_limits_share_color(
                    window.used_percent,
                    window.reached,
                )),
            ),
        ],
        (None, Some(_), _) => Vec::new(),
        (None, None, Some(_)) => vec![
            Span::styled(USAGE_LIMITS_DOT, dim),
            Span::styled(USAGE_LIMITS_FAILED_SUMMARY, Style::new().fg(error_color())),
        ],
        (None, None, None) => vec![Span::styled(
            format!("{USAGE_LIMITS_DOT}{USAGE_LIMITS_LOADING_SUMMARY}"),
            dim,
        )],
    }
}

/// A section's header row: the marker, the subscription's name in bold —
/// the accent on the highlighted section — and its plan dim, a folded
/// section's summary after it, and `in use` at the right edge for the
/// session's own when the row has room for it.
fn section_header(
    sub: &SubscriptionLimits,
    selected: bool,
    folded: bool,
    width: u16,
) -> Line<'static> {
    let (marker, marker_style, name_style) = if selected {
        (
            HOOKS_MARKER.to_string(),
            Style::new().fg(model_selected_color()),
            Style::new()
                .fg(model_selected_color())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            " ".repeat(cols(HOOKS_MARKER)),
            Style::default(),
            Style::new()
                .fg(model_id_color())
                .add_modifier(Modifier::BOLD),
        )
    };
    let dim = Style::new().fg(model_meta_color());
    let mut spans = vec![
        Span::raw(MODEL_INDENT),
        Span::styled(marker, marker_style),
        Span::styled(sub.entry.name.clone(), name_style),
    ];
    if let Some(plan) = sub
        .limits
        .as_ref()
        .and_then(|limits| limits.plan.as_deref())
    {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(plan.to_string(), dim));
    }
    if folded {
        spans.extend(summary_spans(sub));
    }
    if sub.entry.active {
        let used: usize = spans.iter().map(|span| cols(&span.content)).sum();
        let tail = cols(USAGE_LIMITS_IN_USE) + cols(MODEL_INDENT);
        let width = usize::from(width);
        if used + cols(USAGE_LIMITS_GAP) + tail <= width {
            spans.push(Span::raw(" ".repeat(width - used - tail)));
            spans.push(Span::styled(USAGE_LIMITS_IN_USE, dim));
        }
    }
    clamp_spans(spans, usize::from(width))
}

/// A section's body: its windows, its notes, an alert in red and a remark
/// dim — or, before its first answer, `Loading…` — and the newest failure
/// last, under whatever the last answer showed.
fn section_body(
    page: &Page<'_>,
    sub: &SubscriptionLimits,
    columns: Columns,
    details: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    match &sub.limits {
        Some(limits) => {
            for window in &limits.windows {
                rows.extend(window_rows(page, window, columns, details, width));
            }
            for note in &limits.notes {
                rows.extend(note_rows(note, columns, width));
            }
            if let Some(alert) = &limits.alert {
                rows.extend(body_sentence(alert, error_color(), width));
            }
            if let Some(remark) = &limits.remark {
                rows.extend(body_sentence(remark, model_meta_color(), width));
            }
        }
        None if sub.error.is_none() => {
            rows.extend(body_sentence(
                USAGE_LIMITS_LOADING,
                model_meta_color(),
                width,
            ));
        }
        None => {}
    }
    if let Some(error) = &sub.error {
        rows.extend(body_sentence(error, error_color(), width));
    }
    rows
}

// ---------------------------------------------------------------------------
// The blocks and the page
// ---------------------------------------------------------------------------

/// The title row over the subtitle — `{n} subscriptions · updated …`.
fn header_block(page: &Page<'_>, width: u16) -> Vec<Line<'static>> {
    let count = page.subs.len();
    let mut subtitle = format!(
        "{count} {}",
        if count == 1 {
            "subscription"
        } else {
            "subscriptions"
        }
    );
    if let Some(freshness) = page.freshness() {
        subtitle.push_str(USAGE_LIMITS_DOT);
        subtitle.push_str(&freshness);
    }
    vec![
        page_title_line(USAGE_LIMITS_TITLE, width),
        model_placeholder_row(&subtitle, model_meta_color(), width),
    ]
}

/// The header folded into one row for a short terminal: the title in its
/// dress, then how fresh the numbers are.
fn compact_title(page: &Page<'_>, width: u16) -> Line<'static> {
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    spans.extend(page_title_spans(USAGE_LIMITS_TITLE));
    if let Some(freshness) = page.freshness() {
        spans.push(Span::styled(
            format!("{USAGE_LIMITS_DOT}{freshness}"),
            Style::new().fg(model_meta_color()),
        ));
    }
    clamp_spans(spans, usize::from(width))
}

/// Every section whole, a block each.
fn section_blocks(page: &Page<'_>, details: bool, width: u16) -> Vec<Vec<Line<'static>>> {
    let all: Vec<usize> = (0..page.subs.len()).collect();
    let columns = columns(page, &all, width);
    page.subs
        .iter()
        .enumerate()
        .map(|(i, sub)| {
            let mut block = vec![section_header(sub, i == page.selected, false, width)];
            block.extend(section_body(page, sub, columns, details, width));
            block
        })
        .collect()
}

/// One block: every section folded to its header, the highlighted one
/// opened in place — an accordion.
fn accordion_block(page: &Page<'_>, width: u16) -> Vec<Vec<Line<'static>>> {
    let columns = columns(page, &[page.selected], width);
    let mut block = Vec::new();
    for (i, sub) in page.subs.iter().enumerate() {
        let open = i == page.selected;
        block.push(section_header(sub, open, !open, width));
        if open {
            block.extend(section_body(page, sub, columns, false, width));
        }
    }
    vec![block]
}

/// The compact page's window of folded sections, riding the highlight.
fn folded_window(page: &Page<'_>, max: usize, width: u16) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = list_window(page.subs.len(), page.selected, max);
    let rows = range
        .map(|i| section_header(&page.subs[i], i == page.selected, true, width))
        .collect();
    (above, rows, below)
}

/// The tail: the highlighted section's usage page — when `link` and it has
/// one — over the key hint.
fn tail_block(page: &Page<'_>, link: bool, width: u16) -> Vec<Line<'static>> {
    let mut block = Vec::new();
    if link
        && let Some(url) = page
            .subs
            .get(page.selected)
            .and_then(|sub| sub.entry.dashboard.as_deref())
    {
        // Clamped like every row here: the link's wrap keeps one column
        // however narrow the terminal, and a two-column one has none.
        block.extend(
            model_linked_rows(
                &format!("{USAGE_LIMITS_LINK_PREFIX}{url}"),
                model_meta_color(),
                width,
            )
            .into_iter()
            .map(|row| clamp_spans(row.spans, usize::from(width))),
        );
    }
    block.push(hint_row(page, width));
    block
}

/// The key hint — without `↑↓ select` for a lone subscription.
fn hint_row(page: &Page<'_>, width: u16) -> Line<'static> {
    let hint = if page.subs.len() > 1 {
        USAGE_LIMITS_HINT
    } else {
        USAGE_LIMITS_HINT_LONE
    };
    model_placeholder_row(hint, model_meta_color(), width)
}

/// Join `blocks` into the framed page: a top rule, the blocks with exactly
/// one blank row between them (so none hugs a rule — `docs/view-flow.md`
/// *The frame hugs its content*), and a bottom rule.
fn framed(blocks: Vec<Vec<Line<'static>>>, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![model_rule(width)];
    for (i, block) in blocks.into_iter().filter(|b| !b.is_empty()).enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.extend(block);
    }
    lines.push(model_rule(width));
    lines
}

/// The whole framed page as lines — the first of its pages that fits
/// `height` rows (`first_page_that_fits`, `docs/view-flow.md` *A page that
/// sheds rows before it flows*), yielding in this order: the windows'
/// counts, the link, the **other** sections' bodies (an accordion: every
/// section folded to its header and its tightest window, the highlighted
/// one opened in place), the subtitle — the title folding into one row with
/// how fresh the numbers are, since a page never loses its name. Past those
/// the sections
/// are a compact list — one folded row each, three riding the highlight
/// between the reserved `↑`/`↓` slots, under one title row carrying the
/// freshness and over the hint — whose ladder (`compact_page`) sheds down to
/// that title row alone, so no terminal is too short for a page and nothing
/// flows. What [`render_usage_limits`] paints and `usage_limits_menu_rows`
/// counts. Empty when the page is closed.
#[must_use]
pub fn usage_limits_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    let Some(state) = app.usage_limits.as_ref() else {
        return Vec::new();
    };
    let page = Page::of(app, state);
    let header = || header_block(&page, width);
    let title = || vec![compact_title(&page, width)];
    let whole = |details: bool, link: bool| {
        let mut blocks = vec![header()];
        blocks.extend(section_blocks(&page, details, width));
        blocks.push(tail_block(&page, link, width));
        framed(blocks, width)
    };
    let folded = |top: Vec<Line<'static>>| {
        let mut blocks = vec![top];
        blocks.extend(accordion_block(&page, width));
        blocks.push(tail_block(&page, false, width));
        framed(blocks, width)
    };
    let hint = || vec![hint_row(&page, width)];
    first_page_that_fits(
        height,
        &[
            &|| whole(true, true),
            &|| whole(false, true),
            &|| whole(false, false),
            &|| folded(header()),
            &|| folded(title()),
            &|| {
                compact_page(
                    height,
                    &compact_title(&page, width),
                    &|max| folded_window(&page, max, width),
                    &hint(),
                    width,
                )
            },
        ],
    )
}

/// The rows the page's own frame occupies — the built page's line count
/// ([`usage_limits_lines`]). What [`usage_limits_height`] reserves under the
/// strip, and what `render_live` hands [`render_usage_limits`].
pub(super) fn usage_limits_menu_rows(app: &App, width: u16, height: u16) -> u16 {
    u16::try_from(usage_limits_lines(app, width, height).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/usage` page is open, or `None`
/// when it isn't (the caller then falls back to `live_height`). Like every
/// sibling page it **replaces** the composer — and only the composer: the
/// streaming strip keeps its rows above it, so opening `/usage` mid-turn
/// never hides the running turn. Clamped to the terminal height.
#[must_use]
pub fn usage_limits_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.usage_limits.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        usage_limits_menu_rows(app, width, term_height),
        term_height,
    ))
}

/// Render the **inline** `/usage` page into the live region, in place of
/// the composer — the page built for the area's own height, bottom-anchored
/// like every framed page (`docs/view-flow.md`). Pure — `render_live` paints
/// this. See `docs/usage-limits.md`.
pub fn render_usage_limits(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        usage_limits_lines(app, area.width, area.height),
    );
}
