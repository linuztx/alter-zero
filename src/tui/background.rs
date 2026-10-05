//! Background shells: their events, and when their notices are allowed to
//! appear (`docs/background.md`).
//!
//! A `bash` call that outlives its `wait` — or a `wait: 0` launch, or a Ctrl+B
//! hand-off — leaves a process running past the turn that started it, on its
//! own channel that is never swapped (unlike the reply channel), so shells
//! survive an interrupt and `/clear` kills them explicitly.
//!
//! A completion has to reach two audiences, and the split is the whole point of
//! [`Session::on_bg_event`]:
//!
//! - **The model, immediately.** The note goes onto the registry's notice board
//!   the moment the process exits, so an in-flight agent picks it up before its
//!   next round — a shell it just `kill`ed is known to it within the same turn.
//!   A shell launched *by a subagent* reports to that agent first, through the
//!   registry's pending-input seam.
//! - **The conversation, where the model read it.** The `● Background command
//!   "…" completed` cell is held until the in-flight agent announces it took
//!   the note ([`Session::settle_delivered_notice`], on
//!   `StreamEvent::NoticeDelivered`) — the round boundary that request went
//!   out from, right after the previous round's tool results, where the
//!   streaming buffer is empty (invariants 2/3). What no agent read settles
//!   at the turn's end ([`Session::settle_bg_completions`]) and rides the
//!   next turn's context. Settling at the first tool boundary instead put a
//!   note that landed while the model was generating a call *in front of*
//!   that call — so the transcript, and every later turn's context, said the
//!   model had read it and gone on regardless.
//!
//! With nothing in flight, a model-launched completion no agent read starts the
//! automatic follow-up turn instead, so the model can report the result.
//!
//! A session that stops to **ask for input** with nobody watching reaches
//! both audiences the same way (`docs/bash-tools.md`) — the amber `● … is
//! waiting for input` cell, a note naming the session — while it runs on.

use std::time::Instant;

use ratatui::text::Line;

use alter_zero::app::BgCompletion;
use alter_zero::background::BgEvent;
use alter_zero::ui;

use super::Session;

