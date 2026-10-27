//! The read-only `/usage` page. See `docs/usage.md`.
//!
//! The `/context` page's frame — the family's rules and two-column inset,
//! the `❯` on the highlighted section, the pages' title dress, the dim hint
//! — over every signed-in subscription:
//!
//! - a **subtitle** counting the subscriptions and saying how fresh the
//!   numbers are (`updated just now`, `updated 3m ago`, `refreshing…`);
//! - a **section** per subscription, its head naming the provider, the plan
//!   and the account — the one in use tagged `in use` after them — its rows
//!   indented under the name;
//! - a **meter** per limit — a bar of the share used, green until it warns
//!   and red once it is spent, the share, `limit reached` when it is, the
//!   counts behind it, and when it resets, both as a countdown and as a
//!   local time — or a balance, or `Unlimited`, labels in one aligned column
//!   across the page; a per-model limit sits under its model's own heading,
//!   so neither is ever cut to fit a column;
//! - the provider's **notes**, and the **highlighted** subscription's own
//!   usage page linked over the hint, which `c` copies.
//!
//! [`usage_view_lines`] builds the whole framed page line by line, so its
//! height is content-driven — and on a terminal too short for it, the first
//! of its shorter pages that fits. The page reads no clock: the countdowns
//! and the freshness come off [`UsagePage::now`] and the local times off
//! [`App::local_offset`], both injected at the boundary.

use chrono::{DateTime, Datelike, FixedOffset};

use super::header::{page_title_line, page_title_spans};
use super::model_view::{
    compact_page, first_page_that_fits, list_window, model_placeholder_row, model_rule,
    model_wrapped_rows, placeholder_window,
};
use super::theme::*;
use super::wrap::{clamp_spans, cols, wrap_text};
use super::*;

use crate::app::{UsagePage, UsageSection};
use crate::llm::usage::{Limit, MeterValue, NoteTone, UsageMeter, UsageReport, group_thousands};

/// `text` padded with spaces to `width` display columns.
fn pad(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(cols(text))))
}

/// `text` right-aligned in `width` display columns.
fn pad_left(text: &str, width: usize) -> String {
    format!("{}{text}", " ".repeat(width.saturating_sub(cols(text))))
}

/// The columns a run of spans takes.
fn spans_cols(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|span| cols(&span.content)).sum()
}

/// What a section's rows sit under: the page's inset plus the marker's
/// width and two more, so they read as one block under the provider's name.
fn row_indent() -> String {
    format!("{MODEL_INDENT}{USAGE_SECTION_INDENT}")
}

/// A share used as the page shows it — a whole percent that never claims
/// all or none it isn't: `<1%` for a sliver, `>99%` short of the whole, and
/// a share past the whole (an allowance run into overage) as `100%`.
fn percent_text(used: f64) -> String {
    let used = used.clamp(0.0, 100.0);
    if used > 0.0 && used < 0.5 {
        "<1%".to_string()
    } else if (99.5..100.0).contains(&used) {
        ">99%".to_string()
    } else {
        format!("{:.0}%", used.round())
    }
}

/// The colour a share is drawn in: the success green while it is healthy,
/// amber past [`USAGE_WARN_PERCENT`], red past [`USAGE_FULL_PERCENT`] — or
/// red once the provider says the limit is spent, whatever its share.
fn share_color(used: f64, reached: bool) -> Color {
    if reached || used >= USAGE_FULL_PERCENT {
        usage_full_color()
    } else if used >= USAGE_WARN_PERCENT {
        usage_warn_color()
    } else {
        usage_meter_color()
    }
}

/// The spent cells of a `length`-cell bar at `used` percent: its share,
/// rounded — any use at all keeps one cell, a share short of the whole never
/// fills the bar, and a limit used up always does.
fn filled_cells(used: f64, length: usize) -> usize {
    if length == 0 || used <= 0.0 {
        return 0;
    }
    if used >= 100.0 {
        return length;
    }
    let cells = (used / 100.0 * length as f64).round() as usize;
    cells.clamp(1, length.saturating_sub(1).max(1))
}

