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

/// `text`'s rows in the picker family's dim, laid out by `layout`.
pub(super) fn model_description_rows(
    text: &str,
    layout: Description,
    width: u16,
) -> Vec<Line<'static>> {
    match layout {
        Description::Wrapped => model_wrapped_rows(text, model_meta_color(), width),
        Description::OneRow => {
            let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
            vec![model_placeholder_row(&flat, model_meta_color(), width)]
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

/// What one rung of a compact page keeps (`COMPACT_RUNGS`): the frame's
/// rules and gaps, the overflow slots, how many rows of the list, and the
/// tail. The `❯` search line is on every rung.
#[derive(Clone, Copy)]
struct Rung {
    top_rule: bool,
    top_gap: bool,
    slots: bool,
    rows: u16,
    tail: bool,
    bottom_gap: bool,
    bottom_rule: bool,
}

/// The compact page.
const COMPACT: Rung = Rung {
    top_rule: true,
    top_gap: true,
    slots: true,
    rows: COMPACT_LIST_ROWS,
    tail: true,
    bottom_gap: true,
    bottom_rule: true,
};

/// A search-line picker's compact page and the rungs it sheds down to, each
/// one row shorter than the last but where the overflow pair goes —
/// codex's order (`selection_picker_layout.rs`: *"optional gaps and
/// previews yield first; headers, tabs, search, and at least one result
/// remain visible"*), adapted to this frame:
///
/// 1. the compact page: rule, gap, search, `↑`, three rows, `↓`, tail, gap,
///    rule;
/// 2. the gap under the top rule — the search line then sits on the rule,
///    the composer's own look;
/// 3. the overflow slots, as a pair: codex reserves its arrow rows only
///    beside three rows of results;
/// 4. the top rule — a page this short fills the terminal, so nothing is on
///    screen above it to set apart, and the search line opens the screen;
/// 5. the gap over the bottom rule;
/// 6. the rows, three to two to one — the highlighted one;
/// 7. the bottom rule;
/// 8. the tail;
/// 9. the highlighted row, leaving the search line alone.
const COMPACT_RUNGS: [Rung; 10] = [
    COMPACT,
    Rung {
        top_gap: false,
        ..COMPACT
    },
    Rung {
        top_gap: false,
        slots: false,
        ..COMPACT
    },
    Rung {
        top_rule: false,
        top_gap: false,
        slots: false,
        ..COMPACT
    },
    Rung {
        top_rule: false,
        top_gap: false,
        slots: false,
        bottom_gap: false,
        ..COMPACT
    },
    Rung {
        top_rule: false,
        top_gap: false,
        slots: false,
        rows: 2,
        bottom_gap: false,
        ..COMPACT
    },
    Rung {
        top_rule: false,
        top_gap: false,
        slots: false,
        rows: 1,
        bottom_gap: false,
        ..COMPACT
    },
    Rung {
        top_rule: false,
        top_gap: false,
        slots: false,
        rows: 1,
        bottom_gap: false,
        bottom_rule: false,
        ..COMPACT
    },
    Rung {
        top_rule: false,
        top_gap: false,
        slots: false,
        rows: 1,
        tail: false,
        bottom_gap: false,
        bottom_rule: false,
    },
    Rung {
        top_rule: false,
        top_gap: false,
        slots: false,
        rows: 0,
        tail: false,
        bottom_gap: false,
        bottom_rule: false,
    },
];

/// One rung of a compact page as lines: what `rung` keeps of the frame
/// around the `search` line, the list's `window` — called with the rows the
/// rung shows — and the `tail` under it (the key hint, `/model`'s name line,
/// or nothing, when the `↓` slot is the gap over the bottom rule and no
/// second gap stacks on it).
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
    if rung.top_gap {
        lines.push(Line::default());
    }
    lines.push(search.clone());
    if rung.rows > 0 {
        let (above, rows, below) = window(usize::from(rung.rows));
        if rung.slots {
            lines.extend(model_compact_list(above, rows, below, width));
        } else {
            lines.extend(rows);
        }
    }
    if rung.tail && !tail.is_empty() {
        lines.extend(tail.iter().cloned());
        if rung.bottom_gap {
            lines.push(Line::default());
        }
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
            Style::new().fg(model_selected_color()),
            Style::new()
                .fg(model_selected_color())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (Style::default(), Style::new().fg(model_id_color()))
    };
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(id, id_style),
        Span::styled(tag, Style::new().fg(model_meta_color())),
        Span::styled(
            active_mark.to_string(),
            Style::new().fg(model_active_color()),
        ),
    ])
}

