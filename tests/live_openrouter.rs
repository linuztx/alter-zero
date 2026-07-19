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
use inline_tui::llm::{LlmBackend, ModelConfig, ThinkingMode};
use inline_tui::stream::{CancelToken, ReplySource, StreamEvent};

/// A backend configured for OpenRouter from the environment, for `model` with
/// the given thinking mode. Panics with a clear message when the key is
/// missing — these tests are only ever run on purpose (`--ignored`).
fn backend_for(model: String, thinking: Option<ThinkingMode>) -> LlmBackend {
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
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    };
    LlmBackend::with_system_prompt(
        cfg,
        Some("You are a terse assistant. Answer in as few words as possible.".to_string()),
    )
}

/// The default backend under test (`INLINE_TUI_LIVE_MODEL`, else a cheap
/// vision-capable model), no thinking mode.
fn backend() -> LlmBackend {
    let model =
        std::env::var("INLINE_TUI_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    backend_for(model, None)
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
fn live_environment_context_reaches_the_model() {
    // The context-awareness feature end to end (docs/environment.md): the
    // boundary folds date/os/cwd into the system prompt via
    // `augment_with_environment`; a real model must be able to read the cwd
    // back out of that block. Proves the block rides the request and is
    // legible to the model. Tools off so the model answers from the prompt
    // instead of shelling out for the path.
    use inline_tui::llm::backend::augment_with_environment;
    let key =
        std::env::var("OPENROUTER_API_KEY").expect("set OPENROUTER_API_KEY to run the live tests");
    let model =
        std::env::var("INLINE_TUI_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    let cwd = "/home/user/inline-tui-sentinel-42";
    let system = augment_with_environment(
        "You are Alter Zero an autonomous AI agent running in terminal UI",
        "Sunday 2026-07-19",
        "linux",
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
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    };
    let backend = LlmBackend::configure(cfg, Some(system), false);

    let prompt = "What is your current working directory? Reply with just the path.";
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
        Some(ThinkingMode::Effort(inline_tui::llm::ReasoningEffort::Low)),
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

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_vision_survives_a_large_image_upload() {
    // The send-phase regression canary (docs/llm.md): a multi-megabyte base64
    // body — the realistic size of a pasted screenshot — must survive the
    // send/header exchange. Under the old 3 s per-operation timeout this
    // upload burned the whole retry budget re-hitting the same wall and the
    // turn failed ("[Image] can't be processed").
    let path = std::env::temp_dir().join("inline-tui-live-vision-large.png");
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
    let dir = std::env::temp_dir().join(format!("inline-tui-live-ctrlb-{}", std::process::id()));
    let registry = inline_tui::background::BackgroundRegistry::new(bg_tx, dir);
    let backend = backend().with_background(registry.clone());

    let prompt = "Use the bash tool exactly once to run this command in the foreground \
                  (do NOT set run_in_background): sh -c 'echo started; sleep 8; echo finished'. \
                  After the tool result arrives, reply with just the task ID it reported.";
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());

    let mut backgrounded: Option<(String, String)> = None;
    let mut reply = String::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            // The user's Ctrl+B, as the loop performs it (Action::MoveToBackground):
            // raise the latch once the command is running; the executor's poll
            // loop consumes it and adopts the child mid-run.
            StreamEvent::ToolStart { .. } => registry.request_background(),
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
        output.contains(&format!("ID: {id}")),
        "the handoff text still names the task: {output}"
    );
    // The turn completed with a text reply (the model kept going off the
    // handoff text instead of wedging on the missing output); the adopted
    // command finishes on its own and the registry reports the lifecycle.
    let mut streamed = String::new();
    let mut exited = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        match bg_rx.try_recv() {
            Ok(inline_tui::background::BgEvent::Started { id: started, .. }) => {
                assert_eq!(started, id);
            }
            Ok(inline_tui::background::BgEvent::Output { chunk, .. }) => streamed.push_str(&chunk),
            Ok(inline_tui::background::BgEvent::Exited { code, killed, .. }) => {
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
    let dir = std::env::temp_dir().join(format!("inline-tui-live-kill-{}", std::process::id()));
    let registry = inline_tui::background::BackgroundRegistry::new(bg_tx, dir);
    let backend = backend().with_background(registry.clone());

    // The boundary simulator: apply the registry's events to the pure App
    // state and, on Exited, post the completion's note — the `main.rs`
    // Exited-arm dance (docs/background.md).
    let post_registry = registry.clone();
    let boundary = std::thread::spawn(move || {
        let mut app = inline_tui::app::App::new();
        let mut posted = None;
        while let Some(event) = bg_rx.blocking_recv() {
            match event {
                inline_tui::background::BgEvent::Started {
                    id,
                    command,
                    description,
                    from_model,
                } => app.bg_started(&id, &command, description, from_model),
                inline_tui::background::BgEvent::Output { id, chunk } => {
                    app.bg_output(&id, &chunk);
                }
                inline_tui::background::BgEvent::Exited { id, code, killed } => {
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
fn live_run_in_background_resolves_and_completes() {
    // The full production background path (docs/background.md): the model is
    // told to run a command with run_in_background — the agent loop resolves
    // the call via ToolBackgrounded (the launch text as the tool result, the
    // turn finishing while the process runs), and the shared registry reports
    // Started → Output → Exited on its own channel.
    let (bg_tx, mut bg_rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = std::env::temp_dir().join(format!("inline-tui-live-bg-{}", std::process::id()));
    let registry = inline_tui::background::BackgroundRegistry::new(bg_tx, dir);
    let backend = backend().with_background(registry);

    let prompt = "Use the bash tool exactly once to run this command in the background \
                  (set run_in_background to true): sh -c 'echo live_bg_marker; sleep 1; echo done'. \
                  After the tool result arrives, reply with just the task ID it reported.";
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
        output.contains(&format!("ID: {id}")),
        "the model-facing launch text names the task: {output}"
    );
    // The registry reported the whole lifecycle on its own channel.
    let mut streamed = String::new();
    let mut exited = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        match bg_rx.try_recv() {
            Ok(inline_tui::background::BgEvent::Started { id: started, .. }) => {
                assert_eq!(started, id);
            }
            Ok(inline_tui::background::BgEvent::Output { chunk, .. }) => streamed.push_str(&chunk),
            Ok(inline_tui::background::BgEvent::Exited { code, killed, .. }) => {
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
