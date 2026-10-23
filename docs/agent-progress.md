# A subagent's progress updates — on its row, and in `agentoutput`

Date: 2026-10-22

## The request

> add "Agent: ..." to show the agent intermediate response or progress
> update
>
> ```
> ● Agent(Gather GitHub contexts for user linuztx)
>   ⎿  Agent: ... <-like Bash: ... or Edit: ... something like that
> ```
>
> Also for the agentoutput tool, include the agent intermediate response or
> progress update to TUI and context so the main agent has full context
> awareness

## What was missing

A subagent that narrates — `Let me fetch the repositories next.`, `The
profile lists 42 public repos; checking the events now.` — said those words
to nobody but its own session view.

- **The activity row named calls only.** The sticky row under a live agent
  (`docs/agent-tool.md`) is built from `AgentRun::last_call`, so while the
  agent was writing it showed the call before (`⎿ Bash: Fetch the profile`)
  or `⎿ Initializing…`, however much it had said since.
- **`agentoutput` dropped the words on the floor.** The subagent's forwarder
  (`llm::backend::spawn_subagent_run`) accumulates the reply text to know the
  run's final answer, and every tool event **cleared** it: correct for the
  answer, but the text it cleared went nowhere. So a lead polling a
  long-running agent read `Bash(curl …)` lines and nothing of what the agent
  had found or decided — and its only remedy was to wait for the final
  response, or to `agentsend` a question and wait for that.

## What counts as a progress update

A **run of the agent's reply text that more work followed**: the text before
a tool round, before a message it read mid-run, before a stop hook's
continuation (`docs/hooks.md`). Exactly the points at which the forwarder
used to clear the text, and for exactly the reason it did — what came before
them is not the answer. The run of text that nothing follows **is** the
answer, reported as it always was (`Final response:`), and is never an
update. A thinking phase is not one either: its chain-of-thought is the
model's working, not something it said (`docs/thinking-stream.md`).

## The activity row: `Agent: {update}`

```
● Agent(Gather GitHub contexts for user linuztx)
  ⎿  Agent: The profile lists 42 public repos; checking the events now.
```

The row's one grammar (`{Name}: {detail}`, `agents::activity_line`) gains an
actor: the agent itself, under `agents::AGENT_UPDATE_LABEL`. It shows

- **the newest paragraph** of what the agent is saying — the lines after the
  run's last blank line, on one line with every whitespace run one space,
  and clipped at the width like every activity row. Not the whole run: a
  run's opening is stale by the time it says *now let me…*, and it is the
  newest intent the row is for. Not the newest line: a list or a wrapped
  sentence would show a fragment. Within one paragraph the row is
  **prefix-stable** — the clip keeps the head — so once a paragraph is wider
  than the row it stops changing rather than ticking along under the user's
  eyes;
- **while the text streams**, read off `AgentRun::streaming` at each frame;
- **and after it is finalised**, held in `AgentRun::last_update` — through a
  batch's permission prompt, say, between the text and the call — until the
  **next call starts**, which clears it. The sticky rule, made two-sided:
  the row shows the newest thing the agent did, a call or something it said,
  and holds it between them instead of dropping to `Working…`.

**No frame reads the run again.** The row is built several times a frame
while the text streams, and a run can be anything: a table or a code block
thousands of lines long, a URL or a blob with no space in it, a page of
blank space. So `AgentRun` notes, as each chunk arrives, where the newest
paragraph starts and whether the line being written has anything on it yet
(`ParagraphStart`, nothing before the chunk read again, a line split across
chunks — CRLF endings included — landing where a whole-text reading puts
it), and the row reads only that paragraph's head: at most
`ACTIVITY_UPDATE_MAX_CHARS` (512) characters, from no more than four times
as many, which leaves room for any indentation or table padding and stops a
blob inside itself and a blank run short of its end. The first version
found the paragraph again from the end of the text each time, so a
paragraph thousands of lines long was walked every frame and a blob with no
space in it was copied whole. The second still trimmed the line being
written on every frame to learn whether it had opened a paragraph, so a line
ending in four megabytes of spaces cost 24 ms a row; it now costs a few
microseconds.

