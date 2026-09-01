//! The paint boundary: the terminal's graphics capability, the encoded
//! pictures, and the pass that turns a reserved block into one.
//!
//! Everything that *acts* here is I/O — a `Picker` built from the environment
//! and an ioctl, a file decoded off disk, an escape sequence stamped into a
//! `Buffer` cell — so it is exercised by `scripts/smoke.sh` (Phase 107), not
//! by unit tests; the detection's own predicates are pure and are. The pure
//! half it serves (which cells are reserved, and how many) lives beside it in
//! [`super::geometry`]. See `docs/images.md`.

use std::collections::HashMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;
use ratatui_image::Resize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::sliced::{SignedPosition, SlicedImage, SlicedProtocol};

use super::geometry::{FontSize, carrier_parts};
use super::registry::{Placement, placement};

/// Environment gate: a falsy value turns inline pictures off entirely.
/// Default on.
pub const IMAGES_ENV: &str = "ALTER_ZERO_IMAGES";

/// Force the graphics protocol instead of guessing it from the environment:
/// `kitty`, `iterm2`, `sixel`, or `halfblocks`. For the terminal whose env
/// vars say nothing — and for saying "just use half-blocks".
pub const IMAGE_PROTOCOL_ENV: &str = "ALTER_ZERO_IMAGE_PROTOCOL";

/// Force the terminal's cell size in pixels, `WxH` (e.g. `9x18`), for a
/// terminal that reports none. Only the *ratio* matters: it decides how many
/// rows tall a picture of a given width comes out.
pub const IMAGE_CELL_SIZE_ENV: &str = "ALTER_ZERO_IMAGE_CELL_SIZE";

/// Whether inline images should be **off**, given [`IMAGES_ENV`]'s value —
/// [`crate::links::hyperlinks_disabled`]'s twin, same falsy spellings.
#[must_use]
pub fn images_disabled(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "false" | "no" | "off")
    )
}

/// The protocol [`IMAGE_PROTOCOL_ENV`] names, if it names one this build can
/// speak. Pure, so the spellings are unit-tested while the env read stays at
/// the boundary.
#[must_use]
pub fn protocol_from_name(value: &str) -> Option<ProtocolType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "kitty" => Some(ProtocolType::Kitty),
        "iterm2" | "iterm" => Some(ProtocolType::Iterm2),
        "sixel" => Some(ProtocolType::Sixel),
        "halfblocks" | "half-blocks" | "blocks" => Some(ProtocolType::Halfblocks),
        _ => None,
    }
}

/// A `WxH` cell size, as [`IMAGE_CELL_SIZE_ENV`] spells it. Pure.
#[must_use]
pub fn parse_cell_size(value: &str) -> Option<FontSize> {
    let (w, h) = value.trim().split_once(['x', 'X'])?;
    let w: u16 = w.trim().parse().ok()?;
    let h: u16 = h.trim().parse().ok()?;
    (w > 0 && h > 0).then_some((w, h))
}

/// Whether this session runs under a terminal multiplexer — where a protocol
/// guessed from the environment is least trustworthy, because the inner
/// `TERM` says `tmux`/`screen` while `KITTY_WINDOW_ID` may still name an
/// outer terminal that is no longer attached. Pure over the two values the
/// boundary reads.
#[must_use]
pub fn under_multiplexer(term: Option<&str>, tmux: Option<&str>) -> bool {
    tmux.is_some_and(|v| !v.is_empty())
        || term.is_some_and(|t| t.starts_with("tmux") || t.starts_with("screen"))
}

/// Whether the environment names a kitty-protocol terminal (kitty itself, or
/// ghostty). `false` for everything else, which leaves `ratatui_image`'s own
/// env sniff — the iTerm2 family, plus a multiplexer's outer terminal — to
/// decide. Pure.
#[must_use]
pub fn kitty_from_env(term: Option<&str>, term_program: Option<&str>, kitty_id: bool) -> bool {
    if kitty_id {
        return true;
    }
    let names = |value: Option<&str>| {
        value.is_some_and(|v| {
            let v = v.to_ascii_lowercase();
            v.contains("kitty") || v.contains("ghostty")
        })
    };
    names(term) || names(term_program)
}

