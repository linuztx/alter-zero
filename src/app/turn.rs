//! The turn lifecycle: begin/stream/interrupt/finish, the status-line tally,
//! and the committed `Done for Ns` summary.
//! See `docs/status-indicator.md` and `docs/interrupt.md`.

use super::*;

/// The whimsical working verbs, one chosen per turn (by [`App::turn_count`]) for
/// the live status line. Cycled deterministically so the demo varies yet stays
/// testable — no RNG (mirrors how [`dummy_response`] picks a reply).
pub const WORKING_VERBS: &[&str] = &[
    "Working",
    "Generating",
    "Pondering",
    "Cooking",
    "Brewing",
    "Crunching",
    "Conjuring",
    "Churning",
    "Computing",
    "Synthesizing",
];

/// The done verbs, one chosen per turn for the committed `"{verb} for Ns"` summary.
pub const DONE_VERBS: &[&str] = &["Done", "Finished", "Completed", "Wrapped up", "Ready"];

/// The live-status verb for a `!` shell command (fixed, not cycled like the AI
/// [`WORKING_VERBS`]): the status line reads `Running…`. It doubles as the
/// status's (never rendered) done verb — a shell turn ends **without** a
/// summary, [`App::end_turn`] returning `None` for it. See
/// `docs/shell-command.md`.
pub const SHELL_VERB: &str = "Running";

/// The notice committed when the user interrupts a turn (Esc mid-generation) —
/// codex's wording, minus its `/feedback` plug. Recorded as a [`Role::Error`]
/// message: like a backend failure, the notice is the turn's terminal state
/// (no `Done for Ns` summary). See `docs/interrupt.md`.
pub const INTERRUPT_NOTICE: &str =
    "Conversation interrupted - tell the model what to do differently.";

impl App {
    /// Finalise the current run of assistant text as a history message so a
    /// following tool call slots after it in order, then start a fresh empty
    /// buffer for the text that follows. Returns the finalised segment text (for
    /// the loop to flush to scrollback) or `None` if there was nothing buffered.
    /// The stream stays open.
    pub fn flush_streaming_segment(&mut self) -> Option<String> {
        let buf = self.streaming.as_mut()?;
        // A whitespace-only run (a model emitting `\n\n` before a tool call)
        // records no message: it renders to zero rows once trailing blanks are
        // trimmed (`ui::assistant_lines`), so recording it would leave a stray
        // `● ` bullet on a repaint that the live view never showed.
        if buf.trim().is_empty() {
            buf.clear(); // leaves Some("") — the stream stays open
            return None;
        }
        let text = std::mem::take(buf); // leaves Some("") — the stream stays open
        self.record_message(Role::Assistant, text.clone());
        Some(text)
    }

    /// Begin a reply: open an empty streaming buffer so the live region can
    /// show the assistant is responding even before the first chunk arrives, and
    /// start the live turn status (pick this turn's verbs, reset the tally).
    pub fn begin_stream(&mut self) {
        self.streaming = Some(String::new());
        // A new real turn re-arms the auto-compact trigger (one attempt per
        // user turn — docs/compact.md).
        self.auto_compact_blocked = false;
        // A fresh turn starts with the single-row preview; the boundary
        // re-injects the real count each frame (a forming table grows it).
        self.stream_preview_rows = 1;
        // The usage accumulators are per-turn (docs/prompt-caching.md).
        self.turn_usage_tokens = 0;
        self.turn_usage_cached = 0;
        let verb = WORKING_VERBS[self.turn_count % WORKING_VERBS.len()];
        let done_verb = DONE_VERBS[self.turn_count % DONE_VERBS.len()];
        self.turn_count = self.turn_count.wrapping_add(1);
        self.status = Some(TurnStatus {
            verb,
            done_verb,
            tokens: 0,
            arrow: TokenArrow::Down,
            elapsed: Duration::ZERO,
            thinking: None,
            shell: false,
            retry: None,
        });
    }

