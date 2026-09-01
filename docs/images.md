# Inline images

A pasted screenshot and the `read` tool's image reads are drawn as **real
pictures** in the conversation — in the terminal's own scrollback, in the
live region, and in the Ctrl+O transcript — instead of only being described by
a fact line. Where the terminal speaks a graphics protocol (kitty, iTerm2,
sixel) those are real pixels; everywhere else they are unicode half-blocks,
which are ordinary coloured cells and therefore work in every terminal,
scrollback and resize included.

```
● Read(/home/me/shots/cat.png)
  ⎿  Read image (JPEG, 700x689, 63 KB)

<the picture, flush at the left margin>

● Downloaded ✅ — a kitten asleep on a MacBook charger.
```

One blank row above it, one below, and the block starts at **column 0** — it
is deliberately *not* indented into the cell's `⎿` gutter, because a picture
should get the whole width the terminal has.

## The shape of it

Five modules over two layers — pure and boundary — split the way the crate
always splits them (`src/images/`), plus `ui/image.rs` for the rows themselves:

| module | what it is |
| --- | --- |
| `images::geometry` | pure. The cell footprint a picture takes, and the per-cell carrier that marks the rows reserved for it. |
| `images::registry` | the process-global render policy (`/settings` plus what the terminal turned out to support) and the placement interner. |
| `images::fitted` | pure. A PNG decoded at the size it will be shown or sent — rows streamed through an area-average shrink, so the whole picture is never held (`docs/memory.md`). |
| `images::payload` | the other boundary: downscaling a picture before it is **uploaded**, and keeping the result on disk for the session so a re-sent attachment is never shrunk twice. |
| `images::store` | the paint boundary: the terminal capability, the encoded pictures, and the pass that turns a reserved block into one. |
| `ui::image` | pure. Which pictures a history item shows, and the marked rows they reserve under its cell. |

The split is what makes the feature cheap to wire in. `ui` **reserves rows**;
the boundary **draws into them**. A reserved block is `rows` ordinary
`Line`s of `cols` spaces, each cell carrying a marker in its
`underline_color` — so a picture travels through every path the crate already
has (a scrollback commit is a `Vec<Line>` and always has been), and the
boundary's `ImageStore::stamp` turns the markers into a picture in each of the
four places a paint goes through.

### The carrier

`links` already used `Style::underline_color` as a per-cell carrier — the one
channel that survives `Span` → `Cell` → every paint path. Images ride the same
24-bit space, split rather than shared: **bit 23 set** means an image marker
(`images::geometry::IMAGE_CARRIER_FLAG`), and `links::LINK_ID_MAX` was lowered
to `0x7F_FFFF` accordingly, so a URL can never decode as a picture or the
reverse. Below the flag a marker packs `(placement id, row index)` — 15 bits
and 8 — and the row index is what lets the stamp find a block's top-left
corner without scanning for it.

`term::draw_cells` strips a link carrier before the cells go out; the image
stamp clears its own markers for the same reason — a carrier must never paint
as a real underline colour.

### The placement interner

`images::place(path, px, avail_cols)` interns `(path, cols, rows)` to a small
id, because an id is all that fits in a cell. Interning on the **size** as
well as the path is what makes a resize correct: a narrower terminal produces a
different placement id, so the boundary encodes a fresh picture instead of
re-placing the old one at the wrong size.

### Where the pixel size comes from

The row reservation is pure, so it can't open the file. Two sources, and only
two:

* An **image read** — the `read` tool's own fact line already carries it
  (`Read image (JPEG, 700x689, 63 KB)`), so `images::read_image_size` parses
  it back out. That is also the only record that survives a `/resume`: the
  rollout keeps the cell's text, not the file's header.
* A **Ctrl+V paste** — no such line, so the boundary reads the header once
  when the paste lands (`images::remember_size`, from `tui::workers`) — and
  again for every pasted picture a loaded conversation carries
  (`Session::remember_loaded_image_sizes`, after a `/resume`, `--continue` or
  `--resume` load), since pastes are saved under the config home
  (`{config_home}/image-cache/{session}/N.png`, `docs/image-paste.md`) and
  are usually still there. A path with no entry — a picture someone deleted —
  simply isn't drawn.

## The geometry

`images::image_budget` then `images::fit_cells`, both pure:

