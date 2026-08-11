# `/settings` — the session settings menu

Every knob this app has was, until now, an environment variable you had to know
about before launch: `ALTER_ZERO_SHOW_THINKING`, `ALTER_ZERO_TOOLS`,
`ALTER_ZERO_CHECKPOINTS`, `ALTER_ZERO_TEMPERATURE`,
`ALTER_ZERO_PROJECT_DOC_MAX_BYTES`, plus a hard-coded `retry::MAX_RETRIES`, a
hard-coded `agent::MAX_TOOL_ITERATIONS`, and an always-on auto-compaction. `/settings` makes that inventory **visible and
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
  Max tool calls          0
  (1/9)

  Hide the model's chain-of-thought…

  Type to search · Enter/Space to change · Esc to cancel

────────────────────────────────────────────────────────────────────────────
```

## The settings

Ten rows, each one a knob the running session actually reads. Every value
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
| **Hooks** | `true` / `false` | Whether the user's `~/.alter-zero/hooks.json` lifecycle hooks run — around tool calls, turns, and the session boundaries (`docs/hooks.md`). Seeded from `ALTER_ZERO_HOOKS`; **unavailable** when no hooks file resolved or it had nothing runnable in it. |
| **Skills** | `true` / `false` | Whether the `skill` tool is offered and the `<system-reminder>` listing rides the context (`docs/skills.md`). Seeded from `ALTER_ZERO_SKILLS`; **unavailable** when no `SKILL.md` loaded — there is nothing to turn on. |
| **Temperature** | `default` / `0.0` / `0.3` / `0.5` / `0.7` / `1.0` | The sampling temperature every request carries; `default` sends none and leaves it to the provider. Seeded from `ALTER_ZERO_TEMPERATURE`. |
| **Max tool calls** | **`0`** / `5` / `10` / `20` / `50` / `100` | How many tool **calls** one turn may run before it gives up (`llm::agent::run_agent`'s cap). **`0` is no limit, and the default** — see below. |

### The ceiling counts calls, not rounds

It has to. A round can request a whole **parallel batch**
(`docs/parallel-tools.md`) — ask a model to run six commands and it answers
with six `tool_calls` in one round — so a per-round counter let a single round
overspend the ceiling several times over: `Max tool calls 5` really ran 15, the
reported bug. `run_agent` therefore spends the budget **per call**.

A round the budget can only partly afford is **clamped** rather than refused
whole: as many of its calls as fit run, in the model's own order, and the rest
resolve as red cells reading `Not run: this turn had already spent its
tool-call limit.`
Refusing the whole round would be simpler, but a model that opens with a batch
wider than the entire ceiling would then do nothing at all and just error.
Every refused call is still *answered* with that same text, so the stored
message list keeps a `tool` result for every `tool_call` — what a strict
provider requires of the next request, a `/resume`, or an agent continuation.

The turn then ends on a red notice that carries the whole story, because the
user meets it with nothing else on the row to explain it — no cell above, no
hint below:

```
● Stopped after 5 tool calls — this turn hit the "Max tool calls" limit before
  the model finished. Run /settings to raise it, or set it to 0 for no limit.
```

What stopped, **why** (a ceiling the user set — not a model or provider
failure, which is what a bare `stopped after 5 tool calls` read like), and
**how** to change it, naming the row's own label so it can be found.

**That error is also how the model finds out.** A `Role::Error` notice derives
into an `[error] …` **user** entry in the context (`crate::context`), so the
next turn's window already reads:

```
user:
  [error] Stopped after 5 tool calls — this turn hit the "Max tool calls"
  limit before the model finished. Run /settings to raise it, or set it to 0
  for no limit.
```

Which is why the refused calls' own results stay one bare sentence. Putting the
advice there too would say the same thing twice — and a clamped batch refuses
several calls at once, so it would be several paragraphs of screen *and* of
context window for one fact:

```
tool:
  Not run: this turn had already spent its tool-call limit.
```

One place for the advice, one place for the fact. Asked "why did that stop?" on
the next turn, the model reads both and answers with the reason and the
setting.

### Why **Max tool calls** defaults to none

The library keeps `agent::MAX_TOOL_ITERATIONS = 20` as a backstop against a
model that loops forever, and that is the right default for an embedder with no
one watching. It is the wrong one for this app: a cap that trips mid-task
abandons the work half-done — files half-written, a migration half-applied —
and the user is sitting right there with **Esc**, which stops a turn instantly
and keeps everything that streamed. So the app ships uncapped (`0`), and the
ceilings are there for anyone who wants a hard one. `0` renders in the dim
"off" colour, like `false`: the limiter is switched off.

### Unavailable settings

A knob can be **unavailable** rather than merely off: checkpoints need a `git`
binary, a config home, a cwd the feature will snapshot (`checkpoint::cwd_scope`
— not `/tmp`, `~`, `~/.alter-zero`, or a system tree), and a working tree small
enough for the per-turn cost budget (the pre-flight probe, which retires the
store with `CheckpointStore::disable` when it isn't). None of that changes
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
  - **Tools**, **Error retry**, **Temperature** and **Max tool calls** rebuild
    the backend (`ModelSession::set_tools` / `set_max_retries` /
    `set_temperature` / `set_max_tool_calls` — the `/model` switch's rebuild, so
    the whole shared attachment set is re-attached, and a subagent inherits the
    same budgets).
  - **Checkpoints** flips `CheckpointStore::set_enabled`, which can only ever
    turn a *capable* store on or off.
  - **Hooks** rebuilds too: the sink is attached per backend build, so
    flipping the row genuinely stops (or starts) the next turn consulting the
    user's commands rather than leaving a second flag to drift
    (`docs/hooks.md`).
  - **Project docs** reloads (or drops) `App::user_instructions` at once, so
    Ctrl+D shows the change before the next turn.
  - **Skills** does both halves, because the feature has two: the tool rides
    the backend build (so the row rebuilds it) and the listing rides the
    context (so the row re-renders `App::skill_listing`). Flipping it off
    leaves the discovered set in memory, so flipping it back on costs no
    rescan (`docs/skills.md`).
  - **Hide thinking** and **Auto compact** need nothing beyond the pure flag:
    `tui::stream` consults `App::settings().show_thinking()` per phase and
    `App::should_auto_compact` gates on the flag.

Every change confirms with a transient toast (`{label}: {value}`) and persists.

Like `/model` and `/login`, `/settings` is **available mid-turn** — it only
replaces the composer, and a rebuild rebinds the *next* turn's backend while the
running one streams on its own thread, untouched. "Only the composer" is
literal (updated 2026-08-08): the streaming strip — the running tool's live
cell, the status line, the queued messages, the toast — keeps its rows above
the menu's frame, so opening `/settings` mid-turn never hides the turn it was
opened beside. `settings_height` reserves `ui::layout`'s `strip_above_rows`
over the menu's own `settings_rows`, and `render_live` paints the strip above
it through the shared `view_split`. See `docs/llm.md` (the reported bug, first
fixed for the ↓ manager band in `docs/background.md`).

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
