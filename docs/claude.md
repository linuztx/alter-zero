# Anthropic as a provider

Talk to Claude directly — `/login` → **Use an API key** → **Anthropic**, or
`/login` → **Use a subscription** → **Anthropic Console**.

Two providers ship, and they are the *same API* reached two ways:

| provider id | how you authenticate | what is stored |
| --- | --- | --- |
| `anthropic` | a key pasted from the Console | the key, verbatim |
| `anthropic_console` | a browser sign-in (OAuth PKCE) | a **refresh** token |

Both speak the **Messages** API (`wire_api = "anthropic"`), the third wire
format after Chat Completions and Responses (`docs/chatgpt.md`).

## What this is not: the Claude Pro/Max sign-in

There are two Anthropic OAuth clients, and conflating them is the whole trap.
This feature implements the **Console** one — the flow Anthropic's own `ant`
CLI runs, whose tokens are documented as valid on `/v1/messages`, and whose
usage bills to the account's API organisation exactly as a pasted key does.

It deliberately does **not** implement signing in with a Claude Pro/Max
subscription. Anthropic's usage policy is explicit
([code.claude.com/docs/en/legal-and-compliance](https://code.claude.com/docs/en/legal-and-compliance),
*Usage policy → Authentication and credential use*):

> **OAuth authentication** is intended exclusively for purchasers of Claude
> Free, Pro, Max, Team, and Enterprise subscription plans and is designed to
> support ordinary use of Claude Code and other native Anthropic
> applications.
>
> **Developers** building products or services that interact with Claude's
> capabilities … should use API key authentication through Claude Console or
> a supported cloud provider. Anthropic does not permit third-party developers
> to offer Claude.ai login into their own applications, or to route requests
> through Free, Pro, or Max plan credentials on behalf of their users.
> Moreover, developers may not collect, store, or intermediate Claude.ai
> credentials or session tokens — sign-in to a Claude account must complete
> through Anthropic's own flow.

It is also enforced server-side (a subscription token presented by anything
but Claude Code earns `401 OAuth authentication is currently not supported`),
and making it work at all would mean impersonating Claude Code: its client id,
its `User-Agent`, and a mandated first system block reading *"You are Claude
Code, Anthropic's official CLI for Claude."* That last one is why
`src/llm/anthropic.rs` carries a test named
`no_client_identity_is_ever_injected_into_the_system_prompt` — the system
array this app sends is the user's own prompt and nothing else.

Where the other two subscriptions (`docs/copilot.md`, `docs/chatgpt.md`) sit
on the "a client signing a user in to their own seat" side of that line, this
one would not, so it is not built. The Console sign-in gives the same
one-keystroke `/login` experience without it.

## What a provider file says

```toml
[providers.anthropic]
name = "Anthropic"
wire_api = "anthropic"                 # ← a different request shape
api_key_env = "ANTHROPIC_API_KEY"

[providers.anthropic.extra_headers]
anthropic-version = "2023-06-01"

[providers.anthropic.kwargs]
api_base = "https://api.anthropic.com/v1"

[providers.anthropic_console]
name = "Anthropic Console"
auth = "anthropic_console"             # ← a sign-in, not a pasted key
wire_api = "anthropic"
description = "Sign in with your Anthropic account (billed to your API org)"
api_key_env = "ANTHROPIC_CONSOLE_REFRESH_TOKEN"

[providers.anthropic_console.extra_headers]
anthropic-version = "2023-06-01"

[providers.anthropic_console.kwargs]
api_base = "https://api.anthropic.com/v1"
```

`anthropic-version` is the API's own version pin and rides the *file*. The
`anthropic-beta: oauth-2025-04-20` an OAuth bearer additionally needs does
not: it belongs to the **credential** (a pasted key is refused *with* it, and
an OAuth bearer refused *without* it), so the auth seam attaches it.

## Where the credential goes

This is the one wire format whose API key is not a bearer:

| | header |
| --- | --- |
| pasted key | `x-api-key: sk-ant-…` |
| OAuth bearer | `Authorization: Bearer …` + `anthropic-beta: oauth-2025-04-20` |

Sending both is refused outright, so they are alternatives rather than a
fallback pair. `auth::request_auth` decides from the **(scheme, wire format)
pair**, not either half — an Anthropic key reaching an OpenAI-compatible shim
is still a bearer, and an OpenAI key never becomes an `x-api-key`. The
`RequestAuth::headers` field already existed for exactly this shape of answer,
so no new plumbing was needed.

## The sign-in flow

`src/llm/claude.rs`, the third sign-in after Copilot's device flow and
OpenAI's loopback, keeping their two-layer split exactly: a long-lived token
in the `.env` key store, exchanged (and cached in memory, never written) for
the short-lived credential the API takes.

| | value |
| --- | --- |
| client id | `41077d10-94b8-4194-be48-d251e9eb21b4` |
| authorize | `https://platform.claude.com/oauth/authorize` |
| token (both grants) | `https://api.anthropic.com/v1/oauth/token` |
| scopes | `user:profile user:inference user:developer` |
| loopback redirect | `http://localhost:{ephemeral}/callback` |
| hosted redirect | `https://platform.claude.com/oauth/code/callback?app=anthropic-cli` |

Every constant was read from Anthropic's own published CLI source
([`anthropics/anthropic-cli`, `pkg/cmd/cmd_auth.go`](https://github.com/anthropics/anthropic-cli/blob/main/pkg/cmd/cmd_auth.go)).
There is no third-party registration path for this API, so a client that signs
a user in to *their own* Anthropic account uses that public client id — the
same posture `docs/chatgpt.md` documents for Codex's.

Four details are load-bearing and none of them is guessable:

1. **The two grants disagree about everything.** The code exchange is
   form-encoded and sends *no* `anthropic-beta`; the refresh is JSON and
   *requires* one. Swap either and the endpoint routes the request to a
   handler that refuses it.
2. **The hosted redirect keeps its query string.** `?app=anthropic-cli` is
   part of the registered redirect URI verbatim; an authorize call that drops
   it is refused at the authorize step, not the exchange.
3. **The port is ephemeral.** Unlike OpenAI's allow-listed 1455/1457, this
   client pins no port — so a loopback listener is essentially always
   available, and the hosted paste-a-code page is a fallback rather than a
   failure.
4. **`state` is a value of its own**, not the verifier. (Some community
   write-ups of the *other* flow reuse the verifier as `state`; doing that
   here would publish in the URL the one secret the challenge exists to keep.)

**Refresh tokens rotate.** The same fix `docs/chatgpt.md` describes applies:
`claude::persist_refresh` writes the new value straight back into the `.env`
store through a path `tui::models` hands the module once at startup, and an
in-memory rotation map keeps the live `ModelConfig` — which still holds the
token the session started with — refreshing successfully. Skipping either is
not a crash but a forced re-login at the next launch.

The token's cached life comes from `expires_in` (a *duration*, so a skewed
clock cannot make a token look already dead) minus a five-minute skew. For
short-lived tokens the floor is capped at half their remaining lifetime, up
to sixty seconds; an explicitly expired token is never cached. Concurrent
misses share one refresh, and all retired aliases point to the newest token.
Rotation and login share the atomic, serialized key-store updater described
in `docs/chatgpt.md`, preserving other providers' credentials.

A sign-in that granted no inference scope is refused **at the sign-in**
(`scopes_allow_inference`), because the alternative is a token that
authenticates perfectly and then fails every turn with an error that says
nothing about the sign-in that caused it. An *absent* scope list passes —
absence is not evidence of a bad token.

## The wire format

`src/llm/anthropic.rs` translates in both directions between the Messages API
and the Chat Completions currency the rest of the crate uses, exactly as
`src/llm/responses.rs` does for the Responses API. Nothing above
`OpenAiClient` learns that a third format exists: the agent loop, the
transcript, the rollout and the derived context are untouched.

Six shapes differ, and each is a place a naive port breaks:

- **There is no `system` role.** The system prompt is a top-level `system`
  array of text blocks, hoisted out of `messages` entirely.
- **There is no `tool` role either.** A tool result is a `tool_result`
  *content block* inside a **user** message — and every result of one parallel
  batch must ride **one** such message, since splitting them teaches the model
  to stop calling tools in parallel. That is why `build_messages` merges
  consecutive same-role messages rather than emitting one per `ChatMessage`.
- **A tool call is a content block**, and its `input` is a JSON **object**
  where Chat Completions carries a JSON *string*.
- **An image is a `source` object**, so a `data:` URL is split back into its
  media type and payload. An `http(s)` URL becomes a `url` source; anything
  else drops the part rather than 400ing the whole turn.
- **`max_tokens` is required** — `anthropic::MAX_TOKENS` (32 000), under the
  smallest current output cap (Haiku 4.5's 64K) and roomy enough for the long
  file writes an agentic turn makes.
- **Thinking is spelled two ways** — see below.

### `temperature` is never sent

The sampling parameters were removed from every current Claude model, where
sending one is a 400 that blames the request rather than the field. The
`/settings` **Temperature** row therefore has no effect on this provider; the
row still cycles (it is a session-wide value that other providers honour), but
the Anthropic payload builder omits it unconditionally. A test pins that.

### Thinking

`ThinkingMode` already tells the two forms apart without a second lookup:

| mode | wire |
| --- | --- |
| `Off` | `thinking: {"type": "disabled"}` |
| `On` | `thinking: {"type": "enabled", "budget_tokens": 10000}` |
| `Effort(e)` | `thinking: {"type": "adaptive", "display": "summarized"}` + `output_config: {"effort": e}` |

`On` is produced *only* for a reasoner whose `/v1/models` record listed no
effort levels (`ReasoningSupport::modes`), which on this provider is exactly
the older budgeted family — so the mode that means "the model's own default"
maps onto the form that family takes, and the effort ladder maps onto the one
the current models take. The level rides `output_config`, **not** `thinking`:
inside the thinking object it is an unknown-field 400.

`display: "summarized"` is explicit and load-bearing. The current models
default it to `omitted`, which streams thinking blocks whose text is empty —
a live `● Thinking…` cell that shows a long silent pause and then nothing
(`docs/thinking-stream.md`).

### Prompt caching

Caching on this API is always explicit, so — unlike the OpenRouter path — it
needs no model-id sniff (`docs/prompt-caching.md`). `apply_cache_breakpoints`
places at most three of the four allowed `cache_control` breakpoints, on the
same rule `llm::cache` uses for the chat-completions shape: the last **system**
block, the **frontier** (the last content block of the last message), and the
last block of the **user message before it**.

### The stream

`MessageAccumulator` folds the SSE stream into the crate's `StreamOutcome`.
The round is *stateful* where the Responses API's is not — a `tool_use` block
opens with its id and name on `content_block_start`, accumulates its arguments
as `input_json_delta` **partial JSON strings** over any number of frames, and
closes on `content_block_stop` — so the classifier is a fold rather than a
pure `parse_event`, and the in-flight blocks are keyed by **content-block
index**: a parallel batch interleaves them, and a single "current call" would
splice one call's arguments into another's.

Three things it gets right that are easy to get wrong:

- **Usage is summed, not read.** Anthropic reports `input_tokens` as the
  *uncached remainder*; the prompt's real size is that plus
  `cache_read_input_tokens` plus `cache_creation_input_tokens`. Reading the
  single field makes a well-cached session report a few dozen tokens against a
  1M window.
- **`message_delta`'s counters are cumulative** — and it repeats the input
  side too (verified live:
  `{"input_tokens":10,"cache_creation_input_tokens":7998,"cache_read_input_tokens":0,"output_tokens":5}`),
  so the merge is **monotonic per counter**: a later frame can only report
  more, and one that omits a counter leaves what an earlier frame said. That
  is what keeps a delta naming the remainder *without* the cache keys (the
  older documented shape) from zeroing the cached share and shrinking the
  whole-prompt `input` to the remainder on the receipt and the gauge.
- **A refusal is an HTTP 200.** `stop_reason: "refusal"` with nothing streamed
  fails the turn with a reason; after text has streamed the partial *is* the
  answer and the turn ends normally.

## Model metadata

The listing answers all three capability questions natively — no hardcoded
table:

| `ModelEntry` field | `/v1/models` record |
| --- | --- |
| `context` | `max_input_tokens` |
| `vision` | `capabilities.image_input.supported` |
| `reasoning` | `capabilities.thinking` + `capabilities.effort` |

Two traps the sniffs are written around:

- **The window is `max_input_tokens`.** There is no `context_length` and no
  `context_window` on this list, and the `max_tokens` sitting beside it is the
  *output* cap — reading the wrong one gauges a 1M window against 128K.
- **The default page is 20.** `models_url` asks for `limit=1000` (the
  endpoint's own maximum, comfortably one page); without it the catalog
  silently truncates to whichever models sort first, with no error anywhere.
  That page size keys on the **wire format**, not the auth scheme, because the
  pasted-key provider is an ordinary `AuthScheme::ApiKey`.

`capabilities.effort` is the **Ctrl+T ladder itself** — a per-model list of
which rungs the model accepts — making Anthropic the third provider to name
one natively, after Copilot's `supports.reasoning_effort` and the ChatGPT
backend's `supported_reasoning_levels` (`docs/reasoning.md`). A record with
`thinking.supported` but no effort levels is an on/off-only reasoner, which is
the same shape Venice's models already had.

The **`Off` rung is gated too**, on `thinking.types.disabled.supported`: the
models whose thinking is always on answer `{"type": "disabled"}` with a 400,
and Ctrl+T must not be able to reach a mode that fails every request. Where
the record doesn't name the leaf the permissive answer stands, so nothing
changes for a model that simply doesn't say.

## Errors worth explaining

`claude::auth_advice` rewrites the refusals a user can act on, and the point
is that they need *different* answers — "sign in again" is wrong for three of
the four:

| what happened | what it says |
| --- | --- |
| `OAuth authentication is currently not supported` | sign in again, **or use an API key** — retrying cannot fix a policy block |
| 401 / `invalid_grant` / `invalid_token` | run `/login` and sign in again |
| 403 / `permission_error` | check the account's API access — a scope, not an expiry |
| `enforced_spend_limit_reached` | raise the limit in the Console, or switch provider |
| 429 | rate-limited; wait or switch provider |

The pasted-key provider gets the same mapping, minus the sign-in-shaped
advice, since the module tells the cases apart by status and code rather than
by which provider asked.

## Environment

| variable | effect |
| --- | --- |
| `ANTHROPIC_API_KEY` | the pasted key (also read from the `.env` store) |
| `ANTHROPIC_CONSOLE_REFRESH_TOKEN` | the sign-in's refresh token |

Both resolve the ordinary way (`docs/llm.md`): a real process environment
variable wins over the `.env` store.
