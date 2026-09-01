//! Write a small test picture, for driving the inline-image paths by hand
//! (`docs/images.md`): `cargo run --example make_test_image -- out.png [w] [h]`.

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "test.png".to_string());
    let width: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(320);
    let height: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(200);
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        // Four quadrants plus a diagonal, so a wrong aspect ratio or a
        // clipped row is obvious at a glance.
        let (fx, fy) = (x * 255 / width.max(1), y * 255 / height.max(1));
        if x * height == y * width {
            image::Rgb([255, 255, 255])
        } else {
            image::Rgb([fx as u8, fy as u8, 200u8.saturating_sub(fx as u8)])
        }
    });
    image.save(&path).expect("write the test image");
    println!("wrote {path} ({width}x{height})");
}
