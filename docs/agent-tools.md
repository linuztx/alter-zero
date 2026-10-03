# The agent tools — one tool per action, an id for every agent

The `agent` tool launches a subagent (`docs/agent-tool.md` is the launch,
the roster and the session view). What was missing was the **rest of the
conversation**: once an agent was running — or done — nothing the model
could call reached it again. A finished agent's result arrived as a notice
and the agent was gone; a running one could only be waited on by the
`run_in_background: false` launch that blocked the whole turn; a follow-up
question meant a new agent with none of the old one's context.

So the `agent` tool got the `bash` tool's shape (`docs/bash-tools.md`): every
agent has an **id** the model is told, and one tool does one thing to it.

| tool | arguments | what it does | asks permission |
| --- | --- | --- | --- |
| `agent` | `description`, `prompt`, `subagent_type?`, `run_in_background?` | launch an agent; return its id at once (the default) or its final response | never |
| `agentsend` | `agent_id`, `message` | send a message to an agent: a running one reads it at its next step, a finished one starts a new turn on its own conversation | never |
| `agentwait` | `agent_id`, `wait?` | wait for an agent to finish — its final response — or `wait` seconds (default 120, max 600); `wait: 0` just checks | never |
| `agentoutput` | `agent_id` | the agent's progress: the tool calls it has made so far, and its final response once it is done | never |
| `agentkill` | `agent_id` | stop an agent | never |
| `agentlist` | — | every agent of the session: id, type, task, status, running time, tool uses, tokens | never |

The companions' wire names are one lowercase word each — `agentsend`,
`agentwait`, `agentoutput`, `agentkill`, `agentlist` — the bash companions'
convention: the cell shows `AgentSend`, and the display name lowercases to
the wire name, so the context replay needs no table for them. None of them
asks permission: nothing runs that the agent's own tool calls do not already
ask for, a kill ends work the model started (the roster's `x` asks nothing
either), and the rest only read.

## The contract

### Ids

An agent id is the registry's `a` + 8 base36 chars (`a7k2m9x4q`), and every
model-facing text about an agent names it:

- the background launch acknowledgement — `Agent a7k2m9x4q launched in the
  background: general-purpose "Fetch weather in Manila". …`;
- the foreground result, which closes on a `[agent a7k2m9x4q finished —
  agentsend sends it a follow-up]` line under the response;
- the Ctrl+B handoff note;
- the completion notice — `[background agent] Agent a7k2m9x4q "Fetch weather
  in Manila" completed in 35s.` over the final response, then `Reply to it
  with agentsend — it keeps its conversation.`;
- `agentlist`, which is where a model that lost an id to a `/compact` or a
  `/resume` finds it again without guessing.

