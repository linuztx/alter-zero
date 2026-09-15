//! The `/resume` picker (`docs/resume.md`).

use super::*;
use crate::ui::theme::{RESUME_AGE_WIDTH, menu_dim_color, menu_selected_color, resume_selected_bg};

// --- restore_cursor_row (where the shell prompt resumes on exit) ---

#[test]
fn restore_cursor_row_lands_just_below_a_top_anchored_box() {
    // Box at rows 0..3 of a 24-row screen → prompt resumes on row 3, NOT at
    // the screen bottom (which would leave a 20-row blank gap).
    assert_eq!(restore_cursor_row(0, 3, 24), Some(3));
}

#[test]
fn resume_picker_titles_with_the_slash_tiled_resume_header() {
    let app = resume_app(&["hello"]);
    let mut buf = buffer(40, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    let title = row(&buf, 0, 40);
    assert!(title.starts_with("/ R E S U M E"), "{title:?}");
    // The tiling continues to the right edge (the transcript pager's
    // slash-tiled header pattern).
    assert!(title.trim_end().ends_with('/'), "{title:?}");
}

#[test]
fn resume_picker_shows_the_search_placeholder_then_the_query_echo() {
    let mut app = resume_app(&["hello"]);
    let mut buf = buffer(40, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    assert!(row(&buf, 2, 40).contains("Type to search"));
    app.resume_picker.as_mut().unwrap().query = "wrap".into();
    let mut buf = buffer(40, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    assert!(row(&buf, 2, 40).contains("Search: wrap"));
}

#[test]
fn resume_rows_show_marker_age_and_preview_with_the_selection_lit() {
    let mut app = resume_app(&["first message", "second message"]);
    app.resume_picker.as_mut().unwrap().selected = 1;
    let mut buf = buffer(40, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    let first = row(&buf, 4, 40);
    let second = row(&buf, 5, 40);
    assert!(first.starts_with("  5m ago"), "{first:?}");
    assert!(first.contains("first message"), "{first:?}");
    assert!(second.starts_with("❯ 5m ago"), "{second:?}");
    assert!(second.contains("second message"), "{second:?}");
    // The age column pads to a fixed width (codex's dense 12-col date) —
    // measured on the ASCII-marker row (`find` is byte-indexed; `❯` is
    // multi-byte).
    assert_eq!(first.find("first message"), Some(2 + RESUME_AGE_WIDTH));
    // The whole selected row lights up; the others dim — the palette's
    // selection-by-colour convention.
    assert_eq!(buf[(0, 5)].fg, menu_selected_color());
    assert_eq!(buf[(4, 5)].fg, menu_selected_color(), "age too");
    assert_eq!(buf[(4, 4)].fg, menu_dim_color(), "unselected rows dim");
}

#[test]
fn resume_picker_counts_the_selection_in_the_separator() {
    let mut app = resume_app(&["one", "two", "three"]);
    app.resume_picker.as_mut().unwrap().selected = 1;
    let mut buf = buffer(40, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    // Bottom chrome: separator + hints + blank ⇒ separator at h-3.
    let sep = row(&buf, 9, 40);
    assert!(sep.contains("─"), "{sep:?}");
    assert!(sep.contains(" 2/3 "), "{sep:?}");
    assert!(row(&buf, 10, 40).contains("enter resume"));
}

#[test]
fn resume_picker_shows_no_sessions_yet_when_nothing_is_saved() {
    let app = resume_app(&[]);
    let mut buf = buffer(40, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    assert!(row(&buf, 4, 40).contains("No sessions yet"));
}

#[test]
fn resume_picker_shows_no_results_for_a_query_matching_nothing() {
    let mut app = resume_app(&["hello"]);
    app.resume_picker.as_mut().unwrap().query = "zzz".into();
    let mut buf = buffer(40, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    assert!(row(&buf, 4, 40).contains("No results for your search"));
    let sep = row(&buf, 9, 40);
    assert!(
        !sep.contains('/') || !sep.contains("1/"),
        "no count: {sep:?}"
    );
}

#[test]
fn resume_rows_truncate_to_the_width() {
    let app = resume_app(&["a very long preview that cannot possibly fit"]);
    let mut buf = buffer(24, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    let line = row(&buf, 4, 24);
    assert!(line.starts_with("❯ 5m ago"), "{line:?}");
    assert!(!line.contains("possibly"), "truncated: {line:?}");
}

#[test]
fn resume_search_row_carries_the_filter_sort_toolbar_right_aligned() {
    let app = resume_app(&["hello"]);
    let mut buf = buffer(80, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    let search = row(&buf, 2, 80);
    assert!(search.contains("Type to search"), "{search:?}");
    // Codex's toolbar: active values bracketed, both tab pairs shown.
    assert!(search.contains("Filter: [Cwd] All"), "{search:?}");
    assert!(search.contains("Sort: [Updated] Created"), "{search:?}");
    assert!(search.trim_end().ends_with("Created"), "right-aligned");
}

#[test]
fn resume_toolbar_brackets_follow_the_toggles() {
    let mut app = resume_app(&["hello"]);
    {
        let picker = app.resume_picker.as_mut().unwrap();
        picker.filter = crate::app::ResumeFilter::All;
        picker.sort = crate::app::ResumeSort::Created;
    }
    let mut buf = buffer(80, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    let search = row(&buf, 2, 80);
    assert!(search.contains("Filter:  Cwd [All]"), "{search:?}");
    assert!(search.contains("Sort:  Updated [Created]"), "{search:?}");
}

#[test]
fn resume_toolbar_compacts_to_the_active_values_when_narrow() {
    let app = resume_app(&["hello"]);
    let mut buf = buffer(50, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    let search = row(&buf, 2, 50);
    // Codex's compact form: label + active value only.
    assert!(search.contains("Filter:[Cwd]"), "{search:?}");
    assert!(search.contains("Sort:[Updated]"), "{search:?}");
    // Too narrow for even the compact form: the toolbar drops, the
    // search line stays.
    let mut buf = buffer(30, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    let search = row(&buf, 2, 30);
    assert!(search.contains("Type to search"), "{search:?}");
    assert!(!search.contains("Filter:"), "{search:?}");
}

#[test]
fn resume_selected_row_gets_a_full_width_background_tint() {
    let mut app = resume_app(&["first message", "second message"]);
    app.resume_picker.as_mut().unwrap().selected = 1;
    let mut buf = buffer(40, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    // The tint spans the whole selected row — marker cell through the
    // padding past the text (codex's full-width background blend)…
    assert_eq!(buf[(0, 5)].bg, resume_selected_bg());
    assert_eq!(buf[(20, 5)].bg, resume_selected_bg());
    assert_eq!(buf[(39, 5)].bg, resume_selected_bg());
    // …and the unselected row keeps the plain background.
    assert_ne!(buf[(0, 4)].bg, resume_selected_bg());
}

#[test]
fn resume_rows_show_the_age_of_the_active_sort_key() {
    // Codex shows only the active sort key's timestamp per row: the
    // fixture's rows are updated 5m ago but created 2h ago.
    let mut app = resume_app(&["hello"]);
    let mut buf = buffer(40, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    assert!(
        row(&buf, 4, 40).contains("5m ago"),
        "Updated sort: mtime age"
    );
    app.resume_picker.as_mut().unwrap().sort = crate::app::ResumeSort::Created;
    let mut buf = buffer(40, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    assert!(
        row(&buf, 4, 40).contains("2h ago"),
        "Created sort: start age"
    );
}

#[test]
fn resume_picker_scrolls_to_keep_the_selection_visible() {
    let previews: Vec<String> = (0..30).map(|i| format!("message number {i}")).collect();
    let refs: Vec<&str> = previews.iter().map(String::as_str).collect();
    let mut app = resume_app(&refs);
    app.resume_picker.as_mut().unwrap().selected = 29;
    let mut buf = buffer(40, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    let body: String = (4..9).map(|y| row(&buf, y, 40)).collect();
    assert!(
        body.contains("message number 29"),
        "the window follows the selection: {body:?}"
    );
}

#[test]
fn resume_picker_keeps_the_selection_centered_like_the_model_list() {
    // A 12-row screen leaves the picker a 5-row body (rows 4..9). Mid-list
    // the highlight rides the middle row so the sessions above *and* below
    // it stay in view, and each ↓ scrolls the next one in — the `/model`
    // list's `centered_window`, not the palette's edge-pinned `menu_window`,
    // which parked the highlight on the bottom row and hid what came next.
    let previews: Vec<String> = (0..30).map(|i| format!("message number {i}")).collect();
    let refs: Vec<&str> = previews.iter().map(String::as_str).collect();
    let mut app = resume_app(&refs);
    for selected in [15, 16] {
        app.resume_picker.as_mut().unwrap().selected = selected;
        let mut buf = buffer(40, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        let body: Vec<String> = (4..9).map(|y| row(&buf, y, 40)).collect();
        assert!(
            body[2].starts_with("❯ "),
            "the selection rides the middle row: {body:?}"
        );
        for (offset, line) in body.iter().enumerate() {
            let expected = format!("message number {}", selected + offset - 2);
            assert!(
                line.trim_end().ends_with(&expected),
                "row {offset} shows session {expected:?}: {body:?}"
            );
        }
    }
}

// ===== /resume session picker (docs/resume.md) =====

/// An app with the picker open over one session per preview, all aged
/// `5m ago` (updated) / `2h ago` (created), recorded in the picker's own
/// cwd, at paths `s0`, `s1`, ….
fn resume_app(previews: &[&str]) -> App {
    let mut app = App::new();
    app.open_resume_picker(
        previews
            .iter()
            .enumerate()
            .map(|(i, preview)| crate::session::SessionSummary {
                path: std::path::PathBuf::from(format!("s{i}")),
                updated_secs: 300,
                created_secs: 7_200,
                cwd: "/repo".into(),
                preview: (*preview).into(),
            })
            .collect(),
        "/repo".into(),
    );
    app
}

#[test]
fn a_cut_session_preview_ends_with_an_ellipsis() {
    // The preview is how the user tells sessions apart in the dense one-row
    // picker — a cut one must say it was cut instead of ending mid-word.
    let app = resume_app(&["a very long preview that cannot possibly fit"]);
    let mut buf = buffer(24, 12);
    render_resume_picker(buf.area, &mut buf, &app);
    let line = row(&buf, 4, 24);
    assert!(line.trim_end().ends_with('…'), "{line:?}");
}
