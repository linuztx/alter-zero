# Signing in: subscriptions, API keys, and GitHub Copilot

`/login` used to ask one question — *which provider's key are you pasting?* —
because every provider it knew took a pasted key. GitHub Copilot doesn't: it is
a **subscription** you sign in to, through GitHub's device flow, and what comes
back is a token the user never sees, let alone types. So the flow gained a root
step that asks the prior question first, and a second half behind it.

```
/login
  ├─ Use a subscription ──► GitHub Copilot ──► device code ──► (approved)
  └─ Use an API key ──────► Agent Zero API / OpenRouter ──► paste the key
```

Both halves end in the **same place**: a secret in `~/.alter-zero/.env` under
the provider's `api_key_env`. That is the whole reason the split is cheap —
`/model`, the capability probe, the ✓ marks, the startup key resolution and the
next launch all keep working with no second mechanism to teach them.

## The pages

Five steps, one `KeyOnboarding` (`src/app/login.rs`), all sharing the `/model`
picker's frame. **Every title is cyan** (`LOGIN_TITLE_COLOR` = the palette
accent the whole picker family selects with), so the flow's headings read as
one rather than as a fourth colour to learn.

### `KeyStep::Method` — the root

```
────────────────────────────────────────────────────────────────────────────

  ❯

→ Use a subscription
  Use an API key

  ↑↓ navigate  enter select  escape/ctrl+c cancel

────────────────────────────────────────────────────────────────────────────
```

No title — the two rows *are* the question — and no `(1/2)` counter, which
would say nothing they don't. Type-to-filter works here like every list step
(one grammar for all three), and Esc closes: this is the root.

### `KeyStep::Subscription`

```
────────────────────────────────────────────────────────────────────────────

  Use a subscription

  ❯

→ GitHub Copilot  Sign in with your GitHub account ✓
  (1/1)

  ↑↓ navigate  enter sign in  esc back

────────────────────────────────────────────────────────────────────────────
```

The row shape is the provider list's, with the file's `description` in the dim
tail slot where a key row carries `[ENV_VAR]`, and the same green ✓ for
"already signed in" — the *same* check, resolved the same way, because a
subscription's token lives in the same store.

### `KeyStep::Device`

```
────────────────────────────────────────────────────────────────────────

  Sign in to GitHub Copilot

  Visit https://github.com/login/device
  and enter this one-time code

     ╭─────────────╮
     │  C363-262E  │
     ╰─────────────╯

  Waiting for approval… · expires in 14:11

  c copy code  esc cancel

────────────────────────────────────────────────────────────────────────
```

- **No browser is launched.** The URL is text the user opens themselves. A TUI
  that shells out to `xdg-open` is guessing at a desktop it may not be on (an
  SSH session, a container, a headless box), and the failure mode — a stray
  process, or a browser opening on the *wrong machine* — is worse than a URL
  the user can click in any modern terminal anyway.
- The box is **sized to the code**, so a provider that issues a longer one
  still gets a snug frame.
- `c` copies the code through `/copy`'s own clipboard path (arboard with the
  OSC 52 fallback, `docs/copy.md`); only the confirmation differs, because
  "Copied last message" would be a lie here.
- The countdown is a **boundary clock read**, injected per draw
  (`App::set_device_remaining`) exactly like the status line's elapsed — the
  pure core never reads a clock. `wants_animation_frames` includes an open
  device page so the frame chain re-arms and the count actually moves with
  nothing else on the wire.
- The **cursor is hidden** here (`ui::cursor_visible`): the page is a wait, not
  a field, and a kitty cursor trail would streak across it on every tick. Its
  seat still tracks the code box rather than the frame's corner.
- A failure **replaces the wait line, in red, with the page still up** — closing
  on the error would take the explanation away with it. Only Esc takes it down,
  back to the subscription list.

### `KeyStep::Provider` / `KeyStep::Key`

Unchanged but for the cyan `Use an API key` title, a second dim hint row naming
the step's keys, and Esc now stepping *back* to the method root rather than
closing outright. The list shows exactly the providers whose key is pasted —
**Agent Zero API** and **OpenRouter** — with a subscription provider filtered
out, since offering a key field for a flow that has none is a dead end.

