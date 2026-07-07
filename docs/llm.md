# The real LLM backend and the `/model` picker

The app ships with a canned [`DummyAi`](../src/stream.rs) so it runs offline and
`smoke.sh` stays deterministic. This document covers the **real** backend that
streams from any OpenAI-compatible endpoint, the flag that toggles the dummy back
on, and the inline **`/model`** picker that lists and switches models.

## The seam (unchanged)

Nothing about the event loop changes: the backend is still a
[`stream::ReplySource`](../src/stream.rs) whose `spawn(prompt, images, tx, cancel)`
runs on a plain OS thread that *only sends* `StreamEvent`s and polls the
`CancelToken`. The only structural change is that `main.rs` now holds the backend
as a **`Box<dyn ReplySource>`** instead of a concrete `DummyAi`, so `/model` can
swap it at runtime (`start_turn`/`flush_next_queued` take `&dyn ReplySource`).

## The `llm` module (the I/O boundary for a real model)

`src/llm/` is the only place that talks HTTP. Its pure cores are unit-tested; the
network calls are boundary code (like `main.rs`/`term.rs`), verified by hand.

| file | role | pure? |
| --- | --- | --- |
| `llm/mod.rs` | `ChatMessage`, module glue, `LlmError`/`Result` | mostly |
| `llm/config.rs` | `providers.toml` → `Provider`/`ProvidersConfig`, `ModelConfig`, key/model resolution | **pure** |
| `llm/keystore.rs` | `EnvFile` — the `.env` reader/writer the `/login` flow persists keys through | **pure** |
| `llm/settings.rs` | `Settings` — the `config.json` reader/writer persisting the `/model` selection across runs | **pure** |
| `llm/thinking.rs` | `ThinkingSplitter` — peels `<think>`/`<reasoning>` tags (and native `reasoning` deltas) out of the stream | **pure** |
| `llm/openai.rs` | `OpenAiClient` — endpoint/payload build (pure) + the blocking SSE stream (boundary) | split |
| `llm/models.rs` | `/v1/models` response → `Vec<ModelEntry>` (parse pure; fetch boundary) | split |
| `llm/backend.rs` | `LlmBackend: ReplySource` — bridges the SSE deltas to `StreamEvent`s | boundary |

### Why blocking `reqwest` and a hand-rolled SSE reader

`ReplySource::spawn` hands the loop a `std::thread::JoinHandle`, exactly like
`DummyAi`. A blocking `reqwest::Response` is a `std::io::Read`, so the backend
drains it line-by-line for `data:` frames while polling the `CancelToken` between
lines — the same cooperative-cancellation shape as the dummy's `nap`, with no
nested tokio runtime. `reqwest` honours `HTTPS_PROXY` from the environment; the
agent proxy's custom CA is loaded at runtime from `SSL_CERT_FILE` (or
`INLINE_TUI_CA_FILE`) and added as an extra trust root, so `rustls` keeps the
build free of system OpenSSL.

### Reasoning (`ThinkingSplitter`)

Some providers stream reasoning as a native `reasoning`/`reasoning_content` delta;
others inline it as `<think>…</think>` in the content. `ThinkingSplitter::feed`
normalises both to a `(response_delta, reasoning_delta)` pair. The backend maps a
non-empty reasoning delta onto a `ThinkingStart` (once) + `ThinkingChunk(text)`
stream + `ThinkingEnd` (when the response text resumes), driving the existing
`Thinking for Ns` status.

## Configuration and the dummy toggle

`providers.toml` (repo root by default) declares the providers; see the file's own
comments for the block shape. Resolution order for the file: `INLINE_TUI_PROVIDERS_FILE`
→ `./providers.toml` → `~/.inline-tui/providers.toml` → a built-in default with the
two shipped providers (`a0_venice` — the Agent Zero/Venice proxy — and `openrouter`).

The active backend is chosen at startup — from env, then the **persisted
selection** (`~/.inline-tui/config.json`, written by `/model`), then the file's
default — and can be switched live by `/model`:

| env var | meaning | default |
| --- | --- | --- |
| `INLINE_TUI_DUMMY` | truthy (`1`/`true`/`yes`) forces the dummy backend | unset |
| `INLINE_TUI_PROVIDER` | active provider id | saved selection, then first in the file |
| `INLINE_TUI_MODEL` | active model id | saved selection, then unset → dummy |
| `INLINE_TUI_API_KEY` | generic API key (fallback) | unset |
| `<PROVIDER>_API_KEY` | per-provider key, e.g. `OPENROUTER_API_KEY` | unset |
| `INLINE_TUI_CONFIG_DIR` | the config home (holds `.env` + `config.json`) | `~/.inline-tui` |
| `INLINE_TUI_ENV_FILE` | the `.env` key store `/login` reads and writes | `{config_home}/.env` |
| `INLINE_TUI_TEMPERATURE` | sampling temperature | provider/omit |
| `SSL_CERT_FILE` / `INLINE_TUI_CA_FILE` | extra CA bundle for the proxy | unset |

