//! Live Ollama integration tests (`docs/ollama.md`) — the full production
//! pipeline against a real server: the `/api/tags` + `/api/show` catalog,
//! `LlmBackend::spawn` → the native `/api/chat` NDJSON stream →
//! `StreamEvent`s, with thinking, a tool round, an image, the window the
//! request asks for, and the errors it explains.
//!
//! All `#[ignore]`d so `cargo test` stays offline and deterministic. Run
//! explicitly against a server holding the three small models they use:
//!
//! ```sh
//! ollama pull qwen3:1.7b && ollama pull gemma3:270m && ollama pull moondream
//! cargo test --test live_ollama -- --ignored --nocapture
//! ```
//!
//! `OLLAMA_HOST` points them at another server (Ollama's own grammar, e.g.
//! `myhost:11434`); `ALTER_ZERO_LIVE_OLLAMA_MODEL` swaps the tool-capable
//! thinking model (default `qwen3:1.7b` — the 0.6b one thinks and chats but
//! cannot form a call against the real tool schemas, which is the model's
//! limit rather than the wire's).

use std::path::PathBuf;

use alter_zero::context::{ContextMessage, ContextRole};
use alter_zero::llm::models::fetch_models;
use alter_zero::llm::{
    LlmBackend, ModelConfig, ModelEntry, ProvidersFile, ReasoningEffort, Selection, ThinkingMode,
};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent, TokenUsage};

/// The tool-capable thinking model under test.
fn thinking_model() -> String {
    std::env::var("ALTER_ZERO_LIVE_OLLAMA_MODEL").unwrap_or_else(|_| "qwen3:1.7b".to_string())
}

/// The resolved config for `model` on the built-in `ollama` provider, the
/// host taken from `OLLAMA_HOST` exactly as the boundary takes it.
fn config(model: &str, thinking: Option<ThinkingMode>, context: Option<u64>) -> ModelConfig {
    ProvidersFile::builtin()
        .model_config(&Selection {
            provider_id: "ollama".to_string(),
            model: model.to_string(),
            api_key: std::env::var("OLLAMA_API_KEY").ok(),
            temperature: Some(0.0),
            thinking,
            vision: None,
            context,
            api_base: std::env::var("OLLAMA_HOST").ok(),
            cache_key: None,
        })
        .expect("ollama is a built-in provider")
}

/// A tools-on backend for `model`.
fn backend(model: &str, thinking: Option<ThinkingMode>, context: Option<u64>) -> LlmBackend {
    backend_with_tools(model, thinking, context, true)
}

/// A backend for `model`, tools offered or not. The chat-only tests keep
/// them off: a small local model handed four real tool schemas can answer a
/// plain question with a broken tool call and no text at all, which is the
/// model's limit rather than what those tests are about.
fn backend_with_tools(
    model: &str,
    thinking: Option<ThinkingMode>,
    context: Option<u64>,
    tools: bool,
) -> LlmBackend {
    LlmBackend::configure(
        config(model, thinking, context),
        Some("You are a terse assistant. Answer in as few words as possible.".to_string()),
        tools,
    )
}

/// Everything one turn streamed, in order.
fn run(backend: &LlmBackend, prompt: &str, images: Vec<PathBuf>) -> Vec<StreamEvent> {
    let mut context = vec![ContextMessage::new(ContextRole::User, prompt)];
    context[0].images = images.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), images, context, tx, CancelToken::new());
    let mut events = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        let done = matches!(event, StreamEvent::StreamDone | StreamEvent::Error(_));
        if let StreamEvent::Retrying { attempt, max } = &event {
            println!("retrying {attempt}/{max}…");
        }
        events.push(event);
        if done {
            break;
        }
    }
    handle.join().expect("backend thread joins");
    events
}

/// The reply text out of a turn's events; panics on a backend error.
fn reply_text(events: &[StreamEvent]) -> String {
    let mut text = String::new();
    for event in events {
        match event {
            StreamEvent::Chunk(chunk) => text.push_str(chunk),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            _ => {}
        }
    }
    text
}

fn usages(events: &[StreamEvent]) -> Vec<TokenUsage> {
    events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::Usage(usage) => Some(*usage),
            _ => None,
        })
        .collect()
}

