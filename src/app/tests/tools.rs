//! Tool-call bookkeeping and the parallel batch queue
//! (`docs/tools.md`, `docs/parallel-tools.md`).

use super::*;

// --- tool calls ---

#[test]
fn start_tool_marks_a_running_tool_not_yet_in_history() {
    let mut app = App::new();
    app.start_tool("Bash", "cargo test", None);
    let tool = app.current_tool().expect("a tool is running");
    assert_eq!(tool.name, "Bash");
    assert_eq!(tool.args, "cargo test");
    assert_eq!(tool.status, ToolStatus::Running);
    assert!(app.history.is_empty(), "a running tool is not yet history");
}

#[test]
fn start_tool_records_the_verbatim_arguments_on_the_call() {
    // The header summary is lossy on purpose (`● Write(a.py)`), so the raw
    // arguments ride beside it — that is what the derived context replays
    // (`docs/context.md`). It must land on a batch sibling too: a batch no
    // round announced (the offline dummy) carries only the summary, so its
    // queued `Waiting` cell is filled in when its own ToolStart flips it to
    // `Running` (an announced round fills it in up front — see
    // `an_announced_round_gives_each_waiting_call_the_models_own_arguments`).
    let arguments = r#"{"path":"a.py","content":"print(1)\n"}"#;
    let mut app = App::new();
    app.start_tool("Write", "a.py", Some(arguments));
    assert_eq!(
        app.current_tool().unwrap().arguments.as_deref(),
        Some(arguments)
    );
    let recorded = app.end_tool("Wrote 1 line to a.py", true).unwrap();
    assert_eq!(
        recorded.arguments.as_deref(),
        Some(arguments),
        "the record keeps them"
    );

    let mut app = App::new();
    app.start_tool_batch(&ping_batch());
    assert_eq!(
        app.current_tool().unwrap().arguments,
        None,
        "announced, not run"
    );
    app.start_tool(
        "Bash",
        "ping google.com",
        Some(r#"{"command":"ping google.com"}"#),
    );
    assert_eq!(
        app.current_tool().unwrap().arguments.as_deref(),
        Some(r#"{"command":"ping google.com"}"#),
        "…and the sibling that starts takes its own"
    );
}

#[test]
fn start_tool_batch_queues_every_call_as_waiting() {
    // A parallel batch registers all calls up front, all `Waiting`, so the
    // live region can show each — the not-yet-run ones as `⎿ Waiting…`. The
    // front is the first call (about to run); none are in history yet.
    let mut app = App::new();
    app.start_tool_batch(&ping_batch());
    assert_eq!(app.tool_queue().len(), 3, "all three calls are live");
    assert!(
        app.tool_queue()
            .iter()
            .all(|t| t.status == ToolStatus::Waiting),
        "every batched call starts Waiting"
    );
    let front = app
        .current_tool()
        .expect("the front call is the current one");
    assert_eq!(front.args, "ping google.com");
    assert!(app.history.is_empty(), "a queued batch is not yet history");
}

#[test]
fn start_tool_flips_the_front_waiting_call_to_running_without_adding_one() {
    // Executing the first batch call flips the front `Waiting`→`Running` (it
    // does not push a second call): the siblings stay `Waiting`.
    let mut app = App::new();
    app.start_tool_batch(&ping_batch());
    app.start_tool("Bash", "ping google.com", None);
    assert_eq!(app.tool_queue().len(), 3, "no extra call was pushed");
    assert_eq!(app.current_tool().unwrap().status, ToolStatus::Running);
    assert_eq!(
        app.tool_queue()[1].status,
        ToolStatus::Waiting,
        "the siblings are still waiting"
    );
    assert_eq!(app.tool_queue()[2].status, ToolStatus::Waiting);
}

#[test]
fn end_tool_pops_the_front_and_the_next_batch_call_becomes_current() {
    // Finishing the running call removes it from the live queue and records
    // it; the next `Waiting` sibling becomes the front (about to run).
    let mut app = App::new();
    app.start_tool_batch(&ping_batch());
    app.start_tool("Bash", "ping google.com", None);
    let finished = app.end_tool("pong", true).expect("the front call finished");
    assert_eq!(finished.args, "ping google.com");
    assert_eq!(finished.status, ToolStatus::Ok);
    assert_eq!(
        app.tool_queue().len(),
        2,
        "the finished call left the queue"
    );
    assert_eq!(
        app.current_tool().unwrap().args,
        "ping facebook.com",
        "the next sibling is now the front"
    );
    assert_eq!(app.current_tool().unwrap().status, ToolStatus::Waiting);
    assert!(
        matches!(app.history.last(), Some(HistoryItem::Tool(t)) if t.args == "ping google.com"),
        "the finished call is recorded in history"
    );
}

#[test]
fn a_lone_start_tool_without_a_batch_pushes_a_running_call() {
    // The single-tool path (the `!` shell, the dummy's lone Read) is
    // unchanged: with no batch queued, start_tool pushes one Running call.
    let mut app = App::new();
    app.start_tool("Read", "src/main.rs", None);
    assert_eq!(app.tool_queue().len(), 1);
    assert_eq!(app.current_tool().unwrap().status, ToolStatus::Running);
}

#[test]
fn end_tool_records_a_successful_tool_call_and_clears_the_slot() {
    let mut app = App::new();
    app.start_tool("Read", "src/main.rs", None);
    let finished = app
        .end_tool("line1\nline2", true)
        .expect("a tool was running");
    assert_eq!(finished.status, ToolStatus::Ok);
    assert!(app.current_tool().is_none(), "running slot cleared");
    assert_eq!(
        app.history.last(),
        Some(&HistoryItem::Tool(ToolCall {
            name: "Read".to_string(),
            args: "src/main.rs".to_string(),
            status: ToolStatus::Ok,
            output: "line1\nline2".to_string(),
            timestamp: String::new(),
            shell: false,
            truncated: false,
            context_output: None,
            arguments: None,
            approval_note: None,
            batch: None,
            call_id: None,
            position: None,
        }))
    );
}

#[test]
fn reject_tool_keeps_the_model_facing_result_beside_the_cell_text() {
    // A permission rejection resolves red like any failure, but with TWO
    // texts: the short cell line the user reads and the longer instruction
    // the model read. Only keeping both lets the derived context replay what
    // was really sent — Tab's amend feedback included (docs/permissions.md).
    let mut app = App::new();
    app.start_tool("Write", "hello.py", None);
    let finished = app
        .reject_tool(
            "User rejected write to hello.py\nInstructions: just print it",
            "The user doesn't want to proceed with this tool use. …",
        )
        .expect("a tool was running");
    assert_eq!(finished.status, ToolStatus::Failed);
    assert_eq!(
        finished.output,
        "User rejected write to hello.py\nInstructions: just print it"
    );
    assert_eq!(
        finished.context_text(),
        "The user doesn't want to proceed with this tool use. …"
    );
    assert!(app.current_tool().is_none(), "running slot cleared");
    assert_eq!(app.history.last(), Some(&HistoryItem::Tool(finished)));
}

#[test]
fn an_ordinary_call_reads_its_own_output_as_the_model_facing_text() {
    // The split exists only for a rejection: every other call's cell text *is*
    // what the model read, so `context_text` falls through to `output`.
    let mut app = App::new();
    app.start_tool("Read", "a.txt", None);
    let finished = app.end_tool("L1", true).expect("a tool was running");
    assert_eq!(finished.context_output, None);
    assert_eq!(finished.context_text(), "L1");
}

#[test]
fn a_rejections_token_tally_charges_the_text_the_model_reads() {
    // The tally counts what the next request uploads. For a rejection that is
    // the long instruction, not the one-line cell (docs/status-indicator.md).
    let long = "The user doesn't want to proceed with this tool use. The tool use was rejected. \
                STOP what you are doing and wait for the user to tell you how to proceed.";
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Write", "hello.py", None);
    app.reject_tool("User rejected write to hello.py", long);
    let charged = app.status().expect("a turn is active").tokens;
    assert_eq!(charged, crate::app::count_tokens(long));
}

#[test]
fn end_tool_marks_a_failure_red() {
    let mut app = App::new();
    app.start_tool("Bash", "false", None);
    let finished = app.end_tool("boom", false).expect("a tool was running");
    assert_eq!(finished.status, ToolStatus::Failed);
}

#[test]
fn end_tool_when_idle_returns_none_and_records_nothing() {
    let mut app = App::new();
    assert!(app.end_tool("ignored", true).is_none());
    assert!(app.history.is_empty());
}

#[test]
fn set_tool_truncated_marks_the_running_tool_and_end_tool_keeps_it() {
    let mut app = App::new();
    app.begin_shell("tree ~/");
    app.set_tool_truncated();
    let finished = app
        .end_tool("/home/me\n├── a", true)
        .expect("a tool was running");
    assert!(finished.truncated, "the over-cap flag survives end_tool");
}

#[test]
fn set_tool_truncated_is_a_noop_when_no_tool_is_running() {
    let mut app = App::new();
    app.set_tool_truncated(); // must not panic
    assert!(app.current_tool().is_none());
}

#[test]
fn set_tool_note_rides_the_running_call_into_history() {
    // The auto mode classifier's note (docs/permissions.md): recorded on the
    // running call right after its ToolStart, kept by end_tool so the
    // committed cell (and a /resume of it) can append the provenance row.
    let mut app = App::new();
    app.start_tool("Bash", "ls -la", None);
    app.set_tool_note("Allowed by auto mode classifier");
    let finished = app.end_tool("Exit code: 0\ntotal 40", true).expect("ran");
    assert_eq!(
        finished.approval_note.as_deref(),
        Some("Allowed by auto mode classifier")
    );
    let Some(HistoryItem::Tool(recorded)) = app.history.last() else {
        panic!("the finished call is recorded");
    };
    assert_eq!(
        recorded.approval_note.as_deref(),
        Some("Allowed by auto mode classifier")
    );
}

#[test]
fn set_tool_note_ignores_a_waiting_sibling_and_an_idle_queue() {
    // The note always follows its own call's ToolStart — a batch whose front
    // is still Waiting (and an empty queue) must not pick it up.
    let mut app = App::new();
    app.set_tool_note("stray"); // must not panic
    app.start_tool_batch(&ping_batch());
    app.set_tool_note("stray");
    assert!(
        app.tool_queue().iter().all(|t| t.approval_note.is_none()),
        "a Waiting front takes no note"
    );
}

#[test]
fn push_tool_output_tails_the_running_tool() {
    // Live streaming: each ToolOutput chunk appends to the running call's
    // output so the cell tails it (docs/tool-streaming.md).
    let mut app = App::new();
    app.start_tool("Bash", "ping -c 2 x", None);
    app.push_tool_output("line 1\n");
    app.push_tool_output("line 2\n");
    assert_eq!(app.current_tool().unwrap().output, "line 1\nline 2\n");
}

#[test]
fn push_tool_output_is_a_noop_when_no_tool_is_running() {
    let mut app = App::new();
    app.push_tool_output("stray"); // must not panic
    assert!(app.current_tool().is_none());
}

#[test]
fn push_tool_output_does_not_tail_a_waiting_batch_sibling() {
    // Only the front, *running* call tails output — a Waiting batch sibling
    // (not yet executing) is never targeted, even though it is the front in
    // the brief gap before start_tool flips it. The queue only ever runs its
    // front, so streamed output belongs to the running call.
    let mut app = App::new();
    app.start_tool_batch(&ping_batch());
    app.push_tool_output("early"); // front is Waiting, not Running yet
    assert!(
        app.current_tool().unwrap().output.is_empty(),
        "a Waiting sibling does not accumulate output"
    );
}

#[test]
fn a_screen_update_appends_settled_text_and_replaces_the_live_tail() {
    // A terminal session streams its screen (docs/interactive-shell.md):
    // settled text is final and appends, and the live rows replace the ones
    // the last update sent — a progress bar is one row changing in place.
    let mut app = App::new();
    app.start_tool("Bash", "sudo pacman -Syy", None);
    app.push_tool_screen("", " extra  10%");
    app.push_tool_screen("", " extra  50%");
    assert_eq!(app.current_tool().unwrap().output, " extra  50%");
    app.push_tool_screen(
        ":: Synchronizing package databases...\n",
        " extra 100%\n multilib 100%",
    );
    assert_eq!(
        app.current_tool().unwrap().output,
        ":: Synchronizing package databases...\n extra 100%\n multilib 100%"
    );
}

#[test]
fn a_new_call_starts_with_no_live_tail() {
    let mut app = App::new();
    app.start_tool("Bash", "one", None);
    app.push_tool_screen("", "the first call's screen");
    app.end_tool("Exit code: 0\nthe first call's screen", true);
    app.start_tool("Bash", "two", None);
    app.push_tool_screen("", "x");
    assert_eq!(
        app.current_tool().unwrap().output,
        "x",
        "the last call's live length cuts nothing from this one"
    );
}

#[test]
fn a_screen_update_only_reaches_a_running_call() {
    let mut app = App::new();
    app.push_tool_screen("a\n", "b"); // no call at all: no panic
    app.start_tool_batch(&ping_batch());
    app.push_tool_screen("a\n", "b");
    assert!(
        app.current_tool().unwrap().output.is_empty(),
        "a Waiting sibling has not started"
    );
}

#[test]
fn every_change_to_the_running_call_bumps_its_revision() {
    // A same-length redraw (45% → 46%) changes no length the transcript
    // cache watches; the revision is what tells it.
    let mut app = App::new();
    app.start_tool("Bash", "x", None);
    let started = app.tool_revision();
    app.push_tool_screen("", "45%");
    let first = app.tool_revision();
    app.push_tool_screen("", "46%");
    assert!(first > started);
    assert!(app.tool_revision() > first);
    app.set_tool_title("y");
    assert!(app.tool_revision() > first + 1);
}

#[test]
fn a_refined_title_replaces_the_running_calls_header() {
    let mut app = App::new();
    app.start_tool("BashSession", "b7x2k9m1q ← y⏎", None);
    app.set_tool_title("sudo pacman -Syy ← y⏎");
    assert_eq!(app.current_tool().unwrap().args, "sudo pacman -Syy ← y⏎");
    app.end_tool("Exit code: 0\ndone", true);
    app.set_tool_title("stray"); // nothing running: no panic, nothing changed
    assert!(app.current_tool().is_none());
}

#[test]
fn apply_tool_screen_swaps_only_the_live_tail() {
    let mut output = String::from("settled\n");
    let live = crate::app::apply_tool_screen(&mut output, 0, "", "é live");
    assert_eq!(output, "settled\né live");
    let live = crate::app::apply_tool_screen(&mut output, live, "more\n", "next");
    assert_eq!(output, "settled\nmore\nnext");
    assert_eq!(live, "next".len());
}

#[test]
fn push_tool_output_does_not_charge_the_token_tally() {
    // The tally is charged once, from the authoritative ToolEnd output in
    // end_tool — never from the streamed chunks (which would double-count).
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "echo hi", None);
    app.push_tool_output("hi\n");
    assert_eq!(
        app.status().unwrap().tokens,
        0,
        "streaming does not charge tokens"
    );
    app.end_tool("Exit code: 0\nhi", true);
    assert!(
        app.status().unwrap().tokens > 0,
        "end_tool charges the final output once"
    );
}

#[test]
fn background_tool_resolves_the_front_call_as_backgrounded() {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "ping x.com", None);
    let tool = app
        .background_tool(
            "Command running in the background. Output is streaming to /tmp/a0/s1/bash_1.output.",
        )
        .expect("resolves the running call");
    assert_eq!(tool.status, ToolStatus::Backgrounded);
    assert_eq!(
        tool.output,
        "Command running in the background. Output is streaming to /tmp/a0/s1/bash_1.output."
    );
    assert!(
        app.current_tool().is_none(),
        "the live queue is empty again"
    );
    assert_eq!(
        app.history.last(),
        Some(&HistoryItem::Tool(tool)),
        "the backgrounded cell is history — it repaints and rides context"
    );
}

