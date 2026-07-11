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
  produced anything (no partial reply, no tool) and nothing is queued behind
  it, the submission is **rolled back** rather than interrupted: the just-sent
  user message goes back into the composer to edit, and **no** `Conversation
  interrupted` notice is committed. There was no output, so we return to the
  pre-submit state instead of leaving a stray user bubble + notice on screen.
- **A `!` shell interrupt commits no notice.** A cancelled shell command already
  resolves its cell to `⎿ Interrupted by user`; a second `Conversation
  interrupted` line under it would be redundant, so the shell turn skips the
  notice (its cell is the record — like the `Ran for Ns` summary it also omits).

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
        tool: Option<ToolCall>,      // the cancelled tool, resolved Failed, if one ran
        notice: Option<&'static str>,// INTERRUPT_NOTICE, or None for a shell turn
    },
}
```

`interrupt_turn` takes the partial buffer first, then branches:

**Undo path** — `partial.is_none() && current_tool.is_none() && queued.is_empty()`
(nothing was produced and nothing waits behind it):

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
- A **running tool** is resolved as `ToolStatus::Failed` with output
  `INTERRUPT_TOOL_OUTPUT` (`"Interrupted by user"`) via the existing
  `end_tool` path — it was cancelled mid-run, and the record survives in the
  Ctrl+O view (codex: aborted tools "may have partially executed").
- Records the notice as a `Role::Error` message —
  `INTERRUPT_NOTICE` = `"Conversation interrupted - tell the model what to do
  differently."` (codex's wording minus its `/feedback` plug) — **unless it is
  a `!` shell turn** (`status.shell`), whose `⎿ Interrupted by user` cell is its
  own record, so `notice` is `None` and no `Role::Error` message is recorded.
- Clears the live status **without** a `Done for Ns` summary (like
  `fail_stream`: the notice — or the shell cell — is the summary).

`None` (recording nothing) when no turn is in flight.

Note the buffer/tool exclusivity: a `ToolStart` flushes the streaming segment
first, so when a tool is running the buffer is empty — `partial` and `tool`
are never both `Some` in practice, and history order (partial, tool, notice)
matches stream order regardless.

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
2. Reset `committed` / `turn_start` / `thinking_start`, then `app.interrupt_turn()`
   and branch on the outcome:
   - **`Undone`** — the submission was rolled back (message back in the composer,
     dropped from history). `repaint_conversation(ReflowClear::Purge)` rebuilds
     scrollback from the now-truncated history, so the user bubble vanishes and
     the composer shows the restored draft. No commit, and no queue flush (the
     empty queue is the undo's precondition).
   - **`Kept { partial, tool, notice }`** — commit like `StreamDone` does: reseat
     the viewport to its idle height first (the streaming strip is gone —
     invariant 3), then `insert_before` the partial, the cancelled tool
     (collapsed, red), and the notice (`commit_turn_failure`, each with a blank
     spacer). `notice` is `None` for a shell turn, so its `⎿ Interrupted by user`
     cell stands alone. Then flush the front queued batch (`flush_next_queued`)
     onto the **new** channel — Esc sends the queue right away.

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
cancel for `INLINE_TUI_STALL_MS` ms, modelling the wedged read): the stall
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
  with `"Interrupted by user"`, records **no** summary, clears the status,
  stamps with the injected clock, and is a `None` no-op when idle.
- `app` (the **undo** path, req 1): interrupting with no output (`Kept`'s
  opposite, `Undone`) drops the turn's user message(s) from history, restores
  their text (a batch rejoined by `\n`) to the composer, records **no** notice,
  and only reclaims the *current* turn's messages; a non-empty queue opts out
  (back to the keep path, notice committed).
- `app` (the shell notice skip, req 2): interrupting a `!` shell turn resolves
  the command Failed with `"Interrupted by user"` and returns `notice: None` —
  no `Role::Error` `Conversation interrupted` message.
- `ui`: `status_line` ends with the dim `esc to interrupt` hint in every phase.
- `stream`: `StallAi` (the test double for a wedged backend) ignores the
  cancel for its stall — a caller that `join()`s it pays the full stall — and
  streams a normal reply when left to run.
- `main.rs` (smoke, Phase 8): mid-stream Esc (after a partial streamed) leaves
  the partial text and the `Conversation interrupted` notice on screen, clears
  the status line (no `tokens`), commits no `Done for`, and the app still
  completes a following turn normally.
- `main.rs` (smoke, Phase 32 — the interrupt-lag regression guard, req 1): with a
  backend stalled 3 s (`INLINE_TUI_STALL_MS`, ignoring the cancel and streaming
  nothing), Esc **undoes** the turn — the status line clears **within a frame**
  (asserted `< 1.5 s`, well under the stall) and `hello there` returns to the
  composer with **no** `Conversation interrupted` notice — proving the loop
  detaches the thread rather than `join()`ing it. Measured live: ~3.0 s (old,
  frozen) → ~0.015 s (fixed).
- `main.rs` (smoke, Phase 19 — req 2/3): a running `!sleep 9` shows the
  `⎿ Running… (Ns)` preview with **no** status line, and Esc resolves it
  `⎿ Interrupted by user` with **no** `Conversation interrupted` notice.
