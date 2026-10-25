//! A reply a dropped connection cut short, and the request that asks for the
//! rest of it (`docs/dropped-reply.md`).
//!
//! A round whose stream fails after its reply began cannot simply be sent
//! again: the text is already on screen, and a fresh answer would say it a
//! second time. So the round keeps what its failed attempts streamed
//! ([`CutOff::extend`]) and the next attempt **continues** it
//! ([`CutOff::request`]): the round's own messages, then the reply so far as
//! the assistant's own message, then [`CONTINUE_NOTE`] asking for the rest.
//! What comes back streams on after what is on screen, and the round's
//! outcome is the one reply the screen shows ([`CutOff::finish`]) — the note
//! is never replayed. Pure: `llm::backend`'s round holds one per round, and
//! the tests here drive it with no network.

use super::openai::StreamOutcome;
use super::{ChatMessage, MessageContent};

/// The user-role note a continuation closes on — what the model reads in
/// place of the rest of its own reply. It asks for the rest from the exact
/// character the text stopped at, since whatever comes back is appended to
/// what the screen already shows, and it says what became of a call the
/// model was making: a call runs only once its round ends, so one the drop
/// cut off never ran.
pub(super) const CONTINUE_NOTE: &str = "[Your reply above was cut off when the connection \
     dropped. Continue it from exactly where it stops — even mid-word, your first characters \
     follow straight on from its last — without repeating any of it or mentioning the \
     interruption. If you were making a tool call, it did not go through: make it again.]";

/// Whether `message` is the note a continuation closes on — the harness's
/// words in a user-role message, which a provider billing per user turn must
/// not count as one (`copilot::initiator`).
#[must_use]
pub(super) fn is_note(message: &ChatMessage) -> bool {
    message.role == "user"
        && matches!(&message.content, MessageContent::Text(text) if text == CONTINUE_NOTE)
}

/// What a round's attempts streamed of its reply before the connection
/// dropped, in order — empty until one does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CutOff {
    text: String,
}

impl CutOff {
    /// Keep the reply text one failed attempt streamed, after whatever the
    /// attempts before it did.
    pub(super) fn extend(&mut self, streamed: &str) {
        self.text.push_str(streamed);
    }

    /// The next attempt's messages. While no reply has been cut off — no
    /// attempt failed yet, or what streamed was only reasoning or a call
    /// being made, none of which a fresh answer repeats on screen — the
    /// round's own; once one has, those, then the reply so far as the
    /// assistant's own message, then [`CONTINUE_NOTE`].
    #[must_use]
    pub(super) fn request(&self, messages: &[ChatMessage]) -> Vec<ChatMessage> {
        let mut request = messages.to_vec();
        if !self.text.trim().is_empty() {
            request.push(ChatMessage::assistant(self.text.as_str()));
            request.push(ChatMessage::user(CONTINUE_NOTE));
        }
        request
    }

    /// The finished attempt's outcome as the round's: the reply text cut off
    /// before it, then its own — one reply, as the screen shows it.
    #[must_use]
    pub(super) fn finish(&self, mut outcome: StreamOutcome) -> StreamOutcome {
        outcome.text.response.insert_str(0, &self.text);
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::thinking::ChatStreamResult;

    /// The round a continuation is built on: the persona and one question.
    fn round() -> Vec<ChatMessage> {
        vec![ChatMessage::system("persona"), ChatMessage::user("explain")]
    }

    /// A finished stream that answered `text`, its other fields set so a test
    /// can see they ride through.
    fn answered(text: &str) -> StreamOutcome {
        StreamOutcome {
            text: ChatStreamResult {
                response: text.into(),
                reasoning: "thought".into(),
            },
            finish_reason: Some("stop".into()),
            ..StreamOutcome::default()
        }
    }

    #[test]
    fn nothing_cut_off_sends_the_round_as_it_is() {
        assert_eq!(CutOff::default().request(&round()), round());
    }

    #[test]
    fn a_reply_cut_off_is_asked_to_carry_on_from_the_reply_so_far() {
        let mut cut = CutOff::default();
        cut.extend("The connection drops in the mid");
        let mut expected = round();
        expected.push(ChatMessage::assistant("The connection drops in the mid"));
        expected.push(ChatMessage::user(CONTINUE_NOTE));
        assert_eq!(cut.request(&round()), expected);
    }

    #[test]
    fn whitespace_alone_is_no_reply_to_carry_on() {
        // Nothing on screen reads as a reply yet, so a fresh answer repeats
        // nothing: the round goes out as it is.
        let mut cut = CutOff::default();
        cut.extend("\n\n");
        assert_eq!(cut.request(&round()), round());
    }

    #[test]
    fn each_drop_carries_on_from_everything_streamed_before_it() {
        let mut cut = CutOff::default();
        cut.extend("The connection ");
        cut.extend("drops twice");
        let request = cut.request(&round());
        assert_eq!(request.len(), round().len() + 2, "one reply, one note");
        assert_eq!(
            request[round().len()],
            ChatMessage::assistant("The connection drops twice")
        );
    }

    #[test]
    fn the_finished_round_reads_as_one_reply() {
        let mut cut = CutOff::default();
        cut.extend("The connection drops in the mid");
        let finished = cut.finish(answered("dle of it."));
        assert_eq!(
            finished.text.response,
            "The connection drops in the middle of it."
        );
        assert_eq!(
            finished,
            StreamOutcome {
                text: ChatStreamResult {
                    response: "The connection drops in the middle of it.".into(),
                    reasoning: "thought".into(),
                },
                ..answered("")
            },
            "everything else is the finished attempt's own"
        );
    }

    #[test]
    fn a_round_nothing_was_cut_from_finishes_as_it_came() {
        assert_eq!(
            CutOff::default().finish(answered("Whole.")),
            answered("Whole.")
        );
    }

    #[test]
    fn only_the_note_itself_reads_as_the_note() {
        assert!(is_note(&ChatMessage::user(CONTINUE_NOTE)));
        assert!(!is_note(&ChatMessage::user("please continue")));
        assert!(
            !is_note(&ChatMessage::assistant(CONTINUE_NOTE)),
            "the note is a user-role message"
        );
    }

    #[test]
    fn the_note_says_what_happened_and_what_to_do_in_one_paragraph() {
        // Model-facing prose is short and direct (CONTRIBUTING.md
        // *Conventions*): one bracketed line in the harness's own voice,
        // naming the drop, asking for the rest, and saying a call that was
        // cut off never ran.
        assert!(CONTINUE_NOTE.starts_with('[') && CONTINUE_NOTE.ends_with(']'));
        assert!(!CONTINUE_NOTE.contains('\n'));
        assert!(CONTINUE_NOTE.contains("cut off"), "{CONTINUE_NOTE}");
        assert!(CONTINUE_NOTE.contains("tool call"), "{CONTINUE_NOTE}");
    }
}
