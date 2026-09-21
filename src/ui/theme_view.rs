//! The inline `/theme` picker. See `docs/theme.md`.
//!
//! The `/spinner` picker's frame — same rules, same `❯` search line, same
//! `→` marker and accent selection, same `(n/total)` counter and dim hint —
//! over one row per colour theme, and two things of its own. **Every row
//! wears a swatch of its own palette** (five `●`s in that theme's accent,
//! link, success, warning and error colours), the way the spinner rows wear
//! their own spinners, so the catalog compares at a glance. And the
//! highlighted theme previews on **real cells**: a user bubble, an `Edit`
//! diff cell and an assistant reply with a code block, built by
//! [`message_lines`] and [`tool_lines`] — the conversation's own builders —
//! with the highlighted theme scoped active around them
//! ([`with_theme`](super::palette::with_theme)), so what the picker shows
//! and what Enter produces can never disagree. The page is still (nothing
//! on it ticks), so its flow is signed on its rows like the `/mascot`
//! picker's, and the sample is the same shape for every theme, so the frame
//! never moves as ↑/↓ walk the catalog.

use super::model_view::{model_placeholder_row, model_rule};
use super::palette::{palette_of, with_theme};
use super::theme::*;
use super::wrap::cols;
use super::*;

use crate::app::{Theme, ThemeRow};

/// The five swatch cells for `theme`: its accent, link, success, warning and
/// error — the roles a glance down the list should compare — each one
/// [`THEME_SWATCH`] in its own colour, read straight off that theme's palette
/// (never the active one).
fn swatch_spans(theme: Theme) -> Vec<Span<'static>> {
    let p = palette_of(theme);
    [p.accent, p.link, p.success, p.warning, p.error]
        .into_iter()
        .map(|color| Span::styled(THEME_SWATCH, Style::new().fg(color)))
        .collect()
}

/// One theme row: `{marker}{name}{pad}{swatches} {✓}` — the selected row's
/// marker and name light up in the active accent (the picker family's), the
/// name column is sized to the widest visible name plus [`THEME_MENU_GAP`],
/// the row's swatches wear its own palette, and the session's current theme
/// carries the `/model` picker's green ✓.
fn theme_row(row: &ThemeRow, selected: bool, name_col: usize) -> Line<'static> {
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
    let name = row.theme.name();
    let mut spans = vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(name.to_string(), name_style),
        Span::raw(" ".repeat(name_col.saturating_sub(cols(name)))),
    ];
    spans.extend(swatch_spans(row.theme));
    if row.active {
        spans.push(Span::styled(
            MODEL_ACTIVE_MARK.to_string(),
            Style::new().fg(model_active_color()),
        ));
    }
    Line::from(spans)
}

/// The picker's list lines: the rows windowed ([`centered_window`]) at
/// [`THEME_MENU_MAX_ROWS`], or a single placeholder when the search matches
/// nothing.
fn theme_list_lines(rows: &[ThemeRow], selected: usize, width: u16) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return vec![model_placeholder_row(
            THEME_NO_MATCH,
            model_meta_color(),
            width,
        )];
    }
    let max = THEME_MENU_MAX_ROWS as usize;
    let selected = selected.min(rows.len() - 1);
    let offset = centered_window(rows.len(), selected, max);
    let visible = rows.iter().skip(offset).take(max);
    let name_col = visible
        .clone()
        .map(|row| cols(row.theme.name()))
        .max()
        .unwrap_or(0)
        + THEME_MENU_GAP;
    rows.iter()
        .enumerate()
        .skip(offset)
        .take(max)
        .map(|(i, row)| theme_row(row, i == selected, name_col))
        .collect()
}

/// The `(selected+1/total)` counter under the list.
fn theme_counter_line(rows: &[ThemeRow], selected: usize) -> Line<'static> {
    let selected = selected.min(rows.len().saturating_sub(1));
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("({}/{})", selected + 1, rows.len()),
            Style::new().fg(model_meta_color()),
        ),
    ])
}

/// The sample `Edit` call the preview's diff cell shows: a finished call
/// whose output is the executor's own `Updated {path} (+1 -1)` report over
/// the numbered hunk ([`crate::llm::tools::update_report`] — the very string
/// a live `edit` records), so the cell parses and paints exactly as a real
/// one does.
fn sample_edit() -> ToolCall {
    ToolCall {
        name: THEME_PREVIEW_TOOL.to_string(),
        args: THEME_PREVIEW_PATH.to_string(),
        status: ToolStatus::Ok,
        output: crate::llm::tools::update_report(
            THEME_PREVIEW_PATH,
            THEME_PREVIEW_OLD,
            THEME_PREVIEW_NEW,
        ),
        timestamp: String::new(),
        shell: false,
        truncated: false,
        context_output: None,
        arguments: None,
        approval_note: None,
        batch: None,
        call_id: None,
    }
}

