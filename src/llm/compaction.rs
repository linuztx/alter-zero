//! Background compaction (`docs/background-compaction.md`): a summary of the
//! conversation written on a thread of its own while the agent works, held
//! until the request would reach the window's ceiling, then spliced in at a
//! round boundary with every round since its cut kept as it was.
//!
//! The pure half lives here first: what happens at a round top
//! ([`decide`]), what the summarizer reads ([`summarizer_messages`]), what
//! the splice builds ([`spliced_messages`]) and the measure between usage
//! frames ([`estimate_tokens`]).

//!
//! The stateful half: the [`CompactionSlot`] the session, the backend thread
//! and the summarizer thread share, and the [`BackgroundCheckpoint`] the
//! main turn's agent loop asks at every round top.

use std::cell::{Cell, RefCell};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use crate::app::{IMAGE_INPUT_TOKENS, auto_compact_threshold, compaction_start};
use crate::context::{
    COMPACT_SYSTEM_PROMPT, COMPACT_USER_MESSAGE_MAX_TOKENS, MERGE_SEPARATOR, SUMMARIZATION_PROMPT,
    budget_texts, summary_bridge,
};
use crate::llm::agent::RoundCheckpoint;
use crate::llm::{ChatMessage, ContentPart, MessageContent};
use crate::stream::{CancelToken, StreamEvent, TokenUsage};

/// How often a round top waiting on a summary looks at the turn's token —
/// the permission gate's cadence, so an Esc stops the wait at once.
const WAIT_POLL: Duration = Duration::from_millis(20);

/// How far past its cut a held summary's tail may grow, in percent of the
/// ceiling, before it is spliced in rather than held: past it the summary
/// would describe ever less of what it replaces.
const TAIL_PERCENT: u64 = 40;

/// What the session's summary slot holds for this turn at a round top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// No summary for this cycle.
    Nothing,
    /// One is being written; it was started at `at` tokens.
    Running { at: u64 },
    /// One is written and waiting; it was started at `at` tokens.
    Ready { at: u64 },
    /// This cycle's summary failed; the ceiling compacts inline.
    Failed,
    /// A summary an earlier turn started is still being written: it lands at
    /// idle, and this turn starts none of its own.
    Elsewhere,
}

/// What a round top does about compaction (`docs/background-compaction.md`
/// *What happens at a round top*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Send the request as it is.
    Nothing,
    /// Note the cut and start a summary in the background.
    Start,
    /// Splice the finished summary in now.
    Splice,
    /// The ceiling came first: wait for the summary being written, then
    /// splice it in.
    Wait,
    /// The ceiling with nothing usable: summarize everything now, the agent
    /// waiting.
    Inline,
}

/// The pure decision at a round top: `tokens` the next request would carry,
/// in a `window`-token window whose cycle began at `cycle_start`, with
/// `held` in the slot.
#[must_use]
pub fn decide(tokens: u64, window: u64, cycle_start: u64, held: Held) -> Decision {
    let ceiling = auto_compact_threshold(window);
    let at_ceiling = tokens >= ceiling;
    match held {
        Held::Ready { at } => {
            let tail = tokens.saturating_sub(at);
            if at_ceiling || tail > ceiling.saturating_mul(TAIL_PERCENT) / 100 {
                Decision::Splice
            } else {
                Decision::Nothing
            }
        }
        Held::Running { .. } if at_ceiling => Decision::Wait,
        Held::Failed | Held::Elsewhere | Held::Nothing if at_ceiling => Decision::Inline,
        Held::Nothing if tokens >= compaction_start(window, cycle_start) => Decision::Start,
        Held::Running { .. } | Held::Failed | Held::Elsewhere | Held::Nothing => Decision::Nothing,
    }
}

/// What a summary request carries (`docs/background-compaction.md` *What the
/// summarizer reads*): the summarizer's own system prompt, the conversation
/// in `messages[..len]` with the session's system prompt dropped and
/// `reminder` cut from the front of the first user message, then the
/// summarization prompt.
#[must_use]
pub fn summarizer_messages(
    messages: &[ChatMessage],
    len: usize,
    reminder: Option<&str>,
) -> Vec<ChatMessage> {
    let mut out = vec![ChatMessage::system(COMPACT_SYSTEM_PROMPT)];
    let mut first_user = true;
    for message in messages.iter().take(len) {
        if message.role == "system" {
            continue;
        }
        if first_user && message.role == "user" {
            first_user = false;
            if let Some(reminder) = reminder.filter(|reminder| !reminder.is_empty()) {
                out.extend(without_reminder(message, reminder));
                continue;
            }
        }
        out.push(message.clone());
    }
    out.push(ChatMessage::user(SUMMARIZATION_PROMPT));
    out
}

/// `message` with `reminder` cut from the front of its text (and the merge
/// separator after it) — `None` when nothing else was in it.
fn without_reminder(message: &ChatMessage, reminder: &str) -> Option<ChatMessage> {
    let cut = |text: &str| {
        text.strip_prefix(reminder).map(|rest| {
            rest.strip_prefix(MERGE_SEPARATOR)
                .unwrap_or(rest)
                .to_string()
        })
    };
    let mut out = message.clone();
    match &mut out.content {
        MessageContent::Text(text) => {
            if let Some(rest) = cut(text) {
                if rest.is_empty() {
                    return None;
                }
                *text = rest;
            }
        }
        MessageContent::Parts(parts) => {
            let rest = match parts.first() {
                Some(ContentPart::Text { text, .. }) => cut(text),
                _ => None,
            };
            if let Some(rest) = rest {
                if rest.is_empty() {
                    parts.remove(0);
                } else if let Some(ContentPart::Text { text, .. }) = parts.first_mut() {
                    *text = rest;
                }
            }
            if parts.is_empty() {
                return None;
            }
        }
    }
    Some(out)
}

