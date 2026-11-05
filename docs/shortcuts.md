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
band is now `menu_rows(app, width) + shortcuts_rows(app, width)` (one is
always 0);
`render_live` paints whichever is open, `cursor_position` reserves the same
rows (the cursor never moves when the band opens), and `main.rs`'s
`live_region_height` passes the same sum to `ui::live_height`.

`shortcuts_lines(turn_active, can_backtrack, width)` lists this app's actual
bindings, codex's phrasing and two-column layout, keys cyan and labels dim
(`SHORTCUTS_*` consts), the whole grid inset two columns:

```
  / for commands                ! for shell command
  ↑ for input history           ctrl+r to search history
  shift+enter for newline       ctrl+o for tool output
  esc to quit                   ctrl+c to quit
  alt+↑ to edit queue           tab to queue next turn
  ctrl+v for image paste        ctrl+d for llm context
  ctrl+t to cycle thinking      shift+tab for permission mode
  $ for skills                  ctrl+w/u/k to kill text
```

The inset is `MENU_INDENT`, a blank span wearing no colour. The band takes
the footer's slot, and the footer row is inset by the same two columns
(`  {model} · {cwd}`), so the help opens where the line it replaces began —
codex's overlay, one of its footer's modes, wears the footer's indent too. It is
also the column every other band below the box starts its names in, after
the palette's and the pickers' `→ ` marker, so whichever band is open, its
text starts two in from the edge. The grid used to sit flush against the
edge, the one thing below the box that did.

(The `SHORTCUTS` const in `ui/theme.rs` is the single source of truth — entries laid
out two per row in declaration order, so the band is
`SHORTCUTS.len().div_ceil(2)` rows tall; currently 16 entries → 8 rows. The
`$` skill-mention sigil (`docs/skill-mentions.md`) is an ordinary entry in
that grid — the last row's first column, beside the kill keys. It rode the
first row as a third column for a while; the special case is retired, so
the band is one two-column grid wherever both columns fit. Note that adding
an entry **reflows every pairing after
it**, so a wide entry that was safe in a second column can land in a first
one — check `SHORTCUTS_COL` still leaves a gutter when adding one. The
Ctrl+D view's classifier page has no entry of its own: Tab is discovered on
the view's own hint row, `docs/permissions.md`.)

The second column starts `SHORTCUTS_COL` (30) past the inset — column 32 on
screen — sized so the **widest**
first-column variant keeps a readable gutter (an earlier pairing put a
27-column entry in the first column, which at the old column of 28 left a
single space before its neighbour — the two entries read as one run-on line;
the esc context swaps reach 24 the same way). The
`the_shortcuts_columns_keep_a_readable_gutter_in_every_state` test pins a
≥ 2-column gutter and the shared column across every `(turn_active,
can_backtrack)` state; an entry added later that grows past it must widen
`SHORTCUTS_COL` with it (the test fails until it does).

**On a terminal too narrow for both columns the band stacks into one.** Two
columns need the inset, `SHORTCUTS_COL` and the widest second-column entry —
61 columns today (`shift+tab for permission mode` from column 32) — and below
that the paint's clip used to cut the second column mid-word
(`shift+tab for perm` at 50 columns, six of eight rows cut at 51), on the
band whose whole job is to be read. So `shortcuts_lines` measures every grid
row against `width` and, when any would be cut, lists one entry per row in
the same reading order instead, each still inset:

```
  / for commands
  ! for shell command
  ↑ for input history
  ctrl+r to search history
  …
```

The band is taller there (16 rows), which is the price of saying every
binding whole; it is a transient overlay any key dismisses. `shortcuts_rows`
is the built line count — the palette's height-is-the-line-count rule
(`docs/view-flow.md`) — so the reserved rows and the painted rows agree at
every width. The band takes its rows ahead of the streaming strip (the
composer's floor is the only thing `live_layout` holds back first), so on a
51×24 pane with a reply streaming, the composer, its rules and the status
line all stay on screen while the reply's preview yields the rows the band
needs until the band closes (`smoke.sh` Phase 99).

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
- **A terminal both narrow and short cuts the stacked band at the bottom.**
  The band's height is decided by the width alone, so where fewer than its 16
  rows are left under the composer (a terminal under 61 columns *and* under
  about 19 rows) the last entries do not show — at 50×16 the final three.
  Choosing the layout by the height too would make the reserved rows depend
  on a size the paint does not see the same way, and the case is rare: a
  terminal that narrow is usually a split pane on a taller screen.

## Testing

- `app`: `?` toggles on/off with an empty composer (shift-modified too) and
  types into a non-empty draft; any other key closes the band but still
  performs its action (typing, ↑ recall, `/` palette); Esc only dismisses —
  idle (no quit) and mid-turn (no interrupt, band closed, turn untouched);
  `?` is ignored in the tool view.
- `ui`: `shortcuts_rows` is 0 closed / `SHORTCUTS.len().div_ceil(2)` open (8
  rows for the current 16 entries) wherever both columns fit, and
  `SHORTCUTS.len()` one column narrower — two columns at exactly 61, one
  entry per row, in order, at 60; every row opens on the blank
  `MENU_INDENT` inset (the footer's width), and the lines list the bindings in
  two aligned columns (keys cyan, labels dim) with a ≥ 2-column gutter between
  them in every context state (the esc swaps change the first column's width);
  the Esc entry flips between
  `to quit` and `to interrupt` with the turn; `live_height` grows by the band;
  `render_live` paints it below the box and `cursor_position` stays put when
  it opens.
- `src/tui/` (smoke, Phase 11): `?` shows the band (`for commands` visible),
  inset two columns with its second column at 32, `?` again hides it, typing
  a draft containing `?` ends up in the input box untouched, and on a
  50-column pane the band lists one entry per row, every entry whole.
