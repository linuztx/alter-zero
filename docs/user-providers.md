# Your own providers — `~/.alter-zero/providers.toml`

The providers the app ships with live in the repository's `providers.toml`,
embedded in the binary. Anything else that speaks an OpenAI-compatible API —
a hosted service like Groq, DeepSeek, Mistral or Together, a company proxy,
or a model server on your own machine like llama.cpp or vLLM — is
added in a file of the user's own (Ollama and LM Studio ship built in,
`docs/local-models.md`): `~/.alter-zero/providers.toml`
(`{config_home}/providers.toml`, so `ALTER_ZERO_CONFIG_DIR` moves it with the
rest of the state). The block shape is the shipped file's, and the file is
**laid over** the built-in providers: it adds to them, and it can change one
of them, but it never replaces the set.

It used to replace it. The first file that parsed — `ALTER_ZERO_PROVIDERS_FILE`,
then a `providers.toml` in the working directory, then the one in the home
directory — became the whole provider table, so adding one provider meant
copying the entire shipped file, and the copy then never received a fix or a
new provider again. A file that did not parse was skipped without a word: the
app quietly ran on the built-ins while the provider the user wrote simply
wasn't there. All three are gone, and what replaced them is below.

## A provider in a few lines

```toml
[providers.groq]
name = "Groq"
description = "Fast inference on open models."
api_key_url = "https://console.groq.com/keys"

[providers.groq.kwargs]
api_base = "https://api.groq.com/openai/v1"
```

- **`[providers.groq]`** is the id. It tags the provider's rows in `/model`
  (`[groq]`), names it in `config.json` and in `ALTER_ZERO_PROVIDER=groq`, and
  decides where the key is read from: `GROQ_API_KEY`, the id upper-cased with
  `_API_KEY` after it, punctuation becoming `_`. `api_key_env = "…"` names
  another variable.
- **`name`** is the label `/login` and the footer show. A block without one
  is called by its id, so a local server can be its base and nothing else.
- **`kwargs.api_base`** is required — the Chat Completions base. Requests go
  to `{api_base}/chat/completions` and the model list is read from
  `{api_base}/models`. It sits under `kwargs`, the table whose other keys are
  forwarded in the request body, because that is where the shipped file has
  always kept it.
- **`description`** and **`api_key_url`** are optional. `/login`'s key step
  shows the first and links the second, so a user without a key knows where
  to make one.

Then `/login` → **Use an API key** → **Groq** saves the key to
`~/.alter-zero/.env` (an exported `GROQ_API_KEY` works as well), and `/model`
lists Groq's models beside every other configured provider's. **Both re-read
the file when they open**, so a provider added while the app is running — by
hand, or by asking the agent to write the block — is there without a restart.
The live backend is untouched by a re-read: it keeps streaming on the config
it was built from, and only what reads the table next (the page, a pick from
it, a later rebuild) sees the change — and a file read mid-edit never takes
the running model's provider away (*Editing it while the app runs*, below).

Other services take the same block with their own id, name and base:

| id | `kwargs.api_base` | key read from |
| --- | --- | --- |
| `openai` | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
| `deepseek` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` |
| `together` | `https://api.together.xyz/v1` | `TOGETHER_API_KEY` |
| `mistral` | `https://api.mistral.ai/v1` | `MISTRAL_API_KEY` |
| `fireworks` | `https://api.fireworks.ai/inference/v1` | `FIREWORKS_API_KEY` |
| `xai` | `https://api.x.ai/v1` | `XAI_API_KEY` |

## Where the table comes from

| | the provider table |
| --- | --- |
| `ALTER_ZERO_PROVIDERS_FILE` names a file | that file **alone**, none of the built-ins — a curated table, or a test's stub: the smoke suite points it at one stub provider so nothing else is fetched |
| otherwise | the built-in providers, with `{config_home}/providers.toml` laid over them; no such file is the normal case |

Either way the file's blocks are checked the same way (*When the file is
wrong*, below), and a file that cannot be used at all — unreadable, not TOML —
leaves the built-ins in force at launch rather than no providers, and the
session's own providers on a re-read.

**A `providers.toml` in the working directory is never read.** The working
directory is often a checkout the user did not write, and a provider block
there could point a stored key at a host of its choosing:
`[providers.openrouter.kwargs] api_base = "https://…"` would have sent the
user's OpenRouter key there — before the first frame, at that, since the
startup capability probe lists the active provider's models unasked. Project
files that run or reach something are what the `/trust` gate exists for
(`docs/project-config.md`); a provider file belongs to the user, so it is read
from the user's config home only. (The repository's own `providers.toml` is
the source of the embedded built-ins, so running from a checkout loses
nothing.)

## How a block lands

