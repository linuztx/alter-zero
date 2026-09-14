# The status spinner styles and the `/spinner` picker

The live status line opens with a **spinner** — the animated glyph(s) before
the shimmering verb (`⣤⣀⣀⣀⣀⣀⣀⣀ Working… (3s · ↓ 1.2k tokens · esc to
interrupt)`, `docs/status-indicator.md`). It used to be one animation, the
comet. The **`/spinner`** command — the ninth composer-replacing inline
picker, the `/mascot` picker's twin — chooses among nine, previews them
**live**, and persists the choice across sessions — **per working
directory**, the `/mascot` picker's rule (`docs/per-directory-state.md`).
The **default is `gravity`**, the ball hopping along its braille track. The
comet is still the catalog's first row, and `(●•·   )` still means the comet
wherever these docs sketch one — it is simply no longer what a session with
no saved choice opens with.

## The catalog

| name      | look                                                     | cadence |
| --------- | -------------------------------------------------------- | ------- |
| `comet`   | `(●•·   )` — a Larson-scanner sweep between dim walls     | 80 ms |
| `gravity` | `⣤⣀⣀⣀⣀⣀⣀⣀` — a ball hopping along a braille track, bouncing off both walls, accent → link (**the default**) | 2.4 s trip, 0.6 s hop |
| `wave`    | eight braille cells — a wave rolling down the track and reflecting off the walls, in the banner's wash | 3.4 s there and back |
| `sparkle` | `· ✢ ✳ ✶ ✻ ✽ ✻ ✶ ✳ ✢` — a spark blooming into a star, accent → link | 120 ms |
| `dots`    | `⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏` — the classic braille spinner            | 80 ms  |
| `blocks`  | `▙ ▛ ▜ ▟` — the mascots' quadrant glyphs turning, accent → link | 150 ms |
| `pulse`   | `●` — one dot breathing dim → white, the running tool bullet's breath | 1 s breath |
| `bars`    | `▁ ▂ ▃ ▄ ▅ ▆ ▇ █ ▇ …` — a level meter, brightening with height | 60 ms |
| `line`    | `\| / - \` — the classic ASCII spinner, for any font        | 100 ms |

The catalog's **identity** — names, order, descriptions, the `from_name`
round-trip `spinner.json` reads back through — is the pure `app::Spinner`
enum (`Default` = `Gravity`). The list's *order* and the default are
separate things — the picker seats its highlight on the **active** style
whatever its row, so the catalog can keep opening with the comet while a
session opens with the ball. Its **look** — every style's frames, cadence and
colour rule — is styling, so it lives in `ui/theme.rs` beside every other
styling decision (`SPINNER_*_FRAMES` / `_INTERVAL`, the track geometry, the
pulse and bars colour endpoints), and `ui::status::spinner_spans(spinner,
elapsed)` maps one to the other. Two rules every style keeps, pinned by
`ui/tests/status.rs`: every frame of a style is the **same width** (so the
verb after it never jitters — the comet's own rule,
`docs/status-indicator.md`) and every glyph is **single-width** (a wide glyph
would shear the verb and the metrics after it — `docs/table-streaming.md`
*Wide glyphs*).

### The two braille tracks

`gravity` and `wave` have no frame table. Each draws itself on a **braille
track** (`ui::status::Track`): eight cells — the comet's footprint, so the
three wide styles share one width — of 2 × 4 dots each, sixteen dot columns
by four dot rows inside a single text row. That is the resolution that lets
a ball visibly *hop* and a wave visibly *roll* where a glyph table can only
step. A cell is one glyph and so one colour: dim (`status_detail_color()`)
until something coloured lands on it. The pair is a port of the two braille
animations in a bouncing-indicator lab script — its canvas, its `tri`
ping-pong and its `arc` hop — retuned for a fixed eight-cell track and the
crate's palette.

- **`gravity`** lays a floor along the bottom dot row and hops a 2 × 2 dot
  ball along it. Horizontally the ball **ping-pongs** at constant speed with
  a hard reversal at each wall (`ui::wrap::ping_pong`, one round trip per
  `SPINNER_GRAVITY_SWEEP`); vertically it follows a parabola
  (`ui::wrap::hop`, 0 at take-off and landing, 1 at the apex, one hop per
  `SPINNER_GRAVITY_HOP`). The two periods are 2.4 s and 0.6 s — **four hops a
  round trip** — so the ball touches down exactly as it meets each wall, where
  the script's 2.6 s / 0.66 s let the bounces drift against the walls. At
  rest the ball's bottom row shares the floor's, so it reads as landing rather
  than hovering; at the apex it sits whole in the top two dot rows. The ball
  wears the banner gradient by where it is on the track — cyan at the left
  wall, blue at the right — and the cell it sits in takes its colour.
- **`wave`** puts one dot per dot column on a sine whose wavelength is the
  whole track (`SPINNER_WAVE_LENGTH`, so a crest and a trough are always in
  view), quantised to the four dot rows. Its *phase* ping-pongs:
  `SPINNER_WAVE_TRAVEL` (3) wavelengths out and the same back per
  `SPINNER_WAVE_SWEEP`, so the wave rolls right, reflects off the wall and
  rolls back. Because the travel is a whole number of wavelengths, the frame
  at the reversal *is* the frame it set out from — no seam. Each cell wears
  the banner gradient by its place on the track: the mascot's own wash,
  rolling.

Both curves are computed in **whole milliseconds** (`ping_pong` folds the
elapsed onto the way *toward* the far wall; `hop` multiplies integers before
it divides), so the way back retraces the way out bit for bit: the frame at
`T − d` equals the frame at `T + d`, which is what makes the reflection
seamless rather than approximately so. `ping_pong` is the comet's Larson
sweep as a continuous value.

### Colour

Every style ends its spans in the separator space before the verb, so the
verb's shimmer starts at the same distance whatever the style's width; the
comet keeps its eight per-cell spans (white bold head, mid-grey `•`, dim
rest), the two tracks emit one span per cell (not bold — braille dots are
dense already, and a synthesized bold blurs them), and every other style is
**one glyph in one span**, bold, coloured by `glyph_color`:

- `sparkle`, `blocks`, `gravity` and `wave` walk the **banner's gradient**
  (`header_gradient_start()` → `header_gradient_end()`, the theme's accent →
  link, `docs/header.md`, `docs/theme.md`) — the spark by its bloom level
  (the accent at `·`, the link at `✽`, back down the fade), the
  block by its turn, the ball by its position, the wave by each cell's place
  on the track — so the theme's accent rides the status line;
- `pulse` breathes the running tool bullet's raised cosine
  (`ui::wrap::breath`, the helper `tool_pulse_color` now shares,
  `docs/tool-pulse.md`) from the bullet's own dim (`tool_pulse_dim()`) up to
  white — brighter at the crest than the bullet, because a status line's head
  has to read where a resting cell only has to be noticed — and it shares the
  bullet's period, so a pulsing status line and a running tool cell breathe
  in step;
- `bars` brightens with height, the tool pulse's bright grey at `▁` to white
  at `█`;
- `dots` and `line` wear the comet head's white.

## The `/spinner` picker

```
────────────────────────────────────────────────────────────────
  ❯
    comet    (●•·   )
  → gravity  ⣀⣘⣃⣀⣀⣀⣀⣀ ✓
    wave     ⠔⠉⠉⠑⠤⣀⣀⡠
    sparkle  ✶
    dots     ⠹
    blocks   ▛
    pulse    ●
    bars     ▅
    line     /
  (2/9)

  ⣀⣘⣃⣀⣀⣀⣀⣀ Working… (4s · esc to interrupt)

  A ball hopping along the track, bouncing off both walls
  Type to search · Enter to choose · Esc to cancel
