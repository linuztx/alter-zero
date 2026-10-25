# Carrying a reply on after its connection drops

A reply that was streaming when its connection died used to end the turn:

```
● Two steps. First, split the lex

● request failed: request or response body error: error reading a body from
  connection: Connection timed out (os error 110)
```

Half an answer, a red error made of three layers of the HTTP client's own
words, and — in an agentic turn — a multi-step task stopped wherever it was,
waiting for the user to type `continue`. Now the turn carries on:

```
● Two steps. First, split the lex

● Lost the connection to api.venice.ai · reconnecting
  ⎿  Connection timed out (os error 110)

● First, split the lexer. Then test it.

Worked for 1m 12s
```

The half that streamed stays, a marker says what happened and where, and the
model picks the reply up from where it was cut off, in the same turn.

## What the error is

`Connection timed out (os error 110)` while reading a body is the kernel
giving up on a connection that went dead under the reply. `reqwest` 0.12
turns on TCP keepalive by default — a probe after 15 s of silence, three
probes 15 s apart — so about a minute after the network drops, the VPN
reconnects, the laptop changes Wi-Fi or a NAT forgets the flow, the next
read fails with `ETIMEDOUT`. hyper reports it as `error reading a body from
connection` and `reqwest` wraps that as `request or response body error`.

It happens *after the response began* — the request was sent and a `200`
came back — and a dead link is usually noticed mid-reply, which is exactly
the case the retry policy could not touch: `next_step` re-sends only a
request that emitted nothing, because a re-send after content would stream
the reply twice (`docs/llm.md`). So the failure was surfaced, and the turn
was over.

## Naming it

Every failed read of a response body is now
`LlmError::ConnectionLost { host, message }` (`LlmError::body_read`, called
by the transport thread, `openai::run_transport`, with the request's own
URL): a keepalive that gave up, a reset, a stream closed before its last
chunk, a read stalled past `NET_OP_TIMEOUT`. `host` is the host and
non-default port the reply was coming from (`host_label`, the offline wait's
rule); `message` is the **root** of the cause chain — the socket's own
words. The layers above it say only that a body was being read, which the
sentence now says:

```
lost the connection to api.venice.ai: Connection timed out (os error 110)
```

(A deepest layer that prints nothing falls back to the whole chain.) Before
any content has streamed, a `ConnectionLost` is the ordinary bounded retry
(`is_retryable`), exactly as the `Http` it replaces was.

## Carrying it on

### The policy

`retry::resume_number(err, dropped, max)` decides, purely: a failure **after
content** is carried on only when it is a `ConnectionLost` — the connection
cut the reply off, the provider did not refuse it — and only while fewer than
`max` drops in a row have been carried on. `max` is the `/settings` **Error
retry** budget (3 by default), so `0` still means never retry, and a link
that drops again and again still ends the turn — with the `lost the
connection to …` sentence above. `dropped` counts the rounds **in a row**
whose connection died mid-reply; any round that finishes resets it, so a
long agentic turn on a flaky network spends the budget per stretch of bad
luck, not once for the whole turn. Anything else after content — a
provider's own error frame mid-stream, a decode failure — is surfaced as
before.

### The round

`llm::backend::stream_round` keeps, for the last attempt, the visible text it
streamed and whether it was generating a tool call (only an attempt that
emitted nothing is ever re-sent, so the last attempt is the one that did).
When the policy says to carry on, the round reports
`RoundOutcome::Dropped { partial, cut_call, host, cause, attempt, max }`
instead of `Failed`. The count lives with the caller — one per main turn, one
per subagent run — and the round that carries on waits the bounded retry's
backoff first (`retry_backoff`: 500 ms, 1 s, 2 s), interruptible like any
retry's, so a link that drops again at once is not hammered.

### The loop

`llm::agent::run_agent` answers a `Dropped` round by sending
`StreamEvent::ConnectionLost { host, cause, note, attempt, max }` and running
another round in the same turn:

