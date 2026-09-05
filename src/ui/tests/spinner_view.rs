//! The inline `/spinner` picker view (`docs/spinner.md`).

use super::*;
use crate::app::Spinner;
use crate::ui::spinner_view::{spinner_menu_rows, spinner_view_lines};

/// An app with session info and the picker open (the ordinary shape).
fn open_app() -> App {
    let mut app = with_session();
    app.open_spinner_picker();
    app
}

fn texts(app: &App, width: u16) -> Vec<String> {
    spinner_view_lines(app, width).iter().map(plain).collect()
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
    assert!(spinner_view_lines(&with_session(), 80).is_empty());
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
        texts.iter().any(|t| t.contains("(1/9)")),
        "the counter — the open seats on comet, the default: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("Enter to choose")),
        "the key hint: {texts:?}"
    );
    // The live preview: the highlighted (default comet) style as a whole
    // status line, built by the status line's own renderer.
    let preview = preview(&texts);
    assert!(
        preview.contains("(●•·   ) Working… (0s · esc to interrupt)"),
        "the sample status line wears the comet: {preview:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains(Spinner::Comet.description())),
        "the highlighted style's description: {texts:?}"
    );
}

#[test]
fn every_row_carries_its_own_live_glyph() {
    // The list is a live catalog: each row shows its style's spinner at the
    // shared frame clock, so the styles compare at a glance.
    let texts = texts(&open_app(), 80);
    assert!(row_for(&texts, "comet").contains("(●•·   )"), "{texts:?}");
    assert!(row_for(&texts, "dots").contains('⠋'), "{texts:?}");
    assert!(row_for(&texts, "orbit").contains('◐'), "{texts:?}");
    assert!(row_for(&texts, "blocks").contains('▙'), "{texts:?}");
    assert!(row_for(&texts, "pulse").contains('●'), "{texts:?}");
    assert!(row_for(&texts, "bars").contains('▁'), "{texts:?}");
    assert!(row_for(&texts, "line").contains('|'), "{texts:?}");
    assert!(row_for(&texts, "still").contains('•'), "{texts:?}");
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
    assert!(
        preview.contains("( ●•·  )"),
        "the preview's comet is on the same frame as its row: {preview:?}"
    );
}

#[test]
fn the_preview_follows_the_selection() {
    let mut app = open_app();
    app.spinner_picker.as_mut().expect("open").selected = 2; // dots
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
            .any(|t| t.contains(Spinner::Comet.description())),
        "comet's description left with the selection: {texts:?}"
    );
}

#[test]
fn the_page_height_is_stable_across_selections_and_ticks() {
    // Styles differ in width (the comet is eight cells, the rest one) but
    // never in height: moving the selection or advancing the clock must not
    // move the frame (the /settings always-emit rule).
    let mut app = open_app();
    let h0 = spinner_view_lines(&app, 80).len();
    for i in 1..Spinner::ALL.len() {
        app.spinner_picker.as_mut().expect("open").selected = i;
        assert_eq!(
            spinner_view_lines(&app, 80).len(),
            h0,
            "selection {i} changed the page height"
        );
    }
    for ms in [40u64, 333, 999, 12_345] {
        app.set_pulse(Duration::from_millis(ms));
        assert_eq!(
            spinner_view_lines(&app, 80).len(),
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
        texts.iter().any(|t| t.contains("(5/9)")),
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
            for line in spinner_view_lines(&app, width) {
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
    let lines = spinner_view_lines(&app, 80).len() as u16;
    assert_eq!(spinner_menu_rows(&app, 80), lines);
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
fn the_picker_flows_like_its_family() {
    let app = open_app();
    let page = spinner_view_lines(&app, 80).len();
    let flow = crate::ui::view_flow(&app, 80, 10, 1000).expect("flows on a 10-row terminal");
    assert_eq!(flow.lines.len(), page - 10, "the skipped top flows");
}

#[test]
fn a_tick_never_resigns_the_flow_but_a_keystroke_does() {
    // The page animates at the 32 ms cadence, so it signs the SELECTION and
    // the query — the ↓ manager's details-page rule — never its rows: a
    // frame advancing the spinners must not purge-rebuild the screen, while
    // a keystroke that moves the highlight re-flows the page.
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = open_app();
    let (width, height) = (80u16, 10u16);
    let before = crate::ui::view_flow_signature(&app, width, height, 1000).expect("overflows");
    app.set_pulse(Duration::from_millis(480));
    let ticked = crate::ui::view_flow_signature(&app, width, height, 1000).expect("overflows");
    assert_eq!(before, ticked, "a tick must not churn a purge rebuild");
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let moved = crate::ui::view_flow_signature(&app, width, height, 1000).expect("overflows");
    assert_ne!(before, moved, "moving the highlight re-signs the flow");
    app.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    let typed = crate::ui::view_flow_signature(&app, width, height, 1000).expect("overflows");
    assert_ne!(moved, typed, "typing into the search re-signs the flow");
}
