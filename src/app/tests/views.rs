//! The Ctrl+O transcript pager and Ctrl+D context-debug view state.

use super::*;
use crate::app::views::TOOL_VIEW_PAGE;

#[test]
fn question_mark_is_ignored_in_the_tool_view() {
    let mut app = App::new();
    app.on_key(ctrl('o'));
    app.on_key(key(KeyCode::Char('?')));
    assert!(!app.shortcuts_open, "the tool view has no composer or band");
}

#[test]
fn ctrl_o_dismisses_the_band_like_any_other_key() {
    // The band's rule is "any key but Esc closes it, then acts normally"
    // (docs/shortcuts.md) — the global Ctrl+O arm is no exception, so the
    // band must not re-show under the box after the overlay round-trip.
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('?')));
    assert!(app.shortcuts_open);
    app.on_key(ctrl('o')); // into the overlay
    assert!(!app.shortcuts_open, "opening the overlay closes the band");
    app.on_key(ctrl('o')); // and back
    assert!(
        !app.shortcuts_open,
        "the band stays closed after the round-trip"
    );
}

#[test]
fn ctrl_o_toggles_into_and_out_of_the_tool_view() {
    let mut app = App::new();
    assert_eq!(app.view, View::Conversation);
    assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
    assert_eq!(app.view, View::ToolOutput);
    assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
    assert_eq!(app.view, View::Conversation);
}

// ===== Ctrl+D context-debug view (docs/context.md) =====

#[test]
fn ctrl_d_toggles_into_and_out_of_the_context_debug_view() {
    let mut app = App::new();
    assert_eq!(app.view, View::Conversation);
    assert_eq!(app.on_key(ctrl('d')), Action::ToggleContextDebug);
    assert_eq!(app.view, View::ContextDebug);
    assert_eq!(app.on_key(ctrl('d')), Action::ToggleContextDebug);
    assert_eq!(app.view, View::Conversation);
}

#[test]
fn q_and_esc_close_the_context_debug_view() {
    for code in [KeyCode::Char('q'), KeyCode::Esc] {
        let mut app = App::new();
        app.on_key(ctrl('d'));
        assert_eq!(app.on_key(key(code)), Action::ToggleContextDebug);
        assert_eq!(app.view, View::Conversation, "{code:?} closes");
    }
}

#[test]
fn ctrl_d_is_inert_in_the_other_overlays_and_ctrl_o_in_it() {
    // All three full-screen views share the alternate screen, so they
    // never stack: Ctrl+D does nothing under the tool view or the resume
    // picker, and Ctrl+O does nothing under the context-debug view.
    let mut app = App::new();
    app.on_key(ctrl('o'));
    assert_eq!(app.on_key(ctrl('d')), Action::None);
    assert_eq!(app.view, View::ToolOutput);

    let mut app = App::new();
    app.open_resume_picker(Vec::new(), String::new());
    assert_eq!(app.on_key(ctrl('d')), Action::None);
    assert_eq!(app.view, View::ResumePicker);

    let mut app = App::new();
    app.on_key(ctrl('d'));
    assert_eq!(app.on_key(ctrl('o')), Action::None);
    assert_eq!(app.view, View::ContextDebug);
}

#[test]
fn ctrl_d_works_mid_turn_like_ctrl_o() {
    // The conversation keeps streaming underneath; the debug view only
    // reads state, so it opens even while a turn is active.
    let mut app = App::new();
    app.record_user_message("hi");
    app.begin_stream();
    assert_eq!(app.on_key(ctrl('d')), Action::ToggleContextDebug);
    assert_eq!(app.view, View::ContextDebug);
}

#[test]
fn scroll_keys_move_the_context_debug_offset() {
    let mut app = App::new();
    app.on_key(ctrl('d'));
    assert!(app.debug_follow, "opens pinned to the bottom");
    app.on_key(key(KeyCode::Up));
    assert!(!app.debug_follow, "scrolling up unpins");
    app.debug_scroll = 5;
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.debug_scroll, 4);
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.debug_scroll, 5);
    app.on_key(key(KeyCode::PageUp));
    assert_eq!(app.debug_scroll, 0);
    app.on_key(key(KeyCode::PageDown));
    assert_eq!(app.debug_scroll, TOOL_VIEW_PAGE);
    app.on_key(key(KeyCode::Home));
    assert_eq!(app.debug_scroll, 0);
    app.on_key(key(KeyCode::End));
    assert_eq!(app.debug_scroll, usize::MAX, "End settles at the draw");
}

#[test]
fn settle_debug_scroll_caps_a_stale_offset_and_repins_follow() {
    let mut app = App::new();
    app.on_key(ctrl('d'));
    app.debug_follow = false;
    app.debug_scroll = 100;
    app.settle_debug_scroll(7);
    assert_eq!(app.debug_scroll, 7);
    assert!(app.debug_follow, "hitting the end re-engages follow");
    app.debug_follow = false;
    app.debug_scroll = 3;
    app.settle_debug_scroll(7);
    assert_eq!(app.debug_scroll, 3, "an in-range offset is kept");
}

