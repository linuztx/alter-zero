//! Clipboard I/O — the boundary for Ctrl+V image paste (read) and `/copy`
//! (write).
//!
//! The **read** side is a focused port of codex's `clipboard_paste.rs`: pull an
//! image off the system clipboard (raw bytes, or a copied image *file*) and
//! write it to a kept temp file whose path the backend reads (the TUI never
//! base64-encodes it — codex parity). A file already in an accepted format is
//! **copied verbatim** (no decode/re-encode — the fast path); anything else is
//! transcoded to PNG. The event loop calls this on a **background thread**
//! (`main.rs::spawn_image_paste`): a large screenshot's PNG encode takes real
//! time, and running it on the loop would freeze the status animations. The
//! **write** side ([`copy_to_clipboard`]) is codex's `/copy` path: set the
//! clipboard via arboard, falling back to an OSC 52 terminal escape when there's
//! no native clipboard (headless / SSH / tmux). See `docs/copy.md`.
//!
//! The clipboard/filesystem/terminal **I/O** here carries no unit tests (like
//! `term.rs`); the happy paths need a real clipboard server, so they're
//! exercised by hand / on a desktop, and `scripts/smoke.sh` covers the no-clipboard
//! paths (the image read's graceful error, and `/copy`'s OSC 52 fallback in
//! tmux). The **pure** helpers it calls *are* unit-tested, though — the image
//! attach/placeholder logic in `App` ([`crate::app::App::attach_image`]), and
//! the `base64`/OSC 52 framing below (like `frame`'s rate-limit math).

use std::io::Write;
use std::path::PathBuf;

/// Read an image from the system clipboard, returning the path to a freshly
/// written temporary image file (kept on disk for the backend to read by
/// path). **Blocking** — the loop runs it on a worker thread
/// (`main.rs::spawn_image_paste`), never inline.
///
/// On any failure — no clipboard server (headless / unsupported), nothing
/// image-shaped on the clipboard, or an encode/write error — returns a short
/// human-readable message for the red `Failed to paste image: {msg}` notice the
/// loop commits (codex's `new_error_event`).
///
/// Mirrors codex: prefer an image *file* on the clipboard (e.g. one copied in
/// a GUI file manager) — copied verbatim when it is already in an accepted
/// format, transcoded to PNG otherwise ([`temp_image_from_files`]) — falling
/// back to the raw RGBA image bytes (e.g. a screenshot), which are always
/// PNG-encoded.
pub fn read_clipboard_image() -> Result<PathBuf, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;

    // An image *file* on the clipboard (codex's `file_list()` path — one copied
    // in a GUI file manager) is handled without a full decode where possible.
    if let Ok(files) = clipboard.get().file_list()
        && let Some(result) = temp_image_from_files(&files)
    {
        return result;
    }

    // Raw image bytes (e.g. a screenshot) — rebuild and PNG-encode.
    let image = clipboard_raw_image(&mut clipboard)
        .ok_or_else(|| "no image on the clipboard".to_string())?;
    encode_png_to_temp(&image)
}

/// Produce the temp file for the first usable image among `files`, or `None`
/// when no file yields one (the caller falls through to the raw-bytes path).
///
/// A file already in a format the backend accepts ([`accepted_image_extension`])
/// whose header parses (`image::image_dimensions` reads only the header) is
/// **copied verbatim** — no decode, no re-encode: the fast path that makes a
/// file paste effectively instant. Anything else `image::open` can read (it
/// sniffs content, not just the extension) is transcoded to a temp PNG as
/// before. Either way the result is **our own temp copy**, never the user's
/// path — the composer's discard cleanup deletes what this returns
/// (docs/image-paste.md).
fn temp_image_from_files(files: &[PathBuf]) -> Option<Result<PathBuf, String>> {
    for path in files {
        if let Some(ext) = accepted_image_extension(path)
            && image::image_dimensions(path).is_ok()
        {
            return Some(copy_file_to_temp(path, &ext));
        }
    }
    let image = files.iter().find_map(|path| {
        // Sniff the content, not just the extension (`image::open` trusts the
        // extension alone) — a mislabelled image file still decodes.
        image::ImageReader::open(path)
            .ok()?
            .with_guessed_format()
            .ok()?
            .decode()
            .ok()
    })?;
    Some(encode_png_to_temp(&image))
}

