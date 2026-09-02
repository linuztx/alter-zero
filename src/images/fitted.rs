//! Decoding a PNG at the size it will be **shown** or **sent** — never at its
//! own.
//!
//! The `image` crate decodes a picture whole — `4 × width × height` bytes of
//! RGBA — and only then shrinks it, so drawing a 1920×1080 screenshot into a
//! 120-column block materialised 8 MB to produce a 3 MB picture, and a 4K one
//! 33 MB; sending an attachment cost the same again on every turn it stayed in
//! context. Those buffers were the single largest allocations this process
//! ever made, and glibc's dynamic `mmap` threshold turned them from a spike
//! into a residue: a freed multi-megabyte block raises the threshold past its
//! own size, so the *next* one of that size is carved from the heap instead —
//! where a free never reaches the OS. Three pasted screenshots measured a
//! 103 MB process (`docs/memory.md`).
//!
//! The `png` crate underneath `image` can hand rows out one at a time, and a
//! shrink that only ever goes **down** needs nothing but the row in hand and
//! the destination row it is averaging into. So this module streams a PNG's
//! rows through an area-averaging [`Downsampler`] and materialises only the
//! fitted picture: peak memory is the *target* size plus one row, whatever
//! the file holds.
//!
//! Pure: it reads from any seekable [`BufRead`] and touches no policy, so every rule
//! here is unit-tested against a naive oracle (`images::tests`). The two
//! callers are the paint boundary ([`super::store`], fitting into the cell
//! box a placement reserved) and the payload downscale ([`super::payload`],
//! fitting into the model's pixel cap).

use std::io::{BufRead, Seek};

use image::{DynamicImage, RgbaImage};

/// The most pixels a picture may have before a **whole** decode is refused
/// outright — the non-PNG paths, and a PNG the streaming decoder declines.
/// Decoding is `4 × width × height` bytes resident, so a 12000×12000 scan
/// would cost 576 MB just to discover it should have been declined.
pub const WHOLE_DECODE_MAX_PIXELS: u64 = 50_000_000;

/// The most source pixels the streaming decode will walk. It never holds
/// them, so this bounds **time**, not memory — a row-by-row pass over a
/// 200-megapixel file is a few seconds on the main thread, and anything
/// bigger is not a picture anyone pasted.
pub const FIT_MAX_SOURCE_PIXELS: u64 = 200_000_000;

/// Whether a picture of `px` pixels may be decoded **whole** — four bytes a
/// pixel resident — under [`WHOLE_DECODE_MAX_PIXELS`]. Pure.
#[must_use]
pub fn whole_decode_fits(px: (u32, u32)) -> bool {
    u64::from(px.0) * u64::from(px.1) <= WHOLE_DECODE_MAX_PIXELS
}

/// Shrink `px` proportionally until it fits inside `box_px`, never growing.
///
/// This is `ratatui_image`'s own `Resize::Fit` arithmetic — which is the
/// `image` crate's `resize_dimensions` — reproduced so the picture handed to
/// the encoder is already the size it would have shrunk it to, and so its
/// cell footprint is exactly what [`super::fit_cells`] reserved: a row of
/// disagreement is a blank gap under every picture. Pinned to the encoder by
/// a differential test.
#[must_use]
pub fn fit_box(px: (u32, u32), box_px: (u32, u32)) -> (u32, u32) {
    let (w, h) = (px.0.max(1), px.1.max(1));
    // `Fit` clamps the target to the image itself first, which is what makes
    // it shrink-only: a picture smaller than the box scales by exactly 1.
    let (bw, bh) = (box_px.0.min(w).max(1), box_px.1.min(h).max(1));
    let ratio = (f64::from(bw) / f64::from(w)).min(f64::from(bh) / f64::from(h));
    (
        ((f64::from(w) * ratio).round() as u32).max(1),
        ((f64::from(h) * ratio).round() as u32).max(1),
    )
}

/// One source pixel's share of a destination axis: the first destination
/// index it lands on, its weight there, and its weight on the next one (`0`
/// when it lands whole). Under a shrink-only scale a source pixel spans at
/// most two destination pixels, so two weights are always enough.
#[derive(Debug, Clone, Copy)]
struct Span {
    first: usize,
    w0: f32,
    w1: f32,
}

