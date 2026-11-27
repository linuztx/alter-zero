//! An attachment encoded for the wire **once per session**, and shared.
//!
//! A pasted picture — or one the `read` tool looked at — rides every later
//! request as a base64 `data:` URL, because the whole conversation is re-sent
//! each turn (`docs/context.md`). Building that URL is the largest thing a
//! turn does with memory: for a 6 MB screenshot it used to be the file read
//! whole, an 8 MB string encoded from it, a copy of that string into the
//! request's JSON tree and a body grown by doubling — every turn, every
//! agentic round, on a fresh thread. glibc's dynamic `mmap` threshold puts
//! blocks of that size on a thread arena's heap, which never shrinks, so the
//! resident set stepped up by the picture's weight whenever a turn landed on
//! a new arena and never came back down (`docs/memory.md`, *Every turn
//! re-sent the picture*).
//!
//! So an attachment is encoded **once** — streamed from the file (or from the
//! payload cache's downscaled sidecar) straight into the one string the
//! session keeps — and every request shares that string by reference:
//! [`AttachmentUrl`] is the handle, cloned by the round's copy of the messages
//! and serialized in place by the body writer. The cache is keyed on the path
//! and validated against an [`AttachmentStamp`] — the file's size and mtime
//! plus the **Auto-resize images** setting, everything that decides what the
//! wire carries — so a rewritten file or a flipped setting is a rebuild, never
//! a stale picture. It is bounded in bytes ([`ATTACHMENT_CACHE_MAX_BYTES`],
//! the least-recently-sent picture dropped first) and swept to the pictures
//! the conversation still carries at every turn start
//! ([`retain_attachments`]), so a `/clear`, a backtrack or a `/compact` that
//! dropped a picture lets its megabytes go.
//!
//! The cache and the stamp are pure and unit-tested (`images::tests`); the
//! encoder that fills a miss is boundary code over the file.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, Read};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use image::ImageFormat;

use super::AUTO_RESIZE_MAX_PIXELS;
use super::payload::{
    cached_payload_file, downscale_file_to, downscale_for_model_at, payload_mime,
};
use super::registry::auto_resizing;
use super::resize_target;

/// How many bytes of encoded attachments one session keeps resident before
/// the least recently sent are dropped. An entry is the picture's base64 —
/// four thirds of its payload — so a 2000-pixel screenshot is a couple of
/// megabytes and a paste that already fits the model's cap is its file's
/// weight and a third. A picture past the cap is re-encoded on the next turn
/// that sends it: one transient, never a wrong picture.
pub const ATTACHMENT_CACHE_MAX_BYTES: usize = 32 * 1024 * 1024;

/// An attachment's encoded `data:` URL, shared rather than copied.
///
/// A paste is megabytes of base64, and the turn's messages are copied for
/// every round: the string behind this handle is allocated once and every
/// copy points at it. Derefs to the URL text, so it reads like the `String`
/// it replaced.
#[derive(Clone, PartialEq, Eq)]
pub struct AttachmentUrl(Arc<String>);

impl AttachmentUrl {
    /// Whether two handles share one allocation — what a cache hit means.
    #[must_use]
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }

    /// The URL text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Deref for AttachmentUrl {
    type Target = str;

    fn deref(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for AttachmentUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

impl fmt::Debug for AttachmentUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.0.as_str(), f)
    }
}

impl From<String> for AttachmentUrl {
    fn from(url: String) -> Self {
        Self(Arc::new(url))
    }
}

impl From<&str> for AttachmentUrl {
    fn from(url: &str) -> Self {
        Self(Arc::new(url.to_string()))
    }
}

impl serde::Serialize for AttachmentUrl {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.as_str())
    }
}

/// Everything that decides what an attachment's encoding contains, so a
/// lookup can tell a still-valid entry from a stale one: the file's size and
/// modification time, and whether **Auto-resize images** was on when it was
/// built (the setting changes what goes up, not the file).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachmentStamp {
    len: u64,
    mtime_nanos: u128,
    auto_resize: bool,
}

