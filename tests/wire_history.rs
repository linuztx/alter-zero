//! The retained wire prefix, end to end (`docs/prompt-caching.md`, *Keeping
//! the conversation prefix stable*): a real [`LlmBackend`] turn against a
//! stand-in on the loopback, its events folded into the [`App`] the way the
//! boundary folds them, and the next turn's request read back off the wire.
//! What the provider must see again is what it sent — its own tool-call id
//! and the round's exact text — rather than the `call_0` and the trimmed
//! segment the display history reconstructs. Once per wire whose reply text
//! reaches the app differently: Chat Completions trims a reply's opening
//! whitespace before anyone sees it, the Responses wire hands it over raw.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use alter_zero::app::{App, HistoryItem};
use alter_zero::context::{ContextMessage, ContextRole, context_messages};
use alter_zero::llm::hooks::{HookSink, PromptVerdict};
use alter_zero::llm::{LlmBackend, ModelConfig, WireApi};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};

/// The one call every round below makes, and the arguments it records.
const ARGUMENTS: &str = r#"{"path":"/nonexistent/alter-zero-wire-history"}"#;

/// A Chat Completions round in which the model says nothing but a blank line
/// before its one call.
const CHAT_TOOL_ROUND: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"\\n\\n\"}}]}\n\n",
    r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_from_the_provider","type":"function","function":{"name":"read","arguments":"{\"path\":\"/nonexistent/alter-zero-wire-history\"}"}}]}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
    "\n\n",
    "data: [DONE]\n\n",
);

/// A Chat Completions round with a **parallel batch** of two reads, the
/// shape one assistant message carries on the wire.
const CHAT_BATCH_ROUND: &str = concat!(
    r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_provider_a","type":"function","function":{"name":"read","arguments":"{\"path\":\"/nonexistent/alter-zero-a\"}"}}]}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_provider_b","type":"function","function":{"name":"read","arguments":"{\"path\":\"/nonexistent/alter-zero-b\"}"}}]}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
    "\n\n",
    "data: [DONE]\n\n",
);

/// A plain-text Chat Completions round that ends the turn.
const CHAT_TEXT_ROUND: &str = concat!(
    r#"data: {"choices":[{"delta":{"content":"done"}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
    "\n\n",
    "data: [DONE]\n\n",
);

/// The same tool round on the Responses wire (`docs/chatgpt.md`): the blank
/// line as an output-text delta, the call whole on one finished item.
const RESPONSES_TOOL_ROUND: &str = concat!(
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"\\n\\n\"}\n\n",
    r#"data: {"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_from_the_provider","name":"read","arguments":"{\"path\":\"/nonexistent/alter-zero-wire-history\"}"}}"#,
    "\n\n",
    r#"data: {"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#,
    "\n\n",
);

/// A plain-text Responses round that ends the turn.
const RESPONSES_TEXT_ROUND: &str = concat!(
    r#"data: {"type":"response.output_text.delta","delta":"done"}"#,
    "\n\n",
    r#"data: {"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":12,"output_tokens":1,"total_tokens":13}}}"#,
    "\n\n",
);

/// A provider stand-in on the loopback: answers each request with the next
/// canned SSE body, and keeps every request body it was sent.
fn stand_in(responses: &'static [&'static str]) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let base = format!(
        "http://{}/v1",
        listener.local_addr().expect("the bound address")
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    std::thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().expect("a connection");
            let request = read_request(&mut stream);
            seen.lock().expect("the request log").push(request);
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
                response.len()
            );
            stream
                .write_all(reply.as_bytes())
                .expect("the reply goes out");
            stream.flush().expect("the reply flushes");
        }
    });
    (base, requests)
}

