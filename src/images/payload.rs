//! Downscaling a picture for the **model** — not for the screen.
//!
//! `/settings` **Auto-resize images** is about the request, not the terminal:
//! a 12-megapixel phone photo or a retina screenshot is megabytes of base64
//! that every provider either refuses outright or bills in full, and a model
//! reads it no better than the same picture at 2000 pixels. So the two paths
//! that upload pixels — the `read` tool's image branch and a Ctrl+V
//! attachment — run their bytes through here first.
//!
//! Two things keep that from being the memory spike it used to be
//! (`docs/memory.md`). The shrink itself rides [`super::fitted`]: a PNG is
//! area-averaged **as its rows stream past**, so the file's own pixel count
//! never lands on the heap, and anything else is thumbnailed without the
//! resampler's `f32` pass, and refused whole past
//! [`super::WHOLE_DECODE_MAX_PIXELS`]. And the result is **kept on disk** — one
//! sidecar per picture under the session's temp root
//! ([`set_payload_cache_dir`]), keyed on the file's path, size, mtime and the
//! cap — because an attachment is re-sent with the context on **every later
//! turn**, and each of those used to decode and shrink the original all over
//! again. A later turn now reads a small file it already has
//! ([`cached_downscale`]) and touches the original only to `stat` it.
//!
//! Boundary code (it decodes, re-encodes and writes); the key and the
//! shrink-only aspect math it sits on are pure and tested.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use image::{DynamicImage, ImageFormat};

use super::fitted::{decode_png_fitted, whole_decode_fits};
use super::{AUTO_RESIZE_MAX_PIXELS, auto_resizing, resize_target};

/// The JPEG quality a downscaled photo is re-encoded at — the reference
/// harness's 80, indistinguishable from the original at this size and a
/// fraction of a PNG of the same picture.
const JPEG_QUALITY: u8 = 80;

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

/// [`downscale_for_model`] for the picture **at `path`**, through the disk
/// cache: a payload built on an earlier turn is read back, and one built now
/// is written for the next. `bytes` is the file's content, which the callers
/// already hold — the `read` tool read it for its fact line, the request
/// builder to fall back to the original. The same `None` cases as the
/// uncached form; the cache never changes *what* is sent, only what it costs.
#[must_use]
pub fn downscale_for_model_at(
    path: &Path,
    bytes: &[u8],
    format: ImageFormat,
) -> Option<Downscaled> {
    if !auto_resizing() {
        return None;
    }
    if let Some(hit) = cached_downscale(path, format) {
        return Some(hit);
    }
    let small = downscale_to(bytes, format, AUTO_RESIZE_MAX_PIXELS)?;
    if let Some(sidecar) = sidecar(path, format, AUTO_RESIZE_MAX_PIXELS) {
        remember(&sidecar, &small.bytes);
    }
    Some(small)
}

/// The payload already built for the picture at `path`, if this session's
/// cache holds one for the file **as it is now** — same size and mtime — and
/// the setting is on. Nothing is decoded: the sidecar is read and its header
/// asked for the size. `None` is "build it", never an error.
#[must_use]
pub fn cached_downscale(path: &Path, format: ImageFormat) -> Option<Downscaled> {
    if !auto_resizing() {
        return None;
    }
    let sidecar = sidecar(path, format, AUTO_RESIZE_MAX_PIXELS)?;
    let bytes = std::fs::read(&sidecar).ok()?;
    let format = payload_format(format);
    let size = image::ImageReader::with_format(Cursor::new(&bytes), format)
        .into_dimensions()
        .ok()?;
    Some(Downscaled {
        bytes,
        format,
        size,
    })
}

