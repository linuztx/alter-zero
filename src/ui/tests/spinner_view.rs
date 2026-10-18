//! The inline `/spinner` picker view (`docs/spinner.md`).

use super::*;
use crate::app::{Spinner, Theme, VERB_ROTATION};
use crate::ui::palette::palette_of;
use crate::ui::spinner_view::{spinner_menu_rows, spinner_view_lines};
use crate::ui::wrap::cols;

/// A standard terminal: wide enough for the side-by-side page.
const WIDE: u16 = 80;
/// A terminal too narrow for it: the preview stacks under the list.
const NARROW: u16 = 60;
/// A terminal tall enough for the whole page at any of these widths.
const TALL: u16 = 200;

/// An app with session info and the picker open (the ordinary shape).
fn open_app() -> App {
    let mut app = with_session();
    app.open_spinner_picker();
    app
}

fn texts(app: &App, width: u16) -> Vec<String> {
    page_at(app, width, TALL)
}

/// The page on a terminal `height` rows tall, as trimmed plain rows.
fn page_at(app: &App, width: u16, height: u16) -> Vec<String> {
    spinner_view_lines(app, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

fn select(app: &mut App, i: usize) {
    app.spinner_picker.as_mut().expect("open").selected = i;
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

/// The display column `needle` starts at in `text`.
fn col_of(text: &str, needle: &str) -> usize {
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {text:?}"));
    cols(&text[..at])
}

/// The row carrying the sample status line.
fn preview(texts: &[String]) -> &String {
    texts
        .iter()
        .find(|t| t.contains("Working…"))
        .unwrap_or_else(|| panic!("no preview line: {texts:?}"))
}

/// Whether the page at `width` lays the preview **beside** the list: the
/// sample status line shares a row with a style's row.
fn is_beside(app: &App, width: u16) -> bool {
    texts(app, width)
        .iter()
        .any(|t| Spinner::ALL.iter().any(|s| is_row_for(t, s.name())) && t.contains("Working…"))
}

/// The display column the row's glyph starts at, for the list row naming
/// `name`.
fn glyph_col(texts: &[String], name: &str) -> usize {
    let row = row_for(texts, name);
    let after = row.find(name).expect("the name") + name.len();
    let glyph = row[after..].trim_start();
    cols(row) - cols(glyph)
}

#[test]
fn closed_builds_nothing() {
    assert!(spinner_view_lines(&with_session(), 80, TALL).is_empty());
    assert_eq!(spinner_picker_height(&with_session(), 80, 200), None);
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
        for spinner in Spinner::ALL {
            row_for(&texts, spinner.name());
        }
        assert!(
            row_for(&texts, "gravity").starts_with("  ❯ gravity"),
            "the open seats the marker on gravity, the default: {texts:#?}"
        );
        let title = texts
            .iter()
            .position(|t| t.contains("Gravity"))
            .expect("the highlighted style's name captions the preview");
        assert!(
            texts[title + 1].contains(Spinner::Gravity.description()),
            "its description on the row under it: {texts:#?}"
        );
        // The live preview: the highlighted (default gravity) style as a
        // whole status line, built by the status line's own renderer.
        assert!(
            preview(&texts).contains("⣤⣀⣀⣀⣀⣀⣀⣀ Working… (0s · esc to interrupt)"),
            "the sample status line wears the gravity track: {texts:#?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.trim().starts_with("↑/↓ to navigate · Type to search")),
            "the key hint leads with navigation: {texts:#?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("/9)")),
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
    select(&mut app, 4); // dots
    for width in [WIDE, NARROW] {
        let lines = spinner_view_lines(&app, width, TALL);
        let texts: Vec<String> = lines.iter().map(plain).collect();
        assert!(
            row_for(&texts, "dots").starts_with("  ❯ dots"),
            "{texts:#?}"
        );
        for name in ["comet", "gravity", "wave", "line"] {
            assert!(
                row_for(&texts, name).starts_with(&format!("    {name}")),
                "{name} is indented under the marker: {texts:#?}"
            );
        }
        let row = &lines[row_index(&texts, "dots")];
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
            .find(|s| s.content == "dots")
            .expect("the name span");
        assert!(name.style.add_modifier.contains(Modifier::BOLD), "{name:?}");
    }
}

#[test]
fn every_row_carries_its_own_live_glyph() {
    // The list is a live catalog: each row shows its style's spinner at the
    // shared frame clock, so the styles compare at a glance.
    for width in [WIDE, NARROW] {
        let texts = texts(&open_app(), width);
        assert!(row_for(&texts, "comet").contains("(●•·   )"), "{texts:?}");
        // The two braille tracks at the open: the ball on the floor at the
        // left wall, and the wave's eight two-dot cells.
        assert!(row_for(&texts, "gravity").contains("⣤⣀⣀⣀⣀⣀⣀⣀"), "{texts:?}");
        let is_braille = |c: char| ('\u{2800}'..='\u{28FF}').contains(&c);
        assert_eq!(
            row_for(&texts, "wave")
                .chars()
                .filter(|&c| is_braille(c))
                .count(),
            8,
            "the wave is eight braille cells: {texts:?}"
        );
        assert!(row_for(&texts, "dots").contains('⠋'), "{texts:?}");
        assert!(row_for(&texts, "blocks").contains('▙'), "{texts:?}");
        assert!(row_for(&texts, "pulse").contains('●'), "{texts:?}");
        assert!(row_for(&texts, "bars").contains('▁'), "{texts:?}");
        assert!(row_for(&texts, "line").contains('|'), "{texts:?}");
    }
}

#[test]
fn a_wide_terminal_lays_the_preview_beside_the_list() {
    // codex's side-by-side picker, /theme's: the list on the left, the
    // highlighted style's caption and sample status line on the right,
    // every preview row starting on one column.
    let texts = texts(&open_app(), WIDE);
    let comet = row_for(&texts, "comet");
    assert!(
        comet.contains("Gravity"),
        "the caption's title sits beside the first row: {texts:#?}"
    );
    let column = col_of(comet, "Gravity");
    for (row, piece) in [
        ("gravity", Spinner::Gravity.description()),
        ("sparkle", "⣤⣀⣀⣀⣀⣀⣀⣀ Working… (0s · esc to interrupt)"),
    ] {
        let text = row_for(&texts, row);
        assert!(text.contains(piece), "{piece:?} beside {row}: {texts:#?}");
        assert_eq!(
            col_of(text, piece),
            column,
            "{piece:?} on the preview column: {texts:#?}"
        );
    }
    let wave = row_for(&texts, "wave");
    assert_eq!(
        cols(wave),
        glyph_col(&texts, "wave") + 8,
        "a gap between the caption and the sample: {texts:#?}"
    );
}

#[test]
fn a_narrow_terminal_stacks_the_preview_under_the_list() {
    let app = open_app();
    assert!(is_beside(&app, WIDE), "a standard terminal");
    assert!(!is_beside(&app, NARROW), "a narrow one stacks");
    // Stacked, the preview hangs under the list after a gap, inset two
    // columns: the caption, a gap, the sample status line.
    let texts = texts(&app, NARROW);
    let caption = texts
        .iter()
        .position(|t| t.trim_start() == "Gravity")
        .unwrap_or_else(|| panic!("the caption: {texts:#?}"));
    assert_eq!(texts[caption], "  Gravity", "{texts:#?}");
    assert_eq!(
        texts[caption + 1],
        format!("  {}", Spinner::Gravity.description()),
        "{texts:#?}"
    );
    assert_eq!(texts[caption - 1], "", "a gap over the caption: {texts:#?}");
    assert!(
        is_row_for(&texts[caption - 2], "line"),
        "the list just above the gap: {texts:#?}"
    );
    assert_eq!(texts[caption + 2], "", "a gap over the sample: {texts:#?}");
    assert_eq!(
        texts[caption + 3],
        "  ⣤⣀⣀⣀⣀⣀⣀⣀ Working… (0s · esc to interrupt)",
        "{texts:#?}"
    );
}

#[test]
fn the_layout_switches_once_and_beside_the_list_the_sample_reads_whole() {
    // One breakpoint: every width from it up lays the preview beside the
    // list, every width under it stacks. At the breakpoint the sample status
    // line still reads whole, so the side-by-side page is one shape wherever
    // it is laid out.
    let app = open_app();
    let switch = (20u16..400)
        .find(|&w| is_beside(&app, w))
        .expect("some width lays the preview beside the list");
    assert!(switch <= WIDE, "a standard terminal gets it: {switch}");
    for width in 20u16..switch {
        assert!(!is_beside(&app, width), "width {width} is under the switch");
    }
    let tall = spinner_view_lines(&app, 400, TALL).len();
    for width in switch..400 {
        assert!(is_beside(&app, width), "width {width} is past the switch");
        assert_eq!(
            spinner_view_lines(&app, width, TALL).len(),
            tall,
            "width {width} changed the side-by-side page's height"
        );
    }
    assert!(
        preview(&texts(&app, switch)).contains("Working… (0s · esc to interrupt)"),
        "the sample reads whole at {switch}"
    );
}

#[test]
fn the_preview_column_holds_still_as_the_list_moves_and_filters() {
    // The list column is sized off the whole catalog, so neither ↑/↓ nor a
    // search moves the preview sideways.
    let mut app = open_app();
    let column_of = |app: &App| {
        let texts = texts(app, WIDE);
        let spinner = app.highlighted_spinner().expect("a highlight");
        let description = spinner.description();
        let row = texts
            .iter()
            .find(|t| t.contains(description))
            .unwrap_or_else(|| panic!("the {spinner:?} caption: {texts:#?}"));
        col_of(row, description)
    };
    let home = column_of(&app);
    for i in 0..Spinner::ALL.len() {
        select(&mut app, i);
        assert_eq!(column_of(&app), home, "selection {i} moved the preview");
    }
    for query in ["co", "braille", "track", "a"] {
        let picker = app.spinner_picker.as_mut().expect("open");
        picker.query = query.to_string();
        picker.selected = 0;
        assert_eq!(column_of(&app), home, "query {query:?} moved the preview");
    }
}

#[test]
fn the_glyph_column_holds_still_as_a_search_narrows_the_list() {
    // The name column is sized off the whole catalog too, so the live
    // spinners stay on one column whatever the search leaves listed.
    let mut app = open_app();
    let home = glyph_col(&texts(&app, WIDE), "dots");
    // (Never the bare name: the search line `❯ dots` would read as its row.)
    for query in ["dot", "braille", "classic"] {
        let picker = app.spinner_picker.as_mut().expect("open");
        picker.query = query.to_string();
        picker.selected = 0;
        assert_eq!(
            glyph_col(&texts(&app, WIDE), "dots"),
            home,
            "query {query:?} moved the glyphs"
        );
    }
}

#[test]
fn the_caption_is_a_bold_title_over_a_dim_description() {
    // /theme's caption: the highlighted style's name as a bold title, and
    // under it its description in the frame's dim.
    let mut app = open_app();
    select(&mut app, 6); // pulse
    for width in [WIDE, NARROW] {
        let lines = spinner_view_lines(&app, width, TALL);
        let at = lines
            .iter()
            .position(|l| l.spans.iter().any(|s| s.content == "Pulse"))
            .expect("the title row");
        let title = lines[at]
            .spans
            .iter()
            .find(|s| s.content == "Pulse")
            .expect("the title span");
        assert!(
            title.style.add_modifier.contains(Modifier::BOLD),
            "{title:?}"
        );
        let blurb = lines[at + 1]
            .spans
            .iter()
            .find(|s| s.content.contains(Spinner::Pulse.description()))
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
    // Beside the list the preview column of an 80-column terminal is the
    // narrowest it gets on a standard terminal; every style's description
    // must still read whole there — the default's above all.
    let mut app = open_app();
    for (i, spinner) in Spinner::ALL.iter().enumerate() {
        select(&mut app, i);
        let texts = texts(&app, WIDE);
        assert!(
            texts.iter().any(|t| t.contains(spinner.description())),
            "{spinner:?}'s caption: {texts:#?}"
        );
        assert!(
            !texts
                .iter()
                .any(|t| t.contains('…') && !t.contains("Working…")),
            "nothing on the page is cut: {texts:#?}"
        );
    }
}

#[test]
fn the_preview_walks_the_verbs_like_a_first_turn() {
    // The sample is a first turn's line, so it moves on to the next verb on
    // the clock a real turn does (docs/status-indicator.md) — browse for
    // half a minute and it shows what a long turn would.
    let mut app = open_app();
    app.set_pulse(VERB_ROTATION + Duration::from_millis(120));
    let texts = texts(&app, 80);
    assert!(
        texts.iter().any(|t| t.contains("Generating… (30s ·")),
        "the preview moved on to the second verb: {texts:?}"
    );
}

#[test]
fn the_rows_and_the_preview_tick_with_the_frame_clock() {
    let mut app = open_app();
    app.set_pulse(Duration::from_millis(3_120)); // 3.12 s after the open
    for width in [WIDE, NARROW] {
        let texts = texts(&app, width);
        assert!(
            row_for(&texts, "sparkle").contains('✻'),
            "sparkle 26 frames on (3120 / 120 = 26 ≡ 6 mod 10): {texts:?}"
        );
        assert!(
            row_for(&texts, "comet").contains("( ●•·  )"),
            "the comet 39 frames on (3120 / 80 = 39 ≡ 9 mod 10): {texts:?}"
        );
        let preview = preview(&texts);
        assert!(
            preview.contains("Working… (3s ·"),
            "the preview's seconds count from the open: {preview:?}"
        );
        // The preview wears the highlighted (default gravity) style on the
        // same frame its own row does — read off that row rather than
        // spelled out, so the pairing is what is pinned and not the track's
        // 3.12 s frame.
        let track: String = row_for(&texts, "gravity")
            .chars()
            .filter(|&c| ('\u{2800}'..='\u{28FF}').contains(&c))
            .collect();
        assert_eq!(
            track.chars().count(),
            8,
            "the gravity row's track: {texts:?}"
        );
        assert!(
            preview.contains(&format!("{track} Working…")),
            "the preview's ball is on the same frame as its row ({track:?}): {preview:?}"
        );
    }
}

#[test]
fn the_preview_follows_the_selection() {
    let mut app = open_app();
    select(&mut app, 4); // dots
    for width in [WIDE, NARROW] {
        let texts = texts(&app, width);
        let preview = preview(&texts);
        assert!(
            preview.contains("⠋ Working…"),
            "the dots style previews: {preview:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains(Spinner::Dots.description())),
            "…with its description: {texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|t| t.contains(Spinner::Gravity.description())),
            "the default's description left with the selection: {texts:?}"
        );
    }
}

#[test]
fn the_page_height_is_stable_across_selections_and_ticks() {
    // Styles differ in width (the comet is eight cells, the rest one) but
    // never in height: moving the selection or advancing the clock must not
    // move the frame (the /settings always-emit rule), beside the list or
    // under it.
    let mut app = open_app();
    for width in [WIDE, NARROW] {
        select(&mut app, 0);
        let h0 = spinner_view_lines(&app, width, TALL).len();
        for i in 1..Spinner::ALL.len() {
            select(&mut app, i);
            assert_eq!(
                spinner_view_lines(&app, width, TALL).len(),
                h0,
                "width {width}: selection {i} changed the page height"
            );
        }
        for ms in [40u64, 333, 999, 12_345] {
            app.set_pulse(Duration::from_millis(ms));
            assert_eq!(
                spinner_view_lines(&app, width, TALL).len(),
                h0,
                "width {width}: the clock at {ms} ms changed the page height"
            );
        }
    }
}

#[test]
fn the_active_style_wears_the_check_and_the_open_seats_on_it() {
    let mut app = with_session();
    app.set_spinner(Spinner::Blocks);
    app.open_spinner_picker();
    for width in [WIDE, NARROW] {
        let texts = texts(&app, width);
        assert!(
            row_for(&texts, "blocks").contains('✓'),
            "blocks carries the ✓: {texts:?}"
        );
        assert!(
            !row_for(&texts, "comet").contains('✓'),
            "only the active row is marked: {texts:?}"
        );
        assert!(
            row_for(&texts, "blocks").starts_with("  ❯ blocks"),
            "the highlight seats on the active style: {texts:?}"
        );
    }
}

#[test]
fn an_unmatched_search_collapses_to_placeholder_and_hint() {
    // Nothing matched means there is no list, no caption and no preview —
    // so the page collapses to the placeholder between single gaps instead
    // of a band of empty rows (the /mascot picker's rule, from /model's).
    let mut app = open_app();
    app.spinner_picker.as_mut().expect("open").query = "zzz".to_string();
    let texts = texts(&app, 80);
    let is_rule = |t: &str| !t.is_empty() && t.chars().all(|c| c == '─');
    assert_eq!(texts.len(), 7, "the collapsed page is 7 rows: {texts:?}");
    assert!(is_rule(&texts[0]), "{texts:?}");
    assert!(texts[1].contains('❯'), "the search line: {texts:?}");
    assert_eq!(texts[2], "", "{texts:?}");
    assert!(
        texts[3].contains("No matching spinners"),
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
        for query in ["", "co", "zzz"] {
            let picker = app.spinner_picker.as_mut().expect("open");
            picker.query = query.to_string();
            picker.selected = 0;
            assert_frame_hugs_content(
                &format!("the picker at {width} searching {query:?}"),
                &spinner_view_lines(&app, width, TALL),
            );
        }
    }
}

#[test]
fn no_page_ever_stacks_two_blank_rows() {
    let mut app = open_app();
    for width in [WIDE, NARROW] {
        for query in ["", "co", "zzz"] {
            let picker = app.spinner_picker.as_mut().expect("open");
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
    for width in [24u16, 40, 60, 71, 72, 80, 120] {
        for i in 0..Spinner::ALL.len() {
            select(&mut app, i);
            for line in spinner_view_lines(&app, width, TALL) {
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
    let lines = spinner_view_lines(&app, 80, TALL).len() as u16;
    assert_eq!(spinner_menu_rows(&app, 80, TALL), lines);
    assert_eq!(
        spinner_picker_height(&app, 80, 200),
        Some(lines),
        "an idle app has no strip above the picker"
    );
    assert_eq!(
        spinner_picker_height(&app, 80, 10),
        Some(9),
        "a short terminal gets the page that fits it — the compact one"
    );
}

#[test]
fn the_picker_never_flows_its_page() {
    let app = open_app();
    for width in [WIDE, NARROW] {
        for height in (1..=21).chain([TALL]) {
            assert!(
                crate::ui::view_flow(&app, width, height, 1000).is_none(),
                "{width}x{height}: a rung fits"
            );
        }
    }
}

#[test]
fn a_tick_never_churns_the_page() {
    // The page animates at the 32 ms cadence. A page that flowed would have
    // to sign its selection rather than its rows so a frame never
    // purge-rebuilt the screen (it still does — `FlowSign::Frozen`); the
    // rungs make the question moot, since the page fits at every height and
    // holds its height through a tick.
    let mut app = open_app();
    for height in (1..=21).chain([TALL]) {
        let before = page_at(&app, 80, height).len();
        app.set_pulse(Duration::from_millis(480));
        assert_eq!(page_at(&app, 80, height).len(), before, "{height} rows");
        assert!(
            crate::ui::view_flow(&app, 80, height, 1000).is_none(),
            "{height} rows"
        );
    }
}

// --- the compact page: a short terminal keeps the choices on screen ---

fn is_rule(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c == '─')
}

#[test]
fn a_short_terminal_keeps_the_spinner_choices_on_screen() {
    // The whole page is fifteen rows beside the list and twenty stacked
    // under it; on a terminal shorter than that the preview yields, and the
    // search line, three styles between the overflow slots and the hint stay
    // on screen (bottom-anchoring the whole page kept the preview while the
    // search line and the highlighted style flowed away).
    let app = open_app();
    assert_eq!(page_at(&app, WIDE, TALL).len(), 15, "beside the list");
    assert_eq!(page_at(&app, NARROW, TALL).len(), 20, "stacked");
    for width in [WIDE, NARROW] {
        let texts = page_at(&app, width, 12);
        assert_eq!(texts.len(), 9, "the compact page: {texts:#?}");
        assert!(is_rule(&texts[0]), "{texts:#?}");
        assert_eq!(texts[1], "  ❯", "the search line: {texts:#?}");
        assert_eq!(texts[2], "", "nothing above comet: {texts:#?}");
        assert!(texts[3].starts_with("    comet"), "{texts:#?}");
        assert!(
            texts[4].starts_with("  ❯ gravity"),
            "the open seats on it: {texts:#?}"
        );
        assert!(texts[5].starts_with("    wave"), "{texts:#?}");
        assert_eq!(texts[6], "  ↓ 6 more below", "{texts:#?}");
        assert!(texts[7].contains("Enter to choose"), "{texts:#?}");
        assert!(is_rule(&texts[8]), "{texts:#?}");
        assert!(
            !texts.iter().any(|t| t.contains("Working")),
            "no preview: {texts:#?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("Gravity")),
            "no caption: {texts:#?}"
        );
    }
}

#[test]
fn the_compact_spinner_window_rides_the_selection_and_holds_still() {
    // The glyph column is sized off the whole catalog, so it holds still as
    // the three rows in view change.
    let mut app = open_app();
    let col = glyph_col(&page_at(&app, 80, 12), "gravity");
    for i in 0..Spinner::ALL.len() {
        select(&mut app, i);
        let texts = page_at(&app, 80, 12);
        assert_eq!(texts.len(), 9, "selection {i} moved the frame");
        let name = Spinner::ALL[i].name();
        assert_eq!(
            glyph_col(&texts, name),
            col,
            "selection {i} moved the glyphs: {texts:#?}"
        );
    }
    select(&mut app, 5); // blocks
    let texts = page_at(&app, 80, 12);
    assert_eq!(texts[2], "  ↑ 4 more above", "{texts:#?}");
    assert!(texts[4].starts_with("  ❯ blocks"), "{texts:#?}");
    assert_eq!(texts[6], "  ↓ 2 more below", "{texts:#?}");
}

#[test]
fn the_whole_spinner_page_returns_once_it_fits() {
    let app = open_app();
    let has_preview = |texts: &[String]| texts.iter().any(|t| t.contains("Working"));
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
fn the_cursor_sits_on_the_compact_spinner_search_line() {
    let app = open_app();
    let height = spinner_picker_height(&app, 80, 12).expect("open");
    assert_eq!(height, 9, "the compact page");
    let area = Rect::new(0, 1, 80, height);
    let (_, y) = crate::ui::cursor_position(area, &app);
    assert_eq!(y, 1 + SEARCH_ROW, "on the search line");
    assert!(
        crate::ui::view_flow(&app, 80, 12, 1000).is_none(),
        "nothing flows"
    );
}

// --- a terminal shorter than the compact page keeps the search line ---

#[test]
fn a_tiny_terminal_keeps_the_spinner_search_line() {
    let app = open_app();
    for width in [WIDE, NARROW] {
        assert_sheds_to_its_search_line(
            |height| page_at(&app, width, height),
            "",
            "❯ gravity",
            "Enter to choose",
        );
        for height in 1..=10 {
            assert_caret_on_search_line(&app, &page_at(&app, width, height), width, "");
        }
    }
    let mut app = open_app();
    app.spinner_picker.as_mut().expect("open").query = "zzz".to_string();
    assert_sheds_to_its_search_line(
        |height| page_at(&app, 80, height),
        "zzz",
        "No matching spinners",
        "Enter to choose",
    );
}
