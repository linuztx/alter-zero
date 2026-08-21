# Checkpoints — reset the code, not just the transcript

alter-zero is a coding agent: its `write`/`edit`/`bash` tools change files in the
working directory. But the two "go back in time" gestures only ever rewound the
*conversation* — the **`/resume`** picker (`docs/resume.md`) reloaded an old
transcript, and the **Esc-Esc backtrack** (`docs/backtrack.md`) truncated history
to an earlier message — while the code on disk stayed wherever the latest work
left it. So resuming last week's session showed you last week's messages over
today's files: the transcript and the code disagreed.

**Checkpoints** close that gap. Every turn snapshots the whole working directory
into an isolated git store, keyed to the conversation length. When you rewind the
conversation — resume a saved session, or backtrack to an earlier message — the
files are restored to how they looked at that point. This is Claude Code's
"checkpointing" / codex's `ThreadRollback`, adapted to this codebase's split of
pure logic from boundary I/O.

## What you get

- **Backtrack resets the code.** Esc-Esc to a previous user message and press
  Enter: history truncates to that message *and* the working directory is
  restored to its state when you first sent it (the end of the previous turn, or
  the pristine tree for the very first message). A `Reset files to the checkpoint
  for that message` toast confirms it.
- **Resume resets the code.** Pick a saved session in the `/resume` picker: its
  transcript loads *and* the working directory is restored to that session's
  **final** checkpoint — even if the files diverged since. A `Restored files to
  this session's checkpoint` toast confirms it.
- **Your real git is never touched.** Snapshots live in a private object store
  under `~/.alter-zero/checkpoints`, with its own `GIT_DIR`, index, config, and
  branch. Your `.git`, staging area, branches, and commits are never read or
  written. A restore's cleanup never deletes your `.git` or vendored dirs.
- **Nothing is truly lost.** A restore is destructive to the current working
  tree, so the current state is snapshotted into the store *first* — recoverable
  via the store's `git reflog`.

## The model

A **checkpoint** is `{ after: usize, commit: String }` (`checkpoint::Checkpoint`):
after `after` finished [`HistoryItem`]s, the working directory looked like the
isolated-store commit `commit`. Checkpoints are recorded:

- once at **session start** (`after == 0`) — the pristine tree, so a backtrack to
  the first message has something to restore;
- once at **every turn end** (`after == history.len()`) — captured before any
  queued follow-up starts editing again, so it is the just-ended turn's code
  state. This is exactly what a later backtrack to the *next* message, or a
  resume, wants.

Both rewinds funnel through one pure decision, `checkpoint::restore_target`:

```
restore_target(checkpoints, target_len) = the commit of the checkpoint with the
                                           greatest `after <= target_len`
```

- **Backtrack** to a user message at history position `P` truncates history to
  `P`, then restores `restore_target(cps, P)` — the snapshot taken at the end of
  the previous turn (whose `after == P`), or the pristine `after == 0` snapshot
  for the first message.
- **`/resume`** restores `restore_target(cps, usize::MAX)` — the final snapshot,
  the code as of the end of the saved conversation.

A truncation also drops the checkpoints describing the rewound-away future
(`checkpoint::retain_surviving` — keep `after <= len`), in lockstep with the
rollout file's truncation rewrite.

### Why `after`-keyed, not item-keyed

Keying to history *length* rather than to a specific item makes the truncation
math trivial (a `<=` filter) and lets one function serve both rewinds: resume is
just backtrack with `target_len = usize::MAX`. It also means a checkpoint whose
turn was rewound away is dropped, never resurrected — the same invariant the
resume rollout file already keeps for its transcript items (`docs/resume.md`).

## The two layers

Split the repo's usual way — a **pure core** (unit-tested) and a **boundary**
(git I/O, integration/smoke-tested), both in [`src/checkpoint.rs`](../src/checkpoint.rs).

### Pure core (`checkpoint`)

- `Checkpoint`, `restore_target`, `retain_surviving` — the mapping above.
- `store_git_dir(root, cwd)` — the per-cwd store path, keyed by the cwd with
  every non-alphanumeric char dashed (the same segmenting as
  `background::tasks_dir`), so all sessions run in a directory share one object
  store: a checkpoint recorded last week is restorable today.
