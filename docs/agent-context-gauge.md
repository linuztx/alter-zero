# The agent session view gauges the viewed agent's context

Date: 2026-09-05

## The report

> the subagent TUI should show the current agent's real context count — the
> current implementation shows the context token count from the main agent,
> not from the subagent

```
─────────────────────────────────────────────── Look up linuztx GitHub profile ─
❯
────────────────────────────────────────────────────────────────────────────────
  kimi-k3 medium · ~ · 23.7k/1M (2.4%)                                      auto

  ◯ main
  ● general-purpose  Look up linuztx GitHub profile 39s · ↓ 64.9k tokens
```

Inside a subagent's session view the footer read `23.7k/1M` — the **lead's**
context — under a roster row saying the agent itself had used 64.9k tokens.

## What was actually happening

`ui::footer_line` built its gauge from `App::context_window()` and
`App::context_used()` — the main session's pair, the one `apply_usage` snaps
to the lead's usage frames and the auto-compact trigger reads
(`docs/compact.md`). Nothing about it knew which conversation was on screen.
The agent session view (`docs/agent-tool.md`) swaps the transcript, the strip,
the queue rows, Ctrl+O and Ctrl+D to the viewed agent's —
`docs/agent-view-streaming.md` is the story of every one of those that lagged
— but the footer's model and gauge segments kept describing the lead.

And the agent's context size was recorded nowhere to show. `AgentRun` kept the
**billed tally** (`tokens`: every usage frame's `input + output` summed — the
roster's `↓ 64.9k tokens`) and the per-turn receipt, but the *context* is a
different number: each frame's `input` already carries the whole re-sent
conversation, so the sum of frames is what the agent has cost, not what it
holds, and the two diverge more with every round.

## The design

Three small pieces, each the main session's own rule one level down:

- **`AgentRun::context_used`** (`agents.rs`) — the agent's context size by the
  main gauge's exact rule (`App::apply_usage`): the last usage frame's
  `input + output`, **replaced** per frame rather than accumulated. A turn that
  settles having seen no frame (the offline dummy scripts none on the agent
  channel; a provider may omit usage) falls back to the tokenizer estimate over
  the agent's own derived transcript — `App::take_turn_summary`'s rule —
  through the one shared counting function, `app::estimate_messages_tokens`,
  so the lead's estimate and an agent's can never disagree about what a
  message costs. Zero until the first of either lands, which is where the main
  gauge starts too. The estimate deliberately leaves out the agent's system
  prompt and briefing (the run knows neither; the view's Ctrl+D shows them) —
  it is only the stand-in for the frame a live provider sends every round.
- **`App::context_gauge()`** — the footer's one question, `(used, window)` for
  the conversation **on screen**: the viewed agent's `context_used` against
  `App::agent_context_window` inside a session view, the main pair otherwise,
  and `None` (no gauge) when that window is unknown.
  `context_used`/`context_window` stay the auto-compact trigger's inputs:
  compaction is a fact about the lead's conversation whatever is on screen.
- **Whose window, whose model.** A definition may pin `model:`
  (`docs/subagents.md`), and a pinned model has a window this session never
  learned — the listing is read for the *selected* model only. So
  `ReplySource::agent_model(agent_type)` surfaces the pinned model (`None` =
  inherits; `LlmBackend` reads it off the same definition the launch switches
  the client for), and the boundary's `sync_agent_view_context` — already run
  on entering a view and beside every `sync_backend_info`, now after the
  capability probe too — injects both `App::set_agent_model` and
  `App::set_agent_context_window` (`ModelSession::agent_context_window(inherits)`:
  the session's window for an inheriting type, only the
  `ALTER_ZERO_CONTEXT_WINDOW` override for a pinned one). The footer then names
  the pinned model in place of the session's — without the session's thinking
  mode, which the launch dropped with the model it replaced — while an
  inheriting type keeps the session's pair. A pinned type with no override
  shows no gauge, exactly as the main footer shows none for a model whose
  window the provider never reported: an honest blank beats the lead's
  denominator under the agent's count.

## Testing

- `agents`: usage frames seat `context_used` at `input + output` and a later
  frame replaces it while `tokens` keeps accumulating; a `StreamDone` that saw
  a frame keeps the frame's number; a settle with no frame estimates from the
  transcript through `estimate_messages_tokens`.
- `app`: `context_gauge` follows the viewed agent and comes back to the lead's
  pair on close; a viewed agent with no known window has no gauge;
  `viewed_agent_model` answers only inside a view.
- `ui`: `footer_line` inside an agent session view shows `64.9k/1M (6.5%)` and
  never the lead's `23.7k`; a pinned model is named without the session's
  thinking mode, and an inheriting type keeps `kimi-k3 medium`.
- `llm::backend`: `agent_model` is the definition's pinned model, `None` for
  the inheriting built-in and for an unknown type.
- `scripts/smoke.sh` Phase 111: under `ALTER_ZERO_CONTEXT_WINDOW`, the
  agent-stream demo's session view opens on `0/100k (0.0%)` — the agent's true
  zero, never the lead's count — reads a non-zero estimate of its own
  transcript once the agent settles, and the main footer's gauge, a different
  number, is back in the main view.
