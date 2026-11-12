# `?` shortcuts band — keyboard-shortcut help below the box

Date: 2026-06-11

## Goal

Pressing **`?` in an empty composer** shows a small keyboard-shortcuts
overview — the way openai/codex's `?` toggles its footer shortcut overlay —
and pressing it again (or doing anything else) hides it. With text in the
composer `?` is just a character.

The overview fits itself to the terminal: as many columns as the width holds
with every entry whole, so a wide terminal spends its width instead of rows
below the box, down to two terse columns and then one entry per row on a
narrow pane; and on a terminal too short for it, the rows that fit and a
count of the rest (*As many columns as the width holds*, below — the
responsive layout dates from 2026-11-06).

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
  the Esc-Esc backtrack (`docs/input/backtrack.md`), and doing either to someone who
  is reading the help would be hostile — the same "popup wins" precedence the
  command palette already has.

The band and the palette are mutually exclusive by construction: the palette
needs a non-empty `/token`, the band an empty composer, and the keystroke
that creates either state closes the other.

### Rendering (`ui::shortcuts_rows` / `ui::shortcuts_lines`)

The band reuses the palette's slot — the reserved rows **below the input
box** (codex's footer sits below its composer too). `band_rows` sums
`menu_rows(app, width) + shortcuts_rows(app, width)` with the two pickers'
rows (at most one term is ever non-zero); `render_live` paints whichever is
open, `cursor_position` reserves the same rows (the cursor never moves when
the band opens), and the boundary's `live_region_height` passes the same sum
to `ui::live_height`.

Each entry is codex's `{key} for {thing}` / `{key} to {verb}`, the key cyan
and the label dim (`shortcuts_key_color()` / `shortcuts_text_color()`), and
every row opens on the blank `MENU_INDENT` inset. The band takes the footer's
slot, and the footer row is inset by the same two columns
(`  {model} · {cwd}`), so the help opens where the line it replaces began —
codex's overlay, one of its footer's modes, wears its footer's indent too. It
is also the column every other band below the box starts its names in, after
the palette's and the pickers' `→ ` marker.

#### As many columns as the width holds

The band was a fixed two-column grid, its second column at 32. At 87
columns — the terminal this redesign was asked for from — that left the
right third of the row empty and spent eight rows below the box on a
reference card. It now **spends a wider terminal on columns, not rows**.

80 columns, the classic default — three columns, six rows:

```
  / for commands             shift+enter for newline   ctrl+o for transcript
  ! for shell command        ctrl+v for image paste    ctrl+d for llm context
  @ for file paths           ctrl+w/u/k to kill text   ctrl+t to cycle thinking
  $ for skills               tab to queue next turn    shift+tab for permissions
  ↑ for input history        alt+↑ to edit queue       esc to quit
  ctrl+r to search history   ctrl+b to background      ctrl+c to quit
```

120 columns — four columns, five rows:

```
  / for commands           ctrl+r to search history      alt+↑ to edit queue         ctrl+t to cycle thinking
  ! for shell command      shift+enter for newline       ctrl+b to background        shift+tab for permissions
  @ for file paths         ctrl+v for image paste        ctrl+o for transcript       esc to quit
  $ for skills             ctrl+w/u/k to kill text       ctrl+d for llm context      ctrl+c to quit
  ↑ for input history      tab to queue next turn
```

From 149 columns, six columns of three rows. Under 53 the full wording
cannot make two columns, so the labels drop their `for`/`to` and the band
keeps two columns of nine rows — 50 columns:

```
  / commands                 tab queue next turn
  ! shell command            alt+↑ edit queue
  @ file paths               ctrl+b background
  $ skills                   ctrl+o transcript
  ↑ input history            ctrl+d llm context
  ctrl+r search history      ctrl+t cycle thinking
  shift+enter newline        shift+tab permissions
  ctrl+v image paste         esc quit
  ctrl+w/u/k kill text       ctrl+c quit
```

and under 46, where not even that fits, one entry per row in the full
wording again (one column costs the same rows either way):

```
  / for commands
  ! for shell command
  @ for file paths
  …
```

The layout (`ShortcutsGrid::for_width`, private to `ui/menu.rs`) is the
**fewest rows whose columns fit the width whole**:

- **Column-major, like `ls`.** The entries run down each column in
  `SHORTCUTS` order, so the list's order is what a column holds, and the list
  is grouped for it: the composer's sigils (`/ ! @ $`) and history, then
  editing and the running turn, then the views, the modes and the way out.
  At three columns those are exactly the three columns; at two, the
  composer's keys sit on the left and the session's on the right. A count
  that does not divide makes the **first** columns one entry longer (18 in
  four columns is 5, 5, 4, 4), so the gaps gather at the band's bottom right
  instead of leaving a column of two.
- **Columns sized by their content, measured over every context state.** A
  column is as wide as the widest text any of its entries ever shows — the
  esc entry's `esc esc to edit previous` included — so a turn starting under
  an open band (`esc to quit` → `esc to interrupt`) never slides the columns
  after it.
- **The fewest rows, but at least three.** Row counts are tried from
  `SHORTCUTS_MIN_ROWS` (3) up, each with the fewest columns that make it
  (`ceil(18 / rows)`: 3 rows are 6 columns, 4 are 5, 5 are 4, 6 are 3, 9 are
  2, 18 are 1), and the first whose inset, column widths and
  `SHORTCUTS_MIN_GUTTER` (2) gaps fit the width wins. The floor is there
  because a two-row ribbon across an ultra-wide pane reads as a toolbar to
  scan rather than a list to read.
- **The terse wording only where it buys a column.** Where the full
  wording cannot make even two columns, the same counts are tried with each
  label's leading ` for` / ` to` dropped (`terse_label` — every label leads
  with one, which a test holds the list to). With the key cyan and the rest
  dim, `ctrl+r search history` still reads as key and action, and it keeps a
  narrow pane's band at half the height: eighteen rows of one column at 51
  columns crowded a streaming reply's preview out of the strip entirely
  (`smoke.sh` Phase 99), where nine rows of two leave it room. The terse
  wording is never chosen where the full one fits in two columns or more,
  so resizing a normal terminal never flips the wording back and forth; and
  one column takes the full wording again, unless only the terse one fits
  whole (23 to 26 columns).
