//! Live probe of the task tools (`docs/task-tools.md`) against a real model:
//! does it keep its task list current while it works? NOT run by
//! `cargo test` (it needs the network and an API key).
//!
//! It drives the real agent loop the way the TUI does — the real system
//! prompt, a background registry, the shared task list attached — in a
//! working directory of your choosing, and prints every task call with the
//! checklist it left behind, every task reminder the guard injected, and the
//! final list, so you can see whether a model ticks its tasks or only
//! creates them.
//!
//! ```bash
//! PROVIDER=ollama_cloud MODEL=gpt-oss:120b OLLAMA_API_KEY=… \
//!   cargo run --example task_probe -- /path/to/workdir "the task"
//! ```
//!
//! `PROVIDER` is any built-in provider id (`ollama_cloud`, `openrouter`, …);
//! its key is read from the variable its `providers.toml` entry names.
//! `SHOW_THINKING=1` prints the model's reasoning too.

use std::io::Write;

use alter_zero::background::{BackgroundRegistry, BgEvent};
use alter_zero::context::{ContextMessage, ContextRole};
use alter_zero::llm::LlmBackend;
use alter_zero::llm::config::{ProvidersFile, Selection};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};
use alter_zero::tasks::{TaskRegistry, TaskStatus, TaskStore};
use tokio::sync::mpsc::unbounded_channel;

/// The checklist as the strip draws it, one dim row per task.
fn show_list(store: &TaskStore) {
    for task in store.tasks() {
        let glyph = match task.status {
            TaskStatus::Pending => "◻",
            TaskStatus::InProgress => "◼",
            TaskStatus::Completed => "✔",
        };
        println!("\x1b[90m    {glyph} #{} {}\x1b[0m", task.id, task.subject);
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let workdir = args.next().expect("usage: task_probe <workdir> <prompt>");
    let prompt = args.next().expect("usage: task_probe <workdir> <prompt>");
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

    std::env::set_current_dir(&workdir).expect("the workdir exists");
    let cwd = std::env::current_dir().expect("a cwd");
    let user = alter_zero::llm::backend::user_label(
        Some(rustix::process::geteuid().as_raw()),
        std::env::var("USER").ok().as_deref(),
    );
    let system = alter_zero::llm::backend::augment_with_environment(
        alter_zero::llm::backend::DEFAULT_SYSTEM_PROMPT,
        "Friday 2026-10-02",
        "linux",
        &user,
        &cwd.display().to_string(),
    );

    // Background shells live here (a server the model starts with `wait: 0`).
    let (bg_tx, mut bg_rx) = unbounded_channel::<BgEvent>();
    let shells_dir = std::env::temp_dir().join(format!("task-probe-{}", std::process::id()));
    let registry = BackgroundRegistry::new(bg_tx, shells_dir);
    let _bg_drain = std::thread::spawn(move || while bg_rx.blocking_recv().is_some() {});

    let tasks = TaskRegistry::new();
    // Uncapped, like the app (the `/settings` **Max tool calls** default):
    // the library's backstop would end a long plan before its last tick.
    let backend = LlmBackend::with_system_prompt(cfg, Some(system))
        .with_max_tool_calls(0)
        .with_max_retries(6)
        .with_background(registry.clone())
        .with_tasks(tasks.clone());
    println!(
        "== {provider_id} / {model}\n== in {}\n== {prompt}\n",
        cwd.display()
    );

    let (tx, mut rx) = unbounded_channel();
    let context = vec![ContextMessage::new(ContextRole::User, &prompt)];
    let started = std::time::Instant::now();
    let handle = backend.spawn(prompt, vec![], context, tx, CancelToken::new());
    let (mut task_calls, mut reminders, mut tool_calls) = (0usize, 0usize, 0usize);
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(text) => {
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            // The model's reasoning, when it streams one — why it stopped
            // where it did, which is the point of a probe.
            StreamEvent::ThinkingChunk(text) if std::env::var_os("SHOW_THINKING").is_some() => {
                print!("\x1b[2m{text}\x1b[0m");
                let _ = std::io::stdout().flush();
            }
            StreamEvent::ToolStart { name, args, .. } => {
                tool_calls += 1;
                println!("\n\x1b[33m● {name}({args})\x1b[0m");
            }
            StreamEvent::TaskCall {
                name,
                arguments,
                output,
                ok,
                tasks,
                ..
            } => {
                task_calls += 1;
                let colour = if ok { 36 } else { 31 };
                println!("\n\x1b[{colour}m● {name} {arguments}\x1b[0m");
                println!("\x1b[90m  ⎿ {output}\x1b[0m");
                show_list(&tasks);
            }
            StreamEvent::HookNote { label, text } => {
                reminders += 1;
                println!("\n\x1b[35m● {label}\x1b[0m");
                for line in text.lines() {
                    println!("\x1b[35m  │ {line}\x1b[0m");
                }
            }
            StreamEvent::Retrying { attempt, max } => {
                println!("\n\x1b[31m[retrying {attempt}/{max}]\x1b[0m");
            }
            StreamEvent::StreamDone => {
                println!("\n\x1b[32m[done]\x1b[0m");
                break;
            }
            StreamEvent::Error(err) => {
                println!("\n\x1b[31m[error] {err}\x1b[0m");
                break;
            }
            _ => {}
        }
    }
    let _ = handle.join();
    registry.kill_all();
    let store = tasks.snapshot();
    let counts = store.counts();
    println!(
        "\n\n== {:.1}s · {tool_calls} tool calls · {task_calls} task calls · {reminders} reminders",
        started.elapsed().as_secs_f64(),
    );
    println!(
        "== final list: {} tasks ({} done, {} in progress, {} open)",
        counts.total, counts.completed, counts.in_progress, counts.open
    );
    show_list(&store);
}
