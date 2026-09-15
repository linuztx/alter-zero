//! Live prompt-caching verification, one test per provider
//! (`docs/prompt-caching.md`): the full production pipeline — the provider's
//! config resolved through `ProvidersFile::model_config` exactly as the
//! boundary resolves it, `LlmBackend::spawn`, the real wire — driven twice
//! over one salted ~5k-token system prompt, so turn 2 re-sends a prefix turn 1
//! taught the provider. Each test prints both usage frames and asserts what
//! the provider's own accounting reports: cache **writes** on turn 1 where the
//! provider bills them, cache **reads** on turn 2 where it reports them, and
//! — for a provider that reports neither — that the usage frame still lands
//! whole, so the app's tally is the provider's number rather than the
//! tokenizer's guess.
//!
//! All `#[ignore]`d so `cargo test` stays offline and deterministic. Run on
//! purpose with real credentials, never committed — read from the
//! environment:
//!
//! ```sh
//! A0_VENICE_API_KEY=sk-a0-…    cargo test --test live_caching -- --ignored --nocapture live_venice
//! VENICE_API_KEY=…             cargo test --test live_caching -- --ignored --nocapture live_venice_direct
//! OPENROUTER_API_KEY=sk-or-…   cargo test --test live_caching -- --ignored --nocapture live_openrouter
//! ANTHROPIC_API_KEY=sk-ant-…   cargo test --test live_caching -- --ignored --nocapture live_anthropic_api_key
//! OLLAMA_API_KEY=…             cargo test --test live_caching -- --ignored --nocapture live_ollama_cloud
//! GITHUB_COPILOT_TOKEN=ghu_…   cargo test --test live_caching -- --ignored --nocapture live_copilot
//! ANTHROPIC_CONSOLE_REFRESH_TOKEN=sk-ant-ort01-… ALTER_ZERO_LIVE_TOKEN_STORE=/tmp/live.env \
//!                              cargo test --test live_caching -- --ignored --nocapture live_anthropic_console
//! OPENAI_CHATGPT_REFRESH_TOKEN=rt.1.… ALTER_ZERO_LIVE_TOKEN_STORE=/tmp/live.env \
//!                              cargo test --test live_caching -- --ignored --nocapture live_chatgpt
//! ```
//!
//! `live_chatgpt_cache_diagnostics` is the A/B run that found the ChatGPT
//! backend keys its cache on Codex's `session_id`/`conversation_id` headers
//! rather than the body's `prompt_cache_key` (`docs/chatgpt.md`); it prints
//! rather than asserts, so it stays useful when the backend drifts again.
//!
//! The two sign-ins whose refresh token **rotates** (Anthropic Console,
//! ChatGPT — `docs/claude.md`, `docs/chatgpt.md`) refuse to run without
//! `ALTER_ZERO_LIVE_TOKEN_STORE`: the mint may retire the token it was given,
//! and the rotated one is written back only to a store path, so a run without
//! one would strand the caller with a token the provider no longer honours.
//! After such a run, the value under that file's `*_REFRESH_TOKEN` key is the
//! one to keep.
//!
//! `ALTER_ZERO_LIVE_VENICE_MODEL` (the proxy and the direct provider alike),
//! `ALTER_ZERO_LIVE_VENICE_CLAUDE_MODEL`,
//! `ALTER_ZERO_LIVE_OPENROUTER_MODEL`, `ALTER_ZERO_LIVE_ANTHROPIC_MODEL`,
//! `ALTER_ZERO_LIVE_CHATGPT_MODEL`, `ALTER_ZERO_LIVE_OLLAMA_CLOUD_MODEL` and
//! `ALTER_ZERO_LIVE_COPILOT_MODEL` override each provider's default model
//! (their catalogs churn).

use alter_zero::context::{ContextMessage, ContextRole};
use alter_zero::llm::models::fetch_models;
use alter_zero::llm::{LlmBackend, ModelConfig, ProvidersFile, Selection};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent, TokenUsage};

