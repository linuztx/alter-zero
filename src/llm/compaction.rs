//! Background compaction's meeting point (`docs/compact.md` *Background
//! compaction*): the slot the boundary and the lead agent's thread share, and
//! the agent's half of the protocol — what [`super::agent::run_agent_with`]
//! asks at the top of every round, after the pending inputs and before the
//! request goes out.
//!
//! The boundary owns the *policy* — the window, the `/settings` **Auto
//! compact** knob, how small a compaction can make the context — and the
//! *work*: it records the checkpoint, writes the summary on a thread of its
//! own and builds the compacted head from the history with the very
//! derivation the next turn will use. The agent owns *when*: it knows its own
//! size (the last request's usage frame, plus an estimate of what it appended
//! since), so it takes a checkpoint the moment it crosses the threshold, keeps
//! working while the summary is written, splices the head in at the first
//! round top after it lands, and — never sending a request that would
//! overflow the window — waits at the hard limit for one that is still being
//! written, or forces one when nothing is.
//!
//! The protocol, in channel order on the turn's reply channel:
//!
//! ```text
//! agent  ──CompactionCheckpoint{id}──►  boundary records the cut, starts the summarizer
//! agent  (keeps working; rounds n+1…)
//! agent  ──CompactionWait{id}──►        only at the hard limit
//! boundary publishes Handoff{id, head} on the slot
//! agent  splices [system] + head + messages[n..]
//! agent  ──CompactionApplied{id}──►     boundary appends the marker there
//! ```

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use super::{ChatMessage, ContentPart, MessageContent};
use crate::stream::{CancelToken, StreamEvent};
use crate::tokens::Calibration;

/// How often a wait at the hard limit looks at the turn's cancel token —
/// the cadence every blocking wait on the backend thread polls it at, so
/// Esc stays the stop button (`docs/interrupt.md`).
pub const WAIT_POLL: Duration = Duration::from_millis(20);

/// The share of the window, in hundredths, past which the agent takes a
/// checkpoint on its own — codex's 90% (`ModelInfo::auto_compact_token_limit`).
const SOFT_PERCENT: u64 = 90;

/// The share of the window, in hundredths, the agent never sends a request
/// past: the last twentieth is the reply's own room.
const HARD_PERCENT: u64 = 95;

/// Where compaction starts for a `window`-token context window: codex's 90%.
/// The trigger and the `/context` page's headroom both read it.
#[must_use]
pub const fn soft_threshold(window: u64) -> u64 {
    window.saturating_mul(SOFT_PERCENT) / 100
}

/// What the agent never sends a request past for a `window`-token window —
/// 95%, the last twentieth left for the reply. At or past it the agent waits
/// for the summary at the round boundary instead (`docs/compact.md` *The
/// hard limit*).
#[must_use]
pub const fn hard_limit(window: u64) -> u64 {
    window.saturating_mul(HARD_PERCENT) / 100
}

/// What the boundary tells the running agent about compacting: the window,
/// whether it may compact on its own, and how small compacting can make the
/// context at best.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionPolicy {
    /// The model's context window, in tokens.
    pub window: u64,
    /// The `/settings` **Auto compact** knob: whether the agent checkpoints
    /// on its own past [`soft_threshold`]. Off, it still never overflows —
    /// the hard limit forces one.
    pub auto: bool,
    /// The size of the last compaction's head — instructions, tool schemas,
    /// the recent user messages and the summary — when it is known: the
    /// smallest a compaction can make the context. At or past a threshold,
    /// compacting cannot get under it, so the agent does not try
    /// (`docs/compact.md` *When compacting cannot help*).
    pub floor: Option<u64>,
    /// What the session's usage reports taught about the provider's tokens
    /// against the estimate (`docs/token-estimate.md`): the agent reads what
    /// it appended since its last report through it, as the gauge does.
    /// Read raw, a provider counting twice the estimate would see a request
    /// the reading put under the hard limit go out past the window
    /// (`docs/compact.md` *The hard limit*).
    pub calibration: Calibration,
}

impl CompactionPolicy {
    /// Whether an automatic compaction could get the context under the
    /// threshold at all.
    #[must_use]
    pub fn soft_can_help(&self) -> bool {
        self.floor
            .is_none_or(|floor| floor < soft_threshold(self.window))
    }

    /// Whether a forced compaction could get the context under the hard
    /// limit at all.
    #[must_use]
    pub fn hard_can_help(&self) -> bool {
        self.floor
            .is_none_or(|floor| floor < hard_limit(self.window))
    }
}

/// A finished summary, published for the checkpoint `id`: the compacted
/// head the agent splices in place of the snapshot it checkpointed
/// (`context::compacted_head`, already in the wire's shape) and what the
/// boundary estimates the head costs, the system prompt and the tool
/// schemas included.
#[derive(Debug, Clone, PartialEq)]
pub struct Handoff {
    /// The checkpoint this summary covers.
    pub id: u64,
    /// The `<system-reminder>`, the recent user messages and the summary
    /// bridge — what replaces `messages[1..n]`.
    pub head: Vec<ChatMessage>,
    /// The head's cost, with the system prompt and the tool schemas.
    pub head_tokens: u64,
}

