//! The `Agent` tool's [`App`](super::App) state: the live group, the footer
//! roster, the recorded groups/notices, and the inline session view.
//! See `docs/agent-tool.md`.

use super::*;

/// One subagent of a recorded [`AgentGroup`] (`docs/agent-tool.md`). The
/// display fields (`status`, counters, `result`, `tool_headers`) snapshot the
/// roster at resolution — and are **updated once more** when a background
/// agent later completes ([`App::settle_agent_completion`], bumping the
/// history generation so the Ctrl+O cache re-renders). `output` is the
/// model-facing tool result the parent actually received (the framed final
/// response, a launch acknowledgement, or the stopped/failed note) — it is
/// what [`crate::context::context_messages`] replays, and it never changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentGroupEntry {
    /// The registry id (`a7k2m9x4q`, …).
    pub id: String,
    /// The model's short task label — the tree row / `● Agent({description})`.
    pub description: String,
    /// The `subagent_type` argument (default `general-purpose`).
    pub agent_type: String,
    /// The full task prompt — the Ctrl+O cell's `⎿ Prompt:` block.
    pub prompt: String,
    /// The display status at the last snapshot.
    pub status: AgentStatus,
    pub tool_uses: usize,
    pub tokens: u64,
    /// Its runtime in whole seconds at the last snapshot.
    pub secs: u64,
    /// The final response text (the `⎿ Response:` block) — empty until known.
    pub result: String,
    /// The nested tool-call headers it ran (`Bash(curl …)` one-liners), for
    /// the Ctrl+O cell after the roster entry is swept.
    pub tool_headers: Vec<String>,
    /// The model-facing tool-result text (immutable — see the struct docs).
    pub output: String,
}

/// A resolved group of `agent` tool calls from one round, committed as a
/// single tree cell — `● {n} agents finished (ctrl+o to expand)` /
/// `● {n} background agents launched (↓ to manage)` — and expanded in the
/// Ctrl+O transcript as one `● Agent({description})` cell per entry. See
/// `docs/agent-tool.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentGroup {
    /// Whether the group resolved by *launching in the background* (the calls
    /// returned launch acknowledgements and the agents keep running).
    pub background: bool,
    pub agents: Vec<AgentGroupEntry>,
    /// Wall-clock stamp (recorded like every item's; never displayed).
    pub timestamp: String,
}

impl AgentGroup {
    /// Did every agent in the group finish cleanly? Picks the committed
    /// cell's green vs red bullet (a background launch is green by
    /// definition — nothing has failed yet).
    #[must_use]
    pub fn ok(&self) -> bool {
        self.background || self.agents.iter().all(|agent| agent.status.ok())
    }
}

/// A **background** agent's completion notice, committed to history as a
/// one-line cell — `● Agent "{description}" finished · 35s` (green) /
/// `was stopped by user` / `failed` (red) — the background-shell notice's
/// agent twin. The `result` rides the item for the model's context (the
/// final response) but is never rendered. See `docs/agent-tool.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentNotice {
    pub id: String,
    pub description: String,
    /// The terminal status (`Done` / `Failed` / `Interrupted`).
    pub status: AgentStatus,
    /// Its total runtime in whole seconds.
    pub secs: u64,
    /// The final response (or error) — context-only, never rendered.
    pub result: String,
    /// Wall-clock stamp (recorded like every item's; never displayed).
    pub timestamp: String,
}

impl AgentNotice {
    /// Did the agent finish cleanly? Green vs red bullet.
    #[must_use]
    pub const fn ok(&self) -> bool {
        self.status.ok()
    }

    /// The rendered one-liner (the user's reference wording). The runtime
    /// humanizes past a minute (`· 6m 2s`, never a bare `362s`) — the
    /// [`format_elapsed`] contract every runtime display shares.
    #[must_use]
    pub fn headline(&self) -> String {
        match self.status {
            AgentStatus::Interrupted => {
                format!("Agent \"{}\" was stopped by user", self.description)
            }
            AgentStatus::Failed => format!("Agent \"{}\" failed", self.description),
            _ => format!(
                "Agent \"{}\" finished · {}",
                self.description,
                format_elapsed(self.secs)
            ),
        }
    }

    /// The model-facing context note: outcome + the final response — the
    /// bracketed user-role form [`crate::context::context_messages`] sends,
    /// and the automatic follow-up turn's prompt text (the background-shell
    /// pattern, `docs/background.md`).
    #[must_use]
    pub fn context_text(&self) -> String {
        let outcome = match self.status {
            AgentStatus::Interrupted => "was stopped by the user".to_string(),
            AgentStatus::Failed => "failed".to_string(),
            _ => format!("completed in {}", format_elapsed(self.secs)),
        };
        let body = if self.result.trim().is_empty() {
            "(no output)"
        } else {
            self.result.trim_end_matches('\n')
        };
        format!(
            "[background agent] Agent \"{}\" {outcome}.\nFinal response:\n{body}",
            self.description,
        )
    }
}

