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

// ===== The subagent that ASKS (`docs/permissions.md`) =====

/// The gated demo's task label — the roster row and the session view's rule.
const GATED_DESCRIPTION: &str = "Ask before two commands";

/// Its launch prompt (the first user message of its transcript).
const GATED_PROMPT: &str = "Run two shell commands, asking me before each one.";

/// The two commands the gated subagent requests **in one parallel batch** —
/// `(command, description, output, exit)`. Announced up front, so its session
/// view shows the asked-about call over its `⎿ Waiting…` sibling exactly as
/// the main view shows the main turn's batch (`docs/parallel-tools.md`).
const GATED_COMMANDS: [(&str, &str, &str, u8); 2] = [
    (
        "ls -la",
        "List this directory in long format",
        "total 8\ndrwxr-xr-x  3 you you 4096 Jan  1 09:00 .\n-rw-r--r--  1 you you   42 Jan  1 09:00 notes.md",
        0,
    ),
    (
        "echo hello from the subagent",
        "Print a greeting",
        "hello from the subagent",
        0,
    ),
];

/// How long the gated agent waits before its first event. Longer than
/// [`DEMO_PRE_ROLL`]: the *point* of this demo is the prompt raised from
/// inside the agent's session view, and the prompt owns every key once it is
/// open — so the walk to that view (↓, ↓, Enter) has to finish first, or the
/// first keystroke answers the question instead of opening the screen it is
/// about.
const GATED_PRE_ROLL: Duration = Duration::from_secs(6);

/// What the lead narrates while the gated agent works. Closes on the shared
/// hand-off sentence like every other user-facing demo reply
/// (`docs/dummy-backend.md`).
const GATED_NARRATION: &str = concat!(
    "Launching a subagent that has to ask. Walk the roster with **↓** and \
     press **Enter** on its row to step into its session — the prompt it \
     raises is about *its* call, so the cells above the question are its own \
     `● Bash(…)` over `⎿ Waiting…`, its parallel sibling behind them.\n\n",
    "That is the whole point: the prompt's context is what the screen you are \
     looking at has on it. The lead's `● Agent(…)` cell belongs to the main \
     screen, and stays there.\n\n",
    handoff!()
);

