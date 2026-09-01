//! Unit tests for the pure half of [`super`] — the cell math, the carrier,
//! and the two format helpers. The boundary ([`super::store`]) is smoke-tested.

use std::sync::{Mutex, MutexGuard, OnceLock};

use ratatui::layout::Size;
use ratatui::style::Color;
use ratatui_image::{FontSize as PickerFontSize, Resize};

use super::*;

/// Tests that move the process-global policy hold this, so they can't see
/// each other's settings (`cargo test` runs them on parallel threads).
fn policy_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn image(w: u32, h: u32) -> image::DynamicImage {
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        w.max(1),
        h.max(1),
        image::Rgba([1, 2, 3, 255]),
    ))
}

// ===== the cell footprint =====

#[test]
fn fit_cells_reproduces_the_encoders_own_arithmetic() {
    // The reservation is computed here (pure, from the recorded pixel size)
    // and the picture is encoded there (at the boundary, from the file). They
    // must agree to the cell or every image sits above a blank gap, so this
    // is a differential test against `ratatui_image`'s real `Resize::Fit`.
    let fonts = [(10u16, 20u16), (7, 15), (9, 18), (6, 13)];
    let sizes = [
        (1u32, 1u32),
        (32, 32),
        (700, 689),
        (1920, 1080),
        (600, 4000),
        (4000, 600),
        (2, 3000),
    ];
    let budgets = [(60u16, 30u16), (80, 40), (120, 60), (1, 1), (37, 91)];
    for font in fonts {
        for px in sizes {
            for budget in budgets {
                let ours = fit_cells(px, font, budget);
                let theirs = Resize::Fit(None).size_for(
                    &image(px.0, px.1),
                    PickerFontSize::new(font.0, font.1),
                    Size::new(budget.0, budget.1),
                );
                assert_eq!(
                    ours,
                    (theirs.width, theirs.height),
                    "font {font:?}, image {px:?}, budget {budget:?}"
                );
            }
        }
    }
}

#[test]
fn an_image_smaller_than_the_budget_keeps_its_natural_size() {
    // Fit shrinks, never grows: a 32x32 icon is a handful of cells, not a
    // blown-up 120-column banner.
    assert_eq!(fit_cells((32, 32), (10, 20), (120, 60)), (4, 2));
}

#[test]
fn a_wide_image_is_bounded_by_the_column_cap() {
    let (cols, rows) = fit_cells((4000, 600), (10, 20), (60, 30));
    assert_eq!(cols, 60, "the width cap binds");
    assert!(rows <= 30, "and the height follows it down: {rows}");
}

#[test]
fn a_tall_image_is_bounded_by_the_row_cap_not_the_width() {
    // The bug the square row box exists to stop: a 600x4000 screenshot
    // rendered to the full width would be four hundred rows tall.
    let budget = image_budget(120, 200, (10, 20)).unwrap();
    let (cols, rows) = fit_cells((600, 4000), (10, 20), budget);
    assert_eq!(rows, budget.1, "the height cap is what binds");
    assert!(cols < 120, "so it comes out narrow, not full width: {cols}");
}

// ===== the budget =====

#[test]
fn the_budget_never_exceeds_the_terminal_less_its_gutter() {
    assert_eq!(image_budget(120, 40, (10, 20)), Some((38, 19)));
    assert_eq!(
        image_budget(60, 200, (10, 20)),
        Some((60, 30)),
        "a wide terminal is capped by the setting instead"
    );
}

#[test]
fn the_row_cap_is_the_width_cap_as_a_square_pixel_box() {
    // 60 columns at a 10x20 cell is 600px across; 600px down is 30 rows.
    assert_eq!(image_budget(60, 200, (10, 20)).map(|b| b.1), Some(30));
    // A squarer cell gives a taller box for the same width.
    assert_eq!(image_budget(60, 200, (10, 10)).map(|b| b.1), Some(60));
}

#[test]
fn a_terminal_with_no_room_reserves_nothing() {
    assert_eq!(image_budget(120, IMAGE_GUTTER_COLS, (10, 20)), None);
    assert_eq!(image_budget(120, 0, (10, 20)), None);
    assert_eq!(image_cells((700, 689), (10, 20), 120, 1), None);
}

// ===== the carrier =====

#[test]
fn the_carrier_roundtrips_id_and_row() {
    for (id, row) in [(1u32, 0u16), (7, 3), (IMAGE_ID_MAX, IMAGE_MAX_ROWS)] {
        assert_eq!(
            carrier_parts(carrier(id, row)),
            Some((id, row)),
            "{id}/{row}"
        );
    }
}

