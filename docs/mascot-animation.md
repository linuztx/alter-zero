# The banner mascot's idle animation

While the conversation is empty — a fresh launch, or the moment after
`/clear` — the banner's mascot is alive: a light sweeps it as the screen
opens, it shows the one move its catalog line promises, it blinks — and from
then on it works through everything it knows, a move every few seconds. It
bounces, wiggles, peeks to one side, waves and winks, cheers, stares, taps
its feet, pumps its arms, catches the light, daydreams, ducks out of sight
and peeks back over the edge, turns a somersault.

And it keeps the user **company**. While the first message is typed it
**watches**: its eyes turn to the text — to its right, where the line runs
and the rest of the screen is. Left alone for three quarters of a minute it
**naps** — its eyes close, each a thin lid line where the eye was, its
head goes down, and it sleeps there, still, a `z` coming and going in the
corner of its block. And the first key that finds it asleep
**wakes it with a start**: a `!`, it is wide awake, and it turns to what is
being typed. The first cell committed under the banner
ends it all: from there the banner is scrollback, and scrollback does not
move.

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

Each mascot has a **repertoire** of thirty moves and more (the gem, with no
eyes to make faces with, a score). The first version had six, and three
beats in five were a blink; "it's always just blinking" was a fair review.
The second gave each a score; the third — ported in part from another
branch's take on the same feature — added limbs, phrases and light; the
fourth, tricks; the fifth took the stars out of its eyes and taught it to
sleep (*The nap*, below); the sixth stilled the sleeper and gave the
skitterer its legs to gesture with. The moves come in six families:

- **Looks** — what a pair of eyes does: a **blink** (and a double), a
  **glance** to either side, a **look around**, a **shifty** dart from side
  to side, a **wink** of either eye, a **cross-eyed** moment, and a
  **daydream** — the eyes gone to one side and staying there, blinking
  while they are away. The three whose eyes have a lid add a **startled**
  wide stare, a contented **squint**, and one eye of each — the look of
  something that is not so sure.
- **Phrases** — two moves that belong together, the second the first's
  punctuation: a **wave** and a wink of the eye beside the arm (a hello), a
  **cheer** — a spring with the arms (petals, ears) thrown up, twice, the
  skitterer puffing itself up — and a blink, a startled stare or
  crossed eyes that collect themselves
  with a blink. A phrase pauses inside itself for a beat (150 ms), less
  than the breath that parts two moves, so it reads as one.
- **The whole body, half a cell** — a **bounce** (it dips and is up again,
  twice), a **happy bounce** (three, eyes squeezed shut on every landing), a
  **wiggle** from side to side, a little **dance** (down to the left, down
  to the right), and a **peek**: the eyes go to one side and the mascot
  leans after them.
- **Tricks** — the whole mascot somewhere it does not usually go. It
  **hides**: sinks below the floor of its block until only its top shows,
  comes back up to its eyes, looks both ways over the edge, and pops up.
  And it **flips**: a **somersault** — a crouch, an instant upside down
  with its legs in the air, the landing — or a **cartwheel**, a lean to one
  side, upside down, and down on the other.
- **Its own moves** — the thing its catalog description says it does, and
  three or four more (the table below) — among them a **pump**, a little
  victory dance every eyed mascot has in its own limbs: one goes up on one
  side, then the other, and again — the skitterer, which has none to
  raise, puffing out one side of itself and then the other.
- **Light** — the sweep that opens the greeting comes back once a round; a
  **glint**, a narrow catch of the light, crosses from either side; a
  **glow** swells through the whole mascot and fades, a breath of light;
  and it **glitters**, a scatter of its cells twinkling.

| mascot | its own moves | its whole body |
|---|---|---|
| `crest` | **flares its frill** (the spikes pull in, fan out to six, settle); a **ripple** runs along it, each spike dropping in turn, there and back; it **ducks** — frill flat — and springs up flared; it **waves** an arm from the shoulder and winks; it **cheers**, both arms high, or **pumps** them, one then the other; it **taps** or **patters** its feet | bounces, wiggles, dances, peeks, hides, somersaults, cartwheels |
| `bloom` | **flutters** its petals (down and up, three times) or **flaps** them (up to the head, down to the hem); **seesaws** them; **puffs up** its head, or **swells** it out to the block's edges; its crown **wobbles** from side to side; **tucks** its petals in, shy; **cheers** with them up, or lifts each in turn; taps or patters its feet | bounces, hides, somersaults — it fills its block from edge to edge, so it never leans, and never cartwheels |
| `sprout` | its shoot **sways**; it **unfurls a leaf** to either side, or **spreads both**, or **blossoms** — two leaves opening as wide as they go; it **sinks to a bud and pushes up again**; it throws its **arms up**, three times, and blinks; it **waves** one, and sometimes winks after, or **pumps** them, one then the other; it **patters** its feet | wiggles, peeks, hides, cartwheels — its feet stand on the block's last half-row, so it never bounces, and never somersaults |
| `twin` | **flicks an ear** (either one, twice); **wiggles** both out and in, or **splays** them to the corners; **sways** them; they **droop** and spring up pricked; they **see-saw**, one folding as the other stands; it **cheers** with them splayed; it taps or patters its feet | bounces, wiggles, dances, peeks, hides, somersaults, cartwheels |
| `skiter` | **skitters**; a **wave** runs down its six legs, twice; it **stretches** them — drawn in, then spread wide; it **puffs up**, its body swollen to the edges of its block, twice; it puffs out **one side and then the other**; it **cheers**, springing from a crouch and puffing itself up | bounces, wiggles, **scuttles** (half a cell each way, stopping between), dances, peeks, somersaults, cartwheels, and hides as far as its eyes — they are on its top row, so that is as deep as it goes |
| `gem` | a **spark** opens at any of its four corners, at two in turn, or **all round**; its **facet slides** from side to side as the stone turns, stands tall when it **gleams**, **pulses**; it **spins** — narrower, thin, edge-on and back; a sweep crosses it with a spark; it **flashes** — to full strength, dark, and again | bounces, **floats** (a slow bob), wiggles, dances, rocks slowly from side to side, and **sets like a sun** — half down, down to its tip — to rise glowing. Upside down it is itself, so it never flips |

