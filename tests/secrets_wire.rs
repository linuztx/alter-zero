//! Stored secrets, end to end (`docs/secrets.md`): a real [`LlmBackend`] turn
//! with the real executor against a provider stand-in on the loopback. The
//! stand-in's model writes placeholders into its tool calls; what is checked
//! is the whole promise — the tool acted on the **value**, while not one event
//! the screen draws from, not one background-shell report and not one byte of
//! any request carries it.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alter_zero::app::App;
use alter_zero::background::{BackgroundRegistry, BgEvent};
use alter_zero::context::context_messages_full;
use alter_zero::llm::{LlmBackend, ModelConfig, WireApi};
use alter_zero::permission::PermissionGate;
use alter_zero::secrets::{SecretDraft, SecretRegistry, SecretStore, SecretValue, secret_section};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};

/// The value the stand-in's model never learns.
const VALUE: &str = "s3cr3t-demo-value-9";

/// The session's one stored secret, as the `/secrete` page saves it.
fn secrets() -> SecretRegistry {
    let mut store = SecretStore::new();
    store
        .apply(&SecretDraft {
            original: None,
            name: "DEMO_TOKEN".into(),
            value: Some(SecretValue::new(VALUE)),
            context: "The demo API token".into(),
        })
        .expect("a valid secret");
    SecretRegistry::new(store)
}

/// One Chat Completions round calling `name` with `arguments`.
fn tool_round(id: &str, name: &str, arguments: &serde_json::Value) -> String {
    let delta = serde_json::json!({
        "choices": [{"delta": {"tool_calls": [{
            "index": 0,
            "id": id,
            "type": "function",
            "function": {"name": name, "arguments": arguments.to_string()},
        }]}}]
    });
    format!(
        "data: {delta}\n\n\
         data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\n\
         data: [DONE]\n\n"
    )
}

/// A plain-text round that ends the turn.
fn text_round(text: &str) -> String {
    let delta = serde_json::json!({"choices": [{"delta": {"content": text}}]});
    format!(
        "data: {delta}\n\n\
         data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\n\
         data: [DONE]\n\n"
    )
}

/// One answer of the stand-in, made from the request it answers — a later
/// round can name a session id only the previous result told it.
type Round = Box<dyn FnOnce(&str) -> String + Send>;

/// A round that answers `body` whatever it was asked.
fn fixed(body: String) -> Round {
    Box::new(move |_| body)
}

/// A provider stand-in: answers each request with the next round's body and
/// keeps every request body it was sent, raw.
fn stand_in(rounds: Vec<Round>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let base = format!(
        "http://{}/v1",
        listener.local_addr().expect("the bound address")
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    std::thread::spawn(move || {
        for round in rounds {
            let (mut stream, _) = listener.accept().expect("a connection");
            let request = read_body(&mut stream);
            let response = round(&request);
            seen.lock().expect("the request log").push(request);
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
                response.len()
            );
            stream.write_all(reply.as_bytes()).expect("the reply");
            stream.flush().expect("the flush");
        }
    });
    (base, requests)
}

/// One request's body, read off `stream` by its `content-length`.
fn read_body(stream: &mut TcpStream) -> String {
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
        .expect("a sized body");
    while bytes.len() < header_end + length {
        let n = stream.read(&mut chunk).expect("body bytes");
        assert!(n > 0, "the request ended before its body did");
        bytes.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8_lossy(&bytes[header_end..header_end + length]).into_owned()
}

/// The context the boundary derives for a first turn on `prompt`: the
/// `<system-reminder>` carrying the secrets section, then the message.
fn first_turn_context(
    secrets: &SecretRegistry,
    prompt: &str,
) -> Vec<alter_zero::context::ContextMessage> {
    let mut app = App::new();
    app.record_user_message(prompt);
    let listings = secret_section(&secrets.listing());
    context_messages_full(None, Some(&listings), &app.history)
}

/// One turn's events, up to the terminal one. A permission prompt fails the
/// test at once — the turn is cancelled first, so the backend thread is
/// never left waiting on an answer nobody will give.
fn turn(backend: &LlmBackend, secrets: &SecretRegistry, prompt: &str) -> Vec<StreamEvent> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancelToken::new();
    let handle = backend.spawn(
        prompt.to_string(),
        vec![],
        first_turn_context(secrets, prompt),
        tx,
        cancel.clone(),
    );
    let mut events = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        if let StreamEvent::Permission(request) = &event {
            cancel.cancel();
            let _ = handle.join();
            panic!("a prompt was raised: {request:?}");
        }
        let terminal = matches!(event, StreamEvent::StreamDone | StreamEvent::Error(_));
        events.push(event);
        if terminal {
            break;
        }
    }
    handle.join().expect("the backend thread joins");
    events
}

