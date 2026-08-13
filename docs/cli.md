# CLI resume — `--continue`, `--resume [id]`, and the exit hint

Claude Code's session CLI, ported onto the `/resume` rollout files
(`docs/resume.md`): `alter-zero --continue` reopens the newest conversation
recorded in the current directory, `alter-zero --resume {id}` reopens a
specific session from anywhere, a bare `alter-zero --resume` opens the
`/resume` picker as the very first screen — and quitting a session that
recorded anything prints the copy-paste command that brings it back. (The
same pre-TUI boundary also answers the `mcp` subcommand family —
`alter-zero mcp add/add-json/remove/get/list`, `docs/mcp-cli.md` — routed
by a first argument of `mcp` before the flag grammar below applies.)

```
────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────
  dummy_model_name · ~/repo

Resume this session with:
alter-zero --resume 18a9f2c33d41e5b6-1a2b
```

## What Claude Code does (findings)

- `claude --continue` / `-c` — non-interactive: load the most recent
  conversation **in the current directory** and carry on. With nothing to
  continue it errors out ("No conversation found to continue") instead of
  starting fresh, so a scripted `-c` can't silently lose its thread.
- `claude --resume [id]` / `-r` — with an id (Claude's are UUIDs), resume
  that session; **bare**, open the interactive session picker. An unknown id
  is an error, not a fresh session.
- Both are startup-only concerns; in-session behaviour is unchanged.

## What we build

The same three surfaces, sized to this codebase: a pure `cli` module (arg
parsing + the hint/usage strings), two pure `session` helpers (the filename
→ id mapping and the newest-in-cwd pick), and a thin boundary in `main.rs`
that resolves flags to a rollout *path* before the terminal is ever touched.

### Session ids

The id is the one the recorder already mints (`docs/resume.md`): the
`{id}` segment of `rollout-YYYY-MM-DDThh-mm-ss-{id}.jsonl`, also recorded on
the file's `session_meta` line — nanos-since-epoch + pid in hex. Nothing
about the on-disk format changes; the id merely becomes *addressable*:

- `session::rollout_file_id(file_name)` extracts it back out of a rollout
  filename (`None` for foreign names — the stamp's shape is checked, so a
  stray `rollout-notes.jsonl` can't yield a garbage id).
- `--resume {arg}` matches, in order: an existing rollout **path** (handy —
  the sessions dir is plain files), an exact **filename**, an exact **id**,
  then a **unique id prefix** (two matches make the prefix ambiguous — an
  error listing nothing, never a guess). The scan reuses the `/resume`
  listing's newest-first date-dir walk and needs no head reads.

### The pure `cli` module (src/cli.rs)

- `Cli` — the parsed invocation: `Run` (no args), `Continue`,
  `Resume(Option<String>)`, `Help`, `Version`.
- `parse(args)` — over `std::env::args().skip(1)`: `--continue`/`-c`,
  `--resume [id]`/`-r [id]` (also `--resume=id`), `--help`/`-h`,
  `--version`/`-V`. Anything else — unknown flags, stray positionals,
  `--continue --resume` together — is `Err(message)`; `main` prints the
  message + `USAGE` to stderr and exits `2` (usage errors), while a
  *resolution* failure (nothing to continue, unknown id) exits `1`.
  `--resume`'s value may be a following argument or `=`-attached; a leading
  `-` on the next argument means "bare `--resume`, then that flag" (so
  `--resume --help` doesn't eat `--help` as an id).
- `resume_hint(bin, id)` — the exit hint exactly as shown above (two lines,
  no indent). `bin_name(arg0)` derives the printed program name from how the
  user actually invoked it (`alter0` stays `alter0` — the crate ships both
  bin names), falling back to `alter-zero`.
- `USAGE` — the `--help` text; `main` prints it verbatim.

### The boundary (main.rs)

- **Arg parsing runs in `main()`, *after* the detached-exec hook.** The hook
  must stay the first statement (invariant 1 — a helper re-exec never parses
  TUI flags; its argv is `__alter-zero-detached-exec {cmd}`), and the CLI
  work touches no terminal state, so `--help`/errors print to a normal
  cooked-mode stdio and exit before the tokio runtime boots.
- **Flags resolve to a `Startup` directive before `InlineViewport::init`**
  — fail fast, no TUI flash:
  - `--continue`: `list_sessions(root, None)` (the `/resume` scan —
    eligibility included, so a session you never typed into doesn't
    continue) filtered by `session::latest_for_cwd` against
    `cwd.display()` — the same verbatim string the recorder writes and the
    picker's `Cwd` filter compares. No match → stderr + exit 1.
  - `--resume {id}`: the finder above. No match → stderr + exit 1.
  - The chosen file is read + parsed **here**; an unreadable/foreign file is
    the same fail-fast error. `Startup::Load` carries the parsed
    `(path, text, meta, items)` into `run()`, so the in-TUI path cannot
    fail.
  - bare `--resume` → `Startup::Picker`.
- **`run(term, startup)` applies the directive before the first frame:**
  - `Load`: mirrors the picker's `ResumeSession` arm — restore the cwd to
    the session's final checkpoint (backup snapshot first, unknown commits
    no-op; `docs/checkpoint.md`), `app.load_session(items)`, recorder
    `adopt` (same file accumulates, torn-tail repair included) — then
    commits the loaded conversation to scrollback at the boundary: the
    header banner, a blank, and `ui::repaint_lines(history, width,
    RESIZE_REFLOW_MAX_ROWS)` through the normal `insert_before` pipeline.
    **No `Purge` at startup**: a fresh launch must not wipe the user's
    terminal scrollback (the picker's mid-session purge exists to drop the
    *previous conversation's* rows; at startup there is none), and
    `insert_before` is exactly how the header already lands. A restore
    that actually moved files raises the usual toast.
  - `Picker`: the `OpenResumePicker` arm verbatim (scan → open → 
    `enter_overlay` → paint) before the loop. The header banner is *not*
    pre-committed on this path — every overlay return re-emits it via
    `ui::banner_tail`, so the close repaints it (and quitting straight from
    the picker exits through the same return).
- **The exit hint:** `run()` returns `Option<String>` — the recorder's
  active session id, only when `app.history` is non-empty (codex's deferred
  create means an empty session has no file, no id, and prints nothing;
  a `/clear`ed-then-idle session likewise). `tui_main` prints it **after**
  `term.restore()` so the two lines land below the box in normal terminal
  flow, through `cli::resume_hint` with the invoked bin name. A run that
  errored out surfaces its error instead (no hint).

## Known divergences from Claude Code

- **Ids are the recorder's hex stamps, not UUIDs** — same length class, same
  copy-paste ergonomics, no new dependency; old rollout files keep working
  because nothing re-mints ids. Unique-prefix matching is a convenience
  Claude doesn't offer; ambiguity is an error, so it can't misfire.
- **`--continue` is cwd-scoped by the meta line's verbatim cwd string**
  (the picker's `Cwd` filter rule, `docs/resume.md`) — a session recorded
  through a symlinked path won't match its canonical twin.
- **The hint prints on every quit with history** (Claude gates some exits);
  it is plain stdout after restore, so it scrolls away naturally and never
  enters the transcript, the recorder, or the model's context.
- **`--print`/headless modes, `--fork-session`, `--session-id` are out of
  scope** — this TUI has no non-interactive mode to attach them to.

## Testing

- `cli`: `parse` — bare/`-c`/`--continue`, `--resume` with and without id
  (separate, `=`-attached, `-r`), a following flag not eaten as an id,
  help/version, unknown flag / stray positional / `-c -r` conflicts as
  errors; `resume_hint` matches the printed shape; `bin_name` basenames a
  path and falls back on `None`/empty.
- `session`: `rollout_file_id` round-trips the id `rollout_rel_path` embeds
  (dashed ids included), rejects foreign/stampless names; `latest_for_cwd`
  picks the newest `updated_secs` among matching-cwd rows only, `None`
  otherwise.
- `scripts/smoke.sh` Phase 61 (the I/O boundary): quitting a session that
  recorded a turn prints `Resume this session with:` + a `--resume {id}`
  line whose id names the rollout file; `--continue` relaunches straight
  into the old conversation (no picker) and appends to the same file;
  `--resume {id}` does the same by id; bare `--resume` boots into the
  picker; quitting an *empty* session prints no hint; `--continue` in a
  fresh cwd fails fast with exit 1 and no TUI.
