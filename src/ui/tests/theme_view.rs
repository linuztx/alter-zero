//! The inline `/theme` picker view (`docs/theme.md`).

use super::*;
use crate::app::Theme;
use crate::ui::palette::{active_theme, palette_of};
use crate::ui::theme_view::{theme_menu_rows, theme_view_lines};
use crate::ui::wrap::cols;

/// A terminal wide enough for the side-by-side page.
const WIDE: u16 = 100;
/// A terminal too narrow for it: the preview stacks under the list.
const NARROW: u16 = 60;
/// A terminal tall enough for the whole page at any of these widths.
const TALL: u16 = 200;
/// A terminal too short for the whole page at either width (the side-by-side
/// page is twenty rows, the stacked one thirty-two).
const SHORT: u16 = 15;

/// An app with session info and the picker open (the ordinary shape).
fn open_app() -> App {
    let mut app = with_session();
    app.open_theme_picker();
    app
}

/// `app` with the highlight moved to `selected`.
fn select(app: &mut App, selected: usize) {
    app.theme_picker.as_mut().expect("open").selected = selected;
}

fn texts(app: &App, width: u16) -> Vec<String> {
    texts_at(app, width, TALL)
}

/// The page's rows on a terminal `height` rows tall, trailing blanks trimmed.
fn texts_at(app: &App, width: u16, height: u16) -> Vec<String> {
    theme_view_lines(app, width, height)
        .iter()
        .map(|line| plain(line).trim_end().to_string())
        .collect()
}

/// Whether `text` is the list row for `name`: past the indent and the `❯`
/// marker it opens with the name (a caption or preview row mentioning the
/// same word doesn't count).
fn is_row_for(text: &str, name: &str) -> bool {
    let t = text.trim_start();
    let t = t.strip_prefix("❯ ").unwrap_or(t);
    t.starts_with(name) && t[name.len()..].chars().next().is_none_or(|c| c == ' ')
}

/// The page's row index for `name`.
fn row_index(texts: &[String], name: &str) -> usize {
    texts
        .iter()
        .position(|t| is_row_for(t, name))
        .unwrap_or_else(|| panic!("no row for {name}: {texts:#?}"))
}

/// The page's row for `name`.
fn row_for<'a>(texts: &'a [String], name: &str) -> &'a String {
    &texts[row_index(texts, name)]
}

/// The styled list row for `name`.
fn line_for<'a>(lines: &'a [Line<'static>], name: &str) -> &'a Line<'static> {
    lines
        .iter()
        .find(|l| is_row_for(&plain(l), name))
        .unwrap_or_else(|| panic!("no row for {name}"))
}

/// The display column `needle` starts at in `text`.
fn col_of(text: &str, needle: &str) -> usize {
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {text:?}"));
    cols(&text[..at])
}

/// Whether the page at `width` lays the preview **beside** the list: some
/// row carries both a theme row's swatches and the preview's `Edit` header.
fn is_beside(app: &App, width: u16) -> bool {
    texts(app, width)
        .iter()
        .any(|t| t.contains("●●●●●") && t.contains("Edit(greet.py)"))
}

/// The preview's `● Edit(greet.py)` header line.
fn edit_header<'a>(lines: &'a [Line<'static>]) -> &'a Line<'static> {
    lines
        .iter()
        .find(|l| plain(l).contains("Edit(greet.py)"))
        .expect("the preview's Edit header")
}

/// The colour of the first tool/reply bullet (`● `) on `line` — never a
/// swatch, which is a bare `●`.
fn bullet_fg(line: &Line<'static>) -> Option<Color> {
    line.spans
        .iter()
        .find(|s| s.content == "● ")
        .and_then(|s| s.style.fg)
}

#[test]
fn closed_builds_nothing() {
    assert!(theme_view_lines(&with_session(), 80, TALL).is_empty());
    assert_eq!(theme_picker_height(&with_session(), 80, 200), None);
}

#[test]
fn the_page_is_framed_with_search_rows_preview_and_hint() {
    for width in [WIDE, NARROW] {
        let texts = texts(&open_app(), width);
        let is_rule = |t: &String| !t.trim().is_empty() && t.trim().chars().all(|c| c == '─');
        assert!(is_rule(texts.first().expect("a top rule")), "top rule");
        assert!(is_rule(texts.last().expect("a bottom rule")), "bottom rule");
        assert!(
            texts[2].trim_start().starts_with('❯'),
            "the ❯ search line sits on its fixed row: {texts:#?}"
        );
        assert!(
            row_for(&texts, "mocha").starts_with("  ❯ mocha"),
            "the open seats the marker on the default: {texts:#?}"
        );
        let title = texts
            .iter()
            .position(|t| t.contains("Catppuccin Mocha"))
            .expect("the highlighted theme's title captions the preview");
        assert!(
            texts[title + 1].contains("The darkest flavour, and the default"),
            "its blurb on the row under it: {texts:#?}"
        );
        assert!(
            texts.iter().any(|t| t
                .trim()
                .starts_with("↑/↓ to navigate · Type to search · Enter to choose")),
            "the key hint leads with navigation: {texts:#?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("/11)")),
            "no (n/total) counter — the overflow rows say what is hidden: {texts:#?}"
        );
    }
}

