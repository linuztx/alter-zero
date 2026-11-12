//! The bands below the input box: the slash-command palette, the `@` file
//! picker, and the `?` shortcuts overlay.
//! See `docs/input/file-search.md` and `docs/ui/shortcuts.md`.

use super::theme::*;
use super::wrap::{cols, truncate_cols, wrap_text};
use super::*;

/// How many rows the command palette occupies for `app` at `width`: 0 when
/// closed, otherwise **the built line count** of [`command_menu_lines`] — the
/// window over the matches (at most `MENU_MAX_COMMANDS`), wrapped-description
/// continuation rows included, and the `(n/total)` counter under them, never
/// more than `MENU_MAX_ROWS` (or a single placeholder row when the query
/// matches nothing). Height-is-the-line-count is the settings/model pickers'
/// rule (`docs/ui/view-flow.md`): the reserved rows and the painted rows agree
/// by construction, which is what lets a long description *wrap* instead of
/// clipping at the width. [`live_height`] adds this; [`render_live`] paints
/// exactly this many rows.
#[must_use]
pub fn menu_rows(app: &App, width: u16) -> u16 {
    u16::try_from(command_menu_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// The scroll offset (first visible match index) so a window of `max` rows keeps
/// `selected` visible: 0 while the selection fits the first window, then it
/// follows the selection toward the end, clamped so the last window sits flush
/// with the end.
#[must_use]
pub fn menu_window(len: usize, selected: usize, max: usize) -> usize {
    if max == 0 || len <= max || selected < max {
        0
    } else {
        (selected + 1 - max).min(len - max)
    }
}

/// The scroll offset (first visible match index) that keeps `selected`
/// **centered** in a window of `max` rows: the selection rides the middle row
/// (`max/2`) while there is room on both sides, so the user always sees as much
/// of the list *above and below* the highlight as fits — the "broad view" the
/// `/model` and `/login` pickers want, the `@` and `$` pickers below the box,
/// and the slash-command palette too (row-budgeted by `centered_window_rows`,
/// whose wrapped entries make each one a different height). Near the ends the
/// window can't center (there aren't enough rows on one side), so it anchors:
/// the top for the first `max/2` selections, the bottom (flush with the tail)
/// for the last. Clamped to `[0, len - max]`.
///
/// Unlike [`menu_window`] — which only scrolls once the selection would leave
/// the window, pinning it to whichever edge it exited — this recenters on every
/// move, which is what stops the highlight getting stuck against the bottom row
/// of a long model list.
#[must_use]
pub fn centered_window(len: usize, selected: usize, max: usize) -> usize {
    if max == 0 || len <= max {
        return 0;
    }
    // Put the selection on the middle row, then clamp so the window never runs
    // off either end (`len - max` is safe: `len > max` here).
    selected.saturating_sub(max / 2).min(len - max)
}

/// The window of variable-height entries (entry `i` painting `heights[i]`
/// rows) that keeps `selected` **centered** inside a budget of `max_rows`
/// painted rows and at most `max_entries` entries — [`centered_window`] for
/// the palette's wrapped entries, returned as the visible `(start, end)`
/// entry range. Whole entries only, balanced by *rows*: each step adds the
/// next entry on whichever side of the selection holds fewer rows so far
/// (above on a tie, which is where `centered_window`'s middle row leaves the
/// odd row), and a side that has run out of entries — or whose next entry
/// no longer fits — leaves the rest of the budget to the other, which is
/// what anchors the window at the top and flush with the tail; the window
/// stops growing at whichever limit it reaches first. With uniform heights
/// this is [`centered_window`] over the `max_rows / height` whole entries
/// the budget holds (or `max_entries`, when that is fewer), exactly; a lone
/// entry taller than the whole budget still windows alone (the caller trims
/// its rows to the budget).
pub(super) fn centered_window_rows(
    heights: &[usize],
    selected: usize,
    max_rows: usize,
    max_entries: usize,
) -> (usize, usize) {
    if heights.is_empty() || max_rows == 0 || max_entries == 0 {
        return (0, 0);
    }
    let selected = selected.min(heights.len() - 1);
    let (mut start, mut end) = (selected, selected + 1);
    let (mut above, mut below) = (0, 0);
    loop {
        let rows = heights[selected] + above + below;
        let room = end - start < max_entries;
        let fits = |i: usize| room && rows + heights[i] <= max_rows;
        let up = start > 0 && fits(start - 1);
        let down = end < heights.len() && fits(end);
        if !(up || down) {
            return (start, end);
        }
        // The side holding fewer rows grows first (above on a tie); a side
        // that can't grow hands its turn to the other.
        if up && (above <= below || !down) {
            start -= 1;
            above += heights[start];
        } else {
            below += heights[end];
            end += 1;
        }
    }
}

/// A band's lone placeholder row — `No matching commands`, `Searching…` —
/// dim, and inset by [`MENU_INDENT`] so it says so in the column the names
/// would have filled rather than in the marker's gutter.
fn placeholder_line(text: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(MENU_INDENT),
        Span::styled(text.to_string(), Style::new().fg(menu_dim_color())),
    ])
}