/// The shared slot (`SteerQueue`'s shape): cloneable, every clone the same
/// slot. The boundary keeps one and hands a clone to every backend it
/// builds, so a `/model` switch mid-compaction keeps the summary's way to
/// the agent open.
///
/// A poisoned lock (a panicked thread) reads as an empty slot: a summary
/// lost that way only means the context is not compacted yet.
#[derive(Clone, Default, Debug)]
pub struct CompactionSlot {
    inner: Arc<(Mutex<Slot>, Condvar)>,
}

#[derive(Debug, Default)]
struct Slot {
    policy: Option<CompactionPolicy>,
    requested: bool,
    handoff: Option<Handoff>,
    failed: Option<u64>,
    next_id: u64,
}

impl CompactionSlot {
    /// An empty slot whose checkpoint ids start at `first` — the boundary
    /// seeds it from the clock, so ids stay distinct across a `/resume`.
    #[must_use]
    pub fn starting_at(first: u64) -> Self {
        let slot = Self::default();
        slot.with(|slot| slot.next_id = first);
        slot
    }

    fn with<T>(&self, f: impl FnOnce(&mut Slot) -> T) -> Option<T> {
        let (lock, _) = &*self.inner;
        lock.lock().ok().map(|mut slot| f(&mut slot))
    }

    fn notify(&self) {
        self.inner.1.notify_all();
    }

    // ----- the boundary's side -----

    /// Publish what the agent may do (`None`: the window is unknown — no
    /// automatic compaction and no hard limit).
    pub fn set_policy(&self, policy: Option<CompactionPolicy>) {
        self.with(|slot| slot.policy = policy);
    }

    /// Ask the running agent for a checkpoint at its next round top —
    /// `/compact` while it works.
    pub fn request(&self) {
        self.with(|slot| slot.requested = true);
    }

    /// Hand the agent the summary of its checkpoint.
    pub fn publish(&self, handoff: Handoff) {
        self.with(|slot| {
            slot.failed = None;
            slot.handoff = Some(handoff);
        });
        self.notify();
    }

    /// The summary of checkpoint `id` could not be written.
    pub fn fail(&self, id: u64) {
        self.with(|slot| {
            slot.handoff = None;
            slot.failed = Some(id);
        });
        self.notify();
    }

    /// Forget everything in flight — a turn that ended, an Esc, a `/clear`:
    /// no summary may reach an agent whose list never held its snapshot.
    pub fn reset(&self) {
        self.with(|slot| {
            slot.requested = false;
            slot.handoff = None;
            slot.failed = None;
        });
        self.notify();
    }

    /// Whether a `/compact` request is still waiting for a round top.
    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.with(|slot| slot.requested).unwrap_or(false)
    }

    // ----- the agent's side -----

    fn policy(&self) -> Option<CompactionPolicy> {
        self.with(|slot| slot.policy).flatten()
    }

    fn take_request(&self) -> bool {
        self.with(|slot| std::mem::take(&mut slot.requested))
            .unwrap_or(false)
    }

    fn next_id(&self) -> u64 {
        self.with(|slot| {
            slot.next_id = slot.next_id.wrapping_add(1);
            slot.next_id
        })
        .unwrap_or_default()
    }

    /// What became of checkpoint `id`, taking it.
    fn take_outcome(&self, id: u64) -> Option<Outcome> {
        self.with(|slot| {
            if slot
                .handoff
                .as_ref()
                .is_some_and(|handoff| handoff.id == id)
            {
                return slot.handoff.take().map(Outcome::Ready);
            }
            if slot.failed == Some(id) {
                slot.failed = None;
                return Some(Outcome::Failed);
            }
            None
        })
        .flatten()
    }

    /// Block for at most `timeout` until something is published or failed.
    fn wait(&self, timeout: Duration) {
        let (lock, condvar) = &*self.inner;
        if let Ok(guard) = lock.lock()
            && guard.handoff.is_none()
            && guard.failed.is_none()
        {
            let _ = condvar.wait_timeout(guard, timeout);
        }
    }
}

enum Outcome {
    Ready(Handoff),
    Failed,
}

/// What the round-top seam tells the agent loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundTop {
    /// Send the round.
    Go,
    /// The turn was cancelled while waiting — stop silently.
    Stop,
    /// The request cannot fit the window and nothing can make it: end the
    /// turn on this error.
    Fail(String),
}

/// The agent loop's round-top seam (`docs/compact.md`): consulted at the top
/// of every round, after the pending inputs, before the request. May splice
/// `messages`, and may block — polling `cancel` every [`WAIT_POLL`].
pub trait RoundTopSeam {
    fn before_round(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        tx: &UnboundedSender<StreamEvent>,
        cancel: &CancelToken,
    ) -> RoundTop;
}

/// The seam that does nothing — every backend but the session's own lead
/// (the one-off `/compact` backend, a subagent, the tests).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCompaction;

impl RoundTopSeam for NoCompaction {
    fn before_round(
        &mut self,
        _messages: &mut Vec<ChatMessage>,
        _tx: &UnboundedSender<StreamEvent>,
        _cancel: &CancelToken,
    ) -> RoundTop {
        RoundTop::Go
    }
}