The launch acknowledgement used to be deliberately id-free ("nothing
model-facing takes one back"). Now five tools do.

### `agentsend`

```json
{"agent_id": "a7k2m9x4q", "message": "Also check Tokyo, same format."}
```

Which of two things happens is the **registry's** call, exactly as it is
for a message the user types into an agent's session view
(`docs/queue.md`):

- the agent is **running** → the message is queued into its loop and read at
  the next round boundary, where `StreamEvent::Steered` lands it on the
  agent's transcript as a user message. The result says so and returns at
  once.
- the agent has **finished** (or failed) → a **continuation turn** starts
  over its stored conversation with the message as its newest user turn —
  the agent remembers everything it did. The model's message is echoed onto
  the agent channel as a `Steered` event *before* the run starts, so the
  roster records it and a row the sweep had hidden comes back
  (`AgentRun::reopen` un-hides).
- the agent was **stopped** (`agentkill`, the user's `x`, an Esc on its
  group) → refused: a stopped agent's loop is cancelled and nothing may
  resurrect it. The result says to launch a new agent.

Either way the answer arrives the way a launch's does: the completion
notice when the agent finishes, or `agentwait`. `agentsend` never waits —
an agent's reply takes seconds to minutes, not the ten seconds a REPL's
takes.

### `agentwait`, `agentoutput`

```json
{"agent_id": "a7k2m9x4q", "wait": 300}
{"agent_id": "a7k2m9x4q"}
```

Both answer with the same **report**, the `bash` family's small grammar
over an agent:

```
Running (agent a7k2m9x4q) — general-purpose "Fetch weather in Manila" · 48s · 3 tool uses · 16.5k tokens
Bash(curl -s https://api.github.com/users/linuztx)
Bash(curl -s "https://api.github.com/users/linuztx/repos?per_page=100&sort=updated")
Bash(curl -s https://api.github.com/users/linuztx/events/public) ← running
```

```
Done (agent a7k2m9x4q) — general-purpose "Fetch weather in Manila" · 1m 2s · 3 tool uses · 16.5k tokens
Bash(curl -s https://api.github.com/users/linuztx)
…
Response:
{the agent's final message}
```

The frame line says where the agent stands — `Running`, `Done`, `Failed`
(an `Error: …` line closes the report), `Stopped` — over its type, its
task, its runtime and its counters. Under it, the **tool calls it has made**,
one `Name(args)` header per call in order — the user's own example, and
exactly the headers the Ctrl+O agent cell shows — the call still in flight
marked `← running`, the newest [`REPORT_MAX_CALLS`] kept with a line saying
how many earlier ones were left out. The agent's streaming reply text is
**not** in a running report: the summary is the point, and a half-streamed
paragraph is neither progress nor a result. A finished agent's report ends
with `Response:` over its final message.

`agentoutput` returns at once. `agentwait` polls the registry until the
agent settles or `wait` passes — the parent's `cancel` (Esc) ends it with
`Interrupted by user` like every blocking call, and **Ctrl+B** ends the wait
early with a note that the agent keeps running (the `bashwait` handoff; the
cell's hint reads `ctrl+b to stop waiting`). A wait that runs out returns
the `Running` report.

### `agentkill`

```json
{"agent_id": "a7k2m9x4q"}
```

Cancels the agent's token (its running tool call ends with it, the way an
Esc ends the main turn's) and marks the slot killed, then reports
`Stopped (agent …)` over the calls it had made. The roster hears of it on
its own channel — `AgentEvent::Stopped` — and settles the row locally,
red, exactly as the user's `x` does, minus the `was stopped by user` notice:
the model did it, so it knows, and the `AgentKill` cell is the user's
record. An agent that has already finished is refused — there is nothing to
stop, and `agentsend` is how a finished agent is continued.

### `agentlist`

```
2 agents:
- a7k2m9x4q: general-purpose "Fetch weather in Manila" — running 48s · 3 tool uses · 16.5k tokens
- a3b4c5d6e: explore "Find the config loader" — done after 1m 2s · 5 tool uses · 9.1k tokens
```

Every agent the session still holds, in launch order: running ones, and
finished ones that can still be continued.

## What holds the answers: the registry's progress record

The roster (`App::agents`, the event loop's) folds every agent event into a
transcript the session view renders. The companions run on the **backend
thread**, which cannot read `App` — so the registry
(`agents::AgentRegistry`, the handle the loop and the backend share) keeps
its own, leaner record per slot: an [`AgentProgress`] — the call headers,
whether the newest call is still running, the usage-summed token count,
the tool-use count — folded by `AgentRegistry::send` from the same events
the loop receives, so the dummy's scripted agent and the live forwarder
record alike without either calling anything extra. Beside it the slot
keeps what the report's frame needs: the type and description the launch
registered, when the run started and settled, and the outcome.

`AgentRegistry::snapshot(id)` / `snapshots()` hand the companions a copy of
all of it ([`AgentSnapshot`]), and the pure `agents::report` module turns a
snapshot into the texts above — unit-tested with no thread anywhere.

## A finished agent is kept, not swept

The follow-up to a finished agent needs the agent to still exist: its
registry slot (the stored conversation a continuation resumes from) and
its roster entry (the transcript its session view and Ctrl+O cell show).
The roster used to sweep a finished agent off the footer after its 30 s
linger **and** drop both. Now the sweep distinguishes:

- **done** or **failed** → the row is **hidden** (the footer shows it no
  more, exactly as before) but the entry and the slot stay, so `agentsend`
  can continue it and `agentlist` still names it. A continuation un-hides
  the row: it is running again.
- **stopped** (`agentkill`, the user's `x`, an Esc) → removed outright, as
  before — a cancelled loop cannot be continued, so there is nothing to
  keep.

`/clear` and quit still drop everything (`AgentRegistry::kill_all` now
removes the slots with the cancel, so a cleared session lists no ghosts).

## The completion notice and a result the model already has

A background agent's completion posts a model-facing note on the shared
notice board (`docs/background.md`), which the parent reads at its next
round boundary — or which starts an automatic follow-up turn when the
session is idle. `agentwait` and `agentoutput` report the same result when
they find the agent settled, and a model that waited for a response and
then read it again as a `[background agent] … completed` note would
reasonably wonder whether the agent ran twice.

So a companion that reports a settled outcome **marks it observed** on the
slot, and the note is posted at the **deferred settle** (the same safe
boundary the notice cell commits at — the next tool resolution, else the
turn's end) only when nothing has observed it. The timing is what makes
this race-free enough: the cell settle after a companion's resolution is
the one that would post, and the companion marked the slot before it
returned. The green `● Agent "…" finished` cell still commits either way —
it is the user's record, not the model's. A continuation resets the flag,
so its own completion is reported once more.

## Wiring

- **Specs** (`llm::tools::agent_tool_specs`): the six agent tools ride the
  **main** backend's set only (`tool_specs_with_agents`); a subagent is
  never offered any of them, and one that names a companion anyway is
  declined recoverably — agents don't control agents.
- **Execution**: the main spawn's execute closure routes a companion to
  `llm::backend::run_agent_companion` ahead of the real executor, with the
  registry, the subagent config (a continuation is a `spawn_subagent_run`),
  the background registry (the Ctrl+B latch) and the turn's cancel.
- **Permissions** (`llm::approval`): none asks; `permission_request` has no
  arm for them, so the gate's no-prompt path answers.
- **Hooks** (`hooks::claude_code_alias`): each answers to its cell's display
  name as a second exact name — `AgentSend`, `AgentWait`, `AgentOutput`,
  `AgentKill`, `AgentList`.
- **Cells**: `● AgentSend(Fetch weather in Manila ← Also check Tokyo)`,
  `● AgentWait(Fetch weather in Manila)`, `● AgentOutput(…)`,
  `● AgentKill(…)`, `● AgentList` — an agent named by its description from
  the moment the call is announced, through the same lookup a `bashsend`
  cell names its session's command by (`summarize_call_naming`, over a
  closure that answers a shell id with its command and an agent id with its
  description — the two id shapes never collide, `b…` and `a…`). The report
  renders like a command's output: the multi-row `⎿` peek over the
  `… +N lines` hint, so the user sees the same summary the model read; a
  running `AgentWait` cell counts its clock against the call's own `wait`.
- **Context replay**: the companions are ordinary tool cells, so
  `context_messages` replays each with the model's verbatim arguments under
  its wire name (`AgentSend` lowercases to `agentsend`).

[`AgentProgress`]: ../src/agents.rs
[`AgentSnapshot`]: ../src/agents.rs
[`REPORT_MAX_CALLS`]: ../src/agents/report.rs
