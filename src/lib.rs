//! Inline conversation TUI — library crate.
//!
//! The binary (`main.rs`) is a thin shell around these modules so that all the
//! real logic lives in pure, unit-testable functions. See `docs/design.md`.
//!
//! - [`app`]         — conversation state and the pure key/stream update logic.
//! - [`clipboard`]   — Ctrl+V image reads and the `/copy` write (arboard +
//!   OSC 52); the base64 framing is the tested pure core, the rest is I/O.
//! - [`context`]     — the per-session LLM conversation context: derive the
//!   raw message list a real backend sends (and the Ctrl+D context-debug
//!   view shows) from history. See `docs/context.md`.
//! - [`file_search`] — the `@` picker's pure primitives (token detection,
//!   fuzzy match, ranking).
//! - [`frame`]       — frame scheduling: coalesce redraw requests, 120 fps cap.
//! - [`highlight`]   — dependency-free syntax highlighting for code blocks: a
//!   generic tokenizer (keywords/strings/comments/numbers/calls), prefix-stable.
//! - [`llm`]         — the real OpenAI-compatible backend: provider config, the
//!   streaming client, the `/v1/models` listing, and the `ReplySource` bridge
//!   (the I/O boundary for a real model; pure cores unit-tested).
//! - [`markdown`]    — the pure block parser for assistant replies: split prose
//!   from fenced code blocks and detect ATX headings (see `docs/markdown.md`).
//! - [`paste`]       — paste-burst detection and the large-paste/image
//!   placeholder helpers.
//! - [`session`]     — the `/resume` rollout-file format: serialize/parse the
//!   JSONL session record, the picker preview + humanized age.
//! - [`stream`]      — the dummy AI: canned responses and chunked streaming.
//! - [`term`]        — the custom inline viewport (dynamic-height live region;
//!   I/O).
//! - [`textarea`]    — the grapheme-aware editable composer.
//! - [`tokenizer`]   — accurate token counting for the status tally (tiktoken
//!   `o200k_base`, ranks embedded; the count seam behind `app::count_tokens`).
//! - [`ui`]          — pure rendering helpers (word-wrap, message lines, live
//!   region).

pub mod app;
pub mod clipboard;
pub mod context;
pub mod file_search;
pub mod frame;
pub mod highlight;
pub mod history;
pub mod llm;
pub mod markdown;
pub mod paste;
pub mod session;
pub mod stream;
pub mod term;
pub mod textarea;
pub mod tokenizer;
pub mod ui;