**The dummy is the fallback, never a surprise.** The real backend activates only
when `INLINE_TUI_DUMMY` is unset **and** a provider, a model, and an API key all
resolve. Otherwise the app uses `DummyAi`. `smoke.sh` sets none of these, so it
always gets the dummy — the canned replies, the `dummy_model_name` footer, and the
scripted tool calls its assertions depend on are untouched.

### The config home: `~/.inline-tui`

The app keeps its per-user state under a **config home** — `INLINE_TUI_CONFIG_DIR`,
else `~/.inline-tui` (the same base `providers.toml` and the sessions dir already
use), else `None` when there's no `HOME` (persistence then disabled, falling back
to `./.env`). It holds two files, both written best-effort (a failure is swallowed
so it can never kill the TUI) and created on first write:

- **`.env`** — the API-key store the `/login` flow writes (`main.rs::env_file_path`,
  overridable with `INLINE_TUI_ENV_FILE`). Git-ignored so keys are never committed.
- **`config.json`** — the last `/model` selection (`{ "provider", "model" }`), so
  the choice is the default next run (`llm::settings::Settings`).

### Where a key comes from: `.env` persistence

Because `std::env::set_var` is `unsafe` (this crate `forbid`s unsafe), the app
never mutates the process environment. Instead the `.env` file is loaded once at
startup into an in-memory `llm::keystore::EnvFile` map. Key resolution
(`main.rs::resolve_api_key`) checks the **real process env first** (dotenv
precedence — an exported `OPENROUTER_API_KEY` still wins) and falls back to the
`.env` map, so a key set either way is found.

`EnvFile` is a small **pure, tested** `.env` reader/writer: `parse` reads
`KEY=VALUE` pairs (comments, blanks, `export ` prefix, and quotes handled), and
`upsert` rewrites one key **in place** — every other line, comments and all,
preserved — which is what the `/login` flow writes back.

### Persisting the model: `config.json`

`llm::settings::Settings` is the pure, tested `config.json` reader/writer
(`{ "provider", "model" }`, all fields optional so an old or partial file still
loads). At startup the saved provider/model seed the active selection (env vars
still win); on a successful `/model` switch the boundary writes the new choice
back (`main.rs::save_settings`). So a model picked once is the default on every
later run — and if its key still resolves, the real backend activates
automatically at startup.

## The inline `/model` picker

Unlike `/resume` (a full-screen alt-screen overlay), `/model` is **inline**: it
grows the bottom live region in place, replacing the composer with its own search
prompt — the shape the user asked for. It is `App::model_picker: Option<ModelPicker>`
(not a `View`), owned in `on_key` like the Ctrl+R search, and special-cased in
`ui::render_live` / `ui::live_height` / `ui::cursor_position`.