/// One palette entry's rows: the [`MENU_MARKER`] on the selected entry (the
/// others open on [`MENU_INDENT`], its width in spaces, so every name starts in
/// one column), `/name` padded out to [`MENU_DESC_COL`], then its description
/// **word-wrapped** to the room past that column — a description wider than the
/// terminal continues on rows indented to the same column instead of being
/// silently cut (nothing the palette says is lost at a narrow width). The
/// selected entry also lights up whole in cyan, continuation rows included
/// (name bold), the others dimmed grey — the marker saying which row Enter
/// runs where the colour cannot. No background bar. A width with no
/// description room at all degrades to the marker and the name alone.
fn menu_row_lines(cmd: &SlashCommand, selected: bool, width: u16) -> Vec<Line<'static>> {
    let name = format!("/{}", cmd.name);
    // Name and description share one colour per entry, for consistency.
    let color = if selected {
        menu_selected_color()
    } else {
        menu_dim_color()
    };
    let name_style = if selected {
        Style::new().fg(color).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(color)
    };
    let desc_style = Style::new().fg(color);
    let marker = if selected { MENU_MARKER } else { MENU_INDENT };
    let marker_span = Span::styled(marker, desc_style);
    // Pad the name out to the description column so descriptions line up.
    let pad = " ".repeat(
        MENU_DESC_COL
            .saturating_sub(cols(marker) + cols(&name))
            .max(1),
    );
    let desc_width = (width as usize).saturating_sub(MENU_DESC_COL);
    if desc_width == 0 {
        // Too narrow for a description column at all: the marker and the
        // name (the file picker's marker-and-name degradation), `…`-cut to
        // the room left — codex's popup ellipsizes the name the same way
        // (`/statuslin…`).
        let room = (width as usize).saturating_sub(cols(marker)).max(1);
        return vec![Line::from(vec![
            marker_span,
            Span::styled(super::wrap::ellipsize(&name, room), name_style),
        ])];
    }
    let mut rows = wrap_text(&cmd.description, desc_width as u16).into_iter();
    let first = rows.next().unwrap_or_default();
    let mut lines = vec![Line::from(vec![
        marker_span,
        Span::styled(name, name_style),
        Span::raw(pad),
        Span::styled(first, desc_style),
    ])];
    lines.extend(rows.map(|row| {
        Line::from(vec![
            Span::raw(" ".repeat(MENU_DESC_COL)),
            Span::styled(row, desc_style),
        ])
    }));
    lines
}

/// The palette's dim `(n/total)` counter: which match the highlight is on,
/// of how many — the pickers' counter (`docs/ui/view-flow.md` *A picker's
/// page*), hanging under the command names at [`MENU_INDENT`] the way
/// theirs hangs under their rows' names. `None` on a terminal too narrow to
/// hold it whole, since a count cut at the edge would misread (`(1/2` for
/// one of twenty-one).
fn menu_counter_line(selected: usize, total: usize, width: u16) -> Option<Line<'static>> {
    counter_line(format!("({}/{total})", selected + 1), width)
}

