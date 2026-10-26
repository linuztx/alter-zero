//! The read-only `/usage` page. See `docs/usage.md`.
//!
//! The `/context` page's frame — the family's rules and two-column inset,
//! the `❯` on the highlighted row, the pages' title dress, the dim hint —
//! over what each signed-in subscription's own server says is left of it:
//!
//! - a **header**: the title, then how many subscriptions and when their
//!   numbers are from (`as of 2:25 AM`), or that they are being read;
//! - a **block per subscription**: its name, plan and account (the one the
//!   session runs on tagged `in use`), then a row per limit — the label in
//!   a column aligned down the page, a bar of the share used, the share,
//!   and a dim trailer of what the row adds and when it resets — its
//!   balances, and its notes;
//! - the **highlighted subscription's dashboard**, a link, over the hint.
//!
//! [`usage_view_lines`] builds the whole framed page line by line, so its
//! height is content-driven and falls out as `lines.len()` — and on a
//! terminal too short for it, the first of its shorter pages that fits: the
//! other subscriptions collapsed to their header rows, then no header, then
//! the highlighted subscription alone (`/donate`'s accordion).

use chrono::{Datelike, FixedOffset, NaiveDate, TimeZone};

use super::header::page_title_line;
use super::model_view::{
    first_page_that_fits, model_linked_rows, model_placeholder_row, model_rule, model_wrapped_rows,
};
use super::theme::*;
use super::wrap::{clamp_spans, cols, ellipsize, wrap_text};
use super::*;

use crate::app::{UsageEntry, UsagePage, UsageState};
use crate::llm::plan_usage::{Allowance, Balance, Meter, Note, Reset};

// ---------------------------------------------------------------------------
// The words (pure)
// ---------------------------------------------------------------------------

/// A share used, the way a meter is read: `<1%` for a sliver that is not
/// nothing, a whole percent **floored** below that — `99.6%` is `99%`, so
/// the page never says a limit is spent before it is — and `100%` only when
/// it is.
pub(super) fn percent_text(used: f64) -> String {
    if used >= 100.0 {
        "100%".to_string()
    } else if used <= 0.0 {
        "0%".to_string()
    } else if used < 1.0 {
        "<1%".to_string()
    } else {
        format!("{}%", used.floor() as u32)
    }
}

/// How many of a `len`-cell bar's cells a share fills: rounded, but at
/// least one for a share that is not nothing and never all of them before
/// the limit is spent — the bar agrees with [`percent_text`] at its ends.
pub(super) fn bar_cells(used: f64, len: usize) -> usize {
    if len == 0 || used <= 0.0 {
        return 0;
    }
    if used >= 100.0 {
        return len;
    }
    let cells = (used / 100.0 * len as f64).round() as usize;
    cells.clamp(1, len.saturating_sub(1).max(1))
}

/// The clock `utc_offset` seconds east of UTC — the one the boundary read
/// off the machine. An offset no clock has falls back to UTC.
fn clock(utc_offset: i32) -> FixedOffset {
    FixedOffset::east_opt(utc_offset)
        .or_else(|| FixedOffset::east_opt(0))
        .expect("UTC is an offset")
}

/// `at` (Unix seconds) on that clock.
fn local(at: i64, utc_offset: i32) -> Option<chrono::DateTime<FixedOffset>> {
    clock(utc_offset).timestamp_opt(at, 0).single()
}

/// A wall-clock time — `2:25 AM` — the 12-hour clock the transcript's stamps
/// use, without the leading zero a sentence reads badly with.
fn clock_text(at: i64, utc_offset: i32) -> String {
    local(at, utc_offset).map_or_else(String::new, |at| at.format("%-I:%M %p").to_string())
}

/// A calendar day — `Nov 12`, or `Jan 3, 2027` in another year than `today`.
fn date_text(day: NaiveDate, today: NaiveDate) -> String {
    if day.year() == today.year() {
        day.format("%b %-d").to_string()
    } else {
        day.format("%b %-d, %Y").to_string()
    }
}

