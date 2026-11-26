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
use crate::stream::{CancelToken, StreamEvent};

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
    let mut announced = None;
    loop {
        let window = slot.window()?;
        match decide(
            used,
            window,
            slot.cycle_start(),
            slot.job(),
            slot.failures(),
        ) {
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
}
