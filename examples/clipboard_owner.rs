//! Put a synthetic screenshot-shaped picture on the X11 clipboard and keep
//! serving it, for driving the Ctrl+V paste by hand or under a virtual X
//! server (`docs/image-paste.md`, *Measuring*):
//!
//! ```text
//! Xvfb :99 &
//! DISPLAY=:99 cargo run --release --example clipboard_owner -- 1920 1080 &
//! DISPLAY=:99 cargo run            # then Ctrl+V in the composer
//! ```
//!
//! The picture is a gradient with per-pixel noise, so it PNG-compresses like a
//! real screenshot does (a flat fill would shrink to a few kilobytes and hide
//! every cost this exists to expose), and it is served the way GTK and Qt
//! serve anything large — as `image/png`, in `INCR` segments — by the same
//! selection owner `tests/clipboard_linux.rs` drives the read against.

#[cfg(target_os = "linux")]
#[path = "../tests/support/x11_owner.rs"]
mod owner;

#[cfg(target_os = "linux")]
fn main() {
    let mut args = std::env::args().skip(1);
    let width: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(1920);
    let height: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(1080);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    // A cheap deterministic hash stands in for a random source, so the noise
    // needs no dependency and every run serves the same picture.
    let mut seed: u32 = 0x9E37_79B9;
    for y in 0..height {
        for x in 0..width {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed & 0x1F) as u8;
            let fx = (x * 255 / width.max(1)) as u8;
            let fy = (y * 255 / height.max(1)) as u8;
            rgba.extend_from_slice(&[
                fx.saturating_add(noise),
                fy.saturating_add(noise),
                200u8.saturating_sub(fx).saturating_add(noise),
                255,
            ]);
        }
    }
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().expect("a PNG header");
        writer.write_image_data(&rgba).expect("the picture");
    }
    drop(rgba);
    let bytes = png.len();
    let _owner = owner::Owner::serve(Some(std::sync::Arc::new(png)), Some(256 * 1024))
        .expect("own the clipboard (is DISPLAY set?)");
    println!("serving a {width}x{height} PNG ({bytes} bytes) on the clipboard — Ctrl+C to stop");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("clipboard_owner drives the X11 clipboard and is Linux-only");
}
