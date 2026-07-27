//! The dummy AI and the backend seam.
//!
//! [`dummy_response`] and [`chunks`] are pure and unit-tested. [`DummyAi`] is the
//! built-in [`ReplySource`]: a thin background thread that pushes the chunks onto
//! the event channel with a small delay so the reply visibly streams, stopping
//! early if its [`CancelToken`] is tripped. Swap in a real model by implementing
//! [`ReplySource`] — the event loop depends only on the trait, not on this dummy.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::context::ContextMessage;

/// One call in a [`StreamEvent::ToolBatch`] announcement: the `name` + short
/// `args` summary the `● name(args)` header shows — the *same* two strings the
/// call's own [`StreamEvent::ToolStart`] carries, so a `⎿ Waiting…` cell's header
/// matches the header it shows once it starts running. Carries no output/status —
/// those arrive later via `ToolStart`/`ToolEnd`. See `docs/parallel-tools.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallSummary {
    pub name: String,
    pub args: String,
}

/// One subagent announced by a [`StreamEvent::AgentBatch`]: the identity the
/// roster entry ([`crate::agents::AgentRun`]) is created from. `id` is the
/// registry id its agent-channel events will carry; `background` mirrors the
/// batch's flag (every spec in one batch shares it). See `docs/agent-tool.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    pub id: String,
    pub description: String,
    pub agent_type: String,
    pub prompt: String,
    pub background: bool,
}

/// One `agent` tool call's resolution inside a [`StreamEvent::AgentGroupDone`]:
/// the model-facing tool-result text for the agent `id` — the framed final
/// response (foreground), the launch acknowledgement (background), or the
/// stopped/failed note. `ok` picks the recorded entry's red/green when the
/// roster can no longer say (it mirrors the subagent's own outcome).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCallDone {
    pub id: String,
    pub output: String,
    pub ok: bool,
}

/// Real token usage reported by the provider for one completed request round
/// — the final `usage` frame of an OpenAI-compatible stream (asked for via
/// `stream_options.include_usage`). Unlike the app-side tiktoken estimate it
/// counts **everything** the request billed — system prompt, full context,
/// reasoning — and carries the prompt-cache detail. The loop folds it into
/// the live tally via [`crate::app::App::apply_usage`]. See
/// `docs/prompt-caching.md`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    /// Prompt tokens billed for the round (`prompt_tokens`) — cached reads
    /// included, per the OpenAI accounting shape.
    pub input: u64,
    /// Completion tokens the round generated (`completion_tokens`).
    pub output: u64,
    /// Input tokens served from the provider's prompt cache (a subset of
    /// `input`): `prompt_tokens_details.cached_tokens`, or the
    /// `cache_read_input_tokens` alias Venice/Anthropic-style shims use.
    pub cached: u64,
    /// Input tokens written to the cache by this round (Anthropic-style
    /// explicit caching): `prompt_tokens_details.cache_write_tokens` /
    /// `cache_creation_input_tokens`.
    pub cache_write: u64,
}

impl TokenUsage {
    /// The round's billed total — what the tally grows by.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.input + self.output
    }
}

/// What a backend sends to the event loop. Only the *reply* travels this channel
/// — keyboard input arrives separately via the terminal event stream (see
/// `main.rs`). It is a tokio unbounded channel so the async loop can `select!` on
/// it; the backend thread sends without ever touching the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// A piece of the reply (typically one word).
    Chunk(String),
    /// The model requested a **parallel batch** of tool calls this round,
    /// announced up front — *before* the first [`StreamEvent::ToolStart`] — so the
    /// UI can show every requested call at once, the ones not yet executing as
    /// dim `⎿ Waiting…` cells. Each [`ToolCallSummary`] is the `name`/`args` the
    /// `● name(args)` header shows (the same summary the matching `ToolStart`
    /// carries). Sequential execution then transitions the calls one at a time via
    /// [`StreamEvent::ToolStart`]/[`StreamEvent::ToolEnd`]. A backend that never
    /// batches (the `!` shell, the dummy's lone calls) simply omits this — a lone
    /// `ToolStart` still works. See `docs/parallel-tools.md`.
    ToolBatch(Vec<ToolCallSummary>),
    /// A tool call has started executing. The loop shows it live (blue) until the
    /// matching [`StreamEvent::ToolEnd`] arrives. `args` is a short summary for
    /// the `name(args)` header. When a [`StreamEvent::ToolBatch`] announced this
    /// call, this flips its `⎿ Waiting…` cell to `⎿ Running…`.
    ///
    /// `detail` is the call's **human-readable description** when the model
    /// supplied one (a `bash` call's `description` argument) — `None`
    /// otherwise. The main tool cells ignore it (their headers show the
    /// command, Claude-Code style); the agent roster's tree rows prefer it
    /// for their sticky `{Name}: {detail}` activity line
    /// (`docs/agent-tool.md`).
    ToolStart {
        name: String,
        args: String,
        detail: Option<String>,
    },
    /// The in-flight tool call finished with this `output` and outcome (`ok` →
    /// green, else red). Always follows a [`StreamEvent::ToolStart`].
    ///
    /// `truncated` is `true` when the output exceeded the in-memory cap and was
    /// cut — `output` then holds only the retained head, and the cell appends a
    /// `…` marker (see `docs/shell-command.md`). The `!` shell runner sets it; a
    /// normal backend tool always sends `false`.
    ToolEnd {
        output: String,
        ok: bool,
        truncated: bool,
    },
    /// The in-flight tool call resolved by **moving to the background**
    /// (a `run_in_background` bash call, or Ctrl+B on a running command) —
    /// sent **in place of** [`StreamEvent::ToolEnd`]. `id` is the registry
    /// task id; `output` is the model-facing launch text (the task id +
    /// interim-output path) that becomes the tool result — the cell instead
    /// renders the fixed `⎿ Running in the background (↓ to manage)` row
    /// ([`crate::app::ToolStatus::Backgrounded`]). The process itself reports
    /// through the separate background channel. See `docs/background.md`.
    ToolBackgrounded { id: String, output: String },
    /// A chunk of the **currently-running** tool's output, streamed live as it
    /// is produced (one or more complete lines, stdout+stderr merged in arrival
    /// order), between the call's [`StreamEvent::ToolStart`] and its
    /// [`StreamEvent::ToolEnd`]. The loop appends it to the front running call
    /// ([`crate::app::App::push_tool_output`]) so the live cell **tails** it —
    /// Claude-Code's running-command look (see `docs/tool-streaming.md`). The
    /// authoritative full output still arrives in `ToolEnd`, which overwrites
    /// the tailed partial, so a dropped chunk never corrupts the final cell.
    /// Only the real `bash` executor emits this today; a backend that never
    /// streams simply omits it.
    ToolOutput(String),
    /// The model launched a **group of subagents** this round (its `agent`
    /// tool calls), announced up front like a [`StreamEvent::ToolBatch`]: the
    /// loop seeds the roster (one [`crate::agents::AgentRun`] per spec) and
    /// shows the live group cell — `● Running {n} agents…` over the per-agent
    /// tree rows — while each agent's own stream reports on the dedicated
    /// agent channel. A `background` group resolves immediately (its
    /// [`StreamEvent::AgentGroupDone`] follows at once); a foreground group
    /// stays live until every agent finishes. See `docs/agent-tool.md`.
    AgentBatch {
        background: bool,
        agents: Vec<AgentSpec>,
    },
    /// The announced agent group resolved: every foreground agent finished
    /// (or the background launch was acknowledged, or Ctrl+B moved the rest
    /// to the background — `background` reports the *resolved* mode). Carries
    /// each call's model-facing tool-result text; the loop drains the agent
    /// channel first (the terminal roster events were enqueued before this
    /// was sent), then records the [`crate::app::AgentGroup`] history item
    /// and commits the group cell. See `docs/agent-tool.md`.
    AgentGroupDone {
        background: bool,
        agents: Vec<AgentCallDone>,
    },
    /// The model began a "thinking" (reasoning) phase. The loop shows
    /// `· Thinking for Ns` in the live status line until the matching
    /// [`StreamEvent::ThinkingEnd`] arrives.
    ThinkingStart,
    /// A piece of reasoning text streamed during the thinking phase (a real
    /// API's reasoning delta). Opaque — never rendered — but counted into the
    /// live token tally, so the count keeps ticking while the model thinks.
    /// Only sent between a [`StreamEvent::ThinkingStart`] and its
    /// [`StreamEvent::ThinkingEnd`].
    ThinkingChunk(String),
    /// The model's thinking phase ended. Always follows a
    /// [`StreamEvent::ThinkingStart`]; drops the `Thinking for Ns` suffix.
    ThinkingEnd,
    /// A fragment of a tool call the model is **generating** — the streamed
    /// `name`/`arguments` pieces of a `tool_calls` delta, before the tool runs.
    /// Opaque JSON, never rendered, but counted into the live token tally (like
    /// [`StreamEvent::ThinkingChunk`]) so the status keeps ticking while the
    /// model generates the call. Emitted by a real backend during a
    /// tool-calling round, ahead of the [`StreamEvent::ToolStart`] that begins
    /// executing it. See `docs/status-indicator.md`.
    ToolCallDelta(String),
    /// A failed request is being retried (the connection/send failed before any
    /// content streamed, or the server returned a transient status). Carries the
    /// 1-based retry number and the ceiling, shown live in the status line as
    /// `retrying {attempt}/{max}`. Only a real backend sends this (see
    /// `llm::retry`); the loop shows it in the status and keeps the turn alive.
    Retrying { attempt: u32, max: u32 },
    /// The provider's real token usage for one completed request round (the
    /// final usage frame of the stream — sent once per round by a real
    /// backend, so an agentic turn reports one per tool round). The loop
    /// snaps the live tally to it ([`crate::app::App::apply_usage`]), turning
    /// the app-side estimate into the provider's own accounting, prompt-cache
    /// detail included. The dummy never sends it. See `docs/prompt-caching.md`.
    Usage(TokenUsage),
    /// The backend failed; carries a human-readable message to show the user.
    Error(String),
    /// The reply is complete.
    StreamDone,
}