/// A 64×64 solid red PNG, written to the temp dir.
fn red_png() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "alter-zero-live-ollama-red-{}.png",
        std::process::id()
    ));
    let mut image = image::RgbImage::new(64, 64);
    for pixel in image.pixels_mut() {
        *pixel = image::Rgb([0xff, 0x00, 0x00]);
    }
    image.save(&path).expect("write the png");
    path
}

#[test]
#[ignore = "needs a running Ollama with qwen3:1.7b, gemma3:270m and moondream pulled"]
fn live_catalog_carries_capabilities_and_windows() {
    // The three capability questions answered off `/api/tags` + `/api/show`
    // (docs/ollama.md): vision and thinking off the capabilities list, the
    // window off the model's own maximum under the cap.
    let cfg = config(&thinking_model(), None, None);
    let models = fetch_models(&cfg, &CancelToken::new()).expect("the catalog");
    for model in &models {
        println!(
            "{:<24} vision={:?} reasoning={:?} context={:?}",
            model.id, model.vision, model.reasoning, model.context
        );
    }
    let find = |id: &str| -> Option<&ModelEntry> { models.iter().find(|m| m.id == id) };
    if let Some(qwen) = find(&thinking_model()) {
        assert_eq!(qwen.vision, Some(false));
        let support = qwen.reasoning.clone().expect("qwen3 thinks");
        assert!(support.efforts.is_empty(), "an on/off reasoner");
        assert!(support.can_disable);
        assert_eq!(
            qwen.context,
            Some(alter_zero::llm::ollama::DEFAULT_NUM_CTX_CAP),
            "40960 capped"
        );
    }
    if let Some(moondream) = find("moondream:latest") {
        assert_eq!(moondream.vision, Some(true));
        assert_eq!(moondream.reasoning, None);
        assert_eq!(moondream.context, Some(2048), "under the cap: its own");
    }
    if let Some(gemma) = find("gemma3:270m") {
        assert_eq!(gemma.vision, Some(false));
        assert_eq!(gemma.reasoning, None);
        assert_eq!(gemma.context, Some(32_768));
    }
    assert!(
        models.iter().all(|m| m.provider == "ollama"),
        "every row is tagged with the provider"
    );
}

#[test]
#[ignore = "needs a running Ollama with qwen3:1.7b pulled"]
fn live_thinking_streams_as_its_own_phase_and_usage_lands() {
    let backend = backend_with_tools(&thinking_model(), Some(ThinkingMode::On), Some(8192), false);
    let events = run(&backend, "Say hi in two words.", vec![]);
    let text = reply_text(&events);
    println!("reply: {text:?}");
    assert!(!text.trim().is_empty());
    let thinking: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ThinkingChunk(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    println!("thought: {} chars", thinking.len());
    assert!(!thinking.is_empty(), "qwen3 thinks by default");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ThinkingStart))
    );
    assert!(events.iter().any(|e| matches!(e, StreamEvent::ThinkingEnd)));
    let usage = usages(&events);
    assert_eq!(usage.len(), 1, "one round, one usage frame: {usage:?}");
    assert!(usage[0].input > 0 && usage[0].output > 0, "{usage:?}");
}

#[test]
#[ignore = "needs a running Ollama with qwen3:1.7b pulled"]
fn live_thinking_off_sends_no_thinking() {
    let backend = backend_with_tools(
        &thinking_model(),
        Some(ThinkingMode::Off),
        Some(8192),
        false,
    );
    let events = run(&backend, "Say hi in two words.", vec![]);
    assert!(!reply_text(&events).trim().is_empty());
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, StreamEvent::ThinkingChunk(_))),
        "think:false silences the trace"
    );
}

