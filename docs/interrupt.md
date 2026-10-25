# Esc interrupts the in-flight turn — Design

Date: 2026-06-10

## Goal

Pressing **Esc while a turn is generating** stops the turn immediately — the
way openai/codex interrupts a running task — instead of quitting the app. The
partial reply stays in the conversation, a notice tells the user what
happened, and the input box is ready for the next message at once.

Two deliberate divergences from codex tune what happens when there is nothing
worth keeping:

- **No output yet → undo, don't notify.** If Esc lands before the turn has
  produced anything (no partial reply, no tool, no `write` that had begun
  streaming into its cell — `docs/write-streaming.md`) and nothing waits
  behind it —
  neither a queued follow-up nor a message handed to this turn that it never
  read (`docs/queue.md`) — the submission is **rolled back** rather than
  interrupted: the just-sent
  user message goes back into the composer to edit, and **no** `Conversation
  interrupted` notice is committed. There was no output, so we return to the
  pre-submit state instead of leaving a stray user bubble + notice on screen.
- **A `!` shell interrupt commits no notice.** A cancelled shell command already
  resolves its cell to `⎿ Interrupted by user`; a second `Conversation
  interrupted` line under it would be redundant, so the shell turn skips the
  notice (its cell is the record — like the `Ran for Ns` summary it also omits).

A third divergence runs the other way, keeping something codex drops: **a kept
turn keeps its receipt.** Under the notice commits the dim summary a finished
turn ends on — `Worked for 8s · ≈24.3k tokens` — so stopping a turn no longer
erases the two numbers that showed what it had cost (see *The receipt an
interrupted turn keeps*, below).

## What codex does (ported from `/tmp/codex/codex-rs/tui`)

- **A single Esc interrupts immediately** while a task is running — no
  double-press confirmation (`bottom_pane/mod.rs`: the `interrupt_turn`
  binding fires when `is_task_running`). When a popup is open, **dismissing
  the popup wins** over interrupting.
- The partial streamed text is **kept** in the transcript; nothing already
  shown is retracted.
- An **error-styled cell** is committed:
  `"Conversation interrupted - tell the model what to do differently. …"`
  (`chatwidget/turn_runtime.rs::interrupted_turn_message`). There is **no**
  "done" summary for an interrupted turn — the notice is its terminal state.
- The live status row carries the hint while a task runs:
  `(0m 00s • Esc to interrupt)` (`status_indicator_widget.rs`).
- Esc when idle is codex's backtrack ("edit previous message") — now ported
  (`docs/backtrack.md`): idle Esc arms the Esc-Esc gesture whenever a previous
  user message exists, and quits only with nothing to backtrack to. The
  interrupt keeps strict precedence: `turn_active()` Esc always interrupts,
  and only the *next* Esc (now idle) arms — codex's interrupt → prime →
  preview chain.

## The mapping onto this codebase

### Key handling (`App::on_key_conversation`)

Priority order, mirroring codex's "popup wins" rule:

1. `Esc` with the slash-command palette open → dismiss the palette
   (unchanged).
2. `Esc` while a turn is active (`App::turn_active()`) → **`Action::Interrupt`**
   (new variant).
3. `Esc` otherwise → `Action::Quit` (unchanged).

Esc inside the Ctrl+O overlay still just returns to the chat (the overlay is
a read-only viewer; interrupt from the conversation). Ctrl+C still quits with
an empty input (after codex's composer-clear step — a first Ctrl+C with text
in the input only clears the draft, see `docs/design.md`); a quit mid-stream
already cancels and reaps the backend.

### Pure state (`App::interrupt_turn`)

A sibling of `fail_stream`, returning an enum that tells the loop how to settle
the screen — codex's "keep what streamed" *or* this codebase's "nothing
streamed, so undo it":

