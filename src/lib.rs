//! Inline conversation TUI — library crate.
//!
//! The binary (`main.rs`) is a thin shell around these modules so that all the
//! real logic lives in pure, unit-testable functions. See `docs/design.md`.
//!
//! - [`app`]         — conversation state and the pure key/stream update logic.
//! - [`clipboard`]   — Ctrl+V image reads and the `/copy` write (arboard +
//!   OSC 52); the base64 framing is the tested pure core, the rest is I/O.
//! - [`file_search`] — the `@` picker's pure primitives (token detection,
//!   fuzzy match, ranking).
//! - [`frame`]       — frame scheduling: coalesce redraw requests, 120 fps cap.
//! - [`paste`]       — paste-burst detection and the large-paste/image
//!   placeholder helpers.
//! - [`stream`]      — the dummy AI: canned responses and chunked streaming.
//! - [`term`]        — the custom inline viewport (dynamic-height live region;
//!   I/O).
//! - [`textarea`]    — the grapheme-aware editable composer.
//! - [`ui`]          — pure rendering helpers (word-wrap, message lines, live
//!   region).

pub mod app;
pub mod clipboard;
pub mod file_search;
pub mod frame;
pub mod paste;
pub mod stream;
pub mod term;
pub mod textarea;
pub mod ui;
