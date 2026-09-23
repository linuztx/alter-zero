//! Background shells: their events, and when their notices are allowed to
//! appear (`docs/background.md`).
//!
//! A `run_in_background` bash call — or a Ctrl+B hand-off — leaves a process
//! running past the turn that started it, on its own channel that is never
//! swapped (unlike the reply channel), so shells survive an interrupt and
//! `/clear` kills them explicitly.
//!
//! A completion has to reach two audiences, and the split is the whole point of
//! [`Session::on_bg_event`]:
//!
//! - **The model, immediately.** The note goes onto the registry's notice board
//!   the moment the process exits, so an in-flight agent picks it up before its
//!   next round — a shell it just `kill`ed is known to it within the same turn.
//!   A shell launched *by a subagent* reports to that agent first, through the
//!   registry's pending-input seam.
//! - **The user, at a safe boundary.** The `● Background command "…" completed`
//!   cell waits for [`Session::settle_bg_completions`], which every tool
//!   resolution and every turn end calls. Those are the points where the
//!   streaming buffer is empty, so a notice cell can never split a committed
//!   reply (invariants 2/3).
//!
//! With nothing in flight, a model-launched completion no agent read starts the
//! automatic follow-up turn instead, so the model can report the result.

use std::time::Instant;

use ratatui::text::Line;

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
                if let Some(completion) = self.app.bg_exited(&id, code, killed) {
                    // A subagent-launched shell reports to its launcher first:
                    // the note queues onto that agent's seam and is heard at
                    // its next round boundary — the same seam a user's chat
                    // message rides (docs/queue.md). Queueing is **all** this
                    // does: `StreamEvent::Steered` records the note on the
                    // agent's transcript when the loop actually takes it, and
                    // the session view commits it from there. Recording it
                    // here as well put it on the transcript twice — once
                    // eagerly, once on the echo — permanently, in history, the
                    // rollout and every rebuild.
                    //
                    // A launcher that already settled can't hear it: the shared
                    // board takes the note instead, so the main turn (or the
                    // idle follow-up turn) relays the outcome
                    // (docs/agent-tool.md).
                    let note = completion.context_text();
                    let routed = completion.origin.as_ref().is_some_and(|origin| {
                        self.agent_registry.queue_input(&origin.agent_id, &note)
                    });
                    if !routed {
                        self.registry.post_notice(note, completion.from_model);
                    }
                    self.app.defer_bg_completion(completion);
                    if !self.app.turn_active() {
                        self.dispatch_after_turn();
                    }
                }
            }
        }
        self.frame.schedule_frame();
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

    /// Settle the background completions held so far (empty when none landed):
    /// record + commit each notice in arrival order — commits are view-gated
    /// (invariant 4: history always records; an overlay return repaints from it).
    /// Runs at every **safe boundary** — each tool resolution / segment flush
    /// mid-turn, every turn end, and the idle arrival — points where the
    /// streaming buffer is empty, so a notice cell can never split a committed
    /// reply (invariants 2/3).
    pub(crate) fn settle_bg_completions(&mut self) {
        let committing = self.commits_allowed();
        for completion in self.app.take_pending_bg_completions() {
            let notice = self.app.record_background_notice(&completion);
            if committing {
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
        // Background-agent completions settle at the same boundaries — the green
        // `● Agent "…" finished` / red stopped cell — and update the recorded
        // group entry so the Ctrl+O cell shows the final response
        // (docs/agent-tool.md).
        for notice in self.app.take_pending_agent_notices() {
            self.app.record_agent_notice(&notice);
            self.app.settle_agent_completion(&notice);
            if committing {
                let width = self.term.screen().width;
                let height = self.live_region_height();
                self.term.set_view_height(height);
                self.term
                    .insert_before(ui::agent_notice_lines(&notice, width));
                self.term.insert_before(vec![Line::default()]);
            }
        }
    }
}