    /// Begin a `!` shell command as a turn, reusing the AI turn machinery so the
    /// command gets the live status strip, the spinner/timer, `esc to
    /// interrupt`, and the resize repaint for free (see `docs/shell-command.md`):
    ///
    /// - the command itself is recorded **up front** as a [`Role::Shell`]
    ///   header message (`! pwd` on the dark user-style line) — the top of the
    ///   codex-style exec cell, in history so a mid-run resize repaints it;
    /// - an **empty** streaming buffer (a shell turn has no assistant text) — so
    ///   [`is_streaming`] is true (the strip shows, a mid-run Enter queues) but
    ///   [`finish_stream`] records no phantom message;
    /// - a fixed-verb, `shell`-flagged status ([`SHELL_VERB`] → `Running…`; the
    ///   flag makes [`end_turn`] skip the summary — the cell is its own record);
    /// - the command as the running **shell** tool: headerless, so its
    ///   `⎿ Running…` peek previews flush under the header, resolving into the
    ///   `⎿ output` lines on completion.
    ///
    /// The boundary's runner then streams a `ToolEnd`/`StreamDone` pair back,
    /// committing the cell through the existing paths.
    ///
    /// [`is_streaming`]: App::is_streaming
    /// [`finish_stream`]: App::finish_stream
    /// [`end_turn`]: App::end_turn
    pub fn begin_shell(&mut self, command: &str) {
        self.record_message(Role::Shell, command);
        self.streaming = Some(String::new());
        // A shell turn never receives usage, but the per-turn accumulators
        // reset with every turn machinery start all the same.
        self.turn_usage_tokens = 0;
        self.turn_usage_cached = 0;
        self.status = Some(TurnStatus {
            verb: SHELL_VERB,
            // Never rendered: a shell turn ends without a summary (end_turn
            // returns None for it), so no dedicated done verb exists.
            done_verb: SHELL_VERB,
            tokens: 0,
            arrow: TokenArrow::Down,
            elapsed: Duration::ZERO,
            thinking: None,
            shell: true,
            retry: None,
        });
        self.start_tool(command, "");
        if let Some(tool) = self.tool_queue.front_mut() {
            tool.shell = true;
        }
    }

    /// Count a just-submitted user message into the live tally as **uploaded
    /// input** — the tokens grow and the arrow points `↑` (like a tool result
    /// folded back in, the reverse of streaming output). Called right after
    /// [`begin_stream`] with the turn's prompt, so the status shows
    /// `↑ N tokens` while the model spins up before its first chunk (the first
    /// [`push_chunk`] flips the arrow back to `↓`). The tally is **added to**,
    /// never reset (see `docs/status-indicator.md`). No-op when no turn is in
    /// flight.
    ///
    /// [`begin_stream`]: App::begin_stream
    /// [`push_chunk`]: App::push_chunk
    pub fn count_user_input(&mut self, text: &str) {
        if let Some(status) = self.status.as_mut() {
            status.tokens += count_tokens(text);
            status.arrow = TokenArrow::Up;
        }
    }

    /// Append a streamed chunk to the in-progress reply (and grow the live token
    /// tally, arrow pointing down — output streaming). No-op if not streaming.
    /// A `/compact` turn diverts the chunk into the summary buffer instead —
    /// the visible reply stays empty (the summary is never rendered) while the
    /// tally still ticks. See `docs/compact.md`.
    pub fn push_chunk(&mut self, chunk: &str) {
        if let Some(buf) = self.compact_buffer.as_mut() {
            buf.push_str(chunk);
        } else if let Some(buf) = self.streaming.as_mut() {
            buf.push_str(chunk);
        }
        if let Some(status) = self.status.as_mut() {
            status.tokens += count_tokens(chunk);
            status.arrow = TokenArrow::Down;
            // Content arrived, so the retrying request just succeeded — drop the
            // live retry indicator.
            status.retry = None;
        }
    }

