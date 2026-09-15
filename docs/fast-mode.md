# `/fast` — service tiers

Some models can run a request in a **faster lane**. OpenAI's ChatGPT backend
calls it the **Fast** service tier and publishes it per model; codex exposes it
as Fast mode. This is that, ported: a `/fast` command, a `fast` marker in the
footer, and one extra field on the request.

```
  ❯ ask something…
────────────────────────────────────────────────
  gpt-5.6-sol high fast · ~/alter-zero        (footer: {model} {mode} {lane} · {cwd})
```

`/fast` toggles it and raises a `Fast mode: on (fast service tier)` /
`Fast mode: off` toast; the next request carries the lane. On a model that
offers none, `/fast` raises `{model} does not offer a fast service tier`
instead — the command is never silently dead, the same contract Ctrl+T keeps
on a model with no reasoning (`docs/reasoning.md`).

It works **mid-turn**, like `/model`: the running turn streams on its own
thread with the config it was built from, so a toggle only rebinds the *next*
turn's backend.

## The wire: the id is not the name

The one detail every naive port gets wrong. The tier is *called* `Fast`, is
written `fast` in codex's config, and goes on the wire as **`priority`**:

```jsonc
{ "model": "gpt-5.6-sol", "input": [ … ], "service_tier": "priority" }
```

So the **id** is what a request and a saved choice carry, and the **name** is
what the user reads. `llm::service_tier` keeps both (`FAST_ID` / `FAST_NAME`)
and never conflates them.

The field is **omitted entirely** for the standard lane — not sent as `null`,
which this backend rejects exactly as it rejects a tier the model does not
have. Only the Responses wire carries it (`docs/chatgpt.md`), because that is
the only wire whose catalog publishes tiers; nothing else changes shape.

### `default` is a choice, not a tier

A model's record may name a `default_service_tier` the backend applies when
the request carries none. That makes "never chose" and "chose standard" two
different states, so there is a third value: the **`DEFAULT_ID` sentinel**
(`"default"`), which is never sent and whose whole job is to *suppress* the
catalog default.

`ServiceTierSupport::for_request` is the one place all three rules live:

| chosen | sent |
| --- | --- |
| a lane the model offers | that lane's id |
| `default` (the sentinel) | nothing — and the catalog default does not apply |
| a lane the model does **not** offer | nothing (a stale choice degrades, it never 400s the turn) |
| nothing chosen | the catalog default, when the model really lists it |

That last filter matters: a `default_service_tier` naming a lane absent from
the same record's `service_tiers` is ignored rather than sent.

## Detection: what the catalog says

The lanes are a fact about a *model*, read per record by the pure
`llm::models`' `service_tier_support_of` — the same shape-sniffing pattern
OpenRouter, Venice, Copilot and Anthropic already share, so no other
provider's list is touched. Two spellings, in the order the backend
deprecated them:

- **`service_tiers`** — the live shape, `[{id, name, description}]`. The id is
  the wire value, the name the label.
- **`additional_speed_tiers`** — an older array of bare names (`["fast"]`)
  with no id of its own, so the name stands in for both. Read only when
  `service_tiers` is absent or empty: it alone carries the id the API takes.

A record naming neither — every provider but ChatGPT — yields `None`, and
`/fast` has nothing to switch.

A model can offer **more than one** lane: `gpt-5.6-sol` lists `priority`
("Fast") beside `ultrafast` ("Ultrafast"), a different and faster tier. So the
fast lane is found by *id*, not by position and not by "the name contains
fast" — both of which pick the wrong row here. The other lanes are parsed and
selectable all the same; only the fast one has a command in front of it.

## Where the state lives

`ModelEntry::tiers` (the catalog fact) → `App::service_tier` (the
`ThinkingState` twin: the support plus the chosen id) → `Selection` /
`ModelConfig::service_tier`, which is the **already-resolved** lane, so the
payload builder sends it verbatim or not at all.

It persists **per working directory** beside the `/model` selection
(`config.json`'s `ServiceTierSettings`, `ThinkingSettings`' twin —
`docs/per-directory-state.md`), recording the lanes *and* the choice, so
`/fast` and the footer marker are live from the first frame rather than
waiting on the startup capability probe. A model known to publish no lanes
records the `unsupported` marker, which is what keeps a relaunch from
re-probing for them.

Three paths seed it, exactly as they seed the thinking mode:

- **bootstrap** — the blob `config.json` recorded for this exact selection.
- **a `/model` switch** — the picked row's own lanes, seeded **unchosen**, so
  the new model's catalog default applies rather than a lane picked for the
  old one.
- **the capability probe** — the record's lanes, re-filtering the choice
  already made. The probe is handed the **raw** choice rather than reading the
  resolved one: an explicit standard lane and never having chosen are both
  `None` once resolved, and confusing them would let a catalog default apply
  over a user who deliberately asked for standard — turning fast silently back
  on at every launch.

Writing the blob has a third state for the same reason reading it does.
"Unknown" (a probe still pending, or one that failed) must write *nothing*,
while "the record says this model has no lanes" writes the `unsupported`
marker — and that marker is what stops the next launch probing. Collapsing
them would let a Ctrl+T pressed before the probe answers record `unsupported`
for a model that does have a fast lane, and `/fast` would be dead there for
good. So `ModelSession::persist` takes `Option<Option<&Tiers>>`: only the
probe and a `/model` switch, which learn the lanes first-hand, ever record a
definitive answer.

## Why a command and not a keybinding

Codex ships its `toggle_fast_mode` binding **unbound** by default; its real
surface is a palette row generated under `/model` from the catalog. This does
the same with a static `/fast` entry placed right after `/model` — the lane is
a property of the model, so it belongs beside it — and every Ctrl-letter worth
having is already spoken for (`docs/shortcuts.md`).

## Files

- `src/llm/service_tier.rs` — the pure core: the ids, `ServiceTier`,
  `ServiceTierSupport` and its four rules.
- `src/llm/models.rs` — `service_tier_support_of`, the per-record sniff.
- `src/llm/responses.rs` — the one field on the request.
- `src/llm/settings.rs` — `ServiceTierSettings`, the persisted blob.
- `src/app/mod.rs` — `set_service_tier`, `toggle_service_tier`,
  `service_tier_label`, `service_tier_for_request`.
- `src/app/commands.rs` — the `/fast` registry entry.
- `src/ui/footer.rs` — the marker.
- `src/tui/models.rs` — `rebind_service_tier`, the persist and probe paths.
- `scripts/smoke/phases/117-fastmode.sh` — the palette row and the
  explanatory toast, driven against the offline dummy.
