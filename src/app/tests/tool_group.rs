//! Collapsed read/search groups (`docs/tool-groups.md`): which calls join
//! one, how far a group runs through the history, what it counts, and how
//! the open group is held and closed.

use super::*;
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
        group_kind(
            &resolved("Read", "src/main.rs", true),
            &PathDisplay::VERBATIM
        ),
        Some(GroupKind::Read)
    );
    assert_eq!(
        group_kind(&resolved("Bash", "ls -la", true), &PathDisplay::VERBATIM),
        Some(GroupKind::List)
    );
    assert_eq!(
        group_kind(
            &resolved("Bash", "grep -rn todo src", true),
            &PathDisplay::VERBATIM
        ),
        Some(GroupKind::Search)
    );
    assert_eq!(
        group_kind(
            &resolved("Bash", "cat about.py", true),
            &PathDisplay::VERBATIM
        ),
        Some(GroupKind::Read)
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
            group_kind(&resolved(name, args, true), &PathDisplay::VERBATIM),
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
        assert_eq!(
            group_kind(&resolved("Read", path, true), &PathDisplay::VERBATIM),
            None,
            "{path}"
        );
    }
    // A text file is still a read, whatever its name resembles.
    assert_eq!(
        group_kind(
            &resolved("Read", "png-notes.md", true),
            &PathDisplay::VERBATIM
        ),
        Some(GroupKind::Read)
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
        assert_eq!(
            group_kind(&call, &PathDisplay::VERBATIM),
            None,
            "{arguments}"
        );
    }
    assert_eq!(
        group_kind(&resolved("Bash", "ls &", true), &PathDisplay::VERBATIM),
        None
    );
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
    assert_eq!(group_kind(&call, &PathDisplay::VERBATIM), None);
}

#[test]
fn a_read_that_ran_stays_in_its_group_whatever_its_exit() {
    // A search that found nothing exits 1; a missing file fails the read.
    // Either way the read happened, and the model reads the error — the
    // reference absorbs it, and Ctrl+O keeps the output.
    assert!(is_group_member(
        &resolved("Bash", "grep -rn nope src", false),
        &PathDisplay::VERBATIM
    ));
    assert!(is_group_member(
        &resolved("Read", "missing.rs", false),
        &PathDisplay::VERBATIM
    ));
}

#[test]
fn a_refused_interrupted_or_backgrounded_read_keeps_its_own_cell() {
    let refused = {
        let mut app = App::new();
        app.start_tool("Bash", "ls -la", None);
        app.reject_tool("User rejected ls -la", "The user doesn't want to proceed")
            .unwrap()
    };
    assert!(
        !is_group_member(&refused, &PathDisplay::VERBATIM),
        "a refusal is the user's record"
    );

    let interrupted = {
        let mut app = App::new();
        app.start_tool("Read", "a.rs", None);
        app.end_tool(INTERRUPT_TOOL_OUTPUT, false).unwrap()
    };
    assert!(!is_group_member(&interrupted, &PathDisplay::VERBATIM));

    let errored = {
        let mut app = App::new();
        app.start_tool("Read", "a.rs", None);
        app.end_tool(ERROR_TOOL_OUTPUT, false).unwrap()
    };
    assert!(!is_group_member(&errored, &PathDisplay::VERBATIM));

    let never_run = {
        let mut app = App::new();
        app.start_tool("Read", "a.rs", None);
        app.end_tool(crate::llm::agent::TOOL_LIMIT_OUTPUT, false)
            .unwrap()
    };
    assert!(
        !is_group_member(&never_run, &PathDisplay::VERBATIM),
        "it never ran"
    );

    let backgrounded = {
        let mut app = App::new();
        app.start_tool("Bash", "grep -rn x /", None);
        app.background_tool("launched").unwrap()
    };
    assert!(!is_group_member(&backgrounded, &PathDisplay::VERBATIM));
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
        group_run(&items, &PathDisplay::VERBATIM),
        Some(5),
        "up to the reply, trailing thought included"
    );
    assert_eq!(
        group_run(&items[5..], &PathDisplay::VERBATIM),
        None,
        "a reply is no group"
    );
    assert_eq!(
        group_run(&items[6..], &PathDisplay::VERBATIM),
        Some(1),
        "a lone read is a group"
    );
}

