//! Collapsed read/search groups (`docs/tool-groups.md`): which calls join
//! one, how far a group runs through the history, what it counts, and how
//! the open group is held and closed.

use super::*;
use crate::read_only::ReadKind;
use crate::settings::SettingKey;

/// A call as its record keeps it once resolved.
fn resolved(name: &str, args: &str, ok: bool) -> ToolCall {
    let mut app = App::new();
    app.start_tool(name, args, None);
    app.end_tool("out", ok).expect("a running call")
}

fn tool(name: &str, args: &str) -> HistoryItem {
    HistoryItem::Tool(resolved(name, args, true))
}

fn assistant(text: &str) -> HistoryItem {
    HistoryItem::Message(Message {
        role: Role::Assistant,
        text: text.to_string(),
        timestamp: String::new(),
        images: Vec::new(),
    })
}

fn thought() -> HistoryItem {
    HistoryItem::Reasoning(Reasoning {
        text: "let me look further".to_string(),
        secs: 2,
        tokens: 10,
        timestamp: String::new(),
    })
}

#[test]
fn a_read_and_a_command_that_only_reads_join_a_group() {
    assert_eq!(
        group_kind(&resolved("Read", "src/main.rs", true)),
        Some(ReadKind::Read)
    );
    assert_eq!(
        group_kind(&resolved("Bash", "ls -la", true)),
        Some(ReadKind::List)
    );
    assert_eq!(
        group_kind(&resolved("Bash", "grep -rn todo src", true)),
        Some(ReadKind::Search)
    );
    assert_eq!(
        group_kind(&resolved("Bash", "cat about.py", true)),
        Some(ReadKind::Read)
    );
}

#[test]
fn every_other_call_keeps_its_own_cell() {
    for (name, args) in [
        ("Bash", "rm -rf build"),
        ("Bash", "python3 about.py"),
        ("Write", "a.py"),
        ("Edit", "a.py"),
        ("Agent", "explore the repo"),
        ("BashWait", "b7x2k9m1q"),
        ("Deepwiki - ask_question (MCP)", "{}"),
    ] {
        assert_eq!(
            group_kind(&resolved(name, args, true)),
            None,
            "{name}({args})"
        );
    }
}

#[test]
fn an_image_read_keeps_its_own_cell_so_its_picture_still_draws() {
    // An image `read` is drawn as a picture under its cell (docs/images.md);
    // folded into a group's one line it would draw nothing at all.
    for path in ["shot.png", "/tmp/Photo.JPG", "logo.webp", "anim.gif"] {
        assert_eq!(group_kind(&resolved("Read", path, true)), None, "{path}");
    }
    // A text file is still a read, whatever its name resembles.
    assert_eq!(
        group_kind(&resolved("Read", "png-notes.md", true)),
        Some(ReadKind::Read)
    );
}

#[test]
fn a_background_launch_never_joins_a_group() {
    // Its cell is the `Running in the background (↓ to manage)` row — a
    // process left running, not a read that finished.
    for arguments in [
        r#"{"command":"tail -f server.log","wait":0}"#,
        r#"{"command":"ls -R /","run_in_background":true}"#,
    ] {
        let mut app = App::new();
        let command = serde_json_command(arguments);
        app.start_tool("Bash", &command, Some(arguments));
        let call = app.current_tool().unwrap().clone();
        assert_eq!(group_kind(&call), None, "{arguments}");
    }
    assert_eq!(group_kind(&resolved("Bash", "ls &", true)), None);
}