- `CHECKPOINT_EXCLUDES` / `exclude_file_contents` — the `info/exclude` denylist.
  The project's *own* `.gitignore` files already keep most build/vendor dirs out
  of `git add -A` (the shadow store reads them like any git command); this is the
  backstop for dirs without one, and — above all — the user's real `.git`, which
  a restore's `git clean` must never delete.
- `enabled_by_env` — the `ALTER_ZERO_CHECKPOINTS` gate (the `ALTER_ZERO_TOOLS`
  pattern: off for `0`/`false`/`no`/`off`).
- `cwd_scope(cwd, env)` / `CheckpointRefusal` / `CheckpointEnv` — the
  **project-scope guard**, below.
- `SnapshotBudget` / `SnapshotCost` / `parse_size_limit` — the **cost
  ceiling**, below.

## Is this directory worth snapshotting?

The session-start snapshot runs **before the first frame paints**, in raw mode
where Ctrl+C is just an unread key event, and `git add -A` is O(bytes) because
it hashes every file it stages. Measured on this repo's hardware:

| working directory | to first frame | store after one launch |
| --- | --- | --- |
| an ordinary project | 86 ms | 180 kB |
| this repo (5.3 MB) | 368 ms | 3.2 MB |
| a 235 MB / 6 000-file tree | **9 754 ms** | **260 MB** |

So two guards decide, before any of that runs, whether a directory should be
snapshot at all. Both are pure; the boundary injects what they measure against.

### The categorical half — `cwd_scope`

Some directories are not projects, and the cost is only half the reason: a
restore's `git reset --hard` + `git clean -fd` in one of them deletes files
that were never this session's to touch. Refused, each with its own
`CheckpointRefusal`:

- **`FilesystemRoot`** — `/`, a drive root, or an empty unknowable cwd.
- **`HomeDirectory`** — the home directory itself or an ancestor of it
  (`/home`): the user's entire tree (the original "alter0 hangs in `~`" bug).
