# Colour themes and the `/theme` picker

Every colour the TUI paints — the cyan the pickers select with, the tool
bullet's green, the `⎿` gutter's dim, the user bubble's ground, the diff
tints, the banner gradient, and the syntax colours inside code blocks and
file cells — comes from one **theme**. The **`/theme`** command — the tenth
composer-replacing inline picker, the `/spinner` picker's twin — chooses
among eleven, previews them on real cells, and persists the choice across
sessions. The default is **Catppuccin Mocha**: the code blocks have worn it
since the syntect port (`docs/markdown.md`), and the chrome now matches
instead of framing Catppuccin code in One Dark's cyan.

## The catalog

| name        | title                | family                                       | code theme (two-face)  |
| ----------- | -------------------- | -------------------------------------------- | ---------------------- |
| `mocha`     | Catppuccin Mocha     | the darkest flavour — **the default**        | `Catppuccin Mocha`     |
| `macchiato` | Catppuccin Macchiato | the medium-dark flavour                      | `Catppuccin Macchiato` |
| `frappe`    | Catppuccin Frappé    | the lightest of the dark flavours            | `Catppuccin Frappe`    |
| `latte`     | Catppuccin Latte     | the **light** flavour, for a light terminal  | `Catppuccin Latte`     |
| `onedark`   | One Dark             | this TUI's original chrome, value for value  | `TwoDark` (Atom's)     |
| `dracula`   | Dracula              | vivid purple, pink and cyan on a dark ground | `Dracula`              |
| `nord`      | Nord                 | arctic, bluish and calm                      | `Nord`                 |
| `gruvbox`   | Gruvbox Dark         | retro groove, warm and earthy                | `gruvbox-dark`         |
| `solarized` | Solarized Dark       | Ethan Schoonover's precision palette         | `Solarized (dark)`     |
| `monokai`   | Monokai              | the classic editor palette                   | `Monokai Extended`     |
| `ansi`      | Terminal             | the terminal's own 16 ANSI colours           | `ansi`                 |

Each entry is **one design system**: its chrome palette and its code theme
come from the same family, so a reply's code and the frame around it agree.
That coherence is the reason the catalog is a fixed list rather than a
free-form file: a user-authored palette would need a matching syntect theme
to be worth having, and the two-face bundle already ships one for every
family above.

The catalog's **identity** — names, titles, descriptions, order, the
`from_name` round-trip `theme.json` reads back through — is the pure
`app::Theme` enum (`Default` = `Mocha`), the `app::Spinner` rule. Its
**look** — every theme's palette — is styling, so it lives in `ui` beside
every other styling decision (`src/ui/palette.rs`), and the syntect half in
`highlight::CodeTheme`, one variant per catalog entry, paired by position
(`every_theme_pairs_its_chrome_with_its_own_code_theme` pins the pairing).

## The palette: twenty-one roles

A theme is a `Palette` — a table of colours by **role**. Every colour the
chrome paints is one of these or derived from one:

