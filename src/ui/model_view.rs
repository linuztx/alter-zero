//! The inline `/model` picker. See `docs/llm.md`.

use super::layout::model_has_detail;
use super::theme::*;
use super::wrap::{clamp_spans, cols, ellipsize};
use super::*;

/// A two-space-inset single-row line (a hint, a note, a page's count line)
/// at the picker family's page inset, `…`-cut to `width` ([`ellipsize`]) —
/// the shared single-row primitive of the composer-replacing pages, so a cut
/// row always says so. Text the user must read whole (errors, paths,
/// descriptions) goes through [`model_wrapped_rows`] instead; a row that
/// speaks for a picker's **list** goes through [`list_placeholder_row`].
pub(super) fn model_placeholder_row(text: &str, color: Color, width: u16) -> Line<'static> {
    inset_placeholder_row(MODEL_INDENT, text, color, width)
}

/// [`model_placeholder_row`] in a search-line picker's **names' column**
/// ([`MODEL_LIST_INDENT`]): the counter under its list, the placeholder in
/// the list's place (`Loading models…`, `No matching settings`) — the rows
/// that speak for the list start where its names do.
pub(super) fn list_placeholder_row(text: &str, color: Color, width: u16) -> Line<'static> {
    inset_placeholder_row(MODEL_LIST_INDENT, text, color, width)
}

/// One `…`-cut row of `text` in `color` behind `indent`.
fn inset_placeholder_row(
    indent: &'static str,
    text: &str,
    color: Color,
    width: u16,
) -> Line<'static> {
    let room = (width as usize).saturating_sub(cols(indent));
    Line::from(vec![
        Span::raw(indent),
        Span::styled(ellipsize(text, room), Style::new().fg(color)),
    ])
}

/// A list's dim overflow row — `↑ 3 more above`, `↓ 6 more below` — the
/// `/mcp` list's, `…`-cut to `width` like any [`model_placeholder_row`].
pub(super) fn model_overflow_row(
    arrow: &str,
    hidden: usize,
    direction: &str,
    width: u16,
) -> Line<'static> {
    model_placeholder_row(
        &format!("{arrow}{hidden} more {direction}"),
        model_meta_color(),
        width,
    )
}

/// A compact page's reserved overflow slot, codex's arrow row: the
/// [`model_overflow_row`] when `hidden` rows lie that way, else a blank row —
/// so the page is one height at every selection, and a blank slot is the gap
/// the page wants there anyway. The `/theme` and `/skills` compact pages put
/// one directly under the search line and one directly over the hint.
pub(super) fn model_overflow_slot(
    arrow: &str,
    hidden: usize,
    direction: &str,
    width: u16,
) -> Line<'static> {
    if hidden == 0 {
        Line::default()
    } else {
        model_overflow_row(arrow, hidden, direction, width)
    }
}

/// How a picker lays out the highlighted row's description under its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Description {
    /// Wrapped whole — the whole page's own layout, since the description is
    /// often why the row exists.
    Wrapped,
    /// Cut to one `…`-ended row — the step a description takes before the
    /// list shrinks: a wrapped one moves the page's height with the
    /// selection, and a page deciding whole-versus-compact on that height
    /// alone flips between the two as ↑/↓ cross a long description.
    OneRow,
}

/// `text`'s rows in the picker family's dim, laid out by `layout`, in the
/// names' column ([`MODEL_LIST_INDENT`]): the highlighted row's description
/// speaks for the list, so it starts where the names do.
pub(super) fn model_description_rows(
    text: &str,
    layout: Description,
    width: u16,
) -> Vec<Line<'static>> {
    match layout {
        Description::Wrapped => list_wrapped_rows(text, model_meta_color(), width),
        Description::OneRow => {
            let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
            vec![list_placeholder_row(&flat, model_meta_color(), width)]
        }
    }
}

/// The window of a `len`-row list that keeps `selected` in view — `max`
/// rows, [`centered_window`] riding the selection on the middle one — as how
/// many rows it hides above, the visible index range, and how many it hides
/// below.
pub(super) fn list_window(
    len: usize,
    selected: usize,
    max: usize,
) -> (usize, std::ops::Range<usize>, usize) {
    let offset = centered_window(len, selected, max);
    let end = (offset + max).min(len);
    (offset, offset..end, len - end)
}