#[test]
fn rows_wear_the_mcp_lists_marker_and_indent() {
    // The /mcp list's shape: the selected row is the indent, the accent `❯`
    // and the name; every other row keeps the marker's width blank, so the
    // names line up under it.
    let mut app = open_app();
    select(&mut app, 2);
    for width in [WIDE, NARROW] {
        let lines = theme_view_lines(&app, width, TALL);
        let texts: Vec<String> = lines.iter().map(plain).collect();
        assert!(
            row_for(&texts, "frappe").starts_with("  ❯ frappe"),
            "{texts:#?}"
        );
        for name in ["mocha", "macchiato", "latte", "onedark"] {
            assert!(
                row_for(&texts, name).starts_with(&format!("    {name}")),
                "{name} is indented under the marker: {texts:#?}"
            );
        }
        let marker = line_for(&lines, "frappe")
            .spans
            .iter()
            .find(|s| s.content == "❯ ")
            .expect("the marker span");
        assert_eq!(
            marker.style.fg,
            Some(palette_of(Theme::Mocha).accent),
            "the marker wears the active accent"
        );
    }
}

#[test]
fn the_overflow_rows_say_what_is_hidden() {
    // Ten rows of eleven, in the /mcp list's shape: a gap under the search
    // line, then a dim `↑ N more above` row only when rows are scrolled off
    // the top, the rows, and a dim `↓ N more below` row only when rows are
    // below — one of the two at any time, so the frame never moves.
    let mut app = open_app();
    for width in [WIDE, NARROW] {
        select(&mut app, 0);
        let top: Vec<String> = texts(&app, width)
            .iter()
            .map(|t| t.trim_end().to_string())
            .collect();
        assert_eq!(top[3], "", "the gap under the search line: {top:#?}");
        assert_eq!(row_index(&top, "mocha"), 4, "no `↑` row: {top:#?}");
        assert_eq!(row_index(&top, "monokai"), 13, "{top:#?}");
        assert!(
            top[14].starts_with("  ↓ 1 more below"),
            "the one theme below: {top:#?}"
        );
        assert!(
            !top.iter().any(|t| is_row_for(t, "ansi")),
            "ansi is scrolled off: {top:#?}"
        );

        select(&mut app, Theme::ALL.len() - 1);
        let end: Vec<String> = texts(&app, width)
            .iter()
            .map(|t| t.trim_end().to_string())
            .collect();
        assert_eq!(end[3], "", "the gap under the search line: {end:#?}");
        assert!(
            end[4].starts_with("  ↑ 1 more above"),
            "the one theme above: {end:#?}"
        );
        assert_eq!(row_index(&end, "macchiato"), 5, "{end:#?}");
        assert!(
            end[14].starts_with("  ❯ ansi"),
            "the selection on the last row: {end:#?}"
        );
        assert!(
            !end.iter().any(|t| t.trim_start().starts_with('↓')),
            "nothing below the last row: {end:#?}"
        );
        assert!(
            !end.iter().any(|t| is_row_for(t, "mocha")),
            "mocha is scrolled off: {end:#?}"
        );
    }
}

#[test]
fn stacked_the_list_is_set_off_from_the_caption_by_a_gap() {
    // Under the list the `↓` row, when there is one, then always a blank row
    // before the caption — never the overflow row flush against the title.
    let mut app = open_app();
    for selected in [0, Theme::ALL.len() - 1] {
        select(&mut app, selected);
        let texts: Vec<String> = texts(&app, NARROW)
            .iter()
            .map(|t| t.trim_end().to_string())
            .collect();
        let title = app.highlighted_theme().expect("a highlight").title();
        let caption = texts
            .iter()
            .position(|t| t.trim_start() == title)
            .unwrap_or_else(|| panic!("the caption: {texts:#?}"));
        assert_eq!(texts[caption - 1], "", "a gap over the caption: {texts:#?}");
        assert!(
            is_row_for(&texts[caption - 2], "ansi")
                || texts[caption - 2].starts_with("  ↓ 1 more below"),
            "the list (or its `↓` row) just above the gap: {texts:#?}"
        );
    }
}

