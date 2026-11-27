//! `/compact`, auto-compaction, and the context gauge (`docs/compact.md`).

use super::*;

// --- /compact (docs/compact.md) ---

#[test]
fn the_palette_lists_compact_with_a_concise_description() {
    // Codex's verbatim description was the palette's longest row (59 columns
    // — the first to wrap at the default width); the concise rewording says
    // the same thing inside the standard 80-column description room.
    let cmd = COMMANDS
        .iter()
        .find(|c| c.name == "compact")
        .expect("/compact is registered");
    assert_eq!(
        cmd.description,
        "Summarize the conversation to free up context"
    );
    assert_eq!(cmd.effect, CommandEffect::Compact);
}

#[test]
fn slash_compact_dispatches_the_compact_action_when_idle() {
    let mut app = App::new();
    app.record_user_message("hello");
    type_str(&mut app, "/compact");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Compact);
    assert!(app.input.is_empty());
}

#[test]
fn compact_mid_turn_is_rejected_with_a_toast() {
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_stream();
    type_str(&mut app, "/compact");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Toast(COMPACT_BUSY_NOTICE.to_string()),
    );
    assert!(app.turn_active(), "the running turn is untouched");
}

#[test]
fn compact_with_nothing_to_compact_is_rejected_with_a_toast() {
    let mut app = App::new();
    type_str(&mut app, "/compact");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Toast(COMPACT_EMPTY_NOTICE.to_string()),
    );
}

#[test]
fn begin_compact_starts_a_fixed_verb_turn_without_advancing_the_cycle() {
    let mut app = App::new();
    app.begin_compact(false);
    assert!(app.turn_active());
    assert!(
        app.is_streaming(),
        "the strip shows while the summary streams"
    );
    assert!(app.is_compacting());
    assert_eq!(app.status().expect("a compact status").verb, COMPACT_VERB);
    app.finish_compact(0);
    // The cycled per-turn verbs are unaffected: the next real turn still
    // picks the first (the verb-sequence contract).
    app.begin_stream();
    assert_eq!(app.status().unwrap().verb, STATUS_VERBS[0].working);
}

#[test]
fn compact_chunks_divert_to_the_buffer_and_never_render() {
    let mut app = App::new();
    app.begin_compact(false);
    app.push_chunk("the summary ");
    app.push_chunk("text");
    assert_eq!(
        app.streaming_text(),
        Some(""),
        "the visible reply buffer stays empty — the summary is never rendered"
    );
    assert!(app.status().unwrap().tokens > 0, "the tally still ticks");
    let compaction = app.finish_compact(0).expect("a marker");
    assert_eq!(compaction.summary, "the summary text");
}

#[test]
fn finish_compact_appends_the_marker_and_ends_the_turn_without_a_summary() {
    let mut app = App::new();
    app.record_user_message("hello");
    let before = app.history.len();
    app.begin_compact(false);
    app.push_chunk("gist\n");
    let compaction = app.finish_compact(0).expect("a marker");
    assert_eq!(compaction.summary, "gist", "the streamed text, trimmed");
    assert_eq!(app.history.len(), before + 1);
    assert!(matches!(app.history.last(), Some(HistoryItem::Compaction(c)) if c.summary == "gist"));
    assert!(!app.turn_active(), "the status cleared");
    assert!(!app.is_streaming());
    assert!(!app.is_compacting());
    assert!(
        app.end_turn(3).is_none(),
        "no Done-for-Ns summary for a compact turn — the marker cell is the record"
    );
}

#[test]
fn a_compact_turn_with_no_streamed_text_still_appends_an_empty_marker() {
    // The derivation substitutes codex's "(no summary available)" for the
    // empty summary — the marker still lands so the state is visible.
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_compact(false);
    let compaction = app.finish_compact(0).expect("a marker");
    assert_eq!(compaction.summary, "");
}