/// Whether a failed request's error says it did not fit the model's context
/// window — the one failure a summarizer retries, on a smaller request
/// (codex drops its oldest item and retries; `docs/compact.md` *The
/// summarization request must fit*). Read off the words the providers use,
/// since the wire gives no common code for it.
#[must_use]
pub fn is_context_window_refusal(message: &str) -> bool {
    const SIGNS: &[&str] = &[
        "context window",
        "context length",
        "context_length",
        "maximum context",
        "context limit",
        "prompt is too long",
        "input is too long",
        "too many tokens",
        "maximum number of tokens",
        "exceeds the context",
        "reduce the length",
    ];
    let message = message.to_lowercase();
    SIGNS.iter().any(|sign| message.contains(sign))
}

/// The error a turn ends on when its next request cannot fit the window and
/// compacting could not make room — the summary failed, or the compacted
/// context itself is too big.
#[must_use]
pub fn context_full_notice(size: u64, window: u64) -> String {
    format!(
        "the conversation no longer fits the model's context window (about {size} of {window} \
         tokens) and compacting it could not make room — run /compact to try again, or /clear \
         to start over"
    )
}

/// The lead agent's half of the protocol, one per turn (`docs/compact.md`).
#[derive(Debug)]
pub struct AgentCompactor {
    slot: CompactionSlot,
    /// What the first messages cost: `(tokens, count)` — the provider's count
    /// of the last request (its usage frame), or the head a splice put in,
    /// or what the estimate made of them — so a reading estimates only what
    /// joined since.
    base: (u64, usize),
    /// The checkpoint in flight: its id and the message count it covers.
    pending: Option<(u64, usize)>,
    /// The message count of the last checkpoint whose summary failed — a
    /// new automatic or forced one waits for something to join after it.
    failed_at: Option<usize>,
    /// The head of the last splice this turn — the smallest compacting can
    /// make the context, until the boundary's next policy says otherwise.
    floor: Option<u64>,
    /// Whether the reading stands on a count rather than the estimate alone
    /// — a usage frame, or the head a splice put in. Only such a reading is
    /// trusted to refuse a request; the estimate alone sends it for the
    /// provider to judge.
    anchored: bool,
    /// Whether the last call spliced.
    spliced: bool,
}

impl AgentCompactor {
    /// The compactor for one turn over `slot`; `tools` is the tool schemas'
    /// estimated cost, which every request carries beside its messages.
    #[must_use]
    pub fn new(slot: CompactionSlot, tools: u64) -> Self {
        Self {
            slot,
            base: (tools, 0),
            pending: None,
            failed_at: None,
            floor: None,
            anchored: false,
            spliced: false,
        }
    }

    /// The round that sent `count` messages was counted at `input` tokens by
    /// the provider — the base every later reading adds to.
    pub fn observe(&mut self, input: u64, count: usize) {
        if input > 0 {
            self.base = (input, count);
            self.anchored = true;
        }
    }

    /// Whether the last [`before_round`](RoundTopSeam::before_round) spliced
    /// a summary in — the caller re-reads whatever it derived from the old
    /// list (the skills it holds loaded).
    #[must_use]
    pub const fn spliced(&self) -> bool {
        self.spliced
    }

    /// What `messages` cost: the base, plus the estimate of what joined
    /// after it read through `calibration` into the provider's tokens —
    /// folded into the base, so each reading estimates only the newest
    /// messages.
    fn size(&mut self, messages: &[ChatMessage], calibration: Calibration) -> u64 {
        let (tokens, count) = self.base;
        let count = count.min(messages.len());
        let added: u64 = messages[count..].iter().map(message_tokens).sum();
        self.base = (
            tokens.saturating_add(calibration.correct(added)),
            messages.len(),
        );
        self.base.0
    }

    fn checkpoint(&mut self, messages: &[ChatMessage], tx: &UnboundedSender<StreamEvent>) -> u64 {
        let id = self.slot.next_id();
        self.pending = Some((id, messages.len()));
        let _ = tx.send(StreamEvent::CompactionCheckpoint { id });
        id
    }

    /// Splice `handoff`'s head in place of the snapshot the checkpoint
    /// covered: `[system] + head + messages[n..]`.
    fn splice(&mut self, messages: &mut Vec<ChatMessage>, covered: usize, handoff: Handoff) {
        let keep = usize::from(messages.first().is_some_and(|m| m.role == "system"));
        let covered = covered.clamp(keep, messages.len());
        let head_len = handoff.head.len();
        messages.splice(keep..covered, handoff.head);
        self.base = (handoff.head_tokens, keep + head_len);
        self.anchored = true;
        self.floor = Some(handoff.head_tokens);
        // The message counts a failure was measured against described the
        // list the splice replaced.
        self.failed_at = None;
        self.spliced = true;
    }

    /// Resolve the checkpoint in flight from what the slot holds — splice a
    /// published summary, forget a failed one.
    fn settle(&mut self, messages: &mut Vec<ChatMessage>, tx: &UnboundedSender<StreamEvent>) {
        let Some((id, covered)) = self.pending else {
            return;
        };
        match self.slot.take_outcome(id) {
            Some(Outcome::Ready(handoff)) => {
                self.pending = None;
                self.splice(messages, covered, handoff);
                let _ = tx.send(StreamEvent::CompactionApplied { id });
            }
            Some(Outcome::Failed) => {
                self.pending = None;
                self.failed_at = Some(covered);
            }
            None => {}
        }
    }

