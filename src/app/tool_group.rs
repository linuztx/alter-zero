//! The folded tool view's state (`docs/tool-groups.md`): which calls fold
//! into a read/search/list group, which of them are **held** out of
//! scrollback while the group is live, and when it is over.
//!
//! Pure — the rendering is `ui::tool_group`'s, the commits the boundary's.
//! Nothing here is stored but the [floor](App::tool_group_floor): whether a
//! group is open is re-derived from the turn on every read, so it can never
//! disagree with what the turn is doing.

use super::*;
pub use crate::read_only::{ReadKind, Reading};

/// How the conversation draws its tool calls — the `/settings` **Verbose
/// output** row, plus the group the strip is still showing
/// ([`App::tool_fold`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolFold {
    /// Every call its own cell: Verbose output on.
    #[default]
    Off,
    /// Each run of reads, searches and listings folds into one line; the
    /// run starting at `held_from` is still live in the strip, so a rebuild
    /// leaves it out.
    On {
        /// The history index of the open group's first member, if a group
        /// is open.
        held_from: Option<usize>,
    },
}

/// What `tool` contributes to a folded group, or `None` when it is not a
/// member: a `read` of anything but a picture, or a `bash` command that only
/// searches, reads or lists ([`crate::read_only::reading`]).
///
/// A call still queued or running counts by what it is; a resolved one only
/// if it **succeeded** — a failure leaves the group as its own red cell (the
/// MCP cells' rule: a failure must not whisper), except a search that exited
/// `1`, which is `grep`'s "nothing matched" and `find`'s "part of the tree
/// was unreadable", answers both. A command handed back still running (a
/// session frame, a move to the background) keeps its own cell, which says
/// where the session stands.
#[must_use]
pub fn tool_group_member(tool: &ToolCall) -> Option<Reading> {
    if tool.shell {
        return None;
    }
    let reading = reading_of(&tool.name, &tool.args)?;
    let kept = match tool.status {
        ToolStatus::Waiting | ToolStatus::Running => true,
        ToolStatus::Ok => !still_running(&tool.output),
        ToolStatus::Failed => {
            reading.kind == ReadKind::Search && exit_code(&tool.output) == Some(1)
        }
        ToolStatus::Backgrounded => false,
    };
    kept.then_some(reading)
}

/// Whether a call announced as `name` with the summary `args` — a batch's
/// entry, a lone call's start — would join a group: the membership a call
/// has before it has run, which is what decides whether the open group
/// survives its announcement (`docs/tool-groups.md`).
#[must_use]
pub fn joins_tool_group(name: &str, args: &str) -> bool {
    reading_of(name, args).is_some()
}

/// What a call reads as by its display `name` and `args` summary alone: a
/// `read` of anything but a picture, or a command that only searches, reads
/// or lists.
fn reading_of(name: &str, args: &str) -> Option<Reading> {
    match name {
        "Read" if !crate::llm::tools::is_image_path(args) => Some(Reading {
            kind: ReadKind::Read,
            targets: vec![args.to_string()],
        }),
        "Bash" => crate::read_only::reading(args),
        _ => None,
    }
}

/// The `Exit code: N` a command cell's output opens with, if it has one.
fn exit_code(output: &str) -> Option<i32> {
    let rest = output.strip_prefix("Exit code: ")?;
    rest.split('\n').next()?.trim().parse().ok()
}

/// Whether a command's output opens with a live-session frame — `Running
/// (session …)` or `Stopped (session …)` (`docs/interactive-shell.md`).
fn still_running(output: &str) -> bool {
    let first = output.split('\n').next().unwrap_or_default();
    crate::pty::report::parse_frame(first).is_some()
}

/// Whether `item` may sit inside a group without breaking it: nothing of it
/// stands between the calls on screen — a thinking phase (its settled line
/// commits where it lands, above the live group), a task call or a hook's
/// note (both cell-less inline).
#[must_use]
pub fn passes_through(item: &HistoryItem) -> bool {
    matches!(
        item,
        HistoryItem::Reasoning(_) | HistoryItem::TaskCall(_) | HistoryItem::HookNote(_)
    )
}

/// The history indices of the **held** group: the trailing run of members —
/// passing over what may sit inside a group — at or above `floor`, in order.
/// Empty when `history` ends on anything that breaks a group.
#[must_use]
pub fn held_members(history: &[HistoryItem], floor: usize) -> Vec<usize> {
    let mut members = Vec::new();
    for index in (floor..history.len()).rev() {
        match &history[index] {
            HistoryItem::Tool(tool) if tool_group_member(tool).is_some() => members.push(index),
            item if passes_through(item) => {}
            _ => break,
        }
    }
    members.reverse();
    members
}

impl App {
    /// Whether the trailing read/search/list group is still **open** — live
    /// in the strip and held out of scrollback, because the turn can still
    /// add to it. Derived on every call, never stored:
    ///
    /// - Verbose output is off, and a turn is in flight;
    /// - no assistant text has started (whitespace before a call is none);
    /// - no subagent group is live and no `write` is being generated — both
    ///   are cells of their own, and committed the group as they went up;
    /// - the call up next, if any, is a member;
    /// - and there are held members to show.
    #[must_use]
    pub fn tool_group_open(&self) -> bool {
        !self.settings.verbose
            && self.is_streaming()
            && self
                .streaming
                .as_deref()
                .is_none_or(|text| text.trim().is_empty())
            && self.agent_group.is_none()
            && self.tool_drafts.previews().next().is_none()
            && self
                .tool_queue
                .front()
                .is_none_or(|next| tool_group_member(next).is_some())
            && !held_members(&self.history, self.tool_group_floor).is_empty()
    }

    /// The members the strip is holding out of scrollback, in order — empty
    /// with Verbose output on, where every call commits as its own cell.
    #[must_use]
    pub fn held_tool_group(&self) -> Vec<&ToolCall> {
        if self.settings.verbose {
            return Vec::new();
        }
        held_members(&self.history, self.tool_group_floor)
            .into_iter()
            .filter_map(|index| match &self.history[index] {
                HistoryItem::Tool(tool) => Some(tool),
                _ => None,
            })
            .collect()
    }

    /// The history length at the last group commit — the floor the held run
    /// starts at or above ([`held_members`]).
    #[must_use]
    pub const fn tool_group_floor(&self) -> usize {
        self.tool_group_floor
    }

    /// The held group just reached scrollback (or would have, had committing
    /// been allowed — the return's rebuild draws it): nothing below here is
    /// held any more. The boundary calls it at every group commit.
    pub const fn seal_tool_group(&mut self) {
        self.tool_group_floor = self.history.len();
    }

    /// Keep the floor within a history that just shrank — `/clear`, a
    /// rewind, a withdrawn reply attempt — so the next run is held from its
    /// first member.
    pub(super) fn clamp_tool_group_floor(&mut self) {
        self.tool_group_floor = self.tool_group_floor.min(self.history.len());
    }

    /// How a rebuild draws the tool calls right now: every call its own cell
    /// with Verbose output on, else folded — leaving out the open group,
    /// which the strip is showing.
    #[must_use]
    pub fn tool_fold(&self) -> ToolFold {
        if self.settings.verbose {
            return ToolFold::Off;
        }
        let held_from = self
            .tool_group_open()
            .then(|| {
                held_members(&self.history, self.tool_group_floor)
                    .first()
                    .copied()
            })
            .flatten();
        ToolFold::On { held_from }
    }
}