## What a provider file says

One key decides which list a provider lands in:

```toml
[providers.github_copilot]
name = "GitHub Copilot"
auth = "github_copilot"                        # ← a sign-in, not a pasted key
description = "Sign in with your GitHub account"
api_key_env = "GITHUB_COPILOT_TOKEN"

[providers.github_copilot.extra_headers]
Editor-Version = "vscode/1.104.3"
Editor-Plugin-Version = "copilot-chat/0.26.7"
Copilot-Integration-Id = "vscode-chat"
User-Agent = "GitHubCopilotChat/0.26.7"
Openai-Intent = "conversation-panel"

[providers.github_copilot.kwargs]
api_base = "https://api.githubcopilot.com"
```

`AuthScheme` (`src/llm/config.rs`) is `api_key` by default and **falls back to
it for an unrecognised value** (`#[serde(other)]`): a provider file written
against a newer build must degrade, not fail the whole parse.

## The two token layers

Conflating them is the classic bug in every third-party Copilot client, so
`src/llm/copilot.rs` keeps them apart by name:

| | what it is | lifetime | where it lives |
| --- | --- | --- | --- |
| **OAuth token** (`ghu_…`) | what the device flow mints | long-lived | `.env`, like any other provider's secret |
| **Copilot bearer** (`tid=…:mac`) | what the API actually takes | ~30 min | memory only, never written |

`copilot::authorize` exchanges the first for the second and caches it in a
process-global map keyed by the OAuth token — a turn's rounds, the `/model`
fetch and every subagent's thread need the same bearer, and re-exchanging per
request would spend a network round trip on each against an endpoint GitHub
rate-limits.

**The cached lifetime comes from `refresh_in`, not `expires_at`.** A user whose
clock runs ahead gets an `expires_at` already in the past, and keying off it
re-exchanges on *every single request*. `refresh_in` is a duration and immune to
that; a minute of skew comes off it for clock drift and in-flight latency.

### The seam

`copilot::request_auth(cfg)` answers *(bearer, base override)* for any config:

- **`AuthScheme::ApiKey`** → the stored key and no override, with **no I/O at
  all**. Every existing provider's path is byte-identical to what it was.
- **`AuthScheme::GithubCopilot`** → the exchanged bearer and the account's own
  host. A config with no stored token resolves to `(None, None)` rather than
  calling out: the `/model` picker builds configs for providers that were never
  signed in to, and an exchange attempt there would hang the fetch on a request
  that can only fail.

Both `openai::stream_chat` and `models::fetch_models` go through it. The base
override matters: a Business/Enterprise seat is served from
`api.business.githubcopilot.com`, which the token response names in
`endpoints.api` and the provider file cannot know.

## The request identity

The static half rides `extra_headers` in `providers.toml`. Three headers are
**per-request** and live in `openai::copilot_request_headers`:

