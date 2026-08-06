//! The thinking stream: the live reasoning buffer, the `Thought for …` cell
//! it settles into, and the provider's reasoning-token snap.
//! See `docs/thinking-stream.md`.

use super::*;

/// The `Reasoning` at history index `i` (panics if it is not one).
fn thought_at(app: &App, i: usize) -> &Reasoning {
    match &app.history[i] {
        HistoryItem::Reasoning(r) => r,
        other => panic!("expected a reasoning item at history[{i}], got {other:?}"),
    }
}

#[test]
fn thinking_start_opens_a_live_reasoning_buffer() {
    let mut app = App::new();
    app.begin_stream();
    assert_eq!(app.reasoning(), None, "idle until the phase opens");
    app.begin_reasoning();
    assert_eq!(
        app.reasoning(),
        Some(""),
        "an open phase previews, even before its first delta"
    );
}

#[test]
fn reasoning_deltas_accumulate_in_the_buffer_and_the_tally() {
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("Let me look ");
    app.push_thinking("at the code.");
    assert_eq!(app.reasoning(), Some("Let me look at the code."));
    assert!(
        app.status().unwrap().tokens > 0,
        "reasoning still ticks the tally, as it always did"
    );
}

#[test]
fn deltas_without_an_open_phase_only_count() {
    // ALTER_ZERO_SHOW_THINKING=0: the boundary never opens a buffer, so the
    // pure core keeps exactly the pre-feature behaviour — counted, not kept.
    let mut app = App::new();
    app.begin_stream();
    app.push_thinking("invisible");
    assert_eq!(app.reasoning(), None);
    assert!(app.status().unwrap().tokens > 0);
    assert!(app.finish_reasoning(9).is_none(), "nothing to settle");
    assert!(app.history.is_empty(), "and nothing recorded");
}

#[test]
fn finishing_a_phase_records_the_collapsed_cell() {
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("thinking hard");
    let thought = app.finish_reasoning(65).expect("a phase was open");
    assert_eq!(thought.text, "thinking hard");
    assert_eq!(thought.secs, 65);
    assert_eq!(thought.tokens, count_tokens("thinking hard"));
    assert_eq!(app.reasoning(), None, "the live block is gone");
    assert_eq!(thought_at(&app, 0), &thought, "and recorded in history");
}

#[test]
fn an_empty_phase_records_nothing() {
    // Some providers open and close a reasoning phase without a single delta.
    // A `Thought for 0s` cell for that would be noise.
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    assert!(app.finish_reasoning(0).is_none());
    assert!(app.history.is_empty());
    assert_eq!(app.reasoning(), None);
}

#[test]
fn a_compact_turn_never_opens_a_phase() {
    // The summarization turn is invisible by design (docs/compact.md) — its
    // thinking must not commit a cell into the conversation.
    let mut app = App::new();
    app.begin_compact(false);
    app.begin_reasoning();
    app.push_thinking("summarizing");
    assert_eq!(app.reasoning(), None);
    assert!(app.finish_reasoning(3).is_none());
}

#[test]
fn the_thought_lands_before_the_reply_text() {
    // Reasoning precedes the round's answer, so its cell must slot ahead of
    // the assistant message in history (and so in scrollback).
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("plan");
    app.finish_reasoning(2);
    app.push_chunk("the answer");
    app.finish_stream();
    assert!(matches!(app.history[0], HistoryItem::Reasoning(_)));
    assert_eq!(message_at(&app, 1).role, Role::Assistant);
}

#[test]
fn a_thought_mid_reply_slots_after_the_text_before_it() {
    // A model can reason *after* it has begun answering. The boundary
    // finalises the run of text before the phase (the ToolStart dance) so the
    // cell slots after it — in history, which is what a resize repaints from,
    // exactly as it appeared in scrollback (invariant 4).
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("here is the first part");
    app.begin_reasoning();
    app.push_thinking("second thoughts");
    app.flush_streaming_segment(); // what `Session::settle_reasoning` does
    app.finish_reasoning(2);
    app.push_chunk("and the rest");
    app.finish_stream();
    assert_eq!(message_at(&app, 0).text, "here is the first part");
    assert!(matches!(app.history[1], HistoryItem::Reasoning(_)));
    assert_eq!(message_at(&app, 2).text, "and the rest");
}

