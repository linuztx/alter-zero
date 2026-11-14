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
the source fingerprint, the file name, the ledger of fetches still out and
what an open does about each provider) and the picker's per-provider lists
in `app::model_picker`. The reads and writes are `tui::config`'s, the
ledger is held by `tui::models`' `ModelSession`, and the fetch workers are
`tui::workers`'.

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
  parallel, each on its own worker, and each answer replaces only its own
  provider's file, so no write ever has to merge with another. Invalidating
  a provider means removing one file.
- **Written by the loop, as the fetch settles.** The worker hands its rows
  to the loop, which asks the ledger (below) whether this is still the
  provider's live fetch and, if so, writes the file beside the old one and
  renames it over it, so another instance reading at that moment reads the
  old listing or the new one, never half of either. The loop is where a
  `/login` save runs too, so the verdict and the write are one step on one
  thread: a save that retired the fetch came before it (and the answer is
  dropped) or comes after it (and removes what was written). A worker
  writing on its own thread could land between the save's removal and the
  user's next open, a list read with the replaced credential passing for
  fresh. The write costs the loop about what an open's read does: a 54 KB
  listing (larger than OpenRouter's) writes, renamed into place, in about
  0.4 ms in a dev build.
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
- **No config home, no cache**: every open fetches, exactly as before, and
  an answer is shown without being kept.

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
  The age is computed checked: a hand-edited file dated anywhere an `i64`
  reaches is stale, never an overflow.
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

## The fetches still out

A fetch belongs to the session, not to the picker that started it. The
ledger (`model_cache::FetchLedger`, held by `ModelSession`) keeps at most
one **live** fetch per provider: a ticket, the source it reads, when it
started, and its own cancel token. Every answer names its ticket, and only
the live ticket's answer is kept or shown (`FetchLedger::settle`).

- **Closing the picker cancels nothing.** What a fetch reads is no less true
  for the picker having closed, so its answer is kept when it lands, and the
  next open finds it fresh instead of asking again.
- **A reopen waits on a fetch still out** rather than asking again
  (`model_cache::plan` → `Await`): the page shows the kept rows under
  `refreshing…` (or `Loading models…` with nothing kept), and the answer
  lands in it. Opening and closing the picker in quick succession used to
  cancel and restart the same request each time, so a provider slower than
  the user's reopening never got to answer.
- **An open waits on the fetch out, or retires it**
  (`FetchLedger::await_or_retire`). It waits only on one from the same
  source started under a minute ago (`FETCH_GIVE_UP_SECS`). One read from
  where the provider no longer points, or out for a minute, is retired, its
  answer dropped, and the open asks again if what it kept is not fresh. A
  fetch it does not wait on could otherwise land over a fresh list the open
  kept instead. A minute is far past any listing that is coming, and short
  enough that a fetch hung on a dead connection does not stop the picker
  from asking for a minute more.
- **A `/login` save retires** the fetch out for every provider the saved
  variable configures (`FetchLedger::retire`): its token is cancelled and
  its ticket closed, so a list read with the credential the save replaced is
  neither kept nor shown when it lands. This is the race the first version
  of the cache closed by cancelling every fetch on close (a close may be the
  start of a `/login` save), which also threw away every list still arriving
  when the picker closed; it is closed now where it actually is.
- **Quitting cancels every fetch still out** (`FetchLedger::cancel_all`).
- **A fetch that panics still answers.** The worker holds a guard
  (`tui::workers`' `AnswerOnPanic`) that sends an error for its ticket if
  the thread unwinds, so its provider shows why (`the model list fetch
  stopped unexpectedly`) instead of the next open waiting a minute on an
  answer that will never come.

`fetch_models` honours its cancel token up to the response's headers; a body
already arriving is read to its end, and the worker then sends nothing.

## When a kept list is dropped

A `/login` save (a pasted key, a sign-in's token, a local server's address
or token) removes the kept list of **every provider the saved variable
configures**, by key or by address (`tui::config::providers_reading`), and
retires any fetch still out for them (above). Signing in again is how a
user changes accounts, and another account can list other models (a
Copilot Business seat, a different ChatGPT plan), so the next open fetches.
A key changed outside the app (a hand-edited `.env`, a different exported
variable) is not seen. The hour bounds how long that lasts, and Ctrl+R ends
it at once.

## Ctrl+R, not `r`

The picker is type-to-search: every printable key types into the filter,
and `r` is in `deepseek-r1`, `openrouter` and `grok`. So the refresh is
**Ctrl+R**, which the picker owns while open (the composer's reverse history
search never sees it). It does what an open does, with every fetch forced:
the provider file is re-read, a provider added since is fetched, one
removed since leaves with its rows, and every configured provider is asked
again. It is ignored while a fetch is still out (the `/usage` page's `r`
rule): the fetch in flight already answers the question, and the ledger
would wait on it anyway. It is also the retry after a failure, and the
all-failed page says so under its red rows (`ctrl+r to try again`).

## What the picker takes

Every open and every Ctrl+R lays a **round** over the picker
(`App::begin_model_round`): one `ProviderList` per configured provider,
saying where its rows on screen came from (`source`, the fingerprint above,
and `listed_at`, the cache's date or the fetch's), whether the picker is
waiting on a fetch for it, and why its fetch failed. An answer reaches the
picker only once the ledger has settled it as the provider's live fetch,
and the picker takes it only for a provider it is waiting on
(`App::finish_model_fetch`); a closed picker takes nothing, and its fetch
is kept on disk all the same. A listing **replaces** that provider's rows,
so a model it no longer lists goes. A failure keeps whatever rows the
provider showed.

A round laid over an open picker keeps each provider's rows on screen, so a
refresh never blanks a list it is about to replace, but only rows read from
the same source: after a `providers.toml` edit moved a provider to another
server, its old rows go rather than wait, under the new server's name, for
an answer that may not list them.

**The highlight.** An untouched picker seats on the model in use as lists
land. Once the user moves or types (`ModelPicker::touched`), the highlight
stays on the row it is on. Only a key that does something counts: a typed
character, a move, Backspace, Esc clearing the search, a paste; Tab or a
chord the picker ignores leaves it untouched. The old merge guessed "untouched" from a highlight at
the top, which kept rows made wrong: the picker now opens seated on the
model in use, and a user who pressed Home to the first row would have had a
landing refresh move them back.

## What the page says

All of it sits beside the `(n/total)` counter, the one place the page
already reported on its lists. The page gained no row and its height is
unchanged. In order:

- `updated 2h ago`: how old the **oldest** list on screen is, once that is a
  minute or more (`MODEL_AGE_SHOWN_SECS`). A page fresh off the network
  stays the page it always was. An age that cannot be told (a date at the
  edge of an `i64`) is not shown.
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

**An open picker keeps that true as it sits.** The page is drawn on the
injected clock (`App::set_model_picker_now`), and nothing else redraws a
picker no key is pressed in, so an open picker asks for a frame every ten
seconds (`tui::view`'s `MODEL_PICKER_TICK_INTERVAL`): a list that passes a
minute while the user reads it gains its `updated 1m ago` and the
`ctrl+r to refresh` that goes with it within ten seconds. The tick runs
only while the picker is open, and its frames change nothing else.

A search that matches nothing in a kept list carries the same notes after
`No matching models`, since the model being searched for may have been
listed since. While anything is still being fetched it keeps saying
`Loading models…`, the same rule as before applied to a refresh: a list
still arriving cannot prove a query has no match.

## Verified

- `llm::model_cache`'s tests: the round trip, the version and corrupt-file
  cases, every freshness row of the table above (a date at the edge of time
  included), the source fingerprint's inputs, file names an id quoted in
  `providers.toml` cannot escape, and the ledger: one answer per live
  ticket, a newer fetch superseding and cancelling the one out, a retired
  fetch's answer dropped, tickets kept apart by provider, a young fetch from
  the same source waited on and a minute-old one given up on, an open
  retiring a fetch it does not wait on (another source's, or a minute-old
  one) and keeping the one it does, quitting cancelling every fetch, and
  what an open plans for a fresh list, an old one, none, and a fetch
  already out.
- `app::tests::model_picker`: replacement, kept rows through a failure, a
  refresh keeping the rows on screen and dropping another source's, answers
  taken only for a provider the picker waits on, a reopened picker taking
  the fetch it waits on, nothing over a closed picker, the touched
  highlight (and a key that does nothing leaving it untouched), and Ctrl+R
  (and that a plain `r` still types).
- `ui::tests::model_view`: every note above, the hint left out whole, the
  amber, the retry row, and no age named for a date at the edge of time.
- `tui::workers`' tests: a fetch that panics still answers its ticket, and
  one that ends normally leaves the answer to the worker.
- `smoke.sh` Phase 155, against a stub provider whose list, delays and
  status are files the phase rewrites, and whose log records each request
  and each answer sent: the first open fetches once and keeps the list;
  after a restart the kept list is the page with **no request**, and the
  kept file names neither the stub's address nor its key; Ctrl+R pressed
  twice asks **once** and keeps the new model; a list aged two hours shows
  at once under `updated 2h ago · refreshing…` until the answer replaces it;
  a 503 keeps the rows as `Stub not refreshed`; a fetch whose body is still
  arriving when the picker closes is kept when it lands; reopening while a
  fetch is out asks nothing and takes its answer; a key saved through
  `/login` drops the file, and the fetch still out with the old key keeps
  nothing when it lands; and a picker left open with no key pressed shows
  its list passing a minute (`updated 1m ago · ctrl+r to refresh`). Each of
  the last four steps was checked against the code with the behaviour it
  guards taken out (no retire, no idle redraw, no waiting, a close that
  cancels) and fails there.
