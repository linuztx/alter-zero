# Cline as a provider

Reach the models behind the Cline extension and CLI two ways: `/login` → **Use
an API key** → **Cline**, or `/login` → **Use a subscription** → **ClinePass**.

Two providers ship — one API, two ways in, each with the catalog it pays for:

| provider id | how you authenticate | what is stored | what `/model` lists |
| --- | --- | --- | --- |
| `cline` | a key made at [app.cline.bot/dashboard/account](https://app.cline.bot/dashboard/account) | the key, verbatim | the usage-billed catalog, paid from the account's **credits** |
| `cline_pass` | the Cline account sign-in (a WorkOS device code) | a **refresh** token | the ClinePass plan's `cline-pass/…` models, paid from the **subscription** |

Both speak **Chat Completions** against `https://api.cline.bot/api/v1`, and both
know what each of their models can do — its context window, whether it reads
images, and the thinking levels Ctrl+T cycles — from Cline's own catalog (see
*What each model can do* below).

The sign-in is offered **once**, as ClinePass. It used to be offered twice, a
`cline_account` row beside it running the very same flow and storing the very
same token for the usage-billed catalog. But under *Use a subscription* the
one subscription is ClinePass: usage-billed models bill credits, not a plan,
and a key reaches them already. Two rows for one sign-in invited signing in
twice, and the second row is where a routing bug hid (*Which flow runs*,
below).

## What a provider file says

```toml
[providers.cline]
name = "Cline"
api_model_base = "https://api.cline.bot/api/v1/ai/cline"   # ← the catalog with each model's facts
description = "The models behind the Cline extension and CLI — …"
api_key_url = "https://app.cline.bot/dashboard/account"   # ← where `/login` says keys are made

[providers.cline.kwargs]
api_base = "https://api.cline.bot/api/v1"

[providers.cline_pass]
name = "ClinePass"
auth = "cline_account"                       # ← a Cline account sign-in, not a pasted key
description = "Cline's $9.99/month subscription: …"
api_key_env = "CLINE_ACCOUNT_REFRESH_TOKEN"  # ← the account's refresh token

[providers.cline_pass.kwargs]
api_base = "https://api.cline.bot/api/v1"
```

`auth = "cline_account"` is what puts the row in the **subscription** list,
sends Enter to the device page, and lists the plan's models (*ClinePass*,
below); a mismatch in the spelling degrades *silently* into a pasted-key row,
which is why `llm::config` pins it with a test — the same posture
`anthropic_console` gets (`docs/claude.md`). The scheme keeps its name: what
it describes is the credential, a Cline *account* — the one sign-in, which is
ClinePass. Nothing in the block names the plan's models: the list is Cline's
to publish, not this file's to copy.

### Which flow runs

`tui::workers::spawn_signin` picks the sign-in by the row's **auth scheme**, in
an exhaustive match — never by its provider id. When it matched ids, with a
catch-all for everything else, the ClinePass row fell through to GitHub
Copilot's device flow, and a finished sign-in would have stored GitHub's token
as a Cline refresh token and sent it to Cline's refresh endpoint. A user's own
`providers.toml` can name a row anything, and give two rows one sign-in; the
scheme is what decides.

## The sign-in

Cline's own clients sign in through **WorkOS**, the identity provider behind
app.cline.bot, so this is a port of Cline's published client
(`sdk/packages/core/src/auth/cline.ts` and its provider-auth registry); every
constant in `src/llm/cline.rs` was read from it. The flow is the **device
code** — the same page GitHub Copilot's sign-in uses (`docs/copilot.md`): a
code in a rounded box over the URL to confirm it at, `c` to copy it, a
countdown from the code's own life, no browser launched.

