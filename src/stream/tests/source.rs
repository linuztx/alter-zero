//! The [`ReplySource`] seam itself — the contract every backend keeps.

use std::path::PathBuf;
use std::thread::{self, JoinHandle};

use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::context::ContextMessage;

use super::*;

#[test]
fn agent_system_prompt_defaults_to_the_backends_own_prompt() {
    // A backend without a distinct subagent prompt would send a launched
    // agent its own prompt — the trait default says so honestly (the
    // dummy's None included); `LlmBackend` overrides it with the note-
    // suffixed one (docs/agent-tool.md).
    struct Fixed;
    impl ReplySource for Fixed {
        fn spawn(
            &self,
            _prompt: String,
            _images: Vec<PathBuf>,
            _context: Vec<ContextMessage>,
            _tx: UnboundedSender<StreamEvent>,
            _cancel: CancelToken,
        ) -> JoinHandle<()> {
            thread::spawn(|| {})
        }
        fn model_name(&self) -> String {
            "fixed".to_string()
        }
        fn system_prompt(&self) -> Option<String> {
            Some("base".to_string())
        }
    }
    assert_eq!(
        Fixed.agent_system_prompt("general-purpose").as_deref(),
        Some("base")
    );
    assert!(
        DummyAi::new()
            .agent_system_prompt("general-purpose")
            .is_none()
    );
}

#[test]
fn a_reply_source_can_report_an_error() {
    // Proves the trait + protocol carry backend failures, as a real model
    // would, without any dummy-specific machinery.
    struct Failing;
    impl ReplySource for Failing {
        fn spawn(
            &self,
            _prompt: String,
            _images: Vec<PathBuf>,
            _context: Vec<ContextMessage>,
            tx: UnboundedSender<StreamEvent>,
            _cancel: CancelToken,
        ) -> JoinHandle<()> {
            thread::spawn(move || {
                let _ = tx.send(StreamEvent::Error("backend exploded".to_string()));
            })
        }

        fn model_name(&self) -> String {
            "failing".to_string()
        }
    }
    let (tx, mut rx) = unbounded_channel();
    Failing
        .spawn("x".to_string(), vec![], vec![], tx, CancelToken::new())
        .join()
        .unwrap();
    assert_eq!(
        rx.blocking_recv().unwrap(),
        StreamEvent::Error("backend exploded".to_string())
    );
}
