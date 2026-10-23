//! The `agent` tool's companions — `agentsend`, `agentoutput`, `agentkill`
//! and `agentlist` — run against the shared [`AgentRegistry`]
//! (`docs/agent-tools.md`).
//!
//! The lead's tools alone: a subagent is never offered them. A continuation
//! of a settled agent is spawned through an injected launcher
//! ([`AgentToolContext::resume`]) — the backend's `spawn_subagent_run` in
//! production, a recorder in the tests — so everything here runs with no
//! model at all.

use std::time::{Duration, Instant};

use super::ChatMessage;
use super::exec::ToolProgress;
use super::tools::{
    self, AgentIdArgs, AgentOutputArgs, AgentSendArgs, ToolCallRequest, ToolOutcome,
};
use crate::agents::{AgentRegistry, AgentState};
use crate::background::BackgroundRegistry;
use crate::stream::CancelToken;

/// A settled agent's conversation, handed to the launcher to continue
/// ([`AgentToolContext::resume`]): the message is already on the agent's
/// queue, so the run's first round announces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Continuation {
    pub id: String,
    pub agent_type: String,
    pub messages: Vec<ChatMessage>,
    pub cancel: CancelToken,
}

/// What a companion call runs against.
pub struct AgentToolContext<'a> {
    pub registry: &'a AgentRegistry,
    /// The lead turn's cancel — a waiting `agentoutput` ends on Esc.
    pub cancel: &'a CancelToken,
    /// The owner of the Ctrl+B latch: a waiting `agentoutput` ends when the
    /// user presses it, the agent running on (`bashwait`'s rule).
    pub background: Option<&'a BackgroundRegistry>,
    /// Starts a settled agent's continuation run.
    pub resume: &'a dyn Fn(Continuation),
}

/// How often a waiting `agentoutput` looks at its agent — and at the lead's
/// cancel and the Ctrl+B latch.
const WAIT_POLL: Duration = Duration::from_millis(50);

/// How long an `agentsend` waits for an agent the lead just stopped to let go
/// of its run, before it can resume it — a cancelled loop returns within a
/// round, a running command within its kill grace.
const STOPPING_GRACE: Duration = Duration::from_secs(10);

/// Appended to every report of an agent still at work — a look, or a wait
/// that passed.
const STILL_RUNNING_NOTE: &str = "[Still running — you are notified when it finishes; \
     agentoutput with `wait` waits for it.]";

/// Appended when the user's Ctrl+B ended the wait (`bashwait`'s note).
const WAIT_ENDED_NOTE: &str = "[The user ended this wait early; the agent keeps running \
     and you are notified when it finishes.]";

/// Ahead of an earlier run's notice a report carries: the agent ended that
/// run before the one reported, and the notice never reached the lead
/// (`docs/agent-tools.md` *One notice per answer*).
const EARLIER_RUN_NOTE: &str =
    "[Before this run it had ended an earlier one, whose notice had not reached you:]";

/// [`EARLIER_RUN_NOTE`] for several.
const EARLIER_RUNS_NOTE: &str = "[Before this run it had ended earlier ones, whose notices \
     had not reached you — oldest first:]";

