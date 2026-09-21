//! [`DummyAi`]'s playback: the startup pause, the streamed turn, the cancel.

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc::unbounded_channel;

use super::*;

#[test]
fn dummy_ai_reports_its_model_name() {
    // The footer under the input box names the active backend's model
    // (see docs/footer.md); the dummy reports its placeholder id.
    assert_eq!(DummyAi::default().model_name(), "dummy_model_name");
}

#[test]
fn dummy_ai_waits_the_startup_delay_before_the_first_chunk() {
    // The dummy pauses before streaming so the status indicator (spinner,
    // ticking timer, `↑ N tokens` for the just-sent input) is visible
    // first. Use a short, deterministic delay; assert the first event only
    // arrives after it (a lower bound — the thread genuinely sleeps).
    let delay = Duration::from_millis(150);
    let (tx, mut rx) = unbounded_channel();
    let start = std::time::Instant::now();
    let handle = DummyAi::with_startup_delay(delay).spawn(
        "hi".to_string(),
        vec![],
        vec![],
        tx,
        CancelToken::new(),
    );
    let first = rx.blocking_recv().expect("a first event arrives");
    assert!(
        start.elapsed() >= delay,
        "the first chunk waits out the startup delay"
    );
    assert!(
        matches!(first, StreamEvent::Chunk(_)),
        "streaming still opens with reply text"
    );
    while rx.blocking_recv().is_some() {} // drain the rest
    handle.join().unwrap();
}

#[test]
fn a_cancel_during_the_startup_delay_streams_nothing() {
    // Esc during the pre-stream pause must reap the thread at once — the
    // interruptible nap returns early and no events are sent.
    let (tx, mut rx) = unbounded_channel();
    let cancel = CancelToken::new();
    let backend = DummyAi::with_startup_delay(Duration::from_secs(30));
    let handle = backend.spawn("hello".to_string(), vec![], vec![], tx, cancel.clone());
    cancel.cancel();
    handle.join().unwrap();
    assert!(
        rx.try_recv().is_err(),
        "a cancel during the delay streams nothing"
    );
}

#[test]
fn dummy_ai_emits_all_chunks_and_tool_calls_then_done() {
    let (tx, mut rx) = unbounded_channel();
    let prompt = "hi".to_string();
    let expected = dummy_response(&prompt);
    // Zero startup delay so this content test stays fast.
    let handle = DummyAi::with_startup_delay(Duration::ZERO).spawn(
        prompt,
        vec![],
        vec![],
        tx,
        CancelToken::new(),
    );

    let mut streamed = String::new();
    let mut saw_done = false;
    let mut tool_batches = 0;
    let mut batched_calls = 0;
    let mut tool_starts = 0;
    let mut tool_ends = 0;
    let mut tool_output_chunks = 0;
    let mut think_starts = 0;
    let mut think_chunks = 0;
    let mut think_ends = 0;
    let mut tool_call_deltas = 0;
    // `blocking_recv` waits for each delayed event (no runtime here, so it's
    // allowed); `None` means the backend dropped its sender.
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(c) => streamed.push_str(&c),
            StreamEvent::ToolBatch(items) => {
                tool_batches += 1;
                batched_calls = items.len();
            }
            StreamEvent::ToolStart { .. } => tool_starts += 1,
            StreamEvent::ToolEnd { .. } => tool_ends += 1,
            StreamEvent::ToolOutput(_) => tool_output_chunks += 1,
            StreamEvent::AgentBatch { .. } | StreamEvent::AgentGroupDone { .. } => {}
            StreamEvent::RoundCalls(_) => {}
            StreamEvent::HookNote { .. } | StreamEvent::PromptBlocked { .. } => {}
            StreamEvent::ThinkingStart => think_starts += 1,
            StreamEvent::ThinkingChunk(_) => think_chunks += 1,
            StreamEvent::ThinkingEnd => think_ends += 1,
            StreamEvent::ToolCallDelta(_) => tool_call_deltas += 1,
            // The default demo never touches the task list; the tasks demo
            // has its own suite coverage (docs/task-tools.md).
            StreamEvent::TaskCall { .. } => {}
            StreamEvent::StreamDone => {
                saw_done = true;
                break;
            }
            StreamEvent::Retrying { .. } => panic!("the dummy never retries"),
            StreamEvent::Usage(_) => panic!("the dummy never reports usage"),
            StreamEvent::Permission(_) => {
                panic!("no gate attached — the dummy never asks")
            }
            StreamEvent::AskUser(_) => {
                panic!("no ask gate attached — the dummy never questions")
            }
            StreamEvent::ToolBackgrounded { .. } => {
                panic!("the dummy never backgrounds a tool")
            }
            StreamEvent::ToolRejected { .. } => {
                panic!("no gate attached — nothing is ever rejected")
            }
            // The file tools' two-text resolution (`docs/tools.md`) — the
            // ask tool needs a gate, but a `write`/`edit` ack does not: it
            // closes its call exactly as a `ToolEnd` does.
            StreamEvent::ToolAnswered {
                display, result, ..
            } => {
                assert!(!display.is_empty(), "the cell keeps the numbered body");
                assert!(!result.is_empty(), "the model reads the ack");
                tool_ends += 1;
            }
            StreamEvent::ToolNote(_) => {
                panic!("no gate attached — the classifier never speaks")
            }
            StreamEvent::Steered { .. } => panic!("nothing was queued into this turn"),
            StreamEvent::Error(e) => panic!("dummy never errors, got {e:?}"),
        }
    }
    handle.join().unwrap();

    assert!(saw_done, "stream must end with StreamDone");
    assert_eq!(streamed, expected, "chunks still reconstruct the reply");
    assert!(tool_starts >= 1, "the dummy streams at least one tool call");
    assert_eq!(
        tool_starts, tool_ends,
        "every tool that starts also resolves"
    );
    assert!(
        tool_output_chunks >= 1,
        "the dummy streams live tool output (docs/tool-streaming.md)"
    );
    assert_eq!(
        tool_batches, 1,
        "the dummy announces its parallel batch once"
    );
    assert!(
        batched_calls >= 2,
        "the announced batch has several parallel calls"
    );
    assert!(
        tool_call_deltas >= 1,
        "the dummy generates each tool call first (ticking the tally)"
    );
    assert_eq!(think_starts, 1, "the dummy thinks once");
    assert!(think_chunks >= 1, "reasoning deltas stream while thinking");
    assert_eq!(
        think_starts, think_ends,
        "every think that starts also ends"
    );
}

