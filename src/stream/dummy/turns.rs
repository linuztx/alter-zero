//! The dummy's **scripted** turns: the pure `prompt → Vec<StreamEvent>` demos.
//!
//! Everything here is deterministic and side-effect free, so a turn's whole
//! event order is unit-testable; [`super::DummyAi`] just plays a script back on
//! a thread with delays. The turns that need the *user* — the permission demos
//! — live in [`super::gated`] instead, because they block on the gate.

use std::time::Duration;

use super::super::{AgentCallDone, AgentSpec, StreamEvent, ToolCallSummary};
use super::script::{chunks, dummy_response, image_ack, tool_output_events};

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

/// The opening of codex's `/compact` summarization prompt
/// ([`crate::context::SUMMARIZATION_PROMPT`]) — how [`turn_events`] recognizes
/// a compact turn's request and scripts a text-only summary for it.
const COMPACT_PROMPT_MARKER: &str = "You are performing a CONTEXT CHECKPOINT COMPACTION";

/// The dummy's canned `/compact` summary — streamed word-by-word like every
/// reply, captured (never rendered) by the compact turn (docs/compact.md).
const DUMMY_COMPACT_SUMMARY: &str = "Progress so far: this is a canned handoff summary from the dummy backend. \
     Key decisions: none - no real model is attached. Next steps: keep \
     chatting; the compacted context now rides this summary.";

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
/// pair the dummy streams `DUMMY_THINKING` word-by-word as
/// [`StreamEvent::ThinkingChunk`]s, so the token tally keeps ticking while the
/// thinking timer runs.
///
/// Pure and deterministic so it is unit-testable; [`super::DummyAi`] just plays
/// it back on a thread with delays. The `Chunk` events still concatenate to
/// exactly [`dummy_response`], so streaming stays faithful.
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