#[test]
fn ctrl_o_opens_the_tool_view_even_while_streaming() {
    let mut app = App::new();
    app.begin_stream();
    app.on_key(ctrl('o'));
    assert_eq!(app.view, View::ToolOutput, "the overlay opens mid-stream");
    assert!(
        app.is_streaming(),
        "and the stream keeps running underneath"
    );
}

#[test]
fn esc_in_the_tool_view_returns_to_the_conversation_not_quit() {
    let mut app = App::new();
    app.on_key(ctrl('o'));
    assert_eq!(app.view, View::ToolOutput);
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::ToggleToolView);
    assert_eq!(app.view, View::Conversation, "esc closes the overlay");
}

#[test]
fn scroll_keys_move_the_tool_view_offset() {
    let mut app = App::new();
    app.on_key(ctrl('o'));
    app.tool_scroll = 5;
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.tool_scroll, 4, "up scrolls toward the top");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.tool_scroll, 5, "down scrolls toward the bottom");
    app.on_key(key(KeyCode::PageDown));
    assert_eq!(app.tool_scroll, 5 + TOOL_VIEW_PAGE);
    app.on_key(key(KeyCode::Up)); // saturating, never underflows below 0
    app.on_key(key(KeyCode::PageUp));
    assert_eq!(app.tool_scroll, 5 + TOOL_VIEW_PAGE - 1 - TOOL_VIEW_PAGE);
}

#[test]
fn home_and_end_jump_the_tool_view_to_the_edges() {
    // codex's pager jump keys: Home to the very top (leaving tail-follow),
    // End back to the bottom (re-engaging it).
    let mut app = App::new();
    app.on_key(ctrl('o'));
    app.settle_tool_scroll(20); // pinned to the bottom
    app.on_key(key(KeyCode::Home));
    assert!(!app.tool_follow, "home stops tailing");
    app.settle_tool_scroll(20);
    assert_eq!(app.tool_scroll, 0, "home jumps to the top");

    app.on_key(key(KeyCode::End));
    app.settle_tool_scroll(20);
    assert!(app.tool_follow, "end re-engages tailing");
    assert_eq!(app.tool_scroll, 20, "end jumps to the bottom");
}

#[test]
fn q_closes_the_tool_view_like_esc() {
    // codex's pager close key: q quits the overlay, back to the chat.
    let mut app = App::new();
    app.on_key(ctrl('o'));
    assert_eq!(app.on_key(key(KeyCode::Char('q'))), Action::ToggleToolView);
    assert_eq!(app.view, View::Conversation);
}

#[test]
fn typing_is_ignored_in_the_tool_view() {
    let mut app = App::new();
    app.on_key(ctrl('o'));
    app.on_key(key(KeyCode::Char('x')));
    assert_eq!(app.input.text(), "", "the read-only viewer swallows typing");
}

#[test]
fn opening_the_tool_view_follows_the_bottom() {
    let mut app = App::new();
    app.on_key(ctrl('o'));
    assert!(app.tool_follow, "the view opens pinned to the bottom");
    // Settling against any screen pins the offset to the last line.
    app.settle_tool_scroll(12);
    assert_eq!(app.tool_scroll, 12, "opens on the latest content");
}

#[test]
fn re_opening_the_tool_view_follows_the_bottom_again() {
    let mut app = App::new();
    app.on_key(ctrl('o'));
    app.tool_scroll = 7;
    app.tool_follow = false; // pretend the user scrolled up
    app.on_key(ctrl('o')); // leave
    app.on_key(ctrl('o')); // re-enter
    assert!(app.tool_follow, "re-opening tails the bottom again");
    assert_eq!(app.tool_scroll, 0);
}

#[test]
fn scrolling_up_leaves_the_bottom_then_reaching_it_re_engages() {
    let mut app = App::new();
    app.on_key(ctrl('o')); // follow
    app.settle_tool_scroll(20); // pinned to the bottom (20)
    assert_eq!(app.tool_scroll, 20);

    app.on_key(key(KeyCode::Up)); // read back
    assert!(!app.tool_follow, "scrolling up stops tailing");
    app.settle_tool_scroll(20);
    assert_eq!(app.tool_scroll, 19, "moved one off the bottom, stays put");

    app.on_key(key(KeyCode::Down)); // back down to the bottom
    app.settle_tool_scroll(20);
    assert!(app.tool_follow, "reaching the bottom re-engages tailing");
    assert_eq!(app.tool_scroll, 20);
}

#[test]
fn settle_tool_scroll_caps_a_stale_offset() {
    let mut app = App::new();
    // Not following, but the stored offset is past the end → snaps to the
    // bottom (and resumes tailing, since it was at/past the last line).
    app.tool_scroll = 100;
    app.settle_tool_scroll(12);
    assert_eq!(app.tool_scroll, 12);
}

#[test]
fn ctrl_o_during_a_search_cancels_it_and_opens_the_overlay() {
    let mut app = searchable_app(&["git status"]);
    app.input = TextArea::from_text("a draft");
    app.on_key(ctrl('r'));
    type_query(&mut app, "git");
    assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
    assert_eq!(app.view, View::ToolOutput);
    assert!(
        app.history_search.is_none(),
        "no search leaks into the overlay"
    );
    assert_eq!(app.input.text(), "a draft");
}

