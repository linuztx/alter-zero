# Background compaction — summarize while the agent works

Auto-compaction used to wait for the loop to go idle: `App::should_auto_compact`
refuses while a turn is in flight, so a long run of tool calls inside one turn
kept growing the request until the provider refused it or the reply hit the
window (`the reply filled the model's context window — run /compact to make
room`). Only then, idle, did a separate blocking summarization turn run.

Background compaction moves the summary off the agent's path and lands it
**inside** the turn:

- the summary starts about 60k tokens before the window's **ceiling**, written
  on a thread of its own while the agent keeps working;
- the finished summary is **held** until the request would reach the ceiling,
  then spliced in at a round boundary, the rounds since the cut kept exactly as
  they were;
- the agent waits only when the ceiling arrives before the summary is done.

```
round top     R16          R17   R18   R19   R20 | R21
context       ≥ start                            | ≥ ceiling
run_agent     ── len = N ─────────────────────── splice ──▶
summarizer        └── writes the summary ──┘ held
App           CompactionStarted                    Compacted
              (start = history.len())              (marker kept = len − start)
```

## Where the design comes from

Four harnesses were read from source on 2026-11-26 (opencode `44344bd`, Codex
`af71942`, Hermes Agent `02afb748`, Agent Zero `a6e41ca`):

