//! The inline `/skills` menu. See `docs/skills.md`.
//!
//! The `/settings` menu's frame, reused rather than re-drawn: the same rules,
//! the same `❯` search line, the same aligned `{label}  {value}` column with
//! the same two-tone colouring, the same `(n/total)` counter over the
//! highlighted row's description, the same hint row. Only the rows differ —
//! one per discovered skill, its own `description` as the line beneath — plus
//! one note row this menu has and that one doesn't: the session-off banner,
//! and the where-to-put-one answer for an empty list.

use super::model_view::{model_placeholder_row, model_rule, model_wrapped_rows};
use super::theme::*;
use super::wrap::{cols, ellipsize};
use super::*;

use crate::app::SkillMenuRow;

/// One skill row: `{marker}{name}{pad}{enabled|disabled}` — the `/settings`
/// row's shape exactly, so the two menus read as one family. `name_width` is
/// the (capped) widest visible name, so the value column lines up down the
/// list; a name wider than it `…`-cuts rather than paint-clipping at the
/// buffer edge and shoving the value off the row.
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
    let name = ellipsize(&row.name, name_width.max(1));
    let pad = name_width.saturating_sub(cols(&name)) + SETTINGS_VALUE_GAP;
    let reserved = cols(marker) + cols(&name) + pad;
    let value_room = (width as usize).saturating_sub(reserved).max(1);
    let (value, value_color) = if row.enabled {
        (SKILLS_ON_VALUE, SETTINGS_VALUE_COLOR)
    } else {
        (SKILLS_OFF_VALUE, SETTINGS_VALUE_OFF_COLOR)
    };
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(name, name_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(ellipsize(value, value_room), Style::new().fg(value_color)),
    ])
}

/// The widest visible name — capped so the value column always keeps a seat
/// on the row (`width` minus the marker, the gap, and the widest value) —
/// so [`skills_row`] can align the value column.
fn name_column(rows: &[&SkillMenuRow], width: u16) -> usize {
    let widest_value = cols(SKILLS_ON_VALUE).max(cols(SKILLS_OFF_VALUE));
    let cap = (width as usize)
        .saturating_sub(cols(MODEL_MARKER) + SETTINGS_VALUE_GAP + widest_value)
        .max(1);
    rows.iter()
        .map(|r| cols(&r.name))
        .max()
        .unwrap_or(0)
        .min(cap)
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
        // Nothing discovered: name where one would go — the path rows ARE
        // the instruction, so they wrap rather than losing their tails.
        Some((true, roots)) if !roots.is_empty() => {
            let mut lines = vec![model_placeholder_row(
                SKILLS_NONE_FOUND,
                MODEL_META_COLOR,
                width,
            )];
            lines.extend(roots.iter().flat_map(|root| {
                model_wrapped_rows(&format!("{root}/<name>/SKILL.md"), MODEL_ID_COLOR, width)
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
    let name_width = name_column(&visible, width);
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

/// The whole framed page as lines: a top rule, the `❯` search line, the
/// session-off note (blank when skills are on — always emitted, so the frame
/// doesn't jump), the windowed skill rows, the `(n/total)` counter, the
/// highlighted skill's description, the key hint, and a bottom rule —
/// blank-gapped exactly as the retired internal `Layout` stacked them. What
/// [`render_skills_menu`] paints (bottom-anchored) and [`skills_menu_rows`]
/// counts, so the two can never disagree (`docs/view-flow.md`). Empty when
/// the menu is closed.
pub(super) fn skills_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(menu) = app.skills_menu.as_ref() else {
        return Vec::new();
    };
    let rows = app.skill_menu_rows();
    let selected = menu.selected;
    let search_line = Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(MODEL_SELECTED_COLOR)),
        Span::raw(menu.query.clone()),
    ]);
    let highlighted = rows.get(selected.min(rows.len().saturating_sub(1)));
    let mut lines = vec![model_rule(width), Line::default(), search_line];
    // The session-off note takes rows only when there IS one to show —
    // emitting it blank stacked an empty line on the gap below it. Wrapped:
    // its "/settings" tail is the fix, and a narrow terminal used to cut it.
    if !menu.session_enabled {
        lines.extend(model_wrapped_rows(
            SKILLS_SESSION_OFF,
            TOAST_ERROR_COLOR,
            width,
        ));
    }
    lines.push(Line::default());
    lines.extend(skills_list_lines(app, &rows, selected, width));
    match highlighted {
        // A real row: the counter and the skill's own description — wrapped
        // whole, because this picker doubles as the browser that answers
        // "what is this skill for?" and a SKILL.md description routinely
        // outruns the terminal width.
        Some(row) => {
            lines.push(skills_counter_line(&rows, selected));
            lines.push(Line::default());
            lines.extend(model_wrapped_rows(
                &row.description,
                MODEL_META_COLOR,
                width,
            ));
            lines.push(Line::default());
        }
        // Nothing matched: one gap carries the placeholder to the hint (the
        // `/settings` menu's collapse — the two menus stay twins).
        None => lines.push(Line::default()),
    }
    lines.push(model_placeholder_row(SKILLS_HINT, MODEL_META_COLOR, width));
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// The rows the menu's own frame occupies — the built page's line count
/// ([`skills_view_lines`]). What [`skills_menu_height`] reserves under the
/// strip, and what [`render_live`] hands [`render_skills_menu`].
pub(super) fn skills_menu_rows(app: &App, width: u16) -> u16 {
    u16::try_from(skills_view_lines(app, width).len()).unwrap_or(u16::MAX)
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
/// a bottom rule — bottom-anchored, so a squeezed area keeps the list and the
/// closing chrome on screen while the skipped top flows into scrollback
/// (`docs/view-flow.md`). Pure — `render_live` paints this. See
/// `docs/skills.md`.
pub fn render_skills_menu(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, skills_view_lines(app, area.width));
}
