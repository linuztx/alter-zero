//! Live probe of the interactive shell tools — `bash` with `tty` and
//! `bash_session` (docs/interactive-shell.md) — against a real model. NOT run
//! by `cargo test` (it needs the network and an API key).
//!
//! It drives the real agent loop exactly as the TUI does, with the real
//! system prompt and a background registry attached (sessions live there), in
//! a working directory of your choosing, and prints every tool call with the
//! **model-facing** result — what the model actually read — so you can watch
//! how a model uses the tools and tune their descriptions.
//!
//! ```bash
//! PROVIDER=openrouter MODEL=openai/gpt-4o-mini OPENROUTER_API_KEY=… \
//!   ALTER_ZERO_CA_FILE=/root/.ccr/ca-bundle.crt \
//!   cargo run --example session_probe -- /path/to/workdir "the task"
//! ```
//!
//! `PROVIDER` is any built-in provider id (`openrouter`, `ollama_cloud`,
//! `a0_venice`, …); its key is read from the variable its `providers.toml`
//! entry names.

use std::collections::BTreeMap;
use std::io::Write;

use alter_zero::background::{BackgroundRegistry, BgEvent};
use alter_zero::context::{ContextMessage, ContextRole};
use alter_zero::llm::LlmBackend;
use alter_zero::llm::config::{ProvidersFile, Selection};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};
use tokio::sync::mpsc::unbounded_channel;

/// Print a model-facing tool result indented under its call, dim.
fn show_result(label: &str, text: &str) {
    println!("\x1b[90m  ⎿ [{label}]\x1b[0m");
    for line in text.lines() {
        println!("\x1b[90m    │ {line}\x1b[0m");
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let workdir = args
        .next()
        .expect("usage: session_probe <workdir> <prompt>");
    let prompt = args
        .next()
        .expect("usage: session_probe <workdir> <prompt>");
    let provider_id = std::env::var("PROVIDER").unwrap_or_else(|_| "openrouter".to_string());
    let model = std::env::var("MODEL").expect("set MODEL");

    let providers = ProvidersFile::builtin();
    let provider = providers.get(&provider_id).expect("a built-in provider id");
    let key_env = provider.key_env(&provider_id);
    let key = std::env::var(&key_env).unwrap_or_else(|_| panic!("set {key_env}"));
    let cfg = providers
        .model_config(&Selection {
            provider_id: provider_id.clone(),
            model: model.clone(),
            api_key: Some(key),
            temperature: None,
            thinking: None,
            vision: None,
            context: None,
            api_base: None,
            cache_key: Some(format!("session-probe-{}", std::process::id())),
            service_tier: None,
        })
        .expect("a model config");

    std::env::set_current_dir(&workdir).expect("the workdir exists");
    let cwd = std::env::current_dir().expect("a cwd");
    let system = alter_zero::llm::backend::augment_with_environment(
        alter_zero::llm::backend::DEFAULT_SYSTEM_PROMPT,
        "Wednesday 2026-09-23",
        "linux",
        &cwd.display().to_string(),
    );

    let (bg_tx, mut bg_rx) = unbounded_channel::<BgEvent>();
    let tasks_dir = std::env::temp_dir().join(format!("session-probe-{}", std::process::id()));
    let registry = BackgroundRegistry::new(bg_tx, tasks_dir);
    let _bg_drain = std::thread::spawn(move || {
        while let Some(event) = bg_rx.blocking_recv() {
            match event {
                BgEvent::Started { id, command, .. } => {
                    println!("\x1b[36m  [registry] session {id} listed: {command}\x1b[0m");
                }
                BgEvent::Exited {
                    id,
                    code,
                    killed,
                    observed,
                } => println!(
                    "\x1b[36m  [registry] session {id} exited code={code:?} killed={killed} \
                     observed={observed}\x1b[0m"
                ),
                BgEvent::Output { .. } | BgEvent::Screen { .. } => {}
            }
        }
    });

    let backend =
        LlmBackend::with_system_prompt(cfg, Some(system)).with_background(registry.clone());
    println!(
        "== {provider_id} / {model}\n== in {}\n== {prompt}\n",
        cwd.display()
    );

    let (tx, mut rx) = unbounded_channel();
    let context = vec![ContextMessage::new(ContextRole::User, &prompt)];
    let started = std::time::Instant::now();
    let handle = backend.spawn(prompt, vec![], context, tx, CancelToken::new());
    let mut calls: BTreeMap<String, usize> = BTreeMap::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(text) => {
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            // The model's reasoning, when it streams one — why it did what
            // it did, which is the point of a probe.
            StreamEvent::ThinkingChunk(text) if std::env::var_os("SHOW_THINKING").is_some() => {
                print!("\x1b[35m{text}\x1b[0m");
                let _ = std::io::stdout().flush();
            }
            StreamEvent::ToolStart {
                name, arguments, ..
            } => {
                *calls.entry(name.clone()).or_default() += 1;
                println!(
                    "\n\x1b[33m● {name} {}\x1b[0m",
                    arguments.unwrap_or_default()
                );
            }
            StreamEvent::ToolEnd { output, ok, .. } => {
                show_result(if ok { "ok" } else { "failed" }, &output);
            }
            StreamEvent::ToolAnswered { result, .. } => show_result("ok", &result),
            StreamEvent::ToolRejected { result, .. } => show_result("rejected", &result),
            StreamEvent::ToolBackgrounded { output, .. } => show_result("backgrounded", &output),
            StreamEvent::StreamDone => break,
            StreamEvent::Error(err) => {
                println!("\n\x1b[31m[error] {err}\x1b[0m");
                break;
            }
            _ => {}
        }
    }
    let _ = handle.join();
    registry.kill_all();
    println!(
        "\n\n== {:.1}s · calls: {}",
        started.elapsed().as_secs_f64(),
        calls
            .iter()
            .map(|(name, n)| format!("{name}×{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
}
