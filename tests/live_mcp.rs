//! Live MCP tests (`docs/mcp.md`) — the real DeepWiki server, and a real
//! model driving a **parallel batch** of its tools through the production
//! pipeline: `McpManager` connect → `LlmBackend::spawn` → the tool loop →
//! the `StreamEvent`s the TUI folds into `App` → the very lines the strip,
//! the permission prompt and scrollback render.
//!
//! All `#[ignore]`d so `cargo test` stays offline and deterministic. Run them
//! explicitly:
//!
//! ```sh
//! OPENROUTER_API_KEY=sk-or-… cargo test --test live_mcp -- --ignored --nocapture
//! ```
//!
//! The server-only test needs no key at all:
//!
//! ```sh
//! cargo test --test live_mcp -- --ignored --nocapture live_deepwiki_tool_descriptions
//! ```
//!
//! `ALTER_ZERO_LIVE_MODEL` overrides the model (default `openai/gpt-4o-mini`).

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use alter_zero::app::{App, ToolStatus};
use alter_zero::llm::mcp::{McpManager, McpSources};
use alter_zero::llm::{LlmBackend, ModelConfig};
use alter_zero::permission::{PermissionKind, PermissionRequest, options, title};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent, ToolCallSummary};
use alter_zero::{mcp, ui};

/// The public DeepWiki MCP server — streamable HTTP, no authentication.
/// A **legacy** (handshake) server: it settles at `2025-11-25`.
const DEEPWIKI_URL: &str = "https://mcp.deepwiki.com/mcp";

/// A public **dual-era** server — it serves the modern `server/discover`
/// *and* still answers the legacy `initialize`, which is precisely the shape
/// that a remembered era verdict used to pin a revision behind
/// (`docs/mcp.md`). No authentication.
const DUAL_ERA_URL: &str = "https://docs.mcp.cloudflare.com/mcp";

/// The repository the prompts ask about (the user's own, in the report this
/// work came from).
const REPO: &str = "linuztx/flaredantic";

/// A manager holding just DeepWiki, connected (or panicking with why).
fn connected_deepwiki() -> McpManager {
    connected_server("deepwiki", DEEPWIKI_URL)
}

