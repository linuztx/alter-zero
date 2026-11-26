//! `/compact` and auto-compaction: the summarization turn's buffer, the
//! [`Compaction`](super::Compaction) marker it appends, and the context gauge
//! that decides when to run it. See `docs/compact.md`.

use super::types::IMAGE_INPUT_TOKENS;
use super::*;

/// The fixed live-status verb for a `/compact` turn (the [`SHELL_VERB`]
/// pattern — not cycled): the status line reads `Compacting…`. It doubles as
/// the never-rendered done verb — a compact turn ends without a summary, the
/// `● Context compacted` cell being its record. See `docs/compact.md`.
pub const COMPACT_VERB: &str = "Compacting";

impl App {
    /// Begin a `/compact` turn (`docs/compact.md`), reusing the AI turn
    /// machinery like [`begin_shell`](App::begin_shell) so the summarization
    /// request gets the live status strip, `esc to interrupt`, and the
    /// mid-turn queueing for free:
    ///
    /// - an **empty** streaming buffer — [`is_streaming`](App::is_streaming)
    ///   is true (the strip shows, Enter queues, [`fail_stream`] works) but
    ///   the visible reply stays empty: chunks divert into the summary buffer
    ///   via [`push_chunk`](App::push_chunk), so the summary is never rendered
    ///   (codex parity) while the tally ticks;
    /// - a fixed-verb status ([`COMPACT_VERB`] → `Compacting…`) that never
    ///   rotates and does **not** advance the walk through the status verbs
    ///   (the verb cursor is untouched).
    ///
    /// The boundary spawns the summarize request; [`finish_compact`] lands the
    /// marker at `StreamDone`, while an interrupt/error/`/clear` drops the
    /// buffer — the old context stands (codex swaps only at the very end).
    ///
    /// [`fail_stream`]: App::fail_stream
    /// [`finish_compact`]: App::finish_compact
    pub fn begin_compact(&mut self, auto: bool) {
        self.streaming = Some(String::new());
        self.compact_buffer = Some(String::new());
        // A reply attempt an Esc cut short marks no floor in this turn
        // (`begin_stream`'s rule, docs/offline.md).
        self.attempt_floor = None;
        self.compact_auto = auto;
        self.compact_before = self.context_used;
        self.stream_preview_rows = 1;
        // Per-turn usage accumulators reset with every turn machinery start.
        self.turn_usage_tokens = 0;
        self.turn_usage_cached = 0;
        self.turn_usage_cache_write = 0;
        // The summarizer's own thinking is never shown (the turn is invisible
        // by design) — `begin_reasoning` no-ops while the buffer below is
        // open; this only clears anything a previous turn abandoned.
        self.drop_reasoning();
        self.status = Some(TurnStatus {
            verb: COMPACT_VERB,
            // Never rendered: a compact turn ends without a summary (the
            // `● Context compacted` cell is its record).
            done_verb: COMPACT_VERB,
            // It names the operation, so the clock never moves it on.
            rotates_from: None,
            tokens: 0,
            arrow: TokenArrow::Down,
            elapsed: Duration::ZERO,
            thinking: None,
            shell: false,
            retry: None,
            offline: None,
            tip: None,
        });
    }

    /// Whether a `/compact` turn is in flight (the summary buffer is open).
    #[must_use]
    pub const fn is_compacting(&self) -> bool {
        self.compact_buffer.is_some()
    }

    /// End a `/compact` turn: take the streamed summary, append the
    /// [`HistoryItem::Compaction`] marker (an *append* — the transcript, the
    /// recorder watermark, and the checkpoint keys all stay valid), and clear
    /// the turn state with **no** `Done for Ns` summary. Returns the appended
    /// marker for the boundary to commit the `● Context compacted` cell, or
    /// `None` when no compact turn was in flight. `elapsed_secs` is the
    /// summarization turn's wall-clock from the boundary's clock (the
    /// [`take_turn_summary`](App::take_turn_summary) shape) — recorded on the
    /// marker so the cell shows how long the compaction worked. The context
    /// derivation ([`crate::context::context_messages`]) applies the
    /// compacted shape from the marker on. See `docs/compact.md`.
    pub fn finish_compact(&mut self, elapsed_secs: u64) -> Option<Compaction> {
        let summary = self.compact_buffer.take()?;
        self.streaming = None;
        self.tool_drafts.clear();
        self.status = None;
        let mut compaction = Compaction {
            // A model reply often ends with a trailing newline; the bridge
            // adds its own separators, so store the summary trimmed.
            summary: summary.trim().to_string(),
            timestamp: self.now_stamp(),
            before: self.compact_before,
            after: 0,
            auto: self.compact_auto,
            secs: elapsed_secs,
            keep_from: None,
            background: false,
        };
        self.history
            .push(HistoryItem::Compaction(compaction.clone()));
        // The gauge drops by the estimate of what the summary replaced
        // (codex's recompute_token_usage) — what the provider counts beyond
        // the estimate, its framing and its own rendering of the tool
        // schemas, stays — and the marker records the shrink.
        self.refresh_context_used();
        // The newest frame is the summarizer's request, which described the
        // conversation the marker just replaced (`docs/context-usage.md`).
        self.last_usage = None;
        compaction.after = self.context_used;
        if let Some(HistoryItem::Compaction(recorded)) = self.history.last_mut() {
            recorded.after = self.context_used;
        }
        Some(compaction)
    }