/// The preview for `theme`, inset two columns inside the frame: the sample
/// conversation's three cells rendered under `theme` by the conversation's
/// own builders. The scope ends before the surrounding page is built, so the
/// frame, the rows and the hint keep the **active** theme — only the sample
/// wears the highlighted one.
fn preview_lines(theme: Theme, width: u16) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(cols(MODEL_INDENT) as u16).max(1);
    let cells = with_theme(theme, || {
        let mut lines = message_lines(Role::User, THEME_PREVIEW_USER, inner);
        lines.extend(tool_lines(&sample_edit(), inner, &PathDisplay::VERBATIM));
        lines.extend(message_lines(Role::Assistant, THEME_PREVIEW_REPLY, inner));
        lines
    });
    cells
        .into_iter()
        .map(|line| {
            let style = line.style;
            let mut spans = vec![Span::raw(MODEL_INDENT)];
            spans.extend(line.spans);
            Line::from(spans).style(style)
        })
        .collect()
}

/// The whole framed page as lines: a top rule, the `❯` search line, the
/// theme rows (each wearing its own swatch), the `(n/total)` counter, the
/// **real-cell preview** in the highlighted theme, that theme's description,
/// the key hint, and a bottom rule — blank-gapped like its family. What
/// [`render_theme_picker`] paints (bottom-anchored) and [`theme_menu_rows`]
/// counts, so the reserved height and the painted rows can never disagree
/// (`docs/view-flow.md`). Empty when the picker is closed.
///
/// A search that matched **nothing** has no count, no preview and no
/// description, so those slots collapse to a single blank gap rather than
/// painting as a band of empty rows — the `/mascot` picker's rule.
pub(super) fn theme_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(picker) = app.theme_picker.as_ref() else {
        return Vec::new();
    };
    let rows = app.theme_rows();
    let selected = picker.selected;
    let search_line = Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(model_selected_color())),
        Span::raw(picker.query.clone()),
    ]);
    let highlighted = rows
        .get(selected.min(rows.len().saturating_sub(1)))
        .map(|row| row.theme);
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        search_line,
        Line::default(),
    ];
    lines.extend(theme_list_lines(&rows, selected, width));
    match highlighted {
        // A real selection: the counter, the preview and the description sit
        // between the list and the hint. The sample is the same shape for
        // every theme, so the frame holds still as ↑/↓ move.
        Some(theme) => {
            lines.push(theme_counter_line(&rows, selected));
            lines.push(Line::default());
            lines.extend(preview_lines(theme, width));
            lines.push(Line::default());
            lines.push(model_placeholder_row(
                theme.description(),
                model_meta_color(),
                width,
            ));
            lines.push(Line::default());
        }
        // Nothing matched: one gap carries the placeholder to the hint.
        None => lines.push(Line::default()),
    }
    lines.push(model_placeholder_row(THEME_HINT, model_meta_color(), width));
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// The rows the picker's own frame occupies — the built page's line count
/// ([`theme_view_lines`]). What [`theme_picker_height`] reserves under the
/// strip, and what [`render_live`] hands [`render_theme_picker`].
pub(super) fn theme_menu_rows(app: &App, width: u16) -> u16 {
    u16::try_from(theme_view_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/theme` picker is open, or `None`
/// when it isn't (the caller then falls back to [`live_height`]). Like every
/// sibling picker it **replaces** the composer — and only the composer: the
/// streaming strip keeps its rows above it, so opening `/theme` mid-turn
/// never hides the running turn. Clamped to the terminal height.
#[must_use]
pub fn theme_picker_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.theme_picker.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        theme_menu_rows(app, width),
        term_height,
    ))
}

/// Render the **inline** `/theme` picker into the live region, in place of
/// the composer: a top rule, the `❯` search line, the theme rows with their
/// swatches (`→ mocha      ●●●●● ✓`), a `(n/total)` counter, the real-cell
/// preview in the highlighted theme, its description, the key hint, and a
/// bottom rule — bottom-anchored, so a squeezed area keeps the preview, the
/// hint and the closing rule on screen while the skipped top flows into
/// scrollback (`docs/view-flow.md`). Pure — `render_live` paints this. See
/// `docs/theme.md`.
pub fn render_theme_picker(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, theme_view_lines(app, area.width));
}