### One part moved, or the whole mascot

A frame is one of seven things, and the tests hold every frame to it:

- **the rest art with one part moved** — the eyes, a crest, a leaf, an ear,
  a foot, a leg, a spark — so it differs from the rest art on exactly one
  row;
- **a limb raised from the body's side** — the crest's arm, the bloom's
  petals, each a quarter cell at the end of the middle row, lifted to the
  shoulder: two neighbouring rows redrawn, and nothing in them but the
  art's two edge columns;
- **the body puffed out** — the skitterer's: swollen toward the edges of
  its block, so every quarter cell of the rest art is still inked and all
  that is added lies outside the body, past one end of a half-row of it or
  the other. Nothing is taken away and nothing inside it is filled in — an
  eye closed is a blink, not a breath taken — so it is the same creature,
  bigger;
- **the stone turned** — the gem's spin: every half-row of it as wide as the
  rest art's or narrower, about the same middle, down to a single column
  edge-on;
- **the mascot sunk below the floor** — the rest art moved down one or two
  whole rows, the rows above it blank and its feet (then its body) out of
  sight under the block's last row; what still shows is as drawn, but for
  the eyes, turned to one side as it looks over the edge;
- **the mascot upside down** — the rest art turned top to bottom on its
  half-cell grid: exactly the same creature, legs in the air; or
- **the whole mascot moved half a cell.** The art is drawn on a half-cell
  grid (a quadrant glyph is four quarters of a cell), and every mascot
  leaves a half-row free under its feet, a half-column free at each side,
  or both. So the same figure can be drawn half a cell lower, or to a side,
  inside the same three rows and nine columns: every row is redrawn and
  nothing has changed. Moved back, it is the rest art again — with one part
  moved at most, since a happy bounce shuts the eyes as it dips and a peek
  leans with the eyes already turned.

A **sleeper's eyes are closed** over whichever of these its frame is — the
seedling asleep droops its shoot *and* has its eyes closed, two rows of it
redrawn — so a frame with both eyes closed is held to the rule as the frame
it would be awake. Closed for sleep, each eye is its **lid** (*Eyes*,
below): a line in the eye's own cell, so the eyes still redraw one row.

Beside any of these a **mark** may ride in the block's top right corner —
the `z` and `Z` of a snore, the `!` of a start: light, not mascot, in the
one cell beside its head every mascot leaves empty when it is down, and
never anywhere else.

**The skitterer puffs itself up, and raises nothing.** It has no arm to
throw up, and twice it was given one. The bumps at its sides were claws it
lifted to its shoulders — and a claw half a cell wide, raised, is a thin
stick beside its head: it read as a small hand stuck on, and was not liked.
Then it stood tall on the tips of its six legs instead, each stretched to
the floor of its block, and that was liked no better. What it does now was
picked from eight candidates played side by side: it **puffs up**. Its body
swells to the edges of its block — a quarter cell or two at each end of
every half-row of it, the cut corners kept — with its eyes and its legs as
drawn. So "arms up" is the whole creature bigger, and its pump puffs out
one side of it and then the other:

```
▗▟▀███▀▙▖    ▟█▀███▀█▙    ▟█▀███▀▙▖    ▗▟▀███▀█▙
▝▜█████▛▘    ▜███████▛    ▜██████▛▘    ▝▜██████▛
 ▘▘▝ ▘▝▝      ▘▘▝ ▘▝▝      ▘▘▝ ▘▝▝      ▘▘▝ ▘▝▝
 at rest      puffed up    …one side    …the other
```

Puffed up is held a quarter of a second each time — a sixth in the spring
of a cheer — so it is seen, not flashed. Two tests hold it there: no frame
of the skitterer raises anything at its sides or stretches a leg to the
floor, and the puff is where the others' arms go up — a move of its own,
its cheer, and the end of its start.