/// The `command` of a bash call's arguments.
fn serde_json_command(arguments: &str) -> String {
    serde_json::from_str::<serde_json::Value>(arguments).unwrap()["command"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_shell_command_the_user_ran_is_never_grouped() {
    let mut app = App::new();
    app.begin_shell("ls -la");
    let call = app.current_tool().unwrap().clone();
    assert_eq!(group_kind(&call), None);
}

#[test]
fn a_read_that_ran_stays_in_its_group_whatever_its_exit() {
    // A search that found nothing exits 1; a missing file fails the read.
    // Either way the read happened, and the model reads the error — the
    // reference absorbs it, and Ctrl+O keeps the output.
    assert!(is_group_member(&resolved(
        "Bash",
        "grep -rn nope src",
        false
    )));
    assert!(is_group_member(&resolved("Read", "missing.rs", false)));
}

#[test]
fn a_refused_interrupted_or_backgrounded_read_keeps_its_own_cell() {
    let refused = {
        let mut app = App::new();
        app.start_tool("Bash", "ls -la", None);
        app.reject_tool("User rejected ls -la", "The user doesn't want to proceed")
            .unwrap()
    };
    assert!(!is_group_member(&refused), "a refusal is the user's record");

    let interrupted = {
        let mut app = App::new();
        app.start_tool("Read", "a.rs", None);
        app.end_tool(INTERRUPT_TOOL_OUTPUT, false).unwrap()
    };
    assert!(!is_group_member(&interrupted));

    let errored = {
        let mut app = App::new();
        app.start_tool("Read", "a.rs", None);
        app.end_tool(ERROR_TOOL_OUTPUT, false).unwrap()
    };
    assert!(!is_group_member(&errored));

    let never_run = {
        let mut app = App::new();
        app.start_tool("Read", "a.rs", None);
        app.end_tool(crate::llm::agent::TOOL_LIMIT_OUTPUT, false)
            .unwrap()
    };
    assert!(!is_group_member(&never_run), "it never ran");

    let backgrounded = {
        let mut app = App::new();
        app.start_tool("Bash", "grep -rn x /", None);
        app.background_tool("launched").unwrap()
    };
    assert!(!is_group_member(&backgrounded));
}

#[test]
fn a_group_runs_through_task_calls_hook_notes_and_thoughts() {
    let task = HistoryItem::TaskCall(crate::app::TaskCallRecord {
        name: "TaskUpdate".to_string(),
        args: "1".to_string(),
        arguments: String::new(),
        output: String::new(),
        ok: true,
        timestamp: String::new(),
        tasks: crate::tasks::TaskStore::default(),
        batch: None,
        call_id: None,
        position: None,
    });
    let items = vec![
        tool("Read", "a.rs"),
        task,
        thought(),
        tool("Bash", "ls -la"),
        thought(),
        assistant("Here is what I found."),
        tool("Read", "b.rs"),
    ];
    assert_eq!(
        group_run(&items),
        Some(5),
        "up to the reply, trailing thought included"
    );
    assert_eq!(group_run(&items[5..]), None, "a reply is no group");
    assert_eq!(group_run(&items[6..]), Some(1), "a lone read is a group");
}

#[test]
fn a_call_that_is_not_a_read_ends_the_group() {
    let items = vec![
        tool("Read", "a.rs"),
        tool("Edit", "a.rs"),
        tool("Read", "a.rs"),
    ];
    assert_eq!(group_run(&items), Some(1));
    assert_eq!(group_run(&items[1..]), None);
}

#[test]
fn a_group_counts_each_file_once_and_every_search_and_listing() {
    let calls = [
        resolved("Read", "a.py", true),
        resolved("Read", "a.py", true),
        resolved("Bash", "cat b.py", true),
        resolved("Bash", "grep -rn x src", true),
        resolved("Bash", "rg y", true),
        resolved("Bash", "ls -la", true),
    ];
    let counts = group_counts(calls.iter());
    assert_eq!(
        counts,
        GroupCounts {
            searches: 2,
            reads: 2,
            lists: 1
        }
    );
}

// ===== the open group on App =====

#[test]
fn a_resolved_read_is_held_in_the_open_group() {
    let mut app = App::new();
    app.start_tool("Read", "a.rs", None);
    app.end_tool("1 a", true);
    assert_eq!(app.place_resolved_tool(), ToolPlacement::Held);
    assert_eq!(app.tool_group_range(), Some(0..1));
    app.start_tool("Bash", "ls -la", None);
    app.end_tool("Exit code: 0\na.rs", true);
    assert_eq!(app.place_resolved_tool(), ToolPlacement::Held);
    assert_eq!(app.tool_group_range(), Some(0..2), "one group, two calls");
}

#[test]
fn a_call_that_is_no_read_closes_the_group_ahead_of_its_own_cell() {
    let mut app = App::new();
    app.start_tool("Read", "a.rs", None);
    app.end_tool("1 a", true);
    app.place_resolved_tool();
    app.start_tool("Write", "a.rs", None);
    app.end_tool("Wrote 1 line to a.rs", true);
    assert_eq!(
        app.place_resolved_tool(),
        ToolPlacement::Commit { closed: Some(0..1) }
    );
    assert_eq!(app.tool_group_range(), None, "closed");
}

#[test]
fn a_refused_read_closes_the_group_too() {
    let mut app = App::new();
    app.start_tool("Read", "a.rs", None);
    app.end_tool("1 a", true);
    app.place_resolved_tool();
    app.start_tool("Bash", "ls -la", None);
    app.reject_tool("User rejected ls -la", "no");
    assert_eq!(
        app.place_resolved_tool(),
        ToolPlacement::Commit { closed: Some(0..1) }
    );
}

#[test]
fn closing_the_group_hands_back_everything_it_held() {
    let mut app = App::new();
    app.start_tool("Read", "a.rs", None);
    app.end_tool("1 a", true);
    app.place_resolved_tool();
    // A thought the turn settled while the group was open is held with it.
    app.history.push(thought());
    assert_eq!(app.close_tool_group(), Some(0..2));
    assert_eq!(app.close_tool_group(), None, "only once");
}

#[test]
fn verbose_output_holds_nothing() {
    let mut app = App::new();
    app.settings_mut().verbose = true;
    app.start_tool("Read", "a.rs", None);
    app.end_tool("1 a", true);
    assert_eq!(
        app.place_resolved_tool(),
        ToolPlacement::Commit { closed: None }
    );
    assert_eq!(app.tool_group_range(), None);
}

#[test]
fn turning_verbose_on_lets_go_of_the_open_group() {
    // The held calls were never committed; the rebuild the row triggers
    // paints them as cells of their own.
    let mut app = App::new();
    app.start_tool("Read", "a.rs", None);
    app.end_tool("1 a", true);
    app.place_resolved_tool();
    app.open_settings();
    let at = SettingKey::ALL
        .iter()
        .position(|key| *key == SettingKey::Verbose)
        .unwrap();
    for _ in 0..at {
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Action::SettingChanged(SettingKey::Verbose)
    );
    assert!(app.settings().verbose);
    assert_eq!(app.tool_group_range(), None);
}

#[test]
fn clearing_the_conversation_drops_the_group() {
    let mut app = App::new();
    app.start_tool("Read", "a.rs", None);
    app.end_tool("1 a", true);
    app.place_resolved_tool();
    app.clear_conversation();
    assert_eq!(app.tool_group_range(), None);
    assert_eq!(app.close_tool_group(), None);
}

// ===== the hint row: every call's value, in the order the calls ran =====

fn batch(calls: &[(&str, &str)]) -> Vec<crate::stream::ToolCallSummary> {
    calls
        .iter()
        .map(|(name, args)| crate::stream::ToolCallSummary {
            name: (*name).to_string(),
            args: (*args).to_string(),
        })
        .collect()
}

/// Run the queue's front call to its end and place it, the way the boundary
/// resolves each call of an announced batch.
fn run_front(app: &mut App) {
    let front = app.tool_queue().front().expect("a queued call");
    let (name, args) = (front.name.clone(), front.args.clone());
    app.start_tool(&name, &args, None);
    app.end_tool("out", true).expect("a running call");
    app.place_resolved_tool();
}

fn shown_hint(app: &App) -> Option<String> {
    app.group_hint(true).map(|tool| tool.args.clone())
}

#[test]
fn a_parallel_batch_shows_each_calls_value_in_turn() {
    // docs/tool-groups.md *The hint row*: a batch of reads resolves in
    // milliseconds, and a row that followed the newest call showed only the
    // last one's value. Each value holds the row for GROUP_HINT_HOLD, in the
    // order the calls ran, the first one first — and none is skipped, however
    // late a tick comes.
    let mut app = App::new();
    app.start_tool_batch(&batch(&[
        ("Read", "README.md"),
        ("Bash", "ls -la"),
        ("Bash", "grep -rn todo src"),
        ("Bash", "cat Cargo.toml"),
    ]));
    for _ in 0..4 {
        run_front(&mut app);
    }
    let t0 = Duration::from_secs(10);
    app.advance_group_hint(t0);
    assert_eq!(
        shown_hint(&app).as_deref(),
        Some("README.md"),
        "the first call first"
    );
    app.advance_group_hint(t0 + GROUP_HINT_HOLD - Duration::from_millis(1));
    assert_eq!(
        shown_hint(&app).as_deref(),
        Some("README.md"),
        "held for the whole hold"
    );
    app.advance_group_hint(t0 + GROUP_HINT_HOLD);
    assert_eq!(shown_hint(&app).as_deref(), Some("ls -la"), "then the next");
    app.advance_group_hint(t0 + GROUP_HINT_HOLD * 10);
    assert_eq!(
        shown_hint(&app).as_deref(),
        Some("grep -rn todo src"),
        "one step per hold, never a skip"
    );
    // The late tick starts the next hold, so its value gets the whole of it.
    app.advance_group_hint(t0 + GROUP_HINT_HOLD * 11 - Duration::from_millis(1));
    assert_eq!(shown_hint(&app).as_deref(), Some("grep -rn todo src"));
    app.advance_group_hint(t0 + GROUP_HINT_HOLD * 11);
    assert_eq!(shown_hint(&app).as_deref(), Some("cat Cargo.toml"));
    app.advance_group_hint(t0 + GROUP_HINT_HOLD * 40);
    assert_eq!(
        shown_hint(&app).as_deref(),
        Some("cat Cargo.toml"),
        "the last value stays on the row"
    );
}

#[test]
fn a_call_that_runs_after_the_hold_shows_at_once() {
    // A round at a time — the model takes seconds to write its next call —
    // so the newest call's value is on the row the moment it runs.
    let mut app = App::new();
    let t0 = Duration::from_secs(10);
    app.start_tool("Bash", "ls -la", None);
    app.advance_group_hint(t0);
    assert_eq!(
        shown_hint(&app).as_deref(),
        Some("ls -la"),
        "the running read"
    );
    app.end_tool("out", true).expect("running");
    app.place_resolved_tool();
    app.start_tool("Read", "a.rs", None);
    app.advance_group_hint(t0 + Duration::from_secs(3));
    assert_eq!(shown_hint(&app).as_deref(), Some("a.rs"));
}

#[test]
fn a_call_still_waiting_its_turn_is_not_shown_ahead_of_it() {
    let mut app = App::new();
    app.start_tool_batch(&batch(&[("Read", "a.rs"), ("Read", "b.rs")]));
    let t0 = Duration::from_secs(10);
    app.advance_group_hint(t0);
    assert_eq!(
        shown_hint(&app).as_deref(),
        Some("a.rs"),
        "the batch's first call, about to run"
    );
    app.start_tool("Read", "a.rs", None);
    app.advance_group_hint(t0 + GROUP_HINT_HOLD * 3);
    assert_eq!(
        shown_hint(&app).as_deref(),
        Some("a.rs"),
        "b.rs has not started"
    );
}

#[test]
fn a_new_group_starts_its_hints_from_its_own_first_call() {
    let mut app = App::new();
    let t0 = Duration::from_secs(10);
    app.start_tool_batch(&batch(&[("Read", "a.rs"), ("Read", "b.rs")]));
    run_front(&mut app);
    run_front(&mut app);
    app.advance_group_hint(t0);
    app.advance_group_hint(t0 + GROUP_HINT_HOLD);
    assert_eq!(shown_hint(&app).as_deref(), Some("b.rs"));
    // Reply text arrived and closed the group, then the next round read again.
    assert!(app.close_tool_group().is_some());
    app.start_tool_batch(&batch(&[("Read", "c.rs"), ("Read", "d.rs")]));
    run_front(&mut app);
    run_front(&mut app);
    app.advance_group_hint(t0 + GROUP_HINT_HOLD * 2);
    assert_eq!(
        shown_hint(&app).as_deref(),
        Some("c.rs"),
        "the new group's first call, not where the last one stopped"
    );
}

#[test]
fn the_prompts_still_frame_steps_through_the_held_calls_alone() {
    // A permission prompt about the next read shows the group at rest above
    // that call's own cell, so the row never names the call being asked
    // about.
    let mut app = App::new();
    app.start_tool_batch(&batch(&[("Read", "a.rs"), ("Bash", "ls -la")]));
    run_front(&mut app);
    let t0 = Duration::from_secs(10);
    app.advance_group_hint(t0);
    app.advance_group_hint(t0 + GROUP_HINT_HOLD * 5);
    assert_eq!(
        app.group_hint(false).map(|tool| tool.args.as_str()),
        Some("a.rs")
    );
}

#[test]
fn verbose_output_shows_no_hint() {
    let mut app = App::new();
    app.settings_mut().verbose = true;
    app.start_tool("Read", "a.rs", None);
    app.advance_group_hint(Duration::from_secs(1));
    assert_eq!(app.group_hint(true), None);
}
