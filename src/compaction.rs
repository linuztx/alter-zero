//! Background compaction's policy, and the slot a running turn and the
//! boundary share (`docs/background-compaction.md`).
//!
//! A turn checks, at the top of every round, where its context stands against
//! two lines: past the **start line** a summary is started in the background
//! and the turn goes on; past the **ceiling** the held summary is swapped into
//! the turn's message list, the turn waiting only if it is unwritten. The
//! decision is pure ([`decide`]); the [`CompactionSlot`] carries the job's
//! state between the turn's thread, which asks, and the boundary, which writes
//! the summary and answers a swap with the context re-derived from history;
//! [`round_top`] is the turn's side of that exchange.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::context::ContextMessage;
use crate::hooks::HookReport;
use crate::stream::{CancelToken, StreamEvent, TokenUsage};

/// The share of the window kept free below the ceiling, in percent: room for
/// the last reply before compaction — a reasoning model's thought included —
/// and for one round's growth past the line.
const RESERVE_PERCENT: u64 = 15;

/// The least kept free below the ceiling, from a window of
/// [`RESERVE_FLOOR_FROM`] up.
const RESERVE_MIN: u64 = 16_000;

/// The window from which the reserve is never less than [`RESERVE_MIN`]; a
/// smaller one keeps only its [`RESERVE_PERCENT`].
const RESERVE_FLOOR_FROM: u64 = 32_000;

/// How far below the ceiling a background summary starts, in tokens. A
/// summary takes 30–60 s to write, and what decides whether it lands in time
/// is how fast the turn grows in that while — a few thousand tokens at the
/// median, 25k or more one time in twenty (opencode's measurement) — not how
/// big the window is, so the lead is fixed rather than a share.
pub const PRECOMPUTE_LEAD: u64 = 60_000;

/// Failed summaries after which none is started until a compaction lands
/// (opencode's cap): a provider refusing every summary is not asked again on
/// every round.
pub const MAX_FAILURES: u32 = 2;

/// Landings past the ceiling a turn takes before it compacts no more. The
/// first is refolded — summarized again, the verbatim tail included, which a
/// tail of huge reads needs — and the second shows that what rides every
/// request alone, the system prompt and the tool schemas, passes the ceiling:
/// summarizing again would only land there once more, for ever.
pub const MAX_PAST_LANDINGS: u32 = 2;

/// How long a waiting turn sleeps between looks at the slot.
pub const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How many looks a turn gives the boundary to answer a swap — 5 s at
/// [`POLL_INTERVAL`] — before it sends its request as it is. The answer takes
/// one loop iteration; this only stops a boundary that has gone from holding
/// the turn for ever.
pub const SWAP_ANSWER_POLLS: u32 = 250;

/// The **ceiling** for a `window`-token context window: the window less the
/// larger of 15% of it and 16k (the 16k floor only from a 32k window up) —
/// 170k on a 200k window, 850k on a million. A request is not sent past it
/// until a summary has been swapped in (`docs/background-compaction.md`
/// *The lines on the gauge*).
#[must_use]
pub const fn ceiling(window: u64) -> u64 {
    let share = window.saturating_mul(RESERVE_PERCENT) / 100;
    let reserve = if window >= RESERVE_FLOOR_FROM && share < RESERVE_MIN {
        RESERVE_MIN
    } else {
        share
    };
    window.saturating_sub(reserve)
}

/// The **start line** below `ceiling`: [`PRECOMPUTE_LEAD`] below it, or half
/// the room the cycle began with when that is less — `cycle_start` being the
/// context's size right after the last compaction (0 before the first) — so a
/// summary still covers more than the previous one did.
#[must_use]
pub const fn start_line(ceiling: u64, cycle_start: u64) -> u64 {
    let half_room = ceiling.saturating_sub(cycle_start) / 2;
    let lead = if half_room < PRECOMPUTE_LEAD {
        half_room
    } else {
        PRECOMPUTE_LEAD
    };
    ceiling - lead
}

/// What the boundary hands a turn as it starts: the footer gauge — the
/// context the turn's first request carries, calibrated, the tool schemas
/// counted — and the tool schemas' own estimate, what rides every request
/// beside the messages (`docs/background-compaction.md` *The lines on the
/// gauge*).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Baseline {
    /// The footer gauge, in tokens; 0 for none known.
    pub used: u64,
    /// The tool schemas' estimate, in tokens.
    pub overhead: u64,
}

/// Where the session's one background summary stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Job {
    /// None started, or the last one landed, failed or was forgotten.
    #[default]
    None,
    /// Summary `id` is being written.
    Running(u64),
    /// Summary `id` is written and held until the ceiling.
    Ready(u64),
}

/// What a turn does at a round top, before its request goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Send the request as it is.
    Proceed,
    /// Start a background summary, then send the request.
    Start,
    /// Swap held summary `id` in, then send the request.
    Swap(u64),
    /// Wait for summary `id` to be written, then swap it in.
    Wait(u64),
    /// Past the ceiling with nothing started: start one and wait for it.
    StartAndWait,
}

