# The startup header banner

A Claude-Code-style **header** committed to scrollback at launch: the
session's block-character **mascot** (see `docs/mascot.md`) beside the
product name + version, the working directory, and a one-line hint naming
the composer's keys. It is the first thing the user sees, and it scrolls up
into history naturally as the conversation grows — exactly like Claude
Code's welcome banner. Until then — while no cell has been committed under
it — its mascot **moves**: it blinks, glances and plays its own move, the
committed rows repainted in place (`docs/mascot-animation.md`).

## What it shows

```
 ▙▄▙▄▟▄▟   Alter Zero (v0.1.0)
▝▜▄███▄▛▘  ~/inline-tui
  ▘▘ ▝▝    / commands  @ files  ? shortcuts
```

- **Mascot** — the selected mascot's art (crest by default, switched with
  `/mascot`), drawn flush-left and coloured with a left-to-right
  **accent → link gradient** (the theme's own two accents — Catppuccin
  Mocha's sky `#89DCEB` → blue `#89B4FA` by default, One Dark's `#56B6C2` →
  `#61AFEF` under `/theme onedark`; `docs/theme.md`) keyed by display column
  across the art block's width. Borderless, so the art breathes and no box
  rule is drawn (see *Why borderless* below).
- **Title** — `Alter Zero` **bold**, then `(v{CARGO_PKG_VERSION})` dim.
- **cwd** — the same `~`-relative display string the footer uses
  (`App::session.cwd`, produced by `ui::display_cwd` at the boundary), in the
  footer's path colour too — the theme's success green
  (`header_cwd_color()` is `footer_cwd_color()`, `docs/footer.md`), so the
  path is one colour at both ends of the screen. Skipped when no session info
  is injected.
- **Hint** — the composer's three keys, `/ commands  @ files  ? shortcuts`:
  each key in the cyan accent one space from its label in the dim — the
  `?` band's own dress for its entries (`docs/shortcuts.md`), under every
  theme — and the pairs two spaces apart, the band's own narrowest gutter
  (`HEADER_HINT_GAP` is `SHORTCUTS_MIN_GUTTER`). Each key does from a fresh
  composer what its label names: `/` opens the command palette, `@` the
  file picker, `?` the shortcuts band (*Why the hint names keys* below).

The metadata rows sit in one aligned column at `art width + 2`, and the
block is **vertically centered** in a mascot taller than it, ties resolving
downward — the text seats low rather than hanging off the art's top, so the
pair reads as one composed badge. The catalog is uniformly three rows, which
is exactly the metadata's height *with* a session; the live case is the
**session-less** banner, whose two rows (title + hint, no cwd) seat at 1–2:

```
 ▙▄▙▄▟▄▟
▝▜▄███▄▛▘  Alter Zero (v0.1.0)
  ▘▘ ▝▝    / commands  @ files  ? shortcuts
```