impl Session<'_> {
    /// Fold one background-shell event into `App`: start or stop its runtime
    /// clock, append streamed output, or route a completion (see the module
    /// doc). An exit while nothing is in flight settles at once and may start the
    /// follow-up turn.
    pub(crate) fn on_bg_event(&mut self, event: BgEvent) {
        match event {
            BgEvent::Started {
                id,
                command,
                description,
                from_model,
                origin,
            } => {
                self.bg_clocks.insert(id.clone(), Instant::now());
                self.app
                    .bg_started(&id, &command, description, from_model, origin);
            }
            BgEvent::Output { id, chunk } => self.app.bg_output(&id, &chunk),
            BgEvent::Screen { id, text } => self.app.bg_screen(&id, &text),
            // An exit the model already read — a `bash_session` call reported
            // it (docs/interactive-shell.md): the shell leaves the list, and
            // no notice is owed to anyone.
            BgEvent::Exited {
                id, observed: true, ..
            } => {
                self.bg_clocks.remove(&id);
                let _ = self.app.bg_exited(&id, None, false);
            }
            BgEvent::Exited {
                id, code, killed, ..
            } => {
                self.bg_clocks.remove(&id);
                match self.app.bg_exited(&id, code, killed) {
                    Some(completion) => self.report_bg_completion(completion),
                    // Already swept (`/clear`): no note is owed, so the
                    // registry need not keep the exit claimable.
                    None => self.registry.forget_exit(&id),
                }
            }
            // A session nobody waits on stopped to ask for input
            // (docs/bash-tools.md): reported the way an exit is, while the
            // shell keeps running and keeps its row.
            BgEvent::Waiting { id } => {
                if let Some(waiting) = self.app.bg_waiting(&id) {
                    self.report_bg_completion(waiting);
                }
            }
        }
        self.frame.schedule_frame();
    }

    /// Tell the model what a background shell did — it exited, or stopped to
    /// ask for input — and hold its notice until the model reads it.
    fn report_bg_completion(&mut self, completion: BgCompletion) {
        // A subagent-launched shell reports to its launcher first: the note
        // queues onto that agent's seam and is heard at its next round
        // boundary — the same seam a user's chat message rides
        // (docs/queue.md). Queueing is **all** this does:
        // `StreamEvent::Steered` records the note on the agent's transcript
        // when the loop actually takes it, and the session view commits it
        // from there. Recording it here as well put it on the transcript
        // twice — once eagerly, once on the echo — permanently, in history,
        // the rollout and every rebuild.
        //
        // A launcher that already settled can't hear it: the shared board
        // takes the note instead, so the main turn (or the idle follow-up
        // turn) relays the outcome (docs/agent-tool.md).
        //
        // Either way, a companion call may have got there first — a `bashsend`
        // into a session that ended while the model was writing it reports
        // the exit itself, and a look at a session shows the prompt a waiting
        // note would — and then the model has it: no note, no cell
        // (docs/bash-tools.md *One notice per exit*). The registry answers
        // that under its own lock, atomically with the claim.
        let note = completion.context_text();
        let routed = match &completion.origin {
            Some(origin) => match self.agent_registry.route_shell_note(
                &origin.agent_id,
                &completion.id,
                &note,
                completion.waiting,
                &self.registry,
            ) {
                alter_zero::agents::Route::Covered => return,
                alter_zero::agents::Route::Routed => true,
                alter_zero::agents::Route::Unheard => false,
            },
            None => false,
        };
        let seq = if routed {
            None
        } else {
            let posted = if completion.waiting {
                self.registry
                    .post_waiting_notice(&completion.id, note, completion.from_model)
            } else {
                self.registry
                    .post_exit_notice(&completion.id, note, completion.from_model)
            };
            let Some(seq) = posted else {
                return;
            };
            Some(seq)
        };
        self.app.defer_bg_completion(completion, seq);
        if !self.app.turn_active() {
            self.dispatch_after_turn();
        }
    }

    /// `x` in the ↓ manager: stop the task. The registry's `Exited` event then
    /// removes the row and commits the stopped notice.
    pub(crate) fn kill_background(&self, id: &str) {
        self.registry.kill(id);
    }

    /// Ctrl+B on a running command: raise the latch the runner's poll loop
    /// consumes to hand its child off.
    pub(crate) fn move_to_background(&self) {
        self.registry.request_background();
    }

    /// Settle the notice the in-flight agent just read — board note `seq`
    /// (`StreamEvent::NoticeDelivered`): record + commit its held cell **here**,
    /// at the round boundary the model's request went out from, so history,
    /// scrollback and every later turn's context carry it exactly where the
    /// wire did (`docs/background.md`). A note nobody holds a cell for (one
    /// whose cell already settled) changes nothing.
    pub(crate) fn settle_delivered_notice(&mut self, seq: u64) {
        if let Some(completion) = self.app.take_delivered_bg_completion(seq) {
            self.settle_bg_completion(&completion);
        } else if let Some(notice) = self.app.take_delivered_agent_notice(seq) {
            self.settle_agent_notice(&notice);
        }
    }

    /// Settle every notice still held — the ones no agent read: record +
    /// commit each in arrival order, shells then agents. Runs at every turn
    /// end (`StreamDone` — above its summary — a backend error, both Esc
    /// outcomes) and at an idle arrival; the next turn's context carries them
    /// from there. A note a report took back off the board before any model
    /// read it owes no cell (`docs/agent-tools.md` *One notice per answer*).
    pub(crate) fn settle_bg_completions(&mut self) {
        for held in self.app.take_pending_bg_completions() {
            if !self.retracted(held.seq) {
                self.settle_bg_completion(&held.notice);
            }
        }
        // Background-agent completions settle at the same turn ends — the
        // green `● Agent "…" finished` / red stopped cell — and update the
        // recorded group entry so the Ctrl+O cell shows the final response
        // (docs/agent-tool.md).
        for held in self.app.take_pending_agent_notices() {
            if self.retracted(held.seq) {
                // An `agentoutput` report took the note back before the lead
                // read it: the cell would record a notice nobody sent — but
                // the recorded group entry still learns the final state.
                self.app.settle_agent_completion(&held.notice);
                continue;
            }
            self.settle_agent_notice(&held.notice);
        }
    }

    /// Was held note `seq` taken back off the board unread?
    fn retracted(&self, seq: Option<u64>) -> bool {
        seq.is_some_and(|seq| self.registry.take_retracted(seq))
    }

    /// Record + commit one shell's notice cell — commits are view-gated
    /// (invariant 4: history always records; an overlay return repaints from
    /// it).
    fn settle_bg_completion(&mut self, completion: &BgCompletion) {
        let notice = self.app.record_background_notice(completion);
        if self.commits_allowed() {
            let width = self.term.screen().width;
            // A completion can settle right after a turn whose strip just
            // collapsed — reseat the viewport like every post-stream commit
            // so the notice replaces the strip's rows in place (invariant 3).
            // Mid-turn this re-asserts the current strip-aware height (a
            // no-op sync; `paint_live` re-syncs before any pending flush).
            let height = self.live_region_height();
            self.term.set_view_height(height);
            self.term
                .insert_before(ui::background_notice_lines(&notice, width));
            self.term.insert_before(vec![Line::default()]);
        }
    }

    /// Record + commit one background agent's notice cell, updating its
    /// recorded group entry first so the Ctrl+O cell shows the final response
    /// (docs/agent-tool.md).
    fn settle_agent_notice(&mut self, notice: &alter_zero::app::AgentNotice) {
        self.app.settle_agent_completion(notice);
        self.app.record_agent_notice(notice);
        if self.commits_allowed() {
            let width = self.term.screen().width;
            let height = self.live_region_height();
            self.term.set_view_height(height);
            self.term
                .insert_before(ui::agent_notice_lines(notice, width));
            self.term.insert_before(vec![Line::default()]);
        }
    }
}
