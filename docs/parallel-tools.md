# Parallel tool calls — the visible batch + `⎿ Waiting…`

When the model requests **several tool calls in one assistant turn** (e.g. three
`Bash` commands at once), the TUI now shows **all of them up front**: the one
executing renders live (a blinking grey bullet over `⎿ Running…`, then its
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

The running call **tails its live output** (the last lines + a `+N lines (Ns ·
timeout …)` footer, its own clock beside the timeout it runs under) as it
streams — see `docs/tool-streaming.md`; the `⎿ Waiting…` siblings are unchanged.

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
- On interrupt / backend error **every** call in the queue resolves as `Failed`
  (`Interrupted by user` / `Interrupted by a backend error`) — the running (or
  permission-waiting) front call and each `Waiting` sibling behind it, in the
  batch's order (`App::fail_live_queue`) — and each one is recorded and
  committed above the notice. A call that never started is still a call the
  model made: its cell stays on screen, and its record keeps it in the next
  request's context (see *Interrupting a batch* below). The
  undo-the-submission fast path keys off an **empty** queue
  (`tool_queue.is_empty()`), the old `current_tool.is_none()`.

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

## Interrupting a batch

Esc — or a backend error — mid-batch ends every call the model asked for at
once, and every one of them keeps its cell, the running call and the
`⎿ Waiting…` siblings alike:

```
● Bash(ping google.com -c 10)
  ⎿  Interrupted by user

● Bash(ls -la)
  ⎿  Interrupted by user

● Conversation interrupted - tell the model what to do differently.
```

Three pieces make that hold:

- **The record.** `App::fail_live_queue` resolves the running (or
  permission-waiting) front call and each `Waiting` sibling, in the batch's
  order, through the same `end_tool` path a `ToolEnd` takes. The records keep
  their batch id, provider call id and position, so the next request's derived
  context (`context::context_messages`) replays the whole round the way the
  wire carried it — one assistant message holding every call, each answered
  `Interrupted by user` (Claude Code's own answer for a tool use an abort left
  without a result), then the `[error]` notice. The siblings used to be
  dropped: the model's next turn saw the first call and nothing of the rest,
  so it no longer knew it had asked for them.
- **The arguments.** A call that never reached its own `ToolStart` knows only
  the header summary the batch announcement carries. The round announcement
  ahead of it (`StreamEvent::RoundCalls`) carries each call's verbatim
  arguments (`RoundCall::arguments`), stamped onto its `Waiting` cell, so the
  replay is the call the model made — a `bash` call's `timeout`, a `write`'s
  `content` — rather than one rebuilt from the summary. A call that does start
  takes its `ToolStart`'s arguments instead, since a `PreToolUse` hook may
  have rewritten what runs (`docs/hooks.md`). The offline dummy announces no
  round, so its interrupted siblings replay through the summary
  reconstruction, as every pre-field record does.
- **The commit.** The interrupt records its red notice *behind* the cells, so
  the history no longer ends at a cell and `ui::tool_commit_lines` — built for
  the one call that just resolved — finds nothing to commit. That used to
  leave the notice alone on screen, with no cell at all, until something
  rebuilt the screen from history (closing a permission prompt does, which is
  why the first cell sometimes appeared). `ui::resolved_tools_commit_lines`
  commits the last N tool records instead, each exactly as its own resolution
  would have — the first flushing any parallel MCP run it was holding
  (`docs/mcp.md`) — which is the rows a rebuild paints from the same history.

A subagent's batch settles the same way (`AgentRun::resolve_tools`, on the
roster's `x`, an Esc that takes its group down, or its own backend error):
every call lands on its transcript, its session view commits them all, and its
stored message list — what a chat continuation resumes from — answers a call
the cancel left unexecuted `Interrupted by user` as well, where it used to say
`[not executed]`, so the continuation reads what the view shows.

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
- An interrupt keeps the calls it can see, which is the live queue. A round's
  calls that were never announced still leave no record, so the next request
  does not carry them: the ordinary calls behind a foreground agent group (the
  batch is announced only once the group finishes, `docs/agent-tool.md`), and
  a task call the sequential loop had not reached yet (task calls render no
  cell and are never announced, `docs/task-tools.md`).
