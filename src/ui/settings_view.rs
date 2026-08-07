//! The inline `/settings` menu. See `docs/settings.md`.
//!
//! The `/model` picker's framed shape (`model_view`) with a value column in
//! place of the `[provider]` tag and one extra chrome row — the key hint under
//! the description. It renders from [`App`] rather than from a picker struct,
//! because the rows *are* the live session state ([`App::setting_rows`]).

use super::model_view::{model_placeholder_row, model_rule};
use super::theme::*;
use super::wrap::{cols, truncate_cols};
use super::*;

use crate::app::SettingRow;

/// One setting row: `{marker}{label}{pad}{value}` — the selected row's marker
/// and label light up cyan (the picker family's accent), the value is light
/// grey when it's doing something and dim when it isn't. `label_width` is the
/// widest visible label, so the value column lines up down the list.
fn settings_row(row: &SettingRow, selected: bool, label_width: usize, width: u16) -> Line<'static> {
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
        Span::styled(truncate_cols(&row.value, value_room), value_style),
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
/// placeholder when the search matches nothing. Its length equals
/// [`settings_list_rows`] so the reserved height and painted rows agree.
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

/// How many rows the menu's **list** occupies: the row count capped at
/// [`SETTINGS_MENU_MAX_ROWS`], or one placeholder row when nothing matches.
/// Must equal `settings_list_lines(..).len()`.
fn settings_list_rows(rows: usize) -> u16 {
    if rows == 0 {
        1
    } else {
        (rows as u16).min(SETTINGS_MENU_MAX_ROWS)
    }
}

/// The inline live-region height when the `/settings` menu is open, or `None`
/// when it isn't (the caller then falls back to [`live_height`]). Like the
/// `/model` picker the menu **replaces** the composer, so this is the whole
/// region — the fixed chrome plus the (possibly scrolled) list — clamped to the
/// terminal height. Shared by `tui::view`'s `live_region_height`,
/// [`render_live`], and [`cursor_position`] so all three agree.
#[must_use]
pub fn settings_height(app: &App, term_height: u16) -> Option<u16> {
    app.settings_picker.as_ref()?;
    let list = settings_list_rows(app.setting_rows().len());
    Some((SETTINGS_CHROME_ROWS + list).min(term_height.max(1)))
}

/// Render the **inline** `/settings` menu into the live region, in place of the
/// composer: a top rule, the `❯` search line, the setting rows (`→ {label}
/// {value}`), a `(n/total)` counter, the highlighted setting's description, the
/// key hint, and a bottom rule. Pure — `render_live` paints this. See
/// `docs/settings.md`.
pub fn render_settings(area: Rect, buf: &mut Buffer, app: &App) {
    let Some(picker) = app.settings_picker.as_ref() else {
        return;
    };
    let rows = app.setting_rows();
    let selected = picker.selected;

    let [
        top_rule,
        _gap1,
        search,
        _gap2,
        list,
        counter,
        _gap3,
        description,
        _gap4,
        hint,
        _gap5,
        bottom_rule,
    ] = Layout::vertical([
        Constraint::Length(1), // top rule
        Constraint::Length(1), // gap
        Constraint::Length(1), // search
        Constraint::Length(1), // gap
        Constraint::Min(0),    // setting list
        Constraint::Length(1), // counter
        Constraint::Length(1), // gap
        Constraint::Length(1), // description of the highlighted row
        Constraint::Length(1), // gap
        Constraint::Length(1), // key hint
        Constraint::Length(1), // gap
        Constraint::Length(1), // bottom rule
    ])
    .areas(area);

    let search_line = Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(MODEL_SELECTED_COLOR)),
        Span::raw(picker.query.clone()),
    ]);
    // The description names what the highlighted row does; with nothing
    // matched the row stays blank (the layout still reserves it, so the frame
    // doesn't jump as the search narrows).
    let description_line = rows
        .get(selected.min(rows.len().saturating_sub(1)))
        .map_or_else(Line::default, |row| {
            model_placeholder_row(row.description, MODEL_META_COLOR, area.width)
        });

    Paragraph::new(model_rule(area.width)).render(top_rule, buf);
    Paragraph::new(search_line).render(search, buf);
    Paragraph::new(settings_list_lines(&rows, selected, area.width)).render(list, buf);
    Paragraph::new(settings_counter_line(&rows, selected)).render(counter, buf);
    Paragraph::new(description_line).render(description, buf);
    Paragraph::new(model_placeholder_row(
        SETTINGS_HINT,
        MODEL_META_COLOR,
        area.width,
    ))
    .render(hint, buf);
    Paragraph::new(model_rule(area.width)).render(bottom_rule, buf);
}
