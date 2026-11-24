//! Compaction **while the agent works**, end to end (`docs/compact.md`): a
//! real [`LlmBackend`] turn against a stand-in provider on the loopback
//! whose context window is small and *enforced* — a request past it is
//! refused, as a real one is — with the turn's events folded into the
//! [`App`] the way the boundary folds them.
//!
//! What has to hold: a turn many times longer than the window runs to its
//! end; it compacts as often as it needs, each summary read from the one
//! before; the summarizer is sent a transcript and no tools; and the context
//! the *next* turn derives from the history is the one this turn's last
//! request carried, so nothing the model had is lost — or doubled — at the
//! turn boundary.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alter_zero::app::{App, HistoryItem};
use alter_zero::compaction::{
    CompactionControl, CompactionSeed, DEFAULT_USER_MESSAGE_BUDGET, user_message_budget,
};
use alter_zero::context::{ContextMessage, context_messages, recent_user_texts};
use alter_zero::llm::{LlmBackend, ModelConfig};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};

/// The model's window, in tokens, as the session knows it.
const WINDOW: u64 = 24_000;

/// What the stand-in counts a token as: the file the turn keeps reading is
/// prose of about six bytes a token, so a request of `WINDOW` tokens is one
/// of this many bytes.
const BYTES_PER_TOKEN: usize = 6;

/// How many tool rounds the stand-in asks for before it answers — several
/// windows' worth of reads.
const ROUNDS: usize = 30;

/// What a prompt says to make the stand-in answer at once.
const ANSWER_NOW: &str = "ANSWER_NOW";

/// One request the stand-in received.
#[derive(Debug, Clone)]
struct Seen {
    body: serde_json::Value,
    bytes: usize,
}

impl Seen {
    /// A summarization: the one kind of request that offers no tools.
    fn is_summary(&self) -> bool {
        self.body["tools"].as_array().is_none_or(Vec::is_empty)
    }

    fn messages(&self) -> &[serde_json::Value] {
        self.body["messages"].as_array().map_or(&[], Vec::as_slice)
    }

    /// The text of the request's `index`th message.
    fn text(&self, index: usize) -> String {
        self.messages()
            .get(index)
            .and_then(|message| message["content"].as_str())
            .unwrap_or_default()
            .to_string()
    }
}

/// The stand-in provider's state.
struct Provider {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Provider {
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("the request log").clone()
    }

    fn summaries(&self) -> Vec<Seen> {
        self.seen().into_iter().filter(Seen::is_summary).collect()
    }

    fn agent_requests(&self) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|seen| !seen.is_summary())
            .collect()
    }
}

/// Start the stand-in: a Chat Completions endpoint that
///
/// - **refuses** an agent request larger than `limit_bytes` with the words a
///   real provider uses, when there is a limit;
/// - answers a request offering **no tools** with `SUMMARY {n}`, after
///   `summary_delay` — long enough that the agent runs on meanwhile;
/// - answers an agent request with a `read` of `path`, until it has asked
///   `ROUNDS` times or the newest user message says [`ANSWER_NOW`], and then
///   with a line of text.
fn provider(path: &str, limit_bytes: Option<usize>, summary_delay: Duration) -> Provider {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free loopback port");
    let base = format!(
        "http://{}/v1",
        listener.local_addr().expect("the bound address")
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let asked = Arc::new(AtomicUsize::new(0));
    let summarized = Arc::new(AtomicUsize::new(0));
    let path = path.to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let log = Arc::clone(&log);
            let asked = Arc::clone(&asked);
            let summarized = Arc::clone(&summarized);
            let path = path.clone();
            std::thread::spawn(move || {
                let Some((body, bytes)) = read_request(&mut stream) else {
                    return;
                };
                let request = Seen { body, bytes };
                log.lock().expect("the request log").push(request.clone());
                let reply = if request.is_summary() {
                    std::thread::sleep(summary_delay);
                    let number = summarized.fetch_add(1, Ordering::SeqCst) + 1;
                    Reply::Text(format!("SUMMARY {number}"))
                } else if limit_bytes.is_some_and(|limit| bytes > limit) {
                    Reply::TooLong
                } else if request
                    .messages()
                    .iter()
                    .rev()
                    .find(|message| message["role"] == "user")
                    .is_some_and(|message| message["content"].to_string().contains(ANSWER_NOW))
                    || asked.load(Ordering::SeqCst) >= ROUNDS
                {
                    Reply::Text("All of it is read.".to_string())
                } else {
                    let number = asked.fetch_add(1, Ordering::SeqCst) + 1;
                    Reply::Read {
                        id: format!("call_{number}"),
                        path,
                    }
                };
                // What a provider reports for the request: its size, in the
                // stand-in's tokens.
                let _ = stream.write_all(reply.http(bytes / BYTES_PER_TOKEN).as_bytes());
                let _ = stream.flush();
            });
        }
    });
    Provider { base, seen }
}