/// One HTTP request's JSON body read off `stream`: the headers, then exactly
/// `content-length` bytes.
fn read_request(stream: &mut TcpStream) -> serde_json::Value {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).expect("request bytes");
        assert!(n > 0, "the request ended before its headers did");
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
    let length: usize = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .expect("a sized request body");
    while bytes.len() < header_end + length {
        let n = stream.read(&mut chunk).expect("body bytes");
        assert!(n > 0, "the request ended before its body did");
        bytes.extend_from_slice(&chunk[..n]);
    }
    serde_json::from_slice(&bytes[header_end..header_end + length]).expect("a JSON body")
}

/// Run one turn on `backend` over the context the app derived for it,
/// collecting every event up to the terminal one.
fn turn(backend: &LlmBackend, prompt: &str, context: Vec<ContextMessage>) -> Vec<StreamEvent> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let mut events = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        let terminal = matches!(
            event,
            StreamEvent::StreamDone | StreamEvent::Error(_) | StreamEvent::PromptBlocked { .. }
        );
        events.push(event);
        if terminal {
            break;
        }
    }
    handle.join().expect("the backend thread joins");
    events
}

/// Fold a turn's events into `app` the way `tui::stream` does for the events
/// a tool turn produces: the text before a call finalised as its own segment
/// (a whitespace-only run recording nothing), the batch announced, each call
/// resolved into a history record carrying the model's verbatim arguments,
/// the closing reply recorded on `StreamDone`, the round's announced call
/// ids opened ahead of its cells.
fn fold(app: &mut App, events: Vec<StreamEvent>) {
    for event in events {
        match event {
            StreamEvent::Chunk(text) => app.push_chunk(&text),
            StreamEvent::RoundCalls(calls) => app.open_round(calls),
            StreamEvent::ToolBatch(items) => app.start_tool_batch(&items),
            StreamEvent::ToolStart {
                name,
                args,
                arguments,
                ..
            } => {
                app.flush_streaming_segment();
                app.start_tool(&name, &args, arguments.as_deref());
            }
            StreamEvent::ToolEnd { output, ok, .. } => {
                app.end_tool(&output, ok);
            }
            StreamEvent::ToolAnswered {
                display, result, ..
            } => {
                app.answer_tool(&display, &result);
            }
            StreamEvent::ToolRejected {
                display, result, ..
            } => {
                app.reject_tool(&display, &result);
            }
            StreamEvent::StreamDone => {
                app.finish_stream();
                app.end_turn(0);
            }
            StreamEvent::Error(error) => panic!("backend error: {error}"),
            _ => {}
        }
    }
}

/// A backend on `wire` pointed at the stand-in at `base`, tools on, no
/// retries (a misbehaving stand-in should fail fast, not back off).
fn backend_on(wire: WireApi, base: &str) -> LlmBackend {
    let mut cfg = ModelConfig::fallback();
    cfg.api_base = base.to_string();
    cfg.api_key = Some("stand-in".to_string());
    cfg.wire_api = wire;
    LlmBackend::configure(cfg, Some("be terse".to_string()), true).with_max_retries(0)
}

/// A `UserPromptSubmit` hook that refuses one exact prompt (`docs/hooks.md`).
#[derive(Debug)]
struct BlockPrompt(&'static str);

impl HookSink for BlockPrompt {
    fn user_prompt_submit(&self, prompt: &str, _cancel: &CancelToken) -> PromptVerdict {
        PromptVerdict {
            blocked: (prompt == self.0).then(|| "not now".to_string()),
            contexts: Vec::new(),
        }
    }
}

/// Two human turns on `wire` — the first a tool round then a reply, the
/// second a reply — returning the request that closed the first turn and
/// the request that opened the second.
fn two_turns(
    wire: WireApi,
    rounds: &'static [&'static str],
) -> (serde_json::Value, serde_json::Value) {
    let (base, requests) = stand_in(rounds);
    let backend = backend_on(wire, &base);

    let mut app = App::new();
    app.record_user_message("look at the file");
    app.begin_stream();
    let events = turn(&backend, "look at the file", context_messages(&app.history));
    fold(&mut app, events);
    assert!(
        matches!(
            &app.history[1],
            HistoryItem::Tool(tool) if tool.arguments.as_deref() == Some(ARGUMENTS)
        ),
        "the call's verbatim arguments ride the record: {:?}",
        app.history
    );
    let conversation = app
        .history
        .iter()
        .filter(|item| !matches!(item, HistoryItem::Summary(_)))
        .count();
    assert_eq!(
        conversation, 3,
        "the prompt, the call and the reply — no segment for the blank line: {:?}",
        app.history
    );

    app.record_user_message("and now?");
    app.begin_stream();
    let events = turn(&backend, "and now?", context_messages(&app.history));
    fold(&mut app, events);

    let requests = requests.lock().expect("the request log");
    assert_eq!(requests.len(), 3, "two rounds, then one");
    (requests[1].clone(), requests[2].clone())
}

