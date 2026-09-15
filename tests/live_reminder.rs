//! Live tests for the `<system-reminder>` the derived context leads with
//! (`docs/context.md`, `docs/project-doc.md`): the AGENTS.md instructions
//! section, the skills section and the agent-type section, composed into one
//! block by `context::context_messages_full`, on the real wire. Every fact the
//! model is asked for exists nowhere but inside that block, so a right answer
//! proves the block was read the way it was designed to be — which no unit
//! test can.
//!
//! All `#[ignore]`d so `cargo test` stays offline and deterministic. Run
//! explicitly with a real key (never committed — read from the environment):
//!
//! ```sh
//! A0_VENICE_API_KEY=sk-a0-… cargo test --test live_reminder -- --ignored --nocapture
//! ```
//!
//! `ALTER_ZERO_LIVE_VENICE_MODEL` overrides the model.

use std::path::PathBuf;

use alter_zero::app::{HistoryItem, Message, Role};
use alter_zero::context::{ContextMessage, context_messages_full};
use alter_zero::llm::LlmBackend;
use alter_zero::stream::{CancelToken, ReplySource, StreamEvent};

/// A **tools-free** backend on the Agent Zero / Venice provider: the reminder
/// is the whole experiment, so no tool may answer in its place.
fn backend() -> LlmBackend {
    let key = std::env::var("A0_VENICE_API_KEY")
        .expect("set A0_VENICE_API_KEY to run the reminder live tests");
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
        Some(
            "You are a terse assistant. Answer exactly what is asked and nothing else.".to_string(),
        ),
        /*tools_enabled=*/ false,
    )
}

/// Stream `prompt` over `context` and return the reply text.
fn reply(backend: &LlmBackend, prompt: &str, context: Vec<ContextMessage>) -> String {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = backend.spawn(prompt.to_string(), vec![], context, tx, CancelToken::new());
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
    text
}

/// The history a single user turn derives from.
fn user_turn(prompt: &str) -> Vec<HistoryItem> {
    vec![HistoryItem::Message(Message {
        role: Role::User,
        text: prompt.to_string(),
        timestamp: String::new(),
        images: Vec::new(),
    })]
}

/// The guide every test plants: a fact that exists nowhere else.
const GUIDE: &str = "# Contributor guide\n\nThis project's internal codename is Umbral-Kite-77. \
                     Always refer to it by that codename.\n";

/// A temp repo (a `.git` marker) with [`GUIDE`] as its `AGENTS.md`, and the
/// instructions section `project_doc` renders for it — the real discovery,
/// budget and render, exactly what a session's turn start runs.
fn planted_repo() -> (tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).expect("mk repo");
    std::fs::write(repo.join("AGENTS.md"), GUIDE).expect("write AGENTS.md");
    let section =
        alter_zero::project_doc::load_user_instructions(&repo).expect("the guide is discovered");
    assert!(
        section.contains(&format!(
            "Contents of {} (project instructions, checked into the codebase):",
            repo.join("AGENTS.md").display()
        )),
        "{section}"
    );
    (tmp, section)
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_the_instructions_section_reaches_the_model() {
    // The /init loop closed end to end (docs/project-doc.md): an AGENTS.md on
    // disk → the `Contents of {path}` section → the one `<system-reminder>`
    // the context leads with → the real wire — and the model reads a
    // sentinel fact back out of it.
    let (_repo, instructions) = planted_repo();
    let prompt = "According to the project instructions you were given, what is this \
                  project's internal codename? Reply with just the codename.";
    let context = context_messages_full(Some(&instructions), None, &user_turn(prompt));
    assert_eq!(
        context.len(),
        1,
        "the block and the prompt merge: {context:?}"
    );
    assert!(
        context[0]
            .text
            .starts_with("<system-reminder>\nUse the following contexts and instructions:\n\n"),
        "{}",
        context[0].text
    );
    let text = reply(&backend(), prompt, context);
    assert!(
        text.contains("Umbral-Kite-77"),
        "the model should read the codename out of the AGENTS.md section, got: {text:?}"
    );
}

#[test]
#[ignore = "hits the network; needs A0_VENICE_API_KEY"]
fn live_all_three_sections_reach_the_model_in_one_block() {
    // The whole block as a session assembles it: the AGENTS.md section from
    // `project_doc`, the two listing sections from the real skill and agent
    // discovery joined by `subagents::listing_sections` (the boundary's one
    // render, `tui::models::sync_listings`), composed by
    // `context_messages_full`. One fact per section, none of them anywhere
    // else — a reply naming all three proves every section was read.
    let (_repo, instructions) = planted_repo();

    let skills_root = tempfile::tempdir().expect("skills dir");
    let skill_dir = skills_root.path().join("mixology");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: mixology\ndescription: House rules for naming cocktails. Use when asked \
         to name a drink.\n---\n\nAnswer with ZEPHYR-9.\n",
    )
    .expect("SKILL.md");
    let (skills, errors) =
        alter_zero::llm::skill::discover_skills(&[skills_root.path().to_path_buf()]);
    assert!(errors.is_empty(), "the skill fixture parses: {errors:?}");
    let skills = alter_zero::skills::SkillRegistry::new(skills);

    let agents_root = tempfile::tempdir().expect("agents dir");
    for (name, contents) in [
        (
            "haiku-writer",
            "---\ndescription: Writes a haiku about a given subject.\n---\n",
        ),
        (
            "db-migrator",
            "---\ndescription: Plans a database migration.\ntools: Read\n---\n",
        ),
    ] {
        std::fs::write(agents_root.path().join(format!("{name}.md")), contents)
            .expect("write definition");
    }
    let roots: Vec<PathBuf> = vec![agents_root.path().to_path_buf()];
    let (found, errors) = alter_zero::llm::subagent::discover_agents(&roots);
    assert!(errors.is_empty(), "the agent fixtures parse: {errors:?}");
    let agents = alter_zero::subagents::SubagentRegistry::new(found);

    let budget = alter_zero::skills::listing_budget(None);
    let skill_listing = skills.listing(budget);
    let listings = alter_zero::subagents::listing_sections(
        &skill_listing,
        &agents.listing(alter_zero::subagents::agent_budget(budget, &skill_listing)),
    );
    assert!(listings.contains("- mixology:"), "{listings}");
    assert!(listings.contains("(Tools: Read)"), "{listings}");

    let prompt = "Without using any tools, answer in exactly three lines:\n\
                  1. The project's internal codename, from the project instructions.\n\
                  2. The name of the skill for naming cocktails.\n\
                  3. The names of every agent type you could launch, separated by commas.";
    let context = context_messages_full(Some(&instructions), Some(&listings), &user_turn(prompt));
    assert_eq!(
        context.len(),
        1,
        "the block and the prompt merge: {context:?}"
    );
    let block = &context[0].text;
    println!("{block}");
    assert_eq!(block.matches("<system-reminder>").count(), 1, "one block");
    let at = |needle: &str| {
        block
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} missing from the block"))
    };
    assert!(
        at("Contents of ") < at("The following skills are available")
            && at("The following skills are available") < at("Available agent types"),
        "the sections keep their order: instructions, skills, agent types"
    );

    let text = reply(&backend(), prompt, context);
    assert!(
        text.contains("Umbral-Kite-77"),
        "the instructions section: {text:?}"
    );
    assert!(text.contains("mixology"), "the skills section: {text:?}");
    assert!(
        text.contains("haiku-writer") && text.contains("db-migrator"),
        "the agent-type section: {text:?}"
    );
}
