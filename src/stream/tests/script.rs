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

#[test]
fn tokens_concatenate_back_to_the_original_text() {
    // The stress chunker is faithful like `chunks`: the pieces reproduce the
    // input byte for byte, whatever the boundaries fall on.
    let text = "The **quick** brown\n\n```py\nfox = 1\n```\n| a | b |\n";
    assert_eq!(tokens(text).concat(), text);
    assert!(tokens("").is_empty());
    assert_eq!(tokens("x").concat(), "x");
}

#[test]
fn tokens_are_sub_word_pieces_never_longer_than_a_token() {
    // A slow model streams *tokens*, not words: a few characters at a time,
    // split without regard to word or line boundaries (`docs/slow-stream.md`).
    // Every piece is at most `TOKEN_MAX_CHARS` long, so a long word breaks
    // across pieces — the split `chunks` (whole words) never produces.
    let text = "supercalifragilisticexpialidocious is one word and this is more prose";
    let pieces = tokens(text);
    assert!(
        pieces.iter().all(|p| p.chars().count() <= TOKEN_MAX_CHARS),
        "a piece longer than a token: {pieces:?}"
    );
    assert!(
        pieces.len() > chunks(text).len(),
        "tokens are finer than words: {} pieces vs {} words",
        pieces.len(),
        chunks(text).len()
    );
}

#[test]
fn tokens_split_markdown_markers_and_line_breaks_apart() {
    // What makes the split a stress: the boundaries land INSIDE the markers
    // the renderer keys on — an emphasis run split into `*` + `*bo`, a fence
    // opener into `` ` `` + ``` `` ```, a paragraph break riding one piece with
    // the next line's first characters — exactly the shapes a real
    // tokenizer's pieces take and the word split never produced.
    let text = "aa**bold** text\n\n```rust\nlet x = 1;\n```\n";
    let pieces = tokens(text);
    let split_emphasis = pieces
        .windows(2)
        .any(|w| w[0].ends_with('*') && w[1].starts_with('*'));
    let split_fence = pieces
        .windows(2)
        .any(|w| w[0].ends_with('`') && w[1].starts_with('`'));
    let newline_mid_piece = pieces
        .iter()
        .any(|p| p.contains('\n') && !p.ends_with('\n'));
    assert!(
        split_emphasis,
        "no piece boundary inside a `**` run: {pieces:?}"
    );
    assert!(
        split_fence,
        "no piece boundary inside a ``` run: {pieces:?}"
    );
    assert!(
        newline_mid_piece,
        "no piece carries a newline followed by more text: {pieces:?}"
    );
    // …and pieces never cut a character in half.
    for piece in &pieces {
        assert!(!piece.is_empty(), "an empty piece streams nothing");
    }
    assert_eq!(tokens("🎮世界e\u{301}").concat(), "🎮世界e\u{301}");
}

#[test]
fn tokens_are_deterministic() {
    // The smoke suite times a turn by its piece count, so the split must be
    // the same every run.
    let text = "some text to split the same way twice";
    assert_eq!(tokens(text), tokens(text));
}
