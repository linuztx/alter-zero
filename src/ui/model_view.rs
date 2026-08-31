//! The inline `/model` picker. See `docs/llm.md`.

use super::layout::model_has_detail;
use super::theme::*;
use super::wrap::{clamp_spans, cols, ellipsize};
use super::*;

/// A two-space-inset single-row line (placeholder / hint / description) in
/// the picker family's list area, `…`-cut to `width` ([`ellipsize`]) — the
/// shared single-row primitive of `/model`, `/login`, `/settings`, `/skills`
/// and `/mascot`, so a cut row always says so. Text the user must read whole
/// (errors, paths, descriptions) goes through [`model_wrapped_rows`] instead.
pub(super) fn model_placeholder_row(text: &str, color: Color, width: u16) -> Line<'static> {
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT));
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(ellipsize(text, room), Style::new().fg(color)),
    ])
}

/// `text` word-wrapped to two-space-inset rows in `color` — the multi-row
/// sibling of [`model_placeholder_row`] for informational text (an error's
/// diagnostic, an actionable hint, a path, a description): the picker pages
/// derive their height from the built line count (`docs/view-flow.md`), so
/// continuation rows are free and nothing the user must read is ever cut.
pub(super) fn model_wrapped_rows(text: &str, color: Color, width: u16) -> Vec<Line<'static>> {
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT)).max(1) as u16;
    text.split('\n')
        .flat_map(|part| super::wrap::wrap_text(part, room))
        .map(|row| {
            Line::from(vec![
                Span::raw(MODEL_INDENT),
                Span::styled(row, Style::new().fg(color)),
            ])
        })
        .collect()
}

/// [`model_wrapped_rows`] with every bare URL in `text` made **clickable**
/// (`docs/links.md`).
///
/// The link is stamped on the *unwrapped* text and the wrap carries it, which
/// is the whole point of the carrier: a URL wider than the row hard-breaks
/// across display rows, and a terminal's own detection only ever sees row
/// text — so a per-row pass would leave the second fragment opening a
/// truncated target, which is precisely the bug `links` exists to fix.
///
/// The caller's colour is **kept**, with only an underline added, rather than
/// taking the chat link dress: these pages colour their URL on purpose (the
/// device page dims it so the code box stays what the eye lands on —
/// `docs/copilot.md`), and the underline alone already says "clickable".
pub(super) fn model_linked_rows(text: &str, color: Color, width: u16) -> Vec<Line<'static>> {
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT)).max(1) as u16;
    let base = Style::new().fg(color);
    text.split('\n')
        .flat_map(|part| super::inline::wrap_inline(&linked_segments(part, base), room))
        .map(|spans| {
            let mut row = Vec::with_capacity(spans.len() + 1);
            row.push(Span::raw(MODEL_INDENT));
            row.extend(spans);
            Line::from(row)
        })
        .collect()
}

/// One line split into styled runs: the prose under `base`, each bare URL
/// underlined and carrying itself as its link target.
fn linked_segments(line: &str, base: Style) -> Vec<(String, Style)> {
    let mut out: Vec<(String, Style)> = Vec::new();
    let mut at = 0;
    for range in crate::links::find_urls(line) {
        if range.start > at {
            out.push((line[at..range.start].to_string(), base));
        }
        let url = &line[range.clone()];
        let dress = base.add_modifier(Modifier::UNDERLINED);
        out.push((url.to_string(), crate::links::linked(dress, url)));
        at = range.end;
    }
    if at < line.len() || out.is_empty() {
        out.push((line[at..].to_string(), base));
    }
    out
}

/// One model row: `{marker}{id} [{provider}]{✓}` — the selected row's marker and
/// id light up cyan (the palette accent), the `[provider]` tag is dim, and the
/// active model carries a green ✓. The id is `…`-cut so the tag stays visible
/// *and* the cut shows — two long ids clipped silently read as twins.
fn model_row(entry: &ModelEntry, selected: bool, active: bool, width: u16) -> Line<'static> {
    let marker = if selected { MODEL_MARKER } else { "  " };
    let tag = format!(" [{}]", entry.provider);
    let active_mark = if active { MODEL_ACTIVE_MARK } else { "" };
    let reserved = cols(marker) + cols(&tag) + cols(active_mark);
    let id_room = (width as usize).saturating_sub(reserved).max(1);
    let id = ellipsize(&entry.id, id_room);

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
        // Every provider failed: red rows (`{provider}: {reason}`), or a
        // single legacy message when there are no per-provider errors —
        // **wrapped**, because the error text is the only diagnostic the
        // user gets and a silent clip hid the cause at narrow widths.
        ModelLoad::Error(msg) => {
            if picker.errors.is_empty() {
                model_wrapped_rows(&format!("Error: {msg}"), ERROR_COLOR, width)
            } else {
                picker
                    .errors
                    .iter()
                    .flat_map(|e| {
                        model_wrapped_rows(
                            &format!("{}: {}", e.provider, e.message),
                            ERROR_COLOR,
                            width,
                        )
                    })
                    .collect()
            }
        }
        // No key configured yet — an inviting cyan hint, not a red error.
        // Wrapped: the actionable "run /login" tail must survive any width.
        ModelLoad::NeedsLogin => model_wrapped_rows(MODEL_LOGIN_HINT, MODEL_SELECTED_COLOR, width),
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
/// there's nothing selectable (loading / error / empty). Clamped to the width
/// with a dim `…` ([`clamp_spans`]) — the red `{provider} unavailable` suffix
/// used to paint-clip past the buffer edge, an error the user never saw.
fn model_counter_line(picker: &ModelPicker, width: u16) -> Line<'static> {
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
    clamp_spans(spans, width as usize)
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

/// Why each failed provider failed, wrapped under the list — the counter's
/// `{provider} unavailable` suffix names *which* provider and nothing else,
/// and the reason is the only thing that tells the user whether to re-run
/// `/login`, wait out a 5xx, or check their subscription. Empty when every
/// provider loaded, so a clean page is exactly what it was.
///
/// Bounded to [`MODEL_ERROR_MAX_ROWS`]: a provider that answers a blocked
/// request with an HTML page would otherwise push the list off the frame.
fn model_error_lines(picker: &ModelPicker, width: u16) -> Vec<Line<'static>> {
    let mut rows: Vec<Line<'static>> = picker
        .errors
        .iter()
        .flat_map(|e| {
            model_wrapped_rows(
                &format!("{}: {}", e.provider, e.message),
                ERROR_COLOR,
                width,
            )
        })
        .collect();
    if rows.len() > MODEL_ERROR_MAX_ROWS as usize {
        rows.truncate(MODEL_ERROR_MAX_ROWS as usize);
        // The cut has to show, or a reason clipped mid-sentence reads as a
        // reason that simply ended there. The last kept row is a *wrapped* row,
        // so it already fits the width — trimming to `width - 1` is what makes
        // room for the marker instead of leaving it off.
        if let Some(last) = rows.pop() {
            let text: String = last.spans.iter().map(|s| s.content.as_ref()).collect();
            let room = (width as usize).saturating_sub(1).max(1);
            let mut cut = super::wrap::truncate_cols(text.trim_end(), room);
            cut.push('…');
            rows.push(Line::from(Span::styled(cut, Style::new().fg(ERROR_COLOR))));
        }
    }
    rows
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
            ellipsize(&entry.display_name, room),
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
        lines.push(model_counter_line(picker, width));
        lines.push(Line::default());
        lines.push(model_name_line(picker, width));
        lines.extend(model_error_lines(picker, width));
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
