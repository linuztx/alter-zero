//! Live probe of the task tools (docs/task-tools.md) against a real model:
//! does it keep its task list current while it works? NOT run by
//! `cargo test` (it needs the network and an API key).
//!
//! It drives the real agent loop exactly as the TUI does — the real system
//! prompt, the task list and a background registry attached, no tool-call
//! cap — in a working directory of your choosing, and prints each round's
//! tool calls, every change to the task list, and every reminder the loop
//! injected, then a tally of what each task went through.
//!
//! ```bash
//! PROVIDER=ollama_cloud MODEL=gpt-oss:120b OLLAMA_API_KEY=… \
//!   cargo run --example task_probe -- /path/to/workdir "the task"
//! ```
//!
//! `PROVIDER` is any built-in provider id (`ollama_cloud`, `openrouter`, …);
//! its key is read from the variable its `providers.toml` entry names.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use alter_zero::background::{BackgroundRegistry, BgEvent};
use alter_zero::context::{ContextMessage, ContextRole};
use alter_zero::llm::LlmBackend;
use alter_zero::llm::config::{ProvidersFile, Selection};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};
use alter_zero::tasks::{TaskRegistry, TaskStatus, TaskStore};
use tokio::sync::mpsc::unbounded_channel;

/// Print the list the way `tasklist` reports it, dim, under a heading.
fn show_list(store: &TaskStore) {
    for line in store.run_list().lines() {
        println!("\x1b[90m    {line}\x1b[0m");
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let usage = "usage: task_probe <workdir> <prompt>";
    let workdir = args.next().expect(usage);
    let prompt = args.next().expect(usage);
    let provider_id = std::env::var("PROVIDER").unwrap_or_else(|_| "ollama_cloud".to_string());
    let model = std::env::var("MODEL").unwrap_or_else(|_| "gpt-oss:120b".to_string());

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
            cache_key: Some(format!("task-probe-{}", std::process::id())),
            service_tier: None,
        })
        .expect("a model config");

    std::fs::create_dir_all(&workdir).expect("create the workdir");
    std::env::set_current_dir(&workdir).expect("enter the workdir");
    let cwd = std::env::current_dir().expect("a cwd");
    let system = alter_zero::llm::backend::augment_with_environment(
        alter_zero::llm::backend::DEFAULT_SYSTEM_PROMPT,
        "Friday 2026-10-02",
        "linux",
        &cwd.display().to_string(),
    );

    let (bg_tx, mut bg_rx) = unbounded_channel::<BgEvent>();
    let tasks_dir = std::env::temp_dir().join(format!("task-probe-{}", std::process::id()));
    let background = BackgroundRegistry::new(bg_tx, tasks_dir);
    let _bg_drain = std::thread::spawn(move || {
        while let Some(event) = bg_rx.blocking_recv() {
            if let BgEvent::Started { id, command, .. } = event {
                println!("\x1b[36m  [background] {id}: {command}\x1b[0m");
            }
        }
    });

    let tasks = TaskRegistry::new();
    let backend = LlmBackend::with_system_prompt(cfg, Some(system))
        .with_background(background.clone())
        .with_tasks(tasks.clone())
        .with_max_tool_calls(0);
    println!(
        "== {provider_id} / {model}\n== in {}\n== {prompt}\n",
        cwd.display()
    );

    let (tx, mut rx) = unbounded_channel();
    let context = vec![ContextMessage::new(ContextRole::User, &prompt)];
    let started = std::time::Instant::now();
    let handle = backend.spawn(prompt, vec![], context, tx, CancelToken::new());
    // Every status each task ever held, and the tool calls by name.
    let mut seen: BTreeMap<u64, BTreeSet<&'static str>> = BTreeMap::new();
    let mut calls: BTreeMap<String, usize> = BTreeMap::new();
    let mut reminders = 0usize;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(text) => {
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            StreamEvent::ToolStart {
                name, arguments, ..
            } => {
                *calls.entry(name.clone()).or_default() += 1;
                let arguments = arguments.unwrap_or_default();
                let shown: String = arguments.chars().take(160).collect();
                println!("\n\x1b[33m● {name} {shown}\x1b[0m");
            }
            StreamEvent::ToolEnd { output, ok, .. } => {
                let first = output.lines().next().unwrap_or_default();
                let label = if ok { "ok" } else { "failed" };
                println!("\x1b[90m  ⎿ [{label}] {first}\x1b[0m");
            }
            StreamEvent::ToolAnswered { display, .. } => {
                let first = display.lines().next().unwrap_or_default();
                println!("\x1b[90m  ⎿ [ok] {first}\x1b[0m");
            }
            StreamEvent::ToolBackgrounded { output, .. } => {
                let first = output.lines().next().unwrap_or_default();
                println!("\x1b[90m  ⎿ [background] {first}\x1b[0m");
            }
            StreamEvent::TaskCall {
                name,
                arguments,
                output,
                tasks: snapshot,
                ..
            } => {
                *calls.entry(name.clone()).or_default() += 1;
                println!("\n\x1b[32m◆ {name} {arguments}\x1b[0m");
                println!("\x1b[90m  ⎿ {output}\x1b[0m");
                for task in snapshot.tasks() {
                    seen.entry(task.id).or_default().insert(task.status.wire());
                }
                show_list(&snapshot);
            }
            StreamEvent::HookNote { label, text } => {
                reminders += 1;
                println!("\n\x1b[35m✦ {label}\x1b[0m");
                for line in text.lines() {
                    println!("\x1b[35m  │ {line}\x1b[0m");
                }
            }
            StreamEvent::StreamDone => break,
            StreamEvent::Error(err) => {
                println!("\n\x1b[31m[error] {err}\x1b[0m");
                break;
            }
            _ => {}
        }
    }
    let _ = handle.join();
    background.kill_all();

    let last = tasks.snapshot();
    println!("\n\n== {:.0?} · calls {calls:?}", started.elapsed());
    println!("== reminders injected: {reminders}");
    println!("== final list:");
    show_list(&last);
    let ever = |status: TaskStatus| {
        seen.values()
            .filter(|statuses| statuses.contains(status.wire()))
            .count()
    };
    println!(
        "== {} tasks · {} ever in_progress · {} completed",
        seen.len(),
        ever(TaskStatus::InProgress),
        last.tasks()
            .iter()
            .filter(|task| task.status == TaskStatus::Completed)
            .count(),
    );
}
