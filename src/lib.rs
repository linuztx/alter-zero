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
//! - [`llm`]         — the real OpenAI-compatible backend: provider config, the
//!   streaming client, the `/v1/models` listing, and the `ReplySource` bridge
//!   (the I/O boundary for a real model; pure cores unit-tested).
//! - [`paste`]       — paste-burst detection and the large-paste/image
//!   placeholder helpers.
//! - [`session`]     — the `/resume` rollout-file format: serialize/parse the
//!   JSONL session record, the picker preview + humanized age.
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
pub mod llm;
pub mod paste;
pub mod session;
pub mod stream;
pub mod term;
pub mod textarea;
pub mod ui;
