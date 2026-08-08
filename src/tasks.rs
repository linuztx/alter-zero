//! The task tools' pure model: the store the model's `taskcreate` /
//! `taskget` / `tasklist` / `taskupdate` calls operate on, every result
//! string they return, and the shared registry the executor and the event
//! loop hold together. See `docs/task-tools.md`.
//!
//! Claude Code's structured task list, minus the parameters this single-agent
//! TUI has no use for (`owner`, `metadata`). Everything here is pure and
//! unit-tested — the boundary I/O is one `Arc<Mutex<…>>` away
//! ([`TaskRegistry`], the [`crate::ask::AskGate`] pattern).

use std::sync::{Arc, Mutex};

use serde::Deserialize;

/// The wire names of the four task tools, as offered to the model (lowercase
/// like every other wire name — `crate::llm::tools`'s convention, and what
/// the context replay's lowercasing fallback reproduces from the display
/// names so a replayed call matches the offered spec).
pub const TASK_CREATE_TOOL: &str = "taskcreate";
pub const TASK_GET_TOOL: &str = "taskget";
pub const TASK_LIST_TOOL: &str = "tasklist";
pub const TASK_UPDATE_TOOL: &str = "taskupdate";

/// Every task tool's wire name, in definition order.
pub const TASK_TOOL_NAMES: [&str; 4] = [
    TASK_CREATE_TOOL,
    TASK_GET_TOOL,
    TASK_LIST_TOOL,
    TASK_UPDATE_TOOL,
];

/// Is `name` one of the task tools' wire names? The predicate
/// `llm::agent::run_agent` routes on (task calls resolve as
/// [`crate::stream::StreamEvent::TaskCall`] instead of the visible
/// `ToolStart`/`ToolEnd` pair — `docs/task-tools.md`).
#[must_use]
pub fn is_task_tool(name: &str) -> bool {
    TASK_TOOL_NAMES.contains(&name)
}

/// The CamelCase display name for a task tool's wire name (`taskcreate` →
/// `TaskCreate`), or `None` for anything else — consumed by
/// `llm::tools::display_name`.
#[must_use]
pub fn task_display_name(wire: &str) -> Option<&'static str> {
    match wire {
        TASK_CREATE_TOOL => Some("TaskCreate"),
        TASK_GET_TOOL => Some("TaskGet"),
        TASK_LIST_TOOL => Some("TaskList"),
        TASK_UPDATE_TOOL => Some("TaskUpdate"),
        _ => None,
    }
}

/// A task's lifecycle state. `pending` → `in_progress` → `completed` on the
/// wire (plus the `deleted` pseudo-status that removes the task, handled in
/// [`TaskStore::run_update`], never stored).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
}

impl TaskStatus {
    /// The wire string (`pending` / `in_progress` / `completed`).
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }

    /// Parse a wire string, `None` for anything unknown (including
    /// `deleted`, which is an action, not a state).
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

/// One task: the `#id` the model addresses it by, the subject/description it
/// was created with, the optional present-continuous `activeForm` the spinner
/// wears while it is in progress, its status, and the ids of the tasks that
/// must complete before it (`blocked_by` — the one stored direction; `blocks`
/// is derived by scanning, so the two can never disagree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: u64,
    pub subject: String,
    pub description: String,
    pub active_form: Option<String>,
    pub status: TaskStatus,
    pub blocked_by: Vec<u64>,
}

/// Parsed `taskcreate` arguments (`docs/task-tools.md` — Claude Code's
/// schema minus `metadata`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreateArgs {
    pub subject: String,
    pub description: String,
    #[serde(default, rename = "activeForm")]
    pub active_form: Option<String>,
}

/// Parsed `taskget` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GetArgs {
    #[serde(rename = "taskId")]
    pub task_id: String,
}

/// Parsed `taskupdate` arguments (Claude Code's schema minus `owner` and
/// `metadata`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UpdateArgs {
    #[serde(rename = "taskId")]
    pub task_id: String,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "activeForm")]
    pub active_form: Option<String>,
    /// `pending` / `in_progress` / `completed`, or `deleted` to remove.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, rename = "addBlocks")]
    pub add_blocks: Option<Vec<String>>,
    #[serde(default, rename = "addBlockedBy")]
    pub add_blocked_by: Option<Vec<String>>,
}

/// The tallies the idle summary row shows (`TaskStore::counts`): how many
/// tasks there are, how many are done, how many are running, and how many
/// are still open (everything not completed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TaskCounts {
    pub total: usize,
    pub completed: usize,
    pub in_progress: usize,
    pub open: usize,
}

