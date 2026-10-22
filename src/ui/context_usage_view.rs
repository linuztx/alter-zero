//! The read-only `/context` page. See `docs/context-usage.md`.
//!
//! The `/donate` page's frame — the family's rules and two-column inset,
//! the `❯` on the highlighted row, the pages' title dress, the dim hint —
//! over the context window of the conversation on screen:
//!
//! - a **subtitle** naming the model and the window's fill, the share
//!   turning amber and then red as it nears the threshold where the session
//!   compacts itself;
//! - a **bar** standing for the whole window, one cell per slice of it in
//!   its category's colour, then the free space and the hatched headroom
//!   auto-compact keeps ([`allot_cells`] places every boundary within half a
//!   cell of where it belongs);
//! - a **legend** — a swatch, the label, the tokens and the share in
//!   aligned columns — whose highlighted row's **detail** (its parts,
//!   largest first, each with a bar against the largest) sits beside it on
//!   a wide terminal and under it on a narrow one;
//! - what the **provider** reported: the last request, its cache hit as a
//!   meter, and every request since launch (or, in a subagent's view, the
//!   agent's own).
//!
//! [`context_usage_lines`] builds the whole framed page line by line, so its
//! height is content-driven and falls out as `lines.len()` — and on a
//! terminal too short for it, the first of its shorter pages that fits.

use super::header::{page_title_line, page_title_spans};
use super::model_view::{first_page_that_fits, model_placeholder_row, model_rule};
use super::theme::*;
use super::wrap::{clamp_spans, cols, ellipsize, wrap_text};
use super::*;

use crate::app::{ContextUsagePage, ContextUsageRow, PathDisplay};
use crate::context_usage::{Category, Part, Slice, UsageLedger, cache_share};
use crate::stream::TokenUsage;

/// What the page shows, gathered once per build from the conversation on
/// screen: the main session's, or the viewed subagent's.
struct Gauge<'a> {
    page: &'a ContextUsagePage,
    rows: Vec<ContextUsageRow>,
    window: Option<u64>,
    threshold: Option<u64>,
    total: u64,
    model: Option<String>,
    last: Option<TokenUsage>,
    ledger: UsageLedger,
    ledger_label: &'static str,
    agent: bool,
    /// The session's file-path display rule, which a doc's path is shown by.
    paths: &'a PathDisplay,
}

impl<'a> Gauge<'a> {
    fn of(app: &'a App) -> Option<Self> {
        let page = app.context_usage.as_ref()?;
        let run = app.viewed_agent();
        let session_model = app
            .session
            .as_ref()
            .map(|session| session.model.clone())
            .filter(|model| !model.is_empty());
        let model = match run {
            Some(_) => app
                .viewed_agent_model()
                .map(str::to_string)
                .or(session_model),
            None => session_model,
        };
        let (last, ledger, ledger_label) = match run {
            Some(run) => (run.last_usage, run.usage, CONTEXT_USAGE_THIS_AGENT),
            None => (
                app.last_usage(),
                app.usage_since_launch(),
                CONTEXT_USAGE_SINCE_LAUNCH,
            ),
        };
        Some(Self {
            page,
            rows: app.context_usage_rows(),
            window: app.context_usage_window(),
            threshold: app.context_usage_threshold(),
            total: u64::try_from(page.breakdown.total()).unwrap_or(u64::MAX),
            model,
            last,
            ledger,
            ledger_label,
            agent: run.is_some(),
            paths: app.path_display(),
        })
    }

    /// The room left before the conversation compacts itself — or, with no
    /// threshold, before the window is full.
    fn free(&self) -> u64 {
        match (self.window, self.threshold) {
            (Some(_), Some(threshold)) => threshold.saturating_sub(self.total),
            (Some(window), None) => window.saturating_sub(self.total),
            (None, _) => 0,
        }
    }

    /// The headroom past the threshold the conversation has not filled.
    fn reserved(&self) -> u64 {
        match (self.window, self.threshold) {
            (Some(window), Some(threshold)) => window.saturating_sub(self.total.max(threshold)),
            _ => 0,
        }
    }

    /// A legend row's tokens.
    fn tokens(&self, row: ContextUsageRow) -> u64 {
        match row {
            ContextUsageRow::Slice(category) => self
                .page
                .breakdown
                .slice(category)
                .map_or(0, |slice| u64::try_from(slice.tokens).unwrap_or(u64::MAX)),
            ContextUsageRow::Free => self.free(),
            ContextUsageRow::AutoCompact => self.reserved(),
        }
    }

