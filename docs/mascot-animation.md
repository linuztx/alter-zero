# The banner mascot's idle animation

While the conversation is empty — a fresh launch, or the moment after
`/clear` — the banner's mascot is alive: a light sweeps it as the screen
opens, it shows the one move its catalog line promises, it blinks — and from
then on it works through everything it knows, a move every few seconds. It
bounces, wiggles, peeks to one side, waves and winks, cheers, stares, taps
its feet, catches the light, daydreams, and after a while alone it nods off.
And while the first message is typed it **watches**: its eyes follow the
caret across the composer, reading along. The first cell committed under the
banner ends it all: from there the banner is scrollback, and scrollback does
not move.

```
 ▙▄▙▄▟▄▟   Alter Zero (v0.19.0)      ▙▄▙▄▟▄▟      ▙▙▙▄▟▟▟      ▖ ▖ ▗ ▗     ▐▄▟▄▄▙▄▌
▝▜▄███▄▛▘  ~/alter-zero             ▝▜█████▛▘    ▝▜▄███▄▛▘    ▗█▀███▀█▖    ▀▙███▙█▀
  ▘▘ ▝▝    / commands  @ files        ▘▘ ▝▝        ▘▘ ▝▝       ▝▛▛▀▜▜▘      ▝▝  ▘▘
 at rest                             a blink      frill flared   a bounce   a peek left
```

See `docs/header.md` for the banner and `docs/mascot.md` for the catalog and
the `/mascot` picker, whose preview moves the same way — each mascot
greeting as the highlight lands on it.

## What each mascot does

Each mascot has a **repertoire** of nearly thirty moves (the gem, with no
eyes to make faces with, a score). The first version had six, and three
beats in five were a blink; "it's always just blinking" was a fair review.
The second gave each a score, and the third — ported in part from another
branch's take on the same feature — added limbs, phrases and light. The
moves come in six families:

- **Looks** — what a pair of eyes does: a **blink** (and a double), a
  **glance** to either side, a **look around**, a **shifty** dart from side
  to side, a **wink** of either eye, a **cross-eyed** moment, and a
  **daydream** — the eyes gone to one side and staying there, blinking
  while they are away. The three whose eyes have a lid add a **startled**
  wide stare, a contented **squint**, and one eye of each — the look of
  something that is not so sure.
- **Phrases** — two moves that belong together, the second the first's
  punctuation: a **wave** and a wink of the eye beside the arm (a hello), a
  **cheer** — a spring with the arms (petals, ears, claws) thrown up, twice
  — and a blink, a startled stare or crossed eyes that collect themselves
  with a blink, a doze that blinks the sleep away twice. A phrase pauses
  inside itself for a beat (150 ms), less than the breath that parts two
  moves, so it reads as one.
- **The whole body, half a cell** — a **bounce** (it dips and is up again,
  twice), a **happy bounce** (three, eyes squeezed shut on every landing), a
  **wiggle** from side to side, a little **dance** (down to the left, down
  to the right), and a **peek**: the eyes go to one side and the mascot
  leans after them.
- **Its own moves** — the thing its catalog description says it does, and
  three or four more (the table below).
- **Light** — the sweep that opens the greeting comes back once a round; a
  **glint**, a narrow catch of the light, crosses from either side; and a
  **glow** swells through the whole mascot and fades, a breath of light.
- **A doze** — for the four that can (below).

