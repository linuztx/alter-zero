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
bindings in codex's phrasing, keys cyan and labels dim (`SHORTCUTS_*`
consts), but lays them out the way Claude Code's `?` help does: in
**groups**, a column each, read top to bottom. On an 80-column terminal:

```
  / for commands             esc to quit                 shift+enter for newline
  @ for file paths           ctrl+c to quit              ctrl+v for image paste
  $ for skills               shift+tab for permissions   ctrl+w/u/k to kill text
  ! for shell command        ctrl+t to cycle thinking    tab to queue next turn
  ↑ for input history        ctrl+o for transcript       alt+↑ to edit queue
  ctrl+r to search history   ctrl+d for llm context
```

The first column is what the composer opens and recalls — the four sigils,
`/`, `@`, `$` and `!`, then the history keys — the second the session and
its views, the third editing and the queue. `SHORTCUT_GROUPS` in
`ui/theme.rs` is the single source of truth.

The band used to be a fixed two-column grid, its entries paired two to a row
in declaration order with the second column at 32. That put side by side
whatever the order happened to pair — `shift+enter for newline` beside
`ctrl+o for tool output`, `$ for skills` beside `ctrl+w/u/k to kill text` —
it took eight rows, and on any terminal wider than the 61 columns it needed,
everything right of that was blank: 26 columns at 87, beside a band taking
eight rows off the screen. Now the band spends the terminal's width before
its rows.

#### The layout follows the width

`shortcut_columns(width)` picks the layout with the **fewest rows** that fits
the width:

- **A column per group: six rows, from 80 columns.** The groups side by side,
  as above. The band is as tall as its tallest group.
- **Each group split across two columns: three rows, from 151.** A group is
  cut into columns of one height, half the tallest group's, so it never
  shares a column and its halves read top to bottom, then left to right:

  ```
    / for commands     ! for shell command        esc to quit                 ctrl+t to cycle thinking   shift+enter for newline   tab to queue next turn
    @ for file paths   ↑ for input history        ctrl+c to quit              ctrl+o for transcript      ctrl+v for image paste    alt+↑ to edit queue
    $ for skills       ctrl+r to search history   shift+tab for permissions   ctrl+d for llm context     ctrl+w/u/k to kill text
  ```

  The same rule goes on past that: a third of the tallest group (two rows)
  from 221 columns, one row from 393.
- **Two columns: nine rows, from 54 to 79.** Too narrow for the groups side
  by side, every binding flows down two columns in reading order, the first
  filled before the second:

  ```
    / for commands              ctrl+t to cycle thinking
    @ for file paths            ctrl+o for transcript
    $ for skills                ctrl+d for llm context
    ! for shell command         shift+enter for newline
    ↑ for input history         ctrl+v for image paste
    ctrl+r to search history    ctrl+w/u/k to kill text
    esc to quit                 tab to queue next turn
    ctrl+c to quit              alt+↑ to edit queue
    shift+tab for permissions
  ```

- **One column: seventeen rows, under 54.** One binding a row, in the same
  order, each still inset:

  ```
    / for commands
    @ for file paths
    $ for skills
    …
  ```

  Taller, but nothing on it is cut mid-word. The paint's clip used to cut
  the old grid's second column at the edge (`shift+tab for perm` at 50
  columns, six of eight rows cut at 51), on the band whose whole job is to
  be read.

Each column is as wide as its widest entry, and the next starts
`SHORTCUTS_GUTTER` (3) past it. The inset is `MENU_INDENT`, a blank span
wearing no colour. The band takes the footer's slot, and the footer row is
inset by the same two columns (`  {model} · {cwd}`), so the help opens where
the line it replaces began — codex's overlay, one of its footer's modes,
wears the footer's indent too. It is also the column every other band below
the box starts its names in, after the palette's and the pickers' `→ `
marker, so whichever band is open, its text starts two in from the edge.

The `esc` entry reads three ways (below), so its column is sized by its
widest reading, `esc esc to edit previous`. That is what keeps the band
still when a turn starts or ends: the layout chosen and every entry's place
are the same in all three states, and only the words change. Content-sized
columns replace the old fixed `SHORTCUTS_COL`, which once left its widest
first-column entry a single space from its neighbour, so the two read as one
run-on line.

