//! The collapsed read/search group's two looks (`docs/tool-groups.md`) —
//! Claude Code's non-verbose tool view.
//!
//! **Live**, in the strip while the group is open — its bullet blinking for
//! as long as the group stays open, between calls too, because the next call
//! may well be another read:
//!
//! ```text
//! ● Reading 1 file, listing 1 directory… (ctrl+o to expand)
//!   ⎿  $ ls -la
//! ```
//!
//! **Settled**, in scrollback once something that is not a read arrives —
//! bullet-less and dim, the settled thought's dress, since what is left is a
//! fact about the turn:
//!
//! ```text
//! Read 1 file, listed 1 directory (ctrl+o to expand)
//! ```
//!
//! Every call stays whole in the Ctrl+O transcript; this is only what the
//! conversation shows of them.

use super::reasoning::reasoning_lines;
use super::theme::*;
use super::tool::{bullet_span, result_row};
use super::wrap::{cols, truncate_cols};
use super::*;
use crate::app::{GroupCounts, group_counts, group_kind, is_group_member};

/// One clause of a group's line: its verb, its count, its noun.
struct Clause {
    verb: &'static str,
    count: usize,
    noun: &'static str,
}

/// A group's clauses in the reference's order — searches, then reads, then
/// listings — each verb in the tense the group is in (`Searching for` while
/// it runs, `Searched for` once it is over), nouns in the plural past one.
fn clauses(counts: GroupCounts, active: bool) -> Vec<Clause> {
    let pick = |live: &'static str, done: &'static str| if active { live } else { done };
    let plural = |count: usize, one: &'static str, many: &'static str| {
        if count == 1 { one } else { many }
    };
    let mut out = Vec::new();
    if counts.searches > 0 {
        out.push(Clause {
            verb: pick("searching for", "searched for"),
            count: counts.searches,
            noun: plural(counts.searches, "pattern", "patterns"),
        });
    }
    if counts.reads > 0 {
        out.push(Clause {
            verb: pick("reading", "read"),
            count: counts.reads,
            noun: plural(counts.reads, "file", "files"),
        });
    }
    if counts.lists > 0 {
        out.push(Clause {
            verb: pick("listing", "listed"),
            count: counts.lists,
            noun: plural(counts.lists, "directory", "directories"),
        });
    }
    out
}