/// A band's dim counter row: `counter` hanging at [`MENU_INDENT`] under the
/// names, or `None` where the width cannot hold it whole.
fn counter_line(counter: String, width: u16) -> Option<Line<'static>> {
    (cols(MENU_INDENT) + cols(&counter) <= usize::from(width)).then(|| {
        Line::from(vec![
            Span::raw(MENU_INDENT),
            Span::styled(counter, Style::new().fg(menu_dim_color())),
        ])
    })
}

/// The styled lines for the open command palette: the session's commands
/// ([`App::commands`] — the active model's speed-tier rows included) filtered
/// by the query, their descriptions **wrapped** to the width, windowed by
/// `centered_window_rows` to keep the selection **centered** among at most
/// `MENU_MAX_COMMANDS` of them — the `/model` and `/resume` lists' rule, so
/// the commands above *and* below the highlight stay in view as it scrolls —
/// over the `(n/total)` counter saying where the highlight is and how many
/// commands match. At widths where every description fits its row that is
/// five rows of commands; where descriptions wrap the window shows fewer
/// *whole* commands (scrolling reveals the rest) rather than clipping text
/// or growing the band past `MENU_MAX_ROWS`; or a single dim placeholder
/// when nothing matches. Empty when the palette is closed.
#[must_use]
pub fn command_menu_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(menu) = &app.command_menu else {
        return Vec::new();
    };
    let Some(query) = command_query(app.input.text()) else {
        return Vec::new();
    };
    let commands = app.commands();
    let matches = matching_commands(&commands, query);
    if matches.is_empty() {
        return vec![placeholder_line(MENU_NO_MATCH)];
    }
    // The commands' rows: the band's ceiling less the counter's row.
    let max_rows = usize::from(MENU_MAX_ROWS) - 1;
    let blocks: Vec<Vec<Line<'static>>> = matches
        .iter()
        .enumerate()
        .map(|(i, cmd)| menu_row_lines(cmd, i == menu.selected, width))
        .collect();
    let heights: Vec<usize> = blocks.iter().map(Vec::len).collect();
    let (start, end) = centered_window_rows(&heights, menu.selected, max_rows, MENU_MAX_COMMANDS);
    // `take(max_rows)` only ever bites on a lone entry taller than the whole
    // budget (a degenerately narrow terminal): its first rows show.
    let mut lines: Vec<Line<'static>> = blocks
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .flatten()
        .take(max_rows)
        .collect();
    let selected = menu.selected.min(matches.len() - 1);
    lines.extend(menu_counter_line(selected, matches.len(), width));
    lines
}

