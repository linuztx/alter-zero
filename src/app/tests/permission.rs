//! The inline tool-permission prompt (`docs/permissions.md`): opening stashes
//! the composer, the key map, the amend field, and the queue.

use super::*;
use crate::permission::{PermissionDecision, PermissionKind, PermissionMode, PermissionRequest};

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
fn the_arrows_move_the_selection_and_wrap_at_the_ends() {
    let mut app = App::new();
    app.open_permission(write_request("p1"));
    assert_eq!(app.permission().unwrap().selected, 0, "Yes is preselected");
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.permission().unwrap().selected,
        2,
        "Up from the top wraps to No"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.permission().unwrap().selected,
        0,
        "Down from the bottom wraps back to Yes"
    );
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.permission().unwrap().selected, 2);
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
fn a_bare_a_no_longer_answers_the_prompt() {
    // The old `a` shortcut is gone with the `(a)` hint: options are picked by
    // number or ↑/↓ + Enter (Claude Code's shape) — a bare letter was one
    // fat-finger away from a standing approval.
    let mut app = App::new();
    app.open_permission(bash_request("p1"));
    assert_eq!(app.on_key(key(KeyCode::Char('a'))), Action::None);
    assert!(app.permission().is_some(), "the prompt is still open");
}

#[test]
fn shift_tab_on_a_file_prompt_is_the_remember_option() {
    // Option 2 on a write/edit prompt IS the switch to edit mode, and
    // Shift+Tab is the mode toggle — inside a file prompt it selects that
    // option (the label advertises it: `… during this session (shift+tab)`).
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Manual));
    app.open_permission(write_request("p1"));
    let action = app.on_key(backtab());
    assert_eq!(
        action,
        Action::ResolvePermission {
            request: write_request("p1"),
            decision: PermissionDecision::ApproveAlways,
        }
    );
    assert!(app.permission().is_none(), "the prompt is answered");
}

#[test]
fn shift_tab_on_a_bash_prompt_toggles_the_mode_and_keeps_asking() {
    // The mode never covers commands, so the prompt stays open — Shift+Tab
    // just flips the posture (the loop mirrors it onto the gate and sweeps
    // any queued file requests the new mode covers). Both spellings bind:
    // legacy BackTab and the kitty protocol's Tab+SHIFT.
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Manual));
    app.open_permission(bash_request("p1"));
    let action = app.on_key(backtab());
    assert_eq!(action, Action::SetPermissionMode(PermissionMode::Edit));
    assert_eq!(app.permission_mode(), Some(PermissionMode::Edit));
    assert!(app.permission().is_some(), "the command still asks");
    let action = app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
    assert_eq!(action, Action::SetPermissionMode(PermissionMode::Auto));
    assert!(
        app.permission().is_some(),
        "still asking — Tab+SHIFT never amends"
    );
}

#[test]
fn shift_tab_cycles_the_permission_mode_from_the_composer() {
    // manual → edit → auto → master → manual, one step per press — the loop
    // persists each and shows the toast; the footer's right-edge mode tracks
    // the field. (Ctrl+A, the old binding, is the textarea's line-start now.)
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Manual));
    for expected in [
        PermissionMode::Edit,
        PermissionMode::Auto,
        PermissionMode::Master,
        PermissionMode::Manual,
    ] {
        let action = app.on_key(backtab());
        assert_eq!(action, Action::SetPermissionMode(expected));
        assert_eq!(app.permission_mode(), Some(expected));
    }
}