/// How long until `at`, from `now` (both Unix seconds): `<1m`, `26m`,
/// `4h 56m`, `1d 23h`, and whole days alone past ten of them — or `None`
/// once it is past, which the page words as `resets now`.
fn countdown(at: i64, now: i64) -> Option<String> {
    let left = at - now;
    if left <= 0 {
        return None;
    }
    let (days, hours, minutes) = (left / 86_400, left % 86_400 / 3_600, left % 3_600 / 60);
    Some(match (days, hours, minutes) {
        (0, 0, 0) => "<1m".to_string(),
        (0, 0, m) => format!("{m}m"),
        (0, h, 0) => format!("{h}h"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, _, _) if d >= 10 => format!("{d}d"),
        (d, 0, _) => format!("{d}d"),
        (d, h, _) => format!("{d}d {h}h"),
    })
}

/// When `at` is, on the local clock (`offset` seconds east of UTC there):
/// the time alone today (`3:58 PM`), the weekday within the week ahead
/// (`Sat 3:58 PM`), the date past it (`Nov 12, 1:26 PM`).
fn clock_time(at: i64, now: i64, at_offset: i32, now_offset: i32) -> Option<String> {
    let local = |instant: i64, offset: i32| -> Option<DateTime<FixedOffset>> {
        let zone = FixedOffset::east_opt(offset).or_else(|| FixedOffset::east_opt(0))?;
        DateTime::from_timestamp(instant, 0).map(|utc| utc.with_timezone(&zone))
    };
    let (at, now) = (local(at, at_offset)?, local(now, now_offset)?);
    let days = at.date_naive().num_days_from_ce() - now.date_naive().num_days_from_ce();
    let time = at.format("%-I:%M %p").to_string();
    Some(match days {
        0 => time,
        1..=6 => format!("{} {time}", at.format("%a")),
        _ => format!("{}, {time}", at.format("%b %-d")),
    })
}

/// How long ago the numbers were read, in one unit: `just now`, `3m ago`,
/// `2h ago`, `1d ago`.
fn age_text(seconds: i64) -> String {
    match seconds.max(0) {
        s if s < 60 => "just now".to_string(),
        s if s < 3_600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3_600),
        s => format!("{}d ago", s / 86_400),
    }
}

/// The parts of a limit's trailer, most important first: that it is spent,
/// the counts behind the share, how long until it resets, and when that is
/// on the local clock.
struct Trailer {
    reached: bool,
    counts: Option<String>,
    countdown: Option<String>,
    clock: Option<String>,
}

impl Trailer {
    fn of(limit: &Limit, app: &App, now: i64) -> Self {
        let counts = limit.counts.as_ref().map(|counts| {
            let mut text = format!(
                "{} of {}",
                group_thousands(counts.used),
                group_thousands(counts.total)
            );
            if let Some(unit) = counts.unit {
                text.push(' ');
                text.push_str(unit);
            }
            text
        });
        let (countdown_text, clock) = match limit.resets_at {
            Some(at) => match countdown(at, now) {
                Some(left) => (
                    Some(format!("resets in {left}")),
                    clock_time(at, now, app.local_offset(at), app.local_offset(now)),
                ),
                None => (Some("resets now".to_string()), None),
            },
            None => (None, None),
        };
        Self {
            reached: limit.reached,
            counts,
            countdown: countdown_text,
            clock,
        }
    }

    /// What follows `limit reached` at `level` of detail: everything (`0`),
    /// without the clock time (`1`), the countdown alone (`2`) — what a
    /// narrowing page gives up, in that order.
    fn rest(&self, level: usize) -> String {
        let keep: &[&Option<String>] = match level {
            0 => &[&self.counts, &self.countdown, &self.clock],
            1 => &[&self.counts, &self.countdown],
            _ => &[&self.countdown],
        };
        keep.iter()
            .filter_map(|part| part.as_deref())
            .collect::<Vec<_>>()
            .join(USAGE_SEPARATOR)
    }

    /// The whole trailer at `level`, as wide as the page shows it.
    fn at(&self, level: usize) -> String {
        let rest = self.rest(level);
        match (self.reached, rest.is_empty()) {
            (true, true) => USAGE_REACHED.to_string(),
            (true, false) => format!("{USAGE_REACHED}{USAGE_SEPARATOR}{rest}"),
            (false, _) => rest,
        }
    }

