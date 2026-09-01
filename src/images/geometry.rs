//! Where a picture lands on the character grid.
//!
//! Two pure questions, and nothing else: **how many cells** an image of a
//! given pixel size occupies at the terminal's font size under the
//! `/settings` **Image width** cap, and **which cells** are the ones reserved
//! for it (the per-cell carrier the paint boundary reads back).
//!
//! The cell math deliberately reproduces `ratatui_image`'s own
//! [`Resize::Fit`] arithmetic — proportional shrink into the budget, then
//! `ceil` to whole cells — because the reservation and the encoder must agree
//! to the cell: the rows are reserved here (in pure `ui`, from the image's
//! recorded pixel size) and the protocol is encoded there (at the boundary,
//! from the file). A formula of our own would drift by a row and leave a gap
//! under every picture. `images::tests` pins the two together against the
//! real encoder. See `docs/images.md`.
//!
//! [`Resize::Fit`]: ratatui_image::Resize::Fit

use ratatui::style::Color;

/// A terminal cell's size in pixels — `(width, height)`, as the terminal
/// reported it (or [`DEFAULT_FONT_SIZE`] when it never did).
pub type FontSize = (u16, u16);

/// The cell size assumed when the terminal answers no size query — the same
/// arbitrary 1:2 cell `ratatui_image`'s own picker falls back to. Wrong in
/// pixels, right in proportion, which is all the row count needs.
pub const DEFAULT_FONT_SIZE: FontSize = (10, 20);

/// The widths — in **columns** — the `/settings` **Image width** row cycles.
/// Listed ascending so the menu reads sensibly; the default sits at the end,
/// so cycling from it wraps to the narrowest first.
pub const IMAGE_WIDTH_CHOICES: &[u16] = &[60, 80, 120];

/// The default **Image width**: the widest choice, so a picture opens as
/// large as the terminal allows and narrowing is the deliberate act.
pub const DEFAULT_IMAGE_WIDTH: u16 = 120;

/// Columns held back on the right of an image, so a picture never runs flush
/// into the terminal's edge (and a sixel/kitty placement never has to decide
/// what the last column means). The reference harness's `width - 2`.
pub const IMAGE_GUTTER_COLS: u16 = 2;

/// The tallest block a single image may reserve. Two ceilings meet here: the
/// carrier's row field is 8 bits, and the kitty protocol's unicode
/// placeholders only reach ~297 rows. Neither is a limit a real picture hits,
/// because [`image_budget`] caps the height at the width's own square box
/// long before this.
pub const IMAGE_MAX_ROWS: u16 = 0xFF;

/// The longest edge, in pixels, an image is downscaled to before it is sent
/// to the model when `/settings` **Auto-resize images** is on. The reference
/// harness's 2000: comfortably inside every provider's per-image ceiling
/// while still legible for a screenshot of code.
pub const AUTO_RESIZE_MAX_PIXELS: u32 = 2000;

/// The cell box an image may occupy: the **Image width** cap clamped to what
/// the terminal actually has, and the row cap that follows from it.
///
/// The row cap is the width cap expressed as a *square* pixel box —
/// `max_cols` columns is `max_cols × font.0` pixels across, and the same
/// count of pixels down is `⌈max_cols × font.0 / font.1⌉` rows. Without it a
/// portrait screenshot (600×4000) would spend the whole width budget on its
/// width and then take four hundred rows to match; with it, a tall image is
/// bounded by its height instead and comes out narrow.
///
/// `None` when the terminal is too narrow to hold a picture at all.
#[must_use]
pub fn image_budget(max_cols: u16, avail_cols: u16, font: FontSize) -> Option<(u16, u16)> {
    let usable = avail_cols.saturating_sub(IMAGE_GUTTER_COLS);
    if usable == 0 {
        return None;
    }
    let cols = max_cols.max(1).min(usable);
    let rows = (u32::from(cols) * u32::from(font.0.max(1)))
        .div_ceil(u32::from(font.1.max(1)))
        .clamp(1, u32::from(IMAGE_MAX_ROWS)) as u16;
    Some((cols, rows))
}