/// [`downscale_for_model`] with the cap spelled out — the testable half,
/// which does not consult the policy or the cache.
#[must_use]
pub fn downscale_to(bytes: &[u8], format: ImageFormat, max: u32) -> Option<Downscaled> {
    if bytes.len() > MAX_DECODE_BYTES {
        return None;
    }
    // Ask the header for the size before decoding a single pixel: that read is
    // a few hundred bytes, and it is what keeps an absurd image from being
    // materialised in memory just to discover it is absurd.
    let px = image::ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .ok()?;
    let (width, height) = resize_target(px, max)?;
    let resized = shrink(bytes, format, px, (width, height))?;
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
/// decoded whole (refused past [`super::WHOLE_DECODE_MAX_PIXELS`], where the whole
/// decode would be the spike this module exists to avoid) and thumbnailed: a
/// box filter that keeps a screenshot's thin strokes where nearest-neighbour
/// drops them, and unlike `resize` needs no `f32` working copy of the source.
fn shrink(
    bytes: &[u8],
    format: ImageFormat,
    px: (u32, u32),
    target: (u32, u32),
) -> Option<DynamicImage> {
    if format == ImageFormat::Png
        && let Ok(image) = decode_png_fitted(Cursor::new(bytes), |_| target)
    {
        return Some(image);
    }
    if !whole_decode_fits(px) {
        return None;
    }
    let image = image::load_from_memory_with_format(bytes, format).ok()?;
    Some(image.thumbnail_exact(target.0, target.1))
}

/// The sidecar holding the payload already built for the picture at `path`
/// — the file **as it is now**, under the setting on — and the format it was
/// re-encoded as, so the request can stream those bytes without opening the
/// original or decoding anything. `None` is "no payload cached", never an
/// error.
#[must_use]
pub fn cached_payload_file(path: &Path, format: ImageFormat) -> Option<(PathBuf, ImageFormat)> {
    if !auto_resizing() {
        return None;
    }
    let sidecar = sidecar(path, format, AUTO_RESIZE_MAX_PIXELS)?;
    sidecar.is_file().then(|| (sidecar, payload_format(format)))
}

/// The MIME a downscaled payload is sent under — the two formats the
/// re-encoder writes.
#[must_use]
pub fn payload_mime(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "image/jpeg",
        _ => "image/png",
    }
}

/// What a source `format` is re-encoded as: a JPEG stays JPEG (a photo as
/// PNG grows), everything else — PNG, GIF, WebP — becomes PNG, the only other
/// format this build has an encoder for.
fn payload_format(source: ImageFormat) -> ImageFormat {
    if source == ImageFormat::Jpeg {
        ImageFormat::Jpeg
    } else {
        ImageFormat::Png
    }
}

/// Re-encode `image` as [`payload_format`] says.
fn encode(image: &DynamicImage, source: ImageFormat) -> Option<(Vec<u8>, ImageFormat)> {
    let mut out = Cursor::new(Vec::new());
    let format = payload_format(source);
    if format == ImageFormat::Jpeg {
        // JPEG has no alpha channel; handing it RGBA is an encoder error.
        image
            .to_rgb8()
            .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut out,
                JPEG_QUALITY,
            ))
            .ok()?;
    } else {
        image.write_to(&mut out, ImageFormat::Png).ok()?;
    }
    Some((out.into_inner(), format))
}

// --- The disk cache ---

fn cache_dir_cell() -> &'static Mutex<Option<PathBuf>> {
    static DIR: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    DIR.get_or_init(|| Mutex::new(None))
}

/// Publish where this session keeps its downscaled payloads — the boundary's
/// one write, at startup (`{session_root}/images`, created there). `None`
/// (the default, and every unit test) means every turn builds its payloads
/// afresh, as before the cache existed.
pub fn set_payload_cache_dir(dir: Option<PathBuf>) {
    if let Ok(mut guard) = cache_dir_cell().lock() {
        *guard = dir;
    }
}

fn payload_cache_dir() -> Option<PathBuf> {
    cache_dir_cell().lock().ok()?.clone()
}

/// The sidecar's file name for the picture at `path` in a given state, under
/// a given cap: 32 hex digits of a SHA-256 over all four, so a changed file
/// (a new size or mtime) or a changed cap is a different entry and a stale
/// payload can never be served. Pure, tested.
#[must_use]
pub fn payload_cache_key(path: &Path, len: u64, mtime_nanos: u128, max: u32) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(path.as_os_str().as_encoded_bytes());
    hasher.update(len.to_le_bytes());
    hasher.update(mtime_nanos.to_le_bytes());
    hasher.update(max.to_le_bytes());
    hasher
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A file's size and modification time (nanoseconds since the epoch) — the
/// two facts every picture cache in this module keys on: the payload sidecar
/// here, the wire attachment ([`super::attachment`]) and the encoded picture
/// ([`super::store`]) alike. A changed file is a different state, so a stale
/// copy is never served, and reading it is one `stat`, never the bytes.
pub(super) type FileState = (u64, u128);

