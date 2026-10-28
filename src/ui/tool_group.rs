//! The folded tool view (`docs/tool-groups.md`): with **Verbose output** off,
//! a run of read, search and list calls draws as one line — live in the
//! strip while the turn can still add to it, settled into scrollback once
//! something else happens.
//!
//! The state behind it is `app::tool_group`'s: which calls are members, which
//! are held, whether the group is open. This module only draws: the count
//! line in both tenses, the live cell with its `⎿` hint, the folded rebuild
//! walk, and the lines the boundary commits — built from the same history so
//! that what the commits wrote and what a rebuild draws agree.

use std::collections::BTreeSet;

use super::conversation::{push_item_lines, push_mcp_run};
use super::theme::*;
use super::tool::{bullet_span, result_row, tool_commit_lines};
use super::wrap::{cols, truncate_cols};
use super::*;
use crate::app::{ReadKind, ToolFold, held_members, passes_through, tool_group_member};

/// What a group counts: its searches, and the files and directories it read
/// and listed — **distinct** ones, so a `read` of `a.py` and a `cat a.py` are
/// one file. A read naming no file (a filter over standard input) counts
/// once; so does a bare `ls`, as the directory it lists.
#[derive(Debug, Default)]
struct Tally {
    searches: usize,
    files: BTreeSet<String>,
    unnamed_reads: usize,
    directories: BTreeSet<String>,
}

impl Tally {
    fn of(members: &[&ToolCall], paths: &PathDisplay) -> Self {
        let mut tally = Self::default();
        for reading in members.iter().filter_map(|tool| tool_group_member(tool)) {
            match reading.kind {
                ReadKind::Search => tally.searches += 1,
                ReadKind::Read if reading.targets.is_empty() => tally.unnamed_reads += 1,
                ReadKind::Read => tally
                    .files
                    .extend(reading.targets.iter().map(|path| paths.display(path))),
                ReadKind::List if reading.targets.is_empty() => {
                    tally.directories.insert(".".to_string());
                }
                ReadKind::List => tally.directories.extend(reading.targets.iter().map(|path| {
                    if path == "./" {
                        ".".to_string()
                    } else {
                        paths.display(path)
                    }
                })),
            }
        }
        tally
    }

    /// The count line — Claude Code's: `Searched for 1 pattern, read 2
    /// files, listed 1 directory`, or while `live` the present tense with a
    /// closing `…` (`Searching for 1 pattern, reading 2 files…`).
    fn text(&self, live: bool) -> String {
        let reads = self.files.len() + self.unnamed_reads;
        let parts: Vec<String> = [
            (
                self.searches,
                ["Searching for", "Searched for"],
                ["pattern", "patterns"],
            ),
            (reads, ["Reading", "Read"], ["file", "files"]),
            (
                self.directories.len(),
                ["Listing", "Listed"],
                ["directory", "directories"],
            ),
        ]
        .into_iter()
        .filter(|(count, ..)| *count > 0)
        .enumerate()
        .map(|(at, (count, verbs, nouns))| {
            let verb = verbs[usize::from(!live)];
            let noun = nouns[usize::from(count != 1)];
            // Only the line's first verb is capitalized.
            let verb = if at == 0 {
                verb.to_string()
            } else {
                verb.to_lowercase()
            };
            format!("{verb} {count} {noun}")
        })
        .collect();
        let mut text = parts.join(", ");
        if live {
            text.push('…');
        }
        text
    }
}

/// The settled line a group leaves in scrollback: its counts in the past
/// tense over the `(ctrl+o to expand)` hint, bullet-less and dim — the
/// settled-line family `Thought for …` and `Called {server}` belong to,
/// because what is left is a fact about the turn.
#[must_use]
pub fn tool_group_line(members: &[&ToolCall], paths: &PathDisplay) -> Line<'static> {
    Line::from(Span::styled(
        format!("{}{EXPAND_HINT}", Tally::of(members, paths).text(false)),
        Style::new().fg(reasoning_label_color()),
    ))
}

/// The clocks a live group reads — all boundary-injected, like every clock
/// the strip draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GroupClock {
    /// The strip's blink phase ([`App::pulse`]), or `None` for a group
    /// drawn **at rest** — over a permission prompt, which is a still frame.
    pub(super) pulse: Option<Duration>,
    /// The running command's own runtime ([`App::command_elapsed`]).
    pub(super) elapsed: Option<Duration>,
    /// The running command's elapsed as the Ctrl+B hint's gate
    /// ([`App::background_hint_elapsed`]).
    pub(super) hint_elapsed: Option<Duration>,
}

