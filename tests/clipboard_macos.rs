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
//! It exists because the failure it pins cannot be reached headless: a
//! screenshot copied to the clipboard (⌃⇧⌘4) is a **16-bit floating-point**
//! TIFF, `image`'s TIFF decoder refuses that shape, so arboard's own read
//! answers `ConversionFailure` and the paste used to report "no image on the
//! clipboard" for a perfectly good screenshot (`docs/image-paste.md`,
//! `src/clipboard/macos.rs`).
//!
//! Every picture these tests plant is built here in exactly that shape
//! ([`half_float_tiff`]): arboard refuses it on any Mac, so a paste of it can
//! only succeed through the AppKit fallback. An 8-bit picture, or one whose
//! depth the machine's display decides, would let arboard read it itself and
//! the paste pass without the fallback ever running.
//!
//! A test that cannot reach the pasteboard skips with a note — except on CI
//! (`CI` set), where a skip would pass having tested nothing, so it fails.

#![cfg(target_os = "macos")]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use alter_zero::clipboard::{NO_IMAGE_ON_CLIPBOARD, read_clipboard_image};

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

/// Note a test that cannot reach the pasteboard as skipped — or, on CI, fail
/// it: a skip there would pass having tested nothing.
fn skip(why: &str) {
    assert!(
        std::env::var_os("CI").is_none(),
        "{why}: CI must reach the pasteboard, or this suite tests nothing"
    );
    eprintln!("skipped: {why}");
}

/// Run `script` under `osascript` with `arg` as its `argv`, never
/// interpolated into the script, and say whether it succeeded.
fn osascript(script: &str, arg: impl AsRef<OsStr>) -> bool {
    Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .arg(arg)
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Puts the file `argv` names on the pasteboard as `public.tiff`,
/// **unconverted**: its `NSData` goes straight to `setData:forType:`, so the
/// pasteboard holds exactly the fixture's bytes.
const PUT_TIFF: &str = "use framework \"AppKit\"\n\
     on run argv\n\
     set d to current application's NSData's dataWithContentsOfFile:(item 1 of argv)\n\
     if d is missing value then error \"the fixture could not be read\"\n\
     set pb to current application's NSPasteboard's generalPasteboard()\n\
     pb's clearContents()\n\
     set ok to pb's setData:d forType:(current application's NSPasteboardTypeTIFF)\n\
     if not (ok as boolean) then error \"the pasteboard refused the fixture\"\n\
     end run";

/// AppKit's own conversion of the pasteboard's picture — the coercion the
/// paste runs — written whole to the path `argv` names: what the paste would
/// store if it kept everything AppKit hands over.
const APPKIT_PNG: &str = "on run argv\n\
     set outPath to item 1 of argv\n\
     set png_data to (the clipboard as «class PNGf»)\n\
     set fp to open for access POSIX file outPath with write permission\n\
     set eof fp to 0\n\
     write png_data to fp\n\
     close access fp\n\
     end run";

/// Put `tiff` on the pasteboard, exactly as given. `false` when no GUI
/// session can be reached.
fn put_tiff(tiff: &[u8]) -> bool {
    let fixture = tempfile::NamedTempFile::new().expect("a temp file");
    std::fs::write(fixture.path(), tiff).expect("write the fixture");
    osascript(PUT_TIFF, fixture.path())
}

/// The TIFF tags a pasteboard picture's metadata rides in.
const IMAGE_DESCRIPTION: u16 = 270;
const SOFTWARE: u16 = 305;
const ARTIST: u16 = 315;

/// An 8×8 baseline TIFF of **16-bit floating-point** RGBA samples
/// (`BitsPerSample 16,16,16,16`, `SampleFormat 3`) — the shape a screenshot
/// copied to the clipboard takes, and the one `image`'s TIFF decoder refuses —
/// carrying each of `tags` as an ASCII field. Built by hand, so the test
/// depends neither on AppKit writing one nor on the display deciding its
/// depth.
fn half_float_tiff(tags: &[(u16, &str)]) -> Vec<u8> {
    const WIDTH: u32 = 8;
    const HEIGHT: u32 = 8;
    const SHORT: u16 = 3;
    const LONG: u16 = 4;
    const ASCII: u16 = 2;
    const STRIP_OFFSETS: u16 = 273;
    // Opaque teal as IEEE half floats (0.0, 0.5, 0.5, 1.0), little-endian.
    let pixels: Vec<u8> = (0..WIDTH * HEIGHT)
        .flat_map(|_| [0x0000_u16, 0x3800, 0x3800, 0x3C00])
        .flat_map(u16::to_le_bytes)
        .collect();
    let shorts =
        |values: &[u16]| -> Vec<u8> { values.iter().flat_map(|v| v.to_le_bytes()).collect() };
    // (tag, type, count, value); the strip offset is patched in once the
    // layout is known.
    let mut fields: Vec<(u16, u16, u32, Vec<u8>)> = vec![
        (256, LONG, 1, WIDTH.to_le_bytes().to_vec()),
        (257, LONG, 1, HEIGHT.to_le_bytes().to_vec()),
        (258, SHORT, 4, shorts(&[16; 4])),
        (259, SHORT, 1, shorts(&[1])),
        (262, SHORT, 1, shorts(&[2])),
        (STRIP_OFFSETS, LONG, 1, vec![0; 4]),
        (277, SHORT, 1, shorts(&[4])),
        (278, LONG, 1, HEIGHT.to_le_bytes().to_vec()),
        (279, LONG, 1, (pixels.len() as u32).to_le_bytes().to_vec()),
        (284, SHORT, 1, shorts(&[1])),
        (338, SHORT, 1, shorts(&[2])),
        (339, SHORT, 4, shorts(&[3; 4])),
    ];
    for (tag, text) in tags {
        let mut value = text.as_bytes().to_vec();
        value.push(0);
        fields.push((*tag, ASCII, value.len() as u32, value));
    }
    fields.sort_by_key(|&(tag, ..)| tag);
    // Values over four bytes live after the directory, word-aligned, and the
    // pixels after them.
    let directory_end = 8 + 2 + 12 * fields.len() as u32 + 4;
    let mut values = Vec::new();
    let mut entries = Vec::new();
    for (tag, kind, count, value) in &fields {
        let inline = if value.len() > 4 {
            let offset = directory_end + values.len() as u32;
            values.extend_from_slice(value);
            if values.len() % 2 == 1 {
                values.push(0);
            }
            offset.to_le_bytes().to_vec()
        } else {
            let mut inline = value.clone();
            inline.resize(4, 0);
            inline
        };
        entries.push((*tag, *kind, *count, inline));
    }
    let pixel_offset = directory_end + values.len() as u32;
    let mut out = b"II*\0".to_vec();
    out.extend_from_slice(&8_u32.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, kind, count, inline) in entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        if tag == STRIP_OFFSETS {
            out.extend_from_slice(&pixel_offset.to_le_bytes());
        } else {
            out.extend_from_slice(&inline);
        }
    }
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&values);
    out.extend_from_slice(&pixels);
    out
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[test]
#[ignore = "needs a real macOS pasteboard; overwrites the user's clipboard"]
fn a_screenshot_shaped_tiff_pastes_through_the_appkit_fallback() {
    if !put_tiff(&half_float_tiff(&[])) {
        skip("no GUI session to put a picture on the pasteboard");
        return;
    }
    let store = Store::new("tiff16");
    let path = read_clipboard_image(store.path())
        .unwrap_or_else(|reason| panic!("a 16-bit floating-point TIFF must paste, got: {reason}"));
    assert_eq!(path, store.path().join("1.png"));
    assert_eq!(
        image::image_dimensions(&path).expect("the stored file is a real picture"),
        (8, 8)
    );
    assert_eq!(store.names(), ["1.png"], "no staging file is left behind");
}

