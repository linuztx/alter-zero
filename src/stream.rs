//! The dummy AI.
//!
//! [`dummy_response`] and [`chunks`] are pure and unit-tested. [`spawn_stream`]
//! is the only stateful piece: a thin background thread that pushes the chunks
//! onto the event channel with a small delay so the reply visibly streams.

use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// What the streaming thread sends to the event loop. Only the *reply* travels
/// this channel — keyboard input is read on the main thread (see `main.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// A piece of the reply (typically one word).
    Chunk(String),
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
pub fn dummy_response(prompt: &str) -> String {
    let index = prompt.chars().count() % RESPONSES.len();
    RESPONSES[index].to_string()
}

/// Split text into streamable chunks, one per whitespace-delimited word.
///
/// Each chunk keeps its trailing space (`split_inclusive`), so concatenating
/// the chunks reproduces the input exactly — which keeps streaming faithful.
pub fn chunks(text: &str) -> Vec<String> {
    text.split_inclusive(' ').map(str::to_string).collect()
}

/// Stream a dummy reply on a background thread.
///
/// Sends one [`StreamEvent::Chunk`] per word (with [`CHUNK_DELAY`] between
/// them), then a final [`StreamEvent::StreamDone`]. Sends stop early if the
/// receiver has hung up (e.g. the app quit mid-reply).
pub fn spawn_stream(prompt: String, tx: Sender<StreamEvent>) -> JoinHandle<()> {
    thread::spawn(move || {
        for chunk in chunks(&dummy_response(&prompt)) {
            if tx.send(StreamEvent::Chunk(chunk)).is_err() {
                return; // receiver gone — stop quietly
            }
            thread::sleep(CHUNK_DELAY);
        }
        let _ = tx.send(StreamEvent::StreamDone);
    })
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
    fn spawn_stream_emits_all_chunks_then_done() {
        let (tx, rx) = mpsc::channel();
        let prompt = "hi".to_string();
        let expected = dummy_response(&prompt);
        let handle = spawn_stream(prompt, tx);

        let mut streamed = String::new();
        let mut saw_done = false;
        for event in rx {
            match event {
                StreamEvent::Chunk(c) => streamed.push_str(&c),
                StreamEvent::StreamDone => {
                    saw_done = true;
                    break;
                }
            }
        }
        handle.join().unwrap();

        assert!(saw_done, "stream must end with StreamDone");
        assert_eq!(streamed, expected);
    }
}
