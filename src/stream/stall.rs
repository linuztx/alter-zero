//! The wedged-backend test double (`ALTER_ZERO_STALL_MS`, `docs/interrupt.md`).

use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::context::ContextMessage;

use super::{CancelToken, ReplySource, StreamEvent};

/// A test-only backend that models a real network backend **parked in a
/// blocking read it cannot interrupt**: its thread sleeps for `stall` *without*
/// polling the [`CancelToken`], then — only once the stall elapses — observes
/// the cancel (streaming nothing) or, if it was never cancelled, streams a
/// one-word reply and finishes.
///
/// This is the failure mode the real [`crate::llm::LlmBackend`] hits during the
/// pre-first-token pause (blocked in `req.send()` / the first SSE `read`, which
/// only wake after one op-timeout — see `src/llm/openai.rs`), reproduced
/// deterministically and offline. The event loop must therefore **never
/// `join()`** a cancelled backend on its thread: doing so freezes the UI for
/// the whole stall (the interrupt-lag bug). Selected via `ALTER_ZERO_STALL_MS`
/// and used only by `scripts/smoke.sh` — never in normal operation. See
/// `docs/interrupt.md`.
#[derive(Debug, Clone, Copy)]
pub struct StallAi {
    stall: Duration,
}

impl StallAi {
    /// A stall backend that ignores the cancel for `stall` before it stops.
    #[must_use]
    pub fn new(stall: Duration) -> Self {
        Self { stall }
    }
}

impl ReplySource for StallAi {
    fn spawn(
        &self,
        prompt: String,
        _images: Vec<PathBuf>,
        _context: Vec<ContextMessage>,
        tx: UnboundedSender<StreamEvent>,
        cancel: CancelToken,
    ) -> JoinHandle<()> {
        let stall = self.stall;
        thread::spawn(move || {
            // Block for `stall` in ONE shot, deliberately *not* polling cancel —
            // this is the whole point of the double: a thread wedged in a
            // blocking syscall. A well-behaved loop cancels + detaches us and
            // stays responsive; a loop that join()s us here pays the full stall.
            thread::sleep(stall);
            if cancel.is_cancelled() {
                return; // cancelled while we were blocked — stop silently
            }
            let _ = tx.send(StreamEvent::Chunk(format!("Echo: {prompt}")));
            let _ = tx.send(StreamEvent::StreamDone);
        })
    }

    fn model_name(&self) -> String {
        "stall_model".to_string()
    }
}
