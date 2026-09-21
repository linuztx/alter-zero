# Prompt caching and real usage accounting

Two features that land together, because the second is how you *see* the first
working:

1. **Prompt caching** — every request is shaped so the provider can serve the
   repeated prompt prefix (system prompt + conversation so far) from its
   cache: automatically where the provider caches implicitly, via explicit
   `cache_control` breakpoints where it doesn't, and with the **per-backend
   affinity hint** each backend actually routes on. An agentic session
   re-sends its whole growing context on every round, so this is the
   difference between re-billing the full conversation each round and paying
   ~1/10th for the prefix (an 8k-token prefix on `claude-haiku-4.5` costs
   about a cent to write and a tenth of that to read back).
2. **Real usage accounting** — the request asks for the provider's final
   `usage` frame (`stream_options: {"include_usage": true}` on the chat wire;
   the other three wires report it unasked), and the app's token tally snaps
   to it instead of trusting the app-side `tiktoken` estimate, which never
   saw the system prompt, the re-sent context, or the cache discount. The
   committed turn summary then reads `Done for 12s · 8.2k tokens (8k cached)`
   — or, on the turn that *primed* the cache, `(8k written)` — the visible
   proof caching worked.

Both are provider-shaped per the vendors' own guides — OpenRouter's
[prompt-caching best practices], Venice's [prompt-caching feature guide],
Anthropic's [prompt caching] reference — and **verified on the wire**, per
provider, by `tests/live_caching.rs` (the table at the end).

