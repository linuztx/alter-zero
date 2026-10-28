//! Live tests for the unfinished-reply continuation
//! (`docs/unfinished-replies.md`) on Anthropic's Messages wire, through the
//! production pipeline: the provider's config resolved through
//! `ProvidersFile::model_config` as the boundary resolves it,
//! `LlmBackend::spawn`, the agent loop, the real wire. What no stub can show
//! is the provider's side of it: that it reports the output cap the way the
//! wire reads it, that it takes the request a continuation sends, and that
//! the model picks up where it stopped.
//!
//! All `#[ignore]`d so `cargo test` stays offline and deterministic. Run on
//! purpose with a real sign-in, never committed — read from the environment:
//!
//! ```sh
//! ANTHROPIC_CONSOLE_REFRESH_TOKEN=sk-ant-ort01-… ALTER_ZERO_LIVE_TOKEN_STORE=/tmp/live.env \
//!     cargo test --test live_unfinished -- --ignored --nocapture
//! ```
//!
//! The Console refresh token **rotates**, so these refuse to run without
//! `ALTER_ZERO_LIVE_TOKEN_STORE`, the `live_caching` rule: the mint may retire
//! the token it was given, and the rotated one is written back only to a
//! store path. After a run, the value under that file's
//! `ANTHROPIC_CONSOLE_REFRESH_TOKEN` key is the one to keep.
//! `ALTER_ZERO_LIVE_ANTHROPIC_MODEL` overrides the model.

use alter_zero::context::{ContextMessage, ContextRole};
use alter_zero::llm::agent::{CUT_OFF_LABEL, CUT_OFF_NOTE, EMPTY_REPLY_NOTE, MAX_CONTINUATIONS};
use alter_zero::llm::{LlmBackend, ProvidersFile, Selection};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};

/// Where the Console sign-in's refresh token lives.
const REFRESH_VAR: &str = "ANTHROPIC_CONSOLE_REFRESH_TOKEN";

/// The store a rotated refresh token is written back to. Required — see the
/// module docs.
fn token_store() -> std::path::PathBuf {
    std::env::var_os("ALTER_ZERO_LIVE_TOKEN_STORE")
        .map(std::path::PathBuf::from)
        .expect(
            "set ALTER_ZERO_LIVE_TOKEN_STORE to a file the rotated refresh token can be written to",
        )
}

/// Seed the store with the environment's token when it holds none yet, so the
/// write-back has a chain to replace.
fn seed_store() {
    let store = token_store();
    let text = std::fs::read_to_string(&store).unwrap_or_default();
    if !text
        .lines()
        .any(|line| line.starts_with(&format!("{REFRESH_VAR}=")))
    {
        let token = std::env::var(REFRESH_VAR)
            .unwrap_or_else(|_| panic!("set {REFRESH_VAR} to run this live test"));
        let mut seeded = text;
        if !seeded.is_empty() && !seeded.ends_with('\n') {
            seeded.push('\n');
        }
        seeded.push_str(&format!("{REFRESH_VAR}={token}\n"));
        std::fs::write(&store, seeded).expect("write the token store");
    }
}

/// The refresh token, read the way the boundary reads it after a rotation:
/// the store first, so a second run presents the rotated token rather than
/// the retired one.
fn credential() -> String {
    std::fs::read_to_string(token_store())
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix(&format!("{REFRESH_VAR}=")))
                .map(|value| value.trim().trim_matches('"').to_string())
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("set {REFRESH_VAR} to run this live test"))
}

/// A backend signed in to the Console, tools off and retries off (a refused
/// request should fail the test, not back off), `max_tokens` overridden to
/// `output_cap` when one is given.
fn console_backend(output_cap: Option<u64>) -> LlmBackend {
    seed_store();
    alter_zero::llm::claude::set_store_path(token_store());
    let model = std::env::var("ALTER_ZERO_LIVE_ANTHROPIC_MODEL")
        .unwrap_or_else(|_| "claude-haiku-4-5".to_string());
    let mut cfg = ProvidersFile::builtin()
        .model_config(&Selection {
            provider_id: "anthropic_console".to_string(),
            model,
            api_key: Some(credential()),
            temperature: None,
            thinking: None,
            vision: None,
            context: None,
            api_base: None,
            cache_key: None,
            service_tier: None,
        })
        .expect("anthropic_console is a built-in provider");
    if let Some(cap) = output_cap {
        cfg.extra_body
            .insert("max_tokens".to_string(), serde_json::json!(cap));
    }
    LlmBackend::configure(cfg, Some("You are a terse assistant.".to_string()), false)
        .with_max_retries(0)
}

