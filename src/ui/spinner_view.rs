//! The inline `/spinner` picker. See `docs/spinner.md`.
//!
//! The `/theme` picker's layout over the spinner-style catalog: the
//! family's frame and `❯` search line over one row per style in the **`/mcp`
//! list's shape** — the accent `❯` on the selection, the other rows indented
//! under it, dim `↑ N more above` / `↓ N more below` rows naming whatever a
//! window hides — and the page is **live**: every row carries its own
//! spinner, turning at the shared frame clock, and the highlighted style
//! previews as a whole sample status line under a caption naming it. Both
//! are built by the status line's own renderer
//! ([`styled_status_line`](super::status::styled_status_line) and the
//! `spinner_spans` it opens with), so what the picker shows and what a turn
//! shows can never disagree. Once the terminal has room for the list, a gap
//! and the sample, the preview sits **beside** the list; a narrower terminal
//! stacks it under the list ([`PreviewLayout`]). The page height never
//! depends on the clock or the selection — the comet is eight cells wide and
//! the rest one, but every style is exactly one row — and the list column is
//! sized off the whole catalog, so the frame holds still while it animates.

use super::model_view::{
    PreviewLayout, capitalized, caption_lines, compact_page, first_page_that_fits, list_window,
    marked_row_head, model_placeholder_row, model_rule, overflow_list, placeholder_window,
};
use super::status::{spinner_spans, styled_status_line};
use super::theme::*;
use super::wrap::{clamp_spans, cols, spans_cols};
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

/// The column the spinners start at, past the indent and the marker: the
/// widest name in the **whole catalog** plus [`SPINNER_MENU_GAP`] — never
/// just the listed rows', so a search or a scroll moves neither the spinners
/// nor (beside the list) the preview.
fn name_col() -> usize {
    Spinner::ALL
        .iter()
        .map(|spinner| cols(spinner.name()))
        .max()
        .unwrap_or(0)
        + SPINNER_MENU_GAP
}

/// One style row in the `/mcp` list's shape:
/// `{indent}{❯ }{name}{pad}{spinner} {✓}` ([`marked_row_head`]) — the
/// selected row's marker and name in the accent, every other row keeping
/// the marker's width blank; the row's own spinner turns at `elapsed`, and
/// the session's current style carries the `/model` picker's green ✓.
fn spinner_row(row: &SpinnerRow, selected: bool, elapsed: Duration) -> Line<'static> {
    let name = row.spinner.name();
    let mut spans = marked_row_head(name, selected);
    spans.push(Span::raw(" ".repeat(name_col().saturating_sub(cols(name)))));
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
/// live spinner, each `…`-cut to `width`) and how many it hides below.
/// `rows` is never empty here.
fn spinner_window(
    rows: &[SpinnerRow],
    selected: usize,
    max: usize,
    elapsed: Duration,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = list_window(rows.len(), selected, max);
    let lines = rows[range.clone()]
        .iter()
        .zip(range)
        .map(|(row, i)| {
            clamp_spans(
                spinner_row(row, i == selected, elapsed).spans,
                usize::from(width),
            )
        })
        .collect();
    (above, lines, below)
}

/// The list column's width: the widest row the catalog can paint (a
/// selected row wearing the ✓ — the comet and the two tracks are eight
/// cells wide). A constant of the catalog, so the preview beside it starts
/// on one column whatever is selected, searched or ticking.
fn list_cols() -> usize {
    Spinner::ALL
        .into_iter()
        .map(|spinner| {
            let row = spinner_row(
                &SpinnerRow {
                    spinner,
                    active: true,
                },
                true,
                Duration::ZERO,
            );
            spans_cols(&row.spans)
        })
        .max()
        .unwrap_or(0)
}

/// How the page lays its preview out ([`PreviewLayout`]): beside the list
/// once the terminal has room for the list column, [`SPINNER_PREVIEW_GAP`]
/// and the sample status line ([`SPINNER_PREVIEW_MIN_COLS`]), stacked under
/// it otherwise.
fn layout() -> PreviewLayout {
    PreviewLayout {
        list_cols: list_cols(),
        gap: SPINNER_PREVIEW_GAP,
        min_cols: SPINNER_PREVIEW_MIN_COLS,
    }
}