impl AttachmentStamp {
    /// A stamp from its parts.
    #[must_use]
    pub const fn new(len: u64, mtime_nanos: u128, auto_resize: bool) -> Self {
        Self {
            len,
            mtime_nanos,
            auto_resize,
        }
    }

    /// The stamp of the file at `path` as it is now, under the current
    /// setting — `None` when the file can't be described (gone, unreadable).
    fn of(path: &Path) -> Option<Self> {
        let (len, mtime_nanos) = super::payload::file_state(path)?;
        Some(Self::new(len, mtime_nanos, auto_resizing()))
    }
}

struct Entry {
    stamp: AttachmentStamp,
    url: AttachmentUrl,
}

/// The encoded attachments a session holds, least-recently-sent first out.
pub struct AttachmentCache {
    cap: usize,
    entries: HashMap<PathBuf, Entry>,
    /// Paths in least-recently-used order (the tail is the newest). A handful
    /// of entries at most, so a `Vec` beats a real LRU map.
    order: Vec<PathBuf>,
    bytes: usize,
}

impl AttachmentCache {
    /// An empty cache that keeps at most `cap` bytes of encodings.
    #[must_use]
    pub fn with_cap(cap: usize) -> Self {
        Self {
            cap,
            entries: HashMap::new(),
            order: Vec::new(),
            bytes: 0,
        }
    }

    /// The encoding held for `path`, if it was built from a file in the same
    /// state under the same setting. A hit is the newest entry from here on.
    pub fn get(&mut self, path: impl AsRef<Path>, stamp: AttachmentStamp) -> Option<AttachmentUrl> {
        let path = path.as_ref();
        let entry = self.entries.get(path)?;
        if entry.stamp != stamp {
            return None;
        }
        let url = entry.url.clone();
        self.touch(path);
        Some(url)
    }

    /// Hold `url` as the encoding of `path` in state `stamp`, replacing any
    /// older entry for the path, and drop the least recently sent pictures
    /// until the cache is back inside its cap — never this one, which is
    /// what the next request sends.
    pub fn insert(&mut self, path: PathBuf, stamp: AttachmentStamp, url: AttachmentUrl) {
        self.remove(&path);
        self.bytes = self.bytes.saturating_add(url.len());
        self.entries.insert(path.clone(), Entry { stamp, url });
        self.order.push(path.clone());
        while self.bytes > self.cap && self.order.len() > 1 {
            let Some(victim) = self.order.iter().find(|other| **other != path).cloned() else {
                break;
            };
            self.remove(&victim);
        }
    }

    /// Keep only the entries `keep` names.
    pub fn retain(&mut self, keep: impl Fn(&Path) -> bool) {
        let stale: Vec<PathBuf> = self
            .order
            .iter()
            .filter(|path| !keep(path))
            .cloned()
            .collect();
        for path in stale {
            self.remove(&path);
        }
    }

    /// Drop everything.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }

    /// The bytes of encodings held.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    fn remove(&mut self, path: &Path) {
        if let Some(entry) = self.entries.remove(path) {
            self.bytes = self.bytes.saturating_sub(entry.url.len());
        }
        self.order.retain(|other| other != path);
    }

    fn touch(&mut self, path: &Path) {
        if let Some(at) = self.order.iter().position(|other| other == path) {
            let path = self.order.remove(at);
            self.order.push(path);
        }
    }
}

fn cache() -> &'static Mutex<AttachmentCache> {
    static CACHE: OnceLock<Mutex<AttachmentCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(AttachmentCache::with_cap(ATTACHMENT_CACHE_MAX_BYTES)))
}

fn with_cache<T>(f: impl FnOnce(&mut AttachmentCache) -> T) -> Option<T> {
    let mut guard = cache().lock().ok()?;
    Some(f(&mut guard))
}

