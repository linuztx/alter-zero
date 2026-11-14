# Cached `/model` lists

Date: 2026-11-14

## Goal

`/model` opened on `Loading models…` every time. The picker fetched every
configured provider's list when it opened and kept nothing when it closed
(`docs/llm.md` *Known limitations* said so), so opening it meant waiting for
the slowest provider — a Cloud Code catalog, a ClinePass plan plus Cline's
catalog, an Ollama walk of one `/api/show` per model — to pick a model whose
name you already knew.

Now the picker opens on the lists it read last time, at once, and keeps them
current behind your back:

- every provider's list is **kept** once it has been fetched, in
  `~/.alter-zero/model-cache.json`, so it outlives the session;
- opening `/model` shows every kept list **at once**;
- a kept list **older than an hour**, and any local model server's list, is
  fetched again **behind** the rows on screen, which stay put until the new
  list lands;
- **Ctrl+R** fetches every list again, whatever its age.

## What the picker shows

```
  → anthropic/claude-3-haiku [openrouter]
    anthropic/claude-3.5-haiku [openrouter]
    …
    (1/412)   ·   updated 2h ago   ·   ctrl+r to refresh

    Model Name: Anthropic: Claude 3 Haiku
```

The counter line carries the lists' status, one note at a time, the most
pressing first (`loading more…` and `{provider} unavailable` were there
before; the rest are new):

| State | Beside the counter |
| --- | --- |
| a provider with nothing on screen is still being fetched | `loading more…` (dim) |
| every fetch still out refreshes a list already on screen | `refreshing…` (dim) |
| a provider failed and has nothing on screen | `{provider} unavailable` (red), its reason under the list |
| a refresh failed, and the kept list stays on screen | `couldn't refresh {provider}` (amber), its reason under the list |
| nothing out, nothing failed | `updated {age}` (dim) |

`ctrl+r to refresh` closes the line whenever no fetch is out. After a failure
it tells you how to try again.

The age is the **oldest** list's, because that is the one the claim has to
hold for: "updated 2h ago" says every row on screen was read at most two hours
ago. It is relative (`just now`, `3m ago`, `2h ago`, `1d ago`, the `/usage`
page's words), counted from the wall clock the boundary injects on every draw.
A picker left open keeps a slow frame (every 30 s) so the age doesn't freeze.

## Fresh, stale, and the refresh behind the rows

A kept list is **fresh** for an hour after it was fetched
(`MODEL_CACHE_FRESH_SECS`). After that it is **stale**: still shown at once,
and fetched again behind the rows. An hour, because model catalogs change over
days, not minutes. The window only keeps a reopened picker from refetching
every provider; a list older than that is refreshed out of sight the next time
the picker opens, and a newer one is a keystroke away.

A list stamped later than the clock now reads (a clock set back, or a file
copied from a machine whose clock runs ahead) is stale. Its age can't be
known, so it is read again.

A **local model server** — a provider whose key is optional, the servers
`/login` lists under *Sign in with a local model*: Ollama, LM Studio, a
llama.cpp or vLLM of your own (`docs/local-models.md`) — is never fresh. Its
list is fetched behind the rows on every open. It costs next to nothing (the
server is yours, usually on this machine), and its list changes exactly when
you pull or load a model, which is when you open `/model` to find it.

A refresh **replaces** that provider's rows when it lands. Models it no longer
lists go, new ones appear, and the highlight stays on the same model, the rule
the multi-provider merge already followed. A refresh that **fails** keeps the
old rows (a list an hour old beats no list) and says so in amber. A failed
fetch doesn't change the list's age, so the next open tries again.

The `Loading models…` rule in `docs/llm.md` holds through a refresh: while a
fetch is out, a search that matches nothing keeps saying `Loading models…`,
since the list being fetched may hold the very model being searched for.

## Ctrl+R

The picker types every printable key into its search, so the refresh can't be
a bare `r`: searching for `router` would refresh. Ctrl+R is free inside the
picker (the composer's reverse search can't be open under it), and it is the
reload key in browsers. It fetches every configured provider again. The
provider file is re-read first, as it is on open, so a provider added since
is fetched and a removed one leaves the list. While a fetch is still out the
key does nothing, the `/usage` page's `r` rule: asking again for a list that
is already on its way gains nothing.

## The file

`{config_home}/model-cache.json` holds one entry per provider id:

```json
{
  "providers": {
    "openrouter": {
      "source": "sha256:…",
      "fetched_at": 1763110000,
      "models": [
        {
          "id": "anthropic/claude-fable-5",
          "name": "Anthropic: Claude Fable 5",
          "reasoning": { "efforts": ["low", "medium", "high"], "can_disable": true },
          "vision": true,
          "context": 1000000
        }
      ]
    }
  }
}
```

A row holds what the picker shows and what a pick carries
(`Action::SelectModel`): the id, the friendly name when it differs from the id,
the reasoning ladder, image input, the context window and the speed tiers. The
ladder is stored as labels, so a hand-edited or future file still loads: a
label this build doesn't know is skipped, not fatal (`ThinkingSettings`'
rule).