    /// Count a streamed reasoning delta into the live token tally (arrow down —
    /// it is model output, streaming) **without** touching the reply buffer:
    /// the text itself is opaque and never rendered. This is what keeps the
    /// count ticking while the status line shows `Thinking for Ns`. No-op when
    /// no turn is in flight.
    pub fn push_thinking(&mut self, chunk: &str) {
        if let Some(status) = self.status.as_mut() {
            status.tokens += count_tokens(chunk);
            status.arrow = TokenArrow::Down;
            // Reasoning is content too — the retrying request recovered.
            status.retry = None;
        }
    }

    /// Count a streamed tool-call generation delta (the model emitting a tool
    /// call's `name`/`arguments` fragments) into the live token tally (arrow
    /// down — it is model output, streaming) **without** touching the reply
    /// buffer: the fragment is opaque JSON, never rendered. This keeps the
    /// status count ticking while the model *generates* a tool call, exactly
    /// like reasoning ticks it while the model thinks (see
    /// `docs/status-indicator.md`). No-op when no turn is in flight.
    pub fn push_tool_call_progress(&mut self, chunk: &str) {
        if let Some(status) = self.status.as_mut() {
            status.tokens += count_tokens(chunk);
            status.arrow = TokenArrow::Down;
            // Generating the call is content too — a retrying request recovered.
            status.retry = None;
        }
    }

    /// Fold one provider usage report (the round's final `usage` frame,
    /// [`crate::stream::StreamEvent::Usage`]) into the turn: accumulate the
    /// billed total and its cached share, and **snap the live tally to the
    /// accumulated real number** — replacing the tiktoken estimate ticked so
    /// far, which never saw the system prompt or the re-sent context. Later
    /// estimates (the next round's stream) tick on top of the snapped base,
    /// and that round's own usage frame snaps again — so the tally is always
    /// "everything billed so far, plus the current round's live estimate".
    /// No-op when no turn is in flight. See `docs/prompt-caching.md`.
    pub fn apply_usage(&mut self, usage: &crate::stream::TokenUsage) {
        if self.status.is_none() {
            return;
        }
        self.turn_usage_tokens += usize::try_from(usage.total()).unwrap_or(usize::MAX);
        self.turn_usage_cached += usize::try_from(usage.cached).unwrap_or(usize::MAX);
        if let Some(status) = self.status.as_mut() {
            status.tokens = self.turn_usage_tokens;
        }
        // The round's `input` is the whole re-sent context and its `output`
        // joins the next round's — their sum is the live context gauge
        // (docs/compact.md), authoritative until a history mutation stales it.
        self.context_used = usage.input.saturating_add(usage.output);
    }

    /// Record that a failed request is being retried, so the status line shows
    /// `retrying {attempt}/{max}` while the backend reconnects. The 1-based
    /// `attempt` and the ceiling `max` come straight from the backend's
    /// [`crate::stream::StreamEvent::Retrying`]. Cleared by the next
    /// [`push_chunk`](App::push_chunk)/[`push_thinking`](App::push_thinking) once
    /// content arrives. No-op when no turn is in flight. See `docs/llm.md`.
    pub fn set_retry(&mut self, attempt: u32, max: u32) {
        if let Some(status) = self.status.as_mut() {
            status.retry = Some(RetryInfo { attempt, max });
        }
    }

    /// The live turn status, if a turn is in flight.
    #[must_use]
    pub const fn status(&self) -> Option<&TurnStatus> {
        self.status.as_ref()
    }

    /// Is a turn in flight (its status line should show)? True from
    /// [`begin_stream`] until the turn ends — drives the loop's per-second tick.
    ///
    /// [`begin_stream`]: App::begin_stream
    #[must_use]
    pub const fn turn_active(&self) -> bool {
        self.status.is_some()
    }

    /// Write the boundary-computed times onto the live status before a draw: how
    /// long the turn has run (`elapsed` — it also drives the verb's shimmer
    /// phase), and the current thinking-phase duration (`Some` while thinking,
    /// `None` otherwise). No-op when no turn is in flight. Time is impure, so it
    /// only ever reaches the status this way.
    pub fn set_status_times(&mut self, elapsed: Duration, thinking: Option<Duration>) {
        if let Some(status) = self.status.as_mut() {
            status.elapsed = elapsed;
            status.thinking = thinking;
        }
    }

