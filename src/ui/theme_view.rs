//! The inline `/theme` picker. See `docs/theme.md`.
//!
//! codex's theme picker in this TUI's dress. The family's frame and `❯`
//! search line sit over one row per colour theme in the **`/mcp` list's
//! shape** — the accent `❯` on the selection, the other rows indented under
//! it, a ten-row window whose hidden rows are named by dim `↑ N more above`
//! / `↓ N more below` rows — and **every row wears a swatch of its own
//! palette** (five `●`s in that theme's accent, link, success, warning and
//! error colours), so the catalog compares at a glance.
//!
//! The highlighted theme previews on **real cells**: a user bubble, an
//! `Edit` diff cell and an assistant reply with a code block, built by
//! [`message_lines`] and [`tool_lines`] — the conversation's own builders —
//! with the highlighted theme scoped active around them
//! ([`with_theme`](super::palette::with_theme)), under a caption naming the
//! theme. Once the terminal has room for the list, a gap and the sample
//! unwrapped, the preview sits **beside** the list (codex's side-by-side
//! picker); a narrower terminal stacks it under the list. The list column is
//! sized off the whole catalog, so neither ↑/↓ nor a search moves the
//! preview sideways; beside the list the overflow rows live in the list
//! column, so the preview's rows never move either; and over this catalog
//! one side of the window is always hidden, so the frame holds still as ↑/↓
//! walk it. The page is still (nothing on it ticks), so its flow is signed
//! on its rows like the `/mascot` picker's.

use super::model_view::{model_placeholder_row, model_rule};
use super::palette::{palette_of, with_theme};
use super::theme::*;
use super::wrap::{clamp_spans, cols, ellipsize};
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

/// The column the swatches start at, past the indent and the marker: the
/// widest name in the **whole catalog** plus [`THEME_MENU_GAP`] — never just
/// the visible rows', so a search or a scroll moves neither the swatches nor
/// (beside the list) the preview.
fn name_col() -> usize {
    Theme::ALL
        .iter()
        .map(|theme| cols(theme.name()))
        .max()
        .unwrap_or(0)
        + THEME_MENU_GAP
}

/// One theme row in the `/mcp` list's shape:
/// `{indent}{❯ }{name}{pad}{swatches}{ ✓}` — the selected row's [`HOOKS_MARKER`]
/// and name in the active accent (the name bold), every other row keeping the
/// marker's width blank so the names line up under it; the swatches wear the
/// row's own palette, and the session's current theme carries the `/model`
/// picker's green ✓.
fn theme_row(row: &ThemeRow, selected: bool) -> Line<'static> {
    let (marker, marker_style, name_style) = if selected {
        (
            HOOKS_MARKER.to_string(),
            Style::new().fg(model_selected_color()),
            Style::new()
                .fg(model_selected_color())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            " ".repeat(cols(HOOKS_MARKER)),
            Style::default(),
            Style::new().fg(model_id_color()),
        )
    };
    let name = row.theme.name();
    let mut spans = vec![
        Span::raw(MODEL_INDENT),
        Span::styled(marker, marker_style),
        Span::styled(name.to_string(), name_style),
        Span::raw(" ".repeat(name_col().saturating_sub(cols(name)))),
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

/// The list column's width: the widest row the catalog can paint (a
/// selected row wearing the ✓). A constant of the catalog, so the preview
/// beside it starts on one column whatever is selected or searched.
fn list_cols() -> usize {
    Theme::ALL
        .into_iter()
        .map(|theme| {
            let row = theme_row(
                &ThemeRow {
                    theme,
                    active: true,
                },
                true,
            );
            row.spans.iter().map(|s| cols(&s.content)).sum::<usize>()
        })
        .max()
        .unwrap_or(0)
}

/// Whether the preview sits **beside** the list at `width`: the list
/// column, [`THEME_PREVIEW_GAP`] and the sample's unwrapped width
/// ([`THEME_PREVIEW_MIN_COLS`]) all fit. Under that it stacks under the
/// list.
fn preview_beside(width: u16) -> bool {
    usize::from(width) >= list_cols() + THEME_PREVIEW_GAP + THEME_PREVIEW_MIN_COLS
}

/// The `/mcp` list's dim overflow row — `↑ 3 more above`, `↓ 6 more below`
/// — `…`-cut to `width`.
fn overflow_row(arrow: &str, hidden: usize, direction: &str, width: u16) -> Line<'static> {
    model_placeholder_row(
        &format!("{arrow}{hidden} more {direction}"),
        model_meta_color(),
        width,
    )
}

