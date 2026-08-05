//! The dummy's **scripted** turns: one pure `Cue -> Vec<StreamEvent>` per
//! scenario.
//!
//! Everything here is deterministic and side-effect free, so a turn's whole
//! event order is unit-testable; [`super::DummyAi`] just plays a script back on
//! a thread with delays. The turns that need the *user* — the permission demos
//! — live in [`super::gated`] instead, because they block on the gate.
//!
//! Which one runs is [`super::scenario`]'s call, not a chain of `if`s here.

use std::time::Duration;

use super::super::{AgentCallDone, AgentSpec, StreamEvent, ToolCallSummary};
use super::scenario::Cue;
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

/// The dummy's `Bash` command + output for the default batch's second call.
/// It resolves **red** (an unresolvable host), so the default turn shows both
/// a green (`Read`) and a red outcome.
const DUMMY_BASH_CMD: &str = "ping -c 3 x.invalid";
const DUMMY_BASH_OUTPUT: &str = "ping: cannot resolve x.invalid: Unknown host\nexit status 68";

/// One scripted tool call: the two strings its `● name(args)` header shows,
/// the output it resolves with, whether that is a success (green) or a failure
/// (red), and whether the output **streams** as live `ToolOutput` deltas
/// first — a `bash` command tails its output as it is produced, while a `read`
/// returns all at once, exactly as the real executor does
/// (`docs/tool-streaming.md`).
struct ScriptedCall {
    name: &'static str,
    args: &'static str,
    output: &'static str,
    ok: bool,
    streams: bool,
}

/// The **default** turn's batch: a `Read` then a `Bash`, announced up front so
/// the `Bash` shows `⎿ Waiting…` while the `Read` runs (the visible batch,
/// offline; see `docs/parallel-tools.md`). Kept to two calls with the same
/// output footprint as the pre-batch demo so the committed scrollback is
/// unchanged — a real backend renders however many parallel calls the model
/// actually requests, and the display scales to N.
const DEFAULT_BATCH: &[ScriptedCall] = &[
    ScriptedCall {
        name: "Read",
        args: "src/main.rs",
        output: DUMMY_READ_OUTPUT,
        ok: true,
        streams: false,
    },
    ScriptedCall {
        name: "Bash",
        args: DUMMY_BASH_CMD,
        output: DUMMY_BASH_OUTPUT,
        ok: false,
        streams: true,
    },
];

