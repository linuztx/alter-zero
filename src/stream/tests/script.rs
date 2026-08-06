//! The dummy's canned replies and streaming primitives.

use super::super::dummy::script::{HANDOFF, reply_parts};
use super::*;

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

/// Prompts of every length class, so the walk covers the whole rotation
/// (`dummy_response` picks by `chars().count() % DEMO_REPLIES.len()`).
const EVERY_REPLY: &[&str] = &["", "a", "ab", "abc", "abcd", "abcde"];

#[test]
fn every_demo_reply_points_the_user_at_login_and_model() {
    // The dummy is what a first run meets, and its whole job is to hand the
    // user off to a real model: every canned reply names `/login` (save a
    // provider key) and `/model` (pick one). See `docs/dummy-backend.md`.
    for prompt in EVERY_REPLY {
        let reply = dummy_response(prompt);
        assert!(
            reply.contains("/login"),
            "the reply for {prompt:?} never mentions /login: {reply}"
        );
        assert!(
            reply.contains("/model"),
            "the reply for {prompt:?} never mentions /model: {reply}"
        );
    }
}

#[test]
fn every_demo_reply_splits_into_two_streamable_parts() {
    // A tool turn streams the reply in two parts — before its work and after
    // it — and `flush_streaming_segment` makes each part its own history
    // message. So the split is at a **blank line**, never mid-paragraph: a
    // list or a fenced block cut in half would render as two broken blocks.
    // The separator rides the first part, so the two still concatenate to the
    // whole reply (what `turn_events`' chunks must reproduce).
    for prompt in EVERY_REPLY {
        let reply = dummy_response(prompt);
        let (opening, closing) = reply_parts(&reply);
        assert!(!opening.trim().is_empty(), "{prompt:?} has no opening");
        assert!(!closing.trim().is_empty(), "{prompt:?} has no closing");
        assert_eq!(
            format!("{opening}{closing}"),
            reply,
            "the parts of {prompt:?} don't reconstruct the reply"
        );
        assert!(
            opening.ends_with("\n\n"),
            "{prompt:?} splits mid-paragraph: {opening:?}"
        );
    }
}

#[test]
fn the_handoff_paragraph_closes_every_demo_reply() {
    // The `/login` → `/model` hand-off is the *last* thing said, so it is
    // what stays on screen when the turn settles (and what `/copy` yields).
    // It is one shared sentence, verbatim, in every reply — which is what
    // lets `scripts/smoke.sh` settle on "Two commands away" whatever the
    // prompt selected.
    assert!(HANDOFF.contains("/login") && HANDOFF.contains("/model"));
    for prompt in EVERY_REPLY {
        let reply = dummy_response(prompt);
        assert!(
            reply.ends_with(HANDOFF),
            "the reply for {prompt:?} doesn't close on the hand-off: {reply}"
        );
    }
    // The table demo is a reply too, and just as likely to be a first turn.
    assert!(dummy_response("show me a table").ends_with(HANDOFF));
}