/// When a window starts over, worded against the moment its answer was
/// fetched — **absolute**, never relative, so a page left open never goes
/// stale on screen (codex's `/status` rule): `resets 2:36 PM` the same day,
/// `resets tomorrow 2:36 PM`, `resets Tue 2:36 PM` within the week, `resets
/// Nov 12` past it. A date-only reset (Copilot's monthly) is the day alone,
/// and a reset already past reads `resets now`.
pub(super) fn reset_text(reset: Reset, fetched_at: i64, utc_offset: i32) -> String {
    let Some(now) = local(fetched_at, utc_offset) else {
        return String::new();
    };
    let today = now.date_naive();
    match reset {
        Reset::At(at) => {
            let Some(when) = local(at, utc_offset).filter(|_| at > fetched_at) else {
                return "resets now".to_string();
            };
            let time = when.format("%-I:%M %p");
            match (when.date_naive() - today).num_days() {
                0 => format!("resets {time}"),
                1 => format!("resets tomorrow {time}"),
                2..=6 => format!("resets {} {time}", when.format("%a")),
                _ => format!("resets {}", date_text(when.date_naive(), today)),
            }
        }
        Reset::On(day) => match (day - today).num_days() {
            ..0 => "resets now".to_string(),
            0 => "resets today".to_string(),
            1 => "resets tomorrow".to_string(),
            _ => format!("resets {}", date_text(day, today)),
        },
    }
}

/// What a metered row adds after its share: the provider's own count (`93.5
/// of 300 left`, `$25 cap`), then the reset.
fn trailer_text(meter: &Meter, fetched_at: i64, utc_offset: i32) -> String {
    let reset = meter
        .resets
        .map(|reset| reset_text(reset, fetched_at, utc_offset));
    [meter.detail.clone(), reset]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(USAGE_SEPARATOR)
}

// ---------------------------------------------------------------------------
// The columns
// ---------------------------------------------------------------------------

/// The column geometry every row of a page shares, so the shares read down
/// one column: the label column's width and the bar's length (`0` for no
/// bar).
#[derive(Debug, Clone, Copy)]
struct Columns {
    label: usize,
    bar: usize,
}

/// The columns for `entries` at `width`: the label column as wide as the
/// widest label (a named bucket's rows counting their extra indent), held
/// to [`USAGE_LABEL_MAX_COLS`] and to what the row has room for; then the
/// bar as long as it can be while the **longest trailer** still fits its
/// row — down to [`USAGE_BAR_MIN`], where it stops yielding (a trailer that
/// still does not fit drops under the bar instead) — and gone when even the
/// shortest bar has no room.
fn columns<'a>(
    entries: impl Iterator<Item = &'a UsageEntry>,
    utc_offset: i32,
    width: u16,
) -> Columns {
    let (mut label, mut trailer) = (0, 0);
    for entry in entries {
        let UsageState::Ready { usage, fetched_at } = &entry.state else {
            continue;
        };
        for limit in &usage.limits {
            let indent = limit.scope.as_ref().map_or(0, |_| cols(USAGE_SCOPE_INDENT));
            label = label.max(indent + cols(&limit.label));
            if let Allowance::Metered(meter) = &limit.allowance {
                trailer = trailer.max(cols(&trailer_text(meter, *fetched_at, utc_offset)));
            }
        }
        for balance in &usage.balances {
            label = label.max(cols(&balance.label));
        }
    }
    let width = usize::from(width);
    let gap = cols(USAGE_COLUMN_GAP);
    let share = USAGE_SHARE_COLS + cols(USAGE_USED_SUFFIX);
    let room = width.saturating_sub(cols(USAGE_ROW_INDENT) + gap + share);
    let label = label.min(USAGE_LABEL_MAX_COLS).min(room).max(1);
    let base = cols(USAGE_ROW_INDENT) + label + gap;
    let avail = width.saturating_sub(base + share + gap);
    let need = if trailer > 0 { gap + trailer } else { 0 };
    let bar = if avail >= USAGE_BAR_MIN + need {
        USAGE_BAR_MAX.min(avail - need)
    } else if avail >= USAGE_BAR_MIN {
        USAGE_BAR_MIN
    } else {
        0
    };
    Columns { label, bar }
}

/// A label cell: `text` `…`-cut to the column and padded to it.
fn label_cell(text: &str, width: usize) -> String {
    let shown = ellipsize(text, width);
    let pad = width.saturating_sub(cols(&shown));
    format!("{shown}{}", " ".repeat(pad))
}

