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

/// The share of the window, in percent, the ceiling keeps free: room for the
/// last reply before compaction and for the summary itself — opencode's
/// reserve, which replaced codex's 90% because 20k of a 200k window could not
/// hold a long reply (`docs/background-compaction.md` *The numbers*).
const RESERVE_PERCENT: u64 = 15;

/// The least the ceiling keeps free, on a window with room for it (one at
/// least twice as large).
const RESERVE_MIN: u64 = 16_000;

/// How far before the ceiling a background summary starts: about what a
/// conversation grows by while a summary is written (30–60 s, and big reads
/// can add 25k in that time). Fixed, since it measures growth, not the
/// window.
const COMPACTION_LEAD: u64 = 60_000;

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

    /// A summary of the conversation started at this round top
    /// ([`crate::stream::StreamEvent::CompactionStarted`]): note where it cut
    /// the history, so its landing keeps what joins after
    /// (`docs/background-compaction.md`). What streamed before the cut joins
    /// the history first — the boundary flushes it, and so does this.
    pub fn note_compaction_started(&mut self) {
        self.flush_streaming_segment();
        self.compaction_start = Some((self.history_generation, self.history.len()));
    }

    /// Whether the history was rewritten since the summary in flight
    /// started — a `/clear`, a `/resume`, a rewind, an undone turn: its cut
    /// describes a history that is gone, so the boundary discards it
    /// (`docs/background-compaction.md` *A summary that outlives its turn*).
    #[must_use]
    pub fn compaction_is_stale(&self) -> bool {
        self.compaction_start
            .is_some_and(|(generation, _)| generation != self.history_generation)
    }

    /// Whether a summary's start is noted — one in flight has a cut in this
    /// history to land against ([`land_compaction`](Self::land_compaction)).
    #[must_use]
    pub const fn compaction_noted(&self) -> bool {
        self.compaction_start.is_some()
    }

    /// The summary in flight was discarded: forget where it started, and
    /// stop waiting on it.
    pub const fn forget_compaction(&mut self) {
        self.compaction_start = None;
        self.compaction_waiting = false;
    }

    /// The request reached the ceiling and waits for a summary (or stopped
    /// waiting on a failed one): the status verb reads `Compacting…` while it
    /// does ([`status_verb`](Self::status_verb)).
    pub const fn set_compaction_waiting(&mut self, waiting: bool) {
        self.compaction_waiting = waiting;
    }

    /// The verb the status line wears in place of its own: `Compacting…`
    /// while the ceiling waits on a summary, else the running task's
    /// `activeForm` ([`task_verb`](Self::task_verb)).
    #[must_use]
    pub fn status_verb(&self) -> Option<&str> {
        if self.compaction_waiting {
            Some(COMPACT_VERB)
        } else {
            self.task_verb()
        }
    }

    /// Land a summary of the conversation
    /// ([`crate::stream::StreamEvent::Compacted`], or one the boundary took
    /// at idle): append its marker, keeping the items that joined since
    /// [`note_compaction_started`](Self::note_compaction_started) — the
    /// rounds that ran while it was written — and bill `usage`, its requests'
    /// frames, to the session without letting them measure the conversation.
    /// `background` when it was written while the agent worked; `secs` how
    /// long it took. `None` — nothing appended — when the history was
    /// rewritten since it started, or no start was noted: its cut describes a
    /// history that is gone, or none the app knows.
    /// See `docs/background-compaction.md`.
    pub fn land_compaction(
        &mut self,
        summary: &str,
        secs: u64,
        background: bool,
        usage: &[crate::stream::TokenUsage],
    ) -> Option<Compaction> {
        self.compaction_waiting = false;
        // Its requests were made: billed whether or not it lands.
        for frame in usage {
            self.usage_since_launch.record(frame);
        }
        // No start noted means its `CompactionStarted` never reached the app
        // (dropped with the channel an Esc swaps, or a `/clear` drains), so
        // there is no cut to keep the items after.
        let (generation, start) = self.compaction_start.take()?;
        if generation != self.history_generation {
            return None;
        }
        self.flush_streaming_segment();
        let mut compaction = Compaction {
            summary: summary.trim().to_string(),
            timestamp: self.now_stamp(),
            before: self.context_used,
            after: 0,
            auto: true,
            secs,
            kept: self.history.len().saturating_sub(start),
            background,
        };
        self.history
            .push(HistoryItem::Compaction(compaction.clone()));
        self.refresh_context_used();
        compaction.after = self.context_used;
        if let Some(HistoryItem::Compaction(recorded)) = self.history.last_mut() {
            recorded.after = self.context_used;
        }
        Some(compaction)
    }

    /// What a turn hands its background compaction before it starts
    /// (`docs/background-compaction.md`): the window, the gauge, what the
    /// last compaction left, the tool schemas' estimate, the exact reminder
    /// the turn's context opens with, and the typed texts a splice keeps.
    pub fn compaction_baseline(&mut self) -> crate::llm::compaction::Baseline {
        self.sync_context_used();
        let reminder = crate::reminder::reminder_message(&[
            self.user_instructions.as_deref().unwrap_or_default(),
            self.listings.as_deref().unwrap_or_default(),
        ]);
        let cycle_start = self.history.iter().rev().find_map(|item| match item {
            HistoryItem::Compaction(compaction) => Some(compaction.after),
            _ => None,
        });
        crate::llm::compaction::Baseline {
            window: self.context_window,
            used: self.context_used,
            cycle_start: cycle_start.unwrap_or(0),
            overhead: self.tools_estimate,
            reminder: (!reminder.is_empty()).then_some(reminder),
            typed: crate::context::recent_typed_texts(&self.history),
        }
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
            kept: 0,
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
        // One attempt per user turn: no immediate auto re-trigger.
        self.auto_compact_blocked = true;
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

    /// Whether the loop should start an **auto-compact** turn at the next idle
    /// boundary: the `/settings` **Auto compact** knob is on, the window is
    /// known, the gauge is past the ceiling (`auto_compact_threshold`), no turn is in flight,
    /// the last compact attempt isn't still blocking (one per user turn), and
    /// the conversation derives a non-empty context to summarize. See
    /// `docs/compact.md` / `docs/settings.md`.
    #[must_use]
    pub fn should_auto_compact(&self) -> bool {
        // The `/settings` knob (docs/settings.md): off means the loop never
        // starts one on its own — `/compact` by hand still works.
        if !self.settings.auto_compact {
            return false;
        }
        let Some(window) = self.context_window else {
            return false;
        };
        if self.auto_compact_blocked || self.turn_active() || self.is_streaming() {
            return false;
        }
        if self.context_used <= auto_compact_threshold(window) {
            return false;
        }
        crate::context::derives_conversation(&self.history)
    }
}

