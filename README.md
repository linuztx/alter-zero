<div align="center">

<img src="assets/banner.svg" alt="Alter Zero" width="100%">

# Alter Zero

**An autonomous AI coding agent that lives in your terminal.**

It reads your code, edits files, runs commands, delegates to subagents and asks before it changes anything,
all inline with your scrollback, on whichever model you already have access to.

[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange?logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS-blue)](#quick-start)
[![Version](https://img.shields.io/badge/version-0.1.0-informational)](#quick-start)
[![License](https://img.shields.io/badge/license-Apache--2.0-green)](LICENSE)
[![Support](https://img.shields.io/badge/support-%2Fdonate-ff69b4)](#support-the-project)

[Quick start](#quick-start) ·
[Features](#features) ·
[Models & providers](#bring-your-own-model) ·
[Commands](#slash-commands) ·
[Shortcuts](#keyboard-shortcuts) ·
[Support](#support-the-project) ·
[Acknowledgements](#acknowledgements)

</div>

---

## Why Alter Zero

- **A real agent, not a chat box.** It reads, edits, runs and verifies in small checked steps, streaming every tool's output live as it works.
- **You stay in charge.** Every file change is shown in full before it happens. Approve once, approve for the session, or say no with instructions.
- **Any model you like.** Sign in with a subscription you already pay for, paste an API key, or run local models with Ollama.
- **Extend it the way you already do.** Subagents, skills, hooks and MCP servers use Claude Code's file formats, so anything you have written for it works here unchanged.
- **Light and fast.** A single native binary that is comfortable idling in your terminal all day.

## Quick start

You need a Rust toolchain ([rustup](https://rustup.rs) installs the pinned version automatically) and a modern terminal.

```bash
git clone https://github.com/linuztx/alter-zero.git
cd alter-zero
cargo install --path .
alter-zero
```

The first launch works with no account at all: until you sign in, Alter Zero runs an offline demo that plays scripted turns showing every kind of cell. Then, inside the app:

1. **`/login`** picks how you sign in: a subscription, an API key, or a local Ollama server.
2. **`/model`** lists that provider's models and remembers your choice for this directory.
3. Ask for something. `?` shows the keyboard shortcuts and `/` the commands whenever you need them.

Prefer environment variables? Point it at a provider before launch and the real backend takes over automatically:

```bash
export OPENROUTER_API_KEY=sk-...          # or <PROVIDER>_API_KEY
export ALTER_ZERO_PROVIDER=openrouter
export ALTER_ZERO_MODEL=<model id>
alter-zero
```

Keys and tokens are saved to `~/.alter-zero/.env`, outside any repository, so you sign in once.

## Bring your own model

| Provider | How you connect | Good to know |
| --- | --- | --- |
| **GitHub Copilot** | Subscription. Enter shows a one-time device code; approve it on GitHub and you are in. | Context window, vision and the reasoning levels are read from Copilot itself. |
| **OpenAI (ChatGPT)** | Subscription. A browser sign-in for ChatGPT Plus and Pro seats. | Nothing to paste; the browser hands the session straight back. |
| **Anthropic** | API key, or an account sign-in through the Anthropic Console. | Billed to your API organisation either way. |
| **Agent Zero API** | API key. | Venice.ai's private models through the Agent Zero proxy, with a free daily quota for A0T token holders. |
| **OpenRouter** | API key. | Hundreds of models from every major lab behind one key. |
| **Ollama** | No key. Point it at your server, or accept the default. | Runs local models on your own machine; nothing leaves it. |
| **Ollama Cloud** | API key. | The same open models on Ollama's hosted GPUs. |

Whatever you pick, the footer keeps you informed: the model, its thinking mode, and a live gauge of how much of its context window the conversation has used. Reasoning models get a **Ctrl+T** effort ladder read from the provider, models that cannot see images say so instead of failing a turn, and requests are shaped for prompt caching so long sessions stay affordable.

## Features

### The agent does the work

- **File tools that show their work.** Reads, writes and edits render as numbered, syntax-highlighted cells with green and red diff tints, and the characters that actually changed on a line are highlighted so you can review at a glance.
- **Commands that stream.** A running command tails its output live; long ones fold to a compact peek you can expand at any time.
- **Parallel and background work.** A batch of tool calls is announced up front so you see the whole plan. Long-running commands move to the background with **Ctrl+B** and report back when they finish; **↓** opens a manager to watch or stop them.
- **A visible plan.** When the agent breaks a job into tasks, a live checklist sits under the status line and follows along as it works.
- **It asks when it should.** Multiple-choice questions arrive as an inline form, with free-text answers, previews and notes when the agent needs a decision from you.

### You stay in control

- **Approval before change.** A write, an edit or a command pauses the turn with an inline prompt showing the full content or diff. Answer **Yes**, **Yes for this session** (or *don't ask again* for that command prefix), or **No**. **Tab** rejects with instructions the agent will read, and **Ctrl+E** asks it to explain a command instead of running it.
- **Four permission modes.** `manual` asks for everything, `edit` lets file changes through, `auto` adds a silent safety reviewer that clears routine commands and asks about the rest, and `master` runs unattended. Cycle them with **Shift+Tab**; your rules are remembered per project.
- **Checkpoints and rewind.** Every turn can snapshot your working directory into an isolated store, never your own `.git`. **Esc Esc** steps back to an earlier message and restores both the conversation and your files to that point. Opt in per directory.
- **A private scratchpad.** The agent gets a session-only temp directory for throwaway scripts and notes, so it never litters `/tmp` or your project.
- **Trust before execution.** A project's own agents, hooks and MCP servers are listed but inert until you review and approve them with `/trust`.

### Delegate to subagents

The agent can hand work to side agents that run their own tool loop over a fresh context. Each type is a small markdown file in `~/.alter-zero/agents/` (or a project's `.alter-zero/agents/`), with a name, a description, an optional model and an optional tool allowlist; `general-purpose` and `explore` come built in and are yours to edit. Running agents appear in a roster under the composer where you can open one's live session, chat with it directly, queue it follow-up work, or stop it.

### Extend it

- **Skills.** Drop a `SKILL.md` folder into `~/.alter-zero/skills/` (or `.claude/skills/`) and the agent loads it on demand. Type `$` to mention one, or `/skills` to browse and toggle them. A built-in `skill-creator` teaches the agent to write new ones.
- **MCP servers.** Declare them in `mcp.json` (or a Claude-Code-style `.mcp.json`) over stdio, HTTP or SSE, including remote servers that need OAuth. `/mcp` manages them live and `alter-zero mcp add` from the shell.
- **Hooks.** Wire your own commands into the tool loop with `hooks.json`: refuse a call, rewrite its arguments, approve it, or hand the agent extra context. Every lifecycle event is covered, and `/hooks` lets you browse what is configured.
- **Project docs.** `AGENTS.md` files are read at every turn so the agent knows your conventions, and `/init` writes a first one for you.

### Sessions that persist

- **Every conversation is recorded.** Quit and the app tells you how to get back; `/resume` opens a searchable picker, `--continue` reopens the latest session in this directory, and `--resume <id>` a specific one.
- **Per-directory memory.** The model, settings, permission rules, skill choices and the look you pick are remembered for each project you work in.
- **Input history across sessions.** **↑/↓** recall earlier messages and **Ctrl+R** searches them, in this session and the ones before it.
- **Context that manages itself.** `/compact` summarises a long conversation, and the app does it automatically as the window fills.

### Built for the terminal

- **Rich, live rendering.** Markdown, tables and highlighted code stream in as they arrive. URLs and file paths are clickable links in terminals that support them.
- **Pictures, inline.** Paste a screenshot with **Ctrl+V** and it appears in the conversation; images the agent reads appear too, drawn natively in kitty, iTerm2 and sixel terminals and as half-block art everywhere else.
- **Thinking you can see.** A reasoning model's thoughts stream in a live block, then collapse into a one-line summary you can expand.
- **Two views on the truth.** **Ctrl+O** opens the full transcript with every tool expanded; **Ctrl+D** shows the exact context being sent to the model.
- **Keep talking while it works.** **Enter** mid-turn hands your message to the running turn, **Tab** queues a follow-up turn, and **Alt+↑** pulls it back to edit.
- **Your shell, one keystroke away.** Start a line with `!` to run a shell command right in the conversation. `@` opens a fuzzy file picker for paths.
- **Resize-proof.** The conversation reflows to any terminal width without flicker.

### Make it yours

- **`/theme`**: eleven colour themes, with the four Catppuccin flavours, One Dark, Dracula, Nord, Gruvbox, Solarized, Monokai, and an ANSI theme that follows your terminal. Each is previewed on real cells before you pick it.
- **`/mascot`**: six banner mascots, previewed live.
- **`/spinner`**: nine status-line spinner styles, previewed live. This and the mascot are remembered per project, so each one can wear its own.
- **`/settings`**: everything else, in one searchable menu. Thinking visibility, image display and size, automatic image resizing, error retries, tools, permission mode, checkpoints, auto-compact, project docs, hooks, skills, temperature and a tool-call budget.

## Slash commands

Type `/` to open the palette.

| Command | What it does |
| --- | --- |
| `/help` | List the available commands |
| `/clear` | Clear the conversation |
| `/copy` | Copy the last response to the clipboard |
| `/init` | Create an `AGENTS.md` contributor guide |
| `/compact` | Summarise the conversation to free up context |
| `/resume` | Resume a saved chat |
| `/model` | Switch the active model |
| `/login` | Add or update a provider sign-in |
| `/settings` | Open the settings menu |
| `/theme` | Choose the colour theme |
| `/mascot` | Choose the banner mascot |
| `/spinner` | Choose the status spinner style |
| `/hooks` | Browse the configured lifecycle hooks |
| `/skills` | Browse skills and enable or disable each one |
| `/mcp` | Manage MCP servers |
| `/trust` | Review and approve this project's config |
| `/donate` | Support the project with a crypto donation |
| `/quit` | Exit the app |

## Keyboard shortcuts

Press `?` in an empty composer to see these in the app.

| Key | Action |
| --- | --- |
| `Enter` | Send, or hand a message to the running turn |
| `Tab` | Queue a follow-up turn |
| `Alt+↑` | Pull a queued message back to edit |
| `Shift+Enter` / `Ctrl+J` | Insert a newline |
| `↑` / `↓` | Recall input history |
| `Ctrl+R` | Search input history |
| `Esc` | Interrupt the running turn |
| `Esc Esc` | Step back to a previous message and rewind |
| `Ctrl+O` | Open the full transcript |
| `Ctrl+D` | Show the context sent to the model |
| `Ctrl+T` | Cycle the thinking mode |
| `Shift+Tab` | Cycle the permission mode |
| `Ctrl+B` | Move a running command or agent to the background |
| `↓` (empty composer) | Manage background shells and agents |
| `Ctrl+V` | Paste an image |
| `!` | Run a shell command |
| `@` | Pick a file path |
| `$` | Mention a skill |
| `/` | Open the command palette |
| `Ctrl+W` / `Ctrl+U` / `Ctrl+K` | Delete the previous word, to the start of the line, to the end of the line |
| `Ctrl+C` | Clear the draft, then quit |

## Command line

```
Usage: alter-zero [OPTIONS] [PROMPT]
       alter-zero mcp <COMMAND>

Arguments:
  [PROMPT]        Send this message as the first turn

Options:
  -c, --continue  Continue the most recent conversation in this directory
  -r, --resume    Resume a conversation by id, or pick one from a list
  -h, --help      Print help
  -V, --version   Print version
```

`alter-zero "fix the failing test"` starts a session with that as its first turn, and `alter-zero -c "and now the docs"` sends one into the conversation you left off in.

## Where things live

Everything Alter Zero remembers is in `~/.alter-zero/`: your sign-ins, the model and settings for each directory, permission rules, recorded sessions, input history, your agents and skills, and your hooks and MCP servers. A project can carry its own `.alter-zero/` folder with `agents/`, `skills/`, `hooks.json` and `mcp.json` (Claude Code's `.claude/skills/` and `.mcp.json` are honoured too), all held behind `/trust` until you approve them.

## Support the project

Alter Zero is free and open source. If it earns a place in your terminal, a donation keeps the work going. `/donate` shows these addresses inside the app, each in a copyable box.

| Coin | Address |
| --- | --- |
| BTC (Bitcoin) | `bc1q68v53mjj2uxg9qs5ke55qh4gv7un8esttwmvm9` |
| ETH (Ethereum) | `0xaf7B6ac9BeeFDcfCd118701a00be960a592600CB` |
| SOL (Solana) | `9hWaV4rTqNfF1c6mGDSnksMY1fqKuDU9iKymfbeSqXrA` |

Send each coin over its own network only; a transfer on any other network cannot be recovered. Thank you.

## License

Alter Zero is released under the [Apache License 2.0](LICENSE). Copyright 2026 [linuztx](https://github.com/linuztx).

## Acknowledgements

Alter Zero stands on the work of others:

- [ratatui](https://ratatui.rs) and [crossterm](https://github.com/crossterm-rs/crossterm) for the terminal UI, [ratatui-image](https://github.com/benjajaja/ratatui-image) for the inline pictures, and [tokio](https://tokio.rs) for the event loop.
- [Claude Code](https://claude.com/claude-code) and [Codex CLI](https://github.com/openai/codex), whose interaction design this project studies and borrows from throughout.
- [syntect](https://github.com/trishume/syntect) and [two-face](https://github.com/CosmicHorrorDev/two-face) for syntax highlighting, and OpenAI's [tiktoken](https://github.com/openai/tiktoken) `o200k_base` vocabulary for the token counts.
- The [Catppuccin](https://catppuccin.com), [Dracula](https://draculatheme.com), [Nord](https://www.nordtheme.com), [Gruvbox](https://github.com/morhetz/gruvbox), [Solarized](https://ethanschoonover.com/solarized), [Monokai](https://monokai.nl) and One Dark palettes behind the themes.
- The [Model Context Protocol](https://modelcontextprotocol.io) and the providers that make the models reachable: GitHub Copilot, OpenAI, Anthropic, OpenRouter, Ollama, and Venice.ai through the Agent Zero community API.
- [Agent Zero](https://agent-zero.ai) ([GitHub](https://github.com/agent0ai/agent-zero)) for the free [A0T](https://www.agent-zero.ai/p/token/) inference credit that helped me test this project. In return, the Agent Zero API is a first-class provider here.

---

<div align="center">

Made by [linuztx](https://github.com/linuztx). Design notes for every feature live in [`docs/`](docs/).

</div>
