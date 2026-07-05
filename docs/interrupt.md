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

1. `cancel.cancel()` + `join` the backend thread (it polls the token every
   ≤20 ms, so this is prompt) — same teardown the quit path uses.
2. **Drain the reply channel** of anything the backend sent before it saw the
   cancel. A stale `ToolStart` processed after the interrupt would wedge a
   phantom "running" tool whose `ToolEnd` never comes; the thread is joined,
   so after the drain the channel stays empty.
3. `app.interrupt_turn()`, then commit like `StreamDone` does: reseat the
   viewport to its idle height first (the streaming strip is gone —
   invariant 3), then `insert_before` the partial (via `final_commit`, which
   respects the already-committed lines), the cancelled tool (collapsed,
   red), and the red notice, each with a blank spacer.
4. Reset `committed` / `turn_start` / `thinking_start`.

`Action::Interrupt` can only originate in the conversation view (overlay Esc
returns instead), so the commits never touch the alternate screen.

**`/clear` mid-turn reuses steps 1–2** (cancel + reap + drain — the same
"nothing stale can arrive afterwards" guarantee) but skips the commits
entirely: `App::clear_conversation` wipes history, the streaming buffer, the
running tool, the status, *and the queued backlog*, recording no partial, no
notice, and no summary — the user asked for a blank slate, not a finished
turn. See `docs/design.md` (the `/clear` paragraph) and `smoke.sh` Phase 16.

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
- `main.rs` (smoke, Phase 8): mid-stream Esc leaves the partial text and the
  `Conversation interrupted` notice on screen, clears the status line (no
  `tokens`), commits no `Done for`, and the app still completes a following
  turn normally.
