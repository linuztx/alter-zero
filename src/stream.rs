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
    /// A tool call has started executing. The loop shows it live (blue) until the
    /// matching [`StreamEvent::ToolEnd`] arrives. `args` is a short summary for
    /// the `name(args)` header.
    ToolStart { name: String, args: String },
    /// The in-flight tool call finished with this `output` and outcome (`ok` →
    /// green, else red). Always follows a [`StreamEvent::ToolStart`].
    ToolEnd { output: String, ok: bool },
    /// The backend failed; carries a human-readable message to show the user.
    Error(String),
    /// The reply is complete.
    StreamDone,
}

/// Delay between streamed chunks. Small enough to feel responsive, large
/// enough that the word-by-word reveal is visible.
pub const CHUNK_DELAY: Duration = Duration::from_millis(45);

/// How long a dummy tool "runs" — the pause between its `ToolStart` and
/// `ToolEnd` — so the blue running state is visible before it resolves.
pub const TOOL_DELAY: Duration = Duration::from_millis(450);

/// Canned multi-line output for the dummy `Read` tool (resolves green).
const DUMMY_READ_OUTPUT: &str = "fn main() -> io::Result<()> {\n    \
    let mut term = InlineViewport::init(ui::LIVE_MIN_HEIGHT)?;\n    \
    let result = run(&mut term);\n    \
    let restored = term.restore();\n    \
    result.and(restored)\n}";

/// Canned multi-line output for the dummy `Bash` tool (resolves red, to show
/// the failure colour in the demo).
const DUMMY_BASH_OUTPUT: &str = "grep: TODO: no matches found\n\
    searched 14 files in src/\nexit status 1";

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

/// The full ordered sequence of events for one dummy turn, with tool calls
/// **interleaved** in the reply: stream the first half of the text, run a
/// `Read` tool (resolves green) and a `Bash` tool (resolves red, for colour
/// variety), then stream the rest and finish.
///
/// Pure and deterministic so it is unit-testable; [`DummyAi`] just plays it back
/// on a thread with delays. The `Chunk` events still concatenate to exactly
/// [`dummy_response`], so streaming stays faithful.
#[must_use]
pub fn turn_events(prompt: &str) -> Vec<StreamEvent> {
    let reply = dummy_response(prompt);
    let words: Vec<&str> = reply.split_inclusive(' ').collect();
    let mid = (words.len() / 2).max(1).min(words.len());
    let first: String = words[..mid].concat();
    let second: String = words[mid..].concat();

    let mut events = Vec::new();
    events.extend(chunks(&first).into_iter().map(StreamEvent::Chunk));
    events.push(StreamEvent::ToolStart {
        name: "Read".to_string(),
        args: "src/main.rs".to_string(),
    });
    events.push(StreamEvent::ToolEnd {
        output: DUMMY_READ_OUTPUT.to_string(),
        ok: true,
    });
    events.push(StreamEvent::ToolStart {
        name: "Bash".to_string(),
        args: "grep -n TODO".to_string(),
    });
    events.push(StreamEvent::ToolEnd {
        output: DUMMY_BASH_OUTPUT.to_string(),
        ok: false,
    });
    events.extend(chunks(&second).into_iter().map(StreamEvent::Chunk));
    events.push(StreamEvent::StreamDone);
    events
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
    /// Plays back [`turn_events`]: streams the reply word-by-word (with
    /// [`CHUNK_DELAY`] between words) with a `Read` then a `Bash` tool call
    /// interleaved, pausing [`TOOL_DELAY`] after each `ToolStart` so the blue
    /// running state shows before it resolves, and ends with
    /// [`StreamEvent::StreamDone`]. Stops early — sending nothing further — if
    /// `cancel` is tripped or the receiver has hung up.
    fn spawn(
        &self,
        prompt: String,
        tx: Sender<StreamEvent>,
        cancel: CancelToken,
    ) -> JoinHandle<()> {
        thread::spawn(move || {
            for event in turn_events(&prompt) {
                if cancel.is_cancelled() {
                    return; // asked to stop — drop the rest quietly
                }
                // Pause *after* a word or a tool start: a tool "runs" for
                // TOOL_DELAY (blue) before its ToolEnd resolves it.
                let pause = match &event {
                    StreamEvent::Chunk(_) => Some(CHUNK_DELAY),
                    StreamEvent::ToolStart { .. } => Some(TOOL_DELAY),
                    _ => None,
                };
                if tx.send(event).is_err() {
                    return; // receiver gone — stop quietly
                }
                if let Some(pause) = pause {
                    nap(pause, &cancel);
                }
            }
        })
    }
}