A block whose id is **new** is a new provider. A block naming a **shipped**
provider patches it, key by key: a table meeting a table merges, and any
other value — a string, a number, an array — replaces the shipped one. One
setting can change without restating the rest:

```toml
# Venice's web search, beside the shipped include_venice_system_prompt = false
[providers.venice.kwargs.venice_parameters]
enable_web_search = "auto"

# One more header on OpenRouter's requests; the attribution headers stay
[providers.openrouter.extra_headers]
X-Team = "research"

# OpenRouter's chat requests through a company proxy
[providers.openrouter.kwargs]
api_base = "https://llm-proxy.example.com/openrouter/v1"
```

The last one moves the chat requests only: OpenRouter's shipped block names
its own `api_model_base`, which still decides where the model list comes from
— set `api_model_base` as well to move the listing too.

What a patch cannot do is remove: a shipped key, a shipped header or a shipped
provider stays. A table without some of them is what `ALTER_ZERO_PROVIDERS_FILE`
is for.

## Every key a block can set

| key | what it does |
| --- | --- |
| `name` | the label; the id when unset |
| `kwargs.api_base` | the chat base (required, unless `api_base_env` names a variable that supplies it) |
| `kwargs.*` | anything else under `kwargs` is merged into every request body, nested tables included |
| `description` | one line saying what the provider is — `/login`'s key step, and its type-to-search |
| `api_key_url` | where its keys are made, linked on that step |
| `api_key_env` | the variable the key is read from; `<ID>_API_KEY` by default |
| `api_model_base` | where the model list is read from, when not the chat base — a URL, or a path **relative** to the chat base (`../api/v1`, `./x`, `/x`), resolved the way a browser resolves a link, so it follows the chat base to any host the base is pointed at (LM Studio's native listing, `docs/local-models.md`) |
| `api_base_env` | a variable whose value replaces the chat base (`OLLAMA_HOST`'s role) — a bare host in it (`gpu-box:8080`) is a server, completed from the block's own base: its scheme, its port and its API path; a value with a path is the base itself |
| `extra_headers` | a table of headers sent with every request |
| `auth` | how it authenticates: a pasted key by default, `optional_key` for a server that needs none, or one of the sign-ins (`docs/copilot.md`, `docs/chatgpt.md`, `docs/claude.md`, `docs/cline.md`) |
| `wire_api` | the request shape: Chat Completions by default, or `responses`, `anthropic`, `ollama` |
| `drop_params` | request fields the server refuses, taken out of every request (below) |

## Servers that take no key

`auth = "optional_key"`: a stored key rides as a bearer, and none is fine. Such
a provider is a **local server**, listed under `/login` → **Use a local
model** beside the shipped Ollama and LM Studio rather than under API keys,
and its page tests the connection before saving anything
(`docs/local-models.md`). A key-optional provider that names **no host
variable** is configured by its block alone — the file pointing at it is the
configuration, so `/model` lists it without a trip through `/login`
(`Provider::configured_by_definition`), and its page only tests it:

```toml
[providers.llamacpp]
name = "llama.cpp"
auth = "optional_key"

[providers.llamacpp.kwargs]
api_base = "http://localhost:8080/v1"
```

| server | `kwargs.api_base` |
| --- | --- |
| llama.cpp (`llama-server`) | `http://localhost:8080/v1` |
| vLLM | `http://localhost:8000/v1` |

To have `/login` ask for the address instead, name a variable for it —
`api_base_env = "LLAMACPP_HOST"`. The page then offers an address field over
the block's own server, tests what is typed (a bare `gpu-box:8080` is
completed to the block's `/v1`), saves it to that variable once the test
passes, and the provider counts as configured once the variable resolves:
Ollama's rule (`docs/ollama.md`). No shipped provider is configured by its
block alone — a test pins it — because the shipped `ollama` and `lmstudio`
are for the users who run those servers, and every `/model` open would
otherwise fetch from one most users don't. A server of the user's own that
isn't running costs `/model` a `· llama.cpp unavailable` beside its counter,
nothing more.

LM Studio itself no longer needs a block: it ships, with its native listing
and its `LM_API_TOKEN` (`docs/local-models.md`). A `[providers.lmstudio]`
block written from this page's earlier example now **patches** the shipped
one — connect it once under Use a local model, or delete the block.

## Fields a strict server refuses

Every Chat Completions request carries `stream_options` (asking for the final
usage frame the token tally snaps to) and `prompt_cache_key` (the session's
cache affinity, `docs/prompt-caching.md`). The shipped providers accept both,
but a strict server answers a field it does not know with an error — Mistral,
for one, refuses unsupported parameters with `Extra inputs are not permitted`
— and a kwarg can only *replace* a field, since TOML has no null.
`drop_params` takes the named fields out of every request:

```toml
[providers.strict]
name = "Strict"
drop_params = ["prompt_cache_key", "stream_options"]

[providers.strict.kwargs]
api_base = "https://llm.example.com/v1"
```

The fields come out **last** (`ModelConfig::drop_refused`), after the kwargs
and the thinking mode have been merged in, so whatever put a field there it
does not leave — on every wire format, and for any top-level field, a kwarg's
included. `model` and `stream` are never taken out, since a body without them
is no request to make, and listing either is said. Dropping `stream_options`
can cost the exact token count: a server that then sends no usage frame
leaves the tally on the tokenizer's estimate.

## What `/model` reads from the listing

The picker lists `{api_model_base or api_base}/models`. Both shapes servers
answer with parse — OpenAI's `{"data": […]}` envelope and the bare array
Together AI returns — and each record's facts are read per record, whichever
spelling the server uses: the context window from `context_length`, vLLM's
`max_model_len`, Mistral's (and LM Studio's) `max_context_length` — LM
Studio's native records naming the window a loaded model was loaded with
first (`docs/local-models.md`) — or
`context_window` (gauged at 95%, the ChatGPT backend's rule); image input and
reasoning from the shapes `docs/llm.md` lists. A fact no record states stays
unknown — the footer's gauge hides and auto-compaction stays off
(`ALTER_ZERO_CONTEXT_WINDOW` supplies a window), and images are attached
optimistically (`docs/tools.md`).

A server with no `/models` at all lists nothing; name the model at launch
instead, with `ALTER_ZERO_PROVIDER` and `ALTER_ZERO_MODEL`.

## When the file is wrong

Nothing about the file is skipped in silence. Every problem is one row, raised
as a red toast at startup and again **every time** `/login` or `/model` opens,
showing the first row with the rest counted (`(+2 more)`), each led by the
file, `~`-relative (`~/.alter-zero/providers.toml: …`). Every time, because
a toast is easily lost: the startup one can be covered at once by the next
(a resumed session's checkpoint notice, say), and the page a provider is
missing from is the moment to say why. A problem costs only the block it is
in:

| what | the row | what becomes of it |
| --- | --- | --- |
| a file that does not parse | ``line 3, column 10: expected `.`, `=` — using the built-in providers`` | the whole file is ignored; on a re-read the row ends `— keeping the providers already loaded` instead, and the session's table stands |
| a file that can't be read | `Permission denied (os error 13) — using the built-in providers` | the same |
| `providers` that is not a table | `providers is not a table — using the built-in providers` | the same |
| a provider written without its prefix | `[groq] is not read — a provider goes under [providers.groq]` | nothing loads from it |
| a table of providers under another name (`[provider.groq]`, codex's `[model_providers.groq]`) | `[provider] is not read — providers go under [providers.<id>]` | the same |
| a key nothing reads | `providers.groq.descripton is not a known key` | the key is ignored; the provider loads |
| a key other tools spell differently | `providers.groq.base_url is not a known key — set api_base under [providers.groq.kwargs]` (`api_base` the same; `env_key` → `api_key_env`; `headers`, `http_headers` → `extra_headers`; `model`, `models` → `choose a model with /model`) | the same |
| a key written into the file | `providers.groq.api_key is not a known key — keys are not read from this file: use /login` | the same; the value is never echoed |
| an `auth` or `wire_api` this build doesn't know | `providers.groq.wire_api = "chat_completions" is not known — using chat completions` | the default is used |
| `drop_params` naming `model` or `stream` | `providers.groq.drop_params cannot drop "stream" — every request needs it` | the rest of the list applies |
| a value of the wrong type | ``providers.groq: invalid type: string "x", expected a map, in `extra_headers` — skipped`` | the block is skipped |
| no chat base | `providers.groq has no api_base under [providers.groq.kwargs] — skipped` | the same |
| a base that is no URL | `providers.groq.kwargs.api_base "api.groq.com/openai/v1" is not an http(s) URL — skipped` | the same (an Ollama-wire host like `gpu-box:11434` is exempt, and so is a relative `api_model_base` like `../api/v1`) |
| a block that is no table | `providers.groq is not a table — skipped` | the same |

A broken block that patched a shipped provider ends `— kept the built-in`
instead of `— skipped`: the shipped provider stays as shipped. A broken block
of the provider the session is running on ends `— kept the one in use`
(*Editing it while the app runs*, below).

Unknown `auth` and `wire_api` values still degrade to the defaults rather
than failing the block — the rule that lets a file written for a newer build
load (`docs/copilot.md`) — and so do unknown keys; what changed is that a
user's own typo is said.

**Keys stay out of the file.** A provider file is meant to be read, shared
and copied between machines, and a key is not: keys live in `~/.alter-zero/.env`
(written by `/login`) or in the environment, and an `api_key` in a block is
reported, never read.

## Editing it while the app runs

`/login` and `/model` re-read the file as they open, so it is often read
mid-edit: saved half-written, a block cut to be pasted elsewhere, a base being
retyped. A re-read never takes anything away from the model that is running:

- A file that **can't be used at all** — unreadable, not TOML, no table of
  providers — leaves the session's providers exactly as they were:
  ``~/.alter-zero/providers.toml: line 1, column 16: invalid table header, expected `.`, `]` — keeping the providers already loaded``.
- A file that loads but no longer gives a **usable definition of the provider
  in use** — its block skipped, or gone from the file — keeps the definition
  the running model was built from, its row ending `— kept the one in use`
  (`providers.acme is not in the file — kept the one in use` for a block
  removed). The rest of the file applies as usual.
- A usable edit of the provider in use applies, from the next rebuild on.

It matters because the running model resolves its provider again on every
rebuild — a `/settings` change to Temperature, Tools, Error retry or Max tool
calls, a Ctrl+T thinking mode, a `/fast` tier, the capability probe's answer —
and a provider missing from the table skips that rebuild without a word: the
setting shows as applied and never reaches a request. Keeping the definition
in use is also why a patch of a shipped provider broken mid-edit doesn't send
the running model back to the shipped block: a proxy patched onto OpenRouter
keeps carrying its requests instead of being routed around. Only the provider
in use is kept — one the session isn't running on leaves `/login` when its
block goes, as at a launch — so after a switch to another model the kept one
goes too, at the next re-read, and a restart leaves the file alone to decide.

## Where the code is

- **`llm::config`** (pure) — `ProvidersFile::with_user_layer` and `standalone`
  (the merge, `merge_tables`, every check, `layer_block`, and the `in_use`
  provider a re-read keeps), each an `Err` for a file that can't be used at
  all, so what stands instead is the caller's to say;
  `Provider::unread`, the keys a block set that the parse never read — kept by
  a `#[serde(flatten)]` field, so the struct itself is the list of keys and the
  shipped-file test needs no hand-kept one; `Provider::configured_by_definition`;
  `Provider::drop_params` and `ModelConfig::drop_refused`, which each wire's
  request builder calls last.
- **`llm::models`** (pure) — `parse_models` reads either listing shape, and
  `context_window_of` the window's spellings.
- **`tui::config::load_providers`** (boundary) — which file, its rows led by
  the file `~`-relative; `startup_providers` falls back to the built-ins, and
  `ModelSession::reload_providers`, run whenever `/login` or `/model` opens,
  to the session's own table, handing in the provider in use;
  `Session::report_provider_issues` raises the toast.

## Testing

- **Unit** — `src/llm/config.rs`: `a_user_file_adds_its_providers_beside_the_builtins`,
  `a_user_block_naming_a_shipped_provider_patches_only_what_it_sets`,
  `a_standalone_file_is_the_whole_table`, one test per row of the table above,
  `a_provider_without_a_name_is_called_by_its_id`, the re-read's five
  (`a_reread_keeps_the_provider_in_use_when_its_block_cannot_be_used`,
  `…_when_its_block_is_gone`,
  `a_reread_keeps_a_patched_builtin_in_use_rather_than_the_shipped_one`,
  `a_reread_takes_a_usable_edit_of_the_provider_in_use`,
  `a_reread_keeps_only_the_provider_in_use`),
  the keyless rule's pair (`a_keyless_provider_naming_no_host_variable_is_configured_by_its_definition`,
  `no_shipped_provider_is_configured_by_its_definition`) and `drop_params`'
  three; one `drop_params` test per wire in `openai.rs`, `responses.rs`,
  `anthropic.rs` and `ollama.rs`; `src/llm/models.rs`:
  `a_listing_that_is_a_bare_array_reads_like_the_envelope` and
  `the_window_spellings_self_hosted_servers_use_are_read`.
- **Smoke** — Phase 140 (`scripts/smoke/phases/140-userproviders.sh`) writes a
  provider into the phase HOME's `~/.alter-zero/providers.toml` and drives the
  real binary against a stub that lists its models as a bare array and logs
  each request's keys and temperature: `/login` shows the provider configured
  beside every shipped one, its key step shows the file's description and key
  page, `/model` lists the stub's models, a turn streams with the key as a
  bearer and without the field `drop_params` names, a block added mid-session
  is a row at the next `/login` while a misplaced `base_url` is a toast naming
  the file and the fix — said again at the next open. With the provider in
  use, a file saved half-written keeps every provider and a `/settings`
  Temperature change still reaches the request, a broken block of the
  provider in use is kept and its next request still goes to the stub, and
  the fixed file says nothing. A file that does not parse at launch is a
  toast naming its line with the built-ins standing in; with
  `ALTER_ZERO_CONFIG_DIR` set, `~/.alter-zero/providers.toml` goes unread;
  and a `providers.toml` planted in the working directory never becomes a
  row.
