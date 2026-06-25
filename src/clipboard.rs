//! Clipboard I/O — the boundary for Ctrl+V image paste (read) and `/copy`
//! (write).
//!
//! The **read** side is a focused port of codex's `clipboard_paste.rs`: pull an
//! image off the system clipboard (raw bytes, or a copied image *file*),
//! re-encode it to one consistent PNG, and write it to a kept temp file whose
//! path the backend reads (the TUI never base64-encodes it — codex parity). The
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
/// written temporary PNG (kept on disk for the backend to read by path).
///
/// On any failure — no clipboard server (headless / unsupported), nothing
/// image-shaped on the clipboard, or an encode/write error — returns a short
/// human-readable message for the red `Failed to paste image: {msg}` notice the
/// loop commits (codex's `new_error_event`).
///
/// Mirrors codex: prefer an image *file* on the clipboard (e.g. one copied in a
/// GUI file manager) and decode it from disk; otherwise take the raw RGBA image
/// bytes (e.g. a screenshot). Either source is re-encoded to PNG.
pub fn read_clipboard_image() -> Result<PathBuf, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;

    let image = clipboard_file_image(&mut clipboard)
        .or_else(|| clipboard_raw_image(&mut clipboard))
        .ok_or_else(|| "no image on the clipboard".to_string())?;

    // Re-encode to PNG so the on-disk format is always the same, whatever the
    // source codec was.
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
    // Keep the file past this scope — the backend reads it by path, so it must
    // outlive the `TempfileBuilder` guard (codex `.keep()`s it too).
    let (_file, path) = tmp
        .keep()
        .map_err(|e| format!("could not persist the image: {e}"))?;
    Ok(path)
}

/// The first decodable image among any files on the clipboard — codex's
/// `file_list()` path, for an image *file* copied in a GUI file manager.
fn clipboard_file_image(clipboard: &mut arboard::Clipboard) -> Option<image::DynamicImage> {
    clipboard
        .get()
        .file_list()
        .ok()?
        .into_iter()
        .find_map(|path| image::open(path).ok())
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
