# The agent tools — every agent has an id, and the lead can work it

`agent` launches a subagent (`docs/agent-tool.md`). Until now that was the
only thing the lead model could do with one: a background agent was a
fire-and-forget launch whose final response arrived as a notice, and a
finished agent was gone. The bash family showed the better shape —
`bash` starts a command and four companions work the session it leaves
(`docs/bash-tools.md`) — and the agent tools are that shape, one level up:

| tool | arguments | what it does | asks permission |
| --- | --- | --- | --- |
| `agent` | `description`, `prompt`, `subagent_type?`, `run_in_background?` | launch an agent; the result names its id | never |
| `agentsend` | `agent_id`, `message` | a running agent reads the message after its current step; a finished one **resumes its conversation** with it, in the background | never |
| `agentoutput` | `agent_id`, `wait?` | the agent's state, the tool calls it has made, and its final response once it finishes; `wait` seconds blocks until it finishes | never |
| `agentkill` | `agent_id` | stop a running agent — its conversation is kept, `agentsend` resumes it | never |
| `agentlist` | — | every agent of this session: id, type, task, state, runtime, tool uses | never |

The companions' wire names are one lowercase word each, the bash
companions' convention: the cell shows `AgentSend`, and the display name
lowercases to the wire name, so the context replay needs no table for them.
They are the **lead's** tools: a subagent is never offered them, exactly as
it is never offered `agent` — agents don't nest, and an agent steering its
siblings is a coordination problem this TUI does not take on.

## Ids

Every agent always had one — the registry's `a` + 8 base36 characters
(`a7k2m9x4q`) — but nothing model-facing carried it, because nothing
model-facing took it back (the launch text's own comment called an
`agentId:` line "a token with no consumer"). Now four tools consume it, so
every result that introduces an agent names it:

- a **background launch** — `Async agent "Fetch GitHub profile" launched as
  a7k2m9x4q and working in the background. …`, which also steers off
  polling (a wait is for when the result is needed now);
- a **foreground result** — the agent's final response, then
  `[agent a7k2m9x4q — agentsend continues this conversation]`;
- a **completion notice** — `[background agent] Agent "Fetch GitHub
  profile" (a7k2m9x4q) completed in 35s.`;
- the **Ctrl+O agent cell** — `● Agent(Fetch GitHub profile) · a7k2m9x4q`,
  so the user can match an `AgentSend` cell to the agent it reached.

A recorded entry's `output` is immutable — it is what the model read — so a
launch from before this change replays without an id, as it was sent. A
completion notice is different: its note is rendered from the record at each
replay, so an older one gains its id there (the prompt cache misses once, on
the first resumed turn that carries it).

## The contract

### `agentoutput`

```json
{"agent_id": "a7k2m9x4q"}
{"agent_id": "a7k2m9x4q", "wait": 300}
```

What the agent has been doing, as a **summary** — one `Name(args)` line per
tool call, the same one-liners the Ctrl+O agent cell lists — under a frame
line saying where it stands:

```text
Running (agent a7k2m9x4q · Fetch GitHub profile · 42s · 3 tool uses)
Bash(curl -s https://api.github.com/users/linuztx)
Bash(curl -s "https://api.github.com/users/linuztx/repos?per_page=100&sort=updated")
Bash(curl -s https://api.github.com/users/linuztx/events/public)
```

Once it has settled the frame says how, and what it answered follows:

```text
Done (agent a7k2m9x4q · Fetch GitHub profile · 1m 5s · 3 tool uses)
Bash(curl -s https://api.github.com/users/linuztx)
Bash(curl -s https://api.github.com/users/linuztx/repos)
— message received —

Final response:
…
```

`Failed (…)` carries its error, `Stopped (…)` says by whom — and whether
`agentsend` can resume it. Only the newest 30 calls are listed, each cut to
one line; earlier ones are counted (`… 12 earlier tool calls`). The tool
outputs are deliberately absent: they are the agent's context, not the
lead's, and the point of a subagent is that the lead never pays for them.

