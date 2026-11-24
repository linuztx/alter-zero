//! Compaction policy, and the handle the session shares with its backends
//! (`docs/compact.md`).
//!
//! Two halves, both free of I/O:
//!
//! - **the numbers** — where in the context window a summary starts being
//!   written and where nothing more is sent until the context is smaller
//!   ([`soft_threshold`], [`hard_threshold`]), and the budgets every
//!   compaction is cut to ([`user_message_budget`], [`summary_input_budget`],
//!   [`summary_max_tokens`], [`min_gain`]). One place, so the footer gauge,
//!   the `/context` page, the idle trigger and the agent loop's own trigger
//!   can never act on different thresholds;
//! - **[`CompactionControl`]** — what crosses the thread boundary. The event
//!   loop owns the facts a running turn's compaction needs and cannot see
//!   from its own thread: whether the user has auto compaction on, the
//!   model's window, a `/compact` typed while the turn runs, and the parts of
//!   the compacted context only the conversation's history knows
//!   ([`CompactionSeed`]). It is the [`crate::steer::SteerQueue`] shape:
//!   cloneable, every clone the same handle, re-attached on every backend
//!   build so a `/model` switch keeps it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Codex's `COMPACT_USER_MESSAGE_MAX_TOKENS`: the approx-token budget for the
/// recent user messages replayed before the bridge, and the ceiling
/// [`user_message_budget`] scales under. What a marker recorded before the
/// budget was carries.
pub const DEFAULT_USER_MESSAGE_BUDGET: usize = 20_000;

/// The share of the window past which a summary starts being written **in
/// the background**, while the agent keeps working: eight tenths. Early
/// enough that the summary is usually ready before [`hard_threshold`] is
/// reached, late enough that a compaction — which costs a request over the
/// whole context — is not made before it is needed.
const SOFT_NUMERATOR: u64 = 8;

/// Codex's `auto_compact_token_limit`: **nine tenths** of the window. Past
/// it nothing more is sent until the context is smaller.
const HARD_NUMERATOR: u64 = 9;

const THRESHOLD_DENOMINATOR: u64 = 10;

/// Where a summary starts being written for a `window`-token context window
/// — in the background, the agent still working (`docs/compact.md`
/// *Compacting while the agent works*).
#[must_use]
pub const fn soft_threshold(window: u64) -> u64 {
    share(window, SOFT_NUMERATOR)
}

/// Where the context **must** shrink before another request goes out:
/// codex's 90% (`ModelInfo::auto_compact_token_limit`). The idle trigger
/// ([`crate::app::App::should_auto_compact`]), the agent loop's own and the
/// `/context` page's headroom all read it.
#[must_use]
pub const fn hard_threshold(window: u64) -> u64 {
    share(window, HARD_NUMERATOR)
}

/// `numerator` tenths of `window`, exact for every window: widened first, so
/// no size makes the two thresholds meet.
const fn share(window: u64, numerator: u64) -> u64 {
    // The quotient is at most `window`, so it always fits.
    #[allow(clippy::cast_possible_truncation)]
    {
        (window as u128 * numerator as u128 / THRESHOLD_DENOMINATOR as u128) as u64
    }
}

/// The approx-token budget for the user messages a compaction keeps
/// verbatim: an eighth of the window, never more than codex's
/// [`DEFAULT_USER_MESSAGE_BUDGET`] — which a 50k-token window would spend
/// two fifths of itself on — and never so little that the newest request is
/// cut. An unknown window takes the default.
#[must_use]
pub fn user_message_budget(window: Option<u64>) -> usize {
    const FLOOR: usize = 500;
    window.map_or(DEFAULT_USER_MESSAGE_BUDGET, |window| {
        usize::try_from(window / 8)
            .unwrap_or(usize::MAX)
            .clamp(FLOOR, DEFAULT_USER_MESSAGE_BUDGET)
    })
}

/// How many tokens of conversation the summarizer is handed: six tenths of
/// the window, which leaves room for the system prompt, the instructions,
/// the summary itself and the estimate's own error — so the request that
/// makes room can never be the one that does not fit. An unknown window
/// takes a size every current model holds.
#[must_use]
pub fn summary_input_budget(window: Option<u64>) -> usize {
    const FLOOR: usize = 2_000;
    const UNKNOWN: usize = 96_000;
    window.map_or(UNKNOWN, |window| {
        usize::try_from(window.saturating_mul(6) / 10)
            .unwrap_or(usize::MAX)
            .max(FLOOR)
    })
}