    /// The trailer at `level` as spans: `limit reached` in red, the rest dim.
    fn spans(&self, level: usize) -> Vec<Span<'static>> {
        let dim = Style::new().fg(model_meta_color());
        let rest = self.rest(level);
        let mut spans = Vec::new();
        if self.reached {
            spans.push(Span::styled(
                USAGE_REACHED,
                Style::new().fg(usage_full_color()),
            ));
            if !rest.is_empty() {
                spans.push(Span::styled(USAGE_SEPARATOR, dim));
            }
        }
        if !rest.is_empty() {
            spans.push(Span::styled(rest, dim));
        }
        spans
    }

    /// The levels of detail with something to show, most complete first,
    /// each one shorter than the last.
    fn levels(&self) -> Vec<usize> {
        let mut levels: Vec<usize> = Vec::new();
        for level in 0..3 {
            let form = self.at(level);
            let repeats = levels.last().is_some_and(|&last| self.at(last) == form);
            if !form.is_empty() && !repeats {
                levels.push(level);
            }
        }
        levels
    }
}

/// The columns a bar's share takes beside it: a space, the share
/// right-aligned in four columns, and ` used`.
fn share_cols() -> usize {
    1 + 4 + cols(" used")
}

/// The page's columns: how wide the labels are, and how long every bar is.
#[derive(Clone, Copy)]
struct Columns {
    label: usize,
    bar: usize,
}

/// The widest reset countdown a limit shows — a day or more reads shorter
/// (`6d 23h`, `17d`) — which a bar shortened for a narrow terminal leaves
/// room for beside it.
const WIDEST_COUNTDOWN: &str = "resets in 23h 59m";

/// **One** bar length for the whole page, so a column of meters lines up —
/// set by the terminal and the label column alone, never by the text beside
/// a bar: [`USAGE_BAR_MAX`] wherever the row has room for it and a countdown
/// beside it, shorter only on a terminal too narrow for that, and never
/// below [`USAGE_BAR_MIN`] unless the row cannot hold even that. A long row
/// costs only itself — it gives up its clock time, or moves its counts under
/// its bar ([`limit_rows`]) — and the bars hold their length as the
/// countdowns tick.
fn bar_length(width: u16, label: usize) -> usize {
    let head = cols(&row_indent()) + label + cols(USAGE_LABEL_GAP);
    let row = (width as usize).saturating_sub(head + share_cols());
    row.saturating_sub(cols(USAGE_LABEL_GAP) + cols(WIDEST_COUNTDOWN))
        .clamp(USAGE_BAR_MIN, USAGE_BAR_MAX)
        .min(row)
        .max(1)
}

/// A limit's rows: the label, a `length`-cell bar of the share used (the
/// page's one length, [`bar_length`]), the share, and the most of its
/// [`Trailer`] that fits beside them. A trailer with no room beside the bar
/// at all moves to a row of its own under it, and counts given up beside the
/// bar move under it too — whether the limit is spent, how much of it, and
/// how long until it resets are never lost to a narrow terminal.
fn limit_rows(
    head: Vec<Span<'static>>,
    limit: &Limit,
    length: usize,
    app: &App,
    now: i64,
    width: u16,
) -> Vec<Line<'static>> {
    let width = width as usize;
    let used = limit.used_percent;
    let color = share_color(used, limit.reached);
    let percent = pad_left(&percent_text(used), 4);
    let head_cols = spans_cols(&head);
    let room = width.saturating_sub(head_cols + length + share_cols() + cols(USAGE_LABEL_GAP));
    let trailer = Trailer::of(limit, app, now);
    let levels = trailer.levels();
    let fitting = levels
        .iter()
        .copied()
        .find(|&level| cols(&trailer.at(level)) <= room);
    let filled = filled_cells(used, length);
    let percent_style = if limit.reached || used >= USAGE_WARN_PERCENT {
        Style::new().fg(color)
    } else {
        Style::new().fg(ai_color())
    };
    let mut spans = head;
    spans.push(Span::styled(
        CONTEXT_USAGE_METER_FILL.repeat(filled),
        Style::new().fg(color),
    ));
    spans.push(Span::styled(
        CONTEXT_USAGE_METER_EMPTY.repeat(length - filled),
        Style::new().fg(usage_empty_color()),
    ));
    spans.retain(|span| !span.content.is_empty());
    spans.push(Span::raw(" "));
    spans.push(Span::styled(percent, percent_style));
    spans.push(Span::styled(" used", Style::new().fg(model_meta_color())));
    let mut lines = Vec::new();
    // Under the bar, at the bar's own column.
    let under = |spans: Vec<Span<'static>>| {
        let mut row = vec![Span::raw(" ".repeat(head_cols))];
        row.extend(spans);
        clamp_spans(row, width)
    };
    let shown = match fitting {
        Some(level) => {
            spans.push(Span::raw(USAGE_LABEL_GAP));
            spans.extend(trailer.spans(level));
            lines.push(clamp_spans(spans, width));
            Some(level)
        }
        None => {
            lines.push(clamp_spans(spans, width));
            let room = width.saturating_sub(head_cols);
            let level = levels
                .iter()
                .copied()
                .find(|&level| cols(&trailer.at(level)) <= room)
                .or(levels.last().copied());
            if let Some(level) = level {
                lines.push(under(trailer.spans(level)));
            }
            level
        }
    };
    // The countdown alone (level 2) gave the counts up: they sit under it.
    if let Some(counts) = &trailer.counts
        && shown.is_none_or(|level| level >= 2)
    {
        lines.push(under(vec![Span::styled(
            counts.clone(),
            Style::new().fg(model_meta_color()),
        )]));
    }
    lines
}