// ---------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------

/// `text` after `spans` when the row has room for it, else on its own dim
/// rows under the value column — wrapped there, never cut.
fn with_trailer(
    mut spans: Vec<Span<'static>>,
    trailer: &str,
    value_column: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let dim = Style::new().fg(model_meta_color());
    let width_cols = usize::from(width);
    if trailer.is_empty() {
        return vec![clamp_spans(spans, width_cols)];
    }
    let used: usize = spans.iter().map(|span| cols(&span.content)).sum();
    if used + cols(USAGE_COLUMN_GAP) + cols(trailer) <= width_cols {
        spans.push(Span::raw(USAGE_COLUMN_GAP));
        spans.push(Span::styled(trailer.to_string(), dim));
        return vec![clamp_spans(spans, width_cols)];
    }
    // A value column too far right to hold anything readable wraps from
    // the rows' own indent instead.
    let indent = if width_cols.saturating_sub(value_column) >= 12 {
        value_column
    } else {
        cols(USAGE_ROW_INDENT)
    };
    let room = width_cols.saturating_sub(indent).max(1) as u16;
    let mut lines = vec![clamp_spans(spans, width_cols)];
    for row in wrap_text(trailer, room) {
        lines.push(clamp_spans(
            vec![Span::raw(" ".repeat(indent)), Span::styled(row, dim)],
            width_cols,
        ));
    }
    lines
}

/// A metered limit's row: the label, the bar in the share's colour over the
/// dim empty cells, the share right-aligned in its column, and the trailer.
fn metered_rows(
    label: &str,
    meter: &Meter,
    columns: Columns,
    fetched_at: i64,
    utc_offset: i32,
    width: u16,
) -> Vec<Line<'static>> {
    let colour = Style::new().fg(usage_share_color(meter.used_percent));
    let mut spans = vec![
        Span::raw(USAGE_ROW_INDENT),
        Span::styled(
            label_cell(label, columns.label),
            Style::new().fg(model_id_color()),
        ),
        Span::raw(USAGE_COLUMN_GAP),
    ];
    if columns.bar > 0 {
        let filled = bar_cells(meter.used_percent, columns.bar);
        spans.push(Span::styled(USAGE_BAR_FILL.repeat(filled), colour));
        spans.push(Span::styled(
            USAGE_BAR_EMPTY.repeat(columns.bar - filled),
            Style::new().fg(usage_empty_color()),
        ));
        spans.push(Span::raw(USAGE_COLUMN_GAP));
    }
    spans.push(Span::styled(
        format!(
            "{:>width$}",
            percent_text(meter.used_percent),
            width = USAGE_SHARE_COLS
        ),
        colour,
    ));
    spans.push(Span::styled(
        USAGE_USED_SUFFIX,
        Style::new().fg(model_meta_color()),
    ));
    let value_column = cols(USAGE_ROW_INDENT) + columns.label + cols(USAGE_COLUMN_GAP);
    with_trailer(
        spans,
        &trailer_text(meter, fetched_at, utc_offset),
        value_column,
        width,
    )
}

/// An unlimited allowance's row: the label and the word, in the green a
/// share well inside its limit wears.
fn unlimited_row(label: &str, columns: Columns, width: u16) -> Line<'static> {
    clamp_spans(
        vec![
            Span::raw(USAGE_ROW_INDENT),
            Span::styled(
                label_cell(label, columns.label),
                Style::new().fg(model_id_color()),
            ),
            Span::raw(USAGE_COLUMN_GAP),
            Span::styled(USAGE_UNLIMITED, Style::new().fg(usage_ok_color())),
        ],
        usize::from(width),
    )
}

/// A balance's row: the label, the value bright, its detail dim.
fn balance_rows(balance: &Balance, columns: Columns, width: u16) -> Vec<Line<'static>> {
    let spans = vec![
        Span::raw(USAGE_ROW_INDENT),
        Span::styled(
            label_cell(&balance.label, columns.label),
            Style::new().fg(model_id_color()),
        ),
        Span::raw(USAGE_COLUMN_GAP),
        Span::styled(balance.value.clone(), Style::new().fg(ai_color())),
    ];
    let value_column = cols(USAGE_ROW_INDENT) + columns.label + cols(USAGE_COLUMN_GAP);
    with_trailer(
        spans,
        balance.detail.as_deref().unwrap_or_default(),
        value_column,
        width,
    )
}

