# Loader reference

The exact tokens the skill loader rewrites, and the limits it enforces. This
is a **read** file, not a loaded body: what is written here reaches you
verbatim, which is why it is here rather than in `SKILL.md`.

## Arguments

There are none. A `skill` call carries exactly one field — `skill`, the name —
and the loader performs **no** argument substitution: `$ARGUMENTS`, `$1` … `$9`
and every other `$`-token are ordinary body text that reaches the model as
written.

Give a skill everything it needs in its own body, or have the body say which
file to read for the rest. If a skill needs a value that varies per run, ask
for it in the body ("ask the user which branch to review") rather than
expecting a parameter.

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

The skill-dir expansion above runs over the **whole body**, including its
prose, its code fences and its examples. A skill body therefore cannot document
those two tokens: writing one produces a path, so the instructions come out
looking correct and saying something else.

When a body must show one of them literally, put that part in a file beside
`SKILL.md` — like this one — and have the body say to read it. (`$ARGUMENTS`
used to be caught by the same trap. It no longer is: with the `args` parameter
retired there is no substitution pass, so that token is safe in a body — the
skill-dir tokens are the only ones left that are not.)

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
