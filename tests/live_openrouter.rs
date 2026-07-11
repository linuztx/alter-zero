//! Live OpenRouter integration tests for the conversation context and the
//! vision image path (`docs/context.md`) — the full production pipeline:
//! `LlmBackend::spawn` → context assembly (+ `data:`-URL image encoding on
//! the backend thread) → the blocking SSE stream → `StreamEvent`s.
//!
//! All `#[ignore]`d so `cargo test` stays offline and deterministic. Run
//! explicitly with a real key (never committed — read from the environment):
//!
//! ```sh
//! OPENROUTER_API_KEY=sk-or-… cargo test --test live_openrouter -- --ignored --nocapture
//! ```
//!
//! `INLINE_TUI_LIVE_MODEL` overrides the model (default `openai/gpt-4o-mini`,
//! which is cheap and supports vision).

use std::path::PathBuf;

use inline_tui::context::{ContextMessage, ContextRole, ContextToolCall};
use inline_tui::llm::{LlmBackend, ModelConfig};
use inline_tui::stream::{CancelToken, ReplySource, StreamEvent};

/// The backend under test, configured for OpenRouter from the environment.
/// Panics with a clear message when the key is missing — these tests are only
/// ever run on purpose (`--ignored`).
fn backend() -> LlmBackend {
    let key =
        std::env::var("OPENROUTER_API_KEY").expect("set OPENROUTER_API_KEY to run the live tests");
    let model =
        std::env::var("INLINE_TUI_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    let cfg = ModelConfig {
        provider_id: "openrouter".to_string(),
        provider_name: "OpenRouter".to_string(),
        model,
        api_base: "https://openrouter.ai/api/v1".to_string(),
        api_model_base: "https://openrouter.ai/api/v1".to_string(),
        api_key: Some(key),
        temperature: Some(0.0),
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    };
    LlmBackend::with_system_prompt(
        cfg,
        Some("You are a terse assistant. Answer in as few words as possible.".to_string()),
    )
}

/// Run one turn through the real `ReplySource::spawn` seam and collect the
/// streamed reply text. Panics on a backend error.
fn complete(prompt: &str, images: Vec<PathBuf>, context: Vec<ContextMessage>) -> String {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend().spawn(prompt.to_string(), images, context, tx, CancelToken::new());
    let mut text = String::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(chunk) => text.push_str(&chunk),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    text
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_multi_turn_context_is_remembered() {
    // A fact stated in turn 1 must be recalled in turn 2 — only possible if
    // the whole context rides the second request.
    let context = vec![
        ContextMessage::new(
            ContextRole::User,
            "My name is Zebulon-Quartz-47. Please remember it.",
        ),
        ContextMessage::new(ContextRole::Assistant, "Understood, Zebulon-Quartz-47."),
        ContextMessage::new(
            ContextRole::User,
            "What is my name? Reply with just the name.",
        ),
    ];
    let reply = complete(
        "What is my name? Reply with just the name.",
        vec![],
        context,
    );
    println!("model replied: {reply:?}");
    assert!(
        reply.to_lowercase().contains("zebulon"),
        "the model should recall the name from the context, got: {reply:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_raw_tool_records_are_usable_context() {
    // The native tool round-trip must read as context: a replayed assistant
    // `tool_calls` request + its `tool`-role result (docs/context.md), and the
    // model answers a question whose answer only exists inside that result.
    let context = vec![
        ContextMessage::new(ContextRole::User, "Read the config file."),
        ContextMessage::assistant_tool_calls(
            "",
            vec![ContextToolCall::new(
                "call_0",
                "read",
                r#"{"path":"config.toml"}"#,
            )],
        ),
        ContextMessage::tool_result("call_0", "port = 4821\nhost = \"example.test\""),
        ContextMessage::new(
            ContextRole::User,
            "According to the tool output above, what port is configured? Reply with just the number.",
        ),
    ];
    let reply = complete("what port?", vec![], context);
    println!("model replied: {reply:?}");
    assert!(
        reply.contains("4821"),
        "the model should read the port out of the native tool result, got: {reply:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_tool_call_generation_emits_delta_events() {
    // While the model *generates* a tool call, the backend surfaces
    // ToolCallDelta events (the streamed name/argument fragments) so the live
    // status token tally ticks — like reasoning does (docs/status-indicator.md).
    // Assert at least one arrives, ahead of the ToolStart that runs the call.
    let context = vec![ContextMessage::new(
        ContextRole::User,
        "Run the bash command `echo hi` using your bash tool.",
    )];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend().spawn(
        "echo hi".to_string(),
        vec![],
        context,
        tx,
        CancelToken::new(),
    );
    let mut tool_call_deltas = 0;
    let mut saw_tool_start = false;
    let mut generation_before_start = false;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::ToolCallDelta(frag) => {
                assert!(!frag.is_empty(), "a generation fragment is non-empty");
                if !saw_tool_start {
                    generation_before_start = true;
                }
                tool_call_deltas += 1;
            }
            StreamEvent::ToolStart { .. } => saw_tool_start = true,
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("tool-call-generation deltas: {tool_call_deltas}");
    assert!(saw_tool_start, "the model ran a tool");
    assert!(
        tool_call_deltas >= 1,
        "tool-call generation surfaced at least one delta to count"
    );
    assert!(
        generation_before_start,
        "the generation fragments precede the ToolStart"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_vision_reads_a_pasted_image() {
    // A solid-red PNG written the way the clipboard paste writes one; the
    // context message carries its path like a real [Image #1] attachment.
    let path = std::env::temp_dir().join("inline-tui-live-vision-red.png");
    let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([220, 20, 20, 255]));
    img.save(&path).expect("write the test PNG");

    let context = vec![ContextMessage {
        role: ContextRole::User,
        text: "[Image #1] What is the single dominant color of this image? \
               Answer with one lowercase word."
            .to_string(),
        images: vec![path.clone()],
        tool_calls: vec![],
        tool_call_id: None,
    }];
    let reply = complete("what color?", vec![path.clone()], context);
    std::fs::remove_file(&path).ok();
    println!("model replied: {reply:?}");
    assert!(
        reply.to_lowercase().contains("red"),
        "the model should see the red image, got: {reply:?}"
    );
}
