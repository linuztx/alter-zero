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

// ===== the fitted decode (`fitted`) =====

/// A picture whose every pixel differs, so a shrink that sampled instead of
/// averaging — or averaged the wrong neighbours — shows up in the numbers.
fn rgba_grid(w: u32, h: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(w, h, |x, y| {
        let v = |k: u32| u8::try_from((x * 7 + y * 13 + k * 31) % 256).unwrap_or(0);
        image::Rgba([v(0), v(1), v(2), 255u8.saturating_sub(v(3) / 2)])
    })
}

/// Area averaging written the slow, obvious way — one pass over every source
/// pixel per destination pixel — as an independent oracle for the streaming
/// downsampler.
fn naive_area_average(src: &image::RgbaImage, tw: u32, th: u32) -> image::RgbaImage {
    let (sw, sh) = src.dimensions();
    let (sx, sy) = (f64::from(tw) / f64::from(sw), f64::from(th) / f64::from(sh));
    image::RgbaImage::from_fn(tw, th, |dx, dy| {
        let mut acc = [0f64; 4];
        let mut weight = 0f64;
        for y in 0..sh {
            let (top, bottom) = (f64::from(y) * sy, f64::from(y + 1) * sy);
            let oy = bottom.min(f64::from(dy + 1)) - top.max(f64::from(dy));
            if oy <= 0.0 {
                continue;
            }
            for x in 0..sw {
                let (left, right) = (f64::from(x) * sx, f64::from(x + 1) * sx);
                let ox = right.min(f64::from(dx + 1)) - left.max(f64::from(dx));
                if ox <= 0.0 {
                    continue;
                }
                let w = ox * oy;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += w * f64::from(src.get_pixel(x, y)[c]);
                }
                weight += w;
            }
        }
        image::Rgba(acc.map(|a| (a / weight).round().clamp(0.0, 255.0) as u8))
    })
}

fn assert_close(actual: &image::RgbaImage, expected: &image::RgbaImage, what: &str) {
    assert_eq!(actual.dimensions(), expected.dimensions(), "{what}: size");
    for (x, y, px) in actual.enumerate_pixels() {
        let want = expected.get_pixel(x, y);
        for c in 0..4 {
            let (a, b) = (i16::from(px[c]), i16::from(want[c]));
            assert!(
                (a - b).abs() <= 1,
                "{what}: pixel ({x},{y}) channel {c}: got {a}, expected {b}"
            );
        }
    }
}

#[test]
fn fit_box_reproduces_the_encoders_own_fit() {
    // The fitted decode must hand the encoder a picture already at the size
    // `Resize::Fit` would have shrunk it to — same arithmetic, same cells —
    // or the reservation and the drawing disagree by a row. Pixels are
    // checked against the `image` crate's own `resize` (which is what the
    // encoder calls), cells against `fit_cells`.
    let fonts = [(10u16, 20u16), (7, 15), (9, 18)];
    let sizes = [
        (1u32, 1u32),
        (32, 32),
        (700, 689),
        (1920, 1080),
        (600, 4000),
        (4000, 600),
    ];
    let budgets = [(60u16, 30u16), (120, 60), (37, 91), (1, 1)];
    for font in fonts {
        for px in sizes {
            for budget in budgets {
                let box_px = (
                    u32::from(budget.0) * u32::from(font.0),
                    u32::from(budget.1) * u32::from(font.1),
                );
                let fitted = fitted::fit_box(px, box_px);
                assert!(
                    fitted.0 <= px.0 && fitted.1 <= px.1,
                    "shrink-only: {px:?} into {box_px:?} gave {fitted:?}"
                );
                if px.0 > box_px.0 || px.1 > box_px.1 {
                    let theirs =
                        image(px.0, px.1).resize(box_px.0, box_px.1, image::imageops::Nearest);
                    assert_eq!(
                        fitted,
                        (theirs.width(), theirs.height()),
                        "font {font:?} {px:?} {budget:?}"
                    );
                } else {
                    assert_eq!(fitted, px, "a picture inside the box is left alone");
                }
                let cells = (
                    (f64::from(fitted.0) / f64::from(font.0)).ceil() as u16,
                    (f64::from(fitted.1) / f64::from(font.1)).ceil() as u16,
                );
                assert_eq!(
                    cells,
                    fit_cells(px, font, budget),
                    "font {font:?} {px:?} {budget:?}"
                );
            }
        }
    }
}

