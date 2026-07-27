# The `Agent` tool — Claude-Code-style subagents

The model can now **launch subagents**: autonomous side-conversations that run
their own agentic tool loop (`bash`/`read`/`write`/`edit`) over their own
context, report progress live in the TUI, and hand their final response back to
the main agent as the tool result. It is Claude Code's `Agent` (Task) tool,
adapted to this inline TUI — including the parts Claude Code doesn't have: the
user can **enter a subagent's own inline session and chat with it**.

```
❯ Call two agents to fetch weather and time in Manila and Warsaw

● I'll spawn two agents...

● 2 background agents launched (↓ to manage)
   ├ Fetch current weather and time in Warsaw
   └ Fetch current weather and time in Manila

( ●•·  ) Working… (4s · ↓ 120 tokens · esc to interrupt)

──────────────────────────────────────────────────────────────
❯
──────────────────────────────────────────────────────────────
  kimi-k3 medium · ~/repo · 1.8k/1M (0.2%)

  ● main
  ◯ general-purpose  Fetch current weather and time in… 48s · ↓ 16.5k tokens
  ◯ general-purpose  Fetch current weather and time in… 48s · ↓ 15.9k tokens
```

## The tool

Declared beside `bash`/`read`/`write`/`edit` (`llm::tools::agent_spec`), main
backend only — a subagent never gets the `agent` tool, so agents can't nest:

| param | | |
| --- | --- | --- |
| `description` | required | a short (3-5 word) task label — the tree rows / footer list show it |
| `prompt` | required | the full task for the agent to perform |
| `subagent_type` | optional | `general-purpose` (default, all tools) or `explore` (read-only: `bash`+`read`) |
| `run_in_background` | optional | **default `true`** — the call returns at once with the agent id; `false` blocks the turn until the agent finishes and returns its final response |

(The reference schema's `model` / `isolation` params are deliberately not
implemented — out of scope for this TUI.)

A subagent's conversation starts fresh: the same persona/environment system
prompt plus a subagent note (`prompts/subagent.md` — "your final message is
returned to the caller"), then the `prompt` as the first user message. It runs
`llm::agent::run_agent` with its own executor (no background registry — a
subagent's `run_in_background` bash arg errors recoverably) and its own cancel
token, on its own thread.

## Protocol

Two layers, mirroring background shells (`docs/background.md`):

- **The reply channel** carries the *group lifecycle* inside the parent turn:
  - `StreamEvent::AgentBatch { background, agents: Vec<AgentSpec> }` — the
    round's `agent` calls announced up front (id, description, type, prompt).
    The loop seeds the roster (`App::agent_started`) and the live group cell.
  - `StreamEvent::AgentGroupDone { group: AgentGroupResult }` — the group
    resolved (all foreground agents finished, or the background launch
    acknowledged, or Ctrl+B moved the rest to the background). Carries the
    final per-agent snapshots so the committed cell can never race the agent
    channel. The loop records `HistoryItem::AgentGroup` and commits the cell.
- **The agent channel** (`agents::AgentEvent`, a dedicated `select!` source —
  agents outlive turns) carries each subagent's own stream, tagged by id:
  `AgentEvent::Stream { id, event }` for every `StreamEvent` the subagent's
  `run_agent` emits (chunks, tool batches, usage, done/error). The loop folds
  them into the roster entry (`App::apply_agent_event` →
  `agents::AgentRun::apply`), which accumulates the agent's **own transcript**
  (`Vec<HistoryItem>`), live tool queue, token tally, tool-use count, and
  status — everything the footer list, the Ctrl+O cells, and the agent
  session view render.

`llm::agent::run_agent` gains one closure, `run_agents(&[ToolCallRequest]) ->
Vec<(call_id, output)>`: a round's calls are partitioned (`agent` vs the
rest); agent calls are handed to the closure (which launches them all
concurrently, emits the events above, waits for foreground completion —
polling the parent cancel and the registry's Ctrl+B latch — and returns each
call's tool result); the rest execute exactly as before; results append in the
original call order. The closure is a no-op fake in the unit tests and
`llm::backend`'s real launcher in production.

## The registry (`agents::AgentRegistry` — boundary, like `background`)

A cloneable handle shared by the loop and the backend: allocates
Claude-Code-style ids (`a` + 8 base36 chars), keeps each running subagent's
`CancelToken` + completion flag + final message list, and owns the per-agent
**pending-input queue** (the chat feature). `kill(id)` cancels the token and
marks the completion `killed` so the parent's wait loop resolves at once
(without waiting for the thread to notice); `kill_all()` sweeps on
`/clear`/quit. Completed foreground results are read off the shared slot.

Background completion notices ride the **existing**
`BackgroundRegistry::post_notice` board (`from_model: true`), so the in-flight
agent hears a finished subagent within the same turn and
`dispatch_after_turn`'s automatic follow-up turn covers idle completions with
zero new plumbing. The TUI cell is a new `HistoryItem::AgentNotice` —
`● Agent "{description}" finished · 35s` (green) / `was stopped by user` /
`failed` (red) — deferred to the same safe boundaries as shell notices
(`App::pending agent completions`, settled beside `settle_bg_completions`).

## App state

- `App::agents: Vec<agents::AgentRun>` — the roster. Fed by `AgentBatch`
  (create) and the agent channel (update). A finished agent **lingers** a few
  seconds with a coloured `◯` (green done / red stopped/failed), then the
  boundary sweeps it (`AGENT_LINGER`, timed in `main.rs` like the toast) —
  deferred while the user is inside that agent's session. A user `x` removes
  it immediately.
- `App::agent_group: Option<AgentGroupLive>` — the live group cell (ids +
  background flag), shown in the preview strip while the parent turn runs.
  Esc-interrupt / `fail_stream` resolve it locally (statuses → Interrupted)
  because the channel swap drops the backend's own `AgentGroupDone`.
- `App::agent_selection: Option<usize>` — the ↓ roster navigation: `0` is the
  `● main` row, `i+1` the i-th agent. ↓ from an idle composer steps onto the
  **shell indicator first** when one is lit-able (the existing
  `background_focus`), a second ↓ moves into the roster; with no shells ↓
  goes straight to the roster. ↑/↓ move, `Enter` views (main = leave the
  view / close), `x` stops the selected agent, Esc dismisses, any other key
  falls through after clearing (the `background_focus` contract). The footer
  line swaps to the hint (`↑/↓ to select · Enter to view` on main,
  `Enter to view · x to stop` on an agent) while the selection is active.
- `App::agent_view: Option<String>` — the **agent session view**: the whole
  inline screen shows that agent's own conversation (banner + its transcript,
  Purge-rebuilt like `/clear`), the input box's top rule carries the agent's
  description as a right-aligned label, and the composer **chats with the
  agent**: Enter sends the draft to the subagent — injected at its next round
  boundary while it runs (the `pending_notices` seam), or spawning a
  continuation turn over its stored message list when idle. Esc (empty
  composer) returns to the main session (Purge-rebuild of the main
  transcript, mid-stream partial included). While the view is up the main
  turn's commits are suppressed exactly like the Ctrl+O overlay (invariant
  4); the viewed agent's events commit incrementally through a dedicated
  `StreamRender`.

