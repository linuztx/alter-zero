# The agent session view streams like the main one

Date: 2026-08-26

## The report

> the streaming inside the subagent tui — when it tries to stream it
> disappears. I'm not sure if it's because it's streaming a table.

It is. And, from a second report:

> when the agent uses parallel tool calls the results are not displayed —
> you need to refresh the TUI. It successfully created the files but it does
> not update the inline TUI.

Two bugs, one shape: **the agent session view keeps its own copy of what the
main view does, and the copy fell behind.** (`/copy` inside that view also
copies the wrong conversation.)

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

### 3c. The view commits what the fold *recorded*, not what event arrived

A second report, same shape: a subagent creates files and the cells never
appear — until you resize the terminal, and then all of them do at once.

`commit_agent_view_event` listed the events that resolve a call:

```rust
StreamEvent::ToolEnd { .. } | StreamEvent::ToolRejected { .. } | StreamEvent::ToolBackgrounded { .. }
```

`157e032` ("write/edit: record the model's arguments, hand it a one-line
result") moved `write`/`edit` onto the **two-text split** —
`ToolOutcome::context` → `StreamEvent::ToolAnswered`, so the numbered body
stays on the cell while the model reads a one-line ack. It updated
`src/tui/stream.rs`, the main view's commit path. It did not touch
`src/tui/agent.rs`. So from that commit on, a subagent's `write`/`edit` (and
a loaded `skill`) landed on its transcript and stopped there; a resize
purge-rebuilt the view from that same history and every missing cell appeared
together. A parallel batch of writes made it unmissable — which is how it was
reported.

The fix is not "add `ToolAnswered` to the list". A hand-kept list of events is
what drifted, and this view had already fallen behind the main one once (§1).
`Session::commit_agent_tail` keys on **what `AgentRun::apply` appended to the
transcript** instead: the boundary snapshots `run.history.len()` before the
fold, and afterwards commits the newly-recorded last item — a `Tool` through
`ui::tool_commit_lines` (which still holds an MCP run's members until the run
ends), a `Summary` through `ui::summary_lines`. That is the same question a
rebuild answers, which is exactly why the two now agree; a future resolution
event needs no change here at all.

Everything else that reaches the transcript has its own committer and must not
land twice — a flushed text segment belongs to `agent_render`, a settled
thought to `settle_agent_reasoning`, a hook note is invisible inline — so they
fall through. And the red error notice is the one thing a failure does *not*
record (the run keeps only its `error` field), so it still commits explicitly,
after the cell the failure resolved: before this change the `Error` arm
committed the notice and silently dropped that cell.

The same helper closes the **local** settles, which carry no event at all: the
roster's `x` resolves the agent's running call through `AgentRun::interrupt`,
and its `⎿ Interrupted by user` cell had the identical problem.

### 4. The prompt above the view asks about *this* conversation

A third report, the same shape once more:

> Inside the subagent TUI it shows
>
> ```
> ● Agent(Run ls -la via subagent)
>   ⎿  Working…
> ```
>
> It should show `● Bash(ls -la)` / `⎿ Waiting…` — the same as the main agent
> TUI. And a subagent's parallel tool calls should show every waiting tool.

A permission prompt replaces the whole live region, so inside an agent session
view the *only* thing left of that agent's stream is the prompt's own context
cells — and `ui::permission_view::context_chunks` was the last surface in this
view that had never been made view-aware. It read `App::agent_group()` and
`App::tool_queue()`: the **lead's** live cell and the **main** turn's queue,
drawn on a screen showing a different conversation. With a lone foreground
agent that is the `● Agent({description})` / `⎿ Working…` cell above; with a
background one (whose group has already resolved) it is *nothing at all*, so
the prompt opened bare where the agent's own `● Bash(ls -la)` / `⎿ Waiting…`
belonged. Both are the same bug.

The context now comes from the viewed agent's own `tool_queue`, through the
same `tool_lines` walk the main branch uses (`queue_chunks`, shared by both) —
so a subagent's parallel batch shows the asked-about call over every
`⎿ Waiting…` sibling, blank-separated, exactly as the main view shows the main
turn's. `context_is_stable` (the flow eligibility, `docs/view-flow.md`) reads
the same queue.

