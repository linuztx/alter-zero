//! The ↓ background-shell manager band (`docs/background.md`).

use super::*;
use crate::ui::theme::{TOOL_FAIL_COLOR, TOOL_OK_COLOR};

#[test]
fn background_notice_lines_render_the_headline_with_outcome_colours() {
    let ok = background_notice_lines(&bg_notice(Some(0), false), 80);
    assert_eq!(
        plain(&ok[0]),
        "● Background command \"Ping x.com 200 times\" completed (exit code 0)"
    );
    assert_eq!(ok[0].spans[0].style.fg, Some(TOOL_OK_COLOR), "green bullet");
    let failed = background_notice_lines(&bg_notice(Some(2), false), 80);
    assert_eq!(
        failed[0].spans[0].style.fg,
        Some(TOOL_FAIL_COLOR),
        "red bullet on failure"
    );
    let stopped = background_notice_lines(&bg_notice(None, true), 80);
    assert!(plain(&stopped[0]).contains("was stopped by the user"));
    assert!(
        !plain(&ok[0]).contains("tail"),
        "the output tail is context-only, never rendered"
    );
}

#[test]
fn the_manager_empty_state_says_no_tasks_running() {
    let mut app = App::new();
    app.bg_started("bash_1", "ping x.com", None, true, None);
    app.bg_exited("bash_1", Some(0), false);
    app.open_background_view();
    let texts: Vec<String> = background_view_lines(&app, 60)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(texts[2], "  Background");
    assert_eq!(texts[4], "  No tasks currently running");
    assert_eq!(texts[6], "  ↑/↓ to select · Enter to view · Esc to close");
    assert!(
        !texts.iter().any(|l| l.contains("x to stop")),
        "nothing to stop in the empty state: {texts:?}"
    );
}

#[test]
fn details_of_a_vanished_shell_fall_back_to_the_list() {
    let mut app = App::new();
    app.bg_started("bash_1", "ping x.com", None, true, None);
    // A Details view pointing at an unknown id renders the list instead
    // (bg_exited retargets, so this is the defensive path).
    app.background_view = Some(BackgroundView::Details {
        id: "ghost".to_string(),
    });
    let texts: Vec<String> = background_view_lines(&app, 60).iter().map(plain).collect();
    assert!(texts.iter().any(|l| l.contains("Background")));
    assert!(!texts.iter().any(|l| l.contains("Shell details")));
}
