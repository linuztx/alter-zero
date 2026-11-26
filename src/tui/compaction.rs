//! Background compaction at the boundary (`docs/background-compaction.md`):
//! the summary started where the running turn asks for it, written on a
//! worker of its own, held until the turn's ceiling, swapped in at the round
//! top that waits for it — and, between turns, started and landed by the loop
//! bottom's tick.
//!
//! The pure halves live elsewhere: the policy and the slot the turn shares in
//! `alter_zero::compaction`, the job's state and its anchored marker on `App`
//! (`app/compact.rs`). This module is their I/O — a one-off backend spawned,
//! a worker's answer taken in, the `Context compacted` cell committed.

use std::thread::JoinHandle;
use std::time::Instant;

use ratatui::text::Line;

use alter_zero::app::ToastKind;
use alter_zero::compaction;
use alter_zero::context::{self, ContextMessage, ContextRole};
use alter_zero::session::RequestUsage;
use alter_zero::stream::{CancelToken, ReplySource};
use alter_zero::ui;

use super::Session;
use super::workers::{self, CompactionDone};

/// The background summary's worker as the session holds it: the job it
/// writes, the token that stops it, when it started — the strip row's clock —
/// and its thread, parked in `reaping` when it is stopped.
pub(crate) struct CompactionWorker {
    id: u64,
    cancel: CancelToken,
    started: Instant,
    handle: Option<JoinHandle<()>>,
}

