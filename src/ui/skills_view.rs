//! The inline `/skills` menu. See `docs/skills.md`.
//!
//! The `/settings` menu's list under the pages' header: the header `/hooks`
//! and `/mcp` open on — the title in the pages' title dress over a dim count
//! line — then the `/settings` frame, reused rather than re-drawn: the same
//! `❯` search line, the same aligned `{label}  {value}` column with the same
//! two-tone colouring, the same `(n/total)` counter over the highlighted
//! row's description, the same hint row. Only the rows differ — one per
//! discovered skill, its value led by the `/login` rows' `✔`/`◯` status mark,
//! its own `description` as the line beneath — plus two notes this menu has
//! and that one doesn't: the session-off note in the header, and the
//! where-to-put-one answer for an empty list.
//!
//! A terminal too short for that page gets a **compact** one instead — the
//! `/theme` picker's, over this list: the description and the counter yield
//! first, then the header, leaving the search line, three skill rows between
//! codex's reserved `↑`/`↓` slots, and the hint — and a terminal shorter
//! still sheds that panel's own rows one by one, down to the search line
//! alone. Bottom-anchoring the whole page there flowed the search line and
//! the top of the list into scrollback, so the row Enter would toggle could
//! be off screen.

use super::model_view::{
    Description, compact_page, first_page_that_fits, list_window, model_compact_list,
    model_description_rows, model_placeholder_row, model_rule, model_wrapped_rows,
    placeholder_window,
};
use super::theme::*;
use super::wrap::{cols, ellipsize};
use super::*;

use crate::app::{SkillMenuRow, SkillsMenu};

/// The value a row ends on: its status mark, then its word — `✔ enabled`,
/// `◯ disabled` — each half in its own colour. The check alone is green
/// ([`model_active_color`]), the eye's target down a column of names, while
/// the circle stays dim; the word keeps the `/settings` two-tone, light when
/// the skill is on and dim when it is off.
fn skills_value(enabled: bool) -> (&'static str, Color, &'static str, Color) {
    if enabled {
        (
            SKILLS_ON_MARK,
            model_active_color(),
            SKILLS_ON_VALUE,
            settings_value_color(),
        )
    } else {
        (
            SKILLS_OFF_MARK,
            model_meta_color(),
            SKILLS_OFF_VALUE,
            settings_value_off_color(),
        )
    }
}

/// A value's whole width: the mark, the space after it, the word.
fn value_cols(enabled: bool) -> usize {
    let (mark, _, word, _) = skills_value(enabled);
    cols(mark) + 1 + cols(word)
}

/// One skill row: `{marker}{name}{pad}{mark} {enabled|disabled}` — the
/// `/settings` row's shape with a status mark leading the value, so the two
/// menus read as one family. `name_width` is the (capped) widest listed
/// name, so the value column lines up down the list; a name wider than it
/// `…`-cuts rather than paint-clipping at the buffer edge and shoving the
/// value off the row.
fn skills_row(row: &SkillMenuRow, selected: bool, name_width: usize, width: u16) -> Line<'static> {
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
    let name = ellipsize(&row.name, name_width.max(1));
    let pad = name_width.saturating_sub(cols(&name)) + SETTINGS_VALUE_GAP;
    let reserved = cols(marker) + cols(&name) + pad;
    let value_room = (width as usize).saturating_sub(reserved).max(1);
    let (mark, mark_color, word, word_color) = skills_value(row.enabled);
    let mut spans = vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(name, name_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(mark, Style::new().fg(mark_color)),
    ];
    // The mark IS the state, so it keeps its seat however narrow the row; the
    // word only restates it, so it is the half that `…`-cuts — the `/mcp`
    // rows' rule, where the glyph survives a squeeze its words don't.
    let word_room = value_room.saturating_sub(cols(mark) + 1);
    if word_room > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            ellipsize(word, word_room),
            Style::new().fg(word_color),
        ));
    }
    Line::from(spans)
}