/// How many rows the `@` file picker occupies for `app` at `width`: **the
/// built line count** of [`file_menu_lines`] — 0 when closed, one placeholder
/// row while searching / when nothing matched, else the matches capped at
/// `FILE_MENU_MAX_ROWS` (longer lists scroll) and the `(n/total)` counter
/// under them, which a terminal too narrow to hold it whole goes without.
/// Height-is-the-line-count is [`menu_rows`]' rule: [`live_height`] adds this
/// via [`band_rows`] and [`render_live`] paints exactly this many rows, so the
/// two agree by construction. See `docs/input/file-search.md`.
#[must_use]
pub fn file_menu_rows(app: &App, width: u16) -> u16 {
    u16::try_from(file_menu_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// Append `text` to `spans`, bolding the characters the query matched: a
/// character at local byte `off` is emphasized when `base_off + off` appears in
/// `indices` (the byte offsets in the *whole* match path). Splitting a path
/// into name/parent columns reorders its pieces, so each column passes its own
/// `base_off` and the highlight follows the characters wherever they land
/// (truncation keeps the surviving prefix's offsets, so it stays aligned).
fn file_menu_highlight(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    base_off: usize,
    indices: &[usize],
    normal: Style,
    matched: Style,
) {
    let mut run = String::new();
    let mut run_matched = false;
    for (off, ch) in text.char_indices() {
        let is_match = indices.binary_search(&(base_off + off)).is_ok();
        if !run.is_empty() && is_match != run_matched {
            let style = if run_matched { matched } else { normal };
            spans.push(Span::styled(std::mem::take(&mut run), style));
        }
        run_matched = is_match;
        run.push(ch);
    }
    if !run.is_empty() {
        let style = if run_matched { matched } else { normal };
        spans.push(Span::styled(run, style));
    }
}

/// One file-picker row — the codex-style columns:
///
/// ```text
/// → public      ./                                          Dir
///   cv.pdf      public/assets/                              File
/// ```
///
/// The `→ ` marker on the selected row (the rest indent by its width), the
/// entry's *name*, its parent directory (`./` for a root-level entry, deeper
/// parents truncated to the column), and the `File`/`Dir` kind label pinned at
/// the right edge (`width − FILE_MENU_TYPE_WIDTH`). `name_col` is the shared
/// name-column width — the widest visible name plus [`FILE_MENU_GAP`], computed
/// by [`file_menu_lines`] so the parent column aligns across rows. The selected
/// row lights up cyan like the palette, the rest dim, and the characters the
/// query matched — `indices`, their byte offsets in the path, worked out by
/// [`file_menu_lines`] for the rows it draws — stay bolded across the split
/// columns. A width too narrow for the columns degrades to marker + name
/// alone.
fn file_menu_row(
    m: &FileMatch,
    indices: &[usize],
    selected: bool,
    name_col: usize,
    width: u16,
) -> Line<'static> {
    let color = if selected {
        menu_selected_color()
    } else {
        menu_dim_color()
    };
    let base = Style::new().fg(color);
    let matched = base.add_modifier(Modifier::BOLD);
    let marker = if selected { MENU_MARKER } else { MENU_INDENT };
    let mut spans = vec![Span::styled(marker, base)];
    let avail = (width as usize).saturating_sub(cols(marker));
    if avail < name_col + FILE_MENU_TYPE_WIDTH {
        // Too narrow for the parent/kind columns: just the (truncated) name.
        let name = truncate_cols(m.name(), avail);
        file_menu_highlight(&mut spans, &name, m.name_start(), indices, base, matched);
        return Line::from(spans);
    }
    let name = truncate_cols(m.name(), name_col);
    file_menu_highlight(&mut spans, &name, m.name_start(), indices, base, matched);
    spans.push(Span::styled(" ".repeat(name_col - cols(&name)), base));
    let dir_w = avail - name_col - FILE_MENU_TYPE_WIDTH;
    let parent = m.parent();
    if parent.is_empty() {
        // Root-level: a synthesized `./` no match byte can land on.
        let root = truncate_cols(FILE_MENU_ROOT_DIR, dir_w);
        let pad = " ".repeat(dir_w - cols(&root));
        spans.push(Span::styled(format!("{root}{pad}"), base));
    } else {
        let shown = truncate_cols(parent, dir_w);
        file_menu_highlight(&mut spans, &shown, 0, indices, base, matched);
        spans.push(Span::styled(" ".repeat(dir_w - cols(&shown)), base));
    }
    let kind = if m.is_dir() {
        FILE_MENU_DIR_LABEL
    } else {
        FILE_MENU_FILE_LABEL
    };
    spans.push(Span::styled(kind, base));
    Line::from(spans)
}

/// The styled lines for the open file picker: a *Searching…* / *No matching
/// files* placeholder while the band has no matches, else the file rows
/// windowed by [`centered_window`] — the `/` palette's window, so the
/// highlight rides the middle row and the files above *and* below it stay in
/// view — and capped at `FILE_MENU_MAX_ROWS`, over the palette's dim
/// `(n/total)` counter: which match is highlighted, of how many the search
/// found (every match, `docs/input/file-search.md`) — the total closed by
/// `FILE_MENU_PARTIAL_MARK` when the walk stopped at its cap, the list then
/// being only part of the workspace. Empty when the picker is closed.
#[must_use]
pub fn file_menu_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(fs) = &app.file_search else {
        return Vec::new();
    };
    if fs.matches.is_empty() {
        let text = if fs.waiting {
            FILE_MENU_SEARCHING
        } else {
            FILE_MENU_NO_MATCH
        };
        return vec![placeholder_line(text)];
    }
    let max = FILE_MENU_MAX_ROWS as usize;
    let offset = centered_window(fs.matches.len(), fs.selected, max);
    let visible = &fs.matches[offset..(offset + max).min(fs.matches.len())];
    // The shared name-column width: the widest *visible* name + the gap, so
    // the parent column starts at the same place on every shown row.
    let name_col = visible.iter().map(|m| cols(m.name())).max().unwrap_or(0) + FILE_MENU_GAP;
    let mut lines: Vec<Line<'static>> = visible
        .iter()
        .enumerate()
        .map(|(i, m)| {
            // What to bold, worked out for this row alone: a match keeps no
            // positions, since the list holds every match the walk found and
            // only these few are ever drawn. A row the query no longer
            // matches — the last search's, awaiting the next — bolds nothing.
            let indices = crate::file_search::fuzzy_match(&fs.query, &m.path)
                .map(|(_, indices)| indices)
                .unwrap_or_default();
            file_menu_row(m, &indices, offset + i == fs.selected, name_col, width)
        })
        .collect();
    let selected = fs.selected.min(fs.matches.len() - 1);
    let partial = if fs.truncated {
        FILE_MENU_PARTIAL_MARK
    } else {
        ""
    };
    let counter = format!("({}/{}{partial})", selected + 1, fs.matches.len());
    lines.extend(counter_line(counter, width));
    lines
}

