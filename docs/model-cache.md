# The `/model` list cache

The `/model` picker used to fetch every configured provider's `/models`
listing on every open, so each open started on `Loading models…`. With
OpenRouter's 600 KB catalog, a ChatGPT sign-in, Copilot and a local server
configured, that was a second or more before the list appeared, every time,
and the most common case (opening the picker to switch models) paid it
most. The picker now opens on the lists it **kept** from earlier fetches,
fetches again only what is old, and **Ctrl+R** asks every provider again
on demand.

The pure half is `llm::model_cache` (the file format, the freshness verdict,
the source fingerprint, the file name) and the picker's per-provider rounds
in `app::model_picker`. The reads and writes are `tui::config`'s, and the
fetch workers that write the cache are `tui::workers`'.

## What is kept, and where

One file per provider, `{config_home}/model-cache/{provider}.json`
(`~/.alter-zero/model-cache/openrouter.json`), holding exactly the rows
`fetch_models` returned (`ModelEntry`: id, name, reasoning ladder, image
input, context window, speed tiers) and four facts about them:

```json
{"version":1,"app_version":"0.19.0","source":"sha256:…","fetched_at":1763100000,
 "models":[{"id":"openai/gpt-5.5","name":"OpenAI: GPT-5.5","reasoning":{"efforts":["low","medium","high"],"can_disable":true},"vision":true,"context":400000}]}
```

- **One file per provider** rather than one shared file: the fetches run in
  parallel, each on its own worker, and each worker writes its own file, so
  no write ever has to merge with another. Invalidating a provider means
  removing one file.
- **Written by the worker**, off the loop, the moment its fetch lands, beside
  the file and then renamed over it, so another instance reading at that
  moment reads the old listing or the new one, never half of either.
- **A fetch the picker gave up on keeps nothing.** Closing the picker (or a
  newer round) cancels its fetches, and one that lands anyway (`fetch_models`
  stops at the headers when cancelled, but a body already arriving is read
  to its end) is neither kept nor delivered. Closing the picker may be the
  start of a `/login` save that drops the provider's kept list (below), and
  a listing read with the credential that save replaced, written after it,
  would pass for fresh for an hour. Keeping it would only have saved the
  next open one fetch.
