//! The cancellation flag a long store operation polls. The app bridges its
//! own turn token to this one (cancel this when that is cancelled), so this
//! crate depends on nothing of the app's.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A cheap, cloneable cancellation flag: every clone sees one flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh, uncancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation; idempotent.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Has cancellation been requested?
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Two tokens are equal when they are the same flag.
impl PartialEq for CancelToken {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for CancelToken {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_clone_sees_one_flag() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
        assert_eq!(token, clone);
        assert_ne!(token, CancelToken::new());
    }
}