// --- the file tools' path display (docs/tools.md "Path display") ---

/// The session policy of the worked example: launched in
/// `~/Codes/tests`, home `/home/linuztx`.
fn paths() -> PathDisplay {
    PathDisplay::new(
        "/home/linuztx/Codes/tests",
        Some(std::path::PathBuf::from("/home/linuztx")),
    )
}

#[test]
fn a_path_under_the_cwd_reads_relative_to_it() {
    let paths = paths();
    assert_eq!(
        paths.display("/home/linuztx/Codes/tests/hello.py"),
        "hello.py"
    );
    assert_eq!(
        paths.display("/home/linuztx/Codes/tests/hello/hello.py"),
        "hello/hello.py"
    );
    assert_eq!(paths.display("/home/linuztx/Codes/tests"), ".");
    // Trailing separators and dot segments collapse lexically.
    assert_eq!(paths.display("/home/linuztx/Codes/tests/dir/"), "dir");
    assert_eq!(
        paths.display("/home/linuztx/Codes/tests/./a/../hello.py"),
        "hello.py"
    );
}

#[test]
fn a_path_outside_the_cwd_but_under_home_reads_tilde_relative() {
    let paths = paths();
    assert_eq!(paths.display("/home/linuztx/hello.py"), "~/hello.py");
    // Even a near sibling — the rule is "where is it", not "how far away":
    // a `../other/x.py` climb says less than `~/Codes/other/x.py` does.
    assert_eq!(
        paths.display("/home/linuztx/Codes/other/x.py"),
        "~/Codes/other/x.py"
    );
    assert_eq!(paths.display("/home/linuztx"), "~");
}

