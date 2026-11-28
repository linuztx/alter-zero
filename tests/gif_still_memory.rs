//! A GIF goes up as the still of its frames, never as itself, and drawing
//! that still must cost about **the still**, not the animation
//! (`docs/imagegen.md`, `docs/memory.md`).
//!
//! The frames are decoded one at a time, each shrunk into its cell before the
//! next is read: what is resident afterwards is the encoding the session
//! keeps for every later turn, never a frame-sized block per frame — the
//! blocks glibc's dynamic `mmap` threshold keeps once the second one is made.
//!
//! An integration test rather than a unit test because the measurement is
//! process-wide: a lone `#[test]` in its own binary has nothing running beside
//! it to pollute the reading (`tests/model_parse_memory.rs`'s pattern). It
//! skips where `/proc` isn't available.

/// The process's resident set in bytes — `None` where `/proc` isn't available.
fn rss_bytes() -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = status.lines().find_map(|l| l.strip_prefix("VmRSS:"))?;
    let kb: usize = field.trim().trim_end_matches(" kB").trim().parse().ok()?;
    Some(kb * 1024)
}

/// A looping GIF of `frames` noisy `side`×`side` frames — a large animated GIF,
/// as a `read` or a paste brings one in.
fn noisy_gif(side: u32, frames: u32, seed: u32) -> Vec<u8> {
    let mut state = seed | 1;
    let mut out = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(&mut out, 30);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Infinite)
            .expect("repeat");
        for _ in 0..frames {
            let frame = image::RgbaImage::from_fn(side, side, |_, _| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let [r, g, b, _] = state.to_le_bytes();
                image::Rgba([r, g, b, 255])
            });
            encoder
                .encode_frame(image::Frame::new(frame))
                .expect("encode");
        }
    }
    out
}

#[test]
fn drawing_a_gifs_still_costs_the_still_not_its_frames() {
    if rss_bytes().is_none() {
        eprintln!("no /proc — skipping the resident-memory probe");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");

    // Warm the path once, so the reading below sees the work and not
    // first-touch page faults or lazily-initialised statics.
    let warm = dir.path().join("warm.gif");
    std::fs::write(&warm, noisy_gif(256, 3, 7)).expect("write");
    alter_zero::images::attachment_data_url(&warm).expect("warm still");

    // Three GIFs of nine 1024×1024 frames: 36 MB of frames each.
    let gifs: Vec<_> = (1..=3)
        .map(|n| {
            let path = dir.path().join(format!("{n}.gif"));
            std::fs::write(&path, noisy_gif(1024, 9, n)).expect("write");
            path
        })
        .collect();

    let before = rss_bytes().expect("rss");
    let mut kept = 0;
    for path in &gifs {
        let url = alter_zero::images::attachment_data_url(path).expect("a still");
        assert!(url.starts_with("data:image/png;base64,"), "never the GIF");
        kept += url.len();
    }
    let growth = rss_bytes().expect("rss").saturating_sub(before);
    assert!(
        growth < kept + 4 * 1024 * 1024,
        "drawing three stills kept {kept} bytes of encodings but grew the resident set by \
         {growth} bytes — the frames are being held"
    );
}
