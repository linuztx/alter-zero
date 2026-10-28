//! Collapsed read/search groups (`docs/tool-groups.md`): Claude Code's
//! non-verbose tool view, where a run of reads, searches and listings shows
//! as one cell — `● Reading 2 files, listing 1 directory…` while it runs,
//! `Read 2 files, listed 1 directory (ctrl+o to expand)` once it is over —
//! instead of a cell per call.
//!
//! The grouping is a property of the **history**, like the parallel MCP run
//! (`docs/mcp.md`), so scrollback and every rebuild derive it from the same
//! place: [`group_run`] finds a group wherever one starts, and the open one
//! — still collecting calls, so not yet in scrollback — is the history from
//! [`App::tool_group_range`]'s start on, held by the commit path until
//! something that is not part of it arrives.

use std::collections::{HashSet, VecDeque};
use std::ops::Range;
use std::time::Duration;

use super::{App, ERROR_TOOL_OUTPUT, HistoryItem, INTERRUPT_TOOL_OUTPUT, ToolCall, ToolStatus};
use crate::read_only::ReadKind;

/// How long each call's value holds the open group's `⎿` row before the next
/// call's replaces it — the reference's `MIN_HINT_DISPLAY_MS`. A parallel
/// batch of reads resolves in milliseconds, so a row that followed the newest
/// call showed only the last one's value (`docs/tool-groups.md` *The hint
/// row*).
pub const GROUP_HINT_HOLD: Duration = Duration::from_millis(700);

/// What a call adds to a group, however it resolved — `None` for a call that
/// never joins one: anything but a `Read` or a `Bash` command that only
/// reads ([`crate::read_only::read_kind`]), a `!` command the user ran, a
/// command started in the background, whose cell is the process it left
/// running, and a read of an **image**, whose cell is the picture drawn under
/// it (`docs/images.md`) — folded into a group's line it would draw nothing.
#[must_use]
pub fn group_kind(tool: &ToolCall) -> Option<ReadKind> {
    if tool.shell {
        return None;
    }
    match tool.name.as_str() {
        "Read" if crate::llm::tools::is_image_path(&tool.args) => None,
        "Read" => Some(ReadKind::Read),
        "Bash" if !backgrounds(tool) => crate::read_only::read_kind(&tool.args),
        _ => None,
    }
}

/// Whether a `bash` call starts its command in the background — `wait: 0`
/// or the old `run_in_background: true` in its arguments (a trailing `&` the
/// command classifier refuses on its own).
fn backgrounds(tool: &ToolCall) -> bool {
    tool.arguments
        .as_deref()
        .and_then(|arguments| {
            crate::llm::tools::parse_args::<crate::llm::tools::BashArgs>(arguments).ok()
        })
        .is_some_and(|args| args.explicit_background())
}

/// Whether a **resolved** record belongs in a group: a call of a group kind
/// that ran — whether it succeeded or failed in the running (a search that
/// found nothing exits 1, a missing file fails the read; the model reads the
/// error either way, and Ctrl+O keeps it) — rather than one that never ran
/// or never finished: refused at the prompt or by the classifier, blocked by
/// a hook, cut by Esc or a backend error, refused by the tool-call limit, or
/// moved to the background. Each of those keeps a cell of its own.
#[must_use]
pub fn is_group_member(tool: &ToolCall) -> bool {
    if group_kind(tool).is_none() {
        return false;
    }
    match tool.status {
        ToolStatus::Ok => true,
        ToolStatus::Failed => {
            // A refusal carries the longer text the model read beside it.
            tool.context_output.is_none()
                && !matches!(
                    tool.output.as_str(),
                    INTERRUPT_TOOL_OUTPUT
                        | ERROR_TOOL_OUTPUT
                        | crate::llm::agent::TOOL_LIMIT_OUTPUT
                )
        }
        ToolStatus::Waiting | ToolStatus::Running | ToolStatus::Backgrounded => false,
    }
}

