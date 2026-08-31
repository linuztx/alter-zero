# OpenAI ChatGPT as a provider

Sign in with a ChatGPT Plus/Pro/Team seat instead of pasting an API key —
`/login` → **Use a subscription** → **OpenAI (ChatGPT)**.

This is the second subscription provider, after GitHub Copilot
(`docs/copilot.md`), and it reuses that feature's whole shape: the `/login`
fork, the `.env` store, the two-token split, the `/model` picker's ✓, the
capability probe. What it adds is a second *sign-in* shape (a browser, not a
code) and a second *wire format* (Responses, not Chat Completions) — and those
two are the whole of the new work.

> **A caveat worth stating.** This API is undocumented and unversioned, and
> the OAuth client id it uses is Codex's own — there is no third-party
> registration path. It has already changed shape twice in public
> (`OpenAI-Beta: responses=experimental` is gone; the `session_id` header was
> renamed). Treat drift as expected. Whether a ChatGPT subscription may be
> used from a non-OpenAI client is a question about OpenAI's terms, not about
> this code.

## What a provider file says

```toml
[providers.openai_chatgpt]
name = "OpenAI (ChatGPT)"
auth = "openai_chatgpt"                          # ← a sign-in, not a pasted key
wire_api = "responses"                           # ← and a different request shape
description = "Sign in with your ChatGPT Plus/Pro account"
api_key_env = "OPENAI_CHATGPT_REFRESH_TOKEN"

[providers.openai_chatgpt.extra_headers]
originator = "codex_cli_rs"

[providers.openai_chatgpt.kwargs]
api_base = "https://chatgpt.com/backend-api/codex"
```

Two enums in `src/llm/config.rs` back those keys, and both **degrade rather
than fail** on a value they don't know (`#[serde(other)]`) — a provider file
written against a newer build must still parse:

| key | type | default | what it decides |
| --- | --- | --- | --- |
| `auth` | `AuthScheme` | `api_key` | which `/login` list the provider is in, and what the `Authorization` header gets |
| `wire_api` | `WireApi` | `chat` | `/responses` vs `/chat/completions` — the URL *and* the body |

`AuthScheme::OpenAiChatGpt` carries an explicit `#[serde(rename)]`:
`rename_all = "snake_case"` spells the variant `open_ai_chat_gpt`, and a
mismatched `auth` value falls silently through to `ApiKey` — a subscription
provider that looks configured and asks for a pasted key instead.

**`wire_api` is deliberately not derived from `auth`.** How you authenticate
and what shape the request takes are two questions: an OpenAI API key can
reach the Responses API too, and a future provider may want either pairing.

## The two token layers

The same split `docs/copilot.md` keeps, with different lifetimes:

| | what it is | lifetime | where it lives |
| --- | --- | --- | --- |
| **refresh token** | what the sign-in mints | long-lived, but **rotating** | `.env`, like any other provider's secret |
| **access token** | the JWT the API takes | ~1 hour | memory only, never written |

`chatgpt::authorize` mints the second from the first and caches it in a
process-global map keyed by the refresh token — a turn's rounds, the `/model`
fetch and every subagent's thread want the same bearer.

Two things make this different from Copilot's exchange:

**The refresh token rotates, and a reused one is terminal.** OpenAI may hand
back a *new* refresh token with each use, and re-presenting the retired one
answers `refresh_token_reused` — which is not recoverable, only re-signable.
So the rotation is written straight back to the `.env` store, in place, via
`chatgpt::persist_refresh`. That write needs a path the minting code does not
have (it runs on a backend thread, several layers below the boundary), so
`tui::models` calls `chatgpt::set_store_path` once at startup. Skipping that
call is not a crash — it is a forced re-login at the next launch, which is
exactly the kind of failure that reads as a different bug.

The disk write is only half of it. The live `ModelConfig` carries the token the
session *started* with and nothing reloads it mid-run, so once that token is
retired the next refresh would present a dead one — killing a working session
about an hour in, right when the first access token expires. A small in-memory
chain (`rotations()`) remembers what each retired token became, so a config
holding the old value keeps refreshing. Signing in again clears it: a fresh
token is unrelated to whatever the old one had rotated into.

