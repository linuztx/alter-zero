//! The read-only `/hooks` menu. See `docs/hooks-menu.md`.
//!
//! The ↓ manager band's pattern rather than the `/settings` fixed-slot
//! layout: [`hooks_view_lines`] builds the whole framed body line by line —
//! multi-line event descriptions and the detail page's wrapped command box
//! make the height content-driven, so it falls out as `lines.len()` and
//! [`hooks_menu_height`] / [`render_hooks_menu`] can never disagree about it.

use super::model_view::model_rule;
use super::theme::*;
use super::wrap::{cols, truncate_cols, wrap_output};
use super::*;

use crate::app::{HooksLevel, HooksMenu};
use crate::hooks::{EventOverview, event_description, event_has_matchers, event_summary};

/// A `MODEL_INDENT`-inset single line in `style`, truncated to the width.
fn hooks_line(text: &str, style: Style, width: u16) -> Line<'static> {
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT)).max(1);
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(truncate_cols(text, room), style),
    ])
}

/// A dim inset line.
fn dim_line(text: &str, width: u16) -> Line<'static> {
    hooks_line(text, Style::new().fg(MODEL_META_COLOR), width)
}

/// A level's title — `Hooks`, `PreToolUse - Matchers`, `Hook details` — in
/// the band-title dress.
fn title_line(text: &str, width: u16) -> Line<'static> {
    hooks_line(
        text,
        Style::new().fg(AI_COLOR).add_modifier(Modifier::BOLD),
        width,
    )
}

/// `text` word-wrapped to inset dim rows — the read-only banner, an event
/// description's lines, the detail page's closing note.
fn dim_wrapped(text: &str, width: u16) -> Vec<Line<'static>> {
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT)).max(1) as u16;
    text.split('\n')
        .flat_map(|part| wrap_text(part, room))
        .map(|row| {
            Line::from(vec![
                Span::raw(MODEL_INDENT),
                Span::styled(row, Style::new().fg(MODEL_META_COLOR)),
            ])
        })
        .collect()
}

/// `text` cut to `max` columns with a trailing `…` when anything was cut.
fn ellipsize(text: &str, max: usize) -> String {
    if cols(text) <= max {
        text.to_string()
    } else if max == 0 {
        String::new()
    } else {
        format!("{}…", truncate_cols(text, max - 1))
    }
}

/// One list level's row: the label spans (styled by the caller) and the dim
/// description that rides the aligned column.
struct MenuRow {
    label: Vec<Span<'static>>,
    label_cols: usize,
    desc: String,
}

/// The numbered, windowed list shared by the three list levels: the
/// selection-centered [`HOOKS_MENU_MAX_ROWS`] window, `❯` on the selected
/// row, `↑`/`↓` overflow markers on the window's edge rows, absolute `{n}.`
/// numbers left-padded to the widest number's column, and the descriptions
/// aligned past the widest visible label.
fn list_lines(rows: &[MenuRow], selected: usize, width: u16) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return Vec::new();
    }
    let selected = selected.min(rows.len() - 1);
    let offset = centered_window(rows.len(), selected, HOOKS_MENU_MAX_ROWS);
    let visible = &rows[offset..(offset + HOOKS_MENU_MAX_ROWS).min(rows.len())];
    // `{n}.` left-aligned in the widest number's column plus one space — a
    // single digit reads `1.  ` beside `10. `, the reference's spacing.
    let num_width = format!("{}.", rows.len()).len();
    let fixed = cols(MODEL_INDENT) + cols(HOOKS_MARKER) + num_width + 1;
    // The label column: the widest visible label, capped so the widest
    // description always keeps its seat at the row's end.
    let desc_width = visible.iter().map(|r| cols(&r.desc)).max().unwrap_or(0);
    let label_room = (width as usize)
        .saturating_sub(fixed + HOOKS_DESC_GAP + desc_width)
        .max(1);
    let label_col = visible
        .iter()
        .map(|r| r.label_cols)
        .max()
        .unwrap_or(0)
        .min(label_room);

    visible
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let index = offset + i;
            let (marker, marker_style) = if index == selected {
                (HOOKS_MARKER, Style::new().fg(MODEL_SELECTED_COLOR))
            } else if i == 0 && offset > 0 {
                (HOOKS_UP_MARKER, Style::new().fg(MODEL_META_COLOR))
            } else if i == visible.len() - 1 && offset + HOOKS_MENU_MAX_ROWS < rows.len() {
                (HOOKS_DOWN_MARKER, Style::new().fg(MODEL_META_COLOR))
            } else {
                ("  ", Style::default())
            };
            let number_style = if index == selected {
                Style::new().fg(MODEL_SELECTED_COLOR)
            } else {
                Style::new().fg(MODEL_META_COLOR)
            };
            let mut spans = vec![
                Span::raw(MODEL_INDENT),
                Span::styled(marker.to_string(), marker_style),
                Span::styled(
                    format!("{:<num_width$} ", format!("{}.", index + 1)),
                    number_style,
                ),
            ];
            // The label spans, elided as one run so the description column
            // never falls off the row.
            let mut used = 0;
            for span in &row.label {
                let room = label_col.saturating_sub(used);
                if room == 0 {
                    break;
                }
                let text = ellipsize(&span.content, room);
                used += cols(&text);
                spans.push(Span::styled(text, span.style));
            }
            spans.push(Span::raw(
                " ".repeat(label_col.saturating_sub(used) + HOOKS_DESC_GAP),
            ));
            spans.push(Span::styled(
                row.desc.clone(),
                Style::new().fg(MODEL_META_COLOR),
            ));
            Line::from(spans)
        })
        .collect()
}