/// The list column in the `/mcp` list's shape: a `↑ N more above` row only
/// when rows are scrolled off the top, the window of rows
/// ([`centered_window`] at [`THEME_MENU_MAX_ROWS`], the selection riding the
/// middle), and a `↓ N more below` row only when rows are below. Over this
/// catalog one side is always hidden, so the column is one height at every
/// selection. The overflow rows are `…`-cut to `overflow_width`, the rows to
/// `width`.
fn list_lines(
    rows: &[ThemeRow],
    selected: usize,
    width: u16,
    overflow_width: u16,
) -> Vec<Line<'static>> {
    let max = usize::from(THEME_MENU_MAX_ROWS);
    let offset = centered_window(rows.len(), selected, max);
    let end = (offset + max).min(rows.len());
    let mut lines = Vec::new();
    if offset > 0 {
        lines.push(overflow_row(
            HOOKS_UP_MARKER,
            offset,
            "above",
            overflow_width,
        ));
    }
    lines.extend(rows[offset..end].iter().enumerate().map(|(i, row)| {
        clamp_spans(
            theme_row(row, offset + i == selected).spans,
            usize::from(width),
        )
    }));
    if end < rows.len() {
        lines.push(overflow_row(
            HOOKS_DOWN_MARKER,
            rows.len() - end,
            "below",
            overflow_width,
        ));
    }
    lines
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
        position: None,
    }
}

/// `line` with its own style folded onto its spans (the way ratatui patches
/// it at paint time), and the line itself unstyled — so a row can be laid
/// beside, or inset behind, other content without its ground (the user
/// bubble's) painting that content too.
fn fold_line_style(line: Line<'static>) -> Line<'static> {
    let style = line.style;
    Line::from(
        line.spans
            .into_iter()
            .map(|span| Span::styled(span.content, style.patch(span.style)))
            .collect::<Vec<_>>(),
    )
}

/// What a theme's description says past its title, as a subtitle —
/// `Catppuccin Mocha — the darkest flavour, and the default` reads `The
/// darkest flavour, and the default`: the title and its dash dropped (the
/// caption's first row shows the title) and the first letter capitalised. A
/// description that doesn't open with its title is shown whole.
fn subtitle(theme: Theme) -> String {
    let description = theme.description();
    let rest = description
        .strip_prefix(theme.title())
        .and_then(|rest| rest.strip_prefix(" — "))
        .unwrap_or(description);
    let mut chars = rest.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The caption over the preview — codex's bold-title-over-dim-subtitle
/// header, naming the theme the sample wears: its title in bold, and under
/// it its [`subtitle`] in the frame's dim. Two rows rather than one
/// `Title — description` row, so the caption reads whole in the preview
/// column of a standard terminal; each row is still `…`-cut to `width`
/// rather than wrapped, so the preview is one shape for every theme. Like
/// the rest of the frame it wears the **active** theme.
fn caption_lines(theme: Theme, width: usize) -> [Line<'static>; 2] {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let dim = Style::new().fg(model_meta_color());
    [
        Line::from(Span::styled(ellipsize(theme.title(), width), bold)),
        Line::from(Span::styled(ellipsize(&subtitle(theme), width), dim)),
    ]
}

/// The preview for `theme` at `width`: the caption, a gap, then the sample
/// conversation's three cells rendered under `theme` by the conversation's
/// own builders. The scope ends before the surrounding page is built, so the
/// frame, the rows, the caption and the hint keep the **active** theme —
/// only the sample wears the highlighted one.
fn preview_lines(theme: Theme, width: u16) -> Vec<Line<'static>> {
    let width = width.max(1);
    let cells = with_theme(theme, || {
        let mut lines = message_lines(Role::User, THEME_PREVIEW_USER, width);
        lines.extend(tool_lines(&sample_edit(), width, &PathDisplay::VERBATIM));
        lines.extend(message_lines(Role::Assistant, THEME_PREVIEW_REPLY, width));
        lines
    });
    let mut lines = Vec::from(caption_lines(theme, usize::from(width)));
    lines.push(Line::default());
    lines.extend(cells.into_iter().map(fold_line_style));
    lines
}

/// Lay `right` beside `left`, row by row: each left row padded out to
/// `left_cols` columns, `gap` blank columns, then the right row. A column
/// that runs out leaves its side of the row blank, and a row with nothing on
/// its right carries no trailing padding.
fn beside(
    left: Vec<Line<'static>>,
    left_cols: usize,
    gap: usize,
    right: Vec<Line<'static>>,
) -> Vec<Line<'static>> {
    let rows = left.len().max(right.len());
    let mut left = left.into_iter();
    let mut right = right.into_iter();
    (0..rows)
        .map(|_| {
            let mut spans = left.next().map(|line| line.spans).unwrap_or_default();
            let right = right.next().map(|line| line.spans).unwrap_or_default();
            if right.iter().any(|s| !s.content.is_empty()) {
                let used: usize = spans.iter().map(|s| cols(&s.content)).sum();
                spans.push(Span::raw(" ".repeat(left_cols.saturating_sub(used) + gap)));
                spans.extend(right);
            }
            Line::from(spans)
        })
        .collect()
}

