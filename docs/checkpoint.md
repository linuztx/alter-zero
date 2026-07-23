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

### Boundary (`CheckpointStore`)

An isolated git object store. Every git command runs with a private `GIT_DIR`
(under the checkpoints root), the cwd as its **detached work tree**, a store-local
index (`GIT_INDEX_FILE`), and config fully isolated from the user's system/global
git (`GIT_CONFIG_NOSYSTEM`, a `GIT_CONFIG_GLOBAL` pointing at an absent
store-local file), no prompts, all stdio detached.

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

- **Store + pristine snapshot** are created next to the `SessionRecorder`, gated
  by `checkpoint::enabled_by_env(ALTER_ZERO_CHECKPOINTS)` **and** a `git` binary
  being present. Root: `ALTER_ZERO_CHECKPOINTS_DIR`, else
  `~/.alter-zero/checkpoints` (the `ALTER_ZERO_SESSIONS_DIR` pattern; the smoke
  test points it at a temp dir).
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
  restores `restore_target(recorder.checkpoints(), history.len())` before the
  return-from-overlay repaint; the loop-bottom `recorder.sync` then rewrites the
  file, dropping the rewound-away checkpoint lines.

## The rollout line (`session`)

Checkpoints ride the *same* JSONL file as the transcript, as a
`{"type":"checkpoint","payload":{"commit","after"}}` line
(`session::checkpoint_line`). They are **invisible to the transcript parse**
(`parse_session` skips them, so a resumed conversation is byte-identical whether
or not checkpoints exist) and read by a dedicated sidecar `parse_checkpoints`.
Old builds skip the unknown record type — the forward-compatibility contract.

## Enabling / disabling

On by default when a `git` binary is present. `ALTER_ZERO_CHECKPOINTS=0` (or
`false`/`no`/`off`) disables it; a missing git or no writable root disables it
silently. A disabled store makes every operation an inert no-op, so `/resume` and
backtrack behave exactly as they did before this feature — transcript only.

> **Running the test suite:** `scripts/smoke.sh` drives the real binary *inside
> this repo's working directory*, and a restore's `git clean` would delete files
> not captured in a snapshot. So the suite runs every repo-cwd phase with
> `ALTER_ZERO_CHECKPOINTS=0` and enables checkpoints only in the two phases that
> run in a throwaway temp directory (Phases 46–47). Keep that gating if you add
> phases that resume/backtrack.

## Known limitations (v1)

- **Whole-cwd snapshots.** A checkpoint captures the entire working directory
  (minus the excludes), so a restore reverts *all* changes since that point,
  including files you edited by hand — this is the "git reset" semantic the
  feature is named for. The pre-restore backup + `git reflog` are the safety net.
  Surgical, agent-touched-only restores are future work.
- **Synchronous git.** Snapshots run at idle turn boundaries and restores at a
  user action, so a brief pause on a very large tree is possible. Offloading to a
  worker thread (like the Ctrl+V image pipeline) is future work.
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
  disabling words.
- `session` (pure): a `checkpoint` line round-trips through `checkpoint_line` /
  `parse_checkpoints` in file order, and checkpoint lines are invisible to
  `parse_session`.
- `tests/checkpoint_store.rs` (real git): after edits/creates/deletes, restoring
  an earlier snapshot returns the tree exactly (edits reverted, new files removed,
  deletions undone); a restore never deletes the user's `.git` or vendored dirs;
  an unknown commit is a graceful no-op; a disabled store is inert.
- `scripts/smoke.sh` Phases 46–47 (the real binary, in a temp cwd): an Esc-Esc
  backtrack reverts a `!`-mutated working file to its pristine checkpoint; a
  `/resume` restores a working file to the saved session's final checkpoint even
  after it was diverged on disk between launches.

[`HistoryItem`]: ../src/app.rs
