//! The bands below the box: palette, `@` picker, `?` shortcuts
//! (`docs/file-search.md`, `docs/shortcuts.md`).

use super::*;
use crate::ui::theme::{
    FILE_MENU_MAX_ROWS, MENU_DESC_COL, MENU_MAX_ROWS, SHORTCUTS, SHORTCUTS_COL,
    SKILL_MENU_MAX_ROWS, menu_selected_color, shortcuts_key_color, shortcuts_text_color,
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

/// The palette rows that *start* a command (their first span is the `/name`),
/// as plain text — a wrapped description's continuation rows are excluded, so
/// a count of these is a count of the **commands** shown.
fn command_rows(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .filter(|l| {
            l.spans
                .first()
                .is_some_and(|s| s.content.as_ref().starts_with('/'))
        })
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

#[test]
fn menu_rows_is_zero_when_the_palette_is_closed() {
    assert_eq!(menu_rows(&App::new(), 60), 0);
}

#[test]
fn the_palette_shows_at_most_eight_commands() {
    // The requested cap (the file picker's): a bare `/` shows the first eight
    // commands and longer match lists scroll (menu_window) instead of growing
    // the band — the registry has outgrown the window, so /quit (the ninth)
    // starts off-window (smoke.sh Phase 4 asserts the same on the real
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
    assert!(texts[0].contains("/help"), "{texts:?}");
    assert!(
        !texts.iter().any(|t| t.contains("/quit")),
        "the ninth command starts off-window: {texts:?}"
    );
}

#[test]
fn the_palette_scrolls_down_to_the_last_command() {
    // ↓ walking the selection past the window's bottom edge scrolls the list
    // to keep the highlight visible: with the last command selected the band
    // still shows MENU_MAX_ROWS rows, the top scrolled off and the selection
    // on the bottom row, cyan.
    let last = crate::app::COMMANDS.len() - 1;
    let lines = command_menu_lines(&palette("/", last), 80);
    let texts = command_rows(&lines);
    assert_eq!(texts.len(), MENU_MAX_ROWS as usize);
    assert!(
        !texts.iter().any(|t| t.contains("/help")),
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
    let lines = command_menu_lines(&app, 40); // room for 15 desc columns
    assert!(lines.len() > 1, "the description wrapped: {lines:?}");
    assert!(
        plain(&lines[0]).starts_with("/copy"),
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
fn menu_window_rows_matches_menu_window_for_uniform_heights() {
    // With every entry one row tall the variable-height window IS the fixed
    // one — same offsets, same clamps — so the 80-column palette behaves
    // exactly as before the wrap.
    use crate::ui::menu::menu_window_rows;
    for (len, selected, max) in [(8, 0, 5), (8, 4, 5), (8, 5, 5), (8, 7, 5), (3, 2, 5)] {
        let heights = vec![1usize; len];
        let offset = menu_window(len, selected, max);
        assert_eq!(
            menu_window_rows(&heights, selected, max),
            (offset, (offset + max).min(len)),
            "len {len}, selected {selected}, max {max}"
        );
    }
}

#[test]
fn menu_window_rows_fits_wrapped_entries_to_the_budget() {
    use crate::ui::menu::menu_window_rows;
    // Three-row entries in an eight-row budget: two whole entries fit, and
    // the window follows the selection with it pinned at the bottom edge.
    let heights = vec![3usize; 5];
    assert_eq!(menu_window_rows(&heights, 0, 8), (0, 2));
    assert_eq!(
        menu_window_rows(&heights, 3, 8),
        (2, 4),
        "the selection rides the window's bottom edge"
    );
    // A lone entry taller than the whole budget still windows alone (the
    // caller trims its rows); empty and zero-budget degenerate to nothing.
    assert_eq!(menu_window_rows(&[12, 1], 0, 8), (0, 1));
    assert_eq!(menu_window_rows(&[], 0, 8), (0, 0));
    assert_eq!(menu_window_rows(&[1, 1], 0, 0), (0, 0));
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
    assert!(cmds[0].contains("/help"), "{cmds:?}");
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
fn a_degenerate_width_ellipsizes_the_command_name() {
    // Below the description column there is no room for descriptions at all;
    // the name alone shows, `…`-cut to the width — codex's popup shape
    // (`/statuslin…`) — instead of paint-clipping at the buffer edge.
    let lines = command_menu_lines(&palette("/settings", 0), 7);
    let text = plain(&lines[0]);
    assert!(cols(text.trim_end()) <= 7, "{text:?}");
    assert!(text.trim_end().ends_with('…'), "{text:?}");
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
    assert!(texts.iter().any(|t| t.contains("/help")), "{texts:?}");
    assert!(
        texts
            .iter()
            .any(|t| t.contains("List the available commands")),
        "{texts:?}"
    );
}

#[test]
fn command_menu_aligns_descriptions_in_a_column() {
    // Names are padded so every description starts at the same column,
    // regardless of command-name length; a wrapped description's
    // continuation rows indent to that same column.
    let lines = command_menu_lines(&palette("/", 0), 60);
    for line in &lines {
        let first = line.spans.first().expect("a non-empty row");
        let before_desc: usize = if first.content.as_ref().starts_with('/') {
            line.spans[..2]
                .iter()
                .map(|s| cols(s.content.as_ref()))
                .sum()
        } else {
            cols(first.content.as_ref())
        };
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
        all.contains("/help"),
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

#[test]
fn shortcuts_rows_is_zero_closed_and_counts_the_band_open() {
    let mut app = App::new();
    assert_eq!(shortcuts_rows(&app), 0);
    app.shortcuts_open = true;
    assert_eq!(
        shortcuts_rows(&app),
        SHORTCUTS.len().div_ceil(2) as u16,
        "two entries per row — the $ entry is an ordinary grid slot"
    );
}

#[test]
fn shortcuts_band_advertises_ctrl_v_image_paste() {
    // The `?` overlay lists Ctrl+V image paste (docs/image-paste.md).
    let texts: Vec<String> = shortcuts_lines(false, false)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("ctrl+v for image paste")),
        "{texts:?}"
    );
}

#[test]
fn shortcuts_lines_list_the_bindings_in_two_columns() {
    let texts: Vec<String> = shortcuts_lines(false, false)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(texts.len(), SHORTCUTS.len().div_ceil(2));
    assert!(
        texts[0].contains("/ for commands") && texts[0].contains("! for shell command"),
        "{texts:?}"
    );
    assert!(
        texts[1].contains("↑ for input history") && texts[1].contains("ctrl+r to search history"),
        "{texts:?}"
    );
    assert!(
        texts[2].contains("shift+enter for newline") && texts[2].contains("ctrl+o for tool output"),
        "{texts:?}"
    );
    assert!(
        texts[3].contains("esc to quit") && texts[3].contains("ctrl+c to quit"),
        "{texts:?}"
    );
    assert!(texts[4].contains("alt+↑ to edit queue"), "{texts:?}");
    assert!(
        texts[5].contains("ctrl+v for image paste") && texts[5].contains("ctrl+d for llm context"),
        "{texts:?}"
    );
    // The second column is aligned: both rows' right keys start at the
    // same display column.
    let col = |t: &str, needle: &str| cols(&t[..t.find(needle).unwrap()]);
    assert_eq!(
        col(&texts[0], "!"),
        col(&texts[1], "ctrl+r"),
        "right column aligned: {texts:?}"
    );
}

#[test]
fn shortcuts_lines_list_the_alt_up_queue_edit_binding() {
    // Alt+Up (pull the last queued batch back into the composer,
    // docs/queue.md) is discoverable in the `?` band like every other
    // binding.
    let all: String = shortcuts_lines(false, false)
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("alt+↑ to edit queue"), "{all:?}");
}

#[test]
fn shortcuts_lines_list_the_rebound_session_keys() {
    // Ctrl+T (cycle the thinking mode, docs/reasoning.md), Shift+Tab (the
    // permission mode, docs/permissions.md) and the terminal editing keys
    // are discoverable in the `?` band like every other binding.
    let all: String = shortcuts_lines(false, false)
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("ctrl+t to cycle thinking"), "{all:?}");
    assert!(all.contains("shift+tab for permission mode"), "{all:?}");
    assert!(all.contains("ctrl+w/u/k to kill text"), "{all:?}");
}

#[test]
fn shortcuts_lines_flip_the_esc_entry_while_a_turn_runs() {
    // codex's quit entry is context-sensitive: "to interrupt" while a task
    // runs. Our Esc entry flips the same way.
    let idle: Vec<String> = shortcuts_lines(false, false)
        .iter()
        .map(|l| plain(l))
        .collect();
    let busy: Vec<String> = shortcuts_lines(true, false)
        .iter()
        .map(|l| plain(l))
        .collect();
    assert!(idle.iter().any(|t| t.contains("esc to quit")), "{idle:?}");
    assert!(
        busy.iter().any(|t| t.contains("esc to interrupt")),
        "{busy:?}"
    );
    assert!(!busy.iter().any(|t| t.contains("esc to quit")), "{busy:?}");
}

#[test]
fn the_shortcuts_columns_keep_a_readable_gutter_in_every_state() {
    // The second column starts at SHORTCUTS_COL in every context state, with
    // at least two blank columns of gutter after the first entry — the esc
    // entry swaps text per state (` to interrupt` mid-turn, the `esc esc`
    // edit hint idle-with-target: the widest first-column variant, which
    // used to squeeze the gutter to a single space and read as one run-on
    // line). Adding a wide entry later must widen SHORTCUTS_COL with it.
    for (turn_active, can_backtrack) in [(false, false), (true, false), (false, true), (true, true)]
    {
        for line in shortcuts_lines(turn_active, can_backtrack) {
            if line.spans.len() < 4 {
                continue; // a lone trailing entry has no second column
            }
            let first: usize = line.spans[..2]
                .iter()
                .map(|s| cols(s.content.as_ref()))
                .sum();
            let gutter = cols(line.spans[2].content.as_ref());
            assert!(
                gutter >= 2,
                "a {gutter}-space gutter after {:?} (turn_active={turn_active}, can_backtrack={can_backtrack})",
                plain(&line)
            );
            assert_eq!(
                first + gutter,
                SHORTCUTS_COL,
                "second column aligned: {:?}",
                plain(&line)
            );
        }
    }
}

#[test]
fn shortcuts_lines_style_keys_cyan_and_labels_dim() {
    for line in shortcuts_lines(false, false) {
        // spans = [key, label, pad, key, label] — keys cyan, labels dim.
        assert_eq!(line.spans[0].style.fg, Some(shortcuts_key_color()));
        assert_eq!(line.spans[1].style.fg, Some(shortcuts_text_color()));
    }
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
        shortcuts_rows(&app),
        0,
        0,
    );
    assert_eq!(open, closed + shortcuts_rows(&app));
}

#[test]
fn render_live_draws_the_shortcuts_band_below_the_box() {
    let mut app = App::new();
    app.shortcuts_open = true;
    let h = live_height(
        &app.input,
        60,
        24,
        false,
        0,
        0,
        0,
        0,
        shortcuts_rows(&app),
        0,
        0,
    );
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    let all: String = (0..h)
        .map(|y| row(&buf, y, 60))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("/ for commands"), "band rendered: {all:?}");
    assert!(
        row(&buf, h - 1, 60).contains("$ for skills"),
        "the last band row sits on the last region row"
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
            shortcuts_rows(&app),
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
    let q = queued_rows(&app, 40);
    let h = live_height(
        &app.input,
        40,
        24,
        true,
        1,
        0,
        q,
        0,
        shortcuts_rows(&app),
        0,
        0,
    );
    let mut buf = buffer(40, h);
    render_live(buf.area, &mut buf, &app);
    let rows: Vec<String> = (0..h).map(|y| row(&buf, y, 40)).collect();
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
    assert!(all.contains("/help"), "palette shown: {all:?}");
    assert!(
        !all.contains("dummy_model_name"),
        "the footer yields its slot: {all:?}"
    );
}

#[test]
fn the_shortcuts_band_lists_ctrl_r() {
    let texts: Vec<String> = shortcuts_lines(false, false).iter().map(plain).collect();
    assert!(
        texts.iter().any(|t| t.contains("ctrl+r to search history")),
        "band lists the search binding: {texts:?}"
    );
}

#[test]
fn the_shortcuts_band_lists_the_bang() {
    let texts: Vec<String> = shortcuts_lines(false, false).iter().map(plain).collect();
    assert!(
        texts.iter().any(|t| t.contains("! for shell command")),
        "band lists the shell binding: {texts:?}"
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
    let idle: String = shortcuts_lines(false, false).iter().map(plain).collect();
    assert!(idle.contains("esc to quit"), "{idle:?}");
    let busy: String = shortcuts_lines(true, true).iter().map(plain).collect();
    assert!(busy.contains("esc to interrupt"), "{busy:?}");
    let target: String = shortcuts_lines(false, true).iter().map(plain).collect();
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
    // The `$` skill picker (docs/skill-mentions.md) sits in the last row's
    // first column — a plain [`SHORTCUTS`] entry beside the kill keys, no
    // third-column special case (the band is one two-column grid at every
    // width):
    //   $ for skills                  ctrl+w/u/k to kill text
    let lines = shortcuts_lines(false, false);
    assert_eq!(lines.len(), SHORTCUTS.len().div_ceil(2), "no extra row");
    let last = plain(lines.last().expect("a band row"));
    assert!(
        last.contains("$ for skills") && last.contains("ctrl+w/u/k to kill text"),
        "the $ entry shares the last row with the kill keys: {last:?}"
    );
    assert!(
        !plain(&lines[0]).contains('$'),
        "no first-row third column any more: {:?}",
        plain(&lines[0])
    );
    let listed = lines
        .iter()
        .filter(|l| plain(l).contains("$ for skills"))
        .count();
    assert_eq!(listed, 1, "listed once");
}
