//! Live integration tests for the **agent companions** (`docs/agent-tools.md`)
//! — `agentsend`, `agentwait`, `agentoutput`, `agentkill`, `agentlist` —
//! driven by a real model on the real wire, through the production backend.
//!
//! What a unit test cannot show is a *model* reading the launch's id back
//! out of the acknowledgement and handing it to the companions, each of
//! whose results it then has to make sense of — the whole round trip the
//! tools exist for. Each test here asks for a fixed sequence of calls and
//! asserts on the cells the lead's reply channel carried: the names, the
//! frames, and the registry's own state at the end.
//!
//! All `#[ignore]`d so `cargo test` stays offline and deterministic. Run
//! explicitly with a real key (never committed — read from the environment):
//!
//! ```sh
//! A0_VENICE_API_KEY=sk-a0-… cargo test --test live_agent_tools -- --ignored --nocapture
//! OLLAMA_API_KEY=…          cargo test --test live_agent_tools -- --ignored --nocapture ollama
//! ```
//!
//! `ALTER_ZERO_LIVE_VENICE_MODEL` / `ALTER_ZERO_LIVE_OLLAMA_CLOUD_MODEL`
//! override the models (their catalogs churn).

use alter_zero::agents::{AgentEvent, AgentRegistry, AgentStatus};
use alter_zero::context::{ContextMessage, ContextRole};
use alter_zero::llm::{LlmBackend, ProvidersFile, Selection};
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};
use alter_zero::subagents::SubagentRegistry;

/// A backend on `provider_id` with `model`, tools on, agents attached, and
/// the agent channel's receiver so the test can read what the agents did.
fn backend_with_agents(
    provider_id: &str,
    key_env: &str,
    model: String,
) -> (
    LlmBackend,
    AgentRegistry,
    tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
) {
    let key = std::env::var(key_env)
        .unwrap_or_else(|_| panic!("set {key_env} to run the agent-tools live tests"));
    let providers = ProvidersFile::builtin();
    let sel = Selection {
        provider_id: provider_id.to_string(),
        model,
        api_key: Some(key),
        temperature: Some(0.0),
        thinking: None,
        vision: None,
        context: None,
        api_base: None,
        cache_key: None,
        service_tier: None,
    };
    let cfg = providers
        .model_config(&sel)
        .unwrap_or_else(|| panic!("{provider_id} is built in"));
    let (bg_tx, _bg_rx) = tokio::sync::mpsc::unbounded_channel();
    let scratch = std::env::temp_dir().join(format!(
        "alter-zero-live-agent-tools-{}",
        std::process::id()
    ));
    let background = alter_zero::background::BackgroundRegistry::new(bg_tx, scratch);
    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let agents = AgentRegistry::new(agent_tx);
    let (found, errors) =
        alter_zero::llm::subagent::with_builtins(alter_zero::llm::subagent::discover_agents(&[]));
    assert!(
        errors.is_empty(),
        "the built-in definitions parse: {errors:?}"
    );
    let backend = LlmBackend::configure(
        cfg,
        Some(
            "You are a terse assistant that follows tool instructions exactly, one tool call \
             per step, and never explains in between."
                .to_string(),
        ),
        /*tools_enabled=*/ true,
    )
    .with_background(background)
    .with_agents(agents.clone())
    .with_subagents(SubagentRegistry::new(found));
    (backend, agents, agent_rx)
}

fn venice() -> (
    LlmBackend,
    AgentRegistry,
    tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
) {
    let model = std::env::var("ALTER_ZERO_LIVE_VENICE_MODEL")
        .unwrap_or_else(|_| "openai-gpt-4o-mini-2024-07-18".to_string());
    backend_with_agents("a0_venice", "A0_VENICE_API_KEY", model)
}

fn ollama_cloud() -> (
    LlmBackend,
    AgentRegistry,
    tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
) {
    let model = std::env::var("ALTER_ZERO_LIVE_OLLAMA_CLOUD_MODEL")
        .unwrap_or_else(|_| "gpt-oss:20b".to_string());
    backend_with_agents("ollama_cloud", "OLLAMA_API_KEY", model)
}

/// One resolved cell of the lead's turn: the display name, the header
/// summary, and the result text the model read.
#[derive(Debug, Clone)]
struct Cell {
    name: String,
    args: String,
    output: String,
    ok: bool,
}

/// What one turn produced: the lead's resolved cells in order, the launch
/// acknowledgements (`AgentGroupDone` outputs), and the final reply text.
struct Turn {
    cells: Vec<Cell>,
    launches: Vec<String>,
    reply: String,
}