/// The longest summary a compaction keeps, in tokens: a tenth of the window
/// between a thousand and twelve thousand. A summary is a few hundred to a
/// couple of thousand tokens; one past this is cut in its middle rather than
/// allowed to undo the compaction it came from.
#[must_use]
pub fn summary_max_tokens(window: Option<u64>) -> usize {
    const FLOOR: usize = 1_000;
    const CEILING: usize = 12_000;
    window.map_or(CEILING, |window| {
        usize::try_from(window / 10)
            .unwrap_or(usize::MAX)
            .clamp(FLOOR, CEILING)
    })
}

/// The least a compaction must be able to take out of the context to be
/// worth making: a twentieth of the window. Under it there is nothing to
/// compact — the system prompt, the tools and the last summary are what
/// fill the window — and summarizing a summary would only cost a request.
#[must_use]
pub fn min_gain(window: Option<u64>) -> u64 {
    const FLOOR: u64 = 500;
    const UNKNOWN: u64 = 2_000;
    window.map_or(UNKNOWN, |window| (window / 20).max(FLOOR))
}

/// What only the conversation's history knows about its compacted shape
/// (`docs/compact.md` *The compacted shape, on both sides*): the fragment
/// the window opens with, and the user's own messages. The derived context
/// merges both into its first message, so the agent loop — which holds the
/// messages as the wire carries them — cannot read either back out, and is
/// handed them here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompactionSeed {
    /// The `<system-reminder>` the window opens with, as the derivation
    /// renders it; `None` for a session that sends none.
    pub leading: Option<String>,
    /// The user's typed messages so far, oldest first — as many of the
    /// newest as any budget can keep ([`crate::context::recent_user_texts`]).
    pub user_texts: Vec<String>,
}

/// What the event loop tells a running turn about compaction. Every clone is
/// the same handle; a default one has auto compaction **off**, so a backend
/// nobody handed a handle to never compacts on its own.
#[derive(Clone, Debug, Default)]
pub struct CompactionControl {
    inner: Arc<ControlInner>,
}

#[derive(Debug, Default)]
struct ControlInner {
    /// The `/settings` **Auto compact** row.
    auto: AtomicBool,
    /// The session model's context window; 0 while it is unknown.
    window: AtomicU64,
    /// `ALTER_ZERO_CONTEXT_WINDOW`; 0 when unset.
    override_window: AtomicU64,
    /// A `/compact` asked for while a turn was running.
    requested: AtomicBool,
    /// The lead conversation's seed, set as each of its turns starts.
    seed: Mutex<CompactionSeed>,
}

impl CompactionControl {
    /// A handle with auto compaction on or off and nothing else known.
    #[must_use]
    pub fn new(auto: bool) -> Self {
        let control = Self::default();
        control.set_auto(auto);
        control
    }

    /// The `/settings` **Auto compact** row moved.
    pub fn set_auto(&self, auto: bool) {
        self.inner.auto.store(auto, Ordering::Relaxed);
    }

    /// Whether a turn may compact on its own.
    #[must_use]
    pub fn auto(&self) -> bool {
        self.inner.auto.load(Ordering::Relaxed)
    }

    /// The session model's window, as the footer gauge reads it — `None` (or
    /// a meaningless 0) while it is unknown.
    pub fn set_window(&self, window: Option<u64>) {
        self.inner
            .window
            .store(window.unwrap_or_default(), Ordering::Relaxed);
    }

    /// The session model's window, when known.
    #[must_use]
    pub fn window(&self) -> Option<u64> {
        Some(self.inner.window.load(Ordering::Relaxed)).filter(|&window| window > 0)
    }

    /// `ALTER_ZERO_CONTEXT_WINDOW`, which outranks whatever a provider
    /// reports — and is the only window a subagent pinned to another model
    /// has (`docs/agent-context-gauge.md`).
    pub fn set_override_window(&self, window: Option<u64>) {
        self.inner
            .override_window
            .store(window.unwrap_or_default(), Ordering::Relaxed);
    }

    /// The window a conversation runs against: the session's own when it
    /// `inherits` the session's model, else only the override — a model
    /// this session never listed has no other.
    #[must_use]
    pub fn window_for(&self, inherits: bool) -> Option<u64> {
        if inherits {
            self.window()
        } else {
            Some(self.inner.override_window.load(Ordering::Relaxed)).filter(|&window| window > 0)
        }
    }

    /// `/compact` was run while a turn is in flight: the turn compacts at
    /// its next step.
    pub fn request(&self) {
        self.inner.requested.store(true, Ordering::Relaxed);
    }

    /// Take the pending `/compact`, if one is waiting — once.
    #[must_use]
    pub fn take_request(&self) -> bool {
        self.inner.requested.swap(false, Ordering::Relaxed)
    }

    /// Whether a `/compact` is still waiting to be taken.
    #[must_use]
    pub fn requested(&self) -> bool {
        self.inner.requested.load(Ordering::Relaxed)
    }

