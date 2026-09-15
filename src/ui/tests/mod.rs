//! Unit tests for [`crate::ui`], split to mirror the module layout.
//!
//! Every file here is a descendant of `crate::ui`, so the tests keep the
//! same reach into the renderers' private helpers that the single flat
//! `mod tests` had before the split.

// Re-exported (not just imported) so the area modules below reach the whole
// of `crate::ui` — private items included — through their own `use super::*`.
use super::*;

use crate::app::{
    FileSearch, KeyKind, Message, ModelFetchError, ProviderChoice, RetryInfo, SigninKind,
    SubscriptionChoice,
};

mod agent;
mod ask_view;
mod assistant;
mod background_view;
mod classifier_view;
mod context_view;
mod donate_view;
mod footer;
mod header;
mod hooks_view;
mod image;
mod inline;
mod inline_diff;
mod layout;
mod live;
mod login_view;
mod mascot_view;
mod mcp_view;
mod menu;
mod message;
mod model_view;
mod palette;
mod permission_view;
mod reasoning;
mod resume_view;
mod settings_view;
mod skills_view;
mod spinner_view;
mod status;
mod stream_render;
mod stream_stress;
mod table;
mod tasks;
mod theme_view;
mod tool;
mod transcript;
mod trust_view;
mod view_flow;
mod wrap;

/// The `(r, g, b)` of an RGB colour — the blends' test arithmetic.
pub(super) fn rgb_of(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        other => panic!("expected an RGB colour, got {other:?}"),
    }
}