```rust
pub enum InterruptedTurn {
    Undone,                          // no output + empty queue: rolled back
    Kept {
        partial: Option<String>,     // the kept partial reply, if any text streamed
        tools: Vec<ToolCall>,        // every call in flight — the running one and each
                                     // ⎿ Waiting… sibling — resolved Failed, in order
        notice: Option<&'static str>,// INTERRUPT_NOTICE, or None for a shell turn
        agents: Option<AgentGroup>,  // the live agent group, resolved interrupted
        summary: Option<TurnSummary>,// the receipt — built here, recorded by the
                                     // boundary; None for a shell or /compact turn
    },
}
```

`interrupt_turn(elapsed_secs)` — the turn's wall clock comes from the
boundary, like `end_turn`'s — takes the partial buffer first, then branches:

**Undo path** — `partial.is_none() && current_tool.is_none() && queued.is_empty()
&& steered.is_empty() && !steered_this_turn` (nothing was produced, nothing
waits behind it, and nothing was read mid-turn):

- The **pending** guard: the interrupt is what sends a message the turn never
  read, so pulling the submission back while that dispatches would read as the
  undo having done nothing.
- The **delivered** guard: a message the turn *did* read leaves a user message
  as the history tail even after the turn committed a reply and tool cells, so
  the tail test alone would fire the undo in the gap between the delivery and
  the next round's first token — yanking a message the model has already read
  back into the composer and committing no notice over the output on screen
  (`docs/queue.md`).

Then:

- `take_trailing_user_messages` pops the maximal run of trailing `Role::User`
  messages (the turn's own input — a previous turn always ends with a summary,
  notice, or tool, never a user message) and joins them with `\n` (a batch
  flushed as one turn recorded several; codex's Alt+Up rejoins the same way).
- `recall_input` puts that text back in the composer.
- The status clears and **nothing** is recorded — no partial, no tool (it is
  peeked, not resolved, so the token tally is untouched), no notice.
- Returns `Undone`; the loop repaints scrollback without the rolled-back
  message. A shell turn never lands here — it always has a running tool.

**Keep path** — something streamed (or a queue is waiting):

- Keeps any non-empty partial as a normal `Role::Assistant` history message
  (codex keeps partial output).
- **Every call in flight** is resolved as `ToolStatus::Failed` with output
  `INTERRUPT_TOOL_OUTPUT` (`"Interrupted by user"`) via the existing
  `end_tool` path (`App::fail_live_queue`) — the running call, cancelled
  mid-run (codex: aborted tools "may have partially executed"), **and** every
  `⎿ Waiting…` sibling of a parallel batch behind it, in the batch's order
  (the front may itself still be waiting, on a permission prompt or auto
  mode's classifier — the approve seam runs before its `ToolStart`). Each is
  recorded, so each keeps its red cell on screen and in the Ctrl+O view, and
  the next request's derived context replays the whole round — one
  assistant message holding every call under its provider id and verbatim
  arguments, each answered `Interrupted by user` (Claude Code's answer for a
  tool use an abort left without a result). Dropping the siblings, as this
  used to, erased calls the model had made from everything after the Esc:
  the next turn saw the first call alone. See `docs/parallel-tools.md`
  *Interrupting a batch* — and its *Limitations* for the calls a round never
  announced (behind a foreground agent group, an unreached task call), which
  still leave no record.
- Records the notice as a `Role::Error` message —
  `INTERRUPT_NOTICE` = `"Conversation interrupted - tell the model what to do
  differently."` (codex's wording minus its `/feedback` plug) — **unless it is
  a `!` shell turn** (`status.shell`), whose `⎿ Interrupted by user` cell is its
  own record, so `notice` is `None` and no `Role::Error` message is recorded.
- Builds the turn's **receipt** off the live status — before the calls above
  resolve, so it reads the tally the line was showing — and hands it back as
  `summary` without recording it (*The receipt an interrupted turn keeps*,
  below); `None` for a shell or `/compact` turn.
- Clears the live status.

`None` (recording nothing) when no turn is in flight.

Note the buffer/tool exclusivity: a `ToolStart` (or a batch's `ToolBatch`)
flushes the streaming segment first, so when a tool is in flight the buffer is
empty — `partial` is never `Some` beside a non-empty `tools` in practice, and
history order (partial, tools, agent group, notice, then the receipt the
boundary records) matches stream order regardless.

### The event loop (`main.rs`, `Action::Interrupt` arm)

1. **`abandon_inflight`** — `cancel.cancel()`, then **detach** the backend
   thread (never `join()` it on the loop) and **swap in a fresh reply
   channel**. This is the whole fix for the *interrupt-lag* freeze (below):
   the old code `join`ed the thread here, which blocks the single-threaded
   loop until the thread unwinds — fine for the dummy (it polls the token
   every ≤20 ms) but not for a real network backend parked in a blocking read
   (see *The interrupt-lag freeze*). Detaching keeps the loop responsive; the
   channel swap is both the "thread stopped sending" guarantee **and** the
   drain — any stale event the dying thread still emits (a final chunk, or a
   `ToolStart` that would otherwise wedge a phantom "running" tool) goes to
   its old sender, whose receiver we just dropped, so it can never reach the
   next turn (which spawns on the new sender). The detached handle is parked
   in `reaping` and swept when finished (`is_finished()`, non-blocking).
2. The turn's clock is read **before** step 1, which stops it — the receipt
   says how long the turn ran. Then `app.interrupt_turn(elapsed)` and branch
   on the outcome:
   - **`Undone`** — the submission was rolled back (message back in the composer,
     dropped from history). `repaint_conversation(ReflowClear::Purge)` rebuilds
     scrollback from the now-truncated history, so the user bubble vanishes and
     the composer shows the restored draft. No commit, and no queue flush (the
     empty queue is the undo's precondition).
   - **`Kept { partial, tools, notice, agents, summary }`** — commit like
     `StreamDone` does: reseat the viewport to its idle height first (the
     streaming strip is gone — invariant 3), then `insert_before` the partial,
     every cancelled call (collapsed, red, blank-separated), the agent group,
     and the notice (`commit_turn_failure`, each with a blank spacer). The
     calls are read back off the history by count
     (`ui::resolved_tools_commit_lines`): the notice is already recorded
     behind them, so `ui::tool_commit_lines` — which commits the one call that
     just resolved, the last history item — would find a message there and
     commit nothing, the screen showing the notice with no cell above it at
     all. `notice` is `None` for a shell turn, so its `⎿ Interrupted by user`
     cell stands alone. Then the background completions held during the turn
     settle, and the receipt is recorded (`App::record_turn_summary`) and
     committed under them (`ui::summary_lines`) — `StreamDone`'s order, so
     the summary closes the turn. Then flush the front queued batch
     (`flush_next_queued`) onto the **new** channel — Esc sends the queue
     right away.

`Action::Interrupt` can only originate in the conversation view (overlay Esc
returns instead), so the commits never touch the alternate screen.

**`/clear` mid-turn reuses step 1** (cancel + detach + swap — the same
"nothing stale can arrive afterwards" guarantee, now without the freeze) but
skips the commits entirely: `App::clear_conversation` wipes history, the
streaming buffer, the running tool, the status, *and the queued backlog*,
recording no partial, no notice, and no summary — the user asked for a blank
slate, not a finished turn. See `docs/design.md` (the `/clear` paragraph) and
`smoke.sh` Phase 16. **Quit mid-turn** likewise cancels without joining, so
the terminal restore isn't delayed by a wedged backend thread — the process
exits right after and the OS reaps it.

### The interrupt-lag freeze (why we detach instead of join)

The reply backend streams on a plain OS thread and stops **cooperatively** —
it polls the `CancelToken` between chunks. The dummy checks it every ≤20 ms,
so `cancel + join` returns almost instantly. A **real** backend
(`llm::LlmBackend`) *used to be* different: during the pre-first-token pause
the thread was parked in a *blocking* network op — `reqwest`'s `send()`
(waiting for the response headers, which never polls the token) or the first
SSE `read()` (polled only *between* reads) — waking only after the client's
per-operation timeout, 3 s at the time. So after Esc the thread could not
observe the cancel until its in-flight op returned — the network responding
(~1–2 s) or the 3 s cap. (Today those blocking ops live on a further *detached
transport thread*, and the streaming thread polls the token every ~50 ms while
receiving from it — `openai::drain_stream`, see `docs/llm.md` — so it
acknowledges a cancel promptly even mid-stall. The detach-don't-join
discipline below still stands: it is what keeps *any* backend's worst case off
the loop.)

The old arm called `handle.join()` on that thread on the **single-threaded**
(`current_thread`) tokio loop, so the entire event loop — draw ticks, the
status animation, keystrokes — blocked for that whole window: the reported
"press Esc → the spinner freezes for 1–2 s, then unfreezes" bug. Reproduced
deterministically offline by `stream::StallAi` (a backend that ignores the
cancel for `ALTER_ZERO_STALL_MS` ms, modelling the wedged read): the stall
backend streams nothing before the stall, so Esc lands on the **undo** path —
with the old `join` the status line stays frozen ~`STALL_MS`; with
`abandon_inflight` it clears (and the message returns to the composer) within a
frame. `smoke.sh` Phase 32 asserts that prompt settle.

### The status-line hint (`ui::status_line`)

The detail clause gains codex's discoverability hint as its final, dim
segment (lowercase, matching this codebase's hint convention —
`(ctrl+o to expand)`, `esc return`):

```
( ●    ) Working… (3s · ↓ 27 tokens · Thinking for 2s · esc to interrupt)
```

### The receipt an interrupted turn keeps

Date: 2026-10-25

codex commits nothing under its notice, and neither did this port: the red
line was the turn's whole terminal state. But an interrupt is the moment the
receipt matters most. The user stopped the turn because it was running long
or going wrong, and the two numbers that showed it — the status line's elapsed
and its token tally — vanished with the line. So the **keep** path now commits
the dim summary a finished turn ends on, under the notice:

```
● Here is how the parser handles a nested block. It first

● Conversation interrupted - tell the model what to do differently.

Worked for 8s · ≈24.3k tokens (8k cached)
```

- **Verb and time** follow the finished turn's rule
  (`docs/status-indicator.md`): the past tense of the verb the line last wore,
  and the boundary's own clock, read before `abandon_inflight` stops it. Both
  turn ends build their summary with the one `App::turn_summary`, so the two
  differ only in the estimated share each hands it, and the loop commits both
  through `Session::commit_turn_summary`.
- **Tokens** are the status line's tally at the moment of the Esc: the
  provider's usage frames for the rounds that finished, plus the tokenizer's
  count of the round the Esc cut short. A provider sends a round's `usage`
  frame when the round ends, so an abandoned round never reports one, and its
  share can only be the estimate. The tally is read **before** the interrupt
  resolves the calls in flight: the `Interrupted by user` answers it charges
  are the *next* request's upload, not this turn's.
- **`≈` marks any estimated share.** `docs/prompt-caching.md` promises that a
  receipt's number is the provider's own, never a guess, and the record keeps
  the two apart to hold to that: `TurnSummary::tokens` is still only what
  frames reported, and `TurnSummary::estimated` is the tokenizer's count on
  top. `ui::summary_lines` folds them into one total and marks it with the
  `/context` page's `≈` (`SUMMARY_ESTIMATE_MARK`) whenever the estimate is
  non-zero. Esc during a lone tool call, after the round's frame snapped the
  tally, has nothing estimated and reads bare. Esc mid-stream, or on the
  offline demo (which reports no usage at all), reads `≈`. The cached and
  written halves come from frames only, so they are never marked.
- **The estimate reads low.** The tally never counted the cut round's re-sent
  context: the system prompt, the tool schemas and the conversation so far,
  which is what a finished round's frame adds when it snaps the tally up. The
  receipt says what the turn streamed plus what the provider confirmed. It is
  not the bill; the provider's dashboard is.
- **No receipt** when there is no turn to account for, or when the turn keeps
  its own record (the shell and `/compact` cases are the ones a finished
  turn's summary skips too):
  - the **undo** (nothing streamed, the submission went back to the
    composer, and the screen returns to before it was sent);
  - a **`!` shell** turn (its `⎿ Interrupted by user` cell is its record, as
    a `Ran for Ns` line never was);
  - a **`/compact`** turn (its record is the `● Context compacted` cell, which
    an interrupted compaction never earns, and its fixed `Compacting` verb
    has no past tense for a summary to open with).
- **Order.** History and scrollback gain the partial, the calls, the agent
  group and the notice, then any background completion that settles at this
  turn end, then the receipt. That is `StreamDone`'s order, where the summary
  closes the turn (`docs/background.md`). It is why `interrupt_turn` only
  *builds* the receipt and the boundary *records* it
  (`App::record_turn_summary`), the split `take_turn_summary` already makes.
- **What persists.** The receipt is an ordinary `HistoryItem::Summary`. A
  resize or a modal's close rebuilds it, Ctrl+O lists it, `/export` writes it,
  and the rollout keeps it, with `estimated` beside the other counts and
  omitted when zero. A resumed interrupted turn keeps its `≈`, and a finished
  turn's record serializes exactly as before. The derived context skips
  summaries as chrome, so the next request, and the prompt cache behind it,
  are unchanged.

A backend `Error` still ends on its red error alone (`App::fail_stream`).
Subagents are unchanged too: a stopped agent keeps its elapsed and tally on
its red roster row for `AGENT_STOPPED_LINGER`, and its own transcript records
no receipt (`docs/agent-tool.md`).

## Testing

- `app`: Esc while a turn is active → `Action::Interrupt`; Esc idle still
  quits; Esc with the palette open still only dismisses it (turn keeps
  running); `interrupt_turn` — the **keep** path — keeps the partial + records
  the notice (`Kept { notice: Some(_) }`), resolves a running tool as Failed
  with `"Interrupted by user"`, clears the status, stamps with the injected
  clock, and is a `None` no-op when idle.
- `app` (the receipt): a kept turn hands back the summary a finished turn
  ends on — its verb the one the last frame drew, a rotated one included,
  its seconds the boundary's, the running shells counted
  (`an_interrupted_turn_hands_back_its_receipt`,
  `the_interrupted_receipt_names_the_verb_the_last_frame_drew`,
  `an_interrupted_turn_hands_on_its_last_verb_too`,
  `the_interrupted_receipt_counts_the_shells_still_running`) — built, not
  recorded, so the boundary can settle held completions above it, and once
  recorded the history reads partial, notice, receipt
  (`the_interrupted_receipt_is_built_not_recorded`); the frames' tokens stay
  `tokens` and the cut round's share is `estimated`, the cache halves the
  frames' alone (`the_interrupted_receipt_keeps_reported_usage_apart_from_the_estimate`);
  the tally is read before the calls resolve, so a lone call interrupted
  after its round's frame estimates nothing
  (`an_interrupt_with_nothing_streamed_since_the_frame_estimates_nothing`);
  a `!` shell or a `/compact` turn keeps none
  (`an_interrupted_shell_turn_keeps_no_receipt`,
  `an_interrupted_compaction_keeps_no_receipt`); a finished turn estimates
  nothing (`a_finished_turn_estimates_nothing`). `ui`: the total folds the
  estimate in and wears `≈` only when there is one
  (`summary_lines_mark_an_estimated_share_with_approx`). `session`: the
  estimate round-trips, and a summary with none writes the line it always did.
- `app` (the **undo** path, req 1): interrupting with no output (`Kept`'s
  opposite, `Undone`) drops the turn's user message(s) from history, restores
  their text (a batch rejoined by `\n`) to the composer, records **no** notice,
  and only reclaims the *current* turn's messages; a non-empty queue opts out
  (back to the keep path, notice committed).
- `app` (the shell notice skip, req 2): interrupting a `!` shell turn resolves
  the command Failed with `"Interrupted by user"` and returns `notice: None` —
  no `Role::Error` `Conversation interrupted` message.
- `app` (a parallel batch): Esc mid-batch resolves the running call **and**
  every `⎿ Waiting…` sibling, in order, each recorded ahead of the notice
  (`interrupt_mid_batch_resolves_the_running_call_and_every_waiting_sibling`);
  so does Esc while the whole batch still waits on the front call's permission
  prompt (`interrupt_while_the_whole_batch_still_waits_resolves_every_call`)
  and a backend error mid-batch (`fail_stream_mid_batch_resolves_every_call_in_the_batch`);
  an announced round stamps each waiting call with the model's own arguments,
  and the derived context of an interrupted round is one assistant message
  carrying every call under its provider id, each answered `Interrupted by
  user` (`an_interrupted_round_replays_every_call_the_model_made`).
- `ui`: `resolved_tools_commit_lines` commits every resolved cell in order,
  blank-separated, matching the rebuild, and flushes an MCP run the
  interrupted call was holding; `agents`: a subagent stopped (or failing)
  mid-batch records every call; `llm::agent`: a call a cancel left unexecuted
  is answered `Interrupted by user` in the stored message list.
- `ui`: `status_line` ends with the dim `esc to interrupt` hint in every phase.
- `stream`: `StallAi` (the test double for a wedged backend) ignores the
  cancel for its stall — a caller that `join()`s it pays the full stall — and
  streams a normal reply when left to run.
- `src/tui/` (smoke, Phase 8): mid-stream Esc (after a partial streamed) leaves
  the partial text and the `Conversation interrupted` notice on screen, clears
  the status line (no `esc to interrupt`), commits exactly one receipt under
  the notice — `Worked for Ns · ≈N tokens`, all of it estimated, since the
  dummy reports no usage — and the app still completes a following turn
  normally, its summary the second on screen. Phase 13 waits for that second
  summary too, requires exactly two, and checks that turn 1's receipt sits
  between its notice and the queued message the Esc sent.
- `src/tui/` (smoke, Phase 32 — the interrupt-lag regression guard, req 1): with a
  backend stalled 3 s (`ALTER_ZERO_STALL_MS`, ignoring the cancel and streaming
  nothing), Esc **undoes** the turn — the status line clears **within a frame**
  (asserted `< 1.5 s`, well under the stall) and `hello there` returns to the
  composer with **no** `Conversation interrupted` notice — proving the loop
  detaches the thread rather than `join()`ing it. Measured live: ~3.0 s (old,
  frozen) → ~0.015 s (fixed).
- `src/tui/` (smoke, Phase 19 — req 2/3): a running `!sleep 9` shows the
  `⎿ Running… (Ns)` preview with **no** status line, and Esc resolves it
  `⎿ Interrupted by user` with **no** `Conversation interrupted` notice.
- `src/tui/` (smoke, Phase 123 — a parallel batch): Esc while the dummy's
  three-call `Bash(ping …)` batch runs commits all three red
  `⎿ Interrupted by user` cells, in batch order, above the notice, with the
  receipt under it, Ctrl+D shows all three calls answered (and no receipt —
  a summary is chrome), and a resize's purge rebuild shows each cell and the
  receipt exactly once; Esc on the batch's permission prompt (both cells
  still `⎿ Waiting…`) resolves both calls, on screen and in Ctrl+D, under one
  receipt. Before the fix the pane showed the notice alone and Ctrl+D the
  first call alone.
