//! Where the time goes when a picture is (re-)encoded for a screen — the cost
//! a Ctrl+O pays under kitty, which must transmit afresh per screen buffer
//! (`docs/images.md`). `cargo run --release --example image_encode_cost -- shot.png [cols] [rows]`.

use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "test.png".to_string());
    let cols: u16 = args.next().and_then(|a| a.parse().ok()).unwrap_or(120);
    let rows: u16 = args.next().and_then(|a| a.parse().ok()).unwrap_or(35);
    let size = ratatui::layout::Size::new(cols, rows);

    #[allow(deprecated)]
    let mut picker =
        ratatui_image::picker::Picker::from_fontsize(ratatui_image::FontSize::new(10, 20));
    for proto in [
        ratatui_image::picker::ProtocolType::Kitty,
        ratatui_image::picker::ProtocolType::Halfblocks,
        ratatui_image::picker::ProtocolType::Sixel,
        ratatui_image::picker::ProtocolType::Iterm2,
    ] {
        picker.set_protocol_type(proto);
        let t = Instant::now();
        let bytes = std::fs::read(&path).unwrap();
        let read = t.elapsed();

        let t = Instant::now();
        let image = image::load_from_memory(&bytes).unwrap();
        let decode = t.elapsed();

        let t = Instant::now();
        let sliced = ratatui_image::sliced::SlicedProtocol::new_with_resize(
            &picker,
            image,
            size,
            ratatui_image::Resize::Fit(None),
        )
        .unwrap();
        let encode = t.elapsed();
        println!(
            "{proto:?}: read {read:?}  decode {decode:?}  encode {encode:?}  → {:?}",
            sliced.size()
        );
    }
}
