//! The inline `/settings` menu. See `docs/state/settings.md`.
//!
//! The `/model` picker's framed shape (`model_view`) with a value column in
//! place of the `[provider]` tag and one extra chrome row — the key hint under
//! the description. It renders from [`App`] rather than from a picker struct,
//! because the rows *are* the live session state ([`App::setting_rows`]).

use super::model_view::{
    Description, compact_page, compact_page_with_header, first_page_that_fits, list_counter_line,
    list_window, marked_row_head, model_description_rows, model_placeholder_row, picker_header,
    picker_page, placeholder_window, row_head_cols, search_line,
};
use super::theme::*;
use super::wrap::{cols, ellipsize};
use super::*;

use crate::app::SettingRow;

/// One setting row: `{indent}{marker}{label}{pad}{value}` ([`marked_row_head`])
/// — the selected row's marker and label light up cyan (the picker family's
/// accent), the value is light grey when it's doing something and dim when
/// it isn't. `label_width` is the widest listed label, so the value column
/// lines up down the list.
pub(super) fn settings_row(
    row: &SettingRow,
    selected: bool,
    label_width: usize,
    width: u16,
) -> Line<'static> {
    // A label wider than the column `…`-cuts (`label_column` caps the column
    // so the value keeps a seat on a narrow terminal), and a shorter one pads
    // to it, so every value starts at the same column.
    let label = ellipsize(row.label, label_width.max(1));
    let pad = label_width.saturating_sub(cols(&label)) + SETTINGS_VALUE_GAP;
    let reserved = row_head_cols() + cols(&label) + pad;
    let value_room = (width as usize).saturating_sub(reserved).max(1);
    let value_style = Style::new().fg(if row.available && !is_off_value(&row.value) {
        settings_value_color()
    } else {
        settings_value_off_color()
    });
    let mut spans = marked_row_head(&label, selected);
    spans.push(Span::raw(" ".repeat(pad)));
    // `…`-cut, never silent: "false (unavailable)" clipped to "false" would
    // misrepresent an unavailable knob as a live false.
    spans.push(Span::styled(ellipsize(&row.value, value_room), value_style));
    Line::from(spans)
}

/// Whether a value reads as "off" — dimmed so a glance down the column shows
/// what is actually doing something.
fn is_off_value(value: &str) -> bool {
    SETTINGS_OFF_VALUES.contains(&value)
}

/// The widest label among `rows`, so [`settings_row`] can align the value
/// column — capped so the value keeps at least a column on the row (`width`
/// minus the indent and the marker, the gap, and that column), a label past
/// it `…`-cutting rather than pushing the row off the terminal.
fn label_column(rows: &[SettingRow], width: u16) -> usize {
    let cap = (width as usize)
        .saturating_sub(row_head_cols() + SETTINGS_VALUE_GAP + 1)
        .max(1);
    rows.iter()
        .map(|r| cols(r.label))
        .max()
        .unwrap_or(0)
        .min(cap)
}

/// The window of `rows` that keeps the selection in view — `max` rows,
/// [`list_window`] riding the selection on the middle one — as how many rows
/// it hides above, its rows (the selected one marked) and how many it hides
/// below. The value column is sized off **every** listed row, not just the
/// ones in view, so it holds still as ↑/↓ scroll the window. `rows` is never
/// empty here.
fn settings_window(
    rows: &[SettingRow],
    selected: usize,
    max: usize,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = list_window(rows.len(), selected, max);
    let label_width = label_column(rows, width);
    let lines = rows[range.clone()]
        .iter()
        .zip(range)
        .map(|(row, i)| settings_row(row, i == selected, label_width, width))
        .collect();
    (above, lines, below)
}

/// The whole page's body — what sits between the search line and the hint:
/// a gap, the rows ([`settings_window`] at [`SETTINGS_MENU_MAX_ROWS`]), the
/// `(n/total)` counter, and the highlighted row's description between gaps,
/// laid out by `description` — wrapped whole when the terminal has the room,
/// since the description is why the row exists (type-to-search even matches
/// on it). The counter and the description hang under the labels.
fn settings_whole_body(
    rows: &[SettingRow],
    selected: usize,
    description: Description,
    width: u16,
) -> Vec<Line<'static>> {
    let (_, window, _) =
        settings_window(rows, selected, usize::from(SETTINGS_MENU_MAX_ROWS), width);
    let mut lines = vec![Line::default()];
    lines.extend(window);
    lines.push(list_counter_line(selected, rows.len()));
    lines.push(Line::default());
    lines.extend(model_description_rows(
        rows[selected].description,
        description,
        width,
    ));
    lines.push(Line::default());
    lines
}

