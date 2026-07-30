//! The inline tool-permission prompt (`docs/permissions.md`): opening stashes
//! the composer, the key map, the amend field, and the queue.

use super::*;
use crate::permission::{PermissionDecision, PermissionKind, PermissionRequest};

fn request(id: &str, kind: PermissionKind, target: &str) -> PermissionRequest {
    PermissionRequest {
        id: id.to_string(),
        kind,
        target: target.to_string(),
        body: String::new(),
        detail: None,
        agent: None,
    }
}

fn write_request(id: &str) -> PermissionRequest {
    request(id, PermissionKind::Write, "hello.py")
}

fn bash_request(id: &str) -> PermissionRequest {
    request(id, PermissionKind::Bash, "python3 script.py")
}

/// Type `text` into the app one key at a time (the real composer path).
fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

#[test]
fn a_request_replaces_the_composer_and_gives_the_draft_back_afterwards() {
    // The whole point of the stash: a prompt that lands mid-sentence must not
    // eat what the user was typing.
    let mut app = App::new();
    type_text(&mut app, "half a thought");
    app.open_permission(write_request("p1"));
    assert!(app.permission().is_some());
    assert_eq!(
        app.input.text(),
        "",
        "the composer is cleared for the prompt"
    );
    app.on_key(key(KeyCode::Char('1')));
    assert!(app.permission().is_none());
    assert_eq!(app.input.text(), "half a thought", "the draft came back");
    assert_eq!(app.input.cursor(), "half a thought".len());
}

#[test]
fn a_shell_mode_draft_comes_back_in_shell_mode() {
    let mut app = App::new();
    type_text(&mut app, "!ls -la");
    assert!(app.shell_mode, "the `!` was absorbed");
    app.open_permission(bash_request("p1"));
    assert!(!app.shell_mode, "the prompt is not a shell composer");
    app.on_key(key(KeyCode::Char('3')));
    assert!(app.shell_mode, "the mode came back with the draft");
    assert_eq!(app.input.text(), "ls -la");
}

#[test]
fn the_prompt_owns_every_key_while_it_is_open() {
    let mut app = App::new();
    app.open_permission(write_request("p1"));
    // A printable key that isn't a shortcut neither types nor closes.
    app.on_key(key(KeyCode::Char('z')));
    assert!(app.permission().is_some());
    assert_eq!(app.input.text(), "");
    // …and the palette can't be opened from under it.
    app.on_key(key(KeyCode::Char('/')));
    assert!(app.command_menu.is_none());
}

#[test]
fn the_arrows_move_the_selection_and_clamp_at_the_ends() {
    let mut app = App::new();
    app.open_permission(write_request("p1"));
    assert_eq!(app.permission().unwrap().selected, 0, "Yes is preselected");
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.permission().unwrap().selected, 0, "clamped at the top");
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.permission().unwrap().selected,
        2,
        "clamped at the bottom"
    );
}

#[test]
fn enter_resolves_the_highlighted_option() {
    let mut app = App::new();
    app.open_permission(write_request("p1"));
    app.on_key(key(KeyCode::Down));
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Action::ResolvePermission {
            request: write_request("p1"),
            decision: PermissionDecision::ApproveAlways,
        }
    );
    assert!(app.permission().is_none(), "resolving closes the prompt");
}

#[test]
fn the_number_keys_pick_an_option_directly() {
    for (ch, expected) in [
        ('1', PermissionDecision::Approve),
        ('2', PermissionDecision::ApproveAlways),
        ('3', PermissionDecision::Deny(None)),
    ] {
        let mut app = App::new();
        app.open_permission(write_request("p1"));
        let action = app.on_key(key(KeyCode::Char(ch)));
        assert_eq!(
            action,
            Action::ResolvePermission {
                request: write_request("p1"),
                decision: expected,
            },
            "key {ch}"
        );
    }
}