/// Concatenate a line's span contents into its plain text.
pub(super) fn plain(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Read row `y` of a buffer back as a string.
pub(super) fn row(buf: &Buffer, y: u16, width: u16) -> String {
    (0..width).map(|x| buf[(x, y)].symbol()).collect()
}

/// A queued text batch with no attachments — the queue tests' usual shape.
pub(super) fn batch(texts: &[&str]) -> QueuedTurn {
    QueuedTurn::Messages {
        texts: texts.iter().map(|s| (*s).to_string()).collect(),
        images: Vec::new(),
    }
}

// --- tool-output view: the full conversation transcript (Ctrl+O overlay) ---

/// Drive an app through `user → "let me check" → Read(ok) → "all done"`.
pub(super) fn transcript_fixture() -> App {
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_stream();
    app.push_chunk("let me check");
    app.flush_streaming_segment();
    app.start_tool("Read", "f", None);
    app.end_tool("L1\nL2\nL3", true);
    app.push_chunk("all done");
    app.finish_stream();
    app
}

// --- timestamps: only the user message's, bottom-right, transcript-only ---

pub(super) const STAMP: &str = "03:20 AM";

/// A user message, a tool call, and an assistant reply, all carrying a stamp
/// (only the user's may show).
pub(super) fn stamped_history() -> Vec<HistoryItem> {
    vec![
        HistoryItem::Message(Message {
            role: Role::User,
            text: "hi".to_string(),
            timestamp: STAMP.to_string(),
            images: Vec::new(),
        }),
        HistoryItem::Tool(ToolCall {
            name: "Read".to_string(),
            args: "f".to_string(),
            status: ToolStatus::Ok,
            output: "out".to_string(),
            timestamp: STAMP.to_string(),
            shell: false,
            truncated: false,
            context_output: None,
            arguments: None,
            approval_note: None,
            batch: None,
        }),
        HistoryItem::Message(Message {
            role: Role::Assistant,
            text: "hello".to_string(),
            timestamp: STAMP.to_string(),
            images: Vec::new(),
        }),
    ]
}

// --- live status line + committed "Done" summary (docs/status-indicator.md) ---

/// A live status with the given metrics (verb fixed to "Working").
pub(super) fn status(
    tokens: usize,
    arrow: TokenArrow,
    elapsed: u64,
    thinking: Option<u64>,
) -> TurnStatus {
    TurnStatus {
        verb: "Working",
        done_verb: "Done",
        tokens,
        arrow,
        elapsed: Duration::from_secs(elapsed),
        thinking: thinking.map(Duration::from_secs),
        shell: false,
        retry: None,
    }
}

// --- render_live (against a plain Buffer) ---

pub(super) fn buffer(width: u16, height: u16) -> Buffer {
    Buffer::empty(Rect::new(0, 0, width, height))
}

// --- conversation repaint (after a resize) ---

pub(super) fn msg(role: Role, text: &str) -> HistoryItem {
    HistoryItem::Message(Message {
        role,
        text: text.to_string(),
        timestamp: String::new(),
        images: Vec::new(),
    })
}

pub(super) fn tool(name: &str, args: &str, status: ToolStatus, output: &str) -> ToolCall {
    ToolCall {
        name: name.to_string(),
        args: args.to_string(),
        status,
        output: output.to_string(),
        timestamp: String::new(),
        shell: false,
        truncated: false,
        context_output: None,
        arguments: None,
        approval_note: None,
        batch: None,
    }
}

// --- slash-command palette rendering + geometry ---

/// An app whose input is `input` with the palette open at `selected`.
pub(super) fn palette(input: &str, selected: usize) -> App {
    let mut app = App::new();
    app.input = TextArea::from_text(input);
    app.command_menu = Some(crate::app::CommandMenu { selected });
    app
}

// --- /compact: the marker cell (docs/compact.md) ---

/// A test marker with no token info (the pre-gauge shape).
pub(super) fn bare_compaction(summary: &str) -> crate::app::Compaction {
    crate::app::Compaction {
        summary: summary.into(),
        timestamp: String::new(),
        before: 0,
        after: 0,
        auto: false,
        secs: 0,
    }
}

// --- the session-context footer under the box (docs/footer.md) ---

/// An app with session info injected, as `main.rs` does at startup.
pub(super) fn with_session() -> App {
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/alter-zero");
    app
}

// --- the Ctrl+R search line in the footer slot (docs/history-search.md) ---

/// An app with `history` recorded and a Ctrl+R search open with `query`
/// typed, driven through the real key path.
pub(super) fn searching(history: &[&str], query: &str) -> App {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = App::new();
    for text in history {
        app.input_history.record(text);
    }
    app.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    for c in query.chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app
}

/// An app with an open file picker over `matches` (input is the `@query`).
pub(super) fn file_picker(query: &str, matches: Vec<FileMatch>, selected: usize) -> App {
    let mut app = App::new();
    app.input = TextArea::from_text(&format!("@{query}"));
    app.file_search = Some(FileSearch {
        selected,
        query: query.to_string(),
        matches,
        waiting: false,
    });
    app
}

// --- Esc-Esc backtrack rendering (docs/backtrack.md) ---

/// An app holding two finished exchanges, ready to preview.
pub(super) fn backtrack_app() -> App {
    let mut app = App::new();
    for (user, reply) in [("first", "a"), ("second", "b")] {
        app.record_user_message(user);
        app.begin_stream();
        app.push_chunk(reply);
        app.finish_stream();
        app.end_turn(1);
    }
    app
}

// ===== inline /model picker (docs/llm.md) =====

pub(super) fn model_entry(id: &str, provider: &str, name: &str) -> ModelEntry {
    ModelEntry {
        id: id.into(),
        provider: provider.into(),
        display_name: name.into(),
        reasoning: None,
        vision: None,
        context: None,
        service_tiers: Vec::new(),
    }
}

/// A ready picker over the given models, with `selected`/`active` set.
pub(super) fn model_picker(models: Vec<ModelEntry>, selected: usize, active: &str) -> ModelPicker {
    ModelPicker {
        models,
        status: ModelLoad::Ready,
        selected,
        query: String::new(),
        active_id: active.into(),
        ..ModelPicker::default()
    }
}

pub(super) fn three_models() -> Vec<ModelEntry> {
    vec![
        model_entry(
            "anthropic/claude-3-haiku",
            "openrouter",
            "Anthropic: Claude 3 Haiku",
        ),
        model_entry(
            "anthropic/claude-fable-5",
            "openrouter",
            "Anthropic: Claude Fable 5",
        ),
        model_entry(
            "moonshotai/kimi-k2.6",
            "openrouter",
            "MoonshotAI: Kimi K2.6",
        ),
    ]
}

// --- The inline `/login` onboarding flow (docs/llm.md). ---

fn login_subscriptions() -> Vec<SubscriptionChoice> {
    vec![SubscriptionChoice {
        id: "github_copilot".into(),
        name: "GitHub Copilot".into(),
        description: "Sign in with your GitHub account".into(),
        configured: true,
        kind: SigninKind::DeviceCode,
    }]
}

fn login_choices() -> Vec<ProviderChoice> {
    vec![
        ProviderChoice {
            id: "openrouter".into(),
            name: "OpenRouter".into(),
            env_var: "OPENROUTER_API_KEY".into(),
            configured: true,
            key_kind: KeyKind::Secret,
            description: String::new(),
            key_url: String::new(),
        },
        ProviderChoice {
            id: "together".into(),
            name: "Together AI".into(),
            env_var: "TOGETHER_API_KEY".into(),
            configured: false,
            key_kind: KeyKind::Secret,
            description: String::new(),
            key_url: String::new(),
        },
    ]
}

/// A host-configured provider row — Ollama's, whose `/login` field asks for
/// where the server is rather than a secret (`docs/ollama.md`). Kept out of
/// [`login_choices`] so the provider-step geometry the layout tests pin
/// stays two rows.
pub(super) fn host_choice() -> ProviderChoice {
    ProviderChoice {
        id: "ollama".into(),
        name: "Ollama".into(),
        env_var: "OLLAMA_HOST".into(),
        configured: false,
        key_kind: KeyKind::Host {
            default: "http://127.0.0.1:11434".into(),
        },
        description: String::new(),
        key_url: String::new(),
    }
}

/// The `/login` flow parked on its root (the method step).
pub(super) fn login_app() -> App {
    let mut app = App::new();
    app.open_key_onboarding(login_choices(), login_subscriptions(), "~/.alter-zero/.env");
    app
}

/// …one step down, on the API-key provider list.
pub(super) fn login_app_provider() -> App {
    let mut app = login_app();
    app.key_onboarding.as_mut().unwrap().step = KeyStep::Provider;
    app
}

/// …or on the subscription list.
pub(super) fn login_app_subscription() -> App {
    let mut app = login_app();
    app.key_onboarding.as_mut().unwrap().step = KeyStep::Subscription;
    app
}

/// …or on GitHub Copilot's device page, with a code delivered.
pub(super) fn login_app_device() -> App {
    let mut app = login_app_subscription();
    {
        let onboarding = app.key_onboarding.as_mut().unwrap();
        onboarding.step = KeyStep::Device;
        onboarding.device = Some(crate::app::DeviceLogin {
            provider_id: "github_copilot".into(),
            provider_name: "GitHub Copilot".into(),
            ..Default::default()
        });
    }
    app.set_device_code("https://github.com/login/device", "C363-262E");
    app
}

// --- background shells (docs/background.md) ---

pub(super) fn bg_notice(code: Option<i32>, killed: bool) -> crate::app::BackgroundNotice {
    crate::app::BackgroundNotice {
        description: "Ping x.com 200 times".to_string(),
        id: "bash_1".to_string(),
        code,
        killed,
        output_tail: "tail".to_string(),
        origin: None,
        timestamp: String::new(),
    }
}

pub(super) fn spec(id: &str, desc: &str, background: bool) -> crate::stream::AgentSpec {
    crate::stream::AgentSpec {
        id: id.into(),
        description: desc.into(),
        agent_type: "general-purpose".into(),
        prompt: "task?".into(),
        background,
    }
}