Either way a frame keeps the art's three rows and is never wider than the
art: the banner's metadata column and the composer under it never shift.
Nothing moves up — no mascot has a free half-row above it — so a bounce is
a dip and a recovery, which is what a bounce looks like from the landing,
and a hide goes *down*, behind the floor, which costs nothing: a sunk row
is simply not drawn.

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
  block would draw the missing lid — it is how a sleeper's is drawn, below
  — but it paints its gap in the terminal's opaque background colour, a
  shade off the background of a translucent terminal, and a frame 35 ms
  long is not worth even that.
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
  broken frames (drawn a second time when the stars went, and thrown out a
  second time).
- **Closed for sleep, an eye keeps its lid.** A blink can fill the holes
  for a tenth of a second; a sleeper whose eyes are filled in for most of a
  minute has no face left — the first thing said of the nap was that its
  eyes were *empty*. So an eye closed for sleep is a thin line where the
  eye was: a **lid**, along the bottom of its socket, where a lid comes to
  rest.

  ```
   ▙▄▙▄▟▄▟      ▙▄▙▄▟▄▟      ▖ ▖ ▗ ▗z
  ▝▜▄███▄▛▘    ▝▜━███━▛▘    ▗█▁███▁█▖
    ▘▘ ▝▝        ▘▘ ▝▝       ▝▛▛▀▜▜▘
   awake        its eyes close   …and it settles
  ```

  A socket is the upper half of its cell or the lower, so the line runs
  through the middle of the cell (`━`) or along its bottom (`▁`) — and half
  a cell lower when the sleeper's head goes down, which swaps the two. No
  block glyph leaves either line *dark*: the eighth-block ramp fills a cell
  from the bottom up, and its mirror image is in the Legacy Computing
  blocks. So a lid's cell is drawn **in reverse** (`MASCOT_LIDS`, the one
  cell `ui::header::art_spans` reverses): the body's colour at that column
  fills it — lit like the body when the body is lit — and the line is left
  in the ground's. A closed eye is then what an open one is, a gap in the
  body, and never a mark on it. It costs what reverse video always costs —
  the line is the terminal's *opaque* background colour — and for a line
  two pixels tall, inside the body, that is nothing. Slits a quarter of a
  cell tall were drawn too, and read as small open eyes; a line along the
  *top* of the socket read as a pair of eyebrows. Only the nap's frames
  have lids: awake, a shut eye is a blink, and drawn as it always was.
- **And they never hold a light.** *Starry eyes* — `·`, `✢`, `✶` opening in
  each socket — shipped once, as delight. On a contact sheet they read as
  eyes lit up; in a terminal the spark is a thin text glyph in the
  highlight on the terminal's own background, and a socket filled with it
  reads as an eye put out. An eye is a hole — open, or closed to a lid —
  and a test holds every frame to it. The pump took the move's place in
  every deck.

### Light

Light is colour only — never a glyph — and falls on ink: a blank cell keeps
its colour, so the wire carries no repaint of empty cells. Five kinds:

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
- **Glitter**: no band at all — a scatter of the mascot's cells twinkles at
  full strength, a different scatter at each of nine steps 80 ms apart
  (`ShineBand::Glitter`, `MASCOT_GLITTER_STEPS`), each twinkle keeping a
  faint afterglow for the step after it so it fades rather than snaps off.
  The scatter is thickest in the middle — about one cell in three — and
  thins to either end, so the glitter arrives and leaves by degrees. Which
  cells is a hash of the cell and the step (`ui::header::glitter_at`): a
  pure function like every other light, and the same glitter every time.
  One a round.

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
that same highlight, since a spark is light, not gem — as are the marks
beside a sleeper, the `z` of a snore and the `!` of a start: anything in a
frame that is not a block glyph. And its facet is never shut, since a facet
that closes and opens is an eye.

## When it moves

