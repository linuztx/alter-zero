# The banner mascot's idle animation

While the conversation is empty — a fresh launch, or the moment after
`/clear` — the banner's mascot is alive: a light sweeps it as the screen
opens, it shows the one move its catalog line promises, it blinks, and from
then on it blinks and glances on a rhythm drawn by lot. The first cell
committed under the banner ends it: from there the banner is scrollback, and
scrollback does not move.

```
 ▙▄▙▄▟▄▟   Alter Zero (v0.19.0)        ▙▄▙▄▟▄▟        ▙▙▙▄▟▟▟
▝▜▄███▄▛▘  ~/alter-zero               ▝▜█████▛▘      ▝▜▄███▄▛▘
  ▘▘ ▝▝    / commands  @ files          ▘▘ ▝▝          ▘▘ ▝▝
 at rest                               a blink        the frill flared
```

See `docs/header.md` for the banner and `docs/mascot.md` for the catalog and
the `/mascot` picker, whose preview moves the same way — each mascot
greeting as the highlight lands on it.

## What each mascot does

The eyes do most of the work. Every mascot that has them **blinks** every
few seconds (sometimes twice) and now and then **glances** to one side or
looks around; the gem, which has none, **twinkles** where the others blink.
And each has one move of its own — the thing its description says it does:

| mascot | its eyes | its own move |
|---|---|---|
| `crest` | blink through an eyelid, glance | **flares its frill**: the spikes pull in for an instant, fan out to six, and settle |
| `bloom` | blink through an eyelid, glance | **flutters**: the petals at its sides drop and lift, three times |
| `sprout` | blink at once, glance | its shoot **sways** in a draught, or **unfurls its first leaf** |
| `twin` | blink through an eyelid, glance | **flicks an ear**: it folds flat and springs back, twice — now the right, now the left |
| `skiter` | blink at once, glance | **skitters**: six legs scuttling on the spot |
| `gem` | — | a **spark** opens beside it (high on the right, low on the left), and now and then a **glint** crosses the whole stone |

A frame is the rest art with **one part moved** — the eyes, a crest, a leaf,
an ear, the legs, a spark — so it differs from the rest art on exactly one
row, keeps its three rows, and is never wider than the art: the banner's
metadata column and the composer under it never shift for a blink.

### Eyes

A mascot's eyes are negative space — a hole one cell wide and half a cell
tall, punched in the body. That decides how they can move:

- **A blink** fills the holes. Where the hole is the *upper* half of its
  cell (`▄`: crest, bloom, twin) the font-safe Block Elements have a
  half-way glyph, `▆`, so the blink closes and opens through an **eyelid**:
  35 ms down, 90 ms shut, 50 ms up — quick, and slower to open than to
  close, the way a real one is. Where the hole is the *lower* half (`▀`:
  sprout, skiter) the matching glyphs live in *Symbols for Legacy
  Computing*, which many fonts lack and some draw double-width; those two
  blink in one 120 ms frame instead. Reverse video over a lower-eighth
  block would have drawn the missing lid, but it paints the hole in the
  terminal's opaque background colour, which is not the background of a
  translucent terminal.
- **A glance** narrows both holes to the same side (`▟▟` left, `▙▙` right;
  `▜▜`/`▛▛` for the lower-eyed) for about two thirds of a second. Shifting
  the whole eye a subcell was tried first and read better on paper — but on
  four of the five it either opened the eye onto the outside or detached
  the arm beside it. The narrowed pair keeps every silhouette whole and
  still says *where* it is looking.

### The sweep of light

A band of light crosses the art once, left to right, in about three
quarters of a second: each cell is lifted from its gradient colour toward
the status line's own shimmer highlight (`mascot_shine_color()` is
`shimmer_highlight()`) by a raised-cosine band two and a half columns
either side of its crest, and each row sits half a column further along
than the row above, so the band leans like a glint instead of scanning like
a bar. It runs from a half-width before the art to a half-width past it,
so the light arrives and leaves by degrees and no cell pops. Under
a theme whose colours are the terminal's own (`ansi`), where there is
nothing to mix, the band's core simply flips to the highlight.

Every mascot gets one sweep as the app opens. Only the gem — *a cut gem
catching the terminal light* — glints again afterwards; its sparks are drawn
in that same highlight, since a spark is light, not gem.

## When it moves

Mostly, it doesn't. The timeline is stillness with short clips in it:

