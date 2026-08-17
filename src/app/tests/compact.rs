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
    assert_eq!(app.status().unwrap().verb, WORKING_VERBS[0]);
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
    let outcome = app.interrupt_turn().expect("an interrupt outcome");
    assert!(
        matches!(
            &outcome,
            InterruptedTurn::Kept {
                partial: None,
                tool: None,
                notice: Some(_),
                ..
            }
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
fn auto_compact_never_fires_mid_turn() {
    let mut app = App::new();
    app.record_user_message("hello");
    app.set_context_window(Some(100));
    app.begin_stream();
    app.apply_usage(&usage_of(990, 0));
    assert!(!app.should_auto_compact(), "a turn is in flight");
}

#[test]
fn auto_compact_is_blocked_after_a_compaction_until_the_next_turn() {
    // One attempt per user turn (codex's per-turn semantics): a compaction
    // that leaves the gauge high must not immediately re-trigger — the
    // next completed turn re-arms it.
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
    app.begin_stream();
    app.finish_stream();
    app.end_turn(1);
    assert!(
        app.should_auto_compact(),
        "the next turn re-arms the trigger"
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
    app.interrupt_turn();
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