/// The request after a splice (`docs/background-compaction.md` *What the
/// splice builds*): the session's system prompt, then one user message — the
/// `reminder`, the budgeted `typed` user texts and the summary bridge, joined
/// the way the derivation merges them — then every message from `len` on,
/// as it was. What the derivation of the landed marker sends next turn.
#[must_use]
pub fn spliced_messages(
    messages: &[ChatMessage],
    len: usize,
    reminder: Option<&str>,
    typed: &[String],
    summary: &str,
) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = messages
        .iter()
        .take_while(|message| message.role == "system")
        .cloned()
        .collect();
    let mut head: Vec<String> = reminder
        .filter(|reminder| !reminder.is_empty())
        .map(str::to_string)
        .into_iter()
        .collect();
    head.extend(budget_texts(
        typed.iter().map(String::as_str),
        COMPACT_USER_MESSAGE_MAX_TOKENS,
    ));
    head.push(summary_bridge(summary));
    out.push(ChatMessage::user(head.join(MERGE_SEPARATOR)));
    out.extend(messages.iter().skip(len).cloned());
    out
}

/// The token estimate of `messages` — their text, their tool calls, and the
/// gauge's flat rate per picture: the measure between usage frames, and of a
/// request just spliced.
#[must_use]
pub fn estimate_tokens(messages: &[ChatMessage]) -> u64 {
    let estimate = crate::tokens::estimate;
    let total: usize = messages
        .iter()
        .map(|message| {
            let content = match &message.content {
                MessageContent::Text(text) => estimate(text),
                MessageContent::Parts(parts) => parts
                    .iter()
                    .map(|part| match part {
                        ContentPart::Text { text, .. } => estimate(text),
                        ContentPart::ImageUrl { .. } => IMAGE_INPUT_TOKENS,
                    })
                    .sum(),
            };
            let calls: usize = message
                .tool_calls
                .iter()
                .map(|call| {
                    estimate(&call.function.name) + estimate(call.function.arguments.as_str())
                })
                .sum();
            content + calls
        })
        .sum();
    u64::try_from(total).unwrap_or(u64::MAX)
}

/// A summary written for compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The handoff summary.
    pub text: String,
    /// The usage frames its requests reported: billed to the session, never
    /// a measure of the conversation's own requests.
    pub usage: Vec<TokenUsage>,
    /// The `PreCompact`/`PostCompact` cells its writing left
    /// ([`StreamEvent::HookNote`]s), landed above the marker's cell.
    pub notes: Vec<StreamEvent>,
}

/// What writes a summary: the session's tools-free compact client in the
/// app, a fake in the tests.
pub trait Summarize: Send + Sync {
    /// Write the summary a summary request ([`summarizer_messages`]) asks
    /// for, polling `cancel`.
    ///
    /// # Errors
    ///
    /// Why no summary came back — the request failed, or `cancel` fired.
    fn summarize(
        &self,
        messages: Vec<ChatMessage>,
        cancel: &CancelToken,
    ) -> Result<Written, String>;
}

/// What the app hands each turn before it starts
/// (`docs/background-compaction.md`): the numbers the turn's round tops
/// decide with, and what the splice needs to build exactly what the
/// derivation of the landed marker will.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Baseline {
    /// The model's context window — `None` (unknown) compacts nothing.
    pub window: Option<u64>,
    /// The footer gauge as the turn starts: the measure before the turn's
    /// first usage frame.
    pub used: u64,
    /// What the last compaction left (its marker's `after`), or 0.
    pub cycle_start: u64,
    /// What rides every request but no message — the tool schemas'
    /// estimate — added to a spliced request's own.
    pub overhead: u64,
    /// The exact `<system-reminder>` the turn's context opens with.
    pub reminder: Option<String>,
    /// The typed user texts the splice's budget walk can reach
    /// ([`crate::context::recent_typed_texts`]).
    pub typed: Vec<String>,
}

/// The newest measure of the conversation: `tokens` counted for the request
/// whose reply ends `messages[..len]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measured {
    /// The provider's count (or, after a splice, the estimate).
    pub tokens: u64,
    /// How many messages it counted.
    pub len: usize,
}

/// A finished summary the app lands at idle
/// ([`CompactionSlot::take_landable`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    /// The summary.
    pub written: Written,
    /// How long it took to write, in whole seconds.
    pub secs: u64,
}

/// The session's summary slot (`docs/background-compaction.md` *A summary
/// that outlives its turn*): at most one summary, owned by the session
/// rather than the turn that started it. Every clone is the same slot — the
/// boundary keeps one and hands one to each backend build, as with the
/// steering queue.
#[derive(Debug, Clone, Default)]
pub struct CompactionSlot {
    inner: Arc<(Mutex<Slot>, Condvar)>,
}

#[derive(Debug, Default)]
struct Slot {
    /// The turn the backend runs now, bumped by every [`CompactionSlot::open_turn`].
    turn: u64,
    /// Bumped by every discard: a summary of an older generation describes a
    /// history that is gone.
    generation: u64,
    baseline: Baseline,
    job: Option<Job>,
    /// Told when a summary finishes ([`CompactionSlot::set_waker`]).
    waker: Option<UnboundedSender<()>>,
}

#[derive(Debug)]
struct Job {
    turn: u64,
    generation: u64,
    cut: Cut,
    started: Instant,
    cancel: CancelToken,
    state: JobState,
}

/// Where a summary cut the conversation, and what its splice needs.
#[derive(Debug, Clone)]
struct Cut {
    /// The messages the summary describes: `messages[..len]`.
    len: usize,
    /// The tokens counted when it started.
    at: u64,
    /// The typed user texts before the cut.
    typed: Vec<String>,
    reminder: Option<String>,
}

#[derive(Debug)]
enum JobState {
    Running,
    Ready(Written, u64),
    Failed,
}

impl CompactionSlot {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.inner.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The boundary, once: what to wake when a summary finishes, since one
    /// that outlives its turn may finish while the app sits idle with
    /// nothing else to wake it.
    pub fn set_waker(&self, waker: UnboundedSender<()>) {
        self.lock().waker = Some(waker);
    }