Telling *which* conversation asked needs an id, not a type: two agents can
share `general-purpose`, and the request's `agent` field is only the type the
title names. `PermissionRequest` gains `agent_id`, stamped by
`tui::agent::Session::on_agent_event` — the boundary that routes the event is
the only place that knows it. A request whose id is not the open view's (the
main turn's own call, a sibling agent's) raised its cells on another screen,
so the prompt opens with no context rather than borrowing this agent's
unrelated batch.

### 5. The flush points are the fold's answer, not a list

`on_agent_event` finalises the agent's streamed text — `agent_render.finish`,
the spacer, `reset` — before the fold consumes the buffer. Which events those
are was a **hand-kept list**, the exact thing §3 had just retired for the
commit arm, and it had already drifted: `StreamEvent::HookNote` flushes the
buffer in `AgentRun::apply` (a `SubagentStop` block's continuation feedback,
`docs/hooks.md`) and was missing here. So the reply's withheld tail reached the
agent's transcript and never its screen, *and* `agent_render` kept a prefix of
a buffer that no longer existed — which the continuation's next `Chunk` then
committed against.

The list is now `AgentRun::flushes_segment`, next to the fold that defines it,
and `agents::tests::every_flush_point_is_declared` walks **every** event
variant asserting the predicate and the fold agree about the buffer. A new
variant cannot slip past. (`ThinkingStart` stays separate: it flushes too, but
only when the display is on, and that gate is the boundary's.)

The **local** settle needed the same treatment. `AgentRun::interrupt` finalises
three things — the open thought, the run of streamed text, the running call —
with no event to carry any of them, and the boundary ran only
`commit_agent_tail`, which commits the recorded tail and lets the other items
fall through to *their* committers. Those committers had not run. So a settled
agent's thought and its partial reply's withheld tail were dropped from the
view's scrollback, and `agent_render` stayed holding a prefix of a buffer
`flush_segment` had already taken — which the next chat continuation's `Chunk`
then committed against.

**Three doors reach that settle**, and they all go through
`Session::begin_local_agent_settle` now (`settle_agent_reasoning`, then the
shared `flush_agent_segment`, then the transcript length its
`commit_agent_tail` needs), after which `interrupt`'s own settles are no-ops:

1. the roster's `x` — `Session::stop_agent`;
2. a backend `Error`, whose `App::fail_stream` calls
   `App::resolve_live_agent_group` and interrupts every not-yet-final member
   of the round's live **foreground** group;
3. the Esc interrupt, whose `App::interrupt_turn` does the same.

(2) and (3) share `Session::begin_turn_agent_settle` /
`finish_turn_agent_settle`, which do the work only when the viewed agent is
actually a member of that group **and a turn is in flight** — both app methods
return early otherwise, and a pre-fold flush with no settle behind it would
reset the render while the buffer still held its text, re-committing the whole
reply on the next chunk.

### 6. Three more places the fold had drifted from `App`'s

The subagent's `tool_queue` is a second implementation of `App`'s batch
machinery (`docs/parallel-tools.md`), and the copy had fallen behind in three
small ways, each fixed with the main path's own rule and a unit test:

- **A batch announcement is the round's whole queue.** `App::start_tool_batch`
  *replaces* `tool_queue`; the fold appended, so anything left from an earlier
  round would sit at the front and take this round's resolutions, putting
  every cell off by one.
- **Every resolution records `truncated`.** The main boundary flags it on
  `ToolEnd`, `ToolRejected` *and* `ToolAnswered`; the fold honoured only the
  first, so a subagent's capped `write`/`skill` result silently claimed to be
  whole in Ctrl+O (no dim `…` marker).
- **A backgrounded call charges its acknowledgement.** `resolve_front_tool`
  folds every resolution's model-facing text into the `↑` tally; the fold's
  `ToolBackgrounded` arm charged nothing.
- **A group's resolution settles its members' live calls.** A foreground
  member whose own terminal event never arrived — a killed loop returns
  without one, the offline dummy scripts none — was settled by
  `App::finish_agent_group` with a bare `agent.status = …` write, so a
  *finished* agent went on owning live cells: its session view previewed a
  `⎿ Running…` that could never resolve. It settles through the shared
  `AgentRun::settle_from_group` now (partial thought, `resolve_tools`,
  status), the way every other settle does.

### 7. `/copy` copies what the screen shows

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
- `ui` (unit): inside an agent session view the prompt's context is that
  agent's own call (`● Bash(ls -la)` / `⎿ Waiting…`, never the lead's
  `Agent(` cell), a parallel batch shows every waiting sibling
  blank-separated, and a request from another conversation keeps no context
  at all — while the **main** view's own prompt still keeps the lead's tree,
  the other half of the same rule. An overflowing page keeps the agent's
  static cells and drops its *running* one, so `context_is_stable`'s new
  branch is covered where it matters (`docs/view-flow.md`). The view's
  *strip* previews the same batch, and `preview_rows` reserves exactly the
  rows it paints.
