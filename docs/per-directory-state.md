# Per-directory state — `/model`, `/settings`, `/mascot` and `/spinner` remember the directory

The model you pick and the knobs you set are facts about a **project**, not
about you: the repo that needs a frontier model beside the scratch directory
that runs a cheap local one, the project whose formatter hooks you want beside
the clone you have never audited. Until now both files were one blob for the
whole user — a `/model` switch in one terminal changed what every other
directory started with, and turning checkpoints off for a huge tree turned
them off everywhere. `config.json` and `settings.json` are keyed by working
directory now, the way `permissions.json` and `skills.json` already were —
and so are `mascot.json` and `spinner.json`, the two **looks**
(`docs/mascot.md`, `docs/spinner.md`): the banner a project wears is as much
its own as the model it runs, and a `/mascot` picked in one terminal used to
redraw every other project's banner at its next launch.

## The rule

Every file here keeps a **`projects` map keyed by the cwd's absolute path**,
read and written as a **read-modify-write** (re-read the file, replace this
directory's entry, write it back), so two sessions in two directories never
clobber each other. The top level of each file keeps its old shape, so a file
written before this change still loads — and the two original files answer
"what does a directory I have never launched in start with?" differently, on
purpose (the two look files take `config.json`'s side of the table, entry for
entry — *`mascot.json` and `spinner.json`* below):

| | `config.json` (`/model`) | `settings.json` (`/settings`) |
|---|---|---|
| a directory's own entry | `projects[cwd]` | `projects[cwd]` |
| a directory with no entry starts from | the **last** selection made anywhere — the top-level fields | the **seed** — the top-level keys, which are also the pre-change file |
| when a directory gets its entry | **at its first launch**: the last selection is pinned as its own right then | **at its first cycled knob**: the seed copied in with that one key moved |
| what moves the top level | every `/model` switch (it *is* the last selection) | nothing the app does — edit it by hand to change what a new directory starts with |
| the environment (`ALTER_ZERO_*`) | wins for the run, never written | wins for the run, never written |

The asymmetry follows from what the top level *is*. The model's "last
selection" moves with every switch, so a directory that merely *followed* it
would change under you whenever you picked something elsewhere — the very
coupling this removes — which is why a new directory **pins** its copy at the
first launch and is independent from then on. The settings' seed never moves,
so a directory without an entry cannot drift; its entry can wait until it
actually changes something, and the file stays a plain diff until then.

The model has a **third axis** the other files don't: the session
(`docs/session-model.md`). The directory's entry is what a *new* session
starts on, and nothing more — each conversation's rollout records the model
it actually runs on (an append-only `model` record, `ModelSelection`-shaped
like the entry itself, written with the file and on every `/model` pick,
Ctrl+T cycle and probe answer), and `/resume`, `--resume` and `--continue`
bring that model back without touching the entry. So two instances in one
directory each keep, and each resume, their own model; the entry only ever
moves on a `/model` pick.

"A directory" is the process cwd exactly as `permissions.json` keys it
(`Session::cwd`, not the git root): `repo/` and `repo/src` are two entries.

## `config.json`

```json
{
  "provider": "openrouter",
  "model": "openai/gpt-4o-mini",
  "thinking": { "supported": false },
  "speed": {},
  "projects": {
    "/home/user/work/api": {
      "provider": "a0_venice",
      "model": "llama-3.3-70b",
      "vision": false,
      "context": 131072
    }
  }
}
```

`llm::settings::Settings` is the pure format. The top-level fields — the pair
plus the model's `thinking` blob, `vision`, `context` and the `speed` blob
(`docs/reasoning.md`, `docs/tools.md`, `docs/compact.md`,
`docs/fast-mode.md` — the listed speed tiers and the `/fast` choice, `{}`
being the marker for a model known to list none) — are the last selection
made anywhere; each `projects` entry is a `ModelSelection`, the same six
fields with the pair required. Five pure operations, each
unit-tested:

- `selection_for(dir)` — the directory's entry, else the last selection.
- `adopt(dir)` — pin the last selection as `dir`'s entry when it has none;
  `true` when that changed the file (the boundary writes only then).
- `record(dir, sel)` — a `/model` choice made in `dir`: its entry **and** the
  last selection.
- `record_capabilities(dir, sel)` — what Ctrl+T or the startup probe learned
  about the model `dir` records: replaces the entry only when it names that
  same pair (so an env-overridden selection never writes back), and moves
  the last selection with it only when *it* is that pair too — a new
  directory then seeds without a probe.
- `last()` / `project(dir)` — the two halves, read back.

At the boundary (`tui::config`, `tui::models`): `ModelSession::resolve`
calls `adopt_selection` (load → `adopt` → write if changed → the entry), and
everything downstream — the env-vs-saved precedence, `selection_is_saved`,
the thinking/vision/context seeds, `persisted_selection` — reads that entry
exactly as it used to read the flat file. `switch_to` writes through
`save_selection` (`record`), `persist` through `save_capabilities`
(`record_capabilities`); both are read-modify-writes. `ModelSession::project`
is the key. A resumed conversation then overrides the entry with its own
recorded model through `ModelSession::restore` — a switch minus the write,
so the entry keeps meaning "what a new session here starts on"
(`docs/session-model.md`); `persisted_selection` still names the entry's
pair, which is what keeps a Ctrl+T in a session running another model from
writing that model over the entry.

## `settings.json`

```json
{
  "hide_thinking": true,
  "projects": {
    "/home/user/work/api": { "hide_thinking": true, "error_retry": 5 },
    "/home/user/work/huge-monorepo": { "hide_thinking": true, "checkpoints": true }
  }
}
```

`settings::SettingsFile` is the pure format: `seed` (the flattened top-level
keys) over `projects`, each a `SessionSettings`. An entry is a **whole blob**
— a diff from the *defaults*, like the seed itself — never a layer over the
seed. The diff format could not express layering: a `true` left at its
default is indistinguishable from one deliberately chosen, so a directory
whose seed says `error_retry: 5` would have no way to say "back to 3". That
is also why an entry is **kept even when it equals the defaults** — dropping
it would let the seed back in, and a directory that cycled a knob back to
its default has chosen that default.

- `settings_for(dir)` — the entry, else the seed (availability is a host
  fact the boundary injects afterwards).
- `record_value(dir, key, live)` — take the entry (else the seed), move
  across **only `key`'s value** from the live blob
  (`SessionSettings::copy_value`), keep it as the entry.

That second operation is the write path (`tui::config::save_setting`, from
`Session::apply_setting`), and it is what keeps an `ALTER_ZERO_*` override
from sticking: the live blob carries the environment, but only the cycled
key crosses. The re-read file is the base now — the session no longer keeps
a `saved_settings` copy of its own, since the file's entry for this directory
*is* that copy, and re-reading it also keeps what a second session in the
same directory saved meanwhile.

## `mascot.json` and `spinner.json`

```json
{
  "mascot": "sprout",
  "projects": {
    "/home/user/work/api": { "mascot": "bloom" }
  }
}
```

The same shape under the `spinner` key for `spinner.json`. Each is
`config.json`'s model exactly — a look is a preference nobody hand-edits a
seed for, so a new directory should start from *what you last chose*, and it
must then be independent of what you choose next: the top level is the
**last** choice made anywhere (and exactly the one-value file the app wrote
before looks were per directory, so an old file still loads and seeds every
directory), a directory launched in for the first time **pins** the last as
its own entry at that launch, and a choice made in a directory is the entry
*and* the last. An entry is the same shape as the top level, which is what
lets one parser read both.

The pure format is one generic, `app::LookFile<T>` over the `app::Look`
trait (`KEY` — the JSON key, `name`, `from_name` — implemented by `Mascot`
and `Spinner`), because the two catalogs are twins by design;
`app::MascotFile` and `app::SpinnerFile` are its instances. Its operations
mirror `config.json`'s, each unit-tested:

- `choice_for(dir)` — the directory's entry, else the last choice.
- `project(dir)` — the entry alone.
- `adopt(dir)` — pin the last as `dir`'s entry when it has none; `true`
  when that changed the file (the boundary writes only then).
- `record(dir, look)` — a choice made in `dir`: its entry **and** the last.
- `parse` / `to_json` — lenient in (a corrupt file reads as nothing chosen,
  an unknown name costs only that one value), pretty out with the choice key
  first, so a file with no entries is byte-for-byte the old one-value file.

At the boundary (`tui::config`): `adopt_look::<T>(path, cwd)` at bootstrap
(load → `adopt` → write if changed → `choice_for`), before the first frame
commits the banner; `save_look(path, cwd, look)` from the two pickers'
Enter (`tui::mascot::Session::select_mascot`,
`tui::spinner::Session::select_spinner`) — both read-modify-writes, both
best-effort, a `None` path (no config home) disabling persistence and nothing
else.

### Hooks and checkpoints are off until a directory turns them on

Both defaults flipped from `true` to `false` with this change. Each runs
*code* on the user's behalf — a `hooks.json` handler around every tool call,
a whole-cwd `git add -A` before the first frame — and neither is something
you want on in a directory you did not choose it for: a hooks file written
for one project fires in every project, and a checkpoint store for a tree you
opened once to read a file costs seconds at startup and a copy of the tree
under `~/.alter-zero`. With settings per directory, opting **in** is one
`/settings` cycle in the directory that wants it, recorded as
`"hooks": true` / `"checkpoints": true` in that directory's entry (the diff
predicates flipped with the defaults, so the file still records only what
changed).

