//! A generated picture too large to attach is sent as a smaller copy, and
//! making that copy must cost about **the copy**, not the picture
//! (`docs/imagegen.md`, `docs/memory.md`).
//!
//! The copy is decoded straight from the file into its fitted size and the
//! picture's own encoding is never made when it would only be thrown away —
//! a picture-sized block per generated image is exactly what glibc's dynamic
//! `mmap` threshold keeps. What the session holds on purpose is the copy's
//! encoding, one per picture, which every later turn re-sends; the reading is
//! held to that plus a few megabytes.
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

/// A PNG that compresses about as badly as a detailed picture does, so a
/// 1536×1024 one is past the attachment cap.
fn noisy_png(width: u32, height: u32, seed: u32) -> Vec<u8> {
    let mut state = seed | 1;
    let image = image::RgbaImage::from_fn(width, height, |_, _| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        image::Rgba(state.to_le_bytes())
    });
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode");
    out.into_inner()
}

#[test]
fn fitting_a_picture_past_the_cap_costs_its_copy_not_the_picture() {
    if rss_bytes().is_none() {
        eprintln!("no /proc — skipping the resident-memory probe");
        return;
    }
    let cap = alter_zero::llm::tools::READ_IMAGE_MAX_BYTES;
    let dir = tempfile::tempdir().expect("tempdir");

    // Warm the path once, so the reading below sees the work and not
    // first-touch page faults or lazily-initialised statics.
    let warm = dir.path().join("warm.png");
    std::fs::write(&warm, noisy_png(600, 400, 7)).expect("write");
    alter_zero::images::fitted_attachment_data_url(&warm, 256 * 1024).expect("warm fit");

    // Three generations past the cap, as a session making pictures has.
    let pictures: Vec<_> = (1..=3)
        .map(|n| {
            let path = dir.path().join(format!("{n}.png"));
            let png = noisy_png(1536, 1024, n);
            assert!(png.len() > cap, "the picture must be past the cap");
            std::fs::write(&path, png).expect("write");
            path
        })
        .collect();

    let before = rss_bytes().expect("rss");
    let mut kept = 0;
    for path in &pictures {
        let fitted = alter_zero::images::fitted_attachment_data_url(path, cap).expect("a copy");
        assert!(fitted.sent.is_some(), "sent as a copy");
        kept += fitted.url.len();
    }
    let growth = rss_bytes().expect("rss").saturating_sub(before);
    assert!(
        growth < kept + 4 * 1024 * 1024,
        "fitting three pictures kept {kept} bytes of encodings but grew the resident set by \
         {growth} bytes — a picture's own bytes or encoding are being held"
    );
}
