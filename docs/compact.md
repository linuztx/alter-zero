# /compact — summarize the conversation to free context

Compaction replaces what the model is sent with a **summary** of it: the most
recent user messages (token-budgeted) plus a `SUMMARY_PREFIX`-tagged bridge
message carrying a handoff the model can continue from. The idea and the
compacted shape are codex's (its local "Memento" compaction,
`codex-rs/core/src/compact.rs`). The visible transcript is untouched — a cyan
`● Context compacted` cell marks the spot, exactly like codex's info cell.

It happens three ways, and all three leave the same marker:

- **while the agent works** — a turn that calls tools re-sends its whole
  conversation every round, so a long one fills the window long before it
  ends. The agent loop has a summary written in the background and swaps it
  in between two rounds, as often as the turn needs (*Compacting while the
  agent works*, below). This is the one that matters most, and the one that
  was missing: compaction used to run only between turns, so a single long
  turn grew past its model's window and died there;
- **`/compact`** — the user asks for it. Idle, it runs as a turn of its own
  (*The compact turn*); typed while a turn runs it is taken at the agent's
  next step;
- **between turns** — the loop bottom starts the same turn when an idle
  session sits past the threshold (*Auto-compact + the context gauge*).

```
❯ a long task …
● Bash(cargo test)
  ⎿  …

⣀⣀⣶⣀⣀⣀⣀⣀ Hatching… (2m 4s · ↓ 41.2k tokens · compacting context 6s · esc to interrupt)
                                                  ↑ a summary is being written; the agent works on

● Read(src/parser.rs)
  ⎿  Read 412 lines

● Context compacted · 54.6k → 28.2k tokens · 5s · auto         ← where the swap happened

● Bash(cargo test parser)                                       ← the same turn, going on
```

A `/compact` turn is **not steerable** (`docs/queue.md`): its request is the
fixed handoff prompt over the context being summarized, not a conversation, so
a draft submitted while it runs queues as a follow-up turn — which is what it
was always going to be — instead of being folded into the summarization
request.

## The append-only design (why history is never rewritten)

Codex keeps **two** histories: the TUI transcript stays intact while the
model-facing session history is replaced with the compacted shape (and the
rollout file records everything plus a `Compacted` item; resume derives the
working set from the newest such record). This TUI has **one**
`App::history` feeding the renderer, the recorder, the checkpoint keys, the
Esc-Esc backtrack, *and* the per-turn context derivation
(`context::context_messages`). Physically replacing it would lose the visible
transcript, force a rollout rewrite that drops the pre-compaction turns,
and invalidate every length-keyed checkpoint.

So compaction is **append-only**: `/compact`'s turn ends by *appending* a
`HistoryItem::Compaction { summary, timestamp }` marker, and the replacement
happens at **derivation time** — `context_messages` finds the *last* marker and
emits, in place of everything before it:

1. the pre-marker **typed user messages** (`Role::User` text only — images,
   tool records, shell transcripts, and notices drop, codex-parity), walked
   newest→oldest under a `COMPACT_USER_MESSAGE_MAX_TOKENS` (20 000) budget:
   whole messages are kept while they fit, the first overflowing one is
   **middle-truncated** to the remaining budget (codex's
   `truncate_middle_with_token_budget` — head + `…{n} tokens truncated…` +
   tail), then the selection is re-reversed to chronological order. Tokens are
   codex's `bytes/4` approximation (`approx_token_count`), not the
   estimator's seam — the walk runs at every turn start and must be O(len);
2. the **bridge**: `{SUMMARY_PREFIX}\n{summary}` as a user message (an empty
   summary becomes `(no summary available)`, codex's fallback — which a live
   compaction no longer reaches: the summarization turn runs on the agent
   loop, which asks again for a summary that came back empty, and a
   compaction that never gets one fails like any other, leaving the context
   as it was (`docs/unfinished-replies.md`); the fallback still covers a
   marker recorded before that). The prefix is
   codex's verbatim `prompts/compact_summary_prefix.md` ("Another language
   model started to solve this problem…");

then derives the items *after* the marker normally. Earlier markers (a second
`/compact`) sit before the last one and are skipped — only real `Role::User`
texts are collected, so a prior compaction's summary is structurally excluded
(codex needs an `is_summary_message` prefix check because its summaries are
user messages; ours live in the marker).