/// Run one turn and collect the cells the reply channel carried — printing
/// each as it resolves, so a failing run reads like a transcript.
fn run_turn(backend: &LlmBackend, prompt: &str) -> Turn {
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let mut cells = Vec::new();
    let mut launches = Vec::new();
    let mut reply = String::new();
    let mut open: Option<(String, String)> = None;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(text) => reply.push_str(&text),
            StreamEvent::ToolStart { name, args, .. } => {
                println!("● {name}({args})");
                open = Some((name, args));
            }
            StreamEvent::ToolEnd { output, ok, .. } => resolve(&mut open, &mut cells, output, ok),
            StreamEvent::ToolAnswered { result, .. } => {
                resolve(&mut open, &mut cells, result, true)
            }
            StreamEvent::ToolRejected { result, .. } => {
                resolve(&mut open, &mut cells, result, false)
            }
            StreamEvent::AgentGroupDone { agents, .. } => {
                for done in agents {
                    println!("● agent {} → {}", done.id, done.output.replace('\n', " "));
                    launches.push(done.output);
                }
            }
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("reply: {reply}");
    Turn {
        cells,
        launches,
        reply,
    }
}

/// Close the open cell with its result, printing it the way the cell reads.
fn resolve(open: &mut Option<(String, String)>, cells: &mut Vec<Cell>, output: String, ok: bool) {
    if let Some((name, args)) = open.take() {
        println!("  ⎿ {}", output.replace('\n', "\n    "));
        cells.push(Cell {
            name,
            args,
            output,
            ok,
        });
    }
}

