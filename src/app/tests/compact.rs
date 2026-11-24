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
        !app.should_auto_compact(),
        "auto-compact never fires on an instructions-only context"
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

#[test]
fn auto_compact_triggers_past_ninety_percent_of_the_window() {
    // Codex's threshold: (window * 9) / 10.
    let mut app = App::new();
    app.record_user_message("hello");
    app.set_context_window(Some(1_000));
    app.begin_stream();
    app.apply_usage(&usage_of(950, 0));
    app.finish_stream();
    app.end_turn(1);
    assert!(app.should_auto_compact());
}

#[test]
fn auto_compact_does_not_trigger_at_or_below_the_threshold() {
    let mut app = App::new();
    app.record_user_message("hello");
    app.set_context_window(Some(1_000));
    app.begin_stream();
    app.apply_usage(&usage_of(900, 0));
    app.finish_stream();
    app.end_turn(1);
    assert!(
        !app.should_auto_compact(),
        "900 of 1000 is exactly the line"
    );
}

#[test]
fn auto_compact_needs_a_window_and_derivable_content() {
    let mut app = App::new();
    // Over any threshold but no window known → never.
    app.begin_stream();
    app.apply_usage(&usage_of(1_000_000, 0));
    app.finish_stream();
    app.end_turn(1);
    assert!(!app.should_auto_compact(), "no window, no trigger");
    // A window but an empty conversation → nothing to summarize.
    app.set_context_window(Some(100));
    assert!(!app.should_auto_compact(), "empty context, no trigger");
}

#[test]
fn the_idle_trigger_never_fires_mid_turn() {
    // Mid-turn, compacting is the running agent's own to start, at a round
    // boundary (docs/compact.md *Background compaction*).
    let mut app = App::new();
    app.record_user_message("hello");
    app.set_context_window(Some(100));
    app.begin_stream();
    app.apply_usage(&usage_of(990, 0));
    assert!(!app.should_auto_compact(), "a turn is in flight");
}

#[test]
fn auto_compact_is_not_repeated_right_after_a_compaction() {
    // A compaction that leaves the gauge high must not immediately
    // re-trigger over the conversation it just compacted.
    let mut app = App::new();
    app.record_user_message("hello there my friend");
    app.set_context_window(Some(10)); // tiny: the bridge alone exceeds it
    app.begin_compact(false);
    app.push_chunk("a summary");
    app.finish_compact(0);
    assert!(
        app.context_used() > 9,
        "precondition: still over the threshold after compacting"
    );
    assert!(
        !app.should_auto_compact(),
        "blocked right after a compaction"
    );
}

#[test]
fn an_interrupted_compaction_blocks_the_auto_retrigger() {
    // Esc'ing a compaction must not spawn another one at the same turn end
    // — the user just said no.
    let mut app = App::new();
    app.record_user_message("hello");
    app.set_context_window(Some(10));
    app.begin_stream();
    app.apply_usage(&usage_of(100, 0));
    app.finish_stream();
    app.end_turn(1);
    assert!(app.should_auto_compact(), "precondition: over threshold");
    app.begin_compact(true);
    app.push_chunk("half");
    app.interrupt_turn(0);
    assert!(!app.should_auto_compact(), "an Esc'd compaction stays down");
}

#[test]
fn a_failed_compaction_blocks_the_auto_retrigger() {
    let mut app = App::new();
    app.record_user_message("hello");
    app.set_context_window(Some(10));
    app.begin_compact(true);
    app.push_chunk("half");
    app.fail_stream("boom");
    assert!(!app.should_auto_compact(), "a failed compaction stays down");
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

// --- background compaction: the app's half (docs/compact.md) ---

use crate::app::compact::{BackgroundCompaction, CompactionPhase, SummaryStep};
use crate::stream::StreamEvent;

/// A turn under way on a 50k window: the user's message and one tool round.
fn working_turn() -> App {
    let mut app = App::new();
    app.record_user_message("fix the bug");
    app.set_context_window(Some(50_000));
    app.begin_stream();
    app.start_tool("Bash", "cargo build", None);
    app.end_tool("built", true);
    app
}

/// The running compaction, which the test has just started.
fn running(app: &App) -> &BackgroundCompaction {
    app.background_compaction()
        .expect("a compaction is running")
}

/// Stream `summary` as the summarizer would, to its end.
fn summarize(app: &mut App, summary: &str) {
    assert_eq!(
        app.on_summary_event(&StreamEvent::Chunk(summary.to_string())),
        SummaryStep::Continue
    );
    assert_eq!(
        app.on_summary_event(&StreamEvent::StreamDone),
        SummaryStep::Done
    );
}

#[test]
fn compact_mid_turn_asks_for_a_background_compaction() {
    // No longer refused: the running agent takes a checkpoint at its next
    // round top and keeps working.
    let mut app = working_turn();
    type_str(&mut app, "/compact");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::CompactInBackground);
    assert!(app.compaction_requested());
    assert!(app.turn_active(), "the running turn is untouched");
    assert!(app.input.is_empty());
}