/// The window of a `len`-row list that keeps `selected` in view in a column
/// of `rows` rows **counting its overflow rows** — the previewed pickers'
/// list column (`/theme`, `/mascot`, `/spinner`, [`overflow_list`]): the
/// whole list when it fits, else `rows - 1` rows and the one `↑`/`↓` row
/// naming the side that hides any (either end of the list), else `rows - 2`
/// rows between the two (its middle). The column is then `rows` tall at
/// every selection, so the frame holds still as ↑/↓ walk it, with no blank
/// row reserved for an overflow row that has nothing to say. As
/// [`list_window`]: how many rows it hides above, the visible index range,
/// and how many below.
pub(super) fn fitted_window(
    len: usize,
    selected: usize,
    rows: usize,
) -> (usize, std::ops::Range<usize>, usize) {
    if len <= rows {
        return (0, 0..len, 0);
    }
    let one_side = list_window(len, selected, rows.saturating_sub(1).max(1));
    if one_side.0 == 0 || one_side.2 == 0 {
        return one_side;
    }
    list_window(len, selected, rows.saturating_sub(2).max(1))
}

/// A compact page's list: the reserved `↑` slot, the window's rows, the
/// reserved `↓` slot ([`model_overflow_slot`] both) — what a picker lays
/// between its search line and its hint on a terminal too short for its whole
/// page (`docs/view-flow.md`).
pub(super) fn model_compact_list(
    above: usize,
    window: Vec<Line<'static>>,
    below: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![model_overflow_slot(HOOKS_UP_MARKER, above, "above", width)];
    lines.extend(window);
    lines.push(model_overflow_slot(
        HOOKS_DOWN_MARKER,
        below,
        "below",
        width,
    ));
    lines
}

/// The head of one list row in the **`/mcp` list's shape**: the family's
/// indent, then the accent [`MODEL_MARKER`] and the name in bold accent on
/// the selected row, or the marker's width left blank and the name in the
/// muted ink on every other row, so the names line up under the marker
/// ([`MODEL_LIST_INDENT`]). Every search-line picker opens its rows with it
/// — `/model`, `/login`, `/settings`, `/skills`, `/theme`, `/mascot` and
/// `/spinner` — so the marker sits under the search line's `❯` rather than
/// in the frame's gutter, and it is the `→`, not a second `❯` a row below
/// the prompt's.
pub(super) fn marked_row_head(name: &str, selected: bool) -> Vec<Span<'static>> {
    let (marker, marker_style, name_style) = if selected {
        (
            MODEL_MARKER.to_string(),
            Style::new().fg(model_selected_color()),
            Style::new()
                .fg(model_selected_color())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            " ".repeat(cols(MODEL_MARKER)),
            Style::default(),
            Style::new().fg(model_id_color()),
        )
    };
    vec![
        Span::raw(MODEL_INDENT),
        Span::styled(marker, marker_style),
        Span::styled(name.to_string(), name_style),
    ]
}

/// A list column in the `/mcp` list's shape: a dim `↑ N more above` row over
/// `window` only when rows are hidden above it, and a dim `↓ N more below`
/// row under it only when rows are hidden below, each `…`-cut to `width`.
pub(super) fn overflow_list(
    above: usize,
    window: Vec<Line<'static>>,
    below: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if above > 0 {
        lines.push(model_overflow_row(HOOKS_UP_MARKER, above, "above", width));
    }
    lines.extend(window);
    if below > 0 {
        lines.push(model_overflow_row(HOOKS_DOWN_MARKER, below, "below", width));
    }
    lines
}

/// `text` with its first letter upper-cased: `crest` reads `Crest`.
pub(super) fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The caption over a previewed picker's preview: codex's header, a bold
/// title over a dim subtitle, naming what the preview shows. Two rows rather
/// than one `Title — subtitle` row, so the caption reads whole in the
/// preview column of a standard terminal. Each row is still `…`-cut to
/// `width` rather than wrapped, so the preview is one shape whatever is
/// highlighted.
pub(super) fn caption_lines(title: &str, subtitle: &str, width: usize) -> [Line<'static>; 2] {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let dim = Style::new().fg(model_meta_color());
    [
        Line::from(Span::styled(ellipsize(title, width), bold)),
        Line::from(Span::styled(ellipsize(subtitle, width), dim)),
    ]
}

/// How a picker that previews its highlighted entry lays the preview out.
/// This is codex's theme picker, shared by `/theme`, `/mascot` and
/// `/spinner`. The list sits on the left, and the preview sits **beside**
/// it once the terminal has room for the list column, the gap and the
/// preview's narrowest column. Otherwise the preview is **stacked** under
/// the list (`docs/theme.md`).
#[derive(Clone, Copy, Debug)]
pub(super) struct PreviewLayout {
    /// The list column's width: the widest row the catalog can paint. A
    /// constant of the catalog, so the preview beside it starts on one
    /// column whatever is selected or searched.
    pub list_cols: usize,
    /// Blank columns between the list column and the preview beside it.
    pub gap: usize,
    /// The narrowest preview column laid out beside the list: the room the
    /// preview needs to read whole.
    pub min_cols: usize,
}