/// The lowercased extension of `path` when it names a format the backend seam
/// accepts as-is (the codecs this crate compiles: png/jpg/jpeg/gif/webp) —
/// `None` sends the file through the decode-and-transcode path. Pure, tested.
fn accepted_image_extension(path: &std::path::Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp").then_some(ext)
}

/// Copy `path`'s bytes verbatim into a kept temp file with the same `ext`.
fn copy_file_to_temp(path: &std::path::Path, ext: &str) -> Result<PathBuf, String> {
    let tmp = tempfile::Builder::new()
        .prefix("inline-tui-clipboard-")
        .suffix(&format!(".{ext}"))
        .tempfile()
        .map_err(|e| format!("could not create a temp file: {e}"))?;
    std::fs::copy(path, tmp.path()).map_err(|e| format!("could not copy the image: {e}"))?;
    keep_temp(tmp)
}

/// PNG-encode `image` into a kept temp file — the transcode path for raw
/// clipboard bytes and for file formats not on the verbatim-copy list.
fn encode_png_to_temp(image: &image::DynamicImage) -> Result<PathBuf, String> {
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("could not encode the image: {e}"))?;
    let tmp = tempfile::Builder::new()
        .prefix("inline-tui-clipboard-")
        .suffix(".png")
        .tempfile()
        .map_err(|e| format!("could not create a temp file: {e}"))?;
    std::fs::write(tmp.path(), &png).map_err(|e| format!("could not write the image: {e}"))?;
    keep_temp(tmp)
}

/// Keep the temp file past its guard — the backend reads it by path, so it
/// must outlive the `TempfileBuilder` scope (codex `.keep()`s it too).
fn keep_temp(tmp: tempfile::NamedTempFile) -> Result<PathBuf, String> {
    let (_file, path) = tmp
        .keep()
        .map_err(|e| format!("could not persist the image: {e}"))?;
    Ok(path)
}

/// Raw RGBA image bytes on the clipboard (e.g. a screenshot), rebuilt into an
/// [`image::DynamicImage`].
fn clipboard_raw_image(clipboard: &mut arboard::Clipboard) -> Option<image::DynamicImage> {
    let raw = clipboard.get_image().ok()?;
    let width = u32::try_from(raw.width).ok()?;
    let height = u32::try_from(raw.height).ok()?;
    let buffer = image::RgbaImage::from_raw(width, height, raw.bytes.into_owned())?;
    Some(image::DynamicImage::ImageRgba8(buffer))
}

/// A handle that keeps a native clipboard selection alive. On Linux, arboard
/// serves the clipboard from a background thread tied to the [`arboard::Clipboard`]'s
/// lifetime, so dropping it right after `set_text` can clear what was just set —
/// the caller (the event loop) holds this for the app's lifetime. Empty on other
/// platforms, whose OS owns the clipboard once set. Codex's `ClipboardLease`. See
/// `docs/copy.md`.
pub struct ClipboardLease {
    #[cfg(target_os = "linux")]
    _clipboard: arboard::Clipboard,
}