/// A manager holding one remote server, connected (or panicking with why).
fn connected_server(name: &str, url: &str) -> McpManager {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let manager = McpManager::new(
        tx,
        McpSources {
            entries: vec![mcp::McpServerEntry {
                name: name.to_string(),
                config: mcp::McpServerConfig::Http {
                    url: url.to_string(),
                    headers: Default::default(),
                    sse_fallback: false,
                },
                scope: mcp::McpScope::User,
                config_path: "<test>".to_string(),
            }],
            errors: Vec::new(),
            disabled: BTreeSet::new(),
            untrusted: BTreeSet::new(),
            project: ".".to_string(),
            user_file: None,
            auth_path: None,
            cwd: None,
            startup_timeout: Duration::from_secs(30),
            tool_timeout: Duration::from_secs(120),
        },
    );
    manager.start_connections();
    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline {
        if manager.has_tools() {
            return manager;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "{name} never connected: {:?}",
        manager.snapshot().first().map(|s| s.status_line())
    );
}

/// The model under test.
fn live_model() -> String {
    std::env::var("ALTER_ZERO_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string())
}

/// An OpenRouter backend with the DeepWiki tools attached and permissions
/// off — the prompt shape is asserted separately, from the request itself.
fn backend_with_deepwiki(manager: McpManager) -> LlmBackend {
    let key =
        std::env::var("OPENROUTER_API_KEY").expect("set OPENROUTER_API_KEY to run the live tests");
    let cfg = ModelConfig {
        provider_id: "openrouter".to_string(),
        provider_name: "OpenRouter".to_string(),
        model: live_model(),
        api_base: "https://openrouter.ai/api/v1".to_string(),
        api_model_base: "https://openrouter.ai/api/v1".to_string(),
        api_key: Some(key),
        auth: alter_zero::llm::AuthScheme::ApiKey,
        wire_api: alter_zero::llm::WireApi::Chat,
        temperature: Some(0.0),
        thinking: None,
        vision: None,
        context: None,
        cache_key: None,
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
    };
    LlmBackend::with_system_prompt(cfg, Some("You are a terse assistant.".to_string()))
        .with_mcp(manager)
}

/// Drain a spawned turn's events into a vec (bounded by `budget`).
fn run_turn(backend: &LlmBackend, prompt: &str, budget: Duration) -> Vec<StreamEvent> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancelToken::new();
    let handle = backend.spawn(
        prompt.to_string(),
        Vec::new(),
        Vec::new(),
        tx,
        cancel.clone(),
    );
    let deadline = Instant::now() + budget;
    let mut events = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(event) => {
                let done = matches!(event, StreamEvent::StreamDone | StreamEvent::Error(_));
                events.push(event);
                if done {
                    break;
                }
            }
            Err(_) if Instant::now() > deadline => {
                cancel.cancel();
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let _ = handle.join();
    events
}

/// The plain text of a rendered line.
fn plain(line: &ratatui::text::Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// The **painted** top row of the live region — the strip's first line, which
/// for an in-flight MCP batch is its one aggregated cell (`docs/mcp.md`).
/// Rendered through the real widget, not a helper, so this is what a terminal
/// would show.
fn live_strip_top(app: &App, width: u16) -> String {
    let height = ui::live_height(
        &app.input,
        width,
        40,
        ui::strip_has_status(app),
        ui::preview_rows(app, width),
        ui::task_rows(app, width),
        ui::queued_rows(app, width),
        ui::toast_rows(app),
        0,
        ui::footer_rows(app, 0),
        ui::agent_list_rows(app),
    );
    let area = ratatui::layout::Rect::new(0, 0, width, height);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    ui::render_live(area, &mut buf, app);
    (0..width)
        .map(|x| buf[(x, 0)].symbol())
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The revision on the `/mcp` page is whatever the server settled on in
/// **this** connect (`docs/mcp.md`). Two live servers, two eras, so the pair
/// can tell a working client from the one this replaced: a remembered
/// `legacy` verdict reported the dual-era server at the legacy revision it
/// still serves, and every offline fixture in the world agreed with it
/// because a fixture is written by whoever writes the test.
#[test]
#[ignore = "hits the network (docs.mcp.cloudflare.com, mcp.deepwiki.com)"]
fn live_each_server_reports_the_revision_it_settled_on() {
    let dual = connected_server("cfdocs", DUAL_ERA_URL);
    let modern = dual.snapshot()[0]
        .identity
        .clone()
        .expect("the dual-era server initialized");
    println!("{DUAL_ERA_URL} -> {}", modern.protocol_version);
    assert_eq!(
        modern.protocol_version,
        mcp::PROTOCOL_VERSION,
        "a server serving `server/discover` must be reported at the modern \
         revision, not the legacy one it also still answers"
    );
    dual.shutdown();

    // The other half of the claim: a legacy server is not simply reported at
    // whatever we propose. DeepWiki has no `server/discover`, so the
    // handshake settles the revision — a revision we speak, from its answer.
    let legacy = connected_deepwiki();
    let identity = legacy.snapshot()[0]
        .identity
        .clone()
        .expect("deepwiki initialized");
    println!("{DEEPWIKI_URL} -> {}", identity.protocol_version);
    assert_ne!(identity.protocol_version, mcp::PROTOCOL_VERSION);
    assert!(
        mcp::SUPPORTED_VERSIONS.contains(&identity.protocol_version.as_str()),
        "a settled revision is one we speak, never blank or invented: {:?}",
        identity.protocol_version
    );
    legacy.shutdown();
}

#[test]
#[ignore = "hits the network (mcp.deepwiki.com)"]
fn live_deepwiki_tool_descriptions_drive_the_permission_prompt() {
    // The prompt's body is the server's own words: no key needed, just the
    // server (`docs/mcp.md`).
    let manager = connected_deepwiki();
    let wire = mcp::tool_wire_name("deepwiki", "read_wiki_structure");
    let description = manager
        .tool_description(&wire)
        .expect("the connected server describes its tool");
    println!("description: {description}");
    assert!(!description.trim().is_empty());

    let request = PermissionRequest {
        id: "perm_0".to_string(),
        kind: PermissionKind::Mcp,
        target: wire,
        body: mcp::pretty_args(&format!(r#"{{"repoName":"{REPO}"}}"#)),
        detail: Some(description.clone()),
        agent: None,
        agent_id: None,
    };
    let mut app = App::new();
    app.set_session_info("live", "~/Codes/tests");
    app.open_permission(request.clone());
    let rows: Vec<String> = ui::permission_lines(&app, 76, 30)
        .iter()
        .map(plain)
        .collect();
    println!("{}", rows.join("\n"));
    assert_eq!(title(PermissionKind::Mcp), "Tool use");
    assert!(
        rows.iter().any(|r| r.trim_start().starts_with(&format!(
            "Deepwiki - read_wiki_structure(repoName: \"{REPO}\") (MCP)"
        ))),
        "the call reads as the cell will: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains(&description)),
        "the server's own description rides the prompt: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.trim() == "Do you want to proceed?"),
        "{rows:?}"
    );
    assert_eq!(
        options(&request, Some("~/Codes/tests"))[1],
        "Yes, and don't ask again for Deepwiki - read_wiki_structure commands in ~/Codes/tests"
    );
    manager.shutdown();
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_a_parallel_deepwiki_batch_renders_as_one_cell() {
    // The whole point of the aggregation: a real model, a real parallel
    // batch, and the lines the TUI would actually paint (`docs/mcp.md`).
    let manager = connected_deepwiki();
    let backend = backend_with_deepwiki(manager.clone());
    let events = run_turn(
        &backend,
        &format!(
            "Call the deepwiki tools read_wiki_structure and read_wiki_contents for the \
             repository {REPO}. Issue BOTH calls in the same turn, in parallel, then answer \
             in one sentence."
        ),
        Duration::from_secs(180),
    );

    // Fold them into `App` exactly as `tui::stream` does, rendering at each
    // step the way the loop would.
    let mut app = App::new();
    let mut committed: Vec<String> = Vec::new();
    let mut strips: Vec<String> = Vec::new();
    for event in &events {
        match event {
            StreamEvent::ToolBatch(items) => {
                let summaries: Vec<ToolCallSummary> = items.to_vec();
                app.start_tool_batch(&summaries);
            }
            StreamEvent::ToolStart {
                name,
                args,
                arguments,
                ..
            } => {
                app.start_tool(name, args, arguments.as_deref());
                strips.push(live_strip_top(&app, 80));
            }
            StreamEvent::ToolEnd { output, ok, .. } => {
                app.end_tool(output, *ok);
                if let Some(lines) = ui::tool_commit_lines(&app.history, app.tool_queue(), 80) {
                    committed.extend(lines.iter().map(plain));
                }
            }
            StreamEvent::Error(message) => panic!("the turn failed: {message}"),
            _ => {}
        }
    }
    let calls: Vec<&str> = app
        .history
        .iter()
        .filter_map(|item| match item {
            alter_zero::app::HistoryItem::Tool(tool) => Some(tool.name.as_str()),
            _ => None,
        })
        .collect();
    println!("calls: {calls:?}");
    println!("strip: {strips:?}");
    println!("committed: {committed:?}");
    assert!(
        calls.len() >= 2 && calls.iter().all(|name| mcp::is_mcp_display_name(name)),
        "the model was asked for two MCP calls: {calls:?}"
    );
    // One live line for the batch, counting the batch…
    assert!(
        strips
            .iter()
            .any(|row| row.starts_with("● Calling Deepwiki 2 times…")),
        "{strips:?}"
    );
    // …and one committed line when the run ends, with nothing else between.
    assert_eq!(
        committed,
        vec!["Called Deepwiki 2 times (ctrl+o to expand)".to_string()],
        "the parallel run commits once"
    );
    // The Ctrl+O story is still per call, with the model's own argument
    // order in the header.
    let expanded: Vec<String> = ui::transcript_lines(&app, 100).iter().map(plain).collect();
    println!("{}", expanded.join("\n"));
    assert!(
        expanded
            .iter()
            .any(|row| row.contains("Deepwiki - read_wiki_structure (MCP)(repoName:")),
        "{expanded:?}"
    );
    // …and every resolved call is green, not a failure.
    assert!(
        app.history.iter().all(|item| match item {
            alter_zero::app::HistoryItem::Tool(tool) => tool.status == ToolStatus::Ok,
            _ => true,
        }),
        "a failed call would have rendered loudly instead"
    );
    manager.shutdown();
}

#[test]
#[ignore = "hits the network; needs OPENROUTER_API_KEY"]
fn live_auto_mode_classifies_an_mcp_call_instead_of_prompting() {
    // The reported gap (docs/mcp.md): in auto mode a server tool used to
    // raise the permission prompt anyway. It must ride the classifier like a
    // command — no `Permission` event, the `ToolNote` provenance row on the
    // allowed call, and a green resolution.
    use alter_zero::permission::{PermissionGate, PermissionMode};
    let manager = connected_deepwiki();
    let gate = PermissionGate::new();
    gate.set_mode(PermissionMode::Auto);
    let backend = backend_with_deepwiki(manager.clone()).with_permissions(gate);
    let events = run_turn(
        &backend,
        &format!(
            "Call the deepwiki tool read_wiki_structure for the repository {REPO}, then \
             answer in one sentence."
        ),
        Duration::from_secs(180),
    );
    let mut notes = Vec::new();
    let mut ends = Vec::new();
    for event in &events {
        match event {
            StreamEvent::Permission(request) => panic!("auto mode must not prompt: {request:?}"),
            StreamEvent::ToolNote(note) => notes.push(note.clone()),
            StreamEvent::ToolEnd { ok, .. } => ends.push(*ok),
            StreamEvent::Error(message) => panic!("the turn failed: {message}"),
            _ => {}
        }
    }
    println!("notes: {notes:?}, ends: {ends:?}");
    assert!(
        notes.iter().any(|n| n == "Allowed by auto mode classifier"),
        "the classifier cleared the call: {notes:?}"
    );
    assert!(ends.iter().any(|ok| *ok), "the MCP call ran: {ends:?}");
    manager.shutdown();
}