/// The decision at a round top for a context of `used` tokens in a
/// `window`-token window (`docs/background-compaction.md` *The lifecycle*):
/// below the start line nothing; past it a summary starts unless one is
/// running or held; past the ceiling a held summary is swapped in, a running
/// one waited for, and with none one is started and waited for. After
/// [`MAX_FAILURES`] failed summaries nothing is started until a compaction
/// lands.
#[must_use]
pub const fn decide(used: u64, window: u64, cycle_start: u64, job: Job, failures: u32) -> Decision {
    let ceiling = ceiling(window);
    let may_start = failures < MAX_FAILURES;
    if used >= ceiling {
        return match job {
            Job::Ready(id) => Decision::Swap(id),
            Job::Running(id) => Decision::Wait(id),
            Job::None if may_start => Decision::StartAndWait,
            Job::None => Decision::Proceed,
        };
    }
    if used >= start_line(ceiling, cycle_start) && matches!(job, Job::None) && may_start {
        return Decision::Start;
    }
    Decision::Proceed
}

/// The state a running turn and the boundary share: whether compaction is on
/// and for what window, the one background job, the failures this cycle, the
/// cycle's start, and the boundary's answer to a swap. Cloneable, and every
/// clone is the same slot (the [`crate::steer::SteerQueue`] shape): the
/// boundary keeps one and hands a clone to each backend build.
///
/// A poisoned lock (a panicked thread) reads as compaction off rather than
/// bringing the UI down: a turn that goes out uncompacted meets the
/// provider's limit the way it always did.
#[derive(Clone, Default, Debug)]
pub struct CompactionSlot {
    inner: Arc<Mutex<SlotState>>,
}

#[derive(Debug, Default)]
struct SlotState {
    window: Option<u64>,
    job: Job,
    failures: u32,
    cycle_start: u64,
    next_id: u64,
    answer: Option<(u64, Option<Vec<ContextMessage>>)>,
    /// A summary was swapped in and no round top has measured the context it
    /// left yet.
    landed_unmeasured: bool,
    /// Whether the last landing left the context at or past the ceiling, by
    /// the gauge's count or the turn's own.
    last_landing_past: bool,
    /// This turn's landings that did ([`MAX_PAST_LANDINGS`]).
    past_landings: u32,
    /// The gauge the boundary handed the next turn to start.
    baseline: Baseline,
}

impl CompactionSlot {
    /// A slot with compaction off.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> Option<MutexGuard<'_, SlotState>> {
        self.inner.lock().ok()
    }

    /// Turn compaction on for a `window`-token window, or off with `None` (or
    /// a meaningless 0) — the boundary's, whenever the window or the
    /// **Auto compact** knob changes.
    pub fn configure(&self, window: Option<u64>) {
        if let Some(mut state) = self.state() {
            state.window = window.filter(|&window| window > 0);
        }
    }

    /// The window compaction works against; `None` while it is off.
    #[must_use]
    pub fn window(&self) -> Option<u64> {
        self.state()?.window
    }

    /// The background job.
    #[must_use]
    pub fn job(&self) -> Job {
        self.state().map_or(Job::None, |state| state.job)
    }

    /// Summaries that failed since the last compaction landed.
    #[must_use]
    pub fn failures(&self) -> u32 {
        self.state().map_or(0, |state| state.failures)
    }

    /// The context's size right after the last compaction — the start line's
    /// `cycle_start`.
    #[must_use]
    pub fn cycle_start(&self) -> u64 {
        self.state().map_or(0, |state| state.cycle_start)
    }

    /// Claim a new job, `Running`, and return its id — `None` while one is
    /// already running or held.
    pub fn begin(&self) -> Option<u64> {
        let mut state = self.state()?;
        if !matches!(state.job, Job::None) {
            return None;
        }
        state.next_id += 1;
        let id = state.next_id;
        state.job = Job::Running(id);
        state.answer = None;
        Some(id)
    }

    /// Summary `id` is written: hold it. Nothing for a job no longer running.
    pub fn ready(&self, id: u64) {
        if let Some(mut state) = self.state()
            && state.job == Job::Running(id)
        {
            state.job = Job::Ready(id);
        }
    }

    /// Summary `id` failed: free the slot and count it. Nothing for a job no
    /// longer in the slot.
    pub fn fail(&self, id: u64) {
        if let Some(mut state) = self.state()
            && matches!(state.job, Job::Running(job) | Job::Ready(job) if job == id)
        {
            state.job = Job::None;
            state.failures += 1;
        }
    }

    /// Summary `id` was swapped in and left a context of `after` tokens: a
    /// new cycle starts there, with its failures cleared.
    pub fn landed(&self, id: u64, after: u64) {
        if let Some(mut state) = self.state()
            && matches!(state.job, Job::Running(job) | Job::Ready(job) if job == id)
        {
            state.job = Job::None;
            state.failures = 0;
            state.cycle_start = after;
            state.landed_unmeasured = true;
            state.last_landing_past = state.window.is_some_and(|window| after >= ceiling(window));
            if state.last_landing_past {
                state.past_landings += 1;
            }
        }
    }

    /// The history moved under the job (`/clear`, a backtrack, a `/resume`, a
    /// `/model` switch): drop it and start the cycle over. A worker that
    /// finishes afterwards changes nothing.
    pub fn forget(&self) {
        if let Some(mut state) = self.state() {
            state.job = Job::None;
            state.failures = 0;
            state.cycle_start = 0;
            state.answer = None;
            state.landed_unmeasured = false;
            state.last_landing_past = false;
            state.past_landings = 0;
        }
    }

    /// Whether this turn's compactions could not bring the context under the
    /// ceiling, a refold included ([`MAX_PAST_LANDINGS`]), so no summary is
    /// started until the next turn.
    #[must_use]
    pub fn futile(&self) -> bool {
        self.state()
            .is_some_and(|state| state.past_landings >= MAX_PAST_LANDINGS)
    }

    /// Whether a summary started between turns is held off: the last landing
    /// left the context at or past the ceiling, and with no turn waiting on
    /// it a summary started there could only land there again — the next
    /// turn refolds instead — or this turn is spent.
    #[must_use]
    pub fn holds_idle_starts(&self) -> bool {
        self.state().is_some_and(|state| {
            state.last_landing_past || state.past_landings >= MAX_PAST_LANDINGS
        })
    }

    /// The boundary, just before a turn starts: the gauge it starts from.
    pub fn set_baseline(&self, baseline: Baseline) {
        if let Some(mut state) = self.state() {
            state.baseline = baseline;
        }
    }

    /// A turn began: whatever the last one could not compact, it may — a
    /// swap no round top has measured yet is still measured at its first.
    /// Returns the gauge the turn was handed, which is read once.
    pub fn turn_started(&self) -> Baseline {
        let Some(mut state) = self.state() else {
            return Baseline::default();
        };
        state.past_landings = 0;
        std::mem::take(&mut state.baseline)
    }

    /// The first round top after a swap measures the context it left by the
    /// turn's own count: at or past the ceiling the landing could not help,
    /// counted once whichever count said so.
    fn measure_landing(&self, used: u64) {
        if let Some(mut state) = self.state()
            && std::mem::take(&mut state.landed_unmeasured)
            && !state.last_landing_past
            && state.window.is_some_and(|window| used >= ceiling(window))
        {
            state.last_landing_past = true;
            state.past_landings += 1;
        }
    }

    /// The context's size right after the last compaction, as the boundary
    /// reads it off the history — 0 for a conversation with none.
    pub fn set_cycle_start(&self, cycle_start: u64) {
        if let Some(mut state) = self.state() {
            state.cycle_start = cycle_start;
        }
    }

    /// The loop bottom, with no turn in flight: let go of a job the loop
    /// holds no record of — its start dropped with the channel an Esc or a
    /// `/clear` swapped, so no worker writes it and nothing else would ever
    /// free the slot. Its `known` job, if any, is kept; the cycle is not
    /// touched, the history being the same.
    pub fn drop_unknown(&self, known: Option<u64>) {
        if let Some(mut state) = self.state()
            && let Job::Running(id) | Job::Ready(id) = state.job
            && known != Some(id)
        {
            state.job = Job::None;
            state.answer = None;
        }
    }

    /// The boundary's answer to the swap of summary `id`: the context to go
    /// on with, or `None` to send the request as it is.
    pub fn answer(&self, id: u64, context: Option<Vec<ContextMessage>>) {
        if let Some(mut state) = self.state() {
            state.answer = Some((id, context));
        }
    }

    /// Take the boundary's answer to the swap of summary `id`, once it has
    /// given one.
    fn take_answer(&self, id: u64) -> Option<Option<Vec<ContextMessage>>> {
        let mut state = self.state()?;
        match state.answer.take() {
            Some((answered, context)) if answered == id => Some(context),
            other => {
                state.answer = other;
                None
            }
        }
    }
}