| mascot | its own moves | its whole body |
|---|---|---|
| `crest` | **flares its frill** (the spikes pull in, fan out to six, settle); a **ripple** runs along it, each spike dropping in turn, there and back; it **ducks** — frill flat — and springs up flared; it **waves** an arm from the shoulder and winks; it **cheers**, both arms high; it **taps** or **patters** its feet | bounces, wiggles, dances, peeks |
| `bloom` | **flutters** its petals (down and up, three times) or **flaps** them (up to the head, down to the hem); **seesaws** them; **puffs up** its head, or **swells** it out to the block's edges; its crown **wobbles** from side to side; **tucks** its petals in, shy; **cheers** with them up; taps or patters its feet | bounces — it fills its block from edge to edge, so it never leans |
| `sprout` | its shoot **sways**; it **unfurls a leaf** to either side, or **spreads both**, or **blossoms** — two leaves opening as wide as they go; it **sinks to a bud and pushes up again**; it throws its **arms up**, three times, and blinks; it **waves** one, and sometimes winks after; it **patters** its feet | wiggles and peeks — its feet stand on the block's last half-row, so it never bounces |
| `twin` | **flicks an ear** (either one, twice); **wiggles** both out and in, or **splays** them to the corners; **sways** them; they **droop** and spring up pricked; it **cheers** with them splayed; it taps or patters its feet | bounces, wiggles, dances, peeks |
| `skiter` | **skitters**; a **wave** runs down its six legs, twice; it **stretches** them — drawn in, then spread wide; it **raises its claws**, and **cheers** with them up | bounces, wiggles, **scuttles** (half a cell each way, stopping between), dances, peeks |
| `gem` | a **spark** opens at any of its four corners, at two in turn, or **all round**; its **facet slides** from side to side as the stone turns, stands tall when it **gleams**, **pulses**; it **spins** — narrower, thin, edge-on and back; a sweep crosses it with a spark; it **flashes** — to full strength, dark, and again | bounces, **floats** (a slow bob), wiggles, dances, and rocks slowly from side to side |

### One part moved, or the whole mascot

A frame is one of four things, and the tests hold every frame to it:

- **the rest art with one part moved** — the eyes, a crest, a leaf, an ear,
  a foot, a spark — so it differs from the rest art on exactly one row;
- **a limb raised from the body's side** — the crest's arm, the bloom's
  petals, the skitterer's claws, each a quarter cell at the end of the
  middle row, lifted to the shoulder: two neighbouring rows redrawn, and
  nothing in them but the art's two edge columns;
- **the stone turned** — the gem's spin: every half-row of it as wide as the
  rest art's or narrower, about the same middle, down to a single column
  edge-on; or
- **the whole mascot moved half a cell.** The art is drawn on a half-cell
  grid (a quadrant glyph is four quarters of a cell), and every mascot
  leaves a half-row free under its feet, a half-column free at each side,
  or both. So the same figure can be drawn half a cell lower, or to a side,
  inside the same three rows and nine columns: every row is redrawn and
  nothing has changed. Moved back, it is the rest art again — with one part
  moved at most, since a happy bounce shuts the eyes as it dips and a peek
  leans with the eyes already turned.

Either way a frame keeps the art's three rows and is never wider than the
art: the banner's metadata column and the composer under it never shift.
Nothing moves up — no mascot has a free half-row above it — so a bounce is
a dip and a recovery, which is what a bounce looks like from the landing.

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
  still says *where* it is looking. **Shifty eyes** are the same two looks
  at a sixth of a second each, left-right-left-right.
- **A wink** shuts one eye for about a third of a second — twice a blink,
  so it reads as meant — through its lid where it has one, the way both
  eyes blink (`▆`, held shut, `▆`), and in one frame where it has none.
- **A daydream** is a glance that stays: the eyes go to one side, blink
  there, and are still there after — something seen, and looked at.
- **Crossed** eyes are each narrowed toward the other. Narrowed *apart* is
  the one pairing never drawn: it reads as two creatures.
- **Wide and squinting** come from the same eighth-block ramp as the lid:
  `▂` leaves three quarters of the cell open, a startled stare; the lid
  held for most of a second is a squint; `▂` beside `▆` is one of each.
  Only the three upper-eyed mascots have the ramp, so only they pull these.
- **What eyes cannot do** is look down (the upper-eyed) or up (the
  lower-eyed): the hole would open onto the legs, or cut through the
  forehead. Both were drawn, looked at, and thrown out — as was an eye
  rolled through its four quarters, which passes through the same two
  broken frames.

### Light

