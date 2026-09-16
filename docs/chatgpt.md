# ChatGPT Codex as a provider

Sign in with a ChatGPT Plus/Pro/Team seat instead of pasting an API key —
`/login` → **Use a subscription** → **ChatGPT Codex** → **Browser login**
(or **Device code login** on a machine with no browser).

This is the second subscription provider, after GitHub Copilot
(`docs/copilot.md`), and it reuses that feature's whole shape: the `/login`
fork, the `.env` store, the two-token split, the `/model` picker's ✓, the
capability probe. What it adds is a second *sign-in* shape (a browser, not a
code), a second *wire format* (Responses, not Chat Completions) — and, since
a headless machine has no browser to hand a link to, a device-code twin of
the sign-in, ported from Codex's own, which is what made the row the one
subscription that has to ask *how* before it opens a page.

The provider is **named for what it is**: the ChatGPT seat reached the way
Codex reaches it. It was `OpenAI (ChatGPT)`, which read as a second OpenAI
API-key provider; the id `openai_chatgpt`, the `OPENAI_CHATGPT_REFRESH_TOKEN`
variable and every stored selection kept their names, so the rename cost no
one a sign-in.

> **A caveat worth stating.** This API is undocumented and unversioned, and
> the OAuth client id it uses is Codex's own — there is no third-party
> registration path. It has already changed shape in public more than once
> (`OpenAI-Beta: responses=experimental` came and went), and one of its
> per-request headers turned out to be load-bearing for something the docs
> never mention — see *Cache affinity* below. Treat drift as expected.
> Whether a ChatGPT subscription may be used from a non-OpenAI client is a
> question about OpenAI's terms, not about this code.

## What a provider file says

```toml
[providers.openai_chatgpt]
name = "ChatGPT Codex"
auth = "openai_chatgpt"                          # ← a sign-in, not a pasted key
wire_api = "responses"                           # ← and a different request shape
description = "Sign in with your ChatGPT Plus/Pro account, in a browser or with a device code"
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

## The sign-ins

Two, and the row asks which before it opens either.

### The choice

A subscription row used to open one page. ChatGPT Codex has two, so Enter on
its row opens a **choice** first (`KeyStep::SigninMethod`) — the shape asked
for, verbatim:

```
────────────────────────────────────────────────────────────────────────────

  Select ChatGPT Codex login method:

→ Browser login (default)
  Device code login (headless)

  ↑↓ navigate  enter select  escape/ctrl+c cancel

────────────────────────────────────────────────────────────────────────────
```

- A **title**, unlike the three `/login` lists: the rows answer a question
  the subscription list did not ask, so the page says whose question it is.
- **No `❯` filter and no counter.** Two rows are a question, not a list to
  search; typing does nothing, and the hardware cursor hides (the device
  page's rule — a kitty cursor trail would streak across it on every ↑/↓),
  its seat parked on the title row like the device page's.
- The rows are `SigninKind::method_label`: the first kind the row lists is
  the default and says so; the device code says what it is *for*, since
  "headless" is the one word that tells an SSH user which row is theirs.
- ↑/↓ wrap, Enter opens the highlighted flow's page, Esc steps back to the
  subscription list, Ctrl+C closes.
- **Esc from a page opened this way returns to the choice**, with the row
  just tried still highlighted — codex's own browser page says "on a headless
  machine, press Esc and choose the device code", so the choice must be one
  Esc away. A page a single-flow subscription opened (Copilot's) still
  returns to the list, since there was never a choice.

Which rows a subscription offers is the provider file's `auth` scheme's to
say (`tui::config::signin_kinds` → `SubscriptionChoice::kinds`, the first
being the default): `openai_chatgpt` lists both, `github_copilot` its device
code, `anthropic_console` its browser. A row listing one kind opens its page
at once, exactly as before; `Action::StartDeviceLogin { provider, kind }`
carries the pick to the worker, since the two ChatGPT flows open the *same*
page and only the row knows which flow to run behind it.

### The browser flow

OpenAI's default is a browser PKCE loopback:

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
right process. When even that is too much to ask of the machine, the other
row is one Esc away.

### The device-code flow

The browser flow needs a browser on the machine the TUI runs on, or a
forwarded port. On a headless box — an SSH session into a server, a
container — neither is a given, so Codex offers a second flow, and this
client ports it whole (`chatgpt::request_device_code` /
`await_device_approval`):

1. `POST {issuer}/api/accounts/deviceauth/usercode` with `{"client_id": …}`
   → `{"device_auth_id", "user_code", "interval"}`. The interval arrives as a
   **string** (`"5"`); a number is accepted too, and an absent one falls to
   five seconds rather than the reference's zero, which is a tight loop
   against the server. A `404` here means device code login is not enabled
   for this server — Codex's own reading — and the page says to press Esc
   and choose the browser row instead.
2. Show the code beside `{issuer}/codex/device`, the page the user types it
   at. This is **the Copilot device page, exactly** (`docs/copilot.md`):
   `Visit …` over `and enter this one-time code`, the code in its box,
   `Waiting for approval… · expires in 14:59`, `c copy code`. The
   fifteen-minute countdown is Codex's own poll deadline; OpenAI's code
   response names no expiry, unlike GitHub's.
3. `POST {issuer}/api/accounts/deviceauth/token` with `{"device_auth_id",
   "user_code"}` every `interval` seconds (sleeping first — the user has not
   read the code yet) for up to those fifteen minutes. A `403` **or** a
   `404` is "not yet" — the server answers a pending code with either — a
   `2xx` carries the grant, and anything else ends the flow.
4. The grant is `{"authorization_code", "code_challenge", "code_verifier"}`:
   the **server** minted the PKCE pair, so the exchange repeats *its*
   verifier, form-encoded at the same `/oauth/token` the browser flow posts
   to, with `redirect_uri = {issuer}/deviceauth/callback` — a URI nothing
   ever listens on, repeated because the grant was bound to it.

Nothing is bound locally: no port, no `state`. That is the point, and it is
why the two flows share one `exchange_code` and everything after it — the
token set, the refresh token's rotation, the claims read off the access
token, the cache — is byte-identical between them. Codex's requests to this
issuer carry its `originator` and `User-Agent`, so these do too.

The page is worded off `SigninKind::DeviceCode`, so the table under *The
page* below reads left-to-right for this flow: `Visit`, a box, `c copy
code`, a wait for approval. A page that fell back to the browser wording
would tell the user to wait for a redirect that is never coming.

`smoke.sh` Phase 119 drives the whole flow against a local stub standing in
for the auth server, answering Codex's own wire shapes — the string
interval, a `403` and then a `404` before the grant, the server-minted
verifier, a token set whose access token claims a `pro` plan — and reads the
exchange back off the stub's log: the pair the code request issued on every
poll, and the device callback (never a loopback port) on the exchange.

#### The issuer

Both flows, and the refresh they share, build their URLs on one **issuer**,
`https://auth.openai.com` by default. `ALTER_ZERO_OPENAI_ISSUER` points them
elsewhere — a fork's own auth server, or the smoke suite's stub — read once
at the boundary and handed in through `chatgpt::set_issuer`, the
`set_store_path` pattern: the pure URL builders never read the environment.
`DeviceEndpoints::for_issuer` derives the four device paths from it,
trimming a trailing slash so a stub's `http://127.0.0.1:8080/` builds the
same paths OpenAI's own does.

