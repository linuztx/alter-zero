//! Live tests for stored secrets (`docs/secrets.md`): a real model, the real
//! executor, and the `<system-reminder>` a session leads with — the only place
//! the model learns a placeholder exists. What they prove is the part no unit
//! test can: that a model told about `<secrete:NAME>` actually **uses** it in
//! its tool calls, that the value reaches the tool, and that neither the
//! events the screen draws from, the shell reports, nor the model's own
//! answer ever carry it.
//!
//! All `#[ignore]`d so `cargo test` stays offline and deterministic. Run
//! explicitly with a real key (never committed — read from the environment):
//!
//! ```sh
//! A0_VENICE_API_KEY=sk-a0-… cargo test --test live_secrets -- --ignored --nocapture
//! ```
//!
//! `ALTER_ZERO_LIVE_VENICE_MODEL` overrides the model. The default is GLM
//! 4.7, not the `gpt-4o-mini` the other live suites use: every test here
//! passes on GLM 4.7, Gemini 3.8 Flash and Qwen3 Coder 480B, while
//! `gpt-4o-mini` reliably drops the closing quote of the JSON body it hands
//! `curl -d '…'` — a shell syntax error of its own that says nothing about
//! the feature.

use std::time::{Duration, Instant};

use alter_zero::app::App;
use alter_zero::background::{BackgroundRegistry, BgEvent};
use alter_zero::context::{ContextMessage, context_messages_full};
use alter_zero::llm::LlmBackend;
use alter_zero::secrets::{SecretDraft, SecretRegistry, SecretStore, SecretValue, secret_section};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};

/// The persona the tests run under: short, and silent about secrets — the
/// reminder alone must teach the placeholder.
const SYSTEM: &str = "You are a terse coding agent with shell and file tools. \
                      Do what is asked with your tools, then answer in one short sentence.";

fn key() -> String {
    std::env::var("A0_VENICE_API_KEY").expect("set A0_VENICE_API_KEY to run the secrets live tests")
}

/// What a test holds while its backend runs: the backend, the shell
/// registry's reports, and the directory its task logs go to (removed with
/// it — a log keeps a command's output raw).
struct Harness {
    backend: LlmBackend,
    reports: tokio::sync::mpsc::UnboundedReceiver<BgEvent>,
    _logs: tempfile::TempDir,
}

/// A **tools-on** backend on the Agent Zero / Venice provider carrying
/// `secrets`, with a shell registry for the interactive sessions — and no
/// permission gate, so the model's calls run unasked.
fn backend(secrets: &SecretRegistry) -> Harness {
    let model = std::env::var("ALTER_ZERO_LIVE_VENICE_MODEL")
        .unwrap_or_else(|_| "zai-org-glm-4.7".to_string());
    let providers = alter_zero::llm::ProvidersFile::builtin();
    let sel = alter_zero::llm::Selection {
        provider_id: "a0_venice".to_string(),
        model,
        api_key: Some(key()),
        temperature: Some(0.0),
        thinking: None,
        vision: None,
        context: None,
        api_base: None,
        cache_key: None,
        service_tier: None,
    };
    let cfg = providers.model_config(&sel).expect("a0_venice is built in");
    let (tx, reports) = tokio::sync::mpsc::unbounded_channel();
    let logs = tempfile::tempdir().expect("a temp dir");
    let registry =
        BackgroundRegistry::new(tx, logs.path().join("tasks")).with_secrets(secrets.clone());
    let backend = LlmBackend::configure(cfg, Some(SYSTEM.to_string()), true)
        .with_background(registry)
        .with_secrets(secrets.clone());
    Harness {
        backend,
        reports,
        _logs: logs,
    }
}

/// A registry holding one secret, as the `/secrete` page saves it.
fn one(name: &str, value: &str, context: &str) -> SecretRegistry {
    let mut store = SecretStore::new();
    store
        .apply(&SecretDraft {
            original: None,
            name: name.into(),
            value: Some(SecretValue::new(value)),
            context: context.into(),
        })
        .expect("a valid secret");
    SecretRegistry::new(store)
}

/// A value nothing else could produce, so finding it anywhere is a leak.
fn unique_value(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock")
        .as_nanos();
    format!("{prefix}-{nanos:x}")
}

