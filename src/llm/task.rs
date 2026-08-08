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

use super::tools::{ToolCallRequest, ToolOutcome};
use crate::tasks::TaskRegistry;

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