/// The widest name among `rows` — capped so the value column always keeps a
/// seat on the row (`width` minus the marker, the gap, and the widest value)
/// — so [`skills_row`] can align the value column.
fn name_column(rows: &[SkillMenuRow], width: u16) -> usize {
    let widest_value = value_cols(true).max(value_cols(false));
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
/// raises. The middle of [`skills_placeholder_body`].
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
                model_meta_color(),
                width,
            )];
            lines.extend(roots.iter().flat_map(|root| {
                model_wrapped_rows(&format!("{root}/<name>/SKILL.md"), model_id_color(), width)
            }));
            lines
        }
        // Skills exist, the search just matched none of them.
        _ => vec![model_placeholder_row(
            SKILLS_NO_MATCH,
            model_meta_color(),
            width,
        )],
    }
}

/// The window of `rows` that keeps the selection in view — `max` rows,
/// [`list_window`] riding the selection on the middle one — as how many rows
/// it hides above, its rows (the selected one marked) and how many it hides
/// below. The value column is sized off **every** listed row, not just the
/// ones in view, so it holds still as ↑/↓ scroll the window — the compact
/// page's three rows would otherwise move it at nearly every step. `rows` is
/// never empty here.
fn skills_window(
    rows: &[SkillMenuRow],
    selected: usize,
    max: usize,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = list_window(rows.len(), selected, max);
    let name_width = name_column(rows, width);
    let lines = rows[range.clone()]
        .iter()
        .zip(range)
        .map(|(row, i)| skills_row(row, i == selected, name_width, width))
        .collect();
    (above, lines, below)
}

/// The `(selected+1/total)` counter under the whole page's list.
fn skills_counter_line(selected: usize, total: usize) -> Line<'static> {
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("({}/{total})", selected + 1),
            Style::new().fg(model_meta_color()),
        ),
    ])
}

/// `{n} skill(s)` — the count line's noun, singular for one.
fn skill_noun(n: usize) -> String {
    if n == 1 {
        format!("{n} skill")
    } else {
        format!("{n} skills")
    }
}

/// The header's dim count line — `6 skills · 5 enabled`, `/hooks`' `3 hooks
/// configured` and `/mcp`'s `2 servers` for this page. It counts what is
/// **installed**, never what the search shows: the `(n/total)` counter under
/// the list already follows the query. An empty catalog reads `0 skills`
/// alone, since a `· 0 enabled` clause would say nothing twice over the
/// placeholder that answers an empty list. One `…`-cut row, so the header's
/// height never changes with the width.
fn skills_count_line(menu: &SkillsMenu, width: u16) -> Line<'static> {
    let total = menu.skills.len();
    let text = if total == 0 {
        skill_noun(total)
    } else {
        format!(
            "{}{FOOTER_SEPARATOR}{} {SKILLS_ON_VALUE}",
            skill_noun(total),
            menu.enabled_count()
        )
    };
    model_placeholder_row(&text, model_meta_color(), width)
}

/// The page's header: the title, the count line, and — when skills are off
/// for the session — the note saying so. The note joins the header because
/// it says what state the whole catalog is in, so it sits under the count
/// rather than between the search line and the rows it filters. It takes
/// rows only when there IS one to show, and it wraps — its "/settings" tail
/// is the fix, and a narrow terminal used to cut it. Which is also why the
/// search line has no fixed row: the caret's seat is found in the page
/// (`model_view::view_prompt_row`), never counted.
///
/// Never over an empty catalog: with nothing installed the `/settings` row
/// is unavailable, so the session always reads as off there, and the note's
/// advice is one that row refuses. It explains why *listed* rows do nothing;
/// with none listed, the placeholder answers instead.
fn skills_header_lines(menu: &SkillsMenu, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![
        super::header::page_title_line(SKILLS_TITLE, width),
        skills_count_line(menu, width),
    ];
    if !menu.session_enabled && !menu.skills.is_empty() {
        lines.extend(model_wrapped_rows(
            SKILLS_SESSION_OFF,
            toast_error_color(),
            width,
        ));
    }
    lines
}

