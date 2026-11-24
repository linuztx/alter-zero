# /compact — summarize the conversation to free context

A port of codex's `/compact` (its local "Memento" compaction,
`codex-rs/core/src/compact.rs`): a **summarization request** — the
conversation plus a fixed handoff prompt — goes to the model, the streamed
summary is captured (never rendered), and from then on the model's context
window is rebuilt as codex's *compacted history*: the most recent user
messages (token-budgeted) plus a `SUMMARY_PREFIX`-tagged bridge message
carrying the summary. The visible transcript is untouched — a cyan
`● Context compacted` cell marks the spot, exactly like codex's info cell.

It runs two ways. **Between turns** it is a turn of its own — `/compact`, or
the automatic trigger once the context passes 90% of the window. **During a
turn** it runs in the background: the agent takes a checkpoint at a round
boundary and keeps working while the summary is written beside it, then
splices the summary in at a later round boundary — so a long agentic turn
can outgrow its window several times over and still finish
(*Background compaction* below).

```
❯ long conversation …
● …many turns of replies and tool calls…

(  ●•· ) Compacting… (2s · ↑ 8.1k tokens · esc to interrupt)   ← the turn

● Context compacted                                            ← the marker
───────────────────────────────────────────────────────────────
❯
───────────────────────────────────────────────────────────────
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

So compaction is **append-only**: a compaction ends by *appending* a
`HistoryItem::Compaction` marker — its summary, timestamp, the gauge before
and after, how long it ran, whether it was automatic, and the two fields a
background compaction added: the **cut** it covers up to and the **user
budget** it was made under — and the replacement happens at **derivation
time**: `context_messages` finds the *last* marker and emits, in place of
everything it covers:

1. the covered **typed user messages** (`Role::User` text only — images,
   tool records, shell transcripts, and notices drop, codex-parity), walked
   newest→oldest under the marker's **user budget** (*Small windows*; codex's
   fixed `COMPACT_USER_MESSAGE_MAX_TOKENS`, 20 000, for a marker from before
   the field): whole messages are kept while they fit, the first overflowing
   one is **middle-truncated** to the remaining budget (codex's
   `truncate_middle_with_token_budget` — head + `…{n} tokens truncated…` +
   tail), then the selection is re-reversed to chronological order. Tokens
   are codex's `bytes/4` approximation (`approx_token_count`), not the
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

then derives everything the marker does not cover normally. What a marker
covers is everything before it — or, for a background compaction, everything
before its **cut** (*Where a summary's coverage ends*), the rounds the agent
ran while the summary was written deriving verbatim after the bridge. Earlier
markers sit inside what the last one covers and are skipped — only real
`Role::User` texts are collected, so a prior compaction's summary is
structurally excluded (codex needs an `is_summary_message` prefix check
because its summaries are user messages; ours live in the marker, and reach
the next summary through the summarization request, *Chained compactions*).

Everything downstream holds with zero remapping:

- the **recorder** sees a plain append (no rewrite, no transcript loss; the
  marker is a new `compaction` rollout line, the cut a `compaction_cut` line,
  `session.rs`);
- **checkpoints** stay valid — `after` counts only ever grow;
- **Esc-Esc backtrack** to a pre-compaction user message truncates the marker
  away and the context reverts to the full conversation — codex's
  rollback-past-compaction semantics for free;
- **`/resume`** parses the marker back and the derivation re-applies;
- **Ctrl+D** shows the compacted context automatically (it renders
  `context_messages` fresh);
- the **Ctrl+O transcript** shows the marker cell with the summary text under
  it (dim, indented) — unlike the inline cell, which is just the one line.

## The compact turn

`/compact` (palette description: codex's "summarize conversation to prevent
hitting the context limit") dispatches `Action::Compact` when idle with a
non-empty derivable context, and with nothing to compact it toasts
`COMPACT_EMPTY_NOTICE`. **While a turn runs it is no longer refused** (codex
disables it during a task): it asks the running agent for a checkpoint at its
next round boundary and toasts `Compacting in the background — the agent
keeps working` (*`/compact` while the agent works*); while one is already
running it toasts `COMPACT_BUSY_NOTICE`.

The loop's `Compact` arm is a sibling of `start_background_turn` — a turn with
no new user bubble:

- `App::begin_compact()` opens the status with **fixed** verbs
  (`COMPACT_VERB` "Compacting", which never rotates; the verb cursor does not
  move, so the walk through the status verbs is unaffected) plus an empty streaming buffer and an empty
  **`compact_buffer`** — the flag *and* the accumulator;
- the request is `context::summarization_request`: the context derived as
  usual behind the AGENTS.md instructions alone, **fitted** to the window
  (*The summarization request must fit*), then codex's verbatim summarization
  prompt (`prompts/compact_prompt.md`, "You are performing a CONTEXT
  CHECKPOINT COMPACTION…") as a final user entry — necessary because the real
  backend ignores the bare `prompt` argument whenever the context is
  non-empty — closed by the chain note when the context already holds a
  summary (*Chained compactions*). The prompt is never recorded into history
  (codex-parity: it lives only in the request);
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

## Background compaction

The trigger used to run only between turns. A long agentic turn — dozens of
tool rounds on a small window — grew past the window *inside* the turn and
never reached it: each request carried the whole turn so far, the last ones
twice the window, until the provider refused one or cut the front off
silently. So the turn compacts itself, without stopping: the agent goes on
working while the summary is written, and only ever waits for it at the
hard limit.

### Who decides what

- **The agent decides when** (`llm::compaction::AgentCompactor`, consulted
  by `llm::agent::run_agent_with` at the **top of every round**, after the
  pending inputs and before the request goes out). It is the one place that
  knows the request's size exactly when it matters: the provider's own
  count of the last request (its usage frame) plus the estimate of what it
  appended since, read through the session's calibration — no race with the
  loop, one count.
- **The boundary decides policy and does the work.** It publishes a
  `CompactionPolicy` — the window, the `/settings` **Auto compact** knob, the
  last compaction's head (*When compacting cannot help*), the calibration
  the session's usage reports taught (`docs/token-estimate.md`) — on a shared
  `CompactionSlot`, records the checkpoint, runs the summarizer on a thread
  of its own, builds the head the agent splices in, and commits the marker.
- **`App` holds the state** (`App::background_compaction`, a
  `BackgroundCompaction`: its id, the gauge before, the user budget, the
  summary as it streams, `Summarizing` → `Ready`, whether the agent is
  waiting for it) — pure, and unit-tested like everything else in `app`.

Only the session's own lead agent compacts: the boundary attaches the slot to
the lead's backend alone (`LlmBackend::with_compaction`, on every rebuild),
so the one-off `/compact` backend — the summarizer itself — and a backend
the tests build have no compactor, and a subagent's loop runs the
`NoCompaction` seam (*Subagents*).

### The protocol

In channel order on the turn's reply channel:

```text
agent  ──CompactionRoundTop──►        boundary marks the history's length (every round top)
agent  ──CompactionCheckpoint{id, back}──►  boundary records the cut, starts the summarizer
agent  (keeps working; rounds n+1…)
agent  ──CompactionWait{id}──►        only at the hard limit
boundary publishes Handoff{id, head} on the slot
agent  splices [system] + head + messages[n..]
agent  ──CompactionApplied{id}──►     boundary appends the marker there
```

1. At a round top past the threshold — 90% of the window, codex's — the
   agent takes a **checkpoint**: it remembers how many messages it holds
   (`n`) and sends `CompactionCheckpoint { id, back: false }`. Then it sends
   the round as usual. (At the hard limit it may cover less — *The round
   that crossed the limit*.)
2. The boundary receives the event **in channel order** — every event the
   agent sent before it has already been folded into the history — so the
   history at that instant is exactly the snapshot the agent holds. It
   appends an invisible `HistoryItem::CompactionCut { id }` there and starts
   the summarizer over the history up to it
   (`Session::on_compaction_checkpoint`).
3. The summarizer streams on a channel of its own (`compact_tx`), folded by
   `App::on_summary_event`: text accumulates off screen, a retry withdraws
   what its failed attempt wrote, usage is billed (*Usage*). When it is
   done, the boundary builds the compacted **head** — the
   `<system-reminder>`, the recent user messages, the bridge — and publishes
   it as a `Handoff` on the slot.
4. At its next round top the agent finds the handoff and **splices**:
   `messages[1..n]` (everything after the system prompt up to the
   checkpoint) becomes the head, the rounds it ran since stay as they are.
   It sends `CompactionApplied { id }` and the round.
5. The boundary appends the `Compaction` marker naming the cut and commits
   the cell — mid-turn, under the rounds it kept verbatim and above the next.

### Where a summary's coverage ends

The cut covers everything before it — or, for a checkpoint that reached
back (*The round that crossed the limit*), everything before it **less the
`back` items right before it**: the round that crossed the limit, recorded
before the checkpoint arrived. That count spans one round at most, recorded
in the same few events, so the cut's own position still anchors it.

The cut is a **history item**, not an index. An index would be a claim about
the history's shape that the history cannot check, while the item is the
position itself:

- **`/resume`** reads it back from its own `compaction_cut` rollout line —
  every history item is one line, so a line an older build skips, or one
  that failed to parse, moves nothing.
- **A backtrack** truncates the history: below the cut it takes cut and
  marker away together; between them it takes the marker away and leaves the
  cut, which is inert — nothing names it.
- **A turn that ends before the summary lands** leaves a cut no marker names
  (Esc, a failed summary): inert again, rendered nowhere, derived as nothing.
- **A marker whose cut is missing** — a hand-edited rollout — covers
  everything before itself, which is what every marker before this one
  meant: the derivation can only ever lose verbatim rounds, never duplicate
  them.

The derivation (`context::cut_position`) looks for the nearest preceding
`CompactionCut` with the marker's id; the rollout keeps the marker's `cut`
(omitted when absent) and its `user_budget` (omitted at 0), so files written
before either parse as they always did.

### One rule for the head

The agent's message list and the history's derivation must agree, or the
next turn — or a `/resume` — would send something other than what the model
last saw, and the prompt cache with it. They agree **by construction**: the
head the boundary hands the agent is `context::compacted_head`, the very
function `context_messages_full` opens a compacted history with, called on
the history up to the cut with the marker's summary and budget; and what the
agent keeps after the head — `messages[n..]` — is exactly what the history
holds after the cut, in the order the loop recorded it. The headless suite
proves it end to end (`a_long_turn_compacts_in_the_background_and_its_history_derives_what_the_agent_held`):
a `--continue` run's first request opens on the last request of the turn
that compacted — under another window, past what every request renders
afresh (the system prompt names the process's scratchpad, and the
`<system-reminder>`'s listings are budgeted by the window).

### The hard limit

The agent never sends a request at or past **95% of the window** — the last
twentieth is the reply's own room. What it appended since its last usage
report — a tool's output, mostly — it reads through the session's
calibration, as the gauge does: read raw, a live run on a provider counting
about twice the estimate put a round top between the threshold and the hard
limit (18–19k of a 20k window), and the request went out at 21.4k by the
provider's own count — past the window. At a round top past it:

- with a compaction running, it sends `CompactionWait { id }` once and waits
  for the handoff, polling the turn's `CancelToken` every 20 ms
  (`WAIT_POLL`) — the cadence every blocking wait on the backend thread
  keeps, so Esc stays the stop button;
- with none running, it **forces** one — a checkpoint whatever the
  **Auto compact** knob says (off means "don't compact on your own",
  never "overflow the window") — and waits for it;
- when nothing can make room (*When compacting cannot help*), it sends the
  request if it fits the window at all — or if only the estimate says it
  does not, which the provider's own count may contradict — and otherwise
  ends the turn on an error saying so (`context_full_notice`): codex's
  "still above the token limit after automatic summarization".

The strip's row turns amber while the agent waits (*What the strip shows*).

### The round that crossed the limit

One tool output can take the request from under the threshold to past the
hard limit in a single round — a file bigger than the twentieth of the
window between them. A checkpoint taken there and covering everything would
summarize the very output the agent is about to act on: in the first live
runs the agent took every file it had just read in only through a summary.

So while a window is known the agent announces every round top it sends
from (`StreamEvent::CompactionRoundTop`; the loop notes the history's length
there, `App::mark_round_top`), and a checkpoint taken at or past the hard
limit **reaches back** to the last one when that request fit and held a
round: it covers what that request carried, and the round since stays
verbatim after the summary (`CompactionCheckpoint { back: true }`; the cut
records how many items it reaches back over, `CompactionCut::back`). After a
splice in the same round top there is no such spot — the list was rewritten
— and a checkpoint covers everything. A reaching checkpoint whose summary
fails is not taken again until something new joins. The headless suite's
`a_round_that_crosses_the_hard_limit_reaches_the_model_word_for_word` drives
rounds big enough that every compaction is forced, and checks every round's
output reached the model itself.

### Chained compactions

There is no limit on how many times a turn compacts. A second summary is
written over a snapshot that already holds the first one's bridge, and the
summarization request closes the prompt with `prompts/compact_chain_note.md`
— carry over what the earlier summary still says matters, since its text is
replaced, and treat the work it records as done, never to be checked again
(a live run's second summary called the earlier results unverified, and the
agent read every file again) — so nothing the first summary held is lost. Each marker derives
from its own cut, and only the last marker is applied, so the context is
always one head plus the rounds since.

### Small windows

Codex's fixed 20 000-token budget for the recent user messages is a tenth of
a 200k window; on a 50k one it would be 40% of the window before the summary
or a single round, and the compacted context would land right back over the
threshold. The budget is **a tenth of the window, never more than 20 000**
(`context::compact_user_budget`), and the marker **records** it: the
derivation is a pure function of the history, so a later `/model` switch to
another window — or a resume under another `ALTER_ZERO_CONTEXT_WINDOW` —
derives the same head the agent spliced.

### The summarization request must fit

A summary is most needed exactly when the conversation is too big to send —
and the summarization request carries the conversation. So it is **fitted**
first, deterministically (`context::summarization_request` with
`summarization_budget`: the window less room for the summary — an eighth
of it, at most 16k tokens — in the provider's tokens, turned into the
estimate's through the session's calibration since the request is measured
by the estimate, less the system prompt):

1. the bulk — the tool results and the calls' arguments (a file written
   whole) — is cut in the middle to a common cap no lower than 200 tokens: a
   water level, so the small ones are untouched; a call's cut arguments stay
   a JSON object, the cut text under `truncated_arguments`;
2. then the oldest rounds go whole (an assistant message and its results
   together, so the wire's pairing holds), behind one note saying how many
   messages were left out to fit — joined to the user message before them,
   as the derivation joins user texts — so the summarizer knows the gap is
   the request's;
3. then every text is cut, the last resort — every user message and the
   earlier summary kept as long as anything else can go.

The provider has the last word: a background summarizer the provider refuses
as too long (`is_context_window_refusal`, read off the words providers use)
is sent again on a budget two thirds the size, at most twice — codex's
drop-the-oldest-and-retry, in two big steps instead of one item at a time.

### Never a loop

Compaction is unlimited, so every path that could compact again has to know
whether compacting again can achieve anything:

- **Nothing is compacted before a round has run.** Before the first, the
  context is the fixed overhead and the user's message, which no summary
  shrinks (`has_a_round`).
- **One at a time.** A checkpoint is never taken while another is running;
  the boundary refuses one it cannot run.
- **A failed summary is not retried on the same snapshot**: the agent takes
  a new checkpoint only once something has joined since (`failed_at`).
- **The idle trigger needs news**: after a compaction lands, fails or is
  interrupted, the loop-bottom trigger waits until the history has moved
  (`App::compact_watermark`).
- *When compacting cannot help*, below.

### When compacting cannot help

The **head** — the system prompt, the tool schemas, the recent user messages
and the summary — is the smallest a compaction can make the context. Every
compaction measures it (`App::head_tokens`, the gauge's own estimate through
the provider's calibration) and the policy carries it as the **floor**. At or
past the threshold, an automatic compaction would only land back where it
started, so neither the agent nor the idle trigger starts one, and the loop
says once, in red: `Compacting can't get under the {threshold} threshold:
the summary, instructions and tools alone take {head} — automatic
compaction is paused`. Past the hard limit a forced one could not help
either. The floor describes one conversation under one window: a rewind, a
`/clear`, a `/resume` or a new window forgets it.

### What the strip shows

While a compaction runs beside the turn, a dim row hangs off the status line
in the gutter a tool's output hangs from (`ui::compacting_lines`, one of
`ui::hang_rows`' parts — the tip gives way to it):

```text
⣤⣀⣀⣀⣀⣀⣀⣀ Hatching… (2m 3s · ↓ 18k tokens · esc to interrupt)
  ⎿  Compacting context in the background… (12s)
