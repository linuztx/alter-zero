# Ollama as a provider

Run local models — `/login` → **Use an API key** → **Ollama** → Enter (the
default host is offered), then `/model` lists what `ollama pull` fetched.

Two providers ship, over one **fourth wire format** (`wire_api = "ollama"`,
after Chat Completions, Responses and Messages — `docs/chatgpt.md`,
`docs/claude.md`):

| provider id | reaches | credential |
| --- | --- | --- |
| `ollama` | a server you run (`http://127.0.0.1:11434` unless `OLLAMA_HOST` says otherwise) | none — a stored `OLLAMA_API_KEY` rides as a bearer if a proxy wants one |
| `ollama_cloud` | `https://ollama.com` | an API key from ollama.com/settings/keys |

A local server signed in with `ollama signin` lists its `-cloud` models under
the `ollama` provider too; nothing is loaded locally for those and the window
rule below leaves them at the cloud's maximum.

Every wire shape in this document was **observed** against Ollama 0.33.2
(the current release when this was written) rather than read off the docs,
which lag the server — and cross-checked against the server's own source
(`api/types.go`, `server/routes.go`, `envconfig/config.go`).

## Why the native API, and not Ollama's `/v1`

Ollama also serves an OpenAI-compatible `/v1/chat/completions`, and the
existing Chat Completions wire would have driven it with nothing new. It was
not used, for one reason with two halves:

**The context window.** Ollama loads a model with a context of **4096
tokens** on most machines (a VRAM-tiered default since 0.15.5: 4K under
23 GiB, 32K to 47 GiB, 256K above), and a prompt past it is truncated
**silently** — `server/prompt.go` drops messages from the front, keeping the
system messages and the latest one, with no field in the response and a
`Debug`-level log line nobody has on. An agentic turn — a 5K-token persona
plus tool schemas, then the first file read — overflows that on its first
round, and what it loses is the conversation. The native `/api/chat` takes
`options.num_ctx` per request; `/v1` has no `options` field at all, and gin's
default binder discards an unknown one **without an error**. Cline reached the
same conclusion the same way and moved off `/v1` for exactly this.

**The metadata.** `/v1/models` answers four fields per model (`id`, `object`,
`created`, `owned_by`) — no window, no capabilities. `/api/tags` and
`/api/show` answer all three questions the picker asks (below).

So `ollama` is a wire format, not a base URL: `src/llm/ollama.rs` translates
in both directions between the native shape and the Chat Completions currency
the rest of the crate uses, exactly as `responses.rs` and `anthropic.rs` do.
Nothing above `OpenAiClient` learns a fourth format exists: the agent loop,
the transcript, the rollout and the derived context are untouched.

## What a provider file says

```toml
[providers.ollama]
name = "Ollama"
auth = "optional_key"          # ← no key needed; a stored one still rides
wire_api = "ollama"            # ← the native shape
api_key_env = "OLLAMA_API_KEY"
api_base_env = "OLLAMA_HOST"   # ← Ollama's own variable replaces the base

[providers.ollama.kwargs]
api_base = "http://127.0.0.1:11434"

[providers.ollama_cloud]
name = "Ollama Cloud"
wire_api = "ollama"
api_key_env = "OLLAMA_API_KEY"

[providers.ollama_cloud.kwargs]
api_base = "https://ollama.com"
```

Two keys are new to the file:

- **`auth = "optional_key"`** — `AuthScheme::OptionalKey`. Every other
  scheme's request cannot be made without a credential, so `ModelConfig::
  is_usable` refused a keyless config and the boundary fell back to the dummy.
  A local server takes no credential at all; this scheme makes the base alone
  enough, while a stored key rides as `Authorization: Bearer` exactly as a
  required one does (a local server ignores the header; a hosted one needs
  it). `auth::request_auth` answers it with **no I/O**, like `ApiKey`.
- **`api_base_env`** — an environment variable whose value **replaces**
  `kwargs.api_base`. Resolved at the boundary the way the key is (process env
  first, then the `.env` store) into `Selection::api_base`, and — on this
  wire — run through Ollama's own grammar.

### `OLLAMA_HOST`, in Ollama's grammar

`ollama::host_url` is `envconfig.Host()` ported: no scheme means `http`, no
port means 11434 — but an explicit scheme brings its *own* default port
(`http://x` is port 80), the bare `ollama.com` is the cloud over https, a
path survives (a reverse proxy's subpath), an invalid port falls back to the
default, and an unspecified bind address (`0.0.0.0`, `::`) becomes the
loopback a client can actually connect to. Empty is the local default. The
file's own base takes the same road, so a full URL comes out unchanged.

## Configured means pointed at

`configured` — the ✓ in `/login`, and which providers `/model` fetches —
means "a credential resolves" everywhere else. A provider that needs none
would be configured always, and every `/model` open would then fetch from a
server most users don't run and paint `Ollama unavailable` in red beside the
lists that did load. The smoke suite's `/model` phases, which expect the
`run /login to add one` hint with nothing configured, would read the same
refusal.

So a host-configured provider is configured when it is **pointed at**:
`OLLAMA_HOST` resolves (process env or `.env`), or a key resolves, or
`ALTER_ZERO_PROVIDER` names it. That is what the `/login` row writes.

### The `/login` host field

`/login` → **Use an API key** → **Ollama** opens the key step as a **host
field** rather than a secret one (`KeyKind::Host`, a field on
`ProviderChoice` the boundary fills from `api_base_env`): the title asks
`Enter your Ollama host`, the typed value shows **as typed** (a URL typed
blind is a URL typed wrong), the empty field shows the default, and **Enter
on the empty field saves the default** — `OLLAMA_HOST=http://127.0.0.1:11434`
into `.env`, which is what nearly everyone wants and what makes the provider
configured from then on. A typed host is saved verbatim in Ollama's grammar
(`myhost:11434`, `0.0.0.0`, `https://host`); the config normalizes it, not the
field. A secret field still refuses an empty Enter.

`ollama_cloud` is an ordinary key provider: its row asks for the key.

## The context window

The number the footer gauges against — `{used}/{window} ({pct}%)`, the
auto-compact trigger — **is the number the request asks the server for**
(`options.num_ctx`). Any other arrangement gauges against a window the server
does not hold. `ModelConfig::context` carries it; the boundary's `config_for`
fills it from the same `context_window()` the gauge reads, so the two cannot
disagree; and the startup capability probe, which learns the window from the
listing, rebinds the backend on this wire alone (`wire_sends_context`) so the
very next request carries it.

### The rule

`ollama::context_window`, per model, in order:

1. the Modelfile's own `num_ctx` (`/api/show` → `parameters`) — the user's
   per-model choice, and the reason the listing shows every model rather than
   trusting `/api/tags` alone (Roo Code learned this the hard way: forcing the
   discovered window over a user's `PARAMETER num_ctx 4096` OOMed their GPU);
2. else the server's configured default, `OLLAMA_CONTEXT_LENGTH`, read from
   **this** process's environment as a mirror of what `ollama serve` was
   started with;
3. else `DEFAULT_NUM_CTX_CAP`, **32 768** — Ollama's own middle VRAM tier and
   Cline's default: a bare install's 4096 is unusable for an agent, and the
   model's own maximum (128K, 256K) is what fills a GPU;

**never past what the model can hold** (its `context_length`; the server
clamps there anyway, and a gauge against more would lie). A model whose
maximum is unknown takes the first two or nothing — inventing a window for it
would gauge against a number nobody stated. The cloud serves a model at its
maximum, so a direct `ollama_cloud` listing and a local server's `-cloud`
proxies are **uncapped**.

`ALTER_ZERO_CONTEXT_WINDOW` outranks all of it, as it outranks every
provider's reported window — and on this wire it is also what is *sent*, so
it is the one knob that raises a local model past the cap.

Two consequences worth knowing:

- **Changing `num_ctx` reloads the model.** The window is a load-time option,
  so a switch pays the load latency once; it is stable for a session
  afterwards (which is also why the window is a fixed per-session value and
  not, as aider does it, sized to each request — an agentic loop that changed
  it per round would reload per round).
- **Overflow still truncates.** With the gauge and auto-compaction keeping a
  turn under 90% of the window this is rare, and Ollama's `truncate: false`
  (which errors instead) is deliberately not sent: a single oversize tool
  output would then fail the round, and the compaction that could fix it
  sends the same context and fails too. Dropping the oldest messages and
  continuing is the better degradation.

## Model metadata

The listing answers all three capability questions natively — no hardcoded
table — from **`/api/tags` plus one `/api/show` per model**:

| `ModelEntry` field | where it comes from |
| --- | --- |
| `context` | the rule above, over `details.context_length` (tags, 0.19+) / `model_info["<arch>.context_length"]` (show) and the Modelfile `num_ctx` (show only) |
| `vision` | `capabilities` contains `vision` |
| `reasoning` | `capabilities` contains `thinking` |

`/api/tags` carries `capabilities` since 0.9.1 and the window since 0.19; a
tags record is what the fetch **falls back to** when a model's show fails,
so an older server still lists with whatever it said. The capability list is
explicit, so absence is the answer — a record with the list and no `vision`
cannot see (`Some(false)`, and the backend degrades attachments), where a
record with no list at all stays unknown, the optimistic default every other
provider gets. A model with the list and no `completion` (an embedding model)
has no chat surface and is not offered. The show body is decoded to the few
fields the record needs and read bounded (`MODELS_BODY_MAX_BYTES`); its
`tensors` list is skipped unread.

### Thinking

`thinking` in the capabilities makes an **on/off reasoner** — qwen3,
deepseek-r1: the Ctrl+T cycle is `off ↔ on`, and the wire is `think: false` /
`think: true`. The one exception is the **gpt-oss** family
(`details.family == "gptoss"`), whose thinking is a **level** it cannot turn
off: the ladder is `low → medium → high` (`think: "low"` …), with no `Off`
rung, since `think: false` there is ignored and a footer reading `off` would
lie. The crate's wider ladder clamps onto Ollama's three the way Ollama's own
OpenAI layer clamps `reasoning_effort`: `minimal` is `low`, everything above
`high` is `high`.

Three facts the wire is built around, each verified live:

- **Thinking is on by default.** A thinking model with no `think` field
  thinks; the seeded mode is therefore `On`, so the footer says what the
  server does. The auto mode classifier, which clears the mode, gets the
  model's default too.
- **`think: true` on a non-thinking model is a 400** (`"gemma3:270m" does
  not support thinking`); a level string on a switched reasoner is treated as
  `true` rather than refused, so a stale persisted mode cannot fail every
  turn.
- **The trace streams as `message.thinking`**, a field of its own, and the
  accumulator still runs the Chat Completions wire's `ThinkingSplitter` over
  the content: a custom GGUF import can lack the parser that fills the field
  and emit `<think>` tags instead.

## The wire

### The request

```json
{
  "model": "qwen3:8b",
  "messages": [
    {"role": "system", "content": "…"},
    {"role": "user", "content": "what is this?", "images": ["iVBOR…"]},
    {"role": "assistant", "content": "", "tool_calls": [
      {"id": "call_1", "function": {"name": "read", "arguments": {"path": "f"}}}]},
    {"role": "tool", "content": "L1", "tool_call_id": "call_1", "tool_name": "read"}
  ],
  "stream": true,
  "tools": [ …the Chat Completions specs, verbatim… ],
  "think": true,
  "options": {"num_ctx": 32768, "temperature": 0.5}
}
```

Five shapes differ from Chat Completions, each a place a naive port breaks:

- **A tool call's `arguments` is a JSON object**, in both directions. A
  string there — the Chat Completions currency — is a 400 (`Value looks like
  object, but can't find closing '}' symbol`); a call the model truncated
  degrades to `{}` rather than failing the request.
- **A tool result names its tool.** `tool_name` (0.9.6+) beside
  `tool_call_id` (0.12.10+), read off the assistant call seen earlier in the
  same list; a result whose call was never seen (an old rollout) omits the
  name rather than guessing one. Both fields are ignored by a server that
  predates them.
- **An image is bare base64** in the message's `images` array — no `data:`
  prefix, no media type. A non-`data:` URL has no pixels to send and is
  dropped rather than 400ing the turn.
- **`tools` pass through verbatim** — Ollama's schema *is* the Chat
  Completions one — minus `tool_choice`, which is not a field here.
- **A file-configured `options` table merges under the session's own
  keys**, so a `num_gpu` in `providers.toml` rides along without a stale
  `num_ctx` beside it overriding the gauge; every other kwarg merges at the
  top level as on every wire.

`temperature` rides `options`, where the `/settings` **Temperature** row
puts it; `stream: true` is stated even though it is the default.

### The stream

NDJSON, one object per line — which is exactly what the shared `pump_lines`
byte loop already hands out, so Esc is honoured identically on all four
wires. `ChatAccumulator` folds it:

```
{"message":{"role":"assistant","content":"","thinking":"Okay"},"done":false}
{"message":{"role":"assistant","content":"Hi"},"done":false}
{"message":{"role":"assistant","content":"","tool_calls":[{"id":"call_v1gr5rr3",
  "function":{"index":0,"name":"get_weather","arguments":{"city":"Paris"}}}]},"done":false}
{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop",
  "prompt_eval_count":141,"eval_count":120, …}
```

- **A tool call arrives whole** on one frame, never as argument fragments —
  its object arguments are re-serialized to the string the executor parses,
  its id kept (`call_` + 8 characters, minted server-side since 0.12.10) or
  synthesized (`call_{n}`), and the name plus arguments are surfaced for the
  token tally as the call lands.
- **`done_reason` is `stop` even for a tool round.** The crate's
  `finish_reason` becomes `tool_calls` whenever a call was seen — what the
  agent loop reads to loop again.
- **Usage is the final frame's `prompt_eval_count` / `eval_count`.** Verified
  live that `prompt_eval_count` reports the **whole** prompt on a cache hit
  (85 both times for a repeated request), so the gauge can trust it — unlike
  Anthropic's uncached remainder (`docs/claude.md`). No cache or reasoning
  breakdown exists, so those stay 0 and the `Thought for …` cell keeps its
  tokenizer estimate.
- **An in-band `{"error": …}` line** (a 200 that fails mid-stream) fails the
  turn; a non-2xx status arrives from the transport as an ordinary API error
  and is explained below.

`accept: application/x-ndjson` names what the request expects.

## When it doesn't work

Most Ollama failures are about the *machine*, and `openai::explain` rewrites
them on this wire (`ollama::advice`, `ollama::connection_advice`) into words
that say what to do:

| what happened | what it says |
| --- | --- |
| connection refused | `Could not reach Ollama at {base} — is `ollama serve` running? …` |
| the host won't resolve | check `OLLAMA_HOST` |
| 404 `model 'x' not found` | pull it with `ollama pull x`, or pick another with /model |
| 400 `… does not support tools` | turn Tools off in /settings, or pick a model that does |
| 400 `… does not support thinking` | press ctrl+t to turn it off |
| 400 `Multimodal data provided, but model does not support …` | the attachment was refused; pick a vision model |
| 401 / 403 | set `OLLAMA_API_KEY` (a cloud key), or paste one with /login |
| 429 | rate-limited; wait |
| 502 | the cloud model behind a proxy could not be reached |

A refused connection also retries three times first (`llm::retry` treats a
transport failure as transient) — instantly, since nothing is listening.

**Cold loads are slow.** Ollama sends nothing until the model is in memory,
so a first request on a large model can sit for tens of seconds before its
first byte; `NET_OP_TIMEOUT` (120 s) bounds it, and a load that outlasts it
surfaces as a retry — the load continues on the server meanwhile, so the
retry usually lands. Changing `num_ctx` (a `/model` switch, an
`ALTER_ZERO_CONTEXT_WINDOW`) pays the load again once.

## Testing

The pure module is unit-tested against frames and bodies captured from
0.33.2: the host grammar, the payload, the message translation, the NDJSON
fold, the catalog parse and the window rule, the advice. The boundary is
covered by `tests/live_ollama.rs` — `#[ignore]`d live tests against a server
holding three small models (`qwen3:1.7b` for tools and thinking,
`gemma3:270m` for a model with neither, `moondream` for vision) that drive
the catalog, a thinking turn, a tool round, an image round, the window
reaching `/api/ps`, and the explained refusals:

```sh
ollama pull qwen3:1.7b && ollama pull gemma3:270m && ollama pull moondream
cargo test --test live_ollama -- --ignored --nocapture
```

The tool round runs with thinking **on**: `qwen3:0.6b` thinks and chats over
this wire but cannot form a call against the four real tool schemas (it
emits a broken one the server's parser drops, or a raw JSON blob as
content), and even the 1.7b one only does so reliably after reasoning about
it — the model's limit, not the wire's, and worth knowing when picking a
local model for agentic work.

`scripts/smoke.sh` unsets `OLLAMA_HOST` and `ALTER_ZERO_PROVIDER` beside its
`*_API_KEY` scrub, so a developer's own Ollama pointer never makes the
suite's `/model` phases fetch from a server it doesn't run.

## Environment

| variable | effect |
| --- | --- |
| `OLLAMA_HOST` | where the `ollama` provider's server is, in Ollama's grammar; setting it (or `/login` writing it) is what makes the provider configured |
| `OLLAMA_API_KEY` | a bearer — the cloud's key, or a proxy's; optional for a local server |
| `OLLAMA_CONTEXT_LENGTH` | the server's configured default window, mirrored into the window rule |
| `ALTER_ZERO_CONTEXT_WINDOW` | outranks the rule, and is sent as `num_ctx` |
| `ALTER_ZERO_PROVIDER=ollama` | also counts as pointing at it |

All resolve the ordinary way (`docs/llm.md`): a real process environment
variable wins over the `.env` store.

## Known limitations (v1)

- **A model without the `tools` capability needs Tools off.** The picker
  lists it (it chats, and a vision-only model is worth having), but a
  tools-on request is a 400 — explained, and one `/settings` toggle away —
  rather than the tool set quietly shrinking to fit the model. Reading the
  capability into the backend's tool set is the next step.
- The window is one value per session; there is no `/settings` row for it
  yet (`ALTER_ZERO_CONTEXT_WINDOW` is the knob).
- The listing is one `/api/show` per model, sequential: a cloud catalog of
  many models takes a few seconds to land in the picker (the other providers'
  lists show meanwhile).
- No model preload: the first request of a session pays the load.
- `ollama_cloud`'s `num_ctx` is sent like the local one's; whether the cloud
  honours it is undocumented, and the entry's uncapped window makes it the
  model's maximum either way.
