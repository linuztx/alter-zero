//! Live probe of the task tools (`docs/task-tools.md`) against a real model.
//! NOT run by `cargo test` (it needs the network and an API key).
//!
//! It drives the real agent loop exactly as the TUI does — the real system
//! prompt, a background registry, and the shared task list attached — in a
//! working directory of your choosing, and prints every call, every task-list
//! change as the checklist would show it, and every reminder the loop slips
//! into the conversation, so you can watch whether a model keeps its list
//! current and tune the wording that nudges it.
//!
//! ```bash
//! PROVIDER=ollama_cloud MODEL=gpt-oss:120b OLLAMA_API_KEY=… \
//!   ALTER_ZERO_CA_FILE=/root/.ccr/ca-bundle.crt \
//!   cargo run --example task_probe -- /path/to/workdir "the task"
//! ```
//!
//! `PROVIDER` is any built-in provider id; its key is read from the variable
//! its `providers.toml` entry names.

use std::io::Write;

use alter_zero::background::{BackgroundRegistry, BgEvent};
use alter_zero::context::{ContextMessage, ContextRole};
use alter_zero::llm::LlmBackend;
use alter_zero::llm::config::{ProvidersFile, Selection};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};
use alter_zero::tasks::{TaskRegistry, TaskStatus, TaskStore};
use tokio::sync::mpsc::unbounded_channel;

/// One line per task, the checklist's glyphs.
fn checklist(store: &TaskStore) -> String {
    store
        .tasks()
        .iter()
        .map(|task| {
            let glyph = match task.status {
                TaskStatus::Pending => "◻",
                TaskStatus::InProgress => "◼",
                TaskStatus::Completed => "✔",
            };
            format!("    {glyph} #{} {}", task.id, task.subject)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Print a model-facing result indented under its call, dim.
fn show_result(label: &str, text: &str) {
    println!("\x1b[90m  ⎿ [{label}]\x1b[0m");
    for line in text.lines().take(8) {
        println!("\x1b[90m    │ {line}\x1b[0m");
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let workdir = args.next().expect("usage: task_probe <workdir> <prompt>");
    let prompt = args.next().expect("usage: task_probe <workdir> <prompt>");
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

    let (bg_tx, mut bg_rx) = unbounded_channel::<BgEvent>();
    let tasks_dir = std::env::temp_dir().join(format!("task-probe-{}", std::process::id()));
    let registry = BackgroundRegistry::new(bg_tx, tasks_dir);
    let _bg_drain = std::thread::spawn(move || while bg_rx.blocking_recv().is_some() {});

    let task_list = TaskRegistry::new();
    // Uncapped, as the app ships (`/settings` **Max tool calls** defaults
    // to 0): a build-and-serve task easily runs past the library's 20.
    let backend = LlmBackend::with_system_prompt(cfg, Some(system))
        .with_background(registry.clone())
        .with_tasks(task_list.clone())
        .with_max_tool_calls(0);
    println!(
        "== {provider_id} / {model}\n== in {}\n== {prompt}\n",
        cwd.display()
    );

    let (tx, mut rx) = unbounded_channel();
    let context = vec![ContextMessage::new(ContextRole::User, &prompt)];
    let started = std::time::Instant::now();
    let handle = backend.spawn(prompt, vec![], context, tx, CancelToken::new());
    let mut ever_in_progress = false;
    let mut task_calls = 0usize;
    let mut other_calls = 0usize;
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
                other_calls += 1;
                let arguments = arguments.unwrap_or_default();
                let shown: String = arguments.chars().take(160).collect();
                println!("\n\x1b[33m● {name} {shown}\x1b[0m");
            }
            StreamEvent::ToolEnd { output, ok, .. } => {
                show_result(if ok { "ok" } else { "failed" }, &output);
            }
            StreamEvent::ToolAnswered { result, .. } => show_result("ok", &result),
            StreamEvent::ToolRejected { result, .. } => show_result("rejected", &result),
            StreamEvent::ToolBackgrounded { output, .. } => show_result("backgrounded", &output),
            StreamEvent::TaskCall {
                name,
                arguments,
                output,
                tasks,
                ..
            } => {
                task_calls += 1;
                ever_in_progress |= tasks
                    .tasks()
                    .iter()
                    .any(|task| task.status == TaskStatus::InProgress);
                println!("\n\x1b[36m● {name} {arguments}\x1b[0m");
                show_result("task", &output);
                println!("\x1b[36m{}\x1b[0m", checklist(&tasks));
            }
            StreamEvent::HookNote { label, text } => {
                reminders += 1;
                println!("\n\x1b[35m» {label}\x1b[0m");
                for line in text.lines() {
                    println!("\x1b[35m  {line}\x1b[0m");
                }
            }
            StreamEvent::Retrying { attempt, max } => {
                println!("\n\x1b[31m[retrying {attempt}/{max}]\x1b[0m");
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
    registry.kill_all();
    let end = task_list.snapshot();
    let counts = end.counts();
    println!(
        "\n\n== {:.1}s · {other_calls} tool calls · {task_calls} task calls · {reminders} \
         reminders\n== final list: {} tasks ({} done, {} in progress, {} open) · ever in \
         progress: {ever_in_progress}\n{}",
        started.elapsed().as_secs_f64(),
        counts.total,
        counts.completed,
        counts.in_progress,
        counts.open,
        checklist(&end),
    );
}
