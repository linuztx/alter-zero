# An empty reply is not an answer

Date: 2026-10-28

## The bug

A turn could end on nothing:

```
● I'll look at the current model first, so I know what's wrong with it.

Called Brave-devtools 2 times (ctrl+o to expand)

Thought for 3m 42s · 4.1k tokens (ctrl+o to expand)

Conjured for 4m 33s · 95.3k tokens (60k cached · 3k written)

❯ continue
```

The model's last round streamed its reasoning and then ended with no reply
text and no tool call. `llm::agent::run_agent` read every round without tool
calls as the model's answer (`RoundOutcome::Complete`), so it sent
`StreamDone`, and the turn closed on its summary over a reply that never
came. Nothing said anything had gone wrong. The user had to notice, and type
`continue`.

## Why a round comes back empty

- **The model ends its turn after thinking.** It works out in its reasoning
  what to do next, then stops before doing it. Anthropic documents this for
  Claude ("empty responses with `end_turn`", most often right after tool
  results). Its advice is not to send the same request again, because
  "Claude already determined it's done", but to add a continuation prompt in
  a **new user message**. Its example appends `Please continue`. Gemini CLI
  fails any stream with no tool call and no reply text as invalid, and names
  this case outright: `Model stream ended with empty response text but
  contained reasoning thoughts.`
- **The output budget runs out mid-thought.** The provider ends the round on
  `length` (Chat Completions, Ollama), `max_tokens` (Anthropic) or
  `max_output_tokens` (an `incomplete` Responses round). A Chat Completions
  request here carries no `max_tokens`, so the provider's own default
  applies, and a low default is easy for a long think to spend whole.
- **The stream ends before the provider finishes it.** A clean end of the
  body with nothing in it yet and no finish reason at all.

The three look the same on screen, and none of them is the model answering.

## What happens now

**The round says it was empty.** `backend::round_outcome` sorts a completed
stream: a round that asked for tools is `RoundOutcome::ToolCalls`, whatever
text came with it; a round with no tool call and no reply text is
`RoundOutcome::Empty`, carrying the provider's finish reason; anything else
is `Complete`. "No reply text" means blank once trimmed, the same rule the
app's own record follows: a whitespace-only run records no message, so the
user would see exactly as much as for an empty one. Reasoning does not count.
It is never shown as an answer, and it does not go back to the model on the
next round.

The Responses wire reads `incomplete_details.reason` for an `incomplete`
response (`max_output_tokens`, `content_filter`), since the status alone says
only that the response was cut short.

**The loop answers an empty round with a continue prompt.** `run_agent`
appends one user message telling the model that its last response was
empty, and the same turn runs another round, as the user's own `continue`
did. Nothing is added for the empty reply itself: some wires refuse an empty
assistant message, and there is nothing in it to keep. The prompt has two
wordings:

- after an output-limit stop, that the response hit the output token limit
  while the model was still reasoning, so it should continue with brief
  reasoning. The model cannot see its own cut-off thought, so it would not
  otherwise know.
- otherwise, that the response was empty and the user saw nothing.

Both carry the `[system]` tag every note the app itself puts in the
conversation carries (`[error]`, `[background]`), so the model reads the app
speaking, not the user.

**It is recorded the way a Stop hook's feedback is.** The prompt goes out as
`StreamEvent::HookNote` and lands as a `HistoryItem::HookNote` labelled
`Empty reply`. A Stop hook's continuation is the same mechanism: the loop
adds a user message mid-turn and runs another round. Every consumer already
handles that note. The main session records it cell-less, a `/compact` turn
keeps it out of the conversation's record, and a subagent's fold puts it on
the agent's own transcript. Ctrl+O shows it under its label, the rollout
round-trips it, and every later turn's context replays it where the model
read it. `wire_history` then keeps the next turn's prefix byte-identical to
what the provider saw (`docs/prompt-caching.md`).