/// The spans of every source pixel along an axis of `source` pixels shrunk
/// to `target` — the exact rational overlaps, in destination-pixel units, so
/// each destination pixel's weights sum to one and nothing needs normalising
/// afterwards.
fn spans(source: u32, target: u32) -> Vec<Span> {
    let (s, t) = (u64::from(source.max(1)), u64::from(target.max(1)));
    (0..s)
        .map(|x| {
            // Source pixel `x` covers [x·t, (x+1)·t) in units of 1/s of a
            // destination pixel; `right - 1` keeps the interval half-open so
            // a pixel ending exactly on a boundary never touches the next one.
            let left = x * t;
            let right = (x + 1) * t;
            let first = left / s;
            let last = (right - 1) / s;
            if first == last {
                Span {
                    first: first as usize,
                    w0: t as f32 / s as f32,
                    w1: 0.0,
                }
            } else {
                let split = (first + 1) * s;
                Span {
                    first: first as usize,
                    w0: (split - left) as f32 / s as f32,
                    w1: (right - split) as f32 / s as f32,
                }
            }
        })
        .collect()
}

/// A streaming area-average shrink of RGBA8 rows: feed the source rows top
/// to bottom, take the fitted picture at the end.
///
/// It holds one destination row's accumulator, one shrunk source row, and
/// the output — never the source picture — which is the whole point. At the
/// source size it is an exact copy; at a smaller one every destination pixel
/// is the mean of the source area it covers, which is also a better shrink
/// for a screenshot of text than the encoder's nearest-neighbour default.
pub struct Downsampler {
    source_width: usize,
    target: (u32, u32),
    columns: Vec<Span>,
    rows: Vec<Span>,
    /// The destination row being accumulated, `target.0 × 4` channels.
    acc: Vec<f32>,
    /// The current source row shrunk horizontally — scratch reused per row.
    shrunk: Vec<f32>,
    /// Which destination row `acc` belongs to.
    current: usize,
    /// Source rows fed so far.
    fed: usize,
    out: Vec<u8>,
}

impl Downsampler {
    /// A shrink from `source` to `target` pixels — clamped to the source on
    /// each edge, since this only ever goes down.
    #[must_use]
    pub fn new(source: (u32, u32), target: (u32, u32)) -> Self {
        let source = (source.0.max(1), source.1.max(1));
        let target = (target.0.clamp(1, source.0), target.1.clamp(1, source.1));
        let width = target.0 as usize * 4;
        Self {
            source_width: source.0 as usize,
            target,
            columns: spans(source.0, target.0),
            rows: spans(source.1, target.1),
            acc: vec![0.0; width],
            shrunk: vec![0.0; width],
            current: 0,
            fed: 0,
            out: vec![0; width * target.1 as usize],
        }
    }

    /// Feed the next source row, `source width × 4` bytes of RGBA. Rows past
    /// the source height are ignored.
    pub fn push_row(&mut self, rgba: &[u8]) {
        let Some(&span) = self.rows.get(self.fed) else {
            return;
        };
        self.fed += 1;
        debug_assert_eq!(
            rgba.len(),
            self.source_width * 4,
            "an RGBA row of the source width"
        );
        self.shrink_row(rgba);
        // A row that opens on a later destination row than the one being
        // accumulated means the previous one ended exactly on the boundary.
        if span.first > self.current {
            self.flush();
        }
        for (a, v) in self.acc.iter_mut().zip(&self.shrunk) {
            *a += span.w0 * v;
        }
        if span.w1 > 0.0 {
            // The row straddles a boundary: what is above it finishes the
            // current destination row, what is below opens the next.
            self.flush();
            for (a, v) in self.acc.iter_mut().zip(&self.shrunk) {
                *a += span.w1 * v;
            }
        }
    }

    /// Shrink one source row horizontally into `self.shrunk`.
    fn shrink_row(&mut self, rgba: &[u8]) {
        self.shrunk.fill(0.0);
        for (span, px) in self.columns.iter().zip(rgba.chunks_exact(4)) {
            let at = span.first * 4;
            for (c, &v) in px.iter().enumerate() {
                self.shrunk[at + c] += span.w0 * f32::from(v);
            }
            if span.w1 > 0.0 {
                for (c, &v) in px.iter().enumerate() {
                    self.shrunk[at + 4 + c] += span.w1 * f32::from(v);
                }
            }
        }
    }