```

`Waiting for the context to compact… (12s)` in the warning amber while the
agent waits at the hard limit, `Context summary ready — it applies at the
agent's next step` between the handoff and the splice, and `Compacting at
the agent's next step…` after a `/compact` the agent has not reached yet.
Live only — the record is the cell, committed where the summary lands:
`● Context compacted · 46k → 9.8k tokens · 12s · auto`, its time the
compaction's own, from checkpoint to splice.

### `/compact` while the agent works

The command sets a request on the slot; the agent takes it at its next round
top **whatever the size** — the user asked — as long as a round has run and
no compaction is running. Its marker is not tagged `auto`. A turn that ends
before the agent reaches a round top runs it as an ordinary idle compaction
right after.

### A turn that ends mid-summary

A turn can end with the summary not yet spliced — the model answered, or an
error ended it:

- **The summary is done**: the marker lands at the end of the history. It
  still names its cut, so the rounds after it — the turn's last — stay
  verbatim in the next request.
- **It is still being written**: the summarizer becomes the session's own
  compact turn (`Session::settle_compaction_at_turn_end`) — its channel
  swapped in as the turn's, its partial summary moved into
  `compact_buffer`, its clock running from the checkpoint. It ends like an
  idle compaction (`finish_compact`, keeping the cut and the budget), Esc
  interrupts it like one, and queued messages wait for it as they wait for
  one.