    /// What a share is of: the window when it is known, else the total.
    fn whole(&self) -> u64 {
        self.window.unwrap_or(self.total)
    }
}

/// A token count, humanized like the footer's gauge and the receipt.
fn token_text(tokens: u64) -> String {
    format_token_count(usize::try_from(tokens).unwrap_or(usize::MAX))
}

/// `part` as a share of `whole`, to one decimal — `<0.1%` for a sliver that
/// is not nothing, so a 160-token system prompt never reads `0.0%`.
fn share_text(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "0.0%".to_string();
    }
    let percent = part as f64 * 100.0 / whole as f64;
    if part > 0 && percent < 0.05 {
        "<0.1%".to_string()
    } else {
        format!("{percent:.1}%")
    }
}

/// `text` padded with spaces to `width` display columns.
fn pad(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(cols(text))))
}

/// `text` right-aligned in `width` display columns.
fn pad_left(text: &str, width: usize) -> String {
    format!("{}{text}", " ".repeat(width.saturating_sub(cols(text))))
}

/// `spans` cut to `width` columns (a trailing `…` saying so) — the guard
/// every row of the page goes through, so none is ever wider than the
/// terminal.
fn clip(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    clamp_spans(spans, width).spans
}

fn row_label(row: ContextUsageRow) -> &'static str {
    match row {
        ContextUsageRow::Slice(category) => category.label(),
        ContextUsageRow::Free => CONTEXT_USAGE_FREE_LABEL,
        ContextUsageRow::AutoCompact => CONTEXT_USAGE_RESERVED_LABEL,
    }
}

fn row_swatch(row: ContextUsageRow) -> &'static str {
    match row {
        ContextUsageRow::Slice(_) => CONTEXT_USAGE_SWATCH,
        ContextUsageRow::Free => CONTEXT_USAGE_FREE_SWATCH,
        ContextUsageRow::AutoCompact => CONTEXT_USAGE_RESERVED_SWATCH,
    }
}

fn row_color(row: ContextUsageRow) -> Color {
    match row {
        ContextUsageRow::Slice(category) => context_usage_color(category),
        ContextUsageRow::Free => context_usage_free_color(),
        ContextUsageRow::AutoCompact => context_usage_reserved_color(),
    }
}

