//! The bands below the input box: the slash-command palette, the `@` file
//! picker, and the `?` shortcuts overlay.
//! See `docs/file-search.md` and `docs/shortcuts.md`.

use super::theme::*;
use super::wrap::{cols, truncate_cols, wrap_text};
use super::*;

/// How many rows the command palette occupies for `app` at `width`: 0 when
/// closed, otherwise **the built line count** of [`command_menu_lines`] — the
/// row-budget window over the matches, wrapped-description continuation rows
/// included, never more than `MENU_MAX_ROWS` (or a single placeholder row
/// when the query matches nothing). Height-is-the-line-count is the
/// settings/model pickers' rule (`docs/view-flow.md`): the reserved rows and
/// the painted rows agree by construction, which is what lets a long
/// description *wrap* instead of clipping at the width. [`live_height`] adds
/// this; [`render_live`] paints exactly this many rows.
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

/// The window of variable-height entries (entry `i` painting `heights[i]`
/// rows) that keeps `selected` visible inside a budget of `max_rows` painted
/// rows — [`menu_window`] for the palette's wrapped entries, returned as the
/// visible `(start, end)` entry range. The window grows **upward** from the
/// selection first (the fixed window follows the selection toward the end,
/// pinning it at the window's bottom edge), then fills any leftover budget
/// downward (the tail clamp: near the end the fixed window shows rows below
/// the selection too). With uniform heights of 1 this reproduces
/// [`menu_window`] exactly; a lone entry taller than the whole budget still
/// windows alone (the caller trims its rows to the budget).
pub(super) fn menu_window_rows(
    heights: &[usize],
    selected: usize,
    max_rows: usize,
) -> (usize, usize) {
    if heights.is_empty() || max_rows == 0 {
        return (0, 0);
    }
    let selected = selected.min(heights.len() - 1);
    let mut start = selected;
    let mut rows = heights[selected];
    while start > 0 && rows + heights[start - 1] <= max_rows {
        start -= 1;
        rows += heights[start];
    }
    let mut end = selected + 1;
    while end < heights.len() && rows + heights[end] <= max_rows {
        rows += heights[end];
        end += 1;
    }
    (start, end)
}

/// The scroll offset (first visible match index) that keeps `selected`
/// **centered** in a window of `max` rows: the selection rides the middle row
/// (`max/2`) while there is room on both sides, so the user always sees as much
/// of the list *above and below* the highlight as fits — the "broad view" the
/// `/model` and `/login` pickers want. Near the ends the window can't center
/// (there aren't enough rows on one side), so it anchors: the top for the first
/// `max/2` selections, the bottom (flush with the tail) for the last. Clamped to
/// `[0, len - max]`.
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