/// A per-run salt: the system prompt and the cache-affinity key both carry
/// it, so a rerun can never hit a previous run's still-warm cache and read a
/// stale write as this run's.
fn salt() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A deterministic ~5k-token system prompt — past every provider's minimum
/// cacheable size (Anthropic's largest is 4096 tokens, for Haiku 4.5 and
/// Opus 4.5), unique per `salt`.
fn big_system_prompt(salt: u64) -> String {
    let corpus: String = (0..420)
        .map(|i| format!("Calibration sentence {i} of the standing corpus, run {salt}. "))
        .collect();
    format!("You are a terse assistant. Answer in as few words as possible.\n{corpus}")
}

/// The provider's config resolved the way `tui::config::model_config_for`
/// resolves it — through the built-in `providers.toml`, with the session's
/// cache-affinity key — so the request under test is the request the app
/// sends.
fn config(provider_id: &str, model: &str, key: Option<String>, salt: u64) -> ModelConfig {
    ProvidersFile::builtin()
        .model_config(&Selection {
            provider_id: provider_id.to_string(),
            model: model.to_string(),
            api_key: key,
            temperature: None,
            thinking: None,
            service_tier: None,
            vision: None,
            context: None,
            api_base: None,
            cache_key: Some(format!("alter-zero-live-cache-{salt}")),
        })
        .unwrap_or_else(|| panic!("{provider_id} is a built-in provider"))
}

/// The environment variable a provider's credential lives under, read the
/// way the boundary reads it. Panics with a clear message when it is unset —
/// these tests only ever run on purpose.
fn credential(var: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| panic!("set {var} to run this live test"))
}

/// A model override from the environment, else `default`.
fn model_or(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.to_string())
}

/// Run one plain turn through `backend`, collecting the reply text and every
/// usage frame. Panics on a backend error.
fn complete_with_usage(backend: &LlmBackend, prompt: &str) -> (String, Vec<TokenUsage>) {
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

/// Two identical single-round turns on `backend`, each's one usage frame.
/// The second re-sends the first's whole prefix, which is the shape every
/// agentic round has.
fn two_turns(backend: &LlmBackend) -> (TokenUsage, TokenUsage) {
    let (reply, usages) = complete_with_usage(backend, "Just say ONE.");
    println!("turn 1 reply: {reply:?}, usage: {usages:?}");
    let first = *usages.first().expect("turn 1 reported usage");
    let (reply, usages) = complete_with_usage(backend, "Just say ONE.");
    println!("turn 2 reply: {reply:?}, usage: {usages:?}");
    let second = *usages.first().expect("turn 2 reported usage");
    println!(
        "receipt: turn 1 → {} tokens ({} cached, {} written) · turn 2 → {} tokens ({} cached, {} written)",
        first.total(),
        first.cached,
        first.cache_write,
        second.total(),
        second.cached,
        second.cache_write
    );
    (first, second)
}

/// A raw HTTP client for the frame-level probes, trusting the same CA bundle
/// the crate's own client loads at runtime (`SSL_CERT_FILE` — an agent proxy
/// re-terminates TLS with its own root). A plain `Client::new()` would refuse
/// the proxy's certificate where the backend under test accepted it.
fn raw_client() -> reqwest::blocking::Client {
    let mut builder = reqwest::blocking::Client::builder();
    if let Some(bundle) = std::env::var_os("SSL_CERT_FILE")
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|pem| reqwest::Certificate::from_pem_bundle(&pem).ok())
    {
        for cert in bundle {
            builder = builder.add_root_certificate(cert);
        }
    }
    builder.build().expect("a raw client")
}

/// The store path the rotating sign-ins write a rotated refresh token back
/// to. Required for those tests — see the module docs.
fn token_store() -> std::path::PathBuf {
    std::env::var_os("ALTER_ZERO_LIVE_TOKEN_STORE")
        .map(std::path::PathBuf::from)
        .expect(
            "set ALTER_ZERO_LIVE_TOKEN_STORE to a file the rotated refresh token can be written to",
        )
}