/// How much encoded picture the store keeps before it starts evicting.
///
/// A kitty placement holds the whole image as base64 RGBA — a 120-column
/// screenshot at a 10×20 cell is 1200×600 pixels, ~2.9 MB of pixels and
/// ~3.8 MB of base64 — and this process idles in the user's terminal all day
/// (`docs/memory.md`), so the cache is bounded by *bytes*, estimated from the
/// placement's own geometry rather than by counting entries. Past the budget
/// the least-recently-drawn picture is dropped; meeting it again costs one
/// re-encode, never a wrong picture.
const CACHE_MAX_BYTES: usize = 24 * 1024 * 1024;

/// One encoded picture plus what it costs.
///
/// A [`SlicedProtocol`] rather than a plain `Protocol`, because a block can be
/// **cut by the frame it lands in**: the Ctrl+O transcript opens pinned to the
/// bottom, so a picture taller than the pager's body starts *above* the
/// window, and a plain protocol can only draw from its own first row. The
/// sliced form takes a signed position and clips at either end — natively for
/// kitty (a placeholder skip), sixel (band stripping) and half-blocks, and by
/// slicing the picture into one protocol per row for iTerm2, which can do
/// neither.
struct Encoded {
    /// `None` once the file has failed to load — remembered so a broken path
    /// isn't re-read on every frame of every turn.
    protocol: Option<SlicedProtocol>,
    bytes: usize,
}

/// One reserved block as the frame actually holds it: where its visible rows
/// are, how far its **head** row sits above them, and how wide it is.
///
/// Collected from the carriers rather than taken from the placement, so the
/// picture is drawn into exactly the cells that were reserved — never over the
/// pager's separator and key hints below them, and never guessing at rows the
/// line builder trimmed.
struct VisibleBlock {
    /// Leftmost reserved column of the block's first visible row.
    x: u16,
    /// Reserved columns on that row.
    cols: u16,
    /// The block's first visible row.
    first_y: u16,
    /// Its last visible row.
    last_y: u16,
    /// How many of the block's rows are above the frame — `0` when its head
    /// row is visible, which is the ordinary case.
    skipped: u16,
}

/// The terminal's graphics capability and the pictures encoded for it.
pub struct ImageStore {
    picker: Option<Picker>,
    encoded: HashMap<u32, Encoded>,
    /// Placement ids in least-recently-drawn order (the tail is the newest).
    /// A handful of entries at most, so a `Vec` beats a real LRU map.
    order: Vec<u32>,
    bytes: usize,
}

