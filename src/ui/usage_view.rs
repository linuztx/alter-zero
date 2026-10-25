//! The read-only `/usage` page. See `docs/usage.md`.
//!
//! The `/context` page's frame — the family's rules and two-column inset,
//! the `❯` on the highlighted section, the pages' title dress, the dim hint
//! — over every signed-in subscription:
//!
//! - a **section** per subscription, its head naming the provider, the plan
//!   and the account, the one in use tagged flush right;
//! - a **meter** per limit — a bar of the share used, green until it warns,
//!   the share, the counts behind it, and when it resets, both as a
//!   countdown and as a local time — or a balance, or `Unlimited`, labels in
//!   one aligned column across the page;
//! - the provider's **notes** and a link to its own usage page.
//!
//! [`usage_view_lines`] builds the whole framed page line by line, so its
//! height is content-driven — and on a terminal too short for it, the first
//! of its shorter pages that fits. The page reads no clock: the countdowns
//! come off [`UsagePage::now`] and the local times off
//! [`App::local_offset`], both injected at the boundary.

use chrono::{DateTime, Datelike, FixedOffset};

use super::header::{page_title_line, page_title_spans};
use super::model_view::{
    compact_page, first_page_that_fits, model_placeholder_row, model_rule, model_wrapped_rows,
};
use super::theme::*;
use super::wrap::{clamp_spans, cols, ellipsize, wrap_text};
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
/// width, so they line up under the provider's name.
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
/// amber past [`USAGE_WARN_PERCENT`], red past [`USAGE_FULL_PERCENT`].
fn share_color(used: f64) -> Color {
    if used >= USAGE_FULL_PERCENT {
        usage_full_color()
    } else if used >= USAGE_WARN_PERCENT {
        usage_warn_color()
    } else {
        usage_meter_color()
    }
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

/// The parts of a limit's trailer, most important first: the counts behind
/// the share, how long until it resets, and when that is on the local clock.
struct Trailer {
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
            counts,
            countdown: countdown_text,
            clock,
        }
    }

    /// The trailer at `level` of detail: everything (`0`), without the clock
    /// time (`1`), the countdown alone (`2`) — what a narrowing page gives
    /// up, in that order.
    fn at(&self, level: usize) -> String {
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

    /// The trailer's forms, most complete first, each one shorter.
    fn candidates(&self) -> Vec<String> {
        let mut forms: Vec<String> = (0..3).map(|level| self.at(level)).collect();
        forms.retain(|form| !form.is_empty());
        forms.dedup();
        forms
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

/// **One** bar length for the whole page, so a column of meters lines up:
/// the longest bar, up to [`USAGE_BAR_MAX`], that leaves every limit room
/// for its trailer at the most complete level they can all keep beside a
/// bar of at least [`USAGE_BAR_MIN`] cells — each row then shows the most
/// of its own trailer that fits, so a short one keeps its clock time where
/// a long one gives it up. With no level that fits, every trailer moves
/// under its bar ([`limit_rows`]) and the bar takes the row.
fn bar_length(page: &UsagePage, app: &App, width: u16, label: usize) -> usize {
    let head = cols(&row_indent()) + label + cols(USAGE_LABEL_GAP);
    let row = (width as usize).saturating_sub(head + share_cols());
    let beside = row.saturating_sub(cols(USAGE_LABEL_GAP));
    let trailers: Vec<Trailer> = page
        .sections
        .iter()
        .filter_map(|section| section.report.as_ref())
        .flat_map(|report| &report.meters)
        .filter_map(|meter| match &meter.value {
            MeterValue::Limit(limit) => Some(Trailer::of(limit, app, page.now)),
            _ => None,
        })
        .collect();
    for level in 0..3 {
        let needed = trailers
            .iter()
            .map(|trailer| cols(&trailer.at(level)))
            .max()
            .unwrap_or(0);
        if beside >= USAGE_BAR_MIN + needed {
            return (beside - needed).min(USAGE_BAR_MAX);
        }
    }
    row.clamp(1, USAGE_BAR_MAX)
}

/// A limit's rows: the label, a `length`-cell bar of the share used (the
/// page's one length, [`bar_length`]), the share, and the most of its
/// [`Trailer`] that fits beside them. A trailer with no room beside the bar
/// at all moves to a row of its own under it, so how long until it resets
/// is never lost to a narrow terminal.
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
    let color = share_color(used);
    let percent = pad_left(&percent_text(used), 4);
    let room =
        width.saturating_sub(spans_cols(&head) + length + share_cols() + cols(USAGE_LABEL_GAP));
    let forms = Trailer::of(limit, app, now).candidates();
    let fitting = forms.iter().find(|form| cols(form) <= room);
    // A bar short of the whole never looks full, and a spent one always does.
    let mut filled =
        ((used.clamp(0.0, 100.0) / 100.0 * length as f64).round() as usize).min(length);
    if used < 100.0 && filled == length && length > 1 {
        filled -= 1;
    }
    if used >= 100.0 {
        filled = length;
    }
    let percent_style = if used >= USAGE_WARN_PERCENT {
        Style::new().fg(color)
    } else {
        Style::new().fg(ai_color())
    };
    let mut spans = head.clone();
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
    let dim = Style::new().fg(model_meta_color());
    let mut lines = Vec::new();
    match fitting {
        Some(form) => {
            spans.push(Span::raw(USAGE_LABEL_GAP));
            spans.push(Span::styled(form.clone(), dim));
            lines.push(clamp_spans(spans, width));
        }
        None => {
            lines.push(clamp_spans(spans, width));
            // The trailer under the bar, at the bar's own column.
            let under = spans_cols(&head);
            let room = width.saturating_sub(under);
            if let Some(form) = forms
                .iter()
                .find(|form| cols(form) <= room)
                .or(forms.last())
            {
                lines.push(clamp_spans(
                    vec![
                        Span::raw(" ".repeat(under)),
                        Span::styled(form.clone(), dim),
                    ],
                    width,
                ));
            }
        }
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
                    spans_cols(&spans) + cols(USAGE_LABEL_GAP) + cols(detail) <= width as usize;
                if fits {
                    spans.push(Span::raw(USAGE_LABEL_GAP));
                    spans.push(Span::styled(
                        detail.clone(),
                        Style::new().fg(model_meta_color()),
                    ));
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

/// The link to the provider's own usage page — the URL a real OSC 8 link
/// (`docs/links.md`), underlined, the rest of the row dim.
fn dashboard_row(url: &str, width: u16) -> Line<'static> {
    let dim = Style::new().fg(model_meta_color());
    let link = crate::links::linked(dim.add_modifier(Modifier::UNDERLINED), url);
    clamp_spans(
        vec![
            Span::raw(row_indent()),
            Span::styled(USAGE_DETAILS_PREFIX, dim),
            Span::styled(url.to_string(), link),
        ],
        width as usize,
    )
}

/// The fullest limit a report has, as `{label} {share} used` — a folded
/// section's summary.
fn fullest(report: &UsageReport) -> Option<(String, f64)> {
    report
        .meters
        .iter()
        .filter_map(|meter| match &meter.value {
            MeterValue::Limit(limit) => Some((meter, limit.used_percent)),
            _ => None,
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(meter, used)| (format!("{} {} used", meter.label, percent_text(used)), used))
}

/// How much of a section the page keeps open.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fold {
    /// Every row, the provider's link included.
    Open,
    /// Every row but the link.
    NoLink,
    /// The head row alone, with a summary of the fullest limit.
    Folded,
}

/// A section's head row: the marker and the provider's name (the accent on
/// the highlighted section), the plan and, dim, the account — or, folded,
/// the fullest limit — and `in use` flush right on the one the session runs
/// on. What gives way on a narrow row: the account, then the tag.
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
                (Some(report), _, _) => fullest(report).map(|(text, used)| {
                    let color = if used >= USAGE_WARN_PERCENT {
                        share_color(used)
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
        Fold::Open | Fold::NoLink => {
            if let Some(account) = report.and_then(|report| report.account.as_deref()) {
                tail.push(Span::styled(USAGE_SEPARATOR, dim));
                tail.push(Span::styled(account.to_string(), dim));
            }
        }
    }
    let width = width as usize;
    let tag = section
        .in_use
        .then(|| Span::styled(USAGE_IN_USE, Style::new().fg(usage_ok_color())));
    let with_tail = spans_cols(&spans) + spans_cols(&tail);
    let tag_cols = tag
        .as_ref()
        .map_or(0, |tag| cols(&tag.content) + cols(USAGE_LABEL_GAP));
    if with_tail + tag_cols <= width {
        spans.extend(tail);
    } else if fold != Fold::Folded && spans_cols(&spans) + tag_cols <= width {
        // The account gives way first.
    } else {
        spans.extend(tail);
        return clamp_spans(spans, width);
    }
    if let Some(tag) = tag {
        let used = spans_cols(&spans);
        let fill = width.saturating_sub(used + cols(&tag.content));
        spans.push(Span::raw(" ".repeat(fill)));
        spans.push(tag);
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
            if fold == Fold::Open
                && let Some(url) = &report.dashboard
            {
                lines.push(dashboard_row(url, width));
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

/// The subtitle: how many subscriptions, when they were last read, and
/// whether a read is in flight — or that the page is still checking.
fn subtitle(page: &UsagePage, app: &App, width: u16) -> Option<Line<'static>> {
    let text = if !page.ready {
        USAGE_CHECKING.to_string()
    } else if page.sections.is_empty() {
        return None;
    } else {
        let count = page.sections.len();
        let mut parts = vec![format!(
            "{count} subscription{}",
            if count == 1 { "" } else { "s" }
        )];
        if let Some(at) = page.updated_at {
            let time =
                clock_time(at, at, app.local_offset(at), app.local_offset(at)).unwrap_or_default();
            parts.push(format!("updated {time}"));
        }
        if page.loading() {
            let any_report = page.sections.iter().any(|section| section.report.is_some());
            parts.push(
                if any_report {
                    "refreshing…"
                } else {
                    "loading…"
                }
                .to_string(),
            );
        }
        parts.join(USAGE_SEPARATOR)
    };
    Some(model_placeholder_row(&text, model_meta_color(), width))
}

/// The dim key hint — without ↑↓ on a page with nothing to move between.
fn hint(page: &UsagePage, width: u16) -> Line<'static> {
    let text = if page.sections.len() > 1 {
        USAGE_HINT
    } else {
        USAGE_HINT_SINGLE
    };
    model_placeholder_row(text, model_meta_color(), width)
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

/// The header folded into one row for a short terminal: the title in its
/// dress, then the highlighted subscription's name.
fn compact_header(page: &UsagePage, width: u16) -> Line<'static> {
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    spans.extend(page_title_spans(USAGE_TITLE));
    if let Some(section) = page.sections.get(page.selected) {
        spans.push(Span::styled(
            USAGE_SEPARATOR,
            Style::new().fg(model_meta_color()),
        ));
        spans.push(Span::styled(
            ellipsize(&section.name, width as usize),
            Style::new().fg(model_id_color()),
        ));
    }
    clamp_spans(spans, width as usize)
}

/// The whole framed page as lines — the first of its pages that fits
/// `height` rows (`first_page_that_fits`, `docs/view-flow.md` *A page that
/// sheds rows before it flows*), yielding in this order: the providers'
/// links; every section but the highlighted one, folded to its head row and
/// its fullest limit (`/donate`'s accordion); the title and the subtitle;
/// and then the compact page, the highlighted section's rows windowed under
/// one header row, its ladder shedding down to that row alone — so no
/// terminal is too short and nothing flows. What [`render_usage_page`]
/// paints and `usage_menu_rows` counts. Empty when the page is closed.
#[must_use]
pub fn usage_view_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    let Some(page) = app.usage_page.as_ref() else {
        return Vec::new();
    };
    let now = page.now;
    let label = label_width(page);
    let columns = Columns {
        label,
        bar: bar_length(page, app, width, label),
    };
    let header = || {
        let mut rows = vec![page_title_line(USAGE_TITLE, width)];
        rows.extend(subtitle(page, app, width));
        rows
    };
    let tail = || vec![hint(page, width)];
    let empty = || {
        if page.ready && page.sections.is_empty() {
            vec![model_wrapped_rows(
                USAGE_NO_SUBSCRIPTION,
                model_selected_color(),
                width,
            )]
        } else {
            Vec::new()
        }
    };
    let open = |fold: Fold| -> Vec<Vec<Line<'static>>> {
        page.sections
            .iter()
            .enumerate()
            .map(|(i, section)| {
                section_rows(section, i == page.selected, fold, columns, app, now, width)
            })
            .collect()
    };
    let accordion = || -> Vec<Vec<Line<'static>>> {
        vec![
            page.sections
                .iter()
                .enumerate()
                .flat_map(|(i, section)| {
                    let fold = if i == page.selected {
                        Fold::NoLink
                    } else {
                        Fold::Folded
                    };
                    section_rows(section, i == page.selected, fold, columns, app, now, width)
                })
                .collect(),
        ]
    };
    let selected_rows = || {
        page.sections
            .get(page.selected)
            .map(|section| section_rows(section, true, Fold::NoLink, columns, app, now, width))
            .unwrap_or_default()
    };
    let compact_header_line = compact_header(page, width);
    let pages: [&dyn Fn() -> Vec<Line<'static>>; 5] = [
        &|| {
            framed(
                [vec![header()], empty(), open(Fold::Open), vec![tail()]].concat(),
                width,
            )
        },
        &|| {
            framed(
                [vec![header()], empty(), open(Fold::NoLink), vec![tail()]].concat(),
                width,
            )
        },
        &|| {
            framed(
                [vec![header()], empty(), accordion(), vec![tail()]].concat(),
                width,
            )
        },
        &|| framed([empty(), accordion(), vec![tail()]].concat(), width),
        &|| {
            let rows = selected_rows();
            compact_page(
                height,
                &compact_header_line,
                &|max| {
                    let shown = rows.len().min(max);
                    (0, rows[..shown].to_vec(), rows.len() - shown)
                },
                &tail(),
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
