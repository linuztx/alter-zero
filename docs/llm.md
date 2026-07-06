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
three shipped providers.

The active backend is chosen at startup by `llm::config::resolve` and can be
switched live by `/model`:

| env var | meaning | default |
| --- | --- | --- |
| `INLINE_TUI_DUMMY` | truthy (`1`/`true`/`yes`) forces the dummy backend | unset |
| `INLINE_TUI_PROVIDER` | active provider id | first in the file |
| `INLINE_TUI_MODEL` | active model id | `INLINE_TUI_MODEL` or unset → dummy |
| `INLINE_TUI_API_KEY` | generic API key (fallback) | unset |
| `<PROVIDER>_API_KEY` | per-provider key, e.g. `OPENROUTER_API_KEY` | unset |
| `INLINE_TUI_TEMPERATURE` | sampling temperature | provider/omit |
| `SSL_CERT_FILE` / `INLINE_TUI_CA_FILE` | extra CA bundle for the proxy | unset |

**The dummy is the fallback, never a surprise.** The real backend activates only
when `INLINE_TUI_DUMMY` is unset **and** a provider, a model, and an API key all
resolve. Otherwise the app uses `DummyAi`. `smoke.sh` sets none of these, so it
always gets the dummy — the canned replies, the `dummy_model_name` footer, and the
scripted tool calls its assertions depend on are untouched.

## The inline `/model` picker

Unlike `/resume` (a full-screen alt-screen overlay), `/model` is **inline**: it
grows the bottom live region in place, replacing the composer with its own search
prompt — the shape the user asked for. It is `App::model_picker: Option<ModelPicker>`
(not a `View`), owned in `on_key` like the Ctrl+R search, and special-cased in
`ui::render_live` / `ui::live_height` / `ui::cursor_position`.

Layout (top rule, then the airy codex-style body, then bottom rule):

```
────────────────────────────────────────────────
  Only showing models from configured providers.        (header, gold)
  > cla                                                  (search prompt, cyan '>')
  → anthropic/claude-3-haiku    [openrouter]             (selected: '→', active: ✓)
    anthropic/claude-3.5-haiku  [openrouter]
    anthropic/claude-fable-5    [openrouter] ✓
  (3/12)                                                 (position/total, dim)
  Model Name: Anthropic: Claude 3.5 Haiku                (friendly name, dim)
────────────────────────────────────────────────
```

- Opened by `/model` from an idle composer (`Action::OpenModelPicker`; rejected
  mid-turn with a red notice, like `/resume`).
- The boundary spawns a worker thread that fetches `/models` from the configured
  provider(s), parses it (`llm::models`), and sends `ModelListResult` on a
  dedicated channel (the image-paste worker pattern). The picker shows
  `Loading models…` until it arrives, then the list (or a red error row).
- Type to filter (case-insensitive substring over `id` and provider), `↑/↓`/PgUp/
  PgDn/Home/End move, `Enter` selects → `Action::SelectModel { provider, id }`,
  `Esc` clears the query then closes, `Ctrl+C` closes.
- On select, the loop rebuilds the backend for the new provider/model, updates the
  footer (`App::set_session_info`), and collapses the picker.

### Styling

All picker styling is centralized in `ui.rs`'s `MODEL_*` consts (the section next
to `RESUME_*`): the gold header, the cyan `>` prompt and selection accent (reusing
`MENU_SELECTED_COLOR`), the dim provider tag / counter / model-name, the `→`
marker, the `✓` active mark, and `MODEL_MENU_MAX_ROWS`. Retheme there.

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