    /// The boundary, before every turn: what the turn's round tops decide
    /// with.
    pub fn set_baseline(&self, baseline: Baseline) {
        self.lock().baseline = baseline;
    }

    /// A turn starts: its id, and the baseline it was handed.
    pub fn open_turn(&self) -> (u64, Baseline) {
        let mut slot = self.lock();
        slot.turn += 1;
        (slot.turn, slot.baseline.clone())
    }

    /// What the slot holds, seen from `turn`.
    #[must_use]
    pub fn held(&self, turn: u64) -> Held {
        let slot = self.lock();
        match &slot.job {
            None => Held::Nothing,
            Some(job) if job.turn != turn => match job.state {
                JobState::Failed => Held::Nothing,
                JobState::Running | JobState::Ready(..) => Held::Elsewhere,
            },
            Some(job) => match job.state {
                JobState::Running => Held::Running { at: job.cut.at },
                JobState::Ready(..) => Held::Ready { at: job.cut.at },
                JobState::Failed => Held::Failed,
            },
        }
    }

    /// Whether a summary is being written or waits to land — the idle
    /// summarization turn holds off while one is.
    #[must_use]
    pub fn in_flight(&self) -> bool {
        matches!(
            self.lock().job,
            Some(Job {
                state: JobState::Running | JobState::Ready(..),
                ..
            })
        )
    }

    /// The boundary, idle or about to start a turn: take a finished summary
    /// to land. A failed one is let go.
    pub fn take_landable(&self) -> Option<Landing> {
        let mut slot = self.lock();
        match slot.job.as_ref().map(|job| &job.state) {
            Some(JobState::Ready(..)) => match slot.job.take()?.state {
                JobState::Ready(written, secs) => Some(Landing { written, secs }),
                JobState::Running | JobState::Failed => None,
            },
            Some(JobState::Failed) => {
                slot.job = None;
                None
            }
            Some(JobState::Running) | None => None,
        }
    }

    /// The history the summary describes is gone (`/clear`, `/resume`, a
    /// rewind), a `/compact` by hand replaces it, or the session ends: stop
    /// it, and never land it.
    pub fn discard(&self) {
        let mut slot = self.lock();
        if let Some(job) = slot.job.take() {
            job.cancel.cancel();
        }
        slot.generation += 1;
        drop(slot);
        self.inner.1.notify_all();
    }

    /// Hold a new summary for `turn`, cut at `cut`; its generation.
    fn start(&self, turn: u64, cut: Cut, cancel: CancelToken) -> u64 {
        let mut slot = self.lock();
        if let Some(old) = slot.job.take() {
            old.cancel.cancel();
        }
        let generation = slot.generation;
        slot.job = Some(Job {
            turn,
            generation,
            cut,
            started: Instant::now(),
            cancel,
            state: JobState::Running,
        });
        generation
    }

    /// The summarizer thread's report — kept only while the slot still holds
    /// the summary it was writing.
    fn finish(&self, generation: u64, turn: u64, result: Result<Written, String>) {
        let mut slot = self.lock();
        if let Some(job) = slot.job.as_mut().filter(|job| {
            job.generation == generation
                && job.turn == turn
                && matches!(job.state, JobState::Running)
        }) {
            job.state = match result {
                Ok(written) => JobState::Ready(written, job.started.elapsed().as_secs()),
                Err(_) => JobState::Failed,
            };
            if let Some(waker) = &slot.waker {
                let _ = waker.send(());
            }
        }
        drop(slot);
        self.inner.1.notify_all();
    }

    /// Take `turn`'s finished summary.
    fn take_ready(&self, turn: u64) -> Option<(Cut, Written, u64)> {
        let mut slot = self.lock();
        if !matches!(&slot.job, Some(job) if job.turn == turn && matches!(job.state, JobState::Ready(..)))
        {
            return None;
        }
        let job = slot.job.take()?;
        match job.state {
            JobState::Ready(written, secs) => Some((job.cut, written, secs)),
            JobState::Running | JobState::Failed => None,
        }
    }

    /// Wait for `turn`'s summary to be written, polling `cancel`: `None` on
    /// a cancel, `Some(Err(()))` when it failed or was discarded.
    fn wait(&self, turn: u64, cancel: &CancelToken) -> Option<Result<(Cut, Written, u64), ()>> {
        let (lock, written) = &*self.inner;
        let mut slot = lock.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if cancel.is_cancelled() {
                return None;
            }
            match slot
                .job
                .as_ref()
                .filter(|job| job.turn == turn)
                .map(|job| &job.state)
            {
                Some(JobState::Running) => {}
                Some(JobState::Ready(..)) => {
                    let job = slot.job.take()?;
                    return match job.state {
                        JobState::Ready(text, secs) => Some(Ok((job.cut, text, secs))),
                        JobState::Running | JobState::Failed => Some(Err(())),
                    };
                }
                Some(JobState::Failed) => {
                    slot.job = None;
                    return Some(Err(()));
                }
                None => return Some(Err(())),
            }
            slot = written
                .wait_timeout(slot, WAIT_POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// A compaction landed: whatever the slot still holds — the turn's own
    /// failed summary, or another turn's — describes the history before it,
    /// so it is let go (a running one cancelled) and the next cycle may start
    /// its own.
    fn settle(&self) {
        if let Some(job) = self.lock().job.take() {
            job.cancel.cancel();
        }
    }
}

/// The main turn's round checkpoint (`docs/background-compaction.md` *What
/// happens at a round top*): it measures the next request, asks [`decide`],
/// and starts, holds, splices, waits for or writes inline the summary.
pub struct BackgroundCheckpoint<'a> {
    slot: CompactionSlot,
    summarizer: Arc<dyn Summarize>,
    turn: u64,
    baseline: Baseline,
    window: u64,
    cycle_start: u64,
    /// The newest measure — the round closure sets it after every request.
    measured: &'a Cell<Option<Measured>>,
    /// The messages the user steered into this turn, in order — typed texts
    /// the splice keeps, like the baseline's.
    steered: &'a RefCell<Vec<String>>,
    /// Compaction cannot help this turn: an inline summary failed, or left
    /// the request at the ceiling still — what rides every request alone
    /// passes it. Summarizing again each round would only pay for a summary
    /// and take the newest results away from the model.
    spent: bool,
}

impl<'a> BackgroundCheckpoint<'a> {
    /// The checkpoint for `turn` — `None` when the window is unknown, since
    /// there is no ceiling to act at.
    #[must_use]
    pub fn new(
        slot: CompactionSlot,
        summarizer: Arc<dyn Summarize>,
        turn: u64,
        baseline: Baseline,
        measured: &'a Cell<Option<Measured>>,
        steered: &'a RefCell<Vec<String>>,
    ) -> Option<Self> {
        let window = baseline.window.filter(|&window| window > 0)?;
        Some(Self {
            slot,
            summarizer,
            turn,
            cycle_start: baseline.cycle_start,
            baseline,
            window,
            measured,
            steered,
            spent: false,
        })
    }

