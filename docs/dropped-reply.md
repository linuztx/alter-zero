# Carrying on a reply the connection dropped

A reply that was streaming when its connection died used to end the turn
there: the half that had arrived stayed on screen, a red notice under it,

```
● request failed: request or response body error: error reading a body from connection: Connection
  timed out (os error 110)
```

and the only way on was to type `continue`. The retry policy (`llm::retry`,
`docs/llm.md`) never sent a request again once any of its reply had
streamed, because the same request starts the answer over and the screen
would show its opening twice. So a link that dropped mid-reply — a laptop
waking on another network, a NAT or proxy that forgot the connection, the
kernel giving up on one whose packets stopped arriving (`os error 110`,
`ETIMEDOUT`; a reset is `os error 104`) — cost the whole turn, at the point
where most of its tokens had been paid for.

Now the reply **carries on**. The round goes out again as a
*continuation* — the reply so far as the model's own message, then a note
asking for the rest — and what comes back streams on after what is on
screen:

```
● The connection drops in the mid

⣀⣛⣀⣀⣀⣀⣀⣀ Working… (0s · ↓ 10 tokens · retrying 1/3 · esc to interrupt)
```

```
● The connection drops in the middle of this sentence, and the reply carries on.

Worked for 2s
```

## Continued, not sent again

Three ways to recover were weighed:

- **Send the round again** and show the new answer — what a content-free
  failure gets. Here it repeats the reply: the streamed text is already in
  scrollback — the reply streams straight there, which invariant 2 makes
  safe — and scrollback cannot be rewritten, so the second answer would
  land under the first.
- **Prefill**: end the request on the partial reply as an `assistant`
  message, for the model to continue it. A trailing assistant message means
  *continue this* on some APIs and *this was said, answer after it* on
  others, and some refuse it while the model reasons — four wires, four
  behaviours.
- **A note**, the one taken: the partial reply as the assistant's own
  message, then a short `user`-role note asking for the rest. A user
  message is what every wire carries the same way and every model answers
  the same way, and the request it makes is the round's own plus two
  messages, a prefix the dropped attempt has just sent — one a provider's
  prompt cache can serve again (`docs/prompt-caching.md`).

## How a round carries on

- **The round keeps what streamed.** `drive_round` (`src/llm/backend.rs` —
  the whole of `stream_round`, over any request: the client's in the
  backend, a fake in the tests) adds up each attempt's reply text as it
  streams, and when the attempt fails that text joins the round's `CutOff`
  (`src/llm/continuation.rs`), after whatever earlier attempts left there.
- **The retry policy decides, as before.** A body read that failed is an
  `LlmError::Http` — the connection existed, so there is nothing to wait
  for — and takes the bounded retry like any host that answers badly:
  `retrying 1/3`, the 500 ms → 1 s → 2 s backoff, at most `/settings`
  **Error retry** attempts (`docs/settings.md`). What changed is the gate in
  front of it. `retry::next_step` used to refuse any failure once content
  had streamed; it now asks whether sending again would **repeat** what
  streamed (`repeats`), and the round's attempt never repeats anything:
  before content there is nothing to repeat, and after it the attempt sends
  the continuation. So `Error retry` 0 still means never, and the budget
  still bounds a link that keeps dropping.
- **The continuation.** `CutOff::request` builds the next attempt's
  messages: the round's own, the reply so far as an `assistant` message,
  then `CONTINUE_NOTE` as a `user` one — `[Your reply above was cut off when
  the connection dropped. Continue it from exactly where it stops — even
  mid-word, …]`. It asks for the rest from the exact character the text
  stopped at, because what comes back is appended to the screen, word
  boundary or not, and it says that a tool call the model was making did
  not go through. While nothing visible has been cut off — the drop came
  before the reply began, or only reasoning or a call being made had
  streamed — the round goes out as it was.
- **Billed as the agent's round.** GitHub Copilot charges a premium request
  for a round the user's own text closes, and the note is a user-role
  message; `copilot::initiator` reads it as the harness's words
  (`continuation::is_note`), so a continuation is marked `agent` like a
  tool round and a drop costs no premium request (`docs/copilot.md`).
- **A lost network is waited for, as anywhere.** The link that dropped is
  often the whole network, so the continuation's own connect fails like any
  other, and the wait in `docs/offline.md` takes over: `Waiting for
  internet…`, unbounded, Esc the way out. Once the network is back the
  continuation goes out unchanged.
- **One reply.** `CutOff::finish` puts the cut-off text in front of the
  finished attempt's own, so the round's outcome is the reply the screen
  shows: the `Stop` hook's `last_assistant_message`, the message a later
  round's context keeps, the `tool_calls` message a tool round appends. The
  note is never replayed; it existed for one request. The app needed
  nothing joined: every `Chunk` appends to the one streaming reply, so the
  transcript, the rollout and the next turn's derived context already hold
  it whole.