    /// Hand the lead conversation's seed to the turn about to start.
    pub fn set_seed(&self, seed: CompactionSeed) {
        if let Ok(mut held) = self.inner.seed.lock() {
            *held = seed;
        }
    }

    /// The seed the turn now starting reads. A poisoned lock reads as an
    /// empty one: a compaction that keeps no user message verbatim still
    /// carries them in its summary.
    #[must_use]
    pub fn seed(&self) -> CompactionSeed {
        self.inner
            .seed
            .lock()
            .map(|seed| seed.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_starts_at_eight_tenths_and_must_land_by_nine() {
        assert_eq!(soft_threshold(100_000), 80_000);
        assert_eq!(hard_threshold(100_000), 90_000);
        assert_eq!(soft_threshold(50_000), 40_000);
        assert_eq!(hard_threshold(50_000), 45_000);
        // No window is too large to take a share of.
        assert!(soft_threshold(u64::MAX) < hard_threshold(u64::MAX));
    }

    #[test]
    fn the_kept_user_messages_take_an_eighth_of_a_small_window() {
        // Codex's fixed 20k would be two fifths of a 50k window.
        assert_eq!(user_message_budget(Some(50_000)), 6_250);
        assert_eq!(user_message_budget(Some(128_000)), 16_000);
    }

    #[test]
    fn the_kept_user_messages_never_take_more_than_codexs_budget() {
        assert_eq!(user_message_budget(Some(200_000)), 20_000);
        assert_eq!(user_message_budget(Some(1_000_000)), 20_000);
        assert_eq!(user_message_budget(None), DEFAULT_USER_MESSAGE_BUDGET);
    }

    #[test]
    fn a_tiny_window_still_keeps_the_newest_request() {
        assert_eq!(user_message_budget(Some(100)), 500);
    }

    #[test]
    fn the_summarizer_reads_six_tenths_of_the_window() {
        assert_eq!(summary_input_budget(Some(50_000)), 30_000);
        assert_eq!(summary_input_budget(Some(200_000)), 120_000);
        assert_eq!(summary_input_budget(Some(100)), 2_000);
        assert_eq!(summary_input_budget(None), 96_000);
    }

    #[test]
    fn a_summary_is_held_to_a_tenth_of_the_window() {
        assert_eq!(summary_max_tokens(Some(50_000)), 5_000);
        assert_eq!(summary_max_tokens(Some(4_000)), 1_000);
        assert_eq!(summary_max_tokens(Some(1_000_000)), 12_000);
        assert_eq!(summary_max_tokens(None), 12_000);
    }

    #[test]
    fn a_compaction_must_be_able_to_free_a_twentieth_of_the_window() {
        assert_eq!(min_gain(Some(50_000)), 2_500);
        assert_eq!(min_gain(Some(1_000)), 500);
        assert_eq!(min_gain(None), 2_000);
    }

    #[test]
    fn a_default_handle_never_compacts_on_its_own() {
        let control = CompactionControl::default();
        assert!(!control.auto());
        assert_eq!(control.window(), None);
        assert!(!control.requested());
    }

    #[test]
    fn every_clone_is_the_same_handle() {
        let control = CompactionControl::new(true);
        let seen = control.clone();
        control.set_window(Some(50_000));
        control.set_auto(false);
        assert_eq!(seen.window(), Some(50_000));
        assert!(!seen.auto());
    }

    #[test]
    fn an_unknown_window_reads_as_none() {
        let control = CompactionControl::new(true);
        control.set_window(Some(0));
        assert_eq!(control.window(), None);
        control.set_window(Some(8_000));
        control.set_window(None);
        assert_eq!(control.window(), None);
    }

    #[test]
    fn a_pinned_model_has_only_the_override_for_a_window() {
        let control = CompactionControl::new(true);
        control.set_window(Some(200_000));
        assert_eq!(control.window_for(true), Some(200_000));
        assert_eq!(control.window_for(false), None);
        control.set_override_window(Some(50_000));
        assert_eq!(control.window_for(false), Some(50_000));
    }

    #[test]
    fn a_request_is_taken_once() {
        let control = CompactionControl::new(false);
        control.request();
        assert!(control.requested());
        assert!(control.take_request());
        assert!(!control.take_request(), "taken once");
        assert!(!control.requested());
    }

    #[test]
    fn the_seed_set_for_a_turn_is_the_seed_it_reads() {
        let control = CompactionControl::new(true);
        let seed = CompactionSeed {
            leading: Some("<system-reminder>be terse</system-reminder>".to_string()),
            user_texts: vec!["first".to_string(), "second".to_string()],
        };
        control.clone().set_seed(seed.clone());
        assert_eq!(control.seed(), seed);
    }
}