Two consequences at the boundary:

- **`ALTER_ZERO_HOOKS` seeds the Hooks row** (`apply_setting_overrides`, the
  `ALTER_ZERO_TOOLS` pattern) instead of being ANDed over it. The old gate
  could only ever turn hooks *off*; with the row off by default the variable
  must be able to turn them on for a run — `smoke.sh`'s hooks phases set
  `ALTER_ZERO_HOOKS=1` the way the checkpoint phases set
  `ALTER_ZERO_CHECKPOINTS=1`. Like every override it wins for the run and is
  never saved.
- The checkpoint refusal toast (`Checkpoints off — …`) is silent by default,
  since it was only ever raised when the user had asked for checkpoints; a
  directory that turned them on still hears why it did not get them.

## What is *not* per directory

- **`.env`** — a key is the user's, not a project's; `/login` is unchanged.
- **The rollout's `model` record** — the model a *conversation* runs on
  (`docs/session-model.md`): `config.json`'s entry seeds a new session, the
  rollout carries the session's own from then on, and a resume restores it
  rather than the entry.
- **`theme.json`** — the colours are the user's: a theme is matched to the
  terminal the user sits at, not to a project (`docs/theme.md`).
- **`permissions.json`**, **`skills.json`** — already per project; unchanged.
- **`telemetry.json`** — the anonymous daily ping's switch and install id
  (`docs/telemetry.md`). An opt-out that applied only to the directory you
  happened to be in would be a surprise, so the `/settings` **Telemetry** row
  is the one knob that persists per *user*: `SessionSettings::telemetry` is
  `#[serde(skip)]`, `copy_value` never moves it, and the boundary seeds it
  from — and writes it back to — `telemetry.json` alone.
