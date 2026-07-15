//! The agentic tool-calling loop — a **pure, generic driver** (like
//! [`crate::llm::retry`]'s `run_stream`) so the whole loop is unit-tested with
//! fakes and no network. See `docs/tools.md`.
//!
//! [`run_agent`] drives a turn that may call tools: it asks the `round` closure
//! to stream one model response, and if the model requested tool calls it emits
//! the [`StreamEvent::ToolStart`]/[`StreamEvent::ToolEnd`] pair the TUI already
//! knows how to render, runs each call through the `execute` closure, appends
//! the results to the running message list, and loops — until the model answers
//! with plain text (`StreamDone`), fails (`Error`), is cancelled (silent), or
//! the iteration cap trips.

use tokio::sync::mpsc::UnboundedSender;

use super::tools::{ToolCallRequest, ToolOutcome, display_name, summarize_call};
use super::{ChatMessage, LlmError};
use crate::stream::{CancelToken, StreamEvent, ToolCallSummary};

/// The most rounds of tool calls one turn will run before giving up — a
/// backstop against a model that loops forever. Generous enough for real
/// multi-step tasks.
pub const MAX_TOOL_ITERATIONS: usize = 20;

/// What one streaming round produced, as [`run_agent`] sees it. The `round`
/// closure emits the `Chunk`/`Thinking*` events itself; this only reports the
/// disposition and — for a tool-calling round — the assistant message to append
/// plus the calls to run.
pub enum RoundOutcome {
    /// The assistant answered with plain text (no tool calls) — the turn is done.
    Complete,
    /// The assistant requested tool calls. `assistant` is the message to append
    /// verbatim (carrying its `tool_calls`); `calls` are the parsed requests.
    ToolCalls {
        assistant: ChatMessage,
        calls: Vec<ToolCallRequest>,
    },
    /// The request was cancelled (Esc/quit) — stop silently, the UI owns the notice.
    Cancelled,
    /// The request failed.
    Failed(LlmError),
}

