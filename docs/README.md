# Documentation

One page per design decision. `AGENTS.md` says what to run before a change;
`CLAUDE.md` maps the architecture and the four runtime invariants; this index
says where each mechanism lives. A new feature gets a page here, listed below
— a page that no longer matches the code is a bug.

## Start here

- [`design.md`](design.md): Inline Conversation TUI — Design

## Architecture

How the crate is put together: module layout, the async loop, the context, token and memory rules.

- [`architecture/async-rewrite.md`](architecture/async-rewrite.md): Async runtime rewrite (codex-style)
- [`architecture/compact.md`](architecture/compact.md): /compact — summarize the conversation to free context
- [`architecture/context-usage.md`](architecture/context-usage.md): `/context` — what fills the context window
- [`architecture/context.md`](architecture/context.md): The conversation context, vision attachments, and the Ctrl+D debug view
- [`architecture/dummy-backend.md`](architecture/dummy-backend.md): The dummy backend and its scenario registry
- [`architecture/environment.md`](architecture/environment.md): Environment context in the system prompt — Design
- [`architecture/herdr.md`](architecture/herdr.md): herdr support — the pane's state, reported by the agent itself
- [`architecture/memory.md`](architecture/memory.md): Resident memory: what the app holds, and what `/model` used to cost
- [`architecture/module-layout.md`](architecture/module-layout.md): Module layout: `app/`, `ui/`, `tui/`, and `stream/`
- [`architecture/token-estimate.md`](architecture/token-estimate.md): Token estimates, corrected by the provider
- [`architecture/unfinished-replies.md`](architecture/unfinished-replies.md): A turn never ends on a reply that isn't one

## UI

What the screen is made of: the live region, streaming renderers, themes, the full-screen pages and the transcript pager.

- [`ui/copy.md`](ui/copy.md): `/copy` — copy the last response to the clipboard
- [`ui/diff.md`](ui/diff.md): `/diff` — review the project's Git changes
- [`ui/donate.md`](ui/donate.md): `/donate` — the crypto donation page
- [`ui/export.md`](ui/export.md): `/export` — the conversation as plain text
- [`ui/flicker.md`](ui/flicker.md): Flicker-free rendering — scrollback commits join the draw frame
- [`ui/footer.md`](ui/footer.md): The session-context footer under the input box
- [`ui/header.md`](ui/header.md): The startup header banner
- [`ui/images.md`](ui/images.md): Inline images
- [`ui/inline-diff.md`](ui/inline-diff.md): Character-level diff marking in the `edit`/`write` cell
- [`ui/links.md`](ui/links.md): Clickable links — OSC 8 hyperlinks for wrapped URLs
- [`ui/long-lines.md`](ui/long-lines.md): Very long output lines — bounded rows, honest counts
- [`ui/markdown.md`](ui/markdown.md): Assistant markdown rendering
- [`ui/mascot.md`](ui/mascot.md): The banner mascot and the `/mascot` picker
- [`ui/overlay-repaint.md`](ui/overlay-repaint.md): Overlay repaint — copying text out of Ctrl+O / Ctrl+D while a turn runs
- [`ui/shortcuts.md`](ui/shortcuts.md): `?` shortcuts band — keyboard-shortcut help below the box
- [`ui/slow-stream.md`](ui/slow-stream.md): Slow streams — a few tokens a second through the inline pipeline
- [`ui/spinner.md`](ui/spinner.md): The status spinner styles and the `/spinner` picker
- [`ui/status-indicator.md`](ui/status-indicator.md): Status indicator (codex / Claude-Code style)
- [`ui/strip-flow.md`](ui/strip-flow.md): Strip flow — the streaming strip freezes instead of vanishing
- [`ui/table-streaming.md`](ui/table-streaming.md): Streaming GFM tables (buffered whole, previewed live)
- [`ui/theme.md`](ui/theme.md): Colour themes and the `/theme` picker
- [`ui/thinking-stream.md`](ui/thinking-stream.md): The thinking stream
- [`ui/timestamps.md`](ui/timestamps.md): Timestamps in the Ctrl+O transcript — Design
- [`ui/tips.md`](ui/tips.md): Spinner tips — a hint under the status line while a turn runs
- [`ui/toast.md`](ui/toast.md): Transient toasts
- [`ui/tool-groups.md`](ui/tool-groups.md): Collapsed read/search groups — the non-verbose tool view
- [`ui/tool-pulse.md`](ui/tool-pulse.md): The running bullet's blink
- [`ui/tool-streaming.md`](ui/tool-streaming.md): Live-streaming `bash` output — the running tail + collapsed peek
- [`ui/tool-view-performance.md`](ui/tool-view-performance.md): Ctrl+O performance: the incremental transcript cache and the atomic switch
- [`ui/view-flow.md`](ui/view-flow.md): View flow — long inline-view content flows into scrollback
- [`ui/write-streaming.md`](ui/write-streaming.md): Streaming a `Write` while the model writes it