- **A call cut off is made again.** A call runs only once its round ends,
  so one the drop cut off never ran, and the continuation asks for it again
  (with no text before it, the round simply goes out as it was). Its pieces
  arrive under the same slot the dropped call's did, and folded into that
  draft they would continue its cut-off arguments — a string the decoder
  then closes, freezing the `Writing N lines…` preview at the old count
  until the call ran. So a `Retrying` restarts the round's drafts
  (`ToolDrafts::restart`, from `App::set_retry` and the subagent fold's
  `Retrying` arm; `docs/write-streaming.md`): the half-made preview goes
  with the attempt that streamed it, and the call made again previews from
  its first line. A queued call's preview stays — its cell is on screen.
- **A thought cut off is thought again.** Reasoning that streamed before
  the drop settles into its `Thought for …` cell as the attempt ends
  (`docs/thinking-stream.md`), and the continuation's own reasoning is a
  cell of its own. Reasoning is never put into the continuation: it is not
  the reply.

Subagents get all of it for free: their rounds are the same `stream_round`,
and their fold restarts its drafts on the same event.

## What shows

The partial reply stays where it is, its last line in the live preview. The
status line wears the bounded retry's amber `retrying 1/3` until the
continuation's first content takes it down (`TurnStatus::recovered`), and
the text that comes back continues the same line. Nothing commits for the
drop itself — no notice, no transcript entry — so the reply reads as if
nothing happened, as at the end of an offline wait.

How cleanly the halves meet is up to the model: the note asks it to pick up
mid-word, and one that restarts its sentence instead repeats those few
words. No splice is attempted on the streamed text — there is no telling a
repeat from a model that meant to say the same words again.

## What stays as it was

- `Error retry` at `0`: the drop is the turn's red error, the partial kept,
  nothing sent again.
- Past the budget, the last drop's error ends the turn, everything every
  attempt streamed kept on screen and in the transcript.
- A failure the provider reports **inside** the stream (an error frame,
  `LlmError::Api` with status `0`) is no drop: the provider answered, and
  its answer was that it failed. It is surfaced as before.
- A body that ends cleanly reads as finished, even without the provider's
  closing frame — some servers send none, so it is no sign of a drop.
- Esc during the backoff or the wait stops silently, and the interrupt
  keeps what streamed like any interrupt mid-reply (`docs/interrupt.md`).

## Testing

- `llm::continuation::tests`: nothing cut off sends the round as it is; a
  cut-off reply adds itself as the assistant's message and the note;
  whitespace alone is no reply to continue; successive drops carry
  everything streamed; the finished round reads as one reply and keeps the
  finished attempt's other fields; only the note itself reads as the note;
  the note's shape. `llm::copilot::tests`: a continuation is agent-initiated.
- `llm::backend::tests` drive `drive_round` with a fake request: a dropped
  reply carries on (the requests, the events — nothing streamed twice — and
  the merged text); a call cut off mid-generation is made again after the
  text before it; a drop mid-thought closes the phase and sends the round
  again; a continuation that cannot connect waits for the network and goes
  out unchanged; repeated drops spend the bounded retry, each continuation
  carrying everything before it; with retries off the drop fails the round.
- `llm::retry::tests`: the gate, now `repeats`, still surfaces a failure a
  re-send would repeat.
- `app::tests::drafts`: `restart` drops the calls being made and keeps the
  queued ones; a retry drops the call the dropped attempt was making, and
  the call made again opens fresh. `agents::tests`: the same for a
  subagent's fold.
- `scripts/smoke.sh` **Phase 145** drives the real binary against a stub
  provider that streams half a sentence and resets the connection once the
  half is on screen: the status line shows `retrying 1/3`, the reply ends
  as one sentence shown once, and the second request is the round's own
  messages, the reply so far and the note. Run against 0.16.0 it ends on
  `request failed: request or response body error: error reading a body
  from connection: Connection reset by peer (os error 104)`. A second half
  turns `Error retry` off and finds that error, and nothing sent again. A
  third drops the connection while a `write` streams into its cell: the
  call made again previews past where the dropped one stopped, runs once,
  whole, and shows as one cell.

## Where the pieces live

| Piece | Where |
| --- | --- |
| `CutOff`, `CONTINUE_NOTE`, `is_note` | `src/llm/continuation.rs` |
| A continuation billed as the agent's | `src/llm/copilot.rs` (`initiator`) |
| The round: what streamed kept, the continuation sent, the outcome merged | `src/llm/backend.rs` (`drive_round`, `stream_round`) |
| The gate (`repeats`) | `src/llm/retry.rs` (`next_step`, `run_attempts`) |
| The drafts restart | `src/app/drafts.rs` (`ToolDrafts::restart`), `src/app/status.rs` (`App::set_retry`), `src/agents.rs` (`AgentRun::apply`) |
| The event | `src/stream/event.rs` (`StreamEvent::Retrying`) |
| The smoke phase | `scripts/smoke/phases/145-droppedreply.sh` |
