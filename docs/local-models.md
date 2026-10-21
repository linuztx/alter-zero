# Local models — `/login` → **Use a local model**

`/login` has three ways in. **Use a subscription** signs in to an account,
**Use an API key** pastes a provider's key, and **Use a local model**
connects a model server the user runs: Ollama, LM Studio, or a keyless server
described in their own `providers.toml`. A local server needs no key. What it
needs is an **address**, and the page asking for it **tests the connection
before it saves anything**.

```
/login → Use a local model → LM Studio → Enter        (the address in force)
                                       → type gpu-box:1234, Enter
```

The page tests by listing the server's models exactly the way `/model` will
(`llm::models::fetch_models`). A pass closes the flow and says what was
found: `Connected to LM Studio — 3 models · run /model to pick one`. A
failure stays on the page, in red under the field, until the address is
edited. A server that wants a token asks for one on the same page.

## Which list a provider is in

The three lists partition the provider table by `auth`, the provider file's
own key for how a provider authenticates (`docs/llm.md`):

| `auth` | list | the row asks for |
| --- | --- | --- |
| `github_copilot`, `chatgpt_codex`, `anthropic_console`, `cline_account` | Use a subscription | a sign-in (`docs/copilot.md` and siblings) |
| `api_key` (the default) | Use an API key | a key, masked |
| `optional_key` | Use a local model | an address, tested |

`optional_key` already meant "a server that needs no credential, though a
stored one still rides as a bearer" (`docs/ollama.md`). That is what a local
server is, so the local list is exactly those providers. Before this, the
local Ollama sat in the API-key list as a "host field" that saved whatever was
typed. It now lives on the local list, and the host field became the server
page below. **Ollama Cloud** stays under API keys, because it is a hosted
service reached with a pasted key.

## The list

The same list page the other two use: a `❯` filter, rows in the injected
order (alphabetical by id, so `LM Studio` before `Ollama`), the `(n/total)`
counter, `↑↓ navigate  enter connect  esc back`. Each row says `✔
configured` or `◯ unconfigured`.

**Configured means pointed at** (`tui::config::is_configured`, the rule
`docs/ollama.md` set out for Ollama): the provider's address variable
resolves (`OLLAMA_HOST`, `LMSTUDIO_HOST`), or a key resolves, or
`ALTER_ZERO_PROVIDER` names it. A server a user's own block fixes in place,
with no address variable, is configured by that block alone
(`Provider::configured_by_definition`, `docs/user-providers.md`). Configured
is also which providers `/model` fetches from. A server counted configured
without being pointed at would paint `LM Studio unavailable` in red on every
`/model` open for everyone who does not run one.

## The server page

```
────────────────────────────────────────────────
  Enter your LM Studio server URL                    (title)

  Open models running in LM Studio on your own       (the file's description)
  machine — nothing leaves it, and no key is …
  Install it from https://lmstudio.ai/download       (api_key_url, an OSC 8 link)

  ❯ http://127.0.0.1:1234                            (dim: the address in force)

  Nothing is answering at 127.0.0.1:1234 — is        (red, after a failed test)
  LM Studio's server running?

  Enter to test and save · Esc to go back
────────────────────────────────────────────────
```

- **The field shows the address as typed.** A URL typed blind is a URL typed
  wrong, so nothing is masked. Empty, it shows the **address in force**, dim:
  the variable's value when one is set (as stored), else the file's own
  server. For LM Studio that is `http://127.0.0.1:1234`, the way LM Studio's
  server page shows it, without the `/v1` the config adds back
  (`llm::local::server_address`). An empty Enter tests that address, so
  re-checking a working server is one keystroke. This is also why an empty
  Enter no longer resets a remote `OLLAMA_HOST` to the default: the old host
  field's default was the file's address, whatever was stored.
- **Enter tests.** `Action::TestLocalServer { provider, test }` carries a
  `ServerTest { host, token }`. The page shows `Testing the connection…` under
  the field and `Esc to stop the test` as its hint. The field takes no keys
  while it waits, and the caret hides. The boundary builds the provider's
  config with the tested address in place of its variable
  (`ModelSession::local_test_config`), completes it with the same grammar
  every request uses, and runs `fetch_models` on the `/login` worker
  (`tui::workers::spawn_server_test`). An address that is no URL at all is
  answered at once, with no request made and the server's own address as
  the example (`llm::local::server_label`, `unusable_address`).