#[test]
fn esc_mid_compact_keeps_the_old_history_and_records_the_interrupt_notice() {
    // The swap lives only at StreamDone (codex: replace-at-the-very-end) —
    // an interrupt drops the half summary and leaves the context as it was.
    // The tail being a user message must NOT trigger the interrupt-undo
    // (that would pull an unrelated old message into the composer).
    let mut app = App::new();
    app.record_user_message("hello");
    let before = app.history.clone();
    app.begin_compact(false);
    app.push_chunk("half a summ");
    let outcome = app.interrupt_turn(0).expect("an interrupt outcome");
    assert!(
        matches!(
            &outcome,
            InterruptedTurn::Kept {
                partial: None,
                tools,
                notice: Some(_),
                ..
            } if tools.is_empty()
        ),
        "{outcome:?}"
    );
    assert!(!app.is_compacting(), "the half summary is dropped");
    assert_eq!(app.history[..before.len()], before[..]);
    assert!(
        !app.history
            .iter()
            .any(|i| matches!(i, HistoryItem::Compaction(_))),
        "no marker on interrupt"
    );
    assert!(app.input.is_empty(), "no interrupt-undo composer refill");
}

#[test]
fn an_interrupted_compaction_keeps_no_receipt() {
    // A compaction's record is its `● Context compacted` cell, which an Esc
    // never earns — and its fixed `Compacting` verb has no past tense for a
    // summary to open with (docs/interrupt.md).
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_compact(false);
    app.push_chunk("half a summ");
    let outcome = app.interrupt_turn(6).expect("an interrupt outcome");
    assert!(
        matches!(&outcome, InterruptedTurn::Kept { summary: None, .. }),
        "{outcome:?}"
    );
}

#[test]
fn a_backend_error_mid_compact_drops_the_buffer_and_records_the_notice() {
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_compact(false);
    app.push_chunk("half");
    let failure = app.fail_stream("boom").expect("a failure record");
    assert_eq!(
        failure.partial, None,
        "the half summary never becomes an assistant message"
    );
    assert!(!app.is_compacting());
    assert!(
        !app.history
            .iter()
            .any(|i| matches!(i, HistoryItem::Compaction(_)))
    );
    assert!(matches!(app.history.last(), Some(HistoryItem::Message(m)) if m.role == Role::Error));
}

#[test]
fn clear_conversation_drops_the_compact_state() {
    let mut app = App::new();
    app.record_user_message("hi");
    app.begin_compact(false);
    app.push_chunk("half");
    app.clear_conversation();
    assert!(!app.is_compacting());
    assert!(!app.turn_active());
    assert!(app.history.is_empty());
}

#[test]
fn clear_leaves_the_gauge_reading_exactly_what_a_fresh_session_reads() {
    // /clear promises a fresh slate, so its footer must be indistinguishable
    // from the session that just booted in the same cwd with the same model.
    // The gauge used to re-seat on the standing system prompt + AGENTS.md
    // instructions (they do ride the next request) — but a *booted* session
    // carries those same constants and reads 0, so the same empty
    // conversation showed two different numbers and the post-clear one read
    // as leftover conversation state (`139/1M` on an empty screen).
    let standing = |app: &mut App| {
        app.set_system_prompt(Some("a system prompt".to_string()));
        app.set_user_instructions(Some("standing project instructions".to_string()));
    };
    let fresh = {
        let mut app = App::new();
        standing(&mut app);
        app
    };
    let mut app = App::new();
    standing(&mut app);
    app.record_user_message("hello there");
    app.begin_stream();
    app.apply_usage(&usage_of(5_000, 0));
    app.finish_stream();
    app.end_turn(1);
    assert!(app.context_used() > 0, "a real conversation counts");
    app.clear_conversation();
    assert_eq!(
        app.context_used(),
        fresh.context_used(),
        "a cleared session reads exactly like a fresh one"
    );
    assert_eq!(app.context_used(), 0, "and that reading is a true zero");
}