/// `text` behind `mark` in `colour`, wrapped to the width with the
/// continuation rows under the text rather than the mark.
fn marked_rows(mark: &str, text: &str, colour: Color, width: u16) -> Vec<Line<'static>> {
    let style = Style::new().fg(colour);
    let lead = cols(USAGE_ROW_INDENT) + cols(mark);
    let room = usize::from(width).saturating_sub(lead).max(1) as u16;
    wrap_text(text, room)
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let head = if i == 0 {
                vec![
                    Span::raw(USAGE_ROW_INDENT),
                    Span::styled(mark.to_string(), style),
                ]
            } else {
                vec![Span::raw(" ".repeat(lead))]
            };
            let mut spans = head;
            spans.push(Span::styled(row, style));
            clamp_spans(spans, usize::from(width))
        })
        .collect()
}

/// A note: a warning behind `⚠` in amber, a plain fact dim.
fn note_rows(note: &Note, width: u16) -> Vec<Line<'static>> {
    if note.warning {
        marked_rows(USAGE_WARNING_MARK, &note.text, usage_warn_color(), width)
    } else {
        marked_rows("", &note.text, model_meta_color(), width)
    }
}

/// A dim one-row line at the rows' indent — the fetching and empty states,
/// and a named bucket's heading (in the muted label ink).
fn row_text(text: &str, colour: Color, width: u16) -> Line<'static> {
    clamp_spans(
        vec![
            Span::raw(USAGE_ROW_INDENT),
            Span::styled(text.to_string(), Style::new().fg(colour)),
        ],
        usize::from(width),
    )
}

/// A subscription's rows under its header: what it answered — each limit,
/// a named bucket's under a heading of its own, then the balances, then the
/// notes — or that its answer is still out, or why it failed.
fn entry_rows(
    entry: &UsageEntry,
    columns: Columns,
    utc_offset: i32,
    width: u16,
) -> Vec<Line<'static>> {
    let (usage, fetched_at) = match &entry.state {
        UsageState::Fetching => {
            return vec![row_text(USAGE_FETCHING, model_meta_color(), width)];
        }
        UsageState::Failed(reason) => {
            return marked_rows(USAGE_FAILED_MARK, reason, usage_full_color(), width);
        }
        UsageState::Ready { usage, fetched_at } => (usage, *fetched_at),
    };
    let mut rows = Vec::new();
    let mut scope: Option<&str> = None;
    for limit in &usage.limits {
        if limit.scope.as_deref() != scope {
            scope = limit.scope.as_deref();
            if let Some(heading) = scope {
                rows.push(row_text(heading, model_id_color(), width));
            }
        }
        let label = match &limit.scope {
            Some(_) => format!("{USAGE_SCOPE_INDENT}{}", limit.label),
            None => limit.label.clone(),
        };
        match &limit.allowance {
            Allowance::Metered(meter) => rows.extend(metered_rows(
                &label, meter, columns, fetched_at, utc_offset, width,
            )),
            Allowance::Unlimited => rows.push(unlimited_row(&label, columns, width)),
        }
    }
    for balance in &usage.balances {
        rows.extend(balance_rows(balance, columns, width));
    }
    for note in &usage.notes {
        rows.extend(note_rows(note, width));
    }
    if rows.is_empty() {
        rows.push(row_text(USAGE_NO_LIMITS, model_meta_color(), width));
    }
    rows
}