/// The subtitle: `{model} · ≈{total} of {window} tokens ({share})` — the
/// share in the dim of the line until the window fills, amber past
/// [`CONTEXT_USAGE_WARN_SHARE`], red past the auto-compact threshold — or,
/// with no window known, `{model} · ≈{total} tokens · window unknown`.
fn subtitle(gauge: &Gauge<'_>, width: u16) -> Line<'static> {
    let dim = Style::new().fg(model_meta_color());
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    if let Some(model) = &gauge.model {
        spans.push(Span::styled(
            model.clone(),
            Style::new().fg(model_id_color()),
        ));
        spans.push(Span::styled(" · ", dim));
    }
    spans.extend(fill_spans(gauge));
    clamp_spans(spans, width as usize)
}

/// The header folded into one row for a short terminal: the title in its
/// dress, then the window's fill — how full the window is outlasts both the
/// bar and the model's name.
fn compact_header(gauge: &Gauge<'_>, width: u16) -> Line<'static> {
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    spans.extend(page_title_spans(CONTEXT_USAGE_TITLE));
    spans.push(Span::styled(" · ", Style::new().fg(model_meta_color())));
    spans.extend(fill_spans(gauge));
    clamp_spans(spans, width as usize)
}

/// `≈{total} of {window} tokens ({share})`, or `≈{total} tokens · window
/// unknown` — the subtitle's numbers, shared with [`compact_header`].
fn fill_spans(gauge: &Gauge<'_>) -> Vec<Span<'static>> {
    let dim = Style::new().fg(model_meta_color());
    let mut spans = Vec::new();
    spans.push(Span::styled(
        format!("{CONTEXT_USAGE_ESTIMATE_MARK}{}", token_text(gauge.total)),
        Style::new().fg(ai_color()).add_modifier(Modifier::BOLD),
    ));
    match gauge.window {
        Some(window) => {
            spans.push(Span::styled(
                format!(" of {} tokens (", token_text(window)),
                dim,
            ));
            spans.push(Span::styled(
                share_text(gauge.total, window),
                Style::new().fg(fill_color(gauge, window)),
            ));
            spans.push(Span::styled(")", dim));
        }
        None => spans.push(Span::styled(
            format!(" tokens · {CONTEXT_USAGE_WINDOW_UNKNOWN}"),
            dim,
        )),
    }
    spans
}

/// The subtitle share's colour for a `window`-token window.
fn fill_color(gauge: &Gauge<'_>, window: u64) -> Color {
    if window == 0 {
        return model_meta_color();
    }
    let fill = gauge.total as f64 / window as f64;
    let full = gauge
        .threshold
        .map_or(CONTEXT_USAGE_FULL_SHARE, |threshold| {
            threshold as f64 / window as f64
        });
    if fill >= full {
        context_usage_full_color()
    } else if fill >= CONTEXT_USAGE_WARN_SHARE {
        context_usage_warn_color()
    } else {
        model_meta_color()
    }
}

/// Split `cells` among `sizes` by **boundary**: each running total lands
/// within half a cell of where it belongs, so a position on the bar — the
/// auto-compact threshold — reads true. A non-zero size gets at least one
/// cell while there are cells enough for every one, taken from what
/// follows it, and a zero size gets none; the cells always sum to `cells`
/// (none when every size is zero).
pub(super) fn allot_cells(sizes: &[u64], cells: usize) -> Vec<usize> {
    let sum: u128 = sizes.iter().map(|&size| u128::from(size)).sum();
    if sum == 0 || cells == 0 {
        return vec![0; sizes.len()];
    }
    let mut nonzero_left = sizes.iter().filter(|&&size| size > 0).count();
    let every_one_visible = nonzero_left <= cells;
    let scale = cells as u128;
    let mut out = Vec::with_capacity(sizes.len());
    let mut running: u128 = 0;
    let mut previous = 0usize;
    for &size in sizes {
        if size == 0 {
            // Nothing to draw: a boundary held back a cell for the sizes
            // after it must not hand that cell to an empty one between.
            out.push(0);
            continue;
        }
        running += u128::from(size);
        nonzero_left -= 1;
        // round(running / sum × cells), in integers.
        let mut boundary =
            usize::try_from((running * scale * 2 + sum) / (sum * 2)).unwrap_or(cells);
        if every_one_visible {
            boundary = boundary.max(previous + 1).min(cells - nonzero_left);
        }
        let boundary = boundary.clamp(previous, cells);
        out.push(boundary - previous);
        previous = boundary;
    }
    out
}

/// The bar standing for the whole window, the page's width less its inset
/// on either side: each category's cells in its colour, in the order the
/// request carries them, then the free space (`░`) and the hatched headroom
/// (`╱`). With no window known it is the total's own proportions; with
/// nothing counted at all, empty.
fn bar_line(gauge: &Gauge<'_>, width: u16) -> Line<'static> {
    let cells = (width as usize)
        .saturating_sub(2 * cols(MODEL_INDENT))
        .max(1);
    let mut segments: Vec<(u64, &'static str, Color)> = gauge
        .page
        .breakdown
        .slices
        .iter()
        .map(|slice| {
            (
                u64::try_from(slice.tokens).unwrap_or(u64::MAX),
                CONTEXT_USAGE_BAR_USED,
                context_usage_color(slice.category),
            )
        })
        .collect();
    if gauge.window.is_some() {
        segments.push((
            gauge.free(),
            CONTEXT_USAGE_BAR_FREE,
            context_usage_free_color(),
        ));
        segments.push((
            gauge.reserved(),
            CONTEXT_USAGE_BAR_RESERVED,
            context_usage_reserved_color(),
        ));
    }
    let sizes: Vec<u64> = segments.iter().map(|(size, ..)| *size).collect();
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    if sizes.iter().all(|&size| size == 0) {
        spans.push(Span::styled(
            CONTEXT_USAGE_BAR_FREE.repeat(cells),
            Style::new().fg(context_usage_free_color()),
        ));
    } else {
        for ((_, glyph, color), count) in segments.iter().zip(allot_cells(&sizes, cells)) {
            if count > 0 {
                spans.push(Span::styled(glyph.repeat(count), Style::new().fg(*color)));
            }
        }
    }
    Line::from(spans)
}

/// The legend's rows as span runs, and the columns each takes: `{❯ }{swatch}
/// {label}  {tokens}  {share}`, the label padded and the numbers
/// right-aligned so every row's columns line up. The highlighted row lights
/// up whole in the accent, its label bold (the palette's whole-row rule);
/// the others keep the muted ink, their shares dim. A terminal too narrow
/// for the share column drops it.
fn legend(gauge: &Gauge<'_>, width: u16) -> (Vec<Vec<Span<'static>>>, usize) {
    let entries: Vec<(ContextUsageRow, String, String)> = gauge
        .rows
        .iter()
        .map(|&row| {
            let tokens = gauge.tokens(row);
            (row, token_text(tokens), share_text(tokens, gauge.whole()))
        })
        .collect();
    let label_w = entries
        .iter()
        .map(|(row, ..)| cols(row_label(*row)))
        .max()
        .unwrap_or(0);
    let tokens_w = entries.iter().map(|(_, t, _)| cols(t)).max().unwrap_or(0);
    let share_w = entries.iter().map(|(.., s)| cols(s)).max().unwrap_or(0);
    let gap = cols(CONTEXT_USAGE_COLUMN_GAP);
    let lead = cols(MODEL_INDENT) + cols(HOOKS_MARKER) + cols(CONTEXT_USAGE_SWATCH) + 1;
    let with_share = lead + label_w + gap + tokens_w + gap + share_w;
    let show_share = with_share <= width as usize;
    let legend_w = if show_share {
        with_share
    } else {
        lead + label_w + gap + tokens_w
    };
    let accent = Style::new().fg(model_selected_color());
    let rows = entries
        .iter()
        .map(|(row, tokens, share)| {
            let selected = *row == gauge.page.selected;
            let (marker, marker_style, label_style, tokens_style, share_style) = if selected {
                (
                    HOOKS_MARKER,
                    accent,
                    accent.add_modifier(Modifier::BOLD),
                    accent,
                    accent,
                )
            } else {
                (
                    "  ",
                    Style::default(),
                    Style::new().fg(model_id_color()),
                    Style::new().fg(model_id_color()),
                    Style::new().fg(model_meta_color()),
                )
            };
            let mut spans = vec![
                Span::raw(MODEL_INDENT),
                Span::styled(marker.to_string(), marker_style),
                Span::styled(
                    row_swatch(*row).to_string(),
                    Style::new().fg(row_color(*row)),
                ),
                Span::raw(" "),
                Span::styled(pad(row_label(*row), label_w), label_style),
                Span::raw(CONTEXT_USAGE_COLUMN_GAP),
                Span::styled(pad_left(tokens, tokens_w), tokens_style),
            ];
            if show_share {
                spans.push(Span::raw(CONTEXT_USAGE_COLUMN_GAP));
                spans.push(Span::styled(pad_left(share, share_w), share_style));
            }
            clip(spans, width as usize)
        })
        .collect();
    (rows, legend_w)
}

/// The highlighted row's detail as span runs, at most `max_rows` of them,
/// each within `room` columns: a header — the category in its colour and how
/// many things it holds — over its parts, largest first; or, for the two
/// rows of room left, what they mean.
fn detail(gauge: &Gauge<'_>, room: usize, max_rows: usize) -> Vec<Vec<Span<'static>>> {
    if max_rows == 0 || room == 0 {
        return Vec::new();
    }
    let dim = Style::new().fg(model_meta_color());
    let heading = Style::new()
        .fg(model_id_color())
        .add_modifier(Modifier::BOLD);
    let mut rows = Vec::new();
    match gauge.page.selected {
        ContextUsageRow::Slice(category) => {
            let color = context_usage_color(category);
            let slice = gauge.page.breakdown.slice(category);
            let items = slice.map_or(0, |slice| slice.items);
            let (one, many) = category.unit();
            rows.push(clip(
                vec![
                    Span::styled(
                        category.label().to_string(),
                        Style::new().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" · {items} {}", if items == 1 { one } else { many }),
                        dim,
                    ),
                ],
                room,
            ));
            match slice.filter(|slice| !slice.parts.is_empty()) {
                // A doc's path reads the way every file tool's header shows
                // one (`docs/tools.md` *Path display*); the count keeps the
                // path as the section names it.
                Some(slice) if category == Category::ProjectDocs => {
                    let shown = Slice {
                        parts: slice
                            .parts
                            .iter()
                            .map(|part| Part {
                                label: gauge.paths.display(&part.label),
                                tokens: part.tokens,
                            })
                            .collect(),
                        ..slice.clone()
                    };
                    rows.extend(part_rows(&shown, color, room, max_rows - 1));
                }
                Some(slice) => rows.extend(part_rows(slice, color, room, max_rows - 1)),
                None if category == Category::Messages => {
                    rows.extend(text_rows(CONTEXT_USAGE_NOTHING_YET, room, max_rows - 1));
                }
                None => {}
            }
        }
        ContextUsageRow::Free => {
            rows.push(clip(
                vec![Span::styled(CONTEXT_USAGE_FREE_LABEL, heading)],
                room,
            ));
            let threshold = gauge.threshold.map(token_text);
            let text = context_usage_free_text(
                &token_text(gauge.free()),
                threshold.as_deref(),
                gauge
                    .threshold
                    .is_some_and(|threshold| gauge.total >= threshold),
                !gauge.agent,
            );
            rows.extend(text_rows(&text, room, max_rows - 1));
        }
        ContextUsageRow::AutoCompact => {
            rows.push(clip(
                vec![Span::styled(CONTEXT_USAGE_RESERVED_LABEL, heading)],
                room,
            ));
            if let (Some(window), Some(threshold)) = (gauge.window, gauge.threshold) {
                let text = context_usage_headroom_text(
                    &token_text(window.saturating_sub(threshold)),
                    &token_text(threshold),
                );
                rows.extend(text_rows(&text, room, max_rows - 1));
            }
        }
    }
    rows
}

/// A slice's parts as rows, at most `max_rows`: `{label}  {tokens}  {bar}`,
/// the bar in the category's colour and measured against the largest part.
/// More parts than rows fold the tail into one dim `… N more` row carrying
/// their sum.
fn part_rows(slice: &Slice, color: Color, room: usize, max_rows: usize) -> Vec<Vec<Span<'static>>> {
    if max_rows == 0 {
        return Vec::new();
    }
    let fold = slice.parts.len() > max_rows;
    let shown = if fold {
        max_rows - 1
    } else {
        slice.parts.len()
    };
    let folded: usize = slice.parts[shown..].iter().map(|part| part.tokens).sum();
    let more = format!("… {} more", slice.parts.len() - shown);
    let fold_tokens = token_text(folded as u64);
    let tokens: Vec<String> = slice.parts[..shown]
        .iter()
        .map(|part| token_text(part.tokens as u64))
        .collect();
    let tokens_w = tokens
        .iter()
        .map(|t| cols(t))
        .chain(fold.then(|| cols(&fold_tokens)))
        .max()
        .unwrap_or(0);
    let labels_w = slice.parts[..shown]
        .iter()
        .map(|part| cols(&part.label))
        .chain(fold.then(|| cols(&more)))
        .max()
        .unwrap_or(0);
    let gap = cols(CONTEXT_USAGE_COLUMN_GAP);
    let label_w = labels_w.min(room.saturating_sub(gap + tokens_w)).max(1);
    let bar_room = room
        .saturating_sub(label_w + gap + tokens_w + gap)
        .min(CONTEXT_USAGE_PART_BAR_MAX);
    let largest = slice.parts.first().map_or(1, |part| part.tokens.max(1));
    let dim = Style::new().fg(model_meta_color());
    let mut rows: Vec<Vec<Span<'static>>> = slice.parts[..shown]
        .iter()
        .zip(&tokens)
        .map(|(part, tokens)| {
            let mut spans = vec![
                Span::styled(
                    pad(&ellipsize(&part.label, label_w), label_w),
                    Style::new().fg(model_id_color()),
                ),
                Span::raw(CONTEXT_USAGE_COLUMN_GAP),
                Span::styled(pad_left(tokens, tokens_w), Style::new().fg(ai_color())),
            ];
            if bar_room > 0 {
                let length = ((part.tokens as f64 / largest as f64) * bar_room as f64)
                    .round()
                    .clamp(1.0, bar_room as f64) as usize;
                spans.push(Span::raw(CONTEXT_USAGE_COLUMN_GAP));
                spans.push(Span::styled(
                    CONTEXT_USAGE_PART_BAR.repeat(length),
                    Style::new().fg(color),
                ));
            }
            clip(spans, room)
        })
        .collect();
    if fold {
        rows.push(clip(
            vec![
                Span::styled(pad(&ellipsize(&more, label_w), label_w), dim),
                Span::raw(CONTEXT_USAGE_COLUMN_GAP),
                Span::styled(pad_left(&fold_tokens, tokens_w), dim),
            ],
            room,
        ));
    }
    rows
}

/// `text` wrapped to `room` columns, dim, at most `max_rows` rows.
fn text_rows(text: &str, room: usize, max_rows: usize) -> Vec<Vec<Span<'static>>> {
    wrap_text(text, u16::try_from(room).unwrap_or(u16::MAX).max(1))
        .into_iter()
        .take(max_rows)
        .map(|row| vec![Span::styled(row, Style::new().fg(model_meta_color()))])
        .collect()
}

/// How much of the detail a page keeps.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DetailRoom {
    /// The whole detail: beside the legend, as tall as the legend or
    /// [`CONTEXT_USAGE_DETAIL_PARTS`] parts, whichever is more; under it,
    /// that many parts.
    Whole,
    /// Under the legend, [`CONTEXT_USAGE_DETAIL_COMPACT_PARTS`] parts; beside
    /// it, no taller than the legend.
    Compact,
    /// Only what sits beside the legend at no cost in rows; nothing under it.
    Beside,
}

/// The legend and its detail as page blocks: one block with the detail
/// beside the legend on a terminal wide enough for both, else the legend
/// and — room permitting — the detail under it, a block of its own.
fn body(gauge: &Gauge<'_>, width: u16, room: DetailRoom) -> Vec<Vec<Line<'static>>> {
    let (legend, legend_w) = legend(gauge, width);
    let w = width as usize;
    let panel_gap = cols(CONTEXT_USAGE_PANEL_GAP);
    let beside = width >= CONTEXT_USAGE_SIDE_BY_SIDE_MIN_COLS
        && legend_w + panel_gap + CONTEXT_USAGE_DETAIL_MIN_COLS <= w;
    if beside {
        let detail_rows = match room {
            DetailRoom::Whole => legend.len().max(CONTEXT_USAGE_DETAIL_PARTS + 1),
            DetailRoom::Compact | DetailRoom::Beside => legend.len(),
        };
        let detail = detail(gauge, w - legend_w - panel_gap, detail_rows);
        let lines = (0..legend.len().max(detail.len()))
            .map(|i| {
                let mut spans = legend.get(i).cloned().unwrap_or_default();
                if let Some(row) = detail.get(i) {
                    let used: usize = spans.iter().map(|span| cols(&span.content)).sum();
                    spans.push(Span::raw(
                        " ".repeat(legend_w.saturating_sub(used) + panel_gap),
                    ));
                    spans.extend(row.iter().cloned());
                }
                Line::from(spans)
            })
            .collect();
        return vec![lines];
    }
    let legend_block: Vec<Line<'static>> = legend.into_iter().map(Line::from).collect();
    let parts = match room {
        DetailRoom::Whole => CONTEXT_USAGE_DETAIL_PARTS,
        DetailRoom::Compact => CONTEXT_USAGE_DETAIL_COMPACT_PARTS,
        DetailRoom::Beside => return vec![legend_block],
    };
    // Under the legend the detail lines up with the swatches.
    let indent = format!("{MODEL_INDENT}{}", " ".repeat(cols(HOOKS_MARKER)));
    let detail_block = detail(gauge, w.saturating_sub(cols(&indent)), parts + 1)
        .into_iter()
        .map(|row| {
            let mut spans = vec![Span::raw(indent.clone())];
            spans.extend(row);
            Line::from(spans)
        })
        .collect();
    vec![legend_block, detail_block]
}

/// How much of the usage section a page keeps.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UsageRoom {
    /// The last request, its cache meter, and the ledger.
    Whole,
    /// The last request and its cache meter.
    Reduced,
    /// The last request alone, its cache share riding the same row.
    Minimal,
}

/// What the **provider** reported, under their labels: the last request's
/// input and output, its cache hit as a meter in the success green beside
/// the read and written counts, and the ledger — every request since launch,
/// or the viewed agent's own. Before any frame, one dim row says so.
fn usage(gauge: &Gauge<'_>, width: u16, room: UsageRoom) -> Vec<Line<'static>> {
    let label_w = [
        CONTEXT_USAGE_LAST_REQUEST,
        CONTEXT_USAGE_CACHE_HIT,
        gauge.ledger_label,
    ]
    .iter()
    .map(|label| cols(label))
    .max()
    .unwrap_or(0);
    let value_room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + label_w + cols(CONTEXT_USAGE_LABEL_GAP));
    let label = |text: &str| -> Vec<Span<'static>> {
        vec![
            Span::raw(MODEL_INDENT),
            Span::styled(pad(text, label_w), Style::new().fg(model_id_color())),
            Span::raw(CONTEXT_USAGE_LABEL_GAP),
        ]
    };
    let dim = Style::new().fg(model_meta_color());
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    match gauge.last {
        None => {
            let mut spans = label(CONTEXT_USAGE_LAST_REQUEST);
            spans.push(Span::styled(CONTEXT_USAGE_NO_USAGE, dim));
            rows.push(spans);
        }
        Some(last) => {
            let mut spans = label(CONTEXT_USAGE_LAST_REQUEST);
            spans.extend(in_out(last.input, last.output));
            let share = cache_share(&last);
            if room == UsageRoom::Minimal
                && let Some(share) = share
            {
                spans.push(Span::styled(" · ", dim));
                spans.push(Span::styled(
                    format!("{:.0}%", share * 100.0),
                    Style::new().fg(ai_color()),
                ));
                spans.push(Span::styled(" cached", dim));
            }
            rows.push(spans);
            if room != UsageRoom::Minimal
                && let Some(share) = share
            {
                let mut spans = label(CONTEXT_USAGE_CACHE_HIT);
                spans.extend(meter(share, &last, value_room));
                rows.push(spans);
            }
        }
    }
    if room == UsageRoom::Whole && gauge.ledger.requests > 0 {
        // The cache clause is left out whole rather than cut mid-word when
        // the row cannot hold it.
        let fits = |spans: &[Span<'static>]| {
            spans.iter().map(|span| cols(&span.content)).sum::<usize>() <= width as usize
        };
        let mut spans = label(gauge.ledger_label);
        spans.extend(ledger_values(&gauge.ledger, true));
        if !fits(&spans) {
            spans = label(gauge.ledger_label);
            spans.extend(ledger_values(&gauge.ledger, false));
        }
        rows.push(spans);
    }
    rows.into_iter()
        .map(|spans| clamp_spans(spans, width as usize))
        .collect()
}

/// `{input} in · {output} out` — the counts bright, the words dim.
fn in_out(input: u64, output: u64) -> Vec<Span<'static>> {
    let bright = Style::new().fg(ai_color());
    let dim = Style::new().fg(model_meta_color());
    vec![
        Span::styled(token_text(input), bright),
        Span::styled(" in · ", dim),
        Span::styled(token_text(output), bright),
        Span::styled(" out", dim),
    ]
}

