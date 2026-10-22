//! The `/context` page at the boundary: keeping it live
//! (`docs/context-usage.md`).
//!
//! The pure side counts the window and owns the page. What the boundary
//! adds is the one input `App` does not hold on its own — the tool set the
//! backend will send, which an MCP server connecting or a skills rescan can
//! change while the page is open — and the refresh that re-counts whenever
//! anything the page was counted from has moved.

use super::Session;

impl Session<'_> {
    /// Keep an open `/context` page live: hand `App` the tool set of the
    /// conversation on screen — the backend's own, or the viewed subagent
    /// type's — and let it re-count if anything moved. Run at every loop
    /// bottom (and on the open, so the first frame is already right): a
    /// pointer copy and a few hashes when nothing changed, nothing at all
    /// while the page is closed.
    pub(crate) fn sync_context_usage(&mut self) {
        if self.app.context_usage.is_none() {
            return;
        }
        match self
            .app
            .viewed_agent()
            .map(|agent| agent.agent_type.clone())
        {
            Some(agent_type) => {
                let tools = self.models.backend().agent_tool_definitions(&agent_type);
                self.app.set_agent_tool_definitions(tools);
            }
            None => {
                let tools = self.models.backend().tool_definitions();
                self.app.set_tool_definitions(tools);
            }
        }
        self.app.refresh_context_usage();
    }
}
