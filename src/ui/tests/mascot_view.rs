//! The inline `/mascot` picker view (`docs/mascot.md`).

use super::*;
use crate::app::{Mascot, Theme};
use crate::ui::mascot_view::{mascot_menu_rows, mascot_view_lines};
use crate::ui::palette::palette_of;
use crate::ui::wrap::cols;

/// A standard terminal: wide enough for the side-by-side page.
const WIDE: u16 = 80;
/// A terminal too narrow for it: the preview stacks under the list (the
/// hint still reads as far as `Enter to choose`).
const NARROW: u16 = 54;
/// A terminal tall enough for the whole page at any of these widths.
const TALL: u16 = 200;

/// An app with session info and the picker open (the ordinary shape).
fn open_app() -> App {
    let mut app = with_session();
    app.open_mascot_picker();
    app
}

/// `app` with the highlight moved to `selected`.
fn select(app: &mut App, selected: usize) {
    app.mascot_picker.as_mut().expect("open").selected = selected;
}

fn texts(app: &App, width: u16) -> Vec<String> {
    page_at(app, width, TALL)
}

/// The page on a terminal `height` rows tall, as trimmed plain rows.
fn page_at(app: &App, width: u16, height: u16) -> Vec<String> {
    mascot_view_lines(app, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

/// Whether `text` is the list row for `name`: past the indent and the `❯`
/// marker it opens with the name (a caption or banner row mentioning the
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

/// The display column `needle` starts at in `text`.
fn col_of(text: &str, needle: &str) -> usize {
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {text:?}"));
    cols(&text[..at])
}

/// Whether the page at `width` lays the preview **beside** the list: some
/// mascot row carries the banner's title beside it.
fn is_beside(app: &App, width: u16) -> bool {
    texts(app, width)
        .iter()
        .any(|t| Mascot::ALL.iter().any(|m| is_row_for(t, m.name())) && t.contains("Alter Zero"))
}

#[test]
fn closed_builds_nothing() {
    assert!(mascot_view_lines(&with_session(), 80, TALL).is_empty());
    assert_eq!(mascot_picker_height(&with_session(), 80, 200), None);
}

#[test]
fn the_page_is_framed_with_search_rows_preview_and_hint() {
    for width in [WIDE, NARROW] {
        let texts = texts(&open_app(), width);
        let is_rule = |t: &String| !t.trim().is_empty() && t.trim().chars().all(|c| c == '─');
        assert!(is_rule(texts.first().expect("a top rule")), "top rule");
        assert!(is_rule(texts.last().expect("a bottom rule")), "bottom rule");
        assert!(
            texts[1].trim_start().starts_with('❯'),
            "the ❯ search line sits straight under the top rule: {texts:#?}"
        );
        for mascot in Mascot::ALL {
            row_for(&texts, mascot.name());
        }
        assert!(
            row_for(&texts, "crest").starts_with("  ❯ crest"),
            "the open seats the marker on crest, the default: {texts:#?}"
        );
        let title = texts
            .iter()
            .position(|t| t.contains("Crest"))
            .expect("the highlighted mascot's name captions the preview");
        assert!(
            texts[title + 1].contains("A crested hatchling flaring its frill"),
            "its description on the row under it: {texts:#?}"
        );
        // The live preview: the highlighted (default crest) banner — its art
        // AND its metadata, built by the same header builder the startup
        // banner uses.
        for expect in [
            "▙▄▙▄▟▄▟",
            "Alter Zero (v",
            "~/alter-zero",
            "/login   /model",
        ] {
            assert!(
                texts.iter().any(|t| t.contains(expect)),
                "the banner preview's {expect:?}: {texts:#?}"
            );
        }
        assert!(
            texts
                .iter()
                .any(|t| t.trim().starts_with("↑/↓ to navigate · Type to search")),
            "the key hint leads with navigation: {texts:#?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("/6)")),
            "no (n/total) counter — the overflow rows say what is hidden: {texts:#?}"
        );
    }
}

#[test]
fn rows_wear_the_mcp_lists_marker_and_indent() {
    // The /mcp list's shape, /theme's: the selected row is the indent, the
    // accent `❯` and the bold name; every other row keeps the marker's width
    // blank, so the names line up under it.
    let mut app = open_app();
    select(&mut app, 1); // bloom
    for width in [WIDE, NARROW] {
        let lines = mascot_view_lines(&app, width, TALL);
        let texts: Vec<String> = lines.iter().map(plain).collect();
        assert!(
            row_for(&texts, "bloom").starts_with("  ❯ bloom"),
            "{texts:#?}"
        );
        for name in ["crest", "sprout", "twin", "skiter", "gem"] {
            assert!(
                row_for(&texts, name).starts_with(&format!("    {name}")),
                "{name} is indented under the marker: {texts:#?}"
            );
        }
        let row = &lines[row_index(&texts, "bloom")];
        let marker = row
            .spans
            .iter()
            .find(|s| s.content == "❯ ")
            .expect("the marker span");
        assert_eq!(
            marker.style.fg,
            Some(palette_of(Theme::Mocha).accent),
            "the marker wears the active accent"
        );
        let name = row
            .spans
            .iter()
            .find(|s| s.content == "bloom")
            .expect("the name span");
        assert!(name.style.add_modifier.contains(Modifier::BOLD), "{name:?}");
    }
}

#[test]
fn a_wide_terminal_lays_the_preview_beside_the_list() {
    // codex's side-by-side picker, /theme's: the list on the left, the
    // highlighted mascot's caption and banner on the right, every preview
    // row starting on one column.
    let texts = texts(&open_app(), WIDE);
    let crest = row_for(&texts, "crest");
    assert!(
        crest.contains("Crest"),
        "the caption's title sits beside the first row: {texts:#?}"
    );
    let column = col_of(crest, "Crest");
    for (row, piece) in [
        ("bloom", "A crested hatchling flaring its frill"),
        ("skiter", "▝▜▄███▄▛▘  ~/alter-zero"),
    ] {
        let text = row_for(&texts, row);
        assert!(text.contains(piece), "{piece:?} beside {row}: {texts:#?}");
        assert_eq!(
            col_of(text, piece),
            column,
            "{piece:?} on the preview column: {texts:#?}"
        );
    }
    assert_eq!(
        row_for(&texts, "sprout"),
        "    sprout",
        "a gap between the caption and the banner: {texts:#?}"
    );
    assert!(
        row_for(&texts, "twin").contains("▙▄▙▄▟▄▟   Alter Zero (v"),
        "the banner's first row beside twin: {texts:#?}"
    );
    assert!(
        row_for(&texts, "gem").contains("/login   /model   /resume"),
        "the banner's hint row beside gem: {texts:#?}"
    );
}

#[test]
fn a_narrow_terminal_stacks_the_preview_under_the_list() {
    let app = open_app();
    assert!(is_beside(&app, WIDE), "a standard terminal");
    assert!(!is_beside(&app, NARROW), "a narrow one stacks");
    // Stacked, the preview hangs under the list after a gap, inset two
    // columns, its caption first.
    let texts = texts(&app, NARROW);
    let caption = texts
        .iter()
        .position(|t| t.trim_start() == "Crest")
        .unwrap_or_else(|| panic!("the caption: {texts:#?}"));
    assert_eq!(texts[caption], "  Crest", "{texts:#?}");
    assert_eq!(
        texts[caption + 1],
        "  A crested hatchling flaring its frill",
        "{texts:#?}"
    );
    assert_eq!(texts[caption - 1], "", "a gap over the caption: {texts:#?}");
    assert!(
        is_row_for(&texts[caption - 2], "gem"),
        "the list just above the gap: {texts:#?}"
    );
    let art = texts
        .iter()
        .position(|t| t.contains("▝▜▄███▄▛▘"))
        .expect("the banner");
    assert!(art > caption, "{texts:#?}");
    assert!(texts[art].starts_with("  ▝▜▄███▄▛▘"), "{texts:#?}");
}

#[test]
fn the_layout_switches_once_and_beside_the_list_the_banner_keeps_its_art() {
    // One breakpoint: every width from it up lays the preview beside the
    // list, every width under it stacks. At the breakpoint the banner still
    // seats its art beside its title, and every caption reads whole, so the
    // side-by-side page is one shape wherever it is laid out.
    let mut app = open_app();
    let switch = (20u16..400)
        .find(|&w| is_beside(&app, w))
        .expect("some width lays the preview beside the list");
    assert!(switch <= WIDE, "a standard terminal gets it: {switch}");
    for width in 20u16..switch {
        assert!(!is_beside(&app, width), "width {width} is under the switch");
    }
    let tall = mascot_view_lines(&app, 400, TALL).len();
    for width in switch..400 {
        assert!(is_beside(&app, width), "width {width} is past the switch");
        assert_eq!(
            mascot_view_lines(&app, width, TALL).len(),
            tall,
            "width {width} changed the side-by-side page's height"
        );
    }
    for (i, mascot) in Mascot::ALL.iter().enumerate() {
        select(&mut app, i);
        let texts = texts(&app, switch);
        assert!(
            texts.iter().any(|t| t.contains(mascot.art()[1])),
            "{mascot:?}'s banner keeps its art at {switch}: {texts:#?}"
        );
        assert!(
            texts.iter().any(|t| t.contains(mascot.description())),
            "{mascot:?}'s caption reads whole at {switch}: {texts:#?}"
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
        let mascot = app.highlighted_mascot().expect("a highlight");
        let description = mascot.description();
        let row = texts
            .iter()
            .find(|t| t.contains(description))
            .unwrap_or_else(|| panic!("the {mascot:?} caption: {texts:#?}"));
        col_of(row, description)
    };
    let home = column_of(&app);
    for i in 0..Mascot::ALL.len() {
        select(&mut app, i);
        assert_eq!(column_of(&app), home, "selection {i} moved the preview");
    }
    for query in ["cr", "gem", "leaf", "a"] {
        let picker = app.mascot_picker.as_mut().expect("open");
        picker.query = query.to_string();
        picker.selected = 0;
        assert_eq!(column_of(&app), home, "query {query:?} moved the preview");
    }
}

#[test]
fn the_caption_is_a_bold_title_over_a_dim_description() {
    // /theme's caption: the highlighted mascot's name as a bold title, and
    // under it its description in the frame's dim.
    let mut app = open_app();
    select(&mut app, 5); // gem
    for width in [WIDE, NARROW] {
        let lines = mascot_view_lines(&app, width, TALL);
        let at = lines
            .iter()
            .position(|l| l.spans.iter().any(|s| s.content == "Gem"))
            .expect("the title row");
        let title = lines[at]
            .spans
            .iter()
            .find(|s| s.content == "Gem")
            .expect("the title span");
        assert!(
            title.style.add_modifier.contains(Modifier::BOLD),
            "{title:?}"
        );
        let blurb = lines[at + 1]
            .spans
            .iter()
            .find(|s| s.content.contains("A cut gem catching the terminal light"))
            .unwrap_or_else(|| panic!("the description span: {:?}", lines[at + 1]));
        assert!(
            !blurb.style.add_modifier.contains(Modifier::BOLD),
            "{blurb:?}"
        );
        assert_eq!(
            blurb.style.fg,
            Some(palette_of(Theme::Mocha).dim),
            "the description is the frame's dim"
        );
    }
}

#[test]
fn a_standard_terminal_shows_every_caption_whole() {
    let mut app = open_app();
    for (i, mascot) in Mascot::ALL.iter().enumerate() {
        select(&mut app, i);
        let texts = texts(&app, WIDE);
        assert!(
            texts.iter().any(|t| t.contains(mascot.description())),
            "{mascot:?}'s caption: {texts:#?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains('…')),
            "nothing on the page is cut: {texts:#?}"
        );
    }
}

#[test]
fn the_preview_follows_the_selection() {
    let mut app = open_app();
    select(&mut app, 1); // bloom
    for width in [WIDE, NARROW] {
        let texts = texts(&app, width);
        assert!(
            texts.iter().any(|t| t.contains("▀█▄███▄█▀")),
            "bloom previews: {texts:#?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("▙▄▙▄▟▄▟")),
            "crest's art left with the selection: {texts:#?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains(Mascot::Bloom.description())),
            "…and its caption: {texts:#?}"
        );
    }
}

#[test]
fn the_page_height_is_stable_across_selections() {
    // The banner slot pads to the tallest mascot, so the frame never jumps
    // as ↑/↓ move (the /settings always-emit rule) — beside the list and
    // stacked under it alike.
    let mut app = open_app();
    for width in [WIDE, NARROW] {
        select(&mut app, 0);
        let h0 = mascot_view_lines(&app, width, TALL).len();
        for i in 1..Mascot::ALL.len() {
            select(&mut app, i);
            assert_eq!(
                mascot_view_lines(&app, width, TALL).len(),
                h0,
                "width {width}: selection {i} changed the page height"
            );
        }
    }
}

#[test]
fn the_active_mascot_wears_the_check_and_the_open_seats_on_it() {
    let mut app = with_session();
    app.set_mascot(Mascot::Gem);
    app.open_mascot_picker();
    for width in [WIDE, NARROW] {
        let texts = texts(&app, width);
        assert!(
            row_for(&texts, "gem").contains('✓'),
            "gem carries the ✓: {texts:#?}"
        );
        assert!(
            !row_for(&texts, "crest").contains('✓'),
            "only the active row is marked: {texts:#?}"
        );
        // …and the open landed on it, so the picker resumes where the session is.
        assert!(
            row_for(&texts, "gem").starts_with("  ❯ gem"),
            "the highlight seats on the active mascot: {texts:#?}"
        );
    }
}

#[test]
fn an_unmatched_search_collapses_to_placeholder_and_hint() {
    // Nothing matched means there is no list, no caption and no banner to
    // preview — so the page collapses to the placeholder between single
    // gaps instead of a band of empty rows (the `/model` picker's
    // placeholder rule): rule, search, blank, placeholder, blank, hint, rule.
    let mut app = open_app();
    app.mascot_picker.as_mut().expect("open").query = "zzz".to_string();
    let texts = texts(&app, 80);
    let is_rule = |t: &str| !t.is_empty() && t.chars().all(|c| c == '─');
    assert_eq!(texts.len(), 7, "the collapsed page is 7 rows: {texts:?}");
    assert!(is_rule(&texts[0]), "{texts:?}");
    assert!(texts[1].contains('❯'), "the search line: {texts:?}");
    assert_eq!(texts[2], "", "{texts:?}");
    assert!(
        texts[3].contains("No matching mascots"),
        "the placeholder: {texts:?}"
    );
    assert_eq!(texts[4], "", "one gap under the placeholder: {texts:?}");
    assert!(texts[5].contains("Type to search"), "the hint: {texts:?}");
    assert!(is_rule(&texts[6]), "{texts:?}");
}

#[test]
fn every_page_hugs_its_frame() {
    // The search line straight under the opening rule, the key hint
    // straight over the closing one, whatever the search shows.
    let mut app = open_app();
    for width in [WIDE, NARROW] {
        for query in ["", "cr", "zzz"] {
            let picker = app.mascot_picker.as_mut().expect("open");
            picker.query = query.to_string();
            picker.selected = 0;
            assert_frame_hugs_content(
                &format!("the picker at {width} searching {query:?}"),
                &mascot_view_lines(&app, width, TALL),
            );
        }
    }
}

#[test]
fn no_page_ever_stacks_two_blank_rows() {
    // The reported wart: a run of empty rows in the middle of the frame. No
    // state of the picker — a real selection, a narrowed search, a search
    // that matched nothing — may stack blanks, beside the list or under it.
    let mut app = open_app();
    for width in [WIDE, NARROW] {
        for query in ["", "cr", "zzz"] {
            let picker = app.mascot_picker.as_mut().expect("open");
            picker.query = query.to_string();
            picker.selected = 0;
            let texts = texts(&app, width);
            for pair in texts.windows(2) {
                assert!(
                    !(pair[0].is_empty() && pair[1].is_empty()),
                    "width {width} query {query:?} stacked two blank rows: {texts:#?}"
                );
            }
        }
    }
}

#[test]
fn the_page_never_exceeds_the_width() {
    let mut app = open_app();
    for width in [24u16, 40, 50, 56, 60, 80, 120] {
        for i in 0..Mascot::ALL.len() {
            select(&mut app, i);
            for line in mascot_view_lines(&app, width, TALL) {
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
    let lines = mascot_view_lines(&app, 80, TALL).len() as u16;
    assert_eq!(mascot_menu_rows(&app, 80, TALL), lines);
    assert_eq!(
        mascot_picker_height(&app, 80, 200),
        Some(lines),
        "an idle app has no strip above the picker"
    );
    assert_eq!(
        mascot_picker_height(&app, 80, 10),
        Some(9),
        "a short terminal gets the page that fits it — the compact one"
    );
}

#[test]
fn the_picker_never_flows_its_page() {
    // A terminal shorter than the compact page gets a smaller rung, down to
    // the search line alone, so nothing flows into scrollback — least of all
    // the search line (docs/view-flow.md).
    let app = open_app();
    for width in [WIDE, NARROW] {
        for height in (1..=20).chain([TALL]) {
            assert!(
                crate::ui::view_flow(&app, width, height, 1000).is_none(),
                "{width}x{height}"
            );
        }
    }
}

// --- the compact page: a short terminal keeps the choices on screen ---

/// How many rows of `texts` are mascot rows.
fn mascot_rows(texts: &[String]) -> usize {
    texts
        .iter()
        .filter(|t| Mascot::ALL.iter().any(|m| is_row_for(t, m.name())))
        .count()
}

fn is_rule(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c == '─')
}

#[test]
fn a_short_terminal_keeps_the_mascot_choices_on_screen() {
    // The whole page is twelve rows beside the list and nineteen stacked
    // under it. A terminal shorter than that gets the compact page: the
    // preview yields, and the search line, three mascots between the
    // overflow slots and the hint stay on screen (bottom-anchoring the whole
    // page kept the preview and flowed the search line and the highlighted
    // mascot into scrollback).
    let app = open_app();
    assert_eq!(page_at(&app, WIDE, TALL).len(), 12, "beside the list");
    assert_eq!(page_at(&app, NARROW, TALL).len(), 19, "stacked");
    for (width, height) in [(WIDE, 11), (NARROW, 12)] {
        let texts = page_at(&app, width, height);
        assert_eq!(texts.len(), 9, "{width}x{height}: {texts:#?}");
        assert!(
            texts.iter().any(|t| t == "  ❯"),
            "the search line: {texts:#?}"
        );
        assert!(
            row_for(&texts, "crest").starts_with("  ❯ crest"),
            "{texts:#?}"
        );
        assert_eq!(mascot_rows(&texts), 3, "three rows: {texts:#?}");
        assert!(texts.iter().any(|t| t == "  ↓ 3 more below"), "{texts:#?}");
        assert!(
            texts.iter().any(|t| t.contains("Enter to choose")),
            "{texts:#?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("Alter Zero")),
            "no banner preview: {texts:#?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("crested hatchling")),
            "no caption: {texts:#?}"
        );
    }
}

#[test]
fn the_compact_mascot_page_is_the_familys_short_page() {
    let texts = page_at(&open_app(), WIDE, 11);
    assert_eq!(texts.len(), 9, "{texts:#?}");
    assert!(is_rule(&texts[0]), "{texts:#?}");
    assert_eq!(texts[1], "  ❯", "{texts:#?}");
    assert_eq!(texts[2], "", "{texts:#?}");
    assert_eq!(texts[3], "  ❯ crest ✓", "{texts:#?}");
    assert_eq!(texts[4], "    bloom", "{texts:#?}");
    assert_eq!(texts[5], "    sprout", "{texts:#?}");
    assert_eq!(texts[6], "  ↓ 3 more below", "{texts:#?}");
    assert!(texts[7].contains("Enter to choose"), "{texts:#?}");
    assert!(is_rule(&texts[8]), "{texts:#?}");
}

#[test]
fn the_compact_mascot_window_rides_the_selection_and_holds_still() {
    let mut app = open_app();
    select(&mut app, 3); // twin
    let texts = page_at(&app, WIDE, 11);
    assert_eq!(texts[2], "  ↑ 2 more above", "{texts:#?}");
    assert_eq!(texts[3], "    sprout", "{texts:#?}");
    assert_eq!(texts[4], "  ❯ twin", "{texts:#?}");
    assert_eq!(texts[5], "    skiter", "{texts:#?}");
    assert_eq!(texts[6], "  ↓ 1 more below", "{texts:#?}");
    for i in 0..Mascot::ALL.len() {
        select(&mut app, i);
        assert_eq!(
            page_at(&app, WIDE, 11).len(),
            9,
            "selection {i} moved the frame"
        );
    }
}

#[test]
fn the_whole_mascot_page_returns_once_it_fits() {
    let app = open_app();
    let has_preview = |texts: &[String]| texts.iter().any(|t| t.contains("Alter Zero"));
    for width in [WIDE, NARROW] {
        let whole = u16::try_from(page_at(&app, width, TALL).len()).expect("rows");
        assert!(
            has_preview(&page_at(&app, width, whole)),
            "{width}: whole at its own height"
        );
        assert!(
            !has_preview(&page_at(&app, width, whole - 1)),
            "{width}: compact a row short"
        );
    }
}

#[test]
fn the_cursor_sits_on_the_compact_mascot_search_line() {
    let app = open_app();
    let height = mascot_picker_height(&app, WIDE, 11).expect("open");
    assert_eq!(height, 9, "the compact page");
    let area = Rect::new(0, 1, WIDE, height);
    let (_, y) = crate::ui::cursor_position(area, &app);
    assert_eq!(y, 1 + SEARCH_ROW, "on the search line");
    assert!(
        crate::ui::view_flow(&app, WIDE, 11, 1000).is_none(),
        "nothing flows"
    );
}

// --- a terminal shorter than the compact page keeps the search line ---

#[test]
fn a_tiny_terminal_keeps_the_mascot_search_line() {
    let app = open_app();
    for width in [WIDE, NARROW] {
        assert_sheds_to_its_search_line(
            |height| page_at(&app, width, height),
            "",
            "❯ crest",
            "Enter to choose",
        );
        for height in 1..=10 {
            assert_caret_on_search_line(&app, &page_at(&app, width, height), width, "");
        }
    }
    let mut app = open_app();
    app.mascot_picker.as_mut().expect("open").query = "zzz".to_string();
    assert_sheds_to_its_search_line(
        |height| page_at(&app, 80, height),
        "zzz",
        "No matching mascots",
        "Enter to choose",
    );
}
