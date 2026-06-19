//! Clipboard image read — the I/O boundary for Ctrl+V image paste.
//!
//! A focused port of codex's `clipboard_paste.rs`: pull an image off the system
//! clipboard (raw bytes, or a copied image *file*), re-encode it to one
//! consistent PNG, and write it to a kept temp file whose path the backend reads
//! (the TUI never base64-encodes it — codex parity). This is **boundary** code —
//! it touches the real clipboard and filesystem — so, like `term.rs`, it carries
//! no unit tests; the pure attach/placeholder logic it feeds lives in `App`
//! ([`crate::app::App::attach_image`]). The happy path needs a real clipboard
//! server, so it is exercised by hand / on a desktop; `scripts/smoke.sh` covers
//! only the no-clipboard error path (headless CI has nothing to read). See
//! `docs/image-paste.md`.

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
