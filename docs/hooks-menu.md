# The `/hooks` menu — a read-only hooks browser

`/hooks` opens an inline, **read-only** browser over the user's configured
lifecycle hooks (`docs/hooks.md`) — Claude Code's `/hooks` menu, ported whole
from its `HooksConfigMenu` (the `SelectEventMode` → `SelectMatcherMode` →
`SelectHookMode` → `ViewHookMode` state machine). It answers "what guards are
armed right now?" without leaving the session; to add or change a hook the
user edits `hooks.json` (the menu says so on its face), because duplicating a
settings-file editor in-menu is exactly the maintenance burden that made the
reference go read-only.

(codex grew its own `/hooks` browser too — `hooks_browser_view.rs`, a
two-page events-table → handlers view with trust-review columns for its
plugin sources. Ours follows Claude Code's four-level shape instead: this
port has one user-level `hooks.json` and no trust model to review, and the
numbered `❯` rows / `Hook details` page are the look the feature was asked
for in.)

## Where it lives

The **fourth composer-replacing inline picker**, beside `/model`, `/login`
and `/settings`: it renders between the `/model` picker's rules in place of
the composer, keeps the streaming strip above itself (`ui::layout`'s
`strip_above_rows` / `view_split` — opening it mid-turn never hides the
running turn), owns every key while open, and closes back to the composer.
Like the ↓ background manager band it has **no text entry** — the hardware
cursor parks in the frame's far corner where it reads as chrome, and there is
no type-to-search: navigation is the whole grammar.

Because browsing touches nothing, `/hooks` works mid-turn (the `/model`
rule), and closing is pure collapse — the loop has nothing to reap.

## The four levels

A tiny stack machine (`app::HooksLevel`), Esc popping one frame at a time:

1. **Events** — `Hooks` over `{N} hook(s) configured`, the read-only info
   line (`ℹ This menu is read-only. To add or modify hooks, edit hooks.json
   directly or ask alter-zero. See docs/hooks.md`), and the **eleven**
   [`HookEvent`]s as numbered rows — `{n}.  {Event} ({count})` with the
   count in the selection accent (dropped at zero, Claude Code's shape) and
   the event's one-line summary in an aligned description column. When the
   file holds runnable hooks but the session has them off (`/settings`,
   `ALTER_ZERO_HOOKS=0`), a red-ish note says so — the reference's
   "restricted by policy" slot.
2. **Matchers** — `{Event} - Matchers` over the event's multi-line
   description (stdin payload + exit-code semantics, **as this runner
   implements them**), then one row per configured matcher group —
   `{n}. [User] {matcher}` with `{k} hook(s)` in the description column.
   Groups sharing a matcher string merge into one row (the reference's
   grouping); an absent/empty matcher displays `(all)`.
3. **Hooks** — `{Event} - Matcher: {matcher}` (for a matcher-less event just
   `{Event}`), the same event description, then one row per handler —
   `{n}. [{type}] {command}` (the handler's `statusMessage` stands in for
   the command when set, Claude Code's `getHookDisplayText`) with
   `User Settings` in the description column.
4. **Hook details** — `Hook details` over an aligned field block (`Event:` /
   `Matcher:` (only when the event matches on something) / `Type:` /
   `Source:  User settings ({hooks.json path})`), the `Command:` label over
   a rounded dim-bordered box holding the **real** command word-wrapped
   (`ui::wrap_output`, spaces preserved — `statusMessage` never stands in
   here), a `Status message:` line when one is set, and the closing
   direction to edit `hooks.json`. The hint row reads `Esc to go back` —
   Enter does nothing at the bottom of the stack.

Levels 1–3 share the select grammar: ↑/↓ move (clamped), Home/End jump,
**digits 1–9 jump-activate** their row (the `AskUserQuestion` modal's rule),
Enter descends, Esc ascends (at Events it closes), Ctrl+C closes outright
(the picker family's rule — never quits). Rows past
`HOOKS_MENU_MAX_ROWS` (5, the reference's `visibleOptionCount`) scroll in a
**selection-centered window** (`centered_window`, the `/model` list's), with
dim `↑`/`↓` overflow markers in the marker cell of the window's edge rows —
numbering stays **absolute**, so the second mock's `↑ 2. … ↓ 6.` window reads
exactly as Claude Code draws it.

Drilling into an event with nothing configured still descends — the list is
replaced by the reference's two dim lines (`No hooks configured for this
event.` / `To add hooks, edit hooks.json directly or ask alter-zero.`) — so
the description text (what *would* fire, and how) stays reachable for every
event, hooks or none.

## Matchers are the dispatcher's truth, not a copy of the reference's

An event shows the matcher level exactly when our dispatch passes a match
query for it (`hooks::event_has_matchers`, mirroring `llm/hooks.rs`): the
tool events match the **tool name**, `SubagentStart`/`SubagentStop` the
**agent type**, `SessionStart` the **source**, `SessionEnd` the **reason**,
`PreCompact`/`PostCompact` the **trigger**. `Stop` and `UserPromptSubmit`
match on nothing — their groups run matcher-or-not — so Enter on them skips
straight to level 3 (every handler of the event, flattened), and their
detail page shows no `Matcher:` row: a matcher the engine ignores must not
be presented as if it filtered.

The per-event summary/description strings (`hooks::event_summary` /
`event_description`) are likewise **this runner's** semantics — exit `2`
blocks with stderr as the reason, exit `0` + stdout JSON is a verdict, other
exits are non-blocking, blocks ignored where the engine ignores them
(`SessionStart`, `SubagentStart`), fire-and-forget where the outcome is
discarded (`PostCompact`, `SessionEnd`) — not the reference's, whose exit
codes mean different things.

## The pure/boundary split

- **`hooks::overview`** (pure): `HooksOverview::from_file` digests a parsed
  [`HooksFile`] into the display tree — all eleven events in registry order,
  each with its matcher groups merged by matcher string (config order kept)
  and every handler (any `type`, the skipped kinds included: they are
  *configured*, which is what a browser reports) resolved to
  `{kind, content, status_message}`. Events the file names that we don't
  model never show (the reference iterates its own registry the same way).
- **`app::hooks_menu`** (pure state): the open menu — the overview snapshot,
  the `Source:` path string, the enabled flag, and the level stack — plus
  `on_key_hooks`. Opening is `App::open_hooks_menu(overview, source,
  enabled)`, the boundary-injection seam.
- **`ui::hooks_view`** (pure render): `hooks_view_lines` builds the whole
  framed body line-by-line (the ↓ manager band's pattern — heights fall out
  as `lines.len()`, so multi-line descriptions and the wrapped command box
  need no fixed-slot layout), `hooks_menu_height` seats it under the strip
  via `view_height`, `render_hooks_menu` paints it. Styling is `HOOKS_*` in
  `ui/theme.rs`, reusing the picker family's accents.
- **`tui`** (boundary): the `Action::OpenHooksMenu` arm derives the overview
  from the live `HookSetup` (the same `Arc<HooksFile>` every backend rebuild
  re-attaches, so the browser and the runner can never disagree), resolves
  the display path (`llm::hooks::hooks_file_path` → `ui::display_cwd`
  ~-shortening), reads the live enabled flag, and hands all three to `App`.
  The `/resume`-picker pattern: the pure command returns the intent, the
  loop supplies the data.

A missing or empty `hooks.json` still opens the menu — `0 hooks configured`,
every event at zero — and a malformed one browses as empty while the
startup toast (`docs/hooks.md`) carries the actual error.

## What this deliberately is not

No editing, no reloading (the overview is snapshotted at open, exactly as
the runner snapshotted the file at startup), no project-level sources (one
file → one `[User]` tag, `User Settings`, `User settings ({path})`), and no
new `StreamEvent`/`HistoryItem` — the menu is UI over config, never
conversation.
