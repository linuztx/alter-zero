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

In precedence order — the **first** root to claim a name wins, so a project
can shadow a personal skill of the same name, and a subdirectory can shadow
its own project:

| # | Root | Scope |
|---|------|-------|
| 1 | `<cwd>/.alter-zero/skills` | directory |
| 2 | `<cwd>/.claude/skills` | directory (compat) |
| 3 | `<project root>/.alter-zero/skills` | project |
| 4 | `<project root>/.claude/skills` | project (compat) |
| 5 | `{config_home}/skills` | personal (`~/.alter-zero/skills`) |
| 6 | `~/.claude/skills` | personal (compat) |

Rows 3–4 are the **`.git` walk-up** — `project_doc::find_project_root`, the
same discovery `AGENTS.md` uses — and they are skipped entirely when the
project root *is* the cwd, which is the common case: launched at the repo
root, the walk reads the same two directories it always did. They exist
because launching in `repo/src` must still find `repo/.claude/skills`; a
skill is a property of the project, not of whichever directory you happened
to start in. The cwd keeps precedence over the project root for the same
reason project keeps it over personal: the more specific root wins.

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

The project rows sit deliberately **outside** the `/trust` gate that guards
the project's hooks and MCP files (`docs/project-config.md`): a `SKILL.md`
is inert markdown until the model explicitly loads it, and any command in a
loaded body still meets the permission gate like every other tool call —
there is nothing here that executes on discovery.

### The built-in `skill-creator`

One skill ships with the binary: **`skill-creator`**, which teaches this
format — the frontmatter contract, where to write the folder, how to word a
description that actually triggers, and how to update an existing skill
without clobbering it.

It exists because the format is *ours*. A model asked to "write me a skill"
without it writes something plausible — a lone `my-skill.md` at a root, an
`allowed-tools:` line it expects to be honoured, a description that reads like
a persona — and every one of those fails silently: the walk only looks for
`<root>/<name>/SKILL.md`, and a skill that is never listed is a skill that is
never chosen. The rules are cheap to state and impossible to guess.

Authored in `prompts/skills/skill-creator/` and `include_str!`'d beside every
other markdown this crate carries, then **written into `{config_home}/skills`
at startup when the file is absent** — the agent definitions' rule
(`docs/subagents.md`), for the agent definitions' reasons: a default that is a
real file on disk can be read, edited and diffed, and a release that improves
it ships the improvement. The seed runs *before* the walk, so the session that
installed the app can already use it. An edited copy is never overwritten; a
deleted file comes back next launch, so the off-switch is `/skills` (which
persists) rather than `rm -rf`.

One difference from the agent definitions: `ALTER_ZERO_SKILLS_DIR` is **never
seeded into**. That variable replaces the root list — it says "these are the
skills, and only these" — and a built-in skill is a convenience the session
works without, where a built-in *agent type* has to resolve because
`general-purpose` is the `agent` schema's default. So the override is honoured
literally: nothing of ours is written into a directory the user curates, and a
hermetic run (`smoke.sh`) keeps the empty root it made.

#### Why it is two files

The skill is a directory holding `SKILL.md` **and** `reference.md`, and the
split is forced by the loader itself. A body is rendered through the
`${…SKILL_DIR}` expansion before the model sees it — so a body that
*documents* those tokens has them rewritten out from under it. The first live
run of an earlier draft is the evidence: the sentence naming both
`${…SKILL_DIR}` spellings arrived as the same absolute path twice, explaining
nothing. (`$ARGUMENTS` was caught by the very same trap in that run, reaching
the model as `` `create commit-style` ``; retiring the `args` parameter is
what makes that token safe in a body today, and the skill-dir pair the only
one left that is not.)

Detail that has to survive verbatim therefore lives in a sibling file the
model **reads** — which is also the multi-file pattern the skill teaches, so
the built-in demonstrates it rather than only describing it.
`no_built_in_body_carries_a_placeholder_the_loader_would_eat` keeps a body
from re-acquiring one: it renders every built-in and requires the body back
byte-for-byte.

### The walk re-runs every turn