#[test]
fn an_image_carrier_is_never_read_as_a_link_and_vice_versa() {
    // Both ride `underline_color`; the top bit is what separates them, so a
    // link id can never decode as a placement (which would draw a picture out
    // of a URL) and a placement can never decode as a link.
    for id in [1u32, 2, 1000, IMAGE_ID_MAX] {
        for row in [0u16, 1, 255] {
            assert_eq!(
                crate::links::carrier_id(carrier(id, row)),
                None,
                "image {id}/{row} decoded as a link"
            );
        }
    }
    let linked = crate::links::linked(ratatui::style::Style::default(), "https://example.com/x");
    let color = linked.underline_color.expect("the link carrier is stamped");
    assert!(crate::links::carrier_id(color).is_some());
    assert_eq!(carrier_parts(color), None, "a link decoded as a placement");
}

#[test]
fn only_a_flagged_rgb_underline_decodes() {
    assert_eq!(carrier_parts(Color::Reset), None);
    assert_eq!(carrier_parts(Color::Cyan), None);
    assert_eq!(carrier_parts(Color::Indexed(7)), None);
    assert_eq!(
        carrier_parts(Color::Rgb(0x80, 0, 0)),
        None,
        "id 0 is reserved"
    );
}

// ===== the placement interner =====

#[test]
fn a_placement_interns_on_its_size_so_a_resize_gets_a_fresh_one() {
    let _guard = policy_lock();
    set_policy(ImagePolicy {
        show: true,
        available: true,
        max_cols: 120,
        font: (10, 20),
        auto_resize: true,
    });
    let wide = place("/tmp/cat.png", (700, 689), 200).expect("reserved");
    let same = place("/tmp/cat.png", (700, 689), 200).expect("reserved");
    assert_eq!(wide, same, "the same picture at the same width is one id");
    let narrow = place("/tmp/cat.png", (700, 689), 40).expect("reserved");
    assert_ne!(
        wide.id, narrow.id,
        "a narrower terminal is a different placement, so the boundary \
         re-encodes instead of re-placing the old size"
    );
    assert!(narrow.cols < wide.cols);
    assert_eq!(placement(wide.id).as_ref(), Some(&wide));
    assert_eq!(placement(0), None);
    set_policy(ImagePolicy::default());
}

#[test]
fn nothing_is_reserved_while_images_are_off() {
    let _guard = policy_lock();
    set_policy(ImagePolicy {
        show: false,
        available: true,
        ..ImagePolicy::default()
    });
    assert_eq!(place("/tmp/cat.png", (700, 689), 200), None, "row off");
    set_policy(ImagePolicy {
        show: true,
        available: false,
        ..ImagePolicy::default()
    });
    assert_eq!(place("/tmp/cat.png", (700, 689), 200), None, "no terminal");
    set_policy(ImagePolicy::default());
}

#[test]
fn the_default_policy_draws_nothing_until_the_boundary_reports_in() {
    // Every unit test in the crate runs under this default, which is what
    // keeps the pre-image line output unchanged.
    assert!(!ImagePolicy::default().available);
    assert!(ImagePolicy::default().show, "the row itself is on, though");
}

// ===== the two format helpers =====

#[test]
fn read_image_size_reads_the_tools_own_fact_line() {
    let output = crate::llm::tools::format_read_image("JPEG", 700, 689, 64 * 1024);
    assert_eq!(read_image_size(&output), Some((700, 689)), "{output}");
    assert_eq!(
        read_image_size("Read image (PNG, 512x512, 17 KB)"),
        Some((512, 512))
    );
    assert_eq!(read_image_size("Read 12 lines"), None);
    assert_eq!(read_image_size(""), None);
}

#[test]
fn only_an_image_reads_output_reports_a_size() {
    // A text `read` renders as numbered source, and source contains all sorts
    // of `3x4`. The head marker is what separates the two — without it a file
    // whose line 12 says `max=3x4` reserved rows for a picture it doesn't have.
    assert_eq!(read_image_size("    12 let max = 3x4;"), None);
    assert_eq!(
        read_image_size("     1 grid = 1920x1080\n     2 done"),
        None,
        "a numbered source line is not a fact line"
    );
}

#[test]
fn read_image_size_takes_the_first_pair_so_a_resized_read_still_fits() {
    // An auto-resized read names the file's own size first and the
    // downscaled one after, because the file is what gets drawn.
    assert_eq!(
        read_image_size("Read image (PNG, 4000x3000, 4.6 MB, resized to 2000x1500 for the model)"),
        Some((4000, 3000))
    );
}

#[test]
fn resize_target_shrinks_only_and_keeps_the_aspect() {
    assert_eq!(resize_target((1024, 768), 2000), None, "already fits");
    assert_eq!(resize_target((2000, 2000), 2000), None, "exactly fits");
    assert_eq!(resize_target((4000, 3000), 2000), Some((2000, 1500)));
    assert_eq!(resize_target((600, 8000), 2000), Some((150, 2000)));
    assert_eq!(
        resize_target((10_000, 1), 2000),
        Some((2000, 1)),
        "never zero"
    );
}

// ===== the terminal detection's pure helpers =====

