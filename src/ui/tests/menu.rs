//! The bands below the box: palette, `@` picker, `?` shortcuts
//! (`docs/file-search.md`, `docs/shortcuts.md`).

use super::*;
use crate::ui::theme::{
    FILE_MENU_MAX_ROWS, MENU_DESC_COL, MENU_MAX_ROWS, MENU_SELECTED_COLOR, SHORTCUTS,
    SHORTCUTS_COL, SHORTCUTS_KEY_COLOR, SHORTCUTS_TEXT_COLOR,
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

#[test]
fn menu_rows_is_zero_when_the_palette_is_closed() {
    assert_eq!(menu_rows(&App::new()), 0);
}

#[test]
fn the_palette_shows_at_most_eight_commands() {
    // The requested cap (the file picker's): a bare `/` shows the first eight
    // commands and longer match lists scroll (menu_window) instead of growing
    // the band — the registry has outgrown the window, so /quit (the ninth)
    // starts off-window (smoke.sh Phase 4 asserts the same on the real
    // binary).
    assert_eq!(MENU_MAX_ROWS, 8, "the requested cap");
    assert!(
        crate::app::COMMANDS.len() > MENU_MAX_ROWS as usize,
        "the registry outgrew the window — scrolling is exercised by a bare `/`"
    );
    let texts: Vec<String> = command_menu_lines(&palette("/", 0), 60)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(texts.len(), MENU_MAX_ROWS as usize, "{texts:?}");
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
    let lines = command_menu_lines(&palette("/", last), 60);
    assert_eq!(lines.len(), MENU_MAX_ROWS as usize);
    let texts: Vec<String> = lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
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
            .any(|s| s.style.fg == Some(MENU_SELECTED_COLOR)),
        "the selection rode the window down"
    );
}

#[test]
fn menu_rows_counts_matches_capped_with_a_placeholder_for_none() {
    assert_eq!(
        menu_rows(&palette("/", 0)),
        (crate::app::COMMANDS.len() as u16).min(MENU_MAX_ROWS),
        "match count, capped at the max"
    );
    assert_eq!(menu_rows(&palette("/zzz", 0)), 1, "placeholder row");
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
    // regardless of command-name length.
    let lines = command_menu_lines(&palette("/", 0), 60);
    for line in &lines {
        let before_desc: usize = line.spans[..2]
            .iter()
            .map(|s| cols(s.content.as_ref()))
            .sum();
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
    let menu = menu_rows(&app);
    let h = live_height(&app.input, 40, 24, false, 0, 0, 0, menu, 0, 0);
    let mut buf = buffer(40, h);
    render_live(buf.area, &mut buf, &app);
    let all: String = (0..h)
        .map(|y| row(&buf, y, 40))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all.contains("/help"),
        "menu rendered below the box: {all:?}"
    );
    assert!(
        row(&buf, h - 1, 40).contains('/'),
        "a command sits on the last row"
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
        live_height(&TextArea::from_text("/"), 40, 24, false, 0, 0, 0, 0, 0, 0),
    );
    let closed = cursor_position(closed_area, &app);
    app.command_menu = Some(crate::app::CommandMenu { selected: 0 });
    let menu = menu_rows(&app);
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
        "two entries per row"
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
fn shortcuts_lines_list_the_shift_tab_thinking_binding() {
    // Shift+Tab (cycle the thinking mode, docs/reasoning.md) is
    // discoverable in the `?` band like every other binding.
    let all: String = shortcuts_lines(false, false)
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("shift+tab to cycle thinking"), "{all:?}");
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
        assert_eq!(line.spans[0].style.fg, Some(SHORTCUTS_KEY_COLOR));
        assert_eq!(line.spans[1].style.fg, Some(SHORTCUTS_TEXT_COLOR));
    }
}

#[test]
fn live_height_adds_the_shortcuts_band() {
    let mut app = App::new();
    app.shortcuts_open = true;
    let closed = live_height(&app.input, 40, 24, false, 0, 0, 0, 0, 0, 0);
    let open = live_height(
        &app.input,
        40,
        24,
        false,
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
        row(&buf, h - 1, 60).contains("shift+tab to cycle thinking"),
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
        live_height(&app.input, 40, 24, false, 0, 0, 0, 0, 0, 0),
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
    let band = menu_rows(&app);
    let h = live_height(
        &app.input,
        60,
        24,
        false,
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
            .any(|s| s.style.fg == Some(MENU_SELECTED_COLOR))
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|s| s.style.fg != Some(MENU_SELECTED_COLOR))
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
    let h = live_height(&app.input, 40, 24, false, 0, 0, 0, band, 0, 0);
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
