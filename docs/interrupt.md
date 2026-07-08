# Esc interrupts the in-flight turn — Design

Date: 2026-06-10

## Goal

Pressing **Esc while a turn is generating** stops the turn immediately — the
way openai/codex interrupts a running task — instead of quitting the app. The
partial reply stays in the conversation, a notice tells the user what
happened, and the input box is ready for the next message at once.

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

A sibling of `fail_stream`, returning what the loop must flush:

```rust
pub struct InterruptedTurn {
    pub partial: Option<String>, // the kept partial reply, if any text streamed
    pub tool: Option<ToolCall>,  // the cancelled tool, resolved Failed, if one ran
}
```

- Keeps any non-empty partial as a normal `Role::Assistant` history message
  (codex keeps partial output).
- A **running tool** is resolved as `ToolStatus::Failed` with output
  `INTERRUPT_TOOL_OUTPUT` (`"Interrupted by user"`) via the existing
  `end_tool` path — it was cancelled mid-run, and the record survives in the
  Ctrl+O view (codex: aborted tools "may have partially executed").
- Records the notice as a `Role::Error` message —
  `INTERRUPT_NOTICE` = `"Conversation interrupted - tell the model what to do
  differently."` (codex's wording minus its `/feedback` plug).
- Clears the live status **without** a `Done for Ns` summary (like
  `fail_stream`: the notice is the summary).
- `None` (recording nothing) when no turn is in flight.

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
2. `app.interrupt_turn()`, then commit like `StreamDone` does: reseat the
   viewport to its idle height first (the streaming strip is gone —
   invariant 3), then `insert_before` the partial (via `final_commit`, which
   respects the already-committed lines), the cancelled tool (collapsed,
   red), and the red notice, each with a blank spacer.
3. Reset `committed` / `turn_start` / `thinking_start`, then flush the front
   queued batch (`flush_next_queued`) onto the **new** channel.

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
(`llm::LlmBackend`) is different: during the pre-first-token pause the thread
is parked in a *blocking* network op — `reqwest`'s `send()` (waiting for the
response headers, which never polls the token) or the first SSE `read()`
(the SSE loop polls the token only *between* reads). Both wake only after the
per-operation timeout (`STREAM_OP_TIMEOUT`, 3 s; `src/llm/openai.rs`,
`src/llm/mod.rs::http_client`). So after Esc the thread cannot observe the
cancel until its in-flight read returns — the network responding (~1–2 s) or
the 3 s cap.

The old arm called `handle.join()` on that thread on the **single-threaded**
(`current_thread`) tokio loop, so the entire event loop — draw ticks, the
status animation, keystrokes — blocked for that whole window: the reported
"press Esc → the spinner freezes for 1–2 s, then unfreezes" bug. Reproduced
deterministically offline by `stream::StallAi` (a backend that ignores the
cancel for `INLINE_TUI_STALL_MS` ms, modelling the wedged read): with the old
`join`, the `Conversation interrupted` notice lands ~`STALL_MS` after Esc;
with `abandon_inflight` it lands within a frame. `smoke.sh` Phase 32 asserts
the prompt path.

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
  running); `interrupt_turn` keeps the partial + records the notice, resolves
  a running tool as Failed with `"Interrupted by user"`, records **no**
  summary, clears the status, stamps with the injected clock, and is a `None`
  no-op when idle.
- `ui`: `status_line` ends with the dim `esc to interrupt` hint in every
  phase.
- `stream`: `StallAi` (the test double for a wedged backend) ignores the
  cancel for its stall — a caller that `join()`s it pays the full stall — and
  streams a normal reply when left to run.
- `main.rs` (smoke, Phase 8): mid-stream Esc leaves the partial text and the
  `Conversation interrupted` notice on screen, clears the status line (no
  `tokens`), commits no `Done for`, and the app still completes a following
  turn normally.
- `main.rs` (smoke, Phase 32 — the interrupt-lag regression guard): with a
  backend stalled 3 s (`INLINE_TUI_STALL_MS`, ignoring the cancel), Esc still
  commits the `Conversation interrupted` notice **within a frame** (asserted
  `< 1.5 s`, well under the stall) — proving the loop detaches the thread
  rather than `join()`ing it. Measured live: ~3.0 s (old, frozen) → ~0.015 s
  (fixed), and the real OpenRouter backend interrupts in ~0.015 s too.
