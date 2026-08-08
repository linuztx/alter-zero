//! The task tools' live checklist (`docs/task-tools.md`) — the calls
//! themselves commit no cells, so this strip block is their whole display.
//! It has two dresses, and which one shows is simply whether a turn is
//! running:
//!
//! **In a turn**, the rows hang off the spinner in the tool gutter, so the
//! plan reads as what the model is working through right now:
//!
//! ```text
//! (  ●•· ) Setting up project structure… (25s · esc to interrupt)
//!   ⎿  ◼ Set up project structure
//!      ◻ Write core logic › blocked by #1
//! ```
//!
//! **At rest**, the same rows sit above the composer under a standalone
//! count line (Claude Code's `isStandalone` task list), so a plan with work
//! left is visible while the user decides what to say next — there is no
//! spinner for it to hang from:
//!
//! ```text
//!   1 tasks (0 done, 1 open)
//!   ◻ Review demo output with user
//! ```
//!
//! A **finished** plan appears in neither: it retires whole at the turn
//! boundary (`App::retire_finished_tasks`), so nothing here has to remember
//! that it is over.

use super::layout::strip_has_status;
use super::theme::*;
use super::wrap::{cols, truncate_cols};
use super::*;
use crate::tasks::{Task, TaskStatus, TaskStore};

/// How many strip rows the checklist takes for `app` at `width` — 0 when
/// there is no list to show, else exactly `task_lines`'s count (the in-turn
/// rows, or the idle block's count line + rows). The height side of the pair
/// `render_strip` draws, threaded through
/// `live_height`/`live_layout`/`cursor_position` like the queued rows so the
/// box and cursor stay seated.
#[must_use]
pub fn task_rows(app: &App, width: u16) -> u16 {
    u16::try_from(task_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// The checklist rows for `app` at `width`: the gutter rows while the main
/// turn runs, the standalone count line + rows at rest, and nothing when the
/// list is empty — which, thanks to the turn-boundary retirement, is exactly
/// the state a finished plan leaves behind.
///
/// An **agent session view** shows that agent's own stream, never the main
/// list, so it renders nothing here either.
pub(super) fn task_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    if app.viewed_agent().is_some() || app.tasks().is_empty() {
        return Vec::new();
    }
    if strip_has_status(app) {
        return checklist_lines(app.tasks(), width);
    }
    idle_task_lines(app.tasks(), width)
}

/// The **standalone** block shown at rest: Claude Code's dim count line —
/// `1 tasks (0 done, 1 open)`, the numbers bold, an `N in progress` clause
/// between them when any task is running — over the same rows the in-turn
/// checklist draws, indented to the composer's own inset instead of hanging
/// from a `⎿` gutter that has no spinner above it.
#[must_use]
pub fn idle_task_lines(store: &TaskStore, width: u16) -> Vec<Line<'static>> {
    if store.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![task_count_line(store)];
    lines.extend(task_rows_lines(store, width, TASK_IDLE_INDENT));
    lines
}

/// The count line: `{total} tasks ({done} done[, {n} in progress], {open}
/// open)` — dim, with the numbers bold (Claude Code's standalone header).
/// Never pluralized ("1 tasks"), matching the reference.
fn task_count_line(store: &TaskStore) -> Line<'static> {
    let counts = store.counts();
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let bold = dim.add_modifier(Modifier::BOLD);
    let mut spans = vec![
        Span::styled(TASK_IDLE_INDENT.to_string(), dim),
        Span::styled(counts.total.to_string(), bold),
        Span::styled(" tasks (".to_string(), dim),
        Span::styled(counts.completed.to_string(), bold),
        Span::styled(" done, ".to_string(), dim),
    ];
    if counts.in_progress > 0 {
        spans.push(Span::styled(counts.in_progress.to_string(), bold));
        spans.push(Span::styled(" in progress, ".to_string(), dim));
    }
    spans.push(Span::styled(counts.open.to_string(), bold));
    spans.push(Span::styled(" open)".to_string(), dim));
    Line::from(spans)
}

/// One task's place in the truncation ordering (Claude Code's priority when
/// the list outgrows [`TASK_MAX_ROWS`]): what is happening now first, then
/// what could happen next, then what is stuck, then what is done.
fn priority(store: &TaskStore, task: &Task) -> usize {
    match task.status {
        TaskStatus::InProgress => 0,
        TaskStatus::Pending if store.open_blockers(task).is_empty() => 1,
        TaskStatus::Pending => 2,
        TaskStatus::Completed => 3,
    }
}

/// The checklist for `store`: one `⎿`-gutter row per task — `◻` pending, `◼`
/// in progress (cyan glyph, bold subject), `✔` completed (green glyph, dim
/// struck-through subject) — with a dim `› blocked by #1` suffix naming a
/// task's **open** blockers. Subjects truncate to one row (`…`). Past
/// `TASK_MAX_ROWS` the list keeps the highest-priority tasks and folds the
/// rest into a dim `… +N pending` summary row.
#[must_use]
pub fn checklist_lines(store: &TaskStore, width: u16) -> Vec<Line<'static>> {
    task_rows_lines(store, width, TOOL_RESULT_PREFIX)
}