// ---------------------------------------------------------------------------
// Venice (implicit caching; Claude ids get Venice's own markers)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_venice_second_turn_reads_the_prefix_from_cache() {
    // Venice caches implicitly — no breakpoints — and honours
    // `prompt_cache_key`; its usage frame carries the cached detail under
    // `prompt_tokens_details` / the top-level Anthropic-style aliases.
    let salt = salt();
    let model = model_or(
        "ALTER_ZERO_LIVE_VENICE_MODEL",
        "openai-gpt-4o-mini-2024-07-18",
    );
    let cfg = config(
        "a0_venice",
        &model,
        Some(credential("A0_VENICE_API_KEY")),
        salt,
    );
    let backend = LlmBackend::configure(cfg, Some(big_system_prompt(salt)), false);
    let (first, second) = two_turns(&backend);
    assert!(first.total() > 1_000, "the whole prefix billed: {first:?}");
    assert!(
        second.cached > 1_000,
        "turn 2 read the prefix from Venice's cache: {second:?}"
    );
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY; costs a few cents"]
fn live_venice_claude_caches_without_our_own_breakpoints() {
    // The one claim `cache::needs_cache_breakpoints` makes about Venice: its
    // bare `claude-*` ids get the markers from Venice itself, so the crate
    // sends none (doubling them could pass Anthropic's four-breakpoint
    // limit). If Venice ever stopped injecting them, this is the test that
    // would say so — turn 1 must report a cache write and turn 2 a read with
    // the crate's marker-free request.
    let salt = salt();
    let model = model_or("ALTER_ZERO_LIVE_VENICE_CLAUDE_MODEL", "claude-sonnet-4-6");
    let cfg = config(
        "a0_venice",
        &model,
        Some(credential("A0_VENICE_API_KEY")),
        salt,
    );
    assert!(
        !alter_zero::llm::cache::needs_cache_breakpoints(&cfg.model),
        "a bare Venice claude id is left to Venice's own markers"
    );
    let backend = LlmBackend::configure(cfg, Some(big_system_prompt(salt)), false);
    let (first, second) = two_turns(&backend);
    assert!(
        first.cache_write > 1_000 || first.cached > 1_000,
        "turn 1 wrote (or already read) the prefix: {first:?}"
    );
    assert!(
        second.cached > 1_000,
        "turn 2 read the prefix back from the cache: {second:?}"
    );
}

#[test]
#[ignore = "hits the network; needs VENICE_API_KEY"]
fn live_venice_direct_second_turn_reads_the_prefix_from_cache() {
    // The direct provider (`docs/venice.md`) is the proxy's twin: the same
    // request — `venice_parameters`, `prompt_cache_key`, no markers of ours —
    // sent to Venice's own base with a key from the user's own account. What
    // the proxy test proves about Venice's implicit cache must hold here
    // without the proxy in between: turn 2 reads the prefix back.
    let salt = salt();
    let model = model_or(
        "ALTER_ZERO_LIVE_VENICE_MODEL",
        "openai-gpt-4o-mini-2024-07-18",
    );
    let cfg = config("venice", &model, Some(credential("VENICE_API_KEY")), salt);
    assert_eq!(cfg.api_base, "https://api.venice.ai/api/v1");
    let backend = LlmBackend::configure(cfg, Some(big_system_prompt(salt)), false);
    let (first, second) = two_turns(&backend);
    assert!(first.total() > 1_000, "the whole prefix billed: {first:?}");
    assert!(
        second.cached > 1_000,
        "turn 2 read the prefix from Venice's cache: {second:?}"
    );
}

