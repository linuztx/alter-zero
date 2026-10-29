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
use super::tool::{bullet_span, command_display_lines, result_row};
use super::wrap::{cols, truncate_cols, wrap_output};
use super::*;
use crate::app::{
    EditLines, GroupCounts, GroupHintItem, GroupKind, ToolDraft, group_counts, group_kind,
    is_group_member, leading_group_calls, pending_edit_lines, previewed_display,
};

/// One clause of a group's line: its verb, its count, its noun — and, for
/// the scratchpad edits, the lines they changed.
struct Clause {
    verb: &'static str,
    count: usize,
    noun: &'static str,
    lines: Option<EditLines>,
}

/// A group's clauses in the reference's order — what it changed, then what
/// it read: scratchpad edits, then searches, reads and listings — each verb
/// in the tense the group is in (`Searching for` while it runs, `Searched
/// for` once it is over), nouns in the plural past one.
fn clauses(counts: GroupCounts, active: bool) -> Vec<Clause> {
    let pick = |live: &'static str, done: &'static str| if active { live } else { done };
    let plural = |count: usize, one: &'static str, many: &'static str| {
        if count == 1 { one } else { many }
    };
    let mut out = Vec::new();
    if counts.edits > 0 {
        out.push(Clause {
            verb: pick("making", "made"),
            count: counts.edits,
            noun: plural(counts.edits, "scratchpad edit", "scratchpad edits"),
            lines: Some(counts.lines),
        });
    }
    if counts.searches > 0 {
        out.push(Clause {
            verb: pick("searching for", "searched for"),
            count: counts.searches,
            noun: plural(counts.searches, "pattern", "patterns"),
            lines: None,
        });
    }
    if counts.reads > 0 {
        out.push(Clause {
            verb: pick("reading", "read"),
            count: counts.reads,
            noun: plural(counts.reads, "file", "files"),
            lines: None,
        });
    }
    if counts.lists > 0 {
        out.push(Clause {
            verb: pick("listing", "listed"),
            count: counts.lists,
            noun: plural(counts.lists, "directory", "directories"),
            lines: None,
        });
    }
    out
}

/// The clauses as styled spans — `Read `, **`2`**, ` files, listed `, … —
/// every piece in `style`, the counts bold on top of it, the first verb
/// capitalised, the scratchpad edits' `+55 -2` in `dim`. `ellipsis` closes
/// an open group's line (`…`).
fn summary_spans(
    counts: GroupCounts,
    active: bool,
    style: Style,
    dim: Style,
) -> Vec<Span<'static>> {
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
        // `+55 -2`, dim beside the clause; a side that changed nothing is
        // left out.
        let lines = clause.lines.unwrap_or_default();
        for (count, sign) in [(lines.added, '+'), (lines.removed, '-')] {
            if count > 0 {
                spans.push(Span::styled(" ".to_string(), style));
                spans.push(Span::styled(format!("{sign}{count}"), dim));
            }
        }
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
/// `items` (a [`crate::app::group_run`]), then every thought the turn
/// settled while the group was open, in order, each under a blank spacer as
/// its own cell — the reference defers them past the group's line, which
/// stays where the group's first call was.
#[must_use]
pub fn tool_group_lines(
    items: &[HistoryItem],
    width: u16,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    let members = items.iter().filter_map(|item| match item {
        HistoryItem::Tool(tool) => Some(tool),
        _ => None,
    });
    let counts = group_counts(members, paths);
    let style = Style::new().fg(reasoning_label_color());
    let mut lines = vec![Line::from(fit(
        summary_spans(counts, false, style, style),
        style,
        usize::from(width),
    ))];
    for item in items {
        if let HistoryItem::Reasoning(reasoning) = item {
            lines.push(Line::default());
            lines.extend(reasoning_lines(reasoning, width));
        }
    }
    lines
}

/// What the group's `⎿` row says a value is about.
enum HintText {
    /// A file — read, or edited in the scratchpad — as the session shows
    /// paths (`docs/tools.md` *Path display*).
    Path(String),
    /// A command, as `$ {command}`: its first line, its whitespace runs
    /// closed up, a `…` when more lines follow.
    Command(String),
}

fn hint_text(item: GroupHintItem<'_>, paths: &PathDisplay) -> HintText {
    let tool = match item {
        GroupHintItem::Call(tool) if tool.name == "Bash" => tool,
        other => return HintText::Path(paths.display(other.target())),
    };
    let mut lines = tool.args.lines().filter(|line| !line.trim().is_empty());
    let first = lines
        .next()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    let more = if lines.next().is_some() { " …" } else { "" };
    HintText::Command(format!("{TOOL_GROUP_COMMAND_PROMPT}{first}{more}"))
}

