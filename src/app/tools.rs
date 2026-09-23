//! Tool-call bookkeeping: the parallel batch queue, the running call's live
//! output, and how a call resolves.
//! See `docs/tools.md` and `docs/parallel-tools.md`.

use super::*;
use crate::stream::RoundCall;
use std::path::PathBuf;

/// The output recorded on a tool that was still running when the user
/// interrupted: it resolves as [`ToolStatus::Failed`] with this explanation
/// (codex: an aborted tool "may have partially executed").
pub const INTERRUPT_TOOL_OUTPUT: &str = "Interrupted by user";

/// The output recorded on a tool that was still running when the backend
/// reported a [`crate::stream::StreamEvent::Error`] — the turn died before the
/// tool's `ToolEnd` could arrive, so it resolves as [`ToolStatus::Failed`]
/// with this explanation ([`INTERRUPT_TOOL_OUTPUT`]'s error-path twin).
pub const ERROR_TOOL_OUTPUT: &str = "Interrupted by a backend error";

/// A call's wire identity as a batch announcement stamps it — the provider's
/// id, the call's position in its round and the model's verbatim arguments —
/// all `None` for a batch no round announced.
type Identity = (Option<String>, Option<usize>, Option<String>);

/// A tool round as the backend announced it ([`StreamEvent::RoundCalls`]):
/// the batch id its records share, and its calls in the model's order with a
/// cursor per kind — the visible calls pair with the batch announcement in
/// order, the task calls with their results in order, the agent launches by
/// the ids their specs carry — so every record the round resolves into is
/// stamped with the provider's id for *that* call (`docs/prompt-caching.md`).
#[derive(Debug)]
pub(crate) struct OpenRound {
    pub(super) batch: u64,
    calls: Vec<RoundCall>,
    batch_announced: bool,
    tasks_taken: usize,
}

impl OpenRound {
    /// The next task call of the round, in the model's order — task calls
    /// resolve through their own events, one per call, in that order
    /// (`docs/task-tools.md`) — with its position in the round.
    pub(super) fn take_task_call(&mut self) -> Option<(usize, &RoundCall)> {
        let index = self.tasks_taken;
        self.tasks_taken += 1;
        self.calls
            .iter()
            .enumerate()
            .filter(|(_, call)| crate::tasks::is_task_tool(&call.name))
            .nth(index)
    }

    /// The round's calls that get a cell — everything but a task call (no
    /// cell anywhere, `docs/task-tools.md`) and an agent launch (its own
    /// group cell, `docs/agent-tool.md`) — in the model's order, which is
    /// the order the batch announcement lists them in, each with its
    /// position in the round.
    fn visible(&self) -> impl Iterator<Item = (usize, &RoundCall)> {
        self.calls.iter().enumerate().filter(|(_, call)| {
            !crate::tasks::is_task_tool(&call.name)
                && call.name != crate::llm::tools::AGENT_TOOL_NAME
        })
    }

    /// Where the call `id` sits in the round — how a subagent launch, which
    /// carries its call's id on its spec, learns its position.
    pub(super) fn position_of(&self, id: &str) -> Option<usize> {
        self.calls.iter().position(|call| call.id == id)
    }
}

impl App {
    /// The backend announced a tool round ([`StreamEvent::RoundCalls`]): open
    /// it under a fresh batch id so the records its cells resolve into share
    /// one round and carry the provider's own call ids.
    pub fn open_round(&mut self, calls: Vec<RoundCall>) {
        self.next_batch += 1;
        self.round = Some(OpenRound {
            batch: self.next_batch,
            calls,
            batch_announced: false,
            tasks_taken: 0,
        });
    }

