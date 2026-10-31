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

use super::{
    App, ERROR_TOOL_OUTPUT, HistoryItem, INTERRUPT_TOOL_OUTPUT, PathDisplay, ToolCall, ToolDraft,
    ToolStatus,
};
use crate::read_only::ReadKind;

/// How long each call's value holds the open group's `⎿` row before the next
/// call's replaces it — the reference's `MIN_HINT_DISPLAY_MS`. A parallel
/// batch of reads resolves in milliseconds, so a row that followed the newest
/// call showed only the last one's value (`docs/tool-groups.md` *The hint
/// row*).
pub const GROUP_HINT_HOLD: Duration = Duration::from_millis(700);

/// What a call adds to a group: one of its clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupKind {
    /// A search: `grep`, `rg`, `find`, …
    Search,
    /// A file read: the `read` tool, `cat`, `head`, …
    Read,
    /// A directory listing: `ls`, `tree`, `du`.
    List,
    /// A `write` or `edit` of a file in a session's scratchpad — scratch
    /// work, where the reference folds it (`docs/tool-groups.md` *Scratchpad
    /// edits*).
    ScratchpadEdit,
}

impl From<ReadKind> for GroupKind {
    fn from(kind: ReadKind) -> Self {
        match kind {
            ReadKind::Search => Self::Search,
            ReadKind::Read => Self::Read,
            ReadKind::List => Self::List,
        }
    }
}

/// What a call adds to a group, however it resolved — `None` for a call that
/// never joins one: anything but a `Read`, a `Bash` command that only reads
/// ([`crate::read_only::read_kind`]) or a `Write`/`Edit` of a file in a
/// session's scratchpad ([`PathDisplay::in_scratchpad`]); a `!` command the
/// user ran; a command started in the background, whose cell is the process
/// it left running; and a read of an **image**, whose cell is the picture
/// drawn under it (`docs/images.md`) — folded into a group's line it would
/// draw nothing.
#[must_use]
pub fn group_kind(tool: &ToolCall, paths: &PathDisplay) -> Option<GroupKind> {
    if tool.shell {
        return None;
    }
    match tool.name.as_str() {
        "Read" if crate::llm::tools::is_image_path(&tool.args) => None,
        "Read" => Some(GroupKind::Read),
        "Bash" if !backgrounds(tool) => {
            crate::read_only::read_kind(&tool.args).map(GroupKind::from)
        }
        "Write" | "Edit" if paths.in_scratchpad(&tool.args) => Some(GroupKind::ScratchpadEdit),
        _ => None,
    }
}

/// How many lines a scratchpad edit added and removed — the `+55 -2` its
/// clause wears.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditLines {
    /// Lines added.
    pub added: usize,
    /// Lines removed.
    pub removed: usize,
}

impl std::ops::AddAssign for EditLines {
    fn add_assign(&mut self, other: Self) {
        self.added += other.added;
        self.removed += other.removed;
    }
}

/// The lines a **resolved** `write`/`edit` changed, read off its cell's head
/// (`llm::tools::write_report`/`update_report`): `Wrote {N} lines to …` for
/// a new file, `Updated … (+A -D)` for an overwrite or an edit, and the
/// legacy `Created … ({N} lines)` old rollouts carry. Nothing for `No
/// changes to …`, an error, or any other output.
#[must_use]
pub fn edit_lines(tool: &ToolCall) -> EditLines {
    let head = tool.output.lines().next().unwrap_or_default();
    let number = |text: &str| text.parse::<usize>().ok();
    if let Some(rest) = head.strip_prefix("Wrote ")
        && let Some(added) = rest.split(' ').next().and_then(number)
    {
        return EditLines { added, removed: 0 };
    }
    if head.starts_with("Updated ")
        && let Some(open) = head.rfind("(+")
        && let Some((added, removed)) = head[open + 2..]
            .strip_suffix(')')
            .and_then(|counts| counts.split_once(" -"))
        && let (Some(added), Some(removed)) = (number(added), number(removed))
    {
        return EditLines { added, removed };
    }
    if head.starts_with("Created ")
        && let Some(open) = head.rfind(" (")
        && let Some(added) = head[open + 2..].split(' ').next().and_then(number)
    {
        return EditLines { added, removed: 0 };
    }
    EditLines::default()
}

