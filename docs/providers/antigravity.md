# Google Antigravity as a provider

Sign in with a Google account and use the models Google Antigravity serves —
Gemini, and the Claude and GPT-OSS models Google routes through it — on that
account's Antigravity quota: `/login` → **Sign in with an account** → **Google
Antigravity**. Nothing is pasted; the browser comes back to this machine on
its own.

This is the fifth sign-in (after GitHub Copilot, ChatGPT Codex, the Anthropic
Console and ClinePass) and it keeps their shape: a long-lived token in the
`.env` store, minted per request into a short-lived bearer, the `/login`
fork's browser page, the `/model` picker's ✓, the `/usage` page. What it adds
is a **fifth wire format** — Google's Cloud Code Assist API, the backend
Antigravity itself talks to — and a fact no other sign-in has: every request
names the account's **Cloud Code project**, which is looked up once beside the
bearer.

## Read this first: an unofficial route

Google publishes no API for third-party clients to reach Antigravity's quota.
This provider signs in with **Antigravity's own public desktop OAuth client**
(the same client id and secret every community integration uses, extracted
from the app) and identifies itself to the backend with the **agy CLI's
`User-Agent`**, because the backend gates its model catalog on that header —
the same posture `docs/providers/chatgpt.md` takes with Codex's client id and
`originator`. It is unofficial, and Google may restrict accounts that use
it: the most-used community plugin warns that *"Google may restrict accounts
using third-party clients"*, and its troubleshooting page reports throttling
and shadow bans of flagged accounts. The `/login` row says so in its
description, and the decision to sign in is the user's.

Where it differs from the line `docs/providers/claude.md` draws: there, a vendor's usage
policy forbids the third-party sign-in outright, the server enforces it, and
working around it would mean sending a client's mandated identity prompt
("You are Claude Code…"). Here no such prompt is needed — two of the three
reference implementations send none, and the third sends one only "to match"
another proxy — so **none is sent**: the system instruction this app sends is
the user's own system prompt and nothing else, which
`llm::cloud_code::tests::no_client_identity_is_ever_injected_into_the_system_instruction`
pins, exactly as `docs/providers/claude.md`'s test does for the Messages API.

## What a provider file says

```toml
[providers.antigravity]
name = "Google Antigravity"
auth = "google_antigravity"          # ← a Google sign-in, not a pasted key
wire_api = "cloud_code"              # ← Cloud Code Assist, not chat completions
description = "…"
api_key_env = "ANTIGRAVITY_REFRESH_TOKEN"   # ← the Google refresh token

[providers.antigravity.kwargs]
api_base = "https://daily-cloudcode-pa.googleapis.com"
```

`auth` and `wire_api` are two questions, as everywhere else: the scheme
decides `/login`'s flow and the credential, the wire format decides the
request's shape. Both spellings are pinned by tests in `llm::config`, since a
mismatch degrades **silently** into a pasted-key, chat-completions row.

The base is the **daily** host, the one every current consumer Antigravity
client sends generation, project setup and the catalog to. Hosts are not
mixed: the production host (`cloudcode-pa.googleapis.com`) serves Gemini
CLI's separate quota pool, whose entitlements and catalog differ, and a
fallback from one to the other is how a 404 cascade starts.

## The sign-in

`llm::antigravity` — Google's OAuth 2.0 PKCE flow for an installed app, on the
shared loopback callback (`llm::loopback`), run on `/login`'s worker with the
same browser page ChatGPT's and the Console's use (`SigninKind::BrowserLink`).

| | |
| --- | --- |
| authorize | `https://accounts.google.com/o/oauth2/v2/auth` |
| token | `https://oauth2.googleapis.com/token` |
| client | Antigravity's desktop client: id `1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com`, its (public) secret |
| scopes | `cloud-platform`, `userinfo.email`, `userinfo.profile`, `cclog`, `experimentsandconfigs` |
| redirect | `http://localhost:{port}/oauth-callback`, port `51121` first, else any free one |
| extra | `access_type=offline` and `prompt=consent` — without them Google issues no refresh token |

- **`state` is random and the verifier never leaves the machine.** One
  reference implementation packs the PKCE verifier *into* `state`, which
  publishes the secret the challenge exists to keep; here `state` is a value
  of its own and the shared loopback refuses a callback that does not echo it.
