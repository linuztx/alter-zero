//! A GIF on the wire is never the GIF itself (`docs/imagegen.md`, *A GIF the
//! model looks at*).
//!
//! OpenAI's vision takes only a GIF that does not move, Gemini's takes no GIF
//! at all, and a conversation re-sends every picture it holds on every turn,
//! so one GIF a provider refused would fail every request after it. What goes
//! up instead is a **still**: a PNG of the GIF's frames tiled in a regular
//! grid of equal cells, left to right and top to bottom — a GIF of one frame
//! is that frame, a long one is shown by [`SHEET_MAX_FRAMES`] of its frames,
//! evenly spaced, so the model sees the motion it is looking at. A GIF passed
//! to `imagegen` as a reference goes up as its first frame
//! ([`gif_first_frame`]), the picture an edit edits.
//!
//! The walker ([`gif_info`]) and the layout ([`sheet_grid`], [`sheet_frames`],
//! [`sheet_cell`]) are pure. [`gif_still`] decodes the frames one at a time,
//! each shrunk into its cell before the next is read, so a still costs one
//! decoded frame and the sheet, whatever the GIF's length (`docs/memory.md`).
//! It is deterministic: the turn that attaches a GIF and every turn that
//! re-sends it — after a restart too — send the same bytes, which the prompt
//! cache depends on (`docs/prompt-caching.md`).

use std::io::{self, Read};
use std::path::Path;

use image::RgbaImage;

/// The most frames a still shows; a longer GIF is shown by this many, evenly
/// spaced from its first frame to its last.
pub const SHEET_MAX_FRAMES: u32 = 16;

/// The longest side a still is drawn at, then the shorter ones it is redrawn
/// at until its PNG fits the cap.
const STILL_EDGES: [u32; 4] = [1024, 768, 512, 256];

/// What a GIF's own blocks say, read without decoding a pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GifInfo {
    /// The logical screen's width: every frame is drawn on a canvas this size.
    pub width: u32,
    /// The logical screen's height.
    pub height: u32,
    /// How many frames it holds.
    pub frames: u32,
}

/// Walk a GIF's blocks: its header and screen, then every extension and frame
/// to the trailer, skipping each frame's compressed data rather than
/// decoding it. A file cut short counts the frames that finished. `None` when
/// the content does not open as a GIF.
#[must_use]
pub fn gif_info(reader: impl Read) -> Option<GifInfo> {
    let mut reader = io::BufReader::new(reader);
    let mut head = [0u8; 13];
    reader.read_exact(&mut head).ok()?;
    if !is_gif_header(&head) {
        return None;
    }
    let width = u32::from(u16::from_le_bytes([head[6], head[7]]));
    let height = u32::from(u16::from_le_bytes([head[8], head[9]]));
    if head[10] & 0x80 != 0 {
        skip(&mut reader, color_table_len(head[10]))?;
    }
    let mut frames = 0;
    let mut byte = [0u8; 1];
    while reader.read_exact(&mut byte).is_ok() {
        match byte[0] {
            // An extension: its label, then its sub-blocks.
            0x21 => {
                if reader.read_exact(&mut byte).is_err() || skip_sub_blocks(&mut reader).is_none() {
                    break;
                }
            }
            // A frame: its descriptor, its own colours, its data.
            0x2C => {
                let mut descriptor = [0u8; 9];
                if reader.read_exact(&mut descriptor).is_err() {
                    break;
                }
                if descriptor[8] & 0x80 != 0
                    && skip(&mut reader, color_table_len(descriptor[8])).is_none()
                {
                    break;
                }
                // The data's minimum code size, then its sub-blocks.
                if reader.read_exact(&mut byte).is_err() || skip_sub_blocks(&mut reader).is_none() {
                    break;
                }
                frames += 1;
            }
            // The trailer — or a byte no block opens with.
            _ => break,
        }
    }
    Some(GifInfo {
        width,
        height,
        frames,
    })
}

/// Whether the file at `path` is a GIF, by its first bytes.
#[must_use]
pub fn is_gif(path: &Path) -> bool {
    let mut head = [0u8; 6];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut head))
        .is_ok()
        && is_gif_header(&head)
}