The frame's numbers cover the agent's **whole life** — its runtime and its
tool uses summed over the launch and every continuation — and each message
it read is marked where it arrived: `— message received —` for the lead's
own `agentsend` and a background shell's completion routed to it, and
`— message from the user: {text} —` for one the user typed into its session,
quoted because the lead never saw it (*The user's own messages*, below).
Both came out of a live run: the frame
used to give the current run's runtime beside the lifetime's tool count,
and a model read `9s · 5 tool uses` as five calls its follow-up had made —
it had made none, which the mark after the fifth call now shows.

`wait` (seconds, default 0, at most 600) blocks until the agent settles or
the time passes. A waiting cell **streams** each call the agent starts
during the wait, one line each, so the user watches the agent work from the
lead's own transcript, and Ctrl+B ends the wait early (the agent runs on).
Every report of an agent still at work — a look, or a wait that passed —
closes on a note saying it is still running and how to wait for it: a model
that looked once without `wait` told the user the agent had stopped.

### `agentsend`

```json
{"agent_id": "a7k2m9x4q", "message": "Also list the repos with the most stars."}
```

- **Running** → the message joins the agent's queue and reaches its loop at
  the next round boundary — the same seam a user's message typed into the
  agent's session view rides (`docs/queue.md`). The completion notice, when
  it comes, answers both.
- **Settled** (done, failed, or stopped by the lead) → the agent's stored
  conversation **resumes** with the message as its newest turn, in the
  background; its next completion is noticed like a background launch's.
  This is the follow-up the user asked for: a finished agent keeps its
  context, so a second question costs one message instead of a fresh agent
  re-deriving everything.
- **Stopped by the user** → refused. The `x` in the roster said stop, and a
  model must not undo it; launching a new agent is still possible.

It returns at once. To block on the answer, follow it with
`agentoutput` + `wait` — both calls can ride one message, and they run in
order.

### `agentkill`

Stops a running agent: its loop is cancelled exactly as the roster's `x`
cancels it, the running call resolves `Interrupted by user`, and the result
is the `Stopped (…)` report. Unlike the user's stop it **keeps the
conversation resumable** — kill-then-send is how the lead redirects an agent
that went off course without waiting for its current step. An agent that
already settled is reported as such; nothing is stopped twice.

### `agentlist`

```text
3 agents:
- a7k2m9x4q (general-purpose) Fetch GitHub profile — running 42s · 3 tool uses
- a1b2c3d4e (explore) Search the repo — done in 1m 5s · 12 tool uses
- a9z8y7x6w (general-purpose) Fix the tests — stopped by the user after 30s · 2 tool uses
```

Running agents and the retained settled ones, launch order — so a lead that
lost an id to a `/compact` finds it again without guessing.

## The user's own messages

The user can talk to an agent too — Enter in its session view queues a
message into its running loop or starts a continuation of a finished one
(`docs/queue.md`). The lead never wrote those messages, and it used to have
no way to tell: the agent's next answer came back as an ordinary completion
notice, reading like a reply in the lead's own conversation, and a chat with
an agent whose foreground group had already resolved never reached the lead
at all. So the user's messages are attributed wherever the lead hears from
the agent:

- the **completion notice** names them between its outcome and the answer —

  ```text
  [background agent] Agent "Fetch GitHub profile" (a7k2m9x4q) completed in 35s.
  The user messaged this agent directly in its session view — these came from the user, not from you:
  - also list the repos with the most stars
  Final response:
  …
  ```

- a **foreground result** carries the same lines between the answer and the
  `agentsend` line, for a message the user sent while the lead waited;
- the **`agentoutput` report** quotes each one where it arrived.

A notice names only the messages **its own run** read: each continuation
starts with none (`AgentRun::user_messages`, cleared by `reopen`; the
registry's `user_messages(id)`, counted from the run's start). And a run the
user started or steered **owes a notice even for a foreground agent** once
its group has resolved — the group's result can no longer carry it — while a
member of a group still waiting is reported by that group, never twice.

The roster tells them apart without a new event: only a message the user
typed waits on `AgentRun::queued` (the lead's `agentsend` and a routed shell
note arrive unannounced), so a `Steered` delivery that clears a queued row is
the user's, and a continuation the user started is recorded as theirs by
`App::agent_chat`. On the registry side the user's door is
`queue_user_input` / `note_user_message` beside the lead's `queue_input`. The
notice's list is kept in the rollout (`user_messages`, omitted when empty),
so a resumed session replays the same note.

## Retention

A settled agent's registry slot used to be dropped with its roster row, 30
seconds after it finished (`AGENT_LINGER`). A follow-up needs the
conversation to outlive the row, so the two are decoupled:

- the **roster row** still lingers and is swept exactly as before;
- the **registry slot** — its stored messages, its progress record — is kept
  for the [`AGENT_RETAINED_MAX`] (16) most recently settled agents, the
  oldest evicted first when a new agent registers;
- the swept **roster entry** moves to a retired list of the same size in
  `App`, so an agent the lead resumes comes back to the roster *with* its
  transcript, and its session view shows the whole conversation. One evicted
  from that list comes back on its prompt alone.

`/clear` drops all of it — the slots, the retired entries — since the
conversation that knew those ids is gone. Nothing survives a restart: an id
from a resumed session is answered with the list of agents that do exist.

## One notice per answer

A background agent's completion posts a note the lead hears at its next
round boundary. An `agentoutput` that already reported the same outcome
would make that note a duplicate, so the registry runs the bash sessions'
waiter protocol (`BackgroundRegistry::finalize`):

- the subagent's forwarder **holds back** the run's terminal event
  (`StreamDone`/`Error`) and hands it to `AgentRegistry::settle` once the
  outcome is recorded;
- with nobody waiting, `settle` sends it at once — **under the registry
  lock**, so a launcher that sees the agent done (the foreground group's
  wait loop) knows the event is already on the channel, which is what lets
  the group's resolution drain it first (`docs/agent-tool.md`);
- with an `agentoutput` waiting, the event is held; the waiter's
  `end_wait` reads the final state and releases the event **in one locked
  step**, marked `observed` when that state was settled — and an observed
  settle posts no note and commits no notice cell (the `AgentOutput` cell
  is the record).

A user's `x` asks the same question atomically (`kill` returns whether a
waiter will report the stop), so a wait ended by the user's stop is not also
noticed.

