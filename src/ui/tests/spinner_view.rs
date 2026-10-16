//! The inline `/spinner` picker view (`docs/spinner.md`).

use super::*;
use crate::app::{Spinner, VERB_ROTATION};
use crate::ui::spinner_view::{spinner_menu_rows, spinner_view_lines};

/// An app with session info and the picker open (the ordinary shape).
fn open_app() -> App {
    let mut app = with_session();
    app.open_spinner_picker();
    app
}

/// A terminal tall enough for the whole page.
const TALL: u16 = 200;

fn texts(app: &App, width: u16) -> Vec<String> {
    spinner_view_lines(app, width, TALL)
        .iter()
        .map(plain)
        .collect()
}

/// The page's row for `name` — the list row that opens with the marker or
/// its two-space indent, then the name (a description row mentioning the
/// same word doesn't count).
fn row_for<'a>(texts: &'a [String], name: &str) -> &'a String {
    texts
        .iter()
        .find(|t| {
            let t = t.trim_start_matches("→ ").trim_start();
            t.starts_with(name) && t[name.len()..].chars().next().is_none_or(|c| c == ' ')
        })
        .unwrap_or_else(|| panic!("no row for {name}: {texts:?}"))
}

/// The preview line — the one row carrying the sample status line.
fn preview(texts: &[String]) -> &String {
    texts
        .iter()
        .find(|t| t.contains("Working…"))
        .unwrap_or_else(|| panic!("no preview line: {texts:?}"))
}

#[test]
fn closed_builds_nothing() {
    assert!(spinner_view_lines(&with_session(), 80, TALL).is_empty());
    assert_eq!(spinner_picker_height(&with_session(), 80, 200), None);
}

#[test]
fn the_page_is_framed_with_search_rows_counter_preview_and_hint() {
    let texts = texts(&open_app(), 80);
    let is_rule = |t: &String| !t.trim().is_empty() && t.trim().chars().all(|c| c == '─');
    assert!(is_rule(texts.first().expect("a top rule")), "top rule");
    assert!(is_rule(texts.last().expect("a bottom rule")), "bottom rule");
    assert!(
        texts.iter().any(|t| t.trim_start().starts_with('❯')),
        "the ❯ search line: {texts:?}"
    );
    for spinner in Spinner::ALL {
        row_for(&texts, spinner.name());
    }
    assert!(
        texts.iter().any(|t| t.contains("(2/9)")),
        "the counter — the open seats on gravity, the default: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("Enter to choose")),
        "the key hint: {texts:?}"
    );
    // The live preview: the highlighted (default gravity) style as a whole
    // status line, built by the status line's own renderer.
    let preview = preview(&texts);
    assert!(
        preview.contains("⣤⣀⣀⣀⣀⣀⣀⣀ Working… (0s · esc to interrupt)"),
        "the sample status line wears the gravity track: {preview:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains(Spinner::Gravity.description())),
        "the highlighted style's description: {texts:?}"
    );
}

#[test]
fn every_row_carries_its_own_live_glyph() {
    // The list is a live catalog: each row shows its style's spinner at the
    // shared frame clock, so the styles compare at a glance.
    let texts = texts(&open_app(), 80);
    assert!(row_for(&texts, "comet").contains("(●•·   )"), "{texts:?}");
    // The two braille tracks at the open: the ball on the floor at the left
    // wall, and the wave's eight two-dot cells.
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
    let texts = texts(&app, 80);
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
    // The preview wears the highlighted (default gravity) style on the same
    // frame its own row does — read off that row rather than spelled out, so
    // the pairing is what is pinned and not the track's 3.12 s frame.
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
        preview.contains(&track),
        "the preview's ball is on the same frame as its row ({track:?}): {preview:?}"
    );
}

