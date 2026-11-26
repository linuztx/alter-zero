# Background compaction — compacting a turn while it runs

`/compact` and the auto-compact it shares a turn with (`docs/compact.md`) only
ever ran **between** turns: `App::should_auto_compact` refused while a turn was
in flight, so one long agentic turn — twenty tool rounds of file reads and test
logs — grew until a request no longer fit and the turn ended on the provider's
error or on `the reply filled the model's context window before it finished —
run /compact to make room`. The idle auto-compact that followed had to send the
same full window, and the work only resumed when the user typed again.

This design compacts **inside** the running turn, **without stalling it**: a
summary is written in the background while the agent keeps working, held, and
swapped into the turn's message list at a round boundary once the context
reaches its ceiling. The agent waits only if the summary is not written by
then.

**Status:** landing in phases, each its own commit — the summarizer's input
first, then the anchored marker, the policy, the round-top step, and the
boundary. A section describes what the code does once its phase has landed.

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
  `cycle_start` is the context's size right after the last compaction (0
  before the first): 110k on a 200k window. Past it, a background summary
  starts. The lead is fixed in tokens because a summary takes 30–60 s and what
  decides whether it lands in time is how fast the turn grows, not how big the
  window is (opencode's measurement: a few thousand tokens at the median, 25k
  or more one time in twenty); on a small window it is half the room the cycle
  had, so a summary still covers more than the previous one.

`used` is the provider's prompt count for the turn's latest request plus the
token estimate of every message appended since — the footer gauge's rule
(`docs/token-estimate.md`), kept by the turn itself so it is current at the
round top it decides at.

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
2. **The boundary takes the snapshot** where the event lands: `keep_from =
   history.len()`. The channel is FIFO, so every item the turn produced before
   the event is in history — and nothing of the round about to start is. It
   derives the summarizer's input from `history[..keep_from]` and starts the
   summary on a worker of its own (`tui::workers::spawn_background_compaction`),
   a tools-free one-off backend streaming into a private channel; the result
   comes back on the loop's compaction channel.
3. **Held.** The summary is not applied when it lands: the slot marks it
   ready, the strip's row says so, and the turn keeps working on its full
   list.
4. **Swap — at the first round top past the ceiling.** The turn sends
   `StreamEvent::CompactionSwap { id }` and waits for the boundary's answer.
   The boundary appends `HistoryItem::Compaction { keep_from, background: true,
   … }`, commits the `● Context compacted · 170k → 82k tokens · 47s ·
   background` cell, re-derives the context from history exactly as the next
   turn's start would, and hands it back through the slot. The turn rebuilds
   its message list from it — the system prompt unchanged, the reminder
   re-rendered, the kept user messages, the summary's bridge, and every item
   since `keep_from` as it was recorded, provider call ids included — and
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

- **`keep_from: Option<usize>`** — the history index the verbatim tail starts
  at. The derivation becomes: the kept user texts and the bridge for
  `history[..keep_from]`, then `history[keep_from..]` derived normally (the
  marker itself derives nothing). `None` is the old rule — everything before
  the marker — so a rollout written before this parses and derives exactly as
  it did. A `keep_from` past the marker is clamped to it.
- **`background: bool`** — the cell's `· background` tag in place of `· auto`.

Both persist in the rollout's `compaction` record with serde defaults. History
is append-only and the rollout writes one record per item, so the index means
the same thing after a `/resume`; a backtrack that keeps the marker keeps every
item before it.

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

- **A summary that fails** frees the slot, and the next round top past the
  start line starts another; after **two** failures in a cycle no background
  start is tried until a compaction lands (opencode's cap). At the ceiling the
  turn then goes out uncompacted, as it did before this change, and meets the
  provider's limit the way it always has.
- **The history moved** — `/clear`, a backtrack, a `/resume`, a `/model`
  switch, a manual `/compact`: the job is cancelled and forgotten (codex's
  generation check, `App::history_generation`). A summary already paid for is
  still billed.
- **Esc** stops the turn, not the summary: it stays held, and is applied at
  the idle loop bottom if the history it covers is unchanged.
- **Rate limits**: the summary is a second request on the same provider while
  the turn makes its own; its retries back off on the same budget.
- **Headless** (`-p`) runs the same loop, so a long `-p` task compacts too.
- **Subagents** run their own loops over their own transcripts and do not
  compact yet — they never did. The round-top step is written for any
  `run_agent` loop; wiring a subagent's slot to its `AgentRun` is the next
  step.

## What the user sees

- While a summary is being written: a dim row under the status line,
  `⎿ Compacting in the background · 12s`; once written, `⎿ Summary ready ·
  swaps in at 85%`. The spinner keeps its verb — the agent is still working.
- At the swap: the `● Context compacted · 170k → 82k tokens · 47s ·
  background` cell, in place in the transcript, and the gauge drops.
- Waiting at the ceiling: the status verb `Compacting…` until the swap.

## Files

- `src/compaction.rs` — the pure policy: `ceiling`, `start_line`, the
  decision at a round top, and the `CompactionSlot` the turn and the boundary
  share (the `SteerQueue` shape).
- `src/context.rs` — `conversation_messages` (the summarizer's input) and the
  `keep_from` derivation; `src/session.rs` — the record's new fields.
- `src/llm/backend.rs` — the round-top step in the `round` closure.
- `src/stream/event.rs` — `CompactionStart`, `CompactionWaiting`,
  `CompactionSwap`.
- `src/app/compact.rs` — the job's state, the marker, the idle start.
- `src/tui/` — the worker, the channel, the event arms, the swap.
- `prompts/compact_system.md` — the summarizer's system prompt.
