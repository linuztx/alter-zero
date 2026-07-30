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

use super::tools::{
    ToolCallRequest, ToolOutcome, display_name, image_attachment_note, summarize_call,
};
use super::{ChatMessage, ContentPart, LlmError};
use crate::permission::Approval;
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
/// events. Generic over `round` (one streaming request), `execute` (running
/// a tool), and `pending_notices` (the background-completion notes to
/// inject) so it is fully unit-tested with fakes.
///
/// `messages` is the initial request list (system prompt + conversation
/// context); it grows in place with each assistant/tool message as the loop
/// runs, so every round sees the full history.
///
/// `pending_notices` is taken at the top of **every** round: background
/// shells that finished since the last request — a completion, or a kill the
/// model itself just ran (`kill`/`pkill` in a bash call, or the user's `x`
/// in the ↓ manager) — reach the model *within the same turn*, each appended
/// as a user-role message after the prior round's tool results (the same
/// form `context::context_messages` replays into later turns' contexts).
/// The take sits after the cancel check so an abandoned turn can't steal
/// notes owed to the boundary's automatic follow-up turn. See
/// `docs/background.md`.
///
/// `approve` is the permission gate (`docs/permissions.md`), consulted for
/// every ordinary call **before** its `ToolStart` — so nothing has run, and
/// nothing shows as running, while the user decides. An
/// [`Approval::Reject`] resolves the call without executing it: the
/// Start/End pair still goes out (with the short `display` output, red) so
/// the cell lands in history and the transcript, while the longer `result`
/// becomes the tool result the model reads.
#[allow(clippy::too_many_arguments)] // the loop's full seam set (docs/agent-tool.md)
pub fn run_agent(
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
    max_iterations: usize,
    messages: &mut Vec<ChatMessage>,
    mut round: impl FnMut(&[ChatMessage]) -> RoundOutcome,
    mut execute: impl FnMut(&ToolCallRequest, &mut dyn FnMut(&str)) -> ToolOutcome,
    mut pending_notices: impl FnMut() -> Vec<String>,
    mut run_agents: impl FnMut(&[ToolCallRequest]) -> Vec<(String, String)>,
    mut approve: impl FnMut(&ToolCallRequest) -> Approval,
) {
    let mut iterations = 0usize;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        for note in pending_notices() {
            messages.push(ChatMessage::user(note));
        }
        match round(messages) {
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
                // The round's `agent` calls take their own path
                // (`docs/agent-tool.md`): the `run_agents` closure launches
                // them all **concurrently**, emits the AgentBatch /
                // AgentGroupDone events, waits for foreground completion
                // (polling this same cancel), and returns each call's tool
                // result. The ordinary calls run sequentially exactly as
                // before; every result then appends in the model's original
                // call order (strict providers pair results by contiguity).
                let (agent_calls, rest): (Vec<&ToolCallRequest>, Vec<&ToolCallRequest>) = calls
                    .iter()
                    .partition(|call| call.name == super::tools::AGENT_TOOL_NAME);
                let mut results: Vec<(String, String)> = Vec::with_capacity(calls.len());
                if !agent_calls.is_empty() {
                    let owned: Vec<ToolCallRequest> = agent_calls.into_iter().cloned().collect();
                    results.extend(run_agents(&owned));
                }
                // Announce the ordinary batch up front — before any tool runs
                // — so the UI shows every requested call at once, the ones not
                // yet executing as `⎿ Waiting…`. Each entry's (name, args)
                // equals the matching ToolStart's; execution below is still
                // sequential. See `docs/parallel-tools.md`.
                if !rest.is_empty() {
                    let _ = tx.send(StreamEvent::ToolBatch(
                        rest.iter()
                            .map(|call| ToolCallSummary {
                                name: display_name(&call.name),
                                args: summarize_call(&call.name, &call.arguments),
                            })
                            .collect(),
                    ));
                }
                // An image `read`'s pixels: collected per call and attached
                // AFTER the round's tool results, which must stay contiguous
                // (strict providers require every tool_call answered directly
                // after the assistant message). Each attachment is a
                // user-role parts message — the one multimodal shape every
                // OpenAI-compatible vision endpoint accepts; `tool`-role
                // messages reject image parts. The context replays the same
                // shape into later turns (`crate::context`). See
                // `docs/tools.md`.
                let mut attachments: Vec<ChatMessage> = Vec::new();
                let mut cancelled_mid_tools = false;
                for call in &rest {
                    if cancel.is_cancelled() {
                        cancelled_mid_tools = true;
                        break;
                    }
                    // The permission gate (docs/permissions.md), asked BEFORE
                    // the ToolStart so nothing has run — and nothing has been
                    // announced as running — while the user decides. A
                    // rejection still emits the Start/End pair, so the call
                    // lands in history and the transcript as a red cell, while
                    // the *model* reads the longer instruction.
                    if let Approval::Reject { display, result } = approve(call) {
                        let _ = tx.send(StreamEvent::ToolStart {
                            name: display_name(&call.name),
                            args: summarize_call(&call.name, &call.arguments),
                            detail: super::tools::call_description(&call.name, &call.arguments),
                        });
                        let _ = tx.send(StreamEvent::ToolEnd {
                            output: display,
                            ok: false,
                            truncated: false,
                        });
                        results.push((call.id.clone(), result));
                        continue;
                    }
                    let _ = tx.send(StreamEvent::ToolStart {
                        name: display_name(&call.name),
                        args: summarize_call(&call.name, &call.arguments),
                        detail: super::tools::call_description(&call.name, &call.arguments),
                    });
                    // Forward the tool's live output to the UI as it is produced,
                    // so the running cell tails it (docs/tool-streaming.md). The
                    // sink targets the front running call app-side; a tool that
                    // does not stream (read/write/edit) simply never calls it.
                    let mut on_output = |chunk: &str| {
                        let _ = tx.send(StreamEvent::ToolOutput(chunk.to_string()));
                    };
                    let outcome = execute(call, &mut on_output);
                    // A backgrounded call resolves via its own event — the
                    // cell shows the fixed backgrounded row while the launch
                    // text still becomes the tool result the model reads
                    // (docs/background.md).
                    match &outcome.background {
                        Some(id) => {
                            let _ = tx.send(StreamEvent::ToolBackgrounded {
                                id: id.clone(),
                                output: outcome.output.clone(),
                            });
                        }
                        None => {
                            let _ = tx.send(StreamEvent::ToolEnd {
                                output: outcome.output.clone(),
                                ok: outcome.ok,
                                truncated: outcome.truncated,
                            });
                        }
                    }
                    results.push((call.id.clone(), outcome.output.clone()));
                    if let Some(url) = outcome.image {
                        let path = summarize_call(&call.name, &call.arguments);
                        attachments.push(ChatMessage::with_parts(
                            "user",
                            vec![
                                ContentPart::text(image_attachment_note(&path)),
                                ContentPart::image(url),
                            ],
                        ));
                    }
                }
                // Results append in the model's original call order (an
                // unexecuted call — a cancel landed first — is answered so
                // the stored list stays well-formed for a continuation).
                for call in &calls {
                    let output = results
                        .iter()
                        .find(|(id, _)| id == &call.id)
                        .map_or_else(|| "[not executed]".to_string(), |(_, out)| out.clone());
                    messages.push(ChatMessage::tool_result(&call.id, &output));
                }
                messages.append(&mut attachments);
                // A cancel that landed during a tool run reaps us here rather
                // than spending another round that would just return Cancelled.
                if cancelled_mid_tools || cancel.is_cancelled() {
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
            &mut vec![ChatMessage::user("hi")],
            |_msgs| RoundOutcome::Complete,
            |_call, _sink| panic!("no tools should run"),
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
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
            &mut vec![ChatMessage::user("run ls")],
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
            |c, _sink| ToolOutcome::ok(format!("ran {}", c.name)),
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
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
                    detail: None,
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
    fn a_tool_that_streams_output_emits_tooloutput_between_start_and_end() {
        // The executor's `on_output` sink surfaces as ToolOutput events strictly
        // between the call's ToolStart and ToolEnd, so the running cell tails the
        // output as it is produced. See `docs/tool-streaming.md`.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "bash", r#"{"command":"printf 'a\nb\n'"}"#)];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("run it")],
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
            |_c, sink| {
                sink("a\n");
                sink("b\n");
                ToolOutcome::ok("Exit code: 0\na\nb")
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
        );
        let events = drain(&mut rx);
        let start = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
            .expect("the tool started");
        let end = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolEnd { .. }))
            .expect("the tool ended");
        let outputs: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ToolOutput(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            outputs,
            vec!["a\n", "b\n"],
            "the sink's chunks stream as ToolOutput: {events:?}"
        );
        let all_between = events
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, StreamEvent::ToolOutput(_)))
            .all(|(i, _)| start < i && i < end);
        assert!(all_between, "live output streams between start and end");
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
            &mut vec![ChatMessage::user("ping them")],
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
            |c, _sink| ToolOutcome::ok(format!("ran {}", c.arguments)),
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
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
            &mut vec![ChatMessage::user("read a")],
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
            |_c, _sink| ToolOutcome::ok("file contents"),
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
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
            &mut vec![ChatMessage::user("x")],
            |_msgs| RoundOutcome::Failed(LlmError::Http("boom".to_string())),
            |_c, _sink| panic!("no tools"),
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
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
            &mut vec![ChatMessage::user("x")],
            |_msgs| RoundOutcome::Cancelled,
            |_c, _sink| panic!("no tools"),
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
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
            &mut vec![ChatMessage::user("x")],
            |_msgs| panic!("round should not run once cancelled"),
            |_c, _sink| panic!("no tools"),
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
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
            &mut vec![ChatMessage::user("x")],
            |_msgs| RoundOutcome::ToolCalls {
                assistant: assistant_with(&calls),
                calls: calls.clone(),
            },
            |c, _sink| {
                *ran.borrow_mut() += 1;
                // Cancel after the first tool runs.
                cancel.cancel();
                ToolOutcome::ok(format!("ran {}", c.arguments))
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
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
            &mut vec![ChatMessage::user("x")],
            |_msgs| RoundOutcome::ToolCalls {
                assistant: assistant_with(&calls),
                calls: calls.clone(),
            },
            |_c, _sink| ToolOutcome::ok("again"),
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
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
            &mut vec![ChatMessage::user("x")],
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
            |_c, _sink| ToolOutcome::ok("done"),
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
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

    /// The plain text of a message, for order assertions.
    fn text_of(msg: &ChatMessage) -> String {
        match &msg.content {
            crate::llm::MessageContent::Text(t) => t.clone(),
            crate::llm::MessageContent::Parts(_) => panic!("no multimodal messages here"),
        }
    }

    #[test]
    fn notices_posted_between_rounds_inject_as_user_messages() {
        // A background shell that exits mid-turn — killed by the model's own
        // bash `kill`, the manager's `x`, or a natural death — must reach the
        // model WITHIN the turn: the loop takes the pending notes at the top
        // of each round and appends each as a user-role message, after the
        // prior round's tool results (the same form `context_messages`
        // replays into later turns), so the very next request already carries
        // the outcome. See docs/background.md.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "bash", r#"{"command":"kill 408085; sleep 1"}"#)];
        let seen_round2: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let note = "[background] Background command \"Start the API server\" \
                    (id bvyo7tkbe) was terminated by a signal.";
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("kill the server")],
            |msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    *seen_round2.borrow_mut() =
                        msgs.iter().map(|m| (m.role.clone(), text_of(m))).collect();
                    RoundOutcome::Complete
                }
            },
            |_c, _sink| ToolOutcome::ok("Exit code: 0"),
            || {
                // The exit landed while the kill command ran: the board has
                // the note by the time round 2's request is built.
                if *rounds.borrow() == 1 {
                    vec![note.to_string()]
                } else {
                    Vec::new()
                }
            },
            |_calls| Vec::new(),
            |_call| Approval::Allow,
        );
        drain(&mut rx);
        let seen = seen_round2.borrow();
        assert_eq!(
            seen.last(),
            Some(&("user".to_string(), note.to_string())),
            "the note is the round's last message — a user-role entry after \
             the tool results: {seen:?}"
        );
        assert_eq!(
            seen.iter()
                .map(|(role, _)| role.as_str())
                .collect::<Vec<_>>(),
            vec!["user", "assistant", "tool", "user"],
            "user → assistant(tool_calls) → tool result → the injected note"
        );
    }

    #[test]
    fn a_notice_pending_at_turn_start_rides_the_first_round() {
        // A completion that landed between the turn's dispatch and its first
        // request is picked up at the very first loop top — the model needs
        // no tool round to hear about it.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let pending = RefCell::new(Some("[background] note".to_string()));
        let seen = RefCell::new(Vec::new());
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("hi")],
            |msgs| {
                *seen.borrow_mut() = msgs.iter().map(|m| (m.role.clone(), text_of(m))).collect();
                RoundOutcome::Complete
            },
            |_c, _sink| panic!("no tools requested"),
            || pending.borrow_mut().take().into_iter().collect(),
            |_calls| Vec::new(),
            |_call| Approval::Allow,
        );
        drain(&mut rx);
        assert_eq!(
            *seen.borrow(),
            vec![
                ("user".to_string(), "hi".to_string()),
                ("user".to_string(), "[background] note".to_string()),
            ]
        );
    }

    #[test]
    fn a_cancelled_turn_takes_no_notices() {
        // The take sits AFTER the cancel check: an abandoned (Esc'd) turn's
        // detached thread must not steal notes owed to the boundary's
        // automatic follow-up turn.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        cancel.cancel();
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("x")],
            |_msgs| panic!("no round once cancelled"),
            |_c, _sink| panic!("no tools"),
            || panic!("no notice take once cancelled"),
            |_calls| Vec::new(),
            |_call| Approval::Allow,
        );
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn an_image_read_outcome_attaches_a_user_image_message_after_the_results() {
        // An image `read` (docs/tools.md): the tool result stays the small
        // text while the pixels ride a follow-up USER message — the only
        // multimodal shape every OpenAI-compatible provider accepts
        // (tool-role messages reject image parts). With a sibling call in the
        // round, the results stay contiguous (strict providers require every
        // tool_call answered directly after the assistant message) and the
        // attachment follows them.
        use crate::llm::{ContentPart, MessageContent};
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![
            call("c1", "read", r#"{"path":"shot.png"}"#),
            call("c2", "bash", r#"{"command":"ls"}"#),
        ];
        let seen_round2: RefCell<Vec<ChatMessage>> = RefCell::new(Vec::new());
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("look at shot.png")],
            |msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    *seen_round2.borrow_mut() = msgs.to_vec();
                    RoundOutcome::Complete
                }
            },
            |c, _sink| {
                if c.name == "read" {
                    ToolOutcome::ok(
                        "Read image shot.png (PNG, 3x2, 90 B)\n\
                         The image is attached as the next user message.",
                    )
                    .with_image("data:image/png;base64,AAAA")
                } else {
                    ToolOutcome::ok("Exit code: 0\nfiles")
                }
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
        );
        drain(&mut rx);
        let seen = seen_round2.borrow();
        let roles: Vec<&str> = seen.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(
            roles,
            vec!["user", "assistant", "tool", "tool", "user"],
            "the results stay contiguous; the attachment follows them"
        );
        let attachment = seen.last().unwrap();
        let MessageContent::Parts(parts) = &attachment.content else {
            panic!("the attachment is a multimodal parts message: {attachment:?}");
        };
        assert_eq!(parts.len(), 2, "one text note + one image: {parts:?}");
        let ContentPart::Text { text } = &parts[0] else {
            panic!("a text note leads: {parts:?}");
        };
        assert!(text.starts_with("[image] "), "got {text}");
        assert!(text.contains("shot.png"), "the note names the path: {text}");
        let ContentPart::ImageUrl { image_url } = &parts[1] else {
            panic!("the pixels follow the note: {parts:?}");
        };
        assert_eq!(image_url.url, "data:image/png;base64,AAAA");
        // The tool result itself stays the plain text the cell shows.
        assert_eq!(seen[2].role, "tool");
        assert!(
            matches!(&seen[2].content, MessageContent::Text(t) if t.starts_with("Read image ")),
            "the result content is the text facts: {:?}",
            seen[2]
        );
    }

    #[test]
    fn an_imageless_round_attaches_nothing() {
        // The zero-image path is byte-identical to before the feature.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "read", r#"{"path":"a.txt"}"#)];
        let seen_round2: RefCell<Vec<ChatMessage>> = RefCell::new(Vec::new());
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("read a.txt")],
            |msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    *seen_round2.borrow_mut() = msgs.to_vec();
                    RoundOutcome::Complete
                }
            },
            |_c, _sink| ToolOutcome::ok("1 alpha"),
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
        );
        drain(&mut rx);
        let roles: Vec<String> = seen_round2
            .borrow()
            .iter()
            .map(|m| m.role.clone())
            .collect();
        assert_eq!(roles, vec!["user", "assistant", "tool"]);
    }

    #[test]
    fn a_backgrounded_outcome_emits_tool_backgrounded_and_still_feeds_the_result() {
        // `run_in_background` (or a Ctrl+B handoff): the executor returns a
        // background outcome — the loop resolves the cell via ToolBackgrounded
        // (never ToolEnd) while the launch text still becomes the tool-result
        // message the next round reads (docs/background.md).
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let seen_lens = RefCell::new(Vec::new());
        let calls = vec![call(
            "c1",
            "bash",
            r#"{"command":"ping x.com","run_in_background":true}"#,
        )];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("ping in background")],
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
            |_c, _sink| ToolOutcome::backgrounded("bash_1", "Command running with ID: bash_1"),
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Allow,
        );
        let events = drain(&mut rx);
        assert!(
            events.iter().any(|e| matches!(
                e,
                StreamEvent::ToolBackgrounded { id, output }
                    if id == "bash_1" && output.contains("bash_1")
            )),
            "the call resolves as backgrounded: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolEnd { .. })),
            "no ToolEnd for a backgrounded call: {events:?}"
        );
        assert!(events.iter().any(|e| matches!(e, StreamEvent::StreamDone)));
        // Round 2 saw [user, assistant(tool_calls), tool result] — the loop kept going.
        assert_eq!(*seen_lens.borrow(), vec![1, 3]);
    }

    #[test]
    fn a_rejected_call_never_runs_but_still_resolves_red_for_the_user() {
        // The permission gate (docs/permissions.md): the executor is never
        // reached, the cell still commits (Start + a red End carrying the
        // short display text), and the model reads the longer instruction.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "write", r#"{"path":"hello.py","content":"x"}"#)];
        let mut messages = vec![ChatMessage::user("write hello.py")];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut messages,
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
            |_c, _sink| panic!("a rejected call must never execute"),
            Vec::new,
            |_calls| Vec::new(),
            |_call| Approval::Reject {
                display: "User rejected write to hello.py".to_string(),
                result: "The user doesn't want to proceed…".to_string(),
            },
        );
        let events = drain(&mut rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolStart { .. })),
            "the cell is still announced: {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                StreamEvent::ToolEnd { output, ok, .. }
                    if !ok && output == "User rejected write to hello.py"
            )),
            "…and resolves red with the short display text: {events:?}"
        );
        // The model's tool result is the longer instruction, not the cell text.
        let result = messages
            .iter()
            .find(|m| m.role == "tool")
            .expect("the call was answered");
        assert!(
            matches!(&result.content, crate::llm::MessageContent::Text(t)
                if t.starts_with("The user doesn't want to proceed")),
            "got {result:?}"
        );
        assert!(events.iter().any(|e| matches!(e, StreamEvent::StreamDone)));
    }

    #[test]
    fn approval_is_asked_before_the_tool_starts() {
        // Ordering matters: the prompt must appear with nothing running, so
        // the gate is consulted ahead of the ToolStart event.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let log: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());
        let calls = vec![call("c1", "bash", r#"{"command":"ls"}"#)];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("ls")],
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
            |_c, _sink| {
                log.borrow_mut().push("execute");
                ToolOutcome::ok("files")
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call| {
                log.borrow_mut().push("approve");
                Approval::Allow
            },
        );
        drain(&mut rx);
        assert_eq!(*log.borrow(), vec!["approve", "execute"]);
    }

    #[test]
    fn agent_calls_route_to_the_launcher_and_results_keep_call_order() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![
            call("c1", "agent", r#"{"description":"d","prompt":"p"}"#),
            call("c2", "bash", r#"{"command":"ls"}"#),
        ];
        let mut messages = vec![ChatMessage::user("go")];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut messages,
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
            |c, _sink| {
                assert_eq!(c.name, "bash", "agent calls never reach the executor");
                ToolOutcome::ok("listing")
            },
            Vec::new,
            |agent_calls| {
                assert_eq!(agent_calls.len(), 1);
                assert_eq!(agent_calls[0].id, "c1");
                vec![("c1".to_string(), "agent result".to_string())]
            },
            |_call| Approval::Allow,
        );
        let events = drain(&mut rx);
        // The ordinary batch announces only the bash call.
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolBatch(items) if items.len() == 1 && items[0].name == "Bash"
        )));
        // The tool results append in the model's original call order.
        let results: Vec<(String, String)> = messages
            .iter()
            .filter(|m| m.role == "tool")
            .map(|m| {
                (
                    m.tool_call_id.clone().unwrap(),
                    match &m.content {
                        crate::llm::MessageContent::Text(t) => t.clone(),
                        crate::llm::MessageContent::Parts(_) => String::new(),
                    },
                )
            })
            .collect();
        assert_eq!(
            results,
            vec![
                ("c1".to_string(), "agent result".to_string()),
                ("c2".to_string(), "listing".to_string()),
            ]
        );
    }
}