#[test]
fn usage_snaps_the_rounds_thought_to_the_provider_count() {
    // The cell is built from the tokenizer estimate (ThinkingEnd fires long
    // before the usage frame); the frame's `reasoning_tokens` replaces it.
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("a short thought");
    let estimate = app.finish_reasoning(4).unwrap().tokens;
    assert_ne!(estimate, 169, "the estimate is not the real number");
    app.apply_usage(&crate::stream::TokenUsage {
        input: 40,
        output: 200,
        reasoning: 169,
        ..crate::stream::TokenUsage::default()
    });
    assert_eq!(thought_at(&app, 0).tokens, 169);
}

#[test]
fn usage_without_a_reasoning_count_keeps_the_estimate() {
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("a short thought");
    let estimate = app.finish_reasoning(4).unwrap().tokens;
    app.apply_usage(&usage_of(40, 200));
    assert_eq!(thought_at(&app, 0).tokens, estimate);
}

#[test]
fn a_rounds_two_phases_split_the_reasoning_tokens() {
    // One `reasoning_tokens` covers the whole round; a model that thought
    // twice gets it split by the phases' own weights, summing exactly.
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("one");
    app.finish_reasoning(1);
    app.push_chunk("interlude");
    app.begin_reasoning();
    app.push_thinking("two two two");
    app.finish_reasoning(1);
    app.apply_usage(&crate::stream::TokenUsage {
        input: 10,
        output: 100,
        reasoning: 40,
        ..crate::stream::TokenUsage::default()
    });
    // The interlude text is still in the streaming buffer, so the two
    // thoughts are history's only items.
    let first = thought_at(&app, 0).tokens;
    let second = thought_at(&app, 1).tokens;
    assert_eq!(first + second, 40, "the split is exact");
    assert!(first < second, "weighted by each phase's own estimate");
}

#[test]
fn a_later_rounds_usage_leaves_an_earlier_thought_alone() {
    // An agentic turn reports one usage frame per tool round; round 2's
    // count must not rewrite round 1's already-snapped cell.
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("round one thinking");
    app.finish_reasoning(1);
    app.apply_usage(&crate::stream::TokenUsage {
        input: 10,
        output: 50,
        reasoning: 30,
        ..crate::stream::TokenUsage::default()
    });
    app.apply_usage(&crate::stream::TokenUsage {
        input: 60,
        output: 50,
        reasoning: 7,
        ..crate::stream::TokenUsage::default()
    });
    assert_eq!(thought_at(&app, 0).tokens, 30, "round one keeps its count");
}

#[test]
fn an_interrupt_after_thinking_keeps_the_thought() {
    // The boundary settles the open phase first, so the recorded cell is what
    // makes this a Kept interrupt: real work streamed, and codex never
    // retracts what streamed (docs/interrupt.md).
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("weighing the options");
    app.finish_reasoning(30);
    assert!(matches!(
        app.interrupt_turn(),
        Some(InterruptedTurn::Kept { .. })
    ));
    assert!(matches!(app.history[1], HistoryItem::Reasoning(_)));
    assert_eq!(
        roles(&app),
        [Role::User, Role::Error],
        "the submission stays, with the interrupt notice"
    );
}

#[test]
fn an_interrupt_before_any_thinking_still_undoes_the_submission() {
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_stream();
    app.begin_reasoning();
    assert!(app.finish_reasoning(0).is_none(), "nothing streamed");
    assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
    assert_eq!(app.input.text(), "hello");
}

#[test]
fn clear_wipes_an_open_phase() {
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("mid-thought");
    type_str(&mut app, "/clear");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Clear);
    assert_eq!(app.reasoning(), None);
    assert!(app.history.is_empty());
}

#[test]
fn a_new_turn_starts_with_no_stale_phase() {
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("abandoned");
    app.begin_stream();
    assert_eq!(app.reasoning(), None);
}
