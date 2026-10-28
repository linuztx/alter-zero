//! The folded tool view's state: membership, the held run, the floor, and
//! when a group is open. See `docs/tool-groups.md`.

use super::*;

/// A resolved call, as the history records it.
fn call(name: &str, args: &str, status: ToolStatus, output: &str) -> ToolCall {
    ToolCall {
        name: name.to_string(),
        args: args.to_string(),
        status,
        output: output.to_string(),
        timestamp: String::new(),
        shell: false,
        truncated: false,
        context_output: None,
        approval_note: None,
        arguments: None,
        batch: None,
        call_id: None,
        position: None,
    }
}

fn ok(name: &str, args: &str) -> ToolCall {
    let output = if name == "Bash" {
        "Exit code: 0\nout"
    } else {
        "1 x"
    };
    call(name, args, ToolStatus::Ok, output)
}

fn tool(t: ToolCall) -> HistoryItem {
    HistoryItem::Tool(t)
}

fn message(role: Role, text: &str) -> HistoryItem {
    HistoryItem::Message(Message {
        role,
        text: text.to_string(),
        timestamp: String::new(),
        images: Vec::new(),
    })
}

fn thought() -> HistoryItem {
    HistoryItem::Reasoning(Reasoning {
        text: "hmm".to_string(),
        secs: 1,
        tokens: 3,
        timestamp: String::new(),
    })
}

fn kind_of(t: &ToolCall) -> Option<ReadKind> {
    tool_group_member(t).map(|reading| reading.kind)
}

/// An app mid-turn that has resolved `calls` in order — a read run the
/// strip is holding.
fn turn_with(calls: &[(&str, &str)]) -> App {
    let mut app = App::new();
    app.begin_stream();
    for (name, args) in calls {
        app.start_tool(name, args, None);
        let output = if *name == "Bash" {
            "Exit code: 0\nout"
        } else {
            "1 x"
        };
        app.end_tool(output, true);
    }
    app
}

// ===== membership =====

#[test]
fn a_read_and_a_command_that_only_looks_are_members() {
    assert_eq!(kind_of(&ok("Read", "/repo/a.py")), Some(ReadKind::Read));
    assert_eq!(kind_of(&ok("Bash", "ls -la")), Some(ReadKind::List));
    assert_eq!(kind_of(&ok("Bash", "cat a.py")), Some(ReadKind::Read));
    assert_eq!(
        kind_of(&ok("Bash", "grep -rn foo .")),
        Some(ReadKind::Search)
    );
    // A read names the file it read, so two reads of one file count once.
    assert_eq!(
        tool_group_member(&ok("Read", "/repo/a.py")).map(|r| r.targets),
        Some(vec!["/repo/a.py".to_string()])
    );
}

#[test]
fn an_image_read_a_write_or_a_quiet_command_is_not() {
    for t in [
        ok("Read", "/repo/shot.png"),
        ok("Write", "/repo/a.py"),
        ok("Edit", "/repo/a.py"),
        ok("Bash", "npm test"),
        ok("Bash", "ls > files.txt"),
        ok("Bash", "git status"),
        ok("Skill", "dataviz"),
    ] {
        assert_eq!(kind_of(&t), None, "{} {}", t.name, t.args);
    }
    let mut shell = ok("Bash", "ls");
    shell.shell = true;
    assert_eq!(
        kind_of(&shell),
        None,
        "a `!` command is the user's own cell"
    );
}

#[test]
fn a_failure_leaves_the_group_but_a_search_that_exits_one_stays() {
    let failed =
        |name: &str, args: &str, output: &str| call(name, args, ToolStatus::Failed, output);
    assert_eq!(
        kind_of(&failed("Read", "/repo/missing.py", "not found")),
        None
    );
    assert_eq!(
        kind_of(&failed(
            "Bash",
            "cat missing.txt",
            "Exit code: 1\nNo such file"
        )),
        None
    );
    assert_eq!(
        kind_of(&failed("Bash", "ls nope", "Exit code: 2\nls: nope")),
        None
    );
    // Nothing matched — an answer, not an error.
    assert_eq!(
        kind_of(&failed("Bash", "grep -rn zzz .", "Exit code: 1\n")),
        Some(ReadKind::Search)
    );
    assert_eq!(
        kind_of(&failed(
            "Bash",
            "grep -rn zzz .",
            "Exit code: 2\ngrep: bad regex"
        )),
        None,
        "a search that errored is still a failure"
    );
    assert_eq!(
        kind_of(&failed("Bash", "ls", "Interrupted by user")),
        None,
        "an interrupted call is no member"
    );
    assert_eq!(
        kind_of(&call("Bash", "ls", ToolStatus::Backgrounded, "launched")),
        None
    );
    // Handed back still running: its cell says where the session stands.
    assert_eq!(
        kind_of(&ok_with(
            "Bash",
            "cat",
            "Running (session k3x9, waiting for input)\n"
        )),
        None
    );
}

fn ok_with(name: &str, args: &str, output: &str) -> ToolCall {
    call(name, args, ToolStatus::Ok, output)
}