The transcript keeps the text as it came; the row's one-liner is derived
from it, so nothing about the row's wording reaches `history`, the session
view or the rollout. The row exists where every activity row does — the live
group cell, lone or tree, while a foreground group runs. A background agent's
words are read with `agentoutput` below, or in its session view, which always
showed them.

## The record: `ProgressUpdate`

The registry slot keeps the agent's updates beside its calls and the messages
it read (`AgentSlot::updates`, `AgentSnapshot::updates`), each a
`ProgressUpdate { at, read, text }`:

- `at` — how many calls it had made when it said it, the `FollowUp` rule;
- `read` — how many messages it had read by then, which places it among the
  marks at the same `at` (*Where a mark is placed*, below);
- `text` — trimmed, and kept **whole up to
  `AGENT_REPORT_UPDATE_CHARS`** (2,000 characters), cut there with `…`. A
  blank run is no update and is not recorded.

`AgentRegistry::record_update` is the door. The forwarder reaches it through
`agents::RunRecorder`, which now owns the bookkeeping the forwarder did
inline — the text since the agent last did anything else, the call record at
each `ToolStart`, and now the update at each point that ends a run of text
before more work — so the rule is unit-tested rather than living in a
thread body. The forwarder keeps only the outcome and the held terminal event
(`docs/agent-tools.md` *One notice per answer*).

## The report

`agentoutput`'s report (and `agentkill`'s, which is the same report) lists
the agent's timeline — every call, every message it read, every update it
gave — in the order it happened:

```text
Running (agent a7k2m9x4q · Gather GitHub contexts for user linuztx · 42s · 2 tool uses)
Agent: I'll start with the profile, then the repositories.
Bash(curl -s https://api.github.com/users/linuztx)
Agent: The profile lists 42 public repos and 100 followers.
  Fetching the repositories next, newest first.
Bash(curl -s "https://api.github.com/users/linuztx/repos?per_page=100&sort=updated")

[Still running — you are notified when it finishes; agentoutput with `wait` waits for it.]
```

An update is `Agent: ` over its first line with the rest indented two spaces
beneath it — the user-messages note's shape — and its blank lines dropped,
since a blank line in a report is a section break. It is listed **whole**
where a call is one line cut at 160 characters, and that difference is the
point: a call line summarizes an action whose output the lead deliberately
never pays for, while an update is the agent's own account of what it found
and what it is doing — the content the lead most needs, and a few sentences
in practice. The 2,000-character cap is the bound on a pathological one, not
the expected size.

The window is unchanged: the newest 30 calls are listed, the earlier ones
counted, and what happened among the hidden calls — the messages read there
and now the updates given there — is hidden with them rather than shown out
of place.

## The wait stream

A waiting `agentoutput` streams what the agent does while it is watched
(`docs/agent-tools.md`), and now that includes what it says: each update is
streamed as one line, `Agent: {flattened}`, cut like a call (`report_call`),
and each message it reads as the mark the report gives it. The cell's tail
is a four-row window, and a whole update there would push the calls it sits
among out of it; the settled cell's output is the report, updates whole.

The wait polls every 50 ms, so what one poll costs matters on a long wait
over a busy agent. A poll is `AgentRegistry::progress(id, &mut cursor)`:
under the registry's lock it reads only the entries past a
`ProgressCursor` — how many calls, messages and updates the cell has shown —
formats those, and moves the cursor past them. Nothing the cell has shown is
copied again, where the first version took a `snapshot` per poll and copied
the agent's whole history, updates and all, twenty times a second. The tail
it reads is the report's own: an entry recorded later never sorts before
one recorded earlier, since the counts it is placed by can only have grown,
so the lines past the cursor are the newest of the timeline, in its order,
and the stream and the report the wait ends on cannot disagree.

## What stays as it was

- **The completion notice and the foreground result** still carry the final
  response and nothing before it. A notice is an answer the lead did not ask
  for at that moment; the updates are there for the lead that asks.
