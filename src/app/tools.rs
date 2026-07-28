//! Tool-call bookkeeping: the parallel batch queue, the running call's live
//! output, and how a call resolves.
//! See `docs/tools.md` and `docs/parallel-tools.md`.

use super::*;

/// The output recorded on a tool that was still running when the user
/// interrupted: it resolves as [`ToolStatus::Failed`] with this explanation
/// (codex: an aborted tool "may have partially executed").
pub const INTERRUPT_TOOL_OUTPUT: &str = "Interrupted by user";

/// The output recorded on a tool that was still running when the backend
/// reported a [`crate::stream::StreamEvent::Error`] — the turn died before the
/// tool's `ToolEnd` could arrive, so it resolves as [`ToolStatus::Failed`]
/// with this explanation ([`INTERRUPT_TOOL_OUTPUT`]'s error-path twin).
pub const ERROR_TOOL_OUTPUT: &str = "Interrupted by a backend error";

impl App {
    /// Announce a **parallel batch** of tool calls the model requested this
    /// round (each [`ToolCallSummary`]'s `name`/`args` for the `● name(args)`
    /// header), all queued as [`ToolStatus::Waiting`] so the live region shows
    /// every call at once — the ones not yet running as `⎿ Waiting…`. Sequential
    /// execution then flips them to `Running` one at a time via
    /// [`start_tool`](App::start_tool). Called by the boundary on a
    /// [`crate::stream::StreamEvent::ToolBatch`]; a backend that never batches
    /// (the `!` shell, the dummy's lone calls) skips it and a lone
    /// [`start_tool`](App::start_tool) still works. See `docs/parallel-tools.md`.
    pub fn start_tool_batch(&mut self, items: &[ToolCallSummary]) {
        self.tool_queue = items
            .iter()
            .map(|item| ToolCall {
                name: item.name.clone(),
                args: item.args.clone(),
                status: ToolStatus::Waiting,
                output: String::new(),
                timestamp: String::new(), // stamped when it finishes (see end_tool)
                shell: false,
                truncated: false,
            })
            .collect();
    }

    /// Begin a tool call: mark it the running (blue) call at the front of the
    /// live queue so the bottom region shows it before any output arrives.
    ///
    /// If the front call is a `Waiting` batch sibling (`start_tool_batch`
    /// announced it), it is flipped to `Running` — the batch's `(name, args)` are
    /// authoritative and equal the ones passed here (both come from the same
    /// backend summary), so only the status changes. Otherwise (an empty queue —
    /// the `!` shell, the dummy's lone calls) a fresh `Running` call is pushed, so
    /// the single-tool path is unchanged.
    pub fn start_tool(&mut self, name: &str, args: &str) {
        if let Some(front) = self.tool_queue.front_mut()
            && front.status == ToolStatus::Waiting
        {
            front.status = ToolStatus::Running;
            return;
        }
        self.tool_queue.push_back(ToolCall {
            name: name.to_string(),
            args: args.to_string(),
            status: ToolStatus::Running,
            output: String::new(),
            timestamp: String::new(), // stamped when it finishes (see end_tool)
            shell: false,
            truncated: false,
        });
    }

    /// Mark the running tool's output as **truncated** (set by the boundary just
    /// before [`end_tool`] when a `!` command's output exceeded the in-memory
    /// cap): the cell will append a dim `…` marker at the end of the expanded
    /// output. No-op when no tool is running. See `docs/shell-command.md`.
    ///
    /// [`end_tool`]: App::end_tool
    pub fn set_tool_truncated(&mut self) {
        if let Some(tool) = self.tool_queue.front_mut() {
            tool.truncated = true;
        }
    }

    /// Append a chunk of live output to the **running** tool at the front of the
    /// queue so its cell tails the output as it streams (the boundary's handler
    /// for [`crate::stream::StreamEvent::ToolOutput`]; see
    /// `docs/tool-streaming.md`). A no-op unless the front call is
    /// [`ToolStatus::Running`] — a `Waiting` batch sibling has not started and an
    /// empty queue has nothing to tail. Does **not** count tokens: the tally is
    /// charged once from the authoritative `ToolEnd` output in
    /// [`end_tool`](App::end_tool), which overwrites this partial, so the live
    /// tail and the final cell never double-count.
    pub fn push_tool_output(&mut self, chunk: &str) {
        if let Some(tool) = self.tool_queue.front_mut()
            && tool.status == ToolStatus::Running
        {
            tool.output.push_str(chunk);
        }
    }

    /// The tool currently at the front of the live queue — the running (or, in
    /// the brief gap between a batch's calls, about-to-run) call, if any.
    #[must_use]
    pub fn current_tool(&self) -> Option<&ToolCall> {
        self.tool_queue.front()
    }

    /// The whole live tool queue, front-first: the running/next call followed by
    /// any [`ToolStatus::Waiting`] batch siblings. The renderer walks this to show
    /// every call in a parallel batch (the running one live, the rest as
    /// `⎿ Waiting…`). Empty when no tool is in flight. See `docs/parallel-tools.md`.
    #[must_use]
    pub fn tool_queue(&self) -> &VecDeque<ToolCall> {
        &self.tool_queue
    }

    /// Finish the in-flight tool call with its final `output` and outcome
    /// (`ok` → [`ToolStatus::Ok`], else [`ToolStatus::Failed`]), record it in the
    /// history, and remove it from the front of the live queue — the next batch
    /// sibling (if any) becomes the front. Returns the finished call (for the
    /// event loop to commit to scrollback), or `None` if no tool was running.
    pub fn end_tool(&mut self, output: &str, ok: bool) -> Option<ToolCall> {
        let status = if ok {
            ToolStatus::Ok
        } else {
            ToolStatus::Failed
        };
        self.resolve_front_tool(output, status)
    }

    /// Resolve the in-flight tool call as **moved to the background** (a
    /// `run_in_background` bash call, or Ctrl+B on a running command):
    /// [`ToolStatus::Backgrounded`], with `output` holding the model-facing
    /// launch text (task id + interim-output path) that the cell never shows —
    /// it renders the fixed `⎿ Running in the background (↓ to manage)` row.
    /// The boundary's handler for `StreamEvent::ToolBackgrounded`. See
    /// `docs/background.md`.
    pub fn background_tool(&mut self, output: &str) -> Option<ToolCall> {
        self.resolve_front_tool(output, ToolStatus::Backgrounded)
    }

    /// The shared tail of [`end_tool`]/[`background_tool`]: pop the front
    /// call, stamp + record it with `status`, and fold its output into the
    /// token tally (arrow up — uploaded back; the count is *added to*, never
    /// reset — see `docs/status-indicator.md`).
    ///
    /// [`end_tool`]: App::end_tool
    /// [`background_tool`]: App::background_tool
    fn resolve_front_tool(&mut self, output: &str, status: ToolStatus) -> Option<ToolCall> {
        let mut tool = self.tool_queue.pop_front()?;
        tool.output = output.to_string();
        tool.status = status;
        tool.timestamp = self.now_stamp();
        if let Some(turn) = self.status.as_mut() {
            turn.tokens += count_tokens(output);
            turn.arrow = TokenArrow::Up;
        }
        self.history.push(HistoryItem::Tool(tool.clone()));
        Some(tool)
    }
}
