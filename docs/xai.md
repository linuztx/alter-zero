# xAI Grok as a provider

Reach Grok on an xAI account's **SuperGrok** subscription: `/login` → **Sign in
with an account** → **xAI Grok**.

| provider id | how you authenticate | what is stored | what `/model` lists |
| --- | --- | --- | --- |
| `xai_grok` | the xAI account sign-in (a device code) | a **refresh** token | the language models xAI's API lists for the account |

It speaks OpenAI's **Responses** API against `https://api.x.ai/v1` — the wire
`docs/chatgpt.md` describes, with the three xAI differences under *The wire*
below — and knows what each model can do (its context window, whether it reads
images, and the exact reasoning levels Ctrl+T cycles) from xAI's own listings.

xAI documents no OAuth flow for other apps. Its own Grok CLI signs in with a
device code (`grok login --device-auth`, RFC 8628) on a public client id, and
this is that flow as two open-source agents port it — pi
(`packages/ai/src/auth/oauth/xai.ts`) and hermes-agent
(`hermes_cli/auth_xai.py`), whose constants agree; every wire shape below is
one their tests pin. Like Google Antigravity's (`docs/antigravity.md`) it is an
**unofficial** route, and the row's description says so: the endpoints could
change without notice. Nothing in the requests claims to be the Grok CLI — the
`User-Agent` is this app's own and the system prompt is the user's alone.

## What a provider file says

```toml
[providers.xai_grok]
name = "xAI Grok"
auth = "xai_grok"                        # ← an xAI account sign-in, not a pasted key
wire_api = "responses"                   # ← the Responses API, as xAI serves it
description = "Sign in with your xAI account to use Grok on your SuperGrok subscription — …"
api_key_env = "XAI_GROK_REFRESH_TOKEN"   # ← the account's refresh token

[providers.xai_grok.kwargs]
api_base = "https://api.x.ai/v1"
```

`auth = "xai_grok"` is what puts the row in the **subscription** list, sends
Enter to the device page and mints a bearer for every request; a misspelling
degrades *silently* into a pasted-key row, which is why `llm::config` pins the
spelling with a test, as it does `cline_account`'s (`docs/cline.md`).

## The sign-in

The **device code** — the page GitHub Copilot's and ClinePass's sign-ins use: a
code in a rounded box over the link to approve it at, `c` to copy the code, a
countdown from the code's own life, no browser launched.

1. **The code request** — `POST {issuer}/oauth2/device/code`, form-encoded,
   carrying the public client id (`llm::xai::CLIENT_ID`) and the scopes
   `openid profile email offline_access grok-cli:access api:access` —
   `offline_access` is what earns a refresh token and `api:access` what lets
   the access token reach `api.x.ai`. Nothing else: pi adds a `referrer=pi`
   naming itself, which hermes does not send and nothing needs. The answer
   names the `device_code` the poll presents, the `user_code`, a
   `verification_uri` and its `verification_uri_complete` — the same page with
   the code filled in, which is the link the page shows, so one click is the
   whole step (the code stays in its box to compare against, and the row
   under the link says *and confirm this one-time code* rather than *enter*;
   both references moved to the prefilled link). Both links must be
   `https` — the page is a link the user clicks, so an answer naming
   anything else is refused rather than shown — with a loopback `http`
   allowed for the smoke suite's stub.
   A missing `interval` is RFC 8628's five seconds, and a missing
   `expires_in` fifteen minutes.
2. **The poll** — `POST {issuer}/oauth2/token`, form-encoded, with
   `grant_type=urn:ietf:params:oauth:grant-type:device_code`, the client id
   and the device code. The first poll waits one interval (pi's fix — an
   immediate one only ever answers `authorization_pending`). Then the verdict
   reads the **body**: `authorization_pending` waits again, `slow_down` takes
   the interval its answer names or else grows the current one by RFC 8628's
   five seconds (`xai::slowed` — never faster than the current interval, since
   the server has just said it was too fast), `access_denied`/
   `authorization_denied`/`expired_token`/`invalid_grant` end the flow with a
   sentence, and a body carrying an access token is the approval whatever
   status delivered it. A **408, 429 or 5xx** whose body is none of those —
   a proxy's error page, a rate limit, a server briefly down — is one more
   wait (`Poll::Retry`, hermes polls through the same): it says nothing about
   the code, which is still good, and ending the flow on it would make the
   user start over for a blip. Any other answer ends the flow naming its
   status. However a poll's interval is set — the code's answer, a
   `slow_down` — it is held under `xai::MAX_INTERVAL`, a minute: an answer
   naming an hour would otherwise leave the page counting down long after the
   approval, or past the code's own life.