/// A collapsed subscription's summary on its header row: its tightest
/// limit (`· Weekly limit 100% used`), or that its answer is out or failed.
fn headline(entry: &UsageEntry) -> Vec<Span<'static>> {
    let dim = Style::new().fg(model_meta_color());
    let lead = Span::styled(USAGE_SEPARATOR, dim);
    match &entry.state {
        UsageState::Fetching => vec![lead, Span::styled("fetching…", dim)],
        UsageState::Failed(_) => vec![
            lead,
            Span::styled("couldn't read usage", Style::new().fg(usage_full_color())),
        ],
        UsageState::Ready { usage, .. } => {
            let tightest = usage
                .limits
                .iter()
                .filter_map(|limit| match &limit.allowance {
                    Allowance::Metered(meter) => Some((limit, meter)),
                    Allowance::Unlimited => None,
                })
                .fold(None, |best: Option<(&_, &Meter)>, candidate| match best {
                    Some(best) if best.1.used_percent >= candidate.1.used_percent => Some(best),
                    _ => Some(candidate),
                });
            let Some((limit, meter)) = tightest else {
                return Vec::new();
            };
            vec![
                lead,
                Span::styled(format!("{} ", limit.label), dim),
                Span::styled(
                    percent_text(meter.used_percent),
                    Style::new().fg(usage_share_color(meter.used_percent)),
                ),
                Span::styled(USAGE_USED_SUFFIX, dim),
            ]
        }
    }
}

/// A subscription's header row: the `❯` and the name in the accent when it
/// is highlighted (the muted ink and no marker otherwise), the plan, the
/// account dim, a collapsed block's summary — and `in use` flush right on
/// the subscription the session runs on. A narrow terminal drops the
/// account first, then the tag, then cuts what is left.
fn header_line(entry: &UsageEntry, selected: bool, collapsed: bool, width: u16) -> Line<'static> {
    let accent = model_selected_color();
    let marker = if selected {
        Span::styled(HOOKS_MARKER, Style::new().fg(accent))
    } else {
        Span::raw("  ")
    };
    let name = Style::new()
        .fg(if selected { accent } else { model_id_color() })
        .add_modifier(Modifier::BOLD);
    let (plan, account) = match &entry.state {
        UsageState::Ready { usage, .. } => (usage.plan.clone(), usage.account.clone()),
        _ => (None, None),
    };
    let build = |with_account: bool| {
        let mut spans = vec![
            Span::raw(MODEL_INDENT),
            marker.clone(),
            Span::styled(entry.name.clone(), name),
        ];
        if let Some(plan) = &plan {
            spans.push(Span::raw(USAGE_COLUMN_GAP));
            spans.push(Span::styled(plan.clone(), Style::new().fg(ai_color())));
        }
        if let (true, Some(account)) = (with_account, &account) {
            spans.push(Span::styled(
                USAGE_SEPARATOR,
                Style::new().fg(model_meta_color()),
            ));
            spans.push(Span::styled(
                account.clone(),
                Style::new().fg(model_meta_color()),
            ));
        }
        if collapsed {
            spans.extend(headline(entry));
        }
        spans
    };
    let width_cols = usize::from(width);
    let span_cols = |spans: &[Span<'static>]| spans.iter().map(|s| cols(&s.content)).sum::<usize>();
    let tag = cols(USAGE_IN_USE) + cols(USAGE_COLUMN_GAP);
    let fits = |spans: &[Span<'static>], tagged: bool| {
        span_cols(spans) + if tagged { tag } else { 0 } <= width_cols
    };
    let full = build(true);
    let (spans, tagged) = if fits(&full, entry.in_use) {
        (full, entry.in_use)
    } else {
        let short = build(false);
        let tagged = entry.in_use && fits(&short, true);
        (short, tagged)
    };
    let mut spans = spans;
    if tagged {
        let pad = width_cols - span_cols(&spans) - cols(USAGE_IN_USE);
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(
            USAGE_IN_USE,
            Style::new().fg(model_meta_color()),
        ));
    }
    clamp_spans(spans, width_cols)
}

/// A subscription's block: its header, and — unless it is `collapsed` —
/// its rows.
fn entry_block(
    entry: &UsageEntry,
    selected: bool,
    collapsed: bool,
    columns: Columns,
    utc_offset: i32,
    width: u16,
) -> Vec<Line<'static>> {
    let mut block = vec![header_line(entry, selected, collapsed, width)];
    if !collapsed {
        block.extend(entry_rows(entry, columns, utc_offset, width));
    }
    block
}