#[test]
fn a_path_outside_home_reads_absolute() {
    let paths = paths();
    assert_eq!(paths.display("/tmp/x.py"), "/tmp/x.py");
    assert_eq!(paths.display("/etc/hosts"), "/etc/hosts");
    assert_eq!(paths.display("/tmp/./a/../x.py"), "/tmp/x.py");
    // Another user's home is not `~` — nor is a directory whose name merely
    // starts with the home's (component-wise containment).
    assert_eq!(paths.display("/home/other/x.py"), "/home/other/x.py");
    assert_eq!(paths.display("/home/linuztxx/f"), "/home/linuztxx/f");
}

#[test]
fn a_relative_path_resolves_against_the_cwd_first() {
    let paths = paths();
    assert_eq!(paths.display("hello.py"), "hello.py");
    assert_eq!(paths.display("./src/../hello.py"), "hello.py");
    // A climb out of the cwd lands wherever it lands, and reads by the same
    // rule as an absolute path there.
    assert_eq!(paths.display("../sib/f.txt"), "~/Codes/sib/f.txt");
    assert_eq!(paths.display("."), ".");
}

#[test]
fn without_a_home_the_tilde_rule_is_skipped() {
    let paths = PathDisplay::new("/home/linuztx/Codes/tests", None);
    assert_eq!(
        paths.display("/home/linuztx/hello.py"),
        "/home/linuztx/hello.py"
    );
    assert_eq!(
        paths.display("/home/linuztx/Codes/tests/hello.py"),
        "hello.py"
    );
}

