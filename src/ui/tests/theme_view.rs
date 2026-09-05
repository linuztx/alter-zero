//! The inline `/theme` picker view (`docs/theme.md`).

use super::*;
use crate::app::Theme;
use crate::ui::palette::{active_theme, palette_of};
use crate::ui::theme_view::{theme_menu_rows, theme_view_lines};

/// An app with session info and the picker open (the ordinary shape).
fn open_app() -> App {
    let mut app = with_session();
    app.open_theme_picker();
    app
}

fn texts(app: &App, width: u16) -> Vec<String> {
    theme_view_lines(app, width).iter().map(plain).collect()
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

/// The styled list row for `name`.
fn line_for<'a>(lines: &'a [Line<'static>], name: &str) -> &'a Line<'static> {
    lines
        .iter()
        .find(|l| {
            let t = plain(l);
            let t = t.trim_start_matches("→ ").trim_start();
            t.starts_with(name) && t[name.len()..].chars().next().is_none_or(|c| c == ' ')
        })
        .unwrap_or_else(|| panic!("no row for {name}"))
}

/// The preview's `● Edit(greet.py)` header line.
fn edit_header<'a>(lines: &'a [Line<'static>]) -> &'a Line<'static> {
    lines
        .iter()
        .find(|l| plain(l).contains("Edit(greet.py)"))
        .expect("the preview's Edit header")
}

#[test]
fn closed_builds_nothing() {
    assert!(theme_view_lines(&with_session(), 80).is_empty());
    assert_eq!(theme_picker_height(&with_session(), 80, 200), None);
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
    assert!(
        row_for(&texts, "mocha").starts_with("→ "),
        "the open seats the marker on the default: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("(1/11)")),
        "the counter over the eleven-theme catalog: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("Enter to choose")),
        "the key hint: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains(Theme::Mocha.description())),
        "the highlighted theme's description: {texts:?}"
    );
}

#[test]
fn the_preview_is_the_sample_conversations_real_cells() {
    // A user bubble, the Edit diff cell (head, context row, the removed and
    // added rows), and the reply with its inline code and fenced code line
    // — through the conversation's own builders, inset two columns.
    let texts = texts(&open_app(), 80);
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
            "missing {expect:?}: {texts:?}"
        );
    }
    assert!(
        texts
            .iter()
            .filter(|t| t.contains("Rename the greeting"))
            .all(|t| t.starts_with("  ❯")),
        "the preview is inset two columns: {texts:?}"
    );
}

#[test]
fn every_row_wears_its_own_swatch() {
    // Five swatches per row, each in that theme's own accent, link,
    // success, warning and error — read off its palette, never the active
    // theme's, so Dracula's row is Dracula-coloured while Mocha is active.
    let lines = theme_view_lines(&open_app(), 80);
    for theme in Theme::ALL.iter().take(8) {
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
            "{theme:?}'s swatches"
        );
    }
    assert_eq!(active_theme(), Theme::Mocha, "the swatches leak nothing");
}

#[test]
fn the_preview_wears_the_highlighted_theme_and_the_frame_the_active_one() {
    let mut app = open_app();
    let mocha = theme_view_lines(&app, 80);
    assert_eq!(
        edit_header(&mocha).spans[1].style.fg,
        Some(palette_of(Theme::Mocha).success),
        "the Edit bullet is Mocha's green while Mocha is highlighted"
    );
    // ↓ four times: mocha → macchiato → frappe → latte → onedark; then to
    // dracula, the sixth row.
    app.theme_picker.as_mut().expect("open").selected = 5;
    let dracula = theme_view_lines(&app, 80);
    assert_eq!(
        edit_header(&dracula).spans[1].style.fg,
        Some(palette_of(Theme::Dracula).success),
        "…and Dracula's green once Dracula is highlighted"
    );
    // The code line is syntax-coloured by the highlighted theme's own code
    // theme, so the two previews disagree on the `def` keyword.
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
    let prompt = |lines: &[Line<'static>]| {
        lines
            .iter()
            .find(|l| plain(l).trim_start().starts_with('❯'))
            .expect("the search line")
            .spans[1]
            .style
            .fg
    };
    assert_eq!(prompt(&mocha), Some(palette_of(Theme::Mocha).accent));
    assert_eq!(prompt(&dracula), Some(palette_of(Theme::Mocha).accent));
    assert_eq!(active_theme(), Theme::Mocha);
    assert!(
        texts(&app, 80)
            .iter()
            .any(|t| t.contains(Theme::Dracula.description())),
        "…with Dracula's description"
    );
}

