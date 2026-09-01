//! Inline terminal images: a pasted screenshot and the `read` tool's image
//! reads drawn as real pictures in the conversation (`docs/images.md`).
//!
//! Four modules over two layers — pure and boundary — split the way the crate
//! always splits them:
//!
//! - [`geometry`] — pure. How many cells a picture takes at the terminal's
//!   font size under the `/settings` **Image width** cap, and the per-cell
//!   carrier that marks the rows reserved for it.
//! - [`registry`] — the process-global render policy (`/settings` plus what
//!   the terminal turned out to support) and the placement interner, so pure
//!   `ui` line builders can reserve a block without a new argument on every
//!   signature.
//! - [`payload`] — the other boundary: downscaling a picture before it is
//!   uploaded, which is what `/settings` **Auto-resize images** controls.
//! - [`store`] — the paint boundary. What this terminal can draw (from the
//!   environment and `TIOCGWINSZ`, never a stdin round trip), the encoded
//!   pictures, and the one pass that turns a reserved block into a picture in
//!   a `Buffer`.

pub mod geometry;
pub mod payload;
pub mod registry;
pub mod store;

pub use geometry::{
    AUTO_RESIZE_MAX_PIXELS, DEFAULT_FONT_SIZE, DEFAULT_IMAGE_WIDTH, FontSize, IMAGE_GUTTER_COLS,
    IMAGE_ID_MAX, IMAGE_MAX_ROWS, IMAGE_WIDTH_CHOICES, carrier, carrier_parts, fit_cells,
    image_budget, image_cells,
};
pub use payload::{Downscaled, downscale_for_model, downscale_to};
pub use registry::{
    ImagePolicy, Placement, any_placements, auto_resizing, known_size, place, placement, policy,
    remember_size, set_policy, showing,
};
pub use store::{
    IMAGE_CELL_SIZE_ENV, IMAGE_PROTOCOL_ENV, IMAGES_ENV, ImageStore, images_disabled,
    kitty_from_env, parse_cell_size, protocol_from_name, under_multiplexer,
};

/// The pixel size an image `read`'s own fact line reports.
///
/// The `read` tool answers an image with one line —
/// `Read image (JPEG, 700x689, 63 KB)`
/// ([`crate::llm::tools::format_read_image`]) — and that line is the *only*
/// record of the picture's shape that survives a `/resume`: the rollout keeps
/// the cell's text, not the file's header. So the renderer reads the size
/// back out of it rather than opening the file, which also keeps the row
/// reservation in pure code.
///
/// The **first** `WxH` is the one taken, which is what makes an auto-resized
/// read still reserve the right rows: the fact line leads with the file's own
/// dimensions and names the downscaled ones afterwards.
#[must_use]
pub fn read_image_size(output: &str) -> Option<(u32, u32)> {
    // The head marker is the gate, not just a hint: a text `read` renders as
    // numbered source, and a line like `12 max=3x4` would otherwise parse as
    // a 3x4 picture and reserve rows for a file that has none.
    if !crate::llm::tools::is_image_read_output(output) {
        return None;
    }
    let line = output.lines().next()?;
    let mut rest = line;
    while let Some(at) = rest.find(['x', 'X']) {
        let (before, after) = rest.split_at(at);
        let after = &after[1..];
        let w: String = before
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect();
        let h: String = after.chars().take_while(char::is_ascii_digit).collect();
        if !w.is_empty() && !h.is_empty() {
            let w: u32 = w.chars().rev().collect::<String>().parse().ok()?;
            let h: u32 = h.parse().ok()?;
            if w > 0 && h > 0 {
                return Some((w, h));
            }
        }
        rest = after;
    }
    None
}

/// The pixel size a picture is downscaled to before it is sent to the model,
/// or `None` when it already fits `max` on both edges.
///
/// Aspect-preserving and shrink-only, so a coordinate in the resized picture
/// maps back to the original by one scale factor.
#[must_use]
pub fn resize_target(px: (u32, u32), max: u32) -> Option<(u32, u32)> {
    let (w, h) = (px.0.max(1), px.1.max(1));
    let max = max.max(1);
    if w <= max && h <= max {
        return None;
    }
    let ratio = (f64::from(max) / f64::from(w)).min(f64::from(max) / f64::from(h));
    Some((
        ((f64::from(w) * ratio).round() as u32).max(1),
        ((f64::from(h) * ratio).round() as u32).max(1),
    ))
}

#[cfg(test)]
mod tests;