#[test]
fn the_gauge_reads_zero_exactly_when_there_is_nothing_to_compact() {
    // The gauge's "is there a conversation?" test is the very predicate
    // /compact uses for its `Nothing to compact` rejection, so the footer and
    // the command can never disagree about whether anything is there: no
    // derived context → a true zero (the system prompt and the standing
    // AGENTS.md instructions are session constants, not conversation, and a
    // freshly booted session reads 0 with both of them loaded); any derived
    // context → they are counted along with it.
    let mut app = App::new();
    app.set_system_prompt(Some("a system prompt".to_string()));
    app.set_user_instructions(Some("standing project instructions".to_string()));
    let agree = |app: &App| {
        let derived = crate::context::context_messages(&app.history).is_empty();
        assert_eq!(
            derived,
            app.context_used() == 0,
            "gauge {} vs derived-empty {derived}",
            app.context_used()
        );
    };
    app.refresh_context_used();
    agree(&app);
    assert_eq!(app.context_used(), 0, "an empty conversation");
    // A notice is not chat, but the model IS told about it (it derives as a
    // user-role entry), so it counts — and /compact would have something to
    // summarize. The two move together, which is the invariant.
    app.record_system_message("a notice");
    app.refresh_context_used();
    agree(&app);
    assert!(app.context_used() > 0);
    // A real exchange, then a clear: back to a true zero, in step again.
    app.record_user_message("hello there");
    app.begin_stream();
    app.finish_stream();
    app.end_turn(1);
    app.refresh_context_used();
    agree(&app);
    app.clear_conversation();
    agree(&app);
    assert_eq!(app.context_used(), 0);
}

#[test]
fn an_instructions_only_context_has_nothing_to_compact() {
    // The instructions are derived context, not conversation: with no
    // history they must not make /compact or auto-compact think there is
    // something to summarize (both emptiness checks deliberately derive
    // WITHOUT them — this pins that).
    let mut app = App::new();
    app.set_user_instructions(Some("standing project instructions".to_string()));
    type_str(&mut app, "/compact");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Toast(COMPACT_EMPTY_NOTICE.to_string()),
        "an instructions-only context is still nothing to compact"
    );
    // And the auto trigger stays put even far past the threshold.
    app.set_context_window(Some(10));
    app.begin_stream();
    app.apply_usage(&usage_of(1_000, 0));
    app.finish_stream();
    app.end_turn(1);
    assert!(
        !app.can_compact_idle(),
        "nothing compacts on an instructions-only context"
    );
}

#[test]
fn set_context_window_ignores_a_zero() {
    let mut app = App::new();
    app.set_context_window(Some(0));
    assert_eq!(app.context_window(), None);
    app.set_context_window(Some(100));
    assert_eq!(app.context_window(), Some(100));
}

// --- background compaction (docs/background-compaction.md) ---

#[test]
fn the_compaction_window_is_the_gauges_while_auto_compact_is_on() {
    // The boundary configures the turn's slot with it: no window, or the
    // `/settings` **Auto compact** knob off, means no compaction on its own
    // (`/compact` by hand still works).
    let mut app = App::new();
    assert_eq!(app.compaction_window(), None, "no window known");
    app.set_context_window(Some(200_000));
    assert_eq!(app.compaction_window(), Some(200_000));
    app.settings_mut().auto_compact = false;
    assert_eq!(app.compaction_window(), None);
}

#[test]
fn only_an_idle_conversation_compacts_between_turns() {
    let mut app = App::new();
    assert!(!app.can_compact_idle(), "nothing to summarize");
    app.record_user_message("port the payments module");
    assert!(app.can_compact_idle());
    app.begin_stream();
    assert!(
        !app.can_compact_idle(),
        "a running turn compacts at its own round tops"
    );
}

#[test]
fn a_background_compaction_anchors_where_it_starts() {
    let mut app = App::new();
    app.record_user_message("port the payments module");
    app.begin_background_compaction(7);
    let job = app.background_compaction().expect("a job");
    assert_eq!(job.id, 7);
    assert_eq!(job.keep_from, app.history.len(), "everything so far");
    assert_eq!(job.summary, None, "still being written");
}

#[test]
fn a_written_summary_is_held_until_it_lands() {
    let mut app = App::new();
    app.record_user_message("port the payments module");
    app.begin_background_compaction(1);
    app.record_user_message("and the ledger");
    let length = app.history.len();
    assert!(app.background_compaction_written(1, "ported stripe.rs".into(), 47));
    assert_eq!(
        app.background_compaction()
            .and_then(|job| job.summary.as_deref()),
        Some("ported stripe.rs")
    );
    assert_eq!(app.history.len(), length, "held, not applied");
}