- **`StateDirectory`** — alter-zero's own `~/.alter-zero` **and everything
  under it**: it holds the rollouts, the input history, and every project's
  checkpoint store, so a restore's `git clean -fd` there deletes other
  sessions' records. A cwd that merely *contains* this session's store is
  **not** refused — see [the self-exclude](#the-store-never-snapshots-itself),
  which is the better answer to that direction.
- **`SystemDirectory`** — a `SYSTEM_TREES` entry (`/proc`, `/sys`, `/dev`,
  `/run`) **or anything under one**, since these are not ordinary filesystems
  at all and `/dev/shm` and `/run/user/N` hold other *live* processes' files;
  or a `SYSTEM_ROOTS` entry (`/usr`, `/etc`, `/root`, `/bin`, `/boot`,
  `/lib*`, `/sbin`, and macOS's `/System`, `/Library`, `/Applications`)
  **itself only** — the whole of `/usr` is nobody's project, but people really
  do keep work in `/usr/local/src` and version `/etc/nginx`, and refusing a
  legitimate project is the worse default.
- **`SharedDirectory`** — a `SHARED_PARENTS` entry (`/tmp`, `/var/tmp`, `/var`,
  `/opt`, `/srv`, `/mnt`, `/media`, `/home`, `/net`, `/export`, `/Users`,
  `/Volumes`, and macOS's `/private/tmp`, `/private/var`) or `$TMPDIR`, and
  **only the directory itself**: `/tmp` holds
  every program on the box's scratch files, while `/tmp/my-project` is an
  ordinary project and checkpoints exactly as before (the smoke suite's
  `mktemp -d` work dirs depend on this). `$TMPDIR` is injected because macOS
  puts it at an unpredictable `/var/folders/xx/yyy/T`.

Matching is component-wise throughout, so a `/home/username` sibling of
`/home/user` still checkpoints and a trailing slash never matters; an empty
injected path counts as unknown rather than as a prefix of everything.

### The store never snapshots itself

The other half of the 2 GB `~/.alter-zero`, and the one that is a *bug* rather
than a scope question. When the checkpoints root lands **inside** the work
tree, every `git add -A` re-stages the previous snapshots' object files, so
the tracked set compounds turn after turn — measured on a 40 MB state dir:
59 → 130 → 269 → 528 tracked files over four snapshots, store 41 MB → 165 MB.

`CHECKPOINT_EXCLUDES` cannot cover it: those are *name* patterns, and the root
is whatever path `ALTER_ZERO_CHECKPOINTS_DIR` points at. So
`store_exclude_line(work_tree, root)` derives an extra `info/exclude` line
whenever the root strips as a prefix of the work tree — **anchored** with a
leading `/` so it matches that directory at the work tree's root and not a
same-named one deeper in the project, with the four gitignore metacharacters
escaped so a literal path stays literal (a store in `ck[1]` must exclude
`ck[1]`, not `ck1`). `CheckpointStore::new` computes it once and
`write_excludes` appends it.

This is why a cwd containing the store is *not* refused: taking the whole
feature away over a configuration choice is a worse answer than simply not
staging the store. It is also defence in depth — even if a scope rule is ever
missed, the store can no longer compound into itself.

### The general half — `SnapshotBudget` and the pre-flight probe

No denylist can name the huge directory that *is* a project. So whatever
survives `cwd_scope` is measured: `CheckpointStore::probe` asks
`git ls-files --others --exclude-standard -z` for exactly the paths
`git add -A` would newly stage and adds their `stat` sizes.

- It **honours `.gitignore`** and the store's `info/exclude`, so a repo whose
  bulk is gitignored is never refused for weight the snapshot would not carry.
  That is why it asks git rather than walking the tree itself.
- It is **~800× cheaper than the thing it is predicting** — 26 ms against
  20 354 ms on a measured 470 MB tree — because git walks and stats but never
  reads content.
- It is **bounded three ways** (`max_files`, `max_bytes`,
  `max_time`) and kills the child the moment one trips, so a pathological tree
  costs the budget rather than the walk.
- On a **warm** store it naturally reports only what is new — which is exactly
  what that snapshot will hash, so a big-but-already-captured tree keeps its
  checkpoints across sessions.

Past the budget the store is retired for the session (`CheckpointStore::disable`
— capability, not just the enabled flag, so `/settings` reports the row
unavailable instead of offering a toggle that would re-arm a measured stall).

The defaults are **20 000 files** and **256 MiB**, with a 500 ms probe
deadline. The byte cap is deliberately *not* calibrated as a time — hashing
throughput varies ~20× across disks. It is a statement about what a project
is: a working tree holding more than that much non-ignored content is a data,
media, or scratch directory, and whole-tree snapshots every turn are the wrong
tool for one however fast the disk. The side effect is a cold snapshot around
a second on ordinary hardware and several on the slowest — the price of not
refusing real projects, which is the worse of the two failures.

Running out of *time* is its own verdict (`ProbeOutcome::OutOfTime` →
`CheckpointRefusal::TooSlow`), never folded into "too big": a probe that timed
out has learned nothing about the tree's size, so reporting the handful of
files it managed to count as the reason would be a lie.

Both caps are overridable, each accepting a plain count or a `k`/`m`/`g`
suffix, and each taking **`0` as no limit** (the `/settings` **Max tool calls**
convention):

```
ALTER_ZERO_CHECKPOINT_MAX_FILES=50000
ALTER_ZERO_CHECKPOINT_MAX_BYTES=2g
ALTER_ZERO_CHECKPOINT_MAX_BYTES=0     # no byte ceiling
```

### Saying so

Every refusal raises a one-row `Checkpoints off — {reason}` toast on the first
frame (`CheckpointRefusal`'s `Display`), and `/settings` shows **Checkpoints**
as `false (unavailable)`. The toast is suppressed when the user had already
turned checkpoints off — a refusal is only news when it took something away.
Disabling itself in silence is what made this hard to place: "alter0 takes
seconds to boot in `/tmp`" and "checkpoints do nothing here" were the same
fact seen from two sides.

The refusal toast has a sibling for the snapshot that **does** run: the
pre-flight **announcement**. The session-start snapshot hashes the whole cwd
before the first frame paints, so on a big cold tree the terminal sits frozen
for seconds with nothing saying why. `seed_checkpoints` now binds the probe's
`SnapshotCost` (it used to feed straight into `.refusal()` and drop the
numbers) and, when the tree fits the budget, commits the pure
`checkpoint::snapshot_notice` line — `Snapshotting 326 files (7.9 MB) for
checkpoints…`, `human_bytes` and the refusal's own pluralisation, so the pair
reads as one family — through `ui::startup_notice_lines` (the banner's indent
and dim meta colour) and **forces one frame** (`draw_conversation`) before
`snapshot("session start")` blocks: `insert_before` only queues, and the
loop's first draw tick sits on the far side of the snapshot. Committed ahead
of `paint_first_frame`'s banner, the row lands **above the banner** in
scrollback. It is chrome (never in `history`, never re-emitted by a purge
rebuild — a one-time startup fact, so it disappears on the first resize or
`/clear`, deliberately unlike the banner's `banner_tail`), and it is gated on
the probe's `files > 0`: a warm store with nothing new — every ordinary
relaunch — probes as zero and stays quiet, costing no extra frame
(`probe_shrinks_once_the_store_is_warm` is what makes that gate honest).
Turn-end snapshots stay unannounced: a warm store only hashes what changed,
and naming that would cost a per-turn probe.

### Boundary (`CheckpointStore`)

An isolated git object store. Every git command runs with a private `GIT_DIR`
(under the checkpoints root), the cwd as its **detached work tree**, a store-local
index (`GIT_INDEX_FILE`), and config fully isolated from the user's system/global
git (`GIT_CONFIG_NOSYSTEM`, a `GIT_CONFIG_GLOBAL` pointing at an absent
store-local file), no prompts, all stdio detached.

- **`probe(budget)`** — what the next snapshot would cost, measured without
  paying it (above). Inert on a disabled store, and permissive on any failure
  to run git: the probe never disables the feature on its own uncertainty.
- **`disable()`** — retire the store for the session (the probe's verdict).
  Unlike `set_enabled(false)` it drops **capability**, so `/settings` reports
  the row unavailable.
- **`init`** — `git init --initial-branch=checkpoints` (a fixed branch so `HEAD`
  is valid for the first commit regardless of the user's `init.defaultBranch`),
  write `info/exclude`, and set store-local config: an identity (so `commit`
  works without the user's), `commit.gpgsign=false` + `core.hooksPath=/dev/null`
  (never hang or fork on a hook/signature), and `gc.auto=0` (keep every snapshot
  reachable across the reset a restore performs). Idempotent.
- **`snapshot(msg) -> Option<String>`** — `git add -A` (honouring the work
  tree's `.gitignore` and our `info/exclude`) then `git commit --allow-empty
  --no-verify` (empty allowed, so a no-change turn still maps to a distinct
  restorable SHA), returning `HEAD`. `None` on disable/failure — the TUI never
  dies for a checkpoint.
- **`restore(commit) -> io::Result<bool>`** — verify the commit exists in this
  store (`cat-file -e`; a session recorded in a *different* cwd is unknown → the
  restore is a graceful `Ok(false)`), then `git reset --hard <commit>` (tracked
  files back to the snapshot) + `git clean -fd` (remove files created since,
  honouring the excludes so `.git`/vendor dirs survive). `Ok(true)` on success.

> One non-obvious git detail the tests lock in: `git()` presets the command's
> stdout to null (right for the many `status()` callers), and `Command::output`
> **keeps** an explicit stdout setting — so `head()` must re-pipe stdout or the
> SHA is silently swallowed and every snapshot returns `None`.

## Wiring (`main.rs`, the boundary)

- **Store + pristine snapshot** are created next to the `SessionRecorder`
  (`tui::bootstrap`), gated by
  `checkpoint::enabled_by_env(ALTER_ZERO_CHECKPOINTS)` **and**
  `checkpoint::cwd_scope(cwd, env)` — the categorical guard above, fed `$HOME`,
  `config::config_home()`, `config::checkpoints_root()` and `config::tmp_dir()`
  — **and** a `git` binary being present (probed last, so a refused cwd never
  even spawns git). Root: `ALTER_ZERO_CHECKPOINTS_DIR`, else
  `~/.alter-zero/checkpoints` (the `ALTER_ZERO_SESSIONS_DIR` pattern; the smoke
  test points it at a temp dir).
- **The pre-flight probe** then runs in `seed_checkpoints`, between `init` and
  the pristine snapshot — the last moment before the first frame at which the
  feature can still decide not to run. Over budget it calls `disable()`,
  re-syncs `SettingAvailability`, toasts the reason and takes no snapshot.
- **Turn-end snapshot** rides `dispatch_after_turn` — the single choke point every
  turn end funnels through (`StreamDone`/`Error`, the Esc interrupt, an idle
  background completion) — via `checkpoint_turn_end`, *before* the next queued
  turn dispatches. So even back-to-back queued turns each get their own
  checkpoint.
- **Recording.** `SessionRecorder` grew a `checkpoints` in-memory mirror + a
  `checkpoints_written` watermark (the `recorded` twin). `record_checkpoint` holds
  a snapshot in memory; the next `sync` flushes it interleaved with the item lines
  — but a checkpoint **alone never materializes a file**, so an empty session
  (only the startup snapshot) still leaves no rollout file (codex's deferred
  create). A truncation rewrite re-emits the survivors; `adopt` (resume) takes
  over the file's own checkpoints; `start_new` (`/clear`) clears them and a fresh
  pristine snapshot re-seeds the chain.
- **Restore triggers.** The `ResumeSession` arm parses the file's checkpoints
  (`session::parse_checkpoints`), backs up the current tree, and restores the
  final one before the purge-repaint. The new `ConfirmBacktrack` action (the
  Enter arm of the backtrack preview now returns it instead of `ToggleToolView`)
  restores `restore_target(recorder.checkpoints(), history.len())` before a
  **purge-rebuild** repaint (backtrack shrinks history, so — like `/resume` — an
  in-place overwrite would leave the dropped exchange stale in scrollback;
  `docs/backtrack.md`); the loop-bottom `recorder.sync` then rewrites the file,
  dropping the rewound-away checkpoint lines.

## The rollout line (`session`)

Checkpoints ride the *same* JSONL file as the transcript, as a
`{"type":"checkpoint","payload":{"commit","after"}}` line
(`session::checkpoint_line`). They are **invisible to the transcript parse**
(`parse_session` skips them, so a resumed conversation is byte-identical whether
or not checkpoints exist) and read by a dedicated sidecar `parse_checkpoints`.
Old builds skip the unknown record type — the forward-compatibility contract.

## Enabling / disabling

On by default when a `git` binary is present **and the cwd is a project worth
snapshotting** (both halves of the guard above). `ALTER_ZERO_CHECKPOINTS=0` (or
`false`/`no`/`off`) disables it, as does the `/settings` **Checkpoints** knob; a
missing git or no writable root disables it silently.

Both guards are **unconditional** — an explicit `ALTER_ZERO_CHECKPOINTS=1`
doesn't override either, because they are answers to "can this run without
wrecking the session?", not preferences. The cost ceiling is the one with a
dial: raise or remove `ALTER_ZERO_CHECKPOINT_MAX_FILES` /
`ALTER_ZERO_CHECKPOINT_MAX_BYTES` to checkpoint a tree bigger than the default
budget. For the categorical refusals the answer is to run in a project
directory.

A disabled store makes every operation an inert no-op, so `/resume` and
backtrack behave exactly as they did before this feature — transcript only.

> **Running the test suite:** `scripts/smoke.sh` drives the real binary *inside
> this repo's working directory*, and a restore's `git clean` would delete files
> not captured in a snapshot. So the suite runs every repo-cwd phase with
> `ALTER_ZERO_CHECKPOINTS=0` and enables checkpoints only in the phases that run
> in a throwaway temp directory (Phases 46–47, 49, 71). Keep that gating if you
> add phases that resume/backtrack.

## Known limitations (v1)

- **Whole-cwd snapshots.** A checkpoint captures the entire working directory
  (minus the excludes), so a restore reverts *all* changes since that point,
  including files you edited by hand — this is the "git reset" semantic the
  feature is named for. The pre-restore backup + `git reflog` are the safety net.
  Surgical, agent-touched-only restores are future work.
- **Synchronous git — one slow first snapshot per directory.** The
  session-start snapshot runs on the loop's thread before the first frame, and
  turn-end snapshots at idle boundaries. Only the **first** run in a directory
  pays real time; a warm store's `git add -A` stats rather than hashes.
  Measured on this repo's hardware (a VM whose git hashes at ~23 MB/s, roughly
  a tenth of an SSD):

  | working tree | launch 1 | launch 2 | launch 3 |
  | --- | --- | --- | --- |
  | this repo (5.3 MB) | 368 ms | ~80 ms | ~80 ms |
  | a 201 MB project | 10 134 ms | 71 ms | 81 ms |

  So the byte cap is the knob that decides how bad that one launch may get,
  and it cannot be calibrated as a time: at 256 MiB this machine spends ten
  seconds where an SSD spends under one. Lower
  `ALTER_ZERO_CHECKPOINT_MAX_BYTES` on slow storage. The only thing that would
  *remove* the pause rather than bound it is moving the cold snapshot to a
  worker thread (the Ctrl+V image pipeline's shape) — still future work, and
  it needs care: a background `git add -A` must never overlap a restore's
  `git reset --hard`. The *pathological* cases are refused outright by
  `cwd_scope` and the probe rather than paused for, and those refusals do not
  depend on the cap.
- **No pruning.** `gc.auto=0` keeps every snapshot restorable across the fork a
  restore creates, so the store grows slowly and unbounded. A retention policy is
  future work.
- **Cwd-scoped.** Checkpoints are keyed by working directory; resuming a session
  recorded in a *different* directory finds no matching commits and leaves the
  code untouched (a graceful no-op, not an error).
- **No UI to browse/restore checkpoints directly** — they are driven only by the
  resume/backtrack gestures. The pre-restore backups are reachable only via the
  store's `git reflog`.

## Tests

- `checkpoint` (pure): `restore_target` picks the latest snapshot at or before a
  rewind (and `usize::MAX` = the final one), breaks ties toward the most recent,
  and is `None` when nothing qualifies; `retain_surviving` drops snapshots past a
  truncation; `store_git_dir` dashes the cwd like the tasks dir;
  `exclude_file_contents` always shields `.git`; `enabled_by_env` reads the
  disabling words; `cwd_scope` refuses the home dir (trailing-slash `$HOME`
  included), its ancestors, filesystem roots, the state dir in both directions,
  every system tree *and its subtree*, and each shared parent (plus `$TMPDIR`)
  *itself only* — while accepting project dirs, including a `/home/username`
  sibling (component matching, not prefix), `/tmp/scratch`, and a `mktemp -d`
  under `$TMPDIR`; empty injected paths count as unknown; a shared parent that
  also holds the store reports being shared. `SnapshotBudget::exceeded` flags
  either cap and treats `0` as no limit; `parse_size_limit` reads plain counts
  and `k`/`m`/`g` suffixes, keeps `0`, and falls back on garbage; every
  `CheckpointRefusal` renders one line.
- `session` (pure): a `checkpoint` line round-trips through `checkpoint_line` /
  `parse_checkpoints` in file order, and checkpoint lines are invisible to
  `parse_session`.
- `tests/checkpoint_store.rs` (real git): after edits/creates/deletes, restoring
  an earlier snapshot returns the tree exactly (edits reverted, new files removed,
  deletions undone); a restore never deletes the user's `.git` or vendored dirs;
  an unknown commit is a graceful no-op; a disabled store is inert. And for the
  probe: it counts the files a snapshot would stage with their bytes, is blind
  to `.gitignore`d and `info/exclude`d trees, stops early past a file cap,
  catches a few enormous files a file cap alone would wave through, shrinks to
  just the new files once the store is warm, and is inert when disabled.
- `scripts/smoke.sh` Phases 46–47 (the real binary, in a temp cwd): an Esc-Esc
  backtrack reverts a `!`-mutated working file to its pristine checkpoint; a
  `/resume` restores a working file to the saved session's final checkpoint even
  after it was diverged on disk between launches. Phase 49: launched with
  cwd == `$HOME` (even under an explicit `ALTER_ZERO_CHECKPOINTS=1`) the store
  is never created, while a project dir under that same home still snapshots.
  Phase 71: launching in `/tmp` or in the state dir never creates the store and
  toasts the reason; a tree past a forced-tiny `ALTER_ZERO_CHECKPOINT_MAX_BYTES`
  toasts "too big to snapshot per turn" and leaves the store with no commit;
  the same directory under the default budget snapshots with no toast.

[`HistoryItem`]: ../src/app/types.rs