// ---------------------------------------------------------------------------
// OpenRouter (explicit caching for the Anthropic routing; `~` alias ids)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY; costs about a cent"]
fn live_openrouter_latest_alias_id_gets_breakpoints_and_caches() {
    // OpenRouter's `~vendor/model-latest` alias ids resolve to the vendor's
    // newest model (`~anthropic/claude-haiku-latest` → `anthropic/claude-haiku-4.5`
    // on Amazon Bedrock, per the response's own `model` field). The Anthropic
    // routing caches nothing without `cache_control` breakpoints, so an alias
    // the sniff did not recognise paid full price on every agentic round.
    // Turn 1 must write the prefix, turn 2 must read it.
    let salt = salt();
    let model = model_or(
        "ALTER_ZERO_LIVE_OPENROUTER_MODEL",
        "~anthropic/claude-haiku-latest",
    );
    let mut cfg = config(
        "openrouter",
        &model,
        Some(credential("OPENROUTER_API_KEY")),
        salt,
    );
    // OpenRouter reserves the model's whole output ceiling against the
    // account's credits before it runs a request (a 402 names the number);
    // a one-word answer needs none of it.
    cfg.extra_body
        .insert("max_tokens".to_string(), serde_json::json!(32));
    assert!(
        alter_zero::llm::cache::needs_cache_breakpoints(&cfg.model),
        "{model} is an explicit-caching id"
    );
    let backend = LlmBackend::configure(cfg, Some(big_system_prompt(salt)), false);
    let (first, second) = two_turns(&backend);
    assert!(
        first.cache_write > 1_000,
        "turn 1 wrote the big prefix to the provider's cache: {first:?}"
    );
    assert!(
        second.cached > 1_000,
        "turn 2 read the prefix back from the cache: {second:?}"
    );
}

// ---------------------------------------------------------------------------
// Anthropic's own API (Messages wire; always explicit) — a pasted key on
// `x-api-key`, and the Console sign-in's OAuth bearer
// ---------------------------------------------------------------------------

/// The Messages-wire round trip both Anthropic providers share
/// (`docs/claude.md`): breakpoints on the last system block and the
/// conversation frontier, the usage summed from the uncached remainder plus
/// both cache figures. Turn 1 writes, turn 2 reads — and `input` on turn 2
/// is still the whole prompt, not the few dozen uncached tokens Anthropic
/// reports as `input_tokens`.
fn anthropic_writes_then_reads(provider_id: &str, key: String) {
    let salt = salt();
    let model = model_or("ALTER_ZERO_LIVE_ANTHROPIC_MODEL", "claude-haiku-4-5");
    let cfg = config(provider_id, &model, Some(key), salt);
    let backend = LlmBackend::configure(cfg.clone(), Some(big_system_prompt(salt)), false);
    let (first, second) = two_turns(&backend);
    assert!(
        first.cache_write > 1_000,
        "turn 1 wrote the prefix to Anthropic's cache: {first:?}"
    );
    assert!(
        second.cached > 1_000,
        "turn 2 read the prefix back: {second:?}"
    );
    assert!(
        second.input >= second.cached,
        "input is the whole prompt, cache reads included: {second:?}"
    );
    // What the wire actually carries on each frame, for the record: the
    // `message_start` and `message_delta` usage objects of a third,
    // cache-warm request. The merge in `anthropic::MessageAccumulator` must
    // survive whatever shape the delta takes.
    print_raw_anthropic_usage_frames(&cfg, &big_system_prompt(salt));
}

#[test]
#[ignore = "hits the network; needs ANTHROPIC_API_KEY; costs about a cent"]
fn live_anthropic_api_key_writes_then_reads_the_prefix() {
    anthropic_writes_then_reads("anthropic", credential("ANTHROPIC_API_KEY"));
}

#[test]
#[ignore = "hits the network; needs ANTHROPIC_CONSOLE_REFRESH_TOKEN + ALTER_ZERO_LIVE_TOKEN_STORE; costs about a cent"]
fn live_anthropic_console_writes_then_reads_the_prefix() {
    alter_zero::llm::claude::set_store_path(token_store());
    anthropic_writes_then_reads(
        "anthropic_console",
        credential("ANTHROPIC_CONSOLE_REFRESH_TOKEN"),
    );
}

