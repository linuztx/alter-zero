//! Unit tests for [`crate::app`], split to mirror the module layout.
//!
//! Every file here is a descendant of `crate::app`, so the tests keep the
//! same access to `App`'s private state that the single flat `mod tests`
//! had before the split.

// Re-exported (not just imported) so the area modules below reach the whole
// of `crate::app` — private items included — through their own `use super::*`.
pub(super) use super::*;

// ===== Shift+Tab thinking mode (docs/reasoning.md) =====

use crate::llm::ReasoningEffort;

mod agent;
mod background;
mod backtrack;
mod commands;
mod compact;
mod composer;
mod file_picker;
mod input_history;
mod keys;
mod login;
mod model_picker;
mod permission;
mod queue;
mod resume;
mod tools;
mod turn;
mod types;
mod views;

pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// The roles of the `Message` items in history, in order (tools/summaries
/// skipped).
pub(super) fn roles(app: &App) -> Vec<Role> {
    app.history
        .iter()
        .filter_map(|item| match item {
            HistoryItem::Message(m) => Some(m.role),
            _ => None,
        })
        .collect()
}

/// The `Message` at history index `i` (panics if it is not a message).
pub(super) fn message_at(app: &App, i: usize) -> &Message {
    match &app.history[i] {
        HistoryItem::Message(m) => m,
        _ => panic!("expected a message at history[{i}]"),
    }
}

// ===== ↑/↓ input history (shell-style recall — docs/input-history.md) =====

/// Submit `text` through the real Enter path so it gets recorded.
pub(super) fn submit(app: &mut App, text: &str) {
    app.input = TextArea::from_text(text);
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit(text.to_string())
    );
}

/// A bare file match (no score/indices) for the file-picker tests.
pub(super) fn fm(path: &str) -> FileMatch {
    FileMatch {
        path: path.to_string(),
        score: 0,
        indices: Vec::new(),
    }
}

/// A three-call parallel batch, as the header `name`/`args` summaries.
pub(super) fn ping_batch() -> Vec<ToolCallSummary> {
    ["ping google.com", "ping facebook.com", "ping x.com"]
        .iter()
        .map(|cmd| ToolCallSummary {
            name: "Bash".to_string(),
            args: (*cmd).to_string(),
        })
        .collect()
}

// --- tool-output view (Ctrl+O) ---

pub(super) fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

// --- message queue (docs/queue.md) ---

pub(super) fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::ALT)
}

/// A queued text batch with no attachments — most queue tests' shape.
pub(super) fn batch(texts: &[&str]) -> QueuedTurn {
    QueuedTurn::Messages {
        texts: texts.iter().map(|s| (*s).to_string()).collect(),
        images: Vec::new(),
    }
}

// --- slash-command palette ---

pub(super) fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

// --- auto-compact + the context gauge (docs/compact.md) ---

pub(super) fn usage_of(total_input: u64, output: u64) -> crate::stream::TokenUsage {
    crate::stream::TokenUsage {
        input: total_input,
        output,
        cached: 0,
        cache_write: 0,
    }
}

// --- timestamps (shown only in the Ctrl+O transcript; see docs/timestamps.md) ---

/// A fixed stub clock so timestamp behaviour is deterministic in tests.
pub(super) const STAMP: &str = "2026-06-09 02:32:05 PM";

// --- Ctrl+R history search: InputHistory::search / entry / resume_at
// (docs/history-search.md — codex's chat_composer_history search) ---

/// An InputHistory with `texts` recorded oldest → newest.
pub(super) fn history_of(texts: &[&str]) -> InputHistory {
    let mut history = InputHistory::default();
    for text in texts {
        history.record(text);
    }
    history
}

// --- Ctrl+R history search: the session over App (docs/history-search.md —
// codex's HistorySearchSession in chat_composer/history_search.rs) ---

/// An App with `texts` submitted (each its own finished-enough turn — the
/// submit helper records them), composer empty again.
pub(super) fn searchable_app(texts: &[&str]) -> App {
    let mut app = App::new();
    for text in texts {
        submit(&mut app, text);
    }
    app
}

