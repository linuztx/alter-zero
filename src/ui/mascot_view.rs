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
use super::model_view::{
    compact_page, first_page_that_fits, list_window, model_placeholder_row, model_rule,
    placeholder_window,
};
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
            Style::new().fg(model_selected_color()),
            Style::new()
                .fg(model_selected_color())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (Style::default(), Style::new().fg(model_id_color()))
    };
    let active_mark = if row.active { MODEL_ACTIVE_MARK } else { "" };
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(row.mascot.name().to_string(), name_style),
        Span::styled(
            active_mark.to_string(),
            Style::new().fg(model_active_color()),
        ),
    ])
}

/// The window of `rows` that keeps the selection in view — `max` rows,
/// [`list_window`] riding the selection on the middle one — as how many rows
/// it hides above, its rows (the selected one marked) and how many it hides
/// below. `rows` is never empty here.
fn mascot_window(
    rows: &[MascotRow],
    selected: usize,
    max: usize,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = list_window(rows.len(), selected, max);
    let lines = rows[range.clone()]
        .iter()
        .zip(range)
        .map(|(row, i)| mascot_row(row, i == selected))
        .collect();
    (above, lines, below)
}

/// The `(selected+1/total)` counter under the whole page's list.
fn mascot_counter_line(selected: usize, total: usize) -> Line<'static> {
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("({}/{total})", selected + 1),
            Style::new().fg(model_meta_color()),
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

/// The framed page as lines — a top rule, the `❯` search line, a body, the
/// key hint, and a bottom rule, blank-gapped like its family between its
/// blocks and never against a rule (`docs/view-flow.md` *The frame hugs its
/// content*) — the first of
/// two pages that fits `height` rows ([`first_page_that_fits`]): the
/// **whole** page, whose body is the mascot rows, the `(n/total)` counter,
/// the **live banner preview** and the highlighted mascot's description, or,
/// on a terminal too short for that, a **compact** page ([`compact_page`]):
/// three of the rows between the two reserved overflow slots — the preview,
/// the description and the counter yield — and on a terminal shorter still
/// the slots, the rules and the rows one by one, so the search
/// line, the highlighted mascot and the hint stay on screen
/// (`docs/view-flow.md` *A page that sheds rows before it flows*). What
/// [`render_mascot_picker`] paints and [`mascot_menu_rows`] counts, so the
/// reserved height and the painted rows can never disagree. `height` is the
/// terminal's, or the region already reserved for the page, which builds the
/// same page. Empty when the picker is closed.
///
/// A search that matched **nothing** has no count, no banner to preview and
/// no description, so its page collapses to the placeholder between single
/// gaps — the `/model` picker's placeholder rule
/// ([`model_has_detail`](super::layout::model_has_detail)) — and sheds the
/// same way, keeping what was typed.
pub(super) fn mascot_view_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    let Some(picker) = app.mascot_picker.as_ref() else {
        return Vec::new();
    };
    let rows = app.mascot_rows();
    let search_line = Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(model_selected_color())),
        Span::raw(picker.query.clone()),
    ]);
    let hint = model_placeholder_row(MASCOT_HINT, model_meta_color(), width);
    let page = |body: Vec<Line<'static>>| {
        let mut lines = vec![model_rule(width), search_line.clone()];
        lines.extend(body);
        lines.extend([hint.clone(), model_rule(width)]);
        lines
    };
    if rows.is_empty() {
        let block = [model_placeholder_row(
            MASCOT_NO_MATCH,
            model_meta_color(),
            width,
        )];
        return compact_page(
            height,
            &search_line,
            &|max| placeholder_window(&block, max),
            &[hint],
            width,
        );
    }
    let selected = picker.selected.min(rows.len() - 1);
    let mascot = rows[selected].mascot;
    // The whole page: the counter, the live banner preview and the
    // description sit between the list and the hint. The preview slot is
    // padded to the tallest banner any mascot would draw, so the frame holds
    // still as ↑/↓ move.
    let whole = || {
        let (_, window, _) = mascot_window(&rows, selected, usize::from(MASCOT_MENU_MAX_ROWS));
        let mut body = vec![Line::default()];
        body.extend(window);
        body.push(mascot_counter_line(selected, rows.len()));
        body.push(Line::default());
        body.extend(preview_lines(app, mascot, preview_slot(app, width), width));
        body.push(Line::default());
        body.push(model_placeholder_row(
            mascot.description(),
            model_meta_color(),
            width,
        ));
        body.push(Line::default());
        page(body)
    };
    let compact = || {
        compact_page(
            height,
            &search_line,
            &|max| mascot_window(&rows, selected, max),
            std::slice::from_ref(&hint),
            width,
        )
    };
    first_page_that_fits(height, &[&whole, &compact])
}

/// The rows the picker's own frame occupies — the built page's line count
/// ([`mascot_view_lines`]). What [`mascot_picker_height`] reserves under the
/// strip, and what [`render_live`] hands [`render_mascot_picker`].
pub(super) fn mascot_menu_rows(app: &App, width: u16, height: u16) -> u16 {
    u16::try_from(mascot_view_lines(app, width, height).len()).unwrap_or(u16::MAX)
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
        mascot_menu_rows(app, width, term_height),
        term_height,
    ))
}

/// Render the **inline** `/mascot` picker into the live region, in place of
/// the composer: a top rule, the `❯` search line, the mascot rows
/// (`→ {name} ✓`), a `(n/total)` counter, the live banner preview, the
/// highlighted mascot's description, the key hint, and a bottom rule; or,
/// in an area too short for that, the compact page, which sheds its own rows
/// down to the search line in an area shorter still. The page is built for
/// the area's height, so it always fits and nothing flows
/// (`docs/view-flow.md`). Pure — `render_live` paints this. See
/// `docs/mascot.md`.
pub fn render_mascot_picker(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        mascot_view_lines(app, area.width, area.height),
    );
}
