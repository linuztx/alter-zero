//! The bands below the box: palette, `@` picker, `?` shortcuts
//! (`docs/file-search.md`, `docs/shortcuts.md`).

use super::*;
use crate::ui::theme::{
    FILE_MENU_MAX_ROWS, FOOTER_INDENT, MENU_DESC_COL, MENU_INDENT, MENU_MARKER, MENU_MAX_ROWS,
    SHORTCUTS, SHORTCUTS_BACKTRACK, SHORTCUTS_INTERRUPT, SHORTCUTS_MAX_GUTTER,
    SHORTCUTS_MIN_GUTTER, SHORTCUTS_MIN_ROWS, SKILL_MENU_MAX_ROWS, menu_selected_color,
    shortcuts_key_color, shortcuts_text_color,
};
use crate::ui::wrap::cols;

#[test]
fn menu_window_keeps_the_selection_visible() {
    assert_eq!(menu_window(8, 0, 5), 0);
    assert_eq!(menu_window(8, 4, 5), 0, "within the first window");
    assert_eq!(menu_window(8, 5, 5), 1, "scrolls so the selection shows");
    assert_eq!(menu_window(8, 7, 5), 3, "clamped to the last window");
    assert_eq!(menu_window(3, 2, 5), 0, "no scroll when everything fits");
}

#[test]
fn centered_window_keeps_the_selection_centered() {
    // Everything fits in one window — never scrolls.
    assert_eq!(
        centered_window(3, 2, 10),
        0,
        "no scroll when everything fits"
    );
    assert_eq!(centered_window(10, 9, 10), 0, "an exact fit never scrolls");
    // Near the top of a long list the window is anchored at the top (it can't
    // center a selection with too few rows above it); the highlight walks down
    // to the middle row (max/2).
    assert_eq!(centered_window(445, 0, 10), 0, "top of the list");
    assert_eq!(
        centered_window(445, 4, 10),
        0,
        "still climbing to the middle"
    );
    assert_eq!(
        centered_window(445, 5, 10),
        0,
        "reaches the middle row (max/2)"
    );
    // In the interior the window follows the selection so it stays centered —
    // the fix: broad view above *and* below, not pinned to the bottom edge.
    assert_eq!(centered_window(445, 20, 10), 15, "stays centered mid-list");
    assert_eq!(centered_window(445, 60, 10), 55, "stays centered mid-list");
    // Near the end the window clamps flush with the tail; the highlight rides
    // down from the middle to the bottom row (it can't center past the end).
    assert_eq!(
        centered_window(445, 440, 10),
        435,
        "clamped to the last window"
    );
    assert_eq!(centered_window(445, 444, 10), 435, "last row sits flush");
    // A degenerate zero-height window never panics.
    assert_eq!(centered_window(5, 3, 0), 0, "zero window");
}

/// A palette row's text after its marker — `/name    description` — or
/// `None` for a row that starts no command (a wrapped description's
/// continuation, a placeholder).
fn command_text(row: &str) -> Option<&str> {
    [MENU_MARKER, MENU_INDENT]
        .iter()
        .find_map(|marker| row.strip_prefix(marker))
        .filter(|rest| rest.starts_with('/'))
}

/// The palette rows that *start* a command (the marker or its inset, then the
/// `/name`), as plain text — a wrapped description's continuation rows are
/// excluded, so a count of these is a count of the **commands** shown.
fn command_rows(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .filter(|row| command_text(row).is_some())
        .collect()
}

#[test]
fn menu_rows_is_zero_when_the_palette_is_closed() {
    assert_eq!(menu_rows(&App::new(), 60), 0);
}

#[test]
fn the_palette_shows_at_most_eight_commands() {
    // The requested cap (the file picker's): a bare `/` shows the first eight
    // commands and longer match lists scroll (centered_window_rows) instead of
    // growing the band — the registry has outgrown the window, so /quit (the
    // last) starts off-window (smoke.sh Phase 4 asserts the same on the real
    // binary). The cap counts **commands**, not rows: a wrapped description's
    // continuation rows ride under their command without costing a slot.
    assert_eq!(MENU_MAX_ROWS, 8, "the requested cap");
    assert!(
        crate::app::COMMANDS.len() > MENU_MAX_ROWS as usize,
        "the registry outgrew the window — scrolling is exercised by a bare `/`"
    );
    // At the standard 80 columns every (concise) description fits its row, so
    // the row budget shows exactly eight one-row commands — the smoke pane.
    let lines = command_menu_lines(&palette("/", 0), 80);
    let texts = command_rows(&lines);
    assert_eq!(texts.len(), MENU_MAX_ROWS as usize, "{texts:?}");
    assert_eq!(
        lines.len(),
        texts.len(),
        "one row per command at 80 columns"
    );
    assert!(texts[0].contains("/clear"), "{texts:?}");
    assert!(
        !texts.iter().any(|t| t.contains("/quit")),
        "the ninth command starts off-window: {texts:?}"
    );
}

#[test]
fn the_palette_scrolls_down_to_the_last_command() {
    // Walking the selection to the last command scrolls the list with it: the
    // band still shows MENU_MAX_ROWS rows, the top scrolled off, and — the
    // list having run out below it — the highlight slid on from the middle
    // row to the bottom one, cyan.
    let last = crate::app::COMMANDS.len() - 1;
    let lines = command_menu_lines(&palette("/", last), 80);
    let texts = command_rows(&lines);
    assert_eq!(texts.len(), MENU_MAX_ROWS as usize);
    assert!(
        !texts.iter().any(|t| t.contains("/clear")),
        "the first command scrolled out: {texts:?}"
    );
    let bottom = lines.last().expect("a windowed row");
    assert!(plain(bottom).contains("/quit"), "{texts:?}");
    assert!(
        bottom
            .spans
            .iter()
            .any(|s| s.style.fg == Some(menu_selected_color())),
        "the selection rode the window down"
    );
}