A marker made **while the agent worked** carries three more facts, all
defaulting to what a marker made at rest means, so an old rollout reads as it
always did:

- **`kept`** — how many of the items recorded just before the marker the
  summary does **not** cover. The summarizer reads the conversation as it
  stood when it was started, and the agent goes on working while it writes;
  what was done meanwhile is not in the summary, so it must stay in the
  context word for word. The derivation takes `covered = position − kept`:
  the user's messages are collected from `history[..covered]`, the bridge
  stands for exactly that, and `history[covered..]` — the kept items, then
  everything after the marker — derives normally. The count is relative to
  the marker, so no index is stored that a later edit could invalidate;
- **`resume`** — the bridge closes on `prompts/compact_resume.md` (*the task
  was still in progress… carry it on… any messages after this one are the
  most recent steps, exactly as they happened*). A summary read at rest is
  followed by the user's next message; one read mid-turn is followed by tool
  results, and a model that takes the handoff for the start of a new
  conversation greets the user instead of carrying on;
- **`budget`** — the budget the kept user messages were cut to. Twenty
  thousand tokens of the user's own messages is codex's figure and assumes a
  large window; against a small one it would leave no room for the work, so
  it scales with the window (`compaction::user_message_budget`: an eighth of
  it, between 500 and 20 000), and the marker remembers which, because the
  same history must derive the same context after a `/model` switch.

Everything downstream holds with zero remapping:

- the **recorder** sees a plain append (no rewrite, no transcript loss; the
  marker is a new `compaction` rollout line, `session.rs`);
- **checkpoints** stay valid — `after` counts only ever grow;
- **Esc-Esc backtrack** to a pre-compaction user message truncates the marker
  away and the context reverts to the full conversation — codex's
  rollback-past-compaction semantics for free;
- **`/resume`** parses the marker back and the derivation re-applies;
- **Ctrl+D** shows the compacted context automatically (it renders
  `context_messages` fresh);
- the **Ctrl+O transcript** shows the marker cell with the summary text under
  it (dim, indented) — unlike the inline cell, which is just the one line.

## Compacting while the agent works

The reported bug: *auto compact doesn't start until the agent has finished,
even with the context window exceeded.* The trigger lived at the loop bottom
and required an idle session, and the agent loop had no way to shrink the
conversation it was sending — so one long tool-calling turn grew past the
window and the provider refused it. The fix is not a better trigger; it is
moving compaction **inside** the loop.

### The seam

`llm::agent::run_agent_compacting` consults a `llm::compaction::Compaction`
at three points (`run_agent` is the same loop with `NoCompaction`):

| When | Call | What it may do |
| --- | --- | --- |
| before every request, after the round's pending inputs are in | `before_round(&mut messages)` | start a summary, swap one in, or wait for one |
| a request was refused for its size | `make_room(&mut messages)` | compact now and have the request sent again |
| the model answered with no tool calls | `before_done(&mut messages)` | land a summary still in flight; take a `/compact` typed too late for a round |

`messages` is the loop's own running list, so a swap changes what the next
request carries and nothing else has to be told. The implementation a real
turn runs with is `RollingCompactor`; its summarizer is a closure handed in
(`Summarize`), so the whole state machine is tested with fakes, no network
and no clock but the one that times a summary.

### The two thresholds