/// The task rows themselves, under `prefix` — the `⎿` gutter in a turn, the
/// plain indent at rest. Shared by both dresses so the glyphs, styling,
/// truncation and the `TASK_MAX_ROWS` fold can never differ between them.
fn task_rows_lines(store: &TaskStore, width: u16, prefix: &'static str) -> Vec<Line<'static>> {
    let tasks = store.tasks();
    let (visible, hidden): (Vec<&Task>, Vec<&Task>) = if tasks.len() <= TASK_MAX_ROWS {
        (tasks.iter().collect(), Vec::new())
    } else {
        let mut ordered: Vec<&Task> = tasks.iter().collect();
        // Stable, so equal priorities keep id order.
        ordered.sort_by_key(|task| priority(store, task));
        let hidden = ordered.split_off(TASK_MAX_ROWS);
        (ordered, hidden)
    };
    let mut lines: Vec<Line<'static>> = visible
        .iter()
        .enumerate()
        .map(|(i, task)| task_row(store, task, i, width, prefix))
        .collect();
    if !hidden.is_empty() {
        lines.push(hidden_summary_row(&hidden, lines.len(), prefix));
    }
    lines
}

/// The dim `… +2 in progress, 3 pending, 1 completed` row folding the tasks
/// past the cap (only the statuses present are named).
fn hidden_summary_row(hidden: &[&Task], index: usize, prefix: &'static str) -> Line<'static> {
    let count = |status: TaskStatus| hidden.iter().filter(|t| t.status == status).count();
    let mut parts = Vec::new();
    for (status, label) in [
        (TaskStatus::InProgress, "in progress"),
        (TaskStatus::Pending, "pending"),
        (TaskStatus::Completed, "completed"),
    ] {
        let n = count(status);
        if n > 0 {
            parts.push(format!("{n} {label}"));
        }
    }
    let mut line = gutter_prefix(index, prefix);
    line.push(Span::styled(
        format!("… +{}", parts.join(", ")),
        Style::new().fg(TOOL_DIM_COLOR),
    ));
    Line::from(line)
}

/// The row's leading prefix, dim: `prefix` itself on the first row and its
/// blank width on every later one — the `⎿` corner's geometry in a turn
/// ([`TOOL_RESULT_PREFIX`]), a plain inset at rest ([`TASK_IDLE_INDENT`],
/// which repeats identically since it is already blank).
fn gutter_prefix(index: usize, prefix: &'static str) -> Vec<Span<'static>> {
    let lead = if index == 0 {
        prefix.to_string()
    } else {
        " ".repeat(cols(prefix))
    };
    vec![Span::styled(lead, Style::new().fg(TOOL_DIM_COLOR))]
}

/// One task's row: gutter, status glyph, subject, and — for a task whose
/// open blockers exist — the dim `› blocked by #1, #2` suffix.
fn task_row(
    store: &TaskStore,
    task: &Task,
    index: usize,
    width: u16,
    prefix: &'static str,
) -> Line<'static> {
    let blockers = store.open_blockers(task);
    let suffix = if blockers.is_empty() {
        String::new()
    } else {
        let ids: Vec<String> = blockers.iter().map(|id| format!("#{id}")).collect();
        format!(" {TASK_BLOCKED_MARKER} blocked by {}", ids.join(", "))
    };
    let (glyph, glyph_style, subject_style) = match task.status {
        TaskStatus::Pending if !blockers.is_empty() => (
            TASK_PENDING_GLYPH,
            Style::new().fg(TOOL_DIM_COLOR),
            Style::new().fg(TOOL_DIM_COLOR),
        ),
        TaskStatus::Pending => (TASK_PENDING_GLYPH, Style::new(), Style::new()),
        TaskStatus::InProgress => (
            TASK_IN_PROGRESS_GLYPH,
            Style::new().fg(TASK_IN_PROGRESS_COLOR),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        TaskStatus::Completed => (
            TASK_COMPLETED_GLYPH,
            Style::new().fg(TASK_COMPLETED_COLOR),
            Style::new()
                .fg(TOOL_DIM_COLOR)
                .add_modifier(Modifier::CROSSED_OUT),
        ),
    };
    // One row per task: the subject truncates (`…`) so the glyph, subject and
    // suffix always fit the width — Claude Code's single-line rows.
    let fixed = cols(prefix) + cols(glyph) + 1 + cols(&suffix);
    let avail = usize::from(width).saturating_sub(fixed).max(1);
    let subject = if cols(&task.subject) > avail {
        format!("{}…", truncate_cols(&task.subject, avail.saturating_sub(1)))
    } else {
        task.subject.clone()
    };
    let mut spans = gutter_prefix(index, prefix);
    spans.push(Span::styled(format!("{glyph} "), glyph_style));
    spans.push(Span::styled(subject, subject_style));
    if !suffix.is_empty() {
        spans.push(Span::styled(suffix, Style::new().fg(TOOL_DIM_COLOR)));
    }
    Line::from(spans)
}
