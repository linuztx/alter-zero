//! The inline `/theme` picker. See `docs/theme.md`.
//!
//! The `/spinner` picker's frame — same rules, same `❯` search line, same
//! `(n/total)` counter, description and dim hint — over a list that wears
//! the `/mcp` lists' rows: the accent `❯` on the highlight, the other names
//! inset to match, and a dim `↑ N more above` / `↓ N more below` row where
//! the window hides themes. Two things of its own. **Every row wears a
//! swatch of its own palette** (five `●`s in that theme's accent, link,
//! success, warning and error colours), the way the spinner rows wear their
//! own spinners, so the catalog compares at a glance. And the highlighted
//! theme previews on **real cells**: a user bubble, an `Edit` diff cell and
//! an assistant reply with a code block, built by [`message_lines`] and
//! [`tool_lines`] — the conversation's own builders — with the highlighted
//! theme scoped active around them
//! ([`with_theme`](super::palette::with_theme)), so what the picker shows
//! and what Enter produces can never disagree.
//!
//! Where the preview goes is codex's picker layout: **beside the list**
//! when the terminal can spare it a column [`THEME_PREVIEW_MIN_COLS`] wide,
//! vertically centered against the list, so the page is the list's height
//! rather than the list's plus the preview's; **under the list** on a
//! narrower terminal. The list column is sized off the whole catalog, so the
//! preview never moves sideways as ↑/↓ walk the list or a search narrows it.
//! The page is still (nothing on it ticks), so its flow is signed on its
//! rows like the `/mascot` picker's, and the sample is the same shape for
//! every theme, so the frame never moves as the selection does.

use super::model_view::{model_placeholder_row, model_rule};
use super::palette::{palette_of, with_theme};
use super::theme::*;
use super::wrap::{clamp_spans, cols};
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

/// The name column: the catalog's widest name plus [`THEME_MENU_GAP`]. Sized
/// off the whole catalog rather than the visible rows, so the swatches — and
/// the preview beside them — hold still while the window scrolls or a search
/// narrows the list.
fn name_col() -> usize {
    Theme::ALL
        .iter()
        .map(|theme| cols(theme.name()))
        .max()
        .unwrap_or(0)
        + THEME_MENU_GAP
}

/// One theme row, the `/mcp` lists' shape: `  ❯ gruvbox     ●●●●● ✓` — the
/// two-column inset, the accent `❯` on the highlight (spaces of its width
/// elsewhere), the name (accent and bold when highlighted, the muted ink
/// otherwise) padded out to `name_col`, the row's own swatches, and the
/// `/model` picker's green ✓ on the session's current theme. `…`-cut to
/// `width` on a terminal too narrow for the whole row.
fn theme_row(row: &ThemeRow, selected: bool, name_col: usize, width: u16) -> Line<'static> {
    let (marker, name_style) = if selected {
        (
            Span::styled(HOOKS_MARKER, Style::new().fg(model_selected_color())),
            Style::new()
                .fg(model_selected_color())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Span::raw(" ".repeat(cols(HOOKS_MARKER))),
            Style::new().fg(model_id_color()),
        )
    };
    let name = row.theme.name();
    let mut spans = vec![
        Span::raw(MODEL_INDENT),
        marker,
        Span::styled(name.to_string(), name_style),
        Span::raw(" ".repeat(name_col.saturating_sub(cols(name)))),
    ];
    spans.extend(swatch_spans(row.theme));
    if row.active {
        spans.push(Span::styled(
            MODEL_ACTIVE_MARK,
            Style::new().fg(model_active_color()),
        ));
    }
    clamp_spans(spans, usize::from(width))
}

/// The list column's width: a row at its widest — the active one, ✓
/// included. Every other line the column holds (an overflow row, the
/// counter) is narrower, so this is where the preview's gap starts.
fn list_cols() -> usize {
    let widest = ThemeRow {
        theme: Theme::default(),
        active: true,
    };
    theme_row(&widest, true, name_col(), u16::MAX).width()
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

/// The list column: the rows windowed ([`centered_window`]) at
/// [`THEME_MENU_MAX_ROWS`], a dim `↑ N more above` / `↓ N more below` row on
/// whichever side the window hides themes (the `/mcp` lists' overflow rows),
/// and the `(n/total)` counter — or a single placeholder when the search
/// matches nothing.
fn list_column(rows: &[ThemeRow], selected: usize, width: u16) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return vec![model_placeholder_row(
            THEME_NO_MATCH,
            model_meta_color(),
            width,
        )];
    }
    let window = usize::from(THEME_MENU_MAX_ROWS).max(1);
    let selected = selected.min(rows.len() - 1);
    let offset = centered_window(rows.len(), selected, window);
    let end = (offset + window).min(rows.len());
    let overflow = |text: String| model_placeholder_row(&text, model_meta_color(), width);
    let mut lines = Vec::new();
    if offset > 0 {
        lines.push(overflow(format!("{HOOKS_UP_MARKER}{offset} more above")));
    }
    let name_col = name_col();
    lines.extend(
        rows.iter()
            .enumerate()
            .take(end)
            .skip(offset)
            .map(|(i, row)| theme_row(row, i == selected, name_col, width)),
    );
    if end < rows.len() {
        lines.push(overflow(format!(
            "{HOOKS_DOWN_MARKER}{} more below",
            rows.len() - end
        )));
    }
    lines.push(theme_counter_line(rows, selected));
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

