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

/// Two tokens are equal when they are the **same flag** — clones of one
/// another — never merely two flags in the same state: what a token means is
/// which run it stops.
impl PartialEq for CancelToken {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for CancelToken {}