/// The band row the highlight starts on: the selected entry paints every
/// row in the selection colour, the others never do.
fn highlighted_row(lines: &[Line<'_>]) -> usize {
    lines
        .iter()
        .position(|l| {
            l.spans
                .iter()
                .any(|s| s.style.fg == Some(menu_selected_color()))
        })
        .expect("a highlighted row")
}

#[test]
fn the_palette_keeps_the_highlight_centered_as_it_scrolls() {
    // The `/model` and `/resume` lists' rule (`centered_window`): ↓ walks the
    // highlight down to the band's middle row, then the list scrolls under
    // it, so the commands above *and* below it stay in view and every step
    // scrolls the next one in. Only the list running out lets it slide on to
    // the bottom edge. The edge-pinned window this replaced held the
    // highlight on the bottom row all the way down, showing nothing of what
    // came next.
    let max = MENU_MAX_ROWS as usize;
    let middle = max / 2;
    let len = palette("/", 0).commands().len();
    assert!(len > max, "the registry outgrew the window: {len} commands");
    for selected in 0..len {
        let lines = command_menu_lines(&palette("/", selected), 80);
        assert_eq!(lines.len(), max, "a full window at {selected}");
        let expected = if selected < middle {
            selected // anchored at the top, still walking down to the middle
        } else if selected + (max - middle - 1) >= len {
            selected - (len - max) // flush with the tail
        } else {
            middle
        };
        assert_eq!(
            highlighted_row(&lines),
            expected,
            "selection {selected}: {:?}",
            command_rows(&lines)
        );
    }
}

#[test]
fn the_wrapped_palette_keeps_a_command_on_each_side_of_the_highlight() {
    // Where descriptions wrap the band shows fewer, taller entries, and the
    // window centers the highlight by *rows*. At 60 columns every entry is
    // one or two rows, so three always fit the eight-row budget and the
    // highlight never sits on the band's edge while a command remains on
    // that side. The edge-pinned window filled the band from the highlight
    // upward and left nothing below it.
    let len = palette("/", 0).commands().len();
    for selected in 1..len - 1 {
        let lines = command_menu_lines(&palette("/", selected), 60);
        assert!(lines.len() <= MENU_MAX_ROWS as usize, "{lines:?}");
        let row = highlighted_row(&lines);
        let above = command_rows(&lines[..row]).len();
        let below = command_rows(&lines[row + 1..]).len();
        assert!(
            above >= 1 && below >= 1,
            "selection {selected}: {above} above, {below} below: {:?}",
            command_rows(&lines)
        );
    }
}

#[test]
fn menu_rows_equals_the_painted_lines_at_every_width() {
    // The reserved band height IS the built line count — the settings/model
    // pickers' "height is the line count" rule (docs/view-flow.md) — so a
    // wrapped description can never paint more rows than were reserved.
    for width in [24u16, 40, 60, 80, 120] {
        let app = palette("/", 0);
        assert_eq!(
            usize::from(menu_rows(&app, width)),
            command_menu_lines(&app, width).len(),
            "menu_rows agrees with the painted lines at width {width}"
        );
    }
    assert_eq!(menu_rows(&palette("/zzz", 0), 60), 1, "placeholder row");
}

#[test]
fn the_palette_wraps_a_long_description_instead_of_clipping() {
    // A description wider than the room past MENU_DESC_COL used to be
    // silently truncated (no ellipsis, no wrap — the text just ended). It
    // now word-wraps onto continuation rows indented to the description
    // column, so nothing is lost at narrow widths.
    let app = palette("/copy", 0);
    let lines = command_menu_lines(&app, 40); // room for 13 desc columns
    assert!(lines.len() > 1, "the description wrapped: {lines:?}");
    assert!(
        plain(&lines[0]).starts_with("→ /copy"),
        "{:?}",
        plain(&lines[0])
    );
    // Every row fits the width…
    for line in &lines {
        assert!(cols(plain(line).trim_end()) <= 40, "{:?}", plain(line));
    }
    // …the continuation rows start at the description column…
    for cont in &lines[1..] {
        let text = plain(cont);
        let lead = cols(&text[..text.find(|c: char| !c.is_whitespace()).unwrap()]);
        assert_eq!(lead, MENU_DESC_COL, "continuation aligned: {text:?}");
    }
    // …and joining the pieces reconstructs the whole description.
    let cmd = crate::app::COMMANDS
        .iter()
        .find(|c| c.name == "copy")
        .expect("/copy is registered");
    let desc_parts: Vec<String> = std::iter::once(
        plain(&lines[0])[plain(&lines[0]).find("Copy").expect("the description")..]
            .trim_end()
            .to_string(),
    )
    .chain(lines[1..].iter().map(|l| plain(l).trim().to_string()))
    .collect();
    assert_eq!(desc_parts.join(" "), cmd.description, "nothing clipped");
}

#[test]
fn centered_window_rows_is_centered_window_for_uniform_heights() {
    // With every entry the same height the variable-height window IS the
    // `/model` list's `centered_window` over the whole entries the budget
    // holds — same offsets, same top and tail anchors — so the 80-column
    // palette, every command one row, scrolls exactly as `/model` and
    // `/resume` do. (A budget below one entry's height is the lone-tall-entry
    // case, pinned by the next test.)
    use crate::ui::menu::centered_window_rows;
    for height in 1..=3usize {
        for max_rows in height..=12 {
            let max = max_rows / height;
            for len in 0..=24 {
                let heights = vec![height; len];
                for selected in 0..len.max(1) {
                    let offset = centered_window(len, selected, max);
                    assert_eq!(
                        centered_window_rows(&heights, selected, max_rows),
                        (offset, (offset + max).min(len)),
                        "height {height}, budget {max_rows}, len {len}, selected {selected}"
                    );
                }
            }
        }
    }
}

#[test]
fn centered_window_rows_balances_wrapped_entries_by_rows() {
    use crate::ui::menu::centered_window_rows;
    // Two-row entries in an eight-row budget: four whole entries fit, and
    // mid-list the selection is the third of them — two above, one below,
    // `centered_window`'s middle for a four-entry window — anchoring at the
    // top and flush with the tail only where the list runs out.
    let heights = vec![2usize; 9];
    assert_eq!(centered_window_rows(&heights, 0, 8), (0, 4), "top anchor");
    assert_eq!(centered_window_rows(&heights, 4, 8), (2, 6), "centered");
    assert_eq!(centered_window_rows(&heights, 8, 8), (5, 9), "tail anchor");
    // Mixed heights balance *rows*, not entries: a three-row entry above
    // the selection is answered by three one-row entries below it, so the
    // one-row selection lands on the band's fifth row — exactly where
    // `centered_window` seats it in an eight-row window.
    let mixed = [1, 1, 1, 3, 1, 1, 1, 1, 1];
    let (start, end) = centered_window_rows(&mixed, 4, 8);
    assert_eq!((start, end), (2, 8));
    assert_eq!(mixed[start..4].iter().sum::<usize>(), 4, "rows above");
    assert_eq!(mixed[5..end].iter().sum::<usize>(), 3, "rows below");
    // An entry too tall to join on one side leaves the budget to the other
    // rather than stopping short: the eight-row entry under the selection
    // never fits beside it, so every entry above it does.
    assert_eq!(centered_window_rows(&[1, 1, 1, 1, 1, 1, 8], 5, 8), (0, 6));
    // A lone entry taller than the whole budget still windows alone (the
    // caller trims its rows); empty and zero-budget degenerate to nothing.
    assert_eq!(centered_window_rows(&[12, 1], 0, 8), (0, 1));
    assert_eq!(centered_window_rows(&[1, 12], 1, 8), (1, 2));
    assert_eq!(centered_window_rows(&[], 0, 8), (0, 0));
    assert_eq!(centered_window_rows(&[1, 1], 0, 0), (0, 0));
}

#[test]
fn the_narrow_palette_windows_fewer_commands_inside_the_row_budget() {
    // At 40 columns the descriptions wrap to multi-row entries; the band
    // keeps the MENU_MAX_ROWS row budget by windowing fewer *whole* commands
    // (↑/↓ scroll the rest in) instead of growing under the box — which is
    // what keeps the input box's rows (and the cursor) untouched on a short
    // terminal.
    let lines = command_menu_lines(&palette("/", 0), 40);
    assert!(
        lines.len() <= MENU_MAX_ROWS as usize,
        "the band held its budget: {} rows",
        lines.len()
    );
    let cmds = command_rows(&lines);
    assert!(cmds[0].contains("/clear"), "{cmds:?}");
    assert!(
        cmds.len() < MENU_MAX_ROWS as usize,
        "wrapped entries cost window slots: {cmds:?}"
    );
    // Scrolled to the last command it is still the window's bottom entry.
    let last = crate::app::COMMANDS.len() - 1;
    let scrolled = command_menu_lines(&palette("/", last), 40);
    assert!(scrolled.len() <= MENU_MAX_ROWS as usize);
    assert!(
        command_rows(&scrolled)
            .last()
            .expect("a command row")
            .contains("/quit"),
        "the selection stays visible"
    );
}

#[test]
fn the_palette_lists_the_active_models_speed_tiers_after_model() {
    // The tier rows are ordinary palette entries (docs/fast-mode.md): one per
    // tier the model lists, right after /model, each wearing the backend's
    // own description in the description column and lighting up whole when
    // selected like any other row. A query narrows to them like any other.
    use crate::llm::{ServiceTier, SpeedState};
    // The selection is the second tier row — two past /model wherever the
    // registry puts it, so a built-in added ahead of /model can't move the
    // selection onto some other row.
    let model_index = crate::app::COMMANDS
        .iter()
        .position(|c| c.name == "model")
        .expect("/model is registered");
    let mut app = palette("/", model_index + 2);
    app.set_speed(SpeedState::new(
        vec![
            ServiceTier::new("priority", "Fast", "1.5x speed, increased usage"),
            ServiceTier::new("ultrafast", "Ultrafast", "The fastest available responses."),
        ],
        None,
    ));
    let lines = command_menu_lines(&app, 100);
    let texts = command_rows(&lines);
    let name = |t: &str| command_text(t).unwrap_or_default().to_string();
    let model = texts
        .iter()
        .position(|t| name(t).starts_with("/model"))
        .expect("/model in the window");
    assert!(
        name(&texts[model + 1]).starts_with("/fast")
            && texts[model + 1].ends_with("1.5x speed, increased usage"),
        "{texts:?}"
    );
    assert!(
        name(&texts[model + 2]).starts_with("/ultrafast")
            && texts[model + 2].ends_with("The fastest available responses."),
        "{texts:?}"
    );
    // The description column lines up with the built-ins' (one row each),
    // counted in display columns — the marker's `→` is one column but three
    // bytes.
    let col = |t: &str| {
        let at = t.find("1.5x").or_else(|| t.find("The fastest")).unwrap();
        cols(&t[..at])
    };
    assert_eq!(col(&texts[model + 1]), col(&texts[model + 2]));
    assert_eq!(col(&texts[model + 1]), MENU_DESC_COL);
    // /ultrafast (the second tier row) is selected — marked and lit — the
    // window having scrolled to keep it visible.
    let ultra = lines
        .iter()
        .find(|l| plain(l).starts_with("→ /ultrafast"))
        .expect("the selected tier row is in the window, marked");
    assert!(
        ultra
            .spans
            .iter()
            .all(|s| s.style.fg == Some(menu_selected_color()) || s.content.trim().is_empty()),
        "the selected tier row lights up whole"
    );
    // A query narrows to the tier rows like any other command.
    app.input = TextArea::from_text("/ultra");
    app.command_menu = Some(crate::app::CommandMenu { selected: 0 });
    let narrowed = command_rows(&command_menu_lines(&app, 100));
    assert_eq!(narrowed.len(), 1, "{narrowed:?}");
    assert!(narrowed[0].starts_with("→ /ultrafast"), "{narrowed:?}");
    // …and a model listing none shows none, the built-ins untouched.
    app.set_speed(None);
    app.input = TextArea::from_text("/");
    assert!(
        !command_rows(&command_menu_lines(&app, 100))
            .iter()
            .any(|t| name(t).starts_with("/fast") || name(t).starts_with("/ultrafast")),
    );
}

#[test]
fn a_degenerate_width_ellipsizes_the_command_name() {
    // Below the description column there is no room for descriptions at all;
    // the marker and the name show, the name `…`-cut to what is left of the
    // width — codex's popup shape (`/statuslin…`) — instead of paint-clipping
    // at the buffer edge.
    let lines = command_menu_lines(&palette("/settings", 0), 7);
    let text = plain(&lines[0]);
    assert_eq!(text, "→ /set…");
    assert!(cols(text.trim_end()) <= 7, "{text:?}");
}

#[test]
fn wrapped_continuation_rows_share_the_selection_colour() {
    // The whole highlighted row lights up cyan — its continuation rows
    // included, so a wrapped selected description reads as one entry.
    let app = palette("/copy", 0);
    let lines = command_menu_lines(&app, 40);
    assert!(lines.len() > 1, "the description wrapped: {lines:?}");
    for line in &lines {
        assert!(
            line.spans
                .iter()
                .filter(|s| !s.content.trim().is_empty())
                .all(|s| s.style.fg == Some(menu_selected_color())),
            "selected rows are cyan throughout: {line:?}"
        );
    }
}

#[test]
fn command_menu_lines_lists_commands_with_descriptions() {
    let texts: Vec<String> = command_menu_lines(&palette("/", 0), 60)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(texts.iter().any(|t| t.contains("/clear")), "{texts:?}");
    assert!(
        texts.iter().any(|t| t.contains("Clear the conversation")),
        "{texts:?}"
    );
}

#[test]
fn command_menu_aligns_descriptions_in_a_column() {
    // The marker (or its inset) and the name are padded so every description
    // starts at the same column, regardless of command-name length or which
    // row is selected; a wrapped description's continuation rows indent to
    // that same column. The description is each row's last span.
    let lines = command_menu_lines(&palette("/", 0), 60);
    for line in &lines {
        let (_desc, before) = line.spans.split_last().expect("a non-empty row");
        let before_desc: usize = before.iter().map(|s| cols(s.content.as_ref())).sum();
        assert_eq!(
            before_desc,
            MENU_DESC_COL,
            "desc column for {:?}",
            plain(line)
        );
    }
}

#[test]
fn command_menu_shows_a_placeholder_when_nothing_matches() {
    let texts: Vec<String> = command_menu_lines(&palette("/zzz", 0), 60)
        .iter()
        .map(|l| plain(l).to_string())
        .collect();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].to_lowercase().contains("no matching"), "{texts:?}");
}

