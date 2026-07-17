//! Manual end-to-end check of the real tool-calling loop against a live
//! provider. NOT run by `cargo test` (it needs the network + an API key).
//!
//! Usage:
//! ```bash
//! OPENROUTER_API_KEY=sk-... \
//!   INLINE_TUI_CA_FILE=/root/.ccr/ca-bundle.crt \
//!   cargo run --example tool_smoke -- [model] ["prompt"]
//! ```
//! Defaults to `openai/gpt-4o-mini` and a prompt that forces a `bash` call.
//! Prints every `StreamEvent` as it arrives, so you can see the ToolStart /
//! ToolEnd / final answer flow.

use std::time::Duration;

use inline_tui::context::{ContextMessage, ContextRole};
use inline_tui::llm::LlmBackend;
use inline_tui::llm::config::{ProvidersFile, Selection};
use inline_tui::stream::{CancelToken, ReplySource, StreamEvent};
use tokio::sync::mpsc::unbounded_channel;

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .unwrap_or_else(|| "openai/gpt-4o-mini".to_string());
    let prompt = args.next().unwrap_or_else(|| {
        "Use the bash tool to run `ls` in the current directory, then tell me in one \
         sentence what kind of project this is."
            .to_string()
    });

    let key = std::env::var("OPENROUTER_API_KEY")
        .expect("set OPENROUTER_API_KEY to run the live tool smoke");

    let providers = ProvidersFile::builtin();
    let cfg = providers
        .model_config(&Selection {
            provider_id: "openrouter".to_string(),
            model: model.clone(),
            api_key: Some(key),
            temperature: Some(0.0),
        })
        .expect("openrouter is a built-in provider");

    println!("== model: {model} ==\n== prompt: {prompt}\n");

    let backend = LlmBackend::new(cfg);
    assert!(backend.tools_enabled(), "tools should be on by default");

    let (tx, mut rx) = unbounded_channel();
    let cancel = CancelToken::new();
    // The context ends with the current user message (as the app derives it).
    let context = vec![ContextMessage::new(ContextRole::User, &prompt)];
    let handle = backend.spawn(prompt, vec![], context, tx, cancel);

    let mut reply = String::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(c) => {
                print!("{c}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
                reply.push_str(&c);
            }
            StreamEvent::ToolBackgrounded { id, output } => {
                println!("\n\x1b[90m[backgrounded as {id}]\x1b[0m\n{output}");
            }
            StreamEvent::ToolBatch(items) => {
                // The model requested a batch of calls at once; the TUI shows the
                // not-yet-run ones as `⎿ Waiting…` (docs/parallel-tools.md).
                println!("\n\x1b[90m[batch of {} tool call(s)]\x1b[0m", items.len());
                for call in &items {
                    println!(
                        "\x1b[90m  ● {}({})  ⎿ Waiting…\x1b[0m",
                        call.name, call.args
                    );
                }
            }
            StreamEvent::ToolStart { name, args } => {
                println!("\n\x1b[34m● {name}({args})\x1b[0m");
            }
            StreamEvent::ToolOutput(chunk) => {
                // Live output streamed while the tool runs — the TUI tails it in
                // the running cell (docs/tool-streaming.md). Print it dim, inline.
                print!("\x1b[90m{chunk}\x1b[0m");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            StreamEvent::ToolEnd {
                output,
                ok,
                truncated,
            } => {
                let color = if ok { "\x1b[32m" } else { "\x1b[31m" };
                let head: String = output.lines().take(8).collect::<Vec<_>>().join("\n");
                println!("{color}  ⎿ ok={ok} truncated={truncated}\x1b[0m\n{head}\n  ---");
            }
            StreamEvent::ThinkingStart => println!("\x1b[90m[thinking…]\x1b[0m"),
            StreamEvent::ThinkingChunk(_) => {}
            StreamEvent::ThinkingEnd => {}
            StreamEvent::ToolCallDelta(frag) => {
                // The model is generating a tool call — show the streamed
                // name/argument fragments dim (the app counts these tokens).
                print!("\x1b[90m{frag}\x1b[0m");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            StreamEvent::Retrying { attempt, max } => {
                println!("\x1b[33m[retrying {attempt}/{max}]\x1b[0m");
            }
            StreamEvent::Error(msg) => {
                println!("\n\x1b[31m[error] {msg}\x1b[0m");
                break;
            }
            StreamEvent::StreamDone => {
                println!("\n\x1b[90m[done]\x1b[0m");
                break;
            }
        }
    }
    // Give the backend thread a beat to exit, then join.
    let _ = handle.join();
    std::thread::sleep(Duration::from_millis(50));
}