**It is bounded.** At most `MAX_CONTINUE_PROMPTS` (2) empty rounds in a row
are answered. A round with output, text or calls, starts the count over, so a
long task that comes back empty now and then recovers every time. A third
empty round in a row ends the turn on an error. Two prompts cover the model
whose first try was unlucky. A model still empty after that has a problem a
third prompt will not fix, and each empty round can cost minutes of
reasoning. The error stands alone on a red row, and the model reads it too,
as the next turn's `[error] …` entry. So it says what happened, that the
turn stopped, and what to do:

```
The model returned an empty response 3 times in a row — no text and no tool
call — so the turn stopped here. Send a message to have it continue.
```

After an output-limit stop it adds that the last one ran out of output tokens
while the model was still reasoning, and suggests lowering the thinking level
with Ctrl+T. After any other unusual stop it quotes the provider's finish
reason (`"content_filter"`). An ordinary stop (`stop`, `end_turn`,
`completed`, `stop_sequence`) or none at all explains nothing, so it is not
named.

**Hooks.** `Stop` does not fire for an empty round: the model has not
finished answering. A turn that ends on the error fires `StopFailure` with
`unknown`, the same as the tool-call ceiling (`docs/hooks.md`).

**Esc.** A cancel that lands as the round comes back empty ends the turn
silently. Nothing is recorded and no prompt goes out. The interrupt owns the
turn's end, as it always did.

## What the user sees

When the prompt works, nothing extra inline. The turn keeps running: the
status line keeps its clock, a second `Thought for …` line settles under the
first, and the reply follows. Ctrl+O shows `● Empty reply` over the prompt
between the two thoughts.

When it does not, the red error above.

## Where it applies

Every turn the agent loop drives:

- the main session;
- every subagent, whose empty final round used to hand the lead an empty
  answer;
- `/compact`'s summarization turn. Its empty summary used to commit a
  compaction marker with nothing in it, which replaced the conversation with
  its recent user messages and a `(no summary available)` bridge. Now the
  turn asks again, and if the summary never comes it fails like any failed
  compaction, leaving the old context standing (`docs/compact.md`).

## Known limitations

- A reply the output limit cuts off **with** text still ends the turn on the
  partial reply. Continuing it is a separate change (Claude Code's
  max-output recovery).
- A stream that ends without its finish marker but with some text is still
  taken as finished. Only an empty one is caught here.
- Reasoning does not round-trip, so after an output-limit stop the model
  reasons again from the start. A provider whose cap is smaller than the
  model's thinking can keep cutting it off. The error's Ctrl+T advice is
  the way out.
- On Claude, the first cause Anthropic names is text placed right after
  tool results: the history then shows user text after every tool result,
  and Claude ends its turn to follow the pattern. The app puts text there
  itself. An image that a `read` or an MCP tool returned rides a user
  message after the round's results, as do background notices and messages
  steered into the turn, and the Anthropic wire merges each into the same
  message as the `tool_result` blocks. The continue prompt is one more such
  message, added only once a round has already come back empty, which is
  where Anthropic places it too. Carrying an image inside its own
  `tool_result` on that wire would remove the commonest of them, and is a
  separate change.

## Testing

- `llm::agent::tests`: the prompt and its place in the next request, the
  output-limit wording, the bound and the error that ends the turn, the
  count starting over after a round with output, `Stop` not firing for an
  empty round, `StopFailure` firing for the error, and a cancel recording
  nothing.
- `llm::backend::tests`: the classification (reasoning only, blank text, a
  reply, a call with no text), and a real `stream_round` against the
  loopback SSE stub for a round that reasons until `length`.
- `llm::responses::tests`: an `incomplete` response names why it stopped.
- `tests/wire_history.rs`: a real `LlmBackend` turn whose first round only
  reasons. The next request carries the prompt after the question, the
  reply ends the turn, and the next human turn sends the prompt where the
  provider saw it, byte for byte.
- `smoke.sh` Phase 146: the real binary against a stub provider. A round
  that only thinks and stops is followed by the reply in the same turn, with
  no error and no interrupt; Ctrl+O shows `● Empty reply` over the prompt and
  Ctrl+D shows the prompt in the context. Then a conversation whose every
  round runs out of output tokens: two output-limit prompts go out, and the
  turn ends on the error naming Ctrl+T after exactly three requests.
