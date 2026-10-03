# Waiting for a lost connection

A request that could not reach the provider used to fail the way every other
request failed: three retries half a second apart, then a red `request
failed: error sending request for url (…)` and a dead turn — on a train, at
the edge of the Wi-Fi, through a VPN that was reconnecting. Now a lost
connection is **waited for**: the backend re-sends the request every few
seconds for as long as it takes, the status line says what it is doing, and
the turn carries on by itself the moment the network is back.

```
❯ refactor the parser

⣤⣀⣀⣀⣀⣀⣀⣀ Waiting for internet… (2m 3s · ↑ 42 tokens · offline for 1m 10s · esc to interrupt)
  ⎿  ((·))  No connection to api.venice.ai — trying again · 14 attempts

────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────
```

The verb and the `offline for` clause are amber, the verb under the usual
shimmer; the `((·))` ripples out from its dot and back once a second, fading
with distance; the host is lit, the rest of the row dim. When the request
gets through, all of it goes and the reply streams as if nothing happened.

## Two kinds of failure, two policies

The retry policy (`llm::retry`, `docs/llm.md`) used to know one kind of
failure: a *retryable* one, worth a few more attempts before the error is
shown. That is the right policy for a host that **answers badly** — a `503`,
a `429`, a connection reset mid-exchange, a request that stalls past the
per-operation timeout. Each attempt costs a real request and the budget
exists so a provider having a bad minute does not get hammered forever; past
it, the error is information the user needs.

A host that **cannot be reached at all** is a different thing. Nothing was
sent, nothing is being hammered — the failed connect cost one SYN or one
DNS lookup — and the error is not information: the user already knows the
Wi-Fi dropped. What they want is for the turn to still be there when it
comes back. So that failure gets no budget and no deadline:

- **Classified at the seam that still has the typed error.** Every place a
  `reqwest` send fails goes through `LlmError::transport(&e)`
  (`src/llm/mod.rs`): `e.is_connect()` — the name would not resolve, the TCP
  connect was refused or timed out, the TLS handshake failed, the proxy in
  front of it could not be reached — becomes
  `LlmError::Unreachable { host, message }`, naming the host (and the port,
  when the URL spells one: `127.0.0.1:11434`) the request was for; anything
  else stays `LlmError::Http`. The seam is the streaming transport
  (`openai::run_transport`), the `/models` fetch, and the three sign-in
  exchanges (Copilot, ChatGPT, Claude), so a bearer mint on a machine that is
  offline waits like the request it was for. Both variants carry the
  error's **whole cause chain**: `reqwest`'s own `Display` stops at `error
  sending request for url (…)`, and the part a user can act on — `Connection
  refused`, `failed to lookup address information` — is two sources down.
  Classifying by `is_connect` rather than by matching that text is what
  keeps the rule honest across versions of the transport; the text is kept
  for the user, and for Ollama's advice below.
- **…and checked against the host when the transport will not say.** The
  chat request streams its body out of a pipe (`docs/memory.md`), and the
  first smoke run found what that does to the error: when the connect fails,
  the body pump loses its receiver *first*, and what `reqwest` returns is a
  **body** error — `request or response body error … send failed because
  receiver is gone` — with no connect error anywhere in its chain, so
  `is_connect()` is false and the refusal would have taken the bounded
  retry. `openai::Reachability` answers it: a send failure the transport does
  not blame on the connect is followed by one bodyless `GET` of the request's
  **origin**, through the same client (same proxy, trust roots and pool).
  A host that answers at all — any status — is there, and the failure keeps
  the bounded retry; one the probe cannot connect to is `Unreachable`, with
  the probe's own cause as the message. A plain connect failure (the
  `/models` fetch, the sign-in exchanges, whose bodies are in memory) never
  probes. The probe runs only on a send that already failed, so a working
  connection costs nothing; offline it fails as fast as the request did.
- **Waited for by the driver.** `retry::next_step` reads an `Attempts` pair —
  bounded retries spent, connection attempts failed in a row — and answers
  an `Unreachable` with `RetryStep::AwaitConnection { attempt, wait, host }`:
  `run_attempts` announces `StreamEvent::Offline { host, attempts }`, sleeps
  `offline_backoff(attempt)` (1 s, 2 s, 4 s, then the 5 s
  `OFFLINE_BACKOFF_CAP` for every attempt after — short, because a failed
  connect costs nothing while the network is gone and the turn should pick
  up within seconds of its return), and tries again. **None of it counts
  against the bounded budget**: a hundred failed connects later the answer is
  still to wait, and the `retrying 1/3` that follows a `503` once the host
  is back opens on its first number. A host that answers at all, even with
  an error, resets the outage count, so the next outage counts from one.