/// One raw streamed Messages request, printing every frame's `usage` object.
/// Boundary-shaped test code: the credential rides exactly as the backend
/// sends it — a pasted key on `x-api-key`, an OAuth bearer (minted through
/// the same cached mint the backend used, so no second exchange) with the
/// beta header it needs — and the request is the smallest cache-warm one that
/// shows the input side.
fn print_raw_anthropic_usage_frames(cfg: &ModelConfig, system: &str) {
    use std::io::BufRead;
    let credential = cfg.api_key.as_deref().expect("a stored credential");
    let client = raw_client();
    let body = serde_json::json!({
        "model": cfg.model,
        "max_tokens": 16,
        "stream": true,
        "system": [{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}],
        "messages": [{"role": "user", "content": "Just say ONE."}],
    });
    let mut req = client
        .post(format!("{}/messages", cfg.api_base))
        .header("anthropic-version", "2023-06-01")
        .header("accept", "text/event-stream");
    req = match cfg.auth {
        alter_zero::llm::AuthScheme::AnthropicConsole => {
            let access =
                alter_zero::llm::claude::authorize(credential).expect("the mint the backend used");
            req.bearer_auth(access.bearer)
                .header("anthropic-beta", alter_zero::llm::claude::OAUTH_BETA)
        }
        _ => req.header("x-api-key", credential),
    };
    let resp = req.json(&body).send().expect("the raw request went out");
    println!("raw request status: {}", resp.status());
    for line in std::io::BufReader::new(resp).lines().map_while(Result::ok) {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        let Ok(frame) = serde_json::from_str::<serde_json::Value>(data) else {
            continue;
        };
        let kind = frame.get("type").and_then(serde_json::Value::as_str);
        let usage = match kind {
            Some("message_start") => frame.pointer("/message/usage"),
            Some("message_delta") => frame.get("usage"),
            _ => None,
        };
        if let (Some(kind), Some(usage)) = (kind, usage) {
            println!("raw {kind} usage: {usage}");
        }
    }
}

// ---------------------------------------------------------------------------
// OpenAI ChatGPT (Responses wire; implicit caching + prompt_cache_key)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "hits the network; needs OPENAI_CHATGPT_REFRESH_TOKEN + ALTER_ZERO_LIVE_TOKEN_STORE"]
fn live_chatgpt_responses_second_turn_reads_the_prefix_from_cache() {
    // The Responses wire (`docs/chatgpt.md`): the system prompt hoisted into
    // `instructions`, `prompt_cache_key` riding every request, the usage
    // frame's `input_tokens_details.cached_tokens` parsed. OpenAI caches an
    // identical prefix automatically past 1024 tokens.
    alter_zero::llm::chatgpt::set_store_path(token_store());
    let salt = salt();
    let refresh = credential("OPENAI_CHATGPT_REFRESH_TOKEN");
    let probe = config("openai_chatgpt", "probe", Some(refresh.clone()), salt);
    let listed = fetch_models(&probe, &CancelToken::new()).expect("the ChatGPT model listing");
    let ids: Vec<&str> = listed.iter().map(|m| m.id.as_str()).collect();
    println!("ChatGPT models: {ids:?}");
    let model = std::env::var("ALTER_ZERO_LIVE_CHATGPT_MODEL").unwrap_or_else(|_| {
        ids.iter()
            .find(|id| id.contains("mini"))
            .or_else(|| ids.first())
            .expect("the listing names a model")
            .to_string()
    });
    println!("model under test: {model}");
    let cfg = config("openai_chatgpt", &model, Some(refresh), salt);
    let backend = LlmBackend::configure(cfg, Some(big_system_prompt(salt)), false);
    let (first, second) = two_turns(&backend);
    assert!(first.total() > 1_000, "the whole prefix billed: {first:?}");
    assert!(
        second.cached > 1_000,
        "turn 2 read the prefix from OpenAI's cache: {second:?}"
    );
}