#[test]
fn the_palette_marks_the_selection_with_an_arrow() {
    // The `@` and `$` pickers' marker (docs/file-search.md): the highlighted
    // command opens on `→ `, every other row on the marker's width of
    // spaces, so the names share one column and the selection still reads
    // where the cyan alone says nothing (a monochrome terminal, a capture).
    let lines = command_menu_lines(&palette("/", 1), 80);
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert!(texts[1].starts_with("→ /copy "), "{texts:?}");
    for (i, text) in texts.iter().enumerate().filter(|&(i, _)| i != 1) {
        assert!(text.starts_with("  /"), "row {i} is inset: {text:?}");
    }
    // The marker wears the row's colour, the name still bold beside it.
    let [marker, name, ..] = lines[1].spans.as_slice() else {
        panic!("marker, name, …: {:?}", lines[1].spans);
    };
    assert_eq!(marker.content, MENU_MARKER);
    assert_eq!(marker.style.fg, Some(menu_selected_color()));
    assert_eq!(name.content, "/copy");
    assert!(name.style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(lines[0].spans[0].content, MENU_INDENT);
}

#[test]
fn the_palette_moves_the_arrow_with_the_selection() {
    // Exactly one row wears the marker, and it is the highlighted one —
    // the window scrolling under it never strands the arrow on another row.
    let len = palette("/", 0).commands().len();
    for selected in 0..len {
        let lines = command_menu_lines(&palette("/", selected), 80);
        let arrows: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| plain(l).starts_with(MENU_MARKER))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(arrows, [highlighted_row(&lines)], "selection {selected}");
    }
}

#[test]
fn a_placeholder_row_sits_in_the_bands_text_column() {
    // Every band below the box draws its text from the marker's column:
    // a miss says so where the names would have been, not in the marker's
    // gutter (the `/model` pickers' `  No matching models` rule).
    let commands = command_menu_lines(&palette("/zzz", 0), 60);
    assert_eq!(
        plain(&commands[0]),
        format!("{MENU_INDENT}No matching commands")
    );
    let mut app = file_picker("zz", vec![], 0);
    app.file_search.as_mut().expect("open").waiting = true;
    assert_eq!(
        plain(&file_menu_lines(&app, 60)[0]),
        format!("{MENU_INDENT}Searching…")
    );
    app.file_search.as_mut().expect("open").waiting = false;
    assert_eq!(
        plain(&file_menu_lines(&app, 60)[0]),
        format!("{MENU_INDENT}No matching files")
    );
    let skills = skill_menu_lines(&skill_app("$zzz", 0), 60);
    assert_eq!(
        plain(&skills[0]),
        format!("{MENU_INDENT}No matching skills")
    );
}

#[test]
fn render_live_draws_the_command_menu_below_the_box() {
    let app = palette("/", 0);
    let menu = menu_rows(&app, 60);
    let h = live_height(&app.input, 60, 40, false, 0, 0, 0, 0, menu, 0, 0);
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    let all: String = (0..h)
        .map(|y| row(&buf, y, 60))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all.contains("/clear"),
        "menu rendered below the box: {all:?}"
    );
    // The reserved band height is the painted line count, so the band's
    // final wrapped row lands exactly on the region's last row.
    let lines = command_menu_lines(&app, 60);
    let last = plain(lines.last().expect("a painted band row"));
    assert!(
        row(&buf, h - 1, 60).contains(last.trim_end()),
        "the band's last line sits on the last row"
    );
}

#[test]
fn cursor_stays_in_the_box_when_the_palette_opens() {
    // The menu is reserved *below* the box, so opening the palette must not
    // move the cursor (the end of the input).
    let mut app = App::new();
    app.input = TextArea::from_text("/");
    let closed_area = Rect::new(
        0,
        0,
        40,
        live_height(
            &TextArea::from_text("/"),
            40,
            24,
            false,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ),
    );
    let closed = cursor_position(closed_area, &app);
    app.command_menu = Some(crate::app::CommandMenu { selected: 0 });
    let menu = menu_rows(&app, 40);
    let open_area = Rect::new(
        0,
        0,
        40,
        live_height(
            &TextArea::from_text("/"),
            40,
            24,
            false,
            0,
            0,
            0,
            0,
            menu,
            0,
            0,
        ),
    );
    let open = cursor_position(open_area, &app);
    assert_eq!(open, closed, "cursor unchanged when the menu opens");
}

