# Parallel tool calls — the visible batch + `⎿ Waiting…`

When the model requests **several tool calls in one assistant turn** (e.g. three
`Bash` commands at once), the TUI now shows **all of them up front**: the one
executing renders live (blue `⎿ Running…`, then its output), and the ones not yet
started render as dim `⎿ Waiting…` cells — Claude-Code's parallel-tool look:

```
● Bash(ping -c 20 google.com)
  ⎿ 64 bytes from … icmp_seq=8 ttl=117 time=769 ms
    … +8 lines

● Bash(ping -c 20 facebook.com)
  ⎿ Waiting…

● Bash(ping -c 20 x.com)
  ⎿ Waiting…
```

This lifts the old limitation (`docs/tools.md`, before this change: *"Parallel
tool calls … are executed sequentially (the TUI shows one running tool at a
time)"*). Execution is **still sequential** — the calls run one after another,
preserving the existing timeout / cancel / safety machinery — but the whole batch
is now **visible**, and each call commits to scrollback the moment it finishes.
"Waiting…" is exactly the state of a queued-but-not-started call, so sequential
execution and this display are the same thing the user sees in Claude Code.

## Why display-the-batch, not run-concurrently

The reference (the user's ping example) shows the siblings as `Waiting…` *while*
the first one runs — i.e. they are **not** producing output concurrently. So the
right model is: announce the batch, run in order, show the rest waiting. This:

- keeps `llm::exec`'s single-child timeout + `kill_process_group` + cancel exactly
  as-is (no thread-pool, no interleaved stdout, no multi-child reaping);
- reuses the app's existing **progressive commit** pipeline (a finished item flows
  into real scrollback while the live region shows what's left);
- is the foundation a future *concurrent* executor could build on (the
  Waiting/Running/Ok/Failed cell states already exist).

## The mechanism

### Protocol (`stream.rs`)

A new event announces the batch **before** the first `ToolStart`:

```rust
/// The model requested a batch of tool calls this round, announced up front so
/// the UI can show every call — the ones not yet executing as `⎿ Waiting…`.
/// Each tuple is the `(name, args)` the `● name(args)` header shows. Sequential
/// execution then transitions them one at a time via ToolStart/ToolEnd.
ToolBatch(Vec<(String, String)>),
```

The existing `ToolStart{name,args}` / `ToolEnd{output,ok,truncated}` pair is
unchanged. A backend that never batches (the `!` shell) simply never sends
`ToolBatch` — a lone `ToolStart` still works.

### App state (`app.rs`)

`current_tool: Option<ToolCall>` becomes a **queue**: `tool_queue:
VecDeque<ToolCall>`. The front is the running (or about-to-run) call; the rest
are `ToolStatus::Waiting`.

- `ToolStatus` gains a `Waiting` variant (dim, rendered `⎿ Waiting…`).
- `start_tool_batch(&[(name,args)])` fills the queue with `Waiting` calls.
- `start_tool(name,args)` flips the **front** `Waiting` → `Running`; with an empty
  queue (the dummy's lone tools, the `!` shell) it pushes a fresh `Running` call —
  so the single-tool path is byte-for-byte the old behaviour.
- `end_tool(output,ok)` pops the **front**, finalizes it (Ok/Failed), records it in
  `history`, and returns it to commit — the next call becomes the front.
- `current_tool()` returns the front (unchanged call sites keep working).
- `tool_queue()` exposes the whole batch for rendering.
- On interrupt / backend error the **front** running call resolves as `Failed`
  (`Interrupted by user` / `Interrupted by a backend error`) and any remaining
  `Waiting` siblings are **dropped** — they never ran, so no cell is recorded
  (they only ever lived in the live region). The undo-the-submission fast path
  keys off an **empty** queue (`tool_queue.is_empty()`), the old
  `current_tool.is_none()`.

### Backend (`llm::agent::run_agent`)

The agentic loop already has every call up front (`RoundOutcome::ToolCalls{calls}`).
It now emits one `ToolBatch` (built from `calls`) **before** the existing
`for call in calls { ToolStart; execute; ToolEnd }` loop. Nothing else in the loop
changes; execution stays sequential and cancellation still checks between calls.

### Rendering (`ui.rs`)

The streaming strip's **preview** slot renders the *whole* live queue (each call's
collapsed cell, blank-line-separated), not just one call. `preview_rows` /
`preview_lines` size and draw from the same queue walk so the box + cursor
geometry stay exact (guarded by the existing `debug_assert`). A `Waiting` cell is
`● name(args)` + a dim `⎿ Waiting…` row; the Ctrl+O transcript's live tail walks
the whole queue too, so a waiting call is never hidden. The `TranscriptCache`
signature keys off `(queue length, front status)` so a Waiting→Running flip or a
call committing invalidates the cache.

The single-`!`-shell preview (its `⎿ Running… (Ns)` elapsed row) is unchanged: the
shell is never batched (queue length 1, `shell` flag).

## Demo (the dummy backend)

`stream::turn_events` scripts a **parallel `Bash` batch** (three ping-style
commands, mixed ok/fail for colour) followed by a lone `Read` (the file-cell demo,
exercising the no-batch path), so `cargo run` and `scripts/smoke.sh` show the
`Waiting…` states offline without a real provider. A real backend gets the same
display for free via the `ToolBatch` the agent loop emits.

## Limitations

- Execution is sequential (see above) — a genuinely concurrent executor is future
  work; the cell states are ready for it.
- A very large batch grows the live region upward; it is clamped to the terminal
  height like the rest of the live region (the box then scrolls internally). No
  per-batch cap / "+N more" collapse yet.