#[test]
fn landing_appends_an_anchored_background_marker() {
    let mut app = App::new();
    app.set_context_window(Some(200_000));
    app.record_user_message("port the payments module");
    let anchor = app.history.len();
    app.begin_background_compaction(1);
    // Recorded while the summary was being written: kept verbatim.
    app.record_user_message("and the ledger");
    app.background_compaction_written(1, "ported stripe.rs".into(), 47);
    let marker = app.land_background_compaction(1).expect("it lands");
    assert_eq!(marker.keep_from, Some(anchor));
    assert!(marker.background && marker.auto, "{marker:?}");
    assert_eq!(marker.secs, 47);
    assert_eq!(marker.summary, "ported stripe.rs");
    assert!(
        matches!(app.history.last(), Some(HistoryItem::Compaction(last)) if *last == marker),
        "appended"
    );
    assert!(app.background_compaction().is_none(), "the job is done");
    let ctx = crate::context::context_messages(&app.history);
    let last = ctx.last().expect("a context");
    assert!(
        last.text.ends_with("and the ledger"),
        "the message after the anchor stays verbatim: {ctx:?}"
    );
}

#[test]
fn the_cell_measures_the_context_the_swap_replaced() {
    // The turn kept working while the summary was written: what the summary
    // replaced is the context at the swap, not the smaller one it started
    // from.
    let mut app = App::new();
    app.set_context_window(Some(200_000));
    app.record_user_message("port the payments module");
    app.sync_context_used();
    let at_start = app.context_used();
    app.begin_background_compaction(1);
    app.record_user_message(&"and the ledger, the refunds and the payouts ".repeat(40));
    app.sync_context_used();
    let at_swap = app.context_used();
    assert!(at_swap > at_start, "the context grew meanwhile");
    app.background_compaction_written(1, "ported stripe.rs".into(), 47);
    let marker = app.land_background_compaction(1).expect("it lands");
    assert_eq!(marker.before, at_swap);
}

#[test]
fn the_cycle_starts_where_the_last_compaction_left_the_context() {
    // Read off the history rather than kept beside it, so a /clear, a
    // /resume or a rewind can never leave another conversation's cycle in
    // place.
    let mut app = App::new();
    app.set_context_window(Some(200_000));
    app.record_user_message("port the payments module");
    assert_eq!(app.compaction_cycle_start(), 0, "no compaction yet");
    app.begin_background_compaction(1);
    app.background_compaction_written(1, "ported stripe.rs".into(), 4);
    let marker = app.land_background_compaction(1).expect("it lands");
    app.record_user_message("and the ledger");
    assert_eq!(app.compaction_cycle_start(), marker.after);
    let epoch = app.history_epoch();
    app.clear_conversation();
    assert_eq!(app.compaction_cycle_start(), 0, "a new conversation");
    assert_ne!(app.history_epoch(), epoch);
}

#[test]
fn news_of_another_job_changes_nothing() {
    let mut app = App::new();
    app.record_user_message("port the payments module");
    app.begin_background_compaction(2);
    assert!(!app.background_compaction_written(1, "old".into(), 3));
    assert!(app.land_background_compaction(1).is_none());
    assert_eq!(app.background_compaction().map(|job| job.id), Some(2));
}

#[test]
fn a_job_whose_history_moved_never_lands() {
    // `/clear`, a backtrack, a `/resume`: the summary covers a history that
    // is gone (codex's generation check).
    let mut app = App::new();
    app.record_user_message("port the payments module");
    app.begin_background_compaction(1);
    app.background_compaction_written(1, "ported".into(), 5);
    app.clear_conversation();
    app.record_user_message("something else");
    assert!(app.background_compaction_stale());
    assert!(app.land_background_compaction(1).is_none());
    assert!(
        !app.history
            .iter()
            .any(|item| matches!(item, HistoryItem::Compaction(_))),
        "no marker over the new conversation"
    );
}