- **Only requested**: an idle compaction runs.

### Esc, errors, `/clear`

- **Esc** cancels the turn and the summarizer with it
  (`Session::stop_background_compaction`): no marker, the cut inert, the
  context as it was.
- **An error** in the agent's turn ends the turn; the compaction goes on as
  *A turn that ends mid-summary* says.
- **A failed summary** (anything but a refusal it can retry) tells the agent
  on the slot, which goes on working on the uncompacted context — sending
  the request while it fits the window, ending the turn on its error once the
  provider's own count is past it — and the loop toasts `Compaction failed —
  the agent keeps working: {reason}`.
- **`/clear`** and the quit stop the summarizer and reset the slot.

### Usage

The summarizer is a real request, billed: its usage frames go to the
session's ledger (`App::usage_since_launch`, the `/usage` and `/context`
pages) and to the rollout as `usage` lines tagged `compact: true`
(`docs/rollout-records.md`), placed where they finished — between the cut
and the marker. They never touch the running turn's tally or the gauge,
which count the agent's own requests.

### Headless

`-p` runs the same `Session`, so a headless turn compacts exactly the same
way. `--output-format stream-json` opens an item at the checkpoint and
completes it where the marker lands:

```json
{"type":"item.started","item":{"id":"item_7","type":"compaction","status":"in_progress"}}
{"type":"item.updated","item":{"id":"item_7","type":"compaction","status":"summarized"}}
{"type":"item.completed","item":{"id":"item_7","type":"compaction","auto":true,"before":46012,"after":9817,"summary":"…"}}
```

and `--verbose` logs `● Compacting the context in the background`,
`⎿ Compaction summary ready — it applies at the agent's next step` and the
`● Context compacted · …` line (`docs/headless.md`).