/// A tools-on backend pointed at the stand-in, carrying `secrets`.
fn backend(base: &str, secrets: &SecretRegistry) -> LlmBackend {
    let mut cfg = ModelConfig::fallback();
    cfg.api_base = base.to_string();
    cfg.api_key = Some("stand-in".to_string());
    cfg.wire_api = WireApi::Chat;
    LlmBackend::configure(cfg, Some("be terse".to_string()), true)
        .with_max_retries(0)
        .with_secrets(secrets.clone())
}

/// A shell registry carrying `secrets` — the boundary's wiring
/// (`tui::models::session_backend`) — and its event stream.
fn registry(
    secrets: &SecretRegistry,
    dir: &std::path::Path,
) -> (
    BackgroundRegistry,
    tokio::sync::mpsc::UnboundedReceiver<BgEvent>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let registry = BackgroundRegistry::new(tx, dir.join("tasks")).with_secrets(secrets.clone());
    (registry, rx)
}

/// Every request the stand-in was sent, each checked free of the value.
fn assert_no_request_carries_the_value(requests: &Arc<Mutex<Vec<String>>>) {
    for (round, request) in requests.lock().expect("the request log").iter().enumerate() {
        assert!(
            !request.contains(VALUE),
            "request {round} carried the value: {request}"
        );
    }
}

/// The first `ToolEnd`'s output and whether it resolved green.
fn first_tool_end(events: &[StreamEvent]) -> (String, bool) {
    events
        .iter()
        .find_map(|event| match event {
            StreamEvent::ToolEnd { output, ok, .. } => Some((output.clone(), *ok)),
            _ => None,
        })
        .expect("a call resolved")
}

#[test]
fn a_command_gets_the_value_while_the_screen_and_the_model_get_the_placeholder() {
    let secrets = secrets();
    let dir = tempfile::tempdir().expect("a temp dir");
    let file = dir.path().join("token.txt");
    let command = format!(
        "printf 'token=%s\\n' '<secrete:DEMO_TOKEN>' > '{0}' && cat '{0}'",
        file.display()
    );
    let (base, requests) = stand_in(vec![
        fixed(tool_round(
            "call_1",
            "bash",
            &serde_json::json!({"command": command, "description": "Use the token"}),
        )),
        fixed(text_round("done")),
    ]);
    let events = turn(&backend(&base, &secrets), &secrets, "use the demo token");

    // The command acted on the real value…
    assert_eq!(
        std::fs::read_to_string(&file).expect("the command wrote its file"),
        format!("token={VALUE}\n")
    );
    // …while nothing the screen draws from ever held it.
    let shown = format!("{events:?}");
    assert!(
        !shown.contains(VALUE),
        "an event carried the value: {shown}"
    );
    let (output, ok) = first_tool_end(&events);
    assert!(ok, "{output}");
    assert!(
        output.contains("token=<secrete:DEMO_TOKEN>"),
        "the output names the placeholder: {output}"
    );
    // The header and the recorded arguments keep what the model wrote.
    assert!(
        events.iter().any(|event| matches!(
            event,
            StreamEvent::ToolStart { arguments: Some(arguments), .. }
                if arguments.contains("<secrete:DEMO_TOKEN>")
        )),
        "{shown}"
    );

    // The model was told the placeholder and its context, and got the
    // masked result back — never the value.
    assert_no_request_carries_the_value(&requests);
    let requests = requests.lock().expect("the request log");
    assert_eq!(requests.len(), 2, "one tool round, then the answer");
    assert!(
        requests[0].contains("- <secrete:DEMO_TOKEN>: The demo API token"),
        "the reminder lists the secret: {}",
        requests[0]
    );
    assert!(requests[1].contains("token=<secrete:DEMO_TOKEN>"));
}

