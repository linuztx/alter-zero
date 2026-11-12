# The `/hooks` menu — the hooks browser

`/hooks` opens an inline browser over every configured lifecycle hook — the
user's `hooks.json` and a project's (`docs/tools/hooks.md`) — Claude Code's `/hooks` menu, ported whole
from its `HooksConfigMenu` (the `SelectEventMode` → `SelectMatcherMode` →
`SelectHookMode` → `ViewHookMode` state machine). It answers "what guards are
armed right now?" without leaving the session; to add or change a hook the
user edits `hooks.json` (the menu says so on its face), because duplicating a
settings-file editor in-menu is exactly the maintenance burden that made the
reference go read-only. The one thing it changes is **trust**
(`docs/state/config-review.md`): a hook runs only once the user trusts it, and the
menu is where that changes after the startup review.

(codex grew its own `/hooks` browser too — `hooks_browser_view.rs`, a
two-page events-table → handlers view with trust-review columns for its
plugin sources. Ours follows Claude Code's four-level shape instead, the
numbered `❯` rows / `Hook details` page being the look the feature was asked
for in, and carries trust on the rows and the detail page rather than in
columns.)

## Where it lives

The **fourth composer-replacing inline picker**, beside `/model`, `/login`
and `/settings`: it renders between the `/model` picker's rules in place of
the composer, keeps the streaming strip above itself (`ui::layout`'s
`strip_above_rows` / `view_split` — opening it mid-turn never hides the
running turn), owns every key while open, and closes back to the composer.
Like the ↓ background manager band it has **no text entry**, and there is
no type-to-search: navigation is the whole grammar. So the frame shows **no
hardware cursor** while the menu is up — the permission prompt's rule
(`ui::cursor_visible`): a menu has nothing for a cursor to point at, and a
terminal with a cursor animation (kitty) blinks at whatever seat one picks.
The *seat* still tracks the highlighted `❯` row (`ui::layout`'s marker
scan), so a terminal that ignores the hide — and the cursor's return when
the menu closes — starts from somewhere meaningful. A page with no `❯` —
the hook detail, an event's empty state — seats just past its closing hint
instead, the full-screen overlays' rule (`overlay_cursor_seat`): never on
the bottom rule, whose far corner is where such a page used to park the
seat (`docs/ui/view-flow.md`).

Because browsing touches nothing running — a trust change rebinds the
*next* backend build — `/hooks` works mid-turn (the `/model` rule), and
closing is pure collapse — the loop has nothing to reap.

## The four levels

A tiny stack machine (`app::HooksLevel`), Esc popping one frame at a time:

1. **Events** — `Hooks` over `{N} hook(s) configured`, an amber note when
   any hook is not trusted (`2 hooks are not trusted and do not run — open
   one and press Enter to trust it`), the info line (`ℹ Open a hook to trust
   it, or to stop trusting it. To add or modify hooks, edit hooks.json
   directly or ask alter-zero. See docs/tools/hooks.md`), and the **fourteen**
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
   the command when set, Claude Code's `getHookDisplayText`), an amber
   `⚠ not trusted` after a hook that waits for trust, and `User Settings` in
   the description column.
4. **Hook details** — `Hook details` over an aligned field block (`Event:` /
   `Matcher:` (only when the event matches on something) / `Type:` /
   `Source:  User settings ({hooks.json path})` / `Trust:` — `✔ trusted`, or
   `⚠ not trusted — it does not run` — for a hook that can run), the
   `Command:` label over
   a rounded dim-bordered box holding the **real** command word-wrapped
   (`ui::wrap_output`, spaces preserved — `statusMessage` never stands in
   here), a `Status message:` line when one is set, and the closing
   direction to edit `hooks.json`. **Enter toggles the hook's trust**
   (`Action::SetHookTrust` — the page updates at once, the loop records it in
   `trust.json` and rebinds the hooks the session runs), so the hint reads
   `Enter to trust · Esc to go back` or `Enter to stop trusting · Esc to go
   back`; a handler that never runs has no trust and the hint stays `Esc to
   go back`. A detail whose wrapped
   command box outgrows the terminal renders **bottom-anchored** with its
   skipped top **flowed into real scrollback**, so the whole page stays
   readable and the hint/rule stay on screen (`docs/ui/view-flow.md`;
   `smoke.sh` Phase 85).

