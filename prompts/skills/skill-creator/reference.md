# Loader reference

The exact tokens the skill loader rewrites, and the limits it enforces. This
is a **read** file, not a loaded body: what is written here reaches you
verbatim, which is why it is here rather than in `SKILL.md`.

## Argument substitution

A `skill` call may carry an `args` string. Before the body is handed to the
model, the loader rewrites, in the body text:

| Token | Becomes |
|---|---|
| `$ARGUMENTS` | the whole `args` string |
| `$1` … `$9` | `args` split on whitespace, one token each; an absent slot becomes empty |

If the body contains **none** of those tokens and `args` was non-empty, the
loader appends one final line instead:

```
Arguments: <the args string>
```

so an argument is never silently dropped. A body that uses `$1` but not `$2`
gets no appended line — one placeholder counts as substituted.

## The skill's own directory

| Token | Becomes |
|---|---|
| `${ALTER_ZERO_SKILL_DIR}` | the absolute path of the folder holding this `SKILL.md` |
| `${CLAUDE_SKILL_DIR}` | the same path (the second spelling exists for skills written for Claude Code) |

Every rendered body is also prefixed with:

```
Base directory for this skill: <that same path>
```

so relative references (`reference.md`, `templates/pr.md`) resolve without a
placeholder at all. Use the placeholder when you need the path *inside* a
command line the model will run.

## The trap this file exists for

The substitutions above run over the **whole body**, including its prose, its
code fences and its examples. A skill body therefore cannot document these
tokens: writing `$ARGUMENTS` in a body produces the caller's arguments, and
writing the skill-dir token produces a path. The instructions come out looking
correct and saying something else.

When a body must show one of these tokens literally, put that part in a file
beside `SKILL.md` — like this one — and have the body say to read it.

## Limits

| Limit | Value | What happens past it |
|---|---|---|
| `description` | 250 characters | cut with `…` in the listing every request carries |
| The whole listing | ~1% of the model's context window | descriptions trimmed to an even share, then names only — no skill is dropped |
| Body | 100 KiB rendered | truncated, with a marker appended |
| `name` | 64 characters, `^[a-z0-9]+(-[a-z0-9]+)*$` | the skill is refused at load, with a toast naming the file |

## Failure modes worth knowing

- **Missing or malformed frontmatter** (`---` fences) or a missing
  `description:` — refused at load; the toast names the file and the reason.
- **Two skills with the same name** — the first root in precedence order wins
  and the other is invisible. Fix by renaming, not by editing both.
- **A skill that never triggers** — the description is almost always the
  cause, not the body: the model chooses from the description alone.