/// The subtitle: how many subscriptions, and when their numbers are from —
/// or that some are still being read.
fn subtitle(page: &UsagePage, width: u16) -> Line<'static> {
    let count = match page.entries.len() {
        1 => "1 subscription".to_string(),
        n => format!("{n} subscriptions"),
    };
    let when = if page.entries.iter().any(|entry| entry.refreshing) {
        "refreshing…".to_string()
    } else if page
        .entries
        .iter()
        .any(|entry| entry.state == UsageState::Fetching)
    {
        "fetching…".to_string()
    } else {
        format!("as of {}", clock_text(page.asked_at, page.utc_offset))
    };
    model_placeholder_row(
        &format!("{count}{USAGE_SEPARATOR}{when}"),
        model_meta_color(),
        width,
    )
}

/// The highlighted subscription's dashboard — a link, so a wrapped URL
/// still opens whole (`docs/links.md`).
fn details(page: &UsagePage, selected: usize, width: u16) -> Vec<Line<'static>> {
    page.entries
        .get(selected)
        .and_then(|entry| entry.dashboard.as_deref())
        .map_or_else(Vec::new, |url| {
            model_linked_rows(&format!("{USAGE_DETAILS}{url}"), model_meta_color(), width)
        })
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
/// sheds rows before it flows*):
///
/// 1. the **whole** page — the header, a block per subscription, the
///    highlighted one's dashboard, the hint;
/// 2. the subscriptions the highlight is **not** on collapsed to their
///    header rows, each naming its tightest limit, the highlighted one
///    still whole and laid out on its own columns;
/// 3. that page without the header;
/// 4. that page without the dashboard;
/// 5. the highlighted subscription alone over the hint.
///
/// The hint stays on every page; only a terminal shorter than the last one
/// bottom-anchors it. What [`render_usage_page`] paints and
/// `usage_menu_rows` counts, so the reserved height and the painted rows
/// can never disagree. Empty when the page is closed.
#[must_use]
pub fn usage_view_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    let Some(page) = app.usage_page.as_ref() else {
        return Vec::new();
    };
    let offset = page.utc_offset;
    let selected = page.selected.min(page.entries.len().saturating_sub(1));
    let header = || vec![page_title_line(USAGE_TITLE, width), subtitle(page, width)];
    let hint = || vec![model_placeholder_row(USAGE_HINT, model_meta_color(), width)];
    let dashboard = || details(page, selected, width);
    if page.entries.is_empty() {
        return framed(
            vec![
                header(),
                model_wrapped_rows(USAGE_NONE, model_meta_color(), width),
                hint(),
            ],
            width,
        );
    }
    let whole = columns(page.entries.iter(), offset, width);
    let own = columns(page.entries.get(selected).into_iter(), offset, width);
    let blocks = || {
        page.entries
            .iter()
            .enumerate()
            .map(|(i, entry)| entry_block(entry, i == selected, false, whole, offset, width))
            .collect::<Vec<_>>()
    };
    let accordion = || {
        vec![
            page.entries
                .iter()
                .enumerate()
                .flat_map(|(i, entry)| {
                    entry_block(entry, i == selected, i != selected, own, offset, width)
                })
                .collect::<Vec<_>>(),
        ]
    };
    let single = || {
        vec![entry_block(
            &page.entries[selected],
            true,
            false,
            own,
            offset,
            width,
        )]
    };
    let page_of = |parts: Vec<Vec<Vec<Line<'static>>>>| framed(parts.concat(), width);
    first_page_that_fits(
        height,
        &[
            &|| {
                page_of(vec![
                    vec![header()],
                    blocks(),
                    vec![dashboard()],
                    vec![hint()],
                ])
            },
            &|| {
                page_of(vec![
                    vec![header()],
                    accordion(),
                    vec![dashboard()],
                    vec![hint()],
                ])
            },
            &|| page_of(vec![accordion(), vec![dashboard()], vec![hint()]]),
            &|| page_of(vec![accordion(), vec![hint()]]),
            &|| page_of(vec![single(), vec![hint()]]),
        ],
    )
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

/// Render the **inline** `/usage` page into the live region, in place of
/// the composer — the page built for the area's own height, bottom-anchored
/// like every framed page (`docs/view-flow.md`). Pure — `render_live` paints
/// this. See `docs/usage.md`.
pub fn render_usage_page(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, usage_view_lines(app, area.width, area.height));
}
