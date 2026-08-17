//! The inline `/settings` menu. See `docs/settings.md`.
//!
//! The `/model` picker's framed shape (`model_view`) with a value column in
//! place of the `[provider]` tag and one extra chrome row — the key hint under
//! the description. It renders from [`App`] rather than from a picker struct,
//! because the rows *are* the live session state ([`App::setting_rows`]).

use super::model_view::{model_placeholder_row, model_rule, model_wrapped_rows};
use super::theme::*;
use super::wrap::{cols, ellipsize};
use super::*;

use crate::app::SettingRow;

/// One setting row: `{marker}{label}{pad}{value}` — the selected row's marker
/// and label light up cyan (the picker family's accent), the value is light
/// grey when it's doing something and dim when it isn't. `label_width` is the
/// widest visible label, so the value column lines up down the list.
pub(super) fn settings_row(
    row: &SettingRow,
    selected: bool,
    label_width: usize,
    width: u16,
) -> Line<'static> {
    let marker = if selected { MODEL_MARKER } else { "  " };
    let (marker_style, label_style) = if selected {
        (
            Style::new().fg(MODEL_SELECTED_COLOR),
            Style::new()
                .fg(MODEL_SELECTED_COLOR)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (Style::default(), Style::new().fg(MODEL_ID_COLOR))
    };
    // The label pads to the column even when this row's own label is shorter,
    // so every value starts at the same column.
    let pad = label_width.saturating_sub(cols(row.label)) + SETTINGS_VALUE_GAP;
    let reserved = cols(marker) + cols(row.label) + pad;
    let value_room = (width as usize).saturating_sub(reserved).max(1);
    let value_style = Style::new().fg(if row.available && !is_off_value(&row.value) {
        SETTINGS_VALUE_COLOR
    } else {
        SETTINGS_VALUE_OFF_COLOR
    });
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(row.label.to_string(), label_style),
        Span::raw(" ".repeat(pad)),
        // `…`-cut, never silent: "false (unavailable)" clipped to "false"
        // would misrepresent an unavailable knob as a live false.
        Span::styled(ellipsize(&row.value, value_room), value_style),
    ])
}

/// Whether a value reads as "off" — dimmed so a glance down the column shows
/// what is actually doing something.
fn is_off_value(value: &str) -> bool {
    SETTINGS_OFF_VALUES.contains(&value)
}

/// The widest visible label, so [`settings_row`] can align the value column.
fn label_column(rows: &[&SettingRow]) -> usize {
    rows.iter().map(|r| cols(r.label)).max().unwrap_or(0)
}

/// The menu's list lines: the rows windowed ([`centered_window`]) to keep the
/// selection **centered** and capped at [`SETTINGS_MENU_MAX_ROWS`], or a single
/// placeholder when the search matches nothing.
fn settings_list_lines(rows: &[SettingRow], selected: usize, width: u16) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return vec![model_placeholder_row(
            SETTINGS_NO_MATCH,
            MODEL_META_COLOR,
            width,
        )];
    }
    let max = SETTINGS_MENU_MAX_ROWS as usize;
    let selected = selected.min(rows.len() - 1);
    let offset = centered_window(rows.len(), selected, max);
    let visible: Vec<&SettingRow> = rows.iter().skip(offset).take(max).collect();
    let label_width = label_column(&visible);
    visible
        .iter()
        .enumerate()
        .map(|(i, row)| settings_row(row, offset + i == selected, label_width, width))
        .collect()
}

/// The `(selected+1/total)` counter under the list, or a blank line when the
/// search matched nothing.
fn settings_counter_line(rows: &[SettingRow], selected: usize) -> Line<'static> {
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

/// The whole framed page as lines: a top rule, the `❯` search line, the
/// windowed setting rows, the `(n/total)` counter, the highlighted row's
/// description, the key hint, and a bottom rule — blank-gapped exactly as the
/// retired internal `Layout` stacked them. What [`render_settings`] paints
/// (bottom-anchored) and [`settings_rows`] counts, so the reserved height and
/// the painted rows can never disagree — the `/mcp` family's rule
/// (`docs/view-flow.md`). Empty when the menu is closed.
pub(super) fn settings_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(picker) = app.settings_picker.as_ref() else {
        return Vec::new();
    };
    let rows = app.setting_rows();
    let selected = picker.selected;
    let search_line = Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(MODEL_SELECTED_COLOR)),
        Span::raw(picker.query.clone()),
    ]);
    let highlighted = rows.get(selected.min(rows.len().saturating_sub(1)));
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        search_line,
        Line::default(),
    ];
    lines.extend(settings_list_lines(&rows, selected, width));
    match highlighted {
        // A real row: the counter and the description name what it does —
        // the description **wrapped**, not clipped: it is why the row exists
        // (type-to-search even matches on it), and the page's height is the
        // built line count, so the continuation rows are free.
        Some(row) => {
            lines.push(settings_counter_line(&rows, selected));
            lines.push(Line::default());
            lines.extend(model_wrapped_rows(row.description, MODEL_META_COLOR, width));
            lines.push(Line::default());
        }
        // Nothing matched: there is no count and nothing to describe, so the
        // two slots collapse to ONE blank gap carrying the placeholder to the
        // hint — the `/model` picker's placeholder rule. Painting them as
        // empty rows opened a band of blank lines mid-frame.
        None => lines.push(Line::default()),
    }
    lines.push(model_placeholder_row(
        SETTINGS_HINT,
        MODEL_META_COLOR,
        width,
    ));
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// The rows the menu's own frame occupies — the built page's line count
/// ([`settings_view_lines`]), so the height and the paint agree by
/// construction. What [`settings_height`] reserves under the strip, and what
/// [`render_live`] hands [`render_settings`].
pub(super) fn settings_rows(app: &App, width: u16) -> u16 {
    u16::try_from(settings_view_lines(app, width).len()).unwrap_or(u16::MAX)
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
        settings_rows(app, width),
        term_height,
    ))
}

/// Render the **inline** `/settings` menu into the live region, in place of the
/// composer: a top rule, the `❯` search line, the setting rows (`→ {label}
/// {value}`), a `(n/total)` counter, the highlighted setting's description, the
/// key hint, and a bottom rule — bottom-anchored, so a squeezed area keeps the
/// whole list, the hint and the closing rule on screen while the skipped top
/// flows into scrollback (`docs/view-flow.md`). Pure — `render_live` paints
/// this. See `docs/settings.md`.
pub fn render_settings(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, settings_view_lines(app, area.width));
}
