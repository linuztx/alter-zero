//! A pasted screenshot's memory cost must stay proportional to what is
//! **produced**, never to the picture's own pixel count (`docs/memory.md`).
//!
//! The three places a Ctrl+V paste touches pixels — the paste worker's saved
//! copy, the picture drawn under its bubble, and the payload sent to the
//! model — each used to decode the whole screenshot (four bytes a pixel:
//! 8 MB for 1080p, 15 MB for 1440p) and shrink it from there, all of it
//! transient heap that glibc's dynamic `mmap` threshold then kept: three
//! pasted screenshots measured a 103 MB process. The fixes stream — the
//! clipboard's own PNG bytes go to disk through a buffer, and a PNG is
//! area-averaged straight into the cell box (or the payload size) a row at a
//! time — and this test is what keeps them streaming.
//!
//! An integration test rather than a unit test because the measurement is
//! process-wide: a lone `#[test]` in its own binary has nothing running beside
//! it to pollute the reading (`tests/model_parse_memory.rs`'s pattern). It
//! skips where `/proc` isn't available.

use std::io::Cursor;

/// The process's resident set in bytes — `None` where `/proc` isn't available.
fn rss_bytes() -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = status.lines().find_map(|l| l.strip_prefix("VmRSS:"))?;
    let kb: usize = field.trim().trim_end_matches(" kB").trim().parse().ok()?;
    Some(kb * 1024)
}

/// A screenshot-shaped PNG: a gradient with noise in the low bits, so it
/// neither collapses to kilobytes nor bloats to raw size.
fn screenshot_png(width: u32, height: u32) -> Vec<u8> {
    let mut seed: u32 = 0x9E37_79B9;
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (seed >> 24) as u8 & 0x0f;
        image::Rgba([
            ((x * 255) / width) as u8 ^ noise,
            ((y * 255) / height) as u8 ^ noise,
            (((x + y) * 255) / (width + height)) as u8,
            255,
        ])
    });
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode");
    out.into_inner()
}

#[test]
fn shrinking_a_pasted_screenshot_never_materialises_its_pixels() {
    if rss_bytes().is_none() {
        eprintln!("no /proc — skipping the resident-memory probe");
        return;
    }
    let (width, height) = (2560u32, 1440u32);
    let source_rgba = width as usize * height as usize * 4;

    // Warm every path once on a small picture, so the readings below see the
    // work and not first-touch page faults or lazily-initialised statics.
    let warm = screenshot_png(64, 36);
    let dir = tempfile::tempdir().expect("tempdir");
    let warm_path = dir.path().join("warm.png");
    std::fs::write(&warm_path, &warm).expect("write");
    drop(alter_zero::images::decode_png_fitted(
        Cursor::new(&warm[..]),
        |px| alter_zero::images::fit_box(px, (16, 16)),
    ));
    drop(alter_zero::images::load_fitted(
        warm_path.to_str().expect("utf-8 path"),
        (16, 16),
    ));
    drop(
        alter_zero::clipboard::stream_encoded_image_into(dir.path(), &mut &warm[..], "png")
            .map(std::fs::remove_file),
    );

    let png = screenshot_png(width, height);
    let path = dir.path().join("shot.png");
    std::fs::write(&path, &png).expect("write");

    // 1. The paste worker's streamed path: the clipboard's bytes copied into
    //    the paste folder. Nothing about the picture is decoded at all.
    let before = rss_bytes().expect("rss");
    let saved = alter_zero::clipboard::stream_encoded_image_into(dir.path(), &mut &png[..], "png")
        .expect("streams");
    let growth = rss_bytes().expect("rss").saturating_sub(before);
    let _ = std::fs::remove_file(&saved);
    assert!(
        growth < 2 * 1024 * 1024,
        "streaming the clipboard's {}-byte PNG to disk grew the resident set by {growth} \
         bytes — the picture is being decoded or buffered whole",
        png.len()
    );

    // 2. The display decode: the picture fitted into a 1200x700-pixel cell box
    //    (120 columns at a 10x20 cell). Budget: the fitted output plus a
    //    generous allowance for the decoder's row buffers — an order of
    //    magnitude under the source's own RGBA.
    let before = rss_bytes().expect("rss");
    let fitted = alter_zero::images::load_fitted(path.to_str().expect("utf-8 path"), (1200, 700))
        .expect("fits");
    let growth = rss_bytes().expect("rss").saturating_sub(before);
    let output = fitted.width() as usize * fitted.height() as usize * 4;
    drop(fitted);
    assert!(
        growth < output + 4 * 1024 * 1024,
        "fitting a {width}x{height} PNG into 1200x700 grew the resident set by {growth} \
         bytes against a {source_rgba}-byte source — the whole picture is being decoded"
    );

    // 3. The model payload: the same file shrunk to 2000 px for the request.
    //    Budget: the payload's own pixels, its encoded bytes, and a copy's
    //    worth of slack for the encoder — still well under the source RGBA
    //    (15 MB) this replaced.
    let before = rss_bytes().expect("rss");
    let small =
        alter_zero::images::downscale_to(&png, image::ImageFormat::Png, 2000).expect("shrunk");
    let growth = rss_bytes().expect("rss").saturating_sub(before);
    let payload_px = small.size.0 as usize * small.size.1 as usize * 4;
    let budget = payload_px + 2 * small.bytes.len() + 4 * 1024 * 1024;
    assert_eq!(small.size, (2000, 1125));
    drop(small);
    assert!(
        growth < budget,
        "downscaling a {width}x{height} PNG to 2000 px grew the resident set by {growth} \
         bytes (budget {budget}) — the source is being decoded or resampled whole"
    );
    assert!(
        growth < source_rgba,
        "the payload build must cost less than the source's own RGBA ({source_rgba} bytes)"
    );
}