- **Any port works.** A desktop OAuth client accepts any loopback port, so
  `51121` (Antigravity's own) is only the first choice; a busy port falls back
  to an ephemeral one rather than failing the sign-in.
- **The exchange is form-encoded and carries the client secret** — Google's
  installed-app flow wants it even beside PKCE.
- What is stored is the **refresh token**, under `ANTIGRAVITY_REFRESH_TOKEN`.
  Google keeps a desktop client's refresh token stable, but a refresh that
  returns a new one is written back exactly as `docs/providers/claude.md`'s rotation is
  (only while the store still holds this session's own chain), and one that
  returns none keeps the old.
- A token response without a refresh token is refused at the sign-in: the
  session would otherwise die at the first expiry with no way back but another
  sign-in.
- The confirmation names the **account** (`userinfo`'s email) and its plan,
  the two things a user with several Google accounts has just chosen between.

### The two token layers, and the project

`antigravity::authorize(refresh)` is what the auth seam calls per request:

1. **The bearer** — minted from the refresh token at
   `https://oauth2.googleapis.com/token` and cached in memory for its
   `expires_in` less five minutes, keyed by the refresh token; a cache miss is
   serialized behind one gate, so a turn's rounds and the `/model` fetch never
   refresh twice.
2. **The project** — `POST {base}/v1internal:loadCodeAssist` with
   `{"metadata": {"ideType": "ANTIGRAVITY"}}` (`platform`/`pluginType` values
   are refused there), reading `cloudaicompanionProject` (a string, or an
   object with an `id`). An account the backend has never set up names none:
   it is **onboarded** — `POST {base}/v1internal:onboardUser` with the default
   allowed tier (`free-tier` when none is marked) and snake_case metadata,
   re-sent until it answers `done`, a bounded number of times — and an account
   that still names none falls back to the shared consumer project
   `aicode-consumers`. `ANTIGRAVITY_PROJECT_ID`, when exported, outranks all of
   it. The project is cached beside the bearer for the life of the process.

Onboarding runs **only at the sign-in**, on its cancellable worker: it can
take several asks seconds apart, and a request — whose thread polls Esc — must
never wait on it. A request on an account set up elsewhere asks
`loadCodeAssist` once and takes the fallback when it names nothing.

`antigravity::forget`, which the `/login` boundary calls with the token a
sign-in has just stored (so a stale bearer can never answer for it), drops the
**bearer** and never the account's setup: the project is a fact about the
account, not a credential that goes stale — and dropping it threw away the very
project the sign-in had just onboarded the account to get, which is what
`smoke.sh` Phase 148 caught on its first run.

`RequestAuth` carries the project to the wire as `RequestAuth::project` — a
field only this scheme fills, because only this API names a project in its
body — beside the bearer and the `User-Agent` header.

## The wire format

`llm::cloud_code` translates both directions between the crate's Chat
Completions currency and Cloud Code Assist, the same contract
`llm::anthropic` and `llm::responses` keep: nothing above
`OpenAiClient::stream_chat` learns that a fifth wire exists.

### The envelope

`POST {base}/v1internal:streamGenerateContent?alt=sse`:

```json
{
  "project": "<the account's project>",
  "model": "<the runtime model id>",
  "request": { "contents": [], "systemInstruction": {}, "tools": [], "generationConfig": {}, "sessionId": "…" },
  "requestType": "agent",
  "userAgent": "antigravity",
  "requestId": "agent-<uuid>"
}
```

Headers: the bearer, `Content-Type: application/json`, and the agy CLI
`User-Agent` — `antigravity/cli/{version} (aidev_client; os_type={os};
arch={arch}; cl={changelist}; auth_method=consumer)`, built per machine at the
auth seam. The backend serves a reduced catalog to any other agent string and
answers the hidden models with 404, so the header is load-bearing;
`ALTER_ZERO_ANTIGRAVITY_USER_AGENT` replaces it whole when Google moves the
gate. No `X-Goog-Api-Client`, `Client-Metadata` or `x-goog-user-project`:
the last one turns a request into a 403 on an account whose project lacks the
API. `sessionId` is the session's cache-affinity key
(`docs/providers/prompt-caching.md`).

### The request

- **`contents`**: roles `user` and `model`. The system prompt is hoisted into
  `systemInstruction` (`{"role": "user", "parts": [...]}`, as every
  reference sends it); a tool result is a `functionResponse` part
  (`{"name", "response": {"output": …}}`) in a **user** content; consecutive
  same-role contents are merged, so a parallel batch's results are one user
  content and an image a `read` attached after them rides beside them as
  `inlineData`. Images are `inlineData {mimeType, data}` split out of the
  session's shared `data:` URL — borrowed, never copied (`docs/architecture/memory.md`).
- **Shape rules the backend enforces**: a request may not end on a model turn
  (one that does gets a `Continue.` user turn), and contents with no parts are
  dropped.
- **Call ids** ride `functionCall`/`functionResponse` only for the Claude and
  GPT-OSS models, which need them to pair a result with its call; Gemini pairs
  by order and name, and is sent none. Gemini names no ids of its own, so the
  stream gives each call a random `call_…` id — random rather than counted,
  since ids are recorded and replayed across `/resume`.
- **Tool schemas**, per family. Gemini takes standard JSON Schema as
  `parametersJsonSchema`, once local `$ref`s are inlined and the meta keywords
  (`$schema`, `$id`, `$defs`, `definitions`, `$comment`, `$anchor`) are gone —
  the backend requires self-contained schemas. Claude and GPT-OSS go through
  Google's custom-tool bridge, which takes the legacy `parameters` field and
  answers any keyword outside `type`, `description`, `properties`,
  `required`, `items` and `enum` with `400 Unknown name`, so those six are
  **all** that is kept (an allowlist, so a keyword JSON Schema adds tomorrow
  cannot break a request): a type union collapses to its first non-null type,
  an `enum` survives only when every value is a string, and a property spelled
  `anyOf` becomes `{}`.
- **Thinking**, per family — `generationConfig.thinkingConfig`, sent only when
  the selection carries a thinking mode (the model's record said it reasons):
  - Gemini: `{"includeThoughts": true}`. The **level** is the runtime id's
    suffix (*The catalog*, below), so no budget or level is sent beside it —
    the newest Gemini refuses budgets under 1000, and a level the id already
    chose needs no second statement.
  - Claude (`…-thinking` ids): `{"includeThoughts": true, "thinkingBudget":
    10000}`, the Messages wire's own budget, with `maxOutputTokens` 64000,
    which must exceed it. Claude never gets `temperature`.
  - GPT-OSS: `{"includeThoughts": true, "thinkingBudget": 8192}`.
- **`maxOutputTokens`**: 65535 for Gemini (the smallest cap among the current
  Gemini models, so no model is asked for more than it allows — exceeding a
  cap is a 400), 64000 for Claude, 32768 for GPT-OSS.

### Thought signatures

Gemini 3 and later sign each tool round's reasoning, and **refuse** a later
request whose tool call comes back unsigned
(`Function call is missing a thought_signature`). The crate deliberately
keeps no reasoning in the derived context (`docs/ui/thinking-stream.md`), so the
signatures live **beside** it: `cloud_code` keeps a process-wide memo keyed by
call id, bounded in bytes, filled from each finished round
(`StreamAccumulator::finish` → `RoundEnd::signatures`), and the request
builder looks every call up in it:

- the **first** `functionCall` of each model content carries its signature —
  the real one when the memo has it, else Google's documented
  `skip_thought_signature_validator` sentinel (a round from another provider,
  or from before a `/resume`); the parallel calls after it carry none. A turn
  that opens on a replayed Claude thought (below) is vouched for by that
  thought's own signature and is never *also* given the sentinel — a pairing
  none of the reference implementations sends;
- for **Claude**, whose thinking is Anthropic's own and must open the tool
  loop it started, the round's thought text and signature are memoised too and
  replayed as a leading `{"thought": true, "text", "thoughtSignature"}` part.
  A Claude tool loop whose opening round the memo does not have (a `/resume`
  mid-loop) is sent **without** thinking for that round rather than with a
  reconstructed block the server would refuse — the reference that papers over
  it with synthetic `[Tool execution completed.]` / `[Continue]` turns puts
  words in both parties' mouths, and this does not.

### The stream

`StreamAccumulator` folds the SSE lines: each `data:` line is a
`GenerateContentResponse` wrapped in `{"response": …}` (unwrapped is read the
same). A `functionCall` part arrives **whole**, arguments as an object, and is
surfaced under its slot at once (`docs/ui/write-streaming.md`); `thought: true`
parts are reasoning; `finishReason` maps into the crate's words (`STOP`,
`MAX_TOKENS` → `length`, the safety family → `content_filter`, anything else
lower-cased — `MALFORMED_FUNCTION_CALL` ends a round that answered nothing,
which the agent loop asks the model to carry on from,
`docs/architecture/unfinished-replies.md`). Usage is `usageMetadata`: `promptTokenCount`
(which includes the cached share) as the input, the reply **and** the
thinking as the output. Two failure shapes are in-band: an `{"error": …}`
frame, and a raw multi-line JSON error body written **after** the frames with
no `data:` framing at all. A stream that ends with no finish reason was cut
short and fails as such — on the bounded retry, which voids the attempt.

## The catalog

`POST {base}/v1internal:fetchAvailableModels` with `{"project": …}` answers a
**map** keyed by runtime model id — the id to send, verbatim — with
`displayName`, `maxTokens` (the context window), `maxOutputTokens`,
`supportsImages`, `supportsThinking`, `quotaInfo` and `agentModelSorts`, the
backend's own curated list. `/model` lists:

- the ids `agentModelSorts` names when it names any (it is how the backend
  retires a route that still answers but fails), else every id;
- minus router slots (`chat_…`, `tab_…`), `-tiered` routers, image models,
  internal (`isInternal`, `API_PROVIDER_INTERNAL`) and disabled ones;
- with **level siblings grouped**: `gemini-3.8-flash-low`, `-medium` and
  `-high` are one row, `gemini-3.8-flash`, whose Ctrl+T ladder is exactly the
  suffixes the catalog lists and whose request sends `{model}-{level}`. A
  level suffix with no siblings stays part of its id (`gpt-oss-120b-medium`,
  `claude-opus-5-5-high`). Grouping only exact ladder suffixes is what keeps
  the reconstruction exact: the id sent is always one the catalog listed.
- A row's window is the smallest of its variants', its image input `false`
  when any variant says so, and it reasons when the record says
  `supportsThinking` — for Claude only on a `-thinking` id, the one Claude
  route the backend takes a thinking budget on.

## `/usage`

The same `fetchAvailableModels` answer carries each model's `quotaInfo`
(`remainingFraction` 0–1, `resetTime` RFC 3339): one bar per `/model` row,
grouped by family (Gemini, then Claude, then GPT-OSS), the used share being
`1 - remainingFraction` of the row's **scarcest** variant, with that
variant's reset (a `quotaInfo` with no fraction is spent). The plan is
`loadCodeAssist`'s `paidTier` name, else its `currentTier` — `currentTier`
reads `free-tier` even on a paid account — and the account is `userinfo`'s
email. The pools are shared between models, so the page says so. There is no
dashboard link: Google publishes no page of Antigravity quotas.

## When it doesn't work

`antigravity::auth_advice` rewrites the refusals a user can act on — on the
chat stream through `OpenAiClient::explain`, and on every other call
(`loadCodeAssist`, the catalog) through `antigravity::explained`, so none
reaches the screen as raw JSON:

| status | what the server said | what the user reads |
| --- | --- | --- |
| 400 | `invalid_grant` (refresh) | the sign-in is no longer valid — `/login` again |
| 401 | — | the same |
| 403 | `details[].metadata.validation_url` | Google wants the account verified — the `accounts.google.com` link, and only such a link |
| 403 | anything else | the account cannot use Antigravity, with the server's reason |
| 404 | `Requested entity was not found` | the model is not available to this account (any more) — pick another |
| 429 | `RESOURCE_EXHAUSTED` with a reset | the model's quota is used up, and when it resets |
| 429 | rate-limit wording | a transient throttle (the bounded retry already tried) |
| 503 | `No capacity available` / `MODEL_CAPACITY_EXHAUSTED` | Google has no capacity for the model right now |

## What this does not do

- **No account rotation, no quota-pool fallback.** One account per sign-in,
  one pool (Antigravity's); the community plugins' multi-account rotation and
  their switch to Gemini CLI's pool on a 429 are policy this client does not
  take on its users' behalf.
- **No Google Search grounding or image generation** — both are separate RPCs
  with their own models, not chat.
- **No headless paste.** The browser must reach `localhost`; on a remote
  machine, forward the port the sign-in page names
  (`ssh -L 51121:127.0.0.1:51121 …`).
- **No interleaved Claude thinking** (`anthropic-beta:
  interleaved-thinking-…`): Claude thinks at the start of each turn.

## Environment

| variable | effect |
| --- | --- |
| `ANTIGRAVITY_REFRESH_TOKEN` | the stored credential (`.env`, or exported) |
| `ANTIGRAVITY_PROJECT_ID` | the Cloud Code project to bill, outranking discovery |
| `ALTER_ZERO_ANTIGRAVITY_USER_AGENT` | replaces the agy CLI `User-Agent` |
| `ALTER_ZERO_GOOGLE_OAUTH_BASE` | where the token and userinfo calls go (the smoke suite's stub) |

## Files

- `src/llm/antigravity.rs` — the sign-in, the token cache, the project, the
  catalog and quota parses, the advice.
- `src/llm/cloud_code.rs` — the wire translation and the stream.
- `src/llm/auth.rs` — `RequestAuth::project`.
- `src/tui/workers.rs` — the sign-in worker.