enum Reply {
    Text(String),
    Read { id: String, path: String },
    TooLong,
}

impl Reply {
    fn http(&self, prompt_tokens: usize) -> String {
        let frames = match self {
            Self::TooLong => {
                let body = r#"{"error":{"message":"This model's maximum context length is 24000 tokens. Please reduce the length of the messages.","code":"context_length_exceeded"}}"#;
                return format!(
                    "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
            Self::Text(text) => vec![
                serde_json::json!({"choices": [{"delta": {"content": text}}]}),
                serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
            ],
            Self::Read { id, path } => vec![
                serde_json::json!({"choices": [{"delta": {"tool_calls": [{
                    "index": 0, "id": id, "type": "function",
                    "function": {
                        "name": "read",
                        "arguments": serde_json::json!({"path": path}).to_string(),
                    },
                }]}}]}),
                serde_json::json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
            ],
        };
        let usage = serde_json::json!({
            "choices": [],
            "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": 8},
        });
        let body: String = frames
            .iter()
            .chain(std::iter::once(&usage))
            .map(|frame| format!("data: {frame}\n\n"))
            .chain(std::iter::once("data: [DONE]\n\n".to_string()))
            .collect();
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }
}

/// One HTTP request's JSON body off `stream`, and how many bytes it was.
/// `None` for a connection that closed before sending one.
fn read_request(stream: &mut TcpStream) -> Option<(serde_json::Value, usize)> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let header_end = loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
    let length: usize = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())?;
    while bytes.len() < header_end + length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    let body = serde_json::from_slice(&bytes[header_end..header_end + length]).ok()?;
    Some((body, length))
}

/// A file of prose the turn reads again and again: about 2,500 tokens of it.
fn prose_file(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "alter-zero-compaction-{}-{name}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("a temp directory");
    let path = dir.join("notes.txt");
    std::fs::write(
        &path,
        "lorem ipsum dolor sit amet consectetur adipiscing elit sed do\n".repeat(250),
    )
    .expect("the file is written");
    path
}

/// A backend pointed at the stand-in, tools on, no retries, no ceiling on
/// tool calls (the app's own default — the library's is twenty), compacting
/// under `control`.
fn backend(base: &str, control: &CompactionControl) -> LlmBackend {
    let mut cfg = ModelConfig::fallback();
    cfg.api_base = base.to_string();
    cfg.api_key = Some("stand-in".to_string());
    LlmBackend::configure(cfg, Some("be terse".to_string()), true)
        .with_max_retries(0)
        .with_max_tool_calls(0)
        .with_compaction(control.clone())
}

/// One turn for `prompt`: recorded and spawned the way the boundary does it
/// — the seed set first — its events folded into `app` as they arrive.
/// Returns how the turn ended.
fn turn(
    app: &mut App,
    backend: &LlmBackend,
    control: &CompactionControl,
    prompt: &str,
) -> StreamEvent {
    app.record_user_message(prompt);
    app.begin_stream();
    control.set_seed(CompactionSeed {
        leading: None,
        user_texts: recent_user_texts(&app.history, DEFAULT_USER_MESSAGE_BUDGET),
    });
    let context: Vec<ContextMessage> = context_messages(&app.history);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let end = loop {
        let event = rx.blocking_recv().expect("the turn ends on an event");
        if matches!(event, StreamEvent::StreamDone | StreamEvent::Error(_)) {
            fold(app, event.clone());
            break event;
        }
        fold(app, event);
    };
    handle.join().expect("the backend thread joins");
    end
}

