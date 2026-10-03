# The agent tools — the `agent` launch and its companions

The model controls its subagents the way it controls its commands
(`docs/bash-tools.md`): one tool launches, and one tool per action works
the agent it launched — by **id**.

| tool | arguments | what it does | asks permission |
| --- | --- | --- | --- |
| `agent` | `description`, `prompt`, `subagent_type?`, `run_in_background?` | launch a subagent; the result names its id | never (a launch runs nothing itself) |
| `agentsend` | `agent_id`, `message` | send the agent a message: into its running loop, or a new turn over its finished conversation | never |
| `agentwait` | `agent_id`, `wait?` | wait for the agent to finish, or `wait` seconds; `wait: 0` just looks | never |
| `agentoutput` | `agent_id` | what the agent has done so far — its tool calls, one line each, and its reply so far or its final response | never |
| `agentkill` | `agent_id` | stop the agent | never |
| `agentlist` | — | every agent of this session: id, type, description, state, running time | never |

The companions' wire names are one lowercase word each — `agentsend`,
`agentwait`, `agentoutput`, `agentkill`, `agentlist` — the bash family's and
the task tools' convention: the cell shows `AgentSend`, and the display name
lowercases to the wire name, so the context replay needs no table for them.

`docs/agent-tool.md` is the launch itself (the roster, the group cells, the
session view the *user* chats through); this page is the model's side of the
same agents.

## Why

Before this, a launch acknowledgement named the agent by its description
alone — "nothing model-facing takes an agent id back" — and the model's whole
reach into a running agent was the completion notice. What it could not do:

- **ask what an agent is doing.** A background agent that had been running
  for two minutes was a row on the user's roster and nothing to the model.
- **send a follow-up.** The user could type into an agent's session; the model
  could only launch a fresh agent with a fresh context and re-explain the
  task — losing everything the first one had read.
- **continue a finished agent.** The same, after the fact: an agent that had
  explored the repository and reported was gone from the model's reach the
  moment its result landed, though its conversation was still stored for the
  user's chat continuation.
- **wait on purpose.** A background launch is the default, and the only way
  to block on one was to have launched it foreground in the first place.
- **stop one.** Only the user's `x` could.

Every one of these is an action a model takes on a *command* through the
bash companions, by the session id the `bash` result named. The agent family
takes the same shape.

## The id

Every agent has had a registry id since the roster existed (`a` + 8 base36
characters, `a7k2m9x4q`). It is now in every text the model reads about the
agent, so the model can act on it:

- the **launch acknowledgement** of a background launch names it (`Async
  agent "…" launched as a7k2m9x4q …`), and says what the companions do;
- a **foreground result** closes on a line naming it, so a finished agent
  can be continued — the final response itself is left verbatim above the
  line;
- the **completion notice** (`[background agent a7k2m9x4q] Agent "…"
  completed in 35s.`) names it, so a notice is enough to send a follow-up;
- the **Ctrl+B handoff** text names it, like the launch's.

The user sees it too: the footer roster's row carries it before the elapsed
(`general-purpose  Fetch Warsaw weather  a7k2m9x4q · 48s · ↓ 1.2k tokens`),
and the committed group cell's tree rows name it beside the description —
which is how the user matches a `● AgentSend(…)` cell to the row it talks to.

A stopped agent's result keeps the exact stopped note it always had
(`[agent stopped by the user before completion]`): the roster reads that
text back to tell a stop from a failure, and a stopped agent cannot be
continued anyway.

## The contract

### `agentsend`

```json
{"agent_id": "a7k2m9x4q", "message": "Also check the integration tests."}
```

Delivers the message and returns **at once**, saying how:

- the agent is **running** → the message waits on its queue and its loop
  takes it at the next round boundary, exactly as a message the user types
  into its session does (`docs/queue.md`); the result says so;
- the agent has **finished** → a new turn starts over its stored
  conversation with the message as its newest user message — the user's
  idle-submit continuation, one level down; the result says so. The agent's
  context is kept whole, which is the point: a follow-up costs nothing the
  first task did not already pay for;
- the agent was **stopped** or **failed** → refused with the reason (a
  killed loop reads nothing, and a failed run has no conversation to
  resume).

Either way the agent's completion is announced the way a background
launch's is: the notice carries its final response, and an idle session
starts the automatic follow-up turn. A *foreground* agent the model continues
becomes a background one at that moment (`AgentEvent::Background`), since its
result will arrive as a notice rather than inside a call.

The message reaches the agent through the **same seam** both delivery paths
share: the registry's pending-input queue, drained at every round boundary
of `run_agent`, which announces it with `StreamEvent::Steered` — so the
agent's own transcript records the message as a user bubble when its loop
genuinely has it, in its session view and its Ctrl+O cell alike.

### `agentwait`

```json
{"agent_id": "a7k2m9x4q", "wait": 300}
```

