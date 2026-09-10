# The smoke suite — `scripts/smoke.sh`

`scripts/smoke.sh` drives the real binary inside a real terminal (tmux) and
**asserts** on what it painted. It is the only automated coverage of the
terminal I/O boundary — `src/main.rs`, `src/tui/`, `term.rs` — so it polls
for the state it expects and exits non-zero on a mismatch, which lets it gate
a commit or a CI job. This page is the map of the suite's *structure*; what
each phase checks lives in the phase file's own header comment, and the
environment rules the fixture states are in `docs/design.md` ("The smoke
suite defines its own environment").

## Layout

```
scripts/smoke.sh              the runner: discovery, scheduling, logs, summary
scripts/smoke/lib.sh          the fixture + vocabulary every phase sources
scripts/smoke/phases/         one file per phase: NNN-slug.sh
  001-first-turn.sh
  005-bottom.sh
  …
  107b-imagetransmit.sh
  115-telemetry.sh
```

A **phase** is one self-contained scenario: a file that sources the library,
calls `smoke_begin`, drives the app through its own tmux server and asserts
as it goes. The runner's job is only to find the files, run them — on a pool
of parallel workers — and report. Nothing about a phase lives anywhere but in
its file, which is what makes the suite extensible: adding coverage is adding
a file, and removing it is deleting one.

The suite used to be a single 12,000-line script that ran its 115 phases one
after another, shared one config home and one tmux server across all of
them, kept a 130-line cleanup function naming every session and temp dir by
hand, and checked the first 44 phases in one 1,200-line assertion block two
thousand lines below the drives it read from. Splitting it was mechanical
(every assertion survived verbatim); the structural changes are the ones
below.

## Running it

```bash
cargo build && scripts/smoke.sh          # everything, in parallel
scripts/smoke.sh -j 2                    # two workers
scripts/smoke.sh --serial                # one phase at a time, in phase order
scripts/smoke.sh 55 66 107b              # only these phases
scripts/smoke.sh 50-60 permission        # a range; a slug/title substring
scripts/smoke.sh --skip 115              # everything but
scripts/smoke.sh --failed                # re-run what failed last time
scripts/smoke.sh --list                  # phases, tags, last durations
scripts/smoke.sh -v                      # every phase's whole log
scripts/smoke.sh target/release/alter-zero
bash scripts/smoke/phases/055-permission.sh   # ONE phase, output live
```

The runner prints one line per phase as it finishes — `ok`/`FAIL`, the
counter, id, slug and duration — and on a failure the phase's `FAIL:` lines
under it with the path of its log. Every phase's full log (each pane the
phase captured, under a `==== …` header, exactly what the old script printed
inline) lands in `target/smoke/logs/NNN-slug.log`; `target/smoke/times`
remembers each phase's duration so the next run schedules the longest first,
and `target/smoke/failed` feeds `--failed`. The exit status is 0 only when
every phase passed.

A phase run on its own writes the same log to the terminal and cleans up
after itself; `SMOKE_KEEP=1` (or `-k` on the runner) keeps its temp tree for
a post-mortem. `SMOKE_STARTUP_MS`, `SMOKE_JOBS`, `SMOKE_TIMEOUT`, `SMOKE_OUT`
and `SMOKE_BIN` are the environment spellings of the knobs.

## Isolation: per phase, not per suite

`smoke_begin` gives each phase, in a temp tree of its own (`$SMOKE_TMP`, one
`rm -rf` at exit):

- **its own tmux server** on its own socket — every `tmux …` call in a
  phase is a shell function routing to `-S $SMOKE_TMP/tmux.sock`, so two
  phases can never see each other's sessions or paste buffers, and the
  suite's `set-clipboard on` never reaches the developer's server;
- **its own config home** (`$SMOKE_CFG`), skills root and agents root, spelled
  into `$CFG_ENV` / `$CFG_ENV_NOHIST` / `$APP` exactly as before — so a
  standing permission rule, a `/model` choice or a `/settings` value one
  phase persists is invisible to every other (the old suite had to `rm -f`
  the shared `permissions.json` between the permission phases by hand);
- **`$TMPDIR` pointed into the tree**, so a bare `mktemp -d` — and the app's
  own scratchpad — land inside it;
- the sanitized environment (`smoke_sanitize_env`: every `*_API_KEY`, every
  `ALTER_ZERO_*`, `OLLAMA_HOST`, `NO_COLOR`, the display variables), run
  before the phase's first launch, so a phase run on its own is as hermetic
  as one run by the suite.

That isolation is what makes the parallel runner *correct* rather than
merely fast: the phases were already independent scenarios on separate
sessions, but they shared a server and a config home, so "run them at once"
was not a safe transformation until each carried its own.

## The vocabulary (`scripts/smoke/lib.sh`)

