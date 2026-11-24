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
        // An idle compaction covers everything before its marker, under the
        // budget the window gives it now.
        self.compact_cut = None;
        self.compact_user_budget = None;
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
        // An adopted background compaction (its agent's turn ended before
        // the summary did) keeps the cut and the budget of its checkpoint.
        let cut = self.compact_cut.take();
        let user_budget = self.compact_user_budget.take().unwrap_or_else(|| {
            u64::try_from(crate::context::compact_user_budget(self.context_window))
                .unwrap_or(u64::MAX)
        });
        let compaction = Compaction {
            // A model reply often ends with a trailing newline; the bridge
            // adds its own separators, so store the summary trimmed.
            summary: summary.trim().to_string(),
            timestamp: self.now_stamp(),
            before: self.compact_before,
            after: 0,
            auto: self.compact_auto,
            secs: elapsed_secs,
            cut,
            user_budget,
        };
        let compaction = self.land_marker(compaction);
        // With nothing after its cut, the whole context is the head — the
        // smallest compacting can make it. An adopted one's tail is the
        // agent's last rounds, so its head is counted alone.
        let head = if cut.is_none() {
            compaction.after
        } else {
            self.head_tokens(&self.marker_head(&compaction))
        };
        self.note_floor(head);
        Some(compaction)
    }

    /// Append `compaction`'s marker at the end of the history and land it:
    /// the gauge drops by the estimate of what the summary replaced (codex's
    /// recompute_token_usage) — what the provider counts beyond the estimate,
    /// its framing and its own rendering of the tool schemas, stays — and the
    /// marker records the shrink; the idle trigger waits for the
    /// conversation to move on. Returns the marker as recorded.
    fn land_marker(&mut self, mut compaction: Compaction) -> Compaction {
        self.history
            .push(HistoryItem::Compaction(compaction.clone()));
        self.refresh_context_used();
        // The newest frame described the conversation the marker just
        // replaced (`docs/context-usage.md`).
        self.last_usage = None;
        compaction.after = self.context_used;
        if let Some(HistoryItem::Compaction(recorded)) = self.history.last_mut() {
            recorded.after = self.context_used;
        }
        self.compact_watermark = Some(self.history_signature());
        compaction
    }

    /// Remember what the last compaction's head cost, the smallest
    /// compacting can make this conversation under this window.
    fn note_floor(&mut self, head: u64) {
        self.compact_floor = self.context_window.map(|window| CompactFloor {
            generation: self.history_generation,
            window,
            head,
        });
    }

    /// The head `compaction`'s marker derives — the reminder, the recent
    /// user messages and the bridge — from the history as it stands.
    fn marker_head(&self, compaction: &Compaction) -> Vec<crate::context::ContextMessage> {
        let cut = compaction.cut.and_then(|id| self.covered_end(id));
        let covered = &self.history[..cut.unwrap_or(self.history.len())];
        crate::context::compacted_head(
            self.user_instructions.as_deref(),
            self.listings.as_deref(),
            covered,
            &compaction.summary,
            usize::try_from(compaction.user_budget).unwrap_or(usize::MAX),
        )
    }

    /// Where the summary of the [`CompactionCut`] with `id` ends: its
    /// position, less the items it reaches back over.
    fn covered_end(&self, id: u64) -> Option<usize> {
        let at = self.cut_index(id)?;
        let back = match &self.history[at] {
            HistoryItem::CompactionCut(cut) => usize::try_from(cut.back).unwrap_or(usize::MAX),
            _ => 0,
        };
        Some(at.saturating_sub(back))
    }

    /// Where the [`CompactionCut`] with `id` sits in the history.
    fn cut_index(&self, id: u64) -> Option<usize> {
        self.history
            .iter()
            .rposition(|item| matches!(item, HistoryItem::CompactionCut(cut) if cut.id == id))
    }

    /// What `head` costs as the opening of a request: the system prompt and
    /// the tool schemas every request carries, and its messages — read
    /// through the provider's calibration, the gauge's own way
    /// (`docs/token-estimate.md`).
    fn head_tokens(&self, head: &[crate::context::ContextMessage]) -> u64 {
        let system = self.system_prompt.as_deref().map_or(0, count_tokens);
        let estimate = u64::try_from(system + estimate_messages_tokens(head))
            .unwrap_or(u64::MAX)
            .saturating_add(self.tools_estimate);
        let calibration = self.context_meter.calibration();
        estimate.saturating_mul(calibration.ratio_permille()) / 1000
    }

    // ----- background compaction (docs/compact.md) -----

    /// `/compact` while the agent works: note that the next checkpoint is
    /// the user's — the boundary asks the running agent for it on the
    /// shared slot. `false` (nothing noted) while one is already requested or
    /// running.
    pub fn request_compaction(&mut self) -> bool {
        if self.compaction_requested || self.background_compaction.is_some() {
            return false;
        }
        self.compaction_requested = true;
        true
    }

    /// Whether `/compact` asked for a checkpoint the agent has not taken yet.
    #[must_use]
    pub const fn compaction_requested(&self) -> bool {
        self.compaction_requested
    }

    /// The running agent took checkpoint `id` (a
    /// [`CompactionCheckpoint`](crate::stream::StreamEvent::CompactionCheckpoint)):
    /// record the [`CompactionCut`] where the history stands — in channel
    /// order, exactly the snapshot the agent holds — and open the background
    /// compaction, its summary to be written off screen. `false` (nothing
    /// recorded) while another is still running: one at a time.
    pub fn begin_background_compaction(&mut self, id: u64, back: bool) -> bool {
        if self.background_compaction.is_some() || self.is_compacting() {
            return false;
        }
        let auto = !std::mem::take(&mut self.compaction_requested);
        let user_budget = u64::try_from(crate::context::compact_user_budget(self.context_window))
            .unwrap_or(u64::MAX);
        let timestamp = self.now_stamp();
        // Reaching back: the items since the agent's last round top are the
        // round that crossed the hard limit — no part of the summary.
        let here = self.history.len();
        let reach = if back {
            self.round_top
                .filter(|(generation, at)| *generation == self.history_generation && *at <= here)
                .map_or(0, |(_, at)| u64::try_from(here - at).unwrap_or(0))
        } else {
            0
        };
        self.history.push(HistoryItem::CompactionCut(CompactionCut {
            id,
            timestamp,
            back: reach,
        }));
        self.background_compaction = Some(BackgroundCompaction {
            id,
            auto,
            before: self.context_used,
            user_budget,
            phase: CompactionPhase::Summarizing,
            waiting: false,
            summary: String::new(),
            attempt_floor: None,
            head_tokens: 0,
            reports: Vec::new(),
        });
        true
    }

    /// The running agent's next request goes out from here
    /// ([`CompactionRoundTop`](crate::stream::StreamEvent::CompactionRoundTop)):
    /// where a checkpoint it takes at the hard limit may reach back to.
    pub fn mark_round_top(&mut self) {
        self.round_top = Some((self.history_generation, self.history.len()));
    }

    /// The compaction running beside the agent's turn, if any.
    #[must_use]
    pub const fn background_compaction(&self) -> Option<&BackgroundCompaction> {
        self.background_compaction.as_ref()
    }

    /// The conversation the running background compaction summarizes: the
    /// history up to its cut — what the summarization request is built from.
    #[must_use]
    pub fn background_compaction_snapshot(&self) -> Option<&[HistoryItem]> {
        let id = self.background_compaction.as_ref()?.id;
        Some(&self.history[..self.covered_end(id)?])
    }

    /// Fold one event of the background summarizer's stream
    /// (`docs/compact.md`): its text accumulates off screen, a retry
    /// withdraws what the failed attempt wrote, the compact hooks' cells are
    /// held for the marker, its usage is billed — to the session's ledger
    /// alone: never the running turn's tally, never the gauge, which are the
    /// agent's — and its end says the summary is done or failed.
    pub fn on_summary_event(&mut self, event: &crate::stream::StreamEvent) -> SummaryStep {
        use crate::stream::StreamEvent;
        let Some(compaction) = self.background_compaction.as_mut() else {
            return SummaryStep::Continue;
        };
        if event.is_attempt_output() {
            compaction
                .attempt_floor
                .get_or_insert(compaction.summary.len());
        } else if !matches!(event, StreamEvent::Retrying { .. }) {
            compaction.attempt_floor = None;
        }
        match event {
            StreamEvent::Chunk(text) => compaction.summary.push_str(text),
            StreamEvent::Retrying { .. } => {
                if let Some(floor) = compaction.attempt_floor.take() {
                    compaction.summary.truncate(floor);
                }
            }
            StreamEvent::HookNote {
                report: Some(report),
                ..
            } => compaction.reports.push(report.clone()),
            StreamEvent::Usage(usage) => self.usage_since_launch.record(usage),
            StreamEvent::StreamDone => {
                compaction.phase = CompactionPhase::Ready;
                return SummaryStep::Done;
            }
            StreamEvent::Error(message) => return SummaryStep::Failed(message.clone()),
            _ => {}
        }
        SummaryStep::Continue
    }

    /// The summarization request is being sent again — smaller, after the
    /// provider refused it as too long (`docs/compact.md`): what the refused
    /// attempt wrote goes, the compaction itself goes on.
    pub fn restart_background_summary(&mut self) {
        if let Some(running) = self.background_compaction.as_mut() {
            running.summary.clear();
            running.attempt_floor = None;
            running.phase = CompactionPhase::Summarizing;
        }
    }

    /// The finished summary, trimmed, once the summarizer is done.
    #[must_use]
    pub fn background_summary(&self) -> Option<&str> {
        let compaction = self.background_compaction.as_ref()?;
        (compaction.phase == CompactionPhase::Ready).then(|| compaction.summary.trim())
    }

    /// The handoff for the agent once the summary is done: its checkpoint's
    /// id, the compacted head — the derivation's own
    /// ([`crate::context::compacted_head`]), so the agent's spliced list and
    /// every later derivation of the history agree — and what that head
    /// costs with the system prompt and the tool schemas, which the agent
    /// measures its next request from and which is remembered as the
    /// smallest compacting can make the context.
    pub fn take_compaction_handoff(
        &mut self,
    ) -> Option<(u64, Vec<crate::context::ContextMessage>, u64)> {
        let summary = self.background_summary()?.to_string();
        let compaction = self.background_compaction.as_ref()?;
        let (id, budget) = (compaction.id, compaction.user_budget);
        let covered = self.background_compaction_snapshot()?;
        let head = crate::context::compacted_head(
            self.user_instructions.as_deref(),
            self.listings.as_deref(),
            covered,
            &summary,
            usize::try_from(budget).unwrap_or(usize::MAX),
        );
        let head_tokens = self.head_tokens(&head);
        if let Some(compaction) = self.background_compaction.as_mut() {
            compaction.head_tokens = head_tokens;
        }
        Some((id, head, head_tokens))
    }

    /// The agent spliced the summary of checkpoint `id` in (a
    /// [`CompactionApplied`](crate::stream::StreamEvent::CompactionApplied)),
    /// or its turn ended with the summary ready: append the marker here —
    /// naming its cut, so the rounds since stay verbatim — land it, and
    /// hand back what the loop commits. `secs` is how long the compaction
    /// ran. `None` for a checkpoint that is not the one running, or one
    /// whose summary is not done.
    pub fn apply_background_compaction(&mut self, id: u64, secs: u64) -> Option<LandedCompaction> {
        let running = self.background_compaction.as_ref()?;
        if running.id != id || running.phase != CompactionPhase::Ready {
            return None;
        }
        let running = self.background_compaction.take()?;
        // The compact hooks' cells sit above the marker, as an idle
        // compaction's do — notes that gave the model nothing, so the
        // derivation is untouched.
        for report in &running.reports {
            self.record_hook_note(&report.note_label(), "", Some(report.clone()));
        }
        let compaction = Compaction {
            summary: running.summary.trim().to_string(),
            timestamp: self.now_stamp(),
            before: running.before,
            after: 0,
            auto: running.auto,
            secs,
            cut: Some(running.id),
            user_budget: running.user_budget,
        };
        let marker = self.land_marker(compaction);
        let head = if running.head_tokens > 0 {
            running.head_tokens
        } else {
            self.head_tokens(&self.marker_head(&marker))
        };
        self.note_floor(head);
        Some(LandedCompaction {
            marker,
            reports: running.reports,
        })
    }

    /// The summarizer failed (or could not be started): the background
    /// compaction is over and the conversation stays as it was — the cut it
    /// left is inert. Returns its id, for the loop to tell the agent.
    pub fn fail_background_compaction(&mut self) -> Option<u64> {
        self.background_compaction.take().map(|running| running.id)
    }

    /// Esc, `/clear`, a session swapped: drop the background compaction and
    /// any request for one — no marker. Returns its id, if one ran.
    pub fn cancel_background_compaction(&mut self) -> Option<u64> {
        self.compaction_requested = false;
        self.background_compaction.take().map(|running| running.id)
    }

    /// The agent's turn ended while its summary was still being written: the
    /// summary goes on as the session's **compact turn** — the strip says
    /// `Compacting…`, input queues behind it, Esc stops it — and lands at the
    /// end of the history, still covering only its snapshot
    /// ([`finish_compact`](Self::finish_compact) keeps the cut). Returns the
    /// compact hooks' cells it had held, for the loop to commit now; `None`
    /// when no summary is being written (one already done lands at once,
    /// [`apply_background_compaction`](Self::apply_background_compaction)).
    pub fn adopt_background_compaction(&mut self) -> Option<Vec<crate::hooks::HookReport>> {
        if self
            .background_compaction
            .as_ref()
            .is_none_or(|running| running.phase != CompactionPhase::Summarizing)
        {
            return None;
        }
        let running = self.background_compaction.take()?;
        self.begin_compact(running.auto);
        self.compact_buffer = Some(running.summary);
        self.compact_before = running.before;
        self.compact_cut = Some(running.id);
        self.compact_user_budget = Some(running.user_budget);
        // Mid-attempt: a retry withdraws what this attempt already wrote.
        self.attempt_floor = running.attempt_floor.map(|summary| turn::AttemptFloor {
            history: self.history.len(),
            summary,
        });
        Some(running.reports)
    }

    /// The agent waits at the hard limit for checkpoint `id`'s summary.
    pub fn set_compaction_waiting(&mut self, id: u64) {
        if let Some(running) = self
            .background_compaction
            .as_mut()
            .filter(|running| running.id == id)
        {
            running.waiting = true;
        }
    }

    /// Inject how long the background compaction has run — the boundary's
    /// clock, before each draw.
    pub fn set_compaction_elapsed(&mut self, elapsed: Option<Duration>) {
        self.compaction_elapsed = elapsed;
    }

    /// How long the background compaction has run, as last injected.
    #[must_use]
    pub const fn compaction_elapsed(&self) -> Option<Duration> {
        self.compaction_elapsed
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
    /// known, the gauge is past codex's 90% threshold, no turn and no
    /// compaction is in flight, the conversation has moved since the last
    /// attempt (a compaction is never repeated over the same conversation),
    /// compacting can still get under the threshold at all, and the
    /// conversation derives a non-empty context to summarize. The number of
    /// compactions is otherwise unlimited. See `docs/compact.md` /
    /// `docs/settings.md`.
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
        if self.turn_active() || self.is_streaming() || self.background_compaction.is_some() {
            return false;
        }
        if self.compact_watermark == Some(self.history_signature()) {
            return false;
        }
        if self.context_used <= auto_compact_threshold(window) {
            return false;
        }
        if self.compaction_cannot_help().is_some() {
            return false;
        }
        crate::context::derives_conversation(&self.history)
    }

    /// What the boundary tells the running agent about compacting
    /// ([`CompactionPolicy`](crate::llm::compaction::CompactionPolicy)): the
    /// window, the `/settings` **Auto compact** knob, and — while it still
    /// describes this conversation under this window — the size of the last
    /// compaction's head, the smallest compacting can make it. `None` with
    /// no window known: no threshold, no hard limit. See `docs/compact.md`.
    #[must_use]
    pub fn compaction_policy(&self) -> Option<crate::llm::compaction::CompactionPolicy> {
        let window = self.context_window?;
        Some(crate::llm::compaction::CompactionPolicy {
            window,
            auto: self.settings.auto_compact,
            floor: self.current_floor(),
            calibration: self.context_meter.calibration(),
        })
    }

    /// The last compaction's head, while it still describes this
    /// conversation (no rewind, `/clear` or `/resume` since) under this
    /// window.
    fn current_floor(&self) -> Option<u64> {
        let floor = self.compact_floor?;
        (floor.generation == self.history_generation && Some(floor.window) == self.context_window)
            .then_some(floor.head)
    }

    /// When the last compaction could not get the context under the
    /// threshold even with nothing after it — its head alone, instructions,
    /// tool schemas, the recent user messages and the summary, is at or past
    /// it — the head and the threshold, which the loop says once; automatic
    /// compaction then stops trying until something changes
    /// (`docs/compact.md` *When compacting cannot help*). `None` otherwise.
    #[must_use]
    pub fn compaction_cannot_help(&self) -> Option<(u64, u64)> {
        let head = self.current_floor()?;
        let threshold = auto_compact_threshold(self.context_window?);
        (head >= threshold).then_some((head, threshold))
    }
}

