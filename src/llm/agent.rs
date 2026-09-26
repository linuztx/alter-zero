//! The agentic tool-calling loop — a **pure, generic driver** (like
//! [`crate::llm::retry`]'s `run_stream`) so the whole loop is unit-tested with
//! fakes and no network. See `docs/tools.md`.
//!
//! [`run_agent`] drives a turn that may call tools: it asks the `round` closure
//! to stream one model response, and if the model requested tool calls it emits
//! the [`StreamEvent::ToolStart`]/[`StreamEvent::ToolEnd`] pair the TUI already
//! knows how to render, runs each call through the `execute` closure, appends
//! the results to the running message list, and loops — until the model answers
//! with plain text (`StreamDone`), fails (`Error`), is cancelled (silent), or
//! the tool-call cap trips.

use tokio::sync::mpsc::UnboundedSender;

use super::exec::ToolProgress;
use super::hooks::{HookSink, block_texts as hook_block_texts};
use super::tools::{
    ToolCallRequest, ToolOutcome, display_name, image_attachment_note, summarize_call,
    summarize_call_naming,
};
use super::{ChatMessage, ContentPart, LlmError};
use crate::permission::Approval;
use crate::stream::{CancelToken, RoundCall, StreamEvent, ToolCallSummary};

/// The most tool **calls** one turn will run before giving up — a backstop
/// against a model that loops forever. Generous enough for real multi-step
/// tasks. Counted per call, not per round: a round can request a whole
/// parallel batch (`docs/parallel-tools.md`), and counting rounds let one
/// round overspend the ceiling several times over.
///
/// This is the **library's** default ([`LlmBackend::with_max_tool_calls`]
/// overrides it, and `0` means no limit at all). The app ships uncapped: the
/// `/settings` **Max tool calls** row defaults to `0`, because cutting a long
/// agentic task off part-way leaves its work half-done, and Esc is already the
/// stop button. See `docs/settings.md`.
///
/// [`LlmBackend::with_max_tool_calls`]: super::LlmBackend::with_max_tool_calls
pub const MAX_TOOL_ITERATIONS: usize = 20;

/// What a call refused by the **Max tool calls** ceiling resolves with — the
/// cell's line and the tool result alike. One sentence, stating the fact.
///
/// The advice — raise it in `/settings`, `0` for no limit — is deliberately
/// *not* here, even though the model reads this. It is already in the turn's
/// closing error, and that error reaches the model too: a [`crate::app::Role::Error`]
/// notice derives into an `[error] …` user entry in the context
/// (`crate::context`). Repeating it per refused call would say the same thing
/// twice — and a clamped batch refuses several at once, so it would cost
/// several paragraphs of screen and of context window for one fact. See
/// `docs/settings.md`.
pub const TOOL_LIMIT_OUTPUT: &str = "Not run: this turn had already spent its tool-call limit.";

/// The error that ends a turn which spent its tool-call budget.
///
/// The user meets this as a bare red notice — no cell above it explaining the
/// context, no hint row below — so it carries the whole story itself: **what**
/// stopped the turn, **why** (a ceiling they set, not a model or provider
/// failure), and **how** to change it. Naming the row's own label and the
/// command means they can act on it without going looking. See
/// `docs/settings.md`.
fn limit_error(max_tool_calls: usize) -> String {
    format!(
        "Stopped after {max_tool_calls} tool calls — this turn hit the \
         \"Max tool calls\" limit before the model finished. \
         Run /settings to raise it, or set it to 0 for no limit."
    )
}

/// What one streaming round produced, as [`run_agent`] sees it. The `round`
/// closure emits the `Chunk`/`Thinking*` events itself; this only reports the
/// disposition and — for a tool-calling round — the assistant message to append
/// plus the calls to run.
pub enum RoundOutcome {
    /// The assistant answered with plain text (no tool calls) — the turn is
    /// done, unless a `Stop` hook blocks (`docs/hooks.md`). `text` is the
    /// round's full reply: the stop payload's `last_assistant_message`, and —
    /// on a continuation — the assistant message the next round's context
    /// keeps.
    Complete { text: String },
    /// The assistant requested tool calls. `assistant` is the message to append
    /// verbatim (carrying its `tool_calls`); `calls` are the parsed requests.
    ToolCalls {
        assistant: ChatMessage,
        calls: Vec<ToolCallRequest>,
    },
    /// The request was cancelled (Esc/quit) — stop silently, the UI owns the notice.
    Cancelled,
    /// The request failed.
    Failed(LlmError),
}

/// One thing waiting to be folded into the next round's context, taken at
/// every round boundary by [`run_agent`]'s `pending_inputs` seam. Both
/// variants become a user-role message; what separates them is whether the
/// *user* said it.
///
/// The queue behind them is per-conversation: the session's own
/// [`SteerQueue`](crate::steer::SteerQueue) for the main turn, the agent
/// registry's for a subagent — the same mechanism, twice (`docs/queue.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingInput {
    /// A background shell or agent finished (`docs/background.md`) — the note
    /// the model reads. Invisible: the loop already recorded the completion,
    /// so nothing is announced.
    Notice(String),
    /// A message the **user** queued while the turn was running
    /// (`docs/queue.md`). Announced with [`StreamEvent::Steered`] as it is
    /// taken, so the loop can turn its queued row into a real user bubble.
    User(String),
}