**The account id comes from the token itself.** The backend routes on a
`chatgpt-account-id` header whose value is a claim inside the access token
(`https://api.openai.com/auth` → `chatgpt_account_id`). Reading it off the
token each time it is minted is what keeps the store to **one value**: no
second file, no second env var, and no way for the two to drift apart. The
JWT's signature is never verified — these claims route and label a request the
server is going to validate anyway; they grant nothing.

### The cached lifetime, and the clock

Copilot's exchange publishes a `refresh_in` **duration**, and `docs/copilot.md`
explains why keying off the absolute `expires_at` instead re-exchanges on
every request when the user's clock runs ahead. OpenAI publishes only the
absolute `exp`, so that mitigation isn't available — the guard here is a
**floor**: `cache_lifetime` subtracts a five-minute skew from the remaining
life and then clamps to at least sixty seconds. A clock hours ahead costs one
extra mint a minute instead of one per request.

### The seam

`auth::request_auth(cfg)` answers *(bearer, base override, headers)* for any
config. It is the generalization of the `(bearer, base)` pair Copilot's module
used to return, widened because a ChatGPT request needs a **header** the pair
could not carry:

- **`ApiKey`** → the stored key, no override, no headers, and **no I/O at
  all**. Every existing provider's path is byte-identical.
- **`GithubCopilot`** → the exchanged bearer and the account's own host.
- **`OpenAiChatGpt`** → the minted access token, plus `chatgpt-account-id`,
  `user-agent`, and `x-openai-fedramp` when the account needs that edge.

A subscription config with no stored token resolves to nothing rather than
calling out — the `/model` picker builds a config per provider, signed in or
not, and an exchange attempt there would hang the fetch on a request that can
only fail.

The account-id header is attached **conditionally**. A token that carries no
auth claims still authenticates; sending an empty account id is a *different*
(and invalid) routing hint than sending none.

## The sign-in

Not a device flow — OpenAI's is a browser PKCE loopback:

1. Bind `127.0.0.1:1455`, falling back to `1457`. **Neither port is a free
   choice**: OpenAI's redirect allow-list is pinned to those two against this
   client id, so a port of our own is refused at the authorize step. Both
   already bound is a real failure, not a retry.
2. Build the authorize URL (`chatgpt::authorize_url`) and show it.
3. Block until the browser comes back on the loopback, checking `state`.
4. Redeem the code for the token set; keep the refresh token.

The redirect URI names **`localhost`**, while the listener binds the literal
`127.0.0.1`. That asymmetry is deliberate: the allow-listed string is the
hostname one, but binding `"localhost"` can resolve to `::1` and miss the
browser's connection entirely.

Scopes are `openid profile email offline_access` — the minimum this client
uses. `offline_access` is the one that earns a refresh token. Codex
additionally requests two connectors scopes; they buy nothing here.

**No browser is launched.** The URL is text the user opens, which is also what
makes the flow work over SSH — forward port 1455 and the callback lands in the
right process.

### The page

`KeyStep::Device` is shared with Copilot's device page, because the two really
are the same page: something to show, then a wait. What differs is carried by
`SigninKind` on the row, injected from the provider's `auth` scheme — never
guessed from what the flow happens to have filled in yet:

| | `DeviceCode` (Copilot) | `BrowserLink` (ChatGPT) |
| --- | --- | --- |
| shows | a short URL + a **code in a box** | a long link, and no box |
| the second row says | `and enter this one-time code` | `and sign in — this window continues by itself` |
| `c` copies | the code | the **link** |
| waits for | approval | the browser |

`c` copying the link rather than a code that doesn't exist is the point of the
distinction: a browser page's URL is far too long to retype, so a `c` that did
nothing there would be dead on the one page that most needs it.

## The Responses API

The ChatGPT backend speaks OpenAI's **Responses** API, and only that. An API
key cannot reach it and a ChatGPT token cannot reach `api.openai.com` — the two
credential worlds are disjoint, so this is not an endpoint choice but a
consequence of the sign-in.