```
usable    = terminal width − IMAGE_GUTTER_COLS (2)
max_cols  = min(/settings Image width, usable)
max_rows  = ceil(max_cols × cell_w / cell_h)        ← the square pixel box
(cols, rows) = Fit(image px) into (max_cols, max_rows)
```

Two things are worth spelling out.

**The row cap is the width cap as a square pixel box.** `max_cols` columns is
`max_cols × cell_w` pixels across; the same count of pixels *down* is
`max_rows` rows. Without it a 600×4000 portrait screenshot spends the whole
width budget on its width and then takes four hundred rows to match. With it,
a tall image is bounded by its height and comes out narrow — which is the
reference harness's fix for exactly this bug.

**`Image width` is a cap, not a target.** The fit is `ratatui_image`'s
`Resize::Fit`: proportional, and **shrink-only**. A 32×32 icon stays a
handful of cells rather than being blown up to fill 120 columns, where the
terminal's resampling would just make it blurry.

`fit_cells` deliberately reproduces `ratatui_image`'s own arithmetic rather
than inventing its own, because the reservation and the encoder must agree to
the cell — a row of disagreement is a blank gap under every picture.
`images::tests::fit_cells_reproduces_the_encoders_own_arithmetic` is a
differential test against the real `Resize::Fit` over a matrix of fonts,
image sizes and budgets.

## Detecting the terminal

`ImageStore::detect` runs once in `InlineViewport::init` and **never reads
stdin**.

`ratatui_image` ships a `Picker::from_query_stdio` that asks the terminal
directly, and it is the more accurate answer — but it cannot be used here. It
spawns a reader thread on stdin behind a two-second timeout and *never joins
it*, so on a terminal that does not answer, that thread outlives the query and
eats the user's keystrokes. Observed under tmux while building this: a
two-second startup stall, and then every key swallowed. That is CLAUDE.md
invariant 1 — one stdin reader — and it is not negotiable.

So, like the reference harness:

* **cell size** from the tty's own `ws_xpixel`/`ws_ypixel` (`TIOCGWINSZ` via
  `rustix`), which is an ioctl and not a round trip. Zeroes mean "unknown" and
  fall back to a 1:2 cell — only the *ratio* matters, since it is what decides
  a picture's row count;
* **protocol** from `ratatui_image`'s own env sniff (the iTerm2 family, plus a
  multiplexer's outer terminal) plus kitty/ghostty, which announce themselves
  in `TERM` / `TERM_PROGRAM` / `KITTY_WINDOW_ID` — and *not* under a
  multiplexer, where exactly that answer goes stale;
* anything undetected falls to **half-blocks**.

`ALTER_ZERO_IMAGE_PROTOCOL` (`kitty` / `iterm2` / `sixel` / `halfblocks`) and
`ALTER_ZERO_IMAGE_CELL_SIZE` (`9x18`) override both;
`ALTER_ZERO_IMAGES=0` turns the whole feature off.

## Painting

`ImageStore::stamp(buf)` runs in **four** places, and missing one leaves the
bug alive in that view alone (the `visible_cells` rule):

| path | what it draws |
| --- | --- |
| `write_above_chunk` | a scrollback commit — a picture is written into the terminal's real scrollback once, and scrolls with the text from then on |
| `paint_live` | the live region |
| `paint_reflow` | the live region of a purge rebuild (its tail goes through `write_above`) |
| `draw_overlay` | the Ctrl+O / Ctrl+D alternate screen — stamped *before* the diff, so an unchanged page stays silent (`docs/overlay-repaint.md`) |

`ratatui_image` renders by stashing the whole graphics escape into a `Cell`'s
symbol and marking the covered cells `CellDiffOption::Skip` (half-blocks are
ordinary cells and need none of that). `Buffer::diff` honours both flags
itself, so the diff paths came for free — but `term::visible_cells`, the
direct-emit path, had to learn them:

* a `Skip` cell is **never written**: the escape already painted those
  columns, and a space over them punches a hole in the picture;
* the shadow count comes from `Cell::cell_width()`, never
  `cell.symbol().cell_width()` — an image cell's symbol is hundreds of bytes
  of escape and exactly one column on screen, and only the `Cell` impl honours
  the `ForcedWidth` that says so.