/// The task list itself — and, cloned, the **snapshot** that rides
/// [`crate::stream::StreamEvent::TaskCall`], sits on `App::tasks` for the
/// strip's checklist, and is recorded on every task-call history item so
/// `/resume` and the backtrack restore the exact state
/// (`docs/task-tools.md`). `next_id` is the high-water mark of ids ever
/// assigned: deletion never frees an id, so `#4` after deleting `#1–3`
/// proves the old tasks are gone rather than renumbered (Claude Code's
/// rule).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskStore {
    tasks: Vec<Task>,
    next_id: u64,
}

impl TaskStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild a store from its parts (the `/resume` load — the
    /// `session` module parses records into tasks and the recorded
    /// high-water mark).
    #[must_use]
    pub fn from_parts(tasks: Vec<Task>, next_id: u64) -> Self {
        let watermark = tasks.iter().map(|t| t.id).max().unwrap_or(0);
        Self {
            tasks,
            next_id: next_id.max(watermark),
        }
    }

    /// The tasks, in id order (ids only ever grow, so creation order is id
    /// order and nothing ever reorders).
    #[must_use]
    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    /// The high-water mark: the highest id ever assigned (0 when none).
    #[must_use]
    pub const fn high_water(&self) -> u64 {
        self.next_id
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// The task with `id`, if it exists.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// The ids of the tasks `task` is blocked by that are still **open** —
    /// present in the store and not completed (a finished or deleted blocker
    /// no longer blocks), ascending. What the checklist's `› blocked by #2`
    /// suffix and `tasklist`'s `[blocked by #2]` clause show.
    #[must_use]
    pub fn open_blockers(&self, task: &Task) -> Vec<u64> {
        let mut ids: Vec<u64> = task
            .blocked_by
            .iter()
            .copied()
            .filter(|&id| {
                self.get(id)
                    .is_some_and(|t| t.status != TaskStatus::Completed)
            })
            .collect();
        ids.sort_unstable();
        ids
    }

    /// The ids of the tasks blocked by `id` (the derived reverse of
    /// `blocked_by`), ascending — `taskget`'s `Blocks:` line.
    #[must_use]
    pub fn blocks_of(&self, id: u64) -> Vec<u64> {
        self.tasks
            .iter()
            .filter(|t| t.blocked_by.contains(&id))
            .map(|t| t.id)
            .collect()
    }

    /// Is the plan **finished** — non-empty with every task completed?
    /// Empty is not "finished" (there was never a plan). What
    /// [`retire_if_finished`](Self::retire_if_finished) acts on.
    #[must_use]
    pub fn all_completed(&self) -> bool {
        !self.tasks.is_empty() && self.tasks.iter().all(|t| t.status == TaskStatus::Completed)
    }

    /// **Retire a finished plan**: once every task is completed the list has
    /// served its purpose, so it is dropped whole at the next turn boundary —
    /// the checklist stops showing *and stays gone*, and the plan the model
    /// starts next is genuinely new rather than the old ticks with a fresh
    /// row appended (`docs/task-tools.md`). A plan with any work left is
    /// untouched.
    ///
    /// The id **high-water mark survives** ([`high_water`](Self::high_water)),
    /// so the next task is `#4` after a retired `#1–3` — the same proof of
    /// continuity deletion gives. Returns whether anything was retired, so
    /// the boundary knows to re-sync the shared registry.
    pub fn retire_if_finished(&mut self) -> bool {
        if !self.all_completed() {
            return false;
        }
        self.tasks.clear();
        true
    }

    /// How many tasks are completed / still open — the idle summary row's
    /// counts (`{total} tasks ({done} done, {open} open)`).
    #[must_use]
    pub fn counts(&self) -> TaskCounts {
        let completed = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .count();
        let in_progress = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::InProgress)
            .count();
        TaskCounts {
            total: self.tasks.len(),
            completed,
            in_progress,
            open: self.tasks.len() - completed,
        }
    }

    /// The spinner verb the list currently wants: the **first** in-progress
    /// task's `activeForm`, falling back to its subject — Claude Code's rule
    /// (`currentTodo.activeForm ?? currentTodo.subject`); `None` when nothing
    /// is in progress (the turn's own verb then stands). See
    /// `docs/task-tools.md`.
    #[must_use]
    pub fn running_form(&self) -> Option<&str> {
        let task = self
            .tasks
            .iter()
            .find(|t| t.status == TaskStatus::InProgress)?;
        Some(task.active_form.as_deref().unwrap_or(&task.subject))
    }

    /// Dispatch one task tool call by wire name. `Ok` is the model-facing
    /// result text, `Err` the recoverable error text (the model reads either).
    ///
    /// # Errors
    /// The model-facing message when the arguments don't parse, an id is
    /// unknown, or a value is invalid.
    pub fn run_tool(&mut self, name: &str, arguments: &str) -> Result<String, String> {
        match name {
            TASK_CREATE_TOOL => self.run_create(arguments),
            TASK_GET_TOOL => self.run_get(arguments),
            TASK_LIST_TOOL => Ok(self.run_list()),
            TASK_UPDATE_TOOL => self.run_update(arguments),
            other => Err(format!("unknown task tool: {other}")),
        }
    }

    /// `taskcreate`: append a pending task under the next id. Result:
    /// `Task #3 created successfully: {subject}` (Claude Code's exact text).
    ///
    /// # Errors
    /// Unparseable arguments or a blank subject.
    pub fn run_create(&mut self, arguments: &str) -> Result<String, String> {
        let args: CreateArgs = parse_args(arguments)?;
        if args.subject.trim().is_empty() {
            return Err("subject must not be empty".to_string());
        }
        let id = self.next_id + 1;
        self.next_id = id;
        self.tasks.push(Task {
            id,
            subject: args.subject.clone(),
            description: args.description,
            active_form: args.active_form.filter(|f| !f.trim().is_empty()),
            status: TaskStatus::Pending,
            blocked_by: Vec::new(),
        });
        Ok(format!("Task #{id} created successfully: {}", args.subject))
    }

    /// `taskget`: the full details —
    /// `Task #3: {subject}` / `Status: …` / `Description: …`, plus
    /// `Blocked by: #1, #2` / `Blocks: #4` when linked (Claude Code's shape).
    ///
    /// # Errors
    /// Unparseable arguments or an unknown id.
    pub fn run_get(&self, arguments: &str) -> Result<String, String> {
        let args: GetArgs = parse_args(arguments)?;
        let id = parse_task_id(&args.task_id)?;
        let task = self.get(id).ok_or_else(|| not_found(&args.task_id))?;
        let mut lines = vec![
            format!("Task #{id}: {}", task.subject),
            format!("Status: {}", task.status.wire()),
            format!("Description: {}", task.description),
        ];
        if !task.blocked_by.is_empty() {
            lines.push(format!("Blocked by: {}", id_list(&task.blocked_by)));
        }
        let blocks = self.blocks_of(id);
        if !blocks.is_empty() {
            lines.push(format!("Blocks: {}", id_list(&blocks)));
        }
        Ok(lines.join("\n"))
    }

    /// `tasklist`: one `#3 [pending] {subject}` line per task (id order),
    /// a `[blocked by #2]` clause naming only the **open** blockers;
    /// `No tasks found` when empty (Claude Code's exact texts).
    #[must_use]
    pub fn run_list(&self) -> String {
        if self.tasks.is_empty() {
            return "No tasks found".to_string();
        }
        self.tasks
            .iter()
            .map(|task| {
                let mut line = format!("#{} [{}] {}", task.id, task.status.wire(), task.subject);
                let blockers = self.open_blockers(task);
                if !blockers.is_empty() {
                    line.push_str(&format!(" [blocked by {}]", id_list(&blockers)));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `taskupdate`: change the named fields (only genuine changes are
    /// reported — `Updated task #3 status, subject`), wire dependencies
    /// (`addBlocks`/`addBlockedBy`), or — `status: "deleted"` — remove the
    /// task permanently, scrubbing it from every other task's `blocked_by`.
    /// Validation is atomic: an unknown id or bad value changes nothing.
    ///
    /// # Errors
    /// Unparseable arguments, an unknown task or dependency id, an invalid
    /// status value, or a self-dependency.
    pub fn run_update(&mut self, arguments: &str) -> Result<String, String> {
        let args: UpdateArgs = parse_args(arguments)?;
        let id = parse_task_id(&args.task_id)?;
        let index = self
            .tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| not_found(&args.task_id))?;

        // Deletion is its own early path: remove + scrub the id everywhere.
        if args.status.as_deref() == Some("deleted") {
            self.tasks.remove(index);
            for task in &mut self.tasks {
                task.blocked_by.retain(|&b| b != id);
            }
            return Ok(format!("Updated task #{id} deleted"));
        }

        // Validate everything before mutating anything, so a bad value can't
        // leave a half-applied update.
        let status = match &args.status {
            Some(s) => Some(TaskStatus::from_wire(s).ok_or_else(|| {
                format!("invalid status: {s} (pending, in_progress, completed, or deleted)")
            })?),
            None => None,
        };
        let blocks_ids = parse_dependency_ids(self, id, args.add_blocks.as_deref())?;
        let blocked_by_ids = parse_dependency_ids(self, id, args.add_blocked_by.as_deref())?;

        let mut fields: Vec<&str> = Vec::new();
        {
            let task = &mut self.tasks[index];
            if let Some(subject) = args.subject.filter(|s| *s != task.subject) {
                task.subject = subject;
                fields.push("subject");
            }
            if let Some(description) = args.description.filter(|d| *d != task.description) {
                task.description = description;
                fields.push("description");
            }
            if let Some(form) = args
                .active_form
                .filter(|f| task.active_form.as_ref() != Some(f))
            {
                task.active_form = Some(form);
                fields.push("activeForm");
            }
            if let Some(status) = status.filter(|s| *s != task.status) {
                task.status = status;
                fields.push("status");
            }
        }
        // addBlocks: this task blocks each target → the target is blocked by us.
        let mut added_blocks = false;
        for target in blocks_ids {
            let target = self
                .tasks
                .iter_mut()
                .find(|t| t.id == target)
                .expect("validated above");
            if !target.blocked_by.contains(&id) {
                target.blocked_by.push(id);
                added_blocks = true;
            }
        }
        if added_blocks {
            fields.push("blocks");
        }
        // addBlockedBy: each named task blocks this one.
        let mut added_blocked_by = false;
        {
            let task = &mut self.tasks[index];
            for blocker in blocked_by_ids {
                if !task.blocked_by.contains(&blocker) {
                    task.blocked_by.push(blocker);
                    added_blocked_by = true;
                }
            }
        }
        if added_blocked_by {
            fields.push("blockedBy");
        }
        Ok(format!("Updated task #{id} {}", fields.join(", "))
            .trim_end()
            .to_string())
    }
}

/// Parse and validate one `addBlocks`/`addBlockedBy` id list against the
/// store: every id must name an existing task and none may be `self_id` (a
/// task cannot depend on itself). Returns the parsed ids (order kept).
fn parse_dependency_ids(
    store: &TaskStore,
    self_id: u64,
    raw: Option<&[String]>,
) -> Result<Vec<u64>, String> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::with_capacity(raw.len());
    for value in raw {
        let id = parse_task_id(value)?;
        if id == self_id {
            return Err(format!("Task #{id} cannot depend on itself"));
        }
        if store.get(id).is_none() {
            return Err(not_found(value));
        }
        ids.push(id);
    }
    Ok(ids)
}

/// `#1, #2, #3` — the id list every clause shares.
fn id_list(ids: &[u64]) -> String {
    ids.iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The not-found text (Claude Code's `Task #{id} not found`), from the raw
/// value the model sent so the message echoes what it said.
fn not_found(raw: &str) -> String {
    let clean = raw.trim().trim_start_matches('#');
    format!("Task #{clean} not found")
}

/// Parse a task id the model sent: `"3"` (the wire shape) — a stray leading
/// `#` is tolerated since the display format prints ids that way. Anything
/// unparseable reads as an unknown task.
fn parse_task_id(raw: &str) -> Result<u64, String> {
    raw.trim()
        .trim_start_matches('#')
        .parse::<u64>()
        .map_err(|_| not_found(raw))
}

/// Parse a tool's raw JSON `arguments`, mapping the serde error to the short
/// model-facing message every tool uses (`llm::tools::parse_args`'s local
/// twin — this module can't depend on `llm`).
fn parse_args<T: for<'de> Deserialize<'de>>(arguments: &str) -> Result<T, String> {
    let trimmed = arguments.trim();
    let text = if trimmed.is_empty() { "{}" } else { trimmed };
    serde_json::from_str(text).map_err(|e| format!("invalid tool arguments: {e}"))
}

/// The shared task list: the executor mutates it on the tool thread while
/// the event loop reads/replaces it — the
/// [`crate::background::BackgroundRegistry`] pattern. Cloneable; every clone
/// shares one store. See `docs/task-tools.md`.
#[derive(Debug, Clone, Default)]
pub struct TaskRegistry {
    inner: Arc<Mutex<TaskStore>>,
}

impl TaskRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Run one task tool call against the shared store, returning the
    /// model-facing result and the **post-call snapshot** (what rides
    /// [`crate::stream::StreamEvent::TaskCall`] to the strip's checklist).
    pub fn run_tool(&self, name: &str, arguments: &str) -> (Result<String, String>, TaskStore) {
        let mut store = self.lock();
        let result = store.run_tool(name, arguments);
        (result, store.clone())
    }

    /// The current state, cloned.
    #[must_use]
    pub fn snapshot(&self) -> TaskStore {
        self.lock().clone()
    }

    /// Replace the whole store — `/clear` (an empty one), a `/resume` load,
    /// or a backtrack rewind (the snapshot at the cut). See
    /// `docs/task-tools.md`.
    pub fn replace(&self, store: TaskStore) {
        *self.lock() = store;
    }

    /// The store, recovering from a poisoned mutex (a panicking holder
    /// leaves the data structurally fine — the [`crate::ask::AskGate`]
    /// posture).
    fn lock(&self) -> std::sync::MutexGuard<'_, TaskStore> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(store: &mut TaskStore, subject: &str) -> String {
        store
            .run_create(&serde_json::json!({"subject": subject, "description": format!("{subject} described")}).to_string())
            .expect("create succeeds")
    }

    #[test]
    fn create_assigns_sequential_ids_and_returns_claude_codes_text() {
        let mut store = TaskStore::new();
        assert_eq!(
            create(&mut store, "Set up project structure"),
            "Task #1 created successfully: Set up project structure"
        );
        assert_eq!(
            create(&mut store, "Write core logic"),
            "Task #2 created successfully: Write core logic"
        );
        let tasks = store.tasks();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, 1);
        assert_eq!(tasks[1].id, 2);
        assert_eq!(tasks[0].status, TaskStatus::Pending, "created pending");
        assert_eq!(tasks[0].description, "Set up project structure described");
    }

    #[test]
    fn create_keeps_active_form_and_drops_a_blank_one() {
        let mut store = TaskStore::new();
        store
            .run_create(r#"{"subject":"Run tests","description":"d","activeForm":"Running tests"}"#)
            .unwrap();
        store
            .run_create(r#"{"subject":"Plain","description":"d","activeForm":"  "}"#)
            .unwrap();
        assert_eq!(
            store.tasks()[0].active_form.as_deref(),
            Some("Running tests")
        );
        assert_eq!(store.tasks()[1].active_form, None, "blank form is no form");
    }

    #[test]
    fn create_rejects_a_blank_subject_and_bad_json() {
        let mut store = TaskStore::new();
        let err = store
            .run_create(r#"{"subject":"  ","description":"d"}"#)
            .unwrap_err();
        assert!(err.contains("subject"), "got {err}");
        let err = store.run_create("not json").unwrap_err();
        assert!(err.contains("invalid tool arguments"), "got {err}");
        assert!(store.is_empty(), "nothing was created");
    }

    #[test]
    fn ids_never_restart_after_deletion() {
        // Claude Code's high-water mark: deleting every task and creating a
        // new one continues the numbering — the proof the old ones are gone
        // rather than renumbered (the reference transcript's #4–7).
        let mut store = TaskStore::new();
        create(&mut store, "a");
        create(&mut store, "b");
        create(&mut store, "c");
        for id in 1..=3 {
            store
                .run_update(&format!(r#"{{"taskId":"{id}","status":"deleted"}}"#))
                .unwrap();
        }
        assert!(store.is_empty());
        assert_eq!(
            create(&mut store, "fresh"),
            "Task #4 created successfully: fresh"
        );
    }

    #[test]
    fn get_formats_the_full_details_with_links() {
        let mut store = TaskStore::new();
        create(&mut store, "Audit auth");
        create(&mut store, "Update login");
        create(&mut store, "Add tests");
        // #3 blocked by #2; #2 blocks #3 (wired from the blocked side).
        store
            .run_update(r#"{"taskId":"3","addBlockedBy":["2"]}"#)
            .unwrap();
        assert_eq!(
            store.run_get(r#"{"taskId":"3"}"#).unwrap(),
            "Task #3: Add tests\nStatus: pending\nDescription: Add tests described\nBlocked by: #2"
        );
        assert_eq!(
            store.run_get(r#"{"taskId":"2"}"#).unwrap(),
            "Task #2: Update login\nStatus: pending\nDescription: Update login described\nBlocks: #3"
        );
    }

    #[test]
    fn get_and_update_report_an_unknown_id() {
        let mut store = TaskStore::new();
        assert_eq!(
            store.run_get(r#"{"taskId":"9"}"#).unwrap_err(),
            "Task #9 not found"
        );
        assert_eq!(
            store
                .run_update(r#"{"taskId":"9","status":"completed"}"#)
                .unwrap_err(),
            "Task #9 not found"
        );
        // A non-numeric id reads as unknown, echoed back.
        assert_eq!(
            store.run_get(r#"{"taskId":"nope"}"#).unwrap_err(),
            "Task #nope not found"
        );
    }

    #[test]
    fn a_hash_prefixed_id_is_tolerated() {
        // The display format prints `#1` everywhere, so a model echoing it
        // back must still land.
        let mut store = TaskStore::new();
        create(&mut store, "a");
        assert!(store.run_get(r##"{"taskId":"#1"}"##).is_ok());
    }

    #[test]
    fn list_shows_status_and_open_blockers_only() {
        let mut store = TaskStore::new();
        create(&mut store, "Set up project structure");
        create(&mut store, "Write core logic");
        create(&mut store, "Add tests");
        store
            .run_update(r#"{"taskId":"2","addBlockedBy":["1"]}"#)
            .unwrap();
        store
            .run_update(r#"{"taskId":"3","addBlockedBy":["2"]}"#)
            .unwrap();
        assert_eq!(
            store.run_list(),
            "#1 [pending] Set up project structure\n\
             #2 [pending] Write core logic [blocked by #1]\n\
             #3 [pending] Add tests [blocked by #2]"
        );
        // Completing #1 clears #2's clause — a finished blocker no longer
        // blocks (Claude Code filters resolved ids).
        store
            .run_update(r#"{"taskId":"1","status":"completed"}"#)
            .unwrap();
        assert_eq!(
            store.run_list(),
            "#1 [completed] Set up project structure\n\
             #2 [pending] Write core logic\n\
             #3 [pending] Add tests [blocked by #2]"
        );
    }

    #[test]
    fn an_empty_list_says_so() {
        assert_eq!(TaskStore::new().run_list(), "No tasks found");
    }

    #[test]
    fn update_reports_only_the_fields_that_changed() {
        let mut store = TaskStore::new();
        create(&mut store, "a");
        assert_eq!(
            store
                .run_update(r#"{"taskId":"1","status":"in_progress"}"#)
                .unwrap(),
            "Updated task #1 status"
        );
        // Same-value writes are not changes (Claude Code's field diff).
        assert_eq!(
            store
                .run_update(r#"{"taskId":"1","status":"in_progress","subject":"a"}"#)
                .unwrap(),
            "Updated task #1"
        );
        assert_eq!(
            store
                .run_update(
                    r#"{"taskId":"1","subject":"b","description":"new","activeForm":"Doing b","status":"completed"}"#
                )
                .unwrap(),
            "Updated task #1 subject, description, activeForm, status"
        );
        let task = store.get(1).unwrap();
        assert_eq!(task.subject, "b");
        assert_eq!(task.description, "new");
        assert_eq!(task.active_form.as_deref(), Some("Doing b"));
        assert_eq!(task.status, TaskStatus::Completed);
    }

    #[test]
    fn update_rejects_an_invalid_status() {
        let mut store = TaskStore::new();
        create(&mut store, "a");
        let err = store
            .run_update(r#"{"taskId":"1","status":"done"}"#)
            .unwrap_err();
        assert!(err.contains("invalid status"), "got {err}");
        assert_eq!(store.get(1).unwrap().status, TaskStatus::Pending);
    }

    #[test]
    fn add_blocks_and_add_blocked_by_wire_the_same_relation() {
        let mut store = TaskStore::new();
        create(&mut store, "a");
        create(&mut store, "b");
        // From the blocker's side: #1 blocks #2.
        assert_eq!(
            store
                .run_update(r#"{"taskId":"1","addBlocks":["2"]}"#)
                .unwrap(),
            "Updated task #1 blocks"
        );
        assert_eq!(store.get(2).unwrap().blocked_by, vec![1]);
        // Re-adding is a no-op, not a duplicate — and not a reported change.
        assert_eq!(
            store
                .run_update(r#"{"taskId":"1","addBlocks":["2"]}"#)
                .unwrap(),
            "Updated task #1"
        );
        assert_eq!(store.get(2).unwrap().blocked_by, vec![1]);
        // From the blocked side: #2 also blocked by a new #3.
        create(&mut store, "c");
        assert_eq!(
            store
                .run_update(r#"{"taskId":"2","addBlockedBy":["3"]}"#)
                .unwrap(),
            "Updated task #2 blockedBy"
        );
        assert_eq!(store.get(2).unwrap().blocked_by, vec![1, 3]);
        assert_eq!(store.blocks_of(3), vec![2]);
    }

    #[test]
    fn dependencies_must_name_a_real_other_task() {
        let mut store = TaskStore::new();
        create(&mut store, "a");
        let err = store
            .run_update(r#"{"taskId":"1","addBlockedBy":["7"]}"#)
            .unwrap_err();
        assert_eq!(err, "Task #7 not found");
        let err = store
            .run_update(r#"{"taskId":"1","addBlockedBy":["1"]}"#)
            .unwrap_err();
        assert!(err.contains("itself"), "got {err}");
        // Atomic: a bad list changed nothing — not even valid siblings or
        // the other fields of the same call.
        create(&mut store, "b");
        let err = store
            .run_update(r#"{"taskId":"1","subject":"renamed","addBlockedBy":["2","9"]}"#)
            .unwrap_err();
        assert_eq!(err, "Task #9 not found");
        assert_eq!(store.get(1).unwrap().subject, "a", "nothing applied");
        assert!(store.get(1).unwrap().blocked_by.is_empty());
    }

    #[test]
    fn deletion_removes_the_task_and_scrubs_references() {
        let mut store = TaskStore::new();
        create(&mut store, "a");
        create(&mut store, "b");
        store
            .run_update(r#"{"taskId":"2","addBlockedBy":["1"]}"#)
            .unwrap();
        assert_eq!(
            store
                .run_update(r#"{"taskId":"1","status":"deleted"}"#)
                .unwrap(),
            "Updated task #1 deleted"
        );
        assert!(store.get(1).is_none());
        assert!(
            store.get(2).unwrap().blocked_by.is_empty(),
            "the deleted blocker is scrubbed"
        );
    }

    #[test]
    fn open_blockers_ignores_completed_and_vanished_ids() {
        let mut store = TaskStore::new();
        create(&mut store, "a");
        create(&mut store, "b");
        create(&mut store, "c");
        store
            .run_update(r#"{"taskId":"3","addBlockedBy":["1","2"]}"#)
            .unwrap();
        store
            .run_update(r#"{"taskId":"1","status":"completed"}"#)
            .unwrap();
        let task = store.get(3).unwrap().clone();
        assert_eq!(store.open_blockers(&task), vec![2]);
    }

    #[test]
    fn a_finished_plan_retires_whole_and_never_renumbers() {
        // The user's report: after every task was ticked, creating a new task
        // brought the old ✔ rows back. A finished plan is dropped at the turn
        // boundary, so the next plan is genuinely new — but the ids keep
        // counting, the proof the old ones are gone rather than reused.
        let mut store = TaskStore::new();
        create(&mut store, "a");
        create(&mut store, "b");
        assert!(!store.retire_if_finished(), "an open plan is untouched");
        assert_eq!(store.tasks().len(), 2);
        store
            .run_update(r#"{"taskId":"1","status":"completed"}"#)
            .unwrap();
        assert!(
            !store.retire_if_finished(),
            "one open task keeps the whole plan"
        );
        store
            .run_update(r#"{"taskId":"2","status":"completed"}"#)
            .unwrap();
        assert!(store.retire_if_finished(), "a finished plan retires");
        assert!(store.is_empty(), "and it is gone, not merely hidden");
        assert!(!store.retire_if_finished(), "an empty list retires nothing");
        assert_eq!(
            create(&mut store, "fresh"),
            "Task #3 created successfully: fresh",
            "the next plan continues the numbering"
        );
        assert_eq!(store.tasks().len(), 1, "the new plan stands alone");
    }

    #[test]
    fn counts_tally_the_summary_rows_numbers() {
        let mut store = store_of_three();
        assert_eq!(
            store.counts(),
            TaskCounts {
                total: 3,
                completed: 0,
                in_progress: 0,
                open: 3
            }
        );
        store
            .run_update(r#"{"taskId":"1","status":"completed"}"#)
            .unwrap();
        store
            .run_update(r#"{"taskId":"2","status":"in_progress"}"#)
            .unwrap();
        assert_eq!(
            store.counts(),
            TaskCounts {
                total: 3,
                completed: 1,
                in_progress: 1,
                // Open counts everything not completed — the in-progress one
                // included, like Claude Code's standalone header.
                open: 2
            }
        );
    }

    /// Three plain pending tasks.
    fn store_of_three() -> TaskStore {
        let mut store = TaskStore::new();
        for subject in ["a", "b", "c"] {
            create(&mut store, subject);
        }
        store
    }

    #[test]
    fn all_completed_means_a_non_empty_fully_ticked_plan() {
        let mut store = TaskStore::new();
        assert!(!store.all_completed(), "no plan is not a finished plan");
        create(&mut store, "a");
        create(&mut store, "b");
        assert!(!store.all_completed());
        store
            .run_update(r#"{"taskId":"1","status":"completed"}"#)
            .unwrap();
        assert!(!store.all_completed(), "one open task keeps the plan open");
        store
            .run_update(r#"{"taskId":"2","status":"completed"}"#)
            .unwrap();
        assert!(store.all_completed());
        // Deleting the last open task can finish a plan too.
        create(&mut store, "c");
        assert!(!store.all_completed());
        store
            .run_update(r#"{"taskId":"3","status":"deleted"}"#)
            .unwrap();
        assert!(store.all_completed());
    }

    #[test]
    fn running_form_is_the_first_in_progress_tasks_label() {
        let mut store = TaskStore::new();
        create(&mut store, "Audit current auth code");
        store
            .run_create(
                r#"{"subject":"Update login endpoint","description":"d","activeForm":"Updating login endpoint"}"#,
            )
            .unwrap();
        store
            .run_create(r#"{"subject":"Update signup endpoint","description":"d"}"#)
            .unwrap();
        assert_eq!(store.running_form(), None, "nothing in progress yet");
        // Mark #2 then #3 in progress: the FIRST in id order wins (the
        // reference transcript shows `Updating login endpoint…` with both
        // running), and it wears its activeForm.
        store
            .run_update(r#"{"taskId":"2","status":"in_progress"}"#)
            .unwrap();
        store
            .run_update(r#"{"taskId":"3","status":"in_progress"}"#)
            .unwrap();
        assert_eq!(store.running_form(), Some("Updating login endpoint"));
        // A task without an activeForm falls back to its subject.
        store
            .run_update(r#"{"taskId":"2","status":"completed"}"#)
            .unwrap();
        assert_eq!(store.running_form(), Some("Update signup endpoint"));
    }

    #[test]
    fn run_tool_dispatches_by_wire_name() {
        let mut store = TaskStore::new();
        assert!(
            store
                .run_tool(TASK_CREATE_TOOL, r#"{"subject":"a","description":"d"}"#)
                .is_ok()
        );
        assert!(store.run_tool(TASK_LIST_TOOL, "").is_ok());
        assert!(store.run_tool(TASK_GET_TOOL, r#"{"taskId":"1"}"#).is_ok());
        assert!(
            store
                .run_tool(TASK_UPDATE_TOOL, r#"{"taskId":"1","status":"completed"}"#)
                .is_ok()
        );
        assert!(store.run_tool("bash", "{}").is_err());
    }

    #[test]
    fn the_wire_names_and_display_names_pair_up() {
        for name in TASK_TOOL_NAMES {
            assert!(is_task_tool(name));
            let display = task_display_name(name).expect("every tool has a display name");
            assert_eq!(
                display.to_ascii_lowercase(),
                name,
                "the context replay's lowercasing fallback must reproduce the wire name"
            );
        }
        assert!(!is_task_tool("bash"));
        assert!(task_display_name("bash").is_none());
    }

    #[test]
    fn from_parts_keeps_the_recorded_high_water_mark() {
        // A resumed store whose last tasks were deleted must not reuse their
        // ids: the recorded mark wins over the highest surviving id.
        let mut store = TaskStore::from_parts(
            vec![Task {
                id: 2,
                subject: "kept".into(),
                description: String::new(),
                active_form: None,
                status: TaskStatus::Pending,
                blocked_by: vec![],
            }],
            7,
        );
        assert_eq!(
            create(&mut store, "next"),
            "Task #8 created successfully: next"
        );
        // …and a mark below the tasks' own ids is corrected upward.
        let mut store = TaskStore::from_parts(store.tasks().to_vec(), 0);
        let text = create(&mut store, "again");
        assert_eq!(text, "Task #9 created successfully: again");
    }

    #[test]
    fn the_registry_shares_one_store_across_clones() {
        let registry = TaskRegistry::new();
        let clone = registry.clone();
        let (result, snapshot) =
            registry.run_tool(TASK_CREATE_TOOL, r#"{"subject":"a","description":"d"}"#);
        assert!(result.is_ok());
        assert_eq!(snapshot.tasks().len(), 1, "the snapshot is post-call");
        assert_eq!(clone.snapshot().tasks().len(), 1);
        clone.replace(TaskStore::new());
        assert!(registry.snapshot().is_empty());
    }
}
