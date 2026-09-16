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

### Added

- **Device code sign-in for ChatGPT Codex** (`docs/chatgpt.md`). A machine
  with no browser — an SSH session, a container — could not sign in to the
  ChatGPT subscription, since its only flow handed the user a link a browser
  had to bring back to a local port. Enter on the `ChatGPT Codex` row in
  `/login` now asks how to sign in first, `Browser login (default)` or
  `Device code login (headless)`, and the device row runs Codex's own
  device-code flow: a one-time code shown beside `auth.openai.com/codex/device`
  on the same page GitHub Copilot's code lands on, counted down while OpenAI
  is polled for the approval, and the same refresh token stored at the end —
  `/model`, the ✓ marks and the next launch need nothing new. Esc from either
  sign-in page returns to the choice with the row just tried highlighted.
  `ALTER_ZERO_OPENAI_ISSUER` points both flows at another auth server (a
  fork's own, or the smoke suite's local stub, which drives the whole flow
  offline).

### Changed

- The OpenAI subscription provider is named **ChatGPT Codex** in `/login`
  and the docs (it was `OpenAI (ChatGPT)`): it is the ChatGPT seat reached
  the way Codex reaches it, and the old name read as a second OpenAI API-key
  provider. The provider id, its `OPENAI_CHATGPT_REFRESH_TOKEN` variable and
  every stored selection are unchanged, so nothing needs signing in again.

### Fixed

- **A picture rewritten in place shows its new bytes.** Ask the agent to
  download a picture, then to turn it black and white: the conversion
  happened, the agent read the file again, and the new inline cell still
  showed the colour version — while Ctrl+O showed the conversion. The
  encoded pictures are cached per placement, and a placement is the file's
  path and cell size, so the same file rewritten at the same size was served
  the entry encoded before the change. An encoding now remembers the file's
  size and mtime it was made from — the two facts the upload caches already
  key on — and a file that no longer matches is encoded afresh
  (`docs/images.md`, *A file rewritten in place*; `smoke.sh` Phase 107d).

## [0.2.0] - 2026-09-16

### Added

- **Fast mode for ChatGPT models** — codex's `/fast`, ported
  (`docs/fast-mode.md`). A model whose ChatGPT listing names a speed tier
  (every model on a signed-in account today: `Fast — 1.5x speed, increased
  usage`, `2x` on `gpt-6-astra`) can be switched to priority processing with
  `/fast`: the request carries `service_tier: "priority"` beside codex's
  `x-codex-routing-hint` header, the footer shows `fast` after the thinking
  mode, the toast repeats the backend's own cost statement, and the choice
  persists per directory beside the model selection (`config.json`'s new
  `speed` blob) and carries across a `/model` switch to another model that
  lists the tier. A model listing more than one tier cycles through each; one
  listing none answers `/fast` with a `does not support fast mode` toast, as
  Ctrl+T does for a non-reasoner. A subagent pinned to another model and the
  auto-mode classifier run at standard speed. Existing installs probe the
  listing once in the background at their next launch, exactly as the
  vision field did when it arrived, and record what they learn.

### Changed

- **A conversation remembers its model.** Every session's rollout now
  records the model it runs on — with the file, and again on each `/model`
  pick, Ctrl+T cycle and capability probe — and `/resume`, `--resume` and
  `--continue` bring that model back instead of the directory's current
  entry. Two alter-zero instances in the same directory can each run their
  own model: a `/model` pick in one still sets what a *new* session there
  starts on, but the other instance keeps the model it has, and resuming
  either conversation later reopens it on the model it was on. An
  `ALTER_ZERO_MODEL`/`ALTER_ZERO_PROVIDER` pin still wins for the run, and a
  recorded model whose provider has no key on this machine is reported with
  a `Can't resume on …` toast rather than silently swapped. The record
  carries the model's speed tier too, so a conversation switched to `/fast`
  comes back on it (`docs/session-model.md`).

## [0.1.2] - 2026-09-15

### Changed

