# Plugins — `/plugins`, marketplaces, and three plugin formats

A plugin is a directory somebody else maintains that adds to what a session
can do: skills, subagent types, lifecycle hooks, MCP servers. `/plugins`
browses the plugins of the marketplaces the user has added, installs one,
and turns an installed one on or off for the directory; `alter-zero plugin …`
does the same from a shell.

Three formats load, because all three are in use and none of them asks for
anything this app does not already have:

| Format | Manifest | What marks it |
| --- | --- | --- |
| **Agent Plugins 1.0.0** ([agent-plugins.org](https://agent-plugins.org)) | `plugin.json` at the plugin root | a `$schema` under `https://agent-plugins.org/schemas/` |
| **Codex** | `.codex-plugin/plugin.json` | the directory |
| **Claude Code** | `.claude-plugin/plugin.json` (optional) | the directory, or no manifest at all |

The app-facing half is codex's `/plugins` — tabs, a search line, one row per
plugin, Enter for the details, Space to toggle — drawn in this app's picker
frame (`docs/view-flow.md`). The mechanism under it is the union of what the
three formats define, mapped onto the registries that already exist: a
plugin's skills are skills, its agents are agent definitions, its hooks are a
hooks layer, its MCP servers are a scope of the `/mcp` manager. Nothing a
plugin adds gets a second mechanism.

## The three formats

### Which manifest a directory is read by

Codex's precedence (`find_plugin_manifest_path`), minus `.cursor-plugin`:

1. `plugin.json` at the root, **if** its `$schema` is under
   `https://agent-plugins.org/schemas/`. A supported version (1.0.0) makes
   the directory an Agent Plugin. An unsupported one **rejects the plugin**
   and names the version — the spec's rule (§5.2), and codex's: falling back
   to a second manifest would load a package under rules its author did not
   target. A root `plugin.json` with any other `$schema`, or none, is some
   other tool's file (an npm package's) and is ignored.
2. `.codex-plugin/plugin.json`.
3. `.claude-plugin/plugin.json`.
4. No manifest: a Claude Code plugin whose name and metadata come from its
   marketplace entry, when the directory holds at least one component.

A manifest that is a symlink is refused — a link is how a package would point
its identity at a file outside itself.

### Agent Plugins

The portable contract, implemented as written (`plugins::manifest`,
`plugins::mcp`):

- `plugin.json` is a **closed** object: `$schema`, `name`, `version`,
  `description`, `author {name, email, url}`, `homepage`, `repository`,
  `license`, `keywords`, `extensions`. An unknown top-level field and a
  non-object `extensions` are **reported and ignored**; every other
  violation — a missing or mistyped field, a `null`, an unknown key inside
  `author`, a name that is not 1–64 of `[a-z0-9.-]` starting and ending
  alphanumeric with no `--` or `..` — **rejects the plugin** before any
  component is looked at. Metadata is checked by type only: a `version` that
  is not semver or a `homepage` that is not a URL is fine.
- **Skills** are the immediate children of `skills/` that hold a regular
  `SKILL.md` — never deeper. An invalid one is skipped and reported; the
  rest load.
- **MCP servers** come from `mcp.json` at the root and nowhere else: a closed
  `{ "$schema", "mcpServers" }` document whose `$schema` must name the same
  version as the manifest's (a mismatch disables MCP for the plugin, nothing
  else), each server a closed `stdio` / `streamable-http` / `sse` variant
  validated **on its own**, so one bad entry costs only itself:
  - `command` is one token — a bare name, or a `./` path inside the plugin —
    with no placeholder expansion;
  - `cwd` defaults to the plugin root and may be `./…`, `${PLUGIN_ROOT}[/…]`
    or `${PLUGIN_DATA}[/…]`, contained after resolution;
  - `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` expand, once and textually, in
    `args`, `env` values and `cwd` — nowhere else, and nothing else expands;
  - `env` may not name `PLUGIN_ROOT` or `PLUGIN_DATA`; the client sets both
    last, over whatever the entry said;
  - a remote `url` is absolute `http(s)` with no userinfo or fragment,
    `https` unless the host is loopback, and its headers are literal — never
    expanded — with no name repeated in another case.