#[test]
fn a_downsampler_at_the_source_size_is_an_exact_copy() {
    let src = rgba_grid(7, 5);
    let mut down = fitted::Downsampler::new((7, 5), (7, 5));
    for row in src.rows() {
        let bytes: Vec<u8> = row.flat_map(|p| p.0).collect();
        down.push_row(&bytes);
    }
    assert_eq!(down.finish(), src);
}

#[test]
fn a_downsampler_halves_by_averaging_each_two_by_two_block() {
    // A 4x4 checkerboard of 200s and 100s: every 2x2 block averages to 150.
    let src = image::RgbaImage::from_fn(4, 4, |x, y| {
        let v = if (x + y) % 2 == 0 { 200 } else { 100 };
        image::Rgba([v, v, v, 255])
    });
    let mut down = fitted::Downsampler::new((4, 4), (2, 2));
    for row in src.rows() {
        let bytes: Vec<u8> = row.flat_map(|p| p.0).collect();
        down.push_row(&bytes);
    }
    let out = down.finish();
    assert_eq!(out.dimensions(), (2, 2));
    for px in out.pixels() {
        assert_eq!(px.0, [150, 150, 150, 255]);
    }
}

#[test]
fn a_downsampler_weights_a_fractional_overlap_by_its_area() {
    // Three pixels into two: the middle source pixel is split evenly, so the
    // left result is (0·1 + 60·½)/1.5 = 20 and the right (60·½ + 120·1)/1.5 = 100.
    let mut down = fitted::Downsampler::new((3, 1), (2, 1));
    down.push_row(&[0, 0, 0, 255, 60, 60, 60, 255, 120, 120, 120, 255]);
    let out = down.finish();
    assert_eq!(out.get_pixel(0, 0).0, [20, 20, 20, 255]);
    assert_eq!(out.get_pixel(1, 0).0, [100, 100, 100, 255]);
}

#[test]
fn a_downsampler_matches_the_naive_area_average_at_an_awkward_ratio() {
    // 37x23 → 11x7: nothing divides evenly, so every destination pixel
    // straddles source pixels in both directions.
    let src = rgba_grid(37, 23);
    let mut down = fitted::Downsampler::new((37, 23), (11, 7));
    for row in src.rows() {
        let bytes: Vec<u8> = row.flat_map(|p| p.0).collect();
        down.push_row(&bytes);
    }
    assert_close(
        &down.finish(),
        &naive_area_average(&src, 11, 7),
        "37x23 → 11x7",
    );
}

fn png_of(image: &image::DynamicImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encode");
    bytes
}

#[test]
fn decode_png_fitted_shrinks_to_the_target_the_caller_picks() {
    let src = rgba_grid(64, 48);
    let bytes = png_of(&image::DynamicImage::ImageRgba8(src.clone()));
    let seen = std::cell::Cell::new(None);
    let out = fitted::decode_png_fitted(std::io::Cursor::new(&bytes), |px| {
        seen.set(Some(px));
        (32, 24)
    })
    .expect("decodes");
    assert_eq!(
        seen.get(),
        Some((64, 48)),
        "the header size is what the caller sees"
    );
    assert_close(
        &out.to_rgba8(),
        &naive_area_average(&src, 32, 24),
        "64x48 → 32x24",
    );
}

#[test]
fn decode_png_fitted_at_the_source_size_is_the_whole_picture() {
    let src = rgba_grid(19, 11);
    let bytes = png_of(&image::DynamicImage::ImageRgba8(src.clone()));
    let out = fitted::decode_png_fitted(std::io::Cursor::new(&bytes), |px| px).expect("decodes");
    assert_eq!(out.to_rgba8(), src);
}

