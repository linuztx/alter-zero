//! The turn lifecycle: streaming, the status tally, interrupts, and the
//! committed summary (`docs/status-indicator.md`, `docs/interrupt.md`).

use super::*;

#[test]
fn ctrl_c_clears_the_draft_even_mid_stream() {
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("draft");
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.on_key(ctrl_c), Action::None);
    assert!(app.input.is_empty());
    assert!(
        app.turn_active(),
        "clearing the draft never touches the turn"
    );
}

#[test]
fn a_resent_message_is_not_re_persisted_adjacently() {
    // The persist stream still collapses adjacent duplicates (like the file
    // dedup): sending the same text twice in a row writes it once.
    let mut app = App::new();
    submit(&mut app, "again");
    submit(&mut app, "again");
    assert_eq!(
        app.take_unpersisted_inputs(),
        ["again"],
        "an immediately re-sent message is persisted once, not twice"
    );
}

#[test]
fn esc_dismissing_the_band_wins_over_interrupt_mid_turn() {
    let mut app = App::new();
    app.begin_stream();
    app.on_key(key(KeyCode::Char('?'))); // the toggle works mid-turn too
    assert!(app.shortcuts_open);
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(!app.shortcuts_open);
    assert!(
        app.turn_active(),
        "dismissing the band never touches the turn"
    );
    assert_eq!(
        app.on_key(key(KeyCode::Esc)),
        Action::Interrupt,
        "the next Esc interrupts as usual"
    );
}

#[test]
fn begin_stream_starts_empty_streaming_buffer() {
    let mut app = App::new();
    assert!(!app.is_streaming());
    app.begin_stream();
    assert!(app.is_streaming());
    assert_eq!(app.streaming_text(), Some(""));
}

#[test]
fn push_chunk_accumulates_into_streaming_buffer() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("Hello, ");
    app.push_chunk("world");
    assert_eq!(app.streaming_text(), Some("Hello, world"));
}

#[test]
fn finish_stream_returns_text_and_clears_state() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("done");
    let finished = app.finish_stream();
    assert_eq!(finished, Some("done".to_string()));
    assert!(!app.is_streaming());
    assert_eq!(app.streaming_text(), None);
}

#[test]
fn finish_stream_when_idle_returns_none() {
    let mut app = App::new();
    assert_eq!(app.finish_stream(), None);
}

#[test]
fn finish_stream_records_the_assistant_message_in_history() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("hi there");
    app.finish_stream();
    assert_eq!(
        app.history.last(),
        Some(&HistoryItem::Message(Message {
            role: Role::Assistant,
            text: "hi there".to_string(),
            timestamp: String::new(),
            images: Vec::new(),
        }))
    );
}

#[test]
fn finish_stream_when_idle_records_nothing() {
    let mut app = App::new();
    app.finish_stream();
    assert!(app.history.is_empty());
}

#[test]
fn a_full_turn_records_user_then_assistant_in_order() {
    let mut app = App::new();
    app.record_user_message("q");
    app.begin_stream();
    app.push_chunk("a");
    app.finish_stream();
    assert_eq!(roles(&app), vec![Role::User, Role::Assistant]);
}

#[test]
fn fail_stream_records_partial_then_error_and_clears_state() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("half a rep");
    let failure = app.fail_stream("network down").expect("was streaming");
    assert_eq!(failure.partial.as_deref(), Some("half a rep"));
    assert_eq!(failure.error, "network down");
    assert!(!app.is_streaming());
    assert_eq!(roles(&app), vec![Role::Assistant, Role::Error]);
    assert_eq!(message_at(&app, 1).text, "network down");
}

#[test]
fn fail_stream_with_no_partial_records_only_the_error() {
    let mut app = App::new();
    app.begin_stream(); // errored before any chunk arrived
    let failure = app.fail_stream("died early").expect("was streaming");
    assert!(failure.partial.is_none());
    assert_eq!(roles(&app), vec![Role::Error]);
}

#[test]
fn fail_stream_when_idle_returns_none_and_records_nothing() {
    let mut app = App::new();
    assert!(app.fail_stream("ignored").is_none());
    assert!(app.history.is_empty());
}

#[test]
fn fail_stream_resolves_a_running_tool_as_failed() {
    // The stream contract allows Error in place of StreamDone with a tool
    // still running (a real backend's mid-tool network failure). Like an
    // Esc interrupt, the turn's death must resolve the tool — leaving it
    // Running would wedge a phantom in the preview strip, the transcript,
    // and a later turn's interrupt record.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Read", "src/app.rs", None);
    let failure = app.fail_stream("network down").expect("was streaming");
    assert!(app.current_tool().is_none(), "no phantom running tool");
    let [tool] = failure.tools.as_slice() else {
        panic!("the failed tool rides the failure: {:?}", failure.tools);
    };
    assert_eq!(tool.status, ToolStatus::Failed);
    assert_eq!(tool.output, ERROR_TOOL_OUTPUT);
    // History order: the failed tool slots before the error notice, the
    // same shape a resize repaint (and the scrollback commit) renders.
    assert!(matches!(
        (&app.history[0], &app.history[1]),
        (HistoryItem::Tool(t), HistoryItem::Message(m))
            if t.status == ToolStatus::Failed && m.role == Role::Error
    ));
}

#[test]
fn fail_stream_orders_partial_then_tool_then_notice() {
    // Buffered text streamed before the tool started (in practice a
    // ToolStart flushes it, but the order holds regardless) — mirror
    // interrupt_turn's stream-order: partial, tool, error notice.
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("half a rep");
    app.start_tool("Bash", "ls", None);
    let failure = app.fail_stream("boom").expect("was streaming");
    assert_eq!(failure.partial.as_deref(), Some("half a rep"));
    assert_eq!(failure.tools.len(), 1);
    assert!(matches!(
        (&app.history[0], &app.history[1], &app.history[2]),
        (HistoryItem::Message(p), HistoryItem::Tool(_), HistoryItem::Message(e))
            if p.role == Role::Assistant && e.role == Role::Error
    ));
}

#[test]
fn fail_stream_without_a_tool_carries_none() {
    let mut app = App::new();
    app.begin_stream();
    let failure = app.fail_stream("died early").expect("was streaming");
    assert!(failure.tools.is_empty());
}

// --- Esc interrupts the in-flight turn, codex-style (docs/interrupt.md) ---

#[test]
fn esc_interrupts_while_a_turn_is_active() {
    let mut app = App::new();
    app.begin_stream();
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Interrupt);
}

#[test]
fn esc_with_the_palette_open_only_dismisses_it_even_mid_turn() {
    // Codex's "popup wins" rule: dismissing the palette takes precedence
    // over interrupting; the turn keeps running.
    let mut app = App::new();
    app.begin_stream();
    app.on_key(key(KeyCode::Char('/')));
    assert!(app.command_menu.is_some(), "the palette opened");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(app.command_menu.is_none(), "Esc closed the palette");
    assert!(app.turn_active(), "the turn was not interrupted");
}

