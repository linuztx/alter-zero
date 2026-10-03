# Waiting out a lost connection

A request that cannot reach the provider because the machine has no network
no longer fails the turn. The backend waits for the connection to come back
— with **no deadline** — re-sending the request on a gentle cadence, and the
status line says what is happening instead of counting `retrying 1/3 … 3/3`
down to a red `request failed: error sending request for url (…)`:

```
❯ refactor the parser

▂ ▄ ▆ █ Waiting for internet… (1m 12s · ↑ 1.2k tokens · offline 41s · retrying in 3s · esc to interrupt)

─────────────────────────────────────────────────────────────────────────────
❯
─────────────────────────────────────────────────────────────────────────────
```

The four signal bars fill one by one in amber and empty again — a phone
searching for signal — while `Waiting for internet…` shimmers in amber. The
clause after the tokens says how long the connection has been gone and when
the next attempt goes out; while an attempt is in flight it reads
`reconnecting…`. The moment the provider answers, the line goes back to the
turn's own spinner and verb, a `Back online after 41s offline` toast shows
above the box, and the reply streams as if nothing happened. Esc works
exactly as it always does: nothing has streamed, so it **undoes** the
submission and puts the prompt back in the composer (`docs/interrupt.md`).

## What counts as offline

Only a failure that happens **before a connection exists** — and so before a
single byte of the request left the machine. Re-sending such a request can
never duplicate output or bill twice, which is what makes an unbounded wait
safe. Measured against `reqwest` 0.12's real error chains
(`hyper-util` 0.1 underneath):

| failure | chain (top → root) | offline? |
| --- | --- | --- |
| DNS lookup failed | `client error (Connect)` → `dns error` → `failed to lookup address information: …` | **yes** |
| the same through `HTTPS_PROXY` | `client error (Connect)` → `tunnel error: failed to create underlying connection` → `dns error` → … | **yes** |
| no route / network down | … → `tcp connect error` → io `NetworkUnreachable` / `HostUnreachable` / `NetworkDown` | **yes** |
| no local address | … → io `AddrNotAvailable` | **yes** |
| connect timed out (10 s) | `is_connect()` + `is_timeout()` → `tcp connect error` → `deadline has elapsed` | **yes** |
| connection refused | … → `tcp connect error` → io `ConnectionRefused` | no — the host answered, the network works |
| TLS, an HTTP status, a proxy refusing the tunnel | — | no |
| a read that fails or stalls after the headers | `is_connect()` false | no |

The refused case is deliberate: a `RST` is the remote host talking, so the
network is up and the problem is the server — an Ollama that isn't running
gets its `is ollama serve running?` advice at once, as before. Everything in
the *no* rows keeps the ordinary budgeted retry (`docs/llm.md`).