/// The [`FileState`] of the file at `path` as it is now — `None` when the
/// file can't be described (gone, unreadable).
pub(super) fn file_state(path: &Path) -> Option<FileState> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((meta.len(), mtime))
}

/// Where the payload for `path` lives, if a cache dir is set and the file can
/// be described.
fn sidecar(path: &Path, format: ImageFormat, max: u32) -> Option<PathBuf> {
    let dir = payload_cache_dir()?;
    let (len, mtime) = file_state(path)?;
    let ext = if payload_format(format) == ImageFormat::Jpeg {
        "jpg"
    } else {
        "png"
    };
    Some(dir.join(format!(
        "{}.{ext}",
        payload_cache_key(path, len, mtime, max)
    )))
}

/// Write `bytes` at `sidecar` — through a temp file and a rename, so a turn
/// reading the cache on another thread never sees a half-written payload.
/// Best-effort: a cache that can't be written costs a rebuild next turn.
/// How many bytes of downscaled payloads one session keeps on disk before
/// the oldest are dropped. The cache exists so a re-sent attachment is never
/// decoded twice (`docs/memory.md`); it is not a reason for a long session
/// that pasted forty screenshots to hold every shrunk copy of them forever.
/// `0` is no limit.
pub const PAYLOAD_CACHE_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Which cached payloads to delete, oldest first, so `incoming` bytes fit
/// under `cap` beside what is already there. `cap` of `0` is no limit.
///
/// An `incoming` larger than the whole cap empties the cache and stops —
/// the copy is still written. The cap bounds what is *kept*, and refusing to
/// cache a big picture would mean decoding it again on every turn it stays
/// in context, which is the cost this cache exists to remove. Pure: the
/// boundary stats the directory and removes what this names.
#[must_use]
pub fn cache_eviction(
    entries: &[(PathBuf, u64, std::time::SystemTime)],
    incoming: u64,
    cap: u64,
) -> Vec<PathBuf> {
    if cap == 0 {
        return Vec::new();
    }
    let mut oldest: Vec<&(PathBuf, u64, std::time::SystemTime)> = entries.iter().collect();
    oldest.sort_by_key(|(_, _, modified)| *modified);
    let mut total: u64 = entries
        .iter()
        .map(|(_, bytes, _)| *bytes)
        .sum::<u64>()
        .saturating_add(incoming);
    let mut out = Vec::new();
    for (path, bytes, _) in oldest {
        if total <= cap {
            break;
        }
        total = total.saturating_sub(*bytes);
        out.push(path.clone());
    }
    out
}

/// Make room for `bytes` under [`PAYLOAD_CACHE_MAX_BYTES`] by deleting the
/// oldest sidecars in `dir` ([`cache_eviction`]). Best-effort and stat-only.
fn evict_for(dir: &Path, bytes: u64) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let entries: Vec<(PathBuf, u64, std::time::SystemTime)> = read
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((entry.path(), meta.len(), meta.modified().ok()?))
        })
        .collect();
    for stale in cache_eviction(&entries, bytes, PAYLOAD_CACHE_MAX_BYTES) {
        let _ = std::fs::remove_file(stale);
    }
}

fn remember(sidecar: &Path, bytes: &[u8]) {
    let Some(dir) = sidecar.parent() else {
        return;
    };
    evict_for(dir, bytes.len() as u64);
    let Ok(tmp) = tempfile::Builder::new()
        .prefix(".payload-")
        .tempfile_in(dir)
    else {
        return;
    };
    if std::fs::write(tmp.path(), bytes).is_ok() {
        let _ = tmp.persist(sidecar);
    }
}