A settle **no wait saw** goes out unobserved: the one that lands in the
instant between `agentoutput` reading the agent as running and registering
its wait, or one that lands before the lead looks at all. Any `agentoutput`
that hands the lead a settled outcome then tells the registry
(`AgentRegistry::report_settled`), and the event loop accounts for **every**
unobserved settle with it (`AgentRegistry::post_notice`, a note owed or not),
both under the registry lock, so the two never cross:

- the report comes first: the slot's count of unposted notices is spent, the
  loop posts nothing, and it settles the group entry with no notice cell —
  the observed path;
- the note is posted but the lead has not taken it: the report takes it back
  off the board (`BackgroundRegistry::retract_agent_notice`, the board's
  notes tagged with their agent), and the notice cell held with it is
  dropped when the loop settles it at the turn's end
  (`BackgroundRegistry::take_retracted`). The board remembers retractions by
  each note's **number**, not by agent: the loop holds a cell until the lead
  reads its note (`docs/background.md` *Where a notice lands*), so a resumed
  agent's next notice can be posted while a retracted one is still held, and a
  mark kept per agent could not tell the two apart;
- the lead already took it: the report is a re-read, and the cell records
  the note it read.

The edge left: a notice cell that committed at a mid-turn boundary before
the report took its untaken note back stays in the history, so later turns'
context carries the answer twice.

## Verified live

Against Venice (`z-ai-glm-5-3`, through the Agent Zero proxy) and Ollama
Cloud (`gpt-oss:120b`), in the real binary under tmux: a background launch,
an `agentoutput` wait that streamed the agent's calls and reported its
answer with no notice after it, an `agentsend` that resumed the finished
agent with its context and a second wait on it, `agentlist`; an
`agentkill` mid-loop followed by an `agentsend` redirecting the same agent,
whose answer then arrived as a notice that started the idle follow-up
turn; and a message queued into a running agent, read after its current
step and shown as a user bubble in its session view.

Two of those runs are kept as `#[ignore]`d live tests in
`tests/live_subagents.rs` (`A0_VENICE_API_KEY=… cargo test --test
live_subagents -- --ignored`): a finished foreground agent resumed by id and
waited on with `agentoutput`, its new answer under its own definition; and a
background agent inspected, stopped by the lead and listed, the registry
agreeing that the stop is resumable.

Offline, the dummy's `agent-follow-up` demo plays the first of those with no
network — its companion calls through the real executor, its agent's runs
settling through the real registry — and `smoke.sh` Phase 130 drives it in
the real binary (`docs/dummy-backend.md`).

## Wiring

- **Permissions**: none of the companions ask — a launch never did, and a
  message to an agent runs nothing itself; whatever the agent then runs
  meets the gate as its own call (`docs/permissions.md`).
- **Secrets**: `agentsend`'s message is handed to another model, so its
  placeholders are never expanded (`crate::secrets::expands_placeholders`);
  every report is redacted on the way out like any tool's.
- **Hooks** (`hooks::claude_code_alias`): each companion answers to its
  display name as a second exact name — `AgentSend`, `AgentOutput`,
  `AgentKill`, `AgentList`.
- **Cells**: `● AgentSend(Fetch GitHub profile ← Also list…)`,
  `● AgentOutput(Fetch GitHub profile)`, `● AgentKill(Fetch GitHub
  profile)` — the agent named by its description from the moment the call
  is announced (`tools::summarize_call_naming`, the bash sessions' lookup
  answering agent ids too). A waiting `AgentOutput` renders like a running
  command: the streamed calls tail under a `(12s · wait 5m)` clock row and
  `(ctrl+b to stop waiting)`.
- **The roster**: a resumed agent reappears on it (or reopens there, if
  still lingering) with a fresh runtime clock; one the lead stopped turns red
  without the user's `x to clear` row, and lingers the natural 30 seconds.

## Layout

| | |
| --- | --- |
| `src/llm/tools.rs` | the specs, the argument types, the display names and header summaries |
| `src/agents.rs` | the registry (identity, progress, retention, the waiter protocol) and the pure report formatting |
| `src/llm/agent_tools.rs` | the executor — the companions over the registry, a continuation spawned through an injected launcher |
| `src/llm/backend.rs` | the forwarder's hold-back, the ids in the launch texts, the routing |
| `src/app/agent.rs`, `src/tui/agent.rs` | the roster side: observed settles, the lead's stop, resume and revival |

[`AGENT_RETAINED_MAX`]: ../src/agents.rs
