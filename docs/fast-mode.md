# `/fast`, `/ultrafast`, … — codex's speed tiers as commands

> This is about **how fast** a ChatGPT model answers. What it *thinks*
> before answering — the Ctrl+T reasoning effort — is `docs/reasoning.md`,
> and the sign-in and wire format the whole thing rides on is
> `docs/chatgpt.md`.

Codex's **fast mode** is a *service tier* on the request: a model whose
ChatGPT listing names a `priority` tier can be asked for priority
processing, which the backend serves 1.5–2× faster at increased plan usage.
This document covers how the TUI learns which tiers a model offers, the
**per-tier commands** that switch them — one command per tier the listing
names, `/fast`, `/ultrafast`, whatever a record says, never a hardcoded row
— where the choice shows (the footer, after the thinking mode), how it
rides the request, and how it persists.

```
  ❯ ask something…
────────────────────────────────────────────────
  gpt-5.5 medium fast · ~/alter-zero                (footer: {model} {mode} {tier} · {cwd})
```

Every speed tier the active model's listing names is a slash command of
its own, named after the tier and listed right after `/model`. The
account's catalog today lists one tier, `Fast`, so the palette shows
`/fast`; a record listing `Ultrafast` too shows `/ultrafast` beside it, and
a tier the backend adds tomorrow is a command the day it is listed. Running
a tier's command selects it — a transient `Speed: fast — 1.5x speed,
increased usage` toast (`docs/toast.md`) repeating the record's own cost
statement, and the next request carries the tier; running the same command
again answers `Speed: standard` and the footer word goes away; another
tier's command switches straight over, with no standard step between. A
model that lists no tier at all — every provider but the ChatGPT backend
today, and the offline dummy — lists no tier command: `/fast` typed there
matches nothing (`No matching commands`), and Enter is swallowed like any
other miss rather than sent to the model as a message. The default is
**standard** for every model; nothing is billed at a tier's rate until
asked for.

## What codex does (findings)

Read out of `openai/codex` (`codex-rs`), the reference this ports:

- **The wire.** `ResponsesApiRequest.service_tier: Option<String>` —
  `Some("priority")` for fast mode, omitted for standard
  (`codex-api/src/common.rs`; `protocol::config_types::ServiceTier::Fast`
  has `request_value() == "priority"`). Beside it the client sends an
  `x-codex-routing-hint: model={model};tier={tier}` header on every request
  to the ChatGPT backend — `model={model}` alone when no tier is selected
  (`core/src/client.rs`, `build_routing_hint_header`).
- **The gate.** A model supports fast mode when its `/models` record's
  `service_tiers` array carries an entry with `id: "priority"`
  (`{"id": "priority", "name": "Fast", "description": "1.5x speed,
  increased usage"}`), or — the legacy marker — `additional_speed_tiers`
  contains `"fast"` (`protocol/src/openai_models.rs`,
  `ModelPreset::supports_fast_mode`). Some records list a second tier,
  `ultrafast`. `service_tier_for_request` drops a tier the model does not
  list, so an unsupported tier never reaches the wire.
- **The UI.** Each listed tier becomes a slash command named after it —
  `/fast`, `/ultrafast` — inserted right after `/model` and shown only when
  the current model lists it (`tui/src/bottom_pane/slash_commands.rs`).
  Running one toggles that tier (`chatwidget/service_tiers.rs`,
  `toggle_service_tier_from_ui`): on when off, back to the explicit
  `default` when on. The session header paints `fast` in magenta after the
  model name (`history_cell/session.rs`), and `/status` reports the spend by
  speed. A `toggle_fast_mode` keybinding exists with **no default key**.
- **Persistence.** The choice is written to `config.toml` as
  `service_tier = "fast"` (the request spelling `priority` is accepted too)
  and applies to every model that supports it; a model that does not simply
  runs standard, the configured value untouched.
- **Children.** A spawned agent inherits the root's tier only when its own
  model lists it (`agent/child_config.rs`); the Guardian review request
  always sends none.

## What we build

The same thing, over this crate's existing seams — every one of which the
Ctrl+T thinking mode already built, so fast mode is a second passenger on
each rather than a mechanism of its own:

| | thinking mode (`docs/reasoning.md`) | speed tier |
| --- | --- | --- |
| detected from | the `/models` record's effort ladder | the record's `service_tiers` |
| pure state | `ReasoningSupport` + `ThinkingMode` on `App::thinking` | `SpeedState` on `App::speed` |
| switched by | Ctrl+T (`Action::SetThinking`) | each listed tier's own command (`Action::SetSpeed`) |
| rides the request as | `reasoning: {effort}` | `service_tier: "priority"` |
| shown | after the model name | after the thinking mode |
| persisted | `config.json`'s `thinking` blob | `config.json`'s `speed` blob |
| rebinds | the *next* turn's backend | the *next* turn's backend |

### Detection: what the listing says

`llm::models::service_tiers_of` reads one record into `Vec<ServiceTier>` on
the `ModelEntry` — id, name and description, in the record's order — the
same per-record sniff pattern the reasoning, vision and context-window
fields use, and a branch only the ChatGPT backend's records take. With no
`service_tiers` list the legacy `additional_speed_tiers: ["fast"]` marker
still reads as the fast tier (`ServiceTier::fast()`, no description). Every
other provider's record lists nothing and reads as an **empty** list: a list
is what it lists, so unlike vision there is no "unknown" to carry — a model
with an empty list has no fast mode, full stop.

### The state and the toggle

`llm::service_tier` is the pure core:

- `ServiceTier { id, name, description }` — codex's `ModelServiceTier`,
  verbatim. `is_fast()` is true for the wire id `priority` or a name of
  `fast`; `label()` is the lowercased name (`fast`, `ultrafast`), never the
  bare id, since `priority` says nothing to a user who pressed `/fast`;
  `command_name()` is that label as **one palette token** — codex's own
  rule is `tier.name.to_lowercase()`, and a bare `/token` ends at whitespace
  (`app::command_query`), so a name a catalog could someday carry (`Super
  Fast`) is slugged: anything but letters, digits, `-` and `_` folds to
  `-`, runs collapse, the ends trim (`super-fast`), and a name that leaves
  nothing usable is no command at all.
- `SpeedState { tiers, tier }` — the listed tiers and the selected one's id
  (`None` = standard). `SpeedState::new(tiers, tier)` is `None` for an empty
  list (nothing to select) and drops a `tier` the list does not offer — a
  saved choice the model no longer lists degrades to standard rather than
  riding a request that would be refused, codex's `service_tier_for_request`
  rule. `toggle(id)` is what a tier's command runs — codex's
  `toggle_service_tier_from_ui`: **standard** when `id` is the selection,
  else `id`'s tier, and an `id` the model does not list changes nothing and
  answers the selection as it stands (total rather than panicking; the
  palette only ever offers listed tiers). The new selection is returned for
  the loop to rebind, persist and toast.

So `/fast` on a standard session is fast, `/fast` again is standard, and
`/ultrafast` on a fast session is ultrafast at once — each command is its
own switch, not a stop on a cycle, which is what keeps the footer honest
about what the next request carries whatever order the commands were run
in.

### The commands

There is no `/fast` in the static registry. `App::commands()` builds the
palette's rows for the session: `COMMANDS` (the built-ins, a `const`) with
one row per tier in `App::speed` spliced in **right after `/model`** — where
codex's `commands_for_input` inserts its `SlashCommandItem::ServiceTier`
entries, a tier being a fact about the model just switched to. Each row is
`SlashCommand::tier(&tier)`: named by `command_name()`, described by the
record's own cost statement (`1.5x speed, increased usage` — the price of
the speed said where it is bought, which is the half a user cannot see
coming) or, when the record gave none, `Toggle {label} mode`, and carrying
the tier as its effect, `CommandEffect::ServiceTier(tier)` — the one effect
no registry entry carries, since the rows come from the listing. A tier
whose name leaves no token, or whose name is a listed command's already,
gets no row: a built-in is never shadowed, `/model` being the door to
everything else. The rows are built on demand rather than stored, so they
can never disagree with the state the footer and the next request read; a
built-in's clone is a borrowed pointer copy (`SlashCommand`'s strings are
`Cow`s, which is what lets one type serve a `const` table and a per-session
row), so a keystroke's rebuild costs nothing.

