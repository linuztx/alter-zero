//! The macOS clipboard read, against the **real** pasteboard.
//!
//! Ignored by default like `tests/clipboard_linux.rs`: it needs a GUI session
//! (an ssh login or a launchd daemon has no pasteboard), and it overwrites the
//! user's clipboard while it runs. Run it on a Mac with:
//!
//! ```text
//! cargo test --test clipboard_macos -- --ignored --test-threads=1
//! ```
//!
//! It exists because the failure it pins cannot be reached headless: a macOS
//! screenshot is a **16-bit** TIFF, and `image`'s TIFF decoder refuses that
//! shape, so arboard's own read answers `ConversionFailure` and the paste used
//! to report "no image on the clipboard" for a perfectly good screenshot
//! (`docs/image-paste.md`, `src/clipboard/macos.rs`).

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;

use alter_zero::clipboard::read_clipboard_image;

/// A scratch paste folder, removed on drop.
struct Store(PathBuf);

impl Store {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("alter-zero-macos-clip-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        Some(entry.ok()?.file_name().to_string_lossy().into_owned())
                    })
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Build a TIFF whose tags carry recognizable secrets, the way a photo's GPS,
/// camera and `Artist` tags ride a pasteboard picture. Written as a bare
/// baseline TIFF so the test does not depend on AppKit producing one.
fn tiff_with_secrets() -> Vec<u8> {
    const SECRETS: [&[u8]; 3] = [
        b"SECRET-DESCRIPTION-alice",
        b"SECRET-ARTIST-bob",
        b"SECRET-SOFTWARE-mac",
    ];
    const W: u16 = 8;
    const H: u16 = 8;
    let pixels = vec![200_u8; W as usize * H as usize * 3];

    // Tag layout: the nine baseline tags, then three ASCII tags whose values
    // live past the pixel data.
    let entries = 9 + SECRETS.len();
    let ifd_offset = 8_u32;
    let ifd_size = 2 + entries as u32 * 12 + 4;
    let bits_offset = ifd_offset + ifd_size;
    let data_offset = bits_offset + 6; // three SHORTs

    let mut out = Vec::new();
    out.extend_from_slice(b"II*\0");
    out.extend_from_slice(&ifd_offset.to_le_bytes());
    out.extend_from_slice(&(entries as u16).to_le_bytes());
    let field = |out: &mut Vec<u8>, tag: u16, ty: u16, count: u32, value: u32| {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    };
    field(&mut out, 256, 3, 1, W.into());
    field(&mut out, 257, 3, 1, H.into());
    field(&mut out, 258, 3, 3, bits_offset);
    field(&mut out, 259, 3, 1, 1);
    field(&mut out, 262, 3, 1, 2);
    field(&mut out, 273, 4, 1, data_offset);
    field(&mut out, 277, 3, 1, 3);
    field(&mut out, 278, 3, 1, H.into());
    field(&mut out, 279, 4, 1, u32::try_from(pixels.len()).unwrap());
    let mut cursor = data_offset + u32::try_from(pixels.len()).unwrap();
    for (tag, secret) in [270_u16, 315, 305].iter().zip(SECRETS) {
        field(
            &mut out,
            *tag,
            2,
            u32::try_from(secret.len() + 1).unwrap(),
            cursor,
        );
        cursor += u32::try_from(secret.len() + 1).unwrap();
    }
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&8_u16.to_le_bytes());
    out.extend_from_slice(&8_u16.to_le_bytes());
    out.extend_from_slice(&8_u16.to_le_bytes());
    out.extend_from_slice(&pixels);
    for secret in SECRETS {
        out.extend_from_slice(secret);
        out.push(0);
    }
    out
}