/// The **live** agent group — the round's `agent` calls between their
/// [`StreamEvent::AgentBatch`] announcement and their resolution: the strip
/// previews it as the blue `● Running {n} agents…` tree cell (rendered from
/// the roster entries these ids name). Cleared by
/// [`App::finish_agent_group`]; an interrupt / backend error resolves it
/// locally (`App::resolve_live_agent_group`) because the channel swap drops
/// the backend's own `AgentGroupDone`. See `docs/agent-tool.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentGroupLive {
    pub ids: Vec<String>,
    pub background: bool,
}

/// What the user's `x` on a roster row did — the two halves of the
/// stop-then-clear grammar ([`App::stop_agent`], `docs/agent-tool.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStop {
    /// A **running** agent was interrupted. Its row stays, red, for the
    /// longer [`crate::agents::AGENT_STOPPED_LINGER`] — the boundary kills
    /// the thread and arms that sweep. Carries the completion notice a
    /// *background* agent owes (a foreground one resolves with its group).
    Stopped(Option<AgentNotice>),
    /// An already-settled row was **cleared** — the second `x`. It leaves the
    /// roster at once (the entry's data stays for its group's resolution
    /// until the sweep collects it).
    Cleared,
}

/// The model-facing tool result for an `agent` call the user (or an Esc
/// interrupt) stopped before completion — what the recorded group entry's
/// `output` replays into later contexts.
pub const AGENT_STOPPED_OUTPUT: &str = "[agent stopped by the user before completion]";

/// Snapshot one roster entry into a recorded [`AgentGroupEntry`] around the
/// model-facing `output` (see [`App::finish_agent_group`]).
fn agent_entry_of(agent: &AgentRun, output: &str) -> AgentGroupEntry {
    AgentGroupEntry {
        id: agent.id.clone(),
        description: agent.description.clone(),
        agent_type: agent.agent_type.clone(),
        prompt: agent.prompt.clone(),
        status: agent.status,
        tool_uses: agent.tool_uses,
        tokens: agent.tokens,
        secs: agent.runtime.as_secs(),
        result: agent.result.clone().unwrap_or_default(),
        tool_headers: agent_tool_headers(agent),
        output: output.to_string(),
    }
}

/// The nested `name(args)` one-liners of every tool call an agent ran — the
/// Ctrl+O cell's transcript summary once the roster entry is swept.
fn agent_tool_headers(agent: &AgentRun) -> Vec<String> {
    agent
        .history
        .iter()
        .filter_map(|item| match item {
            HistoryItem::Tool(tool) => Some(tool_header_text(&tool.name, &tool.args)),
            _ => None,
        })
        .collect()
}

/// The nested `Name(args)` one-liner of one tool call an agent ran — the
/// Ctrl+O agent cell's summary row ([`agent_tool_headers`], and the live
/// cell's for a call still on the run); [`file_tool_header`] is its inverse.
/// The `args` are the call's summary as recorded — a file tool's path the
/// model's own — so the entry round-trips the rollout unchanged and the cell
/// shows the path by the session's rule at paint time (`docs/tools.md` *Path
/// display*).
pub(crate) fn tool_header_text(name: &str, args: &str) -> String {
    format!("{name}({args})")
}

/// The `(name, path)` of a recorded [`tool_header_text`] one-liner **when it
/// is a file tool's** (`Read`/`Write`/`Edit`) — what the Ctrl+O agent cell
/// re-shows with the path by the session's rule. `None` for every other
/// tool's header, which the cell renders as recorded: a `Bash(…)` command or
/// an MCP `server - tool (MCP)` name carries parentheses of its own, and only
/// the three file names are known to have none, which is what makes the
/// first `(` theirs.
pub(crate) fn file_tool_header(header: &str) -> Option<(&str, &str)> {
    let open = header.find('(')?;
    let name = &header[..open];
    if !crate::llm::tools::is_file_tool(name) {
        return None;
    }
    let args = header[open + 1..].strip_suffix(')')?;
    Some((name, args))
}

impl App {
    // --- Subagents (`docs/agent-tool.md`) ---

    /// The subagent roster, launch order.
    #[must_use]
    pub fn agents(&self) -> &[AgentRun] {
        &self.agents
    }

    /// The roster entry with this id, if it is still listed.
    #[must_use]
    pub fn agent(&self, id: &str) -> Option<&AgentRun> {
        self.agents.iter().find(|agent| agent.id == id)
    }

    /// Bumped on every roster mutation — the Ctrl+O cache fingerprints it.
    #[must_use]
    pub const fn agents_generation(&self) -> u64 {
        self.agents_generation
    }