`compaction::soft_threshold` is **80%** of the window and
`compaction::hard_threshold` **90%** (codex's figure, and the between-turns
trigger's).

- Past **soft**, with no summary already being written and something to gain
  (below), `before_round` starts one **on a thread of its own** and returns at
  once: `StreamEvent::CompactionStarted`. The agent's request goes out as
  usual. This is the whole point — a summary takes seconds to tens of
  seconds, and the agent does not stop for it.
- At each later round boundary the compactor looks at the job. A summary
  that has **landed** is swapped in before the request is built.
- Past **hard** with a summary still being written, the loop **waits** for it
  (`StreamEvent::CompactionWaiting`) rather than send a request that may not
  fit — polling the turn's `CancelToken` every 20 ms, so Esc is honoured as
  everywhere else. A context that jumps from under soft to over hard in one
  round (one big tool result) starts the summary and waits for it in the same
  call.

The load is the session's own gauge, read on the backend's thread
(`LoadMeter` over the shared `tokens::ContextMeter`): the last usage frame's
`input` plus the calibrated estimate of what joined since, the tool schemas
among it — so the footer and the engine can never disagree about how full the
window is.

### The swap

When a summary lands, everything the summarizer read is replaced and
everything it did not read is kept:

```
before   [system] [user…] [assistant+calls] [tool] … [assistant+calls] [tool] [assistant+calls] [tool]
                  └──────────── what the summarizer read ─────────────┘ └─ made meanwhile ─┘

after    [system] [user: reminder + the user's recent messages + bridge]  [assistant+calls] [tool]
                  └────────── `compacted_prefix` ─────────────────────┘  └── the tail, verbatim ──┘
```

The cut always falls on a round boundary — a summary is started from
`before_round`, where every call of the previous round has its result — so
the tail opens on an assistant message and no tool result is ever orphaned
from its call. `StreamEvent::Compacted { summary, secs, budget }` tells the
app, the meter is rebased on the new messages, and the loop sends its next
request.

### No limit

There is no count to run out. A turn compacts as often as its context fills:
each summary reads the bridge the one before it left (the prompt says to
carry forward what still matters), so the handoff is rolled forward rather
than lost. What stops a pointless compaction is a check, not a counter:

- **worth it** (`RollingCompactor::worth_compacting`) — what the summary
  would replace must exceed what it would leave (the reminder, the kept user
  messages, a summary's worth) by `compaction::min_gain` (a twentieth of the
  window, at least 500 tokens). A context that is all tail — one enormous
  result just read — is not summarized again and again for nothing;
- **failure** — a summarization that fails is said once (a toast:
  `Context not compacted: …`), the context stands as it was, and the turn
  goes on. The next is not started for three rounds, and after three failures
  in a row the turn stops starting them by itself (a request that then
  overflows still gets its try);
- **a summary request that itself overflows** is asked for again with half
  the transcript, up to three times, before it counts as a failure.

### What the user sees

Nothing commits until a summary lands. While one is being written the status
line carries a clause in the accent colour, `· compacting context 6s`, between
the token tally and the interrupt hint; once the loop is *waiting* on it the
verb itself becomes `Compacting context…` (the lost-connection verb outranks
it). When it lands, the `● Context compacted · 54.6k → 28.2k tokens · 5s ·
auto` cell commits **where the swap happened** — under everything the agent
did while it was being written, above what it does next — and the footer
gauge drops. Inside a subagent's session view the same cell lands on that
agent's own transcript.

`before` is the gauge as the summary landed (not as it started: the agent
kept working), `after` the gauge once the swap was made.

### `/compact` while a turn runs

It used to be refused with a toast. Now `Action::CompactWhenReady` sets a
flag on the shared `CompactionControl` and toasts `Compacting at the agent's
next step`; the lead's compactor takes it at its next round boundary — held
only to *having something a summary could stand in for*, never to the
automatic "worth it" bar, since the user asked — or at `before_done` when the
turn was already writing its last reply. A second `/compact` while one is
being written says `Already compacting`. A backend with no engine (the
offline dummy) leaves the flag standing, and the loop bottom runs the
ordinary compact turn as soon as the session is idle
(`Session::run_pending_compact`), so the command is never dropped.
`/clear` forgets it.

### A turn does not end under a summary still in flight

`before_done` waits for a running summary and lands it before `StreamDone`.
Throwing it away would waste a request already paid for, and the turn after
would start over the threshold and compact again at once.

### A request that does not fit

Belt and braces for the cases the thresholds cannot see — a window the
provider enforces below the one it reports, an estimate that ran low, a
session that never learned its window:

- a provider's refusal is recognised by `llm::compaction::is_context_overflow`
  (an API error whose text carries one of the phrases providers use —
  `context_length_exceeded`, `prompt is too long`, `maximum context length`,
  … — and never a 401, 403 or 429), and a reply cut off because the *window*
  filled by each wire's `StopReason::ContextWindow`;
- either becomes `RoundOutcome::Overflow`, which the loop answers with
  `make_room`: wait for the summary being written, or write one now, swap it
  in, and **send the request again** — or, when the reply had started,
  continue it under the `Unfinished::WindowFull` note;
- twice in a row at most (`MAX_OVERFLOW_RECOVERIES`); with **Auto compact**
  off, or nothing to gain, the turn ends on the provider's own error, as it
  always did.

### Subagents

A subagent runs the same loop over its own conversation, so it gets its own
`RollingCompactor` — against the window of the model *it* runs on
(`CompactionControl::window_for`: the session's own when it inherits the
session's model, while an agent pinned to another model has only
`ALTER_ZERO_CONTEXT_WINDOW`, the footer's rule in
`docs/agent-context-gauge.md`, and without it compacts when a request is
refused for its size rather than ahead of it), with a seed built from its own messages
(`seed_from_messages`: its briefing, its task, the messages sent to it since)
and with `takes_requests` off, since a `/compact` is the lead's. The events
ride its agent channel and fold onto its own transcript (`AgentRun::apply`).

### Hooks, usage, the rollout

- `PreCompact` and `PostCompact` fire for a mid-turn compaction exactly as
  for the command (`HookSink::for_compaction` → `CompactHooks`), with
  `trigger: "auto"` or `"manual"`; a `PreCompact` hook's context is added to
  the summarizer's instructions.
- What a summary cost is billed in the turn that asked for it
  (`StreamEvent::CompactionUsage` → the turn's tally and a `usage` rollout
  line marked `compact`), and is **never** read as a measure of the
  conversation: the gauge is rebased from the swapped messages instead.
- The marker is the `compaction` rollout line it always was, with `kept`,
  `resume` and `budget` beside the summary, left out at their defaults.

## The compacted shape, on both sides

The same compaction is described twice: once as the **messages** the running
turn goes on to send (`llm::compaction::compacted_prefix` + the tail), and
once as the **history** every later turn derives its context from (the marker
and `context::derive_history_into`). They have to be the same bytes. If they
drift, the turn after a compaction re-sends a different conversation than the
one the model was just in: the prompt cache misses from the first message,
and anything the two disagree about is either lost or said twice.

Two things make them agree:

- **one set of pieces.** Both sides build the single user message from the
  same functions — `context::budget_user_texts` for the user's messages,
  `context::summary_bridge` for the bridge, joined with the derivation's own
  `MERGE_SEPARATOR` — rather than two implementations of one format;
- **the seed.** Only the history knows which user-role messages the *user*
  typed (the wire also carries notices, hook feedback, attachments), and
  which fragment the window opens with. Before every spawn the boundary hands
  both to the backend through the shared `CompactionControl`
  (`CompactionSeed { leading, user_texts }`, the steer queue's pattern), and
  the loop tells the compactor about each message that reaches it mid-turn
  (`Compaction::user_message`).

The app side correlates by **event order**, with no id: on
`CompactionStarted` it notes the history's length (`App::begin_live_compaction`
— flushing the streaming text first, invariant 4), and on `Compacted` the
marker's `kept` is how much the history grew since
(`App::land_compaction`). `tests/compaction.rs` holds the contract end to
end: after a turn that compacted repeatedly against a stand-in provider, the
context the next turn derives is the compacted turn's last request, message
for message.

## What the summarizer reads

Not the conversation's own messages. `llm::compaction::summary_request`
renders it as a **transcript** inside `<conversation>` tags with the
summarization prompt under it, and sends that as one user message on a
client with no tools:

```
<conversation>
[user]
fix the parser

[assistant]
I'll read it first.
[tool call] read {"path":"src/parser.rs"}

[tool result: read]
     1 use std::…
[… 2140 tokens left out of this transcript …]
   412 }
</conversation>

You are performing a CONTEXT CHECKPOINT COMPACTION. …
```

Three reasons, each a failure this replaced:

- **it can be cut to fit.** The request that makes room must never be the
  one that overflows — the old compact turn sent the whole context natively,
  so a conversation past its window could not be compacted at all, which is
  exactly when it must be. The transcript is held to
  `compaction::summary_input_budget` (60% of the window): a long message
  keeps its head and tail (`Caps` — 2 000 tokens of a tool result, 1 000 of a
  call's arguments, 6 000 of text, the newest eight messages twice that), the
  caps halve until it fits, and past the floor the oldest messages after the
  first are left out under a line saying how many;
- **no tools means nothing to refuse.** Providers reject a request that
  carries tool calls but offers no tools; a transcript carries none;
- **a model handed its own conversation with one more message tends to carry
  on with the task** instead of summarizing it.

Every mark of something missing says `left out of this transcript`
(`TRANSCRIPT_CUT`), and the prompt tells the summarizer those cuts are the
transcript's alone. That wording is load-bearing: with codex's
`…N tokens truncated…` a live summarizer reported the *files* as truncated,
and the agent then told the user it had only read parts of them.

The summarizer's request runs on the agent loop like any turn's, so it gets
the retries and the empty-reply continuation (`docs/unfinished-replies.md`).
A summary longer than `compaction::summary_max_tokens` (a tenth of the
window, between 1 000 and 12 000 tokens) is cut in its middle before it is
swapped in, so a runaway summary cannot fill the window it was meant to
empty.

`prompts/compact_prompt.md` is ours now, not codex's four lines: it asks for
six ordered sections (the user's requests, current state, key facts, problems
and fixes, in progress, next steps), for an earlier summary to be carried
forward, for nothing the system prompt already says, and for the handoff to
read as **the agent's own notes** — never a description of what the
summarizer was shown. That last rule came out of a live run too: once the
cuts were explained, the summaries began to say which steps were "not
visible in this transcript" and which facts were "taken from the earlier
summary, not re-verified", and the agent passed the caveat on to the user.
The bridge's closing note (`prompts/compact_resume.md`) asks for the same
from the other side: carry the work on as your own, without a word about the
summary or the compaction.

## The compact turn

`/compact` (palette description: codex's "summarize conversation to prevent
hitting the context limit") dispatches `Action::Compact` when idle with a
non-empty derivable context; with nothing to compact it toasts
`COMPACT_EMPTY_NOTICE`; and **while a turn runs** it is handed to that turn
(`Action::CompactWhenReady`, *`/compact` while a turn runs* above) — it used
to be refused there, codex's rule, which left the user of a long turn
watching the gauge climb with nothing to do about it.

The loop's `Compact` arm is a sibling of `start_background_turn` — a turn with
no new user bubble:

- `App::begin_compact()` opens the status with **fixed** verbs
  (`COMPACT_VERB` "Compacting", which never rotates; the verb cursor does not
  move, so the walk through the status verbs is unaffected) plus an empty streaming buffer and an empty
  **`compact_buffer`** — the flag *and* the accumulator;
- the context is derived as usual and sent as **one user message**: the
  conversation as a transcript with the summarization prompt
  (`prompts/compact_prompt.md`, "You are performing a CONTEXT CHECKPOINT
  COMPACTION…") under it — `llm::compaction::summary_context`, the mid-turn
  summarizer's own request (*What the summarizer reads*), so a conversation
  that has outgrown its window can still be compacted. The prompt is never
  recorded into history (codex-parity: it lives only in the request). The
  offline dummy is still handed the derived context with the bare prompt
  after it, which is what its `compact` scenario keys on;
- the request runs on a **one-off tools-free backend** —
  `LlmBackend::configure(cfg, system_prompt, /*tools=*/false)` with the same
  persona+environment prompt and no background-notice injection — so the
  model can only answer with text (codex sends the summarize request with no
  tools). When no real backend is configured the session backend (the dummy)
  is used instead; `stream::turn_events` scripts a text-only summary for the
  compact prompt so the flow is drivable offline (and by `smoke.sh`).

  **"No real backend" means the session isn't talking to one** — the tracked
  `ModelSession::real_backend`, not "does a config resolve". Gating on a usable
  config instead was a bug: `ModelConfig::is_usable` only proves that a *key*
  resolved, so a configured provider with **no model selected** (the session
  therefore running the dummy, `active_model` = `dummy_model_name`) still built
  a real one-off backend and sent `POST /chat/completions` for a model that
  doesn't exist — `HTTP 400: dummy_model_name is not a valid model ID`, the
  turn resolving red with no marker cell. `smoke.sh` Phase 65 pins it, offline,
  with a provider whose base is the discard port;
- streamed `Chunk`s **divert** into `compact_buffer` (`App::push_chunk` checks
  the flag) — the streaming buffer stays empty, so the strip shows the status
  line only (no preview row, no scrollback commits) and the token tally still
  ticks; codex likewise never renders the summary;
- on `StreamDone`, `App::finish_compact()` takes the buffer, appends the
  `Compaction` marker, and clears the status with **no** `Done for Ns`
  summary; the loop commits the `● Context compacted` cell
  (`ui::compaction_lines`) and runs the normal `dispatch_after_turn` — a
  queued batch dispatches onto the freshly compacted context, and the
  turn-end checkpoint snapshots at the new (grown) length.

## Auto-compact + the context gauge

Codex also compacts **automatically** near the context limit; so do we.
While a turn runs that is the agent loop's own work (*Compacting while the
agent works*: a summary started at 80% of the window, waited for at 90%).
This section is the **between-turns** half — an idle session past codex's
threshold, **90% of the model's context window**
(`ModelInfo::auto_compact_token_limit`, `(context_window * 9) / 10`,
`compaction::hard_threshold`) — and the gauge both halves read.

One switch governs both: the `/settings` **Auto compact** row. The boundary
copies it, and the window, onto the shared `CompactionControl` at every loop
bottom and before every spawn (`Session::sync_compaction_policy`), so a row
flipped mid-turn takes effect at the agent's next step.

- **The window** comes from the provider's `/v1/models` record —
  `context_length` (OpenRouter and most aggregators) or
  `model_spec.availableContextTokens` (Venice) — parsed into
  `ModelEntry::context`, riding a `/model` selection and the startup
  capability probe, persisted in `config.json` beside the vision flag, and
  overridable (or supplied for a provider that reports none, and for the
  dummy) via `ALTER_ZERO_CONTEXT_WINDOW`. Unknown window → no gauge, no
  auto-compact.
- **The gauge** shows in the footer whenever the window is known:
  `{model} · {cwd} · {used}/{window} ({pct}%)` (e.g. `1.3k/160k (0.8%)` — both
  counts humanized by the summary's token formatter, the share one decimal).
  Showing the raw size beside the share is the divergence from codex, which
  prints the percentage alone: the absolute number is what tells you how much
  room a big paste or a long tool output just cost. `used` is kept in the
  provider's own count (`docs/token-estimate.md`): the last usage frame's
  `input` — its count of the re-sent context, tool schemas and framing
  included — plus the calibrated estimate of everything that joined since
  (the reply, a tool's output, the next message, read at the loop bottom
  so it shows before the next request), and across a mutation that count
  moved by the calibrated estimate of the change: after a compaction
  (codex's `recompute_token_usage`), a backtrack, a `/resume`, a `/clear`
  (→ 0 until the next message). A session that never sees a usage frame
  (the dummy) reads the estimate alone, the tool schemas' among it, which
  is also what the first request reads before its frame lands.
- **A subagent's session view gauges that agent, not the lead**
  (`docs/agent-context-gauge.md`): `App::context_gauge` — the footer's one
  question — answers with the viewed agent's own context
  (`AgentRun::context_used`, the same rule, estimated from its transcript when
  a settle saw no frame) against the window of the model it runs on, while
  `context_used`/`context_window` stay the trigger's inputs: auto-compact is a
  fact about the lead's conversation whatever is on screen.
- **An empty conversation reads a true zero.** The estimate returns 0 as soon
  as the derived context is empty — the *same* predicate `/compact` uses for
  `Nothing to compact`, so the footer and the command never disagree about
  whether anything is there. The system prompt and the standing AGENTS.md
  instructions do ride the next request, and the estimate counts them once a
  conversation exists, but they are session constants: a freshly booted
  session carries both and reads `0/1M`, so billing them to a *cleared*
  session made one state show two numbers, and the leftover-looking one
  (`139/1M` under a blank screen) read as conversation that hadn't really
  gone. `/clear` now lands exactly where a fresh session starts.
- **The between-turns trigger** lives at the loop bottom, where every turn
  end and gauge change lands: idle, past the threshold, and with a non-empty
  derivable context, the loop starts the same summarization turn the command
  runs — marked `auto`, so the cell reads `● Context compacted · 88k → 2.1k
  tokens · auto`. **One attempt per user turn** (codex's per-turn semantics):
  a compact turn ending *any* way — landed, Esc'd, or failed — blocks the
  trigger until the next real turn begins, so an insufficient compaction
  never loops and an interrupted one is never restarted against the user's
  wishes. Messages queued while it runs dispatch onto the compacted context
  at its turn end, exactly like the manual command. It is what catches a
  session resumed over the threshold, a model switched to a smaller window,
  and a backend with no engine of its own (the dummy) — a turn on a real
  backend has normally compacted itself long before it ends.

The marker cell's shrink clause (`· {before} → {after} tokens`) appears on
manual compactions too — `before` is the gauge when the compaction began
(for one made while the agent worked, when its summary *landed*),
`after` the gauge once the marker landed — the last count moved by the
estimate of what the summary replaced, so the tool schemas and framing that
ride every request stay in it (`docs/token-estimate.md`); both persist in the
rollout (old files parse with the clause hidden). A ` · {elapsed}` clause
follows the shrink (before the ` · auto` tag): how long the summarization
turn ran — the boundary's turn clock passed into `finish_compact`,
`format_elapsed`-humanized (`36s`, `1m 36s`), recorded as `secs` on the
marker and persisted beside the gauge counts (0/absent on old rollouts hides
it). The cell reads e.g. `● Context compacted · 2.1k → 507 tokens · 36s ·
auto`.

## Interrupts, errors, `/clear`

The swap-only-at-the-end rule is codex's: nothing mutates until the summary
fully streamed.

- **Esc mid-compact** takes the normal interrupt path: the backend is
  cancelled and `App::interrupt_turn` lands in `Kept` (a compact turn has no
  trailing user message, so `Undone` can't fire) with no partial (the
  streaming buffer is empty — the half-summary in `compact_buffer` is
  dropped), recording only the red `Conversation interrupted` notice. No
  marker is appended; the old context stands. A queued batch then dispatches
  on the *uncompacted* context — intended (the compaction didn't happen).
- **A backend error** rides `App::fail_stream` unchanged: the buffer is
  dropped, the red error notice lands, no marker.
- **`/clear` mid-compact** works because the compact turn lives in the loop's
  normal `inflight` slot: the Clear arm cancels the thread and swaps the
  channel, and `clear_conversation` also drops `compact_buffer`.

A summary being written **behind a running turn** dies with that turn, and
nothing has to be undone, because nothing was changed: the swap is the only
mutation and it happens on the turn's own thread.

- **Esc** cancels the turn; the compactor's `Drop` cancels the summarizer's
  request, and a wait on it returns `Cancelled` within 20 ms. No marker, no
  cell — the app forgets the pending floor wherever a turn ends
  (`App::live_compaction` is cleared by `begin_stream`, `take_turn_summary`,
  `fail_stream`, `interrupt_turn` and `block_prompt`).
- **A backend error** in the turn does the same. An error in the *summary*
  is not the turn's: it is a toast, and the turn goes on.
- **`/clear`** also drops a `/compact` still waiting to be taken.

## Files

- `prompts/compact_prompt.md` — the summarization prompt (ours: six ordered
  sections, the note on the transcript's cuts);
  `prompts/compact_summary_prefix.md` — codex's bridge prefix verbatim (**no**
  trailing newline; the bridge is `prefix + "\n" + summary`);
  `prompts/compact_resume.md` — the note a mid-turn bridge closes on.
- `src/compaction.rs` — the policy both sides share, pure: the thresholds,
  the budgets that scale with the window (`user_message_budget`,
  `summary_input_budget`, `summary_max_tokens`, `min_gain`), `CompactionSeed`,
  and `CompactionControl`, the handle the boundary and the backend's thread
  both hold.
- `src/llm/compaction.rs` — the engine: the `Compaction` seam, `LoadMeter`,
  the transcript (`render_transcript`, `summary_request`, `summary_context`),
  `compacted_prefix`, `seed_from_messages`, the overflow tells, the
  summarizer's fold (`SummaryCollector`) and `RollingCompactor`. Its tests
  drive the state machine with a fake summarizer.
- `src/llm/agent.rs` — `run_agent_compacting`, the three call sites,
  `RoundOutcome::Overflow` and the `WindowFull` continuation.
- `src/llm/backend.rs` — builds a compactor per spawn (lead and subagent),
  measures each round into the meter, classifies an overflow, and runs the
  summarizer (`spawn_summary`/`summarize`) on a tools-free client.
- `src/context.rs` — the prompt consts, `approx_token_count`, the budget walk
  (`budget_user_texts`, `recent_user_texts`), the bridge (`summary_bridge`),
  `leading_reminder`, and the marker-aware derivation; all pure and
  unit-tested.
- `src/app/` — `HistoryItem::Compaction(Compaction)` with `kept`/`resume`/
  `budget` (`compact.rs`), `begin_compact` / `finish_compact` / the
  `push_chunk` diversion, the live half (`begin_live_compaction`,
  `wait_on_compaction`, `land_compaction`, `end_live_compaction`), the
  `/compact` command's guards, `Action::Compact` and
  `Action::CompactWhenReady`, and `TurnStatus::compacting`.
- `src/agents.rs` — the same fold for a subagent's own transcript.
- `src/stream/event.rs` — `CompactionStarted`, `CompactionWaiting`,
  `CompactionUsage`, `Compacted`, `CompactionFailed`.
- `src/session.rs` — the `compaction` rollout record (round-trips; old builds
  skip the unknown line, the established forward-compat contract).
- `src/ui/message.rs` — `compaction_lines` (inline cell) and the transcript arm
  (cell + dim summary body); a `conversation_lines` arm so resizes repaint it.
  `src/ui/status.rs` — the `compacting context` clause and verb.
- `src/tui/` — the `Action::Compact` and `CompactWhenReady` arms
  (`actions.rs`), the five event arms and the `StreamDone` compact branch
  (`stream.rs`), `Session::start_compact_turn`, `sync_compaction_policy` and
  `run_pending_compact` (`turn.rs`, called from the loop bottom in
  `bootstrap.rs`), `ModelSession::compact_backend` + its `real_backend` gate
  and the session's `CompactionControl` (`models.rs`), and the
  `ALTER_ZERO_CONTEXT_WINDOW` override (`config.rs`) with the window's
  seeding/persistence.
- `src/llm/models.rs` / `src/llm/settings.rs` — `ModelEntry::context`
  (`context_length` sniffing) and its `config.json` persistence.
- `src/stream/dummy/turns.rs` — the dummy's text-only compact script (the
  `compact` scenario) and the `compact-live` demo, which scripts a summary
  landing between two rounds (`docs/dummy-backend.md`).
- `tests/compaction.rs` — the real backend against a stand-in provider whose
  window is enforced: a turn many windows long, repeated compactions, an
  overflow made room for, a mid-turn `/compact`, and the next turn deriving
  the compacted turn's last request.
- `scripts/smoke/phases/170-compactlive.sh` (the demo and a `/compact` typed
  mid-turn) and `171-compactwindow.sh` (the real engine in the TUI against a
  stub provider that refuses anything past its window).

## Limitations

- **A compaction rewrites the cached prefix.** The compacted first message
  is a different message from the one before it, so the request after a swap
  reads only the system prompt and tool schemas from the provider's cache
  and writes the rest again. That is the price of a smaller request, paid
  once per compaction; it is why the summary starts at 80% and not earlier.
- **The summary is only as good as its model.** The summarizer runs on the
  session's own model and sees long tool results with their middles left
  out. Exact text the next steps need is kept by the prompt asking for it,
  not by any guarantee — a detail nobody wrote down is gone, as with any
  compaction.
- **One message larger than the window cannot be kept.** The tail is kept
  verbatim; if a single tool result alone exceeds the window, the next
  request overflows, and making room summarizes it away with the rest.
- **The offline dummy has no engine.** Its `compact-live` scenario scripts
  the events, and a `/compact` typed mid-turn runs when the turn ends.
- Codex's post-compact "Heads up: Long threads and multiple compactions…"
  warning cell is not ported — the marker cell is the whole record.
- The user-message budget walk and the bridge count tokens as `bytes/4`, not
  the app's token estimate — the approximation codex uses for the budget.