#[test]
fn interrupt_turn_keeps_the_partial_and_records_the_notice() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("half a rep");
    let InterruptedTurn::Kept {
        partial,
        tools,
        notice,
        ..
    } = app.interrupt_turn().expect("a turn was active")
    else {
        panic!("a streamed partial is kept, not undone");
    };
    assert_eq!(partial.as_deref(), Some("half a rep"));
    assert!(tools.is_empty(), "no tool was running");
    assert_eq!(notice, Some(INTERRUPT_NOTICE), "a normal turn's notice");
    assert!(!app.is_streaming());
    assert!(!app.turn_active(), "the live status cleared");
    assert_eq!(roles(&app), vec![Role::Assistant, Role::Error]);
    assert_eq!(message_at(&app, 0).text, "half a rep");
    assert_eq!(message_at(&app, 1).text, INTERRUPT_NOTICE);
}

#[test]
fn interrupt_turn_with_no_output_undoes_the_submission() {
    // The user submitted "Hi", the backend produced nothing, then Esc:
    // instead of a `Conversation interrupted` notice the whole turn is
    // undone — "Hi" goes back into the composer and out of history, and
    // nothing is recorded (docs/interrupt.md, the "no output yet" case).
    let mut app = App::new();
    app.record_user_message("Hi");
    app.begin_stream();
    app.count_user_input("Hi");
    let outcome = app.interrupt_turn().expect("a turn was active");
    assert_eq!(outcome, InterruptedTurn::Undone);
    assert_eq!(
        app.input.text(),
        "Hi",
        "the message is back in the composer"
    );
    assert!(
        app.history.is_empty(),
        "the user message is dropped from history"
    );
    assert!(!app.turn_active(), "the live status cleared");
    assert!(!app.is_streaming());
}

#[test]
fn interrupt_between_tool_rounds_keeps_the_output_and_records_the_notice() {
    // The real backend's multi-round tool loop can leave the streaming
    // buffer empty (Some("")) with no running tool *after* an earlier round
    // already committed an assistant segment + a tool to history. An Esc in
    // the gap before the next round must NOT undo the turn (that would drop
    // the notice, orphan the committed output, and wipe a typed draft) — it
    // must take the Kept path and record the interrupt notice.
    let mut app = App::new();
    app.record_user_message("fix the bug");
    app.begin_stream();
    // Round 1: an assistant segment, then a finished tool.
    app.push_chunk("let me look");
    app.flush_streaming_segment(); // records the segment, leaves streaming = Some("")
    app.start_tool("Read", "src/app.rs", None);
    app.end_tool("fn main() {}", true); // pushes HistoryItem::Tool, current_tool = None
    // Round 2 is about to start; nothing has streamed yet this round.
    assert!(
        app.is_streaming(),
        "the stream is still open between rounds"
    );
    assert!(app.current_tool().is_none());

    let outcome = app.interrupt_turn().expect("a turn was active");
    match outcome {
        InterruptedTurn::Kept { notice, .. } => {
            assert_eq!(
                notice,
                Some(INTERRUPT_NOTICE),
                "a turn that already produced output records the interrupt notice"
            );
        }
        InterruptedTurn::Undone => panic!("must not undo a turn that already committed output"),
    }
    // The committed round-1 output survives, plus the interrupt notice.
    assert!(
        app.history
            .iter()
            .any(|h| matches!(h, HistoryItem::Message(m) if m.role == Role::Error)),
        "the interrupt notice is in history"
    );
    assert!(
        app.input.text().is_empty(),
        "the composer draft is left untouched (not clobbered by a bogus recall)"
    );
}

#[test]
fn interrupt_turn_undo_discards_a_mid_turn_drafts_attachments() {
    // recall_input clobbers whatever draft was typed mid-turn; its
    // attachments must not linger as invisible pairs — they are discarded
    // (and their temp files handed to the boundary for deletion).
    let mut app = App::new();
    app.record_user_message("Hi");
    app.begin_stream();
    app.attach_image(PathBuf::from("/tmp/draft.png")); // typed mid-turn
    assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
    assert_eq!(app.input.text(), "Hi");
    assert!(app.images.is_empty(), "no unanchored pairs survive");
    assert_eq!(
        app.take_discarded_images(),
        vec![PathBuf::from("/tmp/draft.png")],
        "the clobbered draft's temp file is queued for deletion"
    );
}

#[test]
fn interrupt_turn_undo_rejoins_a_batch_with_newlines() {
    // A queued batch flushes several user messages as one turn; undoing it
    // rejoins them with newlines (codex's Alt+Up shape).
    let mut app = App::new();
    app.record_user_message("first");
    app.record_user_message("second");
    app.begin_stream();
    assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
    assert_eq!(app.input.text(), "first\nsecond");
    assert!(app.history.is_empty());
}

#[test]
fn interrupt_turn_undo_only_reclaims_the_current_turns_message() {
    // A finished prior turn stays put; only the just-submitted message is
    // rolled back (trailing user messages belong to the current turn).
    let mut app = App::new();
    app.record_user_message("old");
    app.record_message(Role::Assistant, "a reply");
    app.record_user_message("new");
    app.begin_stream();
    assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
    assert_eq!(app.input.text(), "new");
    assert_eq!(
        roles(&app),
        vec![Role::User, Role::Assistant],
        "only the new user message was removed"
    );
    assert_eq!(message_at(&app, 0).text, "old");
}

#[test]
fn interrupt_turn_resolves_a_running_tool_as_failed() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("before the tool ");
    app.start_tool("Bash", "sleep 100", None); // flush happens loop-side; buffer keeps streaming
    let InterruptedTurn::Kept { tools, .. } = app.interrupt_turn().expect("a turn was active")
    else {
        panic!("streamed output is kept, not undone");
    };
    let [tool] = tools.as_slice() else {
        panic!("the running tool was resolved: {tools:?}");
    };
    assert_eq!(tool.status, ToolStatus::Failed);
    assert_eq!(tool.output, INTERRUPT_TOOL_OUTPUT);
    assert!(app.current_tool().is_none(), "no tool left running");
    assert!(
        app.history
            .iter()
            .any(|item| matches!(item, HistoryItem::Tool(t) if t.status == ToolStatus::Failed)),
        "the cancelled tool is recorded in history"
    );
}