| role | what wears it |
| --- | --- |
| `text` | the reply text, the composer prompt, a tool's name/arguments/output, the shimmer's crest, the comet's head |
| `text_muted` | an unselected model id, a settings value, the comet's mid-tail, the thinking header's shimmer floor |
| `dim` | the `⎿` corners and placeholders, the footer, the hints, the counters, a waiting or resting-running bullet, quotes, timestamps — and the top of a running bullet's breath |
| `border` | the composer box's rules and every framed view's |
| `user_fg` / `user_bg` | the `❯ …` user bubble (and a `!` command's header) — muted on purpose |
| `selection_bg` | the `/resume` picker's selected row |
| `accent` | what every picker selects with, the system bullet, inline code, the banner hint, a permission prompt's title, the Ctrl+R query, the ↓-focused footer chip's fill — and the gradient's near end |
| `on_accent` | ink over an `accent` fill (the footer chip, the current ask-question chip) |
| `link` | a link's URL, an ordered list's marker, the context view's user tag — and the gradient's far end |
| `success` | the finished tool bullet, a diff's `+`, the active model's ✓, a completed task, a background notice that went well |
| `error` | the error bullet, a failed tool, a diff's `-`, the `!` shell mode, an error toast |
| `warning` | the `retrying n/m` clause, the context view's system tag, the ask review's unanswered warning |
| `purple` | the context view's tool tag, the `/resume` toolbar's focus |
| `diff_add_bg` / `diff_del_bg` | an added / removed numbered row's ground |
| `diff_add_mark_bg` / `diff_del_mark_bg` | the brighter marks under the characters that actually changed (`docs/inline-diff.md`) |
| `shimmer_base` | the status verb's resting grey, under the sweep to `text` |
| `pulse_dim` | the bottom of a running bullet's breath |
| `code` | the `highlight::CodeTheme` the code blocks and file cells are coloured with |

The **derivations** are the accessor functions in `ui/theme.rs`
(`tool_ok_color()` is `success`, `menu_selected_color()` is `accent`,
`header_gradient_start()`/`header_gradient_end()` are `accent` → `link`,
`shimmer_highlight()` is `text`, `tool_pulse_bright()` is `dim`, and so
on): the roles are what `theme.rs` has always named, the palette is what
each theme paints them with, so a new theme is one table of twenty-one
values and nothing else. The Catppuccin flavours take the flavour's own
roles (`text`/`subtext1` inks, `overlay1`/`overlay2` for the dim and the
rules, `surface0`/`surface1` grounds, `sky` for the accent, `blue` for the
link, `mauve` for the purple) and Catppuccin's own DiffAdd/DiffDelete rule
for the tints — the hue mixed 18% into `base` for a row, 42% for a mark
(Solarized's `base03` is so dark a blue that it mixes 25%/50%). `onedark`
is the chrome the TUI shipped with, every RGB it used to hard-code, with
only the code changed — Atom's One Dark instead of the Catppuccin Mocha the
blocks wore under it — so whoever liked the old look keeps it
(`one_dark_is_the_original_look_value_for_value`).

Two entries behave differently, and their descriptions say so, since the
picker's description row is where a user learns why a theme looks wrong on
their terminal:

- **`latte`** is for a **light** terminal: its inks are dark, its grounds
  and tints pale, and the "bright" end of every blend is the *darker*
  colour, because on a light ground that is the one that stands out
  (`the_light_theme_inverts_the_inks_and_the_tints`). On a dark terminal it
  reads washed out — the terminal's background is not ours to paint.
- **`ansi`** names **no RGB at all** — Claude Code's "ANSI colors only"
  mode: `Cyan`, `Green`, `Red`, `Yellow`, `Blue`, `Magenta`, `DarkGray`,
  `Gray`, `Reset`, and bat's `ansi` syntect theme, whose scopes resolve to
  palette *indices* (`highlight::convert_syntect_color` decodes bat's
  alpha-channel encoding, a path that had been dormant since the port).
  So the TUI follows whatever palette the terminal is configured with —
  the one theme that makes a Solarized-terminal user's TUI Solarized
  without saying so. What sixteen colours cannot express goes without: the
  diff rows keep the terminal ground (only the marks tint, both on the
  bright-black, so a row's sign says which), the gradient and the shimmer
  **step** between their two ends instead of blending, and the running
  bullet holds still, its breath's two ends being the same bright-black.

That stepping is the one generalisation the blends needed:
`ui::wrap::lerp_color` and `blend_color` take `Color`s rather than RGB
triples and mix when both ends are `Rgb`, else pick the nearer end
(`the_gradient_and_the_blends_follow_the_palette`). Every consumer — the
mascot gradient, the sparkle/blocks/gravity/wave spinners, the tool pulse,
the status shimmer — is unchanged in shape.

## The ambient active theme

`ui/theme.rs` used to hold every colour as a `const`; a theme that can
change at runtime cannot. The colours are **accessor functions** now
(`tool_ok_color()` where the `TOOL_OK_COLOR` const was — the same names, lowercased,
so every call site and every doc comment kept its meaning), each reading
the **active theme's** palette. The active theme is a **thread-local**
(`ui::palette`), not a parameter and not a process global:

- **Not a parameter**, because the palette is read at some four hundred
  call sites, most of them in builders that take no `&App` (`message_lines`,
  `tool_lines`, `wrap`'s helpers, the file cell, the inline diff). Threading
  a `&Palette` through all of them would have rewritten every signature in
  `ui` for a value that changes twice a session. The renderers stay pure in
  the sense that matters — same inputs, same rows — and the palette is an
  input the boundary injects once (`ui::activate_theme`), the way the clock
  and the path-display policy are injected, rather than one it threads.
- **Not a process global**, because the tests run on parallel threads and
  a global would let one test's theme recolour another's assertions. Each
  thread's cell starts at the default; a test renders under another theme
  through `ui::with_theme(theme, || …)`, which scopes it and restores the
  previous theme on the way out — on a panic too, via a drop guard, so an
  assertion failing inside the scope cannot recolour the tests after it
  (`with_theme_restores_even_when_the_closure_panics`). The footer's queued
  memo already keeps its state per thread for the same reason.

The event loop draws on one thread, so one cell is the whole of the app's
state; `tui::bootstrap` activates the saved theme before the banner is
built, and `tui::theme::Session::select_theme` activates the new one before
the rebuild that repaints in it. The pure `App::theme` records the same
choice — the picker's ✓ and the open's seat read it — and the two are kept
equal at the boundary.

**Everything that caches rendered rows keys on the active theme**, because
a row's colours are baked into it: the Ctrl+O `TranscriptCache`'s frozen
prefix and its signature, the Ctrl+D `ContextCache`'s signature, and the
footer's queued-rows memo all carry `active_theme()`, so a switch rebuilds
them on their next read rather than serving rows in the old palette (the
three `…_when_the_theme_changes` tests). The `StreamRender` needs no key:
every purge rebuild resets it and re-commits the in-flight partial through
the new palette (invariant 3), and a `highlight::Highlighter` keeps the
`CodeTheme` it opened with (`a_highlighter_keeps_the_theme_it_opened_with`)
so a block half-rendered when the theme switches stays one scheme until
that rebuild re-renders it whole.

### The highlighter

`highlight::Highlighter::new(lang, code)` takes the `CodeTheme` explicitly
— `highlight` sits below `ui` and reads no ambient state; `ui` passes
`palette().code` at its five construction sites, and `plain_style(code)`
the same way. The two-face bundle is held whole in one `LazyLock` (its
`themes.bin` is 64 KB — smaller than one parsed grammar) and each theme's
`syntect::Highlighter` is built on first use into a `OnceLock` per
`CodeTheme`, borrowing its theme out of the bundle for the process's life,
which is what lets a `Highlighter`'s carried `HighlightState` hold a
`'static` reference to the highlighter it was opened against. A `/theme`
browse touches several; each costs one parse of a ~20 KB theme, once.

## The `/theme` picker

```
────────────────────────────────────────────────────────────────
  ❯
  → mocha       ●●●●● ✓
    macchiato   ●●●●●
    frappe      ●●●●●
    latte       ●●●●●
    onedark     ●●●●●
    dracula     ●●●●●
    nord        ●●●●●
    gruvbox     ●●●●●
    solarized   ●●●●●
    monokai     ●●●●●
  (1/11)

  ❯ Rename the greeting in greet.py
  ● Edit(greet.py)
    ⎿  Updated greet.py (+1 -1)
        1  def greet(name):
        2 -    print("Hello, world")
        2 +    print(f"Hello, {name}")
  ● Done — greet.py greets by name now:
    greet("Alter Zero")  # Hello, Alter Zero

  Catppuccin Mocha — the darkest flavour, and the default
  Type to search · Enter to choose · Esc to cancel
────────────────────────────────────────────────────────────────
```

Deliberately the `/spinner` picker's twin — same inline frame, same `❯`
type-to-search (matching names, **titles** and descriptions, so
`catppuccin` finds the four flavours and `follows` the terminal theme),
same `→` marker in the active accent, same `(n/total)` counter, description
row and dim hint, the same ten-row window (`THEME_MENU_MAX_ROWS`, the
settings window's size — the eleven-theme catalog windows by one row) —
plus the two things a theme picker exists for:

- **Every row wears its own swatch.** After the name column (sized to the
  widest visible name plus `THEME_MENU_GAP`) each row carries five `●`s in
  *that theme's* accent, link, success, warning and error — read straight
  off its palette, never the active one — so the catalog compares at a
  glance the way the `/spinner` rows' live spinners do. The session's
  current theme carries the `/model` picker's green ✓, and the open seats
  the highlight on it.
- **The highlighted theme previews on real cells.** The preview is a
  three-cell sample conversation — a user bubble, an `Edit` diff cell, an
  assistant reply with inline code and a fenced code line — built by
  `message_lines` and `tool_lines`, the conversation's *own* builders, with
  the highlighted theme scoped active around them (`with_theme`). So one
  glance shows the bubble's ground and ink, the success bullet, the code
  theme (the diff's context row and the reply's code line are
  syntax-highlighted), the diff tints and the inline-diff marks, and the
  accent on the inline code — and what the picker shows and what Enter
  produces can never disagree, since they are the same rows. The diff cell's
  output is the executor's own `llm::tools::update_report` string, so it
  parses and paints exactly as a live `edit` does. The scope ends before the
  page around it is built: the frame, the rows and the hint keep the
  **active** theme, only the sample wears the highlighted one
  (`the_preview_wears_the_highlighted_theme_and_the_frame_the_active_one`).

The sample is the same shape for every theme, so the page height never
depends on the selection — the `/settings` always-emit rule
(`the_page_height_is_stable_across_selections`) — and the page is **still**
(nothing on it ticks), so unlike the `/spinner` picker it asks for no
animation frames and its scrollback flow is signed on its **rows**, like the
`/mascot` picker's: any keystroke re-flows it (`docs/view-flow.md`).

A search that matches **nothing** collapses to its essentials — the
placeholder (`No matching themes`), one blank gap, the hint — the `/mascot`
picker's rule, from the `/model` picker's `model_has_detail`.

Keys: ↑/↓ move, **wrapping at the ends** (the shared `wrap_step` grammar);
PageUp/PageDown/Home/End jump (clamping); printable keys filter (Backspace
pops, a keystroke reseats the highlight on the first match); **Enter or
Space chooses and closes** (so a space never reaches the search — one word
per query); Esc clears the query first, then closes; Ctrl+C closes. The
picker owns every key while open (routed at the top of `App::on_key`,
before the composer's global Ctrl+C/Ctrl+O), pastes are swallowed (nothing
anyone pastes is a theme name), and the running cell's `(ctrl+b to run in
background)` hint is blanked while it is open (`App::background_hint_elapsed`, the
rule every composer-replacing picker follows). It works **mid-turn** like
`/mascot`: it only replaces the composer, the streaming strip keeps its rows
above it, and a switch never touches the running turn — the purge rebuild
re-commits the in-flight partial in the new colours. On a short terminal the
page bottom-anchors and flows its top into scrollback like its whole family.

## Applying a selection

`Action::SelectTheme(theme)` — the pure side already moved `App::theme` and
closed the picker; the boundary (`tui::theme::Session::select_theme`):

1. **persists** `{config_home}/theme.json` (`{"theme": "mocha"}` — the pure
   format is `app::theme_file_json`/`parse_theme_file`; best-effort I/O in
   `tui::config::save_theme`, the `save_spinner` posture),
2. **activates** the palette (`ui::activate_theme`) so every renderer reads
   it from here on,
3. raises the confirming `Theme: {name}` toast, and
4. **purge-rebuilds** (`repaint_active_view`): the colours of every
   committed row are baked into scrollback, so the one way to recolour the
   conversation is the standard rebuild from history — the banner, every
   cell, the in-flight partial — under the palette just made ambient, the
   toast riding the rebuilt frame. The transcript and context caches rebuild
   on their next read (keyed on the theme, above). The switch is visible at
   once, exactly like a resize's repaint; `smoke.sh` Phase 111 reads the
   banner's `/login` hint off the raw SGR bytes before and after and sees
   Mocha's sky become Dracula's cyan.

At startup `tui::bootstrap` seeds `App::theme` from `theme.json` and
activates it **before the banner is built** (an absent or corrupt file keeps
the default — never a startup failure), so the first frame already wears
the saved theme.

## Design notes

- **Why is Catppuccin Mocha the default, not the old look?** The code
  blocks had been Catppuccin Mocha since the syntect port while the chrome
  around them stayed One Dark's cyan — two palettes in one frame. A theme
  system whose default was itself incoherent would have been a poor
  advertisement for it. The old chrome survives as `onedark`, now paired
  with One Dark code, so the choice is one row away.
- **Why a fixed catalog and not a palette file?** Coherence, above: a
  palette without a matching syntect theme colours the chrome one way and
  the code another, which is the very thing the feature exists to end. The
  eleven cover the families the two-face bundle ships; adding one is a
  `Theme` variant, a `Palette` table and a `CodeTheme` mapping.
- **Why accessor functions and not a `Palette` parameter?** Four hundred
  call sites in builders that take no `&App`; see *The ambient active
  theme*. The `const` names became functions of the same name so that no
  call site changed meaning, only spelling.
- **Why a thread-local and not a `static`?** Test isolation; see above. The
  app draws on one thread, so nothing is lost.
- **Why does the preview render real cells rather than swatches?** A
  swatch says what the colours *are*; a cell says what they *look like
  together* — the accent on inline code beside the dim gutter, the diff
  tints under syntax colours. And rendering through the real builders is
  the picker family's rule: the `/mascot` preview is the banner's own
  builder, the `/spinner` preview the status line's.
- **Why is the ANSI theme in the catalog at all?** It is the one theme that
  follows the *terminal's* theme, which for a user who has already themed
  their terminal is the only theme that matters. The cost was making the
  blends step for a non-RGB end, which is a few lines.
- **Why aren't the spinner frames or the mascot art themed?** They are
  glyphs, not colours; the theme colours them (the gradient and the pulse
  come from the palette), and `/mascot` and `/spinner` remain the pickers
  for the glyphs.

## API

- `app::Theme` — the catalog (`ALL`, `name`, `title`, `description`,
  `is_light`, `from_name`), `Default` = `Mocha`.
- `app::ThemePicker` / `ThemeRow` — the open picker's state and one derived
  row; `App::theme()`, `set_theme`, `open_theme_picker`,
  `close_theme_picker`, `theme_rows`, `highlighted_theme`,
  `on_key_theme_picker`.
- `app::theme_file_json` / `parse_theme_file` — the `theme.json` format.
- `highlight::CodeTheme` — the syntect side (`ALL`, `name`);
  `Highlighter::new(lang, code)`, `highlight(lines, lang, code)`,
  `plain_style(code)`.
- `ui::activate_theme` / `active_theme` / `with_theme` — the ambient active
  theme; `ui::palette::Palette` and `palette_of(theme)` (crate-internal),
  the accessor functions in `ui::theme`.
- `ui::wrap::{lerp_color, blend_color}` — the blends over `Color`.
- `ui::theme_view` — `theme_view_lines` (the page builder; its length is
  the reserved height, `docs/view-flow.md`), `theme_picker_height`,
  `render_theme_picker`.
- `tui::theme::Session::select_theme`, `tui::config::{theme_json_path,
  load_theme, save_theme}`.

## Tests

- `app/tests/theme.rs` — the catalog (eleven themes, mocha first and
  default, distinct bare-word names, titles opening the descriptions, the
  light and terminal themes saying so, `from_name` round-trip), the
  persistence format, the `/theme` command opening the picker (listed ahead
  of `/mascot`), no animation request, the Ctrl+B hint blanked, and the
  whole key grammar (filter over names/titles/descriptions, the wrapping
  ↑/↓ against the clamping jump keys, Enter/Space select, Esc/Ctrl+C,
  owns-every-key, mid-turn use leaving the turn untouched).
- `ui/tests/palette.rs` — the default on every thread, `with_theme`'s scope
  and its panic-safety, `activate_theme`, the chrome/code pairing, distinct
  semantic hues per theme, One Dark value for value, the ANSI theme naming
  no RGB and its stepping blends, the light theme's inverted inks and
  tints, the gradient/blend derivations, and a rendered cell wearing the
  active theme.
- `ui/tests/theme_view.rs` — the framed page (rules, search, rows, counter,
  preview, description, hint), the preview's real cells, every row's own
  swatch, the preview in the highlighted theme against the frame in the
  active one (bullet, code keyword, diff tints, bubble ground), the stable
  height, the active ✓ and its seat, the no-match collapse, no stacked
  blanks, width safety, the height contract, the flow and its re-signing,
  and the bottom-anchored render.
- `ui/tests/transcript.rs`, `context_view.rs`, `footer.rs` — the three
  caches rebuilding when the theme changes.
- `highlight` — every code theme resolving, two themes disagreeing on a
  keyword, the ANSI theme's palette colours, a highlighter keeping its
  theme.
- `scripts/smoke.sh` Phase 111 — the picker end to end in a real terminal:
  the banner hint in the default accent (raw SGR), the palette entry, the
  open page with its swatches and real-cell preview, ↓ moving the counter
  and description, a filtered Enter switching the theme with a toast and a
  `theme.json` write, the **already-painted banner recoloured** by the
  rebuild, and a **second process against the same config home launching
  in the saved theme**.