#[test]
fn a_picks_the_remember_option_since_shift_tab_is_taken() {
    let mut app = App::new();
    app.open_permission(bash_request("p1"));
    let action = app.on_key(key(KeyCode::Char('a')));
    assert_eq!(
        action,
        Action::ResolvePermission {
            request: bash_request("p1"),
            decision: PermissionDecision::ApproveAlways,
        }
    );
}

#[test]
fn esc_cancels_the_whole_turn_not_just_the_call() {
    // Esc is the ordinary interrupt (docs/interrupt.md); the abandoned request
    // is released on the gate by the boundary so its thread never parks.
    let mut app = App::new();
    type_text(&mut app, "draft");
    app.open_permission(write_request("p1"));
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Interrupt);
    assert!(app.permission().is_none());
    assert_eq!(app.input.text(), "draft", "the draft still comes back");
    assert_eq!(app.take_abandoned_permissions(), vec!["p1".to_string()]);
    assert!(app.take_abandoned_permissions().is_empty(), "drained");
}

#[test]
fn a_discarded_queue_releases_every_waiting_thread() {
    let mut app = App::new();
    app.open_permission(write_request("p1"));
    app.open_permission(bash_request("p2"));
    app.clear_conversation();
    let mut ids = app.take_abandoned_permissions();
    ids.sort();
    assert_eq!(ids, vec!["p1".to_string(), "p2".to_string()]);
}

#[test]
fn ctrl_e_asks_for_an_explanation_but_only_for_a_command() {
    let mut app = App::new();
    app.open_permission(bash_request("p1"));
    let action = app.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
    assert_eq!(
        action,
        Action::ResolvePermission {
            request: bash_request("p1"),
            decision: PermissionDecision::Explain,
        }
    );
    // A file prompt has no such hint, so the key does nothing there.
    let mut app = App::new();
    app.open_permission(write_request("p2"));
    let action = app.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
    assert_eq!(action, Action::None);
    assert!(app.permission().is_some());
}

#[test]
fn tab_opens_an_empty_amend_field_that_rejects_with_the_typed_feedback() {
    let mut app = App::new();
    type_text(&mut app, "the draft");
    app.open_permission(write_request("p1"));
    app.on_key(key(KeyCode::Tab));
    assert!(app.permission().unwrap().amend, "the composer is back");
    assert_eq!(app.input.text(), "", "…and starts empty");
    type_text(&mut app, "use pathlib instead");
    assert_eq!(app.input.text(), "use pathlib instead");
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Action::ResolvePermission {
            request: write_request("p1"),
            decision: PermissionDecision::Deny(Some("use pathlib instead".to_string())),
        }
    );
    assert_eq!(app.input.text(), "the draft", "the draft still came back");
}

#[test]
fn esc_in_the_amend_field_goes_back_to_the_options() {
    let mut app = App::new();
    app.open_permission(write_request("p1"));
    app.on_key(key(KeyCode::Tab));
    type_text(&mut app, "wait");
    let action = app.on_key(key(KeyCode::Esc));
    assert_eq!(action, Action::None, "Esc backs out, it does not abort");
    let prompt = app.permission().expect("still open");
    assert!(!prompt.amend);
    assert_eq!(app.input.text(), "", "the abandoned feedback is dropped");
}

#[test]
fn an_empty_amend_field_rejects_with_no_feedback() {
    let mut app = App::new();
    app.open_permission(write_request("p1"));
    app.on_key(key(KeyCode::Tab));
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Action::ResolvePermission {
            request: write_request("p1"),
            decision: PermissionDecision::Deny(None),
        }
    );
}

#[test]
fn a_second_request_queues_and_opens_when_the_first_resolves() {
    // Two agents can ask at once; the second thread simply stays blocked.
    let mut app = App::new();
    type_text(&mut app, "draft");
    app.open_permission(write_request("p1"));
    app.open_permission(bash_request("p2"));
    assert_eq!(app.permission().unwrap().request.id, "p1");
    app.on_key(key(KeyCode::Char('1')));
    assert_eq!(
        app.permission().expect("the queued one opened").request.id,
        "p2"
    );
    assert_eq!(app.input.text(), "", "still the prompt's composer");
    app.on_key(key(KeyCode::Char('1')));
    assert!(app.permission().is_none());
    assert_eq!(app.input.text(), "draft", "the draft survives both");
}