    /// Whether something joined since the last failed checkpoint — a new
    /// automatic or forced one would otherwise summarize the very snapshot
    /// that just failed.
    fn grew_since_failure(&self, messages: &[ChatMessage]) -> bool {
        self.failed_at.is_none_or(|at| messages.len() > at)
    }
}

impl RoundTopSeam for AgentCompactor {
    fn before_round(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        tx: &UnboundedSender<StreamEvent>,
        cancel: &CancelToken,
    ) -> RoundTop {
        self.spliced = false;
        let mut waited_for = None;
        loop {
            if cancel.is_cancelled() {
                return RoundTop::Stop;
            }
            self.settle(messages, tx);
            let policy = self.slot.policy();
            let size = self.size(
                messages,
                policy.map(|policy| policy.calibration).unwrap_or_default(),
            );
            // Only a conversation with a round in it has anything to fold
            // into a summary: before the first, the size is the user's
            // message and the fixed overhead, which no summary shrinks.
            let compactable = self.pending.is_none() && has_a_round(messages);
            // A request the user made while this turn ran is taken whatever
            // the size; one of our own past the threshold only while
            // compacting can still help, and not twice over one snapshot.
            let requested = compactable && self.slot.take_request();
            let automatic = compactable
                && policy.is_some_and(|policy| {
                    policy.auto
                        && policy.soft_can_help()
                        && self
                            .floor
                            .is_none_or(|floor| floor < soft_threshold(policy.window))
                        && size >= soft_threshold(policy.window)
                })
                && self.grew_since_failure(messages);
            if requested || automatic {
                self.checkpoint(messages, tx);
            }
            let Some(policy) = policy else {
                return RoundTop::Go;
            };
            if size < hard_limit(policy.window) {
                return RoundTop::Go;
            }
            // At the hard limit: the request must not go out as it stands.
            if self.pending.is_none() {
                let can_help = compactable
                    && policy.hard_can_help()
                    && self
                        .floor
                        .is_none_or(|floor| floor < hard_limit(policy.window))
                    && self.grew_since_failure(messages);
                if !can_help {
                    // Nothing can make room: send it if it fits at all — or
                    // if only the estimate says it does not, which the
                    // provider's own count may well contradict — and let the
                    // provider decide; else end the turn saying so.
                    return if size < policy.window || !self.anchored {
                        RoundTop::Go
                    } else {
                        RoundTop::Fail(context_full_notice(size, policy.window))
                    };
                }
                self.checkpoint(messages, tx);
            }
            let Some((id, _)) = self.pending else {
                continue;
            };
            if waited_for != Some(id) {
                waited_for = Some(id);
                let _ = tx.send(StreamEvent::CompactionWait { id });
            }
            self.slot.wait(WAIT_POLL);
        }
    }
}

/// Whether `messages` hold a round to summarize — a reply of the model's,
/// past the system prompt and the user's message.
fn has_a_round(messages: &[ChatMessage]) -> bool {
    messages.iter().any(|message| message.role == "assistant")
}