/// The turn's side of background compaction, at the top of a round before its
/// request goes out (`docs/background-compaction.md` *The lifecycle*): with
/// `used` tokens in the context it starts a summary, swaps a held one in,
/// waits for a running one, or does nothing, announcing each on `tx`. Returns
/// the context to rebuild the turn's message list from when a swap landed,
/// `None` when the request goes out as it is.
///
/// `pause` sleeps [`POLL_INTERVAL`] between looks at the slot while the turn
/// waits; `cancel` (Esc, quit) ends any wait at once.
pub fn round_top(
    slot: &CompactionSlot,
    used: u64,
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
    pause: &mut dyn FnMut(),
) -> Option<Vec<ContextMessage>> {
    // A turn an Esc stopped may still reach a round top on its way out:
    // what it would start, nobody would hear of.
    if cancel.is_cancelled() {
        return None;
    }
    slot.measure_landing(used);
    let mut announced = None;
    loop {
        let window = slot.window()?;
        // Compactions this turn that could not bring the context under its
        // ceiling, a refold included, start nothing more until the next turn,
        // as a cycle's failures do (`docs/background-compaction.md`
        // *Failures and edges*): what rides every request alone fills the
        // window, and summarizing again would loop.
        let failures = if slot.futile() {
            MAX_FAILURES
        } else {
            slot.failures()
        };
        match decide(used, window, slot.cycle_start(), slot.job(), failures) {
            Decision::Proceed => return None,
            Decision::Start => {
                if let Some(id) = slot.begin() {
                    let _ = tx.send(StreamEvent::CompactionStart { id });
                }
                return None;
            }
            Decision::StartAndWait => {
                if let Some(id) = slot.begin() {
                    let _ = tx.send(StreamEvent::CompactionStart { id });
                }
            }
            Decision::Wait(id) => {
                if announced != Some(id) {
                    announced = Some(id);
                    let _ = tx.send(StreamEvent::CompactionWaiting { id });
                }
                if cancel.is_cancelled() {
                    return None;
                }
                pause();
                if cancel.is_cancelled() {
                    return None;
                }
            }
            Decision::Swap(id) => return request_swap(slot, id, tx, cancel, pause),
        }
    }
}