/// The lines a scratchpad edit that has **not run yet** will add: a
/// `write`'s own content — from the `draft` that streamed it when there is
/// one, else from the call's verbatim arguments — and nothing for an `edit`,
/// whose diff only running it reports.
#[must_use]
pub fn pending_edit_lines(tool: &ToolCall, draft: Option<&str>) -> EditLines {
    if tool.name != "Write" {
        return EditLines::default();
    }
    let added = match draft {
        Some(content) => content.lines().count(),
        None => tool
            .arguments
            .as_deref()
            .and_then(|arguments| {
                crate::llm::tools::parse_args::<crate::llm::tools::WriteArgs>(arguments).ok()
            })
            .map_or(0, |args| args.content.lines().count()),
    };
    EditLines { added, removed: 0 }
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
///
/// A scratchpad edit joins only once it **succeeded**: one that failed or was
/// refused did not happen, so it keeps its red cell and the line's "made N
/// edits" stays true.
#[must_use]
pub fn is_group_member(tool: &ToolCall, paths: &PathDisplay) -> bool {
    match group_kind(tool, paths) {
        None => return false,
        Some(GroupKind::ScratchpadEdit) => return tool.status == ToolStatus::Ok,
        Some(_) => {}
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
/// note have no cell inline at all, and a thought the model has while the
/// group is open is part of the run its line stands for rather than ending
/// it — shown only in the Ctrl+O transcript (`docs/tool-groups.md`
/// *Thoughts*).
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
pub fn group_run(items: &[HistoryItem], paths: &PathDisplay) -> Option<usize> {
    let HistoryItem::Tool(first) = items.first()? else {
        return None;
    };
    if !is_group_member(first, paths) {
        return None;
    }
    Some(
        items
            .iter()
            .take_while(|item| match item {
                HistoryItem::Tool(tool) => is_group_member(tool, paths),
                other => passes_through(other),
            })
            .count(),
    )
}

/// Where the run of members the history ends on starts — the group a turn
/// in flight would be holding — or `None` when the history does not end on
/// one (nothing but members and the items a group runs through since the
/// last item that is neither).
fn trailing_group_start(history: &[HistoryItem], paths: &PathDisplay) -> Option<usize> {
    let joins = |item: &HistoryItem| matches!(item, HistoryItem::Tool(tool) if is_group_member(tool, paths));
    let after = history
        .iter()
        .rposition(|item| !joins(item) && !passes_through(item))
        .map_or(0, |breaker| breaker + 1);
    (after..history.len()).find(|&at| joins(&history[at]))
}

/// A group's clauses: how many searches, files read, listings and
/// scratchpad edits it holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GroupCounts {
    /// Search commands — each one counts, the reference's "patterns".
    pub searches: usize,
    /// Files read — each `Read` path once however often it was read, plus
    /// every reading command (whose files the words do not name).
    pub reads: usize,
    /// Listing commands.
    pub lists: usize,
    /// Scratchpad edits — every `write`/`edit` call counts, the reference's
    /// "scratchpad edits".
    pub edits: usize,
    /// The lines those edits added and removed.
    pub lines: EditLines,
}

impl GroupCounts {
    /// Whether nothing is counted at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.searches == 0 && self.reads == 0 && self.lists == 0 && self.edits == 0
    }

    /// Count one more scratchpad edit — a `write` the model is still
    /// writing, which no call holds yet — with the lines it adds.
    pub fn add_edit(&mut self, lines: EditLines) {
        self.edits += 1;
        self.lines += lines;
    }
}

/// Count `calls` by their [`group_kind`] (calls of no kind count nothing).
/// A scratchpad edit's lines are counted once it **resolved**
/// ([`edit_lines`]); one still queued or running counts as an edit, and its
/// pending lines are the caller's to add ([`pending_edit_lines`]), since only
/// the caller knows the draft that may carry them.
#[must_use]
pub fn group_counts<'a>(
    calls: impl IntoIterator<Item = &'a ToolCall>,
    paths: &PathDisplay,
) -> GroupCounts {
    let mut counts = GroupCounts::default();
    let mut read: HashSet<&str> = HashSet::new();
    for call in calls {
        match group_kind(call, paths) {
            Some(GroupKind::Search) => counts.searches += 1,
            Some(GroupKind::List) => counts.lists += 1,
            Some(GroupKind::Read) if call.name == "Read" => {
                if read.insert(call.args.as_str()) {
                    counts.reads += 1;
                }
            }
            Some(GroupKind::Read) => counts.reads += 1,
            Some(GroupKind::ScratchpadEdit) => {
                counts.edits += 1;
                if call.status == ToolStatus::Ok {
                    counts.lines += edit_lines(call);
                }
            }
            None => {}
        }
    }
    counts
}

/// The calls at the front of a live tool queue that join the open group —
/// every one up to the first call of another kind, which keeps its own cell
/// (and closes the group when it starts).
pub fn leading_group_calls<'a>(
    queue: &'a VecDeque<ToolCall>,
    paths: &'a PathDisplay,
) -> impl Iterator<Item = &'a ToolCall> {
    queue
        .iter()
        .take_while(move |tool| group_kind(tool, paths).is_some())
}

/// Which of the open group's calls its `⎿` row is showing, and since when —
/// stepped by [`App::advance_group_hint`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct GroupHint {
    /// The value on the row, as an index into [`App::group_hint_items`].
    shown: usize,
    /// The frame clock's reading when that call went up; `None` until a tick
    /// has seen the row.
    since: Option<Duration>,
}