/// A meter's rows in the page's [`Columns`].
fn meter_rows(
    meter: &UsageMeter,
    columns: Columns,
    app: &App,
    now: i64,
    width: u16,
) -> Vec<Line<'static>> {
    let label_w = columns.label;
    let label = match meter.group {
        Some(_) => format!("{USAGE_GROUP_INDENT}{}", meter.label),
        None => meter.label.clone(),
    };
    let head = vec![
        Span::raw(row_indent()),
        Span::styled(pad(&label, label_w), Style::new().fg(model_id_color())),
        Span::raw(USAGE_LABEL_GAP),
    ];
    match &meter.value {
        MeterValue::Limit(limit) => limit_rows(head, limit, columns.bar, app, now, width),
        MeterValue::Unlimited => {
            let mut spans = head;
            spans.push(Span::styled(
                USAGE_UNLIMITED,
                Style::new().fg(usage_ok_color()),
            ));
            vec![clamp_spans(spans, width as usize)]
        }
        MeterValue::Text { value, detail } => {
            let mut spans = head;
            spans.push(Span::styled(value.clone(), Style::new().fg(ai_color())));
            if let Some(detail) = detail {
                let fits =
                    spans_cols(&spans) + cols(USAGE_SEPARATOR) + cols(detail) <= width as usize;
                if fits {
                    let dim = Style::new().fg(model_meta_color());
                    spans.push(Span::styled(USAGE_SEPARATOR, dim));
                    spans.push(Span::styled(detail.clone(), dim));
                }
            }
            vec![clamp_spans(spans, width as usize)]
        }
    }
}

/// `text` wrapped under the section's rows in `color`, opening with `mark`
/// and the continuation rows indented past it.
fn section_note(text: &str, mark: &str, color: Color, width: u16) -> Vec<Line<'static>> {
    let indent = row_indent();
    let room = (width as usize)
        .saturating_sub(cols(&indent) + cols(mark))
        .max(1) as u16;
    let style = Style::new().fg(color);
    wrap_text(text, room)
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let lead = if i == 0 {
                mark.to_string()
            } else {
                " ".repeat(cols(mark))
            };
            clamp_spans(
                vec![
                    Span::raw(indent.clone()),
                    Span::styled(lead, style),
                    Span::styled(row, style),
                ],
                width as usize,
            )
        })
        .collect()
}

/// A report's notes, each in its tone's colour — a warning or an error
/// opening with the mark.
fn note_rows(report: &UsageReport, width: u16) -> Vec<Line<'static>> {
    report
        .notes
        .iter()
        .flat_map(|note| match note.tone {
            NoteTone::Info => section_note(&note.text, "", model_meta_color(), width),
            NoteTone::Warning => {
                section_note(&note.text, USAGE_NOTE_MARK, usage_warn_color(), width)
            }
            NoteTone::Error => section_note(&note.text, USAGE_NOTE_MARK, usage_full_color(), width),
        })
        .collect()
}

/// The link to the highlighted subscription's own usage page — the URL a
/// real OSC 8 link (`docs/links.md`), underlined, the rest of the row dim.
fn dashboard_row(url: &str, width: u16) -> Line<'static> {
    let dim = Style::new().fg(model_meta_color());
    let link = crate::links::linked(dim.add_modifier(Modifier::UNDERLINED), url);
    clamp_spans(
        vec![
            Span::raw(MODEL_INDENT),
            Span::styled(USAGE_DETAILS_PREFIX, dim),
            Span::styled(url.to_string(), link),
        ],
        width as usize,
    )
}