/// The context the boundary derives for a first turn: the one reminder —
/// here the secrets section alone — ahead of the prompt.
fn context(secrets: &SecretRegistry, prompt: &str) -> Vec<ContextMessage> {
    let mut app = App::new();
    app.record_user_message(prompt);
    let listings = secret_section(&secrets.listing());
    context_messages_full(None, Some(&listings), &app.history)
}

/// Run one turn, returning every event and the reply's text.
fn turn(
    backend: &LlmBackend,
    secrets: &SecretRegistry,
    prompt: &str,
) -> (Vec<StreamEvent>, String) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(
        prompt.to_string(),
        vec![],
        context(secrets, prompt),
        tx,
        CancelToken::new(),
    );
    let mut events = Vec::new();
    let mut text = String::new();
    while let Some(event) = rx.blocking_recv() {
        match &event {
            StreamEvent::Chunk(chunk) => text.push_str(chunk),
            StreamEvent::ToolStart {
                name, arguments, ..
            } => {
                println!("call {name}: {}", arguments.as_deref().unwrap_or_default());
            }
            StreamEvent::ToolEnd { output, ok, .. } => println!("result ({ok}): {output}"),
            StreamEvent::ToolAnswered {
                display, result, ..
            } => {
                println!("result: {display}\n  model read: {result}");
            }
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            _ => {}
        }
        let done = matches!(event, StreamEvent::StreamDone);
        events.push(event);
        if done {
            break;
        }
    }
    handle.join().expect("the backend thread joins");
    println!("reply: {text:?}");
    (events, text)
}

/// The arguments of every call the model made, as it wrote them.
fn calls(events: &[StreamEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ToolStart { arguments, .. } => arguments.clone(),
            _ => None,
        })
        .collect()
}

/// Whether the model wrote `name`'s placeholder in any call, in either
/// spelling expansion accepts.
fn used_placeholder(events: &[StreamEvent], name: &str) -> bool {
    calls(events).iter().any(|call| {
        call.contains(&format!("<secrete:{name}>")) || call.contains(&format!("<secret:{name}>"))
    })
}

