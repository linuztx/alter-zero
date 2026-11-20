# Plugins — marketplaces, installs, and what a plugin adds to a session

A plugin is a folder someone else maintains that extends the agent: **skills**,
**subagent types**, **lifecycle hooks** and **MCP servers**, packaged
together and installed from a **marketplace** (a git repository or a local
folder that lists plugins). Three formats are read as they are — the
vendor-neutral **Agent Plugins** standard (<https://agent-plugins.org>, spec
1.0.0), **Claude Code** plugins and **OpenAI Codex** plugins — and two
marketplaces are offered out of the box:

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
  trust, hooks, agents, commands — is left to the client, which may keep
  its own files in a top-level directory named for a reverse-domain
  namespace.
- **Claude Code** — `.claude-plugin/plugin.json` (optional; the marketplace
  entry can stand in for it) over default locations: `skills/`, `commands/`,
  `agents/`, `hooks/hooks.json` (the settings shape wrapped in
  `{"hooks": …}`, often beside a `description`), `.mcp.json` (wrapped in
  `mcpServers` or a bare map; no `type` means stdio). Components are
  namespaced `plugin:name`, MCP servers `plugin:<plugin>:<server>` with
  tools `mcp__plugin_<plugin>_<server>__<tool>`. `${CLAUDE_PLUGIN_ROOT}`
  and `${CLAUDE_PLUGIN_DATA}` resolve in hook commands, MCP configs and
  Markdown bodies; `.mcp.json` also expands `${VAR}` / `${VAR:-default}`
  from the environment. Marketplaces are `.claude-plugin/marketplace.json`;
  an entry's `source` is a relative path or an object (`github`, `url`,
  `git-subdir`, `npm`, `archive`, `command`).
- **Codex** — `.codex-plugin/plugin.json` with a large `interface` block for
  its app, over `skills/`, `.mcp.json`, `hooks/hooks.json`, `.app.json`
  (ChatGPT connector ids, meaningless anywhere else). It reads a root
  `plugin.json` first when its `$schema` is an agent-plugins.org one, then
  `.codex-plugin`, `.claude-plugin`, `.cursor-plugin` — and a symlinked or
  non-regular manifest higher in that order **stops** the lookup instead of
  falling through, so a manifest cannot hide behind another. Marketplaces
  are `.agents/plugins/marketplace.json` (then `.claude-plugin/…`), named by
  the manifest's own `name`, cloned whole, and re-checked over the network
  **at every startup**. Its `/plugins` is a tab strip — every plugin, the
  installed ones, one tab per marketplace, an add tab — over a search line
  and one row per plugin. Skills are namespaced `plugin:skill`; Claude
  `commands/` are converted into skills at install and `agents/` is
  ignored. Plugin hooks run only once trusted.
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

Codex's shape — marketplaces, `name@marketplace` ids, its tabbed `/plugins`
browser and a `plugin` command family — with:

- **Every format, read natively.** Agent Plugins strictly by its
  specification; Claude Code and Codex by one lenient reader over the union
  of their layouts, including the parts Codex drops (`agents/`,
  `${CLAUDE_PLUGIN_ROOT}` in `.mcp.json`, the `github` source form).
- **Nothing a plugin runs is run unseen.** Its hooks and MCP servers enter
  through the same startup review every other hook and project server
  passes (`docs/config-review.md`), are asked again after every update, and
  the page that installs a plugin says what it will bring.
- **Enable, disable, update and cancel everywhere** — on the page and in the
  CLI — with the real error on screen when something fails.