#[test]
fn ctrl_o_cancels_the_preview_too() {
    let mut app = App::new();
    exchange(&mut app, "first", "a");
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
    assert_eq!(app.view, View::Conversation);
    assert_eq!(app.history.len(), 3);
    assert_eq!(app.backtrack, Backtrack::default());
}

#[test]
fn scroll_keys_keep_scrolling_while_previewing() {
    let mut app = App::new();
    exchange(&mut app, "one", "a");
    exchange(&mut app, "two", "b");
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc));
    app.tool_scroll = 5;
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.tool_scroll, 4, "↑ still scrolls the transcript");
    assert_eq!(app.backtrack.selected, Some(1), "the highlight stays put");
}

#[test]
fn the_preview_requests_a_scroll_to_the_highlight_once_per_step() {
    let mut app = App::new();
    exchange(&mut app, "one", "a");
    exchange(&mut app, "two", "b");
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc));
    assert!(app.take_backtrack_scroll(), "opening requests a scroll");
    assert!(!app.take_backtrack_scroll(), "…consumed by the draw");
    app.on_key(key(KeyCode::Esc)); // step older
    assert!(app.take_backtrack_scroll(), "stepping requests another");
}

#[test]
fn ctrl_o_is_inert_while_the_picker_is_up() {
    // Both overlays share the alternate screen — the transcript view must
    // not open on top of the picker.
    let mut app = picker_app(&[("a", "hello")]);
    assert_eq!(app.on_key(ctrl('o')), Action::None);
    assert_eq!(app.view, View::ResumePicker);
}

// ===== Ctrl+G classifier-context view (docs/permissions.md) =====

#[test]
fn ctrl_g_toggles_into_and_out_of_the_classifier_context_view() {
    let mut app = App::new();
    assert_eq!(app.view, View::Conversation);
    assert_eq!(app.on_key(ctrl('g')), Action::ToggleClassifierContext);
    assert_eq!(app.view, View::ClassifierContext);
    assert_eq!(app.on_key(ctrl('g')), Action::ToggleClassifierContext);
    assert_eq!(app.view, View::Conversation);
}

#[test]
fn q_and_esc_close_the_classifier_context_view() {
    for code in [KeyCode::Char('q'), KeyCode::Esc] {
        let mut app = App::new();
        app.on_key(ctrl('g'));
        assert_eq!(app.on_key(key(code)), Action::ToggleClassifierContext);
        assert_eq!(app.view, View::Conversation, "{code:?} closes");
    }
}

#[test]
fn the_classifier_view_never_stacks_with_the_other_full_screen_views() {
    // All four share the alternate screen, so none of them opens on top of
    // another — in either direction.
    for opener in [ctrl('o'), ctrl('d')] {
        let mut app = App::new();
        app.on_key(opener);
        let before = app.view;
        assert_eq!(app.on_key(ctrl('g')), Action::None);
        assert_eq!(app.view, before, "ctrl+g must not stack on {before:?}");
    }
    let mut app = App::new();
    app.open_resume_picker(Vec::new(), String::new());
    assert_eq!(app.on_key(ctrl('g')), Action::None);
    assert_eq!(app.view, View::ResumePicker);

    for other in [ctrl('o'), ctrl('d')] {
        let mut app = App::new();
        app.on_key(ctrl('g'));
        assert_eq!(app.on_key(other), Action::None);
        assert_eq!(app.view, View::ClassifierContext);
    }
}

#[test]
fn ctrl_g_works_mid_turn() {
    // The whole point is watching the log grow while the agent works.
    let mut app = App::new();
    app.record_user_message("hi");
    app.begin_stream();
    assert_eq!(app.on_key(ctrl('g')), Action::ToggleClassifierContext);
    assert_eq!(app.view, View::ClassifierContext);
}

#[test]
fn scroll_keys_move_the_classifier_context_offset() {
    let mut app = App::new();
    app.on_key(ctrl('g'));
    assert!(app.classifier_follow, "opens pinned to the bottom");
    app.on_key(key(KeyCode::Up));
    assert!(!app.classifier_follow, "scrolling up drops tail-follow");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.classifier_scroll, 1);
    app.on_key(key(KeyCode::PageDown));
    assert_eq!(app.classifier_scroll, 1 + TOOL_VIEW_PAGE);
    app.on_key(key(KeyCode::Home));
    assert_eq!(app.classifier_scroll, 0);
    app.on_key(key(KeyCode::End));
    app.settle_classifier_scroll(4);
    assert_eq!(app.classifier_scroll, 4, "End pins to the bottom");
    assert!(app.classifier_follow, "and re-engages tail-follow");
}

#[test]
fn the_boundary_injects_the_rendered_classifier_context() {
    // The block is built on the backend thread, so the App only ever holds
    // what the boundary pushed in — the system-prompt/clock pattern.
    let mut app = App::new();
    assert_eq!(app.classifier_context(), None);
    app.set_classifier_context(Some("## Task context".to_string()));
    assert_eq!(app.classifier_context(), Some("## Task context"));
}