#[test]
#[ignore = "needs a real macOS pasteboard; overwrites the user's clipboard"]
fn a_pasted_pictures_metadata_never_reaches_the_file() {
    // AppKit's `«class PNGf»` coercion copies the pasteboard picture's tags:
    // a TIFF's ImageDescription/Artist/Software land in the PNG's `eXIf`
    // chunk and again in an `iTXt` XMP packet. That file is base64'd into
    // every later request (`docs/context.md`), so the paste must keep the
    // pixels and drop everything else.
    const TAGS: [(u16, &str); 3] = [
        (IMAGE_DESCRIPTION, "SECRET-DESCRIPTION-alice"),
        (ARTIST, "SECRET-ARTIST-bob"),
        (SOFTWARE, "SECRET-SOFTWARE-mac"),
    ];
    if !put_tiff(&half_float_tiff(&TAGS)) {
        skip("no GUI session to put a picture on the pasteboard");
        return;
    }
    // The control: AppKit's own conversion of the same pasteboard, kept
    // whole. Unless it still carries the tags, this test could not tell the
    // paste's strip from a conversion that dropped them by itself.
    let control = tempfile::NamedTempFile::new().expect("a temp file");
    assert!(
        osascript(APPKIT_PNG, control.path()),
        "AppKit converts the planted picture"
    );
    let converted = std::fs::read(control.path()).expect("AppKit's conversion");
    assert!(
        TAGS.iter().any(|&(_, text)| contains(&converted, text)),
        "AppKit's conversion no longer carries the fixture's tags, so this test proves nothing"
    );

    let store = Store::new("secrets");
    let path = read_clipboard_image(store.path())
        .unwrap_or_else(|reason| panic!("the picture must paste, got: {reason}"));
    let bytes = std::fs::read(&path).expect("the pasted file is readable");
    let leaks = TAGS.iter().map(|&(_, text)| text);
    for leak in leaks.chain(["eXIf", "iTXt", "xmpmeta"]) {
        assert!(
            !contains(&bytes, leak),
            "{leak:?} survived into {} — the paste would re-send it on every turn",
            path.display()
        );
    }
    // And the picture itself is intact, not blanked by the strip.
    assert_eq!(
        image::image_dimensions(&path).expect("a real picture"),
        (8, 8)
    );
    assert_eq!(store.names(), ["1.png"], "no staging file is left behind");
}

#[test]
#[ignore = "needs a real macOS pasteboard; overwrites the user's clipboard"]
fn a_text_clipboard_says_what_is_missing() {
    let set_text = "on run argv\nset the clipboard to (item 1 of argv)\nend run";
    if !osascript(set_text, "not a picture") {
        skip("no GUI session to reach the pasteboard");
        return;
    }
    let store = Store::new("text");
    let reason = read_clipboard_image(store.path()).expect_err("a text clipboard holds no picture");
    assert_eq!(reason, NO_IMAGE_ON_CLIPBOARD);
    assert!(
        store.names().is_empty(),
        "a refused paste leaves nothing behind"
    );
}