/// Where auto-compaction runs for a `window`-token context window: codex's
/// 90% (`ModelInfo::auto_compact_token_limit`). The trigger
/// ([`App::should_auto_compact`]), the running agent
/// ([`crate::llm::compaction`]) and the `/context` page's headroom
/// (`docs/context-usage.md`) all read it, so none can show a threshold the
/// others do not act on.
#[must_use]
pub(crate) const fn auto_compact_threshold(window: u64) -> u64 {
    crate::llm::compaction::soft_threshold(window)
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
///
/// What the summary covers is everything before its [`cut`](Self::cut) — or,
/// with no cut, everything before the marker itself: a compaction that ran
/// while the agent kept working summarized the conversation as it stood at
/// its checkpoint, and the rounds the agent ran meanwhile stay verbatim
/// between the cut and the marker (`docs/compact.md` *Background
/// compaction*).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    /// The id of the [`CompactionCut`] the summary's coverage ends at — the
    /// checkpoint a running agent took — or `None` when it covers everything
    /// before the marker (an idle compaction, and every marker recorded
    /// before background compaction). A cut the history no longer holds
    /// reads as `None`. See `docs/compact.md` *Where a summary's coverage
    /// ends*.
    pub cut: Option<u64>,
    /// The budget, in approximate tokens, of the recent typed user messages
    /// the derivation keeps ahead of the summary bridge — scaled to the
    /// window the compaction ran under ([`crate::context::compact_user_budget`])
    /// and recorded so the derivation stays a pure function of the history.
    /// 0 = recorded before the field: codex's fixed 20 000.
    pub user_budget: u64,
}