- **The Ctrl+O agent cell** keeps its prompt, call headers and response; the
  agent's session view is where its whole conversation is read.
- **The other offline demos**: their scripted agents do not narrate, so
  their reports read as before (`smoke.sh` Phase 98). The follow-up demo's
  agent does (*Testing*, below).

## Where a mark is placed

A message the agent reads is taken off its queue by its own loop at a round
boundary (`AgentRegistry::take_pending_inputs`), on the agent's thread. Its
calls and updates are recorded by the forwarder as it relays the stream, on
another — and the forwarder can run behind the loop: a call that returns in
microseconds, a machine under load. Every entry is keyed by the counts it
reads when it is recorded, so a mark recorded at the drain could be keyed by
counts the forwarder had not caught up to, and the report listed it ahead of
a call, or of something the agent said, that came before it. The race
predates the updates, which only gave a mark one more thing to land ahead of.

So the drain only notes what each mark says — one for the lead's messages,
each of the user's quoted — and the mark is placed where the stream
announces the message. The loop sends one `StreamEvent::Steered` per message
it took, right after taking them, and nothing else sends one on a subagent's
stream; the forwarder's `RunRecorder`, relaying the first of a drain's
announcements, records what the agent said before it and then places the
drain's marks after the calls it has relayed (`AgentRegistry::announce_read`).
Calls, updates and marks are all recorded on one thread, in the stream's
order. Drains queue up in order, since the loop can drain again before the
forwarder reaches the first drain's announcement, and each is placed at its
own. A drain no announcement reaches — a run cut short between the two — is
placed when the run settles or starts again, late rather than never; and
`AgentRegistry::user_messages` counts the user's messages from the drain,
since a stop can be reported before the forwarder relays them.

## Testing

- `agents` (unit): the row shows the newest paragraph while the text streams,
  holds it once the text is finalised, and yields to the next call; a blank
  run changes nothing; a settled agent still reads `Done`; a paragraph
  ten thousand words long, a run with no space in it and a page of blank
  space are each read only up to the bound, and a row over a line ending in
  four megabytes of spaces is built inside a millisecond (a wall-clock gate,
  skipped by the AUR package's `check()` with the other frame budgets,
  `docs/aur.md`); the paragraph is found where a whole-text reading puts it
  after every chunk of every text of up to six characters drawn from a
  letter, a space, a CR and a line break, cut every way it can be, and in a
  hand-written stream of CRLF endings and words split across chunks. The
  report places updates among calls and
  marks by `at`/`read`, renders a multi-line update indented with its blank
  lines dropped, and hides those among the hidden calls;
  `AgentRegistry::progress` hands a wait only what is new, one line each, in
  order, marks included. `record_update` trims, cuts at the cap and skips a
  blank run. `RunRecorder` records an update at every point that ends a run
  of text before more work — and ahead of the call a `ToolStart` records —
  and keeps the answer's text whole. A drain the forwarder relays after a
  call it had not yet recorded is marked after that call and after what the
  agent said before it; two drains in flight are each marked at their own
  announcement; a drain never announced is marked when the run ends, its
  user message counted from the drain.
- `ui` (unit): the lone and tree cells render `⎿  Agent: …` dim and clipped.
- `llm::agent_tools` (unit): a report hands the lead the updates, and a wait
  streams an update the agent gives while it is watched, in its place among
  the calls, with the mark of a message it reads meanwhile.
- `stream::dummy` (unit) and `scripts/smoke.sh` **Phase 130**: the offline
  follow-up demo's agent says what it is about to do before its call,
  recorded through the same `RunRecorder` the live forwarder keeps, so its
  first wait streams `Agent: I'll read the top of the changelog first.` over
  the call and its report lists both, with no network.
- `scripts/smoke.sh` **Phase 144**: the real backend against a stub provider.
  A foreground agent's reply is held mid-stream so the lead's live cell can be
  read showing `⎿  Agent: …`; then the lead asks `agentoutput`, and the
  report — in the lead's transcript and in the request the stub receives —
  carries both of the agent's updates in their places among its calls.
