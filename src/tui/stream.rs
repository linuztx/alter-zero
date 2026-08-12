//! Folding one streamed reply event into `App` and the terminal's scrollback.
//!
//! [`Session::on_stream_event`] is the counterpart to `App::on_key`: where that
//! turns a key press into a decision, this turns a backend event into committed
//! rows. It returns whether the stream just **ended**, so the loop knows to run
//! the turn-end dispatch.
//!
//! Three shapes repeat through the match, and they are the invariants:
//!
//! - **Flush before you interleave.** Anything that isn't text — a tool batch, a
//!   tool start, an agent group — first finalises the run of assistant text
//!   before it (`App::flush_streaming_segment`), so it slots *after* that text in
//!   scrollback and history alike (invariant 4).
//! - **Reseat before you commit.** A resolution collapses the streaming strip, so
//!   the viewport is reseated to the height the next draw will use *before* the
//!   lines are queued — otherwise the box rises off the bottom and leaves blank
//!   rows beneath it (invariant 3).
//! - **A resolution is a settle point.** Every tool boundary calls
//!   [`Session::settle_bg_completions`], which is why a background shell that a
//!   command just killed reports right after that command's cell instead of at
//!   the turn's distant end (`docs/background.md`).
//!
//! Committing is view-gated throughout: `App` records regardless, and an overlay
//! return repaints from that history.

use ratatui::text::Line;

use alter_zero::stream::StreamEvent;
use alter_zero::ui;

use super::Session;

