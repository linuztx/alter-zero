# Esc interrupts the in-flight turn — Design

Date: 2026-06-10

## Goal

Pressing **Esc while a turn is generating** stops the turn immediately — the
way openai/codex interrupts a running task — instead of quitting the app. The
partial reply stays in the conversation, a notice tells the user what
happened, the turn's summary under it says what the work cost, and the input
box is ready for the next message at once.

Three deliberate divergences from codex. The first two tune what happens when
there is nothing worth keeping:

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

The third is about what was kept:

- **A kept interrupt closes on its summary.** codex ends an interrupted turn
  on the notice alone. Here the turn still gets the dim line a finished turn
  ends on, under the notice — `Worked for 8s · 24.3k tokens (20k cached)` — so
  the user knows how long the work ran and what it used before they stopped
  it. See *The summary under the notice* below.

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
  "done" summary for an interrupted turn — the notice is its terminal state
  (this codebase adds one under it — the third divergence above).
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
        summary: Option<TurnSummary>,// the closing `Worked for 8s · …` line, built
                                     // but not recorded — None for a shell or /compact
    },
}
```

`interrupt_turn` takes the partial buffer first, then branches:

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
- Clears the live status into the turn's **summary**, built from it and the
  `elapsed_secs` the loop hands in (`interrupt_turn(elapsed_secs)`, the
  `end_turn` shape) but left for the loop to record — see *The summary under
  the notice*. `None` for a shell turn and a `/compact` turn.

`None` (recording nothing) when no turn is in flight.

Note the buffer/tool exclusivity: a `ToolStart` (or a batch's `ToolBatch`)
flushes the streaming segment first, so when a tool is in flight the buffer is
empty — `partial` is never `Some` beside a non-empty `tools` in practice, and
history order (partial, tools, agent group, notice) matches stream order
regardless.

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
2. Reset `committed` / `turn_start` / `thinking_start`, then
   `app.interrupt_turn(elapsed)` — the turn's seconds, read off `turn_start`
   *before* step 1 clears it — and branch on the outcome:
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
     cell stands alone. Then settle the background completions held during
     the turn and record + commit the `summary` under them
     (`commit_turn_summary`, the `StreamDone` arm's own helper). Then flush
     the front queued batch (`flush_next_queued`) onto the **new** channel —
     Esc sends the queue right away.

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

### The summary under the notice

A kept interrupt ends the way a finished turn does: on the dim summary line
(`ui::summary_lines`, `docs/status-indicator.md`), committed under the red
notice —

```
● Here is the part of the answer that strea

● Conversation interrupted - tell the model what to do differently.

Worked for 8s · 24.3k tokens (20k cached)
```

— so the user sees how long the work ran and what it used before they stopped
it, not only that it stopped.

- **The verb and the seconds are a finished turn's**: the past tense of the
  verb the status line wore last, and the turn's wall clock, read by the loop
  before `abandon_inflight` stops it and handed to
  `interrupt_turn(elapsed_secs)`, the `end_turn(elapsed_secs)` shape. The two
  turn ends share one builder (`App::turn_summary`), so the lines differ only
  in the token count each is handed.
- **The tokens are the tally the status line showed at the Esc**, not a
  provider receipt. A finished turn's `· N tokens` is the sum of its rounds'
  usage frames (`docs/prompt-caching.md`), but the round the Esc cuts off
  never sends its frame — its stream is dropped mid-flight — so a receipt
  alone would leave out the very round the user stopped, and would show no
  tokens at all for an answer cut off in its first round. The tally is the
  number the user was watching: every frame that did arrive plus the live
  estimate of the cut-off round, so nothing jumps as the line turns into the
  summary. Like the status line, it under-counts that last round's re-sent
  input, which only its frame would have reported. The `(N cached · N
  written)` beside it stays the arrived frames' own: a cached share is never
  estimated.
- **It closes the turn.** `interrupt_turn` builds the summary but does not
  record it (`InterruptedTurn::Kept::summary`): the loop commits the remains,
  settles the background completions held during the turn, and only then
  records and commits the summary (`Session::commit_turn_summary`, shared with
  the `StreamDone` arm). That is the finished turn's order (its
  `take_turn_summary` split, `docs/background.md`), so a late completion lands
  above the line and the summary stays the turn's last row in history and
  scrollback alike. Being history, it survives a resize rebuild and shows in
  the Ctrl+O transcript, `/export` and a `/resume`; the derived context skips
  it as chrome, like every summary, so the model never reads it.
- **None where a finished turn has none**: a `!` shell turn (its `⎿
  Interrupted by user` cell is the record, and a finished command has no `Ran
  for Ns` line either) and a `/compact` turn (it compacted nothing; a finished
  one's record is its marker cell). The undo path records nothing at all, summary included — the turn is
  rolled back as if it was never sent. A backend error still ends on its red
  notice alone.

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

## Testing

- `app`: Esc while a turn is active → `Action::Interrupt`; Esc idle still
  quits; Esc with the palette open still only dismisses it (turn keeps
  running); `interrupt_turn` — the **keep** path — keeps the partial + records
  the notice (`Kept { notice: Some(_) }`), resolves a running tool as Failed
  with `"Interrupted by user"`, clears the status, stamps with the injected
  clock, and is a `None` no-op when idle.
- `app` (the summary): a kept interrupt hands back its summary without
  recording it — the done verb, the boundary's seconds, the clock's stamp —
  and once recorded it closes the turn under the notice
  (`an_interrupted_turn_hands_back_its_summary_without_recording_it`); its
  tokens are the tally the line showed, billed rounds plus the cut-off
  round's estimate, beside the billed cache halves
  (`an_interrupted_turns_summary_keeps_the_tally_its_line_showed`), the
  estimate alone with no frame (`…_with_no_usage_frame_keeps_its_estimate`);
  it counts the running shells, names the verb a rotation moved the line to
  (`an_interrupted_turn_hands_on_its_last_verb_too`), and is `None` for a `!`
  shell and a `/compact` turn.
- `app` (the **undo** path, req 1): interrupting with no output (`Kept`'s
  opposite, `Undone`) drops the turn's user message(s) from history, restores
  their text (a batch rejoined by `\n`) to the composer, records **no** notice,
  and only reclaims the *current* turn's messages; a non-empty queue opts out
  (back to the keep path, notice committed).
- `app` (the shell notice skip, req 2): interrupting a `!` shell turn resolves
  the command Failed with `"Interrupted by user"` and returns `notice: None` —
  no `Role::Error` `Conversation interrupted` message — and `summary: None`.
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
  the status line (no `esc to interrupt`), commits the turn's `Worked for Ns ·
  N tokens` summary under the notice, and the app still completes a following
  turn normally, its summary the second on screen. Phase 13: with a message
  queued, the interrupted turn's summary lands between its notice and the
  queued `❯ world` that Esc sends.
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
  `⎿ Interrupted by user` cells, in batch order, above the notice, the turn's
  summary under it, Ctrl+D shows all three calls answered, and a resize's
  purge rebuild shows each cell — and the summary — exactly once; Esc on the
  batch's permission prompt (both cells still `⎿ Waiting…`) resolves both
  calls, on screen and in Ctrl+D, and closes on the summary too. Before the
  fix the pane showed the notice alone and Ctrl+D the first call alone.