- **`X-Initiator`** is *billing-relevant*, not cosmetic: GitHub charges a
  premium request for a user-initiated round and nothing for the agent's own
  tool round trips. The rule is the last message's role — a round the user's
  own text closes is theirs; one closing on a tool result (or on the
  assistant's own text, as a continuation does) is the agent's. Marking every
  round `user` would bill the user several times over for one question.
- **`Copilot-Vision-Request: true`** whenever a round carries an image part,
  without which the API answers `400 missing required Copilot-Vision-Request
  header for vision requests`.
- **`X-Request-Id`** — what GitHub support asks for when a request misbehaves.
  The per-session cache key doubles as one.

The **payload** is trimmed the same way: no `prompt_cache_key`. Copilot's proxy
answers a request shape it doesn't recognise with `model_not_supported` — an
error that blames the *model* — and its caching is server-side, so the affinity
key buys nothing there worth that risk. The same value rides `X-Request-Id`
instead.

`Editor-Version` and `Copilot-Integration-Id` are the two that *fail loudly*
when wrong: missing the first is a `400 missing Editor-Version header for IDE
auth`, and an integration id the server doesn't know is a `400 The requested
model is not supported` — which reads like a model problem and isn't. The
`vscode-chat` id is paired with the VS Code OAuth client id the device flow
uses; the two are checked together server-side.

## Reasoning, vision and the context window

Copilot's `/models` publishes richer metadata than any other provider here, and
`llm::models`' three per-record sniffs gained a branch each — the same
shape-sniffing pattern OpenRouter and Venice already share, so a bare
OpenAI-style list is untouched by all of them.

| | field | note |
| --- | --- | --- |
| context | `capabilities.limits.max_prompt_tokens` | falling back to `max_context_window_tokens` |
| vision | `capabilities.supports.vision` | |
| reasoning | `capabilities.supports.reasoning_effort` | an **array** — the ladder itself |

Three things are worth stating outright:

- **The prompt cap is the window.** Copilot sets the two apart — gpt-4o windows
  128000 tokens but accepts a 63997-token prompt — and the footer gauge and
  auto-compaction exist to keep a turn inside the limit the API *enforces*.
- **`supports` never sends `false`.** An unsupported capability is simply
  absent, so once the object exists, absence is the answer rather than
  "unknown".
- **`reasoning_effort` is the Ctrl+T ladder, straight off the wire.** Every
  other provider gets a hardcoded low/medium/high guess; here the model itself
  names what it takes (`["minimal","low","medium","high"]`,
  `["low","medium","high","max"]`, …), so the cycle offers exactly what the API
  will accept. A model advertising only a thinking *budget* (the Anthropic
  shape) is an on/off reasoner with no ladder.

On the wire the mode is a **top-level `reasoning_effort` string**, not the
`reasoning` object every other provider here takes — and *instead of* it, never
alongside: the object is an unknown field on this endpoint, and Copilot's proxy
answers unknown request shapes with the same misleading `model_not_supported`
400. `Off` is expressed by omitting the parameter, which is what Copilot's
opt-in reasoning does anyway.

### What the list leaves out

`entry_of` skips two kinds of record, because offering either puts a model in
the picker that fails every turn:

- **embeddings / completion** models (`capabilities.type`), which have no chat
  surface at all;
- models served **only** from `/responses` (`supported_endpoints`) — most of the
  current GPT-5 reasoning family — which answer a `/chat/completions` request
  with a 400.

An absent endpoint list means chat completions (the legacy GPT records omit it),
and a record with no `capabilities` object isn't Copilot's, so every other
provider's list passes through untouched. The picker will legitimately show a
shorter list than the VS Code model picker does.

## The flow at the boundary

`spawn_device_login` (`src/tui/workers.rs`) is the loop's fourth worker and the
only one that runs for **minutes** rather than milliseconds: it polls until the
user approves, the code expires, or its `CancelToken` trips. It reports two
messages on its own `select!` branch — the code, then the verdict — and
`tui::login` handles them:

- a **code** goes on the page and starts the countdown;
- a **success** persists the OAuth token through the same `.env` writer a pasted
  key uses, closes the flow, and toasts `Signed in to GitHub Copilot — run
  /model to use it`;
- a **failure** leaves the page up wearing the reason.

Esc, Ctrl+C and quit all reap the worker. A poll that finds its page gone
delivers nothing — there is nothing to attach the token to, and guessing a
provider would key the wrong one.

The poll's cadence is GitHub's own `interval`, slept in 200 ms naps so an Esc is
honoured about as fast as one mid-turn rather than at the end of a five-second
block. `slow_down` lengthens it (GitHub's new floor if it names one, else the
RFC's five-second penalty); `expired_token` and `access_denied` are terminal.
Both spellings of the expiry are matched — GitHub's own docs contradict
themselves on it, and matching only one turns a routine expiry into "unexpected
response".

Every poll answers **HTTP 200**, error or not: the verdict is in the body's
`error` field, so a status check alone would read a denial as a success. That is
why an unreadable body is terminal rather than "keep waiting" — the latter
loops until the code expires.

## Environment

Nothing new. The Copilot provider is an ordinary `providers.toml` entry, so
`ALTER_ZERO_PROVIDER=github_copilot` selects it, `ALTER_ZERO_MODEL` picks the
model, and `GITHUB_COPILOT_TOKEN` in the real environment outranks the `.env`
store exactly as every other provider's key does — which is also how a machine
that already has a Copilot login (a CI box, a dotfiles export) skips `/login`
entirely.
