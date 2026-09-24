//! The built-in offline backend: [`DummyAi`].
//!
//! It is the demo's stand-in for a real model and the only backend
//! `scripts/smoke.sh` can drive deterministically, so every UI feature that a
//! provider would otherwise be needed to exercise has a scripted turn here.
//! The split keeps those jobs apart:
//!
//! - `scenario` — the registry: which demo a prompt selects. **Adding a demo
//!   is one entry there plus its turn function** (`docs/dummy-backend.md`).
//! - `script` — the canned replies and the streaming primitives.
//! - `turns` — the **pure** scripted turns (`Cue → Vec<StreamEvent>`),
//!   unit-testable event-by-event.
//! - `gated` — the turns that *ask*, blocking on the permission gate the way
//!   a real backend's tool thread does (`docs/permissions.md`).
//!
//! This module holds only the driver: select the scenario for the prompt, then
//! either play its script back with the delays that make streaming visible, or
//! hand a gated demo the channel and let it ask.

use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::context::ContextMessage;

use self::scenario::{Cue, Play, Stage};
use super::{CancelToken, ReplySource, StreamEvent};

mod agent;
mod gated;
pub(super) mod scenario;
pub(super) mod script;
mod turns;

pub use self::script::{MARKDOWN_TOUR, TOKEN_MAX_CHARS, chunks, dummy_response, image_ack, tokens};
pub use self::turns::AGENT_DELAY;

/// The full ordered sequence of events for one dummy turn — the **pure** twin
/// of [`DummyAi::spawn`], and the entry point every offline test drives.
///
/// The scenario registry picks the turn (`docs/dummy-backend.md`): a "table"
/// prompt streams the markdown-table demo, an "agents" prompt the subagent
/// group, a "parallel" prompt the vivid three-`Bash` batch, `/compact`'s
/// request a text-only summary, and anything else the default turn — half the
/// reply, a thinking phase, a `Read`+`Bash` batch, the rest of the reply.
///
/// No gate and no registry are in play here, so the gated, asked and agent
/// demos are skipped and *every* prompt resolves to a script. Deterministic,
/// so a turn's whole event order is unit-testable; [`DummyAi`] plays the same
/// script back with delays.
#[must_use]
pub fn turn_events(prompt: &str, image_count: usize) -> Vec<StreamEvent> {
    let cue = Cue::new(prompt, image_count);
    match scenario::select(&cue, false, false, false).play {
        Play::Script(script) => script(&cue),
        // Unreachable: selecting with nothing attached skips every gated,
        // asked and agent entry. Answering with the default turn beats
        // panicking.
        Play::Gated(_) | Play::Asked(_) | Play::Agent(_) => turns::tools_turn(&cue),
    }
}

/// How long [`DummyAi`] waits after a turn starts before streaming its first
/// chunk — so the status indicator (the spinner, the ticking elapsed timer, and
/// the `↑ N tokens` count for the just-sent user message) is visible *before*
/// any reply text appears. The status animates during this pause because the
/// draw loop re-arms a frame every 32ms while a turn is active (see `main.rs`),
/// and an Esc reaps the thread promptly (the wait is an interruptible
/// `nap`). Configurable per backend via [`DummyAi::with_startup_delay`] (the
/// app reads `ALTER_ZERO_STARTUP_DELAY_MS`; tests use a short delay).
pub const STARTUP_DELAY: Duration = Duration::from_secs(3);

/// Delay between streamed chunks. Small enough to feel responsive, large
/// enough that the word-by-word reveal is visible. The default of
/// [`DummyAi::with_chunk_delay`], which the app overrides from
/// `ALTER_ZERO_CHUNK_DELAY_MS` to mimic a slow model (`docs/slow-stream.md`).
pub const CHUNK_DELAY: Duration = Duration::from_millis(45);

/// How long a dummy tool "runs" — the pause between its `ToolStart` and
/// `ToolEnd` — so the blue running state is visible before it resolves.
pub const TOOL_DELAY: Duration = Duration::from_millis(450);

/// How long each frame of a scripted terminal screen holds before the next
/// replaces it — slow enough to watch a progress bar move in place.
pub const SCREEN_FRAME_DELAY: Duration = Duration::from_millis(250);