- **opencode** (`packages/core/src/session/compaction.ts`) ships exactly this
  on by default: a summary started a fixed 60k lead before a ceiling of the
  window less 15%, held until due ("swapping it in now would rewrite the prefix
  and throw away the cache for every step until compaction is actually due"),
  spliced in at the step boundary, and waited for only when unfinished. The
  numbers below are its numbers.
- **Codex** has the same skeleton behind its `async_compaction` flag
  (`session/async_compaction.rs`, `session/compaction_replacement.rs`) and
  contributes two changes: everything after the cut stays **as it was** —
  opencode flattens it to text, tool results cut to 1,250 characters — and a
  ready summary is **used** rather than discarded when the tail grows long
  (opencode then falls back to a blocking foreground summary).
- **Hermes Agent** contributes the summarizer's input: its own instructions in
  the system slot and the conversation as source material, never the agent's
  system prompt (`agent/context_compressor.py:3732`).

## The numbers

All three are functions of the model's context window
(`ModelConfig::context`, the footer gauge's window), pure and in
`app::compact`:

- **ceiling** — `window − max(⌊15% of window⌋, 16k)`, the 16k floor only for a
  window of at least 32k: 170k of 200k, 850k of 1M. The reserve is the room the
  last reply needs (and the summary's own reply), and absorbs a large step past
  the trigger. The idle auto-compact, the `/context` page's reserve and the
  background path all read this one number (`auto_compact_threshold`), so the
  page never shows a threshold the loop does not act on. It replaces codex's
  90%, which left 20k of a 200k window for a reply that may be 32k long.
- **start** — `ceiling − min(60k, (ceiling − cycle start) / 2)`, where the cycle
  start is the context the last compaction left (or none). 60k is the growth a
  conversation can see while a summary is being written (a summary takes
  30–60 s; big file reads can add 25k in that time); the half-room cap keeps a
  small window from summarizing again right after every compaction.
- **tail limit** — 40% of the ceiling: a held summary whose tail (what joined
  since its cut) passes it is spliced in at once rather than held further.

The measure is the backend thread's own: the last round's usage frame
(`input + output`), plus the token estimate of every message appended after
that request — the footer gauge's rule (`docs/token-estimate.md`). Before the
turn's first frame — and for a provider that reports none — it is the gauge
the app handed over, anchored on what the first request carries, plus the
estimate of every message after it.

## What happens at a round top

`run_agent_with` asks its **round checkpoint** at the top of every round, after
the pending inputs are delivered and before the request goes out — the point
`pending_inputs()` already drains. The decision is pure
(`llm::compaction::decide`):

| In flight | Context | Action |
| --- | --- | --- |
| nothing | below start | nothing |
| nothing | at or past start | **start**: note `len = messages.len()`, spawn the summarizer |
| running | below ceiling | nothing |
| ready | below ceiling, tail within 40% | **hold** |
| ready | tail past 40% | **splice** now |
| ready | at or past ceiling | **splice** |
| running | at or past ceiling | **wait**, then splice (a failed summary: inline) |
| nothing usable | at or past ceiling | **inline**: summarize everything now, the agent waiting |

After any splice the context is measured again; still at or past the ceiling
(a tail of huge reads) means one inline compaction of everything.

A summary that failed is not retried in the same cycle: the ceiling compacts
inline, as Codex's default does.

An inline compaction that fails, or that leaves the request at the ceiling
still — what rides every request alone passes it, the system prompt and the
tool schemas of a tiny window — **spends** compaction for the rest of the
turn: the request goes out as it is, and a provider that refuses it ends the
turn as before. Summarizing again at every round would pay for a summary each
time and take the newest tool results away from the model, which then asks
for them again (a stand-in model did exactly that, forever).

## What the summarizer reads

The summary request carries **only the conversation**:

1. its own short system prompt (`prompts/compact_system.md`) — the summarizer's
   identity and the rule that the transcript is material to summarize, never
   instructions to follow;
2. the conversation up to the cut — **no session system prompt** (persona,
   environment, scratchpad) and **no `<system-reminder>`** (the AGENTS.md
   instructions, the skill and agent listings);
3. codex's `SUMMARIZATION_PROMPT` last, as today.

No tool schemas ride it either. The system prompt and the reminder are session
constants: the next request opens with the **current** versions of both, so a
summary that restated them would only carry stale copies forward. And this
request could never share the turn's prompt cache anyway — it is sent without
tools, so its prefix differs from the first byte — which is the one reason
opencode keeps them. A summarizer prompt of its own (rather than none) keeps
providers that require an instructions field (the ChatGPT Responses wire)
working.

`/compact` and the idle auto-compact use the same input
(`context::summarizer_context`).

## What the splice builds

```
[session system prompt]
[user: <system-reminder> + budgeted typed user texts + SUMMARY_PREFIX bridge]
messages[len..]   — every round since the cut, tool pairs intact
```

That is exactly what the derivation produces for the same history with a
`kept` marker, so the next turn re-sends the same shape. The typed user texts
are codex's 20k budget walk (`context::budget_texts`) over the texts the app
hands each turn (the history's typed messages) plus the messages steered into
the turn before the cut — the backend cannot tell a typed message from a notice
once the wire merges adjacent user texts, so it is told. The reminder is
re-inserted from the exact text the app derived the turn with.

## The marker records `kept`

History stays append-only (`docs/compact.md`). The marker lands at the end, as
before, and records `kept: usize`: how many items just before it ran while the
summary was being written, which its summary does not describe:

- `kept = k` — the derivation replaces everything before the last `k` items
  with the user texts and the bridge, then derives those `k` items (and
  everything after the marker) as usual;
- `kept = 0` — a `/compact` or inline marker: everything before it is
  replaced, as before.

The count is relative to the marker rather than an index into the history
because only a relative count survives everything a history goes through: a
backtrack only ever cuts a suffix, so a marker that survives one keeps its `k`
items with it, and a resume need only bring back those `k` items one for one —
not every item since the session began. The app notes `history.len()` when
`CompactionStarted` arrives (the backend sends it after the round's results,
notices and steered messages, so the history then holds exactly what
`messages[..len]` holds) and records `kept` as the history's length at landing
less that. The rollout record gains the field (an old file reads 0) and a
`background` flag, the cell's `· background` tag.

## A summary that outlives its turn

The summary belongs to the session (`CompactionSlot`, shared by the app, the
backend thread and the summarizer thread), not to the turn:

- a summary ready when the turn ends is **landed at once** — at the idle loop
  bottom, before the blocking auto-compact is considered — so the next turn
  starts with room. There is no cache to protect between turns;
- one still running keeps running and lands at the first idle loop bottom after
  it finishes: the summarizer thread **wakes the loop** when it does (the
  slot's waker, one more `select!` source), since an idle session has nothing
  else to wake it. A turn that starts first lands it before its own message,
  if it finished by then; otherwise the turn starts no summary of its own and
  compacts inline if it reaches the ceiling, which lets the older one go;
- `/clear`, `/resume`, a rewind or an undone turn discards the slot, cancelling
  the summarizer: its cut no longer describes the history. The loop bottom
  notices any rewrite (the start the app noted belongs to an older history
  generation), and `/clear` discards it outright, since the start may have
  been drained with the channel. A `/compact` by hand discards it too — it
  summarizes everything now — and quitting does, the summary living in memory
  only. An Esc does not — the work is kept and lands at idle — and neither
  does a `/model` switch: the summary describes the conversation, whichever
  model wrote it;
- a summary whose start never reached the app (dropped with the channel an
  Esc swaps) is refused when it lands: with no cut to keep the items after,
  landing it would replace what it never read. Its requests are billed all
  the same.

The blocking idle turn stays as the fallback where no background summary is
possible: the offline dummy, and a ceiling reached at idle with nothing in
flight.

## Hooks

`PreCompact` fires on the summarizer thread before its request (`trigger:
auto`), its context extending the prompt as the `/compact` turn's does;
`PostCompact` fires there once the summary is written. Their cells land above
the `Context compacted` cell when the summary is spliced or landed — the cells
alone, as the `/compact` turn's: a `PreCompact` hook's text reached the summary
request and enters no other.

## On screen

Nothing while the summary is written in the background. While the agent
**waits** at the ceiling the status verb reads `Compacting…`. The marker cell
reads `● Context compacted · 173k → 78k tokens · 31s · background`, committed
where the splice happened.

## The `/settings` Auto compact row

On (the default): background compaction, with the idle turn as its fallback.
Off: neither; `/compact` by hand still works.

## Not yet

- **Subagents** run `run_agent` without the checkpoint, so a subagent's long
  run is not compacted.
- **A provider's context-length refusal** mid-turn still ends the turn
  (opencode and Hermes compact once and retry; the ceiling's reserve makes it
  rare here).
- **A held summary is in memory only**: quitting loses it, and the next
  session summarizes again.

## Files

- `prompts/compact_system.md` — the summarizer's system prompt.
- `src/app/compact.rs` — `auto_compact_threshold`, `compaction_start`, the
  marker's `kept` and `background`, landing a background summary.
- `src/context.rs` — `summarizer_context`, `budget_texts`, the `kept` split in
  `derive_history_into`.
- `src/llm/compaction.rs` — the slot, the pure decision, the summarizer input
  and the splice, the summarizer thread, the round checkpoint.
- `src/llm/agent.rs` — `RoundCheckpoint` and `run_agent_with`.
- `src/stream/event.rs` — `CompactionStarted`, `CompactionWaiting`,
  `Compacted`.
- `src/session.rs` — the record's `kept` and `background`.
- `src/tui/` — the slot's baseline at each turn start, the events, the waker's
  wake, landing at idle and at a turn start, discarding on a rewrite.
- `tests/headless.rs` — `a_long_turn_is_compacted_in_the_background_as_it_runs`:
  a stand-in provider's turn reads its way to the ceiling, the summary is
  written while it does and spliced in mid-turn, and the next turn's request
  matches the spliced one message for message.