#[test]
fn compact_while_a_compaction_runs_is_refused_with_a_toast() {
    let mut app = working_turn();
    assert!(app.begin_background_compaction(7));
    type_str(&mut app, "/compact");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Toast(COMPACT_BUSY_NOTICE.to_string())
    );
    let mut idle = App::new();
    idle.record_user_message("hi");
    idle.begin_compact(false);
    type_str(&mut idle, "/compact");
    assert_eq!(
        idle.on_key(key(KeyCode::Enter)),
        Action::Toast(COMPACT_BUSY_NOTICE.to_string()),
        "nor over a summarization turn"
    );
}

#[test]
fn a_checkpoint_records_the_cut_where_the_turn_stands() {
    let mut app = working_turn();
    let len = app.history.len();
    let before = app.context_used();
    assert!(app.begin_background_compaction(7));
    assert_eq!(app.history.len(), len + 1);
    assert!(matches!(app.history.last(), Some(HistoryItem::CompactionCut(cut)) if cut.id == 7));
    assert_eq!(
        app.background_compaction_snapshot()
            .map(<[HistoryItem]>::len),
        Some(len),
        "the snapshot is the history up to the cut"
    );
    let compaction = running(&app);
    assert!(compaction.auto, "nobody asked: the agent's own");
    assert_eq!(compaction.phase, CompactionPhase::Summarizing);
    assert_eq!(compaction.before, before);
    assert!(
        !app.begin_background_compaction(8),
        "one compaction at a time"
    );
}

#[test]
fn a_checkpoint_the_user_asked_for_is_not_tagged_auto() {
    let mut app = working_turn();
    assert!(app.request_compaction());
    assert!(app.begin_background_compaction(7));
    assert!(!running(&app).auto);
    assert!(!app.compaction_requested(), "the request was answered");
}

#[test]
fn the_summary_streams_off_screen_and_never_touches_the_turn() {
    let mut app = working_turn();
    app.push_chunk("the agent's own reply");
    let tally = app.status().expect("a turn").tokens;
    app.begin_background_compaction(7);
    summarize(&mut app, "  the gist\n");
    assert_eq!(app.streaming_text(), Some("the agent's own reply"));
    assert_eq!(app.status().expect("a turn").tokens, tally);
    assert_eq!(app.background_summary(), Some("the gist"));
    assert_eq!(running(&app).phase, CompactionPhase::Ready);
}

#[test]
fn a_retried_summary_attempt_is_withdrawn_before_the_next_one() {
    let mut app = working_turn();
    app.begin_background_compaction(7);
    app.on_summary_event(&StreamEvent::Chunk("the first round. ".to_string()));
    app.on_summary_event(&StreamEvent::HookNote {
        label: "Reply cut off".to_string(),
        text: String::new(),
        report: None,
    });
    app.on_summary_event(&StreamEvent::Chunk("half of the".to_string()));
    app.on_summary_event(&StreamEvent::Retrying { attempt: 1, max: 3 });
    app.on_summary_event(&StreamEvent::Chunk("the second round.".to_string()));
    app.on_summary_event(&StreamEvent::StreamDone);
    assert_eq!(
        app.background_summary(),
        Some("the first round. the second round.")
    );
}

#[test]
fn the_summarizers_usage_is_billed_but_moves_neither_the_tally_nor_the_gauge() {
    let mut app = working_turn();
    app.apply_usage(&usage_of(40_000, 100));
    let used = app.context_used();
    let tally = app.status().expect("a turn").tokens;
    let billed = app.usage_since_launch();
    app.begin_background_compaction(7);
    app.on_summary_event(&StreamEvent::Usage(usage_of(30_000, 500)));
    assert_eq!(app.context_used(), used, "the gauge is the agent's");
    assert_eq!(
        app.status().expect("a turn").tokens,
        tally,
        "so is the tally"
    );
    let after = app.usage_since_launch();
    assert_eq!(
        after.requests,
        billed.requests + 1,
        "but the request is billed"
    );
    assert_eq!(after.input, billed.input + 30_000);
}

#[test]
fn a_failed_summary_reports_its_error_and_leaves_the_conversation_as_it_was() {
    let mut app = working_turn();
    app.begin_background_compaction(7);
    let derived = crate::context::context_messages(&app.history);
    assert_eq!(
        app.on_summary_event(&StreamEvent::Error("boom".to_string())),
        SummaryStep::Failed("boom".to_string())
    );
    assert_eq!(app.fail_background_compaction(), Some(7));
    assert!(app.background_compaction().is_none());
    assert!(
        !app.history
            .iter()
            .any(|item| matches!(item, HistoryItem::Compaction(_)))
    );
    assert_eq!(
        crate::context::context_messages(&app.history),
        derived,
        "the cut left behind is inert"
    );
}