#[test]
fn the_next_chat_completions_turn_sends_the_prefix_the_provider_already_saw() {
    let (sent, next) = two_turns(
        WireApi::Chat,
        &[CHAT_TOOL_ROUND, CHAT_TEXT_ROUND, CHAT_TEXT_ROUND],
    );
    // What the first turn's second round sent back is what the provider
    // knows this conversation as: its own call id over an empty content —
    // this wire trims a reply's opening whitespace before the app or the
    // echo sees it, so the blank line is gone from both.
    assert_eq!(
        sent["messages"][2]["tool_calls"][0]["id"],
        "call_from_the_provider"
    );
    assert_eq!(sent["messages"][2]["content"], "");
    assert_eq!(sent["messages"][3]["role"], "tool");

    let messages = next["messages"].as_array().expect("a messages array");
    assert_eq!(
        messages.len(),
        6,
        "system, the prompt, the call, its result, the reply, the new prompt: {next}"
    );
    assert_eq!(
        messages[2], sent["messages"][2],
        "the provider's own tool call, byte for byte"
    );
    assert_eq!(messages[3], sent["messages"][3], "and its result");
    assert_eq!(messages[4]["content"], "done");
    assert_eq!(messages[5]["content"], "and now?");
}

#[test]
fn the_next_responses_turn_sends_the_prefix_the_provider_already_saw() {
    let (sent, next) = two_turns(
        WireApi::Responses,
        &[
            RESPONSES_TOOL_ROUND,
            RESPONSES_TEXT_ROUND,
            RESPONSES_TEXT_ROUND,
        ],
    );
    // This wire hands the blank line over raw: the echo carries it as an
    // assistant message item ahead of the call, while the app — which
    // records no whitespace-only segment — replays the call alone. Same
    // round; the provider's copy is still the one to reuse.
    let input = sent["input"].as_array().expect("an input array");
    assert_eq!(input[1]["role"], "assistant");
    assert_eq!(input[1]["content"][0]["text"], "\n\n");
    assert_eq!(input[2]["type"], "function_call");
    assert_eq!(input[2]["call_id"], "call_from_the_provider");
    assert_eq!(input[3]["type"], "function_call_output");

    let items = next["input"].as_array().expect("an input array");
    assert_eq!(
        items.len(),
        6,
        "the prompt, the blank line, the call, its result, the reply, the new prompt: {next}"
    );
    assert_eq!(
        &items[..4],
        &input[..4],
        "the provider's own prefix, byte for byte"
    );
    assert_eq!(items[4]["content"][0]["text"], "done");
    assert_eq!(items[5]["content"][0]["text"], "and now?");
}