/// The clauses as styled spans — `Read `, **`2`**, ` files, listed `, … —
/// every piece in `style`, the counts bold on top of it, the first verb
/// capitalised. `ellipsis` closes an open group's line (`…`).
fn summary_spans(counts: GroupCounts, active: bool, style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, clause) in clauses(counts, active).into_iter().enumerate() {
        let verb = if i == 0 {
            capitalize(clause.verb)
        } else {
            format!(", {}", clause.verb)
        };
        spans.push(Span::styled(format!("{verb} "), style));
        spans.push(Span::styled(
            clause.count.to_string(),
            style.add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {}", clause.noun), style));
    }
    if active {
        spans.push(Span::styled(TOOL_GROUP_ACTIVE_SUFFIX.to_string(), style));
    }
    spans
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `spans` laid out in `room` columns with the dim `(ctrl+o to expand)` hint
/// after them: the hint goes first when the row is too narrow for both, then
/// the clauses are cut with a `…` (into one span — a cut count would read as
/// a different number).
fn fit(mut spans: Vec<Span<'static>>, hint: Style, room: usize) -> Vec<Span<'static>> {
    let text: String = spans.iter().map(|span| span.content.as_ref()).collect();
    if cols(&text) + cols(EXPAND_HINT) <= room {
        spans.push(Span::styled(EXPAND_HINT.to_string(), hint));
        return spans;
    }
    if cols(&text) <= room {
        return spans;
    }
    let style = spans.first().map_or_else(Style::new, |span| span.style);
    let cut = truncate_cols(&text, room.saturating_sub(cols(TOOL_HEADER_ELLIPSIS)));
    vec![Span::styled(
        format!("{}{TOOL_HEADER_ELLIPSIS}", cut.trim_end()),
        style.remove_modifier(Modifier::BOLD),
    )]
}

/// A closed group as **committed** lines — what the commit path writes when
/// the group closes and what every rebuild paints in its place
/// ([`conversation_lines`]): the one settled line over the members of
/// `items` (a [`crate::app::group_run`]), then any thought the turn settled
/// after the group's last call, each under a blank spacer as its own cell —
/// the thoughts *between* its calls are part of the work the line stands for
/// and show in Ctrl+O.
#[must_use]
pub fn tool_group_lines(items: &[HistoryItem], width: u16) -> Vec<Line<'static>> {
    let members = items.iter().filter_map(|item| match item {
        HistoryItem::Tool(tool) => Some(tool),
        _ => None,
    });
    let counts = group_counts(members);
    let style = Style::new().fg(reasoning_label_color());
    let mut lines = vec![Line::from(fit(
        summary_spans(counts, false, style),
        style,
        usize::from(width),
    ))];
    let last_call = items
        .iter()
        .rposition(|item| matches!(item, HistoryItem::Tool(_)))
        .unwrap_or(0);
    for item in &items[last_call + 1..] {
        if let HistoryItem::Reasoning(reasoning) = item {
            lines.push(Line::default());
            lines.extend(reasoning_lines(reasoning, width));
        }
    }
    lines
}

/// What the group's `⎿` row says a call is doing: a read's file (the path
/// as the session shows paths, `docs/tools.md` *Path display*), a command as
/// `$ {command}` — its first line, its whitespace runs closed up, a `…` when
/// more lines follow.
fn hint_text(tool: &ToolCall, paths: &PathDisplay) -> String {
    if tool.name == "Read" {
        return paths.display(&tool.args);
    }
    let mut lines = tool.args.lines().filter(|line| !line.trim().is_empty());
    let first = lines
        .next()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    let more = if lines.next().is_some() { " …" } else { "" };
    format!("{TOOL_GROUP_COMMAND_PROMPT}{first}{more}")
}

/// The **open** group's live cell — the held reads plus, with `with_queue`,
/// the reads leading the live tool queue (running, or waiting their turn in
/// a batch) — or nothing when there is no group to show, or **Verbose
/// output** is on. `pulse` blinks the bullet in the strip; `None` draws it
/// at rest (the permission prompt's still frame, which passes `with_queue:
/// false` because the call it asks about has a cell of its own there).
///
/// The `⎿` hint names the running read when one runs, else the group's last
/// read — so between calls it keeps saying what the group last did.
pub(super) fn live_group_lines(
    app: &App,
    width: u16,
    pulse: Option<Duration>,
    with_queue: bool,
) -> Vec<Line<'static>> {
    if app.settings().verbose {
        return Vec::new();
    }
    let held: Vec<&ToolCall> = app
        .tool_group_range()
        .map(|range| {
            app.history[range]
                .iter()
                .filter_map(|item| match item {
                    HistoryItem::Tool(tool) if is_group_member(tool) => Some(tool),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let queued: Vec<&ToolCall> = if with_queue {
        leading_group_calls(app.tool_queue()).collect()
    } else {
        Vec::new()
    };
    let counts = group_counts(held.iter().chain(queued.iter()).copied());
    if counts.is_empty() {
        return Vec::new();
    }
    let label = Style::new().fg(tool_name_color());
    let room = usize::from(width).saturating_sub(cols(TOOL_BULLET));
    let mut spans = vec![bullet_span(tool_running_color(), pulse)];
    spans.extend(fit(
        summary_spans(counts, true, label),
        Style::new().fg(tool_dim_color()),
        room,
    ));
    let mut lines = vec![Line::from(spans)];
    let hint = queued
        .iter()
        .find(|tool| tool.status == ToolStatus::Running)
        .or_else(|| held.last())
        .or_else(|| queued.first())
        .map(|tool| hint_text(tool, app.path_display()));
    if let Some(hint) = hint {
        let room = usize::from(width).saturating_sub(cols(TOOL_RESULT_PREFIX));
        let shown = if cols(&hint) > room {
            let cut = truncate_cols(&hint, room.saturating_sub(cols(TOOL_HEADER_ELLIPSIS)));
            format!("{}{TOOL_HEADER_ELLIPSIS}", cut.trim_end())
        } else {
            hint
        };
        lines.push(result_row(0, shown));
    }
    lines
}

/// The calls at the front of a live tool queue that join the open group —
/// every one up to the first call of another kind, which keeps its own cell
/// (and closes the group when it starts).
pub(super) fn leading_group_calls(queue: &VecDeque<ToolCall>) -> impl Iterator<Item = &ToolCall> {
    queue.iter().take_while(|tool| group_kind(tool).is_some())
}