#[test]
fn a_wide_terminal_lays_the_preview_beside_the_list() {
    // codex's theme picker: the list on the left, the highlighted theme's
    // sample on the right, every preview row starting on one column.
    let texts = texts(&open_app(), WIDE);
    let mocha = row_for(&texts, "mocha");
    assert!(
        mocha.contains("Catppuccin Mocha"),
        "the title sits beside the first row: {texts:#?}"
    );
    let column = col_of(mocha, "Catppuccin Mocha");
    for (row, piece) in [
        ("macchiato", "The darkest flavour, and the default"),
        ("latte", "❯ Rename the greeting in greet.py"),
        ("onedark", "● Edit(greet.py)"),
        ("monokai", "● Done — greet.py greets by name now:"),
    ] {
        let text = row_for(&texts, row);
        assert!(text.contains(piece), "{piece:?} beside {row}: {texts:#?}");
        assert_eq!(
            col_of(text, piece),
            column,
            "{piece:?} on the preview column"
        );
    }
    let code = texts
        .iter()
        .position(|t| t.contains("greet(\"Alter Zero\")"))
        .expect("the sample's code line");
    assert!(
        texts[code - 1].trim_start().starts_with("↓ 1 more below"),
        "the list closes just above the sample's last row: {texts:#?}"
    );
}

#[test]
fn a_standard_terminal_gets_the_side_by_side_page_and_a_narrow_one_stacks() {
    let app = open_app();
    assert!(is_beside(&app, 80), "an 80-column terminal");
    assert!(is_beside(&app, WIDE));
    assert!(!is_beside(&app, NARROW), "a 60-column terminal stacks");
    // Stacked, the preview hangs under the list, inset two columns, its
    // title and blurb first.
    let texts = texts(&app, NARROW);
    let caption = texts
        .iter()
        .position(|t| t.contains("Catppuccin Mocha"))
        .expect("the caption");
    assert!(caption > row_index(&texts, "monokai"), "{texts:#?}");
    assert_eq!(
        texts[caption].trim_end(),
        "  Catppuccin Mocha",
        "{texts:#?}"
    );
    assert_eq!(
        texts[caption + 1].trim_end(),
        "  The darkest flavour, and the default",
        "{texts:#?}"
    );
    let bubble = texts
        .iter()
        .position(|t| t.contains("Rename the greeting"))
        .expect("the bubble");
    assert!(bubble > caption, "{texts:#?}");
    assert!(texts[bubble].starts_with("  ❯ Rename"), "{texts:#?}");
}

#[test]
fn the_layout_switches_once_and_beside_the_list_the_sample_never_wraps() {
    // One breakpoint: every width from it up lays the preview beside the
    // list, every width under it stacks. And the breakpoint is where the
    // sample still fits unwrapped, so the side-by-side page is one height at
    // every width that gets it.
    let app = open_app();
    let switch = (20u16..400)
        .find(|&w| is_beside(&app, w))
        .expect("some width lays the preview beside the list");
    for width in 20u16..switch {
        assert!(!is_beside(&app, width), "width {width} is under the switch");
    }
    let tall = theme_view_lines(&app, 400, TALL).len();
    for width in switch..400 {
        assert!(is_beside(&app, width), "width {width} is past the switch");
        assert_eq!(
            theme_view_lines(&app, width, TALL).len(),
            tall,
            "width {width} wrapped the preview beside the list"
        );
    }
}

#[test]
fn the_preview_column_holds_still_as_the_list_moves_and_filters() {
    // The list column is sized off the whole catalog, so neither ↑/↓ nor a
    // search moves the preview sideways.
    let mut app = open_app();
    let column_of = |app: &App| {
        let texts = texts(app, WIDE);
        let theme = app.highlighted_theme().expect("a highlight");
        let caption = texts
            .iter()
            .find(|t| t.contains(theme.title()))
            .unwrap_or_else(|| panic!("the {theme:?} caption: {texts:#?}"));
        col_of(caption, theme.title())
    };
    let home = column_of(&app);
    for i in 0..Theme::ALL.len() {
        select(&mut app, i);
        assert_eq!(column_of(&app), home, "selection {i} moved the preview");
    }
    for query in ["ca", "nord", "drac", "ansi"] {
        let picker = app.theme_picker.as_mut().expect("open");
        picker.query = query.to_string();
        picker.selected = 0;
        assert_eq!(column_of(&app), home, "query {query:?} moved the preview");
    }
}

