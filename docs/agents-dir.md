# The `.agents/` directory

`.agents/` is the agent-neutral twin of `.alter-zero/` and `.claude/`: a
directory any coding agent may read, which Codex reads for two things. Codex
reads **skills** from `.agents/skills` and **plugin marketplaces** from
`.agents/plugins/marketplace.json`. Alter Zero reads both, from the same places, so a
repository or a home directory arranged for Codex works here unchanged.

This page states what is read and why each choice was made. The mechanisms are
in `docs/skills.md` (the skill roots), `docs/subagents.md` (the agent roots)
and `docs/plugins.md` (found marketplaces); this is the cross-reference for
"does Alter Zero read `.agents/X`?".

The Codex behaviour cited below is from `github.com/openai/codex` at commit
`8653ca9` (`codex-rs/ext/skills/src/host_roots.rs`,
`codex-rs/core-plugins/src/marketplace.rs`).

## Skills: `.agents/skills`

Codex reads skills from `~/.agents/skills` (its user scope) and from
`<dir>/.agents/skills` for **every directory from the project root down to the
working directory** (its repo scope). The project root is the nearest ancestor
holding a `.git` marker, or the working directory itself when none does, so a
directory outside any repository contributes only its own (`host_roots.rs`,
`repo_agents_skill_roots` and `dirs_between_project_root_and_cwd`). Neither
root is gated on trust.

Alter Zero reads the same two places, and the same walk now governs every
per-directory root:

| # | Root | Scope |
|---|------|-------|
| 1 | `<dir>/.alter-zero/skills` | each directory from the cwd up to the project root, nearest first |
| 2 | `<dir>/.agents/skills` | the same directories, in step with row 1 |
| 3 | `<dir>/.claude/skills` | the same directories, in step with row 1 |
| 4 | `{config_home}/skills` | personal (`~/.alter-zero/skills`) |
| 5 | `~/.agents/skills` | personal, shared with Codex |
| 6 | `~/.claude/skills` | personal, shared with Claude Code |

Rows 1–3 repeat per directory: the cwd's three, then its parent's three, up
to and including the project root's. The first root to claim a name wins, as
it always has: a nearer directory shadows a farther one, and a project
shadows a personal skill.

- **`.agents` sits between `.alter-zero` and `.claude`.** Alter Zero's own
  directory comes first, as it always has. `.agents/` is the directory meant
  to be shared by every agent, so it outranks another product's own
  directory.
- **Every directory up to the project root.** This is Codex's rule for
  `.agents/skills`, and the walk `AGENTS.md` discovery already follows
  (`project_doc::doc_chain`). It replaced reading only the cwd and the project
  root. A monorepo package's `packages/web/.agents/skills` used to be found
  only when the session started in `packages/web` itself; now it is found
  from `packages/web/src` too. Nothing changes for a session started at the
  project root or one level below it. `.alter-zero/skills` and
  `.claude/skills` follow the same walk, so the table has one rule rather
  than one per directory name. Agent definitions (`docs/subagents.md`) follow
  it too, which is the walk Codex gives its own `.codex/agents`.
- **One path, one row.** Started in the home directory, the cwd's three rows
  *are* the personal three. The list keeps the first occurrence of each path,
  so `/skills` names each directory once. Codex deduplicates its roots the
  same way.
- **No trust gate.** A `SKILL.md` is inert until the model loads it, and any
  command in its body still meets the permission gate (`docs/skills.md`
  *Roots*). This matches Codex, which reads repo skills in projects it does
  not trust.
- **`ALTER_ZERO_SKILLS_DIR` still replaces the whole list**, the new rows
  included. That is what keeps `smoke.sh` hermetic. The suite also points
  `HOME` at an empty directory, so `~/.agents/skills` is empty there.

### What is not ported

- **The recursive walk.** Codex looks for `SKILL.md` up to six directories
  below a root and skips hidden directories on the way. Alter Zero reads
  `<root>/<name>/SKILL.md` only. That is the layout `docs/skills.md` and
  `skill-creator` teach, and it keeps the per-turn rescan to one `read_dir`
  per root rather than a tree walk. Codex instead caches its listing and
  watches the roots for changes. A skill nested deeper
  (`.agents/skills/team/review/SKILL.md`) is not listed.
- **Listing every same-named skill.** Codex lists all of them and sorts by
  scope; a `$name` mention then needs an unambiguous name. Alter Zero keeps one
  skill per name, the first root's, which is what makes a project's skill
  shadow a personal one.
- **`agents/openai.yaml`.** It is Codex's per-skill interface and dependency
  metadata. A skill carrying one loads here unchanged; the file is ignored.