#[test]
fn a_rebuilt_backend_sends_the_batch_the_provider_saw() {
    // No retained prefix at all: the second turn runs on a *fresh* backend
    // — what a `/model` switch, a settings rebuild or a `/resume` leaves —
    // and still sends the first turn's parallel batch as the provider saw
    // it: one assistant message, both calls, the provider's own ids. The
    // records carry the identity, so the derived context alone reproduces
    // the wire (docs/prompt-caching.md).
    let (base, requests) = stand_in(&[CHAT_BATCH_ROUND, CHAT_TEXT_ROUND, CHAT_TEXT_ROUND]);
    let first = backend_on(WireApi::Chat, &base);
    let mut app = App::new();
    app.record_user_message("read both");
    app.begin_stream();
    let events = turn(&first, "read both", context_messages(&app.history));
    fold(&mut app, events);
    let sent = requests.lock().expect("the request log")[1].clone();
    let calls = sent["messages"][2]["tool_calls"]
        .as_array()
        .expect("the batch rides one assistant message");
    assert_eq!(calls.len(), 2, "{sent}");
    assert_eq!(calls[0]["id"], "call_provider_a");
    assert_eq!(calls[1]["id"], "call_provider_b");

    let rebuilt = backend_on(WireApi::Chat, &base);
    app.record_user_message("and now?");
    app.begin_stream();
    let events = turn(&rebuilt, "and now?", context_messages(&app.history));
    fold(&mut app, events);
    let next = requests.lock().expect("the request log")[2].clone();
    let messages = next["messages"].as_array().expect("a messages array");
    assert_eq!(
        messages.len(),
        7,
        "system, the prompt, the batch, two results, the reply, the new prompt: {next}"
    );
    assert_eq!(messages[2], sent["messages"][2], "the batch, byte for byte");
    assert_eq!(messages[3], sent["messages"][3], "the first result");
    assert_eq!(messages[4], sent["messages"][4], "the second result");
    assert_eq!(messages[5]["content"], "done");
}

