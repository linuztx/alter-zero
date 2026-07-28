# Prompt caching and real usage accounting

Two features that land together, because the second is how you *see* the first
working:

1. **Prompt caching** — every request is shaped so the provider can serve the
   repeated prompt prefix (system prompt + conversation so far) from its
   cache: automatically where the provider caches implicitly, and via explicit
   `cache_control` breakpoints where it doesn't. An agentic session re-sends
   its whole growing context on every round, so this is the difference between
   re-billing the full conversation each round and paying ~1/10th for the
   prefix (verified live: an 8k-token prefix on `anthropic/claude-haiku-4.5`
   via OpenRouter cost $0.0101 to write and $0.0008 to re-read — 92% off).
2. **Real usage accounting** — the request asks for the provider's final
   `usage` frame (`stream_options: {"include_usage": true}`), and the app's
   token tally snaps to it instead of trusting the app-side `tiktoken`
   estimate, which never saw the system prompt, the re-sent context, or the
   cache discount. The committed turn summary then reads
   `Done for 12s · 8.2k tokens (8k cached)` — the cached share being the
   visible proof caching worked.

Both are provider-shaped per the two vendors' own guides: OpenRouter's
[prompt-caching best practices] and Venice's [prompt-caching feature guide].

[prompt-caching best practices]: https://openrouter.ai/docs/guides/best-practices/prompt-caching
[Venice's prompt-caching feature guide]: https://docs.venice.ai/guides/features/prompt-caching

## The two caching worlds

OpenAI-compatible providers split in two:

- **Implicit** — OpenAI, DeepSeek, Grok, Gemini 2.5, Moonshot, and **Venice**
  cache repeated prefixes automatically; the request needs no markers. Venice
  even injects Anthropic's markers itself for the Claude models it serves
  ("Venice adds these automatically for system prompts and conversation
  history"), so adding our own could double past Anthropic's 4-breakpoint
  limit — Venice-served bare `claude-*` ids are deliberately left implicit.
- **Explicit** — Anthropic and Qwen, when reached through an OpenRouter-style
  aggregator, only cache the prefix up to a text block marked
  `cache_control: {"type": "ephemeral"}`. No marks → no caching → every
  agentic round re-bills the whole conversation at full price.

The pure [`llm::cache`](../src/llm/cache.rs) module owns the split:
`needs_cache_breakpoints(model)` is true for the namespaced `anthropic/…` and
`qwen/…` ids (the OpenRouter form; everything else stays untouched), and
`apply_cache_breakpoints(messages)` rewrites the wire `messages` JSON — after
serialization, so the `ChatMessage` types and every other provider's payload
stay byte-identical.

## Breakpoint placement (≤ 3 of Anthropic's 4)

```
[system  ← ①] [user] [assistant] [user ← ③] [assistant+tool_calls] [tool ← ②]
```

1. **The system message** — the big stable prefix (persona + environment +
   tools note) shared by every request of the session.
2. **The last cacheable message** — a *moving* breakpoint tracking the
   conversation frontier. Mid-turn, each agent round ends on the round's tool
   results, so the next round reads everything so far from cache and only the
   new results are fresh input; across turns it sits on the newest user
   message. Verified live that OpenRouter's Anthropic routing accepts the
   marker on a `tool`-role message (and a Qwen tool round completes with it).
3. **The last `user` message before ②** — insurance for the provider's
   bounded lookback when many blocks land between two requests (a big
   parallel batch).

Marking converts a plain-string content into the one-text-part array (the only
shape that can carry `cache_control`); a parts array (a vision message) gets
the marker on its **last non-empty text part** — image parts can't carry one.
Empty/whitespace contents are skipped entirely (providers reject empty marked
text blocks); the frontier walks back past them. Minimum-size rules
(1k–4k tokens depending on the model) are the provider's own: a small
conversation's marks are simply ignored until the prefix grows past the
threshold, so there is nothing to gate app-side.

## Cache affinity: `prompt_cache_key` + OpenRouter's `session_id`

A cache hit needs the request to land where the cache *is*. Two routing hints
ride every request once the boundary mints its per-process key
(`main.rs::session_cache_key` — pid + startup time, the same key for every
backend built this session):

- **`prompt_cache_key`** — the standard OpenAI parameter, verified accepted
  by both shipped providers (Venice documents it as its cache-affinity hint).
- **`session_id`** — OpenRouter's own session pin, sent **only when the chat
  `api_base` is OpenRouter's** (the `openrouter` marker in the base URL):
  OpenRouter routes one model across several upstream providers, and a cache
  written on Amazon Bedrock is unreadable from Google Vertex. Venice rejects
  unknown body keys (`Unrecognized key(s): 'session_id'` — verified live), so
  the pin stays gated.

## The usage pipeline

```
build_payload: stream_options.include_usage      (openai.rs — kwargs can override)
        ↓
final SSE frame: {"usage": {prompt_tokens, completion_tokens,
                            prompt_tokens_details: {cached_tokens, cache_write_tokens, …},
                            cache_read_input_tokens, …}}     (null on delta frames)
        ↓ parse_sse_usage → StreamOutcome::usage             (openai.rs, pure — all alias
        ↓                                                     spellings normalized)
stream_round → StreamEvent::Usage(TokenUsage)                (backend.rs — one per round)
        ↓
App::apply_usage: accumulate + SNAP status.tokens            (app/turn.rs, pure)
        ↓
TurnSummary { tokens, cached } → "Done for 12s · 8.2k tokens (8k cached)"
                                                             (ui::summary_lines; persisted
                                                              by session.rs, serde-defaulted
                                                              so old rollouts still parse)
```

- [`TokenUsage`](../src/stream.rs) carries `input`, `output`, `cached` (reads,
  a subset of `input` per the OpenAI accounting shape), and `cache_write`.
  The parse normalizes the spelling zoo: `prompt_tokens_details.cached_tokens`
  (OpenRouter/OpenAI), the top-level `cache_read_input_tokens` /
  `cache_creation_input_tokens` aliases (Venice, Anthropic-style shims), and
  OpenRouter's `cache_write_tokens`. A `"usage": null` delta frame or an
  all-zero block is not a report.
- **Snap, don't add**: the tiktoken estimate keeps ticking live between
  frames (chunks, reasoning, tool output — unchanged), but each round's usage
  frame **replaces** the tally with the accumulated real total
  (`App::apply_usage`). So the status is always "everything billed so far,
  plus the current round's live estimate", and the final number is the
  provider's own accounting. The tally can jump *up* at a round boundary —
  that's the estimate learning about the system prompt and re-sent context it
  never counted; the truth, not a glitch.
- The tally now reaches six digits on long agentic turns, so the status line
  and summary humanize it (`ui::format_token_count` — `8.1k`, `154.3k`,
  `1.2M`; bare below a thousand, so the dummy's small estimates render as
  before).
- The dummy sends no `Usage` events: `smoke.sh` output — including the bare
  `Done for Ns` summaries its phases assert on — is unchanged.

## Configuration

Nothing to configure. `stream_options` is the one field a provider could
choke on, and `providers.toml` kwargs merge *after* the base payload, so a
misbehaving shim can override it from the file
(`[providers.x.kwargs] stream_options = …` — or null it out). The breakpoint
rules key off the model id, the affinity key off the api_base; adding a new
provider block needs no caching knowledge.

## Live verification

`tests/live_openrouter.rs` (all `#[ignore]`d; run on purpose with real keys):

```sh
OPENROUTER_API_KEY=sk-or-… cargo test --test live_openrouter -- --ignored --nocapture \
  live_usage_frame_reaches_the_app \
  live_anthropic_prompt_cache_writes_then_reads \
  live_qwen_accepts_cache_breakpoints_on_a_tool_round
A0_VENICE_API_KEY=sk-a0-… cargo test --test live_openrouter -- --ignored --nocapture \
  live_venice_reports_usage_and_hits_its_cache
```

Observed on the real wire (2026-07-24):

| provider | turn 1 | turn 2 |
| --- | --- | --- |
| OpenRouter · `anthropic/claude-haiku-4.5` | `cache_write: 8004` | `cached: 8004` |
| Venice (a0 proxy) · gpt-4o-mini | `input: 6749, cached: 0` | `cached: 6656` |

## Known limitations

- The per-model **minimum cacheable sizes** and TTLs (5 min – 1 h) are the
  provider's; a short session below the threshold caches nothing, silently.
- `needs_cache_breakpoints` is a **prefix list** (`anthropic/`, `qwen/`), not
  a capability probe; a future explicit-caching provider means one more
  prefix (and a provider that *ignores* `cache_control` merely ignores it).
- The **status line** shows the snapped total but not the cached share; the
  cached count surfaces in the committed `Done …` summary (and the Ctrl+O
  transcript, where summaries re-render). Keeping the live line's clause set
  stable avoids re-teaching `docs/status-indicator.md`'s geometry for a
  number that only settles at round boundaries anyway.
- OpenRouter's per-request `cost` field is parsed past, not surfaced — the
  tally stays in tokens (Venice bills in a different unit entirely).
