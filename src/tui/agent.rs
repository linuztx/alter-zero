//! Subagents: their events, their roster, and the session view onto one
//! (`docs/agent-tool.md`).
//!
//! The model's `agent` tool launches side-agents that run their own tool loops
//! on their own threads and report on a **dedicated** channel. That channel is
//! separate for a reason: agents outlive turns, while the reply channel is
//! swapped on every interrupt and `/clear`.
//!
//! [`Session::on_agent_event`] folds one event into its roster entry — which is
//! what the footer roster, the live group cell and the Ctrl+O cells all render
//! from — and, while the user is **inside that agent's session view**, commits it
//! to the screen incrementally through `agent_render`, mirroring
//! `Session::on_stream_event`'s commit shape over the agent's own state.
//!
//! Two things reach outside the roster:
//!
//! - **A permission request is the user's business, not the roster's.** It raises
//!   the same shared prompt the main turn does and stops there; the agent's own
//!   state is untouched while it waits (`docs/permissions.md`).
//! - **A settled background agent posts its note on the shared board**, so the
//!   in-flight main turn hears it at its next round and an idle completion starts
//!   the follow-up turn — the background-shell pattern exactly.
//!
//! The clocks here are the boundary's: [`Session::tick_agent_roster`] injects each
//! running agent's elapsed before a draw and sweeps finished rows once their
//! `AGENT_LINGER` is up — deferred while the user is inside that agent's view, so
//! leaving restarts the full linger.

use std::time::Instant;

use ratatui::text::Line;

use alter_zero::agents::{AGENT_LINGER, AgentEvent};
use alter_zero::app::{Role, ToastKind, View};
use alter_zero::stream::StreamEvent;
use alter_zero::ui;

use super::Session;

