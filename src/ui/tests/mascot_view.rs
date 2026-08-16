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

fn texts(app: &App, width: u16) -> Vec<String> {
    mascot_view_lines(app, width).iter().map(plain).collect()
}

#[test]
fn closed_builds_nothing() {
    assert!(mascot_view_lines(&with_session(), 80).is_empty());
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
    let h0 = mascot_view_lines(&app, 80).len();
    for i in 1..Mascot::ALL.len() {
        app.mascot_picker.as_mut().expect("open").selected = i;
        assert_eq!(
            mascot_view_lines(&app, 80).len(),
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
fn an_unmatched_search_shows_the_placeholder() {
    let mut app = open_app();
    app.mascot_picker.as_mut().expect("open").query = "zzz".to_string();
    let texts = texts(&app, 80);
    assert!(
        texts.iter().any(|t| t.contains("No matching mascots")),
        "{texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("/6)")),
        "no counter over an empty list: {texts:?}"
    );
}

#[test]
fn the_page_never_exceeds_the_width() {
    let mut app = open_app();
    for width in [24u16, 40, 60, 80, 120] {
        for i in 0..Mascot::ALL.len() {
            app.mascot_picker.as_mut().expect("open").selected = i;
            for line in mascot_view_lines(&app, width) {
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
    let lines = mascot_view_lines(&app, 80).len() as u16;
    assert_eq!(mascot_menu_rows(&app, 80), lines);
    assert_eq!(
        mascot_picker_height(&app, 80, 200),
        Some(lines),
        "an idle app has no strip above the picker"
    );
    assert_eq!(
        mascot_picker_height(&app, 80, 10),
        Some(10),
        "clamped to a short terminal"
    );
}

#[test]
fn the_picker_flows_like_its_family() {
    // A short terminal bottom-anchors the page and flows the skipped top
    // into scrollback (docs/view-flow.md) — the picker must be flow-eligible
    // like /settings, or its page top would silently clip.
    let app = open_app();
    let page = mascot_view_lines(&app, 80).len();
    let flow = crate::ui::view_flow(&app, 80, 10, 1000).expect("flows on a 10-row terminal");
    assert_eq!(flow.lines.len(), page - 10, "the skipped top flows");
}
