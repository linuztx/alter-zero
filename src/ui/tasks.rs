//! The task tools' live checklist (`docs/task-tools.md`): the `⎿ ◻ subject`
//! rows rendered directly **under the status line** while a turn runs — the
//! calls themselves commit no cells, so this strip block is the whole
//! display. Claude Code's task list, in the tool gutter.
//!
//! ```text
//! (  ●•· ) Setting up project structure… (25s · esc to interrupt)
//!   ⎿  ◼ Set up project structure
//!      ◻ Write core logic › blocked by #1
//! ```

use super::layout::strip_has_status;
use super::theme::*;
use super::wrap::{cols, truncate_cols};
use super::*;
use crate::tasks::{Task, TaskStatus, TaskStore};

/// How many strip rows the checklist takes for `app` at `width` — 0 unless a
/// (main-screen) turn is active and the list is non-empty; else exactly
/// `task_lines`'s count. The height side of the pair `render_strip` draws,
/// threaded through `live_height`/`live_layout`/`cursor_position` like the
/// queued rows so the box and cursor stay seated.
#[must_use]
pub fn task_rows(app: &App, width: u16) -> u16 {
    u16::try_from(task_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// The checklist rows for `app` at `width` — empty unless the **main**
/// screen's turn is active ([`strip_has_status`]; an agent session view shows
/// the agent's stream, not the main list) and there are tasks. Idle, the
/// strip collapses as always: the checklist is a turn-time display.
pub(super) fn task_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    if app.viewed_agent().is_some() || !strip_has_status(app) || app.tasks().is_empty() {
        return Vec::new();
    }
    checklist_lines(app.tasks(), width)
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
        .map(|(i, task)| task_row(store, task, i, width))
        .collect();
    if !hidden.is_empty() {
        lines.push(hidden_summary_row(&hidden, lines.len()));
    }
    lines
}

/// The dim `… +2 in progress, 3 pending, 1 completed` row folding the tasks
/// past the cap (only the statuses present are named).
fn hidden_summary_row(hidden: &[&Task], index: usize) -> Line<'static> {
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
    let mut line = gutter_prefix(index);
    line.push(Span::styled(
        format!("… +{}", parts.join(", ")),
        Style::new().fg(TOOL_DIM_COLOR),
    ));
    Line::from(line)
}

/// The `⎿` corner on the first row, its blank indent on every later one —
/// the tool cell's gutter geometry ([`TOOL_RESULT_PREFIX`]), dim.
fn gutter_prefix(index: usize) -> Vec<Span<'static>> {
    let prefix = if index == 0 {
        TOOL_RESULT_PREFIX.to_string()
    } else {
        " ".repeat(cols(TOOL_RESULT_PREFIX))
    };
    vec![Span::styled(prefix, Style::new().fg(TOOL_DIM_COLOR))]
}

/// One task's row: gutter, status glyph, subject, and — for a task whose
/// open blockers exist — the dim `› blocked by #1, #2` suffix.
fn task_row(store: &TaskStore, task: &Task, index: usize, width: u16) -> Line<'static> {
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
    let fixed = cols(TOOL_RESULT_PREFIX) + cols(glyph) + 1 + cols(&suffix);
    let avail = usize::from(width).saturating_sub(fixed).max(1);
    let subject = if cols(&task.subject) > avail {
        format!("{}…", truncate_cols(&task.subject, avail.saturating_sub(1)))
    } else {
        task.subject.clone()
    };
    let mut spans = gutter_prefix(index);
    spans.push(Span::styled(format!("{glyph} "), glyph_style));
    spans.push(Span::styled(subject, subject_style));
    if !suffix.is_empty() {
        spans.push(Span::styled(suffix, Style::new().fg(TOOL_DIM_COLOR)));
    }
    Line::from(spans)
}