#[test]
fn decode_png_fitted_reads_every_colour_type_the_format_has() {
    // Gray, gray+alpha, RGB, RGBA and 16-bit RGB all arrive as 8-bit rows once
    // the decoder normalises them; a source without alpha stays without it,
    // so a re-encode for the model doesn't grow by a channel of 255s.
    let rgb = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(6, 4, |x, y| {
        image::Rgb([
            u8::try_from(x * 40).unwrap(),
            u8::try_from(y * 60).unwrap(),
            7,
        ])
    }));
    let gray = image::DynamicImage::ImageLuma8(image::GrayImage::from_fn(6, 4, |x, _| {
        image::Luma([u8::try_from(x * 40).unwrap()])
    }));
    let gray_a = image::DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_fn(6, 4, |x, y| {
        image::LumaA([u8::try_from(x * 40).unwrap(), u8::try_from(y * 60).unwrap()])
    }));
    let rgb16 = image::DynamicImage::ImageRgb16(image::ImageBuffer::from_fn(6, 4, |x, y| {
        image::Rgb([
            u16::try_from(x * 40 * 257).unwrap(),
            u16::try_from(y * 60 * 257).unwrap(),
            7 * 257,
        ])
    }));
    for (label, source) in [
        ("rgb", &rgb),
        ("gray", &gray),
        ("gray+alpha", &gray_a),
        ("rgb16", &rgb16),
    ] {
        let out = fitted::decode_png_fitted(std::io::Cursor::new(png_of(source)), |px| px)
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(out.to_rgba8(), source.to_rgba8(), "{label}");
        assert_eq!(
            out.color().has_alpha(),
            source.color().has_alpha(),
            "{label}: alpha is kept exactly when the source had one"
        );
    }
}

#[test]
fn decode_png_fitted_refuses_what_is_not_a_png() {
    assert!(fitted::decode_png_fitted(std::io::Cursor::new(encode_jpeg(8, 8)), |px| px).is_err());
    assert!(
        fitted::decode_png_fitted(std::io::Cursor::new(b"not a picture".as_slice()), |px| px)
            .is_err()
    );
}

// ===== the decoder's bounds (`fitted`) =====

/// CRC-32 (IEEE) — for re-signing a hand-edited PNG chunk in a test.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Re-sign a PNG's IHDR after its bytes were edited in place (the chunk data
/// is bytes 16..29, its CRC covers the type and the data: 12..29).
fn resign_ihdr(png: &mut [u8]) {
    let crc = crc32(&png[12..29]);
    png[29..33].copy_from_slice(&crc.to_be_bytes());
}

#[test]
fn decode_png_fitted_declines_an_interlaced_picture() {
    // An Adam7 picture's rows don't arrive top to bottom, so it is left to
    // the whole decode — decided from the header, before a single pixel.
    let mut png = png_of(&image::DynamicImage::ImageRgba8(rgba_grid(4, 4)));
    png[28] = 1; // IHDR's interlace method
    resign_ihdr(&mut png);
    let header = png::Decoder::new(std::io::Cursor::new(&png[..]))
        .read_info()
        .expect("the header still parses");
    assert!(header.info().interlaced, "the edit took");
    assert!(fitted::decode_png_fitted(std::io::Cursor::new(&png[..]), |px| px).is_err());
}

#[test]
fn decode_png_fitted_expands_a_palette_picture() {
    // `image` can't write an indexed PNG, so the png crate writes one: four
    // palette entries with a tRNS alpha each, which the decoder expands to
    // RGBA rows exactly as a whole decode through `image` would.
    let (w, h) = (64u32, 40u32);
    let mut bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut bytes, w, h);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_palette(vec![10, 20, 30, 200, 100, 0, 0, 0, 255, 255, 255, 255]);
        enc.set_trns(vec![255, 128, 255, 0]);
        let mut writer = enc.write_header().unwrap();
        let data: Vec<u8> = (0..w * h).map(|i| ((i / 3) % 4) as u8).collect();
        writer.write_image_data(&data).unwrap();
    }
    let whole = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .unwrap()
        .into_rgba8();
    let got = fitted::decode_png_fitted(std::io::Cursor::new(&bytes[..]), |px| px)
        .expect("an indexed PNG streams");
    assert!(got.color().has_alpha(), "the tRNS alpha is kept");
    assert_eq!(got.into_rgba8(), whole);
}