/// A label span in the row's resting or selected dress.
fn label_span(text: String, selected: bool) -> Span<'static> {
    if selected {
        Span::styled(
            text,
            Style::new()
                .fg(MODEL_SELECTED_COLOR)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(text, Style::new().fg(MODEL_ID_COLOR))
    }
}

/// `{n} hook(s)` — the matcher rows' description and the count line's noun.
fn hook_noun(n: usize) -> String {
    if n == 1 {
        format!("{n} hook")
    } else {
        format!("{n} hooks")
    }
}

/// Level 1: `Hooks` over the count, the read-only banner (and the disabled
/// note when the configured hooks are off), and the eleven event rows.
fn events_lines(menu: &HooksMenu, selected: usize, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        title_line(HOOKS_TITLE, width),
        dim_line(
            &format!("{} configured", hook_noun(menu.overview.total())),
            width,
        ),
        Line::default(),
    ];
    if !menu.enabled && menu.overview.total() > 0 {
        lines.push(hooks_line(
            HOOKS_DISABLED_NOTE,
            Style::new().fg(ERROR_COLOR),
            width,
        ));
        lines.push(Line::default());
    }
    lines.extend(dim_wrapped(HOOKS_INFO, width));
    lines.push(Line::default());
    let rows: Vec<MenuRow> = menu
        .overview
        .events
        .iter()
        .enumerate()
        .map(|(i, event)| {
            let name = event.event.name().to_string();
            let name_cols = cols(&name);
            let mut label = vec![label_span(name, i == selected)];
            let mut label_cols = name_cols;
            if event.count() > 0 {
                let count = format!(" ({})", event.count());
                label_cols += cols(&count);
                // The count keeps the accent at rest (the reference colours
                // it "suggestion"); selected, the whole label is the accent.
                label.push(Span::styled(count, Style::new().fg(MODEL_SELECTED_COLOR)));
            }
            MenuRow {
                label,
                label_cols,
                desc: event_summary(event.event).to_string(),
            }
        })
        .collect();
    lines.extend(list_lines(&rows, selected, width));
    lines.extend([
        Line::default(),
        dim_line(HOOKS_HINT, width),
        Line::default(),
        model_rule(width),
    ]);
    lines
}

/// The two dim empty-state lines — an event with nothing configured.
fn empty_state_lines(width: u16) -> Vec<Line<'static>> {
    vec![
        dim_line(HOOKS_EMPTY, width),
        Line::default(),
        dim_line(HOOKS_EMPTY_HINT, width),
    ]
}