    /// Inject the active model's context window in tokens (the boundary's
    /// `/v1/models` `context_length`, the saved settings, or the
    /// `ALTER_ZERO_CONTEXT_WINDOW` override) — `None` (or a meaningless 0)
    /// hides the footer gauge and disables auto-compact. See `docs/compact.md`.
    pub fn set_context_window(&mut self, window: Option<u64>) {
        self.context_window = window.filter(|&w| w > 0);
    }

    /// The active model's context window in tokens, when known.
    #[must_use]
    pub const fn context_window(&self) -> Option<u64> {
        self.context_window
    }

    /// The current context size in tokens (see the field doc) — the footer
    /// gauge's numerator.
    #[must_use]
    pub const fn context_used(&self) -> u64 {
        self.context_used
    }

    /// Estimate the whole context the next request carries, and the
    /// pictures in it: the system prompt and the tool schemas, plus every
    /// derived context message (the `<system-reminder>` among them — the
    /// AGENTS.md instructions ride every request) — its text, a flat
    /// [`IMAGE_INPUT_TOKENS`] per attachment, its tool calls. Each text's
    /// count is remembered, so the re-estimate every usage report asks for
    /// counts only what is new. A usage report's count is set beside it, and
    /// a history the report no longer describes (compaction, `/clear`,
    /// backtrack, `/resume`) is re-read from it (`docs/token-estimate.md`).
    ///
    /// **An empty conversation estimates zero.** The system prompt and the
    /// standing instructions do ride the next request, but a *booted* session
    /// carries those same constants and the gauge reads 0 there (nothing
    /// seeds it until a turn), so counting them after a `/clear` showed the
    /// same empty state as two different numbers — and the leftover-looking
    /// one (`139/1M` under a blank screen) read as conversation that hadn't
    /// really gone. The gauge measures the conversation; with none, it is 0,
    /// and `/clear` lands exactly where a fresh session starts. The predicate
    /// is the one `/compact` uses for `Nothing to compact`, so the two never
    /// disagree about whether anything is there.
    pub(super) fn estimate_context(&mut self) -> (u64, usize) {
        if !crate::context::derives_conversation(&self.history) {
            return (0, 0);
        }
        let messages = crate::context::context_messages_full(
            self.user_instructions.as_deref(),
            self.listings.as_deref(),
            &self.history,
        );
        let memo = &mut self.context_memo;
        let mut count = |text: &str| memo.count(text, &count_tokens);
        let prompt = self.system_prompt.as_deref().map_or(0, &mut count);
        let total: usize = prompt
            + messages
                .iter()
                .map(|message| message_tokens(message, &mut count))
                .sum::<usize>();
        memo.finish_pass();
        let images = messages.iter().map(|message| message.images.len()).sum();
        let total = u64::try_from(total)
            .unwrap_or(u64::MAX)
            .saturating_add(self.tools_estimate);
        (total, images)
    }

    /// The history's identity for the gauge: committed items are immutable,
    /// and every change but an append bumps the generation.
    pub(super) fn history_signature(&self) -> (u64, usize) {
        (self.history_generation, self.history.len())
    }

    /// Re-read the gauge for the history as it stands (the
    /// [`context_used`](Self::context_used) field doc): zero for a
    /// conversation that derives nothing; the meter's base plus the
    /// calibrated estimate of what joined since, when the history only grew
    /// past it; otherwise — the history moved under the base — the whole
    /// context re-estimated, the base moved by the change, and the result
    /// taken as the new base (`docs/token-estimate.md`).
    pub(super) fn refresh_context_used(&mut self) {
        let now = self.history_signature();
        self.context_synced = Some(now);
        if !crate::context::derives_conversation(&self.history) {
            self.context_used = 0;
            return;
        }
        if let Some(joined) = self.history_since_base() {
            let added = estimate_messages_tokens(&crate::context::context_messages(joined));
            self.context_used = self
                .context_meter
                .reading_after(u64::try_from(added).unwrap_or(u64::MAX));
            return;
        }
        let (estimate, _) = self.estimate_context();
        self.context_meter.rebase(estimate);
        self.context_base_at = Some(now);
        self.context_used = self.context_meter.reading(estimate);
    }