#[test]
fn a_summary_sent_again_starts_over() {
    // The summarization request did not fit and is sent again smaller: what
    // the refused one wrote is gone, the compaction itself goes on.
    let mut app = working_turn();
    app.begin_background_compaction(7);
    app.on_summary_event(&StreamEvent::Chunk("half".to_string()));
    app.on_summary_event(&StreamEvent::Error("prompt is too long".to_string()));
    app.restart_background_summary();
    assert_eq!(running(&app).phase, CompactionPhase::Summarizing);
    summarize(&mut app, "whole");
    assert_eq!(app.background_summary(), Some("whole"));
}

#[test]
fn the_handoff_is_the_derivations_own_head() {
    let mut app = working_turn();
    app.set_user_instructions(Some("be careful".to_string()));
    app.begin_background_compaction(7);
    summarize(&mut app, "the gist");
    let (id, head, head_tokens) = app.take_compaction_handoff().expect("a handoff");
    assert_eq!(id, 7);
    let snapshot = app
        .background_compaction_snapshot()
        .expect("still running")
        .to_vec();
    assert_eq!(
        head,
        crate::context::compacted_head(
            Some("be careful"),
            None,
            &snapshot,
            "the gist",
            crate::context::compact_user_budget(Some(50_000)),
        )
    );
    assert!(head_tokens > 0);
}