#[test]
fn a_call_that_is_not_a_read_ends_the_group() {
    let items = vec![
        tool("Read", "a.rs"),
        tool("Edit", "a.rs"),
        tool("Read", "a.rs"),
    ];
    assert_eq!(group_run(&items, &PathDisplay::VERBATIM), Some(1));
    assert_eq!(group_run(&items[1..], &PathDisplay::VERBATIM), None);
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
    let counts = group_counts(calls.iter(), &PathDisplay::VERBATIM);
    assert_eq!(
        counts,
        GroupCounts {
            searches: 2,
            reads: 2,
            lists: 1,
            ..GroupCounts::default()
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

// ===== scratchpad edits: a write or edit in the session's scratchpad =====

/// The session's scratchpad in these tests.
const PAD: &str = "/tmp/alter-zero-0/s1/scratchpad";

/// A path policy that knows [`PAD`].
fn pad_paths() -> PathDisplay {
    PathDisplay::VERBATIM.with_scratchpad(Some(PAD.into()))
}

/// A `name` call on `args` resolved with `output`.
fn resolved_with(name: &str, args: &str, ok: bool, output: &str) -> ToolCall {
    let mut app = App::new();
    app.start_tool(name, args, None);
    app.end_tool(output, ok).expect("a running call")
}

fn pad_file(name: &str) -> String {
    format!("{PAD}/{name}")
}

#[test]
fn a_write_or_edit_in_the_scratchpad_joins_a_group_as_an_edit() {
    let paths = pad_paths();
    let write = resolved_with("Write", &pad_file("guess.py"), true, "Wrote 55 lines to x");
    assert_eq!(group_kind(&write, &paths), Some(GroupKind::ScratchpadEdit));
    assert!(is_group_member(&write, &paths));
    let edit = resolved_with("Edit", &pad_file("guess.py"), true, "Updated x (+2 -1)");
    assert_eq!(group_kind(&edit, &paths), Some(GroupKind::ScratchpadEdit));
    // Anywhere else a write keeps its cell: the user's own files are no
    // scratch work.
    for path in ["/home/u/proj/main.py", "main.py", PAD] {
        let write = resolved_with("Write", path, true, "Wrote 1 line to x");
        assert_eq!(group_kind(&write, &paths), None, "{path}");
    }
}

#[test]
fn a_failed_scratchpad_edit_keeps_its_own_cell() {
    let paths = pad_paths();
    let failed = resolved_with("Edit", &pad_file("guess.py"), false, "old_string not found");
    assert!(
        !is_group_member(&failed, &paths),
        "an edit that did not happen was not made"
    );
}

#[test]
fn a_scratchpad_edit_counts_the_lines_its_cell_reports() {
    for (output, added, removed) in [
        ("Wrote 55 lines to guess.py\n 1 a", 55, 0),
        ("Wrote 1 line to guess.py\n1 a", 1, 0),
        ("Updated guess.py (+2 -1)\n3 -a\n3 +b", 2, 1),
        // The pre-rename head old rollouts still carry.
        ("Created guess.py (4 lines)\n1 a", 4, 0),
        ("No changes to guess.py", 0, 0),
        ("something else", 0, 0),
    ] {
        let tool = resolved_with("Write", &pad_file("guess.py"), true, output);
        assert_eq!(edit_lines(&tool), EditLines { added, removed }, "{output}");
    }
}

#[test]
fn a_group_counts_its_scratchpad_edits_and_their_lines() {
    let paths = pad_paths();
    let calls = [
        resolved_with("Read", "a.rs", true, "1 a"),
        resolved_with("Write", &pad_file("guess.py"), true, "Wrote 55 lines to x"),
        resolved_with("Edit", &pad_file("guess.py"), true, "Updated x (+2 -1)"),
    ];
    let counts = group_counts(calls.iter(), &paths);
    assert_eq!((counts.reads, counts.edits), (1, 2));
    assert_eq!(
        counts.lines,
        EditLines {
            added: 57,
            removed: 1
        }
    );
    assert!(!counts.is_empty());
}

#[test]
fn a_scratchpad_edit_extends_a_run_of_reads() {
    let paths = pad_paths();
    let write = resolved_with("Write", &pad_file("x.py"), true, "Wrote 2 lines to x");
    let items = vec![
        tool("Read", "a.rs"),
        HistoryItem::Tool(write),
        assistant("done"),
    ];
    assert_eq!(group_run(&items, &paths), Some(2));
    // A scratchpad moved off the layout (ALTER_ZERO_SCRATCHPAD_DIR) is known
    // only to the session whose it is.
    let moved = resolved_with("Write", "/home/u/scratch/x.py", true, "Wrote 2 lines to x");
    let items = vec![tool("Read", "a.rs"), HistoryItem::Tool(moved)];
    assert_eq!(group_run(&items, &PathDisplay::VERBATIM), Some(1));
    let knows = PathDisplay::VERBATIM.with_scratchpad(Some("/home/u/scratch".into()));
    assert_eq!(group_run(&items, &knows), Some(2));
}

#[test]
fn a_resolved_scratchpad_edit_is_held_in_the_open_group() {
    let mut app = App::new();
    app.set_path_display(pad_paths());
    app.begin_stream();
    app.start_tool("Write", &pad_file("guess.py"), None);
    app.end_tool("Wrote 55 lines to guess.py", true);
    assert_eq!(app.place_resolved_tool(), ToolPlacement::Held);
}

#[test]
fn an_unresolved_write_counts_the_content_it_carries() {
    // Before it runs, a write's lines are its own content: the arguments a
    // live round announced, or the draft that streamed them.
    let mut app = App::new();
    app.start_tool(
        "Write",
        &pad_file("x.py"),
        Some(r#"{"path":"/tmp/alter-zero-0/s1/scratchpad/x.py","content":"a\nb\nc\n"}"#),
    );
    let queued = app.tool_queue().front().expect("running").clone();
    assert_eq!(
        pending_edit_lines(&queued, None),
        EditLines {
            added: 3,
            removed: 0
        }
    );
    let mut bare = queued.clone();
    bare.arguments = None;
    assert_eq!(
        pending_edit_lines(&bare, Some("a\nb")),
        EditLines {
            added: 2,
            removed: 0
        }
    );
    // An edit's lines are its diff, which only running it reports.
    bare.name = "Edit".to_string();
    assert_eq!(pending_edit_lines(&bare, None), EditLines::default());
}

/// One streamed piece of a `write` under slot 0.
fn write_piece(name: &str, arguments: &str) -> crate::stream::ToolCallFragment {
    crate::stream::ToolCallFragment {
        index: 0,
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

#[test]
fn a_streaming_write_ends_the_group_only_once_its_path_is_no_scratch_work() {
    let mut app = App::new();
    app.set_path_display(pad_paths());
    app.begin_stream();
    app.push_tool_call_fragment(&write_piece("write", r#"{"pa"#));
    assert!(!app.draft_ends_group(), "nothing says yet where it writes");
    app.push_tool_call_fragment(&write_piece("", r#"th":"/home/u/proj/main.py","con"#));
    assert!(
        app.draft_ends_group(),
        "a write into the project is no read"
    );
    assert_eq!(app.group_drafts().count(), 0);

    let mut app = App::new();
    app.set_path_display(pad_paths());
    app.begin_stream();
    let arguments = format!(r#"{{"path":"{}","content":"a"#, pad_file("g.py"));
    app.push_tool_call_fragment(&write_piece("write", &arguments));
    assert!(
        !app.draft_ends_group(),
        "a scratchpad write joins the group"
    );
    assert_eq!(app.group_drafts().count(), 1);
    assert_eq!(
        app.group_hint(true).map(|item| item.target()),
        Some(pad_file("g.py").as_str())
    );
}

#[test]
fn verbose_output_leaves_a_scratchpad_write_its_own_cell() {
    let mut app = App::new();
    app.set_path_display(pad_paths());
    app.settings_mut().verbose = true;
    app.begin_stream();
    let arguments = format!(r#"{{"path":"{}","content":"a"#, pad_file("g.py"));
    app.push_tool_call_fragment(&write_piece("write", &arguments));
    assert_eq!(app.group_drafts().count(), 0);
    assert!(app.group_hint(true).is_none());
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

/// Cycle the `/settings` **Verbose output** row with Enter.
fn cycle_verbose(app: &mut App) {
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
}

/// An app mid-turn with **Verbose output** on, whose reads committed as cells
/// of their own: a reply, a read, a thought, a listing.
fn verbose_turn_with_reads() -> App {
    let mut app = App::new();
    app.settings_mut().verbose = true;
    app.begin_stream();
    app.history.push(assistant("Let me look around."));
    app.start_tool("Read", "a.rs", None);
    app.end_tool("1 a", true);
    assert_eq!(
        app.place_resolved_tool(),
        ToolPlacement::Commit { closed: None }
    );
    app.history.push(thought());
    app.start_tool("Bash", "ls -la", None);
    app.end_tool("x", true);
    app.place_resolved_tool();
    app
}

#[test]
fn turning_verbose_off_mid_turn_holds_the_reads_already_on_screen() {
    // The rebuild the row triggers paints them as the open group, and the
    // next read joins it — so a later rebuild draws the one line scrollback
    // will show, never a line for the reads before the switch and another
    // for the ones after it.
    let mut app = verbose_turn_with_reads();
    cycle_verbose(&mut app);
    assert!(!app.settings().verbose);
    assert_eq!(app.tool_group_range(), Some(1..4));
    // The row names the newest of them: the others were cells on screen.
    assert_eq!(
        app.group_hint(true).map(|item| item.target()),
        Some("ls -la")
    );
    app.start_tool("Read", "b.rs", None);
    app.end_tool("1 b", true);
    assert_eq!(app.place_resolved_tool(), ToolPlacement::Held);
    assert_eq!(app.tool_group_range(), Some(1..5));
}

#[test]
fn turning_verbose_off_holds_nothing_the_turn_has_moved_past() {
    // Between turns the rebuild draws the settled line itself.
    let mut app = verbose_turn_with_reads();
    app.end_turn(1);
    cycle_verbose(&mut app);
    assert_eq!(app.tool_group_range(), None, "no turn is running");
    // Reply text after the reads ended their group already.
    let mut app = verbose_turn_with_reads();
    app.push_chunk("It reads a.rs.");
    cycle_verbose(&mut app);
    assert_eq!(app.tool_group_range(), None, "the reply has begun");
    // So did a call of another kind.
    let mut app = verbose_turn_with_reads();
    app.start_tool("Write", "c.rs", None);
    cycle_verbose(&mut app);
    assert_eq!(app.tool_group_range(), None, "a write is running");
    let mut app = verbose_turn_with_reads();
    app.start_tool("Write", "c.rs", None);
    app.end_tool("Wrote 1 lines to c.rs", true);
    app.place_resolved_tool();
    cycle_verbose(&mut app);
    assert_eq!(app.tool_group_range(), None, "the write ended the run");
}

#[test]
fn turning_verbose_off_while_a_scratchpad_write_streams_holds_the_reads() {
    // A write into the scratchpad continues the run rather than ending it
    // (docs/tool-groups.md *Scratchpad edits*): the reads it follows join it
    // in the open group. One anywhere else has ended the run.
    let mut app = verbose_turn_with_reads();
    app.set_path_display(pad_paths());
    app.push_tool_call_fragment(&write_piece(
        "write",
        &format!(r#"{{"path":"{}","content":"a"#, pad_file("g.py")),
    ));
    cycle_verbose(&mut app);
    assert_eq!(app.tool_group_range(), Some(1..4));
    let mut app = verbose_turn_with_reads();
    app.set_path_display(pad_paths());
    app.push_tool_call_fragment(&write_piece(
        "write",
        r#"{"path":"/home/u/c.rs","content":"a"#,
    ));
    cycle_verbose(&mut app);
    assert_eq!(
        app.tool_group_range(),
        None,
        "a write elsewhere ended the run"
    );
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
    app.group_hint(true).map(|item| item.target().to_string())
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
        app.group_hint(false).map(|item| item.target()),
        Some("a.rs")
    );
}

#[test]
fn verbose_output_shows_no_hint() {
    let mut app = App::new();
    app.settings_mut().verbose = true;
    app.start_tool("Read", "a.rs", None);
    app.advance_group_hint(Duration::from_secs(1));
    assert!(app.group_hint(true).is_none());
}
