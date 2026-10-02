//! The task list's guard (`docs/task-tools.md`): a model working with open
//! tasks it isn't keeping current is shown the list in a short
//! `<system-reminder>` at the next round boundary, and once more before its
//! turn ends if it is leaving tasks open.
//!
//! Claude Code's task reminder, extended for the models that need it. The
//! reference only waits for ten rounds without a task call, and a small model
//! finishes the whole job in fewer — measured with gpt-oss:120b, one
//! `taskcreate`, eleven rounds of work and the task never marked in progress.
//! So beside that wait (the stale list) the guard catches the lapse the
//! moment it happens (work going on with nothing in progress) and the moment
//! that would freeze the checklist wrong (the turn's end).
//!
//! Pure: the agent loop feeds it each round's calls and the current list
//! (`llm::agent`), and sends whatever it returns as a user-role note.

use super::{TASK_CREATE_TOOL, TASK_UPDATE_TOOL, TaskStatus, TaskStore, is_task_tool};
use crate::reminder::{REMINDER_CLOSE, REMINDER_OPEN};

/// Rounds of work without a `taskcreate`/`taskupdate` before an in-progress
/// list is reminded, and the fewest rounds between two round reminders — the
/// reference's ten. Shorter nags a model that makes one small call per round
/// while the right task *is* in progress (measured: five rounds of `mkdir`
/// and `npm` on a setup task).
pub const TASK_REMINDER_ROUNDS: usize = 10;

/// The transcript heading a reminder's note is recorded under.
pub const TASK_REMINDER_LABEL: &str = "Task reminder";

/// Why the guard spoke up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskNudge {
    /// Open tasks, none in progress, and the model is working.
    Idle,
    /// A task is in progress, but nothing was created or updated in
    /// [`TASK_REMINDER_ROUNDS`] rounds of work.
    Stale,
    /// The model is ending its turn with tasks still open.
    Closing,
}

/// One turn's guard. Fresh per turn: what it counts is this turn's rounds.
#[derive(Debug, Clone, Default)]
pub struct TaskGuard {
    /// Rounds since the last create/update, counting the round it was in.
    rounds_since_update: usize,
    /// Rounds since the last round reminder: `None` before the first, and
    /// again once the model acts on the list — the gap is for a model
    /// ignoring the reminder, not one answering it.
    rounds_since_reminder: Option<usize>,
    /// A non-task tool ran since the last create/update.
    worked_since_update: bool,
    /// A non-task tool ran this turn.
    worked: bool,
    /// This turn created or updated a task.
    managed: bool,
    /// The closing reminder went out.
    closed: bool,
}

impl TaskGuard {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// One tool round ran: its calls' wire names, in the model's order (the
    /// order counts — work then an update is current, an update then work
    /// has worked since).
    pub fn record_round<'a>(&mut self, calls: impl IntoIterator<Item = &'a str>) {
        for name in calls {
            match name {
                TASK_CREATE_TOOL | TASK_UPDATE_TOOL => {
                    self.rounds_since_update = 0;
                    self.rounds_since_reminder = None;
                    self.worked_since_update = false;
                    self.managed = true;
                }
                // `tasklist`/`taskget` read the list; reading is no work.
                name if is_task_tool(name) => {}
                _ => {
                    self.worked_since_update = true;
                    self.worked = true;
                }
            }
        }
        self.rounds_since_update += 1;
        if let Some(rounds) = &mut self.rounds_since_reminder {
            *rounds += 1;
        }
    }

    /// At a round boundary: the reminder `tasks` needs now, if any. A turn
    /// that has not touched the list hears one at most — the plan may be an
    /// earlier turn's, and the request something else entirely.
    pub fn round_reminder(&mut self, tasks: &TaskStore) -> Option<String> {
        if !self.worked_since_update || !has_open(tasks) {
            return None;
        }
        match self.rounds_since_reminder {
            Some(_) if !self.managed => return None,
            Some(rounds) if rounds < TASK_REMINDER_ROUNDS => return None,
            _ => {}
        }
        let nudge = if !has_status(tasks, TaskStatus::InProgress) {
            TaskNudge::Idle
        } else if self.rounds_since_update >= TASK_REMINDER_ROUNDS {
            TaskNudge::Stale
        } else {
            return None;
        };
        self.rounds_since_reminder = Some(0);
        Some(task_reminder(nudge, tasks))
    }

    /// The model is ending its turn: the reminder to send before it may, if
    /// any — once per turn, when the turn did work and leaves open tasks it
    /// created or updated, or any in progress. Whether its last call was an
    /// update doesn't matter: marking the task just finished says nothing
    /// about the others the same work finished.
    pub fn closing_reminder(&mut self, tasks: &TaskStore) -> Option<String> {
        if self.closed || !self.worked || !has_open(tasks) {
            return None;
        }
        if !self.managed && !has_status(tasks, TaskStatus::InProgress) {
            return None;
        }
        self.closed = true;
        Some(task_reminder(TaskNudge::Closing, tasks))
    }
}