## Input

The composer and everything typed into it: textarea, history, pickers, queues, shell commands and interrupts.

- [`input/backtrack.md`](input/backtrack.md): Esc Esc — edit a previous message (backtrack)
- [`input/file-search.md`](input/file-search.md): `@` file-path mentions — a file picker below the box
- [`input/history-persistence.md`](input/history-persistence.md): Cross-session input history — persisting the ↑/↓ + Ctrl+R history to disk
- [`input/history-search.md`](input/history-search.md): Ctrl+R history search — reverse incremental search over submitted inputs
- [`input/image-paste.md`](input/image-paste.md): Ctrl+V image paste
- [`input/input-history.md`](input/input-history.md): ↑/↓ input history — shell-style recall of submitted messages
- [`input/interrupt.md`](input/interrupt.md): Esc interrupts the in-flight turn — Design
- [`input/paste.md`](input/paste.md): Pasted-content placeholders
- [`input/queue.md`](input/queue.md): Queueing messages while a turn runs
- [`input/shell-command.md`](input/shell-command.md): `!` for shell commands — run a local command from the composer
- [`input/shift-enter.md`](input/shift-enter.md): Shift+Enter inserts a newline — Design
- [`input/skill-mentions.md`](input/skill-mentions.md): `$` skill mentions — a skill picker below the box
- [`input/textarea.md`](input/textarea.md): Textarea — a codex-style editable input

## Tools

What the model can do: the tool loop, bash and its terminal engine, MCP, hooks, skills, tasks, background shells and the scratchpad.

- [`tools/ask.md`](tools/ask.md): The `AskUserQuestion` tool — asking the user mid-turn
- [`tools/background.md`](tools/background.md): Background shells — `wait: 0`, a `wait` that passes, Ctrl+B, and the ↓ manager
- [`tools/bash-tools.md`](tools/bash-tools.md): The bash tools — a real terminal for every command, one tool per action
- [`tools/hooks-menu.md`](tools/hooks-menu.md): The `/hooks` menu — the hooks browser
- [`tools/hooks.md`](tools/hooks.md): Lifecycle hooks
- [`tools/interactive-shell.md`](tools/interactive-shell.md): Interactive shells — the terminal engine behind the bash tools
- [`tools/mcp.md`](tools/mcp.md): MCP servers — the `/mcp` manager and the `mcp__server__tool` tools
- [`tools/parallel-tools.md`](tools/parallel-tools.md): Parallel tool calls — the visible batch + `⎿ Waiting…`
- [`tools/scratchpad.md`](tools/scratchpad.md): The session scratchpad — Design
- [`tools/skills.md`](tools/skills.md): The `Skill` tool
- [`tools/task-tools.md`](tools/task-tools.md): The task tools — a live checklist instead of tool cells
- [`tools/tools.md`](tools/tools.md): LLM tool calling — `bash` and its companions / `read` / `write` / `edit`
- [`tools/tty-detach.md`](tools/tty-detach.md): Detaching subprocesses from the controlling terminal

## Agents

Subagent definitions, runs and progress, and the agent session view.

- [`agents/agent-context-gauge.md`](agents/agent-context-gauge.md): The agent session view gauges the viewed agent's context
- [`agents/agent-progress.md`](agents/agent-progress.md): A subagent's progress updates — on its row, and in `agentoutput`
- [`agents/agent-tool.md`](agents/agent-tool.md): The `Agent` tool — Claude-Code-style subagents
- [`agents/agent-tools.md`](agents/agent-tools.md): The agent tools — every agent has an id, and the lead can work it
- [`agents/agent-view-streaming.md`](agents/agent-view-streaming.md): The agent session view streams like the main one
- [`agents/subagents.md`](agents/subagents.md): Subagent definitions — `agents/*.md`

## Providers

Signing in, the wire formats, model catalogs, prompt caching, reasoning and subscription usage.

