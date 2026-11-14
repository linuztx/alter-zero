# The banner mascot's motion

The startup banner's mascot (`docs/header.md`, `docs/mascot.md`) is alive
while there is nothing else to look at: from launch until the conversation
holds its first cell it hops, waves, patters its feet, winks, glances about,
dozes off and starts awake — a different thing every few seconds. The moment
a message is sent it settles into its resting art for good, because from
then on it is scrollback.

```
 ▙▄▙▄▟▄▟        ▙▄▙▄▟▄▟       ▝▙▄▙▄▟▄▟▘      ▖ ▖ ▗ ▗        ▄▄▙▄▟▄▄
▝▜▄███▄▛▘      ▝▜▄████▛▘       ▜▄███▄▛      ▗█▀███▀█▖      ▝▜█████▛▘
  ▘▘ ▝▝          ▘▘ ▝▝          ▘▘ ▝▝        ▝▛▛▀▜▜▘         ▘▘ ▝▝
   rest            wink         arms up      mid-hop         asleep
```

## A pose is a body with eyes

`app::MascotPose` is what a mascot looks like at one instant, and it has two
layers:

- a **body** — one frame of art, the resting art's shape exactly: its row
  count, never wider than its widest row, single-width block glyphs only.
  That is what lets a pose be drawn *in the resting art's place* — the
  metadata column beside it never moves (`ui::header`), and the tests hold
  every still of every gesture to it;
- the **eyes** laid over it, each in one of six states.

**Eyes are holes** — half a cell of background punched in a solid face — so
an eye's states are the block glyphs that reshape the hole, and they are the
same for every mascot whose eye sits in the same half of its cell:

| state  | upper-half eye (`▄`: crest, bloom, twin, the gem's facet) | lower-half eye (`▀`: sprout, skiter) |
| ------ | --------------------------------------------------------- | ------------------------------------ |
| open   | `▄`                                                       | `▀`                                  |
| lidded | `▆`                                                       | — (no glyph: it closes in one cut)   |
| shut   | `█`                                                       | `█`                                  |
| left   | `▟`                                                       | `▜`                                  |
| right  | `▙`                                                       | `▛`                                  |
| wide   | `▂`                                                       | `▔`                                  |

That is why **one** choreography of blinks, winks and glances serves all six
mascots (`Mascot::look_stills`, `Eye::glyph`), and each mascot's own tables
hold only what its body does. There is no looking up or down: an eye is
enclosed by the half of its own cell it does not fill, and a hole moved to
the other half opens onto the gap between the feet (or the sky) and stops
being an eye — the frames were drawn, looked at, and thrown away.

A body frame that moves the **whole creature** is the resting art's half
cells, each moved one over: every glyph in the catalog is a quadrant block,
so a half-cell shift folds back into quadrant blocks exactly. A hop is the
art half a row lower (the sprout, which stands on the block's floor, springs
half a row *up*, its leaf's tip leaving the block for the moment); the
skitterer's scuttle is the art half a column left and right. The tests
decode the art to half cells and replay the shift, so a hand-typed frame
cannot drift from it.

Light moves no glyph. A **glint** is a band of the theme's brightest ink
three cells wide — full under its centre, half as strong a column off — that
**leans a column a row**, like a `/`, and crosses the art one column a still.
The lean is what makes it read as light glancing off the mascot rather than a
bar wiping across it. It lights ink only, never a blank cell, and its stills
run from the first position that lights a glyph to the last, so no still is
a frame drawn for nothing. A **glow** lights every glyph alike — swelling to
60% and fading the way it came, or flashing to full strength, which is how
the gem catches the light. `MascotPose::light(col, row)` is both — the
stronger of the two — and how a lit glyph is coloured is the renderer's:
lifted off the banner gradient toward `shimmer_highlight()`, the status
verb's own crest, so under the `ansi` theme, whose colours the terminal
owns, a cell steps to the highlight whole instead of mixing (a glow shows
there as one flash at its peak).

## What a mascot does

`app::Gesture` names the moves. The first seven are **looks** — the eyes
and nothing else — and the rest are **moves** of the body or of the light
on it. A sided gesture carries its `Side`; every mascot is the same either
side of its middle, so the other side is the same stills in a mirror
(`Gesture::mirrored`).

| gesture          | what it is                                                          |
| ---------------- | ------------------------------------------------------------------- |
| `Blink`          | the eyes close and open — through a lid where the glyphs allow one   |
| `Wink(side)`     | one eye closes and is held shut a moment                             |
| `Glance(side)`   | the eyes look one way, then the other                                |
| `Dart(side)`     | the eyes flick from side to side, twice                              |
| `Cross`          | the eyes cross                                                       |
| `Widen`          | the eyes go wide                                                     |
| `Doze`           | it sags, sleeps a second, and starts awake wide-eyed                 |
| `Signature`      | the move the mascot's description names                              |
| `Flourish(side)` | a second move of the mascot's own                                    |
| `Hop`            | the whole mascot bobs half a cell and back                           |
| `Wave(side)`     | a hello with an arm — or both, or the claws                          |
| `Cheer`          | a hop with the arms thrown up at the top of it                       |
| `Patter`         | the feet patter on the spot, two at a time                           |
| `Glow`           | the whole mascot swells with light and fades (colour only)           |
| `Glint(side)`    | a slanted band of light crosses the art (colour only)                |

Every mascot has a signature and a flourish of its own, a hop, and the
light. Not every one has a body for the rest: `Mascot::stills(gesture)`
answers with no stills for a move its body cannot make, and no act of a
mascot asks for one:

| mascot   | signature                | flourish                    | hop        | wave        | cheer            | patter | asleep            |
| -------- | ------------------------ | --------------------------- | ---------- | ----------- | ---------------- | ------ | ----------------- |
| `crest`  | drops its frill and flares it | a ripple along the frill | down       | one arm     | both arms up     | ✓      | frill half down   |
| `bloom`  | swells                   | its head wobbles            | down       | flaps both  | both arms up     | ✓      | head to one side  |
| `sprout` | its leaf sways           | unfurls two leaves          | **up**     | one arm     | —                | ✓      | leaf bent         |
| `twin`   | flicks one ear, then the other | splays both ears      | down       | —           | ears splayed     | ✓      | one ear flat      |
| `skiter` | skitters in place        | scuttles half a cell each way | down     | both claws  | claws up         | —      | as it stands      |
| `gem`    | catches the light: two flashes | spins, edge-on and back | floats, slowly | —      | —                | —      | —                 |

The gem has no eyes; its one centre facet stands in for one. Its `Blink` is
the facet closing, its `Glance` the facet sliding a cell left and right — the
gem turning in the light — its `Dart` the facet narrowing from side to side,
its `Widen` the facet opening, and it neither winks, crosses nor dozes. Its
signature is the one that moves no glyph: what a cut gem does is catch the
light.

## Acts

What the schedule plays is not a gesture but an **act**: a gesture, or a
short phrase of them with rests between (`Beat::Do`, `Beat::Rest`). A phrase
is what makes a move read as a creature rather than a clip — it waves and
then winks, it starts awake and blinks the sleep out of its eyes.

Each mascot has two lists of acts (`Mascot::moves`, `Mascot::looks`):

- its **moves** — `Signature` · `Flourish` · `Hop, Hop` · `Cheer, Blink` ·
  `Wave, Wink` (or `Wave, Blink` where both arms wave) · `Patter` · `Glow` ·
  `Glint`, less whatever its body cannot make — five for the gem (`Signature`
  · `Flourish` · `Hop` · `Glow` · `Glint`), seven or eight for the others;
- its **looks** — `Blink` · `Glance` · `Wink` · `Dart` · `Cross, Blink` ·
  `Widen, Blink` · `Doze, Blink, Blink` — four for the gem.

**A blink is punctuation.** Exactly one look is a bare blink; the rest of a
mascot's blinking closes some other act. The first version of this feature
played a blink in two bars out of three, and what it looked like was a
mascot that only ever blinks; `a_bare_blink_is_a_small_share_of_what_a_mascot_does`
holds a blink-only bar to one in eight.

## When it moves

Motion is a pure function of **how long the banner has been up** and a
per-session seed: `app::mascot_motion(mascot, seed, elapsed)` answers with
the pose to draw and how long it holds. No state advances; the same inputs
give the same pose, which is what makes it testable and what lets the
`/mascot` picker run a second, independent clock through the same function.

Time is cut into **bars** of `MOTION_BAR` (4.8 s). Each bar plays one act,
starting somewhere in the bar's middle and over well before the bar is:

- **Bar 0 is the greeting**: 0.7 s after the banner appears, a glint crosses
  the mascot, it makes its signature move, and it blinks. About two seconds
  of motion at launch, the same on every seed, then calm.
- **Every later bar** plays the next act, **a move and a look by turns** —
  a move first — so no stretch of an idle banner is all eyes, and no bar
  plays what the one before it did.
- Each kind is **dealt like a deck** (`dealt`): every cycle through a list
  is a fresh shuffle that plays each act once, so a mascot goes through
  everything it can do before any of it comes round again — thirty seconds
  of idling is six different acts. No act plays twice in a row, even across
  the join between two cycles: a cycle never opens on the act the last one
  closed on — the first act it may open on is brought forward instead. The
  first cycle joins onto the **greeting** the same way: the signature leads
  the moves and the glint closes them, the bare blink leads the looks, and
  the first cycle opens on none of the three (`greeted`), so what follows
  the greeting is always something it has not just shown.
- A sided gesture is played **either way round**, on a coin flipped for the
  bar: which eye winks, which way it glances first, which arm waves.
- The act **starts** 0.9–2.7 s into its bar, hashed from the seed and the
  bar's index — never a metronome — or sooner for a long act, which is
  scattered over what it leaves of the bar: the bar's last 0.9 s are always
  at rest, so consecutive acts begin 3.0–6.6 s apart and never run together.

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
which makes the page a showcase: ↓ onto `skiter` and it skitters, stay on it
and it goes on through its moves and its looks. The preview's clock is its
own (`MascotPicker::shown_at`, latched on the first frame after the
highlight lands, cleared whenever the highlight moves), so it never depends
on how long the session has been running.

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
- `app::MascotPose` — one instant: `MascotPose::rest`, `rows()` (the body
  with its eyes, as the rows to draw), `light(col, row)` (how strongly the
  pose's light falls on a glyph) and the light itself, `glint` and `glow`.
- `app::Gesture`, `app::Side`, `Gesture::mirrored`, `Gesture::ALL`, and
  `Mascot::stills(gesture) -> Vec<Still { pose, hold }>` — the stills
  (`app/mascot_motion.rs`: the eyes' one choreography, and each mascot's
  body frames written against the catalog's own resting rows, so a frame
  can only differ in the rows it redraws).
- Inside `app`: `Beat`, `GREETING`, `Mascot::moves`/`looks` (the acts),
  `Kind` and `dealt(seed, kind, turn, count)` (which act a turn plays).
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

## Adding a move

A new move for one mascot is a table of frames beside its others and an arm
in `Mascot::body_frames`; a new look for all of them is a list of eye states
in `Mascot::look_stills`. Either way it becomes something the mascot *does*
by being listed in an act (`CREST_MOVES`, `LOOKS`, …), and the tests then
hold it to the rules without being told about it: the art block's shape, no
still that repeats its neighbour, a sided gesture mirrored, the act fitting
its bar. Draw the frames on the half-cell grid and **look at them rendered**
before typing them in — every frame here was, and a third of the candidates
did not survive it (a mouth, eyes looking down, a sparkle of loose cells).

## Tests

- `app/tests/mascot_motion.rs` — the stills: every one keeps the art block's
  shape and none repeats its neighbour; a blink closes every eye and nothing
  else, through a lid where the glyphs allow one; a wink closes one; a
  glance looks one way then the other, left first when asked; darting eyes
  trade two looks; crossed eyes look at each other; wide eyes open the hole;
  a doze sags, sleeps a second and starts awake; every signature and every
  flourish is the mascot's own; a hop is the resting art half a cell lower
  (the sprout's higher), replayed on the half-cell grid, as is the
  skitterer's scuttle; a wave leaves the face alone; a cheer is a hop with
  the arms up; pattering feet trade places, two a step; a sided gesture the
  other way is its mirror image; the glint's shape, the glow's swell, the
  gem's two flashes and its spin; a mascot makes no move its body lacks, and
  every one makes those every body can. The acts: a
  handful of each kind, none playing like another, one bare blink, every one
  fitting its bar. The schedule: the greeting's order, the same on every
  seed; bars open and close at rest and acts are spaced and never keep time;
  a long act starts early enough to finish; a hold runs exactly to the next
  pose that differs, across a bar boundary; a deal plays every act once a
  cycle, never one twice in a row, afresh each cycle, and never opens on
  what the greeting showed — on the timeline too; moves and looks take turns and play what is
  dealt; a sided act plays both ways; a bare blink is a small share; half a
  minute shows six different things. And the session: the epoch latch,
  `/clear`'s reset, the picker's own clock, the predicate's three
  conditions.
- `ui/tests/header.rs` — the resting pose is the banner as committed; a
  pose swaps the art and nothing beside it; light lifts exactly the glyphs
  the pose lights — a glint's band, a glow's every glyph — and keeps to the
  terminal's colours under `ansi`; `banner_motion_lines` is `None` once a
  cell lands, with the setting off, and in the badge tier; the committed
  banner and the transcript never move; when the next frame is due, and that
  a still banner asks for none.
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
  art moves within the greeting; between acts not one frame is drawn; the
  bar after the greeting plays a move — the frill or the feet change, in
  glyph or in colour — and the bar after that moves the eyes; a long idle
  draws a handful of frames, never a chain; the first message leaves the
  banner at rest, colours and all, in the frame that commits it, and it
  never moves again on screen or in scrollback; `/clear` greets again; a
  quit mid-glint leaves the resting art; the `/mascot` preview moves; and
  the env seed and the `/settings` row still it. Every other phase runs
  with the animation off.
