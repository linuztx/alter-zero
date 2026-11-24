//! Background compaction at the boundary (`docs/compact.md` *Background
//! compaction*): the summarizer the running agent's checkpoint starts, the
//! summary handed back to the agent on the shared slot, the marker committed
//! where the agent splices it in — and what a turn that ends, an Esc or a
//! `/clear` leaves of all that.
//!
//! The decisions are `App`'s ([`alter_zero::app::App::begin_background_compaction`]
//! and its siblings) and the agent's (`llm::compaction`); this module only
//! starts the thread, routes its channel and commits the cells.

use std::thread::JoinHandle;
use std::time::Instant;

use ratatui::text::Line;

use alter_zero::app::{CompactionPhase, LandedCompaction, SummaryStep, ToastKind};
use alter_zero::context;
use alter_zero::llm::compaction::{Handoff, is_context_window_refusal};
use alter_zero::session::RequestUsage;
use alter_zero::stream::{CancelToken, StreamEvent};
use alter_zero::ui;

use super::Session;

/// How many times a summarization request the provider refused as too long
/// is sent again, each time a third smaller (`docs/compact.md` *The
/// summarization request must fit*).
const REFUSAL_RETRIES: u32 = 2;

/// The background summarizer: its thread, the token that stops it, and the
/// budget its request was fitted to — what a refusal shrinks.
pub(crate) struct Summarizer {
    cancel: CancelToken,
    handle: JoinHandle<()>,
    budget: Option<usize>,
    retries: u32,
}