- [`providers/antigravity.md`](providers/antigravity.md): Google Antigravity as a provider
- [`providers/callback-page.md`](providers/callback-page.md): The browser sign-in page
- [`providers/chatgpt.md`](providers/chatgpt.md): ChatGPT Codex as a provider
- [`providers/claude.md`](providers/claude.md): Anthropic as a provider
- [`providers/cline.md`](providers/cline.md): Cline as a provider
- [`providers/copilot.md`](providers/copilot.md): Signing in: subscriptions, API keys, and GitHub Copilot
- [`providers/fast-mode.md`](providers/fast-mode.md): `/fast`, `/ultrafast`, … — codex's speed tiers as commands
- [`providers/llm.md`](providers/llm.md): The real LLM backend and the `/model` picker
- [`providers/local-models.md`](providers/local-models.md): Local models — `/login` → **Sign in with a local model**
- [`providers/offline.md`](providers/offline.md): Waiting for a lost connection
- [`providers/ollama.md`](providers/ollama.md): Ollama as a provider
- [`providers/prompt-caching.md`](providers/prompt-caching.md): Prompt caching and real usage accounting
- [`providers/reasoning.md`](providers/reasoning.md): Ctrl+T thinking modes (reasoning effort)
- [`providers/usage.md`](providers/usage.md): `/usage` — what is left of your subscription
- [`providers/user-providers.md`](providers/user-providers.md): Your own providers — `~/.alter-zero/providers.toml`
- [`providers/venice.md`](providers/venice.md): Venice as a provider

## State

What persists and what is trusted: permissions, settings, checkpoints, rollouts, sessions, credentials and project config.

- [`state/checkpoint.md`](state/checkpoint.md): Checkpoints — reset the code, not just the transcript
- [`state/config-review.md`](state/config-review.md): Reviewing hooks and project MCP servers before they run
- [`state/init.md`](state/init.md): `/init` — generate an AGENTS.md contributor guide (codex's `/init`)
- [`state/per-directory-state.md`](state/per-directory-state.md): Per-directory state — `/model`, `/settings`, `/mascot` and `/spinner` remember the directory
- [`state/permissions.md`](state/permissions.md): Tool permission requests
- [`state/project-config.md`](state/project-config.md): Project-level `.alter-zero` config — hooks, MCP servers, skills, agents
- [`state/project-doc.md`](state/project-doc.md): AGENTS.md in the context window — codex's project doc (Design)
- [`state/resume.md`](state/resume.md): /resume — pick up a saved session (rollout files + session picker)
- [`state/rollout-records.md`](state/rollout-records.md): What the rollout records about each call and request
- [`state/secrets.md`](state/secrets.md): `/secrets` — credentials the agent uses but never sees
- [`state/session-model.md`](state/session-model.md): Per-session model — a conversation remembers the model it ran on
- [`state/settings.md`](state/settings.md): `/settings` — the session settings menu

## CLI

The pre-TUI command surface: flags, the `[PROMPT]` shortcut, headless mode and the `mcp` subcommand family.

- [`cli/cli.md`](cli/cli.md): CLI — `--help`, the `[PROMPT]` shortcut, `--continue`, `--resume [id]`, and the exit hint
- [`cli/headless.md`](cli/headless.md): Headless mode — `alter-zero -p`
- [`cli/mcp-cli.md`](cli/mcp-cli.md): CLI MCP install — `alter-zero mcp add/remove/list/get/add-json`

## Operations

Shipping it: releases, Docker, AUR, npm, telemetry, updates, the smoke suite, build time and binary size.

- [`ops/aur.md`](ops/aur.md): The Arch Linux packages — `aur/`
- [`ops/binary-size.md`](ops/binary-size.md): Binary size — the binary a release ships
- [`ops/build-time.md`](ops/build-time.md): Build time — `cargo build --release`
- [`ops/docker.md`](ops/docker.md): The headless Kali container
- [`ops/npm.md`](ops/npm.md): The npm packages — `npm/`
- [`ops/release.md`](ops/release.md): Releases — the tag, the workflow, the assets, the notes
- [`ops/smoke.md`](ops/smoke.md): The smoke suite — `scripts/smoke.sh`
- [`ops/telemetry.md`](ops/telemetry.md): Telemetry — counting users by day and country
- [`ops/update.md`](ops/update.md): Update notice — the once-a-day release check and `alter-zero update`

## Credits

- [`credits.md`](credits.md): Technical credits