- **The same two gates as a retry.** A failure after content has streamed is
  surfaced, never re-sent — restarting the request would duplicate the
  streamed text — so a connection that drops *mid-reply* still ends the turn
  with the red notice and the partial; only the request that got nothing
  through waits. (In an agentic turn that is every round's request: a tool
  round that comes back to find the network gone waits, and the turn
  continues where it was.) And `/settings` **Error retry** at `0` means
  *never retry*, the wait included: the `could not reach {host}: …` error
  surfaces at once, in the words of the transport.
- **Esc is the way out**, as for any wait: the sleep is `sleep_cancellable`'s
  50 ms slices, so the interrupt lands at once, and the cancel is silent.
  Then the ordinary interrupt rule applies (`docs/interrupt.md`): with nothing
  streamed and nothing queued the submission is **undone** — the message is
  back in the composer, no notice — so a turn abandoned while offline costs
  nothing to re-send later.

## What shows

Everything on screen comes from one piece of pure state,
`TurnStatus::offline: Option<OfflineInfo { host, attempts, began }>`
(`App::set_offline`, from the event; `AgentRun::offline` for a subagent,
`agents::AgentRun::apply`), cleared the moment the host answers —
`TurnStatus::recovered`, called by every content arm (`push_chunk`,
`push_thinking`, `push_tool_call_progress`), by the round announcements
(`start_tool_batch`/`start_tool`: the wires that deliver a tool call whole
stream no delta ahead of it), and by `set_retry` (a retry means the host
answered). `retry` and `offline` are never `Some` together: the newest
announcement is the state.

- **The verb.** `ui::styled_status_line` wears `OFFLINE_VERB` — `Waiting for
  internet` — in the verb slot, over the turn's own verb *and* over a task's
  `activeForm`: the activeForm says what the model is doing, and while the
  host is gone it is doing nothing but waiting. The shimmer is the usual one
  with its resting colour swapped for `status_offline_color()` (the theme's
  `warning`, the retry clause's amber), so the state is told apart by colour
  as well as by word, and the turn's underlying verb keeps rotating
  underneath — the committed summary still reads `Worked for 3m 2s`.
- **The clause.** `offline for 45s`, in the retry clause's slot and colour.
  It is one subtraction, not a second clock: `OfflineInfo::began` is the turn
  clock's reading (`TurnStatus::elapsed`, injected every frame by the draw
  tick) when the first `Offline` announcement landed, and
  `OfflineInfo::duration(elapsed)` is how long ago that was. Later
  announcements move `attempts` on and keep `began`. The boundary learns
  nothing new and keeps no new `Instant`; an agent's `began` is its own
  per-frame `runtime` the same way.
- **The row.** `ui::offline::offline_lines` builds `  ⎿  ((·))  No connection
  to {host} — trying again · {n} attempts` in the gutter a tool's output
  hangs from: the ripple frame for the moment (`OFFLINE_FRAMES`, six
  fixed-width frames one `OFFLINE_FRAME_INTERVAL` apart, phase-driven by the
  same clock as the status line so the two animate in step), the dot in the
  amber and each ring out faded `OFFLINE_RING_FADE` further toward the dim,
  then the sentence, wrapped to the width under its own column with the host
  lit. The sentence is deliberately short — the verb already says the turn
  is waiting — so it fits one 80-column row beside an ordinary host; a long
  host or a narrow terminal costs rows, never the tail. It takes the slot
  under the status line through `ui::hang_rows` — the one sum
  `live_height`/`live_layout`/`cursor_position` size by, now the
  checklist's, the tip's and this row's — so the rows reserved and the rows
  `strip_lines` paints agree by construction. The tip gives way while the
  row shows (`App::tip`: a shortcut hint beside a network outage is noise,
  and the same tip comes back, unspent, once the request gets through); the
  task checklist does not — a plan's rows and the network's state are both
  worth a row, and `hang_rows` sums them.
- **Live only**, like the tip and the retry clause: nothing commits, the
  rollout and the Ctrl+O transcript never see it, and a resize rebuilds it
  from state. Inside an agent session view the strip is that agent's, so
  `App::offline` answers the viewed agent's wait there and the main turn's
  everywhere else.

## What a local server is not

A connection refused by `127.0.0.1:11434` is not the network: it is
`ollama serve` not running. The Ollama wire already turned that refusal into
advice (`ollama::connection_advice`, `docs/ollama.md`), and waiting on it
forever would hide the one sentence that fixes it. So `OpenAiClient::explain`
maps an `Unreachable` back to an `Http(advice)` — the bounded retry, the
advice surfaced in seconds — **only when the base is on this machine**
(`ollama::is_local_base`: a loopback address or `localhost`). Ollama's cloud
and an `OLLAMA_HOST` elsewhere keep the wait. No other wire makes the
distinction: a chat-completions provider pointed at `localhost` (a local
proxy, LM Studio) that is down shows `Waiting for internet…` over `No
connection to localhost:8080`, which names what is actually gone.

