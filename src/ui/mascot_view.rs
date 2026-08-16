//! The inline `/mascot` picker. See `docs/mascot.md`.
//!
//! The `/settings` menu's frame — same rules, same `❯` search line, same
//! `→` marker and cyan selection, same `(n/total)` counter and dim hint —
//! over one row per mascot, with one thing no sibling has: a **live banner
//! preview** in the page's centre. The preview is built by
//! [`header_lines_for`](super::header::header_lines_for), the *same* builder
//! the startup banner uses, so what the picker shows and what Enter produces
//! can never disagree. The slot is padded to the tallest mascot so the frame
//! never jumps as ↑/↓ move across mascots of different heights.

use super::header::header_lines_for;
use super::model_view::{model_placeholder_row, model_rule};
use super::theme::*;
use super::wrap::cols;
use super::*;

use crate::app::{Mascot, MascotRow};

/// One mascot row: `{marker}{name}{✓}` — the selected row's marker and name
/// light up cyan (the picker family's accent), the session's current mascot
/// carries the `/model` picker's green ✓.
fn mascot_row(row: &MascotRow, selected: bool) -> Line<'static> {
    let marker = if selected { MODEL_MARKER } else { "  " };
    let (marker_style, name_style) = if selected {
        (
            Style::new().fg(MODEL_SELECTED_COLOR),
            Style::new()
                .fg(MODEL_SELECTED_COLOR)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (Style::default(), Style::new().fg(MODEL_ID_COLOR))
    };
    let active_mark = if row.active { MODEL_ACTIVE_MARK } else { "" };
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(row.mascot.name().to_string(), name_style),
        Span::styled(active_mark.to_string(), Style::new().fg(MODEL_ACTIVE_COLOR)),
    ])
}

/// The picker's list lines: the rows windowed ([`centered_window`]) at
/// [`MASCOT_MENU_MAX_ROWS`] (the eight-mascot catalog never actually windows
/// today), or a single placeholder when the search matches nothing.
fn mascot_list_lines(rows: &[MascotRow], selected: usize, width: u16) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return vec![model_placeholder_row(
            MASCOT_NO_MATCH,
            MODEL_META_COLOR,
            width,
        )];
    }
    let max = MASCOT_MENU_MAX_ROWS as usize;
    let selected = selected.min(rows.len() - 1);
    let offset = centered_window(rows.len(), selected, max);
    rows.iter()
        .enumerate()
        .skip(offset)
        .take(max)
        .map(|(i, row)| mascot_row(row, i == selected))
        .collect()
}

/// The `(selected+1/total)` counter under the list, or a blank line when the
/// search matched nothing.
fn mascot_counter_line(rows: &[MascotRow], selected: usize) -> Line<'static> {
    if rows.is_empty() {
        return Line::default();
    }
    let selected = selected.min(rows.len() - 1);
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("({}/{})", selected + 1, rows.len()),
            Style::new().fg(MODEL_META_COLOR),
        ),
    ])
}

/// The live banner preview for `mascot`, inset two columns inside the frame —
/// the startup banner's own builder at the inset width, so the preview *is*
/// the banner. Padded with blank lines to `slot` rows.
fn preview_lines(app: &App, mascot: Mascot, slot: usize, width: u16) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(cols(MODEL_INDENT) as u16);
    let mut lines: Vec<Line<'static>> = header_lines_for(app, mascot, inner)
        .into_iter()
        .map(|line| {
            let mut spans = vec![Span::raw(MODEL_INDENT)];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect();
    lines.truncate(slot);
    while lines.len() < slot {
        lines.push(Line::default());
    }
    lines
}

/// The preview slot's fixed height at this width: the tallest banner any
/// mascot in the catalog would draw, so moving the selection never changes
/// the page height (the `/settings` always-emit rule).
fn preview_slot(app: &App, width: u16) -> usize {
    let inner = width.saturating_sub(cols(MODEL_INDENT) as u16);
    Mascot::ALL
        .iter()
        .map(|&m| header_lines_for(app, m, inner).len())
        .max()
        .unwrap_or(0)
}

/// The whole framed page as lines: a top rule, the `❯` search line, the
/// mascot rows, the `(n/total)` counter, the **live banner preview**, the
/// highlighted mascot's description, the key hint, and a bottom rule —
/// blank-gapped like its family. What [`render_mascot_picker`] paints
/// (bottom-anchored) and [`mascot_menu_rows`] counts, so the reserved height
/// and the painted rows can never disagree (`docs/view-flow.md`). Empty when
/// the picker is closed.
pub(super) fn mascot_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(picker) = app.mascot_picker.as_ref() else {
        return Vec::new();
    };
    let rows = app.mascot_rows();
    let selected = picker.selected;
    let search_line = Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(MODEL_SELECTED_COLOR)),
        Span::raw(picker.query.clone()),
    ]);
    let highlighted = rows
        .get(selected.min(rows.len().saturating_sub(1)))
        .map(|row| row.mascot);
    // The preview slot: the highlighted mascot's banner, or blank rows when
    // the search matched nothing — always the same height, so the frame
    // holds still while the user types and moves.
    let slot = preview_slot(app, width);
    let preview = match highlighted {
        Some(mascot) => preview_lines(app, mascot, slot, width),
        None => vec![Line::default(); slot],
    };
    let description_line = highlighted.map_or_else(Line::default, |mascot| {
        model_placeholder_row(mascot.description(), MODEL_META_COLOR, width)
    });
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        search_line,
        Line::default(),
    ];
    lines.extend(mascot_list_lines(&rows, selected, width));
    lines.push(mascot_counter_line(&rows, selected));
    lines.push(Line::default());
    lines.extend(preview);
    lines.push(Line::default());
    lines.push(description_line);
    lines.push(Line::default());
    lines.push(model_placeholder_row(MASCOT_HINT, MODEL_META_COLOR, width));
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// The rows the picker's own frame occupies — the built page's line count
/// ([`mascot_view_lines`]). What [`mascot_picker_height`] reserves under the
/// strip, and what [`render_live`] hands [`render_mascot_picker`].
pub(super) fn mascot_menu_rows(app: &App, width: u16) -> u16 {
    u16::try_from(mascot_view_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/mascot` picker is open, or `None`
/// when it isn't (the caller then falls back to [`live_height`]). Like every
/// sibling picker it **replaces** the composer — and only the composer: the
/// streaming strip keeps its rows above it, so opening `/mascot` mid-turn
/// never hides the running turn. Clamped to the terminal height.
#[must_use]
pub fn mascot_picker_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.mascot_picker.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        mascot_menu_rows(app, width),
        term_height,
    ))
}

/// Render the **inline** `/mascot` picker into the live region, in place of
/// the composer: a top rule, the `❯` search line, the mascot rows
/// (`→ {name} ✓`), a `(n/total)` counter, the live banner preview, the
/// highlighted mascot's description, the key hint, and a bottom rule —
/// bottom-anchored, so a squeezed area keeps the preview, the hint and the
/// closing rule on screen while the skipped top flows into scrollback
/// (`docs/view-flow.md`). Pure — `render_live` paints this. See
/// `docs/mascot.md`.
pub fn render_mascot_picker(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, mascot_view_lines(app, area.width));
}