#[test]
fn the_preview_follows_the_selection() {
    let mut app = open_app();
    app.spinner_picker.as_mut().expect("open").selected = 4; // dots
    let texts = texts(&app, 80);
    let preview = preview(&texts);
    assert!(
        preview.trim_start().starts_with("⠋ Working…"),
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

#[test]
fn the_page_height_is_stable_across_selections_and_ticks() {
    // Styles differ in width (the comet is eight cells, the rest one) but
    // never in height: moving the selection or advancing the clock must not
    // move the frame (the /settings always-emit rule).
    let mut app = open_app();
    let h0 = spinner_view_lines(&app, 80, TALL).len();
    for i in 1..Spinner::ALL.len() {
        app.spinner_picker.as_mut().expect("open").selected = i;
        assert_eq!(
            spinner_view_lines(&app, 80, TALL).len(),
            h0,
            "selection {i} changed the page height"
        );
    }
    for ms in [40u64, 333, 999, 12_345] {
        app.set_pulse(Duration::from_millis(ms));
        assert_eq!(
            spinner_view_lines(&app, 80, TALL).len(),
            h0,
            "the clock at {ms} ms changed the page height"
        );
    }
}

#[test]
fn the_active_style_wears_the_check_and_the_open_seats_on_it() {
    let mut app = with_session();
    app.set_spinner(Spinner::Blocks);
    app.open_spinner_picker();
    let texts = texts(&app, 80);
    assert!(
        row_for(&texts, "blocks").contains('✓'),
        "blocks carries the ✓: {texts:?}"
    );
    assert!(
        !row_for(&texts, "comet").contains('✓'),
        "only the active row is marked: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("(6/9)")),
        "the highlight seats on the active style: {texts:?}"
    );
}

#[test]
fn an_unmatched_search_collapses_to_placeholder_and_hint() {
    // Nothing matched means there is no count, no preview and no
    // description — so those slots collapse to ONE blank gap instead of a
    // band of empty rows (the /mascot picker's rule, from /model's).
    let mut app = open_app();
    app.spinner_picker.as_mut().expect("open").query = "zzz".to_string();
    let texts: Vec<String> = texts(&app, 80)
        .iter()
        .map(|t| t.trim_end().to_string())
        .collect();
    let is_rule = |t: &str| !t.is_empty() && t.chars().all(|c| c == '─');
    assert_eq!(texts.len(), 9, "the collapsed page is 9 rows: {texts:?}");
    assert!(is_rule(&texts[0]), "{texts:?}");
    assert_eq!(texts[1], "", "{texts:?}");
    assert!(texts[2].contains('❯'), "the search line: {texts:?}");
    assert_eq!(texts[3], "", "{texts:?}");
    assert!(
        texts[4].contains("No matching spinners"),
        "the placeholder: {texts:?}"
    );
    assert_eq!(texts[5], "", "one gap under the placeholder: {texts:?}");
    assert!(texts[6].contains("Type to search"), "the hint: {texts:?}");
    assert_eq!(texts[7], "", "{texts:?}");
    assert!(is_rule(&texts[8]), "{texts:?}");
    assert!(
        !texts.iter().any(|t| t.contains("/9)")),
        "no counter over an empty list: {texts:?}"
    );
}

#[test]
fn no_page_ever_stacks_two_blank_rows() {
    let mut app = open_app();
    for query in ["", "co", "zzz"] {
        let picker = app.spinner_picker.as_mut().expect("open");
        picker.query = query.to_string();
        picker.selected = 0;
        let texts: Vec<String> = texts(&app, 80)
            .iter()
            .map(|t| t.trim_end().to_string())
            .collect();
        for pair in texts.windows(2) {
            assert!(
                !(pair[0].is_empty() && pair[1].is_empty()),
                "query {query:?} stacked two blank rows: {texts:?}"
            );
        }
    }
}

#[test]
fn the_page_never_exceeds_the_width() {
    let mut app = open_app();
    for width in [24u16, 40, 60, 80, 120] {
        for i in 0..Spinner::ALL.len() {
            app.spinner_picker.as_mut().expect("open").selected = i;
            for line in spinner_view_lines(&app, width, TALL) {
                assert!(
                    crate::ui::wrap::cols(&plain(&line)) <= width as usize,
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
        Some(10),
        "clamped to a short terminal"
    );
}

#[test]
fn the_picker_never_flows_its_page() {
    let app = open_app();
    for height in (1..=12).chain([TALL]) {
        assert!(
            crate::ui::view_flow(&app, 80, height, 1000).is_none(),
            "{height} rows: a rung fits"
        );
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
    for height in (1..=12).chain([TALL]) {
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

/// The display column the row's glyph starts at, for the list row naming
/// `name`.
fn glyph_col(texts: &[String], name: &str) -> usize {
    let row = row_for(texts, name);
    let after = row.find(name).expect("the name") + name.len();
    let glyph = row[after..].trim_start();
    crate::ui::wrap::cols(row) - crate::ui::wrap::cols(glyph)
}

fn is_rule(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c == '─')
}

#[test]
fn a_short_terminal_keeps_the_spinner_choices_on_screen() {
    // The whole page is 22 rows; on 12 its preview and description kept the
    // screen while the search line and the highlighted style flowed away.
    let app = open_app();
    assert_eq!(page_at(&app, 80, TALL).len(), 22, "the whole page");
    let texts = page_at(&app, 80, 12);
    assert_eq!(texts.len(), 11, "the compact page: {texts:#?}");
    assert!(is_rule(&texts[0]), "{texts:#?}");
    assert_eq!(texts[2], "  ❯", "the search line: {texts:#?}");
    assert_eq!(texts[3], "", "nothing above comet: {texts:#?}");
    assert!(texts[4].starts_with("  comet"), "{texts:#?}");
    assert!(
        texts[5].starts_with("→ gravity"),
        "the open seats on it: {texts:#?}"
    );
    assert!(texts[6].starts_with("  wave"), "{texts:#?}");
    assert_eq!(texts[7], "  ↓ 6 more below", "{texts:#?}");
    assert!(texts[8].contains("Enter to choose"), "{texts:#?}");
    assert!(is_rule(&texts[10]), "{texts:#?}");
    assert!(
        !texts.iter().any(|t| t.contains("(2/9)")),
        "no counter: {texts:#?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("Working")),
        "no preview: {texts:#?}"
    );
}

#[test]
fn the_compact_spinner_window_rides_the_selection_and_holds_still() {
    // The glyph column is sized off every listed style, so it holds still as
    // the three rows in view change.
    let mut app = open_app();
    let col = glyph_col(&page_at(&app, 80, 12), "gravity");
    for i in 0..Spinner::ALL.len() {
        select(&mut app, i);
        let texts = page_at(&app, 80, 12);
        assert_eq!(texts.len(), 11, "selection {i} moved the frame");
        let name = Spinner::ALL[i].name();
        assert_eq!(
            glyph_col(&texts, name),
            col,
            "selection {i} moved the glyphs: {texts:#?}"
        );
    }
    select(&mut app, 5); // blocks
    let texts = page_at(&app, 80, 12);
    assert_eq!(texts[3], "  ↑ 4 more above", "{texts:#?}");
    assert!(texts[5].starts_with("→ blocks"), "{texts:#?}");
    assert_eq!(texts[7], "  ↓ 2 more below", "{texts:#?}");
}

#[test]
fn the_whole_spinner_page_returns_once_it_fits() {
    let app = open_app();
    let whole = u16::try_from(page_at(&app, 80, TALL).len()).expect("rows");
    let has_preview = |texts: &[String]| texts.iter().any(|t| t.contains("Working"));
    assert!(
        has_preview(&page_at(&app, 80, whole)),
        "whole at its own height"
    );
    assert!(
        !has_preview(&page_at(&app, 80, whole - 1)),
        "compact a row short"
    );
}

#[test]
fn the_cursor_sits_on_the_compact_spinner_search_line() {
    let app = open_app();
    let height = spinner_picker_height(&app, 80, 12).expect("open");
    assert_eq!(height, 11, "the compact page");
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
    assert_sheds_to_its_search_line(
        |height| page_at(&app, 80, height),
        "",
        "→ gravity",
        "Enter to choose",
    );
    for height in 1..=10 {
        assert_caret_on_search_line(&app, &page_at(&app, 80, height), 80, "");
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
