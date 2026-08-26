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

use std::time::{Duration, Instant};

use ratatui::text::Line;

use alter_zero::agents::{AGENT_LINGER, AgentEvent};
use alter_zero::app::{AgentStop, HistoryItem, Role, ToastKind, View};
use alter_zero::stream::{AgentChatDelivery, StreamEvent};
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
        if let StreamEvent::Permission(mut request) = event {
            // Stamp **which** agent asked. The backend fills the type (the
            // title's `· from the general-purpose agent`); only this channel
            // knows the id, and the prompt's context cells need it to tell
            // the viewed agent's own batch from another conversation's
            // (`docs/permissions.md`).
            request.agent_id = Some(id.to_string());
            self.app.open_permission(request);
            return;
        }
        // Screen commits pause while a framed view's flow covers the agent
        // session view (a menu opened from its palette, `docs/view-flow.md`)
        // — the flow-exit rebuild regenerates the agent transcript from its
        // history, exactly like the view's own returns.
        let viewing = self.viewing_agent(id);
        // Freeze the entry's runtime at its live value before a settling event
        // (the per-frame injection stops once the status is final).
        if let Some(elapsed) = self.agent_clocks.get(id).map(Instant::elapsed) {
            self.app.set_agent_runtime(id, elapsed);
        }
        // A thinking phase opens: start its clock, and — with the display on
        // (`/settings` **Hide thinking**, read per phase like the main
        // session's) — open the agent's reasoning buffer below, which is the
        // whole gate. See `docs/agent-view-streaming.md`.
        let opening = matches!(event, StreamEvent::ThinkingStart) && self.show_thinking();
        if matches!(event, StreamEvent::ThinkingStart) {
            self.agent_thinking_clocks
                .insert(id.to_string(), Instant::now());
            self.app.set_agent_thinking(id, Duration::ZERO);
        }
        // Settle the phase this event ends — the one settle helper, at all
        // three of its points: `ThinkingEnd`, a backend error, and a run that
        // ended still thinking. **Before** the segment flush below, because
        // the fold records the flushed text after the cell
        // (`AgentRun::flush_segment` runs inside `apply`, which comes last),
        // and screen order must match history order or a rebuild reorders
        // them (invariant 4). With no text buffered — the only state a real
        // backend reaches, since opening a phase flushes — both are no-ops
        // and the order does not arise.
        if matches!(
            event,
            StreamEvent::ThinkingEnd | StreamEvent::StreamDone | StreamEvent::Error(_)
        ) {
            self.settle_agent_reasoning(id, viewing, width);
        }
        // The view's segment boundaries: the agent's streamed text finalises
        // before a tool cell / the live thinking block / the end of the run,
        // exactly like the main loop's flush points. Committed BEFORE the fold
        // (the fold consumes the buffer).
        //
        // **Which events those are is the fold's own answer**
        // ([`AgentRun::flushes_segment`]), not a list kept here: the
        // hand-kept one drifted, missing `HookNote` — whose arm flushes the
        // buffer — so a `SubagentStop` continuation dropped the reply's
        // withheld tail from the screen *and* left `agent_render` holding a
        // prefix of a buffer that no longer existed, which the continuation's
        // next `Chunk` then committed against
        // (`docs/agent-view-streaming.md`). `ThinkingStart` stays separate
        // (`opening`): it flushes only when the display is on, and that gate
        // is the boundary's.
        if viewing
            && (opening || alter_zero::agents::AgentRun::flushes_segment(&event))
            && let Some(text) = self
                .app
                .viewed_agent()
                .and_then(|run| run.streaming.clone())
                .filter(|text| !text.is_empty())
        {
            self.flush_agent_segment(&text, width);
        }
        // A phase opening records the flushed segment on the agent's own
        // transcript and opens the buffer — the `ThinkingStart` dance, so the
        // `● Thinking…` header sits under the same blank spacer the settled
        // cell will take (`docs/thinking-stream.md`).
        if opening {
            self.app.begin_agent_reasoning(id);
        }
        // What the fold appends to the agent's transcript is what the screen
        // owes it — see `commit_agent_view_event`.
        let recorded = self.app.agent(id).map_or(0, |run| run.history.len());
        let settled = self.app.apply_agent_event(id, &event);
        if viewing {
            self.commit_agent_view_event(&event, width, recorded);
        }
        if let Some(notice) = settled {
            // A background agent completed on its own: the model-facing note
            // goes on the shared board (from_model — its untaken presence at a
            // turn boundary starts the automatic follow-up turn), the notice
            // cell defers to the next safe boundary (docs/agent-tool.md).
            self.registry.post_notice(notice.context_text(), true);
            self.app.defer_agent_notice(notice);
        }
        if let Some(linger) = self
            .app
            .agent(id)
            .filter(|run| run.status.is_final())
            .map(|run| run.linger())
        {
            self.agent_clocks.remove(id);
            self.agent_expiry
                .insert(id.to_string(), Instant::now() + linger);
        }
    }

    /// Commit the run of streamed text `text` finalises — the tail
    /// `agent_render` withheld, then the spacer — and reset the render so the
    /// next segment starts clean.
    ///
    /// One helper for both kinds of segment boundary: the event fold's flush
    /// block above, and the **local** settles that carry no event at all
    /// ([`Session::begin_local_agent_settle`], which finalise the same buffer
    /// through `AgentRun::interrupt`). Those used to skip it entirely, so a
    /// settled agent's partial reply lost its withheld tail from the view's
    /// scrollback and left the render stale for the chat continuation that
    /// follows (`docs/agent-view-streaming.md`).
    fn flush_agent_segment(&mut self, text: &str, width: u16) {
        self.term
            .insert_before(self.agent_render.finish(text, width));
        self.term.insert_before(vec![Line::default()]);
        self.agent_render.reset();
    }

    /// The pre-fold work the session view owes a **local** settle of `id` —
    /// one that carries no `StreamEvent`, so `on_agent_event`'s flush block
    /// never runs. Returns the agent's transcript length, for the
    /// [`commit_agent_tail`](Session::commit_agent_tail) that must follow the
    /// settle itself.
    ///
    /// `AgentRun::interrupt` finalises the same three things a settling event
    /// does — the open thought, the run of streamed text, the running call —
    /// and the first two have committers of their own that a bare
    /// `commit_agent_tail` never triggers. Without this the settled thought
    /// and the partial reply's withheld tail never reached the screen, and
    /// `agent_render` stayed holding a prefix of a buffer `flush_segment`
    /// had already taken — which the next chat continuation's `Chunk` then
    /// committed against (`docs/agent-view-streaming.md`).
    ///
    /// **Three doors reach that settle**, and they all come through here: the
    /// roster's `x` ([`Session::stop_agent`]), and the turn-level resolutions
    /// that take a foreground group down with them —
    /// `App::resolve_live_agent_group`, reached from the backend `Error` arm
    /// and the Esc interrupt.
    fn begin_local_agent_settle(&mut self, id: &str, width: u16) -> usize {
        let on_screen = self.viewing_agent(id);
        self.settle_agent_reasoning(id, on_screen, width);
        if on_screen
            && let Some(text) = self
                .app
                .agent(id)
                .and_then(|run| run.streaming.clone())
                .filter(|text| !text.is_empty())
        {
            self.flush_agent_segment(&text, width);
        }
        self.app.agent(id).map_or(0, |run| run.history.len())
    }

    /// The viewed agent when the turn is about to settle it *locally* — it is
    /// a member of the live foreground group `App::resolve_live_agent_group`
    /// interrupts on a backend error or an Esc. `None` whenever no agent view
    /// is open, the viewed agent is not in that group, or it has already
    /// settled: nothing is being taken from under the screen, so the view
    /// owes nothing.
    fn viewed_agent_losing_its_turn(&self) -> Option<String> {
        // Only a turn that is actually in flight reaches
        // `App::resolve_live_agent_group` — both `fail_stream` and
        // `interrupt_turn` return early otherwise, and a pre-fold flush with
        // no settle behind it would reset the render while the buffer still
        // held its text, re-committing the whole reply on the next chunk.
        if !self.app.is_streaming() && !self.app.turn_active() {
            return None;
        }
        let id = self.app.agent_view.clone()?;
        let live = self.app.agent_group()?;
        if !live.ids.contains(&id) {
            return None;
        }
        self.app
            .agent(&id)
            .filter(|run| !run.status.is_final())
            .map(|_| id)
    }

    /// [`begin_local_agent_settle`](Session::begin_local_agent_settle) for the
    /// viewed agent when a turn-level resolution is about to take its group
    /// down — the backend `Error` arm and the Esc interrupt. Returns what the
    /// matching [`Session::finish_turn_agent_settle`] needs, or `None` when
    /// nothing on screen is being settled.
    pub(crate) fn begin_turn_agent_settle(&mut self, width: u16) -> Option<(String, usize)> {
        let id = self.viewed_agent_losing_its_turn()?;
        let recorded = self.begin_local_agent_settle(&id, width);
        Some((id, recorded))
    }

    /// Commit the cell the local settle recorded on the viewed agent's own
    /// transcript — the second half of [`Session::begin_turn_agent_settle`],
    /// run after the resolution itself.
    pub(crate) fn finish_turn_agent_settle(&mut self, owed: Option<(String, usize)>, width: u16) {
        if let Some((id, recorded)) = owed
            && self.viewing_agent(&id)
        {
            self.commit_agent_tail(recorded, width);
        }
    }

    /// Is the thinking display on? The `/settings` **Hide thinking** knob,
    /// read per phase — so flipping it mid-session takes effect on the very
    /// next one, exactly as the main session reads it (`docs/settings.md`).
    fn show_thinking(&self) -> bool {
        self.app.settings().show_thinking()
    }

    /// Close one subagent's thinking phase: record its `Thought for … · …
    /// tokens` cell on **that agent's** transcript and, when its session view
    /// is on screen, commit the collapsed line.
    ///
    /// The sibling of [`Session::settle_reasoning`], and the one settle helper
    /// for all three points a phase can end — its own `ThinkingEnd`, the
    /// agent's backend error, and a run that ended still thinking — plus the
    /// user's `x`, which settles through the pure `AgentRun::interrupt`.
    /// A no-op when no phase is open (the display is off, or it streamed no
    /// text), so the call sites need no condition of their own.
    ///
    /// **Reseat before you commit** (invariant 3): the strip loses the live
    /// block's rows as the cell lands, so the box must not rise off the
    /// bottom. See `docs/agent-view-streaming.md`.
    fn settle_agent_reasoning(&mut self, id: &str, viewing: bool, width: u16) {
        let secs = self
            .agent_thinking_clocks
            .remove(id)
            .map_or(0, |start| start.elapsed().as_secs());
        let Some(reasoning) = self.app.finish_agent_reasoning(id, secs) else {
            return;
        };
        if !viewing {
            return;
        }
        let height = self.live_region_height();
        self.term.set_view_height(height);
        self.term
            .insert_before(ui::reasoning_lines(&reasoning, width));
        self.term.insert_before(vec![Line::default()]);
    }

    /// Is `id`'s session the conversation currently on the inline screen?
    ///
    /// Commits pause while a framed view's flow covers it (a menu opened from
    /// its palette, `docs/view-flow.md`) — the flow-exit rebuild regenerates
    /// the transcript from its history, exactly like the view's own returns.
    fn viewing_agent(&self, id: &str) -> bool {
        self.app.view == View::Conversation
            && self.app.agent_view.as_deref() == Some(id)
            && self.flowed_view.is_none()
    }

    /// Commit whatever was just appended to the **viewed** agent's transcript
    /// past `recorded` — its resolved call's collapsed cell, or the turn's
    /// summary — reseating the viewport first, since the strip loses the live
    /// cell (or the whole status line) as it lands (invariant 3).
    ///
    /// The one place the session view turns a transcript item into scrollback,
    /// shared by the event fold and the **local** settles that carry no event
    /// at all (the roster's `x` resolves the running call itself). Keying on
    /// the recorded item rather than on what triggered it is what stops this
    /// view falling behind the main one again — see
    /// [`Session::commit_agent_view_event`].
    ///
    /// Every other item has its own committer and must **not** land twice: a
    /// flushed text segment belongs to `agent_render`
    /// ([`Session::on_agent_event`]'s flush block), a settled thought to
    /// [`Session::settle_agent_reasoning`], and a hook note is invisible
    /// inline by design (`docs/hooks.md`).
    fn commit_agent_tail(&mut self, recorded: usize, width: u16) {
        let owed = self
            .app
            .viewed_agent()
            .filter(|run| run.history.len() > recorded)
            .and_then(|run| match run.history.last() {
                // A resolved call — its collapsed cell, through the same
                // history-derived builder the rebuild uses, which also holds
                // an MCP run's members until the run ends (`docs/mcp.md`).
                Some(HistoryItem::Tool(_)) => {
                    ui::tool_commit_lines(&run.history, &run.tool_queue, width)
                }
                // The turn's `Done for Ns · {n} tokens` receipt
                // (`docs/agent-tool.md`).
                Some(HistoryItem::Summary(summary)) => Some(ui::summary_lines(summary, width)),
                // A message the user queued into this agent, now that its loop
                // has read it (`docs/queue.md`) — the bubble lands where the
                // agent saw it, not where it was typed.
                Some(HistoryItem::Message(message)) if message.role == Role::User => {
                    Some(ui::message_lines(Role::User, &message.text, width))
                }
                _ => None,
            })
            .filter(|lines| !lines.is_empty());
        if let Some(lines) = owed {
            let height = self.live_region_height();
            self.term.set_view_height(height);
            self.term.insert_before(lines);
            self.term.insert_before(vec![Line::default()]);
        }
    }

    /// The screen half of [`Session::on_agent_event`], for the agent whose
    /// session view is open: commit its streamed rows, whatever the fold just
    /// recorded on its transcript, and its error notice — reseating the
    /// viewport first when its strip collapses (invariant 3). `recorded` is
    /// the transcript's length *before* the fold.
    ///
    /// **Keyed on the item the fold appended, not on the event.** The event
    /// list is what drifted: `write`/`edit` moved onto the two-text
    /// `StreamEvent::ToolAnswered` when the file tools started handing the
    /// model a one-line ack, the main view's arm was updated and this one was
    /// not, so a subagent's file cells reached its transcript and stopped
    /// there — appearing only when a resize rebuilt the view from that same
    /// history (the reported bug, `docs/agent-view-streaming.md`). Asking
    /// what `AgentRun::apply` actually recorded cannot fall behind a new
    /// resolution event the way a hand-kept list does, and it is the same
    /// question a rebuild answers, which is what keeps the two agreeing.
    ///
    /// Every other item has its own committer and must **not** be committed
    /// twice here: a flushed text segment belongs to `agent_render`
    /// ([`Session::on_agent_event`]'s flush block), a settled thought to
    /// [`Session::settle_agent_reasoning`], and a hook note is invisible
    /// inline by design (`docs/hooks.md`).
    fn commit_agent_view_event(&mut self, event: &StreamEvent, width: u16, recorded: usize) {
        if matches!(event, StreamEvent::Chunk(_)) {
            if let Some(text) = self
                .app
                .viewed_agent()
                .and_then(|run| run.streaming.as_deref())
            {
                let lines = self.agent_render.commit(text, width);
                self.term.insert_before(lines);
            }
            return;
        }
        self.commit_agent_tail(recorded, width);
        // The red notice is the one thing a failure does *not* record on the
        // transcript (the run keeps only its `error` field), so it commits
        // here — after the cell the failure resolved, the main view's order.
        if let StreamEvent::Error(message) = event {
            let height = self.live_region_height();
            self.term.set_view_height(height);
            self.term
                .insert_before(ui::message_lines(Role::Error, message, width));
            self.term.insert_before(vec![Line::default()]);
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
        let now = Instant::now();
        for id in ids {
            self.agent_clocks.remove(&id);
            // `or_insert`: a member the user already stopped is counting down
            // its own (longer) linger — the group's resolution must not
            // restart it, let alone shorten it to the natural one.
            if let Some(linger) = self
                .app
                .agent(&id)
                .filter(|run| run.status.is_final())
                .map(|run| run.linger())
            {
                self.agent_expiry.entry(id).or_insert(now + linger);
            }
        }
    }

    /// `x` on a roster row — the stop, then the clear (`docs/agent-tool.md`).
    ///
    /// The **first** `x` cancels the subagent thread and settles the entry as
    /// interrupted; the row stays, red, for `AGENT_STOPPED_LINGER` (the
    /// entry's own [`linger`](alter_zero::agents::AgentRun::linger)) so the
    /// user sees what they stopped. A background agent's stopped notice
    /// settles like a completion (a foreground one resolves with its group
    /// when the backend's wait loop sees the kill), and with nothing in flight
    /// it starts the follow-up turn. The **second** `x` clears that row: the
    /// app hides it and the sweep collects the entry at the next tick.
    pub(crate) fn stop_agent(&mut self, id: &str) -> std::io::Result<()> {
        // Deliberately looser than `viewing_agent`: a *clear* must take the
        // screen back even from under a framed view's flow, since the view it
        // covers is about to stop existing.
        let viewing = self.app.agent_view.as_deref() == Some(id);
        // The stop resolves the agent's running call **locally** — no event
        // follows to carry its cell — so the session view commits what the
        // stop recorded, exactly as the fold does
        // (`docs/agent-view-streaming.md`).
        let width = self.term.screen().width;
        let on_screen = self.viewing_agent(id);
        let recorded = self.begin_local_agent_settle(id, width);
        match self.app.stop_agent(id) {
            Some(AgentStop::Stopped(notice)) => {
                if on_screen {
                    self.commit_agent_tail(recorded, width);
                }
                let _ = self.agent_registry.kill(id);
                self.agent_clocks.remove(id);
                // The open thought settled above, through the same helper the
                // event path uses — this only makes sure no clock survives to
                // re-arm the phase on the next roster tick.
                self.agent_thinking_clocks.remove(id);
                let linger = self.app.agent(id).map_or(AGENT_LINGER, |run| run.linger());
                self.agent_expiry
                    .insert(id.to_string(), Instant::now() + linger);
                if let Some(notice) = notice {
                    self.registry.post_notice(notice.context_text(), true);
                    self.app.defer_agent_notice(notice);
                    if !self.app.turn_active() {
                        self.dispatch_after_turn();
                    }
                }
            }
            // Cleared: the row is already off the roster — expire the entry
            // now so the sweep (which also unregisters it) collects it on the
            // next tick, deferring only while its session view is open.
            Some(AgentStop::Cleared) => {
                self.agent_clocks.remove(id);
                self.agent_expiry.insert(id.to_string(), Instant::now());
                // The cleared agent was the session on screen — `App` closed
                // the view with the row, so take the screen back to the main
                // conversation (the Esc-from-the-view repaint).
                if viewing {
                    self.leave_agent_view()?;
                }
            }
            None => {}
        }
        self.frame.schedule_frame();
        Ok(())
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

    /// Enter inside an agent session: deliver the draft to the agent. The
    /// **registry** decides how — it is the only thing that knows whether the
    /// loop is still running — and the view follows its answer
    /// (`docs/queue.md`):
    ///
    /// - **running** → the message joins that agent's queue, showing above the
    ///   box like the main session's steered rows, and reaches its loop at the
    ///   next round boundary. Nothing is committed and nothing is claimed on
    ///   its transcript: `StreamEvent::Steered` does both, when the agent
    ///   genuinely has it.
    /// - **idle** → a continuation run started with the message as its newest
    ///   user turn, so the transcript records it now and the bubble commits
    ///   here — an idle submit, one level down.
    pub(crate) fn agent_chat(&mut self, id: &str, text: &str) {
        match self.models.backend().spawn_agent_chat(id, text) {
            AgentChatDelivery::Queued => {
                self.app.queue_agent_chat(id, text);
                self.agent_expiry.remove(id);
            }
            AgentChatDelivery::Started => {
                self.app.agent_chat(id, text);
                let width = self.term.screen().width;
                if self.viewing_agent(id) {
                    let height = self.live_region_height();
                    self.term.set_view_height(height);
                    self.term
                        .insert_before(ui::message_lines(Role::User, text, width));
                    self.term.insert_before(vec![Line::default()]);
                }
                self.agent_clocks
                    .entry(id.to_string())
                    .or_insert_with(Instant::now);
                self.agent_expiry.remove(id);
            }
            AgentChatDelivery::Declined => self.toast(
                "Agent chat is not available with this backend",
                ToastKind::Error,
            ),
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
        // The open thinking phases' elapsed, the same injection: cleared
        // first, so an agent whose phase ended (its clock is gone) drops the
        // `Thinking for Ns` clause instead of freezing it
        // (`docs/agent-view-streaming.md`).
        self.app.clear_agent_thinking();
        for (id, started) in &self.agent_thinking_clocks {
            self.app.set_agent_thinking(id, started.elapsed());
        }
        let now = Instant::now();
        let settled: Vec<(String, std::time::Duration)> = self
            .app
            .agents()
            .iter()
            .filter(|run| run.status.is_final())
            .map(|run| (run.id.clone(), run.linger()))
            .collect();
        for (id, linger) in settled {
            self.agent_expiry.entry(id).or_insert(now + linger);
        }
        // Sweep finished agents whose linger expired — deferred while the user is
        // inside that agent's session view (the deadline pushes forward, so
        // leaving restarts the full linger), and a timer whose agent reopened (a
        // chat continuation) is dropped.
        let Session {
            agent_expiry,
            app,
            agent_registry,
            agent_thinking_clocks,
            ..
        } = self;
        agent_expiry.retain(|id, deadline| {
            if app.agent(id).is_none_or(|run| !run.status.is_final()) {
                return false;
            }
            if app.agent_view.as_deref() == Some(id.as_str()) {
                *deadline = now + app.agent(id).map_or(AGENT_LINGER, |run| run.linger());
                return true;
            }
            if now >= *deadline {
                app.remove_agent(id);
                agent_registry.remove(id);
                agent_thinking_clocks.remove(id);
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
