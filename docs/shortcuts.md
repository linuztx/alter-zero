# `?` shortcuts band — keyboard-shortcut help below the box

Date: 2026-06-11

## Goal

Pressing **`?` in an empty composer** shows a small keyboard-shortcuts
overview — the way openai/codex's `?` toggles its footer shortcut overlay —
and pressing it again (or doing anything else) hides it. With text in the
composer `?` is just a character.

## What codex does (ported from `/tmp/codex/codex-rs/tui`)

`bottom_pane/chat_composer.rs` (`handle_shortcut_overlay_key`) +
`bottom_pane/footer.rs`:

- The toggle is bound to **both plain `?` and shift+`?`**
  (`toggle_shortcuts_keys`) because terminals differ in whether Shift+/
  reports the SHIFT modifier.
- It toggles **only when the composer is empty** (and no paste burst is in
  progress) — "so typing/pasting `?` still inserts text instead of opening
  help".
- The overlay is a footer mode (`FooterMode::ShortcutOverlay`):
  `toggle_shortcut_mode` flips it back to the base mode on a second press,
  and `reset_mode_after_activity` closes it on any other activity — the
  overlay is display-only, never modal: other keys still do their job.
- Content (`shortcut_overlay_lines`): one `{key} for {thing}` / `{key} to
  {verb}` entry per binding (`/ for commands`, `shift+enter for newline`,
  `ctrl+c to quit`, …), laid out in **two aligned columns**, the whole block
  dim with the keys highlighted. The quit entry is context-sensitive: it
  reads **"to interrupt" while a task is running**, "to quit" otherwise.
- The empty-composer footer advertises it with a dim `? for shortcuts` hint.

## The mapping onto this codebase

### State (`App.shortcuts_open: bool`)

One flag — our footer has exactly two modes (closed/open), so codex's
`FooterMode` enum collapses to a bool. In `App::on_key_conversation`:

- `?` (`Char('?')`, no CTRL/ALT — SHIFT allowed, codex's dual binding) with an
  **empty input** toggles the flag and consumes the key. Works mid-turn too
  (the band is below the box; the streaming strip above is untouched).
- `?` with a non-empty input falls through to the ordinary character arm.
- **Any other key while the band is open closes it first** (codex's
  reset-after-activity), then acts normally — typing types, ↑ recalls, `/`
  opens the palette. The exception is **Esc, which only dismisses** (returns
  `Action::None`): our idle Esc quits (with nothing to backtrack to) or arms
  the Esc-Esc backtrack (`docs/backtrack.md`), and doing either to someone who
  is reading the help would be hostile — the same "popup wins" precedence the
  command palette already has.

The band and the palette are mutually exclusive by construction: the palette
needs a non-empty `/token`, the band an empty composer, and the keystroke
that creates either state closes the other.

### Rendering (`ui::shortcuts_rows` / `ui::shortcuts_lines`)

The band reuses the palette's slot — the reserved rows **below the input
box** (codex's footer sits below its composer too). `live_layout`'s third
band is now `menu_rows(app) + shortcuts_rows(app)` (one is always 0);
`render_live` paints whichever is open, `cursor_position` reserves the same
rows (the cursor never moves when the band opens), and `main.rs`'s
`live_region_height` passes the same sum to `ui::live_height`.

`shortcuts_lines(turn_active, can_backtrack)` lists this app's actual
bindings, codex's phrasing and two-column layout, keys cyan and labels dim
(`SHORTCUTS_*` consts):

```
/ for commands            ! for shell command
↑ for input history       ctrl+r to search history
shift+enter for newline   ctrl+o for tool output
esc to quit               ctrl+c to quit
alt+↑ to edit queue       tab to queue next turn
ctrl+v for image paste    ctrl+d for llm context
shift+tab to cycle thinking
```

(The `SHORTCUTS` const in `ui/theme.rs` is the single source of truth — entries laid
out two per row in declaration order, so the band is
`SHORTCUTS.len().div_ceil(2)` rows tall; currently 13 entries → 7 rows.)

The Esc entry is three-way context-sensitive (codex's quit entry): `esc to
interrupt` while a turn is in flight, `esc esc to edit previous`
(`SHORTCUTS_BACKTRACK`) when idle with a previous user message to edit
(`docs/backtrack.md`), and `esc to quit` only with nothing to backtrack to.

### Known divergences

- **No persistent `? for shortcuts` hint**: codex has an always-present
  footer line to put it on; we deliberately keep the idle live region at its
  minimum (one box, nothing else), so discoverability costs a row we don't
  want to spend. The band itself is the documentation once found.
- **No paste-burst guard**: our `paste::PasteBurst` lives at the I/O boundary
  and only times redraws — the pure `App` can't see it. A non-bracketed paste
  beginning with `?` into an empty composer briefly toggles the band (the
  next pasted char closes it again, the `?` is lost). Rare and self-healing;
  not worth leaking boundary state into the core.
- No `customize shortcuts with /keymap` trailer (we have no keymap), and the
  entries are this app's bindings, not codex's.

## Testing

- `app`: `?` toggles on/off with an empty composer (shift-modified too) and
  types into a non-empty draft; any other key closes the band but still
  performs its action (typing, ↑ recall, `/` palette); Esc only dismisses —
  idle (no quit) and mid-turn (no interrupt, band closed, turn untouched);
  `?` is ignored in the tool view.
- `ui`: `shortcuts_rows` is 0 closed / `SHORTCUTS.len().div_ceil(2)` open (6
  rows for the current 11 entries); the lines list the bindings in
  two aligned columns (keys cyan, labels dim); the Esc entry flips between
  `to quit` and `to interrupt` with the turn; `live_height` grows by the band;
  `render_live` paints it below the box and `cursor_position` stays put when
  it opens.
- `main.rs` (smoke, Phase 11): `?` shows the band (`for commands` visible),
  `?` again hides it, and typing a draft containing `?` ends up in the input
  box untouched.