#[test]
fn dummy_ai_acknowledges_attached_images_it_was_spawned_with() {
    // The typed image channel reaches the backend: spawning with two paths
    // makes the dummy open its reply with the acknowledgement.
    let (tx, mut rx) = unbounded_channel();
    let images = vec![PathBuf::from("/tmp/a.png"), PathBuf::from("/tmp/b.png")];
    let handle = DummyAi::with_startup_delay(Duration::ZERO).spawn(
        "describe".to_string(),
        images,
        vec![],
        tx,
        CancelToken::new(),
    );
    let mut streamed = String::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(c) => streamed.push_str(&c),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().unwrap();
    assert!(
        streamed.starts_with("Looking at your 2 images. "),
        "the dummy acknowledges the two images up front, got {streamed:?}"
    );
}

#[test]
fn dummy_ai_sends_nothing_when_cancelled_before_it_starts() {
    let (tx, mut rx) = unbounded_channel();
    let cancel = CancelToken::new();
    cancel.cancel();
    DummyAi::default()
        .spawn("hello".to_string(), vec![], vec![], tx, cancel)
        .join()
        .unwrap();
    // Cancelled before the first chunk → no Chunk and no StreamDone arrive.
    assert!(
        rx.try_recv().is_err(),
        "a cancelled backend streams nothing"
    );
}

#[test]
fn the_dummy_takes_a_queued_message_at_its_next_tool_boundary() {
    // The offline mirror of a real round boundary (docs/queue.md): a message
    // queued while the scripted turn runs is announced as soon as the turn's
    // next tool call resolves — not at the end of the turn — so the whole
    // mid-turn queue is drivable with no network.
    let queue = crate::steer::SteerQueue::new();
    queue.push("also check the tests");
    let (tx, mut rx) = unbounded_channel();
    let handle = DummyAi::with_startup_delay(Duration::ZERO)
        .with_steer(queue.clone())
        .spawn("hi".to_string(), vec![], vec![], tx, CancelToken::new());

    let mut steered_after = None;
    let mut tool_ends = 0;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::ToolEnd { .. } => tool_ends += 1,
            StreamEvent::Steered { text } => steered_after = Some((text, tool_ends)),
            _ => {}
        }
    }
    handle.join().expect("dummy thread");
    assert_eq!(
        steered_after,
        Some(("also check the tests".to_string(), 1)),
        "taken right after the first tool call resolved"
    );
    assert!(
        queue.is_empty(),
        "and drained, so the turn end reclaims nothing"
    );
}

#[test]
fn the_dummy_queues_a_chat_message_into_a_running_agent() {
    // The offline mirror of `LlmBackend::spawn_agent_chat` (docs/queue.md):
    // the registry decides, and a running agent's session takes the message
    // onto its queue for the next round boundary. Without this the agent
    // view's own queue had no offline coverage at all — the dummy declined
    // every chat.
    let (agent_tx, _agent_rx) = unbounded_channel();
    let registry = crate::agents::AgentRegistry::new(agent_tx);
    let (id, _cancel) = registry.register(crate::agents::GENERAL_PURPOSE);
    let dummy = DummyAi::with_startup_delay(Duration::ZERO).with_agents(registry.clone());
    assert_eq!(
        dummy.spawn_agent_chat(&id, "also add Elixir"),
        crate::stream::AgentChatDelivery::Queued
    );
    assert_eq!(registry.take_pending_inputs(&id), vec!["also add Elixir"]);
}

#[test]
fn the_dummy_declines_a_chat_with_no_subagents_attached() {
    let dummy = DummyAi::with_startup_delay(Duration::ZERO);
    assert_eq!(
        dummy.spawn_agent_chat("a1", "hello"),
        crate::stream::AgentChatDelivery::Declined
    );
}
