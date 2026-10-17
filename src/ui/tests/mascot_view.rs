//! The inline `/mascot` picker view (`docs/mascot.md`).

use super::*;
use crate::app::Mascot;
use crate::ui::mascot_view::{mascot_menu_rows, mascot_view_lines};

/// An app with session info and the picker open (the ordinary shape).
fn open_app() -> App {
    let mut app = with_session();
    app.open_mascot_picker();
    app
}

/// A terminal tall enough for the whole page.
const TALL: u16 = 200;

fn texts(app: &App, width: u16) -> Vec<String> {
    mascot_view_lines(app, width, TALL)
        .iter()
        .map(plain)
        .collect()
}

#[test]
fn closed_builds_nothing() {
    assert!(mascot_view_lines(&with_session(), 80, TALL).is_empty());
    assert_eq!(mascot_picker_height(&with_session(), 80, 200), None);
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
    for mascot in Mascot::ALL {
        assert!(
            texts.iter().any(|t| t.contains(mascot.name())),
            "{} listed: {texts:?}",
            mascot.name()
        );
    }
    assert!(
        texts.iter().any(|t| t.contains("(1/6)")),
        "the counter — the open seats on crest, the default: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("Enter to choose")),
        "the key hint: {texts:?}"
    );
    // The live preview: the highlighted (default crest) banner — its art AND
    // its metadata, built by the same header builder the startup banner uses.
    assert!(
        texts.iter().any(|t| t.contains("▙▄▙▄▟▄▟")),
        "the preview art: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("Alter Zero")),
        "the preview title: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("~/alter-zero")),
        "the preview cwd: {texts:?}"
    );
}

#[test]
fn the_preview_follows_the_selection() {
    let mut app = open_app();
    app.mascot_picker.as_mut().expect("open").selected = 1; // bloom
    let texts = texts(&app, 80);
    assert!(
        texts.iter().any(|t| t.contains("▀█▄███▄█▀")),
        "bloom previews: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("▙▄▙▄▟▄▟")),
        "crest's art left with the selection: {texts:?}"
    );
}

#[test]
fn the_preview_slot_height_is_stable_across_selections() {
    // Mascots are 3–5 rows tall; the preview slot pads to the tallest so
    // the frame never jumps as ↑/↓ move (the /settings always-emit rule).
    let mut app = open_app();
    let h0 = mascot_view_lines(&app, 80, TALL).len();
    for i in 1..Mascot::ALL.len() {
        app.mascot_picker.as_mut().expect("open").selected = i;
        assert_eq!(
            mascot_view_lines(&app, 80, TALL).len(),
            h0,
            "selection {i} changed the page height"
        );
    }
}

#[test]
fn the_active_mascot_wears_the_check() {
    let mut app = with_session();
    app.set_mascot(Mascot::Gem);
    app.open_mascot_picker();
    let texts = texts(&app, 80);
    assert!(
        texts.iter().any(|t| t.contains("gem") && t.contains('✓')),
        "gem carries the ✓: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("crest") && t.contains('✓')),
        "only the active row is marked: {texts:?}"
    );
    // …and the open landed on it, so the picker resumes where the session is.
    assert!(
        texts.iter().any(|t| t.contains("(6/6)")),
        "the highlight seats on the active mascot: {texts:?}"
    );
}

#[test]
fn the_highlighted_description_shows() {
    let app = open_app();
    let texts = texts(&app, 80);
    assert!(
        texts
            .iter()
            .any(|t| t.contains(Mascot::Crest.description())),
        "the highlighted (default) mascot's description: {texts:?}"
    );
}

#[test]
fn an_unmatched_search_collapses_to_placeholder_and_hint() {
    // Nothing matched means there is no count, no banner to preview and no
    // description — so those three slots collapse to ONE blank gap instead
    // of a band of empty rows (the `/model` picker's placeholder rule). The
    // whole page is rule, search, blank, placeholder, blank, hint, rule.
    let mut app = open_app();
    app.mascot_picker.as_mut().expect("open").query = "zzz".to_string();
    let texts: Vec<String> = texts(&app, 80)
        .iter()
        .map(|t| t.trim_end().to_string())
        .collect();
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
    assert!(
        !texts.iter().any(|t| t.contains("/6)")),
        "no counter over an empty list: {texts:?}"
    );
}

#[test]
fn every_page_hugs_its_frame() {
    // The search line straight under the opening rule, the key hint
    // straight over the closing one, whatever the search shows.
    let mut app = open_app();
    for query in ["", "cr", "zzz"] {
        let picker = app.mascot_picker.as_mut().expect("open");
        picker.query = query.to_string();
        picker.selected = 0;
        assert_frame_hugs_content(
            &format!("the picker searching {query:?}"),
            &mascot_view_lines(&app, 80, TALL),
        );
    }
}