1. **The greeting**, scripted, the same in every session: 0.4 s after the
   banner appears the light sweeps it; at 1.5 s the mascot shows its own
   move; at 2.6 s it blinks. (The gem: a glint, then its two twinkles.)
2. **Beats** of three seconds from 3.6 s on. Each beat draws by lot whether
   a clip plays in it, which one, and where in the beat it starts:

   | | eyed mascots | gem |
   |---|---|---|
   | blink / twinkle | 48 % | 28 % high, 22 % low |
   | double blink / both sparks | 12 % | 8 % |
   | glance left, right, look around | 6 %, 6 %, 4 % | — |
   | its own move | 9 % | 10 % (the glint) |
   | still | 15 % | 32 % |

   That is a move about every three and a half seconds and a blink about
   every five — and the mascot is at rest nine tenths of the time.

Three rules shape the lots:

- **A clip stays inside its beat**, with 200 ms of stillness to spare at the
  end, so two clips never run together.
- **Never two still beats in a row.** A beat that draws stillness right
  after one that did plays the mascot's everyday move instead. However the
  lots fall, it is never still for more than about eight and a half
  seconds.
- **Each mascot keeps its own time.** The lot is drawn over the seed *and*
  the mascot, so no two mascots play one sequence: walking the `/mascot`
  list, each goes on from its greeting in a rhythm of its own rather than
  repeating the last one's blink for blink. (Under one seed alone they did
  — 30 clips of 32 on the same millisecond — until a test said so.)

The seed is the session's (the boundary's entropy, `App::seed_mascot_motion`),
so no two sessions keep the same rhythm. The clock runs from the frame that
showed a **fresh banner**, and three things open one
(`Session::greet_from_banner`): the launch, `/clear`, and choosing a mascot
in `/mascot`. Each greets — a cleared screen is a new start and says so
within half a second, where a mascot merely carrying on could stand still
for eight; and the mascot just chosen shows what it does from the banner
that confirms the choice. A rebuild that only redraws the banner already
there never restarts the clock — a resize, a rewind to an empty
conversation — so the animation is picked up where it is rather than
greeting again on every step of a dragged window edge.

### A pure function of a seed and a clock

There is no animation state anywhere — no thread, no timer, nothing to
advance. `Mascot::pose(seed, clock)` says what the mascot looks like at an
instant, and `Mascot::pose_holds(seed, clock)` how long that lasts:

```rust
let pose  = mascot.pose(seed, clock);        // the art's rows + the sweep's progress
let sleep = mascot.pose_holds(seed, clock);  // until the pose next changes
```

The beat a clock falls in is a division; its lot is one SplitMix64 hash; the
clip and its step are a table walk. The pose three hours in costs what the
pose three seconds in does, a loop that was busy (or behind an overlay) for
a minute simply asks again and gets the right answer, and the whole thing is
unit-tested with nothing but numbers.

## Where it can be drawn

This is the part that needed design. The banner is **committed to
scrollback** at launch (`docs/header.md`) — and everything committed above
the live region is written once and left alone, because rows up there
scroll into the terminal's own scrollback, which no program can rewrite.

Two ways to make it move were weighed:

- **Keep the banner in the live region while the conversation is empty**,
  and commit it only when the first cell arrives. The live region is the
  thing that repaints, so this breaks no rule — but it changes the region's
  geometry in exactly the state every other feature starts from (its
  height, every composer-replacing page's, the cursor seat, the flows), and
  anything committed while the banner was live — the first-run telemetry
  card, the update card — would land *above* it. The banner would have had
  to freeze on a new user's very first screen.
- **Repaint the committed rows in place.** A row above the live region is
  an ordinary screen row until a scroll carries it off the top; only then is
  it scrollback. The banner sits right there, on screen, for as long as the
  conversation is empty.

The second is what shipped: nothing about the geometry changes, a notice
card under the banner is no obstacle, and the cost is one small, strict
piece of bookkeeping in the viewport.

### The anchored block

`InlineViewport::anchor_above(rest)` declares the rows just committed — the
banner and its spacer — as the one block the viewport may still repaint
(`src/term/anchor.rs`). It keeps a note: which rows, how far above the live
region's top they sit (the one row it tracks; rows committed under the
block since are counted), and what they show now. Each frame the loop hands
it the banner in its current pose (`pose_anchor`) and `draw` writes the
cells that differ, in the same synchronized update as the live region,
before the cursor is seated. A blink is two cells — 95 bytes on the wire.