/// Whether `item` lets a group run on through it: a task call and a hook
/// note have no cell inline at all, and a thought between two reads is part
/// of the work the group stands for (one after the group's last read is
/// shown after the group's line).
fn passes_through(item: &HistoryItem) -> bool {
    matches!(
        item,
        HistoryItem::TaskCall(_) | HistoryItem::HookNote(_) | HistoryItem::Reasoning(_)
    )
}

/// The group `items` opens with, as how many items it spans: from its first
/// member through every member and pass-through item up to the first item
/// that is neither — a reply, a call of another kind, a refused read, a
/// user message, the turn's end. `None` when `items` does not open with a
/// member. A lone read is a group of one.
#[must_use]
pub fn group_run(items: &[HistoryItem]) -> Option<usize> {
    let HistoryItem::Tool(first) = items.first()? else {
        return None;
    };
    if !is_group_member(first) {
        return None;
    }
    Some(
        items
            .iter()
            .take_while(|item| match item {
                HistoryItem::Tool(tool) => is_group_member(tool),
                other => passes_through(other),
            })
            .count(),
    )
}

/// A group's clauses: how many searches, files read, and listings it holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GroupCounts {
    /// Search commands — each one counts, the reference's "patterns".
    pub searches: usize,
    /// Files read — each `Read` path once however often it was read, plus
    /// every reading command (whose files the words do not name).
    pub reads: usize,
    /// Listing commands.
    pub lists: usize,
}

impl GroupCounts {
    /// Whether nothing is counted at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.searches == 0 && self.reads == 0 && self.lists == 0
    }
}

/// Count `calls` by their [`group_kind`] (calls of no kind count nothing).
#[must_use]
pub fn group_counts<'a>(calls: impl IntoIterator<Item = &'a ToolCall>) -> GroupCounts {
    let mut counts = GroupCounts::default();
    let mut paths: HashSet<&str> = HashSet::new();
    for call in calls {
        match group_kind(call) {
            Some(ReadKind::Search) => counts.searches += 1,
            Some(ReadKind::List) => counts.lists += 1,
            Some(ReadKind::Read) if call.name == "Read" => {
                if paths.insert(call.args.as_str()) {
                    counts.reads += 1;
                }
            }
            Some(ReadKind::Read) => counts.reads += 1,
            None => {}
        }
    }
    counts
}

/// The calls at the front of a live tool queue that join the open group —
/// every one up to the first call of another kind, which keeps its own cell
/// (and closes the group when it starts).
pub fn leading_group_calls(queue: &VecDeque<ToolCall>) -> impl Iterator<Item = &ToolCall> {
    queue.iter().take_while(|tool| group_kind(tool).is_some())
}

/// Which of the open group's calls its `⎿` row is showing, and since when —
/// stepped by [`App::advance_group_hint`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct GroupHint {
    /// The call on the row, as an index into [`App::group_hint_calls`].
    shown: usize,
    /// The frame clock's reading when that call went up; `None` until a tick
    /// has seen the row.
    since: Option<Duration>,
}

/// Where the call the boundary just resolved goes
/// ([`App::place_resolved_tool`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolPlacement {
    /// It joined the open group: nothing reaches scrollback until the group
    /// closes.
    Held,
    /// It commits as its own cell — after the group it closed, when it closed
    /// one: `closed` is the held range of history to commit first.
    Commit { closed: Option<Range<usize>> },
}

impl App {
    /// Place the call just resolved — the last record of the history — the
    /// one decision every resolution (`ToolEnd`, a rejection, an answer, a
    /// background launch) makes before it commits (`docs/tool-groups.md`). A
    /// group member is **held** in the open group, opening one at itself
    /// when none is; anything else closes the open group ahead of itself.
    /// With **Verbose output** on nothing is ever held.
    pub fn place_resolved_tool(&mut self) -> ToolPlacement {
        let len = self.history.len();
        let member =
            matches!(self.history.last(), Some(HistoryItem::Tool(tool)) if is_group_member(tool));
        if member && !self.settings.verbose {
            self.tool_group.get_or_insert(len - 1);
            return ToolPlacement::Held;
        }
        let end = if member { len } else { len.saturating_sub(1) };
        let closed = self
            .tool_group
            .take()
            .map(|start| start..end)
            .filter(|range| !range.is_empty());
        if closed.is_some() {
            self.group_hint = GroupHint::default();
        }
        ToolPlacement::Commit { closed }
    }