#[test]
#[ignore = "needs a running Ollama with qwen3:1.7b pulled"]
fn live_tool_round_trip() {
    // The whole agentic loop over the native wire: the model asks for
    // `bash`, the call lands whole with object arguments, the executor runs
    // it, the result goes back as a `tool` message naming its tool, and the
    // model answers from it. Thinking stays on: a small qwen3 forms its call
    // reliably only after reasoning about it.
    let backend = backend(&thinking_model(), Some(ThinkingMode::On), Some(8192));
    let events = run(
        &backend,
        "Run the shell command `echo alter-zero-live-ok` with the bash tool, then reply \
         with exactly what it printed.",
        vec![],
    );
    let text = reply_text(&events);
    println!("reply: {text:?}");
    let started: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolStart { name, args, .. } => Some(format!("{name}({args})")),
            _ => None,
        })
        .collect();
    println!("tool calls: {started:?}");
    assert!(
        started
            .iter()
            .any(|call| call.to_ascii_lowercase().starts_with("bash(")),
        "the model called bash: {started:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolEnd { ok: true, .. })),
        "the call ran and succeeded"
    );
    assert!(
        usages(&events).len() >= 2,
        "a tool round and an answer round each report usage"
    );
}

#[test]
#[ignore = "needs a running Ollama with moondream pulled"]
fn live_image_round_on_a_vision_model() {
    // moondream has no `tools` capability, so the backend offers none — a
    // tools-on request is refused outright (`live_refusals_are_explained`).
    let backend = LlmBackend::configure(config("moondream", None, None), None, false);
    let png = red_png();
    // moondream is prompt-sensitive to the point of answering some wordings
    // with an immediate end-of-turn; this one it answers.
    let events = run(&backend, "What color is this image?", vec![png.clone()]);
    std::fs::remove_file(&png).ok();
    let text = reply_text(&events);
    println!("reply: {text:?}");
    assert!(text.to_ascii_lowercase().contains("red"), "{text:?}");
}

#[test]
#[ignore = "needs a running Ollama with gemma3:270m pulled"]
fn live_num_ctx_is_what_the_server_loads() {
    // The reason the native wire exists: the window the session gauges
    // against is the window the server holds (docs/ollama.md). `/api/ps`
    // reports the loaded model's context length.
    let cfg = config("gemma3:270m", None, Some(6144));
    let backend = LlmBackend::configure(cfg.clone(), None, false);
    let text = reply_text(&run(&backend, "Say hi.", vec![]));
    assert!(!text.trim().is_empty());
    let ps: serde_json::Value = reqwest::blocking::get(format!("{}/api/ps", cfg.api_base))
        .expect("ps")
        .json()
        .expect("json");
    let loaded = ps["models"]
        .as_array()
        .expect("models")
        .iter()
        .find(|m| m["model"] == "gemma3:270m")
        .expect("gemma3 is loaded");
    assert_eq!(loaded["context_length"], 6144, "{loaded}");
}

#[test]
#[ignore = "needs a running Ollama with gemma3:270m pulled"]
fn live_refusals_are_explained() {
    // A model that isn't there, and a capability the model lacks — both in
    // words that say what to do (docs/ollama.md).
    let missing = backend("definitely-not-a-model:latest", None, None);
    let events = run(&missing, "hi", vec![]);
    let Some(StreamEvent::Error(reason)) = events.last() else {
        panic!("a missing model fails the turn: {events:?}");
    };
    println!("missing: {reason}");
    assert!(
        reason.contains("ollama pull definitely-not-a-model:latest"),
        "{reason}"
    );

    // gemma3:270m has no `tools` capability: a tools-on request is refused,
    // and the refusal names the /settings row that fixes it.
    let no_tools = backend("gemma3:270m", None, Some(4096));
    let events = run(&no_tools, "hi", vec![]);
    let Some(StreamEvent::Error(reason)) = events.last() else {
        panic!("a tools request to a no-tools model fails the turn: {events:?}");
    };
    println!("no tools: {reason}");
    assert!(reason.contains("/settings"), "{reason}");
}

#[test]
#[ignore = "needs a running Ollama with qwen3:1.7b pulled"]
fn live_an_effort_level_is_accepted_by_a_switched_reasoner() {
    // A level string on a model that takes only a switch is treated as
    // "on" by the server rather than refused — so a stale persisted mode
    // cannot fail every turn.
    let backend = backend_with_tools(
        &thinking_model(),
        Some(ThinkingMode::Effort(ReasoningEffort::High)),
        Some(8192),
        false,
    );
    let events = run(&backend, "Say hi in two words.", vec![]);
    assert!(!reply_text(&events).trim().is_empty());
}