Three rules keep the note honest:

- **Only the rest pose may leave the screen.** Before anything scrolls — a
  flush of queued commits, a live region growing past the bottom of the
  screen, the quit — the block is put back to the rows it was committed as,
  at the row it still sits on. What reaches scrollback, and what a quit
  leaves on the dead screen, is always the banner standing still.
- **Off screen is over.** A block any row of which has scrolled off the top
  is dropped. On a terminal too short to hold the banner whole above the
  composer it never animates at all.
- **A moved screen is a new screen.** A resize or a purge rebuild drops the
  note; the rebuild writes the banner again, and when it wrote *only* the
  banner (an empty conversation, no flowed page under it) the loop declares
  it again.

And one rule belongs to the loop: the moment the conversation stops being
empty — `App::banner_is_live()` is false: a cell recorded, or a turn in
flight to record one — it calls `release_anchor()`, and the very frame that
writes the first cell under the banner first puts it back to rest. Sent
mid-blink, a first message still leaves open eyes above it.

### Frames only when the pose changes

An idle session should stay idle. The status line's animation re-arms a
frame every 32 ms because it moves continuously; the mascot stands still for
seconds at a time, so it keeps no chain at all. After every draw the loop
asks `App::mascot_frame_in` when the pose next changes and schedules **one**
frame for exactly then:

- a blink is four frames (lid, shut, lid, open), then the loop sleeps until
  the next clip;
- the sweep is nineteen frames, 40 ms apart, once at launch;
- the greeting — sweep, move, blink — is under thirty frames in all, where a
  32 ms chain would have written a hundred in the same three seconds.

Nothing is scheduled for a banner that is not on screen (scrolled off,
behind an alternate-screen overlay, or the narrow text badge, which has no
art to move), so those sessions write nothing at all. That matters for more
than CPU: every frame hides and re-seats the cursor, which a terminal with a
cursor-trail animation renders as a shimmer at thirty frames a second
(`docs/copilot.md`), and a terminal drops a mouse selection when the cells
under it are rewritten (`docs/overlay-repaint.md`). A blink rewrites two
cells of the banner's art and nothing else.

### What always wears the rest pose

`ui::header_lines` — what is committed at launch and by every rebuild, what
tops the Ctrl+O transcript, what `/export` writes — is the rest pose,
whatever the clock says. The pose reaches the screen through exactly two
doors: `ui::banner_frame_lines` (the anchored block) and the `/mascot`
picker's preview.

## The `/mascot` preview moves too

The picker is where a mascot is chosen, so its live preview shows what each
one *does* — and shows it at once. The preview keeps **a clock of its own**:
the frame that first shows a mascot under the highlight is the one its
timeline counts from, so every mascot the highlight lands on **greets** —
the light, its own move, a blink, inside three seconds — however the
highlight got there, a key or a search, and one already seen greets again.
On the session's clock it would have stood in whichever beat that clock was
in, and the one move that tells two mascots apart is drawn in about one
beat in eleven: half a minute's wait, on the page whose whole job is the
comparison.

Still nothing advances frame by frame. `App::set_mascot_clock` — the
reading every draw already injects — remembers which mascot is highlighted
and the reading it was first shown at (`mascot_preview`), derived from the
highlight rather than tracked through the keys; `App::mascot_pose_for` is
that mascot's pose that long into its timeline, and the loop's one frame
question (`mascot_frame_in`) answers for whichever changes first, the
banner on its clock or the preview on its own. The banner above an open
picker keeps its rhythm, so the two never move as one.

A frame changes cells inside the art block and nothing else, so the page
keeps its height and every row its width. Because the page now ticks
between keystrokes it signs its flow on the selection and the query rather
than its rows, the `/spinner` picker's rule (`docs/view-flow.md`) — were it
ever to flow, a blink would not churn a purge rebuild.

## Turning it off