#[test]
fn decode_png_fitted_refuses_a_header_past_the_streaming_bound() {
    // Streaming never holds the picture, so the bound is on *time*: a header
    // claiming 20000x20000 is refused before a row is read.
    let mut png = png_of(&image::DynamicImage::ImageRgba8(rgba_grid(4, 4)));
    png[16..20].copy_from_slice(&20_000u32.to_be_bytes());
    png[20..24].copy_from_slice(&20_000u32.to_be_bytes());
    resign_ihdr(&mut png);
    let err =
        fitted::decode_png_fitted(std::io::Cursor::new(&png[..]), |px| px).expect_err("refused");
    assert!(err.contains("pixels"), "{err}");
    assert!(
        u64::from(20_000u32) * 20_000 > fitted::FIT_MAX_SOURCE_PIXELS,
        "the fixture is past the bound"
    );
}

#[test]
fn a_whole_decode_is_refused_past_fifty_megapixels() {
    // Decoding whole is 4 bytes a pixel resident, so the non-PNG paths ask
    // first: a 7000x7000 photo (49 MP) decodes, a 7100x7100 one (50.4 MP) is
    // declined rather than materialised to discover it shouldn't have been.
    assert!(fitted::whole_decode_fits((7000, 7000)));
    assert!(!fitted::whole_decode_fits((7100, 7100)));
    assert_eq!(fitted::WHOLE_DECODE_MAX_PIXELS, 50_000_000);
}

// ===== the payload cache (`payload`, docs/images.md "Memory") =====

#[test]
fn payload_cache_key_names_the_file_its_state_and_the_cap() {
    let path = std::path::Path::new("/tmp/a.png");
    let a = payload_cache_key(path, 100, 5, 2000);
    assert_eq!(a, payload_cache_key(path, 100, 5, 2000), "deterministic");
    assert_ne!(
        a,
        payload_cache_key(std::path::Path::new("/tmp/b.png"), 100, 5, 2000),
        "the path"
    );
    assert_ne!(a, payload_cache_key(path, 101, 5, 2000), "the size");
    assert_ne!(a, payload_cache_key(path, 100, 6, 2000), "the mtime");
    assert_ne!(a, payload_cache_key(path, 100, 5, 1000), "the cap");
    assert!(
        a.len() == 32 && a.chars().all(|c| c.is_ascii_hexdigit()),
        "a bare file name, never a path: {a}"
    );
}

#[test]
fn a_downscaled_payload_is_written_once_and_read_back_without_decoding() {
    let _guard = policy_lock();
    let cache = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    std::fs::write(&path, encode_png(3000, 2000)).unwrap();
    set_payload_cache_dir(Some(cache.path().to_path_buf()));
    set_policy(ImagePolicy {
        auto_resize: true,
        ..ImagePolicy::default()
    });

    assert!(
        cached_downscale(&path, image::ImageFormat::Png).is_none(),
        "nothing cached yet"
    );
    let bytes = std::fs::read(&path).unwrap();
    let first = downscale_for_model_at(&path, &bytes, image::ImageFormat::Png).expect("shrunk");
    assert_eq!(first.size, (2000, 1333));
    let entries: Vec<_> = std::fs::read_dir(cache.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "one sidecar per picture");
    assert_eq!(
        std::fs::read(&entries[0]).unwrap(),
        first.bytes,
        "the sidecar IS the payload"
    );

    let hit = cached_downscale(&path, image::ImageFormat::Png).expect("served from disk");
    assert_eq!(hit.bytes, first.bytes);
    assert_eq!(hit.size, first.size);
    assert_eq!(hit.format, first.format);
    // Asking again computes nothing new: still the one file.
    downscale_for_model_at(&path, &bytes, image::ImageFormat::Png).expect("shrunk");
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 1);

    // A changed file is a different key — a stale sidecar is never served.
    std::fs::write(&path, encode_png(3000, 2001)).unwrap();
    assert!(cached_downscale(&path, image::ImageFormat::Png).is_none());

    set_payload_cache_dir(None);
    set_policy(ImagePolicy::default());
}

