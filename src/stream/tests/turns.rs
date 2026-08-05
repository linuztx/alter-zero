//! The pure scripted turns: [`turn_events`]'s whole event order.

use super::*;

#[test]
fn a_table_prompt_streams_a_pure_table_turn() {
    // A prompt mentioning "table" plays the markdown-table demo: a
    // text-only turn — no thinking phase, no tool calls (a tool would
    // split the reply and flush the block early) — whose chunks
    // concatenate to the reply, ending in StreamDone. The reply carries a
    // multi-row GFM table with prose AFTER it, so the block closes
    // mid-stream: the strip-collapse geometry the smoke suite guards
    // (docs/table-streaming.md).
    let events = turn_events("show me a table", 0);
    let text: String = chunk_text(&events);
    assert_eq!(text, dummy_response("show me a table"));
    assert!(text.contains("| ID | Name |"), "carries the table: {text}");
    assert!(
        text.trim_end().ends_with("Markdown."),
        "prose follows the table so the block closes mid-stream"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolBatch(_) | StreamEvent::ToolStart { .. } | StreamEvent::ThinkingStart
        )),
        "a table turn is text-only"
    );
    assert!(matches!(events.last(), Some(StreamEvent::StreamDone)));
}

#[test]
fn turn_events_chunks_still_reconstruct_the_reply() {
    // Tool events are interleaved, but the Chunk events alone must still
    // concatenate to exactly the dummy reply.
    let prompt = "tell me something";
    let text: String = chunk_text(&turn_events(prompt, 0));
    assert_eq!(text, dummy_response(prompt));
}

#[test]
fn a_compact_prompt_plays_a_text_only_summary_turn() {
    // /compact's summarization prompt must never trigger the scripted tool
    // batch or thinking phase — codex sends the summarize request with no
    // tools, and the offline path (and smoke.sh) drives this branch
    // (docs/compact.md).
    let events = turn_events(crate::context::SUMMARIZATION_PROMPT, 0);
    assert!(
        events
            .iter()
            .all(|e| matches!(e, StreamEvent::Chunk(_) | StreamEvent::StreamDone)),
        "text-only: {events:?}"
    );
    assert!(matches!(events.last(), Some(StreamEvent::StreamDone)));
    let text: String = chunk_text(&events);
    assert!(
        !text.trim().is_empty(),
        "a non-empty canned summary streams"
    );
}

#[test]
fn turn_events_interleaves_at_least_one_tool_call() {
    let events = turn_events("hi", 0);
    let starts = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ToolStart { .. }))
        .count();
    let ends = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ToolEnd { .. }))
        .count();
    assert!(starts >= 1, "a turn runs at least one tool");
    assert_eq!(starts, ends, "every ToolStart has a matching ToolEnd");
}

#[test]
fn turn_events_announces_a_parallel_batch_before_its_tools() {
    // The dummy scripts a parallel `Bash` batch: a `ToolBatch` (>= 2 calls)
    // is emitted *before* the first `ToolStart`, so the UI shows the
    // not-yet-run calls as `⎿ Waiting…`. Each batch entry's `(name, args)`
    // matches the `ToolStart` that runs it, in order. See
    // `docs/parallel-tools.md`.
    let events = turn_events("hi", 0);
    let batch_pos = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ToolBatch(_)))
        .expect("the dummy announces a parallel batch");
    let StreamEvent::ToolBatch(items) = &events[batch_pos] else {
        unreachable!()
    };
    assert!(items.len() >= 2, "the batch has several parallel calls");
    let first_start = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
        .expect("the batch runs its tools");
    assert!(
        batch_pos < first_start,
        "the batch is announced before any call starts"
    );
    // The batch entries equal the name/args of the ToolStarts that follow.
    let following_starts: Vec<ToolCallSummary> = events[batch_pos + 1..]
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolStart { name, args, .. } => Some(ToolCallSummary {
                name: name.clone(),
                args: args.clone(),
            }),
            _ => None,
        })
        .take(items.len())
        .collect();
    assert_eq!(
        *items, following_starts,
        "each announced call matches its ToolStart"
    );
}