    /// What the next request would carry: the newest measure plus the
    /// estimate of what joined after it — the measure anchored on the gauge
    /// at the first round top ([`RoundCheckpoint::before_request`]).
    fn measure(&self, messages: &[ChatMessage]) -> u64 {
        match self.measured.get() {
            Some(measured) => measured.tokens.saturating_add(estimate_tokens(
                messages.get(measured.len..).unwrap_or_default(),
            )),
            None => self.baseline.used,
        }
    }

    fn cut(&self, messages: &[ChatMessage], tokens: u64) -> Cut {
        let mut typed = self.baseline.typed.clone();
        typed.extend(self.steered.borrow().iter().cloned());
        Cut {
            len: messages.len(),
            at: tokens,
            typed,
            reminder: self.baseline.reminder.clone(),
        }
    }

    /// Cut here and write the summary on a thread of its own, with its own
    /// token: an Esc does not stop it — it lands at idle.
    fn start(&self, messages: &[ChatMessage], tokens: u64, tx: &UnboundedSender<StreamEvent>) {
        let cut = self.cut(messages, tokens);
        let input = summarizer_messages(messages, cut.len, cut.reminder.as_deref());
        let cancel = CancelToken::new();
        let generation = self.slot.start(self.turn, cut, cancel.clone());
        let _ = tx.send(StreamEvent::CompactionStarted);
        let (slot, summarizer, turn) = (self.slot.clone(), Arc::clone(&self.summarizer), self.turn);
        std::thread::spawn(move || {
            let result = summarizer.summarize(input, &cancel);
            slot.finish(generation, turn, result);
        });
    }

    /// Splice `written` over `messages[..cut.len]`, tell the app, and measure
    /// what the request carries now.
    fn splice(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        cut: &Cut,
        written: Written,
        secs: u64,
        background: bool,
        tx: &UnboundedSender<StreamEvent>,
    ) {
        *messages = spliced_messages(
            messages,
            cut.len,
            cut.reminder.as_deref(),
            &cut.typed,
            &written.text,
        );
        for note in written.notes {
            let _ = tx.send(note);
        }
        let _ = tx.send(StreamEvent::Compacted {
            summary: written.text,
            secs,
            background,
            usage: written.usage,
        });
        let after = estimate_tokens(messages).saturating_add(self.baseline.overhead);
        self.measured.set(Some(Measured {
            tokens: after,
            len: messages.len(),
        }));
        self.cycle_start = after;
        self.slot.settle();
    }

    /// Summarize everything now, the agent waiting.
    fn inline(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        tx: &UnboundedSender<StreamEvent>,
        cancel: &CancelToken,
    ) {
        let cut = self.cut(messages, self.measure(messages));
        let _ = tx.send(StreamEvent::CompactionStarted);
        let _ = tx.send(StreamEvent::CompactionWaiting);
        let started = Instant::now();
        let input = summarizer_messages(messages, cut.len, cut.reminder.as_deref());
        match self.summarizer.summarize(input, cancel) {
            Ok(written) => {
                let secs = started.elapsed().as_secs();
                self.splice(messages, &cut, written, secs, false, tx);
                self.spent = self.measure(messages) >= auto_compact_threshold(self.window);
            }
            Err(_) if cancel.is_cancelled() => {}
            Err(reason) => {
                let _ = tx.send(StreamEvent::CompactionFailed { reason });
                self.spent = true;
            }
        }
    }