/// The whole page's body — what sits between the search line and the hint:
/// a gap, the rows ([`skills_window`] at [`SETTINGS_MENU_MAX_ROWS`]), the
/// `(n/total)` counter, and the highlighted skill's description between
/// gaps, laid out by `description`. Wrapped whole when the terminal has the
/// room, because this picker doubles as the browser that answers "what is
/// this skill for?" and a SKILL.md description routinely outruns the
/// terminal width.
fn skills_whole_body(
    rows: &[SkillMenuRow],
    selected: usize,
    description: Description,
    width: u16,
) -> Vec<Line<'static>> {
    let (_, window, _) = skills_window(rows, selected, usize::from(SETTINGS_MENU_MAX_ROWS), width);
    let mut lines = vec![Line::default()];
    lines.extend(window);
    lines.push(skills_counter_line(selected, rows.len()));
    lines.push(Line::default());
    lines.extend(model_description_rows(
        &rows[selected].description,
        description,
        width,
    ));
    lines.push(Line::default());
    lines
}

/// The compact page's body, for a terminal too short for the whole page —
/// codex's short panel, where "optional gaps and previews yield first"
/// (`selection_picker_layout.rs`): a [`COMPACT_LIST_ROWS`]-row window of
/// the rows between two reserved overflow slots ([`model_compact_list`] —
/// the `↑` directly under the search line, the `↓` directly over the hint),
/// and no counter or description, so what stays on screen is the choices.
/// The `/theme` picker's compact body for this list.
fn skills_compact_body(rows: &[SkillMenuRow], selected: usize, width: u16) -> Vec<Line<'static>> {
    let (above, window, below) =
        skills_window(rows, selected, usize::from(COMPACT_LIST_ROWS), width);
    model_compact_list(above, window, below, width)
}

/// The body when the list is empty: the placeholder block between single
/// gaps — one gap carries it to the hint (the `/settings` menu's collapse,
/// so the two menus stay twins). There is no window to compact, so it is the
/// same at every height.
fn skills_placeholder_body(app: &App, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    lines.extend(skills_empty_lines(app, width));
    lines.push(Line::default());
    lines
}

/// The menu's `❯` search line, carrying the query.
fn skills_search_line(menu: &SkillsMenu) -> Line<'static> {
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(model_selected_color())),
        Span::raw(menu.query.clone()),
    ])
}

