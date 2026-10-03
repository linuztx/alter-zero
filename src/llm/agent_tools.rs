//! The agent companions' executor — `agentsend`, `agentwait`,
//! `agentoutput`, `agentkill` and `agentlist` over the shared
//! [`AgentRegistry`] (`docs/agent-tools.md`).
//!
//! [`run_companion`] is the one entry point: the main backend routes a
//! companion call here ahead of the real executor, the way it routes the
//! ask, task and skill tools, and the offline demo drives the same function
//! so its cells carry live output (`docs/dummy-backend.md`). Everything a
//! companion says to the model is built here or in the pure
//! [`crate::agents`] reports; what it *does* goes through the registry — the
//! same calls the user's roster keys make — plus the agent channel, for the
//! two things the event loop must learn from this side
//! ([`AgentEvent::Background`], [`AgentEvent::Stop`]).

use std::time::{Duration, Instant};

use super::ChatMessage;
use super::tools::{
    self, AGENT_KILL_TOOL, AGENT_LIST_TOOL, AGENT_OUTPUT_TOOL, AGENT_SEND_TOOL, AGENT_WAIT_TOOL,
    AgentIdArgs, AgentSendArgs, AgentWaitArgs, ToolCallRequest, ToolOutcome,
};
use crate::agents::{
    AgentEvent, AgentProgress, AgentRegistry, AgentState, agent_list_report, agent_output_report,
    unknown_agent_text,
};
use crate::background::BackgroundRegistry;
use crate::stream::CancelToken;

/// How often `agentwait` polls the registry — the foreground launch's own
/// wait cadence (`llm::backend`'s `AGENT_WAIT_POLL`).
pub const WAIT_POLL: Duration = Duration::from_millis(30);

/// What starts a settled agent's continuation: its id, its stored
/// conversation (the message the model sent rides the pending-input seam,
/// not this list), and the fresh cancel token the registry minted for the
/// run. The backend's is `spawn_subagent_run`; the offline demo's plays a
/// scripted round.
pub type Resume<'a> = &'a dyn Fn(String, Vec<ChatMessage>, CancelToken);

/// The model-facing refusal a **subagent** gets for any agent tool — the
/// launch or a companion: agents do not reach other agents.
pub const NO_NESTING_TEXT: &str =
    "agents cannot launch, steer or inspect other agents — only the main agent can";

/// Run one companion call (`docs/agent-tools.md`). `board` is the session's
/// notice board, where a reported completion is marked observed so its note
/// is not posted as well; `None` (an embedder, a test) skips the mark.
/// `resume` starts the continuation `agentsend` opens on a finished agent.
#[must_use]
pub fn run_companion(
    call: &ToolCallRequest,
    registry: &AgentRegistry,
    board: Option<&BackgroundRegistry>,
    cancel: &CancelToken,
    resume: Resume<'_>,
) -> ToolOutcome {
    match call.name.as_str() {
        AGENT_LIST_TOOL => ToolOutcome::ok(agent_list_report(&registry.list())),
        AGENT_OUTPUT_TOOL => match tools::parse_args::<AgentIdArgs>(&call.arguments) {
            Ok(args) => run_output(args.agent_id.trim(), registry, board),
            Err(e) => ToolOutcome::error(e),
        },
        AGENT_WAIT_TOOL => match tools::parse_args::<AgentWaitArgs>(&call.arguments) {
            Ok(args) => run_wait(
                args.agent_id.trim(),
                Duration::from_millis(tools::agent_wait_ms(Some(&call.arguments))),
                registry,
                board,
                cancel,
            ),
            Err(e) => ToolOutcome::error(e),
        },
        AGENT_KILL_TOOL => match tools::parse_args::<AgentIdArgs>(&call.arguments) {
            Ok(args) => run_kill(args.agent_id.trim(), registry, board),
            Err(e) => ToolOutcome::error(e),
        },
        AGENT_SEND_TOOL => match tools::parse_args::<AgentSendArgs>(&call.arguments) {
            Ok(args) => run_send(args.agent_id.trim(), &args.message, registry, board, resume),
            Err(e) => ToolOutcome::error(e),
        },
        other => ToolOutcome::error(format!("unknown tool: {other}")),
    }
}

