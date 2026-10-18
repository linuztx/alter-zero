//! The inline `/mascot` picker. See `docs/mascot.md`.
//!
//! The `/theme` picker's layout over the mascot catalog: the family's frame
//! and `❯` search line over one row per mascot in the **`/mcp` list's
//! shape** — the accent `❯` on the selection, the other rows indented under
//! it, dim `↑ N more above` / `↓ N more below` rows naming whatever a
//! window hides — and a **live banner preview** of the highlighted mascot
//! under a caption naming it. The preview is built by
//! [`header_lines_for`](super::header::header_lines_for), the *same* builder
//! the startup banner uses, so what the picker shows and what Enter produces
//! can never disagree. Once the terminal has room for the list, a gap and
//! the banner, the preview sits **beside** the list; a narrower terminal
//! stacks it under the list ([`PreviewLayout`]). The banner slot is padded
//! to the tallest mascot, and the list column is sized off the whole
//! catalog, so the frame holds still as ↑/↓ and a search move the list.

use super::header::header_lines_for;
use super::model_view::{
    PreviewLayout, capitalized, caption_lines, compact_page, first_page_that_fits, list_window,
    marked_row_head, model_placeholder_row, model_rule, overflow_list, placeholder_window,
};
use super::theme::*;
use super::wrap::{clamp_spans, spans_cols};
use super::*;

use crate::app::{Mascot, MascotRow};

/// One mascot row in the `/mcp` list's shape: `{indent}{❯ }{name}{ ✓}`
/// ([`marked_row_head`]) — the selected row's marker and name in the
/// accent, every other row keeping the marker's width blank, and the
/// session's current mascot wearing the `/model` picker's green ✓.
fn mascot_row(row: &MascotRow, selected: bool) -> Line<'static> {
    let mut spans = marked_row_head(row.mascot.name(), selected);
    if row.active {
        spans.push(Span::styled(
            MODEL_ACTIVE_MARK.to_string(),
            Style::new().fg(model_active_color()),
        ));
    }
    Line::from(spans)
}

/// The window of `rows` that keeps the selection in view — `max` rows,
/// [`list_window`] riding the selection on the middle one — as how many rows
/// it hides above, its rows (the selected one marked, each `…`-cut to
/// `width`) and how many it hides below. `rows` is never empty here.
fn mascot_window(
    rows: &[MascotRow],
    selected: usize,
    max: usize,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = list_window(rows.len(), selected, max);
    let lines = rows[range.clone()]
        .iter()
        .zip(range)
        .map(|(row, i)| clamp_spans(mascot_row(row, i == selected).spans, usize::from(width)))
        .collect();
    (above, lines, below)
}

/// The list column's width: the widest row the catalog can paint (a
/// selected row wearing the ✓). A constant of the catalog, so the preview
/// beside it starts on one column whatever is selected or searched.
fn list_cols() -> usize {
    Mascot::ALL
        .into_iter()
        .map(|mascot| {
            let row = mascot_row(
                &MascotRow {
                    mascot,
                    active: true,
                },
                true,
            );
            spans_cols(&row.spans)
        })
        .max()
        .unwrap_or(0)
}

/// How the page lays its preview out ([`PreviewLayout`]): beside the list
/// once the terminal has room for the list column, [`MASCOT_PREVIEW_GAP`]
/// and the banner ([`MASCOT_PREVIEW_MIN_COLS`]), stacked under it
/// otherwise.
fn layout() -> PreviewLayout {
    PreviewLayout {
        list_cols: list_cols(),
        gap: MASCOT_PREVIEW_GAP,
        min_cols: MASCOT_PREVIEW_MIN_COLS,
    }
}

/// The list column in the `/mcp` list's shape ([`overflow_list`]): the
/// window of rows at [`MASCOT_MENU_MAX_ROWS`] between overflow rows that
/// name what it hides, if anything. The overflow rows are `…`-cut to
/// `overflow_width`, the rows to `width`.
fn list_lines(
    rows: &[MascotRow],
    selected: usize,
    width: u16,
    overflow_width: u16,
) -> Vec<Line<'static>> {
    let (above, window, below) =
        mascot_window(rows, selected, usize::from(MASCOT_MENU_MAX_ROWS), width);
    overflow_list(above, window, below, overflow_width)
}

/// The preview for `mascot` at `width`: the caption ([`caption_lines`] —
/// the mascot's name in bold over its description in the frame's dim), a
/// gap, and the live banner — the startup banner's own builder at `width`,
/// so the preview *is* the banner — padded with blank rows to the tallest
/// banner any mascot draws there ([`banner_slot`]).
fn preview_lines(app: &App, mascot: Mascot, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::from(caption_lines(
        &capitalized(mascot.name()),
        mascot.description(),
        usize::from(width),
    ));
    lines.push(Line::default());
    let slot = banner_slot(app, width);
    let mut banner = header_lines_for(app, mascot, width);
    banner.truncate(slot);
    banner.resize(slot, Line::default());
    lines.extend(banner);
    lines
}

/// The banner slot's fixed height at `width`: the tallest banner any mascot
/// in the catalog would draw, so moving the selection never changes the
/// page height (the `/settings` always-emit rule).
fn banner_slot(app: &App, width: u16) -> usize {
    Mascot::ALL
        .iter()
        .map(|&m| header_lines_for(app, m, width).len())
        .max()
        .unwrap_or(0)
}

/// The framed page as lines — a top rule, the `❯` search line straight
/// under it, a body, the key hint, and a bottom rule straight under that,
/// no blank row hugging either rule (`docs/view-flow.md` *The frame hugs
/// its content*) — the first of two pages that fits `height` rows
/// ([`first_page_that_fits`]). The **whole** page's body is the list
/// column and the captioned **live banner preview**, beside the list when
/// the terminal has the room and under it when it doesn't
/// ([`PreviewLayout::body`]). On a terminal too short for that it is a
/// **compact** page ([`compact_page`]): three of the rows between the two
/// reserved overflow slots, the preview yielding, and on a terminal shorter
/// still the slots, the rules and the rows one by one, so the search line,
/// the highlighted mascot and the hint stay on screen (`docs/view-flow.md`
/// *A page that sheds rows before it flows*). What [`render_mascot_picker`]
/// paints and [`mascot_menu_rows`] counts, so the reserved height and the
/// painted rows can never disagree. `height` is the terminal's, or the
/// region already reserved for the page, which builds the same page. Empty
/// when the picker is closed.
///
/// A search that matched **nothing** has no list, no caption and no banner
/// to preview, so its page collapses to the placeholder between single
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
    let whole = || {
        page(layout().body(
            width,
            &|overflow_width| list_lines(&rows, selected, width, overflow_width),
            &|preview_width| preview_lines(app, mascot, preview_width),
        ))
    };
    let compact = || {
        compact_page(
            height,
            &search_line,
            &|max| mascot_window(&rows, selected, max, width),
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
/// the composer: a top rule, the `❯` search line, the mascot rows in the
/// `/mcp` list's shape (`❯ crest ✓`), the captioned live banner preview —
/// beside the rows on a wide terminal, under them on a narrow one — the key
/// hint, and a bottom rule; or, in an area too short for that, the compact
/// page, which sheds its own rows down to the search line in an area shorter
/// still. The page is built for the area's height, so it always fits and
/// nothing flows (`docs/view-flow.md`). Pure — `render_live` paints this.
/// See `docs/mascot.md`.
pub fn render_mascot_picker(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        mascot_view_lines(app, area.width, area.height),
    );
}
