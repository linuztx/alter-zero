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
//! - [`ask`]         — the `AskUserQuestion` tool: the question shapes, the
//!   user's decision and its result texts, and the gate the tool thread
//!   blocks on while the modal asks (see `docs/ask.md`).
//! - [`background`]  — background shell processes: the registry behind
//!   `run_in_background`, Ctrl+B, and the ↓ manager (boundary; see
//!   `docs/background.md`).
//! - [`checkpoint`]  — per-turn working-directory snapshots in an isolated git
//!   store so `/resume` and the Esc-Esc backtrack can reset the code, not just
//!   the transcript (pure mapping + boundary store; see `docs/checkpoint.md`).
//! - [`cli`]         — the `--continue`/`--resume` argument parse, usage text,
//!   and the exit-hint shape (pure; resolution/printing stay in `main.rs` —
//!   see `docs/cli.md`).
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
//! - [`links`]       — clickable links: bare-URL detection, the URL interner,
//!   the per-cell link carrier, and the OSC 8 escape framing that lets a
//!   wrapped URL open whole (see `docs/links.md`).
//! - [`llm`]         — the real OpenAI-compatible backend: provider config, the
//!   streaming client, the `/v1/models` listing, and the `ReplySource` bridge
//!   (the I/O boundary for a real model; pure cores unit-tested).
//! - [`hooks`]       — lifecycle hooks: the `hooks.json` format, which
//!   handlers an event selects, the JSON payload each writes to a handler's
//!   stdin and the verdict its stdout is parsed back into (pure; the spawn is
//!   [`llm::hooks`] — see `docs/hooks.md`).
//! - [`markdown`]    — the pure block parser for assistant replies: split prose
//!   from fenced code blocks and detect ATX headings (see `docs/markdown.md`).
//! - [`mcp`]         — MCP servers' pure model: the `mcp.json` config format,
//!   the `mcp__server__tool` naming contract, the JSON-RPC/SSE protocol
//!   shapes, and the `/mcp` manager's snapshot vocabulary (the I/O client is
//!   [`llm::mcp`]; see `docs/mcp.md`).
//! - [`paste`]       — paste-burst detection and the large-paste/image
//!   placeholder helpers.
//! - [`permission`]  — tool permission requests: the pure prompt vocabulary
//!   (titles, options, command scopes, session rules) and the gate the backend
//!   thread blocks on while the user decides (see `docs/permissions.md`).
//! - [`project_doc`] — AGENTS.md discovery: codex's project doc collected
//!   root→cwd under a 32 KiB cap and rendered as the user-instructions
//!   context fragment (see `docs/project-doc.md`).
//! - [`scratchpad`]  — the session's own temp layout: the agent's scratchpad
//!   (the directory its system prompt sends every temporary file to) and the
//!   background shells' `tasks` dir beside it (pure; see
//!   `docs/scratchpad.md`).
//! - [`session`]     — the `/resume` rollout-file format: serialize/parse the
//!   JSONL session record, the picker preview + humanized age.
//! - [`settings`]    — the `/settings` menu's pure model: the knob inventory,
//!   each one's value cycle, and the `settings.json` format they persist in
//!   (see `docs/settings.md`).
//! - [`skills`]      — the `Skill` tool's pure model: the `SKILL.md`
//!   frontmatter parse, the budgeted listing the model chooses from, the
//!   rendered body a call returns, and the registry the boundary fills
//!   (see `docs/skills.md`).
//! - [`stream`]      — the backend seam: the `StreamEvent` reply protocol, the
//!   `ReplySource` trait the event loop depends on, and the built-in offline
//!   `DummyAi` whose scripted turns drive the smoke suite (see
//!   `docs/dummy-backend.md`).
//! - [`tasks`]       — the task tools' pure model: the `taskcreate` /
//!   `taskget` / `tasklist` / `taskupdate` store, every result string, and
//!   the shared registry the executor and the loop hold together (see
//!   `docs/task-tools.md`).
//! - [`subprocess`]  — the shared detached `sh -c` spawn (setsid binary →
//!   helper re-exec → attached): every shell runner's child is severed from
//!   the controlling terminal so a `/dev/tty` password prompt (`sudo`) fails
//!   fast instead of hijacking the TUI.
//! - [`term`]        — the custom inline viewport (dynamic-height live region;
//!   I/O).
//! - [`textarea`]    — the grapheme-aware editable composer.
//! - [`tokenizer`]   — accurate token counting for the status tally (tiktoken
//!   `o200k_base`, ranks embedded; the count seam behind `app::count_tokens`).
//! - [`trust`]       — per-project trust for the `.alter-zero` project config
//!   layer: the content-hash fingerprint, the `trust.json` format, and the
//!   project-file path builders (pure; see `docs/project-config.md`).
//! - [`ui`]          — pure rendering helpers (word-wrap, message lines, live
//!   region), split one module per area with every styling constant in
//!   `ui::theme` (see `docs/module-layout.md`).

/// The product's own name — what the agent is called wherever the app speaks
/// about itself to the user: the startup banner's title
/// (`ui::theme::HEADER_NAME`) and the `AskUserQuestion` cell's headline
/// (`ask::ANSWERED_HEADLINE`, `docs/ask.md`). One place, because a name that
/// lives in several literals is a name that ends up disagreeing with itself —
/// the ported reference's `User answered Claude's questions:` being exactly
/// that bug.
pub const APP_NAME: &str = "Alter Zero";

pub mod agents;
pub mod app;
pub mod ask;
pub mod background;
pub mod checkpoint;
pub mod cli;
pub mod clipboard;
pub mod context;
pub mod file_search;
pub mod frame;
pub mod highlight;
pub mod history;
pub mod hooks;
pub mod links;
pub mod llm;
pub mod markdown;
pub mod mcp;
pub mod paste;
pub mod permission;
pub mod project_doc;
pub mod scratchpad;
pub mod session;
pub mod settings;
pub mod skills;
pub mod steer;
pub mod stream;
pub mod subprocess;
pub mod tasks;
pub mod term;
pub mod textarea;
pub mod tokenizer;
pub mod trust;
pub mod ui;