// --- the `?` shortcuts band (docs/shortcuts.md) ---

/// Every terminal width the band tests sweep: narrower than the one-column
/// stack up to an ultra-wide pane.
const BAND_WIDTHS: std::ops::RangeInclusive<u16> = 20..=260;

/// The `(turn_active, can_backtrack)` states the esc entry swaps between —
/// a running turn that also has something to edit included (the turn wins).
const BAND_STATES: [(bool, bool); 4] = [(false, false), (true, false), (false, true), (true, true)];

/// The band laid out whole, with no terminal height to cut it to.
fn band(turn_active: bool, can_backtrack: bool, width: u16) -> Vec<Line<'static>> {
    shortcuts_lines(turn_active, can_backtrack, width, u16::MAX)
}

/// The entries on one band row: the display column each one's key starts
/// at, and its text (key + label) — read off the spans' styles, so a gap
/// or the `… +N more` row adds none.
fn band_entries(line: &Line) -> Vec<(usize, String)> {
    let mut col = 0;
    let mut entries: Vec<(usize, String)> = Vec::new();
    for span in &line.spans {
        if span.style.fg == Some(shortcuts_key_color()) {
            entries.push((col, span.content.to_string()));
        } else if span.style.fg == Some(shortcuts_text_color())
            && let Some((_, text)) = entries.last_mut()
        {
            text.push_str(&span.content);
        }
        col += cols(&span.content);
    }
    entries
}

/// The narrowest width in [`BAND_WIDTHS`] whose band passes `test`.
fn first_width(test: impl Fn(u16) -> bool) -> u16 {
    for width in BAND_WIDTHS {
        if test(width) {
            return width;
        }
    }
    panic!("no width in {BAND_WIDTHS:?} passes");
}

/// Just the texts of one band row's entries, left to right.
fn band_texts(line: &Line) -> Vec<String> {
    band_entries(line)
        .into_iter()
        .map(|(_, text)| text)
        .collect()
}

/// `SHORTCUTS[i]` as the idle band shows it.
fn entry_text(i: usize) -> String {
    let (key, label) = SHORTCUTS[i];
    format!("{key}{label}")
}

/// The narrowest gap between two entries on any row of the band at
/// `width`, in any context state — the gutter, which the widest entry of a
/// column has exactly after it.
fn band_gutter(width: u16) -> usize {
    BAND_STATES
        .iter()
        .flat_map(|&(turn_active, can_backtrack)| band(turn_active, can_backtrack, width))
        .flat_map(|line| {
            band_entries(&line)
                .windows(2)
                .map(|pair| pair[1].0 - (pair[0].0 + cols(&pair[0].1)))
                .collect::<Vec<_>>()
        })
        .min()
        .expect("a row with two entries")
}

#[test]
fn shortcuts_rows_is_zero_closed_and_counts_the_band_open() {
    let mut app = App::new();
    assert_eq!(shortcuts_rows(&app, 80), 0);
    app.shortcuts_open = true;
    assert_eq!(
        usize::from(shortcuts_rows(&app, 80)),
        SHORTCUTS.len().div_ceil(3),
        "three columns at 80 — six rows, where the two-column grid took more"
    );
    for width in BAND_WIDTHS {
        assert_eq!(
            usize::from(shortcuts_rows(&app, width)),
            band(false, false, width).len(),
            "the reserved rows are the built rows at {width} columns"
        );
    }
}

#[test]
fn the_band_spends_a_wider_terminal_on_columns_not_rows() {
    // The report: at 87 columns the fixed two-column grid left the right
    // third of the row empty and spent eight rows below the box. The band
    // takes as many columns as the width holds with every entry whole, and
    // so as few rows as it can — (width, columns, rows):
    let shape = |width| {
        let lines = band(false, false, width);
        let columns = lines.iter().map(|l| band_entries(l).len()).max();
        (columns.unwrap_or(0), lines.len())
    };
    for (width, columns, rows) in [
        (40, 1, SHORTCUTS.len()),
        (50, 2, 9),
        (60, 2, 9),
        (80, 3, 6),
        (87, 3, 6),
        (100, 4, 5),
        (120, 4, 5),
        (140, 5, 4),
        (200, 6, 3),
        (260, 6, 3),
    ] {
        assert_eq!(shape(width), (columns, rows), "at {width} columns");
    }
}

#[test]
fn the_band_never_outgrows_its_terminal_and_widening_never_costs_a_row() {
    // One entry per row is the narrowest the band can be; from that width
    // up no row is ever wider than the terminal (the paint would cut it
    // mid-word), in any context state, and a wider terminal never takes
    // more rows than a narrower one. Past the floor of SHORTCUTS_MIN_ROWS
    // rows a wider terminal adds no more columns: three rows of six is
    // already a strip, and a two-row ribbon reads as noise.
    let stack = BAND_STATES
        .iter()
        .flat_map(|&(turn_active, can_backtrack)| band(turn_active, can_backtrack, 1))
        .map(|line| cols(&plain(&line)))
        .max()
        .expect("a stacked row");
    let mut rows_before = usize::MAX;
    for width in BAND_WIDTHS {
        let rows = band(false, false, width).len();
        assert!(
            rows <= rows_before,
            "{width} columns took {rows} rows, more than {} columns did",
            width - 1
        );
        assert!(rows >= SHORTCUTS_MIN_ROWS, "{rows} rows at {width} columns");
        rows_before = rows;
        if usize::from(width) < stack {
            continue;
        }
        for (turn_active, can_backtrack) in BAND_STATES {
            for line in band(turn_active, can_backtrack, width) {
                assert!(
                    cols(&plain(&line)) <= usize::from(width),
                    "{:?} is wider than {width} columns",
                    plain(&line)
                );
            }
        }
    }
}

#[test]
fn the_band_reads_down_each_column_in_the_bindings_order() {
    // Column-major, like `ls`: reading down a column is reading the list,
    // so SHORTCUTS' order is what each column holds — at 80 columns the
    // composer's sigils and history, then editing and the running turn,
    // then the views, the modes and the way out.
    let lines = band(false, false, 80);
    let rows = lines.len();
    assert_eq!(rows, 6);
    for (r, line) in lines.iter().enumerate() {
        let expected: Vec<String> = (0..3).map(|c| entry_text(c * rows + r)).collect();
        assert_eq!(band_texts(line), expected, "row {r}");
    }
    assert_eq!(
        band_texts(&lines[0]),
        [
            "/ for commands",
            "shift+enter for newline",
            "ctrl+o for transcript"
        ]
    );
}

#[test]
fn an_uneven_split_shortens_the_last_columns() {
    // Eighteen entries in four columns are five, five, four and four: the
    // gaps gather at the band's bottom right instead of leaving a column
    // of two.
    let lines = band(false, false, 120);
    let counts: Vec<usize> = lines.iter().map(|l| band_entries(l).len()).collect();
    assert_eq!(counts, [4, 4, 4, 4, 2]);
    assert_eq!(band_texts(&lines[4]), [entry_text(4), entry_text(9)]);
}

#[test]
fn the_band_lists_every_binding_once_down_its_columns_at_every_width() {
    // Whatever the layout — six columns or one, the full wording or the
    // terse — reading the band down each column, left to right, reads
    // SHORTCUTS once through: no binding dropped or doubled by a split that
    // does not divide evenly, and none out of line with its column. The
    // layout tests pin shapes at chosen widths; this pins the content at
    // every width they sweep.
    let keys: Vec<&str> = SHORTCUTS.iter().map(|&(key, _)| key).collect();
    for width in BAND_WIDTHS {
        // (the column an entry's key starts in, its row, the key)
        let mut cells: Vec<(usize, usize, String)> = Vec::new();
        for (row, line) in band(false, false, width).iter().enumerate() {
            let mut col = 0;
            for span in &line.spans {
                if span.style.fg == Some(shortcuts_key_color()) {
                    cells.push((col, row, span.content.to_string()));
                }
                col += cols(&span.content);
            }
        }
        cells.sort();
        let read: Vec<&str> = cells.iter().map(|(_, _, key)| key.as_str()).collect();
        assert_eq!(read, keys, "at {width} columns");
    }
}