#[test]
fn a_file_written_through_a_placeholder_reads_back_masked_and_edits_through_it() {
    let secrets = secrets();
    let dir = tempfile::tempdir().expect("a temp dir");
    let file = dir.path().join(".env");
    let path = file.display().to_string();
    let (base, requests) = stand_in(vec![
        fixed(tool_round(
            "call_w",
            "write",
            &serde_json::json!({"path": path, "content": "API_KEY=<secrete:DEMO_TOKEN>\nMODE=dev\n"}),
        )),
        fixed(tool_round(
            "call_r",
            "read",
            &serde_json::json!({"path": path}),
        )),
        fixed(tool_round(
            "call_e",
            "edit",
            &serde_json::json!({
                "path": path,
                "old_string": "API_KEY=<secrete:DEMO_TOKEN>\nMODE=dev",
                "new_string": "API_KEY=<secrete:DEMO_TOKEN>\nMODE=prod",
            }),
        )),
        fixed(text_round("done")),
    ]);
    let events = turn(&backend(&base, &secrets), &secrets, "configure the app");

    // The file holds the value — written, then edited through the
    // placeholder the model read back.
    assert_eq!(
        std::fs::read_to_string(&file).expect("the file exists"),
        format!("API_KEY={VALUE}\nMODE=prod\n")
    );
    let shown = format!("{events:?}");
    assert!(
        !shown.contains(VALUE),
        "an event carried the value: {shown}"
    );
    assert_no_request_carries_the_value(&requests);
    let requests = requests.lock().expect("the request log");
    assert_eq!(requests.len(), 4);
    // What the read handed the model is the placeholder, in the file's shape.
    assert!(
        requests[2].contains("API_KEY=<secrete:DEMO_TOKEN>"),
        "{}",
        requests[2]
    );
}

#[test]
fn a_password_typed_through_a_placeholder_reaches_the_program_alone() {
    // The login the feature exists for: a program reads a password with
    // echo off, the model types the placeholder, the program checks what it
    // got — by hash, so the value is in no argument the model wrote.
    let secrets = secrets();
    let dir = tempfile::tempdir().expect("a temp dir");
    let (registry, mut bg) = registry(&secrets, dir.path());
    let hash = sha256_prefix(VALUE);
    let command = format!(
        "read -r -s -p 'Password: ' pw; echo; \
         if [ \"$(printf '%s' \"$pw\" | sha256sum | cut -c1-16)\" = {hash} ]; \
         then echo ACCESS GRANTED; else echo ACCESS DENIED; fi"
    );
    let (base, requests) = stand_in(vec![
        fixed(tool_round(
            "call_login",
            "bash",
            &serde_json::json!({"command": command, "description": "Log in"}),
        )),
        // The session id is only known once the login is waiting.
        Box::new(|request: &str| {
            tool_round(
                "call_type",
                "bashsend",
                &serde_json::json!({
                    "session_id": session_named_in(request),
                    "input": "<secrete:DEMO_TOKEN>\n",
                }),
            )
        }),
        fixed(text_round("logged in")),
    ]);
    let backend = backend(&base, &secrets).with_background(registry);
    let events = turn(&backend, &secrets, "log in with the demo token");

    let shown = format!("{events:?}");
    assert!(
        shown.contains("ACCESS GRANTED"),
        "the program got the value: {shown}"
    );
    assert!(
        !shown.contains(VALUE),
        "an event carried the value: {shown}"
    );
    assert_no_request_carries_the_value(&requests);
    let reported = format!("{:?}", events_within(&mut bg, Duration::from_millis(300)));
    assert!(
        !reported.contains(VALUE),
        "a shell report carried the value: {reported}"
    );
}

#[test]
fn a_background_command_is_named_by_its_placeholder() {
    // Everything that names a background shell — the ↓ manager, the
    // completion notice, the model's completion note — reads its command
    // and its description as the model wrote them.
    let secrets = secrets();
    let dir = tempfile::tempdir().expect("a temp dir");
    let (registry, mut bg) = registry(&secrets, dir.path());
    let (base, requests) = stand_in(vec![
        fixed(tool_round(
            "call_bg",
            "bash",
            &serde_json::json!({
                "command": "printf 'bg=%s\\n' '<secrete:DEMO_TOKEN>'",
                "description": "Print <secrete:DEMO_TOKEN>",
                "wait": 0,
            }),
        )),
        fixed(text_round("started")),
    ]);
    let backend = backend(&base, &secrets).with_background(registry);
    let events = turn(&backend, &secrets, "start it in the background");
    assert!(
        !format!("{events:?}").contains(VALUE),
        "an event carried the value: {events:?}"
    );

    let reports = events_until_exit(&mut bg);
    let started = reports
        .iter()
        .find_map(|event| match event {
            BgEvent::Started {
                command,
                description,
                ..
            } => Some((command.clone(), description.clone())),
            _ => None,
        })
        .expect("the shell was announced");
    assert_eq!(started.0, "printf 'bg=%s\\n' '<secrete:DEMO_TOKEN>'");
    assert_eq!(started.1.as_deref(), Some("Print <secrete:DEMO_TOKEN>"));
    // What it printed — a terminal's screen, or a pipe's lines — shows the
    // placeholder where the value was.
    assert!(
        reports.iter().any(|event| match event {
            BgEvent::Screen { text, .. } => text.contains("bg=<secrete:DEMO_TOKEN>"),
            BgEvent::Output { chunk, .. } => chunk.contains("bg=<secrete:DEMO_TOKEN>"),
            _ => false,
        }),
        "{reports:?}"
    );
    assert!(
        !format!("{reports:?}").contains(VALUE),
        "a shell report carried the value: {reports:?}"
    );
    assert_no_request_carries_the_value(&requests);
}

