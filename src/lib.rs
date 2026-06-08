//! Inline conversation TUI — library crate.
//!
//! The binary (`main.rs`) is a thin shell around these modules so that all the
//! real logic lives in pure, unit-testable functions. See `docs/design.md`.
//!
//! - [`app`]    — conversation state and the pure key/stream update logic.
//! - [`stream`] — the dummy AI: canned responses and chunked streaming.
//! - [`ui`]     — pure rendering helpers (word-wrap, message lines, live region).
//! - [`term`]   — the custom inline viewport (dynamic-height live region; I/O).

pub mod app;
pub mod frame;
pub mod paste;
pub mod stream;
pub mod term;
pub mod textarea;
pub mod ui;