/// How many rows the `$` skill picker occupies for `app` at `width`: **the
/// built line count** of [`skill_menu_lines`] — 0 when closed (which
/// includes the cursor having *left* the mention, since the matches derive
/// from the live token, [`App::skill_band_active`]), one placeholder row when
/// the query matches no skill, else the matches capped at
/// `SKILL_MENU_MAX_ROWS` (longer lists scroll) and the `(n/total)` counter
/// under them, which a terminal too narrow to hold it whole goes without.
/// Height-is-the-line-count is [`menu_rows`]' rule: [`live_height`] adds
/// this via [`band_rows`] and [`render_live`] paints exactly this many rows,
/// so the two agree by construction. See `docs/input/skill-mentions.md`.
#[must_use]
pub fn skill_menu_rows(app: &App, width: u16) -> u16 {
    u16::try_from(skill_menu_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// One skill-picker row: the marker (or its indent), the skill's name with
/// the query's matched characters bolded, and — past the shared name column —
/// its description, `…`-cut to the width:
///
/// ```text
///   dataviz        Generate or edit charts for websites, games, a…
/// → skill-creator  Create or update a skill
/// ```
///
/// `name_col` is the widest visible name plus [`FILE_MENU_GAP`], computed by
/// [`skill_menu_lines`] so the description column aligns across rows. The
/// selected row lights up cyan whole (the palette's convention), the rest
/// dim.
fn skill_menu_row(
    m: &crate::skills::SkillMatch,
    selected: bool,
    name_col: usize,
    width: u16,
) -> Line<'static> {
    let color = if selected {
        menu_selected_color()
    } else {
        menu_dim_color()
    };
    let base = Style::new().fg(color);
    let matched = base.add_modifier(Modifier::BOLD);
    let marker = if selected { MENU_MARKER } else { MENU_INDENT };
    let mut spans = vec![Span::styled(marker, base)];
    let avail = (width as usize).saturating_sub(cols(marker));
    let name = truncate_cols(&m.name, avail);
    file_menu_highlight(&mut spans, &name, 0, &m.indices, base, matched);
    let desc_room = avail.saturating_sub(name_col);
    if desc_room == 0 {
        // Too narrow for the description column: just the (truncated) name.
        return Line::from(spans);
    }
    spans.push(Span::styled(" ".repeat(name_col - cols(&name)), base));
    let desc = if cols(&m.description) > desc_room {
        // The mock's `…`-cut: a shortened description says it was.
        format!("{}…", truncate_cols(&m.description, desc_room - 1))
    } else {
        m.description.clone()
    };
    spans.push(Span::styled(desc, base));
    Line::from(spans)
}

/// The styled lines for the open skill picker: a *No matching skills*
/// placeholder when the query misses, else the skill rows windowed by
/// [`centered_window`] — the `/` palette's window, so the highlight rides
/// the middle row and the skills above *and* below it stay in view — and
/// capped at `SKILL_MENU_MAX_ROWS`, over the palette's dim `(n/total)`
/// counter: which match is highlighted, of how many the query matched.
/// Empty when the band is closed (or the cursor has left the mention). See
/// `docs/input/skill-mentions.md`.
#[must_use]
pub fn skill_menu_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    if !app.skill_band_active() {
        return Vec::new();
    }
    let matches = app.skill_matches();
    if matches.is_empty() {
        return vec![placeholder_line(SKILL_MENU_NO_MATCH)];
    }
    let selected = app
        .skill_picker
        .as_ref()
        .map_or(0, |picker| picker.selected);
    let max = SKILL_MENU_MAX_ROWS as usize;
    let offset = centered_window(matches.len(), selected, max);
    let visible = &matches[offset..(offset + max).min(matches.len())];
    // The shared name column: the widest visible name + the gap, so the
    // description column starts at the same place on every shown row.
    let name_col = visible.iter().map(|m| cols(&m.name)).max().unwrap_or(0) + FILE_MENU_GAP;
    let mut lines: Vec<Line<'static>> = visible
        .iter()
        .enumerate()
        .map(|(i, m)| skill_menu_row(m, offset + i == selected, name_col, width))
        .collect();
    lines.extend(menu_counter_line(
        selected.min(matches.len() - 1),
        matches.len(),
        width,
    ));
    lines
}