impl Session<'_> {
    /// Fold one subagent event into its roster entry, committing to the screen
    /// when the user is inside that agent's session view (see the module doc).
    pub(crate) fn on_agent_event(&mut self, id: &str, event: StreamEvent) {
        let width = self.term.screen().width;
        // A subagent's permission request is the *user's* business, not the
        // roster's: raise the same shared prompt the main turn does, and stop —
        // the agent's own state is untouched while it waits
        // (docs/permissions.md).
        if let StreamEvent::Permission(request) = event {
            self.app.open_permission(request);
            return;
        }
        // Screen commits pause while a framed view's flow covers the agent
        // session view (a menu opened from its palette, `docs/view-flow.md`)
        // — the flow-exit rebuild regenerates the agent transcript from its
        // history, exactly like the view's own returns.
        let viewing = self.app.view == View::Conversation
            && self.app.agent_view.as_deref() == Some(id)
            && self.flowed_view.is_none();
        // Freeze the entry's runtime at its live value before a settling event
        // (the per-frame injection stops once the status is final).
        if let Some(elapsed) = self.agent_clocks.get(id).map(Instant::elapsed) {
            self.app.set_agent_runtime(id, elapsed);
        }
        // The view's segment boundaries: the agent's streamed text finalises
        // before a tool cell / the end of the run, exactly like the main loop's
        // flush points. Committed BEFORE the fold (the fold consumes the buffer).
        if viewing
            && matches!(
                event,
                StreamEvent::ToolBatch(_)
                    | StreamEvent::ToolStart { .. }
                    | StreamEvent::StreamDone
                    | StreamEvent::Error(_)
            )
            && let Some(text) = self
                .app
                .viewed_agent()
                .and_then(|run| run.streaming.clone())
                .filter(|text| !text.is_empty())
        {
            self.term
                .insert_before(self.agent_render.finish(&text, width));
            self.term.insert_before(vec![Line::default()]);
            self.agent_render.reset();
        }
        let settled = self.app.apply_agent_event(id, &event);
        if viewing {
            self.commit_agent_view_event(&event, width);
        }
        if let Some(notice) = settled {
            // A background agent completed on its own: the model-facing note
            // goes on the shared board (from_model — its untaken presence at a
            // turn boundary starts the automatic follow-up turn), the notice
            // cell defers to the next safe boundary (docs/agent-tool.md).
            self.registry.post_notice(notice.context_text(), true);
            self.app.defer_agent_notice(notice);
        }
        if self.app.agent(id).is_some_and(|run| run.status.is_final()) {
            self.agent_clocks.remove(id);
            self.agent_expiry
                .insert(id.to_string(), Instant::now() + AGENT_LINGER);
        }
    }

    /// The screen half of [`Session::on_agent_event`], for the agent whose session
    /// view is open: commit its streamed rows, its resolved tool cells, its error
    /// notice — and reseat the viewport when its strip collapses (invariant 3).
    fn commit_agent_view_event(&mut self, event: &StreamEvent, width: u16) {
        match event {
            StreamEvent::Chunk(_) => {
                if let Some(text) = self
                    .app
                    .viewed_agent()
                    .and_then(|run| run.streaming.as_deref())
                {
                    let lines = self.agent_render.commit(text, width);
                    self.term.insert_before(lines);
                }
            }
            StreamEvent::ToolEnd { .. }
            | StreamEvent::ToolRejected { .. }
            | StreamEvent::ToolBackgrounded { .. } => {
                // The resolved call was pushed onto the agent's transcript —
                // commit its collapsed cell (the main ToolEnd dance), through
                // the same history-derived builder, so the agent view and its
                // rebuild agree the way the main view's do (`docs/mcp.md`).
                let lines = self
                    .app
                    .viewed_agent()
                    .and_then(|run| ui::tool_commit_lines(&run.history, &run.tool_queue, width));
                if let Some(lines) = lines
                    && !lines.is_empty()
                {
                    let height = self.live_region_height();
                    self.term.set_view_height(height);
                    self.term.insert_before(lines);
                    self.term.insert_before(vec![Line::default()]);
                }
            }
            StreamEvent::Error(message) => {
                let height = self.live_region_height();
                self.term.set_view_height(height);
                self.term
                    .insert_before(ui::message_lines(Role::Error, message, width));
                self.term.insert_before(vec![Line::default()]);
            }
            StreamEvent::StreamDone => {
                // The strip collapses (the agent's status clears) — reseat so
                // the box stays flush (invariant 3). No summary cell: the
                // agent session keeps codex's quiet end.
                let height = self.live_region_height();
                self.term.set_view_height(height);
            }
            _ => {}
        }
    }

    /// Drain the subagent channel before folding a group's resolution: the
    /// members' terminal events were enqueued *before* the backend sent the
    /// resolution, so taking them first is what makes the roster snapshots the
    /// recorded group entries are built from final (`docs/agent-tool.md`).
    pub(crate) fn drain_agent_events(&mut self) {
        while let Ok(AgentEvent::Stream { id, event }) = self.agent_rx.try_recv() {
            self.on_agent_event(&id, event);
        }
    }

    /// A foreground group's members are settled — arm their linger sweeps (a
    /// background group's keep running).
    pub(crate) fn settle_group_members(&mut self, ids: Vec<String>) {
        for id in ids {
            self.agent_clocks.remove(&id);
            if self.app.agent(&id).is_some_and(|run| run.status.is_final()) {
                self.agent_expiry.insert(id, Instant::now() + AGENT_LINGER);
            }
        }
    }

    /// `x` on a roster row: cancel the subagent thread and settle the entry as
    /// interrupted — the row leaves the footer at once. A background agent's
    /// stopped notice settles like a completion (a foreground one resolves with
    /// its group when the backend's wait loop sees the kill), and with nothing in
    /// flight it starts the follow-up turn.
    pub(crate) fn stop_agent(&mut self, id: &str) {
        let _ = self.agent_registry.kill(id);
        self.agent_clocks.remove(id);
        self.agent_expiry.remove(id);
        if let Some(notice) = self.app.stop_agent(id).flatten() {
            self.registry.post_notice(notice.context_text(), true);
            self.app.defer_agent_notice(notice);
            // The stopped background agent is done for good — drop it from the
            // roster now (the user's `x` removes the row right away).
            self.app.remove_agent(id);
            self.agent_registry.remove(id);
            if !self.app.turn_active() {
                self.dispatch_after_turn();
            }
        }
        self.frame.schedule_frame();
    }

    /// Enter on a roster row: swap the screen to that agent's own inline session
    /// — purge + rebuild from its transcript, the `/clear` shape.
    pub(crate) fn enter_agent_view(&mut self) -> std::io::Result<()> {
        self.repaint_agent_view()
    }

    /// Esc (or Enter on `● main`): back to the main conversation — purge +
    /// rebuild from history, the in-flight partial included. The viewed agent's
    /// linger re-arms via the sweep (its deadline was pushed while viewed).
    pub(crate) fn leave_agent_view(&mut self) -> std::io::Result<()> {
        self.repaint_conversation()
    }

    /// Enter inside an agent session: deliver the draft to the agent — queued
    /// into its running loop, or a continuation run when idle. The transcript
    /// already recorded it; commit the bubble in place.
    pub(crate) fn agent_chat(&mut self, id: &str, text: &str) {
        if self.models.backend().spawn_agent_chat(id, text) {
            let width = self.term.screen().width;
            if self.flowed_view.is_none() {
                self.term
                    .insert_before(ui::message_lines(Role::User, text, width));
                self.term.insert_before(vec![Line::default()]);
            }
            self.agent_clocks
                .entry(id.to_string())
                .or_insert_with(Instant::now);
            self.agent_expiry.remove(id);
        } else {
            self.toast(
                "Agent chat is not available with this backend",
                ToastKind::Error,
            );
        }
    }

    /// Inject each running agent's elapsed before a draw (so the roster's
    /// counters tick), then arm and run the linger sweep over the finished ones.
    ///
    /// The arming is deliberately self-healing over *every* settle path — a group
    /// resolution, an Esc interrupt, a backend error, an `x` — so no path can
    /// strand a finished row on the roster.
    pub(crate) fn tick_agent_roster(&mut self) {
        for (id, started) in &self.agent_clocks {
            self.app.set_agent_runtime(id, started.elapsed());
        }
        let now = Instant::now();
        let settled: Vec<String> = self
            .app
            .agents()
            .iter()
            .filter(|run| run.status.is_final())
            .map(|run| run.id.clone())
            .collect();
        for id in settled {
            self.agent_expiry.entry(id).or_insert(now + AGENT_LINGER);
        }
        // Sweep finished agents whose linger expired — deferred while the user is
        // inside that agent's session view (the deadline pushes forward, so
        // leaving restarts the full linger), and a timer whose agent reopened (a
        // chat continuation) is dropped.
        let Session {
            agent_expiry,
            app,
            agent_registry,
            ..
        } = self;
        agent_expiry.retain(|id, deadline| {
            if app.agent(id).is_none_or(|run| !run.status.is_final()) {
                return false;
            }
            if app.agent_view.as_deref() == Some(id.as_str()) {
                *deadline = now + AGENT_LINGER;
                return true;
            }
            if now >= *deadline {
                app.remove_agent(id);
                agent_registry.remove(id);
                return false;
            }
            true
        });
    }
}

impl Session<'_> {
    /// The subagent channel's branch: fold the event in, then — with nothing in
    /// flight and a settled background agent's note waiting — settle it at once and
    /// start the automatic follow-up turn (the background-shell pattern).
    pub(crate) fn on_agent_stream(&mut self, id: &str, event: StreamEvent) {
        self.on_agent_event(id, event);
        if !self.app.turn_active() && self.app.has_pending_agent_notices() {
            self.dispatch_after_turn();
        }
        self.frame.schedule_frame();
    }
}