Light is colour only — never a glyph — and falls on ink: a blank cell keeps
its colour, so the wire carries no repaint of empty cells. Four kinds:

- **The sweep**, the greeting's: a broad band, below.
- **A glint**: the sweep's narrow, quick sibling — a raised-cosine band one
  and a half columns either side of its crest, leaning a whole column a row,
  across the art in under half a second, **entering from either side**
  (`ShineBand::GlintFromLeft`/`GlintFromRight` — the same streak run the
  other way). Two a round, one from each side.
- **A glow**: every cell lifted alike toward the highlight, by degrees — up
  to a little over half the sweep's strength over nearly half a second and
  back down the same way, eleven levels 70 ms apart. A breath of light. One
  a round.
- **A flash**, the gem's own: to full strength, dark, and full again — a cut
  stone catching the light.

A pose carries the band (`MascotPose::shine`, its kind and progress) and the
glow (`MascotPose::glow`, in percent); `ui::header::art_spans` lifts each
cell by whichever lifts it more.

The sweep: a band of light crosses the art once, left to right, in about three
quarters of a second: each cell is lifted from its gradient colour toward
the status line's own shimmer highlight (`mascot_shine_color()` is
`shimmer_highlight()`) by a raised-cosine band two and a half columns
either side of its crest, and each row sits half a column further along
than the row above, so the band leans like a glint instead of scanning like
a bar. It runs from a half-width before the art to a half-width past it,
so the light arrives and leaves by degrees and no cell pops. Under
a theme whose colours are the terminal's own (`ansi`), where there is
nothing to mix, the band's core simply flips to the highlight.

Every mascot gets one sweep as the screen opens, and one more a round. The
gem — *a cut gem catching the terminal light* — **glints** instead: the
sweep, with a spark opening as the light passes it. Its sparks are drawn in
that same highlight, since a spark is light, not gem; and its facet is
never shut, since a facet that closes and opens is an eye.

### The doze

A mascot left alone for a round — about two minutes — nods off: its lids
grow heavy, close, its head dips half a cell and stays down for most of a
second, it stirs, it starts awake wide-eyed, and it blinks the sleep out of
its eyes, twice. Once a round from then on,
and never in the first: a mascot that fell asleep as the app opened would
be saying something about the app. The skiter has no lid to lower — its
eyes close, it sinks, it is up again. The sprout cannot dip and the gem has
no eyes; neither dozes.

## When it moves

Mostly, it doesn't. The timeline is stillness with short clips in it — a
move about every three seconds, and the mascot at rest about four fifths of
the time:

1. **The greeting**, scripted, the same in every session: 0.4 s after the
   banner appears the light sweeps it; at 1.5 s the mascot shows its own
   move; at 2.6 s it blinks. (The gem: a glint, then two twinkles.)
2. **Beats** of three seconds from 3.6 s on, at most one clip to a beat,
   started wherever a drawn offset puts it — anywhere that leaves the clip
   whole inside its beat with 200 ms of stillness to spare, so two clips
   never run together and the gaps between them never repeat.

### Dealt, not rolled

What a beat plays used to be a roll against a table of weights. Reweighting
the table would have cured the blinking and nothing else: dice have no
memory, so a fair table still plays one move twice running and leaves
another unseen for minutes. The repertoire is a **deck** instead:

- every move is a **card**, with as many copies as a round should play it —
  the blink three, the mascot's signature two, most moves one — and four
  **rests**;