Discovery runs at startup **and at every turn start** —
`Session::rescan_skills`, right beside the `AGENTS.md` refresh and for the
same reason. A skill's *body* was always re-read on every invocation, so
editing a `SKILL.md` mid-session already took effect on the next call; what
the startup-only walk could not do was notice a skill that had just been
*added*. That left the session frozen at what it booted with — including,
awkwardly, a skill the agent had written for you one turn earlier, which it
then could not use.

The cost is four to six `read_dir`s and one small read per skill, against a
turn that is about to make a network request. Two things follow from the new
set beyond the registry itself, and both matter:

- **The listing is re-rendered**, so this turn's context carries it. A
  listing that *changed* does invalidate the prompt cache behind it — but a
  listing that changed is one where the answer genuinely differs, and an
  unchanged listing renders byte-identically, so the steady state is
  unaffected.
- **The backend is rebuilt only when the tool set changes.** The `skill` spec
  is decided at `with_skills` time, so the *first* skill appearing (or the
  last one going away) has to re-attach; `ModelSession::refresh_skills`
  compares its recorded `skills_attached` against the live verdict and does
  nothing on the turns — nearly all of them — where the answer is the same.
  Rebuilding unconditionally would re-derive the whole backend every turn for
  nothing.

The `/settings` **Skills** row's availability is re-derived too, since "did
anything load" can now flip mid-session.

Discovery never fails a session: an unreadable root is skipped, and a
`SKILL.md` that won't parse is collected as a [`SkillError`] and surfaced as a
one-row toast naming the file and the reason
(`Skill /p/.claude/skills/broken/SKILL.md: missing YAML frontmatter delimited
by ---`, `(+N more)` when there are others), never a panic and never
silence — going quiet is what makes "my skill isn't being used" unanswerable.
Because the walk now repeats, so would the toast: [`unreported_errors`] keeps
it to **once per file**, and the reported set is re-seeded from each walk, so
a file that is fixed and broken again reports again.

## The listing

Every turn, the derived context leads with one `<system-reminder>` whose
sections are the project doc (AGENTS.md, `docs/project-doc.md`), then the
skill listing, then the subagent types the `Agent` tool can launch
(`docs/subagents.md`; any section may be absent — the wrapper is
`reminder::reminder_message`, the skills section is `skills::skill_section`,
and `skills::listing_message` is that section wrapped alone, which is what a
launched subagent gets):

```
<system-reminder>
Use the following contexts and instructions:

Codebase and user instructions are shown below. Be sure to adhere to these instructions. IMPORTANT: …

Contents of /work/my-app/AGENTS.md (project instructions, checked into the codebase):

…

The following skills are available for use with the Skill tool:

- commit: Create a git commit with staged changes
- dataviz: Use when creating any chart, graph, plot or dashboard…

Available agent types for the Agent tool:

- general-purpose: General-purpose agent for researching complex questions… (Tools: *)
- explore: Read-only search agent for broad fan-out searches… (Tools: Bash, Read, Skill, mcp__*)
</system-reminder>
```

It is a **leading fragment**, not history: the two listing sections are
rendered at the boundary into `App::listings` (`subagents::listing_sections`)
and `context::context_messages_full` wraps them, behind the instructions
section, into the one block in front of the conversation — so Ctrl+D shows
it, `App::estimate_context_tokens` counts it, and it survives a `/compact` at
the front exactly like the project doc. It never enters `history`, so it
cannot be backtracked past or recorded twice.

Its position is *after* the AGENTS.md section and before everything else,
which is a prompt-cache decision: every section is re-rendered per turn, and
one that moves invalidates every token behind it.

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
{"skill": "dataviz"}
```

`skill` is the whole schema. The reference's optional `args` string is
**deliberately absent** — see [Why there are no arguments](#why-there-are-no-arguments)
below.

The body the model receives is prefixed with its own directory so relative
references resolve:

```
Base directory for this skill: /home/u/.claude/skills/dataviz