/// The cache meter: `{fill}{empty}  {share}%  {read} read · {written}
/// written`, sized to the `room` it has — the trailer dropped first, the
/// meter never shorter than [`CONTEXT_USAGE_METER_MIN`] while it fits.
fn meter(share: f64, last: &TokenUsage, room: usize) -> Vec<Span<'static>> {
    let percent = format!("{:.0}%", share * 100.0);
    let mut halves = Vec::new();
    if last.cached > 0 {
        halves.push(format!("{} read", token_text(last.cached)));
    }
    if last.cache_write > 0 {
        halves.push(format!("{} written", token_text(last.cache_write)));
    }
    let trailer = if halves.is_empty() {
        "no cache reads".to_string()
    } else {
        halves.join(" · ")
    };
    let fixed = 2 + cols(&percent);
    let with_trailer = fixed + 2 + cols(&trailer);
    let (length, show_trailer) = if room >= CONTEXT_USAGE_METER_MIN + with_trailer {
        ((room - with_trailer).min(CONTEXT_USAGE_METER_MAX), true)
    } else {
        (
            room.saturating_sub(fixed).clamp(1, CONTEXT_USAGE_METER_MAX),
            false,
        )
    };
    let filled = ((share * length as f64).round() as usize).min(length);
    let mut spans = vec![
        Span::styled(
            CONTEXT_USAGE_METER_FILL.repeat(filled),
            Style::new().fg(context_usage_meter_color()),
        ),
        Span::styled(
            CONTEXT_USAGE_METER_EMPTY.repeat(length - filled),
            Style::new().fg(context_usage_free_color()),
        ),
        Span::raw("  "),
        Span::styled(percent, Style::new().fg(ai_color())),
    ];
    if show_trailer {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(trailer, Style::new().fg(model_meta_color())));
    }
    spans
}