Blocks until the agent finishes — polling the registry every 30 ms, like the
foreground launch's own wait loop — or `wait` seconds pass (default 120, max
600; `0` returns what there is now). The call ends early on an Esc (the
turn's cancel) and on the user's **Ctrl+B** (the latch the foreground group
wait reads too), saying the agent keeps running. The result is the
`agentoutput` report at that moment: the agent's state, its steps, and its
final response once it has one.

A completion `agentwait` or `agentoutput` reports is **observed**: no
completion notice is posted for it, neither on the model's board nor as the
user's cell — the bash companions' rule (an exit a `bashwait` reported is not
announced again), because the model has already read the response in the
call's result and a notice would hand it the same text twice. The mark lives
on the notice board itself (`BackgroundRegistry::observe`, under the board's
own lock beside the notes), which is what makes it race-free: the event loop
posts an agent's note **through** the board's tag check, and a companion that
reports a final state marks the tag and removes any note already posted,
under the same lock, so neither order lets a note slip through. A
continuation (`agentsend`, the user's chat) clears the mark, because the next
settle is a new completion.

### `agentoutput`

```json
{"agent_id": "a7k2m9x4q"}
```

The agent's progress as the registry has recorded it — the user's ask, with
"only the summary" as the shape:

```
Agent a7k2m9x4q (general-purpose, "Fetch linuztx's GitHub profile"): running 48s · 3 tool uses · 1.2k tokens
Steps so far:
  Bash(curl -s https://api.github.com/users/linuztx)
  Bash(curl -s "https://api.github.com/users/linuztx/repos?per_page=100&sort=updated")
  Bash(curl -s https://api.github.com/users/linuztx/events/public)
Reply so far:
  (nothing yet)
```

Each step is the call's cell header — `{Name}({args})`, the same
`display_name`/`summarize_call` pair the cells and the classifier's action
log use, one line each — and a message delivered into the agent reads
`Message: …` in its place among them. A finished agent's report says
`finished in 1m 2s`, lists its steps, and closes on `Final response:` over
the text the caller received; a failed one names the error; a stopped one
says so. Past [`AGENT_REPORT_MAX_STEPS`] the oldest steps fold into one `… +N
earlier steps` line, so a long run's report stays a screen.

The record is kept by the **registry**, not read off the roster: the
companions run on the backend's thread, and the roster is the event loop's.
`AgentRegistry::send` is the one chokepoint every subagent event passes
through — the live forwarder's and the offline demo's alike — so it folds the
progress there: a `ToolStart` is a step, a `ToolTitle` refines the newest one,
a `Chunk` grows the reply so far (reset at a round boundary, the forwarder's
own final-text rule), a `Steered` is a message step, a `Usage` frame counts
the tokens.

### `agentkill`

```json
{"agent_id": "a7k2m9x4q"}
```

Stops a running agent: cancels its token and marks it settled, as the user's
`x` does, and sends `AgentEvent::Stop` on the agent channel so the event loop
settles its roster row through the same path the `x` takes — the row lingers
red, its session view shows `Interrupted`, a waiting foreground group
resolves. The completion is observed (the model did it), so no stop notice is
posted. An agent that has already finished is reported as finished rather
than stopped — there is nothing left to stop, and the result says so.

### `agentlist`

```json
{}
```

Every agent of this session, in launch order — id, type, description, state
and how long it has run (`running 48s`, `finished in 1m 2s`, `stopped after
12s`, `failed after 5s: …`) — so a model that lost an id to a `/compact` or
a `/resume` finds it without guessing, the `bashlist` rule.

## What changed underneath

- **Agents outlive their roster rows.** The roster used to drop a settled
  agent 30 s after its linger *and* drop its registry slot with it, so a
  follow-up sent a minute later would have found nothing. The linger sweep
  now **hides** the row (the second `x`'s clear, applied by the clock) and
  keeps both the entry and the slot: a continuation reopens the entry — its
  whole transcript intact, so its session view shows the earlier exchange
  above the new one — and the row comes back for the run's duration, then
  lingers and hides again. `/clear` and quit still drop everything.
- **The registry keeps what the companions report**: each slot's
  description, launch order and clock beside its cancel token and stored
  conversation, and the progress log above. `register` takes the
  description; `finish` and `kill` stamp the end.
- **The event loop learns two things from the backend** it could not before,
  on the agent channel (`AgentEvent` grew two variants beside `Stream`):
  `Background { id }` — the model continued a settled foreground agent, so
  its next settle owes a notice — and `Stop { id }` — the model stopped it,
  settle the row. The loop's `select!` arm binds every variant; a refutable
  pattern there would silently drop the ones it did not name.
- **The companions are ordinary tools.** Only the launch is special (it
  resolves into a group cell); `agentsend` and the rest run through
  `run_agent`'s ordinary path — the batch announcement, the permission seam
  (where they never ask), the hooks, a `● AgentSend(…)` cell, a `Tool`
  record, the rollout, the context replay — and the main backend's execute
  closure routes them to `llm::agent_tools` ahead of the real executor, the
  way the ask, task and skill tools are routed. A subagent is refused them
  recoverably: agents do not reach other agents, launches included.
- **Cells name the agent by its description**, as session cells name the
  command: `● AgentSend(Fetch Warsaw weather ← Also check the tests)`,
  `● AgentWait(Fetch Warsaw weather)`, `● AgentOutput(…)`, `● AgentKill(…)`,
  `● AgentList` — `summarize_call_naming` resolves an agent id through
  the same lookup that names a session's command, which the backend answers
  from both registries.
- **Hooks** (`hooks::claude_code_alias`): each companion answers to its cell's
  display name as a second exact name — `AgentSend`, `AgentWait`,
  `AgentOutput`, `AgentKill`, `AgentList`; the launch keeps its `Task` alias.
- **Auto mode's classifier** logs them as actions beside the launch, and
  reviews none of them — nothing they do runs a command.

## The offline demo

The `agent-tools` scenario (`scripts/smoke.sh` Phase 130) launches the
table-streaming demo agent in the background and drives every companion
against it with the **real executor** — `agentlist` names it, `agentoutput`
lists its `Write(…)` steps mid-run, `agentsend` queues a message its round
boundary takes (the bubble lands on its transcript), `agentwait` blocks until
its table closes and returns the final response, and `agentkill` on the
finished agent reports that it had already finished — so every cell carries
live output, the dummy-backend rule (`docs/dummy-backend.md`).

[`AGENT_REPORT_MAX_STEPS`]: ../src/agents.rs