/// The tool records in `app`'s history, in order.
fn recorded_tools(app: &App) -> Vec<&ToolCall> {
    app.history
        .iter()
        .filter_map(|item| match item {
            HistoryItem::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect()
}

#[test]
fn interrupt_mid_batch_resolves_the_running_call_and_every_waiting_sibling() {
    // Esc during a parallel batch: the running (front) call AND every
    // `⎿ Waiting…` sibling queued behind it resolve as `Interrupted by user`,
    // in the batch's order, and each one is recorded — so the transcript keeps
    // a cell for every call the model asked for, and the next turn's context
    // replays the whole round (docs/interrupt.md, docs/parallel-tools.md).
    // Dropping the siblings erased calls the model had made from everything
    // after the Esc.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool_batch(&ping_batch());
    app.start_tool("Bash", "ping google.com", None); // the front is now Running
    let InterruptedTurn::Kept { tools, notice, .. } =
        app.interrupt_turn().expect("a turn was active")
    else {
        panic!("a running tool means output streamed — kept, not undone");
    };
    let resolved: Vec<(&str, ToolStatus, &str)> = tools
        .iter()
        .map(|tool| (tool.args.as_str(), tool.status, tool.output.as_str()))
        .collect();
    assert_eq!(
        resolved,
        vec![
            ("ping google.com", ToolStatus::Failed, INTERRUPT_TOOL_OUTPUT),
            (
                "ping facebook.com",
                ToolStatus::Failed,
                INTERRUPT_TOOL_OUTPUT
            ),
            ("ping x.com", ToolStatus::Failed, INTERRUPT_TOOL_OUTPUT),
        ]
    );
    assert!(app.tool_queue().is_empty(), "nothing is left live");
    assert_eq!(
        recorded_tools(&app),
        tools.iter().collect::<Vec<_>>(),
        "every call is recorded, in the batch's order"
    );
    assert_eq!(notice, Some(INTERRUPT_NOTICE));
    assert!(
        matches!(app.history.last(), Some(HistoryItem::Message(m)) if m.role == Role::Error),
        "the notice closes the turn, after every cell: {:?}",
        app.history
    );
}

#[test]
fn interrupt_while_the_whole_batch_still_waits_resolves_every_call() {
    // The approve seam runs before a call's ToolStart, so while the front call
    // waits on a permission prompt — or auto mode's classifier — every cell of
    // the batch still reads `⎿ Waiting…` (the reported case). Esc there is an
    // interrupt like any other: the batch is on screen, so the turn is kept
    // rather than undone, and every call resolves, the front included.
    let mut app = App::new();
    app.record_user_message("ping them all");
    app.begin_stream();
    app.start_tool_batch(&ping_batch());
    let Some(InterruptedTurn::Kept { tools, notice, .. }) = app.interrupt_turn() else {
        panic!("an announced batch is output — kept, not undone");
    };
    assert_eq!(tools.len(), 3, "{tools:?}");
    assert!(
        tools
            .iter()
            .all(|tool| tool.status == ToolStatus::Failed && tool.output == INTERRUPT_TOOL_OUTPUT),
        "{tools:?}"
    );
    assert_eq!(recorded_tools(&app).len(), 3);
    assert_eq!(notice, Some(INTERRUPT_NOTICE));
    assert_eq!(
        app.input.text(),
        "",
        "nothing was pulled back into the composer"
    );
}

#[test]
fn fail_stream_mid_batch_resolves_every_call_in_the_batch() {
    // The interrupt's error-path twin: a backend that dies mid-batch leaves
    // the running call and each `⎿ Waiting…` sibling owed a ToolEnd that will
    // never come. Every one resolves as `Interrupted by a backend error`,
    // ahead of the red notice, rather than the siblings vanishing.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool_batch(&ping_batch());
    app.start_tool("Bash", "ping google.com", None);
    let failure = app.fail_stream("network down").expect("was streaming");
    let args: Vec<&str> = failure
        .tools
        .iter()
        .map(|tool| tool.args.as_str())
        .collect();
    assert_eq!(args, ["ping google.com", "ping facebook.com", "ping x.com"]);
    assert!(
        failure
            .tools
            .iter()
            .all(|tool| tool.status == ToolStatus::Failed && tool.output == ERROR_TOOL_OUTPUT),
        "{:?}",
        failure.tools
    );
    assert!(app.tool_queue().is_empty(), "no phantom waiting cell");
    let shape: Vec<bool> = app
        .history
        .iter()
        .map(|item| matches!(item, HistoryItem::Tool(_)))
        .collect();
    assert_eq!(
        shape,
        [true, true, true, false],
        "three cells, then the notice"
    );
}

#[test]
fn interrupt_turn_records_no_done_summary() {
    // An interrupt has no "Done for Ns" line — the notice is the
    // turn's terminal state, exactly like fail_stream.
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("text");
    app.interrupt_turn().expect("a turn was active");
    assert!(
        !app.history
            .iter()
            .any(|item| matches!(item, HistoryItem::Summary(_))),
        "no summary for an interrupted turn"
    );
}

#[test]
fn interrupt_turn_when_idle_returns_none_and_records_nothing() {
    let mut app = App::new();
    assert!(app.interrupt_turn().is_none());
    assert!(app.history.is_empty());
}

#[test]
fn interrupt_turn_stamps_the_records_with_the_clock() {
    let mut app = App::new();
    app.set_clock(|| "03:20 AM".to_string());
    app.begin_stream();
    app.push_chunk("partial");
    app.interrupt_turn().expect("a turn was active");
    assert_eq!(message_at(&app, 0).timestamp, "03:20 AM");
    assert_eq!(message_at(&app, 1).timestamp, "03:20 AM");
}

#[test]
fn flush_streaming_segment_records_text_and_reopens_an_empty_buffer() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("before the tool");
    let flushed = app.flush_streaming_segment().expect("buffered text");
    assert_eq!(flushed, "before the tool");
    // The stream stays open with a fresh empty buffer for the next segment.
    assert!(app.is_streaming());
    assert_eq!(app.streaming_text(), Some(""));
    assert_eq!(
        app.history.last(),
        Some(&HistoryItem::Message(Message {
            role: Role::Assistant,
            text: "before the tool".to_string(),
            timestamp: String::new(),
            images: Vec::new(),
        }))
    );
}

#[test]
fn flush_streaming_segment_with_an_empty_buffer_records_nothing() {
    let mut app = App::new();
    app.begin_stream(); // empty buffer
    assert!(app.flush_streaming_segment().is_none());
    assert!(app.history.is_empty(), "nothing buffered, nothing recorded");
    assert!(app.is_streaming(), "the stream stays open");
}

#[test]
fn flush_streaming_segment_when_idle_returns_none() {
    let mut app = App::new();
    assert!(app.flush_streaming_segment().is_none());
}

#[test]
fn finish_stream_with_an_empty_final_segment_records_nothing() {
    // A turn that ends right after a tool call (no trailing text) must not
    // leave a phantom empty assistant message behind.
    let mut app = App::new();
    app.begin_stream();
    assert!(app.finish_stream().is_none());
    assert!(app.history.is_empty());
    assert!(!app.is_streaming());
}

#[test]
fn a_turn_interleaves_text_and_a_tool_call_in_order() {
    // text → tool → text, the way the dummy backend streams it. History must
    // hold the two assistant segments with the tool between them, in order.
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("let me check");
    app.flush_streaming_segment(); // text before the tool becomes its own message
    app.start_tool("Bash", "ls", None);
    app.end_tool("a\nb", true);
    app.push_chunk("all done");
    app.finish_stream();

    match app.history.as_slice() {
        [
            HistoryItem::Message(first),
            HistoryItem::Tool(tool),
            HistoryItem::Message(last),
        ] => {
            assert_eq!(first.text, "let me check");
            assert_eq!(first.role, Role::Assistant);
            assert_eq!(tool.name, "Bash");
            assert_eq!(tool.status, ToolStatus::Ok);
            assert_eq!(last.text, "all done");
        }
        other => panic!("unexpected interleaving: {other:?}"),
    }
}

#[test]
fn tab_follow_ups_drain_one_turn_at_a_time() {
    // Each batch is its own turn: drain yields the reclaimed messages first
    // (they belonged to the turn that just ended), then the Tab follow-up —
    // sequential turns, not one merged blob.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("later");
    app.on_key(key(KeyCode::Tab));
    app.input = TextArea::from_text("first");
    app.on_key(key(KeyCode::Enter));
    app.reclaim_steered();
    assert_eq!(
        app.drain_next_batch(),
        Some(batch(&["first"])),
        "the first queue goes first"
    );
    assert_eq!(
        app.drain_next_batch(),
        Some(batch(&["later"])),
        "the Tab follow-up next"
    );
    assert!(app.drain_next_batch().is_none());
}

#[test]
fn tab_on_an_empty_composer_mid_turn_is_a_no_op() {
    let mut app = App::new();
    app.begin_stream();
    assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
    assert!(
        app.queued.is_empty(),
        "nothing to queue from an empty composer"
    );
}

#[test]
fn esc_is_sticky_typing_within_the_same_token_does_not_reopen() {
    let mut app = App::new();
    type_str(&mut app, "/he");
    app.on_key(key(KeyCode::Esc)); // dismiss
    assert!(app.command_menu.is_none());
    app.on_key(key(KeyCode::Char('l'))); // still within "/hel"
    assert!(
        app.command_menu.is_none(),
        "stays dismissed while editing the same token"
    );
}

#[test]
fn clear_mid_turn_wipes_the_streaming_state_and_records_nothing() {
    // `/clear` during a turn is a kill: the loop cancels + reaps the
    // backend (main.rs); the app side must leave no trace of the
    // half-done turn — no partial, no running tool, no live status, no
    // interrupt notice — or the fresh slate isn't fresh.
    let mut app = App::new();
    app.record_user_message("old message");
    app.begin_stream();
    app.push_chunk("half a rep");
    app.start_tool("read_file", "src/app.rs", None);
    type_str(&mut app, "/clear");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Clear);
    assert!(app.history.is_empty(), "no partial/notice/summary recorded");
    assert!(!app.is_streaming(), "the streaming buffer was dropped");
    assert!(!app.turn_active(), "the live status cleared");
    assert!(app.current_tool().is_none(), "the running tool was dropped");
}