/// Sleep up to `dur`, in short slices, returning early the moment `cancel` is
/// tripped — so a quit during a long tool "run" is still reaped promptly.
fn nap(dur: Duration, cancel: &CancelToken) {
    const SLICE: Duration = Duration::from_millis(20);
    let mut left = dur;
    while left > Duration::ZERO {
        if cancel.is_cancelled() {
            return;
        }
        let slice = SLICE.min(left);
        thread::sleep(slice);
        left -= slice;
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
    fn turn_events_chunks_still_reconstruct_the_reply() {
        // Tool events are interleaved, but the Chunk events alone must still
        // concatenate to exactly the dummy reply.
        let prompt = "tell me something";
        let text: String = turn_events(prompt)
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Chunk(c) => Some(c.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, dummy_response(prompt));
    }

    #[test]
    fn turn_events_interleaves_at_least_one_tool_call() {
        let events = turn_events("hi");
        let starts = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolStart { .. }))
            .count();
        let ends = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolEnd { .. }))
            .count();
        assert!(starts >= 1, "a turn runs at least one tool");
        assert_eq!(starts, ends, "every ToolStart has a matching ToolEnd");
    }

    #[test]
    fn turn_events_each_tool_start_is_immediately_resolved() {
        // Tools don't nest: every ToolStart is followed straight away by a
        // ToolEnd, so the loop only ever tracks one running tool at a time.
        let events = turn_events("anything");
        for (i, event) in events.iter().enumerate() {
            if matches!(event, StreamEvent::ToolStart { .. }) {
                assert!(
                    matches!(events.get(i + 1), Some(StreamEvent::ToolEnd { .. })),
                    "ToolStart at {i} is immediately followed by a ToolEnd"
                );
            }
        }
    }

    #[test]
    fn turn_events_shows_both_a_success_and_a_failure() {
        // The demo exercises green and red: at least one ok tool and one failing.
        let events = turn_events("x");
        let oks = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolEnd { ok: true, .. }))
            .count();
        let fails = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolEnd { ok: false, .. }))
            .count();
        assert!(oks >= 1, "at least one tool succeeds (green)");
        assert!(fails >= 1, "at least one tool fails (red)");
    }

    #[test]
    fn turn_events_ends_with_stream_done() {
        assert_eq!(turn_events("x").last(), Some(&StreamEvent::StreamDone));
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
    fn dummy_ai_emits_all_chunks_and_tool_calls_then_done() {
        let (tx, rx) = mpsc::channel();
        let prompt = "hi".to_string();
        let expected = dummy_response(&prompt);
        let handle = DummyAi.spawn(prompt, tx, CancelToken::new());

        let mut streamed = String::new();
        let mut saw_done = false;
        let mut tool_starts = 0;
        let mut tool_ends = 0;
        for event in rx {
            match event {
                StreamEvent::Chunk(c) => streamed.push_str(&c),
                StreamEvent::ToolStart { .. } => tool_starts += 1,
                StreamEvent::ToolEnd { .. } => tool_ends += 1,
                StreamEvent::StreamDone => {
                    saw_done = true;
                    break;
                }
                StreamEvent::Error(e) => panic!("dummy never errors, got {e:?}"),
            }
        }
        handle.join().unwrap();

        assert!(saw_done, "stream must end with StreamDone");
        assert_eq!(streamed, expected, "chunks still reconstruct the reply");
        assert!(tool_starts >= 1, "the dummy streams at least one tool call");
        assert_eq!(tool_starts, tool_ends, "every tool that starts also ends");
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