3. **The account** — the `openid email` scopes put an `id_token` in the
   answer, and the confirmation names its `email` claim, else the access
   token's, else — for an account whose tokens name no email — its
   `preferred_username` handle: `Signed in to xAI Grok (you@example.com) — run
   /model to use it`. Read and never verified: nothing here checks a JWT
   signature, it only reads what the token says about itself.

A token set without a refresh token is refused **at the sign-in**: without one
the session would die at the first access-token expiry with no way back but
another sign-in — the posture `chatgpt::exchange_code` and the ClinePass
registration share.

There is no OIDC discovery. hermes reads `/.well-known/openid-configuration`,
pins the token endpoint it names to an `x.ai` host and caches it, but uses the
device-code URL it hardcodes beside it — and the token endpoint discovery names
is the one hardcoded here. A round trip and a cached endpoint to re-validate,
for an answer known in advance.

## The two token layers

The split every subscription keeps (`docs/chatgpt.md`):

1. The **refresh token** — long-lived, persisted in the `.env` key store under
   `XAI_GROK_REFRESH_TOKEN`, the store a pasted key lands in, so key
   resolution, the `/login` row's ✓ and the next launch need no second
   mechanism.
2. The **access token** — a JWT minted on demand at `POST {issuer}/oauth2/token`
   (`grant_type=refresh_token`, the client id and the refresh token,
   form-encoded), cached in memory, never written, and sent as a plain
   `Authorization: Bearer` — no prefix and no account header.

**Refresh tokens are single-use.** xAI rotates the pair on every refresh and
revokes the token just presented — "the first to refresh strands the other
with `invalid_grant`", in hermes' words. So the three rules the other sign-ins
keep are load-bearing here: refreshes are serialized behind one gate, so two
threads never present one token twice; a rotated token is written straight
back into the `.env` store (`xai::persist_refresh`, through the path
`tui::models` hands the module at startup) — only while the store still holds
this session's own chain, never over a token a newer sign-in stored meanwhile;
and an in-memory rotation map keeps the live `ModelConfig`, which still holds
the token the session started with, presenting the live one. A refresh that
answers without a `refresh_token` keeps the one presented, as both references
do. A fresh sign-in's `xai::forget` clears the cache without waiting behind a
refresh in flight.

The access token's cached life is the **shorter** of the grant's `expires_in`
(a duration, immune to this machine's clock) and the JWT's own `exp` (absolute,
and ignored when the clock reads it as already past — that is the clock
running ahead, not a dead token), less a five-minute skew, never more than half
a short life, and a minute when neither says. The shorter, because the two
references disagree on which to trust: pi reads `expires_in` alone, hermes
reads only `exp`, having seen device-code logins return ~15-minute JWTs — and a
cache trusting a longer `expires_in` would keep sending a dead bearer for
hours. And when the API rejects a bearer anyway — xAI answers an access token
it can no longer validate with a **403** carrying
`[WKE=unauthenticated:bad-credentials]`, not a 401 — the cached one is
dropped (`xai::reject_access`), so the next request mints afresh.

## What `/model` lists

xAI splits a model's facts across two listings (`docs.x.ai`, *Models*), and
`fetch_models` reads both for an xAI provider:

- **`GET /v1/language-models`** — the rows. It lists the language models only
  (where `/v1/models` also carries the image and video models, which cannot
  chat), each with its `input_modalities` (`"image"` there is vision) and its
  `capabilities.reasoning_effort` — the exact levels the model accepts, which
  **is the Ctrl+T ladder** — beside `capabilities.default_reasoning_effort`.
- **`GET /v1/models`** — each model's `context_length`, the window the footer
  gauges and compaction plans against, borrowed onto the row of the same id.

Not having `/v1/models` costs only the windows, never a row. A server that
answers no `/v1/language-models` is listed from `/v1/models` alone — the
OpenAI-compatible list every route to xAI serves — so a user's own pasted-key
provider block pointing at xAI's host lists, too.

A sign-in whose listings name **no model at all** is a failure rather than a
provider with no rows: the listing authenticated, so an empty one is the
account — a Grok plan without API access through the sign-in — and the picker
says so (`the account listed no models — its Grok plan may not include API
access through the sign-in, which xAI limits to some SuperGrok tiers`), the
rule `/model` already keeps for a ChatGPT sign-in. A pasted key's empty list
stays an empty list.

A listed `"none"` is the off switch rather than a rung, and only a listed one:
xAI answers `none` with a 400 on a model whose list doesn't name it, since that
model's reasoning is mandatory (grok-4.3 lists `none`; grok-4.5 and later do
not). So Ctrl+T offers Off exactly where the model does. The listings are
read live rather than copied into a catalog here — pi's catalog, hermes' and
xAI's own pages already disagree about which model takes which level, and the
listing is the one that is right for the account asking.