/// The vivid **three-call parallel `Bash(ping …)` batch** — the user's example,
/// opt-in via a prompt mentioning "parallel" so the default turn keeps its
/// compact footprint and every unrelated smoke phase keeps its sizing. All
/// three are announced up front, so while the first runs the other two show
/// `⎿ Waiting…`; two resolve green, one red. See `docs/parallel-tools.md`.
const PARALLEL_BATCH: &[ScriptedCall] = &[
    ScriptedCall {
        name: "Bash",
        args: "ping -c 20 google.com",
        output: DUMMY_PING_GOOGLE,
        ok: true,
        streams: true,
    },
    ScriptedCall {
        name: "Bash",
        args: "ping -c 20 facebook.com",
        output: DUMMY_PING_FACEBOOK,
        ok: true,
        streams: true,
    },
    ScriptedCall {
        name: "Bash",
        args: "ping -c 20 x.invalid",
        output: DUMMY_PING_FAIL,
        ok: false,
        streams: true,
    },
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

/// The dummy's **subagent demo** (`docs/agent-tool.md`): a two-agent group is
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
/// ([`crate::context::SUMMARIZATION_PROMPT`]) — how the registry recognizes a
/// compact turn's request and scripts a text-only summary for it.
pub(in crate::stream) const COMPACT_PROMPT_MARKER: &str =
    "You are performing a CONTEXT CHECKPOINT COMPACTION";

/// The dummy's canned `/compact` summary — streamed word-by-word like every
/// reply, captured (never rendered) by the compact turn (docs/compact.md).
const DUMMY_COMPACT_SUMMARY: &str = "Progress so far: this is a canned handoff summary from the dummy backend. \
     Key decisions: none - no real model is attached. Next steps: keep \
     chatting; the compacted context now rides this summary.";

/// The events a user-facing reply **opens** with: an acknowledgement of any
/// Ctrl+V images, since the dummy can't actually see them. Visible proof the
/// typed image channel (codex's `UserInput::LocalImage`) carried the paths to
/// the backend; a real vision model would read the files instead. Empty when
/// no images rode along. See `docs/image-paste.md`.
fn opening(cue: &Cue) -> Vec<StreamEvent> {
    image_ack(cue.images())
        .map(|ack| chunks(&ack).into_iter().map(StreamEvent::Chunk).collect())
        .unwrap_or_default()
}

/// The canned reply for this prompt, split in two at the middle word — the
/// text a turn streams *before* its tools/agents and the text it streams
/// after, so the demo shows a reply genuinely interleaved with its work.
fn reply_halves(cue: &Cue) -> (String, String) {
    let reply = dummy_response(cue.text());
    let words: Vec<&str> = reply.split_inclusive(' ').collect();
    let mid = (words.len() / 2).max(1).min(words.len());
    (words[..mid].concat(), words[mid..].concat())
}

/// Stream `text` word-by-word as reply chunks.
fn say(text: &str) -> Vec<StreamEvent> {
    chunks(text).into_iter().map(StreamEvent::Chunk).collect()
}

/// `/compact`'s summarization request plays a **text-only** canned summary —
/// no thinking phase, no tool batch (codex sends the summarize request with no
/// tools) — so the offline dummy path (and `smoke.sh`) can drive the whole
/// compact flow with no provider (docs/compact.md). It is the one scripted
/// turn with no image acknowledgement: the loop builds this request itself and
/// never attaches images to it.
pub(in crate::stream) fn compact_turn(_cue: &Cue) -> Vec<StreamEvent> {
    let mut events = say(DUMMY_COMPACT_SUMMARY);
    events.push(StreamEvent::StreamDone);
    events
}

/// The markdown-table demo as a **text-only** turn: no thinking pause and no
/// tool batch — a tool call would split the reply around it, flushing the
/// block early — so the whole table streams through the strip preview and its
/// block commits at the close (docs/table-streaming.md).
pub(in crate::stream) fn table_turn(cue: &Cue) -> Vec<StreamEvent> {
    let mut events = opening(cue);
    events.extend(say(&dummy_response(cue.text())));
    events.push(StreamEvent::StreamDone);
    events
}

/// The subagent demo (docs/agent-tool.md): the first half of the text, then
/// the scripted two-agent group — announced, "running" for [`AGENT_DELAY`],
/// resolved — then the closing text. A prompt also mentioning "background"
/// launches it in background mode, so the calls resolve at once with launch
/// texts and the roster entries stay running.
pub(in crate::stream) fn agents_turn(cue: &Cue) -> Vec<StreamEvent> {
    let background = cue.mentions("background");
    let (first, second) = reply_halves(cue);
    let mut events = opening(cue);
    events.extend(say(&first));
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
    events.extend(say(&second));
    events.push(StreamEvent::StreamDone);
    events
}

/// The vivid three-call `Bash(ping …)` batch — the user's example.
pub(in crate::stream) fn parallel_turn(cue: &Cue) -> Vec<StreamEvent> {
    tool_turn(cue, &[DUMMY_BASH_CALL], PARALLEL_BATCH)
}

/// The **default** turn, and the fallback for any prompt no other scenario
/// claims: stream the first half of the text, *think* for a moment, run the
/// compact `Read`+`Bash` batch, then stream the rest and finish.
pub(in crate::stream) fn tools_turn(cue: &Cue) -> Vec<StreamEvent> {
    tool_turn(cue, &[DUMMY_READ_CALL, DUMMY_BASH_CALL], DEFAULT_BATCH)
}

/// The shared envelope both tool turns use: half the reply, a thinking phase,
/// the model "generating" the calls (`deltas`, one group per call — their
/// fragments tick the token tally like reasoning), the batch announced up
/// front so the not-yet-run calls show `⎿ Waiting…`, the calls executed in
/// order, then the rest of the reply.
///
/// Every `ToolStart` still lands immediately before its `ToolEnd`, so
/// execution stays sequential — one running call at a time, exactly what the
/// UI's "at most one running tool" invariant expects (`docs/parallel-tools.md`).
fn tool_turn(cue: &Cue, deltas: &[&[&str]], batch: &[ScriptedCall]) -> Vec<StreamEvent> {
    let (first, second) = reply_halves(cue);
    let mut events = opening(cue);
    events.extend(say(&first));
    events.push(StreamEvent::ThinkingStart);
    events.extend(
        chunks(DUMMY_THINKING)
            .into_iter()
            .map(StreamEvent::ThinkingChunk),
    );
    events.push(StreamEvent::ThinkingEnd);
    for call in deltas {
        for frag in *call {
            events.push(StreamEvent::ToolCallDelta((*frag).to_string()));
        }
    }
    events.push(StreamEvent::ToolBatch(
        batch
            .iter()
            .map(|call| ToolCallSummary {
                name: call.name.to_string(),
                args: call.args.to_string(),
            })
            .collect(),
    ));
    for call in batch {
        events.push(StreamEvent::ToolStart {
            name: call.name.to_string(),
            args: call.args.to_string(),
            detail: None,
        });
        // A streaming call's output arrives line-by-line so the live cell
        // tails it; the authoritative ToolEnd then commits the finished cell
        // (docs/tool-streaming.md).
        if call.streams {
            events.extend(tool_output_events(call.output));
        }
        events.push(StreamEvent::ToolEnd {
            output: call.output.to_string(),
            ok: call.ok,
            truncated: false,
        });
    }
    events.extend(say(&second));
    events.push(StreamEvent::StreamDone);
    events
}