/// `agentoutput`: the agent's progress now. A settled agent's report is a
/// completion the model has now read — observed, so no notice follows it.
fn run_output(
    id: &str,
    registry: &AgentRegistry,
    board: Option<&BackgroundRegistry>,
) -> ToolOutcome {
    let Some(progress) = registry.progress(id) else {
        return ToolOutcome::error(unknown_agent_text(id, &registry.list()));
    };
    observe_if_settled(&progress, board);
    ToolOutcome::ok(agent_output_report(&progress))
}

/// `agentwait`: block until the agent settles or `wait` passes — ending
/// early on the turn's cancel and on the user's Ctrl+B, the latch the
/// foreground launch's wait reads too — and report what there is.
fn run_wait(
    id: &str,
    wait: Duration,
    registry: &AgentRegistry,
    board: Option<&BackgroundRegistry>,
    cancel: &CancelToken,
) -> ToolOutcome {
    let started = Instant::now();
    loop {
        let Some(progress) = registry.progress(id) else {
            return ToolOutcome::error(unknown_agent_text(id, &registry.list()));
        };
        if progress.state != AgentState::Running {
            observe_if_settled(&progress, board);
            return ToolOutcome::ok(agent_output_report(&progress));
        }
        if cancel.is_cancelled() {
            return ToolOutcome::error(WAIT_INTERRUPTED_TEXT);
        }
        if board.is_some_and(BackgroundRegistry::take_background_request) {
            return ToolOutcome::ok(format!(
                "{}\n{WAIT_ENDED_NOTE}",
                agent_output_report(&progress)
            ));
        }
        if started.elapsed() >= wait {
            return ToolOutcome::ok(agent_output_report(&progress));
        }
        std::thread::sleep(
            WAIT_POLL.min(
                wait.saturating_sub(started.elapsed())
                    .max(Duration::from_millis(1)),
            ),
        );
    }
}

/// `agentkill`: stop a running agent — observed first, so the stop's own
/// notice is not posted to the model that asked for it — through the
/// registry (the thread's cancel) and the agent channel (the roster's
/// settle, the user's `x` path). A settled agent is reported, not stopped.
fn run_kill(id: &str, registry: &AgentRegistry, board: Option<&BackgroundRegistry>) -> ToolOutcome {
    let Some(progress) = registry.progress(id) else {
        return ToolOutcome::error(unknown_agent_text(id, &registry.list()));
    };
    if progress.state != AgentState::Running {
        return ToolOutcome::ok(not_running_text(&progress));
    }
    if let Some(board) = board {
        let _ = board.observe(id);
    }
    let _ = registry.kill(id);
    registry.send(AgentEvent::Stop { id: id.to_string() });
    ToolOutcome::ok(stopped_text(&progress))
}

/// `agentsend`: into the running loop's queue, or a continuation over a
/// finished agent's stored conversation — the message riding the same
/// pending-input seam either way, so the loop announces it with
/// `StreamEvent::Steered` and the agent's transcript records the bubble
/// when its loop genuinely has it.
fn run_send(
    id: &str,
    message: &str,
    registry: &AgentRegistry,
    board: Option<&BackgroundRegistry>,
    resume: Resume<'_>,
) -> ToolOutcome {
    if message.trim().is_empty() {
        return ToolOutcome::error(EMPTY_MESSAGE_TEXT);
    }
    let Some(progress) = registry.progress(id) else {
        return ToolOutcome::error(unknown_agent_text(id, &registry.list()));
    };
    match progress.state {
        AgentState::Stopped => return ToolOutcome::error(stopped_refusal_text(&progress)),
        AgentState::Failed(_) => return ToolOutcome::error(failed_refusal_text(&progress)),
        AgentState::Running | AgentState::Finished => {}
    }
    // Still running: the next round boundary takes it. (A run that settled
    // between the look above and this queue falls through to the
    // continuation — the registry, not the snapshot, decides.)
    if registry.queue_input(id, message) {
        return ToolOutcome::ok(queued_text(&progress));
    }
    let Some((messages, cancel)) = registry.begin_continuation(id) else {
        return ToolOutcome::error(not_resumable_text(&progress));
    };
    // A new turn is a new completion: the model will hear this one as a
    // notice, so the mark a report may have left comes off, and a
    // foreground agent becomes a background one from here on.
    if let Some(board) = board {
        board.forget_observed(id);
    }
    registry.send(AgentEvent::Background { id: id.to_string() });
    // The slot is busy again, so the queue takes it — and the continuation's
    // first round boundary announces it.
    let _ = registry.queue_input(id, message);
    resume(id.to_string(), messages, cancel);
    ToolOutcome::ok(resumed_text(&progress))
}