#[test]
fn help_mid_turn_is_rejected_with_a_toast_not_the_command_list() {
    // Mid-turn the multi-line list would interleave with the streaming
    // reply, so /help is rejected with a transient toast instead.
    let mut app = App::new();
    app.begin_stream();
    type_str(&mut app, "/help");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Toast(HELP_BUSY_NOTICE.to_string()),
    );
    assert!(app.input.is_empty());
}

#[test]
fn esc_undo_of_an_init_turn_restores_the_command_not_the_prompt() {
    // Esc in the pre-stream window undoes the submission into the
    // composer (docs/interrupt.md). For an /init turn the user typed
    // "/init", not the 1.8KB canned prompt — flooding the composer with
    // text the user never held (which a follow-up Ctrl+C would then
    // record into ↑ recall) breaks the no-recall guarantee, so the undo
    // restores the command itself, palette reopened: the exact
    // pre-submit state.
    let mut app = App::new();
    type_str(&mut app, "/init");
    let Action::Submit(text) = app.on_key(key(KeyCode::Enter)) else {
        panic!("idle /init submits");
    };
    app.record_user_message(&text); // mirror start_turn's recording
    app.begin_stream();
    assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
    assert_eq!(app.input.text(), "/init", "the command, not the prompt");
    assert!(
        app.command_menu.is_some(),
        "the palette reopens on the recalled command"
    );
    assert!(app.history.is_empty(), "the submission rolled back");
}

#[test]
fn init_mid_turn_tab_is_rejected_like_enter() {
    // Tab is the palette's other accept key, and mid-turn it is also the
    // queue-as-new-batch key — the menu_open arm must keep winning, or
    // Tab on "/init" would silently queue the literal text as a message
    // for the model. Both accept keys reject with the busy toast.
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_stream();
    type_str(&mut app, "/init");
    assert_eq!(
        app.on_key(key(KeyCode::Tab)),
        Action::Toast(INIT_BUSY_NOTICE.to_string()),
    );
    assert!(
        app.queued.is_empty(),
        "the literal \"/init\" was not queued as a message"
    );
}

#[test]
fn init_mid_turn_is_rejected_with_a_toast() {
    // Codex's available_during_task is false for /init — submitting would
    // race the running stream with a second turn. Ours rejects with the
    // /compact toast pattern; the running turn is untouched.
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_stream();
    type_str(&mut app, "/init");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Toast(INIT_BUSY_NOTICE.to_string()),
    );
    assert!(app.turn_active(), "the running turn is untouched");
}

#[test]
fn apply_usage_tracks_the_context_size_from_the_usage_frame() {
    // The round's `input` is the whole re-sent context; `output` joins the
    // next round's context — their sum is the live gauge value.
    let mut app = App::new();
    app.begin_stream();
    app.apply_usage(&usage_of(5_000, 200));
    assert_eq!(app.context_used(), 5_200);
}

#[test]
fn a_turn_with_no_usage_frame_estimates_the_context_at_turn_end() {
    // The dummy sends no usage — the tokenizer estimate stands in so the
    // gauge (and the auto trigger) still work offline.
    let mut app = App::new();
    app.record_user_message("hello there");
    app.begin_stream();
    app.push_chunk("a reply");
    app.finish_stream();
    app.end_turn(1);
    assert!(app.context_used() > 0);
}

#[test]
fn the_context_estimate_counts_the_user_instructions() {
    // The instructions ride every request, so the footer gauge's offline
    // estimate must count them like the system prompt.
    let run_turn = |instructions: Option<&str>| {
        let mut app = App::new();
        app.set_user_instructions(instructions.map(str::to_string));
        app.record_user_message("hello there");
        app.begin_stream();
        app.push_chunk("a reply");
        app.finish_stream();
        app.end_turn(1);
        app.context_used()
    };
    let without = run_turn(None);
    let with = run_turn(Some("a long AGENTS.md contributor guide to count"));
    assert!(with > without, "{with} vs {without}");
}

#[test]
fn last_assistant_text_returns_the_last_assistant_message() {
    let mut app = App::new();
    app.record_user_message("q1");
    app.begin_stream();
    app.push_chunk("first answer");
    app.finish_stream();
    app.record_user_message("q2");
    app.begin_stream();
    app.push_chunk("second answer");
    app.finish_stream();
    assert_eq!(app.last_assistant_text().as_deref(), Some("second answer"));
}

#[test]
fn last_assistant_text_skips_trailing_non_assistant_items() {
    // A later user message, tool call, or system notice must not shadow the
    // last *assistant* message — `/copy` copies the model's last response.
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("the answer");
    app.finish_stream();
    app.start_tool("Read", "f", None);
    app.end_tool("out", true);
    app.record_user_message("a follow-up");
    app.record_system_message("a notice");
    assert_eq!(app.last_assistant_text().as_deref(), Some("the answer"));
}

#[test]
fn enter_runs_copy_returning_the_last_assistant_text() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("copy this answer");
    app.finish_stream();
    type_str(&mut app, "/copy");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Copy("copy this answer".to_string())
    );
    assert!(app.input.is_empty(), "the command was consumed");
    assert!(app.command_menu.is_none());
}

#[test]
fn copy_with_no_assistant_message_is_a_toast() {
    // Nothing to copy is the `/export` and `/compact` empty case — a soft
    // rejection the user needn't keep, so the same plain info toast rather
    // than a red failure (docs/toast.md).
    let mut app = App::new();
    app.record_user_message("just me");
    type_str(&mut app, "/copy");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Toast(COPY_EMPTY_NOTICE.to_string())
    );
    assert!(app.input.is_empty(), "the /copy token was consumed");
}