#[test]
fn a_job_outlives_changes_that_leave_what_was_said_alone() {
    // A re-render, an agent's entry updated in place, a failed attempt
    // withdrawn: the history's render generation moves while the
    // conversation the summary covers does not — and a summary already paid
    // for is kept.
    let mut app = App::new();
    app.record_user_message("port the payments module");
    app.begin_background_compaction(1);
    app.invalidate_rendered_history();
    assert!(!app.background_compaction_stale());
    assert!(app.background_compaction_written(1, "ported".into(), 5));
    assert!(app.land_background_compaction(1).is_some());
}

#[test]
fn a_job_whose_coverage_was_cut_back_is_stale() {
    // The turn that started it, undone before anything streamed: the
    // message the summary covers went back to the composer.
    let mut app = App::new();
    app.record_user_message("port the payments module");
    app.begin_stream();
    app.begin_background_compaction(1);
    assert_eq!(app.interrupt_turn(0), Some(InterruptedTurn::Undone));
    assert!(app.background_compaction_stale());
    assert!(!app.background_compaction_written(1, "ported".into(), 5));
}

#[test]
fn a_rewind_makes_the_job_stale() {
    let mut app = App::new();
    app.record_user_message("port the payments module");
    app.record_message(Role::Assistant, "ported");
    app.record_user_message("now the tests");
    app.begin_background_compaction(1);
    app.backtrack.selected = Some(1);
    app.confirm_backtrack();
    assert!(app.background_compaction_stale());
}

#[test]
fn a_finished_wait_wears_the_turns_own_verb_again() {
    // A turn that waited at its ceiling and was stopped: the next turn
    // starts on its own verb, not on the last one's `Compacting…`.
    let mut app = App::new();
    app.record_user_message("port the payments module");
    app.begin_stream();
    app.begin_background_compaction(1);
    app.set_compaction_waiting(1);
    app.end_compaction_wait();
    assert_eq!(app.live_verb(), None);
}

#[test]
fn a_background_summarys_usage_counts_since_launch_and_nowhere_else() {
    // It was billed, so the launch's ledger has it; the summarizer's request
    // is not the conversation's, so the last request and the gauge do not.
    let mut app = App::new();
    app.record_user_message("port the payments module");
    let frame = crate::stream::TokenUsage {
        input: 120_000,
        output: 2_000,
        ..crate::stream::TokenUsage::default()
    };
    let used = app.context_used();
    app.record_compaction_usage(&frame);
    let ledger = app.usage_since_launch();
    assert_eq!(
        (ledger.requests, ledger.input, ledger.output),
        (1, 120_000, 2_000)
    );
    assert_eq!(app.last_usage(), None);
    assert_eq!(app.context_used(), used);
}

#[test]
fn waiting_at_the_ceiling_wears_the_compacting_verb() {
    let mut app = App::new();
    app.record_user_message("port the payments module");
    app.begin_stream();
    assert_eq!(app.live_verb(), None, "the turn's own verb");
    app.begin_background_compaction(1);
    assert_eq!(
        app.live_verb(),
        None,
        "a summary being written changes nothing"
    );
    app.set_compaction_waiting(1);
    assert_eq!(app.live_verb(), Some(COMPACT_VERB));
    app.background_compaction_written(1, "s".into(), 2);
    app.land_background_compaction(1);
    assert_eq!(app.live_verb(), None, "back to work after the swap");
}

#[test]
fn the_row_shows_the_summary_being_written_then_held() {
    let mut app = App::new();
    app.set_context_window(Some(200_000));
    app.record_user_message("port the payments module");
    assert_eq!(app.compaction_row(), None);
    app.begin_background_compaction(1);
    app.set_compaction_elapsed(Some(Duration::from_secs(12)));
    assert_eq!(
        app.compaction_row(),
        Some(CompactionRow::Writing {
            elapsed: Duration::from_secs(12)
        })
    );
    app.background_compaction_written(1, "s".into(), 30);
    assert_eq!(
        app.compaction_row(),
        Some(CompactionRow::Held { percent: 85 }),
        "it swaps in at the ceiling: 85% of the window"
    );
}