/// Neither the events, the shell reports nor the reply carry `value`.
fn assert_never_seen(
    events: &[StreamEvent],
    reports: &mut tokio::sync::mpsc::UnboundedReceiver<BgEvent>,
    reply: &str,
    value: &str,
) {
    let shown = format!("{events:?}");
    assert!(
        !shown.contains(value),
        "an event carried the value: {shown}"
    );
    let mut reported = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        match reports.try_recv() {
            Ok(event) => reported.push(event),
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    let reported = format!("{reported:?}");
    assert!(
        !reported.contains(value),
        "a shell report carried the value: {reported}"
    );
    assert!(
        !reply.contains(value),
        "the reply carried the value: {reply}"
    );
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_the_model_writes_a_stored_secret_through_its_placeholder() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let file = dir.path().join("key.txt");
    let value = unique_value("sk-demo");
    let secrets = one("DEMO_API_KEY", &value, "API key for the demo service");
    let prompt = format!(
        "Save the demo service's API key into {} — the file must contain only the key.",
        file.display()
    );
    let mut harness = backend(&secrets);
    let (events, reply) = turn(&harness.backend, &secrets, &prompt);

    assert_eq!(
        std::fs::read_to_string(&file)
            .expect("the model wrote the file")
            .trim(),
        value,
        "the tool acted on the value"
    );
    assert!(
        used_placeholder(&events, "DEMO_API_KEY"),
        "the model wrote the placeholder: {:?}",
        calls(&events)
    );
    assert_never_seen(&events, &mut harness.reports, &reply, &value);
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_a_file_holding_the_value_reads_back_as_its_placeholder() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let file = dir.path().join("config.env");
    let value = unique_value("tok");
    std::fs::write(&file, format!("SERVICE_TOKEN={value}\nMODE=dev\n")).expect("the fixture");
    let secrets = one("SERVICE_TOKEN", &value, "Token for the internal service");
    let prompt = format!(
        "Read {} and quote its first line to me exactly as it is written.",
        file.display()
    );
    let mut harness = backend(&secrets);
    let (events, reply) = turn(&harness.backend, &secrets, &prompt);

    assert!(
        reply.contains("<secrete:SERVICE_TOKEN>"),
        "the model quotes what it read — the placeholder: {reply}"
    );
    assert_never_seen(&events, &mut harness.reports, &reply, &value);
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_the_model_types_a_password_at_a_prompt_through_its_placeholder() {
    // A login script reads a password with echo off and checks it by hash,
    // so the value is in no command the model runs: only the keys it types
    // into the waiting session can carry it.
    let dir = tempfile::tempdir().expect("a temp dir");
    let value = unique_value("pw");
    let hash = {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg("printf '%s' \"$1\" | sha256sum | cut -c1-16")
            .arg("sh")
            .arg(&value)
            .output()
            .expect("sha256sum runs");
        String::from_utf8(output.stdout)
            .expect("hex")
            .trim()
            .to_string()
    };
    let script = dir.path().join("login.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             printf 'Password: '\n\
             stty -echo 2>/dev/null; read -r pw; stty echo 2>/dev/null; echo\n\
             if [ \"$(printf '%s' \"$pw\" | sha256sum | cut -c1-16)\" = {hash} ]; \
             then echo 'ACCESS GRANTED'; else echo 'ACCESS DENIED'; fi\n"
        ),
    )
    .expect("the script");
    let secrets = one("LOGIN_PASSWORD", &value, "Password login.sh asks for");
    let prompt = format!(
        "Run `sh {}` and log in when it asks for the password. Tell me what it said.",
        script.display()
    );
    let mut harness = backend(&secrets);
    let (events, reply) = turn(&harness.backend, &secrets, &prompt);

    let shown = format!("{events:?}");
    assert!(
        shown.contains("ACCESS GRANTED"),
        "the program got the value: {shown}"
    );
    assert!(
        used_placeholder(&events, "LOGIN_PASSWORD"),
        "the model typed the placeholder: {:?}",
        calls(&events)
    );
    assert_never_seen(&events, &mut harness.reports, &reply, &value);
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_a_misspelled_placeholder_never_reaches_the_file_as_text() {
    // The user names the secret wrong. Two answers are right: the model uses
    // the stored one (from the reminder, or from the refusal naming it), or
    // it stops and tells the user which one is stored. What must never
    // happen is the misspelled placeholder written as literal text — the
    // `.env` line that looks done — or the value anywhere the model sees.
    let dir = tempfile::tempdir().expect("a temp dir");
    let file = dir.path().join("token.txt");
    let value = unique_value("tok");
    let secrets = one("DEPLOY_TOKEN", &value, "Token for the deploy service");
    let prompt = format!(
        "Write <secrete:DEPLOY_TOKN> into {} — the file must contain only the token.",
        file.display()
    );
    let mut harness = backend(&secrets);
    let (events, reply) = turn(&harness.backend, &secrets, &prompt);

    let refused = format!("{events:?}").contains("is not a stored secret");
    println!("refused first: {refused}");
    match std::fs::read_to_string(&file) {
        Ok(written) => assert_eq!(
            written.trim(),
            value,
            "the file holds the value, not the misspelled placeholder"
        ),
        Err(_) => assert!(
            refused && reply.contains("<secrete:DEPLOY_TOKEN>"),
            "no file, so the model must have been refused and named the stored secret: {reply}"
        ),
    }
    assert_never_seen(&events, &mut harness.reports, &reply, &value);
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_the_model_uses_a_real_key_for_a_real_request_without_seeing_it() {
    // The whole point: a real credential doing real work. The key this test
    // runs on is stored as a secret and the model is asked to make an
    // authenticated request with it — to the Agent Zero proxy, which answers
    // 401 to a wrong key, so a reply carrying the proxied model's word is
    // proof the value reached the request while it never reached the model.
    let key = key();
    let secrets = one(
        "A0_API_KEY",
        &key,
        "Agent Zero API key — an OpenAI-compatible Bearer token for https://api.agent-zero.ai/venice/v1",
    );
    let prompt = "Use curl and the stored Agent Zero API key to send a chat completion request to \
                  https://api.agent-zero.ai/venice/v1/chat/completions with model llama-3.2-3b and \
                  the user message 'Reply with just the word PINEAPPLE'. Then tell me exactly what \
                  that model replied.";
    let mut harness = backend(&secrets);
    let (events, reply) = turn(&harness.backend, &secrets, prompt);

    assert!(
        used_placeholder(&events, "A0_API_KEY"),
        "the model sent the placeholder: {:?}",
        calls(&events)
    );
    assert!(
        reply.to_ascii_uppercase().contains("PINEAPPLE"),
        "the authenticated request worked and the model relayed its answer: {reply}"
    );
    assert_never_seen(&events, &mut harness.reports, &reply, &key);
}