/// Where a background compaction's summary stops covering the conversation
/// (`docs/compact.md` *Where a summary's coverage ends*): appended to the
/// history the moment the running agent takes its checkpoint, so everything
/// before it is exactly the snapshot the summarizer reads, and named by the
/// [`Compaction`] marker that lands later by its [`id`](Self::id) — a
/// reference no skipped rollout line or truncation can shift, where an index
/// could. Invisible: it renders nothing, derives nothing and is recorded as
/// its own rollout line. One whose compaction never landed (an Esc, a failed
/// summary) is inert.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompactionCut {
    /// The checkpoint's id, the one its marker names.
    pub id: u64,
    /// Wall-clock stamp (recorded like every item's; never displayed).
    pub timestamp: String,
    /// How many items right before this one the summary does **not**
    /// cover: a checkpoint the agent took at the hard limit reaches back to
    /// the round top its last request went out from, so the round since —
    /// recorded before the checkpoint arrived — stays verbatim
    /// (`docs/compact.md` *The round that crossed the limit*). 0 for every
    /// other cut: it covers everything before it.
    pub back: u64,
}

/// A compaction running **beside** the agent's turn (`docs/compact.md`
/// *Background compaction*): the running agent took its checkpoint, the
/// summary is written off screen on a thread of its own, and the marker
/// lands where the agent splices it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundCompaction {
    /// The checkpoint's id — its [`CompactionCut`]'s, its marker's `cut`.
    pub id: u64,
    /// Whether the agent took it on its own (the cell's `· auto`), rather
    /// than for the user's `/compact`.
    pub auto: bool,
    /// The context gauge at the checkpoint — the marker's `before`.
    pub before: u64,
    /// The recent-user-message budget the window gave it at the checkpoint.
    pub user_budget: u64,
    /// Whether the summary is still being written or waits to be spliced.
    pub phase: CompactionPhase,
    /// Whether the agent waits for it at the hard limit — the one pause
    /// background compaction costs, which the strip says.
    pub waiting: bool,
    /// The summary as it streams, never shown.
    pub(super) summary: String,
    /// Where the summarizer's current attempt began in
    /// [`summary`](Self::summary) — what a retry withdraws back to.
    pub(super) attempt_floor: Option<usize>,
    /// What the handoff's head costs, once it was built.
    pub(super) head_tokens: u64,
    /// The compact hooks' cells, committed with the marker.
    pub(super) reports: Vec<crate::hooks::HookReport>,
}

/// Where a [`BackgroundCompaction`] stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionPhase {
    /// The summarizer is writing.
    Summarizing,
    /// The summary is done and handed to the agent, which splices it in at
    /// its next round top.
    Ready,
}

/// What one summarizer event did ([`App::on_summary_event`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryStep {
    /// Nothing for the loop to do.
    Continue,
    /// The summary is done: hand it to the agent.
    Done,
    /// The summarizer failed with this error.
    Failed(String),
}

/// A background compaction that landed: its marker as recorded, and the
/// compact hooks' cells to commit above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandedCompaction {
    pub marker: Compaction,
    pub reports: Vec<crate::hooks::HookReport>,
}

/// What the last compaction's head cost, and under which history generation
/// and window it was counted — the smallest compacting can make the
/// conversation while both still hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompactFloor {
    pub(super) generation: u64,
    pub(super) window: u64,
    pub(super) head: u64,
}