### Subagents

Subagents do not compact: they run the `NoCompaction` seam, and their events
never carry a checkpoint. A subagent starts on a fresh context with one task,
so the turns that need this are the lead's; giving a subagent its own would
mean a summarizer, a cut and a marker on the agent's own transcript, which
`AgentRun` does not model yet.

## Auto-compact + the context gauge

Codex also compacts **automatically** near the context limit; so do we, with
the same threshold — **90% of the model's context window**
(`ModelInfo::auto_compact_token_limit`, `(context_window * 9) / 10`).

- **The window** comes from the provider's `/v1/models` record —
  `context_length` (OpenRouter and most aggregators) or
  `model_spec.availableContextTokens` (Venice) — parsed into
  `ModelEntry::context`, riding a `/model` selection and the startup
  capability probe, persisted in `config.json` beside the vision flag, and
  overridable (or supplied for a provider that reports none, and for the
  dummy) via `ALTER_ZERO_CONTEXT_WINDOW`. Unknown window → no gauge, no
  auto-compact, no hard limit.
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
- **The triggers** are two. *During* a turn the running agent compacts in
  the background (*Background compaction*). *Between* turns the loop bottom
  — where every turn end and gauge change lands — starts the same
  summarization turn the command runs when it is idle, past the threshold,
  with a non-empty derivable context and no background compaction about to
  land, marked `auto` so the cell reads `● Context compacted · 88k → 2.1k
  tokens · auto`. It never repeats itself: after a compaction lands, fails or
  is interrupted it waits for the history to move (`compact_watermark` — so
  an insufficient compaction never loops and an interrupted one is never
  restarted against the user's wishes), and it stops for good when the last
  compaction's head alone fills the threshold (*When compacting cannot
  help*). Messages queued while it runs dispatch onto the compacted context
  at its turn end, exactly like the manual command.

