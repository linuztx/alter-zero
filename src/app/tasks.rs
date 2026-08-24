//! The task tools' app-side bookkeeping (`docs/task-tools.md`): the hidden
//! [`HistoryItem::TaskCall`] record each resolved call appends, the live
//! checklist snapshot the strip renders under the status line, and the
//! restores that keep both in step with every history rewind.

use super::*;

/// One resolved task tool call — [`ToolCall`]'s cell-less sibling. It renders
/// **nothing** inline (Claude Code hides these calls; the live checklist is
/// their display) but is recorded for everything else: the Ctrl+O transcript
/// expands it as an ordinary tool cell ([`TaskCallRecord::as_tool_call`]),
/// the derived context replays it natively so the model keeps its memory of
/// the call, the rollout round-trips it, and the **post-call snapshot** it
/// carries is what `/resume` and the Esc-Esc backtrack restore the list from
/// (the last record before the cut holds the state the conversation had
/// there). See `docs/task-tools.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCallRecord {
    /// The display name (`TaskCreate` …) — lowercased by the context replay
    /// back to the wire name.
    pub name: String,
    /// The one-line args summary the Ctrl+O header shows
    /// (`llm::tools::summarize_call`) — `#1 → completed`, a create's
    /// subject. Lossy by design: it is a header.
    pub args: String,
    /// The **raw JSON arguments** the model sent, kept beside the summary so
    /// [`crate::context::context_messages`] can replay the call as it was
    /// made. Without it a later turn sees `taskupdate {}` over a result line
    /// and has lost *which* change it asked for — its own subjects,
    /// descriptions and dependency wiring drop out of the conversation.
    /// Empty only for a record written before the field existed (an old
    /// rollout), which replays as `{}` exactly as it used to.
    pub arguments: String,
    /// The model-facing result text (`Task #1 created successfully: …`).
    pub output: String,
    /// Whether the op succeeded (an unknown id, a bad value → red in Ctrl+O).
    pub ok: bool,
    /// Wall-clock stamp of when the call resolved (the [`ToolCall`] rule:
    /// recorded, never displayed).
    pub timestamp: String,
    /// The task list **after** this call ran.
    pub tasks: crate::tasks::TaskStore,
}

impl TaskCallRecord {
    /// The record as an ordinary [`ToolCall`] — what the Ctrl+O transcript
    /// renders (`ui::tool_full_lines`), so the expanded cell needs no second
    /// renderer.
    #[must_use]
    pub fn as_tool_call(&self) -> ToolCall {
        ToolCall {
            name: self.name.clone(),
            args: self.args.clone(),
            status: if self.ok {
                ToolStatus::Ok
            } else {
                ToolStatus::Failed
            },
            output: self.output.clone(),
            timestamp: self.timestamp.clone(),
            shell: false,
            truncated: false,
            context_output: None,
            arguments: String::new(),
            approval_note: None,
            batch: None,
        }
    }
}

impl App {
    /// Record one resolved task tool call (the boundary's handler for
    /// [`crate::stream::StreamEvent::TaskCall`]): append the hidden history
    /// record, install the post-call snapshot as the live checklist, and
    /// charge the result text to the token tally exactly like a visible tool
    /// result (`↑` — it is uploaded back next round).
    pub fn record_task_call(
        &mut self,
        name: &str,
        args: &str,
        arguments: &str,
        output: &str,
        ok: bool,
        tasks: crate::tasks::TaskStore,
    ) {
        if let Some(turn) = self.status.as_mut() {
            turn.tokens += count_tokens(output);
            turn.arrow = TokenArrow::Up;
        }
        self.history.push(HistoryItem::TaskCall(TaskCallRecord {
            name: name.to_string(),
            args: args.to_string(),
            arguments: arguments.to_string(),
            output: output.to_string(),
            ok,
            timestamp: self.now_stamp(),
            tasks: tasks.clone(),
        }));
        self.tasks = tasks;
    }

    /// **Retire a finished plan** at the turn boundary: once every task is
    /// completed the list has done its job, so it is dropped whole — the
    /// checklist stops showing *and stays gone*, and the next plan the model
    /// starts is genuinely new rather than the old ticks with a fresh row
    /// appended (the user-reported reappearance). The all-green moment is
    /// still seen: the sweep runs at the **end** of the turn that completed
    /// it, so those rows stand under the spinner until then.
    ///
    /// A plan with any work left is untouched — it rides every later turn,
    /// the reference transcript's "continue" flow. Returns whether anything
    /// was retired, so the boundary knows to re-sync the shared registry
    /// (the model's next `tasklist` must agree with the strip). See
    /// `docs/task-tools.md`.
    pub fn retire_finished_tasks(&mut self) -> bool {
        self.tasks.retire_if_finished()
    }

    /// The live task list — what the checklist under the status line renders
    /// and the boundary syncs the shared registry from after a rewind.
    #[must_use]
    pub const fn tasks(&self) -> &crate::tasks::TaskStore {
        &self.tasks
    }

    /// The spinner-verb override while some task is in progress: the first
    /// such task's `activeForm` (or its subject) — Claude Code's rule — else
    /// `None` and the turn's own verb stands. Derived, never stored, so
    /// completing the task snaps the verb back mid-turn
    /// (`ui::status_line_with_verb`).
    #[must_use]
    pub fn task_verb(&self) -> Option<&str> {
        self.tasks.running_form()
    }

    /// Re-derive the checklist after a **history rewind** (a `/resume` load,
    /// a backtrack truncation): the last [`HistoryItem::TaskCall`] record
    /// still in history carries the state the conversation had there; none
    /// means no tasks. `/clear` passes through here too via its empty
    /// history.
    ///
    /// The restored snapshot is retired by the same rule as a live one
    /// ([`retire_finished_tasks`](Self::retire_finished_tasks)): a rewind
    /// lands *between* turns, so a plan that was already finished there is
    /// finished now — restoring its ticks would put back exactly what the
    /// retirement removed.
    pub(super) fn reset_tasks_from_history(&mut self) {
        self.tasks = self
            .history
            .iter()
            .rev()
            .find_map(|item| match item {
                HistoryItem::TaskCall(record) => Some(record.tasks.clone()),
                _ => None,
            })
            .unwrap_or_default();
        self.tasks.retire_if_finished();
    }
}
