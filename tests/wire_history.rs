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
use alter_zero::context::{ContextMessage, context_messages};
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
        let terminal = matches!(event, StreamEvent::StreamDone | StreamEvent::Error(_));
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
/// the closing reply recorded on `StreamDone`.
fn fold(app: &mut App, events: Vec<StreamEvent>) {
    for event in events {
        match event {
            StreamEvent::Chunk(text) => app.push_chunk(&text),
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

/// Two human turns on `wire` — the first a tool round then a reply, the
/// second a reply — returning the request that closed the first turn and
/// the request that opened the second.
fn two_turns(
    wire: WireApi,
    rounds: &'static [&'static str],
) -> (serde_json::Value, serde_json::Value) {
    let (base, requests) = stand_in(rounds);
    let mut cfg = ModelConfig::fallback();
    cfg.api_base = base;
    cfg.api_key = Some("stand-in".to_string());
    cfg.wire_api = wire;
    let backend =
        LlmBackend::configure(cfg, Some("be terse".to_string()), true).with_max_retries(0);

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
