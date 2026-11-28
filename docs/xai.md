# xAI Grok as a provider

Sign in with an xAI account's Grok subscription and use xAI's models on it:
`/login` → **Sign in with an account** → **xAI Grok**. It is a **device code**,
so nothing has to open a browser on this machine. The page shows a short code
and the address to enter it at, which works the same over SSH or in a
container.

This is the sixth sign-in, after GitHub Copilot, ChatGPT Codex, the Anthropic
Console, ClinePass and Google Antigravity. It keeps their shape: a long-lived
token in the `.env` store, minted per request into a short-lived bearer, the
`/login` fork's device page, the `/model` picker's ✓, and the `/usage` page.
It adds no wire format. xAI's API speaks the **Responses** API, which
`src/llm/responses.rs` already translates for ChatGPT (`docs/chatgpt.md`).
What is new is a catalog read from two listings, and an effort ladder in
xAI's own record shape.

## Read this first: whose client this is

The sign-in presents the client id `b1a00492-073a-47ea-816f-4c329264a828`.
That is the public OAuth client of xAI's own Grok CLI (its scopes include
`grok-cli:access`), and both open clients this provider follows send it:
hermes-agent (`hermes_cli/auth_constants.py`) and pi
(`packages/ai/src/auth/oauth/xai.ts`). xAI publishes no way for a
third-party client to register its own. It does document the device-code
sign-in itself, for its CLI (`grok login --device-auth`, "device code login
follows RFC 8628"). hermes-agent's comments speak of a partnership with xAI,
but neither project states that xAI issued this client for other clients'
use. The posture is the one `docs/chatgpt.md` takes with Codex's
client id. Whether a Grok subscription may be used from a client xAI did not
write is a question about xAI's terms, not about this code.

Two things are not borrowed from the Grok CLI:

- **No identity is sent.** Requests carry this app's own `User-Agent` (the
  shared HTTP client's), and the system prompt is the user's own. Unlike
  `docs/antigravity.md`, there is no backend here that gates anything on a
  client header.
- **The API, not the Grok CLI's proxy.** xAI's own CLI sends inference to
  `cli-chat-proxy.grok.com`, and its docs list `api.x.ai` as "only needed
  when using `api_key` auth". This provider sends the token to
  `https://api.x.ai/v1` instead, which is what the token's `api:access` scope
  is for. Both references do the same.

## What a provider file says

```toml
[providers.xai_grok]
name = "xAI Grok"
auth = "xai_grok"                 # ← a sign-in, not a pasted key
wire_api = "responses"            # ← xAI's API speaks Responses
description = "Sign in with your Grok subscription (SuperGrok), with a device code"
api_key_env = "XAI_GROK_REFRESH_TOKEN"

[providers.xai_grok.kwargs]
api_base = "https://api.x.ai/v1"
```

- `AuthScheme::XaiGrok` needs no explicit `#[serde(rename)]`, because
  `rename_all = "snake_case"` already spells it `xai_grok`. A test pins that
  spelling. A mismatch would fall silently through to `ApiKey`: a row that
  looks configured and then asks for a pasted key.
- The id is **not** `xai`. `docs/user-providers.md` shows `xai` as a provider
  of your own, defined with `XAI_API_KEY`, and a shipped id is patched key by
  key from a user's file. Someone who followed that page would have
  overwritten the sign-in's variable and kept its `auth`: a sign-in row
  holding a pasted key.
- There are no `extra_headers`. The access token is the whole credential.

## The sign-in: a device code

All of it is in `src/llm/xai.rs`. The pure half covers the bodies, the
device code's parse and the poll's verdict. The boundary half covers the
requests and the wait.

1. `POST {issuer}/oauth2/device/code`, form-encoded, with `client_id` and
   `scope = openid profile email offline_access grok-cli:access api:access`.
   That is the client's own set, asked for whole as both references ask for
   it. `api:access` is what lets the token call the API, and `offline_access`
   is what earns the refresh token. Without either, the sign-in succeeds and
   then cannot do its one job.
2. The answer is RFC 8628's `device_code`, `user_code`, `verification_uri`,
   `expires_in` and `interval`. Seconds may arrive as numbers or as strings
   (the tolerance the other device flows extend). A missing or zero interval
   polls every five seconds, since RFC 8628 allows zero and zero would be a
   tight loop. A missing life is given 900 seconds, so the page still has a
   countdown.
3. **The verification page must be an `https` address.** The page shows it
   as a link to open, so a plain `http` page, a `file:` URL or something that
   is not a URL at all is refused rather than shown. pi refuses the same.
4. `POST {issuer}/oauth2/token` with RFC 8628's grant, every interval,
   sleeping first. The user has not read the code yet, and a poll that beats
   the interval earns a `slow_down`. The verdict (`xai::poll_verdict`):

| answer | verdict |
| --- | --- |
| a `2xx` with both tokens | approved |
| a `2xx` without a refresh token | ended: *"xAI approved the sign-in but sent no refresh token"*. A session that can never refresh would die at the first expiry. |
| `authorization_pending` | wait |
| `slow_down` | wait longer: the interval the server names, or RFC 8628's five seconds more. Never faster than before, and never more than a minute. |
| `access_denied`, or xAI's own `authorization_denied` | ended: denied |
| `expired_token`, or the code's own life running out | ended: expired |
| a `408`, `429` or `5xx` with no error the RFC names | wait: a trip that failed, not an answer. An intercepting proxy's error page is not a denial. |
| any other error | ended, in the server's words |

The **page** is the Copilot device page (`docs/copilot.md`): `Visit {url}`,
`and enter this one-time code`, the code in its box, `Waiting for approval… ·
expires in 9:59`, and `c copy code`. It names the plain `verification_uri`,
not the one-click `verification_uri_complete` that xAI also sends. The page
asks the user to type the code, and the one-click address would make that
instruction false. `docs/cline.md` keeps the same rule for WorkOS's.

The **confirmation** names the account: `Signed in to xAI Grok
(grok@example.com) — run /model to use it`. The account comes from the access
token's own claims (`email`, else `preferred_username`), decoded and never
verified. They label, and grant nothing. A device code leaves exactly that
question open, because the browser it was approved in may have been signed in
to another account.

### Why there is no browser flow

Both references sign in with the device code alone. hermes-agent had a
browser PKCE flow and removed it: a stored `oauth_pkce` mode is still read,
and its tests still carry loopback redirects. Its code now says "device-code
is the only xAI OAuth flow". Which loopback redirects this client accepts is
not documented, and it cannot be checked from here without an account. A
device code needs no redirect, works over SSH, and is the flow xAI documents
for headless machines. If xAI publishes a redirect for this client, the
browser page is a `SigninKind::BrowserLink` in `tui::config::signin_kinds`
and a spawner in `tui::workers`. The loopback listener already exists
(`llm::loopback`).

### The issuer

Both grants and the device code use one **issuer**, `https://auth.x.ai`.
`ALTER_ZERO_XAI_ISSUER` points them somewhere else: a fork's own issuer, or
`smoke.sh` Phase 174's stub. It is read once at the boundary and handed in
through `xai::set_issuer`, the `chatgpt::set_issuer` pattern. The paths are
fixed (`/oauth2/device/code`, `/oauth2/token`) rather than discovered.
hermes-agent reads the token endpoint from the issuer's OpenID configuration
and then has to check the host on every use, because "one MITM at login could
plant a `token_endpoint` that receives the refresh_token forever". A path that
is never read off the wire cannot be planted.

## The two token layers

| | what it is | lifetime | where it lives |
| --- | --- | --- | --- |
| **refresh token** | what the sign-in mints | long-lived, but **rotating** and single-use | `.env`, under `XAI_GROK_REFRESH_TOKEN` |
| **access token** | the bearer the API takes | hours (a device-code sign-in can return a quarter-hour one) | memory only |

The refresh grant is `POST {issuer}/oauth2/token` with `grant_type =
refresh_token`, form-encoded like the poll. xAI rotates the refresh token on
use and revokes the old one (hermes-agent: "xAI rotates refresh_token on every
refresh"), and may leave it out of an answer when it did not rotate it (pi). A
missing one means "keep the one you have". So the cache is `docs/claude.md`'s,
point for point:

- a **refresh gate**, so concurrent misses refresh once rather than present a
  single-use token twice;
- the **rotation chain** (`rotations`), so a config still holding the token
  the session started with finds the live one;
- the **write-back** of the rotated token to the `.env` store before the
  request goes (`persist_refresh`). It is written only while the store still
  holds this session's own chain, so a sign-in that landed meanwhile is never
  overwritten. `tui::models` hands the module the store's path once at
  startup (`xai::set_store_path`). Skipping that call is not a crash but a
  forced re-login at the next launch.
- the **forgotten** list, so `xai::forget` never waits behind a refresh in
  flight and a bearer minted for a credential a sign-in replaced is never
  cached.

How long a bearer is cached is ChatGPT's rule (`xai::cache_lifetime`). It
keys off the grant's `expires_in` first, which is a **duration** and immune
to this machine's clock, and off the token's own `exp` only when that is
missing. Five minutes come off the end. A short life is cached for at most
half of itself, so a quarter-hour token is cached for ten minutes and a known
expiry is never extended. When nothing trustworthy says, a sixty-second floor
applies. An uncached bearer would refresh, and so rotate, on every request.

**One thing differs from the cache it was ported from.** A finished sign-in
calls `forget` on the token it is about to store, before anything has
refreshed it. In the original logic that entry stayed on the forgotten list
until the next refresh settled, which then read it as a forget that had
raced it and discarded its own bearer. The next request refreshed again,
spending one more rotation. Here a refresh clears the list as it starts.
Anything already there is older than the refresh and was honoured when
`forget` cleared the cache. A forget that lands while the request is out is
still recorded after the clear, and still drops the bearer.
`auth_cache_regression_a_refresh_after_a_sign_ins_forget_is_cached` pins it,
and Phase 174 counts exactly one refresh. The ChatGPT, Claude and Cline
modules keep the original logic.

## The wire: Responses on `api.x.ai`

There is no exchange: the access token is the bearer (`auth::request_auth`'s
`XaiGrok` arm), with no account header and the configured base.

The wire is **Responses**, the API xAI recommends for its reasoning models.
models.dev marks every Grok chat model `recommended_api: responses`, and both
references send all their xAI traffic there. Every field this crate's
Responses body sends is one xAI's `POST /v1/responses` reference lists:

| field | xAI's reference says |
| --- | --- |
| `model`, `input`, `stream` | required / as OpenAI |
| `instructions` | "an alternate way to specify the system prompt" |
| `store: false` | accepted |
| `tools`, `tool_choice`, `parallel_tool_calls` | accepted; functions and web search are the tools it supports |
| `reasoning: {effort, summary}` | `effort` per model; `summary` "only included for compatibility" |
| `prompt_cache_key` | routes requests to one server, for the cache |
| `service_tier` | accepted, but never sent here: no xAI record lists a speed tier (`docs/fast-mode.md`) |

That matters: hermes-agent found xAI refusing a field it did not know
(`text.verbosity`, which it stopped sending), and this body sends nothing
outside the list.

**Cache affinity is the body's key.** xAI caches automatically and routes a
conversation to one server by `prompt_cache_key`, or by an `x-grok-conv-id`
header that does the same thing. "When a request sets both,
`prompt_cache_key` takes priority." The body already carries the session's
key (`docs/prompt-caching.md`), so no header is added. This is unlike
ChatGPT's backend, which keys its cache on headers alone.

**Encrypted reasoning does not round-trip.** Both references send `include:
["reasoning.encrypted_content"]` and replay the blocks, and hermes-agent notes
that xAI relies on it across turns. This crate omits `include`, for the
reason `docs/chatgpt.md` gives: carrying it would mean a new field threaded
through the context, the rollout and the transcript. The model re-reasons
each round. That costs quality, not correctness.

## The catalog: two listings, joined

xAI's API publishes what a model can do, so the `/model` picker, the Ctrl+T
ladder, the footer gauge and the vision degradation need no catalog built
into this app. Both references ship one, generated from models.dev. The facts
are spread over two listings, though, and neither has all three:

| | `GET /v1/language-models` | `GET /v1/models` |
| --- | --- | --- |
| lists | the models that chat (and see) | every model it serves, its image generators included |
| envelope | `{"models": […]}` | `{"data": […]}` |
| vision | `input_modalities` | — |
| effort ladder | `capabilities.reasoning_effort` | `capabilities.reasoning_effort` |
| window | — | `context_length` |

So the catalog **is** `/language-models` (`models::models_url`'s `XaiGrok`
arm). `fetch_models` then reads `/models` with the same bearer and joins each
model's window in by id (`models::with_windows`). The join is by id and
nothing more. A model the full listing does not name keeps no window rather
than borrowing one, and a model only the full listing names (an image
generator) is never added. If the second read fails, the catalog is kept
without windows: the gauge shows no share and auto-compaction waits, but no
row is lost. Both records already parse with the existing sniffs: the
`models` envelope is ChatGPT's, a top-level `input_modalities` is ChatGPT's,
and `context_length` is OpenRouter's. One sniff is new.

### The effort ladder

`capabilities.reasoning_effort` names "the values the model accepts for
`reasoning.effort`", beside the model's own `default_reasoning_effort`. That
list is the Ctrl+T ladder (`models::reasoning_support_of`), so the cycle
never offers a level the model would refuse:

- **`none` is not an Off rung.** grok-4.3 lists it. The Responses wire says
  Off by leaving the field out, and an xAI model that is sent no effort
  reasons at its own default. Off would be a footer reading `off` over a model
  thinking at `high`. So `can_disable` is false for every xAI record, and the
  lightest rung is the lowest level the list names.
- **No list, no ladder.** A model that reasons but refuses the parameter
  (grok-4.20-0309-reasoning answers "does not support parameter
  reasoningEffort") lists no `capabilities` at all. It gets no Ctrl+T and its
  requests never carry the field.
- The rung a model starts on is `medium` when the list offers it, the policy
  every ladder here keeps (`docs/reasoning.md`), else xAI's own default.

An **empty catalog is an error** for this provider, as it is for ChatGPT's
(`models::catalog_or_error`). The request authenticated, so listing nothing
is the account, not an empty server: *the account listed no models — its
Grok subscription may not include API access (SuperGrok)*.

## When it doesn't work

`openai::explain` routes an xAI refusal through `xai::auth_advice`. A refused
refresh is worded at the token endpoint by `xai::refresh_advice`, because it
surfaces before any request is made.

| what you see | what it means |
| --- | --- |
| `your xAI sign-in is no longer valid — run /login and sign in again.` | a `401`; an `invalid_grant` or `invalid_token`; a `400` or `401` from the refresh grant (the refresh token is revoked, reused or expired); or xAI's stale-bearer `403`, whose body names `[WKE=unauthenticated:…]` |
| `xAI refused this account — it needs a Grok subscription that includes API access (SuperGrok), or its usage is spent for now. …` | every other `403`, on the API or on the refresh. hermes-agent saw it for no subscription, a spent quota and a model outside the tier. Signing in again cannot fix any of them, which is why it is kept apart from the row above, despite a `code` that reads the same. |
| `xAI is rate-limiting this account — …` | a `429` |
| `the account listed no models — …` | see *The catalog* |
| `The sign-in was denied on xAI's page — …` | `access_denied` / `authorization_denied` |
| `The code expired — press Esc and sign in again.` | `expired_token`, or the code's life ran out |
| `xAI named a sign-in page that is not an https address: …` | see step 3 of the sign-in |

## `/usage`

**No request is made.** xAI's API has usage reads only for API teams, on a
management key. A Grok subscription's weekly usage pool is shown on the Grok
dashboard ("Check usage under **Usage** at grok.com/dashboard", in xAI's own
docs). So the page says that in one note and links
`https://grok.com/dashboard` (`usage::xai_report`, `usage::XAI_DASHBOARD`).
It never mints a bearer only to say nothing, which would also rotate the
sign-in. This is the Console's posture (`docs/usage.md`).

## Environment

| variable | effect |
| --- | --- |
| `XAI_GROK_REFRESH_TOKEN` | the stored refresh token (a real env var wins over `.env`, as everywhere) |
| `ALTER_ZERO_XAI_ISSUER` | the issuer the sign-in and the refresh talk to: a fork's own, or `smoke.sh` Phase 174's stub. `https://auth.x.ai` by default. |
| `ALTER_ZERO_ENV_FILE` | relocates the store the rotation writes back to |

## What this was checked against

There was no xAI account to sign in with while writing this, so every claim
here rests on a source, and the live path is left to a test you run:

- **hermes-agent** (`hermes_cli/auth_xai.py`, `auth_constants.py`, its tests)
  and **pi** (`packages/ai/src/auth/oauth/xai.ts`, `providers/xai.ts`, its
  tests). Both are device-code only, with the same client, scopes and
  endpoints. Where they differ, the choice here is one of theirs or the RFC's:
  no `referrer` field (pi sends `referrer=pi`, hermes-agent sends nothing);
  fixed paths rather than discovery (pi's); `slow_down` adds five seconds (RFC
  8628 and pi; hermes-agent adds one); the lifetime from `expires_in` (pi's)
  with the token's `exp` as the fallback (hermes-agent's).
- **xAI's own documentation** (`docs.x.ai/llms-full.txt`): the Grok CLI's
  device-code sign-in, the `/v1/models` and `/v1/language-models` schemas, the
  `POST /v1/responses` request fields, prompt caching and where a
  subscription's usage is shown.
- **models.dev**, which both references build their catalogs from: the
  windows and the effort lists the fixtures here use.
- `smoke.sh` **Phase 174** drives the whole path against a stub: the
  sign-in, `/model`, Ctrl+T, a turn on the Responses wire, the rotation
  written back, and `/usage`. `tests/live_caching.rs`'s
  `live_xai_lists_its_models_and_reads_the_prefix_from_cache` runs it against
  the real API (ignored by default; it needs `XAI_GROK_REFRESH_TOKEN` and
  `ALTER_ZERO_LIVE_TOKEN_STORE`).

## Files

| file | what's in it |
| --- | --- |
| `src/llm/xai.rs` | the client, scopes and endpoints; the device code's parse, the poll's verdict and pacing, the token set, the cache lifetime, the claims and the advice (pure); the device-code request and poll, the refresh, the cache, the rotation write-back and the issuer override (boundary) |
| `src/llm/config.rs` | `AuthScheme::XaiGrok`, the scheme that puts the row under *Sign in with an account* |
| `src/llm/auth.rs` | `request_auth`'s `XaiGrok` arm: the minted bearer, nothing beside it |
| `src/llm/models.rs` | the `/language-models` URL, the window join from `/models`, the effort ladder's sniff, the empty-catalog error |
| `src/llm/openai.rs` | `explain`'s `XaiGrok` arm |
| `src/llm/usage.rs` | `xai_report` and `XAI_DASHBOARD` |
| `src/tui/config.rs` | `signin_kinds` (one device page) and `xai_issuer` (the override) |
| `src/tui/workers.rs` | `spawn_xai_login`, which `spawn_signin` picks by the row's auth scheme |
| `src/tui/models.rs` | the store path and the issuer, handed in once at startup |
| `providers.toml` | the `xai_grok` row |
| `scripts/smoke/phases/174-xai.sh` | the sign-in and everything after it, against a stub of both hosts |