    /// Inject the current running command's elapsed each frame (the
    /// [`set_status_times`](App::set_status_times) pattern — the clock lives at
    /// the boundary). `None` when no command is running. Read by the preview to
    /// delay the `(ctrl+b to run in background)` hint until a command has run a
    /// few seconds, so a fast command never flashes it (`docs/background.md`).
    pub fn set_command_elapsed(&mut self, elapsed: Option<Duration>) {
        self.command_elapsed = elapsed;
    }

    /// How long the current running command has executed, or `None` when no
    /// command is running (the boundary hasn't injected one). See
    /// [`set_command_elapsed`](App::set_command_elapsed).
    #[must_use]
    pub fn command_elapsed(&self) -> Option<Duration> {
        self.command_elapsed
    }

    /// Inject the streaming strip preview's row count before a draw (the
    /// [`set_status_times`] pattern): only the boundary's `ui::StreamRender`
    /// knows how many rows the preview renders — one for a normal reply, the
    /// whole forming block while a table streams — and `ui::preview_rows` /
    /// `ui::live_height` / `ui::cursor_position` must all reserve exactly what
    /// the strip draws. See `docs/table-streaming.md`.
    ///
    /// [`set_status_times`]: App::set_status_times
    pub fn set_stream_preview_rows(&mut self, rows: u16) {
        self.stream_preview_rows = rows;
    }

    /// The boundary-injected streaming-preview row count (see
    /// [`set_stream_preview_rows`]; 0 until a draw injects one — consumers
    /// floor at 1, the single-row preview).
    ///
    /// [`set_stream_preview_rows`]: App::set_stream_preview_rows
    #[must_use]
    pub const fn stream_preview_rows(&self) -> u16 {
        self.stream_preview_rows
    }

    /// The reply text accumulated so far, or `None` when idle.
    #[must_use]
    pub fn streaming_text(&self) -> Option<&str> {
        self.streaming.as_deref()
    }

    /// Finish streaming, returning the completed final-segment text (if any),
    /// recording it in the history, and clearing the streaming state. Returns
    /// `None` — recording nothing — when the final segment is empty (a turn that
    /// ended right after a tool call) or nothing was streaming.
    pub fn finish_stream(&mut self) -> Option<String> {
        let text = self.streaming.take()?;
        // A turn can end right after a tool call with no trailing text; don't
        // record (or later commit) a phantom empty assistant message for it. A
        // whitespace-only tail counts as empty too — it renders to zero rows once
        // trailing blanks are trimmed (`ui::assistant_lines`), so recording it
        // would leave a stray `● ` bullet on a repaint.
        if text.trim().is_empty() {
            return None;
        }
        self.record_message(Role::Assistant, text.clone());
        Some(text)
    }

    /// End the turn: record its `"{done verb} for {elapsed_secs}s"` summary in the
    /// history (so it persists across a resize and lists in the transcript) and
    /// clear the live status. Returns the recorded [`TurnSummary`] for the loop to
    /// commit to scrollback, or `None` if no turn was active. The boundary calls
    /// this on `StreamDone`, just after [`finish_stream`] flushes any final text.
    ///
    /// [`finish_stream`]: App::finish_stream
    pub fn end_turn(&mut self, elapsed_secs: u64) -> Option<TurnSummary> {
        let summary = self.take_turn_summary(elapsed_secs)?;
        self.record_turn_summary(summary.clone());
        Some(summary)
    }

