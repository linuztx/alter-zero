# The `Skill` tool

Claude Code's skills, ported: folders of authored markdown the model can pull
into the conversation on demand. A skill is a *prompt* — domain knowledge,
a checklist, a workflow — that costs nothing until it is needed, because only
its one-line description sits in the context until the model asks for the body.

```
● Skill(dataviz)
  ⎿  Successfully loaded skill
```

That cell is the whole visible surface. What the *model* reads is the skill's
entire `SKILL.md` body; what the *user* sees is one green line saying it
loaded. The split is the point — a 400-line skill would otherwise dump itself
into the transcript every time it is used.

## On disk

A skill is a **directory** holding a `SKILL.md`, exactly the reference's
layout (`loadSkillsDir.ts` supports the directory form only, and so do we):

```
<root>/<skill-name>/SKILL.md
```

`SKILL.md` is YAML frontmatter over a markdown body:

```markdown
---
name: dataviz
description: Use when creating any chart, graph, plot or dashboard. Covers
  palette choice, axis and legend rules, and the stat-tile layout.
---

# Data visualization

Read `references/palette.md` before choosing colors…
```

`description` is required — it is the only thing the model sees before
invoking, so a skill without one could never be chosen (both references treat
a missing description as a load error, and so do we). `name` is optional and
defaults to the directory name; when present it must match `^[a-z0-9]+(-[a-z0-9]+)*$`
and be ≤ 64 characters, so a name is always safe to print, to match against a
tool argument, and to type after a `/`.

Anything else in the frontmatter is **ignored, not rejected** — a skill
authored for Claude Code carrying `allowed-tools:`, `model:`, `version:` or
`when_to_use:` loads here unchanged, minus the behaviour we don't implement.
That is deliberate: the ecosystem's skills are the reason to have skills at
all, and refusing to parse a file over a key we don't use would lock them out.
(`when_to_use` is the one such key we *do* read — the reference appends it to
the description in the listing, and it is pure prose, so it costs nothing.)

### Roots

Discovered once at startup, in precedence order — the **first** root to claim
a name wins, so a project can shadow a personal skill of the same name:

| # | Root | Scope |
|---|------|-------|
| 1 | `<cwd>/.alter-zero/skills` | project |
| 2 | `<cwd>/.claude/skills` | project (compat) |
| 3 | `{config_home}/skills` | personal (`~/.alter-zero/skills`) |
| 4 | `~/.claude/skills` | personal (compat) |

The personal root hangs off the **config home**, so it follows
`ALTER_ZERO_CONFIG_DIR` exactly like `hooks.json`, `permissions.json`,
`settings.json` and `config.json` do — one rule for where per-user state
lives, rather than a second one reading `$HOME` directly.

`ALTER_ZERO_SKILLS_DIR` **replaces** the whole list rather than adding to it.
That is the convention every other `*_DIR`/`*_FILE` override here follows
(`ALTER_ZERO_CHECKPOINTS_DIR`, `ALTER_ZERO_HOOKS_FILE`), and it is the only
shape that can make a test run hermetic: a merely-prepended root still leaves
the developer's own `~/.claude/skills` in every session, which is exactly how
`smoke.sh` Phase 36 broke on the first full run — a real personal skill's
listing displaced the context rows that phase asserts on. The suite now points
the variable at an empty temp dir.

The `.claude/` rows are the pragmatic half of the design: skills are portable
markdown with no tool-specific behaviour in them, the ecosystem writes them to
`.claude/skills`, and reading that directory costs one `read_dir` that usually
returns `NotFound`. A user who has already written skills for Claude Code gets
them here for free.

Discovery runs **once**, at startup. A skill's *body* is re-read on every
invocation, so editing a `SKILL.md` mid-session takes effect on the next call;
*adding* one needs a restart. That asymmetry is deliberate — re-walking four
roots and re-parsing every `SKILL.md` each turn would cost O(skills) of I/O
per turn and, worse, a listing that changed mid-session would invalidate the
prompt cache behind it for no gain the common case can feel.