### The page

`KeyStep::Device` is shared with Copilot's device page, because the two really
are the same page: something to show, then a wait. What differs is carried by
`SigninKind` on the page, taken from the row the user picked (or the row's
only kind) — never guessed from what the flow happens to have filled in yet:

| | `DeviceCode` (Copilot, and ChatGPT's device code) | `BrowserLink` (ChatGPT's browser) |
| --- | --- | --- |
| shows | `Visit {url}` + a **code in a box** | the bare URL, bright, and no box |
| the second row says | `and enter this one-time code` | `Sign in there — this window continues by itself` |
| `c` copies | the code | the **link** |
| waits for | approval | the browser |

Both URLs are real **OSC 8 hyperlinks** (`docs/links.md`), stamped on the
unwrapped text so every hard-broken fragment opens the whole target rather
than its own row's worth of it. That is also why the browser page carries no
verb in front of its URL: the link is the affordance, and an `Open ` would
only push the target off the start of the row it should begin. The device
page's URL stays **dim** (the code box is what the eye should land on —
`docs/copilot.md`), so linking adds an underline and the carrier without
repainting it.

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
  "service_tier": "priority",   // /fast — codex's fast mode, docs/fast-mode.md
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

### Cache affinity: two headers, not the body key

The backend's prompt cache does **not** key on the body's `prompt_cache_key`.
Measured live on an identical 6.7k-token prefix (`gpt-5.4-mini`, two turns,
a 12 s pause before the second to rule out a slow cache write): the request
as this crate used to send it read **0** tokens from cache on turn 2, the
same request carrying the Codex CLI's `session_id` and `conversation_id`
headers read **6400** of them, and `OpenAI-Beta: responses=experimental` on
its own changed nothing. So the session's cache key rides both header names
(`chatgpt::session_headers`, pure; attached by
`openai::chatgpt_request_headers`, gated on the `OpenAiChatGpt` auth scheme
the way Copilot's per-request headers are gated on its own), beside the body
key it also carries. Without them every agentic round re-billed the whole
conversation at full price on a subscription that never showed a bill —
which is why it went unnoticed until the receipt was checked against the
wire (`docs/prompt-caching.md`).
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

`service_tier` is codex's **fast mode** (`docs/fast-mode.md`): the selected
speed tier's id, present only when `/fast` selected one the model's record
lists, with the `x-codex-routing-hint` header naming the model and the tier
beside the cache-affinity headers below.

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
`Thought for …` cell's snap all work unchanged. That `cached_tokens` is real
(the backend's own count) is what caught the missing session headers above:
a receipt that never said `cached` on a repeated prefix was the wire saying
no.

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
| context | `context_window` × `effective_context_window_percent` (the field is absent live, so the 95% default is what applies) | the footer gauge, auto-compact |
| vision | `input_modalities` contains `image` | `docs/tools.md`'s image degradation |
| reasoning | `supported_reasoning_levels` + `default_reasoning_level` | the Ctrl+T cycle |
| speed tiers | `service_tiers` (`[{id, name, description}]`, the `priority` one being codex's fast mode) | `/fast` (`docs/fast-mode.md`) |

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

### `client_version` is a filter, not a formality

The query parameter is required — but it is also **what decides how much of
the catalog you are shown**. Every record carries its own
`minimal_client_version` (measured live: `0.98.0` through `0.144.0` across one
account), and the backend serves only the records the claimed version
satisfies. A version below all of them is answered `{"models": []}` on an HTTP
**200**, with nothing at all to say why.

That is exactly what sending the crate's own `CARGO_PKG_VERSION` (`0.1.0`)
did: a signed-in account whose `/model` picker listed nothing, and no error
anywhere to explain it. `chatgpt::CLIENT_VERSION` is therefore a **ceiling
sentinel** (`99.99.99`, the value OpenAI's own release tooling uses) rather
than a real release number — OpenAI raises those per-record gates as models
ship, and a pinned real version would silently start dropping models again.
The same value rides the `User-Agent`, so the request's two version claims
agree.

Two things now make that failure impossible to have again silently: the
constant is pinned by a test that also asserts it is *not* the crate version,
and an **empty catalog is an error** for this provider
(`models::catalog_or_error`) naming both causes — the plan, or the gate.
Every other provider's empty list stays an ordinary empty list.

## When it doesn't work

`openai::explain` rewrites the failures whose wire bodies are useless, the way
it already does for Copilot:

| what you see | what it means |
| --- | --- |
| `the account listed no models — its ChatGPT plan may not include Codex, or this client is too old for the models it serves` | the `/models` fetch authenticated and came back empty (see `client_version` above) |
| `Your ChatGPT sign-in has expired. Run /login and sign in again.` | the refresh token expired, was reused, or was revoked — all terminal |
| `OpenAI refused this request. A ChatGPT plan that includes Codex is needed.` | a 403: the seat, not the request |
| `ports 1455 and 1457 are both in use` | another sign-in is holding them; no third port is allow-listed — or use the device code, which binds none |
| `the callback's state doesn't match` | a stale browser tab answered; start again |
| `OpenAI's device code sign-in is not available right now — press Esc and choose Browser login instead.` | the code request answered `404`: device code login is not enabled for this server (Codex's own reading) |
| `The code expired — press Esc and sign in again.` | fifteen minutes of polling without an approval |

A `403` is also what an unrecognised `originator` earns, with a message that
does not say so — which is why that header is pinned in `providers.toml`
rather than left to a default.

## Environment

| variable | effect |
| --- | --- |
| `OPENAI_CHATGPT_REFRESH_TOKEN` | the stored refresh token (a real env var wins over `.env`, as everywhere) |
| `ALTER_ZERO_ENV_FILE` | relocates the store the rotation writes back to |
| `ALTER_ZERO_OPENAI_ISSUER` | the auth server both sign-ins (and the refresh) talk to — a fork's, or `smoke.sh` Phase 119's local stub; `https://auth.openai.com` by default |

## Files

| file | what's in it |
| --- | --- |
| `src/llm/chatgpt.rs` | the claims parse, both flows' URLs/bodies, the device flow's code/poll/grant shapes and verdicts, the freshness rule (pure); the loopback listener, the code request and the approval poll, the shared code exchange, the cache, the rotation write-back and the issuer override (boundary) |
| `src/llm/auth.rs` | `request_auth` — the one seam every outbound call resolves through |
| `src/llm/responses.rs` | the Responses wire format, both directions (pure) |
| `src/llm/service_tier.rs` | the speed tiers a record lists and the `/fast` cycle over them (pure) — `docs/fast-mode.md` |
| `src/llm/openai.rs` | `request_url`/`request_payload` (the wire branch), `drain_responses`, `pump_lines` |
| `src/llm/models.rs` | the `{"models": …}` envelope and the three record sniffs |
| `src/app/login.rs` | `SigninKind` and its method rows, `SubscriptionChoice::kinds`, the `KeyStep::SigninMethod` choice and its keys, `DeviceLogin::copy_target` |
| `src/ui/login_view.rs` | the choice's page and the browser page's wording |
| `src/tui/config.rs` | `signin_kinds` — which pages a scheme offers; `openai_issuer` — the override |
| `src/tui/workers.rs` | `spawn_signin` — which flow a provider and a pick run |
| `scripts/smoke/phases/119-chatgptdevice.sh` | the choice and the device flow, driven against a local stub of the auth server |