- **Width the layout does not need widens its gutters**, up to
  `SHORTCUTS_MAX_GUTTER` (6): at 80 columns the three columns sit three apart
  and the band ends at column 80; at 87, six apart, ending at 86. Past the cap
  a wider terminal leaves the band where it is instead of scattering it
  across the screen.

The breakpoints follow from the entries' own widths: two terse columns
from 46, two columns from 53, three from 78, four from 98, five from 125, six
from 149
(`the_band_spends_a_wider_terminal_on_columns_not_rows` pins a width in each
range). An entry is a column's width wherever it lands, so a long new entry
can move them, and that test says where to look. Two labels changed with the
layout. `shift+tab for permission mode` (29 columns) became `shift+tab for
permissions` (25), which is what lets three columns fit from 78, inside the
classic 80, rather than only from 82. And `ctrl+o for tool output` became
`ctrl+o for transcript`: Ctrl+O has opened the whole conversation's
transcript (its title row says `T R A N S C R I P T`) since the pager port,
and the band was the last place still naming the old tool-output view.

`shortcuts_rows` is the built line count of the whole band — the palette's
height-is-the-line-count rule (`docs/ui/view-flow.md`) — so the reserved rows
and the painted rows agree at every width. The band takes its rows ahead of
the streaming strip (the composer's floor is the only thing `live_layout`
holds back first), so on a 51×24 pane with a reply streaming, the composer,
its rules and the status line all stay on screen while the reply's preview
yields the rows the band needs until the band closes (`smoke.sh` Phase 99).

#### A terminal too short for the band