#[test]
fn finish_compact_records_before_after_and_the_auto_tag() {
    let mut app = App::new();
    app.record_user_message("hello there friend");
    app.set_context_window(Some(1_000));
    app.begin_stream();
    app.apply_usage(&usage_of(500, 0));
    app.finish_stream();
    app.end_turn(1);
    app.begin_compact(true);
    app.push_chunk("the gist");
    let compaction = app.finish_compact(0).expect("a marker");
    assert_eq!(
        compaction.before, 500,
        "the gauge value when compacting began"
    );
    assert!(
        compaction.after > 0,
        "re-estimated from the compacted derivation"
    );
    assert!(compaction.auto);
    assert_eq!(
        app.context_used(),
        compaction.after,
        "the gauge drops to the fresh estimate"
    );
}

#[test]
fn a_manual_compaction_is_not_tagged_auto() {
    let mut app = App::new();
    app.record_user_message("hi");
    app.begin_compact(false);
    app.push_chunk("s");
    assert!(!app.finish_compact(0).expect("a marker").auto);
}

#[test]
fn finish_compact_records_the_turns_elapsed_seconds() {
    // The boundary passes the summarization turn's wall-clock (the
    // take_turn_summary shape) so the cell can show `· 36s` — recorded on
    // the marker and persisted with it (docs/compact.md).
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_compact(false);
    app.push_chunk("gist");
    let compaction = app.finish_compact(36).expect("a marker");
    assert_eq!(compaction.secs, 36);
    assert!(matches!(
        app.history.last(),
        Some(HistoryItem::Compaction(recorded)) if recorded.secs == 36
    ));
}

#[test]
fn clear_conversation_resets_the_context_gauge() {
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_stream();
    app.apply_usage(&usage_of(5_000, 0));
    app.finish_stream();
    app.end_turn(1);
    app.clear_conversation();
    assert_eq!(app.context_used(), 0);
}

#[test]
fn message_tokens_is_the_gauges_rule_one_message_at_a_time() {
    // The Ctrl+D view labels every entry with its own estimate
    // (`docs/context.md`); summed, they must be exactly what the gauge
    // reads, or the view and the footer would disagree about one window.
    let mut app = App::new();
    app.record_user_message_with_images(
        "[Image #1] look",
        vec![std::path::PathBuf::from("/tmp/a.png")],
    );
    app.begin_stream();
    app.push_chunk("let me check");
    app.flush_streaming_segment();
    app.start_tool("Read", "f", None);
    app.end_tool("L1\nL2", true);
    app.finish_stream();
    let messages = crate::context::context_messages(&app.history);
    let summed: usize = messages
        .iter()
        .map(|message| message_tokens(message, &mut count_tokens))
        .sum();
    assert_eq!(summed, estimate_messages_tokens(&messages));
    // It counts with the counter it is handed: the text, each picture's
    // flat charge, and every call's name and arguments.
    let bytes = |message: &crate::context::ContextMessage| {
        message_tokens(message, &mut |text: &str| text.len())
    };
    assert_eq!(
        bytes(&messages[0]),
        messages[0].text.len() + crate::app::types::IMAGE_INPUT_TOKENS
    );
    let call = &messages[1].tool_calls[0];
    assert_eq!(
        bytes(&messages[1]),
        messages[1].text.len() + call.name.len() + call.arguments.len()
    );
}

// --- the gauge's usage-anchored reading (docs/token-estimate.md) ---

/// The estimate of `texts` as user messages, each counted alone — what a
/// tail of plain messages adds to the gauge.
fn estimate_of(texts: &[&str]) -> u64 {
    texts.iter().map(|text| count_tokens(text) as u64).sum()
}

#[test]
fn a_usage_frame_reads_the_providers_count_of_the_request() {
    // The frame's `input` is the request's whole context, counted by the
    // provider — its framing and its own rendering of the tool schemas
    // included. The reply its `output` reports is not context until it is
    // recorded (and its reasoning never is).
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_stream();
    app.apply_usage(&usage_of(5_000, 200));
    assert_eq!(app.context_used(), 5_000);
}