/// Drive one agentic turn to completion, sending the terminal `StreamDone` /
/// `Error` (or nothing on a cancel) and the per-tool `ToolStart`/`ToolEnd`
/// events. Generic over `round` (one streaming request) and `execute` (running
/// a tool) so it is fully unit-tested with fakes.
///
/// `messages` is the initial request list (system prompt + conversation
/// context); it grows in place with each assistant/tool message as the loop
/// runs, so every round sees the full history.
pub fn run_agent(
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
    max_iterations: usize,
    mut messages: Vec<ChatMessage>,
    mut round: impl FnMut(&[ChatMessage]) -> RoundOutcome,
    mut execute: impl FnMut(&ToolCallRequest) -> ToolOutcome,
) {
    let mut iterations = 0usize;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        match round(&messages) {
            RoundOutcome::Complete => {
                let _ = tx.send(StreamEvent::StreamDone);
                return;
            }
            RoundOutcome::Cancelled => return,
            RoundOutcome::Failed(err) => {
                let _ = tx.send(StreamEvent::Error(err.to_string()));
                return;
            }
            RoundOutcome::ToolCalls { assistant, calls } => {
                // The cap bounds TOOL ROUNDS, not the final answer: after the
                // max-th round the model still gets one more request, and a
                // plain-text reply there completes the turn — only a further
                // tool request trips the error (docs/tools.md). Checking here,
                // before the round's tools run, also keeps the max+1-th
                // round's side-effecting calls from executing just to have
                // their results discarded.
                if iterations >= max_iterations {
                    let _ = tx.send(StreamEvent::Error(format!(
                        "stopped after {max_iterations} tool iterations without a final answer"
                    )));
                    return;
                }
                messages.push(assistant);
                // Announce the whole batch up front — before any tool runs — so
                // the UI shows every requested call at once, the ones not yet
                // executing as `⎿ Waiting…`. Each entry's (name, args) equals the
                // matching ToolStart's; execution below is still sequential. See
                // `docs/parallel-tools.md`.
                let _ = tx.send(StreamEvent::ToolBatch(
                    calls
                        .iter()
                        .map(|call| ToolCallSummary {
                            name: display_name(&call.name),
                            args: summarize_call(&call.name, &call.arguments),
                        })
                        .collect(),
                ));
                for call in &calls {
                    if cancel.is_cancelled() {
                        return;
                    }
                    let _ = tx.send(StreamEvent::ToolStart {
                        name: display_name(&call.name),
                        args: summarize_call(&call.name, &call.arguments),
                    });
                    let outcome = execute(call);
                    let _ = tx.send(StreamEvent::ToolEnd {
                        output: outcome.output.clone(),
                        ok: outcome.ok,
                        truncated: outcome.truncated,
                    });
                    messages.push(ChatMessage::tool_result(&call.id, &outcome.output));
                }
                // A cancel that landed during a tool run reaps us here rather
                // than spending another round that would just return Cancelled.
                if cancel.is_cancelled() {
                    return;
                }
                iterations += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolCallSpec;
    use std::cell::RefCell;
    use tokio::sync::mpsc::unbounded_channel;

    fn call(id: &str, name: &str, args: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: id.to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        }
    }

    /// An assistant message echoing the given tool calls (as the real round builds).
    fn assistant_with(calls: &[ToolCallRequest]) -> ChatMessage {
        ChatMessage::assistant_tool_calls(
            "",
            calls
                .iter()
                .map(|c| ToolCallSpec::function(&c.id, &c.name, &c.arguments))
                .collect(),
        )
    }

    fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<StreamEvent>) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    #[test]
    fn a_plain_answer_finishes_with_stream_done_and_no_tools() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            vec![ChatMessage::user("hi")],
            |_msgs| RoundOutcome::Complete,
            |_call| panic!("no tools should run"),
        );
        assert_eq!(drain(&mut rx), vec![StreamEvent::StreamDone]);
    }

    #[test]
    fn a_tool_round_emits_start_end_then_a_final_done() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "bash", r#"{"command":"ls"}"#)];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            vec![ChatMessage::user("run ls")],
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete
                }
            },
            |c| ToolOutcome::ok(format!("ran {}", c.name)),
        );
        let events = drain(&mut rx);
        assert_eq!(
            events,
            vec![
                // The batch is announced up front (here a batch of one) so the
                // UI can show every requested call, the not-yet-run ones as
                // `⎿ Waiting…`, before they execute in order. See
                // `docs/parallel-tools.md`.
                StreamEvent::ToolBatch(vec![ToolCallSummary {
                    name: "Bash".to_string(),
                    args: "ls".to_string(),
                }]),
                StreamEvent::ToolStart {
                    name: "Bash".to_string(),
                    args: "ls".to_string(),
                },
                StreamEvent::ToolEnd {
                    output: "ran bash".to_string(),
                    ok: true,
                    truncated: false,
                },
                StreamEvent::StreamDone,
            ]
        );
        assert_eq!(
            *rounds.borrow(),
            2,
            "a second round produced the final answer"
        );
    }

    #[test]
    fn a_multi_call_round_announces_the_whole_batch_before_running_any() {
        // A parallel batch: the model requests three calls at once. The loop
        // announces the whole batch (all three, in order, as the display
        // `(name, args)`) *before* the first ToolStart, so the UI can show every
        // call — the not-yet-run ones as `⎿ Waiting…` — then runs them in order.
        // See `docs/parallel-tools.md`.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![
            call("c1", "bash", r#"{"command":"ping google.com"}"#),
            call("c2", "bash", r#"{"command":"ping facebook.com"}"#),
            call("c3", "bash", r#"{"command":"ping x.com"}"#),
        ];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            vec![ChatMessage::user("ping them")],
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete
                }
            },
            |c| ToolOutcome::ok(format!("ran {}", c.arguments)),
        );
        let events = drain(&mut rx);
        // The very first event announces the batch, carrying all three calls in
        // request order as their header `(name, args)`.
        let summary = |cmd: &str| ToolCallSummary {
            name: "Bash".to_string(),
            args: cmd.to_string(),
        };
        assert_eq!(
            events.first(),
            Some(&StreamEvent::ToolBatch(vec![
                summary("ping google.com"),
                summary("ping facebook.com"),
                summary("ping x.com"),
            ])),
            "the batch is announced first, with every call: {events:?}"
        );
        // Exactly one batch announce, then three Start/End pairs.
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::ToolBatch(_)))
                .count(),
            1,
            "one batch announce for the round"
        );
        let starts = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolStart { .. }))
            .count();
        let ends = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolEnd { .. }))
            .count();
        assert_eq!((starts, ends), (3, 3), "all three calls run: {events:?}");
        // The announce precedes every ToolStart (nothing runs before the batch
        // is shown).
        let batch_pos = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolBatch(_)))
            .unwrap();
        let first_start = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
            .unwrap();
        assert!(
            batch_pos < first_start,
            "the batch is announced before any call starts"
        );
    }

    #[test]
    fn tool_results_are_appended_so_the_next_round_sees_them() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let seen_lens = RefCell::new(Vec::new());
        let calls = vec![call("c1", "read", r#"{"path":"a"}"#)];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            vec![ChatMessage::user("read a")],
            |msgs| {
                seen_lens.borrow_mut().push(msgs.len());
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete
                }
            },
            |_c| ToolOutcome::ok("file contents"),
        );
        drain(&mut rx);
        // Round 1 saw [user]; round 2 saw [user, assistant(tool_calls), tool].
        assert_eq!(*seen_lens.borrow(), vec![1, 3]);
    }

    #[test]
    fn a_failed_round_surfaces_an_error_and_stops() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            vec![ChatMessage::user("x")],
            |_msgs| RoundOutcome::Failed(LlmError::Http("boom".to_string())),
            |_c| panic!("no tools"),
        );
        let events = drain(&mut rx);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], StreamEvent::Error(m) if m.contains("boom")));
    }

    #[test]
    fn a_cancelled_round_streams_nothing() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            vec![ChatMessage::user("x")],
            |_msgs| RoundOutcome::Cancelled,
            |_c| panic!("no tools"),
        );
        assert!(drain(&mut rx).is_empty(), "a cancel is a silent stop");
    }

    #[test]
    fn a_cancel_before_the_first_round_streams_nothing() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        cancel.cancel();
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            vec![ChatMessage::user("x")],
            |_msgs| panic!("round should not run once cancelled"),
            |_c| panic!("no tools"),
        );
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn a_cancel_between_tools_stops_before_running_the_rest() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let calls = vec![
            call("c1", "bash", r#"{"command":"a"}"#),
            call("c2", "bash", r#"{"command":"b"}"#),
        ];
        let ran = RefCell::new(0);
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            vec![ChatMessage::user("x")],
            |_msgs| RoundOutcome::ToolCalls {
                assistant: assistant_with(&calls),
                calls: calls.clone(),
            },
            |c| {
                *ran.borrow_mut() += 1;
                // Cancel after the first tool runs.
                cancel.cancel();
                ToolOutcome::ok(format!("ran {}", c.arguments))
            },
        );
        assert_eq!(
            *ran.borrow(),
            1,
            "the second tool never ran after the cancel"
        );
        // The first tool's start/end were emitted, then a silent stop.
        let events = drain(&mut rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolStart { .. }))
        );
        assert!(!events.iter().any(|e| matches!(e, StreamEvent::StreamDone)));
    }

    #[test]
    fn the_iteration_cap_stops_a_runaway_loop() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let calls = vec![call("c", "bash", r#"{"command":"loop"}"#)];
        // Every round asks for another tool — the cap must break it.
        run_agent(
            &tx,
            &cancel,
            3,
            vec![ChatMessage::user("x")],
            |_msgs| RoundOutcome::ToolCalls {
                assistant: assistant_with(&calls),
                calls: calls.clone(),
            },
            |_c| ToolOutcome::ok("again"),
        );
        let events = drain(&mut rx);
        let errors: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::Error(_)))
            .collect();
        assert_eq!(errors.len(), 1);
        assert!(matches!(&errors[0], StreamEvent::Error(m) if m.contains("3 tool iterations")));
        // Exactly 3 tool rounds ran.
        let ends = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolEnd { .. }))
            .count();
        assert_eq!(ends, 3);
    }

    #[test]
    fn a_final_answer_after_exactly_max_tool_rounds_completes() {
        // The cap bounds TOOL ROUNDS, not the final answer (docs/tools.md):
        // after the max-th round the model still gets one more request, and a
        // plain-text reply there finishes the turn — only a further tool
        // request trips the error.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c", "bash", r#"{"command":"step"}"#)];
        run_agent(
            &tx,
            &cancel,
            2,
            vec![ChatMessage::user("x")],
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n <= 2 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete
                }
            },
            |_c| ToolOutcome::ok("done"),
        );
        let events = drain(&mut rx);
        assert!(
            events.iter().any(|e| matches!(e, StreamEvent::StreamDone)),
            "a Complete round after max tool rounds still finishes: {events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, StreamEvent::Error(_))),
            "no cap error for a turn that answered: {events:?}"
        );
    }
}