#[test]
fn shift_tab_with_permissions_disabled_explains_instead() {
    // ALTER_ZERO_PERMISSIONS=0 → no gate, nothing ever asks, no mode to
    // toggle. A silent no-op would read as a broken key; the toast says why
    // (the cycle_thinking pattern for a non-reasoning model).
    let mut app = App::new();
    assert_eq!(app.permission_mode(), None);
    assert_eq!(
        app.on_key(backtab()),
        Action::Toast("Tool permissions are disabled".to_string())
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
fn the_amend_field_takes_the_terminal_editing_shortcuts() {
    // edit_amend carries the readline set too (shared with the ask modal's
    // entries — docs/textarea.md): Ctrl+A/E jump the line, Alt+B steps a
    // word, Ctrl+W/U kill placeholder-atomically.
    let mut app = App::new();
    app.open_permission(write_request("p1"));
    app.on_key(key(KeyCode::Tab));
    type_text(&mut app, "use foo bar");
    app.on_key(ctrl('a'));
    assert_eq!(app.input.cursor(), 0, "ctrl+a = line start");
    app.on_key(ctrl('e'));
    assert_eq!(app.input.cursor(), 11, "ctrl+e = line end");
    app.on_key(alt(KeyCode::Char('b')));
    assert_eq!(app.input.cursor(), 8, "alt+b = start of \"bar\"");
    app.on_key(ctrl('e'));
    app.on_key(ctrl('w'));
    assert_eq!(app.input.text(), "use foo ", "ctrl+w killed \"bar\"");
    app.on_key(ctrl('u'));
    assert_eq!(app.input.text(), "", "ctrl+u killed to the line start");
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
    // The loop answers p1 with "always" (option 2), then sweeps whatever the
    // new rule covers — here p2 (the identical command), never p3.
    app.on_key(key(KeyCode::Char('2')));
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

// ===== what the model is told, and keeps being told (docs/permissions.md) =====

/// Drive the whole rejection round trip the way the loop does — the user's
/// Tab-amended answer through the real key path, resolved on a real gate, the
/// backend's `approve` seam consulted, and the resulting events folded into
/// `App` — then hand back the model's tool result and the recorded call.
///
/// This is the seam the bug lived in: everything below the gate was already
/// right, and everything above it kept only the cell text.
fn amended_rejection(feedback: &str) -> (String, ToolCall) {
    use crate::llm::approval::approve_call;
    use crate::llm::tools::ToolCallRequest;
    use crate::permission::PermissionGate;
    use crate::stream::{CancelToken, StreamEvent};

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let gate = PermissionGate::new();
    let cancel = CancelToken::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("hello.py");
    let call = ToolCallRequest {
        id: "c1".to_string(),
        name: "write".to_string(),
        arguments: serde_json::json!({ "path": path.to_str().unwrap(), "content": "x\n" })
            .to_string(),
    };
    // The backend thread blocks on the gate, exactly as `run_agent` does.
    let waiter = {
        let (gate, tx, cancel) = (gate.clone(), tx.clone(), cancel.clone());
        std::thread::spawn(move || {
            approve_call(
                Some(&gate),
                None,
                None,
                &crate::llm::hooks::NoHooks,
                false,
                &tx,
                &cancel,
                None,
                &call,
            )
        })
    };
    let request = loop {
        if let Ok(StreamEvent::Permission(request)) = rx.try_recv() {
            break request;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };

    // The user answers: Tab, type, Enter — the real key path.
    let mut app = App::new();
    app.begin_stream();
    app.open_permission(request);
    app.on_key(key(KeyCode::Tab));
    type_text(&mut app, feedback);
    let Action::ResolvePermission { request, decision } = app.on_key(key(KeyCode::Enter)) else {
        panic!("Enter in the amend field resolves the prompt");
    };
    gate.resolve(&request.id, decision);

    let crate::permission::Approval::Reject { display, result } = waiter.join().unwrap() else {
        panic!("an amended answer is a rejection");
    };
    // …and the loop folds the backend's events into the app.
    app.start_tool("Write", "hello.py");
    let tool = app
        .reject_tool(&display, &result)
        .expect("the refused call resolves");
    (result, tool)
}

#[test]
fn an_amended_rejection_records_exactly_what_the_model_was_told() {
    let (result, tool) = amended_rejection("use pathlib instead");
    // What the backend handed the model this round…
    assert!(
        result.contains("use pathlib instead"),
        "the live tool result carries the feedback: {result}"
    );
    // …is what the recorded call replays, verbatim.
    assert_eq!(tool.context_text(), result);
    assert_eq!(tool.status, ToolStatus::Failed);
}

#[test]
fn the_derived_context_replays_the_amended_instructions_on_later_turns() {
    // The bug: the amend feedback reached the model for one round and then
    // vanished — the next turn rebuilds the context from history, which kept
    // only `User rejected write to hello.py`. Ctrl+D showed the same gap.
    let (result, tool) = amended_rejection("use pathlib instead");
    let history = vec![HistoryItem::Tool(tool)];
    let ctx = crate::context::context_messages(&history);
    let replayed = ctx
        .iter()
        .find(|m| m.role == crate::context::ContextRole::Tool)
        .expect("the call is answered in the derived context");
    assert_eq!(
        replayed.text, result,
        "a later turn sees exactly what the live round sent"
    );
    assert!(
        replayed.text.contains("use pathlib instead"),
        "the user's instructions survive the turn: {}",
        replayed.text
    );
}