/// The fullest limit a report has, as `{label} {share} used` with its share
/// and whether it is spent — a folded section's summary.
fn fullest(report: &UsageReport) -> Option<(String, f64, bool)> {
    report
        .meters
        .iter()
        .filter_map(|meter| match &meter.value {
            MeterValue::Limit(limit) => Some((meter, limit)),
            _ => None,
        })
        .max_by(|a, b| {
            (a.1.reached, a.1.used_percent)
                .partial_cmp(&(b.1.reached, b.1.used_percent))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(meter, limit)| {
            (
                format!("{} {} used", meter.label, percent_text(limit.used_percent)),
                limit.used_percent,
                limit.reached,
            )
        })
}

/// How much of a section the page keeps open.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fold {
    /// Every row.
    Open,
    /// The head row alone, with a summary of the fullest limit.
    Folded,
}

/// A section's head row: the marker and the provider's name (the accent on
/// the highlighted section), the plan and, dim, the account — or, folded,
/// the fullest limit — and, last in the same ` · ` chain, `in use` on the
/// one the session runs on, so it never floats off to the page's edge.
/// What gives way on a narrow row: the account, then the tag.
fn head_row(section: &UsageSection, selected: bool, fold: Fold, width: u16) -> Line<'static> {
    let accent = Style::new().fg(model_selected_color());
    let dim = Style::new().fg(model_meta_color());
    let (marker, marker_style, name_style) = if selected {
        (HOOKS_MARKER, accent, accent.add_modifier(Modifier::BOLD))
    } else {
        (
            "  ",
            Style::default(),
            Style::new()
                .fg(model_id_color())
                .add_modifier(Modifier::BOLD),
        )
    };
    let mut spans = vec![
        Span::raw(MODEL_INDENT),
        Span::styled(marker, marker_style),
        Span::styled(section.name.clone(), name_style),
    ];
    let report = section.report.as_ref();
    if let Some(plan) = report.and_then(|report| report.plan.as_deref()) {
        spans.push(Span::raw(USAGE_LABEL_GAP));
        spans.push(Span::styled(plan.to_string(), Style::new().fg(ai_color())));
    }
    let mut tail: Vec<Span<'static>> = Vec::new();
    match fold {
        Fold::Folded => {
            let summary = match (report, &section.error, section.loading) {
                (Some(report), _, _) => fullest(report).map(|(text, used, reached)| {
                    let color = if reached || used >= USAGE_WARN_PERCENT {
                        share_color(used, reached)
                    } else {
                        model_meta_color()
                    };
                    Span::styled(text, Style::new().fg(color))
                }),
                (None, Some(_), _) => Some(Span::styled(
                    "couldn't load",
                    Style::new().fg(usage_full_color()),
                )),
                (None, None, true) => Some(Span::styled("loading…", dim)),
                (None, None, false) => None,
            };
            if let Some(summary) = summary {
                tail.push(Span::styled(USAGE_SEPARATOR, dim));
                tail.push(summary);
            }
        }
        Fold::Open => {
            if let Some(account) = report.and_then(|report| report.account.as_deref()) {
                tail.push(Span::styled(USAGE_SEPARATOR, dim));
                tail.push(Span::styled(account.to_string(), dim));
            }
        }
    }
    let width = width as usize;
    let tag = if section.in_use {
        vec![
            Span::styled(USAGE_SEPARATOR, dim),
            Span::styled(USAGE_IN_USE, Style::new().fg(usage_ok_color())),
        ]
    } else {
        Vec::new()
    };
    let (head_cols, tag_cols) = (spans_cols(&spans), spans_cols(&tag));
    if head_cols + spans_cols(&tail) + tag_cols <= width {
        spans.extend(tail);
        spans.extend(tag);
    } else if fold == Fold::Open && head_cols + tag_cols <= width {
        // The account gives way first.
        spans.extend(tag);
    } else {
        spans.extend(tail);
    }
    clamp_spans(spans, width)
}

