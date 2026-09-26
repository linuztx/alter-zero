//! The spinner tip's walk (`docs/tips.md`): when a turn draws a tip, which
//! one, and everything that keeps the row — and the cursor — still.

use super::*;
use crate::tasks::TaskStore;
use crate::tips::{TIP_DELAY, TIP_ROTATION, TIPS};

const SECOND: Duration = Duration::from_secs(1);

/// A fresh app whose tip walk the boundary has seeded at `next`.
fn seeded(next: usize) -> App {
    let mut app = App::new();
    app.seed_tips(next);
    app
}

/// Open a turn the way the loop does: the user's message, then the stream.
fn turn(app: &mut App) {
    app.record_user_message("go");
    app.begin_stream();
}

/// End the turn the way `StreamDone` does.
fn finish(app: &mut App) {
    app.finish_stream();
    app.end_turn(12);
}

#[test]
fn no_tip_shows_until_the_boundary_seeds_the_walk() {
    // The unit-test default: like the footer's session info, the walk is
    // injected at the boundary, so every test that runs a turn past three
    // seconds keeps the strip it always had.
    let mut app = App::new();
    turn(&mut app);
    app.set_status_times(TIP_DELAY * 4, None);
    assert_eq!(app.tip(), None);
    assert_eq!(app.tip_cursor(), None);
}

#[test]
fn no_tip_shows_before_the_delay() {
    let mut app = seeded(0);
    turn(&mut app);
    app.set_status_times(TIP_DELAY - Duration::from_millis(1), None);
    assert_eq!(app.tip(), None);
    assert_eq!(app.tip_cursor(), Some(0), "nothing was drawn");
}

#[test]
fn the_tip_appears_at_the_delay_and_moves_the_cursor_on() {
    let mut app = seeded(4);
    turn(&mut app);
    app.set_status_times(TIP_DELAY, None);
    assert_eq!(app.tip(), Some(TIPS[4]));
    assert_eq!(app.tip_cursor(), Some(5));
}

#[test]
fn a_turn_keeps_its_tip_until_the_rotation_draws_the_next() {
    let mut app = seeded(0);
    turn(&mut app);
    app.set_status_times(TIP_DELAY, None);
    assert_eq!(app.tip(), Some(TIPS[0]));
    // Every frame of the slot keeps the tip it drew, and draws nothing new.
    app.set_status_times(TIP_DELAY + SECOND * 90, None);
    assert_eq!(app.tip(), Some(TIPS[0]));
    assert_eq!(app.tip_cursor(), Some(1));
    // One rotation after it appeared, the next tip takes its place.
    app.set_status_times(TIP_DELAY + TIP_ROTATION, None);
    assert_eq!(app.tip(), Some(TIPS[1]));
    assert_eq!(app.tip_cursor(), Some(2));
}

#[test]
fn the_next_turn_opens_on_the_next_tip() {
    let mut app = seeded(0);
    turn(&mut app);
    app.set_status_times(TIP_DELAY, None);
    assert_eq!(app.tip(), Some(TIPS[0]));
    finish(&mut app);
    assert_eq!(app.tip(), None, "the row goes with the turn");
    turn(&mut app);
    assert_eq!(app.tip(), None, "a new turn waits out the delay again");
    app.set_status_times(TIP_DELAY, None);
    assert_eq!(app.tip(), Some(TIPS[1]));
}

#[test]
fn a_turn_that_ends_before_the_delay_leaves_the_cursor_alone() {
    // A string of quick answers must not burn through the catalog unseen.
    let mut app = seeded(3);
    turn(&mut app);
    app.set_status_times(SECOND, None);
    finish(&mut app);
    assert_eq!(app.tip_cursor(), Some(3));
    turn(&mut app);
    app.set_status_times(TIP_DELAY, None);
    assert_eq!(app.tip(), Some(TIPS[3]));
}

#[test]
fn the_cursor_wraps_at_the_end_of_the_catalog() {
    let last = TIPS.len() - 1;
    let mut app = seeded(last);
    turn(&mut app);
    app.set_status_times(TIP_DELAY, None);
    assert_eq!(app.tip(), Some(TIPS[last]));
    assert_eq!(app.tip_cursor(), Some(0));
}

#[test]
fn a_seeded_cursor_past_the_catalog_lands_on_a_tip() {
    // `tips.json` written against a longer catalog.
    let mut app = seeded(TIPS.len() + 2);
    turn(&mut app);
    app.set_status_times(TIP_DELAY, None);
    assert_eq!(app.tip(), Some(TIPS[2]));
    assert_eq!(app.tip_cursor(), Some(3));
}