## What stays as it was

- The bounded retry: `is_retryable`, `retry_backoff`, the `retrying n/N`
  clause and the `Error retry` knob behave exactly as before for a host that
  answers.
- A stall: a provider that accepts the connection and sits on the headers
  past `NET_OP_TIMEOUT` (120 s) is a bounded, counted retry — it answered,
  slowly.
- A drop mid-reply is surfaced, never re-sent (above).
- The `/model` picker's fetch does not retry; its failure now reads `could
  not reach {host}: …` instead of `request failed: error sending request for
  url (…)`.

## Testing

- `llm::tests`: a refused loopback connect classifies as `Unreachable` naming
  `127.0.0.1:{port}` with the cause chain kept (offline-safe — the kernel
  refuses it at once); a request that fails before any socket (an unsupported
  scheme) stays `Http`; a body read's `io::Error` names its cause chain;
  `host_label` keeps only an explicit port; the `Display`.
- `llm::openai::tests`: a **streamed** request to a refused port is
  `Unreachable` (the smoke finding, reproduced offline); a send failure on a
  host that answers the probe keeps `Http`; one whose host cannot be reached
  is `Unreachable` with the probe's own cause; a failure `reqwest` blames on
  the connect needs no probe.
- `llm::retry::tests`: `next_step` answers `Unreachable` with
  `AwaitConnection` at every budget state, `Proceed` once content streamed or
  with retries off; the backoff doubles to the cap; the driver announces one
  `Offline` per failed connect with the running count, spends no retry on
  them, opens the bounded retry on `1` after an outage and restarts the
  outage count after an answer, stops silently on a cancel mid-wait, and
  surfaces the error with retries off.
- `llm::openai::tests`: a refused local Ollama is advice, a remote one is
  waited for; `ollama::is_local_base`.
- `app::tests::turn`: `set_offline` records the host, the count and the
  clock reading; later announcements keep the start; every content arm and
  both round announcements end the wait; `retry` and `offline` never show
  together; idle is a no-op. `app::tests::tips`: the tip hides under the
  wait and returns unspent. `app::tests::agent` / `agents::tests`: a session
  view reports the viewed agent's wait, stamped off its runtime.
- `ui::tests::status`: the verb, its amber base, the clause, the activeForm
  losing the slot. `ui::tests::offline`: the row's text and the singular
  count, the frames' equal widths, the ripple's sequence, loop and fade, the
  wrap under the text column, the host lit, the strip reserving exactly the
  row it paints (and the region settling back on content), the agent view's
  row.
- `scripts/smoke.sh` **Phase 129** drives the real binary against a provider
  file pointing at a loopback port nothing listens on: the turn shows
  `Waiting for internet…`, the `offline for` clause counting and the row
  naming `127.0.0.1:{port}` with its attempts ticking; then a stub
  chat-completions server starts on that port and the reply streams, the
  turn settling as any other. A second half interrupts the wait with Esc and
  finds the message back in the composer. To see it against a real provider,
  run under `HTTPS_PROXY=http://127.0.0.1:1` (every connect refused) — or
  behind a local forwarding proxy you can stop and start.

## Where the pieces live

| Piece | Where |
| --- | --- |
| The classification, `LlmError::Unreachable`, `host_label`, the cause chain | `src/llm/mod.rs` (`LlmError::transport`), applied in `openai.rs`, `models.rs`, `copilot.rs`, `chatgpt.rs`, `claude.rs` |
| The policy: `Attempts`, `RetryStep::AwaitConnection`, `offline_backoff`, the driver | `src/llm/retry.rs` |
| Ollama's exception | `src/llm/ollama.rs` (`is_local_base`), `src/llm/openai.rs` (`explain`) |
| The event | `src/stream/event.rs` (`StreamEvent::Offline`) |
| The state | `src/app/status.rs` (`OfflineInfo`, `TurnStatus::offline`, `set_offline`, `offline`, `recovered`), `src/agents.rs` (`AgentRun::offline`) |
| The boundary arm | `src/tui/stream.rs` |
| The status line | `src/ui/status.rs`; the consts and colours in `src/ui/theme.rs` |
| The row | `src/ui/offline.rs`; summed into the strip by `src/ui/layout.rs` (`hang_rows`) and painted by `src/ui/live.rs` |
| The tip giving way | `src/app/tips.rs` |