#[test]
fn the_columns_hold_still_and_keep_a_gutter_in_every_state() {
    // The esc entry swaps its text with the context (` to interrupt`
    // mid-turn, the `esc esc` edit hint idle with something to edit), and a
    // turn starting under an open band must not slide the columns after it:
    // a column is as wide as the widest text any of its entries ever shows.
    // Between two entries on a row there are always at least
    // SHORTCUTS_MIN_GUTTER blank columns — fewer and two entries read as one
    // run-on line.
    for width in BAND_WIDTHS {
        let starts = |turn_active, can_backtrack| -> Vec<Vec<usize>> {
            band(turn_active, can_backtrack, width)
                .iter()
                .map(|line| band_entries(line).iter().map(|(col, _)| *col).collect())
                .collect()
        };
        let idle = starts(false, false);
        for (turn_active, can_backtrack) in BAND_STATES {
            assert_eq!(
                starts(turn_active, can_backtrack),
                idle,
                "the columns moved at {width} (turn_active={turn_active}, can_backtrack={can_backtrack})"
            );
            for line in band(turn_active, can_backtrack, width) {
                for pair in band_entries(&line).windows(2) {
                    let end = pair[0].0 + cols(&pair[0].1);
                    assert!(
                        pair[1].0 >= end + SHORTCUTS_MIN_GUTTER,
                        "a {}-column gutter at {width}: {:?}",
                        pair[1].0 - end,
                        plain(&line)
                    );
                }
            }
        }
    }
}

#[test]
fn spare_width_widens_the_gutters_up_to_a_cap() {
    // A layout fits from the width its columns need with the narrowest
    // gutter; every column past that is shared out between the gutters, so
    // the band fills the row rather than leaving its right side empty — up
    // to SHORTCUTS_MAX_GUTTER, past which a wider terminal leaves the band
    // where it is instead of scattering its columns across the screen.
    let three = first_width(|w| band(false, false, w).len() == 6);
    assert_eq!(band_gutter(three), SHORTCUTS_MIN_GUTTER, "at {three}");
    assert_eq!(
        band_gutter(three + 2),
        SHORTCUTS_MIN_GUTTER + 1,
        "one more column per gutter"
    );
    assert_eq!(
        band_gutter(87),
        SHORTCUTS_MAX_GUTTER,
        "the reported terminal"
    );
    assert_eq!(band_gutter(200), SHORTCUTS_MAX_GUTTER, "capped");
    // At the reported 87 columns the band reaches across the row.
    let widest = band(false, false, 87)
        .iter()
        .map(|line| cols(&plain(line)))
        .max()
        .expect("a band row");
    assert!(widest >= 85, "the band stops at column {widest} of 87");
}

#[test]
fn a_narrow_terminal_drops_the_connectives_to_keep_two_columns() {
    // Two columns in the full wording need 53 columns. Under that the band
    // would stack into eighteen rows — on a 51x24 pane that crowded a
    // forming reply's preview out of the strip (`smoke.sh` Phase 99) — so
    // first it drops each label's `for`/`to` (`/ commands`, `ctrl+r search
    // history`), which keeps two columns of nine rows down to 46 columns.
    // The full wording comes back as soon as it fits in two.
    let full = first_width(|w| {
        let lines = band(false, false, w);
        lines.len() < SHORTCUTS.len() && plain(&lines[0]).starts_with("  / for commands")
    });
    assert_eq!(full, 53, "the full wording's two columns");
    let terse = first_width(|w| band(false, false, w).len() < SHORTCUTS.len());
    assert_eq!(terse, 46, "the terse wording's two columns");
    let lines = band(false, false, 50);
    assert_eq!(lines.len(), 9);
    assert_eq!(band_texts(&lines[0]), ["/ commands", "tab queue next turn"]);
    assert_eq!(
        band_texts(&lines[8]),
        ["ctrl+w/u/k kill text", "ctrl+c quit"]
    );
    for line in &lines {
        for text in band_texts(line) {
            assert!(
                !text.contains(" for ") && !text.contains(" to "),
                "{text:?} kept its connective"
            );
        }
    }
    // The context swaps are shortened the same way.
    let busy: String = band(true, false, 50).iter().map(plain).collect();
    assert!(busy.contains("esc interrupt"), "{busy:?}");
    let target: String = band(false, true, 50).iter().map(plain).collect();
    assert!(target.contains("esc esc edit previous"), "{target:?}");
}

#[test]
fn every_label_leads_with_a_connective_the_terse_wording_drops() {
    // The terse wording is each label minus its leading ` for` / ` to`, so
    // every label — the esc entry's swaps included — has to start with one
    // and keep a word after it.
    for &(key, label) in SHORTCUTS
        .iter()
        .chain([&SHORTCUTS_INTERRUPT, &SHORTCUTS_BACKTRACK])
    {
        assert!(
            label.starts_with(" for ") || label.starts_with(" to "),
            "{key}{label:?} leads with neither ` for ` nor ` to `"
        );
    }
}

#[test]
fn a_narrower_terminal_stacks_the_shortcuts_in_one_column() {
    // Under the width even the terse wording's two columns need, the band
    // lists one entry per row, in order and in the full wording — one
    // column costs the same rows either way — each row still inset: taller,
    // but nothing on it cut mid-word, which is what the paint's clip did to
    // a fixed second column once.
    let texts: Vec<String> = band(false, false, 45).iter().map(plain).collect();
    let expected: Vec<String> = (0..SHORTCUTS.len())
        .map(|i| format!("{MENU_INDENT}{}", entry_text(i)))
        .collect();
    assert_eq!(texts, expected, "one entry per row, in order");
    // The context swaps ride the stacked band too.
    let busy: Vec<String> = band(true, false, 45).iter().map(plain).collect();
    assert!(
        busy.contains(&format!("{MENU_INDENT}esc to interrupt")),
        "{busy:#?}"
    );
    // Too narrow for even one column in the full wording, the terse one
    // keeps every entry whole a few columns further down.
    let full_stack = BAND_STATES
        .iter()
        .flat_map(|&(turn_active, can_backtrack)| band(turn_active, can_backtrack, 45))
        .map(|line| cols(&plain(&line)))
        .max()
        .expect("a stacked row");
    assert_eq!(full_stack, 27, "the inset and the widest full entry");
    let narrow = band(false, false, 26);
    assert_eq!(narrow.len(), SHORTCUTS.len());
    assert_eq!(plain(&narrow[0]), format!("{MENU_INDENT}/ commands"));
    assert!(narrow.iter().all(|line| cols(&plain(line)) <= 26));
}

#[test]
fn the_shortcuts_band_is_inset_to_the_bands_text_column() {
    // The band takes the footer's slot, so its grid starts where the footer
    // row does — two in from the edge, the column the palette's and the
    // pickers' names start at after their marker. The inset is blank,
    // wearing neither the key's colour nor the label's.
    assert_eq!(cols(MENU_INDENT), cols(FOOTER_INDENT));
    for width in [40, 50, 80, 120, 200] {
        for (turn_active, can_backtrack) in BAND_STATES {
            for line in band(turn_active, can_backtrack, width) {
                let [inset, key, ..] = line.spans.as_slice() else {
                    panic!("an inset and an entry: {line:?}");
                };
                assert_eq!(inset.content, MENU_INDENT, "{:?}", plain(&line));
                assert_eq!(inset.style, Style::default(), "{:?}", plain(&line));
                assert!(!key.content.starts_with(' '), "{:?}", plain(&line));
            }
        }
    }
    let texts: Vec<String> = band(false, false, 80).iter().map(plain).collect();
    assert!(texts[0].starts_with("  / for commands "), "{texts:?}");
}