impl PreviewLayout {
    /// Whether the preview sits beside the list at `width`: the list column,
    /// the gap and the preview's narrowest column all fit.
    pub fn fits_beside(self, width: u16) -> bool {
        usize::from(width) >= self.list_cols + self.gap + self.min_cols
    }

    /// The whole page's body, between the search line and the hint. It is a
    /// gap, then the list column and the preview, then the gap over the
    /// hint. `list` builds the list column and is handed the width its
    /// overflow rows are cut to; `preview` builds the preview and is handed
    /// its column's width. The two sit side by side when
    /// [`fits_beside`](Self::fits_beside), else the preview goes under the
    /// list after a gap, inset by [`MODEL_INDENT`]. Every preview row has its
    /// line style folded onto its spans first, so a ground (the user
    /// bubble's) paints its own column and never the list beside it.
    pub fn body(
        self,
        width: u16,
        list: &dyn Fn(u16) -> Vec<Line<'static>>,
        preview: &dyn Fn(u16) -> Vec<Line<'static>>,
    ) -> Vec<Line<'static>> {
        let mut lines = vec![Line::default()];
        if self.fits_beside(width) {
            // Beside the list the overflow rows live in the list column, so
            // the preview's rows never move as the window scrolls.
            let overflow_width = u16::try_from(self.list_cols).unwrap_or(width);
            let room = usize::from(width) - self.list_cols - self.gap;
            let preview = preview(u16::try_from(room).unwrap_or(width))
                .into_iter()
                .map(fold_line_style)
                .collect();
            lines.extend(lay_beside(
                list(overflow_width),
                self.list_cols,
                self.gap,
                preview,
            ));
        } else {
            lines.extend(list(width));
            lines.push(Line::default());
            let inner = width.saturating_sub(cols(MODEL_INDENT) as u16);
            lines.extend(preview(inner).into_iter().map(|line| {
                let mut spans = vec![Span::raw(MODEL_INDENT)];
                spans.extend(fold_line_style(line).spans);
                Line::from(spans)
            }));
        }
        lines.push(Line::default());
        lines
    }
}