#[test]
fn clearing_the_conversation_drops_the_prompt_and_its_queue() {
    let mut app = App::new();
    app.open_permission(write_request("p1"));
    app.open_permission(bash_request("p2"));
    app.clear_conversation();
    assert!(app.permission().is_none());
    assert!(app.pending_permissions().is_empty());
}

#[test]
fn ctrl_c_cancels_rather_than_quitting_mid_decision() {
    let mut app = App::new();
    app.open_permission(write_request("p1"));
    let action = app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(action, Action::Interrupt, "never Quit");
    assert!(app.permission().is_none());
}

#[test]
fn opening_a_prompt_dismisses_the_bands_it_covers() {
    let mut app = App::new();
    app.shortcuts_open = true;
    type_text(&mut app, "/he");
    assert!(app.command_menu.is_some());
    app.open_permission(write_request("p1"));
    assert!(app.command_menu.is_none());
    assert!(!app.shortcuts_open);
    assert!(app.file_search.is_none());
}

#[test]
fn the_ctrl_b_hint_is_not_advertised_while_a_prompt_is_open() {
    // The prompt owns every key, so Ctrl+B does nothing there — the delayed
    // `(ctrl+b to run in background)` hint hangs off `command_elapsed`, which
    // must therefore read as "nothing running" while a call waits on the user.
    let mut app = App::new();
    app.set_command_elapsed(Some(Duration::from_secs(30)));
    assert!(app.command_elapsed().is_some());
    app.open_permission(bash_request("p1"));
    assert_eq!(app.command_elapsed(), None);
    app.on_key(key(KeyCode::Char('1')));
    assert!(
        app.command_elapsed().is_some(),
        "the clock comes back with the answer"
    );
}

#[test]
fn a_standing_approval_sweeps_the_requests_it_now_covers() {
    // Parallel agents ask before any of them is answered, so "don't ask again"
    // must reach the ones already queued — otherwise three agents running the
    // same command ask three times *after* you said not to.
    let mut app = App::new();
    type_text(&mut app, "draft");
    app.open_permission(bash_request("p1"));
    app.open_permission(bash_request("p2"));
    app.open_permission(request("p3", PermissionKind::Bash, "rm -rf /"));
    // The loop answers p1 with "always", then sweeps whatever the new rule
    // covers — here p2 (the identical command), never p3.
    app.on_key(key(KeyCode::Char('a')));
    let swept = app.drain_covered_permissions(&|r| r.target == "python3 script.py");
    assert_eq!(swept, vec!["p2".to_string()]);
    assert_eq!(
        app.permission().expect("p3 still asks").request.id,
        "p3",
        "an uncovered request is untouched"
    );
    app.on_key(key(KeyCode::Char('3')));
    assert_eq!(app.input.text(), "draft", "the draft survives the sweep");
}

#[test]
fn the_sweep_walks_past_covered_requests_to_the_first_that_still_asks() {
    let mut app = App::new();
    app.open_permission(bash_request("p1"));
    app.open_permission(bash_request("p2"));
    app.open_permission(request("p3", PermissionKind::Bash, "rm -rf /"));
    // Everything matching is released, the open prompt included.
    let mut swept = app.drain_covered_permissions(&|r| r.target == "python3 script.py");
    swept.sort();
    assert_eq!(swept, vec!["p1".to_string(), "p2".to_string()]);
    assert_eq!(app.permission().expect("p3 opened").request.id, "p3");
}

#[test]
fn a_sweep_that_covers_everything_closes_the_prompt() {
    let mut app = App::new();
    app.open_permission(bash_request("p1"));
    app.open_permission(bash_request("p2"));
    let swept = app.drain_covered_permissions(&|_| true);
    assert_eq!(swept.len(), 2);
    assert!(app.permission().is_none());
    assert!(app.pending_permissions().is_empty());
}
