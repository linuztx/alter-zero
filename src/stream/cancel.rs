//! The cancellation flag shared between the event loop and a running backend.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A cheap, cloneable cancellation flag shared between the event loop and a
/// running [`ReplySource`](super::ReplySource). The loop calls
/// [`CancelToken::cancel`] (e.g. on quit); a well-behaved backend polls
/// [`CancelToken::is_cancelled`] between chunks and stops promptly.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh, uncancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent; observed by every clone.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Has cancellation been requested?
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