/// The estimated cost of one wire message: its text, a flat rate per
/// picture, and each tool call's name and arguments — the gauge's rule
/// (`app::message_tokens`) over the wire's shape.
#[must_use]
pub fn message_tokens(message: &ChatMessage) -> u64 {
    let estimate = |text: &str| u64::try_from(crate::tokens::estimate(text)).unwrap_or(u64::MAX);
    let content = match &message.content {
        MessageContent::Text(text) => estimate(text),
        MessageContent::Parts(parts) => parts
            .iter()
            .map(|part| match part {
                ContentPart::Text { text, .. } => estimate(text),
                ContentPart::ImageUrl { .. } => {
                    u64::try_from(crate::app::IMAGE_INPUT_TOKENS).unwrap_or(u64::MAX)
                }
            })
            .sum(),
    };
    let calls: u64 = message
        .tool_calls
        .iter()
        .map(|call| estimate(&call.function.name) + estimate(call.function.arguments.as_str()))
        .sum();
    content.saturating_add(calls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolCallSpec;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn channel() -> (UnboundedSender<StreamEvent>, UnboundedReceiver<StreamEvent>) {
        unbounded_channel()
    }

    fn drain(rx: &mut UnboundedReceiver<StreamEvent>) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    /// A conversation as the agent holds it: the system prompt, the user's
    /// message, then `rounds` tool rounds of `size` bytes of output each.
    fn conversation(rounds: usize, size: usize) -> Vec<ChatMessage> {
        let mut messages = vec![ChatMessage::system("sys"), ChatMessage::user("fix it")];
        for round in 0..rounds {
            push_round(&mut messages, round, size);
        }
        messages
    }

    fn push_round(messages: &mut Vec<ChatMessage>, round: usize, size: usize) {
        let id = format!("call_{round}");
        messages.push(ChatMessage::assistant_tool_calls(
            "",
            vec![ToolCallSpec::function(&id, "bash", r#"{"command":"ls"}"#)],
        ));
        messages.push(ChatMessage::tool_result(&id, "x".repeat(size)));
    }

    fn cost(messages: &[ChatMessage]) -> u64 {
        messages.iter().map(message_tokens).sum()
    }

    fn policy(window: u64) -> CompactionPolicy {
        CompactionPolicy {
            window,
            auto: true,
            floor: None,
            calibration: Calibration::default(),
        }
    }

    /// A calibration that has learned the provider counts twice the
    /// estimate.
    fn doubling() -> Calibration {
        let mut calibration = Calibration::default();
        calibration.learn(2_000, 1_000);
        calibration
    }

    fn head() -> Vec<ChatMessage> {
        vec![ChatMessage::user("fix it\n\nsummary of what happened")]
    }

    fn checkpoint_id(events: &[StreamEvent]) -> u64 {
        events
            .iter()
            .find_map(|event| match event {
                StreamEvent::CompactionCheckpoint { id } => Some(*id),
                _ => None,
            })
            .expect("a checkpoint")
    }

    #[test]
    fn a_context_window_refusal_is_told_apart_from_other_failures() {
        for refusal in [
            "HTTP 400: This model's maximum context length is 8192 tokens",
            "invalid_request_error: prompt is too long: 210000 tokens > 200000 maximum",
            "the reply filled the model's context window before it finished",
            "{\"code\":\"context_length_exceeded\"}",
            "The input token count exceeds the maximum number of tokens allowed",
        ] {
            assert!(is_context_window_refusal(refusal), "{refusal}");
        }
        for other in [
            "HTTP 429: rate limited",
            "connection reset by peer",
            "HTTP 500",
        ] {
            assert!(!is_context_window_refusal(other), "{other}");
        }
    }

    #[test]
    fn the_thresholds_are_ninety_and_ninety_five_percent() {
        assert_eq!(soft_threshold(50_000), 45_000);
        assert_eq!(hard_limit(50_000), 47_500);
    }

    #[test]
    fn a_requested_checkpoint_is_taken_and_the_round_goes_out() {
        let slot = CompactionSlot::starting_at(10);
        let mut compactor = AgentCompactor::new(slot.clone(), 0);
        let (tx, mut rx) = channel();
        let mut messages = conversation(2, 40);
        let before = messages.clone();
        slot.request();
        assert_eq!(
            compactor.before_round(&mut messages, &tx, &CancelToken::new()),
            RoundTop::Go
        );
        assert_eq!(
            drain(&mut rx),
            vec![StreamEvent::CompactionCheckpoint { id: 11 }]
        );
        assert_eq!(messages, before, "nothing changes until the summary lands");
        assert!(!slot.is_requested(), "the request was taken");
    }

    #[test]
    fn the_summary_is_spliced_at_a_later_round_top_and_the_rounds_since_stay() {
        let slot = CompactionSlot::default();
        let mut compactor = AgentCompactor::new(slot.clone(), 0);
        let (tx, mut rx) = channel();
        let cancel = CancelToken::new();
        let mut messages = conversation(2, 40);
        slot.request();
        compactor.before_round(&mut messages, &tx, &cancel);
        let id = checkpoint_id(&drain(&mut rx));
        let covered = messages.len();
        // The agent kept working while the summary was written.
        push_round(&mut messages, 2, 40);
        let tail = messages[covered..].to_vec();
        slot.publish(Handoff {
            id,
            head: head(),
            head_tokens: 30,
        });
        assert_eq!(
            compactor.before_round(&mut messages, &tx, &cancel),
            RoundTop::Go
        );
        assert!(compactor.spliced());
        let mut expected = vec![ChatMessage::system("sys")];
        expected.extend(head());
        expected.extend(tail);
        assert_eq!(messages, expected, "[system] + head + the rounds since");
        assert_eq!(drain(&mut rx), vec![StreamEvent::CompactionApplied { id }]);
        assert_eq!(
            compactor.size(&messages, Calibration::default()),
            30 + cost(&messages[2..]),
            "the head's cost is the new base"
        );
    }

    #[test]
    fn a_summary_for_another_checkpoint_is_never_spliced() {
        let slot = CompactionSlot::default();
        let mut compactor = AgentCompactor::new(slot.clone(), 0);
        let (tx, mut rx) = channel();
        let mut messages = conversation(2, 40);
        let before = messages.clone();
        slot.publish(Handoff {
            id: 99,
            head: head(),
            head_tokens: 30,
        });
        compactor.before_round(&mut messages, &tx, &CancelToken::new());
        assert_eq!(messages, before);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn past_the_threshold_the_agent_checkpoints_on_its_own() {
        let slot = CompactionSlot::default();
        let messages = conversation(3, 400);
        let size = cost(&messages);
        // A window whose threshold the conversation has just crossed, its
        // hard limit still ahead.
        let window = size * 100 / 92;
        slot.set_policy(Some(policy(window)));
        let mut compactor = AgentCompactor::new(slot, 0);
        let (tx, mut rx) = channel();
        let mut messages = messages;
        assert_eq!(
            compactor.before_round(&mut messages, &tx, &CancelToken::new()),
            RoundTop::Go,
            "under the hard limit the round goes out"
        );
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [StreamEvent::CompactionCheckpoint { .. }]
        ));
    }

    #[test]
    fn under_the_threshold_nothing_happens() {
        let slot = CompactionSlot::default();
        let mut messages = conversation(3, 400);
        slot.set_policy(Some(policy(cost(&messages) * 2)));
        let mut compactor = AgentCompactor::new(slot, 0);
        let (tx, mut rx) = channel();
        compactor.before_round(&mut messages, &tx, &CancelToken::new());
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn with_auto_compact_off_only_the_hard_limit_compacts() {
        let slot = CompactionSlot::default();
        let mut messages = conversation(3, 400);
        let window = cost(&messages) * 100 / 92;
        slot.set_policy(Some(CompactionPolicy {
            auto: false,
            ..policy(window)
        }));
        let mut compactor = AgentCompactor::new(slot, 0);
        let (tx, mut rx) = channel();
        compactor.before_round(&mut messages, &tx, &CancelToken::new());
        assert!(
            drain(&mut rx).is_empty(),
            "past 90%, under 95%: no checkpoint"
        );
    }

    #[test]
    fn the_usage_frame_is_what_the_reading_starts_from() {
        let slot = CompactionSlot::default();
        let mut compactor = AgentCompactor::new(slot, 7);
        let mut messages = conversation(1, 40);
        let uncalibrated = Calibration::default();
        assert_eq!(compactor.size(&messages, uncalibrated), 7 + cost(&messages));
        compactor.observe(5_000, messages.len());
        push_round(&mut messages, 1, 40);
        assert_eq!(
            compactor.size(&messages, uncalibrated),
            5_000 + cost(&messages[messages.len() - 2..]),
            "the provider's count, plus the estimate of what joined"
        );
    }

    #[test]
    fn what_joined_since_the_report_is_read_in_the_providers_tokens() {
        let slot = CompactionSlot::default();
        let mut compactor = AgentCompactor::new(slot, 0);
        let mut messages = conversation(1, 40);
        compactor.observe(5_000, messages.len());
        push_round(&mut messages, 1, 4_000);
        let joined = cost(&messages[messages.len() - 2..]);
        assert_eq!(
            compactor.size(&messages, doubling()),
            5_000 + doubling().correct(joined),
            "a provider counting twice the estimate counts what joined twice"
        );
    }

    #[test]
    fn a_round_the_estimate_undercounts_is_held_at_the_hard_limit() {
        // The provider counts twice what the estimate makes of the newest
        // round: read raw, the next request is under the threshold — sent,
        // it would fill the window. Read through the session's calibration,
        // it is past the hard limit, so the agent compacts first.
        let slot = CompactionSlot::default();
        let mut messages = conversation(1, 40);
        let mut compactor = AgentCompactor::new(slot.clone(), 0);
        compactor.observe(1_000, messages.len());
        push_round(&mut messages, 1, 8_000);
        let joined = cost(&messages[messages.len() - 2..]);
        let window = 1_000 + 2 * joined;
        assert!(
            1_000 + joined < soft_threshold(window),
            "raw, it looks small"
        );
        slot.set_policy(Some(CompactionPolicy {
            calibration: doubling(),
            ..policy(window)
        }));
        let (tx, rx) = channel();
        let cancel = CancelToken::new();
        let worker = answer_the_wait(slot.clone(), rx, cancel.clone(), |slot, id| {
            slot.publish(Handoff {
                id,
                head: head(),
                head_tokens: 30,
            });
        });
        assert_eq!(
            compactor.before_round(&mut messages, &tx, &cancel),
            RoundTop::Go
        );
        drop(tx);
        let events = worker.join().expect("the boundary stand-in");
        assert!(
            matches!(
                events.as_slice(),
                [
                    StreamEvent::CompactionCheckpoint { .. },
                    StreamEvent::CompactionWait { .. },
                    StreamEvent::CompactionApplied { .. },
                ]
            ),
            "{events:?}"
        );
    }

    /// How many waits the boundary's stand-in answers before it presses Esc
    /// instead — so an agent that loops on its waits fails a test rather
    /// than hanging it.
    const ANSWERED_WAITS: usize = 3;

    /// The boundary's stand-in on another thread: `answer` each time the
    /// agent says it waits (Esc after [`ANSWERED_WAITS`]), and hand back
    /// every event it saw.
    fn answer_the_wait(
        slot: CompactionSlot,
        mut rx: UnboundedReceiver<StreamEvent>,
        esc: CancelToken,
        answer: impl Fn(&CompactionSlot, u64) + Send + 'static,
    ) -> std::thread::JoinHandle<Vec<StreamEvent>> {
        std::thread::spawn(move || {
            let mut waits = 0;
            let mut seen = Vec::new();
            while let Some(event) = rx.blocking_recv() {
                if let StreamEvent::CompactionWait { id } = &event {
                    waits += 1;
                    if waits > ANSWERED_WAITS {
                        esc.cancel();
                    } else {
                        answer(&slot, *id);
                    }
                }
                seen.push(event);
            }
            seen
        })
    }

    #[test]
    fn at_the_hard_limit_the_agent_waits_for_the_summary_in_flight() {
        let slot = CompactionSlot::default();
        let mut compactor = AgentCompactor::new(slot.clone(), 0);
        let (tx, rx) = channel();
        let cancel = CancelToken::new();
        let mut messages = conversation(2, 400);
        slot.request();
        compactor.before_round(&mut messages, &tx, &cancel);
        // The rounds since made it overflow before the summary landed.
        push_round(&mut messages, 2, 4_000);
        slot.set_policy(Some(policy(cost(&messages) + 10)));
        let worker = answer_the_wait(slot.clone(), rx, cancel.clone(), |slot, id| {
            slot.publish(Handoff {
                id,
                head: head(),
                head_tokens: 30,
            });
        });
        assert_eq!(
            compactor.before_round(&mut messages, &tx, &cancel),
            RoundTop::Go
        );
        drop(tx);
        let events = worker.join().expect("the boundary stand-in");
        assert!(
            matches!(
                events.as_slice(),
                [
                    StreamEvent::CompactionCheckpoint { .. },
                    StreamEvent::CompactionWait { .. },
                    StreamEvent::CompactionApplied { .. },
                ]
            ),
            "{events:?}"
        );
        assert_eq!(
            messages[1],
            head()[0],
            "spliced before the request went out"
        );
    }

    #[test]
    fn at_the_hard_limit_with_nothing_in_flight_a_compaction_is_forced() {
        let slot = CompactionSlot::default();
        let mut messages = conversation(3, 4_000);
        slot.set_policy(Some(CompactionPolicy {
            auto: false,
            ..policy(cost(&messages) + 10)
        }));
        let mut compactor = AgentCompactor::new(slot.clone(), 0);
        let (tx, rx) = channel();
        let cancel = CancelToken::new();
        let worker = answer_the_wait(slot.clone(), rx, cancel.clone(), |slot, id| {
            slot.publish(Handoff {
                id,
                head: head(),
                head_tokens: 30,
            });
        });
        assert_eq!(
            compactor.before_round(&mut messages, &tx, &cancel),
            RoundTop::Go
        );
        drop(tx);
        let events = worker.join().expect("the boundary stand-in");
        assert!(
            matches!(
                events.as_slice(),
                [
                    StreamEvent::CompactionCheckpoint { .. },
                    StreamEvent::CompactionWait { .. },
                    StreamEvent::CompactionApplied { .. },
                ]
            ),
            "{events:?}"
        );
        assert_eq!(
            messages.len(),
            2,
            "[system] + head, nothing after the snapshot"
        );
    }

    #[test]
    fn esc_while_waiting_stops_the_turn() {
        let slot = CompactionSlot::default();
        let mut messages = conversation(3, 4_000);
        slot.set_policy(Some(policy(cost(&messages) + 10)));
        let mut compactor = AgentCompactor::new(slot.clone(), 0);
        let (tx, rx) = channel();
        let cancel = CancelToken::new();
        let esc = cancel.clone();
        let worker = answer_the_wait(slot, rx, cancel.clone(), move |_, _| esc.cancel());
        let started = std::time::Instant::now();
        assert_eq!(
            compactor.before_round(&mut messages, &tx, &cancel),
            RoundTop::Stop
        );
        assert!(started.elapsed() < Duration::from_secs(2), "Esc is prompt");
        drop(tx);
        worker.join().expect("the boundary stand-in");
    }

    #[test]
    fn a_failed_summary_lets_a_turn_that_fits_go_on() {
        let slot = CompactionSlot::default();
        let mut messages = conversation(3, 4_000);
        let size = cost(&messages);
        // Past the hard limit, still inside the window.
        slot.set_policy(Some(policy(size * 100 / 96)));
        let mut compactor = AgentCompactor::new(slot.clone(), 0);
        let (tx, rx) = channel();
        let cancel = CancelToken::new();
        let worker = answer_the_wait(slot, rx, cancel.clone(), |slot, id| slot.fail(id));
        assert_eq!(
            compactor.before_round(&mut messages, &tx, &cancel),
            RoundTop::Go
        );
        drop(tx);
        let events = worker.join().expect("the boundary stand-in");
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::CompactionCheckpoint { .. }))
                .count(),
            1,
            "one attempt over one snapshot, never a loop: {events:?}"
        );
    }

    #[test]
    fn nothing_is_compacted_before_a_round_has_run() {
        // A first request too big for the window is the user's message and
        // the fixed overhead: there is no round to fold into a summary.
        let slot = CompactionSlot::default();
        let mut messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("x".repeat(40_000)),
        ];
        slot.set_policy(Some(policy(cost(&messages) / 2)));
        slot.request();
        let mut compactor = AgentCompactor::new(slot, 0);
        let (tx, mut rx) = channel();
        assert_eq!(
            compactor.before_round(&mut messages, &tx, &CancelToken::new()),
            RoundTop::Go,
            "sent for the provider to judge"
        );
        assert!(drain(&mut rx).is_empty(), "{:?}", drain(&mut rx));
    }

    #[test]
    fn an_estimate_no_provider_has_counted_is_never_refused() {
        // Past the window by the estimate alone — before any usage frame —
        // the request goes out: the provider's own count decides.
        let slot = CompactionSlot::default();
        let mut messages = conversation(3, 4_000);
        slot.set_policy(Some(CompactionPolicy {
            floor: Some(u64::MAX),
            ..policy(cost(&messages) - 10)
        }));
        let mut compactor = AgentCompactor::new(slot, 0);
        let (tx, _rx) = channel();
        assert_eq!(
            compactor.before_round(&mut messages, &tx, &CancelToken::new()),
            RoundTop::Go
        );
    }

    #[test]
    fn a_failed_summary_ends_a_turn_that_cannot_fit() {
        let slot = CompactionSlot::default();
        let mut messages = conversation(3, 4_000);
        slot.set_policy(Some(policy(cost(&messages) - 10)));
        let mut compactor = AgentCompactor::new(slot.clone(), 0);
        // The provider counted what the request before this one carried.
        compactor.observe(cost(&messages[..4]), 4);
        let (tx, rx) = channel();
        let cancel = CancelToken::new();
        let worker = answer_the_wait(slot, rx, cancel.clone(), |slot, id| slot.fail(id));
        let outcome = compactor.before_round(&mut messages, &tx, &cancel);
        drop(tx);
        worker.join().expect("the boundary stand-in");
        let RoundTop::Fail(message) = outcome else {
            panic!("the turn ends: {outcome:?}");
        };
        assert!(message.contains("context window"), "{message}");
    }

    #[test]
    fn after_a_failure_a_new_checkpoint_waits_for_something_new() {
        let slot = CompactionSlot::default();
        let mut messages = conversation(3, 400);
        let window = cost(&messages) * 100 / 92;
        slot.set_policy(Some(policy(window)));
        let mut compactor = AgentCompactor::new(slot.clone(), 0);
        let (tx, mut rx) = channel();
        let cancel = CancelToken::new();
        compactor.before_round(&mut messages, &tx, &cancel);
        let id = checkpoint_id(&drain(&mut rx));
        slot.fail(id);
        compactor.before_round(&mut messages, &tx, &cancel);
        assert!(
            drain(&mut rx).is_empty(),
            "the same snapshot is not retried"
        );
        push_round(&mut messages, 3, 10);
        compactor.before_round(&mut messages, &tx, &cancel);
        assert!(
            matches!(
                drain(&mut rx).as_slice(),
                [StreamEvent::CompactionCheckpoint { .. }]
            ),
            "something new: a later retry"
        );
    }

    #[test]
    fn a_compaction_that_cannot_get_under_the_threshold_is_not_repeated() {
        let slot = CompactionSlot::default();
        let mut messages = conversation(3, 400);
        let window = cost(&messages) * 100 / 92;
        slot.set_policy(Some(policy(window)));
        let mut compactor = AgentCompactor::new(slot.clone(), 0);
        let (tx, mut rx) = channel();
        let cancel = CancelToken::new();
        compactor.before_round(&mut messages, &tx, &cancel);
        let id = checkpoint_id(&drain(&mut rx));
        // The head alone is past the threshold: compacting cannot help.
        slot.publish(Handoff {
            id,
            head: head(),
            head_tokens: window,
        });
        compactor.before_round(&mut messages, &tx, &cancel);
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [StreamEvent::CompactionApplied { .. }]
        ));
        push_round(&mut messages, 3, 10);
        compactor.before_round(&mut messages, &tx, &cancel);
        assert!(
            drain(&mut rx).is_empty(),
            "no second compaction over a head that is itself too big"
        );
    }

    #[test]
    fn a_policy_floor_past_the_threshold_keeps_the_agent_from_trying() {
        let slot = CompactionSlot::default();
        let mut messages = conversation(3, 400);
        let window = cost(&messages) * 100 / 92;
        slot.set_policy(Some(CompactionPolicy {
            floor: Some(window),
            ..policy(window)
        }));
        let mut compactor = AgentCompactor::new(slot, 0);
        let (tx, mut rx) = channel();
        compactor.before_round(&mut messages, &tx, &CancelToken::new());
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn several_compactions_follow_one_another_in_one_turn() {
        let slot = CompactionSlot::default();
        let mut compactor = AgentCompactor::new(slot.clone(), 0);
        let (tx, mut rx) = channel();
        let cancel = CancelToken::new();
        let mut messages = conversation(2, 40);
        for round in 0..3 {
            slot.request();
            compactor.before_round(&mut messages, &tx, &cancel);
            let id = checkpoint_id(&drain(&mut rx));
            push_round(&mut messages, 10 + round, 40);
            slot.publish(Handoff {
                id,
                head: vec![ChatMessage::user(format!("summary {round}"))],
                head_tokens: 30,
            });
            compactor.before_round(&mut messages, &tx, &cancel);
            assert_eq!(drain(&mut rx), vec![StreamEvent::CompactionApplied { id }]);
            assert_eq!(messages[1], ChatMessage::user(format!("summary {round}")));
            assert_eq!(messages.len(), 4, "[system] + head + the one round since");
        }
    }

    #[test]
    fn a_wire_message_costs_its_text_its_pictures_and_its_calls() {
        let text = ChatMessage::user("hello there");
        assert_eq!(
            message_tokens(&text),
            u64::try_from(crate::tokens::estimate("hello there")).unwrap()
        );
        let call = ChatMessage::assistant_tool_calls(
            "",
            vec![ToolCallSpec::function("id", "bash", r#"{"command":"ls"}"#)],
        );
        assert!(message_tokens(&call) > 0);
        let picture = ChatMessage::with_parts(
            "user",
            vec![
                ContentPart::text("look"),
                ContentPart::image("data:image/png;base64,AAAA"),
            ],
        );
        assert!(message_tokens(&picture) >= 256);
    }
}