/// Ask the boundary to swap summary `id` in and wait for its answer.
fn request_swap(
    slot: &CompactionSlot,
    id: u64,
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
    pause: &mut dyn FnMut(),
) -> Option<Vec<ContextMessage>> {
    let _ = tx.send(StreamEvent::CompactionSwap { id });
    for _ in 0..SWAP_ANSWER_POLLS {
        if let Some(answer) = slot.take_answer(id) {
            return answer;
        }
        if cancel.is_cancelled() {
            return None;
        }
        pause();
    }
    slot.take_answer(id).flatten()
}

/// Why a summary that ended with nothing written is no summary.
pub const EMPTY_SUMMARY: &str = "the summary came back empty";

/// A background summarizer's stream, folded event by event on the worker
/// that drains it (`docs/background-compaction.md` *The lifecycle*): the
/// summary's text, the compact hooks' reports for the cells above the
/// `Context compacted` line, and the provider's usage frames for the
/// since-launch ledger.
#[derive(Debug, Default)]
pub struct SummaryStream {
    text: String,
    /// How much of `text` the attempt now streaming found there — what a
    /// retry takes the summary back to (`App::track_attempt`'s rule).
    attempt_floor: Option<usize>,
    hooks: Vec<(String, HookReport)>,
    usage: Vec<TokenUsage>,
}

impl SummaryStream {
    /// Fold one event in; `Some` once the stream has ended — the summary, or
    /// why there is none.
    pub fn push(&mut self, event: StreamEvent) -> Option<Result<String, String>> {
        if event.is_attempt_output() {
            self.attempt_floor.get_or_insert(self.text.len());
        } else if !matches!(event, StreamEvent::Retrying { .. }) {
            self.attempt_floor = None;
        }
        match event {
            StreamEvent::Chunk(text) => self.text.push_str(&text),
            // A connection lost mid-reply restarts the round: what its own
            // attempt streamed is void, and what the rounds before it wrote
            // stays — a summary the output cap cut off goes on in another
            // round (`docs/offline.md`, `docs/unfinished-replies.md`).
            StreamEvent::Retrying { .. } => {
                if let Some(floor) = self.attempt_floor.take() {
                    self.text.truncate(floor);
                }
            }
            StreamEvent::HookNote {
                label,
                report: Some(report),
                ..
            } => self.hooks.push((label, report)),
            StreamEvent::Usage(usage) => self.usage.push(usage),
            StreamEvent::StreamDone => {
                let summary = std::mem::take(&mut self.text);
                return Some(if summary.trim().is_empty() {
                    Err(EMPTY_SUMMARY.to_string())
                } else {
                    Ok(summary)
                });
            }
            StreamEvent::Error(message) => return Some(Err(message)),
            // Its thought, a wait for the connection, the rest: nothing the
            // summary keeps.
            _ => {}
        }
        None
    }

