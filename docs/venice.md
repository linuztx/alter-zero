# Venice as a provider

Venice.ai's models — private, uncensored, open-weight and frontier alike —
are reachable two ways, over one wire (Chat Completions, the default
`wire_api`):

| provider id | chats at | lists from | credential |
| --- | --- | --- | --- |
| `a0_venice` (**Agent Zero API**) | `https://api.agent-zero.ai/venice/v1` — Agent Zero's proxy in front of Venice | `https://api.venice.ai/api/v1` — Venice itself, which the `sk-a0-…` key is also valid against | an Agent Zero key (`A0_VENICE_API_KEY`), with a free daily quota for A0T token holders |
| `venice` (**Venice**) | `https://api.venice.ai/api/v1` — Venice itself | the same base: it chats where it lists | a key from your own Venice account (`VENICE_API_KEY`, made at [venice.ai/settings/api](https://venice.ai/settings/api)) |

The second provider exists because the two keys are not interchangeable:
the proxy takes Agent Zero's key, and a Venice account holder has a Venice
key the proxy does not accept. What the two providers *share* is everything
the request builder keys on — which is why the direct one is a
`providers.toml` block and nothing else. The Agent Zero row was the app's
first provider; this document is where the pair's shared shape is written
down, since the proxy's own behaviour had only ever been described from the
side of the feature that met it (`docs/reasoning.md`,
`docs/prompt-caching.md`, `docs/llm.md`).

## What a provider file says

```toml
[providers.venice]
name = "Venice"
description = "Venice.ai's private, uncensored models straight from Venice's own API — …"
api_key_url = "https://venice.ai/settings/api"

[providers.venice.kwargs]
api_base = "https://api.venice.ai/api/v1"

[providers.venice.kwargs.venice_parameters]
include_venice_system_prompt = false
```

Nothing in the block is new to the file (`docs/llm.md` documents every key).
What makes it Venice is the `venice_parameters` table, and what makes it
*direct* is the base:

- **`api_base` is the listing base too.** The proxy needs a separate
  `api_model_base` because Agent Zero does not serve `/models`; the direct
  provider lists and chats at the same host, so the file names no second
  base and `Provider::models_base` falls back to the chat base.
- **`api_key_env` is the default**, `VENICE_API_KEY` — the id's own
  `<ID_UPPERCASE>_API_KEY`, which is also the variable Venice's own examples
  use — so `/login`'s save and an exported variable agree without the file
  saying so.
- **`venice_parameters.include_venice_system_prompt = false`**, as through the
  proxy: Venice prepends a system prompt of its own unless told not to, and
  the persona is the whole system prompt here.
- **No `disable_thinking` pinned.** The table's *presence* is the
  Venice-family marker the payload builder keys on: with a thinking mode
  active it syncs `disable_thinking` to `mode == Off`, the toggle Venice
  honours where `reasoning.enabled` is ignored (`docs/reasoning.md`).
  A test pins that the two providers' `extra_body()` are equal, so the
  toggle can never sync for one and not the other.
- **No `extra_headers`.** The proxy sends none either; there is no identity
  to declare to Venice beyond the key.

## One request, two destinations

`the_shipped_venice_provider_sends_the_proxys_request_to_venice_itself`
(`src/llm/openai.rs`) resolves both providers through the shipped file
exactly as the boundary does and asserts the two request bodies are
**byte-identical** — only `OpenAiClient::endpoint` differs. Everything below
therefore holds for both:

- **The thinking mode** rides as the `reasoning` object *and* the synced
  `venice_parameters.disable_thinking` (`docs/reasoning.md`). Venice
  documents both `reasoning_effort` and a `reasoning` object today; the
  `disable_thinking` sync is what live testing through the proxy showed
  actually stops a hybrid reasoner, so it stays.
- **Prompt caching is implicit** (`docs/prompt-caching.md`): no
  `cache_control` breakpoints of ours — Venice injects Anthropic's markers
  itself for its bare `claude-*` ids, and `cache::needs_cache_breakpoints`
  leaves those ids alone. The session's `prompt_cache_key` rides the body
  (Venice documents it as its cache-affinity hint), and OpenRouter's
  `session_id` does **not** — Venice rejects unknown body keys
  (`Unrecognized key(s)`), so that pin stays gated on an OpenRouter base.
- **`stream_options.include_usage`** is honoured; the final usage frame's
  cached detail is read off both spellings Venice uses
  (`prompt_tokens_details.cached_tokens` and the top-level
  `cache_read_input_tokens` / `cache_creation_input_tokens` aliases —
  `drain_stream_reads_the_venice_style_usage_aliases`).
- **The catalog** is `GET /models` on the base, the same call the proxy
  makes, and every capability the picker and the footer want comes off each
  record's `model_spec`: `availableContextTokens` as the context window,
  `capabilities.supportsVision`, and `capabilities.supportsReasoning` (+
  `supportsReasoningEffort` for the `low/medium/high` ladder; without it
  the model is an on/off-only reasoner) — `docs/reasoning.md`,
  `docs/tools.md` "Vision detection", `docs/compact.md`.

Because the proxy lists from Venice's own base, `/model` shows the **same
ids twice** once both keys are configured — `[a0_venice]` and `[venice]`
tagged — and the ✓ marks the active `(provider, id)` pair, so the row in use
is never ambiguous.

## `/login` and launching on it

`/login` → **Use an API key** → **Venice** → the key step introduces the
provider with the file's description and links the key page, and Enter saves
the key as `VENICE_API_KEY` in `~/.alter-zero/.env`. Venice issues two kinds
of key; an **Inference Only** key is all the app needs (the Admin kind exists
to manage keys programmatically). From the environment,
`ALTER_ZERO_PROVIDER=venice` with `VENICE_API_KEY` exported launches on it
directly (`ALTER_ZERO_MODEL` names the model, else `/model` picks one).

Every other Venice knob rides the same table: a user's own copy of
`providers.toml` (`~/.alter-zero/providers.toml`, or `ALTER_ZERO_PROVIDERS_FILE`)
can add `enable_web_search = "auto"`, `strip_thinking_response = true` or a
`character_slug` under `[providers.venice.kwargs.venice_parameters]`, and the
table round-trips into the body untouched — the mode's `disable_thinking`
sync preserves the other keys.

## Testing

- **Unit** — `the_builtin_file_ships_venice_direct_beside_the_agent_zero_proxy`
  and `a_venice_selection_resolves_to_venice_itself_and_needs_its_key`
  (`src/llm/config.rs`) pin the block's every field against the proxy's,
  and the payload twin test above pins the request.
- **Smoke** — Phase 103 (`scripts/smoke/phases/103-loginfork.sh`) walks
  `/login`'s API-key list and requires the **Venice** row beside **Agent Zero
  API**, both reading `◯ unconfigured` under the suite's scrubbed key store.
- **Live** — opt-in, never run by `cargo test`:

  ```sh
  VENICE_API_KEY=… cargo test --test live_caching -- --ignored --nocapture live_venice_direct
  ALTER_ZERO_LIVE_PROVIDER=venice VENICE_API_KEY=… ALTER_ZERO_LIVE_MODEL=llama-3.3-70b scripts/live_smoke.sh
  ```

  `live_venice_direct_second_turn_reads_the_prefix_from_cache` is the
  proxy's caching test pointed at the direct provider. **It has not been run
  from here** — no Venice key was available when the provider was added —
  so the direct base is verified by construction (the same request the proxy
  test proved on Venice's own cache, `docs/prompt-caching.md`'s table) and by
  Venice's published API reference, not by a turn of its own. The test is
  the first thing to run with a key.

## Known limitations

- The listing sends no `type` query, exactly as the proxy's does; Venice's
  `/models` also serves image, audio and embedding records behind that
  parameter, and if its default ever widened the picker would show them for
  both providers alike — the per-record filter (`models::is_callable`) is
  where a `type` check would go.
- Venice bills API usage from the account's own balance; the app reads no
  balance and no quota, so an exhausted account surfaces as the request's
  HTTP error in the turn's red notice, like any provider's.
