# A turn never ends on a reply that isn't one

The agent loop (`llm::agent::run_agent`, `docs/tools.md`) used to decide a
turn was over by one rule: **a round that asked for no tools is the model's
answer.** That rule ended turns on rounds that answered nothing:

```
● I'll look at the current model first, so I know what's wrong with it.

Called Brave-devtools 2 times (ctrl+o to expand)

Thought for 3m 42s · 4.1k tokens (ctrl+o to expand)

Conjured for 4m 33s · 95.3k tokens (60k cached · 3k written)

❯ continue
```

Two tool rounds, then a round that only thought — and the turn closed on its
summary, with nothing said, until the user typed `continue`. Now a round
that ends **unfinished** is answered the way a user would answer it, inside
the same turn, and a turn that still gets nothing says so in red rather than
looking finished.

## What ends a round without finishing it

Five things, each a stop the providers document:

- **An empty reply.** The model ends its turn with no text and no tool call.
  Anthropic documents it for Claude — "exactly 2–3 tokens with no content",
  `stop_reason: "end_turn"`, "particularly after tool results" — and other
  models have been reported doing the same after a tool round. Nothing about
  the stream is wrong; the model simply stopped.
- **The output cap, spent thinking.** Reasoning and reply share one output
  budget, so a long thought can spend all of it before a word of the reply
  is written. The provider says so — `finish_reason: "length"` on Chat
  Completions and Ollama, `stop_reason: "max_tokens"` on Anthropic, a
  `response.incomplete` whose `incomplete_details.reason` is
  `max_output_tokens` on the Responses API — but the loop never read it.
  The thought on screen can look small while this happens: Anthropic's
  `display: "summarized"` (`docs/claude.md`) streams a summary of the
  thinking, not the thinking, and a chat provider that applies its own low
  default output cap (the app sends no `max_tokens` there,
  `docs/prompt-caching.md`) hits it after a modest one.
- **The output cap, mid-reply.** The same stop with part of the reply shown:
  the turn ended on half a sentence, looking like an answer.
- **The context window.** Anthropic's `model_context_window_exceeded`: the
  reply filled the model's whole context before it finished.