- `agents` (unit): **every** resolution — `ToolEnd`, `ToolAnswered`,
  `ToolRejected`, `ToolBackgrounded` — leaves its cell as the transcript's
  last item, and a backend `Error` leaves the call it killed there (the
  invariant `commit_agent_tail` keys on; the old event list is what drifted).
  `flushes_segment` agrees with the fold for every event variant
  (`every_flush_point_is_declared`); a batch announcement replaces the queue;
  all three resolutions record `truncated`; a backgrounded call charges its
  acknowledgement.
- `app` (unit): a foreground group's resolution settles its members' live
  calls (`a_foreground_groups_resolution_settles_its_members_live_calls`) —
  the running call resolved onto the transcript, the queue cleared.
- `tests/live_openrouter.rs` (live, `#[ignore]`): the same one-frontier
  contract over a **real model's** streamed reply — hostile markdown at nine
  widths including degenerate narrows, and a CJK+emoji corpus at odd widths
  where a two-column glyph straddles the wrap point — tracking the provider's
  own chunk boundaries so a failure names the split that broke the render.
  The corpus a generator cannot invent, checked at every character prefix.
  `apply` buffers `ThinkingChunk` only while a phase is open,
  opening one flushes the text before it, the settle records
  `HistoryItem::Reasoning` **ahead of** the partial reply on both an `Error`
  and a `StreamDone`, the round's `Usage` snaps the count, an interrupt keeps
  a partial thought, and a `Retrying` shows until the next chunk clears it.
- `ui` (stress, `stream_stress.rs`): the agent view's frontier holds
  `committed ++ strip == the reply so far` over 40 generated hostile
  documents × 3 widths × **every character prefix**, driven through the real
  `preview_lines` dispatch — the main harness's contract, in the view that
  kept a second copy of it. The row comparator carries the text, both
  colours, the modifiers **and `underline_color`**, which is the OSC 8 link
  carrier (`docs/links.md`) rather than decoration: without it a
  batch-vs-streaming divergence in link interning would pass unseen.
- `stream` (unit): the `agent-stream` entry has an example prompt that selects
  *it* (the registry's reachability test) and is skipped when no registry is
  attached, falling through to the `table` demo it outranks.
- `app` (unit): `last_assistant_text` follows the viewed agent, and `/copy`
  from inside the view returns that text.
- `scripts/smoke.sh` **Phase 98**: the offline `agent-permission` scenario
  (`src/stream/dummy/agent.rs`, cue "subagent" + "permission"). Three details
  make it reproduce, and all three are the real backend's shape: the launch is
  **foreground** (a background group resolves at once, so no live lead cell
  survives to cover anything — which would make the negative assertion below
  vacuous), the calls are announced as a **parallel batch** before any runs,
  and the request is raised **before** the `ToolStart`, the approve seam's own
  order, which is why the asked-about call genuinely reads `⎿ Waiting…`. The
  phase asserts the lead's `● Agent(…)` cell *is* in the main view, that the
  ↓ ↓ Enter walk reached the session view (so a race fails naming its cause,
  not as a content mismatch), that the prompt there shows the agent's own
  `● Bash(ls -la)` over `● Bash(pwd)` with two `⎿ Waiting…` rows and **no**
  `● Agent(` cell, and that Esc back to the main view finds the lead's cell
  still on screen.
- `scripts/smoke.sh` **Phase 95**: drives the real binary. The dummy gains an
  `agent-stream` scenario (`src/stream/dummy/agent.rs`, cue "subagent") that
  launches one background subagent and plays *its* round on the subagent
  channel from that agent's own thread — a thinking phase, then the table —
  which is the first time the agent session view has been drivable with no
  network — a thinking phase, a **parallel batch of two `write` calls** (the
  two-text split, the reported case), then the table. The phase walks the
  roster into that session and asserts the live `● Thinking…` block shows, the
  forming **grid** (not just its closing border) is in the strip mid-stream,
  both write cells reach scrollback **with no resize**, the table commits
  exactly once, and the settled `Thought for …` cell lands on the agent's
  transcript.