    /// Clear the live status and **build** (but do not record) the turn's
    /// `"{done verb} for {n}s"` summary. Split out of [`end_turn`] so the
    /// boundary can settle a background completion that was still pending at
    /// turn end **between** clearing the status and recording the summary —
    /// landing that notice above the `Done for Ns` summary in both history
    /// and scrollback (invariant 3), the same placement a mid-turn tool
    /// boundary gives it (`docs/background.md`). Returns `None` (still
    /// clearing the status) for an idle composer or a `!` shell turn, whose
    /// committed cell is its own record (`docs/shell-command.md`).
    pub fn take_turn_summary(&mut self, elapsed_secs: u64) -> Option<TurnSummary> {
        let status = self.status.take()?;
        // No usage frame arrived this turn (the dummy, or a provider that
        // omits them): the tokenizer estimate stands in for the context gauge
        // so it — and the auto-compact trigger — still work (docs/compact.md).
        if self.turn_usage_tokens == 0 {
            self.refresh_context_used();
        }
        if status.shell {
            return None;
        }
        Some(TurnSummary {
            verb: status.done_verb,
            secs: elapsed_secs,
            timestamp: self.now_stamp(),
            // Snapshot the running background shells for the `· {n} shells
            // still running` suffix (docs/background.md). A shell that just
            // finished has already left `self.background` (at `bg_exited`),
            // so settling its notice next doesn't change this count.
            shells: self.background.len(),
            // The turn's real billed usage (`apply_usage`) — 0 (hidden) when
            // the backend reported none. See docs/prompt-caching.md.
            tokens: self.turn_usage_tokens,
            cached: self.turn_usage_cached,
        })
    }

    /// Record a summary built by [`take_turn_summary`] into history (so it
    /// survives a resize and lists in the transcript).
    pub fn record_turn_summary(&mut self, summary: TurnSummary) {
        self.history.push(HistoryItem::Summary(summary));
    }

    /// End the in-progress stream because the backend reported an error.
    ///
    /// Records any non-empty partial reply as an assistant message, resolves a
    /// still-running tool as [`ToolStatus::Failed`] with [`ERROR_TOOL_OUTPUT`]
    /// (the contract allows `Error` in place of `StreamDone` with a `ToolEnd`
    /// still owed — leaving it Running would wedge a phantom in the preview
    /// strip, the transcript, and a later turn's interrupt record), then
    /// records the error as a [`Role::Error`] message and clears the streaming
    /// state. Returns the [`StreamError`] for the event loop to flush to
    /// scrollback, or `None` if no reply was in progress. The stream-order
    /// mirror of [`App::interrupt_turn`].
    pub fn fail_stream(&mut self, error: &str) -> Option<StreamError> {
        let streamed = self.streaming.take()?;
        // A /compact turn's half summary dies with the request — the marker
        // lands only at StreamDone, so the old context stands (docs/compact.md).
        // A failed compaction also blocks the auto re-trigger until the next
        // real turn (it would fail the same way in a tight loop).
        if self.compact_buffer.take().is_some() {
            self.auto_compact_blocked = true;
        }
        let partial = if streamed.is_empty() {
            None
        } else {
            self.record_message(Role::Assistant, streamed.clone());
            Some(streamed)
        };
        let tool = self.end_tool(ERROR_TOOL_OUTPUT, false);
        // Any un-started `Waiting` batch siblings never ran — drop them (they
        // only ever lived in the live region, never committed). See
        // `docs/parallel-tools.md`.
        self.tool_queue.clear();
        // A live agent group dies with the turn — resolve it locally (the
        // channel swap drops the backend's own AgentGroupDone); the loop
        // kills its subagents and commits the tree cell. See
        // `docs/agent-tool.md`.
        let agents = self.resolve_live_agent_group();
        self.record_message(Role::Error, error);
        // An error is the turn's terminal state — clear the live status without a
        // "Done" summary; the red error notice is the summary.
        self.status = None;
        Some(StreamError {
            partial,
            tool,
            agents,
            error: error.to_string(),
        })
    }