`term::draw_cells` additionally ends its cell run **behind** such a blob. The
backend only re-addresses a cell that isn't adjacent to the one before it, and
an escape blob can leave the cursor anywhere — a sixel placement clears its
area row by row first — so the cell after it starts a fresh `draw`, which
always opens with a cursor move.

### Decoding at the fitted size

`ImageStore::encode` used to decode the file whole and hand the picture to
`ratatui_image` to shrink — `4 × width × height` bytes for a moment, 8 MB for
a 1080p screenshot and 33 MB for a 4K one, to produce a block of ~3 MB. And
not only for a moment: that buffer is exactly the size glibc's dynamic `mmap`
threshold learns to keep, so every picture after the first left it behind
(`docs/memory.md`, *Pasting a screenshot*).

A PNG — every paste, and most screenshots a `read` meets — now goes through
`images::fitted::decode_png_fitted` instead. The `png` crate hands out one row
at a time, and an area-averaging `Downsampler` folds each row into the
destination row it belongs to, so only the *fitted* picture is ever held: the
peak is the block plus one row, whatever the file holds. The target is
`fit_box` — `Resize::Fit`'s own arithmetic, pinned to it by a differential
test the way `fit_cells` is — so the encoder receives a picture that already
fits and builds the protocol from it as is. Anything else, and an interlaced
PNG (whose rows arrive out of order), is decoded **whole** — refused past
`WHOLE_DECODE_MAX_PIXELS` (50 megapixels, where the whole decode would be the
spike this exists to avoid) — and `thumbnail_exact`ed into the box, a box
filter with no `f32` working copy of the source, where the encoder's own
`resize` would have allocated one 16 bytes a pixel over the source width. The
streaming decode itself is bounded in *time* rather than memory
(`FIT_MAX_SOURCE_PIXELS`, 200 megapixels: it never holds the rows it walks).
The decoder is pure and reads from any seekable buffer, so `images::tests`
checks it against a naive area-average oracle, at the source size (an exact
copy), across every colour type the format has — a palette with `tRNS`
included — and requires it to decline an interlaced picture, a truncated one
and a header past the bound before reading a row.

### Drawing into exactly the reserved cells

`stamp` reads a block's extent **back off the carriers** — where its visible
rows are, how wide they are, and how many of its rows are above the frame —
rather than taking it from the placement. Two things follow, and both were
bugs in the first version.

A block is drawn into the reserved run and nothing more, so a picture the
frame cut short can never paint over what was drawn below it: the pager's own
`─── 100% ───` separator and its key-hint rows.

And a block whose **head row is above the frame still draws**. This is the
ordinary case, not an edge: Ctrl+O opens pinned to the *bottom*, so any
picture taller than the pager's body starts above the window and its first
visible row carries a non-zero row index. Rendering only from a head row drew
a screenful of reserved-but-empty rows on the most common open there is
(`smoke.sh` Phase 107c measures 0 rows before the fix, 10 after). The row
index is the slice offset, handed to `ratatui_image`'s
`SlicedImage`/`SlicedProtocol` as a negative `SignedPosition`.

Slicing is also what gives **sixel and iTerm2** clipping at all — both decline
to draw an image larger than the area they are given. `SlicedProtocol` strips
sixel bands at render time and cuts an iTerm2 image into one protocol per row;
kitty skips placeholder rows natively and half-blocks are ordinary cells.

### Screen switches

kitty — and Ghostty, which implements its protocol — allocates a **separate
image store per screen buffer**: `main_grman` and `alt_grman` in kitty's own
source, and the protocol spec spells out that *"when switching from the main
screen to the alternate screen buffer (1049 private mode) all images in the
alternate screen must be cleared"*. An image transmitted on one screen is
therefore not addressable from the other.

That was the first version's bug. The kitty protocol transmits an image — and
creates its `U=1` virtual placement — exactly *once* per encoded protocol
object, so a protocol carried across the hop painted the Ctrl+O transcript
with unicode placeholders naming an image the alternate screen's store had
never heard of. The lookup just returns and `q=2` suppresses replies, so it
failed **silently**: reserved rows with nothing in them.