/// How long [`DummyAi`] waits after a turn starts before streaming its first
/// chunk — so the status indicator (the spinner, the ticking elapsed timer, and
/// the `↑ N tokens` count for the just-sent user message) is visible *before*
/// any reply text appears. The status animates during this pause because the
/// draw loop re-arms a frame every 32ms while a turn is active (see `main.rs`),
/// and an Esc reaps the thread promptly (the wait is an interruptible
/// [`nap`]). Configurable per backend via [`DummyAi::with_startup_delay`] (the
/// app reads `ALTER_ZERO_STARTUP_DELAY_MS`; tests use a short delay).
pub const STARTUP_DELAY: Duration = Duration::from_secs(3);

/// Delay between streamed chunks. Small enough to feel responsive, large
/// enough that the word-by-word reveal is visible.
pub const CHUNK_DELAY: Duration = Duration::from_millis(45);

/// How long a dummy tool "runs" — the pause between its `ToolStart` and
/// `ToolEnd` — so the blue running state is visible before it resolves.
pub const TOOL_DELAY: Duration = Duration::from_millis(450);

/// Delay after `ThinkingStart` and after each `ThinkingChunk`, so the
/// reasoning trickles and the token tally visibly ticks while the model
/// "thinks". The phase's total length is one step per event —
/// `(1 + chunks) × THINK_CHUNK_DELAY` (≈1.2s for [`DUMMY_THINKING`]'s seven
/// words), long enough that `Thinking for Ns` ticks from 0s.
pub const THINK_CHUNK_DELAY: Duration = Duration::from_millis(150);

/// The dummy's canned reasoning, streamed word-by-word as
/// [`StreamEvent::ThinkingChunk`]s during its thinking phase. Never shown —
/// it only feeds the token tally (like a real API's reasoning deltas).
const DUMMY_THINKING: &str = "Let me look at the code first.";

/// The dummy's canned tool-call "generation" fragments, streamed as
/// [`StreamEvent::ToolCallDelta`]s just before each `ToolStart` — the pieces a
/// real model emits while producing a `tool_calls` request. Never shown; they
/// only feed the token tally so the status ticks while the model *generates*
/// the call (like [`DUMMY_THINKING`] does for reasoning). See
/// `docs/status-indicator.md`.
const DUMMY_READ_CALL: &[&str] = &["read", "{\"path\":", "\"src/main.rs\"}"];
const DUMMY_BASH_CALL: &[&str] = &["bash", "{\"command\":", "\"ping x.invalid\"}"];

/// Canned multi-line output for the dummy `Read` tool (resolves green).
const DUMMY_READ_OUTPUT: &str = "fn main() -> io::Result<()> {\n    \
    let mut term = InlineViewport::init(ui::LIVE_MIN_HEIGHT)?;\n    \
    let result = run(&mut term);\n    \
    let restored = term.restore();\n    \
    result.and(restored)\n}";

/// The dummy's **parallel batch** command + output for the `Bash` half. The turn
/// announces a two-call batch — a `Read` then this `Bash` — up front, so while
/// the `Read` runs the `Bash` shows `⎿ Waiting…` (the visible batch, offline; see
/// `docs/parallel-tools.md`). Kept to two calls with the same output footprint as
/// the pre-batch demo so the committed scrollback is unchanged (a real backend
/// renders however many parallel calls the model actually requests — the display
/// scales to N). This `Bash` resolves **red** (an unresolvable host), so the demo
/// still shows both a green (`Read`) and a red outcome.
const DUMMY_BASH_CMD: &str = "ping -c 3 x.invalid";
const DUMMY_BASH_OUTPUT: &str = "ping: cannot resolve x.invalid: Unknown host\nexit status 68";

/// The dummy's vivid **three-call parallel `Bash(ping …)` batch** — the user's
/// example. Shown **only when the prompt mentions "parallel"** (opt-in), so the
/// default turn keeps its compact two-call batch and every unrelated smoke phase
/// keeps its footprint; the dedicated phase (and `cargo run` with a "parallel"
/// prompt) triggers this. All three are announced up front, so while the first
/// runs the other two show `⎿ Waiting…`; two resolve green, one red. Each entry
/// is `(command, output, ok)`. See `docs/parallel-tools.md`.
const DUMMY_PARALLEL_BATCH: &[(&str, &str, bool)] = &[
    ("ping -c 20 google.com", DUMMY_PING_GOOGLE, true),
    ("ping -c 20 facebook.com", DUMMY_PING_FACEBOOK, true),
    ("ping -c 20 x.invalid", DUMMY_PING_FAIL, false),
];

const DUMMY_PING_GOOGLE: &str = "PING google.com (142.250.72.14): 56 data bytes\n\
    64 bytes from 142.250.72.14: icmp_seq=0 ttl=117 time=12.3 ms\n\
    64 bytes from 142.250.72.14: icmp_seq=1 ttl=117 time=11.8 ms\n\
    64 bytes from 142.250.72.14: icmp_seq=2 ttl=117 time=12.0 ms\n\
    --- google.com ping statistics ---\n\
    3 packets transmitted, 3 packets received, 0.0% packet loss";
const DUMMY_PING_FACEBOOK: &str = "PING facebook.com (157.240.1.35): 56 data bytes\n\
    64 bytes from 157.240.1.35: icmp_seq=0 ttl=52 time=41.6 ms\n\
    64 bytes from 157.240.1.35: icmp_seq=1 ttl=52 time=39.2 ms\n\
    --- facebook.com ping statistics ---\n\
    2 packets transmitted, 2 packets received, 0.0% packet loss";
const DUMMY_PING_FAIL: &str = "ping: cannot resolve x.invalid: Unknown host\nexit status 68";

