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
tool call, the same one-liners the Ctrl+O agent cell lists, and what it said
on its way, each **progress update** an `Agent: …` entry where it said it
(`docs/agent-progress.md`) — under a frame line saying where it stands:

```text
Running (agent a7k2m9x4q · Fetch GitHub profile · 42s · 3 tool uses)
Agent: I'll start with the profile, then the repositories.
Bash(curl -s https://api.github.com/users/linuztx)
Bash(curl -s "https://api.github.com/users/linuztx/repos?per_page=100&sort=updated")
Agent: 42 public repos, most of them Rust.
  Checking the recent events next.
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
one line; earlier ones are counted (`… 12 earlier tool calls`), and what the
agent said among them goes with them. The tool outputs are deliberately
absent: they are the agent's context, not the lead's, and the point of a
subagent is that the lead never pays for them. Its own words are not: an
update is its account of what it found and what it is doing next, so it is
listed whole — up to 2,000 characters, its first line under `Agent: ` and
the rest indented beneath it.

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
the time passes. A waiting cell **streams** each call the agent starts, each
update it gives and each message it reads during the wait, one line each in
the report's order, so the user watches the agent work from the lead's own
transcript, and Ctrl+B ends the wait early (the agent runs on). Each poll
copies only what is new (`docs/agent-progress.md` *The wait stream*).
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
- the **`agentoutput` report** quotes each one where it arrived;
- and a message that starts a **settled** agent working again is told to
  the lead the moment it does (*A resume is news when it starts*, below),
  not only in the notice its new run ends with.

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

### A resume is news when it starts

A message that brings a **settled** agent back — one that had finished,
failed or been stopped — used to reach the lead only through that run's
completion notice, and until then the lead went on believing what it last
heard. The reported case: a foreground agent building a frontend hit an
error, the lead read `[agent failed: …]` and carried on without it, and the
user messaged the agent in its session view — which took its task up again
while the lead, knowing nothing of it, was free to redo the same work or
launch a replacement. So the message that starts the run is announced **when
it starts**:

```text
[background agent] Agent "Build the frontend" (a7k2m9x4q) is running again: the user resumed it after it failed. It continues its own conversation, and you will be notified with its final response when it finishes. Leave its task to it — don't redo the work or launch another agent for it; if you need its result to go on, agentoutput with `wait` blocks until it finishes.
The user messaged this agent directly in its session view — these came from the user, not from you:
- the API key is set now, carry on
```

It is an `AgentNotice` with `resumed` set — the shell notice's `waiting`
twin, a notice that reports no end: `status` is how the run before it had
ended (what the lead last heard), `user_messages` the message that resumed
it, and its cell reads `● Agent "Build the frontend" was resumed by user` in
the system notices' cyan. It travels the completion notices' way — on the
board, its cell held until the lead reads it at a round boundary, recorded
at once when the lead is idle — with two differences. It is posted as no
model-launched note (`BackgroundRegistry::post_agent_resume_notice`), so it
starts **no follow-up turn** of its own: there is nothing to answer yet; the
run's completion notice, still owed for the user's message, brings that
turn, and its context then carries both. And it is tagged apart from the
agent's completions (`PendingNotice::resumed`, beside `agent`), since it goes
stale on a different event (below). It changes nothing on the recorded group
entry (`App::settle_agent_completion` skips it): the Ctrl+O cell keeps the
outcome the lead read until the run reports its own.

The note is news only until the lead hears how that run ended, and two of
the lead's own calls can tell it first, while the note still waits for the
round boundary: an `agentoutput` report of the settled run, and `agentkill`,
whose result says the run is over either way — the report of the stop, after
which no notice comes, or `not running` for a run that had already ended,
whose completion notice still follows. Read after either, the note would say
the agent runs and that a notice will follow. So both take it back unread,
its held cell dropped with it
(`BackgroundRegistry::retract_agent_resume_notice`):
`AgentRegistry::report_settled` whichever way the run's completion stood
(*One notice per answer*), and `AgentRegistry::stop` — the lead's stop,
whether it stops the run or finds it over — in the same locked step. A
report that read the run from **before** the user started it again is the
exception: it takes back the completion it read and leaves the note, which
is about a run the lead has not heard of. The registry tells the two apart
under its own lock (is the run still over?), and the loop posts the note
under that lock too (`AgentRegistry::post_resume_notice`, only while the run
it announces is going), so a report, a stop and the post never cross. A run
that ended before its note could be posted owes none: its completion notice
names the user's messages as well, and the report of a stop the lead made
first marks them (`— message from the user: … —`).

Both ways a user's message reaches a settled agent count: a chat continuation
started from its session view — Enter, or a Tab follow-up dispatched at its
settle (`App::agent_chat`) — and a message queued while it wrote its last
reply, which its own thread continues with (the `Steered` that reopens a
settled run, folded by `App::apply_agent_event`). None is owed for the lead's
own `agentsend`, which it knows of; for a member of a foreground group the
lead still waits on, whose result carries the resumed run and names the
message; or for a message to an agent still at work, which the lead never
believed had ended — its completion notice names the message.

The roster must have seen the last run end for any of this to hold, and the
registry can be a step ahead of it: a Tab follow-up dispatches from the draw
tick, which may run before the agent channel's branch folds the settle, and
an `agentoutput` waiter's held end is released only as the new run begins
(`begin_continuation`). An entry still reading as running would take the
message without reopening, settle under the new run when the old end
arrived, and drop every event of the run that followed — its answer and its
notice with it. So when the registry answers `Started`, `Session::agent_chat`
first drains the agent channel, where the old run's end already waits.

`smoke.sh` Phase 141 drives the reported case through the real backend
against a stub provider: the subagent's request is refused, the lead goes
on with its turn, the user messages the failed agent back to work from its
session view, and the lead's very next request carries the note — right
after the result of the call it was making when the user spoke — while the
cell lands below that call's. Phase 142 runs the same case with the lead's
held round answering `agentkill` on the agent the user just brought back:
its next request carries the kill's report — the user's message marked in
it — and no note, and no resume cell commits. Phase 130 covers the idle
lead: the user messages the offline demo's agent once it has finished, and
the resume cell lands ahead of the answer's.

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

- the report comes first: the run's owed notice is spent (the slot keeps
  the runs it owes one, `notices_owed`), the loop posts nothing, and it
  settles the group entry with no notice cell — the observed path; a note
  of the run's end posted meanwhile (the user's stop) is taken back too;
- the note is posted but the lead has not taken it: the report takes it back
  off the board (`BackgroundRegistry::retract_agent_notice`, the board's
  notes tagged with their agent and run), and the notice cell held with it is
  dropped when the loop settles it at the turn's end
  (`BackgroundRegistry::take_retracted`). The board remembers retractions by
  each note's **number**, not by agent: the loop holds a cell until the lead
  reads its note (`docs/background.md` *Where a notice lands*), so a resumed
  agent's next notice can be posted while a retracted one is still held, and a
  mark kept per agent could not tell the two apart;
- the lead already took it: the report is a re-read, and the cell records
  the note it read.

Whichever it is, a note still saying that the user started the run goes too
(*A resume is news when it starts*).

### A report covers one run

Each run of an agent has a number — 0 for its launch, one more for every
continuation (`AgentSnapshot::run`) — and a note of a run's end is tagged
with that run: each settle names it (`AgentEvent::Settled::run`), the
offline demo's scripted runs settling the way the live forwarder's do, and
the user's stop names the run it stopped (`Stop::run`). Never the run going
when the loop posts the note: by then the user's message may have started
the next one, whose report would take the note back as its own instead of
handing it back. A report accounts only for the run it read. That matters
because the user's message can start the next run the instant one ends: a
message queued while the agent writes its last reply is continued by the
agent's own thread right after the settle (`docs/queue.md`), so an
`agentoutput` waiting on it sees the agent at work again and reports the run
after, while the first run's end, released by the restart, goes out as an
ordinary notice. A report that took back every unread note of the agent
threw that first answer away — the answer to the task the lead gave — and
taking back only its own run's would deliver the first answer after the
newer one, an older `completed` read last. So the report hands the earlier
answers back: each earlier run's completion still unread on the board is
taken off it, its cell dropped
(`BackgroundRegistry::take_earlier_agent_notices`), and carried at the end
of the report, oldest first, under a line saying it came first:

```text
[Before this run it had ended an earlier one, whose notice had not reached you:]
[background agent] Agent "Build the frontend" (a7k2m9x4q) completed in 2m 5s.
Final response:
…
```

An earlier run whose notice the loop has not posted yet — the loop stalled
for a whole run — stays owed and reaches the lead as a notice. `smoke.sh`
Phase 143 drives the case through the real backend against a stub
provider: the lead waits on a background agent, the user's message is
queued while the agent's final reply is held back, and the wait's result
carries both answers, the newer first.

The edges left: a notice cell that committed at a mid-turn boundary before
the report took its untaken note back stays in the history, so later turns'
context carries the answer twice. And the recorded group entry, which a
taken-back note still updates when its cell settles at the turn's end, can
be left showing the earlier run's outcome in Ctrl+O after a later run's
report updated it — display only; the context replays what the wire
carried.

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
recorded by the live forwarder's `RunRecorder` and settling through the real
registry, so its first wait streams what the agent says before its call —
and `smoke.sh` Phase 130 drives it in the real binary
(`docs/dummy-backend.md`).

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
  command: the streamed lines tail under a `(12s · wait 5m)` clock row and
  `(ctrl+b to stop waiting)`.
- **The roster**: a resumed agent reappears on it (or reopens there, if
  still lingering) with a fresh runtime clock; one the lead stopped turns red
  without the user's `x to clear` row, and lingers the natural 30 seconds.

## Layout

| | |
| --- | --- |
| `src/llm/tools.rs` | the specs, the argument types, the display names and header summaries |
| `src/agents.rs` | the registry (identity, progress, retention, the waiter protocol), the run recorder the forwarder keeps the record with, and the pure report formatting |
| `src/llm/agent_tools.rs` | the executor — the companions over the registry, a continuation spawned through an injected launcher |
| `src/llm/backend.rs` | the forwarder's hold-back, the ids in the launch texts, the routing |
| `src/app/agent.rs`, `src/tui/agent.rs` | the roster side: observed settles, the lead's stop, resume and revival |

[`AGENT_RETAINED_MAX`]: ../src/agents.rs