- **A content filter.** `finish_reason: "content_filter"` (and the Responses
  API's `content_filter` reason): the provider omitted the rest of the reply.

## Classified where the wire speaks, decided where the loop runs

**The wire's half** is one question with one answer per spelling.
`StreamOutcome::stop_reason` (`src/llm/openai.rs`) folds every wire's
finish reason into the crate's own `StopReason` — `Finished`, `OutputCap`,
`ContextWindow`, `ContentFilter` — case-insensitively, so a shim passing a
provider's `MAX_TOKENS` through reads the same. The Responses drain now
carries `incomplete_details.reason` as the finish reason (it used to keep
only the bare status, `incomplete`, which said a response was unfinished but
not why). No reason at all — a shim that never sends one — is a finish.

`llm::backend::round_outcome` then maps a finished stream to what the loop
acts on, purely, so every branch is a unit test:

| the stream | the round |
| --- | --- |
| asked for tools | `ToolCalls` — whatever the stop reason, a finished call runs |
| filled the context window | `Failed`: the conversation has no room for a continuation either, and the message points at `/compact` |
| was stopped by a content filter | `Failed`, naming the filter: asking again meets the same filter |
| hit the output cap | `CutOff { text }`, the reply so far kept |
| anything else | `Complete { text }` — empty or not |

Whether an **empty** `Complete` may end the turn is deliberately not the
wire's question: it is the same fact on every provider, and the loop is
where the turn's other ending rules already live.

**The loop's half** (`run_agent`) treats two rounds as unfinished — a
`Complete` whose text is empty or whitespace, and any `CutOff` — and answers
each with a **continuation prompt in a new user message**:

- `Empty reply`: *[system] Your last response was empty: it had no reply
  and no tool call. Continue with the task — no apology, no recap. If the
  task is already done, reply with a short summary of what you did.*
- `Reply cut off`: *[system] Your last response was cut off at the output
  token limit. Continue directly from where it stopped — no apology, no
  recap. Keep your reasoning short, and break the remaining work into
  smaller steps.*

The `[system]` tag is the one the derived context gives every notice the
app speaks (`docs/context.md`): the prompt rides a user message, and
untagged the model would read the app's words as the user's.

What was said before a cut is kept as an assistant message, so the
continuation reads where it left off. An **empty** reply is never sent back:
Anthropic's stop-reason guidance is explicit — "Sending the empty response
back won't help", and the fix is to "add a continuation prompt in a new user
message rather than retrying with the empty response" — and an empty
assistant message is refused on that wire anyway. The empty-reply prompt
leaves the model a way to finish properly (*reply with a short summary*),
since a model that ended because it was done needs to say so, not to invent
more work.

The prompt is recorded with the event the `Stop` hook's continuation already
uses, `StreamEvent::HookNote` (`docs/hooks.md`), under the label above. That
one choice is the whole integration, because everything that event does is
what a continuation needs:

- the loop flushes the streamed text before it, so the continuation streams
  as its own message (invariant 4);
- it records the cell-less `HistoryItem::HookNote`, so the **Ctrl+O
  transcript shows why the turn kept going** — `● Empty reply` over the
  prompt — while the inline view stays as clean as it is after a hook's
  continuation;
- `context::context_messages` replays it as the user message the model
  read, so the next turn's context — and the retained wire prefix
  (`docs/prompt-caching.md`) — agree with the request that was sent;
- the rollout round-trips it, so a `/resume` restores it;
- a subagent's fold and its `RunRecorder` already handle it
  (`docs/agent-progress.md`), and a `/compact` turn's notes never enter the
  conversation's record, so a summary that came back empty just arrives a
  round later.

## Bounded, and loud when it runs out

`MAX_CONTINUATIONS` (2) counts **consecutive** unfinished rounds — three
attempts in all, the bound Anthropic's own continuation example uses
(`max_attempts=3`). A round that makes progress — a tool call, or a real
answer a `Stop` hook sends back for more — earns the budget back, so a long
task that stalls now and then is never cut off for it, while a model that
will not reply at all costs three requests, not an endless loop. Esc stops
it at any point, as it stops everything; a turn cancelled just as an
unfinished round comes back asks for nothing more.

The third unfinished round in a row ends the turn **the way a failure
does** — a red notice through `StreamEvent::Error`, with `StopFailure`
fired first (`docs/hooks.md`): `unknown` for replies that never came,
`max_output_tokens` (Claude Code's error type for it) for replies that kept
being cut off. Each notice carries its own way out, since the user meets it
as a bare red row:

- *The model stopped without replying, even after being asked to continue 2
  times. Send a message to try again, or switch models with /model.*
- *The model's reply kept getting cut off at its output token limit, even
  after 2 continuations. Ask for the work in smaller steps, or lower the
  thinking effort with Ctrl+T.*

The model reads it too, next turn, as the `[error] …` entry every red
notice derives into (`docs/context.md`).

`Stop` and `SubagentStop` fire only on a real answer — never on the empty
round, and never on a turn that ran out — so a hook sees the reply it was
written to check.

## What it looks like

Inline, the empty round's `Thought for …` cell stays where it settled, and
the next round streams under it — its own thought, its answer, then the
turn's summary. Nothing is added to scrollback for the continuation itself:
like a hook's, it is the transcript's to show (Ctrl+O), and the context's to
carry (Ctrl+D).

## Why not something else

- **Re-send the same request.** The empty reply is what that request
  produced; Anthropic's guidance names doing this as a mistake, and a cap
  spent thinking is spent again.
- **Raise the output cap.** The app keeps no per-model output cap — a
  model's entry holds its context window, a different number
  (`docs/claude.md`) — OpenAI's reasoning models refuse `max_tokens` for
  `max_completion_tokens`, and OpenRouter reserves the whole cap against the
  account's credits before it runs a request (`docs/prompt-caching.md`). A
  continuation works on every wire as it is.
- **Show an error at once.** The user's next move is almost always
  `continue` — so the loop makes it, and keeps the error for when it does
  not help.

## Known limitations

- **A continuation is a new message.** A reply cut mid-sentence and
  continued reads as two bullets, the second picking up where the first
  stopped. A subagent's answer is likewise its last message, the rule a
  `SubagentStop` continuation already follows: the part before the cut is on
  its record as a progress update (`docs/agent-progress.md`).
- **A tool call cut at the cap on Chat Completions** keeps its truncated
  arguments and runs, failing to parse — the model reads that error and
  tries again, as it did before.
- **Prevention is not here.** Anthropic's first cause for an empty reply is
  a user text block straight after the tool results, which teaches the model
  that its turn ends there. An image a tool returns (a `read` of a picture,
  an MCP screenshot) rides exactly that shape today: the `[image] …` note
  and its picture follow the results as a user message, which the
  Anthropic wire merges into the `tool_result` message (`docs/tools.md`
  *Image reads*). Moving the picture into the result itself, where
  Anthropic accepts it, would remove that pattern on that wire; it is a
  separate change across the message model, four wires and the context
  replay.

## Tests

- `llm::agent::tests` — the loop: an empty reply, a whitespace one, a cut-off
  one with and without text, `Stop` hearing only the answer, the give-up on
  each kind with its `StopFailure` type, progress earning the budget back, an
  Esc under an unfinished round, the prompts' `[system]` tag, and the errors'
  wording.
- `llm::backend::tests` — `round_outcome` for every stop reason, and
  `stream_round` against a loopback stub that stops at its output cap.
- `llm::openai::tests` / `llm::responses::tests` — every wire's spelling of
  a stop, and the Responses API's `incomplete_details.reason`.
- `tests/wire_history.rs` — end to end through a real backend: the request
  after an empty reply carries the prompt and nothing for the reply, the
  request after a cut-off one carries what was said and then the prompt, and
  the next human turn sends both as the provider saw them — the cut-off case
  on a rebuilt backend, from the records alone.
- `tests/live_unfinished.rs` — the Anthropic Messages wire through a Console
  sign-in, `#[ignore]`d (the rotating refresh token writes back to
  `ALTER_ZERO_LIVE_TOKEN_STORE`): a 40-token cap stops a count from 1 to 30
  part way with `stop_reason: "max_tokens"`, and the continuation finishes
  it in the same turn (when written, the first round stopped at 20 and the
  second went on from 21 to `DONE`); the request an empty reply is
  continued with — the question and the prompt side by side — is answered.
- `scripts/smoke/phases/146-emptyreply.sh` — the real binary against stub
  providers that reply with nothing, stop at the cap while thinking, and
  never reply: the recovered answers, the `Thought for` cell, the prompt in
  Ctrl+O and in Ctrl+D, the request counts, and the red notice that names
  `/model`.