So **an encoding belongs to a screen** — and, once made, it keeps. The cache
is keyed on `(placement, screen)` and `enter_overlay`/`exit_overlay` call
`ImageStore::enter_screen`, which now only follows the switch: each screen
uploads a given picture **once, ever**, and every later visit redraws
placeholders alone.

That the alternate screen's copy survives is not an assumption. The clear on
the 1049 switch spares exactly the placements this protocol uses — kitty's
filter opens `if (ref->is_virtual_ref) return false;` — and the image behind
it is not collected either, because that virtual ref *counts* as a ref
(`filter_refs` frees an image only when `!vt_size(&img->refs_by_internal_id)`).
The published spec says the same from the other side: a virtual placement is
never touched by the `a`/`c`/`p`/`q`/`x`/`y`/`z` deletion classes, which is
what both the switch and our `ESC [ 2 J` use. Verified in kitty from 0.28
(when placeholders shipped) through current, and in Ghostty from 1.1.

It matters because a picture is not cheap on the wire. `ratatui_image`
transmits kitty images as raw **RGBA**, so a 120×35-cell picture is ~3.4 MB of
pixels and **~4.5 MB of base64**. Measured across one Ctrl+O toggle:

| | first version | per-screen cache | + retention |
| --- | --- | --- | --- |
| open | 4.53 MB | 4.53 MB | 4.53 MB |
| close | 4.53 MB | 1.3 KB | 1.3 KB |
| reopen | 4.53 MB | 4.53 MB | **19.6 KB** |

The one upload that remains is the honest one: the alternate screen's store
genuinely does not have the picture the first time you open it.

`ALTER_ZERO_IMAGE_RETRANSMIT=1` takes the conservative path — re-upload on
every switch — for a terminal that speaks the protocol but not that part of
it. Without it such a terminal would show the picture on the first Ctrl+O and
blank rows on the second. We cannot ask it which kind it is: the reply would
have to be read off stdin, and this crate has exactly one stdin reader
(invariant 1).

Only kitty pays any of this. Sixel, iTerm2 and half-blocks keep nothing per
screen (they carry their whole payload in every render), so they share the
primary's entry and are encoded exactly once — re-encoding them on a screen
switch would be waste on a view that is meant to open instantly
(`docs/tool-view-performance.md`).

The abandoned image ids self-clean: kitty's quota is per buffer and *"existing
images without placements will be preferentially deleted"* under pressure, and
the next entry to the alternate screen clears its store outright.

`smoke.sh` Phase 107b is the guard — a transmit on the way **in**, none on the
way back out, and none on a reopen. It reads the raw byte stream through
`pipe-pane` rather than the pane text, because the pane text is exactly what
cannot tell those cases apart: a kitty placeholder *is* an ordinary cell, so a
capture looks identical whether or not the picture will appear, and identical
again whether or not megabytes went with it.

### Resize

Every resize purge-rebuilds the conversation from history (invariant 3), which
re-runs `images::place` at the new width and so re-encodes every picture to
fit. The purge also drops the encoded pictures
(`ImageStore::invalidate`, from `clear_scrollback_and_screen`): a kitty
placement transmits its pixels once per encoded protocol, so one that outlived
the `ESC[3J` would place an image the terminal may already have dropped.

### Memory

A kitty placement holds the whole picture as base64 RGBA — a 120-column
screenshot at a 10×20 cell is 1200×600 pixels, ~2.9 MB of pixels and ~3.8 MB
of base64 — and this process idles in the user's terminal all day
(`docs/memory.md`). So the store is bounded by **bytes**, estimated from the
placement's own geometry rather than by counting entries: past
`CACHE_MAX_BYTES` (24 MB) the least-recently-drawn picture is dropped. Meeting
it again costs one re-encode, never a wrong picture. And the decode that
fills an entry is bounded by the block, not by the file (*Decoding at the
fitted size*): drawing a 4K screenshot costs what drawing a 1080p one does.

## The `/settings` rows

| row | default | what it does |
| --- | --- | --- |
| **Show images** | `true` | draw pictures inline at all. Unavailable — `false (unavailable)` — when the terminal can't draw one. |
| **Image width** | `120` | the width **cap** in columns; cycles 60 / 80 / 120. |
| **Auto-resize images** | `true` | downscale a large picture before it is **sent to the model**. Nothing to do with the display, so it stays available either way. |