#[test]
fn the_task_checklist_takes_the_tips_slot() {
    // The plan hangs off the spinner, not a tip: it is the more useful row.
    let mut app = seeded(0);
    turn(&mut app);
    let mut store = TaskStore::new();
    store
        .run_create(r#"{"subject": "Write tests", "description": "d"}"#)
        .expect("create succeeds");
    app.record_task_call(
        "TaskCreate",
        "Write tests",
        "{}",
        "Task #1 created successfully: Write tests",
        true,
        store,
    );
    app.set_status_times(TIP_DELAY * 4, None);
    assert_eq!(app.tip(), None);
    assert_eq!(app.tip_cursor(), Some(0), "an unseen tip is not spent");
}

#[test]
fn a_shell_turn_shows_no_tip() {
    // A `!` turn has no status line to hang one from.
    let mut app = seeded(0);
    app.begin_shell("sleep 10");
    app.set_status_times(TIP_DELAY * 4, None);
    assert_eq!(app.tip(), None);
    assert_eq!(app.tip_cursor(), Some(0));
}

#[test]
fn an_agent_session_view_shows_no_tip() {
    // Its status line is the subagent's.
    let mut app = seeded(0);
    turn(&mut app);
    app.start_agent_group(false, &agent_specs(false));
    app.open_agent_view("a1");
    app.set_status_times(TIP_DELAY * 4, None);
    assert_eq!(app.tip(), None);
    assert_eq!(app.tip_cursor(), Some(0));
}

#[test]
fn a_compact_turn_shows_a_tip() {
    // Summarizing can take a while, and its status line is an ordinary one.
    let mut app = seeded(6);
    app.begin_compact(false);
    app.set_status_times(TIP_DELAY, None);
    assert_eq!(app.tip(), Some(TIPS[6]));
}

#[test]
fn show_tips_off_hides_the_row_and_holds_the_cursor() {
    let mut app = seeded(0);
    app.settings_mut().tips = false;
    turn(&mut app);
    app.set_status_times(TIP_DELAY * 4, None);
    assert_eq!(app.tip(), None);
    assert_eq!(app.tip_cursor(), Some(0));
}

#[test]
fn a_hidden_tip_comes_back_as_the_same_tip() {
    let mut app = seeded(0);
    turn(&mut app);
    app.set_status_times(TIP_DELAY, None);
    assert_eq!(app.tip(), Some(TIPS[0]));
    // Turned off mid-turn: gone at once, before the next frame's clock.
    app.settings_mut().tips = false;
    assert_eq!(app.tip(), None);
    app.set_status_times(TIP_DELAY + SECOND * 10, None);
    // …and back on within the same slot: the tip it drew, not the next.
    app.settings_mut().tips = true;
    app.set_status_times(TIP_DELAY + SECOND * 20, None);
    assert_eq!(app.tip(), Some(TIPS[0]));
    assert_eq!(app.tip_cursor(), Some(1));
}

#[test]
fn a_slot_first_seen_late_draws_the_cursors_tip() {
    // Hidden for the turn's first few rotations, the first tip it shows is
    // still the next one in the walk — no tip is skipped unseen.
    let mut app = seeded(0);
    app.settings_mut().tips = false;
    turn(&mut app);
    app.set_status_times(TIP_DELAY + TIP_ROTATION * 2, None);
    app.settings_mut().tips = true;
    app.set_status_times(TIP_DELAY + TIP_ROTATION * 2 + SECOND, None);
    assert_eq!(app.tip(), Some(TIPS[0]));
    assert_eq!(app.tip_cursor(), Some(1));
}

#[test]
fn a_modal_prompt_holds_the_tip_back_until_it_closes() {
    // A permission prompt replaces the whole live region, strip and all: a
    // tip drawn under it would be spent without ever being seen.
    let mut app = seeded(0);
    turn(&mut app);
    app.open_permission(crate::permission::PermissionRequest {
        id: "p1".to_string(),
        kind: crate::permission::PermissionKind::Write,
        target: "hello.py".to_string(),
        body: String::new(),
        detail: None,
        agent: None,
        agent_id: None,
    });
    app.set_status_times(TIP_DELAY * 4, None);
    assert_eq!(app.tip(), None);
    assert_eq!(app.tip_cursor(), Some(0), "nothing was on screen to see");
    // Answered: the next frame draws the tip the prompt held back.
    app.on_key(key(KeyCode::Enter));
    assert!(!app.modal_open());
    app.set_status_times(TIP_DELAY * 4 + SECOND, None);
    assert_eq!(app.tip(), Some(TIPS[0]));
    assert_eq!(app.tip_cursor(), Some(1));
}

#[test]
fn an_overlay_holds_the_tip_back_until_the_conversation_returns() {
    // Ctrl+O and Ctrl+D cover the inline region on the alternate screen.
    let mut app = seeded(0);
    turn(&mut app);
    app.view = View::ToolOutput;
    app.set_status_times(TIP_DELAY * 4, None);
    assert_eq!(app.tip(), None);
    assert_eq!(app.tip_cursor(), Some(0));
    app.view = View::Conversation;
    app.set_status_times(TIP_DELAY * 4 + SECOND, None);
    assert_eq!(app.tip(), Some(TIPS[0]));
    assert_eq!(app.tip_cursor(), Some(1));
}