/// The **ceiling** a `window`-token context window compacts at: the window
/// less [`RESERVE_PERCENT`], at least [`RESERVE_MIN`] where the window has
/// room for it — 170k of 200k, 850k of 1M. The idle trigger
/// ([`App::should_auto_compact`]), the background summary's splice
/// (`docs/background-compaction.md`) and the `/context` page's headroom
/// (`docs/context-usage.md`) all read it, so the page can never show a
/// threshold the loop does not act on.
#[must_use]
pub(crate) const fn auto_compact_threshold(window: u64) -> u64 {
    let share = window.saturating_mul(RESERVE_PERCENT) / 100;
    let floor = if window >= 2 * RESERVE_MIN {
        RESERVE_MIN
    } else {
        0
    };
    window.saturating_sub(if share > floor { share } else { floor })
}

/// Where a background summary starts for a `window`-token window whose
/// cycle began at `cycle_start` tokens (what the last compaction left, or
/// 0): [`COMPACTION_LEAD`] before the ceiling, but never more than half the
/// cycle's room, so a small window does not summarize again right after
/// every compaction (`docs/background-compaction.md` *The numbers*).
#[must_use]
pub(crate) const fn compaction_start(window: u64, cycle_start: u64) -> u64 {
    let ceiling = auto_compact_threshold(window);
    let half_room = ceiling.saturating_sub(cycle_start) / 2;
    ceiling
        - if half_room < COMPACTION_LEAD {
            half_room
        } else {
            COMPACTION_LEAD
        }
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
    /// How many history items just before the marker its summary does **not**
    /// describe: the rounds the agent ran while a background summary was
    /// being written, which the derivation keeps as they were
    /// (`docs/background-compaction.md` *The marker records `kept`*). 0 for
    /// `/compact` and an inline compaction — everything before the marker is
    /// replaced. Counted back from the marker, so a backtrack (which only ever
    /// cuts a suffix) and a resume leave it valid.
    pub kept: usize,
    /// Whether the summary was written in the background while the agent
    /// worked — the cell's `· background` tag — rather than by a turn of its
    /// own.
    pub background: bool,
}