/// Mark a settled agent's completion observed on the board: the model has
/// read it in this call's result, so no notice follows (`docs/agent-tools.md`).
fn observe_if_settled(progress: &AgentProgress, board: Option<&BackgroundRegistry>) {
    if progress.state == AgentState::Running {
        return;
    }
    if let Some(board) = board {
        let _ = board.observe(&progress.id);
    }
}

/// The result of an `agentwait` the turn's cancel ended.
pub const WAIT_INTERRUPTED_TEXT: &str = "[The wait was interrupted; the agent keeps running.]";

/// The note closing an `agentwait` report the user ended with Ctrl+B.
pub const WAIT_ENDED_NOTE: &str = "[The user ended this wait early; the agent keeps running. You will be notified when it finishes.]";

/// The refusal for an `agentsend` with nothing to send.
pub const EMPTY_MESSAGE_TEXT: &str = "`message` was empty — nothing was sent to the agent.";

/// What every delivery promises after the fact: how the response arrives,
/// and the companions that reach the agent meanwhile.
const FOLLOW_UP_HINT: &str = "You will be notified with its response when it finishes; \
                              agentwait waits for it, agentoutput shows its progress.";

/// `agentsend` into a running agent.
#[must_use]
pub fn queued_text(progress: &AgentProgress) -> String {
    format!(
        "Message delivered to agent {} (\"{}\"): it reads it at its next step. {FOLLOW_UP_HINT}",
        progress.id, progress.description
    )
}

/// `agentsend` into a finished agent: a new turn over its conversation.
#[must_use]
pub fn resumed_text(progress: &AgentProgress) -> String {
    format!(
        "Agent {} (\"{}\") resumed with your message as a new turn over its kept \
         conversation. {FOLLOW_UP_HINT}",
        progress.id, progress.description
    )
}

/// `agentkill` on a running agent.
#[must_use]
pub fn stopped_text(progress: &AgentProgress) -> String {
    format!(
        "Stopped agent {} (\"{}\") after {}.",
        progress.id,
        progress.description,
        crate::app::format_elapsed(progress.elapsed.as_secs())
    )
}

/// `agentkill` on an agent that is not running — reported, not stopped.
#[must_use]
pub fn not_running_text(progress: &AgentProgress) -> String {
    format!(
        "Agent {} (\"{}\") is not running — it {}. Nothing to stop.",
        progress.id,
        progress.description,
        settled_clause(progress)
    )
}

/// `agentsend` to a stopped agent.
fn stopped_refusal_text(progress: &AgentProgress) -> String {
    format!(
        "Agent {} (\"{}\") was stopped and cannot take messages — launch a new agent for \
         the task.",
        progress.id, progress.description
    )
}

/// `agentsend` to a failed agent.
fn failed_refusal_text(progress: &AgentProgress) -> String {
    format!(
        "Agent {} (\"{}\") {} and cannot be continued — launch a new agent for the task.",
        progress.id,
        progress.description,
        settled_clause(progress)
    )
}

/// `agentsend` to an agent that is neither taking input nor resumable right
/// now — its slot is between a run's last drain and its settle.
fn not_resumable_text(progress: &AgentProgress) -> String {
    format!(
        "Agent {} (\"{}\") could not take the message right now — it is settling; \
         agentwait on it, then send again.",
        progress.id, progress.description
    )
}

