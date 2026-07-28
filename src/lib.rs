//! Inline conversation TUI — library crate.
//!
//! The binary (`main.rs`) is a thin shell around these modules so that all the
//! real logic lives in pure, unit-testable functions. See `docs/design.md`.
//!
//! - [`agents`]      — the model's `Agent` tool: the subagent roster state and
//!   the registry behind launching, stopping, and chatting with subagents
//!   (pure state + boundary registry; see `docs/agent-tool.md`).
//! - [`app`]         — conversation state and the pure key/stream update logic,
//!   split one module per area (see `docs/module-layout.md`).
//! - [`background`]  — background shell processes: the registry behind
//!   `run_in_background`, Ctrl+B, and the ↓ manager (boundary; see
//!   `docs/background.md`).
//! - [`checkpoint`]  — per-turn working-directory snapshots in an isolated git
//!   store so `/resume` and the Esc-Esc backtrack can reset the code, not just
//!   the transcript (pure mapping + boundary store; see `docs/checkpoint.md`).
//! - [`clipboard`]   — Ctrl+V image reads and the `/copy` write (arboard +
//!   OSC 52); the base64 framing is the tested pure core, the rest is I/O.
//! - [`context`]     — the per-session LLM conversation context: derive the
//!   raw message list a real backend sends (and the Ctrl+D context-debug
//!   view shows) from history. See `docs/context.md`.
//! - [`file_search`] — the `@` picker's pure primitives (token detection,
//!   fuzzy match, ranking).
//! - [`frame`]       — frame scheduling: coalesce redraw requests, 120 fps cap.
//! - [`highlight`]   — grammar-accurate syntax highlighting for code blocks:
//!   syntect + two_face (~250 TextMate grammars, Catppuccin Mocha theme — codex
//!   parity), driven incrementally per line so it stays prefix-stable.
//! - [`llm`]         — the real OpenAI-compatible backend: provider config, the
//!   streaming client, the `/v1/models` listing, and the `ReplySource` bridge
//!   (the I/O boundary for a real model; pure cores unit-tested).
//! - [`markdown`]    — the pure block parser for assistant replies: split prose
//!   from fenced code blocks and detect ATX headings (see `docs/markdown.md`).
//! - [`paste`]       — paste-burst detection and the large-paste/image
//!   placeholder helpers.
//! - [`project_doc`] — AGENTS.md discovery: codex's project doc collected
//!   root→cwd under a 32 KiB cap and rendered as the user-instructions
//!   context fragment (see `docs/project-doc.md`).
//! - [`session`]     — the `/resume` rollout-file format: serialize/parse the
//!   JSONL session record, the picker preview + humanized age.
//! - [`stream`]      — the dummy AI: canned responses and chunked streaming.
//! - [`subprocess`]  — the shared detached `sh -c` spawn (setsid binary →
//!   helper re-exec → attached): every shell runner's child is severed from
//!   the controlling terminal so a `/dev/tty` password prompt (`sudo`) fails
//!   fast instead of hijacking the TUI.
//! - [`term`]        — the custom inline viewport (dynamic-height live region;
//!   I/O).
//! - [`textarea`]    — the grapheme-aware editable composer.
//! - [`tokenizer`]   — accurate token counting for the status tally (tiktoken
//!   `o200k_base`, ranks embedded; the count seam behind `app::count_tokens`).
//! - [`ui`]          — pure rendering helpers (word-wrap, message lines, live
//!   region), split one module per area with every styling constant in
//!   `ui::theme` (see `docs/module-layout.md`).

pub mod agents;
pub mod app;
pub mod background;
pub mod checkpoint;
pub mod clipboard;
pub mod context;
pub mod file_search;
pub mod frame;
pub mod highlight;
pub mod history;
pub mod llm;
pub mod markdown;
pub mod paste;
pub mod project_doc;
pub mod session;
pub mod stream;
pub mod subprocess;
pub mod term;
pub mod textarea;
pub mod tokenizer;
pub mod ui;
