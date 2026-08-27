# Subagent definitions — `agents/*.md`

Claude Code's agent files, ported: the subagent types the `agent` tool can
launch are **markdown files on disk** — YAML frontmatter over an optional
system-prompt body — instead of two names hard-coded in a `match`. The
built-in `general-purpose` and `explore` agents are seeded into
`~/.alter-zero/agents/` on first run, so the defaults are editable and a new
type is one new file.

`docs/agent-tool.md` is the tool itself (the launch, the roster, the session
view); this is only where a type's **definition** comes from and what it
changes about the run.

```
~/.alter-zero/agents/general-purpose.md   ← seeded, user-wide
~/.alter-zero/agents/explore.md           ← seeded, user-wide
{project}/.alter-zero/agents/reviewer.md  ← per project, wins over the above
```

## The file

```markdown
---
name: explore
description: Read-only search agent for broad fan-out searches …
model: inherit
tools: Bash, Read, Skill, mcp__*
---

You are Alter Zero's explore subagent. …
```

| field | | |
| --- | --- | --- |
| `name` | optional | the type the model passes as `subagent_type`. Defaults to the file stem, so `reviewer.md` needs no `name:` |
| `description` | **required** | the one-line pitch the model chooses from — it is the whole basis for picking a type, so a file without one is refused |
| `model` | optional | a provider model id (`kimi-k3`) run this type on, or `inherit` for the session's own model. The seeded files say `inherit` rather than omitting it: the key is then *there* to edit |
| `tools` | optional | comma-separated allowlist. **Omit for every tool.** Built-ins are Capitalized (`Bash`, `Read`, `Write`, `Edit`, `Skill`); MCP tools keep their wire spelling (`mcp__deepwiki__ask_question`), and a trailing `*` globs (`mcp__deepwiki__*`, `mcp__*`) |
| body | optional | **the subagent's system prompt**, replacing the `prompts/alter_zero.md` persona for this type. Omit it and the type runs the session's own persona |

Unknown keys are ignored, not rejected (the `SKILL.md` rule): an agent file
written for another tool — `color:`, `allowed-tools:`, `version:` — loads
here unchanged. A file that cannot be parsed raises a toast naming it and is
skipped; one bad file never costs a session the rest.

The frontmatter parse is the **shared** `crate::frontmatter` module — the
same `---` split, the same plain/quoted/block scalars and indented
continuations `SKILL.md` frontmatter gets, comments included. That is why the
seeded files can carry `#` comments documenting `tools:` and `model:`: a
comment line is skipped by the scanner, so the file is self-teaching for
whoever opens it (the model included) without costing the listing anything.

## Roots

In precedence order — the **first** root to claim a name wins, so a project
can shadow a personal agent of the same name:

1. `{cwd}/.alter-zero/agents`
2. `{project_root}/.alter-zero/agents` — the nearest `.git` ancestor, skipped
   when it *is* the cwd (launching in `repo/src` still finds the repo's
   agents, the `AGENTS.md` walk-up)
3. `{config_home}/agents` — `~/.alter-zero/agents`, following
   `ALTER_ZERO_CONFIG_DIR` like every other user-level config

Deliberately **no `.claude/agents`**, unlike skills. A `SKILL.md` is inert
markdown; an agent file names a *model* and a *tool allowlist*, and silently
inheriting another tool's agents would change which model a task runs on and
what it may touch.

`ALTER_ZERO_AGENTS_DIR` **replaces** the whole list (the `*_DIR` convention),
which is also what makes a test run hermetic.

### Seeding, and the built-in fallback