/// The sample conversation's three cells for `theme` at `width` columns,
/// rendered by the conversation's own builders with `theme` scoped active
/// around them. The scope ends before the surrounding page is built, so the
/// frame, the rows and the hint keep the **active** theme — only the sample
/// wears the highlighted one.
fn sample_cells(theme: Theme, width: u16) -> Vec<Line<'static>> {
    with_theme(theme, || {
        let mut lines = message_lines(Role::User, THEME_PREVIEW_USER, width);
        lines.extend(tool_lines(&sample_edit(), width, &PathDisplay::VERBATIM));
        lines.extend(message_lines(Role::Assistant, THEME_PREVIEW_REPLY, width));
        lines
    })
}

/// The preview stacked under the list: the sample's cells inset two columns
/// inside the frame, each keeping its own line style (the bubble's ground
/// runs the whole row, as in the conversation).
fn stacked_preview(theme: Theme, width: u16) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(cols(MODEL_INDENT) as u16).max(1);
    sample_cells(theme, inner)
        .into_iter()
        .map(|line| {
            let style = line.style;
            let mut spans = vec![Span::raw(MODEL_INDENT)];
            spans.extend(line.spans);
            Line::from(spans).style(style)
        })
        .collect()
}

/// `line`'s spans with the line's own style patched into each — exactly what
/// the paint would show (a line's style reaches only its spans' cells) — so
/// a cell can share its row with another column: the bubble's ground then
/// covers the bubble and stops at its first column instead of washing the
/// list row beside it.
fn flat_spans(line: Line<'static>) -> Vec<Span<'static>> {
    let style = line.style;
    line.spans
        .into_iter()
        .map(|span| Span::styled(span.content, style.patch(span.style)))
        .collect()
}

/// Where a `content`-row preview starts beside an `available`-row list:
/// centered, a [`THEME_PREVIEW_FRAME`] kept free above and below when there
/// is room for both, the odd row going below — codex's `centered_offset`.
/// Zero beside a list no taller than the preview (it top-aligns there).
fn centered_offset(available: usize, content: usize) -> usize {
    let free = available.saturating_sub(content);
    let frame = if free >= 2 * THEME_PREVIEW_FRAME {
        THEME_PREVIEW_FRAME
    } else {
        0
    };
    frame + (free - 2 * frame) / 2
}

/// The list column and the preview side by side — codex's picker layout:
/// each row is the list's line padded out to `preview_x`, then the
/// preview's, the preview vertically centered against the list
/// ([`centered_offset`]). Every row's styles are flattened onto its spans
/// ([`flat_spans`]), so neither column's line style spills onto the other.
fn side_by_side(
    list: Vec<Line<'static>>,
    preview: Vec<Line<'static>>,
    preview_x: usize,
) -> Vec<Line<'static>> {
    let offset = centered_offset(list.len(), preview.len());
    let rows = list.len().max(offset + preview.len());
    let mut list = list.into_iter();
    let mut preview = preview.into_iter();
    (0..rows)
        .map(|row| {
            let mut spans = list.next().map(flat_spans).unwrap_or_default();
            if let Some(cell) = (row >= offset).then(|| preview.next()).flatten() {
                let used: usize = spans.iter().map(|s| cols(&s.content)).sum();
                spans.push(Span::raw(" ".repeat(preview_x.saturating_sub(used))));
                spans.extend(flat_spans(cell));
            }
            Line::from(spans)
        })
        .collect()
}

/// The whole framed page as lines: a top rule, the `❯` search line, the
/// list column (the theme rows, each wearing its own swatch, between their
/// overflow rows, over the `(n/total)` counter) with the **real-cell
/// preview** in the highlighted theme beside it — or, on a terminal too
/// narrow to spare the preview [`THEME_PREVIEW_MIN_COLS`] columns, under it
/// — then that theme's description, the key hint, and a bottom rule,
/// blank-gapped like its family. What [`render_theme_picker`] paints
/// (bottom-anchored) and [`theme_menu_rows`] counts, so the reserved height
/// and the painted rows can never disagree (`docs/view-flow.md`). Empty when
/// the picker is closed.
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
    let list = list_column(&rows, selected, width);
    match highlighted {
        // A real selection: the preview beside the list, or under it, then
        // the description. The sample is the same shape for every theme, so
        // the frame holds still as ↑/↓ move.
        Some(theme) => {
            let preview_x = list_cols() + THEME_PREVIEW_GAP;
            let preview_cols = usize::from(width).saturating_sub(preview_x);
            if preview_cols >= THEME_PREVIEW_MIN_COLS {
                let cells = sample_cells(theme, u16::try_from(preview_cols).unwrap_or(u16::MAX));
                lines.extend(side_by_side(list, cells, preview_x));
            } else {
                lines.extend(list);
                lines.push(Line::default());
                lines.extend(stacked_preview(theme, width));
            }
            lines.push(Line::default());
            lines.push(model_placeholder_row(
                theme.description(),
                model_meta_color(),
                width,
            ));
            lines.push(Line::default());
        }
        // Nothing matched: one gap carries the placeholder to the hint.
        None => {
            lines.extend(list);
            lines.push(Line::default());
        }
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
/// swatches (`  ❯ mocha       ●●●●● ✓`) between their overflow rows over a
/// `(n/total)` counter, the real-cell preview in the highlighted theme
/// beside them (under them on a narrow terminal), its description, the key
/// hint, and a bottom rule — bottom-anchored, so a squeezed area keeps the
/// hint and the closing rule on screen while the skipped top flows into
/// scrollback (`docs/view-flow.md`). Pure — `render_live` paints this. See
/// `docs/theme.md`.
pub fn render_theme_picker(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, theme_view_lines(app, area.width));
}