/// Type `text` into the open search query through the real key path.
pub(super) fn type_query(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

// --- Esc-Esc backtrack: edit a previous message (docs/backtrack.md) ---

/// Run one full finished exchange (user → assistant → summary) so the app
/// ends idle again, exactly as a real completed turn leaves it.
pub(super) fn exchange(app: &mut App, user: &str, reply: &str) {
    app.record_user_message(user);
    app.begin_stream();
    app.push_chunk(reply);
    app.finish_stream();
    app.end_turn(1);
}

// ===== /resume session picker (docs/resume.md) =====

pub(super) fn summary(path: &str, preview: &str) -> crate::session::SessionSummary {
    crate::session::SessionSummary {
        path: PathBuf::from(path),
        updated_secs: 300,
        created_secs: 300,
        cwd: "/repo".into(),
        preview: preview.into(),
    }
}

/// An app with the picker open over one session per `(path, preview)`,
/// all recorded in the picker's own cwd (`/repo`).
pub(super) fn picker_app(sessions: &[(&str, &str)]) -> App {
    let mut app = App::new();
    app.open_resume_picker(
        sessions
            .iter()
            .map(|(path, preview)| summary(path, preview))
            .collect(),
        "/repo".into(),
    );
    app
}

/// Type `text` into the composer key by key.
pub(super) fn type_chars(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

// ===== /model picker (docs/llm.md) =====

pub(super) fn model(id: &str, provider: &str, name: &str) -> ModelEntry {
    ModelEntry {
        id: id.into(),
        provider: provider.into(),
        display_name: name.into(),
        reasoning: None,
        vision: None,
        context: None,
    }
}

/// An app with the model picker open (loaded) over the given models, the
/// first marked active.
pub(super) fn model_app(models: &[ModelEntry]) -> App {
    let mut app = App::new();
    let active = models.first().map(|m| m.id.clone()).unwrap_or_default();
    app.open_model_picker(active);
    app.set_models(models.to_vec());
    app
}

pub(super) fn sample_models() -> Vec<ModelEntry> {
    vec![
        model(
            "anthropic/claude-3.5-haiku",
            "openrouter",
            "Anthropic: Claude 3.5 Haiku",
        ),
        model(
            "anthropic/claude-fable-5",
            "openrouter",
            "Anthropic: Claude Fable 5",
        ),
        model(
            "moonshotai/kimi-k2.6",
            "openrouter",
            "MoonshotAI: Kimi K2.6",
        ),
    ]
}

/// The common support shape: the low/medium/high ladder, disableable.
pub(super) fn trio_support() -> ReasoningSupport {
    ReasoningSupport {
        efforts: vec![
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ],
        can_disable: true,
        default_effort: None,
    }
}

pub(super) fn backtab() -> KeyEvent {
    key(KeyCode::BackTab)
}

// --- The `/login` API-key onboarding flow (docs/llm.md). ---

pub(super) fn sample_choices() -> Vec<ProviderChoice> {
    vec![
        ProviderChoice {
            id: "a0_venice".into(),
            name: "Agent Zero API".into(),
            env_var: "A0_VENICE_API_KEY".into(),
            configured: false,
        },
        ProviderChoice {
            id: "openrouter".into(),
            name: "OpenRouter".into(),
            env_var: "OPENROUTER_API_KEY".into(),
            configured: true,
        },
        ProviderChoice {
            id: "together".into(),
            name: "Together AI".into(),
            env_var: "TOGETHER_API_KEY".into(),
            configured: false,
        },
    ]
}

pub(super) fn login_app() -> App {
    let mut app = App::new();
    app.open_key_onboarding(sample_choices(), "~/.alter-zero/.env");
    app
}

/// Drive the flow to the key-entry step for the given provider id.
pub(super) fn key_app(provider_id: &str) -> App {
    let mut app = login_app();
    let idx = sample_choices()
        .iter()
        .position(|p| p.id == provider_id)
        .unwrap();
    {
        let onboarding = app.key_onboarding.as_mut().unwrap();
        onboarding.selected = onboarding
            .matches()
            .iter()
            .position(|p| p.id == provider_id)
            .unwrap();
    }
    app.on_key(key(KeyCode::Enter));
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Key);
    assert_eq!(onboarding.chosen, Some(idx));
    app
}

// --- background shells (docs/background.md) ---

/// A registered running shell for the manager tests.
pub(super) fn app_with_shells(commands: &[&str]) -> App {
    let mut app = App::new();
    for (i, cmd) in commands.iter().enumerate() {
        app.bg_started(&format!("bash_{}", i + 1), cmd, None, true, None);
    }
    app
}

// ===== The Agent tool (docs/agent-tool.md) =====

/// A two-agent batch announcement, the dummy demo's shape.
pub(super) fn agent_specs(background: bool) -> Vec<AgentSpec> {
    let spec = |id: &str, desc: &str, prompt: &str| AgentSpec {
        id: id.to_string(),
        description: desc.to_string(),
        agent_type: crate::agents::GENERAL_PURPOSE.to_string(),
        prompt: prompt.to_string(),
        background,
    };
    vec![
        spec(
            "a1",
            "Fetch Warsaw weather",
            "What is the weather in Warsaw?",
        ),
        spec(
            "a2",
            "Fetch Manila weather",
            "What is the weather in Manila?",
        ),
    ]
}
