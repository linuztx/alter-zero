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
//!
//! A terminal too short for that page gets a **compact** one instead —
//! codex's short panel, where the preview yields first: the search line,
//! three rows of the list between codex's reserved `↑`/`↓` slots, and the
//! hint — and a terminal shorter still sheds that panel's own rows one by
//! one, down to the search line alone, so the page always fits and the
//! caret always has its search line to sit on. Bottom-anchoring the whole
//! page there kept only its preview on screen and flowed the theme names —
//! and then the search line — into scrollback, which made the picker
//! unusable exactly where it was squeezed.

use super::model_view::{
    PreviewLayout, capitalized, caption_lines, compact_page, first_page_that_fits, list_window,
    marked_row_head, model_placeholder_row, model_rule, overflow_list, placeholder_window,
};
use super::palette::{palette_of, with_theme};
use super::theme::*;
use super::wrap::{clamp_spans, cols, spans_cols};
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
/// `{indent}{❯ }{name}{pad}{swatches}{ ✓}` ([`marked_row_head`]) — the
/// selected row's [`HOOKS_MARKER`] and name in the active accent (the name
/// bold), every other row keeping the marker's width blank so the names line
/// up under it; the swatches wear the row's own palette, and the session's
/// current theme carries the `/model` picker's green ✓.
fn theme_row(row: &ThemeRow, selected: bool) -> Line<'static> {
    let name = row.theme.name();
    let mut spans = marked_row_head(name, selected);
    spans.push(Span::raw(" ".repeat(name_col().saturating_sub(cols(name)))));
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
            spans_cols(&row.spans)
        })
        .max()
        .unwrap_or(0)
}

/// How the page lays its preview out ([`PreviewLayout`]): beside the list
/// once the terminal has room for the list column, [`THEME_PREVIEW_GAP`]
/// and the sample's unwrapped width ([`THEME_PREVIEW_MIN_COLS`]), stacked
/// under it otherwise.
fn layout() -> PreviewLayout {
    PreviewLayout {
        list_cols: list_cols(),
        gap: THEME_PREVIEW_GAP,
        min_cols: THEME_PREVIEW_MIN_COLS,
    }
}

/// The list column in the `/mcp` list's shape ([`overflow_list`]): a
/// `↑ N more above` row only when rows are scrolled off the top, the window
/// of rows ([`centered_window`] at [`THEME_MENU_MAX_ROWS`], the selection
/// riding the middle), and a `↓ N more below` row only when rows are below.
/// Over this catalog one side is always hidden, so the column is one height
/// at every selection. The overflow rows are `…`-cut to `overflow_width`,
/// the rows to `width`.
fn list_lines(
    rows: &[ThemeRow],
    selected: usize,
    width: u16,
    overflow_width: u16,
) -> Vec<Line<'static>> {
    let (above, window, below) =
        window_lines(rows, selected, usize::from(THEME_MENU_MAX_ROWS), width);
    overflow_list(above, window, below, overflow_width)
}

/// The window of `rows` that keeps the selection in view — `max` rows,
/// [`centered_window`] riding the selection on the middle one — as how many
/// rows it hides above, its rows (the selected one marked, each `…`-cut to
/// `width`) and how many it hides below.
fn window_lines(
    rows: &[ThemeRow],
    selected: usize,
    max: usize,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = list_window(rows.len(), selected, max);
    let lines = rows[range.clone()]
        .iter()
        .zip(range)
        .map(|(row, i)| clamp_spans(theme_row(row, i == selected).spans, usize::from(width)))
        .collect();
    (above, lines, below)
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
    capitalized(rest)
}

/// The preview for `theme` at `width`: the caption ([`caption_lines`] — the
/// theme's title in bold over its [`subtitle`] in the frame's dim), a gap,
/// then the sample conversation's three cells rendered under `theme` by the
/// conversation's own builders. The scope ends before the surrounding page
/// is built, so the frame, the rows, the caption and the hint keep the
/// **active** theme — only the sample wears the highlighted one.
fn preview_lines(theme: Theme, width: u16) -> Vec<Line<'static>> {
    let width = width.max(1);
    let cells = with_theme(theme, || {
        let mut lines = message_lines(Role::User, THEME_PREVIEW_USER, width);
        lines.extend(tool_lines(&sample_edit(), width, &PathDisplay::VERBATIM));
        lines.extend(message_lines(Role::Assistant, THEME_PREVIEW_REPLY, width));
        lines
    });
    let mut lines = Vec::from(caption_lines(
        theme.title(),
        &subtitle(theme),
        usize::from(width),
    ));
    lines.push(Line::default());
    lines.extend(cells);
    lines
}