`src/llm/responses.rs` is a **translation**, both directions, between that
format and the Chat Completions currency the rest of the crate already uses
(`ChatMessage` in, `Delta`/`StreamOutcome` out). Nothing above
`openai::OpenAiClient` learns a second wire format exists: the agent loop, the
transcript, the rollout and the derived context are untouched.

One consequence worth knowing: **Ctrl+D still shows the chat-completions
shape** (`docs/context.md`) — the derived conversation, before the last-moment
translation. That is the conversation the session actually holds, and it is
what a `/resume` restores and what every other provider would be sent; the
`input` array is a rendering of it, not a second source of truth.

### The request

Three fields are **mandatory** rather than conventional, and omitting any is a
400 that does not say which:

```jsonc
{
  "model": "gpt-5.5",
  "instructions": "…",   // the system prompt, hoisted — required, even empty
  "input": [ … ],        // typed items, not role/content pairs
  "store": false,        // this backend keeps nothing
  "stream": true,
  "tools": [ … ], "tool_choice": "auto", "parallel_tool_calls": true,
  "reasoning": {"effort": "medium", "summary": "auto"},
  "prompt_cache_key": "…"
}
```

Four shape differences, each a place a naive port breaks:

- **`messages` → `input`**, an array of typed items. A tool call is an item of
  its own (`function_call`), not a field on an assistant message, and its
  result is a `function_call_output` item linked by `call_id` — there is no
  `role: "tool"`.
- **The system prompt is `instructions`**, top-level. `build_input` hoists
  every system message into it, joined by blank lines.
- **Content parts are `input_text` / `input_image`.** An image's URL is the
  value of `input_image`, not the nested `image_url: {url}` object Chat
  Completions uses.
- **An empty assistant `content` array is a 400** where Chat Completions
  tolerates `""` — so a round that was only tool calls carries no message item
  at all.

One **invariant this format depends on and Chat Completions does not**: every
`function_call` item must be answered by a `function_call_output` with the same
`call_id`, and vice versa — an orphan either way is a 400 that fails the turn
and every turn after it, since the history only grows. `context::context_messages`
already guarantees it (each `HistoryItem::Tool` derives its call and its result
together, and the compacted shape drops tool pairs wholesale rather than
cutting between them), so no reconciliation pass exists here. A change that
made the derivation able to emit a call without its result would need one.

Tool specs are flattened out of their `function` object: `{"type":"function",
"name":…,"parameters":…}` rather than `{"type":"function","function":{…}}`.
`strict: false`, because every schema here has optional fields and strict mode
demands each property be required.

`Off` sends **no** `reasoning` field. This API has no `enabled: false`, and
inventing one is a 400.

### The stream

Same transport, same cancel cadence — `drain_responses` is `drain_stream`'s
sibling, and `pump_lines` is the byte loop they share, so an Esc is honoured
identically whichever format is in flight.

| event | becomes |
| --- | --- |
| `response.output_text.delta` | reply text |
| `response.reasoning_summary_text.delta`, `response.reasoning_text.delta` | reasoning |
| `response.function_call_arguments.delta` | a `Delta.tool_call` fragment, counted only |
| `response.output_item.done` (a `function_call` item) | a whole `ToolCallRequest` |
| `response.completed` / `.incomplete` | usage + the finish reason |
| `response.failed`, `error` | an in-band failure |

The structural difference from Chat Completions: **a tool call arrives whole**,
on one frame, where Chat Completions dribbles it out as `tool_calls` fragments.
So there is no accumulator here. The argument fragments that *do* stream are
surfaced for the token tally, exactly as the chat path surfaces its own, so the
status line ticks while the call is produced.

Usage is renamed but not reshaped: `input_tokens`/`output_tokens` rather than
`prompt_tokens`/`completion_tokens`, with `input_tokens_details.cached_tokens`
and `output_tokens_details.reasoning_tokens` nested the same way — so the
footer gauge, the `Done for Ns · N tokens (N cached)` receipt and the
`Thought for …` cell's snap all work unchanged.

