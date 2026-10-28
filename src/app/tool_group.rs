//! The compact tool display's **group** (`docs/compact-tools.md`): with the
//! `/settings` **Verbose output** row off, consecutive read-only calls — a
//! `read`, a `bash` listing, read or search ([`crate::readonly::kind`]) — fold
//! into one Claude-Code-style summary instead of a cell each:
//!
//! ```text
//! ● Searching for 1 pattern, reading 2 files… (ctrl+o to expand)
//!   ⎿  $ rg -n 'fn main' src
//! ```
//!
//! live in the strip while the group is **open**, and the bullet-less dim
//! `Searched for 1 pattern, read 2 files (ctrl+o to expand)` once it has
//! **settled** — the shape a parallel MCP run's `Called deepwiki 2 times`
//! line already has (`docs/mcp.md`). The members are ordinary
//! [`HistoryItem::Tool`] records: the Ctrl+O transcript, the derived
//! context and the rollout keep every one of them whole; only inline
//! scrollback — where a row, once written, is immutable — sees the summary,
//! which is why the group lives in the strip until something ends it.
//!
//! What ends a group ([`breaks`]): assistant text, a call that is not a
//! member (a `write`, a command that is not read-only, an MCP call, a failed
//! or refused member — a failure never whispers), a user message, a notice,
//! the turn's end. A thinking phase, a task call and a hook note are not
//! breakers (they are cell-less, or commit their own `Thought for …` line),
//! so a model that thinks between every read still folds its reads. The
//! decision is derived from the records (never recorded), so a `/resume` and
//! every purge rebuild fold the same cells; one runtime bit —
//! [`App::tool_group_open`] — says whether the trailing run is still held
//! out of scrollback.

use std::collections::BTreeSet;
use std::time::Duration;

use super::{App, HistoryItem, ToolCall, ToolStatus};
use crate::readonly::Kinds;

/// What a member of a group does, read off the call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Member {
    /// A `read` of `path` — counted once per distinct path.
    Read { path: String },
    /// A read-only `bash` command and what it looks like it does.
    Command { command: String, kinds: Kinds },
}

impl Member {
    /// The `⎿` hint a group shows while this member is its newest: the
    /// file's path (shown by the path display rule) or the command, after
    /// `$ `.
    #[must_use]
    pub fn hint(&self) -> Hint {
        match self {
            Self::Read { path } => Hint::Path(path.clone()),
            Self::Command { command, .. } => Hint::Command(command.clone()),
        }
    }
}

/// The group's `⎿` row: what its newest member is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hint {
    /// A file, shown by the session's path display rule.
    Path(String),
    /// A command, shown after `$ `.
    Command(String),
}

/// Whether `tool` — resolved or still queued — is a call the compact
/// display folds: a `read` that is not a picture, or a `bash` whose command
/// reads, lists or searches and nothing else.
#[must_use]
pub fn member_of(tool: &ToolCall) -> Option<Member> {
    if tool.shell {
        return None;
    }
    match tool.name.as_str() {
        // An image read commits a picture with its cell — something to look
        // at, never folded away (`docs/images.md`).
        "Read" if !tool.output.starts_with("Read image (") => Some(Member::Read {
            path: tool.args.clone(),
        }),
        "Bash" => {
            let command = tool
                .arguments
                .as_deref()
                .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
                .and_then(|v| v.get("command")?.as_str().map(str::to_string))
                .unwrap_or_else(|| tool.args.clone());
            let kinds = crate::readonly::kind(&command)?;
            Some(Member::Command { command, kinds })
        }
        _ => None,
    }
}

/// A resolved record that joins a group: a member that ran and succeeded.
#[must_use]
pub fn is_member(item: &HistoryItem) -> bool {
    matches!(item, HistoryItem::Tool(tool) if tool.status == ToolStatus::Ok && member_of(tool).is_some())
}

/// A record a group reads **through** without ending: cell-less inline (a
/// task call, a hook note) or committing a line of its own that says nothing
/// about the tools around it (a thinking phase's `Thought for …`).
#[must_use]
pub fn is_skippable(item: &HistoryItem) -> bool {
    matches!(
        item,
        HistoryItem::Reasoning(_) | HistoryItem::TaskCall(_) | HistoryItem::HookNote(_)
    )
}

/// A record that ends a group: anything that is neither a member nor
/// skippable.
#[must_use]
pub fn breaks(item: &HistoryItem) -> bool {
    !is_member(item) && !is_skippable(item)
}

/// The **run** leading `items`: how many records from its first — a member
/// — belong to one group, skippable records between and after the members
/// included, up to the first breaker. `0` when `items` does not open on a
/// member.
#[must_use]
pub fn run_len(items: &[HistoryItem]) -> usize {
    if !items.first().is_some_and(is_member) {
        return 0;
    }
    items.iter().take_while(|item| !breaks(item)).count()
}