#[test]
fn shortcuts_lines_style_keys_cyan_and_labels_dim() {
    // spans = [inset, key, label, (gap, key, label)…] — keys cyan, labels
    // dim, gaps blank and unstyled, in every column at every width.
    for width in [26, 40, 50, 80, 120, 200] {
        for line in band(false, false, width) {
            for (i, span) in line.spans.iter().enumerate().skip(1) {
                match (i - 1) % 3 {
                    0 => assert_eq!(span.style.fg, Some(shortcuts_key_color()), "{span:?}"),
                    1 => assert_eq!(span.style.fg, Some(shortcuts_text_color()), "{span:?}"),
                    _ => {
                        assert!(span.content.trim().is_empty(), "{span:?}");
                        assert_eq!(span.style, Style::default(), "{span:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn the_band_lists_every_binding_the_composer_answers() {
    // Each entry once, in its own words: the sigils (`@` is the file picker,
    // docs/file-search.md; `$` the skill picker, docs/skill-mentions.md),
    // the history (docs/input-history.md, docs/history-search.md), the
    // editing keys (docs/shift-enter.md, docs/image-paste.md,
    // docs/textarea.md), the running turn (docs/queue.md — Tab and Alt+Up;
    // docs/background.md — Ctrl+B), the two views, the thinking and
    // permission modes (docs/reasoning.md, docs/permissions.md) and the
    // way out.
    let all: String = band(false, false, 80)
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n");
    for entry in [
        "/ for commands",
        "! for shell command",
        "@ for file paths",
        "$ for skills",
        "↑ for input history",
        "ctrl+r to search history",
        "shift+enter for newline",
        "ctrl+v for image paste",
        "ctrl+w/u/k to kill text",
        "tab to queue next turn",
        "alt+↑ to edit queue",
        "ctrl+b to background",
        "ctrl+o for transcript",
        "ctrl+d for llm context",
        "ctrl+t to cycle thinking",
        "shift+tab for permissions",
        "esc to quit",
        "ctrl+c to quit",
    ] {
        assert_eq!(all.matches(entry).count(), 1, "{entry:?} once in: {all}");
    }
    assert_eq!(SHORTCUTS.len(), 18, "and nothing else");
}

#[test]
fn shortcuts_lines_flip_the_esc_entry_while_a_turn_runs() {
    // codex's quit entry is context-sensitive: "to interrupt" while a task
    // runs. Our Esc entry flips the same way.
    let idle: String = band(false, false, 80).iter().map(plain).collect();
    let busy: String = band(true, false, 80).iter().map(plain).collect();
    assert!(idle.contains("esc to quit"), "{idle:?}");
    assert!(busy.contains("esc to interrupt"), "{busy:?}");
    assert!(!busy.contains("esc to quit"), "{busy:?}");
}

#[test]
fn a_band_cut_short_ends_on_a_count_of_what_it_hid() {
    // On a terminal too short for the whole band the paint is handed fewer
    // rows than the band asked for. Rather than lose the last entries
    // without a word (the way out among them), it shows the rows that fit
    // and names the rest on a dim last row.
    let whole: Vec<String> = band(false, false, 40).iter().map(plain).collect();
    assert_eq!(whole.len(), SHORTCUTS.len());
    let cut = shortcuts_lines(false, false, 40, 7);
    let texts: Vec<String> = cut.iter().map(plain).collect();
    assert_eq!(texts.len(), 7);
    assert_eq!(texts[..6], whole[..6]);
    assert_eq!(texts[6], format!("{MENU_INDENT}… +12 more"));
    let marker = cut[6].spans.last().expect("the marker");
    assert_eq!(marker.style.fg, Some(shortcuts_text_color()));
    // A band of columns counts every entry on the rows it drops.
    let cut = shortcuts_lines(false, false, 80, 4);
    assert_eq!(cut.len(), 4);
    assert_eq!(plain(&cut[3]), format!("{MENU_INDENT}… +9 more"));
    // Room for the whole band shows the whole band; no room shows nothing,
    // and one row is only the count.
    assert_eq!(shortcuts_lines(false, false, 80, 6), band(false, false, 80));
    assert!(shortcuts_lines(false, false, 80, 0).is_empty());
    let one = shortcuts_lines(false, false, 80, 1);
    assert_eq!(one.len(), 1);
    assert_eq!(
        plain(&one[0]),
        format!("{MENU_INDENT}… +{} more", SHORTCUTS.len())
    );
}

#[test]
fn render_live_cuts_the_band_to_the_rows_a_short_terminal_leaves() {
    // 40×10: the band asks for all eighteen rows, the region is clamped to
    // ten, the composer keeps its three, and the band paints the seven left
    // — six entries and the count of the other twelve — instead of losing
    // the bottom of the list to the clip.
    let mut app = App::new();
    app.shortcuts_open = true;
    let ask = shortcuts_rows(&app, 40);
    assert_eq!(usize::from(ask), SHORTCUTS.len());
    let h = live_height(&app.input, 40, 10, false, 0, 0, 0, 0, ask, 0, 0);
    assert_eq!(h, 10, "the region is clamped to the terminal");
    let mut buf = buffer(40, h);
    render_live(buf.area, &mut buf, &app);
    assert_eq!(
        row(&buf, 1, 40).trim_end(),
        "❯",
        "the composer's prompt row"
    );
    assert_eq!(row(&buf, h - 7, 40).trim_end(), "  / for commands");
    assert_eq!(
        row(&buf, h - 2, 40).trim_end(),
        "  ctrl+r to search history"
    );
    assert_eq!(row(&buf, h - 1, 40).trim_end(), "  … +12 more");
}

#[test]
fn render_live_paints_the_stacked_band_on_a_narrow_terminal() {
    // The region reserves the stacked band's rows, and the paint builds the
    // band at the same width — so the last entry lands on the last row.
    let mut app = App::new();
    app.shortcuts_open = true;
    let band_rows = shortcuts_rows(&app, 40);
    assert_eq!(
        usize::from(band_rows),
        SHORTCUTS.len(),
        "stacked at 40 columns"
    );
    let h = live_height(&app.input, 40, 30, false, 0, 0, 0, 0, band_rows, 0, 0);
    let mut buf = buffer(40, h);
    render_live(buf.area, &mut buf, &app);
    assert_eq!(
        row(&buf, h - 1, 40).trim_end(),
        "  ctrl+c to quit",
        "the last entry on the last region row"
    );
    assert_eq!(
        row(&buf, h - band_rows, 40).trim_end(),
        "  / for commands",
        "the first entry on the band's first row"
    );
}

#[test]
fn live_height_adds_the_shortcuts_band() {
    let mut app = App::new();
    app.shortcuts_open = true;
    let closed = live_height(&app.input, 40, 24, false, 0, 0, 0, 0, 0, 0, 0);
    let open = live_height(
        &app.input,
        40,
        24,
        false,
        0,
        0,
        0,
        0,
        shortcuts_rows(&app, 40),
        0,
        0,
    );
    assert_eq!(open, closed + shortcuts_rows(&app, 40));
}

#[test]
fn render_live_draws_the_shortcuts_band_below_the_box() {
    let mut app = App::new();
    app.shortcuts_open = true;
    let h = live_height(
        &app.input,
        80,
        24,
        false,
        0,
        0,
        0,
        0,
        shortcuts_rows(&app, 80),
        0,
        0,
    );
    let mut buf = buffer(80, h);
    render_live(buf.area, &mut buf, &app);
    let all: String = (0..h)
        .map(|y| row(&buf, y, 80))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("  / for commands"), "band rendered: {all:?}");
    let last = row(&buf, h - 1, 80);
    assert!(
        last.starts_with("  ctrl+r to search history")
            && last.trim_end().ends_with("ctrl+c to quit"),
        "the last band row sits on the last region row: {last:?}"
    );
}

#[test]
fn cursor_stays_in_the_box_when_the_shortcuts_band_opens() {
    let mut app = App::new();
    let closed_area = Rect::new(
        0,
        0,
        40,
        live_height(&app.input, 40, 24, false, 0, 0, 0, 0, 0, 0, 0),
    );
    let closed = cursor_position(closed_area, &app);
    app.shortcuts_open = true;
    let open_area = Rect::new(
        0,
        0,
        40,
        live_height(
            &app.input,
            40,
            24,
            false,
            0,
            0,
            0,
            0,
            shortcuts_rows(&app, 40),
            0,
            0,
        ),
    );
    let open = cursor_position(open_area, &app);
    assert_eq!(open, closed, "cursor unchanged when the band opens");
}

#[test]
fn the_queue_and_the_shortcuts_band_show_in_their_own_slots() {
    // The queue is in the strip (above the box); the shortcuts band is below
    // it — independent slots, both visible at once.
    let mut app = App::new();
    app.begin_stream();
    app.shortcuts_open = true;
    app.queued.push_back(batch(&["world"]));
    let q = queued_rows(&app, 80);
    let h = live_height(
        &app.input,
        80,
        24,
        true,
        1,
        0,
        q,
        0,
        shortcuts_rows(&app, 80),
        0,
        0,
    );
    let mut buf = buffer(80, h);
    render_live(buf.area, &mut buf, &app);
    let rows: Vec<String> = (0..h).map(|y| row(&buf, y, 80)).collect();
    let rule = rows
        .iter()
        .position(|r| r.contains('─'))
        .expect("a box rule");
    let world = rows
        .iter()
        .position(|r| r.contains("❯ world"))
        .expect("the queued message");
    let cmds = rows
        .iter()
        .position(|r| r.contains("for commands"))
        .expect("the shortcuts band");
    assert!(world < rule, "queue above the box: {rows:?}");
    assert!(cmds > rule, "shortcuts below the box: {rows:?}");
}

#[test]
fn the_palette_replaces_the_footer() {
    let mut app = palette("/", 0);
    app.set_session_info("dummy_model_name", "~/alter-zero");
    let band = menu_rows(&app, 60);
    let h = live_height(
        &app.input,
        60,
        24,
        false,
        0,
        0,
        0,
        0,
        band,
        footer_rows(&app, band),
        0,
    );
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    let all: String = (0..h)
        .map(|y| row(&buf, y, 60))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("/clear"), "palette shown: {all:?}");
    assert!(
        !all.contains("dummy_model_name"),
        "the footer yields its slot: {all:?}"
    );
}

#[test]
fn file_menu_rows_counts_closed_placeholder_and_capped_matches() {
    assert_eq!(file_menu_rows(&App::new()), 0, "closed");
    assert_eq!(
        file_menu_rows(&file_picker("a", vec![], 0)),
        1,
        "one placeholder row while empty"
    );
    let many: Vec<_> = (0..12).map(|i| fmatch(&format!("d/f{i}.rs"))).collect();
    assert_eq!(
        file_menu_rows(&file_picker("f", many, 0)),
        FILE_MENU_MAX_ROWS,
        "capped"
    );
}

#[test]
fn file_menu_lines_placeholder_is_searching_then_no_match() {
    let mut app = file_picker("a", vec![], 0);
    app.file_search.as_mut().unwrap().waiting = true;
    assert!(plain(&file_menu_lines(&app, 40)[0]).contains("Searching"));
    app.file_search.as_mut().unwrap().waiting = false;
    assert!(plain(&file_menu_lines(&app, 40)[0]).contains("No matching files"));
}

#[test]
fn file_menu_lines_lists_names_with_the_selection_highlighted() {
    let app = file_picker("ma", vec![fmatch("src/main.rs"), fmatch("READ.md")], 1);
    let lines = file_menu_lines(&app, 40);
    assert_eq!(lines.len(), 2);
    // Each row shows the basename in the name column with its parent dir
    // (`./` for a root-level entry) beside it — not one raw path string.
    assert!(
        plain(&lines[0]).contains("main.rs") && plain(&lines[0]).contains("src/"),
        "{:?}",
        plain(&lines[0])
    );
    assert!(
        plain(&lines[1]).contains("READ.md") && plain(&lines[1]).contains("./"),
        "{:?}",
        plain(&lines[1])
    );
    // The selected row (index 1) carries the arrow marker and the cyan
    // selection colour; the unselected row indents by the marker's width.
    assert!(plain(&lines[1]).starts_with("→ "), "{:?}", plain(&lines[1]));
    assert!(plain(&lines[0]).starts_with("  "), "{:?}", plain(&lines[0]));
    assert!(
        lines[1]
            .spans
            .iter()
            .any(|s| s.style.fg == Some(menu_selected_color()))
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|s| s.style.fg != Some(menu_selected_color()))
    );
}

#[test]
fn file_menu_rows_split_name_parent_and_kind_into_columns() {
    // The requested look — name, parent dir, and kind in aligned columns:
    //   → public      ./       …             Dir
    //     assets      public/  …             Dir
    //     cv.pdf      public/assets/ …       File
    let app = file_picker(
        "public",
        vec![
            fmatch("public/"),
            fmatch("public/assets/"),
            fmatch("public/assets/cv.pdf"),
        ],
        0,
    );
    let texts: Vec<String> = file_menu_lines(&app, 80).iter().map(plain).collect();
    assert_eq!(texts.len(), 3);
    assert!(texts[0].starts_with("→ public"), "{texts:?}");
    assert!(texts[1].starts_with("  assets"), "{texts:?}");
    assert!(texts[2].starts_with("  cv.pdf"), "{texts:?}");
    // Columns align by *display* width (the `→` marker is one column but three
    // bytes, so measure in columns, not `find` offsets).
    let col_of = |t: &str, needle: &str| cols(&t[..t.find(needle).unwrap()]);
    // The parent column starts right after the name column (the widest visible
    // name + a two-space gap): every name here is 6 wide → column 10.
    assert_eq!(col_of(&texts[0], "./"), 10, "{texts:?}");
    assert_eq!(col_of(&texts[1], "public/"), 10, "{texts:?}");
    assert_eq!(col_of(&texts[2], "public/assets/"), 10, "{texts:?}");
    // The kind column is pinned at the right edge (width − 6): File / Dir.
    assert_eq!(col_of(&texts[0], "Dir"), 74, "{texts:?}");
    assert_eq!(col_of(&texts[1], "Dir"), 74, "{texts:?}");
    assert_eq!(col_of(&texts[2], "File"), 74, "{texts:?}");
}

#[test]
fn file_menu_pins_the_kind_column_under_a_truncated_parent() {
    // A parent deeper than the dir column is truncated so the kind column
    // stays put at width − 6.
    let deep = format!("{}x.rs", "a/".repeat(30));
    let app = file_picker("x", vec![fmatch(&deep)], 0);
    let texts: Vec<String> = file_menu_lines(&app, 40).iter().map(plain).collect();
    let before_kind = cols(&texts[0][..texts[0].find("File").expect("kind label")]);
    assert_eq!(before_kind, 34, "{texts:?}");
    assert!(cols(&texts[0]) <= 40, "row fits the width: {texts:?}");
}

#[test]
fn file_menu_degrades_to_marker_and_name_on_narrow_widths() {
    // Too narrow for the parent/kind columns → just the marker and the name.
    let app = file_picker("cv", vec![fmatch("public/assets/cv.pdf")], 0);
    let texts: Vec<String> = file_menu_lines(&app, 12).iter().map(plain).collect();
    assert_eq!(texts[0].trim_end(), "→ cv.pdf", "{texts:?}");
}

#[test]
fn the_file_picker_shows_at_most_eight_results() {
    assert_eq!(FILE_MENU_MAX_ROWS, 8, "the requested cap");
    let many: Vec<_> = (0..30).map(|i| fmatch(&format!("f{i}.rs"))).collect();
    assert_eq!(file_menu_lines(&file_picker("f", many, 0), 80).len(), 8);
}

// ===== the `$` skill picker band (docs/skill-mentions.md) =====

/// A discovered skill for the band tests.
fn smeta(name: &str, description: &str) -> crate::skills::SkillMetadata {
    crate::skills::SkillMetadata {
        name: name.to_string(),
        description: description.to_string(),
        dir: std::path::PathBuf::from("/skills").join(name),
        path: std::path::PathBuf::from("/skills")
            .join(name)
            .join("SKILL.md"),
    }
}

/// An app whose composer holds `text` (cursor at the end) with the skill
/// picker open at `selected`, over the two demo skills.
fn skill_app(text: &str, selected: usize) -> App {
    let mut app = App::new();
    app.set_skills(vec![
        smeta(
            "dataviz",
            "Generate or edit charts for websites, games, and apps",
        ),
        smeta("skill-creator", "Create or update a skill"),
    ]);
    app.input = TextArea::from_text(text);
    app.skill_picker = Some(crate::app::SkillPicker {
        selected,
        query: String::new(),
    });
    app
}

#[test]
fn skill_menu_rows_counts_closed_placeholder_and_capped_matches() {
    assert_eq!(skill_menu_rows(&App::new()), 0, "closed");
    assert_eq!(
        skill_menu_rows(&skill_app("$zzz", 0)),
        1,
        "one placeholder row when nothing matches"
    );
    let mut app = App::new();
    app.set_skills((0..12).map(|i| smeta(&format!("skill-{i}"), "d")).collect());
    app.input = TextArea::from_text("$");
    app.skill_picker = Some(crate::app::SkillPicker::default());
    assert_eq!(skill_menu_rows(&app), SKILL_MENU_MAX_ROWS, "capped");
}

#[test]
fn skill_menu_hides_when_the_cursor_leaves_the_mention() {
    // The rows derive from the live token: a cursor parked before the `$`
    // shows no band even while the picker state lingers.
    let mut app = skill_app("$da", 0);
    app.input.move_home();
    assert_eq!(skill_menu_rows(&app), 0);
    assert!(skill_menu_lines(&app, 60).is_empty());
}

#[test]
fn skill_menu_lines_column_name_and_description() {
    // The requested look — names, then descriptions in one aligned column:
    //     dataviz        Generate or edit charts for websites, games, a…
    //   → skill-creator  Create or update a skill
    let app = skill_app("$", 1);
    let lines = skill_menu_lines(&app, 80);
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(texts.len(), 2);
    assert!(texts[0].starts_with("  dataviz"), "{texts:?}");
    assert!(texts[1].starts_with("→ skill-creator"), "{texts:?}");
    // The description column starts right after the widest visible name
    // (`skill-creator`, 13 wide) + the gap, marker included: 2 + 13 + 2.
    let col_of = |t: &str, needle: &str| cols(&t[..t.find(needle).unwrap()]);
    assert_eq!(col_of(&texts[0], "Generate"), 17, "{texts:?}");
    assert_eq!(col_of(&texts[1], "Create"), 17, "{texts:?}");
    // Selection is shown by colour: the highlighted row cyan, the rest not.
    assert!(
        lines[1]
            .spans
            .iter()
            .any(|s| s.style.fg == Some(menu_selected_color()))
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|s| s.style.fg != Some(menu_selected_color()))
    );
}

#[test]
fn skill_menu_placeholder_names_the_miss() {
    let app = skill_app("$zzz", 0);
    let lines = skill_menu_lines(&app, 60);
    assert_eq!(lines.len(), 1);
    assert!(
        plain(&lines[0]).contains("No matching skills"),
        "{:?}",
        plain(&lines[0])
    );
}

#[test]
fn skill_menu_truncates_the_description_with_an_ellipsis() {
    // The mock's `…`-cut description: the row never overflows the width and
    // a cut description says so. (Width 42 → marker 2 + name column 15 leave
    // 25 for the description: the 54-wide first is cut, the 24-wide second
    // fits whole.)
    let app = skill_app("$", 0);
    let texts: Vec<String> = skill_menu_lines(&app, 42).iter().map(plain).collect();
    assert!(cols(&texts[0]) <= 42, "{texts:?}");
    assert!(texts[0].trim_end().ends_with('…'), "{texts:?}");
    assert!(
        texts[1].trim_end().ends_with("Create or update a skill"),
        "an uncut description keeps its tail: {texts:?}"
    );
}

#[test]
fn skill_menu_bolds_the_matched_characters() {
    // "$dv" fuzzy-matches d…v in `dataviz` (bytes 0 and 4) — those glyphs
    // render bold, the rest of the name plain.
    let app = skill_app("$dv", 0);
    let line = &skill_menu_lines(&app, 60)[0];
    let bolded: String = line
        .spans
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(bolded, "dv", "{line:?}");
}

#[test]
fn band_rows_counts_the_skill_menu() {
    let app = skill_app("$", 0);
    assert_eq!(band_rows(&app, 80), 2, "one row per matched skill");
}

#[test]
fn file_menu_bolds_the_matched_characters() {
    // The query matched "ma" → bytes 0 and 1 of "main.rs".
    let m = FileMatch {
        path: "main.rs".to_string(),
        score: 1,
        indices: vec![0, 1],
    };
    let app = file_picker("ma", vec![m], 0);
    let line = &file_menu_lines(&app, 40)[0];
    assert!(
        line.spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD)),
        "the matched characters are bolded: {line:?}"
    );
}