Everything downstream reads that list: `matching_commands(&commands, query)`
is the filter the palette, ↑/↓ and Enter share, `/help` enumerates it — so
`/fast — 1.5x speed, increased usage` and `/ultrafast — …` list under
`/model` there too — and the palette band renders a tier row exactly as it
renders a built-in (`ui::command_menu_lines`). Running one is
`App::toggle_speed_tier`: the pure state toggles at once and the loop is
handed `Action::SetSpeed(Option<ServiceTier>)`. It works **mid-turn**
exactly as Ctrl+T does — the pure state moves now, and only the *next*
turn's backend is rebound (the running turn streams on its own thread,
untouched).

The loop's `Session::set_speed` does the three things Ctrl+T's `set_thinking`
does: `ModelSession::rebind_speed` rebuilds the backend with the tier in its
`ModelConfig`, `persist` writes the choice beside the model selection, and
the toast is presented from the loop (so the boundary's expiry timer arms —
`docs/toast.md`). The toast repeats the backend's own description of the
tier — `Speed: fast — 1.5x speed, increased usage` — because the price of
the speed is worth saying where it is bought; a tier the record described
with nothing reads `Speed: fast`, and standard reads `Speed: standard`.

### The wire

`ModelConfig::service_tier` (from `Selection::service_tier`) carries the
selected tier's id; both OpenAI wires send it as the top-level
`service_tier` field:

| wire | request |
| --- | --- |
| Responses (`docs/chatgpt.md`) | `"service_tier": "priority"` beside `reasoning`, `store`, `prompt_cache_key` |
| Chat Completions | `"service_tier": "priority"` beside `prompt_cache_key`, before the provider `extra_body` merge |
| standard | no field at all — the backend's own default, never a `"default"` it would have to know |

The Chat Completions wire takes the field because OpenAI's own API does;
since a tier is only ever set to one the model's *record* listed, and only
the ChatGPT backend's records list any, no other provider ever sees it. The
Anthropic and Ollama wires ignore the config field entirely.

On the ChatGPT backend the request also carries codex's routing hint:
`chatgpt::routing_hint(model, tier)` renders `model={model}` on every
request and `;tier={tier}` when a tier is selected, attached by
`openai::chatgpt_request_headers` beside the `session_id`/`conversation_id`
cache-affinity headers and gated the same way — a pasted-key provider never
sees a codex header.

Three requests deliberately run at **standard** whatever the session's tier:

- a subagent whose definition **pins another model** (`OpenAiClient::with_model`
  drops the tier with the thinking mode and the vision verdict — all three
  were detected for the model being replaced, and a tier the pinned model
  does not list is a request the backend may refuse; codex's
  `apply_spawn_agent_service_tier` makes the same cut). An inheriting
  subagent runs on the session's config and so at the session's speed.
- the **auto-mode classifier** (`SafetyClassifier::new` clears it beside
  the thinking mode): a verdict is a few tokens, priority processing bills
  plan usage, and `ALTER_ZERO_CLASSIFIER_MODEL` may name a model that lists
  no tier at all. Codex's Guardian request sends none for the same reason.
- nothing else: the `/compact` summary rides the tier like any other turn
  of the same model.

### Where the state comes from

- **`/model` switch** — the picked `ModelEntry` carries its
  `service_tiers`, riding `Action::SelectModel` so a successful switch seeds
  the tier commands with no refetch. The **current selection carries across** when the
  new model lists the same tier (codex keeps its configured tier for any
  model that supports it) and falls back to standard otherwise.
- **Startup, saved selection** — `config.json` persists a `speed` blob
  (`llm::settings::SpeedSettings`: the listed tiers and the selected id)
  beside the `thinking` blob, in the working directory's own entry
  (`docs/per-directory-state.md`), so startup seeds `App::set_speed` from
  the file and the tier rides the first request. The **empty** blob (`{}`)
  is the marker for a model known to list no tier — the
  `ThinkingSettings::unsupported` idea — so startup never re-probes for it.
- **Startup, support unknown** — a file from before this feature has no
  blob at all, and the existing capability probe (`docs/reasoning.md`)
  fires for that as it does for an unknown thinking or vision state: one
  background `/models` fetch, the active model's record read out of it, the
  tiers adopted and persisted. A selected tier the record no longer lists
  is dropped and the backend rebound so the next request stops carrying it.
  As everywhere, an env-overridden selection never writes back.
- **Until the probe answers, the tiers are a third state — unknown — and
  `persist` treats it as one.** `ModelSession::speed_known` says whether the
  tiers were ever learned first-hand (the saved blob, a switch, the probe),
  and `SpeedSettings::recorded(known, state)` writes **no blob at all**
  while they were not: a Ctrl+T that persists in the seconds before the
  probe answers, or after a probe that failed, would otherwise write the
  empty blob — the marker for a model *known* to list none — and the next
  launch would read it as a reason never to probe again, leaving `/fast`
  dead for that model in that directory with nothing ever saying why. The
  review of a parallel implementation demonstrated exactly that sequence
  against this code, which is why the rule has a pure test of its own.