/// Drive one agentic turn to completion, sending the terminal `StreamDone` /
/// `Error` (or nothing on a cancel) and the per-tool `ToolStart`/`ToolEnd`
/// events. Generic over `round` (one streaming request), `execute` (running
/// a tool), and `pending_inputs` (what the conversation gained since the last
/// request) so it is fully unit-tested with fakes.
///
/// `messages` is the initial request list (system prompt + conversation
/// context); it grows in place with each assistant/tool message as the loop
/// runs, so every round sees the full history.
///
/// `pending_inputs` is taken at the top of **every** round, and it is what
/// makes a turn *reachable while it runs*. Two kinds of
/// [`PendingInput`] arrive there, both appended as user-role messages after
/// the prior round's tool results (the same form `context::context_messages`
/// replays into later turns' contexts):
///
/// - [`PendingInput::Notice`] — a background shell or agent that finished
///   since the last request: a completion, or a kill the model itself just
///   ran (`kill`/`pkill` in a bash call, or the user's `x` in the ↓ manager).
///   Invisible; the loop already recorded it (`docs/background.md`).
/// - [`PendingInput::User`] — a message the user queued **while this turn was
///   running**. Announced with [`StreamEvent::Steered`] as it is taken, so
///   the loop turns its queued row into a real user bubble at the moment the
///   model genuinely has it (`docs/queue.md`).
///
/// Notices lead, the user's own messages close: a completion is a *result*
/// and belongs with the results it follows, while the newest thing the user
/// said must be the last thing the model reads. The take sits after the
/// cancel check so an abandoned turn can't steal notes owed to the boundary's
/// automatic follow-up turn — nor a queued message the boundary is about to
/// re-dispatch as the next turn.
///
/// `approve` is the permission gate (`docs/permissions.md`), consulted for
/// every ordinary call **before** its `ToolStart` — so nothing has run, and
/// nothing shows as running, while the user decides. An
/// [`Approval::Reject`] resolves the call without executing it: the
/// Start/End pair still goes out (with the short `display` output, red) so
/// the cell lands in history and the transcript, while the longer `result`
/// becomes the tool result the model reads. Its `bool` argument is a
/// `PreToolUse` hook's `permissionDecision: "ask"` — *put this to the user
/// even if a standing rule would have allowed it* (`docs/hooks.md`).
///
/// `hooks` is the lifecycle-hook seam (`docs/hooks.md`), consulted twice per
/// call: `PreToolUse` **before** `approve` (so a hook can refuse, rewrite the
/// arguments, or pre-approve without the user) and `PostToolUse` on the
/// outcome (so a hook can feed the model something about what just happened).
/// [`NoHooks`](super::hooks::NoHooks) is the do-nothing default and costs
/// one vtable dispatch.
///
/// `session_command` maps a session id to the command that session runs —
/// the lookup the permission prompt makes — so a `bash_session` call's
/// header names the program its keys go to from the moment the call is
/// announced, above its prompt and on a refused cell alike
/// (`docs/interactive-shell.md`). One that knows no sessions keeps the id.
#[allow(clippy::too_many_arguments)] // the loop's full seam set (docs/agent-tool.md)
pub fn run_agent(
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
    max_tool_calls: usize,
    messages: &mut Vec<ChatMessage>,
    mut round: impl FnMut(&[ChatMessage]) -> RoundOutcome,
    mut execute: impl FnMut(&ToolCallRequest, &mut dyn FnMut(ToolProgress<'_>)) -> ToolOutcome,
    mut pending_inputs: impl FnMut() -> Vec<PendingInput>,
    mut run_agents: impl FnMut(&[ToolCallRequest]) -> Vec<(String, String)>,
    mut approve: impl FnMut(&ToolCallRequest, bool) -> Approval,
    hooks: &dyn HookSink,
    session_command: &dyn Fn(&str) -> Option<String>,
) {
    let mut used_calls = 0usize;
    // True from the first Stop/SubagentStop-forced continuation on — the
    // payloads' loop-guard flag (docs/hooks.md).
    let mut stop_hook_active = false;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        // Notices first, the user's own messages last: a completion note is a
        // *result*, belonging with the tool results it follows, while what the
        // user just said is the newest thing in the conversation and must be
        // the last thing the model reads.
        let (notices, steered): (Vec<_>, Vec<_>) = pending_inputs()
            .into_iter()
            .partition(|input| matches!(input, PendingInput::Notice(_)));
        for input in notices.into_iter().chain(steered) {
            match input {
                PendingInput::Notice(note) => messages.push(ChatMessage::user(&note)),
                // The queued row above the user's box becomes a real bubble
                // the moment the model actually has the text (docs/queue.md).
                PendingInput::User(text) => {
                    let _ = tx.send(StreamEvent::Steered { text: text.clone() });
                    messages.push(ChatMessage::user(&text));
                }
            }
        }
        match round(messages) {
            RoundOutcome::Complete { text } => {
                // `Stop` / `SubagentStop` (docs/hooks.md), fired where "the
                // model finished answering" is native — and *only* here:
                // never on a cancel (Claude Code returns before stop hooks
                // on an abort), an error (its reference fires StopFailure,
                // which we don't model), a `/compact` turn (that backend
                // carries NoHooks) or a `!` shell (no agent loop). A block
                // is continuation feedback: the reply so far becomes an
                // assistant message, the feedback the next user message —
                // recorded via HookNote so the transcript and every later
                // turn's context keep it — and the SAME turn runs another
                // round. StreamDone waits, so the turn-end checkpoint lands
                // after every continuation and a backtrack restores the
                // hook-driven work too. The engine never refuses a re-block:
                // `stop_hook_active` is the hook's own guard (the
                // reference's exact posture), and Esc stays the stop button
                // (the runner polls the token; an interrupt never fires
                // Stop, so it always breaks the chain).
                if !cancel.is_cancelled()
                    && let Some(reason) = hooks.stop(stop_hook_active, &text, cancel)
                {
                    let (label, feedback) = super::hooks::stop_feedback_texts(&reason);
                    let _ = tx.send(StreamEvent::HookNote {
                        label,
                        text: feedback.clone(),
                    });
                    if !text.trim().is_empty() {
                        messages.push(ChatMessage::new("assistant", &text));
                    }
                    messages.push(ChatMessage::user(&feedback));
                    stop_hook_active = true;
                    continue;
                }
                let _ = tx.send(StreamEvent::StreamDone);
                return;
            }
            RoundOutcome::Cancelled => return,
            RoundOutcome::Failed(err) => {
                let _ = tx.send(StreamEvent::Error(err.to_string()));
                return;
            }
            RoundOutcome::ToolCalls { assistant, calls } => {
                // The cap bounds TOOL CALLS, not the final answer: once the
                // budget is spent the model still gets one more request, and a
                // plain-text reply there completes the turn — only a further
                // tool request trips the error (docs/tools.md). Checking here,
                // before the round's tools run, also keeps a call the budget
                // can no longer afford from executing just to have its result
                // discarded.
                //
                // **Zero lifts the backstop entirely** — the `/settings`
                // **Max tool calls** default (docs/settings.md): a long
                // agentic task runs to its own end rather than being cut off
                // part-way with its work half-done. The user's Esc is the
                // stop button; the cap is for those who want a hard ceiling.
                if max_tool_calls > 0 && used_calls >= max_tool_calls {
                    let _ = tx.send(StreamEvent::Error(limit_error(max_tool_calls)));
                    return;
                }
                messages.push(assistant);
                // **The budget is spent per CALL, not per round.** A round can
                // request a whole parallel batch (docs/parallel-tools.md), so
                // counting rounds let one round overspend the ceiling several
                // times over — a `Max tool calls` of 5 running 15 calls, the
                // reported bug. Take as many of this round's calls as the
                // budget still allows, **in the model's own order**; the rest
                // are refused below, answered so the stored message list stays
                // well-formed, and the turn then ends.
                //
                // Clamping rather than refusing the whole round matters: a
                // model that opens with a batch wider than the entire ceiling
                // would otherwise do nothing at all and just error.
                let budget = if max_tool_calls == 0 {
                    calls.len()
                } else {
                    max_tool_calls.saturating_sub(used_calls)
                };
                let allowed_n = calls.len().min(budget);
                let (allowed, refused) = calls.split_at(allowed_n);
                used_calls += allowed_n;
                // The round's `agent` calls take their own path
                // (`docs/agent-tool.md`): the `run_agents` closure launches
                // them all **concurrently**, emits the AgentBatch /
                // AgentGroupDone events, waits for foreground completion
                // (polling this same cancel), and returns each call's tool
                // result. The ordinary calls run sequentially exactly as
                // before; every result then appends in the model's original
                // call order (strict providers pair results by contiguity).
                let (agent_calls, rest): (Vec<&ToolCallRequest>, Vec<&ToolCallRequest>) = allowed
                    .iter()
                    .partition(|call| call.name == super::tools::AGENT_TOOL_NAME);
                // The over-budget ordinary calls: shown and resolved red below
                // rather than dropped, so the cell says why nothing ran. A
                // refused *task* call is answered but — like a refused agent
                // call — shows no cell: it never had one to begin with
                // (docs/task-tools.md).
                let refused_rest: Vec<&ToolCallRequest> = refused
                    .iter()
                    .filter(|call| {
                        call.name != super::tools::AGENT_TOOL_NAME
                            && !crate::tasks::is_task_tool(&call.name)
                    })
                    .collect();
                // The round's wire identity, ahead of anything else it emits
                // (docs/prompt-caching.md): every call that resolves into a
                // record — the allowed ones of every kind in the model's
                // order, then the refused ordinary ones, which still get a
                // cell — under the provider's own id, so the app stamps the
                // records those cells become. A refused task or agent call
                // gets no record and is left out.
                let _ = tx.send(StreamEvent::RoundCalls(
                    allowed
                        .iter()
                        .chain(refused_rest.iter().copied())
                        .map(|call| RoundCall {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                        })
                        .collect(),
                ));
                let mut results: Vec<(String, String)> = Vec::with_capacity(calls.len());
                if !agent_calls.is_empty() {
                    // `PreToolUse` gates a subagent launch too — Claude Code
                    // fires it for its Task tool, and `Task` aliases to our
                    // `agent` in the matcher (docs/hooks.md). A block refuses
                    // the launch as an ordinary red cell; `updatedInput`
                    // rewrites it. The gate-side verdicts (`pre_approved`,
                    // `force_ask`) have nothing to act on — a launch never
                    // asks permission — and a context lands nowhere: the
                    // launcher owns the call's result.
                    let mut launchable: Vec<ToolCallRequest> = Vec::new();
                    for call in agent_calls {
                        let hook = hooks.pre_tool_use(call, cancel);
                        if let Some(reason) = &hook.blocked {
                            let (display, result) = hook_block_texts(reason);
                            let _ = tx.send(tool_start_event(call, session_command));
                            let _ = tx.send(StreamEvent::ToolRejected {
                                display,
                                result: result.clone(),
                                truncated: false,
                            });
                            results.push((call.id.clone(), result));
                            continue;
                        }
                        launchable.push(match hook.updated_input {
                            Some(arguments) => ToolCallRequest {
                                id: call.id.clone(),
                                name: call.name.clone(),
                                arguments,
                            },
                            None => call.clone(),
                        });
                    }
                    if !launchable.is_empty() {
                        results.extend(run_agents(&launchable));
                    }
                }
                // Announce the ordinary batch up front — before any tool runs
                // — so the UI shows every requested call at once, the ones not
                // yet executing as `⎿ Waiting…`. Each entry's (name, args)
                // equals the matching ToolStart's; execution below is still
                // sequential. The refused tail rides the same announcement, so
                // a clamped batch still shows whole. See `docs/parallel-tools.md`.
                // Task calls are left out: they render no cell anywhere — the
                // live checklist is their display (docs/task-tools.md).
                let announced: Vec<&ToolCallRequest> = rest
                    .iter()
                    .copied()
                    .filter(|call| !crate::tasks::is_task_tool(&call.name))
                    .chain(refused_rest.iter().copied())
                    .collect();
                if !announced.is_empty() {
                    let _ = tx.send(StreamEvent::ToolBatch(
                        announced
                            .iter()
                            .map(|call| ToolCallSummary {
                                name: display_name(&call.name),
                                args: summarize_call_naming(
                                    &call.name,
                                    &call.arguments,
                                    session_command,
                                ),
                            })
                            .collect(),
                    ));
                }
                // An image `read`'s pixels: collected per call and attached
                // AFTER the round's tool results, which must stay contiguous
                // (strict providers require every tool_call answered directly
                // after the assistant message). Each attachment is a
                // user-role parts message — the one multimodal shape every
                // OpenAI-compatible vision endpoint accepts; `tool`-role
                // messages reject image parts. The context replays the same
                // shape into later turns (`crate::context`). See
                // `docs/tools.md`.
                let mut attachments: Vec<ChatMessage> = Vec::new();
                let mut cancelled_mid_tools = false;
                for call in &rest {
                    if cancel.is_cancelled() {
                        cancelled_mid_tools = true;
                        break;
                    }
                    // A task tool call (docs/task-tools.md): permission-free,
                    // instant, and cell-less — resolved through the single
                    // TaskCall event instead of the announce/Start/End trio,
                    // the post-call snapshot riding the outcome so the live
                    // checklist updates exactly when the op ran, between the
                    // round's visible calls, in the model's own order. The
                    // result still feeds back like any tool's.
                    if crate::tasks::is_task_tool(&call.name) {
                        let mut sink = |_: ToolProgress<'_>| {};
                        let outcome = execute(call, &mut sink);
                        let _ = tx.send(StreamEvent::TaskCall {
                            name: display_name(&call.name),
                            args: summarize_call(&call.name, &call.arguments),
                            arguments: call.arguments.clone(),
                            output: outcome.output.clone(),
                            ok: outcome.ok,
                            tasks: outcome.tasks.clone().unwrap_or_default(),
                        });
                        results.push((call.id.clone(), outcome.context_text().to_string()));
                        continue;
                    }
                    // `PreToolUse` (docs/hooks.md), ahead of the gate for the
                    // same reason the gate is ahead of the ToolStart: nothing
                    // has run and nothing shows as running while the user's
                    // own command decides. A hook may refuse the call, rewrite
                    // its arguments, or answer the gate's question itself.
                    let hook = hooks.pre_tool_use(call, cancel);
                    if let Some(reason) = &hook.blocked {
                        let (display, result) = hook_block_texts(reason);
                        let _ = tx.send(tool_start_event(call, session_command));
                        // The permission gate's own rejection event, reused
                        // whole: red cell, model-facing text on
                        // `context_output`, and a `/resume` that replays both.
                        let _ = tx.send(StreamEvent::ToolRejected {
                            display,
                            result: result.clone(),
                            // A refusal: nothing ran, so no capped output.
                            truncated: false,
                        });
                        results.push((call.id.clone(), result));
                        continue;
                    }
                    // A hook's `updatedInput` replaces the arguments for
                    // everything downstream — the gate's prompt, the executor,
                    // and the recorded cell all see what will actually run.
                    let rewritten = hook
                        .updated_input
                        .as_ref()
                        .map(|arguments| ToolCallRequest {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            arguments: arguments.clone(),
                        });
                    let call = rewritten.as_ref().unwrap_or(call);
                    // The permission gate (docs/permissions.md), asked BEFORE
                    // the ToolStart so nothing has run — and nothing has been
                    // announced as running — while the user (or auto mode's
                    // classifier) decides. A rejection still emits the
                    // Start/End pair, so the call lands in history and the
                    // transcript as a red cell, while the *model* reads the
                    // longer instruction.
                    let approval = if hook.pre_approved {
                        // `permissionDecision: "allow"` skips the gate wholesale
                        // — and with it the `PermissionRequest` hook, which asks
                        // "may this run without the user?" and has been answered.
                        // The classifier's provenance row, with a hook's name on it.
                        Approval::AllowNoted {
                            note: hook.note.clone().map_or_else(
                                || "Allowed by hook".to_string(),
                                // Both facts are true and the user wants both:
                                // *why it ran without them*, and what else the
                                // hook did.
                                |note| format!("Allowed by hook · {note}"),
                            ),
                        }
                    } else {
                        approve(call, hook.force_ask)
                    };
                    if let Approval::Reject { display, result } = approval {
                        let _ = tx.send(tool_start_event(call, session_command));
                        // ToolRejected, not ToolEnd: it carries BOTH texts, so
                        // the recorded call keeps the model-facing `result`
                        // beside the short cell line and later turns replay
                        // what this round actually sent (docs/permissions.md).
                        let _ = tx.send(StreamEvent::ToolRejected {
                            display,
                            result: result.clone(),
                            // A refusal: nothing ran, so no capped output.
                            truncated: false,
                        });
                        results.push((call.id.clone(), result));
                        continue;
                    }
                    let _ = tx.send(tool_start_event(call, session_command));
                    // A noted approval (the auto mode classifier's allow)
                    // rides its own event so the resolved cell can append the
                    // provenance row (docs/permissions.md).
                    if let Approval::AllowNoted { note } = approval {
                        let _ = tx.send(StreamEvent::ToolNote(note));
                        // A pre-approving hook folded its own note into the
                        // AllowNoted text above; any *other* noted approval
                        // (the classifier, a PermissionRequest hook) must not
                        // swallow the PreToolUse hook's provenance row.
                        if !hook.pre_approved
                            && let Some(note) = &hook.note
                        {
                            let _ = tx.send(StreamEvent::ToolNote(note.clone()));
                        }
                    } else if let Some(note) = &hook.note {
                        // A hook that added context, warned, or spoke to the
                        // user without deciding the call still leaves its dim
                        // `⎿` row — the same provenance channel, so the
                        // record and the `/resume` come for free.
                        let _ = tx.send(StreamEvent::ToolNote(note.clone()));
                    }
                    // Forward the tool's live output to the UI as it is produced,
                    // so the running cell tails it (docs/tool-streaming.md). The
                    // sink targets the front running call app-side; a tool that
                    // does not stream (read/write/edit) simply never calls it.
                    let mut on_output = |progress: ToolProgress<'_>| {
                        let event = match progress {
                            // Settled text appends, live rows replace the
                            // last ones (docs/interactive-shell.md).
                            ToolProgress::Screen { settled, live } => StreamEvent::ToolScreen {
                                settled: settled.to_string(),
                                live: live.to_string(),
                            },
                            ToolProgress::Title(title) => StreamEvent::ToolTitle(title.to_string()),
                        };
                        let _ = tx.send(event);
                    };
                    let mut outcome = execute(call, &mut on_output);
                    // `PostToolUse` (docs/hooks.md): the call ran, so there is
                    // nothing left to refuse — only things to say. Whatever a
                    // hook adds goes onto the **model-facing** text via the
                    // outcome's existing two-text split, so the cell keeps
                    // showing the tool's own output while `context_output`
                    // carries what the model actually read (and a `/resume`
                    // replays it). It fires **only for a call that succeeded**
                    // — both references do (Claude Code routes failures to its
                    // separate `PostToolUseFailure` event, codex builds no
                    // payload at all), so a format-after-write hook written
                    // for either never runs here after a failed write. A
                    // backgrounded call has produced no output yet, so it too
                    // is left alone.
                    let post = if outcome.background.is_none() && outcome.ok {
                        hooks.post_tool_use(call, &outcome, cancel)
                    } else {
                        super::hooks::PostToolVerdict::default()
                    };
                    if let Some(note) = post.note {
                        let _ = tx.send(StreamEvent::ToolNote(note));
                    }
                    // The PreToolUse hook's context was promised before the
                    // call ran, so it folds whatever happened — a failure and
                    // a background launch included.
                    let added: Vec<String> = hook.context.into_iter().chain(post.context).collect();
                    if !added.is_empty() {
                        outcome.context = Some(format!(
                            "{}\n\n{}",
                            outcome.context_text(),
                            added.join("\n\n")
                        ));
                    }
                    // A backgrounded call resolves via its own event — the
                    // cell shows the fixed backgrounded row while the launch
                    // text still becomes the tool result the model reads
                    // (docs/background.md). An outcome whose model-facing
                    // `context` differs from the displayed text (the ask
                    // tool's resolutions, `docs/ask.md`) rides the two-text
                    // events instead — `ToolAnswered` green, `ToolRejected`
                    // red — so the recorded call keeps both.
                    match (&outcome.background, &outcome.context) {
                        (Some(id), _) => {
                            let _ = tx.send(StreamEvent::ToolBackgrounded {
                                id: id.clone(),
                                output: outcome.output.clone(),
                            });
                        }
                        (None, Some(context)) if outcome.ok => {
                            let _ = tx.send(StreamEvent::ToolAnswered {
                                display: outcome.output.clone(),
                                result: context.clone(),
                                // The call ran: a hook-amended `bash` whose
                                // output hit the byte cap still needs its `…`
                                // marker (docs/hooks.md).
                                truncated: outcome.truncated,
                            });
                        }
                        (None, Some(context)) => {
                            let _ = tx.send(StreamEvent::ToolRejected {
                                display: outcome.output.clone(),
                                result: context.clone(),
                                truncated: outcome.truncated,
                            });
                        }
                        (None, None) => {
                            let _ = tx.send(StreamEvent::ToolEnd {
                                output: outcome.output.clone(),
                                ok: outcome.ok,
                                truncated: outcome.truncated,
                            });
                        }
                    }
                    // The model reads the context text when the outcome split
                    // the two (it equals the display for every ordinary call).
                    results.push((call.id.clone(), outcome.context_text().to_string()));
                    if let Some(url) = outcome.image {
                        let path = summarize_call(&call.name, &call.arguments);
                        attachments.push(ChatMessage::with_parts(
                            "user",
                            vec![
                                ContentPart::text(image_attachment_note(&path)),
                                ContentPart::image(url),
                            ],
                        ));
                    }
                }
                // The calls the ceiling refused: each shows as its own red
                // cell (a batch sibling that never got to run) and is answered
                // with the same text the model reads, so the stored message
                // list stays well-formed for a `/resume` or a continuation.
                for call in &refused_rest {
                    let _ = tx.send(tool_start_event(call, session_command));
                    // A plain ToolEnd: the cell text and the tool result are
                    // the same one line, so there is no second text for
                    // `ToolRejected` to carry (docs/settings.md).
                    let _ = tx.send(StreamEvent::ToolEnd {
                        output: TOOL_LIMIT_OUTPUT.to_string(),
                        ok: false,
                        truncated: false,
                    });
                }
                // A refused `agent` call never launched, so it has no cell —
                // only the answer the model needs.
                for call in refused {
                    results.push((call.id.clone(), TOOL_LIMIT_OUTPUT.to_string()));
                }
                // Results append in the model's original call order. An
                // unexecuted call — a cancel landed first — is answered so the
                // stored list stays well-formed for a continuation, and in the
                // words its cell shows (`Interrupted by user`), so a stopped
                // subagent's continuation reads what its session view shows
                // (docs/interrupt.md).
                for call in &calls {
                    let output = results.iter().find(|(id, _)| id == &call.id).map_or_else(
                        || crate::app::INTERRUPT_TOOL_OUTPUT.to_string(),
                        |(_, out)| out.clone(),
                    );
                    messages.push(ChatMessage::tool_result(&call.id, &output));
                }
                messages.append(&mut attachments);
                // A cancel that landed during a tool run reaps us here rather
                // than spending another round that would just return Cancelled.
                if cancelled_mid_tools || cancel.is_cancelled() {
                    return;
                }
                // The ceiling clamped this round: what fitted has run and
                // every call is answered, so end the turn here rather than
                // asking for a round whose budget is already spent.
                if !refused.is_empty() {
                    let _ = tx.send(StreamEvent::Error(limit_error(max_tool_calls)));
                    return;
                }
            }
        }
    }
}