# Data visualization
…
```

`${ALTER_ZERO_SKILL_DIR}` / `${CLAUDE_SKILL_DIR}` in the body expand to that
same directory (the second spelling for ecosystem compatibility). That
expansion is the **only** rewrite the loader performs; everything else in a
body reaches the model exactly as its author wrote it.

### Why there are no arguments

The reference's schema carries an optional `args` string, substituted into the
body for `$ARGUMENTS` and for `$1`…`$9` split on whitespace, with a trailing
`Arguments: {args}` line appended when the body named no placeholder. All of
it is gone. Three things were wrong with it:

- **It rewrote the body's own prose.** The substitution pass ran over the
  whole text — prose, code fences, examples alike — so a skill could not
  *document* the tokens it was written to use. That is not a hypothetical: the
  built-in `skill-creator` had to be split into two files over it, because a
  live run read `` `create commit-style` `` where its `SKILL.md` said
  `$ARGUMENTS`. A body that means `$ARGUMENTS` literally now says so.
- **Nothing supplied it.** A skill is loaded from the model's own tool call or
  from a `$name` mention (`docs/skill-mentions.md`), and a mention is plain
  text carrying no parameter. The field existed for the model to fill in
  freehand — an invented value, substituted into instructions the user wrote,
  with no user in the loop to see it happen.
- **It cost a decision on every call.** A parameter in the schema is one the
  model weighs and fills; the tokens are spent whether or not the skill has
  any use for them.

What a skill needs to vary per run belongs in its body — *ask the user which
branch to review* — where the instruction is visible, rather than in a
parameter the model quietly guesses at.

A call recorded before the change still replays cleanly: the executor's
`SkillArgs` ignores unknown fields (serde's default), so an old rollout's
`{"skill": "x", "args": "y"}` loads `x` and drops the rest.

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

The replayed call carries `{"skill": "<name>"}`: history stores the one-line
summary, and for this tool the summary *is* the name. Since `skill` is now the
whole schema, that replay is **exact** — nothing is lost — where it used to be
lossy for the optional `args` (a provider that validates the schema rejects
the `skill({})` an unmapped tool would have replayed, which is why the
reconstruction exists at all).

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
user-visible seams for all of them (`/settings`, Shift+Tab, `docs/permissions.md`,
`docs/agent-tool.md`). A skill here is *text*, and text cannot escalate.

That is also why a skill invocation raises **no permission prompt**: nothing
runs. The body it loads is subject to every gate it then tries to pass
through — a skill telling the model to run `rm -rf` still meets the permission
modal at the `bash` call.

## `$<skill-name>`

The user-invocation shorthand is the `$` mention, and it needs no code of its
own. A submitted message carrying `$haiku-writer about tmux` is an ordinary
user turn; the **Skill tool's own description** tells the model a `$<name>`
mention is a request to run that skill, and the listing tells it which names
exist — so it answers with a `skill` call naming it. The rest of the line
needs no parameter to carry it: it is already in the user's message, right
there in the same context as the body the call loads. (The guidance lives on
the tool because it rides every request the
tool does — the retired `prompts/tools.md` note and the listing's old closing
sentence each said it a second time, per turn.) Verified end to end against a
live model:

```
❯ $haiku-writer about tmux
● Skill(haiku-writer)
  ⎿  Successfully loaded skill
● Grid of terminals, …
```

The user-invoked and model-invoked paths therefore converge on one
implementation rather than two. The composer's **`$` skill-mention picker**
(`docs/skill-mentions.md`) feeds the same rails: typing `$` anywhere in a
message fuzzy-completes the discovered skills' names and Tab/Enter insert
the `$<name>` mention.

What is **not** implemented is palette *autocomplete*: typing `/` lists only
the built-in commands, not the discovered skills. `app::COMMANDS` is a `const`
array and `matching_commands` hands out `&'static SlashCommand`, so mixing in
runtime-discovered entries means making that surface owned — a refactor
reaching `CommandMenu`, `ui::menu`, and their tests, for discovery alone.
The `$` picker covers the discovery gap from the mention side — every skill
name is completable there — and nothing here forecloses the palette half.

## Turning one skill off: the `/skills` menu

`/skills` opens the **fifth composer-replacing inline picker**, and it is
deliberately the `/settings` menu's twin rather than a new shape — same frame,
same `❯` type-to-search, same aligned `{label}  {value}` column with the same
two-tone colouring, same `(n/total)` counter, same Enter/Space grammar:

```
  ❯
→ commit-helper        enabled
  haiku-writer         disabled
  startup-hook-skill   enabled
  (1/3)
  Write a git commit message in this project's house style…
  Type to search · Enter/Space to enable/disable · Esc to cancel
```

Only the rows differ: one per discovered skill instead of one per knob, with
the skill's **own description** as the line under the list — so the picker
doubles as the browser that answers "what is this skill actually for?", which
is the `/hooks` menu's value in the shape of the `/settings` menu.

Two rows the `/settings` menu has no need of:

- a **session-off note** under the search line when the `/settings` **Skills**
  row is off, so browsing and toggling with the master switch down never reads
  as "my toggles do nothing" (it costs a row only when shown — reserving it
  blank stacked an empty line on the gap beneath it);
- an empty list that **names the roots** rather than just saying "none" —
  `No skills found. Add one at:` over `~/.claude/skills/<name>/SKILL.md` — since
  "why isn't my skill here?" is the only question an empty list ever raises.

### What a toggle changes

Disabling has to hold in **two** places, and does:

- the skill leaves the `<system-reminder>` listing, so the model never chooses
  it;
- `SkillRegistry::find` refuses it, so a model that remembers the name from an
  earlier turn gets the recoverable "unknown skill" error instead of loading
  something the user turned off.

`SkillRegistry::snapshot` still returns it — the menu must show a disabled
skill, or you could never turn it back on. That is why the registry answers
two different questions: `is_empty` ("was anything **found**?", the
`/settings` row's availability) and `has_enabled` ("is anything **on**?",
whether the `skill` tool is offered at all). Turning every skill off withdraws
the tool exactly as having none installed does — the same rule, since a tool
that can only answer "unknown skill" is worse than no tool. That withdrawal is
also why a toggle rebuilds the backend: the registry handle is shared, so the
executor and the listing see the change immediately, but the **tool set** is
decided when `with_skills` runs.

### Where it persists

`{config_home}/skills.json`, `permissions.json`'s shape exactly:

```json
{ "projects": { "/abs/cwd": { "disabled": ["haiku-writer"] } } }
```

Per **project**, because skill relevance is project-specific: a `dataviz`
skill earns its listing tokens in an analytics repo and not in a kernel
driver. The session-wide switch already exists as the `/settings` **Skills**
row, so this file is the finer scope rather than a second copy of the same
decision. Writes are a read-modify-write (another project's entry written
meanwhile survives) and best-effort (a read-only home never kills the TUI); an
empty set **drops** the project's entry, so re-enabling the last skill leaves
no residue and the file stays a diff from "everything on".

A disabled name that isn't installed here is **kept** rather than pruned: the
same file serves a checkout on another machine where that skill does exist,
and pruning on load would silently re-enable it there.

### Verified end to end

Against a live model, with three skills on disk:

| step | observed |
|---|---|
| disable `haiku-writer`, ask for a haiku | no `Skill(…)` cell, and the reply **lost the `— alter-zero` signature** the skill body mandates — the body never reached the model |
| Ctrl+D | the `<system-reminder>` lists the other two only |
| force `skill(haiku-writer)` while others are on | red `⎿ Unknown skill: haiku-writer. Available skills: commit-helper.` — recoverable, and the "available" list excludes every disabled one |
| disable **all** | no listing at all, and the tool is withdrawn (`⎿ unknown tool: skill`) |
| restart | the menu opens with the same skills off |
| re-enable | the skill loads again and the signature is back |

The built-in `skill-creator` has its own two live runs (`tests/live_openrouter.rs`,
on the `a0_venice` key):

| test | observed |
|---|---|
| `live_the_built_in_skill_creator_writes_a_skill_this_crate_can_load` | the prompt never names the skill — the **description alone** earns the load — and the `commit-style/SKILL.md` the model then writes is one this crate's own walk discovers and parser accepts |
| `live_the_skill_creator_sends_the_model_to_its_reference_file` | asked for a skill that takes an argument, the model follows the body's pointer, `Read`s `reference.md`, and writes a body using a live `$1` — the indirection works on a real model, not just on paper |

The second is the one worth keeping: the reference file only earns its
existence if a model actually opens it.