/// Write `text` to the system clipboard for `/copy`, returning a
/// [`ClipboardLease`] to hold for the app's lifetime when one is needed
/// (Linux/arboard). Tries the **native** clipboard (arboard) first; on failure —
/// headless, no display server, an SSH host with no clipboard — falls back to an
/// **OSC 52** terminal escape, which reaches the controlling terminal (and so
/// tmux / the far end of an SSH session) where arboard can't. `Err` only when
/// both fail, naming each cause. Codex's environment-aware copy, trimmed (see
/// `docs/copy.md` for the divergences). Boundary I/O — no unit tests; the pure
/// `base64`/OSC 52 framing it calls is tested.
pub fn copy_to_clipboard(text: &str) -> Result<Option<ClipboardLease>, String> {
    match arboard_copy(text) {
        Ok(lease) => Ok(lease),
        Err(native_err) => match osc52_copy(text) {
            Ok(()) => Ok(None),
            Err(osc_err) => Err(format!("{native_err}; OSC 52 fallback: {osc_err}")),
        },
    }
}

/// Set the clipboard via arboard (the native path). On **Linux** the live
/// `Clipboard` is returned in a [`ClipboardLease`] so the X11/Wayland selection
/// survives the call; on other platforms the OS retains the text once set, so
/// `Ok(None)`. Mirrors codex's `arboard_copy`.
#[cfg(target_os = "linux")]
fn arboard_copy(text: &str) -> Result<Option<ClipboardLease>, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("could not set the clipboard: {e}"))?;
    Ok(Some(ClipboardLease {
        _clipboard: clipboard,
    }))
}

#[cfg(not(target_os = "linux"))]
fn arboard_copy(text: &str) -> Result<Option<ClipboardLease>, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("could not set the clipboard: {e}"))?;
    Ok(None)
}

/// Write the OSC 52 set-clipboard escape for `text` to the controlling terminal
/// (`/dev/tty`, falling back to stdout). The terminal — or tmux with
/// `set-clipboard` on — consumes it and updates the system clipboard. This is the
/// path that works headless / over SSH / in tmux, where arboard has no clipboard
/// to talk to. Boundary I/O.
fn osc52_copy(text: &str) -> Result<(), String> {
    let sequence = osc52_sequence(text)?;
    // Prefer /dev/tty: the escape then bypasses the TUI's buffered stdout (no
    // interleaving with a half-written frame) and still reaches the terminal if
    // stdout is redirected. Fall back to stdout when there's no controlling tty.
    if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        write_all_flush(&mut tty, &sequence)
            .map_err(|e| format!("could not write to /dev/tty: {e}"))
    } else {
        let mut stdout = std::io::stdout().lock();
        write_all_flush(&mut stdout, &sequence)
            .map_err(|e| format!("could not write to stdout: {e}"))
    }
}

/// Write the whole string and flush it (one atomic escape).
fn write_all_flush(w: &mut impl Write, s: &str) -> std::io::Result<()> {
    w.write_all(s.as_bytes())?;
    w.flush()
}

/// Maximum raw byte length we'll push through OSC 52 (codex's cap). Terminals
/// bound the escape payload, so an oversized selection is refused rather than
/// silently truncated.
const OSC52_MAX_BYTES: usize = 100_000;