The marker cell's shrink clause (`· {before} → {after} tokens`) appears on
manual compactions too — `before` is the gauge when the compaction began (for
a background one, at its checkpoint), `after` the gauge once the marker
landed — the last count moved by the estimate of what the summary replaced,
so the tool schemas and framing that ride every request stay in it
(`docs/token-estimate.md`); both persist in the rollout (old files parse with
the clause hidden). A ` · {elapsed}` clause follows the shrink (before the
` · auto` tag): how long the summarization ran — the boundary's turn clock
for an idle one, the time from checkpoint to splice for a background one —
`format_elapsed`-humanized (`36s`, `1m 36s`), recorded as `secs` on the
marker and persisted beside the gauge counts (0/absent on old rollouts hides
it). The cell reads e.g. `● Context compacted · 2.1k → 507 tokens · 36s ·
auto`.

## Interrupts, errors, `/clear`

The swap-only-at-the-end rule is codex's: nothing mutates until the summary
fully streamed. (A background compaction's are under *Background
compaction*.)

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

## Files

- `prompts/compact_prompt.md` / `prompts/compact_summary_prefix.md` — codex's
  prompt bytes verbatim (the prefix file has **no** trailing newline; the
  bridge is `prefix + "\n" + summary`); `prompts/compact_chain_note.md` —
  the note a chained summarization closes on.
