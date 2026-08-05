//! [`StallAi`] — the backend wedged in a blocking read (`docs/interrupt.md`).

use std::time::Duration;

use tokio::sync::mpsc::unbounded_channel;

use super::*;

#[test]
fn stall_ai_ignores_cancel_until_its_stall_elapses() {
    // The stall double models a backend wedged in a blocking read: a cancel
    // does NOT stop it early, so a caller that join()s it pays the whole
    // stall (the interrupt-lag freeze the loop must avoid — it detaches
    // instead; see docs/interrupt.md). A short stall keeps the test fast.
    let stall = Duration::from_millis(200);
    let (tx, mut rx) = unbounded_channel();
    let cancel = CancelToken::new();
    let start = std::time::Instant::now();
    let handle = StallAi::new(stall).spawn("hi".to_string(), vec![], vec![], tx, cancel.clone());
    cancel.cancel(); // interrupt immediately — the stall ignores it
    handle.join().unwrap();
    assert!(
        start.elapsed() >= stall,
        "joining a cancelled stall backend blocks for the full stall"
    );
    assert!(
        rx.try_recv().is_err(),
        "a stall cancelled mid-block streams nothing"
    );
}

#[test]
fn stall_ai_streams_a_reply_when_not_cancelled() {
    // Left to run, the stall backend completes a normal turn, so the smoke
    // harness can also exercise an uninterrupted stalled turn.
    let (tx, mut rx) = unbounded_channel();
    let handle = StallAi::new(Duration::from_millis(10)).spawn(
        "ping".to_string(),
        vec![],
        vec![],
        tx,
        CancelToken::new(),
    );
    let first = rx.blocking_recv().expect("a chunk arrives");
    assert!(matches!(first, StreamEvent::Chunk(_)));
    assert_eq!(rx.blocking_recv(), Some(StreamEvent::StreamDone));
    handle.join().unwrap();
}
