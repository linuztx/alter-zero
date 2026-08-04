//! Tool-call bookkeeping and the parallel batch queue
//! (`docs/tools.md`, `docs/parallel-tools.md`).

use super::*;

// --- tool calls ---

#[test]
fn start_tool_marks_a_running_tool_not_yet_in_history() {
    let mut app = App::new();
    app.start_tool("Bash", "cargo test");
    let tool = app.current_tool().expect("a tool is running");
    assert_eq!(tool.name, "Bash");
    assert_eq!(tool.args, "cargo test");
    assert_eq!(tool.status, ToolStatus::Running);
    assert!(app.history.is_empty(), "a running tool is not yet history");
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
    app.start_tool("Bash", "ping google.com");
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
    app.start_tool("Bash", "ping google.com");
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
    app.start_tool("Read", "src/main.rs");
    assert_eq!(app.tool_queue().len(), 1);
    assert_eq!(app.current_tool().unwrap().status, ToolStatus::Running);
}

#[test]
fn end_tool_records_a_successful_tool_call_and_clears_the_slot() {
    let mut app = App::new();
    app.start_tool("Read", "src/main.rs");
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
            approval_note: None,
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
    app.start_tool("Write", "hello.py");
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
    app.start_tool("Read", "a.txt");
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
    app.start_tool("Write", "hello.py");
    app.reject_tool("User rejected write to hello.py", long);
    let charged = app.status().expect("a turn is active").tokens;
    assert_eq!(charged, crate::app::count_tokens(long));
}

#[test]
fn end_tool_marks_a_failure_red() {
    let mut app = App::new();
    app.start_tool("Bash", "false");
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
    app.start_tool("Bash", "ls -la");
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
    app.start_tool("Bash", "ping -c 2 x");
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
fn push_tool_output_does_not_charge_the_token_tally() {
    // The tally is charged once, from the authoritative ToolEnd output in
    // end_tool — never from the streamed chunks (which would double-count).
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "echo hi");
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
    app.start_tool("Bash", "ping x.com");
    let tool = app
        .background_tool("Command running in background with ID: bash_1.")
        .expect("resolves the running call");
    assert_eq!(tool.status, ToolStatus::Backgrounded);
    assert_eq!(
        tool.output,
        "Command running in background with ID: bash_1."
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
