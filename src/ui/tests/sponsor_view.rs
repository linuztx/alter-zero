//! The read-only `/sponsor` page (`docs/sponsor.md`).

use super::*;
use crate::app::SPONSOR_LINKS;
use crate::ui::sponsor_view::{sponsor_menu_rows, sponsor_view_lines};
use crate::ui::theme::{border_color, error_color, model_meta_color, model_selected_color};

/// The page with the highlight on row `selected`.
fn open_at(selected: usize) -> App {
    let mut app = with_session();
    app.open_sponsor_page();
    app.sponsor_page.as_mut().expect("open").selected = selected;
    app
}

/// A terminal tall enough for the whole page.
const TALL: u16 = 200;

/// The page on a terminal `height` rows tall, as trimmed plain rows.
fn page_at(app: &App, width: u16, height: u16) -> Vec<String> {
    sponsor_view_lines(app, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

fn is_rule(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty() && t.chars().all(|c| c == '─')
}

fn boxes(texts: &[String]) -> usize {
    texts.iter().filter(|t| t.contains('╭')).count()
}

#[test]
fn closed_builds_nothing() {
    assert!(sponsor_view_lines(&with_session(), 80, TALL).is_empty());
    assert_eq!(sponsor_page_height(&with_session(), 80, 200), None);
}

#[test]
fn the_page_reads_title_blurb_two_boxed_rows_and_the_hint() {
    let texts = page_at(&open_at(0), 80, TALL);
    assert_eq!(
        texts,
        [
            "─".repeat(80).as_str(),
            "  ♥ Sponsor Alter Zero",
            "",
            "  Alter Zero is built and maintained by one developer. A sponsorship pays for",
            "  the time that goes into it — thank you.",
            "",
            "  ❯ 1. GitHub Sponsors  monthly or one-time",
            "     ╭───────────────────────────────────────╮",
            "     │  https://github.com/sponsors/linuztx  │",
            "     ╰───────────────────────────────────────╯",
            "",
            "    2. Email  company sponsorships and anything else",
            "     ╭─────────────────────╮",
            "     │  linuztx@gmail.com  │",
            "     ╰─────────────────────╯",
            "",
            "  ↑↓ navigate  enter/c copy  esc close",
            "─".repeat(80).as_str(),
        ]
    );
}

#[test]
fn the_title_wears_the_red_heart_and_the_selection_lights_its_row_and_box() {
    let lines = sponsor_view_lines(&open_at(1), 80, TALL);
    let title = lines
        .iter()
        .find(|l| plain(l).contains("Sponsor Alter Zero"))
        .expect("title");
    let heart = title
        .spans
        .iter()
        .find(|s| s.content.contains('♥'))
        .expect("heart");
    assert_eq!(heart.style.fg, Some(error_color()));
    // Each box's top wall: the highlighted row's in the accent, the other's
    // in the frame colour.
    let walls: Vec<_> = lines
        .iter()
        .filter_map(|l| l.spans.iter().find(|s| s.content.contains('╭')))
        .map(|s| s.style.fg)
        .collect();
    assert_eq!(walls, [Some(border_color()), Some(model_selected_color())]);
    let note = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains("monthly or one-time"))
        .expect("note");
    assert_eq!(note.style.fg, Some(model_meta_color()), "the note is dim");
}

#[test]
fn each_boxed_value_is_a_link_to_its_target() {
    // A click opens the sponsor page, or a message to the address — and on
    // a pane too narrow for the URL every wrapped fragment opens the whole
    // target (docs/links.md).
    for width in [80u16, 30] {
        let lines = sponsor_view_lines(&open_at(0), width, TALL);
        for link in SPONSOR_LINKS {
            let fragments: Vec<_> = lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .filter(|s| {
                    !s.content.trim().is_empty()
                        && link.value.contains(s.content.as_ref())
                        && s.style.add_modifier.contains(Modifier::UNDERLINED)
                })
                .collect();
            let joined: String = fragments.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(joined, link.value, "width {width}: the value, whole");
            for span in fragments {
                assert_eq!(
                    crate::links::style_link(&span.style).as_deref(),
                    Some(link.target),
                    "width {width}: {:?}",
                    span.content
                );
            }
        }
    }
}

#[test]
fn every_page_fits_the_width_and_hugs_its_frame() {
    // Whichever page a height builds — the whole one, the accordion, the
    // highlighted row alone — nothing overflows and no blank row meets a
    // rule or another blank (docs/view-flow.md).
    for width in [20u16, 30, 60, 90] {
        for selected in 0..SPONSOR_LINKS.len() {
            let app = open_at(selected);
            for height in 1..=24u16 {
                let lines = sponsor_view_lines(&app, width, height);
                let what = format!("width {width} selection {selected} on {height} rows");
                assert_frame_hugs_content(&what, &lines);
                let texts = page_at(&app, width, height);
                for pair in texts.windows(2) {
                    assert!(
                        !(pair[0].is_empty() && pair[1].is_empty()),
                        "{what}: {texts:#?}"
                    );
                }
                for text in &texts {
                    assert!(
                        crate::ui::wrap::cols(text) <= width as usize,
                        "{what}: {text:?} overflows"
                    );
                }
            }
        }
    }
}

#[test]
fn a_short_terminal_folds_the_other_row_then_drops_the_header() {
    let app = open_at(1);
    let whole = page_at(&app, 80, TALL);
    assert_eq!(boxes(&whole), 2);
    // One row short: the row the highlight is not on folds to its label.
    let accordion = page_at(&app, 80, whole.len() as u16 - 1);
    assert_eq!(boxes(&accordion), 1, "{accordion:#?}");
    assert!(accordion.iter().any(|t| t.contains("1. GitHub Sponsors")));
    assert!(accordion.iter().any(|t| t.contains("linuztx@gmail.com")));
    assert!(accordion.iter().any(|t| t.contains("Sponsor Alter Zero")));
    // Shorter still: the title and blurb go, then the other label.
    let headless = page_at(&app, 80, accordion.len() as u16 - 1);
    assert!(!headless.iter().any(|t| t.contains("Sponsor Alter Zero")));
    assert!(headless.iter().any(|t| t.contains("1. GitHub Sponsors")));
    let single = page_at(&app, 80, headless.len() as u16 - 1);
    assert_eq!(
        single,
        [
            "─".repeat(80).as_str(),
            "  ❯ 2. Email  company sponsorships and anything else",
            "     ╭─────────────────────╮",
            "     │  linuztx@gmail.com  │",
            "     ╰─────────────────────╯",
            "",
            "  ↑↓ navigate  enter/c copy  esc close",
            "─".repeat(80).as_str(),
        ]
    );
    assert!(is_rule(&single[0]));
}

#[test]
fn height_is_the_line_count_and_a_shorter_terminal_flows() {
    let app = open_at(0);
    let lines = sponsor_view_lines(&app, 80, TALL).len() as u16;
    assert_eq!(sponsor_menu_rows(&app, 80, TALL), lines);
    assert_eq!(sponsor_page_height(&app, 80, 200), Some(lines));
    assert_eq!(sponsor_page_height(&app, 80, 5), Some(5), "clamped");
    // Shorter than the smallest page: bottom-anchored, the top flowing into
    // scrollback like its family.
    let page = sponsor_view_lines(&app, 80, 5).len();
    let flow = crate::ui::view_flow(&app, 80, 5, 1000).expect("flows on 5 rows");
    assert_eq!(flow.lines.len(), page - 5);
}

#[test]
fn render_paints_the_lines_and_the_cursor_hides_seated_on_the_marker() {
    for (selected, highlighted) in SPONSOR_LINKS.iter().enumerate() {
        let app = open_at(selected);
        let height = sponsor_page_height(&app, 80, 200).unwrap();
        let area = Rect::new(0, 0, 80, height);
        let mut buf = buffer(80, height);
        render_live(area, &mut buf, &app);
        let painted: Vec<String> = (0..height).map(|y| row(&buf, y, 80)).collect();
        assert!(painted[0].starts_with('─'), "{painted:#?}");
        for link in SPONSOR_LINKS {
            assert!(
                painted.iter().any(|r| r.contains(link.value)),
                "{painted:#?}"
            );
        }
        assert!(!cursor_visible(&app), "a read-only page shows no cursor");
        let marker_row = (0..height)
            .find(|&y| row(&buf, y, 80).trim_start().starts_with('❯'))
            .expect("the selected row wears the marker");
        assert!(row(&buf, marker_row, 80).contains(highlighted.label));
        assert_eq!(cursor_position(area, &app), (2, marker_row));
    }
}