#[test]
fn what_joins_after_a_frame_adds_its_estimate_to_the_providers_count() {
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_stream();
    app.apply_usage(&usage_of(5_000, 200));
    let reply = "a reply of several words, which the next request re-sends";
    app.push_chunk(reply);
    app.finish_stream();
    app.end_turn(1);
    assert_eq!(app.context_used(), 5_000 + estimate_of(&[reply]));
    // A message sent next moves the gauge before its request does.
    app.record_user_message("and a follow-up question");
    app.sync_context_used();
    assert_eq!(
        app.context_used(),
        5_000 + estimate_of(&[reply, "and a follow-up question"])
    );
}

#[test]
fn a_message_moves_the_gauge_before_any_frame_has_counted() {
    // Offline (the dummy reports no usage) the estimate is the whole
    // reading, and a long paste shows at once.
    let mut app = App::new();
    app.record_user_message("first");
    app.sync_context_used();
    let before = app.context_used();
    assert!(before > 0);
    let paste = "a long paste of many words ".repeat(50);
    app.record_user_message(&paste);
    app.sync_context_used();
    assert_eq!(app.context_used(), before + estimate_of(&[&paste]));
}

#[test]
fn a_compaction_keeps_what_the_provider_counted_beyond_the_estimate() {
    // 3k of the provider's count is beyond the estimate (a template's
    // framing, its own rendering of the tool schemas): it rides the request
    // after a compaction just as before, so the marker's `after` and the
    // gauge keep it — where they used to drop to the bare estimate.
    let mut app = App::new();
    app.record_user_message(&"an early request with a lot of detail ".repeat(40));
    app.begin_stream();
    let (estimate, _) = app.estimate_context();
    app.apply_usage(&usage_of(estimate + 3_000, 0));
    app.finish_stream();
    app.end_turn(1);
    app.begin_compact(false);
    app.push_chunk("the gist");
    let compaction = app.finish_compact(0).expect("a marker");
    let (compacted, _) = app.estimate_context();
    assert_eq!(compaction.before, estimate + 3_000);
    assert_eq!(compaction.after, compacted + 3_000);
    assert_eq!(app.context_used(), compacted + 3_000);
}

#[test]
fn a_compaction_turns_frame_neither_reads_nor_teaches() {
    // The summarizer's request is shaped unlike the conversation's — no
    // tools, the compact prompt — so its count measures neither.
    let mut app = App::new();
    app.record_user_message("hello there");
    app.begin_stream();
    app.apply_usage(&usage_of(4_000, 0));
    app.finish_stream();
    app.end_turn(1);
    app.begin_compact(false);
    app.apply_usage(&usage_of(900, 100));
    assert_eq!(app.context_used(), 4_000);
    assert_eq!(app.context_calibration().ratio_permille(), 1000);
}

/// A tool set of `schemas`, each the client's serialized
/// `{"type":"function",…}` entry, named for the gauge tests below.
fn tool_set(schemas: &[&str]) -> std::sync::Arc<[crate::context_usage::ToolDefinition]> {
    schemas
        .iter()
        .enumerate()
        .map(|(i, schema)| crate::context_usage::ToolDefinition {
            name: format!("tool_{i}"),
            schema: (*schema).to_string(),
        })
        .collect()
}

const BASH_SCHEMA: &str = r#"{"type":"function","function":{"name":"bash","description":"Run a shell command in the working directory and return what it printed.","parameters":{"type":"object","properties":{"command":{"type":"string","description":"The command to run."}},"required":["command"]}}}"#;

const SEARCH_SCHEMA: &str = r#"{"type":"function","function":{"name":"mcp__docs__search","description":"Search the project's documentation and return the best matching pages.","parameters":{"type":"object","properties":{"query":{"type":"string","description":"What to look for."}},"required":["query"]}}}"#;