- a **round** is the deck, shuffled: some forty beats, two minutes (the
  gem's twenty-six, under a minute and a half). In it the mascot does everything it
  knows exactly as often as the deck holds it, so any two minutes show
  fifteen different moves or more;
- **never the same move twice running** — not across a rest, and not from
  the end of one round into the start of the next. A card with a left and
  a right (a glance, a wink, a peek) or a set of variants (the three
  stares and the crossed eyes; a bounce and a happy one) plays each in
  turn, in an order the round draws, and it is the *card* that may not
  repeat: never a wink after a wink, never a blink after a double blink;
- **never two rests in a row**, so however the cards fall it is never still
  for more than about eight and a half seconds;
- **the doze is held back from the first round**.

For the crest that is three blinks in thirty-five moves — one in twelve,
where it was three in five — seven of the whole body, a cheer and two waves,
six of its frill and feet, four of light, and from the second round a doze.

A round is dealt by `Motions::dealt(seed, round, beat)`, and three things
make it a function rather than a shuffle that has to be remembered:

- **The round's last move is drawn first**, from the cards it holds one
  copy of. Knowing how a round ends without dealing it is what lets the
  next round promise to open on something else — otherwise each round would
  need the one before it, back to the first.
- **The moves are dealt one at a time**, each drawn from every card left
  but the kind just played. A kind that has come to make up more than half
  of what is left goes at once: the one order that can still keep its
  copies apart. That rule is the difference between a deal that usually
  works and one that always does, and a test deals sixty rounds of every
  mascot under six seeds to say so.
- **Each rest is slipped in ahead of a move of its own**, chosen among the
  moves still to come — so two rests never meet and no round ends on one.

Two more rules shape the timeline:

- **A clip never pauses as long as a breath.** Inside a clip — between two
  bounces, the two halves of a double blink — the mascot stands still for
  160 ms at most, less than the 200 ms that parts two clips, so one move
  never reads as two.
- **Each mascot keeps its own time.** The deal is drawn over the seed *and*
  the mascot, so no two mascots play one sequence: walking the `/mascot`
  list, each goes on from its greeting in an order of its own rather than
  repeating the last one's move for move. (Under one seed alone they did
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

The beat a clock falls in is a division; its card is the deal of its round
— some thirty draws of a SplitMix64 hash, a microsecond's work; the clip and
its step are a table walk. The pose three hours in costs what the pose three
seconds in does, a loop that was busy (or behind an overlay) for a minute
simply asks again and gets the right answer, and the whole thing is
unit-tested with nothing but numbers.

## Watching the user type

The banner is live for exactly as long as the first message is being
written, so that is when the mascot pays attention to it: while a draft is
typed its eyes **follow the caret** across the composer — to the left while
the caret is in the composer's left third, straight ahead in the middle, to
the right in the right third — reading along. The gem, which has no eyes,
turns its facet toward the caret's side. It blinks now and then while it
watches (once in every 3.2 s, at an offset the seed draws, never in the
first 0.9 s, so the eyes turning is what a watch opens on). Typing wakes it
from whatever it was doing, a doze included.

```
 ▙▄▙▄▟▄▟   Alter Zero (v0.19.0)       ▙▄▙▄▟▄▟   Alter Zero (v0.19.0)
▝▜▟███▟▛▘  ~/alter-zero              ▝▜▙███▙▛▘  ~/alter-zero
  ▘▘ ▝▝    / commands  @ files          ▘▘ ▝▝    / commands  @ files

❯ hello                              ❯ hello the quick brown fox jumps over the lazy dog
 the caret in the left third          …and in the right third
```

Four seconds after the last keystroke (`MASCOT_WATCH_LINGER`) — long enough
to wait out a pause to think — it looks away and goes back to its own moves,
and an **emptied** composer ends the watch at once: a message sent, a
`/clear` run, Ctrl+U — there is nothing left to read. Going back, it never
joins a move half way: a clip its timeline started while it watched is left
out (`Mascot::pose_after`), and it stands still until the next.

Nothing is tracked through the keys. `App::set_mascot_clock` — the reading
every draw injects — compares the composer's draft and caret with the last
frame's (`watch_the_composer`), so any change is a keystroke: a key, a
paste, a recalled message, the caret moved. The first frame only looks, and
a watch from before the clock started over (`/clear`, a `/mascot` choice)
is forgotten, so the fresh banner greets. *Where* the caret is is `ui`'s to
measure (`layout::caret_gaze` — its column on its own wrapped row, so a draft
that wraps sends the eyes back to the left with it) and the pose is
`App::banner_pose_watching(gaze)`, a pure function of the seed, the clock,
the watch and the gaze. A keystroke brings a frame of its own, so the loop
schedules only the watch's blinks and its end (`Mascot::watched_holds`).

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
before the cursor is seated. A blink is two cells — 95 bytes on the wire;
a frame that moves the whole mascot redraws its three rows, some 550 bytes;
and a round of everything a mascot knows, a minute and a half of idling,
comes to about 40 kB.

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

- a blink is four frames (lid, shut, lid, open), a bounce four, a dance
  eight — then the loop sleeps until the next clip;
- the sweep is nineteen frames, 40 ms apart, at launch and once a round; a
  glint twelve; a glow eleven, one for each of its levels;
- while the user types, a keystroke brings its frame with it — the mascot
  adds only its blinks and the frame that looks away;
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
On the session's clock it would have stood wherever that clock's round had
got to — a rest, a blink, a move every mascot shares — on the page whose
whole job is telling the six apart. Left on a mascot, the preview goes on
through its deck like the banner does.

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
| The frames, the clips, the decks and their deal (`Card`, `Motions::dealt`), the timeline, the watch (`Gaze`, `Watch`, `watched_pose`, `pose_after`), the session's side (`banner_is_live`, `banner_pose_watching`, `watch_the_composer`, `mascot_frame_in`) | `src/app/mascot_motion.rs` |
| The posed banner and its light (`header_lines_posed`, `banner_frame_lines`, `banner_block_lines`, `banner_shows_art`, `shine_at`, `art_spans`) | `src/ui/header.rs`; the look (`MASCOT_SHINE_*`, `MASCOT_GLINT_*`) in `src/ui/theme.rs` |
| Where the caret is (`caret_gaze`) | `src/ui/layout.rs` |
| The picker's moving preview | `src/ui/mascot_view.rs` |
| The anchored block's bookkeeping and cell diff | `src/term/anchor.rs` |
| Declaring, posing, settling and releasing it | `src/term.rs` (`anchor_above`, `pose_anchor`, `release_anchor`, `paint_anchor`, `settle_anchor`) |
| The seed, the clock, the frame scheduling, the release | `src/tui/bootstrap.rs`, `src/tui/view.rs` (`pose_banner`, `mascot_frame_due`) |
| The row and its override | `src/settings.rs`, `src/tui/config.rs` |

## Tests

- `app/tests/mascot_motion.rs` — the frames (the rest art's rows, never
  wider, single-width glyphs in the art's own alphabet, none trailing a
  blank, and each one row redrawn, a limb raised across two rows at the
  art's edges, the stone turned about its middle, or the whole mascot moved
  half a cell — checked by folding the art onto its half-cell grid and
  moving it back; the mascots with limbs at their sides raise them; the gem
  spins edge-on and back the way it went);
  the timeline (opens at rest; a pose holds exactly as long as it says,
  checked a millisecond at a time; the same seed the same timeline, another
  seed another; two mascots on one seed out of step); the greeting (the same
  for every seed: a sweep, the mascot's own move, a blink; one sweep from
  edge to edge); the light (every mascot glows and fades by degrees, never
  as bright as a flash; a glint crosses from either side, quicker than the
  sweep; the gem flashes twice, dark between, and nothing else flashes);
  the rhythm (at rest far more than moving, a move every few
  seconds, never still past nine; a clip never runs into the next; every
  frame reaches the screen); the repertoire (never the same move twice
  running; blinks under fifteen moves in a hundred, where they were seventy;
  any two minutes show fifteen different moves or more and none of them
  more than six times; every mascot moves its whole body at least three
  times in any two hundred seconds; a doze comes to the four that can, and
  never in the first eighty seconds); the moves (eyes shut through a lid
  where the glyphs have one; a blink quick and slower to open; both eyes
  glance one way, cross, and never part; a wink shuts one eye; only lidded
  eyes widen and squint; a lidded eye winks through its lid; some moves
  close on a blink; a wave closes on a wink of the eye beside the arm; feet
  patter on the spot; eyes wander off and blink where they went; no look but
  the ones eyes are known to give; the gem sparks at four corners, turns its
  facet and never shuts it); watching the user type (a caret is read by the
  third of the composer it sits in; the eyes follow it, the gem's facet
  turns to it; a watching mascot still blinks now and then; it goes back to
  its own moves when the typing stops, never joining a move half way; a
  frame is due at every change while it watches, and none for nothing; an
  emptied composer ends the watch at once; a launch watches nothing; a clock
  that starts over forgets the watch); and the session's side (unseeded never animates; only an empty conversation's
  banner is posed; a turn in flight stills it; the row turns it off and on;
  no frame for a banner off screen or behind an overlay; the picker's
  preview runs on its own clock — a mascot greets as the highlight lands on
  it, by a key or a search, again when the picker is reopened, and starts
  over with a session clock that does — while the banner keeps its own).
