# Plugins — marketplaces, installs, and what a plugin adds to a session

A plugin is a folder someone else wrote that extends the agent — **skills**,
**slash commands**, **subagent types**, **lifecycle hooks** and **MCP
servers**, packaged together — and a **marketplace** is a git repository or a
local folder that lists plugins. Three formats are read as they are: the
vendor-neutral **Agent Plugins** specification (<https://agent-plugins.org>),
**Claude Code** plugins and **Codex** plugins. Two marketplaces are known out
of the box and more are added by source:

```
$ alter-zero plugin install frontend-design@claude-plugins-official
Fetching anthropics/claude-plugins-official…
Installed frontend-design@claude-plugins-official (v1.1.1)
  1 skill is ready for your next message.

$ alter-zero plugin marketplace add acme/agent-plugins
Added marketplace acme (12 plugins) from acme/agent-plugins
Install one with: alter-zero plugin install <plugin>@acme
```

Inside a session the same things are one page, `/plugins` (*The page*,
below); the command family has its own page, `docs/plugin-cli.md`.

The rule a user can hold in their head: **instructions are live from the next
message; anything that runs code waits for your review at the next launch.**

## What the references do (findings)

All three were read end to end — Codex at its tree of 2026-11-20, Claude
Code's documentation, the Agent Plugins 1.0.0 specification and its 1.1.0
draft — and both default marketplaces were inventoried file by file.

- **Agent Plugins 1.0.0** is a deliberately small portable floor. A plugin
  is a directory with a **root `plugin.json`** whose `$schema` is
  `https://agent-plugins.org/schemas/1.0.0/plugin.schema.json` and whose
  `name` is 1–64 characters of `[a-z0-9.-]`, alphanumeric at both ends, with
  no `--` and no `..`. The manifest is **closed**: an unknown top-level field
  is reported and ignored, a non-object `extensions` is reported and
  ignored, and any other violation rejects the whole plugin. Components sit
  at **fixed** places — `skills/<name>/SKILL.md` (direct children only) and
  a root **`mcp.json`** whose servers each declare a `type` (`stdio`,
  `streamable-http`, `sse`) — the plugin root is a stdio server's default
  working directory, and exactly two placeholders exist, `${PLUGIN_ROOT}`
  and `${PLUGIN_DATA}`. Distribution, installation, trust, hooks, agents and
  commands are left to the client.
- **Claude Code** reads `.claude-plugin/plugin.json` (optional — the
  marketplace entry can stand in for it) over default places: `skills/`,
  `commands/`, `agents/`, `hooks/hooks.json`, `.mcp.json` (wrapped in
  `mcpServers` or a bare map). Components are namespaced `plugin:name`;
  `${CLAUDE_PLUGIN_ROOT}` and `${CLAUDE_PLUGIN_DATA}` resolve in hook
  commands, MCP configs and markdown bodies. A marketplace is
  `.claude-plugin/marketplace.json`, and an entry's `source` is a relative
  path or an object (`github`, `url`, `git-subdir`, `npm`, `archive`,
  `command`).
- **Codex** reads `.codex-plugin/plugin.json` with a large `interface` block
  for its app, over `skills/`, `.mcp.json`, `hooks/hooks.json` and
  `.app.json` (ChatGPT connector ids, meaningless anywhere else). It looks
  for a root `plugin.json` first when its `$schema` is an agent-plugins.org
  one, then `.codex-plugin`, `.claude-plugin`, `.cursor-plugin`. A
  marketplace is `.agents/plugins/marketplace.json`; its entries carry no
  description — the plugin's own manifest does. Codex converts `commands/`
  into skills at install, ignores `agents/`, cannot read the `github` source
  form, has no enable or disable in its CLI, and **syncs its curated
  marketplace and every configured one over the network at every startup**.
- **The two marketplaces as they are today.**
  `anthropics/claude-plugins-official` lists 317 plugins in a 192 KB
  manifest: 55 live in the repository and 261 are other repositories, all
  but one pinned to a commit; one is found by running a program.
  `openai/plugins` lists 65 in 19 KB, 62 of them in the repository. Between
  them the in-repository plugins hold some 560 skills, 75 commands, 46
  agents, nine hooks files and 46 `.mcp.json` files.

## What we build

Codex's shape — marketplaces, `name@marketplace` ids, a tabbed `/plugins`
browser, a `plugin` command family — with these differences:

- **Every format, read natively, by one tolerant reader** — including what
  Codex drops: `agents/`, slash commands *as* commands,
  `${CLAUDE_PLUGIN_ROOT}` in `.mcp.json`, the `github` source form.
- **Nothing a plugin runs is run unseen.** Its hooks and MCP servers pass
  the same startup review every other hook and project server passes
  (`docs/config-review.md`), and the page that installs a plugin says what
  it will bring before and after.
- **Enable, disable, update and cancel everywhere** — on the page and in the
  CLI — with the real error on screen when something fails.
- **No network, no `git` and no catalog at startup** (*Memory*, below).

### One reader for three formats

A plugin folder's manifest is the first of these that exists
(`plugins::manifest`, `plugins::layout`):

1. a root `plugin.json` **whose `$schema` begins
   `https://agent-plugins.org/schemas/`** — Agent Plugins. The 1.0.0 and
   1.1.0 identifiers are read; any other is refused by name, as the
   specification requires. The closed manifest is validated rule by rule,
   and only the fixed places count (`skills/`, a root `mcp.json`, and for
   the 1.1 draft a root `hooks.json`). A root `plugin.json` without that
   `$schema` is somebody else's file and is ignored;
2. `.claude-plugin/plugin.json`;
3. `.codex-plugin/plugin.json`;
4. `.cursor-plugin/plugin.json`;
5. none — the **marketplace entry is the manifest** (Claude Code's rule),
   and failing that the folder's name is the plugin's.

For 2–4 nothing is required, unknown keys are ignored, and a key of the
wrong shape is dropped with a warning rather than failing the plugin. When
the manifest that won has no display fields and a `.codex-plugin/plugin.json`
sits beside it, that file's `interface` supplies them.

Components are found **by convention, and a manifest's paths add to it**
(a Claude manifest's `commands` and `agents` replace the default directory,
as its documentation says). A path must begin `./`, hold no `..`, and
resolve inside the plugin; a bad one is dropped with a warning.

| Component | Looked for at | Becomes |
| --- | --- | --- |
| skills | `skills/<name>/SKILL.md`, the manifest's `skills` paths, a lone root `SKILL.md` | a skill named `plugin:skill` |
| commands | `commands/**/*.md`, or the manifest's `commands` | the slash command `/plugin:command` |
| agents | `agents/**/*.md`, or the manifest's `agents` | the subagent type `plugin:agent` |
| hooks | `hooks/hooks.json`, a root `hooks.json`, the manifest's `hooks` (paths or inline) | one generated hooks file |
| MCP servers | `.mcp.json`, a root `mcp.json`, the manifest's `mcpServers` (paths or inline) | one generated MCP file |

What a plugin carries that this app has no use for — ChatGPT apps, LSP
servers, output styles, themes, workflows, monitors — is named on its detail
page under *Not used*, never loaded.

### Marketplaces

A marketplace's manifest is the first of
`.claude-plugin/marketplace.json`, `.agents/plugins/marketplace.json` and
`.cursor-plugin/marketplace.json` under its root; its `name` is the
marketplace's name. Every `source` shape either reference writes is read:

| An entry's `source` | Read as |
| --- | --- |
| `"./plugins/x"`, or a bare name under `metadata.pluginRoot` | a folder of the marketplace |
| `{"source":"local","path":…}` | a folder of the marketplace |
| `{"source":"url","url":…,"path"?,"ref"?,"sha"?}` | a git repository, or a folder of one |
| `{"source":"git-subdir","url":…,"path":…,"ref"?,"sha"?}` | a folder of a git repository |
| `{"source":"github","repo":"owner/name","ref"?,"sha"?}` | a GitHub repository |
| `npm`, `archive`, `command`, anything else | listed, marked as one that cannot be installed here, **never run** |

A marketplace is added by `owner/repo` (GitHub), any git URL (`#ref` or
`@ref` names a branch or tag), or a local folder. Two are **defaults**:
`anthropics/claude-plugins-official` and `openai/plugins`. A default is not
fetched until it is first needed — opening `/plugins`, or a CLI command that
has to look in it — and a default the user removes stays removed.
`ALTER_ZERO_PLUGIN_MARKETPLACES` replaces the default list (`none` for no
defaults), the `*_DIR` variables' convention.

### On disk

Everything lives under the config home:

```
plugins.json                                  the marketplaces you have, the plugins you installed
plugins/marketplaces/<marketplace>/           a marketplace's git checkout (depth 1)
plugins/cache/<marketplace>/<plugin>/         an installed plugin: a copy, without .git or symlinks
plugins/cache/<marketplace>/<plugin>/.alter-zero-plugin/
    hooks.json  mcp.json                      generated at install (below)
plugins/data/<plugin>-<marketplace>/          the plugin's own data directory, kept across updates
plugins/tmp/                                  staging for clones and installs
```

A local-folder marketplace is read where it is; a plugin is **always
copied**, whatever its source, so updating a marketplace never changes an
installed plugin until it is updated itself. The install path carries **no
version**: hook trust is keyed on the hooks file's path, so an update asks
again only about a hook whose command actually changed.

`plugins.json` is small, written atomically, always as a read-modify-write
over the file as it is on disk at that moment, and never over a file that
will not parse. Beside each plugin's state (`enabled`, `version`,
`revision`) it **records where its components are** — the `skills`,
`commands` and `agents` paths and whether hooks and MCP files were generated
— so a session never parses a manifest to start.

### Installing

`git` is the user's own, run as they would run it — their config, proxies
and credential helpers stay in force — with its output read under a bound, a
timeout, a cancel that kills everything it started, and, inside the TUI, no
way to ask the terminal for a password (`plugins::git`).

An install resolves the marketplace entry, gets the plugin's folder — the
marketplace's own for an in-repository plugin, a one-commit fetch for a git
source (the pinned `sha` when there is one, verified) — reads it
(`plugins::layout::inspect`), copies it into staging without `.git` or
symlinks, writes the two generated files, and swaps the result into place in
one rename; then it records the plugin. A failure at any step leaves the
previous install and the state file exactly as they were.

**The two generated files are the whole trick on the session's side.** What
a plugin ships as hooks and MCP configuration comes in three dialects with
different variable spellings and working-directory rules. Rather than teach
the hooks runner and the MCP loader those dialects, the installer rewrites
what it finds once (`plugins::normalize`) into the two formats this app
already reads:

- `.alter-zero-plugin/hooks.json` is an ordinary `hooks.json`
  (`docs/hooks.md`) holding only what can run here — `command` handlers on
  events this app fires — with `${CLAUDE_PLUGIN_ROOT}`, `${PLUGIN_ROOT}`
  and the two data variables already expanded. An event this app does not
  fire, a `prompt` or `agent` handler, and a handler behind an `if`
  condition are left out and counted for the detail page.
- `.alter-zero-plugin/mcp.json` is an ordinary `mcp.json` (`docs/mcp.md`):
  each server named `plugin_<plugin>_<server>` (Claude Code's wire shape, so
  a hook matcher written for `mcp__plugin_<plugin>_<server>__<tool>` matches
  unchanged), `streamable-http` spelt `http`, the variables expanded, the
  plugin's four variables set in a stdio server's environment, and — for
  the two formats whose servers start in the plugin's folder — the command
  wrapped in a `cd`.

### What an enabled plugin adds to a session

A session reads `plugins.json` once at launch and re-checks its modification
time at each turn (`llm::plugin::PluginSnapshot`); with no such file that is
one failed `stat` and nothing else.

- **Skills** join the skill registry as `plugin:skill`
  (`docs/skills.md`). The `$` picker completes them, a bare `skill` name
  finds one when exactly one plugin has it, and a plugin skill's body has
  `${CLAUDE_PLUGIN_ROOT}` expanded and a `Plugin root:` line under its
  `Base directory` header.
- **Slash commands** are prompt templates the *user* runs: `/plugin:command
  arguments` submits the template with `$ARGUMENTS`, `$1`…`$9` and
  `$ARGUMENTS[n]` filled in, the way `/init` submits its prompt. The palette
  lists them after the built-ins and finds one by any of its `:`-separated
  parts, so `/rev` finds `/code-review:review`.
- **Agents** join the subagent types as `plugin:agent`
  (`docs/subagents.md`), always on the session's own model, with a Claude
  Code agent's tool names mapped to the tools this app has.
- **Hooks** are a third hooks layer, after the user's and the project's, and
  **MCP servers** are a `Plugin` scope held for approval like a project's
  servers. Both are read once at launch and pass the startup review
  (`docs/config-review.md`) before anything runs; a hook process gets
  `PLUGIN_ROOT`, `CLAUDE_PLUGIN_ROOT`, `PLUGIN_DATA` and
  `CLAUDE_PLUGIN_DATA` in its environment.

So a plugin installed mid-session gives its skills, commands and agents to
the next message, and its hooks and MCP servers to the next launch — where
the review shows each hook's command verbatim and each server's command or
URL before the user says yes. `ALTER_ZERO_PLUGINS=0` loads none of it.

## The page

`/plugins` is a composer-replacing page in the `/skills` and `/mcp` family
(`docs/view-flow.md`): it works mid-turn with the streaming strip kept above
it, hugs its rules, and sheds rows on a short terminal rather than flowing.

```
────────────────────────────────────────────────────────────────────────────────────────────────
  Plugins
  2 marketplaces · 382 plugins · 3 installed · updated 2h ago

   All    Installed 3    claude-plugins-official 317    openai-curated 65    Marketplaces
  ❯

  → ✔ code-review          Automated code review for pull requests       claude-plugins-official
    ◯ feature-dev          Guided feature development with codebase un…  claude-plugins-official
    ↑ linear               Find and manage Linear issues                 openai-curated
      agent-sdk-dev        Claude Agent SDK Development Plugin           claude-plugins-official
    (1/382)

    Automated code review for pull requests using multiple specialized agents with confidence-
    based scoring to filter false positives.
    development · Anthropic · v1.0.0

  Enter for details · Space to disable · ←/→ tabs · Type to search · Esc to close
────────────────────────────────────────────────────────────────────────────────────────────────
```

- **Tabs**: `All`, `Installed`, one per marketplace, and `Marketplaces`,
  where marketplaces are rows to open, add (`owner/repo`, a git URL or a
  folder) and restore — visible rows, not chords.
- **One status vocabulary**, a mark per row: `✔` installed and enabled, `◯`
  installed and disabled, `↑` an update is available, `⊘` cannot be
  installed here, blank for not installed.
- **Enter** opens a plugin's page: what it is, where it comes from, and
  **what it brings** — its skills, commands, agents, hooks and MCP servers
  by name, and what it carries that is not used — over the actions that fit
  its state (Install; Enable or Disable, Update, Uninstall). **Space**
  enables or disables an installed plugin in place.
- **Esc goes back exactly one level**, everywhere: a running operation is
  cancelled, an armed confirmation disarmed, a typed search cleared, a page
  left, and only then the page closed.
- **Slow work is shown and can be stopped**: the step, git's own percentage
  when it reports one, and the elapsed time, with Esc to cancel.
- **The result is said on the page**, including when it takes effect —
  `2 skills and 1 command are ready for your next message`, `1 hook and 1
  MCP server wait for your review at the next launch` — and a failure shows
  the real error, in red, wrapped, where it happened.

Opening the page fetches the marketplaces that were never fetched and
refreshes the ones older than a day, behind rows that are already usable;
`Ctrl+R` refreshes them all. Operations run on worker threads whose answers
are folded in by round, so one for a closed or reopened page is dropped.

## Memory

A fresh session's resident memory is a feature of this app
(`docs/memory.md`), and this one was built to leave it where it was:

1. **Nothing at startup.** No network, no `git`, no thread and no catalog —
   Codex syncs its marketplaces at every launch; here a marketplace is
   fetched only when `/plugins` is open or a CLI command asks. With no
   `plugins.json` a launch does one failed `stat`; with plugins it reads one
   small file whose records already say where each component is.
2. **A held MCP server launches nothing.** A plugin's server costs a process
   only after the user approves it.
3. **A marketplace manifest is never held whole.** It is read through a
   buffered reader a record at a time into typed entries that keep only what
   the catalog shows, descriptions cut as they are read — never a document
   tree, never the body in one buffer (`plugins::marketplace`,
   `tests/plugin_catalog_memory.rs`).
4. **The catalog lives only while the page is open**; closing `/plugins`
   drops it.
5. **A still page writes and schedules nothing**; it asks for frames only
   while a fetch or an operation is running. Even the channel its workers
   answer on is created the first time the page opens, so a session that
   never opens it allocates nothing for it.

## Deliberately not doing

- **Running anything to find or build a plugin.** `npm`, `archive` and
  `command` sources are listed and refused; an install never runs a
  lifecycle script, and a plugin's MCP server that needs `npm install`
  first is the user's to prepare.
- **ChatGPT apps, LSP servers, output styles, themes.** Named under *Not
  used*; nothing in this app would consume them.
- **Per-project plugin sets.** A plugin is installed for the user; a skill
  it brings can still be turned off per project in `/skills`.
- **Background updates.** Nothing checks for a newer plugin unless the page
  is open or `alter-zero plugin update` runs.
- **Live hooks and MCP servers.** They are read at launch, like the
  project's (`docs/project-config.md`), so the review is always the door.
- **`!` shell expansion and `@file` attachment in a command's body.** The
  template is submitted as written; the model can run what it names.

## Settings

- `ALTER_ZERO_PLUGINS` — `0`/`false`/`no`/`off` loads no plugin for a run
  (nothing is read, not even `plugins.json`); the page and the command
  family still manage what is installed.
- `ALTER_ZERO_PLUGIN_MARKETPLACES` — replaces the two default marketplaces:
  whitespace-separated sources, or `none` for no defaults. The smoke suite
  exports `none`, so no phase can reach the network.
- `ALTER_ZERO_CONFIG_DIR` moves `plugins.json` and `plugins/` with the rest
  of the config home.

## Testing

- `cargo test --lib -- plugins:: llm::plugin` — the formats, the transforms
  (their output parsed back by the real hooks and MCP parsers), `git` and
  the filesystem primitives against repositories made in temp dirs, the
  layout reader, the session's snapshot and discovery.
- `cargo test --test plugin_store` — the store end to end over local
  `file://` repositories built by `tests/support/plugin_fixtures.rs`:
  marketplaces added, updated, renamed, removed and restored; installs from
  every source kind; a failure leaving the previous install untouched.
- `cargo test --test plugin_cli` — the command family through the real
  binary (`docs/plugin-cli.md`).
- `cargo test --test plugin_catalog_memory` — reading a marketplace and
  dropping it leaves no resident growth worth the name.
- `scripts/smoke.sh 162` — the page in a real terminal against fixture
  marketplaces: the fetch on first open, search, a plugin's page, install,
  enable and disable, uninstall, adding and removing a marketplace, and a
  14-row terminal.
- Nothing in the suite reaches the network.

## Module layout

| Where | What |
| --- | --- |
| `src/plugins/id.rs`, `source.rs`, `manifest.rs`, `marketplace.rs`, `state.rs`, `catalog.rs` | the formats: pure, unit-tested |
| `src/plugins/vars.rs`, `command.rs`, `normalize.rs` | the transforms: pure |
| `src/plugins/git.rs`, `fsops.rs` | `git` and filesystem primitives |
| `src/plugins/layout.rs`, `store.rs` | what a plugin folder contains; every operation the CLI and the page share |
| `src/llm/plugin.rs` | the session's snapshot of enabled plugins and the skill, agent and command discovery |
| `src/app/plugins_menu.rs`, `src/ui/plugins_view.rs`, `src/tui/plugins.rs` | the page: state and keys, the view, the boundary and its workers |
| `src/cli.rs`, `src/tui/plugin_cli.rs` | the command family (`docs/plugin-cli.md`) |