#[test]
fn before_any_report_the_gauge_counts_the_tool_schemas_too() {
    // Every request carries the tool schemas beside the conversation, the
    // first one included: until a provider has counted one, their estimate
    // is in the reading rather than missing from it.
    let mut app = App::new();
    app.set_tool_definitions(tool_set(&[BASH_SCHEMA]));
    app.record_user_message("hello");
    app.sync_context_used();
    assert_eq!(app.context_used(), estimate_of(&["hello", BASH_SCHEMA]));
}

#[test]
fn an_empty_conversation_reads_zero_whatever_the_tools() {
    // The gauge measures a conversation: with none, it reads what a fresh
    // session reads (docs/compact.md), tools or not.
    let mut app = App::new();
    app.set_tool_definitions(tool_set(&[BASH_SCHEMA]));
    app.sync_context_used();
    assert_eq!(app.context_used(), 0);
}

#[test]
fn a_tool_set_that_grows_moves_the_gauge_by_the_new_schemas() {
    // An MCP server connecting adds its tools to every request after it:
    // the reading moves by their estimate before the next request counts
    // them, and the provider's count of the tools it already had stays.
    let mut app = App::new();
    app.set_tool_definitions(tool_set(&[BASH_SCHEMA]));
    app.record_user_message("hello");
    app.begin_stream();
    app.apply_usage(&usage_of(5_000, 0));
    app.finish_stream();
    app.end_turn(1);
    assert_eq!(app.context_used(), 5_000);
    app.set_tool_definitions(tool_set(&[BASH_SCHEMA, SEARCH_SCHEMA]));
    app.sync_context_used();
    assert_eq!(app.context_used(), 5_000 + estimate_of(&[SEARCH_SCHEMA]));
}

#[test]
fn the_same_tool_set_handed_over_again_moves_nothing() {
    // The boundary hands the backend's set over at every loop bottom; a
    // rebuilt backend's equal set in a new handle is no change.
    let mut app = App::new();
    app.set_tool_definitions(tool_set(&[BASH_SCHEMA]));
    app.record_user_message("hello");
    app.begin_stream();
    app.apply_usage(&usage_of(5_000, 0));
    app.finish_stream();
    app.end_turn(1);
    app.set_tool_definitions(tool_set(&[BASH_SCHEMA]));
    app.sync_context_used();
    assert_eq!(app.context_used(), 5_000);
}

/// Two frames whose change the provider counts at three halves of the
/// estimate — the second request carrying `added` more than the first.
fn calibrated_at_three_halves(app: &mut App, added: &str) -> u64 {
    app.record_user_message("hello");
    app.begin_stream();
    let (first, _) = app.estimate_context();
    app.apply_usage(&usage_of(first + 1_000, 0));
    app.record_user_message(added);
    let (second, _) = app.estimate_context();
    let counted = first + 1_000 + (second - first) * 3 / 2;
    app.apply_usage(&usage_of(counted, 0));
    counted
}

#[test]
fn consecutive_frames_calibrate_what_later_estimates_add() {
    let mut app = App::new();
    let counted = calibrated_at_three_halves(&mut app, &"word ".repeat(600));
    let ratio = app.context_calibration().ratio_permille();
    assert!((1_499..=1_500).contains(&ratio), "{ratio}");
    let later = "other ".repeat(600);
    app.record_user_message(&later);
    app.sync_context_used();
    assert_eq!(
        app.context_used(),
        counted + app.context_calibration().correct(estimate_of(&[&later]))
    );
}

#[test]
fn a_model_switch_starts_the_calibration_over() {
    // Another model counts in another tokenizer.
    let mut app = App::new();
    app.set_session_info("first-model", "~");
    let counted = calibrated_at_three_halves(&mut app, &"word ".repeat(600));
    app.set_session_info("first-model", "~/elsewhere");
    assert!(
        app.context_calibration().ratio_permille() > 1_000,
        "the same model keeps it"
    );
    app.set_session_info("second-model", "~");
    assert_eq!(app.context_calibration().ratio_permille(), 1000);
    let later = "other ".repeat(600);
    app.record_user_message(&later);
    app.sync_context_used();
    assert_eq!(app.context_used(), counted + estimate_of(&[&later]));
}