/// The `data:` URL the request carries for the picture at `path` — the
/// session's shared encoding, built on the first turn that sends it and
/// served by reference on every turn after. `None` when the file can't be
/// read, which the request builder turns into an `[image unavailable]` note.
///
/// A miss is built outside the lock (the encode is the slow part, and two
/// turns racing on one picture just both build it), then held under the
/// file's current stamp.
#[must_use]
pub fn attachment_data_url(path: &Path) -> Option<AttachmentUrl> {
    let stamp = AttachmentStamp::of(path)?;
    if let Some(hit) = with_cache(|cache| cache.get(path, stamp)).flatten() {
        return Some(hit);
    }
    let url = encode_attachment(path)?;
    with_cache(|cache| cache.insert(path.to_path_buf(), stamp, url.clone()));
    Some(url)
}

/// Hold `payload` (already the bytes the wire should carry, under `mime`) as
/// the encoding of the picture at `path` — the `read` tool's seam, which has
/// the bytes in hand and whose picture every later turn re-sends. The URL
/// comes back for the tool's own attachment.
#[must_use]
pub fn remember_attachment(path: &Path, payload: &[u8], mime: &str) -> AttachmentUrl {
    let url = encode_bytes(payload, mime);
    if let Some(stamp) = AttachmentStamp::of(path) {
        with_cache(|cache| cache.insert(path.to_path_buf(), stamp, url.clone()));
    }
    url
}

/// Let go of every encoding but the ones for `paths` — the pictures the
/// conversation still carries, taken at each turn start from the context
/// about to be sent.
pub fn retain_attachments(paths: &[&Path]) {
    with_cache(|cache| cache.retain(|path| paths.contains(&path)));
}

/// Let go of every encoding (a `/clear`).
pub fn clear_attachments() {
    with_cache(AttachmentCache::clear);
}

/// The MIME an attachment is sent under, by extension. The paste path only
/// ever writes the accepted formats (`docs/image-paste.md`); PNG — its
/// transcode target — is the default.
#[must_use]
pub fn attachment_mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

/// The decoder for an attachment's MIME — [`attachment_mime`]'s inverse,
/// `None` for anything this build cannot re-encode.
fn attachment_format(mime: &str) -> Option<ImageFormat> {
    match mime {
        "image/jpeg" => Some(ImageFormat::Jpeg),
        "image/png" => Some(ImageFormat::Png),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::WebP),
        _ => None,
    }
}

/// Build the encoding for the picture at `path`: the downscaled payload
/// where **Auto-resize images** calls for one (`docs/images.md`), else the
/// file itself — streamed into the one string, so the only picture-sized
/// allocation is the encoding that is kept. The original is read whole only
/// to shrink it, and only once: the shrunk copy is a sidecar on disk from
/// then on ([`super::payload`]).
fn encode_attachment(path: &Path) -> Option<AttachmentUrl> {
    // A GIF never goes up as itself, whatever its name says: a provider may
    // refuse one that moves, or any GIF at all, and every later turn would
    // re-send the refusal. Its frames go up tiled on a PNG ([`super::gif`]) —
    // or, when they cannot be drawn, nothing does, which the request turns
    // into a note (`docs/images.md` *A GIF goes up as its frames*).
    if super::gif::is_gif(path) {
        let still = super::gif::gif_still(path, crate::llm::tools::READ_IMAGE_MAX_BYTES)?;
        return Some(encode_bytes(&still.bytes, "image/png"));
    }
    // A picture too large to attach was sent as a smaller copy, kept beside
    // it so every later turn — a restart included — sends that copy again
    // ([`fitted_attachment_data_url`]).
    if let Some((copy, mime)) = fitted_copy(path) {
        return stream_file(&copy, mime);
    }
    let mime = attachment_mime(path);
    if let Some(format) = attachment_format(mime)
        && auto_resizing()
    {
        if let Some((sidecar, small_format)) = cached_payload_file(path, format) {
            return stream_file(&sidecar, payload_mime(small_format));
        }
        if needs_shrink(path, AUTO_RESIZE_MAX_PIXELS) {
            let bytes = std::fs::read(path).ok()?;
            if let Some(small) = downscale_for_model_at(path, &bytes, format) {
                drop(bytes);
                return Some(encode_bytes(&small.bytes, payload_mime(small.format)));
            }
            // Declined — the picture didn't decode, or came out no smaller —
            // so the file goes up as it is, like every other oversized one.
        }
    }
    stream_file(path, mime)
}