### What this does not do

**Encrypted reasoning does not round-trip.** Codex sends
`include: ["reasoning.encrypted_content"]` and replays the returned blocks, so
a reasoning model keeps its chain of thought across the rounds of one tool
loop. Carrying that here would mean a new field on `ChatMessage` threaded
through `context`, the rollout format and the transcript — so this build omits
`include` and the model simply re-reasons each round. It is a quality cost, not
an error: nothing fails, and the path to adding it later is exactly that field.

## Reasoning, vision and the context window

`GET {api_base}/models?client_version={version}` works under ChatGPT auth and
publishes richer metadata than the OpenAI-compatible listing, so the `/model`
picker, the Ctrl+T ladder, the footer gauge and the vision degradation all
work with **no hardcoded catalog**. `llm::models`' per-record sniffs gained a
branch each — the same shape-sniffing pattern OpenRouter, Venice and Copilot
already share, so no other provider's list is touched:

| | ChatGPT record | what it drives |
| --- | --- | --- |
| id | `slug` (not `id`) | the model sent to the API |
| name | `display_name` | the picker's label |
| context | `context_window` × `effective_context_window_percent` | the footer gauge, auto-compact |
| vision | `input_modalities` contains `image` | `docs/tools.md`'s image degradation |
| reasoning | `supported_reasoning_levels` + `default_reasoning_level` | the Ctrl+T cycle |

The envelope is `{"models": […]}`, not `{"data": […]}` — one extra field on
the response struct, so a second parse function never has to exist.

The window is scaled by the **share the backend will actually accept a prompt
in** (95% by default), which is the same rule Copilot's `max_prompt_tokens`
gets: the gauge exists to keep a turn inside the limit the API enforces, not
the nominal one.

`supported_reasoning_levels` names the **exact** rungs a model takes, so
Ctrl+T offers what the API will accept — including `ultra`, a rung above
`max` that no other provider names and that `ReasoningEffort` gained for this.
A record marked `visibility: "none"` is withheld; `"hide"` only means "not a
headline model" and stays selectable.

`client_version` is **required** — without it the listing is refused rather
than defaulted, which is why `models_url` is the one place that query is added.

## When it doesn't work

`openai::explain` rewrites the failures whose wire bodies are useless, the way
it already does for Copilot:

| what you see | what it means |
| --- | --- |
| `Your ChatGPT sign-in has expired. Run /login and sign in again.` | the refresh token expired, was reused, or was revoked — all terminal |
| `OpenAI refused this request. A ChatGPT plan that includes Codex is needed.` | a 403: the seat, not the request |
| `ports 1455 and 1457 are both in use` | another sign-in is holding them; no third port is allow-listed |
| `the callback's state doesn't match` | a stale browser tab answered; start again |

A `403` is also what an unrecognised `originator` earns, with a message that
does not say so — which is why that header is pinned in `providers.toml`
rather than left to a default.

## Environment

| variable | effect |
| --- | --- |
| `OPENAI_CHATGPT_REFRESH_TOKEN` | the stored refresh token (a real env var wins over `.env`, as everywhere) |
| `ALTER_ZERO_ENV_FILE` | relocates the store the rotation writes back to |

## Files

| file | what's in it |
| --- | --- |
| `src/llm/chatgpt.rs` | the claims parse, the flow's URLs/bodies, the freshness rule (pure); the loopback listener, the exchanges, the cache and the rotation write-back (boundary) |
| `src/llm/auth.rs` | `request_auth` — the one seam every outbound call resolves through |
| `src/llm/responses.rs` | the Responses wire format, both directions (pure) |
| `src/llm/openai.rs` | `request_url`/`request_payload` (the wire branch), `drain_responses`, `pump_lines` |
| `src/llm/models.rs` | the `{"models": …}` envelope and the three record sniffs |
| `src/app/login.rs` | `SigninKind`, `DeviceLogin::copy_target` |
| `src/ui/login_view.rs` | the browser page's wording |
| `src/tui/workers.rs` | `spawn_signin` — which flow a provider runs |
