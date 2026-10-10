# Cline as a provider

Reach the models behind the Cline extension and CLI — `/login` → **Use an API
key** → **Cline**, or `/login` → **Use a subscription** → **Cline Account**.

Three providers ship — one API, two ways in, and a second catalog for the
subscription:

| provider id | how you authenticate | what is stored |
| --- | --- | --- |
| `cline` | a key made in the Cline dashboard | the key, verbatim |
| `cline_account` | the Cline account sign-in (a WorkOS device code) | a **refresh** token |
| `cline_pass` | the *same* sign-in, for the ClinePass subscription's models | the same refresh token |

`cline` and `cline_account` are the same API reached two ways;
`cline_pass` is that same **sign-in** again with a different **catalog**
(see *ClinePass* below).

Both speak **Chat Completions** against `https://api.cline.bot/api/v1`, whose
`/models` is public — so the picker lists the catalog off `kwargs.api_base`
with no separate `api_model_base`, keyed or signed in.

## What a provider file says

```toml
[providers.cline]
name = "Cline"
description = "The models behind the Cline extension and CLI — …"
api_key_url = "https://app.cline.bot"

[providers.cline.kwargs]
api_base = "https://api.cline.bot/api/v1"

[providers.cline_account]
name = "Cline Account"
auth = "cline_account"                       # ← a sign-in, not a pasted key
description = "Sign in with your Cline account — …"
api_key_env = "CLINE_ACCOUNT_REFRESH_TOKEN"

[providers.cline_account.kwargs]
api_base = "https://api.cline.bot/api/v1"

[providers.cline_pass]
name = "ClinePass"
auth = "cline_account"                       # ← the same sign-in
description = "Cline's $9.99/month subscription: …"
api_key_env = "CLINE_ACCOUNT_REFRESH_TOKEN"  # ← and the same stored token
models = [                                   # ← the plan's static catalog
    "cline-pass/glm-5.3",
    "cline-pass/deepseek-v4.1-flash",
    # …
]

[providers.cline_pass.kwargs]
api_base = "https://api.cline.bot/api/v1"
```

`auth = "cline_account"` is what puts the row in the **subscription** list and
sends Enter to the device page; a mismatch in the spelling degrades *silently*
into a pasted-key row, which is why `llm::config` pins it with a test — the
same posture `anthropic_console` gets (`docs/claude.md`).

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
   `data.userInfo.email` is what the confirmation names — `Signed in to Cline
   Account (you@example.com) — run /model to use it`. Nothing WorkOS-shaped
   is stored: after the sign-in, the WorkOS pair is never spoken to again.

A sign-in that registers without a refresh token is refused **at the
sign-in** — without one the session would die at the first access-token
expiry with no way back but another sign-in — exactly as
`chatgpt::exchange_code` refuses a token set with no refresh token.

## ClinePass

ClinePass is Cline's **subscription** — $9.99/month for 2–5× the usage on a
curated set of open coding models. It is a *separate provider on the same
account*: Cline's own clients make you point at **ClinePass** instead of
**Cline** after subscribing, and the API takes the plan's models by their
**`cline-pass/…` slugs** — `cline-pass/glm-5.3`, `cline-pass/deepseek-v4.1-flash`,
`cline-pass/kimi-k3`, `cline-pass/qwen3.7-max` and the rest of the documented
catalog — while the usage-billed ids (`deepseek/deepseek-v4.1-flash` …) keep
billing the account's **credits**. Pick the wrong one and a Pass account with
an empty credit balance earns exactly the 402 this page's advice explains.

The two rows therefore share one **sign-in**, one stored refresh token, and
one base — what differs is the model ids. And because those ids are the
plan's own catalog, the public `/models` listing does **not** carry them, so
the file names them instead of pointing at an endpoint: `models` is a
**static** list (`providers.toml`), and `llm::models::fetch_models` hands it
straight to the `/model` picker and the startup probe with **no request at
all**. `cline_account` keeps the fetched 458-model usage-billing catalog; the
picker shows both rows and both are selectable once signed in.