────────────────────────────────────────────────────────────────
```

Deliberately the `/mascot` picker's twin — same inline frame, same `❯`
type-to-search (matching names *and* descriptions, so `braille` finds
`dots`), same `→` marker with the cyan selection, same `(n/total)` counter,
description row and dim hint — plus the thing a spinner picker exists for:
the page is **live**.

- **Every row wears its own spinner.** The name column is sized to the
  widest visible name plus `SPINNER_MENU_GAP`, and after it each row draws
  its style's frame through the same `spinner_spans` the status line uses, so
  the nine styles compare at a glance, all turning at once. The session's
  current style carries the `/model` picker's green `✓` after its spinner,
  and the open seats the highlight on it.
- **The highlighted style previews as a whole status line** — a sample
  `TurnStatus` (the `Working` verb, no tokens yet: a turn just submitted)
  through `ui::styled_status_line`, the *same* renderer the strip's status
  row uses, so what the picker shows and what a turn shows can never
  disagree. Its elapsed is the frame clock **measured from the open**
  (`SpinnerPicker::opened_at`, `App::spinner_preview_elapsed`), so the line
  reads `0s` when the page appears and counts up while the user browses —
  a turn that just began, not a clock that has been running since launch —
  and the spinner's motion and the verb's shimmer take their phase from that
  same value exactly as a real turn's do.
- **It animates with no turn running.** `App::wants_animation_frames` is
  true while the picker is open, so the draw tick re-arms the 32 ms clock
  chain (codex's status-widget cadence) exactly as an active turn or the ↓
  manager band does, and the frame clock it animates against is the one the
  boundary already injects every draw (`App::set_pulse`, `docs/tool-pulse.md`).
  The chain stops by itself on the first draw after the picker closes.
- **The page never moves while it animates.** Styles differ in width (the
  comet and the two tracks are eight cells, the rest one) but every style is
  exactly one row, so neither the selection nor the clock changes the page
  height — the `/settings` always-emit rule, pinned by
  `the_page_height_is_stable_across_selections_and_ticks`.

A search that matches **nothing** collapses to its essentials — the
placeholder (`No matching spinners`), one blank gap, the hint — because
there is no count, no preview and no description to show; the `/mascot`
picker's rule, from the `/model` picker's `model_has_detail`.

Keys: ↑/↓ move, **wrapping at the ends** (the shared `wrap_step` grammar);
PageUp/PageDown/Home/End jump (clamping); printable keys filter (Backspace
pops, and a keystroke reseats the highlight on the first match); **Enter or
Space chooses and closes**; Esc clears the query first, then closes; Ctrl+C
closes. The picker owns every key while open (routed at the top of
`App::on_key`, before the composer's global Ctrl+C/Ctrl+O), pastes are
swallowed (nothing anyone pastes is a style name), and the running cell's
`(ctrl+b to run in background)` hint is blanked while it is open
(`App::background_hint_elapsed`, the rule every composer-replacing picker follows —
`/mascot` joins the list with it). It works **mid-turn** like `/mascot`: it
only replaces the composer, and the streaming strip keeps its rows above it
— which for this picker is the point, since the strip's status row is what
a switch changes, on the very next frame. On a short terminal the page
bottom-anchors and flows its top into scrollback like its whole family
(`docs/view-flow.md`) — with one difference, below.

### The flow is signed on the selection, not the rows

`docs/view-flow.md`'s one per-view choice: a page that changes only on a
keystroke signs its **rows**, so any edit re-flows; a page that **ticks**
between keystrokes must not, or every frame would purge-rebuild the whole
screen thirty times a second. The ↓ manager's details page was the one such
page; the `/spinner` picker is the second. So `ui::view_flow` signs it
`FlowSign::Frozen` on the picker's `(selected, query)`: a frame advancing
the spinners leaves the signature alone (its flowed top freezes in
scrollback, which is what scrollback holds for everything else anyway),
while a keystroke that moves the highlight or edits the search re-signs it
and the standard purge rebuild re-flows the page. Pinned by
`a_tick_never_resigns_the_flow_but_a_keystroke_does`.

## Applying a selection

`Action::SelectSpinner(spinner)` — the pure side already moved `App::spinner`
and closed the picker; the boundary (`tui::spinner::Session::select_spinner`):

1. **persists** `{config_home}/spinner.json` — as **this working
   directory's** own choice *and* the last made anywhere, a read-modify-write
   over the file (`tui::config::save_look`, best-effort like every write
   there): the pure format is `app::SpinnerFile`, the `spinner.json` instance
   of the `app::LookFile` the two looks share — `config.json`'s
   per-directory rule, the same file shape `docs/mascot.md` *Per directory*
   shows under the `spinner` key — and
2. raises the confirming `Spinner: {name}` toast.

Unlike a `/mascot` switch this needs **no purge rebuild**: the status line is
live-region-only — its per-frame spinner and shimmer colours never reach
scrollback (`docs/status-indicator.md`) — so nothing committed has to be
redrawn. The strip's status row is built per frame for `App::spinner()`
(`ui::live` passes it to `styled_status_line`, for the main turn and for a
subagent session view's synthesized status alike), so a running turn wears
the new style from its next frame, and the next turn from its first.

At startup `tui::bootstrap` seeds `App::spinner` from `spinner.json` before
the first frame — the directory's own entry, or, launched in for the first
time, the last choice made anywhere, **pinned** as the directory's own right
then (`tui::config::adopt_look`); an absent or corrupt file keeps the default
gravity, never a startup failure — so the first turn's status line already
wears it.

## Design notes

- **Why a picker and not a `/settings` row?** A `/settings` row cycles a
  value you can't see until the next turn runs; the status line is the one
  thing in the TUI that only exists *while* something runs. A picker whose
  rows and preview animate is the only honest way to choose an animation.
- **Why `/spinner` and not `/status`?** `/status` reads as "show me the
  session's status" (the command Claude Code gives that name). The spinner
  is precisely the part being chosen.
- **Why are the frames in `ui/theme.rs` and not on the enum, like
  `Mascot::art()`?** The mascot's art is *content* the banner colours; a
  spinner's frames, cadence and colour rule are its whole look — pure
  styling, which this crate keeps in one file so a retheme touches one
  place. The enum knows nothing about glyphs, and `spinner_spans` is the
  only mapping.
- **Why are the two tracks drawn and not tabled?** Their motion runs on two
  independent periods (the ball's trip and its hop; the wave's sweep and its
  wavelength), so a table of their frames would run to hundreds of entries
  and still quantise the motion to the table's step. Drawing from `elapsed`
  costs a few dozen dot writes per frame and keeps the same purity every
  other spinner has: the phase comes from the boundary clock, nothing else.
- **Why four hops a round trip?** So the ball lands exactly as it reaches
  each wall. The script's 2.6 s / 0.66 s let the bounce drift against the
  walls, which reads as random; a whole ratio reads as a rhythm.
- **Why one span per one-cell style?** The comet's per-cell spans exist so
  each cell can carry its own fade step. A one-glyph style has nothing to
  fade across, and the `/spinner` rows and `VERB_START` arithmetic in the
  tests both read simpler for it.
- **Why not also restyle the `● Thinking…` header or the tool bullet?** They
  are not spinners: the header borrows the tool cell's bullet because it
  *means* the same thing (`docs/thinking-stream.md`), and both keep the
  crate's one meaning for `●`. What `/spinner` restyles is exactly the
  status line's opening animation, on both surfaces that draw one.

## API

- `app::Spinner` — the catalog (`ALL`, `name`, `description`, `from_name`),
  `Default` = `Gravity`.
- `app::SpinnerPicker` / `SpinnerRow` — the open picker's state (with
  `opened_at`, the preview clock's origin) and one derived row;
  `App::spinner()`, `set_spinner`, `open_spinner_picker`,
  `close_spinner_picker`, `spinner_rows`, `highlighted_spinner`,
  `spinner_preview_elapsed`, `on_key_spinner_picker`.
- `app::SpinnerFile` — the `spinner.json` format: `app::LookFile` over
  `app::Spinner` (`Look::KEY = "spinner"`), the `MascotFile` API under the
  other key (`docs/mascot.md`).
- `ui::styled_status_line(status, verb, spinner, width)` — the status line in
  a given style; `status_line` / `status_line_with_verb` are its default
  (`gravity`) case.
- `ui::spinner_view` — `spinner_view_lines` (the page builder; its length is
  the reserved height, `docs/view-flow.md`), `spinner_picker_height`,
  `render_spinner_picker`.
- `ui::status::Track` (private) — the eight-cell braille canvas the two
  tracks draw on; `ui::wrap::{breath, ping_pong, hop}` — the shared motion
  curves (`pulse` and the tool bullet's `tool_pulse_color`; the tracks).
- `tui::spinner::Session::select_spinner`, `tui::config::{spinner_json_path,
  adopt_look, save_look}`.

## Tests

- `app/tests/spinner.rs` — the catalog (nine styles listed comet-first;
  gravity the default, of the enum and of a fresh `App`;
  distinct bare-word names, `from_name` round-trip), the per-directory
  persistence format (the round trip under the `spinner` key, the
  last-and-pinned rule, the lenient parse that refuses the mascot key),
  the `/spinner` command opening the picker (listed beside `/mascot`), the
  animation-frame request while open, the preview clock counting from the
  open, the Ctrl+B hint blanked, and the whole key grammar (filter, the
  wrapping ↑/↓ against the clamping jump keys, Enter/Space select, Esc/Ctrl+C,
  owns-every-key, mid-turn use leaving the turn untouched).
- `ui/tests/status.rs` — every style's frames single- and fixed-width, the
  default line byte-identical to the gravity track at every phase (and
  opening with the ball on its floor), each style's first frame and its
  one separator space, the one-span rule, the sparkle's bloom and gradient,
  the pulse's breath, the bars' brightening, the blocks' gradient, the
  classic steps; the gravity ball's exact frames at the walls, a quarter hop
  in and at the apex, its floor under every cell of every frame, its
  gradient over a dim floor; the wave's two dots per cell, its motion, its
  seamless reversal and time-symmetric crawl, its gradient wash; and
  `render_live` wearing the session's style.
- `ui/tests/spinner_view.rs` — the framed page (rules, search, rows, counter,
  preview, description, hint), every row's live glyph (the tracks included),
  the rows and preview ticking with the clock, the preview following the
  selection, the stable height across selections *and* ticks, the active ✓
  and its seat, the no-match collapse, no stacked blanks, width safety, the
  height contract, flow eligibility, and the tick-stable / keystroke-sensitive
  flow signature.
- `scripts/smoke.sh` Phase 110 — the picker end to end in a real terminal:
  open from the palette seated on the default gravity (its ✓ on a fresh
  config home), the live rows (the ball on its floor, the wave's
  braille cells) and the track previewing, ↑ moving the preview onto the
  comet, that preview turning between two captures with
  no turn running, a filtered
  Enter switching the style with a toast and a `spinner.json` write, the very
  next turn's status line opening with the new style mid pre-stream pause, and
  a **second process against the same config home launching with it**.
- `scripts/smoke.sh` Phase 114 — the per-directory rule end to end, beside
  `/mascot` (`docs/mascot.md` *Tests*): a style chosen in one directory is
  its own and the last, a new directory pins that last, a choice there never
  moves the first's, and a third directory takes the new last.
