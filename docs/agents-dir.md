# The `.agents/` directory

`.agents/` is the agent-neutral twin of `.alter-zero/` and `.claude/`: a
directory any coding agent may read, which Codex reads for two things. Codex
reads **skills** from `.agents/skills` and **plugin marketplaces** from
`.agents/plugins/marketplace.json`. Alter Zero reads skills from the same
places, so a repository or a home directory that keeps its skills for Codex
works here unchanged. An added marketplace's catalog has always been read from
`.agents/plugins/marketplace.json` first (`docs/plugins.md`).

This page states what is read and why each choice was made. The mechanisms are
in `docs/skills.md` (the skill roots) and `docs/subagents.md` (the agent
roots); this is the cross-reference for "does Alter Zero read `.agents/X`?".

The Codex behaviour cited below is from `github.com/openai/codex` at commit
`8653ca9` (`codex-rs/ext/skills/src/host_roots.rs`).

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