    /// The live (announced, unresolved) agent group, if one is.
    #[must_use]
    pub const fn agent_group(&self) -> Option<&AgentGroupLive> {
        self.agent_group.as_ref()
    }

    /// The ↓ roster selection: `Some(0)` = the `● main` row, `Some(i+1)` =
    /// the i-th visible roster entry.
    #[must_use]
    pub const fn agent_selection(&self) -> Option<usize> {
        self.agent_selection
    }

    /// The roster rows the footer list shows — every entry the user's `x`
    /// hasn't *cleared* (a stopped agent lingers red, a finished one green,
    /// until the boundary sweeps it or that second `x` takes it off).
    #[must_use]
    pub fn visible_agents(&self) -> Vec<&AgentRun> {
        self.agents.iter().filter(|agent| !agent.hidden).collect()
    }

    /// A [`StreamEvent::AgentBatch`] announcement: seed one roster entry per
    /// spec and open the live group cell. The loop flushed the streaming
    /// segment first (the ToolBatch dance), so the cell slots after the text.
    pub fn start_agent_group(&mut self, background: bool, specs: &[AgentSpec]) {
        for spec in specs {
            self.agents.push(AgentRun::new(
                spec.id.clone(),
                spec.description.clone(),
                spec.agent_type.clone(),
                spec.prompt.clone(),
                spec.background,
            ));
        }
        self.agent_group = Some(AgentGroupLive {
            ids: specs.iter().map(|spec| spec.id.clone()).collect(),
            background,
        });
        self.agents_generation += 1;
    }

    /// Fold one subagent event (the agent channel) into its roster entry.
    /// Returns `Some(notice)` when the event **settled a background agent**
    /// (its completion owes a notice + the model-facing board note — the
    /// loop posts/defers them); `None` otherwise. A foreground agent's
    /// settling is carried by its group's resolution instead. Unknown ids
    /// are dropped (a chunk racing a sweep).
    pub fn apply_agent_event(&mut self, id: &str, event: &StreamEvent) -> Option<AgentNotice> {
        let stamp = self.now_stamp();
        let agent = self.agents.iter_mut().find(|agent| agent.id == id)?;
        let settled = agent.apply(event);
        self.agents_generation += 1;
        if !settled {
            return None;
        }
        let notice = AgentNotice {
            id: agent.id.clone(),
            description: agent.description.clone(),
            status: agent.status,
            secs: agent.runtime.as_secs(),
            result: agent
                .result
                .clone()
                .or_else(|| agent.error.clone())
                .unwrap_or_default(),
            timestamp: stamp,
        };
        // A background agent's completion is its own event (the group cell
        // resolved long ago); a foreground one resolves with its group.
        agent.background.then_some(notice)
    }