    /// The batch id of the round announced last, while one is open.
    #[must_use]
    pub fn open_round_batch(&self) -> Option<u64> {
        self.round.as_ref().map(|round| round.batch)
    }
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
        // One id per announced batch: what tells the renderer a run of MCP
        // cells was **parallel** (one `Called deepwiki 2 times` line) from two
        // sequential single calls that merely landed next to each other in
        // history (`docs/mcp.md`). The round announced ahead of this batch
        // (docs/prompt-caching.md) names it and, in the model's order, the
        // provider's id of each visible call — the announcement lists exactly
        // those, in that order, so the k-th cell is the k-th visible call. A
        // count that disagrees stamps no ids rather than guessing; a batch no
        // round announced (the offline dummy, a scripted test) numbers itself.
        let (batch, ids): (u64, Vec<Identity>) = match self.round.as_mut() {
            Some(round) if !round.batch_announced => {
                round.batch_announced = true;
                let visible: Vec<(usize, &RoundCall)> = round.visible().collect();
                let ids = if visible.len() == items.len() {
                    visible
                        .iter()
                        .map(|(position, call)| {
                            (
                                Some(call.id.clone()),
                                Some(*position),
                                Some(call.arguments.clone()),
                            )
                        })
                        .collect()
                } else {
                    vec![(None, None, None); items.len()]
                };
                (round.batch, ids)
            }
            _ => {
                self.next_batch += 1;
                (self.next_batch, vec![(None, None, None); items.len()])
            }
        };
        self.tool_queue = items
            .iter()
            .zip(ids)
            .map(|(item, (call_id, position, arguments))| ToolCall {
                name: item.name.clone(),
                args: item.args.clone(),
                status: ToolStatus::Waiting,
                output: String::new(),
                timestamp: String::new(), // stamped when it finishes (see end_tool)
                shell: false,
                truncated: false,
                context_output: None,
                // The model's own arguments, from the round: what a call that
                // never reaches its `ToolStart` is recorded with — a waiting
                // sibling an Esc resolves still replays exactly as the model
                // made it (docs/interrupt.md). The call's own `ToolStart`
                // replaces them (`start_tool`): a `PreToolUse` hook may have
                // rewritten what runs.
                arguments,
                approval_note: None,
                batch: Some(batch),
                call_id,
                position,
            })
            .collect();
    }

    /// Begin a tool call: mark it the running (blue) call at the front of the
    /// live queue so the bottom region shows it before any output arrives.
    ///
    /// If the front call is a `Waiting` batch sibling (`start_tool_batch`
    /// announced it), it is flipped to `Running` — the batch's `(name, args)` are
    /// authoritative and equal the ones passed here (both come from the same
    /// backend summary), so only the status changes, plus the `arguments`: the
    /// ones this call actually runs with, replacing whatever its round
    /// announced (a `PreToolUse` hook may have rewritten them,
    /// `docs/hooks.md`). Otherwise (an empty queue —
    /// the `!` shell, the dummy's lone calls) a fresh `Running` call is pushed, so
    /// the single-tool path is unchanged.
    ///
    /// `arguments` is the model's verbatim JSON, `None` when the caller has
    /// none ([`ToolCall::arguments`]). It is a **parameter, not a follow-up
    /// setter**, unlike [`set_tool_note`](App::set_tool_note): a note is
    /// optional and conditional, while every backend call has arguments, and
    /// a second call that a future path could forget would drop them
    /// silently — the derived context would quietly fall back to the lossy
    /// summary with nothing to notice.
    pub fn start_tool(&mut self, name: &str, args: &str, arguments: Option<&str>) {
        let arguments = arguments.map(str::to_string);
        if let Some(front) = self.tool_queue.front_mut()
            && front.status == ToolStatus::Waiting
        {
            front.status = ToolStatus::Running;
            front.arguments = arguments;
            return;
        }
        self.tool_queue.push_back(ToolCall {
            name: name.to_string(),
            args: args.to_string(),
            arguments,
            status: ToolStatus::Running,
            output: String::new(),
            timestamp: String::new(), // stamped when it finishes (see end_tool)
            shell: false,
            truncated: false,
            context_output: None,
            approval_note: None,
            // A lone call is nobody's batch sibling.
            batch: None,
            call_id: None,
            position: None,
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

    /// Record how the **running** tool came to run without the user — the
    /// auto mode classifier's `Allowed by auto mode classifier` — so the
    /// resolved cell appends it as a dim `⎿` row (the boundary's handler for
    /// [`crate::stream::StreamEvent::ToolNote`]; see `docs/permissions.md`).
    /// A no-op unless the front call is [`ToolStatus::Running`] — the note
    /// always follows its call's `ToolStart`.
    pub fn set_tool_note(&mut self, note: &str) {
        if let Some(tool) = self.tool_queue.front_mut()
            && tool.status == ToolStatus::Running
        {
            tool.approval_note = Some(note.to_string());
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
        self.resolve_front_tool(output, None, status)
    }

    /// Resolve **every** call still in the live queue as
    /// [`ToolStatus::Failed`] with `output` — the running (or asked-about)
    /// front call and each `⎿ Waiting…` sibling behind it, in the batch's
    /// order — recording each one exactly as [`end_tool`](App::end_tool)
    /// would, and return them for the boundary to commit. What a turn's death
    /// owes its batch: an Esc ([`INTERRUPT_TOOL_OUTPUT`]) or a backend error
    /// ([`ERROR_TOOL_OUTPUT`]) ends every call the model asked for at once,
    /// and a call that never started is still a call the model made — its
    /// cell stays on screen and its record keeps it in the next request's
    /// context (`docs/interrupt.md`). Empty when nothing was in flight.
    pub(super) fn fail_live_queue(&mut self, output: &str) -> Vec<ToolCall> {
        let mut resolved = Vec::with_capacity(self.tool_queue.len());
        while let Some(tool) = self.end_tool(output, false) {
            resolved.push(tool);
        }
        resolved
    }

    /// Resolve the in-flight call as **refused at the permission prompt**: red
    /// like any failure, with `display` on the cell and `result` — the longer
    /// stop-and-wait instruction the *model* reads, carrying Tab's amend
    /// feedback — kept beside it as [`ToolCall::context_output`] so the derived
    /// context replays what was really sent. The boundary's handler for
    /// [`crate::stream::StreamEvent::ToolRejected`]. See `docs/permissions.md`.
    pub fn reject_tool(&mut self, display: &str, result: &str) -> Option<ToolCall> {
        self.resolve_front_tool(display, Some(result.to_string()), ToolStatus::Failed)
    }

    /// Resolve the in-flight call as **answered by the user** — the
    /// `AskUserQuestion` submission (`docs/ask.md`): green, with the
    /// `User answered Alter Zero's questions:` cell text on `display` and the
    /// model-facing answers JSON kept beside it as
    /// [`ToolCall::context_output`], [`reject_tool`](Self::reject_tool)'s
    /// green twin. The boundary's handler for
    /// [`crate::stream::StreamEvent::ToolAnswered`].
    pub fn answer_tool(&mut self, display: &str, result: &str) -> Option<ToolCall> {
        self.resolve_front_tool(display, Some(result.to_string()), ToolStatus::Ok)
    }

    /// Resolve the in-flight tool call as **moved to the background** (a
    /// `run_in_background` bash call, or Ctrl+B on a running command):
    /// [`ToolStatus::Backgrounded`], with `output` holding the model-facing
    /// launch text (interim-output path + completion promise) that the cell never shows —
    /// it renders the fixed `⎿ Running in the background (↓ to manage)` row.
    /// The boundary's handler for `StreamEvent::ToolBackgrounded`. See
    /// `docs/background.md`.
    pub fn background_tool(&mut self, output: &str) -> Option<ToolCall> {
        self.resolve_front_tool(output, None, ToolStatus::Backgrounded)
    }

    /// The shared tail of [`end_tool`]/[`reject_tool`]/[`background_tool`]: pop
    /// the front call, stamp + record it with `status`, and fold its output into
    /// the token tally (arrow up — uploaded back; the count is *added to*, never
    /// reset — see `docs/status-indicator.md`). The tally charges the
    /// **model-facing** text ([`ToolCall::context_text`]), which is what the
    /// next request actually uploads — for a rejection that is the longer
    /// `context_output`, not the short cell line.
    ///
    /// [`end_tool`]: App::end_tool
    /// [`reject_tool`]: App::reject_tool
    /// [`background_tool`]: App::background_tool
    fn resolve_front_tool(
        &mut self,
        output: &str,
        context_output: Option<String>,
        status: ToolStatus,
    ) -> Option<ToolCall> {
        let mut tool = self.tool_queue.pop_front()?;
        tool.output = output.to_string();
        tool.context_output = context_output;
        tool.status = status;
        tool.timestamp = self.now_stamp();
        if let Some(turn) = self.status.as_mut() {
            turn.tokens += count_tokens(tool.context_text());
            turn.arrow = TokenArrow::Up;
        }
        self.history.push(HistoryItem::Tool(tool.clone()));
        Some(tool)
    }
}

/// The lifecycle of a tool call — selects its bullet colour when rendered:
/// waiting is dim, running is blue, success green, failure red.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    /// Queued in a parallel batch but not yet started — shown live as a dim
    /// `⎿ Waiting…` cell alongside the running call, until its own `ToolStart`
    /// flips it to [`Running`](ToolStatus::Running). Only a batched sibling is
    /// ever `Waiting`; a lone tool (the `!` shell, the dummy's single calls)
    /// starts `Running`. See `docs/parallel-tools.md`.
    Waiting,
    /// Executing — shown live (blue) in the bottom region while it runs.
    Running,
    /// Finished successfully (green).
    Ok,
    /// Finished with an error (red).
    Failed,
    /// Resolved by moving to the **background** (a `run_in_background` bash
    /// call, or Ctrl+B on a running command): the process keeps running under
    /// the [`App::background`] registry while the cell resolves with a green
    /// bullet and the fixed `⎿ Running in the background (↓ to manage)` row —
    /// the stored `output` is the model-facing text (interim-output
    /// path), never displayed. See `docs/background.md`.
    Backgrounded,
}

/// One tool invocation: its `name`, a short `args` summary, its lifecycle
/// `status`, and the (possibly multi-line) `output` it produced.
///
/// While running, `output` is empty/partial and `status` is
/// [`ToolStatus::Running`]; once finished it is recorded in [`App::history`] so
/// it repaints on resize and is listed in full in the Ctrl+O tool-output view.
/// Inline it renders collapsed (a one-line peek); the full `output` is only shown
/// in that separate view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub name: String,
    pub args: String,
    pub status: ToolStatus,
    pub output: String,
    /// Wall-clock stamp of when the call finished (set in [`App::end_tool`]).
    /// Recorded but not currently displayed — only user-message stamps show.
    /// Empty while running and when no clock is injected. See
    /// `docs/timestamps.md`.
    pub timestamp: String,
    /// Whether this is a `!` shell command (set by [`App::begin_shell`]). A
    /// shell call renders **headerless** — just its `⎿` output lines, flush
    /// under the [`Role::Shell`] header message recorded with it — instead of
    /// the `● name(args)` bullet header. See `docs/shell-command.md`.
    pub shell: bool,
    /// Set when a `!` shell command's output **exceeded the in-memory cap** and
    /// was cut: `output` holds only the retained head, and the cell appends a
    /// dim `…` marker at the end of the expanded output (`ui::tool_full_lines`)
    /// to show more was dropped. `false` for output kept in full. See
    /// `docs/shell-command.md`.
    pub truncated: bool,
    /// The **model-facing** tool result, when it differs from the displayed
    /// `output`. Set only by a permission rejection (`docs/permissions.md`):
    /// the cell shows the short `User rejected write to hello.py` (plus the
    /// amended instructions) while the model reads the full stop-and-wait
    /// text. [`crate::context::context_messages`] replays *this* — via
    /// [`context_text`](ToolCall::context_text) — so a later turn's context
    /// carries exactly what the live loop sent, Tab's amend feedback
    /// included.
    ///
    /// `None` for every ordinary call, whose display *is* what the model read.
    /// (A [`ToolStatus::Backgrounded`] call splits the same way from the other
    /// side: its `output` holds the model-facing launch text and the cell's
    /// row is synthesized from the status.)
    pub context_output: Option<String>,
    /// How the call came to run **without the user's approval** — the auto
    /// mode classifier's `Allowed by auto mode classifier`
    /// (`docs/permissions.md`). Set by [`App::set_tool_note`] right after
    /// the call starts; the resolved cell appends it as a dim `⎿` row (the
    /// example transcript's last line), and a `/resume` restores it. `None`
    /// for every call the user approved (or that needed no approval).
    pub approval_note: Option<String>,
    /// The model's **verbatim** JSON arguments for this call, beside the
    /// derived one-line `args` summary. This is what
    /// [`crate::context::context_messages`] replays on the assistant
    /// `tool_calls` entry, so a later turn sees the call the model really
    /// made: a `write`'s whole `content`, an `edit`'s two strings, a `bash`
    /// call's `timeout`. The summary alone was lossy — a `write` replayed as
    /// `{"path": …}` — which is why the executor could not collapse its
    /// result to one line before (`docs/context.md`, `docs/tools.md`).
    ///
    /// `None` for a `!` shell cell, a call the app synthesized itself, and
    /// every record written before the field existed — the summary
    /// reconstruction stays the fallback for those.
    pub arguments: Option<String>,
    /// The **parallel batch** this call was announced in
    /// ([`App::start_tool_batch`]), or `None` for a lone call. Every call of
    /// one round's batch shares the id, which is what lets the renderer
    /// collapse a run of MCP cells that really ran *in parallel* into one
    /// `Called deepwiki 2 times (ctrl+o to expand)` line without also
    /// collapsing two sequential calls that happen to sit next to each other
    /// in history. Round-trips through the rollout (`docs/mcp.md`).
    pub batch: Option<u64>,
    /// The provider's own id for this call — the `tool_calls[].id` the
    /// model's round carried and the wire echoed back with its result — so
    /// the next request's derived context can send the id the provider
    /// already cached rather than a synthesized `call_N`
    /// (`docs/prompt-caching.md`). `None` on a record made before the field
    /// existed, a `!` shell run, or a call no round announced.
    pub call_id: Option<String>,
    /// The call's index in its round as the model made it (0-based) —
    /// `batch`'s companion, and what lets the derived context replay a
    /// round in the wire's order when its records landed in another: a
    /// subagent group resolves before the round's ordinary calls run, so
    /// `[bash, agent, read]` records as `[agent, bash, read]`
    /// (`docs/prompt-caching.md`). `None` wherever `call_id` is.
    pub position: Option<usize>,
}

impl ToolCall {
    /// What the **model** read as this call's result — [`context_output`]
    /// when the display diverged from it, else the displayed `output`. The
    /// one seam [`crate::context::context_messages`] and the token tally read,
    /// so a rejection's replay can never drift from what was actually sent.
    ///
    /// [`context_output`]: ToolCall::context_output
    #[must_use]
    pub fn context_text(&self) -> &str {
        self.context_output.as_deref().unwrap_or(&self.output)
    }
}

/// How a file tool's path reads on the screen (`docs/tools.md` *Path
/// display*): a `Read`/`Write`/`Edit` argument under the session's cwd shows
/// **relative** to it (`hello.py`, `src/app.rs`), one outside the cwd but
/// under the home directory shows **`~`-relative** (`~/hello.py`), and
/// anything else shows **absolute** (`/tmp/x.py`; another user's home is not
/// `~`). The `● Write({path})` header applies it at render time — inline, in
/// the live strip and in the Ctrl+O transcript alike, as do the permission
/// prompt's target row and the agent rows that name a file — while the
/// `Wrote N lines to {path}` / `Updated {path} (+A -D)` corner head under it
/// stays what the executor recorded: its own cwd-relative form
/// (`llm::tools::display_path`, a `../` climb outside the cwd). The record
/// underneath keeps the model's own absolute argument, which is what the
/// derived context (Ctrl+D), the rollout, the classifier's action log and
/// the permission rules read.
///
/// The rule itself is [`llm::tools::header_path`](crate::llm::tools::header_path),
/// the corner row's `display_path`'s sibling (pure and lexical: `.`/`..`
/// collapse, a relative input resolves against the cwd first, symlinks are
/// never consulted, containment is component-wise so `/home/user2` is not
/// under `/home/user`); this is the session policy that carries the two
/// directories to it. Injected at the I/O boundary
/// ([`App::set_path_display`]) like the clock; the [`VERBATIM`](Self::VERBATIM)
/// default — no cwd — leaves every path exactly as recorded, which is what
/// every unit test renders with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathDisplay {
    /// The absolute working directory, or `None` for the verbatim policy.
    cwd: Option<PathBuf>,
    /// The absolute home directory, when one is known.
    home: Option<PathBuf>,
}