/// `finished in 1m 2s` / `was stopped after 12s` / `failed after 5s: …` —
/// the past-tense clause the refusals read.
fn settled_clause(progress: &AgentProgress) -> String {
    let elapsed = crate::app::format_elapsed(progress.elapsed.as_secs());
    match &progress.state {
        AgentState::Running => format!("is running ({elapsed})"),
        AgentState::Finished => format!("finished in {elapsed}"),
        AgentState::Stopped => format!("was stopped after {elapsed}"),
        AgentState::Failed(error) => {
            format!("failed after {elapsed}: {}", tools::flatten_one_line(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::time::Duration;

    use super::*;
    use crate::agents::GENERAL_PURPOSE;
    use crate::stream::StreamEvent;

    fn new_registry() -> (
        AgentRegistry,
        tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (AgentRegistry::new(tx), rx)
    }

    fn board() -> BackgroundRegistry {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        BackgroundRegistry::new(
            tx,
            std::env::temp_dir().join(format!("alter-zero-agent-tools-{}", std::process::id())),
        )
    }

    fn call(name: &str, arguments: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: "c1".to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        }
    }

    fn no_resume(_: String, _: Vec<ChatMessage>, _: CancelToken) {
        panic!("nothing should resume here");
    }

    /// Run `name` with `arguments` against `registry`, no board, no resume.
    fn run(name: &str, arguments: &str, registry: &AgentRegistry) -> ToolOutcome {
        run_companion(
            &call(name, arguments),
            registry,
            None,
            &CancelToken::new(),
            &no_resume,
        )
    }

    #[test]
    fn agentlist_names_every_agent() {
        let (registry, _rx) = new_registry();
        let (first, _) = registry.register(GENERAL_PURPOSE, "Fetch weather");
        let (second, _) = registry.register("explore", "Find the loader");
        registry.finish(&second, Ok("found".into()), Vec::new());
        let outcome = run(AGENT_LIST_TOOL, "{}", &registry);
        assert!(outcome.ok);
        assert!(
            outcome.output.starts_with("2 agents:\n"),
            "{}",
            outcome.output
        );
        assert!(
            outcome.output.contains(&format!(
                "- {first}: general-purpose \"Fetch weather\" — running"
            )),
            "{}",
            outcome.output
        );
        assert!(
            outcome.output.contains(&format!(
                "- {second}: explore \"Find the loader\" — finished in"
            )),
            "{}",
            outcome.output
        );
        let (empty, _rx) = new_registry();
        assert_eq!(
            run(AGENT_LIST_TOOL, "{}", &empty).output,
            "No agents have been launched."
        );
    }

    #[test]
    fn agentoutput_reports_the_steps_and_observes_a_settled_agent() {
        let (registry, _rx) = new_registry();
        let board = board();
        let (id, _) = registry.register(GENERAL_PURPOSE, "Fetch weather");
        registry.send(AgentEvent::Stream {
            id: id.clone(),
            event: StreamEvent::ToolStart {
                name: "Bash".into(),
                args: "curl -s wttr.in".into(),
                detail: None,
                arguments: None,
            },
        });
        let args = format!(r#"{{"agent_id":"{id}"}}"#);
        let running = run_companion(
            &call(AGENT_OUTPUT_TOOL, &args),
            &registry,
            Some(&board),
            &CancelToken::new(),
            &no_resume,
        );
        assert!(running.ok);
        assert!(running.output.contains("running"), "{}", running.output);
        assert!(
            running.output.contains("  Bash(curl -s wttr.in)"),
            "{}",
            running.output
        );
        assert!(
            !board.is_observed(&id),
            "a running agent's report observes nothing"
        );
        registry.finish(&id, Ok("19°C".into()), Vec::new());
        let settled = run_companion(
            &call(AGENT_OUTPUT_TOOL, &args),
            &registry,
            Some(&board),
            &CancelToken::new(),
            &no_resume,
        );
        assert!(
            settled.output.ends_with("Final response:\n19°C"),
            "{}",
            settled.output
        );
        assert!(board.is_observed(&id), "the model read the completion here");
        // An unknown id names the ones that exist.
        let unknown = run(AGENT_OUTPUT_TOOL, r#"{"agent_id":"a0000000"}"#, &registry);
        assert!(!unknown.ok);
        assert!(
            unknown.output.starts_with("No agent a0000000"),
            "{}",
            unknown.output
        );
        assert!(unknown.output.contains(&id), "{}", unknown.output);
    }

    #[test]
    fn agentwait_returns_when_the_agent_settles() {
        let (registry, _rx) = new_registry();
        let board = board();
        let (id, _) = registry.register(GENERAL_PURPOSE, "Fetch weather");
        let settler = registry.clone();
        let settling = id.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            settler.finish(&settling, Ok("19°C".into()), Vec::new());
        });
        let started = Instant::now();
        let outcome = run_companion(
            &call(
                AGENT_WAIT_TOOL,
                &format!(r#"{{"agent_id":"{id}","wait":5}}"#),
            ),
            &registry,
            Some(&board),
            &CancelToken::new(),
            &no_resume,
        );
        thread.join().expect("settler");
        assert!(outcome.ok);
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "returned at the settle, not the budget"
        );
        assert!(outcome.output.contains("finished in"), "{}", outcome.output);
        assert!(
            outcome.output.ends_with("Final response:\n19°C"),
            "{}",
            outcome.output
        );
        assert!(board.is_observed(&id));
    }

    #[test]
    fn agentwait_with_no_budget_looks_and_a_cancel_ends_it() {
        let (registry, _rx) = new_registry();
        let (id, _) = registry.register(GENERAL_PURPOSE, "Fetch weather");
        let look = run(
            AGENT_WAIT_TOOL,
            &format!(r#"{{"agent_id":"{id}","wait":0}}"#),
            &registry,
        );
        assert!(look.ok);
        assert!(look.output.contains("running"), "{}", look.output);
        assert!(
            look.output.contains("Reply so far:\n(nothing yet)"),
            "{}",
            look.output
        );
        let cancel = CancelToken::new();
        cancel.cancel();
        let interrupted = run_companion(
            &call(
                AGENT_WAIT_TOOL,
                &format!(r#"{{"agent_id":"{id}","wait":60}}"#),
            ),
            &registry,
            None,
            &cancel,
            &no_resume,
        );
        assert!(!interrupted.ok);
        assert_eq!(interrupted.output, WAIT_INTERRUPTED_TEXT);
    }

    #[test]
    fn agentwait_ends_on_the_users_ctrl_b() {
        let (registry, _rx) = new_registry();
        let board = board();
        let (id, _) = registry.register(GENERAL_PURPOSE, "Fetch weather");
        board.request_background();
        let outcome = run_companion(
            &call(
                AGENT_WAIT_TOOL,
                &format!(r#"{{"agent_id":"{id}","wait":60}}"#),
            ),
            &registry,
            Some(&board),
            &CancelToken::new(),
            &no_resume,
        );
        assert!(outcome.ok);
        assert!(
            outcome.output.ends_with(WAIT_ENDED_NOTE),
            "{}",
            outcome.output
        );
        assert!(!board.is_observed(&id), "the agent keeps running");
    }

    #[test]
    fn agentkill_stops_a_running_agent_and_reports_a_settled_one() {
        let (registry, mut rx) = new_registry();
        let board = board();
        let (id, cancel) = registry.register(GENERAL_PURPOSE, "Fetch weather");
        let args = format!(r#"{{"agent_id":"{id}"}}"#);
        let stopped = run_companion(
            &call(AGENT_KILL_TOOL, &args),
            &registry,
            Some(&board),
            &CancelToken::new(),
            &no_resume,
        );
        assert!(stopped.ok);
        assert!(
            stopped
                .output
                .starts_with(&format!("Stopped agent {id} (\"Fetch weather\") after")),
            "{}",
            stopped.output
        );
        assert!(cancel.is_cancelled(), "the thread is told to stop");
        assert!(registry.is_killed(&id));
        assert!(board.is_observed(&id), "the model did it — no notice owed");
        assert_eq!(
            rx.try_recv().ok(),
            Some(AgentEvent::Stop { id: id.clone() }),
            "the loop settles the roster row"
        );
        let again = run_companion(
            &call(AGENT_KILL_TOOL, &args),
            &registry,
            Some(&board),
            &CancelToken::new(),
            &no_resume,
        );
        assert!(again.ok);
        assert!(
            again
                .output
                .contains("is not running — it was stopped after"),
            "{}",
            again.output
        );
        assert!(rx.try_recv().is_err(), "nothing to settle twice");
    }

    #[test]
    fn agentsend_queues_into_a_running_agent() {
        let (registry, mut rx) = new_registry();
        let (id, _) = registry.register(GENERAL_PURPOSE, "Fetch weather");
        let outcome = run(
            AGENT_SEND_TOOL,
            &format!(r#"{{"agent_id":"{id}","message":"also Krakow"}}"#),
            &registry,
        );
        assert!(outcome.ok);
        assert!(
            outcome.output.starts_with(&format!(
                "Message delivered to agent {id} (\"Fetch weather\")"
            )),
            "{}",
            outcome.output
        );
        assert_eq!(registry.take_pending_inputs(&id), vec!["also Krakow"]);
        assert!(rx.try_recv().is_err(), "a running agent is left as it is");
    }

    #[test]
    fn agentsend_resumes_a_finished_agent_through_the_pending_input_seam() {
        let (registry, mut rx) = new_registry();
        let board = board();
        let (id, _) = registry.register(GENERAL_PURPOSE, "Fetch weather");
        registry.finish(&id, Ok("19°C".into()), vec![ChatMessage::user("Warsaw?")]);
        assert!(
            board.observe(&id) || board.is_observed(&id),
            "a report left its mark"
        );
        let resumed: RefCell<Option<(String, Vec<ChatMessage>)>> = RefCell::new(None);
        let resume = |id: String, messages: Vec<ChatMessage>, cancel: CancelToken| {
            assert!(!cancel.is_cancelled());
            *resumed.borrow_mut() = Some((id, messages));
        };
        let outcome = run_companion(
            &call(
                AGENT_SEND_TOOL,
                &format!(r#"{{"agent_id":"{id}","message":"and Krakow"}}"#),
            ),
            &registry,
            Some(&board),
            &CancelToken::new(),
            &resume,
        );
        assert!(outcome.ok, "{}", outcome.output);
        assert!(
            outcome
                .output
                .starts_with(&format!("Agent {id} (\"Fetch weather\") resumed")),
            "{}",
            outcome.output
        );
        let (resumed_id, messages) = resumed.borrow().clone().expect("the continuation started");
        assert_eq!(resumed_id, id);
        assert_eq!(
            messages.len(),
            1,
            "the stored conversation, the message riding the seam instead"
        );
        assert_eq!(registry.take_pending_inputs(&id), vec!["and Krakow"]);
        assert!(!board.is_observed(&id), "a new turn is a new completion");
        assert_eq!(
            rx.try_recv().ok(),
            Some(AgentEvent::Background { id: id.clone() })
        );
        assert!(!registry.is_done(&id), "busy again");
    }

    #[test]
    fn agentsend_refuses_what_cannot_take_a_message() {
        let (registry, _rx) = new_registry();
        let (stopped, _) = registry.register(GENERAL_PURPOSE, "s");
        let _ = registry.kill(&stopped);
        let (failed, _) = registry.register(GENERAL_PURPOSE, "f");
        registry.finish(&failed, Err("boom".into()), Vec::new());
        let send = |id: &str, message: &str| {
            run(
                AGENT_SEND_TOOL,
                &format!(r#"{{"agent_id":"{id}","message":"{message}"}}"#),
                &registry,
            )
        };
        let refused = send(&stopped, "hi");
        assert!(!refused.ok);
        assert!(
            refused
                .output
                .contains("was stopped and cannot take messages"),
            "{}",
            refused.output
        );
        let refused = send(&failed, "hi");
        assert!(!refused.ok);
        assert!(
            refused.output.contains("failed after"),
            "{}",
            refused.output
        );
        assert!(refused.output.contains("boom"), "{}", refused.output);
        let empty = send(&stopped, "  ");
        assert_eq!(empty.output, EMPTY_MESSAGE_TEXT);
        let unknown = send("a0000000", "hi");
        assert!(
            unknown.output.starts_with("No agent a0000000"),
            "{}",
            unknown.output
        );
    }
}