    /// The compact hooks' reports, each under its event's label, and the
    /// usage frames.
    #[must_use]
    pub fn into_parts(self) -> (Vec<(String, HookReport)>, Vec<TokenUsage>) {
        (self.hooks, self.usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ContextMessage, ContextRole};
    use crate::stream::{CancelToken, StreamEvent};
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

    // ── The lines (docs/background-compaction.md *The lines on the gauge*) ─

    #[test]
    fn the_ceiling_keeps_fifteen_percent_of_the_window_free() {
        assert_eq!(ceiling(200_000), 170_000);
        assert_eq!(ceiling(1_000_000), 850_000);
    }

    #[test]
    fn the_ceiling_keeps_at_least_16k_free_from_a_32k_window_up() {
        assert_eq!(ceiling(100_000), 84_000);
        assert_eq!(ceiling(32_000), 16_000);
    }

    #[test]
    fn a_window_under_32k_keeps_only_its_fifteen_percent() {
        assert_eq!(ceiling(20_000), 17_000);
    }

    #[test]
    fn a_summary_starts_60k_below_the_ceiling() {
        assert_eq!(start_line(170_000, 0), 110_000);
        assert_eq!(start_line(850_000, 30_000), 790_000);
    }

    #[test]
    fn on_a_small_cycle_it_starts_halfway_through_the_room() {
        // After a compaction left 82k, half the room to a 170k ceiling is 44k:
        // the next summary still covers more than the last one did.
        assert_eq!(start_line(170_000, 82_000), 126_000);
        assert_eq!(start_line(16_000, 2_000), 9_000);
    }

    // ── The decision at a round top ────────────────────────────────────────

    const WINDOW: u64 = 200_000;

    #[test]
    fn below_the_start_line_the_request_goes_out() {
        assert_eq!(decide(100_000, WINDOW, 0, Job::None, 0), Decision::Proceed);
    }

    #[test]
    fn past_the_start_line_a_summary_starts_and_the_request_goes_out() {
        assert_eq!(decide(120_000, WINDOW, 0, Job::None, 0), Decision::Start);
    }

    #[test]
    fn below_the_ceiling_a_running_or_held_summary_is_left_alone() {
        assert_eq!(
            decide(150_000, WINDOW, 0, Job::Running(1), 0),
            Decision::Proceed
        );
        assert_eq!(
            decide(150_000, WINDOW, 0, Job::Ready(1), 0),
            Decision::Proceed
        );
    }

    #[test]
    fn at_the_ceiling_a_held_summary_is_swapped_in() {
        assert_eq!(
            decide(170_000, WINDOW, 0, Job::Ready(1), 0),
            Decision::Swap(1)
        );
    }

    #[test]
    fn at_the_ceiling_a_running_summary_is_waited_for() {
        assert_eq!(
            decide(180_000, WINDOW, 0, Job::Running(4), 0),
            Decision::Wait(4)
        );
    }

    #[test]
    fn at_the_ceiling_with_nothing_started_one_starts_and_is_waited_for() {
        assert_eq!(
            decide(190_000, WINDOW, 0, Job::None, 0),
            Decision::StartAndWait
        );
    }

    #[test]
    fn two_failures_stop_any_start_until_a_compaction_lands() {
        assert_eq!(
            decide(120_000, WINDOW, 0, Job::None, MAX_FAILURES),
            Decision::Proceed
        );
        assert_eq!(
            decide(190_000, WINDOW, 0, Job::None, MAX_FAILURES),
            Decision::Proceed
        );
        assert_eq!(
            decide(120_000, WINDOW, 0, Job::None, MAX_FAILURES - 1),
            Decision::Start
        );
    }

    // ── The slot ───────────────────────────────────────────────────────────

    fn configured() -> CompactionSlot {
        let slot = CompactionSlot::new();
        slot.configure(Some(WINDOW));
        slot
    }

    #[test]
    fn one_job_at_a_time() {
        let slot = configured();
        let id = slot.begin().expect("nothing running");
        assert_eq!(slot.job(), Job::Running(id));
        assert_eq!(slot.begin(), None, "a second start is refused");
        slot.ready(id);
        assert_eq!(slot.job(), Job::Ready(id));
        assert_eq!(slot.begin(), None, "nor while one is held");
    }

    #[test]
    fn a_failure_frees_the_slot_and_counts() {
        let slot = configured();
        let id = slot.begin().expect("nothing running");
        slot.fail(id);
        assert_eq!(slot.job(), Job::None);
        assert_eq!(slot.failures(), 1);
    }

    #[test]
    fn a_landed_compaction_starts_a_new_cycle() {
        let slot = configured();
        let failed = slot.begin().expect("nothing running");
        slot.fail(failed);
        let id = slot.begin().expect("free again");
        slot.ready(id);
        slot.landed(id, 82_000);
        assert_eq!(slot.job(), Job::None);
        assert_eq!(slot.failures(), 0, "a landing clears the failures");
        assert_eq!(slot.cycle_start(), 82_000);
    }

    #[test]
    fn the_cycle_start_the_boundary_reads_off_the_history_moves_the_start_line() {
        let slot = configured();
        slot.set_cycle_start(90_000);
        assert_eq!(slot.cycle_start(), 90_000);
        let (tx, mut rx) = channel();
        // 170k less half the 80k the cycle began with: 130k.
        round_top(&slot, 125_000, &tx, &CancelToken::new(), &mut || {});
        assert!(drained(&mut rx).is_empty(), "below the cycle's start line");
        round_top(&slot, 131_000, &tx, &CancelToken::new(), &mut || {});
        assert!(
            matches!(
                drained(&mut rx).as_slice(),
                [StreamEvent::CompactionStart { .. }]
            ),
            "past it"
        );
    }

    #[test]
    fn a_turn_reads_the_gauge_it_was_handed_once() {
        let slot = configured();
        let baseline = Baseline {
            used: 150_000,
            overhead: 8_000,
        };
        slot.set_baseline(baseline);
        assert_eq!(slot.turn_started(), baseline);
        assert_eq!(
            slot.turn_started(),
            Baseline::default(),
            "a turn nobody handed one measures by the estimate"
        );
    }

    #[test]
    fn a_job_the_loop_never_heard_of_is_let_go() {
        // Its start was dropped with the channel an Esc or a /clear swapped:
        // no worker writes it, and nothing else would ever free the slot.
        let slot = configured();
        let id = slot.begin().expect("nothing running");
        slot.drop_unknown(None);
        assert_eq!(slot.job(), Job::None);
        assert!(
            slot.begin().is_some_and(|next| next != id),
            "the slot is free again"
        );
    }

    #[test]
    fn the_loop_keeps_the_job_it_knows() {
        let slot = configured();
        let id = slot.begin().expect("nothing running");
        slot.drop_unknown(Some(id));
        assert_eq!(slot.job(), Job::Running(id));
        slot.ready(id);
        slot.drop_unknown(Some(id));
        assert_eq!(slot.job(), Job::Ready(id));
    }

    #[test]
    fn a_cancelled_turn_starts_no_summary() {
        // A turn an Esc stopped may still reach a round top on its way out:
        // what it would start, nobody would hear of.
        let slot = configured();
        let (tx, mut rx) = channel();
        let cancel = CancelToken::new();
        cancel.cancel();
        let swapped = round_top(&slot, 175_000, &tx, &cancel, &mut || {
            panic!("nothing to wait for")
        });
        assert_eq!(swapped, None);
        assert_eq!(slot.job(), Job::None);
        assert!(drained(&mut rx).is_empty());
    }

    #[test]
    fn a_forgotten_job_ignores_its_late_news() {
        // The history moved (/clear, a backtrack): the job is dropped, and a
        // worker that finishes afterwards changes nothing.
        let slot = configured();
        let id = slot.begin().expect("nothing running");
        slot.forget();
        slot.ready(id);
        assert_eq!(slot.job(), Job::None);
        slot.fail(id);
        assert_eq!(slot.failures(), 0);
    }

    // ── The turn's side: `round_top` ───────────────────────────────────────

    fn channel() -> (UnboundedSender<StreamEvent>, UnboundedReceiver<StreamEvent>) {
        unbounded_channel()
    }

    fn drained(rx: &mut UnboundedReceiver<StreamEvent>) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    fn compacted() -> Vec<ContextMessage> {
        vec![ContextMessage::new(
            ContextRole::User,
            "the compacted window",
        )]
    }

    #[test]
    fn an_unconfigured_slot_never_compacts() {
        let slot = CompactionSlot::new();
        let (tx, mut rx) = channel();
        let swapped = round_top(&slot, 190_000, &tx, &CancelToken::new(), &mut || {
            panic!("nothing to wait for")
        });
        assert_eq!(swapped, None);
        assert!(drained(&mut rx).is_empty());
    }

    #[test]
    fn a_start_announces_itself_and_does_not_wait() {
        let slot = configured();
        let (tx, mut rx) = channel();
        let swapped = round_top(&slot, 120_000, &tx, &CancelToken::new(), &mut || {
            panic!("a start never waits")
        });
        assert_eq!(swapped, None);
        let Job::Running(id) = slot.job() else {
            panic!("the job is running: {:?}", slot.job());
        };
        let events = drained(&mut rx);
        assert!(
            matches!(events.as_slice(), [StreamEvent::CompactionStart { id: sent }] if *sent == id),
            "{events:?}"
        );
    }

    #[test]
    fn at_the_ceiling_a_held_summary_is_swapped_for_the_boundarys_answer() {
        let slot = configured();
        let id = slot.begin().expect("nothing running");
        slot.ready(id);
        let (tx, mut rx) = channel();
        // The boundary answers the swap request on its next iteration.
        let swapped = round_top(&slot, 175_000, &tx, &CancelToken::new(), &mut || {
            slot.answer(id, Some(compacted()));
        });
        assert_eq!(swapped, Some(compacted()));
        let events = drained(&mut rx);
        assert!(
            matches!(events.as_slice(), [StreamEvent::CompactionSwap { id: sent }] if *sent == id),
            "{events:?}"
        );
    }

    #[test]
    fn at_the_ceiling_a_running_summary_is_waited_for_then_swapped() {
        let slot = configured();
        let id = slot.begin().expect("nothing running");
        let (tx, mut rx) = channel();
        let mut polls = 0;
        let swapped = round_top(&slot, 180_000, &tx, &CancelToken::new(), &mut || {
            polls += 1;
            match slot.job() {
                Job::Running(_) => slot.ready(id),
                _ => slot.answer(id, Some(compacted())),
            }
        });
        assert_eq!(swapped, Some(compacted()));
        let events = drained(&mut rx);
        assert!(
            matches!(
                events.as_slice(),
                [StreamEvent::CompactionWaiting { id: a }, StreamEvent::CompactionSwap { id: b }]
                    if *a == id && *b == id
            ),
            "{events:?}"
        );
    }

    #[test]
    fn past_the_ceiling_with_nothing_started_it_starts_one_and_waits() {
        let slot = configured();
        let (tx, mut rx) = channel();
        let swapped = round_top(
            &slot,
            190_000,
            &tx,
            &CancelToken::new(),
            &mut || match slot.job() {
                Job::Running(id) => slot.ready(id),
                Job::Ready(id) => slot.answer(id, Some(compacted())),
                Job::None => {}
            },
        );
        assert_eq!(swapped, Some(compacted()));
        let events = drained(&mut rx);
        assert!(
            matches!(
                events.as_slice(),
                [
                    StreamEvent::CompactionStart { .. },
                    StreamEvent::CompactionWaiting { .. },
                    StreamEvent::CompactionSwap { .. }
                ]
            ),
            "{events:?}"
        );
    }

    #[test]
    fn a_refused_swap_sends_the_request_as_it_is() {
        let slot = configured();
        let id = slot.begin().expect("nothing running");
        slot.ready(id);
        let (tx, _rx) = channel();
        let swapped = round_top(&slot, 175_000, &tx, &CancelToken::new(), &mut || {
            slot.answer(id, None);
        });
        assert_eq!(swapped, None);
    }

    #[test]
    fn failures_while_waiting_give_up_after_the_cap() {
        // Every summary fails: the turn retries up to the cap, then goes out
        // uncompacted, as it would have before this existed.
        let slot = configured();
        let first = slot.begin().expect("nothing running");
        let (tx, mut rx) = channel();
        let swapped = round_top(&slot, 180_000, &tx, &CancelToken::new(), &mut || {
            if let Job::Running(id) = slot.job() {
                slot.fail(id);
            }
        });
        assert_eq!(swapped, None);
        assert_eq!(slot.failures(), MAX_FAILURES);
        let events = drained(&mut rx);
        assert!(
            matches!(
                events.as_slice(),
                [
                    StreamEvent::CompactionWaiting { id: a },
                    StreamEvent::CompactionStart { .. },
                    StreamEvent::CompactionWaiting { .. }
                ] if *a == first
            ),
            "{events:?}"
        );
    }

    #[test]
    fn a_cancelled_turn_stops_waiting() {
        let slot = configured();
        slot.begin().expect("nothing running");
        let cancel = CancelToken::new();
        let (tx, _rx) = channel();
        let swapped = round_top(&slot, 180_000, &tx, &cancel, &mut || cancel.cancel());
        assert_eq!(swapped, None);
    }

    #[test]
    fn a_swap_nobody_answers_is_given_up_on() {
        // The boundary answers within one loop iteration; one that never does
        // (it has gone) must not hold the turn for ever.
        let slot = configured();
        let id = slot.begin().expect("nothing running");
        slot.ready(id);
        let (tx, _rx) = channel();
        let mut polls = 0u32;
        let swapped = round_top(&slot, 175_000, &tx, &CancelToken::new(), &mut || polls += 1);
        assert_eq!(swapped, None);
        assert_eq!(polls, SWAP_ANSWER_POLLS);
    }

    // ── A compaction that could not help (docs/background-compaction.md
    //    *Failures and edges*) ──────────────────────────────────────────────

    /// A held summary of job `id` swapped in, leaving the context at `after`
    /// by the boundary's gauge.
    fn swapped_in(slot: &CompactionSlot, after: u64) {
        let id = slot.begin().expect("nothing running");
        slot.ready(id);
        slot.landed(id, after);
    }

    /// The job a refold started at the last round top, written and swapped
    /// in at `after`.
    fn refold_landed(slot: &CompactionSlot, after: u64) {
        let Job::Running(id) = slot.job() else {
            panic!("a refold is running: {:?}", slot.job());
        };
        slot.ready(id);
        slot.landed(id, after);
    }

    /// One round top at `used` whose waits end as an Esc would: what it sent.
    fn round_top_once(slot: &CompactionSlot, used: u64) -> Vec<StreamEvent> {
        let (tx, mut rx) = channel();
        let cancel = CancelToken::new();
        round_top(slot, used, &tx, &cancel, &mut || cancel.cancel());
        drained(&mut rx)
    }

    fn starts(events: &[StreamEvent]) -> bool {
        matches!(events.first(), Some(StreamEvent::CompactionStart { .. }))
    }

    #[test]
    fn a_swap_past_the_ceiling_is_refolded_once() {
        // A tail of huge reads kept verbatim leaves the swap past the
        // ceiling: summarizing everything once more, the tail included,
        // brings it down, so the turn starts that summary and waits for it.
        let slot = configured();
        swapped_in(&slot, 180_000);
        assert!(
            !slot.futile(),
            "one landing past the ceiling is not the end"
        );
        let events = round_top_once(&slot, 185_000);
        assert!(starts(&events), "{events:?}");
    }

    #[test]
    fn a_refold_that_lands_past_the_ceiling_spends_the_turn() {
        // The system prompt and the tool schemas alone fill a small window:
        // another summary would land where this one did, so the turn goes on
        // uncompacted rather than loop.
        let slot = configured();
        swapped_in(&slot, 180_000);
        round_top_once(&slot, 185_000);
        refold_landed(&slot, 181_000);
        assert!(slot.futile());
        for used in [181_000, 190_000, 199_000] {
            assert!(round_top_once(&slot, used).is_empty(), "no summary started");
        }
    }

    #[test]
    fn the_turns_own_measure_after_a_swap_counts_too() {
        // The gauge's calibrated count put each swap under the ceiling, the
        // turn's own count of its rebuilt list over it: either says the
        // landing could not help.
        let slot = configured();
        swapped_in(&slot, 120_000);
        let events = round_top_once(&slot, 175_000);
        assert!(starts(&events), "the refold: {events:?}");
        refold_landed(&slot, 120_000);
        assert!(round_top_once(&slot, 176_000).is_empty(), "spent");
        assert!(slot.futile());
    }

    #[test]
    fn only_the_first_round_top_after_a_swap_measures_it() {
        let slot = configured();
        swapped_in(&slot, 90_000);
        let (tx, mut rx) = channel();
        // Below the start line the cycle began at: nothing, and not futile.
        round_top(&slot, 95_000, &tx, &CancelToken::new(), &mut || {
            panic!("nothing to wait for")
        });
        assert!(!slot.futile());
        // Rounds later the context has grown past the ceiling: a summary
        // is started and waited for as usual — until an Esc ends the wait.
        let cancel = CancelToken::new();
        round_top(&slot, 175_000, &tx, &cancel, &mut || cancel.cancel());
        assert!(
            matches!(
                drained(&mut rx).first(),
                Some(StreamEvent::CompactionStart { .. })
            ),
            "a summary started"
        );
    }

    #[test]
    fn a_new_turn_may_compact_again() {
        let slot = configured();
        swapped_in(&slot, 180_000);
        round_top_once(&slot, 185_000);
        refold_landed(&slot, 181_000);
        assert!(slot.futile());
        slot.turn_started();
        assert!(!slot.futile());
        // Still past the ceiling, with the next message on top: the next turn
        // starts a summary and waits for it — until an Esc ends the wait.
        let events = round_top_once(&slot, 185_000);
        assert!(starts(&events), "the next turn tries again: {events:?}");
    }

    #[test]
    fn a_new_turn_still_measures_a_swap_no_round_top_saw() {
        // Landed between turns: the turn that follows counts it at its first
        // round top, so its own refold landing past the ceiling spends it.
        let slot = configured();
        swapped_in(&slot, 120_000);
        slot.turn_started();
        let events = round_top_once(&slot, 176_000);
        assert!(starts(&events), "the refold: {events:?}");
        refold_landed(&slot, 180_000);
        assert!(slot.futile(), "the landing between turns counted");
    }

    #[test]
    fn between_turns_nothing_starts_after_a_landing_past_the_ceiling() {
        // No turn waits on a summary there, so one started at idle after a
        // landing past the ceiling could only land there again; the next
        // turn refolds instead.
        let slot = configured();
        swapped_in(&slot, 180_000);
        assert!(slot.holds_idle_starts());
        swapped_in(&slot, 90_000);
        assert!(
            !slot.holds_idle_starts(),
            "a landing under the ceiling lifts it"
        );
    }

    #[test]
    fn forgetting_a_conversation_forgets_its_futility() {
        let slot = configured();
        swapped_in(&slot, 180_000);
        round_top_once(&slot, 185_000);
        refold_landed(&slot, 181_000);
        assert!(slot.futile() && slot.holds_idle_starts());
        slot.forget();
        assert!(!slot.futile() && !slot.holds_idle_starts());
    }

    // ── The summarizer's stream (docs/background-compaction.md *The lifecycle*)

    fn fold(events: Vec<StreamEvent>) -> (Option<Result<String, String>>, SummaryStream) {
        let mut stream = SummaryStream::default();
        let mut end = None;
        for event in events {
            if let Some(ended) = stream.push(event) {
                end = Some(ended);
                break;
            }
        }
        (end, stream)
    }

    fn report(event: &str) -> HookReport {
        HookReport {
            event: event.to_string(),
            hooks: Vec::new(),
            warnings: vec!["a warning".to_string()],
        }
    }

    #[test]
    fn a_summary_is_the_text_streamed_before_the_stream_ends() {
        let (end, _) = fold(vec![
            StreamEvent::ThinkingStart,
            StreamEvent::ThinkingChunk("what to keep".into()),
            StreamEvent::ThinkingEnd,
            StreamEvent::Chunk("The user is porting ".into()),
            StreamEvent::Chunk("the payments module.".into()),
            StreamEvent::StreamDone,
            StreamEvent::Chunk("never read".into()),
        ]);
        assert_eq!(
            end,
            Some(Ok("The user is porting the payments module.".to_string())),
            "the reply alone — no thought, nothing after the end"
        );
    }

    #[test]
    fn a_stream_still_going_has_not_ended() {
        let (end, _) = fold(vec![StreamEvent::Chunk("The user is".into())]);
        assert_eq!(end, None);
    }

    #[test]
    fn an_error_ends_it_without_a_summary() {
        let (end, _) = fold(vec![
            StreamEvent::Chunk("The user is".into()),
            StreamEvent::Error("429 Too Many Requests".into()),
        ]);
        assert_eq!(end, Some(Err("429 Too Many Requests".to_string())));
    }

    #[test]
    fn an_empty_summary_is_no_summary() {
        let (end, _) = fold(vec![
            StreamEvent::Chunk("  \n".into()),
            StreamEvent::StreamDone,
        ]);
        assert_eq!(end, Some(Err(EMPTY_SUMMARY.to_string())));
    }

    #[test]
    fn a_retried_attempt_voids_what_it_streamed() {
        // A connection lost mid-reply restarts the round, and the restart
        // writes the summary from its beginning (docs/offline.md).
        let (end, _) = fold(vec![
            StreamEvent::Chunk("The user is por".into()),
            StreamEvent::Retrying { attempt: 1, max: 3 },
            StreamEvent::Chunk("The user is porting payments.".into()),
            StreamEvent::StreamDone,
        ]);
        assert_eq!(end, Some(Ok("The user is porting payments.".to_string())));
    }

    #[test]
    fn a_retry_takes_back_only_its_own_attempt() {
        // A summary the output cap cut off goes on in another round
        // (docs/unfinished-replies.md); a connection lost in that round takes
        // back what its own attempt streamed and nothing the first round
        // wrote.
        let (end, _) = fold(vec![
            StreamEvent::Chunk("The user is porting ".into()),
            StreamEvent::HookNote {
                label: String::new(),
                text: "Your reply was cut off. Continue it.".into(),
                report: None,
            },
            StreamEvent::Chunk("the paym".into()),
            StreamEvent::Retrying { attempt: 1, max: 3 },
            StreamEvent::Chunk("the payments module.".into()),
            StreamEvent::StreamDone,
        ]);
        assert_eq!(
            end,
            Some(Ok("The user is porting the payments module.".to_string()))
        );
    }

    #[test]
    fn the_compact_hooks_reports_are_kept_for_their_cells() {
        // PreCompact and PostCompact said something: their cells go above
        // the `Context compacted` line when it lands, and nothing of their
        // text joins the summary.
        let (_, stream) = fold(vec![
            StreamEvent::HookNote {
                label: "PreCompact".into(),
                text: "keep the API names".into(),
                report: Some(report("PreCompact")),
            },
            StreamEvent::HookNote {
                label: "PreCompact".into(),
                text: "silent".into(),
                report: None,
            },
            StreamEvent::Chunk("The user is porting payments.".into()),
            StreamEvent::StreamDone,
        ]);
        let (hooks, _) = stream.into_parts();
        assert_eq!(
            hooks,
            vec![("PreCompact".to_string(), report("PreCompact"))]
        );
    }

    #[test]
    fn its_usage_frames_are_kept_for_the_launch_ledger() {
        let frame = TokenUsage {
            input: 120_000,
            output: 2_000,
            ..TokenUsage::default()
        };
        let (_, stream) = fold(vec![
            StreamEvent::Chunk("The user is porting payments.".into()),
            StreamEvent::Usage(frame),
            StreamEvent::StreamDone,
        ]);
        let (_, usage) = stream.into_parts();
        assert_eq!(usage, vec![frame]);
    }
}