impl GroupClock {
    /// A group drawn at rest: the bullet shown, no command clock, no Ctrl+B
    /// hint — what a still frame shows of it.
    pub(super) const AT_REST: Self = Self {
        pulse: None,
        elapsed: None,
        hint_elapsed: None,
    };
}

/// How long a command runs before its group's hint carries its clock — the
/// threshold Claude Code's collapsed group uses, so a quick `ls` never
/// flickers a `(0s)`.
const GROUP_CLOCK_AFTER: Duration = Duration::from_secs(2);

/// The most rows a command's `⎿` hint takes — a script's first lines say
/// what it does; the rest is one `ctrl+o` away.
const GROUP_HINT_MAX_ROWS: usize = 2;

/// The live group cell: the tool bullet **blinking** at the strip's pulse
/// over the counts in the present tense and the dim `(ctrl+o to expand)` —
/// dropped first on a terminal too narrow for both — then a `⎿` row naming
/// what the group is doing: the running call, else the newest one to
/// resolve, else the first still waiting. A command that has run a couple
/// of seconds adds its clock, and past the Ctrl+B delay the hint the running
/// cell shows (`docs/tool-groups.md`). It blinks between calls too: the
/// group is not over until something else happens.
pub(super) fn live_tool_group_lines(
    members: &[&ToolCall],
    clock: GroupClock,
    width: u16,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    let label = Tally::of(members, paths).text(true);
    let width = usize::from(width);
    let room = width.saturating_sub(cols(TOOL_BULLET)).max(1);
    let mut header = vec![
        bullet_span(tool_running_color(), clock.pulse),
        Span::styled(
            truncate_cols(&label, room),
            Style::new()
                .fg(tool_name_color())
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if cols(TOOL_BULLET) + cols(&label) + cols(EXPAND_HINT) <= width {
        header.push(Span::styled(
            EXPAND_HINT.to_string(),
            Style::new().fg(tool_dim_color()),
        ));
    }
    let mut lines = vec![Line::from(header)];
    let running = members
        .iter()
        .find(|tool| tool.status == ToolStatus::Running);
    let Some(focus) = running
        .or_else(|| {
            members
                .iter()
                .rev()
                .find(|tool| !matches!(tool.status, ToolStatus::Waiting))
        })
        .or_else(|| members.first())
    else {
        return lines;
    };
    let command = focus.name == "Bash";
    let mut rows = if command {
        command_hint(&focus.args)
    } else {
        vec![paths.display(&focus.args)]
    };
    let suffix = match clock.elapsed {
        Some(elapsed) if running.is_some() && command && elapsed >= GROUP_CLOCK_AFTER => {
            format!(" ({})", format_elapsed(elapsed.as_secs()))
        }
        _ => String::new(),
    };
    let row_room = width.saturating_sub(cols(TOOL_RESULT_PREFIX)).max(1);
    let last = rows.len().saturating_sub(1);
    for (index, row) in rows.drain(..).enumerate() {
        let tail = if index == last { suffix.as_str() } else { "" };
        lines.push(result_row(index, fit_row(&row, row_room, tail)));
    }
    if running.is_some()
        && command
        && clock
            .hint_elapsed
            .is_some_and(|elapsed| elapsed >= TOOL_BACKGROUND_HINT_DELAY)
    {
        lines.push(result_row(1, TOOL_BACKGROUND_HINT.to_string()));
    }
    lines
}

/// A command as its group's hint shows it: `$ ` and its lines, each with its
/// whitespace collapsed and the blank ones dropped, at most
/// [`GROUP_HINT_MAX_ROWS`] of them — the last closing on `…` when there was
/// more.
fn command_hint(command: &str) -> Vec<String> {
    let lines: Vec<String> = command
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect();
    let cut = lines.len() > GROUP_HINT_MAX_ROWS;
    let mut rows: Vec<String> = lines.into_iter().take(GROUP_HINT_MAX_ROWS).collect();
    if let Some(first) = rows.first_mut() {
        first.insert_str(0, "$ ");
    }
    if cut && let Some(last) = rows.last_mut() {
        last.push('…');
    }
    rows
}

/// `text` then `tail`, within `room` columns: the text cut (closing on `…`)
/// to leave the tail whole.
fn fit_row(text: &str, room: usize, tail: &str) -> String {
    let text_room = room.saturating_sub(cols(tail));
    let mut row = if cols(text) <= text_room {
        text.to_string()
    } else {
        let mut cut = truncate_cols(text, text_room.saturating_sub(1));
        cut.push('…');
        cut
    };
    row.push_str(tail);
    row
}

/// The conversation drawn from `history` under `fold`: every call its own
/// cell when it is [`ToolFold::Off`] — exactly [`conversation_lines`] —
/// else each run of members as its settled line ([`tool_group_line`]) with
/// the same breakers the commits close a group on, a thought, task call or
/// hook note inside a run drawn where it lies (above the run's line, where
/// it committed while the group was live), and the open group — the run
/// starting at `held_from` — left to the strip.
#[must_use]
pub fn conversation_lines_with(
    history: &[HistoryItem],
    width: u16,
    paths: &PathDisplay,
    fold: ToolFold,
) -> Vec<Line<'static>> {
    let ToolFold::On { held_from } = fold else {
        return conversation_lines(history, width, paths);
    };
    let mut lines = Vec::new();
    let mut group: Vec<&ToolCall> = Vec::new();
    let close = |lines: &mut Vec<Line<'static>>, group: &mut Vec<&ToolCall>| {
        if !group.is_empty() {
            lines.push(tool_group_line(group, paths));
            lines.push(Line::default());
            group.clear();
        }
    };
    let mut index = 0;
    while index < history.len() {
        // The open group starts here: whatever ran before it committed as a
        // line of its own (the floor, `docs/tool-groups.md`).
        if Some(index) == held_from {
            close(&mut lines, &mut group);
        }
        let item = &history[index];
        if let HistoryItem::Tool(tool) = item
            && tool_group_member(tool).is_some()
        {
            group.push(tool);
            index += 1;
            continue;
        }
        if group.is_empty() || !passes_through(item) {
            close(&mut lines, &mut group);
        }
        if let Some(len) = push_mcp_run(&mut lines, &history[index..]) {
            index += len;
            continue;
        }
        push_item_lines(&mut lines, item, width, paths);
        index += 1;
    }
    if held_from.is_none() {
        close(&mut lines, &mut group);
    }
    lines
}