- `app/mascot_motion.rs` (in place, on the cards rather than the poses) —
  a round deals every card it holds and every rest; no move is dealt twice
  running and no two rests meet, across sixty rounds and both ends of each;
  a card's moves take turns, from a different one round by round; a late
  card stays out of the first round; a deck's first card is the move its
  greeting closes on, which the first round never opens with; every clip
  fits a beat with a breath to spare and never pauses as long as one; no
  card crowds its round; every round has several moves it can close on; no
  two cards play the same move; every round is shuffled afresh; every
  mascot knows a score of moves. Six ways of breaking the dealer — a move
  allowed to follow itself, no crowding rule, a round forgetting how the
  last one closed, rests falling anywhere, the doze dealt at once, a card
  always leading with its first move — each fail these.
- `ui/tests/header.rs` — the committed banner wears the rest pose whatever
  the clock says; the live banner draws the pose beside unmoved metadata
  (a blink: two cells); no pose moves the metadata or the banner's shape; a
  banner with nothing moving *is* the committed banner; a sweep lights the
  art under its band and nothing else, leans, and enters and leaves without
  a jump; a glow lifts every cell of the art alike; a glint lights a narrow
  band from its own side; the live banner follows the caret across the
  composer; a spark is drawn in the sweep's light; the narrow badge has no
  art to move.
