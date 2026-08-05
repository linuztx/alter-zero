//! The built-in offline backend: [`DummyAi`].
//!
//! It is the demo's stand-in for a real model and the only backend
//! `scripts/smoke.sh` can drive deterministically, so every UI feature that a
//! provider would otherwise be needed to exercise has a scripted turn here.
//! The split keeps those two jobs apart:
//!
//! - [`script`] — the canned replies and the streaming primitives.
//! - [`turns`] — the **pure** scripted turns (`prompt → Vec<StreamEvent>`),
//!   unit-testable event-by-event.
//! - [`gated`] — the turns that *ask*, blocking on the permission gate the way
//!   a real backend's tool thread does (`docs/permissions.md`).
//!
//! This module holds only the [`ReplySource`] impl: pick the turn for the
//! prompt, then play it back with the delays that make streaming visible.

use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::context::ContextMessage;

use super::{CancelToken, ReplySource, StreamEvent};

mod gated;
mod script;
mod turns;

pub use self::script::{chunks, dummy_response, image_ack};
pub use self::turns::{AGENT_DELAY, turn_events};

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
/// enough that the word-by-word reveal is visible.
pub const CHUNK_DELAY: Duration = Duration::from_millis(45);

/// How long a dummy tool "runs" — the pause between its `ToolStart` and
/// `ToolEnd` — so the blue running state is visible before it resolves.
pub const TOOL_DELAY: Duration = Duration::from_millis(450);

/// Delay after `ThinkingStart` and after each `ThinkingChunk`, so the
/// reasoning trickles and the token tally visibly ticks while the model
/// "thinks". The phase's total length is one step per event —
/// `(1 + chunks) × THINK_CHUNK_DELAY` (≈1.2s for the canned reasoning's seven
/// words), long enough that `Thinking for Ns` ticks from 0s.
pub const THINK_CHUNK_DELAY: Duration = Duration::from_millis(150);

/// The built-in canned-reply backend used by the demo.
#[derive(Debug, Clone)]
pub struct DummyAi {
    /// Pause before the first streamed event so the status indicator shows
    /// first ([`STARTUP_DELAY`] by default; the app overrides it from
    /// `ALTER_ZERO_STARTUP_DELAY_MS`, tests use a short value).
    startup_delay: Duration,
    /// The shared tool-permission gate, when the app attached one — lets the
    /// **offline** dummy drive the whole approval round trip for a prompt
    /// mentioning "permission" (`docs/permissions.md`, `smoke.sh` Phase 55).
    permissions: Option<crate::permission::PermissionGate>,
}

impl Default for DummyAi {
    fn default() -> Self {
        Self {
            startup_delay: STARTUP_DELAY,
            permissions: None,
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
            permissions: None,
        }
    }

    /// Attach the session's permission gate, so a prompt mentioning
    /// "permission" plays the scripted `Write` approval — the offline mirror of
    /// what a real backend's `write` call does (`docs/permissions.md`).
    #[must_use]
    pub fn with_permissions(mut self, gate: crate::permission::PermissionGate) -> Self {
        self.permissions = Some(gate);
        self
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
        let permissions = self.permissions.clone();
        thread::spawn(move || {
            // Pause before streaming so the status indicator is visible first
            // (interruptibly — an Esc during the wait reaps the thread at once).
            nap(startup_delay, &cancel);
            if cancel.is_cancelled() {
                return;
            }
            // The scripted approval demos: ask, block on the gate exactly as a
            // real backend's tool thread does, then resolve accordingly. A
            // prompt naming both "parallel" and "permission" plays the
            // two-command `bash` batch (back-to-back prompts, no pauses —
            // the covering machinery's hardest timing); "staggered" and
            // "permission" the two-`write` batch whose prompts differ wildly
            // in height (the tall one capped, the next tiny — the pinned
            // modal region's hardest shrink); "permission" alone keeps the
            // single `Write` approval.
            if let Some(gate) = permissions.filter(|_| prompt.to_lowercase().contains("permission"))
            {
                if prompt.to_lowercase().contains("auto") {
                    gated::dummy_auto_permission_turn(&gate, &tx, &cancel);
                } else if prompt.to_lowercase().contains("parallel") {
                    gated::dummy_parallel_permission_turn(&gate, &tx, &cancel);
                } else if prompt.to_lowercase().contains("staggered") {
                    gated::dummy_staggered_permission_turn(&gate, &tx, &cancel);
                } else {
                    gated::dummy_permission_turn(&gate, &tx, &cancel);
                }
                return;
            }
            for event in turn_events(&prompt, image_count) {
                if cancel.is_cancelled() {
                    return; // asked to stop — drop the rest quietly
                }
                let pause = pace(&event);
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

/// How long to pause *after* sending `event`, so a scripted turn plays back at
/// human speed: a tool "runs" for [`TOOL_DELAY`] (grey) before its `ToolEnd`
/// resolves it, the model "thinks" one [`THINK_CHUNK_DELAY`] step per reasoning
/// event, and every word/output line trickles at [`CHUNK_DELAY`]. `None` means
/// the next event follows immediately.
fn pace(event: &StreamEvent) -> Option<Duration> {
    match event {
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