/// The `ToolStart` announcing `call`: the display name, the one-line header
/// summary, the model's own `description` when it gave one — and the
/// **verbatim arguments**, which is what lets the recorded call replay
/// losslessly next turn instead of being rebuilt from the summary
/// (`docs/context.md`).
fn tool_start_event(
    call: &ToolCallRequest,
    session_command: &dyn Fn(&str) -> Option<String>,
) -> StreamEvent {
    StreamEvent::ToolStart {
        name: display_name(&call.name),
        args: summarize_call_naming(&call.name, &call.arguments, session_command),
        detail: super::tools::call_description(&call.name, &call.arguments),
        arguments: Some(call.arguments.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::hooks::{HookSink, NoHooks, PostToolVerdict, PreToolVerdict};
    use super::*;
    use crate::llm::ToolCallSpec;
    use std::cell::RefCell;
    use tokio::sync::mpsc::unbounded_channel;

    /// The session lookup of a loop that knows no sessions: every
    /// `bash_session` header keeps its id.
    fn no_sessions(_: &str) -> Option<String> {
        None
    }

    fn call(id: &str, name: &str, args: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: id.to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        }
    }

    /// An assistant message echoing the given tool calls (as the real round builds).
    fn assistant_with(calls: &[ToolCallRequest]) -> ChatMessage {
        ChatMessage::assistant_tool_calls(
            "",
            calls
                .iter()
                .map(|c| ToolCallSpec::function(&c.id, &c.name, &c.arguments))
                .collect(),
        )
    }

    fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<StreamEvent>) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    #[test]
    fn a_plain_answer_finishes_with_stream_done_and_no_tools() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("hi")],
            |_msgs| RoundOutcome::Complete {
                text: String::new(),
            },
            |_call, _sink| panic!("no tools should run"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        assert_eq!(drain(&mut rx), vec![StreamEvent::StreamDone]);
    }

    /// A `HookSink` that answers from fixtures — the pure half of the hook
    /// wiring, with no process anywhere (`docs/hooks.md`).
    #[derive(Debug, Default)]
    struct FakeHooks {
        pre: PreToolVerdict,
        post: PostToolVerdict,
        /// Every call the sink was asked about, in order, with the arguments
        /// it was shown — so a test can prove `updatedInput` reached the gate
        /// and the executor and not just the cell.
        ///
        /// A `Mutex`, not a `RefCell`: [`HookSink`] is `Sync` because real
        /// sinks are shared across a subagent's threads, and this crate
        /// forbids the `unsafe impl` that would paper over it.
        seen: std::sync::Mutex<Vec<(String, String)>>,
        /// Every `PostToolUse` dispatch, with the outcome's `ok` — so a test
        /// can prove the event fires only for calls that succeeded.
        posts: std::sync::Mutex<Vec<(String, bool)>>,
        /// Scripted `stop` answers, popped front-first; empty = never block.
        stops: std::sync::Mutex<Vec<Option<String>>>,
        /// Every `stop` dispatch: the loop-guard flag and the last message it
        /// was shown.
        stop_seen: std::sync::Mutex<Vec<(bool, String)>>,
    }

    impl FakeHooks {
        fn seen(&self) -> Vec<(String, String)> {
            self.seen.lock().expect("not poisoned").clone()
        }

        fn posts(&self) -> Vec<(String, bool)> {
            self.posts.lock().expect("not poisoned").clone()
        }

        fn stop_seen(&self) -> Vec<(bool, String)> {
            self.stop_seen.lock().expect("not poisoned").clone()
        }
    }

    impl HookSink for FakeHooks {
        fn pre_tool_use(&self, call: &ToolCallRequest, _cancel: &CancelToken) -> PreToolVerdict {
            self.seen
                .lock()
                .expect("not poisoned")
                .push((call.name.clone(), call.arguments.clone()));
            self.pre.clone()
        }

        fn post_tool_use(
            &self,
            call: &ToolCallRequest,
            outcome: &ToolOutcome,
            _cancel: &CancelToken,
        ) -> PostToolVerdict {
            self.posts
                .lock()
                .expect("not poisoned")
                .push((call.name.clone(), outcome.ok));
            self.post.clone()
        }

        fn stop(
            &self,
            stop_hook_active: bool,
            last_message: &str,
            _cancel: &CancelToken,
        ) -> Option<String> {
            self.stop_seen
                .lock()
                .expect("not poisoned")
                .push((stop_hook_active, last_message.to_string()));
            let mut stops = self.stops.lock().expect("not poisoned");
            if stops.is_empty() {
                None
            } else {
                stops.remove(0)
            }
        }
    }

    /// Drive one `bash` round through `hooks` and hand back the events plus
    /// the tool result the model was given.
    fn round_with_hooks(hooks: &dyn HookSink) -> (Vec<StreamEvent>, String) {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "bash", r#"{"command":"ls"}"#)];
        let mut messages = vec![ChatMessage::user("run ls")];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut messages,
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |c, _sink| ToolOutcome::ok(format!("ran {}", c.arguments)),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            hooks,
            &no_sessions,
        );
        let result = messages
            .iter()
            .rev()
            .find_map(|m| match (&m.role[..], &m.content) {
                ("tool", crate::llm::MessageContent::Text(text)) => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        (drain(&mut rx), result)
    }

    /// [`round_with_hooks`] with the executor's outcome and the gate's answer
    /// chosen by the test — one `bash` round, the events plus the tool result
    /// the model was given.
    fn round_with(
        hooks: &dyn HookSink,
        outcome: ToolOutcome,
        approval: Approval,
    ) -> (Vec<StreamEvent>, String) {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "bash", r#"{"command":"ls"}"#)];
        let mut messages = vec![ChatMessage::user("run ls")];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut messages,
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, _sink| outcome.clone(),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| approval.clone(),
            hooks,
            &no_sessions,
        );
        let result = messages
            .iter()
            .rev()
            .find_map(|m| match (&m.role[..], &m.content) {
                ("tool", crate::llm::MessageContent::Text(text)) => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        (drain(&mut rx), result)
    }

    #[test]
    fn a_pre_tool_use_block_refuses_an_agent_launch_too() {
        // Claude Code fires PreToolUse for its Task tool, so a config gating
        // subagent launches (`"matcher": "Task"` — aliased to our `agent`)
        // must gate them here: the launcher is never called, the refusal is
        // an ordinary red cell, and the model reads why.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call(
            "a1",
            "agent",
            r#"{"description":"probe","prompt":"go"}"#,
        )];
        let hooks = FakeHooks {
            pre: PreToolVerdict {
                blocked: Some("no subagents today".to_string()),
                ..PreToolVerdict::default()
            },
            ..FakeHooks::default()
        };
        let mut messages = vec![ChatMessage::user("launch it")];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut messages,
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, _sink| panic!("no ordinary tools requested"),
            Vec::new,
            |_calls| panic!("a blocked launch must never reach the launcher"),
            |_call, _force| Approval::Allow,
            &hooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolRejected { display, .. }
                    if display.contains("no subagents today"))),
            "{events:?}"
        );
        assert_eq!(hooks.seen().len(), 1, "the hook saw the agent call");
        let result = messages
            .iter()
            .rev()
            .find_map(|m| match (&m.role[..], &m.content) {
                ("tool", crate::llm::MessageContent::Text(text)) => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(result.contains("no subagents today"), "{result}");
    }

    #[test]
    fn a_stop_hook_block_makes_the_same_turn_run_another_round() {
        // Claude Code's shape exactly: the stop hooks fire inside the query
        // loop, and a block IS the next iteration — the reply so far becomes
        // an assistant message, the feedback the next user message, the flag
        // goes true, and StreamDone waits for a firing that lets go.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let hooks = FakeHooks {
            stops: std::sync::Mutex::new(vec![Some("tests are red".to_string()), None]),
            ..FakeHooks::default()
        };
        let mut messages = vec![ChatMessage::user("fix it")];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut messages,
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                RoundOutcome::Complete {
                    text: format!("answer {n}"),
                }
            },
            |_c, _sink| panic!("no tools in this turn"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &hooks,
            &no_sessions,
        );
        assert_eq!(
            *rounds.borrow(),
            2,
            "the block bought exactly one more round"
        );
        assert_eq!(
            hooks.stop_seen(),
            vec![
                (false, "answer 1".to_string()),
                (true, "answer 2".to_string()),
            ],
            "the loop-guard flag goes true on the continuation's firing"
        );
        let events = drain(&mut rx);
        let notes: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::HookNote { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(notes, vec!["Stop hook feedback:\ntests are red"]);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::StreamDone))
                .count(),
            1,
            "one turn, one StreamDone: {events:?}"
        );
        // The continuation's context carries the first answer and the
        // feedback, in order.
        let tail: Vec<(String, String)> = messages
            .iter()
            .map(|m| {
                (
                    m.role.clone(),
                    match &m.content {
                        crate::llm::MessageContent::Text(t) => t.clone(),
                        _ => String::new(),
                    },
                )
            })
            .collect();
        assert_eq!(
            tail,
            vec![
                ("user".to_string(), "fix it".to_string()),
                ("assistant".to_string(), "answer 1".to_string()),
                (
                    "user".to_string(),
                    "Stop hook feedback:\ntests are red".to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_cancelled_or_failed_round_never_fires_stop() {
        // Claude Code returns before its stop hooks on an abort, and fires
        // StopFailure (unmodelled here) on an API error — either way, Stop
        // never sees a turn that didn't finish answering.
        for outcome in [
            RoundOutcome::Cancelled,
            RoundOutcome::Failed(crate::llm::LlmError::Http("boom".to_string())),
        ] {
            let (tx, _rx) = unbounded_channel();
            let cancel = CancelToken::new();
            let hooks = FakeHooks {
                stops: std::sync::Mutex::new(vec![Some("never".to_string())]),
                ..FakeHooks::default()
            };
            let outcome = RefCell::new(Some(outcome));
            run_agent(
                &tx,
                &cancel,
                MAX_TOOL_ITERATIONS,
                &mut vec![ChatMessage::user("hi")],
                |_msgs| outcome.borrow_mut().take().expect("one round"),
                |_c, _sink| panic!("no tools"),
                Vec::new,
                |_calls| Vec::new(),
                |_call, _force| Approval::Allow,
                &hooks,
                &no_sessions,
            );
            assert_eq!(hooks.stop_seen(), Vec::<(bool, String)>::new());
        }
    }

    #[test]
    fn a_post_tool_use_hook_never_fires_for_a_failed_call() {
        // Both references fire PostToolUse only when the tool succeeded
        // (Claude Code routes failures to its separate PostToolUseFailure
        // event; codex builds no payload at all) — so a format-after-write
        // hook written for either must not run here after a failed write.
        let hooks = FakeHooks {
            post: PostToolVerdict {
                context: Some("should never reach the model".to_string()),
                note: None,
            },
            ..FakeHooks::default()
        };
        let (events, result) = round_with(&hooks, ToolOutcome::error("boom"), Approval::Allow);
        assert_eq!(hooks.posts(), Vec::<(String, bool)>::new());
        assert!(
            !result.contains("should never reach the model"),
            "no post context on a failed call: {result}"
        );
        // The failed call resolves as a plain ToolEnd, exactly as before.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolEnd { ok: false, .. })),
            "{events:?}"
        );
    }

    #[test]
    fn a_post_tool_use_hook_fires_for_a_successful_call() {
        let hooks = FakeHooks::default();
        let _ = round_with(&hooks, ToolOutcome::ok("fine"), Approval::Allow);
        assert_eq!(hooks.posts(), vec![("bash".to_string(), true)]);
    }

    #[test]
    fn pre_hook_context_still_reaches_the_model_when_the_call_fails() {
        // The context was promised before the call ran; a failing call keeps
        // it (only the PostToolUse dispatch is gated on success).
        let hooks = FakeHooks {
            pre: PreToolVerdict {
                context: Some("reviewed by policy".to_string()),
                ..PreToolVerdict::default()
            },
            ..FakeHooks::default()
        };
        let (_events, result) = round_with(&hooks, ToolOutcome::error("boom"), Approval::Allow);
        assert!(result.contains("reviewed by policy"), "{result}");
    }

    #[test]
    fn both_the_approvals_note_and_the_hooks_note_reach_the_cell() {
        // The regression: an AllowNoted from the classifier (or a
        // PermissionRequest hook) used to swallow the PreToolUse hook's own
        // provenance row — the context reached the model with no visible
        // trace.
        let hooks = FakeHooks {
            pre: PreToolVerdict {
                context: Some("reviewed".to_string()),
                note: Some("Context added by hook".to_string()),
                ..PreToolVerdict::default()
            },
            ..FakeHooks::default()
        };
        let (events, _result) = round_with(
            &hooks,
            ToolOutcome::ok("fine"),
            Approval::AllowNoted {
                note: "Allowed by auto mode classifier".to_string(),
            },
        );
        let notes: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ToolNote(n) => Some(n.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            notes,
            vec!["Allowed by auto mode classifier", "Context added by hook"],
            "{events:?}"
        );
    }

    #[test]
    fn a_backgrounded_call_still_carries_pre_hook_context_to_the_model() {
        let hooks = FakeHooks {
            pre: PreToolVerdict {
                context: Some("reviewed by policy".to_string()),
                ..PreToolVerdict::default()
            },
            ..FakeHooks::default()
        };
        let (events, result) = round_with(
            &hooks,
            ToolOutcome::backgrounded("task-1", "launched in the background"),
            Approval::Allow,
        );
        assert!(
            result.contains("launched in the background") && result.contains("reviewed by policy"),
            "the model reads the launch text and the hook's context: {result}"
        );
        // The cell still shows the fixed backgrounded row — the launch text
        // rides the event untouched.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolBackgrounded { .. })),
            "{events:?}"
        );
        // And PostToolUse stays silent: the call has not finished.
        assert_eq!(hooks.posts(), Vec::<(String, bool)>::new());
    }

    #[test]
    fn a_pre_tool_use_block_refuses_the_call_through_the_rejection_path() {
        let hooks = FakeHooks {
            pre: PreToolVerdict {
                blocked: Some("no destructive deletes".to_string()),
                ..PreToolVerdict::default()
            },
            ..FakeHooks::default()
        };
        let (events, result) = round_with_hooks(&hooks);
        // The cell still lands — Start then the two-text rejection, never a
        // ToolEnd — so history, the transcript and a /resume all have it.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolStart { .. })),
            "the refused call still shows: {events:?}"
        );
        let Some(StreamEvent::ToolRejected { display, .. }) = events
            .iter()
            .find(|e| matches!(e, StreamEvent::ToolRejected { .. }))
        else {
            panic!("a hook block resolves as a rejection: {events:?}");
        };
        assert!(display.contains("no destructive deletes"), "{display}");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolEnd { .. })),
            "a blocked call never runs: {events:?}"
        );
        assert!(
            result.contains("no destructive deletes"),
            "the model is told why: {result}"
        );
    }

    #[test]
    fn an_updated_input_is_what_actually_runs() {
        let hooks = FakeHooks {
            pre: PreToolVerdict {
                updated_input: Some(r#"{"command":"ls -la"}"#.to_string()),
                ..PreToolVerdict::default()
            },
            ..FakeHooks::default()
        };
        let (_events, result) = round_with_hooks(&hooks);
        assert!(
            result.contains("ls -la"),
            "the executor saw the rewritten arguments: {result}"
        );
    }

    #[test]
    fn a_pre_tool_use_context_reaches_the_model_but_not_the_cell() {
        let hooks = FakeHooks {
            pre: PreToolVerdict {
                context: Some("reviewed by policy".to_string()),
                note: Some("Context added by hook".to_string()),
                ..PreToolVerdict::default()
            },
            ..FakeHooks::default()
        };
        let (events, result) = round_with_hooks(&hooks);
        assert!(
            result.contains("reviewed by policy"),
            "the model reads it: {result}"
        );
        // The cell keeps the tool's own output; the note is the visible trace.
        let Some(StreamEvent::ToolAnswered { display, .. }) = events
            .iter()
            .find(|e| matches!(e, StreamEvent::ToolAnswered { .. }))
        else {
            panic!("the two-text split carries the context: {events:?}");
        };
        assert!(
            !display.contains("reviewed by policy"),
            "the cell stays the tool's own output: {display}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolNote(n) if n.contains("hook"))),
            "the user sees a provenance row: {events:?}"
        );
    }

    #[test]
    fn a_post_tool_use_note_appends_after_the_tool_ran() {
        let hooks = FakeHooks {
            post: PostToolVerdict {
                context: Some("the linter reformatted it".to_string()),
                note: Some("Context added by hook".to_string()),
            },
            ..FakeHooks::default()
        };
        let (_events, result) = round_with_hooks(&hooks);
        assert!(
            result.starts_with("ran "),
            "the tool's own output leads: {result}"
        );
        assert!(
            result.contains("the linter reformatted it"),
            "the hook's note follows: {result}"
        );
    }

    #[test]
    fn a_hook_amended_call_keeps_its_truncation_marker() {
        // Folding hook context onto an outcome swaps the plain ToolEnd for the
        // two-text ToolAnswered/ToolRejected pair. Those must still carry
        // `truncated`, or a capped `bash` output silently loses the `…` its
        // expanded cell appends (the regression this locks).
        for ok in [true, false] {
            let (tx, mut rx) = unbounded_channel();
            let cancel = CancelToken::new();
            let rounds = RefCell::new(0);
            let calls = vec![call("c1", "bash", r#"{"command":"yes"}"#)];
            // The successful call is amended by a PostToolUse hook; the failed
            // one by a PreToolUse context (PostToolUse never fires for a
            // failure) — both routes reroute the resolution through the
            // two-text events and must keep the flag.
            let hooks = FakeHooks {
                post: PostToolVerdict {
                    context: ok.then(|| "a note".to_string()),
                    note: None,
                },
                pre: PreToolVerdict {
                    context: (!ok).then(|| "a note".to_string()),
                    ..PreToolVerdict::default()
                },
                ..FakeHooks::default()
            };
            run_agent(
                &tx,
                &cancel,
                MAX_TOOL_ITERATIONS,
                &mut vec![ChatMessage::user("run it")],
                |_msgs| {
                    let mut n = rounds.borrow_mut();
                    *n += 1;
                    if *n == 1 {
                        RoundOutcome::ToolCalls {
                            assistant: assistant_with(&calls),
                            calls: calls.clone(),
                        }
                    } else {
                        RoundOutcome::Complete {
                            text: String::new(),
                        }
                    }
                },
                |_c, _sink| ToolOutcome {
                    ok,
                    ..ToolOutcome::ok("a very long output".to_string()).with_truncated(true)
                },
                Vec::new,
                |_calls| Vec::new(),
                |_call, _force| Approval::Allow,
                &hooks,
                &no_sessions,
            );
            let events = drain(&mut rx);
            let truncated = events.iter().find_map(|e| match e {
                StreamEvent::ToolAnswered { truncated, .. }
                | StreamEvent::ToolRejected { truncated, .. } => Some(*truncated),
                _ => None,
            });
            assert_eq!(
                truncated,
                Some(true),
                "ok={ok}: the amended resolution lost the truncation flag: {events:?}"
            );
        }
    }

    #[test]
    fn a_silent_sink_changes_nothing_at_all() {
        let (with_hooks, result_hooked) = round_with_hooks(&FakeHooks::default());
        let (without, result_plain) = round_with_hooks(&NoHooks);
        assert_eq!(with_hooks, without);
        assert_eq!(result_hooked, result_plain);
        // And the plain path is still a ToolEnd, not the two-text split.
        assert!(
            without
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolEnd { .. })),
            "{without:?}"
        );
    }

    #[test]
    fn the_hook_sees_the_call_before_anything_has_run() {
        let hooks = FakeHooks::default();
        let _ = round_with_hooks(&hooks);
        assert_eq!(
            hooks.seen(),
            vec![("bash".to_string(), r#"{"command":"ls"}"#.to_string())]
        );
    }

    #[test]
    fn a_tool_round_emits_start_end_then_a_final_done() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "bash", r#"{"command":"ls"}"#)];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("run ls")],
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |c, _sink| ToolOutcome::ok(format!("ran {}", c.name)),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        assert_eq!(
            events,
            vec![
                // The round's wire identity leads (docs/prompt-caching.md),
                // the model's own arguments with it — what a call that never
                // reaches its ToolStart is recorded with (docs/interrupt.md).
                StreamEvent::RoundCalls(vec![RoundCall {
                    id: "c1".to_string(),
                    name: "bash".to_string(),
                    arguments: r#"{"command":"ls"}"#.to_string(),
                }]),
                // The batch is announced up front (here a batch of one) so the
                // UI can show every requested call, the not-yet-run ones as
                // `⎿ Waiting…`, before they execute in order. See
                // `docs/parallel-tools.md`.
                StreamEvent::ToolBatch(vec![ToolCallSummary {
                    name: "Bash".to_string(),
                    args: "ls".to_string(),
                }]),
                StreamEvent::ToolStart {
                    name: "Bash".to_string(),
                    args: "ls".to_string(),
                    detail: None,
                    arguments: Some(r#"{"command":"ls"}"#.to_string()),
                },
                StreamEvent::ToolEnd {
                    output: "ran bash".to_string(),
                    ok: true,
                    truncated: false,
                },
                StreamEvent::StreamDone,
            ]
        );
        assert_eq!(
            *rounds.borrow(),
            2,
            "a second round produced the final answer"
        );
    }

    #[test]
    fn a_split_outcome_resolves_answered_or_rejected_and_the_model_reads_the_context() {
        // The ask tool's resolutions (docs/ask.md): the executor hands back a
        // display text + a different model-facing context. An ok outcome rides
        // ToolAnswered (green), a failed one ToolRejected (red) — never a
        // plain ToolEnd — and the stored tool result is the CONTEXT text.
        for (ok, expected) in [
            (
                true,
                StreamEvent::ToolAnswered {
                    display: "User answered Alter Zero's questions:\n· Q → A".to_string(),
                    result: r#"{"answers":{"Q":"A"}}"#.to_string(),
                    truncated: false,
                },
            ),
            (
                false,
                StreamEvent::ToolRejected {
                    display: "User answered Alter Zero's questions:\n· Q → A".to_string(),
                    result: r#"{"answers":{"Q":"A"}}"#.to_string(),
                    truncated: false,
                },
            ),
        ] {
            let (tx, mut rx) = unbounded_channel();
            let cancel = CancelToken::new();
            let rounds = RefCell::new(0);
            let calls = vec![call("c1", "askuserquestion", r#"{"questions":[]}"#)];
            let mut messages = vec![ChatMessage::user("ask me")];
            run_agent(
                &tx,
                &cancel,
                MAX_TOOL_ITERATIONS,
                &mut messages,
                |_msgs| {
                    let mut n = rounds.borrow_mut();
                    *n += 1;
                    if *n == 1 {
                        RoundOutcome::ToolCalls {
                            assistant: assistant_with(&calls),
                            calls: calls.clone(),
                        }
                    } else {
                        RoundOutcome::Complete {
                            text: String::new(),
                        }
                    }
                },
                |_c, _sink| {
                    let outcome = ToolOutcome {
                        output: "User answered Alter Zero's questions:\n· Q → A".to_string(),
                        ok,
                        truncated: false,
                        background: None,
                        image: None,
                        tasks: None,
                        context: None,
                    };
                    outcome.with_context(r#"{"answers":{"Q":"A"}}"#)
                },
                Vec::new,
                |_calls| Vec::new(),
                |_call, _force| Approval::Allow,
                &NoHooks,
                &no_sessions,
            );
            let events = drain(&mut rx);
            assert!(
                events.contains(&expected),
                "ok={ok}: expected {expected:?} in {events:?}"
            );
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, StreamEvent::ToolEnd { .. })),
                "a split outcome never resolves through ToolEnd: {events:?}"
            );
            // The stored tool result is the model-facing context text, not the
            // cell display — later rounds and the recorded conversation carry
            // what the model actually read.
            let result = messages
                .iter()
                .find(|m| m.tool_call_id.as_deref() == Some("c1"))
                .expect("the call was answered");
            assert_eq!(
                result.content,
                crate::llm::MessageContent::Text(r#"{"answers":{"Q":"A"}}"#.to_string())
            );
        }
    }

    #[test]
    fn a_tool_that_streams_output_emits_toolscreen_between_start_and_end() {
        // The executor's `on_output` sink surfaces as ToolScreen events strictly
        // between the call's ToolStart and ToolEnd, so the running cell tails the
        // output as it is produced. See `docs/tool-streaming.md`.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "bash", r#"{"command":"printf 'a\nb\n'"}"#)];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("run it")],
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, sink| {
                sink(ToolProgress::Screen {
                    settled: "a\n",
                    live: "b",
                });
                sink(ToolProgress::Screen {
                    settled: "b\n",
                    live: "",
                });
                ToolOutcome::ok("Exit code: 0\na\nb")
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        let start = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
            .expect("the tool started");
        let end = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolEnd { .. }))
            .expect("the tool ended");
        let screens: Vec<(&str, &str)> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ToolScreen { settled, live } => {
                    Some((settled.as_str(), live.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            screens,
            vec![("a\n", "b"), ("b\n", "")],
            "the sink's updates stream as ToolScreen: {events:?}"
        );
        let all_between = events
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, StreamEvent::ToolScreen { .. }))
            .all(|(i, _)| start < i && i < end);
        assert!(all_between, "live output streams between start and end");
    }

    #[test]
    fn a_terminal_backed_tool_streams_screen_and_title_events() {
        // A terminal session's live output and its refined header reach the
        // UI as ToolScreen and ToolTitle, between the call's start and end
        // (docs/interactive-shell.md).
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "bash_session", r#"{"session_id":"b1"}"#)];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("wait on it")],
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, sink| {
                sink(ToolProgress::Title("sudo pacman -Syy"));
                sink(ToolProgress::Screen {
                    settled: ":: Synchronizing\n",
                    live: " extra  45%",
                });
                ToolOutcome::ok("Exit code: 0\n extra 100%")
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        let start = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
            .expect("the tool started");
        let end = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolEnd { .. }))
            .expect("the tool ended");
        let title = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolTitle(t) if t == "sudo pacman -Syy"))
            .expect("the refined header streamed");
        let screen = events
            .iter()
            .position(|e| {
                matches!(e, StreamEvent::ToolScreen { settled, live }
                    if settled == ":: Synchronizing\n" && live == " extra  45%")
            })
            .expect("the screen streamed");
        assert!(
            start < title && title < screen && screen < end,
            "{events:?}"
        );
    }

    #[test]
    fn a_multi_call_round_announces_the_whole_batch_before_running_any() {
        // A parallel batch: the model requests three calls at once. The loop
        // announces the whole batch (all three, in order, as the display
        // `(name, args)`) *before* the first ToolStart, so the UI can show every
        // call — the not-yet-run ones as `⎿ Waiting…` — then runs them in order.
        // See `docs/parallel-tools.md`.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![
            call("c1", "bash", r#"{"command":"ping google.com"}"#),
            call("c2", "bash", r#"{"command":"ping facebook.com"}"#),
            call("c3", "bash", r#"{"command":"ping x.com"}"#),
        ];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("ping them")],
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |c, _sink| ToolOutcome::ok(format!("ran {}", c.arguments)),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        // Right after the round's identity (its own test), the batch is
        // announced, carrying all three calls in request order as their
        // header `(name, args)`.
        let summary = |cmd: &str| ToolCallSummary {
            name: "Bash".to_string(),
            args: cmd.to_string(),
        };
        assert!(
            matches!(events.first(), Some(StreamEvent::RoundCalls(round)) if round.len() == 3),
            "the round's identity leads: {events:?}"
        );
        assert_eq!(
            events.get(1),
            Some(&StreamEvent::ToolBatch(vec![
                summary("ping google.com"),
                summary("ping facebook.com"),
                summary("ping x.com"),
            ])),
            "the batch is announced next, with every call: {events:?}"
        );
        // Exactly one batch announce, then three Start/End pairs.
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::ToolBatch(_)))
                .count(),
            1,
            "one batch announce for the round"
        );
        let starts = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolStart { .. }))
            .count();
        let ends = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolEnd { .. }))
            .count();
        assert_eq!((starts, ends), (3, 3), "all three calls run: {events:?}");
        // The announce precedes every ToolStart (nothing runs before the batch
        // is shown).
        let batch_pos = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolBatch(_)))
            .unwrap();
        let first_start = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
            .unwrap();
        assert!(
            batch_pos < first_start,
            "the batch is announced before any call starts"
        );
    }

    #[test]
    fn tool_results_are_appended_so_the_next_round_sees_them() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let seen_lens = RefCell::new(Vec::new());
        let calls = vec![call("c1", "read", r#"{"path":"a"}"#)];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("read a")],
            |msgs| {
                seen_lens.borrow_mut().push(msgs.len());
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, _sink| ToolOutcome::ok("file contents"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        drain(&mut rx);
        // Round 1 saw [user]; round 2 saw [user, assistant(tool_calls), tool].
        assert_eq!(*seen_lens.borrow(), vec![1, 3]);
    }

    #[test]
    fn a_failed_round_surfaces_an_error_and_stops() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("x")],
            |_msgs| RoundOutcome::Failed(LlmError::Http("boom".to_string())),
            |_c, _sink| panic!("no tools"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], StreamEvent::Error(m) if m.contains("boom")));
    }

    #[test]
    fn a_cancelled_round_streams_nothing() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("x")],
            |_msgs| RoundOutcome::Cancelled,
            |_c, _sink| panic!("no tools"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        assert!(drain(&mut rx).is_empty(), "a cancel is a silent stop");
    }

    #[test]
    fn a_cancel_before_the_first_round_streams_nothing() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        cancel.cancel();
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("x")],
            |_msgs| panic!("round should not run once cancelled"),
            |_c, _sink| panic!("no tools"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn a_cancel_between_tools_stops_before_running_the_rest() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let calls = vec![
            call("c1", "bash", r#"{"command":"a"}"#),
            call("c2", "bash", r#"{"command":"b"}"#),
        ];
        let ran = RefCell::new(0);
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("x")],
            |_msgs| RoundOutcome::ToolCalls {
                assistant: assistant_with(&calls),
                calls: calls.clone(),
            },
            |c, _sink| {
                *ran.borrow_mut() += 1;
                // Cancel after the first tool runs.
                cancel.cancel();
                ToolOutcome::ok(format!("ran {}", c.arguments))
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        assert_eq!(
            *ran.borrow(),
            1,
            "the second tool never ran after the cancel"
        );
        // The first tool's start/end were emitted, then a silent stop.
        let events = drain(&mut rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolStart { .. }))
        );
        assert!(!events.iter().any(|e| matches!(e, StreamEvent::StreamDone)));
    }

    #[test]
    fn a_call_the_cancel_left_unexecuted_is_answered_as_interrupted() {
        // The stored list stays well-formed after a cancel — every call
        // answered — and a call that never ran is answered the way the
        // transcript shows it, `Interrupted by user`: a subagent's chat
        // continuation resumes from this list, so its model reads what its
        // session view shows (docs/interrupt.md).
        let (tx, _rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let calls = vec![
            call("c1", "bash", r#"{"command":"a"}"#),
            call("c2", "bash", r#"{"command":"b"}"#),
        ];
        let mut messages = vec![ChatMessage::user("x")];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut messages,
            |_msgs| RoundOutcome::ToolCalls {
                assistant: assistant_with(&calls),
                calls: calls.clone(),
            },
            |_c, _sink| {
                cancel.cancel();
                ToolOutcome::error(crate::app::INTERRUPT_TOOL_OUTPUT)
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let results: Vec<&ChatMessage> = messages.iter().filter(|m| m.role == "tool").collect();
        assert_eq!(
            results,
            vec![
                &ChatMessage::tool_result("c1", crate::app::INTERRUPT_TOOL_OUTPUT),
                &ChatMessage::tool_result("c2", crate::app::INTERRUPT_TOOL_OUTPUT),
            ]
        );
    }

    #[test]
    fn the_call_cap_stops_a_runaway_loop() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let calls = vec![call("c", "bash", r#"{"command":"loop"}"#)];
        // Every round asks for another tool — the cap must break it.
        run_agent(
            &tx,
            &cancel,
            3,
            &mut vec![ChatMessage::user("x")],
            |_msgs| RoundOutcome::ToolCalls {
                assistant: assistant_with(&calls),
                calls: calls.clone(),
            },
            |_c, _sink| ToolOutcome::ok("again"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        let errors: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::Error(_)))
            .collect();
        assert_eq!(errors.len(), 1);
        assert!(matches!(&errors[0], StreamEvent::Error(m) if m.contains("3 tool calls")));
        // Exactly 3 tool rounds ran.
        let ends = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolEnd { .. }))
            .count();
        assert_eq!(ends, 3);
    }

    #[test]
    fn the_limit_error_explains_itself_and_points_at_the_setting() {
        // The user meets this as a bare red notice with nothing else on the
        // row, so it has to carry the whole story: what stopped, why it
        // stopped, and how to change it (docs/settings.md).
        let msg = limit_error(5);
        assert!(
            msg.contains("5 tool calls"),
            "names the ceiling it hit: {msg}"
        );
        assert!(
            msg.contains("Max tool calls"),
            "names the setting row by its label: {msg}"
        );
        assert!(msg.contains("/settings"), "names the command: {msg}");
        assert!(
            msg.contains('0'),
            "says how to lift the limit entirely: {msg}"
        );
    }

    #[test]
    fn the_refusal_states_the_fact_and_nothing_else() {
        // One line, in the cell AND in the context. The advice belongs to the
        // turn's closing error, which reaches the model too — a `Role::Error`
        // notice derives into an `[error] …` user entry (`crate::context`) —
        // so repeating it here would say the same thing twice, once per
        // refused sibling. A clamped batch of three would spend three
        // paragraphs of screen and of context window on it (docs/settings.md).
        assert_eq!(
            TOOL_LIMIT_OUTPUT,
            "Not run: this turn had already spent its tool-call limit."
        );
        for noise in ["/settings", "Max tool calls", "user"] {
            assert!(
                !TOOL_LIMIT_OUTPUT.contains(noise),
                "the advice lives in the closing error, not here: {noise:?}"
            );
        }
        // …and the closing error is where it does live.
        assert!(limit_error(5).contains("/settings"));
    }

    #[test]
    fn the_cap_counts_tool_calls_not_rounds() {
        // The user's report: "Max tool calls 5" still ran more than five.
        // A round can request a whole PARALLEL BATCH (docs/parallel-tools.md),
        // so counting rounds lets one round spend the whole budget several
        // times over. The cap must bound the calls themselves.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let calls = vec![
            call("a", "bash", r#"{"command":"one"}"#),
            call("b", "bash", r#"{"command":"two"}"#),
            call("c", "bash", r#"{"command":"three"}"#),
        ];
        let mut messages = vec![ChatMessage::user("x")];
        run_agent(
            &tx,
            &cancel,
            5,
            &mut messages,
            |_msgs| RoundOutcome::ToolCalls {
                assistant: assistant_with(&calls),
                calls: calls.clone(),
            },
            |_c, _sink| ToolOutcome::ok("again"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        let ran = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolEnd { ok: true, .. }))
            .count();
        // Round 1 spends 3 of the 5; round 2 can afford 2 of its 3, so the
        // third is refused and the turn ends. Exactly the budget, never more.
        assert_eq!(ran, 5, "a cap of 5 runs exactly 5 tool calls");
        let refused = events
            .iter()
            .filter(|e| {
                matches!(e, StreamEvent::ToolEnd { ok: false, output, .. }
                    if output == TOOL_LIMIT_OUTPUT)
            })
            .count();
        assert_eq!(refused, 1, "the over-budget call shows why it didn't run");
        assert!(
            matches!(events.last(), Some(StreamEvent::Error(m)) if m.contains("5 tool calls")),
            "the turn ends on the limit error: {:?}",
            events.last()
        );
        // Every requested call is answered, so the stored list stays valid for
        // a `/resume` or an agent continuation (strict providers reject an
        // assistant `tool_calls` message with a missing `tool` result).
        let requested: usize = messages.iter().map(|m| m.tool_calls.len()).sum();
        let answered = messages.iter().filter(|m| m.role == "tool").count();
        assert_eq!(
            requested, answered,
            "every tool_call has a matching tool result"
        );
    }

    #[test]
    fn a_zero_cap_means_no_limit() {
        // The `/settings` **Max tool calls** knob's default (docs/settings.md):
        // 0 lifts the backstop entirely, so a long agentic task is never cut
        // off mid-way. Run more rounds than any cap we offer, then finish.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c", "bash", r#"{"command":"step"}"#)];
        run_agent(
            &tx,
            &cancel,
            0,
            &mut vec![ChatMessage::user("x")],
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n <= 25 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, _sink| ToolOutcome::ok("done"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        assert!(
            !events.iter().any(|e| matches!(e, StreamEvent::Error(_))),
            "an uncapped run never trips the backstop"
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::ToolEnd { .. }))
                .count(),
            25,
            "every round it asked for ran"
        );
        assert!(events.iter().any(|e| matches!(e, StreamEvent::StreamDone)));
    }

    #[test]
    fn a_final_answer_after_exactly_max_tool_calls_completes() {
        // The cap bounds TOOL CALLS, not the final answer (docs/tools.md):
        // once the budget is spent the model still gets one more request, and
        // a plain-text reply there finishes the turn — only a further tool
        // request trips the error.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c", "bash", r#"{"command":"step"}"#)];
        run_agent(
            &tx,
            &cancel,
            2,
            &mut vec![ChatMessage::user("x")],
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n <= 2 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, _sink| ToolOutcome::ok("done"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        assert!(
            events.iter().any(|e| matches!(e, StreamEvent::StreamDone)),
            "a Complete round after max tool rounds still finishes: {events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, StreamEvent::Error(_))),
            "no cap error for a turn that answered: {events:?}"
        );
    }

    /// The plain text of a message, for order assertions.
    fn text_of(msg: &ChatMessage) -> String {
        match &msg.content {
            crate::llm::MessageContent::Text(t) => t.clone(),
            crate::llm::MessageContent::Parts(_) => panic!("no multimodal messages here"),
        }
    }

    #[test]
    fn notices_posted_between_rounds_inject_as_user_messages() {
        // A background shell that exits mid-turn — killed by the model's own
        // bash `kill`, the manager's `x`, or a natural death — must reach the
        // model WITHIN the turn: the loop takes the pending notes at the top
        // of each round and appends each as a user-role message, after the
        // prior round's tool results (the same form `context_messages`
        // replays into later turns), so the very next request already carries
        // the outcome. See docs/background.md.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "bash", r#"{"command":"kill 408085; sleep 1"}"#)];
        let seen_round2: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let note = "[background] Background command \"Start the API server\" \
                    was terminated by a signal.";
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("kill the server")],
            |msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    *seen_round2.borrow_mut() =
                        msgs.iter().map(|m| (m.role.clone(), text_of(m))).collect();
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, _sink| ToolOutcome::ok("Exit code: 0"),
            || {
                // The exit landed while the kill command ran: the board has
                // the note by the time round 2's request is built.
                if *rounds.borrow() == 1 {
                    vec![PendingInput::Notice(note.to_string())]
                } else {
                    Vec::new()
                }
            },
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        drain(&mut rx);
        let seen = seen_round2.borrow();
        assert_eq!(
            seen.last(),
            Some(&("user".to_string(), note.to_string())),
            "the note is the round's last message — a user-role entry after \
             the tool results: {seen:?}"
        );
        assert_eq!(
            seen.iter()
                .map(|(role, _)| role.as_str())
                .collect::<Vec<_>>(),
            vec!["user", "assistant", "tool", "user"],
            "user → assistant(tool_calls) → tool result → the injected note"
        );
    }

    #[test]
    fn a_notice_pending_at_turn_start_rides_the_first_round() {
        // A completion that landed between the turn's dispatch and its first
        // request is picked up at the very first loop top — the model needs
        // no tool round to hear about it.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let pending = RefCell::new(Some("[background] note".to_string()));
        let seen = RefCell::new(Vec::new());
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("hi")],
            |msgs| {
                *seen.borrow_mut() = msgs.iter().map(|m| (m.role.clone(), text_of(m))).collect();
                RoundOutcome::Complete {
                    text: String::new(),
                }
            },
            |_c, _sink| panic!("no tools requested"),
            || {
                pending
                    .borrow_mut()
                    .take()
                    .map(PendingInput::Notice)
                    .into_iter()
                    .collect()
            },
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        drain(&mut rx);
        assert_eq!(
            *seen.borrow(),
            vec![
                ("user".to_string(), "hi".to_string()),
                ("user".to_string(), "[background] note".to_string()),
            ]
        );
    }

    #[test]
    fn a_steered_message_lands_in_the_next_rounds_context() {
        // The mid-turn queue's whole point (docs/queue.md): a message the user
        // submitted while the turn was running reaches the model at the next
        // ROUND boundary — right after the round's tool results — instead of
        // waiting for the turn to finish. It is a user-role message like any
        // other, and the loop is told it landed (`Steered`) so the inset
        // queued row can become a real user bubble.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "bash", r#"{"command":"ls"}"#)];
        let seen_round2: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("list the repo")],
            |msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    *seen_round2.borrow_mut() =
                        msgs.iter().map(|m| (m.role.clone(), text_of(m))).collect();
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, _sink| ToolOutcome::ok("Exit code: 0"),
            || {
                // Typed while the `ls` ran: waiting at the top of round 2.
                if *rounds.borrow() == 1 {
                    vec![PendingInput::User("also check the tests".to_string())]
                } else {
                    Vec::new()
                }
            },
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let seen = seen_round2.borrow();
        assert_eq!(
            seen.last(),
            Some(&("user".to_string(), "also check the tests".to_string())),
            "the steered message is the last thing round 2 sends"
        );
        assert!(
            seen.iter().any(|(role, _)| role == "tool"),
            "…and it sits after the round's tool result, not in front of it"
        );
        assert!(
            drain(&mut rx).contains(&StreamEvent::Steered {
                text: "also check the tests".to_string(),
            }),
            "the loop hears that the message was taken into the turn"
        );
    }

    #[test]
    fn a_notice_leads_a_steered_message_in_the_same_round() {
        // Both seams feed one round. A background completion is a *result* —
        // it belongs with the tool results it follows — while the user's own
        // message is the newest thing said, so it goes last.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let taken = RefCell::new(false);
        let seen = RefCell::new(Vec::new());
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("hi")],
            |msgs| {
                *seen.borrow_mut() = msgs.iter().map(|m| (m.role.clone(), text_of(m))).collect();
                RoundOutcome::Complete {
                    text: String::new(),
                }
            },
            |_c, _sink| panic!("no tools requested"),
            || {
                if std::mem::replace(&mut *taken.borrow_mut(), true) {
                    return Vec::new();
                }
                vec![
                    PendingInput::User("and deploy it".to_string()),
                    PendingInput::Notice("[background] note".to_string()),
                ]
            },
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        assert_eq!(
            *seen.borrow(),
            vec![
                ("user".to_string(), "hi".to_string()),
                ("user".to_string(), "[background] note".to_string()),
                ("user".to_string(), "and deploy it".to_string()),
            ],
            "notices first, the user's own message last — whatever order they arrived in"
        );
        let events = drain(&mut rx);
        assert!(
            events.contains(&StreamEvent::Steered {
                text: "and deploy it".to_string(),
            }),
            "only the user's message is announced"
        );
        assert!(
            !events.contains(&StreamEvent::Steered {
                text: "[background] note".to_string(),
            }),
            "a background notice is not a user bubble"
        );
    }

    #[test]
    fn a_cancelled_turn_takes_no_notices() {
        // The take sits AFTER the cancel check: an abandoned (Esc'd) turn's
        // detached thread must not steal notes owed to the boundary's
        // automatic follow-up turn.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        cancel.cancel();
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("x")],
            |_msgs| panic!("no round once cancelled"),
            |_c, _sink| panic!("no tools"),
            || panic!("no notice take once cancelled"),
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn an_image_read_outcome_attaches_a_user_image_message_after_the_results() {
        // An image `read` (docs/tools.md): the tool result stays the small
        // text while the pixels ride a follow-up USER message — the only
        // multimodal shape every OpenAI-compatible provider accepts
        // (tool-role messages reject image parts). With a sibling call in the
        // round, the results stay contiguous (strict providers require every
        // tool_call answered directly after the assistant message) and the
        // attachment follows them.
        use crate::llm::{ContentPart, MessageContent};
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![
            call("c1", "read", r#"{"path":"shot.png"}"#),
            call("c2", "bash", r#"{"command":"ls"}"#),
        ];
        let seen_round2: RefCell<Vec<ChatMessage>> = RefCell::new(Vec::new());
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("look at shot.png")],
            |msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    *seen_round2.borrow_mut() = msgs.to_vec();
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |c, _sink| {
                if c.name == "read" {
                    ToolOutcome::ok(
                        "Read image shot.png (PNG, 3x2, 90 B)\n\
                         The image is attached as the next user message.",
                    )
                    .with_image("data:image/png;base64,AAAA")
                } else {
                    ToolOutcome::ok("Exit code: 0\nfiles")
                }
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        drain(&mut rx);
        let seen = seen_round2.borrow();
        let roles: Vec<&str> = seen.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(
            roles,
            vec!["user", "assistant", "tool", "tool", "user"],
            "the results stay contiguous; the attachment follows them"
        );
        let attachment = seen.last().unwrap();
        let MessageContent::Parts(parts) = &attachment.content else {
            panic!("the attachment is a multimodal parts message: {attachment:?}");
        };
        assert_eq!(parts.len(), 2, "one text note + one image: {parts:?}");
        let ContentPart::Text { text, .. } = &parts[0] else {
            panic!("a text note leads: {parts:?}");
        };
        assert!(text.starts_with("[image] "), "got {text}");
        assert!(text.contains("shot.png"), "the note names the path: {text}");
        let ContentPart::ImageUrl { image_url } = &parts[1] else {
            panic!("the pixels follow the note: {parts:?}");
        };
        assert_eq!(&*image_url.url, "data:image/png;base64,AAAA");
        // The tool result itself stays the plain text the cell shows.
        assert_eq!(seen[2].role, "tool");
        assert!(
            matches!(&seen[2].content, MessageContent::Text(t) if t.starts_with("Read image ")),
            "the result content is the text facts: {:?}",
            seen[2]
        );
    }

    #[test]
    fn an_imageless_round_attaches_nothing() {
        // The zero-image path is byte-identical to before the feature.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "read", r#"{"path":"a.txt"}"#)];
        let seen_round2: RefCell<Vec<ChatMessage>> = RefCell::new(Vec::new());
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("read a.txt")],
            |msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    *seen_round2.borrow_mut() = msgs.to_vec();
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, _sink| ToolOutcome::ok("1 alpha"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        drain(&mut rx);
        let roles: Vec<String> = seen_round2
            .borrow()
            .iter()
            .map(|m| m.role.clone())
            .collect();
        assert_eq!(roles, vec!["user", "assistant", "tool"]);
    }

    #[test]
    fn a_backgrounded_outcome_emits_tool_backgrounded_and_still_feeds_the_result() {
        // `run_in_background` (or a Ctrl+B handoff): the executor returns a
        // background outcome — the loop resolves the cell via ToolBackgrounded
        // (never ToolEnd) while the launch text still becomes the tool-result
        // message the next round reads (docs/background.md).
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let seen_lens = RefCell::new(Vec::new());
        let calls = vec![call(
            "c1",
            "bash",
            r#"{"command":"ping x.com","run_in_background":true}"#,
        )];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("ping in background")],
            |msgs| {
                seen_lens.borrow_mut().push(msgs.len());
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, _sink| ToolOutcome::backgrounded("bash_1", "Command running with ID: bash_1"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        assert!(
            events.iter().any(|e| matches!(
                e,
                StreamEvent::ToolBackgrounded { id, output }
                    if id == "bash_1" && output.contains("bash_1")
            )),
            "the call resolves as backgrounded: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolEnd { .. })),
            "no ToolEnd for a backgrounded call: {events:?}"
        );
        assert!(events.iter().any(|e| matches!(e, StreamEvent::StreamDone)));
        // Round 2 saw [user, assistant(tool_calls), tool result] — the loop kept going.
        assert_eq!(*seen_lens.borrow(), vec![1, 3]);
    }

    #[test]
    fn a_rejected_call_never_runs_but_still_resolves_red_for_the_user() {
        // The permission gate (docs/permissions.md): the executor is never
        // reached, the cell still commits (Start + a red End carrying the
        // short display text), and the model reads the longer instruction.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "write", r#"{"path":"hello.py","content":"x"}"#)];
        let mut messages = vec![ChatMessage::user("write hello.py")];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut messages,
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, _sink| panic!("a rejected call must never execute"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Reject {
                display: "User rejected write to hello.py".to_string(),
                result: "The user doesn't want to proceed…".to_string(),
            },
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolStart { .. })),
            "the cell is still announced: {events:?}"
        );
        // ToolRejected, not ToolEnd: it carries BOTH the cell text and the
        // model-facing result, so the recorded call keeps what the model read
        // and later turns replay it (docs/permissions.md).
        assert!(
            events.iter().any(|e| matches!(
                e,
                StreamEvent::ToolRejected { display, result, .. }
                    if display == "User rejected write to hello.py"
                        && result == "The user doesn't want to proceed…"
            )),
            "…and resolves as a rejection carrying both texts: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolEnd { .. })),
            "a refused call never ends like an executed one: {events:?}"
        );
        // The model's tool result is the longer instruction, not the cell text.
        let result = messages
            .iter()
            .find(|m| m.role == "tool")
            .expect("the call was answered");
        assert!(
            matches!(&result.content, crate::llm::MessageContent::Text(t)
                if t.starts_with("The user doesn't want to proceed")),
            "got {result:?}"
        );
        assert!(events.iter().any(|e| matches!(e, StreamEvent::StreamDone)));
    }

    #[test]
    fn a_session_calls_header_names_its_command_before_it_is_approved() {
        // The permission prompt names the program the keys go to; the cell
        // it sits under — announced before the prompt opens, and committed
        // red when the user says no, which never reaches the executor —
        // names it too, from the same lookup (docs/bash-tools.md).
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call(
            "c1",
            crate::llm::tools::BASH_SEND_TOOL,
            r#"{"session_id":"b5xg4o2w0","input":"password123<Enter>"}"#,
        )];
        let mut messages = vec![ChatMessage::user("try password123")];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut messages,
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, _sink| panic!("a rejected call must never execute"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Reject {
                display: "User rejected input".to_string(),
                result: "The user doesn't want to proceed…".to_string(),
            },
            &NoHooks,
            &|id: &str| (id == "b5xg4o2w0").then(|| "sudo pacman -Syy".to_string()),
        );
        let events = drain(&mut rx);
        let header = "sudo pacman -Syy ← password123⏎";
        assert!(
            events.iter().any(|e| matches!(
                e,
                StreamEvent::ToolBatch(items) if items.len() == 1 && items[0].args == header
            )),
            "announced by its command: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolStart { args, .. } if args == header)),
            "…and started so, refused or not: {events:?}"
        );
    }

    #[test]
    fn a_noted_approval_emits_the_note_after_start_and_still_runs_the_call() {
        // The auto mode classifier's allow (docs/permissions.md): the call
        // runs exactly like a plain Allow, with one extra ToolNote event
        // between its ToolStart and its execution so the resolved cell can
        // append the provenance row.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call("c1", "bash", r#"{"command":"ls -la"}"#)];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("list files")],
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, _sink| ToolOutcome::ok("Exit code: 0\ntotal 40"),
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::AllowNoted {
                note: "Allowed by auto mode classifier".to_string(),
            },
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        let start = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
            .expect("the call starts");
        let note = events
            .iter()
            .position(
                |e| matches!(e, StreamEvent::ToolNote(n) if n == "Allowed by auto mode classifier"),
            )
            .expect("the note is emitted");
        let end = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolEnd { ok: true, .. }))
            .expect("the call still runs to its end");
        assert!(start < note && note < end, "start → note → end: {events:?}");
        assert!(events.iter().any(|e| matches!(e, StreamEvent::StreamDone)));
    }

    #[test]
    fn approval_is_asked_before_the_tool_starts() {
        // Ordering matters: the prompt must appear with nothing running, so
        // the gate is consulted ahead of the ToolStart event.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let log: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());
        let calls = vec![call("c1", "bash", r#"{"command":"ls"}"#)];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("ls")],
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |_c, _sink| {
                log.borrow_mut().push("execute");
                ToolOutcome::ok("files")
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| {
                log.borrow_mut().push("approve");
                Approval::Allow
            },
            &NoHooks,
            &no_sessions,
        );
        drain(&mut rx);
        assert_eq!(*log.borrow(), vec!["approve", "execute"]);
    }

    #[test]
    fn a_task_call_resolves_through_one_taskcall_event_and_no_cells() {
        // The task tools render no tool cell anywhere (docs/task-tools.md):
        // no batch announcement, no Start/End pair — one TaskCall event
        // carrying the display header, the result text, and the post-call
        // snapshot. The result still feeds back as the tool message.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![call(
            "c1",
            "taskcreate",
            r#"{"subject":"Set up project structure","description":"d"}"#,
        )];
        let mut messages = vec![ChatMessage::user("plan it")];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut messages,
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |c, _sink| {
                let mut store = crate::tasks::TaskStore::new();
                let text = store.run_tool(&c.name, &c.arguments).unwrap();
                ToolOutcome::ok(text).with_tasks(store)
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| panic!("task calls never consult the permission gate"),
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        assert!(
            !events.iter().any(|e| matches!(
                e,
                StreamEvent::ToolBatch(_)
                    | StreamEvent::ToolStart { .. }
                    | StreamEvent::ToolEnd { .. }
            )),
            "no cell events for a task call: {events:?}"
        );
        let task_call = events
            .iter()
            .find(|e| matches!(e, StreamEvent::TaskCall { .. }))
            .expect("one TaskCall event");
        let StreamEvent::TaskCall {
            arguments,
            name,
            args,
            output,
            ok,
            tasks,
        } = task_call
        else {
            unreachable!()
        };
        assert_eq!(name, "TaskCreate");
        assert_eq!(args, "Set up project structure");
        assert_eq!(
            arguments, r#"{"subject":"Set up project structure","description":"d"}"#,
            "the raw arguments ride along so the context can replay the call"
        );
        assert_eq!(
            output,
            "Task #1 created successfully: Set up project structure"
        );
        assert!(ok);
        assert_eq!(tasks.tasks().len(), 1, "the post-call snapshot rides along");
        // The model still reads the result like any tool's.
        let result = messages
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("c1"))
            .expect("the call was answered");
        assert_eq!(
            result.content,
            crate::llm::MessageContent::Text(
                "Task #1 created successfully: Set up project structure".to_string()
            )
        );
        assert!(events.iter().any(|e| matches!(e, StreamEvent::StreamDone)));
    }

    #[test]
    fn a_tool_round_announces_every_call_id_first_in_the_models_order() {
        // Before the batch announcement, the cells, or any result: the
        // round's calls of every kind — visible, task, refused ordinary —
        // under the ids the provider gave them, in the model's order
        // (docs/prompt-caching.md).
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let calls = vec![
            call("call_bash", "bash", r#"{"command":"ls"}"#),
            call(
                "call_task",
                "taskupdate",
                r#"{"taskId":"1","status":"completed"}"#,
            ),
            call("call_read", "read", r#"{"path":"a.rs"}"#),
        ];
        let rounds = RefCell::new(0);
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("go")],
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |c, _sink| {
                if crate::tasks::is_task_tool(&c.name) {
                    ToolOutcome::ok("Updated task #1 status")
                        .with_tasks(crate::tasks::TaskStore::new())
                } else {
                    ToolOutcome::ok("fine")
                }
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        let StreamEvent::RoundCalls(round) = &events[0] else {
            panic!("the round is announced first: {events:?}");
        };
        let ids: Vec<&str> = round.iter().map(|call| call.id.as_str()).collect();
        assert_eq!(ids, ["call_bash", "call_task", "call_read"]);
        let names: Vec<&str> = round.iter().map(|call| call.name.as_str()).collect();
        assert_eq!(names, ["bash", "taskupdate", "read"]);
        assert!(
            matches!(events[1], StreamEvent::ToolBatch(_)),
            "the batch announcement follows: {events:?}"
        );
    }

    #[test]
    fn a_mixed_round_announces_only_the_visible_calls() {
        // [taskupdate, bash] in one round: the batch announcement carries the
        // bash call alone (a Waiting cell must never be created for an
        // invisible call), and the TaskCall resolves before the bash starts —
        // the model's own order.
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![
            call("c1", "taskupdate", r#"{"taskId":"1","status":"completed"}"#),
            call("c2", "bash", r#"{"command":"ls"}"#),
        ];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut vec![ChatMessage::user("go")],
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |c, _sink| {
                if crate::tasks::is_task_tool(&c.name) {
                    ToolOutcome::error("Task #1 not found")
                        .with_tasks(crate::tasks::TaskStore::new())
                } else {
                    ToolOutcome::ok("Exit code: 0")
                }
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        let batch = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::ToolBatch(items) => Some(items.clone()),
                _ => None,
            })
            .expect("the visible call is announced");
        assert_eq!(batch.len(), 1, "only bash is announced: {batch:?}");
        assert_eq!(batch[0].name, "Bash");
        let task_pos = events
            .iter()
            .position(|e| matches!(e, StreamEvent::TaskCall { .. }))
            .expect("the task call resolves");
        let start_pos = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
            .expect("bash runs");
        assert!(
            task_pos < start_pos,
            "the task op lands before bash, the model's order: {events:?}"
        );
        // A failed op is still a TaskCall (red text for the model), never a
        // ToolEnd cell.
        assert!(matches!(
            &events[task_pos],
            StreamEvent::TaskCall { ok: false, .. }
        ));
    }

    #[test]
    fn a_task_call_past_the_budget_is_answered_without_any_event() {
        // Task calls spend the Max-tool-calls budget like every call; past it
        // they are answered with the limit text but — having no cell — emit
        // nothing (the refused-agent-call rule).
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let calls = vec![
            call("c1", "taskcreate", r#"{"subject":"a","description":"d"}"#),
            call("c2", "taskcreate", r#"{"subject":"b","description":"d"}"#),
        ];
        let mut messages = vec![ChatMessage::user("plan")];
        run_agent(
            &tx,
            &cancel,
            1,
            &mut messages,
            |_msgs| RoundOutcome::ToolCalls {
                assistant: assistant_with(&calls),
                calls: calls.clone(),
            },
            |c, _sink| {
                let mut store = crate::tasks::TaskStore::new();
                let text = store.run_tool(&c.name, &c.arguments).unwrap();
                ToolOutcome::ok(text).with_tasks(store)
            },
            Vec::new,
            |_calls| Vec::new(),
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::TaskCall { .. }))
                .count(),
            1,
            "only the affordable call ran: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolStart { .. })),
            "the refused task call shows no cell: {events:?}"
        );
        let refused = messages
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("c2"))
            .expect("the refused call is still answered");
        assert_eq!(
            refused.content,
            crate::llm::MessageContent::Text(TOOL_LIMIT_OUTPUT.to_string())
        );
    }

    #[test]
    fn agent_calls_route_to_the_launcher_and_results_keep_call_order() {
        let (tx, mut rx) = unbounded_channel();
        let cancel = CancelToken::new();
        let rounds = RefCell::new(0);
        let calls = vec![
            call("c1", "agent", r#"{"description":"d","prompt":"p"}"#),
            call("c2", "bash", r#"{"command":"ls"}"#),
        ];
        let mut messages = vec![ChatMessage::user("go")];
        run_agent(
            &tx,
            &cancel,
            MAX_TOOL_ITERATIONS,
            &mut messages,
            |_msgs| {
                let mut n = rounds.borrow_mut();
                *n += 1;
                if *n == 1 {
                    RoundOutcome::ToolCalls {
                        assistant: assistant_with(&calls),
                        calls: calls.clone(),
                    }
                } else {
                    RoundOutcome::Complete {
                        text: String::new(),
                    }
                }
            },
            |c, _sink| {
                assert_eq!(c.name, "bash", "agent calls never reach the executor");
                ToolOutcome::ok("listing")
            },
            Vec::new,
            |agent_calls| {
                assert_eq!(agent_calls.len(), 1);
                assert_eq!(agent_calls[0].id, "c1");
                vec![("c1".to_string(), "agent result".to_string())]
            },
            |_call, _force| Approval::Allow,
            &NoHooks,
            &no_sessions,
        );
        let events = drain(&mut rx);
        // The ordinary batch announces only the bash call.
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolBatch(items) if items.len() == 1 && items[0].name == "Bash"
        )));
        // The tool results append in the model's original call order.
        let results: Vec<(String, String)> = messages
            .iter()
            .filter(|m| m.role == "tool")
            .map(|m| {
                (
                    m.tool_call_id.clone().unwrap(),
                    match &m.content {
                        crate::llm::MessageContent::Text(t) => t.clone(),
                        crate::llm::MessageContent::Parts(_) => String::new(),
                    },
                )
            })
            .collect();
        assert_eq!(
            results,
            vec![
                ("c1".to_string(), "agent result".to_string()),
                ("c2".to_string(), "listing".to_string()),
            ]
        );
    }
}