    /// After a splice: a request still at the ceiling (a tail of huge reads)
    /// compacts everything once more, inline.
    fn settle_ceiling(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        tx: &UnboundedSender<StreamEvent>,
        cancel: &CancelToken,
    ) {
        if self.measure(messages) >= auto_compact_threshold(self.window) {
            self.inline(messages, tx, cancel);
        }
    }
}

impl RoundCheckpoint for BackgroundCheckpoint<'_> {
    fn before_request(
        &mut self,
        messages: &mut Vec<ChatMessage>,
        tx: &UnboundedSender<StreamEvent>,
        cancel: &CancelToken,
    ) {
        // Before the turn's first usage frame — and for a provider that
        // reports none — the gauge it started from counts what the first
        // request carries, and every message after is estimated on top.
        if self.measured.get().is_none() {
            self.measured.set(Some(Measured {
                tokens: self.baseline.used,
                len: messages.len(),
            }));
        }
        if self.spent {
            return;
        }
        let tokens = self.measure(messages);
        match decide(
            tokens,
            self.window,
            self.cycle_start,
            self.slot.held(self.turn),
        ) {
            Decision::Nothing => {}
            Decision::Start => self.start(messages, tokens, tx),
            Decision::Splice => {
                if let Some((cut, written, secs)) = self.slot.take_ready(self.turn) {
                    self.splice(messages, &cut, written, secs, true, tx);
                    self.settle_ceiling(messages, tx, cancel);
                }
            }
            Decision::Wait => {
                let _ = tx.send(StreamEvent::CompactionWaiting);
                match self.slot.wait(self.turn, cancel) {
                    None => {}
                    Some(Ok((cut, written, secs))) => {
                        self.splice(messages, &cut, written, secs, true, tx);
                        self.settle_ceiling(messages, tx, cancel);
                    }
                    Some(Err(())) => self.inline(messages, tx, cancel),
                }
            }
            Decision::Inline => self.inline(messages, tx, cancel),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{COMPACT_SYSTEM_PROMPT, SUMMARIZATION_PROMPT, summary_bridge};
    use crate::llm::{ChatMessage, MessageContent};

    /// A 200k window: ceiling 170k, a cold cycle's summary starting at 110k.
    const WINDOW: u64 = 200_000;

    fn text_of(message: &ChatMessage) -> String {
        match &message.content {
            MessageContent::Text(text) => text.clone(),
            MessageContent::Parts(_) => String::new(),
        }
    }

    // --- what happens at a round top ---

    #[test]
    fn nothing_happens_below_the_start() {
        assert_eq!(decide(100_000, WINDOW, 0, Held::Nothing), Decision::Nothing);
    }

    #[test]
    fn a_summary_starts_at_the_start() {
        assert_eq!(decide(110_000, WINDOW, 0, Held::Nothing), Decision::Start);
    }

    #[test]
    fn the_start_follows_the_cycle() {
        // A compaction that left 140k has 30k of room: start halfway.
        assert_eq!(
            decide(150_000, WINDOW, 140_000, Held::Nothing),
            Decision::Nothing
        );
        assert_eq!(
            decide(155_000, WINDOW, 140_000, Held::Nothing),
            Decision::Start
        );
    }

    #[test]
    fn a_summary_being_written_is_left_alone_below_the_ceiling() {
        let held = Held::Running { at: 110_000 };
        assert_eq!(decide(150_000, WINDOW, 0, held), Decision::Nothing);
    }

    #[test]
    fn a_finished_summary_is_held_below_the_ceiling() {
        let held = Held::Ready { at: 110_000 };
        assert_eq!(decide(150_000, WINDOW, 0, held), Decision::Nothing);
    }

    #[test]
    fn a_finished_summary_is_spliced_in_at_the_ceiling() {
        let held = Held::Ready { at: 110_000 };
        assert_eq!(decide(170_000, WINDOW, 0, held), Decision::Splice);
    }

    #[test]
    fn a_finished_summary_falling_behind_is_spliced_in_early() {
        // The tail limit is 40% of the 170k ceiling, 68k: past it, a summary
        // held any longer would describe ever less of what it replaces.
        let held = Held::Ready { at: 100_000 };
        assert_eq!(decide(168_000, WINDOW, 0, held), Decision::Nothing);
        assert_eq!(decide(169_000, WINDOW, 0, held), Decision::Splice);
    }

    #[test]
    fn the_ceiling_waits_for_a_summary_still_being_written() {
        let held = Held::Running { at: 110_000 };
        assert_eq!(decide(175_000, WINDOW, 0, held), Decision::Wait);
    }

    #[test]
    fn the_ceiling_compacts_inline_with_nothing_usable() {
        for held in [Held::Nothing, Held::Failed, Held::Elsewhere] {
            assert_eq!(
                decide(175_000, WINDOW, 0, held),
                Decision::Inline,
                "{held:?}"
            );
        }
    }

    #[test]
    fn a_failed_summary_is_not_tried_again_before_the_ceiling() {
        assert_eq!(decide(150_000, WINDOW, 0, Held::Failed), Decision::Nothing);
    }

    #[test]
    fn another_turns_summary_keeps_this_one_from_starting_its_own() {
        assert_eq!(
            decide(150_000, WINDOW, 0, Held::Elsewhere),
            Decision::Nothing
        );
    }

    // --- what the summarizer reads ---

    fn reminder() -> String {
        crate::reminder::reminder_message(&["Run the linter before committing."])
    }

    #[test]
    fn the_summarizer_reads_the_conversation_up_to_the_cut_then_the_prompt() {
        let reminder = reminder();
        let messages = vec![
            ChatMessage::system("You are Alter Zero"),
            ChatMessage::user(format!("{reminder}\n\nfix the parser")),
            ChatMessage::assistant("reading it"),
            ChatMessage::user("said after the cut"),
        ];
        assert_eq!(
            summarizer_messages(&messages, 3, Some(&reminder)),
            vec![
                ChatMessage::system(COMPACT_SYSTEM_PROMPT),
                ChatMessage::user("fix the parser"),
                ChatMessage::assistant("reading it"),
                ChatMessage::user(SUMMARIZATION_PROMPT),
            ]
        );
    }

    #[test]
    fn a_reminder_alone_in_its_message_leaves_nothing_behind() {
        let reminder = reminder();
        let messages = vec![
            ChatMessage::system("You are Alter Zero"),
            ChatMessage::user(reminder.clone()),
            ChatMessage::assistant("reading it"),
        ];
        let input = summarizer_messages(&messages, 3, Some(&reminder));
        assert_eq!(input.len(), 3, "{input:?}");
        assert_eq!(input[1], ChatMessage::assistant("reading it"));
    }

    #[test]
    fn without_a_reminder_only_the_system_prompt_changes() {
        let messages = vec![
            ChatMessage::system("You are Alter Zero"),
            ChatMessage::user("fix the parser"),
        ];
        let input = summarizer_messages(&messages, 2, None);
        assert_eq!(input[0], ChatMessage::system(COMPACT_SYSTEM_PROMPT));
        assert_eq!(input[1], ChatMessage::user("fix the parser"));
    }

    // --- what the splice builds ---

    #[test]
    fn the_splice_keeps_the_system_prompt_and_the_tail_and_bridges_the_rest() {
        let reminder = reminder();
        let messages = vec![
            ChatMessage::system("You are Alter Zero"),
            ChatMessage::user(format!("{reminder}\n\nfix the parser")),
            ChatMessage::assistant("reading it"),
            ChatMessage::assistant("a round after the cut"),
            ChatMessage::tool_result("c1", "its output"),
        ];
        let typed = vec!["fix the parser".to_string()];
        assert_eq!(
            spliced_messages(&messages, 3, Some(&reminder), &typed, "we read the parser"),
            vec![
                ChatMessage::system("You are Alter Zero"),
                ChatMessage::user(format!(
                    "{reminder}\n\nfix the parser\n\n{}",
                    summary_bridge("we read the parser")
                )),
                ChatMessage::assistant("a round after the cut"),
                ChatMessage::tool_result("c1", "its output"),
            ]
        );
    }

    #[test]
    fn a_splice_without_a_reminder_opens_on_the_user_texts() {
        let messages = vec![ChatMessage::user("fix the parser")];
        let typed = vec!["fix the parser".to_string()];
        let spliced = spliced_messages(&messages, 1, None, &typed, "s");
        assert_eq!(
            text_of(&spliced[0]),
            format!("fix the parser\n\n{}", summary_bridge("s"))
        );
    }

    // --- the checkpoint (docs/background-compaction.md *What happens at a round top*) ---

    use crate::llm::agent::RoundCheckpoint;
    use crate::stream::{CancelToken, StreamEvent};
    use std::cell::{Cell, RefCell};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    /// A summarizer the test answers: it reads what it was given and waits for
    /// the test to send the outcome, polling the token like the real one.
    struct Fake {
        outcomes: Mutex<mpsc::Receiver<Result<Written, String>>>,
        read: Mutex<Vec<Vec<ChatMessage>>>,
    }

    impl Summarize for Fake {
        fn summarize(
            &self,
            messages: Vec<ChatMessage>,
            cancel: &CancelToken,
        ) -> Result<Written, String> {
            self.read.lock().unwrap().push(messages);
            let outcomes = self.outcomes.lock().unwrap();
            loop {
                match outcomes.recv_timeout(Duration::from_millis(5)) {
                    Ok(outcome) => return outcome,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Err("gone".into()),
                    Err(_) if cancel.is_cancelled() => return Err("cancelled".into()),
                    Err(_) => {}
                }
            }
        }
    }

    fn written(text: &str) -> Written {
        Written {
            text: text.to_string(),
            usage: Vec::new(),
            notes: Vec::new(),
        }
    }

    struct Rig {
        slot: CompactionSlot,
        fake: Arc<Fake>,
        answer: mpsc::Sender<Result<Written, String>>,
        measured: Cell<Option<Measured>>,
        steered: RefCell<Vec<String>>,
        events: RefCell<UnboundedReceiver<StreamEvent>>,
        tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
        turn: u64,
        baseline: Baseline,
    }

    impl Rig {
        /// A turn in a 200k window, its conversation opened by a reminder.
        fn new() -> Self {
            let (answer, outcomes) = mpsc::channel();
            let (tx, events) = unbounded_channel();
            let slot = CompactionSlot::default();
            slot.set_baseline(Baseline {
                window: Some(WINDOW),
                used: 50_000,
                cycle_start: 0,
                overhead: 0,
                reminder: Some(reminder()),
                typed: vec!["fix the parser".to_string()],
            });
            let (turn, baseline) = slot.open_turn();
            Self {
                slot,
                fake: Arc::new(Fake {
                    outcomes: Mutex::new(outcomes),
                    read: Mutex::new(Vec::new()),
                }),
                answer,
                measured: Cell::new(None),
                steered: RefCell::new(Vec::new()),
                events: RefCell::new(events),
                tx,
                turn,
                baseline,
            }
        }

        fn checkpoint(&self) -> BackgroundCheckpoint<'_> {
            BackgroundCheckpoint::new(
                self.slot.clone(),
                self.fake.clone(),
                self.turn,
                self.baseline.clone(),
                &self.measured,
                &self.steered,
            )
            .expect("a known window")
        }

        /// The round top with the next request at `tokens`.
        fn round_top(
            &self,
            checkpoint: &mut BackgroundCheckpoint<'_>,
            messages: &mut Vec<ChatMessage>,
            tokens: u64,
            cancel: &CancelToken,
        ) {
            self.measured.set(Some(Measured {
                tokens,
                len: messages.len(),
            }));
            checkpoint.before_request(messages, &self.tx, cancel);
        }

        fn events(&self) -> Vec<StreamEvent> {
            let mut out = Vec::new();
            while let Ok(event) = self.events.borrow_mut().try_recv() {
                out.push(event);
            }
            out
        }

        /// Wait for the background summary to be written.
        fn until_held(&self, held: Held) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.slot.held(self.turn).kind() != held.kind() {
                assert!(Instant::now() < deadline, "the slot never held {held:?}");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    impl Held {
        /// The variant alone, for the test's waits.
        fn kind(self) -> u8 {
            match self {
                Self::Nothing => 0,
                Self::Running { .. } => 1,
                Self::Ready { .. } => 2,
                Self::Failed => 3,
                Self::Elsewhere => 4,
            }
        }
    }

    fn conversation() -> Vec<ChatMessage> {
        vec![
            ChatMessage::system("You are Alter Zero"),
            ChatMessage::user(format!("{}\n\nfix the parser", reminder())),
            ChatMessage::assistant("reading it"),
        ]
    }

    fn compacted(events: &[StreamEvent]) -> Option<(String, bool)> {
        events.iter().find_map(|event| match event {
            StreamEvent::Compacted {
                summary,
                background,
                ..
            } => Some((summary.clone(), *background)),
            _ => None,
        })
    }

    #[test]
    fn a_summary_starts_in_the_background_over_the_conversation_alone() {
        let rig = Rig::new();
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        rig.round_top(&mut checkpoint, &mut messages, 110_000, &CancelToken::new());
        assert_eq!(messages, conversation(), "nothing changes yet");
        assert_eq!(rig.events(), vec![StreamEvent::CompactionStarted]);
        rig.until_held(Held::Running { at: 0 });
        let deadline = Instant::now() + Duration::from_secs(5);
        while rig.fake.read.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "the summarizer never ran");
            std::thread::sleep(Duration::from_millis(2));
        }
        let read = rig.fake.read.lock().unwrap()[0].clone();
        assert_eq!(
            read,
            summarizer_messages(&conversation(), 3, Some(&reminder()))
        );
    }

    #[test]
    fn a_finished_summary_waits_for_the_ceiling_then_lands_with_the_tail_kept() {
        let rig = Rig::new();
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        let cancel = CancelToken::new();
        rig.round_top(&mut checkpoint, &mut messages, 110_000, &cancel);
        rig.answer.send(Ok(written("we read the parser"))).unwrap();
        rig.until_held(Held::Ready { at: 0 });
        // A round runs; the summary is held below the ceiling.
        messages.push(ChatMessage::assistant("a round after the cut"));
        rig.round_top(&mut checkpoint, &mut messages, 150_000, &cancel);
        assert_eq!(messages.len(), 4, "held, not applied");
        // The ceiling: spliced in, the round after the cut kept as it was.
        rig.round_top(&mut checkpoint, &mut messages, 170_000, &cancel);
        let typed = vec!["fix the parser".to_string()];
        assert_eq!(
            messages,
            spliced_messages(
                &[
                    conversation(),
                    vec![ChatMessage::assistant("a round after the cut")]
                ]
                .concat(),
                3,
                Some(&reminder()),
                &typed,
                "we read the parser"
            )
        );
        let events = rig.events();
        assert_eq!(
            compacted(&events),
            Some(("we read the parser".to_string(), true))
        );
        assert_eq!(rig.slot.held(rig.turn).kind(), Held::Nothing.kind());
    }

    #[test]
    fn the_checkpoint_waits_at_the_ceiling_for_a_summary_still_being_written() {
        let rig = Rig::new();
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        let cancel = CancelToken::new();
        rig.round_top(&mut checkpoint, &mut messages, 110_000, &cancel);
        rig.until_held(Held::Running { at: 0 });
        let answer = rig.answer.clone();
        let late = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            answer.send(Ok(written("late summary"))).unwrap();
        });
        rig.round_top(&mut checkpoint, &mut messages, 175_000, &cancel);
        late.join().unwrap();
        let events = rig.events();
        assert!(
            events.contains(&StreamEvent::CompactionWaiting),
            "{events:?}"
        );
        assert_eq!(compacted(&events), Some(("late summary".to_string(), true)));
        assert_eq!(messages[0], ChatMessage::system("You are Alter Zero"));
        assert!(
            text_of(&messages[1]).ends_with("late summary"),
            "{messages:?}"
        );
    }

    #[test]
    fn the_ceiling_with_nothing_held_compacts_inline() {
        let rig = Rig::new();
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        // The answer is ready before it is asked for: the agent waits on it.
        rig.answer.send(Ok(written("all of it"))).unwrap();
        rig.round_top(&mut checkpoint, &mut messages, 175_000, &CancelToken::new());
        let events = rig.events();
        assert_eq!(events[0], StreamEvent::CompactionStarted, "{events:?}");
        assert_eq!(events[1], StreamEvent::CompactionWaiting, "{events:?}");
        assert_eq!(compacted(&events), Some(("all of it".to_string(), false)));
        assert_eq!(
            messages.len(),
            2,
            "nothing kept after the cut: {messages:?}"
        );
        assert!(text_of(&messages[1]).ends_with("all of it"));
    }

    #[test]
    fn a_failed_summary_leaves_the_ceiling_to_an_inline_one() {
        let rig = Rig::new();
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        let cancel = CancelToken::new();
        rig.round_top(&mut checkpoint, &mut messages, 110_000, &cancel);
        rig.answer.send(Err("HTTP 500".into())).unwrap();
        rig.until_held(Held::Failed);
        rig.round_top(&mut checkpoint, &mut messages, 150_000, &cancel);
        assert_eq!(
            rig.events(),
            vec![StreamEvent::CompactionStarted],
            "no retry"
        );
        rig.answer.send(Ok(written("inline instead"))).unwrap();
        rig.round_top(&mut checkpoint, &mut messages, 175_000, &cancel);
        assert_eq!(
            compacted(&rig.events()),
            Some(("inline instead".to_string(), false))
        );
    }

    #[test]
    fn an_esc_while_the_ceiling_waits_stops_without_splicing() {
        let rig = Rig::new();
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        let turn = CancelToken::new();
        rig.round_top(&mut checkpoint, &mut messages, 110_000, &turn);
        rig.until_held(Held::Running { at: 0 });
        let esc = turn.clone();
        let pressed = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            esc.cancel();
        });
        rig.round_top(&mut checkpoint, &mut messages, 175_000, &turn);
        pressed.join().unwrap();
        assert_eq!(messages, conversation(), "nothing spliced");
        assert_eq!(compacted(&rig.events()), None);
        // The summary itself goes on: it lands at idle.
        assert_eq!(
            rig.slot.held(rig.turn).kind(),
            Held::Running { at: 0 }.kind()
        );
    }

    #[test]
    fn a_summary_that_outlives_its_turn_is_landed_by_the_app() {
        let rig = Rig::new();
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        rig.round_top(&mut checkpoint, &mut messages, 110_000, &CancelToken::new());
        assert!(rig.slot.in_flight());
        assert!(rig.slot.take_landable().is_none(), "still being written");
        rig.answer
            .send(Ok(written("written after the turn")))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let landing = loop {
            if let Some(landing) = rig.slot.take_landable() {
                break landing;
            }
            assert!(Instant::now() < deadline, "it never landed");
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(landing.written.text, "written after the turn");
        assert!(!rig.slot.in_flight());
    }

    #[test]
    fn a_discarded_summary_never_lands() {
        let rig = Rig::new();
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        rig.round_top(&mut checkpoint, &mut messages, 110_000, &CancelToken::new());
        rig.slot.discard();
        rig.answer
            .send(Ok(written("about a history gone")))
            .unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert!(rig.slot.take_landable().is_none());
        assert!(!rig.slot.in_flight());
    }

    #[test]
    fn another_turns_summary_keeps_a_new_turn_from_starting_one() {
        let rig = Rig::new();
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        rig.round_top(&mut checkpoint, &mut messages, 110_000, &CancelToken::new());
        let (next, _) = rig.slot.open_turn();
        assert_eq!(rig.slot.held(next), Held::Elsewhere);
    }

    #[test]
    fn a_compaction_lets_go_of_another_turns_summary() {
        // A summary the last turn started, finished while this one ran: this
        // turn cannot splice it, and once it compacts on its own the other
        // describes a history before that.
        let rig = Rig::new();
        let mut first = rig.checkpoint();
        let mut messages = conversation();
        let cancel = CancelToken::new();
        rig.round_top(&mut first, &mut messages, 110_000, &cancel);
        rig.answer.send(Ok(written("the last turn's"))).unwrap();
        rig.until_held(Held::Ready { at: 0 });
        drop(first);
        let (next, baseline) = rig.slot.open_turn();
        let mut checkpoint = BackgroundCheckpoint::new(
            rig.slot.clone(),
            rig.fake.clone(),
            next,
            baseline,
            &rig.measured,
            &rig.steered,
        )
        .expect("a known window");
        rig.answer.send(Ok(written("this turn's"))).unwrap();
        rig.round_top(&mut checkpoint, &mut messages, 175_000, &cancel);
        assert_eq!(
            compacted(&rig.events()),
            Some(("this turn's".to_string(), false))
        );
        assert!(!rig.slot.in_flight());
        assert!(rig.slot.take_landable().is_none());
    }

    #[test]
    fn a_compaction_that_cannot_make_room_is_not_repeated_in_the_turn() {
        // What rides every request alone passes the ceiling: summarizing
        // again each round would only pay for a summary and take the newest
        // results away from the model.
        let mut rig = Rig::new();
        rig.baseline.overhead = 180_000;
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        let cancel = CancelToken::new();
        rig.answer.send(Ok(written("all of it"))).unwrap();
        rig.answer.send(Ok(written("all of it again"))).unwrap();
        rig.round_top(&mut checkpoint, &mut messages, 175_000, &cancel);
        assert_eq!(
            compacted(&rig.events()),
            Some(("all of it".to_string(), false))
        );
        messages.push(ChatMessage::tool_result("c1", "its output"));
        rig.round_top(&mut checkpoint, &mut messages, 185_000, &cancel);
        assert_eq!(rig.events(), Vec::new(), "spent for the turn");
        assert_eq!(rig.fake.read.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_failed_inline_compaction_is_not_repeated_in_the_turn() {
        let rig = Rig::new();
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        let cancel = CancelToken::new();
        rig.answer.send(Err("HTTP 400: too long".into())).unwrap();
        rig.answer.send(Ok(written("never asked for"))).unwrap();
        rig.round_top(&mut checkpoint, &mut messages, 175_000, &cancel);
        assert!(
            rig.events()
                .iter()
                .any(|event| matches!(event, StreamEvent::CompactionFailed { .. }))
        );
        rig.round_top(&mut checkpoint, &mut messages, 180_000, &cancel);
        assert_eq!(rig.events(), Vec::new());
        assert_eq!(rig.fake.read.lock().unwrap().len(), 1);
    }

    #[test]
    fn without_usage_frames_the_measure_grows_with_what_joins() {
        // A provider that reports no usage: the gauge the turn started from,
        // plus the estimate of every message since.
        let rig = Rig::new();
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        let cancel = CancelToken::new();
        checkpoint.before_request(&mut messages, &rig.tx, &cancel);
        assert_eq!(rig.events(), Vec::new(), "50k is below the start");
        messages.push(ChatMessage::tool_result("c1", "word ".repeat(80_000)));
        checkpoint.before_request(&mut messages, &rig.tx, &cancel);
        assert_eq!(rig.events(), vec![StreamEvent::CompactionStarted]);
    }

    #[test]
    fn a_finished_summary_wakes_the_loop() {
        // It may finish while the app is idle, with nothing else to wake it.
        let rig = Rig::new();
        let (waker, mut woken) = unbounded_channel();
        rig.slot.set_waker(waker);
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        rig.round_top(&mut checkpoint, &mut messages, 110_000, &CancelToken::new());
        assert!(woken.try_recv().is_err(), "nothing finished yet");
        rig.answer.send(Ok(written("done"))).unwrap();
        rig.until_held(Held::Ready { at: 0 });
        assert_eq!(woken.try_recv(), Ok(()));
    }

    #[test]
    fn a_failed_summary_wakes_the_loop_to_let_it_go() {
        let rig = Rig::new();
        let (waker, mut woken) = unbounded_channel();
        rig.slot.set_waker(waker);
        let mut checkpoint = rig.checkpoint();
        let mut messages = conversation();
        rig.round_top(&mut checkpoint, &mut messages, 110_000, &CancelToken::new());
        rig.answer.send(Err("HTTP 500".into())).unwrap();
        rig.until_held(Held::Failed);
        assert_eq!(woken.try_recv(), Ok(()));
    }

    // --- the measure ---

    #[test]
    fn the_estimate_counts_text_and_tool_calls() {
        let text = ChatMessage::user("fix the parser in src/parse.rs");
        let call = ChatMessage::assistant_tool_calls(
            "",
            vec![crate::llm::ToolCallSpec::function(
                "c1",
                "read",
                r#"{"path":"src/parse.rs"}"#,
            )],
        );
        let words = estimate_tokens(std::slice::from_ref(&text));
        assert!(words > 0);
        assert!(
            estimate_tokens(&[text, call]) > words,
            "a call is tokens too"
        );
    }
}
