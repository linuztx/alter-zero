//! The offline demo whose **subagent streams its own session**
//! (`docs/agent-view-streaming.md`).
//!
//! Every other scripted turn produces one `Vec<StreamEvent>` for the *reply*
//! channel ([`super::turns`]). This one also writes to the **agent** channel:
//! it launches a background subagent and then plays that agent's own round —
//! a thinking phase, then a markdown table streamed row by row — on the
//! [`AgentRegistry`], from a thread of its own, exactly as
//! `llm::agent::run_agent` does.
//!
//! It exists because the agent **session view** had no offline coverage at
//! all: the dummy announced groups but never streamed a member, so nothing
//! could drive the strip that view paints. That is the gap the reported
//! "streaming disappears inside the subagent TUI" bug lived in — the view's
//! preview showed one row of a block its own commits had withheld whole. The
//! table is the case that shows it: `StreamRender` withholds a forming table
//! entirely, so a broken strip renders its closing border and nothing else,
//! and a working one renders the grid (`scripts/smoke.sh` Phase 95).

use std::thread;
use std::time::Duration;

use crate::agents::{AgentEvent, AgentRegistry, GENERAL_PURPOSE};

use super::super::{AgentCallDone, AgentSpec, CancelToken, StreamEvent};
use super::scenario::AgentStage;
use super::script::{chunks, handoff, reply_parts};
use super::{CHUNK_DELAY, THINK_CHUNK_DELAY, nap};

/// The subagent's task label — the roster row and the session view's rule.
const DEMO_DESCRIPTION: &str = "Stream a comparison table";

/// The prompt it is launched with (the first user message of its transcript).
const DEMO_PROMPT: &str = "Compare four languages as a markdown table.";

/// How long the launched agent waits before its first event, so the user has
/// time to walk the roster (↓) and open its session view (Enter) *before* the
/// stream starts. A real subagent's first token costs about this much anyway.
const DEMO_PRE_ROLL: Duration = Duration::from_millis(1200);

/// What the agent "thinks" before answering — one `⎿` row per line in the
/// live `● Thinking…` block.
const DEMO_THOUGHT: &str = "The user wants a comparison table.\n\
     Four rows, and the grid is the case worth showing: its block is withheld \
     whole until it closes, so every row of it lives in the strip until then.";

/// The agent's reply — a GFM table, which the incremental renderer withholds
/// **whole** until its closing row (`docs/table-streaming.md`), plus a line
/// after it so the block actually closes on screen.
const DEMO_REPLY: &str = "\
| Language | Year | Paradigm | Typing |\n\
|----------|------|----------|--------|\n\
| Python | 1991 | Multi-paradigm | Dynamic |\n\
| Rust | 2010 | Multi-paradigm | Static |\n\
| Go | 2009 | Procedural | Static |\n\
| Ruby | 1995 | Object-oriented | Dynamic |\n\
\n\
Four rows, one grid.";

/// The main turn's narration, in the two-part shape every demo reply uses —
/// the text before the launch, a blank line, the text after it.
const DEMO_NARRATION: &str = concat!(
    "Launching one background subagent that streams a table of its own. Press \
     **↓** to walk the roster below, then **Enter** on its row to open its \
     session — you'll watch the grid form there exactly as it does here.\n\n",
    "It is running now. Inside its session view the strip shows the whole \
     forming table, not just the row the block happens to end on: an agent's \
     commits withhold a table until it closes, so every row of it belongs in \
     the strip until then.\n\n",
    handoff!()
);

/// Play the demo: narrate, launch the agent, stream *its* round on the agent
/// channel from its own thread, resolve the launch, close.
///
/// The agent is registered for real ([`AgentRegistry::register`]), so its id
/// and cancel token are the ones the roster's `x` and the session view act on
/// — the same contract the live backend's launches have.
pub(in crate::stream) fn agent_stream_turn(stage: &AgentStage<'_>) {
    let (first, second) = reply_parts(DEMO_NARRATION);
    if !say(&first, stage.tx, stage.cancel) {
        return;
    }
    let (id, agent_cancel) = stage.agents.register(GENERAL_PURPOSE);
    let announced = stage.tx.send(StreamEvent::AgentBatch {
        background: true,
        agents: vec![AgentSpec {
            id: id.clone(),
            description: DEMO_DESCRIPTION.to_string(),
            agent_type: GENERAL_PURPOSE.to_string(),
            prompt: DEMO_PROMPT.to_string(),
            background: true,
        }],
    });
    if announced.is_err() {
        return;
    }
    spawn_agent_session(stage.agents.clone(), id.clone(), agent_cancel);
    let resolved = stage.tx.send(StreamEvent::AgentGroupDone {
        background: true,
        agents: vec![AgentCallDone {
            id,
            // The real executor's own acknowledgement (the dummy-backend
            // rule: an offline cell carries live output).
            output: crate::llm::backend::agent_launch_text(DEMO_DESCRIPTION),
            ok: true,
        }],
    });
    if resolved.is_err() {
        return;
    }
    if say(&second, stage.tx, stage.cancel) {
        let _ = stage.tx.send(StreamEvent::StreamDone);
    }
}

/// Stream the launched agent's own round on the agent channel: a pre-roll
/// pause, a thinking phase, the table word by word, then the settle.
///
/// Its own thread and its own cancel token, because that is what a subagent
/// is — it outlives the turn that launched it, and the roster's `x` stops it
/// through the token the registry handed out.
fn spawn_agent_session(registry: AgentRegistry, id: String, cancel: CancelToken) {
    thread::spawn(move || {
        nap(DEMO_PRE_ROLL, &cancel);
        let send = |event: StreamEvent, pause: Duration| -> bool {
            if cancel.is_cancelled() {
                return false;
            }
            registry.send(AgentEvent::Stream {
                id: id.clone(),
                event,
            });
            nap(pause, &cancel);
            !cancel.is_cancelled()
        };
        if !send(StreamEvent::ThinkingStart, THINK_CHUNK_DELAY) {
            return;
        }
        for piece in chunks(DEMO_THOUGHT) {
            if !send(StreamEvent::ThinkingChunk(piece), THINK_CHUNK_DELAY) {
                return;
            }
        }
        if !send(StreamEvent::ThinkingEnd, THINK_CHUNK_DELAY) {
            return;
        }
        for piece in chunks(DEMO_REPLY) {
            if !send(StreamEvent::Chunk(piece), CHUNK_DELAY) {
                return;
            }
        }
        send(StreamEvent::StreamDone, Duration::ZERO);
        // Close the slot so the roster's stop/sweep and any continuation see a
        // finished run, exactly as the live backend's wait loop leaves it.
        registry.finish(&id, Ok(DEMO_REPLY.to_string()), Vec::new());
    });
}

/// Stream `text` word by word onto the reply channel, pausing like the
/// scripted replay does. `false` once the receiver is gone or the turn was
/// cancelled — the caller stops quietly.
fn say(
    text: &str,
    tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
) -> bool {
    for piece in chunks(text) {
        if cancel.is_cancelled() || tx.send(StreamEvent::Chunk(piece)).is_err() {
            return false;
        }
        nap(CHUNK_DELAY, cancel);
    }
    !cancel.is_cancelled()
}