#[test]
#[ignore = "hits the network; needs OPENAI_CHATGPT_REFRESH_TOKEN + ALTER_ZERO_LIVE_TOKEN_STORE; diagnostic"]
fn live_chatgpt_cache_diagnostics() {
    // Two A/B rounds over the ChatGPT backend, printed rather than asserted:
    // (A) the request exactly as the app sends it, with a pause before the
    // repeat so an asynchronous cache write has landed; (B) the same with
    // the Codex CLI's own per-request identity headers on top
    // (`session_id` / `conversation_id` / `OpenAI-Beta`), which is what
    // the reference client sends beside `prompt_cache_key`. Whichever
    // reports cache reads tells what the backend's routing keys on.
    alter_zero::llm::chatgpt::set_store_path(token_store());
    let refresh = credential("OPENAI_CHATGPT_REFRESH_TOKEN");
    let model = model_or("ALTER_ZERO_LIVE_CHATGPT_MODEL", "gpt-5.4-mini");
    let pause = std::time::Duration::from_secs(12);
    let beta = ("OpenAI-Beta", "responses=experimental");
    // Each variant: a label and the extra headers it adds on top of what the
    // app sends; `{key}` stands for the session's cache key.
    let variants: [(&str, &[(&str, &str)]); 4] = [
        ("as sent", &[]),
        (
            "session_id + conversation_id + OpenAI-Beta",
            &[("session_id", "{key}"), ("conversation_id", "{key}"), beta],
        ),
        (
            "session_id + conversation_id",
            &[("session_id", "{key}"), ("conversation_id", "{key}")],
        ),
        ("OpenAI-Beta only", &[beta]),
    ];
    for (offset, (label, headers)) in variants.iter().enumerate() {
        let salt = salt() + offset as u64;
        let mut cfg = config("openai_chatgpt", &model, Some(refresh.clone()), salt);
        let key = cfg.cache_key.clone().expect("the cache key");
        for (name, value) in *headers {
            cfg.extra_headers
                .push(((*name).to_string(), value.replace("{key}", &key)));
        }
        let backend = LlmBackend::configure(cfg, Some(big_system_prompt(salt)), false);
        let (_, first) = complete_with_usage(&backend, "Just say ONE.");
        println!("({label}) turn 1: {first:?}");
        std::thread::sleep(pause);
        let (_, second) = complete_with_usage(&backend, "Just say ONE.");
        println!("({label}) turn 2 after {pause:?}: {second:?}");
    }
}

// ---------------------------------------------------------------------------
// Ollama Cloud (native wire; usage reported, no cache detail exists)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "hits the network; needs OLLAMA_API_KEY"]
fn live_ollama_cloud_reports_usage_but_no_cache_detail() {
    // The native `/api/chat` wire (`docs/ollama.md`) reports the final
    // frame's `prompt_eval_count` / `eval_count` and nothing about caching —
    // there is no such field on that API. The tally is real (the whole
    // prompt, both turns); the cached share is honestly zero rather than a
    // guess.
    let salt = salt();
    let model = model_or("ALTER_ZERO_LIVE_OLLAMA_CLOUD_MODEL", "gpt-oss:20b");
    let cfg = config(
        "ollama_cloud",
        &model,
        Some(credential("OLLAMA_API_KEY")),
        salt,
    );
    let backend = LlmBackend::configure(cfg, Some(big_system_prompt(salt)), false);
    let (first, second) = two_turns(&backend);
    assert!(
        first.input > 1_000,
        "turn 1 counted the whole prompt: {first:?}"
    );
    assert!(
        second.input > 1_000,
        "turn 2 counted the whole prompt too, cache hit or not: {second:?}"
    );
    assert_eq!(
        (second.cached, second.cache_write),
        (0, 0),
        "the wire has no cache accounting to report: {second:?}"
    );
}

// ---------------------------------------------------------------------------
// GitHub Copilot (chat wire behind a token exchange; server-side caching)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "hits the network; needs GITHUB_COPILOT_TOKEN and a route to api.github.com's token exchange"]
fn live_copilot_reports_its_usage_frame() {
    // Copilot's proxy answers with an OpenAI-shaped usage frame; whether the
    // cached detail is filled in depends on the upstream model. The tally
    // must at least be the provider's number.
    let salt = salt();
    let model = model_or("ALTER_ZERO_LIVE_COPILOT_MODEL", "gpt-4.1");
    let cfg = config(
        "github_copilot",
        &model,
        Some(credential("GITHUB_COPILOT_TOKEN")),
        salt,
    );
    let backend = LlmBackend::configure(cfg, Some(big_system_prompt(salt)), false);
    let (first, second) = two_turns(&backend);
    assert!(first.total() > 1_000, "the whole prefix billed: {first:?}");
    assert!(second.total() > 1_000, "{second:?}");
}
