# `/settings` — the session settings menu

Every knob this app has was, until now, an environment variable you had to know
about before launch: `ALTER_ZERO_SHOW_THINKING`, `ALTER_ZERO_TOOLS`,
`ALTER_ZERO_CHECKPOINTS`, `ALTER_ZERO_TEMPERATURE`,
`ALTER_ZERO_PROJECT_DOC_MAX_BYTES`, plus a hard-coded `retry::MAX_RETRIES` and an
always-on auto-compaction. `/settings` makes that inventory **visible and
changeable mid-session**, in the shape `/model` and `/login` already established:
an inline picker that replaces the composer, type-to-search, and one keystroke
per change.

```
────────────────────────────────────────────────────────────────────────────

  ❯

→ Hide thinking           false
  Error retry             3
  Tools                   true
  Permission mode         manual
  ...
  (1/8)

  Hide the model's chain-of-thought…

  Type to search · Enter/Space to change · Esc to cancel

────────────────────────────────────────────────────────────────────────────
```

## The settings

Eight rows, each one a knob the running session actually reads. Every value
**cycles** — there is no free-text field anywhere, so Enter and Space mean the
same thing on every row and the menu never needs an edit mode.

| Setting | Values | What it changes |
|---|---|---|
| **Hide thinking** | `false` / `true` | Whether the model's chain-of-thought streams in the live `● Thinking…` cell and collapses into a `Thought for …` line (`docs/thinking-stream.md`). `true` restores the counted-and-dropped behaviour. Seeded from `ALTER_ZERO_SHOW_THINKING`. |
| **Error retry** | `0` / `1` / `2` / `3` / `5` / `10` | How many times a failed request is retried before the error surfaces (`llm::retry`, the `retrying n/N` status). Was the fixed `MAX_RETRIES = 3`. |
| **Tools** | `true` / `false` | Whether `bash`/`read`/`write`/`edit`/`agent` are offered to the model at all (`docs/tools.md`). Seeded from `ALTER_ZERO_TOOLS`. |
| **Permission mode** | `manual` / `edit` / `auto` / `master` | The same posture Ctrl+A cycles (`docs/permissions.md`) — the row is a second door onto one state, not a copy of it. |
| **Checkpoints** | `true` / `false` | Per-turn working-directory snapshots (`docs/checkpoint.md`). Seeded from `ALTER_ZERO_CHECKPOINTS`; forced to `false`, unchangeably, when the store can't run at all (no git, no config home, or a cwd the feature refuses — see *Unavailable settings*). |
| **Auto compact** | `true` / `false` | Whether the loop runs the summarization turn on its own past 90 % of the context window (`docs/compact.md`). `/compact` by hand is unaffected. |
| **Project docs** | `true` / `false` | Whether the project's `AGENTS.md` files are re-read each turn into the context's leading user entry (`docs/project-doc.md`). Seeded from `ALTER_ZERO_PROJECT_DOC_MAX_BYTES=0`. |
| **Temperature** | `default` / `0.0` / `0.3` / `0.5` / `0.7` / `1.0` | The sampling temperature every request carries; `default` sends none and leaves it to the provider. Seeded from `ALTER_ZERO_TEMPERATURE`. |

### Unavailable settings

A knob can be **unavailable** rather than merely off: checkpoints need a `git`
binary, a config home, and a project-scoped cwd, and none of that changes
mid-session. Such a row renders its value dim as `false (unavailable)`, cycling
it raises an explanatory toast, and it is never written to the settings file —
so a session in `~` doesn't persist "checkpoints off" into every later project.
[`SessionSettings::availability`] carries the boundary's verdict, injected at
bootstrap like the clock.

## The shape

`SettingsPicker` is `ModelPicker`'s sibling: it lives in `App` as an
`Option<SettingsPicker>`, renders **inline** (replacing the composer, never an
alternate screen), and owns every key while open. Its rows are derived on demand
from the live state — `App::setting_rows()` — rather than stored, exactly as the
`/model` picker derives its matches, so the value column can never drift from
what the session is actually doing.

The layout is the `/model` picker's plus one hint row:

```
rule · gap · ❯ search · gap · [rows] · (n/total) · gap · description · gap · hint · gap · rule
```

— eleven fixed rows (`SETTINGS_CHROME_ROWS`) around a list capped at
`SETTINGS_MENU_MAX_ROWS`, windowed by the shared `centered_window` so the
selection stays centred. Unlike `/model` the chrome never collapses: there is
always at least one setting, and a search that matches nothing still shows its
placeholder in the same frame.

### Keys

| Key | Effect |
|---|---|
| ↑/↓, PageUp/PageDown, Home/End | Move the selection (clamped) |
| **Enter** / **Space** | Cycle the highlighted setting to its next value |
| printable characters | Type into the search (Space is *not* one of them — it's the cycle key) |
| Backspace | Pop the search |
| Esc | Clear a non-empty search, else close |
| Ctrl+C | Close |

Space cycling is why the search can't contain a space; that's fine — the labels
are matched case-insensitively as substrings against the label *and* the
description, so `perm` finds `Permission mode` and `agents` finds `Project
docs`.

## What happens on a change

`App::cycle_selected_setting()` advances the pure value and returns an
[`Action`] for the loop, which is where every knob is actually *applied*:

- **Permission mode** returns the existing `Action::SetPermissionMode`, so it
  goes through the one path that mirrors the mode onto the gate, sweeps the
  requests the new mode now covers, and persists this project's entry.
- Everything else returns `Action::SettingChanged(key)`, and
  `tui::settings::Session::apply_setting` does the work:
  - **Tools**, **Error retry**, **Temperature** rebuild the backend
    (`ModelSession::set_tools` / `set_max_retries` / `set_temperature` — the
    `/model` switch's rebuild, so the whole shared attachment set is
    re-attached).
  - **Checkpoints** flips `CheckpointStore::set_enabled`, which can only ever
    turn a *capable* store on or off.
  - **Project docs** reloads (or drops) `App::user_instructions` at once, so
    Ctrl+D shows the change before the next turn.
  - **Hide thinking** and **Auto compact** need nothing beyond the pure flag:
    `tui::stream` consults `App::settings().show_thinking()` per phase and
    `App::should_auto_compact` gates on the flag.

Every change confirms with a transient toast (`{label}: {value}`) and persists.

Like `/model` and `/login`, `/settings` is **available mid-turn** — it only
replaces the composer, and a rebuild rebinds the *next* turn's backend while the
running one streams on its own thread, untouched.

## Persistence

`~/.alter-zero/settings.json`, its own file beside `config.json` (the `/model`
selection) and `permissions.json` (the per-project rules) — one file per feature
that owns it, so a write can never clobber a neighbour's state. The format is
pure ([`settings::SessionSettings`], every field optional and skipped when it
equals the default) and the read/write is the boundary's
(`tui::config::load_saved_settings` / `save_session_settings`), best-effort like
the others: a read-only home must never kill the TUI. The file reads as a **diff
from the defaults** — only what you actually changed is in it:

```json
{
  "hide_thinking": true,
  "error_retry": 5,
  "temperature": 0.7
}
```

Precedence at startup is the same rule the rest of the app follows — **the
environment wins**: an explicitly set `ALTER_ZERO_*` variable overrides the saved
value for that one setting (`tui::config::apply_setting_overrides`). Anything
neither set nor saved takes its default.

And, as with `ALTER_ZERO_MODEL`, the environment **never sticks**. The session
keeps the file's own blob beside the live one (`Session::saved_settings`) and a
save is a read-modify-write that moves across **only the key the user cycled**
(`SessionSettings::copy_value`). Writing the merged blob back instead would have
quietly persisted an override set for one run: turn `Error retry` up in a shell
that happens to export `ALTER_ZERO_TOOLS=0`, and every later session in every
other directory would have started with tools off.

## Testing

The pure model (`src/settings.rs`), the picker state (`src/app/settings.rs`) and
the renderer (`src/ui/settings_view.rs`) are unit-tested. The boundary — the
backend rebuild, the checkpoint flip, the file write — is covered by
`scripts/smoke.sh` Phase 67, which opens the menu, searches, cycles a value, and
checks the toast and the collapsed composer.