#[test]
fn a_placeholder_naming_nothing_stored_is_refused_before_anything_runs() {
    let secrets = secrets();
    let dir = tempfile::tempdir().expect("a temp dir");
    let marker = dir.path().join("ran");
    let command = format!("touch '{}' && echo <secrete:DEMO_TOKN>", marker.display());
    let (base, requests) = stand_in(vec![
        fixed(tool_round(
            "call_1",
            "bash",
            &serde_json::json!({"command": command}),
        )),
        fixed(text_round("done")),
    ]);
    let events = turn(&backend(&base, &secrets), &secrets, "try it");
    assert!(!marker.exists(), "the command must not have run");
    let (refusal, ok) = first_tool_end(&events);
    assert!(!ok, "{refusal}");
    assert!(
        refusal.starts_with("Not run: <secrete:DEMO_TOKN> is not a stored secret.")
            && refusal.contains("Stored: <secrete:DEMO_TOKEN>."),
        "{refusal}"
    );
    // The model reads the refusal and can correct itself in one step.
    let requests = requests.lock().expect("the request log");
    assert!(requests[1].contains("Stored: <secrete:DEMO_TOKEN>."));
}

#[test]
fn a_refused_placeholder_never_asks_for_permission() {
    // Behind the permission gate the refusal comes first: nothing of the
    // call may run, so the user is never asked about it (`turn` fails the
    // test on a prompt).
    let secrets = secrets();
    let dir = tempfile::tempdir().expect("a temp dir");
    let marker = dir.path().join("ran");
    let command = format!("touch '{}' && echo <secrete:DEMO_TOKN>", marker.display());
    let (base, _requests) = stand_in(vec![
        fixed(tool_round(
            "call_1",
            "bash",
            &serde_json::json!({"command": command}),
        )),
        fixed(text_round("done")),
    ]);
    let backend = backend(&base, &secrets).with_permissions(PermissionGate::new());
    let events = turn(&backend, &secrets, "try it");
    assert!(!marker.exists(), "the command must not have run");
    let refusal = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::ToolRejected { display, .. } => Some(display.clone()),
            _ => None,
        })
        .expect("the call was refused");
    assert!(
        refusal.starts_with("Not run: <secrete:DEMO_TOKN> is not a stored secret."),
        "{refusal}"
    );
}

/// The first 16 hex digits of `text`'s SHA-256 — `sha256sum | cut -c1-16`,
/// computed by the tool itself so the test adds no dependency.
fn sha256_prefix(text: &str) -> String {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg("printf '%s' \"$1\" | sha256sum | cut -c1-16")
        .arg("sh")
        .arg(text)
        .output()
        .expect("sha256sum runs");
    String::from_utf8(output.stdout)
        .expect("hex")
        .trim()
        .to_string()
}

/// The session id the last tool result in `request` names — the `Running
/// (session …` frame a waiting command reports.
fn session_named_in(request: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(request).expect("a JSON request");
    let result = value["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .and_then(|message| message["content"].as_str())
        .expect("a tool result")
        .to_string();
    let first = result.lines().next().unwrap_or_default();
    match alter_zero::pty::report::parse_frame(first) {
        Some(alter_zero::pty::report::Frame::Running { session, .. }) => session.to_string(),
        other => panic!("not a running frame ({other:?}): {result}"),
    }
}

/// Every shell report that arrives within `window`.
fn events_within(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<BgEvent>,
    window: Duration,
) -> Vec<BgEvent> {
    let deadline = Instant::now() + window;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(event) => events.push(event),
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    events
}

/// The shell reports up to the first exit (ten seconds at most).
fn events_until_exit(rx: &mut tokio::sync::mpsc::UnboundedReceiver<BgEvent>) -> Vec<BgEvent> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(event) => {
                let exited = matches!(event, BgEvent::Exited { .. });
                events.push(event);
                if exited {
                    return events;
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    panic!("the shell never exited: {events:?}");
}