#[test]
fn a_parallel_prompt_triggers_the_vivid_three_call_bash_batch() {
    // A prompt mentioning "parallel" opts into the vivid demo: a single
    // ToolBatch of three `Bash(ping …)` calls announced up front (so the
    // not-yet-run ones show `⎿ Waiting…`), then run in order. The default turn
    // keeps its compact two-call batch. See `docs/parallel-tools.md`.
    let events = turn_events("run three pings in parallel", 0);
    let StreamEvent::ToolBatch(items) = events
        .iter()
        .find(|e| matches!(e, StreamEvent::ToolBatch(_)))
        .expect("a parallel prompt announces a batch")
    else {
        unreachable!()
    };
    assert_eq!(items.len(), 3, "three parallel calls: {items:?}");
    assert!(
        items
            .iter()
            .all(|s| s.name == "Bash" && s.args.contains("ping")),
        "every call is a Bash ping: {items:?}"
    );
    let ends = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ToolEnd { .. }))
        .count();
    assert_eq!(ends, 3, "all three calls run");
}

#[test]
fn the_default_turn_keeps_the_compact_two_call_batch() {
    // Without "parallel", the turn runs the compact Read+Bash batch (baseline
    // footprint), not the three-ping demo — so unrelated smoke phases keep
    // their sizing.
    let events = turn_events("hello there", 0);
    let StreamEvent::ToolBatch(items) = events
        .iter()
        .find(|e| matches!(e, StreamEvent::ToolBatch(_)))
        .expect("the default turn still announces a batch")
    else {
        unreachable!()
    };
    assert_eq!(items.len(), 2, "two calls by default: {items:?}");
    assert_eq!(items[0].name, "Read", "the Read runs first: {items:?}");
}

#[test]
fn turn_events_includes_one_paired_thinking_phase_before_the_tools() {
    let events = turn_events("hi", 0);
    let starts = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ThinkingStart))
        .count();
    let ends = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ThinkingEnd))
        .count();
    assert_eq!(starts, 1, "the turn thinks exactly once");
    assert_eq!(ends, 1, "every ThinkingStart has a matching ThinkingEnd");

    let start = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ThinkingStart))
        .unwrap();
    let end = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ThinkingEnd))
        .unwrap();
    let first_tool = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
        .unwrap();
    assert!(start < end, "thinking starts before it ends");
    assert!(
        end < first_tool,
        "thinking resolves before the first tool, so tool start/end stay adjacent"
    );
}

#[test]
fn turn_events_streams_thinking_chunks_inside_the_thinking_phase() {
    // The reasoning text travels as ThinkingChunk events strictly between
    // the ThinkingStart/ThinkingEnd pair — opaque to the renderer (never
    // displayed) but counted into the live token tally, like a real API's
    // reasoning deltas.
    let events = turn_events("hi", 0);
    let start = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ThinkingStart))
        .unwrap();
    let end = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ThinkingEnd))
        .unwrap();
    let chunk_positions: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, StreamEvent::ThinkingChunk(_)))
        .map(|(i, _)| i)
        .collect();
    assert!(
        !chunk_positions.is_empty(),
        "the dummy streams reasoning text while it thinks"
    );
    assert!(
        chunk_positions.iter().all(|&i| start < i && i < end),
        "every ThinkingChunk sits inside the Start/End pair"
    );
}

#[test]
fn turn_events_resolve_each_tool_before_the_next_starts() {
    // Tools don't nest: after a ToolStart, only its live ToolOutput chunks
    // may appear before the matching ToolEnd — never a second ToolStart — so
    // the loop only ever tracks one running tool at a time. See
    // `docs/tool-streaming.md`.
    let events = turn_events("anything", 0);
    let mut running = false;
    for event in &events {
        match event {
            StreamEvent::ToolStart { .. } => {
                assert!(!running, "a tool starts only after the previous one ended");
                running = true;
            }
            StreamEvent::ToolEnd { .. } => {
                assert!(running, "a ToolEnd closes a running tool");
                running = false;
            }
            StreamEvent::ToolOutput(_) => {
                assert!(running, "live output only streams while a tool runs");
            }
            _ => {}
        }
    }
    assert!(!running, "every tool that started also ended");
}