- **No network, no `git`, no catalog at startup.** A session that has no
  plugins reads one small file that does not exist and stops there. Codex
  syncs its curated repository and every configured marketplace on every
  launch; here a marketplace is fetched when the user asks for it, and its
  catalog is read when `/plugins` opens and dropped when it closes
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
   manifest** (Claude Code's rule; the entry is saved beside the install —
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
held inside the resolved plugin root. The component types the standard
leaves to each client reach this one two ways:

- **This client's extension namespace**, `io.github.linuztx.alter-zero`
  (the reverse of the project's home, the convention for a client with no
  domain of its own): the top-level directory of that name is laid out as a
  Claude Code plugin's — `agents/`, `commands/`, `hooks/hooks.json` —
  exactly where §8.2 puts a client's files. Every other namespace is ignored
  without a look at its value.
- **A package that is also a Claude Code or Codex plugin** — it carries one
  of the legacy manifest directories — still contributes its agents,
  commands and hooks through that legacy layout, the way Codex overlays its
  own manifest, so adopting the portable manifest never costs a plugin its
  hooks. Its skills and MCP servers stay the standard's alone.

**Legacy mode** is the union of what the two clients read, by Claude Code's
documented rules (they are a superset of Codex's for every plugin in either
marketplace):

| Component | Default location | Manifest key | Rule |
| --- | --- | --- | --- |
| Skills | `skills/<name>/SKILL.md` | `skills` (path or paths) | added to the default |
| Commands | `commands/**/*.md` | `commands` (path or paths) | replaces the default |
| Agents | `agents/**/*.md` | `agents` (path or paths) | replaces the default |
| Hooks | `hooks/hooks.json` | `hooks` (path, paths, inline object) | merged with the default |
| MCP servers | `.mcp.json` | `mcpServers` (path, inline object, list) | merged, a later name replaces an earlier |

A manifest path must begin `./`, may not contain `..`, and must resolve
inside the plugin. Display metadata is read from whichever spelling is
there: `displayName` or `interface.displayName`, `description` or
`interface.shortDescription` / `longDescription`, `author` as an object or a
string, `interface.category`, `interface.developerName`, `homepage` or
`interface.websiteURL`.

What a plugin may ship that this client cannot use is **named, never
silently dropped** — the plugin's page and `plugin info` list it: `.app.json`
apps (ChatGPT connectors), LSP servers, output styles, themes, monitors,
workflows, `bin/`, `userConfig`, channels, mods (`modules` in a hooks file).

### What a plugin adds, and under what name

| It ships | It becomes | Named | Reviewed first |
| --- | --- | --- | --- |
| `skills/<n>/SKILL.md` | a skill in the listing, the `$` picker and `/skills` | `plugin:n` | no — inert Markdown |
| `commands/<n>.md` | a skill whose body is that file | `plugin:n`, a subfolder adding a segment | no |
| `agents/<n>.md` | a subagent type | `plugin:n`, a subfolder adding a segment | no — its calls still meet the permission gate |
| a hooks file | a hooks layer after the user's and the project's | by event and matcher | **yes** — the hooks page |
| MCP servers | servers in `/mcp`'s **Plugin** scope | `plugin:{plugin}:{server}` | **yes** — the servers page |

The prefix is the **manifest's** name — what both references namespace
under — while the install key is the marketplace entry's
(`{entry}@{marketplace}`), since two marketplaces may list one plugin. Two
enabled plugins of one manifest name would share every name above: the
first in id order loads and the other is listed `✘ not loaded`, its page
naming the one that is on.

- **Skills** join `SkillRegistry` after the user's and the project's own, as
  `plugin:skill` — the reference spelling, and the one skill bodies written
  for either client use to point at each other. A bare `skill` still
  resolves when exactly one plugin provides it. A body's
  `${CLAUDE_PLUGIN_ROOT}` / `${PLUGIN_ROOT}` (and the two `…_DATA` names) is
  expanded beside the existing `${…SKILL_DIR}`; `allowed-tools` grants
  nothing.
- **Commands are skills.** Claude Code merged the two itself, and this app
  has one way for a user to invoke a skill — the `$` mention — so a plugin
  command is reached as `$commit-commands:commit`; the palette stays the
  app's own. Its description is the frontmatter's, else the first non-blank
  line of the body. `$ARGUMENTS` and `` !`cmd` `` are left as written: the
  model reads *"Current status: !`git status`"* and runs the command through
  the permission gate, which is what the line asks for.
- **The `$` mention grammar** grows by two characters to hold these names:
  `:` and `.` continue a mention when a name character follows them, so
  `$acme.tools:deploy` is one mention and the full stop in *"use $dataviz."*
  still ends one.
- **Agents** join the subagent roster as `plugin:agent`. A plugin agent
  **always runs on the session's model** (`model: sonnet` names a model of a
  provider this session may not be on), and its `tools:` list is
  translated: `Read`, `Write`, `Edit`, `Bash`, `Skill` and `mcp__…` are this
  app's own names; `Glob`, `Grep` and `LS` become `Bash`, since a search
  here is a shell command; a tool with no counterpart (`WebFetch`,
  `TodoWrite`, `Task`) is dropped. An agent that asked only for tools that
  do not exist here keeps an allowlist that matches nothing, which is the
  truth.
- **Hooks** are one more layer in `TrustState::hook_layers()` per file, so
  the review, `/hooks`, trust and the runner treat them as they treat the
  user's own. `${CLAUDE_PLUGIN_ROOT}`, `${PLUGIN_ROOT}`,
  `${CLAUDE_PLUGIN_DATA}` and `${PLUGIN_DATA}` are substituted into each
  command when the file is loaded (so two plugins' identical command text
  never collapses into one handler, and the review shows the command that
  runs) and exported to the process. The exec form (`command` + `args`)
  passes each argument as one word. A file that carries keys this client
  does not model (`modules`) still loads its hooks; one with no `hooks` at
  all is not an error.
- **MCP servers** are named as Claude Code names them —
  `plugin:{plugin}:{server}` declared, `mcp__plugin_{plugin}_{server}__{tool}`
  on the wire — so a plugin's own hook matcher or an agent's `tools:` list
  written for Claude Code finds its tools, and no plugin's server can take
  the name of one the user configured or another plugin's. Legacy files get
  the four plugin placeholders and `${CLAUDE_PROJECT_DIR}` expanded, a
  relative `cwd` and a `./` command resolved against the plugin root, then
  the ordinary `${VAR}` expansion (refused, as for a project server, where
  it would put a provider key in a remote header). Agent Plugins `mcp.json`
  gets exactly what the specification allows — the two placeholders in
  `args`, `env` values and `cwd`, a working directory of the plugin root,
  nothing else expanded — and both kinds get `PLUGIN_ROOT`, `PLUGIN_DATA`,
  `CLAUDE_PLUGIN_ROOT` and `CLAUDE_PLUGIN_DATA` in a stdio server's
  environment.