/// One palette entry's rows: `/name` padded out to [`MENU_DESC_COL`] columns,
/// then its description **word-wrapped** to the room past that column — a
/// description wider than the terminal continues on rows indented to the same
/// column instead of being silently cut (nothing the palette says is lost at a
/// narrow width). The selection is shown by **colour** — the selected entry
/// lights up whole in cyan, continuation rows included (name bold), the others
/// are dimmed grey. No caret, no background bar. A width with no description
/// room at all degrades to the name alone.
fn menu_row_lines(cmd: &SlashCommand, selected: bool, width: u16) -> Vec<Line<'static>> {
    let name = format!("/{}", cmd.name);
    // Name and description share one colour per entry, for consistency.
    let color = if selected {
        MENU_SELECTED_COLOR
    } else {
        MENU_DIM_COLOR
    };
    let name_style = if selected {
        Style::new().fg(color).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(color)
    };
    let desc_style = Style::new().fg(color);
    // Pad the name out to the description column so descriptions line up.
    let pad = " ".repeat(MENU_DESC_COL.saturating_sub(cols(&name)).max(1));
    let desc_width = (width as usize).saturating_sub(MENU_DESC_COL);
    if desc_width == 0 {
        // Too narrow for a description column at all: the name alone (the
        // file picker's marker-and-name degradation), `…`-cut to the width
        // — codex's popup ellipsizes the name the same way (`/statuslin…`).
        return vec![Line::from(Span::styled(
            super::wrap::ellipsize(&name, (width as usize).max(1)),
            name_style,
        ))];
    }
    let mut rows = wrap_text(cmd.description, desc_width as u16).into_iter();
    let first = rows.next().unwrap_or_default();
    let mut lines = vec![Line::from(vec![
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

/// The styled lines for the open command palette: the filtered commands with
/// their descriptions **wrapped** to the width, windowed by
/// `menu_window_rows` to keep the selection visible inside the
/// `MENU_MAX_ROWS` **row budget** — at widths where every description fits
/// its row that is the familiar eight commands, and where descriptions wrap
/// the band shows fewer *whole* commands (scrolling reveals the rest) rather
/// than clipping text or growing under the box; or a single dim placeholder
/// when nothing matches. Empty when the palette is closed.
#[must_use]
pub fn command_menu_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(menu) = &app.command_menu else {
        return Vec::new();
    };
    let Some(query) = command_query(app.input.text()) else {
        return Vec::new();
    };
    let matches = matching_commands(query);
    if matches.is_empty() {
        return vec![Line::from(Span::styled(
            MENU_NO_MATCH.to_string(),
            Style::new().fg(MENU_DIM_COLOR),
        ))];
    }
    let max = MENU_MAX_ROWS as usize;
    let blocks: Vec<Vec<Line<'static>>> = matches
        .iter()
        .enumerate()
        .map(|(i, cmd)| menu_row_lines(cmd, i == menu.selected, width))
        .collect();
    let heights: Vec<usize> = blocks.iter().map(Vec::len).collect();
    let (start, end) = menu_window_rows(&heights, menu.selected, max);
    // `take(max)` only ever bites on a lone entry taller than the whole
    // budget (a degenerately narrow terminal): its first rows show.
    blocks
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .flatten()
        .take(max)
        .collect()
}

/// How many rows the `@` file picker occupies for `app`: 0 when closed, one
/// placeholder row while searching / when nothing matched, else the match count
/// capped at `FILE_MENU_MAX_ROWS` (longer lists scroll, like the palette).
/// [`live_height`] adds this to its band and [`render_live`] paints exactly this
/// many rows — the two must agree. See `docs/file-search.md`.
#[must_use]
pub fn file_menu_rows(app: &App) -> u16 {
    match &app.file_search {
        None => 0,
        Some(fs) if fs.matches.is_empty() => 1,
        Some(fs) => (fs.matches.len() as u16).min(FILE_MENU_MAX_ROWS),
    }
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
/// row lights up cyan like the palette, the rest dim, and the query's matched
/// characters ([`FileMatch::indices`]) stay bolded across the split columns.
/// A width too narrow for the columns degrades to marker + name alone.
fn file_menu_row(m: &FileMatch, selected: bool, name_col: usize, width: u16) -> Line<'static> {
    let color = if selected {
        MENU_SELECTED_COLOR
    } else {
        MENU_DIM_COLOR
    };
    let base = Style::new().fg(color);
    let matched = base.add_modifier(Modifier::BOLD);
    let marker = if selected {
        FILE_MENU_MARKER
    } else {
        FILE_MENU_INDENT
    };
    let mut spans = vec![Span::styled(marker, base)];
    let avail = (width as usize).saturating_sub(cols(marker));
    if avail < name_col + FILE_MENU_TYPE_WIDTH {
        // Too narrow for the parent/kind columns: just the (truncated) name.
        let name = truncate_cols(m.name(), avail);
        file_menu_highlight(&mut spans, &name, m.name_start(), &m.indices, base, matched);
        return Line::from(spans);
    }
    let name = truncate_cols(m.name(), name_col);
    file_menu_highlight(&mut spans, &name, m.name_start(), &m.indices, base, matched);
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
        file_menu_highlight(&mut spans, &shown, 0, &m.indices, base, matched);
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
/// files* placeholder while the band has no matches, else the file rows windowed
/// (`menu_window`) to keep the selection visible and capped at
/// `FILE_MENU_MAX_ROWS`. Empty when the picker is closed.
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
        return vec![Line::from(Span::styled(
            text.to_string(),
            Style::new().fg(MENU_DIM_COLOR),
        ))];
    }
    let max = FILE_MENU_MAX_ROWS as usize;
    let offset = menu_window(fs.matches.len(), fs.selected, max);
    let visible = &fs.matches[offset..(offset + max).min(fs.matches.len())];
    // The shared name-column width: the widest *visible* name + the gap, so
    // the parent column starts at the same place on every shown row.
    let name_col = visible.iter().map(|m| cols(m.name())).max().unwrap_or(0) + FILE_MENU_GAP;
    visible
        .iter()
        .enumerate()
        .map(|(i, m)| file_menu_row(m, offset + i == fs.selected, name_col, width))
        .collect()
}

/// How many rows the `$` skill picker occupies for `app`: 0 when closed —
/// which includes the cursor having *left* the mention, since the matches
/// derive from the live token ([`App::skill_band_active`]) — one placeholder
/// row when the query matches no skill, else the match count capped at
/// `SKILL_MENU_MAX_ROWS` (longer lists scroll, like the palette).
/// [`live_height`] adds this via [`band_rows`] and [`render_live`] paints
/// exactly this many rows — the two must agree. See `docs/skill-mentions.md`.
#[must_use]
pub fn skill_menu_rows(app: &App) -> u16 {
    if !app.skill_band_active() {
        return 0;
    }
    let matches = app.skill_matches().len();
    if matches == 0 {
        1
    } else {
        (matches as u16).min(SKILL_MENU_MAX_ROWS)
    }
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
        MENU_SELECTED_COLOR
    } else {
        MENU_DIM_COLOR
    };
    let base = Style::new().fg(color);
    let matched = base.add_modifier(Modifier::BOLD);
    let marker = if selected {
        FILE_MENU_MARKER
    } else {
        FILE_MENU_INDENT
    };
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
/// placeholder when the query misses, else the skill rows windowed
/// (`menu_window`) to keep the selection visible and capped at
/// `SKILL_MENU_MAX_ROWS`. Empty when the band is closed (or the cursor has
/// left the mention). See `docs/skill-mentions.md`.
#[must_use]
pub fn skill_menu_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    if !app.skill_band_active() {
        return Vec::new();
    }
    let matches = app.skill_matches();
    if matches.is_empty() {
        return vec![Line::from(Span::styled(
            SKILL_MENU_NO_MATCH.to_string(),
            Style::new().fg(MENU_DIM_COLOR),
        ))];
    }
    let selected = app
        .skill_picker
        .as_ref()
        .map_or(0, |picker| picker.selected);
    let max = SKILL_MENU_MAX_ROWS as usize;
    let offset = menu_window(matches.len(), selected, max);
    let visible = &matches[offset..(offset + max).min(matches.len())];
    // The shared name column: the widest visible name + the gap, so the
    // description column starts at the same place on every shown row.
    let name_col = visible.iter().map(|m| cols(&m.name)).max().unwrap_or(0) + FILE_MENU_GAP;
    visible
        .iter()
        .enumerate()
        .map(|(i, m)| skill_menu_row(m, offset + i == selected, name_col, width))
        .collect()
}