/// A section's rows as `fold` keeps them, in the page's [`Columns`].
fn section_rows(
    section: &UsageSection,
    selected: bool,
    fold: Fold,
    columns: Columns,
    app: &App,
    now: i64,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![head_row(section, selected, fold, width)];
    if fold == Fold::Folded {
        return lines;
    }
    let indented =
        |text: &str, color: Color| -> Vec<Line<'static>> { section_note(text, "", color, width) };
    match &section.report {
        None => match &section.error {
            Some(error) => lines.extend(indented(
                &format!("{USAGE_LOAD_FAILED}{error}"),
                usage_full_color(),
            )),
            None if section.loading => lines.extend(indented(USAGE_LOADING, model_meta_color())),
            None => {}
        },
        Some(report) => {
            let mut group: Option<&str> = None;
            for meter in &report.meters {
                if meter.group.as_deref() != group {
                    group = meter.group.as_deref();
                    if let Some(name) = group {
                        lines.push(clamp_spans(
                            vec![
                                Span::raw(row_indent()),
                                Span::styled(
                                    name.to_string(),
                                    Style::new()
                                        .fg(model_id_color())
                                        .add_modifier(Modifier::BOLD),
                                ),
                            ],
                            width as usize,
                        ));
                    }
                }
                lines.extend(meter_rows(meter, columns, app, now, width));
            }
            lines.extend(note_rows(report, width));
            if let Some(error) = &section.error {
                lines.extend(section_note(
                    &format!("{USAGE_REFRESH_FAILED}{error}"),
                    USAGE_NOTE_MARK,
                    usage_warn_color(),
                    width,
                ));
            }
        }
    }
    lines
}

/// The label column's width: the widest label on the page, a per-model
/// limit's counted with its inset — one column for every section, so the
/// bars start together.
fn label_width(page: &UsagePage) -> usize {
    page.sections
        .iter()
        .filter_map(|section| section.report.as_ref())
        .flat_map(|report| &report.meters)
        .map(|meter| {
            let inset = if meter.group.is_some() {
                cols(USAGE_GROUP_INDENT)
            } else {
                0
            };
            inset + cols(&meter.label)
        })
        .max()
        .unwrap_or(0)
}

/// How fresh the page's numbers are: how long ago they were read (`updated
/// 3m ago`) and whether a read is in flight — `refreshing…` over the
/// numbers an earlier round left, `loading…` while the first round is still
/// landing (a subscription read in it is new, not refreshed). What the
/// subtitle and the compact title say after their own words.
fn freshness(page: &UsagePage) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(at) = page.updated_at {
        parts.push(format!("updated {}", age_text(page.now - at)));
    }
    if page.loading() {
        parts.push(
            if page.updated_at.is_some() {
                USAGE_REFRESHING
            } else {
                USAGE_LOADING_ALL
            }
            .to_string(),
        );
    }
    parts
}

/// The subtitle: how many subscriptions, and how fresh their numbers are —
/// none on a page with no subscription left on it, where the advice stands.
fn subtitle(page: &UsagePage, width: u16) -> Option<Line<'static>> {
    if page.sections.is_empty() {
        return None;
    }
    let count = page.sections.len();
    let mut parts = vec![format!(
        "{count} subscription{}",
        if count == 1 { "" } else { "s" }
    )];
    parts.extend(freshness(page));
    Some(model_placeholder_row(
        &parts.join(USAGE_SEPARATOR),
        model_meta_color(),
        width,
    ))
}

/// The header folded into one row for a short terminal — a page never loses
/// its name: the title in its dress, then how fresh the numbers are.
fn compact_title(page: &UsagePage, width: u16) -> Line<'static> {
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    spans.extend(page_title_spans(USAGE_TITLE));
    for part in freshness(page) {
        spans.push(Span::styled(
            format!("{USAGE_SEPARATOR}{part}"),
            Style::new().fg(model_meta_color()),
        ));
    }
    clamp_spans(spans, width as usize)
}

/// The dim key hint — `↑↓ select` only with more than one subscription to
/// move between, `c copy link` only with a link to copy.
fn hint(page: &UsagePage, width: u16) -> Line<'static> {
    let mut parts = Vec::new();
    if page.sections.len() > 1 {
        parts.push(USAGE_HINT_SELECT);
    }
    parts.push(USAGE_HINT_REFRESH);
    if page
        .highlighted()
        .is_some_and(|section| section.dashboard.is_some())
    {
        parts.push(USAGE_HINT_COPY);
    }
    parts.push(USAGE_HINT_CLOSE);
    model_placeholder_row(&parts.join(USAGE_LABEL_GAP), model_meta_color(), width)
}