The band's ask is decided by the width alone, but a terminal can be too
short to grant it: on a 40×10 pane the one-column band asks for eighteen
rows and the region, clamped to ten, keeps three for the composer. The paint
is handed the band area it actually got (`shortcuts_lines(…, width,
height)`), and a band with more rows than that shows the rows that fit but
the last, and on the last a dim `… +12 more` counting the entries left out —
where the clip used to drop the bottom of the list, the way out among it,
without a word. The ask never depends on the height, so the reserved rows
and the paint still agree; only the paint knows it was short-changed. The
marker is the permission prompt's `… +N more waiting` idiom
(`SHORTCUTS_MORE_MARKER`): a static overview has no window to scroll, so the
row names what is hidden rather than pointing at it.

#### The entries

The `SHORTCUTS` const in `ui/theme.rs` is the single source of truth — 18
entries, in reading order. `@ for file paths` (`docs/input/file-search.md`, the one
sigil the band did not name) and `ctrl+b to background`
(`docs/tools/background.md` — the running command, or the foreground agents, moved
to the background) joined it with the responsive layout. The `$`
skill-mention sigil (`docs/input/skill-mentions.md`) closes the sigils at the head
of the first column, an ordinary entry. The Ctrl+D view's classifier page has
no entry of its own: Tab is discovered on the view's own hint row,
`docs/state/permissions.md`.

The Esc entry is three-way context-sensitive (codex's quit entry): `esc to
interrupt` (`SHORTCUTS_INTERRUPT`) while a turn is in flight, `esc esc to
edit previous` (`SHORTCUTS_BACKTRACK`) when idle with a previous user message
to edit (`docs/input/backtrack.md`), and `esc to quit` only with nothing to
backtrack to.

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
- **The column count follows the terminal.** Codex's overlay is two columns
  at every width and Claude Code's three; ours is one to six, whichever is
  the fewest rows the width holds whole.

## Testing

- `app`: `?` toggles on/off with an empty composer (shift-modified too) and
  types into a non-empty draft; any other key closes the band but still
  performs its action (typing, ↑ recall, `/` palette); Esc only dismisses —
  idle (no quit) and mid-turn (no interrupt, band closed, turn untouched);
  `?` is ignored in the tool view.
- `ui`: `shortcuts_rows` is 0 closed and the built line count open at every
  width (six rows at 80); the columns and rows at a width in each range (one
  column of 18 at 40, 2×9 at 50 and 60, 3×6 at 80 and 87, 4×5 at 100 and 120,
  5×4 at 140, 6×3 at 200 and 260); from the one-column width up no row is wider than
  its terminal, and a wider terminal never costs a row; the order is
  column-major and an uneven split shortens the last columns; the columns hold
  still across every esc context state and keep at least a two-column gutter;
  the gutters widen with spare width (2 at the three-column breakpoint, 3 two
  columns wider, 6 at 87, capped at 200); at every width from 20 to 260,
  reading down the columns left to right reads every binding exactly once,
  in order; under 53 the terse wording keeps
  two columns (from 46, the esc swaps shortened too, every label leading with
  the connective it drops), and under 46 one entry per row, in order, in the
  full wording, falling back to the terse one only where the full one would
  not fit whole; every row opens on the blank `MENU_INDENT` inset (the footer's width);
  keys cyan, labels dim, gaps unstyled; every binding listed once; the Esc
  entry flips three ways; a band cut by its height ends on `… +N more`
  counting the entries left out, in one column and in three; `render_live`
  paints the cut band on a 40×10 terminal and the stacked band on a 40×30
  one, draws the band below the box, and `cursor_position` stays put when it
  opens.
- `src/tui/` (smoke, Phase 11): `?` shows the band — three columns at 80,
  inset two columns — `?` again hides it, and typing a draft containing `?`
  ends up in the input box untouched; at 120 columns the band is four columns
  of five rows; on a 50-column pane it is two terse columns of nine rows; on
  a 40-column pane it lists one entry per row, every entry whole; and on a
  40×10 pane it ends on its `… +12 more` row with the composer still on
  screen. Phase 99 opens it at 51×24 under a streaming table and finds the
  table, the status line, the composer and the band on screen together.