- `src/context.rs` — the prompt consts, `approx_token_count`, the budget walk
  (`budgeted_user_texts`, `compact_user_budget`), the bridge, the cut-aware
  `context_messages`, `compacted_head`, and the fitted
  `summarization_request`; all pure and unit-tested.
- `src/llm/compaction.rs` — the shared `CompactionSlot`, the
  `CompactionPolicy`, the thresholds, and the agent's `AgentCompactor`
  behind the `RoundTopSeam` trait `run_agent_with` consults at every round
  top; pure enough to test with fakes (`src/llm/agent.rs` tests the seam's
  place in the loop).
- `src/llm/backend.rs` — the lead's seam: the compactor fed each round's
  usage frame, the loaded skills re-read after a splice.
- `src/app/` — `HistoryItem::Compaction(Compaction)` and
  `HistoryItem::CompactionCut` (`types.rs`), `begin_compact` /
  `finish_compact` / the `push_chunk` diversion, the background state
  (`BackgroundCompaction`, `begin_background_compaction`,
  `on_summary_event`, `take_compaction_handoff`,
  `apply_background_compaction`, `adopt_background_compaction`, the floor),
  the `/compact` command + busy/empty guards, `Action::Compact` and
  `Action::CompactInBackground` (all in `compact.rs` / `commands.rs`).