/// Levels 2 and 3 share this frame: a title, the event's description, then
/// the rows (or the empty state) over the key hint.
fn described_level_lines(
    title: &str,
    description: &str,
    rows: &[MenuRow],
    selected: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![model_rule(width), Line::default(), title_line(title, width)];
    lines.extend(dim_wrapped(description, width));
    lines.push(Line::default());
    let hint = if rows.is_empty() {
        lines.extend(empty_state_lines(width));
        HOOKS_DETAIL_HINT
    } else {
        lines.extend(list_lines(rows, selected, width));
        HOOKS_HINT
    };
    lines.extend([
        Line::default(),
        dim_line(hint, width),
        Line::default(),
        model_rule(width),
    ]);
    lines
}

/// Level 2: one event's matcher rows — `[User] {matcher}` over `{n} hook(s)`.
fn matchers_lines(event: &EventOverview, selected: usize, width: u16) -> Vec<Line<'static>> {
    let rows: Vec<MenuRow> = event
        .matchers
        .iter()
        .enumerate()
        .map(|(i, matcher)| {
            let text = format!("[{HOOKS_SOURCE_INLINE}] {}", matcher.label());
            let label_cols = cols(&text);
            MenuRow {
                label: vec![label_span(text, i == selected)],
                label_cols,
                desc: hook_noun(matcher.hooks.len()),
            }
        })
        .collect();
    described_level_lines(
        &format!("{} - Matchers", event.event.name()),
        event_description(event.event),
        &rows,
        selected,
        width,
    )
}

/// Level 3: the handlers under one matcher row (or the whole event for a
/// matcher-less one) — `[{type}] {command}` over the source header.
fn hook_list_lines(
    event: &EventOverview,
    matcher: Option<usize>,
    selected: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let title = match matcher {
        Some(i) => format!(
            "{} - Matcher: {}",
            event.event.name(),
            event
                .matchers
                .get(i)
                .map_or(crate::hooks::MATCHER_ALL_LABEL, |m| m.label())
        ),
        // A matcher-less event descended straight from its row — no matcher
        // to name (the reference titles these bare).
        None => event.event.name().to_string(),
    };
    let rows: Vec<MenuRow> = event
        .hooks_at(matcher)
        .iter()
        .enumerate()
        .map(|(i, hook)| {
            let text = format!("[{}] {}", hook.kind, hook.display());
            let label_cols = cols(&text);
            MenuRow {
                label: vec![label_span(text, i == selected)],
                label_cols,
                desc: HOOKS_SOURCE_HEADER.to_string(),
            }
        })
        .collect();
    described_level_lines(
        &title,
        event_description(event.event),
        &rows,
        selected,
        width,
    )
}

/// One `{label:<10}{value}` field row of the detail page.
fn field_line(label: &str, value: &str, value_style: Style, width: u16) -> Line<'static> {
    let room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + HOOKS_FIELD_COL)
        .max(1);
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("{label:<HOOKS_FIELD_COL$}"),
            Style::new().fg(MODEL_META_COLOR),
        ),
        Span::styled(truncate_cols(value, room), value_style),
    ])
}