/// The page's foot: the highlighted subscription's own usage page — when
/// `link` and it has one — over the key hint.
fn tail(page: &UsagePage, link: bool, width: u16) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    if link
        && let Some(url) = page
            .highlighted()
            .and_then(|section| section.dashboard.as_deref())
    {
        rows.push(dashboard_row(url, width));
    }
    rows.push(hint(page, width));
    rows
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
/// sheds rows before it flows*), yielding in this order: the link; every
/// section but the highlighted one, folded to its head row and its fullest
/// limit (`/donate`'s accordion); the subtitle, the title folding into one
/// row with how fresh the numbers are, since a page never loses its name;
/// and then the compact page, one folded row per subscription riding the
/// highlight under that title row, its ladder shedding down to the title row
/// alone — so no terminal is too short and nothing flows. What
/// [`render_usage_page`] paints and `usage_menu_rows` counts. Empty when the
/// page is closed.
#[must_use]
pub fn usage_view_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    let Some(page) = app.usage_page.as_ref() else {
        return Vec::new();
    };
    let now = page.now;
    let label = label_width(page);
    let columns = Columns {
        label,
        bar: bar_length(width, label),
    };
    let header = || {
        let mut rows = vec![page_title_line(USAGE_TITLE, width)];
        rows.extend(subtitle(page, width));
        rows
    };
    let empty = || -> Vec<Line<'static>> {
        if page.sections.is_empty() {
            model_wrapped_rows(USAGE_NO_SUBSCRIPTION, model_selected_color(), width)
        } else {
            Vec::new()
        }
    };
    let open = || -> Vec<Vec<Line<'static>>> {
        page.sections
            .iter()
            .enumerate()
            .map(|(i, section)| {
                section_rows(
                    section,
                    i == page.selected,
                    Fold::Open,
                    columns,
                    app,
                    now,
                    width,
                )
            })
            .collect()
    };
    let accordion = || -> Vec<Line<'static>> {
        page.sections
            .iter()
            .enumerate()
            .flat_map(|(i, section)| {
                let fold = if i == page.selected {
                    Fold::Open
                } else {
                    Fold::Folded
                };
                section_rows(section, i == page.selected, fold, columns, app, now, width)
            })
            .collect()
    };
    let whole = |link: bool| {
        let mut blocks = vec![header(), empty()];
        blocks.extend(open());
        blocks.push(tail(page, link, width));
        framed(blocks, width)
    };
    let folded = |top: Vec<Line<'static>>| {
        framed(
            vec![top, empty(), accordion(), tail(page, false, width)],
            width,
        )
    };
    let heads: Vec<Line<'static>> = page
        .sections
        .iter()
        .enumerate()
        .map(|(i, section)| head_row(section, i == page.selected, Fold::Folded, width))
        .collect();
    let placeholder = empty();
    let title_row = compact_title(page, width);
    let hint_row = [hint(page, width)];
    let pages: [&dyn Fn() -> Vec<Line<'static>>; 5] = [
        &|| whole(true),
        &|| whole(false),
        &|| folded(header()),
        &|| folded(vec![compact_title(page, width)]),
        &|| {
            compact_page(
                height,
                &title_row,
                &|max| {
                    if heads.is_empty() {
                        return placeholder_window(&placeholder, max);
                    }
                    let (above, range, below) = list_window(heads.len(), page.selected, max);
                    (above, heads[range].to_vec(), below)
                },
                &hint_row,
                width,
            )
        },
    ];
    first_page_that_fits(height, &pages)
}

/// The rows the page's own frame occupies — the built page's line count
/// ([`usage_view_lines`]). What [`usage_page_height`] reserves under the
/// strip, and what `render_live` hands [`render_usage_page`].
pub(super) fn usage_menu_rows(app: &App, width: u16, height: u16) -> u16 {
    u16::try_from(usage_view_lines(app, width, height).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/usage` page is open, or `None`
/// when it isn't (the caller then falls back to `live_height`). Like every
/// sibling page it **replaces** the composer — and only the composer: the
/// streaming strip keeps its rows above it, so opening `/usage` mid-turn
/// never hides the running turn. Clamped to the terminal height.
#[must_use]
pub fn usage_page_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.usage_page.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        usage_menu_rows(app, width, term_height),
        term_height,
    ))
}

/// Render the **inline** `/usage` page into the live region, in place of the
/// composer — the page built for the area's own height, bottom-anchored like
/// every framed page (`docs/view-flow.md`). Pure — `render_live` paints this.
/// See `docs/usage.md`.
pub fn render_usage_page(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, usage_view_lines(app, area.width, area.height));
}
