//! The **task reminder** — Claude Code's `task_reminder` attachment, ported
//! for the task tools (`docs/task-tools.md`, *Keeping the model aware*): a
//! `<system-reminder>` the agent loop folds into a running turn when the
//! model has gone a while without a `taskcreate`/`taskupdate`, naming every
//! task and its status, plus the **finish guard** that delivers the same list
//! once more when the model is about to end its turn with open tasks it
//! worked on but never reported.
//!
//! The whole module is pure: a backward scan over the request's own messages
//! (every assistant message is one round, a round carrying a task call or a
//! user message carrying a reminder ends a count — the reference's scan of
//! its transcript) plus the list's snapshot. `llm::agent::run_agent` asks it
//! at every round boundary and at the model's final answer, and records what
//! it sends as the cell-less [`crate::app::HistoryItem::TaskReminder`], so
//! the Ctrl+O transcript shows it, the derived context replays it in place
//! on every later turn — which is what keeps the counts honest across turns
//! and the prompt-cache prefix stable — and a `/resume` restores it.

use super::tools::{ASK_TOOL_NAME, BASH_KILL_TOOL, BASH_LIST_TOOL, BASH_WAIT_TOOL};
use super::{ChatMessage, ContentPart, MessageContent};
use crate::tasks::{TASK_CREATE_TOOL, TASK_UPDATE_TOOL, TaskStore};

/// Rounds the model may go without a `taskcreate`/`taskupdate` before the
/// reminder lands while the list is empty or finished — and the gap kept
/// between two reminders. Claude Code's `TURNS_SINCE_WRITE` and
/// `TURNS_BETWEEN_REMINDERS`, both 10.
pub const TASK_REMINDER_ROUNDS: usize = 10;

/// The same window while the list has **open work** (a task pending or in
/// progress). Shorter, because a task left `pending` while its work happens
/// is exactly the symptom the reminder exists for: a model that marks tasks
/// as it goes never sees it (every update restarts the count), while one
/// that forgot gets the list back after a handful of rounds, not ten.
pub const TASK_REMINDER_OPEN_ROUNDS: usize = 5;

/// The heading over the `#1. [status] subject` rows, when there are any.
pub const TASK_LISTING_HEADING: &str = "Existing tasks:";

/// The stale nag (`prompts/task_reminder.md`): Claude Code's wording, cut to
/// the house length — what to do, the opt-out, the never-mention rule.
const STALE_TEMPLATE: &str = include_str!("../../prompts/task_reminder.md");

/// The finish guard (`prompts/task_finish.md`): reconcile the list, then end
/// the turn without repeating the answer already given.
const FINISH_TEMPLATE: &str = include_str!("../../prompts/task_finish.md");

/// Which reminder is being sent — the two texts, one listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderKind {
    /// The round-boundary nag: the tools have gone unused for a while.
    Stale,
    /// The finish guard: the model is answering with open tasks it worked on.
    Finish,
}

/// How many assistant rounds back the two events are
/// ([`round_counts`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RoundCounts {
    /// Rounds since the last `taskcreate`/`taskupdate` (every round when
    /// there never was one).
    pub since_task_call: usize,
    /// Rounds since the last reminder (every round when there never was one).
    pub since_reminder: usize,
}

/// Scan `messages` backwards, Claude Code's `getTaskReminderTurnCounts`:
/// each assistant message is one round; an assistant message whose calls
/// include a `taskcreate`/`taskupdate` ends the first count before it is
/// counted (the round that managed the list is not a round *since*), a
/// user message carrying a reminder ends the second. Reading the list
/// (`tasklist`/`taskget`) is not managing it.
#[must_use]
pub fn round_counts(messages: &[ChatMessage]) -> RoundCounts {
    let mut counts = RoundCounts::default();
    let mut found_task_call = false;
    let mut found_reminder = false;
    for message in messages.iter().rev() {
        if found_task_call && found_reminder {
            break;
        }
        match message.role.as_str() {
            "assistant" => {
                if !found_task_call
                    && message
                        .tool_calls
                        .iter()
                        .any(|call| is_task_management(&call.function.name))
                {
                    found_task_call = true;
                }
                if !found_task_call {
                    counts.since_task_call += 1;
                }
                if !found_reminder {
                    counts.since_reminder += 1;
                }
            }
            "user" if !found_reminder && is_task_reminder(&text_of(message)) => {
                found_reminder = true;
            }
            _ => {}
        }
    }
    counts
}

