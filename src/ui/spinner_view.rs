//! The inline `/spinner` picker. See `docs/spinner.md`.
//!
//! The `/mascot` picker's frame — same rules, same `❯` search line, same
//! `→` marker and cyan selection, same `(n/total)` counter and dim hint —
//! over one row per spinner style, and the page is **live**: every row
//! carries its own spinner, turning at the shared frame clock, and the
//! highlighted style previews as a whole sample status line. Both are built
//! by the status line's own renderer
//! ([`styled_status_line`](super::status::styled_status_line) and the
//! `spinner_spans` it opens with), so what the picker shows and what a turn
//! shows can never disagree. The page height never depends on the clock or
//! the selection — the comet is eight cells wide and the rest one, but every
//! style is exactly one row — so the frame holds still while it animates.

use super::model_view::{
    compact_page, first_page_that_fits, list_window, model_placeholder_row, model_rule,
    placeholder_window,
};
use super::status::{spinner_spans, styled_status_line};
use super::theme::*;
use super::wrap::cols;
use super::*;

use crate::app::{Spinner, SpinnerRow, StatusVerb};

/// The sample the preview renders: a just-submitted **first** turn's status
/// — the first status verb, no tokens yet — at `elapsed` since the picker
/// opened, so the line reads `0s` when the page appears and counts up while
/// the user browses, moving on to the next verb every
/// [`VERB_ROTATION`](crate::app::VERB_ROTATION) as a real turn's line does.
/// The comet's sweep and the verb's shimmer take their phase from the same
/// value, exactly as a real turn's do.
fn sample_status(elapsed: Duration) -> TurnStatus {
    let verb = StatusVerb::at(StatusVerb::rotated(0, elapsed));
    TurnStatus {
        verb: verb.working,
        done_verb: verb.done,
        rotates_from: Some(0),
        tokens: 0,
        arrow: TokenArrow::Down,
        elapsed,
        thinking: None,
        shell: false,
        retry: None,
        offline: None,
        tip: None,
    }
}

/// One style row: `{marker}{name}{pad}{spinner} {✓}` — the selected row's
/// marker and name light up cyan (the picker family's accent), the name
/// column is sized to the widest visible name plus [`SPINNER_MENU_GAP`], the
/// row's own spinner turns at `elapsed`, and the session's current style
/// carries the `/model` picker's green ✓.
fn spinner_row(
    row: &SpinnerRow,
    selected: bool,
    name_col: usize,
    elapsed: Duration,
) -> Line<'static> {
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
    let name = row.spinner.name();
    let mut spans = vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(name.to_string(), name_style),
        Span::raw(" ".repeat(name_col.saturating_sub(cols(name)))),
    ];
    // The spinner's own spans end in the separator space the status line
    // puts before its verb, so the ✓ sits one column after the glyphs.
    spans.extend(spinner_spans(row.spinner, elapsed));
    if row.active {
        spans.push(Span::styled(
            MODEL_ACTIVE_MARK.trim_start().to_string(),
            Style::new().fg(model_active_color()),
        ));
    }
    Line::from(spans)
}

/// The window of `rows` that keeps the selection in view — `max` rows,
/// [`list_window`] riding the selection on the middle one — as how many rows
/// it hides above, its rows (the selected one marked, each wearing its own
/// live spinner) and how many it hides below. The glyph column is sized off
/// **every** listed style, so it holds still as ↑/↓ scroll the window.
/// `rows` is never empty here.
fn spinner_window(
    rows: &[SpinnerRow],
    selected: usize,
    max: usize,
    elapsed: Duration,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = list_window(rows.len(), selected, max);
    let name_col = rows
        .iter()
        .map(|row| cols(row.spinner.name()))
        .max()
        .unwrap_or(0)
        + SPINNER_MENU_GAP;
    let lines = rows[range.clone()]
        .iter()
        .zip(range)
        .map(|(row, i)| spinner_row(row, i == selected, name_col, elapsed))
        .collect();
    (above, lines, below)
}

/// The `(selected+1/total)` counter under the whole page's list.
fn spinner_counter_line(selected: usize, total: usize) -> Line<'static> {
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("({}/{total})", selected + 1),
            Style::new().fg(model_meta_color()),
        ),
    ])
}

/// The live preview for `spinner`, inset two columns inside the frame: the
/// sample status line at `elapsed`, through the status line's own renderer
/// at the inset width (it clamps itself with a dim `…` on a narrow
/// terminal, exactly as the real one does).
fn preview_line(spinner: Spinner, elapsed: Duration, width: u16) -> Line<'static> {
    let inner = width.saturating_sub(cols(MODEL_INDENT) as u16);
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    spans.extend(styled_status_line(&sample_status(elapsed), None, spinner, inner).spans);
    Line::from(spans)
}

