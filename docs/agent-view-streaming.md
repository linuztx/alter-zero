# The agent session view streams like the main one

Date: 2026-08-26

## The report

> the streaming inside the subagent tui — when it tries to stream it
> disappears. I'm not sure if it's because it's streaming a table.

It is. And `/copy` inside that view copies the wrong conversation.

## What was actually happening

The agent session view (`docs/agent-tool.md`) has its own streaming strip,
built by `ui::agent::agent_view_preview_lines`. For a streaming reply it did:

```rust
message_lines(Role::Assistant, text, width).pop()   // the LAST row, batch-rendered
```

That is **not** the frontier the agent's commits leave behind.
`tui::agent::commit_agent_view_event`'s `Chunk` arm commits through
`Session::agent_render` — a `ui::StreamRender`, the same incremental renderer
the main session uses — and `StreamRender::commit` withholds **by source
line**: an open table, a fenced code line, a partial fence / heading / list
marker, an unclosed inline marker, a forming URL (CLAUDE.md invariant 2).

The contract those two halves are supposed to keep is
`committed ++ preview == the whole reply at every instant`
(`StreamRender::preview`'s doc). The main strip keeps it because
`Session::stream_preview_lines` previews through the very render that commits.
The agent view broke it: `commit` withheld a whole block, and the strip showed
one row of it. Everything else was on screen **nowhere**.

Captured live against `anthropic/claude-haiku-4.5` (100×30 tmux, an 8-row
markdown table asked of a subagent from inside its session view):

```
                        agent view (before)                main view
  ┌───────────────────────────────────────────┬──────────────────────────────┐
  │  └────────────┴──────┴──────────┴───────┘ │ ┌──────────┬──────┬────────┐ │
  │                                           │ │ Language │ Year │ Typing │ │
  │  (●•·   ) Working… (2s · ↓ 2.2k tokens)   │ ├──────────┼──────┼────────┤ │
  │                                           │ │ Python   │ 1991 │ Dynamic│ │
  │                                           │ │ Java…                      │
  └───────────────────────────────────────────┴──────────────────────────────┘
```

One row — the table's *bottom border* — where the main view streams the whole
forming grid with its columns re-fitting as wider cells arrive
(`docs/table-streaming.md`). To a user that is "the streaming disappeared".

Three further divergences in the same view, all the same root cause (a second,
thinner copy of the main strip's logic):

1. A running `bash` call showed the plain `⎿ Running…` peek instead of tailing
   its streamed output (`running_command_lines`, `docs/tool-streaming.md`).
2. A **thinking phase was invisible**: `AgentRun::apply` listed
   `ThinkingStart`/`ThinkingEnd` among its no-ops and only *counted*
   `ThinkingChunk`, so a reasoning subagent showed a bare spinner where the
   main view shows the live `● Thinking…` block and then the settled
   `Thought for 1s · 206 tokens (ctrl+o to expand)` cell
   (`docs/thinking-stream.md`).
3. The strip's status line never said `Thinking for Ns`
   (`agent_view_status` hard-coded `thinking: None`).

And `/copy`: `App::last_assistant_text` walks `App::history` unconditionally,
so inside an agent session view it copied the **main** conversation's last
assistant message — verified live: the screen showed the agent's table, the
clipboard got the lead's `Background agent "table demo" completed…` summary.

## The fixes

### 1. One frontier, both views

`Session::stream_preview_lines` becomes view-aware. With an agent session view
open it previews the **viewed agent's** buffer through `self.agent_render` (the
render its commits use); otherwise the main buffer through `self.render`, as
before. Either way it injects the row count with `App::set_stream_preview_rows`,
and `ui::preview_rows`' agent branch reports that count for a streaming agent
exactly as the main branch does — so the reserved rows and the painted rows
still agree by construction (the strip's `debug_assert`).

`agent_view_preview_lines` takes the built preview
(`stream_preview: Option<&[Line]>`) and uses it; the old batch-render stays as
the `None` fallback, which is what the pure unit tests and any caller without a
render use — the *same* shape `preview_lines` already had for the main branch.

`repaint_agent_view` computes the preview **before** `live_region_height()`
(the count feeds the height) and passes it to the render closure; it passed
`None`, so every purge rebuild of the view — a resize, an overlay return, a
flow exit — collapsed a multi-row preview to one row for that frame.

### 2. One renderer for a live tool cell

`ui::live::preview_tool_lines`' per-call body moves into a shared
`live_call_lines(tool, elapsed, pulse, width)`, and `agent_view_preview_lines`
calls it. A subagent's running `bash` now tails its output with the same
`+N lines (Ns)` footer, and its `!`-shell/plain cases stay identical. The
elapsed is the agent's own `run.runtime` (what `agent_view_status` already
shows), not the main turn's.

The delayed `(ctrl+b to run in background)` hint is deliberately **not** shared:
`App::command_elapsed` is the *main* turn's command clock, and Ctrl+B in the
agent view moves the whole group, not the call. Advertising it off another
turn's clock would be a lie.

### 3. The agent thinks out loud

`AgentRun` gains the main session's three pieces, in the same split:

- `reasoning: Option<String>` — the open phase's buffer. Opened by
  `AgentRun::begin_reasoning` (via `App::begin_agent_reasoning`), which the
  boundary calls **only when `/settings` **Hide thinking** is off** — so with
  the display off no buffer is ever opened and `ThinkingChunk` only counts,
  exactly as before (the `ALTER_ZERO_SHOW_THINKING` posture,
  `docs/thinking-stream.md`).
- `round_reasoning: Vec<usize>` — where this round's cells landed, so the
  agent's own `Usage` frame snaps their tokenizer estimates to the provider's
  `reasoning_tokens`. The distribute-by-weight helper is now the shared pure
  `app::reasoning::snap_reasoning_tokens`, called by `App` and `AgentRun`
  alike rather than copied.
- `thinking: Option<Duration>` — the open phase's elapsed, boundary-injected
  per frame from `Session::agent_thinking_clocks` (the `set_status_times`
  pattern), so `agent_view_status` shows `Thinking for Ns` and no clock
  reaches the pure core.

`Session::settle_agent_reasoning` is the one settle helper — the sibling of
`Session::settle_reasoning` — called from all three places a phase can end:
its own `ThinkingEnd`, the agent's `Error`, and a user stop. It records the
`HistoryItem::Reasoning` on the **agent's** transcript and, when that agent's
session view is up, commits the collapsed cell (reseat before you commit).
`ThinkingStart` flushes the agent's streamed segment first — flush before you
interleave — so the header sits under the same blank spacer the settled cell
will take.

Everything downstream is free: `ui::conversation_lines` (the view's rebuild)
and `ui::transcript_lines` (its Ctrl+O) already render
`HistoryItem::Reasoning`, and `context::context_messages` already skips it.

### 3b. The rest of the strip's parity

Two smaller divergences went with it, both "the agent view had its own thinner
copy":

- **`retrying {n}/{max}`.** `AgentRun::apply` dropped `StreamEvent::Retrying`
  on the floor and `agent_view_status` hard-coded `retry: None`, so a subagent
  reconnecting showed a bare spinner while the main turn says what it is doing
  (`docs/llm.md`). `AgentRun::retry` now carries it, cleared by the next
  streamed chunk exactly as `App::push_chunk` clears the main one.
- **The agent's Ctrl+O tail.** `agent_transcript_lines` rendered settled
  `HistoryItem::Reasoning` cells but not the *open* phase; the main tail shows
  it whole (`reasoning_live_full_lines` — the pager has no row budget, unlike
  the strip's windowed block). It does now.

### 4. `/copy` copies what the screen shows

`App::last_assistant_text` reads the **viewed agent's** transcript when an
agent session view is open, else `App::history` — the `viewed_agent()` branch
`ui::context_lines`, `ui::agent_transcript_lines` and the Ctrl+D classifier
page already take. The clipboard I/O, the toast, and the empty case are
untouched.

## Testing

- `ui` (unit): the viewed agent's strip previews the injected frontier and
  `preview_rows` reserves exactly it; `commit ++ preview` reconstructs a
  forming table's whole block; a running agent `bash` cell tails its output;
  the live `● Thinking…` block previews and `agent_view_status` carries the
  phase's elapsed.
- `agents` (unit): `apply` buffers `ThinkingChunk` only while a phase is open,
  opening one flushes the text before it, the settle records
  `HistoryItem::Reasoning` **ahead of** the partial reply on both an `Error`
  and a `StreamDone`, the round's `Usage` snaps the count, an interrupt keeps
  a partial thought, and a `Retrying` shows until the next chunk clears it.
- `stream` (unit): the `agent-stream` entry has an example prompt that selects
  *it* (the registry's reachability test) and is skipped when no registry is
  attached, falling through to the `table` demo it outranks.
- `app` (unit): `last_assistant_text` follows the viewed agent, and `/copy`
  from inside the view returns that text.
- `scripts/smoke.sh` **Phase 95**: drives the real binary. The dummy gains an
  `agent-stream` scenario (`src/stream/dummy/agent.rs`, cue "subagent") that
  launches one background subagent and plays *its* round on the subagent
  channel from that agent's own thread — a thinking phase, then the table —
  which is the first time the agent session view has been drivable with no
  network. The phase walks the roster into that session and asserts the live
  `● Thinking…` block shows, the forming **grid** (not just its closing
  border) is in the strip mid-stream, the table commits exactly once, and the
  settled `Thought for …` cell lands on the agent's transcript.