/// Level 4: one handler's read-only details — the field block, the `Command:`
/// label over the rounded box holding the **real** command word-wrapped, the
/// status message when one is set, and the closing direction.
fn detail_lines(
    menu: &HooksMenu,
    event: &EventOverview,
    matcher: Option<usize>,
    hook: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(hook) = event.hooks_at(matcher).into_iter().nth(hook) else {
        // Defensive — the state machine only builds live indices.
        return vec![model_rule(width), Line::default(), model_rule(width)];
    };
    let hook = hook.clone();
    let value = Style::new().fg(AI_COLOR);
    let dim = Style::new().fg(MODEL_META_COLOR);
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        title_line(HOOKS_DETAIL_TITLE, width),
        Line::default(),
        field_line("Event:", event.event.name(), value, width),
    ];
    // A matcher the engine ignores is not presented as if it filtered — the
    // matcher-less events skip the row (the reference's eventSupportsMatcher).
    if event_has_matchers(event.event) {
        let label = matcher
            .and_then(|i| event.matchers.get(i))
            .map_or(crate::hooks::MATCHER_ALL_LABEL, |m| m.label());
        lines.push(field_line("Matcher:", label, value, width));
    }
    lines.push(field_line("Type:", &hook.kind, value, width));
    let source = menu.source.as_ref().map_or_else(
        || HOOKS_SOURCE_LABEL.to_string(),
        |path| format!("{HOOKS_SOURCE_LABEL} ({path})"),
    );
    lines.push(field_line("Source:", &source, dim, width));
    lines.push(Line::default());
    lines.push(dim_line("Command:", width));

    // The rounded box: dim borders, one padding column, the real command
    // word-wrapped inside (`wrap_output` — spaces preserved, nothing cut).
    let box_width = (width as usize)
        .saturating_sub(2 * cols(MODEL_INDENT))
        .max(4);
    let inner = box_width - 4;
    let border = Style::new().fg(BORDER_COLOR);
    let horizontal = "─".repeat(box_width - 2);
    lines.push(Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(format!("╭{horizontal}╮"), border),
    ]));
    let content = if hook.content.is_empty() {
        crate::hooks::HOOK_NO_CONTENT_LABEL.to_string()
    } else {
        hook.content.clone()
    };
    for row in wrap_output(&content, inner as u16) {
        let pad = inner.saturating_sub(cols(&row));
        lines.push(Line::from(vec![
            Span::raw(MODEL_INDENT),
            Span::styled("│ ".to_string(), border),
            Span::styled(row, value),
            Span::raw(" ".repeat(pad)),
            Span::styled(" │".to_string(), border),
        ]));
    }
    lines.push(Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(format!("╰{horizontal}╯"), border),
    ]));

    if let Some(message) = &hook.status_message {
        lines.push(Line::default());
        lines.push(dim_line(&format!("Status message: {message}"), width));
    }
    lines.push(Line::default());
    lines.extend(dim_wrapped(HOOKS_MODIFY_NOTE, width));
    lines.extend([
        Line::default(),
        dim_line(HOOKS_DETAIL_HINT, width),
        Line::default(),
        model_rule(width),
    ]);
    lines
}

/// The whole framed body for the menu's current level — empty when the menu
/// is closed. What [`render_hooks_menu`] paints and [`hooks_menu_height`]
/// counts.
#[must_use]
pub fn hooks_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(menu) = &app.hooks_menu else {
        return Vec::new();
    };
    let event_at = |i: usize| menu.overview.events.get(i);
    match menu.level {
        HooksLevel::Events { selected } => events_lines(menu, selected, width),
        HooksLevel::Matchers { event, selected } => event_at(event)
            .map(|e| matchers_lines(e, selected, width))
            .unwrap_or_default(),
        HooksLevel::Hooks {
            event,
            matcher,
            selected,
        } => event_at(event)
            .map(|e| hook_list_lines(e, matcher, selected, width))
            .unwrap_or_default(),
        HooksLevel::Detail {
            event,
            matcher,
            hook,
        } => event_at(event)
            .map(|e| detail_lines(menu, e, matcher, hook, width))
            .unwrap_or_default(),
    }
}

/// The inline live-region height when the `/hooks` menu is open, or `None`
/// when it isn't (the caller then falls back to `live_height`). Like the
/// `/model` picker the menu **replaces** the composer — and only the
/// composer: the streaming strip keeps its rows above it
/// (`layout::strip_above_rows`), so opening `/hooks` mid-turn never hides the
/// running turn. The body is the built line count **at the terminal's real
/// width** (descriptions wrap, the command box grows), the sum clamped to the
/// terminal — the ↓ manager band's rule. Shared by `tui::view`'s
/// `live_region_height`, `render_live`, and `cursor_position`.
#[must_use]
pub fn hooks_menu_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.hooks_menu.as_ref()?;
    let body = u16::try_from(hooks_view_lines(app, width).len()).unwrap_or(u16::MAX);
    Some(super::layout::view_height(app, width, body, term_height))
}

/// Render the **inline** `/hooks` menu into the live region, in place of the
/// composer. Pure — `render_live` paints this. See `docs/hooks-menu.md`.
pub fn render_hooks_menu(area: Rect, buf: &mut Buffer, app: &App) {
    Paragraph::new(hooks_view_lines(app, area.width)).render(area, buf);
}