/// Frame `body` as a page: a top rule and a gap, the `header` and a gap
/// under it (when there is a header), the `search` line, the body, the
/// `hint`, a gap and the bottom rule.
fn skills_page(
    header: &[Line<'static>],
    search: &Line<'static>,
    body: Vec<Line<'static>>,
    hint: &Line<'static>,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![model_rule(width), Line::default()];
    if !header.is_empty() {
        lines.extend_from_slice(header);
        lines.push(Line::default());
    }
    lines.push(search.clone());
    lines.extend(body);
    lines.push(hint.clone());
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// The framed page as lines — one blank row between each block, never two —
/// the first of four pages that fits `height` rows ([`first_page_that_fits`]):
///
/// 1. the **whole** page: the header ([`skills_header_lines`]), the `❯`
///    search line, the windowed skill rows, the `(n/total)` counter, the
///    highlighted skill's description wrapped whole, and the key hint;
/// 2. the whole page with the description **cut to one row**, which is one
///    height at every selection, so a terminal at the edge never flips
///    between the whole and the compact page as ↑/↓ cross a long one;
/// 3. the **compact** page, the header kept: the description and the counter
///    yield ([`skills_compact_body`] — three rows between reserved `↑`/`↓`
///    slots), codex's short-panel rule;
/// 4. the compact page **without the header**, which yields next — the
///    `/theme` picker's compact page, row for row — and below it the rungs
///    every search-line picker sheds to ([`compact_page`]): the gaps, the
///    overflow slots, the rules and the rows one by one, never the search
///    line.
///
/// So on a terminal too short for the whole page the search line, the
/// highlighted row and the hint stay on screen, where bottom-anchoring it
/// flowed the search line and the top of the list into scrollback
/// (`docs/view-flow.md`). What [`render_skills_menu`] paints and
/// [`skills_menu_rows`] counts, so the two can never disagree. Empty when
/// the menu is closed.
///
/// `height` is the rows the page may take: the terminal's, since the view
/// keeps its rows and the strip above it gives way (`view_split`), or the
/// region already reserved for it — which holds the same page, because that
/// region is at least the chosen page's height and at most the terminal's.
///
/// An empty list — a search matching nothing, or nothing installed — has no
/// window to compact, so its header yields and then it sheds the same way,
/// its placeholder in the list's place.
pub(super) fn skills_view_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    let Some(menu) = app.skills_menu.as_ref() else {
        return Vec::new();
    };
    let rows = app.skill_menu_rows();
    let selected = menu.selected.min(rows.len().saturating_sub(1));
    let header = skills_header_lines(menu, width);
    let search = skills_search_line(menu);
    let hint = model_placeholder_row(SKILLS_HINT, model_meta_color(), width);
    let page = |body: Vec<Line<'static>>| skills_page(&header, &search, body, &hint, width);
    if rows.is_empty() {
        let block = skills_empty_lines(app, width);
        return first_page_that_fits(
            height,
            &[&|| page(skills_placeholder_body(app, width)), &|| {
                compact_page(
                    height,
                    &search,
                    &|max| placeholder_window(&block, max),
                    std::slice::from_ref(&hint),
                    width,
                )
            }],
        );
    }
    let whole = |description| skills_whole_body(&rows, selected, description, width);
    first_page_that_fits(
        height,
        &[
            &|| page(whole(Description::Wrapped)),
            &|| page(whole(Description::OneRow)),
            &|| page(skills_compact_body(&rows, selected, width)),
            &|| {
                compact_page(
                    height,
                    &search,
                    &|max| skills_window(&rows, selected, max, width),
                    std::slice::from_ref(&hint),
                    width,
                )
            },
        ],
    )
}

/// The rows the menu's own frame occupies on a terminal `height` rows tall —
/// the built page's line count ([`skills_view_lines`]). What
/// [`skills_menu_height`] reserves under the strip, and what [`render_live`]
/// hands [`render_skills_menu`].
pub(super) fn skills_menu_rows(app: &App, width: u16, height: u16) -> u16 {
    u16::try_from(skills_view_lines(app, width, height).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/skills` menu is open, or `None`
/// when it isn't (the caller then falls back to [`live_height`]). Like every
/// sibling picker the menu **replaces** the composer — and only the composer:
/// the streaming strip keeps its rows above it, so opening `/skills` mid-turn
/// never hides the running turn. The page is a compact one when the whole
/// one is taller than the terminal (`skills_view_lines`); clamped to the
/// terminal height.
#[must_use]
pub fn skills_menu_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.skills_menu.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        skills_menu_rows(app, width, term_height),
        term_height,
    ))
}

/// Render the **inline** `/skills` menu into the live region, in place of the
/// composer: a top rule, the `Skills` title over its count line (and the
/// session-off note when it applies), the `❯` search line, the skill rows
/// (`→ {name}  ✔ enabled`), a `(n/total)` counter, the highlighted skill's
/// description, the key hint, and a bottom rule; or, in an area too short
/// for that, the compact page of three rows, with the header while it fits,
/// which sheds its own rows down to the search line in an area shorter
/// still. The page is built for the area's height, so it always fits and
/// nothing flows (`docs/view-flow.md`). Pure — `render_live` paints this.
/// See `docs/skills.md`.
pub fn render_skills_menu(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        skills_view_lines(app, area.width, area.height),
    );
}