/// The run **trailing** `history`: the members (and the skippable records
/// among them) after the last breaker — the group a turn in flight is
/// holding, or has just closed. Empty when the newest records hold no
/// member.
#[must_use]
pub fn trailing_run(history: &[HistoryItem]) -> &[HistoryItem] {
    let after_breaker = history
        .iter()
        .rposition(breaks)
        .map_or(0, |index| index + 1);
    let tail = &history[after_breaker..];
    let first_member = tail.iter().position(is_member);
    first_member.map_or(&tail[tail.len()..], |start| &tail[start..])
}

/// A group's tally: what its members did, and the hint of the newest.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolGroup {
    /// Searches — one per searching command.
    pub searches: usize,
    /// Reads — the distinct paths `read` opened plus one per reading command.
    pub reads: usize,
    /// Listings — one per listing command.
    pub lists: usize,
    /// How many calls joined.
    pub members: usize,
    /// The newest member's hint.
    pub hint: Option<Hint>,
    paths: BTreeSet<String>,
}

impl ToolGroup {
    /// Count `member` in.
    pub fn add(&mut self, member: &Member) {
        self.members += 1;
        self.hint = Some(member.hint());
        match member {
            Member::Read { path } => {
                if self.paths.insert(path.clone()) {
                    self.reads += 1;
                }
            }
            // A pipeline that both reads and searches is counted once, as
            // the most specific thing it does (Claude Code's own order).
            Member::Command { kinds, .. } => {
                if kinds.list {
                    self.lists += 1;
                } else if kinds.search {
                    self.searches += 1;
                } else if kinds.read {
                    self.reads += 1;
                }
            }
        }
    }

    /// The group of the members among `items` (a run, or any slice).
    #[must_use]
    pub fn of(items: &[HistoryItem]) -> Self {
        let mut group = Self::default();
        for item in items {
            if let HistoryItem::Tool(tool) = item
                && tool.status == ToolStatus::Ok
                && let Some(member) = member_of(tool)
            {
                group.add(&member);
            }
        }
        group
    }

    /// Nothing joined.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members == 0
    }

    /// The summary — `Searching for 1 pattern, reading 2 files, listing 1
    /// directory` while `active`, `Searched for 1 pattern, read 2 files,
    /// listed 1 directory` once settled: searches, reads, listings, in that
    /// order, the first clause capitalized, each only when counted.
    #[must_use]
    pub fn summary(&self, active: bool) -> String {
        let mut parts: Vec<String> = Vec::new();
        let plural = |n: usize, one: &str, many: &str| {
            if n == 1 {
                format!("{n} {one}")
            } else {
                format!("{n} {many}")
            }
        };
        if self.searches > 0 {
            let verb = if active {
                "searching for"
            } else {
                "searched for"
            };
            parts.push(format!(
                "{verb} {}",
                plural(self.searches, "pattern", "patterns")
            ));
        }
        if self.reads > 0 {
            let verb = if active { "reading" } else { "read" };
            parts.push(format!("{verb} {}", plural(self.reads, "file", "files")));
        }
        if self.lists > 0 {
            let verb = if active { "listing" } else { "listed" };
            parts.push(format!(
                "{verb} {}",
                plural(self.lists, "directory", "directories")
            ));
        }
        let mut summary = parts.join(", ");
        if let Some(first) = summary.get(..1) {
            summary.replace_range(..1, &first.to_uppercase());
        }
        summary
    }
}

/// What the commit of a just-resolved call does about the group
/// (`docs/compact-tools.md`), decided as the call resolves
/// ([`App::tool_group_commit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupCommit {
    /// Verbose output, or a call with no open group around it: the cell
    /// commits as it always did.
    #[default]
    Off,
    /// The call joined the group and the group goes on — nothing commits;
    /// the strip's summary speaks for it.
    Hold,
    /// The call joined the group and ends it (the next queued call is not a
    /// member): the group commits as one settled line, this call inside it.
    FlushWith,
    /// The call is a breaker that found the group open: the group's settled
    /// line commits first, then the call's own cell.
    FlushBefore,
}

/// The group the strip shows while it is open: the held members plus the
/// queue's leading members (the running call and the siblings behind it
/// that will join), with the running call's hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveGroup {
    /// The tally, held and queued members alike.
    pub group: ToolGroup,
    /// How many calls at the front of the queue the summary covers.
    pub queued: usize,
    /// How long the front call has been running, when it is a running
    /// command — the clock the hint row wears past the Ctrl+B hint's delay
    /// (`ui::TOOL_BACKGROUND_HINT_DELAY`).
    pub running: Option<Duration>,
}

