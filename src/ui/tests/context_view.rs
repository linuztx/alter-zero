//! The Ctrl+D context-debug overlay (`docs/context.md`).

use super::*;
use crate::ui::theme::{CONTEXT_VIEW_EMPTY, TOOL_VIEW_FOOTER_ROWS, TOOL_VIEW_TITLE_ROWS};
use crate::ui::wrap::cols;

#[test]
fn context_lines_show_the_raw_window_with_role_tags() {
    let app = context_fixture();
    let texts: Vec<String> = context_lines(&app, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    // The system prompt leads, tagged apart from mid-conversation notes.
    assert_eq!(texts[0], "system prompt:", "{texts:?}");
    assert_eq!(texts[1], "  be nice", "{texts:?}");
    assert!(texts.iter().any(|t| t == "user:"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "  hello"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "assistant:"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "  let me check"), "{texts:?}");
    // The tool call appears as a native `→ name(arguments)` request under
    // the assistant — the raw wire form, not the old `[tool …]` bracket and
    // not the TUI's bullet rendering.
    assert!(
        texts.iter().any(|t| t == r#"  → read({"path":"f"})"#),
        "{texts:?}"
    );
    assert!(!texts.iter().any(|t| t.contains("[tool")), "{texts:?}");
    // The result rides its own `tool:` role entry, in full.
    assert!(texts.iter().any(|t| t == "tool:"), "{texts:?}");
    for needle in ["  L1", "  L2", "  L3"] {
        assert!(texts.iter().any(|t| t == needle), "{texts:?}");
    }
    // Turn summaries are TUI chrome; they never reach the context.
    assert!(!texts.iter().any(|t| t.contains("Done")), "{texts:?}");
}

#[test]
fn context_lines_show_the_user_instructions_first() {
    // The AGENTS.md instructions fragment (docs/project-doc.md) is the
    // first user entry of the window — right after the system prompt, in
    // front of the conversation, exactly what the request carries.
    let mut app = context_fixture();
    app.set_user_instructions(Some("# AGENTS.md instructions\n\nguide".to_string()));
    let texts: Vec<String> = context_lines(&app, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(texts[0], "system prompt:", "{texts:?}");
    let first_user = texts.iter().position(|t| t == "user:").unwrap();
    assert_eq!(
        texts[first_user + 1],
        "  # AGENTS.md instructions",
        "{texts:?}"
    );
    assert_eq!(texts[first_user + 3], "  guide", "{texts:?}");
}

#[test]
fn context_lines_list_image_attachments_under_their_message() {
    let mut app = App::new();
    app.record_user_message_with_images(
        "[Image #1] what is this?",
        vec![std::path::PathBuf::from("/tmp/shot.png")],
    );
    let texts: Vec<String> = context_lines(&app, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(
        texts.iter().any(|t| t == "  [Image #1] what is this?"),
        "the placeholder stays raw in the text: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "  image: /tmp/shot.png"),
        "the attachment path lists beneath: {texts:?}"
    );
}

#[test]
fn context_lines_wrap_a_long_image_path_instead_of_clipping() {
    let mut app = App::new();
    app.record_user_message_with_images(
        "[Image #1]",
        vec![std::path::PathBuf::from(
            "/tmp/a-very-long-temp-directory-name/alter-zero-clipboard-0123456789.png",
        )],
    );
    let texts: Vec<String> = context_lines(&app, 30)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(
        texts.iter().any(|t| t.starts_with("  image: /tmp")),
        "the label row starts the path: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.ends_with(".png") && !t.contains("image:")),
        "the path's tail wraps onto a continuation row: {texts:?}"
    );
    assert!(
        texts.iter().all(|t| cols(t) <= 30),
        "no row exceeds the width: {texts:?}"
    );
}

#[test]
fn an_empty_context_shows_the_placeholder() {
    let lines = context_lines(&App::new(), 80);
    assert_eq!(lines.len(), 1);
    assert_eq!(plain(&lines[0]), CONTEXT_VIEW_EMPTY);
}

#[test]
fn render_context_view_paints_the_pager_chrome_with_its_own_title_and_keys() {
    let app = context_fixture();
    let mut buf = buffer(50, 14);
    render_context_view(buf.area, &mut buf, &app);
    let header = row(&buf, 0, 50);
    assert!(
        header.starts_with("/ C O N T E X T / "),
        "the title overlays the slash tiling: {header:?}"
    );
    let sep = row(&buf, 10, 50);
    assert!(sep.starts_with('─'), "{sep:?}");
    assert!(sep.contains('%'), "the scroll percentage rides it: {sep:?}");
    assert!(
        row(&buf, 11, 50).contains("to scroll"),
        "{:?}",
        row(&buf, 11, 50)
    );
    assert!(
        row(&buf, 12, 50).contains("q/esc/ctrl+d to quit"),
        "{:?}",
        row(&buf, 12, 50)
    );
    assert_eq!(row(&buf, 13, 50).trim(), "", "a blank final row");
}

#[test]
fn render_context_view_fills_rows_below_the_content_with_tildes() {
    let mut app = App::new();
    app.record_user_message("hi");
    let mut buf = buffer(30, 16);
    render_context_view(buf.area, &mut buf, &app);
    assert!(
        row(&buf, 9, 30).starts_with('~'),
        "vi-style filler past the end: {:?}",
        row(&buf, 9, 30)
    );
}

#[test]
fn context_view_max_scroll_is_total_lines_minus_the_body() {
    let app = context_fixture();
    let total = context_lines(&app, 40).len();
    let screen_h = 10u16;
    let body = (screen_h - TOOL_VIEW_TITLE_ROWS - TOOL_VIEW_FOOTER_ROWS) as usize;
    assert_eq!(
        context_view_max_scroll(&app, 40, screen_h),
        total.saturating_sub(body)
    );
}

#[test]
fn render_context_view_windows_by_the_debug_scroll() {
    let mut app = context_fixture();
    app.debug_scroll = 2; // past "system prompt:" and "  be nice"
    let mut buf = buffer(50, 14);
    render_context_view(buf.area, &mut buf, &app);
    assert!(
        !row(&buf, 1, 50).contains("system prompt"),
        "the scrolled-off tag is gone: {:?}",
        row(&buf, 1, 50)
    );
}
