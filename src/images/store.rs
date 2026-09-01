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
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};

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
struct Encoded {
    /// `None` once the file has failed to load — remembered so a broken path
    /// isn't re-read on every frame of every turn.
    protocol: Option<Protocol>,
    bytes: usize,
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

    /// Draw every reserved block in `buf`.
    ///
    /// The single pass every paint path runs before its cells reach the
    /// terminal (`term`'s `visible_cells` rule: a path that skips it shows
    /// blank rows in that view alone). Reserved cells carry
    /// [`super::geometry::carrier`]; a block's **head row** (row 0) is where
    /// the picture is rendered from, and every carrier is cleared afterwards
    /// so it can never paint as a real underline colour.
    ///
    /// A block whose head row is scrolled off the top of `buf` — a picture
    /// the Ctrl+O transcript is showing the bottom half of — is left blank:
    /// no protocol here can start a picture partway down.
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
        let area = buf.area;
        let mut heads: Vec<(u16, u16, u32)> = Vec::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let Some(cell) = buf.cell_mut((x, y)) else {
                    continue;
                };
                let Some((id, row)) = carrier_parts(cell.underline_color) else {
                    continue;
                };
                cell.underline_color = Color::Reset;
                if row == 0
                    && heads
                        .last()
                        .is_none_or(|&(_, hy, hid)| hy != y || hid != id)
                {
                    heads.push((x, y, id));
                }
            }
        }
        for (x, y, id) in heads {
            self.draw(buf, x, y, id);
        }
    }

    /// Render placement `id` at `(x, y)` in `buf`, encoding it on first use.
    fn draw(&mut self, buf: &mut Buffer, x: u16, y: u16, id: u32) {
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
        let area = buf.area;
        let width = place.cols.min(area.right().saturating_sub(x));
        let height = place.rows.min(area.bottom().saturating_sub(y));
        if width == 0 || height == 0 {
            return;
        }
        // `allow_clipping` so a block the region cut short still draws the
        // part that fits (kitty and half-blocks honour it; the protocols that
        // cannot clip decline, which is the old behaviour).
        Image::new(protocol)
            .allow_clipping(true)
            .render(Rect::new(x, y, width, height), buf);
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
            .and_then(|image| picker.new_protocol(image, size, Resize::Fit(None)).ok());
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
