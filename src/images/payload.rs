//! Downscaling a picture for the **model** — not for the screen.
//!
//! `/settings` **Auto-resize images** is about the request, not the terminal:
//! a 12-megapixel phone photo or a retina screenshot is megabytes of base64
//! that every provider either refuses outright or bills in full, and a model
//! reads it no better than the same picture at 2000 pixels. So the two paths
//! that upload pixels — the `read` tool's image branch and a Ctrl+V
//! attachment — run their bytes through here first.
//!
//! Boundary code (it decodes and re-encodes); the shrink-only aspect math it
//! sits on is the pure [`super::resize_target`], and a PNG's shrink is the
//! streaming [`super::fitted`] decode, so a 4K screenshot costs its 2000-pixel
//! target to send — on every turn it stays in context — rather than its own
//! 33 MB (`docs/memory.md`).

use image::{DynamicImage, ImageFormat, imageops::FilterType};

use super::{AUTO_RESIZE_MAX_PIXELS, auto_resizing, fitted, resize_target};

/// The JPEG quality a downscaled photo is re-encoded at — the reference
/// harness's 80, indistinguishable from the original at this size and a
/// fraction of a PNG of the same picture.
const JPEG_QUALITY: u8 = 80;

/// The most pixels a picture may have before the downscale declines to touch
/// it. Decoding is `4 × width × height` bytes of resident memory
/// (`docs/memory.md`), so a 12000×12000 scan would cost 576 MB to shrink —
/// past this the caller refuses the read instead, and says so.
const MAX_DECODE_PIXELS: u64 = 50_000_000;

/// The most *source* bytes the downscale will decode, for the shapes where
/// the pixel count alone doesn't bound the work (a deeply-layered PNG, an
/// animation). Generous next to the ~3.75 MB an image read may upload.
const MAX_DECODE_BYTES: usize = 64 * 1024 * 1024;

/// A picture re-encoded small enough to send.
pub struct Downscaled {
    /// The re-encoded bytes.
    pub bytes: Vec<u8>,
    /// What they are now — a JPEG stays a JPEG; everything else becomes PNG,
    /// the one lossless format this build can both read and write.
    pub format: ImageFormat,
    /// The new pixel size.
    pub size: (u32, u32),
}

/// `bytes` shrunk to at most [`AUTO_RESIZE_MAX_PIXELS`] on each edge, or
/// `None` when nothing needed to change — the picture already fits, the
/// setting is off, the bytes don't decode, or the re-encode came out no
/// smaller than the original (a photo re-encoded as PNG easily does, and
/// uploading *more* bytes to save some would be the wrong trade).
#[must_use]
pub fn downscale_for_model(bytes: &[u8], format: ImageFormat) -> Option<Downscaled> {
    if !auto_resizing() {
        return None;
    }
    downscale_to(bytes, format, AUTO_RESIZE_MAX_PIXELS)
}

/// [`downscale_for_model`] with the cap spelled out — the testable half,
/// which does not consult the policy.
#[must_use]
pub fn downscale_to(bytes: &[u8], format: ImageFormat, max: u32) -> Option<Downscaled> {
    if bytes.len() > MAX_DECODE_BYTES {
        return None;
    }
    // Ask the header for the size before decoding a single pixel: that read is
    // a few hundred bytes, and it is what keeps an absurd image from being
    // materialised in memory just to discover it is absurd.
    let px = image::ImageReader::with_format(std::io::Cursor::new(bytes), format)
        .into_dimensions()
        .ok()?;
    if u64::from(px.0) * u64::from(px.1) > MAX_DECODE_PIXELS {
        return None;
    }
    let (width, height) = resize_target(px, max)?;
    let resized = shrink(bytes, format, (width, height))?;
    let size = (resized.width(), resized.height());
    let (encoded, format) = encode(&resized, format)?;
    (encoded.len() < bytes.len()).then_some(Downscaled {
        bytes: encoded,
        format,
        size,
    })
}

/// `bytes` decoded and shrunk to `target`. A PNG streams through the fitted
/// decoder, which holds the target and one row rather than the whole
/// picture; anything else — or a PNG the streaming decoder declines — is
/// decoded whole and shrunk with `Triangle`, over the default nearest
/// neighbour, because this picture is going to a model and nearest-neighbour
/// downscaling of text in a screenshot drops whole strokes.
fn shrink(bytes: &[u8], format: ImageFormat, target: (u32, u32)) -> Option<DynamicImage> {
    if format == ImageFormat::Png
        && let Ok(image) = fitted::decode_png_fitted(std::io::Cursor::new(bytes), |_| target)
    {
        return Some(image);
    }
    let image = image::load_from_memory_with_format(bytes, format).ok()?;
    Some(image.resize(target.0, target.1, FilterType::Triangle))
}

/// Re-encode `image`: a JPEG source stays JPEG (a photo as PNG grows), and
/// everything else — PNG, GIF, WebP — becomes PNG, which is the only other
/// format this build has an encoder for.
fn encode(image: &DynamicImage, source: ImageFormat) -> Option<(Vec<u8>, ImageFormat)> {
    let mut out = std::io::Cursor::new(Vec::new());
    if source == ImageFormat::Jpeg {
        // JPEG has no alpha channel; handing it RGBA is an encoder error.
        image
            .to_rgb8()
            .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut out,
                JPEG_QUALITY,
            ))
            .ok()?;
        return Some((out.into_inner(), ImageFormat::Jpeg));
    }
    image.write_to(&mut out, ImageFormat::Png).ok()?;
    Some((out.into_inner(), ImageFormat::Png))
}