- **`update.json`** — the once-a-day update check's switch and record
  (`docs/update.md`): the same per-*user* rule for the same reason, with
  `SessionSettings::update_check` `#[serde(skip)]` beside `telemetry`.
- The **project-level `.alter-zero/`** layer (`docs/project-config.md`) is
  a different axis: files *inside* the project, behind `/trust`. Both files
  here live in the user's config home, keyed by directory, and need no trust
  gate — nothing in them executes.

## Testing

The pure formats are unit-tested (`src/llm/settings.rs`, `src/settings/tests.rs`:
the compat of a flat file, the pin, the read-modify-write, the whole-blob
rule, the new defaults; `src/app/tests/mascot.rs` and `spinner.rs` for the
two looks: the old one-value file as the last, the pin-once, the
entry-and-last record, the byte-identical no-entry shape, the lenient
parse). `scripts/smoke.sh` Phase 109 drives the boundary end to end against
one config home from two directories: a knob cycled in the first stays
there, the second starts at the defaults (hooks and checkpoints off), and a
pre-seeded last model is pinned per directory; Phase 114 drives the two
looks the same way — a mascot and a spinner chosen in the first directory
are its own and the last, the second directory's first launch pins that
last and its own choices never move the first's, and a third directory
takes the new last; Phase 118 drives the session axis — two instances in
one directory on two models, each resuming on its own while a fresh launch
takes the entry (`docs/session-model.md`). The live `/model` write is
exercised against a real provider in the tmux run described in the commit.
