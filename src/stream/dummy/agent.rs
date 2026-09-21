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
//! could drive the strip that view paints. That is the gap two reported bugs
//! lived in, and the round it plays drives both (`scripts/smoke.sh` Phase 95,
//! `docs/agent-view-streaming.md`):
//!
//! - **The forming table.** `StreamRender` withholds a table's block whole
//!   until it closes, so a broken strip renders its closing border and
//!   nothing else where a working one renders the grid.
//! - **The parallel `write` batch.** The file tools resolve through the
//!   two-text split (`StreamEvent::ToolAnswered`), which the view's commit
//!   arm did not list — the cells reached the agent's transcript but never
//!   scrollback, so they appeared only when a resize rebuilt the view.

use std::thread;
use std::time::Duration;

use crate::agents::{AgentEvent, AgentRegistry, GENERAL_PURPOSE};
use crate::permission::{PermissionDecision, PermissionGate, PermissionKind, PermissionRequest};

use super::super::{AgentCallDone, AgentSpec, CancelToken, StreamEvent};
use super::scenario::AgentStage;
use super::script::{chunks, handoff, reply_parts};
use super::turns::ScriptedCall;
use super::{CHUNK_DELAY, THINK_CHUNK_DELAY, TOOL_DELAY, nap};

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

/// The two files the subagent writes **in one parallel batch** — the case the
/// reported bug lived in: a `write` resolves through the two-text split
/// (`StreamEvent::ToolAnswered`, `docs/tools.md`), which the session view's
/// commit arm did not list, so its cells reached the transcript but never
/// scrollback until a resize rebuilt the view from history
/// (`docs/agent-view-streaming.md`).
const DEMO_FILES: [(&str, &str); 2] = [
    (
        "notes/languages.md",
        "# Languages\n\nFour rows, one grid — see the table below.\n",
    ),
    (
        "notes/sources.md",
        "# Sources\n\nRelease years from each language's own documentation.\n",
    ),
];

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
            call_id: None,
            arguments: None,
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
        // A **parallel batch** of two `write` calls: announced up front, so
        // the not-yet-run one shows `⎿ Waiting…` in the session view's strip,
        // then executed in order. Each resolves with `ToolAnswered` — the
        // file tools' two-text split — which is exactly what the view failed
        // to commit (`docs/agent-view-streaming.md`).
        let calls: Vec<ScriptedCall> = DEMO_FILES
            .iter()
            .map(|&(path, content)| ScriptedCall::write(path, content))
            .collect();
        if !send(
            StreamEvent::ToolBatch(calls.iter().map(ScriptedCall::summary).collect()),
            TOOL_DELAY,
        ) {
            return;
        }
        for call in &calls {
            if !send(call.start(), TOOL_DELAY) || !send(call.end(), CHUNK_DELAY) {
                return;
            }
        }
        // The batch resolved: a **round boundary**, where a real subagent's
        // loop takes what the user typed into its session while it worked and
        // folds it into the next request (`docs/queue.md`). Announcing it here
        // is what makes the agent view's own mid-turn queue drivable offline —
        // the row above the box becomes a user bubble on this transcript.
        for text in registry.take_pending_inputs(&id) {
            if !send(StreamEvent::Steered { text }, CHUNK_DELAY) {
                return;
            }
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

/// What the demo agent answers a **chat continuation** with — a message the
/// user sent into its session after it had already settled. Short on purpose:
/// the point of the offline round trip is the mechanics (the bubble, the
/// stream, the `Done for Ns` receipt on the agent's own transcript), not a
/// second demo reply.
const DEMO_CONTINUATION: &str = "\
Noted — a real subagent would rebuild the table with that row. This one is \
scripted, so it can only show you the shape: your message reached its \
conversation, and this reply is its next turn.";

/// Answer a chat message sent into the demo agent **after it settled**: play a
/// short continuation round on the agent channel, from a thread of its own,
/// exactly as [`spawn_agent_session`] plays the first one. The offline half of
/// `LlmBackend::spawn_agent_chat`'s continuation path, so the agent session
/// view's chat is drivable with no network (`docs/queue.md`).
pub(super) fn spawn_chat_continuation(registry: AgentRegistry, id: String, cancel: CancelToken) {
    thread::spawn(move || {
        nap(CHUNK_DELAY, &cancel);
        for piece in chunks(DEMO_CONTINUATION) {
            if cancel.is_cancelled() {
                return;
            }
            registry.send(AgentEvent::Stream {
                id: id.clone(),
                event: StreamEvent::Chunk(piece),
            });
            nap(CHUNK_DELAY, &cancel);
        }
        if cancel.is_cancelled() {
            return;
        }
        registry.send(AgentEvent::Stream {
            id: id.clone(),
            event: StreamEvent::StreamDone,
        });
        registry.finish(&id, Ok(DEMO_CONTINUATION.to_string()), Vec::new());
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

// --- the subagent that ASKS (docs/permissions.md) ---------------------------
//
// The reported bug, drivable offline: standing inside a subagent's session
// view in manual mode, its `bash` request opened over the *lead's*
// `● Agent(…)` / `⎿ Working…` cell — the main strip's lone-agent tree, which
// that screen does not show — instead of the agent's own `● Bash(ls -la)` /
// `⎿ Waiting…` and its batch siblings.
//
// Three details make it reproduce, and all three are the real backend's shape:
// the launch is **foreground** (a background group resolves at once, so no
// live lead cell survives to cover anything), the calls are announced as a
// **parallel batch** before any runs, and the request is raised **before** the
// `ToolStart` — the approve seam's own order, which is why the asked-about
// call genuinely reads `⎿ Waiting…`.

/// The gated demo's task label — the roster row, the session view's rule, and
/// the lead's live `● Agent({description})` cell.
const GATED_DESCRIPTION: &str = "Run ls -la via subagent";

/// The prompt it is launched with (the first user message of its transcript).
const GATED_PROMPT: &str = "Run `ls -la` and `pwd`, then report what you saw.";

/// Its parallel batch: two commands, announced together, asked one at a time.
const GATED_COMMANDS: [(&str, &str); 2] = [
    (
        "ls -la",
        "total 12\ndrwxr-xr-x  3 user user 4096 Jan  1 00:00 .\n\
         -rw-r--r--  1 user user   42 Jan  1 00:00 notes.md",
    ),
    ("pwd", "/home/user/repo"),
];

/// How often the launcher polls the registry while the foreground group runs
/// — `llm::backend`'s own wait cadence.
const GATED_WAIT_POLL: Duration = Duration::from_millis(30);

/// How long this agent waits before its first request. Longer than
/// [`DEMO_PRE_ROLL`] on purpose: the user has to reach the session view
/// *before* the first prompt opens, and a prompt is modal — an early one
/// swallows the ↓ ↓ Enter walk and answers itself on the Enter.
const GATED_PRE_ROLL: Duration = Duration::from_millis(3500);

/// What the agent answers once its batch resolves.
const GATED_REPLY: &str = "One directory, one file, and the working directory \
     is `/home/user/repo`. Both commands went through the permission prompt \
     first — inside this session view it asks about **my** call, not the cell \
     that launched me.";

/// The main turn's narration, in the two-part shape every demo reply uses.
const GATED_NARRATION: &str = concat!(
    "Launching one **foreground** subagent that runs a parallel `bash` batch. \
     In manual mode each of its commands asks first. Press **↓** to walk the \
     roster below, then **Enter** on its row to open its session — the prompt \
     that opens there is about *its* call.\n\n",
    "That is the point of the context cells above a prompt: they are the \
     conversation on screen. In the main view you see the lead's `Agent(…)` \
     cell; inside the agent's own session you see its `Bash(…)` cell over each \
     waiting sibling of the batch.\n\n",
    handoff!()
);

/// Play the gated demo: narrate, launch **one foreground agent**, wait for it
/// the way `llm::backend::run_agent_calls` does (polling the registry, killing
/// on an Esc), then resolve the group and close.
pub(in crate::stream) fn agent_permission_turn(stage: &AgentStage<'_>) {
    let (first, second) = reply_parts(GATED_NARRATION);
    if !say(&first, stage.tx, stage.cancel) {
        return;
    }
    let (id, agent_cancel) = stage.agents.register(GENERAL_PURPOSE);
    let announced = stage.tx.send(StreamEvent::AgentBatch {
        background: false,
        agents: vec![AgentSpec {
            id: id.clone(),
            description: GATED_DESCRIPTION.to_string(),
            agent_type: GENERAL_PURPOSE.to_string(),
            prompt: GATED_PROMPT.to_string(),
            background: false,
            call_id: None,
            arguments: None,
        }],
    });
    if announced.is_err() {
        return;
    }
    spawn_gated_agent_session(
        stage.agents.clone(),
        stage.gate.cloned(),
        id.clone(),
        agent_cancel,
    );
    // The foreground wait loop, one level down: poll until the agent settles,
    // and an Esc on the *launching* turn kills it (`docs/agent-tool.md`).
    while !stage.agents.is_done(&id) {
        if stage.cancel.is_cancelled() {
            let _ = stage.agents.kill(&id);
            return;
        }
        nap(GATED_WAIT_POLL, stage.cancel);
    }
    let (output, ok) = match stage.agents.outcome(&id) {
        Some(Ok(text)) => (text, true),
        Some(Err(error)) => (format!("[agent failed: {error}]"), false),
        None => (crate::app::AGENT_STOPPED_OUTPUT.to_string(), false),
    };
    let resolved = stage.tx.send(StreamEvent::AgentGroupDone {
        background: false,
        agents: vec![AgentCallDone { id, output, ok }],
    });
    if resolved.is_err() {
        return;
    }
    if say(&second, stage.tx, stage.cancel) {
        let _ = stage.tx.send(StreamEvent::StreamDone);
    }
}

/// Stream the gated agent's round on the agent channel: the pre-roll, the
/// announced batch, then each command asked at the shared gate before it runs.
///
/// The `Permission` event rides the **agent** channel like everything else a
/// subagent reports — `tui::agent::Session::on_agent_event` lifts it out and
/// raises the one shared prompt, exactly as the live forwarder's does.
fn spawn_gated_agent_session(
    registry: AgentRegistry,
    gate: Option<PermissionGate>,
    id: String,
    cancel: CancelToken,
) {
    thread::spawn(move || {
        let outcome = gated_agent_round(&registry, gate.as_ref(), &id, &cancel);
        // **Always** close the slot, on every path out — the real
        // `spawn_subagent_run` does, and a *foreground* launcher polls
        // `is_done` to resolve its group. An early return that skipped this
        // would leave the launching turn spinning for an agent that had
        // already stopped. (A killed slot keeps its own outcome.)
        registry.finish(&id, outcome, Vec::new());
    });
}

/// The gated agent's round, returning what its slot settles with — `Ok(reply)`
/// for a completed run, `Err` for one the user stopped, the real backend's
/// two outcomes.
fn gated_agent_round(
    registry: &AgentRegistry,
    gate: Option<&PermissionGate>,
    id: &str,
    cancel: &CancelToken,
) -> Result<String, String> {
    const STOPPED: &str = "stopped by the user";
    nap(GATED_PRE_ROLL, cancel);
    let send = |event: StreamEvent, pause: Duration| -> bool {
        if cancel.is_cancelled() {
            return false;
        }
        registry.send(AgentEvent::Stream {
            id: id.to_string(),
            event,
        });
        nap(pause, cancel);
        !cancel.is_cancelled()
    };
    let calls: Vec<ScriptedCall> = GATED_COMMANDS
        .iter()
        .map(|&(command, output)| ScriptedCall::command(command, output, 0))
        .collect();
    if !send(
        StreamEvent::ToolBatch(calls.iter().map(ScriptedCall::summary).collect()),
        TOOL_DELAY,
    ) {
        return Err(STOPPED.to_string());
    }
    for (call, (command, _)) in calls.iter().zip(GATED_COMMANDS) {
        // The approve seam: ask BEFORE the `ToolStart`, so the cell the
        // prompt is about is still `⎿ Waiting…` while it asks.
        let refusal = match gate {
            Some(gate) => match ask_at_gate(gate, registry, id, command, cancel) {
                Gated::Cancelled => return Err(STOPPED.to_string()),
                Gated::Allowed => None,
                Gated::Refused(texts) => Some(texts),
            },
            None => None,
        };
        if !send(call.start(), TOOL_DELAY) {
            return Err(STOPPED.to_string());
        }
        let resolution = match refusal {
            Some((display, result)) => StreamEvent::ToolRejected {
                display,
                result,
                truncated: false,
            },
            None => call.end(),
        };
        if !send(resolution, CHUNK_DELAY) {
            return Err(STOPPED.to_string());
        }
    }
    for piece in chunks(GATED_REPLY) {
        if !send(StreamEvent::Chunk(piece), CHUNK_DELAY) {
            return Err(STOPPED.to_string());
        }
    }
    send(StreamEvent::StreamDone, Duration::ZERO);
    Ok(GATED_REPLY.to_string())
}

/// How one gated command came back from the shared prompt.
enum Gated {
    /// Run it (approved, or already covered by a standing rule).
    Allowed,
    /// Don't: the cell text and the model-facing instruction.
    Refused((String, String)),
    /// The turn was cancelled out from under us — stop.
    Cancelled,
}

/// Raise `command`'s request on the **agent** channel and block on the shared
/// gate until the user answers — [`gated::Stage::ask`]'s shape, over the
/// subagent's own channel and carrying its `agent` attribution so the prompt's
/// title says `· from the general-purpose agent`.
fn ask_at_gate(
    gate: &PermissionGate,
    registry: &AgentRegistry,
    id: &str,
    command: &str,
    cancel: &CancelToken,
) -> Gated {
    let mut request = PermissionRequest {
        id: gate.next_id(),
        kind: PermissionKind::Bash,
        target: command.to_string(),
        body: String::new(),
        detail: None,
        agent: Some(GENERAL_PURPOSE.to_string()),
        // The type the title names; **which** run asked is the boundary's to
        // stamp, since only the agent channel knows the id
        // (`tui::agent::Session::on_agent_event`).
        agent_id: None,
    };
    if gate.allows(&request) {
        return Gated::Allowed;
    }
    registry.send(AgentEvent::Stream {
        id: id.to_string(),
        event: StreamEvent::Permission(request.clone()),
    });
    match gate.wait(&request.id, &|| cancel.is_cancelled()) {
        Some(PermissionDecision::Approve) => Gated::Allowed,
        Some(PermissionDecision::ApproveAlways) => {
            request.id.clear();
            gate.remember(&request);
            Gated::Allowed
        }
        Some(PermissionDecision::Deny(feedback)) => Gated::Refused((
            crate::permission::denied_display(&request, feedback.as_deref()),
            crate::permission::denial_result(feedback.as_deref()),
        )),
        Some(PermissionDecision::Explain) => Gated::Refused((
            crate::permission::explain_display(),
            crate::permission::explain_result(&request),
        )),
        None => Gated::Cancelled,
    }
}