fn is_gif_header(head: &[u8]) -> bool {
    head.len() >= 6 && &head[..3] == b"GIF" && matches!(&head[3..6], b"87a" | b"89a")
}

/// The bytes a colour table takes, from the packed field that declares it.
fn color_table_len(packed: u8) -> u64 {
    3 * (1u64 << ((packed & 0x07) + 1))
}

fn skip(reader: &mut impl Read, n: u64) -> Option<()> {
    let skipped = io::copy(&mut reader.by_ref().take(n), &mut io::sink()).ok()?;
    (skipped == n).then_some(())
}

/// Skip data sub-blocks — a length byte, that many bytes — to the empty one
/// that ends them.
fn skip_sub_blocks(reader: &mut impl Read) -> Option<()> {
    let mut len = [0u8; 1];
    loop {
        reader.read_exact(&mut len).ok()?;
        if len[0] == 0 {
            return Some(());
        }
        skip(reader, u64::from(len[0]))?;
    }
}

/// The grid a still of `frames` frames is laid out in, `(columns, rows)`: the
/// squarest that holds them, at most one row short of square.
#[must_use]
pub fn sheet_grid(frames: u32) -> (u32, u32) {
    let frames = frames.max(1);
    let mut columns = 1;
    while columns * columns < frames {
        columns += 1;
    }
    (columns, frames.div_ceil(columns))
}

/// Which frames a still shows, by index: every one up to [`SHEET_MAX_FRAMES`],
/// else that many evenly spaced from the first to the last.
#[must_use]
pub fn sheet_frames(frames: u32) -> Vec<u32> {
    if frames <= SHEET_MAX_FRAMES {
        return (0..frames).collect();
    }
    let last = u64::from(frames - 1);
    let steps = u64::from(SHEET_MAX_FRAMES - 1);
    (0..u64::from(SHEET_MAX_FRAMES))
        .map(|i| u32::try_from((i * last + steps / 2) / steps).unwrap_or(frames - 1))
        .collect()
}

/// The size of one cell of a still: a frame of `frame` pixels, scaled so the
/// `grid` of them is at most `max_edge` on its long side, never larger than
/// the frame itself.
#[must_use]
pub fn sheet_cell(frame: (u32, u32), grid: (u32, u32), max_edge: u32) -> (u32, u32) {
    let (width, height) = (f64::from(frame.0.max(1)), f64::from(frame.1.max(1)));
    let edge = f64::from(max_edge.max(1));
    let scale = (edge / (width * f64::from(grid.0.max(1))))
        .min(edge / (height * f64::from(grid.1.max(1))))
        .min(1.0);
    (
        ((width * scale).floor() as u32).max(1),
        ((height * scale).floor() as u32).max(1),
    )
}

/// How a still shows a GIF of `frames` frames, in words:
/// `its 9 frames tiled left to right and top to bottom, 3 to a row`, or, past
/// [`SHEET_MAX_FRAMES`], `16 of its 40 frames, evenly spaced, tiled …`.
#[must_use]
pub fn tiled_frames(frames: u32) -> String {
    let shown = frames.min(SHEET_MAX_FRAMES);
    let (columns, _) = sheet_grid(shown);
    let which = if frames > shown {
        format!("{shown} of its {frames} frames, evenly spaced,")
    } else {
        format!("its {frames} frames")
    };
    format!("{which} tiled left to right and top to bottom, {columns} to a row")
}

/// A GIF as the still that goes up in its place ([`gif_still`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GifStill {
    /// The PNG.
    pub bytes: Vec<u8>,
    /// Its pixel size.
    pub size: (u32, u32),
    /// How many frames the GIF has.
    pub frames: u32,
    /// How many of them the still shows ([`sheet_frames`]).
    pub shown: u32,
    /// How many it shows to a row ([`sheet_grid`]).
    pub columns: u32,
}

/// The still that goes up in place of the GIF at `path`: its frames tiled
/// ([`sheet_grid`], [`sheet_frames`]) on a PNG at most 1024 pixels on its long
/// side, redrawn smaller until the PNG is at most `max_bytes`. `None` when the
/// file is no GIF, a frame's canvas is past what this build decodes whole
/// ([`super::whole_decode_fits`]), no frame decodes, or no still fits.
#[must_use]
pub fn gif_still(path: &Path, max_bytes: usize) -> Option<GifStill> {
    draw_still(path, max_bytes, sheet_frames)
}