/// Whether the first row has room for the [`SHORTCUTS_SKILLS`] third column
/// at `width`: the entry must fit whole past [`SHORTCUTS_THIRD_COL`] — the
/// band never clips what it teaches, so a narrow terminal moves the entry to
/// its own row instead ([`shortcuts_lines`] / [`shortcuts_rows`] share this
/// verdict, which is what keeps the reserved and painted rows agreeing).
fn shortcuts_third_column_fits(width: u16) -> bool {
    let (key, label) = SHORTCUTS_SKILLS;
    SHORTCUTS_THIRD_COL + cols(key) + cols(label) <= width as usize
}

/// How many rows the `?` shortcuts band occupies for `app` at `width`: 0 when
/// closed, otherwise the entry list two-per-row — plus one when the terminal
/// is too narrow for the first row's `SHORTCUTS_SKILLS` third column, which
/// then takes its own row. [`live_height`] adds this (via its band
/// parameter); [`render_live`] paints exactly this many rows — the two must
/// agree, like [`menu_rows`].
#[must_use]
pub fn shortcuts_rows(app: &App, width: u16) -> u16 {
    if app.shortcuts_open {
        SHORTCUTS.len().div_ceil(2) as u16 + u16::from(!shortcuts_third_column_fits(width))
    } else {
        0
    }
}