/// The agent id a launch acknowledgement names (`Agent a… launched …`).
fn launched_id(ack: &str) -> &str {
    ack.strip_prefix("Agent ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("the launch acknowledgement names no id: {ack}"))
}

/// The companions' whole round trip on one provider: a background launch,
/// `agentoutput`, `agentwait` for the result, `agentsend` with a follow-up
/// the finished agent answers on its own conversation, `agentwait` again,
/// `agentlist`.
fn companions_control_a_background_agent(
    (backend, agents, mut agent_rx): (
        LlmBackend,
        AgentRegistry,
        tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    ),
) {
    let prompt = "Do exactly these steps, in order, one tool call per step, and do not \
        skip any:\n\
        1. Call the agent tool with description \"Count entries\", run_in_background true, \
        and this exact prompt: \"Run `ls -1 | wc -l` in the working directory and reply \
        with one sentence stating the number of entries.\"\n\
        2. Call agentoutput with the agent id the launch returned.\n\
        3. Call agentwait on that agent id with wait 180.\n\
        4. Call agentsend on that agent id with the message \"Now also tell me the absolute \
        path of the working directory, in one sentence.\"\n\
        5. Call agentwait on that agent id with wait 180.\n\
        6. Call agentlist.\n\
        Then reply with exactly one word: done.";
    let turn = run_turn(&backend, prompt);
    assert_eq!(turn.launches.len(), 1, "one launch: {:?}", turn.launches);
    let ack = &turn.launches[0];
    assert!(
        ack.contains("launched in the background") && ack.contains("agentwait"),
        "the launch acknowledgement names the companions: {ack}"
    );
    let id = launched_id(ack).to_string();
    assert!(id.starts_with('a') && id.len() == 9, "a registry id: {id}");
    let names: Vec<&str> = turn.cells.iter().map(|cell| cell.name.as_str()).collect();
    println!("cells: {names:?}");
    for wanted in ["AgentOutput", "AgentWait", "AgentSend", "AgentList"] {
        assert!(
            names.contains(&wanted),
            "the model called {wanted}: {names:?}"
        );
    }
    // Every companion cell names the agent by its task, the bashsend way.
    for cell in turn
        .cells
        .iter()
        .filter(|cell| cell.name != "AgentList" && cell.name.starts_with("Agent"))
    {
        assert!(
            cell.args.starts_with("Count entries"),
            "{} names the agent by its description: {}",
            cell.name,
            cell.args
        );
    }
    let output = turn
        .cells
        .iter()
        .find(|cell| cell.name == "AgentOutput")
        .expect("an agentoutput cell");
    assert!(
        output.output.starts_with(&format!("Running (agent {id})"))
            || output.output.starts_with(&format!("Done (agent {id})")),
        "agentoutput's frame names the agent: {}",
        output.output
    );
    let waits: Vec<&Cell> = turn
        .cells
        .iter()
        .filter(|cell| cell.name == "AgentWait")
        .collect();
    assert!(!waits.is_empty(), "agentwait was called");
    let settled = waits
        .iter()
        .find(|cell| cell.output.starts_with(&format!("Done (agent {id})")))
        .unwrap_or_else(|| panic!("an agentwait returned the result: {waits:?}"));
    assert!(
        settled.output.contains("Response:"),
        "the wait carries the response: {}",
        settled.output
    );
    assert!(
        settled.output.contains("Bash("),
        "the report lists the agent's calls: {}",
        settled.output
    );
    let send = turn
        .cells
        .iter()
        .find(|cell| cell.name == "AgentSend")
        .expect("an agentsend cell");
    assert!(send.ok, "agentsend succeeded: {}", send.output);
    assert!(
        send.output.contains(&id) && send.output.contains("new turn of its conversation"),
        "agentsend continued the finished agent: {}",
        send.output
    );
    let list = turn
        .cells
        .iter()
        .find(|cell| cell.name == "AgentList")
        .expect("an agentlist cell");
    assert!(
        list.output.starts_with("1 agent:") && list.output.contains(&id),
        "agentlist names the one agent: {}",
        list.output
    );
    assert!(
        turn.reply.to_lowercase().contains("done"),
        "the turn closed: {}",
        turn.reply
    );
    // The registry's own state: the agent finished its continuation too,
    // keeping the calls of both turns on one record.
    let snapshot = agents.snapshot(&id).expect("the finished agent is kept");
    assert_eq!(snapshot.status, AgentStatus::Done, "{snapshot:?}");
    assert!(
        snapshot.tool_uses >= 1,
        "the agent ran at least one command: {snapshot:?}"
    );
    // …and the follow-up reached the agent as a Steered event on its own
    // channel — the echo that lands it on the roster's transcript.
    let mut steered = Vec::new();
    while let Ok(event) = agent_rx.try_recv() {
        if let AgentEvent::Stream {
            event: StreamEvent::Steered { text },
            ..
        } = event
        {
            steered.push(text);
        }
    }
    assert!(
        steered.iter().any(|text| text.contains("absolute path")),
        "the follow-up was announced on the agent channel: {steered:?}"
    );
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_venice_companions_control_a_background_agent() {
    companions_control_a_background_agent(venice());
}

#[test]
#[ignore = "hits the network; needs OLLAMA_API_KEY"]
fn live_ollama_cloud_companions_control_a_background_agent() {
    companions_control_a_background_agent(ollama_cloud());
}

/// `agentkill` on a running agent: the report says `Stopped`, the registry
/// marks it killed, and a message to it is refused.
fn agentkill_stops_a_running_agent(
    (backend, agents, _agent_rx): (
        LlmBackend,
        AgentRegistry,
        tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    ),
) {
    let prompt = "Do exactly these steps, in order, one tool call per step:\n\
        1. Call the agent tool with description \"Sleep a while\", run_in_background true, \
        and this exact prompt: \"Run `sleep 240` with wait 300, then reply with the word \
        slept.\"\n\
        2. Call agentkill with the agent id the launch returned.\n\
        3. Call agentsend on that agent id with the message \"are you there?\"\n\
        4. Call agentlist.\n\
        Then reply with exactly one word: done.";
    let turn = run_turn(&backend, prompt);
    assert_eq!(turn.launches.len(), 1, "one launch: {:?}", turn.launches);
    let id = launched_id(&turn.launches[0]).to_string();
    let kill = turn
        .cells
        .iter()
        .find(|cell| cell.name == "AgentKill")
        .expect("an agentkill cell");
    assert!(
        kill.output.starts_with(&format!("Stopped (agent {id})")),
        "the kill reports the stop: {}",
        kill.output
    );
    assert!(agents.is_killed(&id), "the registry marks it killed");
    if let Some(send) = turn.cells.iter().find(|cell| cell.name == "AgentSend") {
        assert!(
            !send.ok,
            "a stopped agent takes no messages: {}",
            send.output
        );
        assert!(
            send.output.contains("was stopped"),
            "…and says so: {}",
            send.output
        );
    }
    if let Some(list) = turn.cells.iter().find(|cell| cell.name == "AgentList") {
        assert!(
            list.output.contains("stopped after"),
            "the list shows it stopped: {}",
            list.output
        );
    }
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_venice_agentkill_stops_a_running_agent() {
    agentkill_stops_a_running_agent(venice());
}

#[test]
#[ignore = "hits the network; needs OLLAMA_API_KEY"]
fn live_ollama_cloud_agentkill_stops_a_running_agent() {
    agentkill_stops_a_running_agent(ollama_cloud());
}