#[test]
fn turn_events_streams_each_bash_output_before_its_end() {
    // Each Bash call streams its output as ToolOutput chunks between its
    // ToolStart and ToolEnd; concatenated, they equal the ToolEnd output
    // (the authoritative full cell). See `docs/tool-streaming.md`.
    let events = turn_events("run three pings in parallel", 0);
    let mut streamed = String::new();
    let mut resolved = 0;
    for event in &events {
        match event {
            StreamEvent::ToolOutput(chunk) => streamed.push_str(chunk),
            StreamEvent::ToolEnd { output, .. } => {
                // Each Bash cell's streamed output equals its final output.
                assert_eq!(&streamed, output, "the tail reconstructs the final cell");
                streamed.clear();
                resolved += 1;
            }
            _ => {}
        }
    }
    assert_eq!(resolved, 3, "all three Bash cells streamed then resolved");
}

#[test]
fn turn_events_generates_each_tool_call_before_it_starts() {
    // Each ToolStart is preceded by ToolCallDelta fragments (the model
    // "generating" the call) and never sits between a Start and its End, so
    // the tally ticks during generation and tools still resolve one at a time.
    let events = turn_events("anything", 0);
    let deltas = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ToolCallDelta(_)))
        .count();
    assert!(
        deltas >= 1,
        "the dummy streams tool-call generation fragments"
    );
    let mut running = false;
    for event in &events {
        match event {
            StreamEvent::ToolStart { .. } => running = true,
            StreamEvent::ToolEnd { .. } => running = false,
            StreamEvent::ToolCallDelta(_) => assert!(
                !running,
                "a generation fragment never streams while a tool is running"
            ),
            _ => {}
        }
    }
}

#[test]
fn turn_events_shows_both_a_success_and_a_failure() {
    // The demo exercises green and red: at least one ok tool and one failing.
    let events = turn_events("x", 0);
    let oks = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ToolEnd { ok: true, .. }))
        .count();
    let fails = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ToolEnd { ok: false, .. }))
        .count();
    assert!(oks >= 1, "at least one tool succeeds (green)");
    assert!(fails >= 1, "at least one tool fails (red)");
}

#[test]
fn turn_events_ends_with_stream_done() {
    assert_eq!(turn_events("x", 0).last(), Some(&StreamEvent::StreamDone));
}

#[test]
fn turn_events_without_images_streams_just_the_reply() {
    // image_count 0 is the old behaviour: chunks reconstruct the reply.
    let prompt = "hello";
    assert_eq!(chunk_text(&turn_events(prompt, 0)), dummy_response(prompt));
}

#[test]
fn turn_events_acknowledges_attached_images_up_front() {
    // The dummy has no vision, so it acknowledges the attachments instead —
    // a leading chunk proving the typed image channel reached the backend.
    let prompt = "what is this";
    assert_eq!(
        chunk_text(&turn_events(prompt, 2)),
        format!("Looking at your 2 images. {}", dummy_response(prompt))
    );
}

#[test]
fn image_acknowledgement_is_singular_for_one_image() {
    assert!(chunk_text(&turn_events("x", 1)).starts_with("Looking at your 1 image. "));
}

#[test]
fn an_agents_prompt_scripts_the_two_agent_demo() {
    let events = turn_events("call agents for weather", 0);
    let batch = events.iter().find_map(|e| match e {
        StreamEvent::AgentBatch { background, agents } => Some((background, agents)),
        _ => None,
    });
    let (background, agents) = batch.expect("the demo announces a group");
    assert!(!background, "foreground by default");
    assert_eq!(agents.len(), 2);
    assert!(agents[0].description.contains("Warsaw"));
    assert!(agents[0].id.starts_with('a'));
    let done = events.iter().find_map(|e| match e {
        StreamEvent::AgentGroupDone { agents, .. } => Some(agents),
        _ => None,
    });
    let done = done.expect("the demo resolves the group");
    assert_eq!(done.len(), 2);
    assert!(done.iter().all(|d| d.ok));
    assert_eq!(events.last(), Some(&StreamEvent::StreamDone));
    // A "background" prompt launches in background mode with launch texts.
    let events = turn_events("call background agents", 0);
    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::AgentBatch {
            background: true,
            ..
        }
    )));
    // /init's canned prompt names AGENTS.md — never the demo.
    let events = turn_events("Generate a file named AGENTS.md", 0);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, StreamEvent::AgentBatch { .. }))
    );
}