#[test]
fn copy_dispatches_mid_turn_instead_of_queuing() {
    // `/copy` is available during a task (codex): the palette's Enter wins
    // over the mid-turn queue, copying the last *completed* answer (not the
    // streaming buffer).
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("earlier answer");
    app.finish_stream();
    app.begin_stream();
    app.push_chunk("streaming, not yet recorded");
    type_str(&mut app, "/copy");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Copy("earlier answer".to_string()),
        "the palette runs /copy mid-turn rather than queuing it"
    );
    assert!(app.queued.is_empty(), "nothing was queued");
}

// --- live status indicator (see docs/status-indicator.md) ---

#[test]
fn idle_has_no_status_and_begin_stream_starts_one() {
    let mut app = App::new();
    assert!(!app.turn_active(), "no turn in flight when idle");
    assert!(app.status().is_none());
    app.begin_stream();
    assert!(app.turn_active(), "a turn is in flight while streaming");
    let status = app.status().expect("a status once streaming");
    assert!(
        WORKING_VERBS.contains(&status.verb),
        "a working verb is chosen: {:?}",
        status.verb
    );
    assert_eq!(status.tokens, 0, "no tokens counted yet (the '0s' state)");
    assert_eq!(status.arrow, TokenArrow::Down);
    assert_eq!(status.thinking, None, "not thinking yet");
}

#[test]
fn the_per_turn_verb_changes_from_one_turn_to_the_next() {
    // Deterministic but varied: consecutive turns pick the next verb in the
    // registry, so the demo isn't monotonous.
    let mut app = App::new();
    app.begin_stream();
    let first = app.status().unwrap().verb;
    app.finish_stream();
    app.end_turn(1);
    app.begin_stream();
    let second = app.status().unwrap().verb;
    assert_ne!(first, second, "the next turn picks a different verb");
    assert_eq!(first, WORKING_VERBS[0]);
    assert_eq!(second, WORKING_VERBS[1]);
}

#[test]
fn push_chunk_grows_the_token_tally_pointing_down() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("some words here");
    let before = app.status().unwrap().tokens;
    assert!(before > 0, "streamed text counts tokens");
    assert_eq!(app.status().unwrap().arrow, TokenArrow::Down);
    app.push_chunk(" and more");
    assert!(
        app.status().unwrap().tokens > before,
        "the tally only grows"
    );
}

#[test]
fn count_user_input_adds_tokens_pointing_the_arrow_up() {
    // The user's just-sent message is counted into the tally as uploaded
    // input (arrow ↑), so the status shows `↑ N tokens` while the model
    // spins up before its first chunk.
    let mut app = App::new();
    app.begin_stream();
    app.count_user_input("a user message worth several tokens");
    let status = app.status().unwrap();
    assert!(status.tokens > 0, "the user message is counted");
    assert_eq!(status.arrow, TokenArrow::Up, "uploaded input → ↑");
}

#[test]
fn count_user_input_is_a_noop_when_no_turn_is_active() {
    let mut app = App::new();
    app.count_user_input("nothing is streaming yet");
    assert!(app.status.is_none(), "no status to count into");
}

#[test]
fn the_first_chunk_flips_the_arrow_back_down_after_the_user_input() {
    let mut app = App::new();
    app.begin_stream();
    app.count_user_input("hello");
    let input_tokens = app.status().unwrap().tokens;
    assert_eq!(app.status().unwrap().arrow, TokenArrow::Up);
    app.push_chunk("hi there");
    let status = app.status().unwrap();
    assert_eq!(status.arrow, TokenArrow::Down, "streaming output → ↓");
    assert!(
        status.tokens > input_tokens,
        "the reply's tokens add on top of the counted input"
    );
}

#[test]
fn a_tool_adds_to_the_tally_and_flips_the_arrow_up_without_resetting() {
    // "dont reset the existing token count from response count just add and
    // use up arrow" — the tool's output is *added* to the running total.
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("the streamed reply so far");
    let after_text = app.status().unwrap().tokens;
    app.start_tool("Read", "f", None);
    app.end_tool("a multi line\ntool output blob", true);
    let status = app.status().unwrap();
    assert!(
        status.tokens > after_text,
        "the tool's output is added on top: {} !> {after_text}",
        status.tokens
    );
    assert_eq!(status.arrow, TokenArrow::Up, "arrow flips up after a tool");
}

#[test]
fn streaming_again_after_a_tool_points_the_arrow_back_down() {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Read", "f", None);
    app.end_tool("out", true);
    assert_eq!(app.status().unwrap().arrow, TokenArrow::Up);
    app.push_chunk("more reply text");
    assert_eq!(
        app.status().unwrap().arrow,
        TokenArrow::Down,
        "resuming the reply points the arrow back down"
    );
}

#[test]
fn thinking_chunks_grow_the_tally_pointing_down_without_touching_the_reply() {
    // Reasoning deltas count into the live tally like reply text (they are
    // streamed output, so ↓ — even right after a tool's ↑), but the text
    // itself is opaque: it never reaches the reply buffer.
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("reply so far ");
    app.start_tool("Read", "f", None);
    app.end_tool("out", true);
    let before = app.status().unwrap().tokens;
    assert_eq!(app.status().unwrap().arrow, TokenArrow::Up);
    app.push_thinking("weighing the options carefully");
    let status = app.status().unwrap();
    assert!(status.tokens > before, "reasoning text counts tokens");
    assert_eq!(status.arrow, TokenArrow::Down, "thinking streams down");
    assert_eq!(
        app.streaming_text(),
        Some("reply so far "),
        "the reply buffer is untouched by reasoning text"
    );
}

#[test]
fn push_thinking_is_a_no_op_when_idle() {
    let mut app = App::new();
    app.push_thinking("stray reasoning after the turn ended");
    assert!(app.status().is_none(), "no status conjured up");
    assert_eq!(app.streaming_text(), None);
}