/// The dummy's **subagent demo** (`docs/agent-tool.md`), played for a prompt
/// mentioning "agents" (opt-in, like "parallel"/"table" — but never for
/// `/init`, whose canned prompt names `AGENTS.md`): a two-agent group is
/// announced, "runs" for [`AGENT_DELAY`] (the live tree cell shows, each row
/// `⎿ Initializing…`), then resolves with canned final responses. A prompt
/// also mentioning "background" launches the group in background mode instead
/// — the calls resolve at once with launch texts and the roster entries stay
/// running (stoppable with `x`, the manager demo). Each entry is
/// `(id, description, prompt, response)`; ids use the roster's `a…` shape.
const DUMMY_AGENTS: &[(&str, &str, &str, &str)] = &[
    (
        "ademowars",
        "Fetch current weather and time in Warsaw",
        "What is the current weather and time in Warsaw, Poland? Provide the \
         temperature, conditions, and local time.",
        "Warsaw is currently 19°C and partly cloudy; the local time is 14:32 CEST.",
    ),
    (
        "ademomnla",
        "Fetch current weather and time in Manila",
        "What is the current weather and time in Manila, Philippines? Provide the \
         temperature, conditions, and local time.",
        "Manila is currently 28°C with patchy rain; the local time is 20:32 PST.",
    ),
];

/// How long the dummy's scripted agent group "runs" between its announcement
/// and its resolution — long enough that the live tree cell (and the footer
/// roster's `Initializing…` rows) are visible.
pub const AGENT_DELAY: Duration = Duration::from_millis(1600);

/// The dummy's **markdown table** demo reply, played for any prompt mentioning
/// "table" (opt-in, like the "parallel" batch): prose, a 10-row GFM table whose
/// cells carry `` `code` `` spans **and a two-column-wide status emoji**, then
/// closing prose. Its turn is text-only —
/// no thinking phase and no tool calls, which would split the reply around them
/// and flush the block early — so the whole forming table previews in the strip
/// and its block commits at the close (docs/table-streaming.md). This is the
/// reported blank-band regression's shape: the strip collapses from the tall
/// forming-table preview to one row in the same frame the block's rows flush,
/// which the smoke suite guards (the box must stay flush at the bottom).
///
/// The emoji make it the **emoji-table** shape too: a wide grapheme costs two
/// terminal columns, so a mismeasured one tears the grid's right border out of
/// line — the reported "emoji cuts the table" (`term::visible_cells`,
/// docs/table-streaming.md), which the smoke suite guards by asserting every
/// grid row is the same width.
const DUMMY_TABLE_REPLY: &str = "Here's a table with data that uses backticks:\n\n\
| ID | Name | Code Snippet | Description |\n\
|----|------|--------------|-------------|\n\
| 1 | Hello World | `` `print(\"Hello\")` `` | ✅ Basic greeting function |\n\
| 2 | SQL Query | `` `SELECT * FROM users` `` | ✅ Database selection query |\n\
| 3 | Markdown | `` `**bold text**` `` | ✅ Formatting example |\n\
| 4 | Shell Command | `` `ls -la` `` | ✅ List directory contents |\n\
| 5 | JavaScript | `` `const x = 42;` `` | ❌ Variable declaration |\n\
| 6 | Rust | `` `let mut vec = Vec::new();` `` | ✅ Mutable vector creation |\n\
| 7 | Python | `` `def foo(): return None` `` | ✅ Empty function definition |\n\
| 8 | HTML | `` `<div class=\"container\">` `` | ❌ Container element |\n\
| 9 | CSS | `` `.class { color: red; }` `` | ✅ Style rule |\n\
| 10 | Regex | `` `/^[A-Z]+$/` `` | ✅ Pattern matching |\n\n\
The backticks are wrapped in double backticks (`` `code` ``) so they display \
properly in Markdown.";

/// The opening of codex's `/compact` summarization prompt
/// ([`crate::context::SUMMARIZATION_PROMPT`]) — how [`turn_events`] recognizes
/// a compact turn's request and scripts a text-only summary for it.
const COMPACT_PROMPT_MARKER: &str = "You are performing a CONTEXT CHECKPOINT COMPACTION";

/// The dummy's canned `/compact` summary — streamed word-by-word like every
/// reply, captured (never rendered) by the compact turn (docs/compact.md).
const DUMMY_COMPACT_SUMMARY: &str = "Progress so far: this is a canned handoff summary from the dummy backend. \
     Key decisions: none - no real model is attached. Next steps: keep \
     chatting; the compacted context now rides this summary.";

/// Canned replies. One is chosen deterministically per prompt so the demo has
/// a little variety without any real model behind it.
const RESPONSES: &[&str] = &[
    "Sure! This is a streaming demo, so I'm a dummy reply rather than a real \
     model. Notice how each word appears on its own and longer answers wrap to \
     fit your terminal — try resizing the window while I talk.",
    "Great question. There's no AI behind this yet — these words are streamed \
     from a canned response to show off the inline TUI. Finished messages \
     scroll up into your normal terminal history, just like Claude Code.",
    "Happy to help! For now I only pretend to think. The point of this little \
     program is the rendering: a bottom-pinned input box, live streaming, and \
     a layout that reflows responsively as the terminal changes size.",
];

/// Pick a deterministic dummy reply for a prompt.
///
/// Deterministic so it's testable; varied so the demo isn't monotonous. A
/// prompt mentioning **"table"** plays the markdown-table demo
/// ([`DUMMY_TABLE_REPLY`], opt-in like the "parallel" batch).
#[must_use]
pub fn dummy_response(prompt: &str) -> String {
    if prompt.to_lowercase().contains("table") {
        return DUMMY_TABLE_REPLY.to_string();
    }
    let index = prompt.chars().count() % RESPONSES.len();
    RESPONSES[index].to_string()
}

/// Split text into streamable chunks, one per whitespace-delimited word.
///
/// Each chunk keeps its trailing space (`split_inclusive`), so concatenating
/// the chunks reproduces the input exactly — which keeps streaming faithful.
#[must_use]
pub fn chunks(text: &str) -> Vec<String> {
    text.split_inclusive(' ').map(str::to_string).collect()
}

/// A canned tool output as per-line [`StreamEvent::ToolOutput`] chunks (each
/// line keeping its `\n`), so the dummy streams a `Bash` cell's output the way
/// the real executor does — the live cell **tails** it as it arrives, before the
/// authoritative `ToolEnd`. Concatenated, the chunks equal `output`. See
/// `docs/tool-streaming.md`.
#[must_use]
fn tool_output_events(output: &str) -> Vec<StreamEvent> {
    output
        .split_inclusive('\n')
        .map(|line| StreamEvent::ToolOutput(line.to_string()))
        .collect()
}

/// The dummy's leading acknowledgement for `count` pasted images, or `None` when
/// none are attached. The dummy has no vision (see [`turn_events`]); this is a
/// stand-in so the demo visibly reflects that the images reached the backend.
/// Pluralised, with a trailing space so it streams as a leading chunk.
#[must_use]
pub fn image_ack(count: usize) -> Option<String> {
    match count {
        0 => None,
        1 => Some("Looking at your 1 image. ".to_string()),
        n => Some(format!("Looking at your {n} images. ")),
    }
}