/// The cell footprint `px` takes inside `budget` at `font` — `ratatui_image`'s
/// `Resize::Fit` in cells: shrink proportionally until both edges fit (never
/// *grow* — a 32×32 icon stays a handful of cells), then round each edge up
/// to a whole cell.
#[must_use]
pub fn fit_cells(px: (u32, u32), font: FontSize, budget: (u16, u16)) -> (u16, u16) {
    let (pw, ph) = (px.0.max(1), px.1.max(1));
    let (fw, fh) = (u32::from(font.0.max(1)), u32::from(font.1.max(1)));
    // `Fit` clamps the target to the image itself first, which is what makes
    // it shrink-only: a picture smaller than the budget scales by exactly 1.
    let target_w = u32::from(budget.0).saturating_mul(fw).min(pw);
    let target_h = u32::from(budget.1).saturating_mul(fh).min(ph);
    let ratio = (f64::from(target_w) / f64::from(pw)).min(f64::from(target_h) / f64::from(ph));
    let nw = (f64::from(pw) * ratio).round().max(1.0);
    let nh = (f64::from(ph) * ratio).round().max(1.0);
    let cols = (nw / f64::from(fw)).ceil().min(f64::from(budget.0)) as u16;
    let rows = (nh / f64::from(fh)).ceil().min(f64::from(budget.1)) as u16;
    (cols.max(1), rows.max(1))
}

/// The whole geometry in one call: the cell footprint an image of `px` pixels
/// takes in a `avail_cols`-wide region under the `max_cols` cap. `None` when
/// there is no room at all.
#[must_use]
pub fn image_cells(
    px: (u32, u32),
    font: FontSize,
    max_cols: u16,
    avail_cols: u16,
) -> Option<(u16, u16)> {
    Some(fit_cells(
        px,
        font,
        image_budget(max_cols, avail_cols, font)?,
    ))
}

// --- The per-cell carrier ---

/// The bit that separates an **image** carrier from a [`crate::links`] one.
/// Both ride the same 24-bit `underline_color` channel — the one per-cell
/// field that survives `Span` → `Cell` → every paint path — so the space is
/// split rather than shared: link ids stay below it
/// (`links::LINK_ID_MAX`), image markers set it.
pub const IMAGE_CARRIER_FLAG: u32 = 0x80_0000;

/// The largest image id the carrier can hold (15 bits, 1-based).
pub const IMAGE_ID_MAX: u32 = 0x7FFF;

/// The underline colour that marks a cell as row `row` of the reserved block
/// for placement `id` — [`carrier_parts`]' inverse.
///
/// The paint boundary decodes it, renders the picture over the block, and
/// clears the carrier, so it can never reach the terminal as a real underline
/// colour (the [`crate::links`] rule).
#[must_use]
pub fn carrier(id: u32, row: u16) -> Color {
    let packed = IMAGE_CARRIER_FLAG | (id.min(IMAGE_ID_MAX) << 8) | u32::from(row.min(0xFF));
    Color::Rgb(
        (packed >> 16) as u8,
        (packed >> 8) as u8,
        u8::try_from(packed & 0xFF).unwrap_or(0),
    )
}

/// Decode a cell's underline colour back to `(placement id, row)`. `None` for
/// every colour that isn't one of ours — a link carrier (the flag bit clear),
/// a real colour, or a marker whose id is the reserved `0`.
#[must_use]
pub fn carrier_parts(underline_color: Color) -> Option<(u32, u16)> {
    let Color::Rgb(r, g, b) = underline_color else {
        return None;
    };
    let packed = u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b);
    if packed & IMAGE_CARRIER_FLAG == 0 {
        return None;
    }
    let id = (packed & !IMAGE_CARRIER_FLAG) >> 8;
    (id != 0).then_some((id, (packed & 0xFF) as u16))
}