/// The base64 alphabet (RFC 4648, standard `+/` variant).
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 (RFC 4648 — `+/` alphabet, `=` padding, no line wrapping) of
/// `bytes`. Hand-rolled to keep the dependency list lean: it's a few lines, pure,
/// and unit-tested against the RFC vectors. Used to encode the clipboard text for
/// the OSC 52 escape.
#[must_use]
fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64_ALPHABET[(n >> 18) & 0x3f] as char);
        out.push(BASE64_ALPHABET[(n >> 12) & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[(n >> 6) & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[n & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

/// The OSC 52 "set clipboard" escape carrying `text` (base64-encoded), or an
/// error when `text` exceeds [`OSC52_MAX_BYTES`]. The `c` selects the clipboard
/// (vs the primary selection); `\x1b]52;c;{b64}\x07` is the BEL-terminated form
/// terminals and tmux (`set-clipboard`) understand. Pure, so it's unit-tested.
/// See `docs/copy.md`.
fn osc52_sequence(text: &str) -> Result<String, String> {
    if text.len() > OSC52_MAX_BYTES {
        return Err(format!(
            "selection too large for OSC 52 ({} bytes, max {OSC52_MAX_BYTES})",
            text.len()
        ));
    }
    Ok(format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== Ctrl+V file fast path (docs/image-paste.md) =====

    #[test]
    fn accepted_image_extension_recognises_the_backend_formats() {
        use std::path::Path;
        for ok in ["a.png", "b.PNG", "c.jpg", "d.JPEG", "e.gif", "f.webp"] {
            assert!(
                accepted_image_extension(Path::new(ok)).is_some(),
                "{ok} should copy verbatim"
            );
        }
        assert_eq!(
            accepted_image_extension(Path::new("shot.PnG")).as_deref(),
            Some("png"),
            "the extension is lowercased for the temp suffix"
        );
        for bad in ["a.bmp", "b.txt", "c.tiff", "noext", "d.png.zip"] {
            assert!(
                accepted_image_extension(Path::new(bad)).is_none(),
                "{bad} must go through the decode path"
            );
        }
    }

    #[test]
    fn a_pasted_image_file_is_copied_verbatim_not_transcoded() {
        // A real (tiny) PNG on disk: the fast path must copy its exact bytes
        // to a NEW temp file — never hand back the user's own path (the
        // discard cleanup deletes what we return; see docs/image-paste.md).
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("shot.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .save(&src)
            .unwrap();
        let original = std::fs::read(&src).unwrap();

        let result = temp_image_from_files(std::slice::from_ref(&src)).expect("fast path taken");
        let path = result.expect("copy succeeds");
        assert_ne!(path, src, "a copy, not the user's file");
        assert_eq!(path.extension().unwrap(), "png", "extension preserved");
        assert_eq!(std::fs::read(&path).unwrap(), original, "bytes verbatim");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_junk_file_with_an_image_extension_is_rejected() {
        // The extension says png but the header doesn't parse: neither the
        // fast copy nor the decode path can use it, so the file list yields
        // nothing and the caller falls through to the raw-clipboard path.
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake.png");
        std::fs::write(&fake, b"not an image at all").unwrap();
        assert!(temp_image_from_files(&[fake]).is_none());
    }

    #[test]
    fn an_unlisted_extension_still_decodes_and_reencodes_to_png() {
        // PNG bytes under an extension the fast path doesn't recognise: the
        // verbatim copy is skipped, but image::open sniffs the content and
        // the fallback transcodes it to a temp PNG like before.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("shot.img");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([9, 8, 7, 255]))
            .save_with_format(&src, image::ImageFormat::Png)
            .unwrap();

        let result = temp_image_from_files(&[src]).expect("decode path taken");
        let path = result.expect("transcode succeeds");
        assert_eq!(path.extension().unwrap(), "png");
        assert!(image::open(&path).is_ok(), "the copy is a valid PNG");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn base64_encode_matches_the_rfc_vectors() {
        // RFC 4648 §10 — exercises the 0/1/2 trailing-pad cases and empty input.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_encode_handles_high_bytes() {
        // Non-ASCII bytes (e.g. a UTF-8 multibyte char) encode by their raw bytes.
        assert_eq!(base64_encode(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn osc52_sequence_frames_the_base64_in_the_set_clipboard_escape() {
        assert_eq!(osc52_sequence("foobar").unwrap(), "\x1b]52;c;Zm9vYmFy\x07");
    }

    #[test]
    fn osc52_sequence_rejects_oversized_input() {
        let big = "a".repeat(OSC52_MAX_BYTES + 1);
        assert!(osc52_sequence(&big).is_err());
    }

    #[test]
    fn osc52_sequence_allows_input_at_the_cap() {
        let at_cap = "a".repeat(OSC52_MAX_BYTES);
        assert!(osc52_sequence(&at_cap).is_ok());
    }
}