The slugs are Cline's to change (it has already retired some), so the list is
the one shipped `providers.toml` carries — a slug added or removed is a line
edited there, never a code change. Quota is measured against rolling 5-hour,
weekly and monthly windows; the dashboard is
[app.cline.bot/dashboard/subscription?personal=true](https://app.cline.bot/dashboard/subscription?personal=true),
and when a window runs out the refusal reads *ClinePass limit reached* with
the advice to fall back to the usage-billed row (`/model` → a
`cline_account` model, or the official clients' own wording).

**Free models are deliberately not listed here.** Cline runs rotating
`cline-free/…` promotions, but its docs are explicit that free-model usage is
**not supported through the Cline API** — they work only inside the Cline
extension and CLI — so a row here would offer models the endpoint refuses.

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
(`docs.cline.bot/api/errors`), each of which wants a different sentence:

| what you see | what it means |
| --- | --- |
| `your Cline sign-in is no longer valid — run /login and sign in again.` | a `401`: the refresh token was rejected (revoked, or the account signed out) |
| `this Cline account is out of credits — add credits at app.cline.bot/dashboard, or switch provider with /model.` | a `402`: the account's balance, not the request |
| `this Cline account isn't allowed to make that request — …` | a `403`: the credential's reach |
| `this account's ClinePass usage limit is reached for its current window — …` | Cline's own *ClinePass limit reached*: the subscription's rolling window (5-hour / weekly / monthly) is used up — wait for the reset, or switch to a usage-billed model |
| `this Cline account has no ClinePass subscription — subscribe at …` | a `cline-pass/…` model asked for without the plan (Cline's *"No access to ClinePass subscription models yet"*) |
| the **browser** shows *Access blocked, please contact support.* (「访问被阻止，请联系支持。」) | Cline's identity provider (WorkOS) refused the sign-in before any token was minted — usually a block on the account, the network/IP, or the sign-up itself. Nothing in this client can lift it: the same message appears signing in to app.cline.bot, the extension or the CLI. Cline's own resolution is emailing **support@cline.bot** with the account's address (cline/cline#11405); a different sign-in method, account or network may also get past it |

On the sign-in page itself, a failure stays *on* the page in red:
`expired_token`/`invalid_grant` read as *"the device code expired — press Esc
and sign in again"*, a denied confirmation as *"the sign-in was denied in the
browser — press Esc and try again"* (or the server's own sentence when it
wrote one), and a code request WorkOS refused as `WorkOS answered {status}`
with whatever came back.

## Environment

| variable | effect |
| --- | --- |
| `CLINE_ACCOUNT_REFRESH_TOKEN` | the stored refresh token (a real env var wins over `.env`, as everywhere) |
| `CLINE_API_KEY` | the *pasted-key* provider's variable — a different credential for a different row |
| `ALTER_ZERO_CLINE_AUTH_BASE` | the WorkOS host the sign-in talks to — a fork's own, or `smoke.sh` Phase 136's local stub; `https://api.workos.com` by default |
| `ALTER_ZERO_CLINE_API_BASE` | the Cline API host register/refresh talk to — Phase 136's stub; `https://api.cline.bot` by default |

## Files

| file | what's in it |
| --- | --- |
| `src/llm/cline.rs` | the client id and the four endpoints, the device-code/poll/register/refresh shapes and verdicts, the expiry and cache-lifetime rules, the `workos:` spelling and the advice (pure); the code request, the cancel-aware poll, the registration, the cache, the rotation write-back and the two base overrides (boundary) |
| `src/llm/auth.rs` | `request_auth` — the one seam every outbound call resolves through |
| `src/llm/config.rs` | `AuthScheme::ClineAccount` |
| `src/tui/config.rs` | `signin_kinds` — the device page this row offers; the two base overrides |
| `src/tui/workers.rs` | `spawn_cline_login` — the flow the device page runs |
| `src/tui/models.rs` | the store path and the base overrides, handed in once at startup |
| `src/llm/models.rs` | `fetch_models`'s static short-circuit — a provider that names its models is its own catalog, with no request |
| `providers.toml` | all three rows, and the `models` static list ClinePass's catalog rides in |
| `scripts/smoke/phases/136-clineaccount.sh` | the row, the page, and the whole wire against a local stub |