/// The framed page as lines — a top rule, the `Settings` title over its dim
/// subtitle, the `❯` search line, a body, the key hint, and a bottom rule,
/// one blank row between each block and none hugging either rule
/// (`docs/ui/view-flow.md` *The frame hugs its content*) — the first of four
/// pages that fits `height` rows ([`first_page_that_fits`]):
///
/// 1. the **whole** page: the windowed setting rows, the `(n/total)`
///    counter and the highlighted row's description wrapped whole;
/// 2. the whole page with the description **cut to one row**, which is one
///    height at every selection, so a terminal at the edge never flips
///    between the whole and the compact page as ↑/↓ cross a long one;
/// 3. the **compact** page with the header kept
///    ([`compact_page_with_header`]): the description and the counter
///    yield, and three rows of the list stay between the search line and
///    the hint;
/// 4. the header-less compact page ([`compact_page`]), and on a terminal
///    shorter still its rungs: the overflow slots, the rules and the rows
///    yield one by one, never the search line (`docs/ui/view-flow.md`).
///
/// What [`render_settings`] paints and
/// [`settings_rows`] counts, so the reserved height and the painted rows can
/// never disagree. `height` is the terminal's, or the region already
/// reserved for the page, which builds the same page. A search that matched
/// nothing has no count and nothing to describe, so its page collapses to
/// the placeholder between single gaps — the `/model` picker's placeholder
/// rule — and sheds the same way, keeping what was typed. Empty when the
/// menu is closed.
pub(super) fn settings_view_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    let Some(picker) = app.settings_picker.as_ref() else {
        return Vec::new();
    };
    let rows = app.setting_rows();
    let header = picker_header(SETTINGS_TITLE, SETTINGS_SUBTITLE, width);
    let search = search_line(&picker.query);
    let hint = model_placeholder_row(SETTINGS_HINT, model_meta_color(), width);
    let page = |mut body: Vec<Line<'static>>| {
        body.push(hint.clone());
        picker_page(&header, &search, body, width)
    };
    if rows.is_empty() {
        let block = [model_placeholder_row(
            SETTINGS_NO_MATCH,
            model_meta_color(),
            width,
        )];
        return first_page_that_fits(
            height,
            &[
                &|| page(vec![Line::default(), block[0].clone(), Line::default()]),
                &|| {
                    compact_page(
                        height,
                        &search,
                        &|max| placeholder_window(&block, max),
                        std::slice::from_ref(&hint),
                        width,
                    )
                },
            ],
        );
    }
    let selected = picker.selected.min(rows.len() - 1);
    let whole = |description| settings_whole_body(&rows, selected, description, width);
    let window = |max| settings_window(&rows, selected, max, width);
    let tail = std::slice::from_ref(&hint);
    first_page_that_fits(
        height,
        &[
            &|| page(whole(Description::Wrapped)),
            &|| page(whole(Description::OneRow)),
            &|| compact_page_with_header(&header, &search, &window, tail, width),
            &|| compact_page(height, &search, &window, tail, width),
        ],
    )
}

/// The rows the menu's own frame occupies — the built page's line count
/// ([`settings_view_lines`]), so the height and the paint agree by
/// construction. What [`settings_height`] reserves under the strip, and what
/// [`render_live`] hands [`render_settings`].
pub(super) fn settings_rows(app: &App, width: u16, height: u16) -> u16 {
    u16::try_from(settings_view_lines(app, width, height).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/settings` menu is open, or `None`
/// when it isn't (the caller then falls back to [`live_height`]). Like the
/// `/model` picker the menu **replaces** the composer — and only the composer:
/// the streaming strip keeps its rows above it (`ui::layout`'s
/// `strip_above_rows`), so opening `/settings` mid-turn never hides the
/// running turn. Clamped to the terminal height. Shared by `tui::view`'s
/// `live_region_height`, [`render_live`], and [`cursor_position`] so all three
/// agree.
#[must_use]
pub fn settings_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.settings_picker.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        settings_rows(app, width, term_height),
        term_height,
    ))
}

/// Render the **inline** `/settings` menu into the live region, in place of the
/// composer: a top rule, the `❯` search line, the setting rows (`→ {label}
/// {value}`), a `(n/total)` counter, the highlighted setting's description, the
/// key hint, and a bottom rule; or, in an area too short for that, the
/// compact page, which sheds its own rows down to the search line in an area
/// shorter still. The page is built for the area's height, so it always fits
/// and nothing flows (`docs/ui/view-flow.md`). Pure — `render_live` paints
/// this. See `docs/state/settings.md`.
pub fn render_settings(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        settings_view_lines(app, area.width, area.height),
    );
}