    /// Close the open group, handing back the range of history it held for
    /// the boundary to commit — `None` when no group is open. Called wherever
    /// something that is not part of the group is about to reach scrollback:
    /// a reply, a call of another kind, a steered message, a notice, the
    /// turn's end.
    pub fn close_tool_group(&mut self) -> Option<Range<usize>> {
        let start = self.tool_group.take()?;
        self.group_hint = GroupHint::default();
        let range = start..self.history.len();
        (!range.is_empty()).then_some(range)
    }

    /// The range of history the open group holds — recorded, but not yet in
    /// scrollback; what every rebuild leaves out while the strip shows it.
    /// `None` when no group is open.
    #[must_use]
    pub fn tool_group_range(&self) -> Option<Range<usize>> {
        self.tool_group
            .map(|start| start.min(self.history.len())..self.history.len())
    }

    /// Let go of the open group without committing it — the history it held
    /// is rendered by the rebuild that follows (a **Verbose output** flip,
    /// a cleared or rewound conversation).
    pub(super) fn drop_tool_group(&mut self) {
        self.tool_group = None;
        self.group_hint = GroupHint::default();
    }

    /// The open group's calls in the order they ran — the held ones, then
    /// the reads leading the live queue that have started — every value the
    /// `⎿` row steps through; before any has started, the batch's first call,
    /// about to run. With `with_queue` false, the held calls alone: the
    /// permission prompt's still frame, where the call being asked about has
    /// a cell of its own. Empty with **Verbose output** on.
    #[must_use]
    pub fn group_hint_calls(&self, with_queue: bool) -> Vec<&ToolCall> {
        if self.settings.verbose {
            return Vec::new();
        }
        let mut calls: Vec<&ToolCall> = self
            .tool_group_range()
            .map(|range| {
                self.history[range]
                    .iter()
                    .filter_map(|item| match item {
                        HistoryItem::Tool(tool) if is_group_member(tool) => Some(tool),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        if with_queue {
            let leading: Vec<&ToolCall> = leading_group_calls(&self.tool_queue).collect();
            let started = leading
                .iter()
                .take_while(|tool| tool.status == ToolStatus::Running)
                .count();
            calls.extend(&leading[..started]);
            if calls.is_empty() {
                calls.extend(leading.first());
            }
        }
        calls
    }

    /// The call the open group's `⎿` row shows — the one
    /// [`advance_group_hint`](App::advance_group_hint) has stepped to among
    /// [`group_hint_calls`](App::group_hint_calls). `None` when there is no
    /// group to show.
    #[must_use]
    pub fn group_hint(&self, with_queue: bool) -> Option<&ToolCall> {
        let calls = self.group_hint_calls(with_queue);
        let last = calls.len().checked_sub(1)?;
        Some(calls[self.group_hint.shown.min(last)])
    }

    /// Step the open group's `⎿` row on the boundary's frame clock `now`
    /// (injected before each draw, beside [`set_pulse`](App::set_pulse)):
    /// once the call on the row has held it for [`GROUP_HINT_HOLD`], the
    /// next call that ran replaces it — one step per hold, in the order the
    /// calls ran, so a batch that resolves within a frame shows every value
    /// instead of its last (the reference throttles the row instead, which
    /// drops a value that arrives during the hold). A group with nothing to
    /// show resets the row, so the next one starts from its own first call.
    pub fn advance_group_hint(&mut self, now: Duration) {
        let reached = self.group_hint_calls(true).len();
        let Some(last) = reached.checked_sub(1) else {
            self.group_hint = GroupHint::default();
            return;
        };
        let hint = &mut self.group_hint;
        hint.shown = hint.shown.min(last);
        let since = *hint.since.get_or_insert(now);
        if hint.shown < last && now.saturating_sub(since) >= GROUP_HINT_HOLD {
            hint.shown += 1;
            hint.since = Some(now);
        }
    }
}
