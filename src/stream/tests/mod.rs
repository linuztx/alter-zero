//! Unit tests for [`crate::stream`], split to mirror the module layout.
//!
//! Every file here is a descendant of `crate::stream`, so the tests keep the
//! same reach into the seam's internals that the single flat `mod tests` had
//! before the split.

// Re-glob'd (not just imported) so the area modules below reach the whole of
// `crate::stream` — private items included — through their own `use super::*`.
use super::*;

mod cancel;
mod dummy;
mod gated;
mod scenario;
mod script;
mod source;
mod stall;
mod turns;

/// Concatenate just the `Chunk` text of a turn (its visible reply).
pub(super) fn chunk_text(events: &[StreamEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::Chunk(c) => Some(c.as_str()),
            _ => None,
        })
        .collect()
}