/// The full ordered sequence of events for one dummy turn, with a thinking phase
/// and tool calls **interleaved** in the reply: stream the first half of the
/// text, *think* for a moment, run a **parallel batch** announced up front — so
/// its not-yet-run calls show `⎿ Waiting…` while the front one runs
/// (`docs/parallel-tools.md`) — then stream the rest and finish.
///
/// The batch is **prompt-gated**: a prompt mentioning "parallel" runs the vivid
/// three-call `Bash(ping …)` demo (the user's example); any other prompt runs the
/// compact two-call `Read`+`Bash` batch (baseline footprint, so unrelated smoke
/// phases keep their sizing, with the feature still visible every turn).
///
/// The thinking phase sits after the first text segment (so the demo shows
/// `↓ tokens · Thinking for Ns`) and before the tools. The batch is announced via
/// a [`StreamEvent::ToolBatch`] before its `ToolStart`s, and every `ToolStart` is
/// still immediately followed by its `ToolEnd` — execution stays sequential (one
/// running call at a time; see `docs/parallel-tools.md`). Between the thinking
/// pair the dummy streams [`DUMMY_THINKING`] word-by-word as
/// [`StreamEvent::ThinkingChunk`]s, so the token tally keeps ticking while the
/// thinking timer runs.
///
/// Pure and deterministic so it is unit-testable; [`DummyAi`] just plays it back
/// on a thread with delays. The `Chunk` events still concatenate to exactly
/// [`dummy_response`], so streaming stays faithful.
#[must_use]
pub fn turn_events(prompt: &str, image_count: usize) -> Vec<StreamEvent> {
    // `/compact`'s summarization request plays a **text-only** canned summary
    // — no thinking phase, no tool batch (codex sends the summarize request
    // with no tools) — so the offline dummy path (and smoke.sh) can drive the
    // whole compact flow with no provider (docs/compact.md).
    if prompt.starts_with(COMPACT_PROMPT_MARKER) {
        let mut events: Vec<StreamEvent> = chunks(DUMMY_COMPACT_SUMMARY)
            .into_iter()
            .map(StreamEvent::Chunk)
            .collect();
        events.push(StreamEvent::StreamDone);
        return events;
    }
    let reply = dummy_response(prompt);
    let words: Vec<&str> = reply.split_inclusive(' ').collect();
    let mid = (words.len() / 2).max(1).min(words.len());
    let first: String = words[..mid].concat();
    let second: String = words[mid..].concat();

    let mut events = Vec::new();
    // The dummy can't actually see images, so when some are attached it opens by
    // acknowledging them — visible proof the typed image channel (codex's
    // `UserInput::LocalImage`) carried the paths to the backend. A real vision
    // model would read the files instead. See `docs/image-paste.md`.
    if let Some(ack) = image_ack(image_count) {
        events.extend(chunks(&ack).into_iter().map(StreamEvent::Chunk));
    }
    // A "table" prompt plays the markdown-table demo as a **text-only** turn:
    // no thinking pause and no tool batch — a tool call would split the reply
    // around it, flushing the block early — so the whole table streams through
    // the strip preview and its block commits at the close
    // (docs/table-streaming.md).
    if prompt.to_lowercase().contains("table") {
        events.extend(chunks(&reply).into_iter().map(StreamEvent::Chunk));
        events.push(StreamEvent::StreamDone);
        return events;
    }
    // An "agents" prompt plays the subagent demo (docs/agent-tool.md): the
    // first half of the text, then the scripted two-agent group — announced,
    // "running" for AGENT_DELAY, resolved — then the closing text. Never for
    // /init (its canned prompt names AGENTS.md).
    let lower = prompt.to_lowercase();
    if lower.contains("agents") && !lower.contains("agents.md") {
        let background = lower.contains("background");
        events.extend(chunks(&first).into_iter().map(StreamEvent::Chunk));
        events.push(StreamEvent::AgentBatch {
            background,
            agents: DUMMY_AGENTS
                .iter()
                .map(|&(id, description, prompt, _)| AgentSpec {
                    id: id.to_string(),
                    description: description.to_string(),
                    agent_type: "general-purpose".to_string(),
                    prompt: prompt.to_string(),
                    background,
                })
                .collect(),
        });
        events.push(StreamEvent::AgentGroupDone {
            background,
            agents: DUMMY_AGENTS
                .iter()
                .map(|&(id, description, _, response)| AgentCallDone {
                    id: id.to_string(),
                    output: if background {
                        format!(
                            "Background agent launched with ID: {id} \
                             (\"{description}\"). You will be notified when it \
                             completes."
                        )
                    } else {
                        response.to_string()
                    },
                    ok: true,
                })
                .collect(),
        });
        events.extend(chunks(&second).into_iter().map(StreamEvent::Chunk));
        events.push(StreamEvent::StreamDone);
        return events;
    }
    events.extend(chunks(&first).into_iter().map(StreamEvent::Chunk));
    events.push(StreamEvent::ThinkingStart);
    events.extend(
        chunks(DUMMY_THINKING)
            .into_iter()
            .map(StreamEvent::ThinkingChunk),
    );
    events.push(StreamEvent::ThinkingEnd);
    // The model "generates" a **parallel batch** (its fragments tick the token
    // tally, like reasoning) and announces every call up front — so the not-yet-run
    // ones show `⎿ Waiting…` while the front one runs — then executes them in
    // order. Each ToolStart still lands immediately before its ToolEnd, so
    // execution stays sequential (one running call at a time; see
    // `docs/parallel-tools.md`).
    //
    // A prompt mentioning **"parallel"** triggers the vivid three-call
    // `Bash(ping …)` batch (the user's example); otherwise the default turn runs a
    // compact two-call `Read`+`Bash` batch (baseline footprint — so unrelated
    // smoke phases keep their sizing — with the feature still visible every turn).
    let summary = |name: &str, args: &str| ToolCallSummary {
        name: name.to_string(),
        args: args.to_string(),
    };
    if prompt.to_lowercase().contains("parallel") {
        for frag in DUMMY_BASH_CALL {
            events.push(StreamEvent::ToolCallDelta((*frag).to_string()));
        }
        events.push(StreamEvent::ToolBatch(
            DUMMY_PARALLEL_BATCH
                .iter()
                .map(|&(cmd, _, _)| summary("Bash", cmd))
                .collect(),
        ));
        for &(cmd, output, ok) in DUMMY_PARALLEL_BATCH {
            events.push(StreamEvent::ToolStart {
                name: "Bash".to_string(),
                args: cmd.to_string(),
                detail: None,
            });
            // Stream the output line-by-line so the live cell tails it, then the
            // authoritative ToolEnd commits the finished cell (docs/tool-streaming.md).
            events.extend(tool_output_events(output));
            events.push(StreamEvent::ToolEnd {
                output: output.to_string(),
                ok,
                truncated: false,
            });
        }
    } else {
        for frag in DUMMY_READ_CALL {
            events.push(StreamEvent::ToolCallDelta((*frag).to_string()));
        }
        for frag in DUMMY_BASH_CALL {
            events.push(StreamEvent::ToolCallDelta((*frag).to_string()));
        }
        events.push(StreamEvent::ToolBatch(vec![
            summary("Read", "src/main.rs"),
            summary("Bash", DUMMY_BASH_CMD),
        ]));
        events.push(StreamEvent::ToolStart {
            name: "Read".to_string(),
            args: "src/main.rs".to_string(),
            detail: None,
        });
        events.push(StreamEvent::ToolEnd {
            output: DUMMY_READ_OUTPUT.to_string(),
            ok: true,
            truncated: false,
        });
        events.push(StreamEvent::ToolStart {
            name: "Bash".to_string(),
            args: DUMMY_BASH_CMD.to_string(),
            detail: None,
        });
        // Stream the output line-by-line so the live cell tails it (the Read
        // above returns all at once, like the real executor). See
        // `docs/tool-streaming.md`.
        events.extend(tool_output_events(DUMMY_BASH_OUTPUT));
        events.push(StreamEvent::ToolEnd {
            output: DUMMY_BASH_OUTPUT.to_string(),
            ok: false,
            truncated: false,
        });
    }
    events.extend(chunks(&second).into_iter().map(StreamEvent::Chunk));
    events.push(StreamEvent::StreamDone);
    events
}

