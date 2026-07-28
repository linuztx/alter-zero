//! Unit tests for [`crate::ui`], split to mirror the module layout.
//!
//! Every file here is a descendant of `crate::ui`, so the tests keep the
//! same reach into the renderers' private helpers that the single flat
//! `mod tests` had before the split.

// Re-exported (not just imported) so the area modules below reach the whole
// of `crate::ui` — private items included — through their own `use super::*`.
pub(super) use super::*;

use crate::app::{FileSearch, Message, ModelFetchError, RetryInfo};

mod agent;
mod assistant;
mod background_view;
mod context_view;
mod footer;
mod header;
mod layout;
mod live;
mod login_view;
mod menu;
mod message;
mod model_view;
mod resume_view;
mod status;
mod stream_render;
mod table;
mod tool;
mod transcript;
mod wrap;

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

// --- markdown tables (docs/markdown.md) ---

/// The plain text of each content row (span contents concatenated).
pub(super) fn rows_text(rows: &[Vec<Span<'static>>]) -> Vec<String> {
    rows.iter()
        .map(|r| r.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect()
}

/// Whether `prefix` ends inside an OPEN GFM table — its last non-blank source
/// line is still a table row/delimiter/header, so no non-table line has
/// closed the block. Used by the differential test to skip the preview
/// equality check exactly where the streaming preview intentionally shows the
/// last content row rather than the batch's flushed bottom border.
pub(super) fn ends_in_open_table(prefix: &str) -> bool {
    prefix
        .split('\n')
        .rev()
        .find(|l| !l.trim().is_empty())
        .is_some_and(markdown::is_table_row)
}

/// The rendered cell widths of a table's grid, read off its `┌──┬──┐` top
/// border — the widths the columns were actually allocated, minus the two
/// pad spaces each side of a cell.
pub(super) fn grid_column_widths(rows: &[String]) -> Vec<usize> {
    // The border may carry the message bullet (`● ┌──…`) when the table is
    // the reply's first block, so slice from the corner itself.
    let top = rows
        .iter()
        .find_map(|r| r.find('┌').map(|i| &r[i..]))
        .expect("a grid top border");
    top.trim()
        .trim_start_matches('┌')
        .trim_end_matches('┐')
        .split('┬')
        .map(|seg| cols(seg).saturating_sub(2))
        .collect()
}

// --- tool-output view: the full conversation transcript (Ctrl+O overlay) ---

/// Drive an app through `user → "let me check" → Read(ok) → "all done"`.
pub(super) fn transcript_fixture() -> App {
    let mut app = App::new();
    app.record_user_message("hello");
    app.begin_stream();
    app.push_chunk("let me check");
    app.flush_streaming_segment();
    app.start_tool("Read", "f");
    app.end_tool("L1\nL2\nL3", true);
    app.push_chunk("all done");
    app.finish_stream();
    app
}

/// The transcript rows *after* the banner chrome (banner + spacer),
/// trimmed — for tests asserting on the conversation walk itself (the
/// banner atop it has its own tests).
pub(super) fn transcript_body(app: &App, width: u16) -> Vec<String> {
    let chrome = header_lines(app, width).len() + 1;
    transcript_lines(app, width)
        .iter()
        .skip(chrome)
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

// ===== Ctrl+D context-debug view (docs/context.md) =====

/// A conversation with a system prompt, a user turn, a raw tool record,
/// and a summary — everything the context window derives from.
pub(super) fn context_fixture() -> App {
    let mut app = transcript_fixture();
    app.set_system_prompt(Some("be nice".to_string()));
    app.end_turn(2);
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

/// The RGB triple of a span's foreground (panics on a non-RGB colour).
pub(super) fn span_rgb(span: &Span) -> (u8, u8, u8) {
    match span.style.fg {
        Some(Color::Rgb(r, g, b)) => (r, g, b),
        other => panic!("expected an RGB fg, got {other:?}"),
    }
}

/// The spinner contributes the first [`SPINNER_SPAN_COUNT`] spans of the
/// status line; the shimmering verb's per-char spans start right after.
pub(super) const VERB_START: usize = SPINNER_SPAN_COUNT;

/// A live status carrying a retry indicator (verb fixed to "Working").
pub(super) fn status_retrying(attempt: u32, max: u32, tokens: usize) -> TurnStatus {
    let mut s = status(tokens, TokenArrow::Up, 5, None);
    s.retry = Some(RetryInfo { attempt, max });
    s
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
    }
}

// --- the session-context footer under the box (docs/footer.md) ---

/// An app with session info injected, as `main.rs` does at startup.
pub(super) fn with_session() -> App {
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/alter-zero");
    app
}

// --- the startup header banner (docs/header.md) ---

/// The whole banner as one plain string (rows joined by newlines).
pub(super) fn header_text(app: &App, width: u16) -> String {
    header_lines(app, width)
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n")
}

// --- terminal-size sweep ---

/// Mixed-width stress text: prose, wide CJK, emoji, and an unbreakable
/// over-long token, so the sweep hits every wrap branch.
const SWEEP_TEXT: &str = "The quick brown fox 世界你好 mixes wide CJK with \
    emoji 🎉🎊 and averyveryverylongunbreakabletokenthatmusthardbreak too.";

/// One busy `App` per live-region feature, so the sweep exercises every
/// width-dependent render path on top of a shared finished history
/// (messages, a tool call, a summary, the session footer).
fn size_sweep_apps() -> Vec<(&'static str, App)> {
    let base = || {
        let mut app = App::new();
        app.set_session_info("dummy_model_name", "~/repo/some/longish/path");
        app.record_user_message("first message with CJK 世界 and emoji 🎉");
        app.record_system_message("help text\nwith a second line");
        app.begin_stream();
        app.push_chunk("text before the tool call. ");
        app.start_tool("read_file", "src/app.rs with a long argument string");
        app.end_tool(SWEEP_TEXT, true);
        app.push_chunk(SWEEP_TEXT);
        app.finish_stream();
        app.end_turn(3);
        app
    };
    let streaming = || {
        let mut app = base();
        app.begin_stream();
        app.push_chunk(SWEEP_TEXT);
        app.set_status_times(Duration::from_secs(7), None);
        app
    };
    let tool = {
        let mut app = base();
        app.begin_stream();
        app.push_chunk("before tool ");
        app.start_tool("write_file", SWEEP_TEXT);
        app.set_status_times(Duration::from_secs(7), Some(Duration::from_secs(2)));
        app
    };
    let queued = {
        let mut app = streaming();
        app.queued.push_back(batch(&["queued one with CJK 世界"]));
        app
    };
    let menu = {
        let mut app = base();
        app.input = TextArea::from_text("/");
        app.command_menu = Some(crate::app::CommandMenu { selected: 0 });
        app
    };
    let shortcuts = {
        let mut app = base();
        app.shortcuts_open = true;
        app
    };
    let draft = {
        let mut app = base();
        app.input = TextArea::from_text(&format!("{SWEEP_TEXT}\n{SWEEP_TEXT}"));
        app
    };
    let file_picker = {
        // An open `@` picker with match indices deep enough in a long
        // mixed-width path that tiny widths truncate past them, so
        // `file_menu_row`'s truncate + match-span grouping is swept too.
        let long = "src/some/deeply/nested/世界 with spaces/🎉emoji/averylongfilename.rs";
        let mut app = base();
        app.input = TextArea::from_text("@src");
        app.file_search = Some(FileSearch {
            selected: 0,
            query: "src".into(),
            matches: vec![
                FileMatch {
                    path: "src/app.rs".into(),
                    score: 10,
                    indices: vec![0, 1, 2],
                },
                FileMatch {
                    path: long.into(),
                    // Byte offsets of `s`, `r`, `c`, `世`, `界`, `🎉`, `a`,
                    // and `l` — the last five land past a narrow truncation.
                    score: 5,
                    indices: vec![0, 1, 2, 23, 26, 42, 52, 57],
                },
            ],
            waiting: false,
        });
        app
    };
    vec![
        ("idle", base()),
        ("streaming", streaming()),
        ("tool", tool),
        ("queued", queued),
        ("menu", menu),
        ("shortcuts", shortcuts),
        ("draft", draft),
        ("file_picker", file_picker),
    ]
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

// --- the `!` shell mode + its exec cell (docs/shell-command.md) ---

/// An app in shell mode with `command` typed (the bang absorbed into the
/// mode flag, codex-style — the textarea holds just the command).
pub(super) fn shelling(command: &str) -> App {
    let mut app = App::new();
    app.shell_mode = true;
    app.input = TextArea::from_text(command);
    app
}

// --- the `@` file picker (docs/file-search.md) ---

/// A bare file match for the picker render tests.
pub(super) fn fmatch(path: &str) -> FileMatch {
    FileMatch {
        path: path.to_string(),
        score: 0,
        indices: Vec::new(),
    }
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

// ===== /resume session picker (docs/resume.md) =====

/// An app with the picker open over one session per preview, all aged
/// `5m ago` (updated) / `2h ago` (created), recorded in the picker's own
/// cwd, at paths `s0`, `s1`, ….
pub(super) fn resume_app(previews: &[&str]) -> App {
    let mut app = App::new();
    app.open_resume_picker(
        previews
            .iter()
            .enumerate()
            .map(|(i, preview)| crate::session::SessionSummary {
                path: std::path::PathBuf::from(format!("s{i}")),
                updated_secs: 300,
                created_secs: 7_200,
                cwd: "/repo".into(),
                preview: (*preview).into(),
            })
            .collect(),
        "/repo".into(),
    );
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

fn login_choices() -> Vec<ProviderChoice> {
    vec![
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

pub(super) fn login_app_provider() -> App {
    let mut app = App::new();
    app.open_key_onboarding(login_choices(), "~/.alter-zero/.env");
    app
}

pub(super) fn login_app_key() -> App {
    let mut app = login_app_provider();
    // Advance to the masked key step for the highlighted provider (index 0).
    let onboarding = app.key_onboarding.as_mut().unwrap();
    onboarding.step = KeyStep::Key;
    onboarding.chosen = Some(0);
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
        timestamp: String::new(),
    }
}

// ===== The Agent tool's cells + roster (docs/agent-tool.md) =====

pub(super) fn agent_entry(
    id: &str,
    desc: &str,
    status: crate::agents::AgentStatus,
) -> crate::app::AgentGroupEntry {
    crate::app::AgentGroupEntry {
        id: id.to_string(),
        description: desc.to_string(),
        agent_type: "general-purpose".to_string(),
        prompt: format!("What is the weather in {desc}?"),
        status,
        tool_uses: 2,
        tokens: 16_100,
        secs: 39,
        result: "It is 19°C.".to_string(),
        tool_headers: vec!["Bash(curl wttr.in)".to_string()],
        output: "It is 19°C.".to_string(),
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