Levels 1–3 share the select grammar: ↑/↓ move (wrapping at the ends), Home/End
jump,
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
`PreCompact`/`PostCompact` the **trigger**, `StopFailure` the **error
type**, `Notification` the **notification type**. `Stop` and `UserPromptSubmit`
match on nothing — their groups run matcher-or-not — so Enter on them skips
straight to level 3 (every handler of the event, flattened), and their
detail page shows no `Matcher:` row: a matcher the engine ignores must not
be presented as if it filtered.

The per-event summary/description strings (`hooks::event_summary` /
`event_description`) are likewise **this runner's** semantics — exit `2`
blocks with stderr as the reason, exit `0` + stdout JSON is a verdict, other
exits are non-blocking, blocks ignored where the engine ignores them
(`SessionStart`, `SubagentStart`), fire-and-forget where the outcome is
discarded (`PostCompact`, `SessionEnd`, `StopFailure`, `Notification`) — not
the reference's, whose exit codes mean different things.

## The pure/boundary split

- **`hooks::overview`** (pure): `HooksOverview::from_file` digests a parsed
  [`HooksFile`] into the display tree — all fourteen events in registry order,
  each with its matcher groups merged by matcher string (config order kept)
  and every handler (any `type`, the skipped kinds included: they are
  *configured*, which is what a browser reports) resolved to
  `{kind, content, status_message}` plus, for a hook that can run, its
  `trust_key` (its `hooks.json` and `trust::HookIdentity`) — and
  `with_trust` answers each one's `trusted`. Events the file names that we
  don't model never show (the reference iterates its own registry the same
  way).
- **`app::hooks_menu`** (pure state): the open menu — the overview snapshot,
  the `Source:` path string, the enabled flag, and the level stack — plus
  `on_key_hooks`. Opening is `App::open_hooks_menu(overview, source,
  enabled)`, the boundary-injection seam.
- **`ui::hooks_view`** (pure render): `hooks_view_lines` builds the whole
  framed body line-by-line (the ↓ manager band's pattern — heights fall out
  as `lines.len()`, so multi-line descriptions and the wrapped command box
  need no fixed-slot layout), `hooks_menu_height` seats it under the strip
  via `view_height`, `render_hooks_menu` paints it. Styling is `HOOKS_*` in
  `ui/theme.rs`, reusing the picker family's accents; every level's title
  wears the pages' one title dress — bold, washed in the active theme's
  banner gradient (`ui::header::page_title_line`, `docs/ui/theme.md`), like
  the `/mcp` manager's: both menus are walks several levels deep, and the
  headline is the row that answers "where am I?".
- **`tui`** (boundary): the `Action::OpenHooksMenu` arm derives the overview
  from every configured hook — the user's file and the project's, as read at
  bootstrap — with the trust `trust.json` records (the runner runs exactly
  the trusted share, so the browser and the runner can never disagree), resolves
  the display path (`llm::hooks::hooks_file_path` → `ui::display_cwd`
  ~-shortening), reads the live enabled flag, and hands all three to `App`.
  The `/resume`-picker pattern: the pure command returns the intent, the
  loop supplies the data.

A missing or empty `hooks.json` still opens the menu — `0 hooks configured`,
every event at zero — and a malformed one browses as empty while the
startup toast (`docs/tools/hooks.md`) carries the actual error.

## What this deliberately is not

No editing beyond trust, no reloading (the overview is snapshotted at open,
exactly as the runner snapshotted the files at startup), and no new
`StreamEvent`/`HistoryItem` — the menu is UI over config, never
conversation.