impl Session<'_> {
    /// `/compact` while the agent works: ask the running agent for a
    /// checkpoint at its next round top, and say so.
    pub(crate) fn request_background_compaction(&mut self) {
        self.compaction_slot.request();
        self.toast(alter_zero::app::COMPACT_REQUESTED_NOTICE, ToastKind::Info);
    }

    /// Publish what the agent may do about compacting — the window, the
    /// **Auto compact** knob, how small compacting can make the context —
    /// when it changed. The loop bottom's call, and every turn start's.
    pub(crate) fn sync_compaction_policy(&mut self) {
        let policy = self.app.compaction_policy();
        if policy != self.published_policy {
            self.compaction_slot.set_policy(policy);
            self.published_policy = policy;
        }
    }

    /// The running agent took checkpoint `id`: record its cut and start
    /// writing the summary of everything before it.
    pub(crate) fn on_compaction_checkpoint(&mut self, id: u64, back: bool) {
        if !self.app.begin_background_compaction(id, back) {
            // One at a time — the agent never takes a second while one runs,
            // so this one is no compaction of ours to write.
            self.compaction_slot.fail(id);
            return;
        }
        self.compaction_started = Some(Instant::now());
        let budget = self.summarization_budget();
        self.spawn_summarizer(budget, 0);
    }

    /// Start the summarizer over the running compaction's snapshot, its
    /// request fitted to `budget`, on the compaction channel.
    fn spawn_summarizer(&mut self, budget: Option<usize>, retries: u32) {
        let Some(auto) = self.app.background_compaction().map(|running| running.auto) else {
            return;
        };
        let thinking = self.app.thinking.as_ref().map(|thinking| thinking.mode);
        let Some(backend) = self.models.compact_backend(thinking, auto) else {
            self.summarizer_failed("no model is configured to write the summary");
            return;
        };
        let Some(snapshot) = self.app.background_compaction_snapshot() else {
            return;
        };
        let request =
            context::summarization_request(self.app.user_instructions.as_deref(), snapshot, budget);
        let cancel = CancelToken::new();
        let handle = alter_zero::stream::ReplySource::spawn(
            &backend,
            context::SUMMARIZATION_PROMPT.to_string(),
            Vec::new(),
            request,
            self.compact_tx.clone(),
            cancel.clone(),
        );
        self.summarizer = Some(Summarizer {
            cancel,
            handle,
            budget,
            retries,
        });
    }

    /// One event off the compaction channel: the summary streams off
    /// screen, its usage is billed and recorded as a compaction's, and its
    /// end hands it to the agent or fails it. Returns whether a summary was
    /// just handed over — what a headless run says.
    pub(crate) fn on_compaction_event(&mut self, event: StreamEvent) -> bool {
        // A summarizer stopped since (an Esc, a `/clear`) sends on a channel
        // that was swapped away; anything else with no compaction running is
        // nobody's.
        if self.summarizer.is_none() || self.app.background_compaction().is_none() {
            return false;
        }
        if let StreamEvent::Usage(usage) = &event {
            self.recorder.record_usage(RequestUsage {
                after: self.app.history.len(),
                usage: *usage,
                agent: None,
                compact: true,
            });
        }
        match self.app.on_summary_event(&event) {
            SummaryStep::Continue => false,
            SummaryStep::Done => self.hand_over_summary(),
            SummaryStep::Failed(message) => {
                self.summarizer_failed(&message);
                false
            }
        }
    }

    /// The summary is done: build the head the agent splices in — the
    /// derivation's own — and publish it on the slot.
    fn hand_over_summary(&mut self) -> bool {
        let Some((id, head, head_tokens)) = self.app.take_compaction_handoff() else {
            return false;
        };
        let head = alter_zero::llm::backend::build_messages_for(
            None,
            None,
            "",
            &head,
            alter_zero::images::attachment_data_url,
        );
        self.compaction_slot.publish(Handoff {
            id,
            head,
            head_tokens,
        });
        if let Some(summarizer) = self.summarizer.take() {
            self.reaping.push(summarizer.handle);
        }
        self.frame.schedule_frame();
        true
    }

    /// The summarizer failed: a request the provider refused as too long is
    /// sent again, smaller; anything else ends the compaction — the agent
    /// is told, the conversation stays as it was, and a turn that still fits
    /// goes on (the agent retries once something new has joined).
    fn summarizer_failed(&mut self, message: &str) {
        if let Some(summarizer) = self.summarizer.take() {
            self.reaping.push(summarizer.handle);
            if is_context_window_refusal(message)
                && summarizer.retries < REFUSAL_RETRIES
                && let Some(budget) = summarizer.budget
            {
                self.app.restart_background_summary();
                self.spawn_summarizer(Some(budget / 3 * 2), summarizer.retries + 1);
                return;
            }
        }
        if let Some(id) = self.app.fail_background_compaction() {
            self.compaction_slot.fail(id);
        }
        self.compaction_started = None;
        self.toast(
            format!("Compaction failed — the agent keeps working: {message}"),
            ToastKind::Error,
        );
    }

    /// The agent spliced checkpoint `id`'s summary in — or its turn ended
    /// with the summary done: the marker lands here, under the compact
    /// hooks' cells.
    pub(crate) fn land_background_compaction(&mut self, id: u64, committing: bool, width: u16) {
        let secs = self
            .compaction_started
            .map_or(0, |started| started.elapsed().as_secs());
        let Some(landed) = self.app.apply_background_compaction(id, secs) else {
            return;
        };
        self.compaction_started = None;
        self.commit_landed_compaction(&landed, committing, width);
        self.say_if_compaction_cannot_help();
    }

    fn commit_landed_compaction(
        &mut self,
        landed: &LandedCompaction,
        committing: bool,
        width: u16,
    ) {
        if !committing {
            return;
        }
        let height = self.live_region_height();
        self.term.set_view_height(height);
        for report in &landed.reports {
            self.term
                .insert_before(ui::hook_report_lines(report, width));
            self.term.insert_before(vec![Line::default()]);
        }
        self.term
            .insert_before(ui::compaction_lines(&landed.marker, width));
        self.term.insert_before(vec![Line::default()]);
    }

    /// Say it once, when a compaction just landed whose head alone fills the
    /// threshold: compacting cannot help here, so it stops trying
    /// (`docs/compact.md` *When compacting cannot help*).
    pub(crate) fn say_if_compaction_cannot_help(&mut self) {
        if let Some((head, threshold)) = self.app.compaction_cannot_help() {
            let count =
                |tokens: u64| ui::format_token_count(usize::try_from(tokens).unwrap_or(usize::MAX));
            self.toast(
                format!(
                    "Compacting can't get under the {} threshold: the summary, instructions and \
                     tools alone take {} — automatic compaction is paused",
                    count(threshold),
                    count(head)
                ),
                ToastKind::Error,
            );
        }
    }

    /// The agent's turn just ended: settle what it left of background
    /// compaction. A `/compact` it never got to runs now, as an idle
    /// compaction; a summary already done lands at the end of the history;
    /// one still being written goes on as the session's own compact turn.
    /// Returns whether a compaction became the turn — queued input then
    /// waits for it, as it does for an idle one.
    pub(crate) fn settle_compaction_at_turn_end(&mut self) -> bool {
        let width = self.term.screen().width;
        let committing = self.commits_allowed();
        if self.app.compaction_requested() && self.app.background_compaction().is_none() {
            self.app.cancel_background_compaction();
            self.compaction_slot.reset();
            if !context::derives_conversation(&self.app.history) {
                return false;
            }
            self.start_compact_turn(/*auto=*/ false);
            return true;
        }
        let Some(running) = self.app.background_compaction() else {
            return false;
        };
        let id = running.id;
        self.compaction_slot.reset();
        if running.phase == CompactionPhase::Ready {
            self.land_background_compaction(id, committing, width);
            return false;
        }
        let Some(summarizer) = self.summarizer.take() else {
            // Nothing is writing it after all: it simply did not happen.
            let _ = self.app.fail_background_compaction();
            self.compaction_started = None;
            return false;
        };
        let Some(reports) = self.app.adopt_background_compaction() else {
            self.summarizer = Some(summarizer);
            return false;
        };
        // The cells the compact hooks already left: above the marker, as
        // an idle compaction's.
        for report in reports {
            self.commit_hook_note(&report.note_label(), "", Some(report), committing, width);
        }
        // The summarizer's channel becomes the turn's: what it streams from
        // here on is the compact turn's, folded like any.
        std::mem::swap(&mut self.tx, &mut self.compact_tx);
        std::mem::swap(&mut self.reply_rx, &mut self.compact_rx);
        self.inflight = Some((summarizer.cancel, summarizer.handle));
        self.render.reset();
        self.clocks.start_turn();
        // Its clock runs from its checkpoint, so the cell says how long the
        // compaction worked in all.
        self.clocks.turn_start = self.compaction_started.take().or(self.clocks.turn_start);
        true
    }

    /// Esc, `/clear`, the quit: stop the summarizer and forget the
    /// compaction — no marker lands, and the agent (cancelled with it, or
    /// gone) is owed nothing.
    pub(crate) fn stop_background_compaction(&mut self) {
        if let Some(summarizer) = self.summarizer.take() {
            summarizer.cancel.cancel();
            self.reaping.push(summarizer.handle);
            // Whatever it still sends goes nowhere.
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            self.compact_tx = tx;
            self.compact_rx = rx;
        }
        self.app.cancel_background_compaction();
        self.compaction_slot.reset();
        self.compaction_started = None;
    }
}