The grouped layout needs exactly 80 columns, 2 + 24 + 3 + 25 + 3 + 23, and
the labels were chosen to fit it: `shift+tab for permission mode` (29) became
`shift+tab for permissions` (25), which is what lets a standard terminal hold
every group side by side. `every_group_has_a_column_of_its_own_on_an_80_column_terminal`
fails if an entry added or reworded later widens a column past that. Adding
an entry also lengthens its group, and the band is as tall as its tallest
group, so a group grown past six costs every layout a row: add to the
shortest. Two more entries changed with the layout. `@ for file paths` is
new: the `@` file picker (`docs/file-search.md`) was the one composer sigil
the band did not list. And `ctrl+o for tool output` became `ctrl+o for
transcript`: Ctrl+O has opened the whole conversation transcript (its title
row says `T R A N S C R I P T`) since the pager port, and the band was the
last place still naming the old tool-output view. The Ctrl+D view's
classifier page has no entry of its own: Tab is discovered on the view's own
hint row, `docs/permissions.md`.

`shortcuts_rows` is the built line count — the palette's
height-is-the-line-count rule (`docs/view-flow.md`) — so the reserved rows
and the painted rows agree at every width. The band takes its rows ahead of
the streaming strip (the composer's floor is the only thing `live_layout`
holds back first), so on a 51×24 pane with a reply streaming, the composer,
its rules and the status line all stay on screen while the reply's preview
yields the rows the band needs until the band closes (`smoke.sh` Phase 99).

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
  The band's layout is decided by the width alone, so where fewer than its 17
  rows are left under the composer (a terminal under 54 columns *and* under
  about 20 rows) the last entries do not show — at 50×16 the final four.
  Choosing the layout by the height too would make the reserved rows depend
  on a size the paint does not see the same way, and it would not help here:
  no layout but the one column fits that width. The case is rare: a terminal
  that narrow is usually a split pane on a taller screen.
- **Claude Code's layout, not codex's.** codex lays its overlay out in two
  fixed columns; the grouped columns are Claude Code's `?` help, which also
  lists `@ for file paths`.

## Testing

- `app`: `?` toggles on/off with an empty composer (shift-modified too) and
  types into a non-empty draft; any other key closes the band but still
  performs its action (typing, ↑ recall, `/` palette); Esc only dismisses —
  idle (no quit) and mid-turn (no interrupt, band closed, turn untouched);
  `?` is ignored in the tool view.
- `ui`: `shortcuts_rows` is 0 closed and the tallest group open on an
  80-column terminal; there every group is a column of its own, read top to
  bottom, the second starting one gutter past the first column's widest
  entry, and the composer's four sigils head the first. At 200 columns each
  group splits across two columns in half the rows; at 60 the bindings flow
  down two columns in reading order; under 54 (pinned) one entry per row, in
  order. At every width from 1 to 400: the band never grows as the width
  does, lists every binding exactly once, keeps every row inside the width
  once the width holds the widest entry, and holds still across the four
  `(turn_active, can_backtrack)` states, with at least `SHORTCUTS_GUTTER`
  between entries. Every row opens on the blank `MENU_INDENT` inset (the
  footer's width), keys cyan and labels dim with unstyled gutters; the Esc
  entry flips between `to quit`, `to interrupt` and `esc esc to edit
  previous` with the context; `live_height` grows by the band; `render_live`
  paints it below the box, the last entry on the last row, and
  `cursor_position` stays put when it opens.
- `src/tui/` (smoke, Phase 11): `?` shows the band (`for commands` visible),
  inset two columns with a column per group on an 80-column pane (`@ for
  file paths` under `/ for commands`, six rows), `?` again hides it, typing a
  draft containing `?` ends up in the input box untouched; a 60-column pane
  flows the band into two columns, a 50-column pane lists one entry per row,
  every entry whole, and a 160-column pane splits each group across two
  columns in three rows.
