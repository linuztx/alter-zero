//! The Ctrl+V image read on Linux must **stream** the clipboard's `image/png`
//! bytes to the temp file — never decode them to RGBA and re-encode
//! (`docs/image-paste.md`). That round trip cost tens of megabytes per paste
//! to produce the bytes the clipboard owner had already handed over.
//!
//! Needs an X server, so it is ignored by default; the offline smoke suite
//! covers the no-clipboard failure. Under a virtual one:
//!
//! ```text
//! Xvfb :99 & DISPLAY=:99 cargo test --test clipboard_linux -- --ignored
//! ```
//!
//! The owner is the crate's own test-support selection server, which serves
//! the picture both ways a real owner can — whole, and in `INCR` segments —
//! so both branches of the read are exercised against a real X server.
#![cfg(target_os = "linux")]

#[path = "support/x11_owner.rs"]
mod owner;

use std::sync::Arc;

use owner::Owner;

/// A screenshot-shaped PNG whose bytes a decode-and-re-encode could never
/// reproduce: encoded at a non-default compression level and carrying a text
/// chunk, both of which a re-encode drops. Verbatim equality is therefore
/// proof the bytes were streamed, not rebuilt.
fn served_png(w: u32, h: u32) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    let mut seed: u32 = 0x9E37_79B9;
    for y in 0..h {
        for x in 0..w {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed & 0x1F) as u8;
            let fx = (x * 255 / w.max(1)) as u8;
            let fy = (y * 255 / h.max(1)) as u8;
            rgba.extend_from_slice(&[
                fx.saturating_add(noise),
                fy.saturating_add(noise),
                200u8.saturating_sub(fx).saturating_add(noise),
                255,
            ]);
        }
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder
            .add_text_chunk("Comment".to_string(), "served verbatim".to_string())
            .expect("a text chunk");
        let mut writer = encoder.write_header().expect("a PNG header");
        writer.write_image_data(&rgba).expect("the picture");
    }
    out
}

fn has_display() -> bool {
    if std::env::var_os("DISPLAY").is_some() {
        return true;
    }
    eprintln!("no DISPLAY — skipping the X11 clipboard probe");
    false
}

#[test]
#[ignore = "needs an X server (DISPLAY) — run under Xvfb"]
fn a_png_on_the_clipboard_streams_to_the_temp_file_verbatim() {
    if !has_display() {
        return;
    }
    let png = Arc::new(served_png(1920, 1080));
    // Whole in one property, then in INCR segments the way GTK and Qt hand
    // over anything larger than a few hundred kilobytes.
    for (label, chunk) in [("whole", None), ("INCR", Some(200_000))] {
        let owner = Owner::serve(Some(Arc::clone(&png)), chunk).expect("own the selection");
        let path = alter_zero::clipboard::read_clipboard_image()
            .unwrap_or_else(|e| panic!("{label}: paste failed: {e}"));
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(
            name.starts_with("alter-zero-clipboard-") && name.ends_with(".png"),
            "{label}: our own temp copy, named as the paste names it: {name}"
        );
        let copied = std::fs::read(&path).expect("read the temp file");
        assert!(
            copied == *png,
            "{label}: the temp file must hold the served bytes verbatim \
             ({} bytes served, {} written)",
            png.len(),
            copied.len()
        );
        let _ = std::fs::remove_file(&path);
        drop(owner);
    }
}

#[test]
#[ignore = "needs an X server (DISPLAY) — run under Xvfb"]
fn a_clipboard_with_no_picture_is_reported_as_before() {
    if !has_display() {
        return;
    }
    // An owner that offers no image/png at all: the direct read finds nothing
    // and the paste fails with the message the red notice always carried.
    let _owner = Owner::serve(None, None).expect("own the selection");
    let err = alter_zero::clipboard::read_clipboard_image().expect_err("nothing to paste");
    assert_eq!(err, "no image on the clipboard");
}