/// The lines a **just-resolved** call commits under the folded view — the
/// last item of `app.history`, the one commit site every resolution goes
/// through (`docs/tool-groups.md`):
///
/// - a member that leaves the group open commits nothing (`None`) — the
///   strip shows it, and the next call may join it;
/// - a member that ends it (the call up next is no member) commits the
///   group's line, itself included;
/// - a call that is no member commits the held group's line, then its own
///   cell — the MCP-aware [`tool_commit_lines`], whose held parallel run
///   still commits nothing.
///
/// With Verbose output on it is exactly [`tool_commit_lines`]. The caller
/// seals the group ([`App::seal_tool_group`]) whenever this returns lines.
#[must_use]
pub fn folded_tool_commit_lines(app: &App, width: u16) -> Option<Vec<Line<'static>>> {
    let paths = app.path_display();
    if app.settings().verbose {
        return tool_commit_lines(&app.history, app.tool_queue(), width, paths);
    }
    let history = &app.history;
    let HistoryItem::Tool(last) = history.last()? else {
        return None;
    };
    let member = tool_group_member(last).is_some();
    if member && app.tool_group_open() {
        return None;
    }
    let before = if member {
        history
    } else {
        &history[..history.len() - 1]
    };
    let held: Vec<&ToolCall> = held_members(before, app.tool_group_floor())
        .into_iter()
        .filter_map(|index| match &history[index] {
            HistoryItem::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect();
    let mut lines = Vec::new();
    if !held.is_empty() {
        lines.push(tool_group_line(&held, paths));
    }
    if !member {
        match tool_commit_lines(history, app.tool_queue(), width, paths) {
            Some(cell) if !cell.is_empty() => {
                if !lines.is_empty() {
                    lines.push(Line::default()); // the spacer between the two
                }
                lines.extend(cell);
            }
            // A parallel MCP run holding its line, with nothing of ours to
            // commit either.
            None if lines.is_empty() => return None,
            _ => {}
        }
    }
    Some(lines)
}

/// The held group's settled line — what a breaker commits **before** its
/// own rows (`docs/tool-groups.md`) — or `None` when nothing is held. The
/// caller seals the group ([`App::seal_tool_group`]) either way.
#[must_use]
pub fn held_tool_group_lines(app: &App) -> Option<Vec<Line<'static>>> {
    let held = app.held_tool_group();
    (!held.is_empty()).then(|| vec![tool_group_line(&held, app.path_display())])
}
