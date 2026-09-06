# The banner mascot and the `/mascot` picker

The startup banner draws a small block-character **mascot** beside the session
metadata (see `docs/header.md` for the banner itself). The mascot is chosen
with the **`/mascot`** command — the eighth composer-replacing inline picker —
and the choice persists across sessions, **per working directory**
(`docs/per-directory-state.md`): the repo that wears the gem beside the
scratch directory that wears the hatchling.

## The catalog

Six mascots, each three rows of single-width quadrant/half-block glyphs:

| name      | creature                                |
| --------- | --------------------------------------- |
| `crest`   | a crested hatchling (the default)       |
| `bloom`   | a round creature in full bloom          |
| `sprout`  | a seedling pushing up its first leaf    |
| `twin`    | two tufted ears over one shared grin    |
| `skiter`  | a many-legged skitterer                 |
| `gem`     | a cut gem                               |

The art lives on the pure `app::Mascot` enum (`name()`, `description()`,
`art()`, `art_width()`, `Mascot::ALL`); every row is single-width (a wide
emoji/CJK glyph would shear the rows after it — `docs/table-streaming.md`
*Wide glyphs* — and the catalog tests pin it). The mascot wears the banner's
own accent → link gradient (`header_gradient_start()` →
`header_gradient_end()` — the theme's, `docs/theme.md`), keyed by display
column across the art block's width.

## The `/mascot` picker

```
────────────────────────────────────────────────────────────────
  ❯
  → crest ✓
    bloom
    sprout
    …
  (1/6)

   ▙▄▙▄▟▄▟   Alter Zero (v0.1.0)
  ▝▜▄███▄▛▘  ~/inline-tui
    ▘▘ ▝▝    /login   /model   /resume

  A crested hatchling flaring its frill
  Type to search · Enter to choose · Esc to cancel
────────────────────────────────────────────────────────────────
```

Deliberately the `/settings` menu's twin — same inline frame, same `❯`
type-to-search (matching names *and* descriptions), same `→` marker with the
cyan selection, same `(n/total)` counter and dim hint — plus the one thing no
sibling has: a **live banner preview** in the page's centre. The preview is
built by the *same* `ui::header` builder the startup banner uses
(`header_lines_for(app, mascot, width)`), inset two columns, so what the
picker shows and what Enter produces can never disagree — the cwd, the
version, the hint and the gradient all preview exactly. The slot is padded to
the tallest banner any mascot would draw at that width, so the frame can
never jump as ↑/↓ move — today's catalog is uniform, but the padding is what
keeps that true of any mascot added later (the `/settings` always-emit rule). The `→ {name}` rows carry the `/model` picker's green `✓`
on the session's current mascot, and the open seats the highlight on it.

A search that matches **nothing** collapses to its essentials — the
placeholder, one blank gap, the hint — because there is no count, no banner
to preview and no description to show, and painting those slots as empty
rows left a band of blank lines mid-frame. It is the `/model` picker's
placeholder rule (`model_has_detail`), and the page is then exactly:

```
────────────────────────────────────────────────────────────────
  ❯ ?

  No matching mascots

  Type to search · Enter to choose · Esc to cancel
────────────────────────────────────────────────────────────────
```

Keys: ↑/↓ move, **wrapping at the ends** (the shared `wrap_step` grammar
every menu follows); PageUp/PageDown/Home/End jump (clamping); printable
keys filter (Backspace pops); **Enter or Space chooses and closes**; Esc
clears the query first, then closes; Ctrl+C closes. The picker owns every key while open (routed at
the top of `App::on_key`, before the composer's global Ctrl+C/Ctrl+O), and
pastes are swallowed (nothing anyone pastes is a mascot name). It works
**mid-turn** like `/settings` — it only replaces the composer, and the
streaming strip keeps its rows above it. On a short terminal the page
bottom-anchors and flows its top into scrollback like its whole family
(`docs/view-flow.md`).

## Applying a selection

`Action::SelectMascot(mascot)` — the pure side already moved `App::mascot`
and closed the picker; the boundary (`tui::mascot::Session::select_mascot`):

1. **persists** `{config_home}/mascot.json` — as **this working
   directory's** own choice *and* the last made anywhere, a read-modify-write
   over the file (`tui::config::save_look`, best-effort like every write
   there): the pure format is `app::MascotFile`, the `mascot.json` instance
   of the `app::LookFile` the two looks share (*Per directory* below),
2. raises the confirming `Mascot: {name}` toast, and
3. **purge-rebuilds** (`repaint_active_view`): the banner is chrome at the
   *top of scrollback*, outside `history` (`docs/header.md`), so the rebuild
   — which re-emits it via `ui::banner_tail` → `header_lines` — is the one
   way to redraw it. The switch is visible immediately, exactly like a
   resize's repaint; a mid-turn switch keeps the in-flight partial through
   the standard rebuild path (invariant 3).

At startup `tui::bootstrap` seeds `App::mascot` from `mascot.json` before the
first frame commits the header — the directory's own entry, or, launched in
for the first time, the last choice made anywhere, **pinned** as the
directory's own right then (`tui::config::adopt_look`); an absent or corrupt
file keeps the default, never a startup failure. The Ctrl+O transcript tops
itself with `header_lines` too, so it shows the switched mascot for free.

## Per directory

A mascot is a fact about a *project* as much as a `/model` choice is, so
`mascot.json` follows `config.json`'s rule (`docs/per-directory-state.md`):

```json
{
  "mascot": "sprout",
  "projects": {
    "/home/user/work/api": { "mascot": "bloom" }
  }
}
```

The top level is the **last** choice made anywhere — and exactly the
one-value file written before mascots were per directory, so an old file
still loads and seeds every directory. A directory launched in for the first
time takes the last and **pins** it as its own entry right then
(`LookFile::adopt`, written only when that changed), so a `/mascot` switch
in one terminal never moves the banner in another; a switch made in a
directory is that directory's entry *and* the new last (`LookFile::record`).
"A directory" is the process cwd exactly as every other per-directory file
keys it (`Session::cwd`, not the git root). The format is written once —
the generic `app::LookFile<T>` over the `app::Look` trait (`KEY`, `name`,
`from_name`), which `Mascot` and `Spinner` both implement — because the two
catalogs are twins by design; `MascotFile` is its `mascot` instance. The
`/theme` colours stay the user's: a theme is matched to the terminal, not to
a project.

## API

- `app::Mascot` — the catalog (`ALL`, `name`, `description`, `art`,
  `art_width`, `from_name`), `Default` = `Crest`.
- `app::MascotPicker` / `MascotRow` — the open picker's state and one derived
  row; `App::mascot()`, `set_mascot`, `open_mascot_picker`,
  `close_mascot_picker`, `mascot_rows`, `highlighted_mascot`,
  `on_key_mascot_picker`.
- `app::MascotFile` — the `mascot.json` format: `app::LookFile` over
  `app::Mascot` (`Look::KEY = "mascot"`) — `parse`/`to_json`, `project`,
  `choice_for`, `adopt`, `record`, over the `last` + `projects` fields.
- `ui::mascot_view` — `mascot_view_lines` (the page builder; its length is
  the reserved height, `docs/view-flow.md`), `mascot_picker_height`,
  `render_mascot_picker`; `ui::header::header_lines_for` renders the preview.
- `tui::mascot::Session::select_mascot`, `tui::config::{mascot_json_path,
  adopt_look, save_look}`.

## Tests

- `app/tests/mascot.rs` — the catalog (six mascots, crest first,
  single-width art, `from_name` round-trip), the per-directory persistence
  format (the old one-value file as the last choice seeding every directory,
  an entry outranking it, `adopt`'s pin-once, `record`'s entry-and-last, the
  round trip whose no-entry shape is byte-for-byte the old file, the lenient
  parse that drops one unknown name and never the file), the
  `/mascot` command opening the picker, and the whole key grammar (filter,
  the wrapping ↑/↓ against the clamping jump keys, Enter/Space select,
  Esc/Ctrl+C, owns-every-key).
- `ui/tests/mascot_view.rs` — the framed page (rules, search, rows, counter,
  preview, description, hint), the preview following the selection, the
  stable slot height, the active ✓, the no-match placeholder, width safety,
  the height contract, and flow eligibility.
- `ui/tests/header.rs` — the banner side (`docs/header.md`).
- `scripts/smoke.sh` Phase 86 — the picker end to end in a real terminal:
  open from the palette, preview follows ↓, filtered Enter switches the
  banner, the toast confirms, and a **second process against the same config
  home launches with the switched mascot** (persistence).
- `scripts/smoke.sh` Phase 114 — the per-directory rule end to end, for
  `/mascot` and `/spinner` together: two directories against one config
  home, a choice in the first recorded as its own and the last, the second's
  first launch pinning that last, a choice there never moving the first's
  banner, and a third directory taking the new last.