#[test]
fn push_tool_call_progress_grows_the_tally_pointing_down_without_touching_the_reply() {
    // While the model *generates* a tool call, the streamed name/argument
    // fragments count into the tally (arrow ↓ — model output) so the status
    // keeps ticking, but the opaque JSON never reaches the reply buffer.
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("let me check ");
    let before = app.status().unwrap().tokens;
    app.push_tool_call_progress(r#"bash{"command":"ls -la"}"#);
    let status = app.status().unwrap();
    assert!(
        status.tokens > before,
        "the tool-call fragment counts tokens"
    );
    assert_eq!(status.arrow, TokenArrow::Down, "generation streams down");
    assert_eq!(
        app.streaming_text(),
        Some("let me check "),
        "the reply buffer is untouched by tool-call JSON"
    );
}

#[test]
fn push_tool_call_progress_is_a_no_op_when_idle() {
    let mut app = App::new();
    app.push_tool_call_progress(r#"{"command":"ls"}"#);
    assert!(app.status().is_none(), "no status conjured up");
    assert_eq!(app.streaming_text(), None);
}

#[test]
fn apply_usage_snaps_the_tally_to_the_real_total() {
    // The provider's usage frame counts what the estimate never saw (the
    // system prompt, the re-sent context), so it REPLACES the ticked
    // estimate rather than adding to it — and later estimates tick on
    // top of the snapped base (docs/prompt-caching.md).
    use crate::stream::TokenUsage;
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("a streamed reply estimate ");
    app.apply_usage(&TokenUsage {
        input: 8080,
        output: 20,
        cached: 8063,
        cache_write: 0,
        ..TokenUsage::default()
    });
    assert_eq!(
        app.status().unwrap().tokens,
        8100,
        "the tally snapped to the real input+output"
    );
    let before = app.status().unwrap().tokens;
    app.push_chunk("next round streaming ");
    assert!(
        app.status().unwrap().tokens > before,
        "the next round's estimate ticks on top of the snapped base"
    );
}

#[test]
fn apply_usage_accumulates_across_agent_rounds() {
    // An agentic turn reports one usage frame per round; the tally is the
    // billed sum, not the last round alone.
    use crate::stream::TokenUsage;
    let mut app = App::new();
    app.begin_stream();
    let round = |input, output, cached| TokenUsage {
        input,
        output,
        cached,
        cache_write: 0,
        ..TokenUsage::default()
    };
    app.apply_usage(&round(1000, 50, 0));
    app.apply_usage(&round(1200, 30, 900));
    assert_eq!(app.status().unwrap().tokens, 2280, "both rounds billed");
}

#[test]
fn apply_usage_is_a_no_op_when_idle() {
    use crate::stream::TokenUsage;
    let mut app = App::new();
    app.apply_usage(&TokenUsage {
        input: 10,
        output: 10,
        cached: 0,
        cache_write: 0,
        ..TokenUsage::default()
    });
    assert!(app.status().is_none(), "no status conjured up");
    app.begin_stream();
    assert_eq!(
        app.status().unwrap().tokens,
        0,
        "an idle report never leaks into the next turn"
    );
}

#[test]
fn the_turn_summary_carries_the_real_usage() {
    // The committed `Done for Ns` summary appends the billed tokens and
    // their cached share when the provider reported usage — and the next
    // turn starts from zero (the accumulators are per-turn).
    use crate::stream::TokenUsage;
    let mut app = App::new();
    app.begin_stream();
    app.apply_usage(&TokenUsage {
        input: 8080,
        output: 123,
        cached: 8063,
        cache_write: 17,
        ..TokenUsage::default()
    });
    let summary = app.end_turn(12).expect("a turn was active");
    assert_eq!(summary.tokens, 8203);
    assert_eq!(summary.cached, 8063);
    assert_eq!(
        summary.cache_write, 17,
        "the written share rides the receipt too"
    );

    app.begin_stream();
    let summary = app.end_turn(1).expect("second turn");
    assert_eq!(summary.tokens, 0, "a usage-less turn reports none");
    assert_eq!(summary.cached, 0);
    assert_eq!(summary.cache_write, 0);
}

#[test]
fn set_status_times_writes_the_boundary_times_onto_the_status() {
    let mut app = App::new();
    app.begin_stream();
    app.set_status_times(Duration::from_secs(7), Some(Duration::from_secs(2)));
    let status = app.status().unwrap();
    assert_eq!(status.elapsed, Duration::from_secs(7));
    assert_eq!(status.thinking, Some(Duration::from_secs(2)));
    // Thinking ends → the suffix is dropped.
    app.set_status_times(Duration::from_secs(8), None);
    assert_eq!(app.status().unwrap().thinking, None);
}

#[test]
fn set_status_times_is_a_no_op_when_idle() {
    let mut app = App::new();
    // No turn → nothing to write.
    app.set_status_times(Duration::from_secs(5), Some(Duration::from_secs(1)));
    assert!(app.status().is_none());
}

#[test]
fn a_fresh_turn_has_no_retry() {
    let mut app = App::new();
    app.begin_stream();
    assert_eq!(app.status().unwrap().retry, None);
}

#[test]
fn set_retry_records_the_attempt_on_the_status() {
    let mut app = App::new();
    app.begin_stream();
    app.set_retry(2, 3);
    assert_eq!(
        app.status().unwrap().retry,
        Some(RetryInfo { attempt: 2, max: 3 })
    );
}

#[test]
fn set_retry_is_a_no_op_when_idle() {
    let mut app = App::new();
    app.set_retry(1, 3);
    assert!(app.status().is_none());
}

#[test]
fn streamed_content_clears_the_retry_indicator() {
    // A retry means a request failed before any byte; once content flows the
    // attempt has succeeded, so the live "retrying" indicator must clear.
    let mut app = App::new();
    app.begin_stream();
    app.set_retry(1, 3);
    app.push_chunk("hello");
    assert_eq!(app.status().unwrap().retry, None, "a chunk clears it");

    app.set_retry(2, 3);
    app.push_thinking("hmm");
    assert_eq!(
        app.status().unwrap().retry,
        None,
        "a reasoning delta clears it too"
    );
}

#[test]
fn end_turn_records_a_summary_and_clears_the_status() {
    let mut app = App::new();
    app.begin_stream();
    let done_verb = app.status().unwrap().done_verb;
    app.push_chunk("a reply");
    app.finish_stream();
    let summary = app.end_turn(20).expect("a turn was active");
    assert_eq!(summary.verb, done_verb, "the summary uses the done verb");
    assert_eq!(summary.secs, 20, "the boundary-supplied duration");
    assert_eq!(
        app.history.last(),
        Some(&HistoryItem::Summary(summary.clone())),
        "the summary is recorded in history so it survives a resize"
    );
    assert!(!app.turn_active(), "the status clears when the turn ends");
}

#[test]
fn end_turn_when_idle_returns_none_and_records_nothing() {
    let mut app = App::new();
    assert!(app.end_turn(3).is_none());
    assert!(app.history.is_empty());
}

#[test]
fn take_turn_summary_builds_without_recording_then_record_pushes() {
    // The split behind end_turn (docs/background.md): take_turn_summary
    // clears the status and builds the summary but does NOT record it, so
    // the boundary can settle a background completion pending at turn end
    // *between* the two — landing the notice above the Done summary in
    // history. record_turn_summary then pushes it.
    let mut app = App::new();
    app.begin_stream();
    let summary = app.take_turn_summary(5).expect("a turn was active");
    assert!(!app.turn_active(), "the status is cleared");
    assert!(
        !app.history
            .iter()
            .any(|i| matches!(i, HistoryItem::Summary(_))),
        "take_turn_summary does not record the summary"
    );
    app.record_turn_summary(summary.clone());
    assert_eq!(
        app.history.last(),
        Some(&HistoryItem::Summary(summary)),
        "record_turn_summary pushes it into history"
    );
}

#[test]
fn take_turn_summary_is_none_for_an_idle_or_shell_turn() {
    let mut idle = App::new();
    assert!(idle.take_turn_summary(1).is_none(), "no turn active");
    let mut shell = App::new();
    shell.begin_shell("sleep 1");
    assert!(
        shell.take_turn_summary(1).is_none(),
        "a `!` shell turn has no Done summary — its cell is the record"
    );
    assert!(!shell.turn_active(), "but the status still clears");
}

#[test]
fn a_completion_pending_at_turn_end_records_above_the_summary() {
    // The turn-end settle ordering (docs/background.md): a background
    // shell that finished during the final assistant text — with no tool
    // call after it to settle at — must still land its notice ABOVE the
    // Done summary, in both history and (via the same order) scrollback,
    // matching the mid-turn tool-boundary placement. This replays the
    // exact app-call sequence the StreamDone arm runs.
    let mut app = App::new();
    app.set_clock(|| STAMP.to_string());
    app.record_user_message("start the server then stop it");
    app.begin_stream();
    app.bg_started(
        "bash_1",
        "python3 server.py",
        Some("API server".into()),
        true,
        None,
    );
    let completion = app.bg_exited("bash_1", None, false).expect("it finished");
    app.defer_bg_completion(completion);
    app.push_chunk("Done — the server was stopped.");
    let _ = app.finish_stream();
    // The StreamDone sequence: build the summary (status cleared, not yet
    // recorded), settle the held completion, then record the summary.
    let summary = app.take_turn_summary(7).expect("a turn was active");
    for completion in app.take_pending_bg_completions() {
        app.record_background_notice(&completion);
    }
    app.record_turn_summary(summary);
    let kinds: Vec<&str> = app
        .history
        .iter()
        .map(|item| match item {
            HistoryItem::Message(_) => "message",
            HistoryItem::Tool(_) => "tool",
            HistoryItem::Background(_) => "background",
            HistoryItem::Summary(_) => "summary",
            HistoryItem::AgentGroup(_) => "agent_group",
            HistoryItem::AgentNotice(_) => "agent_notice",
            HistoryItem::Compaction(_) => "compaction",
            HistoryItem::Reasoning(_) => "reasoning",
            HistoryItem::TaskCall(_) => "task_call",
            HistoryItem::HookNote(_) => "hook_note",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["message", "message", "background", "summary"],
        "user, assistant reply, THEN the notice, THEN the Done summary"
    );
}

#[test]
fn end_turn_uses_the_clock_for_the_summary_timestamp() {
    let mut app = App::new();
    app.set_clock(|| STAMP.to_string());
    app.begin_stream();
    app.finish_stream();
    let summary = app.end_turn(5).unwrap();
    assert_eq!(summary.timestamp, STAMP, "stamped like every recorded item");
}

#[test]
fn fail_stream_clears_the_status_without_a_summary() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("half a reply");
    app.fail_stream("network down").expect("was streaming");
    assert!(!app.turn_active(), "an error clears the live status");
    assert!(
        !app.history
            .iter()
            .any(|i| matches!(i, HistoryItem::Summary(_))),
        "no Done summary is recorded for a failed turn"
    );
}

#[test]
fn a_full_turn_records_user_assistant_then_a_summary_in_order() {
    let mut app = App::new();
    app.record_user_message("q");
    app.begin_stream();
    app.push_chunk("a");
    app.finish_stream();
    app.end_turn(3);
    match app.history.as_slice() {
        [
            HistoryItem::Message(u),
            HistoryItem::Message(a),
            HistoryItem::Summary(s),
        ] => {
            assert_eq!(u.role, Role::User);
            assert_eq!(a.role, Role::Assistant);
            assert!(DONE_VERBS.contains(&s.verb));
        }
        other => panic!("unexpected history: {other:?}"),
    }
}

#[test]
fn search_lists_matching_entries_newest_first() {
    let history = history_of(&["git status", "cargo build", "git push"]);
    assert_eq!(history.search("git"), vec![2, 0]);
    assert_eq!(history.entry(2), Some("git push"));
    assert_eq!(history.entry(0), Some("git status"));
}

#[test]
fn ctrl_u_clears_the_query_back_to_idle_and_restores_the_draft() {
    let mut app = searchable_app(&["git status"]);
    app.input = TextArea::from_text("a draft");
    app.on_key(ctrl('r'));
    type_query(&mut app, "git");
    assert_eq!(app.input.text(), "git status");
    app.on_key(ctrl('u'));
    let search = app.history_search.as_ref().expect("search open");
    assert_eq!(search.query, "");
    assert_eq!(search.state, SearchState::Idle);
    assert_eq!(app.input.text(), "a draft");
}

#[test]
fn esc_mid_turn_cancels_the_search_not_the_turn() {
    let mut app = searchable_app(&["git status"]);
    app.begin_stream();
    app.on_key(ctrl('r'));
    assert_eq!(
        app.on_key(key(KeyCode::Esc)),
        Action::None,
        "search-cancel wins over interrupt, like palette-dismiss"
    );
    assert!(app.history_search.is_none());
    assert!(app.is_streaming(), "the turn keeps streaming");
}

#[test]
fn highlight_ranges_cover_the_query_in_the_preview_case_insensitively() {
    let mut app = searchable_app(&["Git status on git repo"]);
    app.on_key(ctrl('r'));
    type_query(&mut app, "git");
    let text = app.input.text();
    let ranges = app.search_highlight_ranges();
    let covered: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
    assert_eq!(covered, vec!["Git", "git"]);
}

#[test]
fn begin_shell_records_the_command_and_runs_it_as_the_turn_tool() {
    let mut app = App::new();
    app.begin_shell("ls -la");
    assert!(app.turn_active(), "the status strip shows");
    assert!(
        app.is_streaming(),
        "an empty stream buffer so the strip shows and mid-run Enter queues"
    );
    // The command itself is the cell's header — a Role::Shell message
    // recorded up front so a mid-run resize repaints it.
    match app.history.as_slice() {
        [HistoryItem::Message(m)] => {
            assert_eq!(m.role, Role::Shell);
            assert_eq!(m.text, "ls -la");
        }
        other => panic!("unexpected history: {other:?}"),
    }
    let tool = app.current_tool().expect("the command runs as a tool");
    assert_eq!(tool.name, "ls -la");
    assert!(tool.shell, "marked shell so it renders headerless (⎿ only)");
    assert_eq!(tool.status, ToolStatus::Running);
    let status = app.status().expect("a live status");
    assert_eq!(status.verb, SHELL_VERB);
    assert!(status.shell);
}

#[test]
fn a_shell_turn_ends_without_a_summary_or_phantom_message() {
    let mut app = App::new();
    app.begin_shell("echo hi");
    app.end_tool("hi", true);
    assert!(app.finish_stream().is_none(), "no assistant text to record");
    assert!(
        app.end_turn(2).is_none(),
        "no `Ran for Ns` line — the cell itself is the record"
    );
    assert!(!app.turn_active(), "the status is still cleared");
    // history = [Message(Shell), Tool] — the mock's two-line cell.
    match app.history.as_slice() {
        [HistoryItem::Message(m), HistoryItem::Tool(t)] => {
            assert_eq!(m.role, Role::Shell);
            assert_eq!(t.name, "echo hi");
        }
        other => panic!("unexpected history: {other:?}"),
    }
}

#[test]
fn interrupting_a_shell_turn_resolves_the_command_as_failed() {
    let mut app = App::new();
    app.begin_shell("sleep 5");
    let InterruptedTurn::Kept { tools, notice, .. } =
        app.interrupt_turn().expect("a turn was in flight")
    else {
        panic!("a shell turn has a running tool, so it is kept, not undone");
    };
    let [tool] = tools.as_slice() else {
        panic!("the running command is resolved: {tools:?}");
    };
    assert_eq!(tool.name, "sleep 5");
    assert_eq!(tool.status, ToolStatus::Failed);
    assert_eq!(tool.output, INTERRUPT_TOOL_OUTPUT);
    assert!(app.current_tool().is_none());
    assert!(!app.turn_active());
    // Req 2: the `⎿ Interrupted by user` cell is the record — a shell turn
    // commits no redundant `Conversation interrupted` notice.
    assert_eq!(notice, None, "shell interrupt commits no notice");
    assert!(
        !roles(&app).contains(&Role::Error),
        "no `Conversation interrupted` error message for a shell interrupt"
    );
}

#[test]
fn esc_with_only_non_user_history_still_quits() {
    // Nothing to backtrack to — a summary/error/system-only history has no
    // user prompt to edit, so Esc keeps its old idle meaning: quit.
    let mut app = App::new();
    app.begin_stream();
    app.finish_stream();
    app.end_turn(1); // history holds just the turn summary
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Quit);
    assert!(!app.backtrack.primed);
}

#[test]
fn esc_mid_turn_still_interrupts_not_primes() {
    let mut app = App::new();
    exchange(&mut app, "hello", "hi");
    app.begin_stream();
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Interrupt);
    assert!(!app.backtrack.primed, "interrupt wins while a turn runs");
}