`reqwest`'s `Display` names none of this — it is `error sending request for
url (…)` whatever went wrong, the cause living only in `source()`. So the
classification walks the chain (`llm::network::is_offline`, pure, unit-tested
against hand-built chains of `io::Error`s and message links), and the error
the user sees on a *surfaced* failure now carries the whole chain too
(`llm::network::describe`: `request failed: error sending request for url
(…): client error (Connect): tcp connect error: Connection refused (os error
111)`). That also fixes `ollama::connection_advice`, which matched
`connection refused` and `dns error` against a message that never contained
either.

The boundary helper `llm::transport_error(&reqwest::Error)` makes the call —
`LlmError::Offline` or `LlmError::Http` — at every place a turn can meet the
network: the streaming transport shared by all four wire formats, and the
per-request token refreshes behind the three sign-ins (Copilot's exchange,
ChatGPT's and Anthropic's refresh grants), so a subscription that goes
offline at its token refresh waits like a pasted key does.

### Two failures that do not say they are offline

Driving the real backend through a network that dies (below) turned up two
cases the chain walk alone misses, both of them the common case rather than
the corner:

- **A sizeable request hides its own DNS failure.** `reqwest`'s blocking
  client pumps a streamed body itself, and when the connection fails before
  the upload is done it reports the pump's broken channel — `request or
  response body error … send failed because receiver is gone` — instead of
  the connection's error. A one-line prompt wins the race and surfaces the
  real `dns error`; a conversation carrying the tool schemas never does. So
  the transport treats that one message (`llm::network::is_unsent_body`) as
  a question rather than an answer, and asks a fresh connection: if even a
  probe cannot connect, the failure is `Offline`.
- **The outage that lands mid-turn hits a pooled connection.** Rounds of an
  agentic turn are seconds apart, so the next round reuses the connection
  the last one left in the pool — no DNS lookup, nothing to fail. A dead
  link reports nothing to the socket waiting on it, so that request would
  sit out the 120 s stall detector before its retry even met the outage.
  Every request therefore runs an **outage watchdog**
  (`openai::watch_for_outage`): while no response headers have come, every
  `OUTAGE_PROBE_AFTER` (10 s) it tries a fresh connection to the same host,
  and the first time that cannot be made either, the request fails as
  `Offline` — sent down the transport channel ahead of the hung read — and
  the turn waits. A probe that gets through means the provider is merely
  slow (a model loading, a big upload), and the watch goes on.

The probe (`openai::probe_connection`) is a credential-less `HEAD` of the
request's own base, on a client of its own (`llm::probe_client`) that keeps
**no connection pool** — the pooled connection may be exactly the one the
outage killed — with the chat client's proxy and trust roots and a 10 s
deadline. It is classified by the same rule as everything else: any answer at
all means the network is there. A shared `RequestPhase` makes the watchdog
and the transport mutually exclusive: either the provider answers first and
the watchdog stands down, or the watchdog fails the request first and the
transport drops whatever arrives later unread. The watchdog's way into the
transport channel lives in that phase rather than with the watchdog, and the
answer drops it on the spot — so a probe still in flight can never hold the
channel open past the response's end, which some drains read as their EOF.

## The wait

`llm::retry` owns the policy, pure like the rest of the module:

- **`next_step`** answers `AwaitNetwork { check, wait }` for an `Offline`
  failure that emitted nothing. It does **not** touch the Error-retry budget —
  the budget is for a server misbehaving, and a lost connection is not that —
  and it has no ceiling. A budget of **`0`** (`/settings` → **Error retry**)
  still means *never retry*, the offline wait included: that one knob is how
  someone who wants every failure surfaced at once gets it.
- **`offline_backoff`** spaces the attempts **1 s → 2 s → 4 s, then every
  5 s**. An attempt against a dead network fails in microseconds (no route)
  or at worst after the 10 s connect timeout, so a short cap costs nothing and
  keeps the gap between the network returning and the turn resuming under
  five seconds.
- **`run_attempts`** announces each wait as `StreamEvent::Offline { wait }`
  before sleeping it (interruptibly — Esc lands within 50 ms), then re-sends.
  The streak ends the first time an attempt fails any other way or succeeds,
  so a later offline spell starts the cadence over at 1 s.

**The retry is the probe.** While waiting, nothing else checks the
connection: there is no request to a third-party host (a privacy statement
this project would have to make, and a host that may be unreachable behind a
corporate proxy when the provider is not), and nothing that can disagree with
the request that actually matters. The real request is the cheapest possible
probe while offline — it fails before its body is serialized past the first
pipe chunk — and when the network is back it is simply the request. The one
other request the feature ever sends is the credential-less `HEAD` of the
provider's own base described above, and only to *recognise* an outage that
a failure did not name, never to wait one out.

**`Connected` comes from the response headers, not the first token.** When
an attempt follows an offline failure, `stream_round` hands the transport a
hook (`OpenAiClient::stream_chat_with`) that fires on the transport thread
the moment `send()` returns a response — any status — and sends
`StreamEvent::Connected`. Waiting for the first streamed token instead would
leave `reconnecting…` on screen for as long as a reasoning model sits on a
hidden chain of thought, which can be a minute. The hook is only armed after
an offline failure, so a normal turn's event stream is unchanged.

## The state and the look

`TurnStatus::offline: Option<OfflineInfo>` holds two instants on the turn's
own clock — `since` (when this streak began) and `next_check` (when the next
attempt goes out) — both read off `TurnStatus::elapsed`, which the boundary
already injects every frame. So the pure renderer derives `offline 41s` and
`retrying in 3s` with no clock of its own (the `set_status_times` pattern),
and the countdown rounds **up**, so it reads `1s` until the attempt actually
fires and then `reconnecting…`.

- `App::set_offline(wait)` opens or extends the streak and drops any
  `retrying n/N` clause — the two describe different problems.
- `App::set_connected()` closes it and returns how long it lasted, for the
  toast; streamed content (`push_chunk`, `push_thinking`,
  `push_tool_call_progress`) and `set_retry` close it too.
- A subagent's `AgentRun` carries the same `offline` field on its own
  `runtime` clock, and `ui::agent_view_status` copies it, so an agent waiting
  for the network says so in its session view exactly like the main turn.

`ui::status::styled_status_line` draws the offline line in place of the
working one while `offline` is `Some` — the session's `/spinner` style and a
task's `activeForm` both give way to it, since neither is true while nothing
is working:

- **The signal** (`OFFLINE_SIGNAL_BARS`, `▂ ▄ ▆ █`): bar *n* lit once
  `n` steps of the `OFFLINE_SIGNAL_STEP` (250 ms) sweep have passed, an
  all-dark beat between sweeps. Lit bars wear `status_offline_color()` — the
  palette's `warning`, the retry clause's amber — and the rest the dim detail
  grey. Every glyph is single-width, like every spinner frame. A session on
  the ASCII `line` spinner gets `. . . .` instead, the font that picked it
  having no block elements to promise.
- **The verb** `Waiting for internet…` carries the ordinary shimmer
  (`shimmer_spans_from`) over the amber base, so it reads as a warning that
  is still alive rather than a frozen error.
- **The clause** `offline {for} · retrying in {n}s` is amber; elapsed, tokens
  and the `esc to interrupt` hint stay dim. The line clamps with `…` like the
  working line.

## Offline demo

The dummy has an `offline` scenario (cue: **offline**): two failed checks
paced by their own announced waits — the real `offline_backoff(1)` and
`offline_backoff(2)`, so the countdown is the real one — then `Connected`, then a
two-part reply narrating what happened and closing on the `/login` →
`/model` hand-off. `pace` sleeps an `Offline` event's `wait`, so the demo
plays at the speed the real backend would. `smoke.sh` Phase 129 drives it:
the amber line, the countdown, the reply, and the `Back online` toast.

The live path was verified against the Agent Zero API (`a0_venice`) by
pointing `HTTPS_PROXY` at a hostname and adding and removing it from
`/etc/hosts` — the lookup failing is a real `dns error` chain, and restoring
the entry let the waiting turn connect and stream. Offline at submission,
with tools off and on: the wait, then `Back online after 8s offline` and the
reply. Mid-turn: a small TCP relay in front of the proxy that can freeze its
open tunnels plays the dead link, so the `bash` round's follow-up goes out
over a pooled connection that never answers — the watchdog caught it ten
seconds in, the wait held through the cadence, and the round finished when
the link came back. Both of the failures above were found that way; neither
showed with a small request or a healthy pool.

## Known limitations

- **A misspelled host waits too.** DNS cannot tell "this name does not
  exist" from "I cannot reach a resolver" in a way that holds across
  platforms — macOS reports the same `EAI_NONAME` for both — so a typo in
  `api_base` or `OLLAMA_HOST` shows `Waiting for internet…` instead of an
  error. Esc stops it, and the very first turn is where such a typo shows.
- **A drop mid-stream is still surfaced.** Once content has streamed,
  re-sending would duplicate it, so a connection lost mid-reply ends the turn
  with the error, as before. The next turn waits if the network is still
  down. The watchdog only watches for the response headers, so a link that
  dies after them but before any content still waits out the stall detector
  first.
- **A request the watchdog gives up on may already have reached the
  provider** — an outage after the upload but before the answer — and the
  re-sent one is then billed again. The stall detector's retry did the same,
  two minutes later.
- **A proxy that loses its own upstream** answers the tunnel with an error
  status; `reqwest` reports a tunnel error rather than anything about the
  network, so that case keeps the budgeted retry.
- The auto-mode classifier's one-shot request and the `/model` catalog fetch
  do not wait: the classifier falls back to asking the user, and the picker
  shows the error.
