# The session scratchpad — Design

Date: 2026-08-24

## Goal

Give the agent a **temp directory of its own**, and tell it about it. Claude
Code's rule, ported whole: every intermediate result, throwaway script and
working note goes to a session-private directory instead of `/tmp`, where it
can't collide with another session, can't be mistaken for the user's work, and
doesn't need a permission prompt to write.

The same move organises what was already there: the background shells' interim
`{id}.output` files move under a `tasks/` leaf, so one session root holds two
named children instead of one flat pile.

## The layout

```text
{tmp}/alter-zero-{uid}/{session}/
├── scratchpad/            the agent's temp files
└── tasks/                 {id}.output — background shells' interim output
```

e.g. `/tmp/alter-zero-1000/18cea7cc0aee22c0-5d77f/scratchpad`.

- `{tmp}` is `std::env::temp_dir()` — `TMPDIR` moves the whole tree.
- `alter-zero-{uid}` is Claude Code's `claude-{uid}` pattern: one stable root
  per user, so a shared `/tmp` never mixes two users' sessions.
- `{session}` is `tui::host::session_id()` (nanos + pid in hex), keeping
  concurrent instances off each other's files.

Both leaves are read *by the model* — the scratchpad out of its system prompt,
a task's `{id}.output` out of every background launch text — so the layout
stays as short as uniqueness allows. The session id already separates
projects, which is why there is no dashed-cwd segment.

The shape is the pure `scratchpad` module (`session_root` / `scratchpad_dir` /
`tasks_dir`); the boundary injects the temp dir, uid and session id (the
`set_session_info` pattern) in `tui::bootstrap`.

**One session id.** `bootstrap` mints it once now and shares it: the temp tree
*and* the lifecycle hooks' payloads (`docs/hooks.md`). It used to call
`host::session_id()` twice — and since the id is nanos-derived, the two
answers differed, so a hook could not find the session's own files from the id
it was handed.

## The prompt block

`prompts/scratchpad.md`, in the persona's terse style, with the one
`{scratchpad}` placeholder the boundary fills:

```
## Scratchpad

Dir {scratchpad}
Put temp files here not /tmp: intermediate data, scratch scripts, working notes
Session private and outside the project so file writes here need no approval
Use /tmp only when asked
```

It is the **third** block of the assembled system prompt:

```
persona        prompts/alter_zero.md    (who you are)
environment    prompts/environment.md   (where/when you are)
scratchpad     prompts/scratchpad.md    (where your scratch goes)   ← this doc
```

`augment_with_scratchpad` composes after `augment_with_environment` — same
rules, so the pair reads as a pair: a blank base is returned unchanged (the
"empty `ALTER_ZERO_SYSTEM_PROMPT` → no system message" contract,
`docs/context.md`), and the whole assembly is visible in the Ctrl+D
context-debug view. Subagents inherit it with the rest of the prompt
(`docs/agent-tool.md`), so a side agent scratches in the same directory.

The block is **omitted entirely** when the session has no scratchpad — the
feature is off, or the directory could not be created. Pointing a model at a
path that does not exist, and refusing its writes there in the same breath, is
worse than saying nothing.

## Writes there need no approval

The block claims file writes in the scratchpad need no approval, so they must
not raise one. `PermissionGate::scratchpad_covers` answers that, and
`llm::approval::approve_call` consults it **beside the standing allowlist** —
before the `PermissionRequest` hook, the auto-mode classifier and the prompt,
because it answers the same question those do: *may this run without asking?*

It is deliberately narrow:

- **`write` and `edit` only.** Their `target` *is* the thing being changed. A
  `bash` command naming a scratchpad path still asks — what it goes on to
  touch is its own business — and so does an MCP call.
- **Strictly inside, lexically** (`scratchpad::contains`): both paths absolute
  (a relative target resolves against the cwd, which is the user's project), a
  `..` anywhere refuses outright rather than resolving, and the match is
  component-wise so `{root}-elsewhere` is not inside `{root}`.
- **A forced ask still asks.** A `PreToolUse` hook's `permissionDecision:
  "ask"` means *a human decides*, and it overrides this exactly as it
  overrides the allowlist (`docs/hooks.md`).

It is also **visible**: the call resolves as `Approval::AllowNoted` with
`SCRATCHPAD_ALLOWED_NOTE`, so the cell wears a dim
`⎿ Allowed in the session scratchpad` row — the classifier note's sibling
(`docs/permissions.md`), recorded on the call, replayed in Ctrl+O, and
round-tripped by a `/resume`. Nothing runs unasked without saying so.

## Configuration

- `ALTER_ZERO_SCRATCHPAD` — falsy (`0`/`false`/`no`/`off`) turns the whole
  feature off: no directory, no prompt block, no exemption.
- `ALTER_ZERO_SCRATCHPAD_DIR` — use this exact directory instead of
  `{session_root}/scratchpad` (the `ALTER_ZERO_SKILLS_DIR` convention).
- `TMPDIR` moves the whole session tree, tasks dir included — which is how
  `smoke.sh` Phase 91 gets a tree it can find unambiguously.

## Testing

- Pure (`src/scratchpad.rs`): the root/leaf shapes, and `contains` against the
  escapes it must refuse — relative, `..`, the sibling-prefix directory, the
  root itself, the `tasks` sibling.
- Pure (`llm::backend`): `render_scratchpad` leaves no placeholder;
  `augment_with_scratchpad` appends after the environment block (order
  persona < environment < scratchpad) and leaves a blank base alone.
- Pure (`permission`): the gate covers `write`/`edit` inside and nothing else
  — not `bash`, not MCP, not outside, not relative, not through `..`.
- Pure (`llm::approval`): a scratchpad write returns `AllowNoted` with no
  request raised; a write outside still raises one; a forced ask still asks.
  (The exemption test runs with a **pre-cancelled** token: it changes nothing
  when the exemption works — it resolves before the gate is consulted — but a
  regression fails fast instead of blocking the suite on a prompt nobody will
  answer.)
- Live (`tests/live_openrouter.rs`, `--ignored`): a real model reads the
  scratchpad path back out of its prompt and names it as where a temporary
  file goes; and a real `write` into the scratchpad lands on disk with no
  `StreamEvent::Permission` raised and the note on its cell.
- Boundary: `scripts/smoke.sh` drives the real binary — the directory exists
  under the session root, and a backgrounded `!` command tees into
  `{session}/tasks/{id}.output` beside it.

## Known limitations

- **A symlink inside the scratchpad can point outside it.** The containment
  test is lexical — no filesystem is consulted, so a `write` through such a
  link would be allowed. Creating the link takes a `bash` call, which asks.
- **Nothing sweeps old session directories.** Each session leaves its
  scratchpad and task output behind for the OS's own `/tmp` cleaning, exactly
  as the interim files always did — an unused scratchpad included, since the
  directory is created up front so the prompt can name a path that exists.
- **A `/resume`d session gets a new scratchpad.** The id is per *launch*, not
  per conversation, so a resumed transcript's older tool records name the
  previous session's directory while the prompt names the new one. The files
  are still on disk at the path the transcript shows; nothing rewrites them.
- The scratchpad is **not** a `/settings` row: it is a launch-time fact (the
  prompt is assembled once at startup), so the environment variables are its
  only switch.