/// Delay after `ThinkingStart` and after each `ThinkingChunk`, so the
/// reasoning trickles — visibly, in the live block (`docs/thinking-stream.md`)
/// — and the token tally ticks while the model "thinks". The phase's total
/// length is one step per event — `(1 + chunks) × THINK_CHUNK_DELAY` (≈2s for
/// the canned reasoning's twelve words), long enough that `Thinking for Ns`
/// ticks from 0s.
pub const THINK_CHUNK_DELAY: Duration = Duration::from_millis(150);

/// The built-in canned-reply backend used by the demo.
#[derive(Debug, Clone)]
pub struct DummyAi {
    /// Pause before the first streamed event so the status indicator shows
    /// first ([`STARTUP_DELAY`] by default; the app overrides it from
    /// `ALTER_ZERO_STARTUP_DELAY_MS`, tests use a short value).
    startup_delay: Duration,
    /// Pause after every streamed piece of reply text (and each live tool
    /// output line) — [`CHUNK_DELAY`] by default; the app overrides it from
    /// `ALTER_ZERO_CHUNK_DELAY_MS`, which is how the offline backend mimics
    /// a model streaming a few tokens a second (`docs/slow-stream.md`).
    chunk_delay: Duration,
    /// The shared tool-permission gate, when the app attached one — lets the
    /// **offline** dummy drive the whole approval round trip for a prompt
    /// mentioning "permission" (`docs/permissions.md`, `smoke.sh` Phase 55).
    permissions: Option<crate::permission::PermissionGate>,
    /// The shared ask gate, when the app attached one — lets the offline
    /// dummy drive the whole `AskUserQuestion` round trip for a prompt
    /// mentioning "ask" + "question" (`docs/ask.md`).
    ask: Option<crate::ask::AskGate>,
    /// The shared subagent registry, when the app attached one — the channel
    /// the "subagent" demo streams a launched agent's own round on, so the
    /// agent **session view** is drivable offline
    /// (`docs/agent-view-streaming.md`).
    agents: Option<crate::agents::AgentRegistry>,
    /// The session's mid-turn message queue (`docs/queue.md`), so the whole
    /// round trip is drivable offline: a scripted turn takes what the user
    /// queued at each of its **tool boundaries** — the dummy's honest stand-in
    /// for a real round boundary — and announces it with
    /// [`StreamEvent::Steered`] exactly as `run_agent` does.
    steer: crate::steer::SteerQueue,
}

