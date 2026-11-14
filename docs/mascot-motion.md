# The banner mascot's motion

The startup banner's mascot (`docs/header.md`, `docs/mascot.md`) is alive
while there is nothing else to look at: from launch until the conversation
holds its first cell, it blinks, glances about, and now and then makes the
one move its description names. The moment a message is sent it settles
into its resting art for good, because from then on it is scrollback.

```
 ▙▄▙▄▟▄▟        ▙▄▙▄▟▄▟        ▙▄▙▄▟▄▟        ▄▄▄▄▄▄▄
▝▜▄███▄▛▘      ▝▜█████▛▘      ▝▜▟███▟▛▘      ▝▜▄███▄▛▘
  ▘▘ ▝▝          ▘▘ ▝▝          ▘▘ ▝▝          ▘▘ ▝▝
   rest            blink          glance      frill down
```

## What moves

Every mascot has the same small vocabulary, drawn in its own anatomy:

| gesture     | what it is                                                        |
| ----------- | ----------------------------------------------------------------- |
| `Blink`     | the eyes close and open                                           |
| `Glance`    | the eyes look left, then right                                    |
| `Signature` | the move the mascot's description names                           |
| `Glint`     | a slanted band of light crosses the art (colour only, no glyphs)  |

**Eyes are holes** — a half cell of background punched in a solid face — so
an eye's states are the block glyphs that reshape the hole:

- `▄`-eyed (crest, bloom, twin; the hole is the cell's upper half): the lid
  rises through `▆` to `█` and back, which reads as an eyelid rather than a
  cut; a glance narrows the hole to one quadrant, `▟` (left) or `▙` (right).
- `▀`-eyed (sprout, skiter; the hole is the lower half): the blink is the
  plain cut to `█` — there is no block glyph for "upper five eighths" — and
  a glance is `▜` (left) or `▛` (right).
- The gem has no eyes. Its `Blink` is its centre facet closing, its `Glance`
  the facet sliding a cell left and right — the gem turning in the light.

The signatures:

| mascot   | signature                                                        |
| -------- | ---------------------------------------------------------------- |
| `crest`  | drops its frill flat and flares it back up from the middle       |
| `bloom`  | swells — the head fills out to its full width and eases back     |
| `sprout` | its leaf sways left, then right                                  |
| `twin`   | flicks one ear, then the other                                   |
| `skiter` | skitters in place, legs trading positions three times            |
| `gem`    | catches the light a second time — the glint, travelling back     |

Every still keeps the art block's shape: the resting art's row count, never
wider than its widest row, single-width block glyphs only. That is what
lets a pose be drawn *in the resting art's place* — the metadata column
beside it never moves (`ui::header`), and the catalog tests hold every
still of every gesture to it.

The **glint** moves no glyph. It is a band of the theme's brightest ink
three cells wide — full under its centre, half as strong a column off —
that **leans a column a row**, like a `/`, and crosses the art one column a
still (`MascotPose::light`). The lean is what makes it read as light
glancing off the mascot rather than a bar wiping across it. It lights ink
only, never a blank cell, and its stills run from the first position that
lights a glyph to the last, so no still is a frame drawn for nothing. How a
lit glyph is coloured is the renderer's: lifted off the banner gradient
toward `shimmer_highlight()` — the status verb's own crest — so under the
`ansi` theme, whose colours the terminal owns, a cell steps to the
highlight whole instead of mixing.

## When it moves

Motion is a pure function of **how long the banner has been up** and a
per-session seed: `app::mascot_motion(mascot, seed, elapsed)` answers with
the pose to draw and how long it holds. No state advances; the same inputs
give the same pose, which is what makes it testable and what lets the
`/mascot` picker run a second, independent clock through the same function.

Time is cut into **bars** of `MOTION_BAR` (4.8 s). Each bar plays one
**act** — a gesture, or a short run of them with rests between — starting
somewhere in the bar's middle and ending well before the bar does:

- **Bar 0 is the greeting**: 0.7 s after the banner appears, a glint crosses
  the mascot, it makes its signature move, and it blinks. About two seconds
  of motion at launch, then calm.
- **Every later bar** starts its act 0.9–2.7 s in (hashed from the seed and
  the bar's index), so acts begin 3.0–6.6 s apart — never a metronome, never
  two at once — and picks the act by weight: a blink a little over half the
  time, then a double blink, a glance, the signature, a glint.

A bar begins and ends at rest, and a pose's `holds` runs to the **next pose
that differs**, across a bar boundary if need be. So the boundary can sleep
exactly that long: an idle banner costs one frame per pose change and
nothing at all between acts — no 32 ms chain, no bytes on the wire, the
cursor left alone. (The status line's chain is the wrong tool here: every
frame hides and re-seats the cursor, which is fine under a streaming turn
and a visible shimmer on an idle screen — `docs/copilot.md` learned that on
the device page.)

`elapsed` counts from an **epoch latched on the first frame** after the
banner appears (`App::set_pulse`), not from process start — the checkpoint
snapshot can hold the first frame for seconds, and the greeting must not
have played to nobody. `/clear` clears the epoch, so the fresh banner greets
again.

## When it does not

`App::banner_in_motion` is the one predicate:

- the **Mascot animation** setting is on (below);
- the conversation is **empty** — `history` holds no cell. Chrome committed
  under the banner (the telemetry and update cards, a startup notice) is not
  a cell, so a first launch still gets its greeting;
- the screen shows the main conversation — not an agent session view, whose
  transcript the banner merely tops.

A `--continue`/`--resume` launch therefore never animates, `/clear` starts
it again, and an interrupt that undoes the first message resumes it.

## Where it is painted

The banner is not in the live region. It is committed above it like any
other row (`docs/header.md`), and until the first cell lands it is still
**on the screen**, at a row the viewport can know. So the motion is painted
there, in place, by the one component that already owns every cell write:

- `InlineViewport::insert_banner` queues the banner like `insert_before`
  and remembers which queued rows are it; `reflow` is told how many rows at
  the head of its tail are the banner. When either is written, the viewport
  **seats** the banner: its top row, and the rows as committed (at rest).
- The seat is carried through every scroll (`scroll_up`, the one place the
  screen moves) and dropped the moment any of its rows leaves the top of the
  screen, on a resize (the emulator reflowed the screen; the purge rebuild
  that follows reseats it), and on the purge itself.
- `draw` takes the banner's rows for this frame — `ui::banner_motion_lines`,
  `None` when it is not in motion — and paints them over the seat **inside
  the frame's synchronized update**, diffed against what is on screen, so a
  blink writes two cells and an unchanged banner writes nothing.

Two rules keep scrollback honest:

1. **At rest before it moves.** A frame that will scroll the screen — it
   flushes commits, or the region grows past the bottom — paints the resting
   art first, whatever the clock says. The first cell's own frame is such a
   frame, which is how the banner settles before it becomes history, and why
   a row that scrolls into the terminal's scrollback is never mid-blink.
2. **At rest on the way out.** `restore` settles the seat before it hands
   the terminal back, so quitting during a blink leaves the resting art.

Nothing else about the banner changes: it is still chrome outside
`history`, still re-emitted at rest atop every purge rebuild, and the
Ctrl+O transcript and `/export` still open with the resting art — they are
records, and a record does not blink.

## The `/mascot` picker

The picker's preview *is* the banner (`docs/mascot.md`), so it moves too —
and the moment a mascot is highlighted it plays its greeting from the top,
which makes the page a showcase: ↓ onto `skiter` and it skitters. The
preview's clock is its own (`MascotPicker::shown_at`, latched on the first
frame after the highlight lands, cleared whenever the highlight moves), so
it never depends on how long the session has been running.

A page that ticks between keystrokes must not sign its flow on its rows
(`docs/view-flow.md`): the picker signs its selection and query instead,
the `/spinner` picker's rule, so a frame never churns a purge rebuild.

## The setting

**Mascot animation** in `/settings` (`docs/settings.md`), on by default and
saved per directory like its neighbours. Off, the banner and the picker's
preview are still. `ALTER_ZERO_MASCOT_ANIMATION` seeds it for a run, the
`ALTER_ZERO_TOOLS` grammar; the smoke suite exports it `0`, since a hundred
phases read the banner's art off the screen and a blink between two
captures is a failure that says nothing about the app.

## API

- `app::mascot_motion(mascot, seed, elapsed) -> MascotMotion { pose, holds }`
  — the timeline; `MOTION_BAR`, `MOTION_GREETING_DELAY`,
  `MOTION_ACT_EARLIEST`/`MOTION_ACT_LATEST` are its measures.
- `app::MascotPose { art, glint }` — one instant: `MascotPose::rest`, and
  `light(col, row)`, how strongly the glint lights a glyph.
  `app::Gesture` and `Mascot::stills(gesture) -> Vec<Still { pose, hold }>`
  — the stills (`app/mascot_motion.rs`, each written against the catalog's
  own resting rows, so a still can only differ in the row it redraws).
- `App::seed_mascot_motion`, `App::banner_in_motion`, `App::banner_motion`,
  `App::mascot_preview_motion`; the clocks are latched in `App::set_pulse`.
- `ui::banner_motion_lines(app, width)` — the banner in its current pose, or
  `None` when nothing moves (the narrow badge tier has no art to move);
  `ui::mascot_motion_due(app, width, banner_on_screen)` — how long until
  anything the motion draws changes, `None` under an overlay;
  `ui::header::banner_lines`, the one builder behind the committed banner,
  the moving one and the picker's preview.
- `InlineViewport::insert_banner`, `banner_on_screen`, the `banner` argument
  of `draw`, the `banner_rows` argument of `reflow`; the seat itself is
  `term.rs`'s private `BannerSeat`.

## Tests

- `app/tests/mascot_motion.rs` — every still of every gesture keeps the art
  block's shape; a blink closes every eye and touches nothing else, through
  a lid where the glyphs allow one; a glance looks one way then the other;
  every signature is the mascot's own; the glint's shape, and that every
  still of it lights a glyph; the greeting's order, the same on every seed;
  bars open and close at rest and acts are spaced and never keep time; a
  hold runs exactly to the next pose that differs, across a bar boundary;
  the schedule is the seed's and every gesture turns up; the epoch latch,
  `/clear`'s reset and the picker's own clock; the predicate's three
  conditions.
- `ui/tests/header.rs` — the resting pose is the banner as committed; a
  pose swaps the art and nothing beside it; the glint lifts exactly the
  glyphs the pose lights, and keeps to the terminal's colours under `ansi`;
  `banner_motion_lines` is `None` once a cell lands, with the setting off,
  and in the badge tier; the committed banner and the transcript never
  move; when the next frame is due, and that a still banner asks for none.
- `ui/tests/mascot_view.rs` — the preview wears the highlighted mascot's
  motion, every mascot greets in it, the setting stills it, and the page
  holds its height through every pose.
- `term.rs` — the seat's bookkeeping, over a viewport that paints into
  nothing: seated only when wholly on screen, where the first frame wrote
  it; a pose painted over it and rest back without one; at rest in a frame
  that commits or grows past the bottom; unseated by a scroll past the top,
  a resize, and a rebuild that outgrew the screen; reseated by a rebuild;
  settled on the way out.
- `scripts/smoke.sh` Phase 154 — a real terminal with the animation on: the
  art moves within the greeting; between acts not one frame is drawn, and a
  long idle draws a handful, never a chain; the first message leaves the
  banner at rest, colours and all, in the frame that commits it, and it
  never moves again on screen or in scrollback; `/clear` greets again; a
  quit mid-glint leaves the resting art; the `/mascot` preview moves; and
  the env seed and the `/settings` row still it. Every other phase runs
  with the animation off.
