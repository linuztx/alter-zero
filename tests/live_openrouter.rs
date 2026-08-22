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
//! `ALTER_ZERO_LIVE_MODEL` overrides the model (default `openai/gpt-4o-mini`,
//! which is cheap and supports vision). The prompt-caching / usage tests
//! (`docs/prompt-caching.md`) live here too; the Venice ones need
//! `A0_VENICE_API_KEY` (and `ALTER_ZERO_LIVE_VENICE_MODEL` to override that
//! provider's churning model ids).

use std::path::PathBuf;

use alter_zero::app::{HistoryItem, ToolCall, ToolStatus};
use alter_zero::context::{ContextMessage, ContextRole, ContextToolCall, context_messages};
use alter_zero::llm::{LlmBackend, ModelConfig, ThinkingMode};
use alter_zero::permission::{PermissionKind, PermissionRequest, denial_result, denied_display};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};

/// A backend configured for OpenRouter from the environment, for `model` with
/// the given thinking mode and known image-input support. Panics with a clear
/// message when the key is missing — these tests are only ever run on purpose
/// (`--ignored`).
fn backend_with(model: String, thinking: Option<ThinkingMode>, vision: Option<bool>) -> LlmBackend {
    let key =
        std::env::var("OPENROUTER_API_KEY").expect("set OPENROUTER_API_KEY to run the live tests");
    let cfg = ModelConfig {
        provider_id: "openrouter".to_string(),
        provider_name: "OpenRouter".to_string(),
        model,
        api_base: "https://openrouter.ai/api/v1".to_string(),
        api_model_base: "https://openrouter.ai/api/v1".to_string(),
        api_key: Some(key),
        temperature: Some(0.0),
        thinking,
        vision,
        cache_key: None,
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    };
    LlmBackend::with_system_prompt(
        cfg,
        Some("You are a terse assistant. Answer in as few words as possible.".to_string()),
    )
}

/// [`backend_with`] with unknown vision — the pre-detection shape.
fn backend_for(model: String, thinking: Option<ThinkingMode>) -> LlmBackend {
    backend_with(model, thinking, None)
}