The signature is the useful probe: it is *in the skill body and nowhere else*,
so its presence proves the body reached the model and its absence proves it
didn't — better evidence than the cell, which only shows what the TUI drew.

## Settings

`/settings` gains a **Skills** row (`SettingKey::Skills`): on by default,
`false` drops the tool and the listing at the next turn — the row reports
`false (unavailable)` when no skill loaded, since there is nothing to turn on.
`ALTER_ZERO_SKILLS=0` seeds it off for the session. It is the session-wide
switch; `/skills` is the per-skill one.

`/settings` → **Tools** = `false` withdraws the whole tool set, `skill`
included — so it withdraws the listing too. That is
`SessionSettings::skills_offered`, the one gate the listing and the tool set
share, and it is deliberately *not* `skills_active`: the **Skills** row keeps
its own value (Tools must not rewrite it), while what reaches the wire is the
conjunction. Gating the listing on the Skills row alone left a
`<system-reminder>` naming a `skill` tool the request never carried — a dead
end the model spends a round hunting for. That is one invariant with three
enforcement points, and this is the third: the rescan's conditional rebuild
keeps the tool set current, `skills_offered` keeps the listing and the tool
set on one gate, and `subagent_skill_reminder` (below) carries the pair onto
the subagent surface. The `/compact` summarization turn runs on the
tools-free backend and so carries no listing either.

Because a `skill` call goes through the ordinary tool loop, it also meets the
lifecycle hooks: `PreToolUse` can block or rewrite one, `PostToolUse` sees the
load. Claude Code's spelling is aliased, so `{"matcher": "Skill"}` selects it
here unchanged (`hooks::claude_code_alias`, `docs/hooks.md`).

## Module layout

| Where | What |
|---|---|
| `src/skills.rs` | **pure**: `SkillMetadata`, frontmatter parse, name validation, listing + budget, body render, the `SkillRegistry` handle |
| `prompts/skills/skill-creator/` | the built-in skill itself — `SKILL.md` + `reference.md`, embedded and seeded |
| `src/llm/skill.rs` | **boundary**: root resolution, the `read_dir` walk, the built-in seed (`seed_builtin_skills`), the tool executor (`run_skill_tool`) |
| `src/llm/tools.rs` | `skill_spec()`, `display_name`, `summarize_call` |
| `src/llm/backend.rs` | `with_skills` — the `with_tasks` pattern |
| `src/context.rs` | the leading listing fragment |
| `src/tui/bootstrap.rs` | the built-in seed, then discovery at startup, and the failure toast |
| `src/tui/models.rs` | `with_skills` on every backend rebuild, `sync_skill_listing` |
| `src/app/skills_menu.rs` | the `/skills` picker's state, rows and key map |
| `src/ui/skills_view.rs` | the `/skills` picker's rendering and geometry |
| `src/tui/settings.rs` | opening the menu, and applying a toggle |

Subagents carry the tool too (`SubagentConfig::skills`): a side agent benefits
from an authored `SKILL.md` exactly as the lead does, and loading one is pure
text, so it needs no gate of its own.

They carry the **listing** with it — `subagent_skill_reminder`, pushed onto
the agent's message list right after its launch prompt and ahead of any
`SubagentStart` hook note, so it reads as part of the briefing. A subagent
starts on a fresh context, so the lead's `<system-reminder>` never reaches it;
with the spec but no roster it would have to guess a name and read the real
ones back out of the "unknown skill" error, and the spec's own description
would be lying to it ("the available skills are listed in a system-reminder
message in the conversation"). This is the same invariant `skills_offered`
enforces for the lead, applied to the other surface: the tool and the listing
travel together, or neither does. Its budget is the default 8 000 characters —
`SubagentConfig` carries no context window, and a roster is small.

The offline `DummyAi` carries a `skills` scenario (cue `skill`) whose cell and
context entry are built by [`SKILL_LOADED_DISPLAY`] and [`render_skill_body`] —
the very constants and formatter `run_skill_tool` uses — so the offline demo is
byte-for-byte the live one, the rule every scripted demo follows. `smoke.sh`
Phase 76 drives it, and gets its hermetic (skill-free) session for every other
phase from `ALTER_ZERO_SKILLS_DIR` pointed at an empty temp dir.