impl Session<'_> {
    /// Apply one streamed reply event, committing finished lines to scrollback in
    /// the conversation view. Returns whether the stream just **ended**
    /// (`StreamDone`/`Error`), so the caller can run the turn-end dispatch. The
    /// commit work mirrors how a resize repaints the same items from history.
    pub(crate) fn on_stream_event(&mut self, event: StreamEvent) -> bool {
        let width = self.term.screen().width;
        let committing = self.commits_allowed();
        match event {
            StreamEvent::AgentBatch { background, agents } => {
                // The model launched a group of subagents: finalise the text
                // before them (the ToolBatch dance), settle held completions at
                // this safe boundary, then seed the roster + the live group cell.
                // No scrollback commit — the cell is live until the group
                // resolves. See docs/agent-tool.md.
                self.flush_segment(committing, width);
                self.render.reset();
                self.settle_bg_completions();
                self.app.start_agent_group(background, &agents);
                // The delayed Ctrl+B hint clock — a foreground group can be moved
                // to the background like a running command (docs/background.md).
                if !background {
                    self.clocks.command_start = Some(std::time::Instant::now());
                }
                false
            }
            StreamEvent::AgentGroupDone { background, agents } => {
                // The group resolved: record the tree cell and commit it — a tool
                // resolution boundary like ToolEnd (the caller drained the agent
                // channel first, so the roster snapshots are final). See
                // docs/agent-tool.md.
                let group = self.app.finish_agent_group(background, &agents);
                if committing {
                    let height = self.live_region_height();
                    self.term.set_view_height(height);
                    self.term
                        .insert_before(ui::agent_group_lines(&group, width));
                    self.term.insert_before(vec![Line::default()]);
                }
                self.settle_bg_completions();
                self.clocks.command_start = None;
                false
            }
            StreamEvent::Chunk(chunk) => {
                self.app.push_chunk(&chunk);
                if committing && let Some(text) = self.app.streaming_text() {
                    let lines = self.render.commit(text, width);
                    self.term.insert_before(lines);
                }
                false
            }
            StreamEvent::ToolBatch(items) => {
                // The model requested a batch of tool calls. Finalise the
                // assistant text before them (so they slot after it in
                // scrollback), then register the whole batch as `Waiting` in the
                // live region — every call shows at once, the ones not yet
                // running as `⎿ Waiting…`. No scrollback commit: the batch is
                // live-only until each call ends. The subsequent per-call
                // ToolStart flips its front `Waiting` to `Running`. See
                // `docs/parallel-tools.md`.
                self.flush_segment(committing, width);
                self.render.reset();
                // The flush left the streaming buffer empty — a safe boundary for
                // background completions that landed while the text streamed
                // (docs/background.md), committed before the batch goes live.
                self.settle_bg_completions();
                self.app.start_tool_batch(&items);
                false
            }
            StreamEvent::ToolStart { name, args, .. } => {
                // Finalise the current run of assistant text so the tool slots
                // after it in scrollback, then show the tool running (pulsing
                // grey) in the live region until its ToolEnd arrives. The flush
                // always runs (it records history); only the commit is
                // view-gated. For a batched call the flush already ran on
                // ToolBatch (the buffer is empty, so this is a no-op) and
                // `start_tool` flips the front `Waiting` to `Running`; a lone
                // call (dummy/shell, no batch) pushes a fresh running call.
                self.flush_segment(committing, width);
                self.render.reset();
                // Same safe boundary as ToolBatch: the buffer is empty, so any
                // held completions commit ahead of the tool (docs/background.md).
                self.settle_bg_completions();
                self.app.start_tool(&name, &args);
                // Start this command's own clock — the delayed Ctrl+B hint waits
                // on it, so a fast command never flashes the hint (a model tool
                // that starts deep into a turn can't inherit the turn's elapsed).
                self.clocks.command_start = Some(std::time::Instant::now());
                false
            }
            StreamEvent::ToolEnd {
                output,
                ok,
                truncated,
            } => {
                // A `!` output that overflowed the in-memory cap was cut by the
                // runner; mark the running tool so its expanded cell appends a `…`
                // marker (before end_tool takes it). `output` is the retained head.
                if truncated {
                    self.app.set_tool_truncated();
                }
                // Commit the finished tool *collapsed* (green/red) to scrollback;
                // its full (retained) output lives in the Ctrl+O view.
                let resolved = self.app.end_tool(&output, ok).is_some();
                let held = resolved && !self.commit_tool_cell(committing, width);
                // A tool resolution is a settle point: completions that landed
                // while the call ran (often a `kill` this very command issued)
                // commit right after its cell — the user's example order — not
                // at the turn's distant end (docs/background.md). A **held**
                // MCP run is the exception: its cells commit together when the
                // run ends, so a notice must not land inside it (docs/mcp.md).
                if !held {
                    self.settle_bg_completions();
                }
                // The command resolved — stop its Ctrl+B-hint clock (the turn may
                // continue with more text/tools).
                self.clocks.command_start = None;
                false
            }
            StreamEvent::ToolRejected {
                display,
                result,
                truncated,
            } => {
                // The user refused the call at the permission prompt: commit the
                // red cell with the short `display` (Tab's instructions on its
                // second line) while `result` — the longer text the model read —
                // rides the recorded call so the derived context replays it on
                // every later turn (docs/permissions.md). Mirrors the ToolEnd
                // commit dance; nothing ran, so there is no truncation to mark.
                if truncated {
                    self.app.set_tool_truncated();
                }
                if self.app.reject_tool(&display, &result).is_some() {
                    self.commit_tool_cell(committing, width);
                }
                // A resolution boundary like ToolEnd (docs/background.md).
                self.settle_bg_completions();
                self.clocks.command_start = None;
                false
            }
            StreamEvent::ToolAnswered {
                display,
                result,
                truncated,
            } => {
                // The user answered the `AskUserQuestion` call: commit the green
                // cell with the `· Q → A` display while `result` — the answers
                // JSON the model read — rides the recorded call, the
                // ToolRejected dance in green (docs/ask.md).
                if truncated {
                    self.app.set_tool_truncated();
                }
                if self.app.answer_tool(&display, &result).is_some() {
                    self.commit_tool_cell(committing, width);
                }
                self.settle_bg_completions();
                self.clocks.command_start = None;
                false
            }
            StreamEvent::ToolBackgrounded { id: _, output } => {
                // The call resolved by moving to the background: commit its cell
                // with the fixed `⎿ Running in the background (↓ to manage)` row
                // (the stored output is the model-facing launch text). The process
                // itself now reports through the background channel. Mirrors the
                // ToolEnd commit dance (docs/background.md).
                if self.app.background_tool(&output).is_some() {
                    self.commit_tool_cell(committing, width);
                }
                // A resolution boundary like ToolEnd — completions held during
                // the launch settle here (docs/background.md).
                self.settle_bg_completions();
                // The command moved to the background — stop its hint clock.
                self.clocks.command_start = None;
                false
            }
            StreamEvent::TaskCall {
                name,
                args,
                arguments,
                output,
                ok,
                tasks,
            } => {
                // A task tool call resolved (docs/task-tools.md): finalise
                // the text before it — each round's narration becomes its own
                // `●` bullet, the ToolBatch dance — and settle held
                // completions at the safe boundary. Then record: the hidden
                // history record appends, the checklist snapshot installs,
                // and the strip re-renders on the scheduled frame. **No
                // scrollback commit** — the call has no cell; the checklist
                // under the status line is its whole display.
                self.flush_segment(committing, width);
                self.render.reset();
                self.settle_bg_completions();
                self.app
                    .record_task_call(&name, &args, &arguments, &output, ok, tasks);
                false
            }
            StreamEvent::ToolOutput(chunk) => {
                // Live tool output: append it to the running call so the live cell
                // tails it (docs/tool-streaming.md). No scrollback commit — the
                // tail is live-only until the ToolEnd commits the finished cell;
                // the next draw tick repaints the preview with the grown output.
                self.app.push_tool_output(&chunk);
                false
            }
            StreamEvent::HookNote { label, text } => {
                // A lifecycle hook injected conversation text mid-turn
                // (docs/hooks.md): finalise the assistant run before it —
                // invariant 4's flush-before-you-interleave, so the
                // continuation that may follow streams as its own message —
                // and record the cell-less item the transcript shows and the
                // derived context replays. The flush leaves the buffer empty:
                // a safe boundary for held background completions too. A
                // compact turn's notes (PreCompact's instructions) are
                // ephemeral like its chunks: they reached the summarization
                // request, and must not enter the conversation's own record.
                if !self.app.is_compacting() {
                    self.flush_segment(committing, width);
                    self.render.reset();
                    self.settle_bg_completions();
                    self.app.record_hook_note(&label, &text);
                }
                false
            }
            StreamEvent::PromptBlocked { reason } => {
                // A UserPromptSubmit hook refused the prompt before the first
                // request (docs/hooks.md): the turn is over. Tear it down and
                // roll the submission back out of history with a red notice —
                // Claude Code's "erased from context, shown to the user".
                self.prompt_blocked(&reason);
                true
            }
            StreamEvent::ToolNote(note) => {
                // The auto mode classifier allowed the running call: keep the
                // provenance note on it so the resolved cell appends the dim
                // `⎿ Allowed by auto mode classifier` row (docs/permissions.md).
                // No commit — the note rides the call into its ToolEnd.
                self.app.set_tool_note(&note);
                false
            }
            StreamEvent::Permission(request) => {
                // A tool is waiting on the user: raise the inline prompt (which
                // stashes the composer draft) — the backend thread is parked on
                // the gate until an `Action::ResolvePermission` answers it.
                // Nothing is committed: the prompt is live-only, and no call has
                // started. See docs/permissions.md.
                self.app.open_permission(request);
                false
            }
            StreamEvent::AskUser(request) => {
                // The model is asking the user questions: raise the inline
                // modal (which stashes the composer draft) — the backend
                // thread is parked on the ask gate until an
                // `Action::ResolveAsk` answers it. See docs/ask.md.
                self.app.open_ask(request);
                false
            }
            StreamEvent::ThinkingStart => {
                // Phase boundary: start the thinking clock so the status line
                // shows `Thinking for Ns`. The thought itself never commits
                // (it is live-only) — but with the display on, open the
                // reasoning buffer so the strip previews the chain-of-thought
                // as it streams (docs/thinking-stream.md).
                self.clocks.thinking_start = Some(std::time::Instant::now());
                // The `/settings` **Hide thinking** knob, read per phase — so
                // flipping it mid-session takes effect on the very next one
                // (docs/settings.md).
                if self.app.settings().show_thinking() {
                    // Flush before you interleave (the module's first rule):
                    // the live block is a cell like a tool's, so the run of
                    // assistant text before it is finalised **here**, as the
                    // block goes up. That spacer is the blank row the
                    // `● Thinking…` header sits under — without it the header
                    // butted against the paragraph that was still streaming
                    // (the reported bug) and then jolted down a row when the
                    // phase collapsed and `settle_reasoning` flushed instead.
                    // Gated on the display: a hidden phase shows nothing, so
                    // it must not split the reply. A shown phase that ends up
                    // with no text still leaves the split — but the header was
                    // on screen, so the break is honest, and the real backend
                    // only ever opens a phase holding a reasoning delta
                    // (`llm::backend`).
                    self.flush_segment(committing, width);
                    self.render.reset();
                    self.app.begin_reasoning();
                }
                false
            }
            StreamEvent::ThinkingChunk(chunk) => {
                // Reasoning delta: counted into the token tally, and — while a
                // phase is open — accumulated for the live block. Never
                // committed: only the collapsed cell at the phase's end is.
                self.app.push_thinking(&chunk);
                false
            }
            StreamEvent::ToolCallDelta(chunk) => {
                // The model is generating a tool call: opaque JSON, counted into
                // the token tally only (like reasoning) so the status ticks while
                // it generates — never rendered, never committed.
                self.app.push_tool_call_progress(&chunk);
                false
            }
            StreamEvent::ThinkingEnd => {
                // The phase is over: collapse the live block into the
                // committed `Thought for … · … tokens` cell. It lands here,
                // before the round's reply text or tool calls exist, which is
                // what puts it ahead of them in scrollback and history alike
                // (docs/thinking-stream.md).
                self.settle_reasoning();
                false
            }
            StreamEvent::StreamDone => self.on_stream_done(committing, width),
            StreamEvent::Retrying { attempt, max } => {
                // A failed request is being retried (the connection/send failed
                // before any content streamed). Show it live in the status line;
                // nothing commits to scrollback — the turn is still in flight.
                self.app.set_retry(attempt, max);
                false
            }
            StreamEvent::Usage(usage) => {
                // The round's real usage frame: snap the live tally from the
                // app-side estimate to the provider's own accounting (cache
                // detail included). Live-only — the turn summary commits the
                // total at StreamDone (docs/prompt-caching.md).
                self.app.apply_usage(&usage);
                false
            }
            StreamEvent::Error(message) => {
                // A phase still open when the backend died keeps what streamed
                // — settled first, so its cell sits ahead of the partial reply
                // and the red notice (docs/thinking-stream.md).
                self.settle_reasoning();
                if let Some(failure) = self.app.fail_stream(&message) {
                    // A live agent group died with the turn: its subagent threads
                    // keep running unless killed here (the backend thread that
                    // owned the wait loop is gone). Idempotent for agents the
                    // backend already resolved. See docs/agent-tool.md.
                    if let Some(group) = &failure.agents {
                        for entry in &group.agents {
                            let _ = self.agent_registry.kill(&entry.id);
                        }
                    }
                    if committing {
                        // Flush whatever streamed before the failure, then the
                        // tool the error killed mid-run (resolved red), then the
                        // red error notice — the Esc interrupt's exact commit
                        // shape.
                        self.commit_turn_failure(
                            failure.partial,
                            failure.tool,
                            failure.agents,
                            Some(&failure.error),
                        );
                    }
                }
                self.render.reset();
                self.clocks.end_turn();
                true
            }
        }
    }

    /// The `StreamDone` arm: a `/compact` turn's quiet end, else the final reply
    /// text, the held completions, and the `Done for Ns` summary — in that order,
    /// which is what keeps a late completion's notice *above* the summary.
    fn on_stream_done(&mut self, committing: bool, width: u16) -> bool {
        // A /compact turn's end (docs/compact.md): the marker is appended here
        // (an append — the loop-bottom recorder sync writes it, the checkpoint
        // keys stay valid) and nothing streamed to the transcript, so there is no
        // final text and no Done-for-Ns summary — the `● Context compacted` cell
        // is the record. dispatch_after_turn then snapshots + drains the queue
        // like any turn end, so a batch queued mid-compact goes out over the
        // freshly compacted context. The turn's wall-clock rides the marker so
        // the cell shows how long the compaction worked.
        if let Some(compaction) = self.app.finish_compact(
            self.clocks
                .turn_start
                .map_or(0, |start| start.elapsed().as_secs()),
        ) {
            if committing {
                let height = self.live_region_height();
                self.term.set_view_height(height);
                self.term
                    .insert_before(ui::compaction_lines(&compaction, width));
                self.term.insert_before(vec![Line::default()]); // blank spacer
            }
            self.settle_bg_completions();
            self.render.reset();
            self.clocks.end_turn();
            return true;
        }
        let final_text = self.app.finish_stream();
        let elapsed = self
            .clocks
            .turn_start
            .map_or(0, |start| start.elapsed().as_secs());
        // Clear the status and BUILD the summary, but don't record it yet: a
        // background completion still pending at turn end (a shell that finished
        // during this final text, with no tool call after it to settle at) must
        // land its notice ABOVE the "Done for Ns" summary — the same placement a
        // mid-turn tool boundary gives it — in both history and scrollback
        // (invariant 3). So the order is: reseat to idle (the status is now
        // cleared), commit the final reply, settle the held completions, THEN
        // record + commit the summary. The common case (nothing pending) is
        // unchanged — `settle_bg_completions` is then a no-op. See
        // `docs/background.md`.
        let summary = self.app.take_turn_summary(elapsed);
        if committing {
            // The reply just ended, so the streaming strip (preview + gap +
            // status, drawn *above* the box) is gone. Reseat the viewport to its
            // idle height *before* committing so the final reply line and the
            // "Done for Ns" summary replace the strip's rows in place and the box
            // stays flush at the bottom (instead of rising and leaving blank rows
            // beneath it).
            let height = self.live_region_height();
            self.term.set_view_height(height);
            if let Some(text) = final_text {
                self.term.insert_before(self.render.finish(&text, width));
                self.term.insert_before(vec![Line::default()]); // blank spacer
            }
        }
        // Settle before recording the summary — history-gated commit inside
        // (invariant 4), so the notice sits above the summary either way.
        self.settle_bg_completions();
        if let Some(summary) = summary {
            self.app.record_turn_summary(summary.clone());
            if committing {
                self.term.insert_before(ui::summary_lines(&summary, width));
                self.term.insert_before(vec![Line::default()]); // blank spacer
            }
        }
        self.render.reset();
        self.clocks.end_turn();
        true
    }

    /// Close an open thinking phase: stop the thinking clock, record the phase
    /// as a [`crate::app::Reasoning`] item, and commit its collapsed
    /// `Thought for … · … tokens` cell — the live block that was previewing
    /// it goes with it (`docs/thinking-stream.md`).
    ///
    /// The one settle helper, called from all three places a phase can end: its
    /// own `ThinkingEnd`, an Esc interrupt, and a backend error. Calling it
    /// *before* those two record anything of their own is what keeps the thought
    /// ahead of the partial reply it preceded.
    ///
    /// A no-op when no phase is open (the display is off, or the phase produced
    /// no text) — so the sites need no condition of their own.
    ///
    /// Two of the module's three shapes apply. **Flush before you interleave**:
    /// a model can reason *after* it has begun answering, so the run of
    /// assistant text before the phase is finalised first
    /// (`App::flush_streaming_segment`, the `ToolStart` dance) — otherwise the
    /// cell splices itself into the paragraph that was streaming, and `history`
    /// (where the thought is appended now) disagrees with scrollback (where the
    /// text came first), so a resize repaint would reorder them (invariant 4).
    /// And **reseat before you commit**: the strip loses the live block's rows
    /// as the cell commits, so the box must not rise off the bottom
    /// (invariant 3).
    pub(crate) fn settle_reasoning(&mut self) {
        let secs = self
            .clocks
            .thinking_start
            .map_or(0, |start| start.elapsed().as_secs());
        self.clocks.thinking_start = None;
        // Nothing was thought — no phase open (the display is off), or one that
        // streamed no text. Drop it and leave the reply alone: flushing its
        // segment would split the paragraph in two for a cell that will never
        // be recorded.
        if self
            .app
            .reasoning()
            .is_none_or(|text| text.trim().is_empty())
        {
            self.app.finish_reasoning(secs);
            return;
        }
        let width = self.term.screen().width;
        let committing = self.commits_allowed();
        // The text before the phase becomes its own history message, so the
        // thought slots after it in scrollback and history alike. Normally
        // already done — the `ThinkingStart` arm flushes as the live block
        // goes up, so the header and the settled cell share one spacer — and
        // a no-op anyway for the common case (a model that reasons before it
        // answers). It stays because this is also the settle point for an
        // Esc/error mid-phase, and for a phase whose block never showed.
        self.flush_segment(committing, width);
        self.render.reset();
        let Some(reasoning) = self.app.finish_reasoning(secs) else {
            return;
        };
        if committing {
            let height = self.live_region_height();
            self.term.set_view_height(height);
            self.term
                .insert_before(ui::reasoning_lines(&reasoning, width));
            self.term.insert_before(vec![Line::default()]);
        }
    }

    /// Finalise the run of assistant text before an interleaved cell, so it slots
    /// after that text in scrollback and history alike. The flush **always** runs
    /// (it records history); only the commit is view-gated. A no-op when the
    /// buffer is already empty — which is the common case for a batched call,
    /// whose batch event flushed first.
    fn flush_segment(&mut self, committing: bool, width: u16) {
        if let Some(segment) = self.app.flush_streaming_segment()
            && committing
        {
            self.term.insert_before(self.render.finish(&segment, width));
            self.term.insert_before(vec![Line::default()]);
        }
    }
}