/// Put raw TIFF bytes on the pasteboard as `public.tiff`. `false` when no GUI
/// session can be reached.
fn put_tiff(bytes: &[u8]) -> bool {
    let path = std::env::temp_dir().join("alter-zero-macos-clip-secret.tiff");
    if std::fs::write(&path, bytes).is_err() {
        return false;
    }
    Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(format!(
            "set d to (read (POSIX file \"{}\") as «class TIFF»)\n\
             set pb to current application's NSPasteboard's generalPasteboard()\n\
             pb's clearContents()\n\
             pb's setData:d forType:(current application's NSPasteboardTypeTIFF)",
            path.display()
        ))
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Run an AppleScript that puts a picture of `kind` on the pasteboard.
/// `None` when this machine cannot run it (no GUI session, or a denied
/// automation permission) — the test then skips rather than fails, since the
/// pasteboard is not reachable at all.
fn put_picture(kind: &str) -> bool {
    let script = match kind {
        // The shape that used to fail: `NSImage.tiffRepresentation` is 16 bits
        // per channel, which is exactly what a ⇧⌘4 screenshot plants.
        "tiff16" => {
            "use framework \"AppKit\"\n\
             set img to current application's NSImage's alloc()'s initWithSize:{width:120, height:80}\n\
             img's lockFocus()\n\
             current application's NSColor's systemTealColor()'s setFill()\n\
             current application's NSBezierPath's fillRect:{origin:{x:0, y:0}, |size|:{width:120, height:80}}\n\
             img's unlockFocus()\n\
             set d to img's TIFFRepresentation()\n\
             set pb to current application's NSPasteboard's generalPasteboard()\n\
             pb's clearContents()\n\
             pb's setData:d forType:(current application's NSPasteboardTypeTIFF)"
        }
        _ => return false,
    };
    Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

#[test]
#[ignore = "needs a real macOS pasteboard; overwrites the user's clipboard"]
fn a_sixteen_bit_tiff_screenshot_pastes() {
    if !put_picture("tiff16") {
        eprintln!("skipped: no GUI session to put a picture on the pasteboard");
        return;
    }
    let store = Store::new("tiff16");
    let path = match read_clipboard_image(store.path()) {
        Ok(path) => path,
        Err(reason) => panic!("a 16-bit TIFF screenshot must paste, got: {reason}"),
    };
    assert_eq!(path, store.path().join("1.png"));
    let (width, height) =
        image::image_dimensions(&path).expect("the stored file is a real picture");
    assert!(width > 0 && height > 0, "{width}x{height}");
    assert_eq!(store.names(), ["1.png"], "no staging file is left behind");
}

#[test]
#[ignore = "needs a real macOS pasteboard; overwrites the user's clipboard"]
fn a_pasted_pictures_metadata_never_reaches_the_file() {
    // AppKit's `«class PNGf»` coercion copies the pasteboard picture's tags:
    // a TIFF's ImageDescription/Artist/Software land in the PNG's `eXIf`
    // chunk and again in an `iTXt` XMP packet. That file is base64'd into
    // every later request (`docs/context.md`), so the paste must keep the
    // pixels and drop everything else (`strip_metadata`).
    if !put_tiff(&tiff_with_secrets()) {
        eprintln!("skipped: no GUI session to put a picture on the pasteboard");
        return;
    }
    let store = Store::new("secrets");
    let path = match read_clipboard_image(store.path()) {
        Ok(path) => path,
        Err(reason) => panic!("the picture must paste, got: {reason}"),
    };
    let bytes = std::fs::read(&path).expect("the pasted file is readable");
    let text = String::from_utf8_lossy(&bytes);
    for leak in [
        "SECRET-DESCRIPTION-alice",
        "SECRET-ARTIST-bob",
        "SECRET-SOFTWARE-mac",
        "xmpmeta",
        "eXIf",
    ] {
        assert!(
            !text.contains(leak),
            "{leak:?} survived into {} — the paste would re-send it on every turn",
            path.display()
        );
    }
    // And the picture itself is intact, not blanked by the strip.
    let (width, height) = image::image_dimensions(&path).expect("a real picture");
    assert_eq!((width, height), (8, 8));
}

#[test]
#[ignore = "needs a real macOS pasteboard; overwrites the user's clipboard"]
fn a_text_clipboard_says_what_is_missing() {
    let ok = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg("set the clipboard to \"not a picture\"")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("skipped: no GUI session to reach the pasteboard");
        return;
    }
    let store = Store::new("text");
    let reason = read_clipboard_image(store.path()).expect_err("a text clipboard holds no picture");
    assert!(reason.contains("no image on the clipboard"), "{reason}");
    assert!(
        reason.contains("read its path"),
        "the refusal names the way around it: {reason}"
    );
    assert!(
        store.names().is_empty(),
        "a refused paste leaves nothing behind"
    );
}
