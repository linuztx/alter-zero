//! The `--max-turns` budget on a real backend (`docs/cli/headless.md`): an
//! [`LlmBackend`] carrying a [`RoundBudget`] against a stand-in provider on
//! the loopback, counting the requests that actually go out. The budget is
//! checked before a request is sent, so a reply past the limit never reaches
//! the provider — the turn ends on the budget's own message instead.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use alter_zero::llm::agent::RoundBudget;
use alter_zero::llm::{LlmBackend, ModelConfig};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};

/// A round in which the model reads a file that is not there — one call, so
/// the turn needs a second request to finish.
const TOOL_ROUND: &str = concat!(
    r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"/nonexistent/alter-zero-round-budget\"}"}}]}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
    "\n\n",
    "data: [DONE]\n\n",
);

/// A round that answers and ends the turn.
const TEXT_ROUND: &str = concat!(
    r#"data: {"choices":[{"delta":{"content":"done"}}]}"#,
    "\n\n",
    r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
    "\n\n",
    "data: [DONE]\n\n",
);

/// A stand-in that answers every request with the next of `rounds` — the
/// last one again once they run out — counting the requests it served.
fn stand_in(rounds: &'static [&'static str]) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let base = format!("http://{}/v1", listener.local_addr().expect("an address"));
    let served = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&served);
    std::thread::spawn(move || {
        for (n, stream) in listener.incoming().enumerate() {
            let Ok(mut stream) = stream else { return };
            read_request(&mut stream);
            count.fetch_add(1, Ordering::SeqCst);
            let body = rounds[n.min(rounds.len() - 1)];
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.flush();
        }
    });
    (base, served)
}

/// Read one request off `stream`: its headers, then its sized body.
fn read_request(stream: &mut TcpStream) {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).expect("request bytes");
        assert!(n > 0, "the request ended before its headers did");
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
    let length: usize = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    while bytes.len() < header_end + length {
        let n = stream.read(&mut chunk).expect("body bytes");
        assert!(n > 0, "the request ended before its body did");
        bytes.extend_from_slice(&chunk[..n]);
    }
}

fn backend(base: &str, budget: &RoundBudget) -> LlmBackend {
    let mut cfg = ModelConfig::fallback();
    cfg.api_base = base.to_string();
    cfg.api_key = Some("stand-in".to_string());
    LlmBackend::configure(cfg, Some("be terse".to_string()), true)
        .with_max_retries(0)
        .with_round_budget(budget.clone())
}

/// One turn's events, up to the terminal one.
fn turn(backend: &LlmBackend) -> Vec<StreamEvent> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn("go".to_string(), vec![], vec![], tx, CancelToken::new());
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

fn last(events: &[StreamEvent]) -> &StreamEvent {
    events.last().expect("a terminal event")
}

#[test]
fn a_budget_of_one_reply_never_sends_the_second() {
    let (base, served) = stand_in(&[TOOL_ROUND, TEXT_ROUND]);
    let budget = RoundBudget::new(NonZeroUsize::new(1));
    let events = turn(&backend(&base, &budget));
    assert!(
        matches!(last(&events), StreamEvent::Error(message)
            if message == "Stopped after 1 model reply — the run's --max-turns limit"),
        "{events:?}"
    );
    assert_eq!(
        served.load(Ordering::SeqCst),
        1,
        "the second request never went out"
    );
    assert_eq!(budget.used(), 1);
    assert!(budget.spent());
}

#[test]
fn a_budget_that_covers_the_turn_lets_it_finish_and_counts_every_reply() {
    let (base, served) = stand_in(&[TOOL_ROUND, TEXT_ROUND]);
    let budget = RoundBudget::new(NonZeroUsize::new(2));
    let events = turn(&backend(&base, &budget));
    assert!(
        matches!(last(&events), StreamEvent::StreamDone),
        "{events:?}"
    );
    assert_eq!(served.load(Ordering::SeqCst), 2);
    assert_eq!(budget.used(), 2);
    assert!(!budget.spent(), "nothing was refused");
}

#[test]
fn the_budget_spans_every_turn_of_the_run() {
    // The prompt's turn spends the budget; a follow-up turn on the same
    // backend is refused before it sends anything.
    let (base, served) = stand_in(&[TOOL_ROUND, TEXT_ROUND]);
    let budget = RoundBudget::new(NonZeroUsize::new(2));
    let backend = backend(&base, &budget);
    assert!(matches!(last(&turn(&backend)), StreamEvent::StreamDone));
    let events = turn(&backend);
    assert!(matches!(last(&events), StreamEvent::Error(_)), "{events:?}");
    assert_eq!(served.load(Ordering::SeqCst), 2);
}