    /// End the in-progress turn because the user interrupted it (Esc). Returns
    /// the [`InterruptedTurn`] telling the loop how to settle the screen, or
    /// `None` if no turn was in flight. Two outcomes (see `docs/interrupt.md`):
    ///
    /// - **No output yet** — nothing streamed (no non-empty partial reply, no
    ///   running tool) and nothing is queued behind this turn: the submission is
    ///   **undone** rather than interrupted. The turn's just-submitted user
    ///   message(s) are pulled back into the composer
    ///   ([`take_trailing_user_messages`] + [`recall_input`]) and dropped from
    ///   history, the status clears, and **no** `Conversation interrupted`
    ///   notice is recorded — there is nothing to keep, so we roll back to the
    ///   pre-submit state (the user's "move it back to the textarea" case). A
    ///   non-empty queue opts out: the user wants their follow-ups sent, so the
    ///   keep path runs instead.
    /// - **Something streamed** — keep any non-empty partial reply as an
    ///   assistant message (codex never retracts streamed text), resolve a
    ///   still-running tool as [`ToolStatus::Failed`] with
    ///   [`INTERRUPT_TOOL_OUTPUT`], and record the [`INTERRUPT_NOTICE`] as a
    ///   [`Role::Error`] message — **except for a `!` shell turn**, whose
    ///   `⎿ Interrupted by user` cell already says it, so no redundant notice is
    ///   committed. The live status clears **without** a `Done for Ns` summary
    ///   (like [`App::fail_stream`], the notice — or the shell cell — is the
    ///   turn's terminal state).
    ///
    /// [`take_trailing_user_messages`]: App::take_trailing_user_messages
    /// [`recall_input`]: App::recall_input
    pub fn interrupt_turn(&mut self) -> Option<InterruptedTurn> {
        if !self.is_streaming() && !self.turn_active() {
            return None;
        }
        // A /compact turn: drop the half-streamed summary — the swap happens
        // only at StreamDone (codex replaces history at the very end), so an
        // interrupt leaves the old context standing. Taking the buffer also
        // keeps the undo branch below from firing: the history tail may be an
        // unrelated user message (the marker was never appended), and pulling
        // it into the composer would undo a submission this turn never made.
        // See docs/compact.md.
        let compacting = self.compact_buffer.take().is_some();
        if compacting {
            // The user said no — don't restart it at this same turn end
            // (docs/compact.md's one-attempt-per-turn rule).
            self.auto_compact_blocked = true;
        }
        // Take the partial first (empties the streaming buffer either way).
        let partial = self.streaming.take().filter(|text| !text.is_empty());

        // No output produced (no partial, no running or waiting tool) and
        // nothing queued behind it → undo the submission wholesale: roll the
        // user's message(s) back into the composer and drop them from history,
        // recording no notice. The tool queue is peeked (not resolved) so the
        // undo path never touches history or the token tally. A shell turn
        // always has a running tool, so it can never land here — it takes the
        // keep path below (and skips the notice there); likewise any tool of a
        // parallel batch (running *or* still `Waiting`) keeps the queue
        // non-empty, so the undo can't fire mid-batch.
        //
        // The trailing-user-message guard is what makes this safe under the
        // real backend's **multi-round tool loop**: after an earlier round
        // committed an assistant segment + a tool, the streaming buffer is
        // `Some("")` and the tool queue is empty again, so the three empties
        // alone would wrongly fire the undo mid-turn — dropping the notice,
        // orphaning the committed output, and (via `recall_input("")`) wiping a
        // typed draft. When the just-submitted user message(s) are still the
        // history tail, nothing has been committed since, so the undo is the
        // right call; otherwise this turn already produced output and the Kept
        // path below records the notice. See `docs/tools.md` / `docs/interrupt.md`.
        let submission_is_intact =
            matches!(self.history.last(), Some(HistoryItem::Message(m)) if m.role == Role::User);
        if !compacting
            && submission_is_intact
            && partial.is_none()
            && self.tool_queue.is_empty()
            && self.agent_group.is_none()
            && self.queued.is_empty()
        {
            self.status = None;
            let (text, pairs) = self.take_trailing_user_messages();
            // An /init turn's recorded message is the canned prompt, but the
            // user typed "/init" — restoring 1.8KB of text the composer never
            // held would flood it (and a follow-up Ctrl+C would record the
            // prompt into ↑ recall, breaking docs/init.md's no-recall
            // guarantee). Recall the command instead; refresh_command_menu
            // reopens the palette on it — the exact pre-submit state. (A
            // hand-typed submission of the identical text lands here too;
            // "/init" resubmits the same prompt, so nothing is lost.)
            let text = if text == INIT_PROMPT.trim_end() {
                "/init".to_string()
            } else {
                text
            };
            self.recall_input(&text);
            // recall_input replaced the draft, unanchoring any pairs that
            // backed it — discard those (the boundary deletes the orphaned
            // temp files) and restore the undone submission's own pairs so
            // the placeholders back in the composer are backed again.
            let stale = std::mem::take(&mut self.images);
            self.discarded_images
                .extend(stale.into_iter().map(|(_, path)| path));
            self.images = pairs;
            return Some(InterruptedTurn::Undone);
        }

        // Keep what streamed, in stream order: the partial (Assistant) before
        // the tool the interrupt resolves (a ToolStart flushes the buffer, so
        // the two are never both non-empty — but the order holds regardless).
        if let Some(text) = &partial {
            self.record_message(Role::Assistant, text.clone());
        }
        let tool = self.end_tool(INTERRUPT_TOOL_OUTPUT, false);
        // Any un-started `Waiting` batch siblings never ran — drop them (they
        // only lived in the live region, never committed). See
        // `docs/parallel-tools.md`.
        self.tool_queue.clear();
        // A live agent group is stopped with the turn: settle its members as
        // interrupted and record the tree cell — the loop kills the
        // subagent threads via the registry (docs/agent-tool.md).
        let agents = self.resolve_live_agent_group();
        // A `!` shell turn's `⎿ Interrupted by user` cell is its own record —
        // committing a second `Conversation interrupted` line would be
        // redundant, so skip the notice for it.
        let notice = if self.status.as_ref().is_some_and(|s| s.shell) {
            None
        } else {
            self.record_message(Role::Error, INTERRUPT_NOTICE);
            Some(INTERRUPT_NOTICE)
        };
        self.status = None;
        Some(InterruptedTurn::Kept {
            partial,
            tool,
            notice,
            agents,
        })
    }