/// Run one companion call (`docs/agent-tools.md`). `on_output` streams a
/// waiting `agentoutput`'s calls as the agent starts them, so its running
/// cell shows the agent at work.
pub fn run_agent_tool(
    ctx: &AgentToolContext<'_>,
    call: &ToolCallRequest,
    on_output: &mut dyn FnMut(ToolProgress<'_>),
) -> ToolOutcome {
    match call.name.as_str() {
        tools::AGENT_SEND_TOOL => match tools::parse_args::<AgentSendArgs>(&call.arguments) {
            Ok(args) => send(ctx, &args),
            Err(e) => ToolOutcome::error(e),
        },
        tools::AGENT_OUTPUT_TOOL => match tools::parse_args::<AgentOutputArgs>(&call.arguments) {
            Ok(args) => output(ctx, &args, on_output),
            Err(e) => ToolOutcome::error(e),
        },
        tools::AGENT_KILL_TOOL => match tools::parse_args::<AgentIdArgs>(&call.arguments) {
            Ok(args) => kill(ctx, args.id()),
            Err(e) => ToolOutcome::error(e),
        },
        tools::AGENT_LIST_TOOL => {
            ToolOutcome::ok(crate::agents::list_report(&ctx.registry.snapshots()))
        }
        other => ToolOutcome::error(format!("unknown tool: {other}")),
    }
}

/// `agentsend`: onto a running agent's queue, or a settled one's
/// continuation — the registry decides which, since only it knows whether
/// the loop is still going.
fn send(ctx: &AgentToolContext<'_>, args: &AgentSendArgs) -> ToolOutcome {
    let id = args.id();
    if args.message.trim().is_empty() {
        return ToolOutcome::error(
            "`message` is empty — say what the agent should do next.".to_string(),
        );
    }
    let deadline = Instant::now() + STOPPING_GRACE;
    loop {
        match ctx.registry.resume(id, &args.message) {
            crate::agents::Resume::Queued => {
                return ToolOutcome::ok(format!(
                    "Sent to agent {id}: it reads your message after its current step. You \
                     will be notified with its response when it finishes."
                ));
            }
            crate::agents::Resume::Continue {
                messages,
                cancel,
                agent_type,
            } => {
                (ctx.resume)(Continuation {
                    id: id.to_string(),
                    agent_type,
                    messages,
                    cancel,
                });
                return ToolOutcome::ok(format!(
                    "Agent {id} resumed with your message and is working in the background. \
                     You will be notified with its response when it finishes."
                ));
            }
            crate::agents::Resume::StoppedByUser => {
                return ToolOutcome::error(format!(
                    "Agent {id} was stopped by the user and cannot be resumed — launch a new \
                     agent if the task still needs doing."
                ));
            }
            crate::agents::Resume::Unknown => return unknown_agent(ctx.registry, id),
            crate::agents::Resume::Stopping => {
                if ctx.cancel.is_cancelled() || Instant::now() >= deadline {
                    return ToolOutcome::error(format!(
                        "Agent {id} is still stopping — send the message again in a moment."
                    ));
                }
                std::thread::sleep(WAIT_POLL);
            }
        }
    }
}

/// `agentoutput`: the agent's report at once, or — with a `wait` and the
/// agent still running — once it settles, the wait passes, the lead is
/// interrupted, or the user's Ctrl+B ends the wait. A wait **registers** with
/// the registry, so a settle it sees is reported here and not noticed again
/// (`docs/agent-tools.md` *One notice per answer*).
fn output(
    ctx: &AgentToolContext<'_>,
    args: &AgentOutputArgs,
    on_output: &mut dyn FnMut(ToolProgress<'_>),
) -> ToolOutcome {
    let id = args.id();
    let Some(snapshot) = ctx.registry.snapshot(id) else {
        return unknown_agent(ctx.registry, id);
    };
    let wait = Duration::from_millis(args.wait_ms());
    if wait.is_zero() || snapshot.state != AgentState::Running {
        return reported(ctx, &snapshot, false);
    }
    if !ctx.registry.begin_wait(id) {
        return unknown_agent(ctx.registry, id);
    }
    // A Ctrl+B pressed before this wait began belongs to nothing — the
    // `bash` runner's rule (docs/background.md).
    if let Some(background) = ctx.background {
        background.clear_background_request();
    }
    let deadline = Instant::now() + wait;
    // What the agent did before the wait is the report's; the cell streams
    // what it starts, says and reads while it is watched.
    let mut cursor = crate::agents::ProgressCursor::at(&snapshot);
    let mut ended_by_user = false;
    loop {
        let Some(progress) = ctx.registry.progress(id, &mut cursor) else {
            break;
        };
        // What the agent has done since the last look, a line each, in the
        // order it did it — the running cell tails them
        // (docs/agent-progress.md).
        for line in &progress.lines {
            on_output(ToolProgress::Screen {
                settled: &format!("{line}\n"),
                live: "",
            });
        }
        if !progress.running || ctx.cancel.is_cancelled() || Instant::now() >= deadline {
            break;
        }
        if ctx
            .background
            .is_some_and(BackgroundRegistry::take_background_request)
        {
            ended_by_user = true;
            break;
        }
        std::thread::sleep(WAIT_POLL);
    }
    // A cancelled call hands the lead nothing, so the notice stays owed.
    let reporting = !ctx.cancel.is_cancelled();
    let Some(snapshot) = ctx.registry.end_wait(id, reporting) else {
        return unknown_agent(ctx.registry, id);
    };
    if !reporting {
        return ToolOutcome::ok(report(&snapshot, ended_by_user));
    }
    reported(ctx, &snapshot, ended_by_user)
}

/// The report, handed to the lead — and, for a settled agent, the
/// registry told so, so the completion notice saying the same is cancelled
/// or taken back (`docs/agent-tools.md` *One notice per answer*). A wait
/// that saw the settle already marked it observed; this covers the settle
/// it just missed, and the one that landed before the lead looked. Any
/// earlier run's notice still unread comes back from the registry and
/// follows the report, introduced as the earlier answer it is.
fn reported(
    ctx: &AgentToolContext<'_>,
    snapshot: &crate::agents::AgentSnapshot,
    ended_by_user: bool,
) -> ToolOutcome {
    let earlier = if snapshot.state == AgentState::Running {
        Vec::new()
    } else {
        ctx.registry
            .report_settled(&snapshot.id, snapshot.run, ctx.background)
    };
    let mut report = report(snapshot, ended_by_user);
    if !earlier.is_empty() {
        report.push_str("\n\n");
        report.push_str(if earlier.len() == 1 {
            EARLIER_RUN_NOTE
        } else {
            EARLIER_RUNS_NOTE
        });
        report.push('\n');
        report.push_str(&earlier.join("\n\n"));
    }
    ToolOutcome::ok(report)
}

/// `agentoutput`'s result: the report, and — for an agent still at work —
/// the note saying so and how to wait for it, on every look (a model that
/// looked once without `wait` told the user the agent had stopped).
/// `ended_by_user` swaps in the note for a wait the user's Ctrl+B ended.
fn report(snapshot: &crate::agents::AgentSnapshot, ended_by_user: bool) -> String {
    let mut report = crate::agents::output_report(snapshot);
    if snapshot.state == AgentState::Running {
        report.push_str("\n\n");
        report.push_str(if ended_by_user {
            WAIT_ENDED_NOTE
        } else {
            STILL_RUNNING_NOTE
        });
    }
    report
}

/// `agentkill`: the lead's stop — the agent's report, where it got to, as
/// the result. One already settled is left as it was. The board goes with
/// the stop: either result says the run is over, so a note still announcing
/// that the user started the run is taken back ([`AgentRegistry::stop`]).
fn kill(ctx: &AgentToolContext<'_>, id: &str) -> ToolOutcome {
    let Some(stop) = ctx.registry.stop(id, ctx.background) else {
        return unknown_agent(ctx.registry, id);
    };
    let Some(snapshot) = ctx.registry.snapshot(id) else {
        return unknown_agent(ctx.registry, id);
    };
    if !stop.was_live {
        return ToolOutcome::ok(format!(
            "Agent {id} is not running — nothing was stopped.\n{}",
            snapshot.frame()
        ));
    }
    ToolOutcome::ok(crate::agents::output_report(&snapshot))
}

/// The error for an id nothing answers to — naming the agents that do, so a
/// model that lost track (a `/compact`, a `/resume`) finds its way back
/// rather than guessing (`bashwait`'s rule).
fn unknown_agent(registry: &AgentRegistry, id: &str) -> ToolOutcome {
    let head = format!(
        "No agent {id} here — agents end with /clear or a restart, and only the {} most \
         recently finished are kept.",
        crate::agents::AGENT_RETAINED_MAX
    );
    let agents: Vec<String> = registry
        .snapshots()
        .into_iter()
        .map(|snapshot| {
            if snapshot.description.is_empty() {
                snapshot.id
            } else {
                format!("{} ({})", snapshot.id, snapshot.description)
            }
        })
        .collect();
    if agents.is_empty() {
        return ToolOutcome::error(format!("{head} No agents in this session."));
    }
    ToolOutcome::error(format!("{head} Agents: {}.", agents.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentEvent, GENERAL_PURPOSE};
    use crate::stream::StreamEvent;
    use std::cell::RefCell;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn registry() -> (AgentRegistry, UnboundedReceiver<AgentEvent>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (AgentRegistry::new(tx), rx)
    }

    fn drain(rx: &mut UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    fn call(name: &str, arguments: serde_json::Value) -> ToolCallRequest {
        ToolCallRequest {
            id: "c1".into(),
            name: name.into(),
            arguments: arguments.to_string(),
        }
    }

    /// Run `call` with a launcher that records each continuation it is asked
    /// to start, returning the outcome and the recorded continuations.
    fn run(
        registry: &AgentRegistry,
        cancel: &CancelToken,
        background: Option<&BackgroundRegistry>,
        call: &ToolCallRequest,
    ) -> (ToolOutcome, Vec<Continuation>, Vec<String>) {
        let started = RefCell::new(Vec::new());
        let resume = |continuation: Continuation| started.borrow_mut().push(continuation);
        let ctx = AgentToolContext {
            registry,
            cancel,
            background,
            resume: &resume,
        };
        let mut streamed = Vec::new();
        let outcome = run_agent_tool(&ctx, call, &mut |progress| {
            if let ToolProgress::Screen { settled, .. } = progress {
                streamed.push(settled.to_string());
            }
        });
        (outcome, started.into_inner(), streamed)
    }

    #[test]
    fn agentsend_queues_into_a_running_agent() {
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "Fetch profile", "p");
        let (outcome, started, _) = run(
            &registry,
            &CancelToken::new(),
            None,
            &call(
                tools::AGENT_SEND_TOOL,
                serde_json::json!({"agent_id": id, "message": "also check gists"}),
            ),
        );
        assert!(outcome.ok, "{}", outcome.output);
        assert!(outcome.output.contains(&id), "{}", outcome.output);
        assert!(
            outcome.output.contains("current step"),
            "{}",
            outcome.output
        );
        assert!(started.is_empty(), "a running agent needs no new run");
        assert_eq!(registry.take_pending_inputs(&id), ["also check gists"]);
    }

    #[test]
    fn agentsend_resumes_a_settled_agent_through_the_launcher() {
        // The follow-up the feature exists for: a finished agent keeps its
        // context, and the message continues it.
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent("explore", "Fetch profile", "p");
        registry.finish(&id, Ok("done".into()), vec![ChatMessage::user("p")]);
        let (outcome, started, _) = run(
            &registry,
            &CancelToken::new(),
            None,
            &call(
                tools::AGENT_SEND_TOOL,
                serde_json::json!({"agent_id": id, "message": "now list the repos"}),
            ),
        );
        assert!(outcome.ok, "{}", outcome.output);
        assert!(outcome.output.contains("resumed"), "{}", outcome.output);
        assert!(outcome.output.contains("notified"), "{}", outcome.output);
        let [continuation] = started.as_slice() else {
            panic!("one continuation started: {started:?}");
        };
        assert_eq!(continuation.id, id);
        assert_eq!(continuation.agent_type, "explore");
        assert_eq!(continuation.messages, [ChatMessage::user("p")]);
        assert_eq!(registry.take_pending_inputs(&id), ["now list the repos"]);
    }

    #[test]
    fn agentsend_refuses_an_agent_the_user_stopped() {
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        let _ = registry.kill(&id);
        registry.finish(&id, Err("stopped".into()), vec![ChatMessage::user("p")]);
        let (outcome, started, _) = run(
            &registry,
            &CancelToken::new(),
            None,
            &call(
                tools::AGENT_SEND_TOOL,
                serde_json::json!({"agent_id": id, "message": "go on"}),
            ),
        );
        assert!(!outcome.ok);
        assert!(
            outcome.output.contains("stopped by the user"),
            "{}",
            outcome.output
        );
        assert!(started.is_empty());
    }

    #[test]
    fn agentsend_waits_for_a_stopping_agent_to_let_go() {
        // kill-then-send in one message: the stop lands, the agent's thread
        // takes a moment to return, and the send resumes it once it has.
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        let _ = registry.stop(&id, None);
        let finisher = {
            let registry = registry.clone();
            let id = id.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(60));
                registry.finish(&id, Err("stopped".into()), vec![ChatMessage::user("p")]);
            })
        };
        let (outcome, started, _) = run(
            &registry,
            &CancelToken::new(),
            None,
            &call(
                tools::AGENT_SEND_TOOL,
                serde_json::json!({"agent_id": id, "message": "do this instead"}),
            ),
        );
        finisher.join().unwrap();
        assert!(outcome.ok, "{}", outcome.output);
        assert_eq!(started.len(), 1);
    }

    #[test]
    fn a_call_naming_no_agent_here_lists_the_ones_that_are() {
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "Fetch profile", "p");
        for name in [
            tools::AGENT_SEND_TOOL,
            tools::AGENT_OUTPUT_TOOL,
            tools::AGENT_KILL_TOOL,
        ] {
            let (outcome, _, _) = run(
                &registry,
                &CancelToken::new(),
                None,
                &call(
                    name,
                    serde_json::json!({"agent_id": "a000nope0", "message": "hi"}),
                ),
            );
            assert!(!outcome.ok, "{name}");
            assert!(
                outcome.output.contains("a000nope0"),
                "{name}: {}",
                outcome.output
            );
            assert!(
                outcome.output.contains(&id) && outcome.output.contains("Fetch profile"),
                "{name} names the real agents: {}",
                outcome.output
            );
        }
    }

    #[test]
    fn an_empty_message_sends_nothing() {
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        let (outcome, _, _) = run(
            &registry,
            &CancelToken::new(),
            None,
            &call(
                tools::AGENT_SEND_TOOL,
                serde_json::json!({"agent_id": id, "message": "  "}),
            ),
        );
        assert!(!outcome.ok);
        assert!(registry.take_pending_inputs(&id).is_empty());
    }

    #[test]
    fn agentoutput_reports_progress_at_once_by_default() {
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "Fetch profile", "p");
        registry.record_call(&id, "Bash(curl -s https://api.github.com/users/linuztx)");
        let started = Instant::now();
        let (outcome, _, _) = run(
            &registry,
            &CancelToken::new(),
            None,
            &call(
                tools::AGENT_OUTPUT_TOOL,
                serde_json::json!({"agent_id": id}),
            ),
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "no wait asked for"
        );
        assert!(outcome.ok, "{}", outcome.output);
        // A look at a running agent says it is still running, and how to wait
        // for it — a model that looked without `wait` once told the user the
        // agent had stopped.
        assert_eq!(
            outcome.output,
            format!(
                "{}\n\n{STILL_RUNNING_NOTE}",
                crate::agents::output_report(&registry.snapshot(&id).unwrap())
            )
        );
        // A settled one is just its report.
        registry.finish(&id, Ok("done".into()), Vec::new());
        let (settled, _, _) = run(
            &registry,
            &CancelToken::new(),
            None,
            &call(
                tools::AGENT_OUTPUT_TOOL,
                serde_json::json!({"agent_id": id}),
            ),
        );
        assert_eq!(
            settled.output,
            crate::agents::output_report(&registry.snapshot(&id).unwrap())
        );
    }

    #[test]
    fn a_waiting_agentoutput_reports_the_settle_and_owes_no_notice() {
        let (registry, mut rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "Fetch profile", "p");
        registry.record_call(&id, "Bash(curl one)");
        let worker = {
            let registry = registry.clone();
            let id = id.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(80));
                registry.record_call(&id, "Bash(curl two\n  --silent)");
                std::thread::sleep(Duration::from_millis(80));
                registry.settle(
                    &id,
                    Ok("It has 12 repos.".into()),
                    Vec::new(),
                    Some(StreamEvent::StreamDone),
                );
            })
        };
        let (outcome, _, streamed) = run(
            &registry,
            &CancelToken::new(),
            None,
            &call(
                tools::AGENT_OUTPUT_TOOL,
                serde_json::json!({"agent_id": id, "wait": 30}),
            ),
        );
        worker.join().unwrap();
        assert!(outcome.ok, "{}", outcome.output);
        assert!(
            outcome.output.starts_with("Done (agent"),
            "{}",
            outcome.output
        );
        assert!(
            outcome.output.ends_with("It has 12 repos."),
            "{}",
            outcome.output
        );
        assert_eq!(
            streamed,
            ["Bash(curl two --silent)\n"],
            "each call streams once, on one line, as it starts — the ones before \
             the wait are the report's, never shown as new"
        );
        assert_eq!(
            drain(&mut rx),
            [AgentEvent::Settled {
                id,
                event: StreamEvent::StreamDone,
                observed: true,
                run: 0,
            }],
            "the report was the notice"
        );
    }

    #[test]
    fn agentoutput_hands_the_lead_what_the_agent_said_on_its_way() {
        // docs/agent-progress.md: the lead reads the agent's own account of
        // its progress, not only the calls it made.
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "Fetch profile", "p");
        registry.record_update(&id, "I'll start with the profile.");
        registry.record_call(&id, "Bash(curl -s https://api.github.com/users/linuztx)");
        registry.record_update(&id, "It lists 42 public repos.");
        let (outcome, _, _) = run(
            &registry,
            &CancelToken::new(),
            None,
            &call(
                tools::AGENT_OUTPUT_TOOL,
                serde_json::json!({"agent_id": id}),
            ),
        );
        assert!(outcome.ok, "{}", outcome.output);
        let lines: Vec<&str> = outcome.output.lines().collect();
        assert_eq!(
            lines[1..4],
            [
                "Agent: I'll start with the profile.",
                "Bash(curl -s https://api.github.com/users/linuztx)",
                "Agent: It lists 42 public repos.",
            ],
            "{}",
            outcome.output
        );
    }

    #[test]
    fn a_waiting_agentoutput_streams_the_agents_updates_among_its_calls() {
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "Fetch profile", "p");
        registry.record_update(&id, "I'll start with the profile.");
        registry.record_call(&id, "Bash(curl one)");
        let worker = {
            let registry = registry.clone();
            let id = id.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(80));
                registry.record_update(&id, "Found the profile.\n\nNow the repositories.");
                registry.record_call(&id, "Bash(curl two)");
                // A message it reads while watched is part of what it did:
                // its loop takes it, and the forwarder relays the announcement.
                assert!(registry.queue_input(&id, "also the stars"));
                let _ = registry.take_pending_inputs(&id);
                registry.announce_read(&id);
                std::thread::sleep(Duration::from_millis(80));
                registry.settle(
                    &id,
                    Ok("It has 12 repos.".into()),
                    Vec::new(),
                    Some(StreamEvent::StreamDone),
                );
            })
        };
        let (outcome, _, streamed) = run(
            &registry,
            &CancelToken::new(),
            None,
            &call(
                tools::AGENT_OUTPUT_TOOL,
                serde_json::json!({"agent_id": id, "wait": 30}),
            ),
        );
        worker.join().unwrap();
        assert_eq!(
            streamed,
            [
                "Agent: Found the profile. Now the repositories.\n",
                "Bash(curl two)\n",
                "— message received —\n"
            ],
            "what the agent says, starts and reads while watched streams in \
             its place, one line each — what it did before the wait is the \
             report's"
        );
        assert!(
            outcome.output.contains(
                "Agent: I'll start with the profile.\nBash(curl one)\n\
                 Agent: Found the profile.\n  Now the repositories.\nBash(curl two)\n\
                 — message received —\n"
            ),
            "{}",
            outcome.output
        );
    }

    /// A finished agent whose completion went out unobserved — the settle a
    /// wait just missed, or one that landed before the lead looked.
    fn settled_unobserved() -> (AgentRegistry, String, BackgroundRegistry) {
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        registry.settle(
            &id,
            Ok("It has 12 repos.".into()),
            vec![ChatMessage::user("p")],
            Some(StreamEvent::StreamDone),
        );
        let (bg_tx, _bg_rx) = tokio::sync::mpsc::unbounded_channel();
        let background = BackgroundRegistry::new(bg_tx, std::env::temp_dir());
        (registry, id, background)
    }

    #[test]
    fn a_report_that_beats_the_notice_cancels_it() {
        // The settle landed just before the wait could register, so its
        // notice went out unobserved; the report hands the lead the answer,
        // and the loop must then post nothing (docs/agent-tools.md *One
        // notice per answer*).
        let (registry, id, background) = settled_unobserved();
        let (outcome, _, _) = run(
            &registry,
            &CancelToken::new(),
            Some(&background),
            &call(
                tools::AGENT_OUTPUT_TOOL,
                serde_json::json!({"agent_id": id, "wait": 60}),
            ),
        );
        assert!(
            outcome.output.ends_with("It has 12 repos."),
            "{}",
            outcome.output
        );
        assert_eq!(
            registry.post_notice(&id, 0, &background, Some("note".into())),
            crate::agents::NoticePost::Covered
        );
        assert!(background.take_pending_notices().is_empty());
    }

    #[test]
    fn a_report_takes_back_a_notice_the_lead_has_not_read() {
        let (registry, id, background) = settled_unobserved();
        let crate::agents::NoticePost::Posted(Some(seq)) =
            registry.post_notice(&id, 0, &background, Some("note".into()))
        else {
            panic!("the settle owed a notice");
        };
        let (outcome, _, _) = run(
            &registry,
            &CancelToken::new(),
            Some(&background),
            &call(
                tools::AGENT_OUTPUT_TOOL,
                serde_json::json!({"agent_id": id}),
            ),
        );
        assert!(
            outcome.output.ends_with("It has 12 repos."),
            "{}",
            outcome.output
        );
        assert!(
            background.take_pending_notices().is_empty(),
            "off the board"
        );
        assert!(background.take_retracted(seq), "and its cell is not owed");
    }

    /// A failed agent the user brought back with "continue with React" while
    /// the lead was mid-round — the note that it runs again posted, unread.
    fn resumed_by_user() -> (AgentRegistry, String, BackgroundRegistry, u64) {
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        registry.finish(&id, Err("refused".into()), vec![ChatMessage::user("p")]);
        let (bg_tx, _bg_rx) = tokio::sync::mpsc::unbounded_channel();
        let background = BackgroundRegistry::new(bg_tx, std::env::temp_dir());
        let _ = registry.begin_continuation(&id).expect("settled");
        registry.note_user_message(&id, "continue with React");
        let seq = registry
            .post_resume_notice(&id, &background, "running again".into())
            .expect("the run is going");
        (registry, id, background, seq)
    }

    #[test]
    fn a_look_at_the_resumed_runs_end_takes_back_the_note_that_it_runs() {
        // The resumed run ends and the lead looks before the loop has handled
        // the settle: the report hands it the answer, so the note that the
        // agent runs again must not reach it after (docs/agent-tools.md *A
        // resume is news when it starts*).
        let (registry, id, background, seq) = resumed_by_user();
        registry.settle(
            &id,
            Ok("Frontend created.".into()),
            vec![ChatMessage::user("p")],
            Some(StreamEvent::StreamDone),
        );
        let (outcome, _, _) = run(
            &registry,
            &CancelToken::new(),
            Some(&background),
            &call(
                tools::AGENT_OUTPUT_TOOL,
                serde_json::json!({"agent_id": id}),
            ),
        );
        assert!(
            outcome.output.ends_with("Frontend created."),
            "{}",
            outcome.output
        );
        assert!(background.take_pending_notices().is_empty());
        assert!(background.take_retracted(seq), "and its cell is not owed");
        assert_eq!(
            registry.post_notice(&id, 1, &background, Some("note".into())),
            crate::agents::NoticePost::Covered
        );
    }

    #[test]
    fn agentkill_takes_back_the_note_that_the_user_resumed_the_run() {
        // The lead stops the agent before reading the note: the stop's report
        // is the run's end — the user's message marked in it — and no notice
        // follows a stop the lead made, so "running again, you will be
        // notified" must not reach it after (docs/agent-tools.md).
        let (registry, id, background, seq) = resumed_by_user();
        let (outcome, _, _) = run(
            &registry,
            &CancelToken::new(),
            Some(&background),
            &call(tools::AGENT_KILL_TOOL, serde_json::json!({"agent_id": id})),
        );
        assert!(
            outcome.output.starts_with("Stopped (agent"),
            "{}",
            outcome.output
        );
        assert!(
            outcome
                .output
                .contains("— message from the user: continue with React —"),
            "{}",
            outcome.output
        );
        assert!(background.take_pending_notices().is_empty());
        assert!(background.take_retracted(seq), "and its cell is not owed");
    }

    #[test]
    fn agentkill_of_a_resumed_agent_already_done_takes_back_its_note() {
        // The resumed run finished before the lead read the note: the kill
        // stops nothing and its report says the agent is done, so the note
        // that it runs again must not follow it (docs/agent-tools.md).
        let (registry, id, background, seq) = resumed_by_user();
        registry.finish(
            &id,
            Ok("Frontend created.".into()),
            vec![ChatMessage::user("p")],
        );
        let (outcome, _, _) = run(
            &registry,
            &CancelToken::new(),
            Some(&background),
            &call(tools::AGENT_KILL_TOOL, serde_json::json!({"agent_id": id})),
        );
        assert!(
            outcome.output.contains("is not running"),
            "{}",
            outcome.output
        );
        assert!(background.take_pending_notices().is_empty());
        assert!(background.take_retracted(seq), "and its cell is not owed");
    }

    #[test]
    fn a_report_carries_an_earlier_runs_unread_answer_after_its_own() {
        // The run the report covers came last; the one before it ended
        // unread, and its notice — taken off the board — follows the report,
        // marked as the earlier answer, where it used to be thrown away
        // (docs/agent-tools.md *One notice per answer*).
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "Create the frontend", "p");
        let (bg_tx, _bg_rx) = tokio::sync::mpsc::unbounded_channel();
        let background = BackgroundRegistry::new(bg_tx, std::env::temp_dir());
        registry.settle(
            &id,
            Ok("Frontend created.".into()),
            vec![ChatMessage::user("p")],
            Some(StreamEvent::StreamDone),
        );
        let earlier = "[background agent] Agent \"Create the frontend\" completed.\n\
                       Final response:\nFrontend created.";
        let crate::agents::NoticePost::Posted(Some(seq)) =
            registry.post_notice(&id, 0, &background, Some(earlier.into()))
        else {
            panic!("the first run owed a notice");
        };
        let _ = registry.begin_continuation(&id).expect("settled");
        registry.settle(
            &id,
            Ok("Added tests.".into()),
            vec![ChatMessage::user("p")],
            Some(StreamEvent::StreamDone),
        );
        let (outcome, _, _) = run(
            &registry,
            &CancelToken::new(),
            Some(&background),
            &call(
                tools::AGENT_OUTPUT_TOOL,
                serde_json::json!({"agent_id": id}),
            ),
        );
        let output = &outcome.output;
        let own = output
            .find("Final response:\nAdded tests.")
            .unwrap_or_else(|| panic!("the report's own answer: {output}"));
        let lead_in = output
            .find(EARLIER_RUN_NOTE)
            .unwrap_or_else(|| panic!("the earlier answer is introduced: {output}"));
        let carried = output
            .find(earlier)
            .unwrap_or_else(|| panic!("the earlier answer is carried: {output}"));
        assert!(own < lead_in && lead_in < carried, "{output}");
        assert!(background.take_pending_notices().is_empty());
        assert!(background.take_retracted(seq), "and its cell is not owed");
    }

    #[test]
    fn a_running_agents_report_cancels_no_notice() {
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        let (bg_tx, _bg_rx) = tokio::sync::mpsc::unbounded_channel();
        let background = BackgroundRegistry::new(bg_tx, std::env::temp_dir());
        let _ = run(
            &registry,
            &CancelToken::new(),
            Some(&background),
            &call(
                tools::AGENT_OUTPUT_TOOL,
                serde_json::json!({"agent_id": id}),
            ),
        );
        registry.settle(
            &id,
            Ok("later".into()),
            vec![ChatMessage::user("p")],
            Some(StreamEvent::StreamDone),
        );
        assert!(
            matches!(
                registry.post_notice(&id, 0, &background, Some("note".into())),
                crate::agents::NoticePost::Posted(Some(_))
            ),
            "the answer it did not report is still noticed"
        );
    }

    #[test]
    fn ctrl_b_ends_a_wait_with_the_agent_running_on() {
        let (registry, _rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        let (bg_tx, _bg_rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = std::env::temp_dir().join("alter-zero-agent-tools-test");
        let background = BackgroundRegistry::new(bg_tx, dir);
        let latch = background.clone();
        let presser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            latch.request_background();
        });
        let started = Instant::now();
        let (outcome, _, _) = run(
            &registry,
            &CancelToken::new(),
            Some(&background),
            &call(
                tools::AGENT_OUTPUT_TOOL,
                serde_json::json!({"agent_id": id, "wait": 600}),
            ),
        );
        presser.join().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the key ended it"
        );
        assert!(outcome.ok, "{}", outcome.output);
        assert!(
            outcome.output.starts_with("Running (agent"),
            "{}",
            outcome.output
        );
        assert!(
            outcome.output.contains("ended this wait"),
            "{}",
            outcome.output
        );
        assert!(!registry.is_done(&id), "the agent runs on");
    }

    #[test]
    fn a_cancelled_wait_leaves_the_completion_notice_owed() {
        let (registry, mut rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        let cancel = CancelToken::new();
        cancel.cancel();
        let (_outcome, _, _) = run(
            &registry,
            &cancel,
            None,
            &call(
                tools::AGENT_OUTPUT_TOOL,
                serde_json::json!({"agent_id": id, "wait": 600}),
            ),
        );
        registry.settle(
            &id,
            Ok("done".into()),
            Vec::new(),
            Some(StreamEvent::StreamDone),
        );
        assert_eq!(
            drain(&mut rx),
            [AgentEvent::Settled {
                id,
                event: StreamEvent::StreamDone,
                observed: false,
                run: 0,
            }]
        );
    }

    #[test]
    fn agentkill_stops_a_running_agent_and_reports_where_it_got_to() {
        let (registry, mut rx) = registry();
        let (id, agent_cancel) = registry.register_agent(GENERAL_PURPOSE, "Fetch profile", "p");
        registry.record_call(&id, "Bash(curl one)");
        let (outcome, _, _) = run(
            &registry,
            &CancelToken::new(),
            None,
            &call(tools::AGENT_KILL_TOOL, serde_json::json!({"agent_id": id})),
        );
        assert!(outcome.ok, "{}", outcome.output);
        assert!(agent_cancel.is_cancelled(), "its loop is cancelled");
        assert!(
            outcome.output.starts_with("Stopped (agent"),
            "{}",
            outcome.output
        );
        assert!(
            outcome.output.contains("Bash(curl one)"),
            "{}",
            outcome.output
        );
        assert!(
            outcome.output.contains("agentsend resumes it"),
            "{}",
            outcome.output
        );
        assert_eq!(drain(&mut rx), [AgentEvent::Stopped { id }]);
    }

    #[test]
    fn agentkill_on_a_settled_agent_stops_nothing() {
        let (registry, mut rx) = registry();
        let (id, _cancel) = registry.register_agent(GENERAL_PURPOSE, "d", "p");
        registry.finish(&id, Ok("done".into()), Vec::new());
        let (outcome, _, _) = run(
            &registry,
            &CancelToken::new(),
            None,
            &call(tools::AGENT_KILL_TOOL, serde_json::json!({"agent_id": id})),
        );
        assert!(outcome.ok, "{}", outcome.output);
        assert!(outcome.output.contains("not running"), "{}", outcome.output);
        assert!(drain(&mut rx).is_empty(), "nothing to tell the roster");
        assert!(matches!(
            registry.snapshot(&id).unwrap().state,
            AgentState::Done(_)
        ));
    }

    #[test]
    fn agentlist_names_every_agent() {
        let (registry, _rx) = registry();
        let (first, _c1) = registry.register_agent(GENERAL_PURPOSE, "Fetch profile", "p");
        let (second, _c2) = registry.register_agent("explore", "Search the repo", "p");
        let (outcome, _, _) = run(
            &registry,
            &CancelToken::new(),
            None,
            &call(tools::AGENT_LIST_TOOL, serde_json::json!({})),
        );
        assert!(outcome.ok);
        assert_eq!(
            outcome.output,
            crate::agents::list_report(&registry.snapshots())
        );
        assert!(outcome.output.contains(&first) && outcome.output.contains(&second));
    }
}