- **Client extensions.** `extensions` is read for exactly one namespace,
  this app's, `io.github.linuztx.alter-zero` (the reverse of the project's
  home, the convention for a client with no domain of its own); every other
  namespace is ignored without a look at its value. The files of that
  namespace live in the top-level directory of the same name, laid out as a
  Claude Code plugin's — `agents/`, `commands/`, `hooks/hooks.json` — which
  is how a portable package carries the component types the standard leaves
  to each client (§8.2). A package's root-level `agents/` or `hooks/` is
  **not** read: the portable manifest cannot override the fixed locations,
  and a second set of rules for "an Agent Plugin that also looks like a
  Claude one" is how two clients come to disagree about one package.

### Claude Code and Codex

One lenient parser (codex's "legacy" manifest): unknown fields are ignored, a
bad component path is dropped with a note and the manifest still loads, and
the name falls back to the directory's.

| Component | Default location | Manifest key |
| --- | --- | --- |
| Skills | `skills/<name>/SKILL.md`; a lone `SKILL.md` at the root when there is no `skills/` | `skills` — path or paths, **added to** the default |
| Commands | `commands/**/*.md` | `commands` — path or paths, **replacing** the default |
| Agents | `agents/**/*.md` | `agents` — `.md` paths, **replacing** the default |
| Hooks | `hooks/hooks.json` | `hooks` — path, paths, or an inline event map, **merged** with the default |
| MCP servers | `.mcp.json` | `mcpServers` — path, paths, or an inline server map, **merged**; a later name replaces an earlier one |

A manifest path must start with `./`, hold no `..`, and exist inside the
plugin. `.mcp.json` is read in both shapes the ecosystem writes — wrapped in
`mcpServers`, or the bare map — and a plugin `hooks.json` may carry top-level
keys we do not model (`modules`), which are skipped rather than failing the
file the way the user's own `hooks.json` would.

`${CLAUDE_PLUGIN_ROOT}` and `${CLAUDE_PLUGIN_DATA}` (and `${PLUGIN_ROOT}` /
`${PLUGIN_DATA}`, codex's spellings) expand in an MCP entry's `command`,
`args`, `env` and `cwd`, and in skill, command and agent bodies. Hook
commands are **not** rewritten: the four names are exported to the hook's
process instead, and the shell the hook already runs under expands them —
which leaves the command exactly as its author wrote it, so the trust review
shows, and pins, the real text (*Trust* below).

What is read and not used is said on the plugin's page rather than dropped
in silence: LSP servers, output styles, themes, monitors, workflows, `bin/`,
`userConfig`, `channels`, codex's `.app.json` connectors. None of them has a
counterpart here.

## What a plugin adds, and under what name

| Component | Becomes | Named |
| --- | --- | --- |
| Skill | a skill the `skill` tool loads and `$` mentions complete | `{plugin}:{skill}` |
| Command | a skill — its markdown is the body | `{plugin}:{command}`, subdirectories adding segments (`{plugin}:db:migrate`) |
| Agent | a subagent type | `{plugin}:{agent}`, subdirectories adding segments |
| Hook | a hooks layer after the user's and the project's | by its event and matcher, as every hook is |
| MCP server | a server in the `/mcp` manager's **Plugin** scope | its own name |

The prefix is the **manifest's** name — what Claude Code and codex both
namespace under — while the install key is the marketplace entry's
(`{entry}@{marketplace}`), since two marketplaces may list one plugin.

**Skills and commands.** A plugin skill is a `SkillMetadata` like any other,
carrying the plugin it came from so its body's `${CLAUDE_PLUGIN_ROOT}` can be
expanded; it lists in `/skills`, turns off there per directory, and loads by
the same tool. A command is a skill whose `SKILL.md` is the command file:
Claude Code merged the two itself (*"commands/ — skills as flat Markdown
files"*), and this app has one way for the user to invoke a skill — the `$`
mention — so a plugin command is reached as `$commit-commands:commit`. Its
description is the frontmatter's, else the first non-blank line of the body.
`$ARGUMENTS` and `` !`cmd` `` are left as written: the skill loader rewrites
nothing in a body (`docs/skills.md`), and a model reading *"Current status:
!`git status`"* runs the command, which is what the line asks for.

The `$` mention grammar grows by two characters to hold the name: `:` and
`.` continue a mention **when a name character follows them**, so
`$acme.tools:deploy` is one mention and the full stop in *"use $dataviz."*
still ends one (`skills::mention_token`).

**Agents.** A plugin agent file is this app's agent file — the format came
from Claude Code (`docs/subagents.md`) — with two rules of its own. It
**always runs on the session's model**: `model: sonnet` names a model of a
provider this session may not be on, so the key is ignored rather than
turned into a request that fails. And its `tools:` list is **translated**
(`plugins::agents::translate_tools`): `Read`, `Write`, `Edit`, `Bash`,
`Skill` and `mcp__…` are this app's own names; `Glob`, `Grep` and `LS` become
`Bash`, since a search here is a shell command, as the built-in read-only
`explore` type's own list says; and a tool with no counterpart (`WebFetch`,
`TodoWrite`, `Task`) is dropped. An agent that asked only for tools that do
not exist here keeps an allowlist that matches nothing, which is the truth.

**MCP servers** keep the name their plugin gave them — codex's rule, and
the one that leaves `mcp__context7__query` the same tool whether the user
configured the server by hand or installed the plugin. A name the user's or
the project's config already uses, or an earlier plugin's, is left to its
first owner and the plugin's page says the server was not added.

## Marketplaces

A marketplace is a directory with a catalog file, read from the first of

1. `.agents/plugins/marketplace.json` (codex)
2. `.claude-plugin/marketplace.json` (Claude Code)

— `{ "name", "plugins": [ { "name", "source", … } ] }`, every other key
ignored, each entry validated on its own so one bad entry does not cost the
catalog. A directory with no catalog but a plugin manifest at its root is a
**marketplace of one**, named after its plugin: Agent Plugins defines no
distribution format, so a plugin's own repository has to be something a
user can add.

An entry's `source`:

| Shape | Meaning | Installable |
| --- | --- | --- |
| `"./plugins/x"` (or a bare name under `metadata.pluginRoot`) | a directory of the marketplace | yes |
| `{"source": "local", "path": "./plugins/x"}` | the same (codex) | yes |
| `{"source": "github", "repo": "o/r", "ref"?, "sha"?}` | a GitHub repository | yes |
| `{"source": "url", "url", "path"?, "ref"?, "sha"?}` | any git repository | yes |
| `{"source": "git-subdir", "url", "path", "ref"?, "sha"?}` | one directory of a git repository | yes |
| `npm`, `archive`, `command`, anything else | — | **no** — listed, with the reason |

A relative path must start with `./` (or be `.`), hold no `..`, and stay in
the marketplace. A `sha` wins over a `ref`, and the checkout must land on it.
`command` sources run a shell command to *find* a plugin; that one is refused
on purpose rather than for want of time.

The entry's other keys (`description`, `version`, `author`, `category`,
`homepage`, `keywords`, and the component keys) describe the plugin in the
list before it is fetched, and **are the manifest** of a plugin directory
that has none — Claude Code's rule, and codex's fallback.

### Adding one

`/plugins` → Marketplaces → Add, or `alter-zero plugin marketplace add`:

| Typed | Read as |
| --- | --- |
| `owner/repo`, `owner/repo@ref`, `owner/repo#ref` | `https://github.com/owner/repo.git` |
| `https://…`, `http://…`, `ssh://…`, `git@host:path`, each with an optional `#ref` | a git URL |
| `./path`, `../path`, `/path`, `~/path` | a local directory, **read in place** |

The name is the catalog's own `name`, never the user's — which is also what
keeps two sources from being added under one name.

## On disk

Everything is under the config home (`ALTER_ZERO_CONFIG_DIR`, else
`~/.alter-zero`):

```
plugins.json                               the registry
plugins/marketplaces/{name}/               a git marketplace's checkout
plugins/installed/{marketplace}/{plugin}/  an installed plugin — PLUGIN_ROOT
plugins/data/{marketplace}/{plugin}/       its data directory — PLUGIN_DATA
plugins/staging/                           clones and copies in flight
```

An installed plugin has **one path for as long as it is installed**, not a
directory per version as both references keep. A version in the path makes
every update a new place: the hooks' and servers' trust records, which are
keyed by the file they were read from, would start over, and a skill
loaded into a conversation would be a different directory after the update.
The version is a fact about the copy and lives in the registry.

`plugins.json`:

```json
{
  "marketplaces": {
    "claude-plugins-official": {
      "source": { "kind": "git", "url": "https://github.com/anthropics/claude-plugins-official.git" },
      "revision": "1a2b3c4d…",
      "updated_at": 1763380000
    },
    "team": { "source": { "kind": "local", "path": "/home/me/team-plugins" } }
  },
  "plugins": {
    "hookify@claude-plugins-official": {
      "version": "0.1.1",
      "revision": "1a2b3c4d…",
      "installed_at": 1763380000
    }
  },
  "projects": {
    "/home/me/work/api": { "disabled": ["hookify@claude-plugins-official"] }
  }
}
```

A plugin is installed for the user and **on in every directory until one
turns it off** — `skills.json`'s shape and reason (`docs/per-directory-state.md`):
which plugins earn their context is a fact about a project. Turning one off
everywhere is uninstalling it.

Every write is a read-modify-write over the file as it is on disk at that
moment, through a temp file and a rename, and a `plugins.json` that will not
parse is **never written over** — the session runs with no plugins and says
so, and the installs it records wait for the typo to be fixed (`trust.json`'s
posture: a registry silently reset is a set of plugins silently gone).

## Installing

1. **Find the source.** A relative entry is a directory of the marketplace.
   A git entry is cloned into `plugins/staging/` — `git init`, a `--depth 1`
   fetch of the `sha`, the `ref`, or `HEAD`, a checkout — and a `sha` the
   server will not serve shallow falls back to a full clone. The checkout
   must be on the pinned `sha`.
2. **Read it.** The manifest by the precedence above; an Agent Plugin that
   fails validation stops here. With no manifest, the entry is one.
3. **Copy it** into `plugins/staging/`, then rename it into place — over the
   old copy on an update, with the old one moved aside first and put back if
   the rename fails. Regular files and directories only; a symlink that
   resolves inside the plugin is copied as what it points at, one that leaves
   it is skipped and noted; `.git` is left behind; the copy stops at 20,000
   entries or 256 MiB, which no plugin reaches and a hostile repository must
   not pass.
4. **Record it** in `plugins.json` with its version — the manifest's, else
   the entry's — and the commit it came from.

`git` is the user's own, found on `PATH`, run with `GIT_TERMINAL_PROMPT=0`
(a private repository uses the credential helper or the ssh agent, and is
never asked for a password over the TUI), `GIT_ALLOW_PROTOCOL` holding the
`ext::` transport out, the repository-redirecting variables removed, stdin
closed, and a cancel token polled every 20 ms — Esc on the page stops a
clone. Each git command has two minutes.

An **update** is the same four steps; the plugin's data directory is never
touched. Updating a git marketplace re-clones it beside the old checkout and
swaps the two, and marks which of its installed plugins now differ from
their catalog entry — by version, or by pinned commit.

## Trust

A plugin's skills, commands and agents are text the model reads, and are in
force the moment the plugin is on, as a project's are. Its hooks and its MCP
servers **run**, so they go through the review every hook and every project
server already goes through (`docs/config-review.md`):

- a plugin's hooks files are a third layer of `TrustState::hook_layers`,
  after the user's and the project's, keyed by their path under the installed
  copy — so each hook is trusted by event, matcher and command, `/hooks`
  lists it, and an update that changes a command asks again and shows what
  it was;
- a plugin's MCP servers are held by the manager (`McpHold::Review`) until
  the servers page enables them, each decision pinned to the whole entry and
  recorded under the plugin's MCP file — so `/mcp`'s Enable and Disable on a
  plugin server are the review's decision too, kept in `trust.json`, never in
  the user's MCP file. A plugin server whose name the user or the project
  already uses is not started, and the startup toast says so.

Installing from `/plugins` opens that review as soon as the copy is in place,
so the commands are on screen, verbatim, before anything of the plugin has
run; installing from the shell leaves them for the next launch's review. An
install confirms nothing by itself, which is the difference from both
references' "installing is trusting": the listing a user picked a plugin
from says what it is for, not what it executes.

## The `/plugins` page

The composer-replacing page (`app::PluginsMenu`, `ui::plugins_view`), opening
mid-turn like every other.

```
────────────────────────────────────────────────────────────────────────────────
  Plugins
  2 installed · 319 plugins in 2 marketplaces

   Installed (2)    Discover (319)    Marketplaces (2)

  ❯

  → commit-commands   ◯ disabled   Streamline your git workflow with simple…
    hookify           ✔ enabled    Easily create hooks to prevent unwanted …
    (1/2)

    Streamline your git workflow with simple commands for committing,
    pushing, and creating pull requests
    commit-commands@claude-plugins-official · v1.0.0 · 3 commands

  Type to search · ←/→ tabs · Enter to open · Space to toggle · Esc to close
────────────────────────────────────────────────────────────────────────────────
```

- **Tabs.** *Installed*, *Discover* (every plugin of every marketplace) and
  *Marketplaces*, the active one a filled chip in the accent — the
  `AskUserQuestion` strip's dress. ←/→ and Tab/Shift+Tab move, wrapping, and
  clear the search. Each tab names its count once the catalog is read. The
  page opens on *Installed* when anything is, else on *Discover*, else —
  nothing added yet — on *Marketplaces*, whose last row adds one. Three
  fixed tabs rather than codex's one per marketplace: the
  strip is one width whatever the user has added, and a marketplace's own
  plugins are a search away (the search matches the marketplace's name).
- **Rows.** `→ {name}  {mark} {status}  {description}`, the name and status
  columns sized off every listed row so they hold still as the window
  scrolls, the description `…`-cut. An installed plugin is `✔ enabled` (the
  one green mark) or `◯ disabled` here, `✘ error` (red) when its copy cannot
  be read; one that is not is `+ available`, or `– unavailable` when its
  source is one this app does not fetch. An update opens the description on
  an amber `▲ update ·`. Two rows of one name — the same plugin in two
  marketplaces — show their whole ids. Installed plugins sort first, then by
  name — codex's order. The Marketplaces tab's rows are `→ {name}  {n}
  plugins · {k} installed · {source}` (a red `✘` and why, for one whose
  catalog cannot be read) over the `+ Add a marketplace` row.
- **Search** matches the name, the id, the marketplace, the description and
  the keywords, case-insensitively.
- **Under the list**, the highlighted plugin's id, version and what it is
  made of — or, on *Marketplaces*, the marketplace's source and when it was
  last updated.
- **Keys.** ↑/↓ (wrapping), PageUp/PageDown, Home/End; Enter opens the row;
  Space turns an installed plugin on or off for this directory — the row
  answers at once, the write follows; Ctrl+R reads the marketplaces again;
  Esc clears the search, then closes; Ctrl+C closes. Space is the toggle,
  so a search never holds one (the `/skills` rule).

**A plugin's page** is the `/mcp` server page's twin: the name in the title
dress over its id, version and author; its description; the fact rows —
status, format, source, and each kind of component by name; a line for what
it carries that is not used here; the trust note for one not yet installed;
then the numbered actions its state affords — `Install`; or `Disable` /
`Enable`, `Update`, `Uninstall` — and Esc goes back, to the plugin's own
row wherever an install or an uninstall has since sorted it. A homepage is
a link. A plugin kept in its marketplace is read before it is installed, so
the page names what it adds — and its manifest's version, author and
homepage when the marketplace entry leaves them out.

**A marketplace's page**: its source, revision, age and location, then
`Browse plugins` (Discover, searched for it), `Update`, `Remove` — which
asks once more, naming the plugins it would uninstall.

**Adding a marketplace** is a one-field page; Enter adds, and a failure
stays on the page in red under what was typed.

**While a job runs** — a clone, an install — the page that started it says
what is happening over its hint, `Installing deploy… (4s · esc to cancel)`
in the status line's grammar, every other operation waits (a second one is
refused with a toast naming the first), and Esc cancels the job: the git
child is killed, and the worker still answers with what is on disk, so the
page never waits on a job that is gone. One job at a time: they all write
one registry. The first read of a page that has never had one shows that
line where the rows will be. Each answer carries its round (`/usage`'s
rule), so one that comes back after the page closed — or after a newer job
began — is not taken; a change it made is still loaded into the session,
and a failure nobody saw is a toast.

On a short terminal the list page is a picker like any other and sheds rows
down the picker ladder (`ui::plugins_view::list_page`, `docs/view-flow.md`
*A page that sheds rows before it flows*): the highlighted plugin's
description first, leaving its one-row facts; then the facts and the
counter, the rows down to the compact three between the `↑`/`↓` slots, the
title and count kept; then those too, the **tab strip** kept — ←/→ still
move between tabs, so it outlasts the title; then the compact rungs down to
the search line alone. So the search line, the highlighted row and the hint
stay on screen and the list never flows. A detail page bottom-anchors
instead, its actions, job line and hint staying on screen while its top
flows into scrollback.

## From a shell

```
alter-zero plugin marketplace add <source>
alter-zero plugin marketplace list
alter-zero plugin marketplace update [<name>]
alter-zero plugin marketplace remove <name>
alter-zero plugin install <plugin>[@<marketplace>]
alter-zero plugin uninstall <plugin>[@<marketplace>]
alter-zero plugin update [<plugin>[@<marketplace>]]
alter-zero plugin enable <plugin>[@<marketplace>]
alter-zero plugin disable <plugin>[@<marketplace>]
alter-zero plugin list [--available]
```

Both references' spellings work: `add` and `remove` are `install` and
`uninstall`, `upgrade` is `update`, `rm` is `remove`. A bare plugin name is
enough while only one marketplace lists it — two that do are named back with
their marketplaces. `enable` and `disable` are for the directory the command
runs in, as the page's toggle is.

The grammar is the pure `cli::PluginCli` (`plugin` routes as the first
argument only, like `mcp`, so it is a prompt anywhere else), its page
`alter-zero plugin --help`; the work is the same store operations the page
runs, in `tui::plugin_cli`, before the terminal is touched. `update` reads
each git marketplace the plugins came from again first, then reinstalls the
ones whose marketplace has a newer copy — by version, pinned commit, or the
fingerprint of a plugin kept in a directory — and says when nothing is.
An install that brings hooks or servers says the app asks about them at its
next start: the shell asks nothing. Exit `0` done, `1` anything that could
not be (a name nothing lists, a failed clone, a `plugins.json` that will not
parse), `2` a grammar error, over the plugin page's usage block.
`tests/plugin_cli.rs` drives every verb against the real binary, offline.

## Switches

- `ALTER_ZERO_PLUGINS=0` loads no plugin for the run; `/plugins` still
  opens and says so.
- `ALTER_ZERO_PLUGINS_DIR` moves the `plugins/` tree and `plugins.json`
  with it — what a test points at a temp directory.

## Where it lives

- `src/plugins/` — the pure half and the store:
  `id` (names, `plugin@marketplace`), `manifest` (the three formats),
  `marketplace` (catalogs and entry sources), `source` (what the user types
  for a marketplace), `registry` (`plugins.json`), `layout` (the paths),
  `components` (what a plugin directory holds), `mcp` (the two MCP file
  formats, onto `mcp::McpServerConfig`), `agents` (plugin agent tools),
  `git` and `store` (cloning, copying, installing), `catalog` (the snapshot
  `/plugins` shows, and `load_enabled` — what a session takes from the
  plugins it has on).
- `src/app/plugins_menu.rs` — `PluginsMenu`, its pages and keys, `PluginOp`.
- `src/ui/plugins_view.rs` — the page.
- `src/tui/plugins.rs` — the boundary: loading at bootstrap, the worker, the
  reload after every change. `src/tui/plugin_cli.rs` — the subcommands.
- `smoke.sh` Phase 161 drives the page in tmux against a marketplace kept
  in a directory: added through the field, a plugin installed from its page,
  one with a hook installed and the review answered, its skill completed as
  a `$` mention, the installs persisted for a second process, a toggle with
  Space, and `plugin list` reading the same store.

## What this deliberately is not

- **No `npm`, `archive` or `command` sources**, no marketplace that is one
  JSON file at a URL. Each needs something this app does not link (an npm
  client, a zip reader) or should not do (run a catalog's command).
- **No dependencies between plugins**, no `userConfig` prompts, no
  auto-update. An update is something the user asks for.
- **No project-scoped installs.** A repository can still ship its own
  skills, agents, hooks and servers the way it always could
  (`docs/project-config.md`).
- **No custom slash commands.** A plugin command is a skill; the palette
  stays the app's own.