#[test]
fn file_menu_bolds_matched_characters_in_both_columns() {
    // "sma" matched the `s` of the parent (`src/`, byte 0) and `ma` in the
    // name (bytes 4–5 of "src/main.rs"): the byte offsets in `indices` are
    // remapped onto the split name / parent columns.
    let m = FileMatch {
        path: "src/main.rs".to_string(),
        score: 1,
        indices: vec![0, 4, 5],
    };
    let app = file_picker("sma", vec![m], 0);
    let line = &file_menu_lines(&app, 60)[0];
    let bold: Vec<&str> = line
        .spans
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(bold.contains(&"ma"), "name-column hit bolded: {bold:?}");
    assert!(bold.contains(&"s"), "parent-column hit bolded: {bold:?}");
}

#[test]
fn render_live_draws_the_file_picker_below_the_box() {
    let app = file_picker("ma", vec![fmatch("src/main.rs")], 0);
    let band = file_menu_rows(&app);
    let h = live_height(&app.input, 40, 24, false, 0, 0, 0, 0, band, 0, 0);
    let mut buf = buffer(40, h);
    render_live(buf.area, &mut buf, &app);
    let all: String = (0..h)
        .map(|y| row(&buf, y, 40))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all.contains("main.rs") && all.contains("src/") && all.contains("File"),
        "the file picker rendered below the box: {all:?}"
    );
}