#[test]
fn the_protocol_override_names_every_encoder_this_build_has() {
    use ratatui_image::picker::ProtocolType;
    assert_eq!(protocol_from_name("kitty"), Some(ProtocolType::Kitty));
    assert_eq!(protocol_from_name(" Kitty "), Some(ProtocolType::Kitty));
    assert_eq!(protocol_from_name("iterm2"), Some(ProtocolType::Iterm2));
    assert_eq!(protocol_from_name("iterm"), Some(ProtocolType::Iterm2));
    assert_eq!(protocol_from_name("sixel"), Some(ProtocolType::Sixel));
    assert_eq!(
        protocol_from_name("halfblocks"),
        Some(ProtocolType::Halfblocks)
    );
    assert_eq!(protocol_from_name("chafa"), None, "not an encoder we have");
    assert_eq!(protocol_from_name(""), None);
}

#[test]
fn the_cell_size_override_parses_wxh() {
    assert_eq!(parse_cell_size("9x18"), Some((9, 18)));
    assert_eq!(parse_cell_size(" 10 X 20 "), Some((10, 20)));
    assert_eq!(parse_cell_size("0x18"), None, "a zero cell is not a size");
    assert_eq!(parse_cell_size("9"), None);
    assert_eq!(parse_cell_size("wide"), None);
}

#[test]
fn a_multiplexer_is_recognised_from_either_side() {
    assert!(under_multiplexer(Some("tmux-256color"), None));
    assert!(under_multiplexer(Some("screen"), None));
    assert!(under_multiplexer(
        Some("xterm-256color"),
        Some("/tmp/tmux-1000/default,7,0")
    ));
    assert!(!under_multiplexer(Some("xterm-256color"), None));
    assert!(!under_multiplexer(Some("xterm-256color"), Some("")));
    assert!(!under_multiplexer(None, None));
}

#[test]
fn kitty_and_ghostty_announce_themselves_and_nothing_else_does() {
    assert!(kitty_from_env(None, None, true), "KITTY_WINDOW_ID");
    assert!(kitty_from_env(Some("xterm-kitty"), None, false));
    assert!(kitty_from_env(None, Some("ghostty"), false));
    assert!(kitty_from_env(Some("xterm-ghostty"), None, false));
    assert!(!kitty_from_env(
        Some("xterm-256color"),
        Some("iTerm.app"),
        false
    ));
    assert!(!kitty_from_env(None, None, false));
}

// ===== the model-facing downscale =====

/// A gradient (not a flat fill) so the encoded bytes actually shrink with the
/// pixels — a solid colour compresses to nearly nothing at any size.
fn gradient(w: u32, h: u32) -> image::DynamicImage {
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
    }))
}

fn encode_png(w: u32, h: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    gradient(w, h)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn encode_jpeg(w: u32, h: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    gradient(w, h)
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .unwrap();
    out.into_inner()
}

#[test]
fn downscale_shrinks_an_oversized_picture_and_keeps_its_format() {
    let bytes = encode_png(3000, 2000);
    let small = downscale_to(&bytes, image::ImageFormat::Png, 2000).expect("shrunk");
    assert_eq!(small.size, (2000, 1333));
    assert_eq!(small.format, image::ImageFormat::Png);
    assert!(small.bytes.len() < bytes.len());
}

#[test]
fn a_jpeg_stays_a_jpeg_because_a_photo_as_png_grows() {
    let bytes = encode_jpeg(3000, 2000);
    let small = downscale_to(&bytes, image::ImageFormat::Jpeg, 2000).expect("shrunk");
    assert_eq!(small.format, image::ImageFormat::Jpeg);
    assert_eq!(small.size, (2000, 1333));
}

#[test]
fn a_picture_already_inside_the_cap_is_sent_untouched() {
    let bytes = encode_png(800, 600);
    assert!(downscale_to(&bytes, image::ImageFormat::Png, 2000).is_none());
}

#[test]
fn the_setting_is_what_decides_whether_a_picture_is_downscaled_at_all() {
    let _guard = policy_lock();
    let bytes = encode_png(3000, 2000);
    set_policy(ImagePolicy {
        auto_resize: true,
        ..ImagePolicy::default()
    });
    assert!(downscale_for_model(&bytes, image::ImageFormat::Png).is_some());
    set_policy(ImagePolicy {
        auto_resize: false,
        ..ImagePolicy::default()
    });
    assert!(
        downscale_for_model(&bytes, image::ImageFormat::Png).is_none(),
        "with the row off the original bytes go up unchanged"
    );
    set_policy(ImagePolicy::default());
}

#[test]
fn the_retransmit_gate_is_off_unless_asked_for() {
    // The inverse of the other gates: this one costs megabytes per Ctrl+O, so
    // it stays off until a terminal is found that needs it.
    assert!(!retransmit_forced(None));
    assert!(!retransmit_forced(Some("")));
    assert!(!retransmit_forced(Some("0")));
    assert!(!retransmit_forced(Some("false")));
    assert!(!retransmit_forced(Some(" OFF ")));
    assert!(retransmit_forced(Some("1")));
    assert!(retransmit_forced(Some("true")));
    assert!(retransmit_forced(Some("yes")));
}