/// One value the open group's `⎿` row can show
/// ([`App::group_hint_items`]).
#[derive(Debug, Clone, Copy)]
pub enum GroupHintItem<'a> {
    /// A call the group holds, or one leading the live queue.
    Call(&'a ToolCall),
    /// A scratchpad `write` the model is still writing, its path out
    /// ([`App::is_group_draft`]).
    Draft(&'a ToolDraft),
}

impl<'a> GroupHintItem<'a> {
    /// What the value is about: a call's summary — its path or its command —
    /// or the draft's path.
    #[must_use]
    pub fn target(&self) -> &'a str {
        match self {
            Self::Call(tool) => &tool.args,
            Self::Draft(draft) => draft.path().unwrap_or_default(),
        }
    }
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
        let member = matches!(
            self.history.last(),
            Some(HistoryItem::Tool(tool)) if is_group_member(tool, &self.path_display)
        );
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

    /// Hold the reads a turn in flight already committed as cells of their
    /// own, when **Verbose output** turns off under it: the rebuild the row
    /// triggers paints them as the open group, and the next read joins them
    /// — so a later rebuild draws the one line scrollback will show, never a
    /// line for the reads before the switch and another for the ones after.
    /// Nothing is held between turns (a rebuild draws a settled run's line by
    /// itself), nor once something has ended the run: reply text, a call of
    /// another kind running or being written (a write into the scratchpad is
    /// part of the run), a subagent launch.
    pub(super) fn hold_trailing_group(&mut self) {
        let moved_on = self
            .streaming_text()
            .is_some_and(|text| !text.trim().is_empty())
            || self
                .tool_queue
                .front()
                .is_some_and(|tool| group_kind(tool, &self.path_display).is_none())
            || self.draft_ends_group()
            || self.agent_group().is_some();
        if self.settings.verbose || !self.is_streaming() || self.tool_group.is_some() || moved_on {
            return;
        }
        self.tool_group = trailing_group_start(&self.history, &self.path_display);
        // The row names the newest of them: the ones before it were cells.
        let held = self.group_hint_items(false).len();
        self.group_hint = GroupHint {
            shown: held.saturating_sub(1),
            since: None,
        };
    }

    /// The open group's values in the order they came — the calls it holds,
    /// then the reads leading the live queue that have started, then the
    /// scratchpad writes still being generated ([`group_drafts`](App::group_drafts))
    /// — every value the `⎿` row steps through; before any of them, the
    /// batch's first call, about to run. With `with_queue` false, the held
    /// calls alone: the permission prompt's still frame, where the call being
    /// asked about has a cell of its own. Empty with **Verbose output** on.
    #[must_use]
    pub fn group_hint_items(&self, with_queue: bool) -> Vec<GroupHintItem<'_>> {
        if self.settings.verbose {
            return Vec::new();
        }
        let mut items: Vec<GroupHintItem<'_>> = self
            .tool_group_range()
            .map(|range| {
                self.history[range]
                    .iter()
                    .filter_map(|item| match item {
                        HistoryItem::Tool(tool) if is_group_member(tool, &self.path_display) => {
                            Some(GroupHintItem::Call(tool))
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        if with_queue {
            let leading: Vec<&ToolCall> =
                leading_group_calls(&self.tool_queue, &self.path_display).collect();
            let started = leading
                .iter()
                .take_while(|tool| tool.status == ToolStatus::Running)
                .count();
            items.extend(
                leading[..started]
                    .iter()
                    .map(|tool| GroupHintItem::Call(tool)),
            );
            items.extend(self.group_drafts().map(GroupHintItem::Draft));
            if items.is_empty() {
                items.extend(leading.first().map(|tool| GroupHintItem::Call(tool)));
            }
        }
        items
    }

    /// The value the open group's `⎿` row shows — the one
    /// [`advance_group_hint`](App::advance_group_hint) has stepped to among
    /// [`group_hint_items`](App::group_hint_items). `None` when there is no
    /// group to show.
    #[must_use]
    pub fn group_hint(&self, with_queue: bool) -> Option<GroupHintItem<'_>> {
        let items = self.group_hint_items(with_queue);
        let last = items.len().checked_sub(1)?;
        Some(items[self.group_hint.shown.min(last)])
    }

    /// Whether `draft` — a `write` the model is still generating — is part
    /// of the open group rather than a cell of its own: a write into a
    /// session's scratchpad, once its path is out, with **Verbose output**
    /// off (`docs/tool-groups.md` *Scratchpad edits*).
    #[must_use]
    pub fn is_group_draft(&self, draft: &ToolDraft) -> bool {
        !self.settings.verbose
            && draft
                .path()
                .is_some_and(|path| self.path_display.in_scratchpad(path))
    }

    /// The writes still being generated that are part of the group
    /// ([`is_group_draft`](App::is_group_draft)), in the order each opened.
    pub fn group_drafts(&self) -> impl Iterator<Item = &ToolDraft> {
        self.tool_drafts
            .previews()
            .filter(|draft| self.is_group_draft(draft))
    }

    /// Whether a write still being generated ends the open group: one whose
    /// path is out and lies outside every scratchpad. Until its path is out
    /// nothing says where it writes, so the group stays open over its cell
    /// (`docs/tool-groups.md` *Scratchpad edits*).
    #[must_use]
    pub fn draft_ends_group(&self) -> bool {
        self.tool_drafts.previews().any(|draft| {
            draft
                .path()
                .is_some_and(|path| !self.path_display.in_scratchpad(path))
        })
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
        let reached = self.group_hint_items(true).len();
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