impl ImageStore {
    /// A store that can never draw: no query is sent and every reserved block
    /// stays blank. The gate's `off` state, and what a host without a
    /// terminal gets.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            picker: None,
            encoded: HashMap::new(),
            order: Vec::new(),
            bytes: 0,
        }
    }

    /// Work out what this terminal can draw and how big a cell is —
    /// **without reading stdin**.
    ///
    /// `ratatui_image` ships a `Picker::from_query_stdio` that asks the
    /// terminal directly, and it is the more accurate answer, but it cannot
    /// be used here: it spawns a reader thread on stdin behind a two-second
    /// timeout and **never joins it**, so on any terminal that does not
    /// answer, that thread outlives the query and eats the user's first
    /// keystrokes (observed under tmux: every key swallowed, and a two-second
    /// startup stall to boot). That is invariant 1 — one stdin reader — and it
    /// is not negotiable, so the detection is environment plus `TIOCGWINSZ`,
    /// which is what the reference harness does too:
    ///
    /// - the **cell size** comes from the tty's own `ws_xpixel`/`ws_ypixel`
    ///   (an ioctl, never a round trip), falling back to a 1:2 cell — only
    ///   the *ratio* matters, since it is what decides a picture's row count;
    /// - the **protocol** is `ratatui_image`'s own env sniff (the iTerm2
    ///   family, and a multiplexer's outer terminal) plus kitty/ghostty,
    ///   which announce themselves in `TERM`/`TERM_PROGRAM`/
    ///   `KITTY_WINDOW_ID` — and *not* under a multiplexer, where exactly
    ///   that answer goes stale;
    /// - anything undetected falls to unicode half-blocks, which are ordinary
    ///   coloured cells and so work in every terminal, scrollback, resize and
    ///   copy included.
    ///
    /// [`IMAGE_PROTOCOL_ENV`] and [`IMAGE_CELL_SIZE_ENV`] override both.
    #[must_use]
    pub fn detect() -> Self {
        let env = |name: &str| std::env::var(name).ok();
        let font = env(IMAGE_CELL_SIZE_ENV)
            .as_deref()
            .and_then(parse_cell_size)
            .or_else(cell_size_from_tty)
            .unwrap_or(super::geometry::DEFAULT_FONT_SIZE);
        // `from_fontsize` is deprecated in favour of the stdin query above,
        // which this crate cannot call. It is also the only constructor that
        // takes a cell size, and it still runs the env-based tmux + iTerm2
        // detection we want.
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(font.0, font.1));
        let multiplexed = under_multiplexer(env("TERM").as_deref(), env("TMUX").as_deref());
        if !multiplexed
            && kitty_from_env(
                env("TERM").as_deref(),
                env("TERM_PROGRAM").as_deref(),
                env("KITTY_WINDOW_ID").is_some(),
            )
        {
            picker.set_protocol_type(ProtocolType::Kitty);
        }
        if let Some(forced) = env(IMAGE_PROTOCOL_ENV)
            .as_deref()
            .and_then(protocol_from_name)
        {
            picker.set_protocol_type(forced);
        }
        Self {
            picker: Some(picker),
            encoded: HashMap::new(),
            order: Vec::new(),
            bytes: 0,
        }
    }

    /// Whether this store can draw at all — [`ImagePolicy::available`].
    ///
    /// [`ImagePolicy::available`]: super::registry::ImagePolicy::available
    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.picker.is_some()
    }

    /// The terminal's cell size in pixels, or `None` when nothing was
    /// detected. The pure row math needs this and nothing else.
    #[must_use]
    pub fn font_size(&self) -> Option<FontSize> {
        let font = self.picker.as_ref()?.font_size();
        Some((font.width, font.height))
    }

    /// The protocol the pictures will be drawn with — for the `/settings`
    /// row's explanation and the smoke suite.
    #[must_use]
    pub fn protocol_name(&self) -> Option<&'static str> {
        Some(match self.picker.as_ref()?.protocol_type() {
            ProtocolType::Halfblocks => "halfblocks",
            ProtocolType::Sixel => "sixel",
            ProtocolType::Kitty => "kitty",
            ProtocolType::Iterm2 => "iterm2",
        })
    }

    /// Drop every encoded picture.
    ///
    /// Called whenever the terminal's scrollback is purged: a kitty image is
    /// transmitted exactly once per protocol object, so a protocol that
    /// outlived the purge would place a picture the terminal may already have
    /// dropped. Re-encoding is the price of a rebuild that is actually
    /// correct.
    pub fn invalidate(&mut self) {
        self.encoded.clear();
        self.order.clear();
        self.bytes = 0;
    }

    /// Drop the encodings a **screen switch** invalidates — and only those.
    ///
    /// kitty (and Ghostty, which implements its protocol) allocates a
    /// *separate image store per screen buffer* — `main_grman` and
    /// `alt_grman` in kitty's own source, and the protocol spec spells out
    /// that "when switching from the main screen to the alternate screen
    /// buffer (1049 private mode) all images in the alternate screen must be
    /// cleared". An image transmitted on one screen is therefore not
    /// addressable from the other, and a unicode placeholder that names it
    /// resolves to nothing — **silently**, since the lookup just returns and
    /// `q=2` suppresses replies. That is reserved rows with no picture in
    /// them, which is exactly what the Ctrl+O transcript showed. There is no
    /// public way to re-arm `ratatui_image`'s transmit flag, so the encoding
    /// is dropped and the next frame rebuilds it (with a fresh random image
    /// id, so the two screens' stores can't collide).
    ///
    /// The other three protocols are placement-free by construction — sixel,
    /// iTerm2 and half-blocks carry their whole payload in every render — so
    /// they survive the hop, and re-encoding them would be pure waste on a
    /// Ctrl+O that is meant to open instantly
    /// (`docs/tool-view-performance.md`).
    pub fn invalidate_across_screens(&mut self) {
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.protocol_type() == ProtocolType::Kitty)
        {
            self.invalidate();
        }
    }

    /// Draw every reserved block in `buf`.
    ///
    /// The single pass every paint path runs before its cells reach the
    /// terminal (`term`'s `visible_cells` rule: a path that skips it shows
    /// blank rows in that view alone). Reserved cells carry
    /// [`super::geometry::carrier`] — a placement id and a row index — and
    /// every carrier is cleared afterwards so it can never paint as a real
    /// underline colour.
    ///
    /// The picture is drawn into exactly the reserved cells this frame holds,
    /// which is what makes a **cut** block work: the Ctrl+O transcript opens
    /// pinned to the bottom, so a picture taller than the pager's body starts
    /// above the window, and its first visible row carries a non-zero row
    /// index. That index is the slice offset ([`SignedPosition`]), so the
    /// visible part draws instead of nothing at all.
    pub fn stamp(&mut self, buf: &mut Buffer) {
        // The overwhelmingly common session shows no picture at all, and this
        // runs on every frame of every paint path — so the walk below is
        // behind a single relaxed load.
        if !super::any_placements() {
            return;
        }
        if self.picker.is_none() {
            clear_carriers(buf);
            return;
        }
        for (id, block) in visible_blocks(buf) {
            self.draw(buf, id, &block);
        }
    }

    /// Render placement `id` into the cells `block` names, encoding it on
    /// first use.
    fn draw(&mut self, buf: &mut Buffer, id: u32, block: &VisibleBlock) {
        let Some(place) = placement(id) else {
            return;
        };
        self.encode(&place);
        // Touch: the entry just drawn is the newest.
        if let Some(at) = self.order.iter().position(|&other| other == id) {
            let id = self.order.remove(at);
            self.order.push(id);
        }
        let Some(Encoded {
            protocol: Some(protocol),
            ..
        }) = self.encoded.get(&id)
        else {
            return;
        };
        let height = block.last_y.saturating_sub(block.first_y).saturating_add(1);
        if block.cols == 0 || height == 0 {
            return;
        }
        // The area is the reserved run and nothing more, so a picture the
        // frame cut short can never paint over what was drawn below it — the
        // pager's own separator and key-hint rows. The position is relative to
        // that area, and negative by however many of the block's rows are
        // above it.
        let area = Rect::new(block.x, block.first_y, block.cols, height);
        let position = SignedPosition {
            x: 0,
            y: -i16::try_from(block.skipped).unwrap_or(i16::MAX),
        };
        SlicedImage::new(protocol, position).render(area, buf);
    }

    /// Encode `place` if it isn't cached — decode the file, fit it into the
    /// reserved cells, and charge the result against the byte budget.
    fn encode(&mut self, place: &Placement) {
        if self.encoded.contains_key(&place.id) {
            return;
        }
        let Some(picker) = self.picker.as_ref() else {
            return;
        };
        let size = ratatui::layout::Size::new(place.cols, place.rows);
        let protocol = image::ImageReader::open(&place.path)
            .ok()
            .and_then(|reader| reader.with_guessed_format().ok())
            .and_then(|reader| reader.decode().ok())
            .and_then(|image| {
                SlicedProtocol::new_with_resize(picker, image, size, Resize::Fit(None)).ok()
            });
        let font = picker.font_size();
        let bytes = protocol.as_ref().map_or(0, |_| {
            // The pixels the encoder had to carry, plus base64's third.
            usize::from(place.cols)
                * usize::from(place.rows)
                * usize::from(font.width)
                * usize::from(font.height)
                * 4
                * 4
                / 3
        });
        self.encoded.insert(place.id, Encoded { protocol, bytes });
        self.order.push(place.id);
        self.bytes = self.bytes.saturating_add(bytes);
        self.evict(place.id);
    }

    /// Drop least-recently-drawn pictures until the store is back inside
    /// [`CACHE_MAX_BYTES`]. The entry just encoded is never the one dropped —
    /// it is about to be drawn.
    fn evict(&mut self, keep: u32) {
        while self.bytes > CACHE_MAX_BYTES && self.order.len() > 1 {
            let Some(at) = self.order.iter().position(|&id| id != keep) else {
                break;
            };
            let id = self.order.remove(at);
            if let Some(entry) = self.encoded.remove(&id) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
            }
        }
    }
}