/// The picker's list lines: a single placeholder while loading / errored /
/// empty, else the model rows windowed ([`centered_window`]) to keep the
/// selection **centered** and capped at [`MODEL_MENU_MAX_ROWS`].
fn model_list_lines(picker: &ModelPicker, width: u16) -> Vec<Line<'static>> {
    match &picker.status {
        ModelLoad::Loading => vec![model_placeholder_row(
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
                model_wrapped_rows(&format!("Error: {msg}"), error_color(), width)
            } else {
                picker
                    .errors
                    .iter()
                    .flat_map(|e| {
                        model_wrapped_rows(
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
        ModelLoad::NeedsLogin => {
            model_wrapped_rows(MODEL_LOGIN_HINT, model_selected_color(), width)
        }
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
                return vec![model_placeholder_row(text, model_meta_color(), width)];
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
fn model_error_lines(picker: &ModelPicker, width: u16) -> Vec<Line<'static>> {
    let mut rows: Vec<Line<'static>> = picker
        .errors
        .iter()
        .flat_map(|e| {
            model_wrapped_rows(
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
    lines
        .iter()
        .position(|line| line.spans.iter().any(|s| s.content == MODEL_PROMPT))
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

/// The framed page as lines — the shape of the user's mock: a top rule, the
/// `❯` search line, the scrolling model list (each row `→ id [provider] ✓`),
/// and — when a real model is highlighted — the `(n/total)` counter and the
/// `Model Name:` line above the bottom rule (a placeholder page collapses
/// them to a single gap). Headerless (the "Showing models…" banner was
/// dropped).
///
/// The first of two pages that fits `height` rows ([`first_page_that_fits`]):
/// the **whole** page, or, on a terminal too short for it, a **compact**
/// page ([`compact_page`]) — three models between the reserved overflow
/// slots, the counter and the failed providers' rows yielding, and on a
/// terminal shorter still the gaps, the slots, the rules and the rows one
/// by one — so the search line and the highlighted model stay on screen
/// (`docs/view-flow.md` *A page that sheds rows before it flows*). The
/// picker has no hint row, so the compact page keeps the highlighted
/// model's `Model Name:` line where its siblings keep theirs, which gives it
/// their shape. A placeholder page (loading, an error, no match) has
/// nothing under its list, so its `↓` slot is the gap over the bottom rule,
/// and it sheds the same way, keeping what was typed. What
/// [`render_model_picker`] paints and `layout::model_picker_rows` counts,
/// so the reserved height and the painted rows can never disagree.
pub(super) fn model_view_lines(
    picker: &ModelPicker,
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let search_line = Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(model_selected_color())),
        Span::raw(picker.query.clone()),
    ]);
    let page = |body: Vec<Line<'static>>| {
        let mut lines = vec![model_rule(width), Line::default(), search_line.clone()];
        lines.extend(body);
        lines.push(model_rule(width));
        lines
    };
    if !model_has_detail(picker) {
        // A placeholder (loading / error / needs-login / no match): the blank
        // counter + name collapse to a single gap above the bottom rule.
        let block = model_list_lines(picker, width);
        return compact_page(
            height,
            &search_line,
            &|max| placeholder_window(&block, max),
            &[],
            width,
        );
    }
    let whole = || {
        let mut body = vec![Line::default()];
        body.extend(model_list_lines(picker, width));
        body.push(model_counter_line(picker, width));
        body.push(Line::default());
        body.push(model_name_line(picker, width));
        body.extend(model_error_lines(picker, width));
        body.push(Line::default());
        page(body)
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
    let compact = || {
        compact_page(
            height,
            &search_line,
            &window,
            &[model_name_line(picker, width)],
            width,
        )
    };
    first_page_that_fits(height, &[&whole, &compact])
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
