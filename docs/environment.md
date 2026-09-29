# Environment context in the system prompt — Design

Date: 2026-07-19

## Goal

Give the "Alter Zero" agent **context awareness** of where and when it runs:
the current **date**, **os**, **user** and **cwd** ride in the system prompt
every session. Without them the model guesses the date, assumes a platform,
can't tell whether it is root, and has no idea which directory its
`bash`/`read`/`write`/`edit` tools act in.

The block is authored in [`prompts/environment.md`](../prompts/environment.md)
as the persona's `## Environment` section:

```
## Environment

Date {date}
OS {os}
User {user}
CWD {cwd}
```

The `{date}`/`{os}`/`{user}`/`{cwd}` placeholders are filled at runtime, e.g.:

```
## Environment

Date Sunday 2026-07-19
OS linux (Ubuntu 24.04.4 LTS)
User linuztx
CWD /home/linuztx/alter-zero
```

## Where it sits in the prompt

The full system prompt the real backend sends is three blocks, in order:

```
persona        prompts/alter_zero.md   (who you are)
environment    prompts/environment.md  (where/when you are)   ← this doc
scratchpad     prompts/scratchpad.md   (where your scratch goes, docs/scratchpad.md)
```

`persona → environment`, joined by `augment_with_environment`, then the
scratchpad block appended by `augment_with_scratchpad` when the session has a
scratchpad directory — nothing else:
the tool schemas carry their own capability detail, so `LlmBackend::configure`
appends no tools note (the retired `prompts/tools.md` re-spent those tokens on
every request). The Ctrl+D context-debug view shows the whole assembled
prompt, so the environment block is visible there too.

## Why this shape

Like the Ctrl+O timestamp clock (`docs/timestamps.md`), a wall-clock and a CWD
read can't live in the pure, deterministically-tested library. So the split is:

1. **The values are gathered at the I/O boundary.** `tui::host` reads the
   date (`local_date` — `chrono::Local`, `%A %Y-%m-%d`), the os
   (`os_context` — `std::env::consts::OS`, enriched on Linux with the distro
   from `/etc/os-release`, e.g. `linux (Ubuntu 24.04.4 LTS)`), and the user
   (`user_context` — the effective uid and its name, [below](#the-user-line));
   the cwd (`std::env::current_dir`) is already in hand at startup.
   `tui::config::system_prompt` folds them into the prompt **once**. Every
   backend the loop rebuilds on a `/model` switch inherits the block via
   `system_prompt.clone()`, so there is a single injection point — and
   `tui::config::prompt_context` renders the same block from the same reads
   for a subagent whose definition replaces the persona
   (`docs/subagents.md`).

2. **The formatting is pure and unit-tested** (`llm::backend`):
   - `render_environment(date, os, user, cwd)` fills the template — every
     `{token}` is substituted, none survive.
   - `augment_with_environment(base, date, os, user, cwd)` appends the
     rendered block to a base prompt after a blank line.
   - `os_release_name(contents)` parses `/etc/os-release`, preferring
     `PRETTY_NAME` then `NAME` (quotes stripped), so the distro enrichment is
     tested without reading a real file.
   - `passwd_name(contents, uid)` and `user_label(uid, name)` are the user
     line's parse and its wording ([below](#the-user-line)).

3. **A blank base stays blank.** The "empty `ALTER_ZERO_SYSTEM_PROMPT` → no
   system message" contract (`docs/context.md`) is preserved:
   `augment_with_environment` returns a blank base unchanged, so
   `configure` still drops it to `None`. Any non-empty prompt — the default
   persona *or* a custom `ALTER_ZERO_SYSTEM_PROMPT` — gets the environment
   block, because context awareness is orthogonal to persona.

## The user line

Date: 2026-09-29

Whether the agent runs as root decides whether it needs `sudo` at all, and a
model that guesses gets it wrong both ways: `sudo` in a root container that
doesn't have it installed (the Kali image, `docs/docker.md`), or a bare
`apt install` as an ordinary user. A `sudo` that asks for a password is also
a prompt only `bashsend` can answer (`docs/bash-tools.md`), so knowing up
front saves a round. One line settles it, stated as a fact with no
instructions attached, in the block's own terse style:

| The process               | The line           |
|---------------------------|--------------------|
| uid 0, named `root`       | `User root`        |
| uid 0 under another name  | `User toor (root)` |
| any other uid             | `User linuztx`     |
| a uid with no name        | `User uid 1000`    |

**The uid decides root; the name only labels it** (`user_label`). A root
account under another name — BSD's `toor`, or a `$USER` that `su` left
behind — still says `(root)`, and a stale `$USER=root` in an unprivileged
process can never make the line claim root: that uid reads `uid 1000`
instead, as does `docker run --user 1000` naming no account. Off unix there
is no uid, so the name is taken as given, else `unknown`.

Where the values come from (`tui::host::user_context`):

- **The effective uid**, through rustix's safe `geteuid` — the uid
  permissions are checked against, and what `whoami` reports. Not
  `host::process_uid`: that is a path segment for the session's temp tree,
  and its fallback where `/proc` is absent (macOS) is 0, which here would
  claim root.
- **The name** from `/etc/passwd` (`passwd_name`: the first entry carrying
  the uid, as `getpwuid` answers; comments and NIS `+`/`-` compat entries
  skipped), streamed a line at a time and decoded lossily, so a big file
  costs one line of memory and a GECOS field in a legacy encoding can't hide
  the entries after it. An account outside the file — every ordinary macOS
  account, a directory-service login — falls back to `$USER`, then
  `$LOGNAME`, then `$USERNAME`. The crate forbids `unsafe`, so there is no
  `getpwuid` call to lean on.

## Testing

- `llm::backend` (pure): `render_environment` fills every placeholder and
  leaves no `{`, the user on a line of its own; `augment_with_environment`
  appends the block after the base, leaves a blank base untouched, and —
  composed with `configure` — yields the persona → environment prompt whole,
  with no tools suffix. `passwd_name` finds a uid's account, takes the first
  of two sharing a uid, skips comments, blank lines and compat entries, and
  matches the uid field whole; `user_label` covers every row of the table
  above, including the stale-`$USER=root` guard.
- `tui::host` (boundary): the date/os/user gathering — including
  `os_context` reading the real `/etc/os-release` and `user_context` the real
  uid and `/etc/passwd` — is verified by `smoke.sh` Phase 127: a local stub
  stands in for an OpenAI-compatible provider and logs the request, and the
  phase reads the block back off the wire — its `User` line checked against
  `id -un`, the cwd against the launch directory, the four lines in order —
  then finds the same line in Ctrl+D. The live OpenRouter check
  (`live_environment_context_reaches_the_model`) asks a real model to report
  its os, user and cwd from the prompt.

## Known limitations

- The date/os/user/cwd are captured at **session start** (and on `/model`
  rebuilds), not per turn — a session spanning midnight keeps the start date,
  and a `cd` performed by a tool call is not reflected. This matches the
  session-scoped clock and is fine for a terminal session.
- The user is the one the process runs as. A `sudo` or `su` inside a `bash`
  call changes who that command runs as, not the line.
- An account known only to a directory service, with no `$USER`, `$LOGNAME`
  or `$USERNAME` set, reads as `uid N`: the name comes from `/etc/passwd`,
  not from NSS.
