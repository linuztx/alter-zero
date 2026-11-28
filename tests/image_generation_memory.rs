//! A generated image's answer must cost **kilobytes** to save, whatever the
//! image weighs (`docs/imagegen.md`, `docs/memory.md`).
//!
//! The Codex backend answers an image request with one JSON object whose
//! `b64_json` string is the picture — twenty-odd megabytes of text for a 4K
//! PNG. Read the ordinary way that is the body, a string and the decoded
//! bytes: three picture-sized blocks on the worker's arena, which glibc's
//! dynamic `mmap` threshold then keeps. The answer is scanned instead, the
//! base64 decoded straight into the file a buffer at a time, and this test is
//! what keeps it that way.
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

/// A generated-image-shaped PNG: a gradient with noise in the low bits, so it
/// neither collapses to kilobytes nor bloats to raw size.
fn picture_png(width: u32, height: u32) -> Vec<u8> {
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

/// The answer the backend sends for `png`: Codex's shape, base64 and all.
fn answer(png: &[u8]) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(png.len() / 3 * 4 + 256);
    out.extend_from_slice(
        br#"{"created": 1778832973, "background": "opaque", "data": [{"b64_json": ""#,
    );
    for chunk in png.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63]);
        out.push(ALPHABET[(n >> 12) as usize & 63]);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63]
        } else {
            b'='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63]
        } else {
            b'='
        });
    }
    out.extend_from_slice(
        br#"", "revised_prompt": "a gradient"}], "output_format": "png", "size": "3840x2160"}"#,
    );
    out
}

#[test]
fn saving_a_generated_image_never_holds_its_base64_or_its_bytes() {
    if rss_bytes().is_none() {
        eprintln!("no /proc — skipping the resident-memory probe");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");

    // Warm the path once on a small picture, so the reading below sees the
    // work and not first-touch page faults or lazily-initialised statics.
    let warm = answer(&picture_png(64, 36));
    let saved = alter_zero::llm::imagegen::save_generated_answer(dir.path(), &mut &warm[..])
        .expect("warm save");
    std::fs::remove_file(saved).expect("remove");

    // A 4K output, the largest an Images 2.6 still comes in.
    let png = picture_png(3840, 2160);
    let body = answer(&png);

    let before = rss_bytes().expect("rss");
    let saved = alter_zero::llm::imagegen::save_generated_answer(dir.path(), &mut &body[..])
        .expect("saves");
    let growth = rss_bytes().expect("rss").saturating_sub(before);
    assert_eq!(
        std::fs::read(&saved).expect("read back"),
        png,
        "the file is the image"
    );
    assert!(
        growth < 2 * 1024 * 1024,
        "saving a {}-byte answer ({}-byte PNG) grew the resident set by {growth} bytes — \
         the answer, its base64 or the image is being held whole",
        body.len(),
        png.len()
    );
}