- **Visible text was kept** — the model said something the user has on
  screen. It stays: the text is pushed as the assistant message it was, the
  note after it as a user message, and the next round continues the reply.
- **Nothing visible was kept** — only reasoning had streamed, or the pieces
  of a tool call. There is nothing to go on from (a previous round's
  reasoning is never sent back, `docs/thinking-stream.md`), so the round
  simply goes out again exactly as it was, and `note` is `None`.

A call the model was still generating when the connection died never reached
the loop, so nothing of it is kept — not on the wire, not in the record
(`docs/write-streaming.md`). The note asks for it again; with nothing
visible kept, the round sent again makes it afresh.

If the network is gone, the continuing round's connect fails and the
**offline wait** takes over (`docs/offline.md`): `Waiting for internet…`,
unbounded and uncounted, until the link is back. That is the common shape of
the reported case — the link died, so the reconnect has to wait for it — and
the two compose without either knowing about the other.

## The note

What the model reads between its cut-off message and the round that
continues it (`llm::agent::resume_note`):

> [system] The connection dropped while you were replying, so your message
> above was cut off where it ends. Continue it: start again at the beginning
> of the sentence or code line it stopped in, copying that text exactly, and
> don't repeat anything before it. If it stopped inside a code block, reopen
> the block first.

…with `The tool call you were making was not received; make it again.` when
`cut_call` says one was being generated.

- **`[system]`-prefixed**, like every note the app writes to the model
  (`docs/context.md`), so it never reads as the user.
- **Start again at the sentence**, not mid-word: the continuation lands under
  the marker as a message of its own, so opening on a whole sentence is what
  reads; and a repeat that begins where a sentence or a line begins is what
  `join_resumed` can find again (below). A model that carries straight on
  mid-word instead still reads, under the marker.
- **Reopen the block**: the continuation renders on its own, so code without
  its fence would render as prose.

### Why not re-send it, or prefill it

Re-sending the round from scratch would put a second copy of the reply under
the first — scrollback is immutable, so the half that streamed cannot be
taken back — pay for the whole reply again, and get a different reply than
the one the user already started reading. Asking the provider to continue an
assistant message in place (a prefill) is not portable: Chat Completions has
no such thing, and Anthropic's current models answer a request that ends on
an assistant turn with a `400`. Continuing in a round of its own — the
request ends on the note, a user message — works on every wire, and is what
the user saw anyway: something was cut off, and it went on.

## What shows

- **The marker** — `● Lost the connection to {host} · reconnecting` with an
  amber bullet (`connection_lost_color()`, the theme's `warning`: the turn
  goes on, so neither an outcome's green nor an error's red) over the cause
  in the `⎿` gutter a tool's output hangs from, both wrapped under their own
  columns (`ui::reconnect_lines`). The text before it is finalised as its own
  message first (invariant 4's flush before you interleave), and the
  continuation streams as a message of its own after it. Ctrl+O shows the
  note under the marker (`reconnect_full_lines`).
- **The status line** — the amber `retrying {attempt}/{max}` clause, set by
  the event and cleared by the continuation's first content (or replaced by
  `Waiting for internet…` if the connect fails, `TurnStatus::recovered`).
- **A thought** the drop cut off settles into its `Thought for …` cell first;
  a `write` preview it cut off leaves the strip.
- **Subagents** carry on the same way, on their own transcripts and session
  views (`AgentRun::apply`, a declared flush point). The answer the lead is
  handed is the reply joined back into one (`RunRecorder`, below).

## What the record keeps

`HistoryItem::Reconnect { host, cause, note, timestamp }`. The derived
context replays `note` verbatim as a user-role message exactly where the
model read it (`TextOrigin::Notice`, an app notice on `/context`), and
nothing when there was no note — so the next turn's context is what the
continuing round was sent, and the prompt cache's retained prefix
(`llm::wire_history`) still matches. The rollout keeps it as a `reconnect`
record, the note omitted when absent; older builds skip the unknown type. A
`/compact` turn's drop records nothing: its request is not the
conversation's (the hook-note rule, `docs/compact.md`).

