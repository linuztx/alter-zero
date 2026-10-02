//! The task tools' executor half (`docs/task-tools.md`): run one
//! `taskcreate`/`taskget`/`tasklist`/`taskupdate` call against the shared
//! [`TaskRegistry`] and hand back the split [`ToolOutcome`] — the
//! model-facing result text in `output`, the post-call snapshot on `tasks`
//! (what `run_agent` surfaces as `StreamEvent::TaskCall` so the live
//! checklist tracks the change). [`crate::llm::ask`]'s sibling, minus the
//! blocking: a task op is instant, so there is nothing to wait on.
//!
//! All the semantics — parsing, ids, every result string — live in the pure
//! [`crate::tasks`] module; this is the one-lock adapter.

use super::agent::{GuardNote, TurnGuard};
use super::tools::{ToolCallRequest, ToolOutcome};
use crate::tasks::{TASK_REMINDER_LABEL, TaskGuard, TaskRegistry};

/// Run one task tool call. Never panics, never blocks beyond the registry
/// lock; an argument/id error resolves as a recoverable red outcome whose
/// message the model reads (the snapshot still rides it — the state is
/// unchanged but current either way).
#[must_use]
pub fn run_task_tool(registry: &TaskRegistry, call: &ToolCallRequest) -> ToolOutcome {
    let (result, snapshot) = registry.run_tool(&call.name, &call.arguments);
    match result {
        Ok(output) => ToolOutcome::ok(output).with_tasks(snapshot),
        Err(output) => ToolOutcome::error(output).with_tasks(snapshot),
    }
}

/// The task guard (`docs/task-tools.md`) bound to the shared list — the main
/// backend's [`TurnGuard`]. The policy is the pure [`TaskGuard`]; this hands
/// it the list as it stands at each question, so a reminder always shows
/// what the model's own last call left behind. One per turn: its counts are
/// the turn's.
#[derive(Debug)]
pub struct RegistryGuard {
    registry: TaskRegistry,
    guard: TaskGuard,
}

impl RegistryGuard {
    /// A fresh turn's guard over `registry`.
    #[must_use]
    pub fn new(registry: TaskRegistry) -> Self {
        Self {
            registry,
            guard: TaskGuard::default(),
        }
    }
}

impl TurnGuard for RegistryGuard {
    fn round_ran(&mut self, calls: &[ToolCallRequest], refused: bool) {
        let names: Vec<&str> = calls.iter().map(|call| call.name.as_str()).collect();
        self.guard.round_ran(&names, refused);
    }

    fn before_round(&mut self) -> Option<GuardNote> {
        self.guard
            .round_reminder(&self.registry.snapshot())
            .map(reminder_note)
    }

    fn before_finish(&mut self) -> Option<GuardNote> {
        self.guard
            .finish_reminder(&self.registry.snapshot())
            .map(reminder_note)
    }
}

/// A task reminder's text under its transcript heading.
fn reminder_note(text: String) -> GuardNote {
    GuardNote {
        label: TASK_REMINDER_LABEL.to_string(),
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{TASK_CREATE_TOOL, TASK_UPDATE_TOOL};

    fn call(name: &str, args: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: "c".to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        }
    }

    #[test]
    fn a_create_resolves_ok_with_the_post_call_snapshot() {
        let registry = TaskRegistry::new();
        let outcome = run_task_tool(
            &registry,
            &call(TASK_CREATE_TOOL, r#"{"subject":"a","description":"d"}"#),
        );
        assert!(outcome.ok);
        assert_eq!(outcome.output, "Task #1 created successfully: a");
        let tasks = outcome.tasks.expect("the snapshot rides the outcome");
        assert_eq!(tasks.tasks().len(), 1);
        assert!(outcome.context.is_none(), "display and result are one text");
        assert!(outcome.background.is_none());
    }

    #[test]
    fn the_guard_reminds_against_the_shared_list_as_it_stands() {
        use crate::llm::agent::TurnGuard;
        let registry = TaskRegistry::new();
        let mut guard = RegistryGuard::new(registry.clone());
        let create = call(
            TASK_CREATE_TOOL,
            r#"{"subject":"Serve the page","description":"d"}"#,
        );
        let _ = run_task_tool(&registry, &create);
        guard.round_ran(std::slice::from_ref(&create), false);
        assert_eq!(guard.before_round(), None, "nothing stale yet");
        guard.round_ran(&[call("bash", r#"{"command":"node server.js"}"#)], false);
        let note = guard
            .before_round()
            .expect("a round of work, nothing in progress");
        assert_eq!(note.label, "Task reminder");
        assert!(
            note.text.contains("#1 [pending] Serve the page"),
            "{}",
            note.text
        );
    }

    #[test]
    fn the_end_of_turn_guard_reads_the_latest_list() {
        use crate::llm::agent::TurnGuard;
        let registry = TaskRegistry::new();
        let create = call(TASK_CREATE_TOOL, r#"{"subject":"a","description":"d"}"#);
        let _ = run_task_tool(&registry, &create);
        let mut guard = RegistryGuard::new(registry.clone());
        guard.round_ran(std::slice::from_ref(&create), false);
        // The model finished the task before answering: nothing to say.
        let done = call(TASK_UPDATE_TOOL, r#"{"taskId":"1","status":"completed"}"#);
        let _ = run_task_tool(&registry, &done);
        guard.round_ran(std::slice::from_ref(&done), false);
        assert_eq!(guard.before_finish(), None);

        // Another turn's guard over a list left open: reminded.
        let registry = TaskRegistry::new();
        let _ = run_task_tool(&registry, &create);
        let mut guard = RegistryGuard::new(registry);
        guard.round_ran(std::slice::from_ref(&create), false);
        let note = guard.before_finish().expect("open at the end");
        assert!(note.text.contains("#1 [pending] a"), "{}", note.text);
        assert_eq!(guard.before_finish(), None, "not again without more work");
    }

    #[test]
    fn an_unknown_id_resolves_red_with_the_unchanged_snapshot() {
        let registry = TaskRegistry::new();
        let outcome = run_task_tool(
            &registry,
            &call(TASK_UPDATE_TOOL, r#"{"taskId":"9","status":"completed"}"#),
        );
        assert!(!outcome.ok);
        assert_eq!(outcome.output, "Task #9 not found");
        assert!(outcome.tasks.expect("snapshot rides errors too").is_empty());
    }
}
