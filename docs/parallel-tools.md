# Parallel tool calls — the visible batch + `⎿ Waiting…`

When the model requests **several tool calls in one assistant turn** (e.g. three
`Bash` commands at once), the TUI now shows **all of them up front**: the one
executing renders live (a breathing-grey bullet over `⎿ Running…`, then its
output — `docs/tool-pulse.md`), and the ones not yet
started render as dim `⎿ Waiting…` cells — Claude-Code's parallel-tool look:

```
● Bash(ping -c 20 google.com)
  ⎿  64 bytes from … icmp_seq=6 ttl=117 time=68.3 ms
     64 bytes from … icmp_seq=7 ttl=117 time=247 ms
     64 bytes from … icmp_seq=8 ttl=117 time=332 ms
     64 bytes from … icmp_seq=9 ttl=117 time=144 ms
     +5 lines (9s)

● Bash(ping -c 20 facebook.com)
  ⎿  Waiting…

● Bash(ping -c 20 x.com)
  ⎿  Waiting…
```

The running call **tails its live output** (the last lines + a `+N lines (Ns)`
footer) as it streams — see `docs/tool-streaming.md`; the `⎿ Waiting…` siblings
are unchanged.

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

### Protocol (`stream/event.rs`)

A new event announces the batch **before** the first `ToolStart`, carrying a
small named struct per call (`ToolCallSummary { name, args }` — the *same* two
strings the call's own `ToolStart` carries, so a `⎿ Waiting…` header matches the
header it shows once running):

```rust
pub struct ToolCallSummary { pub name: String, pub args: String }

/// The model requested a batch of tool calls this round, announced up front so
/// the UI can show every call — the ones not yet executing as `⎿ Waiting…`.
/// Sequential execution then transitions them one at a time via ToolStart/ToolEnd.
ToolBatch(Vec<ToolCallSummary>),
```

The existing `ToolStart{name,args}` / `ToolEnd{output,ok,truncated}` pair is
unchanged. A backend that never batches (the `!` shell) simply never sends
`ToolBatch` — a lone `ToolStart` still works.

### App state (`app/tools.rs`)

`current_tool: Option<ToolCall>` becomes a **queue**: `tool_queue:
VecDeque<ToolCall>`. The front is the running (or about-to-run) call; the rest
are `ToolStatus::Waiting`.

- `ToolStatus` gains a `Waiting` variant (dim, rendered `⎿ Waiting…`).
- `start_tool_batch(&[(name,args)])` fills the queue with `Waiting` calls,
  each stamped with the batch's own id (`ToolCall::batch`, a plain counter).
  Nothing in *this* feature reads it; it is what lets a later renderer tell
  one round's parallel calls from two rounds' sequential ones — the MCP run
  collapse (`docs/mcp.md`). A lone call's is `None`.
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

### Rendering (`ui/tool.rs`, `ui/live.rs`)

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

`stream::turn_events` is **prompt-gated** so the demo is vivid without bloating
every turn's scrollback (which would push content off the fixed-size panes the
smoke suite asserts against):

- a prompt mentioning **"parallel"** runs the vivid three-call `Bash(ping …)`
  batch — the user's example (two green, one red) — so `cargo run` with a
  "parallel" prompt and `scripts/smoke.sh`'s dedicated phase show all three cells,
  the running one live over two dim `⎿ Waiting…` siblings;
- any **other** prompt runs a compact two-call `Read`+`Bash` batch (the `Bash`
  waits while the `Read` runs) — the feature is still visible every turn, at the
  **baseline scrollback footprint**, so unrelated smoke phases keep their sizing.

A real backend gets the same display for free via the `ToolBatch` the agent loop
emits, rendering however many parallel calls the model actually requests.

## Inside a subagent

A subagent runs the **same** `llm::agent::run_agent`, so its rounds announce
their batches the same way, and the forwarder passes every event through to the
agent channel verbatim. `AgentRun::apply`'s `ToolBatch` arm is `App`'s twin —
one `ToolStatus::Waiting` cell per announced call, replacing the queue (the
announcement is the round's whole set) — so a subagent's parallel batch shows
exactly as the main turn's does on all four surfaces of that agent's session
view (`docs/agent-tool.md`): the strip previews the whole queue through the
shared `ui::live::live_call_lines`, Ctrl+O expands every cell, each resolution
commits to the agent's own scrollback, and a **permission prompt raised there
shows the asked-about call over every `⎿ Waiting…` sibling** through the same
`queue_chunks` walk the main branch uses (`docs/permissions.md`,
`docs/agent-view-streaming.md`).

Two deliberate differences remain, both about aggregation rather than display:
a subagent's calls carry no batch id, so a run of MCP cells is never collapsed
into one `Called {server} N times` line (`docs/mcp.md`), and the parent's own
`● Running {n} agents…` tree shows one sticky activity row per agent rather
than a cell per call — the tree is a roster, not a transcript.

## Limitations

- Execution is sequential (see above) — a genuinely concurrent executor is future
  work; the cell states are ready for it.
- A very large batch grows the live region upward; it is clamped to the terminal
  height like the rest of the live region (the box then scrolls internally). No
  per-batch cap / "+N more" collapse yet.