/// Is `name` a call that **manages** the list — the two the reference
/// counts (`taskcreate`, `taskupdate`)?
fn is_task_management(name: &str) -> bool {
    name == TASK_CREATE_TOOL || name == TASK_UPDATE_TOOL
}

/// The text a message carries, its parts joined — what the reminder scan
/// reads.
fn text_of(message: &ChatMessage) -> String {
    match &message.content {
        MessageContent::Text(text) => text.clone(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text, .. } => Some(text.as_str()),
                ContentPart::ImageUrl { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Does `text` carry a task reminder — either template's opening sentence?
/// `contains` rather than `starts_with`: the derived context joins adjacent
/// user texts (`context::push_text`), so a replayed reminder can sit behind
/// a background notice in one message.
#[must_use]
pub fn is_task_reminder(text: &str) -> bool {
    [STALE_TEMPLATE, FINISH_TEMPLATE]
        .iter()
        .any(|template| text.contains(opening_sentence(template)))
}

/// A template's first sentence — the marker the scan matches on.
fn opening_sentence(template: &str) -> &str {
    let trimmed = template.trim();
    trimmed.find(". ").map_or(trimmed, |end| &trimmed[..=end])
}

/// The round-boundary reminder, when it is due: the model has gone
/// [`TASK_REMINDER_ROUNDS`] rounds without a `taskcreate`/`taskupdate` —
/// [`TASK_REMINDER_OPEN_ROUNDS`] while the list has open work — and as many
/// since the last reminder. `None` otherwise, which is the common case.
#[must_use]
pub fn stale_reminder(messages: &[ChatMessage], store: &TaskStore) -> Option<String> {
    let window = if store.has_open_work() {
        TASK_REMINDER_OPEN_ROUNDS
    } else {
        TASK_REMINDER_ROUNDS
    };
    let counts = round_counts(messages);
    (counts.since_task_call >= window && counts.since_reminder >= window)
        .then(|| reminder_text(ReminderKind::Stale, store))
}

/// The finish guard's reminder, when the model is answering with a list it
/// owes a status: the list has open work, this turn did some (`worked` —
/// an acting call, [`is_work_call`]), and the round before the answer did
/// not **close** a task ([`last_round_closed_a_task`]). `None` otherwise —
/// an answered question, a plan the model just reconciled, a finished one.
///
/// Closing, not merely touching: the live shape that slipped past a
/// "no task call since the work" rule was a model that built the thing,
/// then created a task, marked it `in_progress` and answered — a round that
/// only *opens* work leaves the list saying running while the model says
/// done, which is exactly what the guard exists to catch. A round that
/// completed or deleted something was the model reconciling, and whatever
/// it left open is its call.
#[must_use]
pub fn finish_reminder(
    messages: &[ChatMessage],
    store: &TaskStore,
    worked: bool,
) -> Option<String> {
    (store.has_open_work() && worked && !last_round_closed_a_task(messages))
        .then(|| reminder_text(ReminderKind::Finish, store))
}

/// Did the most recent assistant round carry a `taskupdate` that set a task
/// `completed` or `deleted`? The model's own arguments say so
/// ([`crate::tasks::UpdateArgs`], the executor's parse); an unparseable
/// call closed nothing.
#[must_use]
pub fn last_round_closed_a_task(messages: &[ChatMessage]) -> bool {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .is_some_and(|round| {
            round.tool_calls.iter().any(|call| {
                call.function.name == TASK_UPDATE_TOOL
                    && serde_json::from_str::<crate::tasks::UpdateArgs>(&call.function.arguments)
                        .ok()
                        .and_then(|args| args.status)
                        .is_some_and(|status| status == "completed" || status == "deleted")
            })
        })
}

/// Is a call to `name` **work** the finish guard counts — something that
/// changes the world (a command, a file, typed input, a launched agent, an
/// MCP tool), as opposed to reading it or keeping the list?
#[must_use]
pub fn is_work_call(name: &str) -> bool {
    if crate::tasks::is_task_tool(name) || crate::skills::is_skill_tool(name) {
        return false;
    }
    !matches!(
        name,
        "read" | BASH_WAIT_TOOL | BASH_KILL_TOOL | BASH_LIST_TOOL | ASK_TOOL_NAME
    )
}

/// The reminder text of `kind` over `store`: the template, then — when the
/// list has rows — the [`TASK_LISTING_HEADING`] over
/// [`TaskStore::reminder_listing`], wrapped in the `<system-reminder>` tags
/// (`reminder::wrap`). The whole text the model reads, and the exact text
/// the history item keeps.
#[must_use]
pub fn reminder_text(kind: ReminderKind, store: &TaskStore) -> String {
    let template = match kind {
        ReminderKind::Stale => STALE_TEMPLATE,
        ReminderKind::Finish => FINISH_TEMPLATE,
    };
    let mut text = template.trim().to_string();
    let listing = store.reminder_listing();
    if !listing.is_empty() {
        text.push_str("\n\n");
        text.push_str(TASK_LISTING_HEADING);
        text.push('\n');
        text.push_str(&listing);
    }
    crate::reminder::wrap(&text)
}

#[cfg(test)]
mod tests {
    use super::super::tools::{AGENT_TOOL_NAME, BASH_SEND_TOOL};
    use super::*;
    use crate::llm::ToolCallSpec;
    use crate::tasks::TASK_LIST_TOOL;

    fn store_with(subjects: &[&str]) -> TaskStore {
        let mut store = TaskStore::new();
        for subject in subjects {
            store
                .run_create(&format!(r#"{{"subject":"{subject}","description":"d"}}"#))
                .unwrap();
        }
        store
    }

    fn tool_round(name: &str) -> Vec<ChatMessage> {
        vec![
            ChatMessage::assistant_tool_calls("", vec![ToolCallSpec::function("c", name, "{}")]),
            ChatMessage::tool_result("c", "ok"),
        ]
    }

    fn answer(text: &str) -> ChatMessage {
        ChatMessage::new("assistant", text)
    }

    #[test]
    fn round_counts_walk_back_to_the_last_task_call_and_the_last_reminder() {
        // One assistant message is one round; a round carrying a
        // taskcreate/taskupdate ends the first count (it is not itself "a
        // round since"), a user message carrying a reminder ends the second.
        let mut messages = vec![ChatMessage::system("s"), ChatMessage::user("go")];
        messages.extend(tool_round(TASK_CREATE_TOOL));
        messages.extend(tool_round("write"));
        messages.push(ChatMessage::user(reminder_text(
            ReminderKind::Stale,
            &TaskStore::new(),
        )));
        messages.extend(tool_round("bash"));
        messages.push(answer("done"));
        let counts = round_counts(&messages);
        assert_eq!(counts.since_task_call, 3, "write, bash, the answer");
        assert_eq!(counts.since_reminder, 2, "bash, the answer");
    }

    #[test]
    fn round_counts_treat_a_list_or_get_as_no_management() {
        // Claude Code counts TaskCreate/TaskUpdate only: reading the list is
        // not keeping it.
        let mut messages = vec![ChatMessage::user("go")];
        messages.extend(tool_round(TASK_UPDATE_TOOL));
        messages.extend(tool_round(TASK_LIST_TOOL));
        let counts = round_counts(&messages);
        assert_eq!(counts.since_task_call, 1);
        assert_eq!(
            counts.since_reminder, 2,
            "no reminder ever: every round counts"
        );
    }

    #[test]
    fn a_reminder_is_recognised_inside_a_merged_user_message() {
        // The derived context joins adjacent user texts (`context::push_text`),
        // so a replayed reminder may not open its message.
        let text = reminder_text(ReminderKind::Finish, &store_with(&["a"]));
        assert!(is_task_reminder(&text));
        assert!(is_task_reminder(&format!("[background] note\n\n{text}")));
        assert!(!is_task_reminder("please update the tasks"));
        assert!(!is_task_reminder(""));
    }

    #[test]
    fn the_stale_reminder_lands_after_ten_quiet_rounds_with_no_open_work() {
        // Claude Code's thresholds: ten assistant rounds since the last
        // taskcreate/taskupdate, and ten since the last reminder.
        let store = TaskStore::new();
        let mut messages = vec![ChatMessage::user("go")];
        for _ in 0..TASK_REMINDER_ROUNDS - 1 {
            messages.extend(tool_round("read"));
        }
        assert_eq!(stale_reminder(&messages, &store), None, "one round short");
        messages.extend(tool_round("read"));
        let text = stale_reminder(&messages, &store).expect("due");
        assert!(text.starts_with("<system-reminder>\n"), "{text}");
        assert!(text.contains("haven't been used recently"), "{text}");
        assert!(
            !text.contains(TASK_LISTING_HEADING),
            "an empty list shows no rows: {text}"
        );
        // Once it has landed, the next ten rounds are quiet again.
        messages.push(ChatMessage::user(&text));
        messages.extend(tool_round("read"));
        assert_eq!(stale_reminder(&messages, &store), None);
    }

    #[test]
    fn open_work_shortens_the_window_and_lists_the_tasks() {
        let mut store = store_with(&["Create the login page", "Expose port 3000"]);
        store
            .run_update(r#"{"taskId":"1","status":"in_progress"}"#)
            .unwrap();
        let mut messages = vec![ChatMessage::user("go")];
        messages.extend(tool_round(TASK_UPDATE_TOOL));
        for _ in 0..TASK_REMINDER_OPEN_ROUNDS - 1 {
            messages.extend(tool_round("write"));
        }
        assert_eq!(stale_reminder(&messages, &store), None, "one round short");
        messages.extend(tool_round("write"));
        let text = stale_reminder(&messages, &store).expect("due");
        assert!(
            text.contains(&format!(
                "{TASK_LISTING_HEADING}\n#1. [in_progress] Create the login page\n#2. [pending] Expose port 3000\n"
            )),
            "{text}"
        );
        // A finished plan is not open work: back to the long window.
        store
            .run_update(r#"{"taskId":"1","status":"completed"}"#)
            .unwrap();
        store
            .run_update(r#"{"taskId":"2","status":"completed"}"#)
            .unwrap();
        assert_eq!(stale_reminder(&messages, &store), None);
    }

    #[test]
    fn a_task_call_this_round_keeps_the_stale_reminder_quiet() {
        let store = store_with(&["a"]);
        let mut messages = vec![ChatMessage::user("go")];
        for _ in 0..TASK_REMINDER_ROUNDS {
            messages.extend(tool_round("write"));
        }
        messages.extend(tool_round(TASK_CREATE_TOOL));
        assert_eq!(stale_reminder(&messages, &store), None);
    }

    fn update_round(args: &str) -> Vec<ChatMessage> {
        vec![
            ChatMessage::assistant_tool_calls(
                "",
                vec![ToolCallSpec::function("u", TASK_UPDATE_TOOL, args)],
            ),
            ChatMessage::tool_result("u", "Updated task #1 status"),
        ]
    }

    #[test]
    fn the_finish_reminder_needs_open_work_the_turn_did_and_no_closing_round() {
        let store = store_with(&["a", "b"]);
        let mut worked_rounds = vec![ChatMessage::user("go")];
        worked_rounds.extend(tool_round("write"));
        // Work, list open, last round not a reconciliation: due.
        assert!(finish_reminder(&worked_rounds, &store, true).is_some());
        // No acting call this turn (a question answered): nothing owed.
        assert_eq!(finish_reminder(&worked_rounds, &store, false), None);
        // Nothing open: nothing owed, whatever ran.
        assert_eq!(
            finish_reminder(&worked_rounds, &TaskStore::new(), true),
            None
        );
        let mut done = store_with(&["a"]);
        done.run_update(r#"{"taskId":"1","status":"completed"}"#)
            .unwrap();
        assert_eq!(finish_reminder(&worked_rounds, &done, true), None);
    }

    #[test]
    fn a_closing_round_is_a_reconciliation_an_opening_one_is_not() {
        // The model completing (or deleting) a task in the round before its
        // answer chose what to leave open; one that only created a task or
        // set it in_progress said "running" and then stopped.
        let store = store_with(&["a", "b"]);
        let mut closed = vec![ChatMessage::user("go")];
        closed.extend(tool_round("write"));
        closed.extend(update_round(r#"{"taskId":"1","status":"completed"}"#));
        assert!(last_round_closed_a_task(&closed));
        assert_eq!(finish_reminder(&closed, &store, true), None);
        let mut deleted = vec![ChatMessage::user("go")];
        deleted.extend(update_round(r#"{"taskId":"2","status":"deleted"}"#));
        assert!(last_round_closed_a_task(&deleted));
        let mut opened = vec![ChatMessage::user("go")];
        opened.extend(tool_round("write"));
        opened.extend(tool_round(TASK_CREATE_TOOL));
        opened.extend(update_round(r#"{"taskId":"1","status":"in_progress"}"#));
        assert!(!last_round_closed_a_task(&opened));
        assert!(finish_reminder(&opened, &store, true).is_some());
        // A closing update further back does not count: only the last round.
        let mut stale = closed.clone();
        stale.extend(tool_round("bash"));
        assert!(!last_round_closed_a_task(&stale));
        // Unparseable arguments close nothing.
        let mut broken = vec![ChatMessage::user("go")];
        broken.extend(update_round("not json"));
        assert!(!last_round_closed_a_task(&broken));
        assert!(!last_round_closed_a_task(&[]));
    }

    #[test]
    fn the_finish_text_names_the_open_tasks_and_what_to_do() {
        let store = store_with(&["a"]);
        let mut messages = vec![ChatMessage::user("go")];
        messages.extend(tool_round("write"));
        let text = finish_reminder(&messages, &store, true).unwrap();
        assert!(
            text.starts_with("<system-reminder>\nYou are about to end your turn"),
            "{text}"
        );
        assert!(text.contains("#1. [pending] a"), "{text}");
        assert!(text.ends_with("\n</system-reminder>"), "{text}");
        assert!(
            text.contains("Never mention this reminder to the user."),
            "{text}"
        );
    }

    #[test]
    fn work_calls_are_the_acting_tools() {
        // What the finish guard counts as work: a call that changes
        // something — never a read, a skill load, a list, or a task op.
        for name in [
            "bash",
            "write",
            "edit",
            BASH_SEND_TOOL,
            AGENT_TOOL_NAME,
            "mcp__srv__tool",
        ] {
            assert!(is_work_call(name), "{name}");
        }
        for name in [
            "read",
            "skill",
            BASH_LIST_TOOL,
            BASH_WAIT_TOOL,
            BASH_KILL_TOOL,
            ASK_TOOL_NAME,
            TASK_CREATE_TOOL,
            TASK_LIST_TOOL,
        ] {
            assert!(!is_work_call(name), "{name}");
        }
    }

    #[test]
    fn the_templates_are_short_and_name_both_tools() {
        for kind in [ReminderKind::Stale, ReminderKind::Finish] {
            let text = reminder_text(kind, &TaskStore::new());
            assert!(text.contains("taskupdate"), "{text}");
            assert!(
                text.contains("Never mention this reminder to the user."),
                "{text}"
            );
            assert!(text.len() < 600, "{} chars: {text}", text.len());
        }
    }
}