impl Default for DummyAi {
    fn default() -> Self {
        Self {
            startup_delay: STARTUP_DELAY,
            chunk_delay: CHUNK_DELAY,
            permissions: None,
            ask: None,
            agents: None,
            steer: crate::steer::SteerQueue::new(),
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
        Self {
            startup_delay,
            ..Self::default()
        }
    }

    /// A dummy pacing its reply at `chunk_delay` per streamed piece — the app
    /// threads `ALTER_ZERO_CHUNK_DELAY_MS` through here so the demo streams
    /// as slowly as a struggling model, and the smoke suite drives the
    /// inline pipeline at that pace (`docs/slow-stream.md`).
    #[must_use]
    pub fn with_chunk_delay(mut self, chunk_delay: Duration) -> Self {
        self.chunk_delay = chunk_delay;
        self
    }

    /// Attach the session's permission gate, so a prompt mentioning
    /// "permission" plays the scripted `Write` approval — the offline mirror of
    /// what a real backend's `write` call does (`docs/permissions.md`).
    #[must_use]
    pub fn with_permissions(mut self, gate: crate::permission::PermissionGate) -> Self {
        self.permissions = Some(gate);
        self
    }

    /// Attach the session's ask gate, so a prompt mentioning "ask" and
    /// "question" plays the scripted `AskUserQuestion` round trip — the
    /// offline mirror of the real tool (`docs/ask.md`).
    #[must_use]
    pub fn with_ask(mut self, gate: crate::ask::AskGate) -> Self {
        self.ask = Some(gate);
        self
    }

    /// Attach the session's subagent registry, enabling the "subagent" demo:
    /// the launched agent's own round streams on that channel, which is what
    /// makes the agent session view drivable with no network
    /// (`docs/agent-view-streaming.md`).
    #[must_use]
    pub fn with_agents(mut self, registry: crate::agents::AgentRegistry) -> Self {
        self.agents = Some(registry);
        self
    }

    /// Attach the session's mid-turn message queue, so a scripted turn takes
    /// what the user queued into it at its next tool boundary — the offline
    /// mirror of a real round boundary (`docs/queue.md`).
    #[must_use]
    pub fn with_steer(mut self, queue: crate::steer::SteerQueue) -> Self {
        self.steer = queue;
        self
    }
}

impl ReplySource for DummyAi {
    /// Pauses for the startup delay, then plays the scenario the prompt selects
    /// (`docs/dummy-backend.md`).
    ///
    /// A **scripted** one is [`turn_events`]' list, replayed with the delays
    /// that make streaming visible: the reply word-by-word ([`CHUNK_DELAY`]
    /// between words) with a parallel batch interleaved — announced up front,
    /// so the not-yet-run calls show `⎿ Waiting…` while the front one runs —
    /// pausing [`TOOL_DELAY`] after each `ToolStart` so the running state shows
    /// before it resolves, ending in [`StreamEvent::StreamDone`]. A **gated**
    /// one instead streams as it goes, blocking on the attached permission gate
    /// for each call's answer (`docs/permissions.md`); it is only ever selected
    /// when [`with_permissions`](DummyAi::with_permissions) supplied a gate.
    ///
    /// Either way it stops early — sending nothing further — if `cancel` is
    /// tripped or the receiver has hung up. With `images` attached the reply
    /// opens with an acknowledgement (the dummy has no vision; see
    /// `docs/image-paste.md`).
    fn spawn(
        &self,
        prompt: String,
        images: Vec<PathBuf>,
        _context: Vec<ContextMessage>, // canned replies — no context to use
        tx: UnboundedSender<StreamEvent>,
        cancel: CancelToken,
    ) -> JoinHandle<()> {
        let startup_delay = self.startup_delay;
        let chunk_delay = self.chunk_delay;
        // The dummy can't read the files, only acknowledge how many arrived.
        let image_count = images.len();
        let permissions = self.permissions.clone();
        let ask = self.ask.clone();
        let agents = self.agents.clone();
        let steer = self.steer.clone();
        thread::spawn(move || {
            // Pause before streaming so the status indicator is visible first
            // (interruptibly — an Esc during the wait reaps the thread at once).
            nap(startup_delay, &cancel);
            if cancel.is_cancelled() {
                return;
            }
            let cue = Cue::new(&prompt, image_count);
            match scenario::select(&cue, permissions.is_some(), ask.is_some(), agents.is_some())
                .play
            {
                // A gated demo asks, blocking on the permission gate exactly
                // as a real backend's tool thread does, and streams as it
                // resolves. Selection only offers one when the gate was
                // attached, so the fallback arm is unreachable by
                // construction; a script is the safe answer.
                Play::Gated(play) => match &permissions {
                    Some(gate) => play(&Stage {
                        gate,
                        tx: &tx,
                        cancel: &cancel,
                    }),
                    None => replay(turns::tools_turn(&cue), &tx, &cancel, &steer, chunk_delay),
                },
                // The ask demo raises the question modal and blocks on the
                // ask gate the same way (`docs/ask.md`).
                Play::Asked(play) => match &ask {
                    Some(gate) => play(&scenario::AskStage {
                        gate,
                        tx: &tx,
                        cancel: &cancel,
                    }),
                    None => replay(turns::tools_turn(&cue), &tx, &cancel, &steer, chunk_delay),
                },
                // The subagent demo streams on the agent channel too, from
                // the launched agent's own thread (docs/agent-view-streaming.md).
                Play::Agent(play) => match &agents {
                    Some(registry) => play(&scenario::AgentStage {
                        agents: registry,
                        tx: &tx,
                        cancel: &cancel,
                        // A subagent's own call asks on the same gate the
                        // main turn's does; `None` when permissions are off
                        // (docs/permissions.md).
                        gate: permissions.as_ref(),
                    }),
                    None => replay(turns::tools_turn(&cue), &tx, &cancel, &steer, chunk_delay),
                },
                Play::Script(script) => replay(script(&cue), &tx, &cancel, &steer, chunk_delay),
            }
        })
    }

    /// The dummy's placeholder model id (a real backend reports its real one).
    fn model_name(&self) -> String {
        "dummy_model_name".to_string()
    }