#[test]
fn without_a_cache_dir_the_payload_is_computed_and_nothing_is_written() {
    let _guard = policy_lock();
    set_payload_cache_dir(None);
    set_policy(ImagePolicy {
        auto_resize: true,
        ..ImagePolicy::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    std::fs::write(&path, encode_png(3000, 2000)).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(downscale_for_model_at(&path, &bytes, image::ImageFormat::Png).is_some());
    assert!(cached_downscale(&path, image::ImageFormat::Png).is_none());
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "the picture's own directory gains no sidecar"
    );
    set_policy(ImagePolicy::default());
}

#[test]
fn the_setting_off_serves_no_cached_payload_either() {
    let _guard = policy_lock();
    let cache = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    std::fs::write(&path, encode_png(3000, 2000)).unwrap();
    set_payload_cache_dir(Some(cache.path().to_path_buf()));
    set_policy(ImagePolicy {
        auto_resize: true,
        ..ImagePolicy::default()
    });
    let bytes = std::fs::read(&path).unwrap();
    downscale_for_model_at(&path, &bytes, image::ImageFormat::Png).expect("cached");
    set_policy(ImagePolicy {
        auto_resize: false,
        ..ImagePolicy::default()
    });
    assert!(
        cached_downscale(&path, image::ImageFormat::Png).is_none(),
        "with the row off the original goes up, whatever the cache holds"
    );
    assert!(downscale_for_model_at(&path, &bytes, image::ImageFormat::Png).is_none());
    set_payload_cache_dir(None);
    set_policy(ImagePolicy::default());
}

#[test]
fn a_jpeg_payload_is_shrunk_to_the_target_without_a_whole_source_resample() {
    // The non-PNG path thumbnails (a box filter with no f32 pass); the result
    // is the right size, a JPEG, and smaller than the source.
    let bytes = encode_jpeg(3000, 2000);
    let small = downscale_to(&bytes, image::ImageFormat::Jpeg, 2000).expect("shrunk");
    assert_eq!(small.size, (2000, 1333));
    assert_eq!(small.format, image::ImageFormat::Jpeg);
    assert!(small.bytes.len() < bytes.len());
}

// ===== the payload cache's bound (docs/memory.md) =====

/// `(sidecar, bytes, age-in-days)` → what the eviction weighs.
fn sidecars(rows: &[(&str, u64, u64)]) -> Vec<(std::path::PathBuf, u64, std::time::SystemTime)> {
    let day = std::time::Duration::from_secs(86_400);
    rows.iter()
        .map(|(name, bytes, age)| {
            (
                std::path::PathBuf::from(format!("/s/images/{name}")),
                *bytes,
                std::time::UNIX_EPOCH + day * 400 - day * u32::try_from(*age).unwrap(),
            )
        })
        .collect()
}

#[test]
fn a_payload_cache_with_room_for_the_new_copy_evicts_nothing() {
    let entries = sidecars(&[("a", 10, 9), ("b", 20, 1)]);
    assert!(cache_eviction(&entries, 30, 100).is_empty());
}

#[test]
fn a_full_payload_cache_evicts_the_oldest_until_the_new_copy_fits() {
    // The incoming copy counts against the cap, so the room made is for it
    // and not merely for what is already there.
    let entries = sidecars(&[("old", 40, 30), ("mid", 40, 20), ("new", 40, 1)]);
    assert_eq!(
        cache_eviction(&entries, 20, 140),
        Vec::<std::path::PathBuf>::new(),
        "120 held + 20 incoming is exactly 140: the cap bounds the cache, it \
         is not a strict inequality (`CappedWriter`'s rule)"
    );
    assert_eq!(
        cache_eviction(&entries, 20, 130),
        vec![std::path::PathBuf::from("/s/images/old")],
        "one over, so the oldest goes and no more"
    );
    assert_eq!(
        cache_eviction(&entries, 20, 60),
        vec![
            std::path::PathBuf::from("/s/images/old"),
            std::path::PathBuf::from("/s/images/mid"),
        ]
    );
}

#[test]
fn a_payload_cap_of_zero_means_no_limit() {
    let entries = sidecars(&[("a", 900, 9)]);
    assert_eq!(
        cache_eviction(&entries, 900, 0),
        Vec::<std::path::PathBuf>::new()
    );
}

#[test]
fn an_incoming_copy_larger_than_the_whole_cap_empties_the_cache_and_no_more() {
    // Nothing that can be evicted makes it fit, so the cache is cleared and
    // the copy is still written: the cap bounds what is *kept*, and refusing
    // to cache would mean decoding the picture again on every later turn.
    let entries = sidecars(&[("a", 10, 9), ("b", 10, 1)]);
    assert_eq!(
        cache_eviction(&entries, 500, 100),
        vec![
            std::path::PathBuf::from("/s/images/a"),
            std::path::PathBuf::from("/s/images/b"),
        ]
    );
}

// ===== the attachment cache (`attachment`, docs/memory.md "Every turn re-sent the picture") =====

/// A picture's `data:` URL as the wire carries it — the reference the cache's
/// output is checked against.
fn data_url_of(mime: &str, bytes: &[u8]) -> String {
    let mut url = format!("data:{mime};base64,");
    crate::clipboard::base64_encode_into(bytes, &mut url);
    url
}

#[test]
fn an_attachment_is_encoded_once_and_shared_after() {
    let _guard = policy_lock();
    set_policy(ImagePolicy::default());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    let png = encode_png(100, 80);
    std::fs::write(&path, &png).unwrap();

    let first = attachment_data_url(&path).expect("a readable picture encodes");
    assert_eq!(&*first, data_url_of("image/png", &png));
    let again = attachment_data_url(&path).expect("still there");
    assert!(
        AttachmentUrl::ptr_eq(&first, &again),
        "a later turn shares the one encoding rather than building another"
    );
}

#[test]
fn a_rewritten_attachment_is_encoded_afresh() {
    let _guard = policy_lock();
    set_policy(ImagePolicy::default());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    std::fs::write(&path, encode_png(100, 80)).unwrap();
    let first = attachment_data_url(&path).expect("encodes");

    let replacement = encode_png(120, 80);
    std::fs::write(&path, &replacement).unwrap();
    // A rewrite within the same clock tick keeps the mtime; bump it so the
    // stamp the cache validates against moves the way a real edit's does.
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    let second = attachment_data_url(&path).expect("encodes");
    assert!(
        !AttachmentUrl::ptr_eq(&first, &second),
        "a changed file is a new entry"
    );
    assert_eq!(&*second, data_url_of("image/png", &replacement));
}

#[test]
fn a_missing_attachment_encodes_to_nothing() {
    let _guard = policy_lock();
    set_policy(ImagePolicy::default());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gone.png");
    assert!(attachment_data_url(&path).is_none());
    std::fs::write(&path, encode_png(10, 10)).unwrap();
    assert!(attachment_data_url(&path).is_some());
    std::fs::remove_file(&path).unwrap();
    assert!(
        attachment_data_url(&path).is_none(),
        "a picture deleted from under the cache is not served from it"
    );
}

#[test]
fn the_read_tools_payload_is_remembered_for_the_turns_after() {
    // The `read` tool has the bytes in hand; what it sends is what every later
    // turn re-sends, so it hands the encoding to the cache instead of leaving
    // the next request to read and encode the file again.
    let _guard = policy_lock();
    set_policy(ImagePolicy::default());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.jpg");
    let jpeg = encode_jpeg(64, 48);
    std::fs::write(&path, &jpeg).unwrap();
    let remembered = remember_attachment(&path, &jpeg, "image/jpeg");
    assert_eq!(&*remembered, data_url_of("image/jpeg", &jpeg));
    let later = attachment_data_url(&path).expect("served");
    assert!(AttachmentUrl::ptr_eq(&remembered, &later));
}

#[test]
fn retain_attachments_keeps_only_the_pictures_still_in_the_conversation() {
    let _guard = policy_lock();
    set_policy(ImagePolicy::default());
    let dir = tempfile::tempdir().unwrap();
    let kept = dir.path().join("kept.png");
    let dropped = dir.path().join("dropped.png");
    std::fs::write(&kept, encode_png(10, 10)).unwrap();
    std::fs::write(&dropped, encode_png(12, 10)).unwrap();
    let kept_url = attachment_data_url(&kept).unwrap();
    let dropped_url = attachment_data_url(&dropped).unwrap();
    retain_attachments(&[kept.as_path()]);
    assert!(AttachmentUrl::ptr_eq(
        &kept_url,
        &attachment_data_url(&kept).unwrap()
    ));
    assert!(
        !AttachmentUrl::ptr_eq(&dropped_url, &attachment_data_url(&dropped).unwrap()),
        "a picture the context no longer carries is let go"
    );
    clear_attachments();
    assert!(!AttachmentUrl::ptr_eq(
        &kept_url,
        &attachment_data_url(&kept).unwrap()
    ));
}

#[test]
fn the_attachment_cache_is_bounded_by_bytes_and_keeps_the_newest() {
    // Pure: the cache struct itself, with a tiny cap.
    let mut cache = AttachmentCache::with_cap(100);
    let stamp = AttachmentStamp::new(1, 1, true);
    let url = |n: usize| AttachmentUrl::from("x".repeat(n));
    cache.insert("a".into(), stamp, url(60));
    cache.insert("b".into(), stamp, url(60));
    assert!(
        cache.get("a", stamp).is_none(),
        "the older entry went to make room"
    );
    assert!(cache.get("b", stamp).is_some());
    assert_eq!(cache.bytes(), 60);

    cache.insert("a".into(), stamp, url(30));
    assert!(
        cache.get("b", stamp).is_some(),
        "touched: b is now the newest"
    );
    cache.insert("c".into(), stamp, url(40));
    assert!(
        cache.get("a", stamp).is_none(),
        "a was the least recently used"
    );
    assert!(cache.get("b", stamp).is_some() && cache.get("c", stamp).is_some());
    assert_eq!(
        cache.bytes(),
        100,
        "the cap is a bound, not a strict inequality"
    );

    cache.insert("big".into(), stamp, url(500));
    assert!(
        cache.get("big", stamp).is_some(),
        "a picture larger than the whole cap is still kept — it is what the next turn sends"
    );
    assert!(cache.get("b", stamp).is_none() && cache.get("c", stamp).is_none());
    assert_eq!(cache.bytes(), 500);

    let other = AttachmentStamp::new(2, 1, true);
    assert!(
        cache.get("big", other).is_none(),
        "a different file state (or setting) is a miss, never a stale picture"
    );
}

#[test]
fn streamed_base64_matches_the_slice_encoder() {
    // The streaming encoder feeds the file through a small buffer; every
    // chunk boundary must land where the slice encoder's 3-byte groups do.
    let mut seed = 7u32;
    let bytes: Vec<u8> = (0..200_003)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 24) as u8
        })
        .collect();
    for n in (0..=70).chain([4095, 4096, 4097, 49_151, 49_152, 49_153, 200_003]) {
        let mut expected = String::new();
        crate::clipboard::base64_encode_into(&bytes[..n], &mut expected);
        let mut streamed = String::new();
        base64_encode_reader(&bytes[..n], n, &mut streamed).unwrap();
        assert_eq!(streamed, expected, "{n} bytes");
    }
}