Discovery never fails a session: an unreadable root is skipped, and a
`SKILL.md` that won't parse is collected as a [`SkillError`] and surfaced as a
one-row startup toast naming the file and the reason
(`Skill /p/.claude/skills/broken/SKILL.md: missing YAML frontmatter delimited
by ---`, `(+N more)` when there are others), never a panic and never
silence — going quiet is what makes "my skill isn't being used" unanswerable.

## The listing

Every turn, the derived context leads with the project doc (AGENTS.md) and
then the skill listing, wrapped in the reference's `<system-reminder>`:

```
<system-reminder>
The following skills are available for use with the Skill tool:

- commit: Create a git commit with staged changes
- dataviz: Use when creating any chart, graph, plot or dashboard…
</system-reminder>
```

It is a **leading fragment**, not history: `App::skill_listing` is rendered at
the boundary and `context::context_messages_full` injects it in front of the
conversation — so Ctrl+D shows it, `App::estimate_context_tokens` counts it,
and it survives a `/compact` at the front exactly like the project doc. It
never enters `history`, so it cannot be backtracked past or recorded twice.

Its position is *after* `user_instructions` and before everything else, which
is a prompt-cache decision: both are re-rendered per turn, and a fragment that
moves invalidates every token behind it.

The listing is budgeted like the reference's `formatCommandsWithinBudget`:
each entry's description is capped at [`MAX_LISTING_DESC_CHARS`] (250), and if
the whole listing still exceeds [`listing_budget`] — 1% of the model's context
window, in characters, the reference's `SKILL_BUDGET_CONTEXT_PERCENT` — the
descriptions are trimmed to an even share, degrading to names-only rather than
dropping skills. A skill you can't see is a skill you can't invoke; a skill
with a short description is merely a worse match.

## The tool

Offered as `skill` (lowercase on the wire like every other tool here,
displayed as `Skill`) **only when at least one skill loaded** — an empty
listing means the tool has nothing to do, and both references omit it too.

```json
{"skill": "dataviz", "args": "quarterly revenue"}
```

`args` is optional. It is substituted into the body for `$ARGUMENTS`, and for
`$1`…`$9` split on whitespace — the reference's `substituteArguments`. A body
with no placeholder and non-empty args gets them appended as a final
`Arguments: {args}` line, so an argument is never silently dropped.

The body the model receives is prefixed with its own directory so relative
references resolve:

```
Base directory for this skill: /home/u/.claude/skills/dataviz

# Data visualization
…
```

`${ALTER_ZERO_SKILL_DIR}` / `${CLAUDE_SKILL_DIR}` in the body expand to that
same directory (the second spelling for ecosystem compatibility).

### Why the result is a two-text split

`ToolOutcome::context` already exists for exactly this shape — "the cell shows
one thing, the model reads another" — built for `AskUserQuestion`
(`docs/ask.md`). A skill call reuses it wholesale rather than growing a
parallel mechanism:

- `output` = `Successfully loaded skill` → the committed cell's `⎿` row;
- `context` = the whole rendered body → what the tool call returns to the model.

Everything downstream is inherited and needs no new code: the green
`StreamEvent::ToolAnswered` path, `ToolCall::context_output` recording,
`context::context_messages` replaying the body as the `tool` result on every
later turn, the rollout round-trip so a `/resume` restores it, and Ctrl+D
showing what was actually sent. The reference achieves the same thing by
injecting a separate user message after the tool result; we don't need one,
because a `tool`-role result is already a place a long text can live here.

**Ctrl+O keeps the one line too.** The transcript is the record of what
happened; Ctrl+D is the record of what was *sent*. That is already the rule for
every other two-text call here — a permission-rejected `write` expands to its
red `User rejected …` display, never the stop-and-wait text the model read —
and it is the reference's behaviour as well. It also keeps a 100 KiB skill body
out of the transcript render cache, which is rebuilt per commit.

The replayed call carries `{"skill": "<name>"}` rather than the reference's
verbatim arguments: history stores the one-line summary, and for this tool the
summary *is* the name. That is exact for the required parameter (a provider
that validates the schema rejects `skill({})`, which is what an unmapped tool
would have replayed) and lossy only for the optional `args`, whose effect is
already baked into the body sitting right below in the same context.