Mostly, it doesn't. Awake, its timeline is stillness with short clips in it
— a move about every three seconds, and the mascot at rest about four
fifths of the time:

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
- a **round** is the deck, shuffled: some forty-four beats, a little over
  two minutes (the gem's twenty-eight, under a minute and a half). In it the mascot does everything it
  knows exactly as often as the deck holds it, so any two minutes show
  fifteen different moves or more;
- **never the same move twice running** — not across a rest, and not from
  the end of one round into the start of the next. A card with a left and
  a right (a glance, a wink, a peek) or a set of variants (the three
  stares and the crossed eyes; a bounce and a happy one) plays each in
  turn, in an order the round draws, and it is the *card* that may not
  repeat: never a wink after a wink, never a blink after a double blink;
- **never two rests in a row**, so however the cards fall it is never still
  for more than about eight and a half seconds.

For the crest that is three blinks in forty moves — one in thirteen, where
it was three in five — seven of the whole body, three tricks (a hide, a
somersault, a cartwheel), a cheer, two waves and a pump of its arms, six of
its frill and feet, and five of light.

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
let pose  = mascot.pose(seed, clock);        // the art's rows + the light on them
let sleep = mascot.pose_holds(seed, clock);  // until the pose next changes
```

That is the mascot **kept awake** — its moves, and what the `/mascot`
preview shows. The banner's own pose (`App::banner_pose`) lays the user's
company over it — the watch, the nap, the start, the next two sections —
and is the same kind of function, of the seed, the clock, and three numbers
a frame keeps of what it found (`Company`, below).

The beat a clock falls in is a division; its card is the deal of its round
— some thirty draws of a SplitMix64 hash, a microsecond's work; the clip and
its step are a table walk. The pose three hours in costs what the pose three
seconds in does, a loop that was busy (or behind an overlay) for a minute
simply asks again and gets the right answer, and the whole thing is
unit-tested with nothing but numbers.

## Watching the user type

The banner is live for exactly as long as the first message is being
written, so that is when the mascot pays attention to it: while a draft is
typed its eyes **turn to the text**, read from where the mascot stands. It
stands at the screen's left edge, over the composer's first columns, so a
line runs off to its **right** — and so does everything else on screen: its
eyes go right as soon as the caret is past its middle, which the third
character of a line is, and stay straight ahead while the caret is still
under it. The gem, which has no eyes, turns its facet the same way. It blinks now and then while it
watches (once in every 3.2 s, at an offset the seed draws, never in the
first 0.9 s, so the eyes turning is what a watch opens on). Typing takes it
from whatever it was doing — a nap included, with a start (*The nap*,
below).

```
 ▙▄▙▄▟▄▟   Alter Zero (v0.19.0)       ▙▄▙▄▟▄▟   Alter Zero (v0.19.0)
▝▜▄███▄▛▘  ~/alter-zero              ▝▜▙███▙▛▘  ~/alter-zero
  ▘▘ ▝▝    / commands  @ files          ▘▘ ▝▝    / commands  @ files

❯ hi                                 ❯ hello there
 the caret still under it             …and past it: it looks toward the text
```

It first read the caret by the **third of the composer** it sat in — left,
ahead, right, as if reading along the line. But the mascot is not above the
middle of the composer; it is at its left end. A short message never leaves
the left third, so through nearly everything anyone typed it stared at the
left edge of the screen, where there is nothing. `Gaze::toward` reads the
caret's screen column against the columns the mascot stands over instead:
past its middle is right, short of its columns is left — which at the
screen's edge no caret is — and the rest is under it.

Four seconds after the last keystroke (`MASCOT_WATCH_LINGER`) — long enough
to wait out a pause to think — it looks away and goes back to its own moves,
and an **emptied** composer ends the watch at once: a message sent, a
`/clear` run, Ctrl+U — there is nothing left to read. Going back, it never
joins a move half way: a clip its timeline started while it watched is left
out (`Mascot::pose_between`), and it stands still until the next.

Nothing is tracked through what the keys mean. `App::set_mascot_clock` — the
reading every draw injects — compares what it finds with the last frame's
(`keep_company`): the composer's draft, its caret, and a count of the keys
and pastes the app has taken. Any change is a **sign of the user** — a key,
a paste, a recalled message, the caret moved — and over a draft it starts
the watch or carries it on. The first frame only looks, and what was seen
before the clock started over (`/clear`, a `/mascot` choice) is forgotten,
so the fresh banner greets. *Where* the caret is is `ui`'s to measure
(`layout::caret_gaze` — its screen column on its own wrapped row, so a
draft that wraps brings the eyes back ahead with it) and the pose is
`App::banner_pose_watching(gaze)`, a pure function of the seed, the clock,
the company kept and the gaze. A keystroke brings a frame of its own, so
the loop schedules only the watch's blinks and its end
(`Mascot::company_holds`).

## The nap

A mascot nobody is typing at has nothing to wait for, and after three
quarters of a minute of it — every one of the six — it **naps**:

```
 ▙▄▙▄▟▄▟       ▖ ▖ ▗ ▗       ▖ ▖ ▗ ▗z      ▖ ▖ ▗ ▗Z      ▖ ▖ ▗ ▗!      ▙▄▙▄▟▄▟!
▝▜━███━▛▘     ▗█▁███▁█▖     ▗█▁███▁█▖     ▗█▁███▁█▖     ▗█▀███▀█▖     ▝▜▂███▂▛▘
  ▘▘ ▝▝        ▝▛▛▀▜▜▘       ▝▛▛▀▜▜▘       ▝▛▛▀▜▜▘       ▝▛▛▀▜▜▘        ▘▘ ▝▝
 its eyes close  it settles   a snore       louder        a key!       up, wide awake
```

- **It nods off**, and it is seen coming: its eyes grow heavy — through
  their lid, where they have one — open again, grow heavy and close, all
  of it on its feet. A second and a half. The seedling has no lid to
  lower, so its eyes close, open and close for good; the skitterer settles
  onto its belly first and closes them there; the gem, with no eyes at
  all, settles with a last light fading out of it.
- **It settles, once.** Its head goes down half a cell — the seedling,
  which has no room to dip, droops its shoot to a bud — and there it
  sleeps, all of it on screen, its eyes closed: each a lid, a thin line
  where the eye was, not a blank (*Eyes*).
- **Asleep it holds still**, and its snore is all of it that moves: a
  quiet moment, a `z` in the top corner of its block that grows to a `Z`,
  and gone — a breath every 3.4 s, three frames of one cell each. (The `z`
  and the `!` are the only letters the animation draws: plain ASCII, so no
  font lacks them, and what a sleeper and a start have been in every comic
  strip since comic strips.) A sleeper is never mistaken for a mascot at
  rest — no stage of a breath is the rest art — and never looks frozen:
  the snore changes every second and a third at most.
- **A nap ends by itself** after nine to thirteen breaths — the better part
  of a minute: an eye half open, open, a **stretch** that is the mascot's
  own (the crest's frill flared, the bloom's head swollen out, the twin's
  ears splayed, the seedling's leaves spread wide, every leg of the
  skitterer), and two blinks to clear the sleep away. The gem's light comes
  up where it lies and it **rises glowing**, the glow fading once it is
  afloat again, and a spark opens at its corner.
- **Then it plays again**, and naps again: one nap to every two minutes of
  being left alone, begun three quarters of a minute to a minute in — the
  start drawn afresh each time, from the session, the mascot and the moment
  it was left, so it never nods off on the dot.
- **The third time it stays asleep.** Five minutes into being left alone a
  nap has no end: whoever comes back to the keyboard an hour later, or the
  next morning, finds it sleeping — still breathing — and wakes it.

A nap never cuts a move in two: a clip that would still be running as the
mascot nods off is left out whole, as is one begun before it is up again
(`Mascot::plays`), so it goes under standing still and comes round to a
move with a start of its own. Not sooner than three quarters of a minute,
either: a mascot asleep as the app opened would be saying something about
the app.

**It was first built lying down** — the art a row lower in its block, its
feet below the floor, the snore drifting along the emptied row above — and
that was taken out. Three rows are too few to lie down in: a mascot with
its legs out of sight reads as a mascot cut in half, and the first question
the sleeper drew was why only half of it was there. The doze it replaced —
one clip of the repertoire: eyes shut, the head dipping, a `z` and a `Z` in
the corner — had the look worth keeping, so the nap is that look, held for
the length of a nap.

**And then it breathed with its whole body** — up on its feet on the breath
in, half a cell down on the breath out, the gem's light swelling and fading
— and that went too. Half a cell is the smallest step the grid has, so a
breath was not a rise and fall but a hop: a sleeper that jumped up and down
every three seconds, and nodded twice on its way under. A thing asleep is
still. The body goes down once and stays; what says it is alive is the
snore, which was always the part that read.

### Woken with a start

**Any key wakes it** — and it is the one moment the mascot answers the user
directly. Its eyes snap open **where it lies**, a `!` in the corner where
the snore was; then it is up, wide awake — an eighth of a second after the
key for all but the skitterer, the `!` still beside it — and three fifths
of a second after the key the start has worn off:

| | the start |
|---|---|
| `crest`, `bloom`, `twin` | eyes open where it lies; then up with eyes **wide** (`▂`) |
| `sprout` | eyes open under its drooped shoot; then up with **both arms thrown in the air** |
| `skiter` | eyes open, frozen on its belly; then up, **puffed out** to the edges of its block |
| `gem` | it lights up where it lies, and is up in a **flash** — to full strength, and fading |

**A start never takes a mascot down.** One caught while it is still nodding
off — its eyes heavy or just closed, the four that droop on their feet not
yet down — starts **where it stands**: wide-eyed at once (the seedling's
arms up), the `!` beside it, for the same three fifths of a second
(`Nap::caught`). It used to play the sleeper's start from wherever it was,
and that was a **flicker**: a key that found it standing dropped it into
the sleeper's crouch for an eighth of a second and brought it straight back
up — the whole block redrawn twice — which is what typing looked like just
as a nap began. (The breathing sleeper had the same fault for the third of
every breath it spent on its feet.) So which start plays is recorded with
the key that woke it (`Woke::nodding`), and asleep there is only one place
to start from. The skitterer and the stone, down from the first step of
their nod, start as they do asleep.

Then, if something is being typed, it **turns to it** — the watch, its eyes
on the caret, as if it had been awake all along — and it reads a while
before it blinks: the watch's blinks are counted from the end of the start,
not from the key, where the first used to land a third of a second after
the eyes had turned. A key that types nothing (an arrow in an empty
composer, a picker opened) leaves it standing, and it goes back to its own
moves. Either way it is *up*: the next nap is three quarters of a minute
off again, counted from the last sign of the user, or from the moment the
watch looked away. One already stirring by itself is not surprised, and
simply looks.

**A key that types nothing leaves a move alone.** Awake, with nothing in
the composer to look at, a key says only that someone is there: the nap is
put off and the move the mascot is in the middle of **plays on**. It used
to stop dead at the key — a glance cut off a twentieth of a second in, a
flip left upside down for a frame — because the timeline was restarted from
every sign of the user. Now only what gives it something to do restarts it:
a watch, a start. One case still does (`App::keep_company`): a move the
coming nap had left out, which the key — by putting the nap off — would
otherwise bring in from its middle.

### Company, kept in four numbers

What the mascot knows of the user is a `Company`: the run of typing it is
watching (`watch`), when it last saw a sign of anyone (`last`), the sign
that last found it napping and where in its nap (`woke`), and since when
its timeline has been its own to play (`up`). `keep_company` updates them
on the one frame that finds something changed — and the count of keys taken
is what lets it notice someone who changed nothing it could see: an arrow
key, Esc, a paste into an open search. That count is a number bumped at the
top of `App::on_key` and `App::on_paste`, never what the key was.

Everything else is arithmetic on the clock:

- the mascot has been **left to itself** (`Mascot::alone`) in two senses,
  which a key that types nothing tells apart: its *moves* are its own from
  the end of its watch or its start — or from `up` — and its *naps* are
  counted from the latest of that and `last`;
- a stretch of being left alone is cut into two-minute pieces with one nap
  in each, so *which* nap an instant falls in is a division, and its start
  and length two draws of the same SplitMix64 hash the deck is dealt with
  (`Mascot::nap`, `Mascot::idle`) — asleep an hour in costs what asleep a
  minute in does;
- asleep, the pose is a step of one of three clips — nodding off, a breath
  (taken modulo its length), waking (`Mascot::sleeping`) — and the start is
  one of two more, played from `woke`: from where it lies, or from where it
  stood (`Mascot::start`).

So the nap is as stateless as the blink: a loop that was busy for a minute
asks again and gets the right answer, and the whole of it is unit-tested
with numbers and a handful of key presses.

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
the skitterer's puff is the eight cells at its sides, 260 bytes;
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
  glint twelve; a glow eleven, one for each of its levels; a glitter nine
  scatters (a frame that finds the scatter unchanged writes nothing);
- while the user types, a keystroke brings its frame with it — the mascot
  adds only its blinks and the frame that looks away;
- asleep, a breath is three frames of one cell each — three and a half
  seconds of them — and nothing between: twenty seconds of a sleeping
  mascot measured 17 frames, 1.3 kB and 10 ms of CPU (a sleeper that
  breathed with its body wrote 28 frames and 7.6 kB), far less than the
  same twenty seconds awake;
- a start is three frames, on top of the one the key itself asked for;
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
through its deck like the banner does — and never naps: it is
`Mascot::pose`, the mascot kept awake, since the page is there to show how
each one moves.

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
  still and an idle screen's byte count is zero; Phases 159 and 160 turn it
  back on.
  `scripts/codegen_mem.py` sets it too: what that probe measures is the
  memory a turn leaves behind, with nothing else moving on the screen.

A headless run (`-p`) has no screen and no anchor, and an `App` nobody
seeded — every unit test's — never moves.

## Where the code lives

| | |
|---|---|
| The frames, the clips, the decks and their deal (`Card`, `Motions::dealt`), the timeline (`pose`, `pose_between`), the nap (`Nap`, `Sleep`, `Idle`, `nap`, `idle`, `sleeping`), the company kept (`Gaze`, `Watch`, `Woke`, `Company`, `Alone`, `alone`, `startled`, `start`, `moving`, `company_pose`, `company_holds`), the session's side (`banner_is_live`, `banner_pose_watching`, `keep_company`, `mascot_frame_in`) | `src/app/mascot_motion.rs` |
| The count of keys and pastes taken (`keys_taken`) | `src/app/keys.rs` (`on_key`), `src/app/composer.rs` (`on_paste`) |
| The posed banner and its light (`header_lines_posed`, `banner_frame_lines`, `banner_block_lines`, `banner_shows_art`, `shine_at`, `glitter_at`, `art_spans`) | `src/ui/header.rs`; the look (`MASCOT_SHINE_*`, `MASCOT_GLINT_*`, `MASCOT_GLITTER_*`) in `src/ui/theme.rs` |
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
  art's edges, the body puffed out past its own sides with nothing taken
  away, the stone turned about its middle, the mascot sunk whole rows
  below the floor, the mascot upside down, or the whole mascot moved half a
  cell — checked by folding the art onto its half-cell grid and moving it
  back, a sleeper's shut eyes opened first; a mark rides in the block's top
  right corner and nowhere else, and every mascot has all three; no light is
  ever put in an eye — it holds the body's blocks or its own lid; a lid is
  the line along the bottom of its socket, two to a frame, in an eye's cell
  and nowhere else, on its feet and with its head down; the mascots with
  limbs at their sides raise them; the skitterer puffs itself up — both
  sides, or one, its eyes and legs as drawn — raises nothing at its sides
  and stretches no leg to the floor, and is puffed up in a move of its own,
  in its cheer and at the end of its start, never for less than 150 ms; the
  gem spins edge-on and back the way it went);
  the timeline (opens at rest; a pose holds exactly as long as it says,
  checked a millisecond at a time; the same seed the same timeline, another
  seed another; two mascots on one seed out of step); the greeting (the same
  for every seed: a sweep, the mascot's own move, a blink; one sweep from
  edge to edge); the light (every mascot glows and fades by degrees, never
  as bright as a flash; a glint crosses from either side, quicker than the
  sweep; the gem flashes twice, dark between, and nothing else flashes;
  every mascot glitters);
  the rhythm (at rest far more than moving, a move every few
  seconds, never still past nine; a clip never runs into the next; every
  frame reaches the screen); the repertoire (never the same move twice
  running; blinks under fifteen moves in a hundred, where they were seventy;
  any two minutes show fifteen different moves or more and none of them
  more than six times; every mascot moves its whole body at least three
  times in any two hundred seconds); the moves (eyes shut through a lid
  where the glyphs have one; a blink quick and slower to open; both eyes
  glance one way, cross, and never part; a wink shuts one eye; only lidded
  eyes widen and squint; a lidded eye winks through its lid; some moves
  close on a blink; a wave closes on a wink of the eye beside the arm; feet
  patter on the spot; eyes wander off and blink where they went; no look but
  the ones eyes are known to give; the gem sparks at four corners, turns its
  facet and never shuts it); watching the user type (a caret is read from
  where the mascot stands — ahead while it is under it, right once it is
  past its middle, left only when it is off to its left; the eyes turn to
  it, and the gem's facet; a watching mascot still blinks now and then; it goes back to
  its own moves when the typing stops, never joining a move half way; a
  frame is due at every change while it watches, and none for nothing; an
  emptied composer ends the watch at once; a launch watches nothing; a clock
  that starts over forgets the watch); the tricks (a mascot ducks out of
  sight — down to its deepest and back up the way it went — and looks both
  ways over the edge; the gem sets like a sun and rises glowing; a mascot
  flips head over heels, upside down between a crouch and a landing or a
  lean and a lean, the bloom never cartwheeling and the seedling never
  somersaulting; every eyed mascot pumps its limbs, one side and then the
  other — the skitterer its two sides — its eyes where they were); the nap
  (a mascot left alone nods off
  three quarters of a minute to a minute in — all six, under six seeds —
  and is itself move for move until then, and all of it is on screen from
  the first nod to the last blink; its eyes grow heavy, open, and close
  before its first snore; falling asleep it goes down once and stays there
  — the skitterer and the stone down from the first frame — under six
  seeds; asleep its eyes are closed, not gone — two lids from the moment
  they close to the moment it stirs, none on open eyes and none in any move
  of the repertoire; asleep it holds still — one body from its first breath
  to its last, settled, no light coming and going in it, each step of its
  sleep redrawing the one cell its snore sits in — a `z` and then a `Z`,
  over and over, a breath every three or four seconds, nothing still past a
  second and a half; the snore rises in the block's top right corner; a nap
  ends by itself — a stretch, two blinks, nothing of it off screen — and
  its own timeline is back a breath later; the gem settles as a last light
  fades out of it, sleeps dark, and rises in a brighter glow; no move is
  cut short by a nap — a straddling one left out whole, and nothing but
  the nod and a quiet moment between the first sign of a nap and its first
  snore;
  two naps two minutes apart and then it sleeps until it is woken, still
  breathing an hour on, a week on, and at the end of the clock; no two
  naps begin at the same moment; a frame is due at every change while it
  sleeps, and none for nothing; the picker's preview shows a mascot awake;
  one that does not animate never sleeps); woken by the user (a keystroke
  wakes a sleeper with a start — a `!` in the block's corner as its eyes
  open, all of it still on screen, up inside half a second, the `!` gone
  inside four fifths, no snore after it; the start is the same wherever
  in its sleep the key finds it; caught on its feet it is never taken down
  — a key at every twentieth of a second through the nod and into the
  first breath keeps a standing mascot standing and brings a settled one
  up once; woken it reads a while before it blinks, under six seeds; a
  lidded mascot starts awake wide-eyed and the gem in a flash; the start
  first, whatever the caret is
  doing, and then its eyes on what is typed; any key wakes it, an arrow in
  an empty composer too, and then it is itself again; a paste wakes it,
  one that lands in an open search and leaves the draft alone included;
  woken it stays up three quarters of a minute; it naps again only after
  the typing stops, counted from when it looked away; a key keeps a
  wakeful mascot up and gives it no start; a key that types nothing
  leaves a move under way alone, and one as a nap nears never joins a move
  half way; caught nodding off it starts
  too; a key as it wakes by itself is no surprise; it can be woken from
  the sleep that has no end, and the count begins again; a frame is due at
  every change through a start; a clock that starts over forgets it was
  woken; a key before the first frame is nobody at the keyboard yet —
  twenty-seven ways of breaking the nap and the wake each fail these: a key
  that never startles, a mascot that never naps, a nap allowed to cut a
  move in two, a key that does not keep it up, keys left uncounted, pastes
  left uncounted, a nap that always ends by itself, one that begins on the
  dot, a start given to one already stirring, a start skipped for the
  watch, a clock that starts over and keeps its company, no frame due at
  the nap's start, a preview that naps, a sleeper with no snore, a nap
  counted from the clock instead of from when the watch looked away; the
  sleeper's start played to one caught on its feet, one sound asleep taken
  for one nodding off, a seedling that droops before it starts, a watch
  that blinks the moment the start is over, a key that types nothing
  stopping a move dead, a key bringing a left-out move in from its middle;
  a sleeper on its feet between snores, one that nods twice on its way
  under, a snore over another body, a seedling whose shoot comes up as it
  snores, a gem whose light breathes in its sleep, one whose light swells
  as it settles); and
  the session's side (unseeded never animates; only an empty conversation's
  banner is posed; a turn in flight stills it; the row turns it off and on;
  no frame for a banner off screen or behind an overlay; the picker's
  preview runs on its own clock — a mascot greets as the highlight lands on
  it, by a key or a search, again when the picker is reopened, and starts
  over with a session clock that does — while the banner keeps its own).
- `app/mascot_motion.rs` (in place, on the cards rather than the poses) —
  a round deals every card it holds and every rest; no move is dealt twice
  running and no two rests meet, across sixty rounds and both ends of each;
  a card's moves take turns, from a different one round by round; a deck's
  first card is the move its
  greeting closes on, which the first round never opens with; every clip
  fits a beat with a breath to spare and never pauses as long as one; no
  card crowds its round; every round has several moves it can close on; no
  two cards play the same move; every round is shuffled afresh; every
  mascot knows a score of moves. Five ways of breaking the dealer — a move
  allowed to follow itself, no crowding rule, a round forgetting how the
  last one closed, rests falling anywhere, a card always leading with its
  first move — each fail these. And the nap's schedule: a nap is over, with
  room for a move, before the next stretch of idleness begins — asleep at
  its two ends, up either side — and the one after the cat-naps has no end,
  not at the end of the clock; a nap is drawn from the session, the mascot
  and the moment it was left; a nap is three clips of a sleeper's pace, the
  same breath for all six — three stills of one body, no stage of it the
  rest art — the nod never ending on the sleeper's own still, and either
  start three fifths of a second.
- `ui/tests/header.rs` — the committed banner wears the rest pose whatever
  the clock says; the live banner draws the pose beside unmoved metadata
  (a blink: two cells); no pose moves the metadata or the banner's shape; a
  banner with nothing moving *is* the committed banner; a sweep lights the
  art under its band and nothing else, leans, and enters and leaves without
  a jump; a glow lifts every cell of the art alike; a glint lights a narrow
  band from its own side; glitter twinkles a scatter of the art's cells —
  never a blank one, never most of the mascot, a different scatter each
  step, an afterglow behind it, thinner at its ends; a mark beside the
  mascot — a snore, a start — is drawn in the light; a sleeper's lid is cut
  out of the ink — its cell drawn in reverse in the colour the body wears
  at that column, lit as the body is lit, and nothing else of the art
  reversed; the live banner looks toward the text being typed — ahead for
  a caret still under it, right from a line's third character, never left
  along a line or a wrapped one; a spark is drawn in the sweep's light; the narrow badge has no
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
  sits after the picture rows, found by what it does (a search for `nap`
  included), recorded per directory.
- `scripts/smoke.sh` Phase 159 — on the raw byte stream: the greeting's
  frames reach the terminal (the sweep's frames ahead of the flare, the
  first eyelid within a frame count no clock chain could keep) and leave one
  banner at rest over one composer; with the row off an idle session writes
  nothing; a first message sent while the eyes watch the draft leaves the
  rest art above the conversation and nothing is written afterwards; the
  eyes turn right toward a short line and a long one and never to the
  screen's left edge, go straight ahead when the caret is brought back
  under the mascot and right again when it leaves, look away
  about four seconds after the last keystroke, and at once when Ctrl+U
  empties the draft; `/clear`
  greets again (about thirty frames in the three seconds after it, the
  flared frill among them, where a mascot carrying on would write a clip's
  handful) and leaves one banner at rest; quitting mid-move leaves the rest
  art on the dead screen; the `/mascot` preview greets as a search lands
  the highlight on a mascot; and the mascot just chosen greets from the
  banner.
- `scripts/smoke.sh` Phase 160 — the nap, on the screen and the raw byte
  stream: left untouched the mascot is snoring no sooner than 44 s after
  the app opens and inside 80, the whole of it on screen — its frill on the
  title's row, its eyes closed to lids under it and never filled in, its
  feet on the third — over one banner and one composer, every lid written
  as it fell asleep in reverse video; asleep it writes a frame a stage of
  a breath (about six in seven seconds, where a clock chain would write
  two hundred), a snore among them and **nothing but the snore's one cell**
  in any of them — no cell of its body; a keystroke puts the `!` on the
  wire with
  no snore after it, the mascot back on its feet with its eyes on the
  draft; and the message it woke for commits the rest art above the
  conversation — no snore, no start, no shut or turned eyes left in
  scrollback — after which nothing is written.
