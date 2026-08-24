//! The Esc-Esc backtrack (`docs/backtrack.md`).

use super::*;

#[test]
fn esc_with_a_previous_user_message_primes_instead_of_quitting() {
    let mut app = App::new();
    exchange(&mut app, "hello", "hi");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(app.backtrack.primed, "first Esc arms the gesture (codex)");
}

#[test]
fn esc_with_a_draft_neither_primes_nor_quits() {
    // Priming requires an empty composer (codex's composer_is_empty
    // guard) — and idle Esc with a draft is a no-op like codex's, never
    // a quit that throws typed work away (Ctrl+C is the composer-clear).
    let mut app = App::new();
    exchange(&mut app, "hello", "hi");
    app.input = TextArea::from_text("draft");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(!app.backtrack.primed);
    assert_eq!(app.input.text(), "draft", "the draft is untouched");
}

#[test]
fn esc_with_a_draft_and_no_history_does_not_quit_either() {
    // The no-op holds regardless of whether a backtrack target exists:
    // quitting on Esc requires an *empty* composer (codex never quits on
    // Esc at all; ours only does with nothing typed and nothing to edit).
    let mut app = App::new();
    app.input = TextArea::from_text("draft");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(app.input.text(), "draft");
}

#[test]
fn shell_headers_are_not_backtrack_targets() {
    // A `!` command is user-typed but not a user *prompt* (codex only
    // targets user messages); with nothing else in history Esc still quits.
    let mut app = App::new();
    app.begin_shell("pwd");
    app.start_tool("shell", "pwd", None);
    app.end_tool("/home", true);
    app.end_turn(1);
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Quit);
}

#[test]
fn applying_a_backtrack_scroll_disengages_tail_follow() {
    // Previewing the *last* message can leave the scroll at max, which
    // re-engages tail-follow (`settle_tool_scroll`); a step older must
    // not have its scroll-into-view yanked back to the bottom by it.
    let mut app = App::new();
    exchange(&mut app, "one", "a");
    exchange(&mut app, "two", "b");
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc));
    app.tool_follow = true; // the open landed at the bottom
    app.apply_backtrack_scroll(3);
    app.settle_tool_scroll(10);
    assert!(!app.tool_follow, "the follow pin is released");
    assert_eq!(app.tool_scroll, 3, "the highlight's scroll survives");
}

#[test]
fn interrupt_undo_stops_at_a_backtrack_rewind_boundary() {
    // A batch records two user messages; backtracking to the second
    // leaves the first as the history tail. A new submission undone by an
    // early Esc must not pull that older message back with it.
    let mut app = App::new();
    app.record_user_message("first");
    app.record_user_message("second");
    app.on_key(key(KeyCode::Esc)); // arm
    app.on_key(key(KeyCode::Esc)); // preview at the newest ("second")
    app.on_key(key(KeyCode::Enter)); // rewind: history = [first]
    assert_eq!(app.input.text(), "second");
    app.record_user_message("new");
    app.begin_stream();
    assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
    assert_eq!(app.input.text(), "new");
    assert_eq!(roles(&app), vec![Role::User]);
    assert_eq!(message_at(&app, 0).text, "first");
}

#[test]
fn overlay_esc_backtracks_exactly_when_esc_would_not_quit() {
    // The one predicate the Ctrl+O key handler's Esc arm AND the overlay's
    // closing hint row share (`ui::render_tool_view`), so what the hint
    // promises and what the key does can never drift: Esc begins the
    // edit-previous preview only when idle, outside an agent session view,
    // with a previous user message to edit — otherwise it keeps closing the
    // overlay (and the hint keeps listing it as a quit key).
    let mut app = App::new();
    assert!(!app.overlay_esc_backtracks(), "nothing to edit yet");
    app.record_user_message("hi");
    app.begin_stream();
    assert!(!app.overlay_esc_backtracks(), "a running turn wins");
    app.finish_stream();
    app.end_turn(1);
    assert!(app.overlay_esc_backtracks(), "idle with a target");
    app.agent_view = Some("a1".to_string());
    assert!(
        !app.overlay_esc_backtracks(),
        "an agent session view has no backtrack"
    );
}
