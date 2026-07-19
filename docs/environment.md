# Environment context in the system prompt — Design

Date: 2026-07-19

## Goal

Give the "Alter Zero" agent **context awareness** of where and when it runs:
the current **date**, **os**, and **cwd** ride in the system prompt every
session. Without them the model guesses the date, assumes a platform, and has
no idea which directory its `bash`/`read`/`write`/`edit` tools act in.

The block is authored in [`prompts/environment.md`](../prompts/environment.md),
terse in the persona's own style:

```
Know your runtime environment

Date {date}
OS {os}
Directory {cwd}
```

The `{date}`/`{os}`/`{cwd}` placeholders are filled at runtime, e.g.:

```
Know your runtime environment

Date Sunday 2026-07-19
OS linux
Directory /home/user/inline-tui
```

## Where it sits in the prompt

The full system prompt the real backend sends is three blocks, in order:

```
persona        prompts/alter_zero.md   (who you are)
environment    prompts/environment.md  (where/when you are)   ← this doc
tools          prompts/tools.md        (what you can do)      when tools are on
```

`persona → environment → tools`. The persona and environment are joined by
`augment_with_environment`; the tools note is appended afterwards by
`LlmBackend::configure` (unchanged). The Ctrl+D context-debug view shows the
whole assembled prompt, so the environment block is visible there too.

## Why this shape

Like the Ctrl+O timestamp clock (`docs/timestamps.md`), a wall-clock and a CWD
read can't live in the pure, deterministically-tested library. So the split is:

1. **The values are gathered at the I/O boundary.** `main.rs` reads the date
   (`local_date` — `chrono::Local`, `%A %Y-%m-%d`), the os
   (`std::env::consts::OS`), and the cwd (`std::env::current_dir`, already in
   hand at startup), then folds them into `system_prompt` **once**. Every
   backend the loop rebuilds on a `/model` switch inherits the block via
   `system_prompt.clone()`, so there is a single injection point.

2. **The formatting is pure and unit-tested** (`llm::backend`):
   - `render_environment(date, os, cwd)` fills the template — every `{token}`
     is substituted, none survive.
   - `augment_with_environment(base, date, os, cwd)` appends the rendered block
     to a base prompt after a blank line.

3. **A blank base stays blank.** The "empty `INLINE_TUI_SYSTEM_PROMPT` → no
   system message" contract (`docs/context.md`) is preserved:
   `augment_with_environment` returns a blank base unchanged, so
   `configure` still drops it to `None`. Any non-empty prompt — the default
   persona *or* a custom `INLINE_TUI_SYSTEM_PROMPT` — gets the environment
   block, because context awareness is orthogonal to persona.

## Testing

- `llm::backend` (pure): `render_environment` fills every placeholder and
  leaves no `{`; `augment_with_environment` appends the block after the base,
  leaves a blank base untouched, and — composed with `configure` — yields the
  persona → environment → tools order.
- `main.rs` (boundary): the date/os/cwd gathering is verified by running the
  app (Ctrl+D shows the block) and by the live OpenRouter check that the model
  can report its cwd from the prompt.

## Known limitations

- The date/os/cwd are captured at **session start** (and on `/model` rebuilds),
  not per turn — a session spanning midnight keeps the start date, and a `cd`
  performed by a tool call is not reflected. This matches the session-scoped
  clock and is fine for a terminal session.
