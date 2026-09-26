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

use super::model_view::{model_placeholder_row, model_rule};
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

/// The picker's list lines: the rows windowed ([`centered_window`]) at
/// [`SPINNER_MENU_MAX_ROWS`] (the nine-style catalog never actually windows
/// today), or a single placeholder when the search matches nothing.
fn spinner_list_lines(
    rows: &[SpinnerRow],
    selected: usize,
    elapsed: Duration,
    width: u16,
) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return vec![model_placeholder_row(
            SPINNER_NO_MATCH,
            model_meta_color(),
            width,
        )];
    }
    let max = SPINNER_MENU_MAX_ROWS as usize;
    let selected = selected.min(rows.len() - 1);
    let offset = centered_window(rows.len(), selected, max);
    let visible = rows.iter().skip(offset).take(max);
    let name_col = visible
        .clone()
        .map(|row| cols(row.spinner.name()))
        .max()
        .unwrap_or(0)
        + SPINNER_MENU_GAP;
    rows.iter()
        .enumerate()
        .skip(offset)
        .take(max)
        .map(|(i, row)| spinner_row(row, i == selected, name_col, elapsed))
        .collect()
}

/// The `(selected+1/total)` counter under the list.
fn spinner_counter_line(rows: &[SpinnerRow], selected: usize) -> Line<'static> {
    let selected = selected.min(rows.len().saturating_sub(1));
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("({}/{})", selected + 1, rows.len()),
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

/// The whole framed page as lines: a top rule, the `❯` search line, the
/// style rows (each wearing its own live spinner), the `(n/total)` counter,
/// the **live status-line preview**, the highlighted style's description,
/// the key hint, and a bottom rule — blank-gapped like its family. What
/// [`render_spinner_picker`] paints (bottom-anchored) and
/// [`spinner_menu_rows`] counts, so the reserved height and the painted rows
/// can never disagree (`docs/view-flow.md`). Empty when the picker is closed.
///
/// A search that matched **nothing** has no count, no preview and no
/// description, so those slots collapse to a single blank gap rather than
/// painting as a band of empty rows — the `/mascot` picker's rule.
pub(super) fn spinner_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(picker) = app.spinner_picker.as_ref() else {
        return Vec::new();
    };
    let rows = app.spinner_rows();
    let selected = picker.selected;
    let elapsed = app.spinner_preview_elapsed();
    let search_line = Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(model_selected_color())),
        Span::raw(picker.query.clone()),
    ]);
    let highlighted = rows
        .get(selected.min(rows.len().saturating_sub(1)))
        .map(|row| row.spinner);
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        search_line,
        Line::default(),
    ];
    lines.extend(spinner_list_lines(&rows, selected, elapsed, width));
    match highlighted {
        // A real selection: the counter, the live preview and the
        // description sit between the list and the hint. Every style is one
        // row, so the frame holds still as ↑/↓ move and the clock ticks.
        Some(spinner) => {
            lines.push(spinner_counter_line(&rows, selected));
            lines.push(Line::default());
            lines.push(preview_line(spinner, elapsed, width));
            lines.push(Line::default());
            lines.push(model_placeholder_row(
                spinner.description(),
                model_meta_color(),
                width,
            ));
            lines.push(Line::default());
        }
        // Nothing matched: one gap carries the placeholder to the hint.
        None => lines.push(Line::default()),
    }
    lines.push(model_placeholder_row(
        SPINNER_HINT,
        model_meta_color(),
        width,
    ));
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// The rows the picker's own frame occupies — the built page's line count
/// ([`spinner_view_lines`]). What [`spinner_picker_height`] reserves under
/// the strip, and what [`render_live`] hands [`render_spinner_picker`].
pub(super) fn spinner_menu_rows(app: &App, width: u16) -> u16 {
    u16::try_from(spinner_view_lines(app, width).len()).unwrap_or(u16::MAX)
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
        spinner_menu_rows(app, width),
        term_height,
    ))
}

/// Render the **inline** `/spinner` picker into the live region, in place of
/// the composer: a top rule, the `❯` search line, the style rows with their
/// live spinners (`→ gravity  ⣤⣀⣀⣀⣀⣀⣀⣀ ✓`), a `(n/total)` counter, the live
/// status-line preview, the highlighted style's description, the key hint,
/// and a bottom rule — bottom-anchored, so a squeezed area keeps the
/// preview, the hint and the closing rule on screen while the skipped top
/// flows into scrollback (`docs/view-flow.md`). Pure — `render_live` paints
/// this. See `docs/spinner.md`.
pub fn render_spinner_picker(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, spinner_view_lines(app, area.width));
}
