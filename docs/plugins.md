# Plugins — marketplaces, installs, and what a plugin adds to a session

A plugin is a folder someone else wrote that extends the agent: **skills**,
**subagent definitions**, **slash commands**, **lifecycle hooks** and **MCP
servers**, packaged together and installed from a **marketplace** (a git
repository or a local folder that lists plugins). Three formats are read as
they are — the vendor-neutral **Agent Plugins** standard
(<https://agent-plugins.org>, spec 1.0.0), **Claude Code** plugins and
**OpenAI Codex** plugins — and two marketplaces are offered out of the box:

```
$ alter-zero plugin marketplace add --defaults
Added marketplace claude-plugins-official (317 plugins)
Added marketplace openai-curated (65 plugins)

$ alter-zero plugin install frontend-design@claude-plugins-official
Installed frontend-design 1.0.0 from claude-plugins-official
  1 skill
```

Inside a session the same thing is one page, `/plugins`
(`docs/plugins-menu.md`); the command family is `docs/plugin-cli.md`.

## What the references do (findings)

All three were read end to end (Codex at its 2026-11-20 tree, Claude Code's
documentation, the Agent Plugins 1.0.0 specification), and both default
marketplaces were inventoried file by file.

- **Agent Plugins 1.0.0** — a deliberately small portable floor. A plugin is
  a directory with a **root `plugin.json`** whose `$schema` is exactly
  `https://agent-plugins.org/schemas/1.0.0/plugin.schema.json` and whose
  `name` is 1–64 characters of `[a-z0-9.-]` (alphanumeric at both ends, no
  `--`, no `..`). The manifest is **closed** — `$schema`, `name`, `version`,
  `description`, `author {name,email,url}`, `homepage`, `repository`,
  `license`, `keywords`, `extensions` — an unknown top-level field is
  reported and ignored, a non-object `extensions` is reported and ignored,
  and any other violation rejects the whole plugin. Components live at
  **fixed** locations: `skills/<name>/SKILL.md` (immediate children only)
  and a root **`mcp.json`** (not `.mcp.json`) with its own `$schema`, a
  required `type` per server (`stdio`, `streamable-http`, `sse`), a stdio
  `command` that is one token (a bare name or a `./` path inside the
  plugin), a default working directory of the plugin root, and exactly two
  placeholders — `${PLUGIN_ROOT}`, `${PLUGIN_DATA}` — expanded only in
  `args`, `env` values and `cwd`. Everything else — distribution, install,
  trust, hooks, agents, commands — is left to the client.
- **Claude Code** — `.claude-plugin/plugin.json` (optional; the marketplace
  entry can stand in for it) over default locations: `skills/`, `commands/`,
  `agents/`, `hooks/hooks.json` (the settings shape wrapped in
  `{"hooks": …}`, often beside a `description`), `.mcp.json` (wrapped in
  `mcpServers` or a bare map; no `type` means stdio). Components are
  namespaced `plugin:name`. `${CLAUDE_PLUGIN_ROOT}` and
  `${CLAUDE_PLUGIN_DATA}` resolve in hook commands, MCP configs and Markdown
  bodies; `.mcp.json` also expands `${VAR}` / `${VAR:-default}` from the
  environment. Marketplaces are `.claude-plugin/marketplace.json`; an entry's
  `source` is a relative path or an object (`github`, `url`, `git-subdir`,
  `npm`, `archive`, `command`). State lives under `~/.claude/plugins/`
  (`marketplaces/<name>/` clones, `cache/<marketplace>/<plugin>/<version>/`
  copies, `data/<id>/`).
- **Codex** — `.codex-plugin/plugin.json` with a large `interface` block for
  its app, over `skills/`, `.mcp.json`, `hooks/hooks.json`, `.app.json`
  (ChatGPT connector ids, meaningless anywhere else). It reads a root
  `plugin.json` first when its `$schema` is an agent-plugins.org one, then
  `.codex-plugin`, `.claude-plugin`, `.cursor-plugin` — and a symlinked or
  non-regular manifest higher in that order **stops** the lookup instead of
  falling through, so a manifest cannot hide behind another. Marketplaces
  are `.agents/plugins/marketplace.json` (then `.claude-plugin/…`), named by
  the manifest's own `name`, cloned whole, and re-checked over the network
  **at every startup**. Installs copy into
  `plugins/cache/<marketplace>/<plugin>/<version>/`, skipping symlinks.
  Skills are namespaced `plugin:skill`; a config MCP server beats a plugin's
  of the same name; Claude `commands/` are converted into skills at install
  and `agents/` is ignored. Plugin hooks run only once trusted.
- **The two marketplaces, as they are today.**
  `anthropics/claude-plugins-official` lists **317** plugins in a 192 KB
  manifest: 55 live in the repository, 261 are other repositories (163
  `url`, 98 `git-subdir`, every one pinned to a commit `sha`), one is a
  `command` source. Its 57 in-repo plugins carry 57 skills, 33 commands, 37
  agents, 8 hooks files (28 handlers, all `sh` commands through
  `${CLAUDE_PLUGIN_ROOT}`) and 15 `.mcp.json` files (9 of them bare maps).
  `openai/plugins` (`openai-curated`) lists **65** in 19 KB with no
  descriptions on the entries — the plugin's own manifest has them — 62
  in-repo plugins holding 507 skills and 31 `.mcp.json` files (26 remote),
  and three unpinned external ones. A full shallow clone of it is 105 MB; a
  blobless sparse clone that materialises only the manifest is **1.4 MB in
  about a second**. No plugin in either repository carries a second
  format's manifest.

## What we build

Codex's shape — marketplaces, a versioned cache, `name@marketplace` ids, a
tabbed `/plugins` browser and a `plugin` command family — with three things
it does not have and one it does that we refuse:

- **Every format, read natively.** Agent Plugins strictly by its
  specification; Claude Code and Codex by one lenient reader over the union
  of their layouts, including the parts Codex drops (`agents/`, slash
  commands as commands, `${CLAUDE_PLUGIN_ROOT}` in `.mcp.json`, the `github`
  source form).
- **Nothing a plugin runs is run unseen.** Its hooks and MCP servers enter
  through the same startup review every other hook and project server
  passes (`docs/config-review.md`), are asked again after every update, and
  the page that installs a plugin says what it will bring before and after.
- **Enable, disable, update and cancel everywhere** — on the page and in the
  CLI — with the real error on screen when something fails.
- **No network, no `git`, no catalog at startup.** A session that has no
  plugins reads one small file that does not exist and stops there. Codex
  syncs its curated repository and every configured marketplace on every
  launch; here a marketplace is fetched when the user asks for it, and its
  catalog is parsed when `/plugins` is opened and dropped when it closes
  (*Memory*, below).

### Three formats, one reader

A plugin root is classified once, by what it holds (`plugins::manifest`):

1. **Root `plugin.json` whose `$schema` begins
   `https://agent-plugins.org/schemas/`** → **Agent Plugins mode**. The
   exact 1.0.0 identifier is the one supported version; any other
   agent-plugins.org identifier rejects the plugin as an unsupported version
   (the specification's MUST). A root `plugin.json` with no such `$schema`
   is somebody else's file and is ignored.
2. Otherwise the first of **`.claude-plugin/plugin.json`**,
   **`.codex-plugin/plugin.json`**, **`.cursor-plugin/plugin.json`** that
   exists → **legacy mode**, one parser for all three. No field is required
   (`name` falls back to the marketplace entry's, then the folder's);
   unknown fields are ignored; a field of the wrong shape is reported and
   dropped without failing the plugin.
3. None of them → legacy mode with the **marketplace entry as the
   manifest** (Claude Code's rule; the entry was saved beside the install —
   *Installing*).

Codex's anti-concealment rule is kept: a manifest path that exists as a
symlink or as anything but a regular file **rejects the plugin** rather than
letting a lower-precedence manifest answer for it.

**Agent Plugins mode is the specification, not an approximation.** The
closed manifest is validated field by field (types, the name grammar, the
closed `author`); an unknown top-level field and a non-object `extensions`
are *notices*, everything else is fatal and the plugin contributes nothing.
Skills come only from `skills/<name>/SKILL.md`, MCP only from `mcp.json`
(two-stage validation: a bad document disables MCP for the plugin, a bad
entry disables that entry), its `$schema` version must match the
manifest's, and every package path is resolved through the filesystem and
held inside the resolved plugin root. No extension namespace is implemented
yet, so `extensions` members are ignored unvalidated, as the specification
requires. One thing is added on top, the way Codex adds its own overlay: a
package that is *also* a Claude Code or Codex plugin (it carries one of the
legacy manifest directories) still contributes the component types the
standard does not cover — agents, commands, hooks — through that legacy
layout, so adopting the portable manifest never costs a plugin its hooks.
Its skills and MCP servers stay the standard's alone.

**Legacy mode** is the union of what the two clients read, by Claude Code's
documented rules (they are a superset of Codex's for every plugin in either
marketplace):

| Component | Default location | Manifest key | Rule |
| --- | --- | --- | --- |
| Skills | `skills/<name>/SKILL.md` | `skills` (path or paths) | added to the default |
| Commands | `commands/**/*.md` | `commands` (path or paths) | replaces the default |
| Agents | `agents/**/*.md` | `agents` (path or paths) | replaces the default |
| Hooks | `hooks/hooks.json` | `hooks` (path, paths, inline object) | merged with the default |
| MCP servers | `.mcp.json` | `mcpServers` (path, inline object, list) | merged, first name wins |

A manifest path must begin `./`, may not contain `..`, and must resolve
inside the plugin. Display metadata is read from whichever spelling is
there: `displayName` or `interface.displayName`, `description` or
`interface.shortDescription` / `longDescription`, `author` as an object or a
string, `interface.category`, `interface.developerName`, `homepage` or
`interface.websiteURL`.

What a plugin may ship that this client cannot use is **named, never
silently dropped** — the detail page and `plugin info` list it under *Not
used here*: `.app.json` apps (ChatGPT connectors), LSP servers, output
styles, themes, monitors, workflows, `bin/`, `userConfig`, channels, mods
(`modules` in a hooks file).

### What a plugin adds

| It ships | It becomes | Named | Reviewed first |
| --- | --- | --- | --- |
| `skills/<n>/SKILL.md` | a skill in the listing, the `$` picker and `/skills` | `plugin:n` | no — inert Markdown |
| `agents/<n>.md` | a subagent type | `plugin:n` | no — its calls still meet the permission gate |
| `commands/<n>.md` | a slash command in the palette | `/plugin:n` | no — it is a prompt |
| a hooks file | lifecycle hooks | by event | **yes** — the hooks page |
| MCP servers | servers in `/mcp` | the server's own name | **yes** — the servers page |

- **Skills** join `SkillRegistry` after the user's and the project's own, as
  `plugin:skill` — the reference spelling, and the one skill bodies written
  for either client use to point at each other. A bare `skill` still
  resolves when exactly one plugin provides it. The `$` picker accepts `:`
  in a name. A body's `${CLAUDE_PLUGIN_ROOT}` / `${PLUGIN_ROOT}` is expanded
  beside the existing `${…SKILL_DIR}`. `` !`cmd` `` blocks are **not** run
  for the plugin — the model reads them and runs what it needs through the
  permission gate — and `allowed-tools` grants nothing.
- **Agents** join the subagent roster as `plugin:agent`. A plugin's `model:`
  is ignored (the agent inherits the session's model: a Claude alias means
  nothing on another provider), and a `tools:` list is read in Claude Code's
  vocabulary — `Read`, `Write`, `Edit`, `Bash` as themselves, and the
  search tools this client serves through the shell (`Grep`, `Glob`, `LS`)
  as `Bash`, since an agent allowed only tools that do not exist here could
  do nothing.
- **Commands** are palette rows named `/plugin:command` (a subfolder adds a
  segment), matched by the short name too. Running one submits its body as
  the prompt with `$ARGUMENTS` / `$1`…`$9` filled from what was typed after
  it; nothing in it is executed.
- **Hooks** are one more layer in `TrustState::hook_layers()` per file, so
  the review, `/hooks`, trust and the runner treat them as they treat the
  user's own. `${CLAUDE_PLUGIN_ROOT}`, `${PLUGIN_ROOT}`,
  `${CLAUDE_PLUGIN_DATA}` and `${PLUGIN_DATA}` are substituted into each
  command when the file is loaded (so two plugins' identical command text
  never collapses into one handler, and the review shows the real path) and
  exported to the process. A file that carries keys this client does not
  model (`modules`) still loads its hooks; one with no `hooks` at all is not
  an error.
- **MCP servers** are a third reviewed scope, `McpScope::Plugin`. A server
  keeps its own name; the user's and a project's servers of the same name
  win, and among plugins the first enabled one does — the loser is reported,
  not silently hidden. Legacy files get the four plugin placeholders
  expanded everywhere, a relative `cwd` and a `./` command resolved against
  the plugin root, then the ordinary `${VAR}` expansion (refused, as for a
  project server, where it would put a provider key in a remote header).
  Agent Plugins `mcp.json` gets exactly what the specification allows — the
  two placeholders in `args`, `env` values and `cwd`, a working directory of
  the plugin root, nothing else expanded — and both kinds get
  `PLUGIN_ROOT`, `PLUGIN_DATA`, `CLAUDE_PLUGIN_ROOT` and
  `CLAUDE_PLUGIN_DATA` in a stdio server's environment.

`PLUGIN_DATA` is `{config_home}/plugins/data/<marketplace>/<plugin>/`,
created before the first hook or stdio server of the plugin is loaded, kept
across updates and removed with the plugin.

### Marketplaces

A marketplace root holds one manifest, the first that exists of
`.agents/plugins/marketplace.json`, `.claude-plugin/marketplace.json`,
`.cursor-plugin/marketplace.json`. Its `name` (letters, digits, `.`, `_`,
`-`) is the marketplace's name — never derived from the URL — and adding a
second source under a name already in use is refused.

| Read from the manifest | |
| --- | --- |
| top level | `name`, `plugins`; `interface.displayName`; `description` / `metadata.description`; `owner.name`; `metadata.pluginRoot` |
| each entry | `name`, `source`; `description`, `version`, `category`, `author`, `homepage`, `keywords` / `tags`, `displayName` / `interface.displayName`; `policy.installation`; the component keys of an entry that stands in for a manifest |

An entry that cannot be read is skipped with a notice and the rest of the
marketplace still loads. An entry's `source`:

| Form | Supported |
| --- | --- |
| `"./plugins/x"`, a bare name under `metadata.pluginRoot`, `{"source":"local","path":…}` | yes — a folder of the marketplace |
| `{"source":"github","repo":"owner/repo","ref"?,"sha"?}` | yes |
| `{"source":"url","url":…,"path"?,"ref"?,"sha"?}` | yes (`path`, undocumented but used by four official entries, is a subfolder) |
| `{"source":"git-subdir","url":…,"path":…,"ref"?,"sha"?}` | yes |
| `npm`, `archive`, `command`, anything else | **no** — listed, marked unavailable with the reason; `command` would run a shell command at install and every launch |

`policy.installation: "NOT_AVAILABLE"` makes an entry unavailable too.

**Adding one** takes `owner/repo`, an `https://` / `ssh://` / `git@host:` URL
or a local folder (`/abs`, `./rel`, `../rel`, `~/…`), with an optional
`@ref` / `#ref` or `--ref` on a git source. A local folder is used **in
place** — never copied, so an author's edits are live. The parse is pure
(`plugins::source`) and runs before anything is fetched, so a typo is an
error at once rather than a failed clone.

**Two defaults are offered, not fetched**:

| Name | Source |
| --- | --- |
| `claude-plugins-official` | `anthropics/claude-plugins-official` |
| `openai-curated` | `openai/plugins` |

They appear in `/plugins` and `plugin marketplace list` as *not added* until
the user adds them (one keypress adds both); installing
`plugin@claude-plugins-official` adds that marketplace on the way. Removing
one returns it to *not added*. `ALTER_ZERO_PLUGIN_MARKETPLACES` replaces the
default list for a run (the smoke suite points it at local repositories).

### On disk

```
{config_home}/plugins.json                              what is added, installed and enabled
{config_home}/plugins/marketplaces/<name>/              a sparse clone of a git marketplace
{config_home}/plugins/cache/<marketplace>/<plugin>/<version>/   an installed plugin
{config_home}/plugins/data/<marketplace>/<plugin>/      PLUGIN_DATA, kept across updates
```

```json
{
  "marketplaces": {
    "claude-plugins-official": {
      "source": "anthropics/claude-plugins-official",
      "revision": "d7d3279…",
      "updated": 1795180000
    },
    "team": { "source": "/home/me/dev/team-plugins" }
  },
  "plugins": {
    "frontend-design@claude-plugins-official": {
      "version": "1.0.0",
      "revision": "9c1e0b4…",
      "enabled": true,
      "installed": 1795180100
    }
  }
}
```

`plugins.json` follows the house rules for a state file: a pure format
(`plugins::state::PluginsFile`), read-modify-write over the file as it is on
disk at that moment, written through a temp file and a rename (an install
worker and a second session can both be writing), unknown keys kept, and a
file that will not parse reads as *nothing installed* and is never
overwritten. `revision` is what the copy was made from — the commit of an
external source, the **tree** of an in-repo plugin (`git rev-parse
HEAD:<path>`), so "an update is available" is one string comparison after a
marketplace update and is true only when that plugin's own files changed.
The version directory is the manifest's `version`, else the first twelve
characters of the revision, else `local`.

### Fetching — `git` as a child process

Nothing is linked in: `git` on `PATH` is run as a child (`plugins::git`),
which costs this process no memory and no binary size. Every invocation is
hardened the same way — no terminal to prompt on (`GIT_TERMINAL_PROMPT=0`,
`ssh -oBatchMode=yes`, stdin closed, the tty-detach chain of
`docs/tty-detach.md`), the caller's repository out of the picture
(`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and their kin removed,
`-c safe.bareRepository=explicit`, `-c core.fsmonitor=false`), output
captured into a bounded buffer whose tail is the error message, a timeout,
and the operation's `CancelToken` polled every 20 ms so Esc kills the child.
No submodules are fetched.

- **A marketplace** is a blobless, sparse, depth-1 clone —
  `git clone --depth 1 --filter=blob:none --sparse [--branch <ref>]` into a
  staging directory, then `sparse-checkout set` over the three manifest
  directories — so adding `openai/plugins` downloads 1.4 MB rather than
  105 MB. The staged clone is validated (a manifest, a usable name) and
  renamed into place; a failed add leaves nothing behind. A server that
  refuses the filter still works (git falls back to a plain shallow clone).
- **Updating** one is `fetch --depth 1` of its ref and a detached checkout;
  the same revision back means *already up to date*.
- **An in-repo plugin** is materialised on demand: `sparse-checkout add
  <path>` in the marketplace clone fetches that folder's blobs, and the
  folder is copied. Before that, what it holds is already known without the
  network — a blobless clone has every tree, so `git ls-tree -r` lists a
  plugin's skills, commands, agents and whether it has hooks or MCP servers
  (the page's *Contains* rows for a plugin not yet installed).
- **An external plugin** is fetched into a staging repository at exactly
  the commit the marketplace names: `init`, `remote add`, `sparse-checkout
  set <subdir>` when there is one, `fetch --depth 1 origin <sha|ref|HEAD>`,
  `checkout FETCH_HEAD`, and a check that a pinned `sha` is what was checked
  out.

### Installing

1. Resolve `plugin@marketplace` in the marketplace's catalog (adding a
   default marketplace first if it is one and not yet added); refuse an
   unavailable entry with its reason.
2. Materialise the source into a staging directory.
3. Classify and validate the plugin (*Three formats*); an Agent Plugins
   manifest that is fatal by the specification fails the install.
4. Copy into `cache/<marketplace>/<plugin>/.staging-…/` — regular files and
   directories only, `.git` left out, a symlink kept only when it resolves
   inside the plugin, bounded at 200 MiB and 20 000 files — and, when the
   entry declares components or the plugin has no manifest of its own, save
   the entry beside the copy as `.alter-zero-entry.json`.
5. Rename into `cache/<marketplace>/<plugin>/<version>/`, remove the other
   versions, and record the plugin `enabled` in `plugins.json`.
6. **Forget every trust record under that plugin's cache directory**
   (`trust.json`'s hooks and MCP entries are keyed by file path), so an
   install or an update is always reviewed again.

Uninstalling removes the cache and data directories, the record and the
trust records. Removing a marketplace leaves the plugins installed from it
installed — and says so.

### Loading into a session

Startup reads `plugins.json` and nothing else: no directory is walked, no
marketplace is opened. With no plugins the cost is one `read` that fails.
With some, each enabled plugin is classified from its own small manifest
and contributes paths to the walks that already run — its `skills/` to the
skill discovery, its `agents/` to the agent discovery, its hook files to
the trust layers, its MCP entries to the manager's sources — so what stays
resident is what those features already keep per item (a skill's name and
description, an agent's definition), and nothing for a plugin that is
disabled.

The snapshot is taken again whenever this session changes it (an install,
an update, an uninstall, a toggle — from `/plugins`) and when `/plugins` is
opened (another process may have installed something). Skills, agents and
commands follow at once; hooks are rebound through `set_hooks_file`; MCP
servers are added to or dropped from the running manager; and when the
change brought hooks or servers that are not yet trusted, the review opens
on them (`ConfigReview::requested`). `ALTER_ZERO_PLUGINS=0` loads no plugin
for a run.

### Nothing runs unseen

A plugin is somebody else's code arriving over the network, so the rule the
rest of the app keeps — no hook and no project MCP server runs until the
user has seen it — is kept without a second mechanism:

- Hooks and stdio/remote servers are **off until reviewed**, on the same
  two pages, grouped under the plugin's name.
- **Every update is reviewed again**: the install path carries the version,
  and the trust records under the plugin's directory are dropped by the
  install itself, since a remote update can change a script under an
  unchanged command line.
- A `command` plugin source, `` !`cmd` `` injection, `allowed-tools`,
  `bin/` on the `PATH`, monitors and in-process mods are **never run**.
- `${VAR}` expansion cannot carry a provider key into a plugin's remote
  server (the project-scope protection).
- Skills, agents and commands are inert text: what they ask the model to do
  still meets the permission gate.

### Memory

Resident memory is a feature here (`docs/memory.md`), and the idle number is
the one this design is held to: **a session with no plugins costs what it
cost before**, and one with plugins costs only what their components cost in
the features that load them.

- **Startup**: one bounded read of `plugins.json`, typed, no `Value` tree.
- **A marketplace catalog** is read only while `/plugins` is open (or a CLI
  command runs): streamed off the file through `serde_json::from_reader`
  over a `Read::take` cap (8 MiB) into typed records naming only the fields
  the page shows — never the 192 KB body in one buffer, never a tree of it —
  with descriptions cut at 2 000 characters, and dropped when the page
  closes.
- **Fetching** is a child process; its output is captured into a few
  kilobytes.
- **The worker thread** that waits on `git` builds nothing large (a new
  thread is a new arena): results cross back as small typed values and the
  catalog is read on the loop.
- No new dependency, no static table of references.

`scripts/smoke` holds it: an idle session with an empty config home and one
with the feature's state present are compared by resident size (Phase 162).

## The `/plugins` page

`docs/plugins-menu.md` is the full description; the shape:

```
────────────────────────────────────────────────────────────────────────────────────────
  Plugins
  ←  Discover   Installed (2)   Marketplaces (2)  →

  ❯ fig
  → figma                ✔ installed   openai-curated            Design-to-code with …
    figma                              claude-plugins-official   Figma's MCP server …
    (1/2) · updated 2h ago · ctrl+r to refresh

    figma@openai-curated · Developer Tools · 2.1.0 · Figma
    Turn Figma files into implementation context: read designs, components and
    variables straight from Dev Mode.

  Type to search · ←/→ tabs · Enter for details · Space to turn on or off · Esc to close
────────────────────────────────────────────────────────────────────────────────────────
```

- **Three tabs** in the ask modal's chip dress — **Discover** (every plugin
  of every added marketplace), **Installed**, **Marketplaces** — switched
  with ←/→ or Tab/Shift+Tab; the search survives the switch.
- **One status vocabulary** on every screen: `✔ installed`, `◯ disabled`,
  `↑ update`, `✘ unavailable`, `⚠ invalid`, and nothing for a plugin that
  is simply available.
- **A first open with no marketplace** is not an empty list but the offer:
  add both defaults, either one, or another source.
- **Enter** opens a plugin: what it is, where it comes from, what it
  contains (skills, commands, agents, hooks by event, MCP servers, and what
  is not used here), and numbered actions for its state — Install; or
  Enable/Disable, Update, Uninstall (confirmed) — with Space toggling an
  installed plugin straight from the list.
- **Work happens on a worker** and the page says so: the action rows give
  way to a spinner line naming the step and its elapsed time, **Esc
  cancels**, a failure stays on the page as the real message with *Try
  again*, and a success is a toast naming what was added.
- **Marketplaces** lists the added ones with their size and age, the
  defaults not yet added, and *Add a marketplace…*, a text field that
  refuses a malformed source before fetching and keeps what was typed when a
  fetch fails.
- It is a framed page of the family (`docs/view-flow.md`): it hugs its
  rules, sheds rows on a short terminal down to its search line, and is the
  one builder its height, its paint and its cursor all read.

## The `plugin` command family

`docs/plugin-cli.md` is the full description; the grammar:

```
alter-zero plugin install <plugin>[@<marketplace>]
alter-zero plugin uninstall <plugin>[@<marketplace>]
alter-zero plugin enable <plugin>[@<marketplace>]
alter-zero plugin disable <plugin>[@<marketplace>]
alter-zero plugin update [<plugin>[@<marketplace>]]
alter-zero plugin list [--available] [--json]
alter-zero plugin info <plugin>[@<marketplace>]
alter-zero plugin marketplace add <source> [--ref <ref>]
alter-zero plugin marketplace add --defaults
alter-zero plugin marketplace list [--json]
alter-zero plugin marketplace update [<name>]
alter-zero plugin marketplace remove <name>
```

Resolved and answered before the TUI boots, like `mcp` (`docs/mcp-cli.md`):
results on stdout, one plain failure line on stderr, exit `2` for a grammar
error, `1` for a failure, `0` otherwise. A bare `plugin` name is looked up
in every added marketplace and must be unambiguous. An install that brought
hooks or MCP servers says they will be reviewed at the next launch.

## Environment

| Variable | |
| --- | --- |
| `ALTER_ZERO_PLUGINS` | `0` loads no plugin for the run (the page and the CLI still work) |
| `ALTER_ZERO_PLUGIN_MARKETPLACES` | replaces the default marketplaces: sources separated by commas; empty offers none |
| `ALTER_ZERO_PLUGIN_GIT_TIMEOUT_SECS` | the bound on one network `git` command (default 300) |

The state lives in the config home, so `ALTER_ZERO_CONFIG_DIR` relocates all
of it — which is what makes every smoke phase start with no plugins.

## Known divergences

- **Not every source form.** `npm`, `archive` and `command` sources are
  listed as unavailable. One official plugin (`xcode`) is a `command`
  source.
- **User scope only.** A plugin is installed and enabled for the user;
  there is no project or local scope, and a repository cannot enable one.
- **No automatic updates.** Nothing is fetched unless asked; the page shows
  how old a catalog is.
- **`renames`, `strict`, dependencies, `engines`** in a marketplace are
  read past. An entry's components are appended to the plugin's own.
- **A root `SKILL.md`** (a plugin that is one skill) and skills nested
  deeper than `skills/<name>/` are not discovered.
- **`.app.json` apps, LSP servers, output styles, themes, monitors,
  workflows, `bin/`, `userConfig`, channels and mods** are not used, and are
  named as such.
- **Command bodies run nothing**: `` !`cmd` `` stays text.
- **Agent Plugins extensions**: none implemented; 1.1.0 (a draft that adds
  a portable `hooks.json`) is refused as an unsupported version until it is
  published and supported.
- **Codex's MCP extras** (`oauth_resource`, `scopes`, `env_vars`, the
  timeouts) are ignored; a stdio child already inherits the environment.

## Module layout

| Where | What |
| --- | --- |
| `src/plugins/manifest.rs` | classify a plugin root; the legacy manifest; display metadata |
| `src/plugins/agent_plugins.rs` | Agent Plugins 1.0.0: the closed manifest, the name grammar, `mcp.json` |
| `src/plugins/marketplace.rs` | the marketplace manifest, streamed into `Catalog` |
| `src/plugins/source.rs` | what the user types to add a marketplace |
| `src/plugins/state.rs` | `plugins.json`, ids, the default marketplaces |
| `src/plugins/layout.rs` | the paths under the plugins home; version directory names |
| `src/plugins/components.rs` | what an installed plugin contributes, and its summary |
| `src/plugins/git.rs` | the hardened, bounded, cancellable `git` runner |
| `src/plugins/store.rs` | add / update / remove a marketplace; install / update / uninstall / toggle |
| `src/app/plugins_menu.rs` | the page's pure state and keys |
| `src/ui/plugins_view.rs` | the page's one line builder |
| `src/tui/plugins.rs` | the page's boundary: the worker, the operations, the snapshot |
| `src/tui/plugin_runtime.rs` | enabled plugins → skills, agents, commands, hooks, MCP; reload |
| `src/tui/plugin_cli.rs` | the `plugin` command family |
| `src/cli.rs` | its grammar and help pages |

## Testing

- `cargo test --lib plugins::` — the formats (every manifest rule of the
  specification has a test named for its section), the catalog reader
  against both real manifests' shapes, the source grammar, the state file,
  the path math.
- `cargo test --test plugin_store` — the store against local `git`
  repositories: add, update, install each source form, a pinned `sha`
  mismatch, a cancelled clone, an uninstall; no network.
- `cargo test --test plugin_cli` — the real binary over a temp config home.
- `cargo test --lib -- app::tests::plugins_menu ui::tests::plugins_view` —
  the page: keys, the height ladder, the frame hug, the cursor seat.
- `scripts/smoke.sh 162 163` — the page and the runtime against a local
  marketplace, and the idle-memory comparison.
