//! The task guard, end to end (`docs/task-tools.md`): a real [`LlmBackend`]
//! main turn with the task list attached, against a provider stand-in on the
//! loopback. The stand-in's model plans, works without marking the task in
//! progress, then answers — gpt-oss:120b's measured lapse — and what is
//! checked is what the next requests carry: the list in a short
//! `<system-reminder>` right after the work, and once more before the turn
//! is allowed to end.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use alter_zero::context::{ContextMessage, ContextRole};
use alter_zero::llm::{LlmBackend, ModelConfig, WireApi};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};
use alter_zero::tasks::{TASK_REMINDER_LABEL, TaskNudge, TaskRegistry, task_reminder};

/// The plan: one task, created.
const CREATE_ROUND: &str = concat!(
    r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_create","type":"function","function":{"name":"taskcreate","arguments":"{\"subject\":\"Write the page\",\"description\":\"A login form\"}"}}]}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
    "\n\n",
    "data: [DONE]\n\n",
);

/// The work, with the task never marked in progress.
const WORK_ROUND: &str = concat!(
    r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_read","type":"function","function":{"name":"read","arguments":"{\"path\":\"/nonexistent/alter-zero-task-guard\"}"}}]}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
    "\n\n",
    "data: [DONE]\n\n",
);

/// The answer the model would end its turn on.
const ANSWER_ROUND: &str = concat!(
    r#"data: {"choices":[{"delta":{"content":"Done."}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
    "\n\n",
    "data: [DONE]\n\n",
);

/// Its reply to the closing reminder: nothing more to say.
const SILENT_ROUND: &str = concat!(
    r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
    "\n\n",
    "data: [DONE]\n\n",
);

/// A provider stand-in: answers each request with the next response and
/// keeps every request body it was sent.
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

/// One turn's events, up to the terminal one.
fn turn(backend: &LlmBackend, prompt: &str) -> Vec<StreamEvent> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
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

/// The (role, content) of a request's last `n` messages.
fn tail(request: &serde_json::Value, n: usize) -> Vec<(String, String)> {
    let messages = request["messages"].as_array().expect("a messages array");
    messages[messages.len() - n..]
        .iter()
        .map(|message| {
            (
                message["role"].as_str().unwrap_or_default().to_string(),
                message["content"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[test]
fn a_main_turn_shows_the_model_its_list_after_work_and_before_it_ends() {
    let (base, requests) = stand_in(&[CREATE_ROUND, WORK_ROUND, ANSWER_ROUND, SILENT_ROUND]);
    let mut cfg = ModelConfig::fallback();
    cfg.api_base = base;
    cfg.api_key = Some("stand-in".to_string());
    cfg.wire_api = WireApi::Chat;
    let tasks = TaskRegistry::new();
    let backend = LlmBackend::configure(cfg, Some("be terse".to_string()), true)
        .with_max_retries(0)
        .with_tasks(tasks.clone());

    let events = turn(&backend, "build a login page");

    let list = tasks.snapshot();
    let idle = task_reminder(TaskNudge::Idle, &list);
    let closing = task_reminder(TaskNudge::Closing, &list);
    let requests = requests.lock().expect("the request log");
    assert_eq!(requests.len(), 4, "plan, work, answer, the closing round");
    assert_eq!(
        tail(&requests[2], 2),
        vec![
            ("tool".to_string(), tail(&requests[2], 2)[0].1.clone()),
            ("user".to_string(), idle.clone()),
        ],
        "the work's result, then the list"
    );
    assert_eq!(
        tail(&requests[3], 2),
        vec![
            ("assistant".to_string(), "Done.".to_string()),
            ("user".to_string(), closing.clone()),
        ],
        "the answer it meant to end on, then the list once more"
    );
    let notes: Vec<(&str, &str)> = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::HookNote { label, text } => Some((label.as_str(), text.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        notes,
        vec![
            (TASK_REMINDER_LABEL, idle.as_str()),
            (TASK_REMINDER_LABEL, closing.as_str()),
        ],
        "both recorded where the model read them"
    );
    assert_eq!(events.last(), Some(&StreamEvent::StreamDone));
}