/// The whole framed page as lines: a top rule, the `❯` search line, the
/// list column ([`list_lines`] — the windowed theme rows, each wearing its
/// own swatch, between their `↑`/`↓` overflow rows), the **real-cell
/// preview** in the highlighted theme under its caption — beside the list
/// when the terminal has the room ([`preview_beside`]), under it after a gap
/// when it doesn't — the key hint, and a bottom rule, blank-gapped like its
/// family. What [`render_theme_picker`] paints (bottom-anchored) and
/// [`theme_menu_rows`] counts, so the reserved height and the painted rows
/// can never disagree (`docs/view-flow.md`). Empty when the picker is closed.
///
/// A search that matched **nothing** has no list and no preview, so the
/// page collapses to the placeholder between single gaps — the `/mascot`
/// picker's rule.
pub(super) fn theme_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(picker) = app.theme_picker.as_ref() else {
        return Vec::new();
    };
    let rows = app.theme_rows();
    let search_line = Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(model_selected_color())),
        Span::raw(picker.query.clone()),
    ]);
    let hint = model_placeholder_row(THEME_HINT, model_meta_color(), width);
    let mut lines = vec![model_rule(width), Line::default(), search_line];
    if rows.is_empty() {
        lines.extend([
            Line::default(),
            model_placeholder_row(THEME_NO_MATCH, model_meta_color(), width),
            Line::default(),
            hint,
            Line::default(),
            model_rule(width),
        ]);
        return lines;
    }
    let selected = picker.selected.min(rows.len() - 1);
    let theme = rows[selected].theme;
    lines.push(Line::default());
    if preview_beside(width) {
        // Beside the list the overflow rows live in the list column, so the
        // preview's rows never move as the window scrolls.
        let list_width = list_cols();
        let overflow_width = u16::try_from(list_width).unwrap_or(width);
        let list = list_lines(&rows, selected, width, overflow_width);
        let room = usize::from(width) - list_width - THEME_PREVIEW_GAP;
        let preview = preview_lines(theme, u16::try_from(room).unwrap_or(width));
        lines.extend(beside(list, list_width, THEME_PREVIEW_GAP, preview));
    } else {
        lines.extend(list_lines(&rows, selected, width, width));
        lines.push(Line::default());
        let inner = width.saturating_sub(cols(MODEL_INDENT) as u16);
        lines.extend(preview_lines(theme, inner).into_iter().map(|line| {
            let mut spans = vec![Span::raw(MODEL_INDENT)];
            spans.extend(line.spans);
            Line::from(spans)
        }));
    }
    lines.push(Line::default());
    lines.push(hint);
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
/// the composer: a top rule, the `❯` search line, the theme rows in the
/// `/mcp` list's shape with their swatches (`❯ mocha       ●●●●● ✓`) and
/// overflow rows, the captioned real-cell preview in the highlighted theme —
/// beside the rows on a wide terminal, under them on a narrow one — the key
/// hint, and a bottom rule — bottom-anchored, so a squeezed area keeps the
/// hint and the closing rule on screen while the skipped top flows into
/// scrollback (`docs/view-flow.md`). Pure — `render_live` paints this. See
/// `docs/theme.md`.
pub fn render_theme_picker(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, theme_view_lines(app, area.width));
}
