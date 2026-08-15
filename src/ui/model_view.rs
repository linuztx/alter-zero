//! The inline `/model` picker. See `docs/llm.md`.

use super::layout::model_has_detail;
use super::theme::*;
use super::wrap::{cols, truncate_cols};
use super::*;

/// A dim two-space-inset placeholder row (loading / empty / error) in the
/// picker's list area, truncated to `width`.
pub(super) fn model_placeholder_row(text: &str, color: Color, width: u16) -> Line<'static> {
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT));
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(truncate_cols(text, room), Style::new().fg(color)),
    ])
}

/// One model row: `{marker}{id} [{provider}]{✓}` — the selected row's marker and
/// id light up cyan (the palette accent), the `[provider]` tag is dim, and the
/// active model carries a green ✓. The id is truncated so the tag stays visible.
fn model_row(entry: &ModelEntry, selected: bool, active: bool, width: u16) -> Line<'static> {
    let marker = if selected { MODEL_MARKER } else { "  " };
    let tag = format!(" [{}]", entry.provider);
    let active_mark = if active { MODEL_ACTIVE_MARK } else { "" };
    let reserved = cols(marker) + cols(&tag) + cols(active_mark);
    let id_room = (width as usize).saturating_sub(reserved).max(1);
    let id = truncate_cols(&entry.id, id_room);

    let (marker_style, id_style) = if selected {
        (
            Style::new().fg(MODEL_SELECTED_COLOR),
            Style::new()
                .fg(MODEL_SELECTED_COLOR)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (Style::default(), Style::new().fg(MODEL_ID_COLOR))
    };
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(id, id_style),
        Span::styled(tag, Style::new().fg(MODEL_META_COLOR)),
        Span::styled(active_mark.to_string(), Style::new().fg(MODEL_ACTIVE_COLOR)),
    ])
}

/// The picker's list lines: a single placeholder while loading / errored /
/// empty, else the model rows windowed ([`centered_window`]) to keep the
/// selection **centered** and capped at [`MODEL_MENU_MAX_ROWS`].
fn model_list_lines(picker: &ModelPicker, width: u16) -> Vec<Line<'static>> {
    match &picker.status {
        ModelLoad::Loading => vec![model_placeholder_row(
            MODEL_LOADING,
            MODEL_META_COLOR,
            width,
        )],
        // Every provider failed: one red row each (`{provider}: {reason}`), or a
        // single legacy message when there are no per-provider errors.
        ModelLoad::Error(msg) => {
            if picker.errors.is_empty() {
                vec![model_placeholder_row(
                    &format!("Error: {msg}"),
                    ERROR_COLOR,
                    width,
                )]
            } else {
                picker
                    .errors
                    .iter()
                    .map(|e| {
                        model_placeholder_row(
                            &format!("{}: {}", e.provider, e.message),
                            ERROR_COLOR,
                            width,
                        )
                    })
                    .collect()
            }
        }
        // No key configured yet — an inviting cyan hint, not a red error.
        ModelLoad::NeedsLogin => vec![model_placeholder_row(
            MODEL_LOGIN_HINT,
            MODEL_SELECTED_COLOR,
            width,
        )],
        ModelLoad::Ready => {
            let matches = picker.matches();
            if matches.is_empty() {
                let text = if picker.models.is_empty() {
                    MODEL_NONE
                } else {
                    MODEL_NO_MATCH
                };
                return vec![model_placeholder_row(text, MODEL_META_COLOR, width)];
            }
            let max = MODEL_MENU_MAX_ROWS as usize;
            let selected = picker.selected.min(matches.len() - 1);
            let offset = centered_window(matches.len(), selected, max);
            matches
                .iter()
                .enumerate()
                .skip(offset)
                .take(max)
                .map(|(i, m)| model_row(m, i == selected, picker.is_active(m), width))
                .collect()
        }
    }
}