    /// A message sent into the demo subagent's session (`docs/queue.md`). The
    /// **registry** decides, exactly as it does for a real backend: a running
    /// agent takes it onto its queue for the next round boundary, a settled
    /// one gets a short scripted continuation round. Declined without a
    /// registry attached — there is no agent to talk to.
    fn spawn_agent_chat(&self, id: &str, text: &str) -> super::AgentChatDelivery {
        let Some(registry) = &self.agents else {
            return super::AgentChatDelivery::Declined;
        };
        if registry.queue_input(id, text) {
            return super::AgentChatDelivery::Queued;
        }
        let Some((_messages, cancel)) = registry.begin_continuation(id) else {
            return super::AgentChatDelivery::Declined;
        };
        agent::spawn_chat_continuation(registry.clone(), id.to_string(), cancel);
        super::AgentChatDelivery::Started
    }

    fn agent_ready_for_turn(&self, id: &str) -> bool {
        self.agents
            .as_ref()
            .is_some_and(|registry| registry.ready_for_turn(id))
    }

    fn reclaim_agent_input(&self, id: &str) -> Option<String> {
        self.agents.as_ref()?.take_last_input(id)
    }
}

/// Play a scripted turn onto the channel: send each event, then pause for as
/// long as [`pace`] says so the reply visibly streams — every piece of reply
/// text `chunk_delay` apart. Stops early — sending nothing further — the
/// moment `cancel` is tripped or the receiver hangs up.
///
/// **Tool boundaries are round boundaries here** (`docs/queue.md`): a resolved
/// call is the point a real agent loop would build its next request at, so
/// that is where a message the user queued mid-turn is taken and announced
/// with [`StreamEvent::Steered`]. Without this the offline demo would show a
/// queued row that only ever moved at the end of the turn — the behaviour this
/// whole seam replaced.
fn replay(
    events: Vec<StreamEvent>,
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
    steer: &crate::steer::SteerQueue,
    chunk_delay: Duration,
) {
    for event in events {
        if cancel.is_cancelled() {
            return; // asked to stop — drop the rest quietly
        }
        let boundary = matches!(
            event,
            StreamEvent::ToolEnd { .. } | StreamEvent::ToolAnswered { .. }
        );
        let pause = pace(&event, chunk_delay);
        if tx.send(event).is_err() {
            return; // receiver gone — stop quietly
        }
        if boundary {
            for text in steer.take() {
                if tx.send(StreamEvent::Steered { text }).is_err() {
                    return;
                }
            }
        }
        if let Some(pause) = pause {
            nap(pause, cancel);
        }
    }
}

/// How long to pause *after* sending `event`, so a scripted turn plays back at
/// human speed: a tool "runs" for [`TOOL_DELAY`] (grey) before its `ToolEnd`
/// resolves it, the model "thinks" one [`THINK_CHUNK_DELAY`] step per reasoning
/// event, and every word/output line trickles at `chunk_delay` —
/// [`CHUNK_DELAY`] unless the backend was built slower. `None` means the next
/// event follows immediately.
fn pace(event: &StreamEvent, chunk_delay: Duration) -> Option<Duration> {
    match event {
        StreamEvent::Chunk(_) => Some(chunk_delay),
        StreamEvent::ToolStart { .. } => Some(TOOL_DELAY),
        // A foreground agent group "runs" between its announcement
        // and its resolution so the live tree cell shows; a
        // background launch resolves at once (docs/agent-tool.md).
        StreamEvent::AgentBatch { background, .. } if !background => Some(AGENT_DELAY),
        // Each streamed output line pauses like a word so the live
        // cell visibly tails (docs/tool-streaming.md).
        StreamEvent::ToolOutput(_) => Some(chunk_delay),
        // A terminal session's frame holds long enough to be seen before
        // the next replaces it (docs/interactive-shell.md).
        StreamEvent::ToolScreen { .. } => Some(SCREEN_FRAME_DELAY),
        // Each task-tool call pauses like a running tool so the checklist
        // under the status line visibly grows row by row
        // (docs/task-tools.md).
        StreamEvent::TaskCall { .. } => Some(TOOL_DELAY),
        StreamEvent::ThinkingStart
        | StreamEvent::ThinkingChunk(_)
        | StreamEvent::ToolCallDelta(_) => Some(THINK_CHUNK_DELAY),
        _ => None,
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