/// The ledger's row: `{n} requests · {input} in · {output} out · {share}%
/// cached` — the cache clause only when `with_cache` asks for it.
fn ledger_values(ledger: &UsageLedger, with_cache: bool) -> Vec<Span<'static>> {
    let dim = Style::new().fg(model_meta_color());
    let bright = Style::new().fg(ai_color());
    let mut spans = vec![
        Span::styled(ledger.requests.to_string(), bright),
        Span::styled(
            if ledger.requests == 1 {
                " request · "
            } else {
                " requests · "
            },
            dim,
        ),
    ];
    spans.extend(in_out(ledger.input, ledger.output));
    if with_cache && let Some(share) = ledger.cache_share() {
        spans.push(Span::styled(" · ", dim));
        spans.push(Span::styled(format!("{:.0}%", share * 100.0), bright));
        spans.push(Span::styled(" cached", dim));
    }
    spans
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
/// sheds rows before it flows*), yielding in this order: the detail's parts
/// (a detail under the legend first shrinks, then goes), the ledger row, the
/// cache meter (its share riding the last request's row instead), the bar
/// and the model's name (the title and the window's fill folding into one
/// row), that row, and the usage row — the legend and the hint stay on
/// every page. What [`render_context_usage`] paints and
/// `context_usage_menu_rows` counts, so the reserved height and the painted
/// rows can never disagree. Empty when the page is closed.
#[must_use]
pub fn context_usage_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    let Some(gauge) = Gauge::of(app) else {
        return Vec::new();
    };
    let header = || {
        vec![
            page_title_line(CONTEXT_USAGE_TITLE, width),
            subtitle(&gauge, width),
        ]
    };
    let bar = || vec![bar_line(&gauge, width)];
    let hint = || {
        vec![model_placeholder_row(
            CONTEXT_USAGE_HINT,
            model_meta_color(),
            width,
        )]
    };
    let page = |top: Vec<Vec<Line<'static>>>, room: DetailRoom, tail: Vec<Vec<Line<'static>>>| {
        framed([top, body(&gauge, width, room), tail].concat(), width)
    };
    let usage = |room| usage(&gauge, width, room);
    first_page_that_fits(
        height,
        &[
            &|| {
                page(
                    vec![header(), bar()],
                    DetailRoom::Whole,
                    vec![usage(UsageRoom::Whole), hint()],
                )
            },
            &|| {
                page(
                    vec![header(), bar()],
                    DetailRoom::Compact,
                    vec![usage(UsageRoom::Whole), hint()],
                )
            },
            &|| {
                page(
                    vec![header(), bar()],
                    DetailRoom::Beside,
                    vec![usage(UsageRoom::Whole), hint()],
                )
            },
            &|| {
                page(
                    vec![header(), bar()],
                    DetailRoom::Beside,
                    vec![usage(UsageRoom::Reduced), hint()],
                )
            },
            &|| {
                page(
                    vec![header(), bar()],
                    DetailRoom::Beside,
                    vec![usage(UsageRoom::Minimal), hint()],
                )
            },
            &|| {
                page(
                    vec![vec![compact_header(&gauge, width)]],
                    DetailRoom::Beside,
                    vec![usage(UsageRoom::Minimal), hint()],
                )
            },
            &|| {
                page(
                    Vec::new(),
                    DetailRoom::Beside,
                    vec![usage(UsageRoom::Minimal), hint()],
                )
            },
            &|| page(Vec::new(), DetailRoom::Beside, vec![hint()]),
        ],
    )
}

/// The rows the page's own frame occupies — the built page's line count
/// ([`context_usage_lines`]). What [`context_usage_height`] reserves under
/// the strip, and what `render_live` hands [`render_context_usage`].
pub(super) fn context_usage_menu_rows(app: &App, width: u16, height: u16) -> u16 {
    u16::try_from(context_usage_lines(app, width, height).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/context` page is open, or
/// `None` when it isn't (the caller then falls back to `live_height`). Like
/// every sibling page it **replaces** the composer — and only the composer:
/// the streaming strip keeps its rows above it, so opening `/context`
/// mid-turn never hides the running turn. Clamped to the terminal height.
#[must_use]
pub fn context_usage_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.context_usage.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        context_usage_menu_rows(app, width, term_height),
        term_height,
    ))
}

/// Render the **inline** `/context` page into the live region, in place of
/// the composer — bottom-anchored, so a squeezed area keeps the legend, the
/// hint and the closing rule on screen while the skipped top flows into
/// scrollback (`docs/view-flow.md`). Pure — `render_live` paints this. See
/// `docs/context-usage.md`.
pub fn render_context_usage(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        context_usage_lines(app, area.width, area.height),
    );
}