`/clear` and `/resume` never touch the state (the model did not change); a
switch to a model listing no tier clears it (`App::set_speed(None)`), which
blanks the footer's tier word and takes the tier rows out of the palette.

## Verified live

Run against a signed-in ChatGPT account (`tests/live_caching.rs`,
`live_chatgpt_fast_mode_is_listed_and_a_priority_request_is_served`, the
ignored live test): every one of the account's seven models —
`codex-auto-review`, `gpt-5.5`, `gpt-5.6-luna`, `gpt-5.6-sol`,
`gpt-5.6-terra`, `gpt-6-astra`, `gpt-reserve` — listed exactly one tier,
`Fast=priority`, described `1.5x speed, increased usage` (`2x` on
`gpt-6-astra`); no record listed `ultrafast` on that account. The same
one-word prompt answered both at standard and with `service_tier:
"priority"` + the routing hint, each with a usage frame (`input: 20,
output: 5`). One sample says nothing about speed and is not measured; what
it proves is that the field and the header are accepted and served.

Re-run on 2026-09-17, after the provider's rename (`docs/chatgpt.md`) and
with the per-tier commands in place, under `CHATGPT_CODEX_REFRESH_TOKEN`:
the same seven models, each listing exactly one tier — `Fast=priority`,
`1.5x speed, increased usage` (`2x` on `gpt-6-astra`) — and the standard
and the priority request each answered with a usage frame. The real binary,
driven in tmux against the account with `ALTER_ZERO_MODEL=gpt-5.5`, listed
`/fast` right after `/model` wearing `1.5x speed, increased usage`, toasted
`Speed: fast — 1.5x speed, increased usage` with `gpt-5.5 medium fast` in
the footer on the first run and `Speed: standard` on the second, and wrote
the rotated refresh token back under the new variable. No live record lists
a second tier yet, which is why the second row is exercised by `smoke.sh`
Phase 117's stub-served two-tier catalog rather than live.

## Testing

The tier parse (a listed catalog, the legacy marker, a nameless or id-less
entry, a record listing none), the state and its toggle (select, the same
command back to standard, a straight switch between two tiers, an unlisted
id changing nothing, the stale-choice fallback), the command name (codex's
lowercased name, the slug for a name that is not one token, a name that
leaves none), the two payloads and the routing hint, the pinned-model and
classifier drops, the settings round-trip (the blob, the empty marker, a
legacy file, the three-state `recorded` rule), the palette (no `/fast` in
the registry, one row per listed tier right after `/model` with the
record's description, the generic description for a tier described with
nothing, no row for an unusable or shadowing name, the rows gone with the
state, `/help` listing them), the key path (`/ultrafast` Enter →
`SetSpeed`, the same command back to standard, `/fast` then `/ultrafast`
switching straight over, mid-turn, an unmatched `/fast` swallowed rather
than sent), the band's rendering of a tier row, and the footer wearing any
tier's name are unit-tested in their modules. The boundary wiring — probe,
persistence, backend rebinds — is exercised by the live test and by running
the app; `smoke.sh` Phase 117 drives both sides in the real binary: the
dummy showing no tier row (`No matching commands`, Enter swallowed, no
footer word, no `Speed:` toast), then a ChatGPT Codex model whose listing
— served by a local stub standing in for the token mint and the `/models`
catalog — names two tiers, `/fast` and `/ultrafast` rows after `/model`
wearing the record's descriptions, each toggled with its toast and its
footer word, one listing fetch on the stub's log. Phase 1 asserts the
palette's row cap by counting rows rather than naming whichever command
sits eighth, so the tier rows landing before `/login` break nothing.

## Known limitations (v1)

- A record's `default_service_tier` is not honoured (every record ships
  `null` today, and codex treats a configured choice as outranking it
  anyway); the default here is always standard.
- The tier is per selection, not global: switching to a model that lists it
  carries the choice over, but a fresh directory starts from the last
  selection made anywhere, exactly as the thinking mode does.
- Nothing measures the speed. The toast repeats the backend's own claim and
  the plan usage it costs; whether a given request was faster is the
  backend's business.