#[test]
fn a_queued_or_running_member_counts_before_it_resolves() {
    for status in [ToolStatus::Waiting, ToolStatus::Running] {
        assert_eq!(
            kind_of(&call("Bash", "ls", status, "")),
            Some(ReadKind::List)
        );
        assert_eq!(
            kind_of(&call("Read", "/a.py", status, "")),
            Some(ReadKind::Read)
        );
    }
}

// ===== the held run =====

#[test]
fn the_held_run_is_the_trailing_members_above_the_floor() {
    let history = vec![
        message(Role::User, "explore"),
        tool(ok("Read", "/repo/a.py")),
        thought(),
        tool(ok("Bash", "ls")),
    ];
    assert_eq!(
        held_members(&history, 0),
        vec![1, 3],
        "a thought passes through"
    );
    assert_eq!(held_members(&history, 2), vec![3], "the floor cuts the run");
    assert!(held_members(&history, 4).is_empty());
    let mut ended = history.clone();
    ended.push(message(Role::Assistant, "done"));
    assert!(held_members(&ended, 0).is_empty(), "text breaks the run");
    let mut broken = history;
    broken.push(tool(ok("Write", "/repo/b.py")));
    assert!(held_members(&broken, 0).is_empty(), "a write breaks it");
}

#[test]
fn a_group_is_open_while_the_turn_can_still_add_to_it() {
    let mut app = turn_with(&[("Read", "/repo/a.py"), ("Bash", "ls")]);
    assert!(app.tool_group_open(), "a held run between rounds is open");
    assert_eq!(app.held_tool_group().len(), 2);
    // Whitespace before the next call says nothing.
    app.push_chunk("\n\n");
    assert!(app.tool_group_open());
    // Text does.
    app.push_chunk("Found it.");
    assert!(!app.tool_group_open(), "text ends it");
}

#[test]
fn the_next_call_decides_whether_the_group_continues() {
    let mut app = turn_with(&[("Read", "/repo/a.py")]);
    app.start_tool_batch(&[
        ToolCallSummary {
            name: "Bash".to_string(),
            args: "rg TODO".to_string(),
        },
        ToolCallSummary {
            name: "Write".to_string(),
            args: "/repo/b.py".to_string(),
        },
    ]);
    assert!(app.tool_group_open(), "a member is next");
    app.start_tool("Bash", "rg TODO", None);
    app.end_tool("Exit code: 0\nx", true);
    assert!(!app.tool_group_open(), "a write is next");
}

#[test]
fn a_group_is_never_open_verbose_or_between_turns() {
    let mut app = turn_with(&[("Read", "/repo/a.py")]);
    app.settings_mut().verbose = true;
    assert!(!app.tool_group_open(), "verbose draws every call");
    assert!(app.held_tool_group().is_empty());
    app.settings_mut().verbose = false;
    assert!(app.tool_group_open());
    app.finish_stream();
    assert!(!app.tool_group_open(), "the turn is over");
}

#[test]
fn sealing_commits_the_held_run_and_a_new_member_starts_the_next() {
    let mut app = turn_with(&[("Read", "/repo/a.py")]);
    app.seal_tool_group();
    assert!(app.held_tool_group().is_empty());
    assert!(!app.tool_group_open(), "nothing left to hold");
    app.start_tool("Bash", "ls", None);
    app.end_tool("Exit code: 0\nx", true);
    let held: Vec<&str> = app
        .held_tool_group()
        .iter()
        .map(|t| t.args.as_str())
        .collect();
    assert_eq!(held, ["ls"]);
}

#[test]
fn the_floor_follows_history_down() {
    let mut app = turn_with(&[("Read", "/repo/a.py"), ("Bash", "ls")]);
    app.seal_tool_group();
    assert_eq!(app.tool_group_floor(), app.history.len());
    app.clear_conversation();
    assert_eq!(
        app.tool_group_floor(),
        0,
        "a cleared history holds nothing below"
    );
    app.begin_stream();
    app.start_tool("Bash", "ls", None);
    app.end_tool("Exit code: 0\nx", true);
    assert_eq!(app.held_tool_group().len(), 1, "a new turn's run is held");
}

#[test]
fn the_fold_names_the_open_groups_first_member() {
    let mut app = App::new();
    app.begin_stream();
    app.record_message(Role::User, "explore");
    app.start_tool("Read", "/repo/a.py", None);
    app.end_tool("1 x", true);
    assert_eq!(app.tool_fold(), ToolFold::On { held_from: Some(1) });
    app.push_chunk("Done.");
    assert_eq!(app.tool_fold(), ToolFold::On { held_from: None });
    app.settings_mut().verbose = true;
    assert_eq!(app.tool_fold(), ToolFold::Off);
}

#[test]
fn an_announced_call_joins_a_group_by_what_it_is() {
    // What a batch announcement or a lone call's start says decides whether
    // the open group survives it — before anything has run.
    assert!(joins_tool_group("Read", "/repo/a.py"));
    assert!(joins_tool_group("Bash", "rg -n TODO"));
    assert!(!joins_tool_group("Read", "/repo/shot.png"));
    assert!(!joins_tool_group("Bash", "npm test"));
    assert!(!joins_tool_group("Write", "/repo/a.py"));
}