/// The framed page as lines — a top rule, the `❯` search line, a body, the
/// key hint, and a bottom rule, blank-gapped like its family between its
/// blocks and never against a rule (`docs/view-flow.md` *The frame hugs its
/// content*) — the first of
/// two pages that fits `height` rows ([`first_page_that_fits`]): the
/// **whole** page, whose body is the style rows (each wearing its own live
/// spinner), the `(n/total)` counter, the **live status-line preview** and
/// the highlighted style's description, or, on a terminal too short for
/// that, a **compact** page ([`compact_page`]): three of the rows between
/// the two reserved overflow slots — the preview, the description and the
/// counter yield — and on a terminal shorter still the slots, the rules and
/// the rows one by one, so the search line, the highlighted style
/// and the hint stay on screen (`docs/view-flow.md` *A page that sheds rows
/// before it flows*). What [`render_spinner_picker`] paints and
/// [`spinner_menu_rows`] counts, so the reserved height and the painted rows
/// can never disagree. `height` is the terminal's, or the region already
/// reserved for the page, which builds the same page. Empty when the picker
/// is closed.
///
/// A search that matched **nothing** has no count, no preview and no
/// description, so its page collapses to the placeholder between single
/// gaps — the `/mascot` picker's rule — and sheds the same way, keeping what
/// was typed.
pub(super) fn spinner_view_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    let Some(picker) = app.spinner_picker.as_ref() else {
        return Vec::new();
    };
    let rows = app.spinner_rows();
    let elapsed = app.spinner_preview_elapsed();
    let search_line = Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(model_selected_color())),
        Span::raw(picker.query.clone()),
    ]);
    let hint = model_placeholder_row(SPINNER_HINT, model_meta_color(), width);
    let page = |body: Vec<Line<'static>>| {
        let mut lines = vec![model_rule(width), search_line.clone()];
        lines.extend(body);
        lines.extend([hint.clone(), model_rule(width)]);
        lines
    };
    if rows.is_empty() {
        let block = [model_placeholder_row(
            SPINNER_NO_MATCH,
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
    let spinner = rows[selected].spinner;
    // The whole page: the counter, the live preview and the description sit
    // between the list and the hint. Every style is one row, so the frame
    // holds still as ↑/↓ move and the clock ticks.
    let whole = || {
        let (_, window, _) =
            spinner_window(&rows, selected, usize::from(SPINNER_MENU_MAX_ROWS), elapsed);
        let mut body = vec![Line::default()];
        body.extend(window);
        body.push(spinner_counter_line(selected, rows.len()));
        body.push(Line::default());
        body.push(preview_line(spinner, elapsed, width));
        body.push(Line::default());
        body.push(model_placeholder_row(
            spinner.description(),
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
            &|max| spinner_window(&rows, selected, max, elapsed),
            std::slice::from_ref(&hint),
            width,
        )
    };
    first_page_that_fits(height, &[&whole, &compact])
}

/// The rows the picker's own frame occupies — the built page's line count
/// ([`spinner_view_lines`]). What [`spinner_picker_height`] reserves under
/// the strip, and what [`render_live`] hands [`render_spinner_picker`].
pub(super) fn spinner_menu_rows(app: &App, width: u16, height: u16) -> u16 {
    u16::try_from(spinner_view_lines(app, width, height).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/spinner` picker is open, or
/// `None` when it isn't (the caller then falls back to [`live_height`]). Like
/// every sibling picker it **replaces** the composer — and only the composer:
/// the streaming strip keeps its rows above it, so opening `/spinner`
/// mid-turn never hides the running turn (whose status line is, after all,
/// what the picker is about). Clamped to the terminal height.
#[must_use]
pub fn spinner_picker_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.spinner_picker.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        spinner_menu_rows(app, width, term_height),
        term_height,
    ))
}

/// Render the **inline** `/spinner` picker into the live region, in place of
/// the composer: a top rule, the `❯` search line, the style rows with their
/// live spinners (`→ gravity  ⣤⣀⣀⣀⣀⣀⣀⣀ ✓`), a `(n/total)` counter, the live
/// status-line preview, the highlighted style's description, the key hint,
/// and a bottom rule; or, in an area too short for that, the compact page,
/// which sheds its own rows down to the search line in an area shorter
/// still. The page is built for the area's height, so it always fits and
/// nothing flows (`docs/view-flow.md`). Pure — `render_live` paints this.
/// See `docs/spinner.md`.
pub fn render_spinner_picker(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        spinner_view_lines(app, area.width, area.height),
    );
}