/// Run one turn over `context`, collecting every event up to the terminal
/// one.
fn turn(backend: &LlmBackend, context: Vec<ContextMessage>) -> Vec<StreamEvent> {
    let prompt = context.last().map(|m| m.text.clone()).unwrap_or_default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt, vec![], context, tx, CancelToken::new());
    let mut events = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        let terminal = matches!(event, StreamEvent::StreamDone | StreamEvent::Error(_));
        events.push(event);
        if terminal {
            break;
        }
    }
    handle.join().expect("the backend thread joins");
    events
}

/// The text each round streamed, split where a continuation prompt went out.
fn rounds(events: &[StreamEvent]) -> Vec<String> {
    let mut rounds = vec![String::new()];
    for event in events {
        match event {
            StreamEvent::Chunk(text) => rounds.last_mut().expect("a round").push_str(text),
            StreamEvent::HookNote { .. } => rounds.push(String::new()),
            _ => {}
        }
    }
    rounds
}

/// Every continuation prompt the turn sent, as `(label, text)`.
fn notes(events: &[StreamEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::HookNote { label, text } => Some((label.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

#[test]
#[ignore = "hits the network; needs ANTHROPIC_CONSOLE_REFRESH_TOKEN + ALTER_ZERO_LIVE_TOKEN_STORE; costs well under a cent"]
fn live_anthropic_console_continues_a_reply_cut_off_at_the_output_cap() {
    // A cap far under the reply's length: the first round stops at it with
    // `stop_reason: "max_tokens"`, the loop keeps what it said and asks for
    // the rest, and the model finishes the count inside the same turn.
    let backend = console_backend(Some(40));
    let events = turn(
        &backend,
        vec![ContextMessage::new(
            ContextRole::User,
            "Count from 1 to 30, one number per line, then write DONE on its own line. \
             Nothing else.",
        )],
    );
    let rounds = rounds(&events);
    for (i, text) in rounds.iter().enumerate() {
        println!("round {}: {text:?}", i + 1);
    }
    println!("terminal event: {:?}", events.last());

    let notes = notes(&events);
    assert!(
        !notes.is_empty(),
        "the capped round was answered with a continuation: {events:?}"
    );
    assert!(notes.len() <= MAX_CONTINUATIONS, "{notes:?}");
    assert!(
        notes
            .iter()
            .all(|(label, text)| label == CUT_OFF_LABEL && text == CUT_OFF_NOTE),
        "every continuation names the cap: {notes:?}"
    );
    assert!(
        matches!(events.last(), Some(StreamEvent::StreamDone)),
        "the provider took the continuation and the model finished: {events:?}"
    );
    assert!(
        rounds.iter().skip(1).any(|text| !text.trim().is_empty()),
        "a continuation round streamed a reply: {rounds:?}"
    );
    assert!(
        rounds.concat().contains("DONE"),
        "the count reached its end: {rounds:?}"
    );
}

#[test]
#[ignore = "hits the network; needs ANTHROPIC_CONSOLE_REFRESH_TOKEN + ALTER_ZERO_LIVE_TOKEN_STORE; costs well under a cent"]
fn live_anthropic_console_answers_the_request_an_empty_reply_is_continued_with() {
    // An empty reply is never sent back, so the request after one is the
    // question and the prompt side by side — two text blocks in one user
    // message on this wire. No model can be made to reply with nothing on
    // cue, so this sends that request directly: the API takes it, and the
    // model answers the question rather than replying with nothing again.
    let backend = console_backend(None);
    let events = turn(
        &backend,
        vec![
            ContextMessage::new(
                ContextRole::User,
                "What is 6 times 7? Reply with just the number.",
            ),
            ContextMessage::new(ContextRole::User, EMPTY_REPLY_NOTE),
        ],
    );
    let rounds = rounds(&events);
    println!("reply: {rounds:?}");
    println!("terminal event: {:?}", events.last());

    assert!(
        matches!(events.last(), Some(StreamEvent::StreamDone)),
        "the provider took the request: {events:?}"
    );
    assert!(
        notes(&events).is_empty(),
        "answered at once, nothing left to continue: {events:?}"
    );
    assert!(rounds.concat().contains("42"), "{rounds:?}");
}
