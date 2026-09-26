//! Scripted driver for the bash tools — `bash` and its companions `bashsend`,
//! `bashwait`, `bashkill` and `bashlist` (docs/bash-tools.md, over
//! docs/interactive-shell.md's engine) — with **no model**: each line
//! of the script is one tool call, run through the real executor with a
//! background registry attached, and the model-facing result is printed
//! with how long the call took. NOT run by `cargo test`.
//!
//! ```bash
//! cargo run --example pty_drive -- steps.jsonl
//! ```
//!
//! A step is `{"tool": "bash" | "bashsend" | …, "args": {…}}`, one per line
//! (`#` lines and blank lines are skipped). `$S` anywhere in the args is
//! replaced by the session id the last running report named, so a script can
//! launch a program and type into it without knowing the id in advance.
//! `{"sleep": ms}` pauses between steps (the model thinking).

use std::io::BufRead;
use std::time::{Duration, Instant};

use alter_zero::background::{BackgroundRegistry, BgEvent};
use alter_zero::llm::exec::{RealToolExecutor, ToolExecutor, ToolProgress};
use alter_zero::llm::tools::ToolCallRequest;
use alter_zero::pty::report::{Frame, parse_frame};
use alter_zero::stream::CancelToken;
use serde_json::Value;
use tokio::sync::mpsc::unbounded_channel;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: pty_drive <steps.jsonl>");
    let file = std::fs::File::open(&path).expect("the steps file");

    let (bg_tx, mut bg_rx) = unbounded_channel::<BgEvent>();
    let tasks_dir = std::env::temp_dir().join(format!("pty-drive-{}", std::process::id()));
    let registry = BackgroundRegistry::new(bg_tx, tasks_dir);
    let _drain = std::thread::spawn(move || {
        while let Some(event) = bg_rx.blocking_recv() {
            match event {
                BgEvent::Started { id, command, .. } => {
                    println!("\x1b[36m  [registry] {id} listed: {command}\x1b[0m");
                }
                BgEvent::Exited {
                    id,
                    code,
                    killed,
                    observed,
                } => println!(
                    "\x1b[36m  [registry] {id} exited code={code:?} killed={killed} \
                     observed={observed}\x1b[0m"
                ),
                BgEvent::Waiting { id } => {
                    println!("\x1b[36m  [registry] {id} waiting for input\x1b[0m");
                }
                BgEvent::Output { .. } | BgEvent::Screen { .. } => {}
            }
        }
    });
    let executor = RealToolExecutor::new().with_background(registry.clone());
    let cancel = CancelToken::new();
    let mut session = String::new();

    for (n, line) in std::io::BufReader::new(file).lines().enumerate() {
        let line = line.expect("a line");
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let step: Value = serde_json::from_str(line)
            .unwrap_or_else(|err| panic!("step {}: {err}: {line}", n + 1));
        if let Some(ms) = step.get("sleep").and_then(Value::as_u64) {
            std::thread::sleep(Duration::from_millis(ms));
            continue;
        }
        let tool = step["tool"].as_str().expect("a tool").to_string();
        let args = step["args"].to_string().replace("$S", &session);
        println!("\n\x1b[33m● {tool} {args}\x1b[0m");
        let call = ToolCallRequest {
            id: format!("call-{n}"),
            name: tool,
            arguments: args,
        };
        let started = Instant::now();
        let mut updates = 0usize;
        let mut title = None;
        let outcome = executor.execute(&call, &cancel, &mut |progress| match progress {
            ToolProgress::Screen { .. } => updates += 1,
            ToolProgress::Title(t) => title = Some(t.to_string()),
        });
        let elapsed = started.elapsed();
        if let Some(title) = title {
            println!("\x1b[36m  ↳ {title}\x1b[0m");
        }
        let text = outcome.context.as_deref().unwrap_or(&outcome.output);
        println!(
            "\x1b[90m  ⎿ [{}] {:.2}s, {updates} stream updates\x1b[0m",
            if outcome.ok { "ok" } else { "failed" },
            elapsed.as_secs_f64()
        );
        for line in text.lines() {
            println!("\x1b[90m    │\x1b[0m {line}");
        }
        if let Some(Frame::Running { session: id, .. } | Frame::Stopped { session: id }) =
            text.lines().next().and_then(parse_frame)
        {
            session = id.to_string();
        }
    }
    registry.kill_all();
}