/// The folder beside a picture that holds the copy sent in its place when the
/// picture itself is past the attachment cap ([`fitted_attachment_data_url`],
/// `docs/imagegen.md`).
pub const FITTED_DIR: &str = ".attachments";

/// The edges a picture past the cap is shrunk to, largest first, until a
/// copy fits.
const FITTED_EDGES: [u32; 5] = [1536, 1024, 768, 512, 256];

/// An encoding held to an attachment cap: the `data:` URL, and the pixel size
/// it was shrunk to when the picture itself was too large to send — `None`
/// when it went up as it is.
#[derive(Debug, Clone)]
pub struct FittedAttachment {
    /// What the request carries.
    pub url: AttachmentUrl,
    /// The copy's pixel size, when a copy was sent.
    pub sent: Option<(u32, u32)>,
}

/// The encoding of the picture at `path`, its payload held to `cap` bytes —
/// [`attachment_data_url`]'s when that fits, else a copy shrunk until it
/// does. A provider refuses an image past its limit, and a conversation
/// re-sends every picture it holds on every turn, so a picture too large to
/// attach would fail each request from then on; the copy is written to
/// [`FITTED_DIR`] beside the picture, which the encoder sends in the
/// picture's place on every later turn — a restart included — and is held as
/// the picture's encoding now. The picture itself is never touched. `None`
/// when the file can't be read or no copy fits.
#[must_use]
pub fn fitted_attachment_data_url(path: &Path, cap: usize) -> Option<FittedAttachment> {
    // A GIF goes up as the still of its frames, already held to the
    // attachment cap: no copy to keep, since every turn draws the same one.
    if super::gif::is_gif(path) {
        let url = attachment_data_url(path)?;
        return (payload_len(&url) <= cap).then_some(FittedAttachment { url, sent: None });
    }
    // Decided before anything is encoded: a picture's encoding is its size
    // in memory again, and one made only to be thrown away is the residue
    // glibc keeps (`docs/memory.md`).
    let len = usize::try_from(std::fs::metadata(path).ok()?.len()).ok()?;
    let shrinks = auto_resizing() && needs_shrink(path, AUTO_RESIZE_MAX_PIXELS);
    if !shrinks && len <= cap {
        return attachment_data_url(path).map(|url| FittedAttachment { url, sent: None });
    }
    if shrinks {
        // **Auto-resize images** makes a smaller payload first, as for any
        // large picture; past the cap even so, it is fitted below.
        let url = attachment_data_url(path)?;
        if payload_len(&url) <= cap {
            return Some(FittedAttachment { url, sent: None });
        }
    }
    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let format = reader.format()?;
    let (width, height) = reader.into_dimensions().ok()?;
    let longest = width.max(height);
    let small = FITTED_EDGES
        .iter()
        .filter(|edge| **edge < longest)
        .find_map(|edge| downscale_file_to(path, format, *edge).filter(|s| s.bytes.len() <= cap))?;
    let copy = fitted_copy_path(path, small.format)?;
    write_copy(&copy, &small.bytes).ok()?;
    let url = remember_attachment(path, &small.bytes, payload_mime(small.format));
    Some(FittedAttachment {
        url,
        sent: Some(small.size),
    })
}

/// How many bytes a `data:` URL's payload decodes to, read off its length.
fn payload_len(url: &str) -> usize {
    let b64 = url.split_once(";base64,").map_or(url, |(_, b64)| b64);
    let padding = b64.bytes().rev().take_while(|b| *b == b'=').count();
    (b64.len() / 4 * 3).saturating_sub(padding)
}