/// A cheap, cloneable cancellation flag shared between the event loop and a
/// running [`ReplySource`]. The loop calls [`CancelToken::cancel`] (e.g. on
/// quit); a well-behaved backend polls [`CancelToken::is_cancelled`] between
/// chunks and stops promptly.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh, uncancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent; observed by every clone.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Has cancellation been requested?
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// A source of streamed replies. Implement this to plug a real model into the
/// app: the event loop depends only on this trait, never on a concrete backend.
///
/// `spawn` must return promptly and do the work on a background thread (or task)
/// that *only sends* on `tx` — it must never read stdin (terminal init and
/// `insert_before` own stdin; see `main.rs`). It should poll `cancel` and stop
/// early when cancellation is requested, and may send [`StreamEvent::Error`] to
/// report a failure in place of [`StreamEvent::StreamDone`].
///
/// `images` are the temp-PNG paths of any Ctrl+V-pasted images attached to the
/// turn (codex's `UserInput::LocalImage` typed channel, distinct from the text
/// `prompt`): a real vision backend reads each file and attaches it to its
/// request. The built-in [`DummyAi`] has no vision, so it only acknowledges
/// their count. See `docs/image-paste.md`.
///
/// `context` is the whole conversation so far — derived from the history by
/// [`crate::context::context_messages`] right after the turn's user message
/// was recorded, so its last entry *is* the current message (text and
/// attachments included). A real backend sends it verbatim so the model never
/// loses context; the dummy ignores it (its replies are canned). See
/// `docs/context.md`.
pub trait ReplySource {
    fn spawn(
        &self,
        prompt: String,
        images: Vec<PathBuf>,
        context: Vec<ContextMessage>,
        tx: UnboundedSender<StreamEvent>,
        cancel: CancelToken,
    ) -> JoinHandle<()>;

    /// The model id this backend answers as, shown in the session-context
    /// footer under the input box (see `docs/footer.md`). A real backend
    /// returns its real model name.
    fn model_name(&self) -> String;

    /// The system prompt this backend prepends to every request, if any —
    /// surfaced so the Ctrl+D context-debug view can show the *whole* context
    /// window (see `docs/context.md`). The dummy sends none.
    fn system_prompt(&self) -> Option<String> {
        None
    }

    /// Send a chat message into a running/settled subagent's session
    /// (`docs/agent-tool.md`): queued into its loop at the next round
    /// boundary, or a continuation run when it is idle. Returns whether the
    /// message was accepted. The default (the dummy, backends without a
    /// subagent registry) declines — the loop raises a toast.
    fn spawn_agent_chat(&self, _id: &str, _text: &str) -> bool {
        false
    }
}

/// The built-in canned-reply backend used by the demo.
#[derive(Debug, Clone, Copy)]
pub struct DummyAi {
    /// Pause before the first streamed event so the status indicator shows
    /// first ([`STARTUP_DELAY`] by default; the app overrides it from
    /// `ALTER_ZERO_STARTUP_DELAY_MS`, tests use a short value).
    startup_delay: Duration,
}

impl Default for DummyAi {
    fn default() -> Self {
        Self {
            startup_delay: STARTUP_DELAY,
        }
    }
}

impl DummyAi {
    /// The default dummy: a [`STARTUP_DELAY`] pause before streaming.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A dummy with a custom pre-stream pause — the app threads
    /// `ALTER_ZERO_STARTUP_DELAY_MS` through here, and tests pass a short delay.
    #[must_use]
    pub fn with_startup_delay(startup_delay: Duration) -> Self {
        Self { startup_delay }
    }
}

impl ReplySource for DummyAi {
    /// Plays back [`turn_events`]: streams the reply word-by-word (with
    /// [`CHUNK_DELAY`] between words) with a **parallel batch** interleaved
    /// (announced up front, so the not-yet-run calls show `⎿ Waiting…` while the
    /// front one runs — three `Bash(ping …)` calls for a "parallel" prompt, else a
    /// compact `Read`+`Bash` batch), pausing [`TOOL_DELAY`] after each
    /// `ToolStart` so the blue running state shows before it resolves, and ends with
    /// [`StreamEvent::StreamDone`]. Stops early — sending nothing further — if
    /// `cancel` is tripped or the receiver has hung up. With `images` attached the
    /// reply opens with an acknowledgement (the dummy has no vision; see
    /// [`turn_events`] and `docs/image-paste.md`).
    fn spawn(
        &self,
        prompt: String,
        images: Vec<PathBuf>,
        _context: Vec<ContextMessage>, // canned replies — no context to use
        tx: UnboundedSender<StreamEvent>,
        cancel: CancelToken,
    ) -> JoinHandle<()> {
        let startup_delay = self.startup_delay;
        // The dummy can't read the files, only acknowledge how many arrived.
        let image_count = images.len();
        thread::spawn(move || {
            // Pause before streaming so the status indicator is visible first
            // (interruptibly — an Esc during the wait reaps the thread at once).
            nap(startup_delay, &cancel);
            if cancel.is_cancelled() {
                return;
            }
            for event in turn_events(&prompt, image_count) {
                if cancel.is_cancelled() {
                    return; // asked to stop — drop the rest quietly
                }
                // Pause *after* a word, a tool start, or a thinking event: a tool
                // "runs" for TOOL_DELAY (blue) before its ToolEnd resolves it, and
                // the model "thinks" one THINK_CHUNK_DELAY step per reasoning
                // event before its ThinkingEnd.
                let pause = match &event {
                    StreamEvent::Chunk(_) => Some(CHUNK_DELAY),
                    StreamEvent::ToolStart { .. } => Some(TOOL_DELAY),
                    // A foreground agent group "runs" between its announcement
                    // and its resolution so the live tree cell shows; a
                    // background launch resolves at once (docs/agent-tool.md).
                    StreamEvent::AgentBatch { background, .. } if !background => Some(AGENT_DELAY),
                    // Each streamed output line pauses like a word so the live
                    // cell visibly tails (docs/tool-streaming.md).
                    StreamEvent::ToolOutput(_) => Some(CHUNK_DELAY),
                    StreamEvent::ThinkingStart
                    | StreamEvent::ThinkingChunk(_)
                    | StreamEvent::ToolCallDelta(_) => Some(THINK_CHUNK_DELAY),
                    _ => None,
                };
                if tx.send(event).is_err() {
                    return; // receiver gone — stop quietly
                }
                if let Some(pause) = pause {
                    nap(pause, &cancel);
                }
            }
        })
    }

    /// The dummy's placeholder model id (a real backend reports its real one).
    fn model_name(&self) -> String {
        "dummy_model_name".to_string()
    }
}

/// A test-only backend that models a real network backend **parked in a
/// blocking read it cannot interrupt**: its thread sleeps for `stall` *without*
/// polling the [`CancelToken`], then — only once the stall elapses — observes
/// the cancel (streaming nothing) or, if it was never cancelled, streams a
/// one-word reply and finishes.
///
/// This is the failure mode the real [`crate::llm::LlmBackend`] hits during the
/// pre-first-token pause (blocked in `req.send()` / the first SSE `read`, which
/// only wake after one op-timeout — see `src/llm/openai.rs`), reproduced
/// deterministically and offline. The event loop must therefore **never
/// `join()`** a cancelled backend on its thread: doing so freezes the UI for
/// the whole stall (the interrupt-lag bug). Selected via `ALTER_ZERO_STALL_MS`
/// and used only by `scripts/smoke.sh` — never in normal operation. See
/// `docs/interrupt.md`.
#[derive(Debug, Clone, Copy)]
pub struct StallAi {
    stall: Duration,
}

impl StallAi {
    /// A stall backend that ignores the cancel for `stall` before it stops.
    #[must_use]
    pub fn new(stall: Duration) -> Self {
        Self { stall }
    }
}

impl ReplySource for StallAi {
    fn spawn(
        &self,
        prompt: String,
        _images: Vec<PathBuf>,
        _context: Vec<ContextMessage>,
        tx: UnboundedSender<StreamEvent>,
        cancel: CancelToken,
    ) -> JoinHandle<()> {
        let stall = self.stall;
        thread::spawn(move || {
            // Block for `stall` in ONE shot, deliberately *not* polling cancel —
            // this is the whole point of the double: a thread wedged in a
            // blocking syscall. A well-behaved loop cancels + detaches us and
            // stays responsive; a loop that join()s us here pays the full stall.
            thread::sleep(stall);
            if cancel.is_cancelled() {
                return; // cancelled while we were blocked — stop silently
            }
            let _ = tx.send(StreamEvent::Chunk(format!("Echo: {prompt}")));
            let _ = tx.send(StreamEvent::StreamDone);
        })
    }

