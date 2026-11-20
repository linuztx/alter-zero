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

A Codex plugin's server writes its OAuth client in codex's spelling — an
`oauth` object of `client_id`, `client_secret`, `callback_port` and
`callback_url`, the `scopes` on the entry beside it — and the shared parser
reads all of it (`docs/mcp.md` *Pre-registered clients*); `oauth_resource`
and codex's `.app.json` connectors are not read. A plugin that ships a real
client is signed in to as that client, on the redirect codex would send. One
that ships a **placeholder** (`<GMAIL_PUBLIC_CLIENT_ID>`, as OpenAI's gmail
and google-drive do) has no client: Authenticate in `/mcp` asks for one, on
the plugin's callback port, and the sign-in asks for the plugin's scopes.
What is typed there is kept in the token store under the server's URL, not
in the plugin's files, so updating or reinstalling the plugin never loses it.

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
| MCP server | a server in the `/mcp` manager's **Plugin** scope | `plugin:{plugin}:{server}` — its tools `mcp__plugin_{plugin}_{server}__{tool}` |

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

**MCP servers** are named as Claude Code names them: `plugin:{plugin}:{server}`
declared, `mcp__plugin_{plugin}_{server}__{tool}` on the wire
(`plugins::plugin_server_name`) — the plugin's name, then the server's key.
A plugin's own hook matcher (`mcp__plugin_my-plugin_db__.*`) or an agent's
`tools:` list written for Claude Code finds its tools under that name, and
no plugin's server can take the name of one the user configured or another
plugin's. A stdio server's `${CLAUDE_PROJECT_DIR}` is the session's directory
(an Agent Plugin's entry expands nothing but its two variables, §9.2).

**Hooks** have their plugin's directories substituted into the command —
`${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}` and the Agent Plugins
spellings — and exported to it too, Claude Code's rule
(`HooksFile::with_substitutions`): the review shows the command that runs,
and two plugins that both write `${CLAUDE_PLUGIN_ROOT}/hooks/run.sh` stay two
hooks rather than deduplicating into the first. Claude Code's **exec form**
(`command` and `args`) runs each argument as one word exactly as written,
a path placeholder still the directory it names — for every hook, a
plugin's or the user's.

**Two enabled plugins of one name** would share every name above: the first
in id order loads, and the other is listed `✘ not loaded`, its page saying
which plugin is on under its name — turn one off (`load_enabled`).

## Marketplaces

A marketplace is a directory with a catalog file, read from the first of

1. `.agents/plugins/marketplace.json` (codex)
2. `.claude-plugin/marketplace.json` (Claude Code)

— `{ "name", "plugins": [ { "name", "source", … } ] }`, every other key
ignored, each entry validated on its own so one bad entry does not cost the
catalog. A directory with no catalog but a plugin manifest at its root is a
**marketplace of one**, named after its plugin: Agent Plugins defines no
distribution format, so a plugin's own repository has to be something a
user can add. The name becomes a directory under the plugins tree, so it
must be a plain name — a manifest naming itself `../../.ssh` is refused
before any path is made of it — and a manifest that names nothing is named
after its repository (`my-tool.git` is `my-tool`).

### Reserved names

A catalog cannot pass for an official one. The names Claude Code reserves
for Anthropic's marketplaces (`claude-plugins-official`,
`claude-code-marketplace`, `anthropic-agent-skills` and the rest,
`marketplace::ANTHROPIC_RESERVED`) come only from `github.com/anthropics/`;
the names codex reserves (`openai-curated`, `openai-bundled`, …,
`OPENAI_RESERVED`) only from `github.com/openai/`. Another spelling of one —
`claude.plugins.official`, a trailing `.`, another case — is the same name
(Claude Code's rule). Anything else is anyone's.

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

`/plugins` → **+ Add**, where Claude Code's official catalog
(`anthropics/claude-plugins-official`) and codex's
(`openai/plugins`) are one Enter away — or type a source there, or choose
**Add a marketplace…** there, a field for any source, or run
`alter-zero plugin marketplace add`:

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
plugins/installed/{marketplace}/{plugin}/  a git marketplace's plugin, copied — PLUGIN_ROOT
plugins/data/{marketplace}/{plugin}/       its data directory — PLUGIN_DATA
plugins/staging/                           clones and copies in flight
```

A plugin of a marketplace **kept in a directory** is not copied: it loads in
place from its folder there — Claude Code's rule — so an edit to it is what
the next session (or the next `/plugins` reload) reads, with no update to
run. Its registry record names that folder (`"root"`), and an uninstall
forgets it without touching it: the folder is the user's.

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
3. **Copy it** — unless it is a plugin of a marketplace kept in a
   directory, which loads in place and is recorded with its folder — into
   `plugins/staging/`, then rename it into place — over the
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
their catalog entry — each kind of source by what it can tell
(`store::update_available`): a plugin kept in the marketplace by its files,
one pinned to a commit by the commit, one fetched from a repository of its
own by the entry's version against the one the entry **offered at install**
(never the manifest's, which may say another — the manifest wins, Claude
Code's rule — and would read as an update forever). One loaded in place is
always current.

A catalog's `ref` and `sha` reach `git` as arguments, so they are read as
what they must be — a branch or tag name, a commit id — and an entry whose
`ref` is anything else (`--upload-pack=…`, an option) is skipped with the
reason on its marketplace's tab: nothing a catalog says runs before the
review.

## Trust

A plugin's skills, commands and agents are text the model reads, and are in
force the moment the plugin is on, as a project's are. Its hooks and its MCP
servers **run**, so they go through the review every hook and every project
server already goes through (`docs/config-review.md`):

- a plugin's hooks files are a third layer of `TrustState::hook_layers`,
  after the user's and the project's, keyed by their path — under the
  installed copy, or the plugin's own folder for one loaded in place — so
  each hook is trusted by event, matcher and command, `/hooks` lists it, and
  an update that changes a command asks again and shows what it was. The
  review shows each file under its plugin (`Plugin hello@demo · …`), and a
  servers page made of plugins' servers says they were found in your
  plugins, not in this project;
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
mid-turn like every other — codex's `/plugins`, a tab per marketplace.

```
────────────────────────────────────────────────────────────────────────────────
  Plugins
  The plugins installed on this machine
  3 installed · 3 on in this directory

   All (382)    Installed (3)    claude-plugins-official (317)  ›

  ❯

  → context7   ✔ enabled   Upstash Context7 MCP server for up-to-date documenta…
    github     ✔ enabled   Inspect repositories, triage pull requests and issue…
    hookify    ✔ enabled   Easily create custom hooks to prevent unwanted behav…
    (1/3)

    Upstash Context7 MCP server for up-to-date documentation lookup. Connects t…
    context7@claude-plugins-official · 1 MCP server

  ←/→ tabs · Enter to open · Space to toggle · Esc to close
────────────────────────────────────────────────────────────────────────────────
```

- **Tabs**, codex's: *All*, *Installed*, **one per added marketplace** and
  **+ Add**, the active one a filled chip in the accent — the
  `AskUserQuestion` strip's dress. ←/→ and Tab/Shift+Tab move, wrapping, and
  clear the search; each tab names its count once the catalog is read. A
  strip too wide for the terminal shows a window around the active tab, a
  dim `‹`/`›` where tabs are hidden. The page opens on *Installed* when
  anything is, else on *All*, else — nothing added yet — on *+ Add*. Three
  fixed tabs were tried in this strip's place and dropped — *Installed*,
  *Discover* and *Marketplaces*, first as chips with counts and then as one
  quiet line, `Installed · Discover · Marketplaces`, with the marketplaces
  listed on a tab of their own and a page each.
- **Each tab's header** says what it is: *All* counts every plugin and
  marketplace, *Installed* what is on in this directory, a marketplace's tab
  its description, its counts and age, its source and commit — and, in red,
  why its catalog could not be read, or, in amber, the entries it skipped
  and the first one's reason.
- **Rows.** `→ {name}  {mark} {status}  [{marketplace}  ]{description}`,
  every column sized off every listed row so it holds still as the window
  scrolls (`row_columns`). The **name column** is the widest name, capped at
  two fifths of what the row has past its status (12 to 32 columns): on the
  official catalogs one 29-column name used to size the column for all 382
  rows, and at 60 columns no row had room left for its description — the
  marketplace was cut to `clau…` — so a name past the cap is `…`-cut now,
  whole in the facts under the list. The **marketplace** is a column of its
  own only where the listed plugins come from more than one — a
  marketplace's own tab, or *All* with one marketplace added, needs no
  column repeating its name — and only where the description keeps 30
  columns beside it (from about 112 columns on those catalogs); narrower,
  the facts under the list name it for the highlighted row. What
  follows the status is left out whole rather than cut to a stub — a
  description gets at least eight columns or none — and a row never ends on
  a column's padding. An installed plugin is `✔ enabled` (the one green
  mark) or `◯ disabled` here, `✘ not loaded` (red) when a session cannot
  load it; one that is not is `+ available`, or `– unavailable` when its
  source is one this app does not fetch. An update opens the description on
  an amber `▲ update ·`. Two rows of one name show their whole ids.
  Installed plugins sort first, then by name — codex's order.
- **Under the list**, the highlighted row's description cut to one row
  (`No description` for a plugin with none) over its facts — the id, the
  version, what it is made of — **two rows at every selection**, so ↑/↓
  never resize the page: wrapped whole, a three-row description gave way to
  a one-row one and the page's bottom rule jumped with every step — the
  official catalogs moved it from row 30 to 26 to 22 and back — each growth
  scrolling the conversation above the page. The whole description is on
  the plugin's page. The *+ Add* tab's rows say the same two things: what a
  catalog is and where it comes from; for the typed source and the row for
  any other, the forms a source takes and how to pin one.
- **+ Add** is the field and the official catalogs under it: the `❯` line
  takes a source (owner/repo, a git URL, a directory — Space types, a path
  may hold one), and its rows are what Enter adds — the typed source first,
  then Claude Code's and codex's catalogs (`SUGGESTED_MARKETPLACES`), one
  already added marked `✔ added`, where Enter shows its tab — and, last
  while nothing is typed, **`Add a marketplace…`**, which opens the add
  page below. With only the two official catalogs listed, nothing on the
  tab said the `❯` line took any other source. A success lands on the new
  marketplace's tab and says nothing more: the tab and its rows show it.
- **Search** matches the name, the id, the marketplace, the description and
  the keywords, case-insensitively.
- **Keys.** ↑/↓ (wrapping), PageUp/PageDown, Home/End; Enter opens the row;
  Space turns an installed plugin on or off for this directory — the row
  answers at once, the write follows; on a marketplace's tab **Ctrl+U**
  updates it (a git one; a directory is always current) and **Ctrl+R**
  removes it, asking once more and naming the plugins it would uninstall;
  Esc clears the search, then closes; Ctrl+C closes. The hint names the
  tab's keys and, on a narrow terminal, leaves out whole clauses — never the
  key that closes the page — rather than ending on a cut word.

The *+ Add* tab of a page with nothing added yet:

```
────────────────────────────────────────────────────────────────────────────────
  Plugins
  Add a marketplace: owner/repo, a git URL, or a directory
  Pick one below or type a source

   All (0)    Installed (0)    + Add

  ❯

  → anthropics/claude-plugins-official   Claude Code's official plugin directory
    openai/plugins                       Codex's official plugin directory
    Add a marketplace…                   owner/repo, git URL or path
    (1/3)

    Claude Code's official plugin directory
    github.com/anthropics/claude-plugins-official · its catalog is `claude-plug…

  ←/→ tabs · type a source · Enter to add · Esc to close
────────────────────────────────────────────────────────────────────────────────
```

**The add page** is one field: what a marketplace is, the `❯` line — its
dim `owner/repo` placeholder until something is typed, the caret in it —
the forms a source takes (owner/repo, a git URL, a directory; Space types,
since a path may hold one) and how to pin one (`owner/repo@ref`, `#ref` on
any git URL). Enter adds; a failure says why in red and keeps what was
typed; a success lands on the new marketplace's tab. Esc goes back to the
row it was opened from.

**A plugin's page** is the `/mcp` server page's twin:

```
────────────────────────────────────────────────────────────────────────────────
  42crunch-api-security-testing
  + not installed · 42crunch-api-security-testing@claude-plugins-official
  by 42Crunch

  Automate API security directly in Claude Code with 42Crunch - automatically
  audit OpenAPI specs, detect vulnerabilities aligned with OWASP API Security
  …

  Source:    https://github.com/42Crunch-AI/claude-plugins.git ·
             plugins/api-security-testing @ v1.5.5
  Homepage:  https://42crunch.com
  Category:  security
  Contents:  read once it is installed

  Plugins can run code on your computer — install only plugins you trust. Any
  hooks or MCP servers it brings wait for your review before they run.

  ❯ 1. Install
    2. Back to plugins

  ↑/↓ to navigate · Enter to select · Esc to go back
────────────────────────────────────────────────────────────────────────────────
```

- **Its state leads the line under its name** — `✔ enabled in this
  directory`, `◯ disabled in this directory`, `✘ installed, but not
  loaded`, `+ not installed`, `– cannot be installed`, the mark in its
  colour — beside its id, version and author: a `Status:` fact row sat
  under the description, five rows down on a long one. The line breaks
  between its clauses (`clause_lines`), never inside one: `… · by` over
  `42Crunch` was the old wrap at 60 columns.
- **The description**, its addresses links (`docs/links.md`).
- **The facts**, on a label column sized to the labels the page shows —
  `/mcp`'s is sized for its `Config location:`, four columns more than a
  plugin's page needs — an update, the format, the source (a repository's
  address a link, whole behind every wrapped piece), where it loads from
  (one loaded in place), the homepage, the category, and each kind of
  component by the name a session knows it by (`hello:greet`,
  `plugin:guard:probe`); then what it carries that is not used here.