impl App {
    /// The compact tool display is on (`docs/compact-tools.md`).
    #[must_use]
    pub fn compact_tools(&self) -> bool {
        self.settings.compact_tools()
    }

    /// Whether the trailing run of `history` is a group still **open** —
    /// held out of scrollback, the strip's summary speaking for it.
    #[must_use]
    pub const fn tool_group_open(&self) -> bool {
        self.tool_group_open
    }

    /// What the commit of the call that just resolved does about the group
    /// — decided as the call resolves (`App::resolve_front_tool`), read by
    /// the boundary's commit right after.
    #[must_use]
    pub const fn tool_group_commit(&self) -> GroupCommit {
        self.tool_group_commit
    }

    /// **Close** the open group — the boundary is about to commit a breaker
    /// (assistant text, a user message, a notice, the turn's end) and
    /// commits the group's settled line first. Returns whether a group was
    /// open, so the caller commits a line only when there is one.
    pub fn close_tool_group(&mut self) -> bool {
        std::mem::take(&mut self.tool_group_open)
    }

    /// Decide the group's fate as `tool` resolves — called with the queue
    /// already advanced past it, before it is recorded.
    pub(super) fn settle_tool_group(&mut self, tool: &ToolCall) {
        let was_open = self.tool_group_open;
        let compact = self.compact_tools();
        let member = compact && tool.status == ToolStatus::Ok && member_of(tool).is_some();
        self.tool_group_commit = if !compact {
            GroupCommit::Off
        } else if member {
            // The next call is already announced: a breaker there ends the
            // group now, with this call as its last member, rather than
            // leaving the summary to stand through that call's prompt.
            let next_breaks = self
                .tool_queue
                .front()
                .is_some_and(|next| member_of(next).is_none());
            if next_breaks {
                GroupCommit::FlushWith
            } else {
                GroupCommit::Hold
            }
        } else if was_open {
            GroupCommit::FlushBefore
        } else {
            GroupCommit::Off
        };
        self.tool_group_open = self.tool_group_commit == GroupCommit::Hold;
    }

    /// Whether a call about to **start** ends the open group: a breaker by
    /// name, in compact mode, with a group open. The boundary commits the
    /// settled line before the call's cell can appear.
    #[must_use]
    pub fn tool_group_breaks_on(&self, name: &str, args: &str, arguments: Option<&str>) -> bool {
        if !self.tool_group_open {
            return false;
        }
        let call = ToolCall {
            name: name.to_string(),
            args: args.to_string(),
            status: ToolStatus::Running,
            output: String::new(),
            timestamp: String::new(),
            shell: false,
            truncated: false,
            context_output: None,
            arguments: arguments.map(str::to_string),
            approval_note: None,
            batch: None,
            call_id: None,
            position: None,
        };
        member_of(&call).is_none()
    }

    /// The group the strip shows: `None` in verbose mode, or when nothing is
    /// held and the queue opens on no member.
    #[must_use]
    pub fn live_tool_group(&self) -> Option<LiveGroup> {
        if !self.compact_tools() {
            return None;
        }
        let mut group = if self.tool_group_open {
            ToolGroup::of(trailing_run(&self.history))
        } else {
            ToolGroup::default()
        };
        let queued: Vec<Member> = self.tool_queue.iter().map_while(member_of).collect();
        for member in &queued {
            group.add(member);
        }
        if group.is_empty() {
            return None;
        }
        // The running call is the newest thing happening, so its hint leads
        // — over the waiting siblings behind it that `add` passed last.
        if let Some(front) = queued.first() {
            group.hint = Some(front.hint());
        }
        let running = self
            .tool_queue
            .front()
            .filter(|front| {
                front.status == ToolStatus::Running
                    && matches!(queued.first(), Some(Member::Command { .. }))
            })
            .and_then(|_| self.command_elapsed());
        Some(LiveGroup {
            group,
            queued: queued.len(),
            running,
        })
    }

    /// Re-decide the open bit after a **Verbose output** switch mid-turn:
    /// turning the compact display on holds the trailing run while a turn is
    /// still running and no text has started, so the rebuild shows it in the
    /// strip; turning it off releases it, the rebuild painting every cell.
    pub fn reopen_tool_group(&mut self) {
        self.tool_group_open = self.compact_tools()
            && self.status.is_some()
            && !trailing_run(&self.history).is_empty()
            && self
                .streaming_text()
                .is_none_or(|text| text.trim().is_empty())
            && self
                .tool_queue
                .front()
                .is_none_or(|front| member_of(front).is_some());
    }

    /// Forget the group on a history rewind — a `/clear`, a `/resume`, a
    /// backtrack, an interrupt-undo: nothing is held across one.
    pub(super) fn drop_tool_group(&mut self) {
        self.tool_group_open = false;
        self.tool_group_commit = GroupCommit::Off;
    }
}