`PLUGIN_DATA` is `plugins/data/<marketplace>/<plugin>/`, created before the
first hook or stdio server of the plugin is loaded, kept across updates and
removed with the plugin.

### Marketplaces

A marketplace root holds one catalog, the first that exists of
`.agents/plugins/marketplace.json`, `.claude-plugin/marketplace.json`,
`.cursor-plugin/marketplace.json`. Its `name` is the marketplace's name —
never derived from the URL — and it becomes a directory, so it must be a
plain name (letters, digits, `.`, `_`, `-`; a manifest naming itself
`../../.ssh` is refused before any path is made of it). Adding a second
source under a name already in use is refused — and so is one whose name
differs from an added one **only by case** (`Team` beside `team`), naming
both: on a case-insensitive filesystem (macOS, Windows) the two are one
folder, and the second would overwrite the first's checkout. The same rule
holds inside a catalog (`Lint` listed after `lint` is a duplicate) and for
the plugins installed from one marketplace. Nothing is lowercased: a name
keeps the spelling it was added with.

A source with **no catalog but a plugin manifest at its root is a
marketplace of one**, named after its plugin (a nameless one after its
repository): Agent Plugins defines no distribution format, so a plugin's
own repository has to be something a user can add.

**Reserved names.** A catalog cannot pass for an official one: the names
Claude Code reserves for its publisher's marketplaces
(`claude-plugins-official` and the rest) come only from
`github.com/anthropics/`, the names Codex reserves (`openai-curated`, …)
only from `github.com/openai/`, compared without regard to case or
punctuation. Only an authenticated transport counts — `https://`,
`ssh://git@github.com/` or `git@github.com:` (never plain `http://` or
`git://`, which anyone on the way can answer) — and only a URL whose path
git fetches as written: a git URL with a `.`, `..` or empty path segment or
a `%`-escape in its path is refused outright, wherever it is typed or
listed, since libcurl collapses `/../` before it asks the server and
`github.com/anthropics/../evil/x` would read as one account and fetch from
another.

**Bounded, and printable.** A catalog is read in one pass that stays linear
however large it is — a duplicate is found through a hash of each
case-folded name, confirmed against the names already met only on a hit —
reading at most 5 000 entries (the rest summarized in one notice) and
keeping at most 100 notices (then one `… and N more`), since the page reads
catalogs on the UI loop. Every display string a catalog or a manifest gives
(a description, a display name, an author, a category, a homepage, a
version, keywords) and `git`'s own error text are cleaned where they are
decoded (`plugins::text`): C0 and C1 controls, DEL and the Unicode bidi
controls are dropped — a description's line breaks kept, a tab a space,
every other field's line break a space — so an OSC 52, OSC 8 or CSI sequence
in somebody's catalog never reaches the terminal through the CLI or the
page. `--json` output is serde's, escaped already.

| Read from the catalog | |
| --- | --- |
| top level | `name`, `plugins`; `interface.displayName`; `description` / `metadata.description`; `owner.name`; `metadata.pluginRoot` |
| each entry | `name`, `source`; `description`, `version`, `category`, `author`, `homepage`, `keywords` / `tags`, `displayName` / `interface.displayName`; `policy.installation`; the component keys of an entry that stands in for a manifest |

An entry that cannot be read is skipped with a notice and the rest of the
catalog still loads. An entry's `source`:

| Form | Supported |
| --- | --- |
| `"./plugins/x"`, a bare name under `metadata.pluginRoot`, `{"source":"local","path":…}` | yes — a folder of the marketplace |
| `{"source":"github","repo":"owner/repo","ref"?,"sha"?}` | yes |
| `{"source":"url","url":…,"path"?,"ref"?,"sha"?}` | yes (`path`, undocumented but used by four official entries, is a subfolder) |
| `{"source":"git-subdir","url":…,"path":…,"ref"?,"sha"?}` | yes |
| `npm`, `archive`, `command`, anything else | **no** — listed, marked unavailable with the reason; `command` would run a shell command at install and every launch |