/// Total rows of the band below the input box: the slash-command palette, the
/// `?` shortcuts overview, the `@` file picker, *or* the `$` skill picker
/// (mutually exclusive — the palette needs a `/token`, the shortcuts an empty
/// composer, and each picker its own sigil opening the token, so at most one
/// term is non-zero). `width` sizes the palette's wrapped descriptions
/// ([`menu_rows`]). The **one** band-height sum shared by [`render_live`],
/// [`cursor_position`], and the boundary's `live_region_height`, so the three
/// can never drift.
#[must_use]
pub fn band_rows(app: &App, width: u16) -> u16 {
    menu_rows(app, width) + shortcuts_rows(app, width) + file_menu_rows(app) + skill_menu_rows(app)
}

/// The styled lines for the open shortcuts band: the `SHORTCUTS` entries two
/// per row — the second column starting at `SHORTCUTS_COL` — with keys cyan
/// and labels dim, and the `SHORTCUTS_SKILLS` `$` entry as a **third
/// column** on the first row (at `SHORTCUTS_THIRD_COL`, beside its sibling
/// composer sigils `/` and `!`) when `width` has room for it whole — else on
/// its own last row (the band never clips what it teaches). The `esc` entry
/// is three-way context-sensitive (codex's quit entry): ` to interrupt`
/// while a turn is in flight, the `SHORTCUTS_BACKTRACK` `esc esc` edit hint
/// when idle with a previous user message to edit, and ` to quit` only with
/// nothing to backtrack to (docs/backtrack.md).
#[must_use]
pub fn shortcuts_lines(turn_active: bool, can_backtrack: bool, width: u16) -> Vec<Line<'static>> {
    let entry = |key: &'static str, label: &'static str| {
        let (key, label) = if key == "esc" && turn_active {
            (key, " to interrupt")
        } else if key == "esc" && can_backtrack {
            SHORTCUTS_BACKTRACK
        } else {
            (key, label)
        };
        [
            Span::styled(key, Style::new().fg(SHORTCUTS_KEY_COLOR)),
            Span::styled(label, Style::new().fg(SHORTCUTS_TEXT_COLOR)),
        ]
    };
    let third_fits = shortcuts_third_column_fits(width);
    let (skills_key, skills_label) = SHORTCUTS_SKILLS;
    let mut lines: Vec<Line<'static>> = SHORTCUTS
        .chunks(2)
        .enumerate()
        .map(|(row, pair)| {
            let [key, label] = entry(pair[0].0, pair[0].1);
            let mut spans = vec![key, label];
            if let Some(&(key2, label2)) = pair.get(1) {
                // Pad from the *displayed* widths — a context swap can change
                // the key text too (`esc` → `esc esc`).
                let used = cols(spans[0].content.as_ref()) + cols(spans[1].content.as_ref());
                spans.push(Span::raw(
                    " ".repeat(SHORTCUTS_COL.saturating_sub(used).max(1)),
                ));
                spans.extend(entry(key2, label2));
            }
            if row == 0 && third_fits {
                let used: usize = spans.iter().map(|s| cols(s.content.as_ref())).sum();
                spans.push(Span::raw(
                    " ".repeat(SHORTCUTS_THIRD_COL.saturating_sub(used).max(1)),
                ));
                spans.extend(entry(skills_key, skills_label));
            }
            Line::from(spans)
        })
        .collect();
    if !third_fits {
        let [key, label] = entry(skills_key, skills_label);
        lines.push(Line::from(vec![key, label]));
    }
    lines
}