Motion on an idle screen is not for everyone. It also means output on an
idle screen, every few seconds for as long as the conversation stays empty:
a tmux window with `monitor-activity` on flags each blink, and a terminal
set to jump to the bottom on output (xterm's default) does so while you are
scrolled up reading something older.

- **`/settings` → Animate mascot** (`true` / `false`), on until a directory
  turns it off, persisted with that directory's other settings
  (`docs/settings.md`). It is read by the frame that poses the banner, so
  off puts the mascot back to rest on the very next draw and on picks the
  animation up where the clock is.
- **`ALTER_ZERO_MASCOT_ANIMATION=0`** seeds the row off for a run, never
  saved. The smoke suite exports it, so the banner its phases capture holds
  still and an idle screen's byte count is zero; Phase 154 turns it back on.
  `scripts/codegen_mem.py` sets it too: what that probe measures is the
  memory a turn leaves behind, with nothing else moving on the screen.

A headless run (`-p`) has no screen and no anchor, and an `App` nobody
seeded — every unit test's — never moves.

## Where the code lives

| | |
|---|---|
| The frames, the clips, the timeline, the session's side (`banner_is_live`, `banner_pose`, `mascot_frame_in`) | `src/app/mascot_motion.rs` |
| The posed banner and the sweep's light (`header_lines_posed`, `banner_frame_lines`, `banner_block_lines`, `banner_shows_art`) | `src/ui/header.rs`; the look (`MASCOT_SHINE_*`) in `src/ui/theme.rs` |
| The picker's moving preview | `src/ui/mascot_view.rs` |
| The anchored block's bookkeeping and cell diff | `src/term/anchor.rs` |
| Declaring, posing, settling and releasing it | `src/term.rs` (`anchor_above`, `pose_anchor`, `release_anchor`, `paint_anchor`, `settle_anchor`) |
| The seed, the clock, the frame scheduling, the release | `src/tui/bootstrap.rs`, `src/tui/view.rs` (`pose_banner`, `mascot_frame_due`) |
| The row and its override | `src/settings.rs`, `src/tui/config.rs` |

## Tests

- `app/tests/mascot_motion.rs` — the frames (the rest art's rows, never
  wider, single-width glyphs in the art's own alphabet, one part moved at a
  time, none trailing a blank); the timeline (opens at rest; a pose holds
  exactly as long as it says, checked a millisecond at a time; the same seed
  the same timeline, another seed another; two mascots on one seed out of
  step); the greeting (the same for every seed: a sweep, the mascot's own
  move, a blink; one sweep from edge to edge); the rhythm (at rest far more
  than moving, a move every few seconds, never still past nine; a clip never
  runs into the next; every frame reaches the screen); the moves (eyes shut
  through a lid where the glyphs have one; a blink quick and slower to open;
  both eyes glance one way; the gem twinkles and never shuts its facet); and
  the session's side (unseeded never animates; only an empty conversation's
  banner is posed; a turn in flight stills it; the row turns it off and on;
  no frame for a banner off screen or behind an overlay; the picker's
  preview runs on its own clock — a mascot greets as the highlight lands on
  it, by a key or a search, again when the picker is reopened, and starts
  over with a session clock that does — while the banner keeps its own).
- `ui/tests/header.rs` — the committed banner wears the rest pose whatever
  the clock says; the live banner draws the pose beside unmoved metadata
  (a blink: two cells); no pose moves the metadata or the banner's shape; a
  banner with nothing moving *is* the committed banner; a sweep lights the
  art under its band and nothing else, leans, and enters and leaves without
  a jump; a spark is drawn in the sweep's light; the narrow badge has no
  art to move.
- `ui/tests/mascot_view.rs` — the preview shows the highlighted mascot
  moving; a moving preview never moves the page.
- `term/anchor.rs` — a block declared while its rows are queued seats once
  they flush; rows queued ahead of it are not under it and rows committed
  after it are; a block with a row off the top has no seat; a still block
  has nothing due; a pose is due until painted; settling keeps the pose; a
  released block wants its rest rows; a blink rewrites the eyes' two cells
  and nothing else.
- `settings/tests.rs` — the row: on by default and off the wire, cycles,
  sits after the picture rows, found by what it does, recorded per
  directory.
- `scripts/smoke.sh` Phase 154 — on the raw byte stream: the greeting's
  frames reach the terminal (the sweep's frames ahead of the flare, the
  first eyelid within a frame count no clock chain could keep) and leave one
  banner at rest over one composer; with the row off an idle session writes
  nothing; a first message sent while the frill is flared leaves the rest
  art above the conversation and nothing is written afterwards; `/clear`
  greets again (about thirty frames in the three seconds after it, the
  flared frill among them, where a mascot carrying on would write a clip's
  handful) and leaves one banner at rest; quitting mid-move leaves the rest
  art on the dead screen; the `/mascot` preview greets as a search lands
  the highlight on a mascot; and the mascot just chosen greets from the
  banner.
