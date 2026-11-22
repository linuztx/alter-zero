//! The read-only `/discord` page (`docs/discord.md`).

use super::*;
use crate::app::DISCORD_INVITE;
use crate::ui::discord_view::{discord_menu_rows, discord_view_lines};
use crate::ui::theme::{error_color, model_meta_color, model_selected_color};

/// The page, open.
fn open() -> App {
    let mut app = with_session();
    app.open_discord_page();
    app
}

/// A terminal tall enough for the whole page.
const TALL: u16 = 200;

/// The page on a terminal `height` rows tall, as trimmed plain rows.
fn page_at(app: &App, width: u16, height: u16) -> Vec<String> {
    discord_view_lines(app, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

#[test]
fn closed_builds_nothing() {
    assert!(discord_view_lines(&with_session(), 80, TALL).is_empty());
    assert_eq!(discord_page_height(&with_session(), 80, 200), None);
}

#[test]
fn the_page_reads_title_blurb_the_boxed_invite_and_the_hint() {
    let texts = page_at(&open(), 80, TALL);
    assert_eq!(
        texts,
        [
            "─".repeat(80).as_str(),
            "  ♥ Join the Alter Zero Discord",
            "",
            "  Ask for help, share what you have built, and hear about new releases first.",
            "  Ideas for new features are welcome too.",
            "",
            "  ❯ 1. Discord  invite to the community server",
            "     ╭─────────────────────────────────╮",
            "     │  https://discord.gg/EDBSpmq2fV  │",
            "     ╰─────────────────────────────────╯",
            "",
            "  enter/c copy invite link  esc close",
            "─".repeat(80).as_str(),
        ]
    );
}

#[test]
fn the_title_wears_the_red_heart_and_the_one_row_is_lit() {
    let lines = discord_view_lines(&open(), 80, TALL);
    let title = lines
        .iter()
        .find(|l| plain(l).contains("Join the Alter Zero Discord"))
        .expect("title");
    let heart = title
        .spans
        .iter()
        .find(|s| s.content.contains('♥'))
        .expect("heart");
    assert_eq!(heart.style.fg, Some(error_color()));
    // The page's one row is the highlighted row: its box lights in the
    // accent, like the selected row's on /sponsor.
    let walls: Vec<_> = lines
        .iter()
        .filter_map(|l| l.spans.iter().find(|s| s.content.contains('╭')))
        .map(|s| s.style.fg)
        .collect();
    assert_eq!(walls, [Some(model_selected_color())]);
    let note = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains("invite to the community server"))
        .expect("note");
    assert_eq!(note.style.fg, Some(model_meta_color()), "the note is dim");
}

#[test]
fn the_boxed_invite_is_a_link_to_itself() {
    // A click opens the invite — and on a pane too narrow for the URL every
    // wrapped fragment opens the whole of it (docs/links.md).
    for width in [80u16, 30] {
        let lines = discord_view_lines(&open(), width, TALL);
        let fragments: Vec<_> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| {
                !s.content.trim().is_empty()
                    && DISCORD_INVITE.contains(s.content.as_ref())
                    && s.style.add_modifier.contains(Modifier::UNDERLINED)
            })
            .collect();
        assert!(
            width >= 80 || fragments.len() > 1,
            "width {width}: the invite wraps across the box"
        );
        let joined: String = fragments.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, DISCORD_INVITE, "width {width}: the invite, whole");
        for span in fragments {
            assert_eq!(
                crate::links::style_link(&span.style).as_deref(),
                Some(DISCORD_INVITE),
                "width {width}: {:?}",
                span.content
            );
        }
    }
}

#[test]
fn every_page_fits_the_width_and_hugs_its_frame() {
    // Whichever page a height builds — the whole one, or the row alone —
    // nothing overflows and no blank row meets a rule or another blank
    // (docs/view-flow.md).
    let app = open();
    for width in [20u16, 30, 60, 90] {
        for height in 1..=24u16 {
            let lines = discord_view_lines(&app, width, height);
            let what = format!("width {width} on {height} rows");
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

#[test]
fn a_short_terminal_drops_the_title_and_blurb_first() {
    let app = open();
    let whole = page_at(&app, 80, TALL);
    let short = page_at(&app, 80, whole.len() as u16 - 1);
    assert_eq!(
        short,
        [
            "─".repeat(80).as_str(),
            "  ❯ 1. Discord  invite to the community server",
            "     ╭─────────────────────────────────╮",
            "     │  https://discord.gg/EDBSpmq2fV  │",
            "     ╰─────────────────────────────────╯",
            "",
            "  enter/c copy invite link  esc close",
            "─".repeat(80).as_str(),
        ]
    );
}

#[test]
fn height_is_the_line_count_and_a_shorter_terminal_flows() {
    let app = open();
    let lines = discord_view_lines(&app, 80, TALL).len() as u16;
    assert_eq!(discord_menu_rows(&app, 80, TALL), lines);
    assert_eq!(discord_page_height(&app, 80, 200), Some(lines));
    assert_eq!(discord_page_height(&app, 80, 5), Some(5), "clamped");
    // Shorter than the smallest page: bottom-anchored, the top flowing into
    // scrollback like its family.
    let page = discord_view_lines(&app, 80, 5).len();
    let flow = crate::ui::view_flow(&app, 80, 5, 1000).expect("flows on 5 rows");
    assert_eq!(flow.lines.len(), page - 5);
}

#[test]
fn render_paints_the_lines_and_the_cursor_hides_seated_on_the_marker() {
    let app = open();
    let height = discord_page_height(&app, 80, 200).unwrap();
    let area = Rect::new(0, 0, 80, height);
    let mut buf = buffer(80, height);
    render_live(area, &mut buf, &app);
    let painted: Vec<String> = (0..height).map(|y| row(&buf, y, 80)).collect();
    assert!(painted[0].starts_with('─'), "{painted:#?}");
    assert!(
        painted.iter().any(|r| r.contains(DISCORD_INVITE)),
        "{painted:#?}"
    );
    assert!(!cursor_visible(&app), "a read-only page shows no cursor");
    let marker_row = (0..height)
        .find(|&y| row(&buf, y, 80).trim_start().starts_with('❯'))
        .expect("the row wears the marker");
    assert!(row(&buf, marker_row, 80).contains("Discord"));
    assert_eq!(cursor_position(area, &app), (2, marker_row));
}