/// The GIF at `path`'s first frame alone, drawn and held to `max_bytes` as
/// [`gif_still`] draws its sheet: what an `imagegen` edit of the GIF edits.
#[must_use]
pub fn gif_first_frame(path: &Path, max_bytes: usize) -> Option<GifStill> {
    draw_still(path, max_bytes, |_| vec![0])
}

/// A still of the frames `picks` chooses from the GIF's count.
fn draw_still(path: &Path, max_bytes: usize, picks: fn(u32) -> Vec<u32>) -> Option<GifStill> {
    let info = gif_info(std::fs::File::open(path).ok()?)?;
    if info.frames == 0 || !super::whole_decode_fits((info.width, info.height)) {
        return None;
    }
    let screen = (info.width, info.height);
    let picks = picks(info.frames);
    let shown = u32::try_from(picks.len()).ok()?;
    let grid = sheet_grid(shown);
    let cell = sheet_cell(screen, grid, STILL_EDGES[0]);
    let sheet = draw_sheet(path, &picks, grid, cell)?;
    let mut tried = None;
    for edge in STILL_EDGES {
        let small = sheet_cell(screen, grid, edge);
        if tried == Some(small) {
            continue;
        }
        tried = Some(small);
        let bytes = if small == cell {
            encode_png(&sheet)?
        } else {
            encode_png(&resample(&sheet, grid, cell, small))?
        };
        if bytes.len() <= max_bytes {
            return Some(GifStill {
                bytes,
                size: (grid.0 * small.0, grid.1 * small.1),
                frames: info.frames,
                shown,
                columns: grid.0,
            });
        }
    }
    None
}

/// Draw the frames `picks` names into a `grid` of `cell`-sized cells, decoding
/// them in order and shrinking each into its cell before the next is read. A
/// GIF that stops decoding keeps the cells drawn before it; one that draws
/// none has no sheet.
fn draw_sheet(path: &Path, picks: &[u32], grid: (u32, u32), cell: (u32, u32)) -> Option<RgbaImage> {
    use image::AnimationDecoder;
    let file = io::BufReader::new(std::fs::File::open(path).ok()?);
    let decoder = image::codecs::gif::GifDecoder::new(file).ok()?;
    let mut sheet = RgbaImage::new(grid.0 * cell.0, grid.1 * cell.1);
    let mut wanted = picks.iter().copied().zip(0u32..).peekable();
    let mut drawn = 0;
    for (index, frame) in (0u32..).zip(decoder.into_frames()) {
        let Some(&(pick, slot)) = wanted.peek() else {
            break;
        };
        let Ok(frame) = frame else {
            break;
        };
        if index != pick {
            continue;
        }
        wanted.next();
        let shrunk = image::imageops::thumbnail(frame.buffer(), cell.0, cell.1);
        let (x, y) = (slot % grid.0 * cell.0, slot / grid.0 * cell.1);
        image::imageops::replace(&mut sheet, &shrunk, i64::from(x), i64::from(y));
        drawn += 1;
    }
    (drawn > 0).then_some(sheet)
}

/// `sheet`'s cells of `from` pixels each shrunk to `to`, in the same grid.
fn resample(sheet: &RgbaImage, grid: (u32, u32), from: (u32, u32), to: (u32, u32)) -> RgbaImage {
    let mut small = RgbaImage::new(grid.0 * to.0, grid.1 * to.1);
    for row in 0..grid.1 {
        for column in 0..grid.0 {
            let cell =
                image::imageops::crop_imm(sheet, column * from.0, row * from.1, from.0, from.1)
                    .to_image();
            let shrunk = image::imageops::thumbnail(&cell, to.0, to.1);
            image::imageops::replace(
                &mut small,
                &shrunk,
                i64::from(column * to.0),
                i64::from(row * to.1),
            );
        }
    }
    small
}

fn encode_png(image: &RgbaImage) -> Option<Vec<u8>> {
    let mut out = io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}
