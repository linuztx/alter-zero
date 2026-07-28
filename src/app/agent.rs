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

    /// The rendered one-liner (the user's reference wording).
    #[must_use]
    pub fn headline(&self) -> String {
        match self.status {
            AgentStatus::Interrupted => {
                format!("Agent \"{}\" was stopped by user", self.description)
            }
            AgentStatus::Failed => format!("Agent \"{}\" failed", self.description),
            _ => format!("Agent \"{}\" finished · {}s", self.description, self.secs),
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
            _ => format!("completed in {}s", self.secs),
        };
        let body = if self.result.trim().is_empty() {
            "(no output)"
        } else {
            self.result.trim_end_matches('\n')
        };
        format!(
            "[background agent] Agent \"{}\" (id {}) {outcome}.\nFinal response:\n{body}",
            self.description, self.id,
        )
    }
}

/// The **live** agent group — the round's `agent` calls between their
/// [`StreamEvent::AgentBatch`] announcement and their resolution: the strip
/// previews it as the blue `● Running {n} agents…` tree cell (rendered from
/// the roster entries these ids name). Cleared by
/// [`App::finish_agent_group`]; an interrupt / backend error resolves it
/// locally ([`App::resolve_live_agent_group`]) because the channel swap drops
/// the backend's own `AgentGroupDone`. See `docs/agent-tool.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentGroupLive {
    pub ids: Vec<String>,
    pub background: bool,
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
            HistoryItem::Tool(tool) => Some(format!("{}({})", tool.name, tool.args)),
            _ => None,
        })
        .collect()
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

    /// The roster rows the footer list shows — every entry a user `x` hasn't
    /// hidden (a stopped entry leaves the list at once; a naturally finished
    /// one lingers coloured until the boundary sweeps it).
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
                    agent.status = if done.ok {
                        AgentStatus::Done
                    } else if done.output == AGENT_STOPPED_OUTPUT {
                        AgentStatus::Interrupted
                    } else {
                        AgentStatus::Failed
                    };
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

    /// `x` on a roster entry: settle it as interrupted and **hide** it (the
    /// user's `x` removes the row right away — no linger). For a background
    /// agent (its group already committed) this also returns its stopped
    /// notice for the boundary to settle; a foreground one resolves with its
    /// group (the registry kill unblocks the backend's wait loop). `None`
    /// when the id isn't listed or was already final.
    pub fn stop_agent(&mut self, id: &str) -> Option<Option<AgentNotice>> {
        let stamp = self.now_stamp();
        let agent = self.agents.iter_mut().find(|agent| agent.id == id)?;
        if !agent.interrupt() {
            // Already settled — `x` on a lingering row just dismisses it.
            agent.hidden = true;
            self.clamp_agent_selection();
            self.agents_generation += 1;
            return None;
        }
        agent.hidden = true;
        let notice = agent.background.then(|| AgentNotice {
            id: agent.id.clone(),
            description: agent.description.clone(),
            status: agent.status,
            secs: agent.runtime.as_secs(),
            result: String::new(),
            timestamp: stamp,
        });
        self.agents_generation += 1;
        self.clamp_agent_selection();
        Some(notice)
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

    /// Enter an agent's session view (the roster selection's Enter).
    pub fn open_agent_view(&mut self, id: &str) {
        self.agent_view = Some(id.to_string());
        self.shortcuts_open = false;
        self.backtrack = Backtrack::default();
        self.command_menu = None;
        self.file_search = None;
    }

    /// Leave the agent session view back to the main conversation.
    pub fn close_agent_view(&mut self) {
        self.agent_view = None;
    }

    /// A chat message submitted inside an agent session view: record it into
    /// the agent's transcript (the registry delivers the same text to its
    /// loop) and reopen a settled entry so the continuation's events fold in.
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
    /// ↑/↓ move (↑ from the `main` row exits), Enter views (`main` leaves an
    /// open agent view / closes), `x` stops the selected agent, Esc/Ctrl+C
    /// dismiss, and every other key clears the selection and falls through.
    pub(super) fn on_key_agent_selection(&mut self, key: KeyEvent) -> Option<Action> {
        let selected = self.agent_selection?;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.agent_selection = None;
            return Some(Action::None);
        }
        let rows = self.visible_agents().len();
        match key.code {
            KeyCode::Down => {
                self.agent_selection = Some((selected + 1).min(rows));
                Some(Action::None)
            }
            KeyCode::Up => {
                if selected == 0 {
                    self.agent_selection = None;
                } else {
                    self.agent_selection = Some(selected - 1);
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
                    // the keys again (typing chats), and the roster's `❯`
                    // stays on the viewed agent implicitly
                    // (`ui::agent_list_lines`).
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