[prompt-caching best practices]: https://openrouter.ai/docs/guides/best-practices/prompt-caching
[Venice's prompt-caching feature guide]: https://docs.venice.ai/guides/features/prompt-caching
[prompt caching]: https://platform.claude.com/docs/en/build-with-claude/prompt-caching

## Real or estimated? Both, and here is which is which

| number | where it comes from |
| --- | --- |
| the status line's `↓ N tokens` **between** usage frames | the app-side `o200k_base` estimate of what streamed (chunks, reasoning, tool output, the tool call being generated) — exact for OpenAI models, close for the rest |
| the tally **at every round boundary**, the `Done … · N tokens` receipt, its `(N cached · N written)` | the provider's own usage frame — never estimated; a provider that reports none (the dummy, a shim that drops usage) leaves the receipt bare rather than guessing |
| the footer's context gauge after a round | real: the last round's `input + output` |
| the gauge after a history mutation (`/clear`, backtrack, `/resume`, compaction) or with no frame yet | the tokenizer estimate of the derived context, until the next frame |
| the `Thought for … · N tokens` cell | snapped to the provider's `reasoning_tokens` when the frame carries one (OpenAI, OpenRouter, the Responses wire); the estimate where none does (Anthropic folds thinking into `output_tokens`, Ollama reports none) |
| a subagent's roster tally and its own receipt | the same rule over its own frames (`docs/agent-context-gauge.md`) |

So the caching numbers are **always real**: the app never estimates a cached
share, and a `(N cached)` on the receipt is the provider's own claim of what it
served from cache.

## The two caching worlds

OpenAI-compatible providers split in two:

- **Implicit** — OpenAI, DeepSeek, Grok, Gemini 2.5+, Moonshot, Z.ai and
  **Venice** cache repeated prefixes automatically; the request needs no
  markers. Venice even injects Anthropic's markers itself for the Claude
  models it serves — verified live: a marker-free request for
  `claude-sonnet-4-6` wrote 7998 tokens on turn 1 and read them back on turn 2
  — so adding our own could double past Anthropic's 4-breakpoint limit, and
  Venice-served bare `claude-*` ids are deliberately left implicit.
- **Explicit** — Anthropic and Qwen (Alibaba), when reached through an
  OpenRouter-style aggregator, only cache the prefix up to a text block marked
  `cache_control: {"type": "ephemeral"}`. No marks → no caching → every
  agentic round re-bills the whole conversation at full price.

The pure [`llm::cache`](../src/llm/cache.rs) module owns the split:
`needs_cache_breakpoints(model)` is true for the namespaced `anthropic/…` and
`qwen/…` ids **and their `~vendor/model-latest` aliases** — OpenRouter's
`~anthropic/claude-haiku-latest` resolves to `anthropic/claude-haiku-4.5` on
Amazon Bedrock (the response's own `model` field says so), the same routing
that caches nothing unmarked; the sniff used to miss the `~`, which meant an
alias id paid full price on every round — and `apply_cache_breakpoints(messages)`
marks the round's **copy** of the `ChatMessage`s — typed, through the
`cache_control` field on a text part (`skip_serializing_if` absent, so every
other provider's payload stays byte-identical). It used to rewrite the
serialized JSON tree instead, which meant building a `Value` of the whole
conversation per round — a copy of a pasted picture's megabytes of base64 each
time (`docs/memory.md`); the copy it marks now is shallow, the pictures shared
by reference.

Anthropic's **own** API (`wire_api = "anthropic"`, `docs/claude.md`) needs no
model-id sniff — there caching is *always* explicit — so `anthropic.rs` places
the same three breakpoints on its own shape: the last system block, the last
content block of the last message, the last block of the user message before
that.

## Breakpoint placement (≤ 3 of Anthropic's 4)

```
[system ← ①] … [previous request's final message ← ③] [assistant+tool_calls] [tool ← ②]
```

1. **The system message** — the big stable prefix (persona + environment)
   shared by every request of the session.
2. **The last cacheable message** — a *moving* breakpoint tracking the
   conversation frontier. Mid-turn, each agent round ends on the round's tool
   results, so the next round reads everything so far from cache and only the
   new results are fresh input; across turns it sits on the newest user
   message. Verified live that OpenRouter's Anthropic routing accepts the
   marker on a `tool`-role message (and a Qwen tool round completes with it).
3. **The previous request's frontier** — the last cacheable message before
   the newest assistant response, including a tool result from the previous
   round. Pinning only the last human message loses intervening tool context
   when new content exceeds the provider's lookback. On the native Messages
   wire, tool results become user blocks, so its previous user message already
   identifies that boundary. Claude's own API counts consecutive tool-use and
   tool-result runs as one lookback position; a large batch alone does not
   exhaust that API's window, but a long text/image suffix can.

Marking converts a plain-string content into the one-text-part array (the only
shape that can carry `cache_control`); a parts array (a vision message) gets
the marker on its **last non-empty text part** — image parts can't carry one.
Empty/whitespace contents are skipped entirely (providers reject empty marked
text blocks); the frontier walks back past them. Minimum-size rules
(1k–4k tokens depending on the model) are the provider's own: a small
conversation's marks are simply ignored until the prefix grows past the
threshold, so there is nothing to gate app-side.

Reapplying the transform replaces its old markers instead of accumulating
them. Blank system/developer text is omitted on the native Messages wire,
where an empty marked block would reject the request.

## Keeping the conversation prefix stable

The request a new human turn sends is derived from the display history, and
it used to differ from the request the provider had cached in two ways that
cost the whole suffix: the derivation minted fresh `call_0`, `call_1`… ids
on the wires that carry an id through unrewritten (Responses, Messages,
Ollama), and it split a parallel batch into one assistant/result pair per
call. Equivalent information, a different prefix — and a cache read is a
prefix match.

Two layers keep the prefix now. The **durable** one is the record
(`docs/context.md`): every tool round announces its ids first
(`StreamEvent::RoundCalls`, in the model's order), the loop stamps them onto
the records the round's cells resolve into beside a per-round batch number
(`ToolCall::call_id`/`batch`, `TaskCallRecord`'s pair, an agent group's
entry with the `call_id` its launch answered and its verbatim `arguments`,
both carried on the launch's `AgentSpec`) **and each call's `position` in
the round** — the index the model gave it, which the batch announcement,
the task call's turn and the launch's id each recover from the announced
list — the rollout round-trips all of it, and `context::context_messages`
folds the records sharing a batch into the one `tool_calls` message they
arrived in, **sorted back into the model's order**: a subagent group
resolves before the round's ordinary calls run, so `[bash, agent, read]`
records as `[agent, bash, read]`, and without the position the replay
would send the calls, and their results, in that order — a different
prefix from the one the provider cached, from that round to the end.
Ordinary calls, task calls and subagent launches alike, every result
behind the calls in the same order, a notice committed mid-batch deferred
past it. So the derived request *is* the cached one
after a `/resume`, a `/settings` or `/model` rebuild of the backend, or a
restart; a record from a rollout written before the fields gets a synthetic
id minted clear of the recorded ones, so an old session's later turns still
pair. The **retained** one is the previous request itself: the active
backend keeps its last request and restores that exact prefix when the
rebuilt history has matching text, images, ordered tool names, arguments and
results — which still covers what no record can, a whitespace-only lead
before a batch's calls (the Responses and Messages wires echo a model's
`\n\n` back as the round's content while the app records no segment for
it, and counting the two as different kept exactly those wires from ever
matching) and adjacent same-role plain-message boundaries. A
`UserPromptSubmit` hook that blocks the prompt (`docs/hooks.md`) is checked
before the retained prefix is consulted, so the block costs it nothing and
the next prompt still matches. Any mismatch uses the newly derived history,
so edits, rewinds and compaction cannot resurrect stale context. Retention
is capped at 8 MiB (images share their existing allocation); a backend
rebuild or a restart discards it and falls back on the record. MCP tool
definitions use stable name order so discovery order does not change a
prompt.

`tests/wire_history.rs` drives the Chat Completions and Responses wires end
to end against a loopback stand-in — a real turn, its events folded into the
`App` exactly as the loop folds them, the next turn's request read back off
the wire carrying the provider's own call id — and proves the durable half
on its own: a backend rebuilt between the turns, its retained request gone,
sends the batch the provider saw, and a prompt a hook blocks does not cost
the prefix. Its `#[ignore]`d live twin measures that half on a real cache
(`live_openrouter_a_rebuilt_backend_reads_the_tool_round_back_from_cache`,
OpenRouter's Anthropic routing): a real `bash` turn folded into the `App`,
then a fresh backend's first request reading 9,540 of its 9,561 input tokens
back — the tool round, under the provider's own `toolu_…` id, from the
record alone (measured 2026-09-21). Its Responses-wire twin
(`live_chatgpt_a_rebuilt_backend_reads_the_tool_round_back_from_cache`,
an 800-line `seq` result so the round spans several of OpenAI's 128-token
blocks) read 8,704 of 9,067 back on `codex-auto-review` — past the system
prompt and through the tool result, the model having emitted no blank
lead that run; the blank-lead shape stays the retained request's alone.

## Cache affinity: what each backend routes on

A cache hit needs the request to land where the cache *is*. The boundary
mints one key per process (`tui::config::session_cache_key` — pid + startup
time, the same key for every backend built this session) and each backend
gets it in the form it actually keys on:

- **`prompt_cache_key`** in the body — the standard OpenAI parameter,
  verified accepted by OpenRouter and Venice (which documents it as its
  cache-affinity hint), and sent on the Responses wire too.
- **`session_id`** in the body — OpenRouter's own session pin, sent **only
  when the chat `api_base` is OpenRouter's** (the `openrouter` marker in the
  base URL): OpenRouter routes one model across several upstream providers,
  and a cache written on Amazon Bedrock is unreadable from Google Vertex.
  Venice rejects unknown body keys (`Unrecognized key(s): 'session_id'` —
  verified live), so the pin stays gated.
- **`session_id` + `conversation_id` as headers** — the ChatGPT backend
  (`chatgpt.com/backend-api/codex`, `docs/chatgpt.md`). This one **ignores
  the body key**: measured live on an identical 6.7k-token prefix, the
  request as the app used to send it read **0** tokens from cache on turn 2
  (with a 12 s pause, to rule out a slow write), while the same request
  carrying the Codex CLI's two identity headers read **6400** — and
  `OpenAI-Beta: responses=experimental` on its own changed nothing. So
  `chatgpt::session_headers` puts the session key under both names and
  `openai::chatgpt_request_headers` attaches them, gated on the
  `ChatGptCodex` auth scheme exactly as Copilot's per-request headers are
  gated on its own — a pasted-key provider never sees a header it did not
  ask for.
- **Nothing** — GitHub Copilot. Its proxy answers an unrecognised body shape
  with `model_not_supported`, and its caching is server-side; the key rides
  `X-Request-Id` for support's sake only (`docs/copilot.md`).

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
App::apply_usage: accumulate + SNAP status.tokens            (app/status.rs, pure)
        ↓
TurnSummary { tokens, cached, cache_write }
        → "Done for 12s · 8.2k tokens (8k cached · 1.2k written)"
                                                             (ui::summary_lines; persisted
                                                              by session.rs, serde-defaulted
                                                              so old rollouts still parse)
```

- [`TokenUsage`](../src/stream/event.rs) carries `input`, `output`, `cached`
  (reads, a subset of `input` per the OpenAI accounting shape), `cache_write`
  and `reasoning`. Each wire's parser lands in the same struct:
  - **Chat Completions** (`openai::parse_sse_usage`) normalizes the spelling
    zoo: `prompt_tokens_details.cached_tokens` (OpenRouter/OpenAI/Venice), the
    top-level `cache_read_input_tokens` / `cache_creation_input_tokens`
    aliases (Anthropic-style shims), OpenRouter's `cache_write_tokens` and
    Venice's nested `cache_creation_input_tokens`. A `"usage": null` delta
    frame or an all-zero block is not a report.
  - **Responses** (`responses::parse_usage`): `input_tokens` /
    `output_tokens` with `input_tokens_details.cached_tokens` and
    `output_tokens_details.reasoning_tokens`.
  - **Messages** (`anthropic::MessageAccumulator::merge_usage`): Anthropic
    reports `input_tokens` as the *uncached remainder*, so `input` is the sum
    of that and both cache figures; the counters land on `message_start` and
    are **repeated cumulatively on `message_delta`** (verified live:
    `{"input_tokens":10,"cache_creation_input_tokens":7998,"cache_read_input_tokens":0,"output_tokens":5}`),
    and the merge is **monotonic per counter** — a later frame can only report
    more, and one that omits a counter leaves it — so a delta naming the
    remainder without the cache keys (the older documented shape) can never
    zero the cached share or shrink the whole-prompt `input` to a few dozen
    tokens.
  - **Ollama** (`ollama.rs`): the final frame's `prompt_eval_count` /
    `eval_count`, the whole prompt on a cache hit too; the API has no cache
    accounting at all, so the cached share is honestly zero.
- **Snap, don't add**: the tiktoken estimate keeps ticking live between
  frames (chunks, reasoning, tool output — unchanged), but each round's usage
  frame **replaces** the tally with the accumulated real total
  (`App::apply_usage`). So the status is always "everything billed so far,
  plus the current round's live estimate", and the final number is the
  provider's own accounting. The tally can jump *up* at a round boundary —
  that's the estimate learning about the system prompt and re-sent context it
  never counted; the truth, not a glitch.
- The receipt's parenthetical shows the two cache halves the turn's frames
  summed — `(8k cached)`, `(8k written)`, or `(24k cached · 2k written)` on an
  agentic turn that read the old prefix and wrote the new — a zero half
  omitted, and no parenthetical when neither applies. The written share is
  what tells the *first* turn's story: an explicit-caching provider bills the
  write at 1.25×, and a receipt reading just `8.2k tokens` looked like caching
  had done nothing.
- The tally reaches six digits on long agentic turns, so the status line
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
rules key off the model id, the affinity key off the api_base and the auth
scheme; adding a new provider block needs no caching knowledge.

## Live verification

`tests/live_caching.rs` — one `#[ignore]`d test per provider, each building
its config through `ProvidersFile::model_config` exactly as the boundary does
and driving two identical turns over a salted ~5k-token system prompt (past
every provider's minimum cacheable size), printing both usage frames:

```sh
A0_VENICE_API_KEY=sk-a0-…    cargo test --test live_caching -- --ignored --nocapture live_venice
VENICE_API_KEY=…             cargo test --test live_caching -- --ignored --nocapture live_venice_direct
OPENROUTER_API_KEY=sk-or-…   cargo test --test live_caching -- --ignored --nocapture live_openrouter
ANTHROPIC_API_KEY=sk-ant-…   cargo test --test live_caching -- --ignored --nocapture live_anthropic_api_key
OLLAMA_API_KEY=…             cargo test --test live_caching -- --ignored --nocapture live_ollama_cloud
GITHUB_COPILOT_TOKEN=ghu_…   cargo test --test live_caching -- --ignored --nocapture live_copilot
CHATGPT_CODEX_REFRESH_TOKEN=rt.1.… ALTER_ZERO_LIVE_TOKEN_STORE=/tmp/live.env \
                             cargo test --test live_caching -- --ignored --nocapture live_chatgpt
ANTHROPIC_CONSOLE_REFRESH_TOKEN=… ALTER_ZERO_LIVE_TOKEN_STORE=/tmp/live.env \
                             cargo test --test live_caching -- --ignored --nocapture live_anthropic_console
```

The two sign-ins whose refresh token **rotates** refuse to run without
`ALTER_ZERO_LIVE_TOKEN_STORE`: the mint may retire the token it was given, and
the rotated one is written back only to a store path — after such a run the
value under that file's `*_REFRESH_TOKEN` key is the one to keep. The older
`tests/live_openrouter.rs` cache tests (`live_usage_frame_reaches_the_app`,
`live_anthropic_prompt_cache_writes_then_reads`,
`live_qwen_accepts_cache_breakpoints_on_a_tool_round`,
`live_venice_reports_usage_and_hits_its_cache`) still run too.

Observed on the real wire (2026-09-06), each turn one request:

| provider · model | turn 1 | turn 2 |
| --- | --- | --- |
| Venice (a0 proxy) · `openai-gpt-4o-mini-2024-07-18` | `input: 6749, cached: 0` | `cached: 6656` |
| Venice · `claude-sonnet-4-6`, **no markers of ours** | `cache_write: 7998` | `cached: 7998` |
| OpenRouter · `~anthropic/claude-haiku-latest` (→ `claude-haiku-4.5` on Bedrock) | `cache_write: 8004` | `cached: 8004` |
| Anthropic · `claude-haiku-4-5`, pasted key on `x-api-key` | `cache_write: 8004` | `cached: 8004` (`input: 8007`, the whole prompt) |
| ChatGPT · `gpt-5.4-mini`, Responses wire | `cached: 0` | `cached: 6400` with the session headers — `0` without them |
| Ollama Cloud · `gpt-oss:20b` | `input: 6813` | `input: 6813`, no cache fields on the wire |
| OpenRouter · `qwen/qwen3.6-flash`, breakpoint on a tool result | the round completes | — |

Two rows could not be observed from the verification sandbox and are
**unverified** here: GitHub Copilot (the sandbox's GitHub proxy allows only
repository-scoped endpoints, so the token exchange at
`api.github.com/copilot_internal/v2/token` was refused before any request
reached Copilot) and the Anthropic Console sign-in (the refresh token on hand
had already been retired — `/login` mints a fresh one; the wire it then
speaks is the pasted key's, verified above, plus the OAuth beta header).

One OpenRouter quirk worth knowing: it reserves the model's **whole output
ceiling** against the account's credits before running a request, and a
balance that cannot cover it is a 402 naming the number (`You requested up to
64000 tokens, but can only afford 41732`). The app sends no `max_tokens`, so a
nearly-empty OpenRouter balance can refuse a big-output model outright; the
live test sets `max_tokens: 32` through the config's `extra_body` for its
one-word answers.

## Stress audit (2026-09-20)

The deterministic suite exercises 32 simultaneous authorization callers per
subscription provider, 64 successive refresh rotations, 32 concurrent
credential-store writers, 64 human turns with 16 parallel tool calls each,
and breakpoint placement across 32 rounds at batch sizes 1, 19, 20, 21 and 64.
It also covers large text/image followups, repeated marker preparation,
hook-message merging, a whitespace-only lead before a batch's calls,
interrupted-turn ID collisions, changed history, retained-memory limits, a
bearer whose `exp` the local clock reads as past, and a symlinked key store.
`tests/wire_history.rs` adds the end-to-end leg: a real backend turn on the
Chat Completions wire and on the Responses wire against a loopback stand-in,
the next turn's request carrying the provider's own call id — from the
retained request and, with the backend rebuilt between the turns, from the
record alone — and a prompt a hook blocks costing no prefix. The auth
half additionally covers a `forget` landing while a refresh is in flight
and a rotation racing a newer sign-in for the key store. The complete
local gate passed: 4,037 tests, formatting, Clippy with warnings
denied and the documentation build.

Live tests used the production provider configurations and synthetic prompts.
The identical-request baselines: OpenRouter read 8,004 of 8,007 input tokens,
ChatGPT's `codex-auto-review` read 5,888 of 6,748, and Venice read 6,528 of
6,749 on one afternoon and nothing at all on another — back to back or eight
seconds apart — which is the provider's cache and not the request, the
request being byte-identical both times.

The `live_*_growing_conversation_keeps_cache` tests send six growing turns
and print every usage frame. What each asserts follows the provider's cache
(`CacheReads` in the test): explicit breakpoints are deterministic, so every
warm round must read its prefix back; an implicit cache is best-effort and
asynchronous, so a run in which no warm round reads is checked against an
identical re-send of its last request — a provider that reads *that* back
but none of the growing rounds is a prefix the request moved, one that reads
neither is not reading today. They are ignored by default, since provider
availability and cache placement are outside the local test's control.
Observed:

| Provider/model | Five warm rounds | Result |
| --- | --- | --- |
| OpenRouter / `~anthropic/claude-haiku-latest` | 40,250 / 40,360 input tokens reused (99.7%), every round | Passed, deterministic |
| ChatGPT / `codex-auto-review` | 5 of 5 read 5,888 tokens (86.3%); an earlier run 4 of 5, one round reading nothing | Passed, best-effort |
| Venice proxy / `openai-gpt-4o-mini-2024-07-18` | 1 of 5 read (6,656 tokens) in two runs, 0 of 5 in two more — one with an eight-second pause between rounds — with the identical-request baseline missing alongside | The provider, not the request |

The flat 5,888 is OpenAI's accounting, not a stalled prefix: it counts whole
128-token blocks, and a round adds fewer tokens than one block. And
`live_openrouter_marks_the_previous_tool_result_and_reads_it_back` sends the
new breakpoint ③ on a tool result in the middle of the conversation: accepted
by OpenRouter's Anthropic routing, 10,151 tokens written on the tool round and
read back whole on the next turn. No fabricated cache count or production
delay was added to conceal any of these results; a provider hit on an
identical request does not prove reliable hits throughout a growing session,
and a miss on one does not indict the request.

## Known limitations

- The per-model **minimum cacheable sizes** and TTLs (5 min – 1 h) are the
  provider's; a short session below the threshold caches nothing, silently.
- `needs_cache_breakpoints` is a **prefix list** (`anthropic/`, `qwen/`, with
  or without OpenRouter's `~` alias marker), not a capability probe. The
  listings do carry a signal — OpenRouter's `pricing.input_cache_write`,
  Venice's `pricing.cache_write` — but neither says whether the *markers*
  are required, so the list stays; a future explicit-caching vendor means one
  more prefix (and a provider that *ignores* `cache_control` merely ignores
  it). Gemini 2.5+ is left implicit on purpose: its automatic caching costs
  no write, where an explicit `cache_control` would bill one.
- **GitHub Copilot** is unverified from here (above). Its proxy may want its
  own `copilot_cache_control` marker for the Claude models it serves; without
  a reachable exchange that stays a hypothesis, not a change.
- **Ollama** reports no cache accounting, so its receipt never shows a cached
  share even when the server reused its KV cache.
- The **status line** shows the snapped total but not the cached share; the
  cache halves surface in the committed `Done …` summary (and the Ctrl+O
  transcript, where summaries re-render). Keeping the live line's clause set
  stable avoids re-teaching `docs/status-indicator.md`'s geometry for a
  number that only settles at round boundaries anyway.
- OpenRouter's per-request `cost` field is parsed past, not surfaced — the
  tally stays in tokens (Venice bills in a different unit entirely).
- The **durable prefix** replays what the records can say, and two round
  shapes still differ from the wire once the retained request is gone (a
  rebuild, a `/resume`, a restart — the running backend's retained request
  covers both until then, and the cost is one re-read of the conversation
  from that round on, never a malformed request): a task or agent call the
  **Max tool calls** ceiling refused leaves no record at all (it never had
  a cell), and a launch a `PreToolUse` hook blocked resolves as a lone
  cell under a synthetic id ahead of the batch; and several image reads in
  one round replay as one merged user message where the live loop sent one
  per read.