Cycling either display row republishes the policy, drops the encoded pictures
and purge-rebuilds the conversation, so committed pictures change size (or
disappear) at once — the `/mascot` switch's rule. They persist in
`settings.json` as a diff from the defaults like every other knob
(`docs/settings.md`).

**Auto-resize images** is the payload row, and it is deliberately a different
kind of thing: a 12-megapixel phone photo is megabytes of base64 that a
provider either refuses outright or bills in full, and a model reads it no
better than the same picture at 2000 pixels. So the two paths that upload
pixels — the `read` tool's image branch and a Ctrl+V attachment — run their
bytes through `images::payload` first. A JPEG stays a JPEG (a photo re-encoded
as PNG *grows*); everything else becomes PNG. A PNG is shrunk by the same
streaming decoder the display uses, fitted straight to the 2000-pixel
target, so shrinking a 4K screenshot costs the target's ~9 MB rather than the
picture's 33; the other formats decode whole (refused past
`WHOLE_DECODE_MAX_PIXELS`) and `thumbnail_exact` into it, with no `f32` pass.

And it is done **once**. An attachment is re-sent with the context on every
later turn, and each of those turns used to decode and shrink the original
all over again — on a 4K screenshot, a 20 MB spike per turn for as long as
the picture stayed in context. The downscaled bytes are now **kept on disk
for the session** (`images::payload`'s sidecar under
`{tmp}/alter-zero-{uid}/{session}/images/`, `docs/scratchpad.md`), keyed on
the file's path, size, mtime and the cap, and a later turn serves the request
from that small file — `cached_downscale`, consulted before the original is
even opened. The key is what makes a stale payload impossible: a changed file
is a different entry. With the row off nothing is cached or served, exactly
as before; `downscale_to`, the uncached core, is what the unit tests drive,
and the cache has tests of its own (written once, read back without a decode,
nothing written without a cache dir, nothing served with the row off). The
`read` tool's images go through the same cache, so a picture the model reads
twice is shrunk once. The **file** is untouched, which
is why the picture on screen is unaffected, and why an auto-resized read's
fact line leads with the file's own dimensions and names the sent ones after:

```
Read image (PNG, 4000x3000, 4.6 MB; sent resized to 2000x1500, 1.1 MB)
```

That order is load-bearing twice: the model needs the original dimensions to
map a coordinate it reads off the picture back to the file, and
`images::read_image_size` takes the **first** `WxH` as the size of the picture
it is about to draw from disk — which is the original.

With the row off, an oversized image is refused as before (the error now
points at the setting). The downscale also declines outright past 50
megapixels or 64 MB of source bytes: decoding is `4 × width × height` bytes
resident, and shrinking a 12000×12000 scan would cost 576 MB to discover it
was a bad idea.

## Known limits

* Under **tmux/screen** the protocol guess is deliberately conservative:
  half-blocks unless the outer terminal is a known iTerm2-family one.
  `ALTER_ZERO_IMAGE_PROTOCOL` overrides it for a passthrough-enabled setup.
* The live **streaming preview** doesn't draw a picture: a `read` has no image
  until it resolves, and the committed cell is one frame away.

## Driving it by hand

`cargo run --example make_test_image -- shot.png 640 400` writes a gradient
with a white diagonal — a wrong aspect ratio or a clipped row is obvious at a
glance. Then ask a vision model to `read` it, or force a protocol; for the
paste path, `cargo run --example clipboard_owner -- 1920 1080` serves a
screenshot-shaped PNG on the X11 clipboard for Ctrl+V to pick up
(`docs/image-paste.md`, *Measuring*):

```
ALTER_ZERO_IMAGE_PROTOCOL=halfblocks ALTER_ZERO_IMAGE_CELL_SIZE=5x10 cargo run
```

`smoke.sh` Phase 107 does exactly that against a handcrafted rollout carrying
an image `read` of a real PNG: it asserts the picture's row count, that it
starts at column 0, the blank row under the cell, that Ctrl+O draws the same
picture, and that `/settings` **Show images** off purge-rebuilds without it.
Phase 107b reads the raw byte stream and asserts a kitty transmit lands going
**into** the alternate screen and none coming back out (see *Screen
switches*).
Phase 107c drives a picture taller than the pager and asserts the
bottom-pinned open still draws its visible part.