#[test]
fn the_caption_is_codexs_bold_title_over_a_dim_subtitle() {
    // The highlighted theme's title in bold, and under it the rest of its
    // description — the title not repeated, the first letter capitalised —
    // in the frame's dim: codex's `Select Syntax Theme` header, naming the
    // theme the preview wears.
    let mut app = open_app();
    select(&mut app, 7); // gruvbox
    for width in [WIDE, NARROW] {
        let lines = theme_view_lines(&app, width, TALL);
        let at = lines
            .iter()
            .position(|l| plain(l).contains("Gruvbox Dark"))
            .expect("the title row");
        let title = lines[at]
            .spans
            .iter()
            .find(|s| s.content == "Gruvbox Dark")
            .expect("the title span");
        assert!(
            title.style.add_modifier.contains(Modifier::BOLD),
            "{title:?}"
        );
        let blurb = lines[at + 1]
            .spans
            .iter()
            .find(|s| s.content.contains("Retro groove, warm and earthy"))
            .unwrap_or_else(|| panic!("the subtitle span: {:?}", lines[at + 1]));
        assert!(
            !blurb.style.add_modifier.contains(Modifier::BOLD),
            "{blurb:?}"
        );
        assert_eq!(
            blurb.style.fg,
            Some(palette_of(Theme::Mocha).dim),
            "the subtitle is the frame's dim"
        );
        assert!(
            !plain(&lines[at + 1]).contains("Gruvbox Dark —"),
            "the title is not repeated: {:?}",
            lines[at + 1]
        );
    }
}

#[test]
fn a_standard_terminal_shows_every_caption_whole() {
    // Split over two rows the caption fits the preview column: at 80
    // columns every title, and every subtitle but the terminal theme's
    // long one, reads whole; at 100 every one does.
    let mut app = open_app();
    for (width, cut) in [(80u16, vec![Theme::Ansi]), (WIDE, vec![])] {
        for (i, theme) in Theme::ALL.iter().enumerate() {
            select(&mut app, i);
            let texts = texts(&app, width);
            assert!(
                texts.iter().any(|t| t.contains(theme.title())),
                "{theme:?}'s title at {width}: {texts:#?}"
            );
            let whole = !texts.iter().any(|t| t.contains('…'));
            assert_eq!(
                whole,
                !cut.contains(theme),
                "{theme:?}'s caption at {width}: {texts:#?}"
            );
        }
    }
}

#[test]
fn the_preview_is_the_sample_conversations_real_cells() {
    // A user bubble, the Edit diff cell (head, context row, the removed and
    // added rows), and the reply with its inline code and fenced code line
    // — through the conversation's own builders, beside the list or under it.
    for width in [WIDE, NARROW] {
        let texts = texts(&open_app(), width);
        for expect in [
            "❯ Rename the greeting in greet.py",
            "● Edit(greet.py)",
            "⎿  Updated greet.py (+1 -1)",
            "def greet(name):",
            "-    print(\"Hello, world\")",
            "+    print(f\"Hello, {name}\")",
            "● Done — greet.py greets by name now:",
            "greet(\"Alter Zero\")  # Hello, Alter Zero",
        ] {
            assert!(
                texts.iter().any(|t| t.contains(expect)),
                "width {width} is missing {expect:?}: {texts:#?}"
            );
        }
    }
}

#[test]
fn every_row_wears_its_own_swatch() {
    // Five swatches per row, each in that theme's own accent, link,
    // success, warning and error — read off its palette, never the active
    // theme's, so Dracula's row is Dracula-coloured while Mocha is active.
    // Beside the list the preview's bullets share the row, but a bullet is
    // `● ` and a swatch a bare `●`.
    for width in [WIDE, NARROW] {
        let lines = theme_view_lines(&open_app(), width, TALL);
        for theme in Theme::ALL.iter().take(10) {
            let p = palette_of(*theme);
            let swatches: Vec<Color> = line_for(&lines, theme.name())
                .spans
                .iter()
                .filter(|s| s.content == "●")
                .filter_map(|s| s.style.fg)
                .collect();
            assert_eq!(
                swatches,
                [p.accent, p.link, p.success, p.warning, p.error],
                "{theme:?}'s swatches at width {width}"
            );
        }
    }
    assert_eq!(active_theme(), Theme::Mocha, "the swatches leak nothing");
}