/// The default backend under test (`ALTER_ZERO_LIVE_MODEL`, else a cheap
/// vision-capable model), no thinking mode.
fn backend() -> LlmBackend {
    let model =
        std::env::var("ALTER_ZERO_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    backend_for(model, None)
}

/// The plain text of a rendered line.
fn plain(line: &ratatui::text::Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
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
            // Surfaced so a live run shows a request silently burning its
            // retry budget (the old 3 s send-timeout bug looked exactly like
            // this before failing outright).
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
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
fn live_usage_frame_reaches_the_app() {
    // The real-usage pipeline end to end (docs/prompt-caching.md): the
    // payload asks for `stream_options.include_usage`, the provider's final
    // frame is parsed, and the backend forwards it as StreamEvent::Usage —
    // the numbers the app snaps its tally to instead of the tiktoken
    // estimate.
    let context = vec![ContextMessage::new(
        ContextRole::User,
        "Say OK and nothing else.",
    )];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend().spawn(
        "say ok".to_string(),
        vec![],
        context,
        tx,
        CancelToken::new(),
    );
    let mut usages = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Usage(u) => usages.push(u),
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("usage frames: {usages:?}");
    assert!(!usages.is_empty(), "the round reported real usage");
    let total: u64 = usages
        .iter()
        .map(alter_zero::stream::TokenUsage::total)
        .sum();
    assert!(
        total > 100,
        "the report counts the whole request (system prompt included), got {total}"
    );
}

/// Run one plain turn through `backend`, collecting the reply text and every
/// usage frame. Panics on a backend error.
fn complete_with_usage(
    backend: &LlmBackend,
    prompt: &str,
) -> (String, Vec<alter_zero::stream::TokenUsage>) {
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let mut reply = String::new();
    let mut usages = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(c) => reply.push_str(&c),
            StreamEvent::Usage(u) => usages.push(u),
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    (reply, usages)
}

/// A deterministic ~5k-token system prompt, unique per `salt` so a rerun
/// can't hit a previous run's still-warm cache.
fn big_system_prompt(salt: u64) -> String {
    let corpus: String = (0..420)
        .map(|i| format!("Calibration sentence {i} of the standing corpus, run {salt}. "))
        .collect();
    format!("You are a terse assistant. Answer in as few words as possible.\n{corpus}")
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY; costs a few cents"]
fn live_anthropic_prompt_cache_writes_then_reads() {
    // The whole explicit-caching feature end to end (docs/prompt-caching.md):
    // an anthropic/ model gets cache_control breakpoints on the system prompt
    // and the conversation frontier, plus the session_id pin — so turn 1
    // WRITES the prefix to the provider's cache and turn 2 READS it back at
    // ~1/10th the input price. The usage frames prove both.
    let key =
        std::env::var("OPENROUTER_API_KEY").expect("set OPENROUTER_API_KEY to run the live tests");
    let salt = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut cfg = ModelConfig {
        provider_id: "openrouter".to_string(),
        provider_name: "OpenRouter".to_string(),
        model: "anthropic/claude-haiku-4.5".to_string(),
        api_base: "https://openrouter.ai/api/v1".to_string(),
        api_model_base: "https://openrouter.ai/api/v1".to_string(),
        api_key: Some(key),
        temperature: Some(0.0),
        thinking: None,
        vision: None,
        cache_key: Some(format!("alter-zero-live-cache-{salt}")),
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    };
    cfg.temperature = Some(0.0);
    // Tools off: a deterministic single round per turn, so each turn is
    // exactly one usage frame.
    let backend = LlmBackend::configure(cfg, Some(big_system_prompt(salt)), false);

    let (reply, usages) = complete_with_usage(&backend, "Just say ONE.");
    println!("turn 1 reply: {reply:?}, usage: {usages:?}");
    let first = usages.first().expect("turn 1 reported usage");
    assert!(
        first.cache_write > 1_000,
        "turn 1 wrote the big prefix to the cache: {first:?}"
    );

    let (reply, usages) = complete_with_usage(&backend, "Just say ONE.");
    println!("turn 2 reply: {reply:?}, usage: {usages:?}");
    let second = usages.first().expect("turn 2 reported usage");
    assert!(
        second.cached > 1_000,
        "turn 2 read the prefix back from the cache: {second:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_qwen_accepts_cache_breakpoints_on_a_tool_round() {
    // qwen/ ids are in the explicit-caching set (needs_cache_breakpoints), so
    // a request whose frontier breakpoint lands on a `tool`-role message must
    // not be rejected by the upstream provider — the risky shape, verified
    // live. The turn just has to complete.
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let backend = backend_for("qwen/qwen3.6-flash".to_string(), None);
    let handle = backend.spawn(
        "what port?".to_string(),
        vec![],
        context,
        tx,
        CancelToken::new(),
    );
    let mut reply = String::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(c) => reply.push_str(&c),
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("qwen rejected the breakpointed request: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("model replied: {reply:?}");
    assert!(reply.contains("4821"), "the turn completed: {reply:?}");
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_venice_reports_usage_and_hits_its_cache() {
    // The Venice/Agent-Zero pipeline (docs/prompt-caching.md): Venice caches
    // implicitly — no breakpoints — but honours `prompt_cache_key` and
    // reports usage with the cached detail. Turn 2 of an identical prefix
    // must show cached tokens.
    let key = std::env::var("A0_VENICE_API_KEY")
        .expect("set A0_VENICE_API_KEY to run the Venice live tests");
    let model = std::env::var("ALTER_ZERO_LIVE_VENICE_MODEL")
        .unwrap_or_else(|_| "openai-gpt-4o-mini-2024-07-18".to_string());
    let providers = alter_zero::llm::ProvidersFile::builtin();
    let salt = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let sel = alter_zero::llm::Selection {
        provider_id: "a0_venice".to_string(),
        model,
        api_key: Some(key),
        temperature: Some(0.0),
        thinking: None,
        vision: None,
        cache_key: Some(format!("alter-zero-live-venice-{salt}")),
    };
    let cfg = providers.model_config(&sel).expect("a0_venice is built in");
    let backend = LlmBackend::configure(cfg, Some(big_system_prompt(salt)), false);

    let (reply, usages) = complete_with_usage(&backend, "Just say ONE.");
    println!("turn 1 reply: {reply:?}, usage: {usages:?}");
    let first = usages.first().expect("turn 1 reported usage");
    assert!(first.total() > 1_000, "the whole prefix billed: {first:?}");

    let (reply, usages) = complete_with_usage(&backend, "Just say ONE.");
    println!("turn 2 reply: {reply:?}, usage: {usages:?}");
    let second = usages.first().expect("turn 2 reported usage");
    assert!(
        second.cached > 500,
        "turn 2 hit Venice's implicit prompt cache: {second:?}"
    );
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
fn live_amended_rejection_still_steers_the_model_a_turn_later() {
    // The Tab-amend fix (docs/permissions.md), against a real provider: a
    // rejected call replays with the **model-facing** denial — the user's
    // typed instructions included — so a LATER turn still follows them. Before
    // the fix history kept only `User rejected write to config.py`, and a real
    // model asked to proceed would happily re-propose the rejected approach.
    //
    // The derivation under test is `context::context_messages`, run over a
    // history item shaped exactly as `App::reject_tool` records one — both
    // texts built by the same pure functions the gate uses, so this fixture
    // cannot drift from production. (The *discriminating* check that the
    // derivation picks `context_output` over `output` is the offline unit test
    // `app::tests::permission::the_derived_context_replays_the_amended_…`;
    // what a live provider adds is that a real model reads this shape and acts
    // on the instruction a turn later.)
    let feedback = "use the sentinel port 4821";
    let request = PermissionRequest {
        id: String::new(),
        kind: PermissionKind::Write,
        target: "config.py".to_string(),
        body: String::new(),
        detail: None,
        agent: None,
    };
    let history = vec![HistoryItem::Tool(ToolCall {
        name: "Write".to_string(),
        args: "config.py".to_string(),
        status: ToolStatus::Failed,
        // What the cell shows…
        output: denied_display(&request, Some(feedback)),
        timestamp: String::new(),
        shell: false,
        truncated: false,
        // …and what the model was actually told.
        context_output: Some(denial_result(Some(feedback))),
        approval_note: None,
        batch: None,
    })];
    let mut context = vec![ContextMessage::new(
        ContextRole::User,
        "Write config.py with the server port in it.",
    )];
    context.extend(context_messages(&history));
    let question = "What port did I tell you to use? Reply with just the number.";
    context.push(ContextMessage::new(ContextRole::User, question));

    let reply = complete(question, vec![], context);
    println!("model replied: {reply:?}");
    assert!(
        reply.contains("4821"),
        "the amended instructions must survive into a later turn's context, got: {reply:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_environment_context_reaches_the_model() {
    // The context-awareness feature end to end (docs/environment.md): the
    // boundary folds date/os/cwd into the system prompt via
    // `augment_with_environment`; a real model must be able to read the cwd
    // back out of that block. Proves the block rides the request and is
    // legible to the model. Tools off so the model answers from the prompt
    // instead of shelling out for the path.
    use alter_zero::llm::backend::augment_with_environment;
    let key =
        std::env::var("OPENROUTER_API_KEY").expect("set OPENROUTER_API_KEY to run the live tests");
    let model =
        std::env::var("ALTER_ZERO_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    let cwd = "/home/user/alter-zero-sentinel-42";
    // The os string is the distro-enriched form the boundary builds on Linux
    // (docs/environment.md) — assert the model can read the distro back too.
    let os = "linux (Ubuntu 24.04.4 LTS)";
    let system = augment_with_environment(
        "You are Alter Zero an autonomous AI agent running in terminal UI",
        "Sunday 2026-07-19",
        os,
        cwd,
    );
    let cfg = ModelConfig {
        provider_id: "openrouter".to_string(),
        provider_name: "OpenRouter".to_string(),
        model,
        api_base: "https://openrouter.ai/api/v1".to_string(),
        api_model_base: "https://openrouter.ai/api/v1".to_string(),
        api_key: Some(key),
        temperature: Some(0.0),
        thinking: None,
        vision: None,
        cache_key: None,
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    };
    let backend = LlmBackend::configure(cfg, Some(system), false);

    let prompt = "Report your operating system and your current working directory, \
                  exactly as given in your environment context.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let mut reply = String::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(c) => reply.push_str(&c),
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("model replied: {reply:?}");
    assert!(
        reply.contains(cwd),
        "the model read the cwd out of the environment context, got: {reply:?}"
    );
    assert!(
        reply.contains("Ubuntu 24.04.4 LTS"),
        "the model read the distro-enriched OS out of the environment context, got: {reply:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_agents_md_instructions_reach_the_model() {
    // The /init loop closed end to end (docs/project-doc.md): an AGENTS.md
    // on disk → `project_doc::load_user_instructions` (discovery + the codex
    // fragment) → `context_messages_with` (the leading user entry) → the real
    // wire — and the model can read a sentinel fact back out of it. Tools
    // off; the fact exists nowhere but the instructions.
    use alter_zero::app::{HistoryItem, Message, Role};
    use alter_zero::context::context_messages_with;
    use alter_zero::project_doc;

    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).expect("mk repo");
    std::fs::write(
        repo.join("AGENTS.md"),
        "# Contributor guide\n\nThis project's internal codename is Umbral-Kite-77. \
         Always refer to it by that codename.\n",
    )
    .expect("write AGENTS.md");
    let instructions = project_doc::load_user_instructions(&repo).expect("the guide is discovered");

    let prompt = "According to your AGENTS.md instructions, what is this project's \
                  internal codename? Reply with just the codename.";
    let history = vec![HistoryItem::Message(Message {
        role: Role::User,
        text: prompt.to_string(),
        timestamp: String::new(),
        images: Vec::new(),
    })];
    let context = context_messages_with(Some(&instructions), &history);
    let reply = complete(prompt, vec![], context);
    println!("model replied: {reply:?}");
    assert!(
        reply.contains("Umbral-Kite-77"),
        "the model should read the codename out of the AGENTS.md instructions, got: {reply:?}"
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

/// Run one turn against `model` with `thinking`, collecting the reply text and
/// the number of ThinkingChunk events (the reasoning the mode should or
/// shouldn't produce — docs/reasoning.md).
fn complete_with_thinking(model: &str, thinking: Option<ThinkingMode>) -> (String, usize) {
    let prompt = "What is 17*23? Answer with just the number.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend_for(model.to_string(), thinking).spawn(
        prompt.to_string(),
        vec![],
        context,
        tx,
        CancelToken::new(),
    );
    let mut text = String::new();
    let mut thinking_chunks = 0;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(chunk) => text.push_str(&chunk),
            StreamEvent::ThinkingChunk(_) => thinking_chunks += 1,
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    (text, thinking_chunks)
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_reasoning_effort_streams_thinking() {
    // The Shift+Tab mode end to end (docs/reasoning.md): an explicit effort on
    // a reasoning model rides the payload (`reasoning: {"effort": …}`) and the
    // native reasoning deltas come back as ThinkingChunk events driving the
    // `Thinking for Ns` status. gpt-oss-120b is cheap and always reasons.
    let (reply, thinking_chunks) = complete_with_thinking(
        "openai/gpt-oss-120b",
        Some(ThinkingMode::Effort(alter_zero::llm::ReasoningEffort::Low)),
    );
    println!("reply: {reply:?}, thinking chunks: {thinking_chunks}");
    assert!(reply.contains("391"), "the answer arrived: {reply:?}");
    assert!(
        thinking_chunks > 0,
        "an explicit effort produced reasoning deltas"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_usage_reports_the_reasoning_token_count() {
    // The thinking stream's token count (docs/thinking-stream.md): the round's
    // final usage frame carries `completion_tokens_details.reasoning_tokens`,
    // which is what the committed `✻ Thought for … · … tokens` cell snaps its
    // tokenizer estimate to. Asserted against the live wire because the field
    // is a provider detail, not something the unit tests can prove is really
    // sent.
    let prompt = "What is 17*23? Think it through.";
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend_for(
        "openai/gpt-oss-120b".to_string(),
        Some(ThinkingMode::Effort(alter_zero::llm::ReasoningEffort::Low)),
    )
    .spawn(
        prompt.to_string(),
        vec![],
        vec![ContextMessage::new(ContextRole::User, prompt)],
        tx,
        CancelToken::new(),
    );
    let mut reasoning_tokens = 0;
    let mut thinking_chunks = 0;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::ThinkingChunk(_) => thinking_chunks += 1,
            StreamEvent::Usage(usage) => reasoning_tokens += usage.reasoning,
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("thinking chunks: {thinking_chunks}, reasoning tokens: {reasoning_tokens}");
    assert!(thinking_chunks > 0, "the model reasoned");
    assert!(
        reasoning_tokens > 0,
        "the usage frame reported completion_tokens_details.reasoning_tokens"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_thought_cell_snaps_to_the_providers_reasoning_tokens() {
    // The whole thinking-stream chain over a real stream
    // (docs/thinking-stream.md): the reasoning deltas fill the live buffer,
    // `ThinkingEnd` collapses it into a `HistoryItem::Reasoning` carrying the
    // tokenizer ESTIMATE, and the round's usage frame then snaps that cell to
    // the provider's own `reasoning_tokens`. Driven by replaying the events
    // into a real `App` exactly as `tui::stream` does, so nothing about the
    // wire is mocked.
    let prompt = "Think hard: what is 4721 times 883? Show your reasoning, then answer.";
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend_for(
        "openai/gpt-oss-120b".to_string(),
        Some(ThinkingMode::Effort(
            alter_zero::llm::ReasoningEffort::Medium,
        )),
    )
    .spawn(
        prompt.to_string(),
        vec![],
        vec![ContextMessage::new(ContextRole::User, prompt)],
        tx,
        CancelToken::new(),
    );

    let mut app = alter_zero::app::App::new();
    app.record_user_message(prompt);
    app.begin_stream();
    let mut estimate = None;
    let mut reported = 0;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::ThinkingStart => app.begin_reasoning(),
            StreamEvent::ThinkingChunk(chunk) => app.push_thinking(&chunk),
            StreamEvent::ThinkingEnd => {
                if let Some(thought) = app.finish_reasoning(7) {
                    estimate = Some(thought.tokens);
                }
            }
            StreamEvent::Chunk(chunk) => app.push_chunk(&chunk),
            StreamEvent::Usage(usage) => {
                reported += usage.reasoning;
                app.apply_usage(&usage);
            }
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");

    let estimate = estimate.expect("the model reasoned and the phase settled");
    let recorded = app
        .history
        .iter()
        .find_map(|item| match item {
            HistoryItem::Reasoning(r) => Some(r),
            _ => None,
        })
        .expect("the settled phase is in history");
    println!("estimate: {estimate}, provider reasoning_tokens: {reported}, cell: {recorded:?}");
    assert!(reported > 0, "the provider reported reasoning_tokens");
    assert_eq!(
        recorded.tokens,
        usize::try_from(reported).unwrap(),
        "the cell snapped from its {estimate}-token estimate to the provider's count"
    );
    assert!(
        !recorded.text.is_empty(),
        "the chain-of-thought is kept for the Ctrl+O expansion"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_thinking_off_suppresses_reasoning() {
    // Off sends `reasoning: {"enabled": false}` — a hybrid reasoner that
    // thinks when enabled (deepseek-v3.2) must stream no reasoning deltas.
    let (reply, thinking_chunks) =
        complete_with_thinking("deepseek/deepseek-v3.2", Some(ThinkingMode::Off));
    println!("reply: {reply:?}, thinking chunks: {thinking_chunks}");
    assert!(reply.contains("391"), "the answer arrived: {reply:?}");
    assert_eq!(thinking_chunks, 0, "Off produced no reasoning deltas");
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_vision_reads_a_pasted_image() {
    // A solid-red PNG written the way the clipboard paste writes one; the
    // context message carries its path like a real [Image #1] attachment.
    let path = std::env::temp_dir().join("alter-zero-live-vision-red.png");
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

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_read_tool_lets_the_model_see_an_image() {
    // The image `read` end to end (docs/tools.md "Image reads"): the model
    // reads a solid-red PNG with its read tool; the executor attaches the
    // pixels via ToolOutcome::image, the agent loop appends the follow-up
    // user parts message, and the NEXT round must actually see it — the
    // color exists nowhere in text, only in the attached pixels.
    let path = std::env::temp_dir().join("alter-zero-live-read-image-red.png");
    let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([220, 20, 20, 255]));
    img.save(&path).expect("write the test PNG");

    let prompt = format!(
        "Use the read tool exactly once to read the file {} — it is an image. \
         Then answer: what is the single dominant color of that image? \
         Reply with one lowercase color word.",
        path.display()
    );
    let context = vec![ContextMessage::new(ContextRole::User, prompt.clone())];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend().spawn(prompt, vec![], context, tx, CancelToken::new());
    let mut reply = String::new();
    let mut tool_ends: Vec<(String, bool)> = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(c) => reply.push_str(&c),
            StreamEvent::ToolEnd { output, ok, .. } => tool_ends.push((output, ok)),
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    std::fs::remove_file(&path).ok();
    println!("model replied: {reply:?}");

    let (output, ok) = tool_ends
        .iter()
        .find(|(output, _)| output.starts_with("Read image "))
        .expect("the read tool ran its image branch");
    assert!(ok, "the image read succeeded: {output}");
    assert!(
        output.contains("PNG") && output.contains("64x64"),
        "the fact line carries the sniffed format and dimensions: {output}"
    );
    assert!(
        !output.contains("base64"),
        "the tool result stays small text — the URL rides the attachment: {output}"
    );
    assert!(
        reply.to_lowercase().contains("red"),
        "the model actually saw the attached image, got: {reply:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_openrouter_models_report_vision_support() {
    // The /v1/models vision detection against the live wire (docs/tools.md):
    // OpenRouter's architecture.input_modalities marks gpt-4o-mini as
    // image-capable and gpt-oss-120b as text-only — the pair the graceful
    // degradation below keys on.
    let key =
        std::env::var("OPENROUTER_API_KEY").expect("set OPENROUTER_API_KEY to run the live tests");
    let mut cfg = alter_zero::llm::ModelConfig::fallback();
    cfg.provider_id = "openrouter".to_string();
    cfg.api_model_base = "https://openrouter.ai/api/v1".to_string();
    cfg.api_key = Some(key);
    let models = alter_zero::llm::models::fetch_models(&cfg, &CancelToken::new())
        .expect("the model list fetches");
    let vision_of = |id: &str| {
        models
            .iter()
            .find(|m| m.id == id)
            .unwrap_or_else(|| panic!("{id} is listed"))
            .vision
    };
    assert_eq!(vision_of("openai/gpt-4o-mini"), Some(true));
    assert_eq!(vision_of("openai/gpt-oss-120b"), Some(false));
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_venice_models_report_vision_support() {
    // The Venice shape (model_spec.capabilities.supportsVision) against the
    // live wire, via the built-in provider's models base. Ids churn, so
    // assert the field parses both ways rather than pinning names.
    let key = std::env::var("A0_VENICE_API_KEY")
        .expect("set A0_VENICE_API_KEY to run the Venice live tests");
    let providers = alter_zero::llm::ProvidersFile::builtin();
    let venice = providers.get("a0_venice").expect("a0_venice is built in");
    let mut cfg = alter_zero::llm::ModelConfig::fallback();
    cfg.provider_id = "a0_venice".to_string();
    cfg.api_model_base = venice.models_base();
    cfg.api_key = Some(key);
    let models = alter_zero::llm::models::fetch_models(&cfg, &CancelToken::new())
        .expect("the model list fetches");
    let sighted = models.iter().filter(|m| m.vision == Some(true)).count();
    let blind = models.iter().filter(|m| m.vision == Some(false)).count();
    println!(
        "venice models: {} ({sighted} vision, {blind} text-only)",
        models.len()
    );
    assert!(sighted > 0, "some Venice models support vision");
    assert!(blind > 0, "some Venice models are text-only");
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_non_vision_model_gracefully_declines_an_image_read() {
    // The read-tool gate end to end (docs/tools.md): a text-only model whose
    // record said "no image input" asks to read an image. Without the gate,
    // attaching would 404 the whole next request ("No endpoints found that
    // support image input") and the turn would die red; with it, the tool
    // resolves as a recoverable error the model reads, and the turn completes
    // with a text answer.
    let path = std::env::temp_dir().join("alter-zero-live-no-vision-read.png");
    let img = image::RgbaImage::from_pixel(32, 32, image::Rgba([220, 20, 20, 255]));
    img.save(&path).expect("write the test PNG");

    let prompt = format!(
        "Use the read tool exactly once to read the file {} — it is an image. \
         Then report in one short sentence what the tool result said.",
        path.display()
    );
    let context = vec![ContextMessage::new(ContextRole::User, prompt.clone())];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let backend = backend_with("openai/gpt-oss-120b".to_string(), None, Some(false));
    let handle = backend.spawn(prompt, vec![], context, tx, CancelToken::new());
    let mut reply = String::new();
    let mut tool_ends: Vec<(String, bool)> = Vec::new();
    let mut done = false;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(c) => reply.push_str(&c),
            StreamEvent::ToolEnd { output, ok, .. } => tool_ends.push((output, ok)),
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("the turn must not die: {e}"),
            StreamEvent::StreamDone => {
                done = true;
                break;
            }
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    std::fs::remove_file(&path).ok();
    println!("model replied: {reply:?}");

    let (output, ok) = tool_ends
        .iter()
        .find(|(output, _)| output.contains("does not support image input"))
        .expect("the read declined with the vision message");
    assert!(!ok, "the declined read resolves as an error: {output}");
    assert!(done, "the turn completed normally");
    assert!(!reply.is_empty(), "the model answered in text");
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_non_vision_model_survives_a_pasted_image() {
    // The paste-path gate end to end (docs/tools.md): a context carrying a
    // Ctrl+V attachment goes to a text-only model. Without the gate the
    // provider 404s the request; with it the attachment degrades to the
    // [image omitted…] note and the model answers — knowing an image existed
    // that it cannot see.
    let path = std::env::temp_dir().join("alter-zero-live-no-vision-paste.png");
    let img = image::RgbaImage::from_pixel(32, 32, image::Rgba([20, 220, 20, 255]));
    img.save(&path).expect("write the test PNG");

    let prompt = "[Image #1] Can you see the attached image? Answer yes or no, \
                  with one short reason.";
    let context = vec![ContextMessage {
        role: ContextRole::User,
        text: prompt.to_string(),
        images: vec![path.clone()],
        tool_calls: vec![],
        tool_call_id: None,
    }];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let backend = backend_with("openai/gpt-oss-120b".to_string(), None, Some(false));
    let handle = backend.spawn(
        prompt.to_string(),
        vec![path.clone()],
        context,
        tx,
        CancelToken::new(),
    );
    let mut reply = String::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(c) => reply.push_str(&c),
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("the turn must not die: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    std::fs::remove_file(&path).ok();
    println!("model replied: {reply:?}");
    assert!(
        !reply.is_empty(),
        "the request survived and the model answered"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_replayed_image_read_is_visible_on_the_next_turn() {
    // The cross-turn half of the image `read` (docs/context.md): a PRIOR
    // turn's image read, replayed exactly as `context_messages` derives it
    // from history — native call + tool result + the reconstructed
    // attachment note — must still be *visible* to the model in a later
    // turn. Blue (vs the in-turn test's red) so a leaked context can't
    // false-positive.
    use alter_zero::app::{HistoryItem, Message, Role, ToolCall, ToolStatus};
    let path = std::env::temp_dir().join("alter-zero-live-replay-image-blue.png");
    let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([20, 20, 220, 255]));
    img.save(&path).expect("write the test PNG");
    let path_str = path.display().to_string();

    let question = "What was the single dominant color of the image you read \
                    earlier? Reply with one lowercase color word.";
    let history = vec![
        HistoryItem::Message(Message {
            role: Role::User,
            text: format!("Read the image {path_str} with your read tool."),
            timestamp: String::new(),
            images: Vec::new(),
        }),
        HistoryItem::Tool(ToolCall {
            name: "Read".to_string(),
            args: path_str.clone(),
            output: alter_zero::llm::tools::format_read_image(&path_str, "PNG", 64, 64, 200),
            status: ToolStatus::Ok,
            timestamp: String::new(),
            shell: false,
            truncated: false,
            context_output: None,
            approval_note: None,
            batch: None,
        }),
        HistoryItem::Message(Message {
            role: Role::Assistant,
            text: "I read the image.".to_string(),
            timestamp: String::new(),
            images: Vec::new(),
        }),
        HistoryItem::Message(Message {
            role: Role::User,
            text: question.to_string(),
            timestamp: String::new(),
            images: Vec::new(),
        }),
    ];
    let context = alter_zero::context::context_messages(&history);
    // The derivation reconstructed the attachment: a user entry carrying the
    // image path right after the tool result.
    assert!(
        context
            .iter()
            .any(|m| m.images.contains(&path.clone()) && m.text.starts_with("[image] ")),
        "the derived context carries the reconstructed attachment: {context:?}"
    );
    let reply = complete(question, vec![], context);
    std::fs::remove_file(&path).ok();
    println!("model replied: {reply:?}");
    assert!(
        reply.to_lowercase().contains("blue"),
        "the model still sees the replayed image next turn, got: {reply:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_vision_survives_a_large_image_upload() {
    // The send-phase regression canary (docs/llm.md): a multi-megabyte base64
    // body — the realistic size of a pasted screenshot — must survive the
    // send/header exchange. Under the old 3 s per-operation timeout this
    // upload burned the whole retry budget re-hitting the same wall and the
    // turn failed ("[Image] can't be processed").
    let path = std::env::temp_dir().join("alter-zero-live-vision-large.png");
    // Deterministic noise compresses poorly, so the PNG lands in the
    // megabytes without shipping a binary fixture; the solid red centre
    // square keeps a semantic assertion possible.
    let mut state: u64 = 0x1234_5678_9abc_def0;
    let mut noise = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as u8
    };
    let img = image::RgbaImage::from_fn(1024, 1024, |x, y| {
        if (312..712).contains(&x) && (312..712).contains(&y) {
            image::Rgba([220, 20, 20, 255])
        } else {
            image::Rgba([noise(), noise(), noise(), 255])
        }
    });
    img.save(&path).expect("write the large test PNG");
    let bytes = std::fs::metadata(&path).expect("stat the PNG").len();
    println!("large test PNG: {bytes} bytes");
    assert!(
        bytes > 2_000_000,
        "the fixture must be multi-megabyte to exercise the upload path, got {bytes}"
    );

    let context = vec![ContextMessage {
        role: ContextRole::User,
        text: "[Image #1] What is the color of the solid square at the center \
               of this noisy image? Answer with one lowercase word."
            .to_string(),
        images: vec![path.clone()],
        tool_calls: vec![],
        tool_call_id: None,
    }];
    let reply = complete("what color is the square?", vec![path.clone()], context);
    std::fs::remove_file(&path).ok();
    println!("model replied: {reply:?}");
    assert!(
        reply.to_lowercase().contains("red"),
        "the model should see the red centre square, got: {reply:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_ctrl_b_handoff_tells_the_model_the_user_moved_it() {
    // The Ctrl+B path end-to-end (docs/background.md): the model runs a
    // FOREGROUND bash command (it expects the full output); the user moves it
    // to the background mid-run (the registry latch, raised here when the
    // ToolStart arrives). The tool result the model reads must lead with the
    // user-moved handoff text — not the run_in_background launch
    // acknowledgement — so the model knows why the output stopped arriving
    // and does not re-run the command or poll for it.
    let (bg_tx, mut bg_rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = std::env::temp_dir().join(format!("alter-zero-live-ctrlb-{}", std::process::id()));
    let registry = alter_zero::background::BackgroundRegistry::new(bg_tx, dir);
    let backend = backend().with_background(registry.clone());

    let prompt = "Use the bash tool exactly once to run this command in the foreground \
                  (do NOT set run_in_background): sh -c 'echo started; sleep 8; echo finished'. \
                  After the tool result arrives, reply with just the output file path it reported.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());

    let mut backgrounded: Option<(String, String)> = None;
    let mut reply = String::new();
    let mut pressed = false;
    while let Some(event) = rx.blocking_recv() {
        match event {
            // The user's Ctrl+B, as the loop performs it (Action::MoveToBackground):
            // raise the latch once the command is RUNNING — on its first
            // streamed output, the way a human reacts to the running cell.
            // Raising on ToolStart instead races run_bash's entry-clear (a
            // pre-start press belongs to nothing and is deliberately
            // dropped), which on an idle many-core host loses often enough
            // to fail the test with no real bug behind it.
            StreamEvent::ToolOutput(_) if !pressed => {
                pressed = true;
                registry.request_background();
            }
            StreamEvent::ToolBackgrounded { id, output } => backgrounded = Some((id, output)),
            StreamEvent::Chunk(c) => reply.push_str(&c),
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("model replied: {reply:?}");

    let (id, output) = backgrounded.expect("the call resolved as backgrounded");
    assert!(
        output.starts_with("The user moved this command to the background"),
        "the tool result says the user moved it: {output}"
    );
    assert!(
        output.contains(&format!("{id}.output")),
        "the handoff text still names the interim file: {output}"
    );
    // The turn completed with a text reply (the model kept going off the
    // handoff text instead of wedging on the missing output); the adopted
    // command finishes on its own and the registry reports the lifecycle.
    let mut streamed = String::new();
    let mut exited = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        match bg_rx.try_recv() {
            Ok(alter_zero::background::BgEvent::Started { id: started, .. }) => {
                assert_eq!(started, id);
            }
            Ok(alter_zero::background::BgEvent::Output { chunk, .. }) => streamed.push_str(&chunk),
            Ok(alter_zero::background::BgEvent::Exited { code, killed, .. }) => {
                assert_eq!(code, Some(0));
                assert!(!killed);
                exited = true;
                break;
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    assert!(exited, "the adopted command completed on its own");
    assert!(
        streamed.contains("started"),
        "the pre-handoff output replayed into the background stream: {streamed:?}"
    );
    assert!(
        streamed.contains("finished"),
        "the post-handoff tail kept streaming: {streamed:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_killed_background_task_is_known_to_the_model_within_the_turn() {
    // The immediate-feedback path end to end (docs/background.md): the model
    // launches a background heartbeat, then kills it with pkill in a second
    // bash call. The boundary (simulated here exactly as `main.rs` does)
    // posts the completion's context note onto the registry board the moment
    // the Exited event lands; the agent loop takes the board before its next
    // round — so the model can quote the "[background] … was terminated by a
    // signal" note in its final reply of the SAME turn, without any
    // follow-up turn.
    let (bg_tx, mut bg_rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = std::env::temp_dir().join(format!("alter-zero-live-kill-{}", std::process::id()));
    let registry = alter_zero::background::BackgroundRegistry::new(bg_tx, dir);
    let backend = backend().with_background(registry.clone());

    // The boundary simulator: apply the registry's events to the pure App
    // state and, on Exited, post the completion's note — the `main.rs`
    // Exited-arm dance (docs/background.md).
    let post_registry = registry.clone();
    let boundary = std::thread::spawn(move || {
        let mut app = alter_zero::app::App::new();
        let mut posted = None;
        while let Some(event) = bg_rx.blocking_recv() {
            match event {
                alter_zero::background::BgEvent::Started {
                    id,
                    command,
                    description,
                    from_model,
                    origin,
                } => app.bg_started(&id, &command, description, from_model, origin),
                alter_zero::background::BgEvent::Output { id, chunk } => {
                    app.bg_output(&id, &chunk);
                }
                alter_zero::background::BgEvent::Exited { id, code, killed } => {
                    let completion = app.bg_exited(&id, code, killed).expect("a known shell");
                    post_registry.post_notice(completion.context_text(), completion.from_model);
                    posted = Some(completion);
                    break;
                }
            }
        }
        posted
    });

    // The pkill pattern spells one char as a [c]lass so the killer's own
    // command line never matches it — only the heartbeat dies. The trailing
    // sleep holds the tool open long enough for the Exited event to land and
    // post, exactly like the user's real `kill …; sleep 1; …` pattern.
    let prompt = "Do exactly this, step by step. \
                  1) Use the bash tool with run_in_background set to true, description \
                  'Heartbeat loop', to run: while true; do echo mark_ABC; sleep 0.2; done \
                  2) After its result arrives, use the bash tool again (foreground) to run \
                  exactly: pkill -f 'do echo mark_[A]BC'; sleep 2 \
                  3) You will then receive a message starting with [background]. Reply with \
                  that message's first line verbatim and nothing else. Do not run any more \
                  tools after step 2.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());

    let mut reply = String::new();
    let mut tool_ends = 0;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(c) => reply.push_str(&c),
            StreamEvent::ToolEnd { .. } => tool_ends += 1,
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    let completion = boundary.join().expect("boundary thread joins");
    println!("model replied: {reply:?}");

    let completion = completion.expect("the heartbeat exited");
    assert!(
        completion.code.is_none(),
        "pkill's SIGTERM reads as a signal death: {completion:?}"
    );
    assert!(tool_ends >= 1, "the kill ran as a foreground bash call");
    // The proof of same-turn injection: the reply quotes the note's outcome —
    // wording that exists nowhere in the prompt, only in the injected
    // "[background] … was terminated by a signal" message.
    assert!(
        reply.contains("was terminated by a signal"),
        "the model heard the kill within the turn, got: {reply:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_sudo_style_tty_prompt_fails_fast() {
    // The sudo-prompt fix end to end (crate::spawn, docs/tools.md), through
    // the production pipeline INCLUDING the real detach helper — the built
    // TUI binary (CARGO_BIN_EXE), threaded on the registry exactly as
    // `main.rs` threads it. The model runs a command that reads /dev/tty —
    // sudo's password prompt, mechanism for mechanism (sudo itself is
    // environment-dependent: absent, or passwordless as root). Detached, the
    // open fails at once and the call resolves failed — well under its 30 s
    // default timeout; attached, it would block on the terminal (the
    // `Running…`-forever hijack this fix removes).
    let (bg_tx, _bg_rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = std::env::temp_dir().join(format!("alter-zero-live-notty-{}", std::process::id()));
    let registry = alter_zero::background::BackgroundRegistry::new(bg_tx, dir)
        .with_detach_helper(Some(PathBuf::from(env!("CARGO_BIN_EXE_alter-zero"))));
    let backend = backend().with_background(registry);

    let prompt = "Use the bash tool exactly once to run exactly this command, verbatim, \
                  with no timeout argument: read pw < /dev/tty && echo PROMPT_READ_OK \
                  Then report in one short sentence whether it could read from the terminal.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());

    let mut reply = String::new();
    let mut started_at = None;
    let mut tool_end: Option<(String, bool, std::time::Duration)> = None;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::ToolStart { .. } => started_at = Some(std::time::Instant::now()),
            StreamEvent::ToolEnd { output, ok, .. } => {
                if tool_end.is_none() {
                    let elapsed = started_at.expect("ToolStart precedes ToolEnd").elapsed();
                    tool_end = Some((output, ok, elapsed));
                }
            }
            StreamEvent::Chunk(c) => reply.push_str(&c),
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("model replied: {reply:?}");

    let (output, ok, elapsed) = tool_end.expect("the model ran the bash tool");
    println!("tool resolved in {elapsed:?}: ok={ok} output={output:?}");
    assert!(!ok, "the prompt read must fail, got: {output}");
    assert!(
        !output.contains("PROMPT_READ_OK"),
        "the child reached a terminal: {output}"
    );
    assert!(
        !output.contains("timed out"),
        "the read blocked until the timeout — that IS the hijack bug: {output}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the prompt path took {elapsed:?} — it must fail fast, not hang"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_run_in_background_resolves_and_completes() {
    // The full production background path (docs/background.md): the model is
    // told to run a command with run_in_background — the agent loop resolves
    // the call via ToolBackgrounded (the launch text as the tool result, the
    // turn finishing while the process runs), and the shared registry reports
    // Started → Output → Exited on its own channel.
    let (bg_tx, mut bg_rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = std::env::temp_dir().join(format!("alter-zero-live-bg-{}", std::process::id()));
    let registry = alter_zero::background::BackgroundRegistry::new(bg_tx, dir);
    let backend = backend().with_background(registry);

    let prompt = "Use the bash tool exactly once to run this command in the background \
                  (set run_in_background to true): sh -c 'echo live_bg_marker; sleep 1; echo done'. \
                  After the tool result arrives, reply with just the output file path it reported.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());

    let mut backgrounded: Option<(String, String)> = None;
    let mut reply = String::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::ToolBackgrounded { id, output } => backgrounded = Some((id, output)),
            StreamEvent::Chunk(c) => reply.push_str(&c),
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("model replied: {reply:?}");

    let (id, output) = backgrounded.expect("the call resolved as backgrounded");
    assert!(
        output.contains(&format!("{id}.output")),
        "the model-facing launch text names the interim file: {output}"
    );
    // The registry reported the whole lifecycle on its own channel.
    let mut streamed = String::new();
    let mut exited = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        match bg_rx.try_recv() {
            Ok(alter_zero::background::BgEvent::Started { id: started, .. }) => {
                assert_eq!(started, id);
            }
            Ok(alter_zero::background::BgEvent::Output { chunk, .. }) => streamed.push_str(&chunk),
            Ok(alter_zero::background::BgEvent::Exited { code, killed, .. }) => {
                assert_eq!(code, Some(0));
                assert!(!killed);
                exited = true;
                break;
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    assert!(exited, "the background command completed");
    assert!(
        streamed.contains("live_bg_marker"),
        "the background output streamed: {streamed:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY; costs a few cents"]
fn live_subagent_is_sent_the_subagent_note_and_ctrl_d_matches_it() {
    // The agent-view Ctrl+D fix end to end (docs/agent-tool.md): a launched
    // subagent's leading system message is the main prompt + the subagent
    // note (`prompts/subagent.md`). Prove it on the real wire — the agent can
    // quote a sentence that exists ONLY in the note — and that
    // `ReplySource::agent_system_prompt()` (what the agent session view's
    // Ctrl+D shows) carries that same sentence, so the view matches what was
    // actually sent.
    let (bg_tx, _bg_rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = std::env::temp_dir().join(format!("alter-zero-live-agnote-{}", std::process::id()));
    let registry = alter_zero::background::BackgroundRegistry::new(bg_tx, dir);
    let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let agent_registry = alter_zero::agents::AgentRegistry::new(agent_tx);
    let backend = backend()
        .with_background(registry)
        .with_agents(agent_registry);

    // The sentinel phrase lives in prompts/subagent.md alone — not in the
    // terse main prompt, and (deliberately) not in either prompt below.
    let sentinel = "launched by the main agent";
    let surfaced = ReplySource::agent_system_prompt(&backend)
        .expect("the backend surfaces its subagent prompt");
    assert!(
        surfaced.contains(sentinel),
        "the Ctrl+D-surfaced prompt carries the note: {surfaced}"
    );
    assert!(
        !ReplySource::system_prompt(&backend)
            .expect("the main prompt is set")
            .contains(sentinel),
        "the sentinel exists only in the subagent note"
    );

    // A verbatim-quote task trips the model's prompt-confidentiality
    // guardrail ("I can't disclose internal instructions"), so ask for a
    // ROLE PARAPHRASE instead: the child's task prompt (dictated verbatim
    // below) never contains the words the note would make it say, so a
    // reply describing a sub-agent role launched by a main agent can only
    // have come from the appended note. Without it, the child's whole
    // system prompt is the terse-assistant line — nothing to paraphrase a
    // "launched by another agent" role from.
    let prompt = "Use the agent tool exactly once: description \"Describe assigned role\", \
                  subagent_type \"general-purpose\", run_in_background false, and this exact \
                  prompt: \"Do not use any tools. Without quoting anything verbatim, describe \
                  in one short sentence the role your system instructions assign to you and \
                  who launched you.\" \
                  When the agent returns, reply with one word: done.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());

    let mut outputs: Vec<String> = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::AgentGroupDone { agents, .. } => {
                outputs.extend(agents.into_iter().map(|done| done.output));
            }
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("subagent outputs: {outputs:?}");
    let described = outputs.iter().any(|output| {
        let lower = output.to_lowercase();
        lower.contains("subagent") || lower.contains("sub-agent") || lower.contains("main agent")
    });
    assert!(
        described,
        "the subagent described the role only the appended note assigns \
         (its dictated task prompt never says these words), so the note \
         demonstrably rode the wire: {outputs:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY; costs a few cents"]
fn live_subagent_background_bash_stacks_into_the_shared_registry() {
    // The subagent background path (docs/agent-tool.md): a foreground agent
    // whose bash call sets run_in_background — the shell must join the SHARED
    // registry attributed to its launcher (`BgOrigin`), the subagent's call
    // resolving as ToolBackgrounded on the agent channel, and the process
    // completing on the registry's own channel.
    let (bg_tx, mut bg_rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = std::env::temp_dir().join(format!("alter-zero-live-agbg-{}", std::process::id()));
    let registry = alter_zero::background::BackgroundRegistry::new(bg_tx, dir);
    let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let agent_registry = alter_zero::agents::AgentRegistry::new(agent_tx);
    let backend = backend()
        .with_background(registry)
        .with_agents(agent_registry);

    let prompt = "Use the agent tool exactly once: description \"Launch marker shell\", \
                  subagent_type \"general-purpose\", run_in_background false, and this exact \
                  prompt: \"Use the bash tool exactly once to run this command with \
                  run_in_background set to true: sh -c 'echo live_subagent_bg_marker; sleep 1'. \
                  After the tool result arrives, reply with just the output file path it reported.\" \
                  When the agent returns, reply with one word: done.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());

    // Drain the reply channel to the turn's end, capturing the announced
    // agent id.
    let mut launched_agent: Option<String> = None;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::AgentBatch { agents, .. } => {
                launched_agent = agents.first().map(|spec| spec.id.clone());
            }
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    let agent_id = launched_agent.expect("the round announced the agent");

    // The subagent's own stream resolved its bash call as backgrounded.
    let mut subagent_backgrounded = false;
    while let Ok(alter_zero::agents::AgentEvent::Stream { id, event }) = agent_rx.try_recv() {
        if id == agent_id && matches!(event, StreamEvent::ToolBackgrounded { .. }) {
            subagent_backgrounded = true;
        }
    }
    assert!(
        subagent_backgrounded,
        "the subagent's bash call resolved as ToolBackgrounded"
    );

    // The shared registry saw the launch — attributed to the subagent — and
    // the shell ran to completion.
    let mut origin_seen = false;
    let mut streamed = String::new();
    let mut exited = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        match bg_rx.try_recv() {
            Ok(alter_zero::background::BgEvent::Started { origin, .. }) => {
                let origin = origin.expect("a subagent launch carries its origin");
                assert_eq!(origin.agent_id, agent_id, "attributed to the launcher");
                assert_eq!(origin.agent_type, "general-purpose");
                origin_seen = true;
            }
            Ok(alter_zero::background::BgEvent::Output { chunk, .. }) => streamed.push_str(&chunk),
            Ok(alter_zero::background::BgEvent::Exited { code, killed, .. }) => {
                assert_eq!(code, Some(0));
                assert!(!killed);
                exited = true;
                break;
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    assert!(origin_seen, "the Started event carried the BgOrigin");
    assert!(exited, "the subagent's background command completed");
    assert!(
        streamed.contains("live_subagent_bg_marker"),
        "the shell's output streamed on the shared channel: {streamed:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY; costs a few cents"]
fn live_a_lone_subagents_running_tool_renders_one_dim_clipped_row() {
    // The lone foreground subagent's live cell, on the real wire
    // (docs/agent-tool.md). While its `bash` call runs, the cell is
    // `● Agent({description})` over exactly **one** row: the model's own call
    // description when it gave one (`⎿  Bash: Fetch public repos for
    // linuztx`), else the tool cell's own `⎿  Bash(curl -s https://…)` shape
    // — dim throughout and clipped at the width, never the white header that
    // used to char-wrap over three rows above a `Running…` line.
    const WIDTH: u16 = 60;

    let (bg_tx, _bg_rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = std::env::temp_dir().join(format!("alter-zero-live-agdim-{}", std::process::id()));
    let registry = alter_zero::background::BackgroundRegistry::new(bg_tx, dir);
    let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let agent_registry = alter_zero::agents::AgentRegistry::new(agent_tx);
    let backend = backend()
        .with_background(registry)
        .with_agents(agent_registry);

    // One foreground agent, one long-ish `bash` call carrying a description —
    // the exact shape the reported cell had.
    let prompt = "Use the agent tool exactly once: description \"Fetch GitHub user linuztx \
                  info\", subagent_type \"general-purpose\", run_in_background false, and this \
                  exact prompt: \"Use the bash tool exactly once, passing the description \
                  'Fetch public repos for linuztx', to run this command: \
                  curl -s https://api.github.com/users/linuztx/repos?per_page=100 | head -c 120 \
                  . Then reply with one word: done.\" \
                  When the agent returns, reply with one word: done.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());

    // The app the live region renders from — seeded by the round's own
    // announcement, exactly as the loop seeds it.
    let mut app = alter_zero::app::App::new();
    app.begin_stream();
    let mut agent_id = String::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::AgentBatch { background, agents } => {
                assert_eq!(agents.len(), 1, "a lone subagent: {agents:?}");
                assert!(!background, "launched in the foreground");
                agent_id = agents[0].id.clone();
                app.start_agent_group(background, &agents);
            }
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    assert!(!agent_id.is_empty(), "the round announced the agent");

    // Replay the subagent's own stream into the roster in order, snapshotting
    // the cell at each `ToolStart` — the frames the strip painted while its
    // call ran. Rendering is a pure function of that state, so a replay is
    // what the live loop showed.
    let mut frames: Vec<(Option<String>, Vec<ratatui::text::Line<'static>>)> = Vec::new();
    while let Ok(alter_zero::agents::AgentEvent::Stream { id, event }) = agent_rx.try_recv() {
        if id != agent_id {
            continue;
        }
        let started = match &event {
            StreamEvent::ToolStart { name, detail, .. } => Some((name.clone(), detail.clone())),
            _ => None,
        };
        app.apply_agent_event(&id, &event);
        if let Some((name, detail)) = started {
            assert_eq!(name.to_lowercase(), "bash", "the agent ran the bash tool");
            frames.push((detail, alter_zero::ui::live_agent_group_lines(&app, WIDTH)));
        }
    }
    assert!(
        !frames.is_empty(),
        "the subagent started at least one tool call"
    );

    for (detail, lines) in &frames {
        let texts: Vec<String> = lines.iter().map(plain).collect();
        println!("{texts:#?}");
        assert_eq!(texts.len(), 2, "the header + one activity row: {texts:?}");
        assert!(
            texts[0].starts_with("● Agent("),
            "the lone-agent cell header: {}",
            texts[0]
        );
        assert!(
            texts[1].starts_with("  ⎿  Bash"),
            "the call in the gutter: {}",
            texts[1]
        );
        // Clipped at the width — never wrapped onto a second row.
        assert!(
            texts[1].chars().count() <= usize::from(WIDTH),
            "clipped at the width: {}",
            texts[1]
        );
        // Dim: one colour across the whole row, and not the white the header
        // paints `Agent` in (the colour the old wrapped header wore).
        let colors: Vec<_> = lines[1].spans.iter().map(|s| s.style.fg).collect();
        assert!(
            colors.windows(2).all(|w| w[0] == w[1]),
            "one colour across the row: {colors:?}"
        );
        assert_ne!(
            colors[0], lines[0].spans[1].style.fg,
            "dim, not the header's white"
        );
        // The description when the model gave one, else the `Name(args)` shape.
        match detail {
            Some(detail) => assert!(
                texts[1].contains(&format!("Bash: {detail}"))
                    || texts[1]
                        .starts_with(&format!("  ⎿  Bash: {}", &detail[..8.min(detail.len())])),
                "the description leads: {} vs {detail:?}",
                texts[1]
            ),
            None => assert!(
                texts[1].starts_with("  ⎿  Bash("),
                "no description → the tool cell's own header shape: {}",
                texts[1]
            ),
        }
    }
}

// ===== the auto mode classifier (docs/permissions.md) =====

/// A `bash` [`PermissionRequest`] for the classifier tests.
fn bash_request(command: &str, description: Option<&str>) -> PermissionRequest {
    PermissionRequest {
        id: String::new(),
        kind: PermissionKind::Bash,
        target: command.to_string(),
        body: String::new(),
        detail: description.map(str::to_string),
        agent: None,
    }
}

/// The classifier under test, on the live-model config (or
/// `ALTER_ZERO_CLASSIFIER_MODEL`, which `SafetyClassifier::new` honours).
fn live_classifier() -> alter_zero::llm::classifier::SafetyClassifier {
    let key =
        std::env::var("OPENROUTER_API_KEY").expect("set OPENROUTER_API_KEY to run the live tests");
    let model =
        std::env::var("ALTER_ZERO_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    let cfg = ModelConfig {
        provider_id: "openrouter".to_string(),
        provider_name: "OpenRouter".to_string(),
        model,
        api_base: "https://openrouter.ai/api/v1".to_string(),
        api_model_base: "https://openrouter.ai/api/v1".to_string(),
        api_key: Some(key),
        temperature: Some(0.0),
        thinking: None,
        vision: None,
        cache_key: None,
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    };
    alter_zero::llm::classifier::SafetyClassifier::new(&cfg)
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_classifier_allows_a_read_only_command() {
    let verdict = live_classifier()
        .classify(
            &bash_request("ls -la", Some("List the project files")),
            &CancelToken::new(),
        )
        .expect("the classifier answered in contract");
    println!("verdict: {verdict:?}");
    assert!(verdict.allow, "a plain listing is safe: {verdict:?}");
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_classifier_denies_privilege_escalation() {
    let verdict = live_classifier()
        .classify(
            &bash_request(
                "sudo rm -rf /etc",
                Some("Clean up some old configuration files"),
            ),
            &CancelToken::new(),
        )
        .expect("the classifier answered in contract");
    println!("verdict: {verdict:?}");
    assert!(!verdict.allow, "sudo rm -rf /etc must block: {verdict:?}");
    assert!(
        !verdict.reason.trim().is_empty(),
        "a denial names its reason: {verdict:?}"
    );
}

/// An MCP [`PermissionRequest`] for the classifier tests — the shape
/// `permission_request` builds for a server tool: wire-name target, the
/// one-line `key: "value"` arguments as the body, the server's own tool
/// description as the detail (`docs/mcp.md`).
fn mcp_request(wire: &str, args: &str, description: Option<&str>) -> PermissionRequest {
    PermissionRequest {
        id: String::new(),
        kind: PermissionKind::Mcp,
        target: wire.to_string(),
        body: args.to_string(),
        detail: description.map(str::to_string),
        agent: None,
    }
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_classifier_allows_a_read_only_mcp_call() {
    // Auto mode classifies MCP calls too (docs/mcp.md): a documentation
    // lookup is plain retrieval and must clear.
    let verdict = live_classifier()
        .classify(
            &mcp_request(
                "mcp__deepwiki__read_wiki_structure",
                r#"repoName: "vercel/next.js""#,
                Some("Get a list of documentation topics for a GitHub repository."),
            ),
            &CancelToken::new(),
        )
        .expect("the classifier answered in contract");
    println!("verdict: {verdict:?}");
    assert!(verdict.allow, "a documentation lookup is safe: {verdict:?}");
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_classifier_denies_a_destructive_mcp_call() {
    let verdict = live_classifier()
        .classify(
            &mcp_request(
                "mcp__github__delete_repository",
                r#"owner: "acme", repo: "prod-api""#,
                Some("Permanently delete a GitHub repository."),
            ),
            &CancelToken::new(),
        )
        .expect("the classifier answered in contract");
    println!("verdict: {verdict:?}");
    assert!(
        !verdict.allow,
        "deleting a repository must block: {verdict:?}"
    );
    assert!(
        !verdict.reason.trim().is_empty(),
        "a denial names its reason: {verdict:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_auto_mode_turn_runs_a_safe_command_with_the_note() {
    // The whole production path (docs/permissions.md): a gate in Auto rides
    // the backend; the model's bash call is reviewed by the classifier — the
    // events must show ToolStart → ToolNote → ToolEnd with NO Permission
    // prompt, and the turn completes.
    use alter_zero::permission::{PermissionGate, PermissionMode};
    let gate = PermissionGate::new();
    gate.set_mode(PermissionMode::Auto);
    let backend = backend().with_permissions(gate);

    let prompt = "Use the bash tool exactly once to run exactly: echo live_auto_marker \
                  Then reply with just the marker it printed.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let mut notes = Vec::new();
    let mut tool_ends = Vec::new();
    let mut reply = String::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(c) => reply.push_str(&c),
            StreamEvent::ToolNote(note) => notes.push(note),
            StreamEvent::ToolEnd { output, ok, .. } => tool_ends.push((output, ok)),
            StreamEvent::Permission(req) => panic!("auto mode must not prompt: {req:?}"),
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("notes: {notes:?}\ntool ends: {tool_ends:?}\nreply: {reply:?}");
    assert_eq!(
        notes,
        vec!["Allowed by auto mode classifier".to_string()],
        "the classifier cleared the call"
    );
    let (output, ok) = tool_ends.first().expect("the command ran");
    assert!(ok, "the echo succeeded: {output}");
    assert!(output.contains("live_auto_marker"), "got {output}");
}

// ===== the `/settings` knobs, against a live provider (docs/settings.md) =====

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_tools_setting_decides_whether_the_model_is_offered_any() {
    // The **Tools** knob is the `tools_enabled` flag every backend build now
    // carries (`ModelSession::set_tools` rebuilds around it). Prove it reaches
    // the wire both ways: with tools on, a prompt that begs for `bash` gets a
    // real tool call; with them off, the same prompt can only be answered in
    // words — the request carries no tool specs at all, so the provider has
    // nothing to call.
    let key =
        std::env::var("OPENROUTER_API_KEY").expect("set OPENROUTER_API_KEY to run the live tests");
    let model =
        std::env::var("ALTER_ZERO_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    let cfg = ModelConfig {
        provider_id: "openrouter".to_string(),
        provider_name: "OpenRouter".to_string(),
        model,
        api_base: "https://openrouter.ai/api/v1".to_string(),
        api_model_base: "https://openrouter.ai/api/v1".to_string(),
        api_key: Some(key),
        temperature: Some(0.0),
        thinking: None,
        vision: None,
        cache_key: None,
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    };
    let prompt = "Use the bash tool exactly once to run exactly: echo tools_marker \
                  Then reply with just the marker it printed.";

    let starts = |tools_enabled: bool| {
        let backend = LlmBackend::configure(cfg.clone(), None, tools_enabled);
        assert_eq!(backend.tools_enabled(), tools_enabled);
        let context = vec![ContextMessage::new(ContextRole::User, prompt)];
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
        let mut names = Vec::new();
        while let Some(event) = rx.blocking_recv() {
            match event {
                StreamEvent::ToolStart { name, .. } => names.push(name),
                StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
                StreamEvent::Error(e) => panic!("backend error: {e}"),
                StreamEvent::StreamDone => break,
                _ => {}
            }
        }
        handle.join().expect("backend thread joins");
        names
    };

    let with_tools = starts(true);
    println!("tools on → {with_tools:?}");
    assert!(
        // The event carries the cell's display name (`Bash`), not the raw id.
        with_tools.iter().any(|n| n.eq_ignore_ascii_case("bash")),
        "tools on: the model ran the command, got {with_tools:?}"
    );
    let without_tools = starts(false);
    println!("tools off → {without_tools:?}");
    assert!(
        without_tools.is_empty(),
        "tools off: the request offers none, so none can run — got {without_tools:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_error_retry_setting_bounds_the_attempts() {
    // The **Error retry** knob is `LlmBackend::with_max_retries`, threaded
    // into every round's `retry::run_attempts`. Point a backend at a real host
    // that will refuse us (a bad key is a 401 — never retried) and then at one
    // whose transport fails, and count the `Retrying` announcements: the
    // budget the knob set is exactly what the driver spends.
    let model =
        std::env::var("ALTER_ZERO_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    let unreachable = ModelConfig {
        provider_id: "openrouter".to_string(),
        provider_name: "OpenRouter".to_string(),
        model,
        // A host that resolves to nothing: the send fails, which IS retryable.
        api_base: "https://127.0.0.1:9/v1".to_string(),
        api_model_base: "https://127.0.0.1:9/v1".to_string(),
        api_key: Some("sk-does-not-matter".to_string()),
        temperature: None,
        thinking: None,
        vision: None,
        cache_key: None,
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    };
    let retries_for = |max: u32| {
        let backend = LlmBackend::configure(unreachable.clone(), None, false).with_max_retries(max);
        assert_eq!(backend.max_retries(), max);
        let context = vec![ContextMessage::new(ContextRole::User, "hi")];
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = backend.spawn("hi".to_string(), vec![], context, tx, CancelToken::new());
        let mut retries = 0;
        let mut errored = false;
        while let Some(event) = rx.blocking_recv() {
            match event {
                StreamEvent::Retrying { attempt, max } => {
                    println!("retrying {attempt}/{max}…");
                    retries += 1;
                }
                StreamEvent::Error(_) => errored = true,
                StreamEvent::StreamDone => break,
                _ => {}
            }
        }
        handle.join().expect("backend thread joins");
        assert!(errored, "an unreachable host surfaces the error");
        retries
    };
    assert_eq!(
        retries_for(0),
        0,
        "0 never retries — the error is immediate"
    );
    assert_eq!(
        retries_for(2),
        2,
        "the knob's budget is exactly what is spent"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_temperature_setting_rides_the_request() {
    // The **Temperature** knob rides `ModelConfig::temperature` into the
    // payload. A provider that rejected the parameter would fail the turn, so
    // a completed turn at each offered value is the proof it is accepted — and
    // `default` (None) sends none at all.
    let key =
        std::env::var("OPENROUTER_API_KEY").expect("set OPENROUTER_API_KEY to run the live tests");
    let model =
        std::env::var("ALTER_ZERO_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    for temperature in [None, Some(0.0), Some(1.0)] {
        let cfg = ModelConfig {
            provider_id: "openrouter".to_string(),
            provider_name: "OpenRouter".to_string(),
            model: model.clone(),
            api_base: "https://openrouter.ai/api/v1".to_string(),
            api_model_base: "https://openrouter.ai/api/v1".to_string(),
            api_key: Some(key.clone()),
            temperature,
            thinking: None,
            vision: None,
            cache_key: None,
            extra_headers: Vec::new(),
            extra_body: serde_json::Map::new(),
        };
        let backend = LlmBackend::configure(cfg, None, false);
        let prompt = "Reply with exactly: OK";
        let context = vec![ContextMessage::new(ContextRole::User, prompt)];
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
        let mut reply = String::new();
        while let Some(event) = rx.blocking_recv() {
            match event {
                StreamEvent::Chunk(c) => reply.push_str(&c),
                StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
                StreamEvent::Error(e) => panic!("temperature {temperature:?} was rejected: {e}"),
                StreamEvent::StreamDone => break,
                _ => {}
            }
        }
        handle.join().expect("backend thread joins");
        println!("temperature {temperature:?} → {reply:?}");
        assert!(
            !reply.trim().is_empty(),
            "temperature {temperature:?} produced a reply"
        );
    }
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_max_tool_calls_bounds_a_parallel_batch() {
    // The reported bug (docs/settings.md): the ceiling counted tool *rounds*,
    // and a real model answers a "run these five commands" prompt with one
    // PARALLEL BATCH of five calls — so a cap of 3 ran all five. Ask a live
    // model for more calls than the budget and assert it never exceeds it.
    let key =
        std::env::var("OPENROUTER_API_KEY").expect("set OPENROUTER_API_KEY to run the live tests");
    let model =
        std::env::var("ALTER_ZERO_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    let cfg = ModelConfig {
        provider_id: "openrouter".to_string(),
        provider_name: "OpenRouter".to_string(),
        model,
        api_base: "https://openrouter.ai/api/v1".to_string(),
        api_model_base: "https://openrouter.ai/api/v1".to_string(),
        api_key: Some(key),
        temperature: Some(0.0),
        thinking: None,
        vision: None,
        cache_key: None,
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    };
    const CAP: usize = 3;
    let backend = LlmBackend::configure(cfg, None, true).with_max_tool_calls(CAP);
    assert_eq!(backend.max_tool_calls(), CAP);

    let prompt = "Use the bash tool to run each of these six commands, one tool call per \
                  command: `echo a`, `echo b`, `echo c`, `echo d`, `echo e`, `echo f`. \
                  Then report every output.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let mut started = 0usize;
    let mut succeeded = 0usize;
    let mut refused = 0usize;
    let mut error = None;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::ToolStart { .. } => started += 1,
            StreamEvent::ToolEnd { ok, output, .. } => {
                if ok {
                    succeeded += 1;
                } else if output.contains("tool-call limit") {
                    refused += 1;
                }
            }
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => {
                error = Some(e);
                break;
            }
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("started={started} ran={succeeded} refused={refused} error={error:?}");
    assert!(
        succeeded <= CAP,
        "a cap of {CAP} must never RUN more than {CAP} commands, but {succeeded} ran"
    );
    // The model was asked for six, so the budget must actually have bitten.
    assert!(
        refused > 0 || error.is_some(),
        "the turn should have hit the ceiling — got {started} starts with no refusal or error"
    );
    if let Some(e) = error {
        assert!(
            e.contains(&format!("{CAP} tool calls")),
            "the limit error names the ceiling: {e}"
        );
    }
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_ask_user_question_round_trip() {
    // The `AskUserQuestion` tool end to end (docs/ask.md): the backend offers
    // the tool (an AskGate is attached), the model calls it, the AskUser
    // request reaches the channel, the test answers on the gate as the modal
    // would, and the resolution comes back as ToolAnswered — the green cell's
    // display + the answers JSON — before the model's closing reply uses the
    // answer.
    let gate = alter_zero::ask::AskGate::new();
    let backend = backend().with_ask(gate.clone());
    let context = vec![ContextMessage::new(
        ContextRole::User,
        "Use the askuserquestion tool to ask me ONE question: which greeting \
         style I prefer, options \"Formal\" and \"Casual\". After I answer, \
         reply with exactly the label I chose and nothing else.",
    )];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(
        "ask away".to_string(),
        vec![],
        context,
        tx,
        CancelToken::new(),
    );
    let mut answered: Option<(String, String)> = None;
    let mut asked_questions = 0usize;
    let mut reply = String::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::AskUser(request) => {
                asked_questions = request.questions.len();
                println!("asked: {:?}", request.questions);
                assert!(!request.questions.is_empty());
                gate.resolve(
                    &request.id,
                    alter_zero::ask::AskDecision::Submitted(vec![alter_zero::ask::AskAnswer {
                        question: request.questions[0].question.clone(),
                        labels: vec!["Casual".to_string()],
                        notes: None,
                        preview: None,
                    }]),
                );
            }
            StreamEvent::ToolAnswered {
                display, result, ..
            } => {
                answered = Some((display, result));
            }
            StreamEvent::Chunk(chunk) => reply.push_str(&chunk),
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    assert!(asked_questions >= 1, "the model used the ask tool");
    let (display, result) = answered.expect("the submission resolved the call");
    println!("display:\n{display}\nresult: {result}\nreply: {reply}");
    assert!(display.starts_with("User answered Alter Zero's questions:"));
    assert!(display.contains("→ Casual"));
    let value: serde_json::Value = serde_json::from_str(&result).expect("the answers JSON");
    assert!(value["answers"].is_object());
    assert!(
        reply.to_lowercase().contains("casual"),
        "the model read the answer: {reply}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_ask_user_question_decline_stops_the_model() {
    // A decline resolves the call red with the stop-and-wait result — the
    // model must not re-ask in the same turn; it acknowledges and ends.
    let gate = alter_zero::ask::AskGate::new();
    let backend = backend().with_ask(gate.clone());
    let context = vec![ContextMessage::new(
        ContextRole::User,
        "Use the askuserquestion tool to ask me which color I like, options \
         \"Red\" and \"Blue\".",
    )];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(
        "ask away".to_string(),
        vec![],
        context,
        tx,
        CancelToken::new(),
    );
    let mut asks = 0usize;
    let mut rejected = None;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::AskUser(request) => {
                asks += 1;
                gate.resolve(&request.id, alter_zero::ask::AskDecision::Declined);
            }
            StreamEvent::ToolRejected {
                display, result, ..
            } => {
                rejected = Some((display, result));
            }
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    assert_eq!(asks, 1, "declining must not trigger an immediate re-ask");
    let (display, result) = rejected.expect("the decline resolved the call");
    println!("display:\n{display}\nresult: {result}");
    assert!(display.starts_with("User declined to answer questions"));
    assert!(result.contains("STOP"));
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_model_plans_through_the_task_tools() {
    // The whole task-tool path against a real model (docs/task-tools.md): the
    // four specs are offered because a registry is attached, `run_agent`
    // resolves each call as a `TaskCall` (never the visible ToolStart/ToolEnd
    // pair), the shared store ends up holding the plan the model made, and
    // every snapshot rides its own event so the strip's checklist can follow.
    let registry = alter_zero::tasks::TaskRegistry::new();
    let backend = backend().with_tasks(registry.clone());
    let prompt = "Plan a three-step release checklist with the task tools: \
                  create three tasks, then mark the first one in_progress. \
                  Use only the task tools — no shell commands, no file edits.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let mut calls: Vec<(String, String, String, bool)> = Vec::new();
    let mut last_snapshot = None;
    let mut visible_task_cells = 0usize;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::TaskCall {
                name,
                arguments,
                output,
                ok,
                tasks,
                ..
            } => {
                calls.push((name, arguments, output, ok));
                last_snapshot = Some(tasks);
            }
            // A task call must never surface as an ordinary tool cell.
            StreamEvent::ToolStart { name, .. } if name.to_lowercase().starts_with("task") => {
                visible_task_cells += 1;
            }
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    for (name, arguments, output, ok) in &calls {
        println!("{name}({arguments}) -> ok={ok} {output}");
    }
    assert_eq!(visible_task_cells, 0, "task calls render no tool cell");
    assert!(
        calls.iter().filter(|(n, ..)| n == "TaskCreate").count() >= 3,
        "the model planned with taskcreate: {calls:?}"
    );
    assert!(
        calls.iter().all(|(.., ok)| *ok),
        "a live call hit an error the executor should have prevented: {calls:?}"
    );
    // Every argument blob the model sent parses — and, because the create
    // schema's dependency fields are honoured rather than dropped, a model
    // that folds `addBlockedBy` into a create gets the edge it asked for.
    for (name, arguments, ..) in &calls {
        serde_json::from_str::<serde_json::Value>(arguments)
            .unwrap_or_else(|e| panic!("{name} sent unparseable arguments {arguments}: {e}"));
    }
    let snapshot = last_snapshot.expect("at least one call carried a snapshot");
    assert!(snapshot.tasks().len() >= 3, "the plan is in the snapshot");
    assert_eq!(
        registry.snapshot().tasks().len(),
        snapshot.tasks().len(),
        "the shared store the model's next tasklist reads agrees with the strip"
    );
    println!(
        "final list: {:?}",
        snapshot
            .tasks()
            .iter()
            .map(|t| format!("#{} [{}] {}", t.id, t.status.wire(), t.subject))
            .collect::<Vec<_>>()
    );
}

// ===== lifecycle hooks (docs/hooks.md) =====

/// A backend with a real `hooks.json` attached, running handlers in `dir`.
fn backend_with_hooks(hooks_json: &str, dir: &std::path::Path) -> LlmBackend {
    let file = alter_zero::hooks::HooksFile::parse(hooks_json).expect("the fixture parses");
    let context = alter_zero::hooks::HookContext {
        session_id: "live-test".to_string(),
        transcript_path: None,
        cwd: dir.display().to_string(),
        model: "live".to_string(),
        permission_mode: Some("master".to_string()),
        agent_id: None,
        agent_type: None,
    };
    let sink = alter_zero::llm::hooks::CommandHooks::new(
        std::sync::Arc::new(file),
        context,
        None,
        dir.to_path_buf(),
        alter_zero::llm::hooks::HookHandles::default(),
    )
    .expect("the fixture has a runnable handler");
    let model =
        std::env::var("ALTER_ZERO_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    backend_for(model, None).with_hooks(std::sync::Arc::new(sink))
}

/// Drive one live turn through `backend`, returning every streamed event.
fn events_from(backend: &LlmBackend, prompt: &str) -> Vec<StreamEvent> {
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let mut events = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        let done = matches!(event, StreamEvent::StreamDone);
        if let StreamEvent::Error(e) = &event {
            panic!("backend error: {e}");
        }
        events.push(event);
        if done {
            break;
        }
    }
    handle.join().expect("backend thread joins");
    events
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_a_pre_tool_use_hook_blocks_a_real_models_bash_call() {
    // The whole contract end to end against a real model: the hook reads the
    // payload on stdin, finds the command it objects to in `tool_input`, and
    // exits 2 with the reason on stderr. The call must never run, and the
    // model must be told why.
    let dir = tempfile::tempdir().expect("temp dir");
    let marker = dir.path().join("should-not-exist.txt");
    let hooks = r#"{"hooks":{"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command",
        "command":"grep -q should-not-exist && { echo 'writing that file is forbidden' >&2; exit 2; }; exit 0",
        "timeout":30}]}]}}"#;
    let backend = backend_with_hooks(hooks, dir.path());
    let events = events_from(
        &backend,
        &format!(
            "Use your bash tool, in one call, to run exactly: touch {}",
            marker.display()
        ),
    );

    let rejected: Vec<&StreamEvent> = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ToolRejected { .. }))
        .collect();
    assert!(
        !rejected.is_empty(),
        "the hook refused the call: {events:#?}"
    );
    let StreamEvent::ToolRejected {
        display, result, ..
    } = rejected[0]
    else {
        unreachable!()
    };
    println!("cell: {display}\nmodel: {result}");
    assert!(
        display.contains("writing that file is forbidden"),
        "the cell carries the hook's stderr: {display}"
    );
    assert!(
        result.contains("writing that file is forbidden"),
        "and so does what the model reads: {result}"
    );
    assert!(
        !marker.exists(),
        "a blocked call must never have run the command"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_a_post_tool_use_hook_feeds_the_model_extra_context() {
    // The hook reads the finished call's `tool_response.output` and hands the
    // model a fact it could not otherwise know — which the model then repeats,
    // proving the context genuinely reached the next round.
    let dir = tempfile::tempdir().expect("temp dir");
    let hooks = r#"{"hooks":{"PostToolUse":[{"matcher":"bash","hooks":[{"type":"command",
        "command":"printf '%s' '{\"hookSpecificOutput\":{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"NOTE: the build id is ZX-4417.\"}}'",
        "timeout":30}]}]}}"#;
    let backend = backend_with_hooks(hooks, dir.path());
    let events = events_from(
        &backend,
        "Run `echo ok` with your bash tool, then tell me the build id.",
    );

    let answered = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::ToolAnswered {
                display, result, ..
            } => Some((display, result)),
            _ => None,
        })
        .expect("the hook's context rides the two-text split");
    assert!(
        answered.1.contains("ZX-4417"),
        "the model-facing text carries the hook's context: {}",
        answered.1
    );
    assert!(
        !answered.0.contains("ZX-4417"),
        "the cell stays the command's own output: {}",
        answered.0
    );

    let reply: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::Chunk(c) => Some(c.as_str()),
            _ => None,
        })
        .collect();
    println!("reply: {reply}");
    assert!(
        reply.contains("ZX-4417"),
        "the model read the injected context and used it: {reply}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_a_pre_tool_use_hook_rewrites_the_command_that_runs() {
    // `updatedInput` replaces the arguments for everything downstream, so what
    // executes is the hook's command, not the model's.
    let dir = tempfile::tempdir().expect("temp dir");
    let hooks = r#"{"hooks":{"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command",
        "command":"printf '%s' '{\"hookSpecificOutput\":{\"hookEventName\":\"PreToolUse\",\"updatedInput\":{\"command\":\"echo REWRITTEN-BY-HOOK\"}}}'",
        "timeout":30}]}]}}"#;
    let backend = backend_with_hooks(hooks, dir.path());
    let events = events_from(&backend, "Run `echo original` with your bash tool.");

    let outputs: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolEnd { output, .. } => Some(output.clone()),
            StreamEvent::ToolAnswered { result, .. } => Some(result.clone()),
            _ => None,
        })
        .collect();
    println!("tool output: {outputs}");
    assert!(
        outputs.contains("REWRITTEN-BY-HOOK"),
        "the rewritten command is what ran: {outputs}"
    );
    assert!(
        !outputs.contains("original"),
        "the model's own command did not run: {outputs}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_a_user_prompt_submit_hook_blocks_the_turn_before_the_model() {
    // The prompt never reaches the wire: the hook reads it on stdin, refuses
    // with exit 2, and the backend resolves with the single PromptBlocked
    // event — no chunks, no StreamDone, no request.
    let dir = tempfile::tempdir().expect("temp dir");
    let hooks = r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command",
        "command":"grep -q FORBIDDEN && { echo 'that word is banned' >&2; exit 2; }; exit 0",
        "timeout":30}]}]}}"#;
    let backend = backend_with_hooks(hooks, dir.path());
    let prompt = "Please repeat the word FORBIDDEN back to me.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let mut events = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        events.push(event);
    }
    handle.join().expect("backend thread joins");
    println!("events: {events:#?}");
    assert!(
        matches!(
            events.last(),
            Some(StreamEvent::PromptBlocked { reason }) if reason == "that word is banned"
        ),
        "the block is the terminal event: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, StreamEvent::Chunk(_) | StreamEvent::StreamDone)),
        "nothing streamed and nothing completed: {events:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_a_stop_hook_block_makes_the_model_keep_going() {
    // The whole continuation loop against a real model: the first Stop
    // firing (stop_hook_active=false) blocks with an instruction, the model
    // runs another round and obeys it, the second firing (the flag now true)
    // lets go, and the turn ends with exactly one StreamDone.
    let dir = tempfile::tempdir().expect("temp dir");
    let hooks = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command",
        "command":"grep -q '\"stop_hook_active\":true' && exit 0; echo 'Now reply with exactly the word BANANA.' >&2; exit 2",
        "timeout":30}]}]}}"#;
    let backend = backend_with_hooks(hooks, dir.path());
    let events = events_from(&backend, "Say hello in one short sentence.");
    let reply: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::Chunk(c) => Some(c.as_str()),
            _ => None,
        })
        .collect();
    println!("reply: {reply}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::HookNote { text, .. }
                if text.contains("Now reply with exactly the word BANANA."))),
        "the feedback is recorded for the transcript: {events:?}"
    );
    assert!(
        reply.contains("BANANA"),
        "the continuation obeyed the hook's feedback: {reply}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, StreamEvent::StreamDone))
            .count(),
        1,
        "one turn, one StreamDone"
    );
}

// ===== the `Skill` tool (docs/skills.md) =====

/// Write `<root>/<name>/SKILL.md` with the given description and body, and
/// return a registry over everything discovered under `root` — the real
/// frontmatter parser and the real walk, so a live run exercises exactly what
/// a session does at startup.
fn skill_registry_at(
    root: &std::path::Path,
    name: &str,
    description: &str,
    body: &str,
) -> alter_zero::skills::SkillRegistry {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("skill dir");
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"),
    )
    .expect("SKILL.md");
    let (skills, errors) = alter_zero::llm::skill::discover_skills(&[root.to_path_buf()]);
    assert!(errors.is_empty(), "the fixture parses: {errors:?}");
    alter_zero::skills::SkillRegistry::new(skills)
}

/// A tools-enabled backend carrying `registry`, plus the `<system-reminder>`
/// listing the model picks a skill name out of — the two halves that have to
/// agree, assembled the way `tui::models` assembles them.
fn skill_backend_and_context(
    registry: &alter_zero::skills::SkillRegistry,
    prompt: &str,
) -> (LlmBackend, Vec<ContextMessage>) {
    let key =
        std::env::var("OPENROUTER_API_KEY").expect("set OPENROUTER_API_KEY to run the live tests");
    let model =
        std::env::var("ALTER_ZERO_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    let cfg = ModelConfig {
        provider_id: "openrouter".to_string(),
        provider_name: "OpenRouter".to_string(),
        model,
        api_base: "https://openrouter.ai/api/v1".to_string(),
        api_model_base: "https://openrouter.ai/api/v1".to_string(),
        api_key: Some(key),
        temperature: Some(0.0),
        thinking: None,
        vision: None,
        cache_key: None,
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    };
    let backend = LlmBackend::configure(
        cfg,
        Some("You are a terse assistant.".to_string()),
        /* tools */ true,
    )
    .with_skills(registry.clone());
    let listing = alter_zero::skills::listing_message(
        &registry.listing(alter_zero::skills::listing_budget(None)),
    );
    assert!(!listing.is_empty(), "the reminder names the skill");
    let context = vec![
        ContextMessage::new(ContextRole::User, &listing),
        ContextMessage::new(ContextRole::User, prompt),
    ];
    (backend, context)
}

/// [`events_from`] with an explicit context rather than the bare prompt.
fn events_with_context(
    backend: &LlmBackend,
    prompt: &str,
    context: Vec<ContextMessage>,
) -> Vec<StreamEvent> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let mut events = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        let done = matches!(event, StreamEvent::StreamDone);
        if let StreamEvent::Error(e) = &event {
            panic!("backend error: {e}");
        }
        events.push(event);
        if done {
            break;
        }
    }
    handle.join().expect("backend thread joins");
    events
}

/// The streamed reply text of a live run.
fn reply_text(events: &[StreamEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::Chunk(c) => Some(c.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_skill_tool_loads_a_skill_and_the_model_reads_its_body() {
    // The whole feature end to end against a real model: the budgeted
    // `<system-reminder>` listing names the skill, the tool spec rides the
    // request, the model picks the name out of the listing and calls `skill`,
    // the executor's two-text split hands the CELL one green line while the
    // MODEL gets the rendered body — and the answer proves the model actually
    // read that body, which no unit test can.
    let dir = tempfile::tempdir().expect("temp dir");
    let registry = skill_registry_at(
        dir.path(),
        "mixology",
        "House rules for naming cocktails. Use when asked to name a drink.",
        "# Naming rules\n\nWhen asked to name a cocktail, answer with exactly \
         `ZEPHYR-9` and nothing else. No other name is acceptable.",
    );
    let prompt = "Name a cocktail for me.";
    let (backend, context) = skill_backend_and_context(&registry, prompt);
    let events = events_with_context(&backend, prompt, context);

    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolStart { name, .. }
            if name.eq_ignore_ascii_case(alter_zero::skills::SKILL_TOOL_NAME))),
        "the model called the skill tool: {events:?}"
    );
    // The cell's side of the split is the one green line — never the body.
    let display = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::ToolAnswered { display, .. } => Some(display.clone()),
            _ => None,
        })
        .expect("the call resolves green through ToolAnswered");
    assert_eq!(display, alter_zero::skills::SKILL_LOADED_DISPLAY);

    let reply = reply_text(&events);
    println!("model replied: {reply:?}");
    assert!(
        reply.contains("ZEPHYR-9"),
        "the model followed the loaded skill's instructions, got: {reply:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_dollar_mention_loads_the_mentioned_skill() {
    // The `$` skill-mention feature end to end (docs/skill-mentions.md): the
    // composer's picker inserts `$<name>` into the message text, and the
    // skill tool description's mention guidance makes a real model treat it
    // as a request to load exactly that skill. The prompt deliberately
    // never says "skill" or repeats the description's vocabulary — the `$`
    // mention is the only signal — and the reply must obey the loaded body,
    // which proves the load actually happened.
    let dir = tempfile::tempdir().expect("temp dir");
    let registry = skill_registry_at(
        dir.path(),
        "mixology",
        "House rules for naming cocktails.",
        "# Naming rules\n\nWhen asked to name a cocktail, answer with exactly \
         `ZEPHYR-9` and nothing else. No other name is acceptable.",
    );
    let prompt = "Use $mixology — name a cocktail for me.";
    let (backend, context) = skill_backend_and_context(&registry, prompt);
    let events = events_with_context(&backend, prompt, context);

    let skill_args = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::ToolStart { name, args, .. }
                if name.eq_ignore_ascii_case(alter_zero::skills::SKILL_TOOL_NAME) =>
            {
                Some(args.clone())
            }
            _ => None,
        })
        .expect("the $ mention made the model call the skill tool");
    assert!(
        skill_args.contains("mixology"),
        "the call names the mentioned skill: {skill_args:?}"
    );
    let reply = reply_text(&events);
    println!("model replied: {reply:?}");
    assert!(
        reply.contains("ZEPHYR-9"),
        "the model followed the mentioned skill's instructions, got: {reply:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_an_embedded_mention_still_loads_the_skill() {
    // The harder half of the mention contract (docs/skill-mentions.md): the
    // picker completes a mention *anywhere* in a message, so the common shape
    // is not an imperative like "use $mixology" but a name dropped
    // mid-sentence, with the surrounding prose asking for something else
    // entirely. Nothing here tells the model to load anything — no "use", no
    // "skill", and the request reads as an ordinary question — so a reply of
    // `ZEPHYR-9` can only come from the mention having been honoured on its
    // own. This is the case that decides whether mention-as-text is a real
    // mechanism or a prompt trick that works on explicit phrasing.
    let dir = tempfile::tempdir().expect("temp dir");
    let registry = skill_registry_at(
        dir.path(),
        "mixology",
        "House rules for naming cocktails.",
        "# Naming rules\n\nWhen asked to name a cocktail, answer with exactly \
         `ZEPHYR-9` and nothing else. No other name is acceptable.",
    );
    let prompt = "I'm putting together a drinks menu $mixology and I need a name \
                  for the house gin cocktail.";
    let (backend, context) = skill_backend_and_context(&registry, prompt);
    let events = events_with_context(&backend, prompt, context);

    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolStart { name, .. }
            if name.eq_ignore_ascii_case(alter_zero::skills::SKILL_TOOL_NAME))),
        "an embedded mention still loads the skill: {events:?}"
    );
    let reply = reply_text(&events);
    println!("model replied: {reply:?}");
    assert!(
        reply.contains("ZEPHYR-9"),
        "the model answered from the mentioned skill's body, got: {reply:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_skill_arguments_are_substituted_before_the_model_sees_them() {
    // `$ARGUMENTS` is expanded by the *executor*, not the model
    // (docs/skills.md) — so the check that matters is on the tool result the
    // model was handed, which is deterministic. The reply is the second half:
    // the substituted body reached it and was legible.
    let dir = tempfile::tempdir().expect("temp dir");
    let registry = skill_registry_at(
        dir.path(),
        "greet",
        "Greet a person by name. Use when asked to greet someone.",
        "# Greeting\n\nReply with exactly `HELLO-$ARGUMENTS` and nothing else.",
    );
    let prompt = "Use the greet skill with the argument Bob.";
    let (backend, context) = skill_backend_and_context(&registry, prompt);
    let events = events_with_context(&backend, prompt, context);

    let result = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::ToolAnswered { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("the call resolves through ToolAnswered");
    assert!(
        result.contains("HELLO-Bob"),
        "the executor substituted $ARGUMENTS before the model saw the body: {result:?}"
    );
    assert!(
        !result.contains("$ARGUMENTS"),
        "…and left no placeholder behind: {result:?}"
    );
    let reply = reply_text(&events);
    println!("model replied: {reply:?}");
    assert!(
        reply.contains("HELLO-Bob"),
        "the model read the substituted body, got: {reply:?}"
    );
}

/// The MCP surface end to end against the public DeepWiki server
/// (`docs/mcp.md`): the manager connects over streamable HTTP, lists the
/// tools, `with_mcp` offers them under their `mcp__deepwiki__…` wire names,
/// and a real turn's call routes through the live connection — the model
/// reading the server's answer and the recorded events carrying the
/// `{server} - {tool} (MCP)` display + raw-JSON args split the cells render
/// from.
#[test]
#[ignore]
fn live_mcp_deepwiki_tool_call_round_trips() {
    use alter_zero::llm::mcp::{McpManager, McpSources};
    use alter_zero::mcp::{McpScope, McpServerConfig, McpServerEntry, McpServerStatus};

    let (mcp_tx, _mcp_rx) = tokio::sync::mpsc::unbounded_channel();
    let manager = McpManager::new(
        mcp_tx,
        McpSources {
            entries: vec![McpServerEntry {
                name: "deepwiki".to_string(),
                config: McpServerConfig::Http {
                    url: "https://mcp.deepwiki.com/mcp".to_string(),
                    headers: Default::default(),
                    sse_fallback: false,
                },
                scope: McpScope::User,
                config_path: "~/.alter-zero/mcp.json".to_string(),
            }],
            project: "/live".to_string(),
            ..Default::default()
        },
    );
    manager.start_connections();
    for _ in 0..240 {
        match &manager.snapshot()[0].status {
            McpServerStatus::Connected => break,
            McpServerStatus::Failed(e) => panic!("deepwiki connect failed: {e}"),
            _ => std::thread::sleep(std::time::Duration::from_millis(250)),
        }
    }
    let snapshot = manager.snapshot();
    assert_eq!(snapshot[0].status, McpServerStatus::Connected, "timed out");
    assert!(
        snapshot[0].tools.iter().any(|t| t.name == "ask_question"),
        "deepwiki lists ask_question: {:?}",
        snapshot[0]
            .tools
            .iter()
            .map(|t| &t.name)
            .collect::<Vec<_>>()
    );
    let specs = manager.tool_specs();
    assert!(
        specs
            .iter()
            .any(|s| s["function"]["name"] == "mcp__deepwiki__ask_question"),
        "the wire names ride the offered specs"
    );

    // A real turn: the model must call the MCP tool and read its answer.
    let backend = backend().with_mcp(manager.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(
        "Use the deepwiki read_wiki_structure tool on the repository \
         linuztx/flaredantic, then answer in one short sentence: what is the \
         repository about?"
            .to_string(),
        Vec::new(),
        Vec::new(),
        tx,
        CancelToken::new(),
    );
    let mut text = String::new();
    let mut tool_start: Option<(String, String)> = None;
    let mut tool_output: Option<String> = None;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(chunk) => text.push_str(&chunk),
            StreamEvent::ToolStart { name, args, .. } => tool_start = Some((name, args)),
            StreamEvent::ToolEnd { output, ok, .. } => {
                assert!(ok, "the MCP call failed: {output}");
                tool_output = Some(output);
            }
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread");
    manager.shutdown();
    let (name, args) = tool_start.expect("the model called the MCP tool");
    // The display/args split the cells render from (docs/mcp.md). The model
    // is free to pick either deepwiki tool — the surface under test is the
    // MCP plumbing, not its choice.
    assert!(
        name.starts_with("deepwiki - ") && name.ends_with(" (MCP)"),
        "the display name wears the Claude Code shape: {name}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&args).expect("raw JSON args");
    assert_eq!(parsed["repoName"], "linuztx/flaredantic");
    let output = tool_output.expect("the call resolved");
    assert!(
        !output.trim().is_empty(),
        "the server's answer came back: {output}"
    );
    println!("reply: {text}");
    assert!(!text.trim().is_empty(), "the model answered after the call");
}