/// The list column in the `/mcp` list's shape ([`overflow_list`]): the
/// window of rows at [`SPINNER_MENU_MAX_ROWS`] between overflow rows that
/// name what it hides, if anything. The overflow rows are `…`-cut to
/// `overflow_width`, the rows to `width`.
fn list_lines(
    rows: &[SpinnerRow],
    selected: usize,
    elapsed: Duration,
    width: u16,
    overflow_width: u16,
) -> Vec<Line<'static>> {
    let (above, window, below) = spinner_window(
        rows,
        selected,
        usize::from(SPINNER_MENU_MAX_ROWS),
        elapsed,
        width,
    );
    overflow_list(above, window, below, overflow_width)
}

/// The preview for `spinner` at `width`: the caption ([`caption_lines`] —
/// the style's name in bold over its description in the frame's dim), a
/// gap, and the sample status line at `elapsed`, through the status line's
/// own renderer at `width` (it clamps itself with a dim `…` on a narrow
/// terminal, exactly as the real one does).
fn preview_lines(spinner: Spinner, elapsed: Duration, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::from(caption_lines(
        &capitalized(spinner.name()),
        spinner.description(),
        usize::from(width),
    ));
    lines.push(Line::default());
    lines.push(styled_status_line(
        &sample_status(elapsed),
        None,
        spinner,
        width,
    ));
    lines
}

/// The framed page as lines — a top rule, the `❯` search line straight
/// under it, a body, the key hint, and a bottom rule straight under that,
/// no blank row hugging either rule (`docs/view-flow.md` *The frame hugs
/// its content*) — the first of two pages that fits `height` rows
/// ([`first_page_that_fits`]). The **whole** page's body is the list column
/// (each row wearing its own live spinner) and the captioned **live
/// status-line preview**, beside the list when the terminal has the room
/// and under it when it doesn't ([`PreviewLayout::body`]). On a terminal too
/// short for that it is a **compact** page ([`compact_page`]): three of the
/// rows between the two reserved overflow slots, the preview yielding, and
/// on a terminal shorter still the slots, the rules and the rows one by
/// one, so the search line, the highlighted style and the hint stay on
/// screen (`docs/view-flow.md` *A page that sheds rows before it flows*).
/// What [`render_spinner_picker`] paints and [`spinner_menu_rows`] counts,
/// so the reserved height and the painted rows can never disagree. `height`
/// is the terminal's, or the region already reserved for the page, which
/// builds the same page. Empty when the picker is closed.
///
/// A search that matched **nothing** has no list, no caption and no
/// preview, so its page collapses to the placeholder between single gaps —
/// the `/mascot` picker's rule — and sheds the same way, keeping what was
/// typed.
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
    let whole = || {
        page(layout().body(
            width,
            &|overflow_width| list_lines(&rows, selected, elapsed, width, overflow_width),
            &|preview_width| preview_lines(spinner, elapsed, preview_width),
        ))
    };
    let compact = || {
        compact_page(
            height,
            &search_line,
            &|max| spinner_window(&rows, selected, max, elapsed, width),
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
/// the composer: a top rule, the `❯` search line, the style rows in the
/// `/mcp` list's shape with their live spinners (`❯ gravity  ⣤⣀⣀⣀⣀⣀⣀⣀ ✓`),
/// the captioned live status-line preview — beside the rows on a wide
/// terminal, under them on a narrow one — the key hint, and a bottom rule;
/// or, in an area too short for that, the compact page, which sheds its own
/// rows down to the search line in an area shorter still. The page is built
/// for the area's height, so it always fits and nothing flows
/// (`docs/view-flow.md`). Pure — `render_live` paints this. See
/// `docs/spinner.md`.
pub fn render_spinner_picker(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        spinner_view_lines(app, area.width, area.height),
    );
}
