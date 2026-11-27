# Background compaction — compacting a turn while it runs

`/compact` and the auto-compact it shared a turn with (`docs/compact.md`) only
ever ran **between** turns: the trigger refused while a turn was in flight, so
one long agentic turn — twenty tool rounds of file reads and test
logs — grew until a request no longer fit and the turn ended on the provider's
error or on `the reply filled the model's context window before it finished —
run /compact to make room`. The idle auto-compact that followed had to send the
same full window, and the work only resumed when the user typed again.

This design compacts **inside** the running turn, **without stalling it**: a
summary is written in the background while the agent keeps working, held, and
swapped into the turn's message list at a round boundary once the context
reaches its ceiling. The agent waits only if the summary is not written by
then.

## Where it comes from

Four harnesses were read for this, each at a pinned commit
(the study's replay and scorecard are in the session that produced it):

- **opencode** (`packages/core/src/session/compaction.ts`, `precompute`) is the
  policy: on by default, on every provider. Within a fixed 60k tokens of a
  ceiling of the window minus 15%, it summarizes the prefix and **holds** the
  summary — swapping it in early would rewrite the prefix the provider has
  cached and give up the verbatim context while it still fits — then swaps it
  in at the ceiling, waiting only if it is unfinished. Its store (an
  append-only compaction row with a keep marker, the context re-derived from
  the latest one) is this crate's design already.
- **codex** (`session/async_compaction.rs`, `compaction_replacement.rs`) is
  the install: its flagged async path keeps everything after the snapshot as
  **native messages, verbatim** (`split_at(prefix.len)`), where opencode keeps
  that stretch only as text with each tool result cut to 1,250 characters — and
  a `cargo test` log puts its failures at the end.
- **hermes-agent** and **Agent Zero** supply the summarizer's input rule: the
  summarizer is sent **the conversation only**, under instructions of its own
  — never the agent's system prompt, never a `<system-reminder>`.

## What the summarizer reads

The conversation, and nothing that every turn re-sends anyway:

- **its own system prompt** (`prompts/compact_system.md`): what the material
  is and that it is material, not instructions — never the agent's persona,
  environment and scratchpad blocks;
- **the conversation before the cut**, derived by
  `context::conversation_messages`: user messages, replies, tool calls and
  results, steered messages, completion notices, an earlier compaction's
  bridge — and **no** `<system-reminder>`: neither the one the window opens on
  (AGENTS.md, the skills and agent listings) nor a hook's `additionalContext`
  note, which is stored wrapped in one;
- **the compact prompt** (`prompts/compact_prompt.md`, codex's) as the last
  user message, unchanged.

Nothing is lost by leaving them out: after the swap the system prompt goes
back on unchanged and the reminder is re-rendered for the turn, as every turn
already does. Leaving them out means the summary cannot restate AGENTS.md or
the environment (opencode's prompt *asks* its summarizer not to; this one
cannot), the persona cannot steer the summarizer into continuing the task, and
a directive buried in a tool output competes with a user message rather than
with the summarizer's own instructions (hermes's argument).

It costs no prompt cache relative to what it replaced: the summarization
request was already sent with no tools, and the tools open the cached prefix
on the major providers, so it was never a cache read. opencode's alternative —
the live request plus one message, tools and all, so the summary *is* a cache
read — was weighed and left: it needs `tool_choice: none` on five wire formats,
some of which have none, and it gives the summarizer everything above.

The same rule applies to `/compact` and to the idle auto-compact.

## The lines on the gauge

For a window `W` (the footer gauge's window — `/v1/models`, the saved
selection or `ALTER_ZERO_CONTEXT_WINDOW`; unknown means no compaction, as
before):

- **The ceiling** — `W − max(15% of W, 16k)` (the 16k floor only for windows
  of 32k and more): 170k on a 200k window, 850k on a million. Past it, the
  next request does not go out until a summary has been swapped in. It leaves
  room for the reply — a reasoning model's thought included — and for one
  round's growth past it. It replaces codex's 90% (`auto_compact_threshold`,
  which the `/context` page's reserve also reads).
- **The start line** — `ceiling − min(60k, (ceiling − cycle_start) / 2)`, where
  `cycle_start` is the context's size right after the last compaction — the
  last marker's `after`, read off the history at every loop bottom, 0 for a
  conversation with none, so a `/clear` or a `/resume` moves it with the
  conversation: 110k on a 200k window. Past it, a background summary
  starts. The lead is fixed in tokens because a summary takes 30–60 s and what
  decides whether it lands in time is how fast the turn grows, not how big the
  window is (opencode's measurement: a few thousand tokens at the median, 25k
  or more one time in twenty); on a small window it is half the room the cycle
  had, so a summary still covers more than the previous one.

`used` is the provider's prompt count for the turn's latest request plus the
token estimate of every message appended since — the footer gauge's rule
(`docs/token-estimate.md`), kept by the turn itself so it is current at the
round top it decides at. Before the turn has a count of its own, the footer
gauge it started from stands in for one (handed over by the boundary as the
turn is spawned, `CompactionSlot::set_baseline`): it counts what the first
request carries, calibrated and with the system prompt and the tool schemas,
where the bare estimate of the list counts neither and read a turn opening
near the start line as tens of thousands of tokens short of it. After a swap
the rebuilt list has no count until its request answers, so it is estimated
whole with the tool schemas' estimate beside it.

## The lifecycle

```
              start line                      ceiling
   agent ─────────┼──── works ──── works ─────────┼──── works ───▶
                  │                               │ swap (or wait)
   summary        └── written in the background ──┘
   (held)                 ready ─ ─ ─ held ─ ─ ─ ─┘
```

1. **Start — at a round top past the start line**, with no job held. The
   check runs in the backend's `round` closure, before the request goes out,
   so the snapshot is at a round boundary: every call of the last round has
   its result, and the pending inputs of this round are already folded in. The
   turn sends `StreamEvent::CompactionStart { id }` and goes on to its request
   without waiting.
2. **The boundary takes the snapshot** where the event lands: the job's
   `start = history.len()`. The channel is FIFO, so every item the turn
   produced before the event is in history — and nothing of the round about to
   start is. It derives the summarizer's input from `history[..start]` and
   starts the
   summary on a worker of its own (`tui::workers::spawn_background_compaction`),
   a tools-free one-off backend streaming into a private channel; the result
   comes back on the loop's compaction channel. A real session never writes
   it on its own backend, which checks the slot at its round tops and would
   wait on itself: with no one-off to hand the summary counts as failed. The
   compact hooks run for it as for any compaction on its own (`trigger:
   auto`, `docs/hooks.md`); their cells are held and committed above the
   summary's own when it lands.
3. **Held.** The summary is not applied when it lands: the slot marks it
   ready, the strip's row says so, and the turn keeps working on its full
   list.
4. **Swap — at the first round top past the ceiling.** The turn sends
   `StreamEvent::CompactionSwap { id }` and waits for the boundary's answer.
   The boundary appends `HistoryItem::Compaction { kept, background: true, …
   }` — `kept = history.len() − start`, what ran while the summary was written
   — commits the `● Context compacted · 170k → 82k tokens · 47s ·
   background` cell, re-derives the context from history exactly as the next
   turn's start would, and hands it back through the slot. The turn rebuilds
   its message list from it — the system prompt unchanged, the reminder
   re-rendered, the kept user messages, the summary's bridge, and every item
   since the job's start as it was recorded, provider call ids included — and
   sends its request. The answer takes one loop iteration: milliseconds.
5. **Wait — at the ceiling with the summary unwritten.** The turn sends
   `StreamEvent::CompactionWaiting` once (the status verb becomes the fixed
   `Compacting…`) and polls the slot and its cancel token every 20 ms until the
   summary lands, then swaps. With no job at all (the turn jumped from below
   the start line past the ceiling in one round), it starts one and waits for
   it — today's auto-compact, mid-turn.

Re-deriving rather than splicing is the point of the handshake: the list the
turn continues with is, by construction, the list the next turn derives, so
the prompt cache's prefix carries across the turn boundary (`WireHistory`), and
no index in the turn's own list has to be mapped onto history.

## The marker

`Compaction` gains:

- **`kept: usize`** — how many items just before the marker its summary
  leaves out: the ones recorded while it was written. The derivation becomes:
  the kept user texts and the bridge for everything before those `kept`
  items, then the `kept` items and everything after the marker derived
  normally (the marker itself derives nothing). `0` is the old rule —
  everything before the marker — so a rollout written before this parses and
  derives exactly as it did. A count larger than the history before the
  marker keeps all of it.
- **`background: bool`** — the cell's `· background` tag in place of `· auto`.

Both persist in the rollout's `compaction` record with serde defaults, left
off the line at their defaults. The tail is a count back from the marker
rather than an index into the history. Inside a session either would do —
history is append-only, a backtrack only ever cuts a suffix, so a marker that
survives one keeps its tail — but the rollout's reader skips a record type it
does not know (an older build reading a newer file), and an item skipped
anywhere in the long prefix would move an index, where it moves a count only
when the skipped item is in the short tail itself.

## Between turns

- **A summary still running when the turn ends** finishes in the background
  and is held like one started mid-turn.
- **A summary held at the idle loop bottom is applied there**: the marker is
  appended, the cell committed, and the next turn starts compacted. Between
  turns there is no request in flight whose cached prefix it would cost.
- **The idle start** replaces the blocking auto-compact turn: past the start
  line with nothing running, the loop bottom starts a background summary
  instead of a `Compacting…` turn, so a message sent meanwhile dispatches at
  once — onto the full context, which is still under the ceiling.
- **A turn that starts while one is running** adopts it: its first round top
  past the ceiling waits for it and swaps it in.

## Failures and edges

- **A summary that fails** frees the slot, raises a red `Background
  compaction failed — {reason}` toast, and the next round top past the start
  line starts another; after **two** failures in a cycle no background start
  is tried until a compaction lands (opencode's cap). At the ceiling the turn
  then goes out uncompacted, as it did before this change, and meets the
  provider's limit the way it always has.
- **The history moved** — the conversation replaced or rewound (`/clear`, a
  `/resume`, a backtrack: `App::history_epoch`), or an item the summary
  covers taken back (the turn that started it undone before anything
  streamed): the job is cancelled and forgotten at the next loop bottom
  (codex's generation check, narrowed). The epoch is not
  `App::history_generation`, which also moves for changes that leave what
  was said alone — a re-render, an agent's entry updated in place, a failed
  attempt withdrawn — and forgetting a summary on those would throw away one
  already paid for. The slot is let go of the moment the epoch moves, so
  nothing of the last conversation's cycle — its failures, a landing past the
  ceiling — carries into the next. A `/compact` by hand lets the job go for
  its own, as does
  turning **Auto compact** off or a window becoming unknown. A `/model`
  switch keeps it: the summary is the conversation's whichever model wrote
  it, and the lines move with the new window.
- **A start the loop never heard of** — an Esc swaps the reply channel, and a
  `CompactionStart` still in it goes with it: the slot holds a running job no
  worker writes, and nothing would ever free it. The idle loop bottom lets go
  of a job the app has no record of (`CompactionSlot::drop_unknown`), and the
  round top of a turn already cancelled starts nothing.
- **A compaction that could not help** — one that leaves the context still
  at or past the ceiling, by the gauge's count or by the turn's own at its
  next round top. Once, that is a tail kept verbatim that is itself huge (a
  few enormous reads while the summary was written), and summarizing again —
  the tail included this time — brings the context down: the next round top
  starts that summary at once and waits for it. Twice in a turn, what rides
  every request alone — the system prompt and the tool schemas — passes the
  ceiling, and summarizing again would land in the same place for ever, so
  none is started for the rest of the turn (`MAX_PAST_LANDINGS`,
  `CompactionSlot::futile`); the turn goes on uncompacted, as before this
  change, and the next turn tries again (`CompactionSlot::turn_started`,
  called by the turn's own thread). Between turns the loop bottom starts none
  after a landing past the ceiling — with no turn waiting on it, it could only
  land there again; the next turn refolds instead. This is the old trigger's
  one attempt per turn, kept for the case it existed for, with the one retry
  a huge tail needs.
- **Billing**: a summary's usage frames join the since-launch ledger on the
  `/context` page, whatever became of it, and never the gauge or the last
  request, its request being the summarizer's. The rollout records each as a
  `compact` usage line where the summary finished (`docs/rollout-records.md`).
- **A summary the output cap cut off** goes on in another round, as any
  reply does (`docs/unfinished-replies.md`); a connection lost in a round
  takes back only what that round's attempt streamed
  (`compaction::SummaryStream`, `App::track_attempt`'s rule).
- **Esc** stops the turn, not the summary: it stays held, and is applied at
  the idle loop bottom if the history it covers is unchanged.
- **Rate limits**: the summary is a second request on the same provider while
  the turn makes its own; its retries back off on the same budget.
- **Headless** (`-p`) runs the same loop, so a long `-p` task compacts too.
- **Subagents** compact too, on their own threads — the next section.

## Subagents

A subagent runs its own loop over its own context, and an `explore` agent
reading a codebase is the workload that fills a window fastest. Without
compaction one that outgrew its window ended on the provider's error, and on
Ollama, which drops a too-long prompt from the front without a word, it lost
its system prompt and its task. It now compacts by the same lines and the
same lifecycle as the lead, one level down
(`src/llm/compaction.rs`, `AgentCompaction`).

**Where it runs.** The lead hands its swap to the boundary because its
conversation of record is its history, from which the boundary re-derives
the list the turn goes on with. A subagent's conversation of record is its
own message list — what its thread sends every round, and what the registry
stores for a chat continuation — and only its thread holds it. So the whole
job runs there: the round top decides with the lead's `decide`, a summary
starts on a thread of its own (`spawn_agent_summary`, a tools-free run of
the agent loop, so an empty or cut-off summary is asked to go on, folded by
`SummaryStream`), and at the ceiling the agent **splices its own list**.
Nothing is derived twice, so nothing can drift: the list the agent goes on
with is the list the registry stores.

**The window** is the one its session view gauges against
(`docs/agent-context-gauge.md`): the session's for a type that inherits the
model, and for a type pinned to another model only the
`ALTER_ZERO_CONTEXT_WINDOW` override, since the listing is read for the
selected model alone. The boundary configures both on the shared slot at
every loop bottom (`CompactionSlot::configure_pinned`, `agent_window`), off
with **Auto compact** like the lead's, and the agent reads its own at every
round top. Its measure is the lead's `PromptMeter`: the provider's count of
its last request plus the estimate of what joined since, its tool schemas
beside a list it estimates whole.

**What the summarizer reads** is the lead's rule over the agent's list
(`summarizer_input`): its own system prompt, the conversation, the compact
prompt — never the agent's system prompt, never a `<system-reminder>` (its
briefing, a hook's added context).

**The splice** (`spliced`) keeps the agent's system prompt and its briefing,
then the user's texts the 20k budget keeps — its task and the messages the
user typed into its session; not a `[system]` note the app spoke, a reminder
or an earlier bridge, the lead's typed-only rule read off the wire
(`context::kept_user_texts`) — then the summary's bridge, then every message
from the summary's start on verbatim. A continuation over a list a summary
was spliced into resumes its cycle at its first round top
(`AgentCompaction::for_list`), so a list compacted moments ago is not
summarized again at once.

**What its transcript records.** The agent's thread tells its own stream,
so every event lands in order with everything else the agent did:
`CompactionStart` (the transcript notes its length there, the lead's
snapshot one level down), `CompactionWaiting` (its session view's status
line wears `Compacting…`), `CompactionLanded` — the marker, keeping what was
recorded since the start, so the session view commits the `● Context
compacted · … · background` cell in place and the agent's gauge and Ctrl+D
derive the compacted shape — and `CompactionUsage`, the summary's frames:
billed, into the session's and the agent's ledgers and the rollout's `usage`
lines under the agent with `compact`, never its gauge or last request.

**Failures and edges** are the lead's: two failed summaries stop starts
until one lands, a landing past the ceiling is summarized once more and a
second spends the run, a cancelled run starts nothing and stops waiting, and
a run that ends mid-summary stops its summary's thread (billing it if it had
finished). A subagent's summary runs no compact hooks: `PreCompact` and
`PostCompact` are the lead's conversation's events.

## What the user sees

- While a summary is being written: a dim row under the status line,
  `⎿ Compacting in the background · 12s`; once written, `⎿ Summary ready ·
  swaps in at 85%`. The spinner keeps its verb — the agent is still working.
- Inside a subagent's session view, the same for that agent: `Compacting…`
  while it waits, the cell in its transcript, its gauge dropping, and on its
  `/context` page the hatched reserve at its own ceiling.
- At the swap: the `● Context compacted · 170k → 82k tokens · 47s ·
  background` cell, in place in the transcript, and the gauge drops. Its
  `before` is the context at the swap — the turn kept working while the
  summary was written — and its elapsed the summary's own.
- Waiting at the ceiling: the status verb `Compacting…` until the swap.

## Files

- `src/compaction.rs` — the pure policy: `ceiling`, `start_line`, the
  decision at a round top, and the `CompactionSlot` the turn and the boundary
  share (the `SteerQueue` shape), the subagents' windows on it.
- `src/llm/compaction.rs` — a subagent's: `summarizer_input`, `spliced` and
  `AgentCompaction`; its thread's half in `src/llm/backend.rs`
  (`spawn_subagent_run`, `spawn_agent_summary`); its transcript's in
  `src/agents.rs` (`AgentRun::apply`, `live_verb`).
- `src/context.rs` — `conversation_messages` (the summarizer's input) and the
  `kept` derivation; `src/session.rs` — the record's new fields.
- `src/llm/backend.rs` — the round-top step in the `round` closure.
- `src/stream/event.rs` — `CompactionStart`, `CompactionWaiting`,
  `CompactionSwap`.
- `src/app/compact.rs` — the job's state, the marker, the idle start.
- `src/tui/compaction.rs` — the start, the worker's answer, the swap, the
  loop-bottom tick; `src/tui/workers.rs` — the worker
  (`spawn_background_compaction`); the event arms in `src/tui/stream.rs`,
  the channel in `src/tui/event_loop.rs` and the headless loop.
- `src/ui/compaction_row.rs` — the row under the status line.
- `prompts/compact_system.md` — the summarizer's system prompt.
- `scripts/smoke/phases/051-autocompact.sh` (between turns),
  `170-compactinput.sh` (what the summarizer reads),
  `171-midturncompact.sh` (inside a running turn, against a stub provider:
  the start, the wait, the swap, the verbatim tail, the cell in place) and
  `172-backgroundcompact.sh` (a summary that lands while the turn reads on, one
  that lands at idle on its own, and one a `/clear` lets go of) and
  `173-agentcompact.sh` (a subagent's, watched from its session view).
- `tests/headless.rs` — `a_long_turn_is_compacted_in_the_background_as_it_runs`:
  the request the swap built is the one the next turn derives, message for
  message; `a_subagent_that_outgrows_its_window_compacts_and_finishes`.