#[test]
fn a_blocked_prompt_does_not_cost_the_retained_prefix() {
    // The Responses wire echoes the model's blank lead as a message item the
    // records cannot reproduce, so only the retained prefix restores it. A
    // `UserPromptSubmit` block ends a turn before any request goes out; it
    // must not consume that prefix, or the next real turn re-sends a
    // different conversation (docs/prompt-caching.md).
    let (base, requests) = stand_in(&[
        RESPONSES_TOOL_ROUND,
        RESPONSES_TEXT_ROUND,
        RESPONSES_TEXT_ROUND,
    ]);
    let backend =
        backend_on(WireApi::Responses, &base).with_hooks(Arc::new(BlockPrompt("not yet")));
    let mut app = App::new();
    app.record_user_message("look at the file");
    app.begin_stream();
    let events = turn(&backend, "look at the file", context_messages(&app.history));
    fold(&mut app, events);
    let sent = requests.lock().expect("the request log")[1].clone();
    assert_eq!(sent["input"][1]["content"][0]["text"], "\n\n");

    // The blocked turn: the boundary rolls a blocked submission back out of
    // the history, so its prompt rides only this one context.
    let mut blocked = context_messages(&app.history);
    blocked.push(ContextMessage::new(ContextRole::User, "not yet"));
    let events = turn(&backend, "not yet", blocked);
    assert!(
        matches!(events.last(), Some(StreamEvent::PromptBlocked { .. })),
        "{events:?}"
    );
    assert_eq!(
        requests.lock().expect("the request log").len(),
        2,
        "nothing went out for the blocked prompt"
    );

    app.record_user_message("and now?");
    app.begin_stream();
    let events = turn(&backend, "and now?", context_messages(&app.history));
    fold(&mut app, events);
    let next = requests.lock().expect("the request log")[2].clone();
    let items = next["input"].as_array().expect("an input array");
    let prefix = sent["input"].as_array().expect("an input array");
    assert_eq!(
        &items[..4],
        &prefix[..4],
        "the provider's own prefix survives the block"
    );
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY; costs about a cent"]
fn live_openrouter_a_rebuilt_backend_reads_the_tool_round_back_from_cache() {
    // The durable half on a real cache: a tool turn on OpenRouter's
    // Anthropic routing (explicit breakpoints, so a read is deterministic),
    // its events folded into the `App` as the loop folds them, and the next
    // turn sent by a *fresh* backend — no retained request, what a `/model`
    // switch, a settings rebuild or a `/resume` leaves — from the derived
    // context alone. The records carry the provider's own call id and the
    // round's exact text, so the provider reads the prefix through the tool
    // result back instead of re-reading it from the call on
    // (docs/prompt-caching.md). Both backends share the session's affinity
    // key, as two backends of one process do.
    use alter_zero::llm::{ProvidersFile, Selection};
    let key =
        std::env::var("OPENROUTER_API_KEY").expect("set OPENROUTER_API_KEY to run this live test");
    let model = std::env::var("ALTER_ZERO_LIVE_OPENROUTER_MODEL")
        .unwrap_or_else(|_| "~anthropic/claude-haiku-latest".to_string());
    let salt = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Past every provider's minimum cacheable size, salted so the run
    // starts cold.
    let corpus: String = (0..420)
        .map(|i| format!("Calibration sentence {i} of the standing corpus, run {salt}. "))
        .collect();
    let system =
        format!("You are a terse assistant. Answer in as few words as possible.\n{corpus}");
    let configure = || {
        let mut cfg = ProvidersFile::builtin()
            .model_config(&Selection {
                provider_id: "openrouter".to_string(),
                model: model.clone(),
                api_key: Some(key.clone()),
                temperature: None,
                thinking: None,
                vision: None,
                context: None,
                api_base: None,
                cache_key: Some(format!("alter-zero-wire-history-{salt}")),
                service_tier: None,
            })
            .expect("openrouter is a built-in provider");
        cfg.extra_body
            .insert("max_tokens".into(), serde_json::json!(64));
        LlmBackend::configure(cfg, Some(system.clone()), true)
    };
    let usages = |events: &[StreamEvent]| -> Vec<alter_zero::stream::TokenUsage> {
        events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::Usage(usage) => Some(*usage),
                _ => None,
            })
            .collect()
    };

    let first = configure();
    let mut app = App::new();
    let prompt = format!(
        "Use the bash tool to run `echo run-{salt}` (one call), then reply with exactly DONE."
    );
    app.record_user_message(&prompt);
    app.begin_stream();
    let events = turn(&first, &prompt, context_messages(&app.history));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::ToolStart { .. })),
        "the model made a tool call: {events:?}"
    );
    for usage in usages(&events) {
        println!("tool turn usage: {usage:?}");
    }
    fold(&mut app, events);
    let tool = app
        .history
        .iter()
        .find_map(|item| match item {
            HistoryItem::Tool(tool) => Some(tool),
            _ => None,
        })
        .expect("the tool call was recorded");
    println!(
        "recorded call id: {:?}, batch: {:?}",
        tool.call_id, tool.batch
    );
    assert!(
        tool.call_id.is_some(),
        "the record carries the provider's id"
    );

    let rebuilt = configure();
    let prompt = "Now reply with exactly AGAIN.";
    app.record_user_message(prompt);
    app.begin_stream();
    let events = turn(&rebuilt, prompt, context_messages(&app.history));
    let usage = usages(&events)
        .first()
        .copied()
        .expect("the next turn reported usage");
    println!("rebuilt backend usage: {usage:?}");
    fold(&mut app, events);
    assert!(
        usage.cached * 10 >= usage.input * 9,
        "the rebuilt backend read the tool round back from cache: {usage:?}"
    );
}