Art rows without a metadata neighbour render alone (no trailing pad), and
metadata rows past the art (nothing in today's catalog) would keep their
column. Every row is clamped to the width with a trailing `…`, exactly like
`footer_line`.

## Why the hint names keys, not commands

The hint used to list three commands, `/login   /model   /resume`: sign in,
pick a model, pick a conversation back up. Each of them is one row of the
`/` palette, which lists every command with what it does, and the first run
already points at the first two: every user-facing offline demo reply closes
on the `/login` then `/model` hand-off (`docs/dummy-backend.md`). What
nothing else on screen teaches is the composer itself, so the banner's one
row of advice names the three keys that open everything else: `/` reaches
every command (those three included), `@` puts a file's path in the
message, and `?` lists the rest of the keys.

The row is three of the `?` band's own entries, and it reads like them —
keys in the accent, labels in the dim — in the terse wording the band takes
on a narrow pane (`/ commands`, never `/ for commands`), with `file paths`
shortened to `files`. Dropping the `for` also keeps the banner clear of
`for commands`, the string the smoke suite reads as the band being open
(Phase 11). One space joins a key to its label and two separate the
pairs, the band's own narrowest gutter, so spacing alone groups each key
with its label and the three keys stay the only symbols on the row. The
footer's ` · ` was tried between the pairs and taken back out: it added a
fourth symbol beside three keys that are symbols already, and the cyan
keys already mark where each pair starts, so only a copy with no colour
(an `/export` file) read any better for it. A test presses each key on a
fresh composer and checks that it opens what its label names
(`header_hint_keys_open_what_they_name`), so the banner cannot go on
advertising a key that stopped working.

## Responsive fallback

The banner never overflows: when the terminal can't seat the art beside the
full title (`art width + gap + "Alter Zero (v…)"`), `header_lines` drops to a
one-line **text badge** — the gradient `Alter Zero` + an accent `v…` — over
the same clamped cwd/hint rows. Both tiers carry the literal `Alter Zero`,
which is what the smoke suite keys on (Phase 45's tier-independent marker).

## It is chrome, not history

The header is **pure chrome**, like the session footer — it never enters
`App::history`. That means it is never sent to the model
(`context::context_messages` only walks history) and never recorded to a
`/resume` rollout file (`tui::recorder::SessionRecorder` mirrors history). It **does**
top the Ctrl+O transcript — `ui::transcript_build` prepends `header_lines` + a
blank before walking history, so the overlay mirrors the inline scrollback,
which opens with the banner (the empty-transcript placeholder keys on "nothing
beyond the banner", and the backtrack selection/scroll offsets come from the
same walk, so they stay consistent). `header_lines` is a pure `ui` helper fed
from `App::session` + `App::mascot` + `env!("CARGO_PKG_VERSION")`; the
boundary owns the timing.

### Surviving every rebuild

The one wrinkle of an inline TUI whose reflow rebuilds from `history`
(invariant 3): every repaint regenerates the screen from history, which the
header is not part of. So it is re-emitted at the I/O boundary:

1. **Startup** — `tui::bootstrap` commits it once via
   `term.insert_before(ui::header_lines(&app, width))` (plus a blank spacer),
   right before the first paint. It flows into scrollback through the normal
   flicker-free pipeline.
2. **Every purge rebuild** (resize, `/clear`, a history rewind, a `/mascot`
   switch) — the purge dropped scrollback outright, so `repaint_conversation`
   restores the banner over the rebuilt tail via the pure
   `ui::banner_tail(banner, tail)` (banner + spacer + tail): it reappears
   after a resize **and** after `/clear` (a fresh-start banner, matching
   Claude Code's `/clear`), and a `/mascot` switch redraws it with the new
   mascot at once (`docs/mascot.md`).

Both commit the banner **standing still** — `header_lines` is the rest pose
whatever the mascot's animation is doing — and, when the banner is all they
wrote (an empty conversation), both leave it **anchored**: the viewport may
repaint those rows in place until the first cell lands under them, so the
mascot can move (`InlineViewport::anchor_above`, `docs/mascot-animation.md`).
The first cell, a scroll that would carry the banner off the screen and the
quit each put it back to the rows written here first, so what scrollback
holds is always this banner.

An **overlay return** (Ctrl+O / `/resume` / Ctrl+D) needs no re-emission at
all: it never rewrites the screen — everything that committed while the
overlay was up flushes from the viewport's pending queue *beneath* whatever is
already there — so the banner the launch committed simply survives, on screen
or in the terminal's kept scrollback, and is never duplicated (`smoke.sh`
Phase 3 pins the no-duplicate half, Phase 45 the settled round-trips; the
overwrite-the-window return this replaces used to wipe it on a short
conversation, and, when a reply had grown past the window under the overlay,
silently dropped everything above the window from the terminal — the
scrollback hole Phases 81/82 now pin shut).

Because the repaint window is **bottom-anchored** (the input box is pinned to the
bottom and overflow scrolls up — invariant 3), prepending the header never hides
content that was on screen before: the header simply occupies scrollback above
the conversation. On a long conversation it scrolls off the top, capped like
everything else by `RESIZE_REFLOW_MAX_ROWS`.

## Why borderless

The `smoke.sh` resize check (Phase 17) counts an input box by its parts —
`^─+$` rule rows == 2, bare `^❯ *$` prompt rows == 1, `dummy_model_name ·`
footers == 1. A *framed* header (a `╭──╮ … ╰──╯` box) would add `─`-only rule
rows and inflate that count. The borderless design draws no full-width
rule, no bare prompt, and no model name, so it is invisible to those structural
counters — it only ever adds mascot/metadata rows. The header text is likewise
kept clear of the strings other phases key on (`for commands`,
`dummy_model_name ·`, `Happy`, `⎿`, `Done for`, `esc to interrupt`).

## API (all pure, in `ui/header.rs`)

- `header_lines(app: &App, width: u16) -> Vec<Line<'static>>` — the whole
  banner as scrollback rows for the session's mascot, sized to `width`,
  **at rest**. No trailing spacer; the caller adds one (the
  `insert_before(msg); insert_before(blank)` pattern).
- `header_lines_posed(app, mascot, pose, width)` (`pub(super)`) — the one
  builder behind every banner: an explicit mascot in an explicit
  `MascotPose` (its art's rows and how far a sweep of light has crossed
  them). `header_lines_for(app, mascot, width)` is it at rest; the `/mascot`
  picker's preview is it in the highlighted mascot's current pose
  (`docs/mascot.md`).
- `banner_block_lines(app, width)` / `banner_frame_lines(app, width)` — the
  banner over its spacer as committed, and as it should look *now*: what the
  boundary anchors and what it repaints the anchor with
  (`docs/mascot-animation.md`). Equal whenever nothing is moving.
- `banner_shows_art(app, width)` — whether this width draws the mascot at
  all (the text badge has nothing to animate).
- `banner_tail(banner, tail) -> Vec<Line<'static>>` — the banner + a blank
  spacer over a purge rebuild's repaint tail (see *Surviving every rebuild*).
- `HEADER_NAME` / `HEADER_HINT` / `HEADER_ART_GAP` — the title word, the
  hint's `(key, label)` pairs, and the art→metadata gap.
- `header_gradient_start()` / `header_gradient_end()` — the mascot gradient's
  endpoints; `gradient_spans` lerps per display column and coalesces
  equal-colour runs into spans.
- The art itself lives on `app::Mascot` (`docs/mascot.md`).

## Tests

- `ui/tests/header.rs` unit tests (`header_*`): the banner composes art rows
  beside the aligned metadata column (verbatim art, the 2-column gap, no
  trailing pad on meta-less rows); the metadata block seats lower beside a
  taller mascot (every mascot's session-less banner puts its two rows at
  1–2, art alone above); it follows `App::mascot`; the title is bold-name + dim-version,
  the cwd in the footer's path colour, the hint's keys in the accent beside
  dim labels — the `?` band's dress — under every theme, the pairs the
  band's narrowest gutter apart, and each key it shows opens what its
  label names from a fresh composer
  (`header_hint_keys_open_what_they_name`); the art carries the accent → link gradient; a
  very narrow width falls back to the text badge and never exceeds the
  width; a session-less `App` still renders art + version (no cwd row); the
  banner contains none of the smoke-reserved strings at any width for any
  mascot.
- `ui/tests/header.rs` unit tests (`banner_tail_*`): the banner + spacer
  restore over the rebuilt tail, and a long tail never clips it.
- `ui/tests/header.rs` unit tests (`transcript_*`): the transcript opens with
  the banner + a spacer before the history walk.
- `ui/tests/wrap.rs`: the mascot tier still fits at the smoke suite's 50-col
  resize width, and no width in a broad sweep overflows.
- `ui/tests/header.rs` unit tests for the live banner — the committed one
  always at rest, a pose never moving the metadata, the sweep's light
  (`docs/mascot-animation.md` lists them).
- `scripts/smoke.sh` Phase 45: the header shows at startup (the `Alter Zero`
  marker), survives a resize (still present after 80×24 → 50×24 → 80×24),
  re-shows after `/clear`, tops the Ctrl+O transcript (empty and with a real
  conversation), and **survives the Ctrl+O round-trip**; Phase 3 pins the
  no-duplication half (never more than one banner in screen+scrollback after
  a mid-stream return over a long conversation); Phases 16/17 confirm it
  doesn't disturb the `/clear`-blank or resize structural counts; Phase 86
  drives the `/mascot` switch (`docs/mascot.md`).