- **The trust warning** for one not yet installed that runs something — a
  hook or an MCP server — **or whose contents are read only once it is
  installed** (a plugin in a repository of its own): the risk in Claude
  Code's opening words, then this app's own guard (*Trust* above). Before,
  that last case — the one the user knows least about — had no warning at
  all.
- **The numbered actions** its state affords — `Install`; or `Disable` /
  `Enable`, `Update` (not for one loaded in place), `Uninstall` — over
  `Back to plugins`. The primary action comes first, where codex puts
  `Back to plugins`: an install opens the review before anything runs and
  an uninstall asks once more, so the highlighted first row is the one the
  page was opened for. Esc goes back too, to the plugin's own row wherever
  an install or an uninstall has since sorted it — and the hint, here and
  on the confirmation, gives way a whole clause at a time on a narrow
  terminal, the list's rule, never losing `Esc to go back` (it was cut to
  `E…` at 40 columns).

**The confirmation** — before an uninstall, and before a marketplace is
removed with the plugins installed from it — highlights the answer the user
just chose: the question is the safeguard, so Enter confirms it, and Esc or
*Cancel* goes back. Highlighting *Cancel* meant an arrow key every time to
do what the user had already picked.

### No flicker

Acting on the page never moves it — the reported bug was Enter on
*Disable*: a job line and a gap that came for one frame, then a toast row
over the page that pushed it down a row and, three seconds later, back up,
each height change scrolling or blanking the bottom of a full screen. Now:

- **a job is said in the hint's own row** (`status_row`), and only once it
  has run `PLUGINS_JOB_DELAY` (400 ms) — a toggle answers in milliseconds,
  and a line shown for one frame is the flicker — then `Installing
  deploy… (4s · esc to cancel)`; a failure in red over that row, until the
  next key;
- **what went right is never said**: the row turns `◯ disabled`, the
  header counts it, the page leads with `✔ enabled in this directory`, an
  add lands on the new marketplace's tab, a removed marketplace's tab is
  gone. A `✔ Disabled hookify in this directory` line in the hint's seat
  repeated what the row already showed, and stood where the hint belonged;
- **no toast is raised over an open page** — one goes up only for an answer
  the page closed before taking, where nothing else can say what it did,
  and a reload's plugin errors are said on their rows;
- **the page opens on its rows**: the catalog is read on the loop as the
  page opens, so its first frame is the page at its own height, with no
  placeholder for the rows to push away. The read takes a few milliseconds
  even over the two official catalogs' 382 plugins, because a plugin that is
  not installed is read only as far as its row needs — its manifest — and
  what it adds is read for one plugin at a time, as the page shows it
  (*Memory*, below).

A toggle rewrites the rows it changes and nothing else: `smoke.sh` Phase 161
records the bytes and holds the page's bottom rule on one row throughout.