    /// A [`StreamEvent::AgentGroupDone`]: snapshot each call's roster entry
    /// (status, counters, transcript summary) around its model-facing
    /// `output`, record the [`AgentGroup`], and clear the live cell. The
    /// loop commits the returned group's tree cell. Roster entries stay for
    /// their linger (the boundary sweeps them); a background group's entries
    /// keep running.
    pub fn finish_agent_group(
        &mut self,
        background: bool,
        results: &[AgentCallDone],
    ) -> AgentGroup {
        // A group resolving in background mode (a `run_in_background` launch,
        // or Ctrl+B moving a running group over) marks its roster entries
        // background, so each later completion posts its notice
        // (docs/agent-tool.md). Flipping an already-final entry is inert.
        if background {
            for done in results {
                if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == done.id) {
                    agent.background = true;
                }
            }
        } else {
            // A **foreground** resolution IS its members' resolution: any
            // entry not yet settled from its own event stream (a backend
            // whose events raced the drain, or the offline dummy — which
            // scripts no per-agent events at all) settles from the call's
            // outcome, so the tree shows `⎿ Done` and the linger sweep can
            // arm. The stopped note maps to Interrupted, not Failed.
            for done in results {
                if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == done.id)
                    && !agent.status.is_final()
                {
                    let status = if done.ok {
                        AgentStatus::Done
                    } else if done.output == AGENT_STOPPED_OUTPUT {
                        AgentStatus::Interrupted
                    } else {
                        AgentStatus::Failed
                    };
                    // Through the shared settle, not a bare status write: a
                    // member resolved from its group's outcome still has to
                    // give up what it had in flight, or a finished agent
                    // keeps live cells nothing will ever resolve
                    // (`AgentRun::settle_from_group`).
                    let cell = if status == AgentStatus::Interrupted {
                        crate::app::INTERRUPT_TOOL_OUTPUT
                    } else {
                        &done.output
                    };
                    agent.settle_from_group(status, cell);
                    if done.ok && agent.result.is_none() {
                        agent.result = Some(done.output.clone());
                    }
                }
            }
        }
        let entries = results
            .iter()
            .map(|done| {
                let agent = self.agents.iter().find(|agent| agent.id == done.id);
                match agent {
                    Some(agent) => agent_entry_of(agent, &done.output),
                    // The roster entry is gone (swept early) — record what
                    // the event carries so the cell still renders.
                    None => AgentGroupEntry {
                        id: done.id.clone(),
                        description: done.id.clone(),
                        agent_type: crate::agents::GENERAL_PURPOSE.to_string(),
                        prompt: String::new(),
                        status: if done.ok {
                            AgentStatus::Done
                        } else {
                            AgentStatus::Failed
                        },
                        tool_uses: 0,
                        tokens: 0,
                        secs: 0,
                        result: String::new(),
                        tool_headers: Vec::new(),
                        output: done.output.clone(),
                    },
                }
            })
            .collect();
        let group = AgentGroup {
            background,
            agents: entries,
            timestamp: self.now_stamp(),
        };
        self.history.push(HistoryItem::AgentGroup(group.clone()));
        self.agent_group = None;
        self.agents_generation += 1;
        group
    }

    /// Resolve the live agent group **locally** — the Esc interrupt / backend
    /// error path (the channel swap drops the backend's own `AgentGroupDone`):
    /// settle every not-yet-final member as interrupted, record the
    /// [`AgentGroup`] with stopped-note outputs, and clear the live cell.
    /// `None` when no group was live.
    pub(super) fn resolve_live_agent_group(&mut self) -> Option<AgentGroup> {
        let live = self.agent_group.take()?;
        let stamp = self.now_stamp();
        let entries = live
            .ids
            .iter()
            .filter_map(|id| {
                let agent = self.agents.iter_mut().find(|agent| agent.id == *id)?;
                agent.interrupt();
                let output = match agent.status {
                    AgentStatus::Done => agent.result.clone().unwrap_or_default(),
                    _ => AGENT_STOPPED_OUTPUT.to_string(),
                };
                Some(agent_entry_of(agent, &output))
            })
            .collect();
        self.agents_generation += 1;
        let group = AgentGroup {
            background: live.background,
            agents: entries,
            timestamp: stamp,
        };
        self.history.push(HistoryItem::AgentGroup(group.clone()));
        Some(group)
    }

    /// `x` on a roster entry — the stop/clear pair (`docs/agent-tool.md`).
    ///
    /// On a **running** agent it settles the entry as interrupted and leaves
    /// the row in place: the red `◯` lingers for
    /// [`AGENT_STOPPED_LINGER`](crate::agents::AGENT_STOPPED_LINGER) so the
    /// user can see what they stopped ([`AgentStop::Stopped`], carrying the
    /// notice a *background* agent owes — a foreground one resolves with its
    /// group when the registry kill unblocks the backend's wait loop). On an
    /// already-settled row — the user's second `x`, or a naturally finished
    /// agent still lingering — it is the **clear**: the row leaves the roster
    /// at once ([`AgentStop::Cleared`]). `None` when the id isn't listed.
    pub fn stop_agent(&mut self, id: &str) -> Option<AgentStop> {
        let stamp = self.now_stamp();
        // A cleared row is off the roster: there is nothing left for `x` to
        // act on, even while the entry waits for its group's resolution.
        let agent = self
            .agents
            .iter_mut()
            .find(|agent| agent.id == id && !agent.hidden)?;
        if !agent.interrupt() {
            // Already settled — this `x` is the clear.
            agent.hidden = true;
            // A cleared agent can't be the one in view: the session view
            // renders from this entry, and the sweep is about to drop it.
            // (`remove_agent`'s rule; the boundary repaints the main
            // conversation, so the screen goes back with the row.)
            if self.agent_view.as_deref() == Some(id) {
                self.agent_view = None;
            }
            self.forget_agent_selection(id);
            self.clamp_agent_selection();
            self.agents_generation += 1;
            return Some(AgentStop::Cleared);
        }
        agent.stopped_by_user = true;
        let notice = agent.background.then(|| AgentNotice {
            id: agent.id.clone(),
            description: agent.description.clone(),
            status: agent.status,
            secs: agent.runtime.as_secs(),
            result: String::new(),
            timestamp: stamp,
        });
        self.agents_generation += 1;
        Some(AgentStop::Stopped(notice))
    }

    /// Inject one agent's runtime before a draw (the
    /// [`set_status_times`](App::set_status_times) pattern). Frozen once the
    /// agent settles (the tree keeps its final elapsed).
    pub fn set_agent_runtime(&mut self, id: &str, runtime: Duration) {
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id)
            && !agent.status.is_final()
        {
            agent.runtime = runtime;
        }
    }

    /// Open a thinking phase on one agent — the boundary's `ThinkingStart`
    /// handler, called **only when the display is on** (the whole gate; see
    /// [`AgentRun::begin_reasoning`]). See `docs/agent-view-streaming.md`.
    ///
    /// [`AgentRun::begin_reasoning`]: crate::agents::AgentRun::begin_reasoning
    pub fn begin_agent_reasoning(&mut self, id: &str) {
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id) {
            agent.begin_reasoning();
            self.agents_generation += 1;
        }
    }

    /// One agent's open chain-of-thought, or `None` when it isn't thinking —
    /// what its session view's strip previews.
    #[must_use]
    pub fn agent_reasoning(&self, id: &str) -> Option<&str> {
        self.agent(id)?.reasoning()
    }

    /// Close one agent's thinking phase, recording the cell on **its**
    /// transcript and handing it back for the session view to commit
    /// ([`AgentRun::finish_reasoning`] — `None` when nothing was thought).
    ///
    /// [`AgentRun::finish_reasoning`]: crate::agents::AgentRun::finish_reasoning
    pub fn finish_agent_reasoning(&mut self, id: &str, secs: u64) -> Option<Reasoning> {
        let agent = self.agents.iter_mut().find(|agent| agent.id == id)?;
        let settled = agent.finish_reasoning(secs)?;
        self.agents_generation += 1;
        Some(settled)
    }

    /// Inject one agent's **open thinking phase** elapsed before a draw (the
    /// [`set_agent_runtime`](App::set_agent_runtime) sibling), so its session
    /// view's status line shows `Thinking for Ns`.
    pub fn set_agent_thinking(&mut self, id: &str, elapsed: Duration) {
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id) {
            agent.set_thinking(Some(elapsed));
        }
    }

    /// Clear every agent's injected thinking elapsed — run before the live
    /// clocks are re-injected each frame, so a phase that ended (its clock is
    /// gone) drops the `Thinking for Ns` clause instead of freezing it.
    pub fn clear_agent_thinking(&mut self) {
        for agent in &mut self.agents {
            agent.set_thinking(None);
        }
    }

    /// Inject one agent's **running command's** elapsed before a draw (the
    /// [`set_agent_thinking`](App::set_agent_thinking) sibling), so its
    /// session view's `bash` tail counts its `+N lines (Ns)` footer from the
    /// call's own start rather than the agent's whole runtime
    /// (`docs/agent-view-streaming.md`).
    pub fn set_agent_command_elapsed(&mut self, id: &str, elapsed: Duration) {
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id) {
            agent.set_command_elapsed(Some(elapsed));
        }
    }

    /// Clear every agent's injected command elapsed — run before the live
    /// clocks are re-injected each frame (the
    /// [`clear_agent_thinking`](App::clear_agent_thinking) rule), so a
    /// command that resolved (its clock is gone) drops its `(Ns)` instead of
    /// freezing it.
    pub fn clear_agent_command_elapsed(&mut self) {
        for agent in &mut self.agents {
            agent.set_command_elapsed(None);
        }
    }

    /// Sweep one roster entry (its linger expired). Deferred by the boundary
    /// while the user is inside that agent's session view. Also drops the
    /// selection/view if they pointed at it.
    pub fn remove_agent(&mut self, id: &str) {
        let Some(index) = self.agents.iter().position(|agent| agent.id == id) else {
            return;
        };
        self.agents.remove(index);
        self.agents_generation += 1;
        if self.agent_view.as_deref() == Some(id) {
            self.agent_view = None;
        }
        self.forget_agent_selection(id);
        self.clamp_agent_selection();
    }

    /// A background agent completed: update its recorded group entry's
    /// display fields (status, counters, response) so the Ctrl+O cell shows
    /// the final state — the one sanctioned mutation of a committed item,
    /// paid for with a generation bump (full cache re-render). The
    /// model-facing `output` is untouched (the wire history stays faithful).
    pub fn settle_agent_completion(&mut self, notice: &AgentNotice) {
        let run = self.agents.iter().find(|agent| agent.id == notice.id);
        let mut changed = false;
        for item in &mut self.history {
            let HistoryItem::AgentGroup(group) = item else {
                continue;
            };
            for entry in &mut group.agents {
                if entry.id != notice.id {
                    continue;
                }
                entry.status = notice.status;
                entry.secs = notice.secs;
                entry.result = notice.result.clone();
                if let Some(run) = run {
                    entry.tool_uses = run.tool_uses;
                    entry.tokens = run.tokens;
                    entry.tool_headers = agent_tool_headers(run);
                }
                changed = true;
            }
        }
        if changed {
            self.history_generation += 1;
        }
    }

    /// Hold a background-agent completion for the next boundary settle
    /// (beside [`defer_bg_completion`](App::defer_bg_completion)).
    pub fn defer_agent_notice(&mut self, notice: AgentNotice) {
        self.pending_agents.push_back(notice);
    }

    /// Drain the held agent completions — settled at the same safe
    /// boundaries as the shell completions.
    pub fn take_pending_agent_notices(&mut self) -> Vec<AgentNotice> {
        self.pending_agents.drain(..).collect()
    }

    /// Are agent completions awaiting a settle? (The idle arrival dispatches
    /// straight away — the background-shell pattern.)
    #[must_use]
    pub fn has_pending_agent_notices(&self) -> bool {
        !self.pending_agents.is_empty()
    }

    /// Record a settled agent notice in history (stamped) for the loop to
    /// commit.
    pub fn record_agent_notice(&mut self, notice: &AgentNotice) {
        self.history.push(HistoryItem::AgentNotice(notice.clone()));
    }

    /// Is the composer in an agent session view, and for which agent?
    #[must_use]
    pub fn viewed_agent(&self) -> Option<&AgentRun> {
        self.agent(self.agent_view.as_deref()?)
    }

    /// Inject the viewed subagent's **context window** (from the boundary's
    /// model bookkeeping, beside every
    /// [`set_agent_system_prompt`](App::set_agent_system_prompt)) — the
    /// session's own for a type that inherits the model, `None` (or a
    /// meaningless 0) for one pinned to a model whose window this session
    /// cannot know, which hides the gauge in that view. See
    /// `docs/agent-context-gauge.md`.
    pub fn set_agent_context_window(&mut self, window: Option<u64>) {
        self.agent_context_window = window.filter(|&w| w > 0);
    }

    /// Inject the model the viewed subagent's type is **pinned** to (from
    /// `ReplySource::agent_model`, beside the window), `None` when it
    /// inherits the session's. See `docs/agent-context-gauge.md`.
    pub fn set_agent_model(&mut self, model: Option<String>) {
        self.agent_model = model.filter(|model| !model.trim().is_empty());
    }

    /// The pinned model of the agent whose session view is open — what its
    /// footer names in place of the session's model — or `None` when no view
    /// is open or the viewed type inherits the session's model.
    #[must_use]
    pub fn viewed_agent_model(&self) -> Option<&str> {
        self.viewed_agent()?;
        self.agent_model.as_deref()
    }

    /// The footer's context gauge — `(used, window)` for the conversation
    /// **on screen**, or `None` when its window is unknown (no gauge): inside
    /// an agent session view the viewed subagent's own context
    /// ([`AgentRun::context_used`]) against its model's window, otherwise the
    /// main session's (`docs/agent-context-gauge.md`). The main pair alone —
    /// [`context_used`](App::context_used) /
    /// [`context_window`](App::context_window) — keeps driving auto-compact,
    /// a fact about the lead's conversation whatever is on screen.
    ///
    /// [`AgentRun::context_used`]: crate::agents::AgentRun::context_used
    #[must_use]
    pub fn context_gauge(&self) -> Option<(u64, u64)> {
        match self.viewed_agent() {
            Some(run) => Some((run.context_used(), self.agent_context_window?)),
            None => Some((self.context_used(), self.context_window()?)),
        }
    }

    /// Enter an agent's session view (the roster selection's Enter) — and the
    /// pick the next ↓ comes back to (`agent_selection_start`).
    pub fn open_agent_view(&mut self, id: &str) {
        self.agent_view = Some(id.to_string());
        self.agent_selection_memory = Some(id.to_string());
        self.shortcuts_open = false;
        self.backtrack = Backtrack::default();
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
    }

    /// Leave the agent session view back to the main conversation.
    pub fn close_agent_view(&mut self) {
        self.agent_view = None;
    }

    /// A message typed into a **running** agent's session: park it on that
    /// agent's queue (`docs/queue.md`). It shows above the box until the
    /// agent's loop takes it at its next round boundary, when
    /// [`StreamEvent::Steered`] turns it into a real user message on that
    /// transcript — the main session's steering, one level down.
    ///
    /// Which of this and [`agent_chat`](App::agent_chat) runs is the
    /// **registry's** call, not the roster's: only the registry knows whether
    /// the loop is still running, and a roster status that lagged it by one
    /// event would either strand the row forever or record the message twice.
    pub fn queue_agent_chat(&mut self, id: &str, text: &str) {
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id) {
            agent.queue_chat(text);
            self.agents_generation += 1;
        }
    }

    /// **Tab** in an agent session view: park the text as a **follow-up turn**
    /// for the agent on screen — its own queue, never the main session's
    /// ([`queued`](App::queued), which is what Tab used to reach here: a
    /// message typed into a subagent ran as a follow-up turn of the *lead*
    /// conversation once the lead's turn ended). It waits below the steered
    /// rows until that agent's loop settles, when the boundary hands it over
    /// as a chat continuation. See `docs/queue.md`.
    ///
    /// A no-op when no agent session view is open.
    pub fn queue_agent_followup(&mut self, text: &str) {
        let Some(id) = self.agent_view.clone() else {
            return;
        };
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id) {
            agent.queue_followup(text);
            self.agents_generation += 1;
        }
    }

    /// Tab's whole path in an agent session view: consume the composer,
    /// record the text for ↑ recall (a queued message recalls like a
    /// submitted one, exactly as [`queue_draft`](App::queue_draft) does), and
    /// park it as that agent's next follow-up turn.
    pub(super) fn queue_agent_draft(&mut self) {
        let text = self.take_input();
        self.file_search = None; // the composer is consumed into the queue
        self.skill_picker = None;
        self.input_history.record(&text);
        self.queue_agent_followup(&text);
    }

    /// Take `id`'s next follow-up turn — the boundary drains one per settle,
    /// [`drain_next_batch`](App::drain_next_batch)'s twin one level down.
    /// Takes an id rather than the viewed agent: an agent settles whether or
    /// not its session is the screen on show.
    pub fn take_agent_followup(&mut self, id: &str) -> Option<String> {
        let agent = self.agents.iter_mut().find(|agent| agent.id == id)?;
        let text = agent.take_followup()?;
        self.agents_generation += 1;
        Some(text)
    }

    /// The ids of every agent with a follow-up turn waiting — what the
    /// boundary sweeps at each settle. Empty in the ordinary case, so the
    /// per-tick sweep costs one `is_empty` per roster row.
    #[must_use]
    pub fn agents_awaiting_followup(&self) -> Vec<String> {
        self.agents
            .iter()
            .filter(|agent| !agent.followups.is_empty())
            .map(|agent| agent.id.clone())
            .collect()
    }

    /// Alt+Up in an agent session view: pull that agent's **last** follow-up
    /// back into the composer to edit, extend or drop — the earlier ones stay
    /// queued ([`recall_last_queued`](App::recall_last_queued)'s twin).
    /// Returns whether anything came back.
    pub(super) fn recall_last_agent_followup(&mut self) -> bool {
        let Some(id) = self.agent_view.clone() else {
            return false;
        };
        let Some(text) = self
            .agents
            .iter_mut()
            .find(|agent| agent.id == id)
            .and_then(AgentRun::take_last_followup)
        else {
            return false;
        };
        self.agents_generation += 1;
        self.recall_input(&text);
        true
    }

    /// Alt+Up's whole path in an agent session view: the deliberate backlog
    /// first — that agent's last **follow-up turn** — and only with none left
    /// the message its loop has not read yet, which the boundary must ask the
    /// registry for ([`Action::ReclaimAgentChat`]). The main session's two
    /// Alt+Up arms, one level down; it never falls through to the main
    /// session's own backlog, which belongs to a conversation the user is not
    /// looking at.
    pub(super) fn recall_agent_pending(&mut self) -> Action {
        if self.recall_last_agent_followup() {
            return Action::None;
        }
        match self.viewed_agent() {
            Some(agent) if !agent.queued.is_empty() => Action::ReclaimAgentChat {
                id: agent.id.clone(),
            },
            _ => Action::None,
        }
    }

    /// The boundary's answer to [`Action::ReclaimAgentChat`]: the agent's loop
    /// had not read `text`, so drop its pending row and put it back in the
    /// composer to edit, extend or drop ([`recall_steered`](App::recall_steered)'s
    /// twin).
    pub fn recall_agent_chat(&mut self, id: &str, text: &str) {
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id)
            && let Some(index) = agent.queued.iter().rposition(|pending| pending == text)
        {
            agent.queued.remove(index);
            self.agents_generation += 1;
        }
        self.recall_input(text);
    }

    /// Is the agent on screen still running — i.e. can Tab queue a follow-up
    /// turn against it? The main session's `is_streaming()` one level down,
    /// and the same rule follows from it: an idle Tab is a no-op that keeps
    /// the draft (`docs/queue.md`).
    #[must_use]
    pub fn viewed_agent_running(&self) -> bool {
        self.viewed_agent()
            .is_some_and(|agent| !agent.status.is_final())
    }

    /// A chat message that started a **continuation run** on a settled agent:
    /// record it into the agent's transcript at once (the run carries it as
    /// its newest user turn, so no round boundary will announce it) and
    /// reopen the entry so the continuation's events fold in. The idle-submit
    /// half of [`queue_agent_chat`](App::queue_agent_chat).
    pub fn agent_chat(&mut self, id: &str, text: &str) {
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id) {
            agent.push_user_message(text);
            if agent.status.is_final() {
                agent.reopen();
            }
            self.agents_generation += 1;
        }
    }

    /// Should ↓ open the roster selection? Mirrors
    /// [`background_focusable`](App::background_focusable): an idle-looking
    /// composer and at least one visible roster row to land on.
    pub(super) fn agent_selectable(&self) -> bool {
        !self.visible_agents().is_empty()
            && self.input.is_empty()
            && !self.shell_mode
            && self.command_menu.is_none()
            && self.file_search.is_none()
    }

    /// Where ↓ opens the roster selection: the **last picked** agent's row
    /// when it is still listed, else `0` (the `● main` row). Stepping between
    /// several agents is what this buys — the second visit resumes where the
    /// first left off instead of restarting at `main` (`docs/agent-tool.md`).
    /// Entering an agent's session view is a pick, so Enter-then-↓ comes back
    /// to that agent; walking the `❯` onto `● main` is the way to forget one.
    pub(super) fn agent_selection_start(&self) -> usize {
        let Some(id) = self.agent_selection_memory.as_deref() else {
            return 0;
        };
        self.visible_agents()
            .iter()
            .position(|agent| agent.id == id)
            .map_or(0, |index| index + 1)
    }

    /// Remember the row the selection just landed on (the `● main` row
    /// forgets — ↓ then opens on `main` again, as it always did).
    fn remember_agent_selection(&mut self, selected: usize) {
        self.agent_selection_memory = selected
            .checked_sub(1)
            .and_then(|index| self.visible_agents().get(index).map(|a| a.id.clone()));
    }

    /// Drop the memory when it points at `id` (that agent is leaving the
    /// roster — ↓ falls back to the `● main` row).
    fn forget_agent_selection(&mut self, id: &str) {
        if self.agent_selection_memory.as_deref() == Some(id) {
            self.agent_selection_memory = None;
        }
    }

    /// Keep the selection on a real row as the roster shrinks.
    fn clamp_agent_selection(&mut self) {
        let rows = self.visible_agents().len();
        if rows == 0 {
            self.agent_selection = None;
            return;
        }
        if let Some(selected) = self.agent_selection {
            self.agent_selection = Some(selected.min(rows));
        }
    }

    /// Keys while the ↓ roster selection is active — the
    /// [`on_key_background_focus`](App::on_key_background_focus) contract:
    /// ↑/↓ move (↑ from the `main` row steps back onto the footer's shell
    /// indicator when a shell is running — the reverse of ↓'s indicator →
    /// roster walk — else exits to the composer), Enter views (`main`
    /// leaves an open agent view / closes), `x` stops the selected agent,
    /// Esc/Ctrl+C dismiss, and every other key clears the selection and
    /// falls through.
    pub(super) fn on_key_agent_selection(&mut self, key: KeyEvent) -> Option<Action> {
        let selected = self.agent_selection?;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.agent_selection = None;
            return Some(Action::None);
        }
        let rows = self.visible_agents().len();
        match key.code {
            KeyCode::Down => {
                let next = (selected + 1).min(rows);
                self.agent_selection = Some(next);
                self.remember_agent_selection(next);
                Some(Action::None)
            }
            KeyCode::Up => {
                if selected == 0 {
                    // Step back the way ↓ came: from `● main` onto the
                    // footer's shell indicator when a shell is running (a
                    // second ↑ there dismisses it to the composer), else
                    // straight back to the composer.
                    self.agent_selection = None;
                    if self.background_focusable() {
                        self.background_focus = true;
                    }
                } else {
                    self.agent_selection = Some(selected - 1);
                    self.remember_agent_selection(selected - 1);
                }
                Some(Action::None)
            }
            KeyCode::Enter
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                if selected == 0 {
                    // `● main`: return from an open agent view; a no-op (just
                    // closing the selection) when already in the main session.
                    self.agent_selection = None;
                    if self.agent_view.take().is_some() {
                        return Some(Action::LeaveAgentView);
                    }
                    return Some(Action::None);
                }
                let agent = self
                    .visible_agents()
                    .get(selected - 1)
                    .map(|a| a.id.clone());
                if let Some(id) = agent {
                    // The selection hands over to the view: the composer owns
                    // the keys again (typing chats) and the `❯` leaves with
                    // the selection — the roster marks the viewed agent by
                    // its filled bullet instead (`ui::agent_list_lines`).
                    self.agent_selection = None;
                    if self.agent_view.as_deref() == Some(id.as_str()) {
                        // Already viewing it — nothing to switch.
                        return Some(Action::None);
                    }
                    self.open_agent_view(&id);
                    return Some(Action::ViewAgent(id));
                }
                Some(Action::None)
            }
            KeyCode::Char('x') if selected > 0 => {
                let id = self
                    .visible_agents()
                    .get(selected - 1)
                    .map(|agent| agent.id.clone());
                id.map_or(Some(Action::None), |id| Some(Action::StopAgent(id)))
            }
            KeyCode::Esc => {
                self.agent_selection = None;
                Some(Action::None)
            }
            _ => {
                self.agent_selection = None;
                None
            }
        }
    }
}