## The wire

The Responses translation (`src/llm/responses.rs`) matches hermes' wire
snapshot for its xAI sign-in field for field: `instructions` hoisted from the
system prompt, `store: false`, `tool_choice: "auto"`, `parallel_tool_calls:
true`, the tools flattened with `strict: false`, `function_call` /
`function_call_output` items linked by `call_id`, and `prompt_cache_key` for
the cache. What a request to xAI's API (`xai::serves` — the sign-in, or any
provider whose base is an `x.ai` host) does differently:

- **Tool schemas lose the keywords xAI refuses.** xAI's `/responses` answers a
  tool whose schema carries a `pattern` or `format` keyword, or an `enum` with
  a `/` in a value, with a 400 (hermes #27197). No built-in tool has one, but
  an MCP server's tools often do (`format: "uri"`), and one such server would
  fail every request. `xai::sanitize_schema` drops the keywords from the copy
  the request carries — walking the schema's subschemas, so a *property* named
  `pattern` or `format` is kept — and drops a slash-bearing `enum` whole,
  leaving its `type`; the executor still validates nothing by them, so nothing
  the tool accepts changes.
- **Off is `reasoning: {"effort": "none"}`.** The Responses API elsewhere has
  no explicit disable, so `Off` sends no `reasoning` field — which on grok-4.3
  would leave it reasoning at its default `low`. Off is only ever offered
  where the model's ladder lists `none` (*What `/model` lists*), so it is only
  ever sent where it is accepted.
- **No `include: ["reasoning.encrypted_content"]`.** Both references ask for
  the encrypted reasoning and replay it every later round; this client
  round-trips reasoning on no provider (`docs/chatgpt.md`), so asking for it
  would only download it. The cost is the one ChatGPT pays: the model reasons
  afresh each round.

`prompt_cache_key` is xAI's documented affinity key on the Responses API; the
`x-grok-conv-id` header is its Chat Completions twin, and the body key wins
when both are sent, so no header is added. Usage is read as on every Responses
stream — `input_tokens_details.cached_tokens` and
`output_tokens_details.reasoning_tokens` — and the stream's in-band `error`
frame, which xAI can send first with its `message` at the top level, is shown
in its own words.

## `/usage`

xAI publishes no usage or limits endpoint for a Grok sign-in — neither
reference calls one, and the API documents none. The page says so in one line
and links grok.com's own usage page (`https://grok.com/?_s=usage`), the place
both references send a user to see what a plan has left.

## When it doesn't work

xAI's errors come in a `{"code": "…", "error": "…"}` envelope — the `error`
string is the sentence, the `code` a machine label — and the ones a user can
act on are 403s, each wanting a different answer. A turn's refusal
(`openai::explain`) and the `/model` listing's both go through
`xai::advise`, which drops a rejected bearer and words the refusal with
`xai::auth_advice`:

| what you see | what it means |
| --- | --- |
| `xAI no longer accepts this sign-in's access token — send your message again; if it keeps failing, run /login and sign in again.` | a 401, or a 403 whose `error` says *The OAuth2 access token could not be validated. [WKE=unauthenticated:bad-credentials]*: the bearer was cached past the point xAI honours it. The cached one is dropped, so the next request mints a new one |
| `this xAI account has no active Grok subscription, or has used what its plan includes — check grok.com/?_s=usage, or switch provider with /model.` | a 403 saying *You have either run out of available resources or do not have an active Grok subscription.* — it may also arrive as a status-less stream error. hermes adds that an X Premium+ plan alone may not include this access, which is why the row's description names SuperGrok |
| `this xAI account is out of credits — add credits or upgrade at grok.com, or switch provider with /model.` | a 403 coded `personal-team-blocked:spending-limit` (*You have run out of credits or need a Grok subscription.*) |
| `your xAI sign-in is no longer valid — run /login and sign in again.` | the token endpoint refusing the stored refresh token — a 400/401, or an `invalid_grant`/`invalid_token` code on any status: a single-use token presented twice, or a grant revoked |
| `xAI refused to renew this sign-in: the account isn't allowed API access through the Grok sign-in — …` | the token endpoint answering a **403** (`permission_denied`): xAI limiting the sign-in to some SuperGrok tiers (hermes #26847). Signing in again won't change it, and the sentence says so |

Any other refusal keeps xAI's own words, lifted out of the envelope — `xAI
answered 400: Request contains an invalid argument: Invalid PNG image.`, never
the JSON around it. On the sign-in page a failure stays on the page in red:
`expired_token` reads as *"the device code expired — press Esc and sign in
again"*, a denial as *"the sign-in was denied in the browser — press Esc and
try again"*.

## Known limits

- **Reasoning is not replayed** (*The wire*): each round the model reasons
  afresh, where the two references thread xAI's encrypted reasoning back.
- **A model the listings omit is not in `/model`.** hermes keeps a few ids it
  found callable through the sign-in but absent from xAI's listings
  (`grok-composer-2.5-fast` among them); nothing here copies such a list, since
  a stale one shows models that fail. `ALTER_ZERO_MODEL` reaches one by id.
- **Long first bytes.** hermes gives large xAI requests a stall allowance of
  ten minutes and more; every provider here shares one 120-second
  per-operation deadline (`openai::NET_OP_TIMEOUT`). A very large prompt that
  xAI sits on longer than that ends on a timeout the retry then repeats.

## Environment

| variable | effect |
| --- | --- |
| `XAI_GROK_REFRESH_TOKEN` | the stored refresh token (a real environment variable wins over `.env`, as everywhere) |
| `ALTER_ZERO_XAI_ISSUER` | the auth host the sign-in and the token mint talk to — a fork's own, or `smoke.sh` Phase 174's local stub; `https://auth.x.ai` by default |

## Files

| file | what's in it |
| --- | --- |
| `src/llm/xai.rs` | the client id, scopes and endpoints; the device-code/poll/refresh shapes and verdicts, the poll's cadence (the slow-down rule and its cap), the link check, the cache-lifetime rule, the account's name, the schema sanitizer and the advice (pure); the code request, the cancel-aware poll, the token mint, the cache, the rotation write-back and the issuer override (boundary) |
| `src/llm/auth.rs` | `request_auth` — the one seam every outbound call resolves through |
| `src/llm/config.rs` | `AuthScheme::XaiGrok` — the scheme that puts the row under *Sign in with an account* and runs the device page |
| `src/llm/models.rs` | `fetch_models`' xAI arm — `/v1/language-models` merged with `/v1/models`, an empty sign-in listing an error — and the `capabilities.reasoning_effort` ladder sniff |
| `src/llm/responses.rs` | the three xAI differences on the Responses wire |
| `src/llm/openai.rs` | `explain`'s xAI arm, which also drops a rejected bearer |
| `src/llm/usage.rs` | the `/usage` page's xAI section |
| `src/tui/config.rs` | `signin_kinds` — the device page this row offers; the issuer override |
| `src/tui/workers.rs` | `spawn_xai_login` — the flow the device page runs, picked by the row's auth scheme |
| `src/tui/models.rs` | the store path and the issuer override, handed in once at startup |
| `providers.toml` | the `xai_grok` row |
| `scripts/smoke/phases/174-xai.sh` | the row, the device page and the whole wire against a local stub: the sign-in, the stored token, `/model`'s merged listing, a turn, and a missing subscription read as advice |