impl Session<'_> {
    /// The reply channel's branch: fold one event in, keeping the subagent
    /// bookkeeping around it in step, and run the turn-end dispatch when the
    /// stream resolved.
    ///
    /// Draining the subagent channel first is what makes a group's resolution
    /// correct: the members' terminal events were enqueued there *before* the
    /// backend sent the resolution, so taking them first is what makes the roster
    /// snapshots the recorded group entries are built from final
    /// (`docs/agent-tool.md`).
    pub(crate) fn on_reply_event(&mut self, event: StreamEvent) {
        // A launched agent group: start each member's runtime clock (the boundary
        // owns the clocks — docs/agent-tool.md).
        if let StreamEvent::AgentBatch { agents, .. } = &event {
            for spec in agents {
                self.agent_clocks
                    .insert(spec.id.clone(), std::time::Instant::now());
            }
        }
        let group_done_ids: Option<Vec<String>> =
            if let StreamEvent::AgentGroupDone { agents, background } = &event {
                (!background).then(|| agents.iter().map(|a| a.id.clone()).collect())
            } else {
                None
            };
        if matches!(&event, StreamEvent::AgentGroupDone { .. }) {
            self.drain_agent_events();
        }
        let resolved = self.on_stream_event(event);
        // A foreground group's members are settled now — arm their linger sweeps
        // (a background group's keep running).
        if let Some(ids) = group_done_ids {
            self.settle_group_members(ids);
        }
        if resolved {
            // The stream ended. Settle any background completions still held
            // (most settle earlier, at a tool boundary — this catches ones that
            // landed during the final text), then send the next queued batch as
            // the following turn — Enter messages batch into one turn, while
            // Tab-opened follow-up batches each flush at their own turn-end, so
            // they iterate in order; with nothing queued, a model-launched
            // completion the agent never heard about (its note untaken on the
            // board) dispatches the automatic follow-up turn instead
            // (docs/background.md). This runs under the Ctrl+O overlay too
            // (codex's queue drains at turn end regardless of its Ctrl+T view,
            // the transcript following along): dispatching only records history
            // and *queues* the user bubbles — `term` never flushes pending lines
            // into the alternate screen, and the return's reflow drops +
            // regenerates them from history — so invariant 4 holds.
            self.dispatch_after_turn();
        }
        self.frame.schedule_frame();
    }
}

impl Session<'_> {
    /// A `UserPromptSubmit` hook blocked the prompt (`docs/hooks.md`): roll
    /// the submission back ([`alter_zero::app::App::block_prompt`] — the
    /// message out of history and back into the composer, the red
    /// reason-only notice recorded) and purge-repaint so the echoed `❯`
    /// message leaves the screen and the scrollback alike; the loop-bottom
    /// `recorder.sync` sees history shrink and rewrites the rollout without
    /// it. The caller returns `true` from the event arm, so
    /// `dispatch_after_turn` still snapshots and drains the queue.
    fn prompt_blocked(&mut self, reason: &str) {
        self.app.block_prompt(reason);
        self.render.reset();
        self.clocks.command_start = None;
        // Best-effort like every mid-event repaint: a failed write means the
        // terminal is gone, and the next draw tick repaints anyway.
        let _ = self.repaint_conversation(alter_zero::term::ReflowClear::Purge);
    }
}