1. **The code request** — `POST {auth}/user_management/authorize/device`,
   form-encoded, carrying the **public** WorkOS client id
   (`llm::cline::CLIENT_ID`) and nothing else. A device-code flow has no
   client secret, so that id is all a client needs; the response names the
   `device_code` the poll presents, the `user_code`/`verification_uri` the
   page shows, and the expiry/interval (defaulted to WorkOS's own 300 s / 5 s
   when absent, and read as a number *or* a string, the tolerance
   `docs/chatgpt.md`'s device code already extends).
2. **The poll** — `POST {auth}/user_management/authenticate` with
   `grant_type=urn:ietf:params:oauth:grant-type:device_code` and the pair the
   code request issued, once per interval. The verdict reads the **body**:
   `authorization_pending` keeps waiting, `slow_down` grows the interval by a
   second (Cline's own reading of the hint), `access_denied`/`expired_token`/
   `invalid_grant` end the flow with the server's sentence when it wrote one,
   and a body carrying both token strings is the approval regardless of the
   status that delivered it.
3. **The registration** — the WorkOS pair is *not* what the API takes. It is
   exchanged **once**, at `POST {api}/api/v1/auth/register`, for Cline's own
   tokens: `data.refreshToken` is what the key store keeps and
   `data.userInfo.email` is what the confirmation names — `Signed in to
   ClinePass (you@example.com) — run /model to use it`. Nothing WorkOS-shaped
   is stored: after the sign-in, the WorkOS pair is never spoken to again.

A sign-in that registers without a refresh token is refused **at the
sign-in** — without one the session would die at the first access-token
expiry with no way back but another sign-in — exactly as
`chatgpt::exchange_code` refuses a token set with no refresh token.

## ClinePass

ClinePass is Cline's **subscription** — $9.99/month for 2–5× the usage on a
curated set of open coding models. Cline's own clients make you point at
**ClinePass** instead of **Cline** after subscribing, and the API takes the
plan's models by their **`cline-pass/…` slugs** — `cline-pass/glm-5.3`,
`cline-pass/deepseek-v4.1-flash`, `cline-pass/kimi-k3`, `cline-pass/qwen3.7-max`
and the rest of the plan's list — while the usage-billed ids
(`deepseek/deepseek-v4.1-flash` …) bill the account's **credits** instead.
That split is the split between the two rows: the sign-in lists the plan's
models, and the key lists the usage-billed ones.

Because those ids are the plan's own catalog, the public `/models` listing
does **not** carry them.
Cline publishes the plan's list for its own clients instead —
`GET https://api.cline.bot/api/v1/ai/cline/recommended-models`, public, a page
of four lists (`recommended`, `free`, `clinePass`, `clineCloud`) of which the
`clinePass` array is the plan's (`sdk/packages/core/src/cloud/models.ts`,
`loadCloudModels`) — and a `cline_account` sign-in reads it from there:
`llm::cline::fetch_clinepass_models` GETs it with **no credential**, the way
Cline's clients do, `parse_clinepass_models` keeps the list in Cline's own
order (a record with no usable id skipped, a repeated id kept once, a
`data`-wrapped page read through), and `llm::models::fetch_models` hands the
rows to the `/model` picker and the startup probe, tagged `[cline_pass]`.

The slugs are Cline's to change: it has already retired some, a copy of the
list shipped with this provider went two models stale before it merged
(`cline-pass/mimo-v2.6-flash` and `-pro`), and the live list has reordered
since. So the published list is the **only** source, as every other
provider's listing is its only source: a model the plan adds is in the picker
the day it is listed, and one it retires leaves with it. A list that can't be
had — offline, refused, a page this parse no longer reads, or one naming no
model — is the provider's own note in the picker (`⚠ ClinePass unavailable`
beside the other providers' models, or the reason itself in red when it is
the only one): never a shipped copy that looks current and isn't, and never a
provider with no rows, which would read as though the fetch never happened
(the signed-in ChatGPT listing's rule).

The listing is chosen by the row's **auth scheme**, as the ChatGPT one's
required `client_version` is — it belongs to the credential's backend — and
not by a key of its own, a provider id, or a model name. The id (`cline_pass`)
is whatever a user's own `providers.toml` calls the row, so keying on it is
the routing bug *Which flow runs* describes: a renamed or copied block would
quietly list from the wrong place. And a model-name prefix (`cline-pass/…`)
can't choose the list it would be read off, which has not arrived yet.

Quota is measured against rolling 5-hour, weekly and monthly windows; the
dashboard is
[app.cline.bot/dashboard/subscription?personal=true](https://app.cline.bot/dashboard/subscription?personal=true).
When a window runs out the server says which (*You have reached your 5-hour
Clinepass limit. Please try again later.*), and the advice names that window
and points at the usage-billed models, which the key row reaches (`/login` →
Use an API key → Cline).

**Free models are deliberately not listed here.** Cline runs rotating
`cline-free/…` promotions, but its docs are explicit that free-model usage is
**not supported through the Cline API** — they work only inside the Cline
extension and CLI — so a row here would offer models the endpoint refuses.

## What each model can do

The chat base's own `/models` (`https://api.cline.bot/api/v1/models`) names
each model's id and nothing else — no window, no modalities, no reasoning — so
a model listed from it had no context gauge, no auto-compaction, no Ctrl+T
ladder, and attached pictures blind. Cline's clients read their catalog from
`https://api.cline.bot/api/v1/ai/cline/models` instead: the same 461 models,
public (a signed-in request answers byte for byte what an anonymous one does),
in **OpenRouter's record shape**, which `llm::models::parse_models` already
reads:

| record field | what it gives |
| --- | --- |
| `context_length` | the window the footer gauges against, and auto-compaction's 90% |
| `architecture.input_modalities` | image input: a picture is attached, or degraded to a note for a model that can't read one |
| `supported_parameters` naming `reasoning` / `reasoning_effort` | a reasoner: Ctrl+T cycles `off → low → medium → high`, `medium` by default |

**The key row** lists that catalog: its `api_model_base` is
`https://api.cline.bot/api/v1/ai/cline`, so `{api_model_base}/models` is the
catalog itself, and the ordinary listing path does the rest.

**ClinePass** lists its own published models, which the catalog does not carry
— and which say nothing about themselves (`id`, `name`, `description`,
`tags`). So each one borrows its facts from the catalog the way Cline's own
client does (`normalizeClineRecommendedProviderModels`): by the model's name
**without its lab**, `cline-pass/glm-5.3` being `z-ai/glm-5.3`, its display name
(`Z.ai: GLM 5.3`) included. `llm::models::clinepass_entries` is the join; the
catalog is read beside the published list (`fetch_cline_catalog`, no
credential, from `llm::cline::catalog_url` on the API host in force, so Phase
137's stub can serve it). Thirteen of today's fourteen plan models match. One
that doesn't — `cline-pass/qwen3.8-max` today, or a bare name two labs ever
came to share, which is not guessed at — gets what Cline's client assumes for
such a model (`CLINE_PASS_MODEL_DEFAULTS`): a **128K** window, reasoning on,
no image input. A catalog that can't be fetched costs the facts, never the
rows: every plan model takes those defaults then.

Both were checked against the live API with a real account and key: the
reasoning object this client sends — `reasoning: {"effort": …}`, and
`{"enabled": false}` for Off — is accepted on a ClinePass model and on a
usage-billed one alike, an effort streams `reasoning` deltas into the thinking
block and Off answers without one, and the usage frame reports
`completion_tokens_details.reasoning_tokens`.

## The wire form, and the two token layers

The same split every subscription keeps (`docs/chatgpt.md`):

1. The **Cline refresh token** the registration minted. Long-lived, persisted
   in the `.env` key store under `CLINE_ACCOUNT_REFRESH_TOKEN` — the same
   store a pasted key lands in, so key resolution, the `/model` picker's ✓ and
   the next launch need no second mechanism.
2. The **access token** — a WorkOS JWT the API actually takes, minted on
   demand at `POST {api}/api/v1/auth/refresh` (JSON: `refreshToken` plus
   `grantType: "refresh_token"`), cached in memory, never written.

The bearer is not the access token verbatim: Cline's API takes OAuth access
tokens spelled `workos:<jwt>` — a **pasted dashboard key** rides verbatim,
and prefixing one 401s — so `cline::bearer_for` applies that prefix where the
credential is minted (`Authorization: Bearer workos:<jwt>`), while the
pasted-key provider next door sends its key straight through.

**Refresh tokens rotate.** Cline's register/refresh responses re-send
`refreshToken` when it rotated and omit it when it did not, so the same fix
`docs/chatgpt.md` describes applies: `cline::persist_refresh` writes a new
value straight back into the `.env` store through a path `tui::models` hands
the module once at startup — only while the store still holds this session's
own chain, never over a token a newer sign-in stored meanwhile — and an
in-memory rotation map keeps the live `ModelConfig` (which still holds the
token the session started with) refreshing successfully. A fresh sign-in's
`cline::forget` clears the cache without waiting behind a refresh in flight.

The token's cached life comes from the response's ISO-8601 `expiresAt` when
it parses, the access token's own `exp` claim when it does not, and a
conservative minute when neither does — less a five-minute skew so no request
leaves with a token that dies in flight. Short-lived tokens keep at most half
their advertised life; an already-expired deadline caches nothing.

## When it doesn't work

`openai::explain` rewrites the refusals Cline's API documents
(`docs.cline.bot/api/errors`), each of which wants a different sentence. The
refusals about the ClinePass **plan** are messages rather than statuses, matched
on the server's own words in the order Cline's client matches them
(`handleClineResponseError` in `sdk/packages/llms/src/providers/builtins.ts`),
and they outrank the status that carried them — a Pass window running out must
not read as "add credits":

| what you see | what it means |
| --- | --- |
| `your Cline sign-in is no longer valid — run /login and sign in again.` | a `401` on a request, or the refresh endpoint refusing the stored token. The live server answers a dead refresh token with a **`400`** — `{"data":"","error":"failed to refresh token: invalid_grant","success":false}` — so a refused refresh is read by Cline's own rule (`isLikelyInvalidGrant`, ported as `cline::is_likely_invalid_grant`): an error code naming `invalid_grant`, `invalid_token` or `unauthorized` on any status, or a `400`/`401`/`403` whose message says `invalid`, `expired`, `revoked` or `unauthorized` |
| `this Cline account is out of credits — add credits at app.cline.bot/dashboard, or switch provider with /model.` | a `402`: the account's balance, not the request |
| `this Cline account isn't allowed to make that request — …` | a `403`: the credential's reach |
| `ClinePass is for personal Cline accounts, and this is an organization account — …` | the server's *organization accounts cannot use individual model inference subscriptions*: the sign-in is an organization account, and ClinePass is a personal plan |
| `this account's ClinePass 5-hour limit is reached — …` | the server's *You have reached your 5-hour Clinepass limit. Please try again later.*: the subscription's rolling window (5-hour, weekly or monthly — the sentence names which, and so does the advice) is used up — wait for the reset, or switch to a usage-billed model on the key row |
| `this Cline account has no ClinePass subscription — subscribe at …` | a `cline-pass/…` model asked for without the plan: the server's *the user is not subscribed to required model plan* (a `403`), or the sentence Cline's client shows in its place, *"No access to ClinePass subscription models yet"* |
| `Cline rejected this API key — check it at app.cline.bot/dashboard/account, then run /login and enter it again.` | a `401` on the **pasted-key** row. The live API answers a bad key with the same 401 a dead sign-in earns — *"Unauthorized: Please make sure you're using the latest version of Cline and re-authenticate your Cline account."*, words written for Cline's own client — so a provider whose base is `api.cline.bot` gets `cline::key_advice`, which is the sign-in's advice with this one sentence changed |
| the **browser** shows *Access blocked, please contact support.* (「访问被阻止，请联系支持。」) | Cline's identity provider (WorkOS) refused the sign-in before any token was minted — usually a block on the account, the network/IP, or the sign-up itself. Nothing in this client can lift it: the same message appears signing in to app.cline.bot, the extension or the CLI. Cline's own resolution is emailing **support@cline.bot** with the account's address (cline/cline#11405); a different sign-in method, account or network may also get past it |

Any other refused register or refresh call keeps the server's own words, lifted
out of the JSON envelope they came in — `Cline answered 500: database
unavailable`, never the `{"data":"",…}` around it.

On the sign-in page itself, a failure stays *on* the page in red:
`expired_token` reads as *"the device code expired — press Esc and sign in
again"* and `invalid_grant` as *"the device code is no longer valid — press Esc
and sign in again"*, a denied confirmation as *"the sign-in was denied in the
browser — press Esc and try again"* (or the server's own sentence when it
wrote one), and a code request WorkOS refused as `WorkOS answered {status}`
with whatever came back.

## Environment

| variable | effect |
| --- | --- |
| `CLINE_ACCOUNT_REFRESH_TOKEN` | the stored refresh token (a real env var wins over `.env`, as everywhere) |
| `CLINE_API_KEY` | the *pasted-key* provider's variable — a different credential for a different row |
| `ALTER_ZERO_CLINE_AUTH_BASE` | the WorkOS host the sign-in talks to — a fork's own, or `smoke.sh` Phase 137's local stub; `https://api.workos.com` by default |
| `ALTER_ZERO_CLINE_API_BASE` | the Cline API host register/refresh talk to, and the ClinePass list and its models' facts are read from — Phase 137's stub; `https://api.cline.bot` by default |

## Files

| file | what's in it |
| --- | --- |
| `src/llm/cline.rs` | the client id and the four endpoints, the device-code/poll/register/refresh shapes and verdicts, the expiry and cache-lifetime rules, the `workos:` spelling, the advice for a sign-in and for a pasted key, the dead-token rule, and the ClinePass list's parse (pure); the code request, the cancel-aware poll, the registration, the cache, the rotation write-back, the two base overrides, and where the published list and the catalog are read from (boundary) |
| `src/llm/auth.rs` | `request_auth` — the one seam every outbound call resolves through |
| `src/llm/config.rs` | `AuthScheme::ClineAccount` — the scheme that puts the row under *Use a subscription*, runs the device page and lists the plan's models |
| `src/tui/config.rs` | `signin_kinds` — the device page this row offers; the two base overrides |
| `src/tui/workers.rs` | `spawn_cline_login` — the flow the device page runs; `spawn_signin` picks it by the row's auth scheme rather than its id |
| `src/tui/models.rs` | the store path and the base overrides, handed in once at startup |
| `src/llm/models.rs` | `fetch_models`'s `cline_account` arm: the published list, each model's facts borrowed from Cline's catalog by bare name (`clinepass_entries`, `fetch_cline_catalog`) |
| `providers.toml` | both rows — the key row's `api_model_base` naming Cline's catalog, and ClinePass's `cline_account` scheme |
| `scripts/smoke/phases/137-clinepass.sh` | the one sign-in row, the page, and the whole wire against a local stub — then `/model`'s ClinePass list — refused once, reading as the provider's own failure with no model listed, then published — with the facts it borrows (the footer's thinking level and gauge), and a turn on a dead refresh token reading as a sign-in to redo |