/// Fold one event into `app` the way `tui::stream` does: the text before a
/// call finalised, each call recorded, and a compaction's marker landed
/// where the boundary lands it.
fn fold(app: &mut App, event: StreamEvent) {
    match event {
        StreamEvent::Chunk(text) => app.push_chunk(&text),
        StreamEvent::RoundCalls(calls) => app.open_round(calls),
        StreamEvent::ToolBatch(items) => {
            app.flush_streaming_segment();
            app.start_tool_batch(&items);
        }
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
        StreamEvent::HookNote {
            label,
            text,
            report,
        } => {
            app.flush_streaming_segment();
            app.record_hook_note(&label, &text, report);
        }
        StreamEvent::Usage(usage) => app.apply_usage(&usage),
        StreamEvent::CompactionStarted { auto } => {
            app.flush_streaming_segment();
            app.begin_live_compaction(auto);
        }
        StreamEvent::CompactionWaiting => app.wait_on_compaction(),
        StreamEvent::CompactionUsage(usage) => app.apply_compaction_usage(&usage),
        StreamEvent::Compacted {
            summary,
            secs,
            budget,
        } => {
            app.flush_streaming_segment();
            app.land_compaction(&summary, secs, budget);
        }
        StreamEvent::CompactionFailed { .. } => app.end_live_compaction(),
        StreamEvent::StreamDone => {
            app.finish_stream();
            app.end_turn(0);
        }
        StreamEvent::Error(error) => {
            app.fail_stream(&error);
        }
        _ => {}
    }
}