#[test]
fn cursor_stays_in_the_box_when_the_file_picker_opens() {
    // The picker is reserved *below* the box, so opening it must not move the
    // cursor (mirrors the palette).
    let area = Rect::new(0, 0, 40, 24);
    let mut closed = App::new();
    closed.input = TextArea::from_text("@m");
    let before = cursor_position(area, &closed);
    let open = file_picker("m", vec![fmatch("main.rs")], 0);
    let after = cursor_position(area, &open);
    assert_eq!(after, before, "cursor unchanged when the picker opens");
}

#[test]
fn shortcuts_esc_entry_is_three_way_context_sensitive() {
    // Interrupt while a turn runs (as before); the Esc-Esc edit hint when
    // idle with a previous user message; quit only with nothing to edit.
    let idle: String = band(false, false, 80).iter().map(plain).collect();
    assert!(idle.contains("esc to quit"), "{idle:?}");
    let busy: String = band(true, true, 80).iter().map(plain).collect();
    assert!(busy.contains("esc to interrupt"), "{busy:?}");
    let target: String = band(false, true, 80).iter().map(plain).collect();
    assert!(target.contains("esc esc to edit previous"), "{target:?}");
    assert!(!target.contains("esc to quit"), "{target:?}");
}

// --- the `@` file picker (docs/file-search.md) ---

/// A bare file match for the picker render tests.
fn fmatch(path: &str) -> FileMatch {
    FileMatch {
        path: path.to_string(),
        score: 0,
        indices: Vec::new(),
    }
}

#[test]
fn the_skill_entry_is_an_ordinary_grid_slot() {
    // The `$` skill picker (docs/skill-mentions.md) closes the composer's
    // sigils — `/ ! @ $` down the head of the first column — as a plain
    // [`SHORTCUTS`] entry, listed once at every width, no special case.
    let lines = band(false, false, 80);
    let sigils: Vec<String> = lines[..4]
        .iter()
        .map(|line| band_texts(line)[0].clone())
        .collect();
    assert_eq!(
        sigils,
        [
            "/ for commands",
            "! for shell command",
            "@ for file paths",
            "$ for skills"
        ]
    );
    for width in BAND_WIDTHS {
        let listed = band(false, false, width)
            .iter()
            .flat_map(band_texts)
            .filter(|text| text.starts_with("$ "))
            .count();
        assert_eq!(listed, 1, "listed once at {width} columns");
    }
}