/// A path on the `⎿` row, **wrapped** under the corner rather than cut —
/// its file name is at its end, which a cut would take — over at most
/// [`TOOL_GROUP_HINT_MAX_ROWS`] rows, the last cut with a `…` when even those
/// are not enough.
fn path_hint_rows(path: &str, room: usize) -> Vec<Line<'static>> {
    let width = u16::try_from(room.max(1)).unwrap_or(u16::MAX);
    let mut rows = wrap_output(path, width);
    if rows.len() > TOOL_GROUP_HINT_MAX_ROWS {
        rows.truncate(TOOL_GROUP_HINT_MAX_ROWS);
        if let Some(last) = rows.last_mut() {
            let cut = truncate_cols(last, room.saturating_sub(cols(TOOL_HEADER_ELLIPSIS)));
            *last = format!("{}{TOOL_HEADER_ELLIPSIS}", cut.trim_end());
        }
    }
    rows.into_iter()
        .enumerate()
        .map(|(at, row)| result_row(usize::from(at > 0), row))
        .collect()
}

/// The **open** group's live cell — the held reads plus, with `with_queue`,
/// the reads leading the live tool queue (running, or waiting their turn in
/// a batch) — or nothing when there is no group to show, or **Verbose
/// output** is on. `pulse` blinks the bullet in the strip; `None` draws it
/// at rest (the permission prompt's still frame, which passes `with_queue:
/// false` because the call it asks about has a cell of its own there).
///
/// The `⎿` hint steps through the group's calls in the order they ran, each
/// value holding the row for [`GROUP_HINT_HOLD`](crate::app::GROUP_HINT_HOLD)
/// ([`App::group_hint`]) — so a batch that resolved within a frame still shows
/// every call's value, its first call first — and between rounds it keeps
/// saying what the group last did.
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
                    HistoryItem::Tool(tool) if is_group_member(tool, app.path_display()) => {
                        Some(tool)
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let queued: Vec<&ToolCall> = if with_queue {
        leading_group_calls(app.tool_queue(), app.path_display()).collect()
    } else {
        Vec::new()
    };
    let paths = app.path_display();
    let mut counts = group_counts(held.iter().chain(queued.iter()).copied(), paths);
    if with_queue {
        // A queued scratchpad write counts the content it carries: the draft
        // it streamed, which the batch handed its cell
        // (docs/write-streaming.md), else its announced arguments. The k-th
        // previewed call pairs with the k-th adopted draft, and the group's
        // calls lead the queue.
        let mut adopted = app.tool_drafts().queued();
        for tool in &queued {
            let draft = previewed_display(&tool.name)
                .then(|| adopted.next())
                .flatten();
            if group_kind(tool, paths) == Some(GroupKind::ScratchpadEdit) {
                counts.lines += pending_edit_lines(tool, draft.map(ToolDraft::content));
            }
        }
        // A scratchpad write still being generated is an edit already, its
        // lines counted as they arrive.
        for draft in app.group_drafts() {
            counts.add_edit(EditLines {
                added: draft.content().lines().count(),
                removed: 0,
            });
        }
    }
    if counts.is_empty() {
        return Vec::new();
    }
    let label = Style::new().fg(tool_name_color());
    let dim = Style::new().fg(tool_dim_color());
    let room = usize::from(width).saturating_sub(cols(TOOL_BULLET));
    let mut spans = vec![bullet_span(tool_running_color(), pulse)];
    spans.extend(fit(summary_spans(counts, true, label, dim), dim, room));
    let mut lines = vec![Line::from(spans)];
    if let Some(item) = app.group_hint(with_queue) {
        let room = usize::from(width).saturating_sub(cols(TOOL_RESULT_PREFIX));
        match hint_text(item, paths) {
            HintText::Path(path) => lines.extend(path_hint_rows(&path, room)),
            HintText::Command(hint) => {
                // The clock outranks the end of the command on a narrow row.
                let clock = group_clock(app, item, with_queue).filter(|clock| cols(clock) < room);
                let room = room - clock.as_deref().map_or(0, cols);
                let mut shown = if cols(&hint) > room {
                    let cut = truncate_cols(&hint, room.saturating_sub(cols(TOOL_HEADER_ELLIPSIS)));
                    format!("{}{TOOL_HEADER_ELLIPSIS}", cut.trim_end())
                } else {
                    hint
                };
                shown.push_str(clock.as_deref().unwrap_or_default());
                lines.push(result_row(0, shown));
            }
        }
    }
    lines
}

/// What the hint row adds while it shows a command that has run
/// [`TOOL_GROUP_CLOCK_DELAY`]: ` (12s)`, then ` (12s · 34 lines)` once it
/// has printed — the reference's collapsed group, so a long search does not
/// look frozen. Only the running command's own value wears it: a row still
/// showing an earlier call never borrows the clock of the one behind it.
fn group_clock(app: &App, shown: GroupHintItem<'_>, with_queue: bool) -> Option<String> {
    let GroupHintItem::Call(shown) = shown else {
        return None;
    };
    let running = app
        .tool_queue()
        .front()
        .filter(|front| with_queue && std::ptr::eq(*front, shown))
        .filter(|front| front.status == ToolStatus::Running && front.name == "Bash")?;
    let elapsed = app
        .command_elapsed()
        .filter(|elapsed| *elapsed >= TOOL_GROUP_CLOCK_DELAY)?;
    let time = crate::app::format_elapsed(elapsed.as_secs());
    Some(match command_display_lines(running).len() {
        0 => format!(" ({time})"),
        1 => format!(" ({time}{TOOL_CLOCK_SEPARATOR}1 line)"),
        printed => format!(" ({time}{TOOL_CLOCK_SEPARATOR}{printed} lines)"),
    })
}