- **The verdict** is pure (`llm::local::verdict`), worded for the page:

  | the listing… | the page says |
  | --- | --- |
  | answered with N models | *(passes: saves, closes, toasts `Connected to {name} — N models …`)* |
  | could not connect: refused | `Nothing is answering at {host} — is {name}'s server running?` |
  | could not connect: the name did not resolve | `Could not find {host} — check the address.` |
  | could not connect: the certificate is untrusted | `{host}'s certificate is not trusted — it may be self-signed. Point ALTER_ZERO_CA_FILE at its CA certificate, then test again.` |
  | could not connect: an https request to a plain-http server | `{host} does not speak https — try http:// instead.` |
  | timed out | `{host} did not answer in time — check the address, and that nothing blocks the port.` |
  | answered 401 or 403 | *(asks for a token: the next section)* |
  | answered 404, or a body that is not a listing | `{host} answered, but not as {name} — check the address and port.` |
  | answered any other status | `{name} at {host} answered HTTP {status}: {body}` |

  `{host}` is `host[:port]`, the port shown only when the URL names one. A
  server with nothing to chat with still passes. The address is right; the
  toast says `Connected to {name}, but it has no models to chat with yet`.
- **An answer to a test the page moved on from is dropped.** The page keeps
  the test in flight, and `App::finish_server_test` applies a verdict only
  when it answers that test. Esc stops a test (`Action::CancelServerTest`
  cancels the worker, which then sends nothing). Even a verdict that slipped
  past the cancel cannot land on the next test, or close a page the user has
  left.
- **Esc** stops a running test, else steps back to the list with the row just
  left still highlighted. **Ctrl+C** closes the flow from anywhere, stopping a
  running test with it.
- **Editing clears a failure.** A red reason is about the address that was
  tested. The first keystroke after it removes it rather than leaving it to
  describe an address no longer in the field.

### The token step

LM Studio 0.4+ can be set to *Require Authentication* (an API token sent as
`Authorization: Bearer …`), and an Ollama behind a reverse proxy may want a
bearer too. A test answered **401 or 403** means the server is there and is
refusing an unauthenticated request. That is not a failure, so the page asks:

```
  Enter your LM Studio API token

  LM Studio at gpu-box:1234 wants an API token.

  ❯ ••••••••
```

The token is masked like a key. Enter tests it **at the address the server
asked from**. A token the server refuses again says
`LM Studio refused that token — check it, then test again.` and stays to be
fixed. Esc returns to the address. A pass saves both: the address under the
provider's `api_base_env`, the token under its `api_key_env`. A token already
stored (`LM_API_TOKEN` exported, say) rides the first test the way it rides
every request, so a server that accepts it never asks.

### What is saved, and where

Into the same `.env` store a pasted key lands in (`docs/llm.md`), so
`/model`, the ✓ marks and the next launch read it with no second mechanism,
and only once the test passed:

| server | address | token |
| --- | --- | --- |
| Ollama | `OLLAMA_HOST` (Ollama's own variable, in its own grammar) | `OLLAMA_HOST_API_KEY` |
| LM Studio | `LMSTUDIO_HOST` | `LM_API_TOKEN` (LM Studio's own name for it) |
| a server the user's block fixes | *(nothing: its address is the file's)* | its `api_key_env` |

The address is saved **as typed**, as the old host field saved it; the
config completes it. Nothing in the running session is rebuilt. The next
backend build (a `/model` pick, a `/settings` change) resolves the new
address, as a newly pasted key is resolved.

**A variable exported in the shell outranks the store.** Resolution reads the
process environment first (`tui::config::resolve_env`), and an app cannot
unset its own environment, so with `OLLAMA_HOST` exported (common among
Ollama users) an address typed here would pass its test, be saved, and never
be used. The field offers the exported value as the address in force, and a
save it overrides gets `Saved — unset OLLAMA_HOST in your environment, which
overrides it` (red, `ServerSave::overridden_notice`) instead of the
confirmation. `ModelSession::exported_overrides` decides: a host is compared
as the server it names, so `localhost:1234` exported and
`http://localhost:1234/v1` saved are one address and nothing is said; a
token is compared as typed. The save still happens, so it takes effect once
the variable is unset.

There is deliberately no "save anyway" for an address that fails its test.
`/login` saves what it has seen work. A server that is down right now is
pointed at by setting its variable by hand (`export LMSTUDIO_HOST=…`, or a
line in `~/.alter-zero/.env`).

## LM Studio as a provider

```toml
[providers.lmstudio]
name = "LM Studio"
auth = "optional_key"
api_key_env = "LM_API_TOKEN"
api_base_env = "LMSTUDIO_HOST"
api_model_base = "../api/v1"

[providers.lmstudio.kwargs]
api_base = "http://127.0.0.1:1234/v1"
```

- **The wire is chat completions**, over LM Studio's OpenAI-compatible `/v1`.
  Tool calls, streaming and usage frames are its native shape:
  `stream_options.include_usage` is honoured since LM Studio 0.3.18. Its
  reasoning arrives as a `reasoning` / `reasoning_content` delta, which the
  `ThinkingSplitter` already reads, so a reasoning model's thoughts stream
  into the `● Thinking…` block like any other provider's.
- **The listing is LM Studio's native one**, `GET /api/v1/models` (LM Studio
  0.4+). `/v1/models` names ids and nothing else. The native records carry
  what the picker needs, read per record like every other provider's
  (`llm::models`, `is_lmstudio_record`: a string `key` beside a
  `loaded_instances` array):
  - **the id is the `key`** (`qwen/qwen3-8b`), and `display_name` the label;
  - **only language models are offered.** `type: "embedding"` (every install
    ships one) has no chat surface and would fail every turn;
  - **the window is the loaded one.** LM Studio serves a model at the
    context length it was **loaded** with (`loaded_instances[0].config.
    context_length`, commonly 4096), not the most it could take. A loaded
    model gauges against that, an unloaded one against `max_context_length`;
  - **image input** is `capabilities.vision`, an explicit bool.
- **`api_model_base` is relative**, `../api/v1`, resolved against the chat
  base the way a browser resolves a link (`config::resolve_relative_base`). It
  follows the server wherever it is pointed: `gpu-box:1234` lists from
  `http://gpu-box:1234/api/v1`, and a reverse proxy's
  `https://proxy/lmstudio/v1` from `https://proxy/lmstudio/api/v1`. Any
  provider may write one (`./x`, `../x`, `/x`). An absolute model base on a
  provider whose host was overridden still gives way to the chat base, as it
  always did.
- **An older LM Studio** (before 0.4) has no `/api/v1`. Its native listing
  answers 404, and `fetch_models` falls back to the chat base's `/models` for
  any chat-completions provider whose model base differs from its chat base.
  The rows then carry ids alone: no window, image input unknown, and any
  embedding model listed.
- **A stopped server is advice, not a wait.** A refused connection to a
  keyless server on this machine is a server that is not running, not the
  internet being down. `OpenAiClient::explain` answers it with the test's own
  sentence (`local::refused_advice`): `Nothing is answering at
  127.0.0.1:1234 — is LM Studio's server running?`, surfaced on the bounded
  retry in seconds, as a stopped Ollama already was. LM Studio on another
  machine keeps the offline wait (`docs/offline.md`).
- **No cache breakpoints.** `qwen/qwen3-8b` is spelled the way OpenRouter
  names an explicit-caching model, so `cache::needs_cache_breakpoints` says
  yes to it. The payload builder marks only a provider that takes a key, so a
  keyless server's messages keep the plain string form
  (`docs/prompt-caching.md`).

### The address grammar

`LMSTUDIO_HOST` is typed the way LM Studio shows its server, so the config
reads a bare host as a **server** (`llm::local::base_from_host`, applied to
every non-Ollama provider whose `api_base_env` is set):