Layout (headerless — top rule, a blank, the airy codex-style body, a blank, then
the bottom rule — the shape of the user's mock):

```
────────────────────────────────────────────────

  ❯ cla                                                  (search prompt, cyan '❯')

→ anthropic/claude-3-haiku    [openrouter]               (selected: '→', active: ✓)
  anthropic/claude-3.5-haiku  [openrouter]
  anthropic/claude-fable-5    [openrouter] ✓
  (3/12)                                                 (position/total, dim)

  Model Name: Anthropic: Claude 3.5 Haiku                (friendly name, dim)

────────────────────────────────────────────────
```

- Opened by `/model` from an idle composer (`Action::OpenModelPicker`; rejected
  mid-turn with a red notice, like `/resume`).
- **A configured provider is required.** If no provider has a resolvable key, the
  boundary skips the fetch and the list area shows a cyan
  `No API key yet — run /login to add one` (`ModelLoad::NeedsLogin`) instead of
  offering models the user can't use.
- Otherwise the boundary spawns a worker thread that fetches `/models` from a
  provider whose key resolves — the active one if it's configured, else the first
  configured one — parses it (`llm::models`), and sends the result on a dedicated
  channel (the image-paste worker pattern). The picker shows `Loading models…`
  until it arrives, then the list (or a red error row).
- Type to filter (case-insensitive substring over `id` and provider), `↑/↓`/PgUp/
  PgDn/Home/End move, `Enter` selects → `Action::SelectModel { provider, id }`,
  `Esc` clears the query then closes, `Ctrl+C` closes.
- On select, the loop rebuilds the backend for the new provider/model, updates the
  footer (`App::set_session_info`), and **persists the choice to `config.json`**
  (so it's the default next run), then collapses the picker.

- The picker is **headerless** — the old "Showing models…" banner was dropped, so
  the `❯` search line sits directly under the top rule; the selection marker is `→`.
- The counter + `Model Name:` rows show **only when a real model is highlighted**.
  A **placeholder** state (`Loading models…`, a red error, the `/login` hint, or
  `No matching models`) has no counter or name, so those rows collapse to a single
  gap above the bottom rule — the box hugs the placeholder instead of leaving four
  blank rows (`model_has_detail`/`model_chrome_rows`; `MODEL_CHROME_ROWS` 9 →
  `MODEL_CHROME_ROWS_COLLAPSED` 6).

### Styling

All picker styling is centralized in `ui.rs`'s `MODEL_*` consts: the cyan `❯`
prompt and selection accent (reusing `MENU_SELECTED_COLOR`), the dim provider tag /
counter / model-name, the `→` marker, the `✓` active mark, the cyan
`MODEL_LOGIN_HINT`, and `MODEL_MENU_MAX_ROWS`. Retheme there.

## The inline `/login` onboarding flow

`/login` collects a provider API key and saves it to `.env` so it persists across
runs — the "onboarding" the user asked for. Like `/model` it is **inline**
(`App::key_onboarding: Option<KeyOnboarding>`, not a `View`), replacing the composer
in place; unlike it, it is a **two-step** flow.

**Step 1 — pick a provider** (a filterable list, headerless like `/model`;
`KeyStep::Provider`):

```
────────────────────────────────────────────────

  ❯ agent                                                (filter, cyan '❯')

→ Agent Zero API   [A0_VENICE_API_KEY] ✓                 (selected '→'; ✓ = configured)
  OpenRouter       [OPENROUTER_API_KEY]
  (1/2)                                                  (position/total, dim)

  Keys are saved to ~/.inline-tui/.env                   (dim hint — the real path)

────────────────────────────────────────────────
```

**Step 2 — enter the key** (masked; `KeyStep::Key`):

```
────────────────────────────────────────────────

  Enter your Agent Zero API key                          (periwinkle #96a0d5, names the provider)

  ❯ ••••••••••••••••••••                                 (masked field, cyan '❯')

  Enter to save · Esc to go back                         (dim hint)

────────────────────────────────────────────────
```

- Opened by `/login` from an idle composer (`Action::OpenKeyOnboarding`; rejected
  mid-turn with a red notice, like `/model`). The boundary builds the provider
  rows so the ✓ reflects real env / `.env` key resolution (`provider_choices`) and
  injects the `~`-relative `.env` path the provider-step hint names.
- **Provider step**: type-to-filter (id/name substring), `↑/↓`/PgUp/PgDn/Home/End
  move, `Enter` advances to key entry for the highlighted provider (its index is
  pinned so the filter can't reorder it out from under you), `Esc` clears the
  filter then closes, `Ctrl+C` closes.
- **Key step**: printable keys and Backspace edit the key, a **bracketed paste**
  (`App::paste_into_key_onboarding`) appends it with whitespace/newlines stripped
  (API keys are always pasted), `Enter` saves a non-empty key
  (`Action::SaveApiKey { provider, env_var, key }`) and closes, `Esc` steps *back*
  to the provider list, `Ctrl+C` closes. The field is masked to `•` glyphs — the
  plaintext key never touches the screen. The prompt names the provider, avoiding
  a doubled "API" when its name already ends in it (`login_key_prompt`).
- On save, the loop `EnvFile::upsert`s the key into `~/.inline-tui/.env` (creating
  the config home first), refreshes its in-memory copy (so the next `/model`
  fetch/switch resolves it immediately), and commits a
  `Saved {ENV} to {path} — run /model to use {provider}` system notice. It does
  **not** auto-switch the model: the user picks one with `/model`, which now finds
  the key.

### Styling

The `/login` flow reuses the `/model` picker's colours (indent, cyan `❯` prompt /
selection, dim meta, green ✓, `→` marker) plus the `LOGIN_*` strings and geometry
consts in `ui.rs` — including the periwinkle `LOGIN_KEY_PROMPT_COLOR` (`#96a0d5`)
that names the provider on the key step. Retheme there.

## Known limitations (v1)

- Token counts remain an app-side estimate; the OpenAI streaming protocol's final
  `usage` block is not surfaced through `StreamEvent` (unchanged from the dummy).
- Switching models mid-session does not rewrite the already-recorded `/resume`
  session-meta `model` field (it names the model the file was started with).
- No streaming tool-call support from the model yet — assistant text and reasoning
  stream; a real tool-calling loop is future work. The `!` local shell and the
  dummy's scripted tools are unaffected.
- The picker fetches models when opened (no cache); a slow provider shows
  `Loading models…` until the response lands.
- **Interrupt latency during a network stall.** The SSE drain runs on a blocking
  thread that the event loop `join()`s on interrupt/quit. Blocking `reqwest`
  applies its `timeout` per read, so the thread wakes every `STREAM_OP_TIMEOUT`
  (3s) to poll the `CancelToken` — meaning Esc/quit reaps within ~3s *even if the
  connection stalls with no bytes arriving*. During normal streaming (bytes
  flowing) a read returns immediately, so interrupt is effectively instant; the
  3s cap only bites while genuinely waiting on a silent socket. The same 3s also
  bounds the initial send/header exchange (blocking `reqwest` couples the two),
  so it's kept above a normal connect-plus-headers latency rather than tuned as
  low as possible.
- The `ThinkingSplitter`'s inline-tag path and a provider's *native* `reasoning`
  field are handled independently; a single completion that mixed inline
  `<think>` tags **and** native reasoning deltas could misorder a buffered tag
  fragment. No real OpenAI-compatible provider does both in one response, so
  there's no realistic trigger; `flush()` still guarantees the text is never
  lost, only (in that impossible case) reordered.
