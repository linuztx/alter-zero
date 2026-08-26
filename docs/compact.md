# /compact — summarize the conversation to free context

A port of codex's `/compact` (its local "Memento" compaction,
`codex-rs/core/src/compact.rs`): the command runs a special **summarization
turn** — the whole conversation plus a fixed handoff prompt goes to the model,
the streamed summary is captured (never rendered), and from then on the model's
context window is rebuilt as codex's *compacted history*: the most recent user
messages (token-budgeted) plus a `SUMMARY_PREFIX`-tagged bridge message carrying
the summary. The visible transcript is untouched — a cyan
`● Context compacted` cell marks the spot, exactly like codex's info cell.

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
   codex's `bytes/4` approximation (`approx_token_count`), not the tiktoken
   seam — the walk runs at every turn start and must be O(len);
2. the **bridge**: `{SUMMARY_PREFIX}\n{summary}` as a user message (an empty
   summary becomes `(no summary available)`, codex's fallback). The prefix is
   codex's verbatim `prompts/compact_summary_prefix.md` ("Another language
   model started to solve this problem…");

then derives the items *after* the marker normally. Earlier markers (a second
`/compact`) sit before the last one and are skipped — only real `Role::User`
texts are collected, so a prior compaction's summary is structurally excluded
(codex needs an `is_summary_message` prefix check because its summaries are
user messages; ours live in the marker).

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

## The compact turn

`/compact` (palette description: codex's "summarize conversation to prevent
hitting the context limit") dispatches `Action::Compact` when idle with a
non-empty derivable context; mid-turn it is rejected with a
`COMPACT_BUSY_NOTICE` toast (the `/help`/`/resume` pattern — codex also
disables it during a task), and with nothing to compact it toasts
`COMPACT_EMPTY_NOTICE`.

The loop's `Compact` arm is a sibling of `start_background_turn` — a turn with
no new user bubble:

- `App::begin_compact()` opens the status with **fixed** verbs
  (`COMPACT_VERB` "Compacting"; `turn_count` does not advance, so the cycled
  per-turn verbs are unaffected) plus an empty streaming buffer and an empty
  **`compact_buffer`** — the flag *and* the accumulator;
- the context is derived as usual and codex's verbatim summarization prompt
  (`prompts/compact_prompt.md`, "You are performing a CONTEXT CHECKPOINT
  COMPACTION…") is pushed onto it as a final user entry — necessary because
  the real backend ignores the bare `prompt` argument whenever the context is
  non-empty. The prompt is never recorded into history (codex-parity: it
  lives only in the request);
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
  auto-compact.
- **The gauge** shows in the footer whenever the window is known:
  `{model} · {cwd} · {used}/{window} ({pct}%)` (e.g. `1.3k/160k (0.8%)` — both
  counts humanized by the summary's token formatter, the share one decimal).
  Showing the raw size beside the share is the divergence from codex, which
  prints the percentage alone: the absolute number is what tells you how much
  room a big paste or a long tool output just cost. `used` is the last
  usage frame's `input + output` — the provider's own accounting of the
  re-sent context plus the reply that joins the next request — kept honest
  across mutations by a tokenizer re-estimate: after a compaction (codex's
  `recompute_token_usage`), a `/clear` (→ 0), a backtrack, a `/resume`, and
  at the end of any turn that saw no usage frame (the dummy).
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
- **The trigger** lives at the loop bottom, where every turn end and gauge
  change lands: idle, past the threshold, and with a non-empty derivable
  context, the loop starts the same summarization turn the command runs —
  marked `auto`, so the cell reads `● Context compacted · 88k → 2.1k tokens
  · auto`. **One attempt per user turn** (codex's per-turn semantics): a
  compact turn ending *any* way — landed, Esc'd, or failed — blocks the
  trigger until the next real turn begins, so an insufficient compaction
  never loops and an interrupted one is never restarted against the user's
  wishes. Messages queued while it runs dispatch onto the compacted context
  at its turn end, exactly like the manual command.

The marker cell's shrink clause (`· {before} → {after} tokens`) appears on
manual compactions too — `before` is the gauge when the compaction began,
`after` the fresh estimate of the compacted derivation; both persist in the
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

## Files

- `prompts/compact_prompt.md` / `prompts/compact_summary_prefix.md` — codex's
  prompt bytes verbatim (the prefix file has **no** trailing newline; the
  bridge is `prefix + "\n" + summary`).
- `src/context.rs` — the prompt consts, `approx_token_count`, the budget walk
  (`compacted_user_texts`), the bridge, and the marker-aware
  `context_messages`; all pure and unit-tested.
- `src/app/` — `HistoryItem::Compaction(Compaction)` (`types.rs`), `begin_compact` /
  `finish_compact` / the `push_chunk` diversion, the `/compact` command +
  busy/empty guards, `Action::Compact`.
- `src/session.rs` — the `compaction` rollout record (round-trips; old builds
  skip the unknown line, the established forward-compat contract).
- `src/ui/message.rs` — `compaction_lines` (inline cell) and the transcript arm
  (cell + dim summary body); a `conversation_lines` arm so resizes repaint it.
- `src/tui/` — the `Action::Compact` arm (`actions.rs`), the `StreamDone`
  compact branch (`stream.rs`), `Session::start_compact_turn` (`turn.rs`,
  shared with the loop-bottom auto trigger in `bootstrap.rs`),
  `ModelSession::compact_backend` + its `real_backend` gate (`models.rs`), and
  the `ALTER_ZERO_CONTEXT_WINDOW` override (`config.rs`) with the window's
  seeding/persistence.
- `src/llm/models.rs` / `src/llm/settings.rs` — `ModelEntry::context`
  (`context_length` sniffing) and its `config.json` persistence.
- `src/stream/dummy/turns.rs` — the dummy's text-only compact script (the
  `compact` scenario; `docs/dummy-backend.md`).

## Limitations

- No context-window-exceeded recovery *during* the summarize call: codex
  drops the oldest history item and retries on that provider error; our
  `ReplySource` seam has no such classification, so the turn fails with the
  provider's error notice once (ironically, the failure `/compact` exists to
  prevent). Retry the command after trimming by hand (`/clear`, backtrack).
- Codex's post-compact "Heads up: Long threads and multiple compactions…"
  warning cell is not ported — the marker cell is the whole record.
- The bridge re-tokenizes as `bytes/4`, not the real tokenizer — same
  approximation codex uses for the budget.