/// `line` with its own style folded onto its spans (the way ratatui patches
/// it at paint time), and the line itself unstyled, so a row can be laid
/// beside, or inset behind, other content without its ground painting that
/// content too.
fn fold_line_style(line: Line<'static>) -> Line<'static> {
    let style = line.style;
    Line::from(
        line.spans
            .into_iter()
            .map(|span| Span::styled(span.content, style.patch(span.style)))
            .collect::<Vec<_>>(),
    )
}

/// Lay `right` beside `left`, row by row: each left row padded out to
/// `left_cols` columns, `gap` blank columns, then the right row. A column
/// that runs out leaves its side of the row blank, and a row with nothing on
/// its right carries no trailing padding.
fn lay_beside(
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

/// What one rung of a compact page keeps (`COMPACT_RUNGS`): the frame's
/// rules, the overflow slots, how many rows of the list, and the tail. The
/// `❯` search line is on every rung.
#[derive(Clone, Copy)]
struct Rung {
    top_rule: bool,
    slots: bool,
    rows: u16,
    tail: bool,
    bottom_rule: bool,
}

/// The compact page.
const COMPACT: Rung = Rung {
    top_rule: true,
    slots: true,
    rows: COMPACT_LIST_ROWS,
    tail: true,
    bottom_rule: true,
};

/// A search-line picker's compact page and the rungs it sheds down to, each
/// one row shorter than the last but where the overflow pair goes —
/// codex's order (`selection_picker_layout.rs`: *"optional gaps and
/// previews yield first; headers, tabs, search, and at least one result
/// remain visible"*), adapted to this frame, which pads neither rule
/// (`docs/view-flow.md` *The frame hugs its content*):
///
/// 1. the compact page: rule, search, `↑`, three rows, `↓`, tail, rule;
/// 2. the overflow slots, as a pair: codex reserves its arrow rows only
///    beside three rows of results;
/// 3. the top rule — a page this short fills the terminal, so nothing is on
///    screen above it to set apart, and the search line opens the screen;
/// 4. the rows, three to two to one — the highlighted one;
/// 5. the bottom rule;
/// 6. the tail;
/// 7. the highlighted row, leaving the search line alone.
const COMPACT_RUNGS: [Rung; 8] = [
    COMPACT,
    Rung {
        slots: false,
        ..COMPACT
    },
    Rung {
        top_rule: false,
        slots: false,
        ..COMPACT
    },
    Rung {
        top_rule: false,
        slots: false,
        rows: 2,
        ..COMPACT
    },
    Rung {
        top_rule: false,
        slots: false,
        rows: 1,
        ..COMPACT
    },
    Rung {
        top_rule: false,
        slots: false,
        rows: 1,
        bottom_rule: false,
        ..COMPACT
    },
    Rung {
        top_rule: false,
        slots: false,
        rows: 1,
        tail: false,
        bottom_rule: false,
    },
    Rung {
        top_rule: false,
        slots: false,
        rows: 0,
        tail: false,
        bottom_rule: false,
    },
];

/// One rung of a compact page as lines: what `rung` keeps of the frame
/// around the `search` line, the list's `window` — called with the rows the
/// rung shows — and the `tail` under it (the key hint, `/model`'s name line,
/// or nothing, when the page closes on its list: a blank `↓` slot is then
/// left off rather than hug the bottom rule). Only a placeholder's page has
/// no tail, and a placeholder hides nothing, so its height still holds.
fn rung_lines(
    rung: Rung,
    search: &Line<'static>,
    window: &dyn Fn(usize) -> (usize, Vec<Line<'static>>, usize),
    tail: &[Line<'static>],
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if rung.top_rule {
        lines.push(model_rule(width));
    }
    lines.push(search.clone());
    if rung.rows > 0 {
        let (above, rows, below) = window(usize::from(rung.rows));
        if rung.slots {
            let mut list = model_compact_list(above, rows, below, width);
            if tail.is_empty() && below == 0 {
                list.pop();
            }
            lines.extend(list);
        } else {
            lines.extend(rows);
        }
    }
    if rung.tail {
        lines.extend(tail.iter().cloned());
    }
    if rung.bottom_rule {
        lines.push(model_rule(width));
    }
    lines
}

/// A search-line picker's compact page on a terminal `height` rows tall: the
/// first of `COMPACT_RUNGS` that fits — the search line on every one, so it
/// never flows into scrollback (where the caret cannot follow it), and the
/// highlighted row on every one but the last. `window(max)` builds the
/// list's window of at most `max` rows riding the selection, as
/// [`list_window`] does — how many rows it hides above, its rows, how many
/// below; a placeholder's window is [`placeholder_window`]. Like
/// [`first_page_that_fits`], a region between the chosen page's height and
/// the terminal's builds the same rung.
pub(super) fn compact_page(
    height: u16,
    search: &Line<'static>,
    window: &dyn Fn(usize) -> (usize, Vec<Line<'static>>, usize),
    tail: &[Line<'static>],
    width: u16,
) -> Vec<Line<'static>> {
    let mut page = Vec::new();
    for rung in COMPACT_RUNGS {
        page = rung_lines(rung, search, window, tail, width);
        if page.len() <= usize::from(height) {
            break;
        }
    }
    page
}

/// A placeholder block — a search that matched nothing, a list still
/// loading, an empty catalog — as a compact page's window
/// ([`compact_page`]): the whole block while the page keeps three rows of
/// list, its first `max` rows once the rows shrink. Nothing is hidden around
/// it, so the overflow slots stay blank.
pub(super) fn placeholder_window(
    block: &[Line<'static>],
    max: usize,
) -> (usize, Vec<Line<'static>>, usize) {
    let shown = if max >= usize::from(COMPACT_LIST_ROWS) {
        block.len()
    } else {
        max.min(block.len())
    };
    (0, block[..shown].to_vec(), 0)
}

/// The first of `pages` — tried in order, most complete first, each built
/// only once the ones before it did not fit — whose rows fit `height`; the
/// last one when none does, which then bottom-anchors like any framed page.
/// The rule every height-aware page shares (`docs/view-flow.md`): on a
/// terminal too short for the whole page, what is optional yields before the
/// choices do.
///
/// The pages must shrink in order, so that every reader of a page — the
/// reserved height, the paint, the cursor seat, the flow — builds the same
/// one whether it passes the terminal's height or the region already
/// reserved for the page, which is at least the chosen page's height and at
/// most the terminal's.
pub(super) fn first_page_that_fits(
    height: u16,
    pages: &[&dyn Fn() -> Vec<Line<'static>>],
) -> Vec<Line<'static>> {
    let mut page = Vec::new();
    for build in pages {
        page = build();
        if page.len() <= usize::from(height) {
            break;
        }
    }
    page
}

/// `text` word-wrapped to two-space-inset rows in `color` — the multi-row
/// sibling of [`model_placeholder_row`] for informational text (an error's
/// diagnostic, an actionable hint, a path, a description): the picker pages
/// derive their height from the built line count (`docs/view-flow.md`), so
/// continuation rows are free and nothing the user must read is ever cut.
pub(super) fn model_wrapped_rows(text: &str, color: Color, width: u16) -> Vec<Line<'static>> {
    inset_wrapped_rows(MODEL_INDENT, text, color, width)
}

/// [`model_wrapped_rows`] in a search-line picker's names' column
/// ([`MODEL_LIST_INDENT`]) — the multi-row sibling of
/// [`list_placeholder_row`], for what speaks for the list and must be read
/// whole: a failed provider's reason, the `/login` hint standing in for
/// `/model`'s list, the roots an empty `/skills` names.
pub(super) fn list_wrapped_rows(text: &str, color: Color, width: u16) -> Vec<Line<'static>> {
    inset_wrapped_rows(MODEL_LIST_INDENT, text, color, width)
}

/// `text` word-wrapped behind `indent`, every row in `color`.
fn inset_wrapped_rows(
    indent: &'static str,
    text: &str,
    color: Color,
    width: u16,
) -> Vec<Line<'static>> {
    let room = (width as usize).saturating_sub(cols(indent)).max(1) as u16;
    text.split('\n')
        .flat_map(|part| super::wrap::wrap_text(part, room))
        .map(|row| {
            Line::from(vec![
                Span::raw(indent),
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
        .map(inset_row)
        .collect()
}

/// [`model_linked_rows`] for a text that **is** one URL — the `/mcp` sign-in
/// page's authorize URL (`docs/mcp.md`): the same dress, the caller's colour
/// underlined, but the target is the whole text **verbatim**
/// ([`whole_url_run`]). The page shows one thing, the URL `c` copies and the
/// browser was handed, so a click must open exactly that — where prose
/// detection would give a closing `.` or an unbalanced `)` back to a
/// sentence that isn't there. Any other text is linked the prose way.
pub(super) fn model_url_rows(text: &str, color: Color, width: u16) -> Vec<Line<'static>> {
    let Some(run) = whole_url_run(text, Style::new().fg(color)) else {
        return model_linked_rows(text, color, width);
    };
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT)).max(1) as u16;
    super::inline::wrap_inline(&[run], room)
        .into_iter()
        .map(inset_row)
        .collect()
}

/// One wrapped row of spans behind the pages' `MODEL_INDENT` inset.
fn inset_row(spans: Vec<Span<'static>>) -> Line<'static> {
    let mut row = Vec::with_capacity(spans.len() + 1);
    row.push(Span::raw(MODEL_INDENT));
    row.extend(spans);
    Line::from(row)
}

/// A text that is, whole, one URL ([`links::is_whole_url`]) as the one
/// styled run that links it: underlined over `base` and carrying the whole
/// text as its target, which the wrap then carries onto every fragment —
/// `None` for anything else. The shared piece of [`model_url_rows`] and of
/// the `/mcp` server page's `URL:` row, which wraps under its label.
///
/// [`links::is_whole_url`]: crate::links::is_whole_url
pub(super) fn whole_url_run(text: &str, base: Style) -> Option<(String, Style)> {
    crate::links::is_whole_url(text).then(|| {
        let dress = base.add_modifier(Modifier::UNDERLINED);
        (text.to_string(), crate::links::linked(dress, text))
    })
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

/// One model row: `{indent}{marker}{id} [{provider}]{✓}` ([`marked_row_head`])
/// — the selected row's marker and id light up cyan (the palette accent), the
/// `[provider]` tag is dim, and the active model carries a green ✓. The id is
/// `…`-cut so the tag stays visible *and* the cut shows — two long ids
/// clipped silently read as twins.
fn model_row(entry: &ModelEntry, selected: bool, active: bool, width: u16) -> Line<'static> {
    let tag = format!(" [{}]", entry.provider);
    let active_mark = if active { MODEL_ACTIVE_MARK } else { "" };
    let reserved = cols(MODEL_LIST_INDENT) + cols(&tag) + cols(active_mark);
    let id_room = (width as usize).saturating_sub(reserved).max(1);
    let mut spans = marked_row_head(&ellipsize(&entry.id, id_room), selected);
    spans.push(Span::styled(tag, Style::new().fg(model_meta_color())));
    spans.push(Span::styled(
        active_mark.to_string(),
        Style::new().fg(model_active_color()),
    ));
    Line::from(spans)
}

/// The picker's list lines: a single placeholder while loading / errored /
/// empty, else the model rows windowed ([`centered_window`]) to keep the
/// selection **centered** and capped at [`MODEL_MENU_MAX_ROWS`]. A
/// placeholder stands in the list's place, so it sits in the ids' column
/// ([`MODEL_LIST_INDENT`]).
fn model_list_lines(picker: &ModelPicker, width: u16) -> Vec<Line<'static>> {
    match &picker.status {
        ModelLoad::Loading => vec![list_placeholder_row(
            MODEL_LOADING,
            model_meta_color(),
            width,
        )],
        // Every provider failed: red rows (`{provider}: {reason}`), or a
        // single legacy message when there are no per-provider errors —
        // **wrapped**, because the error text is the only diagnostic the
        // user gets and a silent clip hid the cause at narrow widths.
        ModelLoad::Error(msg) => {
            if picker.errors.is_empty() {
                list_wrapped_rows(&format!("Error: {msg}"), error_color(), width)
            } else {
                picker
                    .errors
                    .iter()
                    .flat_map(|e| {
                        list_wrapped_rows(
                            &format!("{}: {}", e.provider, e.message),
                            error_color(),
                            width,
                        )
                    })
                    .collect()
            }
        }
        // No key configured yet — an inviting cyan hint, not a red error.
        // Wrapped: the actionable "run /login" tail must survive any width.
        ModelLoad::NeedsLogin => list_wrapped_rows(MODEL_LOGIN_HINT, model_selected_color(), width),
        ModelLoad::Ready => {
            let matches = picker.matches();
            if matches.is_empty() {
                // A partial multi-provider list cannot prove the query has no
                // match — a provider still fetching may hold the very model
                // being searched for. Keep the loading placeholder until every
                // list has landed rather than flashing `No matching models`
                // and then filling in the match seconds later.
                let text = if picker.pending > 0 {
                    MODEL_LOADING
                } else if picker.models.is_empty() {
                    MODEL_NONE
                } else {
                    MODEL_NO_MATCH
                };
                return vec![list_placeholder_row(text, model_meta_color(), width)];
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

/// The `(selected+1/total)` counter line under the list, in the ids' column,
/// or a blank line when there's nothing selectable (loading / error / empty).
/// Clamped to the width with a dim `…` ([`clamp_spans`]) — the red
/// `{provider} unavailable` suffix used to paint-clip past the buffer edge,
/// an error the user never saw.
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
        Span::raw(MODEL_LIST_INDENT),
        Span::styled(
            format!("({}/{})", selected + 1, matches.len()),
            Style::new().fg(model_meta_color()),
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
        Some((MODEL_LOADING_MORE.to_string(), model_meta_color()))
    } else if picker.errors.len() == 1 {
        Some((
            format!("{} unavailable", picker.errors[0].provider),
            error_color(),
        ))
    } else if !picker.errors.is_empty() {
        Some((
            format!("{} providers unavailable", picker.errors.len()),
            error_color(),
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
/// In the ids' column, under the name line: it speaks for the list.
fn model_error_lines(picker: &ModelPicker, width: u16) -> Vec<Line<'static>> {
    let mut rows: Vec<Line<'static>> = picker
        .errors
        .iter()
        .flat_map(|e| {
            list_wrapped_rows(
                &format!("{}: {}", e.provider, e.message),
                error_color(),
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
            rows.push(Line::from(Span::styled(
                cut,
                Style::new().fg(error_color()),
            )));
        }
    }
    rows
}

/// The `Model Name: {friendly}` line under the counter, in the ids' column,
/// naming the highlighted model, or a blank line when nothing is
/// highlighted.
fn model_name_line(picker: &ModelPicker, width: u16) -> Line<'static> {
    let Some(entry) = picker.highlighted() else {
        return Line::default();
    };
    let room = (width as usize)
        .saturating_sub(cols(MODEL_LIST_INDENT) + cols(MODEL_NAME_LABEL))
        .max(1);
    Line::from(vec![
        Span::raw(MODEL_LIST_INDENT),
        Span::styled(MODEL_NAME_LABEL, Style::new().fg(model_meta_color())),
        Span::styled(
            ellipsize(&entry.display_name, room),
            Style::new().fg(model_meta_color()),
        ),
    ])
}

/// The row of a framed page's `lines` carrying the `❯` prompt — `/login`'s
/// list filter or key field, the `/skills` search line — and `None` on a page
/// that has none (the `/login` sign-in page, which is a wait rather than a
/// field).
///
/// **Found, not counted.** `/login`'s key field used to sit on a constant
/// row, which stopped being true the moment the provider's description block
/// moved in above it — that block is as tall as the terminal is narrow — and
/// the `/skills` search line sits under a session-off note that wraps the
/// same way. Reading the row back out of the very page the paint builds is
/// `menu_marker_seat`'s rule, and it cannot drift from what is on screen.
pub(super) fn view_prompt_row(lines: &[Line<'static>]) -> Option<u16> {
    view_row_of(lines, MODEL_PROMPT)
}

/// The row of a framed page's `lines` carrying a span that reads exactly
/// `prompt` — [`view_prompt_row`] for a field whose prompt is not the `❯`,
/// like the `/mcp` auth page's `URL >`. `None` on a page without one.
pub(super) fn view_row_of(lines: &[Line<'static>], prompt: &str) -> Option<u16> {
    lines
        .iter()
        .position(|line| line.spans.iter().any(|s| s.content == prompt))
        .and_then(|row| u16::try_from(row).ok())
}

/// A full-width `─` rule in the box's border colour (the picker's top/bottom
/// frame, matching the input box's rules).
pub(super) fn model_rule(width: u16) -> Line<'static> {
    Line::from(Span::styled(
        VIEW_RULE.repeat(width as usize),
        Style::new().fg(border_color()),
    ))
}

/// A search-line picker's `❯` search line: the family's inset, the cyan
/// prompt, then what was typed.
pub(super) fn picker_search_line(query: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(model_selected_color())),
        Span::raw(query.to_string()),
    ])
}

/// A search-line picker's header: its `title` in the pages' title dress
/// ([`page_title_line`](super::header::page_title_line)) over its
/// `description`, dim and wrapped whole — the header `/skills`, `/hooks` and
/// `/mcp` open on, so every composer-replacing page says what it is in the
/// same place (`docs/view-flow.md` *A header over the search line*).
pub(super) fn picker_header(title: &str, description: &str, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![super::header::page_title_line(title, width)];
    lines.extend(model_wrapped_rows(description, model_meta_color(), width));
    lines
}

/// A search-line picker's page: the top rule, the `header` and a gap under
/// it (when there is one), the `search` line, the `body`, the `tail` (the
/// key hint, `/model`'s name line, or nothing) and the bottom rule — no
/// blank row hugs either rule (`docs/view-flow.md` *The frame hugs its
/// content*).
pub(super) fn search_page(
    header: &[Line<'static>],
    search: &Line<'static>,
    body: Vec<Line<'static>>,
    tail: &[Line<'static>],
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![model_rule(width)];
    if !header.is_empty() {
        lines.extend_from_slice(header);
        lines.push(Line::default());
    }
    lines.push(search.clone());
    lines.extend(body);
    lines.extend_from_slice(tail);
    lines.push(model_rule(width));
    lines
}

/// The compact page ([`compact_page`]'s first rung) with `header` and a gap
/// between its top rule and its search line: what a picker with a header
/// builds on a terminal too short for its whole page but tall enough for the
/// header, since the header yields last (`docs/view-flow.md` *A page that
/// sheds rows before it flows*). `window` and `tail` are [`compact_page`]'s.
pub(super) fn headed_compact_page(
    header: &[Line<'static>],
    search: &Line<'static>,
    window: &dyn Fn(usize) -> (usize, Vec<Line<'static>>, usize),
    tail: &[Line<'static>],
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = rung_lines(COMPACT, search, window, tail, width);
    let mut head = header.to_vec();
    head.push(Line::default());
    lines.splice(1..1, head);
    lines
}

/// A search-line picker's page when its list is a placeholder — a search
/// matching nothing, a list still loading, an empty catalog — on a terminal
/// `height` rows tall: the placeholder `block` in the list's place under the
/// `header` ([`headed_compact_page`]: the placeholder between single gaps,
/// over the `tail`), or, on a terminal too short for the header, the rungs
/// of [`compact_page`], keeping what was typed.
pub(super) fn placeholder_page(
    height: u16,
    header: &[Line<'static>],
    search: &Line<'static>,
    block: &[Line<'static>],
    tail: &[Line<'static>],
    width: u16,
) -> Vec<Line<'static>> {
    let window = |max| placeholder_window(block, max);
    first_page_that_fits(
        height,
        &[
            &|| headed_compact_page(header, search, &window, tail, width),
            &|| compact_page(height, search, &window, tail, width),
        ],
    )
}

/// A previewed picker's page (`/theme`, `/mascot`, `/spinner`) on a terminal
/// `height` rows tall: the first that fits of the **whole** page under its
/// `header` (`whole` builds the body between the search line and the hint
/// — the list beside or over the preview, [`PreviewLayout::body`]), the
/// whole page without the header, the compact page under the header, and
/// the compact page's rungs. The header yields **before** the preview, the
/// one place it does not go last (`docs/view-flow.md`): the preview is what
/// the choice is made by, while the header names a page opened by name.
pub(super) fn previewed_page(
    height: u16,
    header: &[Line<'static>],
    search: &Line<'static>,
    whole: &dyn Fn() -> Vec<Line<'static>>,
    window: &dyn Fn(usize) -> (usize, Vec<Line<'static>>, usize),
    hint: &[Line<'static>],
    width: u16,
) -> Vec<Line<'static>> {
    first_page_that_fits(
        height,
        &[
            &|| search_page(header, search, whole(), hint, width),
            &|| search_page(&[], search, whole(), hint, width),
            &|| headed_compact_page(header, search, window, hint, width),
            &|| compact_page(height, search, window, hint, width),
        ],
    )
}

/// The framed page as lines — the shape of the user's mock: a top rule, the
/// header (the `Select model` title over a dim line saying what a pick
/// changes, [`picker_header`]), a gap, the `❯` search line, the scrolling
/// model list (each row `→ id [provider] ✓`, under the `❯`), and — when a
/// real model is highlighted — the `(n/total)` counter and the `Model Name:`
/// line straight over the bottom rule (a placeholder page has neither, so
/// the placeholder closes it). No blank row hugs either rule
/// (`docs/view-flow.md` *The frame hugs its content*).
///
/// The first page that fits `height` rows ([`first_page_that_fits`]): the
/// **whole** page; on a terminal too short for it, the **compact** page
/// under the header ([`headed_compact_page`]) — three models between the
/// reserved overflow slots, the counter and the failed providers' rows
/// yielding; then the compact page alone ([`compact_page`]), the header
/// yielding last, and on a terminal shorter still the slots, the rules and
/// the rows one by one — so the search line and the highlighted model stay
/// on screen (`docs/view-flow.md` *A page that sheds rows before it flows*).
/// The picker has no hint row, so a compact page keeps the highlighted
/// model's `Model Name:` line where its siblings keep theirs, which gives it
/// their shape. A placeholder page (loading, an error, no match) has nothing
/// under its list, so the placeholder closes it, and it sheds the same way,
/// keeping what was typed. What [`render_model_picker`] paints and
/// `layout::model_picker_rows` counts, so the reserved height and the
/// painted rows can never disagree.
pub(super) fn model_view_lines(
    picker: &ModelPicker,
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let header = picker_header(MODEL_TITLE, MODEL_DESCRIPTION, width);
    let search_line = picker_search_line(&picker.query);
    if !model_has_detail(picker) {
        // A placeholder (loading / error / needs-login / no match) has no
        // counter and no name to show, so the placeholder closes the page.
        let block = model_list_lines(picker, width);
        return placeholder_page(height, &header, &search_line, &block, &[], width);
    }
    let whole = || {
        let mut body = vec![Line::default()];
        body.extend(model_list_lines(picker, width));
        body.push(model_counter_line(picker, width));
        body.push(Line::default());
        body.push(model_name_line(picker, width));
        body.extend(model_error_lines(picker, width));
        search_page(&header, &search_line, body, &[], width)
    };
    let matches = picker.matches();
    let selected = picker.selected.min(matches.len() - 1);
    let window = |max| {
        let (above, range, below) = list_window(matches.len(), selected, max);
        let rows = matches[range.clone()]
            .iter()
            .zip(range)
            .map(|(m, i)| model_row(m, i == selected, picker.is_active(m), width))
            .collect();
        (above, rows, below)
    };
    let tail = [model_name_line(picker, width)];
    first_page_that_fits(
        height,
        &[
            &whole,
            &|| headed_compact_page(&header, &search_line, &window, &tail, width),
            &|| compact_page(height, &search_line, &window, &tail, width),
        ],
    )
}

/// Render the **inline** `/model` picker into the live region, in place of the
/// composer. The page is built for the area's height — the whole list, the
/// compact page, or that page's rungs down to the search line — so it always
/// fits and nothing flows (`docs/view-flow.md`). Pure — `render_live` paints
/// this. See `docs/llm.md`.
pub fn render_model_picker(area: Rect, buf: &mut Buffer, picker: &ModelPicker) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        model_view_lines(picker, area.width, area.height),
    );
}