## Rendering (`ui`)

- **Live group cell** (`agent_group_lines`): blue `● Running {n} agents…
  (ctrl+o to expand)` over the tree —
  `   ├ {description} · {n} tool uses · {tokens} tokens` with a
  `   │ ⎿  {activity}` status row per agent (`Initializing…` → the running
  tool's `name(args)` → `Done`) — plus the delayed
  `(ctrl+b to run in background)` hint (foreground only). The strip's
  `preview_rows`/`preview_lines` size and draw it like the tool queue.
- **Committed cells**: `● {n} background agents launched (↓ to manage)` over
  description-only tree rows (green); `● {n} agents finished (ctrl+o to
  expand)` over the counted tree rows with `⎿ Done` / `⎿ Interrupted` /
  `⎿ Failed` per agent (green when all done, red otherwise).
- **Ctrl+O**: each `AgentGroup` entry expands as its own cell —
  `● Agent({description})` / `⎿ Prompt:` (indented block) / the nested tool
  headers the agent ran (`Bash(curl …)`) / `⎿ Response:` (the final text) /
  `⎿ Done ({n} tool uses · {tokens} tokens · {s}s)` or `⎿ Interrupted`. The
  **live tail** walks `App::agent_group` + the roster the same way (activity
  `Running…`), and the `TranscriptSig` fingerprints the roster generation so
  a streaming agent invalidates the cache.
- **Footer roster** (`agent_list_lines`): while agents exist, the live region
  gains rows below the footer — a blank spacer, `  ● main`, then per agent
  `  ◯ {type}  {description trimmed}… {elapsed}s · ↓ {tokens} tokens` (dim
  type, live counters). The selection paints `❯ ` on the active row; a
  finished agent's `◯` turns green/red for the linger window. The rows are a
  fifth `live_layout` area, so `live_height`, the cursor seat, and the
  overlays are untouched.

## Invariant notes

- Subagent events ride their own channel and **never** commit to scrollback
  directly — the roster is state; cells commit only at the group's reply-
  channel boundaries (or, in the agent session view, through that view's own
  render). Invariants 1-4 hold unchanged.
- The parent's tool results replay on later turns via a `context.rs` arm:
  `HistoryItem::AgentGroup` derives one assistant `tool_calls` entry (one
  `agent` call per entry, arguments reconstructed) + one `tool` result per
  agent; `AgentNotice` derives the bracketed user-role note carrying the
  final response.
- `session.rs` round-trips both new items (`agent_group` / `agent_notice`
  records); old builds skip them (the forward-compatibility contract). The
  roster itself is ephemeral, like background shells.
- `/clear` and quit `kill_all()` agents; an Esc interrupt kills only the
  in-flight **foreground** group (background agents keep running, like
  background shells).