/// How many rows the `?` shortcuts band asks for, for `app` at `width`: 0
/// when closed, otherwise **the built line count** of the whole band
/// ([`shortcuts_lines`] with no height to cut it to) — as many columns as
/// `width` holds whole, and so as few rows as fit (`docs/ui/shortcuts.md`).
/// [`live_height`] adds this (via its band parameter); [`render_live`] paints
/// exactly this many rows wherever the terminal has them — the two agree by
/// construction, like [`menu_rows`] — and on a terminal too short for them it
/// paints the rows it was given, the last one counting what did not fit.
#[must_use]
pub fn shortcuts_rows(app: &App, width: u16) -> u16 {
    if !app.shortcuts_open {
        return 0;
    }
    let lines = shortcuts_lines(
        app.turn_active(),
        app.has_backtrack_target(),
        width,
        u16::MAX,
    );
    u16::try_from(lines.len()).unwrap_or(u16::MAX)
}

/// Total rows of the band below the input box: the slash-command palette, the
/// `?` shortcuts overview, the `@` file picker, *or* the `$` skill picker
/// (mutually exclusive — the palette needs a `/token`, the shortcuts an empty
/// composer, and each picker its own sigil opening the token, so at most one
/// term is non-zero). `width` sizes the palette's wrapped descriptions
/// ([`menu_rows`]) and the shortcuts' columns ([`shortcuts_rows`]). The
/// **one** band-height sum shared by [`render_live`], [`cursor_position`],
/// and the boundary's `live_region_height`, so the three can never drift.
#[must_use]
pub fn band_rows(app: &App, width: u16) -> u16 {
    menu_rows(app, width)
        + shortcuts_rows(app, width)
        + file_menu_rows(app, width)
        + skill_menu_rows(app, width)
}

/// The text a `SHORTCUTS` entry shows in a context state. Only the `esc`
/// entry changes, three ways (codex's quit entry): [`SHORTCUTS_INTERRUPT`]
/// while a turn is in flight, the [`SHORTCUTS_BACKTRACK`] `esc esc` edit hint
/// when idle with a previous user message to edit, and its own ` to quit`
/// only with nothing to backtrack to (docs/input/backtrack.md).
fn shortcut_shown(
    (key, label): (&'static str, &'static str),
    turn_active: bool,
    can_backtrack: bool,
) -> (&'static str, &'static str) {
    if key != "esc" {
        (key, label)
    } else if turn_active {
        SHORTCUTS_INTERRUPT
    } else if can_backtrack {
        SHORTCUTS_BACKTRACK
    } else {
        (key, label)
    }
}