#[test]
fn the_preview_wears_the_highlighted_theme_and_the_frame_the_active_one() {
    for width in [WIDE, NARROW] {
        let mut app = open_app();
        let mocha = theme_view_lines(&app, width, TALL);
        assert_eq!(
            bullet_fg(edit_header(&mocha)),
            Some(palette_of(Theme::Mocha).success),
            "the Edit bullet is Mocha's green while Mocha is highlighted"
        );
        select(&mut app, 5); // dracula, the sixth row
        let dracula = theme_view_lines(&app, width, TALL);
        assert_eq!(
            bullet_fg(edit_header(&dracula)),
            Some(palette_of(Theme::Dracula).success),
            "…and Dracula's green once Dracula is highlighted"
        );
        // The code line is syntax-coloured by the highlighted theme's own
        // code theme, so the two previews disagree on the `def` keyword.
        let keyword = |lines: &[Line<'static>]| {
            lines
                .iter()
                .find(|l| plain(l).contains("def greet(name):"))
                .expect("the context row")
                .spans
                .iter()
                .find(|s| s.content.contains("def"))
                .expect("the keyword segment")
                .style
                .fg
        };
        assert!(keyword(&mocha).is_some());
        assert_ne!(keyword(&mocha), keyword(&dracula), "two code themes");
        // The frame around the preview stays in the ACTIVE theme: the `❯`
        // prompt is Mocha's accent on both pages, and nothing leaked.
        let prompt = |lines: &[Line<'static>]| lines[2].spans[1].style.fg;
        assert_eq!(prompt(&mocha), Some(palette_of(Theme::Mocha).accent));
        assert_eq!(prompt(&dracula), Some(palette_of(Theme::Mocha).accent));
        assert_eq!(active_theme(), Theme::Mocha);
        assert!(
            texts(&app, width)
                .iter()
                .any(|t| t.contains("Vivid purple, pink and cyan on a dark ground")),
            "…under Dracula's caption"
        );
    }
}

#[test]
fn the_diff_rows_wear_the_highlighted_themes_tints() {
    let mut app = open_app();
    select(&mut app, 3); // latte
    let p = palette_of(Theme::Latte);
    for width in [WIDE, NARROW] {
        let lines = theme_view_lines(&app, width, TALL);
        let removed = lines
            .iter()
            .find(|l| plain(l).contains("-    print(\"Hello, world\")"))
            .expect("the removed row");
        assert!(
            removed.spans.iter().any(
                |s| s.style.bg == Some(p.diff_del_bg) || s.style.bg == Some(p.diff_del_mark_bg)
            ),
            "the removed row is tinted in Latte's red: {removed:?}"
        );
        let added = lines
            .iter()
            .find(|l| plain(l).contains("+    print(f\"Hello, {name}\")"))
            .expect("the added row");
        assert!(
            added.spans.iter().any(
                |s| s.style.bg == Some(p.diff_add_bg) || s.style.bg == Some(p.diff_add_mark_bg)
            ),
            "the added row is tinted in Latte's green: {added:?}"
        );
        let bubble = lines
            .iter()
            .find(|l| plain(l).contains("Rename the greeting"))
            .expect("the user bubble");
        assert!(
            bubble.spans.iter().any(|s| s.style.bg == Some(p.user_bg)),
            "the bubble is Latte's pale ground: {bubble:?}"
        );
    }
}

#[test]
fn the_bubble_ground_stays_in_the_preview_column() {
    // The bubble's ground is a line style in the conversation; beside the
    // list it is folded onto the bubble's own spans, so it never paints the
    // theme row sharing its line.
    let texts = texts(&open_app(), WIDE);
    let lines = theme_view_lines(&open_app(), WIDE, TALL);
    let column = col_of(row_for(&texts, "mocha"), "Catppuccin Mocha");
    let bubble = lines
        .iter()
        .find(|l| plain(l).contains("Rename the greeting"))
        .expect("the user bubble");
    assert_eq!(bubble.style.bg, None, "no line-wide ground: {bubble:?}");
    let user_bg = palette_of(Theme::Mocha).user_bg;
    let mut at = 0;
    let mut grounded = 0;
    for span in &bubble.spans {
        if span.style.bg == Some(user_bg) {
            assert!(
                at >= column,
                "a grounded span left of the preview: {bubble:?}"
            );
            grounded += 1;
        } else if at < column {
            assert_eq!(span.style.bg, None, "the list row is unpainted: {span:?}");
        }
        at += cols(&span.content);
    }
    assert!(grounded > 0, "the bubble keeps its ground: {bubble:?}");
}

#[test]
fn the_page_height_is_stable_across_selections() {
    // The sample is the same shape for every theme and the overflow rows
    // are always there, so moving the selection must not move the frame
    // (the /settings always-emit rule) — even onto the ANSI theme, whose
    // cells carry no RGB.
    for width in [WIDE, 80, NARROW] {
        let mut app = open_app();
        let h0 = theme_view_lines(&app, width, TALL).len();
        for i in 1..Theme::ALL.len() {
            select(&mut app, i);
            assert_eq!(
                theme_view_lines(&app, width, TALL).len(),
                h0,
                "selection {i} changed the page height at width {width}"
            );
        }
    }
}

#[test]
fn the_side_by_side_page_is_the_shorter_one() {
    let app = open_app();
    assert!(
        theme_view_lines(&app, WIDE, TALL).len() < theme_view_lines(&app, NARROW, TALL).len(),
        "laying the preview beside the list saves its rows"
    );
}

#[test]
fn the_active_theme_wears_the_check_and_the_open_seats_on_it() {
    let mut app = with_session();
    app.set_theme(Theme::Nord);
    app.open_theme_picker();
    for width in [WIDE, NARROW] {
        let texts = texts(&app, width);
        assert!(
            row_for(&texts, "nord").contains('✓'),
            "nord carries the ✓: {texts:#?}"
        );
        // The window has scrolled `mocha` off the top; of the rows it shows,
        // only nord's is marked.
        assert_eq!(
            texts.iter().filter(|t| t.contains('✓')).count(),
            1,
            "only the active row is marked: {texts:#?}"
        );
        assert!(
            row_for(&texts, "nord").starts_with("  ❯ nord"),
            "the highlight seats on the active theme: {texts:#?}"
        );
    }
}

#[test]
fn an_unmatched_search_collapses_to_placeholder_and_hint() {
    let mut app = open_app();
    app.theme_picker.as_mut().expect("open").query = "zzz".to_string();
    for width in [WIDE, NARROW] {
        let texts: Vec<String> = texts(&app, width)
            .iter()
            .map(|t| t.trim_end().to_string())
            .collect();
        let is_rule = |t: &str| !t.is_empty() && t.chars().all(|c| c == '─');
        assert_eq!(texts.len(), 9, "the collapsed page is 9 rows: {texts:#?}");
        assert!(is_rule(&texts[0]), "{texts:#?}");
        assert_eq!(texts[1], "", "{texts:#?}");
        assert!(texts[2].contains('❯'), "the search line: {texts:#?}");
        assert_eq!(texts[3], "", "{texts:#?}");
        assert!(
            texts[4].contains("No matching themes"),
            "the placeholder: {texts:#?}"
        );
        assert_eq!(texts[5], "", "one gap under the placeholder: {texts:#?}");
        assert!(texts[6].contains("↑/↓ to navigate"), "the hint: {texts:#?}");
        assert_eq!(texts[7], "", "{texts:#?}");
        assert!(is_rule(&texts[8]), "{texts:#?}");
    }
}

#[test]
fn no_page_ever_stacks_two_blank_rows() {
    let mut app = open_app();
    for width in [WIDE, NARROW] {
        for query in ["", "ca", "nord", "zzz"] {
            let len = {
                let picker = app.theme_picker.as_mut().expect("open");
                picker.query = query.to_string();
                app.theme_rows().len().max(1)
            };
            for i in 0..len {
                select(&mut app, i);
                let texts: Vec<String> = texts(&app, width)
                    .iter()
                    .map(|t| t.trim_end().to_string())
                    .collect();
                for pair in texts.windows(2) {
                    assert!(
                        !(pair[0].is_empty() && pair[1].is_empty()),
                        "width {width} query {query:?} selection {i} stacked two blank rows: {texts:#?}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_page_never_exceeds_the_width() {
    let mut app = open_app();
    for width in [24u16, 40, 60, 70, 71, 72, 73, 80, 100, 120, 200] {
        for i in 0..Theme::ALL.len() {
            select(&mut app, i);
            for line in theme_view_lines(&app, width, TALL) {
                assert!(
                    cols(&plain(&line)) <= width as usize,
                    "width {width} selection {i}: {:?} overflows",
                    plain(&line)
                );
            }
        }
    }
}

#[test]
fn height_is_the_line_count_clamped_to_the_terminal() {
    let app = open_app();
    let lines = theme_view_lines(&app, 80, TALL).len() as u16;
    assert_eq!(theme_menu_rows(&app, 80, TALL), lines);
    assert_eq!(
        theme_picker_height(&app, 80, 200),
        Some(lines),
        "an idle app has no strip above the picker"
    );
    assert_eq!(
        theme_picker_height(&app, 80, 10),
        Some(10),
        "clamped to a short terminal"
    );
}

#[test]
fn a_short_terminal_keeps_the_theme_choices_on_screen() {
    // codex's short-panel rule: the preview yields first, so the search
    // line, three theme rows and the hint fit where the whole page cannot —
    // where the bottom anchor used to keep only the preview on screen and
    // flow the theme names into scrollback.
    for width in [WIDE, NARROW] {
        let texts = texts_at(&open_app(), width, SHORT);
        assert!(texts.len() <= usize::from(SHORT), "fits: {texts:#?}");
        assert!(
            texts[2].trim_start().starts_with('❯'),
            "the search line on its fixed row: {texts:#?}"
        );
        assert!(
            row_for(&texts, "mocha").starts_with("  ❯ mocha"),
            "{texts:#?}"
        );
        assert_eq!(
            texts.iter().filter(|t| t.contains("●●●●●")).count(),
            3,
            "three theme rows: {texts:#?}"
        );
        assert!(
            texts.iter().any(|t| t.trim_start() == "↓ 8 more below"),
            "the rest named under them: {texts:#?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("↑/↓ to navigate")),
            "the hint: {texts:#?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("Edit(greet.py)")),
            "no preview: {texts:#?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("Catppuccin Mocha")),
            "no caption: {texts:#?}"
        );
    }
}

#[test]
fn the_compact_page_is_codexs_short_picker() {
    // Rule, gap, the search line with the `↑` slot directly under it (codex's
    // search row over its arrow), three rows, the `↓` slot directly over the
    // hint, gap, rule. Both slots are always there — blank when nothing is
    // hidden that way — so the page is one height at every selection, and
    // a blank slot is the gap the page wants there anyway.
    let texts = texts_at(&open_app(), NARROW, SHORT);
    let is_rule = |t: &str| !t.is_empty() && t.chars().all(|c| c == '─');
    assert_eq!(texts.len(), 11, "{texts:#?}");
    assert!(is_rule(&texts[0]), "{texts:#?}");
    assert_eq!(texts[1], "", "{texts:#?}");
    assert_eq!(texts[2], "  ❯", "the search line: {texts:#?}");
    assert_eq!(texts[3], "", "nothing above the first row: {texts:#?}");
    assert!(texts[4].starts_with("  ❯ mocha"), "{texts:#?}");
    assert!(texts[5].starts_with("    macchiato"), "{texts:#?}");
    assert!(texts[6].starts_with("    frappe"), "{texts:#?}");
    assert_eq!(texts[7], "  ↓ 8 more below", "{texts:#?}");
    assert!(
        texts[8].starts_with("  ↑/↓ to navigate · Type to search"),
        "{texts:#?}"
    );
    assert_eq!(texts[9], "", "{texts:#?}");
    assert!(is_rule(&texts[10]), "{texts:#?}");
}

#[test]
fn the_compact_window_keeps_the_selection_in_its_middle() {
    let mut app = open_app();
    select(&mut app, 5); // dracula: one row either side, the rest named
    let middle = texts_at(&app, NARROW, SHORT);
    assert_eq!(middle[3], "  ↑ 4 more above", "{middle:#?}");
    assert!(middle[4].starts_with("    onedark"), "{middle:#?}");
    assert!(middle[5].starts_with("  ❯ dracula"), "{middle:#?}");
    assert!(middle[6].starts_with("    nord"), "{middle:#?}");
    assert_eq!(middle[7], "  ↓ 4 more below", "{middle:#?}");
    select(&mut app, Theme::ALL.len() - 1); // ansi: the window pinned to the end
    let end = texts_at(&app, NARROW, SHORT);
    assert_eq!(end[3], "  ↑ 8 more above", "{end:#?}");
    assert!(end[4].starts_with("    solarized"), "{end:#?}");
    assert!(end[5].starts_with("    monokai"), "{end:#?}");
    assert!(end[6].starts_with("  ❯ ansi"), "{end:#?}");
    assert_eq!(end[7], "", "nothing below the last row: {end:#?}");
}

#[test]
fn the_compact_page_holds_still_across_selections() {
    for width in [WIDE, NARROW] {
        let mut app = open_app();
        let h0 = theme_view_lines(&app, width, SHORT).len();
        for i in 1..Theme::ALL.len() {
            select(&mut app, i);
            assert_eq!(
                theme_view_lines(&app, width, SHORT).len(),
                h0,
                "selection {i} moved the compact frame at width {width}"
            );
        }
    }
}

#[test]
fn the_whole_page_returns_once_it_fits() {
    // The compact page is only for a terminal too short for the whole one:
    // at the whole page's own height it is the whole page, one row shorter
    // the compact one.
    let app = open_app();
    let has_preview = |texts: &[String]| texts.iter().any(|t| t.contains("Edit(greet.py)"));
    for width in [WIDE, NARROW] {
        let whole = u16::try_from(theme_view_lines(&app, width, TALL).len()).expect("rows");
        assert!(
            has_preview(&texts_at(&app, width, whole)),
            "width {width}: the whole page at its own height"
        );
        assert!(
            !has_preview(&texts_at(&app, width, whole - 1)),
            "width {width}: compact one row short of it"
        );
    }
}

#[test]
fn a_compact_page_fits_and_never_overflows_the_width_or_stacks_blanks() {
    let mut app = open_app();
    for width in [24u16, 40, NARROW, WIDE] {
        for query in ["", "ca", "nord"] {
            let len = {
                let picker = app.theme_picker.as_mut().expect("open");
                picker.query = query.to_string();
                app.theme_rows().len()
            };
            for i in 0..len {
                select(&mut app, i);
                let texts = texts_at(&app, width, SHORT);
                assert!(
                    texts.len() <= usize::from(SHORT),
                    "width {width} query {query:?} selection {i} overflows the height: {texts:#?}"
                );
                for text in &texts {
                    assert!(
                        cols(text) <= usize::from(width),
                        "width {width} query {query:?} selection {i}: {text:?} overflows"
                    );
                }
                for pair in texts.windows(2) {
                    assert!(
                        !(pair[0].is_empty() && pair[1].is_empty()),
                        "width {width} query {query:?} selection {i} stacked two blank rows: {texts:#?}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_cursor_sits_on_the_compact_pages_search_line() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = open_app();
    for c in "ca".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let height = theme_picker_height(&app, NARROW, SHORT).expect("open");
    assert!(height <= SHORT, "the compact page fits the terminal");
    let area = Rect::new(0, SHORT - height, NARROW, height);
    let (x, y) = crate::ui::cursor_position(area, &app);
    assert_eq!(
        y,
        area.y + crate::ui::theme::THEME_SEARCH_ROW,
        "on the search line"
    );
    // indent (2) + `❯ ` (2) + the two-character query.
    assert_eq!(x, 2 + 2 + 2);
}

#[test]
fn the_picker_flows_like_its_family_and_a_keystroke_resigns_the_flow() {
    // Even the compact page is taller than a 6-row terminal: its skipped top
    // — down into the list — flows into scrollback like every framed view's.
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = open_app();
    let page = theme_view_lines(&app, 80, 6).len();
    let flow = crate::ui::view_flow(&app, 80, 6, 1000).expect("flows on a 6-row terminal");
    assert_eq!(flow.lines.len(), page - 6, "the skipped top flows");
    // A still page signs its rows: moving the highlight off a flowed row
    // re-flows it.
    let before = crate::ui::view_flow_signature(&app, 80, 6, 1000).expect("overflows");
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let moved = crate::ui::view_flow_signature(&app, 80, 6, 1000).expect("overflows");
    assert_ne!(before, moved, "moving the highlight re-signs the flow");
}

#[test]
fn a_page_that_fits_flows_nothing() {
    // The compact page exists so a short terminal keeps the page on screen:
    // where it fits, nothing flows into scrollback above it.
    let app = open_app();
    for width in [WIDE, NARROW] {
        assert!(
            crate::ui::view_flow(&app, width, SHORT, 1000).is_none(),
            "width {width}: the compact page fits {SHORT} rows"
        );
        assert!(
            crate::ui::view_flow(&app, width, TALL, 1000).is_none(),
            "width {width}: the whole page fits {TALL} rows"
        );
    }
}

#[test]
fn the_picker_renders_the_page_its_height_reserves() {
    // On a 12-row terminal the picker reserves the compact page's rows, and
    // the paint fills exactly them: the search line, the theme names, the
    // hint and the closing rule all on screen.
    let app = open_app();
    let height = theme_picker_height(&app, 80, 12).expect("open");
    let area = Rect::new(0, 0, 80, height);
    let mut buf = Buffer::empty(area);
    render_theme_picker(area, &mut buf, &app);
    let rows: Vec<String> = (0..height).map(|y| row(&buf, y, 80)).collect();
    let last = rows.last().expect("rows");
    assert!(
        last.trim().chars().all(|c| c == '─') && !last.trim().is_empty(),
        "the closing rule sits on the last row: {rows:#?}"
    );
    assert!(
        rows.iter().any(|r| r.trim() == "❯"),
        "the search line: {rows:#?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("❯ mocha")),
        "the theme names: {rows:#?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("↑/↓ to navigate")),
        "the hint: {rows:#?}"
    );
}

#[test]
fn a_page_taller_than_its_area_is_bottom_anchored() {
    // An area shorter than even the compact page paints its tail — the hint
    // and the closing rule stay on screen, with the rows just above them.
    let app = open_app();
    let area = Rect::new(0, 0, 80, 6);
    let mut buf = Buffer::empty(area);
    render_theme_picker(area, &mut buf, &app);
    let rows: Vec<String> = (0..6).map(|y| row(&buf, y, 80)).collect();
    assert!(
        rows[5].trim().chars().all(|c| c == '─') && !rows[5].trim().is_empty(),
        "the closing rule sits on the last row: {rows:#?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("↑/↓ to navigate")),
        "the hint stays on screen: {rows:#?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("frappe")),
        "…under the last rows of the list: {rows:#?}"
    );
}
