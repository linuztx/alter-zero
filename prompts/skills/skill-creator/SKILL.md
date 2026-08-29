---
name: skill-creator
description: Create a new skill, or edit and improve an existing one. Use when
  asked to write, add, update or fix a SKILL.md, when a repeated workflow or
  house style should become a reusable skill, or when a skill never triggers
  and its description needs work.
---

# Writing a skill

A skill is a folder holding a `SKILL.md`: YAML frontmatter over a markdown
body. Only the **description** rides in every request; the **body** arrives
only when the `skill` tool loads it. So the description decides *whether* a
skill is ever used, and the body decides how well it goes.

Write skills that are worth their tokens: one description in context forever,
one body loaded on demand.

## 1. Decide whether it should be a skill at all

Write one when the knowledge is **reusable, specific and hard to guess**:

- a workflow with steps that are easy to get wrong (release, migration, triage);
- a house style or convention that must be followed exactly;
- domain knowledge or an API this project uses that is not in the code;
- a checklist worth running the same way every time.

Do **not** write one for a single task you are doing right now, for something
the code or `AGENTS.md` already states, or for generic advice the model already
follows. A skill that says the obvious costs its description on every turn and
earns nothing.

If a task needs to run *code*, the skill is still the right place for the
instructions — it just calls the ordinary tools when loaded. Nothing in a skill
executes on load.

## 2. Pick the directory

`<root>/<name>/SKILL.md`. The roots, first match winning the name:

| Root | Use it for |
|---|---|
| `<cwd>/.alter-zero/skills` | this project (preferred for project skills) |
| `<cwd>/.claude/skills` | this project, shared with Claude Code |
| `<project root>/.alter-zero/skills` | the repo, when the cwd is a subdirectory |
| `<config home>/skills` | personal, every project (usually `~/.alter-zero/skills`) |
| `~/.claude/skills` | personal, shared with Claude Code |

Default to the **project** root when the skill is about this codebase, and to
the **personal** root when it is about how the user works everywhere. Ask which
one only when it is genuinely ambiguous.

The personal root is this skill's own parent directory — the `Base directory`
line above names `<personal root>/skill-creator`. Write the resolved absolute
path (no `~`, no `..`); the file tools want absolute paths, and a prompt asking
the user to approve `../..` is a prompt they cannot read.

## 3. Write the frontmatter

```yaml
---
name: release-checklist
description: Run this project's release steps in order. Use when cutting a
  release, tagging a version, or publishing a build.
---
```

- **`name`** — optional, defaults to the folder name; keep the two identical.
  Lowercase letters, digits and single hyphens (`^[a-z0-9]+(-[a-z0-9]+)*$`),
  64 characters at most.
- **`description`** — required. A skill without one is refused at load.
- Anything else (`allowed-tools:`, `model:`, `version:`, `license:`) is
  **ignored, not rejected**, so a skill written for another tool loads here
  unchanged. `when_to_use:` is appended to the description.

### The description is the trigger

It is all the model sees before choosing. Write it in the third person, say
**what it does** and **when to use it**, and name the words a user would
actually type:

- Good: `Generate release notes from merged PRs. Use when writing a changelog,
  drafting release notes, or summarising what shipped since the last tag.`
- Bad: `Helps with releases.` — no trigger, no scope, never chosen.
- Bad: `You are a release assistant…` — that is body text; the description is
  a label, not a persona.

Keep it under **250 characters**: past that it is cut with a `…` in the
listing, and the cut lands on your triggers.

## 4. Write the body

The body is a prompt, not documentation. Write it for the model that will read
it mid-task:

- Lead with what to do, in order. Imperative sentences, short paragraphs.
- Be concrete: real commands, real paths, real file names, the exact wording
  when wording matters.
- State the decision rules and the traps ("never force-push", "the id is the
  high-water mark, not the count"). The rules are the reason the skill exists.
- Prefer a checklist or a table over prose whenever the content is a set of
  steps or a set of cases.
- Cut everything the model already knows. No "as an AI", no motivational
  preamble, no restating the task back.
- Keep it under a few hundred lines. The whole body is loaded every time, and
  it is truncated past 100 KiB.

Extra files live beside `SKILL.md` in the same folder — reference documents,
templates, scripts, checklists. Point at them by relative path and say *when*
to read them, so the body stays small and the detail stays exact:

```markdown
Read `reference.md` in this skill's directory before editing the schema.
```

Every loaded body is prefixed with a `Base directory for this skill:` line
naming that folder, so a relative reference always resolves.

## 5. Arguments and expansions

A `skill` call can carry `args`, and the loader substitutes them into the body
before the model reads it — as it does a placeholder for the skill's own
directory. The exact tokens are in **`reference.md`, in this skill's
directory** (the `Base directory` line above): read it before writing a body
that uses them.

They are not written here, because they cannot be: the loader expands this
file's own examples on the way in — it did, in the load you are reading.

Most skills need no arguments at all. Add them only when the skill genuinely
takes a parameter.

## 6. Verify it

Skills are re-discovered at the start of **every turn**, so a skill written now
is loadable on the very next message — no restart:

1. Re-read the file you wrote and check the frontmatter delimiters (`---` on
   its own line, top and bottom) and that `description:` is present.
2. A `SKILL.md` that will not parse raises a toast naming the file and the
   reason — that toast is the fastest diagnosis when a skill does not appear.
3. `/skills` lists everything discovered, with each description, and toggles
   one off per project.
4. To use it: mention `$<name>` in a message, or just describe the task and let
   the description do its job. The load shows as `● Skill(<name>)`.

## Updating an existing skill

1. **Find it first.** Search the roots for `<name>/SKILL.md` — do not assume
   which root it came from, and never create a second copy in a different root:
   the first root wins and the other becomes dead weight nobody edits.
2. **Read the whole file** before changing anything. It may be the user's own
   work, or a seeded default they have edited.
3. **Edit, do not rewrite.** Change the lines the request is about and leave
   the rest — including the `name`, which is how the model, the listing and
   `skills.json` all refer to it. Renaming means a new folder and a stale
   entry in the on/off state.
4. If the complaint is **"it never triggers"**, the description is the suspect,
   not the body: add the missing trigger words and the "Use when…" clause.
   If the complaint is **"it triggers but does the wrong thing"**, the body is.
5. Re-read the file after writing, and say what changed.

## What this runtime does not do

Parsed and ignored, so do not promise them: `allowed-tools` (a skill cannot
widen permissions), `model:`/`effort:` overrides, `context: fork`, hooks in
frontmatter, and `!`-shell interpolation inside the body. A skill here is
text — and text cannot escalate. Anything the loaded body then asks for still
meets the ordinary permission prompt.

## Template

```markdown
---
name: <folder-name>
description: <What it does.> Use when <trigger>, <trigger>, or <trigger>.
---

# <Title>

<One line saying what this skill is for.>

## Steps

1. <Do this.>
2. <Then this.>

## Rules

- <The thing that is easy to get wrong.>
- <The thing that must be exact.>
```