/// The `(selected+1/total)` counter line under the list, or a blank line when
/// there's nothing selectable (loading / error / empty).
fn model_counter_line(picker: &ModelPicker) -> Line<'static> {
    if picker.status != ModelLoad::Ready {
        return Line::default();
    }
    let matches = picker.matches();
    if matches.is_empty() {
        return Line::default();
    }
    let selected = picker.selected.min(matches.len() - 1);
    let mut spans = vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("({}/{})", selected + 1, matches.len()),
            Style::new().fg(MODEL_META_COLOR),
        ),
    ];
    // Beside the counter, the multi-provider load status: dim while more
    // providers are still fetching, red when one finished but failed.
    if let Some((text, color)) = model_load_status_suffix(picker) {
        spans.push(Span::styled(
            format!("{MODEL_STATUS_SEP}{text}"),
            Style::new().fg(color),
        ));
    }
    Line::from(spans)
}

/// The trailing status shown beside the `(n/total)` counter during a
/// multi-provider load: `loading more…` (dim) while fetches are still out, then
/// a red `{provider} unavailable` / `N providers unavailable` note if any
/// failed. `None` once every provider succeeded. See `docs/llm.md`.
fn model_load_status_suffix(picker: &ModelPicker) -> Option<(String, Color)> {
    if picker.pending > 0 {
        Some((MODEL_LOADING_MORE.to_string(), MODEL_META_COLOR))
    } else if picker.errors.len() == 1 {
        Some((
            format!("{} unavailable", picker.errors[0].provider),
            ERROR_COLOR,
        ))
    } else if !picker.errors.is_empty() {
        Some((
            format!("{} providers unavailable", picker.errors.len()),
            ERROR_COLOR,
        ))
    } else {
        None
    }
}

/// The `Model Name: {friendly}` line under the counter, naming the highlighted
/// model, or a blank line when nothing is highlighted.
fn model_name_line(picker: &ModelPicker, width: u16) -> Line<'static> {
    let Some(entry) = picker.highlighted() else {
        return Line::default();
    };
    let room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + cols(MODEL_NAME_LABEL))
        .max(1);
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_NAME_LABEL, Style::new().fg(MODEL_META_COLOR)),
        Span::styled(
            truncate_cols(&entry.display_name, room),
            Style::new().fg(MODEL_META_COLOR),
        ),
    ])
}

/// A full-width `─` rule in the box's border colour (the picker's top/bottom
/// frame, matching the input box's rules).
pub(super) fn model_rule(width: u16) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width as usize),
        Style::new().fg(BORDER_COLOR),
    ))
}

/// The whole framed page as lines — the shape of the user's mock: a top rule,
/// the `❯` search line, the scrolling model list (each row `→ id [provider]
/// ✓`), and — when a real model is highlighted — the `(n/total)` counter and
/// the `Model Name:` line above the bottom rule (a placeholder page collapses
/// them to a single gap). Headerless (the "Showing models…" banner was
/// dropped). What [`render_model_picker`] paints (bottom-anchored) and
/// `layout::model_picker_rows` counts, so the reserved height and the painted
/// rows can never disagree (`docs/view-flow.md`).
pub(super) fn model_view_lines(picker: &ModelPicker, width: u16) -> Vec<Line<'static>> {
    let search_line = Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(MODEL_SELECTED_COLOR)),
        Span::raw(picker.query.clone()),
    ]);
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        search_line,
        Line::default(),
    ];
    lines.extend(model_list_lines(picker, width));
    if model_has_detail(picker) {
        lines.push(model_counter_line(picker));
        lines.push(Line::default());
        lines.push(model_name_line(picker, width));
        lines.push(Line::default());
    } else {
        // A placeholder (loading / error / needs-login / no match): the blank
        // counter + name collapse to a single gap above the bottom rule.
        lines.push(Line::default());
    }
    lines.push(model_rule(width));
    lines
}

/// Render the **inline** `/model` picker into the live region, in place of the
/// composer — bottom-anchored, so a squeezed area keeps the list and closing
/// chrome on screen while the skipped top flows into scrollback
/// (`docs/view-flow.md`). Pure — `render_live` paints this. See `docs/llm.md`.
pub fn render_model_picker(area: Rect, buf: &mut Buffer, picker: &ModelPicker) {
    super::view_flow::render_framed_tail(area, buf, model_view_lines(picker, area.width));
}