- `ui/tests/mascot_view.rs` — the preview shows the highlighted mascot
  moving; a moving preview never moves the page.
- `term/anchor.rs` — a block declared while its rows are queued seats once
  they flush; rows queued ahead of it are not under it and rows committed
  after it are; a block with a row off the top has no seat; a still block
  has nothing due; a pose is due until painted; settling keeps the pose; a
  released block wants its rest rows; a blink rewrites the eyes' two cells
  and nothing else; a move of the whole mascot redraws its art — rows
  longer and shorter than the ones they replace — and not a cell of the
  title beside it, and is put back cell for cell.
- `settings/tests.rs` — the row: on by default and off the wire, cycles,
  sits after the picture rows, found by what it does, recorded per
  directory.
- `scripts/smoke.sh` Phase 154 — on the raw byte stream: the greeting's
  frames reach the terminal (the sweep's frames ahead of the flare, the
  first eyelid within a frame count no clock chain could keep) and leave one
  banner at rest over one composer; with the row off an idle session writes
  nothing; a first message sent while the eyes watch the draft leaves the
  rest art above the conversation and nothing is written afterwards; the
  eyes follow the caret into the composer's left and right thirds, look away
  about four seconds after the last keystroke, and at once when Ctrl+U
  empties the draft; `/clear`
  greets again (about thirty frames in the three seconds after it, the
  flared frill among them, where a mascot carrying on would write a clip's
  handful) and leaves one banner at rest; quitting mid-move leaves the rest
  art on the dead screen; the `/mascot` preview greets as a search lands
  the highlight on a mascot; and the mascot just chosen greets from the
  banner.