    /// Round the accumulated destination row out and move on to the next.
    fn flush(&mut self) {
        let width = self.target.0 as usize * 4;
        if let Some(row) = self
            .out
            .get_mut(self.current * width..(self.current + 1) * width)
        {
            for (o, a) in row.iter_mut().zip(&self.acc) {
                *o = a.round().clamp(0.0, 255.0) as u8;
            }
        }
        self.acc.fill(0.0);
        self.current += 1;
    }

    /// The fitted picture. Destination rows no source row reached (a
    /// truncated file) stay transparent black.
    #[must_use]
    pub fn finish(mut self) -> RgbaImage {
        if self.fed > 0 && self.current < self.target.1 as usize {
            self.flush();
        }
        RgbaImage::from_raw(self.target.0, self.target.1, self.out)
            .unwrap_or_else(|| RgbaImage::new(self.target.0, self.target.1))
    }
}

/// Decode the PNG on `reader`, shrunk to the size `target_for` picks for its
/// pixel size — streaming its rows, so the source picture is never held.
///
/// `target_for` sees the header's `(width, height)` and answers the pixel
/// size to shrink to; anything not smaller is clamped to the source (this
/// only ever shrinks). A source without an alpha channel comes back without
/// one, so a re-encode for the model doesn't grow by a channel of `255`s.
///
/// `Err` for anything that isn't a PNG this can stream — a different format,
/// a corrupt or truncated file, an **interlaced** picture (whose rows arrive
/// out of order), or one past [`FIT_MAX_SOURCE_PIXELS`]; the callers fall
/// back to a whole decode for those, which has its own, tighter bound.
pub fn decode_png_fitted<R: BufRead + Seek>(
    reader: R,
    target_for: impl FnOnce((u32, u32)) -> (u32, u32),
) -> Result<DynamicImage, String> {
    let mut decoder = png::Decoder::new(reader);
    // Every colour type and depth as 8-bit samples: palettes expanded, 16-bit
    // stripped — so a row is always 1, 2, 3 or 4 bytes a pixel.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let info = reader.info();
    if info.interlaced {
        return Err("an interlaced PNG is decoded whole".to_string());
    }
    let source = (info.width, info.height);
    if u64::from(source.0) * u64::from(source.1) > FIT_MAX_SOURCE_PIXELS {
        return Err(format!(
            "{}x{} is more than {FIT_MAX_SOURCE_PIXELS} pixels — too large to stream",
            source.0, source.1
        ));
    }
    let (color, depth) = reader.output_color_type();
    if depth != png::BitDepth::Eight {
        return Err(format!("unexpected sample depth {depth:?}"));
    }
    let channels = color.samples();
    let has_alpha = matches!(color, png::ColorType::GrayscaleAlpha | png::ColorType::Rgba);
    let target = target_for(source);
    let mut down = Downsampler::new(source, target);
    let mut rgba = vec![0u8; source.0 as usize * 4];
    let mut rows = 0u32;
    while let Some(row) = reader.next_row().map_err(|e| e.to_string())? {
        expand_to_rgba(row.data(), channels, &mut rgba);
        down.push_row(&rgba);
        rows += 1;
    }
    if rows < source.1 {
        // A truncated file: fewer rows than the header promised.
        return Err(format!("only {rows} of {} rows arrived", source.1));
    }
    let fitted = down.finish();
    Ok(if has_alpha {
        DynamicImage::ImageRgba8(fitted)
    } else {
        DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(fitted).into_rgb8())
    })
}

/// Widen one decoded row of `channels` samples per pixel to RGBA.
fn expand_to_rgba(row: &[u8], channels: usize, rgba: &mut [u8]) {
    match channels {
        1 => {
            for (out, px) in rgba.chunks_exact_mut(4).zip(row.chunks_exact(1)) {
                out.copy_from_slice(&[px[0], px[0], px[0], 255]);
            }
        }
        2 => {
            for (out, px) in rgba.chunks_exact_mut(4).zip(row.chunks_exact(2)) {
                out.copy_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        3 => {
            for (out, px) in rgba.chunks_exact_mut(4).zip(row.chunks_exact(3)) {
                out.copy_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        _ => {
            let n = rgba.len().min(row.len());
            rgba[..n].copy_from_slice(&row[..n]);
        }
    }
}