It is a cache. Delete it and the next `/model` fetches, as before. A file that
can't be read, doesn't parse, or is larger than 16 MiB
(`MODEL_CACHE_MAX_BYTES`) reads as empty, and the next fetch writes it again.

The file is read when `/model` opens, and written when a fetch lands or
`/login` drops a list. Each write is a read-modify-write of the entries it
changes (the `tips.json` posture), so two sessions refreshing different
providers keep each other's lists. Nothing of
it stays in memory between opens: the picker's rows are the only copy, and
they go when it closes (`docs/memory.md`). The file decodes into typed
structs, never a `Value` tree.

### When a kept list isn't used

- **The provider now points somewhere else.** `source` is a sha256 fingerprint
  of where the list was read from: the listing's base, the chat base, the auth
  scheme, the wire format and the provider's own headers. An `OLLAMA_HOST`
  that names another server, or a `providers.toml` whose base URL moved,
  misses, so the picker never offers a model the server a pick would go to
  doesn't have. It is a fingerprint, not the URL itself, because a base URL
  can carry credentials (`https://user:pass@host/v1`) and a cache is no place
  for them.
- **The credential changed through `/login`.** A pasted key, a finished
  sign-in, or a local server's saved address or token drops the kept list of
  every provider that reads that variable. A different account can mean a
  different catalog (a Copilot Business seat lists models a Free seat
  doesn't), and signing in again is how a user recovers from a wrong list.
  The credential itself is deliberately **not** in the fingerprint: ChatGPT,
  Claude, Cline and Antigravity rotate the stored refresh token as it is used
  (`docs/chatgpt.md`), and a fingerprint over it would miss after every
  rotation.
- **The provider is no longer configured.** Only configured providers are
  shown, kept list or not.

## Fetches outlive the picker

The fetch used to be cancelled when the picker closed, because its list had
nowhere to go. Now it has one, the cache. With the list on screen at once,
picking a model takes a keystroke or two, so a refresh cancelled on close
would seldom finish and a slow provider's list would only ever get older. A
fetch therefore runs to its end, or until the app quits, whatever the picker
does, and its list is kept.

That needs one piece of bookkeeping, the **fetch ledger**
(`llm::models_cache::FetchLedger`, pure): one ticket for each provider fetch
that is out.

- A picker opened while a provider's fetch is still out **waits on it**
  instead of starting a second.
- A list is kept, and shown, only when it answers the provider's live ticket.
- `/login` saving a credential retires the provider's ticket, so a fetch made
  with the old credential can't write its list back after the save, or land
  in a picker opened since.

The decision of what to show, fetch or wait on is the pure `models_cache::plan`;
the boundary only gathers its inputs and carries it out.

## Alternatives considered

- **Always refresh behind the rows, with no freshness window.** The simplest
  rule, and a list is never stale for longer than one fetch. But every open
  refetches every provider: an Ollama walk is one request per model, a
  ChatGPT listing is the whole catalog. That cost is only worth paying where
  it is free and the list moves under the user's hand, so local model servers
  keep it.
- **Memory only.** The first open of each launch would still wait, and that is
  most opens, since `alter-zero` is started per task.
- **A long window, with no refresh behind it.** Fewer requests, but a catalog
  change would stay invisible until the window ran out, unless the user knew
  to press Ctrl+R.
- **A bare `r`, like `/usage`.** The picker's search takes every printable
  key; `/usage` has no search.

## Tests

- `llm::models_cache`: the file format round trip (every field of a row; a
  label this build doesn't know skipped rather than failing), freshness at the
  window's edge and for a clock set back, the local-server rule, the source
  fingerprint (each input moves it, the credential doesn't), the plan (what is
  shown, fetched and waited on), and the ledger.
- `app::tests::model_picker`: kept lists installed at open, a refresh
  replacing a provider's rows, a failed refresh keeping them, the highlight
  across a refresh, Ctrl+R, and Ctrl+R doing nothing while a fetch is out.
- `ui::tests::model_view`: the counter line in each state, and the amber
  reason rows.
- `smoke.sh` Phase 155: the real binary against a stub provider. A second
  `/model` lists the models without asking the stub, a restart reads them
  from disk, Ctrl+R asks again (once, however often it is pressed while the
  answer is out) and a new model appears, a refresh the stub refuses keeps
  the rows under an amber note, and `/login` saving the provider's key drops
  its kept list.