#[test]
#[ignore = "hits the network; needs CHATGPT_CODEX_REFRESH_TOKEN + ALTER_ZERO_LIVE_TOKEN_STORE + ALTER_ZERO_LIVE_CHATGPT_MODEL"]
fn live_chatgpt_a_rebuilt_backend_reads_the_tool_round_back_from_cache() {
    // The Responses wire's durable half on OpenAI's cache: a real `bash`
    // turn whose result spans several 128-token cache blocks, folded into
    // the `App`, then a fresh backend — no retained request — sending the
    // next turn from the derived context alone. A read that stops at the
    // system prompt and the first user message would be a mismatch at the
    // tool round; a read through the tool result is the record replaying
    // what the provider cached (docs/prompt-caching.md). OpenAI's cache is
    // best-effort, so the assertion is the floor and the ratio is printed.
    use alter_zero::llm::{ProvidersFile, Selection};
    alter_zero::llm::chatgpt::set_store_path(
        std::env::var_os("ALTER_ZERO_LIVE_TOKEN_STORE")
            .map(std::path::PathBuf::from)
            .expect("set ALTER_ZERO_LIVE_TOKEN_STORE to a file the rotated refresh token can be written to"),
    );
    let refresh = std::env::var("CHATGPT_CODEX_REFRESH_TOKEN")
        .expect("set CHATGPT_CODEX_REFRESH_TOKEN to run this live test");
    let model = std::env::var("ALTER_ZERO_LIVE_CHATGPT_MODEL")
        .expect("set ALTER_ZERO_LIVE_CHATGPT_MODEL to a model available to your account");
    let salt = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let corpus: String = (0..420)
        .map(|i| format!("Calibration sentence {i} of the standing corpus, run {salt}. "))
        .collect();
    let system =
        format!("You are a terse assistant. Answer in as few words as possible.\n{corpus}");
    let configure = || {
        let cfg = ProvidersFile::builtin()
            .model_config(&Selection {
                provider_id: "chatgpt_codex".to_string(),
                model: model.clone(),
                api_key: Some(refresh.clone()),
                temperature: None,
                thinking: None,
                vision: None,
                context: None,
                api_base: None,
                cache_key: Some(format!("alter-zero-wire-history-{salt}")),
                service_tier: None,
            })
            .expect("chatgpt_codex is a built-in provider");
        LlmBackend::configure(cfg, Some(system.clone()), true)
    };
    let usages = |events: &[StreamEvent]| -> Vec<alter_zero::stream::TokenUsage> {
        events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::Usage(usage) => Some(*usage),
                _ => None,
            })
            .collect()
    };

    let first = configure();
    let mut app = App::new();
    let prompt = format!(
        "Use the bash tool to run `seq 1 800` (one call), then reply with exactly DONE. Run {salt}."
    );
    app.record_user_message(&prompt);
    app.begin_stream();
    let events = turn(&first, &prompt, context_messages(&app.history));
    let lead: String = events
        .iter()
        .take_while(|event| !matches!(event, StreamEvent::ToolStart { .. }))
        .filter_map(|event| match event {
            StreamEvent::Chunk(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    println!("text before the first call: {lead:?}");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::ToolStart { .. })),
        "the model made a tool call: {events:?}"
    );
    for usage in usages(&events) {
        println!("tool turn usage: {usage:?}");
    }
    fold(&mut app, events);
    let tool = app
        .history
        .iter()
        .find_map(|item| match item {
            HistoryItem::Tool(tool) => Some(tool),
            _ => None,
        })
        .expect("the tool call was recorded");
    println!(
        "recorded call id: {:?}, batch: {:?}, position: {:?}",
        tool.call_id, tool.batch, tool.position
    );
    assert!(
        tool.call_id.is_some(),
        "the record carries the provider's id"
    );

    let rebuilt = configure();
    let prompt = "Now reply with exactly AGAIN.";
    app.record_user_message(prompt);
    app.begin_stream();
    let events = turn(&rebuilt, prompt, context_messages(&app.history));
    let usage = usages(&events)
        .first()
        .copied()
        .expect("the next turn reported usage");
    println!(
        "rebuilt backend usage: {usage:?} (cached {}% of the input)",
        usage.cached * 100 / usage.input.max(1)
    );
    fold(&mut app, events);
    assert!(
        usage.cached > 1_000,
        "the rebuilt backend read the prefix back from cache: {usage:?}"
    );
}