- `src/session.rs` — the `compaction` and `compaction_cut` rollout records
  (round-trip; old builds skip unknown lines, the established forward-compat
  contract).
- `src/ui/message.rs` — `compaction_lines` (inline cell) and the transcript arm
  (cell + dim summary body); a `conversation_lines` arm so resizes repaint it.
  `src/ui/compaction.rs` — the strip's row.
- `src/tui/` — the `Action::Compact` arm (`actions.rs`), the `StreamDone`
  compact branch and the three background events (`stream.rs`),
  `Session::start_compact_turn` (`turn.rs`, shared with the loop-bottom auto
  trigger in `bootstrap.rs`), the background summarizer and its turn-end
  settling (`compaction.rs`), `ModelSession::compact_backend` + its
  `real_backend` gate (`models.rs`), and the `ALTER_ZERO_CONTEXT_WINDOW`
  override (`config.rs`) with the window's seeding/persistence.
- `src/headless.rs` — the stream-json item and the `--verbose` lines.
- `src/llm/models.rs` / `src/llm/settings.rs` — `ModelEntry::context`
  (`context_length` sniffing) and its `config.json` persistence.
- `src/stream/dummy/turns.rs` — the dummy's text-only compact script (the
  `compact` scenario; `docs/dummy-backend.md`). The dummy reports no usage
  and runs no agent loop, so it never compacts in the background.
- Tests: `tests/headless.rs` (`a_long_turn_compacts_in_the_background…`,
  the background and idle cut-summary retries), `smoke.sh` Phases 50, 51
  and 65 (idle), 170 (a long turn against a stub provider: the cell
  committed mid-turn, above the turn's summary).

## Limitations

- **Subagents never compact** (*Subagents*): a subagent that outgrows its
  window fails on the provider's refusal, as before.
- The calibration corrects by at most a factor of two (`docs/token-estimate.md`),
  and learns only from the session's own usage reports: before the second
  one, or on a tokenizer counting more than twice the estimate, a round that
  appends more than the window's last twentieth can still carry a request
  past the window.
- The summarizer runs on the session's model beside the agent's own
  requests: on a provider with tight rate limits, a background summary can
  cost the agent a retry.
- The idle path fits its request but does not retry a refusal; only the
  background summarizer does.
- A reaching cut counts the items it reaches back over (one round's, at
  most): a rollout line inside that round lost on a reload shifts its
  coverage by one item. A cut that covers everything before it is exact.
- Codex's post-compact "Heads up: Long threads and multiple compactions…"
  warning cell is not ported — the marker cell is the whole record.
- The bridge re-tokenizes as `bytes/4`, not the app's token estimate — same
  approximation codex uses for the budget.