- The `/resume` picker scrolls like the `/model` list: on a list taller
  than the screen the highlighted session rides the middle row, so the
  sessions above and below it stay in view and each ↑/↓ scrolls the next
  one in. It used to pin the highlight to the edge it had crossed, so
  scrolling down showed one new row at the bottom and nothing beyond it,
  and scrolling back up moved the highlight over the rows already on
  screen without scrolling at all.
- The `bash` tool's `run_in_background` now says what the flag is *for* — a
  dev server, a watch build, a full test suite — rather than only how it
  works, and notes that it defaults to false (the `agent` tool's own
  `run_in_background` defaults the other way). And both background launches,
  a command and a subagent alike, now answer with *you will be notified with
  the final output when it finishes* in place of *you will be re-invoked*: a
  result to expect rather than an internal event to brace for. Between them,
  fewer long commands run in the foreground, and fewer rounds are spent
  re-reading the interim output file for a result that arrives on its own.

### Fixed

- Telemetry now reports an update the day it happens. The daily ping names
  the app version, but its once-a-day throttle was keyed on the day alone, so
  the first launch after `alter-zero update` sent nothing until midnight UTC
  — and the collector's `INSERT OR IGNORE` dropped a same-day ping outright
  (answering `204`), so even a forced one left the day's row, and the
  dashboard's Versions panel, on the old version. `telemetry.json` now records
  `last_ping_version` beside `last_ping_day`, a launch on a different version
  pings again, and the collector's `(day, id)` row is an upsert that moves the
  install onto its new version. The install id survives an update, so an
  update is still one user, never a new install. Redeploy the collector
  (`cd telemetry && npx wrangler deploy`); no database migration is needed.

## [0.1.1] - 2026-09-15

### Added

- **Venice as a direct provider.** `/login` → **Use an API key** →
  **Venice** takes a key from your own Venice.ai account, and `/model` lists
  Venice's catalog straight from `api.venice.ai` — the same private,
  uncensored models the Agent Zero API reaches through its proxy, over the
  same request: the thinking mode still drives
  `venice_parameters.disable_thinking`, the session's `prompt_cache_key`
  still pins Venice's implicit prompt cache, and each model's context
  window, vision, and reasoning support are still read off its `model_spec`
  record. Set `VENICE_API_KEY` (or `ALTER_ZERO_PROVIDER=venice` with the key
  in `.env`) to launch on it directly.

### Fixed

- The running `bash` cell's `+N lines (Ns)` footer now counts from the
  moment the command started instead of copying the status indicator's
  turn timer, so a command launched a minute into a turn opens on `(0s)`
  rather than `(60s)`. A subagent's session view counts its own running
  command the same way, from that call's start rather than the agent's
  whole runtime.

## [0.1.0] - 2026-09-13

**Alter Zero Initial Release**

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
- **One-line install.** `curl -fsSL https://raw.githubusercontent.com/linuztx/alter-zero/main/install.sh | sh`
  picks the build for the machine, verifies its SHA-256 against the
  published checksum, and installs `alter-zero` into `~/.local/bin`.
- **Update notice.** Once a day the app asks the repository's releases page
  whether a newer version is out — one request, carrying nothing about you
  — and says so under the banner, naming the release and `alter-zero
  update`, which installs it over the running binary through the same
  checksum-verified installer. Off with the **Update check** setting or
  `ALTER_ZERO_UPDATE_CHECK=0`; `TELEMETRY.md` states the request in full.
- **Release tooling.** A CI workflow running the project's gate (`fmt`,
  `clippy`, `test`, `doc`), the smoke suite, and the release tooling's own
  tests; a release workflow that, on a `vX.Y.Z` tag, verifies the version
  against this changelog, builds Linux (x86_64, arm64) and macOS (Intel,
  Apple silicon) archives with SHA-256 checksums, and publishes a GitHub
  release whose notes come from this file — driven end to end by
  `scripts/release.sh`, which also rehearses a release locally.

[Unreleased]: https://github.com/linuztx/alter-zero/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/linuztx/alter-zero/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/linuztx/alter-zero/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/linuztx/alter-zero/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/linuztx/alter-zero/releases/tag/v0.1.0