#[test]
fn the_verbatim_policy_leaves_every_path_as_given() {
    // The unit-test default (and the policy of a session whose cwd is
    // unreadable): the header echoes the argument exactly.
    for path in [
        "/home/linuztx/Codes/tests/hello.py",
        "/home/linuztx/hello.py",
        "/tmp/./x.py",
        "../rel/path.txt",
    ] {
        assert_eq!(PathDisplay::VERBATIM.display(path), path);
        assert_eq!(PathDisplay::default().display(path), path);
    }
    // A relative cwd has nothing to relate to: verbatim as well.
    assert_eq!(
        PathDisplay::new("relative", None).display("/tmp/x.py"),
        "/tmp/x.py"
    );
}

#[test]
fn the_app_holds_the_verbatim_policy_until_the_boundary_injects_one() {
    let mut app = App::new();
    assert_eq!(*app.path_display(), PathDisplay::VERBATIM);
    app.set_path_display(paths());
    assert_eq!(
        app.path_display().display("/home/linuztx/hello.py"),
        "~/hello.py"
    );
}

#[test]
fn the_header_link_is_the_files_absolute_url() {
    // The `● Write(hello.py)` header's `file://` target names the whole
    // file, whatever short form the row shows (docs/links.md).
    let paths = paths();
    assert_eq!(
        paths
            .file_url("/home/linuztx/Codes/tests/hello.py")
            .as_deref(),
        Some("file:///home/linuztx/Codes/tests/hello.py")
    );
    assert_eq!(
        paths.file_url("hello.py").as_deref(),
        Some("file:///home/linuztx/Codes/tests/hello.py"),
        "a relative argument resolves against the cwd"
    );
    assert_eq!(
        paths.file_url("/home/linuztx/hello.py").as_deref(),
        Some("file:///home/linuztx/hello.py"),
        "the `~` form still links the absolute file"
    );
    assert_eq!(paths.file_url(""), None);
}