| typed | chat base |
| --- | --- |
| *(empty)* | `http://127.0.0.1:1234/v1` (the file's own) |
| `localhost:1234` | `http://localhost:1234/v1` (the file's scheme and path) |
| `gpu-box` | `http://gpu-box:1234/v1` (and its port) |
| `https://lms.example.com` | `https://lms.example.com/v1` (a typed scheme keeps its own default port) |
| `0.0.0.0:1234` | `http://127.0.0.1:1234/v1` (a bind address is not one a client reaches) |
| `::1` / `[::1]:1234` | `http://[::1]:1234/v1` |
| `http://gpu-box:1234/v1` | as typed (a path is the base itself) |
| `https://proxy.example/openai` | as typed |

A value with a path is taken as typed, exactly as before. So a full base in
the variable (the old `/login` host field saved the block's base) resolves
unchanged. Ollama keeps its own grammar, `ollama::host_url`, Ollama's
`envconfig.Host()` ported (`docs/ollama.md`).

### Thinking

LM Studio names its own reasoning switch (`capabilities.reasoning`, e.g.
`allowed_options: ["off", "on"]`). The chat wire's `reasoning` parameter is
not that switch. LM Studio documents `reasoning.effort` only on its
`/v1/responses`, so a Ctrl+T mode would be a footer claiming an effort the
server ignores. An LM Studio model therefore offers **no** Ctrl+T cycle: it
thinks as LM Studio is configured to, and its thoughts still stream.

## Self-signed certificates

A local server reached over **https** often has a certificate no public CA
signed: a reverse proxy on a home lab, LM Studio behind Caddy with its
internal CA. The connection test fails such a server at the TLS handshake,
and the page says so and how to get past it:

> `gpu-box:8443's certificate is not trusted — it may be self-signed. Point
> ALTER_ZERO_CA_FILE at its CA certificate, then test again.`

`ALTER_ZERO_CA_FILE` (or `SSL_CERT_FILE`) adds trust roots to every request
this process makes (`llm::http_client`). It is read when the first client is
built, so it is set before launch.

**Future: ask whether to accept the certificate.** The page could offer the
choice instead of only reporting the failure. This is written down here
rather than built, so its shape is agreed before any code exists:

1. **Show what is being trusted.** On an untrusted-certificate failure, the
   page fetches the leaf certificate (a TLS handshake with verification off,
   reading the peer certificate and sending nothing) and shows its subject,
   issuer, validity and **SHA-256 fingerprint**. It then asks
   `Trust this certificate for gpu-box:8443? ❯ 1. Trust it  2. Cancel`, the
   same numbered-choice shape as the permission prompt.
2. **Pin, don't disable.** Trusting records `{host:port → fingerprint}` in a
   per-user file in the config home (`trusted_certs.json`, the `trust.json`
   posture: read-modify-write, a malformed file failing closed with a red
   toast). Requests to that host:port then verify the chain against that
   one fingerprint and nothing else, through a custom rustls
   `ServerCertVerifier` used only for pinned hosts. Verification is never
   turned off. A changed certificate (re-issued, or a man in the middle)
   fails the pin and asks again, showing both fingerprints.
3. **Scope.** Only addresses entered under **Use a local model**, never a
   hosted provider: a cloud API presenting an untrusted certificate is an
   attack to refuse, not a prompt to answer.
4. **Why it is not built yet.** It needs a second TLS verifier path into the
   one shared HTTP client (`llm::http_client` caches clients per timeout), a
   handshake that reads a certificate it did not verify, and a new file the
   whole session trusts. Each is security-relevant code and deserves its own
   review. `ALTER_ZERO_CA_FILE` covers the need meanwhile, and the failure
   names it.

## Upgrading

- **A `[providers.lmstudio]` block of your own** (written from the 0.15.0
  `docs/user-providers.md` example) now **patches** the shipped LM Studio
  instead of adding a provider, since a block naming a shipped id changes
  only the keys it sets. The shipped block's `api_base_env` makes it
  configured by being pointed at rather than by the block, so connect it once:
  `/login` → Use a local model → LM Studio → Enter tests the address your
  block names and saves it. Or delete the block, since the shipped one is
  what it described.
- **A local Ollama connected before** keeps working: its `OLLAMA_HOST` is
  read as it always was. It is found under Use a local model now, not under
  Use an API key.

## Where the code is

- **`llm::local`** (pure): `base_from_host` / `server_address` (the address
  grammar and the field's offer), `server_label`, `Verdict` + `verdict` (the
  page's sentences), `unusable_address` (a typo answered before any
  request, with the server's own address as the example) and
  `refused_advice` (a stopped server mid-session, in the test's words).
- **`llm::openai`**: `explain` turns a keyless loopback refusal into that
  advice, and `chat_request` leaves a keyless provider's messages unmarked.
- **`llm::config`** (pure): the shipped `lmstudio` block; a non-Ollama
  `api_base_env` value completed by `base_from_host`; relative
  `api_model_base` (`is_relative_base`, `resolve_relative_base`), accepted in
  a user's block.
- **`llm::models`**: LM Studio's native records (`is_lmstudio_record`,
  `lmstudio_loaded_context`, the type filter, `capabilities.vision`, no
  reasoning); the 404 fallback to the chat base's listing in `fetch_models`.
- **`app::login`** (pure): `LoginMethod::Local`, `KeyStep::Local` /
  `KeyStep::Server`, `LocalChoice`, `ServerPage`, `ServerTest`, `TokenAsk`,
  `ServerSave` (+ `confirmation`, `overridden_notice`),
  `App::finish_server_test`,
  `KeyOnboarding::server_takes_typing`. The old `KeyKind::Host` field is gone
  with the host-configured rows it served.
- **`ui::login_view`**: the local list (the shared list page) and
  `server_page_lines`. `ui::layout` seats the caret after the typed address
  or the token's mask and hides it while a test runs or the address is fixed.
- **`tui::config::local_choices`** / `is_configured`, the partition in
  `provider_choices` (`auth == api_key` only);
  `ModelSession::local_test_config` / `exported_overrides`;
  `tui::workers::spawn_server_test` (`DeviceEvent::Tested`); `tui::login`'s
  `test_local_server` / `finish_server_test` (save, toast, frame).

## Testing

The grammar, the verdict, the listing parse and its fallback (against a
loopback stub), the relative listing base, and the page's whole key grammar
are unit-tested (`llm::local`, `llm::config`, `llm::models`,
`app::tests::login`, `ui::tests::login_view`). `smoke.sh` Phase 141 drives
the real binary against three local stubs: an LM Studio listing one language
model and one embedding model, an LM Studio requiring a token, and an
Ollama. It walks the third root row, the API-key list without the local
Ollama, a refused address shown in red and saved nowhere, a pass that saves
`LMSTUDIO_HOST` and lists the language model in `/model` (never the
embedding one), the token step (asked, refused, then saved beside the
address), and Ollama saving `OLLAMA_HOST`. Then a session on LM Studio's
model whose server has stopped must end its turn on `Nothing is answering at
…` rather than `Waiting for internet…` (the check fails with the advice
disabled, which is how it was verified). Last, in that session, whose
`LMSTUDIO_HOST` is exported, a working address saved under `/login` must say
`Saved — unset LMSTUDIO_HOST in your environment, which overrides it`
rather than confirm. The token step's pass is awaited on the `.env` write,
not its toast: the earlier pass's confirmation reads the same and a toast
lives 4 s, so a toast wait could return before the test did. The suite
unsets `OLLAMA_HOST`,
`LMSTUDIO_HOST` and `LM_API_TOKEN` (the last does not end in `API_KEY`, so
the key scrub would miss it), so a developer's own server never configures a
phase.

## Known limitations

- **LM Studio's window is its load setting.** Unlike Ollama's
  `options.num_ctx` (`docs/ollama.md`), the chat-completions wire cannot ask
  LM Studio for a context length, so a model loaded at 4096 overflows on an
  agentic turn's first file read. The gauge shows the loaded window when the
  model is loaded. Raise it in LM Studio, or with
  `lms load <model> --context-length 32768`. Loading the selected model at
  the session's window through LM Studio's `/api/v1/models/load` is the
  next step.
- **A model not yet loaded** is gauged against its maximum: LM Studio loads it
  on the first request at a setting its listing does not report.
- **No Ctrl+T for LM Studio** (above).
- **No "save anyway"** for an address that fails its test (above).
- **Self-signed certificates** are trusted through `ALTER_ZERO_CA_FILE`; asking
  to accept one is designed above, not built.