    /// What joined the history after the meter's base, when it only grew
    /// since: the same generation, and no compaction among it — a compaction
    /// replaces everything before it in the derived context.
    fn history_since_base(&self) -> Option<&[HistoryItem]> {
        let (generation, length) = self.context_base_at?;
        if generation != self.history_generation {
            return None;
        }
        let joined = self.history.get(length..)?;
        (!joined
            .iter()
            .any(|item| matches!(item, HistoryItem::Compaction(_))))
        .then_some(joined)
    }

    /// Re-read the gauge when the history moved since it was last read — the
    /// loop bottom's call, so what joins between usage reports (a message
    /// sent, a tool's output) shows before the next request is counted. Two
    /// integer compares when nothing moved.
    pub fn sync_context_used(&mut self) {
        if self.context_synced != Some(self.history_signature()) {
            self.refresh_context_used();
        }
    }

    /// What the provider's usage reports have taught about its tokens
    /// against the estimate, for the conversation **on screen** — the viewed
    /// agent's inside its session view, the main session's otherwise. The
    /// `/context` page and the Ctrl+D view count in it
    /// (`docs/token-estimate.md`).
    #[must_use]
    pub fn context_calibration(&self) -> crate::tokens::Calibration {
        match self.viewed_agent() {
            Some(run) => run.context_calibration(),
            None => self.context_meter.calibration(),
        }
    }

    /// The window the running turn compacts against
    /// (`docs/background-compaction.md` *The lines on the gauge*) — the
    /// gauge's, while the `/settings` **Auto compact** knob is on; `None`
    /// otherwise, and then nothing compacts on its own (`/compact` by hand
    /// still works). The boundary configures the turn's slot with it.
    #[must_use]
    pub const fn compaction_window(&self) -> Option<u64> {
        if self.settings.auto_compact {
            self.context_window
        } else {
            None
        }
    }

    /// Whether a background compaction may start or land **between turns**:
    /// no turn in flight (a running one compacts at its own round tops) and a
    /// conversation to summarize.
    #[must_use]
    pub fn can_compact_idle(&self) -> bool {
        !self.turn_active()
            && !self.is_streaming()
            && crate::context::derives_conversation(&self.history)
    }

    /// A background summary started as job `id`: its coverage ends here, at
    /// everything recorded so far (`docs/background-compaction.md` *The
    /// lifecycle*). The boundary calls it where the turn's
    /// [`StreamEvent::CompactionStart`] lands, so no item of the round about
    /// to start is in it.
    pub fn begin_background_compaction(&mut self, id: u64) {
        self.background_compaction = Some(BackgroundCompaction {
            id,
            keep_from: self.history.len(),
            epoch: self.history_epoch,
            summary: None,
            secs: 0,
            waiting: false,
        });
    }

    /// The background summary being written or held, if any.
    #[must_use]
    pub const fn background_compaction(&self) -> Option<&BackgroundCompaction> {
        self.background_compaction.as_ref()
    }

    /// Whether the history moved under the background summary, so it covers
    /// a conversation that is gone and must never land (codex's generation
    /// check, narrowed): the conversation was replaced or rewound since it
    /// started (a `/clear`, a `/resume`, a backtrack), or an item it covers
    /// was taken back (the turn that started it undone before anything
    /// streamed). A re-render, an agent's entry updated in place or a failed
    /// attempt withdrawn leaves it standing.
    #[must_use]
    pub fn background_compaction_stale(&self) -> bool {
        self.background_compaction
            .as_ref()
            .is_some_and(|job| self.covers_gone(job))
    }

    /// Whether `job`'s coverage is no longer this conversation's.
    fn covers_gone(&self, job: &BackgroundCompaction) -> bool {
        job.epoch != self.history_epoch || job.keep_from > self.history.len()
    }

    /// Job `id`'s summary is written, in `secs`: hold it until it lands.
    /// `false` — and nothing kept — for another job or a stale one.
    pub fn background_compaction_written(&mut self, id: u64, summary: String, secs: u64) -> bool {
        if self.background_compaction_stale() {
            return false;
        }
        match &mut self.background_compaction {
            Some(job) if job.id == id => {
                job.summary = Some(summary.trim().to_string());
                job.secs = secs;
                true
            }
            _ => false,
        }
    }