#[test]
fn esc_in_the_overlay_still_closes_it_mid_turn() {
    // While a turn streams the transcript can't be rewound mid-flight;
    // Esc keeps meaning "back to the chat" (interrupting stays a
    // conversation-view gesture).
    let mut app = App::new();
    exchange(&mut app, "hello", "hi");
    app.begin_stream();
    app.on_key(ctrl('o'));
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::ToggleToolView);
    assert_eq!(app.view, View::Conversation);
    assert_eq!(app.backtrack.selected, None);
}

#[test]
fn set_thinking_seeds_the_state() {
    let mut app = App::new();
    assert!(app.thinking.is_none(), "unknown/unsupported by default");
    app.set_thinking(Some((
        trio_support(),
        ThinkingMode::Effort(ReasoningEffort::Medium),
    )));
    let state = app.thinking.as_ref().expect("seeded");
    assert_eq!(state.mode, ThinkingMode::Effort(ReasoningEffort::Medium));
    assert_eq!(state.support, trio_support());
    app.set_thinking(None);
    assert!(
        app.thinking.is_none(),
        "a switch to a non-reasoner clears it"
    );
}

#[test]
fn ctrl_t_cycles_the_thinking_mode() {
    // Ctrl+T — the cycle moved off Shift+Tab, which cycles the permission
    // mode now (docs/reasoning.md).
    let mut app = App::new();
    app.set_thinking(Some((
        trio_support(),
        ThinkingMode::Effort(ReasoningEffort::Medium),
    )));
    assert_eq!(
        app.on_key(ctrl('t')),
        Action::SetThinking(ThinkingMode::Effort(ReasoningEffort::High)),
        "medium steps to high"
    );
    assert_eq!(
        app.thinking.as_ref().unwrap().mode,
        ThinkingMode::Effort(ReasoningEffort::High),
        "the state advanced too"
    );
    assert_eq!(
        app.on_key(ctrl('t')),
        Action::SetThinking(ThinkingMode::Off),
        "high wraps to off"
    );
    assert_eq!(
        app.on_key(ctrl('t')),
        Action::SetThinking(ThinkingMode::Effort(ReasoningEffort::Low)),
        "off steps to low"
    );
}

