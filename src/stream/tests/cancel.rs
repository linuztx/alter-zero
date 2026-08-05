//! [`CancelToken`] — the shared stop flag.

use super::*;

#[test]
fn cancel_token_starts_uncancelled_and_latches() {
    let token = CancelToken::new();
    assert!(!token.is_cancelled());
    token.cancel();
    assert!(token.is_cancelled());
}

#[test]
fn cancel_token_clone_shares_the_same_flag() {
    let token = CancelToken::new();
    let clone = token.clone();
    token.cancel();
    assert!(clone.is_cancelled(), "a clone observes the cancellation");
}