    /// Land job `id`: append its anchored marker
    /// (`docs/background-compaction.md` *The marker*) — everything before
    /// its anchor summarized, everything from there on verbatim — re-read the
    /// gauge, and return the marker for the boundary's `● Context compacted
    /// · … · background` cell. `None`, and nothing appended, for another job,
    /// a stale one or one not written yet.
    pub fn land_background_compaction(&mut self, id: u64) -> Option<Compaction> {
        let job = self.background_compaction.as_ref()?;
        if job.id != id || self.covers_gone(job) {
            return None;
        }
        let summary = job.summary.clone()?;
        let job = self.background_compaction.take()?;
        // What the summary replaces is the context at the swap: the turn kept
        // working while it was written.
        self.sync_context_used();
        let mut compaction = Compaction {
            summary,
            timestamp: self.now_stamp(),
            before: self.context_used,
            after: 0,
            auto: true,
            secs: job.secs,
            keep_from: Some(job.keep_from),
            background: true,
        };
        self.history
            .push(HistoryItem::Compaction(compaction.clone()));
        // The gauge re-reads the compacted derivation, and the newest frame
        // described the conversation the marker just replaced
        // (`docs/context-usage.md`) — `finish_compact`'s rule.
        self.refresh_context_used();
        self.last_usage = None;
        compaction.after = self.context_used;
        if let Some(HistoryItem::Compaction(recorded)) = self.history.last_mut() {
            recorded.after = self.context_used;
        }
        Some(compaction)
    }

    /// Forget the background summary — its history moved, or it failed —
    /// returning its id for the boundary to forget it in the turn's slot.
    pub fn drop_background_compaction(&mut self) -> Option<u64> {
        self.background_compaction.take().map(|job| job.id)
    }

    /// A turn reached its ceiling with job `id` unwritten and waits for it —
    /// the status line's fixed `Compacting…` until the swap ([`live_verb`]).
    ///
    /// [`live_verb`]: App::live_verb
    pub fn set_compaction_waiting(&mut self, id: u64) {
        if let Some(job) = &mut self.background_compaction
            && job.id == id
        {
            job.waiting = true;
        }
    }

    /// A background summary's usage frame: billed, so it joins the
    /// since-launch ledger (`docs/context-usage.md`) — and only that, its
    /// request being the summarizer's rather than the conversation's.
    pub fn record_compaction_usage(&mut self, usage: &crate::stream::TokenUsage) {
        self.usage_since_launch.record(usage);
    }

    /// No turn waits for the background summary any more — the waiting turn
    /// ended (an Esc, an error) with the summary still unwritten. The next
    /// turn wears its own verb until it reaches its ceiling and waits again.
    pub fn end_compaction_wait(&mut self) {
        if let Some(job) = &mut self.background_compaction {
            job.waiting = false;
        }
    }

    /// The verb the live status line wears in place of the turn's own:
    /// `Compacting…` while the turn waits for a background summary at its
    /// ceiling, else the running task's activeForm (`docs/task-tools.md`),
    /// else none.
    #[must_use]
    pub fn live_verb(&self) -> Option<&str> {
        if self
            .background_compaction
            .as_ref()
            .is_some_and(|job| job.waiting)
        {
            return Some(COMPACT_VERB);
        }
        self.task_verb()
    }

    /// How long the background summary has been written for — the
    /// boundary's clock, injected per draw like the status line's
    /// (`set_status_times`'s pattern).
    pub const fn set_compaction_elapsed(&mut self, elapsed: Option<Duration>) {
        self.compaction_elapsed = elapsed;
    }

    /// What the strip's row under the status line says about the background
    /// summary (`docs/background-compaction.md` *What the user sees*): being
    /// written and for how long, or written and held until the context
    /// reaches the ceiling's share of the window.
    #[must_use]
    pub fn compaction_row(&self) -> Option<CompactionRow> {
        let job = self.background_compaction.as_ref()?;
        if job.summary.is_some() {
            let window = self.context_window.filter(|&window| window > 0)?;
            return Some(CompactionRow::Held {
                percent: crate::compaction::ceiling(window).saturating_mul(100) / window,
            });
        }
        Some(CompactionRow::Writing {
            elapsed: self.compaction_elapsed.unwrap_or_default(),
        })
    }
}

/// Where a running turn swaps a summary in for a `window`-token context
/// window: background compaction's ceiling (`compaction::ceiling`). The
/// `/context` page's reserve (`docs/context-usage.md`) reads it, so the page
/// can never show a line the turn does not act on.
#[must_use]
pub(crate) const fn auto_compact_threshold(window: u64) -> u64 {
    crate::compaction::ceiling(window)
}