Two readers want the reply whole rather than as the wire carried it, and
both join the halves with `llm::agent::join_resumed`: `/copy` (the last
response, `App::last_assistant_text`) and a subagent's answer. The join
takes out the longest opening of the continuation that the cut-off text
already ends with and that starts a sentence or a line there — the repeat
the note asked for — and otherwise joins the two as they came. `I saw the` +
` theater` stays `I saw the theater`: the `the` that `theater` opens on does
not start a sentence there, so it is no repeat.

## What stays as it was

- A failure before any content: the bounded retry. A connection that cannot
  be made: the offline wait.
- An in-band error frame after content (a provider's `overloaded` mid-reply)
  is surfaced, never carried on — the provider said no.
- **Error retry** `0` surfaces a drop mid-reply at once.
- A stream a server closes **cleanly** before its last frame, on a body with
  no length and no chunking, still reads as the reply's end: the transport
  sees an ordinary end of file.
- Noticing a dead link still takes the keepalive's minute; a connection that
  goes silent without dying takes `NET_OP_TIMEOUT` (120 s), as before.

## Testing

- `llm::tests`: a body read's failure is `ConnectionLost` naming the host, its
  message the socket's words rather than the wrappers', an empty deepest layer
  falling back to the chain.
- `llm::openai::tests`: a real loopback server that answers `200`, streams a
  frame and closes mid-chunk is a `ConnectionLost` naming `127.0.0.1:{port}`,
  the frame before it delivered.
- `llm::retry::tests`: before content a lost connection takes the bounded
  retry; after content it is resumed within the budget, never with retries
  off, and nothing but a lost connection is.
- `llm::agent::tests`: a dropped round carries the turn on with the partial
  and the note in the next request; with nothing visible kept the same
  request goes out again; the note asks for a cut-off call; `Stop` fires only
  for the round that finishes; a cancel after a drop is silent; the joins.
- `app::tests::turn`: the marker lands between the cut-off text and the
  continuation; the retry clause until content; a cut-off call leaves no
  draft and no cell; a compact turn records nothing; `/copy` joins the halves.
- `context::tests`, `session::tests`: the note replays where it was read
  (nothing without one); the record round-trips with and without its note.
- `ui::tests::message`: the marker inline, the note in Ctrl+O only, a long
  cause wrapped under its column.
- `agents::tests`: a subagent's drop on its own transcript, the preview gone,
  the declared flush point, the answer joined across the seam.
- `scripts/smoke.sh` **Phase 145** drives the real binary against a stub
  provider whose first answer streams half a reply and dies mid-chunk: the
  half stays, the marker and `retrying 1/3` show, the stub records the
  continuing request carrying the half and the note, the continuation
  streams and the turn settles; Ctrl+O shows the note. A second turn's
  connection dies mid-thought, and the identical request goes out again with
  no note.

## Where the pieces live

| Piece | Where |
| --- | --- |
| `LlmError::ConnectionLost`, `body_read`, the root cause | `src/llm/mod.rs`, called from `src/llm/openai.rs` (`run_transport`) |
| The policy: `resume_number` | `src/llm/retry.rs` |
| The round: `stream_round`, the drop count, the backoff | `src/llm/backend.rs` |
| The loop: `RoundOutcome::Dropped`, `resume_note`, `join_resumed` | `src/llm/agent.rs` |
| The event | `src/stream/event.rs` (`StreamEvent::ConnectionLost`) |
| The record | `src/app/types.rs` (`Reconnect`), `src/app/turn.rs` (`record_connection_lost`), `src/app/mod.rs` (`last_assistant_text`) |
| The context, the rollout | `src/context.rs`, `src/session.rs` |
| The marker | `src/ui/message.rs`; the colour and the words in `src/ui/theme.rs` |
| The boundary arms | `src/tui/stream.rs`, `src/tui/agent.rs` |
| Subagents | `src/agents.rs` (`AgentRun::apply`, `RunRecorder`) |
