//! Live integration tests for the **subagent definitions** (`docs/subagents.md`)
//! — the `agents/*.md` files behind the `agent` tool's `subagent_type`, on the
//! real wire.
//!
//! Everything the definitions do is invisible from inside a unit test: a
//! definition's body only matters if the provider is sent it, its `tools:`
//! allowlist only matters if the model is offered exactly that set, and its
//! `model:` only matters if the request carries that id. Each test here picks
//! an observable the wire alone can produce.
//!
//! All `#[ignore]`d so `cargo test` stays offline and deterministic. Run
//! explicitly with a real key (never committed — read from the environment):
//!
//! ```sh
//! A0_VENICE_API_KEY=sk-a0-… cargo test --test live_subagents -- --ignored --nocapture
//! ```
//!
//! `ALTER_ZERO_LIVE_VENICE_MODEL` overrides the model.

use std::path::{Path, PathBuf};

use alter_zero::context::{ContextMessage, ContextRole};
use alter_zero::llm::LlmBackend;
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};
use alter_zero::subagents::SubagentRegistry;

/// A backend on the Agent Zero / Venice provider, tools **on** (the whole
/// point here is which tools a subagent is offered), with a terse system
/// prompt so a definition's own body is the only voice in the child's prompt.
fn backend() -> LlmBackend {
    let key = std::env::var("A0_VENICE_API_KEY")
        .expect("set A0_VENICE_API_KEY to run the subagent live tests");
    let model = std::env::var("ALTER_ZERO_LIVE_VENICE_MODEL")
        .unwrap_or_else(|_| "openai-gpt-4o-mini-2024-07-18".to_string());
    let providers = alter_zero::llm::ProvidersFile::builtin();
    let sel = alter_zero::llm::Selection {
        provider_id: "a0_venice".to_string(),
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
    let cfg = providers.model_config(&sel).expect("a0_venice is built in");
    LlmBackend::configure(
        cfg,
        Some("You are a terse assistant. Answer in as few words as possible.".to_string()),
        /*tools_enabled=*/ true,
    )
}

/// A backend that launches subagents, with `dir`'s definitions attached.
fn backend_with_agents(dir: &Path) -> (LlmBackend, alter_zero::agents::AgentRegistry) {
    let (bg_tx, _bg_rx) = tokio::sync::mpsc::unbounded_channel();
    let scratch =
        std::env::temp_dir().join(format!("alter-zero-live-subagents-{}", std::process::id()));
    let background = alter_zero::background::BackgroundRegistry::new(bg_tx, scratch);
    let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let agents = alter_zero::agents::AgentRegistry::new(agent_tx);
    let (found, errors) =
        alter_zero::llm::subagent::with_builtins(alter_zero::llm::subagent::discover_agents(&[
            dir.to_path_buf(),
        ]));
    assert!(
        errors.is_empty(),
        "the fixture definitions parse: {errors:?}"
    );
    let backend = backend()
        .with_background(background)
        .with_agents(agents.clone())
        .with_subagents(SubagentRegistry::new(found));
    (backend, agents)
}

/// Run one turn and return every foreground subagent's result text.
fn agent_outputs(backend: &LlmBackend, prompt: &str) -> Vec<String> {
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let mut outputs = Vec::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::AgentGroupDone { agents, .. } => {
                outputs.extend(agents.into_iter().map(|done| done.output));
            }
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("subagent outputs: {outputs:?}");
    outputs
}

/// A launch instruction the model can follow exactly, foreground so the
/// result comes back inside this turn.
fn launch(agent_type: &str, description: &str, task: &str) -> String {
    format!(
        "Use the agent tool exactly once: description \"{description}\", subagent_type \
         \"{agent_type}\", run_in_background false, and this exact prompt: \"{task}\" \
         When the agent returns, reply with one word: done."
    )
}

/// A temp directory holding `files` as `{name}.md` agent definitions.
fn fixture(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    for (name, contents) in files {
        std::fs::write(dir.path().join(format!("{name}.md")), contents).expect("write definition");
    }
    dir
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_a_definitions_body_is_the_subagents_system_prompt() {
    // The body replaces the persona for that type (`docs/subagents.md`). The
    // sentinel is in the FILE only — never in the launch instruction, never in
    // the child's task — so a reply carrying it can only have come from the
    // definition riding the wire as that agent's system message.
    let dir = fixture(&[(
        "sentinel",
        "---\ndescription: Answers with a fixed word.\nmodel: inherit\n---\n\
         You are a test agent. Whatever you are asked, reply with exactly one \
         word: PLATYPUS. No punctuation, no explanation.\n",
    )]);
    let (backend, _agents) = backend_with_agents(dir.path());

    // …and the Ctrl+D surface shows the same prompt the launch composes.
    let surfaced = ReplySource::agent_system_prompt(&backend, "sentinel")
        .expect("the type resolves to a prompt");
    assert!(
        surfaced.contains("PLATYPUS"),
        "the view shows the definition's own body: {surfaced}"
    );
    assert!(
        surfaced.contains("launched by the main agent"),
        "the subagent note still closes it: {surfaced}"
    );
    assert!(
        !ReplySource::system_prompt(&backend)
            .expect("the main prompt is set")
            .contains("PLATYPUS"),
        "the body belongs to that type alone"
    );

    let outputs = agent_outputs(
        &backend,
        &launch(
            "sentinel",
            "Say the word",
            "What is 2 + 2? Answer normally.",
        ),
    );
    assert!(
        outputs
            .iter()
            .any(|output| output.to_uppercase().contains("PLATYPUS")),
        "the definition's body governed the child's answer: {outputs:?}"
    );
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_a_tools_allowlist_withholds_the_tool_it_omits() {
    // `tools:` is an allowlist over the specs the child is offered, so a type
    // without `Write` cannot write — checked on the filesystem rather than on
    // the model's word, which is the only assertion a model cannot talk its
    // way around.
    let dir = fixture(&[(
        "reader",
        "---\ndescription: Reads only.\ntools: Bash, Read\n---\n",
    )]);
    let (backend, _agents) = backend_with_agents(dir.path());

    let target: PathBuf = std::env::temp_dir().join(format!(
        "alter-zero-live-nowrite-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let _ = std::fs::remove_file(&target);
    let outputs = agent_outputs(
        &backend,
        &launch(
            "reader",
            "Try to write a file",
            &format!(
                "Use the write tool to create the file {} with the text hello. \
                 If you have no tool that can write a file, say NO WRITE TOOL and stop.",
                target.display()
            ),
        ),
    );
    assert!(
        !target.exists(),
        "a type whose allowlist omits Write has no write tool: {} exists, outputs {outputs:?}",
        target.display()
    );
    let _ = std::fs::remove_file(&target);
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_a_definitions_model_reaches_the_request() {
    // `model:` swaps the model id for that type's run — same provider, same
    // key. Observable only from the provider's own answer, so this asks for
    // one: a bogus id must fail the child's request (and, since the id is the
    // only thing that changed, the *reason* is the model), while the sibling
    // type on a real second model completes normally.
    let real = std::env::var("ALTER_ZERO_LIVE_VENICE_ALT_MODEL")
        .unwrap_or_else(|_| "llama-3.3-70b".to_string());
    let dir = fixture(&[
        (
            "bogus-model",
            "---\ndescription: Runs on a model that does not exist.\n\
             model: alter-zero-no-such-model-xyz\n---\n",
        ),
        (
            "alt-model",
            &format!("---\ndescription: Runs on a second real model.\nmodel: {real}\n---\n"),
        ),
    ]);
    let (backend, _agents) = backend_with_agents(dir.path());

    let failed = agent_outputs(
        &backend,
        &launch("bogus-model", "Say hi", "Reply with one word: hi."),
    );
    assert!(
        failed
            .iter()
            .any(|output| output.starts_with("[agent failed:")),
        "the bogus model id reached the provider and was refused: {failed:?}"
    );

    let ok = agent_outputs(
        &backend,
        &launch("alt-model", "Say hi", "Reply with one word: hi."),
    );
    assert!(
        ok.iter()
            .any(|output| output.to_lowercase().contains("hi") && !output.starts_with("[agent")),
        "the same override on a real model runs normally: {ok:?}"
    );
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_an_unknown_agent_type_is_corrected_in_the_same_round() {
    // An unknown `subagent_type` resolves as a recoverable error naming the
    // types that exist — so the model retries with a real one inside the same
    // turn instead of silently getting a general-purpose agent.
    let dir = fixture(&[(
        "sentinel",
        "---\ndescription: Answers with a fixed word.\n---\n\
         Whatever you are asked, reply with exactly one word: PLATYPUS.\n",
    )]);
    let (backend, _agents) = backend_with_agents(dir.path());
    let outputs = agent_outputs(
        &backend,
        &launch(
            "sentinal",
            "Say the word",
            "What is 2 + 2? Answer normally.",
        ),
    );
    assert!(
        outputs
            .iter()
            .any(|output| output.to_uppercase().contains("PLATYPUS")),
        "the typo was corrected from the error's list of types: {outputs:?}"
    );
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_the_reminder_tells_the_model_which_types_exist() {
    // The `<system-reminder>`'s agent section (`docs/subagents.md`): the
    // listing is the only place a type's name and description reach the
    // model, so a session carrying it can answer what it could launch.
    let dir = fixture(&[
        (
            "haiku-writer",
            "---\ndescription: Writes a haiku about a given subject.\n---\n",
        ),
        (
            "db-migrator",
            "---\ndescription: Plans a database migration.\ntools: Read\n---\n",
        ),
    ]);
    let (found, _) = alter_zero::llm::subagent::discover_agents(&[dir.path().to_path_buf()]);
    let registry = SubagentRegistry::new(found);
    let reminder =
        alter_zero::reminder::reminder_message(&[&alter_zero::subagents::agent_section(
            &registry.listing(alter_zero::skills::listing_budget(None)),
        )]);
    println!("{reminder}");
    assert!(reminder.contains("(Tools: Read)"), "{reminder}");

    let prompt = "Without using any tools, list the names of every agent type you can \
                  launch with the agent tool, separated by commas. Names only.";
    let context = vec![
        ContextMessage::new(ContextRole::User, reminder),
        ContextMessage::new(ContextRole::User, prompt.to_string()),
    ];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend().spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let mut text = String::new();
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::Chunk(chunk) => text.push_str(&chunk),
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    println!("reply: {text:?}");
    assert!(
        text.contains("haiku-writer") && text.contains("db-migrator"),
        "the model read the agent listing: {text:?}"
    );
}

/// Run one turn and return every tool cell it resolved as `(display name,
/// output)` — a companion's cell beside its real report — plus the
/// foreground agents' results (`docs/agent-tools.md`).
fn turn_cells(backend: &LlmBackend, prompt: &str) -> (Vec<(String, String)>, Vec<String>) {
    let context = vec![ContextMessage::new(ContextRole::User, prompt)];
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
    let mut cells = Vec::new();
    let mut agent_results = Vec::new();
    let mut open: Option<String> = None;
    while let Some(event) = rx.blocking_recv() {
        match event {
            StreamEvent::ToolStart { name, .. } => open = Some(name),
            StreamEvent::ToolEnd { output, .. }
            | StreamEvent::ToolAnswered { result: output, .. }
            | StreamEvent::ToolRejected { result: output, .. } => {
                if let Some(name) = open.take() {
                    cells.push((name, output));
                }
            }
            StreamEvent::AgentGroupDone { agents, .. } => {
                agent_results.extend(agents.into_iter().map(|done| done.output));
            }
            StreamEvent::Retrying { attempt, max } => println!("retrying {attempt}/{max}…"),
            StreamEvent::Error(e) => panic!("backend error: {e}"),
            StreamEvent::StreamDone => break,
            _ => {}
        }
    }
    handle.join().expect("backend thread joins");
    for (name, output) in &cells {
        println!("● {name}\n{output}\n");
    }
    println!("agent results: {agent_results:?}");
    (cells, agent_results)
}

/// The output of the first cell named `display`, or a panic naming the
/// companion the model never called.
fn cell(cells: &[(String, String)], display: &str) -> String {
    cells
        .iter()
        .find(|(name, _)| name == display)
        .map(|(_, output)| output.clone())
        .unwrap_or_else(|| panic!("the model called {display}: {cells:?}"))
}

/// The agent id a launch acknowledgement or a foreground result names.
fn named_id(text: &str) -> String {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .find(|word| word.len() == 9 && word.starts_with('a'))
        .unwrap_or_else(|| panic!("an agent id in {text:?}"))
        .to_string()
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_the_model_resumes_a_finished_agent_by_id() {
    // A foreground result names the agent's id, `agentsend` on that id
    // resumes the FINISHED agent over its kept conversation, and
    // `agentoutput` with `wait` returns its new answer — the sentinel body
    // proves the same agent answered again (docs/agent-tools.md).
    let dir = fixture(&[(
        "sentinel",
        "---\ndescription: Answers with a fixed word.\n---\n\
         Whatever you are asked, reply with exactly one word: PLATYPUS.\n",
    )]);
    let (backend, agents) = backend_with_agents(dir.path());
    let prompt = "Do exactly these steps, one tool call at a time, in order. \
                  1) Use the agent tool once: description \"Say the word\", \
                  subagent_type \"sentinel\", run_in_background false, prompt \"What is 2 + 2?\". \
                  2) The result names the agent's id. Call agentsend with that agent_id and \
                  the message \"Say it once more.\". \
                  3) Call agentoutput with the same agent_id and wait 120. \
                  4) Reply with one word: done.";
    let (cells, agent_results) = turn_cells(&backend, prompt);
    let result = agent_results
        .iter()
        .find(|result| result.contains("agentsend continues this conversation"))
        .unwrap_or_else(|| panic!("the foreground result names the id: {agent_results:?}"));
    let id = named_id(result);
    let sent = cell(&cells, alter_zero::llm::tools::AGENT_SEND_DISPLAY);
    assert!(
        sent.contains(&format!("Agent {id} resumed")),
        "a finished agent resumes: {sent}"
    );
    let report = cell(&cells, alter_zero::llm::tools::AGENT_OUTPUT_DISPLAY);
    assert!(report.starts_with(&format!("Done (agent {id}")), "{report}");
    assert!(
        report.contains(alter_zero::agents::AGENT_FOLLOW_UP_MARK),
        "the lead's message is marked where it arrived: {report}"
    );
    assert!(report.contains("Final response:"), "{report}");
    assert!(
        report.to_uppercase().contains("PLATYPUS"),
        "the resumed agent answered under its own definition: {report}"
    );
    let snapshot = agents.snapshot(&id).expect("the agent is still kept");
    assert!(
        matches!(snapshot.state, alter_zero::agents::AgentState::Done(_)),
        "{snapshot:?}"
    );
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_the_model_inspects_stops_and_lists_a_background_agent() {
    // `agentoutput` on a running background agent reports its calls,
    // `agentkill` stops it with its conversation kept, and `agentlist` then
    // names it as stopped — each cell the real executor's report, and the
    // registry agreeing (docs/agent-tools.md).
    let dir = fixture(&[(
        "sleeper",
        "---\ndescription: Runs a slow command.\ntools: Bash\n---\n\
         Run the bash command `sleep 240; echo done` with wait 300, then reply done.\n",
    )]);
    let (backend, agents) = backend_with_agents(dir.path());
    let prompt = "Do exactly these steps, one tool call at a time, in order. \
                  1) Use the agent tool once: description \"Sleep a while\", \
                  subagent_type \"sleeper\", run_in_background true, prompt \"Run the slow \
                  command now.\". The result names the agent's id. \
                  2) Call bash with the command \"sleep 8\". \
                  3) Call agentoutput with that agent_id and no wait. \
                  4) Call agentkill with that agent_id. \
                  5) Call agentlist. \
                  6) Reply with one word: done.";
    let (cells, agent_results) = turn_cells(&backend, prompt);
    let ack = agent_results
        .iter()
        .find(|result| result.contains("launched as a"))
        .unwrap_or_else(|| panic!("the launch names the id: {agent_results:?}"));
    let id = named_id(ack);
    let report = cell(&cells, alter_zero::llm::tools::AGENT_OUTPUT_DISPLAY);
    assert!(
        report.starts_with(&format!("Running (agent {id}")),
        "{report}"
    );
    assert!(
        report.contains("Bash(sleep"),
        "the agent's command is one of its calls: {report}"
    );
    let killed = cell(&cells, alter_zero::llm::tools::AGENT_KILL_DISPLAY);
    assert!(
        killed.starts_with(&format!("Stopped (agent {id}")),
        "{killed}"
    );
    assert!(
        killed.contains("agentsend resumes it"),
        "the lead's stop keeps the conversation: {killed}"
    );
    let listed = cell(&cells, alter_zero::llm::tools::AGENT_LIST_DISPLAY);
    assert!(listed.contains(&format!("- {id} (sleeper)")), "{listed}");
    assert!(listed.contains("stopped after"), "{listed}");
    let snapshot = agents.snapshot(&id).expect("the agent is still kept");
    assert!(
        matches!(
            snapshot.state,
            alter_zero::agents::AgentState::Stopped { by_user: false }
        ),
        "{snapshot:?}"
    );
}