/// The estimate of a derived context: every message's text, a flat
/// `IMAGE_INPUT_TOKENS` per attachment, and each tool call's name and
/// arguments. The one counting rule behind both context gauges — the main
/// session's (`App::estimate_context`, which adds the system prompt
/// and hands in the fragments-led derivation) and a subagent's
/// ([`crate::agents::AgentRun`], over its own transcript) — so the two can
/// never disagree about what a message costs (`docs/agent-context-gauge.md`).
#[must_use]
pub(crate) fn estimate_messages_tokens(messages: &[crate::context::ContextMessage]) -> usize {
    let mut count = count_tokens;
    messages
        .iter()
        .map(|message| message_tokens(message, &mut count))
        .sum()
}

/// [`estimate_messages_tokens`]' rule for **one** message, counting with
/// `count`: its text, a flat `IMAGE_INPUT_TOKENS` per attachment, and each
/// tool call's name and arguments. The Ctrl+D view labels every entry with
/// it (`docs/context.md`), handing in a memoized counter so a rebuild
/// tokenizes only what is new — and since the gauge sums the very same rule,
/// the entries add up to what the footer reads.
pub(crate) fn message_tokens(
    message: &crate::context::ContextMessage,
    count: &mut dyn FnMut(&str) -> usize,
) -> usize {
    let calls: usize = message
        .tool_calls
        .iter()
        .map(|call| count(&call.name) + count(&call.arguments))
        .sum();
    count(&message.text) + message.images.len() * IMAGE_INPUT_TOKENS + calls
}

/// A `/compact` marker's payload: the model-written handoff summary the
/// context derivation bridges into every later request, codex's local
/// compaction (`docs/compact.md`). Renders as the `● Context compacted` cell
/// (the summary body expands in the Ctrl+O transcript only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    /// The compact turn's full streamed reply — the handoff summary (may be
    /// empty when the model streamed nothing; the derivation substitutes
    /// codex's "(no summary available)").
    pub summary: String,
    /// Wall-clock stamp (recorded like every item's; never displayed).
    pub timestamp: String,
    /// The context gauge when the compaction began (tokens) — the cell shows
    /// `· {before} → {after} tokens`. 0 = unknown (an old rollout), hiding
    /// the clause.
    pub before: u64,
    /// The re-estimated context size right after the compaction (tokens).
    pub after: u64,
    /// Whether this compaction was **auto-triggered** by the context gauge
    /// crossing the threshold (the cell appends `· auto`), vs the manual
    /// `/compact` command. See `docs/compact.md`.
    pub auto: bool,
    /// How long the summarization turn ran, in whole seconds — the cell's
    /// ` · {elapsed}` clause ([`format_elapsed`]-humanized). 0 = unknown (an
    /// old rollout), hiding the clause.
    pub secs: u64,
    /// Where the summary's coverage ends: the history index the verbatim
    /// tail starts at. A background compaction is anchored where its summary
    /// started, and the items recorded from there until the marker landed
    /// stay verbatim; `None` (a manual or pre-background compaction) covers
    /// everything before the marker. A value past the marker is the marker
    /// (`docs/background-compaction.md` *The marker*).
    pub keep_from: Option<usize>,
    /// Whether the summary was written in the background while the agent
    /// kept working — the cell's `· background` in place of `· auto`.
    pub background: bool,
}

/// A summary being written in the background, or written and held until a
/// turn swaps it in (`docs/background-compaction.md` *The lifecycle*).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundCompaction {
    /// The job's id in the turn's `compaction::CompactionSlot`.
    pub id: u64,
    /// Where the summary's coverage ends — the history's length when it
    /// started; the marker's anchor.
    pub keep_from: usize,
    /// The conversation's epoch when it started: a `/clear`, a `/resume` or
    /// a backtrack since makes the job stale.
    pub epoch: u64,
    /// The summary, once written.
    pub summary: Option<String>,
    /// How long the summary took to write — the cell's elapsed.
    pub secs: u64,
    /// Whether a turn is waiting for it at its ceiling.
    pub waiting: bool,
}

/// What the strip's row under the status line says about a background
/// summary (`docs/background-compaction.md` *What the user sees*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionRow {
    /// Being written, for this long.
    Writing {
        /// How long it has been written for.
        elapsed: Duration,
    },
    /// Written and held: it swaps in once the context reaches this share of
    /// the window, in percent.
    Held {
        /// The ceiling's share of the window.
        percent: u64,
    },
}