/// The compaction markers `app` recorded, oldest first.
fn markers(app: &App) -> Vec<alter_zero::app::Compaction> {
    app.history
        .iter()
        .filter_map(|item| match item {
            HistoryItem::Compaction(compaction) => Some(compaction.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_turn_many_windows_long_runs_to_its_end() {
    // Thirty reads of 2,500 tokens through a 24,000-token window that the
    // provider enforces: without compaction the tenth request is refused.
    let path = prose_file("long");
    let limit = usize::try_from(WINDOW).expect("fits") * BYTES_PER_TOKEN;
    let provider = provider(
        path.to_str().expect("a UTF-8 path"),
        Some(limit),
        Duration::from_millis(150),
    );
    let control = CompactionControl::new(true);
    control.set_window(Some(WINDOW));
    let backend = backend(&provider.base, &control);
    let mut app = App::new();
    app.set_context_window(Some(WINDOW));

    let end = turn(
        &mut app,
        &backend,
        &control,
        "read the notes until you know them",
    );
    assert_eq!(end, StreamEvent::StreamDone, "the turn finished");

    // It asked for every round, and no request was ever refused for its size.
    let agent = provider.agent_requests();
    assert_eq!(agent.len(), ROUNDS + 1, "every round, then the answer");
    assert!(
        agent.iter().all(|request| request.bytes <= limit),
        "largest request: {} of {limit} bytes",
        agent.iter().map(|request| request.bytes).max().unwrap_or(0)
    );

    // It compacted more than once — as often as it needed — and each summary
    // was written over the one before.
    let summaries = provider.summaries();
    assert!(summaries.len() >= 3, "{} summaries", summaries.len());
    for (index, summary) in summaries.iter().enumerate() {
        assert_eq!(
            summary.messages().len(),
            2,
            "the system prompt and one message"
        );
        assert_eq!(summary.messages()[0]["role"], "system");
        let request = summary.text(1);
        assert!(
            request.starts_with("<conversation>\n[user]\n"),
            "{}",
            &request[..80]
        );
        assert!(request.contains("read the notes until you know them"));
        assert!(
            request.contains("CONTEXT CHECKPOINT COMPACTION"),
            "the prompt closes it"
        );
        if index > 0 {
            assert!(
                request.contains(&format!("SUMMARY {index}")),
                "summary {} was written over summary {index}",
                index + 1
            );
        }
    }

    // The history records each one where it landed, as a compaction made
    // mid-turn: the bridge says carry on, the budget is the window's share.
    let markers = markers(&app);
    assert_eq!(markers.len(), summaries.len());
    for (index, marker) in markers.iter().enumerate() {
        assert_eq!(marker.summary, format!("SUMMARY {}", index + 1));
        assert!(marker.resume && marker.auto, "{marker:?}");
        assert_eq!(marker.budget, user_message_budget(Some(WINDOW)));
        assert!(marker.after < marker.before, "{marker:?}");
    }
    // Every record is still there for the user: nothing was rewritten.
    let reads = app
        .history
        .iter()
        .filter(|item| matches!(item, HistoryItem::Tool(_)))
        .count();
    assert_eq!(reads, ROUNDS);
}

#[test]
fn the_next_turn_derives_the_context_the_compacted_turn_ended_on() {
    // What the history gives back has to be what the wire carried: the
    // user's message and the last summary first, then the rounds the summary
    // never covered — or the turn boundary would lose them, or send them
    // twice.
    let path = prose_file("derive");
    let limit = usize::try_from(WINDOW).expect("fits") * BYTES_PER_TOKEN;
    let provider = provider(
        path.to_str().expect("a UTF-8 path"),
        Some(limit),
        Duration::from_millis(150),
    );
    let control = CompactionControl::new(true);
    control.set_window(Some(WINDOW));
    let backend = backend(&provider.base, &control);
    let mut app = App::new();
    app.set_context_window(Some(WINDOW));

    assert_eq!(
        turn(
            &mut app,
            &backend,
            &control,
            "read the notes until you know them"
        ),
        StreamEvent::StreamDone
    );
    let last = provider
        .agent_requests()
        .pop()
        .expect("the turn's last request");
    let before = provider.agent_requests().len();

    assert_eq!(
        turn(
            &mut app,
            &backend,
            &control,
            &format!("{ANSWER_NOW} and now?")
        ),
        StreamEvent::StreamDone
    );
    let next = provider.agent_requests()[before].clone();

    // The compacted prefix, word for word: the kept message, the bridge.
    assert_eq!(next.text(1), last.text(1));
    let newest = markers(&app).pop().expect("a compaction");
    assert!(
        next.text(1).contains(&newest.summary),
        "the newest summary bridges: {}",
        newest.summary
    );
    assert!(
        next.text(1)
            .starts_with("read the notes until you know them\n\n")
    );

    // Then everything the last request carried, in order — the rounds since
    // the summary — then the turn's closing reply and the new message.
    let carried = last.messages().len();
    assert_eq!(
        &next.messages()[..carried],
        last.messages(),
        "the same messages, call ids and all"
    );
    assert_eq!(next.messages().len(), carried + 2);
    assert_eq!(next.messages()[carried]["content"], "All of it is read.");
    assert!(
        next.text(carried + 1).contains(ANSWER_NOW),
        "{}",
        next.text(carried + 1)
    );
}

#[test]
fn a_request_the_provider_refuses_for_its_size_is_made_room_for() {
    // Nobody told the session how large the window is, so nothing is
    // started by the gauge — the provider's refusal is what says so, and the
    // turn compacts and sends the request again instead of dying on it.
    let path = prose_file("overflow");
    let limit = usize::try_from(WINDOW).expect("fits") * BYTES_PER_TOKEN;
    let provider = provider(
        path.to_str().expect("a UTF-8 path"),
        Some(limit),
        Duration::ZERO,
    );
    let control = CompactionControl::new(true);
    let backend = backend(&provider.base, &control);
    let mut app = App::new();

    let end = turn(
        &mut app,
        &backend,
        &control,
        "read the notes until you know them",
    );
    assert_eq!(end, StreamEvent::StreamDone, "{:?}", app.history.last());
    assert!(
        provider.agent_requests().iter().any(|r| r.bytes > limit),
        "a request did overflow"
    );
    assert!(!markers(&app).is_empty(), "and room was made for it");
    assert!(
        !app.history.iter().any(|item| matches!(
            item,
            HistoryItem::Message(message) if message.role == alter_zero::app::Role::Error
        )),
        "no error reached the conversation"
    );
}

#[test]
fn with_auto_compaction_off_an_overflow_ends_the_turn_as_it_always_did() {
    let path = prose_file("off");
    let limit = usize::try_from(WINDOW).expect("fits") * BYTES_PER_TOKEN;
    let provider = provider(
        path.to_str().expect("a UTF-8 path"),
        Some(limit),
        Duration::ZERO,
    );
    let control = CompactionControl::new(false);
    control.set_window(Some(WINDOW));
    let backend = backend(&provider.base, &control);
    let mut app = App::new();
    app.set_context_window(Some(WINDOW));

    let end = turn(
        &mut app,
        &backend,
        &control,
        "read the notes until you know them",
    );
    let StreamEvent::Error(error) = end else {
        panic!("the turn should have failed: {end:?}");
    };
    assert!(error.contains("maximum context length"), "{error}");
    assert!(provider.summaries().is_empty(), "nothing was summarized");
    assert!(markers(&app).is_empty());
}

#[test]
fn a_compact_typed_mid_turn_is_made_at_the_next_step() {
    // No window, auto compaction off: only the user's own `/compact`.
    let path = prose_file("manual");
    let provider = provider(path.to_str().expect("a UTF-8 path"), None, Duration::ZERO);
    let control = CompactionControl::new(false);
    let backend = backend(&provider.base, &control);
    let mut app = App::new();
    control.request();

    assert_eq!(
        turn(&mut app, &backend, &control, &format!("{ANSWER_NOW} hello")),
        StreamEvent::StreamDone
    );
    // The request was taken at the turn's first boundary and landed before
    // the turn ended; the marker says the user asked.
    let markers = markers(&app);
    assert_eq!(markers.len(), 1, "{:?}", app.history);
    assert!(!markers[0].auto);
    assert!(!control.requested(), "taken");
    assert_eq!(provider.summaries().len(), 1);
}