#[test]
fn no_page_ever_stacks_two_blank_rows() {
    // The reported wart: a run of empty rows in the middle of the frame. No
    // state of the picker — a real selection, a narrowed search, a search
    // that matched nothing — may stack blanks.
    let mut app = open_app();
    for query in ["", "cr", "zzz"] {
        let picker = app.mascot_picker.as_mut().expect("open");
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
        for i in 0..Mascot::ALL.len() {
            app.mascot_picker.as_mut().expect("open").selected = i;
            for line in mascot_view_lines(&app, width, TALL) {
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
    for height in (1..=12).chain([TALL]) {
        assert!(
            crate::ui::view_flow(&app, 80, height, 1000).is_none(),
            "{height} rows"
        );
    }
}

// --- the compact page: a short terminal keeps the choices on screen ---

/// The page on a terminal `height` rows tall, as trimmed plain rows.
fn page_at(app: &App, width: u16, height: u16) -> Vec<String> {
    mascot_view_lines(app, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

fn select(app: &mut App, i: usize) {
    app.mascot_picker.as_mut().expect("open").selected = i;
}

/// How many rows of `texts` are mascot rows.
fn mascot_rows(texts: &[String]) -> usize {
    texts
        .iter()
        .filter(|t| {
            let name = t.trim_start_matches('→').trim();
            Mascot::ALL
                .iter()
                .any(|m| name == m.name() || name.starts_with(&format!("{} ", m.name())))
        })
        .count()
}

fn is_rule(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c == '─')
}

#[test]
fn a_short_terminal_keeps_the_mascot_choices_on_screen() {
    // The whole page — the list, the counter, the banner preview and the
    // description — is 19 rows. On 12, bottom-anchoring it kept the preview
    // and flowed the search line and the highlighted mascot into scrollback.
    // The preview, the description and the counter yield instead.
    let app = open_app();
    assert_eq!(page_at(&app, 80, TALL).len(), 19, "the whole page");
    let texts = page_at(&app, 80, 12);
    assert!(texts.len() <= 12, "fits: {texts:#?}");
    assert!(
        texts.iter().any(|t| t == "  ❯"),
        "the search line: {texts:#?}"
    );
    assert!(texts.iter().any(|t| t.starts_with("→ crest")), "{texts:#?}");
    assert_eq!(mascot_rows(&texts), 3, "three rows: {texts:#?}");
    assert!(texts.iter().any(|t| t == "  ↓ 3 more below"), "{texts:#?}");
    assert!(
        texts.iter().any(|t| t.contains("Enter to choose")),
        "{texts:#?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("(1/6)")),
        "no counter: {texts:#?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("Alter Zero")),
        "no banner preview: {texts:#?}"
    );
}

#[test]
fn the_compact_mascot_page_is_the_familys_short_page() {
    let texts = page_at(&open_app(), 80, 12);
    assert_eq!(texts.len(), 9, "{texts:#?}");
    assert!(is_rule(&texts[0]), "{texts:#?}");
    assert_eq!(texts[1], "  ❯", "{texts:#?}");
    assert_eq!(texts[2], "", "{texts:#?}");
    assert!(texts[3].starts_with("→ crest"), "{texts:#?}");
    assert_eq!(texts[4], "  bloom", "{texts:#?}");
    assert_eq!(texts[5], "  sprout", "{texts:#?}");
    assert_eq!(texts[6], "  ↓ 3 more below", "{texts:#?}");
    assert!(texts[7].contains("Enter to choose"), "{texts:#?}");
    assert!(is_rule(&texts[8]), "{texts:#?}");
}

#[test]
fn the_compact_mascot_window_rides_the_selection_and_holds_still() {
    let mut app = open_app();
    select(&mut app, 3); // twin
    let texts = page_at(&app, 80, 12);
    assert_eq!(texts[2], "  ↑ 2 more above", "{texts:#?}");
    assert_eq!(texts[3], "  sprout", "{texts:#?}");
    assert!(texts[4].starts_with("→ twin"), "{texts:#?}");
    assert_eq!(texts[5], "  skiter", "{texts:#?}");
    assert_eq!(texts[6], "  ↓ 1 more below", "{texts:#?}");
    for i in 0..Mascot::ALL.len() {
        select(&mut app, i);
        assert_eq!(
            page_at(&app, 80, 12).len(),
            9,
            "selection {i} moved the frame"
        );
    }
}

#[test]
fn the_whole_mascot_page_returns_once_it_fits() {
    let app = open_app();
    let whole = u16::try_from(page_at(&app, 80, TALL).len()).expect("rows");
    let has_preview = |texts: &[String]| texts.iter().any(|t| t.contains("Alter Zero"));
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
fn the_cursor_sits_on_the_compact_mascot_search_line() {
    let app = open_app();
    let height = mascot_picker_height(&app, 80, 12).expect("open");
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
fn a_tiny_terminal_keeps_the_mascot_search_line() {
    let app = open_app();
    assert_sheds_to_its_search_line(
        |height| page_at(&app, 80, height),
        "",
        "→ crest",
        "Enter to choose",
    );
    for height in 1..=10 {
        assert_caret_on_search_line(&app, &page_at(&app, 80, height), 80, "");
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