#[test]
fn backtab_keeps_the_draft_intact() {
    // The mode cycle only reads the session state — a typed draft survives
    // it untouched (both Shift+Tab spellings and Ctrl+T alike).
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Manual));
    app.set_thinking(Some((trio_support(), ThinkingMode::Off)));
    type_chars(&mut app, "keep me");
    app.on_key(backtab());
    assert_eq!(app.input.text(), "keep me");
    app.on_key(ctrl('t'));
    assert_eq!(app.input.text(), "keep me");
}

#[test]
fn ctrl_t_mid_turn_cycles_for_the_next_turn() {
    // Like /model, the cycle never touches the running turn — the new mode
    // simply rides the next request.
    let mut app = App::new();
    app.set_thinking(Some((
        trio_support(),
        ThinkingMode::Effort(ReasoningEffort::Medium),
    )));
    app.begin_stream();
    assert_eq!(
        app.on_key(ctrl('t')),
        Action::SetThinking(ThinkingMode::Effort(ReasoningEffort::High))
    );
    assert!(app.turn_active(), "the turn keeps running underneath");
}

#[test]
fn slash_model_opens_the_picker_mid_turn() {
    // /model only swaps the composer, never the running turn, so it opens
    // regardless of turn state (docs/toast.md). The *loop* does the fetch.
    let mut app = App::new();
    app.begin_stream();
    type_chars(&mut app, "/model");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenModelPicker);
    assert!(app.turn_active(), "the turn keeps running underneath");
}

#[test]
fn end_turn_snapshots_the_running_shell_count() {
    let mut app = app_with_shells(&["a", "b", "c"]);
    app.begin_stream();
    let summary = app.end_turn(22).expect("a summary");
    assert_eq!(summary.shells, 3, "Done for 22s · 3 shells still running");
    // With none running the suffix stays off.
    let mut idle = App::new();
    idle.begin_stream();
    assert_eq!(idle.end_turn(2).unwrap().shells, 0);
}

#[test]
fn record_hook_note_slots_after_the_flushed_segment() {
    // The Stop-continuation dance (docs/hooks.md): the loop flushes the
    // streamed text, records the note, and the next round streams a fresh
    // segment — three history items, in order, and the buffer stays open.
    let mut app = App::new();
    app.record_user_message("fix it");
    app.begin_stream();
    app.push_chunk("first answer");
    app.flush_streaming_segment();
    app.record_hook_note("Stop hook", "Stop hook feedback:\ntests are red");
    app.push_chunk("second answer");
    app.finish_stream();
    let kinds: Vec<String> = app
        .history
        .iter()
        .map(|item| match item {
            HistoryItem::Message(m) => format!("{:?}:{}", m.role, m.text),
            HistoryItem::HookNote(n) => format!("hook:{}", n.label),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "User:fix it",
            "Assistant:first answer",
            "hook:Stop hook",
            "Assistant:second answer",
        ]
    );
}

#[test]
fn block_prompt_rolls_the_submission_back_and_keeps_hook_notes() {
    // A UserPromptSubmit block (docs/hooks.md): the prompt leaves history
    // (nothing the hook censored reaches a later context), the text returns
    // to the composer, a SessionStart hook's note recorded after it
    // survives, and the red notice carries the reason alone.
    let mut app = App::new();
    app.record_user_message("here is my secret");
    app.begin_stream();
    app.record_hook_note("SessionStart hook", "the build id is ZX-4417");
    let generation = app.history_generation();
    assert!(app.block_prompt("no secrets in prompts"));
    assert_eq!(
        app.input.text(),
        "here is my secret",
        "back in the composer"
    );
    let kinds: Vec<String> = app
        .history
        .iter()
        .map(|item| match item {
            HistoryItem::Message(m) => format!("{:?}:{}", m.role, m.text),
            HistoryItem::HookNote(n) => format!("hook:{}", n.label),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "hook:SessionStart hook".to_string(),
            "Error:UserPromptSubmit hook blocked the prompt\nReason: no secrets in prompts"
                .to_string(),
        ],
        "the prompt is gone, the note and the reason-only notice remain"
    );
    assert!(
        app.history_generation() > generation,
        "a non-append mutation bumps the transcript cache's generation"
    );
    assert!(!app.is_streaming(), "the turn is over");
    assert!(app.status.is_none(), "no summary will be recorded");
    // A stale event (no turn open) rolls nothing.
    assert!(!app.block_prompt("again"));
}
