//! The inline `/skills` menu. See `docs/skills.md`.
//!
//! The `/settings` menu's frame, reused rather than re-drawn: the same rules,
//! the same `❯` search line, the same aligned `{label}  {value}` column with
//! the same two-tone colouring, the same `(n/total)` counter over the
//! highlighted row's description, the same hint row. Only the rows differ —
//! one per discovered skill, its own `description` as the line beneath — plus
//! one note row this menu has and that one doesn't: the session-off banner,
//! and the where-to-put-one answer for an empty list.

use super::model_view::{model_placeholder_row, model_rule};
use super::theme::*;
use super::wrap::{cols, truncate_cols};
use super::*;

use crate::app::SkillMenuRow;

/// One skill row: `{marker}{name}{pad}{enabled|disabled}` — the `/settings`
/// row's shape exactly, so the two menus read as one family. `name_width` is
/// the widest visible name, so the value column lines up down the list.
fn skills_row(row: &SkillMenuRow, selected: bool, name_width: usize, width: u16) -> Line<'static> {
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
    let pad = name_width.saturating_sub(cols(&row.name)) + SETTINGS_VALUE_GAP;
    let reserved = cols(marker) + cols(&row.name) + pad;
    let value_room = (width as usize).saturating_sub(reserved).max(1);
    let (value, value_color) = if row.enabled {
        (SKILLS_ON_VALUE, SETTINGS_VALUE_COLOR)
    } else {
        (SKILLS_OFF_VALUE, SETTINGS_VALUE_OFF_COLOR)
    };
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(row.name.clone(), name_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(
            truncate_cols(value, value_room),
            Style::new().fg(value_color),
        ),
    ])
}

/// The widest visible name, so [`skills_row`] can align the value column.
fn name_column(rows: &[&SkillMenuRow]) -> usize {
    rows.iter().map(|r| cols(&r.name)).max().unwrap_or(0)
}

/// The placeholder lines shown in place of the list: one row saying nothing
/// matched the search, or — when nothing was discovered at all — the roots
/// skills are looked for in, since that is the only question an empty list
/// raises. Its length is [`skills_list_rows`]'s zero-row answer.
fn skills_empty_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let roots = app
        .skills_menu
        .as_ref()
        .map(|menu| (menu.skills.is_empty(), menu.roots.clone()));
    match roots {
        // Nothing discovered: name where one would go.
        Some((true, roots)) if !roots.is_empty() => {
            let mut lines = vec![model_placeholder_row(
                SKILLS_NONE_FOUND,
                MODEL_META_COLOR,
                width,
            )];
            lines.extend(roots.iter().map(|root| {
                model_placeholder_row(&format!("{root}/<name>/SKILL.md"), MODEL_ID_COLOR, width)
            }));
            lines
        }
        // Skills exist, the search just matched none of them.
        _ => vec![model_placeholder_row(
            SKILLS_NO_MATCH,
            MODEL_META_COLOR,
            width,
        )],
    }
}

/// How many rows the menu's **list** occupies: the row count capped at
/// [`SETTINGS_MENU_MAX_ROWS`], else the placeholder block's height. Must equal
/// `skills_list_lines(..).len()`.
fn skills_list_rows(app: &App, rows: usize, width: u16) -> u16 {
    if rows == 0 {
        return u16::try_from(skills_empty_lines(app, width).len()).unwrap_or(1);
    }
    (rows as u16).min(SETTINGS_MENU_MAX_ROWS)
}

/// The menu's list lines: the rows windowed ([`centered_window`]) to keep the
/// selection **centered** and capped at [`SETTINGS_MENU_MAX_ROWS`], else the
/// placeholder block.
fn skills_list_lines(
    app: &App,
    rows: &[SkillMenuRow],
    selected: usize,
    width: u16,
) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return skills_empty_lines(app, width);
    }
    let max = SETTINGS_MENU_MAX_ROWS as usize;
    let selected = selected.min(rows.len() - 1);
    let offset = centered_window(rows.len(), selected, max);
    let visible: Vec<&SkillMenuRow> = rows.iter().skip(offset).take(max).collect();
    let name_width = name_column(&visible);
    visible
        .iter()
        .enumerate()
        .map(|(i, row)| skills_row(row, offset + i == selected, name_width, width))
        .collect()
}

/// The `(selected+1/total)` counter under the list, or a blank line when the
/// list is empty.
fn skills_counter_line(rows: &[SkillMenuRow], selected: usize) -> Line<'static> {
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

/// The rows the menu's own frame occupies: the fixed chrome plus the (possibly
/// scrolled) list. What [`skills_menu_height`] reserves under the strip, and
/// what [`render_live`] hands [`render_skills_menu`].
pub(super) fn skills_menu_rows(app: &App, width: u16) -> u16 {
    SKILLS_CHROME_ROWS + skills_list_rows(app, app.skill_menu_rows().len(), width)
}

/// The inline live-region height when the `/skills` menu is open, or `None`
/// when it isn't (the caller then falls back to [`live_height`]). Like every
/// sibling picker the menu **replaces** the composer — and only the composer:
/// the streaming strip keeps its rows above it, so opening `/skills` mid-turn
/// never hides the running turn. Clamped to the terminal height.
#[must_use]
pub fn skills_menu_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.skills_menu.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        skills_menu_rows(app, width),
        term_height,
    ))
}

/// Render the **inline** `/skills` menu into the live region, in place of the
/// composer: a top rule, the `❯` search line, the session-off note (blank when
/// skills are on), the skill rows (`→ {name}  {enabled|disabled}`), a
/// `(n/total)` counter, the highlighted skill's description, the key hint, and
/// a bottom rule. Pure — `render_live` paints this. See `docs/skills.md`.
pub fn render_skills_menu(area: Rect, buf: &mut Buffer, app: &App) {
    let Some(menu) = app.skills_menu.as_ref() else {
        return;
    };
    let rows = app.skill_menu_rows();
    let selected = menu.selected;

    let [
        top_rule,
        _gap1,
        search,
        note,
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
        Constraint::Length(1), // the session-off note (blank when on)
        Constraint::Length(1), // gap
        Constraint::Min(0),    // skill list
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
        Span::raw(menu.query.clone()),
    ]);
    // The row is always reserved, so turning the session switch off doesn't
    // make the frame jump — it just fills in.
    let note_line = if menu.session_enabled {
        Line::default()
    } else {
        model_placeholder_row(SKILLS_SESSION_OFF, TOAST_ERROR_COLOR, area.width)
    };
    let description_line = rows
        .get(selected.min(rows.len().saturating_sub(1)))
        .map_or_else(Line::default, |row| {
            model_placeholder_row(&row.description, MODEL_META_COLOR, area.width)
        });

    Paragraph::new(model_rule(area.width)).render(top_rule, buf);
    Paragraph::new(search_line).render(search, buf);
    Paragraph::new(note_line).render(note, buf);
    Paragraph::new(skills_list_lines(app, &rows, selected, area.width)).render(list, buf);
    Paragraph::new(skills_counter_line(&rows, selected)).render(counter, buf);
    Paragraph::new(description_line).render(description, buf);
    Paragraph::new(model_placeholder_row(
        SKILLS_HINT,
        MODEL_META_COLOR,
        area.width,
    ))
    .render(hint, buf);
    Paragraph::new(model_rule(area.width)).render(bottom_rule, buf);
}