    fn model_name(&self) -> String {
        "stall_model".to_string()
    }
}

/// Sleep up to `dur`, in short slices, returning early the moment `cancel` is
/// tripped — so a quit during a long tool "run" is still reaped promptly.
fn nap(dur: Duration, cancel: &CancelToken) {
    const SLICE: Duration = Duration::from_millis(20);
    let mut left = dur;
    while left > Duration::ZERO {
        if cancel.is_cancelled() {
            return;
        }
        let slice = SLICE.min(left);
        thread::sleep(slice);
        left -= slice;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn dummy_response_is_non_empty() {
        assert!(!dummy_response("hello").is_empty());
    }

    #[test]
    fn dummy_ai_reports_its_model_name() {
        // The footer under the input box names the active backend's model
        // (see docs/footer.md); the dummy reports its placeholder id.
        assert_eq!(DummyAi::default().model_name(), "dummy_model_name");
    }

    #[test]
    fn dummy_ai_waits_the_startup_delay_before_the_first_chunk() {
        // The dummy pauses before streaming so the status indicator (spinner,
        // ticking timer, `↑ N tokens` for the just-sent input) is visible
        // first. Use a short, deterministic delay; assert the first event only
        // arrives after it (a lower bound — the thread genuinely sleeps).
        let delay = Duration::from_millis(150);
        let (tx, mut rx) = unbounded_channel();
        let start = std::time::Instant::now();
        let handle = DummyAi::with_startup_delay(delay).spawn(
            "hi".to_string(),
            vec![],
            vec![],
            tx,
            CancelToken::new(),
        );
        let first = rx.blocking_recv().expect("a first event arrives");
        assert!(
            start.elapsed() >= delay,
            "the first chunk waits out the startup delay"
        );
        assert!(
            matches!(first, StreamEvent::Chunk(_)),
            "streaming still opens with reply text"
        );
        while rx.blocking_recv().is_some() {} // drain the rest
        handle.join().unwrap();
    }

    #[test]
    fn a_cancel_during_the_startup_delay_streams_nothing() {
        // Esc during the pre-stream pause must reap the thread at once — the
        // interruptible nap returns early and no events are sent.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let backend = DummyAi::with_startup_delay(Duration::from_secs(30));
        let handle = backend.spawn("hello".to_string(), vec![], vec![], tx, cancel.clone());
        cancel.cancel();
        handle.join().unwrap();
        assert!(
            rx.try_recv().is_err(),
            "a cancel during the delay streams nothing"
        );
    }

    #[test]
    fn dummy_response_is_deterministic() {
        assert_eq!(dummy_response("hello"), dummy_response("hello"));
    }

    #[test]
    fn a_table_prompt_streams_a_pure_table_turn() {
        // A prompt mentioning "table" plays the markdown-table demo: a
        // text-only turn — no thinking phase, no tool calls (a tool would
        // split the reply and flush the block early) — whose chunks
        // concatenate to the reply, ending in StreamDone. The reply carries a
        // multi-row GFM table with prose AFTER it, so the block closes
        // mid-stream: the strip-collapse geometry the smoke suite guards
        // (docs/table-streaming.md).
        let events = turn_events("show me a table", 0);
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Chunk(c) => Some(c.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, dummy_response("show me a table"));
        assert!(text.contains("| ID | Name |"), "carries the table: {text}");
        assert!(
            text.trim_end().ends_with("Markdown."),
            "prose follows the table so the block closes mid-stream"
        );
        assert!(
            !events.iter().any(|e| matches!(
                e,
                StreamEvent::ToolBatch(_)
                    | StreamEvent::ToolStart { .. }
                    | StreamEvent::ThinkingStart
            )),
            "a table turn is text-only"
        );
        assert!(matches!(events.last(), Some(StreamEvent::StreamDone)));
    }

    #[test]
    fn stall_ai_ignores_cancel_until_its_stall_elapses() {
        // The stall double models a backend wedged in a blocking read: a cancel
        // does NOT stop it early, so a caller that join()s it pays the whole
        // stall (the interrupt-lag freeze the loop must avoid — it detaches
        // instead; see docs/interrupt.md). A short stall keeps the test fast.
        let stall = Duration::from_millis(200);
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let start = std::time::Instant::now();
        let handle =
            StallAi::new(stall).spawn("hi".to_string(), vec![], vec![], tx, cancel.clone());
        cancel.cancel(); // interrupt immediately — the stall ignores it
        handle.join().unwrap();
        assert!(
            start.elapsed() >= stall,
            "joining a cancelled stall backend blocks for the full stall"
        );
        assert!(
            rx.try_recv().is_err(),
            "a stall cancelled mid-block streams nothing"
        );
    }

    #[test]
    fn stall_ai_streams_a_reply_when_not_cancelled() {
        // Left to run, the stall backend completes a normal turn, so the smoke
        // harness can also exercise an uninterrupted stalled turn.
        let (tx, mut rx) = unbounded_channel();
        let handle = StallAi::new(Duration::from_millis(10)).spawn(
            "ping".to_string(),
            vec![],
            vec![],
            tx,
            CancelToken::new(),
        );
        let first = rx.blocking_recv().expect("a chunk arrives");
        assert!(matches!(first, StreamEvent::Chunk(_)));
        assert_eq!(rx.blocking_recv(), Some(StreamEvent::StreamDone));
        handle.join().unwrap();
    }

    #[test]
    fn chunks_concatenate_back_to_the_original_text() {
        let text = "The quick brown fox jumps over the lazy dog.";
        assert_eq!(chunks(text).concat(), text);
    }

    #[test]
    fn chunks_splits_multi_word_text_into_several_pieces() {
        assert!(chunks("one two three").len() > 1);
    }

    #[test]
    fn chunks_of_empty_text_is_empty() {
        assert!(chunks("").is_empty());
    }

    #[test]
    fn turn_events_chunks_still_reconstruct_the_reply() {
        // Tool events are interleaved, but the Chunk events alone must still
        // concatenate to exactly the dummy reply.
        let prompt = "tell me something";
        let text: String = turn_events(prompt, 0)
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Chunk(c) => Some(c.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, dummy_response(prompt));
    }

    #[test]
    fn a_compact_prompt_plays_a_text_only_summary_turn() {
        // /compact's summarization prompt must never trigger the scripted tool
        // batch or thinking phase — codex sends the summarize request with no
        // tools, and the offline path (and smoke.sh) drives this branch
        // (docs/compact.md).
        let events = turn_events(crate::context::SUMMARIZATION_PROMPT, 0);
        assert!(
            events
                .iter()
                .all(|e| matches!(e, StreamEvent::Chunk(_) | StreamEvent::StreamDone)),
            "text-only: {events:?}"
        );
        assert!(matches!(events.last(), Some(StreamEvent::StreamDone)));
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Chunk(c) => Some(c.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            !text.trim().is_empty(),
            "a non-empty canned summary streams"
        );
    }

    #[test]
    fn turn_events_interleaves_at_least_one_tool_call() {
        let events = turn_events("hi", 0);
        let starts = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolStart { .. }))
            .count();
        let ends = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolEnd { .. }))
            .count();
        assert!(starts >= 1, "a turn runs at least one tool");
        assert_eq!(starts, ends, "every ToolStart has a matching ToolEnd");
    }

    #[test]
    fn turn_events_announces_a_parallel_batch_before_its_tools() {
        // The dummy scripts a parallel `Bash` batch: a `ToolBatch` (>= 2 calls)
        // is emitted *before* the first `ToolStart`, so the UI shows the
        // not-yet-run calls as `⎿ Waiting…`. Each batch entry's `(name, args)`
        // matches the `ToolStart` that runs it, in order. See
        // `docs/parallel-tools.md`.
        let events = turn_events("hi", 0);
        let batch_pos = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolBatch(_)))
            .expect("the dummy announces a parallel batch");
        let StreamEvent::ToolBatch(items) = &events[batch_pos] else {
            unreachable!()
        };
        assert!(items.len() >= 2, "the batch has several parallel calls");
        let first_start = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
            .expect("the batch runs its tools");
        assert!(
            batch_pos < first_start,
            "the batch is announced before any call starts"
        );
        // The batch entries equal the name/args of the ToolStarts that follow.
        let following_starts: Vec<ToolCallSummary> = events[batch_pos + 1..]
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ToolStart { name, args, .. } => Some(ToolCallSummary {
                    name: name.clone(),
                    args: args.clone(),
                }),
                _ => None,
            })
            .take(items.len())
            .collect();
        assert_eq!(
            *items, following_starts,
            "each announced call matches its ToolStart"
        );
    }

    #[test]
    fn a_parallel_prompt_triggers_the_vivid_three_call_bash_batch() {
        // A prompt mentioning "parallel" opts into the vivid demo: a single
        // ToolBatch of three `Bash(ping …)` calls announced up front (so the
        // not-yet-run ones show `⎿ Waiting…`), then run in order. The default turn
        // keeps its compact two-call batch. See `docs/parallel-tools.md`.
        let events = turn_events("run three pings in parallel", 0);
        let StreamEvent::ToolBatch(items) = events
            .iter()
            .find(|e| matches!(e, StreamEvent::ToolBatch(_)))
            .expect("a parallel prompt announces a batch")
        else {
            unreachable!()
        };
        assert_eq!(items.len(), 3, "three parallel calls: {items:?}");
        assert!(
            items
                .iter()
                .all(|s| s.name == "Bash" && s.args.contains("ping")),
            "every call is a Bash ping: {items:?}"
        );
        let ends = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolEnd { .. }))
            .count();
        assert_eq!(ends, 3, "all three calls run");
    }

    #[test]
    fn the_default_turn_keeps_the_compact_two_call_batch() {
        // Without "parallel", the turn runs the compact Read+Bash batch (baseline
        // footprint), not the three-ping demo — so unrelated smoke phases keep
        // their sizing.
        let events = turn_events("hello there", 0);
        let StreamEvent::ToolBatch(items) = events
            .iter()
            .find(|e| matches!(e, StreamEvent::ToolBatch(_)))
            .expect("the default turn still announces a batch")
        else {
            unreachable!()
        };
        assert_eq!(items.len(), 2, "two calls by default: {items:?}");
        assert_eq!(items[0].name, "Read", "the Read runs first: {items:?}");
    }

    #[test]
    fn turn_events_includes_one_paired_thinking_phase_before_the_tools() {
        let events = turn_events("hi", 0);
        let starts = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ThinkingStart))
            .count();
        let ends = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ThinkingEnd))
            .count();
        assert_eq!(starts, 1, "the turn thinks exactly once");
        assert_eq!(ends, 1, "every ThinkingStart has a matching ThinkingEnd");

        let start = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ThinkingStart))
            .unwrap();
        let end = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ThinkingEnd))
            .unwrap();
        let first_tool = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
            .unwrap();
        assert!(start < end, "thinking starts before it ends");
        assert!(
            end < first_tool,
            "thinking resolves before the first tool, so tool start/end stay adjacent"
        );
    }

    #[test]
    fn turn_events_streams_thinking_chunks_inside_the_thinking_phase() {
        // The reasoning text travels as ThinkingChunk events strictly between
        // the ThinkingStart/ThinkingEnd pair — opaque to the renderer (never
        // displayed) but counted into the live token tally, like a real API's
        // reasoning deltas.
        let events = turn_events("hi", 0);
        let start = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ThinkingStart))
            .unwrap();
        let end = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ThinkingEnd))
            .unwrap();
        let chunk_positions: Vec<usize> = events
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, StreamEvent::ThinkingChunk(_)))
            .map(|(i, _)| i)
            .collect();
        assert!(
            !chunk_positions.is_empty(),
            "the dummy streams reasoning text while it thinks"
        );
        assert!(
            chunk_positions.iter().all(|&i| start < i && i < end),
            "every ThinkingChunk sits inside the Start/End pair"
        );
    }

    #[test]
    fn turn_events_resolve_each_tool_before_the_next_starts() {
        // Tools don't nest: after a ToolStart, only its live ToolOutput chunks
        // may appear before the matching ToolEnd — never a second ToolStart — so
        // the loop only ever tracks one running tool at a time. See
        // `docs/tool-streaming.md`.
        let events = turn_events("anything", 0);
        let mut running = false;
        for event in &events {
            match event {
                StreamEvent::ToolStart { .. } => {
                    assert!(!running, "a tool starts only after the previous one ended");
                    running = true;
                }
                StreamEvent::ToolEnd { .. } => {
                    assert!(running, "a ToolEnd closes a running tool");
                    running = false;
                }
                StreamEvent::ToolOutput(_) => {
                    assert!(running, "live output only streams while a tool runs");
                }
                _ => {}
            }
        }
        assert!(!running, "every tool that started also ended");
    }

    #[test]
    fn turn_events_streams_each_bash_output_before_its_end() {
        // Each Bash call streams its output as ToolOutput chunks between its
        // ToolStart and ToolEnd; concatenated, they equal the ToolEnd output
        // (the authoritative full cell). See `docs/tool-streaming.md`.
        let events = turn_events("run three pings in parallel", 0);
        let mut streamed = String::new();
        let mut resolved = 0;
        for event in &events {
            match event {
                StreamEvent::ToolOutput(chunk) => streamed.push_str(chunk),
                StreamEvent::ToolEnd { output, .. } => {
                    // Each Bash cell's streamed output equals its final output.
                    assert_eq!(&streamed, output, "the tail reconstructs the final cell");
                    streamed.clear();
                    resolved += 1;
                }
                _ => {}
            }
        }
        assert_eq!(resolved, 3, "all three Bash cells streamed then resolved");
    }

    #[test]
    fn turn_events_generates_each_tool_call_before_it_starts() {
        // Each ToolStart is preceded by ToolCallDelta fragments (the model
        // "generating" the call) and never sits between a Start and its End, so
        // the tally ticks during generation and tools still resolve one at a time.
        let events = turn_events("anything", 0);
        let deltas = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolCallDelta(_)))
            .count();
        assert!(
            deltas >= 1,
            "the dummy streams tool-call generation fragments"
        );
        let mut running = false;
        for event in &events {
            match event {
                StreamEvent::ToolStart { .. } => running = true,
                StreamEvent::ToolEnd { .. } => running = false,
                StreamEvent::ToolCallDelta(_) => assert!(
                    !running,
                    "a generation fragment never streams while a tool is running"
                ),
                _ => {}
            }
        }
    }

    #[test]
    fn turn_events_shows_both_a_success_and_a_failure() {
        // The demo exercises green and red: at least one ok tool and one failing.
        let events = turn_events("x", 0);
        let oks = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolEnd { ok: true, .. }))
            .count();
        let fails = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolEnd { ok: false, .. }))
            .count();
        assert!(oks >= 1, "at least one tool succeeds (green)");
        assert!(fails >= 1, "at least one tool fails (red)");
    }

    #[test]
    fn turn_events_ends_with_stream_done() {
        assert_eq!(turn_events("x", 0).last(), Some(&StreamEvent::StreamDone));
    }

    /// Concatenate just the `Chunk` text of a turn (its visible reply).
    fn chunk_text(events: &[StreamEvent]) -> String {
        events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Chunk(c) => Some(c.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn turn_events_without_images_streams_just_the_reply() {
        // image_count 0 is the old behaviour: chunks reconstruct the reply.
        let prompt = "hello";
        assert_eq!(chunk_text(&turn_events(prompt, 0)), dummy_response(prompt));
    }

    #[test]
    fn turn_events_acknowledges_attached_images_up_front() {
        // The dummy has no vision, so it acknowledges the attachments instead —
        // a leading chunk proving the typed image channel reached the backend.
        let prompt = "what is this";
        assert_eq!(
            chunk_text(&turn_events(prompt, 2)),
            format!("Looking at your 2 images. {}", dummy_response(prompt))
        );
    }

    #[test]
    fn image_acknowledgement_is_singular_for_one_image() {
        assert!(chunk_text(&turn_events("x", 1)).starts_with("Looking at your 1 image. "));
    }

    #[test]
    fn every_dummy_response_chunks_round_trip() {
        // Guards against a response that can't be streamed faithfully.
        for prompt in ["", "a", "tell me a story", "what is ratatui?"] {
            let response = dummy_response(prompt);
            assert_eq!(chunks(&response).concat(), response);
        }
    }

    #[test]
    fn cancel_token_starts_uncancelled_and_latches() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancel_token_clone_shares_the_same_flag() {
        let token = CancelToken::new();
        let clone = token.clone();
        token.cancel();
        assert!(clone.is_cancelled(), "a clone observes the cancellation");
    }

    #[test]
    fn dummy_ai_emits_all_chunks_and_tool_calls_then_done() {
        let (tx, mut rx) = unbounded_channel();
        let prompt = "hi".to_string();
        let expected = dummy_response(&prompt);
        // Zero startup delay so this content test stays fast.
        let handle = DummyAi::with_startup_delay(Duration::ZERO).spawn(
            prompt,
            vec![],
            vec![],
            tx,
            CancelToken::new(),
        );

        let mut streamed = String::new();
        let mut saw_done = false;
        let mut tool_batches = 0;
        let mut batched_calls = 0;
        let mut tool_starts = 0;
        let mut tool_ends = 0;
        let mut tool_output_chunks = 0;
        let mut think_starts = 0;
        let mut think_chunks = 0;
        let mut think_ends = 0;
        let mut tool_call_deltas = 0;
        // `blocking_recv` waits for each delayed event (no runtime here, so it's
        // allowed); `None` means the backend dropped its sender.
        while let Some(event) = rx.blocking_recv() {
            match event {
                StreamEvent::Chunk(c) => streamed.push_str(&c),
                StreamEvent::ToolBatch(items) => {
                    tool_batches += 1;
                    batched_calls = items.len();
                }
                StreamEvent::ToolStart { .. } => tool_starts += 1,
                StreamEvent::ToolEnd { .. } => tool_ends += 1,
                StreamEvent::ToolOutput(_) => tool_output_chunks += 1,
                StreamEvent::AgentBatch { .. } | StreamEvent::AgentGroupDone { .. } => {}
                StreamEvent::ThinkingStart => think_starts += 1,
                StreamEvent::ThinkingChunk(_) => think_chunks += 1,
                StreamEvent::ThinkingEnd => think_ends += 1,
                StreamEvent::ToolCallDelta(_) => tool_call_deltas += 1,
                StreamEvent::StreamDone => {
                    saw_done = true;
                    break;
                }
                StreamEvent::Retrying { .. } => panic!("the dummy never retries"),
                StreamEvent::Usage(_) => panic!("the dummy never reports usage"),
                StreamEvent::ToolBackgrounded { .. } => {
                    panic!("the dummy never backgrounds a tool")
                }
                StreamEvent::Error(e) => panic!("dummy never errors, got {e:?}"),
            }
        }
        handle.join().unwrap();

        assert!(saw_done, "stream must end with StreamDone");
        assert_eq!(streamed, expected, "chunks still reconstruct the reply");
        assert!(tool_starts >= 1, "the dummy streams at least one tool call");
        assert_eq!(tool_starts, tool_ends, "every tool that starts also ends");
        assert!(
            tool_output_chunks >= 1,
            "the dummy streams live tool output (docs/tool-streaming.md)"
        );
        assert_eq!(
            tool_batches, 1,
            "the dummy announces its parallel batch once"
        );
        assert!(
            batched_calls >= 2,
            "the announced batch has several parallel calls"
        );
        assert!(
            tool_call_deltas >= 1,
            "the dummy generates each tool call first (ticking the tally)"
        );
        assert_eq!(think_starts, 1, "the dummy thinks once");
        assert!(think_chunks >= 1, "reasoning deltas stream while thinking");
        assert_eq!(
            think_starts, think_ends,
            "every think that starts also ends"
        );
    }

    #[test]
    fn dummy_ai_acknowledges_attached_images_it_was_spawned_with() {
        // The typed image channel reaches the backend: spawning with two paths
        // makes the dummy open its reply with the acknowledgement.
        let (tx, mut rx) = unbounded_channel();
        let images = vec![PathBuf::from("/tmp/a.png"), PathBuf::from("/tmp/b.png")];
        let handle = DummyAi::with_startup_delay(Duration::ZERO).spawn(
            "describe".to_string(),
            images,
            vec![],
            tx,
            CancelToken::new(),
        );
        let mut streamed = String::new();
        while let Some(event) = rx.blocking_recv() {
            match event {
                StreamEvent::Chunk(c) => streamed.push_str(&c),
                StreamEvent::StreamDone => break,
                _ => {}
            }
        }
        handle.join().unwrap();
        assert!(
            streamed.starts_with("Looking at your 2 images. "),
            "the dummy acknowledges the two images up front, got {streamed:?}"
        );
    }

    #[test]
    fn dummy_ai_sends_nothing_when_cancelled_before_it_starts() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        cancel.cancel();
        DummyAi::default()
            .spawn("hello".to_string(), vec![], vec![], tx, cancel)
            .join()
            .unwrap();
        // Cancelled before the first chunk → no Chunk and no StreamDone arrive.
        assert!(
            rx.try_recv().is_err(),
            "a cancelled backend streams nothing"
        );
    }

    #[test]
    fn a_reply_source_can_report_an_error() {
        // Proves the trait + protocol carry backend failures, as a real model
        // would, without any dummy-specific machinery.
        struct Failing;
        impl ReplySource for Failing {
            fn spawn(
                &self,
                _prompt: String,
                _images: Vec<PathBuf>,
                _context: Vec<ContextMessage>,
                tx: UnboundedSender<StreamEvent>,
                _cancel: CancelToken,
            ) -> JoinHandle<()> {
                thread::spawn(move || {
                    let _ = tx.send(StreamEvent::Error("backend exploded".to_string()));
                })
            }

            fn model_name(&self) -> String {
                "failing".to_string()
            }
        }
        let (tx, mut rx) = unbounded_channel();
        Failing
            .spawn("x".to_string(), vec![], vec![], tx, CancelToken::new())
            .join()
            .unwrap();
        assert_eq!(
            rx.blocking_recv().unwrap(),
            StreamEvent::Error("backend exploded".to_string())
        );
    }

    #[test]
    fn an_agents_prompt_scripts_the_two_agent_demo() {
        let events = turn_events("call agents for weather", 0);
        let batch = events.iter().find_map(|e| match e {
            StreamEvent::AgentBatch { background, agents } => Some((background, agents)),
            _ => None,
        });
        let (background, agents) = batch.expect("the demo announces a group");
        assert!(!background, "foreground by default");
        assert_eq!(agents.len(), 2);
        assert!(agents[0].description.contains("Warsaw"));
        assert!(agents[0].id.starts_with('a'));
        let done = events.iter().find_map(|e| match e {
            StreamEvent::AgentGroupDone { agents, .. } => Some(agents),
            _ => None,
        });
        let done = done.expect("the demo resolves the group");
        assert_eq!(done.len(), 2);
        assert!(done.iter().all(|d| d.ok));
        assert_eq!(events.last(), Some(&StreamEvent::StreamDone));
        // A "background" prompt launches in background mode with launch texts.
        let events = turn_events("call background agents", 0);
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::AgentBatch {
                background: true,
                ..
            }
        )));
        // /init's canned prompt names AGENTS.md — never the demo.
        let events = turn_events("Generate a file named AGENTS.md", 0);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StreamEvent::AgentBatch { .. }))
        );
    }
}