/// A label without its leading ` for` / ` to` — ` for commands` →
/// ` commands`, ` to search history` → ` search history` — the terse
/// wording a narrow terminal gets when the full one would cost the band its
/// second column (`docs/ui/shortcuts.md`). Every label leads with one
/// (`every_label_leads_with_a_connective_the_terse_wording_drops`); one that
/// did not would keep its words.
fn terse_label(label: &'static str) -> &'static str {
    [" for", " to"]
        .into_iter()
        .find_map(|connective| {
            label
                .strip_prefix(connective)
                .filter(|rest| rest.starts_with(' '))
        })
        .unwrap_or(label)
}

/// The text a `SHORTCUTS` entry shows in a context state, in the full or
/// the terse wording.
fn shortcut_text(
    entry: (&'static str, &'static str),
    turn_active: bool,
    can_backtrack: bool,
    terse: bool,
) -> (&'static str, &'static str) {
    let (key, label) = shortcut_shown(entry, turn_active, can_backtrack);
    (key, if terse { terse_label(label) } else { label })
}

/// The widest text a `SHORTCUTS` entry ever shows in a wording, over every
/// context state — what a column holding it is sized by, so a context swap
/// under an open band never slides the columns after it.
fn shortcut_widest(entry: (&'static str, &'static str), terse: bool) -> usize {
    [(false, false), (true, false), (false, true)]
        .into_iter()
        .map(|(turn_active, can_backtrack)| {
            let (key, label) = shortcut_text(entry, turn_active, can_backtrack, terse);
            cols(key) + cols(label)
        })
        .max()
        .unwrap_or(0)
}

/// How the `?` band lays its entries out at one terminal width
/// (`docs/ui/shortcuts.md`): the `SHORTCUTS` entries run **down** each column
/// in order (column-major, like `ls`), the first columns one entry longer
/// when the count does not divide; a column is as wide as the widest text
/// any of its entries ever shows; two columns sit `gutter` blank columns
/// apart; and the labels keep their `for`/`to` unless `terse`.
struct ShortcutsGrid {
    /// The `SHORTCUTS` indices each column holds, top to bottom.
    columns: Vec<Range<usize>>,
    /// Each column's width, in display columns.
    widths: Vec<usize>,
    /// The blank columns between one column's width and the next column.
    gutter: usize,
    /// Whether the labels drop their leading ` for` / ` to` (`terse_label`).
    terse: bool,
}

impl ShortcutsGrid {
    /// `SHORTCUTS` split into `count` balanced columns in a wording, at no
    /// gutter yet.
    fn split(count: usize, terse: bool) -> Self {
        let total = SHORTCUTS.len();
        let count = count.clamp(1, total.max(1));
        let (base, extra) = (total / count, total % count);
        let mut start = 0;
        let columns: Vec<Range<usize>> = (0..count)
            .map(|c| {
                let len = base + usize::from(c < extra);
                let column = start..start + len;
                start += len;
                column
            })
            .collect();
        let widths = columns
            .iter()
            .map(|column| {
                SHORTCUTS[column.clone()]
                    .iter()
                    .map(|&entry| shortcut_widest(entry, terse))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        Self {
            columns,
            widths,
            gutter: 0,
            terse,
        }
    }

    /// `count` columns in a wording, if they fit `width` whole — inset, and
    /// at least [`SHORTCUTS_MIN_GUTTER`] apart — with the width they do not
    /// need shared out between the gutters, up to [`SHORTCUTS_MAX_GUTTER`].
    fn fitted(count: usize, terse: bool, width: u16) -> Option<Self> {
        let grid = Self::split(count, terse);
        let content: usize = grid.widths.iter().sum();
        let gaps = grid.columns.len() - 1;
        let need = cols(MENU_INDENT) + content + gaps * SHORTCUTS_MIN_GUTTER;
        let spare = usize::from(width).checked_sub(need)?;
        let gutter = if gaps == 0 {
            0
        } else {
            (SHORTCUTS_MIN_GUTTER + spare / gaps).min(SHORTCUTS_MAX_GUTTER)
        };
        Some(Self { gutter, ..grid })
    }

    /// The layout for a terminal `width` columns wide: the **fewest rows**
    /// whose columns fit the width whole — never fewer than
    /// [`SHORTCUTS_MIN_ROWS`], and for a given row count the fewest columns
    /// that make it — in the full wording. Where the full wording cannot
    /// make even two columns, the terse one (`terse_label`) tries the same
    /// counts, since dropping each `for`/`to` is what keeps a narrow pane's
    /// band at half the height; one column, where nothing else fits, is the
    /// full wording again — the terse one would save no row there — unless
    /// only the terse one fits whole. A terminal too narrow even for that
    /// still gets the one column; the paint's clip is all that is left to it.
    fn for_width(width: u16) -> Self {
        let total = SHORTCUTS.len();
        let mut counts: Vec<usize> = (SHORTCUTS_MIN_ROWS.clamp(1, total.max(1))..=total)
            .map(|rows| total.div_ceil(rows))
            .collect();
        counts.dedup(); // more rows, the same columns: measured once
        let (several, one): (Vec<usize>, Vec<usize>) =
            counts.into_iter().partition(|&count| count > 1);
        [
            (&several, false),
            (&several, true),
            (&one, false),
            (&one, true),
        ]
        .into_iter()
        .find_map(|(counts, terse)| {
            counts
                .iter()
                .find_map(|&count| Self::fitted(count, terse, width))
        })
        .unwrap_or_else(|| Self::split(1, true))
    }

    /// How many rows the layout takes: its first (longest) column's length.
    fn rows(&self) -> usize {
        self.columns.first().map_or(0, ExactSizeIterator::len)
    }

    /// How many entries sit on row `row` and below — what a band cut to
    /// `row` rows leaves out.
    fn entries_from(&self, row: usize) -> usize {
        self.columns
            .iter()
            .map(|column| column.len().saturating_sub(row))
            .sum()
    }

    /// Row `row` of the band: the inset, then the entry each column holds on
    /// that row, the key cyan and the label dim, each padded out to its
    /// column's width plus the gutter before the next. Padded from the text
    /// actually shown — a context swap can change the key too (`esc` →
    /// `esc esc`) — so the next column starts where it always does.
    fn line(&self, row: usize, turn_active: bool, can_backtrack: bool) -> Line<'static> {
        let mut spans = vec![Span::raw(MENU_INDENT)];
        let mut pad = None;
        for (column, &width) in self.columns.iter().zip(&self.widths) {
            if row >= column.len() {
                break; // the columns only shorten to the right
            }
            let entry = SHORTCUTS[column.start + row];
            let (key, label) = shortcut_text(entry, turn_active, can_backtrack, self.terse);
            if let Some(pad) = pad {
                spans.push(Span::raw(" ".repeat(pad)));
            }
            spans.push(Span::styled(key, Style::new().fg(shortcuts_key_color())));
            spans.push(Span::styled(label, Style::new().fg(shortcuts_text_color())));
            pad = Some(width.saturating_sub(cols(key) + cols(label)) + self.gutter);
        }
        Line::from(spans)
    }
}

/// The styled lines for the open shortcuts band at `width`, cut to `height`
/// rows: the `SHORTCUTS` entries inset by `MENU_INDENT` — the band takes the
/// footer's slot, so it starts where the footer does, in the column every
/// band's names start at — laid out by `ShortcutsGrid::for_width` in as
/// many columns as the width holds whole, keys cyan and labels dim. A
/// narrow terminal gets the labels without their `for`/`to` where that
/// keeps two columns, and one entry per row under that — never an entry cut
/// mid-word, which is what the paint's clip did to a fixed second column
/// once (`docs/ui/shortcuts.md`). The `esc` entry follows the context
/// (`shortcut_shown`).
///
/// A band with more rows than `height` — a terminal too short for the whole
/// band, where the region gave it fewer rows than [`shortcuts_rows`] asked
/// for — shows the rows that fit but the last, and on that last row a dim
/// `… +N more` counting the entries left out, rather than losing the bottom
/// of the list (the way out among it) without a word.
#[must_use]
pub fn shortcuts_lines(
    turn_active: bool,
    can_backtrack: bool,
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let grid = ShortcutsGrid::for_width(width);
    let rows = grid.rows();
    let height = usize::from(height);
    let shown = if rows <= height {
        rows
    } else {
        height.saturating_sub(1)
    };
    let mut lines: Vec<Line<'static>> = (0..shown)
        .map(|row| grid.line(row, turn_active, can_backtrack))
        .collect();
    if shown < rows && height > 0 {
        lines.push(Line::from(vec![
            Span::raw(MENU_INDENT),
            Span::styled(
                format!("{SHORTCUTS_MORE_MARKER} +{} more", grid.entries_from(shown)),
                Style::new().fg(shortcuts_text_color()),
            ),
        ]));
    }
    lines
}