The body is capped at [`SKILL_BODY_MAX_BYTES`] (100 KiB — the reference's
`maxResultSizeChars`), truncated with a marker rather than refused.

A call naming an unknown skill resolves **red and recoverable** with the
available names listed, so the model can correct itself in the same turn
rather than ending the round on an error.

### What it deliberately does not do

The reference's `context: fork` (run the skill in a subagent), `allowed-tools`
(widen the permission set for the skill's duration), `model:`/`effort:`
overrides, hooks-in-frontmatter, and `!`-shell interpolation in the body are
all **parsed-and-ignored**. Each is a permission or a control-flow decision
wearing a markdown file's clothes, and this TUI already has explicit,
user-visible seams for all of them (`/settings`, Ctrl+A, `docs/permissions.md`,
`docs/agent-tool.md`). A skill here is *text*, and text cannot escalate.

That is also why a skill invocation raises **no permission prompt**: nothing
runs. The body it loads is subject to every gate it then tries to pass
through — a skill telling the model to run `rm -rf` still meets the permission
modal at the `bash` call.

## `/<skill-name>`

The reference's user-invocation shorthand works, and needs no code of its own.
Typing `/haiku-writer about tmux` matches no built-in command, so it submits as
an ordinary user turn; the system prompt's guidance line (`prompts/tools.md`)
tells the model that `/<skill-name>` *is* a skill invocation, and the listing
tells it which names exist — so it answers with a `skill` call carrying the
rest of the line as `args`. Verified end to end against a live model:

```
❯ /haiku-writer about tmux
● Skill(haiku-writer)
  ⎿  Successfully loaded skill
● Grid of terminals, …
```

The user-invoked and model-invoked paths therefore converge on one
implementation rather than two.

What is **not** implemented is palette *autocomplete*: typing `/` lists only
the built-in commands, not the discovered skills. `app::COMMANDS` is a `const`
array and `matching_commands` hands out `&'static SlashCommand`, so mixing in
runtime-discovered entries means making that surface owned — a refactor
reaching `CommandMenu`, `ui::menu`, and their tests, for discovery alone. It is
the obvious next increment, and nothing here forecloses it.

## Settings

`/settings` gains a **Skills** row (`SettingKey::Skills`): on by default,
`false` drops the tool and the listing at the next turn — the row reports
`false (unavailable)` when no skill loaded, since there is nothing to turn on.
`ALTER_ZERO_SKILLS=0` seeds it off for the session.

## Module layout

| Where | What |
|---|---|
| `src/skills.rs` | **pure**: `SkillMetadata`, frontmatter parse, name validation, listing + budget, `$ARGUMENTS` substitution, body render, the `SkillRegistry` handle |
| `src/llm/skill.rs` | **boundary**: root resolution, the `read_dir` walk, the tool executor (`run_skill_tool`) |
| `src/llm/tools.rs` | `skill_spec()`, `display_name`, `summarize_call` |
| `src/llm/backend.rs` | `with_skills` — the `with_tasks` pattern |
| `src/context.rs` | the leading listing fragment |
| `src/tui/bootstrap.rs` | discovery at startup, the failure toast |
| `src/tui/models.rs` | `with_skills` on every backend rebuild, `sync_skill_listing` |

Subagents carry the tool too (`SubagentConfig::skills`): a side agent benefits
from an authored `SKILL.md` exactly as the lead does, and loading one is pure
text, so it needs no gate of its own.

The offline `DummyAi` carries a `skills` scenario (cue `skill`) whose cell and
context entry are built by [`SKILL_LOADED_DISPLAY`] and [`render_skill_body`] —
the very constants and formatter `run_skill_tool` uses — so the offline demo is
byte-for-byte the live one, the rule every scripted demo follows. `smoke.sh`
Phase 76 drives it, and gets its hermetic (skill-free) session for every other
phase from `ALTER_ZERO_SKILLS_DIR` pointed at an empty temp dir.