#[test]
fn the_verbatim_policy_links_only_what_it_can_place() {
    // No cwd: an absolute path is its own place, a relative one has none.
    assert_eq!(
        PathDisplay::VERBATIM.file_url("/tmp/x.py").as_deref(),
        Some("file:///tmp/x.py")
    );
    assert_eq!(PathDisplay::VERBATIM.file_url("rel/x.py"), None);
    assert_eq!(
        PathDisplay::new("relative", None).file_url("rel/x.py"),
        None
    );
}

/// The wire identity a backend announces ahead of a round's cells.
fn round_call(id: &str, name: &str) -> crate::stream::RoundCall {
    round_call_with(id, name, "{}")
}

/// [`round_call`] naming the model's verbatim arguments for the call.
fn round_call_with(id: &str, name: &str, arguments: &str) -> crate::stream::RoundCall {
    crate::stream::RoundCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

fn summary(name: &str, args: &str) -> ToolCallSummary {
    ToolCallSummary {
        name: name.to_string(),
        args: args.to_string(),
    }
}

#[test]
fn an_announced_round_stamps_the_provider_ids_and_one_batch_on_every_record() {
    // The round the model made: a bash call, a task call, a read — in that
    // order. The batch announcement lists the two visible calls; the task
    // call resolves through its own event. Every record ends up carrying
    // the provider's id for its call and the round's one batch id
    // (docs/prompt-caching.md).
    let mut app = App::new();
    app.begin_stream();
    app.open_round(vec![
        round_call("call_x", "bash"),
        round_call("call_t", "taskupdate"),
        round_call("call_y", "read"),
    ]);
    let batch = app.open_round_batch().expect("a round is open");
    app.start_tool_batch(&[summary("Bash", "ls"), summary("Read", "a.rs")]);
    app.start_tool("Bash", "ls", Some(r#"{"command":"ls"}"#));
    app.end_tool("a", true);
    app.record_task_call(
        "TaskUpdate",
        "#1 → completed",
        r#"{"taskId":"1","status":"completed"}"#,
        "Updated task #1 status",
        true,
        crate::tasks::TaskStore::new(),
    );
    app.start_tool("Read", "a.rs", Some(r#"{"path":"a.rs"}"#));
    app.end_tool("source", true);
    let identities: Vec<(Option<String>, Option<u64>)> = app
        .history
        .iter()
        .filter_map(|item| match item {
            HistoryItem::Tool(tool) => Some((tool.call_id.clone(), tool.batch)),
            HistoryItem::TaskCall(record) => Some((record.call_id.clone(), record.batch)),
            _ => None,
        })
        .collect();
    assert_eq!(
        identities,
        vec![
            (Some("call_x".to_string()), Some(batch)),
            (Some("call_t".to_string()), Some(batch)),
            (Some("call_y".to_string()), Some(batch)),
        ],
        "{:?}",
        app.history
    );
}

#[test]
fn an_announced_round_stamps_each_records_position_in_the_models_order() {
    // The model's round: [bash, agent, taskupdate, read]. The agent group
    // resolves first and the task call rides its own event, so the records
    // land in another order — each carries the index the model gave its
    // call, which is what lets the replay restore the wire's order
    // (docs/prompt-caching.md).
    let mut app = App::new();
    app.begin_stream();
    app.open_round(vec![
        round_call("call_b", "bash"),
        round_call("call_a", "agent"),
        round_call("call_t", "taskupdate"),
        round_call("call_r", "read"),
    ]);
    let mut specs = agent_specs(false);
    specs.truncate(1);
    specs[0].call_id = Some("call_a".to_string());
    app.start_agent_group(false, &specs);
    assert_eq!(
        app.agents()[0].position,
        Some(1),
        "the launch's index in the round"
    );
    app.finish_agent_group(
        false,
        &[AgentCallDone {
            id: "a1".into(),
            output: "done".into(),
            ok: true,
        }],
    );
    app.start_tool_batch(&[summary("Bash", "ls"), summary("Read", "a.rs")]);
    app.start_tool("Bash", "ls", Some(r#"{"command":"ls"}"#));
    app.end_tool("a", true);
    app.record_task_call(
        "TaskUpdate",
        "#1 → completed",
        r#"{"taskId":"1","status":"completed"}"#,
        "Updated task #1 status",
        true,
        crate::tasks::TaskStore::new(),
    );
    app.start_tool("Read", "a.rs", Some(r#"{"path":"a.rs"}"#));
    app.end_tool("source", true);
    let positions: Vec<(Option<String>, Option<usize>)> = app
        .history
        .iter()
        .flat_map(|item| match item {
            HistoryItem::Tool(tool) => vec![(tool.call_id.clone(), tool.position)],
            HistoryItem::TaskCall(record) => vec![(record.call_id.clone(), record.position)],
            HistoryItem::AgentGroup(group) => group
                .agents
                .iter()
                .map(|entry| (entry.call_id.clone(), entry.position))
                .collect(),
            _ => Vec::new(),
        })
        .collect();
    assert_eq!(
        positions,
        vec![
            (Some("call_a".to_string()), Some(1)),
            (Some("call_b".to_string()), Some(0)),
            (Some("call_t".to_string()), Some(2)),
            (Some("call_r".to_string()), Some(3)),
        ],
        "{:?}",
        app.history
    );
}

#[test]
fn an_announced_round_gives_each_waiting_call_the_models_own_arguments() {
    // The batch announcement carries only the header summaries; the round
    // ahead of it carries the model's verbatim arguments, so a call that
    // never reaches its own ToolStart still records exactly what the model
    // asked for (docs/interrupt.md).
    let ping = r#"{"command":"ping google.com -c 10","timeout":60000}"#;
    let ls = r#"{"command":"ls -la","description":"List files"}"#;
    let mut app = App::new();
    app.begin_stream();
    app.open_round(vec![
        round_call_with("call_ping", "bash", ping),
        round_call_with("call_ls", "bash", ls),
    ]);
    app.start_tool_batch(&[
        summary("Bash", "ping google.com -c 10"),
        summary("Bash", "ls -la"),
    ]);
    let arguments: Vec<Option<&str>> = app
        .tool_queue()
        .iter()
        .map(|call| call.arguments.as_deref())
        .collect();
    assert_eq!(arguments, [Some(ping), Some(ls)]);
    // A hook's `updatedInput` rewrites what runs: the call's own ToolStart
    // still has the last word.
    let rewritten = r#"{"command":"ping google.com -c 3"}"#;
    app.start_tool("Bash", "ping google.com -c 3", Some(rewritten));
    assert_eq!(
        app.current_tool()
            .and_then(|call| call.arguments.as_deref()),
        Some(rewritten)
    );
}

#[test]
fn an_interrupted_round_replays_every_call_the_model_made() {
    // The model's parallel round: two bash calls. The first is running and
    // the second still `⎿ Waiting…` when Esc lands. The next request must
    // still carry BOTH calls — one assistant message, under the provider's own
    // ids and with the model's own arguments — each answered `Interrupted by
    // user`, then the notice. A call missing from the context is a call the
    // model no longer knows it made (docs/interrupt.md).
    let ping = r#"{"command":"ping google.com -c 10","timeout":60000}"#;
    let ls = r#"{"command":"ls -la","description":"List files"}"#;
    let mut app = App::new();
    app.record_user_message("ping google, then list the files");
    app.begin_stream();
    app.open_round(vec![
        round_call_with("call_ping", "bash", ping),
        round_call_with("call_ls", "bash", ls),
    ]);
    app.start_tool_batch(&[
        summary("Bash", "ping google.com -c 10"),
        summary("Bash", "ls -la"),
    ]);
    app.start_tool("Bash", "ping google.com -c 10", Some(ping));
    app.interrupt_turn().expect("a turn was active");

    use crate::context::{ContextMessage, ContextRole, ContextToolCall};
    assert_eq!(
        crate::context::context_messages(&app.history),
        vec![
            ContextMessage::new(ContextRole::User, "ping google, then list the files"),
            ContextMessage::assistant_tool_calls(
                "",
                vec![
                    ContextToolCall::new("call_ping", "bash", ping),
                    ContextToolCall::new("call_ls", "bash", ls),
                ],
            ),
            ContextMessage::tool_result("call_ping", INTERRUPT_TOOL_OUTPUT),
            ContextMessage::tool_result("call_ls", INTERRUPT_TOOL_OUTPUT),
            ContextMessage::new(ContextRole::User, format!("[error] {INTERRUPT_NOTICE}")),
        ]
    );
}

#[test]
fn a_batch_announced_without_a_round_carries_no_ids() {
    // The offline dummy announces batches but never a round: its records
    // stay unstamped and replay under synthesized ids, as they always did.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool_batch(&[summary("Bash", "ls")]);
    app.start_tool("Bash", "ls", None);
    app.end_tool("a", true);
    let HistoryItem::Tool(tool) = &app.history[0] else {
        panic!("a tool record: {:?}", app.history);
    };
    assert_eq!(tool.call_id, None);
    assert_eq!(tool.batch, Some(1));
    assert_eq!(tool.position, None);
}

#[test]
fn each_announced_round_is_its_own_batch_and_a_new_turn_forgets_the_last() {
    let mut app = App::new();
    app.begin_stream();
    app.open_round(vec![round_call("call_1", "bash")]);
    app.start_tool_batch(&[summary("Bash", "ls")]);
    app.start_tool("Bash", "ls", None);
    app.end_tool("a", true);
    app.open_round(vec![round_call("call_2", "bash")]);
    app.start_tool_batch(&[summary("Bash", "pwd")]);
    app.start_tool("Bash", "pwd", None);
    app.end_tool("/x", true);
    let batches: Vec<Option<u64>> = app
        .history
        .iter()
        .filter_map(|item| match item {
            HistoryItem::Tool(tool) => Some(tool.batch),
            _ => None,
        })
        .collect();
    assert_eq!(batches.len(), 2);
    assert_ne!(
        batches[0], batches[1],
        "two rounds, two batches: {batches:?}"
    );
    app.begin_stream();
    assert_eq!(app.open_round_batch(), None, "a new turn opens on no round");
}