`policy.installation: "NOT_AVAILABLE"` makes an entry unavailable too. A
`sha` wins over a `ref`, and the checkout must land on it. A pinned `sha`
must be the **whole** 40-digit commit id (as every official entry's is): a
prefix is something the plugin repository's owner could make a second
commit match, so an entry pinned to one is listed unavailable with the
reason. A catalog's
`ref`, `sha`, URL and subfolder reach `git` as arguments, so each is read as
what it must be — a branch or tag name, a commit id, a URL, a relative path
— and an entry whose value could be read as an option (`--upload-pack=…`)
is skipped with the reason: nothing a catalog says runs anything.

**Adding one** takes `owner/repo`, an `https://` / `ssh://` / `git@host:` /
`file://` URL or a local folder (`/abs`, `./rel`, `../rel`, `~/…`), with an
optional `@ref` / `#ref` or `--ref` on a git source. A local folder is read
**in place**. The parse is pure (`plugins::source`) and runs before anything
is fetched, so a typo is an error at once rather than a failed clone.

**Two defaults are offered, not fetched**:

| Name | Source |
| --- | --- |
| `claude-plugins-official` | `anthropics/claude-plugins-official` |
| `openai-curated` | `openai/plugins` |

They are one Enter away on the page's `+ Add` tab and one flag away in the
shell (`plugin marketplace add --defaults`); installing
`plugin@claude-plugins-official` adds that marketplace on the way.
`ALTER_ZERO_PLUGIN_MARKETPLACES` replaces the offered list for a run (the
smoke suite points it at local repositories): comma-separated sources, an
item optionally `name=source` so an install naming that marketplace can add
it on the way.

### On disk

Everything is under the config home (`ALTER_ZERO_CONFIG_DIR`, else
`~/.alter-zero`; `ALTER_ZERO_PLUGINS_DIR` moves the plugins tree and
`plugins.json` with it):

```
plugins.json                               the registry
plugins/marketplaces/{name}/               a git marketplace's sparse checkout
plugins/installed/{marketplace}/{plugin}/  an installed plugin — PLUGIN_ROOT
plugins/data/{marketplace}/{plugin}/       its data directory — PLUGIN_DATA
plugins/staging/                           clones and copies in flight
```

A plugin of a marketplace **kept in a directory** is not copied: it loads
in place from its folder there, so an author's edit is what the next
session (or the next `/plugins` reload) reads, with no update to run. Its
record names that folder (`"root"`), and an uninstall forgets it without
touching it.

An installed plugin has **one path for as long as it is installed**, not a
directory per version: a version in the path would make a skill loaded into
a conversation a different directory after an update. The version is a fact
about the copy and lives in the registry.

```json
{
  "marketplaces": {
    "claude-plugins-official": {
      "source": "anthropics/claude-plugins-official",
      "revision": "1a2b3c4d…",
      "updated": 1795180000
    },
    "team": { "source": "/home/me/team-plugins" }
  },
  "plugins": {
    "hookify@claude-plugins-official": {
      "version": "0.1.1",
      "revision": "9c1e0b4…",
      "offered": "0.1.1",
      "installed": 1795180100
    },
    "lint@team": { "root": "/home/me/team-plugins/lint" }
  },
  "projects": {
    "/home/me/work/api": { "disabled": ["hookify@claude-plugins-official"] }
  }
}
```

A plugin is installed for the user and **on in every directory until one
turns it off** — `skills.json`'s shape and reason
(`docs/per-directory-state.md`): which plugins earn their context is a fact
about a project. Turning one off everywhere is uninstalling it.

`plugins.json` follows the house rules for a state file: a pure format
(`plugins::state`), read-modify-write over the file as it is on disk at
that moment, written through a temp file and a rename, unknown keys kept,
and a file that will not parse is **never written over** — the session runs
with no plugins and says so. `revision` is what the copy was made from: the
commit of an external source, the **tree** of a plugin kept in its
marketplace (`git rev-parse HEAD:<path>`). A marketplace's `source` is what
was typed, normalised so it parses back to itself (`owner/repo`, the git
URL, the absolute folder), with a git source's `ref` beside it; `offered` is
the version the catalog entry offered at install; `root` the folder a
plugin loads from in place. A marketplace key that is no plain name is kept
as written but never followed as a path.

### Fetching — `git` as a child process

Nothing is linked in: `git` on `PATH` is run as a child (`plugins::git`),
which costs this process no memory and no binary size. Every invocation is
hardened the same way — no terminal to prompt on (`GIT_TERMINAL_PROMPT=0`,
askpass refused, stdin closed, the tty-detach chain of
`docs/tty-detach.md`; a `GIT_SSH_COMMAND` of the user's own is kept with
`-oBatchMode=yes` after it, and none is set where they have none, since
that would override their `core.sshCommand`), the caller's repository out
of the picture
(`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and their kin removed,
`-c safe.bareRepository=explicit`, `-c core.fsmonitor=false`), the `ext`
transport held out (`GIT_ALLOW_PROTOCOL`), a user's global git-lfs smudge
filter kept off a fetched repository's `.gitattributes`
(`GIT_LFS_SKIP_SMUDGE=1`), output captured into a bounded
buffer whose tail is the error message, a timeout, and the operation's
`CancelToken` polled every 20 ms so Esc kills the child. No submodules are
fetched.

- **A marketplace** is a blobless, sparse, depth-1 clone —
  `git clone --depth 1 --filter=blob:none --sparse [--branch <ref>]` into
  `plugins/staging/`, then `sparse-checkout set` over the catalog
  directories — so adding `openai/plugins` downloads 1.4 MB rather than
  105 MB. The staged clone is validated (a catalog or a plugin manifest, a
  usable name) and renamed into place; a failed add leaves nothing behind.
  A server that refuses the filter still works (git falls back to a plain
  shallow clone).
- **Updating** one is `fetch --depth 1` of its ref and a detached checkout;
  the same revision back means *already up to date*.
- **A plugin kept in its marketplace** is materialised on demand:
  `sparse-checkout add <path>` in the marketplace clone fetches that
  folder's blobs, and the folder is copied. Before that, what it holds is
  already known without the network — a blobless clone has every tree, so
  `git ls-tree -r` lists a plugin's skills, commands, agents and whether it
  has hooks or MCP servers (the page's *Contents* for a plugin not yet
  installed).
- **A plugin in a repository of its own** is fetched into a staging
  repository at exactly the commit the marketplace names: `init`,
  `remote add`, a sparse-checkout pattern file naming `<subdir>` when there
  is one (written, never passed on a command line),
  `fetch --depth 1 origin <sha|ref|HEAD>`, `checkout FETCH_HEAD`, and a
  check that a pinned `sha` is what was checked out.

### Installing

1. Resolve `plugin@marketplace` in the marketplace's catalog (adding a
   default marketplace first if it is one and not yet added); refuse an
   unavailable entry with its reason.
2. Find the source: a folder of the marketplace, or a staging clone.
3. Classify and validate the plugin (*Three formats*); an Agent Plugins
   manifest that is fatal by the specification stops here. The name is
   validated before it becomes a path.
4. **Copy it** — unless its marketplace is kept in a directory, where it
   loads in place — into `plugins/staging/`, then rename it into
   `plugins/installed/{marketplace}/{plugin}/`, over the old copy on an
   update (moved aside first and put back if the rename fails). Regular
   files and directories only; `.git` is left behind; a symlink that
   resolves inside the plugin is copied as what it points at, one that
   leaves it is skipped and noted; the copy stops at 20 000 entries or
   256 MiB. When the entry declares components or the plugin has no
   manifest of its own, the entry is saved beside the copy as
   `.alter-zero-entry.json`.
5. Record it in `plugins.json` with its version and revision.
6. **Forget every trust record under the plugin's directory**
   (`trust.json`'s hooks and MCP entries are keyed by file path), so an
   install or an update is always reviewed again. The records are keyed by
   the plugin's **resolved** root (it is loaded through `canonicalize`),
   so the directory is forgotten under its spelling as configured and as
   resolved through the filesystem (`plugins::trust_forms` — a symlink in
   the config home's path, such as Silverblue's `/home -> /var/home`, or a
   relative `ALTER_ZERO_CONFIG_DIR`, would otherwise forget nothing), a
   directory an uninstall just removed resolved through its parent.
   **A failure forgets too**: an install that fails once its new copy is
   moved in, an update of several plugins that fails part way, an
   uninstall or a marketplace removal that fails, and a `/plugins` worker
   that dies all hand back every directory whose files may have changed
   (`PluginStore::install_tracked` / `update_tracked`,
   `page::may_replace`) — over-forgetting being the safe direction.

An **update** is the same steps; the data directory is never touched. What
"has an update" means depends on the source: a plugin kept in its
marketplace by its files (the tree), one pinned to a commit by the commit,
one from a repository of its own by the entry's version against the version
the entry **offered at install** (never the manifest's, which may say
another and would read as an update forever) — or, when neither side names
a version, by the remote's head commit (`git ls-remote`); one loaded in
place is always current.

Uninstalling removes the copy, the data directory, the record and the trust
records. **Removing a marketplace uninstalls the plugins installed from
it** — the confirmation names them.

### Loading into a session

Startup reads `plugins.json` and nothing else: no directory is walked, no
marketplace is opened, no catalog is parsed. With no plugins the cost is one
`read` that fails. With some, each plugin that is on in this directory is
classified from its own small manifest and contributes paths to the walks
that already run — its `skills/` and `commands/` to the skill discovery, its
`agents/` to the agent discovery, its hook files to the trust layers, its
MCP entries to the manager's sources — so what stays resident is what those
features already keep per item (a skill's name and description, an agent's
definition), and nothing for a plugin that is off.

The set is taken again whenever this session changes it (an install, an
update, an uninstall, a toggle — from `/plugins`) and when `/plugins` is
opened (another process may have installed something) — and checked at
every turn start, beside the skills rescan, for one cheap reason: another
process (`alter-zero plugin update`, a second session's `/plugins`) forgets
a reinstalled plugin's trust in `trust.json` only, and this session's copy
of the store would otherwise run the new files under the old approval
until a restart. `plugins.json` records each plugin's install stamp and
revision; the startup read carries them (`EnabledPlugin::installed`,
`::revision`), and a plugin whose stamp, revision or version moved since
the session loaded it is reinstalled (`plugins::reinstalled`): its
directory is forgotten in the session too, so its hooks and servers go back
to waiting for the review. A plugin the session loads that it did not load
before takes what `trust.json` says about it **now** rather than the
session's older copy. With nothing changed the check is one read of
`plugins.json`, one failed `open` with none. Skills and agents
follow at once; hooks are rebound through `set_hooks_file`; MCP servers are
swapped in the running manager; and when the change brought hooks or
servers that are not yet trusted, the review opens on them
(`ConfigReview::requested`) with the commands on screen before anything of
the plugin has run. `ALTER_ZERO_PLUGINS=0` loads no plugin for a run;
`/plugins` still opens and says so.

In the code (`src/tui/plugin_runtime.rs`): bootstrap resolves the layout,
calls `startup_plugins` and stops there when it lists nothing that is on
here — no conversion, no setter, not even the `ALTER_ZERO_PLUGINS` read,
which is made only once a plugin would load. Otherwise one `#[cold]`
function runs `load_enabled` (which creates each plugin's data directory
first) and converts each contribution: skill directories and commands into
`PluginSkillSource`s, agent directories into `PluginAgentSource`s (whose
walk already reads `tools:` in Claude Code's vocabulary and ignores
`model:`), each hooks source into a `PluginHooks` layer stamped with the
plugin's root and data, and the servers into one `PluginMcpFile` per
declaring file — each problem a startup toast. The skill and agent sources
are kept on the session for the per-turn rescans, which read no plugin file
of their own. `Session::reload_plugins(forget: &[PathBuf])` is what the
page's boundary calls: one `plugins.json` read when nothing changed and
nothing is to be forgotten; otherwise the reload, the two rescans, and
`set_plugin_contributions(hooks, mcp, forget)`.

### Nothing runs unseen

A plugin is somebody else's code arriving over the network, so the rule the
rest of the app keeps — no hook and no project MCP server runs until the
user has seen it — is kept without a second mechanism:

- Hooks and servers are **off until reviewed**, on the same two pages,
  each section named for its plugin (`Plugin hello@demo · …`), a servers
  page made of plugins' servers saying they were found in your plugins.
- **Every update is reviewed again**: the install forgets the trust records
  under the plugin's directory, since a remote update can change a script
  under an unchanged command line. (A plugin loaded in place is the user's
  own folder: a changed command is asked again, as a project's is.)
- A `command` plugin source, `allowed-tools`, `bin/` on the `PATH`,
  monitors and in-process mods are **never run**.
- `${VAR}` expansion cannot carry a provider key into a plugin's remote
  server (the project-scope protection), and every other variable a
  plugin's server reads is **named on the review**: its row says `reads
  from your environment: GITHUB_TOKEN, EXA_API_KEY` (the names in its URL,
  headers, args, env values and `oauth` fields, the plugin's own four path
  variables aside), since a header such as `"X-Debug": "${GITHUB_TOKEN}"`
  sends that value to the server's host and the review shows header names,
  never values. `${VAR}` in a header is also how a plugin's server gets its
  own API key, so it is shown, not refused.
- **A reinstall elsewhere is noticed**: a plugin updated by another
  process goes back to the review at this session's next turn (*Loading
  into a session*), never running its new files under the old approval.
- **What a plugin says is printed clean**: no control or bidi character of
  a catalog's, a manifest's or `git`'s reaches the terminal (*Marketplaces*).
- A plugin's hooks file, and a manifest's `mcpServers` file, are read
  bounded and only when they are regular files — a plugin loaded in place
  can hold a FIFO, which an `open` would wait on forever.
- Skills, commands and agents are inert text: what they ask the model to do
  still meets the permission gate.
- Installing from the shell asks nothing: the hooks and servers wait for
  the next launch's review, and the command says so.

### Memory

Resident memory is a feature here (`docs/memory.md`), and the idle number is
the one this design is held to: **a session with no plugins costs what it
cost before**, and one with plugins costs what loading them takes and no
more. Holding it turned out to be mostly not about data, so this section is
the record of what was assumed, what was measured, and what holds it now.

- **Startup**: one bounded read of `plugins.json`, typed, no `Value` tree.
  No catalog, no thread, no `git`. The entry point is one small function,
  `plugins::startup_plugins`: with no `plugins.json` it returns after one
  failed `open`, having touched no other plugin code (no store, no parser,
  no directory probe). Classifying a root and reading manifests
  (`plugins::load_enabled`) is reached only when the registry lists a
  plugin that is on in this directory. An earlier version of this feature
  read every marketplace's catalog behind the first frame and kept it so
  the page could open on it; this one does not.
- **A catalog** is read only when `/plugins` opens (or a CLI command runs):
  streamed off the file through `serde_json::from_reader` over a
  `Read::take` cap (8 MiB) into typed records naming only the fields the
  page shows — never the 192 KB body in one buffer, never a tree of it —
  with descriptions cut at 2 000 characters — and, for the page, **one
  entry at a time** (`stream_catalog`), each packed into the snapshot's one
  text buffer and one row table and dropped (`docs/plugins-menu.md` *The
  snapshot's shape*). It is read on the loop (two catalogs are 4–7 ms), so
  the page's first frame already has its rows and there is no placeholder
  for them to push away; it is dropped when the page closes. Opening the
  page on the two official catalogs' 382 plugins and closing it leaves
  about 290 kB of freed heap behind, once (`/settings` leaves 160 kB), where
  the first version of the page left 650 kB and more with every cycle.
- **Fetching** is a child process; its output is captured into a few
  kilobytes.
- **The worker thread** that waits on `git` builds nothing large (a new
  thread is a new arena): results cross back as small typed values, and the
  snapshot after an operation is read on the loop.
- No new dependency, no static table of references.

**Where the code sits is what decided the idle number.** The earlier
version idled 1.4–1.6 MB above the baseline with no plugin installed, and
switching every one of its startup calls off at run time gave none of that
back: its code was resident without running. So the core became a crate of
its own, `crates/plugins` (`alter-zero-plugins`, re-exported as
`alter_zero::plugins`), on the observation that cold dependency crates —
`rustls`, `ring`, `x11rb` — sat in one block each and were not resident at
idle. That was the wrong lesson. A 2.8 MB block of code nothing calls cost
an idle session 0.8 MB whether it was linked into the main crate or kept in
a crate of its own, and this crate as first merged, none of it run, moved
the idle figure by 0.8 MB in `release` and 1.5 MB in `dist`. What an idle
session pays for code is set by where the linker puts the megabyte it
*does* run, and adding any code anywhere shifts that.

What holds the number is therefore not in this crate at all: the **idle
text layout** (`docs/text-layout.md`) has the linker put the functions a
session runs at rest side by side, with the ones a session with plugins
adds — the registry read, the manifests, the skill and agent walks, the
hook layers; about 280 kB of code — as a group of their own after them.
A session without a plugin keeps none of the plugin system resident, and
one with a plugin keeps that group:

| The shipped binary at rest, MB | Just written | Read from disk |
|---|---|---|
| a configured session, before the plugin system (`main`) | 13.07 | 14.48 |
| a configured session, no plugin | 8.04 | 8.90 |
| the same with a plugin on (a skill, a command, an agent, a trusted hook) | 8.37 | 8.89 |
| that plugin session under the linker's default order | 14.15 | 16.40 |

The crate stays for what it is good for: it depends on nothing of the
app's (cancellation is its own `CancelToken`, an MCP server its own
`ServerTransport`, `git` is spawned through an injected `CommandBuilder`
the app passes over its tty-detach tiers), what it returns is paths and
plain data, and the surface the main crate touches is concrete,
non-generic functions. In the shipped binary the whole plugin system —
three formats, the catalog, the store, the commands, the page — is about
0.6 MB of code of its own and 1.4 MB of file (18.99 → 20.37 MB).

`scripts/smoke` holds what a test can hold: an idle session with an empty
config home and one with marketplaces added and nothing installed are the
same size to the page (Phase 162), and `scripts/text_layout.sh measure
--plugins` is the measurement for the rest.

## The `/plugins` page

`docs/plugins-menu.md` is the full description; the shape is Codex's — a tab
per marketplace — drawn in this app's picker frame:

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

- **Tabs**: `All`, `Installed`, one per added marketplace and `+ Add`, the
  current one a lit chip, each with its count, over that tab's own subtitle
  and counts. A marketplace's tab says where it comes from and how old its
  catalog is; **Ctrl+U** updates it and **Ctrl+R** removes it (confirmed,
  naming what goes with it). `+ Add` offers the two official catalogs, takes
  a typed source on its `❯` line, and lists `Add a marketplace…`, a field
  for any source.
- **One status vocabulary**: `✔ enabled`, `◯ disabled`, `✘ not loaded`,
  `+ available`, `– unavailable`, and an amber `▲ update ·` opening the
  description. On and off are **for the directory you are in** (Space).
- **Enter** opens a plugin: its state first, where it comes from, what it
  adds by the names a session uses, a warning before installing one that
  may run code, and the actions its state affords, the primary one first.
- **The page never moves under you.** It says nothing about what went right
  — the rows show it; a job is said in the hint's own row, and only once it
  has run 400 ms; a failure is said there in red; no toast is raised over
  the open page; neither a toggle nor moving through the list resizes it.
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
alter-zero plugin info <plugin>[@<marketplace>] [--json]
alter-zero plugin marketplace add <source> [--ref <ref>]
alter-zero plugin marketplace add --defaults
alter-zero plugin marketplace list [--json]
alter-zero plugin marketplace update [<name>]
alter-zero plugin marketplace remove <name>
```

Both references' spellings work: `add` and `remove` are `install` and
`uninstall`, `upgrade` is `update`, `rm` is `remove`. Resolved and answered
before the TUI boots, like `mcp` (`docs/mcp-cli.md`): results on stdout, one
plain failure line on stderr, exit `2` for a grammar error, `1` for a
failure, `0` otherwise. A bare plugin name is looked up in every added
marketplace and must be unambiguous. `enable` and `disable` are for the
directory the command runs in. An install that brought hooks or MCP servers
says they will be reviewed at the next launch.

## Environment

| Variable | |
| --- | --- |
| `ALTER_ZERO_PLUGINS` | `0` loads no plugin for the run (the page and the CLI still work) |
| `ALTER_ZERO_PLUGINS_DIR` | moves the `plugins/` tree and `plugins.json` with it |
| `ALTER_ZERO_PLUGIN_MARKETPLACES` | replaces the offered marketplaces: sources separated by commas; empty offers none |
| `ALTER_ZERO_PLUGIN_GIT_TIMEOUT_SECS` | the bound on one network `git` command (default 300) |

The state lives in the config home, so `ALTER_ZERO_CONFIG_DIR` relocates all
of it — which is what makes every smoke phase start with no plugins.

## Known divergences

- **Not every source form.** `npm`, `archive` and `command` sources are
  listed as unavailable. One official plugin (`xcode`) is a `command`
  source.
- **User installs only.** A plugin is installed for the user and turned off
  per directory; there is no project-scoped install, and a repository
  cannot enable one.
- **No automatic updates.** Nothing is fetched unless asked; the page shows
  how old a catalog is.
- **No custom slash commands.** A plugin command is a skill; the palette
  stays the app's own.
- **`renames`, `strict`, dependencies, `engines`** in a marketplace are
  read past. An entry's components are appended to the plugin's own.
- **A root `SKILL.md`** (a plugin that is one skill) and skills nested
  deeper than `skills/<name>/` are not discovered.
- **`.app.json` apps, LSP servers, output styles, themes, monitors,
  workflows, `bin/`, `userConfig`, channels and mods** are not used, and are
  named as such.
- **Agent Plugins 1.1.0** (a draft that adds a portable `hooks.json`) is
  refused as an unsupported version until it is published and supported.
- **Codex's MCP extras** (`oauth_resource`, `scopes`, `env_vars`, the
  timeouts) are ignored; a stdio child already inherits the environment.
- **Stricter than the references** on what a catalog may say: a `sha`
  prefix is unavailable rather than trusted, a URL with a dot segment or a
  `%`-escape in its path is refused, a reserved name needs `https`/`ssh`,
  names differing only by case are one name, and a catalog is read up to
  5 000 entries.
- **A reinstall mid-turn** by another process is noticed at the next turn
  start, not by a hook already running in this one.

## Module layout

| Where | What |
| --- | --- |
| `crates/plugins/` | the plugin core, the crate `alter-zero-plugins` (re-exported as `alter_zero::plugins`): depends on nothing of the app's |
| `crates/plugins/src/cancel.rs` | the cancellation flag a long operation polls (the app bridges its turn token to it) |
| `crates/plugins/src/mcp.rs` | a plugin server's transport as plain data (`ServerTransport`), the legacy `.mcp.json` entry reader; the app converts to `mcp::McpServerConfig` |
| `crates/plugins/src/manifest.rs` | classify a plugin root; the legacy manifest; display metadata |
| `crates/plugins/src/agent_plugins.rs` | Agent Plugins 1.0.0: the closed manifest, the name grammar, `mcp.json` |
| `crates/plugins/src/marketplace.rs` | the catalog, streamed into typed records; reserved names |
| `crates/plugins/src/source.rs` | what the user types to add a marketplace |
| `crates/plugins/src/state.rs` | `plugins.json`, ids, the offered marketplaces |
| `crates/plugins/src/layout.rs` | the paths under the plugins home |
| `crates/plugins/src/components.rs` | what an installed plugin contributes (skills, commands as skills, agents, hooks, MCP servers by Claude Code's names), its summary, and the `git ls-tree` preview |
| `crates/plugins/src/git.rs` | the hardened, bounded, cancellable `git` runner |
| `crates/plugins/src/store.rs` | add / update / remove a marketplace; install / update / uninstall / toggle; `startup_plugins` (reads `plugins.json` alone) and `load_enabled` (each plugin's own files) |
| `crates/plugins/src/page/` | the page's state, keys and view (`docs/plugins-menu.md`) |
| `src/app/plugins_menu.rs` | the page's shim in `App`: open, close, forward |
| `src/ui/plugins_view.rs` | the page's one line builder |
| `crates/plugins/src/page/host.rs` | the page's work over the store: the snapshot, a plugin's contents, every operation (`docs/plugins-menu.md` *The boundary*) |
| `src/tui/plugins.rs` | the page's boundary: the store while open, a thread per operation, Esc's cancel, the reload after a change |
| `src/tui/plugin_runtime.rs` | plugins on in this directory → skills, agents, hooks, MCP; reload |
| `src/tui/plugin_cli.rs` | the `plugin` command family |
| `src/cli.rs` | its grammar and help pages |

## Testing

- `cargo test -p alter-zero-plugins` — the formats (every manifest rule of the
  specification has a test named for its section), the catalog reader
  against both real manifests' shapes, the source grammar, the state file,
  the path math.
- `cargo test -p alter-zero-plugins --test store` — the store against local `git`
  repositories: add, update, install each source form, a pinned `sha`
  mismatch, a cancelled clone, an uninstall; no network.
- `cargo test --test plugin_cli` — the real binary over a temp config home.
- `cargo test --lib -- app::tests::plugins_menu ui::tests::plugins_view` —
  the page: keys, the height ladder, the frame hug, the cursor seat.
- `scripts/smoke.sh 162 163` — the page and the runtime against a local
  marketplace, and the idle-memory comparison.