| helper | what it does |
|---|---|
| `launch [-c DIR] [-w REGEX \| -n] SESSION COLS ROWS [CMD]` | `tmux new-session` sized as given (CMD defaults to `$APP`), then **waits for the composer prompt** instead of sleeping 0.4s and hoping. `-w` waits for another pattern, `-n` not at all (a shell pane, a `--help` run). |
| `submit SESSION TEXT` | type the text, pause `SMOKE_TYPE_SETTLE` (0.2s — an Enter riding straight on a key burst is read as part of a paste), press Enter |
| `keys SESSION KEY…` / `type_text SESSION TEXT` | `send-keys` with key names / literal text |
| `pane SESSION [capture opts]` | `capture-pane -p`; `-S -N` reaches scrollback, `-e` keeps the SGR escapes |
| `wait_for SECS SESSION [capture opts --] [grep opts] PATTERN` | poll until the pane matches; 1 on timeout |
| `wait_pane SECS SESSION … PATTERN` | the same wait, **printing the last capture** so the phase can assert on — and log — the frame that satisfied it |
| `wait_settled SECS SESSION … PATTERN` | matches AND two identical samples 0.2s apart: the settled layout after a reply, not a mid-stream frame |
| `poll SECS COMMAND ARGS…` | the generic form for a compound condition |
| `wait_file` / `wait_gone` | a file that matches; a session whose app exited |
| `has CONTENT … PATTERN` / `lacks` / `pane_has SESSION …` | the predicates the waits are built on |
| `expect_has CONTENT [grep opts] PATTERN MESSAGE` / `expect_lacks` / `expect_eq` / `expect_ne` / `expect_file_has` | one assertion each: on a miss, `fail MESSAGE` |
| `fail MESSAGE` | record a failed check — `FAIL: Phase N — MESSAGE` on stderr — and keep going: a phase is a scenario, and the later checks usually say more about what broke |
| `note TITLE` / `dump TITLE CONTENT` | a `==== Phase N: TITLE ====` section marker (over a captured pane) in the log |
| `smoke_on_exit CMD…` | run at teardown — a stub server to kill |
| `count_bare_prompts` / `count_rules` / `count_footers` / `count_msg_lines` | the "exactly one input box on screen" counters the resize phases share |

Every timeout is a **cap**: a wait returns the moment the state holds, so
the suite runs at the app's speed on a fast machine and still passes on a
slow one. The rule the old script stated for itself still holds — *poll for
the state, never fixed-sleep and hope* — and the fixed sleeps that remain are
the ones that are part of the fixture (the 0.2s between typing and Enter; a
window a negative assertion needs to stay open; a pause that is the thing
being measured).

## Writing a phase

```bash
#!/usr/bin/env bash
# Phase 116 — the /foo page opens, lists its rows and closes on Esc
# smoke: tags=serial            ← only if the phase measures time
#
# Why this exists, what it drives, what it proves. The header comment is the
# phase's documentation; the runner's --list shows the first line.

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

S116="${S}_foo"
launch "$S116" 100 30
submit "$S116" "/foo"
page="$(wait_pane 3 "$S116" -F "Foo rows")"
dump "the /foo page" "$page"
expect_has "$page" -F "❯ 1. First row" "the page did not list its first row"
expect_lacks "$page" -F "dummy_model_name" "the page did not displace the footer"
keys "$S116" Escape
wait_for 3 "$S116" -F "dummy_model_name ·" || fail "Esc did not close the page"
```

- Name the file `NNN-slug.sh` with the next free number; the id, slug and
  title all come from the file, so there is nothing to register anywhere.
  A letter suffix (`107b`) is for a phase that extends its neighbour.
- `S` is the session-name prefix; name every session `${S}_something` and
  give the phase its own sizes — the frame is whatever the scenario needs.
- Capture the panes you assert on and `dump` them: on a failure the log is
  the evidence, and the old script's habit of printing every capture is what
  made its failures diagnosable from a CI log alone.
- Everything the phase creates goes under `$SMOKE_TMP` (`mktemp -d` does
  this by itself); the harness removes the tree and kills the server, so a
  phase needs no cleanup code — the old suite's per-phase `kill-session`
  lines are harmless and can stay when a phase relaunches a session name.
- A phase that needs the app in a temp cwd passes `-c "$dir"` to `launch`
  and uses `$BIN_ABS`/`$APP_ABS`; one that re-enables checkpoints must do so
  only in such a cwd (docs/checkpoint.md — a restore runs `git clean`).
- A phase whose assertion is a **measurement** — a latency, a byte count over
  a window — tags itself `# smoke: tags=serial` so the runner holds it until
  the parallel pool has drained and runs it alone.

## Speed

Two things make the suite faster than the monolith it replaced, and neither
changes what a phase checks:

1. **Parallelism.** The phases are independent by construction now, so the
   runner packs them onto `-j` workers — by default twice the CPU count,
   capped at eight — longest-known first. The suite is wait-bound, not
   CPU-bound: an app streaming at 45ms per chunk and a shell loop sampling it
   every 100ms use almost nothing, so the wall time divides by roughly the
   worker count until the longest phases (~40s each) bound it.
2. **Waiting for the state instead of a fixed time.** Every launch waits for
   the prompt rather than 0.4s and every poll samples at 100ms — a robustness
   gain more than a speed one (a slow machine no longer races a fixed sleep),
   and the two phases the faster fixture exposed as sampling-sensitive now
   wait for the row they go on to assert (31 accepts the `now` age a
   same-second picker shows; 105 waits for the details page's tail before
   reading it).

Measured on a 4-core machine driving the debug binary, the same 115 phases:

| run | wall time |
|---|---|
| the old monolithic script | 21m16s |
| `scripts/smoke.sh --serial` (one phase at a time) | 21m04s |
| `scripts/smoke.sh -j 4` | 5m16s |
| `scripts/smoke.sh -j 8` (the default here) | 2m38s |

The serial figure is the honest one: a phase's time is the dummy's own
streaming and the waits that are part of its fixture, so the polling launches
save about what the per-phase servers cost, and the packing is the whole
gain. A phase
that measures time (a latency, a byte count over a window) can tag itself
`# smoke: tags=serial` to run alone after the pool drains — none needed it at
`-j 8` on four cores, but the door is there for one that does.

## What the runner catches that a phase cannot

A phase keeps going after a failed check (the later checks say more), so its
exit status is the count of failed checks, not the last command's. The one
failure a phase cannot report on itself is a bash abort — an unbound
variable under `set -u`, a syntax error — which ends the script before its
remaining checks run; bash prints `file: line N: …` for those, and the runner
marks the phase failed when its log carries that line, whatever the exit
status said.