    /// Remove and return the turn's just-submitted user message(s) — the
    /// maximal run of trailing [`Role::User`] messages in [`history`] — joined
    /// with newlines (a batch flushed as one turn records several; codex's
    /// Alt+Up joins them the same way), plus their image attachments rebuilt
    /// as `(placeholder, path)` pairs: each message's paths are recorded in
    /// its text's placeholder-occurrence order (`paste::distribute_images`),
    /// so zipping the occurrences back over them per message is exact — a
    /// duplicate `[Image #1]` across a merged batch re-keys to each message's
    /// own path. The undo path of [`interrupt_turn`] hands the text to
    /// [`recall_input`] and the pairs to the composer. Empty when there is no
    /// trailing user message (a bare `begin_stream` with no submission). A
    /// previous turn always ends with a summary, notice, or tool — never a
    /// user message — so this only ever reclaims the current turn's own input.
    ///
    /// [`history`]: App::history
    /// [`interrupt_turn`]: App::interrupt_turn
    /// [`recall_input`]: App::recall_input
    pub(super) fn take_trailing_user_messages(&mut self) -> (String, Vec<(String, PathBuf)>) {
        let mut messages = Vec::new();
        while self.history.len() > self.undo_floor
            && matches!(self.history.last(), Some(HistoryItem::Message(m)) if m.role == Role::User)
        {
            if let Some(HistoryItem::Message(m)) = self.history.pop() {
                messages.push(m);
            }
        }
        if !messages.is_empty() {
            self.history_generation += 1;
        }
        messages.reverse();
        let mut pairs = Vec::new();
        let mut texts = Vec::with_capacity(messages.len());
        for message in messages {
            pairs.extend(
                crate::paste::image_placeholder_occurrences(&message.text)
                    .into_iter()
                    .zip(message.images),
            );
            texts.push(message.text);
        }
        (texts.join("\n"), pairs)
    }
}