/// The reminder text for `nudge` over `tasks`: what to do, then the list as
/// `tasklist` shows it.
#[must_use]
pub fn task_reminder(nudge: TaskNudge, tasks: &TaskStore) -> String {
    let lead = match nudge {
        TaskNudge::Idle => {
            "None of these tasks is in progress. If you are working on one, \
             mark it in_progress with taskupdate, and mark each one completed \
             as soon as it is done. Do not mention this reminder to the user."
        }
        TaskNudge::Stale => {
            "These tasks have not been updated in a while. Mark finished ones \
             completed and the one you are working on in_progress with \
             taskupdate. Do not mention this reminder to the user."
        }
        TaskNudge::Closing => {
            "You are ending your turn with these tasks still open. Mark each \
             one you finished completed with taskupdate and leave the rest \
             open. Do not repeat your answer or mention this reminder to the \
             user."
        }
    };
    format!(
        "{REMINDER_OPEN}\n{lead}\n\n{}\n{REMINDER_CLOSE}",
        tasks.run_list()
    )
}

/// Does `tasks` hold a task that is not completed?
fn has_open(tasks: &TaskStore) -> bool {
    tasks
        .tasks()
        .iter()
        .any(|task| task.status != TaskStatus::Completed)
}

fn has_status(tasks: &TaskStore, status: TaskStatus) -> bool {
    tasks.tasks().iter().any(|task| task.status == status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{TASK_CREATE_TOOL, TASK_LIST_TOOL, TASK_UPDATE_TOOL};

    /// A store holding `subjects` as tasks #1…, all pending.
    fn plan(subjects: &[&str]) -> TaskStore {
        let mut store = TaskStore::new();
        for subject in subjects {
            store
                .run_create(
                    &serde_json::json!({"subject": subject, "description": "d"}).to_string(),
                )
                .expect("create");
        }
        store
    }

    fn set(store: &mut TaskStore, id: u64, status: &str) {
        store
            .run_update(
                &serde_json::json!({"taskId": id.to_string(), "status": status}).to_string(),
            )
            .expect("update");
    }

    #[test]
    fn a_plan_no_one_has_worked_on_yet_is_left_alone() {
        // Creating the tasks is the plan; nothing to remind until work starts.
        let tasks = plan(&["Write the page", "Serve it on port 3000"]);
        let mut guard = TaskGuard::new();
        assert_eq!(guard.round_reminder(&tasks), None, "the turn's first round");
        guard.record_round([TASK_CREATE_TOOL, TASK_CREATE_TOOL]);
        assert_eq!(guard.round_reminder(&tasks), None);
        guard.record_round([TASK_LIST_TOOL]);
        assert_eq!(
            guard.round_reminder(&tasks),
            None,
            "reading the list is no work"
        );
        assert_eq!(guard.closing_reminder(&tasks), None, "a plan-only turn");
    }

    #[test]
    fn work_with_no_task_in_progress_is_reminded_at_the_next_round() {
        // Measured with gpt-oss:120b: one taskcreate, eleven rounds of work,
        // the task never marked in progress.
        let tasks = plan(&["Write the page", "Serve it on port 3000"]);
        let mut guard = TaskGuard::new();
        guard.record_round([TASK_CREATE_TOOL, TASK_CREATE_TOOL]);
        guard.record_round(["write"]);
        assert_eq!(
            guard.round_reminder(&tasks),
            Some(task_reminder(TaskNudge::Idle, &tasks))
        );
    }

    #[test]
    fn a_round_reminder_waits_its_gap_before_repeating() {
        let tasks = plan(&["Write the page"]);
        let mut guard = TaskGuard::new();
        guard.record_round([TASK_CREATE_TOOL]);
        guard.record_round(["write"]);
        assert!(guard.round_reminder(&tasks).is_some());
        for _ in 1..TASK_REMINDER_ROUNDS {
            guard.record_round(["bash"]);
            assert_eq!(guard.round_reminder(&tasks), None, "inside the gap");
        }
        guard.record_round(["bash"]);
        assert!(guard.round_reminder(&tasks).is_some(), "the gap has passed");
    }

    #[test]
    fn an_in_progress_task_is_reminded_once_it_goes_stale() {
        let mut tasks = plan(&["Write the page", "Serve it on port 3000"]);
        set(&mut tasks, 1, "in_progress");
        let mut guard = TaskGuard::new();
        guard.record_round([TASK_UPDATE_TOOL, "write"]);
        for _ in 1..TASK_REMINDER_ROUNDS {
            assert_eq!(guard.round_reminder(&tasks), None, "still current");
            guard.record_round(["bash"]);
        }
        assert_eq!(
            guard.round_reminder(&tasks),
            Some(task_reminder(TaskNudge::Stale, &tasks))
        );
    }

    #[test]
    fn an_update_after_the_work_keeps_the_list_current() {
        // The order inside a round counts: work then an update is current;
        // an update then work has worked since.
        let mut tasks = plan(&["Write the page", "Serve it on port 3000"]);
        set(&mut tasks, 1, "completed");
        let mut guard = TaskGuard::new();
        guard.record_round(["write", TASK_UPDATE_TOOL]);
        assert_eq!(guard.round_reminder(&tasks), None);
        let mut guard = TaskGuard::new();
        guard.record_round([TASK_UPDATE_TOOL, "write"]);
        assert!(
            guard.round_reminder(&tasks).is_some(),
            "#2 open, none in progress"
        );
    }

    #[test]
    fn a_list_with_nothing_open_needs_no_reminder() {
        let mut tasks = plan(&["Write the page"]);
        set(&mut tasks, 1, "completed");
        let mut guard = TaskGuard::new();
        guard.record_round([TASK_CREATE_TOOL]);
        guard.record_round(["write"]);
        assert_eq!(guard.round_reminder(&tasks), None);
        assert_eq!(guard.closing_reminder(&tasks), None);
        let mut guard = TaskGuard::new();
        guard.record_round(["write"]);
        assert_eq!(
            guard.round_reminder(&TaskStore::new()),
            None,
            "no list at all"
        );
        assert_eq!(guard.closing_reminder(&TaskStore::new()), None);
    }

    #[test]
    fn ending_a_turn_with_worked_on_tasks_open_is_reminded_once() {
        let tasks = plan(&["Write the page", "Serve it on port 3000"]);
        let mut guard = TaskGuard::new();
        guard.record_round([TASK_CREATE_TOOL, TASK_CREATE_TOOL]);
        guard.record_round(["write"]);
        guard.record_round(["bash"]);
        assert_eq!(
            guard.closing_reminder(&tasks),
            Some(task_reminder(TaskNudge::Closing, &tasks))
        );
        guard.record_round(["bash"]);
        assert_eq!(guard.closing_reminder(&tasks), None, "once per turn");
    }

    #[test]
    fn a_turn_that_left_an_earlier_plan_alone_ends_without_one() {
        // A plan from an earlier turn, nothing in progress, and this turn
        // doing something else: its end is no time to nag about the plan.
        let mut tasks = plan(&["Review the page with the user"]);
        let mut guard = TaskGuard::new();
        guard.record_round(["edit"]);
        assert_eq!(guard.closing_reminder(&tasks), None);
        // One left in progress is this turn's business, whoever started it.
        set(&mut tasks, 1, "in_progress");
        assert_eq!(
            guard.closing_reminder(&tasks),
            Some(task_reminder(TaskNudge::Closing, &tasks))
        );
    }

    #[test]
    fn a_model_that_acts_on_a_reminder_is_reminded_again_at_once() {
        // The gap is for a model ignoring the reminder; one that answers it
        // by touching the list earns a fresh slate, so its next lapse — #1
        // completed, work going on, #2 never started — is caught at once.
        let mut tasks = plan(&["Write the page", "Serve it on port 3000"]);
        let mut guard = TaskGuard::new();
        guard.record_round([TASK_CREATE_TOOL, TASK_CREATE_TOOL]);
        guard.record_round(["write"]);
        assert!(guard.round_reminder(&tasks).is_some());
        set(&mut tasks, 1, "completed");
        guard.record_round([TASK_UPDATE_TOOL, "bash"]);
        assert_eq!(
            guard.round_reminder(&tasks),
            Some(task_reminder(TaskNudge::Idle, &tasks))
        );
    }

    #[test]
    fn a_turn_that_never_touches_the_list_hears_one_round_reminder() {
        // An earlier turn's plan and a request that may have nothing to do
        // with it: the model is shown the list once, and not nagged after.
        let tasks = plan(&["Write the page", "Serve it on port 3000"]);
        let mut guard = TaskGuard::new();
        guard.record_round(["read"]);
        assert!(guard.round_reminder(&tasks).is_some());
        for _ in 0..3 * TASK_REMINDER_ROUNDS {
            guard.record_round(["bash"]);
            assert_eq!(guard.round_reminder(&tasks), None);
        }
    }

    #[test]
    fn a_turn_ending_on_an_update_is_still_reminded_of_what_it_left_open() {
        // #1 started, the whole job done, #1 completed as the last call —
        // #2's work was done too, and only the closing reminder says so.
        let mut tasks = plan(&["Write the page", "Serve it on port 3000"]);
        let mut guard = TaskGuard::new();
        guard.record_round([TASK_CREATE_TOOL, TASK_CREATE_TOOL]);
        set(&mut tasks, 1, "in_progress");
        guard.record_round([TASK_UPDATE_TOOL]);
        guard.record_round(["write"]);
        guard.record_round(["bash"]);
        set(&mut tasks, 1, "completed");
        guard.record_round([TASK_UPDATE_TOOL]);
        assert_eq!(
            guard.round_reminder(&tasks),
            None,
            "the list was just updated"
        );
        assert_eq!(
            guard.closing_reminder(&tasks),
            Some(task_reminder(TaskNudge::Closing, &tasks))
        );
    }

    #[test]
    fn the_reminder_says_what_to_do_over_the_list_tasklist_shows() {
        let mut tasks = plan(&["Write the page", "Serve it on port 3000"]);
        set(&mut tasks, 1, "in_progress");
        assert_eq!(
            task_reminder(TaskNudge::Idle, &tasks),
            "<system-reminder>\n\
             None of these tasks is in progress. If you are working on one, mark \
             it in_progress with taskupdate, and mark each one completed as soon \
             as it is done. Do not mention this reminder to the user.\n\n\
             #1 [in_progress] Write the page\n\
             #2 [pending] Serve it on port 3000\n\
             </system-reminder>"
        );
        assert!(task_reminder(TaskNudge::Stale, &tasks).starts_with(
            "<system-reminder>\n\
                 These tasks have not been updated in a while. Mark finished ones \
                 completed and the one you are working on in_progress with \
                 taskupdate. Do not mention this reminder to the user.\n\n#1 "
        ));
        assert!(task_reminder(TaskNudge::Closing, &tasks).starts_with(
            "<system-reminder>\n\
                 You are ending your turn with these tasks still open. Mark each \
                 one you finished completed with taskupdate and leave the rest \
                 open. Do not repeat your answer or mention this reminder to the \
                 user.\n\n#1 "
        ));
    }
}