- **Codex's `[[skills.config]]`.** Turning a skill off is `/skills`, which
  persists per project in `skills.json`.
- **The `.claude/skills` import.** Codex copies `.claude/skills` into
  `.agents/skills` once, on the user's say-so, rewriting the bodies. Alter
  Zero reads `.claude/skills` where it is, so there is nothing to import.

## Plugin marketplaces: `.agents/plugins/marketplace.json`

A marketplace's catalog has always been read from
`.agents/plugins/marketplace.json` first, then `.claude-plugin/marketplace.json`
(`MARKETPLACE_MANIFESTS`). What is new is that **a marketplace no one added is
found**. Codex finds two (`marketplace.rs`, `discover_marketplace_paths_from_roots`):

- **Home**: `~/.agents/plugins/marketplace.json`. This is where Codex keeps the
  personal marketplace its plugin sharing writes into.
- **The project**: the working directory's catalog, or, when it has none, the
  catalog at its project root. Directories in between are not read.

Alter Zero finds the same two, with the same catalog locations it reads in an
added marketplace (`plugins::find_marketplaces`). A found marketplace is
**listed, never installed**. It appears in `/plugins` as a tab of its own after
the added ones, its header saying `found in this project, not added` over its
directory, and in `alter-zero plugin list --available` and
`plugin marketplace list`. Its hint offers no Ctrl+R, since there is nothing
to remove, and Ctrl+R says so. Codex does the same; nothing it finds is
installed either.

- **Installing from one adds it.** A plugin of a marketplace kept in a
  directory loads in place (`docs/plugins.md`), so the marketplace must stay
  known for the plugin to keep loading. The install adds the found
  directory first, exactly as `alter-zero plugin marketplace add <dir>` would
  (`PluginStore::install_found`), and from then on it is an added marketplace
  in every directory, removable with Ctrl+R. An install that fails takes the
  marketplace back off. Codex copies the plugin into its cache instead and
  records nothing about the marketplace, but Codex has no in-place install to
  keep working.
- **Added marketplaces win their names, then home, then the project.** Plugin
  IDs are `plugin@marketplace`, so two marketplaces of one name cannot both be
  listed. The user's explicit choices come first, then their home, then the
  checkout. A checkout's catalog can therefore never take over the name of a
  marketplace the user added or keeps at home, and so never offers its own
  copy as an update to a plugin installed from there. Codex lists home before
  the project too.
- **A found marketplace that cannot be used is still shown.** A catalog that
  will not parse, a name that is invalid or reserved, or a name already taken
  is listed under where it was found (`this project`, `home folder`) with the
  reason in its header. Its plugins are not offered. Those labels hold a
  space, which no marketplace name can, so they never collide with a real
  tab. A marketplace that silently fails to appear is the one question the
  page could not otherwise answer.
- **A plugin directory is not a found marketplace.** An added directory with
  no catalog is read as a marketplace of the one plugin it is. A found one is
  not: a repository that is itself a plugin is not offered to itself.
- **Read only while the page is open.** The catalogs are read when `/plugins`
  opens and as a job lands, and by the `plugin` subcommands that list or
  install, never at startup (`docs/plugins.md` *Memory*). Finding them costs a
  few `stat`s and one catalog read each.
- **No trust gate.** A found marketplace only lists what it offers. Installing
  is the user's own act, and whatever an installed plugin runs (hooks, MCP
  servers) waits for the startup review like any other plugin's. Codex
  doesn't gate this on trust either.

### What is not ported

- **`api_marketplace.json`** is Codex's curated catalog for API-key accounts,
  kept under its own home. **`.cursor-plugin/marketplace.json`** is Cursor's.
  Alter Zero reads neither, found or added.
- **Writing the personal marketplace.** Codex's shared-plugin checkout writes
  into `~/.agents/plugins/marketplace.json`. Alter Zero only reads it.

## `.agents` as protected metadata: not ported

Under its workspace-write sandbox Codex keeps an existing `.agents/`
read-only, like `.git/` and `.codex/` (`codex-rs/protocol/src/permissions.rs`,
`PROTECTED_METADATA_PATH_NAMES`). The likely reason is to stop the agent
rewriting the skills it will read next. Alter Zero has no such sandbox. Its
`write`, `edit` and `bash` calls meet the permission gate instead
(`docs/permissions.md`), and `skill-creator` writes skills into a skill
directory on purpose. A protected-path rule would belong to the gate, and it
would cover `.alter-zero/` as much as `.agents/`. It is left for a design of
its own.
