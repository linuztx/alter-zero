# `/fast` — codex's fast mode for ChatGPT models

> This is about **how fast** a ChatGPT model answers. What it *thinks*
> before answering — the Ctrl+T reasoning effort — is `docs/reasoning.md`,
> and the sign-in and wire format the whole thing rides on is
> `docs/chatgpt.md`.

Codex's **fast mode** is a *service tier* on the request: a model whose
ChatGPT listing names a `priority` tier can be asked for priority
processing, which the backend serves 1.5–2× faster at increased plan usage.
This document covers how the TUI learns which models offer it, the `/fast`
command that switches it, where the choice shows (the footer, after the
thinking mode), how it rides the request, and how it persists.

```
  ❯ ask something…
────────────────────────────────────────────────
  gpt-5.5 medium fast · ~/alter-zero                (footer: {model} {mode} {tier} · {cwd})
```

Running `/fast` on a model that lists the tier raises a transient
`Speed: fast — 1.5x speed, increased usage` toast (`docs/toast.md`) and the
next request carries the tier; running it again answers `Speed: standard`
and the field goes away. On a model that lists no tier at all — every
provider but the ChatGPT backend today, and the offline dummy — `/fast`
raises `{model} does not support fast mode` instead: the command is never
silently dead, Ctrl+T's rule for a non-reasoner. The default is **standard**
for every model; nothing is billed at the fast rate until asked for.

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
| switched by | Ctrl+T (`Action::SetThinking`) | `/fast` (`Action::SetSpeed`) |
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

### The state and the cycle

`llm::service_tier` is the pure core:

- `ServiceTier { id, name, description }` — codex's `ModelServiceTier`,
  verbatim. `is_fast()` is true for the wire id `priority` or a name of
  `fast`; `label()` is the lowercased name (`fast`, `ultrafast`), never the
  bare id, since `priority` says nothing to a user who pressed `/fast`.
- `SpeedState { tiers, tier }` — the listed tiers and the selected one's id
  (`None` = standard). `SpeedState::new(tiers, tier)` is `None` for an empty
  list (nothing to select) and drops a `tier` the list does not offer — a
  saved choice the model no longer lists degrades to standard rather than
  riding a request that would be refused, codex's `service_tier_for_request`
  rule. `next`/`advance` step the cycle **standard → the first listed tier →
  each next one → standard**.

With one listed tier — the usual catalog — that cycle *is* codex's toggle.
With two (`gpt-5.6-sol` lists `priority` and `ultrafast`) codex grows a
second command; this palette is a static registry, so `/fast` reaches the
second tier on its next press instead, and the toast and footer name which
one is on. The divergence is deliberate and small: the footer never lies
about which tier a request carries, and a third press is standard again.

### The command

`/fast` is a static `COMMANDS` entry right after `/model` (where codex
inserts its tier commands), described as `Toggle fast mode (faster replies,
more usage)` — the cost in the same breath as the speed, since that is the
half a user cannot see coming. Its effect is `App::cycle_speed`: with a
`SpeedState` the pure state advances at once and the loop is handed
`Action::SetSpeed(Option<ServiceTier>)`; without one the command answers
with the explanatory toast itself. It works **mid-turn** exactly as Ctrl+T
does — the pure state moves now, and only the *next* turn's backend is
rebound (the running turn streams on its own thread, untouched).

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
  `/fast` with no refetch. The **current selection carries across** when the
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
blanks the footer's tier word.

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

## Testing

The tier parse (a listed catalog, the legacy marker, a nameless or id-less
entry, a record listing none), the state and its cycle (the toggle, the
multi-tier walk, the stale-choice fallback), the two payloads and the
routing hint, the pinned-model and classifier drops, the settings
round-trip (the blob, the empty marker, a legacy file, the three-state
`recorded` rule), the `/fast` grammar (the palette row, the cycle, the
unsupported toast, mid-turn), and the footer rendering are unit-tested in
their modules. The boundary wiring — probe, persistence, backend rebinds —
is exercised by the live test and by running the app; `smoke.sh` Phase 117
drives the palette row and the unsupported toast in the real binary, and
Phase 1 asserts the palette's row cap by counting rows rather than naming
whichever command sits eighth, so the next command inserted before `/login`
breaks nothing.

## Known limitations (v1)

- `ultrafast` (and any further tier a record lists) is reached by cycling
  `/fast`, not by a command of its own. Codex's per-tier commands need a
  dynamic palette; the cycle names its stop in the toast and the footer.
- A record's `default_service_tier` is not honoured (every record ships
  `null` today, and codex treats a configured choice as outranking it
  anyway); the default here is always standard.
- The tier is per selection, not global: switching to a model that lists it
  carries the choice over, but a fresh directory starts from the last
  selection made anywhere, exactly as the thinking mode does.
- Nothing measures the speed. The toast repeats the backend's own claim and
  the plan usage it costs; whether a given request was faster is the
  backend's business.