The two defaults ship embedded in the binary (`agents/general-purpose.md`,
`agents/explore.md` in the repo, `include_str!`'d). At startup each is
**written into the user root when that file is absent** — so a fresh install
finds them on disk and can edit them, and a release that adds a default gets
it too. Deleting one brings it back next launch; to change a default, edit
it (that is what the seeding is *for*).

If the walk still ends without one of them — no writable home, an
`ALTER_ZERO_AGENTS_DIR` pointed at a read-only directory — the embedded
definition is appended as the lowest-precedence entry. The `agent` tool's own
schema names `general-purpose` as its default, so that type existing is a
property the tool depends on, not a convenience.

## What a definition changes about the run

`llm::backend::spawn_subagent_run` resolves the definition once, at launch:

- **tools** — `subagent_tool_specs(def)` filters `bash`/`read`/`write`/`edit`,
  the `skill` spec, and every connected MCP tool through
  `AgentTools::allows`. `agent` is never in the set, whatever the file says:
  agents don't nest. The same predicate guards the **executor**, not just the
  offered specs: a model can name a tool it was never given (some providers
  pass one through), and a `tools:` line that only shaped the request would be
  a promise the executor doesn't keep — a withheld call resolves as a
  recoverable error naming the tool and the type, so the agent picks another
  way in the same round. A type that excludes `Skill` loses the skills
  `<system-reminder>` with it, the tool-and-listing pairing `docs/skills.md`
  keeps everywhere.
- **model** — `AgentModel::Named(id)` swaps `ModelConfig::model` for this run
  (same provider, base, key and temperature) and clears the thinking mode and
  the vision flag, which describe the *session's* model and not this one.
  `inherit` (or an omitted key) changes nothing.
- **system prompt** — the body replaces the persona, keeping the environment
  and scratchpad blocks (`{body}` → environment → scratchpad → subagent note):
  those are runtime facts about *this* session — the date, the os, the cwd,
  where temporary files go — and a subagent that doesn't know them writes into
  `/tmp` and guesses the year. With no body the prompt is exactly what it was
  before this feature: the session's own assembled prompt plus the note.

An unknown `subagent_type` no longer silently becomes `general-purpose`: the
call resolves as a recoverable error naming the available types, so the model
corrects itself in the same round.

## The listing

The `<system-reminder>` the derived context leads with (`docs/context.md`)
gains a second section, so the model reads the roster of types beside the
roster of skills:

```
<system-reminder>
The following skills are available for use with the Skill tool:

- terminal-mascot: Design small mascots …

Available agent types for the Agent tool:

- general-purpose: General-purpose agent for researching complex questions … (Tools: *)
- explore: Read-only search agent … (Tools: Bash, Read, Skill, mcp__*)
</system-reminder>
```

One reminder, two sections, either of which may be absent — the wrapper is
`subagents::reminder_message`, the sections are `skills::skill_listing` and
`subagents::agent_listing`, and both are budgeted the same way (the model's
context window × 1%, descriptions trimmed to an even share before any entry
is dropped). `(Tools: …)` is the allowlist as the file wrote it, or `*` when
it was omitted — a type's reach is the other half of choosing it.

The agent section rides exactly when the `agent` tool does (a real backend
with tools on): a reminder naming a tool the request never carries is the
listing/tool mismatch `skills_offered` exists to prevent. The offline dummy
scripts its agent demo rather than being offered a spec, so it sends no agent
section and every offline context is byte-identical to before.

A **subagent** gets the skills half only (`subagent_skill_reminder`): it has
no `agent` tool, so naming types to it would be a roster it cannot use.

## Rescanning

The roots are re-walked at **every turn start**, beside the `SKILL.md`
rescan: an agent file you just added — or one the agent wrote *for* you — is
live on the next turn instead of the next restart. The registry is a shared
handle, so a replaced set is already what the launcher reads; the listing is
re-rendered in the same pass, and a file that stopped parsing reports once
per path (`unreported_errors`, re-seeded each pass so a file fixed and
re-broken reports again).

## Layout

| | |
| --- | --- |
| `src/frontmatter.rs` | the shared `---` frontmatter parse (skills + agents) |
| `src/subagents.rs` | pure: `AgentDefinition`, the parse, `AgentTools::allows`, the listing, the reminder wrapper, `SubagentRegistry` |
| `src/llm/subagent.rs` | boundary: the root walk, the discovery, the seeding of the embedded defaults |
| `agents/*.md` | the embedded defaults themselves |

`ALTER_ZERO_AGENTS_DIR` relocates the roots. There is no separate on/off
switch: the types *are* the `agent` tool, which the `/settings` **Tools** row
already governs.