/// Play the gated subagent demo: narrate, launch a background agent, and
/// stream a round on the agent channel whose **own** parallel `bash` batch
/// raises the shared permission prompt.
///
/// The request travels the *agent* channel, so `tui::agent`'s handler stamps
/// whose it is before raising it — which is what lets the prompt's context
/// cells be that agent's own queue instead of the lead's tree
/// (`docs/agent-view-streaming.md`).
pub(in crate::stream) fn agent_permission_turn(stage: &AgentStage<'_>) {
    let (first, second) = reply_parts(GATED_NARRATION);
    if !say(&first, stage.tx, stage.cancel) {
        return;
    }
    let (id, agent_cancel) = stage.agents.register(GENERAL_PURPOSE);
    let announced = stage.tx.send(StreamEvent::AgentBatch {
        background: true,
        agents: vec![AgentSpec {
            id: id.clone(),
            description: GATED_DESCRIPTION.to_string(),
            agent_type: GENERAL_PURPOSE.to_string(),
            prompt: GATED_PROMPT.to_string(),
            background: true,
        }],
    });
    if announced.is_err() {
        return;
    }
    spawn_gated_agent_session(
        stage.agents.clone(),
        id.clone(),
        agent_cancel,
        stage.gate.cloned(),
    );
    let resolved = stage.tx.send(StreamEvent::AgentGroupDone {
        background: true,
        agents: vec![AgentCallDone {
            id,
            output: crate::llm::backend::agent_launch_text(GATED_DESCRIPTION),
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

/// One gated call's request, as the real `permission_request` builds a `bash`
/// one: the command as the target, the model's own description as the detail,
/// and the asker's **type** (the title's `· from the general-purpose agent`).
/// The id is the boundary's to stamp — only the agent channel knows it.
fn gated_request(gate: &PermissionGate, command: &str, detail: &str) -> PermissionRequest {
    PermissionRequest {
        id: gate.next_id(),
        kind: PermissionKind::Bash,
        target: command.to_string(),
        body: String::new(),
        detail: Some(detail.to_string()),
        agent: Some(GENERAL_PURPOSE.to_string()),
        agent_id: None,
    }
}

/// How one gated call resolved, offline — the shape
/// [`spawn_gated_agent_session`] acts on.
enum GatedOutcome {
    /// Approved: run it, and let the scripted resolution follow.
    Run,
    /// Refused: this `ToolRejected` replaces the call's `ToolEnd`
    /// (`docs/permissions.md`).
    Refused(StreamEvent),
    /// The turn was cancelled out from under us — the channel is already
    /// abandoned, so the caller simply stops.
    Cancelled,
}

/// Map the user's answer onto that outcome, remembering an "allow always" as
/// a session rule on the way through — the real approve seam's `judge`.
fn judge_gated(
    gate: &PermissionGate,
    request: &mut PermissionRequest,
    cancel: &CancelToken,
) -> GatedOutcome {
    let reject = |display: String, result: String| {
        GatedOutcome::Refused(StreamEvent::ToolRejected {
            display,
            result,
            truncated: false,
        })
    };
    match gate.wait(&request.id, &|| cancel.is_cancelled()) {
        Some(PermissionDecision::Approve) => GatedOutcome::Run,
        Some(PermissionDecision::ApproveAlways) => {
            request.id.clear();
            gate.remember(request);
            GatedOutcome::Run
        }
        Some(PermissionDecision::Deny(feedback)) => reject(
            crate::permission::denied_display(request, feedback.as_deref()),
            crate::permission::denial_result(feedback.as_deref()),
        ),
        Some(PermissionDecision::Explain) => reject(
            crate::permission::explain_display(),
            crate::permission::explain_result(request),
        ),
        None => GatedOutcome::Cancelled,
    }
}

/// Stream the gated agent's round on the agent channel: a pre-roll pause so
/// the user can open its session view, the two-call `bash` batch announced up
/// front, then per call — ask, **block on the gate**, resolve — exactly as
/// `llm::agent::run_agent`'s loop does for a real subagent.
///
/// With no gate attached (`ALTER_ZERO_PERMISSIONS=0`) the calls simply run,
/// which is what that setting means.
fn spawn_gated_agent_session(
    registry: AgentRegistry,
    id: String,
    cancel: CancelToken,
    gate: Option<PermissionGate>,
) {
    thread::spawn(move || {
        nap(GATED_PRE_ROLL, &cancel);
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
        let calls: Vec<ScriptedCall> = GATED_COMMANDS
            .iter()
            .map(|&(command, _, output, exit)| ScriptedCall::command(command, output, exit))
            .collect();
        if !send(
            StreamEvent::ToolBatch(calls.iter().map(ScriptedCall::summary).collect()),
            TOOL_DELAY,
        ) {
            return;
        }
        let mut refused = false;
        for (call, &(command, detail, _, _)) in calls.iter().zip(GATED_COMMANDS.iter()) {
            // The approve seam, offline: a standing rule runs the call
            // unasked, otherwise raise the request on the **agent** channel
            // (so the boundary stamps whose it is) and block on the gate
            // exactly as a real backend's tool thread does. `None` means the
            // turn was cancelled out from under us.
            let resolved = match &gate {
                None => GatedOutcome::Run,
                Some(gate) => {
                    let mut request = gated_request(gate, command, detail);
                    if gate.allows(&request) {
                        GatedOutcome::Run
                    } else {
                        registry.send(AgentEvent::Stream {
                            id: id.clone(),
                            event: StreamEvent::Permission(request.clone()),
                        });
                        judge_gated(gate, &mut request, &cancel)
                    }
                }
            };
            if matches!(resolved, GatedOutcome::Cancelled) {
                return;
            }
            // The cell goes live first either way — the real loop emits
            // `ToolStart` and then sends `ToolRejected` *in place of* the
            // `ToolEnd`, which is what carries the call's verbatim arguments
            // onto the recorded cell (`docs/permissions.md`).
            if !send(call.start(), TOOL_DELAY) {
                return;
            }
            let event = match resolved {
                GatedOutcome::Run => call.end(),
                GatedOutcome::Refused(rejection) => {
                    refused = true;
                    rejection
                }
                GatedOutcome::Cancelled => return,
            };
            if !send(event, CHUNK_DELAY) {
                return;
            }
        }
        let closing = if refused {
            "Understood — I left the refused command alone."
        } else {
            "Both commands ran; their cells are above."
        };
        for piece in chunks(closing) {
            if !send(StreamEvent::Chunk(piece), CHUNK_DELAY) {
                return;
            }
        }
        send(StreamEvent::StreamDone, Duration::ZERO);
        registry.finish(&id, Ok(closing.to_string()), Vec::new());
    });
}
