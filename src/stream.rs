//! The dummy AI and the backend seam.
//!
//! [`dummy_response`] and [`chunks`] are pure and unit-tested. [`DummyAi`] is the
//! built-in [`ReplySource`]: a thin background thread that pushes the chunks onto
//! the event channel with a small delay so the reply visibly streams, stopping
//! early if its [`CancelToken`] is tripped. Swap in a real model by implementing
//! [`ReplySource`] — the event loop depends only on the trait, not on this dummy.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// What a backend sends to the event loop. Only the *reply* travels this channel
/// — keyboard input is read on the main thread (see `main.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// A piece of the reply (typically one word).
    Chunk(String),
    /// The backend failed; carries a human-readable message to show the user.
    Error(String),
    /// The reply is complete.
    StreamDone,
}

/// Delay between streamed chunks. Small enough to feel responsive, large
/// enough that the word-by-word reveal is visible.
pub const CHUNK_DELAY: Duration = Duration::from_millis(45);

/// Canned replies. One is chosen deterministically per prompt so the demo has
/// a little variety without any real model behind it.
const RESPONSES: &[&str] = &[
    "Sure! This is a streaming demo, so I'm a dummy reply rather than a real \
     model. Notice how each word appears on its own and longer answers wrap to \
     fit your terminal — try resizing the window while I talk.",
    "Great question. There's no AI behind this yet — these words are streamed \
     from a canned response to show off the inline TUI. Finished messages \
     scroll up into your normal terminal history, just like Claude Code.",
    "Happy to help! For now I only pretend to think. The point of this little \
     program is the rendering: a bottom-pinned input box, live streaming, and \
     a layout that reflows responsively as the terminal changes size.",
];

/// Pick a deterministic dummy reply for a prompt.
///
/// Deterministic so it's testable; varied so the demo isn't monotonous.
#[must_use]
pub fn dummy_response(prompt: &str) -> String {
    let index = prompt.chars().count() % RESPONSES.len();
    RESPONSES[index].to_string()
}

/// Split text into streamable chunks, one per whitespace-delimited word.
///
/// Each chunk keeps its trailing space (`split_inclusive`), so concatenating
/// the chunks reproduces the input exactly — which keeps streaming faithful.
#[must_use]
pub fn chunks(text: &str) -> Vec<String> {
    text.split_inclusive(' ').map(str::to_string).collect()
}

/// A cheap, cloneable cancellation flag shared between the event loop and a
/// running [`ReplySource`]. The loop calls [`CancelToken::cancel`] (e.g. on
/// quit); a well-behaved backend polls [`CancelToken::is_cancelled`] between
/// chunks and stops promptly.
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

/// A source of streamed replies. Implement this to plug a real model into the
/// app: the event loop depends only on this trait, never on a concrete backend.
///
/// `spawn` must return promptly and do the work on a background thread (or task)
/// that *only sends* on `tx` — it must never read stdin (terminal init and
/// `insert_before` own stdin; see `main.rs`). It should poll `cancel` and stop
/// early when cancellation is requested, and may send [`StreamEvent::Error`] to
/// report a failure in place of [`StreamEvent::StreamDone`].
pub trait ReplySource {
    fn spawn(&self, prompt: String, tx: Sender<StreamEvent>, cancel: CancelToken)
    -> JoinHandle<()>;
}

/// The built-in canned-reply backend used by the demo.
#[derive(Debug, Clone, Copy, Default)]
pub struct DummyAi;

impl ReplySource for DummyAi {
    /// Sends one [`StreamEvent::Chunk`] per word (with [`CHUNK_DELAY`] between
    /// them), then a final [`StreamEvent::StreamDone`]. Stops early — sending
    /// nothing further — if `cancel` is tripped or the receiver has hung up.
    fn spawn(
        &self,
        prompt: String,
        tx: Sender<StreamEvent>,
        cancel: CancelToken,
    ) -> JoinHandle<()> {
        thread::spawn(move || {
            for chunk in chunks(&dummy_response(&prompt)) {
                if cancel.is_cancelled() {
                    return; // asked to stop — drop the rest quietly
                }
                if tx.send(StreamEvent::Chunk(chunk)).is_err() {
                    return; // receiver gone — stop quietly
                }
                thread::sleep(CHUNK_DELAY);
            }
            let _ = tx.send(StreamEvent::StreamDone);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn dummy_response_is_non_empty() {
        assert!(!dummy_response("hello").is_empty());
    }

    #[test]
    fn dummy_response_is_deterministic() {
        assert_eq!(dummy_response("hello"), dummy_response("hello"));
    }

    #[test]
    fn chunks_concatenate_back_to_the_original_text() {
        let text = "The quick brown fox jumps over the lazy dog.";
        assert_eq!(chunks(text).concat(), text);
    }

    #[test]
    fn chunks_splits_multi_word_text_into_several_pieces() {
        assert!(chunks("one two three").len() > 1);
    }

    #[test]
    fn chunks_of_empty_text_is_empty() {
        assert!(chunks("").is_empty());
    }

    #[test]
    fn every_dummy_response_chunks_round_trip() {
        // Guards against a response that can't be streamed faithfully.
        for prompt in ["", "a", "tell me a story", "what is ratatui?"] {
            let response = dummy_response(prompt);
            assert_eq!(chunks(&response).concat(), response);
        }
    }

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

    #[test]
    fn dummy_ai_emits_all_chunks_then_done() {
        let (tx, rx) = mpsc::channel();
        let prompt = "hi".to_string();
        let expected = dummy_response(&prompt);
        let handle = DummyAi.spawn(prompt, tx, CancelToken::new());

        let mut streamed = String::new();
        let mut saw_done = false;
        for event in rx {
            match event {
                StreamEvent::Chunk(c) => streamed.push_str(&c),
                StreamEvent::StreamDone => {
                    saw_done = true;
                    break;
                }
                StreamEvent::Error(e) => panic!("dummy never errors, got {e:?}"),
            }
        }
        handle.join().unwrap();

        assert!(saw_done, "stream must end with StreamDone");
        assert_eq!(streamed, expected);
    }

    #[test]
    fn dummy_ai_sends_nothing_when_cancelled_before_it_starts() {
        let (tx, rx) = mpsc::channel();
        let cancel = CancelToken::new();
        cancel.cancel();
        DummyAi
            .spawn("hello".to_string(), tx, cancel)
            .join()
            .unwrap();
        // Cancelled before the first chunk → no Chunk and no StreamDone arrive.
        assert!(
            rx.try_recv().is_err(),
            "a cancelled backend streams nothing"
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
                tx: Sender<StreamEvent>,
                _cancel: CancelToken,
            ) -> JoinHandle<()> {
                thread::spawn(move || {
                    let _ = tx.send(StreamEvent::Error("backend exploded".to_string()));
                })
            }
        }
        let (tx, rx) = mpsc::channel();
        Failing
            .spawn("x".to_string(), tx, CancelToken::new())
            .join()
            .unwrap();
        assert_eq!(
            rx.recv().unwrap(),
            StreamEvent::Error("backend exploded".to_string())
        );
    }
}