**While a job runs** every other operation waits (a second one is refused
with a toast naming the first), and Esc cancels the job: the git child is
killed, and the worker still answers, so the page never waits on a job that
is gone; the catalog is read again on the loop as the answer lands, from
whatever the job left on disk. One job at a time: they all write one
registry. Each answer carries its round (`/usage`'s rule), so one that
comes back after the page closed — or after a newer job began — is not
taken; a change it made is still loaded into the session, and a failure
nobody saw is a toast.

On a short terminal the list page is a picker like any other and sheds rows
down the picker ladder (`ui::plugins_view::list_page`, `docs/view-flow.md`
*A page that sheds rows before it flows*): the highlighted row's
description first, leaving its one-row facts; then the facts and the
counter, the rows down to the compact three between the `↑`/`↓` slots, the
header kept; then the header too, the **tab strip** kept — ←/→ still move
between tabs, so it outlasts the title; then the compact rungs down to the
search line alone. A detail page bottom-anchors instead, its actions and
status row staying on screen while its top flows into scrollback.

## Memory

Resident memory is a feature here (`docs/memory.md`), and the figure this
design is held to is a session's at rest: **a session with no plugin on
costs what it would without the plugin system**, and one with plugins what
loading them takes.

- **Startup reads `plugins.json`** — one failed `open` when there is none —
  and a plugin's files only for the plugins on in the directory
  (`load_enabled`). No catalog, no thread, no `git`. The first version of
  the page read every added marketplace's catalog on a worker behind the
  first frame and kept it, so the page could open on it: with the two
  official marketplaces added and nothing installed, that held 1.4 MB of a
  session's resident memory at rest — the catalog, and the heap of the
  thread that read it, which glibc keeps (`docs/memory.md`'s sixth rule).
- **The catalog lives only while the page is open**: read on the loop as it
  opens and again when a job's answer lands for a page still waiting on
  it, and dropped with the page. The worker that runs a job answers with
  what it did and nothing it read.
- **A catalog read takes what the rows need.** For a plugin that is not
  installed, that is its marketplace entry and its manifest — the
  description and version a row shows, which Codex's official catalog
  leaves to the manifest. What it adds is read for the one plugin the page
  shows, the highlighted row or the plugin's own page, before the frame
  that shows it (`plugins::read_contents`, from
  `Session::tick_plugins_page`). That walk — every skill's `SKILL.md`
  parsed, every command and agent file read — was 84% of the read: over the
  two official catalogs' 382 plugins it went from 42 ms to 7 ms, the
  process's own start included, which is what lets it run on the loop. An
  installed plugin is read whole, since its row's state comes from its
  files.
- Opening the page on those 382 plugins and closing it leaves 0.7 to 1 MB
  of freed heap behind, and a second and a third visit do not add to it.
  The first version, whose session already held a catalog at rest, held
  1.2 to 1.5 MB more after the same visits.

**Where the code sits decided more than any of that.** The plugin system's
code, none of which runs without a plugin, moved a configured session's
idle figure by 0.95 MB under the linker's default order. The
idle text layout (`docs/text-layout.md`) puts the functions a session runs
at rest side by side, with the ones a session with a plugin on adds — the
registry read, the manifests, the skill and agent walks, the hook layers —
as a group of their own after them, so a session with no plugin keeps none
of the plugin system resident and one with a plugin keeps that group:

| At rest, MB — just written / read from disk | `release` | `dist`, the shipped binary |
|---|---|---|
| a configured session, `main` before the plugin system | 14.75 / 16.11 | — |
| a configured session, the linker's default order | 15.30 / 17.06 | 13.96 / 15.86 |
| a configured session, laid out | **9.38 / 10.22** | **8.02 / 8.90** |
| a plugin on (a skill, a command, an agent, a trusted hook), laid out | **9.65 / 10.44** | **8.29 / 9.08** |
| the two official marketplaces added, nothing installed — the first version, which read and kept their catalog at startup, default order | 16.52 / 18.43 | — |
| the same, now, laid out | **9.02 / 10.12** | **8.00 / 8.68** |

`scripts/text_layout.sh measure --plugins` is the measurement for a session
with a plugin on, and `--config-home` takes one of your own.

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
- `src/tui/plugins.rs` — the boundary: loading at bootstrap, the page's
  reads on the loop, the worker, the reload after every change. `src/tui/plugin_cli.rs` — the subcommands.
- `smoke.sh` Phase 161 drives the page in tmux against a marketplace kept
  in a directory: the official catalogs offered on an empty config home, the
  marketplace added by typing its folder, a plugin installed in place from
  its page, one with a hook and a server installed and the review answered,
  its server's tool named `mcp__plugin_guard_probe__…` in `/mcp`, its skill
  completed as a `$` mention, the installs persisted for a second process
  that opens on them at once, a toggle with Space that writes no clear and
  never moves the page, and `plugin list` reading the same store.

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