impl PathDisplay {
    /// The policy that shortens nothing — the unit-test default, and what a
    /// session whose cwd is unreadable falls back to.
    pub const VERBATIM: Self = Self {
        cwd: None,
        home: None,
    };

    /// The policy for a session launched in `cwd` with the user's `home`. A
    /// relative `cwd` has nothing to relate a path to and yields the verbatim
    /// policy; a relative `home` is dropped, leaving no `~` rule.
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>, home: Option<PathBuf>) -> Self {
        let cwd: PathBuf = cwd.into();
        Self {
            cwd: cwd.is_absolute().then_some(cwd),
            home: home.filter(|home| home.is_absolute()),
        }
    }

    /// `path` as the screen shows it — [`header_path`](crate::llm::tools::header_path)
    /// against the session's cwd and home: relative under the cwd (`.` for
    /// the cwd itself), `~`-relative under home (`~` for home itself),
    /// absolute otherwise — or unchanged under the verbatim policy.
    #[must_use]
    pub fn display(&self, path: &str) -> String {
        match &self.cwd {
            Some(cwd) => crate::llm::tools::header_path(path, cwd, self.home.as_deref()),
            None => path.to_string(),
        }
    }

    /// The `file://` target the header's shown path **links** to
    /// ([`links::file_url`](crate::links::file_url), `docs/links.md`): the
    /// file's absolute URI whatever short form [`display`](Self::display)
    /// painted, a relative argument resolved against the session's cwd.
    /// `None` when there is nothing to resolve a relative path against — the
    /// verbatim policy, whose absolute paths still link — or for an empty
    /// one; the header then paints the path plain.
    #[must_use]
    pub fn file_url(&self, path: &str) -> Option<String> {
        crate::links::file_url(path, self.cwd.as_deref())
    }
}

impl App {
    /// Inject the session's file-path display rule ([`PathDisplay`]) — called
    /// once at the I/O boundary with the process cwd and the user's home,
    /// like [`set_clock`](App::set_clock). Every `Read`/`Write`/`Edit` cell
    /// the session paints, inline or in the Ctrl+O transcript, reads its path
    /// through it; the records themselves are untouched.
    pub fn set_path_display(&mut self, paths: PathDisplay) {
        self.path_display = paths;
    }

    /// The session's file-path display rule — [`PathDisplay::VERBATIM`]
    /// until the boundary injects one.
    #[must_use]
    pub fn path_display(&self) -> &PathDisplay {
        &self.path_display
    }
}