#[test]
fn the_diff_rows_wear_the_highlighted_themes_tints() {
    let mut app = open_app();
    app.theme_picker.as_mut().expect("open").selected = 3; // latte
    let lines = theme_view_lines(&app, 80);
    let p = palette_of(Theme::Latte);
    let removed = lines
        .iter()
        .find(|l| plain(l).contains("-    print(\"Hello, world\")"))
        .expect("the removed row");
    assert!(
        removed
            .spans
            .iter()
            .any(|s| s.style.bg == Some(p.diff_del_bg) || s.style.bg == Some(p.diff_del_mark_bg)),
        "the removed row is tinted in Latte's red: {removed:?}"
    );
    let added = lines
        .iter()
        .find(|l| plain(l).contains("+    print(f\"Hello, {name}\")"))
        .expect("the added row");
    assert!(
        added
            .spans
            .iter()
            .any(|s| s.style.bg == Some(p.diff_add_bg) || s.style.bg == Some(p.diff_add_mark_bg)),
        "the added row is tinted in Latte's green: {added:?}"
    );
    let bubble = lines
        .iter()
        .find(|l| plain(l).contains("Rename the greeting"))
        .expect("the user bubble");
    assert_eq!(
        bubble.style.bg,
        Some(p.user_bg),
        "the bubble is Latte's pale ground"
    );
}

#[test]
fn the_page_height_is_stable_across_selections() {
    // The sample is the same shape for every theme, so moving the selection
    // must not move the frame (the /settings always-emit rule) — even onto
    // the ANSI theme, whose cells carry no RGB.
    let mut app = open_app();
    let h0 = theme_view_lines(&app, 80).len();
    for i in 1..Theme::ALL.len() {
        app.theme_picker.as_mut().expect("open").selected = i;
        assert_eq!(
            theme_view_lines(&app, 80).len(),
            h0,
            "selection {i} changed the page height"
        );
    }
}

#[test]
fn the_active_theme_wears_the_check_and_the_open_seats_on_it() {
    let mut app = with_session();
    app.set_theme(Theme::Nord);
    app.open_theme_picker();
    let texts = texts(&app, 80);
    assert!(
        row_for(&texts, "nord").contains('✓'),
        "nord carries the ✓: {texts:?}"
    );
    // The ten-row window has scrolled `mocha` off the top; of the rows it
    // shows, only nord's is marked.
    assert_eq!(
        texts.iter().filter(|t| t.contains('✓')).count(),
        1,
        "only the active row is marked: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("(7/11)")),
        "the highlight seats on the active theme: {texts:?}"
    );
}

#[test]
fn an_unmatched_search_collapses_to_placeholder_and_hint() {
    let mut app = open_app();
    app.theme_picker.as_mut().expect("open").query = "zzz".to_string();
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
        texts[4].contains("No matching themes"),
        "the placeholder: {texts:?}"
    );
    assert_eq!(texts[5], "", "one gap under the placeholder: {texts:?}");
    assert!(texts[6].contains("Type to search"), "the hint: {texts:?}");
    assert_eq!(texts[7], "", "{texts:?}");
    assert!(is_rule(&texts[8]), "{texts:?}");
    assert!(
        !texts.iter().any(|t| t.contains("/11)")),
        "no counter over an empty list: {texts:?}"
    );
}

#[test]
fn no_page_ever_stacks_two_blank_rows() {
    let mut app = open_app();
    for query in ["", "ca", "zzz"] {
        let picker = app.theme_picker.as_mut().expect("open");
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
        for i in 0..Theme::ALL.len() {
            app.theme_picker.as_mut().expect("open").selected = i;
            for line in theme_view_lines(&app, width) {
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
    let lines = theme_view_lines(&app, 80).len() as u16;
    assert_eq!(theme_menu_rows(&app, 80), lines);
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
fn the_picker_flows_like_its_family_and_a_keystroke_resigns_the_flow() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = open_app();
    let page = theme_view_lines(&app, 80).len();
    let flow = crate::ui::view_flow(&app, 80, 10, 1000).expect("flows on a 10-row terminal");
    assert_eq!(flow.lines.len(), page - 10, "the skipped top flows");
    // A still page signs its rows: moving the highlight re-flows it.
    let before = crate::ui::view_flow_signature(&app, 80, 10, 1000).expect("overflows");
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let moved = crate::ui::view_flow_signature(&app, 80, 10, 1000).expect("overflows");
    assert_ne!(before, moved, "moving the highlight re-signs the flow");
}

#[test]
fn the_picker_renders_into_a_buffer_bottom_anchored() {
    let app = open_app();
    let area = Rect::new(0, 0, 80, 12);
    let mut buf = Buffer::empty(area);
    render_theme_picker(area, &mut buf, &app);
    let rows: Vec<String> = (0..12).map(|y| row(&buf, y, 80)).collect();
    assert!(
        rows[11].trim().chars().all(|c| c == '─') && !rows[11].trim().is_empty(),
        "the closing rule sits on the last row: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("Type to search")),
        "the hint stays on screen: {rows:?}"
    );
}