#[test]
fn an_oversized_attachment_is_downscaled_before_it_is_encoded() {
    let _guard = policy_lock();
    set_payload_cache_dir(None);
    set_policy(ImagePolicy {
        auto_resize: true,
        ..ImagePolicy::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.png");
    let png = encode_png(3000, 2000);
    std::fs::write(&path, &png).unwrap();
    let small = downscale_to(&png, image::ImageFormat::Png, 2000).expect("shrinks");
    assert_eq!(
        &*attachment_data_url(&path).unwrap(),
        data_url_of("image/png", &small.bytes),
        "what goes up is the 2000-pixel payload"
    );
    set_policy(ImagePolicy {
        auto_resize: false,
        ..ImagePolicy::default()
    });
    assert_eq!(
        &*attachment_data_url(&path).unwrap(),
        data_url_of("image/png", &png),
        "with the row off the file goes up verbatim — the entry re-keys on the setting"
    );
    set_policy(ImagePolicy::default());
}

#[test]
fn a_fitting_attachment_goes_up_verbatim_under_its_own_mime() {
    let _guard = policy_lock();
    set_policy(ImagePolicy::default());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photo.jpeg");
    let jpeg = encode_jpeg(80, 60);
    std::fs::write(&path, &jpeg).unwrap();
    assert_eq!(
        &*attachment_data_url(&path).unwrap(),
        data_url_of("image/jpeg", &jpeg)
    );
}

#[test]
fn attachment_mime_follows_the_extension_with_png_as_the_default() {
    let p = std::path::Path::new;
    assert_eq!(attachment_mime(p("a.png")), "image/png");
    assert_eq!(attachment_mime(p("a.JPG")), "image/jpeg");
    assert_eq!(attachment_mime(p("a.jpeg")), "image/jpeg");
    assert_eq!(attachment_mime(p("a.gif")), "image/gif");
    assert_eq!(attachment_mime(p("a.webp")), "image/webp");
    assert_eq!(attachment_mime(p("no-extension")), "image/png");
}