- **Compact, and only what the picker shows.** A row leaves out its provider
  (the file is the provider's) and a name that only repeats its id. The
  measured cost: a 611 KB OpenRouter-shaped listing of 420 models keeps as a
  **37 KB** file and reads back in about half a millisecond in a debug build,
  so the open reads it on the loop. It is read through a buffer into typed
  rows, never through a `Value` tree (`docs/memory.md`), and bounded at the
  listing's own 8 MiB.
- **Nothing is held in memory between opens.** The picker holds the rows
  while it is open, as it always did. The disk is the cache, so two instances
  share it.
- **No config home, no cache**: every open fetches, exactly as before.

A file that does not parse, or is of another format version, is no cache
at all, never an error: its provider is fetched as though nothing had been
kept.

## What a kept list is worth when the picker opens

`model_cache::freshness` answers once per configured provider:

| The kept listing | The picker |
| --- | --- |
| none, or unreadable | fetches it; `Loading models…` / `loading more…` as before |
| fetched from somewhere else (`source` differs) | **unusable**: treated as none |
| under an hour old, same app version | **fresh**: shown, nothing is fetched |
| an hour old or more, dated in the future, or kept by another app version | **stale**: shown at once, fetched again behind it (`refreshing…`) |
| a server the user runs (`auth = "optional_key"`: Ollama, LM Studio, a keyless server of their own) | **stale** at any age |

- **An hour** (`MODEL_CACHE_TTL_SECS`). A catalog changes over days, so a
  model listed this morning is there by lunch without anyone pressing
  anything, while reopening the picker to switch back and forth costs no
  request at all. Since a stale list is shown before it is refreshed, the
  TTL decides how often the network is asked, never how long the user waits.
- **A self-hosted server is always asked again.** Pulling a model is exactly
  how its list changes, and the request goes to a server the user owns. This
  is decided by the kind of provider and not by its address: an Ollama on
  another machine changes on every pull just the same, while a key-taking
  gateway on `localhost` (a LiteLLM proxy) lists a catalog that moves no
  faster than any other. An address rule was tried first, and the smoke
  stub on `127.0.0.1` is what showed it to be the wrong question.
- **Another app version's listing is stale, not unusable.** A newer version
  may read a fact from the listing an older one did not (a new capability
  field), so it asks again, but the old rows are still a far better first
  frame than `Loading models…`.
- **The source** (`listing_source`) fingerprints what a listing is fetched
  through: the wire format, the auth scheme, the two bases (the listing's,
  and the chat base a server without one is listed through) and the
  request's extra headers. A `providers.toml` edit or an `OLLAMA_HOST` that
  moves any of them makes the kept rows another server's. It is a SHA-256,
  so a base that carries a password writes it nowhere new, and the key is
  not part of it. A credential never reaches this file in any form.

## When a kept list is dropped

A `/login` save (a pasted key, a sign-in's token, a local server's address
or token) removes the kept list of **every provider the saved variable
configures**, by key or by address (`tui::config::providers_reading`). Signing
in again is how a user changes accounts, and another account can list other
models (a Copilot Business seat, a different ChatGPT plan), so the next open
fetches. A key changed outside the app (a hand-edited `.env`, a different
exported variable) is not seen. The hour bounds how long that lasts, and
Ctrl+R ends it at once.

## Ctrl+R, not `r`

The picker is type-to-search: every printable key types into the filter,
and `r` is in `deepseek-r1`, `openrouter` and `grok`. So the refresh is
**Ctrl+R**, which the picker owns while open (the composer's reverse history
search never sees it). It does what an open does, with every fetch forced:
the provider file is re-read, a provider added since is fetched, one
removed since leaves with its rows, and every configured provider is asked
again. It is ignored while a round is still out (the `/usage` page's `r`
rule): the round in flight already answers the question, and a second would
only cancel it and ask again. It is also the retry after a failure, and the
all-failed page says so under its red rows (`ctrl+r to try again`).

## Rounds

Every open and every Ctrl+R is a **round** (`App::begin_model_round`), and
every fetch result names the round it answers (`App::finish_model_fetch`).
A result from any other round is dropped: an earlier round's (Ctrl+R
pressed while a fetch was out), or a closed picker's. The counter lives on
`App` rather than the picker (the `/usage` page's `usage_rounds`), so a
picker closed and reopened can never take the old one's answer. The old
append-only merge had exactly that race: a late answer for a closed picker
was merged into the next one and decremented its outstanding count.

Within a round, each configured provider is a `ProviderList`: where its
rows on screen came from (`listed_at`, the cache's date or the fetch's),
whether the round is fetching it, and why its fetch failed. A listing
**replaces** that provider's rows, so a model it no longer lists goes. A
failure keeps whatever rows the provider showed. A round laid over an open
picker keeps each provider's rows on screen, so a refresh never blanks a
list it is about to replace.

**The highlight.** An untouched picker seats on the model in use as lists
land. Once the user moves or types (`ModelPicker::touched`), the highlight
stays on the row it is on. The old merge guessed "untouched" from a
highlight at the top, which kept rows made wrong: the picker now opens
seated on the model in use, and a user who pressed Home to the first row
would have had a landing refresh move them back.

## What the page says

All of it sits beside the `(n/total)` counter, the one place the page
already reported on its lists. The page gained no row and its height is
unchanged. In order:

- `updated 2h ago`: how old the **oldest** list on screen is, once that is a
  minute or more (`MODEL_AGE_SHOWN_SECS`). A page fresh off the network
  stays the page it always was.
- `loading more…` while a provider with nothing on screen is still being
  fetched, else `refreshing…` while kept rows are.
- `{provider} unavailable` (red) for a provider whose fetch failed with
  nothing of it to show, and `{provider} not refreshed` (amber, the `/usage`
  page's failed-refresh colour) for one whose kept rows stay on screen and
  stay usable. Its reason sits under the list in the same colour.
- `ctrl+r to refresh`, only when nothing is out and pressing it would
  change something: the lists are a minute old or more, or a provider
  failed. It goes **whole** when the row cannot hold it, never cut to half
  a key name.

```
  → anthropic/claude-sonnet-5 [openrouter] ✓
    deepseek/deepseek-r1 [openrouter]
    (1/412)   ·   updated 2h ago   ·   refreshing…
```

A search that matches nothing in a kept list carries the same notes after
`No matching models`, since the model being searched for may have been
listed since. While anything is still being fetched it keeps saying
`Loading models…`, the same rule as before applied to a refresh: a list
still arriving cannot prove a query has no match.

## Verified

- `llm::model_cache`'s tests: the round trip, the version and corrupt-file
  cases, every freshness row of the table above, the source fingerprint's
  inputs, and file names an id quoted in `providers.toml` cannot escape.
- `app::tests::model_picker`: rounds, replacement, kept rows through a
  failure, dropped rounds, a refresh keeping the rows on screen, the
  touched highlight, and Ctrl+R (and that a plain `r` still types).
- `ui::tests::model_view`: every note above, the hint left out whole, the
  amber, and the retry row.
- `smoke.sh` Phase 155, against a stub provider whose list, delay and status
  are files the phase rewrites: the first open fetches once and keeps the
  list; after a restart the kept list is the page with **no request**; Ctrl+R
  brings a new model and keeps it; a list aged two hours shows at once under
  `updated 2h ago · refreshing…` until the answer replaces it; a 503 keeps
  the rows as `Stub not refreshed`; a key saved through `/login` drops the
  file; and a fetch whose body is still arriving when the picker closes
  keeps nothing.
