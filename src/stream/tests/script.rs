//! The dummy's canned replies and streaming primitives.

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