/// The reserved blocks `buf` holds, keyed by placement id, with every carrier
/// **cleared** on the way through (it must never reach the terminal as a real
/// underline colour — the [`crate::links`] rule).
///
/// A block is identified by its id *and* its head row's position, which the
/// carrier's row index gives even when that row is above the frame: two
/// pictures of the same file at the same size share an id, and this is what
/// keeps them apart.
fn visible_blocks(buf: &mut Buffer) -> Vec<(u32, VisibleBlock)> {
    let area = buf.area;
    // A frame holds a handful of blocks at most, so a `Vec` scan beats a map —
    // and it keeps them in the order they were met, which is deterministic.
    let mut blocks: Vec<(u32, i32, VisibleBlock)> = Vec::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let Some(cell) = buf.cell_mut((x, y)) else {
                continue;
            };
            let Some((id, row)) = carrier_parts(cell.underline_color) else {
                continue;
            };
            cell.underline_color = Color::Reset;
            let head = i32::from(y) - i32::from(row);
            let Some((_, _, block)) = blocks
                .iter_mut()
                .find(|(other, other_head, _)| *other == id && *other_head == head)
            else {
                // First cell of this block in the frame. The scan runs
                // top-down and left-to-right, so this row is its topmost
                // visible one and this column its leftmost.
                blocks.push((
                    id,
                    head,
                    VisibleBlock {
                        x,
                        cols: 1,
                        first_y: y,
                        last_y: y,
                        skipped: row,
                    },
                ));
                continue;
            };
            if y == block.first_y {
                block.cols = block.cols.saturating_add(1);
            }
            block.last_y = block.last_y.max(y);
        }
    }
    blocks
        .into_iter()
        .map(|(id, _, block)| (id, block))
        .collect()
}

/// Strip every image carrier from `buf` without drawing anything — what a
/// store with no picker does, so a reserved cell is a plain space rather than
/// a cell asking the terminal for an underline colour.
fn clear_carriers(buf: &mut Buffer) {
    let area = buf.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y))
                && carrier_parts(cell.underline_color).is_some()
            {
                cell.underline_color = Color::Reset;
            }
        }
    }
}

/// The terminal's cell size from the tty itself: the window's pixel size
/// divided by its cell grid. Zeroes — what a terminal that doesn't report
/// pixels sends — mean "unknown", not a zero-sized cell.
fn cell_size_from_tty() -> Option<FontSize> {
    let size = rustix::termios::tcgetwinsize(std::io::stdout()).ok()?;
    let (px_w, px_h) = (size.ws_xpixel, size.ws_ypixel);
    let (cols, rows) = (size.ws_col, size.ws_row);
    (px_w > 0 && px_h > 0 && cols > 0 && rows > 0).then(|| (px_w / cols, px_h / rows))
}
