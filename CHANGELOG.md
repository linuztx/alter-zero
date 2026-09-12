# Changelog

All notable changes to Alter Zero are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A release's notes on GitHub are generated from its section below
(`scripts/release.sh notes`, see `docs/release.md`), so the entry written
here is the entry users read. Changes land under **Unreleased** as they
merge; `scripts/release.sh prepare X.Y.Z` rolls that section into a dated
release heading when a version is cut.

## [Unreleased]

## [0.1.0] - 2026-09-12

The first release: an autonomous coding agent that lives in the terminal,
as a single self-contained binary for Linux and macOS.

### Added

- **An agent that works in your project.** The model reads files, writes
  and edits them, and runs shell commands through an agentic tool loop.
  Parallel tool batches are announced before they run, command output
  streams live into the conversation and folds into an expandable preview,
  and long-running commands move to the background with **Ctrl+B** (or
  start there), with a manager band on **↓** from an empty composer.
- **Control over what runs.** An inline approval prompt shows the whole
  file, diff, or command before it executes, with "allow once", a session
  or per-project rule, or a rejection carrying your instructions (**Tab**).
  Four permission modes — `manual`, `edit`, `auto` (an LLM safety
  classifier reviews routine commands), `master` — cycle with
  **Shift+Tab** and persist per project. Shell children are detached from
  the terminal so a `sudo` prompt fails fast instead of hijacking the UI.
- **Checkpoints and rewind.** Per-directory filesystem checkpoints snapshot
  the working tree into an isolated git store each turn; **Esc Esc** steps
  back to an earlier message and restores the files with it.
- **Extensibility.** Subagents defined in `agents/*.md` (built-in
  `general-purpose` and `explore`), each with its own context, model, and
  tool allowlist, and an inline session view to chat with a running one;
  skills as `SKILL.md` folders with a `$` mention picker, the `/skills`
  browser, and a built-in `skill-creator`; MCP servers over stdio, HTTP,
  and SSE (OAuth included) managed with `/mcp` and `alter-zero mcp`;
  Claude Code-compatible lifecycle hooks in `hooks.json`; `AGENTS.md`
  project instructions with `/init` to write one; structured task lists
  with a live checklist; mid-turn questions to the user; a per-session
  scratchpad for temporary files.
- **Project trust.** Project-level `.alter-zero/` hooks and MCP servers are
  fingerprinted and inert until approved with `/trust`.
- **Model providers.** OpenAI-compatible Chat Completions (OpenRouter, the
  Agent Zero API, Ollama Cloud), GitHub Copilot by device-code sign-in,
  OpenAI ChatGPT by browser sign-in over the Responses API, Anthropic by
  API key or Console sign-in over the Messages API, and local Ollama over
  its native `/api/chat`. `/login` connects, `/model` picks a model, each
  provider's catalog reports context windows, vision, and reasoning
  support, **Ctrl+T** cycles the thinking mode, and prompt caching is
  requested wherever the provider offers it.
- **Context management.** A footer gauge of the context window, `/compact`
  with automatic compaction past 90%, and the **Ctrl+D** view of exactly
  what the next request carries (including the classifier's own context).
- **A native terminal UI.** Conversation flows into the terminal's real
  scrollback under a pinned composer; streaming Markdown with tables and
  syntax-highlighted code; collapsible reasoning; clickable URLs and file
  paths (OSC 8); inline images in kitty, iTerm2, and sixel terminals with a
  half-block fallback; **Ctrl+V** image paste; the **Ctrl+O** full
  transcript; a readline-style composer with **Shift+Enter** newlines,
  cross-session input history, and **Ctrl+R** search; `@` file picker and
  `!` shell commands; messages that steer the running turn (**Enter**) or
  queue a follow-up (**Tab**).
- **Looks.** Eleven colour themes (`/theme`), six banner mascots
  (`/mascot`), nine spinner styles (`/spinner`), and a searchable
  `/settings` menu, all remembered per directory.
- **Sessions and the command line.** Every conversation is recorded;
  `/resume` opens a searchable picker, `--continue` reopens the latest
  session in the directory, `--resume <id>` a specific one, and a quoted
  `[PROMPT]` argument starts a turn directly. `--help` and `--version` are
  available, as is the `mcp` subcommand family.
- **Telemetry, disclosed.** One anonymous daily ping (install id, version,
  OS, architecture) described in full in `TELEMETRY.md`, shown once at
  startup, and switched off with the **Telemetry** setting,
  `ALTER_ZERO_TELEMETRY=0`, or `DO_NOT_TRACK=1`.
- **Release tooling.** A CI workflow running the project's gate (`fmt`,
  `clippy`, `test`, `doc`), the smoke suite, and the release tooling's own
  tests; a release workflow that, on a `vX.Y.Z` tag, verifies the version
  against this changelog, builds Linux (x86_64, arm64) and macOS (Intel,
  Apple silicon) archives with SHA-256 checksums, and publishes a GitHub
  release whose notes come from this file — driven end to end by
  `scripts/release.sh`, which also rehearses a release locally.

[Unreleased]: https://github.com/linuztx/alter-zero/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/linuztx/alter-zero/releases/tag/v0.1.0
