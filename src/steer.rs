//! The mid-turn queue's **delivery seam**: messages the user submitted while a
//! turn was already running, waiting to be folded into that turn's context at
//! its next round boundary. See `docs/queue.md`.
//!
//! One type, shared by both sides of the app, which is the point: the main
//! session's queue and a subagent session's queue are the *same* mechanism
//! seen twice. The event loop pushes ([`SteerQueue::push`]) and the agent
//! loop's own thread drains ([`SteerQueue::take`]) at the top of every round —
//! right after the previous round's tool results — so a message typed mid-turn
//! reaches the model **within that turn** instead of waiting for it to finish
//! (codex's steering; `run_agent`'s `pending_inputs` seam).
//!
//! It holds only what crosses the thread boundary. What the user *sees* — the
//! inset `❯ …` rows above the box — is the app's own mirror
//! ([`crate::app::App::steered`], [`crate::agents::AgentRun::queued`]), which
//! the delivery event ([`crate::stream::StreamEvent::Steered`]) turns into a
//! real user message once the model actually has it.

use std::sync::{Arc, Mutex};

/// A running turn's pending user messages — cloneable, and every clone is the
/// same queue (the [`crate::background::BackgroundRegistry`] shape). The
/// boundary keeps one and hands a clone to each backend build, so a `/model`
/// switch or a `/settings` change re-attaches the queue the loop is already
/// pushing into.
///
/// A poisoned lock (a panicked tool thread) is treated as an empty queue
/// rather than taken as a reason to bring the UI down: a lost mid-turn message
/// is recoverable — it stays in the app's mirror and dispatches as the next
/// turn — where a panic here is not.
#[derive(Clone, Default, Debug)]
pub struct SteerQueue {
    inner: Arc<Mutex<Vec<String>>>,
}

impl SteerQueue {
    /// An empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hand one message to the running turn — read at its next round boundary.
    pub fn push(&self, text: &str) {
        if let Ok(mut pending) = self.inner.lock() {
            pending.push(text.to_string());
        }
    }

    /// Drain every pending message, oldest first — what the agent loop calls
    /// once per round. Empty when nothing is waiting, which is the common case
    /// and costs one uncontended lock.
    #[must_use]
    pub fn take(&self) -> Vec<String> {
        self.inner
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default()
    }

    /// Take back the **newest** still-undelivered message, for Alt+Up to pull
    /// it into the composer. `None` once the round boundary has taken it — the
    /// model has it, so there is nothing left to edit.
    #[must_use]
    pub fn take_last(&self) -> Option<String> {
        self.inner.lock().ok()?.pop()
    }

    /// Is nothing waiting?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().map_or(true, |pending| pending.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pushed_message_is_taken_once() {
        let queue = SteerQueue::default();
        queue.push("also check Manila");
        assert_eq!(queue.take(), vec!["also check Manila".to_string()]);
        assert!(queue.take().is_empty(), "drained once");
    }

    #[test]
    fn messages_are_taken_in_submission_order() {
        let queue = SteerQueue::default();
        queue.push("first");
        queue.push("second");
        assert_eq!(
            queue.take(),
            vec!["first".to_string(), "second".to_string()],
            "the round boundary reads them oldest first"
        );
    }

    #[test]
    fn a_clone_shares_one_queue() {
        let queue = SteerQueue::default();
        let handed_to_the_backend = queue.clone();
        queue.push("mid-turn note");
        assert_eq!(
            handed_to_the_backend.take(),
            vec!["mid-turn note".to_string()],
            "the boundary pushes and the agent thread drains one queue"
        );
    }

    #[test]
    fn take_last_reclaims_only_an_undelivered_message() {
        let queue = SteerQueue::default();
        queue.push("first");
        queue.push("second");
        assert_eq!(
            queue.take_last(),
            Some("second".to_string()),
            "Alt+Up pulls the newest still-undelivered message back"
        );
        assert_eq!(
            queue.take(),
            vec!["first".to_string()],
            "the older one stays"
        );
        assert_eq!(queue.take_last(), None, "nothing left to reclaim");
    }

    #[test]
    fn is_empty_tracks_the_pending_set() {
        let queue = SteerQueue::default();
        assert!(queue.is_empty());
        queue.push("x");
        assert!(!queue.is_empty());
        let _ = queue.take();
        assert!(queue.is_empty());
    }
}