impl Session<'_> {
    /// Start background summary `id`: the running turn reached its start
    /// line (`StreamEvent::CompactionStart`), or the loop bottom found the
    /// conversation past it between turns. Its coverage is everything
    /// recorded so far — the channel is FIFO, so every item the turn produced
    /// before asking is in history and nothing of the round about to start is
    /// — and it is written by a tools-free one-off backend on a worker of its
    /// own while the turn goes on (`docs/background-compaction.md` *The
    /// lifecycle*).
    pub(crate) fn start_background_compaction(&mut self, id: u64) {
        // One job at a time, and the slot has just handed this one out: an
        // older job this side still held is dropped here, never in the slot.
        if self.app.background_compaction().is_some() {
            self.stop_compaction_worker();
            self.app.drop_background_compaction();
            self.compaction_hooks.clear();
        }
        let one_off = self
            .models
            .compact_backend(self.app.thinking.as_ref().map(|t| t.mode), true);
        // A real session never writes the summary on its own backend: that
        // one checks the slot at its round tops, and would wait on itself.
        // The offline demo and the stall backend have no one-off; the demo
        // scripts a summary for the compact prompt.
        if one_off.is_none() && self.models.is_real() {
            self.compaction.fail(id);
            return;
        }
        self.app.begin_background_compaction(id);
        let mut input = context::conversation_messages(&self.app.history);
        input.push(ContextMessage::new(
            ContextRole::User,
            context::SUMMARIZATION_PROMPT,
        ));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = CancelToken::new();
        let spawn_on: &dyn ReplySource = match one_off.as_ref() {
            Some(one_off) => one_off,
            None => self.models.backend(),
        };
        let summarizer = spawn_on.spawn(
            context::SUMMARIZATION_PROMPT.to_string(),
            Vec::new(),
            input,
            tx,
            cancel.clone(),
        );
        let started = Instant::now();
        let handle = workers::spawn_background_compaction(
            id,
            rx,
            summarizer,
            started,
            self.compaction_tx.clone(),
        );
        self.compaction_worker = Some(CompactionWorker {
            id,
            cancel,
            started,
            handle: Some(handle),
        });
        self.compaction_hooks.clear();
        self.frame.schedule_frame();
    }

    /// Background summary `done.id` was written, or failed. Written, it is
    /// held — for the running turn's ceiling, or the idle loop bottom's
    /// landing; failed, the slot is freed and counts it, and the next round
    /// top past the start line starts another, until the cycle's second
    /// failure (`docs/background-compaction.md` *Failures and edges*). An
    /// answer for a job no longer wanted changes nothing.
    pub(crate) fn on_compaction_done(&mut self, done: CompactionDone) {
        if self
            .compaction_worker
            .as_ref()
            .is_some_and(|worker| worker.id == done.id)
        {
            self.compaction_worker = None;
        }
        // Billed whatever became of it: the launch's ledger has it.
        for usage in &done.usage {
            self.app.record_compaction_usage(usage);
        }
        if self
            .app
            .background_compaction()
            .is_none_or(|job| job.id != done.id)
        {
            return;
        }
        // …and the conversation's rollout, a `compact` request's line where
        // it finished — part of no turn's summary (docs/rollout-records.md).
        for usage in &done.usage {
            self.recorder.record_usage(RequestUsage {
                after: self.app.history.len(),
                usage: *usage,
                agent: None,
                compact: true,
            });
        }
        match done.summary {
            Ok(summary) => {
                if self
                    .app
                    .background_compaction_written(done.id, summary, done.secs)
                {
                    self.compaction.ready(done.id);
                    self.compaction_hooks = done.hooks;
                }
            }
            Err(reason) => {
                self.app.drop_background_compaction();
                self.compaction.fail(done.id);
                self.compaction_hooks.clear();
                self.toast(
                    format!("{BACKGROUND_COMPACTION_FAILED}{reason}"),
                    ToastKind::Error,
                );
            }
        }
        self.frame.schedule_frame();
    }

    /// The running turn reached its ceiling with summary `id` held
    /// (`StreamEvent::CompactionSwap`) and waits for the answer: land it —
    /// the anchored marker appended, the compact hooks' cells and the
    /// `● Context compacted · … · background` cell committed in place — and
    /// hand the turn the context re-derived from history exactly as the next
    /// turn's start would, so the list it goes on with is the list that turn
    /// derives (`docs/background-compaction.md` *The lifecycle*). A job that
    /// cannot land is let go, and the turn sends its request as it is.
    pub(crate) fn on_compaction_swap(&mut self, id: u64, committing: bool, width: u16) {
        let Some(after) = self.land_background_compaction(id, committing, width) else {
            if self.app.background_compaction().is_some() {
                self.let_go_of_compaction();
            } else {
                self.compaction.forget();
            }
            self.compaction.answer(id, None);
            return;
        };
        let context = context::context_messages_full(
            self.app.user_instructions.as_deref(),
            self.app.listings.as_deref(),
            &self.app.history,
        );
        self.compaction.landed(id, after);
        self.compaction.answer(id, Some(context));
        self.frame.schedule_frame();
    }

    /// Background compaction's loop-bottom work
    /// (`docs/background-compaction.md` *Between turns*): keep the slot on
    /// the gauge's window, let go of a summary whose history moved or that
    /// auto compaction no longer wants, and — between turns — land a held
    /// summary, or start one once the conversation is past the start line.
    /// `idle_start` is whether a summary may start between turns at all: a
    /// headless run starts one only while it waits on its subagents, the
    /// boundary a follow-up turn comes out of.
    pub(crate) fn tick_background_compaction(&mut self, idle_start: bool) {
        self.compaction.configure(self.app.compaction_window());
        if self.app.background_compaction().is_some()
            && (self.app.background_compaction_stale() || self.compaction.window().is_none())
        {
            self.let_go_of_compaction();
        }
        if self.inflight.is_some() || !self.app.can_compact_idle() {
            return;
        }
        // No turn is waiting any more: the next one wears its own verb until
        // it reaches its ceiling.
        self.app.end_compaction_wait();
        if let Some(job) = self.app.background_compaction() {
            // Written: there is no request in flight whose cached prefix it
            // would cost, so the next turn starts compacted. Still being
            // written: it is held when it lands, and landed here then.
            if job.summary.is_some() {
                let id = job.id;
                let committing = self.commits_allowed();
                let width = self.term.screen().width;
                if let Some(after) = self.land_background_compaction(id, committing, width) {
                    self.compaction.landed(id, after);
                    self.frame.schedule_frame();
                }
            }
            return;
        }
        if !idle_start || self.models.is_stalled_test_backend() {
            return;
        }
        let Some(window) = self.compaction.window() else {
            return;
        };
        let line =
            compaction::start_line(compaction::ceiling(window), self.compaction.cycle_start());
        // Two failed summaries this cycle, or a swap that left the context
        // past its ceiling: summarizing again now would only land in the same
        // place, so nothing starts until the next turn.
        if self.app.context_used() < line
            || self.compaction.failures() >= compaction::MAX_FAILURES
            || self.compaction.futile()
        {
            return;
        }
        if let Some(id) = self.compaction.begin() {
            self.start_background_compaction(id);
        }
    }

    /// Let go of the background summary — its history moved, auto
    /// compaction went off, a `/compact` by hand replaces it, the session
    /// ends: the worker stopped and its thread parked for the sweep, the job
    /// forgotten on both sides. A summary already paid for is still billed.
    pub(crate) fn let_go_of_compaction(&mut self) {
        self.stop_compaction_worker();
        self.app.drop_background_compaction();
        self.compaction.forget();
        self.compaction_hooks.clear();
    }

    /// Stop the background summary's worker, if one is writing: its
    /// summarizer cancelled, its thread parked in `reaping` for the sweep —
    /// never joined on the loop (`docs/interrupt.md`).
    fn stop_compaction_worker(&mut self) {
        if let Some(mut worker) = self.compaction_worker.take() {
            worker.cancel.cancel();
            if let Some(handle) = worker.handle.take() {
                self.reaping.push(handle);
            }
        }
    }

    /// How long the background summary has been written for — the strip
    /// row's clock, read per draw; `None` with none being written.
    pub(crate) fn compaction_elapsed(&self) -> Option<std::time::Duration> {
        self.compaction_worker
            .as_ref()
            .map(|worker| worker.started.elapsed())
    }

    /// Land held summary `id` in the history and on screen, returning the
    /// context's size after it — the slot's next cycle start. `None`, and
    /// nothing committed, for a job that is not this one, is stale or is not
    /// written yet.
    fn land_background_compaction(&mut self, id: u64, committing: bool, width: u16) -> Option<u64> {
        let landable = self
            .app
            .background_compaction()
            .is_some_and(|job| job.id == id && job.summary.is_some())
            && !self.app.background_compaction_stale();
        if !landable {
            return None;
        }
        // Flush before you interleave (invariant 4): a round top has nothing
        // streaming, but whatever is goes ahead of the cell, as does the
        // read/search group the turn has open (docs/tool-groups.md).
        self.flush_segment(committing, width);
        self.render.reset();
        self.close_tool_group(committing, width);
        // The compact hooks' trace sits above the `Context compacted` line,
        // as a compaction turn's does; their text reached the summarizer
        // and joins nothing else.
        for (label, report) in std::mem::take(&mut self.compaction_hooks) {
            self.commit_hook_note(&label, "", Some(report), committing, width);
        }
        let compaction = self.app.land_background_compaction(id)?;
        if committing {
            let height = self.live_region_height();
            self.term.set_view_height(height);
            self.term
                .insert_before(ui::compaction_lines(&compaction, width));
            self.term.insert_before(vec![Line::default()]);
        }
        Some(compaction.after)
    }
}

/// The toast a failed background summary raises, ahead of the reason.
const BACKGROUND_COMPACTION_FAILED: &str = "Background compaction failed — ";