/// The whole page's body — what sits between the search line and the hint:
/// a gap, the list column ([`list_lines`]) and the **real-cell preview** in
/// the highlighted theme under its caption — beside the list when the
/// terminal has the room, under it after a gap when it doesn't
/// ([`PreviewLayout::body`]) — and the gap over the hint.
fn whole_body(rows: &[ThemeRow], selected: usize, width: u16) -> Vec<Line<'static>> {
    let theme = rows[selected].theme;
    layout().body(
        width,
        &|overflow_width| list_lines(rows, selected, width, overflow_width),
        &|preview_width| preview_lines(theme, preview_width),
    )
}

/// The framed page as lines: a top rule, the `❯` search line straight under
/// it, a body, the key hint, and a bottom rule straight under that — no
/// blank row hugs either rule (`docs/view-flow.md` *The frame hugs its
/// content*). The body is the **whole page's**
/// ([`whole_body`] — the list beside or over the captioned real-cell
/// preview) when the page fits `height` rows, else the page is a **compact**
/// one ([`compact_page`] — codex's short panel, where the preview yields
/// first: three rows of the list between the reserved `↑`/`↓` overflow
/// slots, and on a terminal shorter still the slots, the rules and the
/// rows yield one by one, never the search line), so on a terminal
/// too short for the whole page the theme names stay on screen rather than
/// flowing into scrollback above the preview. What
/// [`render_theme_picker`] paints and [`theme_menu_rows`] counts, so the
/// reserved height and the painted rows can never disagree
/// (`docs/view-flow.md`). Empty when the picker is closed.
///
/// `height` is the rows the page may take: the terminal's, since the view
/// keeps its rows and the strip above it gives way (`view_split`), or the
/// region already reserved for it — which holds the same page, because that
/// region is at least the chosen page's height and at most the terminal's.
///
/// A search that matched **nothing** has no list and no preview, so the
/// page collapses to the placeholder between single gaps — the `/mascot`
/// picker's rule — and sheds the same way, keeping what was typed.
pub(super) fn theme_view_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
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
    let page = |body: Vec<Line<'static>>| {
        let mut lines = vec![model_rule(width), search_line.clone()];
        lines.extend(body);
        lines.extend([hint.clone(), model_rule(width)]);
        lines
    };
    if rows.is_empty() {
        let block = [model_placeholder_row(
            THEME_NO_MATCH,
            model_meta_color(),
            width,
        )];
        return compact_page(
            height,
            &search_line,
            &|max| placeholder_window(&block, max),
            &[hint],
            width,
        );
    }
    let selected = picker.selected.min(rows.len() - 1);
    first_page_that_fits(
        height,
        &[&|| page(whole_body(&rows, selected, width)), &|| {
            compact_page(
                height,
                &search_line,
                &|max| window_lines(&rows, selected, max, width),
                std::slice::from_ref(&hint),
                width,
            )
        }],
    )
}

/// The rows the picker's own frame occupies on a terminal `height` rows
/// tall — the built page's line count ([`theme_view_lines`]). What
/// [`theme_picker_height`] reserves under the strip, and what
/// [`render_live`] hands [`render_theme_picker`].
pub(super) fn theme_menu_rows(app: &App, width: u16, height: u16) -> u16 {
    u16::try_from(theme_view_lines(app, width, height).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/theme` picker is open, or `None`
/// when it isn't (the caller then falls back to [`live_height`]). Like every
/// sibling picker it **replaces** the composer — and only the composer: the
/// streaming strip keeps its rows above it, so opening `/theme` mid-turn
/// never hides the running turn. The page is the compact one when the whole
/// one is taller than the terminal (`theme_view_lines`); clamped to the
/// terminal height.
#[must_use]
pub fn theme_picker_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.theme_picker.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        theme_menu_rows(app, width, term_height),
        term_height,
    ))
}

/// Render the **inline** `/theme` picker into the live region, in place of
/// the composer: a top rule, the `❯` search line, the theme rows in the
/// `/mcp` list's shape with their swatches (`❯ mocha       ●●●●● ✓`) and
/// overflow rows, the captioned real-cell preview in the highlighted theme —
/// beside the rows on a wide terminal, under them on a narrow one — the key
/// hint, and a bottom rule; or, in an area too short for that, the compact
/// page of three rows and no preview, which sheds its own rows down to the
/// search line in an area shorter still. The page is built for the area's
/// height, so it always fits and nothing flows (`docs/view-flow.md`). Pure —
/// `render_live` paints this. See `docs/theme.md`.
pub fn render_theme_picker(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, theme_view_lines(app, area.width, area.height));
}