#[test]
fn the_marker_lands_where_the_agent_spliced_and_names_its_cut() {
    let mut app = working_turn();
    app.begin_background_compaction(7);
    summarize(&mut app, "we built it");
    // The agent kept working while the summary was written.
    app.start_tool("Bash", "cargo test", None);
    app.end_tool("ok", true);
    assert!(app.apply_background_compaction(8, 3).is_none(), "not ours");
    let landed = app.apply_background_compaction(7, 12).expect("it lands");
    let marker = &landed.marker;
    assert_eq!(marker.cut, Some(7));
    assert_eq!(marker.summary, "we built it");
    assert!(marker.auto);
    assert_eq!(marker.secs, 12);
    assert_eq!(marker.user_budget, 5_000, "a tenth of the 50k window");
    assert!(matches!(app.history.last(), Some(HistoryItem::Compaction(c)) if c.cut == Some(7)));
    assert!(app.background_compaction().is_none());
    let ctx = crate::context::context_messages(&app.history);
    let calls: Vec<&str> = ctx
        .iter()
        .flat_map(|m| m.tool_calls.iter().map(|c| c.arguments.as_str()))
        .collect();
    assert_eq!(calls, vec![r#"{"command":"cargo test"}"#], "{ctx:?}");
    assert!(ctx[0].text.ends_with("we built it"), "{ctx:?}");
}

#[test]
fn the_gauge_drops_when_the_summary_lands_and_the_next_frame_snaps_it() {
    let mut app = working_turn();
    // A round whose output filled most of the window.
    app.start_tool("Bash", "cat big.log", None);
    app.end_tool(&"log line words ".repeat(12_000), true);
    app.apply_usage(&usage_of(46_000, 100));
    app.begin_background_compaction(7);
    summarize(&mut app, "we built it");
    let landed = app.apply_background_compaction(7, 1).expect("it lands");
    assert_eq!(landed.marker.before, 46_000);
    assert!(landed.marker.after < 46_000, "{landed:?}");
    assert_eq!(app.context_used(), landed.marker.after);
    app.apply_usage(&usage_of(9_000, 50));
    assert_eq!(app.context_used(), 9_000, "the first frame after it snaps");
}

#[test]
fn esc_drops_the_background_compaction_and_never_lands_a_marker() {
    let mut app = working_turn();
    app.begin_background_compaction(7);
    summarize(&mut app, "we built it");
    app.interrupt_turn(3).expect("an interrupt");
    assert!(app.background_compaction().is_none());
    assert!(app.apply_background_compaction(7, 1).is_none());
    assert!(
        !app.history
            .iter()
            .any(|item| matches!(item, HistoryItem::Compaction(_)))
    );
}

#[test]
fn clear_drops_the_background_compaction_and_the_request() {
    let mut app = working_turn();
    app.request_compaction();
    app.begin_background_compaction(7);
    app.request_compaction();
    app.clear_conversation();
    assert!(app.background_compaction().is_none());
    assert!(!app.compaction_requested());
}

#[test]
fn a_turn_that_ends_mid_summary_finishes_as_an_idle_compaction() {
    // The agent finished before the summary did: the summary keeps going as
    // the session's turn — queued input waits for it — and lands at the end
    // of the history, still covering only its snapshot.
    let mut app = working_turn();
    app.begin_background_compaction(7);
    app.on_summary_event(&StreamEvent::Chunk("we bui".to_string()));
    app.start_tool("Bash", "cargo test", None);
    app.end_tool("ok", true);
    app.push_chunk("All done.");
    app.finish_stream();
    let summary = app.take_turn_summary(9).expect("a turn summary");
    app.record_turn_summary(summary);
    assert!(app.adopt_background_compaction().is_some());
    assert!(app.is_compacting() && app.turn_active());
    assert!(app.background_compaction().is_none(), "it is the turn now");
    app.push_chunk("lt it");
    let marker = app.finish_compact(14).expect("it lands");
    assert_eq!(marker.summary, "we built it");
    assert_eq!(marker.cut, Some(7));
    assert!(marker.auto);
    assert!(matches!(
        app.history.last(),
        Some(HistoryItem::Compaction(_))
    ));
    let ctx = crate::context::context_messages(&app.history);
    assert!(
        ctx.iter().any(|m| m.text == "All done."),
        "what the agent did after the checkpoint stays verbatim: {ctx:?}"
    );
}

#[test]
fn a_summary_ready_when_the_turn_ends_lands_at_its_end() {
    let mut app = working_turn();
    app.begin_background_compaction(7);
    summarize(&mut app, "we built it");
    app.push_chunk("All done.");
    app.finish_stream();
    let summary = app.take_turn_summary(9).expect("a turn summary");
    app.record_turn_summary(summary);
    assert!(
        app.adopt_background_compaction().is_none(),
        "nothing left to write"
    );
    let landed = app.apply_background_compaction(7, 2).expect("it lands");
    assert_eq!(landed.marker.cut, Some(7));
    assert!(matches!(
        app.history.last(),
        Some(HistoryItem::Compaction(_))
    ));
}

#[test]
fn the_waiting_flag_says_the_agent_paused_at_the_hard_limit() {
    let mut app = working_turn();
    app.begin_background_compaction(7);
    assert!(!running(&app).waiting);
    app.set_compaction_waiting(8);
    assert!(!running(&app).waiting, "not ours");
    app.set_compaction_waiting(7);
    assert!(running(&app).waiting);
}

#[test]
fn the_policy_carries_the_window_the_knob_and_what_compacting_can_reach() {
    let mut app = working_turn();
    let policy = app.compaction_policy().expect("a window");
    assert_eq!(policy.window, 50_000);
    assert!(policy.auto);
    assert_eq!(policy.floor, None);
    app.set_context_window(None);
    assert_eq!(app.compaction_policy(), None, "no window, no policy");
}

#[test]
fn a_compaction_that_cannot_reach_the_threshold_says_so_and_stops_the_automatic_ones() {
    // A 100-token window: the summary bridge alone is past 90 of it.
    let mut app = App::new();
    app.record_user_message("hello there my friend");
    app.set_context_window(Some(100));
    app.begin_stream();
    app.apply_usage(&usage_of(200, 0));
    app.finish_stream();
    app.end_turn(1);
    assert!(app.should_auto_compact());
    app.begin_compact(true);
    app.push_chunk("a summary");
    app.finish_compact(1).expect("a marker");
    let (head, threshold) = app.compaction_cannot_help().expect("it says so");
    assert!(head >= threshold, "{head} {threshold}");
    assert!(
        app.compaction_policy()
            .and_then(|policy| policy.floor)
            .is_some_and(|floor| floor >= 90),
        "the agent is told compacting cannot help"
    );
    // A new turn changes nothing about that.
    app.record_user_message("again");
    app.begin_stream();
    app.finish_stream();
    app.end_turn(1);
    assert!(!app.should_auto_compact(), "no compaction that cannot help");
    // A new window, a new verdict.
    app.set_context_window(Some(1_000_000));
    assert!(app.compaction_cannot_help().is_none());
}

#[test]
fn auto_compact_waits_for_the_conversation_to_change_after_an_attempt() {
    // Unlimited compactions, never a loop: after one — landed, failed or
    // Esc'd — the trigger re-arms only once something new joins.
    let mut app = App::new();
    app.record_user_message(&"a long message ".repeat(40));
    app.set_context_window(Some(1_000));
    app.begin_stream();
    app.apply_usage(&usage_of(950, 0));
    app.finish_stream();
    app.end_turn(1);
    assert!(app.should_auto_compact());
    app.begin_compact(true);
    app.push_chunk("half");
    app.fail_stream("boom");
    assert!(
        !app.should_auto_compact(),
        "the same conversation is not retried"
    );
    app.record_user_message("and more");
    app.begin_stream();
    app.apply_usage(&usage_of(990, 0));
    app.finish_stream();
    app.end_turn(1);
    assert!(app.should_auto_compact(), "something new: a later retry");
}