/// Where the copy of the picture at `path` lives in `format`:
/// `{dir}/.attachments/{file name}.{png|jpg}`.
fn fitted_copy_path(path: &Path, format: ImageFormat) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let ext = if format == ImageFormat::Jpeg {
        "jpg"
    } else {
        "png"
    };
    Some(
        path.parent()?
            .join(FITTED_DIR)
            .join(format!("{name}.{ext}")),
    )
}

/// The copy sent in place of the picture at `path`, and its MIME — only one
/// made from the picture as it is now: a picture rewritten since its copy was
/// made is newer than the copy, and goes up itself.
fn fitted_copy(path: &Path) -> Option<(PathBuf, &'static str)> {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|meta| meta.modified()).ok();
    let picture = modified(path)?;
    [
        (ImageFormat::Png, "image/png"),
        (ImageFormat::Jpeg, "image/jpeg"),
    ]
    .into_iter()
    .filter_map(|(format, mime)| Some((fitted_copy_path(path, format)?, mime)))
    .find(|(copy, _)| modified(copy).is_some_and(|made| made >= picture))
}

/// Write the copy through a temp file and a rename, so a turn reading it on
/// another thread never sees half of one.
fn write_copy(copy: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = copy
        .parent()
        .ok_or_else(|| io::Error::other("a copy has a folder"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = tempfile::Builder::new().prefix(".copy-").tempfile_in(dir)?;
    std::fs::write(tmp.path(), bytes)?;
    tmp.persist(copy).map_err(|e| e.error)?;
    Ok(())
}

/// Whether the picture at `path` is larger than `max` on an edge, read from
/// its header alone. A header that can't be read answers `false`: the file
/// goes up as it is, which is what the downscale would have decided too.
fn needs_shrink(path: &Path, max: u32) -> bool {
    image::ImageReader::open(path)
        .ok()
        .and_then(|reader| reader.with_guessed_format().ok())
        .and_then(|reader| reader.into_dimensions().ok())
        .is_some_and(|px| resize_target(px, max).is_some())
}

/// The `data:` URL for bytes already in hand — the prefix first, the base64
/// appended onto it, one allocation.
fn encode_bytes(payload: &[u8], mime: &str) -> AttachmentUrl {
    let mut url = String::with_capacity(url_capacity(mime, payload.len()));
    url.push_str(&prefix(mime));
    crate::clipboard::base64_encode_into(payload, &mut url);
    AttachmentUrl::from(url)
}

/// The `data:` URL for the file at `path`, its bytes streamed through a
/// small buffer into the one string — the file is never held whole.
fn stream_file(path: &Path, mime: &str) -> Option<AttachmentUrl> {
    let file = std::fs::File::open(path).ok()?;
    let len = usize::try_from(file.metadata().ok()?.len()).ok()?;
    let mut url = String::with_capacity(url_capacity(mime, len));
    url.push_str(&prefix(mime));
    base64_encode_reader(io::BufReader::new(file), len, &mut url).ok()?;
    Some(AttachmentUrl::from(url))
}

fn prefix(mime: &str) -> String {
    format!("data:{mime};base64,")
}

fn url_capacity(mime: &str, payload_len: usize) -> usize {
    prefix(mime).len() + payload_len.div_ceil(3) * 4
}

/// Base64-encode everything `reader` yields onto `out`, `len_hint` bytes
/// expected. Reads through a buffer that is a multiple of three, so every
/// chunk but the last encodes as whole groups and the result is exactly what
/// `clipboard::base64_encode_into` makes of the same bytes.
pub fn base64_encode_reader(
    mut reader: impl Read,
    len_hint: usize,
    out: &mut String,
) -> io::Result<()> {
    const CHUNK: usize = 48 * 1024;
    out.reserve(len_hint.div_ceil(3) * 4);
    let mut buf = vec![0u8; CHUNK];
    loop {
        let mut filled = 0;
        while filled < CHUNK {
            match reader.read(&mut buf[filled..])? {
                0 => break,
                n => filled += n,
            }
        }
        crate::clipboard::base64_encode_into(&buf[..filled], out);
        if filled < CHUNK {
            return Ok(());
        }
    }
}
